//! Batch scheduler core.
//!
//! This module implements the runtime half of the batch-child-spawning
//! feature (Issue #12 in the batch-child-spawning plan). It is invoked
//! from [`crate::cli::handle_next`] immediately after
//! `advance_until_stop` returns. When the parent's final state carries
//! a `materialize_children` hook, the scheduler:
//!
//! 1. Extracts the submitted task list from the latest epoch's
//!    `EvidenceSubmitted` event.
//! 2. Builds an in-memory DAG (`build_dag`) with a topological order.
//! 3. Classifies each task by reading child state files directly from
//!    the session backend (`classify_task`) — no persistent cursor.
//! 4. Spawns ready tasks via
//!    [`crate::cli::init_child_from_parent`], collecting
//!    per-task failures into [`SchedulerOutcome::Scheduled::errored`].
//!
//! The classification is pure disk-state — running `run_batch_scheduler`
//! on a fully-spawned batch is a no-op (`spawned_this_tick` is empty).
//! A crash mid-spawn leaves the disk in a consistent state because
//! each `init_state_file` call is atomic.
//!
//! # Scope
//!
//! Issue #12 implements the happy-path loop only. Specifically out of
//! scope (landing in later issues):
//!
//! - Runtime reclassification of already-spawned children whose
//!   upstream outcomes flipped (Issue #13).
//! - `ready_to_drive` dispatch gating (Issue #13).
//! - `retry_failed` handling (Issue #14).
//! - `SchedulerRan` / `BatchFinalized` event emission (Issues #16/#17).
//! - `feedback`, orphan detection, and skip-marker synthesis (Issue
//!   #13/#20).
//!
//! The types here carry only the fields Issue #12 produces today. Later
//! issues extend the struct shapes (see design DESIGN-batch-child-
//! spawning.md Decision 12) without breaking callers.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::cli::batch_error::BatchError;
use crate::cli::init_child::TemplateCompileCache;
use crate::cli::task_spawn_error::TaskSpawnError;
use crate::engine::batch_validation::TaskEntry;
use crate::engine::persistence::derive_state_from_log;
use crate::engine::scheduler_warning::SchedulerWarning;
use crate::engine::types::{Event, EventPayload, SpawnEntrySnapshot};
use crate::session::SessionBackend;
use crate::template::types::{CompiledTemplate, FailurePolicy, MaterializeChildrenSpec};

// --------- Public types (wire-level) ---------------------------------

/// Top-level result of a scheduler tick.
///
/// `Scheduled` is the normal outcome and covers the "hook present,
/// tasks parseable" path. `NoBatch` signals that the parent's current
/// state has no `materialize_children` hook — the caller treats this
/// as a no-op. `Error` is reserved for tick-wide failures that prevent
/// any meaningful per-task reporting (e.g., backend list failure
/// during classification).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SchedulerOutcome {
    /// Parent state carries no `materialize_children` hook. Caller
    /// treats this as a no-op and does not attach a `scheduler` key
    /// to the response.
    NoBatch,

    /// Hook present; tasks parsed and classified. Every submitted
    /// task appears in `materialized_children` (either because a
    /// state file exists on disk, or because the task is still
    /// pending/blocked).
    Scheduled {
        /// Children whose state file was created during *this* tick.
        /// On resume after a crash, tasks whose state file was already
        /// present do NOT appear here — they surface under
        /// `materialized_children` with their current outcome.
        spawned_this_tick: Vec<String>,
        /// Ledger of every child known to this batch right now. Each
        /// entry is derived fresh from disk every tick; callers use
        /// this for idempotent dispatch rather than reading
        /// `spawned_this_tick` across ticks.
        materialized_children: Vec<MaterializedChild>,
        /// Per-task spawn errors accumulated during the tick. Issue
        /// #12 surfaces the common cases (compile failure, collision,
        /// I/O); siblings always keep spawning per Decision 11 Q4.
        errored: Vec<TaskSpawnError>,
        /// Non-fatal warnings the scheduler emitted this tick
        /// (Decision 14). Path-resolution warnings land here.
        warnings: Vec<SchedulerWarning>,
        /// True when at least one child's classification changed during
        /// this tick (stale skip marker respawned, running child
        /// respawned as skip marker, or pending task moved to Ready).
        /// Agents use this as a cheap signal that dispatch state may
        /// have shifted — `false` means the scheduler was a pure no-op.
        /// Issue #13 Round-3 polish.
        reclassified_this_tick: bool,
    },

    /// Tick-wide failure that prevents classification of any task
    /// (e.g., backend list failure). Agents retry; individual tasks
    /// stay untouched on disk.
    Error { reason: String },
}

/// Per-child ledger entry returned in `SchedulerOutcome::Scheduled`.
///
/// One entry is emitted for every submitted task entry — whether or
/// not a child state file exists on disk. When the child has been
/// spawned the `state` field carries its current state name; when the
/// task is still `pending` or `blocked` the `state` is `None`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MaterializedChild {
    /// Full composed workflow name (`<parent>.<task>`).
    pub name: String,
    /// Short task name as submitted by the agent (not the composed
    /// workflow name). Agents keying into their own task list read
    /// this field.
    pub task: String,
    /// Typed outcome discriminator.
    pub outcome: TaskOutcome,
    /// Current state of the child on disk, when spawned. `None` when
    /// the task has not yet been spawned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Copy of the submitted `waits_on` list for the convenience of
    /// agents rendering a batch view. Always present; an empty list
    /// means "no dependencies".
    pub waits_on: Vec<String>,
    /// Dispatch gate: `true` only when every `waits_on` entry is
    /// terminal (done / done_blocked / skipped) AND the child's own
    /// outcome is not `spawn_failed`. Workers filter
    /// `materialized_children` by `ready_to_drive: true AND
    /// outcome != spawn_failed` before picking up tasks (Decision 9;
    /// DESIGN-batch-child-spawning.md:1070-1086).
    ///
    /// Freshly-respawned children whose upstream deps have not yet
    /// settled remain `ready_to_drive: false` until the next tick's
    /// classification confirms their dependencies are terminal.
    pub ready_to_drive: bool,
}

/// Per-entry feedback discriminator (Decision 10 —
/// DESIGN-batch-child-spawning.md:1916). Reserved for the
/// `SchedulerOutcome::Scheduled.feedback` map that lands in a later
/// issue; Issue #13 defines the enum so the R8-vacuous `Respawning`
/// window has a named variant and so future wire-level feedback lands
/// on a stable shape.
///
/// Variants are keyed by the agent-submitted short task name. `Round-3`
/// polish (Issue #13) pins the documentation so each `Already*` variant
/// has an unambiguous meaning:
///
/// - [`AlreadyRunning`](Self::AlreadyRunning): child state file exists
///   on disk and its current state is **non-terminal**. The variant
///   says nothing about whether a worker is *actively driving* the
///   child right now — it is a disk-state assertion, not a liveness
///   probe.
/// - [`AlreadyTerminalSuccess`](Self::AlreadyTerminalSuccess): child
///   state file exists on disk, current state is terminal, and the
///   template does NOT flag `failure: true` or `skipped_marker: true`.
///   The normal "task completed successfully" outcome.
/// - [`AlreadyTerminalFailure`](Self::AlreadyTerminalFailure): child
///   state file exists on disk, current state is terminal, and the
///   template flags `failure: true`. Kept distinct from
///   `AlreadyTerminalSuccess` so agents don't need to peek at the
///   child's template flags to tell the two apart.
/// - [`AlreadySkipped`](Self::AlreadySkipped): child state file exists
///   on disk, current state is terminal, and the template flags
///   `skipped_marker: true`. A stale skip marker (upstream has since
///   flipped to success) is detected and respawned within the same
///   tick; by the time feedback is emitted the outcome has already
///   transitioned away from `AlreadySkipped`.
/// - [`Respawning`](Self::Respawning): the target child is mid-respawn
///   in **this** tick. R8 comparison is vacuous during this window
///   (the on-disk `spawn_entry` is transiently absent), so the
///   submission is accepted; the next tick re-evaluates against the
///   committed new `spawn_entry`. See
///   DESIGN-batch-child-spawning.md:1960-1972.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
#[allow(dead_code)]
pub enum EntryOutcome {
    /// Task entry accepted; the scheduler spawned (or already has
    /// spawned) a matching child.
    Accepted,
    /// Child exists on disk and is non-terminal. This is a pure
    /// disk-state assertion; it does NOT imply a worker is actively
    /// driving the child.
    AlreadyRunning,
    /// Child exists on disk at a terminal, non-failure, non-skip state.
    AlreadyTerminalSuccess,
    /// Child exists on disk at a terminal state whose template flags
    /// `failure: true`.
    AlreadyTerminalFailure,
    /// Child exists on disk at a terminal state whose template flags
    /// `skipped_marker: true`.
    AlreadySkipped,
    /// A dependency of this entry is non-terminal (or failed under
    /// `skip_dependents`). The entry is deferred; the same tick's
    /// `materialized_children` ledger reports `outcome: blocked`.
    Blocked { waits_on: Vec<String> },
    /// Spawn failure (compile error, collision, I/O). Mirrors
    /// `TaskSpawnError.kind` so agents can key off a single string.
    Errored { kind: String },
    /// Target child is mid-respawn **this tick**. R8 comparison is
    /// vacuous for this entry; the next tick re-evaluates against the
    /// new `spawn_entry`.
    Respawning,
}

/// Shared per-task outcome discriminator.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskOutcome {
    /// Child is in a terminal, non-failure, non-skip state.
    Success,
    /// Child is in a terminal state whose template declares
    /// `failure: true`.
    Failure,
    /// Child is in a terminal state whose template declares
    /// `skipped_marker: true`.
    Skipped,
    /// Task has no child state file and has no unmet `waits_on`
    /// dependencies. Scheduler will spawn this entry on the current
    /// or a near-future tick.
    Pending,
    /// Task has no child state file and at least one `waits_on`
    /// dependency is non-terminal.
    Blocked,
    /// Child exists on disk but is not terminal yet.
    ///
    /// `Running` IS part of the on-the-wire envelope: it is serialized
    /// as `"running"` in the `materialized_children[*].outcome` field
    /// (see `tests/batch_scheduler_test.rs` line 321). Downstream
    /// agents rendering a batch view can distinguish "still in progress"
    /// from "not yet spawned" (`Pending`/`Blocked`), so the variant is
    /// intentionally kept distinct rather than collapsed into `Pending`.
    Running,
    /// A previous tick's `init_state_file` failed and no child state
    /// file exists. Caller treats this as a terminal per-task error
    /// and leaves a matching entry in `errored`.
    SpawnFailed,
}

// --------- Internal types -------------------------------------------

/// In-memory DAG representation produced by [`build_dag`]. The
/// adjacency map records predecessor edges (`name -> list of tasks
/// that the named task waits on`) and the sorted topological order is
/// the order the scheduler iterates tasks in.
///
/// Pre-condition: the supplied task list must already have passed
/// [`crate::engine::batch_validation::validate_batch_submission`]
/// (in particular R3 "no cycles" and R4 "dangling refs"). Issue #12
/// is scoped to the happy path — callers hit `BatchError::Invalid-
/// BatchDefinition` before reaching this module if either rule
/// trips, so `build_dag` itself does not re-validate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchDag<'a> {
    /// Adjacency: predecessor edges. `predecessors[name]` is the
    /// list of tasks `name` must wait for. Ordered as submitted so
    /// error messages stay deterministic.
    pub predecessors: HashMap<&'a str, Vec<&'a str>>,
    /// Topological order — a task appears after all of its
    /// predecessors. Stable under the input order so deterministic
    /// agents observe a deterministic sequence.
    pub topological_order: Vec<&'a str>,
}

/// Build the batch DAG.
///
/// Implements Kahn's algorithm with ties broken by the input order so
/// the topological sequence is deterministic — the order the agent
/// submitted tasks in is preserved at each "ready" frontier. Accepts
/// the `tasks` slice by reference to avoid copying strings.
///
/// # Panics
///
/// Never. Invalid DAGs (cycles, dangling refs) must be rejected
/// upstream; if they somehow reach this function, remaining entries
/// are silently dropped from the topological order. Callers that
/// care call validation first.
pub(crate) fn build_dag<'a>(tasks: &'a [TaskEntry]) -> BatchDag<'a> {
    let mut predecessors: HashMap<&str, Vec<&str>> = HashMap::with_capacity(tasks.len());
    let mut in_degree: HashMap<&str, usize> = HashMap::with_capacity(tasks.len());
    let name_set: HashMap<&str, usize> = tasks
        .iter()
        .enumerate()
        .map(|(i, t)| (t.name.as_str(), i))
        .collect();

    for task in tasks {
        // Only count dependencies that resolve to submitted tasks.
        // R4 guarantees this is all of them when called on a
        // validated batch; the filter keeps `build_dag` safe under
        // a malformed input (invalid batches still produce a sane
        // topological order for the valid subset).
        let deps: Vec<&str> = task
            .waits_on
            .iter()
            .filter_map(|d| name_set.get(d.as_str()).map(|_| d.as_str()))
            .collect();
        in_degree.insert(task.name.as_str(), deps.len());
        predecessors.insert(task.name.as_str(), deps);
    }

    // Kahn's algorithm. Seed the queue with nodes of in-degree 0, in
    // submission order.
    let mut queue: Vec<&str> = tasks
        .iter()
        .filter(|t| in_degree.get(t.name.as_str()).copied().unwrap_or(0) == 0)
        .map(|t| t.name.as_str())
        .collect();

    let mut topological_order: Vec<&str> = Vec::with_capacity(tasks.len());

    // Successor map derived from predecessor map.
    let mut successors: HashMap<&str, Vec<&str>> = HashMap::with_capacity(tasks.len());
    for task in tasks {
        for dep in predecessors.get(task.name.as_str()).unwrap_or(&Vec::new()) {
            successors.entry(*dep).or_default().push(task.name.as_str());
        }
    }

    while !queue.is_empty() {
        // Drain the current frontier in submission order so the
        // output is stable. We iterate `queue` and rebuild it as the
        // "next" frontier.
        let frontier = std::mem::take(&mut queue);
        let mut next_frontier: Vec<&str> = Vec::new();
        for name in &frontier {
            topological_order.push(*name);
            if let Some(succs) = successors.get(*name) {
                for succ in succs {
                    let d = in_degree.entry(*succ).or_insert(0);
                    if *d > 0 {
                        *d -= 1;
                        if *d == 0 {
                            next_frontier.push(*succ);
                        }
                    }
                }
            }
        }
        // Maintain submission order within the next frontier so two
        // ready siblings surface in the order the agent submitted
        // them.
        next_frontier.sort_by_key(|n| name_set.get(*n).copied().unwrap_or(usize::MAX));
        queue = next_frontier;
    }

    BatchDag {
        predecessors,
        topological_order,
    }
}

/// Classification of a single task, derived strictly from disk state
/// and the known outcomes of the task's dependencies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskClassification {
    /// Task has no child on disk, no dependencies or all dependencies
    /// have succeeded. Ready to spawn.
    Ready,
    /// Task has no child on disk and at least one dependency is
    /// non-terminal. Not ready this tick.
    BlockedByDep,
    /// Child exists on disk and is non-terminal.
    Running,
    /// Child exists on disk and is terminal-success.
    Success,
    /// Child exists on disk and is terminal-failure (template flagged
    /// `failure: true`).
    Failure,
    /// Child exists on disk and is terminal-skipped (template flagged
    /// `skipped_marker: true`).
    Skipped,
    /// A dependency of this task is in `Failure` (or transitively in
    /// `ShouldBeSkipped`) and the batch's `failure_policy` is
    /// `SkipDependents`. Issue #13 spawns a terminal skip marker for
    /// these tasks so the `children-complete` gate can tally them.
    ShouldBeSkipped,
}

impl TaskClassification {
    fn to_outcome(&self) -> TaskOutcome {
        match self {
            TaskClassification::Ready => TaskOutcome::Pending,
            TaskClassification::BlockedByDep => TaskOutcome::Blocked,
            // `ShouldBeSkipped` in the `materialized_children` ledger
            // means the task has been classified as needing a skip
            // marker but has not yet been spawned this tick. The
            // scheduler's outer loop spawns it and rewrites the
            // ledger entry to `Skipped` via the fresh read-back; this
            // arm is only hit when the spawn itself failed, in which
            // case the `errored` vector carries the detail and the
            // outcome is overridden to `SpawnFailed` upstream.
            TaskClassification::ShouldBeSkipped => TaskOutcome::Skipped,
            TaskClassification::Running => TaskOutcome::Running,
            TaskClassification::Success => TaskOutcome::Success,
            TaskClassification::Failure => TaskOutcome::Failure,
            TaskClassification::Skipped => TaskOutcome::Skipped,
        }
    }
}

/// Snapshot of a child state file that `classify_task` needs to
/// determine the child's `TaskOutcome`. Built once per tick by the
/// scheduler and looked up by short task name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChildSnapshot {
    /// Current state name as derived from the event log.
    pub current_state: String,
    /// Whether the current state is terminal, per the child's own
    /// compiled template.
    pub terminal: bool,
    /// Whether the current state has `failure: true`.
    pub failure: bool,
    /// Whether the current state has `skipped_marker: true`.
    pub skipped_marker: bool,
    /// `spawn_entry` recorded on the child's `WorkflowInitialized`
    /// event, when present. Issue #12 does not yet consume this
    /// (no R8 runtime check); later issues use it for rename
    /// detection and respawn-entry comparison.
    #[allow(dead_code)]
    pub spawn_entry: Option<SpawnEntrySnapshot>,
}

/// Classify a single task.
///
/// Classification rules (Issue #12 subset):
///
/// - Child state file present and terminal:
///   - `failure: true` → [`TaskClassification::Failure`]
///   - `skipped_marker: true` → [`TaskClassification::Skipped`]
///   - otherwise → [`TaskClassification::Success`]
/// - Child state file present and non-terminal → [`TaskClassification::Running`]
/// - Child state file absent:
///   - every `waits_on` dependency is `Success` (or empty) →
///     [`TaskClassification::Ready`]
///   - at least one dependency is not-yet-`Success` →
///     [`TaskClassification::BlockedByDep`]
///
/// `ShouldBeSkipped` (failed-upstream under `skip_dependents`) is
/// checked by the scheduler's outer loop once it has classified every
/// task in topological order; this function does not by itself produce
/// that classification.
pub(crate) fn classify_task(
    task: &TaskEntry,
    existing: Option<&ChildSnapshot>,
    classifications: &HashMap<String, TaskClassification>,
    failure_policy: FailurePolicy,
) -> TaskClassification {
    if let Some(snap) = existing {
        if snap.terminal {
            if snap.failure {
                return TaskClassification::Failure;
            }
            if snap.skipped_marker {
                return TaskClassification::Skipped;
            }
            return TaskClassification::Success;
        }
        return TaskClassification::Running;
    }

    // Not yet spawned — check dependencies against the prior sweep's
    // classifications. Under `SkipDependents`, a failed or
    // transitively-skipped upstream yields `ShouldBeSkipped`; the
    // scheduler spawns a skip-marker child in that case. Under
    // `Continue`, dependents still block on non-success but never
    // inherit a failure.
    let mut should_skip = false;
    for dep in &task.waits_on {
        match classifications.get(dep.as_str()) {
            Some(TaskClassification::Success) => {}
            Some(TaskClassification::Failure) | Some(TaskClassification::Skipped)
                if matches!(failure_policy, FailurePolicy::SkipDependents) =>
            {
                should_skip = true;
            }
            Some(TaskClassification::ShouldBeSkipped)
                if matches!(failure_policy, FailurePolicy::SkipDependents) =>
            {
                should_skip = true;
            }
            _ => return TaskClassification::BlockedByDep,
        }
    }
    if should_skip {
        TaskClassification::ShouldBeSkipped
    } else {
        TaskClassification::Ready
    }
}

// --------- Scheduler entry point ------------------------------------

/// Run one scheduler tick.
///
/// # Arguments
///
/// - `backend`: the session backend used to read child state files
///   and to spawn new ones via
///   [`crate::cli::init_child_from_parent`].
/// - `template`: the parent's compiled template.
/// - `current_state`: the parent's state at the moment
///   `advance_until_stop` returned. The scheduler only runs when this
///   state carries a `materialize_children` hook.
/// - `parent_name`: the parent workflow's identifier. Children are
///   named `<parent>.<task>`.
/// - `events`: the parent's event log.
///
/// # Behavior
///
/// Reads the `materialize_children` hook on the current state. If
/// absent, returns [`SchedulerOutcome::NoBatch`]. Otherwise, extracts
/// the task list from the latest epoch's `EvidenceSubmitted` event,
/// builds the DAG, classifies every task, and spawns children that
/// are in [`TaskClassification::Ready`].
#[allow(clippy::result_large_err)]
pub(crate) fn run_batch_scheduler(
    backend: &dyn SessionBackend,
    template: &CompiledTemplate,
    current_state: &str,
    parent_name: &str,
    events: &[Event],
) -> Result<SchedulerOutcome, BatchError> {
    // Look up the hook. No hook -> no batch.
    let hook: &MaterializeChildrenSpec = match template
        .states
        .get(current_state)
        .and_then(|s| s.materialize_children.as_ref())
    {
        Some(h) => h,
        None => return Ok(SchedulerOutcome::NoBatch),
    };

    // Pull the task list from the latest EvidenceSubmitted event for
    // this state. We walk backward because the latest write wins.
    let field = hook.from_field.as_str();
    let tasks: Vec<TaskEntry> = match extract_tasks(events, current_state, field) {
        Some(list) => list,
        // No evidence yet (or evidence without the task field) — the
        // scheduler has nothing to do. Agents see this as an empty
        // batch view.
        None => {
            return Ok(SchedulerOutcome::Scheduled {
                spawned_this_tick: Vec::new(),
                materialized_children: Vec::new(),
                errored: Vec::new(),
                warnings: Vec::new(),
                reclassified_this_tick: false,
            });
        }
    };

    // Build the DAG once per tick. Validated upstream, so build_dag
    // is safe to call unconditionally here.
    let dag = build_dag(&tasks);
    let name_to_task: HashMap<&str, &TaskEntry> =
        tasks.iter().map(|t| (t.name.as_str(), t)).collect();

    // Snapshot all existing children on disk. One backend.list() +
    // one read_events per child is the read-side cost per tick.
    let mut snapshots = match snapshot_existing_children(backend, parent_name, &name_to_task) {
        Ok(s) => s,
        Err(reason) => return Ok(SchedulerOutcome::Error { reason }),
    };

    let failure_policy = hook.failure_policy;

    // Classify every task in topological order so dependency
    // outcomes are known when we classify a downstream task. This is
    // the "current" classification — based on what is on disk right
    // now. Reclassification below compares against the "ideal"
    // classification (what the task SHOULD be given the current
    // upstream outcomes, regardless of what's on disk).
    let mut classifications: HashMap<String, TaskClassification> =
        HashMap::with_capacity(tasks.len());
    for name in &dag.topological_order {
        let task = match name_to_task.get(*name) {
            Some(t) => *t,
            None => continue,
        };
        let c = classify_task(task, snapshots.get(*name), &classifications, failure_policy);
        classifications.insert(task.name.clone(), c);
    }

    // Spawn ready tasks and reclassify-and-respawn mismatched
    // children. Per-child accumulation: failures land in `errored`
    // but never halt the tick — subsequent tasks still spawn.
    let mut spawned_this_tick: Vec<String> = Vec::new();
    let mut errored: Vec<TaskSpawnError> = Vec::new();
    let mut warnings: Vec<SchedulerWarning> = Vec::new();
    let mut cache = TemplateCompileCache::new();
    let mut reclassified_this_tick = false;

    // Resolve the parent's template source dir + submitter cwd once.
    let (template_source_dir, submitter_cwd) = resolution_context(backend, parent_name, events);

    // Iterate tasks in topological order so a respawn that updates a
    // downstream's upstream outcome propagates within the same tick.
    for name in dag.topological_order.clone() {
        let task = match name_to_task.get(name) {
            Some(t) => *t,
            None => continue,
        };

        let current_class = classifications
            .get(task.name.as_str())
            .cloned()
            .unwrap_or(TaskClassification::BlockedByDep);

        // Derive the "ideal" classification from current upstream
        // outcomes: what WOULD we classify this task as if no child
        // existed on disk? Compare against the actual on-disk state
        // to detect stale skip markers (upstream recovered) and
        // running children whose upstream flipped to failure.
        let ideal_class = classify_task(task, None, &classifications, failure_policy);

        // Decide whether this child needs to be respawned.
        // - Skipped on disk but upstream now suggests Ready → stale
        //   skip marker, respawn as real child.
        // - Running on disk but upstream now suggests ShouldBeSkipped
        //   → respawn as skip marker.
        // - No other terminal-on-disk triggers reclassification:
        //   terminal Success / Failure stay put.
        let respawn_as = match (&current_class, &ideal_class) {
            (TaskClassification::Skipped, TaskClassification::Ready) => Some(RespawnTarget::Real),
            (TaskClassification::Running, TaskClassification::ShouldBeSkipped) => {
                Some(RespawnTarget::SkipMarker)
            }
            _ => None,
        };

        if let Some(target) = respawn_as {
            // Delete the existing child state file and respawn with
            // the current submission entry. The transient window
            // between delete and re-init is the R8-vacuous
            // `EntryOutcome::Respawning` window — any concurrent
            // submission for the same task name observes `Some(None)`
            // in `existing_children_snapshot` and defers R8.
            let child_name = format!("{}.{}", parent_name, task.name);
            if let Err(e) = backend.cleanup(&child_name) {
                errored.push(TaskSpawnError::new(
                    &child_name,
                    crate::cli::task_spawn_error::SpawnErrorKind::IoError,
                    format!(
                        "failed to delete stale child during reclassification: {}",
                        e
                    ),
                ));
                // Record the spawn_failed outcome and continue with
                // the next task. The next tick will retry.
                classifications.insert(task.name.clone(), TaskClassification::Failure);
                continue;
            }
            // Drop the stale on-disk snapshot so downstream
            // ready_to_drive computation uses the post-respawn view.
            snapshots.remove(task.name.as_str());
            reclassified_this_tick = true;

            // Fall through: the classification is updated to the
            // ideal so the spawn path below treats it like a fresh
            // task.
            classifications.insert(
                task.name.clone(),
                match target {
                    RespawnTarget::Real => TaskClassification::Ready,
                    RespawnTarget::SkipMarker => TaskClassification::ShouldBeSkipped,
                },
            );
        }

        // Re-read the classification after possible reclassification
        // override.
        let effective = classifications
            .get(task.name.as_str())
            .cloned()
            .unwrap_or(TaskClassification::BlockedByDep);

        match effective {
            TaskClassification::Ready => {
                spawn_ready_task(
                    backend,
                    parent_name,
                    task,
                    hook,
                    template_source_dir.as_deref(),
                    submitter_cwd.as_deref(),
                    &mut cache,
                    &mut spawned_this_tick,
                    &mut errored,
                    &mut warnings,
                    &mut classifications,
                );
            }
            TaskClassification::ShouldBeSkipped => {
                spawn_skip_marker_task(
                    backend,
                    parent_name,
                    task,
                    hook,
                    template_source_dir.as_deref(),
                    submitter_cwd.as_deref(),
                    &mut cache,
                    &mut spawned_this_tick,
                    &mut errored,
                    &mut warnings,
                    &mut classifications,
                );
            }
            _ => {}
        }
    }

    // Build materialized_children ledger covering every submitted
    // task, regardless of whether it has a child file on disk.
    let mut materialized_children: Vec<MaterializedChild> = Vec::with_capacity(tasks.len());
    for task in &tasks {
        let name = format!("{}.{}", parent_name, task.name);
        let class = classifications
            .get(task.name.as_str())
            .cloned()
            .unwrap_or(TaskClassification::BlockedByDep);
        let task_errored = errored.iter().any(|e| e.task == name);
        let outcome = if task_errored {
            TaskOutcome::SpawnFailed
        } else {
            class.to_outcome()
        };
        let state = if spawned_this_tick.iter().any(|n| n == &name) {
            // A freshly-spawned child's state is its initial state;
            // read it back for honesty.
            backend
                .read_events(&name)
                .ok()
                .and_then(|(_, evts)| derive_state_from_log(&evts))
        } else {
            snapshots
                .get(task.name.as_str())
                .map(|s| s.current_state.clone())
        };

        // `ready_to_drive` is the dispatch gate. True iff:
        //   - outcome is not `spawn_failed`, AND
        //   - every `waits_on` entry resolves to a terminal
        //     classification (success / failure / skipped).
        // A child that has not yet been spawned (outcome Pending /
        // Blocked) is never `ready_to_drive: true` even when its
        // deps are terminal — the dispatch gate is for already-on-
        // disk non-terminal children, and a Pending task has no
        // child file for a worker to drive.
        let all_deps_terminal = task.waits_on.iter().all(|dep| {
            matches!(
                classifications.get(dep.as_str()),
                Some(
                    TaskClassification::Success
                        | TaskClassification::Failure
                        | TaskClassification::Skipped
                )
            )
        });
        let ready_to_drive =
            !task_errored && matches!(outcome, TaskOutcome::Running) && all_deps_terminal;

        materialized_children.push(MaterializedChild {
            name,
            task: task.name.clone(),
            outcome,
            state,
            waits_on: task.waits_on.clone(),
            ready_to_drive,
        });
    }

    // `reclassified_this_tick` also flips true whenever any task
    // transitioned from not-yet-spawned to spawned in this tick: the
    // classification map was mutated, which is what the flag signals
    // to agents.
    if !spawned_this_tick.is_empty() {
        reclassified_this_tick = true;
    }

    Ok(SchedulerOutcome::Scheduled {
        spawned_this_tick,
        materialized_children,
        errored,
        warnings,
        reclassified_this_tick,
    })
}

/// Which shape to respawn a reclassified child into.
#[derive(Debug, Clone, Copy)]
enum RespawnTarget {
    /// Spawn the child with its real template at the normal initial
    /// state. Used when a stale skip marker's upstream has since
    /// flipped back to success.
    Real,
    /// Spawn the child directly into its `skipped_marker: true`
    /// terminal. Used when a running child's upstream flipped to
    /// failure.
    SkipMarker,
}

/// Walk `backend.list()` and return a map from short task name to
/// [`ChildSnapshot`] for every child of `parent_name` that matches a
/// submitted task.
fn snapshot_existing_children(
    backend: &dyn SessionBackend,
    parent_name: &str,
    name_to_task: &HashMap<&str, &TaskEntry>,
) -> Result<HashMap<String, ChildSnapshot>, String> {
    let sessions = backend
        .list()
        .map_err(|e| format!("failed to list sessions: {}", e))?;
    let child_prefix = format!("{}.", parent_name);
    let mut snapshots: HashMap<String, ChildSnapshot> = HashMap::new();
    for info in sessions {
        if info.parent_workflow.as_deref() != Some(parent_name) {
            continue;
        }
        if !info.id.starts_with(&child_prefix) {
            continue;
        }
        let task_name = info.id[child_prefix.len()..].to_string();
        if !name_to_task.contains_key(task_name.as_str()) {
            continue;
        }
        let (_, child_events) = match backend.read_events(&info.id) {
            Ok(x) => x,
            Err(_) => continue,
        };
        let current = match derive_state_from_log(&child_events) {
            Some(s) => s,
            None => continue,
        };
        let (terminal, failure, skipped_marker) =
            child_state_flags(&child_events, &current).unwrap_or((false, false, false));
        let spawn_entry = child_events.iter().find_map(|e| match &e.payload {
            EventPayload::WorkflowInitialized { spawn_entry, .. } => spawn_entry.clone(),
            _ => None,
        });
        snapshots.insert(
            task_name,
            ChildSnapshot {
                current_state: current,
                terminal,
                failure,
                skipped_marker,
                spawn_entry,
            },
        );
    }
    Ok(snapshots)
}

/// Spawn a freshly-classified `Ready` task. Updates the classification
/// map to `Running` on success or `Failure` on error so downstream
/// ready/blocked decisions are consistent within the same tick.
#[allow(clippy::too_many_arguments)]
fn spawn_ready_task(
    backend: &dyn SessionBackend,
    parent_name: &str,
    task: &TaskEntry,
    hook: &MaterializeChildrenSpec,
    template_source_dir: Option<&std::path::Path>,
    submitter_cwd: Option<&std::path::Path>,
    cache: &mut TemplateCompileCache,
    spawned_this_tick: &mut Vec<String>,
    errored: &mut Vec<TaskSpawnError>,
    warnings: &mut Vec<SchedulerWarning>,
    classifications: &mut HashMap<String, TaskClassification>,
) {
    let raw_template = task
        .template
        .clone()
        .unwrap_or_else(|| hook.default_template.clone());
    let resolution = crate::engine::path_resolution::resolve_template_path(
        &raw_template,
        template_source_dir,
        submitter_cwd,
    );
    accumulate_resolution_warnings(&resolution.warnings, warnings);

    let child_name = format!("{}.{}", parent_name, task.name);
    let vars = vars_to_cli_args(&task.vars);
    let snapshot = build_spawn_entry_snapshot(task, &raw_template);

    match crate::cli::init_child_from_parent(
        backend,
        Some(parent_name),
        &child_name,
        &resolution.resolved,
        &vars,
        cache,
        Some(snapshot),
    ) {
        Ok(()) => {
            spawned_this_tick.push(child_name);
            classifications.insert(task.name.clone(), TaskClassification::Running);
        }
        Err(err) => {
            errored.push(err);
            classifications.insert(task.name.clone(), TaskClassification::Failure);
        }
    }
}

/// Spawn a `ShouldBeSkipped` task directly into its terminal
/// `skipped_marker: true` state. Requires compiling the child template
/// to find the skipped-marker state name — that happens via
/// [`find_skipped_state_name`].
#[allow(clippy::too_many_arguments)]
fn spawn_skip_marker_task(
    backend: &dyn SessionBackend,
    parent_name: &str,
    task: &TaskEntry,
    hook: &MaterializeChildrenSpec,
    template_source_dir: Option<&std::path::Path>,
    submitter_cwd: Option<&std::path::Path>,
    cache: &mut TemplateCompileCache,
    spawned_this_tick: &mut Vec<String>,
    errored: &mut Vec<TaskSpawnError>,
    warnings: &mut Vec<SchedulerWarning>,
    classifications: &mut HashMap<String, TaskClassification>,
) {
    let raw_template = task
        .template
        .clone()
        .unwrap_or_else(|| hook.default_template.clone());
    let resolution = crate::engine::path_resolution::resolve_template_path(
        &raw_template,
        template_source_dir,
        submitter_cwd,
    );
    accumulate_resolution_warnings(&resolution.warnings, warnings);

    let child_name = format!("{}.{}", parent_name, task.name);
    let skipped_state = match find_skipped_state_name(&resolution.resolved) {
        Ok(name) => name,
        Err(msg) => {
            errored.push(TaskSpawnError::new(
                &child_name,
                crate::cli::task_spawn_error::SpawnErrorKind::TemplateCompileFailed,
                msg,
            ));
            classifications.insert(task.name.clone(), TaskClassification::Failure);
            return;
        }
    };

    let vars = vars_to_cli_args(&task.vars);
    let snapshot = build_spawn_entry_snapshot(task, &raw_template);

    match crate::cli::init_child::init_child_as_skip_marker_from_parent(
        backend,
        Some(parent_name),
        &child_name,
        &resolution.resolved,
        &vars,
        cache,
        Some(snapshot),
        &skipped_state,
    ) {
        Ok(()) => {
            spawned_this_tick.push(child_name);
            classifications.insert(task.name.clone(), TaskClassification::Skipped);
        }
        Err(err) => {
            errored.push(err);
            classifications.insert(task.name.clone(), TaskClassification::Failure);
        }
    }
}

/// Compile the child template at `template_path` (via the on-disk
/// cache) and return the name of a state declaring `skipped_marker:
/// true`. F5 guarantees at least one such state exists on any
/// batch-eligible child template; if none is found, return an error
/// message the caller surfaces as `TaskSpawnError`.
fn find_skipped_state_name(template_path: &std::path::Path) -> Result<String, String> {
    let canonical = std::fs::canonicalize(template_path).map_err(|e| {
        format!(
            "failed to resolve template {}: {}",
            template_path.display(),
            e
        )
    })?;
    let (cache_path, _) = crate::cache::compile_cached(&canonical, false)
        .map_err(|e| format!("failed to compile template {}: {}", canonical.display(), e))?;
    let bytes = std::fs::read(&cache_path).map_err(|e| {
        format!(
            "failed to read compiled template {}: {}",
            cache_path.display(),
            e
        )
    })?;
    let compiled: CompiledTemplate = serde_json::from_slice(&bytes).map_err(|e| {
        format!(
            "failed to parse compiled template {}: {}",
            cache_path.display(),
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
        canonical.display()
    ))
}

/// Build the `spawn_entry` snapshot recorded on a child's
/// `WorkflowInitialized` event. The stored template string is the raw
/// submitted path (or inherited default), not the resolved absolute
/// path, matching Decision 10's canonical-form rule.
fn build_spawn_entry_snapshot(task: &TaskEntry, raw_template: &str) -> SpawnEntrySnapshot {
    let mut vars_map: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for (k, v) in &task.vars {
        vars_map.insert(k.clone(), v.clone());
    }
    SpawnEntrySnapshot::new(raw_template.to_string(), vars_map, task.waits_on.clone())
}

/// Dedup `MissingTemplateSourceDir` (per-tick warning, not per-task)
/// and append the remaining warnings verbatim.
fn accumulate_resolution_warnings(
    resolution_warnings: &[SchedulerWarning],
    warnings: &mut Vec<SchedulerWarning>,
) {
    for w in resolution_warnings {
        if matches!(w, SchedulerWarning::MissingTemplateSourceDir)
            && warnings
                .iter()
                .any(|existing| matches!(existing, SchedulerWarning::MissingTemplateSourceDir))
        {
            continue;
        }
        warnings.push(w.clone());
    }
}

// --------- Helpers ---------------------------------------------------

/// Parse the task list from the latest `EvidenceSubmitted` event for
/// `state` whose fields contain `field`. Returns `None` when no such
/// event is found.
fn extract_tasks(events: &[Event], state: &str, field: &str) -> Option<Vec<TaskEntry>> {
    for event in events.iter().rev() {
        if let EventPayload::EvidenceSubmitted {
            state: s, fields, ..
        } = &event.payload
        {
            if s != state {
                continue;
            }
            if let Some(raw) = fields.get(field) {
                // The engine validates the field at accept time, but
                // the runtime shape is `Vec<TaskEntry>` — deserialize
                // from the JSON value.
                return serde_json::from_value::<Vec<TaskEntry>>(raw.clone()).ok();
            }
        }
    }
    None
}

/// Read back the terminal / failure / skipped_marker flags for a
/// child's current state. The child's compiled template lives under
/// the path recorded on its `WorkflowInitialized` event.
fn child_state_flags(events: &[Event], current_state: &str) -> Option<(bool, bool, bool)> {
    let template_path = events.iter().find_map(|e| match &e.payload {
        EventPayload::WorkflowInitialized { template_path, .. } => Some(template_path.clone()),
        _ => None,
    })?;
    let bytes = std::fs::read(&template_path).ok()?;
    let compiled: CompiledTemplate = serde_json::from_slice(&bytes).ok()?;
    let state = compiled.states.get(current_state)?;
    Some((state.terminal, state.failure, state.skipped_marker))
}

/// Derive the `template_source_dir` and latest `submitter_cwd` for
/// the parent. Returns `(base, cwd)`; either can be `None`. The base
/// directory comes from the parent's state-file header; the cwd
/// comes from the latest `EvidenceSubmitted` event.
fn resolution_context(
    backend: &dyn SessionBackend,
    parent_name: &str,
    events: &[Event],
) -> (Option<PathBuf>, Option<PathBuf>) {
    let base = backend
        .read_events(parent_name)
        .ok()
        .and_then(|(header, _)| header.template_source_dir.clone());
    let cwd = events.iter().rev().find_map(|e| match &e.payload {
        EventPayload::EvidenceSubmitted { submitter_cwd, .. } => submitter_cwd.clone(),
        _ => None,
    });
    (base, cwd)
}

/// Convert a task entry's `vars` map into the `KEY=VALUE` CLI-arg
/// form expected by [`crate::cli::init_child_from_parent`]. Values
/// are stringified via `to_string` so objects/arrays serialize to
/// their JSON representation.
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

/// Public helper: does this parent state trigger the batch scheduler?
/// Wired into `state_is_batch_scoped` so the advisory flock is taken
/// for states that materialize children.
pub(crate) fn state_has_materialize_children(
    compiled: &CompiledTemplate,
    state_name: &str,
) -> bool {
    compiled
        .states
        .get(state_name)
        .and_then(|s| s.materialize_children.as_ref())
        .is_some()
}

/// Build the `existing_children_snapshot` map for
/// [`crate::engine::batch_validation::validate_batch_submission`].
///
/// The returned map keys short task names (what the agent submitted, not
/// the composed `<parent>.<task>` workflow name) to the `spawn_entry`
/// snapshot recorded on the child's `WorkflowInitialized` event when the
/// child exists on disk:
///
/// - `Some(Some(snapshot))` — child exists and carries a canonical-form
///   `spawn_entry`; R8 compares the submission against it.
/// - `Some(None)` — child exists but has no `spawn_entry` (pre-feature
///   or mid-respawn window). R8 treats this as vacuously satisfied.
/// - key absent — child has never been spawned; R8 does not apply.
///
/// Note: `backend.list()` is `O(total sessions on backend)`, not
/// `O(children)`. Under `CloudBackend` this becomes a cross-host
/// metadata listing. Acceptable for v1; revisit when batch scale tests
/// land.
pub(crate) fn build_existing_children_snapshot(
    backend: &dyn SessionBackend,
    parent_name: &str,
) -> HashMap<String, Option<SpawnEntrySnapshot>> {
    let mut snapshot: HashMap<String, Option<SpawnEntrySnapshot>> = HashMap::new();
    let sessions = match backend.list() {
        Ok(s) => s,
        Err(_) => return snapshot,
    };
    let child_prefix = format!("{}.", parent_name);
    for info in sessions {
        if info.parent_workflow.as_deref() != Some(parent_name) {
            continue;
        }
        if !info.id.starts_with(&child_prefix) {
            continue;
        }
        let task_name = info.id[child_prefix.len()..].to_string();
        let (_, child_events) = match backend.read_events(&info.id) {
            Ok(x) => x,
            Err(_) => {
                // Child exists on disk but we can't read it. Record the
                // name with no spawn_entry so R8 is vacuous rather than
                // silently skipping the entry.
                snapshot.insert(task_name, None);
                continue;
            }
        };
        let spawn_entry = child_events.iter().find_map(|e| match &e.payload {
            EventPayload::WorkflowInitialized { spawn_entry, .. } => spawn_entry.clone(),
            _ => None,
        });
        snapshot.insert(task_name, spawn_entry);
    }
    snapshot
}

// --------- Tests ----------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn task(name: &str, waits_on: &[&str]) -> TaskEntry {
        TaskEntry {
            name: name.to_string(),
            template: None,
            vars: BTreeMap::new(),
            waits_on: waits_on.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn build_dag_linear_chain_topological_order_matches_submission() {
        let tasks = vec![task("a", &[]), task("b", &["a"]), task("c", &["b"])];
        let dag = build_dag(&tasks);
        assert_eq!(dag.topological_order, vec!["a", "b", "c"]);
    }

    #[test]
    fn build_dag_diamond_topological_order_is_valid() {
        // a -> b, a -> c, b -> d, c -> d
        let tasks = vec![
            task("a", &[]),
            task("b", &["a"]),
            task("c", &["a"]),
            task("d", &["b", "c"]),
        ];
        let dag = build_dag(&tasks);
        // a before b/c, b/c before d.
        let idx = |n: &str| {
            dag.topological_order
                .iter()
                .position(|&x| x == n)
                .expect("in order")
        };
        assert!(idx("a") < idx("b"));
        assert!(idx("a") < idx("c"));
        assert!(idx("b") < idx("d"));
        assert!(idx("c") < idx("d"));
        assert_eq!(dag.topological_order.len(), 4);
    }

    #[test]
    fn build_dag_preserves_submission_order_at_each_frontier() {
        // Two independent parallel tasks: submission order is b, a.
        let tasks = vec![task("b", &[]), task("a", &[])];
        let dag = build_dag(&tasks);
        assert_eq!(dag.topological_order, vec!["b", "a"]);
    }

    #[test]
    fn classify_task_unspawned_ready_when_no_deps() {
        let t = task("a", &[]);
        let classifications = HashMap::new();
        assert_eq!(
            classify_task(&t, None, &classifications, FailurePolicy::SkipDependents),
            TaskClassification::Ready
        );
    }

    #[test]
    fn classify_task_unspawned_blocked_when_dep_not_success() {
        let t = task("b", &["a"]);
        let mut classifications = HashMap::new();
        classifications.insert("a".to_string(), TaskClassification::Running);
        assert_eq!(
            classify_task(&t, None, &classifications, FailurePolicy::SkipDependents),
            TaskClassification::BlockedByDep
        );
    }

    #[test]
    fn classify_task_unspawned_ready_when_all_deps_succeed() {
        let t = task("b", &["a"]);
        let mut classifications = HashMap::new();
        classifications.insert("a".to_string(), TaskClassification::Success);
        assert_eq!(
            classify_task(&t, None, &classifications, FailurePolicy::SkipDependents),
            TaskClassification::Ready
        );
    }

    #[test]
    fn classify_task_spawned_running_when_non_terminal() {
        let t = task("a", &[]);
        let snap = ChildSnapshot {
            current_state: "work".to_string(),
            terminal: false,
            failure: false,
            skipped_marker: false,
            spawn_entry: None,
        };
        let classifications = HashMap::new();
        assert_eq!(
            classify_task(
                &t,
                Some(&snap),
                &classifications,
                FailurePolicy::SkipDependents
            ),
            TaskClassification::Running
        );
    }

    #[test]
    fn classify_task_spawned_success_when_terminal_non_failure() {
        let t = task("a", &[]);
        let snap = ChildSnapshot {
            current_state: "done".to_string(),
            terminal: true,
            failure: false,
            skipped_marker: false,
            spawn_entry: None,
        };
        let classifications = HashMap::new();
        assert_eq!(
            classify_task(
                &t,
                Some(&snap),
                &classifications,
                FailurePolicy::SkipDependents
            ),
            TaskClassification::Success
        );
    }

    #[test]
    fn classify_task_spawned_failure_takes_priority_over_skipped_marker() {
        let t = task("a", &[]);
        let snap = ChildSnapshot {
            current_state: "failed".to_string(),
            terminal: true,
            failure: true,
            skipped_marker: false,
            spawn_entry: None,
        };
        let classifications = HashMap::new();
        assert_eq!(
            classify_task(
                &t,
                Some(&snap),
                &classifications,
                FailurePolicy::SkipDependents
            ),
            TaskClassification::Failure
        );
    }

    #[test]
    fn classify_task_spawned_skipped_when_flagged() {
        let t = task("a", &[]);
        let snap = ChildSnapshot {
            current_state: "skipped".to_string(),
            terminal: true,
            failure: false,
            skipped_marker: true,
            spawn_entry: None,
        };
        let classifications = HashMap::new();
        assert_eq!(
            classify_task(
                &t,
                Some(&snap),
                &classifications,
                FailurePolicy::SkipDependents
            ),
            TaskClassification::Skipped
        );
    }

    #[test]
    fn state_has_materialize_children_true_when_hook_present() {
        use crate::template::types::{
            default_failure_policy, FailurePolicy, MaterializeChildrenSpec, TemplateState,
        };
        let mut compiled = CompiledTemplate {
            format_version: 1,
            name: "t".to_string(),
            version: "1".to_string(),
            description: String::new(),
            initial_state: "s".to_string(),
            variables: BTreeMap::new(),
            states: BTreeMap::new(),
        };
        let _ = FailurePolicy::SkipDependents; // silence unused import
        let mut state = TemplateState {
            directive: "x".to_string(),
            details: String::new(),
            transitions: Vec::new(),
            terminal: false,
            gates: BTreeMap::new(),
            accepts: None,
            integration: None,
            default_action: None,
            materialize_children: None,
            failure: false,
            skipped_marker: false,
        };
        compiled.states.insert("s".to_string(), state.clone());
        assert!(!state_has_materialize_children(&compiled, "s"));
        state.materialize_children = Some(MaterializeChildrenSpec {
            from_field: "tasks".to_string(),
            default_template: "child.md".to_string(),
            failure_policy: default_failure_policy(),
        });
        compiled.states.insert("s".to_string(), state);
        assert!(state_has_materialize_children(&compiled, "s"));
    }
}
