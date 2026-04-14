# Simulation Round 3, Pair B2: retry-respawn meets the typed envelope

**Shape:** 5 siblings (`X1..X5`) under parent `coord`, mix of outcomes
at start (success, failure, skipped-marker, spawn_failed, running).
Plus a nested-batch probe (`coord-outer` → `Y` → inner coord).

**Round-3 purpose.** Round 2 landed `retry_failed` as an atomic,
typed-envelope operation but left the `spawn_failed` corner under-
specified (round-2 pair B2 Gap 3). Round 3 loosens R10: a
`spawn_failed` child is retry-eligible. The retry path branches at
runtime into three shapes — **retry-rewind** (rewinds the child's
event log), **retry-respawn-of-skip-marker** (deletes and respawns
skip marker), **retry-respawn-of-spawn_failed** (re-attempts
`init_state_file` from the current submission entry). Round 3 also
re-shapes the retry error envelope: adds `UnknownChildren` and
`ChildIsBatchParent` variants, kills the `current_outcome: "unknown"`
sentinel used in some round-2 walkthroughs.

Round 3 asks: does the envelope stay coherent across the whole edge
space now that `spawn_failed` is on the retryable side and two new
variants carve out previously sentinel-coded cases?

---

## Preamble — state at start of each scenario

Unless stated otherwise: parent `coord` has driven one round of work
and is parked at `analyze_failures` with this batch:

| child | state | outcome | note |
|---|---|---|---|
| `coord.X1` | `done` | `success` | clean |
| `coord.X2` | `done_blocked` | `failure` | `failure_reason: "compile error"` |
| `coord.X3` | `skipped_due_to_dep_failure` | `skipped` | `skipped_marker: true`; `skipped_because: "coord.X2"` |
| `coord.X4` | `null` (no state file) | `spawn_failed` | `TaskSpawnError.kind: template_not_found` |
| `coord.X5` | `working` | `pending` | still running |

Note that `X5` being `pending` while parent is at `analyze_failures`
is only coherent if the parent's transition out of `plan_and_await`
was driven by something other than `all_complete` — possibly a
template that routes to `analyze_failures` on `any_failed ||
any_spawn_failed && total_pending <= 1` or a cancellation flow. For
this simulation treat it as given; the point is to exercise retry
on a mix of outcomes, not to litigate the template.

Gate output at preamble:

```json
{
  "total": 5, "completed": 3, "pending": 1,
  "success": 1, "failed": 1, "skipped": 1, "blocked": 0, "spawn_failed": 1,
  "all_complete": false, "all_success": false,
  "any_failed": true, "any_skipped": true, "any_spawn_failed": true,
  "needs_attention": true
}
```

`reserved_actions` payload:

```json
{
  "name": "retry_failed",
  "description": "Re-queue failed, skipped, and spawn-failed children.",
  "payload_schema": {
    "children": {"type": "array<string>", "required": false, "default": ["coord.X2", "coord.X3", "coord.X4"]},
    "include_skipped": {"type": "boolean", "required": false, "default": true}
  },
  "applies_to": ["coord.X2", "coord.X3", "coord.X4"],
  "invocation": "koto next coord --with-data '{\"retry_failed\": {\"children\": [\"coord.X2\", \"coord.X3\", \"coord.X4\"]}}'"
}
```

Three names appear in `applies_to`: failure, skipped, and
spawn_failed. `X1` is excluded (success), `X5` is excluded (pending).
This matches R10's loosened eligibility: `failure | skipped |
spawn_failed`.

---

## Section 1: Transcript

### Scenario 1 — Retry on a mix including ineligible children

**AGENT:**

```
koto next coord --with-data '{"retry_failed": {"children": ["coord.X1", "coord.X2", "coord.X3", "coord.X4", "coord.X5"], "include_skipped": true}}'
```

**KOTO reasoning (R10 pre-append validation):**

- Load each child's `spawn_entry` / current state classification:
  - `coord.X1` → `current_outcome: "success"` → **not eligible**
  - `coord.X2` → `current_outcome: "failure"` → eligible
  - `coord.X3` → `current_outcome: "skipped"` → eligible
  - `coord.X4` → `current_outcome: "spawn_failed"` → eligible (round-3 loosening)
  - `coord.X5` → `current_outcome: "pending"` → **not eligible**

- R10 aggregates ineligibles into `ChildEligibility` entries. Per
  CD9 Part 4 + design line 2272 (`ChildNotEligible { children:
  Vec<ChildEligibility> }`): the **discriminator is
  `child_not_eligible`**, not `non_retryable_children`. The prompt's
  proposed `NonRetryableChildren` name is not in the design.
- CD9 atomicity: any ineligible child rejects the whole submission.
  Pre-append. No `EvidenceSubmitted` append. No rewinds. No
  respawns.

**KOTO response:**

```json
{
  "action": "error",
  "state": "analyze_failures",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Retry rejected: 2 children are not in a retryable state (coord.X1 is already successful; coord.X5 is currently running).",
    "details": [{"field": "retry_failed.children", "reason": "child_not_eligible"}],
    "batch": {
      "kind": "invalid_retry_request",
      "reason": "child_not_eligible",
      "children": [
        {"name": "coord.X1", "current_outcome": "success"},
        {"name": "coord.X5", "current_outcome": "pending"}
      ]
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

**Gloss.**
- The envelope lists ONLY the ineligible children (X1, X5) — not the
  eligible ones (X2, X3, X4). The agent's recovery path is "remove
  the ineligible names and resubmit."
- `current_outcome` values are the actual on-disk outcomes
  (`success`, `pending`); both are on the pre-R10-loosening
  non-retryable side, distinct from the new `spawn_failed` case.
- Atomic rejection means nothing happened to X2/X3/X4 either. The
  parent log and all child logs are bit-identical to the preamble.

`[VERIFIED: Scenario 1 commits. Mixed eligibility rejects atomically
via ChildNotEligible with a single typed variant carrying one entry
per ineligible child, differentiated by current_outcome. The
prompt's suggested NonRetryableChildren variant does not exist —
the design uses one ChildNotEligible variant for both success and
running, which is a strictly-less-surface-area choice.]`

---

### Scenario 2 — Retry on only the eligible three

**AGENT (correction):**

```
koto next coord --with-data '{"retry_failed": {"children": ["coord.X2", "coord.X3", "coord.X4"]}}'
```

**KOTO reasoning:**

- R10: `coord.X2` failure, `coord.X3` skipped, `coord.X4`
  spawn_failed. All three eligible. Payload passes.
- CD9 Part 4: retry branches by child:
  - `coord.X2`: **retry-rewind.** Append `Rewound` event to X2's log;
    bumps epoch, flips state machine back to the root.
  - `coord.X3`: **retry-respawn-of-skip-marker.** Delete X3's state
    file; next tick's scheduler will re-issue `init_state_file` with
    the real template (not the skip-marker synthetic).
  - `coord.X4`: **retry-respawn-of-spawn_failed** (round-3 path).
    No existing state file to delete. Next tick's scheduler
    re-attempts `init_state_file` using the CURRENT submission's
    entry for `X4` (R8 is vacuous: no prior `spawn_entry` to clash
    with). If the entry still fails, `spawn_failed` recurs.

- CD9 canonical sequence on the parent:
  1. Append `EvidenceSubmitted { retry_failed: {...} }` to parent log.
  2. Append `EvidenceSubmitted { retry_failed: null }` clearing event.
  3. Per-child rewrites above.
  4. Advance loop: transition on `evidence.retry_failed: present`
     fires `analyze_failures → plan_and_await`.
  5. Scheduler tick at `plan_and_await`: X2 resumes at fresh epoch,
     X3 respawned fresh from template, X4 `init_state_file`
     re-attempted (and either succeeds or re-errors).

**KOTO response (assuming X4 respawn succeeds this time — agent
fixed the template path out-of-band):**

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "Drive the children. coord.X2, coord.X3, coord.X4 are ready.",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 5, "completed": 1, "pending": 4,
      "success": 1, "failed": 0, "skipped": 0, "blocked": 0, "spawn_failed": 0,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false, "any_spawn_failed": false,
      "needs_attention": false,
      "children": [
        {"name": "coord.X1", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.X2", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.X3", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.X4", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.X5", "state": "working", "complete": false, "outcome": "pending"}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": ["coord.X3", "coord.X4"],
    "materialized_children": [
      {"name": "coord.X1", "outcome": "success", "state": "done"},
      {"name": "coord.X2", "outcome": "pending", "state": "working"},
      {"name": "coord.X3", "outcome": "pending", "state": "working"},
      {"name": "coord.X4", "outcome": "pending", "state": "working"},
      {"name": "coord.X5", "outcome": "pending", "state": "working"}
    ],
    "already": ["coord.X1", "coord.X2", "coord.X5"],
    "blocked": [], "skipped": [], "errored": [],
    "warnings": [],
    "feedback": {
      "entries": {
        "X2": {"outcome": "accepted"},
        "X3": {"outcome": "accepted"},
        "X4": {"outcome": "accepted"}
      },
      "orphan_candidates": []
    }
  }
}
```

**Gloss.** The response does NOT tell the agent which of X2/X3/X4
took which retry path (rewind vs. respawn-of-skip vs.
respawn-of-spawn_failed). `spawned_this_tick` surfaces X3 and X4
(both went through `init_state_file`) but not X2 (rewind on an
existing state file, no spawn). This is the closest-to-discriminator
signal the agent gets. `feedback.entries` marks all three
`accepted` — undifferentiated.

`[GAP 1: The retry path taken per child is NOT surfaced in the
response. An agent reading the scheduler outcome can infer:
    - if child in `spawned_this_tick` and prior outcome was
      `skipped` or `spawn_failed` → respawn path;
    - if child NOT in `spawned_this_tick` and prior outcome was
      `failure` → rewind path.
This inference requires cross-referencing prior-tick state, which
is fragile. Consider adding a `scheduler.feedback.entries.<name>`
sub-outcome like `"rewound"` / `"respawned"` for retry ticks, or a
new `retry_paths: {"X2": "rewind", "X3": "respawn", "X4":
"respawn_spawn_failed"}` section on the response. Decision 9 does
not specify this surface. Round 3 should commit one way or the
other: either make the path opaque (agents don't need to know) or
make it explicit.]`

`[VERIFIED: The retry-respawn-of-spawn_failed path uses R8's
vacuous-window logic. For X4 there is no prior `spawn_entry`, so the
current submission's entry for X4 is used. R8 is not a comparison —
it's a first-spawn. This matches CD9 Part 4's loosened semantic.]`

---

### Scenario 3 — UnknownChildren

**AGENT:**

```
koto next coord --with-data '{"retry_failed": {"children": ["coord.ghost"]}}'
```

**KOTO reasoning:**

- R10 loads child state for `coord.ghost`: not present on disk, not
  in the declared task set (the `EvidenceSubmitted.tasks` event never
  named it).
- CD9 Part 4 (design line 2273-2274): "Unknown child names (not
  present on disk) reject with
  `InvalidRetryReason::UnknownChildren { children: Vec<String> }`."
- Pre-append rejection. Parent log unchanged.

**KOTO response:**

```json
{
  "action": "error",
  "state": "analyze_failures",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Retry rejected: 1 named child does not exist on this parent (coord.ghost).",
    "details": [{"field": "retry_failed.children", "reason": "unknown_children"}],
    "batch": {
      "kind": "invalid_retry_request",
      "reason": "unknown_children",
      "children": ["coord.ghost"]
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

**Gloss.**
- `error.batch.reason` is `"unknown_children"` (flat discriminator).
  **The prompt said `error.batch.reason.kind: "unknown_children"`,
  nested one layer deeper. That is wrong against the design.** The
  envelope is flat: `batch.kind` at top level names the category
  (`invalid_retry_request`), `batch.reason` is the variant tag, and
  typed fields sit alongside as siblings (here: `children:
  Vec<String>`).
- `children` is `Vec<String>` for `UnknownChildren`, NOT
  `Vec<ChildEligibility>`. The shape differs from
  `ChildNotEligible`'s `children` by design — there is no
  `current_outcome` to report because the child doesn't exist.
- **`current_outcome: "unknown"` does not appear anywhere.** Round 2
  walkthrough a2 line 3299-3300 pinned this: "The `current_outcome`
  field on `ChildEligibility` never carries the sentinel
  `"unknown"`." Round 3 enforces it by routing unknown names to a
  different variant entirely.

`[VERIFIED: Round-3 UnknownChildren variant present; sentinel
pattern retired; envelope shape matches design.]`

`[ENVELOPE DISCREPANCY: Prompt specifies
`error.batch.reason.kind: "unknown_children"`. Design specifies
`error.batch.reason: "unknown_children"` (flat). Followed the
design. If round 3 intends to nest `reason` as an object with its
own discriminator, that's a spec bug — would break every round-1/2
assertion in the walkthrough.]`

---

### Scenario 4 — ChildIsBatchParent (nested)

Setup: a separate parent `coord-outer` has submitted one task `Y`
whose template is itself a batch-parent template (it declares a
`materialize_children` hook at its `plan_and_await` state). `Y`
spawned, `Y` ran its inner batch, the inner batch failed;
`coord-outer.Y` is now at `analyze_failures` with `any_failed: true`.
Outer parent `coord-outer` is at `analyze_failures` too (its
children-complete gate rolled up the inner failure).

**AGENT (at outer):**

```
koto next coord-outer --with-data '{"retry_failed": {"children": ["coord-outer.Y"]}}'
```

**KOTO reasoning:**

- R10 loads `coord-outer.Y`'s state. Classifier sees
  `materialize_children` hook declared (either on current state or
  historically). Per CD9 Part 4 (design line 1752-1763) and
  `InvalidRetryReason::ChildIsBatchParent { children: Vec<String> }`
  (design line 3275-3279): the submission rejects.
- Pre-append. No rewind would be safe — the retry machinery rewinds
  the outer `Y`'s event log but leaves Y's inner children's state
  files orphaned. CD9 documents this explicitly: "silently succeeding
  here would leave stale inner-batch state behind a rewound outer
  child." Cross-level retry is out of scope for v1.

**KOTO response:**

```json
{
  "action": "error",
  "state": "analyze_failures",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Retry rejected: coord-outer.Y is itself a batch parent. Retry at the level where the failure occurred (drive coord-outer.Y's inner batch, then bubble up).",
    "details": [{"field": "retry_failed.children", "reason": "child_is_batch_parent"}],
    "batch": {
      "kind": "invalid_retry_request",
      "reason": "child_is_batch_parent",
      "children": ["coord-outer.Y"]
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

**Gloss.**
- `children: Vec<String>` shape — same as `UnknownChildren`, no
  `current_outcome`. Y's outcome is technically `failure` (rollup),
  but the design dropped the detail because the variant carries its
  own meaning.
- Message text explicitly tells the agent to retry at the inner
  coordinator. Agent's recovery path is `koto next coord-outer.Y
  --with-data '{"retry_failed": {"children": ["coord-outer.Y.<inner>"]}}'`.

`[VERIFIED: ChildIsBatchParent variant fires; typed envelope; no
sentinel-coded detection (round 2 would have had to route this
through ChildNotEligible with `current_outcome: "failure"`, leaving
the agent no way to distinguish "real failure retryable at this
level" from "nested-batch, retry one level down"). Round 3
resolves.]`

---

### Scenario 5 — Mixed reasons: unknown + batch-parent

**AGENT:**

```
koto next coord-outer --with-data '{"retry_failed": {"children": ["coord-outer.ghost", "coord-outer.Y"]}}'
```

`coord-outer.ghost` doesn't exist; `coord-outer.Y` is a batch parent.
Two different `InvalidRetryReason` variants are in play for a single
submission.

**KOTO reasoning:**

- R10 classifies each child:
  - `coord-outer.ghost` → `UnknownChildren`
  - `coord-outer.Y` → `ChildIsBatchParent`
- CD11 Q11 pins **"typed enum discriminator"**. The envelope carries
  `error.batch.reason: <one tag>` — it is a single enum variant.
  Two variants cannot both fire in one response under the current
  envelope shape.
- The design does NOT specify the precedence rule for mixed-reason
  rejection. Options:
  - (a) First-match wins in some validation order.
  - (b) Group by the "most severe" / earliest-in-enum-definition-order.
  - (c) Aggregate via a new `AnyInvalidRetryReason { reasons:
        Vec<InvalidRetryReason> }` wrapping variant.

**KOTO behaviour (best-available-interpretation):**

R10 walks the children list in submission order. First classification
that yields an ineligibility sets the variant. Subsequent names of
the *same* variant accumulate into that variant's `children` list;
subsequent names of *different* variants are silently subsumed
under the first reported variant's discriminator, or listed under a
generic secondary "other_ineligible" bucket — neither is specified.

Pragmatic reading: emit the FIRST variant encountered; carry only
its children.

```json
{
  "action": "error",
  "state": "analyze_failures",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Retry rejected: 1 named child does not exist on this parent (coord-outer.ghost).",
    "details": [{"field": "retry_failed.children", "reason": "unknown_children"}],
    "batch": {
      "kind": "invalid_retry_request",
      "reason": "unknown_children",
      "children": ["coord-outer.ghost"]
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

Agent fixes `ghost`, resubmits `["coord-outer.Y"]`, gets the
`child_is_batch_parent` variant, fixes that. Two round-trips.

`[GAP 2 — MAJOR: CD9/CD11 do not specify behaviour when a single
submission surfaces multiple InvalidRetryReason variants. The
InvalidRetryReason enum (design line 3264-3287) is a tagged union —
one variant per response, by construction. This forces agents
through N round-trips for N different ineligibility types in one
submission. For N=2 (unknown + batch-parent), two round-trips. For
N=3 (add a non-eligible child too), three. Round 3 should either:
  (a) commit to an ordering rule and document it ("first unknown,
      then batch-parent, then non-eligible; earliest-encountered
      variant fires") so agents know which to fix first and can
      anticipate the next round-trip; or
  (b) add an aggregate `MultipleReasons { reasons:
      Vec<InvalidRetryReason> }` variant and ship the full
      classification in one response. This is what ChildNotEligible
      does WITHIN one variant (it accumulates multiple children
      across outcomes into one Vec<ChildEligibility>); extending the
      same philosophy across variants would be consistent.
Recommend (b). Agents benefit from a single rejection with full
detail. Round 3 leaves this at (a) by implication, which is the
unfriendly default. Flag for the round-3 decision log.]`

`[ENVELOPE DISCREPANCY: The prompt suggested possibly firing an
"aggregate AnyInvalidRetryReason" — not in the design. The design's
current InvalidRetryReason enum has six variants and none is an
aggregate. Round 3 has NOT added MultipleReasons / AnyInvalid
despite this being the natural place for it.]`

---

### Scenario 6 — Rewind, respawn, rewind again; BatchFinalized re-issue

Setup: from preamble, agent issues Scenario 2's retry
(`["X2","X3","X4"]`). X3 runs and succeeds. X2 runs and succeeds.
X4 respawns, runs, but fails again (different failure this time).
X5 still `pending` from scenario 2 preamble; assume it also
completes success by this point.

Current state:
- `coord.X1`: success
- `coord.X2`: success (post-retry)
- `coord.X3`: success (post-respawn)
- `coord.X4`: failure (`failure_reason: "runtime panic"`)
- `coord.X5`: success

Gate: `all_complete: true`, `any_failed: true, any_spawn_failed:
false`. Parent transitions to `analyze_failures` via the
`children-complete + any_failed` route. CD13: a **first**
`BatchFinalized` event is appended here (this is actually the
second entry into `analyze_failures` — the first was before the
original Scenario-2 retry. Was a `BatchFinalized` appended then?
Only if `children-complete` evaluated `all_complete: true` on that
earlier pass. In the preamble, `all_complete: false` (X5 still
pending), so **no** earlier `BatchFinalized`. This is the first.)

Parent log now carries:
```
... EvidenceSubmitted(tasks=...), SchedulerRan(...),
    EvidenceSubmitted(retry_failed=...), EvidenceSubmitted(retry_failed=null),
    SchedulerRan(...), Transitioned(plan_and_await→analyze_failures),
    BatchFinalized { view: <snapshot with X4 failure> }
```

Call this the **first BatchFinalized event**. Agent observes:

```json
{
  "action": "evidence_required",
  "state": "analyze_failures",
  "directive": "...",
  "reserved_actions": [{
    "name": "retry_failed",
    "applies_to": ["coord.X4"],
    ...
  }],
  "blocking_conditions": [...]
}
```

**AGENT:**

```
koto next coord --with-data '{"retry_failed": {"children": ["coord.X4"]}}'
```

**KOTO reasoning:**

- R10: X4 is `failure`, eligible. Rewind path (not respawn, this
  time — X4 has a state file with an epoch from the prior respawn).
- CD9 canonical sequence runs. `Rewound` event on X4.
- Advance loop transitions `analyze_failures → plan_and_await`.

**Now the key CD13 question: the first `BatchFinalized` event is
still on the parent log. But the parent has re-entered a batched
state. CD13 (design line 2419-2423): "Re-entering a batched state
(after `retry_failed`, for example) invalidates the prior
finalization: the next finalization appends a new
`BatchFinalized` event, and `batch_final_view` on terminal
responses always reflects the MOST RECENT `BatchFinalized`
event."**

Parent log after this tick:
```
... (everything above) ... BatchFinalized { ... },
    EvidenceSubmitted(retry_failed=...), EvidenceSubmitted(retry_failed=null),
    SchedulerRan(...), Transitioned(analyze_failures→plan_and_await)
```

- Old `BatchFinalized` still on log (append-only; nothing is ever
  removed).
- No new `BatchFinalized` yet — the parent is at `plan_and_await`
  again; `children-complete` hasn't evaluated `all_complete: true`
  in this pass.

Per CD13 line 2425-2436 **`batch.phase` semantics**: "The transient
single-tick window where the `BatchFinalized` event is appended but
the parent has not yet left the batched state is classified as
`'final'`: the event's existence is load-bearing, the parent's
current state is not. A retry tick that re-enters the batched state
does not immediately revert `phase` — the old `BatchFinalized` event
remains on the log until a new one supersedes it, and `koto status`
reports `'final'` with the previous snapshot until the new
finalization lands."

So during the retry pass:
- `koto status coord` reports `batch.phase: "final"` with the
  OLD snapshot (X4 still at `failure`, pre-retry) — stale but
  deterministic.
- `koto next coord` returns a `gate_blocked` response with LIVE
  gate output showing X4 at `pending` again. The gate output is
  live-fresh; the `batch_final_view` (on `done` responses) is not
  live and is not emitted on `gate_blocked` responses anyway.

X4 runs, succeeds. `all_complete: true, all_success: true`.
Parent transitions to `done`. Advance loop: `children-complete`
evaluates `all_complete: true` on a state with
`materialize_children` — CD13 appends a **second** `BatchFinalized`
event with the post-retry snapshot (X4 now success).

Terminal response on `done`:

```json
{
  "action": "done",
  "state": "done",
  "is_terminal": true,
  "batch_final_view": {
    "phase": "final",
    "summary": {
      "total": 5, "success": 5, "failed": 0, "skipped": 0,
      "pending": 0, "blocked": 0, "spawn_failed": 0
    },
    "tasks": [
      {"name": "X1", "child": "coord.X1", "outcome": "success"},
      {"name": "X2", "child": "coord.X2", "outcome": "success"},
      {"name": "X3", "child": "coord.X3", "outcome": "success"},
      {"name": "X4", "child": "coord.X4", "outcome": "success"},
      {"name": "X5", "child": "coord.X5", "outcome": "success"}
    ]
  }
}
```

`batch_final_view` reflects the MOST RECENT `BatchFinalized` event,
per CD13 line 2422-2423. The stale first event is still on the log
(visible via `koto query --events`) but every read-path surface
surfaces the second.

`[VERIFIED: CD13's most-recent-wins rule handles retry correctly.
The append-only log preserves audit for both finalizations; consumer-
facing surfaces dedupe to the latest. Round-3 retry-respawn does
not perturb this — retry-respawn for spawn_failed follows the same
BatchFinalized lifecycle as retry-rewind for failure.]`

---

## Section 2: Probes

### Probe A — `AlreadyTerminal` in feedback.entries

Scenario: after scenario-6 completion, agent resubmits the original
task list (not retry_failed; raw `tasks`) — agent didn't realize
the batch was done.

**AGENT:**

```
koto next coord --with-data '{"tasks": [{"name": "X1", "template": "impl-issue.md"}, ...5 tasks...]}'
```

**KOTO:**

- R8 runs on each entry against `spawn_entry` snapshots. All five
  match (agent re-submitted the exact same entries). R8 passes.
- Pre-append validation passes. `EvidenceSubmitted { tasks: [...] }`
  appends.
- Advance loop runs. Parent is at terminal `done`. Gate output:
  `all_complete: true` already. Loop stops.
- Scheduler tick: `classify_task` on each of the five. All five are
  at terminal-success. **Per CD10 (design line 1926, 1938):
  `EntryOutcome::AlreadyTerminal` is the outcome for each.**
  Distinct from `AlreadyRunning` (non-terminal) and `AlreadySkipped`
  (skipped_marker terminal).

```json
{
  "scheduler": {
    "spawned_this_tick": [],
    "materialized_children": [...five at success/done...],
    "already": ["coord.X1", "coord.X2", "coord.X3", "coord.X4", "coord.X5"],
    "blocked": [], "skipped": [], "errored": [],
    "feedback": {
      "entries": {
        "X1": {"outcome": "already_terminal"},
        "X2": {"outcome": "already_terminal"},
        "X3": {"outcome": "already_terminal"},
        "X4": {"outcome": "already_terminal"},
        "X5": {"outcome": "already_terminal"}
      },
      "orphan_candidates": []
    }
  }
}
```

`[VERIFIED: AlreadyTerminal surfaces distinctly from AlreadyRunning
and AlreadySkipped. Round 3 introduces this split per CD10's
EntryOutcome enum (design line 1917-1933). Agents can distinguish
"worker still running" (AlreadyRunning) from "task finished clean"
(AlreadyTerminal) from "task is skip-marker" (AlreadySkipped).]`

---

### Probe B — `spawn_entry` snapshot for retry-respawned spawn_failed child

Back to scenario 2. `coord.X4` was `spawn_failed` with the ORIGINAL
submission's entry referencing `does-not-exist.md`. Agent fixed the
template path out-of-band — but did not resubmit the task entry
itself. Submission for retry looks like:

```
koto next coord --with-data '{"retry_failed": {"children": ["coord.X4"]}}'
```

Retry-respawn fires. But **which `spawn_entry` does the new
`WorkflowInitialized` event carry?** CD9 Part 4 (design line 1746):
"re-attempts `init_state_file` on the next tick using the **CURRENT
submission's entry** for that name." But this retry payload contains
NO `tasks` field — only `retry_failed.children`. So the scheduler
must reach back into the **most recent `EvidenceSubmitted { tasks:
[...] }` event** and pull X4's entry from there.

- If the agent resubmitted `tasks` with a corrected template path
  BEFORE this retry, R8's union-by-name (CD10) means the latest
  entry for X4 wins — and that's what gets snapshotted.
- If the agent never resubmitted, the original (broken) entry is
  what's in the evidence log, and the retry-respawn re-attempts
  `init_state_file` with the same broken path. It re-fails with
  `spawn_failed`. Retry without correction is a no-op loop.

`[VERIFIED: `spawn_entry` on the new WorkflowInitialized event
matches CURRENT submission entry per CD9 Part 4 + CD10 union-by-name.
"Current" means the latest EvidenceSubmitted { tasks: [...] } entry
for that name; the retry payload itself doesn't carry tasks, so the
scheduler replays the evidence log to find it. Agents fixing a
spawn_failed child must resubmit tasks first (pre-spawn LWW
correction) and THEN retry_failed.]`

`[GAP 3 — MINOR: The design does not pin what happens if NO
EvidenceSubmitted { tasks } event exists for a named spawn_failed
child. This case can't arise from normal flow (the child must have
been in a prior task list to have reached `spawn_failed`), but
crash-resume scenarios (cloud-sync partial replay) could
theoretically produce it. Probably rejects via UnknownChildren but
the round-3 spec should pin the path explicitly.]`

---

### Probe C — Retry payload echo

Does the error envelope echo the submitted retry payload, or just
describe what was wrong?

Looking at every scenario above, the error envelope carries:
- `error.batch.kind` — category
- `error.batch.reason` — variant tag
- typed fields per variant (e.g. `children`, `extra_fields`)

It does NOT echo:
- The original `retry_failed` payload verbatim
- The `include_skipped` flag the agent submitted
- Any "you submitted X" context

Agents wanting to correlate the rejection to their submission must
keep their own copy. Under Decision 9's pre-append semantics this is
trivial (the submission is in the shell history / agent scratch),
but round-3 could add an optional `error.batch.submitted: {...}`
echo field for debugger ergonomics. The design does not do this.

`[VERIFIED: No payload echo. Agents correlate locally. Round-2 pair
A2 Finding 7 already flagged this; round 3 did not add an echo
field. Deliberate absence. Acceptable.]`

---

## Section 3: Findings

### F1. Retry-respawn-path opacity

Retry against a mix of eligible children (failure / skipped /
spawn_failed) succeeds without telling the agent which path each
child took (rewind vs. respawn-of-skip-marker vs.
respawn-of-spawn_failed). Inference requires cross-tick state
comparison. **Recommend:** add a per-entry retry-path discriminator
to `scheduler.feedback.entries` OR a dedicated `retry_paths` section
on the response.

### F2. `MultipleReasons` / `AnyInvalidRetryReason` absent

A retry payload that triggers two+ distinct `InvalidRetryReason`
variants (e.g., UnknownChildren + ChildIsBatchParent) forces
multi-round-trip recovery. The atomic-rejection philosophy
(ChildNotEligible accumulates multiple children into one variant) is
not extended across variants. **Recommend:** add `MultipleReasons
{ reasons: Vec<InvalidRetryReason> }` to emit the full
classification in one response. Current behaviour defaults to
"first-variant-wins" which is unspecified and should be pinned one
way or the other.

### F3. Envelope shape: `error.batch.reason` is flat, not nested

The round-3 prompt uses `error.batch.reason.kind: "unknown_children"`
phrasing (nested). The design uses flat: `error.batch.reason:
"unknown_children"`, with typed fields (e.g. `children`) alongside
at the same level. **Recommend:** pin the flat shape explicitly in
the round-3 walkthrough; the prompt's nested phrasing, if taken
literally, would be a breaking change from round 1/2.

### F4. `current_outcome: "unknown"` sentinel gone

Round 3's `UnknownChildren` variant fully subsumes the sentinel
pattern. `ChildEligibility.current_outcome` (design line 3298-3301)
explicitly documents "no `"unknown"` sentinel — unknown names
surface through `InvalidRetryReason::UnknownChildren` instead."
**Verified.**

### F5. `ChildIsBatchParent` cleanly carves out nested-batch retry

Pre-round-3, this case would have routed through `ChildNotEligible`
with the inner parent's `current_outcome: "failure"`, indistinguish-
able from a legitimate retryable failure. The new variant gives
agents a clear recovery path ("drive the inner coordinator").
**Verified.**

### F6. `AlreadyTerminal` distinct from `AlreadyRunning`

Round 3's CD10 EntryOutcome split is clean. Agents get three
terminal/non-terminal discriminators (`AlreadyTerminal`,
`AlreadyRunning`, `AlreadySkipped`) instead of round-1's single
`Already`. **Verified.**

### F7. `BatchFinalized` retry-re-issue holds under retry-respawn

CD13's most-recent-wins rule survives both retry-rewind and
retry-respawn semantics. `batch_final_view` on terminal `done`
responses shows the final (post-retry) snapshot; the old
`BatchFinalized` event remains in the audit log.
**Verified.**

### F8. Retry-respawn `spawn_entry` source is evidence-log-derived

For a `spawn_failed` child, the current `spawn_entry` used during
retry-respawn comes from the most-recent `EvidenceSubmitted {
tasks: [...] }` for that child name — not from the retry payload
itself (which carries only child names). Agents correcting a bad
template path must resubmit `tasks` FIRST (LWW correction) THEN
`retry_failed`. **Verified** but could be more prominent in the
koto-user skill documentation.

### F9. No retry-payload echo in error envelope

`error.batch` describes the variant + typed detail but does not
echo the submitted payload. Agents correlate locally. **Verified as
deliberate absence** (round-2 finding, round-3 unchanged).

---

## Section 4: Quick reference — round-3 retry envelope shape

```
{
  "action": "error",
  "state": "<parent_state>",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "<human>",
    "details": [{"field": "retry_failed[.<subfield>]", "reason": "<snake_case>"}],
    "batch": {
      "kind": "invalid_retry_request",          // literal; always this value
      "reason": "<InvalidRetryReason tag>",     // one of: no_batch_materialized,
                                                //   empty_child_list, child_not_eligible,
                                                //   unknown_children, child_is_batch_parent,
                                                //   retry_already_in_progress,
                                                //   mixed_with_other_evidence
      <typed fields per variant>:
        - child_not_eligible:       "children": [{"name", "current_outcome"}]
        - unknown_children:         "children": ["name", ...]            // flat strings
        - child_is_batch_parent:    "children": ["name", ...]            // flat strings
        - mixed_with_other_evidence: "extra_fields": ["key", ...]
        - others:                   (no extra fields)
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

`batch.reason` is **flat** (string literal). `batch.reason.kind`
(nested) does not exist. Typed detail fields sit alongside
`reason` as siblings, shape varies per variant.

Six `InvalidRetryReason` variants total; no aggregate variant.
