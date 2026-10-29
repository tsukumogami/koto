use anyhow::Result;

use crate::cli::ChildrenPolicy;
use crate::engine::types::{ValidatedCoordId, ValidatedSessionId};
use crate::session::cloud::{ChildResolution, CloudBackend};
use crate::session::recover;
use crate::session::Backend;
use crate::session::SessionBackend;

/// Maximum byte length accepted for the `--inputs` JSON payload.
///
/// Caps the parser memory footprint at parse time (Security
/// Considerations defense-in-depth #1, design lines 2007-2010). A
/// 1 MiB ceiling is far more than any legitimate dispatch input and
/// still keeps the worst-case allocation bounded.
pub(super) const INPUTS_MAX_BYTES: usize = 1024 * 1024;

/// Maximum JSON nesting depth accepted for the `--inputs` payload.
///
/// Pairs with [`INPUTS_MAX_BYTES`] to defend against parser-memory
/// bombs that stay under the size cap by recursing through deeply
/// nested arrays / objects.
pub(super) const INPUTS_MAX_DEPTH: usize = 128;

/// Walk a parsed [`serde_json::Value`] and return the maximum
/// nesting depth (an empty leaf has depth 1).
///
/// Used after the size-bounded parse to enforce
/// [`INPUTS_MAX_DEPTH`]. Implemented iteratively (no recursion) so a
/// deeply nested input cannot blow our own stack while we're
/// measuring its depth.
fn json_max_depth(value: &serde_json::Value) -> usize {
    let mut max_depth = 1usize;
    let mut stack: Vec<(&serde_json::Value, usize)> = vec![(value, 1)];
    while let Some((v, d)) = stack.pop() {
        if d > max_depth {
            max_depth = d;
        }
        match v {
            serde_json::Value::Array(items) => {
                for item in items {
                    stack.push((item, d + 1));
                }
            }
            serde_json::Value::Object(map) => {
                for (_, item) in map {
                    stack.push((item, d + 1));
                }
            }
            _ => {}
        }
    }
    max_depth
}

/// Parse the `--inputs` flag value into a [`serde_json::Value`],
/// enforcing the two DoS guards from Security Considerations
/// defense-in-depth #1:
///
/// 1. **Size cap.** Reject before invoking the parser if the raw
///    string exceeds [`INPUTS_MAX_BYTES`]. The check is on the
///    UTF-8 byte length, not the character count, so multi-byte
///    payloads cannot evade it.
/// 2. **Depth cap.** Parse first (the byte cap already bounds the
///    parser's work), then walk the resulting value tree
///    iteratively to enforce [`INPUTS_MAX_DEPTH`].
fn parse_inputs(raw: &str) -> anyhow::Result<serde_json::Value> {
    if raw.len() > INPUTS_MAX_BYTES {
        anyhow::bail!(
            "--inputs payload too large: {} bytes (max {} = 1 MiB)",
            raw.len(),
            INPUTS_MAX_BYTES,
        );
    }
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| anyhow::anyhow!("--inputs is not valid JSON: {}", e))?;
    let depth = json_max_depth(&value);
    if depth > INPUTS_MAX_DEPTH {
        anyhow::bail!(
            "--inputs JSON nests {} levels deep (max {})",
            depth,
            INPUTS_MAX_DEPTH,
        );
    }
    Ok(value)
}

/// Implements `koto session start --parent <p>` (Issue 4).
///
/// Two paths:
///
/// 1. **Plain start** — when `needs_agent` is `false` and none of
///    `role` / `template` / `inputs` / `coordinator_of_record` are
///    set, write a child header with `needs_agent = None`. The
///    session can later be dispatched via a separate authoring
///    flow.
/// 2. **Dispatch-request start** — when `needs_agent` is `true`,
///    require the full `role` / `template` / `inputs` companion
///    set, parse and validate `inputs` (1 MiB max, 128-level
///    nesting max — Security Considerations defense-in-depth #1),
///    and populate the new request-store header fields:
///    `needs_agent = Some(true)`, `role`, `inputs`,
///    `coordinator_of_record`, `requested_by`, `dispatch_epoch = 0`,
///    `template_name` (reusing the existing field per Decision 1
///    line 222 rather than introducing a separate `template`).
///
/// Companion-flag contract: when `needs_agent` is unset and any of
/// `role` / `template` / `inputs` / `coordinator_of_record` is
/// present, reject naming `--needs-agent` as the missing flag.
///
/// All caller-supplied ids — `parent`, `coordinator_of_record`,
/// and the auto-derived `requested_by` — flow through
/// [`ValidatedSessionId`] / [`ValidatedCoordId`] from Issue 3
/// before any path operation, closing the shell-injection and
/// path-traversal surface (Security Considerations N1 / N5).
#[allow(clippy::too_many_arguments)]
pub fn handle_start(
    backend: &dyn SessionBackend,
    name: &str,
    parent: &str,
    needs_agent: bool,
    role: Option<&str>,
    template: Option<&str>,
    inputs: Option<&str>,
    coordinator_of_record: Option<&str>,
) -> Result<()> {
    use crate::engine::types::{
        generate_session_id, now_iso8601, Event, EventPayload, StateFileHeader,
    };

    // -- Companion-flag contract (Decision 1) --
    //
    // Validate the request-store flag set BEFORE we touch the
    // filesystem so parse-time rejection is observable from CLI
    // exit codes without any side effects.
    if needs_agent {
        let mut missing: Vec<&'static str> = Vec::new();
        if role.is_none() {
            missing.push("--role");
        }
        if template.is_none() {
            missing.push("--template");
        }
        if inputs.is_none() {
            missing.push("--inputs");
        }
        if !missing.is_empty() {
            anyhow::bail!(
                "--needs-agent requires {} (missing: {})",
                "--role, --template, --inputs",
                missing.join(", "),
            );
        }
    } else {
        let mut orphans: Vec<&'static str> = Vec::new();
        if role.is_some() {
            orphans.push("--role");
        }
        if template.is_some() {
            orphans.push("--template");
        }
        if inputs.is_some() {
            orphans.push("--inputs");
        }
        if coordinator_of_record.is_some() {
            orphans.push("--coordinator-of-record");
        }
        if !orphans.is_empty() {
            anyhow::bail!("{} requires --needs-agent", orphans.join(", "),);
        }
    }

    // -- Newtype validation of caller-supplied ids --
    //
    // `parent` is operator-controlled and immediately feeds into a
    // path lookup; route it through `ValidatedSessionId::new` first
    // so injection attempts (e.g., `--parent ../etc/passwd`) reject
    // with the newtype's typed error before any filesystem call.
    let validated_parent = ValidatedSessionId::new(parent)?;

    // -- Validate the new session name with the existing rule --
    //
    // The new session name uses the workspace's existing workflow-
    // name validator (it's the same shape as `koto init` requires).
    if let Err(msg) = crate::discover::validate_workflow_name(name) {
        anyhow::bail!(msg);
    }

    if !backend.exists(validated_parent.as_str()) {
        anyhow::bail!("parent workflow '{}' not found", validated_parent);
    }

    if backend.exists(name) {
        anyhow::bail!(
            "session '{}' already exists; run `koto session cleanup {}` first",
            name,
            name,
        );
    }

    // Read the parent's header so we can default `requested_by` and
    // `coordinator_of_record` from it. The CLI surface treats the
    // *parent* as the spawning context: the parent's session id is
    // the subagent that's requesting the dispatch, and the parent's
    // coordinator (or its session id if pre-request-store) is the default
    // coordinator-of-record.
    let parent_header = backend
        .read_header(validated_parent.as_str())
        .map_err(|e| anyhow::anyhow!("failed to read parent header: {}", e))?;

    let coord_id_str = match coordinator_of_record {
        Some(c) => c.to_string(),
        None => parent_header
            .coordinator_of_record
            .clone()
            .unwrap_or_else(|| parent_header.session_id.clone()),
    };
    // If the parent itself was pre-request-store and carries an empty
    // session_id, fall back to the validated parent name so the
    // newtype constructor still has a non-empty input to validate
    // (and the resulting header records a meaningful identifier).
    let coord_id_str = if coord_id_str.is_empty() {
        validated_parent.as_str().to_string()
    } else {
        coord_id_str
    };
    let validated_coord = ValidatedCoordId::new(&coord_id_str)?;

    // `requested_by` is the spawning subagent's session id (the
    // parent's koto session id). Pre-request-store parents have an empty
    // session_id; fall back to the parent name so the field carries
    // a stable identifier even on legacy state files.
    let requested_by_str = if parent_header.session_id.is_empty() {
        validated_parent.as_str().to_string()
    } else {
        parent_header.session_id.clone()
    };
    let validated_requested_by = ValidatedSessionId::new(&requested_by_str)?;

    // -- Parse and depth-check `--inputs` if present --
    let parsed_inputs = match inputs {
        Some(raw) => Some(parse_inputs(raw)?),
        None => None,
    };

    // -- Recursion-cap enforcement (Issue 17, PRD R29) --
    //
    // The three hard-coded dimensions (depth, fanout, total-unassigned)
    // fire ONLY for --needs-agent spawns; plain `koto session start`
    // doesn't participate in the request-store protocol and isn't
    // subject to the caps. Validation runs BEFORE any disk write so a
    // cap rejection leaves no on-disk side effects.
    if needs_agent {
        // The total-unassigned counter consults the terminal index at
        // `<koto_root>/_terminal_index.jsonl`. `koto_root` is the
        // workspace root the SessionBackend uses; for the LocalBackend
        // that's `<base_dir>/..` (the `.koto` directory containing the
        // `sessions/` subdir). Use the koto root (`~/.koto` unless
        // `KOTO_HOME` moves it) when it resolves; on substrates that
        // don't have one we leave `koto_root` as the literal `.koto`
        // path which simply means the terminal-index filter sees an
        // empty index (safe over-count, never an under-count).
        let koto_root =
            crate::koto_home::koto_home().unwrap_or_else(|| std::path::PathBuf::from(".koto"));
        let outcome = crate::engine::caps::validate_recursion_caps(
            backend,
            validated_parent.as_str(),
            &koto_root,
        )
        .map_err(|e| anyhow::anyhow!("recursion-cap validation failed: {}", e))?;
        // Emit warn-level logs for any soft-cap hits. Each dimension
        // is reported independently so operators reading the log see
        // which threshold was crossed.
        for warn in outcome.warnings() {
            if let crate::engine::caps::CapEvaluation::Warn {
                dimension,
                threshold,
                observed,
            } = warn
            {
                eprintln!(
                    "warning: recursion-cap soft threshold reached ({dimension}): observed {observed}, warn at {threshold}"
                );
            }
        }
        // Hard-reject short-circuit. The orchestrator already evaluated
        // all three dimensions; pull the first rejecting one and
        // surface it as a typed EngineError mapped to exit code 64
        // (EX_USAGE).
        if let Some(crate::engine::caps::CapEvaluation::Reject {
            dimension,
            threshold,
            observed,
        }) = outcome.first_reject()
        {
            return Err(anyhow::anyhow!(
                "recursion cap exceeded ({dimension}): observed {observed}, hard reject at {threshold}"
            ));
        }
    }

    // A child session takes its parent's recorded environment
    // (DESIGN-koto-fixed-environment.md, R8), or records this process's
    // when the parent has none yet. The recording report is dropped on
    // purpose: `koto session start` prints the session's identity, not an
    // init response, and a parent with no record adopts one on its own
    // next tick.
    let (command_environment, _) = crate::cli::init_child::resolve_command_environment(
        backend,
        Some(validated_parent.as_str()),
        false,
    )
    .map_err(|msg| anyhow::anyhow!(msg))?;

    // The session's run identity, fixed now from the parent's header (see
    // `crate::run_journal`).
    let (root_session_id, parent_session_id) =
        crate::run_journal::child_lineage(backend, validated_parent.as_str());

    // -- Compose the header --
    let ts = now_iso8601();
    let template_name = template.map(|s| s.to_string());

    let header = StateFileHeader {
        command_environment: Some(command_environment),
        schema_version: 1,
        workflow: name.to_string(),
        template_hash: String::new(),
        created_at: ts.clone(),
        parent_workflow: Some(validated_parent.as_str().to_string()),
        template_source_dir: None,
        template_source_file: None,
        origin: None,
        execution_dir: None,
        session_id: generate_session_id(),
        intent: None,
        template_name,
        needs_agent: if needs_agent { Some(true) } else { None },
        role: role.map(|s| s.to_string()),
        inputs: parsed_inputs,
        coordinator_of_record: Some(validated_coord.as_str().to_string()),
        requested_by: Some(validated_requested_by.as_str().to_string()),
        assignment_claim: None,
        dispatch_epoch: 0,
        priority: None,
        deadline: None,
        retry_count: None,
        agent_config: None,
        kind: None,
        root_session_id,
        parent_session_id,
        respawn_generation: None,
    };

    // For a plain `koto session start` (no `--needs-agent`), the
    // header's coordinator_of_record / requested_by are still
    // populated for traceability but only the dispatch-request
    // branch should expose them. Clear them for the plain path so
    // pre-request-store readers don't observe unexpected fields on a header
    // that isn't requesting dispatch.
    let header = if needs_agent {
        header
    } else {
        StateFileHeader {
            coordinator_of_record: None,
            requested_by: None,
            ..header
        }
    };

    // -- Write a minimal initial event log so the file is well-formed --
    //
    // The first event is a `WorkflowInitialized` marker. We don't
    // have a compiled template at this stage (no `--template` for
    // the plain branch, and the dispatch branch records only the
    // template *name*); the empty `template_path` makes that
    // explicit. Downstream consumers that need a compiled template
    // path can attach one later via the existing init flow.
    let init_payload = EventPayload::WorkflowInitialized {
        template_path: String::new(),
        variables: Default::default(),
        spawn_entry: None,
    };
    let initial_events = vec![Event {
        seq: 1,
        timestamp: ts.clone(),
        event_type: init_payload.type_name().to_string(),
        payload: init_payload,
        idempotency_hash: None,
    }];

    backend
        .create(name)
        .map_err(|e| anyhow::anyhow!("failed to create session directory: {}", e))?;

    backend
        .init_state_file(name, header, initial_events)
        .map_err(|e| anyhow::anyhow!("failed to write session state file: {}", e))?;

    // Echo a small JSON object so CLI consumers can program against
    // the result; mirrors `Command::Init`'s `{name, state}` line.
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "name": name,
            "parent": validated_parent.as_str(),
            "needs_agent": needs_agent,
        }))?
    );
    Ok(())
}

// Run unit tests for the input-validation helpers without setting
// up a backend.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_inputs_accepts_typical_json() {
        let v = parse_inputs(r#"{"draft": "foo", "n": 1}"#).expect("must accept");
        assert_eq!(v["draft"], serde_json::json!("foo"));
        assert_eq!(v["n"], serde_json::json!(1));
    }

    #[test]
    fn parse_inputs_rejects_malformed_json() {
        let err = parse_inputs("{not json}").expect_err("must reject malformed");
        assert!(err.to_string().contains("not valid JSON"), "got {}", err);
    }

    #[test]
    fn parse_inputs_rejects_oversized_payload() {
        // 1 MiB + 1 byte of valid JSON ("a" repeated inside a string)
        let mut s = String::with_capacity(INPUTS_MAX_BYTES + 16);
        s.push('"');
        s.push_str(&"a".repeat(INPUTS_MAX_BYTES));
        s.push('"');
        let err = parse_inputs(&s).expect_err("must reject oversize");
        assert!(err.to_string().contains("too large"), "got {}", err);
    }

    #[test]
    fn parse_inputs_rejects_overnested_json() {
        // Build a 200-level-deep array: "[[[...]]]" wrapping a leaf.
        let mut s = String::new();
        let depth = 200;
        for _ in 0..depth {
            s.push('[');
        }
        s.push('0');
        for _ in 0..depth {
            s.push(']');
        }
        let err = parse_inputs(&s).expect_err("must reject overnest");
        // serde_json's built-in recursion limit (128) typically fires
        // before our own depth walk; either rejection path counts as
        // honoring the AC. Payloads that slip past serde's limit but
        // exceed our cap surface the "nests N levels deep" message.
        let msg = err.to_string();
        assert!(
            msg.contains("nests") || msg.contains("recursion limit"),
            "expected depth rejection, got {}",
            msg
        );
    }

    #[test]
    fn parse_inputs_accepts_boundary_depth() {
        // 128 levels exactly should be the maximum allowed.
        let mut s = String::new();
        let depth = INPUTS_MAX_DEPTH;
        for _ in 0..(depth - 1) {
            s.push('[');
        }
        s.push('0');
        for _ in 0..(depth - 1) {
            s.push(']');
        }
        parse_inputs(&s).expect("128-level nesting must be accepted");
    }

    #[test]
    fn json_max_depth_counts_arrays_and_objects() {
        let v = serde_json::json!({"a": [1, [2, [3]]]});
        // Walk: object(1) → "a"-value array(2) → element array(3) →
        // element array(4) → leaf `3` (5). Containers and scalar
        // leaves alike count, so the deepest path is 5.
        assert_eq!(json_max_depth(&v), 5);
    }
}

/// Append an `IntentUpdated` event to the named session's log.
///
/// The event goes through `backend.append_event`, the store's commit
/// funnel, like every other event. Only the event is appended: the
/// session header is never written here, and a change to it would go
/// through `SessionBackend::rewrite_header`.
pub fn handle_update(backend: &dyn SessionBackend, name: &str, intent: &str) -> anyhow::Result<()> {
    use crate::engine::types::{now_iso8601, EventPayload};

    if intent.len() > 1024 {
        anyhow::bail!(
            "intent string too long: {} characters (max 1024)",
            intent.len()
        );
    }

    if !backend.exists(name) {
        anyhow::bail!("session '{}' does not exist", name);
    }

    let payload = EventPayload::IntentUpdated {
        intent: intent.to_string(),
    };
    backend.append_event(name, &payload, &now_iso8601())?;
    Ok(())
}

/// `koto session link <session> --dispatched-by <name>`: record a
/// dispatched-by link to `dispatched_by` in `session`, and only there.
///
/// Every check runs before anything is written: the session name, the
/// dispatcher name (the session-name rule and the context-key grammar,
/// and not the session itself), that the session exists and, under the
/// cloud backend, that it wasn't imported elsewhere. Each refusal exits 2
/// with `{"error", "command"}`; a write that fails exits 3. Prints
/// `{"name", "kind", "peer", "written"}`, `written: false` when the link
/// was already there and nothing changed.
pub fn handle_link(backend: &Backend, session: &str, dispatched_by: &str) -> Result<()> {
    use crate::session::links::{self, LinkHints, LinkKind};

    let kind = LinkKind::DispatchedBy;
    let refuse = |error: String| -> ! {
        super::exit_with_error_code(
            serde_json::json!({ "error": error, "command": "session link" }),
            super::EXIT_CALLER_ERROR,
        )
    };

    if let Err(e) = ValidatedSessionId::new(session) {
        refuse(e.to_string());
    }
    if let Err(e) = links::validate_peer(kind, dispatched_by) {
        refuse(e.to_string());
    }
    if dispatched_by == session {
        refuse(format!(
            "session '{session}' can't be recorded as dispatched by itself"
        ));
    }
    if !backend.exists(session) {
        refuse(format!("session '{session}' does not exist"));
    }
    if let Err(e) = backend.check_not_migrated(session) {
        refuse(e.to_string());
    }

    let hints = LinkHints::for_peer(backend, dispatched_by);
    let outcome = match links::write_link(backend, session, kind, dispatched_by, &hints) {
        Ok(outcome) => outcome,
        Err(e) => super::exit_with_error_code(
            serde_json::json!({
                "error": format!("could not record the link in '{session}': {e}"),
                "command": "session link"
            }),
            super::EXIT_INFRASTRUCTURE,
        ),
    };
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "name": session,
            "kind": kind.as_str(),
            "peer": dispatched_by,
            "written": outcome.written(),
        }))?
    );
    Ok(())
}

/// A predecessor `--continues` named, checked before anything is written:
/// its name and what the relation will record about it (R2).
pub(crate) struct ContinuesTarget {
    pub pred: String,
    hints: crate::session::links::LinkHints,
    /// Under the cloud backend, the predecessor's workspace prefix: where
    /// its back-reference goes and what the relation records.
    prefix: Option<String>,
}

impl ContinuesTarget {
    /// The predecessor's workspace prefix, under the cloud backend.
    pub(crate) fn prefix(&self) -> Option<&str> {
        self.prefix.as_deref()
    }
}

/// Why `--continues` was refused: the message and the exit code (2 for
/// what the caller named, 3 for a store that couldn't be read or written).
pub(crate) struct ContinuesError {
    pub message: String,
    pub exit: i32,
}

impl ContinuesError {
    fn caller(message: String) -> Self {
        Self {
            message,
            exit: super::EXIT_CALLER_ERROR,
        }
    }
}

/// Check `--continues <pred> [--continues-from <workspace>]` for session
/// `successor` before anything is written (R1, R3): `pred` passes the
/// session-name rule and isn't `successor`; `--continues-from` names a
/// workspace (an absolute path or a 16-hex workspace id, as `koto
/// --workspace` takes it); `pred` is a session in the local store, and not
/// an anchor or an anchor's close record. A running, finished or pinned
/// predecessor may be named. Nothing here reads `KOTO_ANCHOR`,
/// `KOTO_AUTHORITY`, a request leg or a dispatcher (R20): a coordinator's
/// successor, an operator by hand and an unrelated agent session record the
/// relation the same way.
///
/// Under the cloud backend the predecessor is looked up in this host's copy
/// when it is in the successor's own workspace and held here, and
/// otherwise in the bucket, under the workspace `--continues-from` names
/// (the successor's own by default): a bucket that can't be listed exits 3,
/// a session the bucket doesn't hold exits 2. A predecessor with the
/// successor's own name is accepted only from another workspace (an
/// import's copy naming its source). On the local backend there is one
/// store, so `--continues-from` is checked and the predecessor is looked up
/// there.
pub(crate) fn resolve_continues(
    backend: &Backend,
    successor: &str,
    pred: &str,
    from: Option<&str>,
) -> std::result::Result<ContinuesTarget, ContinuesError> {
    use crate::engine::types::SessionKind;
    use crate::session::links::{self, LinkHints, LinkKind};

    links::validate_peer(LinkKind::Continues, pred)
        .map_err(|e| ContinuesError::caller(format!("--continues: {e}")))?;
    let from_prefix = match from {
        Some(from) => Some(
            crate::session::cloud::workspace_read::WorkspaceId::parse(from)
                .map_err(|e| ContinuesError::caller(format!("--continues-from: {}", e.message)))?
                .prefix()
                .to_string(),
        ),
        None => None,
    };
    let cloud = backend.as_cloud();
    let own_prefix = cloud.map(|c| c.workspace_prefix().to_string());
    let elsewhere = matches!(
        (&from_prefix, &own_prefix),
        (Some(f), Some(own)) if f != own
    );
    if pred == successor && !elsewhere {
        return Err(ContinuesError::caller(format!(
            "--continues: session '{successor}' can't continue itself"
        )));
    }
    let (header, hints, prefix) = match cloud {
        Some(cloud) if elsewhere || !backend.session_dir(pred).is_dir() => {
            let pred_prefix = from_prefix
                .clone()
                .or(own_prefix.clone())
                .unwrap_or_default();
            let header = cloud
                .remote_header_at(&pred_prefix, pred)
                .map_err(|e| ContinuesError {
                    message: format!(
                        "--continues: the bucket couldn't be read to look '{pred}' up: {e:#}; \
                         nothing was written"
                    ),
                    exit: super::EXIT_INFRASTRUCTURE,
                })?
                .ok_or_else(|| {
                    ContinuesError::caller(format!(
                        "--continues: no session named '{pred}' in workspace {pred_prefix}"
                    ))
                })?;
            let hints = LinkHints {
                session_id: Some(header.session_id.clone()).filter(|id| !id.is_empty()),
                workspace: header
                    .execution_dir
                    .clone()
                    .or_else(|| header.origin.as_ref().map(|o| o.anchor.clone()))
                    .map(|p| p.to_string_lossy().into_owned()),
            };
            (Some(header), hints, Some(pred_prefix))
        }
        _ => {
            if !backend.exists(pred) {
                return Err(ContinuesError::caller(format!(
                    "--continues: no session named '{pred}' in this store"
                )));
            }
            (
                backend.read_header(pred).ok(),
                LinkHints::for_peer(backend, pred),
                own_prefix.clone(),
            )
        }
    };
    if let Some(header) = header {
        match header.session_kind() {
            SessionKind::Anchor => {
                return Err(ContinuesError::caller(format!(
                    "--continues: '{pred}' is an agent session's anchor, which no session \
                     continues"
                )))
            }
            SessionKind::AnchorClose => {
                return Err(ContinuesError::caller(format!(
                    "--continues: '{pred}' is an anchor's close record, which no session \
                     continues"
                )))
            }
            SessionKind::Ordinary => {}
        }
    }
    Ok(ContinuesTarget {
        pred: pred.to_string(),
        hints,
        prefix,
    })
}

/// Write the continues relation `target` names into `successor`, and only
/// there (Decision 1). Returns whether it was written; a relation the
/// session already holds changes nothing. A session already continuing
/// another predecessor is refused with exit 2, writing nothing; a store
/// that can't be read or written exits 3.
///
/// Under the cloud backend the back-reference goes first: it is PUT beside
/// the predecessor before the relation is written, and a PUT the bucket
/// doesn't confirm exits 3 with nothing written. The relation is then
/// pushed strictly; when that fails, the relation and the pointer are taken
/// back and the command exits 3, so running it again works (R19).
pub(crate) fn record_continues(
    backend: &Backend,
    successor: &str,
    target: &ContinuesTarget,
) -> std::result::Result<crate::session::links::LinkWrite, ContinuesError> {
    use crate::session::links::{self, ContinuesHints, ContinuesRefusal};
    let holder_session_id = backend
        .read_header(successor)
        .ok()
        .map(|h| h.session_id)
        .filter(|id| !id.is_empty());
    let cloud = backend.as_cloud();
    let hints = ContinuesHints {
        peer: target.hints.clone(),
        prefix: cloud.and(target.prefix.clone()),
        holder_session_id: holder_session_id.clone(),
        holder_prefix: cloud.map(|c| c.workspace_prefix().to_string()),
    };
    let failed = |e: anyhow::Error| match e.downcast_ref::<ContinuesRefusal>() {
        Some(refusal) => ContinuesError::caller(format!("--continues: {refusal}")),
        None => ContinuesError {
            message: format!(
                "could not record that '{successor}' continues '{}': {e:#}; nothing was written",
                target.pred
            ),
            exit: super::EXIT_INFRASTRUCTURE,
        },
    };
    let write = || links::write_continues(backend, successor, &target.pred, &hints);
    let (Some(cloud), Some(pred_prefix)) = (cloud, target.prefix.as_deref()) else {
        return write().map_err(failed);
    };
    // A session that already continues another predecessor is refused
    // before any pointer is written.
    let held = links::continues_of(backend, successor).map_err(failed)?;
    if let Some(existing) = held.iter().find(|p| **p != target.pred) {
        return Err(failed(
            ContinuesRefusal::OtherPredecessor {
                existing: existing.clone(),
            }
            .into(),
        ));
    }
    let Some(holder_id) = holder_session_id else {
        return Err(failed(anyhow::anyhow!(
            "the session's header records no session id, which its back-reference needs"
        )));
    };
    let owed = cloud.pointer_for(successor, pred_prefix, &target.pred, &holder_id);
    cloud
        .record_continues_with_pointer(successor, &owed, write)
        .map_err(failed)
}

/// Record `koto init --continues` in the session just opened, before the
/// command reports it: the session never exists without its relation
/// unless this write fails, and then the command fails (exit 3, or 2 when
/// an attached session already continues another one) naming the
/// `koto session link` that records it.
pub(crate) fn record_continues_at_init(backend: &Backend, name: &str, target: &ContinuesTarget) {
    if let Err(e) = record_continues(backend, name, target) {
        super::exit_with_error_code(
            serde_json::json!({
                "error": format!(
                    "{}; session '{name}' was opened without it: run `koto session link {name} \
                     --continues {}` to record it",
                    e.message, target.pred
                ),
                "command": "init",
            }),
            e.exit,
        )
    }
}

/// `koto session link <session> --continues <pred> [--continues-from
/// <workspace>]`: record that `session` continues `pred`, in `session` only.
///
/// Every check runs before anything is written: the session name, that the
/// session exists and wasn't imported elsewhere, and
/// [`resolve_continues`]'s checks of the predecessor. A refusal exits 2
/// with `{"error", "command"}`; a write that fails exits 3. Prints
/// `{"name", "kind": "continues", "peer", "written"}`, `written: false`
/// when the session already held this relation and nothing changed.
pub fn handle_link_continues(
    backend: &Backend,
    session: &str,
    pred: &str,
    from: Option<&str>,
) -> Result<()> {
    let refuse = |error: String, code: i32| -> ! {
        super::exit_with_error_code(
            serde_json::json!({ "error": error, "command": "session link" }),
            code,
        )
    };
    if let Err(e) = ValidatedSessionId::new(session) {
        refuse(e.to_string(), super::EXIT_CALLER_ERROR);
    }
    let target = resolve_continues(backend, session, pred, from)
        .unwrap_or_else(|e| refuse(e.message, e.exit));
    if !backend.exists(session) {
        refuse(
            format!("session '{session}' does not exist"),
            super::EXIT_CALLER_ERROR,
        );
    }
    if let Err(e) = backend.check_not_migrated(session) {
        refuse(e.to_string(), super::EXIT_CALLER_ERROR);
    }
    // Under the cloud backend the back-reference names this directory's
    // workspace as the successor's: a session whose own copy is under
    // another prefix refuses.
    if let Err(e) = backend.check_located(session) {
        refuse(e.to_string(), super::EXIT_CALLER_ERROR);
    }
    if !backend
        .home_of(session)
        .is_ok_and(|home| home.own_workspace)
    {
        refuse(
            format!(
                "session '{session}' keeps its own copy under another directory's prefix, and \
                 a continues relation is recorded only from the session's own workspace; run \
                 the command from the directory the session was started in"
            ),
            super::EXIT_CALLER_ERROR,
        );
    }
    let outcome =
        record_continues(backend, session, &target).unwrap_or_else(|e| refuse(e.message, e.exit));
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "name": session,
            "kind": crate::session::links::LinkKind::Continues.as_str(),
            "peer": pred,
            "written": outcome.written(),
        }))?
    );
    Ok(())
}

/// `koto session pin <name> --reason <reason>`: record a pin in `name`.
///
/// Prints `{"name", "pin": {"reason", "set_by", "set_at"}, "store"}`, the
/// existing pin unchanged when the session already held one. A refusal
/// prints `{"error": {"code", "message", ...}}` and exits 2; a store that
/// can't be written exits 3. See [`crate::session::pin::pin`].
pub fn handle_pin(backend: &Backend, name: &str, reason: &str) -> Result<()> {
    use crate::session::hierarchy::display_text;
    use crate::session::pin;

    let outcome = pin::pin(backend, name, reason)
        .unwrap_or_else(|e| super::exit_with_error_code(e.to_json(), e.exit));
    let p = outcome.pin();
    let store = match backend {
        Backend::Local(_) => serde_json::json!("local"),
        Backend::Cloud(_) => serde_json::json!({
            "cloud": backend.store_identity().and_then(|s| s.bucket),
        }),
    };
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "name": name,
            "pin": {
                "reason": display_text(&p.reason),
                "set_by": p.set_by.as_deref().map(display_text),
                "set_at": display_text(&p.set_at),
            },
            "store": store,
        }))?
    );
    Ok(())
}

/// `koto session unpin <name> [--reason <reason>]`: release `name`'s pin.
///
/// Prints `{"name", "released"}`, where `released` is the release record
/// written, or `null` when the session held no pin. A refusal prints
/// `{"error": {"code", "message", ...}}` and exits 2; a store that can't be
/// written exits 3. See [`crate::session::pin::unpin`].
pub fn handle_unpin(backend: &Backend, name: &str, reason: Option<&str>) -> Result<()> {
    use crate::session::pin;

    let released = pin::unpin(backend, name, reason)
        .unwrap_or_else(|e| super::exit_with_error_code(e.to_json(), e.exit));
    let released = released
        .as_ref()
        .map(crate::session::hierarchy::release_json);
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "name": name,
            "released": released,
        }))?
    );
    Ok(())
}

/// Handle `koto session needed <path>`: print one line and exit 0 (not
/// needed), 1 (needed) or 2 (can't decide), as
/// [`crate::session::needed`] judges `path`.
///
/// Builds no backend: constructing one can migrate an old store layout,
/// and the cloud backend's reaches for its bucket. The sessions directory
/// is the one the configured backend keeps on this host: the cloud
/// backend's local copies under the koto root, or for the local backend
/// `KOTO_SESSIONS_BASE` when set. Under the cloud backend the bucket is
/// read through a read-only reader ([`CloudBackend::bucket_reader`]),
/// which is only listed and read. A reader that can't be made keeps every
/// running session it would have been asked about. A configuration that
/// can't be loaded or checked exits 2, like any other undecided answer.
///
/// [`CloudBackend::bucket_reader`]: crate::session::cloud::CloudBackend::bucket_reader
pub fn handle_needed(path: &str) -> Result<()> {
    use crate::session::cloud::CloudBackend;
    use crate::session::needed::{needed, Answer, Bucket};

    let answer = match needed_store() {
        Ok((base, None)) => needed(&base, std::path::Path::new(path), &Bucket::None),
        Ok((base, Some(cloud))) => match CloudBackend::bucket_reader(base.clone(), &cloud) {
            Ok(reader) => needed(&base, std::path::Path::new(path), &Bucket::Reads(&reader)),
            Err(_) => needed(&base, std::path::Path::new(path), &Bucket::Unavailable),
        },
        Err(e) => Answer::Undecided(format!(
            "can't decide: the sessions directory can't be found: {e:#}"
        )),
    };
    match &answer {
        Answer::Undecided(_) => eprintln!("{}", answer.line()),
        _ => println!("{}", answer.line()),
    }
    std::process::exit(answer.exit_code());
}

/// The sessions directory `koto session needed` reads, resolved the way
/// `build_backend` resolves it, without building the backend, and the
/// cloud configuration when the backend is the cloud one.
fn needed_store() -> Result<(std::path::PathBuf, Option<crate::config::CloudConfig>)> {
    let config = crate::config::resolve::load_config()?;
    crate::config::session_env::check(&config)?;
    let koto_sessions = || -> Result<std::path::PathBuf> {
        Ok(crate::koto_home::koto_home_or_err()?.join("sessions"))
    };
    match config.session.backend.as_str() {
        "cloud" => Ok((koto_sessions()?, Some(config.session.cloud.clone()))),
        "local" => match std::env::var("KOTO_SESSIONS_BASE") {
            Ok(base) => Ok((std::path::PathBuf::from(base), None)),
            Err(_) => Ok((koto_sessions()?, None)),
        },
        other => anyhow::bail!("unknown backend: {other}"),
    }
}

/// Handle `koto session hierarchy <name> [--json]`: judge the hierarchy
/// `name` belongs to and print the verdict, its members and the settings.
///
/// Reads the local sessions directory directly and writes nothing; under
/// the cloud backend the bucket is asked only about the pins the scan read
/// (one check each), and no other session's data is listed or read from it.
/// A malformed name or retention setting exits 2, as does a name
/// with no session; a sessions directory that can't be listed exits 3.
///
/// For a pinned session it shows the pin and that the pin keeps that
/// session only; for a session with a release record, the latest one; for
/// any other session whose hierarchy holds a pinned member, that member's
/// name; and for each anchor member, its last activity.
pub fn handle_hierarchy(backend: &Backend, name: &str, json: bool) -> Result<()> {
    use crate::session::hierarchy::{self, Graph, Judge, RetentionSettings, Scope};

    let fail = |error: String, code: i32| -> ! {
        super::exit_with_error_code(
            serde_json::json!({ "error": error, "command": "session hierarchy" }),
            code,
        )
    };
    if let Err(e) = ValidatedSessionId::new(name) {
        fail(e.to_string(), super::EXIT_CALLER_ERROR);
    }
    let settings = RetentionSettings::load()
        .unwrap_or_else(|e| fail(format!("{e:#}"), super::EXIT_CALLER_ERROR));
    let base = backend.local_base_dir();
    let mut graph = Graph::scan(base).unwrap_or_else(|e| {
        fail(
            format!("the sessions directory can't be read, so nothing can be judged: {e:#}"),
            super::EXIT_INFRASTRUCTURE,
        )
    });
    // Under the cloud backend, each pin the scan read is checked in the
    // bucket, so one released on another host isn't shown as in force.
    crate::session::pin::reconcile(backend, &mut graph);
    if !graph.contains(name) {
        fail(
            format!("session '{name}' does not exist"),
            super::EXIT_CALLER_ERROR,
        );
    }
    let mut judge = Judge::new(&graph, settings.clone(), std::time::SystemTime::now());
    let judgment = judge
        .judge(name, &Scope::Whole)
        .unwrap_or_else(|e| fail(format!("{e:#}"), super::EXIT_INFRASTRUCTURE));
    let release = crate::session::pin::read_release(base, name);
    // An anchor's authority record, read from its file like `unpin` reads
    // it; an anchor with none, or an unreadable one, shows no block.
    let authority = crate::session::anchor::is_anchor(backend, name)
        .then(|| crate::session::anchor::read_authority(&base.join(name)))
        .and_then(|read| match read {
            crate::session::anchor::AuthorityRead::Record(r) => Some(r),
            _ => None,
        });
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&hierarchy::query_json(
                &judgment,
                &settings,
                release.as_ref(),
                authority.as_ref()
            ))?
        );
    } else {
        print!(
            "{}",
            hierarchy::query_text(&judgment, &settings, release.as_ref(), authority.as_ref())
        );
    }
    Ok(())
}

/// Handle `koto session list --pinned [--json]`: every session in the local
/// store holding a pin, readable or not, sorted by name, each with its pin
/// (or `pin_unreadable`), whether it is `running` or `finished`, and
/// `source: local`. A koto session whose log can't be read is
/// `unreadable`. No pins prints an empty list (`[]`, or `no pinned
/// sessions` as text) and exits 0.
///
/// It reads the local sessions directory directly, as the retention engine
/// does, and writes nothing. A malformed retention setting exits 2 and a
/// sessions directory that can't be listed exits 3.
///
/// Under the cloud backend each local pin is checked in the bucket (one
/// released on another host isn't listed, one the bucket holds is
/// `source: bucket`), and one listing of the workspace's prefix adds every
/// session pinned in the bucket that this host holds no pinned copy of, with
/// `source: bucket` and the `state` `unknown` when no copy is here to read
/// it from. When the bucket can't be listed, the local rows are printed
/// with `bucket_unchecked` (`{"pinned": [...], "bucket_unchecked":
/// {"error"}}` in JSON) and the command exits 3.
pub fn handle_list_pinned(backend: &Backend, json: bool) -> Result<()> {
    use crate::engine::types::SessionKind;
    use crate::session::hierarchy::{self, Graph, Judge, RetentionSettings};
    use crate::session::inventory::{inspect, SessionStatus, TemplateCache};
    use crate::session::pin::{BucketPin, PinFact};

    let fail = |error: String, code: i32| -> ! {
        super::exit_with_error_code(
            serde_json::json!({ "error": error, "command": "session list --pinned" }),
            code,
        )
    };
    let settings = RetentionSettings::load()
        .unwrap_or_else(|e| fail(format!("{e:#}"), super::EXIT_CALLER_ERROR));
    let base = backend.local_base_dir();
    let mut graph = Graph::scan(base).unwrap_or_else(|e| {
        fail(
            format!("the sessions directory can't be read, so no pin can be listed: {e:#}"),
            super::EXIT_INFRASTRUCTURE,
        )
    });
    crate::session::pin::reconcile(backend, &mut graph);
    let mut judge = Judge::new(&graph, settings, std::time::SystemTime::now());
    let mut cache = TemplateCache::new();
    // (name, pin, kind, source); `None` for the kind of a session this host
    // holds no copy of.
    let mut pinned: Vec<(String, PinFact, Option<SessionKind>, &'static str)> = graph
        .names()
        .filter_map(|n| {
            let facts = graph.facts(n)?;
            facts.pin.is_pinned().then(|| {
                (
                    n.to_string(),
                    facts.pin.clone(),
                    Some(facts.kind),
                    facts.pin_source(),
                )
            })
        })
        .collect();
    let mut unchecked: Option<crate::session::pin::BucketError> = None;
    if let Backend::Cloud(cloud) = backend {
        match cloud.bucket_pinned_sessions() {
            Ok(names) => {
                for name in names {
                    if pinned.iter().any(|(n, ..)| *n == name) {
                        continue;
                    }
                    match cloud.bucket_pin(&name, None) {
                        BucketPin::Pin(pin) if pin.is_pinned() => {
                            let kind = graph.facts(&name).map(|f| f.kind);
                            pinned.push((name, pin, kind, crate::session::pin::SOURCE_BUCKET));
                        }
                        BucketPin::Unchecked(e) => {
                            unchecked.get_or_insert(e);
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => unchecked = Some(e),
        }
    }
    pinned.sort_by(|a, b| a.0.cmp(&b.0));
    let mut rows = Vec::new();
    let mut lines = Vec::new();
    for (name, pin, kind, source) in pinned {
        let state = match kind {
            Some(SessionKind::Ordinary) => match inspect(base, &name, &mut cache).status {
                SessionStatus::Running => "running",
                SessionStatus::Terminal | SessionStatus::Cancelled => "finished",
                SessionStatus::Unreadable => "unreadable",
            },
            // An anchor or close record found pinned (it arrived by
            // import): running while the engine counts it alive.
            Some(SessionKind::Anchor | SessionKind::AnchorClose) => {
                if judge.is_alive(&name) {
                    "running"
                } else {
                    "finished"
                }
            }
            None => "unknown",
        };
        let shown = hierarchy::sanitize(&name);
        let mut row = serde_json::Map::new();
        row.insert("name".to_string(), serde_json::json!(shown));
        row.insert("state".to_string(), serde_json::json!(state));
        for (k, v) in hierarchy::pin_fields(&pin, source) {
            if k != "kept_by_pin" {
                row.insert(k, v);
            }
        }
        row.insert("source".to_string(), serde_json::json!(source));
        rows.push(serde_json::Value::Object(row));
        lines.push(format!(
            "{shown} ({state}): {}",
            hierarchy::pin_text(&pin).unwrap_or_default()
        ));
    }
    if let Some(error) = unchecked {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "pinned": rows,
                    "bucket_unchecked": {"error": error.as_str()},
                }))?
            );
        } else {
            for line in lines {
                println!("{line}");
            }
            println!(
                "bucket not checked for pins ({}); only this host's pins are listed",
                error.as_str()
            );
        }
        std::process::exit(super::EXIT_INFRASTRUCTURE);
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else if lines.is_empty() {
        println!("no pinned sessions");
    } else {
        for line in lines {
            println!("{line}");
        }
    }
    Ok(())
}

/// What an explicit removal (`koto session cleanup`, `koto cancel
/// --cleanup`) broke: every other live member of the removed session's
/// hierarchy, judged just before the removal.
pub(crate) struct LiveMembers {
    /// `{name, kind, observed}` for each live member other than the session
    /// itself. Empty when nothing else is alive, so the rule would not have
    /// kept the session for a live member.
    pub members: Vec<serde_json::Value>,
    /// Why the hierarchy couldn't be judged, when it couldn't; `members` is
    /// then empty and says nothing.
    pub error: Option<String>,
    /// What the continues relation would have kept the session for:
    /// `relation_keepers` and `relation_unreadable`
    /// (`hierarchy::relation_report_json`). Empty when nothing continuing
    /// it has standing.
    pub relation: serde_json::Map<String, serde_json::Value>,
    /// The same as text lines (`kept: continued by <keeper> (<standing>)
    /// via <path>`), for stderr.
    pub relation_lines: Vec<String>,
}

/// Judge `name`'s hierarchy in the local store `base` and list its other
/// live members, and what the continues relation would have kept it for
/// (with, under the cloud backend, the bucket's answer when nothing local
/// keeps it). Never fails: the removal goes ahead whatever this finds. A
/// malformed retention setting falls back to the defaults (it only moves
/// when an unobserved anchor idles out), with a warning.
pub(crate) fn live_members(
    backend: &dyn SessionBackend,
    base: &std::path::Path,
    name: &str,
) -> LiveMembers {
    use crate::session::hierarchy::{
        relation_report_json, relation_text, sanitize, Graph, Judge, RetentionSettings, Scope,
        Verdict,
    };

    let none = |error: Option<String>| LiveMembers {
        members: Vec::new(),
        error,
        relation: Default::default(),
        relation_lines: Vec::new(),
    };
    if ValidatedSessionId::new(name).is_err() {
        return none(None);
    }
    let settings = RetentionSettings::load().unwrap_or_else(|e| {
        eprintln!("warning: {e:#}; judging '{name}''s hierarchy with the default settings");
        let day = std::time::Duration::from_secs(86_400);
        RetentionSettings::from_durations(7 * day, 7 * day, 30 * day)
    });
    let graph = match Graph::scan(base) {
        Ok(graph) => graph,
        Err(e) => {
            return none(Some(format!(
                "the sessions directory can't be read, so the hierarchy wasn't judged: {e:#}"
            )))
        }
    };
    if !graph.contains(name) {
        return none(None);
    }
    let mut judge = Judge::new(&graph, settings, std::time::SystemTime::now());
    match judge.judge(name, &Scope::Whole) {
        Ok(j) => {
            let related = crate::session::continues_bucket::relation_at_removal(backend, &j, name);
            let members = match &j.verdict {
                Verdict::Keep { keepers, .. } => keepers
                    .iter()
                    .filter(|k| k.name != name)
                    .map(|k| {
                        serde_json::json!({
                            "name": sanitize(&k.name),
                            "kind": k.kind.as_str(),
                            "observed": k.observed,
                        })
                    })
                    .collect(),
                Verdict::Release { .. } => Vec::new(),
            };
            LiveMembers {
                members,
                error: None,
                relation: relation_report_json(&related),
                relation_lines: relation_text(&related),
            }
        }
        Err(e) => none(Some(format!("{e:#}"))),
    }
}

/// Warn on stderr that removing `name` broke what `live` names.
pub(crate) fn warn_live_members(name: &str, live: &LiveMembers) {
    use crate::session::hierarchy::sanitize;
    if let Some(e) = &live.error {
        eprintln!("warning: removed '{}'; {}", sanitize(name), sanitize(e));
    }
    // What the continues relation would have kept it for, one line per
    // keeper, as every removal by rule prints it.
    for line in &live.relation_lines {
        eprintln!(
            "warning: removed '{}', which the retention rule would have {line}",
            sanitize(name)
        );
    }
    if live.members.is_empty() {
        return;
    }
    let names: Vec<String> = live
        .members
        .iter()
        .filter_map(|m| m["name"].as_str())
        .map(str::to_string)
        .collect();
    eprintln!(
        "warning: removed '{}', which the retention rule would have kept: still alive in its \
         hierarchy: {}",
        sanitize(name),
        names.join(", ")
    );
}

/// Record a dispatched-by link to `dispatcher` in `session` after the
/// session was opened, warning on stderr instead of failing when it can't
/// be written: the session is already there, and `koto session link`
/// repairs a missing link. `dispatcher` was validated before the session
/// was opened.
pub(crate) fn record_dispatched_by(backend: &Backend, session: &str, dispatcher: &str) {
    use crate::session::links::{self, LinkHints, LinkKind};
    let hints = LinkHints::for_peer(backend, dispatcher);
    if let Err(e) = links::write_link(backend, session, LinkKind::DispatchedBy, dispatcher, &hints)
    {
        eprintln!(
            "warning: could not record that '{session}' was dispatched by '{dispatcher}': {e}; \
             run `koto session link {session} --dispatched-by {dispatcher}` to record it"
        );
    }
}

/// Record the dispatched-by link a leg-bound session gets: `requested_by`,
/// the request's requester, when it passes the session-name rule and isn't
/// the session itself. Any other value writes nothing, silently: a
/// requester need not be a session. A write that fails warns on stderr.
pub(crate) fn record_requester_link(backend: &Backend, session: &str, requested_by: &str) {
    use crate::session::links::{validate_peer, LinkKind};
    if requested_by == session || validate_peer(LinkKind::DispatchedBy, requested_by).is_err() {
        return;
    }
    record_dispatched_by(backend, session, requested_by);
}

/// Refuse `koto session rebind <name>` from a directory that is neither the
/// session's own workspace nor the one the rebind moves it to, when koto
/// can't tell which bucket copy is the session's own (koto#477).
///
/// A session that records the prefix it was created under is pushed there
/// whatever directory the rebind runs in, so it rebinds from anywhere, and
/// so does one recorded before koto named the prefix once its copy is
/// located. One that can't be located would be pushed under the third
/// directory's prefix, where neither the old workspace nor the new one reads
/// it. Run from the target (the usual `koto session rebind <name>` stood in
/// the new checkout) it is accepted even when every directory the session
/// records is gone, as for a session whose instance was torn down; the
/// rebind's fence makes any writer left at the old anchor refuse. The local
/// backend never refuses.
pub fn rebind_guard(backend: &Backend, name: &str, to: Option<&str>) -> Result<()> {
    use crate::session::{Unlocated, UnlocatedReason};
    use std::path::{Path, PathBuf};
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let here = canonical(&std::env::current_dir()?);
    let target = to.map_or_else(|| here.clone(), |dir| canonical(&PathBuf::from(dir)));
    if target == here {
        return Ok(());
    }
    let Err(e) = backend.check_located(name) else {
        return Ok(());
    };
    let Some(Unlocated {
        reason: UnlocatedReason::NoRecordedPrefix { anchor, .. },
        ..
    }) = e.downcast_ref::<Unlocated>()
    else {
        return Err(e);
    };
    let at = anchor
        .as_deref()
        .unwrap_or("the directory the session was started in");
    anyhow::bail!(
        "{e}. A rebind of it runs from the directory it is bound to or from the directory it \
         moves to: run `koto session rebind {name} --to {}` from {at}, or `koto session rebind \
         {name}` from {}",
        target.display(),
        target.display()
    )
}

/// Move a session's execution anchor to `to`, defaulting to the
/// current working directory.
///
/// The anchor is the directory every tick runs its gates and actions
/// in, and `koto next` refuses from anywhere outside it. This is the
/// only verb that changes one (Decision 6 of
/// `DESIGN-koto-runs-commands.md`), so a checkout that genuinely moved
/// is repaired by one deliberate command and the move lands in the log
/// as an `execution_anchor_rebound` event instead of happening
/// silently. Nothing here cares how the session was created, so it
/// works on a child exactly as on any other session.
///
/// Rebinding to the directory the session is already bound to appends
/// nothing and reports `rebound: false`. The event means the anchor
/// moved; a no-op invocation did not move it.
pub fn handle_rebind(backend: &dyn SessionBackend, name: &str, to: Option<&str>) -> Result<()> {
    use crate::engine::types::{now_iso8601, EventPayload};
    use anyhow::Context;
    use std::path::PathBuf;

    // `name` reaches a path join below, so it goes through the
    // newtype before any filesystem call, as every other
    // caller-supplied session id in this module does.
    let name = ValidatedSessionId::new(name)?.into_inner();
    let name = name.as_str();

    if !backend.exists(name) {
        anyhow::bail!("session '{}' does not exist", name);
    }

    let requested = match to {
        Some(dir) => PathBuf::from(dir),
        None => std::env::current_dir()
            .context("--to was omitted and the current directory could not be read")?,
    };

    // The per-tick check compares canonical paths, so the anchor is
    // stored the way it will be read. Canonicalizing here also means a
    // target that does not resolve is refused now, rather than written
    // and then refused on every tick afterwards.
    let target = std::fs::canonicalize(&requested).with_context(|| {
        format!(
            "cannot rebind '{}' to {}: no such directory on this machine",
            name,
            requested.display()
        )
    })?;
    if !target.is_dir() {
        anyhow::bail!(
            "cannot rebind '{}' to {}: not a directory",
            name,
            target.display()
        );
    }

    let from = backend.read_header(name)?.execution_dir;

    if from.as_deref() == Some(target.as_path()) {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "name": name,
                "rebound": false,
                "from": from,
                "to": target,
            }))?
        );
        return Ok(());
    }

    // Under the cloud backend, the home before the move, when it has one:
    // a session that takes its home from its anchor moves home with it.
    let home_before = backend
        .as_cloud()
        .map(|cloud| cloud.home_prefix(name).ok().map(|home| home.prefix));

    // Event first, header second, matching the adoption path in
    // `handle_next`: a crash between the two leaves a visible record of
    // the move rather than a silent one.
    let payload = EventPayload::ExecutionAnchorRebound {
        from: from.clone(),
        to: target.clone(),
    };
    backend.append_event(name, &payload, &now_iso8601())?;

    // Through the backend, which pushes the rewritten header: a local
    // rewrite alone is undone by the next read's pull on the cloud
    // backend (koto#310).
    backend.rewrite_header(name, &|mut h| {
        // The origin record moves with an explicit, audited rebind, so
        // `koto init --attach-live` from the new checkout still finds
        // its own session. A session with no origin record gets none:
        // nothing backfills one.
        //
        // The rebind also fences the anchor it leaves (koto#477): the epoch
        // goes up and the old anchor is retired, so a process still writing
        // from there gets `session_rebound` instead of forking the log.
        if let Some(origin) = h.origin.as_mut() {
            let left = from.clone().unwrap_or_else(|| origin.anchor.clone());
            origin.anchor = target.clone();
            crate::session::rebind_fence::advance(origin, Some(&left), &target);
        }
        h.execution_dir = Some(target.clone());
        h
    })?;

    // The rewrite pushed the state file to the new home; the context keys
    // go there too, or stderr names the ones that didn't (koto#524).
    if let (Some(cloud), Some(before)) = (backend.as_cloud(), home_before) {
        cloud.carry_context_after_rebind(name, before.as_deref());
    }

    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "name": name,
            "rebound": true,
            "from": from,
            "to": target,
        }))?
    );
    Ok(())
}

/// Import session `name` from the workspace at `from` into the current
/// directory, under the cloud backend, as `as_name` when given.
/// `trust_template` takes the compiled template from the bucket instead of
/// this machine's cache.
///
/// Prints one JSON object on success. A refusal or failure prints
/// `{"error": {"code": ..., "message": ...}}` and exits 2 for the codes a
/// caller acts on and 1 for the rest (see
/// [`crate::session::cloud::ImportErrorCode::exit_code`]).
/// The remote work, and the order that keeps the source untouched until
/// the end, is in [`CloudBackend::import_session`].
pub fn handle_import(
    backend: &Backend,
    name: &str,
    from: &str,
    as_name: Option<&str>,
    trust_template: bool,
) -> Result<()> {
    use crate::session::cloud::{ImportError, ImportErrorCode, ImportRequest};
    use anyhow::Context;

    let refuse = |err: ImportError| -> ! {
        super::exit_with_error_code(
            serde_json::json!({
                "error": {
                    "code": err.code.as_str(),
                    "message": err.message,
                }
            }),
            err.code.exit_code(),
        )
    };

    let cloud = match backend {
        Backend::Cloud(c) => c,
        Backend::Local(_) => refuse(ImportError::new(
            ImportErrorCode::RequiresCloud,
            "session import reads another workspace's copy in the bucket, which needs the \
             cloud backend (session.backend = \"cloud\")",
        )),
    };

    // Both names reach remote keys and a local path join, so they go
    // through the newtype before either.
    let name = ValidatedSessionId::new(name)?.into_inner();
    let target = match as_name {
        Some(n) => ValidatedSessionId::new(n)?.into_inner(),
        None => name.clone(),
    };

    let cwd = std::env::current_dir().context("the current directory could not be read")?;
    let anchor = std::fs::canonicalize(&cwd)
        .with_context(|| format!("cannot resolve the current directory {}", cwd.display()))?;

    // The importing agent session's anchor, which a held-by link whose
    // anchor isn't open here is rewritten to.
    let importer_anchor = crate::session::anchor::anchor_from_env();
    let request = ImportRequest {
        name: &name,
        target: &target,
        from: std::path::Path::new(from),
        anchor: &anchor,
        trust_template,
        explicit_as: as_name.is_some(),
        importer_anchor: importer_anchor.as_deref(),
    };
    let outcome = match cloud.import_session(&request) {
        Ok(outcome) => outcome,
        Err(err) => refuse(err),
    };

    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "name": outcome.name,
            "imported": true,
            "from": {
                "workspace": outcome.from_workspace,
                "session": outcome.from_session,
            },
            "keys": outcome.keys,
            "template": outcome.template.as_str(),
            "marked": true,
        }))?
    );
    Ok(())
}

/// Print the absolute session directory path.
pub fn handle_dir(backend: &dyn SessionBackend, name: &str) -> Result<()> {
    let dir = backend.session_dir(name);
    println!("{}", dir.display());
    Ok(())
}

/// Print all sessions as a JSON array.
///
/// Each row's `template_source_status` (already populated by the backend's
/// `list()`, no new I/O here) gains a `note` field when its `exists` is
/// `false`, worded via `format_stale_template_source_note` and gated on
/// `backend.is_cloud()` -- softened for `CloudBackend` sessions, direct for
/// `LocalBackend`. Takes `&Backend` (the concrete enum) rather than `&dyn
/// SessionBackend` so it can call that inherent accessor, mirroring
/// `handle_resolve`'s existing `&Backend` parameter.
///
/// `execution_dir` keeps only sessions whose recorded execution directory is
/// that directory or lies under it (see
/// [`crate::cli::session_inventory::under_dir`]); it implies `with_state` and
/// wraps the rows as `{"sessions": [...], "excluded_remote_only": N}`, where N
/// counts the cloud backend's remote-only rows, which have no header to
/// match. `with_state` adds each row's `state`, `status`, `failure` and
/// `finished_at` from one inspection per session; the filter runs on the
/// header first, so only matching rows are inspected.
pub fn handle_list(backend: &Backend, execution_dir: Option<&str>, with_state: bool) -> Result<()> {
    use crate::cli::session_inventory::{inspect, normalize_filter, under_dir, TemplateCache};

    let mut sessions = backend.list()?;
    let base = backend.local_base_dir().to_path_buf();
    let remote_only = |id: &str| backend.is_cloud() && !base.join(id).is_dir();

    let mut excluded_remote_only = 0usize;
    if let Some(arg) = execution_dir {
        let filter = normalize_filter(arg);
        sessions.retain(|s| {
            if remote_only(&s.id) {
                excluded_remote_only += 1;
                return false;
            }
            s.execution_dir
                .as_deref()
                .is_some_and(|dir| under_dir(dir, &filter))
        });
    }
    let with_state = with_state || execution_dir.is_some();

    let mut rows = serde_json::to_value(&sessions)?;
    if let serde_json::Value::Array(rows) = &mut rows {
        let is_cloud = backend.is_cloud();
        let mut cache = TemplateCache::new();
        for row in rows.iter_mut() {
            if with_state {
                let id = row["id"].as_str().unwrap_or_default().to_string();
                if remote_only(&id) {
                    for key in ["state", "status", "failure", "finished_at"] {
                        row[key] = serde_json::Value::Null;
                    }
                } else {
                    let seen = inspect(&base, &id, &mut cache);
                    row["state"] = serde_json::json!(seen.state);
                    row["status"] = serde_json::json!(seen.status);
                    row["failure"] = serde_json::json!(seen.failure);
                    row["finished_at"] = serde_json::json!(seen.finished_at);
                }
            }
            let exists_false = row
                .get("template_source_status")
                .and_then(|s| s.get("exists"))
                == Some(&serde_json::Value::Bool(false));
            if exists_false {
                if let Some(status) = row.get_mut("template_source_status") {
                    status["note"] = serde_json::json!(
                        crate::engine::template_source_status::format_stale_template_source_note(
                            is_cloud
                        )
                    );
                }
            }
        }
    }
    if execution_dir.is_some() {
        rows = serde_json::json!({
            "sessions": rows,
            "excluded_remote_only": excluded_remote_only,
        });
    }
    println!("{}", serde_json::to_string_pretty(&rows)?);
    Ok(())
}

/// Report, and with `apply` restore, the sessions the old-layout migration
/// moved into quarantine because their name was already taken at the flat
/// level.
///
/// Reporting is the default because this moves real user data. `--apply`
/// commits, and it commits in bulk: an install that accumulated a thousand
/// collisions is not one anybody should resolve a session at a time.
/// `--session` narrows the set by original name for the case where somebody
/// wants one workflow back and not the rest.
///
/// Idempotent by construction. A name that matches nothing in the quarantine
/// is reported under `unmatched` rather than failing, so re-running after a
/// successful recovery is a no-op instead of an error. The command exits
/// non-zero only when a move was attempted and failed, which leaves that
/// session in quarantine and the run repeatable.
pub fn handle_recover(backend: &Backend, apply: bool, sessions: &[String]) -> Result<()> {
    let base = backend.local_base_dir().to_path_buf();
    let mut entries = recover::scan(&base);

    let mut unmatched: Vec<&String> = Vec::new();
    if !sessions.is_empty() {
        for wanted in sessions {
            let matched = entries
                .iter()
                .any(|e| e.session_id.as_deref() == Some(wanted.as_str()));
            if !matched {
                unmatched.push(wanted);
            }
        }
        entries.retain(|e| {
            e.session_id
                .as_deref()
                .is_some_and(|id| sessions.iter().any(|s| s == id))
        });
    }

    let mut rows = Vec::with_capacity(entries.len());
    let (mut recovered, mut skipped, mut failed) = (0usize, 0usize, 0usize);

    for entry in &entries {
        let mut row = serde_json::json!({
            "repo_id": entry.repo_id,
            "session": entry.session_id,
            "path": entry.path.display().to_string(),
            "recovered_as": entry.proposed_id(),
        });

        if !apply {
            match entry.proposed_id() {
                Some(_) => {
                    row["status"] = serde_json::json!("pending");
                }
                None => {
                    skipped += 1;
                    row["status"] = serde_json::json!("skipped");
                    row["reason"] = serde_json::json!(
                        "no state file, or a name that would not be a valid session id"
                    );
                }
            }
            rows.push(row);
            continue;
        }

        match recover::recover_one(&base, entry) {
            recover::Outcome::Recovered { id, header_error } => {
                recovered += 1;
                row["status"] = serde_json::json!("recovered");
                row["recovered_as"] = serde_json::json!(id);
                row["header_rewritten"] = serde_json::json!(header_error.is_none());
                if let Some(e) = header_error {
                    row["reason"] = serde_json::json!(format!(
                        "the session is back in place, but its state file header still \
                         names the old id: {e}"
                    ));
                }
            }
            recover::Outcome::Skipped(reason) => {
                skipped += 1;
                row["status"] = serde_json::json!("skipped");
                row["reason"] = serde_json::json!(reason);
            }
            recover::Outcome::Failed(reason) => {
                failed += 1;
                row["status"] = serde_json::json!("failed");
                row["reason"] = serde_json::json!(reason);
            }
        }
        rows.push(row);
    }

    let mut report = serde_json::json!({
        "quarantine_dir": recover::quarantine_root(&base).display().to_string(),
        "applied": apply,
        "sessions": rows,
        "summary": {
            "total": entries.len(),
            "recovered": recovered,
            "skipped": skipped,
            "failed": failed,
        },
        "unmatched": unmatched,
    });

    if !apply && entries.len() > skipped {
        report["hint"] = serde_json::json!(
            "run `koto session recover --apply` to move these back into the session list"
        );
    }
    if backend.is_cloud() {
        report["note"] = serde_json::json!(
            "recovery works on the local session store; recovered sessions are not \
             pushed to the configured bucket by this command"
        );
    }

    println!("{}", serde_json::to_string_pretty(&report)?);

    if failed > 0 {
        anyhow::bail!(
            "{} session(s) could not be recovered and are still in {}",
            failed,
            recover::quarantine_root(&base).display()
        );
    }
    Ok(())
}

/// Remove a session directory. Idempotent: succeeds even if the session doesn't exist.
pub fn handle_cleanup(backend: &dyn SessionBackend, name: &str) -> Result<()> {
    // Whether a session directory was there to remove. Cleanup is idempotent,
    // and on a name koto already removed itself the record is the answer
    // `koto status` gives, so it must stay.
    let had_session = backend.session_dir(name).is_dir();
    // Through the pin guard: a pinned session is refused with exit 2 and
    // nothing removed, naming the pin and saying to unpin first.
    if let crate::session::pin::Removal::Kept(guard) =
        crate::session::pin::remove_guarded(backend, name)?
    {
        let refusal = guard
            .refusal(name)
            .expect("a guard that keeps has a refusal");
        super::exit_with_error_code(refusal.to_json(), refusal.exit);
    }
    // A person's removal leaves no finished record: one a crash left beside
    // the session (written just before a removal that never happened) must
    // not answer for it as removed by koto.
    #[cfg(unix)]
    if had_session {
        super::forget_finished_record(backend, name);
    }
    Ok(())
}

/// Resolve a session version conflict by keeping either the local or
/// remote state. Under `CloudBackend`, a `--children` policy reconciles
/// the parent's direct children using the strict-prefix rule (default)
/// or an explicit side-selection.
///
/// `sync_status` and `machine_id` fields in the JSON response are
/// emitted only under `CloudBackend` per Decision 12 Q5 of the batch
/// child-spawning design — under `LocalBackend` those fields have no
/// meaningful value and are elided.
pub fn handle_resolve(
    backend: &Backend,
    name: &str,
    keep: &str,
    children: ChildrenPolicy,
) -> Result<()> {
    match keep {
        "local" | "remote" => {}
        other => anyhow::bail!(
            "invalid --keep value: '{}'. Must be 'local' or 'remote'.",
            other
        ),
    }

    let cloud = match backend {
        Backend::Cloud(c) => c,
        Backend::Local(_) => {
            anyhow::bail!("session resolve requires cloud backend (session.backend = \"cloud\")")
        }
    };

    // Push-parent-first ordering: parent reconciliation commits before
    // we touch any child. `resolve_conflict` runs the parent leg and
    // pushes to S3 first; only after that succeeds do we enumerate
    // children and apply the policy. This preserves Decision 12 Q6:
    // children never appear "ahead" of the parent log on S3.
    // The parent leg's time is reported: on a large session it is the one
    // that moves every object (koto#428).
    let started = std::time::Instant::now();
    cloud.resolve_conflict(name, keep)?;
    let elapsed_ms = started.elapsed().as_millis() as u64;

    let children_result = apply_children_policy(cloud, backend, name, children);

    let machine_id = crate::session::version::get_or_create_machine_id()?;
    let sync_status = parent_remote_presence_label(cloud, name);

    let response = serde_json::json!({
        "name": name,
        "keep": keep,
        "children_policy": children_policy_label(children),
        "sync_status": sync_status,
        "machine_id": machine_id,
        "elapsed_ms": elapsed_ms,
        "children": children_result,
    });

    println!("{}", serde_json::to_string_pretty(&response)?);

    // Return an error only if the `auto` policy hit at least one
    // true conflict — that's the case the design reserves for per-child
    // `koto session resolve <child>`. Errored children are reported in
    // the JSON body but do not abort (mirrors per-task spawn-error
    // accumulation elsewhere in the codebase).
    if children == ChildrenPolicy::Auto
        && children_result
            .iter()
            .any(|r| matches!(r.resolution, ChildResolution::Conflict))
    {
        anyhow::bail!(
            "one or more children are in conflict; run `koto session resolve <child>` on each \
             flagged child"
        );
    }

    Ok(())
}

/// Per-child row in the response body, pairing the child's name with
/// its reconciliation outcome. Kept a flat struct so the JSON shape is
/// `{"name": "...", "action": "...", "message": "..."}` rather than a
/// nested object.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChildResolutionRow {
    /// Child session name (fully-qualified `<parent>.<task>`).
    pub name: String,
    /// Flattened `ChildResolution` so the JSON is a single object.
    #[serde(flatten)]
    pub resolution: ChildResolution,
}

/// Enumerate the parent's direct children and apply `policy` to each.
///
/// Runs under `CloudBackend` only; the `handle_resolve` caller has
/// already verified that. Errors from the per-child call surface as
/// `ChildResolution::Errored` entries so siblings continue to process.
fn apply_children_policy(
    cloud: &CloudBackend,
    backend: &Backend,
    parent: &str,
    policy: ChildrenPolicy,
) -> Vec<ChildResolutionRow> {
    let policy_str = children_policy_label(policy);

    // Known v1 limitation: `Backend::list()` on CloudBackend merges
    // remote-only session IDs into the returned `Vec<SessionInfo>`, but
    // the placeholder entries it produces have an empty
    // `parent_workflow` (the remote state file isn't downloaded here).
    // That means the `filter` below drops any child that exists only
    // on S3 (e.g., initialized on another host that hasn't yet synced
    // back locally) because its parent_workflow cannot be recovered.
    // Running `session resolve --children` therefore reconciles only
    // children this host has already observed. A future revision
    // should either (a) HEAD the remote state header to populate
    // parent_workflow, or (b) add a dedicated `list_children(parent)`
    // on the backend so we can enumerate S3 prefixes directly.
    let sessions = match backend.list() {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "warning: session resolve: failed to enumerate children: {}",
                e
            );
            return Vec::new();
        }
    };

    sessions
        .into_iter()
        .filter(|s| s.parent_workflow.as_deref() == Some(parent))
        .map(|child| {
            let resolution = cloud.reconcile_child(&child.id, policy_str);
            ChildResolutionRow {
                name: child.id,
                resolution,
            }
        })
        .collect()
}

/// Map the `ChildrenPolicy` enum to the wire string accepted by
/// `CloudBackend::reconcile_child` and echoed in the JSON response.
fn children_policy_label(p: ChildrenPolicy) -> &'static str {
    match p {
        ChildrenPolicy::Auto => "auto",
        ChildrenPolicy::Skip => "skip",
        ChildrenPolicy::AcceptRemote => "accept-remote",
        ChildrenPolicy::AcceptLocal => "accept-local",
    }
}

/// Probe whether the parent's state file is visible on the remote
/// after reconciliation.
///
/// The returned label (`"fresh"` or `"local_only"`) describes only
/// whether a HEAD on the remote state object succeeded; it does NOT
/// compare bytes or versions. After `resolve_conflict` succeeds, local
/// and remote normally converge on the same content, so the expected
/// label is `"fresh"`. `"local_only"` surfaces the narrow case where
/// S3 was unreachable when the parent leg wrote locally, so the caller
/// doesn't mistake an offline push-back for a clean sync. Downstream
/// machinery that needs byte-level parity must perform its own
/// reconciliation — this label exists purely to flag a missing remote.
fn parent_remote_presence_label(cloud: &CloudBackend, name: &str) -> &'static str {
    if cloud.remote_state_exists(name) {
        "fresh"
    } else {
        "local_only"
    }
}

#[cfg(test)]
mod rebind_tests {
    use super::*;
    use crate::engine::template_source_status::{check_execution_anchor, ExecutionAnchorCheck};
    use crate::engine::types::EventPayload;
    use crate::session::cloud::test_support::{
        cloud_backend_at, header_of, seed_session, serve, state_key_suffix,
    };

    /// koto#310: on the cloud backend a rebind reported success and the
    /// next command's pull restored the old anchor, because the header
    /// rewrite never left the machine. The remote copy has to carry the
    /// new anchor, and the read `koto next` makes from the new directory
    /// has to find it there.
    #[test]
    fn a_cloud_rebind_pushes_the_new_anchor_and_next_from_there_proceeds() {
        let sessions = tempfile::TempDir::new().unwrap();
        let old = tempfile::TempDir::new().unwrap();
        let new = tempfile::TempDir::new().unwrap();
        let old_anchor = std::fs::canonicalize(old.path()).unwrap();
        let new_anchor = std::fs::canonicalize(new.path()).unwrap();

        // The session as `koto init` left it: the same bytes locally and
        // remotely, anchored at the old directory.
        let seeded = seed_session(sessions.path(), "wf", Some(old_anchor.clone()));
        let endpoint = serve(vec![(state_key_suffix("wf"), seeded)]);
        let backend = Backend::Cloud(cloud_backend_at(sessions.path(), endpoint.url.clone()));

        handle_rebind(&backend, "wf", Some(new.path().to_str().unwrap())).unwrap();

        let pushed = endpoint
            .last_put(&state_key_suffix("wf"))
            .expect("rebind pushed the state file");
        assert_eq!(
            header_of(&pushed).execution_dir,
            Some(new_anchor.clone()),
            "the last state push carries the new anchor"
        );

        // What `koto next` does first: a pulling read, then the anchor
        // check against the directory it runs in.
        let (header, events) = backend.read_events("wf").unwrap();
        assert_eq!(header.execution_dir, Some(new_anchor.clone()));
        assert!(events.iter().any(|e| matches!(
            &e.payload,
            EventPayload::ExecutionAnchorRebound { from, to }
                if from.as_deref() == Some(old_anchor.as_path()) && *to == new_anchor
        )));
        assert_eq!(
            check_execution_anchor(header.execution_dir.as_deref(), new.path()),
            ExecutionAnchorCheck::Satisfied { anchor: new_anchor }
        );
    }
}
