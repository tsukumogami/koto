# Simulation Round 1 Pair 3b: Error shapes and recovery

Role-play validation of error-response shape and recovery paths for the
batch-child-spawning design. The parent template is the canonical
`coord` from `walkthrough.md`. AGENT is a frustrated template author
iterating on a bad task list. KOTO must emit concrete JSON for every
error -- where the design does not fix the shape, KOTO flags the gap
inline with `[GAP: ...]` and proposes a concrete shape.

Grounding assumptions:

- `NextError` today already has `code: NextErrorCode`, `message:
  String`, `details: Vec<ErrorDetail>` (src/cli/next_types.rs:283-289).
  The design's line 1610 "BatchError maps to NextError::Batch { kind,
  message }" contradicts that shape -- the enum-variant style doesn't
  match the existing struct-style `NextError`. Every response below
  uses the existing struct shape and places BatchError specifics under
  `details`.
- Top-level `scheduler` field is merged in by `handle_next` even on
  error responses, per design lines 1587-1594.

---

## Section 1: Transcript

### Step 1 -- First submission: cycle

tasks.json (A waits on B, B waits on A):

```json
{
  "tasks": [
    {"name": "issue-A", "vars": {"ISSUE_NUMBER": "1"}, "waits_on": ["issue-B"]},
    {"name": "issue-B", "vars": {"ISSUE_NUMBER": "2"}, "waits_on": ["issue-A"]}
  ]
}
```

AGENT: `koto next coord --with-data @tasks.json`

KOTO response:

```json
{
  "action": "error",
  "state": "plan_and_await",
  "error": {
    "code": "invalid_submission",
    "message": "Invalid batch definition: cycle detected in waits_on graph",
    "details": [
      {
        "kind": "batch_error",
        "batch_kind": "invalid_batch_definition",
        "reason": "cycle",
        "cycle": ["issue-A", "issue-B", "issue-A"],
        "field": "tasks"
      }
    ]
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

Gloss: Design says "rejects before any new spawn happens" (line 1864)
but never fixes the envelope. Proposal: reuse existing `NextError`
envelope with `code: invalid_submission` and push BatchError-specific
fields into `details[0]`. `action` is `"error"` (new) because
`evidence_rejected` isn't an existing top-level action in the walkthrough
either.

`[GAP-E1: top-level action for NextError responses is unspecified.
Walkthrough shows "action": "evidence_required", "gate_blocked", "done"
but no error form. Propose "action": "error" as the canonical envelope.]`

`[GAP-E2: BatchError variant discriminator name is unspecified. The
design writes the enum in Rust but doesn't pin the serde tag. Propose
"batch_kind" with snake_case values matching the Rust variant names:
"invalid_batch_definition", "spawn_failed", "template_resolve_failed",
"backend_error", "limit_exceeded".]`

`[GAP-E3: InvalidBatchDefinition carries only a free-string "reason"
in Rust (line 1599). Agents cannot programmatically distinguish cycles
from dangling refs from duplicate names. Propose a sub-discriminator
"reason" as a fixed enum (see Finding 3) with typed extra fields
("cycle", "dangling_refs", "duplicates").]`

---

### Step 2 -- Inspect state after rejection

AGENT: `koto status coord`

KOTO response (proposed):

```json
{
  "workflow": "coord",
  "state": "plan_and_await",
  "initialized": true,
  "terminal": false,
  "event_count": 2,
  "last_event": "Transitioned",
  "pending_evidence": true
}
```

Gloss: AGENT wants to know whether the rejected submission was
persisted. The design says InvalidBatchDefinition rejects "before any
new spawn happens" but never pins where the rejection happens relative
to the EvidenceSubmitted append. Two semantics are possible:

(i) validate-first, then append (fail-closed) -- the bad submission
leaves no trace, parent is unchanged.

(ii) append-first, then validate (audit trail) -- the bad submission
is in the log with a follow-up rejection marker.

`[GAP-E4: persistence semantics for rejected evidence is unspecified.
Propose (i) validate-first: InvalidBatchDefinition is a pure check on
the submitted payload; no EvidenceSubmitted event is appended. Rationale:
the advance loop's existing evidence validation (accepts field types)
already rejects before append; batch validation should behave the same.
Add this as an explicit invariant in Decision 1.]`

---

### Step 3 -- Second submission: dangling ref

AGENT fixes the cycle but introduces a dangling reference. Task D
waits on `"does-not-exist"`.

tasks.json:

```json
{
  "tasks": [
    {"name": "issue-A", "vars": {"ISSUE_NUMBER": "1"}},
    {"name": "issue-B", "vars": {"ISSUE_NUMBER": "2"}, "waits_on": ["issue-A"]},
    {"name": "issue-D", "vars": {"ISSUE_NUMBER": "4"}, "waits_on": ["does-not-exist"]}
  ]
}
```

AGENT: `koto next coord --with-data @tasks.json`

KOTO response:

```json
{
  "action": "error",
  "state": "plan_and_await",
  "error": {
    "code": "invalid_submission",
    "message": "Invalid batch definition: 1 dangling waits_on reference",
    "details": [
      {
        "kind": "batch_error",
        "batch_kind": "invalid_batch_definition",
        "reason": "dangling_refs",
        "dangling_refs": [
          {"task": "issue-D", "missing": ["does-not-exist"]}
        ],
        "field": "tasks"
      }
    ]
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

Gloss: Same `batch_kind: invalid_batch_definition` as Step 1 but a
different `reason`. AGENT can programmatically dispatch on `reason`
only if the design pins it as an enum, which it currently does not.

`[GAP-E5: R4 (no dangling refs) and R5 (task names unique) and the DAG
cycle check (R3) all collapse into InvalidBatchDefinition { reason:
String } with free-form text. The walkthrough surface gives agents no
way to tell them apart without string-scraping. Propose a fixed
"reason" enum: "cycle" | "dangling_refs" | "duplicate_names" |
"invalid_name" | "trigger_rule_unsupported" plus typed payload
fields per reason.]`

---

### Step 4 -- Valid definition, scheduler spawn fails on C

tasks.json: A (no deps, good template), B (waits on A, good template),
C (waits on A, `template: "does-not-exist.md"`).

```json
{
  "tasks": [
    {"name": "issue-A", "vars": {"ISSUE_NUMBER": "1"}},
    {"name": "issue-B", "vars": {"ISSUE_NUMBER": "2"}, "waits_on": ["issue-A"]},
    {"name": "issue-C", "vars": {"ISSUE_NUMBER": "3"}, "template": "does-not-exist.md"}
  ]
}
```

AGENT: `koto next coord --with-data @tasks.json`

Sequence in the advance loop: evidence is structurally valid (DAG,
names unique, refs resolve) so EvidenceSubmitted is appended. Gate
evaluates against 3 declared tasks, 0 children, returns Failed.
Scheduler then runs -- only A and C are Ready (empty waits_on). A
spawns; C fails template resolution.

`[GAP-E6: design does not say whether the scheduler halts on the first
per-task failure or processes all ready tasks. Line 1583 shows
SchedulerOutcome::Error { reason: String } as a single-error outcome,
implying halt-on-first. But Decision 1's failure_policy (skip_dependents
vs continue) is about child-run failures, not spawn failures. Propose:
scheduler accumulates per-task spawn errors and returns a per-task
error list inside Scheduled so partial progress is always visible.]`

KOTO response (proposed accumulated shape):

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "...",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 3, "completed": 0, "pending": 3,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 2,
      "all_complete": false,
      "children": [
        {"name": "coord.issue-A", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.issue-B", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.issue-A"]},
        {"name": "coord.issue-C", "state": null, "complete": false, "outcome": "pending"}
      ]
    }
  }],
  "scheduler": {
    "spawned": ["coord.issue-A"],
    "already": [],
    "blocked": ["coord.issue-B"],
    "skipped": [],
    "errored": [
      {
        "task": "issue-C",
        "batch_kind": "template_resolve_failed",
        "paths_tried": [
          "/home/user/repo/templates/does-not-exist.md",
          "/home/user/repo/does-not-exist.md"
        ]
      }
    ]
  }
}
```

Gloss: The envelope stays `gate_blocked` because the parent's advance
loop did block on a gate; the per-task spawn failures surface through
a new `scheduler.errored` array. Issue-C's outcome in the gate output
is `pending` (no child session on disk) which is misleading.

`[GAP-E7: "errored" is not in the SchedulerOutcome enum today. Design
treats scheduler errors as a single Error variant. Add an "errored"
vector alongside "spawned", "already", "blocked", "skipped".]`

`[GAP-E8: gate child entry for a task the scheduler failed to spawn
looks identical to "not yet spawned". Propose a new outcome value
"spawn_failed" on BatchTaskView (line 1657-1669 already has a "reason"
field that could carry it), mirrored into children-complete output.]`

---

### Step 5 -- Partial success reporting

2 of 5 ready tasks spawn; 1 hits TemplateResolveFailed, 1 hits
SpawnFailed (I/O). AGENT submits, advance loop accepts, scheduler runs.

KOTO response (proposed):

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [...],
  "scheduler": {
    "spawned": ["coord.t1", "coord.t2"],
    "already": [],
    "blocked": [],
    "skipped": [],
    "errored": [
      {
        "task": "t3",
        "batch_kind": "template_resolve_failed",
        "paths_tried": ["/home/user/repo/templates/missing.md"]
      },
      {
        "task": "t4",
        "batch_kind": "spawn_failed",
        "source": "io error: No space left on device (os error 28)"
      }
    ]
  }
}
```

Gloss: Both errored entries share a `task` key and a `batch_kind`
discriminator. `SpawnFailed`'s `source: anyhow::Error` (line 1601)
serializes to a human string -- agents cannot program against it.

`[GAP-E9: SpawnFailed.source is anyhow::Error. Agents cannot distinguish
EEXIST (collision) from ENOSPC (disk full) from EACCES (permissions)
without regex. Propose an additional "source_kind" fixed enum for
machine-readable dispatch: "collision" | "io" | "backend_unavailable".
Keep "source" for human diagnostics.]`

---

### Step 6 -- LimitExceeded

AGENT submits 1001 tasks.

KOTO response:

```json
{
  "action": "error",
  "state": "plan_and_await",
  "error": {
    "code": "invalid_submission",
    "message": "Batch submission exceeds hard limit: tasks (1001 > 1000)",
    "details": [
      {
        "kind": "batch_error",
        "batch_kind": "limit_exceeded",
        "which": "tasks",
        "limit": 1000,
        "actual": 1001
      }
    ]
  },
  "scheduler": null
}
```

Gloss: LimitExceeded's `which` is `&'static str` in the Rust signature
(line 1607). That makes it look like an arbitrary string, but the only
meaningful limits mentioned in the design (line 2129, 2291) are
tasks, edges, and depth.

`[GAP-E10: "which" is a &'static str, not an enum. Agents parsing the
response have no contract that pins valid values. Propose a fixed enum:
"tasks" | "edges" | "depth" | "vars_size" | "waits_on_width". Document
it in Decision 1's table.]`

---

### Step 7 -- Recovery after partial spawn

After Step 4, 1 task (A) was actually spawned and B is blocked. C
failed template resolution. AGENT edits the template path and
re-submits the full task list with C's template fixed.

```json
{
  "tasks": [
    {"name": "issue-A", "vars": {"ISSUE_NUMBER": "1"}},
    {"name": "issue-B", "vars": {"ISSUE_NUMBER": "2"}, "waits_on": ["issue-A"]},
    {"name": "issue-C", "vars": {"ISSUE_NUMBER": "3"}, "template": "impl-issue.md"}
  ]
}
```

AGENT: `koto next coord --with-data @tasks.json`

Open question: is a second EvidenceSubmitted event for the same field
allowed? The design does not say. Two interpretations:

(a) Re-submission is rejected -- `accepts.tasks` was already satisfied,
so the advance loop returns InvalidSubmission("already submitted").

(b) Re-submission replaces the batch definition; the scheduler
reconciles: A is `already`, B is still `blocked`, C is the new Ready.

`[GAP-E11: resubmission semantics for a tasks-typed field are
unspecified. Decision 1 says "one batch per template in v1" but that
refers to one materialize_children hook per state, not one submission.
Propose (b) replace-and-reconcile: the scheduler treats the latest
EvidenceSubmitted as the batch definition of record. Task names that
already have a spawned session go into `already`. Task names in the
new definition that were not in the previous one (or had their
template path changed) become Ready and get spawned. Task names
removed between submissions surface as a new `orphaned` list in the
scheduler outcome. Reject if a already-spawned task has its `vars`
changed (would silently diverge from child state).]`

KOTO response (under interpretation b):

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "output": {
      "total": 3, "completed": 0, "pending": 3,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 1,
      "all_complete": false,
      "children": [
        {"name": "coord.issue-A", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.issue-B", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.issue-A"]},
        {"name": "coord.issue-C", "state": "working", "complete": false, "outcome": "pending"}
      ]
    }
  }],
  "scheduler": {
    "spawned": ["coord.issue-C"],
    "already": ["coord.issue-A"],
    "blocked": ["coord.issue-B"],
    "skipped": [],
    "errored": [],
    "orphaned": []
  }
}
```

Gloss: The event log now has two EvidenceSubmitted events for
`tasks`. `[GAP-E12: does derive_batch_view read the first or last
EvidenceSubmitted? Propose "last wins"; surface the earlier ones as
superseded but keep them in the log for audit.]`

---

### Step 8 -- Event log visibility

AGENT: `koto query coord --events` (not yet in the walkthrough but
implied by `koto query` inspect semantics).

KOTO response (proposed; events list):

```json
{
  "workflow": "coord",
  "events": [
    {"seq": 0, "type": "WorkflowInitialized", "timestamp": "..."},
    {"seq": 1, "type": "Transitioned", "to": "plan_and_await"},
    {"seq": 2, "type": "EvidenceSubmitted", "fields": {"tasks": [...step4...]}},
    {"seq": 3, "type": "SchedulerRan", "spawned": ["coord.issue-A"], "errored": [{"task": "issue-C", "batch_kind": "template_resolve_failed"}]},
    {"seq": 4, "type": "EvidenceSubmitted", "fields": {"tasks": [...step7...]}},
    {"seq": 5, "type": "SchedulerRan", "spawned": ["coord.issue-C"], "already": ["coord.issue-A"], "blocked": ["coord.issue-B"]}
  ]
}
```

Gloss: The design never names a SchedulerRan event type. Without one,
partial spawn failures leave no audit trail -- the scheduler's decisions
are ephemeral, only visible in the response of the `koto next` call
that produced them.

`[GAP-E13: "SchedulerRan" is not in the event enum. Partial spawn
failures are not persisted. Propose appending a SchedulerRan event
(or equivalent) whenever the scheduler makes a decision, carrying the
same payload as the response's scheduler field. Alternative: do not
append on NoBatch or no-op ticks to avoid log noise; append only when
spawned/errored/skipped is non-empty.]`

`[GAP-E14: rejected-evidence responses from Step 1 and Step 3 -- the
cycle and dangling-ref cases -- leave no trace (see GAP-E4). That's
the right default for validity errors, but operators investigating "why
did my workflow stall?" won't be able to see the failed submissions.
Non-blocking but worth noting.]`

---

### Step 9 -- Scheduler error while gate is also blocked

`run_batch_scheduler` returns `Err(BatchError::BackendError { source })`
(backend.list() failed during classification). The advance loop has
already finalized a `gate_blocked` response.

`[GAP-E15: design says the scheduler outcome is "an additive field for
observability" (line 1587-1594) but does not address BatchError returned
from run_batch_scheduler vs. the Ok(SchedulerOutcome) case. Two
plausible envelopes:

(x) keep action=gate_blocked, put the scheduler error in a new top-level
`scheduler_error` field alongside `scheduler: null`;

(y) promote to action=error, hiding the gate result entirely.

Propose (x). Rationale: BackendError is transient (exit code 1 already
per NextErrorCode::IntegrationUnavailable); the gate info the advance
loop computed is still valid; the agent retries next tick. Promoting
to error loses the gate state.]`

KOTO response (under x):

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [...],
  "scheduler": null,
  "scheduler_error": {
    "batch_kind": "backend_error",
    "message": "backend list failed: connection refused",
    "retryable": true
  }
}
```

---

### Step 10 -- Compile error at `koto init`

Parent template declares `materialize_children.default_template:
"missing.md"` which does not resolve at compile time (E9).

AGENT: `koto init coord --template coord.md --var plan_path=...`

KOTO response:

```json
{
  "action": "error",
  "error": {
    "code": "template_error",
    "message": "Template validation failed",
    "details": [
      {
        "kind": "compile_error",
        "rule": "E9",
        "state": "plan_and_await",
        "field": "materialize_children.default_template",
        "value": "missing.md",
        "message": "default_template does not resolve to a compilable template",
        "paths_tried": [
          "/home/user/repo/templates/missing.md"
        ]
      }
    ]
  }
}
```

Gloss: Compile errors use the existing `NextErrorCode::TemplateError`
(exit code 3). The envelope is identical to batch-error responses --
same `action: error`, same `error.details[]` shape. Only the
`details[].kind` discriminator changes (`compile_error` vs.
`batch_error`).

`[GAP-E16: no written rule that compile errors and batch errors share
the NextError envelope. The design's prose around "NextError::Batch {
kind, message }" (line 1610) hints at a separate variant, but that
conflicts with the current struct-shaped NextError. Propose explicit
language: all koto domain errors use one envelope; the `details[].kind`
tag discriminates compile / batch / gate / evidence failures. See
Finding 1 for the full envelope.]`

---

## Section 2: Findings

### Finding 1: No canonical error-response envelope is specified

- **Observation**: The design mentions `NextError::Batch { kind,
  message }` (line 1610) but never writes out the JSON envelope. The
  existing `NextError` in `src/cli/next_types.rs:283-289` has `code`,
  `message`, `details` -- a different shape than the design's enum-
  variant sketch. The walkthrough has no error-response example.
- **Location in design**: Decision 1 (lines 555-670), scheduler
  section (lines 1574-1612).
- **Severity**: High. Agents cannot be coded against an unspecified
  shape. Every error path in the simulation had to flag this gap.
- **Proposed uniform error-envelope shape**:

```json
{
  "action": "error",
  "state": "<current state or null>",
  "error": {
    "code": "<NextErrorCode snake_case>",
    "message": "<human message>",
    "details": [
      {
        "kind": "<batch_error | compile_error | gate_error | evidence_error>",
        "...": "discriminator-specific fields"
      }
    ]
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

Mapping of BatchError variants to `details[0]`:

| Variant | details[0] fields |
|---------|--------------------|
| InvalidBatchDefinition | `kind=batch_error`, `batch_kind=invalid_batch_definition`, `reason=<enum>`, plus typed payload per reason |
| SpawnFailed | `kind=batch_error`, `batch_kind=spawn_failed`, `task`, `source_kind=<enum>`, `source` (human string) |
| TemplateResolveFailed | `kind=batch_error`, `batch_kind=template_resolve_failed`, `task`, `paths_tried` |
| BackendError | `kind=batch_error`, `batch_kind=backend_error`, `message`, `retryable=true` |
| LimitExceeded | `kind=batch_error`, `batch_kind=limit_exceeded`, `which=<enum>`, `limit`, `actual` |

Compile errors (E1-E10) use `kind=compile_error` with the rule id
(`E9`), the state name, the offending field path, and the raw value.

Map to `NextErrorCode`:

- InvalidBatchDefinition, LimitExceeded -> `invalid_submission`
- TemplateResolveFailed at compile time -> `template_error`
- TemplateResolveFailed at runtime, SpawnFailed -> `persistence_error`
  (infra) or `invalid_submission` depending on whether the path came
  from the agent's submission or the parent template
- BackendError -> `integration_unavailable` (retryable exit code 1)

### Finding 2: Scheduler halt-on-error vs. partial-accumulation is unspecified

- **Observation**: `SchedulerOutcome::Error { reason: String }` (line
  1583) is a single-error outcome. `SchedulerOutcome::Scheduled` has
  `spawned`, `skipped`, `already`, `blocked` but no per-task error
  list. Steps 4 and 5 of the simulation hit this gap: a partial-success
  tick cannot be faithfully represented.
- **Location in design**: Scheduler section (lines 1574-1594).
- **Severity**: High. Batch correctness requires visible partial
  progress; halt-on-first forces agents to reset and retry whole
  batches.
- **Proposed resolution**: Add `errored: Vec<TaskSpawnError>` to
  `SchedulerOutcome::Scheduled`. `SchedulerOutcome::Error` remains for
  top-level failures (bad evidence, backend list failure) that
  invalidate the whole tick. Per-task spawn failures go into
  `errored` and the scheduler continues. Add `spawn_failed` to the
  `BatchTaskView.outcome` enum so the `children-complete` gate output
  mirrors it.

### Finding 3: InvalidBatchDefinition `reason` is unstructured

- **Observation**: `InvalidBatchDefinition { reason: String }` (line
  1599) carries a free-form string. R1-R7 cover cycles, dangling
  refs, duplicate names, invalid names, template non-compile, var
  resolution, name collisions -- all collapse into the same string
  bucket.
- **Location in design**: line 1597-1599, Decision 1 runtime-check
  table (line 640).
- **Severity**: Medium. Agents can't programmatically recover (e.g.
  "ask user to rename a task" vs. "rerun with dep fix") without
  string-matching.
- **Proposed resolution**: Turn `reason` into a fixed enum with typed
  payload:

  | reason | extra fields |
  |--------|--------------|
  | `cycle` | `cycle: Vec<String>` (ordered node sequence) |
  | `dangling_refs` | `dangling_refs: Vec<{task, missing: Vec<String>}>` |
  | `duplicate_names` | `duplicates: Vec<String>` |
  | `invalid_name` | `task`, `reason_detail` |
  | `template_not_compilable` | `task`, `template_path`, `compile_error` |
  | `var_unresolved` | `task`, `vars: Vec<String>` |
  | `name_collision` | `task`, `existing_child` |
  | `trigger_rule_unsupported` | `task`, `trigger_rule` |

### Finding 4: Persistence semantics for rejected evidence are undefined

- **Observation**: The design says InvalidBatchDefinition rejects
  "before any new spawn happens" (line 1864) but never says whether
  the EvidenceSubmitted event is persisted.
- **Location in design**: Decision 1 (lines 622-640), line 1864.
- **Severity**: Medium. Without a fixed semantics, recovery (Step 2)
  and audit (Step 8) behave differently across implementations.
- **Proposed resolution**: Make batch validity a pre-append check.
  No EvidenceSubmitted on rejection. Add explicit invariant to
  Decision 1: "R1-R7 runtime checks run before the advance loop
  appends EvidenceSubmitted. A failed check returns the error
  envelope in Finding 1 with zero state-file writes."

### Finding 5: `LimitExceeded.which` is not an enum

- **Observation**: `which: &'static str` (line 1607) is static only
  to avoid allocation -- it's a free string from the agent's point of
  view. No list of valid values is given.
- **Location in design**: line 1597-1608, line 2291.
- **Severity**: Low. One-line fix but necessary for agent dispatch.
- **Proposed resolution**: Document the enum: `"tasks"`, `"edges"`,
  `"depth"`, `"vars_size"`, `"waits_on_width"`. Expose `limit` and
  `actual` as numbers (already in the signature).

### Finding 6: `SpawnFailed.source` is opaque

- **Observation**: `source: anyhow::Error` (line 1601) serializes to
  a human string. EEXIST (name collision) vs. ENOSPC vs. EACCES are
  indistinguishable to the agent.
- **Location in design**: line 1601.
- **Severity**: Medium. Name collisions are agent-recoverable (rename
  and retry); disk full is not.
- **Proposed resolution**: Add `source_kind: SpawnErrorKind` with
  variants `collision` (EEXIST on init_state_file), `io` (generic I/O),
  `backend_unavailable` (transient network), `permission_denied`. Keep
  `source` as the human message.

### Finding 7: Resubmission semantics are undefined

- **Observation**: The tasks-typed accepts field can receive multiple
  submissions in principle (EvidenceSubmitted events are append-only).
  Decision 1 never pins what happens on a second submission.
- **Location in design**: Decision 1, line 622-640.
- **Severity**: High. Direct recovery path (Step 7) depends on this.
- **Proposed resolution**: "Last-write wins" with reconciliation:
  latest EvidenceSubmitted defines the batch; scheduler reconciles
  against existing child sessions; changing `vars` on an already-spawned
  task is an error; removing a task surfaces it as `orphaned`. Document
  as Decision 1 addendum.

### Finding 8: No event for scheduler decisions

- **Observation**: Design does not define a `SchedulerRan` event
  type. Spawn-failure decisions exist only in the response of one
  tick; later `koto query --events` cannot recover them.
- **Location in design**: lines 1574-1594.
- **Severity**: Medium. Breaks post-hoc debugging of stalled batches.
- **Proposed resolution**: Append `SchedulerRan` event when the
  scheduler does non-trivial work (spawn/error/skip). Payload matches
  the response's `scheduler` field. Skip append on NoBatch /
  all-already ticks to keep the log readable.

### Finding 9: Scheduler-error envelope when gate is also blocked

- **Observation**: `run_batch_scheduler` returning `Err(BatchError)`
  after the advance loop produced `gate_blocked` is not addressed by
  the additive-field note (line 1587-1594).
- **Location in design**: lines 1587-1594.
- **Severity**: Medium.
- **Proposed resolution**: Keep the outer envelope as
  `gate_blocked` (the gate result is valid; agent should retry).
  Add a top-level `scheduler_error` field mirroring the BatchError
  mapping in Finding 1, with `retryable: bool`. Leave
  `scheduler: null` so the additive-field invariant is preserved.

### Finding 10: Compile-error JSON shape is unspecified

- **Observation**: E1-E10 are described in prose but the design does
  not fix their JSON shape. Agents hitting E5, E9, or E10 at
  `koto init` need a contract.
- **Location in design**: Decision 1 validation table (lines 622-640).
- **Severity**: Medium. Compile errors share the envelope proposed in
  Finding 1; every rule needs a `rule` id and a `field` path.
- **Proposed resolution**: All compile errors use `details[0].kind =
  "compile_error"` with required fields `rule` (e.g. "E9"), `state`
  (may be null for frontmatter-level rules), `field` (dotted path),
  `value`, and `message`. `NextErrorCode::TemplateError` (exit 3).

### Finding 11: Gate child-outcome enum missing `spawn_failed`

- **Observation**: After a partial-spawn failure, the
  `children-complete` output for a failed-to-spawn task reverts to
  `outcome: pending` or `blocked` -- identical to "not yet processed".
- **Location in design**: BatchTaskView (lines 1657-1669).
- **Severity**: Low-medium.
- **Proposed resolution**: Add `spawn_failed` as an outcome on
  `BatchTaskView.outcome` and on the `children-complete` gate output.
  Use `BatchTaskView.reason` to carry the BatchError kind.

### Finding 12: Top-level action for error responses is unspecified

- **Observation**: Walkthrough enumerates actions `evidence_required`,
  `gate_blocked`, `done`. No error action is defined.
- **Location in design**: walkthrough.md (no error examples).
- **Severity**: Low. One-word decision; blocks nothing but agents
  need a stable string to dispatch on.
- **Proposed resolution**: `action: "error"` when `error` is non-null.
  Document in Decision 1 and in the walkthrough appendix.
