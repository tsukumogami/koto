# Simulation Round 1, Pair 1c: Diamond DAG with Observability Emphasis

Scenario: 5-issue plan A -> (B, C) -> D -> E. `failure_policy: skip_dependents`.
A succeeds, B fails (no `failure_reason` written), C succeeds, D and E are
synthesized as skipped.

Parent template: `coord` with single state `plan_and_await` (identical shape
to the walkthrough), plus `summarize` as terminal. Child template:
`impl-issue` whose `done_blocked` terminal-failure state **does not write
`failure_reason`** before entering. The parent submitted tasks for
issue-A/B/C/D/E, all using default `impl-issue.md`.

---

## Section 1: Transcript

Preconditions: all five children have terminated (A, B, C) or been synthesized
(D, E). AGENT is now re-ticking `coord` for the first time after B's failure.

### Turn 1 -- parent re-tick, observe batch completion

**AGENT:** `koto next coord`

**KOTO:**
```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "Read the plan document at PLAN.md. For each issue outline...\n\nAfter submission, children will be spawned automatically. Drive each child in `scheduler.spawned` via `koto next <child-name>`. You can run independent children in parallel. After any child completes, re-check the parent with `koto next coord` to spawn newly-unblocked tasks.",
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
      "all_complete": true,
      "children": [
        {"name": "coord.issue-A", "state": "done",                       "complete": true, "outcome": "success"},
        {"name": "coord.issue-B", "state": "done_blocked",               "complete": true, "outcome": "failure", "failure_mode": true},
        {"name": "coord.issue-C", "state": "done",                       "complete": true, "outcome": "success"},
        {"name": "coord.issue-D", "state": "skipped_due_to_dep_failure", "complete": true, "outcome": "skipped", "skipped_because": "coord.issue-B"},
        {"name": "coord.issue-E", "state": "skipped_due_to_dep_failure", "complete": true, "outcome": "skipped", "skipped_because": "coord.issue-D"}
      ]
    }
  }],
  "scheduler": {
    "spawned": [],
    "already": ["coord.issue-A", "coord.issue-B", "coord.issue-C", "coord.issue-D", "coord.issue-E"],
    "blocked": [],
    "skipped": []
  }
}
```

Gloss: `all_complete: true` because `pending == 0 AND blocked == 0` even with
a failure. The transition `when: gates.done.all_complete: true` should now
fire. But the advance loop also appended the gate evaluation BEFORE transition
routing; since the parent template only has one transition and it matches,
one would expect the response to be `done` with state=`summarize`. The fact
that KOTO returns `gate_blocked` here means the gate is being evaluated
*before* the transition is attempted at this tick, OR the gate is being
treated as "temporal" and blocking even when `all_complete: true`.

`[GAP 1: Is `gate_blocked` vs `done` routing well-defined when a
children-complete gate is `all_complete: true` but failed > 0? The gate
returns `Failed` in Decision 5 wording, which seems to say "the gate is
blocked, not passing", yet `all_complete: true` should satisfy the
`when` clause.]`

Re-reading Decision 5.3: "`all_complete` tightens to `pending == 0 AND
blocked == 0`". There is no statement that `Failed` children make the gate
*block*. The transition `when` reads `gates.done.all_complete: true`. So the
expected response is in fact `action: done, state: summarize`. Retconning:

**KOTO (corrected):**
```json
{
  "action": "done",
  "state": "summarize",
  "directive": "All issues are complete. Write a summary of what was implemented.",
  "is_terminal": true,
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 5, "completed": 5, "pending": 0,
      "success": 2, "failed": 1, "skipped": 2, "blocked": 0,
      "all_complete": true,
      "children": [
        {"name": "coord.issue-A", "state": "done",                       "complete": true, "outcome": "success"},
        {"name": "coord.issue-B", "state": "done_blocked",               "complete": true, "outcome": "failure", "failure_mode": true},
        {"name": "coord.issue-C", "state": "done",                       "complete": true, "outcome": "success"},
        {"name": "coord.issue-D", "state": "skipped_due_to_dep_failure", "complete": true, "outcome": "skipped", "skipped_because": "coord.issue-B"},
        {"name": "coord.issue-E", "state": "skipped_due_to_dep_failure", "complete": true, "outcome": "skipped", "skipped_because": "coord.issue-D"}
      ]
    }
  }],
  "scheduler": null
}
```

Gloss: the parent advanced all the way to `summarize` because the template
didn't route failure to any other state. The agent has *already past*
`plan_and_await`, and the directive on `summarize` is generic ("write a
summary"). It says nothing about the failure. The agent has to notice
`failed: 1, skipped: 2` in `blocking_conditions[0].output` to realize not
everything worked.

`[GAP 2: Once the parent has advanced to a terminal summarize state, the
batch view is still attached via blocking_conditions[0] from the most
recent gate evaluation. Is this actually present on a terminal-done
response? The design example for interaction 8 in walkthrough.md shows
only action/state/directive/is_terminal with no blocking_conditions. If
it's dropped, observability evaporates the moment the batch completes.]`

### Turn 2 -- status on the parent

**AGENT:** `koto status coord`

**KOTO:**
```json
{
  "workflow": "coord",
  "state": "summarize",
  "is_terminal": true,
  "batch": null
}
```

Gloss: Per Decision 6, the `batch` section is emitted "when the current state
has a `materialize_children` hook". `summarize` has no hook, so `batch` is
`null`. The agent cannot see the batch summary from `koto status` anymore.

`[GAP 3: After the parent leaves plan_and_await, the batch view disappears
from koto status. Decision 6 ties `batch` section emission to the current
state's `materialize_children` hook, but the batch's *historical* outcome
is still relevant for the agent writing the summarize directive. The
shirabe "one koto status per poll" claim breaks down once the parent is
on its summarize state.]`

### Turn 3 -- status on synthetic skipped child D

**AGENT:** `koto status coord.issue-D`

**KOTO:**
```json
{
  "workflow": "coord.issue-D",
  "state": "skipped_due_to_dep_failure",
  "is_terminal": true,
  "batch": null
}
```

Gloss: The response confirms the workflow is terminal and in the synthetic
skipped-marker state. But:

- There's no field on `koto status` that says "this is a synthetic child,
  not real work". An observer has to know that `skipped_due_to_dep_failure`
  is a reserved state name from the synthetic mini-template.
- `skipped_because: coord.issue-B` is not exposed via `koto status`; it
  lives in the context (per Decision 5.2, "context key, not a new event
  type"). The design example in Decision 6 shows `skipped_because` in the
  parent's batch view, but not on the child's own `status`.
- `skipped_marker: true` is a template field; there's no indication whether
  it's projected into the `koto status` response.

`[GAP 4: koto status on a synthetic skipped child is indistinguishable in
shape from any other terminal workflow. The caller can't tell "synthetic
skip" from "real success" without reading the state name or calling koto
query. Proposal: add `skipped_marker: true` and `skipped_because:
<name>` to the koto status response for skipped children.]`

### Turn 4 -- AGENT tries to drive the synthetic child

**AGENT:** `koto next coord.issue-D`

**KOTO:**
```json
{
  "action": "done",
  "state": "skipped_due_to_dep_failure",
  "directive": "",
  "is_terminal": true
}
```

Gloss: The synthetic mini-template was hardcoded in `src/cli/batch.rs`
(Decision 5.2) with "exactly two states: an initial state and a
`skipped_due_to_dep_failure` terminal state". The design does NOT specify
what directive prose this synthetic terminal state has. Most likely it's
empty. An agent that doesn't understand it's on a synthetic child will
get a terminal response with a blank directive and no context.

`[GAP 5: The synthetic skipped mini-template has no authored directive. An
agent calling koto next on a synthetic child gets a silent terminal
response. Proposal: the synthetic template should ship with a standard
directive like "This task was skipped because dependency {{skipped_because}}
did not succeed. No action required." The context key can be interpolated
because it was written at synthesis time.]`

`[GAP 6: Decision 5.2 calls the non-terminal state an "initial state" but
gives it no markdown body. If koto next ever lands on this initial state
(e.g., from a future `retry_failed` that rewinds a skipped child), what
directive is returned?]`

### Turn 5 -- workflows --children coord

**AGENT:** `koto workflows --children coord`

**KOTO:**
```json
{
  "children": [
    {
      "name": "coord.issue-A",
      "state": "done",
      "is_terminal": true,
      "task_name": "issue-A",
      "waits_on": [],
      "reason_code": "success",
      "reason": null,
      "skip_reason": null
    },
    {
      "name": "coord.issue-B",
      "state": "done_blocked",
      "is_terminal": true,
      "task_name": "issue-B",
      "waits_on": ["issue-A"],
      "reason_code": "failure",
      "reason": "done_blocked",
      "skip_reason": null
    },
    {
      "name": "coord.issue-C",
      "state": "done",
      "is_terminal": true,
      "task_name": "issue-C",
      "waits_on": ["issue-A"],
      "reason_code": "success",
      "reason": null,
      "skip_reason": null
    },
    {
      "name": "coord.issue-D",
      "state": "skipped_due_to_dep_failure",
      "is_terminal": true,
      "task_name": "issue-D",
      "waits_on": ["issue-B"],
      "reason_code": "skipped",
      "reason": null,
      "skip_reason": "coord.issue-B"
    },
    {
      "name": "coord.issue-E",
      "state": "skipped_due_to_dep_failure",
      "is_terminal": true,
      "task_name": "issue-E",
      "waits_on": ["issue-D"],
      "reason_code": "skipped",
      "reason": null,
      "skip_reason": "coord.issue-D"
    }
  ]
}
```

Gloss:

- `reason_code` values `success | failure | skipped` are not enumerated
  explicitly in Decision 6. The design lists the fields but not the value
  vocabulary. `[GAP 7: reason_code enum values and their relationship to
  the `outcome` enum in gate output are not specified. Are they identical
  strings? Is `blocked` a possible reason_code? What about `running`?]`
- For issue-B, `reason` fell back to `"done_blocked"` (the state name),
  per Decision 6 "When failure_reason is unset, reason defaults to the
  failure state's name". This is the designed behavior but uninformative.
- For issue-E, `skip_reason` is `coord.issue-D` -- the *direct* skipped
  dependency, not the root-cause failure `coord.issue-B`. The design says
  `skipped_because: <failed_task_name>` (singular), but here issue-D is
  itself skipped, not failed. `[GAP 8: Transitive skip attribution is not
  specified. Decision 5.2 says `skipped_because: <failed_task_name>`.
  When issue-E is skipped because issue-D is skipped because issue-B
  failed, does `skipped_because` record "issue-D" (direct) or "issue-B"
  (root)? The singular phrasing suggests the scheduler picks one -- but
  which? Nothing in the design constrains this.]`
- There is no signal in `--children` output that coord.issue-D is
  *synthetic*. The caller would need to combine `state ==
  skipped_due_to_dep_failure` with out-of-band knowledge.
- `coord.issue-B` shows `waits_on: ["issue-A"]`. Good.
- Ordering of children matches `tasks` submission order.

### Turn 6 -- agent tries to learn WHY B failed

**AGENT:** `koto status coord.issue-B`

**KOTO:**
```json
{
  "workflow": "coord.issue-B",
  "state": "done_blocked",
  "is_terminal": true,
  "batch": null
}
```

Gloss: No reason, no directive, no context. The child's template didn't
write `failure_reason`, so there's nothing to surface. `koto status`
doesn't expose the `directive` for the current state either.

**AGENT:** `koto next coord.issue-B`

**KOTO:**
```json
{
  "action": "done",
  "state": "done_blocked",
  "directive": "Issue #102 is blocked and cannot proceed.",
  "is_terminal": true
}
```

Gloss: Re-ticking a terminal workflow returns the terminal directive for
the current state. This is consistent with the walkthrough's
"Interaction 5a" shape. Good -- it's idempotent and not an error.

`[GAP 9: Decision 5 / Decision 6 don't explicitly say re-ticking a terminal
failure workflow is idempotent. walkthrough.md shows `action: done,
is_terminal: true` but the user-facing contract should make this a
documented guarantee, not just an observed shape. Agents may otherwise
assume they'll get an error from "re-starting a finished workflow".]`

The directive prose here is helpful ("Issue #102 is blocked") but tells the
agent NOTHING about the underlying cause. The template author failed to
write `failure_reason` -- no amount of careful observability design saves
the agent from a careless template. But koto doesn't *warn* at compile-time
that `done_blocked` is a failure state without a failure_reason writer.

`[GAP 10: The compile-time warnings list in Decision 5 does not include a
W-code like "failure state doesn't appear to write failure_reason
context". Without this, template authors silently produce unhelpful
observability. Proposal: W3 "terminal failure state has no
context_assignments writing failure_reason" -- implementable by
grepping the state body or the accepts block.]`

### Turn 7 -- fallback to raw query

**AGENT:** `koto query coord`

**KOTO (abbreviated):**
```json
{
  "workflow": "coord",
  "state": "summarize",
  "is_terminal": true,
  "events": [
    {"type": "WorkflowInitialized", "ts": "..."},
    {"type": "Transitioned", "to": "plan_and_await"},
    {"type": "EvidenceSubmitted", "fields": {"tasks": [...], "submitter_cwd": "..."}},
    {"type": "Transitioned", "to": "summarize"}
  ],
  "context": {
    "plan_path": "PLAN.md"
  }
}
```

Gloss: `koto query` gives the raw events but doesn't embed child event logs.
To learn why B failed, the agent now has to `koto query coord.issue-B` too.
The design doesn't specify a parent-inclusive query or any aggregation.

`[GAP 11: To diagnose a batch failure when failure_reason is absent, the
agent must query each failed child individually. No single command gives
the complete diagnostic picture. Not a blocker, but worth noting for
shirabe ergonomics.]`

**AGENT:** `koto query coord.issue-B`

**KOTO:**
```json
{
  "workflow": "coord.issue-B",
  "state": "done_blocked",
  "is_terminal": true,
  "events": [
    {"type": "WorkflowInitialized", "ts": "..."},
    {"type": "Transitioned", "to": "working"},
    {"type": "EvidenceSubmitted", "fields": {"status": "blocked"}},
    {"type": "Transitioned", "to": "done_blocked"}
  ],
  "context": {
    "ISSUE_NUMBER": "102"
  }
}
```

Gloss: The events say *when* B failed (the `status: blocked` submission)
but not *why*. `koto query` is the floor for observability, and it's silent
on cause because the template didn't record one.

### Turn 8 -- query the synthetic child

**AGENT:** `koto query coord.issue-D`

**KOTO:**
```json
{
  "workflow": "coord.issue-D",
  "state": "skipped_due_to_dep_failure",
  "is_terminal": true,
  "events": [
    {"type": "WorkflowInitialized", "ts": "..."},
    {"type": "Transitioned", "to": "skipped_due_to_dep_failure"},
    {"type": "ContextAssigned", "key": "skipped_because", "value": "coord.issue-B"}
  ],
  "context": {
    "skipped_because": "coord.issue-B"
  }
}
```

Gloss: Here the `skipped_because` is recoverable from context. But the
synthesis atomicity story from Decision 5.2 -- "WorkflowInitialized plus
Transitioned to skipped_marker_state plus a context write" -- bundles
three events into one rename, which means the event ordering isn't
necessarily authoritative. The design says this is atomic via the
Decision 2 init bundle; fine, but the synthesis template is not documented
anywhere the agent can introspect. `[GAP 12: The synthetic mini-template
(Decision 5.2) is a reserved implementation detail. An agent running
`koto template compile` or `koto template list` sees nothing about it.
If a user ever needs to debug a skipped child, there's no canonical
reference for what states it has or what events to expect.]`

---

## Section 2: Findings

### 1. `all_complete: true` with failures -> parent routes to `summarize` silently
**Observation:** With `failure_policy: skip_dependents` and at least one
failure, the parent's children-complete gate reports `all_complete: true`
(since `pending == 0 AND blocked == 0`). The single transition `when:
gates.done.all_complete: true` fires and the parent advances to
`summarize`. Nothing in the template author's default directive for
`summarize` prepares them for the partial-failure case.
**Location:** Decision 5.3, Decision 7, walkthrough.md line 587.
**Severity:** MEDIUM. The failure scenario section of walkthrough.md
acknowledges this in prose ("The parent template's `summarize` state
should handle the partial-success case in its directive, or the template
could add a separate transition routing on `gates.done.failed > 0` to an
`analyze_failures` state") but no compile-time guidance or warning
enforces it.
**Proposed resolution:** Add W4 compile warning: "single-transition
children-complete gate with `failure_policy: skip_dependents` will
advance past failures silently; consider adding a transition on
`gates.<gate>.failed > 0`". Call this out in the koto-author skill's
batch reference.

### 2. `blocking_conditions` attachment on terminal responses is unspecified
**Observation:** The walkthrough's interaction 8 (terminal done response)
shows `action/state/directive/is_terminal` only, no
`blocking_conditions` and no `scheduler`. But the most recent gate
evaluation output is the ONLY machine-readable source of truth about
what succeeded, failed, and was skipped. Dropping it on the terminal
transition means the agent loses batch detail at the exact moment they
need to write a summary.
**Location:** walkthrough.md line 521, Decision 6 (silent on this).
**Severity:** HIGH for shirabe. The design's "one koto status per poll"
promise doesn't survive the parent leaving the batched state.
**Proposed resolution:** Specify that terminal responses include the
final `blocking_conditions` from the just-passed gate evaluation (the
snapshot the transition fired on) and that `koto status` on a terminal
parent still emits the `batch` section if the parent *has ever had* a
`materialize_children` hook in its event history, drawn from the last
gate evaluation. Alternatively, persist a final `BatchView` into context
as part of the transition.

### 3. `koto status` batch section disappears after leaving batched state
**Observation:** Decision 6 says the `batch` section is emitted "when
the current state has a `materialize_children` hook". Once the parent
advances to `summarize`, the hook is gone and `batch` becomes null,
even though the batch's results are still relevant to anyone polling.
**Location:** Decision 6, line 1099.
**Severity:** HIGH.
**Proposed resolution:** Emit the `batch` section whenever a batch was
ever materialized in this workflow (historical view from the most recent
gate evaluation on any batched state), not just on states that currently
hold the hook. Label it `batch_final` or add `batch.state: current |
historical` to the shape.

### 4. Synthetic skipped children are indistinguishable from real work
**Observation:** `koto status coord.issue-D` returns a shape identical
to any other terminal workflow. `skipped_marker: true` from the
template lives in the compile cache, not in `koto status` output. An
observer cannot tell from the response alone whether the child did real
work or was synthesized.
**Location:** Decision 5.2, Decision 6.
**Severity:** MEDIUM. Workable via the state name convention
(`skipped_due_to_dep_failure`) but fragile.
**Proposed resolution:** Project `skipped_marker: true` and
`skipped_because: <name>` into the `koto status` response for children
whose current state has `skipped_marker: true`.

### 5. Synthetic skipped mini-template has undefined directive prose
**Observation:** Decision 5.2 hardcodes the synthetic mini-template in
`src/cli/batch.rs` with "two states: initial and
skipped_due_to_dep_failure". The design doesn't specify what markdown
body either state has. `koto next coord.issue-D` likely returns a blank
directive.
**Location:** Decision 5.2, line 937.
**Severity:** LOW for end users (unlikely to koto-next a skipped child)
but MEDIUM for diagnostic UX.
**Proposed resolution:** Ship the synthetic template with fixed
directive text: "Task skipped because dependency
{{skipped_because}} did not succeed. No action required." Interpolate
`skipped_because` from the context. Document the synthetic template
shape in the koto-user skill.

### 6. Transitive skip attribution is ambiguous
**Observation:** Decision 5.2 says "skipped_because:
<failed_task_name>" (singular, implies a single failed task). When
issue-E is skipped because issue-D is skipped because issue-B failed,
which name is recorded? The transcript assumes "direct parent"
(issue-D) but the design doesn't specify. Both answers are defensible;
neither is documented.
**Location:** Decision 5.2, line 922.
**Severity:** MEDIUM.
**Proposed resolution:** Decide: "skipped_because records the direct
dependency whose failure or skip caused this task to be skipped."
Record the root cause separately as `skip_root_cause` (the first
failed task in the transitive chain), computed at synthesis time by
walking upstream. This gives observers both views without forcing a
single choice.

### 7. `reason_code` enum vocabulary is unspecified
**Observation:** Decision 6 lists `reason_code` as a `--children`
per-row field but doesn't enumerate allowed values. The parallel
`outcome` field on gate output enumerates `success | failure | skipped
| pending | blocked`. Is `reason_code` the same enum? Can it be
`running`? `not_spawned`?
**Location:** Decision 6, line 1132.
**Severity:** LOW-MEDIUM.
**Proposed resolution:** State explicitly that `reason_code ==
outcome` for children whose workflows exist; for not-spawned children
use `not_spawned`.

### 8. `reason` field falling back to state name is opaque
**Observation:** Decision 6 specifies: "When failure_reason is unset,
reason defaults to the failure state's name." For a careless template
like ours, `reason: "done_blocked"` is nearly useless -- it repeats
what the state field already says. The fallback is correct behavior but
its un-informativeness is not flagged.
**Location:** Decision 6, line 1142.
**Severity:** MEDIUM.
**Proposed resolution:** (a) Add compile warning W3 when a terminal
`failure: true` state has no context-assignment writing
`failure_reason` (see Finding 10). (b) When the fallback engages,
return `reason: null` instead of the state name, so observers can
distinguish "author wrote nothing" from "author chose to echo the
state name". (c) Add `reason_source: "failure_reason" | "state_name"
| "none"` to disambiguate.

### 9. Re-ticking a terminal workflow is not contractually specified
**Observation:** Throughout the walkthrough and transcript, `koto next`
on a terminal workflow returns the terminal directive idempotently.
This behavior is observed but never written into the contract.
**Location:** Not documented.
**Severity:** LOW (current code likely does this; risk is future
regressions).
**Proposed resolution:** Add a one-liner to Decision 5 or Decision 7
stating: "koto next on a terminal workflow is idempotent and returns
the terminal directive; it does not error and does not append events."

### 10. No compile warning for missing `failure_reason` writer
**Observation:** A template author can mark `failure: true` on a
terminal state without writing `failure_reason`, and koto will
silently produce the opaque fallback described in Finding 8. The
W1-W2 warnings listed in the design catch other classes of problems
but not this one.
**Location:** Decision 5.1, Decision 6.
**Severity:** MEDIUM. Biggest lever for improving batch diagnostics
with minimal engine change.
**Proposed resolution:** Add W3 compile warning: "failure state '%s'
has no context-assignment or accepts-field named `failure_reason`;
the batch view will fall back to the state name." Scan the state's
markdown body and accepts block.

### 11. No single-command diagnostic view
**Observation:** Learning *why* a batch failed requires: (a) `koto
status coord` for the summary, (b) `koto query coord.issue-B` to
inspect the failed child's events, (c) inference from context. No
aggregate "diagnose this batch" command exists. This is workable but
noisy for shirabe.
**Location:** Decision 6 (scope boundary).
**Severity:** LOW. Existing primitives cover the base case; shirabe
can aggregate.
**Proposed resolution:** Consider `koto status coord --deep` in a
follow-up that walks failed/skipped children and embeds their
`failure_reason`/`skipped_because` context keys in the batch view.

### 12. Synthetic template shape is undiscoverable
**Observation:** The synthetic mini-template used for skipped children
is hardcoded in `src/cli/batch.rs`. Neither `koto template list` nor
any other command exposes it. Agents and template authors cannot
introspect what states a skipped child can be in or what events its
log carries.
**Location:** Decision 5.2, line 939.
**Severity:** LOW.
**Proposed resolution:** Document the synthetic mini-template's full
schema (states, initial events, context keys) in the koto-user skill
reference and in the design doc's "Solution Architecture" section.

---

End of simulation.
