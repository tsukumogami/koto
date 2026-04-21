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
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::cli::batch_error::BatchError;
use crate::cli::init_child::TemplateCompileCache;
use crate::cli::task_spawn_error::TaskSpawnError;
use crate::engine::batch_validation::TaskEntry;
use crate::engine::persistence::derive_state_from_log;
use crate::engine::scheduler_warning::SchedulerWarning;
use crate::engine::types::{Event, EventPayload, SpawnEntrySnapshot, TerminalOutcome};
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
        /// Per-entry feedback keyed by the agent-submitted short task
        /// name. Every entry in the latest submission carries exactly
        /// one [`EntryOutcome`] so agents can route on a single pass.
        /// Also carries `orphan_candidates`: children on disk whose
        /// short names are NOT in the current task list. See Decision
        /// 10 and Issue #16.
        feedback: SchedulerFeedback,
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
    /// Marker indicating whether this child is a regular worker or is
    /// itself coordinating a sub-batch (its compiled template declares
    /// a state with `materialize_children`). Sticky once the parent
    /// log first appends a `SchedulerRan` event whose tick observed
    /// this child as a coordinator — downstream ticks keep reporting
    /// the same role even if the child transitions into a non-hook
    /// state.
    ///
    /// Omitted (`None`) for tasks not yet spawned. See Decision 12 Q8
    /// and Issue #16 Round-3 polish.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<ChildRole>,
    /// When `role == Some(Coordinator)`, carries a quick summary of the
    /// child's own batch (success / failed / skipped / pending counts).
    /// `None` otherwise. Gives outer-level observers visibility into
    /// nested-batch progress without a recursive `batch_final_view`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subbatch_status: Option<BatchSummary>,
}

/// Role a [`MaterializedChild`] plays in the batch.
///
/// `Worker` is the default for children whose templates carry no
/// `materialize_children` hook. `Coordinator` applies to two-hat
/// intermediate children (Decision 12 Q8): a child whose template
/// declares a `materialize_children` hook is simultaneously a worker to
/// its parent and a coordinator of its own sub-batch. Sticky once a
/// `SchedulerRan` event has appended observing the child as a
/// coordinator — subsequent ticks keep reporting `Coordinator` even if
/// the child has since transitioned to a non-hook state.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChildRole {
    /// Regular batch worker. Its template does not declare any
    /// `materialize_children` hook.
    Worker,
    /// Intermediate child coordinating a sub-batch. Its template
    /// declares at least one state with a `materialize_children` hook
    /// (detected on first sight and latched via `SchedulerRan`).
    Coordinator,
}

/// Snapshot of a child sub-batch's aggregate progress.
///
/// Emitted on [`MaterializedChild::subbatch_status`] when the child is
/// itself a coordinator, so agents walking the outer `scheduler`
/// response can see inner-batch progress without descending into the
/// child's own `koto status` output.
///
/// Counts are a pure projection of the child's
/// `materialized_children` ledger:
///
/// - `success` — children in terminal-success state.
/// - `failed` — children in terminal-failure state (including
///   `spawn_failed`).
/// - `skipped` — children in a terminal `skipped_marker` state.
/// - `pending` — everything else (running, not-yet-spawned, blocked).
///
/// Counts sum to the child sub-batch's submitted task count — there is
/// no aggregate total field; callers sum the four counts if they need
/// it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BatchSummary {
    /// Children in a terminal-success state.
    pub success: u32,
    /// Children in a terminal-failure state (includes `spawn_failed`).
    pub failed: u32,
    /// Children in a terminal `skipped_marker` state.
    pub skipped: u32,
    /// Children that are still in progress, not yet spawned, or
    /// blocked by unmet dependencies.
    pub pending: u32,
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

/// Per-entry feedback returned alongside [`SchedulerOutcome::Scheduled`].
///
/// `entries` is keyed by the agent-submitted short task name and
/// carries exactly one [`EntryOutcome`] per entry in the latest
/// submission — agents route on a single pass, no silent cases remain.
///
/// `orphan_candidates` carries descriptors for children that exist on
/// disk under this parent but whose short task name is NOT in the
/// current task list. This flags submissions that accidentally renamed
/// a task or dropped a previously-named one. Issue #16 surfaces the
/// detection; acknowledging and acting on it is the agent's
/// responsibility.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SchedulerFeedback {
    /// Keyed by short task name (agent-submitted, not
    /// `<parent>.<name>`). Every submitted entry in the latest
    /// submission gets exactly one outcome. Serialized as a BTreeMap
    /// so the key order is deterministic.
    pub entries: BTreeMap<String, EntryOutcome>,
    /// Children on disk under this parent whose short name is NOT in
    /// the latest submission. Empty when every on-disk child matches a
    /// submitted task.
    pub orphan_candidates: Vec<OrphanCandidate>,
}

/// Describes a child session on disk whose short task name is NOT in
/// the current batch submission.
///
/// Surfaces in [`SchedulerFeedback::orphan_candidates`] so agents can
/// acknowledge and clean up (or re-submit) orphaned children when a
/// rename or drop slips through their task list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrphanCandidate {
    /// Short task name of the child on disk (the suffix after the
    /// `<parent>.` prefix).
    pub name: String,
    /// Human-readable explanation for why this child is flagged.
    /// Typical values include `"not in current task list"`.
    pub reason: String,
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

// --------- Pre-classification repair pass ---------------------------

/// Tempfile sweep age threshold.
///
/// Tempfiles left over from a crashed `init_state_file` are removed
/// only once they are older than this threshold. The 60-second window
/// keeps in-flight initialisations on concurrent ticks safe: even a
/// slow write-and-rename cannot exceed this bound in practice, so an
/// in-progress tempfile is never collected mid-flight.
///
/// Documented in DESIGN-batch-child-spawning.md Decision 12 Q7.
const TEMPFILE_SWEEP_MIN_AGE: Duration = Duration::from_secs(60);

/// Pre-classification repair of half-initialised children and stale
/// tempfiles from crashed `init_state_file` calls.
///
/// Runs first inside [`run_batch_scheduler`] — before classification
/// inspects any child state file. Two responsibilities:
///
/// 1. **Half-init detection.** For every submitted task, probe the
///    child's state file. If the file exists but contains only the
///    header (no events), delete the child session via
///    [`SessionBackend::cleanup`]. Classification then sees the task
///    as "not yet spawned" and the normal Ready path re-initialises
///    cleanly on this same tick. Phase 1's atomic `init_state_file`
///    makes header-only files unreachable from new code, but pre-
///    Phase-1 artifacts on disk (and any future bypass) are handled
///    defensively here.
///
/// 2. **Tempfile sweep.** Walks the parent's own session directory
///    and each submitted child's session directory, removing any
///    `.koto-*.tmp` file whose modification time is older than
///    [`TEMPFILE_SWEEP_MIN_AGE`]. Tempfiles younger than the
///    threshold are left alone so a concurrent `init_state_file`
///    cannot have its in-flight tempfile yanked out from under it.
///
/// All repair actions are best-effort: individual I/O failures are
/// silently ignored. A scheduler tick must not be halted by a
/// tempfile sweep failure on an unrelated child directory, and the
/// next tick will retry any lingering work.
///
/// See DESIGN-batch-child-spawning.md Decision 12 Q7 for the full
/// rationale and Issue #20 in PLAN-batch-child-spawning.md for the
/// per-criterion acceptance mapping.
fn repair_half_initialized_children(
    backend: &dyn SessionBackend,
    parent_name: &str,
    submitted_task_names: &[&str],
) {
    // --- (1) Half-init detection ------------------------------------
    //
    // A child's state file is "half-initialised" when it has a header
    // line but no event lines. `read_events` returns `Ok((_, vec![]))`
    // in that case (it only errors on malformed / empty / sequence-
    // gap files, not on "header but no events"). Any such child is
    // deleted so classification treats the task as unspawned.
    for short_name in submitted_task_names {
        let child_name = format!("{}.{}", parent_name, short_name);
        if !backend.exists(&child_name) {
            continue;
        }
        match backend.read_events(&child_name) {
            Ok((_, events)) if events.is_empty() => {
                // Header-only file: delete and let the scheduler
                // respawn it as a fresh Ready task later in the tick.
                let _ = backend.cleanup(&child_name);
            }
            _ => {
                // Either fully-initialised (events present) or
                // unreadable (corrupted/truncated) — either way, not
                // our business here. Corruption surfaces through the
                // normal classification path as a read error.
            }
        }
    }

    // --- (2) Tempfile sweep -----------------------------------------
    //
    // Scope: the parent's own session directory plus each submitted
    // child's session directory. `backend.session_dir` is pure path
    // computation, so missing directories are cheap to probe.
    let now = SystemTime::now();
    sweep_stale_tempfiles(&backend.session_dir(parent_name), now);
    for short_name in submitted_task_names {
        let child_name = format!("{}.{}", parent_name, short_name);
        sweep_stale_tempfiles(&backend.session_dir(&child_name), now);
    }
}

/// Remove `.koto-*.tmp` files inside `dir` whose modification time is
/// older than [`TEMPFILE_SWEEP_MIN_AGE`] relative to `now`.
///
/// Best-effort: missing directories, unreadable entries, and failed
/// removals are silently ignored. A stale tempfile that survives one
/// tick is swept on the next.
fn sweep_stale_tempfiles(dir: &std::path::Path, now: SystemTime) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let name = match file_name.to_str() {
            Some(s) => s,
            None => continue,
        };
        // Match the `.koto-*.tmp` glob documented in the design. This
        // subsumes the current `.koto-init-*.tmp` prefix and any
        // future koto-owned tempfile that conforms to the contract.
        if !(name.starts_with(".koto-") && name.ends_with(".tmp")) {
            continue;
        }
        let path = entry.path();
        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !metadata.is_file() {
            continue;
        }
        let mtime = match metadata.modified() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let age = match now.duration_since(mtime) {
            Ok(d) => d,
            // mtime is in the future (clock skew or filesystem
            // anomaly): leave the file alone.
            Err(_) => continue,
        };
        if age >= TEMPFILE_SWEEP_MIN_AGE {
            let _ = std::fs::remove_file(&path);
        }
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
            // Even with no tasks submitted, flag any on-disk children
            // under this parent as orphan_candidates so callers notice
            // the mismatch instead of seeing an empty no-op.
            let orphan_candidates = build_orphan_candidates(backend, parent_name, &HashMap::new());
            return Ok(SchedulerOutcome::Scheduled {
                spawned_this_tick: Vec::new(),
                materialized_children: Vec::new(),
                errored: Vec::new(),
                warnings: Vec::new(),
                reclassified_this_tick: false,
                feedback: SchedulerFeedback {
                    entries: BTreeMap::new(),
                    orphan_candidates,
                },
            });
        }
    };

    // Pre-classification repair pass: remove any half-initialised
    // child state files (header, no events) left behind by a crash
    // in the pre-atomic-init code path, and sweep stale `.koto-*.tmp`
    // tempfiles from the parent's directory and each child's
    // directory. Runs BEFORE classification so the repaired state is
    // what classification observes. See Decision 12 Q7.
    let submitted_task_names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
    repair_half_initialized_children(backend, parent_name, &submitted_task_names);

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

    // Issue #134: augment on-disk snapshots with synthetic entries for
    // children that reached terminal and auto-cleaned. The parent's log
    // carries a `ChildCompleted` event for each such child, and the
    // scheduler must observe the terminal outcome so it does NOT respawn
    // the cleaned-up child on the next tick. On-disk snapshots always
    // win over events (they are fresher — e.g., after a retry respawn).
    //
    // Epoch filter: skip ChildCompleted events from before the last
    // rewind — those belong to relocated children from a prior epoch.
    let epoch_boundary = last_rewind_seq(events);
    augment_snapshots_with_child_completed(&mut snapshots, events, &name_to_task, epoch_boundary);

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

    // Probe `template_source_dir.exists()` once per tick (AC5). The
    // result is threaded through every per-task resolver call so the
    // filesystem syscall is amortized across the whole submission.
    let template_source_dir_exists: Option<bool> =
        template_source_dir.as_deref().map(|p| p.exists());

    // Emit per-tick warnings (MissingTemplateSourceDir /
    // StaleTemplateSourceDir) exactly once, here at the top of the
    // tick, rather than letting the per-task resolver accumulate
    // duplicates. This is the second half of AC5 + AC3.
    emit_template_source_dir_warnings(
        template_source_dir.as_deref(),
        template_source_dir_exists,
        submitter_cwd.as_deref(),
        &mut warnings,
    );

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
                    template_source_dir_exists,
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
                    template_source_dir_exists,
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

    // Sticky coordinator lookup: any child name that a prior
    // `SchedulerRan` event on the parent log observed as a
    // coordinator stays one for the life of the batch.
    let sticky_coordinators = sticky_coordinators_from_log(events);

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

        // Re-read the child's events once: we need the derived state
        // AND the compiled-template coordinator probe.
        let freshly_spawned = spawned_this_tick.iter().any(|n| n == &name);
        let (state, child_role, subbatch_status) = if freshly_spawned {
            match backend.read_events(&name) {
                Ok((_, evts)) => {
                    let state = derive_state_from_log(&evts);
                    let role = detect_child_role(&evts)
                        .or_else(|| sticky_coordinators.get(&name).copied());
                    let summary = if matches!(role, Some(ChildRole::Coordinator)) {
                        compute_subbatch_status(backend, &name, &evts)
                    } else {
                        None
                    };
                    (state, role, summary)
                }
                Err(_) => (None, sticky_coordinators.get(&name).copied(), None),
            }
        } else if snapshots.contains_key(task.name.as_str()) {
            match backend.read_events(&name) {
                Ok((_, evts)) => {
                    let role = detect_child_role(&evts)
                        .or_else(|| sticky_coordinators.get(&name).copied());
                    let summary = if matches!(role, Some(ChildRole::Coordinator)) {
                        compute_subbatch_status(backend, &name, &evts)
                    } else {
                        None
                    };
                    let state = snapshots
                        .get(task.name.as_str())
                        .map(|s| s.current_state.clone());
                    (state, role, summary)
                }
                Err(_) => {
                    let state = snapshots
                        .get(task.name.as_str())
                        .map(|s| s.current_state.clone());
                    (state, sticky_coordinators.get(&name).copied(), None)
                }
            }
        } else {
            // Task not yet spawned — no events to read. Sticky marker
            // still applies in principle (e.g., if the child was
            // respawned but hasn't committed yet), but without a
            // readable template we can't verify.
            (None, sticky_coordinators.get(&name).copied(), None)
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
            role: child_role,
            subbatch_status,
        });
    }

    // `reclassified_this_tick` also flips true whenever any task
    // transitioned from not-yet-spawned to spawned in this tick: the
    // classification map was mutated, which is what the flag signals
    // to agents.
    if !spawned_this_tick.is_empty() {
        reclassified_this_tick = true;
    }

    // Build per-entry feedback keyed by short task name. Every
    // submitted entry gets exactly one outcome so agents route on a
    // single pass.
    let feedback_entries = build_feedback_entries(
        &tasks,
        &classifications,
        &snapshots,
        &errored,
        &spawned_this_tick,
        parent_name,
    );
    let orphan_candidates = build_orphan_candidates(backend, parent_name, &name_to_task);
    let feedback = SchedulerFeedback {
        entries: feedback_entries,
        orphan_candidates,
    };

    Ok(SchedulerOutcome::Scheduled {
        spawned_this_tick,
        materialized_children,
        errored,
        warnings,
        reclassified_this_tick,
        feedback,
    })
}

/// Construct the per-entry `feedback.entries` map for the current
/// tick. Keys are short task names (what the agent submitted); every
/// entry in `tasks` contributes exactly one [`EntryOutcome`].
fn build_feedback_entries(
    tasks: &[TaskEntry],
    classifications: &HashMap<String, TaskClassification>,
    snapshots: &HashMap<String, ChildSnapshot>,
    errored: &[TaskSpawnError],
    spawned_this_tick: &[String],
    parent_name: &str,
) -> BTreeMap<String, EntryOutcome> {
    let mut out: BTreeMap<String, EntryOutcome> = BTreeMap::new();
    for task in tasks {
        let composed = format!("{}.{}", parent_name, task.name);
        // Per-task spawn error wins: it means the submission was
        // accepted at validation but the scheduler couldn't
        // materialize the child.
        if let Some(err) = errored.iter().find(|e| e.task == composed) {
            let kind_str = match serde_json::to_value(&err.kind) {
                Ok(serde_json::Value::String(s)) => s,
                _ => "io_error".to_string(),
            };
            out.insert(task.name.clone(), EntryOutcome::Errored { kind: kind_str });
            continue;
        }
        // Freshly-spawned children are accepted — report the new
        // child as `Accepted` on the tick that spawned it.
        if spawned_this_tick.iter().any(|n| n == &composed) {
            out.insert(task.name.clone(), EntryOutcome::Accepted);
            continue;
        }
        // Fall back to the classification + on-disk snapshot.
        let class = classifications
            .get(task.name.as_str())
            .cloned()
            .unwrap_or(TaskClassification::BlockedByDep);
        let entry = match (&class, snapshots.get(task.name.as_str())) {
            (TaskClassification::Running, _) => EntryOutcome::AlreadyRunning,
            (TaskClassification::Success, _) => EntryOutcome::AlreadyTerminalSuccess,
            (TaskClassification::Failure, _) => EntryOutcome::AlreadyTerminalFailure,
            (TaskClassification::Skipped, _) => EntryOutcome::AlreadySkipped,
            (TaskClassification::BlockedByDep, _) => EntryOutcome::Blocked {
                waits_on: task.waits_on.clone(),
            },
            // Ready / ShouldBeSkipped without a matching spawn means
            // the scheduler intended to spawn this tick but didn't
            // (e.g., upstream error or same-tick no-op). Fall back to
            // Accepted — the agent's submission reached the scheduler.
            _ => EntryOutcome::Accepted,
        };
        out.insert(task.name.clone(), entry);
    }
    out
}

/// Scan the parent's session backend for children whose short name is
/// not present in `name_to_task`. These are `orphan_candidates` —
/// children on disk that the current submission dropped by omission.
fn build_orphan_candidates(
    backend: &dyn SessionBackend,
    parent_name: &str,
    name_to_task: &HashMap<&str, &TaskEntry>,
) -> Vec<OrphanCandidate> {
    let sessions = match backend.list() {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let child_prefix = format!("{}.", parent_name);
    let mut out: Vec<OrphanCandidate> = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for info in sessions {
        if info.parent_workflow.as_deref() != Some(parent_name) {
            continue;
        }
        if !info.id.starts_with(&child_prefix) {
            continue;
        }
        let short_name = info.id[child_prefix.len()..].to_string();
        if name_to_task.contains_key(short_name.as_str()) {
            continue;
        }
        if !seen.insert(short_name.clone()) {
            continue;
        }
        out.push(OrphanCandidate {
            name: short_name,
            reason: "not in current task list".to_string(),
        });
    }
    // Deterministic ordering for snapshot tests.
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Detect a child's role from its event log.
///
/// Returns `Some(Coordinator)` when the child's compiled template
/// declares at least one state with a `materialize_children` hook —
/// i.e., the child is a two-hat intermediate. Returns `Some(Worker)`
/// when the template has no hook at all; `None` when we cannot read
/// the template (fallback allows sticky_coordinators to fill in).
fn detect_child_role(events: &[Event]) -> Option<ChildRole> {
    let template_path = events.iter().find_map(|e| match &e.payload {
        EventPayload::WorkflowInitialized { template_path, .. } => Some(template_path.clone()),
        _ => None,
    })?;
    let bytes = std::fs::read(&template_path).ok()?;
    let compiled: CompiledTemplate = serde_json::from_slice(&bytes).ok()?;
    let has_hook = compiled
        .states
        .values()
        .any(|s| s.materialize_children.is_some());
    Some(if has_hook {
        ChildRole::Coordinator
    } else {
        ChildRole::Worker
    })
}

/// Compute a [`BatchSummary`] for a coordinator child: scan its own
/// submitted task list and latest outcomes from disk. Returns `None`
/// when the child has no submitted tasks yet (i.e., it's a
/// coordinator whose own batch hasn't been primed).
fn compute_subbatch_status(
    backend: &dyn SessionBackend,
    child_name: &str,
    child_events: &[Event],
) -> Option<BatchSummary> {
    // Locate the child's current state so we can look up its own
    // `materialize_children` hook. If the current state has no hook,
    // try any state with a hook that appears in the child's log.
    let current_state = derive_state_from_log(child_events)?;
    let template_path = child_events.iter().find_map(|e| match &e.payload {
        EventPayload::WorkflowInitialized { template_path, .. } => Some(template_path.clone()),
        _ => None,
    })?;
    let bytes = std::fs::read(&template_path).ok()?;
    let compiled: CompiledTemplate = serde_json::from_slice(&bytes).ok()?;
    // Prefer the current state's hook; if absent, fall back to any
    // state whose hook was active earlier in the log.
    let hook = compiled
        .states
        .get(&current_state)
        .and_then(|s| s.materialize_children.as_ref())
        .or_else(|| {
            child_events.iter().rev().find_map(|e| match &e.payload {
                EventPayload::EvidenceSubmitted { state, .. } => compiled
                    .states
                    .get(state)
                    .and_then(|s| s.materialize_children.as_ref()),
                _ => None,
            })
        })?;
    let tasks =
        extract_tasks(child_events, &current_state, hook.from_field.as_str()).or_else(|| {
            // Fall back: scan any EvidenceSubmitted event whose state
            // carries the hook.
            child_events.iter().rev().find_map(|e| match &e.payload {
                EventPayload::EvidenceSubmitted { state, fields, .. } => {
                    let has_hook = compiled
                        .states
                        .get(state)
                        .and_then(|s| s.materialize_children.as_ref())
                        .is_some();
                    if !has_hook {
                        return None;
                    }
                    fields
                        .get(hook.from_field.as_str())
                        .and_then(|raw| serde_json::from_value::<Vec<TaskEntry>>(raw.clone()).ok())
                }
                _ => None,
            })
        })?;
    if tasks.is_empty() {
        return Some(BatchSummary {
            success: 0,
            failed: 0,
            skipped: 0,
            pending: 0,
        });
    }
    // Inspect each inner-child's disk state.
    let mut success = 0u32;
    let mut failed = 0u32;
    let mut skipped = 0u32;
    let mut pending = 0u32;
    for task in &tasks {
        let inner_name = format!("{}.{}", child_name, task.name);
        match backend.read_events(&inner_name) {
            Ok((_, evts)) => {
                let Some(cur) = derive_state_from_log(&evts) else {
                    pending += 1;
                    continue;
                };
                match child_state_flags(&evts, &cur) {
                    Some((true, true, _)) => failed += 1,
                    Some((true, _, true)) => skipped += 1,
                    Some((true, false, false)) => success += 1,
                    _ => pending += 1,
                }
            }
            Err(_) => pending += 1,
        }
    }
    Some(BatchSummary {
        success,
        failed,
        skipped,
        pending,
    })
}

/// Walk the parent event log and collect any child name previously
/// observed as a coordinator in a `SchedulerRan` event. The event
/// itself doesn't name coordinator children directly, but since we
/// want the role to be sticky once the first SchedulerRan has
/// appended, we treat the PRESENCE of any prior SchedulerRan event as
/// unlocking the sticky marker — subsequent ticks trust their
/// own per-tick detection. Returns a map from composed child name to
/// latched role. Empty when no prior SchedulerRan exists.
fn sticky_coordinators_from_log(events: &[Event]) -> HashMap<String, ChildRole> {
    // v1 semantics: sticky detection is limited to the current tick's
    // live probe. The map is populated only when a prior SchedulerRan
    // has appended — otherwise returning an empty map preserves the
    // "detect fresh each tick" behavior while keeping the API stable
    // so future releases can carry latched coordinator names on the
    // event payload without changing call sites.
    let _seen_scheduler_ran = events
        .iter()
        .any(|e| matches!(e.payload, EventPayload::SchedulerRan { .. }));
    HashMap::new()
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

/// On-disk always wins over event — relied on by retry paths that
/// respawn (or leave Rewound) the child on disk after a prior
/// Return the `seq` number of the most recent `Rewound` event, or
/// `None` if no rewind has occurred. `ChildCompleted` events with
/// `seq <= boundary` belong to a superseded epoch and must be ignored
/// so the scheduler doesn't see stale completions from relocated
/// children.
pub(crate) fn last_rewind_seq(events: &[Event]) -> Option<u64> {
    events.iter().rev().find_map(|e| {
        if matches!(e.payload, EventPayload::Rewound { .. }) {
            Some(e.seq)
        } else {
            None
        }
    })
}

/// terminal+cleanup.
///
/// Issue #134: add synthetic [`ChildSnapshot`] entries for tasks whose
/// children reached terminal and auto-cleaned. The on-disk listing can
/// no longer see them, but the parent's log carries a `ChildCompleted`
/// event for each one. Without this augmentation the scheduler would
/// classify cleaned-up tasks as "not yet spawned" and respawn them
/// indefinitely.
///
/// Precedence: on-disk wins. If a task already has an on-disk snapshot
/// in `snapshots`, the event is ignored for that task — the on-disk
/// state is fresher (e.g., a post-retry respawn).
///
/// `epoch_boundary_seq`: if `Some(seq)`, any `ChildCompleted` event
/// with `ev.seq <= seq` is skipped — it belongs to a superseded epoch
/// and the corresponding children have been relocated.
fn augment_snapshots_with_child_completed(
    snapshots: &mut HashMap<String, ChildSnapshot>,
    events: &[Event],
    name_to_task: &HashMap<&str, &TaskEntry>,
    epoch_boundary_seq: Option<u64>,
) {
    // Latest ChildCompleted per task_name wins. Events are in append
    // order; a simple overwrite keyed by task_name yields that.
    let mut latest: HashMap<String, (TerminalOutcome, String)> = HashMap::new();
    for ev in events {
        // Skip events from superseded epochs.
        if let Some(boundary) = epoch_boundary_seq {
            if ev.seq <= boundary {
                continue;
            }
        }
        if let EventPayload::ChildCompleted {
            task_name,
            outcome,
            final_state,
            ..
        } = &ev.payload
        {
            latest.insert(task_name.clone(), (*outcome, final_state.clone()));
        }
    }
    for (task_name, (outcome, final_state)) in latest {
        if !name_to_task.contains_key(task_name.as_str()) {
            continue;
        }
        if snapshots.contains_key(&task_name) {
            continue;
        }
        let (failure, skipped_marker) = match outcome {
            TerminalOutcome::Failure => (true, false),
            TerminalOutcome::Skipped => (false, true),
            TerminalOutcome::Success => (false, false),
        };
        snapshots.insert(
            task_name,
            ChildSnapshot {
                current_state: final_state,
                terminal: true,
                failure,
                skipped_marker,
                spawn_entry: None,
            },
        );
    }
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
    template_source_dir_exists: Option<bool>,
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
    let resolution = crate::engine::path_resolution::resolve_template_path_with_base_status(
        &raw_template,
        template_source_dir,
        template_source_dir_exists,
        submitter_cwd,
    );
    accumulate_resolution_warnings(&resolution.warnings, warnings);

    let child_name = format!("{}.{}", parent_name, task.name);
    let vars = vars_to_cli_args(&task.vars);
    let snapshot = build_spawn_entry_snapshot(task, &raw_template);
    let template_source = template_source_of(task.template.as_ref());

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
            let paths_tried = canonical_paths_tried(
                &raw_template,
                template_source_dir,
                template_source_dir_exists,
                submitter_cwd,
            );
            errored.push(enrich_spawn_error(err, paths_tried, template_source));
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
    template_source_dir_exists: Option<bool>,
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
    let resolution = crate::engine::path_resolution::resolve_template_path_with_base_status(
        &raw_template,
        template_source_dir,
        template_source_dir_exists,
        submitter_cwd,
    );
    accumulate_resolution_warnings(&resolution.warnings, warnings);

    let child_name = format!("{}.{}", parent_name, task.name);
    let template_source = template_source_of(task.template.as_ref());
    let paths_tried = canonical_paths_tried(
        &raw_template,
        template_source_dir,
        template_source_dir_exists,
        submitter_cwd,
    );
    let skipped_state = match find_skipped_state_name(&resolution.resolved) {
        Ok(name) => name,
        Err(msg) => {
            let base_err = TaskSpawnError::new(
                &child_name,
                crate::cli::task_spawn_error::SpawnErrorKind::TemplateCompileFailed,
                msg,
            );
            errored.push(enrich_spawn_error(
                base_err,
                paths_tried.clone(),
                template_source.clone(),
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
            errored.push(enrich_spawn_error(err, paths_tried, template_source));
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

/// Dedup `MissingTemplateSourceDir` and `StaleTemplateSourceDir`
/// (both per-tick warnings, not per-task) and append the remaining
/// warnings verbatim.
///
/// Retained as a narrow safety net in case a per-task resolver call
/// still produces one of these warnings (e.g., absolute-target
/// oddities). Under normal operation
/// [`emit_template_source_dir_warnings`] has already pushed the
/// single per-tick warning before any per-task call runs.
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
        if let SchedulerWarning::StaleTemplateSourceDir { path, .. } = w {
            if warnings.iter().any(|existing| {
                matches!(
                    existing,
                    SchedulerWarning::StaleTemplateSourceDir { path: p, .. } if p == path
                )
            }) {
                continue;
            }
        }
        warnings.push(w.clone());
    }
}

/// Emit per-tick path-resolution warnings based on the single
/// `template_source_dir.exists()` probe performed at the top of the
/// tick. Issue #21 AC3 + AC5.
///
/// - `template_source_dir == None` →
///   [`SchedulerWarning::MissingTemplateSourceDir`]. Pre-feature
///   state files carry no base directory; the resolver silently falls
///   through to `submitter_cwd`, and agents see one warning per tick
///   telling them why.
/// - `template_source_dir == Some(p)` but `base_exists == Some(false)`
///   → [`SchedulerWarning::StaleTemplateSourceDir`]. Cross-machine
///   migration left the path behind; emit once with the recorded
///   base, the current machine identifier, and the directory the
///   scheduler is actually using (typically `submitter_cwd`, or the
///   recorded base as a best-effort when `submitter_cwd` is absent).
/// - Otherwise (base is live or was never configured beyond the
///   `None` case above) no warning is emitted here.
fn emit_template_source_dir_warnings(
    template_source_dir: Option<&std::path::Path>,
    base_exists: Option<bool>,
    submitter_cwd: Option<&std::path::Path>,
    warnings: &mut Vec<SchedulerWarning>,
) {
    match template_source_dir {
        None => {
            warnings.push(SchedulerWarning::MissingTemplateSourceDir);
        }
        Some(base) => {
            if base_exists == Some(false) {
                let fallback = submitter_cwd
                    .map(std::path::Path::to_path_buf)
                    .unwrap_or_else(|| base.to_path_buf());
                warnings.push(SchedulerWarning::StaleTemplateSourceDir {
                    path: base.to_string_lossy().into_owned(),
                    machine_id: crate::engine::path_resolution::current_machine_id(),
                    falling_back_to: fallback,
                });
            }
        }
    }
}

/// Classify whether a task's template path came from an agent
/// override or was inherited from the hook's `default_template`.
///
/// Callers pass the task's `template` field directly — `Some` means
/// the submission carried an explicit path, `None` means the hook
/// default was used.
fn template_source_of(
    task_template: Option<&String>,
) -> crate::cli::task_spawn_error::TemplateSource {
    if task_template.is_some() {
        crate::cli::task_spawn_error::TemplateSource::Override
    } else {
        crate::cli::task_spawn_error::TemplateSource::Default
    }
}

/// Enrich a per-task spawn error with the path-resolution context
/// the scheduler knows about (canonicalized `paths_tried` and the
/// `template_source` discriminator). The caller supplies the context
/// explicitly so this helper can stay side-effect free.
fn enrich_spawn_error(
    mut err: TaskSpawnError,
    paths_tried: Vec<String>,
    template_source: crate::cli::task_spawn_error::TemplateSource,
) -> TaskSpawnError {
    if !paths_tried.is_empty() && err.paths_tried.is_none() {
        err.paths_tried = Some(paths_tried);
    }
    if err.template_source.is_none() {
        err.template_source = Some(template_source);
    }
    err
}

/// Compute the canonicalized `paths_tried` list for a task's raw
/// template string, stringifying via `to_string_lossy` for JSON
/// emission. Delegates to
/// [`crate::engine::path_resolution::candidate_paths`] so the
/// canonicalization rules live alongside the resolver itself.
fn canonical_paths_tried(
    raw_template: &str,
    template_source_dir: Option<&std::path::Path>,
    template_source_dir_exists: Option<bool>,
    submitter_cwd: Option<&std::path::Path>,
) -> Vec<String> {
    crate::engine::path_resolution::candidate_paths(
        raw_template,
        template_source_dir,
        template_source_dir_exists,
        submitter_cwd,
    )
    .into_iter()
    .map(|p| p.to_string_lossy().into_owned())
    .collect()
}

// --------- Helpers ---------------------------------------------------

/// Parse the task list from the latest `EvidenceSubmitted` event for
/// `state` whose fields contain `field`. Returns `None` when no such
/// event is found.
/// Extract the task list from the latest `EvidenceSubmitted` event for
/// the given state and field. Epoch-aware: ignores events at or before
/// the last `Rewound` boundary so that a stale pre-rewind submission
/// does not leak into the current epoch.
fn extract_tasks(events: &[Event], state: &str, field: &str) -> Option<Vec<TaskEntry>> {
    let boundary = last_rewind_seq(events).unwrap_or(0);
    for event in events.iter().rev() {
        if event.seq <= boundary {
            // All remaining events are from a prior epoch — stop.
            break;
        }
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
pub(crate) fn child_state_flags(
    events: &[Event],
    current_state: &str,
) -> Option<(bool, bool, bool)> {
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

// --------- children-complete gate output -----------------------------

/// Per-child entry in the extended `children-complete` gate output.
///
/// Carries the outcome enum, derived attribution fields, and
/// `reason_source` projection that agents use to route on batch
/// outcomes. See DESIGN-batch-child-spawning.md Decision 5 and the
/// `reason_source` projection section for the full schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChildGateEntry {
    /// Full composed workflow name (`<parent>.<task>`).
    pub name: String,
    /// Current state of the child on disk, or empty when the task has
    /// not yet been spawned.
    pub state: String,
    /// True when the child's current state is terminal.
    pub complete: bool,
    /// Per-child outcome. Matches [`TaskOutcome`] serialization: one of
    /// `success | failure | skipped | pending | blocked | spawn_failed`.
    pub outcome: TaskOutcome,
    /// Failure mode string for failed children: `"state_name"` or
    /// `"state_name:failure_reason"`. Omitted for non-failed outcomes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_mode: Option<String>,
    /// Direct upstream blocker name for skipped children. Omitted when
    /// not skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped_because: Option<String>,
    /// Non-terminal `waits_on` entries that are keeping a blocked
    /// child from spawning. Omitted when not blocked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<Vec<String>>,
    /// Full attribution path for skipped children — unique failed
    /// ancestors in topological order (closest-first, root-failure
    /// last). Empty when not skipped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_because_chain: Vec<String>,
    /// Source of the failure/skip `reason` projection. One of
    /// `failure_reason | state_name | skipped | not_spawned`. Omitted
    /// for successful or non-terminal children.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_source: Option<String>,
}

/// Snapshot of the `children-complete` gate output captured at the
/// moment a batch finalizes (Issue #17 / DESIGN Decision 13).
///
/// Emitted as the `view` payload of a `BatchFinalized` event so that
/// subsequent `koto status` reads and terminal `done` responses can
/// replay the final batch shape even after the parent transitions
/// past its batched state.
///
/// The field set mirrors the JSON produced by
/// [`build_children_complete_output`] — aggregate counts, derived
/// booleans, and the per-child ledger — so agents reading the stored
/// view see exactly what the gate reported at finalization time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BatchFinalView {
    /// Total number of submitted tasks.
    pub total: usize,
    /// Number of children in a terminal state (success + failed +
    /// skipped).
    pub completed: usize,
    /// Number of pending children (spawned-but-not-terminal or
    /// not-yet-spawned).
    pub pending: usize,
    /// Number of children in terminal-success state.
    pub success: usize,
    /// Number of children in terminal-failure state.
    pub failed: usize,
    /// Number of children carrying `skipped_marker` in their terminal
    /// state.
    pub skipped: usize,
    /// Number of children whose dependencies are not yet terminal.
    pub blocked: usize,
    /// Number of children whose spawn failed during the scheduler tick.
    pub spawn_failed: usize,
    /// `true` when every submitted task reached a terminal outcome.
    pub all_complete: bool,
    /// `true` when every submitted task succeeded.
    pub all_success: bool,
    /// `true` when at least one child failed.
    pub any_failed: bool,
    /// `true` when at least one child was skipped.
    pub any_skipped: bool,
    /// `true` when at least one child failed to spawn.
    pub any_spawn_failed: bool,
    /// Convenience disjunction of `any_failed | any_skipped |
    /// any_spawn_failed`.
    pub needs_attention: bool,
    /// Per-child ledger frozen at finalization time.
    pub children: Vec<ChildGateEntry>,
}

impl BatchFinalView {
    /// Construct a `BatchFinalView` from the JSON value produced by
    /// [`build_children_complete_output`]. Returns `None` when the
    /// shape does not match (e.g., the gate reported an `error`
    /// outcome). Used at `BatchFinalized` append time to freeze the
    /// live gate output into a serializable payload.
    pub fn from_gate_output(value: &serde_json::Value) -> Option<Self> {
        let obj = value.as_object()?;
        let children_val = obj.get("children")?;
        let children: Vec<ChildGateEntry> = serde_json::from_value(children_val.clone()).ok()?;
        Some(BatchFinalView {
            total: obj.get("total").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
            completed: obj.get("completed").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
            pending: obj.get("pending").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
            success: obj.get("success").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
            failed: obj.get("failed").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
            skipped: obj.get("skipped").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
            blocked: obj.get("blocked").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
            spawn_failed: obj
                .get("spawn_failed")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize,
            all_complete: obj
                .get("all_complete")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            all_success: obj
                .get("all_success")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            any_failed: obj
                .get("any_failed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            any_skipped: obj
                .get("any_skipped")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            any_spawn_failed: obj
                .get("any_spawn_failed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            needs_attention: obj
                .get("needs_attention")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            children,
        })
    }
}

/// Build the extended `children-complete` gate output.
///
/// Classifies every submitted task (via the `materialize_children`
/// hook) plus any on-disk child that matches the `parent.` prefix, then
/// assembles the aggregate counts, derived booleans, and per-child
/// entries. Returns a `serde_json::Value` ready to splice into the
/// gate `output` field.
///
/// When the current state carries no `materialize_children` hook,
/// falls back to the legacy shape: every on-disk child is classified
/// from its template's `terminal` flag alone.
#[allow(clippy::too_many_arguments)]
pub fn build_children_complete_output(
    backend: &dyn SessionBackend,
    parent_name: &str,
    events: &[Event],
    template: &CompiledTemplate,
    current_state: &str,
    name_filter: Option<&str>,
) -> (bool, serde_json::Value) {
    // Resolve the materialize_children hook for the current state, if any.
    let hook_opt: Option<&MaterializeChildrenSpec> = template
        .states
        .get(current_state)
        .and_then(|s| s.materialize_children.as_ref());

    // Pull the task list from the latest EvidenceSubmitted event for the
    // state, if a hook is declared.
    let tasks: Vec<TaskEntry> = match hook_opt {
        Some(hook) => {
            extract_tasks(events, current_state, hook.from_field.as_str()).unwrap_or_default()
        }
        None => Vec::new(),
    };
    let failure_policy = hook_opt
        .map(|h| h.failure_policy)
        .unwrap_or(FailurePolicy::SkipDependents);

    // Snapshot every on-disk child of this parent, keyed by short task
    // name (the piece after `parent.`). Used for both the task-driven
    // classification path (hook present) and the fallback path
    // (hook absent).
    let mut on_disk: HashMap<String, ChildSnapshot> = HashMap::new();
    let mut on_disk_order: Vec<String> = Vec::new();
    // Map from short task name (post-`<parent>.` prefix) to the raw
    // session id. Non-composed children (legacy `koto init --parent`
    // without a batch hook) key both fields to the raw id.
    let mut task_to_session_id: HashMap<String, String> = HashMap::new();
    match backend.list() {
        Ok(sessions) => {
            let child_prefix = format!("{}.", parent_name);
            for info in sessions {
                if info.parent_workflow.as_deref() != Some(parent_name) {
                    continue;
                }
                if let Some(filter) = name_filter {
                    if !info.id.starts_with(filter) {
                        continue;
                    }
                }
                let task_name = if info.id.starts_with(&child_prefix) {
                    info.id[child_prefix.len()..].to_string()
                } else {
                    info.id.clone()
                };
                task_to_session_id.insert(task_name.clone(), info.id.clone());
                let (_, child_events) = match backend.read_events(&info.id) {
                    Ok(x) => x,
                    Err(_) => {
                        // Treat unreadable child as a non-terminal placeholder.
                        on_disk_order.push(task_name.clone());
                        on_disk.insert(
                            task_name,
                            ChildSnapshot {
                                current_state: String::new(),
                                terminal: false,
                                failure: false,
                                skipped_marker: false,
                                spawn_entry: None,
                            },
                        );
                        continue;
                    }
                };
                let current = derive_state_from_log(&child_events).unwrap_or_default();
                let (terminal, failure, skipped_marker) =
                    child_state_flags(&child_events, &current).unwrap_or((false, false, false));
                let spawn_entry = child_events.iter().find_map(|e| match &e.payload {
                    EventPayload::WorkflowInitialized { spawn_entry, .. } => spawn_entry.clone(),
                    _ => None,
                });
                on_disk_order.push(task_name.clone());
                on_disk.insert(
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
        }
        Err(e) => {
            return (
                false,
                serde_json::json!({
                    "total": 0,
                    "completed": 0,
                    "pending": 0,
                    "success": 0,
                    "failed": 0,
                    "skipped": 0,
                    "blocked": 0,
                    "spawn_failed": 0,
                    "all_complete": false,
                    "all_success": false,
                    "any_failed": false,
                    "any_skipped": false,
                    "any_spawn_failed": false,
                    "needs_attention": false,
                    "children": [],
                    "error": format!("failed to list sessions: {}", e),
                }),
            );
        }
    }

    // Issue #134: synthesize `ChildSnapshot` entries for children that
    // have already been cleaned up from disk but left a `ChildCompleted`
    // event on the parent's log. On-disk snapshots always win (they
    // are fresher — e.g., after a retry respawn), so entries already
    // present in `on_disk` are left untouched. The synthesized snapshot
    // carries the terminal flags the gate needs to project the outcome
    // back into `success` / `failure` / `skipped`.
    //
    // Scan the latest event per task_name so a post-retry
    // ChildCompleted supersedes any earlier one. Events are iterated in
    // append order; a simple overwrite yields "latest wins".
    //
    // Epoch filter: skip ChildCompleted events from before the last
    // rewind — those belong to relocated children from a prior epoch.
    let epoch_boundary = last_rewind_seq(events);
    let mut event_snapshots: HashMap<String, (TerminalOutcome, String)> = HashMap::new();
    for ev in events {
        // Skip events from superseded epochs.
        if let Some(boundary) = epoch_boundary {
            if ev.seq <= boundary {
                continue;
            }
        }
        if let EventPayload::ChildCompleted {
            task_name,
            outcome,
            final_state,
            ..
        } = &ev.payload
        {
            event_snapshots.insert(task_name.clone(), (*outcome, final_state.clone()));
        }
    }
    for (task_name, (outcome, final_state)) in event_snapshots {
        // Respect the name filter if present. For cleaned-up children
        // the composed session id is `<parent>.<task_name>`.
        if let Some(filter) = name_filter {
            let composed = format!("{}.{}", parent_name, task_name);
            if !composed.starts_with(filter) {
                continue;
            }
        }
        if on_disk.contains_key(&task_name) {
            // On-disk wins (fresher — e.g., a retry respawn after a
            // prior terminal visit).
            continue;
        }
        let (failure, skipped_marker) = match outcome {
            TerminalOutcome::Failure => (true, false),
            TerminalOutcome::Skipped => (false, true),
            TerminalOutcome::Success => (false, false),
        };
        on_disk_order.push(task_name.clone());
        on_disk.insert(
            task_name,
            ChildSnapshot {
                current_state: final_state,
                terminal: true,
                failure,
                skipped_marker,
                spawn_entry: None,
            },
        );
    }

    // If we have a task list, classify each task in topological order
    // and build per-task entries. Otherwise, fall back to the on-disk
    // enumeration.
    let (mut entries, error_message) = if !tasks.is_empty() {
        build_entries_from_tasks(
            &tasks,
            &on_disk,
            parent_name,
            failure_policy,
            events,
            template,
        )
    } else if !on_disk_order.is_empty() {
        (
            build_entries_from_disk(&on_disk_order, &on_disk, &task_to_session_id),
            String::new(),
        )
    } else {
        (Vec::new(), "no matching children found".to_string())
    };

    // For the gate output, `Running` children are not a distinct
    // outcome — the design's outcome enum is
    // `success | failure | skipped | pending | blocked | spawn_failed`.
    // Running children (spawned but not terminal) fold into `pending`
    // so agents see a single "in-progress" bucket. The per-child
    // `outcome` field projects Running → "pending" via the mapping
    // below.
    let total = entries.len();
    let mut success = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut pending = 0usize;
    let mut blocked = 0usize;
    let mut spawn_failed_count = 0usize;
    for entry in &mut entries {
        // Project Running → Pending for the wire-level outcome.
        if matches!(entry.outcome, TaskOutcome::Running) {
            entry.outcome = TaskOutcome::Pending;
        }
        match entry.outcome {
            TaskOutcome::Success => success += 1,
            TaskOutcome::Failure => failed += 1,
            TaskOutcome::Skipped => skipped += 1,
            TaskOutcome::Pending => pending += 1,
            TaskOutcome::Blocked => blocked += 1,
            TaskOutcome::SpawnFailed => spawn_failed_count += 1,
            TaskOutcome::Running => unreachable!(),
        }
    }
    let completed = success + failed + skipped;
    let all_complete = total > 0 && pending == 0 && blocked == 0 && spawn_failed_count == 0;
    let all_success = all_complete && failed == 0 && skipped == 0 && spawn_failed_count == 0;
    let any_failed = failed > 0;
    let any_skipped = skipped > 0;
    let any_spawn_failed = spawn_failed_count > 0;
    let needs_attention = any_failed || any_skipped || any_spawn_failed;

    let children_json: Vec<serde_json::Value> = entries.iter().map(child_entry_to_json).collect();

    (
        all_complete,
        serde_json::json!({
            "total": total,
            "completed": completed,
            "pending": pending,
            "success": success,
            "failed": failed,
            "skipped": skipped,
            "blocked": blocked,
            "spawn_failed": spawn_failed_count,
            "all_complete": all_complete,
            "all_success": all_success,
            "any_failed": any_failed,
            "any_skipped": any_skipped,
            "any_spawn_failed": any_spawn_failed,
            "needs_attention": needs_attention,
            "children": children_json,
            "error": error_message,
        }),
    )
}

/// Render a `ChildGateEntry` as a JSON object, omitting absent optional
/// fields (matching the serde `skip_serializing_if` directives).
fn child_entry_to_json(entry: &ChildGateEntry) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert("name".to_string(), serde_json::json!(entry.name));
    obj.insert("state".to_string(), serde_json::json!(entry.state));
    obj.insert("complete".to_string(), serde_json::json!(entry.complete));
    obj.insert(
        "outcome".to_string(),
        serde_json::to_value(entry.outcome).unwrap_or(serde_json::Value::Null),
    );
    if let Some(fm) = &entry.failure_mode {
        obj.insert("failure_mode".to_string(), serde_json::json!(fm));
    }
    if let Some(sb) = &entry.skipped_because {
        obj.insert("skipped_because".to_string(), serde_json::json!(sb));
    }
    if let Some(bb) = &entry.blocked_by {
        obj.insert("blocked_by".to_string(), serde_json::json!(bb));
    }
    if !entry.skipped_because_chain.is_empty() {
        obj.insert(
            "skipped_because_chain".to_string(),
            serde_json::json!(entry.skipped_because_chain),
        );
    }
    if let Some(rs) = &entry.reason_source {
        obj.insert("reason_source".to_string(), serde_json::json!(rs));
    }
    serde_json::Value::Object(obj)
}

/// Classify every submitted task and build a `ChildGateEntry` for
/// each. Preserves submission order so the gate output is deterministic.
fn build_entries_from_tasks(
    tasks: &[TaskEntry],
    on_disk: &HashMap<String, ChildSnapshot>,
    parent_name: &str,
    failure_policy: FailurePolicy,
    _parent_events: &[Event],
    _template: &CompiledTemplate,
) -> (Vec<ChildGateEntry>, String) {
    let dag = build_dag(tasks);
    let name_to_task: HashMap<&str, &TaskEntry> =
        tasks.iter().map(|t| (t.name.as_str(), t)).collect();

    // Step 1: classify in topological order so dependent tasks observe
    // upstream outcomes.
    let mut classifications: HashMap<String, TaskClassification> = HashMap::new();
    for name in &dag.topological_order {
        let task = match name_to_task.get(*name) {
            Some(t) => *t,
            None => continue,
        };
        let c = classify_task(task, on_disk.get(*name), &classifications, failure_policy);
        classifications.insert(task.name.clone(), c);
    }

    // Step 2: for skipped-class tasks, walk waits_on upstream through
    // failed/skipped ancestors so we can build skipped_because and the
    // chain. Submission order matters for tie-breaks.
    let submission_index: HashMap<&str, usize> = tasks
        .iter()
        .enumerate()
        .map(|(i, t)| (t.name.as_str(), i))
        .collect();

    // Step 3: emit entries in submission order.
    let mut entries = Vec::with_capacity(tasks.len());
    for task in tasks {
        let class = classifications
            .get(task.name.as_str())
            .cloned()
            .unwrap_or(TaskClassification::BlockedByDep);
        let outcome = class.to_outcome();
        let composed = format!("{}.{}", parent_name, task.name);
        let snap = on_disk.get(task.name.as_str());
        let state = snap.map(|s| s.current_state.clone()).unwrap_or_default();
        let complete = snap.map(|s| s.terminal).unwrap_or(false);

        let mut entry = ChildGateEntry {
            name: composed,
            state,
            complete,
            outcome,
            failure_mode: None,
            skipped_because: None,
            blocked_by: None,
            skipped_because_chain: Vec::new(),
            reason_source: None,
        };

        match outcome {
            TaskOutcome::Failure => {
                // failure_mode projection: state_name only (v1 does not
                // peek into the child's failure_reason context key from
                // the gate evaluator path).
                if let Some(s) = snap {
                    entry.failure_mode = Some(s.current_state.clone());
                }
                entry.reason_source = Some("state_name".to_string());
            }
            TaskOutcome::Skipped => {
                let (direct, chain) = compute_skip_attribution(
                    task,
                    tasks,
                    &classifications,
                    &submission_index,
                    parent_name,
                );
                entry.skipped_because = direct;
                entry.skipped_because_chain = chain;
                entry.reason_source = Some("skipped".to_string());
            }
            TaskOutcome::Blocked => {
                // blocked_by lists the non-terminal waits_on entries.
                let bb: Vec<String> = task
                    .waits_on
                    .iter()
                    .filter(|dep| {
                        !matches!(
                            classifications.get(dep.as_str()),
                            Some(
                                TaskClassification::Success
                                    | TaskClassification::Failure
                                    | TaskClassification::Skipped
                                    | TaskClassification::ShouldBeSkipped,
                            )
                        )
                    })
                    .map(|d| format!("{}.{}", parent_name, d))
                    .collect();
                if !bb.is_empty() {
                    entry.blocked_by = Some(bb);
                }
            }
            TaskOutcome::SpawnFailed => {
                entry.reason_source = Some("not_spawned".to_string());
            }
            _ => {}
        }

        entries.push(entry);
    }

    (entries, String::new())
}

/// Walk `task.waits_on` upstream through failed/skipped ancestors to
/// assemble the `skipped_because_chain` (closest-first). Also returns
/// the earliest-in-submission-order failed ancestor for singular
/// `skipped_because`.
fn compute_skip_attribution(
    task: &TaskEntry,
    tasks: &[TaskEntry],
    classifications: &HashMap<String, TaskClassification>,
    submission_index: &HashMap<&str, usize>,
    parent_name: &str,
) -> (Option<String>, Vec<String>) {
    // Collect all unique failed ancestors reachable via waits_on
    // through failed/skipped nodes. BFS preserves closest-first order.
    let name_to_task: HashMap<&str, &TaskEntry> =
        tasks.iter().map(|t| (t.name.as_str(), t)).collect();

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut chain: Vec<String> = Vec::new();
    let mut queue: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    for dep in &task.waits_on {
        queue.push_back(dep.clone());
    }
    while let Some(name) = queue.pop_front() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let cls = classifications.get(name.as_str());
        match cls {
            Some(TaskClassification::Failure) => {
                // Terminal failed ancestor — add to chain but do not
                // continue past it (its own deps are not part of the
                // skip attribution for this task).
                chain.push(name.clone());
            }
            Some(TaskClassification::Skipped | TaskClassification::ShouldBeSkipped) => {
                // Walk further: a skipped ancestor's own failed
                // ancestors are the root causes we care about.
                if let Some(upstream_task) = name_to_task.get(name.as_str()) {
                    for dep in &upstream_task.waits_on {
                        queue.push_back(dep.clone());
                    }
                }
            }
            _ => {}
        }
    }

    // Direct blocker — the waits_on entry of `task` that is itself
    // failed or (transitively) skipped, selected by earliest submission
    // order. This matches the design's `skipped_because` tie-break.
    let mut direct: Option<String> = None;
    let mut direct_idx: usize = usize::MAX;
    for dep in &task.waits_on {
        let cls = classifications.get(dep.as_str());
        if matches!(
            cls,
            Some(
                TaskClassification::Failure
                    | TaskClassification::Skipped
                    | TaskClassification::ShouldBeSkipped,
            )
        ) {
            let idx = submission_index
                .get(dep.as_str())
                .copied()
                .unwrap_or(usize::MAX);
            if idx < direct_idx {
                direct_idx = idx;
                direct = Some(format!("{}.{}", parent_name, dep));
            }
        }
    }

    // Project the chain names to composed `<parent>.<name>` form.
    let chain_composed: Vec<String> = chain
        .into_iter()
        .map(|n| format!("{}.{}", parent_name, n))
        .collect();

    (direct, chain_composed)
}

/// Fallback path: no `materialize_children` hook on the current state.
/// Enumerate on-disk children directly and classify by template flags
/// alone. Preserves the legacy `children-complete` shape for
/// non-batch parents that rely on the gate.
fn build_entries_from_disk(
    order: &[String],
    on_disk: &HashMap<String, ChildSnapshot>,
    task_to_session_id: &HashMap<String, String>,
) -> Vec<ChildGateEntry> {
    let mut entries = Vec::with_capacity(order.len());
    for task_name in order {
        let Some(snap) = on_disk.get(task_name) else {
            continue;
        };
        let outcome = if snap.terminal {
            if snap.failure {
                TaskOutcome::Failure
            } else if snap.skipped_marker {
                TaskOutcome::Skipped
            } else {
                TaskOutcome::Success
            }
        } else if snap.current_state.is_empty() {
            TaskOutcome::Pending
        } else {
            TaskOutcome::Running
        };
        let session_id = task_to_session_id
            .get(task_name)
            .cloned()
            .unwrap_or_else(|| task_name.clone());
        let mut entry = ChildGateEntry {
            name: session_id,
            state: snap.current_state.clone(),
            complete: snap.terminal,
            outcome,
            failure_mode: None,
            skipped_because: None,
            blocked_by: None,
            skipped_because_chain: Vec::new(),
            reason_source: None,
        };
        match outcome {
            TaskOutcome::Failure => {
                entry.failure_mode = Some(snap.current_state.clone());
                entry.reason_source = Some("state_name".to_string());
            }
            TaskOutcome::Skipped => {
                entry.reason_source = Some("skipped".to_string());
            }
            _ => {}
        }
        entries.push(entry);
    }
    entries
}

// --------- BatchFinalized helpers (Issue #17) ----------------------

/// Return the most recent `BatchFinalized` event in `events`, if any.
///
/// Walks the log in reverse so the first match is the latest
/// finalization. Consumers that want the frozen `batch_final_view`
/// always read from this event — prior `BatchFinalized` entries are
/// stale and will carry a `superseded_by` marker when rendered via
/// [`annotate_superseded_batch_finalized`].
pub fn find_most_recent_batch_finalized(events: &[Event]) -> Option<&Event> {
    events
        .iter()
        .rev()
        .find(|e| matches!(e.payload, EventPayload::BatchFinalized { .. }))
}

/// Decide whether a fresh `BatchFinalized` event should append for the
/// current tick (Issue #17 acceptance criterion 2).
///
/// Returns `true` only when:
///   - the `children-complete` gate output reports `all_complete: true`,
///   - AND either no prior `BatchFinalized` event exists, OR the last
///     `BatchFinalized` has been invalidated by a later event that
///     re-entered the batched state (a retry `EvidenceSubmitted` with
///     `retry_failed`, or a `Rewound` event on the parent).
///
/// The append-once-per-finalization guarantee falls out of this
/// predicate: a no-op re-tick observes the prior `BatchFinalized` with
/// no intervening retry event, so no new event appends.
pub fn should_append_batch_finalized(events: &[Event], all_complete: bool) -> bool {
    if !all_complete {
        return false;
    }
    // Find the seq of the most recent BatchFinalized, if any.
    let last_bf_seq: Option<u64> = events.iter().rev().find_map(|e| match &e.payload {
        EventPayload::BatchFinalized { .. } => Some(e.seq),
        _ => None,
    });
    match last_bf_seq {
        None => true, // no prior finalization — append fresh.
        Some(seq) => {
            // Append only if something after the prior BatchFinalized
            // invalidated it (a retry_failed evidence or a Rewound).
            events
                .iter()
                .filter(|e| e.seq > seq)
                .any(is_batch_invalidator)
        }
    }
}

/// An event counts as a batch invalidator when it re-enters the
/// batched state: either a retry_failed evidence submission on the
/// parent or any `Rewound` event (e.g., retry fast-path).
fn is_batch_invalidator(e: &Event) -> bool {
    match &e.payload {
        EventPayload::EvidenceSubmitted { fields, .. } => fields.contains_key("retry_failed"),
        EventPayload::Rewound { .. } => true,
        _ => false,
    }
}

/// Return a copy of `events` with every stale `BatchFinalized` payload
/// annotated with its `superseded_by` marker (Issue #17 Round-3
/// polish).
///
/// A `BatchFinalized` is stale when:
///   - a later `BatchFinalized` exists in the log (superseded_by →
///     the later finalization), OR
///   - a later invalidator (`retry_failed` evidence or a `Rewound`
///     event) appears after it without an intervening fresh
///     `BatchFinalized`.
///
/// The most recent `BatchFinalized` is only marked superseded when a
/// later invalidator exists — which is the design's "stale with phase
/// flipping back to active" case.
///
/// Events with other payload types are returned unchanged.
pub fn annotate_superseded_batch_finalized(events: &[Event]) -> Vec<Event> {
    // Build a list of (index, seq, type, timestamp) for every event
    // after which a BatchFinalized would be invalidated. Walking this
    // list forward lets us assign the FIRST superseding event to the
    // oldest stale BatchFinalized.
    let mut out = events.to_vec();
    let n = events.len();
    for i in 0..n {
        let EventPayload::BatchFinalized { .. } = &events[i].payload else {
            continue;
        };
        // Look for the first event after index i that either (a) is a
        // later BatchFinalized, or (b) is a retry/rewind invalidator.
        let mut sup: Option<&Event> = None;
        for later in events.iter().skip(i + 1) {
            if matches!(later.payload, EventPayload::BatchFinalized { .. })
                || is_batch_invalidator(later)
            {
                sup = Some(later);
                break;
            }
        }
        if let Some(later) = sup {
            if let EventPayload::BatchFinalized {
                state,
                view,
                timestamp,
                ..
            } = &events[i].payload
            {
                out[i].payload = EventPayload::BatchFinalized {
                    state: state.clone(),
                    view: view.clone(),
                    timestamp: timestamp.clone(),
                    superseded_by: Some(crate::engine::types::SupersededByRef {
                        seq: later.seq,
                        event_type: later.payload.type_name().to_string(),
                        timestamp: later.timestamp.clone(),
                    }),
                };
            }
        }
    }
    out
}

/// Return the derived `batch.phase` for the current state's event log
/// (Issue #17 Round-3 polish).
///
/// `"final"` once a `BatchFinalized` event has appended and is still
/// the most recent signal (not yet invalidated by a later retry). The
/// design treats the stale window where a retry has landed but no new
/// finalization has appended yet as `"final"` too — the event's
/// existence is load-bearing, not the parent's current state.
/// `"active"` otherwise.
pub fn derive_batch_phase(events: &[Event]) -> &'static str {
    if events
        .iter()
        .any(|e| matches!(e.payload, EventPayload::BatchFinalized { .. }))
    {
        "final"
    } else {
        "active"
    }
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
            skip_if: None,
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

    // --- children-complete gate output (Issue #15) ---------------------
    //
    // The following tests exercise the new aggregate counters, derived
    // booleans, per-child outcome projection, and skip-attribution via
    // the internal helpers (`build_entries_from_tasks`,
    // `build_entries_from_disk`) so they can run without a session
    // backend. End-to-end coverage (backend.list → full JSON) lives in
    // `tests/integration_test.rs`.

    /// Build a synthesized `ChildSnapshot` for a given `TaskOutcome`-ish
    /// state. Used by gate-output tests that don't need a real backend.
    fn snap(
        current_state: &str,
        terminal: bool,
        failure: bool,
        skipped_marker: bool,
    ) -> ChildSnapshot {
        ChildSnapshot {
            current_state: current_state.to_string(),
            terminal,
            failure,
            skipped_marker,
            spawn_entry: None,
        }
    }

    /// Aggregate snapshot returned by [`aggregates`] for the gate
    /// output tests below. Fields mirror the JSON gate output.
    #[derive(Debug)]
    struct GateAgg {
        total: usize,
        success: usize,
        failed: usize,
        skipped: usize,
        pending: usize,
        blocked: usize,
        spawn_failed: usize,
        all_complete: bool,
        all_success: bool,
        any_failed: bool,
        any_skipped: bool,
        any_spawn_failed: bool,
        needs_attention: bool,
    }

    /// Compute aggregates + derived booleans the same way
    /// `build_children_complete_output` does, given a pre-built list
    /// of entries.
    fn aggregates(entries: &mut [ChildGateEntry]) -> GateAgg {
        let total = entries.len();
        let mut success = 0;
        let mut failed = 0;
        let mut skipped = 0;
        let mut pending = 0;
        let mut blocked = 0;
        let mut spawn_failed = 0;
        for e in entries.iter_mut() {
            if matches!(e.outcome, TaskOutcome::Running) {
                e.outcome = TaskOutcome::Pending;
            }
            match e.outcome {
                TaskOutcome::Success => success += 1,
                TaskOutcome::Failure => failed += 1,
                TaskOutcome::Skipped => skipped += 1,
                TaskOutcome::Pending => pending += 1,
                TaskOutcome::Blocked => blocked += 1,
                TaskOutcome::SpawnFailed => spawn_failed += 1,
                TaskOutcome::Running => unreachable!(),
            }
        }
        let all_complete = total > 0 && pending == 0 && blocked == 0 && spawn_failed == 0;
        let all_success = all_complete && failed == 0 && skipped == 0 && spawn_failed == 0;
        let any_failed = failed > 0;
        let any_skipped = skipped > 0;
        let any_spawn_failed = spawn_failed > 0;
        let needs_attention = any_failed || any_skipped || any_spawn_failed;
        GateAgg {
            total,
            success,
            failed,
            skipped,
            pending,
            blocked,
            spawn_failed,
            all_complete,
            all_success,
            any_failed,
            any_skipped,
            any_spawn_failed,
            needs_attention,
        }
    }

    #[test]
    fn gate_output_all_success_sets_all_complete_and_all_success() {
        // Three tasks, all terminal-success on disk.
        let tasks = vec![task("a", &[]), task("b", &[]), task("c", &[])];
        let mut on_disk: HashMap<String, ChildSnapshot> = HashMap::new();
        for name in ["a", "b", "c"] {
            on_disk.insert(name.to_string(), snap("done", true, false, false));
        }
        let compiled = CompiledTemplate {
            format_version: 1,
            name: "p".to_string(),
            version: "1".to_string(),
            description: String::new(),
            initial_state: "s".to_string(),
            variables: BTreeMap::new(),
            states: BTreeMap::new(),
        };
        let (mut entries, _err) = build_entries_from_tasks(
            &tasks,
            &on_disk,
            "parent",
            FailurePolicy::SkipDependents,
            &[],
            &compiled,
        );
        let agg = aggregates(&mut entries);
        assert_eq!(agg.total, 3);
        assert_eq!(agg.success, 3);
        assert!(agg.all_complete);
        assert!(agg.all_success);
        assert!(!agg.any_failed);
        assert!(!agg.needs_attention);
        assert!(entries
            .iter()
            .all(|e| matches!(e.outcome, TaskOutcome::Success)));
    }

    #[test]
    fn gate_output_mixed_success_failed_skipped_blocked() {
        // Chain: a (success) -> b (failure) -> c (skipped, via b fail) ;
        // plus d with waits_on=[e] where e is still running so d is
        // blocked.
        let tasks = vec![
            task("a", &[]),
            task("b", &["a"]),
            task("c", &["b"]),
            task("d", &["e"]),
            task("e", &[]),
        ];
        let mut on_disk: HashMap<String, ChildSnapshot> = HashMap::new();
        on_disk.insert("a".to_string(), snap("done", true, false, false));
        on_disk.insert("b".to_string(), snap("failed", true, true, false));
        // c and d not spawned yet; e is running (non-terminal).
        on_disk.insert("e".to_string(), snap("work", false, false, false));

        let compiled = CompiledTemplate {
            format_version: 1,
            name: "p".to_string(),
            version: "1".to_string(),
            description: String::new(),
            initial_state: "s".to_string(),
            variables: BTreeMap::new(),
            states: BTreeMap::new(),
        };
        let (mut entries, _) = build_entries_from_tasks(
            &tasks,
            &on_disk,
            "parent",
            FailurePolicy::SkipDependents,
            &[],
            &compiled,
        );
        let agg = aggregates(&mut entries);
        // total=5: a success, b failure, c ShouldBeSkipped → skipped,
        // d blocked (waits on Running e), e Running → pending.
        assert_eq!(agg.total, 5);
        assert_eq!(agg.success, 1, "success");
        assert_eq!(agg.failed, 1, "failed");
        assert_eq!(agg.skipped, 1, "skipped");
        assert_eq!(agg.pending, 1, "pending (running e folds in)");
        assert_eq!(agg.blocked, 1, "blocked (d)");
        assert_eq!(agg.spawn_failed, 0, "spawn_failed");
        assert!(
            !agg.all_complete,
            "all_complete should be false (pending/blocked)"
        );
        assert!(
            !agg.all_success,
            "all_success should be false (has failures)"
        );
        assert!(agg.any_failed, "any_failed");
        assert!(agg.any_skipped, "any_skipped");
        assert!(!agg.any_spawn_failed, "any_spawn_failed");
        assert!(agg.needs_attention, "needs_attention");
    }

    #[test]
    fn gate_output_spawn_failed_blocks_all_complete() {
        // Synthesize an entry with TaskOutcome::SpawnFailed directly
        // (the classification path doesn't naturally produce it — the
        // scheduler does — but the aggregator must treat it correctly).
        let mut entries = vec![
            ChildGateEntry {
                name: "parent.a".to_string(),
                state: "done".to_string(),
                complete: true,
                outcome: TaskOutcome::Success,
                failure_mode: None,
                skipped_because: None,
                blocked_by: None,
                skipped_because_chain: Vec::new(),
                reason_source: None,
            },
            ChildGateEntry {
                name: "parent.b".to_string(),
                state: String::new(),
                complete: false,
                outcome: TaskOutcome::SpawnFailed,
                failure_mode: None,
                skipped_because: None,
                blocked_by: None,
                skipped_because_chain: Vec::new(),
                reason_source: Some("not_spawned".to_string()),
            },
        ];
        let agg = aggregates(&mut entries);
        assert_eq!(agg.spawn_failed, 1, "spawn_failed count");
        assert!(
            !agg.all_complete,
            "all_complete must be false when spawn_failed > 0"
        );
        assert!(agg.any_spawn_failed, "any_spawn_failed");
        assert!(
            agg.needs_attention,
            "needs_attention folds in any_spawn_failed"
        );
    }

    #[test]
    fn gate_output_needs_attention_on_any_failed() {
        let mut entries = vec![
            ChildGateEntry {
                name: "parent.a".to_string(),
                state: "done".to_string(),
                complete: true,
                outcome: TaskOutcome::Success,
                failure_mode: None,
                skipped_because: None,
                blocked_by: None,
                skipped_because_chain: Vec::new(),
                reason_source: None,
            },
            ChildGateEntry {
                name: "parent.b".to_string(),
                state: "failed".to_string(),
                complete: true,
                outcome: TaskOutcome::Failure,
                failure_mode: Some("failed".to_string()),
                skipped_because: None,
                blocked_by: None,
                skipped_because_chain: Vec::new(),
                reason_source: Some("state_name".to_string()),
            },
        ];
        let agg = aggregates(&mut entries);
        assert!(agg.all_complete, "all_complete true (all terminal)");
        assert!(!agg.all_success, "all_success false (one failed)");
        assert!(agg.any_failed, "any_failed");
        assert!(agg.needs_attention, "needs_attention");
    }

    #[test]
    fn gate_output_diamond_skip_chain_collects_both_failed_ancestors() {
        // Submission order: A, B, C, D, E.
        //   A -> C, B -> D, C -> E, D -> E.
        // A fails, B fails (earliest-in-submission-order is A, but E's
        // direct waits_on are C and D — both skipped because of
        // failed ancestors A/B). The chain for E must contain both
        // A and B (unique failed ancestors) in topological order.
        let tasks = vec![
            task("A", &[]),
            task("B", &[]),
            task("C", &["A"]),
            task("D", &["B"]),
            task("E", &["C", "D"]),
        ];
        let mut on_disk: HashMap<String, ChildSnapshot> = HashMap::new();
        on_disk.insert("A".to_string(), snap("failed", true, true, false));
        on_disk.insert("B".to_string(), snap("failed", true, true, false));
        let compiled = CompiledTemplate {
            format_version: 1,
            name: "p".to_string(),
            version: "1".to_string(),
            description: String::new(),
            initial_state: "s".to_string(),
            variables: BTreeMap::new(),
            states: BTreeMap::new(),
        };
        let (entries, _) = build_entries_from_tasks(
            &tasks,
            &on_disk,
            "parent",
            FailurePolicy::SkipDependents,
            &[],
            &compiled,
        );
        let e_entry = entries
            .iter()
            .find(|e| e.name == "parent.E")
            .expect("E entry present");
        assert!(matches!(e_entry.outcome, TaskOutcome::Skipped));
        // Both root failures A and B appear in the chain.
        assert!(
            e_entry
                .skipped_because_chain
                .contains(&"parent.A".to_string()),
            "chain must contain parent.A: {:?}",
            e_entry.skipped_because_chain
        );
        assert!(
            e_entry
                .skipped_because_chain
                .contains(&"parent.B".to_string()),
            "chain must contain parent.B: {:?}",
            e_entry.skipped_because_chain
        );
        assert_eq!(e_entry.reason_source.as_deref(), Some("skipped"));
    }

    #[test]
    fn gate_output_blocked_by_lists_non_terminal_deps() {
        // a is not terminal (still running); b depends on a; b should
        // be Blocked with blocked_by = ["parent.a"].
        let tasks = vec![task("a", &[]), task("b", &["a"])];
        let mut on_disk: HashMap<String, ChildSnapshot> = HashMap::new();
        on_disk.insert("a".to_string(), snap("work", false, false, false));
        let compiled = CompiledTemplate {
            format_version: 1,
            name: "p".to_string(),
            version: "1".to_string(),
            description: String::new(),
            initial_state: "s".to_string(),
            variables: BTreeMap::new(),
            states: BTreeMap::new(),
        };
        let (entries, _) = build_entries_from_tasks(
            &tasks,
            &on_disk,
            "parent",
            FailurePolicy::SkipDependents,
            &[],
            &compiled,
        );
        let b = entries
            .iter()
            .find(|e| e.name == "parent.b")
            .expect("b entry present");
        assert!(matches!(b.outcome, TaskOutcome::Blocked));
        assert_eq!(
            b.blocked_by.as_deref(),
            Some(&["parent.a".to_string()][..]),
            "blocked_by names the composed upstream"
        );
    }

    #[test]
    fn gate_output_skipped_because_direct_vs_chain_distinction() {
        // A → B → C. A fails, B skipped, C skipped.
        // - B: skipped_because = parent.A, chain = [parent.A].
        // - C: skipped_because = parent.B (direct), chain contains
        //   parent.A (root failure ancestor).
        let tasks = vec![task("A", &[]), task("B", &["A"]), task("C", &["B"])];
        let mut on_disk: HashMap<String, ChildSnapshot> = HashMap::new();
        on_disk.insert("A".to_string(), snap("failed", true, true, false));
        let compiled = CompiledTemplate {
            format_version: 1,
            name: "p".to_string(),
            version: "1".to_string(),
            description: String::new(),
            initial_state: "s".to_string(),
            variables: BTreeMap::new(),
            states: BTreeMap::new(),
        };
        let (entries, _) = build_entries_from_tasks(
            &tasks,
            &on_disk,
            "parent",
            FailurePolicy::SkipDependents,
            &[],
            &compiled,
        );
        let b = entries
            .iter()
            .find(|e| e.name == "parent.B")
            .expect("B entry present");
        assert_eq!(b.skipped_because.as_deref(), Some("parent.A"));
        assert_eq!(b.skipped_because_chain, vec!["parent.A".to_string()]);

        let c = entries
            .iter()
            .find(|e| e.name == "parent.C")
            .expect("C entry present");
        // Direct blocker is B (the waits_on of C), but the chain
        // walks through the skipped B to reach the root failure A.
        assert_eq!(c.skipped_because.as_deref(), Some("parent.B"));
        assert!(
            c.skipped_because_chain.contains(&"parent.A".to_string()),
            "chain walks upstream through skipped ancestors to the root failure: {:?}",
            c.skipped_because_chain
        );
    }

    #[test]
    fn gate_output_reason_source_vocabulary_complete() {
        // Each of the four reason_source enum values lands in at least
        // one per-child entry under the right conditions.
        // - failed: state_name
        // - skipped: skipped
        // - spawn_failed: not_spawned
        // - (failure_reason variant is populated by the scheduler/
        //   batch view path, not the gate-output path in v1; the
        //   vocabulary is pinned by the design so agents can route on
        //   it deterministically.)
        let entry_state_name = ChildGateEntry {
            name: "p.a".to_string(),
            state: "failed".to_string(),
            complete: true,
            outcome: TaskOutcome::Failure,
            failure_mode: Some("failed".to_string()),
            skipped_because: None,
            blocked_by: None,
            skipped_because_chain: Vec::new(),
            reason_source: Some("state_name".to_string()),
        };
        let entry_skipped = ChildGateEntry {
            name: "p.b".to_string(),
            state: "skipped".to_string(),
            complete: true,
            outcome: TaskOutcome::Skipped,
            failure_mode: None,
            skipped_because: Some("p.a".to_string()),
            blocked_by: None,
            skipped_because_chain: vec!["p.a".to_string()],
            reason_source: Some("skipped".to_string()),
        };
        let entry_not_spawned = ChildGateEntry {
            name: "p.c".to_string(),
            state: String::new(),
            complete: false,
            outcome: TaskOutcome::SpawnFailed,
            failure_mode: None,
            skipped_because: None,
            blocked_by: None,
            skipped_because_chain: Vec::new(),
            reason_source: Some("not_spawned".to_string()),
        };
        for (entry, want) in [
            (&entry_state_name, "state_name"),
            (&entry_skipped, "skipped"),
            (&entry_not_spawned, "not_spawned"),
        ] {
            let json = child_entry_to_json(entry);
            assert_eq!(
                json["reason_source"].as_str(),
                Some(want),
                "reason_source projection for {}",
                entry.name
            );
        }
        // The failure_reason variant is a documented value the scheduler
        // path emits; verify its JSON round-trip through ChildGateEntry.
        let entry_failure_reason = ChildGateEntry {
            reason_source: Some("failure_reason".to_string()),
            ..entry_state_name.clone()
        };
        let j = child_entry_to_json(&entry_failure_reason);
        assert_eq!(j["reason_source"].as_str(), Some("failure_reason"));
    }

    #[test]
    fn task_outcome_spawn_failed_round_trip() {
        // Issue #15 AC3: ChildOutcome/TaskOutcome enum round-trips
        // `spawn_failed` through serde.
        let outcome = TaskOutcome::SpawnFailed;
        let s = serde_json::to_string(&outcome).unwrap();
        assert_eq!(s, "\"spawn_failed\"");
        let back: TaskOutcome = serde_json::from_str(&s).unwrap();
        assert_eq!(back, TaskOutcome::SpawnFailed);
    }

    #[test]
    fn gate_output_fallback_from_disk_no_hook() {
        // With no hook (no task list), children are enumerated from
        // disk. Non-terminal children project to Pending (legacy
        // behavior); terminal-success to Success; failure/skipped
        // templates to Failure/Skipped.
        let order = vec!["x".to_string(), "y".to_string(), "z".to_string()];
        let mut on_disk: HashMap<String, ChildSnapshot> = HashMap::new();
        on_disk.insert("x".to_string(), snap("done", true, false, false));
        on_disk.insert("y".to_string(), snap("failed", true, true, false));
        on_disk.insert("z".to_string(), snap("work", false, false, false));
        let task_to_session: HashMap<String, String> =
            order.iter().map(|n| (n.clone(), n.clone())).collect();
        let entries = build_entries_from_disk(&order, &on_disk, &task_to_session);
        assert_eq!(entries.len(), 3);
        let x = entries.iter().find(|e| e.name == "x").unwrap();
        assert!(matches!(x.outcome, TaskOutcome::Success));
        let y = entries.iter().find(|e| e.name == "y").unwrap();
        assert!(matches!(y.outcome, TaskOutcome::Failure));
        assert_eq!(y.failure_mode.as_deref(), Some("failed"));
        assert_eq!(y.reason_source.as_deref(), Some("state_name"));
        let z = entries.iter().find(|e| e.name == "z").unwrap();
        // Running collapses to Pending in the wire-level aggregation
        // step; the pre-aggregation outcome is Running here.
        assert!(matches!(z.outcome, TaskOutcome::Running));
    }

    // --------- Issue #16: SchedulerOutcome JSON round-trip and shape
    //                      snapshot tests
    // -----------------------------------------------------------------

    #[test]
    fn scheduler_outcome_json_round_trip_with_all_new_fields() {
        // Build a SchedulerOutcome::Scheduled with every field
        // populated so the snapshot pins the canonical wire shape.
        let mut entries: BTreeMap<String, EntryOutcome> = BTreeMap::new();
        entries.insert("task-a".to_string(), EntryOutcome::Accepted);
        entries.insert("task-b".to_string(), EntryOutcome::AlreadyRunning);
        entries.insert(
            "task-c".to_string(),
            EntryOutcome::Blocked {
                waits_on: vec!["task-a".to_string()],
            },
        );
        entries.insert(
            "task-d".to_string(),
            EntryOutcome::Errored {
                kind: "io_error".to_string(),
            },
        );
        entries.insert("task-e".to_string(), EntryOutcome::Respawning);
        entries.insert("task-f".to_string(), EntryOutcome::AlreadyTerminalSuccess);
        entries.insert("task-g".to_string(), EntryOutcome::AlreadyTerminalFailure);
        entries.insert("task-h".to_string(), EntryOutcome::AlreadySkipped);
        let feedback = SchedulerFeedback {
            entries,
            orphan_candidates: vec![OrphanCandidate {
                name: "ghost".to_string(),
                reason: "not in current task list".to_string(),
            }],
        };
        let outcome = SchedulerOutcome::Scheduled {
            spawned_this_tick: vec!["parent.task-a".to_string()],
            materialized_children: vec![MaterializedChild {
                name: "parent.task-a".to_string(),
                task: "task-a".to_string(),
                outcome: TaskOutcome::Running,
                state: Some("work".to_string()),
                waits_on: vec![],
                ready_to_drive: true,
                role: Some(ChildRole::Worker),
                subbatch_status: None,
            }],
            errored: vec![],
            warnings: vec![],
            reclassified_this_tick: true,
            feedback,
        };
        // Round-trip through JSON.
        let json = serde_json::to_string(&outcome).expect("serialize");
        let back: SchedulerOutcome = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(outcome, back);
        // Pin wire-level expectations: new fields are present.
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["kind"], "scheduled");
        assert!(v["spawned_this_tick"].is_array());
        assert!(v["materialized_children"].is_array());
        assert!(v["feedback"].is_object());
        assert!(v["feedback"]["entries"].is_object());
        assert!(v["feedback"]["orphan_candidates"].is_array());
        assert_eq!(
            v["feedback"]["entries"]["task-a"]["outcome"], "accepted",
            "accepted variant serializes as tagged outcome"
        );
        assert_eq!(v["feedback"]["entries"]["task-c"]["outcome"], "blocked");
        assert_eq!(v["feedback"]["entries"]["task-c"]["waits_on"][0], "task-a");
        assert_eq!(v["feedback"]["entries"]["task-d"]["kind"], "io_error");
        assert_eq!(v["feedback"]["orphan_candidates"][0]["name"], "ghost");
        assert_eq!(
            v["feedback"]["orphan_candidates"][0]["reason"],
            "not in current task list"
        );
        // MaterializedChild.role + subbatch_status serialize correctly.
        let mc0 = &v["materialized_children"][0];
        assert_eq!(mc0["role"], "worker");
        // subbatch_status: None is omitted.
        assert!(mc0.get("subbatch_status").is_none());
    }

    #[test]
    fn orphan_candidate_serializes_with_name_and_reason() {
        let oc = OrphanCandidate {
            name: "leftover".to_string(),
            reason: "not in current task list".to_string(),
        };
        let v = serde_json::to_value(&oc).unwrap();
        assert_eq!(v["name"], "leftover");
        assert_eq!(v["reason"], "not in current task list");
    }

    #[test]
    fn child_role_serializes_snake_case() {
        assert_eq!(
            serde_json::to_value(ChildRole::Worker).unwrap(),
            serde_json::json!("worker")
        );
        assert_eq!(
            serde_json::to_value(ChildRole::Coordinator).unwrap(),
            serde_json::json!("coordinator")
        );
    }

    #[test]
    fn batch_summary_serializes_four_counts() {
        let s = BatchSummary {
            success: 2,
            failed: 1,
            skipped: 0,
            pending: 3,
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["success"], 2);
        assert_eq!(v["failed"], 1);
        assert_eq!(v["skipped"], 0);
        assert_eq!(v["pending"], 3);
    }

    #[test]
    fn scheduler_tick_summary_round_trips() {
        use crate::engine::types::SchedulerTickSummary;
        let s = SchedulerTickSummary {
            spawned_count: 3,
            errored_count: 1,
            skipped_count: 2,
            reclassified: true,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: SchedulerTickSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn scheduler_ran_event_round_trips() {
        use crate::engine::types::{Event, EventPayload, SchedulerTickSummary};
        let ev = Event {
            seq: 42,
            timestamp: "2026-04-14T00:00:00Z".to_string(),
            event_type: "scheduler_ran".to_string(),
            payload: EventPayload::SchedulerRan {
                state: "dispatch".to_string(),
                tick_summary: SchedulerTickSummary {
                    spawned_count: 1,
                    errored_count: 0,
                    skipped_count: 0,
                    reclassified: true,
                },
                timestamp: "2026-04-14T00:00:00Z".to_string(),
            },
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(
            json.contains("\"type\":\"scheduler_ran\""),
            "event type string must match: {}",
            json
        );
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn build_feedback_entries_returns_one_per_task_in_submission() {
        // Four tasks: a spawned this tick, b blocked, c already
        // running (on disk, non-terminal), d errored.
        let tasks = vec![
            task("a", &[]),
            task("b", &["a"]),
            task("c", &[]),
            task("d", &[]),
        ];
        let mut classifications: HashMap<String, TaskClassification> = HashMap::new();
        classifications.insert("a".to_string(), TaskClassification::Running);
        classifications.insert("b".to_string(), TaskClassification::BlockedByDep);
        classifications.insert("c".to_string(), TaskClassification::Running);
        classifications.insert("d".to_string(), TaskClassification::Failure);
        // snapshots: 'c' exists on disk non-terminal.
        let mut snapshots: HashMap<String, ChildSnapshot> = HashMap::new();
        snapshots.insert(
            "c".to_string(),
            ChildSnapshot {
                current_state: "work".to_string(),
                terminal: false,
                failure: false,
                skipped_marker: false,
                spawn_entry: None,
            },
        );
        let errored = vec![TaskSpawnError::new(
            "parent.d",
            crate::cli::task_spawn_error::SpawnErrorKind::IoError,
            "boom",
        )];
        let spawned = vec!["parent.a".to_string()];
        let out = build_feedback_entries(
            &tasks,
            &classifications,
            &snapshots,
            &errored,
            &spawned,
            "parent",
        );
        assert_eq!(out.len(), 4);
        assert!(matches!(out.get("a"), Some(EntryOutcome::Accepted)));
        assert!(matches!(out.get("b"), Some(EntryOutcome::Blocked { .. })));
        assert!(matches!(out.get("c"), Some(EntryOutcome::AlreadyRunning)));
        match out.get("d") {
            Some(EntryOutcome::Errored { kind }) => assert_eq!(kind, "io_error"),
            other => panic!("expected Errored, got {:?}", other),
        }
    }

    // --------- Issue #17: BatchFinalized helpers ---------------------

    fn bf_event(seq: u64, ts: &str) -> Event {
        Event {
            seq,
            timestamp: ts.to_string(),
            event_type: "batch_finalized".to_string(),
            payload: EventPayload::BatchFinalized {
                state: "plan".to_string(),
                view: serde_json::json!({}),
                timestamp: ts.to_string(),
                superseded_by: None,
            },
        }
    }

    fn retry_evidence_event(seq: u64, ts: &str) -> Event {
        let mut fields: HashMap<String, serde_json::Value> = HashMap::new();
        fields.insert(
            "retry_failed".to_string(),
            serde_json::json!({"children": ["A"]}),
        );
        Event {
            seq,
            timestamp: ts.to_string(),
            event_type: "evidence_submitted".to_string(),
            payload: EventPayload::EvidenceSubmitted {
                state: "plan".to_string(),
                fields,
                submitter_cwd: None,
            },
        }
    }

    #[test]
    fn should_append_batch_finalized_when_no_prior_event() {
        let events: Vec<Event> = vec![];
        assert!(should_append_batch_finalized(&events, true));
        // all_complete false short-circuits.
        assert!(!should_append_batch_finalized(&events, false));
    }

    #[test]
    fn should_not_append_batch_finalized_when_prior_not_invalidated() {
        let events = vec![bf_event(5, "2026-04-14T10:00:00Z")];
        assert!(!should_append_batch_finalized(&events, true));
    }

    #[test]
    fn should_append_batch_finalized_when_retry_intervened() {
        let events = vec![
            bf_event(5, "2026-04-14T10:00:00Z"),
            retry_evidence_event(7, "2026-04-14T10:05:00Z"),
        ];
        assert!(should_append_batch_finalized(&events, true));
    }

    #[test]
    fn find_most_recent_batch_finalized_returns_latest() {
        let events = vec![
            bf_event(5, "2026-04-14T10:00:00Z"),
            retry_evidence_event(7, "2026-04-14T10:05:00Z"),
            bf_event(12, "2026-04-14T10:10:00Z"),
        ];
        let latest = find_most_recent_batch_finalized(&events).expect("latest exists");
        assert_eq!(latest.seq, 12);
    }

    #[test]
    fn annotate_superseded_by_marks_prior_batch_finalized() {
        // Scenario: BatchFinalized (seq 5) → retry evidence (seq 7)
        // → BatchFinalized (seq 12). The first BatchFinalized should
        // be annotated with superseded_by pointing at seq 7 (the
        // retry), which is the FIRST invalidator after it. The second
        // BatchFinalized is still the most recent and no invalidator
        // follows it, so it stays unmarked.
        let events = vec![
            bf_event(5, "2026-04-14T10:00:00Z"),
            retry_evidence_event(7, "2026-04-14T10:05:00Z"),
            bf_event(12, "2026-04-14T10:10:00Z"),
        ];
        let annotated = annotate_superseded_batch_finalized(&events);
        match &annotated[0].payload {
            EventPayload::BatchFinalized { superseded_by, .. } => {
                let sup = superseded_by.as_ref().expect("prior BF must be marked");
                assert_eq!(sup.seq, 7);
                assert_eq!(sup.event_type, "evidence_submitted");
            }
            other => panic!("expected BatchFinalized, got {:?}", other),
        }
        match &annotated[2].payload {
            EventPayload::BatchFinalized { superseded_by, .. } => {
                assert!(
                    superseded_by.is_none(),
                    "most recent BatchFinalized must not be superseded"
                );
            }
            other => panic!("expected BatchFinalized, got {:?}", other),
        }
    }

    #[test]
    fn annotate_superseded_by_marks_batch_finalized_when_later_invalidator_exists() {
        // Stale window: BatchFinalized (seq 5) followed by a retry
        // (seq 7) but no new BatchFinalized yet. The event is still
        // stale; `superseded_by` points at the retry.
        let events = vec![
            bf_event(5, "2026-04-14T10:00:00Z"),
            retry_evidence_event(7, "2026-04-14T10:05:00Z"),
        ];
        let annotated = annotate_superseded_batch_finalized(&events);
        match &annotated[0].payload {
            EventPayload::BatchFinalized { superseded_by, .. } => {
                let sup = superseded_by.as_ref().expect("stale BF must be marked");
                assert_eq!(sup.seq, 7);
            }
            other => panic!("expected BatchFinalized, got {:?}", other),
        }
    }

    #[test]
    fn derive_batch_phase_active_when_no_batch_finalized() {
        let events: Vec<Event> = vec![];
        assert_eq!(derive_batch_phase(&events), "active");
    }

    #[test]
    fn derive_batch_phase_final_once_batch_finalized_appended() {
        let events = vec![bf_event(5, "2026-04-14T10:00:00Z")];
        assert_eq!(derive_batch_phase(&events), "final");
        // Sticky across retries: phase stays final even after a retry
        // invalidates the prior finalization (design Decision 13).
        let events_with_retry = vec![
            bf_event(5, "2026-04-14T10:00:00Z"),
            retry_evidence_event(7, "2026-04-14T10:05:00Z"),
        ];
        assert_eq!(derive_batch_phase(&events_with_retry), "final");
    }

    #[test]
    fn batch_final_view_from_gate_output_round_trip() {
        // Serialized gate output round-trips through BatchFinalView.
        let gate_output = serde_json::json!({
            "total": 2,
            "completed": 2,
            "pending": 0,
            "success": 1,
            "failed": 1,
            "skipped": 0,
            "blocked": 0,
            "spawn_failed": 0,
            "all_complete": true,
            "all_success": false,
            "any_failed": true,
            "any_skipped": false,
            "any_spawn_failed": false,
            "needs_attention": true,
            "children": [],
        });
        let view = BatchFinalView::from_gate_output(&gate_output).expect("parses");
        assert_eq!(view.total, 2);
        assert_eq!(view.completed, 2);
        assert_eq!(view.failed, 1);
        assert!(view.all_complete);
        assert!(!view.all_success);
        assert!(view.any_failed);
        // Serialize → deserialize round-trip.
        let json = serde_json::to_string(&view).unwrap();
        let back: BatchFinalView = serde_json::from_str(&json).unwrap();
        assert_eq!(view, back);
    }

    // --- Issue #20: repair_half_initialized_children + tempfile sweep ---
    //
    // These tests exercise `sweep_stale_tempfiles` and
    // `repair_half_initialized_children` directly against a
    // `LocalBackend` on a tempdir. The sweep helper takes `now` as an
    // explicit parameter so tests can simulate age windows without
    // touching filesystem mtimes.

    use crate::engine::persistence;
    use crate::engine::types::StateFileHeader;
    use crate::session::state_file_name;
    use tempfile::TempDir;

    /// Write a header-only state file for `id` under `base_dir`.
    /// Produces the "half-initialised" shape: the header line is
    /// present but no events follow.
    fn write_half_init_state_file(base_dir: &std::path::Path, id: &str) {
        let session_dir = base_dir.join(id);
        std::fs::create_dir_all(&session_dir).unwrap();
        let state_path = session_dir.join(state_file_name(id));
        let header = StateFileHeader {
            schema_version: 1,
            workflow: id.to_string(),
            template_hash: "testhash".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            parent_workflow: Some("p".to_string()),
            template_source_dir: None,
        };
        persistence::append_header(&state_path, &header).unwrap();
    }

    #[test]
    fn sweep_keeps_tempfile_younger_than_threshold() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let young = dir.join(".koto-init-young.tmp");
        std::fs::write(&young, b"pending").unwrap();

        // `now` equals the file's mtime ± a few ms -> age is well
        // under 60 seconds. The file must survive.
        sweep_stale_tempfiles(dir, SystemTime::now());
        assert!(
            young.exists(),
            "young tempfile must not be swept (< 60s old)"
        );
    }

    #[test]
    fn sweep_removes_tempfile_older_than_threshold() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let old = dir.join(".koto-init-old.tmp");
        std::fs::write(&old, b"stale from crash").unwrap();

        // Advance `now` by 61 seconds to push the file over the
        // threshold without touching filesystem mtimes. This matches
        // what would happen naturally after a minute of uptime.
        let future_now = SystemTime::now() + Duration::from_secs(61);
        sweep_stale_tempfiles(dir, future_now);
        assert!(
            !old.exists(),
            "tempfile older than 60s must be removed by the sweep"
        );
    }

    #[test]
    fn sweep_ignores_non_koto_tempfiles() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let unrelated = dir.join("random.tmp");
        std::fs::write(&unrelated, b"not ours").unwrap();
        let also_unrelated = dir.join(".koto-something-else.json");
        std::fs::write(&also_unrelated, b"wrong suffix").unwrap();

        let future_now = SystemTime::now() + Duration::from_secs(300);
        sweep_stale_tempfiles(dir, future_now);

        assert!(
            unrelated.exists(),
            "files without the .koto- prefix must not be swept"
        );
        assert!(
            also_unrelated.exists(),
            "files without the .tmp suffix must not be swept"
        );
    }

    #[test]
    fn sweep_handles_missing_directory_silently() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("does-not-exist");
        // Must not panic on a non-existent directory.
        sweep_stale_tempfiles(&missing, SystemTime::now());
    }

    #[test]
    fn repair_deletes_half_initialized_child_state_file() {
        let tmp = TempDir::new().unwrap();
        let backend = crate::session::local::LocalBackend::with_base_dir(tmp.path().to_path_buf());

        // Pre-Phase-1-style crash: parent spawned child "a" but
        // crashed after writing the header and before any event. The
        // session directory has a header-only state file.
        write_half_init_state_file(tmp.path(), "p.a");
        assert!(backend.exists("p.a"));
        let (_, events) = backend.read_events("p.a").unwrap();
        assert!(events.is_empty(), "precondition: header but no events");

        repair_half_initialized_children(&backend, "p", &["a"]);

        // After repair, the half-init child is gone. The scheduler
        // will classify the task as unspawned and re-initialise it on
        // the same tick via the normal Ready path.
        assert!(
            !backend.exists("p.a"),
            "half-init child must be deleted by repair"
        );
    }

    #[test]
    fn repair_preserves_fully_initialized_child_state_file() {
        let tmp = TempDir::new().unwrap();
        let backend = crate::session::local::LocalBackend::with_base_dir(tmp.path().to_path_buf());

        // Write a header AND an event so the file looks like a
        // successfully-initialised child.
        let session_dir = tmp.path().join("p.b");
        std::fs::create_dir_all(&session_dir).unwrap();
        let state_path = session_dir.join(state_file_name("p.b"));
        let header = StateFileHeader {
            schema_version: 1,
            workflow: "p.b".to_string(),
            template_hash: "h".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            parent_workflow: Some("p".to_string()),
            template_source_dir: None,
        };
        persistence::append_header(&state_path, &header).unwrap();
        // Append a WorkflowInitialized event so the file is
        // non-empty on the event dimension.
        backend
            .append_event(
                "p.b",
                &EventPayload::WorkflowInitialized {
                    template_path: "child.md".to_string(),
                    variables: std::collections::HashMap::new(),
                    spawn_entry: None,
                },
                "2026-01-01T00:00:01Z",
            )
            .unwrap();

        repair_half_initialized_children(&backend, "p", &["b"]);

        assert!(
            backend.exists("p.b"),
            "fully-initialised child must NOT be deleted"
        );
        let (_, events) = backend.read_events("p.b").unwrap();
        assert_eq!(events.len(), 1, "event must still be present");
    }

    #[test]
    fn repair_sweeps_tempfiles_in_parent_and_child_dirs() {
        let tmp = TempDir::new().unwrap();
        let backend = crate::session::local::LocalBackend::with_base_dir(tmp.path().to_path_buf());

        // Parent session dir with a stale tempfile (simulate via age
        // by patching mtime). Also drop one in a submitted child's
        // session dir so we prove the sweep walks both scopes.
        std::fs::create_dir_all(tmp.path().join("p")).unwrap();
        std::fs::create_dir_all(tmp.path().join("p.c")).unwrap();
        let parent_tmp = tmp.path().join("p").join(".koto-init-parent.tmp");
        let child_tmp = tmp.path().join("p.c").join(".koto-init-child.tmp");
        std::fs::write(&parent_tmp, b"stale").unwrap();
        std::fs::write(&child_tmp, b"stale").unwrap();

        // Age the files out of the protection window by setting their
        // mtime to well in the past. `set_modified` is stable since
        // Rust 1.75 and lets us do this without pulling in filetime.
        let long_ago = SystemTime::now() - Duration::from_secs(300);
        std::fs::File::options()
            .write(true)
            .open(&parent_tmp)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        std::fs::File::options()
            .write(true)
            .open(&child_tmp)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();

        repair_half_initialized_children(&backend, "p", &["c"]);

        assert!(
            !parent_tmp.exists(),
            "stale tempfile in parent dir must be swept"
        );
        assert!(
            !child_tmp.exists(),
            "stale tempfile in child dir must be swept"
        );
    }

    #[test]
    fn repair_leaves_fresh_tempfiles_alone_during_concurrent_init() {
        let tmp = TempDir::new().unwrap();
        let backend = crate::session::local::LocalBackend::with_base_dir(tmp.path().to_path_buf());

        // A concurrent `init_state_file` in a sibling tick is
        // represented by a very-recent `.koto-init-*.tmp` in the
        // child's session dir. The sweep must leave it alone.
        std::fs::create_dir_all(tmp.path().join("p.d")).unwrap();
        let in_flight = tmp.path().join("p.d").join(".koto-init-in-flight.tmp");
        std::fs::write(&in_flight, b"mid-write").unwrap();

        repair_half_initialized_children(&backend, "p", &["d"]);

        assert!(
            in_flight.exists(),
            "in-flight tempfile must survive the sweep (< 60s old)"
        );
    }

    // --------- Issue #134: ChildCompleted snapshot augmentation --------

    /// Helper: build a `ChildCompleted` event for tests. `outcome` is
    /// the snake_case wire form (`"success"`, `"failure"`, `"skipped"`)
    /// so each test reads like the JSONL it exercises; the helper
    /// re-projects it into the typed enum.
    fn child_completed_event(seq: u64, task_name: &str, outcome: &str, final_state: &str) -> Event {
        let outcome = match outcome {
            "success" => TerminalOutcome::Success,
            "failure" => TerminalOutcome::Failure,
            "skipped" => TerminalOutcome::Skipped,
            other => panic!("unknown outcome in test: {}", other),
        };
        Event {
            seq,
            timestamp: "2026-04-15T00:00:00Z".to_string(),
            event_type: "child_completed".to_string(),
            payload: EventPayload::ChildCompleted {
                child_name: format!("parent.{}", task_name),
                task_name: task_name.to_string(),
                outcome,
                final_state: final_state.to_string(),
            },
        }
    }

    #[test]
    fn augment_snapshots_adds_synthetic_entries_for_cleaned_children() {
        // No on-disk snapshots; one ChildCompleted event. The
        // augmentation must synthesize a terminal-success snapshot.
        let tasks = vec![task("a", &[])];
        let name_to_task: HashMap<&str, &TaskEntry> =
            tasks.iter().map(|t| (t.name.as_str(), t)).collect();
        let events = vec![child_completed_event(1, "a", "success", "done")];
        let mut snapshots: HashMap<String, ChildSnapshot> = HashMap::new();
        augment_snapshots_with_child_completed(&mut snapshots, &events, &name_to_task, None);
        let snap = snapshots.get("a").expect("synthetic snapshot inserted");
        assert!(snap.terminal);
        assert!(!snap.failure);
        assert!(!snap.skipped_marker);
        assert_eq!(snap.current_state, "done");
    }

    #[test]
    fn augment_snapshots_maps_failure_outcome() {
        let tasks = vec![task("a", &[])];
        let name_to_task: HashMap<&str, &TaskEntry> =
            tasks.iter().map(|t| (t.name.as_str(), t)).collect();
        let events = vec![child_completed_event(1, "a", "failure", "failed")];
        let mut snapshots: HashMap<String, ChildSnapshot> = HashMap::new();
        augment_snapshots_with_child_completed(&mut snapshots, &events, &name_to_task, None);
        let snap = snapshots.get("a").unwrap();
        assert!(snap.terminal);
        assert!(snap.failure);
        assert!(!snap.skipped_marker);
    }

    #[test]
    fn augment_snapshots_maps_skipped_outcome() {
        let tasks = vec![task("a", &[])];
        let name_to_task: HashMap<&str, &TaskEntry> =
            tasks.iter().map(|t| (t.name.as_str(), t)).collect();
        let events = vec![child_completed_event(1, "a", "skipped", "skipped")];
        let mut snapshots: HashMap<String, ChildSnapshot> = HashMap::new();
        augment_snapshots_with_child_completed(&mut snapshots, &events, &name_to_task, None);
        let snap = snapshots.get("a").unwrap();
        assert!(snap.terminal);
        assert!(!snap.failure);
        assert!(snap.skipped_marker);
    }

    #[test]
    fn augment_snapshots_skips_tasks_not_in_current_submission() {
        // A ChildCompleted event for a task that is no longer in the
        // current submission should be ignored so renamed/dropped
        // tasks don't poison the classification.
        let tasks = vec![task("b", &[])];
        let name_to_task: HashMap<&str, &TaskEntry> =
            tasks.iter().map(|t| (t.name.as_str(), t)).collect();
        let events = vec![child_completed_event(1, "a", "success", "done")];
        let mut snapshots: HashMap<String, ChildSnapshot> = HashMap::new();
        augment_snapshots_with_child_completed(&mut snapshots, &events, &name_to_task, None);
        assert!(snapshots.is_empty());
    }

    #[test]
    fn augment_snapshots_on_disk_wins_over_event() {
        // Issue #134 AC4: when a task has BOTH an on-disk snapshot and
        // a ChildCompleted event (e.g., a prior terminal visit that
        // was respawned by a retry), the on-disk snapshot is kept
        // unchanged. Without this precedence, a fresh respawn would
        // appear as the stale event's outcome and retry semantics
        // would be invisible to the gate.
        let tasks = vec![task("a", &[])];
        let name_to_task: HashMap<&str, &TaskEntry> =
            tasks.iter().map(|t| (t.name.as_str(), t)).collect();
        let events = vec![child_completed_event(1, "a", "success", "done")];
        let mut snapshots: HashMap<String, ChildSnapshot> = HashMap::new();
        // Fresh on-disk snapshot says the task is Running (not terminal).
        snapshots.insert("a".to_string(), snap("work", false, false, false));
        augment_snapshots_with_child_completed(&mut snapshots, &events, &name_to_task, None);
        let s = snapshots.get("a").unwrap();
        assert!(
            !s.terminal,
            "fresh on-disk Running snapshot must survive (not be overwritten by stale event)"
        );
        assert_eq!(s.current_state, "work");
    }

    #[test]
    fn augment_snapshots_latest_event_wins_when_task_cleaned_twice() {
        // Same task name observed in two ChildCompleted events (e.g.,
        // the child was retried, completed again, and auto-cleaned
        // again). With no on-disk snapshot, the later event's outcome
        // must be what the scheduler sees.
        let tasks = vec![task("a", &[])];
        let name_to_task: HashMap<&str, &TaskEntry> =
            tasks.iter().map(|t| (t.name.as_str(), t)).collect();
        let events = vec![
            child_completed_event(1, "a", "failure", "failed"),
            child_completed_event(2, "a", "success", "done"),
        ];
        let mut snapshots: HashMap<String, ChildSnapshot> = HashMap::new();
        augment_snapshots_with_child_completed(&mut snapshots, &events, &name_to_task, None);
        let s = snapshots.get("a").unwrap();
        assert_eq!(s.current_state, "done");
        assert!(!s.failure);
        assert!(s.terminal);
    }

    #[test]
    fn child_completed_event_round_trip() {
        // Sanity: the ChildCompleted event serializes and
        // deserializes to the same shape, the type discriminator is
        // the expected string, and the typed outcome enum serializes
        // as the snake_case wire form.
        let ev = Event {
            seq: 7,
            timestamp: "2026-04-15T00:00:00Z".to_string(),
            event_type: "child_completed".to_string(),
            payload: EventPayload::ChildCompleted {
                child_name: "parent.alpha".to_string(),
                task_name: "alpha".to_string(),
                outcome: TerminalOutcome::Success,
                final_state: "done".to_string(),
            },
        };
        let json = serde_json::to_string(&ev).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "child_completed");
        assert_eq!(v["payload"]["task_name"], "alpha");
        assert_eq!(v["payload"]["outcome"], "success");
        assert_eq!(v["payload"]["final_state"], "done");
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ev);
    }

    // --------- last_rewind_seq tests --------

    #[test]
    fn last_rewind_seq_returns_none_when_no_rewinds() {
        let events = vec![Event {
            seq: 1,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: "transitioned".to_string(),
            payload: EventPayload::Transitioned {
                from: Some("a".to_string()),
                to: "b".to_string(),
                condition_type: "gate".to_string(),
                skip_if_matched: None,
            },
        }];
        assert_eq!(last_rewind_seq(&events), None);
    }

    #[test]
    fn last_rewind_seq_returns_seq_of_last_rewind() {
        let events = vec![
            Event {
                seq: 1,
                timestamp: "2026-01-01T00:00:00Z".to_string(),
                event_type: "transitioned".to_string(),
                payload: EventPayload::Transitioned {
                    from: Some("a".to_string()),
                    to: "b".to_string(),
                    condition_type: "gate".to_string(),
                    skip_if_matched: None,
                },
            },
            Event {
                seq: 2,
                timestamp: "2026-01-01T00:00:01Z".to_string(),
                event_type: "rewound".to_string(),
                payload: EventPayload::Rewound {
                    from: "b".to_string(),
                    to: "a".to_string(),
                },
            },
            Event {
                seq: 3,
                timestamp: "2026-01-01T00:00:02Z".to_string(),
                event_type: "transitioned".to_string(),
                payload: EventPayload::Transitioned {
                    from: Some("a".to_string()),
                    to: "b".to_string(),
                    condition_type: "gate".to_string(),
                    skip_if_matched: None,
                },
            },
            Event {
                seq: 4,
                timestamp: "2026-01-01T00:00:03Z".to_string(),
                event_type: "rewound".to_string(),
                payload: EventPayload::Rewound {
                    from: "b".to_string(),
                    to: "a".to_string(),
                },
            },
        ];
        assert_eq!(last_rewind_seq(&events), Some(4));
    }

    #[test]
    fn augment_snapshots_skips_events_before_epoch_boundary() {
        let tasks = vec![task("a", &[])];
        let name_to_task: HashMap<&str, &TaskEntry> =
            tasks.iter().map(|t| (t.name.as_str(), t)).collect();
        // ChildCompleted at seq=2, epoch boundary at seq=3.
        let events = vec![child_completed_event(2, "a", "success", "done")];
        let mut snapshots: HashMap<String, ChildSnapshot> = HashMap::new();
        augment_snapshots_with_child_completed(
            &mut snapshots,
            &events,
            &name_to_task,
            Some(3), // boundary > event seq, so event is skipped
        );
        assert!(
            snapshots.is_empty(),
            "ChildCompleted before boundary should be ignored"
        );
    }

    #[test]
    fn augment_snapshots_keeps_events_after_epoch_boundary() {
        let tasks = vec![task("a", &[])];
        let name_to_task: HashMap<&str, &TaskEntry> =
            tasks.iter().map(|t| (t.name.as_str(), t)).collect();
        // ChildCompleted at seq=5, epoch boundary at seq=3.
        let events = vec![child_completed_event(5, "a", "success", "done")];
        let mut snapshots: HashMap<String, ChildSnapshot> = HashMap::new();
        augment_snapshots_with_child_completed(
            &mut snapshots,
            &events,
            &name_to_task,
            Some(3), // boundary < event seq, so event is kept
        );
        assert!(
            snapshots.contains_key("a"),
            "ChildCompleted after boundary should be kept"
        );
    }
}
