# Simulation Round 2, Pair A3: CD13 Observability Composition Across Terminal

Scenario: 5-issue plan with a long tail -- A -> (B, C) -> D -> E.
`failure_policy: skip_dependents`. A succeeds, B fails (child writes
`failure_reason` to context via `done_blocked.default_action`, W5 pattern),
C succeeds, D and E are synthesized as skipped. D's `skipped_because` is
`coord.issue-B`; E's direct skip blocker is `coord.issue-D`, ultimate cause
is `coord.issue-B`. The aggregate gate fires
`all_complete: true, any_failed: true, any_skipped: true,
needs_attention: true` and the parent transitions to `analyze_failures`.
On that transition, the advance loop appends a `BatchFinalized` event per
Decision 13.

Round 2's focus: does CD13's five-part fix compose into a coherent
observability story that survives the `analyze_failures -> summarize`
terminal transition?

Parent template: same `coord` shape as walkthrough.md (three states:
`plan_and_await`, `analyze_failures`, `summarize`). Child template:
`impl-issue.md` with `done_blocked.default_action.context_assignments:
{ failure_reason: "unit tests failed: 3 assertions, see logs" }`.

---

## Section 1: Transcript

Preconditions: on entry to this transcript, the parent has just ticked
after B's failure and D/E have been synthesized as skip markers in the
same tick. The advance loop appended a `BatchFinalized` event on that
tick, `all_complete: true` plus `needs_attention: true` matched the
transition guard, and the parent is now parked on `analyze_failures`.

### Turn 1 -- AGENT: `koto next coord` (parent on `analyze_failures`)

**AGENT thinks:** "The batch just finalized; I expect the parent to be
in a recovery state. I want the full per-child outcome, including which
children were synthesized and the transitive chain for skips."

**KOTO:**
```json
{
  "action": "evidence_required",
  "state": "analyze_failures",
  "directive": "At least one child failed or was skipped. Inspect the batch view in the response's blocking_conditions[0].output.children array or in `koto status coord`. Two recovery paths:\n\n- Retry the failures. Submit the retry_failed reserved action (see reserved_actions in the response for the ready invocation string). The parent re-enters plan_and_await and the scheduler respawns the named children.\n- Give up or acknowledge. Submit {\"decision\": \"give_up\"} or {\"decision\": \"acknowledge\"} to route to summarize with the batch outcome as-is.",
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "decision": {
        "type": "enum",
        "values": ["give_up", "acknowledge"],
        "required": false
      }
    }
  },
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 5,
      "completed": 5,
      "pending": 0,
      "success": 2,
      "failed": 1,
      "skipped": 2,
      "blocked": 0,
      "spawn_failed": 0,
      "all_complete": true,
      "all_success": false,
      "any_failed": true,
      "any_skipped": true,
      "needs_attention": true,
      "children": [
        {"name": "coord.issue-A", "state": "done",                       "complete": true, "outcome": "success"},
        {"name": "coord.issue-B", "state": "done_blocked",               "complete": true, "outcome": "failure", "failure_mode": true, "reason": "unit tests failed: 3 assertions, see logs", "reason_source": "failure_reason"},
        {"name": "coord.issue-C", "state": "done",                       "complete": true, "outcome": "success"},
        {"name": "coord.issue-D", "state": "skipped_due_to_dep_failure", "complete": true, "outcome": "skipped", "synthetic": true, "skipped_because": "coord.issue-B", "skipped_because_chain": ["coord.issue-B"]},
        {"name": "coord.issue-E", "state": "skipped_due_to_dep_failure", "complete": true, "outcome": "skipped", "synthetic": true, "skipped_because": "coord.issue-D", "skipped_because_chain": ["coord.issue-D", "coord.issue-B"]}
      ]
    }
  }],
  "reserved_actions": [
    {
      "name": "retry_failed",
      "description": "Re-queue failed and skipped children. Dependents are included by default.",
      "payload_schema": {
        "children": {"type": "array<string>", "required": true},
        "include_skipped": {"type": "boolean", "required": false, "default": true}
      },
      "applies_to": ["coord.issue-B", "coord.issue-D", "coord.issue-E"],
      "invocation": "koto next coord --with-data '{\"retry_failed\": {\"children\": [\"coord.issue-B\"]}}'"
    }
  ],
  "scheduler": null
}
```

Gloss: CD13 parts in play on this tick.

- `reason_source: "failure_reason"` on B confirms the W5 pattern engaged
  -- B's `default_action.context_assignments` wrote a readable message
  and the batch view pulled it. If B's template had *not* written the
  key, the compile step would have fired W5 and the runtime `reason`
  would read `"done_blocked"` with `reason_source: "state_name"`. The
  contrast is observable from this response alone.
- `synthetic: true` is present on D and E (skip markers, not real work).
- `skipped_because_chain` differs between D and E: D's is length 1
  (`["coord.issue-B"]`) because B is directly upstream and failed; E's
  is length 2 (`["coord.issue-D", "coord.issue-B"]`) because it walks
  D -> B until the first failed (non-skipped) ancestor. The singular
  `skipped_because` still records the *direct* blocker in both cases.
- `reserved_actions[0].applies_to` includes all three retryable
  children (B failed; D, E skipped). Agent need only name B; the
  retry path propagates downward by virtue of `include_skipped: true`.

Note: the response on this tick is `action: evidence_required` (the
`analyze_failures` state accepts a `decision`), *not* `action: done`.
CD13's `batch_final_view` carve-out is scoped to the terminal `done`
response. This tick parks on `analyze_failures` so the batch payload
is carried via `blocking_conditions[0].output` (the live gate
evaluation snapshot). CD13 does not attach `batch_final_view` here --
it would be redundant with the gate output on this state.

`[CHECK 1: This is fine, but note CD13 says "batch_final_view on
terminal and post-batch done responses". Is there any gap if the
agent is on analyze_failures and asks koto status rather than
koto next? See Turn 2.]`

### Turn 2 -- AGENT: `koto status coord`

**AGENT thinks:** "I want a summary without re-evaluating the gate."

**KOTO:**
```json
{
  "workflow": "coord",
  "state": "analyze_failures",
  "is_terminal": false,
  "batch": {
    "phase": "final",
    "summary": {
      "total": 5,
      "success": 2,
      "failed": 1,
      "skipped": 2,
      "pending": 0,
      "blocked": 0,
      "spawn_failed": 0
    },
    "tasks": [
      {"name": "issue-A", "child": "coord.issue-A", "outcome": "success"},
      {"name": "issue-B", "child": "coord.issue-B", "outcome": "failure", "reason": "unit tests failed: 3 assertions, see logs", "reason_source": "failure_reason"},
      {"name": "issue-C", "child": "coord.issue-C", "outcome": "success"},
      {"name": "issue-D", "child": "coord.issue-D", "outcome": "skipped", "synthetic": true, "skipped_because": "coord.issue-B", "skipped_because_chain": ["coord.issue-B"]},
      {"name": "issue-E", "child": "coord.issue-E", "outcome": "skipped", "synthetic": true, "skipped_because": "coord.issue-D", "skipped_because_chain": ["coord.issue-D", "coord.issue-B"]}
    ],
    "ready": [],
    "blocked": [],
    "skipped": ["issue-D", "issue-E"],
    "failed": ["issue-B"]
  }
}
```

Gloss: `batch.phase: "final"`. The parent is parked on
`analyze_failures`, which has *no* `materialize_children` hook -- the
phase-1 (Decision 6) predicate "current state has the hook" is FALSE
here. But CD13's `BatchFinalized` event fires from the log and
`derive_batch_view` replays it, so the `batch` section is present
with `phase: "final"` rather than `phase: "current"`. This is the
exact gap CD13 was written for.

`[CHECK 2 -- OPEN QUESTION: Decision 13 documents the "parent's
current state has no hook" branch ("regardless of the parent's
current state"), but the text is silent about phase labeling when
the parent is *still* on a batched state and the gate has already
finalized. If the advance loop appended `BatchFinalized` on a
transition leaving `plan_and_await`, `koto status` on the parent
*before* the next tick would see the parent on `plan_and_await` but
with a `BatchFinalized` in the log. Is that `phase: "current"` (from
the live gate re-evaluation) or `phase: "final"` (replayed from the
event)? The live gate would produce identical content, but phase
label discipline needs to be unambiguous. Call this P1.]`

### Turn 3 -- AGENT: `koto status coord.issue-D`

**AGENT thinks:** "D is a synthetic child. Does its status response
advertise that, and does it show the skip chain?"

**KOTO:**
```json
{
  "workflow": "coord.issue-D",
  "state": "skipped_due_to_dep_failure",
  "is_terminal": true,
  "synthetic": true,
  "skipped_because": "coord.issue-B",
  "skipped_because_chain": ["coord.issue-B"]
}
```

Gloss: CD13 projects `synthetic: true` into `koto status <child>`
for children whose current state has `skipped_marker: true`. Both
`skipped_because` and `skipped_because_chain` are surfaced. For D the
chain is length 1 -- B is a direct waits_on upstream and B is failed
(non-skipped), so the walk stops at B.

`[CHECK 3: Design walkthrough line 976 also shows `reason_source:
"state_name"` on the status response for a synthetic child, but
Decision 13's text says `reason_source` is "omitted for successful
or not-yet-terminal children" -- and skipped children are neither
successful nor non-terminal, they are skipped. Is `reason_source`
emitted on skipped-child status, and if so, what value? The
walkthrough shows "state_name" but skipped children never run their
terminal-failure reason-writing path because they never reach
`done_blocked`; the state_name here would be
`skipped_due_to_dep_failure` which is not a failure state at all.
Projecting `reason_source` onto skip markers conflates
"failure reason source" with "skip reason source". Call this P2.]`

### Turn 4 -- AGENT: `koto status coord.issue-E`

**AGENT thinks:** "E is skipped because D is skipped because B
failed. I want to see the full transitive chain on E, not just D."

**KOTO:**
```json
{
  "workflow": "coord.issue-E",
  "state": "skipped_due_to_dep_failure",
  "is_terminal": true,
  "synthetic": true,
  "skipped_because": "coord.issue-D",
  "skipped_because_chain": ["coord.issue-D", "coord.issue-B"]
}
```

Gloss: Per CD13, the chain walks upstream through `waits_on` until it
hits the first failed (non-skipped) ancestor. For E:
E.waits_on = [D], D.state = skipped -> keep walking; D.waits_on = [B],
B.state = failed -> stop. Chain = [D, B]. CD13's "shortest chain,
alphabetical tie-break" is trivially satisfied here because it is a
linear tail (no diamond upstream of E).

`[CHECK 4: The design example for long-tail chains is A -> (B, C) -> D
-> E, not A -> B -> D -> E. E's waits_on is [D] (by plan
specification), and D's waits_on is [B]. A is not on E's chain at
all because A is successful. The chain stops at the first *failed*
ancestor per Decision 13; it does not continue up to A. Verified:
CD13 says "walking upstream through waits_on to the first failed
(non-skipped) ancestor" -- A is not failed, so E's chain cannot
include A. Good -- the design is unambiguous here.]`

`[CHECK 5 -- OPEN QUESTION: What if a skipped child has *multiple*
failed ancestors via different waits_on branches? Diamond case:
X waits_on [Y, Z]; Y fails, Z fails; X is skipped. Decision 13 says
"shortest chain, tie-breaking alphabetically". For X:
skipped_because could be Y or Z; skipped_because_chain likewise.
Both paths have length 1, so alphabetical wins: skipped_because = Y,
chain = [Y]. What about: X waits_on [Y, Z]; Y skipped (because W
failed), Z failed. X has a length-1 chain through Z and a length-2
chain through Y. Shortest wins: chain = [Z]. But does
skipped_because = Z (direct blocker on the shortest chain) or Y
(alphabetically first direct blocker)? Decision 13 is silent on
whether "shortest chain" tie-breaks singular `skipped_because` too,
or if singular always picks any-alphabetical-first direct blocker
independent of chain shortness. Call this P3.]`

### Turn 5 -- AGENT: `koto next coord.issue-D`

**AGENT thinks:** "I've mistakenly started driving D. What happens?"

**KOTO:**
```json
{
  "action": "done",
  "state": "skipped_due_to_dep_failure",
  "directive": "Issue #D was skipped because a dependency failed. No action required -- the scheduler materialized this child directly into its terminal skip state.",
  "is_terminal": true,
  "synthetic": true,
  "skipped_because": "coord.issue-B",
  "skipped_because_chain": ["coord.issue-B"]
}
```

Gloss: CD13 requires `koto next <synthetic-child>` to return an
immediate `action: done` with a non-empty directive. The response is
shape-identical to any other terminal workflow's idempotent re-tick,
plus `synthetic: true` and the skip attribution fields. The
`directive` string is sourced from the synthetic mini-template's
`skipped_due_to_dep_failure` markdown body (walkthrough.md line
185-189), not a CD13-generated default. That means the interpolated
`{{skipped_because}}` (if present in the synthetic template) would
need the context key to be populated at synthesis time -- which it
is, per Decision 5.2.

`[CHECK 6 -- OPEN QUESTION: Walkthrough.md's synthetic child markdown
body is:
  "Issue #{{ISSUE_NUMBER}} was skipped because a dependency failed.
   No action required -- the scheduler materialized this child
   directly into its terminal skip state."
It does NOT interpolate `{{skipped_because}}`. An agent reading the
directive on D sees "Issue #102 was skipped" but not "because of
issue-B". CD13's proposed directive (Decision 13 body text) was:
"This task was skipped because dependency '<skipped_because>' did not
succeed. No action required." Design Decision 13's EXAMPLE shows the
richer directive; walkthrough.md's actual template uses the
leaner prose. The agent recovers the blocker from the parallel
`skipped_because` *field*, not from the directive prose. That is
fine mechanically, but the two surfaces disagree about whether the
directive should name the blocker inline. Call this P4.]`

### Turn 6 -- AGENT: `koto workflows --children coord`

**AGENT thinks:** "I want a one-shot grid of all children with their
outcomes and metadata, including `reason_source` enum on each row."

**KOTO:**
```json
{
  "children": [
    {
      "name": "coord.issue-A",
      "state": "done",
      "is_terminal": true,
      "synthetic": false,
      "task_name": "issue-A",
      "waits_on": [],
      "reason_code": "success",
      "reason": null,
      "reason_source": null,
      "skip_reason": null,
      "skipped_because_chain": null
    },
    {
      "name": "coord.issue-B",
      "state": "done_blocked",
      "is_terminal": true,
      "synthetic": false,
      "task_name": "issue-B",
      "waits_on": ["issue-A"],
      "reason_code": "failure",
      "reason": "unit tests failed: 3 assertions, see logs",
      "reason_source": "failure_reason",
      "skip_reason": null,
      "skipped_because_chain": null
    },
    {
      "name": "coord.issue-C",
      "state": "done",
      "is_terminal": true,
      "synthetic": false,
      "task_name": "issue-C",
      "waits_on": ["issue-A"],
      "reason_code": "success",
      "reason": null,
      "reason_source": null,
      "skip_reason": null,
      "skipped_because_chain": null
    },
    {
      "name": "coord.issue-D",
      "state": "skipped_due_to_dep_failure",
      "is_terminal": true,
      "synthetic": true,
      "task_name": "issue-D",
      "waits_on": ["issue-B"],
      "reason_code": "skipped",
      "reason": null,
      "reason_source": null,
      "skip_reason": "coord.issue-B",
      "skipped_because_chain": ["coord.issue-B"]
    },
    {
      "name": "coord.issue-E",
      "state": "skipped_due_to_dep_failure",
      "is_terminal": true,
      "synthetic": true,
      "task_name": "issue-E",
      "waits_on": ["issue-D"],
      "reason_code": "skipped",
      "reason": null,
      "reason_source": null,
      "skip_reason": "coord.issue-D",
      "skipped_because_chain": ["coord.issue-D", "coord.issue-B"]
    }
  ]
}
```

Gloss: CD13 adds `synthetic`, `skipped_because_chain`, and
`reason_source` to per-row metadata. The user's prompt expected
`reason_source` to be an enum with four values
(`failure_reason, state_name, skipped, not_spawned`). CD13 only
commits `failure_reason | state_name`, with the field *omitted* for
"successful or not-yet-terminal children". That leaves three row
shapes ambiguous:

- Skipped rows: `reason_source: null` above, but CD13 does not say
  either way (the design says "omitted for successful or not-yet-
  terminal"; skipped is neither). Treating skipped as "omitted"
  (null) is the least-surprising interpretation.
- Not-spawned (un-materialized) rows: the row itself is synthesized
  by `--children` even though no state file exists (CD11 + CD13
  Decision 6). The user's prompt mentions a `not_spawned` enum
  variant. CD13 doesn't commit one.
- Pending/running rows: CD13 says `reason_source` is omitted for
  "not-yet-terminal". So `null` or absent.

`[CHECK 7 -- GAP: `reason_source` enum vocabulary is under-specified
in CD13. The design commits two values and says "omitted otherwise".
The user's prompt (the round-2 brief) assumes four values, including
`skipped` and `not_spawned`, which don't exist in CD13. Agents
relying on the `reason_source` field to disambiguate skipped vs
not-spawned rows have to fall back to `reason_code` / `outcome` and
`synthetic`. Call this P5.]`

### Turn 7 -- AGENT: `koto query coord`

**AGENT thinks:** "Is the BatchFinalized event visible in the raw
event log?"

**KOTO (abbreviated):**
```json
{
  "workflow": "coord",
  "state": "analyze_failures",
  "is_terminal": false,
  "events": [
    {"type": "WorkflowInitialized", "ts": "..."},
    {"type": "Transitioned", "to": "plan_and_await"},
    {"type": "EvidenceSubmitted", "fields": {"tasks": [...], "submitter_cwd": "..."}},
    {"type": "SchedulerRan", "spawned": ["coord.issue-A"], "ts": "..."},
    {"type": "SchedulerRan", "spawned": ["coord.issue-B", "coord.issue-C"], "ts": "..."},
    {"type": "SchedulerRan", "spawned_as_skipped": ["coord.issue-D", "coord.issue-E"], "ts": "..."},
    {"type": "BatchFinalized", "view": {
        "phase": "final",
        "summary": {"total": 5, "success": 2, "failed": 1, "skipped": 2, "pending": 0, "blocked": 0, "spawn_failed": 0},
        "tasks": [
          {"name": "issue-A", "child": "coord.issue-A", "outcome": "success"},
          {"name": "issue-B", "child": "coord.issue-B", "outcome": "failure", "reason": "unit tests failed: 3 assertions, see logs", "reason_source": "failure_reason"},
          {"name": "issue-C", "child": "coord.issue-C", "outcome": "success"},
          {"name": "issue-D", "child": "coord.issue-D", "outcome": "skipped", "synthetic": true, "skipped_because": "coord.issue-B", "skipped_because_chain": ["coord.issue-B"]},
          {"name": "issue-E", "child": "coord.issue-E", "outcome": "skipped", "synthetic": true, "skipped_because": "coord.issue-D", "skipped_because_chain": ["coord.issue-D", "coord.issue-B"]}
        ],
        "ready": [], "blocked": [], "skipped": ["issue-D", "issue-E"], "failed": ["issue-B"]
    }},
    {"type": "Transitioned", "to": "analyze_failures"}
  ],
  "context": {"plan_path": "PLAN.md"}
}
```

Gloss: `BatchFinalized` is a first-class event in the log,
appended *immediately before* the transition out of the batched
state (per CD13 text: "when `children-complete` first evaluates
`all_complete: true` on a state with `materialize_children`"). The
event carries the full BatchView snapshot, so `derive_batch_view`'s
replay is cheap.

`[CHECK 8 -- MINOR: The walkthrough.md text (line 733) says "On the
first all_complete: true pass, the advance loop appends a
BatchFinalized event" -- appended during the advance loop at the
still-in-plan_and_await tick, before the transition fires. The event
ordering above places BatchFinalized *before* Transitioned ->
analyze_failures. This is what CD13 implies. Good.]`

### Turn 8 -- AGENT submits acknowledgement, parent transitions to `summarize`

**AGENT:** `koto next coord --with-data '{"decision": "acknowledge"}'`

**AGENT thinks:** "I'm going to acknowledge and write the summary.
Does the terminal `done` response still carry `batch_final_view`?"

**KOTO:**
```json
{
  "action": "done",
  "state": "summarize",
  "directive": "Write a summary covering which issues succeeded, which failed, and why. The batch_final_view field in this response carries the full snapshot so you don't need a second command.",
  "is_terminal": true,
  "batch_final_view": {
    "phase": "final",
    "summary": {
      "total": 5,
      "success": 2,
      "failed": 1,
      "skipped": 2,
      "pending": 0,
      "blocked": 0,
      "spawn_failed": 0
    },
    "tasks": [
      {"name": "issue-A", "child": "coord.issue-A", "outcome": "success"},
      {"name": "issue-B", "child": "coord.issue-B", "outcome": "failure", "reason": "unit tests failed: 3 assertions, see logs", "reason_source": "failure_reason"},
      {"name": "issue-C", "child": "coord.issue-C", "outcome": "success"},
      {"name": "issue-D", "child": "coord.issue-D", "outcome": "skipped", "synthetic": true, "skipped_because": "coord.issue-B", "skipped_because_chain": ["coord.issue-B"]},
      {"name": "issue-E", "child": "coord.issue-E", "outcome": "skipped", "synthetic": true, "skipped_because": "coord.issue-D", "skipped_because_chain": ["coord.issue-D", "coord.issue-B"]}
    ],
    "ready": [],
    "blocked": [],
    "skipped": ["issue-D", "issue-E"],
    "failed": ["issue-B"]
  }
}
```

Gloss: Yes -- the `done` response STILL carries `batch_final_view`.
Per CD13: "The done response shape gains an optional
`batch_final_view` field, present when the parent log contains at
least one BatchFinalized event." The parent's log has one (appended
at Turn 1 / before the analyze_failures transition), and no new
BatchFinalized is appended on this tick (the `summarize` state has
no `materialize_children` hook). The response replays the existing
event. The phase label stays "final" -- CD13 doesn't specify a phase
distinction between "final from the last batch tick" and "final from
terminal tick N ticks later", because there shouldn't be one: the
snapshot is whatever was recorded at finalization time.

`[CHECK 9 -- OBSERVATION, NOT GAP: `batch_final_view` is identical
to what Turn 2's `koto status coord` returned in its `batch` field
(both pull from the same BatchFinalized event). The agent writing
the summary directive does NOT need a second command. CD13's
"eliminates the two-call pattern" promise holds.]`

### Turn 9 -- AGENT: `koto status coord` after terminal

**AGENT thinks:** "Parent is now on summarize (terminal). Is the
batch section still on koto status?"

**KOTO:**
```json
{
  "workflow": "coord",
  "state": "summarize",
  "is_terminal": true,
  "batch": {
    "phase": "final",
    "summary": {"total": 5, "success": 2, "failed": 1, "skipped": 2, "pending": 0, "blocked": 0, "spawn_failed": 0},
    "tasks": [
      {"name": "issue-A", "child": "coord.issue-A", "outcome": "success"},
      {"name": "issue-B", "child": "coord.issue-B", "outcome": "failure", "reason": "unit tests failed: 3 assertions, see logs", "reason_source": "failure_reason"},
      {"name": "issue-C", "child": "coord.issue-C", "outcome": "success"},
      {"name": "issue-D", "child": "coord.issue-D", "outcome": "skipped", "synthetic": true, "skipped_because": "coord.issue-B", "skipped_because_chain": ["coord.issue-B"]},
      {"name": "issue-E", "child": "coord.issue-E", "outcome": "skipped", "synthetic": true, "skipped_because": "coord.issue-D", "skipped_because_chain": ["coord.issue-D", "coord.issue-B"]}
    ],
    "ready": [],
    "blocked": [],
    "skipped": ["issue-D", "issue-E"],
    "failed": ["issue-B"]
  }
}
```

Gloss: Yes -- `koto status` on a terminal parent with a
`BatchFinalized` event in its log emits the batch section with
`phase: "final"`. The data flow: `derive_batch_view` looks for the
most recent `BatchFinalized` event, finds one, replays its payload.
No live gate re-evaluation (the parent is terminal; there's nothing
to gate). The data source is the event log, not the current children
on disk. If a user manually `rm`-ed a child session between
`BatchFinalized` and this `koto status` call, `batch_final_view`
would still show it as terminal per the frozen snapshot.

`[CHECK 10 -- EDGE CASE: If a child's state file drifts (manual
deletion, cloud sync divergence) after BatchFinalized, the
batch_final_view is frozen and does not reflect the drift. The
`phase: "final"` label signals "this is what it looked like at
finalization" without claiming anything about current on-disk
state. That's the right semantics -- it's a snapshot, not a live
view -- but it's worth documenting that consumers cannot assume
batch_final_view reflects current reality. Call this P6.]`

### Turn 10 -- Counterfactual: all-success case

**AGENT (hypothetical):** "If this batch had ALL succeeded, does
koto next on the terminal summarize state still carry
`batch_final_view`?"

**KOTO:**
```json
{
  "action": "done",
  "state": "summarize",
  "directive": "Write a summary...",
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

Gloss: Yes -- CD13 says `batch_final_view` is present "when the
parent log contains at least one BatchFinalized event". It does NOT
gate on "anything interesting happened". An all-success batch fires
BatchFinalized the moment all_complete first evaluates true, and the
terminal response carries the snapshot. This is consistent with
walkthrough.md interaction 8 (line 664-693), which shows
`batch_final_view` with 3-for-3 success.

`[CHECK 11 -- OBSERVATION: The existence of BatchFinalized is keyed
to "first all_complete: true", not to "something needs attention".
Agents should not assume `batch_final_view`-present implies
failures. Omitting it on happy-path batches would actually be worse
-- consumers would need a second command to confirm "nothing
happened worth reporting". The current design is correct. No gap.]`

### Turn 11 -- Contrast case: what if B did NOT write failure_reason?

**AGENT thinks (counterfactual):** "If B's template lacked the
`default_action.context_assignments` (W5 would have fired at
compile), what would the observer see?"

**KOTO (hypothetical):**
```json
{
  "name": "coord.issue-B",
  "state": "done_blocked",
  "complete": true,
  "outcome": "failure",
  "failure_mode": true,
  "reason": "done_blocked",
  "reason_source": "state_name"
}
```

Gloss: The `reason` would fall back to the state name
(`"done_blocked"`), and `reason_source: "state_name"` would flag
the fallback. Compare Turn 1's row for B, which has
`reason: "unit tests failed: 3 assertions, see logs"` and
`reason_source: "failure_reason"`. The contrast is visible from the
row alone. CD13's `reason_source` is what makes this contrast
machine-detectable; without it, observers would see a `reason`
string and have no idea whether it was an author-written sanitized
message or the opaque state-name fallback.

This is the key value-add of CD13's `reason_source`: it lets the
agent (or a downstream tool like shirabe) render a
"template-author-wrote-nothing" warning UI distinct from the
"template-author-wrote-this" case. In our scenario, W5 fired at
compile so the template *has* the writer; at runtime we see
`failure_reason`. Authors who ignored W5 would see `state_name` at
runtime, which makes the failure re-discoverable by the agent.

`[CHECK 12 -- OBSERVATION: W5 + reason_source compose cleanly. W5
is a compile-time prompt; reason_source is a runtime signal.
Authors who ignore W5 still produce non-silent output (state_name)
rather than broken output (null or crashed). Good defence-in-depth.]`

---

## Section 2: Findings

### F1. `batch.phase` labeling when parent is still on the batched state but has finalized

**Observation:** CD13 distinguishes `phase: "current"` (live gate
re-evaluation from Decision 6's `materialize_children` predicate)
and `phase: "final"` (replay from the most recent
`BatchFinalized` event). But if the parent is on the batched state
AND the log contains a `BatchFinalized` event (narrow window between
the advance loop appending the event and transitioning out), both
predicates match. Is the response `current` or `final`?

**Location:** Decision 13, design doc lines 2274-2295;
walkthrough.md lines 722-737.

**Severity:** LOW. Content of the two views is identical at that
instant; label discipline matters only for downstream tools that
branch on `phase`.

**Proposed resolution:** Clarify in Decision 13: "When both
predicates match (advance loop just finalized but has not yet
transitioned), emit `phase: "final"` to reflect that the batch
has closed. The transition to a non-batched state follows in the
same response's event chain." Alternatively: "phase label depends
solely on whether a `BatchFinalized` event exists in the log;
`current` only applies if `BatchFinalized` is absent and the
current state has `materialize_children`."

### F2. `reason_source` on skipped children is under-specified

**Observation:** CD13 says `reason_source` is emitted when `reason`
engages `failure_reason` (value `"failure_reason"`) or falls back to
the state name (value `"state_name"`), and is "omitted for
successful or not-yet-terminal children". Skipped children are
neither successful nor not-yet-terminal -- they are terminal with
outcome `skipped`. The walkthrough.md example on line 976 shows
`reason_source: "state_name"` on a skip-marker child's `koto
status` response, which conflates "failure reason source" with
"skip reason source" -- the skip-marker's state name
(`skipped_due_to_dep_failure`) is not a failure state.

**Location:** Decision 13, lines 2345-2349 (design doc);
walkthrough.md line 976.

**Severity:** MEDIUM. Directly affects downstream consumer
(shirabe) that may branch on `reason_source` to render different UI
for failure vs skip vs success.

**Proposed resolution:** Three options, pick one:
(a) `reason_source` is omitted on skipped children (our Turn 3 /
Turn 6 rendering).
(b) `reason_source: "skipped"` introduced as a third enum value.
(c) `reason_source` is explicitly only about terminal failure
states; a separate `skip_reason_source` field handles skips.
The user's prompt assumed (b). The walkthrough.md example implies
conflation. The design text implies (a) by omission. Pick one and
write it into Decision 13.

### F3. `reason_source` enum vocabulary is incomplete for `--children` rows

**Observation:** The user's prompt mentions a four-value enum:
`failure_reason, state_name, skipped, not_spawned`. CD13 commits
only `failure_reason` and `state_name`. Per-row metadata in
`koto workflows --children` can include rows for children that were
never materialized (ready tasks that haven't spawned yet, or
un-spawned waiting tasks). These rows have no terminal state, no
failure_reason, and no state_name that applies. If `reason_source`
is projected onto all rows, there are row shapes CD13 does not
cover.

**Location:** Decision 13, lines 2345-2349; walkthrough.md
doesn't document a `--children` view, but Decision 6 line 1321-1323
does. CD13 doesn't extend reason_source vocabulary for `--children`.

**Severity:** LOW-MEDIUM. Observers can infer by combining
`reason_code` (aka `outcome`) with `synthetic`, so downstream
tooling has the data. Just not via `reason_source` directly.

**Proposed resolution:** Either extend the enum (preferable; picks
up the user's prompt's `not_spawned` and `skipped` values) or
explicitly scope `reason_source` to terminal-failure rows and state
that consumers wanting skip/not-spawned disambiguation use
`reason_code` + `synthetic`.

### F4. Tie-breaking between singular `skipped_because` and plural `skipped_because_chain`

**Observation:** CD13 says "Diamonds pick the shortest chain,
alphabetical tie-break for determinism" -- but this only specifies
how to choose the CHAIN. The singular `skipped_because` is defined
as "the direct upstream blocker". When a skip-marker child has
multiple `waits_on` parents in different skip states, which one
does `skipped_because` point at? The direct blocker on the shortest
chain? Any alphabetical direct blocker? Is it always consistent
with `skipped_because_chain[0]`?

**Location:** Decision 13, lines 2322-2335.

**Severity:** LOW. A3 scenario doesn't hit this (linear tail); but
Pair 1c's diamond originals would, and shirabe may encounter it.

**Proposed resolution:** Commit: "`skipped_because` is always
`skipped_because_chain[0]`. The tie-break on chain choice
(shortest, then alphabetical by first entry) determines both."

### F5. Synthetic child directive prose vs CD13 example

**Observation:** walkthrough.md's synthetic mini-template body
(line 185-189) uses the interpolated text:

> "Issue #{{ISSUE_NUMBER}} was skipped because a dependency
> failed. No action required -- the scheduler materialized this
> child directly into its terminal skip state."

CD13's example directive (design doc line 2309) is:

> "This task was skipped because dependency '<skipped_because>' did
> not succeed. No action required."

The two disagree about whether the directive inlines the blocker
name. walkthrough.md's version requires the agent to read the
parallel `skipped_because` field; CD13's version bakes the name
into the directive.

**Location:** Decision 13, line 2309; walkthrough.md, line 185-189.

**Severity:** LOW. The data is reachable either way. But template-
authoring guidance (koto-author skill) should pick one.

**Proposed resolution:** Pick the richer form (CD13 text). Update
walkthrough.md's example to interpolate `{{skipped_because}}` so
agents who only read the directive (and not the structured fields)
still get the answer.

### F6. Frozen snapshot semantics of `batch_final_view` under drift

**Observation:** `batch_final_view` (and `batch.phase: "final"` in
`koto status`) replays the most recent `BatchFinalized` event. If a
child's state file is deleted, modified, or cloud-syncs to a
different outcome after BatchFinalized appended, the snapshot does
NOT reflect the drift. This is the right semantics -- snapshot, not
live view -- but consumers relying on `batch_final_view` alone may
miss drift.

**Location:** Decision 13, lines 2276-2295.

**Severity:** LOW. Legitimate drift (cloud sync, manual deletion)
is rare; and the `retry_failed` path appends a *new*
BatchFinalized on the next pass, superseding the stale one.

**Proposed resolution:** Document in Decision 13: "`batch_final_view`
is a snapshot from the moment `all_complete` first evaluated true
on the batched state. Consumers wanting live per-child state should
combine `batch_final_view` with per-child `koto status` calls, or
rely on `retry_failed` appending a fresh BatchFinalized."

### F7. Composition verdict -- CD13's five parts compose cleanly

**Observation:** The five CD13 parts interlock as intended:

1. `BatchFinalized` event: survives the `analyze_failures ->
   summarize` terminal transition. (Turn 7, Turn 9 verify.)
2. `batch_final_view` on `done` responses: populated from the
   BatchFinalized event. Agent gets the full snapshot on the
   terminal tick without a second command. (Turn 8 verifies.)
3. `synthetic: true` marker: projected on `koto status`, `koto
   next`, `koto workflows --children`, and batch view rows. Agent
   can distinguish skip markers from real work without knowing
   reserved state names. (Turns 3, 5, 6 verify.)
4. `skipped_because_chain`: walks upstream to first failed
   ancestor, correctly handles linear tails (E -> D -> B). (Turn 4
   verifies.)
5. W5 + `reason_source`: compile-time + runtime defence-in-depth.
   Authors who ignore W5 still produce machine-detectable fallback
   (`reason_source: "state_name"`). (Turn 1, Turn 11 verify.)

The terminal transition (`analyze_failures -> summarize`) does not
lose observability. Parent status after terminal still emits the
batch section (Turn 9). `koto next` on a synthetic child terminates
cleanly with an informative directive (Turn 5).

**Location:** Decision 13, all.

**Severity:** This is the positive finding. CD13 solves the
round-1 Cluster F gaps.

**Proposed resolution:** None needed for the design. The remaining
findings (F1-F6) are *additional* edge cases within CD13's scope
that should be clarified but do not invalidate CD13.

---

## Section 3: Summary and open questions for CD13 clarification

| ID | Gap | Severity | Blocking? |
|----|-----|----------|-----------|
| P1 / F1 | `batch.phase` labeling in the finalized-but-not-transitioned window | LOW | No |
| P2 / F2 | `reason_source` behavior on skipped children | MEDIUM | Should clarify before shipping |
| P3 / F4 | Tie-break consistency between singular `skipped_because` and `skipped_because_chain` | LOW | No |
| P4 / F5 | Synthetic child directive prose (walkthrough.md vs CD13 example) | LOW | No, documentation only |
| P5 / F3 | `reason_source` enum vocabulary for `--children` rows | LOW-MEDIUM | No |
| P6 / F6 | Snapshot-vs-live semantics of `batch_final_view` under drift | LOW | No, documentation only |

None of the P1-P6 gaps invalidate CD13. All are scoping / label /
vocabulary clarifications inside CD13's existing shape. The core
round-1 Cluster F problem (observability breaks across terminal
transitions) is resolved.

End of simulation.
