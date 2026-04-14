<!-- decision:start id="error-envelope-validation-timing" status="confirmed" -->
### Decision 11: Error envelope, validation timing, and batch-edge validation (combined)

**Context**

Round 1 walkthrough cluster D (pairs 3a / 3b / 3c) and cluster I surfaced
a dozen adjoining gaps that all live on one surface: the shape of an
error response from `koto next`, when in the advance loop validation
runs, what codes an agent can pattern-match on, and which small edge
cases R1-R7 currently fail to cover. The sub-questions are decided
together because they share one JSON envelope and one validation phase;
splitting them produced inconsistent answers across the three round-1
transcripts.

The existing surface anchors every choice:

- `src/cli/next_types.rs:283-289` already defines
  `NextError { code: NextErrorCode, message: String, details: Vec<ErrorDetail> }`
  with nine `NextErrorCode` variants, each mapped to exit codes 1/2/3.
- The design at line 1610 writes `NextError::Batch { kind, message }`
  which is enum-variant-shaped and does not fit the existing struct.
- Design line 1597-1608 defines `BatchError` with a free-string
  `InvalidBatchDefinition.reason`, a single-error `SchedulerOutcome::Error`,
  and `LimitExceeded.which: &'static str`.
- Phase 3 Implementation prose says "submission-time hard limit
  enforcement" (pre-append). Data Flow Step 4 shows
  `EvidenceSubmitted` appended before R1-R7 run. The two are in direct
  contradiction.
- The walkthrough's response-envelope table enumerates `evidence_required`,
  `gate_blocked`, `done`, and `initialized` actions. No `error` action is
  defined, yet the design's response envelope has a `scheduler` additive
  field that makes no sense on a rejected submission.

This decision answers twelve sub-questions as one combined commitment
because the answers are tightly coupled: the envelope shape (Q1)
determines whether a typed `reason` enum (Q8) sits under `error.details`
or elsewhere; the validation timing (Q2) determines whether rejected
submissions leave `EvidenceSubmitted` events, which determines whether
`SchedulerRan` (Q5) is the authoritative audit trail for batch
activity. Treating them in one report lets the cross-validation pass
check one contract instead of twelve.

**Assumptions**

- The existing `NextError` struct shape is a stable public contract of
  the koto CLI as of v0.7.0; breaking it would require a major-version
  bump and affect every existing consumer (every koto-user workflow).
  Any decision that keeps the existing shape is strictly cheaper than
  any decision that changes it.
- Agents pattern-match on string literals at multiple layers (top-level
  `action`, `error.code`, and nested discriminators). Adding a new
  nested discriminator is backward-compatible; changing an existing
  string is not.
- Decision 10 (mutation semantics) will introduce a
  `SpawnedTaskMutated` rejection reason. Decision 9 (retry mechanism)
  will introduce an `InvalidRetryRequest` variant covering premature
  retry, empty child list, and non-failed non-skipped children in the
  retry set. The envelope chosen here must accommodate both without
  re-litigation.
- koto's state file is append-only. No "rollback" primitive exists;
  "unappending" an event is a contradiction in terms. Any validation
  step that happens after an append is a promise koto cannot keep on
  crash-resume.
- The existing advance loop already runs evidence validation (type
  checks, required-field presence, `accepts` field matching) *before*
  appending `EvidenceSubmitted` — R1-R7 batch-definition checks are
  structurally analogous (both are pure functions of the submitted
  payload) and should sit in the same pre-append slot.
- `ErrorDetail` is `{ field: String, reason: String }` today. Extending
  `ErrorDetail` to carry structured per-variant fields breaks existing
  consumers. The decision keeps `ErrorDetail` unchanged and puts
  batch-specific structure in a sibling `error.batch` object.

**Research findings**

1. **Existing CLI contract is richer than the design acknowledges.**
   `NextErrorCode` already has `InvalidSubmission` (exit 2),
   `TemplateError` (exit 3), `PersistenceError` (exit 3),
   `IntegrationUnavailable` (exit 1). Every `BatchError` variant maps
   cleanly to one of these without introducing new top-level codes.
   Reading `src/cli/next_types.rs:306-325` confirms: transient vs
   caller-error vs infra is already the axis of the exit-code table,
   and every batch-error is classifiable on that axis.

2. **The walkthrough's "additive `scheduler` field" principle is
   violated on error responses.** Design lines 1587-1594 say the
   scheduler output is additive on success responses. But error
   responses have no scheduler output because no scheduler ran. Omitting
   the field is trivially correct; setting `scheduler: null` preserves
   a uniform shape. Both work; `null` is preferable because agents can
   destructure the field unconditionally.

3. **Pre-append validation is reversible on crash; post-append is not.**
   A concrete walkthrough:
   - Pre-append, crash mid-validation: nothing appended, next `koto next`
     re-reads the same un-appended payload (or the agent retries). No
     orphan events. Zero recovery code.
   - Post-append, crash after `EvidenceSubmitted` but before
     `Rejected`-marker append: on resume, the advance loop sees an
     `EvidenceSubmitted` that passed syntactic checks (because it was
     appended) and re-enters validation. If validation is deterministic
     (R1-R7 are), the re-run produces the same rejection — but now the
     event log permanently contains a submission that the design claims
     was "rejected before any new spawn happens". Every downstream
     consumer (`koto query`, `derive_batch_view`, audit tooling) must
     learn to filter rejected submissions. That's additive complexity
     on every read path forever, to buy an audit trail that
     `SchedulerRan` can provide instead.

4. **Existing CI tests read `action` exhaustively.** `NextResponse`'s
   custom `Serialize` impl (lines 256-280) already writes `action` on
   every variant and sets `error: null` on non-error variants. Adding
   an `action: "error"` variant is a one-enum-variant extension to
   `NextResponse` + a matching `Serialize` arm. The existing code
   already emits a literal `"error"` key on every other variant
   (`error: None`), so the shape is compatible — consumers that always
   read `error` see `null` today and a populated object on error.

5. **Typed discriminators survive serde better than free strings.**
   A rust `enum Reason { Cycle, DanglingRefs, ... }` with
   `#[serde(rename_all = "snake_case", tag = "reason", content = "detail")]`
   renders deterministic JSON and compiles away typos. A `String` named
   `"Cycle"` vs `"cycle"` vs `"cycle_detected"` is a guaranteed
   incident over a long enough timeline.

6. **`LimitExceeded.which: &'static str` is a false-comfort signature.**
   `&'static str` constrains the *literal* to a compile-time string but
   not the *value* to any fixed set. Agents have no contract on what
   strings are valid. Swapping to a typed `enum LimitKind { Tasks,
   WaitsOn, Depth, PayloadBytes }` gives serde renaming + exhaustive
   match guarantees at no cost.

**Chosen: Unified error envelope with `action: "error"`, typed
batch-kind discriminator nested in `error.batch`, pre-append
validation for all batch-definition checks, and typed enums replacing
every `String`/`&'static str` in `BatchError`**

### Q1. Error response JSON shape (three options considered)

**Option A — Separate error envelope via new `NextError::Batch` variant
(design as written).** Add an enum-shaped variant to `NextError` that
doesn't match the existing struct. Rejected: breaks the existing public
shape. Every consumer of `NextError` today unpacks `code` / `message` /
`details`. An enum-variant alongside a struct-shape is not a valid Rust
`NextError` — the design's line 1610 is internally inconsistent.

**Option B — Reuse existing `NextError` struct, squash batch fields into
`details[0].reason` as a JSON-serialized string.** Rejected: forces
agents to parse strings inside strings. Fragile, untyped, violates the
"machine-parseable" constraint.

**Chosen: Option C — Reuse existing `NextError` struct; add a sibling
`batch` field alongside `details` for structured batch context; add
`action: "error"` to the top-level response envelope.**

The top-level envelope on an error response:

```json
{
  "action": "error",
  "state": "plan_and_await",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Batch definition rejected: cycle in waits_on graph",
    "details": [
      {"field": "tasks", "reason": "cycle"}
    ],
    "batch": {
      "kind": "invalid_batch_definition",
      "reason": "cycle",
      "cycle": ["issue-A", "issue-B", "issue-A"]
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

Shape notes:

- `action: "error"` is the new seventh response variant on
  `NextResponse`.
- `state` is the current template state (never null unless the workflow
  is not yet initialized — in which case `code: "workflow_not_initialized"`).
- `advanced: false` on all error responses — rejection never advances
  state.
- `error.code` is an existing `NextErrorCode` variant, snake_cased by
  serde. Batch-specific errors map as:
  - `InvalidBatchDefinition` → `invalid_submission` (exit 2)
  - `LimitExceeded` → `invalid_submission` (exit 2)
  - `TemplateResolveFailed` at compile-time → `template_error` (exit 3)
  - `SpawnFailed` / `TemplateResolveFailed` at runtime → **not a
    rejection** — per-task errors surface through `scheduler.errored`
    instead (see Q4). Top-level `error` is never populated for
    per-task spawn failures.
  - `BackendError` → `integration_unavailable` (exit 1, retryable)
- `error.details` keeps its existing `[{field, reason}]` shape. Agents
  that already parse this keep working.
- `error.batch` is a new optional field, populated only when the error
  originates from batch logic. Its `kind` is the `BatchError` variant
  name in snake_case; typed fields sit alongside per `kind`. See Q8 for
  the full table.
- `blocking_conditions` is always `[]` on errors — no gate evaluated.
- `scheduler: null` preserves the additive-field invariant.

All twelve sub-question JSON samples below use this one envelope.

### Q2. Pre-append vs post-append validation (two options considered)

**Option A — Post-append (design Data Flow Step 4 as drawn).** Append
`EvidenceSubmitted` first, then run R1-R7. On rejection, append a
`SubmissionRejected` marker event. Rejected: every downstream read path
must filter rejected events forever. Crash-resume semantics are
contorted — on resume between the two appends, validation re-runs and
must reproduce the same outcome, but the event log is already dirty.

**Chosen: Option B — Pre-append (Phase 3 Implementation prose).** Run
R1-R7 + hard-limit checks as pure functions of the submitted payload
*before* the advance loop calls `append_event(EvidenceSubmitted)`. On
rejection, no state file writes occur; the response envelope carries
the error and the parent workflow is exactly as it was before the call.

Crash-resume walkthrough confirming pre-append is sound:

```
t=0   agent runs `koto next coord --with-data @bad.json`
t=1   koto reads state file, computes current state = plan_and_await
t=2   koto validates `bad.json` against tasks-field `accepts` schema (OK)
t=3   koto runs R1-R7 on the parsed task list (cycle detected)
-- CRASH HERE --
t=4   (after restart) agent retries the same `koto next` call
t=5   koto re-reads state file (unchanged, no new events since t=0)
t=6   koto re-validates (same bad.json, same cycle detection)
t=7   koto returns same error response
```

Zero state divergence. No "poison" events. Rejected submission leaves
**nothing** in the parent's event log and does **not** advance state.

This also resolves pair 3b's Finding 4: the invariant is explicit —
"R1-R7 + hard-limit checks run before `EvidenceSubmitted` is appended.
A failed check returns an error response with zero state-file writes."

Data Flow Step 4 must be rewritten to show the validation-then-append
order explicitly. Delete the Step 4 implication that `EvidenceSubmitted`
is written first.

**Consequence for Q5 (SchedulerRan event):** because rejected submissions
leave no log trace at all, the audit question shifts. The answer is that
per-tick scheduler decisions *do* get a `SchedulerRan` event in the log
(see Q5 below) — this is where audit trails live for batch activity.
Rejection audit (the "why did my submission fail?" question) is
intentionally ephemeral: the rejection is observable only in the
response of the tick that produced it. Operators use shell history or
agent logs for forensic replay of rejections; koto's event log is
reserved for accepted state.

### Q3. `NextError::Batch { kind, message }` reconciliation (two options)

**Option A — Break backward compatibility; rewrite `NextError` as an
enum.** Rejected: every existing consumer breaks. This is a
v1.0.0-scale change that the design has no mandate for.

**Chosen: Option B — Delete design line 1610's `NextError::Batch`
reference. Reuse the existing `NextError` struct unchanged. Put
batch-specific fields in a sibling `error.batch` object (Q1's
envelope).** The existing `code`, `message`, `details` do the work they
do today. Batch structure is additive.

The design doc must be updated at line 1610 to read:

```
// BatchError maps to NextError via the following table (see Decision 11
// for the full envelope shape):
//
//   InvalidBatchDefinition → NextError { code: InvalidSubmission, ... }
//   LimitExceeded          → NextError { code: InvalidSubmission, ... }
//   TemplateResolveFailed  → NextError { code: TemplateError, ... } (compile-time only)
//   BackendError           → NextError { code: IntegrationUnavailable, ... }
//
// Runtime per-task spawn failures (SpawnFailed, runtime TemplateResolveFailed)
// do not promote to a top-level NextError; they surface in scheduler.errored.
```

### Q4. Halt-on-error vs accumulate (two options considered)

**Option A — Halt-on-first-error.** First per-task spawn failure
aborts the tick; scheduler returns `SchedulerOutcome::Error { reason }`;
no other ready task is attempted this tick. Rejected: produces a
"one bad apple" failure mode where a typo'd `template:` on task-7
prevents task-1 through task-6 from ever spawning. Agents are forced
to bisect via trial submissions.

**Chosen: Option B — Accumulate per-task; never halt.** The scheduler
iterates all currently-ready tasks. Each task either spawns, is
skipped (dep-failure), or fails to spawn. Failures are collected into
`SchedulerOutcome::Scheduled.errored` (a new field). The scheduler
returns `Scheduled` even when every task failed to spawn — the tick
completed; it just produced no successful spawns. `SchedulerOutcome::Error`
is reserved for failures that invalidate the *entire tick* (backend
list failure during classification — nothing can be known, so nothing
can be reported).

Concrete shape changes:

```rust
pub enum SchedulerOutcome {
    NoBatch,
    Scheduled {
        spawned: Vec<String>,
        already: Vec<String>,
        blocked: Vec<String>,
        skipped: Vec<String>,
        errored: Vec<TaskSpawnError>,   // NEW
    },
    Error { reason: BatchError },       // RESERVED for tick-wide failures
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskSpawnError {
    pub task: String,
    pub kind: SpawnErrorKind,           // typed enum
    pub paths_tried: Option<Vec<String>>, // for TemplateResolveFailed
    pub message: String,                // human-readable diagnostic
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpawnErrorKind {
    TemplateNotFound,
    TemplateCompileFailed,
    Collision,              // EEXIST on init_state_file
    BackendUnavailable,     // transient
    PermissionDenied,
    IoError,                // catch-all
}
```

`BatchTaskView.outcome` gains a `spawn_failed` variant:

```rust
pub enum BatchTaskOutcome {
    Pending,
    Success,
    Failed,
    Skipped,
    Blocked,
    SpawnFailed,    // NEW
}
```

Response shape on a partial-success tick:

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "output": {
      "total": 3, "completed": 0, "pending": 2,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 0,
      "spawn_failed": 1,
      "all_complete": false,
      "children": [
        {"name": "coord.issue-A", "state": "working", "outcome": "pending"},
        {"name": "coord.issue-B", "state": null, "outcome": "pending"},
        {"name": "coord.issue-C", "state": null, "outcome": "spawn_failed",
         "spawn_error": {"kind": "template_not_found",
                         "paths_tried": ["/repo/templates/does-not-exist.md"]}}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": ["coord.issue-A", "coord.issue-B"],
    "already": [],
    "blocked": [],
    "skipped": [],
    "errored": [
      {"task": "issue-C",
       "kind": "template_not_found",
       "paths_tried": ["/repo/templates/does-not-exist.md"],
       "message": "Template not found in template_source_dir or submitter_cwd"}
    ]
  }
}
```

(Decision 12 renames `spawned` → `spawned_this_tick`; this decision
assumes that rename lands and uses the new name throughout.)

### Q5. `SchedulerRan` event in the parent log (two options)

**Option A — No event; scheduler decisions are ephemeral.** Rejected:
partial spawn failures become un-auditable. A user investigating "why
did half my tasks not spawn" has no record.

**Chosen: Option B — Append `SchedulerRan` event on every non-trivial
tick.**

```rust
Event::SchedulerRan {
    seq: u64,
    timestamp: DateTime,
    spawned: Vec<String>,
    already: Vec<String>,
    blocked: Vec<String>,
    skipped: Vec<String>,
    errored: Vec<TaskSpawnError>,
}
```

Append semantics:

- **Append when** at least one of `spawned`, `skipped`, `errored` is
  non-empty. This captures every state-changing or forensically
  interesting tick.
- **Skip append when** the scheduler produced `NoBatch` or when every
  task is `already` / `blocked` with zero changes. This prevents log
  bloat from repeated no-op `koto next` polls.
- The event **does not** carry `all_complete` or the gate output —
  that's re-derivable from the batch view. Keep `SchedulerRan` payload
  minimal: what the scheduler chose to do this tick.
- Does **not** get appended on rejected submissions (see Q2).

Consumers:

- `koto query --events` shows `SchedulerRan` alongside `EvidenceSubmitted`
  and `Transitioned`, giving a complete picture of batch activity.
- `derive_batch_view` does not need to read `SchedulerRan` — it
  continues to derive state from child state files directly. The event
  is for audit, not for correctness.

### Q6. Empty task list `{"tasks": []}` (two options)

**Option A — Valid immediately-complete batch.** Accepts empty, the
gate `children-complete` returns `all_complete: true` immediately, the
parent advances on the next tick. Rejected: collapses "I forgot to add
tasks" with "I intentionally want zero tasks". Silently advancing a
mis-submitted empty batch is a footgun.

**Chosen: Option B — Reject as validation error.** Add **R0:
`tasks.len() >= 1`**. An empty task list fails pre-append validation
with:

```json
{
  "action": "error",
  "state": "plan_and_await",
  "error": {
    "code": "invalid_submission",
    "message": "Task list is empty",
    "details": [{"field": "tasks", "reason": "empty"}],
    "batch": {
      "kind": "invalid_batch_definition",
      "reason": "empty_task_list"
    }
  }
}
```

Template authors who genuinely want a "maybe zero tasks" flow should
model it via a gate that skips `materialize_children` entirely (e.g., a
separate `check_if_any_work` state with a transition on
`any_work == false` routing straight past the batch state). This is
idiomatic and doesn't require koto to special-case the empty case.

### Q7. `template: null` vs omitted (two options)

**Option A — `null` is an error ("template field must be a string or
omitted").** Rejected: pedantic. JSON producers naturally serialize
absent Optional fields as either `null` or by omitting; rejecting one
form creates a pointless distinction.

**Chosen: Option B — `null` is equivalent to omitted: inherits the
hook's `default_template`.** Document in Decision 1:

> Task entries may omit the `template` field, supply `null`, or supply
> a non-empty string. Omitted and `null` are equivalent: the task
> inherits `materialize_children.default_template`. A non-empty string
> overrides the default. An empty string (`""`) is a validation error
> (R4.1: template path, if supplied, must be non-empty).

### Q8. `InvalidBatchDefinition.reason` as typed enum (two options)

**Option A — Keep free string; agents parse prose.** Rejected: fails
the "machine-parseable" constraint.

**Chosen: Option B — Typed enum with per-variant payload.**

```rust
pub enum BatchError {
    InvalidBatchDefinition {
        reason: InvalidBatchReason,
    },
    SpawnFailed { task: String, kind: SpawnErrorKind, message: String },
    TemplateResolveFailed { task: String, kind: TemplateResolveKind, paths_tried: Vec<String> },
    BackendError { message: String, retryable: bool },
    LimitExceeded { which: LimitKind, limit: usize, actual: usize },
    InvalidRetryRequest {                 // populated by Decision 9
        reason: InvalidRetryReason,
    },
}

#[derive(Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum InvalidBatchReason {
    EmptyTaskList,                                       // R0
    Cycle { cycle: Vec<String> },                        // R3
    DanglingRefs { entries: Vec<DanglingRef> },          // R4
    DuplicateNames { duplicates: Vec<String> },          // R5
    InvalidName { task: String, detail: InvalidNameDetail }, // R9 (new)
    ReservedNameCollision { task: String, reserved: String }, // R9 (new)
    TriggerRuleUnsupported { task: String, rule: String }, // R7
    SpawnedTaskMutated {                                 // Decision 10
        task: String,
        changed_fields: Vec<String>,
    },
    LimitExceededTasks { limit: usize, actual: usize },  // R6 tasks
    LimitExceededWaitsOn { task: String, limit: usize, actual: usize }, // R6 edges
    LimitExceededDepth { limit: usize, actual: usize },  // R6 depth
}

#[derive(Serialize)]
#[serde(tag = "detail", rename_all = "snake_case")]
pub enum InvalidNameDetail {
    Empty,
    InvalidChars { pattern: String },
    TooLong { limit: usize, actual: usize },
}
```

Full mapping table:

| `reason` tag | Rule | Typed fields | Example |
|--|--|--|--|
| `empty_task_list` | R0 | (none) | `{"reason": "empty_task_list"}` |
| `cycle` | R3 | `cycle: Vec<String>` | `{"reason": "cycle", "cycle": ["A","B","A"]}` |
| `dangling_refs` | R4 | `entries: [{task, missing}]` | `{"reason": "dangling_refs", "entries": [{"task":"D","missing":["x"]}]}` |
| `duplicate_names` | R5 | `duplicates: Vec<String>` | `{"reason": "duplicate_names", "duplicates": ["A"]}` |
| `invalid_name` | R9 | `task, detail: {...}` | `{"reason":"invalid_name","task":"","detail":{"detail":"empty"}}` |
| `reserved_name_collision` | R9 | `task, reserved: String` | `{"reason":"reserved_name_collision","task":"retry_failed","reserved":"retry_failed"}` |
| `trigger_rule_unsupported` | R7 | `task, rule: String` | `{"reason":"trigger_rule_unsupported","task":"B","rule":"any_success"}` |
| `spawned_task_mutated` | Decision 10 R8 | `task, changed_fields` | `{"reason":"spawned_task_mutated","task":"A","changed_fields":["vars.X"]}` |
| `limit_exceeded_tasks` | R6 | `limit, actual` | `{"reason":"limit_exceeded_tasks","limit":1000,"actual":1001}` |
| `limit_exceeded_waits_on` | R6 | `task, limit, actual` | |
| `limit_exceeded_depth` | R6 | `limit, actual` | |

R1 (child template compilable) and R2 (vars resolve) are **not** in
`InvalidBatchDefinition` — they are per-task runtime checks that
surface via `SchedulerOutcome.errored` with
`SpawnErrorKind::TemplateCompileFailed` and the like. That matches
Cluster H's "per-task not halt-submission" recommendation and this
decision's Q4 choice.

### Q9. Reserved-name validation (R9) (two options)

**Option A — Strict alphanumeric: `[a-z0-9-]+` (kebab-case only).**
Rejected: excludes mixed-case names like `issue-PR-42` and
underscore-separated names that template authors already write in
worked examples.

**Chosen: Option B — Permissive identifier regex with explicit
reserved-name list.**

R9 runtime rules (applied pre-append):

1. `task.name` matches `^[A-Za-z0-9_-]+$`.
2. `task.name.len() >= 1` and `task.name.len() <= 64`.
3. `task.name` is not in the reserved set:
   `{"retry_failed", "cancel_tasks"}`. Decision 9 / 10 may extend this
   set; any extension updates this list.
4. Validation applies to `task.name` (the **short name**), not to the
   computed `<parent>.<task.name>` full name. Rationale: agents submit
   short names; errors should point at what agents wrote. The parent
   prefix is appended by koto and is already known to be valid.

Two concrete rejections:

```json
{"reason": "invalid_name", "task": "issue A", "detail": {"detail": "invalid_chars", "pattern": "^[A-Za-z0-9_-]+$"}}
{"reason": "reserved_name_collision", "task": "retry_failed", "reserved": "retry_failed"}
```

### Q10. `LimitExceeded.which` as enum (two options)

**Option A — Keep `&'static str`.** Rejected: untyped by contract,
typed by coincidence.

**Chosen: Option B — Typed enum.**

```rust
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitKind {
    Tasks,          // R6: total task count
    WaitsOn,        // R6: edges per task
    Depth,          // R6: DAG depth (longest root-to-leaf)
    PayloadBytes,   // future: total submission size
}
```

Note: `InvalidBatchReason` in Q8 already splits the three active limits
into three discrete reasons (`limit_exceeded_tasks`,
`limit_exceeded_waits_on`, `limit_exceeded_depth`) for direct agent
dispatch. `LimitKind` is the Rust-internal discriminator; it does not
appear on the wire. Agents read `error.batch.reason` directly.

### Q11. `InvalidRetryRequest` variant (two options)

**Option A — Roll premature-retry into existing `InvalidBatchDefinition`
reasons.** Rejected: premature retry and empty-retry-set are not
batch-definition problems — they're retry-request problems. Crammed
into the same enum, the error message collapses.

**Chosen: Option B — New `InvalidBatchReason` does not cover retry;
retry rejections use a sibling `BatchError::InvalidRetryRequest` with
its own `reason` enum.**

```rust
#[derive(Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum InvalidRetryReason {
    NoBatchMaterialized,              // premature retry
    EmptyChildList,                    // retry_failed with empty set
    ChildNotEligible {                 // child is running/success, not failed/skipped
        children: Vec<ChildEligibility>,
    },
    // Decision 9 may add: MixedPayload, DoubleRetryWithoutTick, etc.
}

pub struct ChildEligibility {
    pub name: String,
    pub current_outcome: String,   // "running" | "success" | ...
}
```

Wire shape on a premature retry:

```json
{
  "action": "error",
  "error": {
    "code": "invalid_submission",
    "message": "Retry requested before any batch has been materialized",
    "details": [{"field": "retry_failed", "reason": "no_batch_materialized"}],
    "batch": {
      "kind": "invalid_retry_request",
      "reason": "no_batch_materialized"
    }
  }
}
```

The exact retry-edge list (double-retry, running-child guard, mixed
payload) is owned by Decision 9. This decision reserves the envelope
slot and the variant; Decision 9 fills in the concrete edges.

### Q12. `..` in template paths (two options)

**Option A — Surface a warning field in the scheduler output.** Rejected:
warning channels are ill-defined at this layer. `scheduler.warnings` is
new surface area for one case; it invites future growth ("warnings for
everything the agent did weirdly") that the design isn't ready to
scope.

**Chosen: Option B — Silent accept, documented in Security
Considerations only.** Decision 4 already commits to "trusted submitter,
no sandbox". Surfacing a warning at this layer is an inconsistency:
Decision 4 accepts the path; Decision 11 half-takes-it-back with a
warning. If the decision driver says "no sandbox", then also "no
perimeter alarms".

The Security Considerations section of the design doc gets one
additional sentence: "Template paths containing `..` segments are
accepted without warning. A downstream reviewer inspecting a child
state file sees the resolved absolute path; the agent submitting the
path sees no notice. This is consistent with the trusted-submitter
model."

If operational experience surfaces demand for a warning channel, revisit
in a follow-up decision. v1 ships silent.

---

**Summary of concrete changes this decision commits**

Design doc edits (existing design `DESIGN-batch-child-spawning.md`):

1. **Line 1597-1608 (`BatchError` enum).** Replace with the typed-enum
   definition from Q8 + Q10 + Q11. `SpawnFailed.source: anyhow::Error` →
   `SpawnFailed { task, kind: SpawnErrorKind, message: String }`.
   `TemplateResolveFailed` splits into a runtime-only variant carrying
   `kind: TemplateResolveKind`. `LimitExceeded.which: &'static str` →
   `LimitKind`. `InvalidBatchDefinition.reason: String` →
   `InvalidBatchReason`. Add `InvalidRetryRequest { reason:
   InvalidRetryReason }`.

2. **Line 1610 (`NextError::Batch` mapping comment).** Delete the
   `NextError::Batch { kind, message }` reference. Replace with the
   mapping table from Q3.

3. **Line 1583 (`SchedulerOutcome`).** Add `errored:
   Vec<TaskSpawnError>` to the `Scheduled` variant. Keep `Error` reserved
   for tick-wide failures.

4. **Line 1657-1669 (`BatchTaskView`).** Add `SpawnFailed` outcome
   variant; add optional `spawn_error: Option<TaskSpawnError>` field.

5. **Data Flow Step 4.** Rewrite to show validation-then-append order
   explicitly. Delete the implication that `EvidenceSubmitted` is
   written before R1-R7 run.

6. **Decision 1 validation table.** Add R0 (non-empty task list), R9
   (name regex + reserved-name check). Mark all runtime-check rules
   (R0-R9) as pre-append. Document that `template: null` ≡ omitted.

7. **Key Interfaces section / response envelope table.** Add the
   `action: "error"` seventh variant with the envelope shape from Q1.
   Add `Event::SchedulerRan` to the event enum (Q5).

8. **Security Considerations.** One sentence on silent-accept of `..`
   (Q12).

Code changes (new code, doesn't break existing):

1. **`src/cli/next_types.rs`.** Add `NextResponse::Error` variant
   with custom `Serialize`; extend `NextError` with optional `batch:
   Option<BatchErrorContext>` field (serde `skip_serializing_if`).

2. **`src/engine/advance.rs`** (or wherever the advance loop lives).
   Insert R0-R9 pre-append validation pass keyed on the hook's
   `from_field` type being the batch task list.

3. **New module** `src/engine/batch/errors.rs` carrying the typed
   enums from Q8 / Q10 / Q11.

4. **Event log.** Add `SchedulerRan` variant to `Event` enum; update
   serialization + deserialization.

**Rationale summary**

- **One envelope, three discriminator levels.** Top-level `action`
  (error vs success), mid-level `error.code` (which `NextErrorCode`),
  nested `error.batch.kind` + `error.batch.reason` for batch-specific
  dispatch. Every agent knows exactly where to look.
- **Pre-append for all batch-definition checks.** Append-only semantics
  stay clean. Crash-resume is trivially correct. Rejected submissions
  leave zero state.
- **Typed enums everywhere the design had strings.** Agents dispatch
  by serde-renamed snake_case values, not by prose-scraping.
- **Per-task accumulation, never halt.** One bad template doesn't
  kneecap a 500-task batch.
- **`SchedulerRan` carries per-tick audit.** Rejection audit is
  intentionally ephemeral; accepted-state audit is in the log.
- **`InvalidRetryRequest` is reserved, not fully specified.** Decision
  9 owns the retry edges and fills the `InvalidRetryReason` variants;
  this decision reserves the envelope.

**Confidence:** High. Every sub-question has a concrete wire shape
specified or deferred to a named downstream decision. The envelope is
additive over existing `NextError` (backward-compatible). Pre-append
validation is the only choice consistent with append-only state.
Typed enums are strictly cheaper than free strings under every
agent-dispatch scenario.

**Assumptions that could break the decision (watch list for
cross-validation):**

- If Decision 9 lands with a retry mechanism that needs to write
  events before validation (e.g., "retry-accepted marker appended, then
  later rejected on a re-classification"), the pre-append commitment
  here might force Decision 9 into an awkward shape. Likelihood: low —
  Decision 9 is template-transition-based, which already runs *after*
  the advance loop validates.
- If Decision 10 decides that identical-resubmission is a hard
  rejection rather than a no-op, the `InvalidBatchReason` enum gains
  one more variant (`identical_resubmission`). Additive; doesn't
  invalidate the envelope.
- If Decision 12 decides against renaming `scheduler.spawned` →
  `spawned_this_tick`, all JSON samples in Q4 need the field renamed
  back. Shape-neutral.

**Rejected alternatives (for the record)**

- Enum-shaped `NextError::Batch` (breaks existing struct).
- Free-string `reason` fields (fails machine-parseable).
- Post-append validation (crash-resume complexity, forever-filtering).
- Halt-on-first-error scheduler (one-bad-apple footgun).
- Empty task list as immediately-complete batch (silent footgun).
- `template: null` as an error (pedantic).
- Strict kebab-only name regex (excludes legitimate names).
- Warning field for `..` traversal (inconsistent with Decision 4's
  trusted-submitter model).
<!-- decision:end -->
