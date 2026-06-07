---
status: Accepted
upstream: docs/prds/PRD-request-store-converge.md
problem: |
  koto v0.10.0 ships the fan-out half of coordinator-and-delegates: child
  workflows are created, linked, discovered, claimed, and recorded in the
  workspace-wide terminal index. But a child's completion records only a
  terminal-state name and a typed outcome classification, never the closed
  result the child reached. A coordinator that fanned work out must reopen
  each child's session log to learn what it produced, reintroducing the
  working-context load the fan-out existed to remove. The converge half is
  missing, and it must be added without bloating the hot terminal-index scan
  path or introducing a new top-level command noun.
decision: |
  A child's result is auto-promoted from the terminal evidence the completion
  path already writes, into a typed minimal envelope (status + summary +
  optional payload) persisted as a `request_store.result` event on the
  child's own session log. The terminal index gains one bounded additive
  field — a has_result flag — so the hot scan path stays lean and the full
  result is dereferenced lazily only at the converge point. The parent's
  converge point reuses the existing children-complete gate and the
  GateBlocked directive: blocked while any child in the converge set has no
  result, then the cleared directive inlines every child's result. The
  primitive is uniform and recursive — a child converges its own children and
  carries its own result up identically at every depth.
rationale: |
  Every element rides machinery koto already ships. Auto-promotion keeps R3
  true (no extra agent step) and reuses the terminal-evidence write the
  completion path already performs. A typed envelope matches koto's existing
  typed-outcome idiom so a parent reads any child uniformly. Storing the full
  result on the child session and keeping only a bounded flag in the index
  respects the PIPE_BUF line bound that makes concurrent appends atomic.
  Reusing the children-complete gate and GateBlocked means no new response
  variant and no new command noun, and convergence inherits the engine's
  forward-compatible event handling, atomic appends, and epoch fencing
  unchanged.
---

# DESIGN: request-store-converge

## Status

Accepted

## Context and Problem Statement

koto's coordinator-and-delegates model lets one workflow fan work out to child
workflows. As of v0.10.0 the dispatch half is complete: `materialize_children`
spawns child sessions linked to a parent (`parent_workflow` in the child
header), the parent's discovery scan surfaces children needing an agent
(`unassigned_children` in `koto next`), agents claim and drive children through
an epoch-fenced claim path, and a child that reaches a terminal state is
recorded in the workspace-wide terminal index (`_terminal_index.jsonl`). The
`children-complete` gate already lets a parent block until its batch finishes
and reports per-task counts (`pending`, `failed`, `skipped`, `all_complete`).

The technical gap is at the value boundary, not the control boundary. When a
child completes, two records are written: a `ChildCompleted` event appended to
the *parent's* log (carrying a typed `TerminalOutcome` of `Success` / `Failure`
/ `Skipped` plus the child's `final_state` name) and a `TerminalIndexEntry`
appended to the workspace index (carrying `session_id`, `terminal_at`,
`header_mtime_ns`, `terminal_state`). Neither carries the *result* the child
reached — the decision it made, the summary of its work, any structured payload
a downstream consumer needs. A coordinator that fanned out three evaluations
and wants to converge them can learn that all three are done, but to learn what
each decided it must open and replay each child's session log. That replay is
the exact working-context cost the fan-out exists to avoid.

The design must close this gap inside koto's existing engine substrate. Four
constraints make it a non-trivial architecture problem rather than a localized
change:

1. **The result has to be carried by the completion the agent already
   performs** — adding a mandatory extra agent round-trip would defeat the
   fan-out and create a failure mode where a workflow completes with no result.
2. **The terminal-index scan path is the hot path.** The discovery scan walks
   `_terminal_index.jsonl` line-by-line on every parent poll, and each line is
   bounded to `MAX_INDEX_LINE_BYTES` (4096, within Linux `PIPE_BUF`) so that
   concurrent `O_APPEND` writes from independent agents never interleave. An
   arbitrary-size result payload cannot live in an index line.
3. **No new top-level command noun and no new `koto next` response variant.**
   The reserved `request_store` config namespace already anticipates wiring
   convergence into existing structures; the converge point must reuse the
   `GateBlocked` directive surface.
4. **The schema must be forward-compatible** with koto's closed-enum +
   `Unknown` fallthrough event model, its NDJSON append-only logs, and its
   tempfile-rename atomic writes.

The source requirements for this design are recorded in the upstream PRD
(`docs/prds/PRD-request-store-converge.md`, R1–R11, AC1–AC11), which fixes the
constraints and defers three architectural decisions to this design: how the
result is promoted (D1), the exact envelope field set (D2), and where the
result is stored and how the converge point dereferences it while keeping the
index lean (D3).

## Decision Drivers

- **No extra agent step (PRD R3, AC3).** Recording a result must ride the
  completion the agent already signals. The terminal-evidence write and the
  `ChildCompleted` append already happen on that path.
- **Uniform typed read across all children (PRD R2, R6, AC2, AC7).** A parent
  reads any child's result through one accessor with no per-child special
  casing, at every tree depth. This favors a typed envelope over a free-form
  blob, mirroring koto's existing typed `TerminalOutcome`.
- **Lean hot scan path (PRD R9, AC9).** The terminal-index line stays within
  `MAX_INDEX_LINE_BYTES` regardless of result payload size. Whatever the index
  carries must be bounded; the full result is dereferenced elsewhere.
- **Reuse existing surfaces; no new noun (PRD R4, R5, R7, D4, AC6).** The
  converge point is the `children-complete` gate; the directive is
  `GateBlocked`; the storage substrate is the session event log and the
  terminal index. No `koto request` command, no new `NextResponse` variant.
- **Forward-compatible, additive, concurrency-safe (PRD R10, R11, AC10,
  AC11).** New events fall through the `Unknown` arm on older koto builds; new
  struct fields are `#[serde(default, skip_serializing_if)]`; result writes
  preserve the atomic-append / atomic-rename discipline so N concurrent
  completions never corrupt one another.
- **Standalone koto value.** A solo coordinator converges with no companion
  plugins or multi-repo workspace. The design depends only on koto's own engine
  and CLI; teaching and provisioning layers are out of scope.
- **Convergence consumes results, never transcripts (PRD R8, AC8).** The result
  is the legible end-of-work artifact; the dereference path reads the recorded
  result event, never replays the child's working log.

## Considered Options

### Decision 1: How a child's result is recorded on completion

A child has to end up with a closed result attached to its completion. The
question is whether koto synthesizes that result from what the completion path
already produces, or whether the agent performs a distinct result-submission
action. This is the first decision because it sets whether convergence costs the
agent an extra round-trip — the very cost the fan-out exists to remove.

The completion path already does two relevant writes on a child's terminal tick,
both inside `handle_next` just before `backend.cleanup(child)`:
`append_child_completed_to_parent`, which projects a typed `TerminalOutcome`
(`Success` / `Failure` / `Skipped`) from the final state's `failure` and
`skipped_marker` template flags and appends a `ChildCompleted` event to the
parent's log; and `append_terminal_index_for_session`, which records the
terminal entry in the workspace index.

Key assumptions: the terminal evidence and the outcome projection are both
available at the promotion site (they are — that site computes `TerminalOutcome`
today); and the agent's terminal evidence is a sufficient source for a summary
and optional payload.

#### Chosen: Auto-promote terminal evidence

When a child reaches a terminal state, the completion path synthesizes the
result from data it already holds: the status is the same `TerminalOutcome` it
already computes for `ChildCompleted`; the summary is read from a
conventionally-named field on the terminal state's `accepts` block, falling back
to a default derived from the final state name; the optional payload carries the
latest `EvidenceSubmitted.fields`. The agent submits its terminal evidence the
way koto already expects and the result rides that same completion. This keeps
PRD R3 / AC3 true and removes the failure mode where a workflow is terminal but
has no result.

#### Alternatives Considered

**Explicit result-submission step**: the agent calls a dedicated command (a new
`koto result post`, or a `--with-data` submission against a result schema) at or
before completion. Rejected because it adds the agent round-trip the fan-out
exists to avoid, creates a "done but resultless" state every converge would have
to tolerate (complicating the blocked-set semantics in Decision 4), and a new
`koto result` noun conflicts with the no-new-noun boundary (PRD R7 / D4).

### Decision 2: Result-envelope type and exact field set

A parent must read any child's result the same way at any depth (PRD R2, R6).
The question is whether the result is a typed envelope with a fixed core or an
opaque free-form JSON object, and exactly which fields and types the typed form
carries.

Key assumptions: the result's status can reuse the existing `TerminalOutcome`
enum (it is the same classification the completion path already produces); and a
single bounded summary string plus an optional payload covers the common read
without forcing producers into a rigid schema.

#### Chosen: Typed minimal envelope

```rust
pub struct WorkflowResult {
    pub status: TerminalOutcome,        // reuse existing enum; snake_case wire form
    pub summary: String,                // bounded human-readable end-of-work statement
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}
```

`status` reuses `TerminalOutcome` verbatim — no new enum, and the result's
status is the same value `ChildCompleted` already carries. `summary` is a single
bounded human-readable string, the legible end-of-work artifact a parent reads
(PRD R8); it is bound-checked so a result never threatens line bounds downstream.
`payload` is an optional `serde_json::Value`, omitted from the wire when `None`
via `skip_serializing_if`, matching koto's additive-field idiom; it carries the
terminal evidence fields for a parent that wants structured detail, while a
parent that only needs outcome and summary ignores it.

#### Alternatives Considered

**Free-form JSON blob**: the producer writes any JSON and the parent reads an
opaque object. Rejected because it forces every converging parent to know each
child's private shape, defeating the uniform single-accessor read PRD R2 / R6 /
AC2 require, and breaking from koto's typed-outcome idiom (the existing
`TerminalOutcome` was deliberately typed over a stringly value so consumers match
exhaustively and the wire format stays stable). The optional `payload` already
preserves the producer freedom this option offered, without giving up the typed
common path.

### Decision 3 (critical): Result storage location and dereference, keeping the index lean

This is the core design question. The terminal index (`_terminal_index.jsonl`)
is the hot path: the discovery scan walks it line-by-line on every parent poll,
and each line is bounded to `MAX_INDEX_LINE_BYTES` (4096, within Linux
`PIPE_BUF`) so that concurrent `O_APPEND` writes from independent agents never
interleave — `append_terminal_index_entry` hard-errors on an overlength line
rather than risk a non-atomic append. An arbitrary-size result payload cannot
live in an index line. So the question is: where does the full result live, what
(if anything) does the index carry, and how does the parent dereference the
result at converge time without replaying any child transcript?

Key assumptions: the child's session event log is append-only NDJSON with the
same atomic-append discipline as the index (it is); a new closed-enum event
variant degrades gracefully on older koto builds via the existing `Unknown`
fallthrough (it does — confirmed by the index reader's
forward-compat-unknown-keys behavior and the event model's `Unknown` arm); and —
the decisive one — koto auto-cleans a child's session directory on its terminal
tick, so anything stored only on the child log can vanish before the parent
converges.

#### Chosen: Result on the child session log, bounded `has_result` flag in the index, result copy on the parent's `ChildCompleted`

The full result is written to the CHILD's own session event log as a new closed
event variant `EventPayload::RequestStoreResult` (wire `type:
"request_store.result"`, in the reserved `request_store.*` namespace) carrying
the `WorkflowResult` envelope. The child log is the natural home: it is the
append-only NDJSON stream where the terminal evidence already lives, written on
the same terminal tick, with the same atomicity guarantees.

The terminal-index entry gains exactly one additive field — a bounded
`has_result: bool` (serialized only when true via `skip_serializing_if`). This is
the done-bit: it tells the parent's converge gate, without opening any child,
that a result exists to dereference. It is bounded by construction, so the line
stays within 4096 bytes regardless of payload size (PRD R9 / AC9), and older
index readers already tolerate the extra key.

The decisive constraint is cleanup. koto auto-cleans the child session on its
terminal tick — which is precisely why `ChildCompleted` is appended to the
PARENT's log today as a fallback for the cleaned-up child. A result stored only
on the child log would be removed before the parent converges. The design
resolves this exactly as koto already resolves the outcome: the `WorkflowResult`
envelope is also carried as an additive `result: Option<WorkflowResult>` field
on the parent's `ChildCompleted` event. The converge gate dereferences the
result from its own log when the child has been cleaned up, or from the child's
`request_store.result` event when the child session still exists — never by
replaying a transcript (PRD R8 / AC8). The child-log copy is the durable record
for `koto query` / `koto status`; the parent-log copy is the converge-read
source; the index `has_result` flag is the cheap signal that gates the
dereference.

#### Alternatives Considered

**Embed the result in the index line**: serialize the full envelope into the
`TerminalIndexEntry` JSONL line. Rejected because it directly violates R9 / AC9 —
an arbitrary-size payload blows past the 4096-byte `PIPE_BUF` bound, and the
existing writer hard-errors on overlength lines; embedding would force
truncation or sacrifice the atomic-append guarantee that protects the
N-concurrent-writers acceptance criterion (AC11).

**Per-result sidecar file** (`<session>/result.json`): write each result to a
dedicated file and point the index at it. Rejected because it adds a third write
target with its own lifecycle to manage — creation, atomic-rename, cleanup,
stale-orphan recovery — beside the log and the index, the kind of parallel
surface PRD R7 / D4 push against. The session event log already provides
append-only durability and atomic writes; a sidecar re-implements that for no
gain and reintroduces the cleanup race (the sidecar is removed with the session
directory) without solving it as cleanly as the parent-log copy does.

### Decision 4: Converge surfacing through GateBlocked, and the converge set

The parent needs a converge point that stays blocked until its children's
results are in, names the outstanding children while blocked, and then surfaces
all results inline — all without a new top-level response shape or command noun
(PRD R4, R5, R7, D4). The question is whether to reuse koto's existing
`children-complete` gate and `GateBlocked` directive, or introduce a dedicated
converge gate and response variant.

Key assumptions: the converge set is the parent's linked children that the
`children-complete` gate already enumerates via `backend.list()` filtered on
`parent_workflow == parent` (it is — this is the same set the parent dispatches);
and the gate's structured output can carry a per-child result field (it already
emits a `children` array with per-task classification).

#### Chosen: Reuse the children-complete gate and GateBlocked

The converge point IS the existing `children-complete` gate on a
`materialize_children` parent state, so the converge set needs no new
abstraction — it is exactly the children the parent discovered and dispatched
(PRD R5). `build_children_complete_output` already produces a structured gate
output with a `children` array and `all_complete` / `pending` counts; the design
extends each child entry with a `result` field populated from the dereferenced
`WorkflowResult` (per Decision 3). The gate's pass predicate is tightened so it
is non-passing while any non-skipped child in the set has `has_result == false`.
While non-passing, `koto next` returns the existing `NextResponse::GateBlocked`
variant with a converge `blocking_condition` (the `temporal` / retry-later
category already used by `children-complete`) that names the outstanding
children by their fan-out identity; the parent is not advanced past the state.
When the last result lands, the gate passes, the state advances, and the
directive the agent reads carries every child's result inline — no child log
opened (PRD R4 / R8 / AC4 / AC8).

Because the converge point is just a gate and the result is just the
auto-promoted envelope, a mid-level coordinator converges its own children
through the same gate and then auto-promotes its OWN result on its terminal tick
for its parent — the same two mechanisms at every depth, with no depth-specific
code path (PRD R6 / AC7).

#### Alternatives Considered

**New dedicated converge gate type and NextResponse variant**: add a `converge`
gate type and a `Converged` / `ConvergeBlocked` response shape. Rejected because
it introduces the parallel surface PRD R7 / D4 / AC6 explicitly forbid — a second
gate model and a new top-level response an agent must learn, against koto's
minimal-surface design. The `children-complete` gate plus `GateBlocked` already
provide blocked-until-results-in semantics with a named outstanding set; a new
variant would duplicate it.

## Decision Outcome

**Chosen: 1A + 2A + 3A + 4A**

### Summary

A child's result is born on the same terminal tick koto already uses to wrap up a
child. When the child reaches a terminal state, the completion path — the same
code that today appends `ChildCompleted` to the parent and a terminal entry to
the workspace index — synthesizes a typed `WorkflowResult { status, summary,
payload }`: `status` is the `TerminalOutcome` already projected from the final
state's flags, `summary` comes from a conventionally-named field on the terminal
state's `accepts` block (or a default), and `payload` carries the terminal
evidence fields. No extra agent action is required.

That result is persisted in three places, each with a distinct job. It is
appended to the child's own session log as a new closed event,
`EventPayload::RequestStoreResult` (wire `request_store.result`) — the durable
record for status and query, degrading gracefully to `Unknown` on older koto
builds. A copy rides the parent's `ChildCompleted` event as an additive
`result: Option<WorkflowResult>` field, so the result survives the child
session's auto-cleanup and is readable from the parent's own log. And the
workspace terminal index gains a single bounded `has_result: bool` field — a
done-bit only, never the payload — so the hot discovery scan stays within its
4096-byte `PIPE_BUF` line bound and concurrent appends remain atomic.

The parent converges at its existing `children-complete` gate. Its set is the
children it already dispatched (`parent_workflow == parent`). The gate output's
per-child entries gain a `result` field, dereferenced from the child's
`request_store.result` event when the session is live or from the parent's
`ChildCompleted.result` when the child has been cleaned up — never by replaying a
transcript. The gate is non-passing while any non-skipped child has no result;
`koto next` returns the existing `GateBlocked` directive naming the outstanding
children, and the parent does not advance. When the last result lands the gate
passes, the state advances, and the cleared directive carries every child's
result inline. The same gate-plus-auto-promote pair runs at every depth, so a
mid-level coordinator converges its children and then carries its own result up
identically — uniform and recursive with no depth-specific path.

Implementation must handle: a terminal child whose `accepts` block has no
summary field (fall back to a final-state-derived default); a `Skipped` child
that never produced evidence (its result is the skipped status with a default
summary, and it does not block the converge set); a child cleaned up before the
parent polls (read the result from `ChildCompleted.result`); an older koto build
reading newer logs (the `request_store.result` event and the additive fields
fall through `Unknown` / serde defaults); and N children completing concurrently
(each appends its own result to its own child log and the parent's
`ChildCompleted` append uses the same atomic discipline, while the index carries
only the bounded flag).

### Rationale

The decisions reinforce one another around a single spine: the data koto already
computes on the terminal tick. Auto-promotion (1A) is only cheap because the
typed envelope (2A) reuses the `TerminalOutcome` the completion path already
projects — the same enum value flows into the child log event, the parent
`ChildCompleted`, and the converge read with no translation. The lean-index
choice (3A) is what makes the converge gate (4A) able to gate cheaply: the
`has_result` done-bit lets the discovery scan stay hot while the full result is
dereferenced lazily only at the converge point. And the cleanup race — the one
genuinely load-bearing detail — is resolved by carrying the result on the
parent's `ChildCompleted`, which is exactly the surface the converge gate reads,
so 3A and 4A close the loop on each other rather than each solving cleanup
separately.

The trade-offs accepted: the result is stored twice (child log + parent
`ChildCompleted`), a deliberate redundancy that buys cleanup-safety without a
new file lifecycle; and the summary is bounded, trading unbounded prose for a
legible, size-safe end-of-work statement with structured detail pushed into the
optional payload. Every new surface is additive — one event variant, three
optional/defaulted fields — so the change rides koto's existing forward-compat,
atomic-append, atomic-rename, and epoch-fencing machinery unchanged, and a solo
coordinator gets standalone converge value with no companion tooling.

## Solution Architecture

### Overview

The feature adds a typed `WorkflowResult` to a child's completion, persists it
on three existing surfaces (child session log, parent log, workspace index)
each carrying the part of the result it can afford, and reads it back at the
parent's existing `children-complete` gate. No new command, no new `koto next`
response variant, no new storage file.

### Components

- **`WorkflowResult` envelope** (new type, `src/engine/types.rs`): the typed
  `{ status: TerminalOutcome, summary: String, payload: Option<Value> }`
  envelope (Decision 2). `Serialize`/`Deserialize`, `payload` optional via
  `skip_serializing_if`.
- **`EventPayload::RequestStoreResult`** (new closed-enum variant,
  `src/engine/types.rs`): wire `type: "request_store.result"`, payload is a
  `WorkflowResult`. Added to the `payload_type` match arm and the
  `Event::deserialize` dispatch. Unknown to older koto goes through the existing
  `Unknown` fallthrough.
- **Result promotion** (extends `append_child_completed_to_parent` and the
  child terminal path in `src/cli/mod.rs`): synthesizes the `WorkflowResult` on
  the terminal tick from the existing `TerminalOutcome` projection plus the
  terminal evidence, appends `RequestStoreResult` to the child log, and attaches
  the envelope to the `ChildCompleted` event written to the parent.
- **`TerminalIndexEntry.has_result`** (new additive field,
  `src/engine/terminal_index.rs`): bounded `bool`, `#[serde(default,
  skip_serializing_if)]`. Set true when the child recorded a result. The reader
  and compaction paths carry it through unchanged (they already tolerate extra
  keys).
- **`ChildCompleted.result`** (new additive field on the existing variant,
  `src/engine/types.rs` plus `ChildCompletedPayload`): `Option<WorkflowResult>`,
  optional via serde so pre-feature parent logs round-trip.
- **Converge read** (extends `build_children_complete_output`, `src/cli/batch.rs`):
  each entry in the gate output's `children` array gains a `result` field,
  dereferenced from the child's `request_store.result` event (live child) or the
  parent's `ChildCompleted.result` (cleaned-up child). The gate's pass predicate
  treats a non-skipped child with no result as outstanding.
- **Converge directive** (no code change to the variant): the non-passing gate
  flows through the existing `NextResponse::GateBlocked` path
  (`src/cli/next_types.rs`); the converge `BlockingCondition` names outstanding
  children. The passing gate's output (with inlined results) is what the cleared
  directive carries.

### Key Interfaces

```rust
// src/engine/types.rs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowResult {
    pub status: TerminalOutcome,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

// new EventPayload variant
RequestStoreResult { result: WorkflowResult },

// ChildCompleted gains:
ChildCompleted {
    child_name: String,
    task_name: String,
    outcome: TerminalOutcome,
    final_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<WorkflowResult>,   // NEW
}

// src/engine/terminal_index.rs — TerminalIndexEntry gains:
#[serde(default, skip_serializing_if = "is_false")]
pub has_result: bool,                  // NEW, bounded
```

Gate output per-child entry (JSON) gains:
```json
{ "task": "...", "outcome": "success", "state": "...",
  "result": { "status": "success", "summary": "...", "payload": {} } }
```

### Data Flow

1. Child agent submits terminal evidence via `koto next --with-data` as it
   already does; the workflow reaches a terminal state.
2. On the terminal tick in `handle_next`, before `backend.cleanup(child)`:
   - synthesize `WorkflowResult` (status = projected `TerminalOutcome`, summary
     from the terminal `accepts` field or default, payload = terminal evidence
     fields);
   - append `RequestStoreResult` to the child log;
   - `append_child_completed_to_parent` writes `ChildCompleted { …, result:
     Some(envelope) }` to the parent log;
   - `append_terminal_index_for_session` writes the index entry with
     `has_result: true`.
3. Parent polls `koto next` at its `children-complete` (converge) state. The
   gate reads each child's result: live child reads its `request_store.result`
   event; cleaned-up child reads the parent's `ChildCompleted.result`. The index
   `has_result` flag is the cheap done-bit the discovery scan reads first.
4. Any non-skipped child without a result keeps the gate non-passing, so the
   `GateBlocked` directive names the outstanding children and the parent does
   not advance.
5. All results present means the gate passes, the state advances, and the
   cleared directive carries every child's result inline in the gate output.

## Implementation Approach

### Phase 1: Envelope and event types
Add `WorkflowResult`, the `RequestStoreResult` event variant, and the additive
`ChildCompleted.result` field; wire serialization, the `payload_type` arm, and
the deserialize dispatch. Unit tests: round-trip, snake_case status, optional
payload omitted, older-log graceful degrade to `Unknown`.
Deliverables: `src/engine/types.rs` (plus `ChildCompletedPayload`).

### Phase 2: Index `has_result` flag
Add the bounded `has_result` field to `TerminalIndexEntry`; thread it through
`append_terminal_index*`, the reader dedup, and the compaction body. Test that
the line stays within `MAX_INDEX_LINE_BYTES` for a large result and that the
field round-trips and defaults false on older lines.
Deliverables: `src/engine/terminal_index.rs`.

### Phase 3: Result promotion on completion
Synthesize the `WorkflowResult` at the terminal tick; append `RequestStoreResult`
to the child log; attach `result` to the parent `ChildCompleted`; set
`has_result` in the index. Define the summary-field convention and the
final-state-derived default; handle skipped children (status `Skipped`, default
summary). Test that completing a workflow the normal way produces a result with
no extra agent step.
Deliverables: `src/cli/mod.rs` (promotion plus the two existing append sites).

### Phase 4: Converge read at the gate
Extend `build_children_complete_output` to dereference and inline each child's
`result` (live event or parent `ChildCompleted` fallback) and tighten the pass
predicate (non-skipped plus no result equals outstanding). Confirm the
non-passing path emits `GateBlocked` with named outstanding children and the
passing path inlines results. Tests: blocked-set membership equals dispatched
children; cleared directive carries results; no child log replayed; three-level
recursion; N concurrent completions.
Deliverables: `src/cli/batch.rs`, gate evaluator wiring in `src/cli/mod.rs`.

## Security Considerations

This feature has a limited attack surface: it adds only local, append-only
writes and a typed envelope, with no new dependencies, no privilege change, and
no network or external-artifact handling. The dimensions that apply are
documented below.

**External artifacts and dependency trust — not applicable.** The feature
downloads and executes nothing and adds no dependencies; it reads and writes
only local NDJSON session logs and the local terminal index under
`<koto_root>`, using crates already in koto's tree (`serde`, `serde_json`,
`anyhow`) and the derive macros already in use.

**Permission scope — no escalation.** The result is written through the same
sites koto already holds: an append to a session event log, an append to the
terminal index, and a parent event append. No new files, directories, process
spawns, or network access. The terminal-index append keeps its `O_APPEND` +
fsync discipline; the additive `has_result` field does not change the open mode
or introduce a seek/`write_at`. The compaction lease (mode 0600) is untouched.

**Local input validation of the result envelope.** The result `summary` and
optional `payload` originate from agent-submitted terminal evidence. These are
trusted-tenant inputs in koto's model, but because the result is read back and
inlined into the parent's directive, the envelope must be handled defensively:
the `summary` is length-bounded; `payload` is treated as opaque
`serde_json::Value` and never evaluated or executed; and a `request_store.result`
event that fails to parse is skipped under the same skip-and-continue discipline
the index reader already uses, so a malformed result degrades gracefully rather
than aborting a converge.

**Data exposure — intentional, within the existing trust boundary.** The result
envelope is persisted in the child log and copied onto the parent's
`ChildCompleted` event, so a reader with access to the parent log can read child
results without opening the child — which is the feature's purpose
(convergence is a read). koto already places parent and child logs under the
same `<koto_root>` ownership, so no new trust boundary is crossed; result
content inherits the same local-filesystem trust model as all session logs, and
agents must not place secrets in a result any more than in evidence today. The
terminal index carries only the `has_result` boolean — never result content —
which is the minimum disclosure consistent with the lean-scan-path requirement.

**Concurrency integrity (koto's central invariant here).** N children completing
concurrently each append their result to their own child log and ride their own
parent `ChildCompleted` append — no shared file except the terminal index, to
which the design adds only a bounded boolean, explicitly preserving the
`MAX_INDEX_LINE_BYTES` (PIPE_BUF) atomic-append guarantee (the writer still
hard-errors on overlength lines). Each result is a single atomic event append,
so a converging parent never observes a partial or interleaved result
(PRD R11 / AC11). Release-time enforcement: keep the index append on `O_APPEND`
(no seek), keep the result envelope a single event, and keep the summary bounded.

Residual risk is low and matches koto's existing posture: a misbehaving agent
can write a large or misleading summary/payload, bounded by the same evidence
controls and local trust model that already govern session logs.

## Consequences

### Positive
- A coordinator converges a fan-out by reading its own directive — no child log
  opened — extending the clean-context benefit from dispatch through
  convergence (PRD R4 / R8).
- Uniform and recursive: one gate plus one auto-promote at every depth, no
  depth-specific code path (PRD R6).
- The hot discovery scan stays lean — only a bounded `bool` joins the index
  line, preserving the `PIPE_BUF` atomic-append guarantee (PRD R9 / R11).
- Entirely additive: one event variant and three optional/defaulted fields,
  riding koto's existing forward-compat, atomic-write, and epoch machinery; no
  new command noun or response variant (PRD R7 / R10 / D4).
- Standalone value: works for a solo coordinator with no companion tooling.

### Negative
- The result is stored twice (child log plus parent `ChildCompleted`), a
  deliberate redundancy that costs a small extra write and two copies on disk.
- The summary is bounded, so a verbose end-of-work narrative must be truncated
  or pushed into the optional payload.
- Promotion depends on a summary-field convention on the terminal state; a
  template that doesn't follow it falls back to a generic default summary.
- A child that completes with a large `payload` still writes that payload to two
  logs (not the index); very large payloads grow log size even though the index
  stays bounded.

### Mitigations
- The redundancy is the cleanup-safety mechanism, not accidental — the
  parent-log copy is the converge-read source precisely because the child
  session may be auto-cleaned; this is the same pattern koto already uses for
  the bare outcome.
- The summary bound is enforced and documented; structured detail belongs in
  `payload`, which the typed envelope explicitly accommodates.
- The default-summary fallback keeps every completion resultful (no
  "done but resultless" state) even when a template omits the convention.
- Log growth from large payloads is a producer concern bounded by the agent's
  own evidence size; the index — the only true hot path — is unaffected, and
  existing session retention/cleanup policy governs log size.
