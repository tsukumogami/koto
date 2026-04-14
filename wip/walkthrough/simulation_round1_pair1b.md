# Simulation Round 1, Pair 1b: Diamond DAG + Retry Edge Cases

## Scenario

Plan: 4 issues A, B, C, D forming a diamond.

- A (no deps)
- B waits_on [A]
- C waits_on [A]
- D waits_on [B, C]

Parent template `coord` uses `failure_policy: skip_dependents` with a
post-gate analysis state `analyze_results` reached when `all_complete`
is true AND `failed > 0`, plus a `summarize` state when `failed == 0`.

Reach-state for the probe: A succeeded, B failed, C succeeded, D was
synthesized as a skipped marker (because B failed). Parent is at
`analyze_results`. The agent now probes retry edges.

---

## Section 1: Transcript

### Turn 0 (context): Reaching the probe state

Elided. Prior `koto next` calls produced:

- `coord.issue-A`: state `done`, outcome `success`
- `coord.issue-B`: state `done_blocked`, outcome `failure`, `failure_mode: true`
- `coord.issue-C`: state `done`, outcome `success`
- `coord.issue-D`: state `skipped_due_to_dep_failure`, outcome `skipped`,
  `skipped_because: "coord.issue-B"`, `skipped_marker: true`

Parent: `gate children-complete` output shows `total=4, success=2,
failed=1, skipped=1, all_complete=true`. Parent transitioned to
`analyze_results` whose accepts declares `retry_failed: { type: retry }`
(reserved) plus an optional `decision: enum [retry, give_up]`.

---

### Edge 1 — Double `retry_failed` without an intervening `koto next`

**AGENT** (attempting to retry B; hits fat-finger and sends twice):

```
koto next coord --with-data '{"retry_failed": {"children": ["coord.issue-B"], "include_skipped": true}}'
```

**KOTO:**

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "...planning directive...",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 4, "completed": 2, "pending": 2,
      "success": 2, "failed": 0, "skipped": 0, "blocked": 0,
      "all_complete": false,
      "children": [
        {"name": "coord.issue-A", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-B", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.issue-C", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-D", "state": "working", "complete": false, "outcome": "pending"}
      ]
    }
  }],
  "scheduler": {
    "spawned": [],
    "already": ["coord.issue-A", "coord.issue-B", "coord.issue-C", "coord.issue-D"],
    "retried": ["coord.issue-B", "coord.issue-D"],
    "blocked": [],
    "skipped": []
  }
}
```

Internal effects:

1. `handle_next` intercepts `retry_failed` BEFORE advance loop.
2. `handle_retry_failed` computes closure with `include_skipped: true`:
   `{B, D}` (D is a skipped-marker dependent).
3. For B: appends `Rewound → working` to B's log.
4. For D: D has only `WorkflowInitialized` + `Transitioned → skipped_due_to_dep_failure`;
   `skipped_marker: true`, so the state file is **deleted and respawned**
   atomically via `init_state_file`. A fresh `coord.issue-D` at `working`.
5. Appends `{"retry_failed": null}` clearing event to parent.
6. Because the parent needs to leave `analyze_results` to re-evaluate the
   gate, the template routes back to `plan_and_await` on `retry_failed`.
   `[GAP: The design text is inconsistent about how the parent transitions
   back. Section 5.4 step 1 says "Transitions the parent (no rewind at
   parent level) from the post-analysis state back to awaiting_children"
   — implying handle_retry_failed appends a Transitioned event directly.
   Section 1913-1916 of Data Flow says "The advance loop sees the
   retry_failed evidence and transitions the parent back to
   awaiting_children via a template-defined route." These two mechanisms
   conflict: the first bypasses template routing; the second requires a
   template-declared when-clause on the post-analysis state matching
   retry_failed. For this sim we assume template-defined routing: the
   analyze_results state declares a transition `when: evidence.retry_failed:
   present` → plan_and_await. But that breaks the "clearing event" idiom
   because the null-clear happens AFTER the transition has already been
   consumed.]`

**Gloss:** "My retry fired, B and D were rewound (D via delete+respawn),
scheduler already spawned them. Now I call `koto next coord.issue-B` to
drive B."

Before doing that, the agent hits Ctrl-Up and re-sends the same payload.

**AGENT:**

```
koto next coord --with-data '{"retry_failed": {"children": ["coord.issue-B"], "include_skipped": true}}'
```

**KOTO:**

`[GAP: design does not specify double-retry semantics.]` Two plausible
behaviors, neither stated:

(a) **Idempotent no-op** — scheduler sees B at `working` (non-terminal,
    not in `failed` set), so the retry set filters to nothing, the closure
    is empty, no rewind, but a second clearing event is appended.

(b) **Rewind-again** — `handle_retry_failed` rewinds B regardless of
    current state, creating a second Rewound event and destroying
    in-flight work. Because `internal_rewind_to_initial` is described as
    reusing `handle_rewind`'s machinery, and `handle_rewind` errors on
    "already at initial state", B at `working` with only
    `WorkflowInitialized + Rewound → working` is *already* at initial,
    so this case hits the same error path as D. For a failed child
    already rewound, `handle_rewind` should error — but
    `handle_retry_failed` has no documented error-handling branch for
    this case.

Assumed response (option a, best-case):

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "...planning directive...",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 4, "completed": 2, "pending": 2,
      "success": 2, "failed": 0, "skipped": 0, "blocked": 0,
      "all_complete": false,
      "children": [
        {"name": "coord.issue-A", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-B", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.issue-C", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-D", "state": "working", "complete": false, "outcome": "pending"}
      ]
    }
  }],
  "scheduler": {
    "spawned": [], "already": ["coord.issue-A", "coord.issue-B", "coord.issue-C", "coord.issue-D"],
    "retried": [], "blocked": [], "skipped": []
  },
  "warnings": [
    "retry_failed: none of {coord.issue-B} are in a retryable state (failed or skipped); ignored"
  ]
}
```

**Gloss:** "Second retry was a no-op because B is not failed anymore.
The event log has two `retry_failed: {...}` events, each followed by a
`retry_failed: null` clear. B has ONE Rewound event."

---

### Edge 2 — Partial retry: `include_skipped: false`

Reset scenario. Back to the probe state (B failed, D skipped).

**AGENT:**

```
koto next coord --with-data '{"retry_failed": {"children": ["coord.issue-B"], "include_skipped": false}}'
```

**KOTO:**

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "output": {
      "total": 4, "completed": 3, "pending": 1,
      "success": 2, "failed": 0, "skipped": 1, "blocked": 0,
      "all_complete": false,
      "children": [
        {"name": "coord.issue-A", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-B", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.issue-C", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-D", "state": "skipped_due_to_dep_failure", "complete": true, "outcome": "skipped", "skipped_because": "coord.issue-B"}
      ]
    }
  }],
  "scheduler": {
    "spawned": [], "already": ["coord.issue-A", "coord.issue-B", "coord.issue-C", "coord.issue-D"],
    "retried": ["coord.issue-B"], "blocked": [], "skipped": []
  }
}
```

**Gloss:** "B was rewound, D is still a skipped marker. I'll drive B."

The agent drives B to `done` (success).

**AGENT:** `koto next coord`

**KOTO:**

`[GAP: the design does not specify what the scheduler does when a task's
waits_on become satisfied but the task already exists on disk as a
skipped marker. Two candidate behaviors:

(i) "exists + terminal → skip reclassification" — scheduler sees D at
    a skipped_marker terminal state, classifies as Terminal(skipped),
    leaves it. `all_complete` becomes true with D still skipped despite
    B now succeeding. User is surprised.

(ii) "re-evaluate skipped markers when upstream recovers" — scheduler
     detects D is a skipped_marker AND all waits_on are now Terminal+success,
     deletes and re-materializes D. Requires a new pass in
     `run_scheduler_tick`.

The design's classify_task function description in the interfaces
section doesn't explicitly handle this case. Decision 5.4 step 3 says
the delete-and-respawn path is only taken for children whose state
files carry skipped_marker: true *during retry processing* — not during
a normal scheduler tick. So behavior (i) is the implemented default.]`

Assumed response (option i):

```json
{
  "action": "done",
  "state": "summarize",
  "directive": "All issues are complete...",
  "is_terminal": true
}
```

(or, if the template routes `all_complete AND skipped>0` to
`analyze_results` again, then `gate_blocked` at analyze_results.)

**Gloss:** "Parent thinks batch is complete even though D was skipped
and its upstream dependency recovered. To rerun D I must submit another
`retry_failed` with D in the children list."

---

### Edge 3 — Retry on a running (non-terminal) child

Scenario: B's second epoch is mid-flight (state `working`, not yet
terminal). Agent impatiently retries.

**AGENT:**

```
koto next coord --with-data '{"retry_failed": {"children": ["coord.issue-B"], "include_skipped": false}}'
```

**KOTO:**

`[GAP: design does not specify. handle_retry_failed calls
internal_rewind_to_initial for every child in the closure without
checking current state. If it follows handle_rewind's semantics
(src/cli/mod.rs:1198-1204 referenced), rewind on a non-initial,
non-terminal state is likely permitted — rewind just appends a Rewound
event targeting initial. So B at `working` in epoch 2 gets a Rewound
event starting epoch 3. Any in-flight evidence submitted to B (e.g.,
status: complete that the agent was about to submit) is invisible to
epoch 3.]`

Assumed response (permissive — the retry goes through):

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "output": {
      "total": 4, "completed": 2, "pending": 2,
      "success": 2, "failed": 0, "skipped": 0, "blocked": 0,
      "all_complete": false,
      "children": [
        {"name": "coord.issue-A", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-B", "state": "working", "complete": false, "outcome": "pending", "epoch": 3},
        {"name": "coord.issue-C", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-D", "state": "skipped_due_to_dep_failure", "complete": true, "outcome": "skipped"}
      ]
    }
  }],
  "scheduler": {"spawned": [], "retried": ["coord.issue-B"], "already": [...], "blocked": [], "skipped": []}
}
```

**Gloss:** "B was silently restarted. If another sub-agent was driving
B, its next `koto next coord.issue-B --with-data '...'` submission will
land in epoch 3 — but was built from epoch-2 directives. Potential
inconsistency."

---

### Edge 4 — Retry with a Terminal-success child in the set

**AGENT:**

```
koto next coord --with-data '{"retry_failed": {"children": ["coord.issue-A"], "include_skipped": false}}'
```

**KOTO:**

`[GAP: design does not specify whether retry_failed filters by
current outcome or trusts the caller. Three plausible behaviors:

(a) Hard error — response: action: "invalid_retry_set".

(b) Silent filter — the retry set shrinks to `{}` (A is not failed),
    empty closure, no rewinds, a clearing event only. The warnings array
    mentions the ignored name.

(c) "Reset from A downward" — treat retry as a targeted epoch reset;
    rewind A, then per `include_skipped: true` closure semantics,
    cascade to everything downstream (B, C, D). Powerful but unexpected
    from the name `retry_failed`.

The name "retry_failed" strongly suggests (b). The "closure through the
DAG" language in section 5.4 step 2 doesn't clarify direction. There's
no text about rejecting non-failed inputs.]`

Assumed response (option b):

```json
{
  "action": "gate_blocked",
  "state": "analyze_results",
  "blocking_conditions": [...unchanged...],
  "scheduler": {"spawned": [], "retried": [], "already": [...], "blocked": [], "skipped": []},
  "warnings": [
    "retry_failed: {coord.issue-A} is not in a retryable state (outcome=success); ignored. No children were rewound."
  ]
}
```

**Gloss:** "My request to retry A was dropped. If I wanted a full
diamond re-run I'd have to either use option (c) semantics — which
don't exist — or synthesize a retry set for {B, D} manually. There's no
way to express 'redo from A'."

---

### Edge 5 — `include_skipped: false` on a skipped child

Scenario: B failed, D was skipped. Agent attempts to retry only D.

**AGENT:**

```
koto next coord --with-data '{"retry_failed": {"children": ["coord.issue-D"], "include_skipped": false}}'
```

**KOTO:**

`[GAP: closure direction is undefined. Section 5.4 step 2 says closure
extends "through the DAG" and that "include_skipped: true extends the
set to include dependents that were skipped because of a failure in the
retry set." The include_skipped flag only extends DOWNWARD from a failed
node to its skipped dependents. Nothing extends UPWARD from a skipped
node to its failed ancestors. So retrying D without B leaves B still
failed AND tries to rewind D.

For D: it's a skipped_marker, delete-and-respawn path fires. The child
state file is re-initialized. On the next scheduler tick, D's waits_on
are evaluated: B is still failed (outcome=failure), so D gets re-synthesized
as a skipped marker AGAIN. This is a thrash loop with the agent as the
driver.]`

Assumed response:

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "output": {
      "total": 4, "completed": 3, "pending": 0,
      "success": 2, "failed": 1, "skipped": 1, "blocked": 0,
      "all_complete": true,
      "children": [
        {"name": "coord.issue-A", "state": "done", "outcome": "success"},
        {"name": "coord.issue-B", "state": "done_blocked", "outcome": "failure"},
        {"name": "coord.issue-C", "state": "done", "outcome": "success"},
        {"name": "coord.issue-D", "state": "skipped_due_to_dep_failure", "outcome": "skipped", "skipped_because": "coord.issue-B"}
      ]
    }
  }],
  "scheduler": {
    "spawned": [],
    "already": [...],
    "retried": ["coord.issue-D"],
    "skipped": ["coord.issue-D"],
    "blocked": []
  }
}
```

(The scheduler retried D, then re-classified D as ShouldBeSkipped in
the same tick, then re-materialized the skipped marker.)

**Gloss:** "I spent a retry but D is still skipped because the
underlying cause (B's failure) wasn't addressed. The tool accepted a
request that could never succeed."

---

### Edge 6 — Retry where B's new epoch ALSO fails

Scenario: agent retried B, drove B, B failed again (epoch 2,
`done_blocked`).

**AGENT:** `koto next coord`

**KOTO:**

`[GAP: how does the scheduler re-skip D in this second round? D
already exists on disk (it was delete-and-respawned in the retry). If
the skip path in run_scheduler_tick sees D already present as a skipped
marker from an earlier round — no, wait, in this scenario the retry
rewound B *and* deleted-and-respawned D. So D is currently at `working`
(fresh epoch 1 of the post-retry D). Now B fails. Scheduler must
re-classify D as ShouldBeSkipped.

Question: what does that do to D's state file? D is at `working` with
event log [WorkflowInitialized, (no Transitioned yet — D was never
driven)]. The scheduler would need to append a `Transitioned →
skipped_due_to_dep_failure` event. But D was init-spawned with a real
child template, not the synthetic skipped-marker template. Is the
synthetic template's skipped state reachable from D's live template?
Only if every child template declares a skipped_marker state, which
Decision 5.2 explicitly says they should NOT have to.

Option A: delete D again and respawn with the synthetic template.
Requires the scheduler's init path to distinguish "never spawned" from
"was running, now must be skipped."

Option B: allow cross-template transitions for the skipped marker.
Violates template boundaries.

Option C: D stays at `working` forever (outcome: pending). all_complete
is never true. The parent is stuck.

The design does not address re-skipping a running child whose
upstream dependency flipped from running→failure during its execution.]`

Assumed response (option A, but undocumented):

```json
{
  "action": "gate_blocked",
  "state": "analyze_results",
  "blocking_conditions": [{
    "output": {
      "total": 4, "completed": 3, "pending": 0,
      "success": 2, "failed": 1, "skipped": 1, "blocked": 0,
      "all_complete": true,
      "children": [
        {"name": "coord.issue-A", "state": "done", "outcome": "success"},
        {"name": "coord.issue-B", "state": "done_blocked", "outcome": "failure"},
        {"name": "coord.issue-C", "state": "done", "outcome": "success"},
        {"name": "coord.issue-D", "state": "skipped_due_to_dep_failure", "outcome": "skipped", "skipped_because": "coord.issue-B"}
      ]
    }
  }],
  "scheduler": {"spawned": [], "retried": [], "already": [...], "skipped": ["coord.issue-D"], "blocked": []}
}
```

**Gloss:** "D is re-skipped. Under the hood koto had to delete and
re-init D's state file a second time. Event-log auditors looking at D
over the full timeline see: init→skipped (round 1), delete→init→running
(retry round 2), delete→init→skipped (round 3). The audit trail for D
is three *separate files* across time."

---

### Edge 7 — Mixed payload: `retry_failed` + regular field

**AGENT:**

```
koto next coord --with-data '{"retry_failed": {"children": ["coord.issue-B"], "include_skipped": true}, "decision": "retry"}'
```

(The analyze_results state accepts `decision: enum [retry, give_up]` as
a required field.)

**KOTO:**

`[GAP: Decision 5.4 says retry_failed is intercepted BEFORE
advance_until_stop. What happens to the co-submitted `decision` field?

Option (i): both are processed. Interception consumes retry_failed +
rewinds children + appends the clearing event. Then advance_until_stop
sees the evidence *with retry_failed: null but decision: "retry" still
present*, treats decision as the evidence, and transitions per the
template's when-clause. The advance loop effectively fires a transition
on the same call as the retry.

Option (ii): mutual exclusion — if retry_failed is present, the advance
loop is skipped entirely, other fields are ignored. Decision 5.4 text
"Only after retry processing completes does advance_until_stop run on
the (now-modified) parent state" implies (i).

Option (iii): advance loop runs normally, consumes both fields, and the
transition path expects decision == "retry" to route the parent back to
plan_and_await, which is exactly what the template author wanted —
making retry_failed + decision a coherent "I've chosen retry, please
execute it" payload.

The ambiguity matters because in option (i) the agent might
accidentally trigger an unwanted transition if it always includes a
decision field. In option (ii) the required: true on decision must NOT
fire when retry_failed is present, which requires a special-case in
the evidence validator.]`

Assumed response (option iii — most useful):

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [...],
  "scheduler": {"spawned": [], "retried": ["coord.issue-B", "coord.issue-D"], "already": [...], "blocked": [], "skipped": []}
}
```

**Gloss:** "Both fields took effect. retry rewound {B, D}, then the
advance loop saw decision: retry and routed analyze_results →
plan_and_await. On next tick, the scheduler found them rewound and
already-progressing."

But under option (ii), the `decision` field would be dropped silently,
the parent would stay at `analyze_results`, the user would wonder why
nothing happened.

---

### Edge 8 — `retry_failed: null` clearing event visible in history

**AGENT:** `koto query coord`

**KOTO (partial event log excerpt):**

```json
{
  "workflow": "coord",
  "events": [
    {"seq": 0, "type": "WorkflowInitialized", "state": "plan_and_await"},
    {"seq": 1, "type": "EvidenceSubmitted", "fields": {"tasks": [...], "submitter_cwd": "..."}},
    {"seq": 2, "type": "EvidenceSubmitted", "fields": {"retry_failed": {"children": ["coord.issue-B"], "include_skipped": true}}},
    {"seq": 3, "type": "EvidenceSubmitted", "fields": {"retry_failed": null}},
    {"seq": 4, "type": "Transitioned", "state": "analyze_results"}
  ]
}
```

**Gloss:** The agent inspecting this log sees `{"retry_failed": null}`
as a distinct EvidenceSubmitted event with no further context.

`[GAP: the design mentions the null-clearing idiom depends on
merge_epoch_evidence treating null as "unset," but the agent-facing
query output has no rendering hint. An agent reading history may
interpret `{"retry_failed": null}` as "the user submitted a retry
request that was rejected" or "an empty retry that did nothing" rather
than "scheduler acknowledged the retry." Recommend either:

(a) Emit a synthetic event type for the clear, e.g.,
    `RetryAcknowledged { cleared: ["retry_failed"], affected_children: [...] }`.
(b) Tag the clearing event in EvidenceSubmitted.fields with a
    `_clearing: true` marker or system-emitted annotation.
(c) Document in koto-user skill that null-valued fields in event logs
    are scheduler bookkeeping.

The design proposes (c) — "documented with a prominent code comment" —
but only for internal maintainers, not for agents reading logs.]`

---

## Section 2: Findings

### 1. Transition-trigger for parent post-retry is underspecified

- **Observation:** Section 5.4 step 1 says `handle_retry_failed`
  "transitions the parent (no rewind at parent level) from the
  post-analysis state back to awaiting_children," but the Data Flow
  "Retry" section says the advance loop sees retry_failed and
  transitions via "a template-defined route." These two mechanisms
  conflict — one bypasses template routing, the other requires a
  template-declared `when: retry_failed: present` clause.
- **Location in design:** Decision 5.4 step 1 (lines 1013-1014) vs.
  Data Flow "Retry" (lines 1914-1916).
- **Severity:** blocker
- **Proposed resolution:** Pick one. Recommend advance-loop routing
  (template-declared transitions on `evidence.retry_failed: present`)
  because it keeps the template in control and matches the "interception
  BEFORE advance_until_stop" wording — the interception mutates children,
  the advance loop then runs normally and the template's when-clause
  fires. Update 5.4 step 1 to remove the "transitions the parent" claim.

### 2. Double `retry_failed` semantics unspecified

- **Observation:** If the agent submits `retry_failed` twice without a
  child-level intervening call, the second submission's behavior is
  undocumented. Candidates: idempotent no-op, error, or second Rewound
  event (which would destroy the in-flight retry).
- **Location in design:** Decision 5.4 — no mention.
- **Severity:** should-fix
- **Proposed resolution:** Document: "`handle_retry_failed` filters the
  retry set by current outcome. Children not currently in {failure,
  skipped} are silently dropped from the closure and reported in a
  `warnings` array on the next response. The clearing event is appended
  regardless." Add a test covering this.

### 3. Upstream-recovered skipped children are not re-scheduled

- **Observation:** When B fails and D is skipped, then retry succeeds
  on B without `include_skipped: true`, D remains a skipped marker
  forever. Normal scheduler ticks do not re-materialize D even though
  its `waits_on` are now satisfied with successes.
- **Location in design:** Decision 5.4 step 3 — the delete-and-respawn
  path is only specified as part of retry processing, not as part of the
  scheduler tick.
- **Severity:** should-fix
- **Proposed resolution:** Either (a) require `include_skipped: true`
  by default (most users expect "retry and unblock everything that was
  blocked by this"), or (b) add a scheduler-tick rule: "if a task is
  currently a skipped marker and all waits_on are now Terminal+success,
  delete-and-respawn with the real template." Option (a) is simpler.

### 4. Retry on a running (non-terminal) child is not guarded

- **Observation:** `handle_retry_failed` calls
  `internal_rewind_to_initial` on every closure member without checking
  whether the child is terminal. A running child gets a new epoch
  silently. In-flight work and in-flight evidence submissions (possibly
  from a parallel sub-agent) are discarded.
- **Location in design:** Decision 5.4 step 3.
- **Severity:** should-fix
- **Proposed resolution:** `handle_retry_failed` must filter the closure
  to `{outcome ∈ {failure, skipped}}` and warn about ignored names. Do
  not rewind running children silently. (Pairs with finding 2.)

### 5. Retry set including a successful child: behavior unspecified

- **Observation:** `retry_failed: {"children": ["A"]}` with A successful
  has no documented behavior. The name suggests a silent filter, but
  the closure language leaves room for "reset A and cascade to its
  dependents."
- **Location in design:** Decision 5.4 — no mention.
- **Severity:** should-fix
- **Proposed resolution:** Same as finding 4 — filter by current
  outcome, surface warnings. Also, add a separate design note that
  "retry from successful node + cascade" is out of scope for v1 and
  would require a new evidence action (e.g., `rerun_from`).

### 6. Closure direction is ambiguous

- **Observation:** "Transitive closure through the DAG" does not state
  whether closure goes up (to dependencies) or down (to dependents).
  The `include_skipped: true` text implies down-only. No text handles
  "agent retries D but B is still failed" — which produces a thrash:
  D is rewound → D's waits_on still include failed B → scheduler re-skips
  D in the same tick.
- **Location in design:** Decision 5.4 step 2.
- **Severity:** should-fix
- **Proposed resolution:** State explicitly that closure is DOWNWARD
  (dependent-direction) only. When a skipped child is in the retry set
  but its failed ancestor is NOT, fail fast: return an error telling the
  agent which ancestors must also be included. Alternatively, auto-
  expand upward to include failed ancestors. Recommend the former
  (explicit is safer).

### 7. Re-skipping a running child after upstream failure

- **Observation:** If B's retry epoch fails while D is already running
  in its own retry epoch, the scheduler must re-skip D. D's state file
  is currently a real child template instance, not a synthetic
  skipped-marker. There is no documented path from "live real-template
  child at `working`" to "skipped marker terminal state" without
  crossing template boundaries.
- **Location in design:** Decision 5.2 (synthetic skipped template),
  Decision 5.4 (retry), and the scheduler re-classification flow —
  none address this transition.
- **Severity:** blocker
- **Proposed resolution:** Extend the scheduler's re-skip path to:
  (a) detect a running child whose waits_on transitioned to failure,
  (b) atomically delete the child state file and re-spawn it with the
  synthetic skipped-marker template via the same atomic init bundle
  from Decision 2. Document that re-skip is destructive: the child's
  epoch-2 event log is erased. Alternatively, abort the parent with
  a clear error and require the agent to issue an explicit retry_failed.

### 8. Mixed payload `retry_failed` + regular field is unspecified

- **Observation:** A payload containing both `retry_failed` and another
  accepts field (e.g., a required `decision` enum on the analysis
  state) has no documented handling. The "intercepted BEFORE
  advance_until_stop" wording implies the advance loop still runs with
  the remaining fields, but evidence validation for required fields
  when only `retry_failed` was intended is unclear.
- **Location in design:** Decision 5.4 "Interception point."
- **Severity:** should-fix
- **Proposed resolution:** Document: "When `retry_failed` is present,
  it is processed first; the remaining fields then flow through the
  normal advance loop as regular EvidenceSubmitted events. Template
  authors who want retry to be mutually exclusive with other evidence
  must declare a state whose accepts only contains `retry_failed`."
  Add a compile-time warning (W3) when an accepts block mixes
  `retry_failed` with other required fields — likely author error.

### 9. `retry_failed: null` clearing event is user-visible in `koto query`

- **Observation:** The null-clearing idiom is plumbing, but it surfaces
  as a distinct `EvidenceSubmitted {"retry_failed": null}` entry in
  `koto query` output. An agent reading history to reason about prior
  retries cannot distinguish "retry was submitted and acknowledged"
  from "retry was submitted but nulled out by something else." There
  is no rendering hint, no synthetic event type, no marker field.
- **Location in design:** Decision 5.4 step 4.
- **Severity:** nice-to-have
- **Proposed resolution:** Options (best to worst):
  (a) Emit a distinct event type `RetryFailedCleared { children: [...],
      affected: [...], succeeded: true }` instead of
      `EvidenceSubmitted {"retry_failed": null}`.
  (b) Tag the clearing event with `fields._system: true` or
      `fields._clearing_for: "retry_failed"` so renderers can hide/label
      it.
  (c) Document the pattern in the koto-user skill and leave rendering
      as-is.
  The design currently picks (c). Prefer (a) because it makes the audit
  trail legible to agents that weren't involved in the original retry.

### 10. No upper bound on retries of the same child

- **Observation:** Nothing prevents an agent from retrying B 50 times.
  Each retry appends a Rewound event (or delete+respawn for skipped
  markers). No cap, no warning, no rate limit. The design explicitly
  mentions "retry throttling... are not rate-limited" in security
  considerations — but that only covers throttling, not semantic caps.
- **Location in design:** Security Considerations, "Resource bounds."
- **Severity:** nice-to-have
- **Proposed resolution:** Add a per-child retry counter to the gate
  output (`retry_count: N`) so templates can route to a "give up"
  state after M retries. This is observable by the template author
  without changing retry semantics.

### 11. Event-log-per-child after multiple skip/retry cycles is fragmented

- **Observation:** A child that is skipped, retried (delete+respawn),
  driven, re-skipped (because upstream failed again), retried once
  more, ... has multiple discrete state files replaced over time.
  Only the current file exists on disk. The audit trail of "what
  happened to this task" is NOT reconstructible from the child's
  state file alone — the parent's EvidenceSubmitted events are the
  only record. This undermines the design's "unified discovery via
  backend.list()" benefit for skipped children.
- **Location in design:** Decision 5.2 (skipped-child representation),
  Decision 5.4 step 3 (delete+respawn).
- **Severity:** should-fix
- **Proposed resolution:** Before delete+respawn, archive the old
  state file to `~/.koto/sessions/<parent>/<child>.skipped-epoch-N.json`
  so the audit trail is preserved across re-skips. Or, introduce a
  "SkippedReplaced" event on the parent's log that captures the prior
  child's event log by reference. The former is cheaper and more local.
