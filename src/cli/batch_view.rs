//! Shared batch-view derivation for `koto status` and
//! `koto workflows --children`.
//!
//! [`derive_batch_view`] produces a single `BatchView` snapshot that
//! backs both the `batch` section of `koto status <parent>` and the
//! per-row metadata added to `koto workflows --children <parent>`
//! rows. The goal is that the two commands can never diverge — they
//! read the same view and project fields as needed.
//!
//! See `docs/designs/DESIGN-batch-child-spawning.md` Decision 6 and
//! Decision 13 for the field-level schema.

use serde::{Deserialize, Serialize};

use crate::cli::batch::{
    build_children_complete_output, child_state_flags, derive_batch_phase,
    find_most_recent_batch_finalized, BatchFinalView, TaskOutcome,
};
use crate::engine::batch_validation::TaskEntry;
use crate::engine::persistence::derive_state_from_log;
use crate::engine::types::{Event, EventPayload};
use crate::session::SessionBackend;
use crate::template::types::CompiledTemplate;

/// Top-level snapshot rendered into `koto status`'s `batch` section
/// and consumed row-by-row by `koto workflows --children`.
///
/// Field set mirrors DESIGN Decision 6:
/// - `summary`: aggregate counts.
/// - `tasks`: per-task entries in submission / on-disk order.
/// - `ready` / `blocked` / `skipped` / `failed`: convenience name
///   vectors for agents. Dropped when the batch has reached its
///   terminal final phase (Round-3 polish).
/// - `phase`: `"active"` while the parent stays in a
///   `materialize_children` state; `"final"` once a
///   `BatchFinalized` event has appended.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BatchView {
    /// Batch phase discriminator.
    pub phase: BatchPhase,
    /// Aggregate counts for the batch.
    pub summary: BatchViewSummary,
    /// Per-task entries.
    pub tasks: Vec<TaskView>,
    /// Name vectors (short task names). Populated only when
    /// `phase == Active`. Omitted from the wire shape when
    /// `phase == Final` so post-terminal `koto status` responses
    /// shrink to the documented terminal shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed: Option<Vec<String>>,
}

/// Batch phase discriminator rendered as `"active"` or `"final"`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BatchPhase {
    /// Parent currently sits on a `materialize_children` state.
    Active,
    /// Parent has transitioned past the batched state; a
    /// `BatchFinalized` event replays the frozen view.
    Final,
}

/// Aggregate counts for the batch.
///
/// `pending` is the count of tasks classified as `Pending` (unspawned
/// with no unmet deps, plus any spawned children still running — both
/// fold into one "in-progress" bucket per Decision 6). In the `active`
/// phase, it equals `ready.len()` because the two are derived from the
/// same classification pass; the distinction is that `pending`
/// survives into the `final` phase where the `ready` name vector is
/// intentionally dropped. Consumers that want "how many tasks are
/// still moving?" should read `pending` — it is the load-bearing count
/// across both phases.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BatchViewSummary {
    pub total: usize,
    pub success: usize,
    pub failed: usize,
    pub skipped: usize,
    pub pending: usize,
    pub blocked: usize,
    pub spawn_failed: usize,
}

/// Per-task entry.
///
/// `task_name` is the short submitted name (e.g., `"issue-1"`); `name`
/// is the full composed child workflow name (`"<parent>.<task_name>"`).
/// `template` is the child template path when known.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskView {
    /// Full composed child workflow name (`<parent>.<task_name>`).
    pub name: String,
    /// Short task name (submitted by the agent).
    pub task_name: String,
    /// Child template path when known (from the task entry or resolved
    /// at classification time). Omitted when not known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// Copy of the submitted `waits_on` list. Always present; an empty
    /// list means "no dependencies". Short names only.
    pub waits_on: Vec<String>,
    /// Typed per-task outcome: one of
    /// `success | failure | skipped | pending | blocked | spawn_failed`.
    pub outcome: TaskOutcome,
    /// Whether the child on disk currently sits in a
    /// `skipped_marker: true` state. `false` for all non-skipped
    /// outcomes and for skipped tasks that have no state file yet.
    #[serde(default, skip_serializing_if = "is_false")]
    pub synthetic: bool,
    /// Source of the `reason` projection. One of
    /// `failure_reason | state_name | skipped | not_spawned`. Omitted
    /// for successful or non-terminal children. Mirrors the
    /// `reason_source` field on the children-complete gate output — the
    /// two surfaces share the same canonical name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_source: Option<String>,
    /// Human-readable reason string for failures. For `Failure` this
    /// is the child's terminal state name (v1 fallback) or its
    /// `failure_reason` context key when present. Omitted for
    /// non-failed outcomes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Direct blocker for skipped tasks — the composed
    /// `<parent>.<ancestor>` name of the closest failed / skipped
    /// upstream task. Omitted for non-skipped outcomes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    /// Full upstream attribution chain for skipped tasks — every
    /// unique failed ancestor in closest-first order. Empty for
    /// non-skipped outcomes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_because_chain: Vec<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Produce a single [`BatchView`] for a batch-scoped parent.
///
/// Returns `None` when the parent is not batch-scoped — i.e., its
/// current state has no `materialize_children` hook AND no
/// `BatchFinalized` event has ever appended to its log. Callers that
/// render a per-parent envelope should omit their `batch` section
/// entirely when this returns `None`.
///
/// Detection model:
/// 1. If the parent's current state carries a `materialize_children`
///    hook, treat the view as `phase: "active"` and drive it off a
///    live classification pass via
///    [`build_children_complete_output`].
/// 2. Otherwise, if the parent log carries at least one
///    `BatchFinalized` event, replay the most recent one and label
///    the phase `"final"`. The frozen view preserves per-task
///    reasons and the aggregate counts that were live at finalization.
/// 3. Otherwise, return `None`.
pub fn derive_batch_view(
    backend: &dyn SessionBackend,
    parent_events: &[Event],
    parent_compiled: &CompiledTemplate,
    parent_current_state: &str,
    parent_name: &str,
) -> Option<BatchView> {
    // Active phase: current state carries a materialize_children hook.
    let has_hook = parent_compiled
        .states
        .get(parent_current_state)
        .and_then(|s| s.materialize_children.as_ref())
        .is_some();

    if has_hook {
        return Some(build_active_view(
            backend,
            parent_events,
            parent_compiled,
            parent_current_state,
            parent_name,
        ));
    }

    // Final phase: replay the most recent BatchFinalized event, if any.
    if let Some(evt) = find_most_recent_batch_finalized(parent_events) {
        if let EventPayload::BatchFinalized { view, .. } = &evt.payload {
            if let Some(final_view) = BatchFinalView::from_gate_output(view) {
                return Some(build_final_view(backend, parent_name, &final_view));
            }
        }
    }

    None
}

/// Build the live `BatchView` for a parent currently sitting on a
/// `materialize_children` state.
fn build_active_view(
    backend: &dyn SessionBackend,
    parent_events: &[Event],
    parent_compiled: &CompiledTemplate,
    parent_current_state: &str,
    parent_name: &str,
) -> BatchView {
    // Drive classification through the shared helper so the batch
    // section and children-complete gate output stay in lock step.
    let (_, gate_json) = build_children_complete_output(
        backend,
        parent_name,
        parent_events,
        parent_compiled,
        parent_current_state,
        None,
    );

    let obj = gate_json.as_object().cloned().unwrap_or_default();
    let summary = BatchViewSummary {
        total: obj.get("total").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
        success: obj.get("success").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
        failed: obj.get("failed").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
        skipped: obj.get("skipped").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
        pending: obj.get("pending").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
        blocked: obj.get("blocked").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
        spawn_failed: obj
            .get("spawn_failed")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
    };

    // Reconstruct the task list (short names + waits_on + template
    // override) from the latest EvidenceSubmitted for this state. The
    // gate output carries composed names; we want short task names
    // for the `ready/blocked/skipped/failed` vectors and for
    // `task_name` on the per-task rows.
    let hook = parent_compiled
        .states
        .get(parent_current_state)
        .and_then(|s| s.materialize_children.as_ref());
    let tasks: Vec<TaskEntry> = hook
        .and_then(|h| extract_tasks_public(parent_events, parent_current_state, &h.from_field))
        .unwrap_or_default();
    let task_by_name: std::collections::HashMap<String, &TaskEntry> =
        tasks.iter().map(|t| (t.name.clone(), t)).collect();

    let children = obj
        .get("children")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut task_views: Vec<TaskView> = Vec::with_capacity(children.len());
    let mut ready: Vec<String> = Vec::new();
    let mut blocked: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut failed: Vec<String> = Vec::new();

    let prefix = format!("{}.", parent_name);
    for child in children.iter() {
        let Some(child_obj) = child.as_object() else {
            continue;
        };
        let full_name = child_obj
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let short = full_name
            .strip_prefix(&prefix)
            .unwrap_or(&full_name)
            .to_string();
        let outcome: TaskOutcome = child_obj
            .get("outcome")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or(TaskOutcome::Pending);

        let (waits_on, template_override) = match task_by_name.get(short.as_str()) {
            Some(t) => (t.waits_on.clone(), t.template.clone()),
            None => (Vec::new(), None),
        };

        let reason_source = child_obj
            .get("reason_source")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let reason = match outcome {
            TaskOutcome::Failure => child_obj
                .get("failure_mode")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            _ => None,
        };
        let skip_reason = child_obj
            .get("skipped_because")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let skipped_because_chain: Vec<String> = child_obj
            .get("skipped_because_chain")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        // Synthetic marker: the child on disk sits in a terminal
        // `skipped_marker: true` state. Only meaningful when the task
        // outcome is Skipped.
        let synthetic = if matches!(outcome, TaskOutcome::Skipped) {
            is_child_synthetic(backend, &full_name)
        } else {
            false
        };

        // Aggregate the name vectors based on classification.
        match outcome {
            TaskOutcome::Pending => {
                // Pending (unspawned, no unmet deps) is the ready
                // frontier from the scheduler's perspective.
                ready.push(short.clone());
            }
            TaskOutcome::Blocked => blocked.push(short.clone()),
            TaskOutcome::Skipped => skipped.push(short.clone()),
            TaskOutcome::Failure | TaskOutcome::SpawnFailed => failed.push(short.clone()),
            _ => {}
        }

        task_views.push(TaskView {
            name: full_name,
            task_name: short,
            template: template_override,
            waits_on,
            outcome,
            synthetic,
            reason_source,
            reason,
            skip_reason,
            skipped_because_chain,
        });
    }

    // Phase is driven purely by BatchFinalized event existence: even
    // when the parent currently sits on the batched state, a fresh
    // BatchFinalized means we already finalized once. For the active
    // path we always label "active" — reaching this branch means the
    // current state still carries the hook.
    let phase = BatchPhase::Active;
    BatchView {
        phase,
        summary,
        tasks: task_views,
        ready: Some(ready),
        blocked: Some(blocked),
        skipped: Some(skipped),
        failed: Some(failed),
    }
}

/// Rebuild a `BatchView` from the frozen `BatchFinalView` payload.
///
/// The frozen view preserves per-task outcomes and aggregate counts
/// as of finalization time. Post-terminal consumers do NOT need the
/// `ready / blocked / skipped / failed` name vectors (every task is
/// terminal), so those fields are dropped to match the documented
/// terminal shape.
fn build_final_view(
    backend: &dyn SessionBackend,
    parent_name: &str,
    frozen: &BatchFinalView,
) -> BatchView {
    let summary = BatchViewSummary {
        total: frozen.total,
        success: frozen.success,
        failed: frozen.failed,
        skipped: frozen.skipped,
        pending: frozen.pending,
        blocked: frozen.blocked,
        spawn_failed: frozen.spawn_failed,
    };
    let prefix = format!("{}.", parent_name);
    let tasks: Vec<TaskView> = frozen
        .children
        .iter()
        .map(|entry| {
            let short = entry
                .name
                .strip_prefix(&prefix)
                .unwrap_or(&entry.name)
                .to_string();
            let synthetic = if matches!(entry.outcome, TaskOutcome::Skipped) {
                is_child_synthetic(backend, &entry.name)
            } else {
                false
            };
            let reason = match entry.outcome {
                TaskOutcome::Failure => entry.failure_mode.clone(),
                _ => None,
            };
            TaskView {
                name: entry.name.clone(),
                task_name: short,
                template: None,
                waits_on: Vec::new(),
                outcome: entry.outcome,
                synthetic,
                reason_source: entry.reason_source.clone(),
                reason,
                skip_reason: entry.skipped_because.clone(),
                skipped_because_chain: entry.skipped_because_chain.clone(),
            }
        })
        .collect();
    BatchView {
        phase: BatchPhase::Final,
        summary,
        tasks,
        ready: None,
        blocked: None,
        skipped: None,
        failed: None,
    }
}

/// Read a child's state file and check whether its current state has
/// `skipped_marker: true`. Returns `false` on any read/parse error.
fn is_child_synthetic(backend: &dyn SessionBackend, full_child_name: &str) -> bool {
    if !backend.exists(full_child_name) {
        return false;
    }
    let (_, events) = match backend.read_events(full_child_name) {
        Ok(x) => x,
        Err(_) => return false,
    };
    let current = derive_state_from_log(&events).unwrap_or_default();
    if current.is_empty() {
        return false;
    }
    match child_state_flags(&events, &current) {
        Some((_, _, skipped_marker)) => skipped_marker,
        None => false,
    }
}

/// Re-export of the private `extract_tasks` helper from `batch`. The
/// helper walks `events` for the latest `EvidenceSubmitted` on
/// `state` and decodes the named field as a task list. Kept local
/// here so `derive_batch_view` is self-contained.
fn extract_tasks_public(events: &[Event], state: &str, field: &str) -> Option<Vec<TaskEntry>> {
    for event in events.iter().rev() {
        if let EventPayload::EvidenceSubmitted {
            state: s, fields, ..
        } = &event.payload
        {
            if s != state {
                continue;
            }
            if let Some(raw) = fields.get(field) {
                return serde_json::from_value::<Vec<TaskEntry>>(raw.clone()).ok();
            }
        }
    }
    None
}

/// Render a `BatchView` as a `serde_json::Value` suitable for
/// splicing into the `batch` section of a `koto status` response.
///
/// The serde skip-directives on [`BatchView`] ensure that the
/// `ready / blocked / skipped / failed` name vectors disappear in
/// the terminal (`phase: "final"`) shape.
pub fn batch_view_to_json(view: &BatchView) -> serde_json::Value {
    serde_json::to_value(view).unwrap_or(serde_json::Value::Null)
}

/// Project the `batch.phase` label independently of the full view.
///
/// Useful for consumers that need to know the phase before they
/// decide whether to derive the full view.
pub fn phase_for(events: &[Event]) -> &'static str {
    derive_batch_phase(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::batch::{BatchFinalView, ChildGateEntry};

    fn empty_summary() -> BatchViewSummary {
        BatchViewSummary {
            total: 0,
            success: 0,
            failed: 0,
            skipped: 0,
            pending: 0,
            blocked: 0,
            spawn_failed: 0,
        }
    }

    #[test]
    fn batch_view_serializes_active_with_name_vectors() {
        let view = BatchView {
            phase: BatchPhase::Active,
            summary: empty_summary(),
            tasks: vec![],
            ready: Some(vec!["a".to_string()]),
            blocked: Some(vec!["b".to_string()]),
            skipped: Some(vec![]),
            failed: Some(vec![]),
        };
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["phase"], "active");
        assert_eq!(json["ready"][0], "a");
        assert_eq!(json["blocked"][0], "b");
        assert!(json["skipped"].is_array());
        assert!(json["failed"].is_array());
    }

    #[test]
    fn batch_view_serializes_final_without_name_vectors() {
        let view = BatchView {
            phase: BatchPhase::Final,
            summary: empty_summary(),
            tasks: vec![],
            ready: None,
            blocked: None,
            skipped: None,
            failed: None,
        };
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["phase"], "final");
        assert!(json.get("ready").is_none(), "ready must be absent in final");
        assert!(
            json.get("blocked").is_none(),
            "blocked must be absent in final"
        );
        assert!(
            json.get("skipped").is_none(),
            "skipped must be absent in final"
        );
        assert!(
            json.get("failed").is_none(),
            "failed must be absent in final"
        );
    }

    #[test]
    fn task_view_skips_optional_fields_by_default() {
        let task = TaskView {
            name: "p.a".to_string(),
            task_name: "a".to_string(),
            template: None,
            waits_on: vec![],
            outcome: TaskOutcome::Success,
            synthetic: false,
            reason_source: None,
            reason: None,
            skip_reason: None,
            skipped_because_chain: vec![],
        };
        let json = serde_json::to_value(&task).unwrap();
        assert_eq!(json["name"], "p.a");
        assert_eq!(json["task_name"], "a");
        assert_eq!(json["outcome"], "success");
        assert!(
            json.get("synthetic").is_none(),
            "synthetic false should be skipped"
        );
        assert!(json.get("reason_source").is_none());
        assert!(json.get("reason_code").is_none());
        assert!(json.get("reason").is_none());
        assert!(json.get("skip_reason").is_none());
        assert!(json.get("template").is_none());
        assert!(
            json.get("skipped_because_chain").is_none(),
            "empty chain should be skipped"
        );
    }

    #[test]
    fn task_view_emits_synthetic_when_true() {
        let task = TaskView {
            name: "p.a".to_string(),
            task_name: "a".to_string(),
            template: None,
            waits_on: vec![],
            outcome: TaskOutcome::Skipped,
            synthetic: true,
            reason_source: Some("skipped".to_string()),
            reason: None,
            skip_reason: Some("p.b".to_string()),
            skipped_because_chain: vec!["p.b".to_string()],
        };
        let json = serde_json::to_value(&task).unwrap();
        assert_eq!(json["synthetic"], true);
        assert_eq!(json["reason_source"], "skipped");
        assert_eq!(json["skip_reason"], "p.b");
        assert_eq!(json["skipped_because_chain"][0], "p.b");
    }

    #[test]
    fn build_final_view_projects_frozen_entries() {
        // Build a fake BatchFinalView with a mix of outcomes and
        // verify the TaskView projection. `backend` argument is
        // only used for the `synthetic` check, which the test
        // bypasses because none of the skipped entries have a live
        // child state file in the stub backend.
        let frozen = BatchFinalView {
            total: 3,
            completed: 3,
            pending: 0,
            success: 1,
            failed: 1,
            skipped: 1,
            blocked: 0,
            spawn_failed: 0,
            all_complete: true,
            all_success: false,
            any_failed: true,
            any_skipped: true,
            any_spawn_failed: false,
            needs_attention: true,
            children: vec![
                ChildGateEntry {
                    name: "p.A".to_string(),
                    state: "done".to_string(),
                    complete: true,
                    outcome: TaskOutcome::Success,
                    failure_mode: None,
                    skipped_because: None,
                    blocked_by: None,
                    skipped_because_chain: vec![],
                    reason_source: None,
                },
                ChildGateEntry {
                    name: "p.B".to_string(),
                    state: "failed".to_string(),
                    complete: true,
                    outcome: TaskOutcome::Failure,
                    failure_mode: Some("failed".to_string()),
                    skipped_because: None,
                    blocked_by: None,
                    skipped_because_chain: vec![],
                    reason_source: Some("state_name".to_string()),
                },
                ChildGateEntry {
                    name: "p.C".to_string(),
                    state: "skipped_via_upstream_failure".to_string(),
                    complete: true,
                    outcome: TaskOutcome::Skipped,
                    failure_mode: None,
                    skipped_because: Some("p.B".to_string()),
                    blocked_by: None,
                    skipped_because_chain: vec!["p.B".to_string()],
                    reason_source: Some("skipped".to_string()),
                },
            ],
        };
        // Use a fresh LocalBackend on a tempdir; `is_child_synthetic`
        // will see the children as non-existent and return `false`.
        let tmp = tempfile::tempdir().unwrap();
        let backend = crate::session::local::LocalBackend::with_base_dir(tmp.path().to_path_buf());
        let view = build_final_view(&backend, "p", &frozen);
        assert_eq!(view.phase, BatchPhase::Final);
        assert_eq!(view.summary.total, 3);
        assert_eq!(view.summary.success, 1);
        assert_eq!(view.summary.failed, 1);
        assert_eq!(view.summary.skipped, 1);
        assert_eq!(view.tasks.len(), 3);
        assert_eq!(view.tasks[0].task_name, "A");
        assert_eq!(view.tasks[1].task_name, "B");
        assert_eq!(view.tasks[1].reason_source.as_deref(), Some("state_name"));
        assert_eq!(view.tasks[1].reason.as_deref(), Some("failed"));
        assert_eq!(view.tasks[2].task_name, "C");
        assert_eq!(view.tasks[2].reason_source.as_deref(), Some("skipped"));
        assert_eq!(view.tasks[2].skip_reason.as_deref(), Some("p.B"));
        assert_eq!(view.tasks[2].skipped_because_chain, vec!["p.B".to_string()]);
        assert!(view.ready.is_none());
        assert!(view.blocked.is_none());
        assert!(view.skipped.is_none());
        assert!(view.failed.is_none());
    }
}
