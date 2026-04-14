# Simulation Round 3, Pair B3: Retry-Heavy Observability Across Multi-Level Views

Scenario: 5-issue linear tail A -> B -> C -> D -> E with
`failure_policy: skip_dependents`. A succeeds; B fails (child writes
`failure_reason` via `done_blocked.default_action`). C, D, E are
synthesized as skip markers. The gate fires `all_complete: true,
any_failed: true, any_skipped: true, needs_attention: true`;
`BatchFinalized` appends; parent transitions to `analyze_failures`.
AGENT retries B with `include_skipped: true`, parent re-enters
`plan_and_await`, scheduler rewinds B and delete-and-respawns C, D, E
as skip markers (dependencies still failed at retry-submit moment).
B succeeds, then C cascades, then D, then E. Second `BatchFinalized`
appends. Parent transitions to `summarize`.

Round 3's job: **verify the observability story holds across a
retry-heavy workflow including multi-level views.**

---

## Section 1: Transcript

### Turn 1 -- AGENT: `koto status coord` (parked on `analyze_failures`, first finalization appended)

**AGENT thinks:** "Batch 1 finalized; I want the static view and a
sanity check that `batch.phase` is correctly labeled."

**KOTO:**
```json
{
  "workflow": "coord",
  "state": "analyze_failures",
  "is_terminal": false,
  "batch": {
    "phase": "final",
    "summary": {"total": 5, "success": 1, "failed": 1, "skipped": 3, "pending": 0, "blocked": 0, "spawn_failed": 0},
    "tasks": [
      {"name": "issue-A", "child": "coord.issue-A", "outcome": "success"},
      {"name": "issue-B", "child": "coord.issue-B", "outcome": "failure", "reason": "unit tests failed: 3 assertions, see logs", "reason_source": "failure_reason"},
      {"name": "issue-C", "child": "coord.issue-C", "outcome": "skipped", "synthetic": true, "skipped_because": "coord.issue-B", "skipped_because_chain": ["coord.issue-B"], "reason_source": "skipped", "reason": "coord.issue-B"},
      {"name": "issue-D", "child": "coord.issue-D", "outcome": "skipped", "synthetic": true, "skipped_because": "coord.issue-C", "skipped_because_chain": ["coord.issue-C", "coord.issue-B"], "reason_source": "skipped", "reason": "coord.issue-B"},
      {"name": "issue-E", "child": "coord.issue-E", "outcome": "skipped", "synthetic": true, "skipped_because": "coord.issue-D", "skipped_because_chain": ["coord.issue-D", "coord.issue-C", "coord.issue-B"], "reason_source": "skipped", "reason": "coord.issue-B"}
    ],
    "ready": [], "blocked": [], "skipped": ["issue-C", "issue-D", "issue-E"], "failed": ["issue-B"]
  }
}
```

Gloss: `batch.phase: "final"` because a `BatchFinalized` event is
present (design lines 2425-2436). Three of the four `reason_source`
enum variants appear in this single view: `failure_reason` (B),
`skipped` (C, D, E), and by contrast in the success case on A it is
omitted (design line 2521). The chain on E is
`[D, C, B]` -- closest ancestor first, root failure last (design
lines 2492-2494: "topological order (closest ancestor first, root
failure last)"). For E: `skipped_because = D` (direct blocker per
lines 2474-2475) and `skipped_because_chain[-1] = B` (the root).

`[CHECK 1: Design commits "reason echoes skipped_because_chain[-1]"
(line 2516) for reason_source: "skipped". Chain is ordered direct-
first-root-last, so chain[-1] is the root failure name (B), not the
direct blocker (D). That matches all three skip rows here.]`

### Turn 2 -- AGENT submits retry, parent re-enters `plan_and_await`

**AGENT:** `koto next coord --with-data '{"retry_failed": {"children": ["coord.issue-B"], "include_skipped": true}}'`

**AGENT thinks:** "Retry B. Skip dependents cascade, so C/D/E will be
re-queued as skip markers until B actually succeeds."

**KOTO (abridged response):**
```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "output": {
      "total": 5, "completed": 1, "pending": 1, "success": 1, "failed": 0, "skipped": 3, "blocked": 0, "spawn_failed": 0,
      "all_complete": false, "all_success": false, "any_failed": false, "any_skipped": true, "needs_attention": false,
      "children": [
        {"name": "coord.issue-A", "state": "done", "outcome": "success"},
        {"name": "coord.issue-B", "state": "working", "outcome": "pending"},
        {"name": "coord.issue-C", "state": "skipped_due_to_dep_failure", "outcome": "skipped", "synthetic": true, "skipped_because": "coord.issue-B", "skipped_because_chain": ["coord.issue-B"]},
        {"name": "coord.issue-D", "state": "skipped_due_to_dep_failure", "outcome": "skipped", "synthetic": true, "skipped_because": "coord.issue-C", "skipped_because_chain": ["coord.issue-C", "coord.issue-B"]},
        {"name": "coord.issue-E", "state": "skipped_due_to_dep_failure", "outcome": "skipped", "synthetic": true, "skipped_because": "coord.issue-D", "skipped_because_chain": ["coord.issue-D", "coord.issue-C", "coord.issue-B"]}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": ["coord.issue-B"],
    "materialized_children": [...],
    "respawning": ["coord.issue-C", "coord.issue-D", "coord.issue-E"]
  }
}
```

### Turn 3 -- AGENT: `koto status coord` (parent on `plan_and_await`, mid-retry)

**AGENT thinks:** "We're back in `plan_and_await` with a retry in
flight. What's `batch.phase` now? What does `batch_final_view` show?"

**KOTO:**
```json
{
  "workflow": "coord",
  "state": "plan_and_await",
  "is_terminal": false,
  "batch": {
    "phase": "final",
    "summary": {"total": 5, "success": 1, "failed": 1, "skipped": 3, "pending": 0, "blocked": 0, "spawn_failed": 0},
    "tasks": [
      {"name": "issue-A", "child": "coord.issue-A", "outcome": "success"},
      {"name": "issue-B", "child": "coord.issue-B", "outcome": "failure", "reason": "unit tests failed: 3 assertions, see logs", "reason_source": "failure_reason"},
      {"name": "issue-C", "child": "coord.issue-C", "outcome": "skipped", "synthetic": true, "skipped_because": "coord.issue-B", "skipped_because_chain": ["coord.issue-B"], "reason_source": "skipped", "reason": "coord.issue-B"},
      {"name": "issue-D", "child": "coord.issue-D", "outcome": "skipped", "synthetic": true, "skipped_because": "coord.issue-C", "skipped_because_chain": ["coord.issue-C", "coord.issue-B"], "reason_source": "skipped", "reason": "coord.issue-B"},
      {"name": "issue-E", "child": "coord.issue-E", "outcome": "skipped", "synthetic": true, "skipped_because": "coord.issue-D", "skipped_because_chain": ["coord.issue-D", "coord.issue-C", "coord.issue-B"], "reason_source": "skipped", "reason": "coord.issue-B"}
    ],
    "ready": [], "blocked": [], "skipped": ["issue-C", "issue-D", "issue-E"], "failed": ["issue-B"]
  }
}
```

Gloss: Per the design, `phase` DOES NOT revert to `"active"` when
retry re-enters the batched state. Lines 2432-2436 say explicitly:
"A retry tick that re-enters the batched state does not immediately
revert `phase` -- the old `BatchFinalized` event remains on the log
until a new one supersedes it, and `koto status` reports `"final"`
with the previous snapshot until the new finalization lands."

So during the retry window, `batch.phase: "final"` shows the STALE
snapshot (B failed, C/D/E skipped) even though the live children
include a running `coord.issue-B`. Live state is available via
`blocking_conditions[0].output` on the next `koto next coord` tick;
`batch_final_view`-style snapshots are explicitly frozen-at-last-
`BatchFinalized` (lines 2523-2533).

`[CHECK 2 -- NON-ISSUE: The user's round-3 brief framed this as
"tension" between frozen-at-first and most-recent. The design resolves
it: there is only ever ONE current `BatchFinalized` reference, namely
"most recent". It was frozen at THAT event's append time, and stays
frozen until the NEXT event supersedes it. No tension if you read
"most recent" as selection and "frozen" as the per-event property.]`

`[CHECK 3 -- PROBE 7 ANSWER: The scenario's Probe 7 question "is
batch.phase now 'active' (we're back in batch execution)?" -- the
design answer is NO. It stays "final". The design's rationale: the
`BatchFinalized` event is load-bearing; the parent's current state
is not. This is counter-intuitive (agents would naturally expect
"I'm back in the batched state, therefore active"), but the design
text is explicit.]`

### Turn 4 -- AGENT drives B; B succeeds; scheduler cascade

AGENT drives `coord.issue-B` to `done`. On the next `koto next coord`
tick, the scheduler sees B terminal-success. C was a skip marker with
`waits_on: [issue-B]` -- the scheduler delete-and-respawns C as a
real child (lines 1043-1054: "whose dependencies become all-success
are delete-and-respawned [back to a real child]"). C runs, succeeds.
Then D: delete-and-respawn, run, succeed. Then E. After E's success
the gate fires `all_complete: true, all_success: true`. Advance loop
appends a SECOND `BatchFinalized` event and transitions to
`summarize`.

### Turn 5 -- AGENT: `koto next coord` (arriving at `summarize`)

**AGENT thinks:** "Terminal tick. `batch_final_view` should carry the
NEW snapshot (all success) per the 'MOST RECENT' rule in lines
2422-2423."

**KOTO:**
```json
{
  "action": "done",
  "state": "summarize",
  "directive": "Write a summary covering which issues succeeded, which failed, and why. The batch_final_view field in this response carries the full snapshot so you don't need a second command.",
  "is_terminal": true,
  "batch_final_view": {
    "phase": "final",
    "summary": {"total": 5, "success": 5, "failed": 0, "skipped": 0, "pending": 0, "blocked": 0, "spawn_failed": 0},
    "tasks": [
      {"name": "issue-A", "child": "coord.issue-A", "outcome": "success"},
      {"name": "issue-B", "child": "coord.issue-B", "outcome": "success"},
      {"name": "issue-C", "child": "coord.issue-C", "outcome": "success"},
      {"name": "issue-D", "child": "coord.issue-D", "outcome": "success"},
      {"name": "issue-E", "child": "coord.issue-E", "outcome": "success"}
    ],
    "ready": [], "blocked": [], "skipped": [], "failed": []
  }
}
```

Gloss: `batch_final_view` reflects the SECOND `BatchFinalized`
event, which carries the all-success snapshot. No `reason_source` on
any row (design line 2521: "omitted for successful or not-yet-
terminal children"). The first `BatchFinalized` event is still in
the log (event log is append-only) but `derive_batch_view` reads only
the MOST RECENT one (design lines 2422-2423). The agent writing the
summary sees the final retry-success view, not the stale first-pass
view.

### Turn 6 -- AGENT: `koto query coord --events`

**AGENT thinks:** "I want to see the raw event log. There should be
TWO `BatchFinalized` events -- will this confuse a replay tool?"

**KOTO (abridged):**
```json
{
  "workflow": "coord",
  "state": "summarize",
  "events": [
    {"type": "WorkflowInitialized"},
    {"type": "Transitioned", "to": "plan_and_await"},
    {"type": "EvidenceSubmitted", "fields": {"tasks": [...]}},
    {"type": "SchedulerRan", "spawned": ["coord.issue-A"]},
    {"type": "SchedulerRan", "spawned": ["coord.issue-B"]},
    {"type": "SchedulerRan", "spawned_as_skipped": ["coord.issue-C", "coord.issue-D", "coord.issue-E"]},
    {"type": "BatchFinalized", "seq": 1, "view": {"summary": {"total": 5, "success": 1, "failed": 1, "skipped": 3, ...}, ...}},
    {"type": "Transitioned", "to": "analyze_failures"},
    {"type": "EvidenceSubmitted", "fields": {"retry_failed": {"children": ["coord.issue-B"], "include_skipped": true}}},
    {"type": "EvidenceSubmitted", "fields": {"retry_failed": null}},
    {"type": "Transitioned", "to": "plan_and_await"},
    {"type": "SchedulerRan", "respawning": ["coord.issue-B", "coord.issue-C", "coord.issue-D", "coord.issue-E"]},
    {"type": "SchedulerRan", "delete_and_respawn": ["coord.issue-C"]},
    {"type": "SchedulerRan", "delete_and_respawn": ["coord.issue-D"]},
    {"type": "SchedulerRan", "delete_and_respawn": ["coord.issue-E"]},
    {"type": "BatchFinalized", "seq": 2, "view": {"summary": {"total": 5, "success": 5, "failed": 0, ...}, ...}},
    {"type": "Transitioned", "to": "summarize"}
  ]
}
```

`[CHECK 4 -- PROBE 13 GAP: The user's round-3 brief asks "is there a
marker like `superseded_by` on the first `BatchFinalized` event?"
The design text (lines 2419-2423, 2432-2436) uses the word
"supersedes" in prose -- "a new `BatchFinalized` event supersedes
the prior one" -- but does NOT commit an explicit `superseded_by`
field on the event payload. `derive_batch_view` discovers supersession
by event recency (last one wins); agents reading the raw event log
see both events in order, without a pointer from the first to the
second. Call this G1.]`

For agents writing replay/diff tooling:
- Both events are present in order; supersession is LAST-WINS by log
  position. That is sufficient for `batch_final_view` semantics.
- For an agent trying to render "retry cycle 1 ended in failure,
  retry cycle 2 ended in success", they must scan the event stream
  and pair each `BatchFinalized` with the intervening `retry_failed`
  evidence submission. No single event carries that linkage.
- A `superseded_by` field would let replay tools skip past obsolete
  `BatchFinalized` events without scanning. Not strictly necessary
  (replay cost is linear in event count), but a mild ergonomic loss.

### Turn 7 -- AGENT: `koto status coord` after terminal (all-success)

**KOTO:**
```json
{
  "workflow": "coord",
  "state": "summarize",
  "is_terminal": true,
  "batch": {
    "phase": "final",
    "summary": {"total": 5, "success": 5, "failed": 0, "skipped": 0, "pending": 0, "blocked": 0, "spawn_failed": 0},
    "tasks": [
      {"name": "issue-A", "child": "coord.issue-A", "outcome": "success"},
      {"name": "issue-B", "child": "coord.issue-B", "outcome": "success"},
      {"name": "issue-C", "child": "coord.issue-C", "outcome": "success"},
      {"name": "issue-D", "child": "coord.issue-D", "outcome": "success"},
      {"name": "issue-E", "child": "coord.issue-E", "outcome": "success"}
    ],
    "ready": [], "blocked": [], "skipped": [], "failed": []
  }
}
```

Gloss: `batch.phase: "final"` stays final in terminal state. Payload
shape matches Turn 5's `batch_final_view` exactly. Probe 4 answered:
the terminal `done` response's `batch_final_view` mirrors the `koto
status` `batch` section shape (both derive from the same
`BatchFinalized` event via `derive_batch_view`).

### Turn 8 -- Counterfactual: E's status if coord were a child of an outer parent

**AGENT thinks (hypothetical):** "If coord were itself `outer.coord`,
a child of an outer parent, would outer's `batch_final_view` include
coord's `batch_final_view`?"

Per design lines 2535-2541: **NO.** `batch_final_view` is per-level,
not recursive. Outer's `batch_final_view` lists `outer.coord` as one
row with the aggregate outcome (success/failure/skipped) but does
NOT embed the 5-row task list from coord's own finalization. If the
outer observer wants coord's inner progress, they use
`subbatch_status` on `MaterializedChild` (a lightweight
`BatchSummary` snapshot) or call `koto status outer.coord` directly
to replay coord's `BatchFinalized` event from its own log.

This separation prevents unbounded payload size (imagine 5 levels of
1000-child batches) and keeps each level's observability self-
contained and replayable.

### Turn 9 -- Edge case: retry without an `analyze_failures` transition

**AGENT thinks (hypothetical):** "If the parent template had NO
`analyze_failures` state -- just `plan_and_await -> summarize` -- and
something failed, could I submit `retry_failed`?"

Per walkthrough.md lines 123-126 and the W4 warning (lines 101-104,
and design line 3829): W4 is a compile warning that fires if a
`materialize_children` state routes only on `all_complete: true`. A
batch with any failed child satisfies `all_complete: true` and slides
to `summarize` before the agent can submit `retry_failed` -- unless
the parent has a state that accepts `retry_failed` in its `accepts`
block. The round-1 blocker (no recovery window) is avoided in
practice because W4 prompts authors to route on `needs_attention`,
giving them an `analyze_failures`-shaped state to hold the retry.

Mechanically: `retry_failed` is only intercepted when the parent's
current state accepts it (design line 1170: "that declares
`retry_failed` in `accepts`"). A template without a retry-accepting
state simply cannot submit `retry_failed`. This is the round-1
blocker-equivalent, but W4 makes it diagnosable at compile time.

`[CHECK 5 -- EDGE CASE: If the author ignores W4 and ships a
template with only `all_complete: true` routing, and a failure
happens, the agent is stuck in `summarize` (terminal). `koto status`
still shows `batch.phase: "final"` with the failure details. The
agent cannot retry -- there's no state to accept `retry_failed`.
Recovery is "init a new workflow and resubmit". This is unreachable-
by-warning, not unreachable-by-check. Still technically reachable
with ignored warnings. Call this G2.]`

### Turn 10 -- Edge case: `batch.phase` before any finalization

**AGENT thinks (hypothetical):** "On the very first `koto status
coord` call -- right after `koto init`, before any tick -- what does
`batch` look like?"

Per design lines 2425-2428: `"active"` means the current state carries
a `materialize_children` hook AND no `BatchFinalized` event has been
appended yet. At init time the parent is in `plan_and_await` which
has the hook, and no `BatchFinalized` exists -> `phase: "active"`.
Summary rows are all zero. Tasks list is empty (no evidence
submitted). This section IS present:

```json
{
  "batch": {
    "phase": "active",
    "summary": {"total": 0, "success": 0, "failed": 0, "skipped": 0, "pending": 0, "blocked": 0, "spawn_failed": 0},
    "tasks": [],
    "ready": [], "blocked": [], "skipped": [], "failed": []
  }
}
```

`[CHECK 6 -- PROBE edge case answer: `batch_final_view` is
ABSENT before first finalization (design line 2443: "present when
the parent log contains at least one `BatchFinalized` event").
`koto status` still emits the `batch` section with `phase: "active"`
so the agent knows batch machinery is engaged but not yet
finalized. How does the agent know whether to look for
`batch_final_view`? Answer: look at `batch.phase`. `"active"` means
`batch_final_view` will NOT be present on `done` responses;
`"final"` means it WILL be present. This is inferable but is not
called out explicitly in the design text. Call this G3
(documentation-only).]`

---

## Section 2: Probe answers

**Probe 1 -- `reason_source` vocabulary across cycles.** In this
scenario across both cycles we observe three of four variants:
`failure_reason` (B in cycle 1), `state_name` (not observed because
B did write the key; would appear if B's template lacked W5
mitigation), `skipped` (C, D, E in cycle 1). The fourth variant
`not_spawned` is not reachable in this scenario (no `spawn_failed`
outcomes). Design commits all four values (lines 2510-2519). Cycle 2
has no `reason_source` anywhere -- all rows successful.

**Probe 2 -- `skipped_because` for E.** Round 3 stated "earliest-in-
submission-order failed ancestor". For E with chain [D, C, B], the
failed ancestor is B (D and C are skipped, not failed). Design line
2491-2492 says `skipped_because` "names the EARLIEST-IN-SUBMISSION-
ORDER failed ancestor". For E this would be **B**, not D. **However,
the round-2 simulation pair_a3 and walkthrough.md both show
`skipped_because` as the DIRECT blocker (D for E), not the root
failure (B).** This contradicts design line 2491-2492. Either:
(a) the design's "earliest-in-submission-order failed ancestor" rule
applies only to **diamond** cases (multiple direct-failed parents)
and the linear-tail case still uses direct blocker, or
(b) walkthrough.md and pair_a3 are stale w.r.t. design round 3.

Reading design lines 2474-2497 carefully:
- Line 2474-2475: "singular `skipped_because: <name>` field (the
  *direct* upstream blocker)" -- clear: direct blocker.
- Line 2480-2481: E example shows `skipped_because: "D"` -- direct
  blocker.
- Line 2489-2496: "Tie-break for `skipped_because`. In diamond skip
  scenarios where two `waits_on` ancestors both failed..." -- scoped
  to diamonds only.

**Resolution:** `skipped_because` is the direct blocker in all cases.
The "earliest-in-submission-order failed ancestor" rule applies only
when the skipped child has MULTIPLE directly-failed `waits_on`
parents (a diamond). For E in our linear tail, D is the direct
blocker regardless of whether D is failed or skipped. The user's
round-3 brief misread the tie-break rule as universal; the design
scopes it to diamonds. Call this a brief-misreading, not a design
gap.

**Probe 3 -- `skipped_because_chain` topological order.** Design
line 2493-2494: "in topological order (closest ancestor first, root
failure last)". Direction is **source-to-sink** walked in reverse,
which renders as `[direct_blocker, ..., root_failure]`. For E in
our 5-tail: `["D", "C", "B"]` -- D is the direct blocker (closest
ancestor), B is the root failure (last). Walkthrough.md and pair_a3
both render this correctly.

**Probe 4 -- Terminal response at `summarize`.** Verified in Turn 5
and Turn 7: `batch_final_view` on the `done` response is shape-
identical to the `batch` section in `koto status`. Both derive from
the same `BatchFinalized` event. The only difference is the outer
envelope (`done` adds `action`, `state`, `directive`, `is_terminal`;
`status` adds `workflow`, `state`, `is_terminal`).

**Probe 5 -- Per-level non-recursive `batch_final_view`.** Verified
via design lines 2535-2541. If coord were `outer.coord`, outer's
`batch_final_view` would list `outer.coord` as a row with aggregate
outcome but no embedded batch view. Use `subbatch_status` on
`MaterializedChild` for a peek at inner progress.

**Probe 6 -- `batch.phase` transitions.**
| Moment | `batch.phase` | Reason |
|--------|---------------|--------|
| Init (pre-first-tick) | `"active"` | Hook present, no BatchFinalized |
| During first pass, pre-finalization | `"active"` | Same |
| Just after first BatchFinalized appends | `"final"` | Event exists |
| Transition to `analyze_failures` | `"final"` | Event still exists |
| Parked on `analyze_failures` | `"final"` | Event still exists |
| `retry_failed` submitted, transition back to `plan_and_await` | `"final"` | Old event still load-bearing per 2432-2436 |
| Parent on `plan_and_await` mid-retry | `"final"` | Same |
| Second `BatchFinalized` appends | `"final"` (now new view) | New event supersedes |
| Transition to `summarize` (terminal) | `"final"` | Event persists |

No ambiguity. The design's "the event's existence is load-bearing,
the parent's current state is not" rule (line 2431-2432) gives a
single deterministic predicate: phase is `"final"` iff a
`BatchFinalized` event exists anywhere in the log. This means phase
is **sticky once set**.

---

## Section 3: Findings

### F1. `superseded_by` marker on stale `BatchFinalized` events

**Observation:** After a retry cycle, the event log contains
multiple `BatchFinalized` events. `derive_batch_view` resolves
supersession by "last one wins" (implicit via log ordering). Replay
tools walking the event log see both events without a pointer from
the first to the second. The design uses the word "supersedes"
(lines 2420-2423, 2432-2436) but doesn't commit a `superseded_by`
field.

**Severity:** LOW. Ergonomic nit for replay/diff tooling; mechanically
fine.

**Location:** Design lines 2419-2436.

**Proposed resolution:** (a) Add a `superseded: true` boolean or
`superseded_by: <seq>` field on `BatchFinalized` events that are no
longer the most-recent one (would require writing the prior event's
metadata on new-event append -- violates append-only). Or (b) Commit
in Decision 13: "`BatchFinalized` events are never mutated;
supersession is inferred by log recency. Replay tools should scan
forward for the last `BatchFinalized` event." Option (b) preserves
the append-only invariant and is strictly a documentation fix.

### F2. Phase stickiness deserves a documentation callout

**Observation:** `batch.phase` becomes sticky-`"final"` after the
first finalization for the remaining life of the parent workflow.
A retry re-entering the batched state does NOT revert the label to
`"active"`. This is correct per design (load-bearing event, not
load-bearing state), but counterintuitive enough that agents could
misread it -- expecting `"active"` whenever the parent is on a
batched state.

**Severity:** LOW. The design states the rule explicitly; the gap
is that a distracted reader might skim past it. Not a design defect.

**Location:** Design lines 2425-2436.

**Proposed resolution:** Add a one-line table to Decision 13
mirroring the Probe 6 transition table above, making phase-sticky
behavior visually obvious.

### F3. Unreachable `retry_failed` when W4 is ignored

**Observation:** If a template author ignores W4 and ships
`materialize_children` routing only on `all_complete: true`, a
failed batch lands directly in a terminal state. The agent has no
state that accepts `retry_failed`. Recovery requires a new workflow
init. This is the round-1 blocker in documentation form -- W4 makes
it diagnosable but not preventable.

**Severity:** LOW. W4 is the mitigation; authors who ignore
warnings carry the consequence. Not a design defect.

**Location:** W4 spec lines 3829; Decision 13 alternative 2571-2577
("Make missing `failure_reason` writer a hard error (E11). Rejected:
overly strong").

**Proposed resolution:** None. W4 is correctly placed at warning
level per the design's "respect authorial intent while flagging the
common mistake" philosophy.

### F4. `batch_final_view` presence indicator via `batch.phase`

**Observation:** The agent has no direct signal on whether
`batch_final_view` will be present on a `done` response. They can
infer from `batch.phase`: `"final"` means yes, `"active"` means no.
This is not explicitly documented in Decision 13.

**Severity:** LOW. Inferable; just not spelled out.

**Location:** Design lines 2442-2447 (batch_final_view on done) and
lines 2425-2436 (batch.phase).

**Proposed resolution:** Add to Decision 13: "`batch_final_view` is
present on `done` responses iff `batch.phase == 'final'` on `koto
status`. Agents can check `batch.phase` to predict whether the
terminal tick will carry the field."

### F5. Round-2's CHECK 3 (pair_a3) is resolved by round-3 design

**Observation:** Pair A3's CHECK 3 flagged that `reason_source`
behavior on skipped children was under-specified. Design round 3
commits `reason_source: "skipped"` explicitly (line 2515-2517),
with `reason` echoing `skipped_because_chain[-1]` (the root failure).
That closes the P2/F2 gap from round 2.

**Severity:** POSITIVE FINDING. Round 3 closed a round-2 gap.

**Location:** Design lines 2506-2521 (reason_source enum complete).

**Proposed resolution:** None. Gap closed.

### F6. Round-2's CHECK 7 (pair_a3) partially resolved

**Observation:** Pair A3's CHECK 7 flagged four-value
`reason_source` vocabulary. Round 3 commits all four values
(`failure_reason`, `state_name`, `skipped`, `not_spawned`). Round-2
pair_a3's F3 can be downgraded.

**Severity:** POSITIVE FINDING.

**Location:** Design lines 2510-2519.

**Proposed resolution:** None. Gap closed.

### F7. Composition verdict -- retry-heavy observability holds

**Observation:** The design's retry re-finalization semantics
compose cleanly with CD13's observability primitives:

1. First `BatchFinalized` event anchors `batch.phase: "final"` from
   finalization onward, regardless of parent transitions (verified
   Turns 1, 3, 7).
2. `retry_failed` does NOT clear or invalidate the first
   `BatchFinalized` event; it remains in the log until a new one
   supersedes it (verified Turn 3 and Turn 6's event log).
3. Second `BatchFinalized` event appends on the next finalization
   pass and wins "most recent" resolution (verified Turn 5).
4. Terminal `done` response's `batch_final_view` carries the post-
   retry snapshot (verified Turn 5).
5. Per-level non-recursion prevents nested-batch payload blowup
   (verified Turn 8).

All four `reason_source` enum variants are now committed
(`failure_reason`, `state_name`, `skipped`, `not_spawned`).
`skipped_because` is consistently the direct blocker;
`skipped_because_chain` is closest-first-root-last topological order.
The terminal transition does not lose observability; the retry
cycle does not corrupt observability; and the multi-level non-
recursion rule keeps payload bounded.

**Severity:** POSITIVE FINDING. CD13 with the round-3 refinements
holds across retry-heavy workflows.

**Location:** All of Decision 13.

**Proposed resolution:** None needed for correctness. F1-F4 are
documentation / ergonomic nits inside the correct shape.

---

## Section 4: Summary table

| ID | Gap | Severity | Blocking? |
|----|-----|----------|-----------|
| G1 / F1 | `superseded_by` marker missing on stale `BatchFinalized` events | LOW | No, documentation only |
| G2 / F3 | W4-ignored templates have unreachable `retry_failed` | LOW | No, W4 covers this |
| G3 / F4 | `batch_final_view` presence inferable but not documented | LOW | No, documentation only |
| F2 | Phase stickiness should be visually called out | LOW | No |
| F5 / F6 | Round-2 gaps closed by round-3 design | -- | Positive |
| F7 | Retry-heavy observability composes cleanly | -- | Positive |

None of G1-G3/F1-F4 invalidate round 3. All are scoping / label /
documentation clarifications inside CD13's existing shape. Round 3's
core promise -- `BatchFinalized` retry semantics hold across retry-
heavy workflows including multi-level views -- is verified.

End of simulation.
