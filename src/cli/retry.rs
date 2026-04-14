//! Batch retry handler (Issue #14).
//!
//! `retry_failed` is a reserved top-level evidence key. When an agent
//! submits `{"retry_failed": {"children": ["...", ...]}}` to a batch
//! parent, `handle_next` dispatches to [`handle_retry_failed`] in this
//! module BEFORE the normal `advance_until_stop` + `run_batch_scheduler`
//! flow.
//!
//! # Semantics (Decision 9 / 5.4 in the design)
//!
//! 1. Validate the retry set atomically (R10). Any single rejected
//!    child aborts the whole call — no disk state changes.
//! 2. Append `EvidenceSubmitted { retry_failed: <payload> }` to the
//!    parent's log.
//! 3. Append a clearing `EvidenceSubmitted { retry_failed: null }`
//!    event so subsequent epochs observe the submission as "consumed"
//!    yet the `present` matcher still fires on this epoch.
//! 4. For each named child, dispatch on its current outcome:
//!    - `failure` / `done_blocked` (real-template terminal with
//!      `failure: true`) → append a `Rewound` event targeting the
//!      template's initial_state (bumps the child's epoch).
//!    - `skipped` (stale skip marker) → delete-and-respawn via
//!      `init_child_as_skip_marker_from_parent`; the scheduler will
//!      re-materialize on the next tick based on current upstream
//!      outcomes.
//!    - `spawn_failed` → retry-respawn: cleanup (if any tempfile) and
//!      re-invoke `init_child_from_parent` with the CURRENT submission
//!      entry. This re-runs `init_state_file` for a child whose
//!      previous spawn failed.
//!
//! Parent writes happen first. Under `CloudBackend`, this means the
//! parent's state file is pushed to S3 before any child mutation (the
//! backend's `append_event` implementation auto-pushes after every
//! append), satisfying Decision 12 Q6's push-parent-first ordering.
//!
//! Control then returns to `handle_next`'s normal flow: the advance
//! loop fires the template-declared transition on
//! `when: evidence.retry_failed: present`.

use std::collections::{BTreeMap, HashSet};

use crate::cli::batch_error::{BatchError, ChildEligibility, ChildOutcome, InvalidRetryReason};
use crate::cli::init_child::{
    init_child_as_skip_marker_from_parent, init_child_from_parent, TemplateCompileCache,
};
use crate::cli::task_spawn_error::{SpawnErrorKind, TaskSpawnError};
use crate::engine::batch_validation::TaskEntry;
use crate::engine::persistence::derive_state_from_log;
use crate::engine::types::{now_iso8601, Event, EventPayload};
use crate::session::SessionBackend;
use crate::template::types::CompiledTemplate;

// ---------------------------------------------------------------------
//  Input parsing
// ---------------------------------------------------------------------

/// Parsed `retry_failed` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryFailedPayload {
    /// Short task names (NOT composed `<parent>.<task>`) to retry.
    pub children: Vec<String>,
    /// Placeholder for a future flag that lets callers opt out of
    /// propagating the retry to skipped dependents. Kept on the type
    /// so the wire-level payload can evolve without breaking callers.
    pub include_skipped: bool,
}

/// Attempt to parse the `retry_failed` key off a submitted evidence
/// object. Returns `Ok(Some(payload))` when the key is present and
/// well-formed, `Ok(None)` when the evidence object does not carry
/// the key at all, and `Err(_)` when the key is present but malformed
/// (so the caller can surface a typed rejection rather than silently
/// falling back to the normal evidence path).
pub fn parse_retry_failed(
    data: &serde_json::Value,
) -> Result<Option<RetryFailedPayload>, InvalidRetryReason> {
    let obj = match data.as_object() {
        Some(o) => o,
        None => return Ok(None),
    };
    let raw = match obj.get("retry_failed") {
        Some(v) => v,
        None => return Ok(None),
    };

    // Malformed shape: the key exists but its value is not an object.
    let payload_obj = match raw.as_object() {
        Some(o) => o,
        None => {
            // Treat as EmptyChildList so the precedence rules still
            // apply — a non-object payload is semantically equivalent
            // to "no children to retry" from the validator's standpoint.
            return Err(InvalidRetryReason::EmptyChildList);
        }
    };

    let children: Vec<String> = match payload_obj.get("children") {
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        Some(_) => return Err(InvalidRetryReason::EmptyChildList),
        None => Vec::new(),
    };

    if children.is_empty() {
        return Err(InvalidRetryReason::EmptyChildList);
    }

    let include_skipped = payload_obj
        .get("include_skipped")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    Ok(Some(RetryFailedPayload {
        children,
        include_skipped,
    }))
}

/// Snapshot of a child used during R10 validation.
#[derive(Debug, Clone)]
pub(crate) struct RetryChildSnapshot {
    /// Short task name (as the agent submitted).
    pub task: String,
    /// Composed workflow name (`<parent>.<task>`).
    pub composed: String,
    /// Current outcome. Drives dispatch in [`dispatch_retry_paths`].
    pub outcome: ChildOutcome,
    /// True when this child's compiled template carries a
    /// `materialize_children` hook on any state — the marker for a
    /// batch parent. Cross-level retry is rejected in v1. Read only
    /// during aggregation; retained on the struct so tests and future
    /// extensions can inspect it without round-tripping the template.
    #[allow(dead_code)]
    pub is_batch_parent: bool,
    /// Template path recorded on the child's `WorkflowInitialized`
    /// event. Used for the Rewound target (re-running from initial
    /// state requires re-reading the template).
    pub template_path: Option<String>,
    /// Current state name (from `derive_state_from_log`).
    pub current_state: Option<String>,
}

// ---------------------------------------------------------------------
//  R10 validation
// ---------------------------------------------------------------------

/// Validate the retry request. Returns the ordered list of snapshots
/// (matching the submitted order) when the whole submission is valid;
/// otherwise returns a typed [`InvalidRetryReason`] that already
/// respects the Decision 9 / Issue #10 precedence.
pub(crate) fn validate_retry_request(
    backend: &dyn SessionBackend,
    parent_name: &str,
    payload: &RetryFailedPayload,
    extra_top_level_fields: &[String],
) -> Result<Vec<RetryChildSnapshot>, InvalidRetryReason> {
    // R10 short-circuit: empty child list.
    if payload.children.is_empty() {
        return Err(InvalidRetryReason::EmptyChildList);
    }

    // Dedupe names while preserving submission order.
    let mut seen: HashSet<String> = HashSet::new();
    let names: Vec<String> = payload
        .children
        .iter()
        .filter(|n| seen.insert((*n).clone()))
        .cloned()
        .collect();

    let mut snapshots: Vec<RetryChildSnapshot> = Vec::with_capacity(names.len());
    let mut unknown: Vec<String> = Vec::new();
    let mut batch_parents: Vec<String> = Vec::new();
    let mut ineligible: Vec<ChildEligibility> = Vec::new();

    for task in &names {
        let composed = format!("{}.{}", parent_name, task);
        let (_, events) = match backend.read_events(&composed) {
            Ok(x) => x,
            Err(_) => {
                // Child name is not materialized for this parent.
                unknown.push(task.clone());
                continue;
            }
        };

        let current_state = derive_state_from_log(&events);
        let template_path = events.iter().find_map(|e| match &e.payload {
            EventPayload::WorkflowInitialized { template_path, .. } => Some(template_path.clone()),
            _ => None,
        });

        // Load the child's compiled template to determine outcome and
        // check for a batch-parent marker.
        let (compiled, outcome, is_batch_parent) =
            classify_child_outcome(template_path.as_deref(), current_state.as_deref(), &events);

        if is_batch_parent {
            batch_parents.push(task.clone());
            // Still push the snapshot so dispatch sees a consistent
            // view if precedence bumps this to a higher-rank variant.
            snapshots.push(RetryChildSnapshot {
                task: task.clone(),
                composed,
                outcome,
                is_batch_parent,
                template_path,
                current_state,
            });
            continue;
        }

        if !outcome_is_retryable(outcome) {
            ineligible.push(ChildEligibility {
                name: task.clone(),
                current_outcome: outcome,
            });
        }

        snapshots.push(RetryChildSnapshot {
            task: task.clone(),
            composed,
            outcome,
            is_batch_parent,
            template_path,
            current_state,
        });

        // Suppress the "unused" lint for the compiled binding; it is
        // held only so the function explicitly documents that we do
        // parse the child template during validation.
        let _ = compiled;
    }

    // Accumulate the mixed-evidence check.
    let mixed: Vec<String> = extra_top_level_fields
        .iter()
        .filter(|k| k.as_str() != "retry_failed")
        .cloned()
        .collect();

    let mut reasons: Vec<InvalidRetryReason> = Vec::new();
    if !unknown.is_empty() {
        reasons.push(InvalidRetryReason::UnknownChildren { children: unknown });
    }
    if !batch_parents.is_empty() {
        reasons.push(InvalidRetryReason::ChildIsBatchParent {
            children: batch_parents,
        });
    }
    if !ineligible.is_empty() {
        reasons.push(InvalidRetryReason::ChildNotEligible {
            children: ineligible,
        });
    }
    if !mixed.is_empty() {
        reasons.push(InvalidRetryReason::MixedWithOtherEvidence {
            extra_fields: mixed,
        });
    }

    if let Some(reason) = InvalidRetryReason::aggregate(reasons) {
        return Err(reason);
    }

    Ok(snapshots)
}

/// True when the outcome can be retried via Issue #14's machinery.
/// `spawn_failed` joins `failure` / `skipped` here (Round-3 polish:
/// "R10 accepts spawn_failed children for retry-respawn").
fn outcome_is_retryable(outcome: ChildOutcome) -> bool {
    matches!(
        outcome,
        ChildOutcome::Failure | ChildOutcome::Skipped | ChildOutcome::SpawnFailed
    )
}

/// Derive the child's current outcome + whether it is itself a batch
/// parent. The outcome computation mirrors the batch scheduler's
/// classification (`src/cli/batch.rs`). A batch-parent marker is a
/// state in the compiled template that declares `materialize_children`.
fn classify_child_outcome(
    template_path: Option<&str>,
    current_state: Option<&str>,
    _events: &[Event],
) -> (Option<CompiledTemplate>, ChildOutcome, bool) {
    let Some(path) = template_path else {
        // No template path on disk — treat as pending, never retryable.
        return (None, ChildOutcome::Pending, false);
    };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return (None, ChildOutcome::Pending, false),
    };
    let compiled: CompiledTemplate = match serde_json::from_slice(&bytes) {
        Ok(c) => c,
        Err(_) => return (None, ChildOutcome::Pending, false),
    };
    let is_batch_parent = compiled
        .states
        .values()
        .any(|s| s.materialize_children.is_some());

    let outcome = match current_state.and_then(|s| compiled.states.get(s)) {
        Some(state) => {
            if state.terminal {
                if state.failure {
                    ChildOutcome::Failure
                } else if state.skipped_marker {
                    ChildOutcome::Skipped
                } else {
                    ChildOutcome::Success
                }
            } else {
                ChildOutcome::Pending
            }
        }
        None => ChildOutcome::Pending,
    };

    (Some(compiled), outcome, is_batch_parent)
}

// ---------------------------------------------------------------------
//  Dispatch
// ---------------------------------------------------------------------

/// Which retry path fired for a given child.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryAction {
    /// Real-template failure — append a `Rewound` event so the child's
    /// epoch bumps and the usual flow re-runs from `initial_state`.
    Rewind,
    /// Stale skip marker — delete-and-respawn via the skip-marker path
    /// using the CURRENT submission entry.
    RespawnSkipped,
    /// Prior spawn failed — retry-respawn via `init_state_file` using
    /// the CURRENT submission entry.
    RespawnFailed,
}

/// Per-child dispatch record. Surfaced in the response so agents know
/// exactly which path fired for each named child.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RetryDispatchRecord {
    /// Short task name as submitted.
    pub task: String,
    /// Composed workflow name (`<parent>.<task>`).
    pub composed: String,
    /// Retry path fired for this child.
    pub retry_action: RetryAction,
}

/// Outcome of `handle_retry_failed`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RetryOutcome {
    /// Per-child dispatch records in submission order.
    pub dispatched: Vec<RetryDispatchRecord>,
    /// Per-child spawn errors accumulated during retry-respawn paths.
    /// Siblings keep processing on a partial failure (mirrors the batch
    /// scheduler's per-task accumulation).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errored: Vec<TaskSpawnError>,
}

/// Top-level entry point. Intercepts in `handle_next` BEFORE
/// `advance_until_stop` runs.
///
/// Ordering (Decision 12 Q6 — push-parent-first):
/// 1. Validate R10 atomically. Any violation aborts without touching
///    disk.
/// 2. Append `EvidenceSubmitted { retry_failed: <payload> }`.
/// 3. Append clearing `EvidenceSubmitted { retry_failed: null }`. Under
///    `CloudBackend`, both appends trigger an S3 push automatically
///    (see `SessionBackend::append_event`), so the parent log is
///    durable before any child mutation.
/// 4. Dispatch per-child retry paths.
#[allow(clippy::result_large_err)]
pub fn handle_retry_failed(
    backend: &dyn SessionBackend,
    parent_name: &str,
    parent_current_state: &str,
    payload: &RetryFailedPayload,
    extra_top_level_fields: &[String],
    submitter_cwd: Option<std::path::PathBuf>,
    submitted_entries: &[TaskEntry],
) -> Result<RetryOutcome, BatchError> {
    // Step 1: R10 validation. All-or-nothing — no disk writes on failure.
    let snapshots = validate_retry_request(backend, parent_name, payload, extra_top_level_fields)
        .map_err(|reason| BatchError::InvalidRetryRequest { reason })?;

    // Step 2: append the retry_failed evidence event. This carries the
    // original payload for audit; the `present` matcher sees the key on
    // the next advance pass regardless of value.
    let submission_value = payload_to_json(payload);
    let mut submission_fields: std::collections::HashMap<String, serde_json::Value> =
        std::collections::HashMap::new();
    submission_fields.insert("retry_failed".to_string(), submission_value);

    let parent_submit = EventPayload::EvidenceSubmitted {
        state: parent_current_state.to_string(),
        fields: submission_fields,
        submitter_cwd: submitter_cwd.clone(),
    };
    backend
        .append_event(parent_name, &parent_submit, &now_iso8601())
        .map_err(|e| BatchError::BackendError {
            message: format!("failed to append retry_failed evidence: {}", e),
            retryable: false,
        })?;

    // Step 3: clearing event. Later epochs observe `retry_failed: null`
    // in merged evidence; the `present` matcher still fires because
    // `contains_key` returns true.
    let mut clearing_fields: std::collections::HashMap<String, serde_json::Value> =
        std::collections::HashMap::new();
    clearing_fields.insert("retry_failed".to_string(), serde_json::Value::Null);
    let clearing = EventPayload::EvidenceSubmitted {
        state: parent_current_state.to_string(),
        fields: clearing_fields,
        submitter_cwd,
    };
    backend
        .append_event(parent_name, &clearing, &now_iso8601())
        .map_err(|e| BatchError::BackendError {
            message: format!("failed to append retry_failed clearing event: {}", e),
            retryable: false,
        })?;

    // Step 4: per-child dispatch. The submitted_entries slice is the
    // CURRENT submission's task list, parsed fresh upstream; respawn
    // paths read it for the entry to re-init.
    let mut dispatched: Vec<RetryDispatchRecord> = Vec::new();
    let mut errored: Vec<TaskSpawnError> = Vec::new();
    let mut cache = TemplateCompileCache::new();

    // Index current submission entries by short task name. An empty
    // slice is fine — agents may submit retry_failed without a `tasks`
    // entry. In that case the respawn paths fall back to the snapshot's
    // existing `spawn_entry` by loading the previous WorkflowInitialized
    // event (best-effort).
    let entries_by_name: BTreeMap<&str, &TaskEntry> = submitted_entries
        .iter()
        .map(|t| (t.name.as_str(), t))
        .collect();

    for snapshot in &snapshots {
        let retry_action = match snapshot.outcome {
            ChildOutcome::Failure => {
                let ok = write_rewound_event(backend, snapshot);
                if let Err(e) = ok {
                    errored.push(TaskSpawnError::new(
                        &snapshot.composed,
                        SpawnErrorKind::IoError,
                        format!("failed to write Rewound event: {}", e),
                    ));
                }
                RetryAction::Rewind
            }
            ChildOutcome::Skipped => {
                respawn_skipped_child(
                    backend,
                    parent_name,
                    snapshot,
                    entries_by_name.get(snapshot.task.as_str()).copied(),
                    &mut cache,
                    &mut errored,
                );
                RetryAction::RespawnSkipped
            }
            ChildOutcome::SpawnFailed => {
                respawn_failed_child(
                    backend,
                    parent_name,
                    snapshot,
                    entries_by_name.get(snapshot.task.as_str()).copied(),
                    &mut cache,
                    &mut errored,
                );
                RetryAction::RespawnFailed
            }
            // validate_retry_request already rejects these — the match
            // is exhaustive as a defense in depth.
            ChildOutcome::Pending | ChildOutcome::Success | ChildOutcome::Blocked => continue,
        };

        dispatched.push(RetryDispatchRecord {
            task: snapshot.task.clone(),
            composed: snapshot.composed.clone(),
            retry_action,
        });
    }

    Ok(RetryOutcome {
        dispatched,
        errored,
    })
}

// ---------------------------------------------------------------------
//  Per-path helpers
// ---------------------------------------------------------------------

/// Append a `Rewound` event on a failed real-template child. The event
/// bumps the child's epoch so the usual advance-loop flow re-runs from
/// `initial_state`. We do NOT remove the child's state file — the event
/// log carries the full history and a later `koto rewind`/tick path
/// reads the event to derive the new current state.
fn write_rewound_event(
    backend: &dyn SessionBackend,
    snapshot: &RetryChildSnapshot,
) -> Result<(), anyhow::Error> {
    // Compute the target state: the child's template initial_state.
    let target = child_initial_state(snapshot.template_path.as_deref()).unwrap_or_else(|| {
        // Fallback: use current_state as target (vacuous rewind) rather
        // than crashing. Realistically unreachable because R10 rejects
        // children without a readable template.
        snapshot
            .current_state
            .clone()
            .unwrap_or_else(|| "unknown".to_string())
    });
    let from = snapshot
        .current_state
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let payload = EventPayload::Rewound { from, to: target };
    backend.append_event(&snapshot.composed, &payload, &now_iso8601())?;
    Ok(())
}

/// Return the `initial_state` from the child's compiled template on disk.
fn child_initial_state(template_path: Option<&str>) -> Option<String> {
    let path = template_path?;
    let bytes = std::fs::read(path).ok()?;
    let compiled: CompiledTemplate = serde_json::from_slice(&bytes).ok()?;
    Some(compiled.initial_state.clone())
}

/// Delete-and-respawn a skip-marker child. Uses the CURRENT submission
/// entry (matches Issue #13's reclassification mechanism) so the
/// respawn honors any template/vars/waits_on changes the agent made.
fn respawn_skipped_child(
    backend: &dyn SessionBackend,
    parent_name: &str,
    snapshot: &RetryChildSnapshot,
    entry: Option<&TaskEntry>,
    cache: &mut TemplateCompileCache,
    errored: &mut Vec<TaskSpawnError>,
) {
    if let Err(e) = backend.cleanup(&snapshot.composed) {
        errored.push(TaskSpawnError::new(
            &snapshot.composed,
            SpawnErrorKind::IoError,
            format!("failed to remove stale skip marker: {}", e),
        ));
        return;
    }

    let (template_path_str, vars, _waits_on) = match resolve_retry_entry(snapshot, entry) {
        Some(v) => v,
        None => {
            errored.push(TaskSpawnError::new(
                &snapshot.composed,
                SpawnErrorKind::IoError,
                "no template path recorded for retry-respawn".to_string(),
            ));
            return;
        }
    };

    let template_path = std::path::Path::new(&template_path_str);
    let skipped_state = match find_skipped_state_name(template_path) {
        Ok(s) => s,
        Err(msg) => {
            errored.push(TaskSpawnError::new(
                &snapshot.composed,
                SpawnErrorKind::TemplateCompileFailed,
                msg,
            ));
            return;
        }
    };

    if let Err(err) = init_child_as_skip_marker_from_parent(
        backend,
        Some(parent_name),
        &snapshot.composed,
        template_path,
        &vars,
        cache,
        None,
        &skipped_state,
    ) {
        errored.push(err);
    }
}

/// Retry-respawn for a `spawn_failed` child. Cleanup is best-effort —
/// a prior failure may have left a tempfile behind; `backend.cleanup`
/// is idempotent on missing directories.
fn respawn_failed_child(
    backend: &dyn SessionBackend,
    parent_name: &str,
    snapshot: &RetryChildSnapshot,
    entry: Option<&TaskEntry>,
    cache: &mut TemplateCompileCache,
    errored: &mut Vec<TaskSpawnError>,
) {
    if let Err(e) = backend.cleanup(&snapshot.composed) {
        errored.push(TaskSpawnError::new(
            &snapshot.composed,
            SpawnErrorKind::IoError,
            format!("failed to clean stale spawn_failed child: {}", e),
        ));
        return;
    }

    let (template_path_str, vars, _waits_on) = match resolve_retry_entry(snapshot, entry) {
        Some(v) => v,
        None => {
            errored.push(TaskSpawnError::new(
                &snapshot.composed,
                SpawnErrorKind::IoError,
                "no template path recorded for retry-respawn".to_string(),
            ));
            return;
        }
    };

    let template_path = std::path::Path::new(&template_path_str);
    if let Err(err) = init_child_from_parent(
        backend,
        Some(parent_name),
        &snapshot.composed,
        template_path,
        &vars,
        cache,
        None,
    ) {
        errored.push(err);
    }
}

/// Pick the template path and variable bindings for a respawn. Prefers
/// the CURRENT submission entry when the agent supplied one for this
/// task name; falls back to the child's own recorded template when
/// retry_failed was submitted without an accompanying task list.
fn resolve_retry_entry(
    snapshot: &RetryChildSnapshot,
    entry: Option<&TaskEntry>,
) -> Option<(String, Vec<String>, Vec<String>)> {
    let template_path = match entry.and_then(|e| e.template.clone()) {
        Some(t) => t,
        None => snapshot.template_path.clone()?,
    };
    let vars = entry.map(|e| vars_to_cli_args(&e.vars)).unwrap_or_default();
    let waits_on = entry.map(|e| e.waits_on.clone()).unwrap_or_default();
    Some((template_path, vars, waits_on))
}

/// Convert a `TaskEntry::vars` map into the `KEY=VALUE` CLI form.
fn vars_to_cli_args(vars: &BTreeMap<String, serde_json::Value>) -> Vec<String> {
    vars.iter()
        .map(|(k, v)| {
            let s = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            format!("{}={}", k, s)
        })
        .collect()
}

/// Locate the state name that carries `skipped_marker: true`. F5
/// guarantees one exists on any batch-eligible child template.
fn find_skipped_state_name(template_path: &std::path::Path) -> Result<String, String> {
    let bytes = std::fs::read(template_path)
        .map_err(|e| format!("failed to read template {}: {}", template_path.display(), e))?;
    let compiled: CompiledTemplate = serde_json::from_slice(&bytes).map_err(|e| {
        format!(
            "failed to parse template {}: {}",
            template_path.display(),
            e
        )
    })?;
    for (name, state) in &compiled.states {
        if state.terminal && state.skipped_marker {
            return Ok(name.clone());
        }
    }
    Err(format!(
        "template {} has no terminal state with `skipped_marker: true` (F5 violation)",
        template_path.display()
    ))
}

fn payload_to_json(payload: &RetryFailedPayload) -> serde_json::Value {
    serde_json::json!({
        "children": payload.children,
        "include_skipped": payload.include_skipped,
    })
}

// ---------------------------------------------------------------------
//  reserved_actions synthesis
// ---------------------------------------------------------------------

/// One entry under the top-level `reserved_actions` field.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReservedAction {
    pub action: String,
    pub label: String,
    pub description: String,
    pub applies_to: Vec<String>,
    pub invocation: String,
}

/// Synthesize the `reserved_actions` array for a response whose gate
/// output reports `any_failed` or `any_skipped`. `parent_name` is the
/// batch-parent workflow; `retryable_children` is the list of short
/// task names (NOT composed names) currently in a retryable outcome.
///
/// Round-3 polish: the `invocation` string uses POSIX-safe single-quote
/// wrapping so agent shells (including non-bash shells like fish) can
/// paste-and-run without quoting surprises.
pub fn synthesize_reserved_actions(
    parent_name: &str,
    retryable_children: &[String],
) -> Vec<ReservedAction> {
    if retryable_children.is_empty() {
        return Vec::new();
    }
    let payload = serde_json::json!({
        "retry_failed": {
            "children": retryable_children,
        }
    });
    // Serialize compactly. Single-quote wrap the whole JSON string so
    // shell interpolation is disabled; escape any embedded `'`.
    let payload_str = serde_json::to_string(&payload).unwrap_or_default();
    let quoted_payload = shell_single_quote(&payload_str);
    let quoted_parent = shell_single_quote(parent_name);
    let invocation = format!("koto next {} --with-data {}", quoted_parent, quoted_payload);
    vec![ReservedAction {
        action: "retry_failed".to_string(),
        label: "Retry failed children".to_string(),
        description: "Re-run children whose outcome is failure, skipped, or spawn_failed."
            .to_string(),
        applies_to: retryable_children.to_vec(),
        invocation,
    }]
}

/// POSIX-safe single-quote wrap. The shell spec guarantees `'...'` has
/// no interpolation, so the only character we must escape is the single
/// quote itself: `'` → `'\''` (close, escape, reopen).
pub fn shell_single_quote(s: &str) -> String {
    if !s.contains('\'') {
        return format!("'{}'", s);
    }
    let mut out = String::with_capacity(s.len() + 4);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Report whether a gate's structured output signals that one or more
/// children are retryable. Mirrors the vocabulary the Issue #15 gate
/// extension emits (`any_failed`, `any_skipped`, `any_spawn_failed`).
/// Issue #14 reads whichever of those keys are present today; a gate
/// that only emits the legacy `all_complete` shape returns `false`
/// because there is no retryable set to advertise.
pub fn gate_output_reports_retryable(output: &serde_json::Value) -> bool {
    let obj = match output.as_object() {
        Some(o) => o,
        None => return false,
    };
    obj.get("any_failed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || obj
            .get("any_skipped")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        || obj
            .get("any_spawn_failed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
}

// ---------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_retry_failed ------------------------------------------

    #[test]
    fn parse_returns_none_when_key_absent() {
        let v = serde_json::json!({"tasks": []});
        assert_eq!(parse_retry_failed(&v).unwrap(), None);
    }

    #[test]
    fn parse_extracts_children_list() {
        let v = serde_json::json!({"retry_failed": {"children": ["A", "B"]}});
        let p = parse_retry_failed(&v).unwrap().expect("payload");
        assert_eq!(p.children, vec!["A".to_string(), "B".to_string()]);
        assert!(p.include_skipped);
    }

    #[test]
    fn parse_accepts_include_skipped_override() {
        let v = serde_json::json!({
            "retry_failed": {"children": ["A"], "include_skipped": false}
        });
        let p = parse_retry_failed(&v).unwrap().expect("payload");
        assert!(!p.include_skipped);
    }

    #[test]
    fn parse_rejects_empty_children_list() {
        let v = serde_json::json!({"retry_failed": {"children": []}});
        assert!(matches!(
            parse_retry_failed(&v).unwrap_err(),
            InvalidRetryReason::EmptyChildList
        ));
    }

    #[test]
    fn parse_rejects_non_object_payload() {
        let v = serde_json::json!({"retry_failed": "oops"});
        assert!(matches!(
            parse_retry_failed(&v).unwrap_err(),
            InvalidRetryReason::EmptyChildList
        ));
    }

    // --- shell_single_quote ------------------------------------------

    #[test]
    fn shell_quote_wraps_simple_string() {
        assert_eq!(shell_single_quote("abc"), "'abc'");
    }

    #[test]
    fn shell_quote_escapes_embedded_single_quote() {
        // Input: foo'bar
        // Output: 'foo'\''bar'  (close, escaped-quote, reopen)
        assert_eq!(shell_single_quote("foo'bar"), "'foo'\\''bar'");
    }

    #[test]
    fn shell_quote_is_idempotent_on_json() {
        // The retry_failed payload is JSON — contains double quotes but
        // never single quotes. Wrapping in single quotes is lossless.
        let payload = r#"{"retry_failed":{"children":["A"]}}"#;
        let quoted = shell_single_quote(payload);
        assert!(quoted.starts_with('\'') && quoted.ends_with('\''));
        assert_eq!(&quoted[1..quoted.len() - 1], payload);
    }

    #[test]
    fn shell_quote_handles_child_name_with_single_quote() {
        // Rare but possible: a child name containing a single quote.
        let quoted = shell_single_quote("weird'name");
        assert_eq!(quoted, "'weird'\\''name'");
    }

    #[test]
    fn shell_quote_handles_child_name_with_spaces() {
        let quoted = shell_single_quote("task with space");
        assert_eq!(quoted, "'task with space'");
    }

    // --- synthesize_reserved_actions ---------------------------------

    #[test]
    fn synthesize_returns_empty_when_no_retryable() {
        let out = synthesize_reserved_actions("p", &[]);
        assert!(out.is_empty());
    }

    #[test]
    fn synthesize_emits_invocation_with_payload_children() {
        let out = synthesize_reserved_actions("parent", &["A".to_string(), "B".to_string()]);
        assert_eq!(out.len(), 1);
        let entry = &out[0];
        assert_eq!(entry.action, "retry_failed");
        assert_eq!(entry.applies_to, vec!["A".to_string(), "B".to_string()]);
        // The invocation is `koto next <quoted parent> --with-data <quoted json>`.
        assert!(entry
            .invocation
            .starts_with("koto next 'parent' --with-data '"));
        // It must contain both child names.
        assert!(entry.invocation.contains("\"A\""));
        assert!(entry.invocation.contains("\"B\""));
    }

    #[test]
    fn synthesize_quotes_parent_name_with_single_quote() {
        let out = synthesize_reserved_actions("pa'rent", &["x".to_string()]);
        let invocation = &out[0].invocation;
        // The single quote in the parent name must be escaped via
        // `'\''` so the shell re-opens the quoted string after.
        assert!(invocation.contains("'pa'\\''rent'"), "got: {}", invocation);
    }

    // --- gate_output_reports_retryable -------------------------------

    #[test]
    fn gate_output_flags_any_failed() {
        let output = serde_json::json!({"all_complete": true, "any_failed": true});
        assert!(gate_output_reports_retryable(&output));
    }

    #[test]
    fn gate_output_flags_any_skipped() {
        let output = serde_json::json!({"any_skipped": true});
        assert!(gate_output_reports_retryable(&output));
    }

    #[test]
    fn gate_output_flags_any_spawn_failed() {
        let output = serde_json::json!({"any_spawn_failed": true});
        assert!(gate_output_reports_retryable(&output));
    }

    #[test]
    fn gate_output_silent_without_any_retry_keys() {
        let output = serde_json::json!({"all_complete": true});
        assert!(!gate_output_reports_retryable(&output));
    }

    // --- InvalidRetryReason precedence -------------------------------

    #[test]
    fn precedence_unknown_children_sorts_first() {
        let reasons = vec![
            InvalidRetryReason::MixedWithOtherEvidence {
                extra_fields: vec!["other".to_string()],
            },
            InvalidRetryReason::UnknownChildren {
                children: vec!["x".to_string()],
            },
        ];
        let agg = InvalidRetryReason::aggregate(reasons).expect("aggregate");
        let reasons = match agg {
            InvalidRetryReason::MultipleReasons { reasons } => reasons,
            other => panic!("expected MultipleReasons, got {:?}", other),
        };
        assert!(matches!(
            reasons[0],
            InvalidRetryReason::UnknownChildren { .. }
        ));
        assert!(matches!(
            reasons[1],
            InvalidRetryReason::MixedWithOtherEvidence { .. }
        ));
    }

    #[test]
    fn precedence_child_is_batch_parent_before_child_not_eligible() {
        let reasons = vec![
            InvalidRetryReason::ChildNotEligible { children: vec![] },
            InvalidRetryReason::ChildIsBatchParent {
                children: vec!["a".to_string()],
            },
        ];
        let agg = InvalidRetryReason::aggregate(reasons).expect("aggregate");
        let reasons = match agg {
            InvalidRetryReason::MultipleReasons { reasons } => reasons,
            _ => panic!("expected MultipleReasons"),
        };
        assert!(matches!(
            reasons[0],
            InvalidRetryReason::ChildIsBatchParent { .. }
        ));
        assert!(matches!(
            reasons[1],
            InvalidRetryReason::ChildNotEligible { .. }
        ));
    }

    #[test]
    fn precedence_full_stack_orders_all_four() {
        // Submit all four reasons in reverse precedence order; assert
        // the aggregator re-sorts them into the canonical sequence.
        let reasons = vec![
            InvalidRetryReason::RetryAlreadyInProgress,
            InvalidRetryReason::MixedWithOtherEvidence {
                extra_fields: vec!["k".to_string()],
            },
            InvalidRetryReason::ChildNotEligible { children: vec![] },
            InvalidRetryReason::ChildIsBatchParent { children: vec![] },
            InvalidRetryReason::UnknownChildren { children: vec![] },
        ];
        let agg = InvalidRetryReason::aggregate(reasons).expect("aggregate");
        let reasons = match agg {
            InvalidRetryReason::MultipleReasons { reasons } => reasons,
            _ => panic!("expected MultipleReasons"),
        };
        let variants: Vec<&str> = reasons
            .iter()
            .map(|r| match r {
                InvalidRetryReason::UnknownChildren { .. } => "unknown",
                InvalidRetryReason::ChildIsBatchParent { .. } => "batch_parent",
                InvalidRetryReason::ChildNotEligible { .. } => "not_eligible",
                InvalidRetryReason::MixedWithOtherEvidence { .. } => "mixed",
                InvalidRetryReason::RetryAlreadyInProgress => "in_progress",
                _ => "other",
            })
            .collect();
        assert_eq!(
            variants,
            vec![
                "unknown",
                "batch_parent",
                "not_eligible",
                "mixed",
                "in_progress"
            ]
        );
    }

    #[test]
    fn aggregate_single_violation_does_not_wrap() {
        let reasons = vec![InvalidRetryReason::EmptyChildList];
        let agg = InvalidRetryReason::aggregate(reasons).expect("aggregate");
        assert!(matches!(agg, InvalidRetryReason::EmptyChildList));
    }

    #[test]
    fn aggregate_empty_returns_none() {
        assert!(InvalidRetryReason::aggregate(vec![]).is_none());
    }
}
