use anyhow::Result;

use crate::cli::ChildrenPolicy;
use crate::engine::types::{ValidatedCoordId, ValidatedSessionId};
use crate::session::cloud::{ChildResolution, CloudBackend};
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
        // `sessions/` subdir). Fall back to `~/.koto` when the home
        // directory is resolvable; on substrates that don't have a
        // standard home we leave `koto_root` as the literal `.koto`
        // path which simply means the terminal-index filter sees an
        // empty index (safe over-count, never an under-count).
        let koto_root = dirs::home_dir()
            .map(|h| h.join(".koto"))
            .unwrap_or_else(|| std::path::PathBuf::from(".koto"));
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

    // -- Compose the header --
    let ts = now_iso8601();
    let template_name = template.map(|s| s.to_string());

    let header = StateFileHeader {
        schema_version: 1,
        workflow: name.to_string(),
        template_hash: String::new(),
        created_at: ts.clone(),
        parent_workflow: Some(validated_parent.as_str().to_string()),
        template_source_dir: None,
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
        s.push_str("\"");
        s.push_str(&"a".repeat(INPUTS_MAX_BYTES));
        s.push_str("\"");
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
        s.push_str("0");
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
        s.push_str("0");
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
pub fn handle_update(backend: &dyn SessionBackend, name: &str, intent: &str) -> anyhow::Result<()> {
    use crate::engine::{
        persistence,
        types::{now_iso8601, EventPayload},
    };
    use crate::session::state_file_name;

    if intent.len() > 1024 {
        anyhow::bail!(
            "intent string too long: {} characters (max 1024)",
            intent.len()
        );
    }

    let dir = backend.session_dir(name);
    if !backend.exists(name) {
        anyhow::bail!("session '{}' does not exist", name);
    }

    let state_path = dir.join(state_file_name(name));
    let payload = EventPayload::IntentUpdated {
        intent: intent.to_string(),
    };
    persistence::append_event(&state_path, &payload, &now_iso8601())?;
    Ok(())
}

/// Print the absolute session directory path.
pub fn handle_dir(backend: &dyn SessionBackend, name: &str) -> Result<()> {
    let dir = backend.session_dir(name);
    println!("{}", dir.display());
    Ok(())
}

/// Print all sessions as a JSON array.
pub fn handle_list(backend: &dyn SessionBackend) -> Result<()> {
    let sessions = backend.list()?;
    println!("{}", serde_json::to_string_pretty(&sessions)?);
    Ok(())
}

/// Remove a session directory. Idempotent: succeeds even if the session doesn't exist.
pub fn handle_cleanup(backend: &dyn SessionBackend, name: &str) -> Result<()> {
    backend.cleanup(name)?;
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
    cloud.resolve_conflict(name, keep)?;

    let children_result = apply_children_policy(cloud, backend, name, children);

    let machine_id = crate::session::version::get_or_create_machine_id()?;
    let sync_status = parent_remote_presence_label(cloud, name);

    let response = serde_json::json!({
        "name": name,
        "keep": keep,
        "children_policy": children_policy_label(children),
        "sync_status": sync_status,
        "machine_id": machine_id,
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
