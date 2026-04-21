use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

/// Header line written as the first line of a state file.
///
/// Contains metadata about the workflow log. Has no `seq` field -- it is
/// not an event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StateFileHeader {
    /// Format version; currently `1`.
    pub schema_version: u32,

    /// Workflow name; must match the state filename.
    pub workflow: String,

    /// SHA-256 hex of the compiled template JSON.
    pub template_hash: String,

    /// RFC 3339 UTC timestamp of workflow creation.
    pub created_at: String,

    /// Name of the parent workflow, if this workflow was created as a child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_workflow: Option<String>,

    /// Directory the source template was loaded from at `koto init`
    /// time. Used by the batch scheduler's path resolver as the base
    /// for relative child-template paths (Decision 4 / 14 in
    /// DESIGN-batch-child-spawning.md).
    ///
    /// Captured from the parent directory of the absolute template
    /// path passed to `handle_init`. `None` for stdin / inline
    /// templates and for state files written before this field
    /// existed. The resolver emits
    /// `SchedulerWarning::MissingTemplateSourceDir` when this is
    /// `None` and the workflow submits a relative child template.
    ///
    /// Additive field: serde-optional, omitted when `None`, so older
    /// state files round-trip cleanly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_source_dir: Option<PathBuf>,
}

/// Canonical-form snapshot of the batch task entry that spawned a
/// child workflow.
///
/// Recorded on a child's `WorkflowInitialized` event at spawn time so
/// later ticks can compare a fresh submission against the entry the
/// child was actually created under (see Decision 10 / R8 spawn-time
/// immutability). The snapshot captures exactly three fields — the
/// source template path, the variable bindings, and the `waits_on`
/// dependency list — in a deterministic shape:
///
/// * `template` is the source template path string as submitted by
///   the batch scheduler (the path used to compile the template, not
///   the post-compile cache-dir path). For entries that inherited
///   from the parent's `default_template`, this is the resolved
///   source path AS IT STOOD AT THE SPAWNING TICK (not a `None`
///   inherited marker).
/// * `vars` uses a [`BTreeMap`] so the serialized key order is
///   lexicographic and stable — two snapshots with the same bindings
///   serialize byte-identically, which is what R8 needs for
///   spawn-time comparison.
/// * `waits_on` is stored sorted ascending; R8 byte-equality
///   comparison relies on this canonical ordering.
///
/// The struct is only read/written via `WorkflowInitialized` events,
/// never on its own row, so it does not need its own type_name.
///
/// Prefer constructing via [`SpawnEntrySnapshot::new`] to ensure
/// `waits_on` is stored in canonical (sorted) form. Direct struct
/// literal construction is permitted but the caller is responsible
/// for pre-sorting `waits_on`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpawnEntrySnapshot {
    /// Source template path as submitted by the scheduler (the path
    /// used to compile the template, not the cache-dir path).
    pub template: String,
    /// Variable bindings in canonical (sorted) form.
    #[serde(default)]
    pub vars: BTreeMap<String, serde_json::Value>,
    /// `waits_on` dependency list, sorted ascending for canonical
    /// form; R8 byte-equality relies on this.
    #[serde(default)]
    pub waits_on: Vec<String>,
}

impl SpawnEntrySnapshot {
    /// Construct a `SpawnEntrySnapshot` with `waits_on` sorted into
    /// canonical (ascending) order.
    ///
    /// This is the preferred construction path: R8 spawn-time
    /// comparison is a byte-equality check, and two snapshots with
    /// the same dependency set must produce identical JSON. Sorting
    /// here makes that invariant hold regardless of the order the
    /// scheduler submitted dependencies in.
    pub fn new(
        template: String,
        vars: BTreeMap<String, serde_json::Value>,
        waits_on: Vec<String>,
    ) -> Self {
        let mut waits_on = waits_on;
        waits_on.sort();
        Self {
            template,
            vars,
            waits_on,
        }
    }
}

/// Type-specific payload for each event variant.
///
/// Each variant's inner fields are serialized directly as the `payload`
/// object. The discriminant is carried by `Event.event_type`, not by
/// serde's enum tagging.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum EventPayload {
    WorkflowInitialized {
        template_path: String,
        #[serde(default)]
        variables: HashMap<String, String>,
        /// Canonical-form task entry recorded when the workflow was
        /// spawned by a batch scheduler (Decision 10 / 2 amendment).
        /// `None` for top-level `koto init`; `Some` for children
        /// materialized by `init_child_from_parent` with a parent.
        ///
        /// Additive field: `#[serde(default, skip_serializing_if = ...)]`
        /// keeps pre-feature state files round-tripping unchanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spawn_entry: Option<SpawnEntrySnapshot>,
    },
    Transitioned {
        from: Option<String>,
        to: String,
        condition_type: String,
        /// When the transition was triggered by a `skip_if` condition,
        /// this field records the matched key-value pairs from the
        /// state's `skip_if` map. `None` for ordinary evidence-driven
        /// or gate-driven transitions.
        ///
        /// Additive field: omitted from serialization when `None` so
        /// pre-feature JSONL files round-trip without modification.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skip_if_matched: Option<BTreeMap<String, serde_json::Value>>,
    },
    EvidenceSubmitted {
        state: String,
        fields: HashMap<String, serde_json::Value>,
        /// Working directory of the process that submitted this
        /// evidence. Used by the batch scheduler's path resolver as
        /// the final fallback for relative child-template paths
        /// (Decision 4 / 14 in DESIGN-batch-child-spawning.md).
        ///
        /// Captured from `std::env::current_dir()` at submission
        /// time. `None` for evidence submitted before this field
        /// existed; the resolver tolerates the absence by leaving
        /// the relative path unchanged.
        ///
        /// Additive field: serde-optional, omitted when `None`, so
        /// older state files round-trip cleanly.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        submitter_cwd: Option<PathBuf>,
    },
    IntegrationInvoked {
        state: String,
        integration: String,
        output: serde_json::Value,
    },
    DirectedTransition {
        from: String,
        to: String,
    },
    Rewound {
        from: String,
        to: String,
    },
    WorkflowCancelled {
        state: String,
        reason: String,
    },
    DefaultActionExecuted {
        state: String,
        command: String,
        exit_code: i32,
        stdout: String,
        stderr: String,
    },
    DecisionRecorded {
        state: String,
        decision: serde_json::Value,
    },
    GateEvaluated {
        state: String,
        gate: String,
        output: serde_json::Value,
        outcome: String,
        timestamp: String,
    },
    GateOverrideRecorded {
        state: String,
        gate: String,
        rationale: String,
        override_applied: serde_json::Value,
        actual_output: serde_json::Value,
        timestamp: String,
    },
    /// Per-tick audit record emitted by the batch scheduler on
    /// non-trivial ticks (Decision 11 / Issue #16). `tick_summary`
    /// carries per-tick counts (spawned, errored, skipped) and a
    /// `reclassified` flag; non-trivial means any of these non-zero /
    /// true. Pure no-op ticks deliberately skip the append to prevent
    /// log bloat.
    SchedulerRan {
        /// Parent state the scheduler ran against.
        state: String,
        /// Per-tick outcome summary — see [`SchedulerTickSummary`].
        tick_summary: SchedulerTickSummary,
        /// RFC 3339 UTC timestamp mirroring [`Event::timestamp`]. Kept
        /// inside the payload so downstream consumers reading just the
        /// payload don't need to pair it with the outer envelope.
        timestamp: String,
    },
    /// Emitted when the `children-complete` gate on a
    /// `materialize_children` state first reports `all_complete: true`.
    ///
    /// The `view` payload freezes the final batch shape at the moment
    /// the event appends: subsequent `koto status` reads and terminal
    /// `done` responses replay the most recent `BatchFinalized` to
    /// populate `batch_final_view` and to label `batch.phase: "final"`.
    ///
    /// Retries that re-enter the batched state do NOT mutate the prior
    /// event; they simply append a fresh `BatchFinalized` on the next
    /// pass. Consumers always read the MOST RECENT event. See
    /// DESIGN-batch-child-spawning.md Decision 13.
    BatchFinalized {
        /// The `materialize_children` state the batch finalized from.
        state: String,
        /// Frozen snapshot of the `children-complete` gate output at
        /// finalization time.
        view: serde_json::Value,
        /// RFC 3339 UTC timestamp mirroring [`Event::timestamp`].
        timestamp: String,
        /// Optional marker identifying the event that invalidated this
        /// finalization. Populated only when the log carries a later
        /// retry / rewind that re-entered the batched state. Written
        /// as `None` at append time; higher-level code that replays
        /// the log may compute and attach this projection when
        /// rendering stale events. See
        /// [`crate::cli::batch::annotate_superseded_batch_finalized`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        superseded_by: Option<SupersededByRef>,
    },
    /// Emitted on the PARENT'S log when a child workflow reaches a
    /// terminal state and is about to be auto-cleaned.
    ///
    /// Issue #134: the `children-complete` gate enumerates children
    /// via `backend.list()`, but auto-cleanup on the child's own
    /// terminal tick removes the child's session directory before the
    /// next `koto next <parent>` call can observe it. Without this
    /// event, the gate evaluator reclassifies a cleaned-up child as
    /// "pending" (no state file on disk, no classification entry) and
    /// the batch never satisfies `all_complete`.
    ///
    /// The event is appended to the parent's log (NOT the child's)
    /// just before `backend.cleanup(child)` runs, so the parent can
    /// synthesize a `ChildSnapshot` for any task whose on-disk state
    /// file has disappeared. On-disk snapshots always win over event
    /// replay (they are fresher — e.g., after a retry respawn), so
    /// the event is purely a fallback for the cleaned-up case.
    ChildCompleted {
        /// Full composed session id (e.g. `"parent.task-1"`).
        child_name: String,
        /// Short task name — the piece after the `<parent>.` prefix.
        /// For non-composed children (legacy `koto init --parent`
        /// without a batch hook) this equals `child_name`.
        task_name: String,
        /// Terminal outcome classification as a typed enum —
        /// serialized as snake_case (`"success"`, `"failure"`,
        /// `"skipped"`) to keep the wire format stable. Typed at
        /// the Rust level so the gate evaluator's match is
        /// exhaustive; silent typo fallbacks were a correctness
        /// risk with a stringly-typed variant.
        outcome: TerminalOutcome,
        /// The child's final state name. Used by the gate's
        /// `failure_mode` projection when `outcome == Failure`.
        final_state: String,
    },
}

/// Terminal outcome classification for a child workflow session, as
/// carried on [`EventPayload::ChildCompleted`].
///
/// Serialized as snake_case (`"success"`, `"failure"`, `"skipped"`)
/// to keep the JSONL wire format stable and match the string form
/// the batch scheduler's classifier emits. Typed (not stringly) so
/// every match on it is exhaustive — a missing arm is a compile
/// error, not a silent miscategorization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalOutcome {
    Success,
    Failure,
    Skipped,
}

/// Reference to the event that superseded a stale `BatchFinalized`.
///
/// Emitted as the `superseded_by` field on prior `BatchFinalized`
/// events once a retry / rewind / later finalization has invalidated
/// them. Carries both the `seq` (primary identifier) and the
/// timestamp so replay tools rendering non-sequential event streams
/// can correlate without an extra lookup.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SupersededByRef {
    /// Monotonic sequence number of the superseding event.
    pub seq: u64,
    /// Event type string of the superseding event
    /// (e.g., `"evidence_submitted"`, `"batch_finalized"`).
    #[serde(rename = "type")]
    pub event_type: String,
    /// RFC 3339 UTC timestamp of the superseding event.
    pub timestamp: String,
}

/// Per-tick counts recorded on a [`EventPayload::SchedulerRan`] event.
///
/// Mirrors the non-trivial-tick summary captured in
/// `SchedulerOutcome::Scheduled`; emitted as the `tick_summary` body
/// of the event so `koto query --events` can surface per-tick audit
/// without re-running the scheduler.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SchedulerTickSummary {
    /// Number of children whose state file was created during this
    /// tick.
    pub spawned_count: usize,
    /// Number of per-task spawn errors recorded during this tick.
    pub errored_count: usize,
    /// Number of children that moved to `Skipped` during this tick
    /// (fresh skip-marker spawns, not pre-existing skip markers).
    pub skipped_count: usize,
    /// True when at least one child's classification changed during
    /// this tick.
    pub reclassified: bool,
}

impl EventPayload {
    /// Return the string name matching the serialized `type` field.
    pub fn type_name(&self) -> &'static str {
        match self {
            EventPayload::WorkflowInitialized { .. } => "workflow_initialized",
            EventPayload::Transitioned { .. } => "transitioned",
            EventPayload::EvidenceSubmitted { .. } => "evidence_submitted",
            EventPayload::DirectedTransition { .. } => "directed_transition",
            EventPayload::IntegrationInvoked { .. } => "integration_invoked",
            EventPayload::Rewound { .. } => "rewound",
            EventPayload::WorkflowCancelled { .. } => "workflow_cancelled",
            EventPayload::DefaultActionExecuted { .. } => "default_action_executed",
            EventPayload::DecisionRecorded { .. } => "decision_recorded",
            EventPayload::GateEvaluated { .. } => "gate_evaluated",
            EventPayload::GateOverrideRecorded { .. } => "gate_override_recorded",
            EventPayload::SchedulerRan { .. } => "scheduler_ran",
            EventPayload::BatchFinalized { .. } => "batch_finalized",
            EventPayload::ChildCompleted { .. } => "child_completed",
        }
    }
}

/// A single event appended to the JSONL state log.
///
/// The `type` field serializes as a string matching the payload variant
/// name (e.g., "workflow_initialized", "transitioned", "rewound").
/// The `payload` field contains variant-specific data.
///
/// Custom Serialize/Deserialize: on serialization, `event_type` is set
/// from the payload variant name. On deserialization, the `type` field
/// drives which `EventPayload` variant to decode.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Monotonic sequence number starting at 1.
    pub seq: u64,

    /// RFC 3339 UTC timestamp of when this event was recorded.
    pub timestamp: String,

    /// Event type string (e.g., "workflow_initialized", "transitioned").
    pub event_type: String,

    /// Type-specific payload.
    pub payload: EventPayload,
}

impl Serialize for Event {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(4))?;
        map.serialize_entry("seq", &self.seq)?;
        map.serialize_entry("timestamp", &self.timestamp)?;
        map.serialize_entry("type", &self.payload.type_name())?;
        map.serialize_entry("payload", &self.payload)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for Event {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw: serde_json::Value = Deserialize::deserialize(deserializer)?;
        let obj = raw
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("event must be a JSON object"))?;

        let seq = obj
            .get("seq")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| serde::de::Error::custom("missing or invalid seq field"))?;

        let timestamp = obj
            .get("timestamp")
            .and_then(|v| v.as_str())
            .ok_or_else(|| serde::de::Error::custom("missing timestamp field"))?
            .to_string();

        let event_type = obj
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| serde::de::Error::custom("missing type field"))?
            .to_string();

        let payload_val = obj
            .get("payload")
            .ok_or_else(|| serde::de::Error::custom("missing payload field"))?;

        let payload: EventPayload = match event_type.as_str() {
            "workflow_initialized" => {
                let p: WorkflowInitializedPayload = serde_json::from_value(payload_val.clone())
                    .map_err(serde::de::Error::custom)?;
                EventPayload::WorkflowInitialized {
                    template_path: p.template_path,
                    variables: p.variables,
                    spawn_entry: p.spawn_entry,
                }
            }
            "transitioned" => {
                let p: TransitionedPayload = serde_json::from_value(payload_val.clone())
                    .map_err(serde::de::Error::custom)?;
                EventPayload::Transitioned {
                    from: p.from,
                    to: p.to,
                    condition_type: p.condition_type,
                    skip_if_matched: p.skip_if_matched,
                }
            }
            "evidence_submitted" => {
                let p: EvidenceSubmittedPayload = serde_json::from_value(payload_val.clone())
                    .map_err(serde::de::Error::custom)?;
                EventPayload::EvidenceSubmitted {
                    state: p.state,
                    fields: p.fields,
                    submitter_cwd: p.submitter_cwd,
                }
            }
            "directed_transition" => {
                let p: DirectedTransitionPayload = serde_json::from_value(payload_val.clone())
                    .map_err(serde::de::Error::custom)?;
                EventPayload::DirectedTransition {
                    from: p.from,
                    to: p.to,
                }
            }
            "integration_invoked" => {
                let p: IntegrationInvokedPayload = serde_json::from_value(payload_val.clone())
                    .map_err(serde::de::Error::custom)?;
                EventPayload::IntegrationInvoked {
                    state: p.state,
                    integration: p.integration,
                    output: p.output,
                }
            }
            "rewound" => {
                let p: RewoundPayload = serde_json::from_value(payload_val.clone())
                    .map_err(serde::de::Error::custom)?;
                EventPayload::Rewound {
                    from: p.from,
                    to: p.to,
                }
            }
            "workflow_cancelled" => {
                let p: WorkflowCancelledPayload = serde_json::from_value(payload_val.clone())
                    .map_err(serde::de::Error::custom)?;
                EventPayload::WorkflowCancelled {
                    state: p.state,
                    reason: p.reason,
                }
            }
            "default_action_executed" => {
                let p: DefaultActionExecutedPayload = serde_json::from_value(payload_val.clone())
                    .map_err(serde::de::Error::custom)?;
                EventPayload::DefaultActionExecuted {
                    state: p.state,
                    command: p.command,
                    exit_code: p.exit_code,
                    stdout: p.stdout,
                    stderr: p.stderr,
                }
            }
            "decision_recorded" => {
                let p: DecisionRecordedPayload = serde_json::from_value(payload_val.clone())
                    .map_err(serde::de::Error::custom)?;
                EventPayload::DecisionRecorded {
                    state: p.state,
                    decision: p.decision,
                }
            }
            "gate_evaluated" => {
                let p: GateEvaluatedPayload = serde_json::from_value(payload_val.clone())
                    .map_err(serde::de::Error::custom)?;
                EventPayload::GateEvaluated {
                    state: p.state,
                    gate: p.gate,
                    output: p.output,
                    outcome: p.outcome,
                    timestamp: p.timestamp,
                }
            }
            "gate_override_recorded" => {
                let p: GateOverrideRecordedPayload = serde_json::from_value(payload_val.clone())
                    .map_err(serde::de::Error::custom)?;
                EventPayload::GateOverrideRecorded {
                    state: p.state,
                    gate: p.gate,
                    rationale: p.rationale,
                    override_applied: p.override_applied,
                    actual_output: p.actual_output,
                    timestamp: p.timestamp,
                }
            }
            "scheduler_ran" => {
                let p: SchedulerRanPayload = serde_json::from_value(payload_val.clone())
                    .map_err(serde::de::Error::custom)?;
                EventPayload::SchedulerRan {
                    state: p.state,
                    tick_summary: p.tick_summary,
                    timestamp: p.timestamp,
                }
            }
            "batch_finalized" => {
                let p: BatchFinalizedPayload = serde_json::from_value(payload_val.clone())
                    .map_err(serde::de::Error::custom)?;
                EventPayload::BatchFinalized {
                    state: p.state,
                    view: p.view,
                    timestamp: p.timestamp,
                    superseded_by: p.superseded_by,
                }
            }
            "child_completed" => {
                let p: ChildCompletedPayload = serde_json::from_value(payload_val.clone())
                    .map_err(serde::de::Error::custom)?;
                EventPayload::ChildCompleted {
                    child_name: p.child_name,
                    task_name: p.task_name,
                    outcome: p.outcome,
                    final_state: p.final_state,
                }
            }
            other => {
                return Err(serde::de::Error::custom(format!(
                    "unknown event type: {}",
                    other
                )));
            }
        };

        Ok(Event {
            seq,
            timestamp,
            event_type,
            payload,
        })
    }
}

// Helper structs for typed deserialization of payload variants.
#[derive(Deserialize)]
struct WorkflowInitializedPayload {
    template_path: String,
    #[serde(default)]
    variables: HashMap<String, String>,
    #[serde(default)]
    spawn_entry: Option<SpawnEntrySnapshot>,
}

#[derive(Deserialize)]
struct TransitionedPayload {
    from: Option<String>,
    to: String,
    condition_type: String,
    #[serde(default)]
    skip_if_matched: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Deserialize)]
struct EvidenceSubmittedPayload {
    state: String,
    fields: HashMap<String, serde_json::Value>,
    #[serde(default)]
    submitter_cwd: Option<PathBuf>,
}

#[derive(Deserialize)]
struct DirectedTransitionPayload {
    from: String,
    to: String,
}

#[derive(Deserialize)]
struct IntegrationInvokedPayload {
    state: String,
    integration: String,
    output: serde_json::Value,
}

#[derive(Deserialize)]
struct RewoundPayload {
    from: String,
    to: String,
}

#[derive(Deserialize)]
struct WorkflowCancelledPayload {
    state: String,
    reason: String,
}

#[derive(Deserialize)]
struct DefaultActionExecutedPayload {
    state: String,
    command: String,
    exit_code: i32,
    stdout: String,
    stderr: String,
}

#[derive(Deserialize)]
struct DecisionRecordedPayload {
    state: String,
    decision: serde_json::Value,
}

#[derive(Deserialize)]
struct GateEvaluatedPayload {
    state: String,
    gate: String,
    output: serde_json::Value,
    outcome: String,
    timestamp: String,
}

#[derive(Deserialize)]
struct GateOverrideRecordedPayload {
    state: String,
    gate: String,
    rationale: String,
    override_applied: serde_json::Value,
    actual_output: serde_json::Value,
    timestamp: String,
}

#[derive(Deserialize)]
struct SchedulerRanPayload {
    state: String,
    tick_summary: SchedulerTickSummary,
    timestamp: String,
}

#[derive(Deserialize)]
struct BatchFinalizedPayload {
    state: String,
    view: serde_json::Value,
    timestamp: String,
    #[serde(default)]
    superseded_by: Option<SupersededByRef>,
}

#[derive(Deserialize)]
struct ChildCompletedPayload {
    child_name: String,
    task_name: String,
    outcome: TerminalOutcome,
    final_state: String,
}

/// Metadata about a workflow, derived from the state file header.
///
/// Used by `koto workflows` to return structured information about
/// each active workflow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowMetadata {
    /// Workflow name.
    pub name: String,

    /// RFC 3339 UTC timestamp of workflow creation.
    pub created_at: String,

    /// SHA-256 hex of the compiled template JSON.
    pub template_hash: String,

    /// Name of the parent workflow, if this workflow was created as a child.
    ///
    /// Always serialized (as `null` when absent) so that `koto workflows`
    /// JSON output has a consistent shape.
    #[serde(default)]
    pub parent_workflow: Option<String>,
}

/// Derived current state of a workflow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineState {
    /// The name of the current state, derived from log replay.
    pub current_state: String,

    /// Path to the compiled template (from the header / init event).
    pub template_path: String,

    /// SHA-256 hash of the compiled template (from the header).
    pub template_hash: String,
}

/// Return the current UTC time as an ISO 8601 string.
///
/// Implemented without an external time crate to keep the binary self-contained.
pub fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Format as YYYY-MM-DDTHH:MM:SSZ using integer arithmetic.
    let s = secs;
    let sec = s % 60;
    let min = (s / 60) % 60;
    let hour = (s / 3600) % 24;
    let days = s / 86400; // days since 1970-01-01

    // Compute year/month/day from days-since-epoch.
    let (year, month, day) = days_to_ymd(days);

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hour, min, sec
    )
}

fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    // Gregorian calendar computation.
    let mut year = 1970u64;
    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }
    let leap = is_leap(year);
    let month_days: &[u64] = if leap {
        &[31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        &[31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut month = 1u64;
    for &md in month_days {
        if days < md {
            break;
        }
        days -= md;
        month += 1;
    }
    (year, month, days + 1)
}

fn is_leap(y: u64) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_parsing_round_trip() {
        let header = StateFileHeader {
            schema_version: 1,
            workflow: "my-workflow".to_string(),
            template_hash: "abc123def456".to_string(),
            created_at: "2026-03-15T14:30:00Z".to_string(),
            parent_workflow: None,
            template_source_dir: None,
        };
        let json = serde_json::to_string(&header).unwrap();
        let parsed: StateFileHeader = serde_json::from_str(&json).unwrap();
        assert_eq!(header, parsed);
    }

    #[test]
    fn header_round_trip_with_parent_workflow() {
        let header = StateFileHeader {
            schema_version: 1,
            workflow: "child-wf".to_string(),
            template_hash: "abc123def456".to_string(),
            created_at: "2026-03-15T14:30:00Z".to_string(),
            parent_workflow: Some("parent-wf".to_string()),
            template_source_dir: None,
        };
        let json = serde_json::to_string(&header).unwrap();
        assert!(json.contains("\"parent_workflow\":\"parent-wf\""));
        let parsed: StateFileHeader = serde_json::from_str(&json).unwrap();
        assert_eq!(header, parsed);
        assert_eq!(parsed.parent_workflow, Some("parent-wf".to_string()));
    }

    #[test]
    fn header_without_parent_workflow_deserializes_to_none() {
        // Simulates loading an existing state file that was created before
        // the parent_workflow field existed.
        let json = r#"{"schema_version":1,"workflow":"old-wf","template_hash":"abc","created_at":"2026-01-01T00:00:00Z"}"#;
        let parsed: StateFileHeader = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.parent_workflow, None);
        assert_eq!(parsed.template_source_dir, None);
    }

    #[test]
    fn header_round_trip_with_template_source_dir() {
        let header = StateFileHeader {
            schema_version: 1,
            workflow: "wf".to_string(),
            template_hash: "hash".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            parent_workflow: None,
            template_source_dir: Some(PathBuf::from("/abs/templates")),
        };
        let json = serde_json::to_string(&header).unwrap();
        assert!(
            json.contains("\"template_source_dir\":\"/abs/templates\""),
            "got {}",
            json
        );
        let parsed: StateFileHeader = serde_json::from_str(&json).unwrap();
        assert_eq!(header, parsed);
    }

    #[test]
    fn header_none_template_source_dir_not_serialized() {
        let header = StateFileHeader {
            schema_version: 1,
            workflow: "wf".to_string(),
            template_hash: "hash".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            parent_workflow: None,
            template_source_dir: None,
        };
        let json = serde_json::to_string(&header).unwrap();
        assert!(
            !json.contains("template_source_dir"),
            "template_source_dir should be omitted when None, got {}",
            json
        );
    }

    #[test]
    fn header_none_parent_workflow_not_serialized() {
        let header = StateFileHeader {
            schema_version: 1,
            workflow: "wf".to_string(),
            template_hash: "hash".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            parent_workflow: None,
            template_source_dir: None,
        };
        let json = serde_json::to_string(&header).unwrap();
        assert!(
            !json.contains("parent_workflow"),
            "parent_workflow should be omitted when None"
        );
    }

    #[test]
    fn workflow_metadata_with_parent_roundtrip() {
        let wm = WorkflowMetadata {
            name: "child-wf".to_string(),
            created_at: "2026-03-15T14:30:00Z".to_string(),
            template_hash: "abc123".to_string(),
            parent_workflow: Some("parent-wf".to_string()),
        };
        let json = serde_json::to_string(&wm).unwrap();
        assert!(json.contains("\"parent_workflow\":\"parent-wf\""));
        let parsed: WorkflowMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(wm, parsed);
    }

    #[test]
    fn workflow_metadata_null_parent_in_json() {
        let wm = WorkflowMetadata {
            name: "root-wf".to_string(),
            created_at: "2026-03-15T14:30:00Z".to_string(),
            template_hash: "abc123".to_string(),
            parent_workflow: None,
        };
        let json = serde_json::to_string(&wm).unwrap();
        // When serialized via Serialize, None becomes null for non-skip fields
        // WorkflowMetadata doesn't skip_serializing_if, so null is included
        assert!(json.contains("\"parent_workflow\":null"));
    }

    #[test]
    fn event_serializes_type_and_payload() {
        let e = Event {
            seq: 1,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: "workflow_initialized".to_string(),
            payload: EventPayload::WorkflowInitialized {
                template_path: "path/to/template.json".to_string(),
                variables: HashMap::new(),
                spawn_entry: None,
            },
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"type\":\"workflow_initialized\""));
        assert!(json.contains("\"seq\":1"));
        assert!(json.contains("\"template_path\":\"path/to/template.json\""));
        // payload should be flat (no variant wrapper)
        assert!(!json.contains("\"workflow_initialized\":{"));
        // spawn_entry is omitted when None.
        assert!(
            !json.contains("spawn_entry"),
            "spawn_entry must be omitted when None, got {}",
            json
        );
    }

    #[test]
    fn workflow_initialized_without_spawn_entry_round_trip_omits_key() {
        // Simulates a pre-feature WorkflowInitialized event that never
        // carried a `spawn_entry` key. Parsing must succeed, and
        // re-serializing must NOT add a `spawn_entry` key (the feature
        // is additive and opt-in for batch-spawned children).
        let json = r#"{"seq":1,"timestamp":"2026-01-01T00:00:00Z","type":"workflow_initialized","payload":{"template_path":"/cache/abc.json","variables":{}}}"#;
        let parsed: Event = serde_json::from_str(json).expect("parse pre-feature event");
        match &parsed.payload {
            EventPayload::WorkflowInitialized { spawn_entry, .. } => {
                assert!(
                    spawn_entry.is_none(),
                    "pre-feature event must deserialize with spawn_entry=None"
                );
            }
            _ => panic!("expected WorkflowInitialized payload"),
        }
        let reserialized = serde_json::to_string(&parsed).unwrap();
        assert!(
            !reserialized.contains("spawn_entry"),
            "round-tripped pre-feature event must not introduce a spawn_entry key, got {}",
            reserialized
        );
    }

    #[test]
    fn workflow_initialized_with_spawn_entry_round_trip() {
        // With-snapshot path: a WorkflowInitialized event that carries a
        // canonical-form `spawn_entry`. Must round-trip byte-for-byte and
        // the serialized form must include the snapshot.
        let snapshot = SpawnEntrySnapshot::new(
            "impl-issue.md".to_string(),
            {
                let mut m = BTreeMap::new();
                m.insert(
                    "ISSUE_NUMBER".to_string(),
                    serde_json::Value::String("303".to_string()),
                );
                m
            },
            vec!["B".to_string()],
        );
        let e = Event {
            seq: 1,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: "workflow_initialized".to_string(),
            payload: EventPayload::WorkflowInitialized {
                template_path: "/cache/abc.json".to_string(),
                variables: HashMap::new(),
                spawn_entry: Some(snapshot.clone()),
            },
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(
            json.contains("\"spawn_entry\""),
            "with-snapshot event must serialize a spawn_entry key, got {}",
            json
        );
        assert!(json.contains("\"template\":\"impl-issue.md\""));
        assert!(json.contains("\"ISSUE_NUMBER\":\"303\""));
        assert!(json.contains("\"waits_on\":[\"B\"]"));

        let parsed: Event = serde_json::from_str(&json).expect("parse with-snapshot event");
        match &parsed.payload {
            EventPayload::WorkflowInitialized { spawn_entry, .. } => {
                assert_eq!(spawn_entry.as_ref(), Some(&snapshot));
            }
            _ => panic!("expected WorkflowInitialized payload"),
        }
        assert_eq!(e, parsed);
    }

    #[test]
    fn spawn_entry_snapshot_vars_serialize_in_sorted_order() {
        // BTreeMap guarantees lexicographic key order on serialization.
        // R8 spawn-time comparison depends on this so two snapshots
        // with the same bindings produce byte-identical JSON.
        let mut vars = BTreeMap::new();
        vars.insert("ZEBRA".to_string(), serde_json::json!("z"));
        vars.insert("APPLE".to_string(), serde_json::json!("a"));
        vars.insert("MANGO".to_string(), serde_json::json!("m"));

        let snapshot = SpawnEntrySnapshot::new("t.md".to_string(), vars, vec![]);
        let json = serde_json::to_string(&snapshot).unwrap();
        let apple_pos = json.find("APPLE").expect("APPLE present");
        let mango_pos = json.find("MANGO").expect("MANGO present");
        let zebra_pos = json.find("ZEBRA").expect("ZEBRA present");
        assert!(
            apple_pos < mango_pos && mango_pos < zebra_pos,
            "vars must serialize in sorted key order, got {}",
            json
        );
    }

    #[test]
    fn spawn_entry_snapshot_new_sorts_waits_on() {
        // The `new` constructor must store `waits_on` in canonical
        // (sorted) order regardless of the order the scheduler
        // submitted dependencies in. R8 byte-equality relies on this.
        let snapshot = SpawnEntrySnapshot::new(
            "t.md".to_string(),
            BTreeMap::new(),
            vec!["b".to_string(), "a".to_string(), "c".to_string()],
        );
        assert_eq!(
            snapshot.waits_on,
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            "waits_on must be sorted ascending"
        );
    }

    #[test]
    fn event_round_trip() {
        let e = Event {
            seq: 3,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: "rewound".to_string(),
            payload: EventPayload::Rewound {
                from: "analyze".to_string(),
                to: "gather".to_string(),
            },
        };
        let json = serde_json::to_string(&e).unwrap();
        let parsed: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(e, parsed);
    }

    #[test]
    fn event_transitioned_round_trip() {
        let e = Event {
            seq: 2,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: "transitioned".to_string(),
            payload: EventPayload::Transitioned {
                from: None,
                to: "gather".to_string(),
                condition_type: "auto".to_string(),
                skip_if_matched: None,
            },
        };
        let json = serde_json::to_string(&e).unwrap();
        let parsed: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(e, parsed);
    }

    #[test]
    fn event_payload_type_name() {
        let p = EventPayload::Transitioned {
            from: Some("a".to_string()),
            to: "b".to_string(),
            condition_type: "auto".to_string(),
            skip_if_matched: None,
        };
        assert_eq!(p.type_name(), "transitioned");

        let p2 = EventPayload::Rewound {
            from: "b".to_string(),
            to: "a".to_string(),
        };
        assert_eq!(p2.type_name(), "rewound");
    }

    #[test]
    fn workflow_metadata_roundtrip() {
        let wm = WorkflowMetadata {
            name: "test-wf".to_string(),
            created_at: "2026-03-15T14:30:00Z".to_string(),
            template_hash: "abc123".to_string(),
            parent_workflow: None,
        };
        let json = serde_json::to_string(&wm).unwrap();
        let parsed: WorkflowMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(wm, parsed);
    }

    #[test]
    fn machine_state_roundtrip() {
        let ms = MachineState {
            current_state: "plan".to_string(),
            template_path: "/cache/abc.json".to_string(),
            template_hash: "abc123".to_string(),
        };
        let json = serde_json::to_string(&ms).unwrap();
        let ms2: MachineState = serde_json::from_str(&json).unwrap();
        assert_eq!(ms2.current_state, "plan");
        assert_eq!(ms2.template_hash, "abc123");
    }

    #[test]
    fn event_decision_recorded_round_trip() {
        let e = Event {
            seq: 5,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: "decision_recorded".to_string(),
            payload: EventPayload::DecisionRecorded {
                state: "implementation".to_string(),
                decision: serde_json::json!({
                    "choice": "Use retry with backoff",
                    "rationale": "The API has no batch endpoint",
                    "alternatives_considered": ["Parallel requests", "Queue-based processing"]
                }),
            },
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"type\":\"decision_recorded\""));
        assert!(json.contains("\"choice\":\"Use retry with backoff\""));
        let parsed: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(e, parsed);
    }

    #[test]
    fn now_iso8601_format() {
        let ts = now_iso8601();
        assert_eq!(ts.len(), 20);
        assert!(ts.ends_with('Z'));
        assert!(ts.contains('T'));
    }

    #[test]
    fn default_action_executed_round_trip() {
        let e = Event {
            seq: 5,
            timestamp: "2026-03-22T10:00:00Z".to_string(),
            event_type: "default_action_executed".to_string(),
            payload: EventPayload::DefaultActionExecuted {
                state: "setup".to_string(),
                command: "git checkout -b feature".to_string(),
                exit_code: 0,
                stdout: "Switched to a new branch 'feature'\n".to_string(),
                stderr: String::new(),
            },
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"type\":\"default_action_executed\""));
        assert!(json.contains("\"exit_code\":0"));
        let parsed: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(e, parsed);
    }

    #[test]
    fn default_action_executed_type_name() {
        let p = EventPayload::DefaultActionExecuted {
            state: "s".to_string(),
            command: "c".to_string(),
            exit_code: 1,
            stdout: String::new(),
            stderr: "err".to_string(),
        };
        assert_eq!(p.type_name(), "default_action_executed");
    }

    #[test]
    fn gate_evaluated_round_trip() {
        let e = Event {
            seq: 10,
            timestamp: "2026-04-01T00:00:00Z".to_string(),
            event_type: "gate_evaluated".to_string(),
            payload: EventPayload::GateEvaluated {
                state: "review".to_string(),
                gate: "ci-passes".to_string(),
                output: serde_json::json!({"exit_code": 0, "error": ""}),
                outcome: "passed".to_string(),
                timestamp: "2026-04-01T00:00:00Z".to_string(),
            },
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"type\":\"gate_evaluated\""));
        assert!(json.contains("\"gate\":\"ci-passes\""));
        assert!(json.contains("\"outcome\":\"passed\""));
        let parsed: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(e, parsed);
    }

    #[test]
    fn gate_evaluated_type_name() {
        let p = EventPayload::GateEvaluated {
            state: "s".to_string(),
            gate: "g".to_string(),
            output: serde_json::Value::Null,
            outcome: "passed".to_string(),
            timestamp: "2026-04-01T00:00:00Z".to_string(),
        };
        assert_eq!(p.type_name(), "gate_evaluated");
    }

    #[test]
    fn gate_override_recorded_round_trip() {
        let e = Event {
            seq: 11,
            timestamp: "2026-04-01T00:01:00Z".to_string(),
            event_type: "gate_override_recorded".to_string(),
            payload: EventPayload::GateOverrideRecorded {
                state: "review".to_string(),
                gate: "ci-passes".to_string(),
                rationale: "CI is broken in infra, not our code".to_string(),
                override_applied: serde_json::json!({"exit_code": 0, "error": ""}),
                actual_output: serde_json::json!({"exit_code": 1, "error": "timeout"}),
                timestamp: "2026-04-01T00:01:00Z".to_string(),
            },
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"type\":\"gate_override_recorded\""));
        assert!(json.contains("\"gate\":\"ci-passes\""));
        assert!(json.contains("\"rationale\":\"CI is broken in infra, not our code\""));
        assert!(json.contains("\"override_applied\""));
        assert!(json.contains("\"actual_output\""));
        let parsed: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(e, parsed);
    }

    #[test]
    fn gate_override_recorded_type_name() {
        let p = EventPayload::GateOverrideRecorded {
            state: "s".to_string(),
            gate: "g".to_string(),
            rationale: "r".to_string(),
            override_applied: serde_json::Value::Null,
            actual_output: serde_json::Value::Null,
            timestamp: "2026-04-01T00:00:00Z".to_string(),
        };
        assert_eq!(p.type_name(), "gate_override_recorded");
    }

    #[test]
    fn gate_evaluated_missing_state_field_fails() {
        // Negative case: payload missing the required `state` field.
        let json = r#"{"seq":1,"timestamp":"2026-04-01T00:00:00Z","type":"gate_evaluated","payload":{"gate":"ci-passes","output":{},"outcome":"passed","timestamp":"2026-04-01T00:00:00Z"}}"#;
        let result: Result<Event, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "expected error for missing state field, got: {:?}",
            result
        );
    }

    #[test]
    fn gate_override_recorded_missing_state_field_fails() {
        // Negative case: payload missing the required `state` field.
        let json = r#"{"seq":1,"timestamp":"2026-04-01T00:00:00Z","type":"gate_override_recorded","payload":{"gate":"ci-passes","rationale":"r","override_applied":{},"actual_output":{},"timestamp":"2026-04-01T00:00:00Z"}}"#;
        let result: Result<Event, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "expected error for missing state field, got: {:?}",
            result
        );
    }

    #[test]
    fn evidence_submitted_round_trip_with_submitter_cwd() {
        let e = Event {
            seq: 4,
            timestamp: "2026-04-13T10:00:00Z".to_string(),
            event_type: "evidence_submitted".to_string(),
            payload: EventPayload::EvidenceSubmitted {
                state: "review".to_string(),
                fields: {
                    let mut m = HashMap::new();
                    m.insert("decision".to_string(), serde_json::json!("approve"));
                    m
                },
                submitter_cwd: Some(PathBuf::from("/work/repo")),
            },
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(
            json.contains("\"submitter_cwd\":\"/work/repo\""),
            "got {}",
            json
        );
        let parsed: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(e, parsed);
    }

    #[test]
    fn batch_finalized_event_round_trip() {
        // Issue #17: BatchFinalized event carries a frozen batch view
        // and round-trips through serde identically to other events.
        let view = serde_json::json!({
            "total": 2,
            "completed": 2,
            "pending": 0,
            "success": 2,
            "failed": 0,
            "skipped": 0,
            "blocked": 0,
            "spawn_failed": 0,
            "all_complete": true,
            "all_success": true,
            "any_failed": false,
            "any_skipped": false,
            "any_spawn_failed": false,
            "needs_attention": false,
            "children": [],
        });
        let e = Event {
            seq: 42,
            timestamp: "2026-04-14T12:00:00Z".to_string(),
            event_type: "batch_finalized".to_string(),
            payload: EventPayload::BatchFinalized {
                state: "plan".to_string(),
                view: view.clone(),
                timestamp: "2026-04-14T12:00:00Z".to_string(),
                superseded_by: None,
            },
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"type\":\"batch_finalized\""));
        assert!(json.contains("\"state\":\"plan\""));
        assert!(json.contains("\"all_complete\":true"));
        // superseded_by is omitted when None.
        assert!(
            !json.contains("superseded_by"),
            "superseded_by must be omitted when None, got {}",
            json
        );
        let parsed: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(e, parsed);
    }

    #[test]
    fn batch_finalized_event_with_superseded_by_round_trip() {
        // When a prior BatchFinalized has been invalidated by a later
        // retry, the `superseded_by` field points at the superseding
        // event (seq + type + timestamp).
        let e = Event {
            seq: 7,
            timestamp: "2026-04-14T11:00:00Z".to_string(),
            event_type: "batch_finalized".to_string(),
            payload: EventPayload::BatchFinalized {
                state: "plan".to_string(),
                view: serde_json::json!({"total": 1, "children": []}),
                timestamp: "2026-04-14T11:00:00Z".to_string(),
                superseded_by: Some(SupersededByRef {
                    seq: 12,
                    event_type: "evidence_submitted".to_string(),
                    timestamp: "2026-04-14T11:30:00Z".to_string(),
                }),
            },
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(
            json.contains("\"superseded_by\""),
            "superseded_by must serialize when Some, got {}",
            json
        );
        assert!(json.contains("\"seq\":12"));
        assert!(json.contains("\"type\":\"evidence_submitted\""));
        let parsed: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(e, parsed);
    }

    #[test]
    fn batch_finalized_type_name() {
        let p = EventPayload::BatchFinalized {
            state: "plan".to_string(),
            view: serde_json::Value::Null,
            timestamp: "2026-04-14T11:00:00Z".to_string(),
            superseded_by: None,
        };
        assert_eq!(p.type_name(), "batch_finalized");
    }

    #[test]
    fn evidence_submitted_round_trip_without_submitter_cwd_omits_key() {
        // Pre-feature evidence event lacking submitter_cwd must
        // deserialize cleanly and re-serialize without introducing
        // the new key.
        let json = r#"{"seq":4,"timestamp":"2026-04-13T10:00:00Z","type":"evidence_submitted","payload":{"state":"review","fields":{"decision":"approve"}}}"#;
        let parsed: Event = serde_json::from_str(json).expect("parse pre-feature event");
        match &parsed.payload {
            EventPayload::EvidenceSubmitted { submitter_cwd, .. } => {
                assert!(submitter_cwd.is_none());
            }
            other => panic!("expected EvidenceSubmitted, got {:?}", other),
        }
        let reserialized = serde_json::to_string(&parsed).unwrap();
        assert!(
            !reserialized.contains("submitter_cwd"),
            "round-tripped pre-feature event must not introduce submitter_cwd, got {}",
            reserialized
        );
    }
}
