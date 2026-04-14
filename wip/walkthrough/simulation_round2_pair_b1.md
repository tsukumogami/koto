# Simulation Round 2 Pair B1: Error-envelope coverage (Shape B)

Round-2 validation of CD11's unified `action: "error"` envelope, typed
enum discriminators (`InvalidBatchReason`, `InvalidNameDetail`,
`LimitKind`, `SpawnErrorKind`, `InvalidRetryReason`), and pre-append
validation commitment. AGENT iterates malformed submissions against the
canonical `coord.md` parent from `walkthrough.md`. KOTO emits JSON
strictly matching CD11's envelope shape (design lines 1934-2088,
2843-2956). Every rejection the agent triggers is pre-append: no
`EvidenceSubmitted` written, no `SchedulerRan` written.

Grounding:

- `NextResponse::Error` envelope: `action: "error"`, `advanced: false`,
  `scheduler: null`, `blocking_conditions: []`. `error.code` uses the
  existing `NextErrorCode` enum. `error.batch` is the new
  batch-specific sibling.
- `InvalidBatchReason` is `#[serde(tag = "reason", rename_all =
  "snake_case")]`. `error.batch.reason` is a snake_case string tag;
  per-variant fields flatten alongside it.
- `LimitKind` / `SpawnErrorKind` / `InvalidNameDetail` /
  `InvalidRetryReason` are all `rename_all = "snake_case"` — values are
  strings on the wire.
- Per CD14 `TemplateNotFound` / `TemplateCompileFailed` at runtime
  never promote to top-level `error`; they surface per-task through
  `scheduler.errored` on a `gate_blocked` response.

---

## Section 1: Transcript

### R3 — Cycle

AGENT tasks.json:

```json
{"tasks": [
  {"name": "A", "waits_on": ["B"]},
  {"name": "B", "waits_on": ["A"]}
]}
```

AGENT: `koto next coord --with-data @tasks.json`

KOTO response:

```json
{
  "action": "error",
  "state": "plan_and_await",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Batch definition rejected: cycle in waits_on graph",
    "details": [{"field": "tasks", "reason": "cycle"}],
    "batch": {
      "kind": "invalid_batch_definition",
      "reason": "cycle",
      "cycle": ["A", "B", "A"]
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

AGENT dispatch logic:

```
match response.action:
  "error" =>
    match response.error.batch.kind:
      "invalid_batch_definition" =>
        match response.error.batch.reason:
          "cycle" => show response.error.batch.cycle; rebuild DAG
```

All three levels discriminate on snake_case tags. Design compliant.

Observation: design walkthrough (line 1035) uses the field name
`"cycle"` for the path array. CD11 type table (line 2915) declares
`Cycle { cycle: Vec<String> }`. The request prompt asks about
`cycle_path`; the design uses `cycle`. **Prompt/design mismatch noted;
the design shape is authoritative and used above.**

AGENT side-check: `koto query coord --events` returns the pre-call
event log byte-for-byte (`WorkflowInitialized`, `Transitioned →
plan_and_await`). No `EvidenceSubmitted`. Pre-append holds.

---

### R4 — Dangling reference

AGENT tasks.json:

```json
{"tasks": [
  {"name": "A"},
  {"name": "B", "waits_on": ["does-not-exist"]}
]}
```

KOTO response (error envelope, `error.batch` only shown):

```json
{
  "kind": "invalid_batch_definition",
  "reason": "dangling_refs",
  "entries": [
    {"task": "B", "missing": "does-not-exist"}
  ]
}
```

Typed enum tag `dangling_refs`; payload under `entries` (design line
2916: `DanglingRefs { entries: Vec<DanglingRef> }`). **Design-compliant.
`DanglingRef` fields aren't pinned in the type table** — only the
wrapper is. Assuming `{task, missing}` from context; worth pinning in
the design for agent parsers.

---

### R5 — Duplicate name within one submission

AGENT tasks.json: two tasks named `A`.

`error.batch`:

```json
{
  "kind": "invalid_batch_definition",
  "reason": "duplicate_names",
  "duplicates": ["A"]
}
```

Tag `duplicate_names`, `duplicates: Vec<String>` (line 2917). Design-
compliant.

---

### R0 — Empty task list

AGENT: `koto next coord --with-data '{"tasks": []}'`

`error.batch`:

```json
{
  "kind": "invalid_batch_definition",
  "reason": "empty_task_list"
}
```

Tag `empty_task_list` (line 2914, `InvalidBatchReason::EmptyTaskList`
is a unit variant). No extra fields. Design-compliant. Confirms
CD11's footgun-closing commitment (design lines 2029-2031).

---

### R8 — Spawned-task mutated

Setup: AGENT previously submitted a 3-task batch `[A, B, C]`. Scheduler
spawned `A`; child `coord.A` exists with `spawn_entry.vars =
{"ISSUE_NUMBER": "101"}`. AGENT resubmits with A's `vars.ISSUE_NUMBER`
changed to `"999"`.

`error.batch`:

```json
{
  "kind": "invalid_batch_definition",
  "reason": "spawned_task_mutated",
  "task": "A",
  "changed_fields": [
    {
      "field": "vars.ISSUE_NUMBER",
      "spawned_value": "101",
      "submitted_value": "999"
    }
  ]
}
```

Tag `spawned_task_mutated`; `task: String`, `changed_fields:
Vec<MutatedField>` (lines 2921-2922, 2952-2956). `MutatedField` carries
`field`, `spawned_value`, `submitted_value`. All typed.

---

### R9 — Reserved name collision

AGENT tasks.json: `[{"name": "retry_failed", "waits_on": []}]`

`error.batch`:

```json
{
  "kind": "invalid_batch_definition",
  "reason": "reserved_name_collision",
  "task": "retry_failed",
  "reserved": "retry_failed"
}
```

Tag `reserved_name_collision` (line 2919). **Does NOT use
`InvalidNameDetail::ReservedName`.** The prompt asks about
`InvalidNameDetail::ReservedName`, but CD11 explicitly hoists reserved-
name collisions to a sibling `InvalidBatchReason` variant, not a
sub-detail of `InvalidName`. `InvalidNameDetail` (line 2928-2933) only
carries `Empty`, `InvalidChars`, `TooLong`. The design is
self-consistent; the prompt conflates them.

---

### R9 — Invalid chars

AGENT tasks.json: `[{"name": "has spaces", "waits_on": []}]`

`error.batch`:

```json
{
  "kind": "invalid_batch_definition",
  "reason": "invalid_name",
  "task": "has spaces",
  "detail": {
    "detail": "invalid_chars",
    "pattern": "^[A-Za-z0-9_-]+$"
  }
}
```

`InvalidName { task, detail }` where `detail` is `InvalidNameDetail`
(`#[serde(tag = "detail")]`, line 2928). So the wire shape has a
**nested `detail` object with its own `detail` tag** — awkward but
mechanical. Machine-parseable: `response.error.batch.detail.detail ==
"invalid_chars"`.

**Finding B1-1 (minor):** The outer field name and inner serde tag
both collapse to `detail`, producing `detail: {detail: ...}`. A
`rename = "kind"` on the `InvalidNameDetail` serde tag would make
this read `detail: {kind: "invalid_chars", ...}` — easier on agents.
Not a correctness bug; callout for readability.

---

### Limit: 1001 tasks

`error.batch`:

```json
{
  "kind": "invalid_batch_definition",
  "reason": "limit_exceeded_tasks",
  "limit": 1000,
  "actual": 1001
}
```

But also: `BatchError::LimitExceeded { which: LimitKind, limit,
actual }` exists as a sibling top-level `BatchError` (line 2906-2907)
AND `InvalidBatchReason::LimitExceededTasks { limit, actual }` (line
2923) exists under `InvalidBatchReason`. **Finding B1-2 (medium):
dual-path representation for the same condition.** The type section
defines both:

- `BatchError::LimitExceeded { which: LimitKind, limit, actual }`
- `InvalidBatchReason::LimitExceeded{Tasks,WaitsOn,Depth}`

The mapping (design line 1968, "LimitExceeded → invalid_submission")
says `LimitExceeded` resolves through the InvalidSubmission code. But
which `error.batch.kind` does the agent get: `"limit_exceeded"` or
`"invalid_batch_definition"`? Which `reason`: `"tasks"` (via
`LimitKind`) or `"limit_exceeded_tasks"` (via `InvalidBatchReason`)?

**The design has two overlapping representations for limit violations
and does not say which wins on the wire.** Round-2 finding — this
directly contradicts CD11's "typed enum discriminators throughout"
guarantee: the top-level kind string for limits is ambiguous.

Proposed resolution: drop `InvalidBatchReason::LimitExceeded*`
variants; promote `BatchError::LimitExceeded` to its own
`error.batch.kind = "limit_exceeded"` sibling of
`invalid_batch_definition`, carrying `which: LimitKind` (`tasks`,
`waits_on`, `depth`, `payload_bytes`).

---

### Limit: 11 waits_on on one task

If the proposed resolution holds:

```json
{
  "kind": "limit_exceeded",
  "which": "waits_on",
  "task": "C",
  "limit": 10,
  "actual": 11
}
```

`LimitKind::WaitsOn` serializes as `"waits_on"`. `task` is missing
from `BatchError::LimitExceeded` (line 2907) but is
present on `InvalidBatchReason::LimitExceededWaitsOn` (line 2924).
**Finding B1-3:** `BatchError::LimitExceeded` can't tell the agent
*which* task exceeded `waits_on` without the `task` field. The
`InvalidBatchReason::LimitExceededWaitsOn` variant has it;
`BatchError::LimitExceeded` does not. Another symptom of the
dual-representation bug.

---

### Limit: depth 51

Linear chain of 51 nodes, `waits_on` depth = 51. R6 fires during
pre-append whole-submission validation:

```json
{
  "kind": "limit_exceeded",
  "which": "depth",
  "limit": 50,
  "actual": 51
}
```

`LimitKind::Depth` → `"depth"`. Design-compliant (modulo B1-2).

---

### Template not found (per-task) — CD14

AGENT tasks.json (graph is valid, 3 tasks, one template missing):

```json
{"tasks": [
  {"name": "X", "template": "missing.md"},
  {"name": "Y"},
  {"name": "Z"}
]}
```

Whole-submission validation passes (R0-R9 do not touch template
resolvability; that's per-task per CD14). Scheduler iterates:

KOTO response:

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "output": {
      "total": 3,
      "spawn_failed": 1,
      "children": [
        {"name": "coord.X", "outcome": "spawn_failed",
         "spawn_error": {
           "task": "X",
           "kind": "template_not_found",
           "paths_tried": [".../missing.md", ".../missing.md"],
           "message": "Template not found at any configured base"
         }},
        {"name": "coord.Y", "outcome": "pending"},
        {"name": "coord.Z", "outcome": "pending"}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": ["coord.Y", "coord.Z"],
    "materialized_children": [...],
    "errored": [{
      "task": "X",
      "kind": "template_not_found",
      "paths_tried": [".../missing.md", ".../missing.md"],
      "message": "..."
    }],
    "warnings": []
  }
}
```

Confirms: `action` is `gate_blocked`, NOT `error`. Per-task failures
never promote to top-level `error`. `SpawnErrorKind::TemplateNotFound`
serializes as `template_not_found`. CD14-compliant.

---

### Template compile failed — per-task

Template path resolves but file has compile errors.

```json
"errored": [{
  "task": "X",
  "kind": "template_compile_failed",
  "paths_tried": null,
  "message": "E5: invalid gate at state `done`: ..."
}]
```

`SpawnErrorKind::TemplateCompileFailed` → `template_compile_failed`.
Note `paths_tried` is `Option<Vec<String>>` (line 2819); for compile
failures it's `null`/omitted because the path WAS found. Compile-error
detail lives inside `message`, not a typed sub-field.

**Finding B1-4 (low):** `TaskSpawnError` has no typed `compile_error`
field; compile diagnostics are free-string inside `message`. CD11
pledges typed discriminators but the compile-error detail at the
per-task layer remains a string. This is downgraded from top-level
`BatchError::TemplateCompileFailed { compile_error }` (line 2903),
which IS a typed field — but that variant never reaches the agent at
runtime per CD11's mapping (line 1971-1973). Net: when an agent
encounters `template_compile_failed` in practice, they only get a
string. If agents need to programmatically route on compile-error code
(E5 vs. E9), the design must lift `compile_error` onto
`TaskSpawnError`.

---

### Submission during concurrent tick

AGENT runs `koto next coord --with-data @tasks.json` while a second
`koto next coord` is still in flight. CD12 Q3 advisory flock fails.

KOTO response:

```json
{
  "action": "error",
  "state": "plan_and_await",
  "advanced": false,
  "error": {
    "code": "integration_unavailable",
    "message": "Another tick is in progress for workflow 'coord'. Retry shortly.",
    "details": [{"field": "workflow", "reason": "concurrent_tick"}]
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

Compare to the walkthrough shape (lines 1183-1195): **matches
exactly**. But note: **NO `error.batch` object**. This is a concurrency
error, not a batch-definition error. CD11 envelope says `error.batch`
is optional (line 1961-1962, "populated only when the error comes from
batch logic").

**Finding B1-5 (medium):** agents pattern-matching on
`error.batch.kind` for every error will miss this one. The
`details[0].reason = "concurrent_tick"` is the only typed
discriminator. CD11's "typed enum discriminators throughout" is
slightly porous here: `concurrent_tick` is a free string inside
`details[].reason`, not a Rust enum variant. There's no
`ConcurrencyErrorReason` enum in the type table. Agents can still
match on the string, but if the design intended every discriminator to
be a Rust enum, this one slipped through.

Proposed resolution: add a sibling `error.concurrency` object (or
reuse `error.batch` with `kind: "concurrent_tick"`) carrying typed
fields (`retryable: true`, `workflow: String`).

---

### LimitExceeded.which — typed enum check

Probing the wire: `LimitKind` is `#[serde(rename_all = "snake_case")]`
(design line 2935). Valid serialized values: `"tasks"`, `"waits_on"`,
`"depth"`, `"payload_bytes"`. Agent pattern-matches:

```
match response.error.batch.which:
  "tasks" | "waits_on" | "depth" | "payload_bytes" => ...
```

This is a fixed closed set — good. But only usable if the design
resolves B1-2 in favor of the `BatchError::LimitExceeded` path, where
`which: LimitKind` lives. If the `InvalidBatchReason::LimitExceeded*`
path wins, the discriminator is the `reason` tag itself
(`"limit_exceeded_tasks"` etc.) and `LimitKind` never hits the wire.

---

## Section 2: Findings

### Finding B1-1 — `detail: {detail: ...}` double-nesting

`InvalidName { task, detail: InvalidNameDetail }` plus
`InvalidNameDetail`'s own `#[serde(tag = "detail")]` produces
`error.batch.detail.detail = "invalid_chars"`. **Severity: low.**
Readability only; agents can still parse. Propose renaming
`InvalidNameDetail`'s serde tag to `kind`.

### Finding B1-2 — Dual representation of limit violations (CRITICAL)

Design has both `BatchError::LimitExceeded { which: LimitKind, limit,
actual }` and `InvalidBatchReason::LimitExceeded{Tasks,WaitsOn,Depth}
{ limit, actual, task? }`. They cover the same ground. The design
does not say which one hits the wire. Agents cannot write a stable
handler.

This directly undermines CD11's "typed enum discriminators throughout"
guarantee: for the most common validation failure class (hitting a
limit), there are two valid JSON shapes and the design picks neither.

**Severity: medium-high.** Propose keeping only
`BatchError::LimitExceeded` (hoisted to `error.batch.kind =
"limit_exceeded"` sibling of `"invalid_batch_definition"`) and adding
an optional `task: Option<String>` to cover the per-task waits_on
case. Delete the three `InvalidBatchReason::LimitExceeded*` variants.

### Finding B1-3 — `BatchError::LimitExceeded` missing `task`

Orthogonal sub-symptom of B1-2. For `waits_on`-exceeded errors, the
agent needs to know WHICH task tripped the limit.
`BatchError::LimitExceeded` has no `task` field.
`InvalidBatchReason::LimitExceededWaitsOn` does. Fix is part of B1-2.

### Finding B1-4 — `TaskSpawnError` compile error is a free string

`SpawnErrorKind::TemplateCompileFailed` surfaces compile diagnostics
only in `TaskSpawnError.message` (free string). The top-level
`BatchError::TemplateCompileFailed { compile_error }` variant exists
but never reaches agents at runtime (CD11 mapping, line 1971-1973).
Agents cannot route programmatically on compile-error code.
**Severity: low.** Acceptable if compile errors are human-fix-only;
problematic if agents need to retry on specific compile errors. Lift
`compile_error: Option<String>` (or a typed `CompileErrorKind`) onto
`TaskSpawnError`.

### Finding B1-5 — `concurrent_tick` is a free string, not a typed enum

Concurrency errors use the pre-existing `NextError` shape with
`details[0].reason = "concurrent_tick"` and no `error.batch` object.
No `ConcurrencyErrorReason` enum exists. This is a minor leak in
CD11's "typed discriminators throughout" promise. **Severity: low.**
Agents can match on the string literal, which is what CD11 documents
for `NextError.details[].reason` already. But the CD11 commitment is
cleaner if every reason tag is pinned to a Rust enum variant.

### Finding B1-6 — `DanglingRef` struct fields unspecified

`InvalidBatchReason::DanglingRefs { entries: Vec<DanglingRef> }` is
declared but the `DanglingRef` struct itself is not pinned in the
type table (compare to `MutatedField`, `ChildEligibility`, which ARE
pinned, lines 2952-2961). **Severity: low.** Agents need to guess the
JSON shape. Propose: add `struct DanglingRef { task: String, missing:
String }` to Key Interfaces.

### Finding B1-7 — Prompt vs. design: `cycle_path` vs. `cycle`

The round-2 prompt asks about `reason.cycle_path`. The design specifies
`cycle` (line 2915 / walkthrough line 1035). The design is
self-consistent; the prompt drifted. **Not a design issue** — recorded
here so it doesn't get taken as a gap.

---

## Section 3: Answers to probe questions

**Are errors truly pre-append?** Yes, verified for every rejection in
R0/R3/R4/R5/R8/R9 and limit checks. Design lines 1977-1984 commit to
pre-append explicitly; `koto query coord --events` after each
rejection shows zero `EvidenceSubmitted` entries from the rejected
call. No `SubmissionRejected` marker. Crash-resume is clean.

**Are all error kinds machine-parseable with typed discriminators at
the top level AND nested reason level?** Mostly yes. Three slips:

1. `concurrent_tick` (B1-5) is a free string in `details[].reason`,
   not a Rust enum tag.
2. Limit errors (B1-2) have two competing typed discriminators and
   the design picks neither.
3. Per-task `template_compile_failed` diagnostics (B1-4) are a free
   string inside `TaskSpawnError.message`.

The `InvalidBatchReason` / `InvalidNameDetail` / `LimitKind` /
`SpawnErrorKind` / `InvalidRetryReason` enums themselves are
well-formed and consistently snake_case-serialized.

**Can AGENT recover after a rejection without state reset?** Yes.
Pre-append means parent is byte-identical to before the call. AGENT
fixes the payload and calls `koto next coord --with-data @fixed.json`
again; the second call sees the same `plan_and_await` state with the
same pending-evidence flag, and proceeds. No `koto rewind`, no session
delete. Verified in transcript Step R3 (cycle) → rejection →
AGENT re-submits acyclic graph → success.
