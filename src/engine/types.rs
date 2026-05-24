use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;

use crate::engine::errors::EngineError;

pub use crate::engine::persistence::derive_state_from_log;

/// Compiled validation regex shared by [`ValidatedSessionId`] and
/// [`ValidatedCoordId`].
///
/// Rejects any input that contains shell metacharacters, path
/// separators, or any other byte outside `[a-zA-Z0-9._-]`, and any
/// input that does not start with an alphanumeric (so leading `.` and
/// leading `-` — which can be re-interpreted as hidden files or CLI
/// flags downstream — are also rejected).
///
/// The pattern is the security spine for juror-2 N1/N5: every flow
/// that lands a session or coordinator id from caller-controlled
/// input passes through one of the two newtype constructors, which
/// both delegate to this regex. Keep the pattern conservative — it
/// is easier to relax later than to chase a hole through every
/// downstream caller.
static VALIDATED_ID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9._\-]*$")
        .expect("VALIDATED_ID_RE is a constant and always parses")
});

/// Maximum byte length for a validated session or coordinator id.
///
/// Caps prevent log-injection via overlong inputs and keep path
/// components within typical filesystem limits.
const VALIDATED_ID_MAX_LEN: usize = 255;

fn validate_id_input(input: &str) -> Result<(), &'static str> {
    if input.is_empty() {
        return Err("empty input");
    }
    if input.len() > VALIDATED_ID_MAX_LEN {
        return Err("input exceeds 255 characters");
    }
    // Cheap structural rejections first so the regex doesn't even
    // need to run for the common cases — also makes the error
    // messages more specific.
    let first = input.as_bytes()[0];
    if first == b'.' {
        return Err("leading dot");
    }
    if first == b'-' {
        return Err("leading hyphen");
    }
    if !VALIDATED_ID_RE.is_match(input) {
        return Err("contains disallowed characters");
    }
    Ok(())
}

/// Truncate `input` to at most 64 chars (byte-safe), appending `...`
/// when shortening. Used to keep error messages bounded.
fn truncate_id_for_preview(input: &str) -> String {
    const MAX: usize = 64;
    if input.len() <= MAX {
        return input.to_string();
    }
    let mut cut = MAX;
    while !input.is_char_boundary(cut) && cut > 0 {
        cut -= 1;
    }
    format!("{}...", &input[..cut])
}

/// A session id that has passed the workspace's shell-safe validation
/// regex.
///
/// Construct via [`ValidatedSessionId::new`]. Once constructed, the
/// inner string is guaranteed to match
/// `^[a-zA-Z0-9][a-zA-Z0-9._-]*$` and be at most
/// [`VALIDATED_ID_MAX_LEN`] (255) bytes long — downstream path-join
/// operations and shell-quoted contexts can rely on this without
/// re-validating.
///
/// The newtype pattern makes validation un-bypassable at the type
/// level: a function that accepts `&ValidatedSessionId` literally
/// cannot be called with an unvalidated string. Issues 4, 6, 11, 13,
/// 14 build on this invariant. See the design doc's Security
/// Considerations N1 / N5.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ValidatedSessionId(String);

impl ValidatedSessionId {
    /// Construct a `ValidatedSessionId` from caller-controlled input.
    ///
    /// Returns [`EngineError::InvalidSessionId`] when `input` is
    /// empty, longer than 255 bytes, starts with `.` or `-`, or
    /// contains any character outside `[a-zA-Z0-9._-]`.
    pub fn new(input: &str) -> Result<Self, EngineError> {
        validate_id_input(input).map_err(|reason| EngineError::InvalidSessionId {
            reason: reason.to_string(),
            input_preview: truncate_id_for_preview(input),
        })?;
        Ok(Self(input.to_string()))
    }

    /// Borrow the inner string without re-validation.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the newtype and return the inner owned string.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Display for ValidatedSessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ValidatedSessionId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// A coordinator id that has passed the same shell-safe validation
/// regex as [`ValidatedSessionId`].
///
/// Construct via [`ValidatedCoordId::new`]. See `ValidatedSessionId`
/// for the contract — `ValidatedCoordId` shares the same regex,
/// length cap, and Display/AsRef surface so coordinator ids and
/// session ids are interchangeable wherever a callee only needs the
/// shell-safety guarantee.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ValidatedCoordId(String);

impl ValidatedCoordId {
    /// Construct a `ValidatedCoordId` from caller-controlled input.
    ///
    /// Returns [`EngineError::InvalidCoordId`] when `input` is
    /// empty, longer than 255 bytes, starts with `.` or `-`, or
    /// contains any character outside `[a-zA-Z0-9._-]`.
    pub fn new(input: &str) -> Result<Self, EngineError> {
        validate_id_input(input).map_err(|reason| EngineError::InvalidCoordId {
            reason: reason.to_string(),
            input_preview: truncate_id_for_preview(input),
        })?;
        Ok(Self(input.to_string()))
    }

    /// Borrow the inner string without re-validation.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the newtype and return the inner owned string.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Display for ValidatedCoordId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ValidatedCoordId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Current schema version written in every `StateFileHeader`.
///
/// Readers reject log files where `schema_version > CURRENT_SCHEMA_VERSION`
/// with `EngineError::IncompatibleSchemaVersion`. Bump this constant when:
/// - a new required event type is added,
/// - a required field is removed from an existing event type, or
/// - the event envelope keys (seq, timestamp, type, payload) change.
///
/// Additive optional fields do NOT require a bump.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

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

    /// UUID v4 identifier generated at `koto init` time and preserved
    /// unchanged through rename operations.
    ///
    /// Additive field: deserializes to an empty string when absent so
    /// older state files continue to load without error.
    #[serde(default)]
    pub session_id: String,

    /// Human-readable description of the workflow's goal, set at init time.
    ///
    /// Additive field: omitted when None, defaults to None on old state files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,

    /// Name of the template used to initialize this workflow.
    ///
    /// Populated from the template's `name` frontmatter field at init time.
    /// Additive field: omitted when None, defaults to None on old state files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_name: Option<String>,

    // ===== KT1 request-store fields (Decision 1) =====
    //
    // The seven additive + four reserved fields below land the
    // dispatch-request marker that bunki BK2 and downstream KT1
    // components read from the on-disk header. All are
    // `#[serde(default, skip_serializing_if = "Option::is_none")]` so
    // pre-KT1 state files round-trip unchanged.
    /// Whether this workflow is requesting agent assignment.
    ///
    /// Drives the KT1 request-store: when `Some(true)` the workflow is
    /// awaiting (or has been claimed for) an agent dispatch. `None` on
    /// pre-KT1 headers and on workflows that don't need agent
    /// orchestration. Companion-field validation (role / inputs) is
    /// owned by the CLI layer (Issue 4), not the type layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_agent: Option<bool>,

    /// Role identifier the assigning coordinator should match against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,

    /// Arbitrary JSON payload passed to the agent at dispatch time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<serde_json::Value>,

    /// Identifier of the coordinator that owns this request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinator_of_record: Option<String>,

    /// Identifier of the principal that submitted this request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_by: Option<String>,

    /// Claim record written by the coordinator that picked up this
    /// request. Populated as a single atomic write through the
    /// claim sidecar (Issue 11); absent until claim time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignment_claim: Option<AssignmentClaim>,

    /// Generation counter incremented every time the request is
    /// re-dispatched after a previous claim was revoked. Defaults to
    /// `0` on pre-KT1 headers and on never-claimed requests.
    #[serde(default)]
    pub dispatch_epoch: u32,

    // ===== Reserved KT1 fields (forward-compatibility) =====
    //
    // Wire-format placeholders for follow-up features. Always
    // serialize as absent keys when `None`; reading code may parse
    // them but should not act on them yet.
    /// Reserved: dispatch priority (forward-compat placeholder).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,

    /// Reserved: optional deadline (RFC 3339 string, forward-compat).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,

    /// Reserved: retry counter (forward-compat placeholder).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_count: Option<u32>,

    /// Reserved: opaque agent configuration blob (forward-compat).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_config: Option<serde_json::Value>,
}

/// Claim record written by the coordinator that picks up an agent
/// dispatch request.
///
/// Carries the coordinator id and the RFC 3339 claim timestamp. The
/// struct is treated as opaque by the type layer; ordering and
/// fencing semantics live in the claim sidecar and dispatch-epoch
/// fence (Issues 11 and 13).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssignmentClaim {
    /// Identifier of the claiming coordinator.
    pub coord_id: String,
    /// RFC 3339 UTC timestamp at which the claim was recorded.
    pub claimed_at: String,
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
        /// Optional human-readable reason for this directed transition.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rationale: Option<String>,
    },
    Rewound {
        from: String,
        to: String,
        /// Optional human-readable reason for this rewind.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rationale: Option<String>,
    },
    /// Emitted when a context artifact is successfully stored via `koto context add`.
    ContextAdded {
        /// The context key under which the artifact was stored.
        key: String,
        /// SHA-256 hex digest of the artifact content.
        hash: String,
        /// Size of the artifact content in bytes.
        size: u64,
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
    /// Emitted by `koto session update --intent` to update the workflow's
    /// stated goal concurrently with execution. Multiple events may appear;
    /// the last one wins (see `derive_intent`).
    IntentUpdated {
        intent: String,
    },
    /// Catch-all for event type strings not recognized by this koto version.
    ///
    /// Enables graceful degradation when reading logs produced by a newer
    /// koto version that introduced a new event type. The original type
    /// string and raw payload are preserved so operators can inspect them
    /// via `koto query --events`.
    ///
    /// This variant is deserialization-only. Callers must not pass it to
    /// `append_event` — doing so would write a corrupted event to disk.
    /// The guard in `append_event` is a `debug_assert` rather than a hard
    /// panic because this is an internal invariant: `Unknown` is only ever
    /// constructed inside the `Event::deserialize` impl, so production builds
    /// can rely on the type system to uphold the constraint without the runtime
    /// check.
    Unknown {
        /// The unrecognized `type` string from the original event.
        type_name: String,
        /// The original `payload` object, preserved verbatim.
        raw_payload: serde_json::Value,
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

/// Snapshot of a child state file that `classify_task` needs to
/// determine the child's `TaskOutcome`. Built once per tick by the
/// scheduler and looked up by short task name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildSnapshot {
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
            EventPayload::ContextAdded { .. } => "context_added",
            EventPayload::WorkflowCancelled { .. } => "workflow_cancelled",
            EventPayload::DefaultActionExecuted { .. } => "default_action_executed",
            EventPayload::DecisionRecorded { .. } => "decision_recorded",
            EventPayload::GateEvaluated { .. } => "gate_evaluated",
            EventPayload::GateOverrideRecorded { .. } => "gate_override_recorded",
            EventPayload::SchedulerRan { .. } => "scheduler_ran",
            EventPayload::BatchFinalized { .. } => "batch_finalized",
            EventPayload::ChildCompleted { .. } => "child_completed",
            EventPayload::IntentUpdated { .. } => "intent_updated",
            EventPayload::Unknown { .. } => "unknown",
        }
    }
}

/// Return the intent string from the last `IntentUpdated` event in `events`,
/// or `None` if no such event exists.
pub fn derive_intent(events: &[Event]) -> Option<String> {
    events.iter().rev().find_map(|e| {
        if let EventPayload::IntentUpdated { intent } = &e.payload {
            Some(intent.clone())
        } else {
            None
        }
    })
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
        // Use the stored event_type string rather than payload.type_name() so that
        // Unknown events round-trip their original type string (e.g. "pause_requested")
        // instead of the static "unknown" label when serialized by koto query --events.
        map.serialize_entry("type", &self.event_type)?;
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
                    rationale: p.rationale,
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
                    rationale: p.rationale,
                }
            }
            "context_added" => {
                let p: ContextAddedPayload = serde_json::from_value(payload_val.clone())
                    .map_err(serde::de::Error::custom)?;
                EventPayload::ContextAdded {
                    key: p.key,
                    hash: p.hash,
                    size: p.size,
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
            "intent_updated" => {
                let p: IntentUpdatedPayload = serde_json::from_value(payload_val.clone())
                    .map_err(serde::de::Error::custom)?;
                EventPayload::IntentUpdated { intent: p.intent }
            }
            other => EventPayload::Unknown {
                type_name: other.to_string(),
                raw_payload: payload_val.clone(),
            },
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
    #[serde(default)]
    rationale: Option<String>,
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
    #[serde(default)]
    rationale: Option<String>,
}

#[derive(Deserialize)]
struct ContextAddedPayload {
    key: String,
    hash: String,
    size: u64,
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

#[derive(Deserialize)]
struct IntentUpdatedPayload {
    intent: String,
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

    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();

    let secs = duration.as_secs();
    let millis = duration.subsec_millis();

    // Format as YYYY-MM-DDTHH:MM:SS.mmmZ using integer arithmetic.
    let sec = secs % 60;
    let min = (secs / 60) % 60;
    let hour = (secs / 3600) % 24;
    let days = secs / 86400; // days since 1970-01-01

    // Compute year/month/day from days-since-epoch.
    let (year, month, day) = days_to_ymd(days);

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year, month, day, hour, min, sec, millis
    )
}

/// Generate a UUID v4 using `/dev/urandom` with no external crate dependencies.
pub fn generate_session_id() -> String {
    use std::io::Read;

    let mut buf = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .unwrap_or_default();

    // Set version 4 bits.
    buf[6] = (buf[6] & 0x0F) | 0x40;
    // Set variant bits (RFC 4122).
    buf[8] = (buf[8] & 0x3F) | 0x80;

    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        buf[0], buf[1], buf[2], buf[3],
        buf[4], buf[5],
        buf[6], buf[7],
        buf[8], buf[9],
        buf[10], buf[11], buf[12], buf[13], buf[14], buf[15]
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

pub(crate) fn is_leap(y: u64) -> bool {
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
            session_id: String::new(),
            intent: None,
            template_name: None,
            needs_agent: None,
            role: None,
            inputs: None,
            coordinator_of_record: None,
            requested_by: None,
            assignment_claim: None,
            dispatch_epoch: 0,
            priority: None,
            deadline: None,
            retry_count: None,
            agent_config: None,
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
            session_id: String::new(),
            intent: None,
            template_name: None,
            needs_agent: None,
            role: None,
            inputs: None,
            coordinator_of_record: None,
            requested_by: None,
            assignment_claim: None,
            dispatch_epoch: 0,
            priority: None,
            deadline: None,
            retry_count: None,
            agent_config: None,
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
            session_id: String::new(),
            intent: None,
            template_name: None,
            needs_agent: None,
            role: None,
            inputs: None,
            coordinator_of_record: None,
            requested_by: None,
            assignment_claim: None,
            dispatch_epoch: 0,
            priority: None,
            deadline: None,
            retry_count: None,
            agent_config: None,
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
            session_id: String::new(),
            intent: None,
            template_name: None,
            needs_agent: None,
            role: None,
            inputs: None,
            coordinator_of_record: None,
            requested_by: None,
            assignment_claim: None,
            dispatch_epoch: 0,
            priority: None,
            deadline: None,
            retry_count: None,
            agent_config: None,
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
            session_id: String::new(),
            intent: None,
            template_name: None,
            needs_agent: None,
            role: None,
            inputs: None,
            coordinator_of_record: None,
            requested_by: None,
            assignment_claim: None,
            dispatch_epoch: 0,
            priority: None,
            deadline: None,
            retry_count: None,
            agent_config: None,
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
                rationale: None,
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
            rationale: None,
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
        assert_eq!(
            ts.len(),
            24,
            "expected 24-char millisecond-precision timestamp, got '{}'",
            ts
        );
        assert!(ts.ends_with('Z'));
        assert!(ts.contains('T'));
        assert_eq!(
            ts.chars().nth(19),
            Some('.'),
            "expected '.' at position 19, got '{}'",
            ts
        );
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

    // ===== Issue 1: millisecond timestamps =====

    #[test]
    fn now_iso8601_millisecond_precision() {
        let ts = now_iso8601();
        // Format: YYYY-MM-DDTHH:MM:SS.mmmZ (24 chars)
        assert_eq!(ts.len(), 24);
        // Position 19 must be the decimal point.
        let bytes = ts.as_bytes();
        assert_eq!(bytes[19], b'.');
        // Remaining three chars before Z must be digits.
        assert!(bytes[20].is_ascii_digit());
        assert!(bytes[21].is_ascii_digit());
        assert!(bytes[22].is_ascii_digit());
        assert_eq!(bytes[23], b'Z');
    }

    // ===== Issue 2: session_id on StateFileHeader =====

    #[test]
    fn header_session_id_round_trip() {
        let header = StateFileHeader {
            schema_version: 1,
            workflow: "wf".to_string(),
            template_hash: "hash".to_string(),
            created_at: "2026-01-01T00:00:00.000Z".to_string(),
            parent_workflow: None,
            template_source_dir: None,
            session_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            intent: None,
            template_name: None,
            needs_agent: None,
            role: None,
            inputs: None,
            coordinator_of_record: None,
            requested_by: None,
            assignment_claim: None,
            dispatch_epoch: 0,
            priority: None,
            deadline: None,
            retry_count: None,
            agent_config: None,
        };
        let json = serde_json::to_string(&header).unwrap();
        assert!(json.contains("\"session_id\":\"550e8400-e29b-41d4-a716-446655440000\""));
        let parsed: StateFileHeader = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.session_id, "550e8400-e29b-41d4-a716-446655440000");
    }

    #[test]
    fn header_session_id_defaults_to_empty_for_old_state_files() {
        // Old state files without session_id field should parse cleanly.
        let json = r#"{"schema_version":1,"workflow":"old-wf","template_hash":"abc","created_at":"2026-01-01T00:00:00Z"}"#;
        let parsed: StateFileHeader = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.session_id, "");
    }

    #[test]
    fn generate_session_id_format() {
        let id = generate_session_id();
        // UUID v4: 8-4-4-4-12 hex groups separated by hyphens, all lowercase.
        assert_eq!(id.len(), 36, "UUID must be 36 chars, got '{}'", id);
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.len(), 5, "UUID must have 5 hyphen-delimited parts");
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[1].len(), 4);
        assert_eq!(parts[2].len(), 4);
        assert_eq!(parts[3].len(), 4);
        assert_eq!(parts[4].len(), 12);
        // Version nibble: must be '4'.
        assert_eq!(&parts[2][0..1], "4", "version nibble must be 4");
        // Variant bits: first char of group 4 must be 8, 9, a, or b.
        let variant_char = parts[3].chars().next().unwrap();
        assert!(
            matches!(variant_char, '8' | '9' | 'a' | 'b'),
            "variant char must be 8/9/a/b, got '{}'",
            variant_char
        );
        // All chars must be lowercase hex or hyphens.
        for c in id.chars() {
            assert!(
                c.is_ascii_hexdigit() && !c.is_ascii_uppercase() || c == '-',
                "UUID must be lowercase hex, got char '{}'",
                c
            );
        }
    }

    #[test]
    fn generate_session_id_unique() {
        let a = generate_session_id();
        let b = generate_session_id();
        assert_ne!(a, b, "two generated UUIDs must differ");
    }

    // ===== Issue 3: context_added event =====

    #[test]
    fn context_added_round_trip() {
        let e = Event {
            seq: 7,
            timestamp: "2026-05-01T12:00:00.000Z".to_string(),
            event_type: "context_added".to_string(),
            payload: EventPayload::ContextAdded {
                key: "plan.md".to_string(),
                hash: "abc123def456abc123def456abc123def456abc123def456abc123def456abc12345"
                    .to_string(),
                size: 1024,
            },
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"type\":\"context_added\""));
        assert!(json.contains("\"key\":\"plan.md\""));
        assert!(json.contains("\"size\":1024"));
        let parsed: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(e, parsed);
    }

    #[test]
    fn context_added_type_name() {
        let p = EventPayload::ContextAdded {
            key: "scope.md".to_string(),
            hash: "abc".to_string(),
            size: 42,
        };
        assert_eq!(p.type_name(), "context_added");
    }

    // ===== Issue 4: --rationale on directed_transition and rewound =====

    #[test]
    fn directed_transition_with_rationale_round_trip() {
        let e = Event {
            seq: 8,
            timestamp: "2026-05-01T12:00:00.000Z".to_string(),
            event_type: "directed_transition".to_string(),
            payload: EventPayload::DirectedTransition {
                from: "analysis".to_string(),
                to: "implementation".to_string(),
                rationale: Some("Design approved by stakeholders".to_string()),
            },
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"rationale\":\"Design approved by stakeholders\""));
        let parsed: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(e, parsed);
    }

    #[test]
    fn directed_transition_without_rationale_omits_field() {
        let e = Event {
            seq: 9,
            timestamp: "2026-05-01T12:00:00.000Z".to_string(),
            event_type: "directed_transition".to_string(),
            payload: EventPayload::DirectedTransition {
                from: "analysis".to_string(),
                to: "implementation".to_string(),
                rationale: None,
            },
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(
            !json.contains("rationale"),
            "rationale must be absent when None, got {}",
            json
        );
        let parsed: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(e, parsed);
    }

    #[test]
    fn old_directed_transition_event_without_rationale_deserializes() {
        // Pre-feature directed_transition events that have no rationale field
        // must continue to parse correctly.
        let json = r#"{"seq":1,"timestamp":"2026-01-01T00:00:00Z","type":"directed_transition","payload":{"from":"a","to":"b"}}"#;
        let parsed: Event = serde_json::from_str(json).expect("pre-feature event must parse");
        match parsed.payload {
            EventPayload::DirectedTransition { rationale, .. } => {
                assert!(rationale.is_none());
            }
            other => panic!("expected DirectedTransition, got {:?}", other),
        }
    }

    #[test]
    fn rewound_with_rationale_round_trip() {
        let e = Event {
            seq: 10,
            timestamp: "2026-05-01T12:00:00.000Z".to_string(),
            event_type: "rewound".to_string(),
            payload: EventPayload::Rewound {
                from: "implementation".to_string(),
                to: "analysis".to_string(),
                rationale: Some("Scope changed after review".to_string()),
            },
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"rationale\":\"Scope changed after review\""));
        let parsed: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(e, parsed);
    }

    #[test]
    fn rewound_without_rationale_omits_field() {
        let e = Event {
            seq: 11,
            timestamp: "2026-05-01T12:00:00.000Z".to_string(),
            event_type: "rewound".to_string(),
            payload: EventPayload::Rewound {
                from: "implementation".to_string(),
                to: "analysis".to_string(),
                rationale: None,
            },
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(
            !json.contains("rationale"),
            "rationale must be absent when None, got {}",
            json
        );
        let parsed: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(e, parsed);
    }

    #[test]
    fn old_rewound_event_without_rationale_deserializes() {
        // Pre-feature rewound events that have no rationale field must
        // continue to parse correctly.
        let json = r#"{"seq":1,"timestamp":"2026-01-01T00:00:00Z","type":"rewound","payload":{"from":"b","to":"a"}}"#;
        let parsed: Event = serde_json::from_str(json).expect("pre-feature event must parse");
        match parsed.payload {
            EventPayload::Rewound { rationale, .. } => {
                assert!(rationale.is_none());
            }
            other => panic!("expected Rewound, got {:?}", other),
        }
    }

    #[test]
    fn unknown_event_type_deserializes_to_unknown_variant() {
        let json = r#"{"seq":3,"timestamp":"2026-01-01T00:00:00Z","type":"pause_requested","payload":{"reason":"user request"}}"#;
        let parsed: Event =
            serde_json::from_str(json).expect("unknown event type must parse without error");
        match parsed.payload {
            EventPayload::Unknown {
                type_name,
                raw_payload,
            } => {
                assert_eq!(type_name, "pause_requested");
                assert_eq!(raw_payload["reason"], "user request");
            }
            other => panic!("expected Unknown, got {:?}", other),
        }
        assert_eq!(parsed.event_type, "pause_requested");
    }

    #[test]
    fn unknown_event_type_name_returns_unknown() {
        let p = EventPayload::Unknown {
            type_name: "future_event".to_string(),
            raw_payload: serde_json::json!({}),
        };
        assert_eq!(p.type_name(), "unknown");
    }

    #[test]
    fn unknown_event_serializes_with_original_type_string() {
        // koto query --events output must show the original type string, not "unknown".
        let e = Event {
            seq: 5,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: "future_event".to_string(),
            payload: EventPayload::Unknown {
                type_name: "future_event".to_string(),
                raw_payload: serde_json::json!({"field": "value"}),
            },
        };
        let json = serde_json::to_string(&e).unwrap();
        let val: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            val["type"], "future_event",
            "type field must be original type string, not 'unknown'"
        );
    }

    // ===== Issue 2 (new): intent_updated event and derive_intent =====

    #[test]
    fn derive_intent_returns_none_when_no_intent_events() {
        use super::{derive_intent, Event};
        let events: Vec<Event> = vec![];
        assert_eq!(derive_intent(&events), None);
    }

    #[test]
    fn derive_intent_returns_last_intent() {
        use super::{derive_intent, Event, EventPayload};
        fn make_intent_event(seq: u64, intent: &str) -> Event {
            Event {
                seq,
                timestamp: "2026-01-01T00:00:00Z".to_string(),
                event_type: "intent_updated".to_string(),
                payload: EventPayload::IntentUpdated {
                    intent: intent.to_string(),
                },
            }
        }
        let events = vec![
            make_intent_event(1, "first intent"),
            make_intent_event(2, "second intent"),
        ];
        assert_eq!(derive_intent(&events), Some("second intent".to_string()));
    }

    #[test]
    fn intent_updated_roundtrips() {
        use super::{Event, EventPayload};
        let json = r#"{"seq":1,"timestamp":"2026-01-01T00:00:00Z","type":"intent_updated","payload":{"intent":"test goal"}}"#;
        let event: Event = serde_json::from_str(json).unwrap();
        assert!(
            matches!(event.payload, EventPayload::IntentUpdated { ref intent } if intent == "test goal")
        );
        let serialized = serde_json::to_string(&event).unwrap();
        assert!(serialized.contains("intent_updated"));
        assert!(serialized.contains("test goal"));
    }

    // ===== Issue 3: ValidatedSessionId / ValidatedCoordId newtypes =====

    use super::{EngineError, ValidatedCoordId, ValidatedSessionId};

    fn assert_invalid_session(input: &str, expected_reason_substr: &str) {
        match ValidatedSessionId::new(input) {
            Err(EngineError::InvalidSessionId { reason, .. }) => assert!(
                reason.contains(expected_reason_substr),
                "expected reason to contain `{}`, got `{}` (input={:?})",
                expected_reason_substr,
                reason,
                input
            ),
            Err(other) => panic!("expected InvalidSessionId for {:?}, got {:?}", input, other),
            Ok(v) => panic!("expected rejection for {:?}, got Ok({})", input, v),
        }
    }

    fn assert_invalid_coord(input: &str, expected_reason_substr: &str) {
        match ValidatedCoordId::new(input) {
            Err(EngineError::InvalidCoordId { reason, .. }) => assert!(
                reason.contains(expected_reason_substr),
                "expected reason to contain `{}`, got `{}` (input={:?})",
                expected_reason_substr,
                reason,
                input
            ),
            Err(other) => panic!("expected InvalidCoordId for {:?}, got {:?}", input, other),
            Ok(v) => panic!("expected rejection for {:?}, got Ok({})", input, v),
        }
    }

    #[test]
    fn validated_session_id_accepts_typical_input() {
        let v = ValidatedSessionId::new("scrutineer-a").expect("must accept");
        assert_eq!(v.as_str(), "scrutineer-a");
        assert_eq!(v.to_string(), "scrutineer-a");
    }

    #[test]
    fn validated_session_id_accepts_alphanumeric_and_punct() {
        // Compound identifiers used in production session names.
        for ok in [
            "abc",
            "a",
            "A1.b2_c3-d4",
            "parent.task-1",
            "0a",
            "Z_z",
            "a.b.c",
            "0-1.2_3",
        ] {
            ValidatedSessionId::new(ok)
                .unwrap_or_else(|e| panic!("expected accept for {:?}, got {:?}", ok, e));
        }
    }

    #[test]
    fn validated_session_id_rejects_empty() {
        assert_invalid_session("", "empty");
    }

    #[test]
    fn validated_session_id_rejects_leading_dot() {
        assert_invalid_session(".hidden", "leading dot");
    }

    #[test]
    fn validated_session_id_rejects_leading_hyphen() {
        assert_invalid_session("-flag", "leading hyphen");
    }

    #[test]
    fn validated_session_id_rejects_path_traversal() {
        // Both the leading-dot branch and the embedded `/` would
        // reject this; the constructor catches the leading-dot
        // branch first, which is fine — the goal is that the input
        // is never accepted.
        assert_invalid_session("../etc/passwd", "leading dot");
    }

    #[test]
    fn validated_session_id_rejects_shell_metacharacters() {
        // Spaces, semicolons, and shell special characters all map
        // to the regex's disallowed-characters branch.
        for evil in [
            "foo; rm -rf /",
            "a b",
            "a|b",
            "a&b",
            "a$b",
            "a`b",
            "a\"b",
            "a'b",
            "a\\b",
            "a/b",
            "a>b",
            "a<b",
            "a\nb",
            "a\tb",
        ] {
            assert_invalid_session(evil, "disallowed");
        }
    }

    #[test]
    fn validated_session_id_rejects_overlength() {
        let s = "a".repeat(256);
        assert_invalid_session(&s, "exceeds 255");
        // 255 characters must be accepted (boundary).
        let s_ok = "a".repeat(255);
        ValidatedSessionId::new(&s_ok).expect("255-char input must be accepted");
    }

    #[test]
    fn validated_session_id_preview_truncates_long_input() {
        let long_input = format!("{}!", "a".repeat(80));
        match ValidatedSessionId::new(&long_input) {
            Err(EngineError::InvalidSessionId { input_preview, .. }) => {
                // 64 leading chars + literal "..." suffix.
                assert!(
                    input_preview.ends_with("..."),
                    "expected truncation suffix, got {:?}",
                    input_preview
                );
                assert!(
                    input_preview.len() <= 64 + 3,
                    "preview must be bounded, got {} chars",
                    input_preview.len()
                );
            }
            other => panic!("expected InvalidSessionId, got {:?}", other),
        }
    }

    #[test]
    fn validated_coord_id_accepts_typical_input() {
        let v = ValidatedCoordId::new("coord-7").expect("must accept");
        assert_eq!(v.as_str(), "coord-7");
        assert_eq!(v.to_string(), "coord-7");
    }

    #[test]
    fn validated_coord_id_rejects_empty() {
        assert_invalid_coord("", "empty");
    }

    #[test]
    fn validated_coord_id_rejects_leading_dot() {
        assert_invalid_coord(".hidden", "leading dot");
    }

    #[test]
    fn validated_coord_id_rejects_leading_hyphen() {
        assert_invalid_coord("-flag", "leading hyphen");
    }

    #[test]
    fn validated_coord_id_rejects_shell_metacharacters() {
        for evil in ["foo; rm -rf /", "a b", "a|b", "a/b", "a$b"] {
            assert_invalid_coord(evil, "disallowed");
        }
    }

    #[test]
    fn validated_coord_id_rejects_overlength() {
        let s = "a".repeat(256);
        assert_invalid_coord(&s, "exceeds 255");
        let s_ok = "a".repeat(255);
        ValidatedCoordId::new(&s_ok).expect("255-char input must be accepted");
    }

    // ===== Issue 3: exit-code mapping for new EngineError variants =====

    #[test]
    fn engine_error_epoch_fence_violation_exit_code_is_65() {
        let e = EngineError::EpochFenceViolation {
            child_session_id: "child-1".to_string(),
            expected: 4,
            presented: 3,
        };
        assert_eq!(e.exit_code(), 65);
    }

    #[test]
    fn engine_error_redelegation_cap_exceeded_exit_code_is_75() {
        let e = EngineError::RedelegationCapExceeded {
            child_session_id: "child-1".to_string(),
            cap: 10,
        };
        assert_eq!(e.exit_code(), 75);
    }

    #[test]
    fn engine_error_state_file_corrupted_exit_code_is_3() {
        let e = EngineError::StateFileCorrupted("bad".to_string());
        assert_eq!(e.exit_code(), 3);
    }

    #[test]
    fn engine_error_invalid_session_id_exit_code_is_1() {
        // Not pinned to a sysexit value by the design; defaults to 1.
        let e = EngineError::InvalidSessionId {
            reason: "test".to_string(),
            input_preview: "x".to_string(),
        };
        assert_eq!(e.exit_code(), 1);
    }

    #[test]
    fn engine_error_default_exit_code_is_1() {
        let e = EngineError::EmptyLog;
        assert_eq!(e.exit_code(), 1);
    }
}
