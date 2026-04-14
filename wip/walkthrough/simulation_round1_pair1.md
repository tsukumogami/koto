# Simulation Round 1, Pair 1: Diamond DAG with Failure + Retry

**Shape:** A -> B, A -> C, B -> D, C -> D (classic 4-node diamond).
**Pattern:** B fails, C succeeds, D gets skipped, agent retries B, which
drags D along, both succeed.

The parent template in this simulation is the same single-state fan-out
`coord.md` from `walkthrough.md`, except I (as designer) had to add a
second transition to stop the batch from flowing straight into
`summarize` the moment the gate reports `all_complete: true` with
failures. Without that, there is nowhere for the agent to submit
`retry_failed`. See Finding 1.

Parent template used below (conceptual; deviation from walkthrough.md
noted):

```yaml
states:
  plan_and_await:
    transitions:
      - target: analyze_failures
        when:
          gates.done.all_complete: true
          gates.done.failed_or_skipped_nonzero: true  # [GAP -- see Finding 1]
      - target: summarize
        when:
          gates.done.all_complete: true
    accepts:
      tasks: {type: tasks, required: true}
    materialize_children:
      from_field: tasks
      failure_policy: skip_dependents
      default_template: impl-issue.md
    gates:
      done: {type: children-complete}
  analyze_failures:
    accepts:
      retry_failed: {...}  # [GAP -- Decision 5.4 says retry_failed is RESERVED and MUST NOT appear in accepts]
    transitions:
      - target: plan_and_await
        when: ...
  summarize: {terminal: true}
```

`[GAP: The walkthrough's canonical coord.md has a single transition on
gates.done.all_complete: true, which fires on success AND partial-
failure batches because all_complete tightens only to "pending==0 AND
blocked==0" (Decision 5.3). That means a batch with failures slams
straight into the terminal summarize state with no opportunity to
retry. The design mentions "if the template routes to an analysis state
instead of summarize" (walkthrough.md:591) but offers no guidance for
how the template expresses that route.]`

---

## Section 1: Transcript

### Turn 1 -- Parent init

**AGENT:** I'm going to implement a diamond. Four issues with A
unblocking B and C, and D blocked on both. Starting the parent.

```
$ koto init coord --template coord.md --var plan_path=PLAN-diamond.md
```

**KOTO:**
```json
{
  "action": "initialized",
  "workflow": "coord",
  "state": "plan_and_await",
  "template": "coord.md"
}
```

*What this tells the agent:* the parent is parked at `plan_and_await`.
Next step is a `koto next coord`.

### Turn 2 -- First tick, planning directive

**AGENT:**
```
$ koto next coord
```

**KOTO:**
```json
{
  "action": "evidence_required",
  "state": "plan_and_await",
  "directive": "Read the plan document at PLAN-diamond.md. For each issue outline in the plan:\n\n1. Extract the issue number, goal, files, and acceptance criteria\n2. Map dependencies to sibling task names (issue N -> \"issue-N\")\n3. Build a task entry: name=\"issue-N\", vars={\"ISSUE_NUMBER\": \"N\"}, waits_on=[\"issue-X\", ...] for each listed dependency\n   (check the `expects.fields.tasks.item_schema` field in the response for the full task entry schema; template defaults to impl-issue.md)\n\nSubmit the complete task list as JSON:\n`koto next coord --with-data @tasks.json`",
  "details": "Each child is an independent koto workflow named `coord.<task-name>`. The scheduler runs on every `koto next coord` call and spawns tasks whose `waits_on` dependencies are all terminal.",
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "tasks": {
        "type": "tasks",
        "required": true,
        "item_schema": {
          "name": {"type": "string", "required": true, "description": "Child workflow short name"},
          "template": {"type": "string", "required": false, "default": "impl-issue.md"},
          "vars": {"type": "object", "required": false},
          "waits_on": {"type": "array", "required": false, "default": []},
          "trigger_rule": {"type": "string", "required": false, "default": "all_success"}
        }
      }
    }
  },
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 0, "completed": 0, "pending": 0,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 0,
      "all_complete": false, "children": []
    }
  }],
  "scheduler": null
}
```

*What this tells the agent:* I need to produce a tasks array. The
item_schema nails the per-entry shape -- I don't have to guess.

### Turn 3 -- Submit the task list

**AGENT:** Here is `tasks.json`:

```json
{
  "tasks": [
    {"name": "issue-A", "vars": {"ISSUE_NUMBER": "201"}},
    {"name": "issue-B", "vars": {"ISSUE_NUMBER": "202"}, "waits_on": ["issue-A"]},
    {"name": "issue-C", "vars": {"ISSUE_NUMBER": "203"}, "waits_on": ["issue-A"]},
    {"name": "issue-D", "vars": {"ISSUE_NUMBER": "204"}, "waits_on": ["issue-B", "issue-C"]}
  ]
}
```

```
$ koto next coord --with-data @tasks.json
```

**KOTO:**
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
      "total": 4, "completed": 0, "pending": 4,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 3,
      "all_complete": false,
      "children": [
        {"name": "coord.issue-A", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.issue-B", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.issue-A"]},
        {"name": "coord.issue-C", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.issue-A"]},
        {"name": "coord.issue-D", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.issue-B", "coord.issue-C"]}
      ]
    }
  }],
  "scheduler": {
    "spawned": ["coord.issue-A"],
    "already": [],
    "blocked": ["coord.issue-B", "coord.issue-C", "coord.issue-D"],
    "skipped": []
  }
}
```

*What this tells the agent:* issue-A is spawned and ready. The other
three are waiting. Drive A first.

### Turn 4 -- Drive A to completion

**AGENT:**
```
$ koto next coord.issue-A
```

**KOTO:** (the working-state evidence_required response -- eliding for
brevity, shape matches walkthrough.md Interaction 4)

**AGENT:** done implementing 201. tests pass.
```
$ koto next coord.issue-A --with-data '{"status": "complete"}'
```

**KOTO:**
```json
{
  "action": "done",
  "state": "done",
  "directive": "Issue #201 implemented successfully.",
  "is_terminal": true
}
```

### Turn 5 -- Re-tick parent, fan out B and C

**AGENT:**
```
$ koto next coord
```

**KOTO:**
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
      "total": 4, "completed": 1, "pending": 3,
      "success": 1, "failed": 0, "skipped": 0, "blocked": 1,
      "all_complete": false,
      "children": [
        {"name": "coord.issue-A", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-B", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.issue-C", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.issue-D", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.issue-B", "coord.issue-C"]}
      ]
    }
  }],
  "scheduler": {
    "spawned": ["coord.issue-B", "coord.issue-C"],
    "already": ["coord.issue-A"],
    "blocked": ["coord.issue-D"],
    "skipped": []
  }
}
```

*What this tells the agent:* two new workers to start. D still waits.

**AGENT probe:** Hold on -- summary says `pending: 3, blocked: 1`, but
the children array has B and C with `outcome: "pending"` and D with
`outcome: "blocked"`. Why is D counted in both `pending` and `blocked`?

`[GAP: Decision 5.3 says "pending" is an outcome enum value AND a
summary counter, but it's unclear whether the summary "pending" counter
includes items whose outcome is "blocked". In the walkthrough.md
Interaction 6 response the summary shows total=3, completed=1,
pending=2, success=1, failed=0, skipped=0, blocked=0 -- and two
children have outcome "pending". Here with the diamond we have 3
non-terminal children (B pending, C pending, D blocked). Is "pending"
in the summary = "non-terminal" (3) or "running/not-yet-terminal-but-
not-blocked" (2)? The design's invariant `all_complete == pending==0
AND blocked==0` (Decision 5.3) implies pending excludes blocked, but
then in Turn 3 with 4 tasks, 1 running and 3 blocked, the walkthrough-
style response would need `pending: 1, blocked: 3` -- and the
walkthrough.md Interaction 3 response indeed shows `pending: 3,
blocked: 2` for a 3-task batch with 1 running and 2 blocked, which
contradicts pending-excludes-blocked. So the two existing examples are
internally inconsistent.]`

Proceeding with the pending-includes-blocked convention since that's
what both walkthrough.md examples actually show. I'll drive B and C.

### Turn 6 -- B fails

**AGENT:** Going to B first. Implementing... the migration is blocked
by a missing upstream dep I can't resolve.
```
$ koto next coord.issue-B --with-data '{"status": "blocked"}'
```

**KOTO:**
```json
{
  "action": "done",
  "state": "done_blocked",
  "directive": "Issue #202 is blocked and cannot proceed.",
  "is_terminal": true
}
```

*What this tells the agent:* B terminated in a failure state.

**AGENT probe:** From this response alone, can I tell that this was a
failure versus a success? The `action` is just `"done"` and `state` is
`"done_blocked"`, but I would have to know that done_blocked has
`failure: true` to classify this correctly.

`[GAP: The child's terminal response does not surface the "failure"
flag from the child template. If the child is not the walkthrough's
canonical impl-issue template, an agent driving a child it didn't
author has no way to tell a terminal-success from a terminal-failure
without parsing the child's template or re-reading the parent's gate
output. Recommend adding "failure": true to the child's terminal
response when the state has failure: true.]`

### Turn 7 -- C completes

**AGENT:** C was straightforward.
```
$ koto next coord.issue-C --with-data '{"status": "complete"}'
```

**KOTO:**
```json
{
  "action": "done",
  "state": "done",
  "directive": "Issue #203 implemented successfully.",
  "is_terminal": true
}
```

### Turn 8 -- Re-tick parent, D gets synthesized-as-skipped

**AGENT:**
```
$ koto next coord
```

**KOTO:**
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
      "total": 4, "completed": 4, "pending": 0,
      "success": 2, "failed": 1, "skipped": 1, "blocked": 0,
      "all_complete": true,
      "children": [
        {"name": "coord.issue-A", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-B", "state": "done_blocked", "complete": true, "outcome": "failure", "failure_mode": true},
        {"name": "coord.issue-C", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-D", "state": "skipped_due_to_dep_failure", "complete": true, "outcome": "skipped", "skipped_because": "coord.issue-B"}
      ]
    }
  }],
  "scheduler": {
    "spawned": ["coord.issue-D"],
    "already": ["coord.issue-A", "coord.issue-B", "coord.issue-C"],
    "blocked": [],
    "skipped": ["coord.issue-D"]
  }
}
```

*What this tells the agent:* all 4 terminal, but 1 failed and 1 was
skipped because of that failure. `all_complete: true` means the parent
would now satisfy the transition guard.

`[GAP: the scheduler outcome shows coord.issue-D in BOTH spawned AND
skipped. Is "spawned" meant to mean "init_state_file'd this tick" (in
which case yes, D was synthesized here), or "spawned as a real child
ready to be driven" (in which case no, D is a skipped marker and
shouldn't be)? Decision 5 uses init_state_file for both real spawn and
skipped synthesis. The scheduler outcome vocabulary needs clarifying:
either split "spawned" from "synthesized_skipped", or document that
spawned means "state file created this tick, regardless of whether it's
a real child or a skipped marker".]`

**AGENT probe:** The summary says `failed: 1, skipped: 1, success: 2`.
I got D's skipped_because field, great. But there's only ONE transition
in the walkthrough template: `target: summarize when:
gates.done.all_complete: true`. That's going to fire now, right? Which
means the parent is about to go terminal with one failed issue, and I
never got a chance to retry.

`[GAP: Decision 5 describes retry_failed as the recovery mechanism,
but the canonical parent template in walkthrough.md has a single
transition straight from plan_and_await to summarize (terminal) on
all_complete: true. Since all_complete tightens to pending==0 AND
blocked==0 (not "all success"), this fires on partial-failure batches
and strands the agent. The design needs either:
  (a) a recommended two-transition pattern in the walkthrough template
      (to analyze_failures first, then to summarize), or
  (b) a new gate field like gates.done.any_failed or
      gates.done.success_count so the template author can write
      distinct guards, or
  (c) route retry through a reserved top-level evidence action that is
      accepted at ANY state (including summarize), not just states
      declared to accept it.
Without one of these, the retry path is declared but unreachable from
the documented template.]`

Pretending the template has the two-transition pattern (see Finding 1),
the parent routes to `analyze_failures`.

### Turn 9 -- Re-tick again, land at analyze_failures

**AGENT:**
```
$ koto next coord
```

**KOTO:** (same all_complete output, but action now `evidence_required`
at `analyze_failures` state, with a directive about reviewing failures
and optionally submitting retry_failed)

```json
{
  "action": "evidence_required",
  "state": "analyze_failures",
  "directive": "The batch finished with partial success. Review the failed and skipped children. To retry, submit {\"retry_failed\": {\"children\": [...], \"include_skipped\": bool}}. To accept the partial result, submit {\"accept\": true}.",
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "accept": {"type": "boolean", "required": false}
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

`[GAP: retry_failed is a reserved top-level key (Decision 5.4) and
MUST NOT appear in the template's accepts block. But then how does the
agent discover it? The expects.fields in the response won't list it.
The design says the koto-user skill documents it. Agents that don't
have the skill loaded would only see "accept" in expects and would not
know retry_failed is even an option. Recommend either: (a) synthesize
a meta entry in expects.fields for reserved actions like
{"retry_failed": {"type": "reserved_action", "schema": {...}}}, or
(b) include a top-level `available_actions` array in the response at
batch-complete ticks listing `retry_failed` with its schema.]`

### Turn 10 -- Agent probes D's observability

**AGENT probe:** Before I retry, I want to sanity-check D. Can I
inspect D directly?
```
$ koto status coord.issue-D
```

**KOTO:**
```json
{
  "workflow": "coord.issue-D",
  "state": "skipped_due_to_dep_failure",
  "is_terminal": true
}
```

`[GAP: koto status on a synthetic-skipped child. Decision 6 extends
koto status with a "batch" section on PARENT workflows, but says
nothing about what koto status returns on a SKIPPED CHILD. The child
has a real state file (Decision 5.2 uses init_state_file for
synthesis), so koto status should work, but: (i) the state name comes
from the synthetic template, not the parent's actual child template
(impl-issue.md has no skipped_due_to_dep_failure state); (ii) the
child header's template_source_dir is... what? The parent's? The
synthetic template has no source file. This interacts badly with
Decision 4's path-resolution scheme. Recommend: synthetic children
should write a header with a sentinel marker like
template_source_dir: "<synthetic>" and koto status should expose
skipped_because and failure-dependency-chain context.]`

**AGENT probe:** Where does `failure_reason` come from? I never saw a
reason attached to B anywhere in these responses.

`[GAP: Decision 6 says reason is pulled from a context key named
failure_reason that the child writes "before entering its terminal-
failure state." The walkthrough's impl-issue.md template does not
write failure_reason when status is blocked -- it only accepts status:
{complete, blocked}. So in a canonical happy/sad-path example, reason
will always default to the state name "done_blocked". The design
should either: (a) update the reference impl-issue.md to demonstrate
writing failure_reason, or (b) add a reason field to the accepts block
that is automatically written to the context on terminal-failure
transitions, or (c) accept that v1 reason is always the state name and
document that clearly.]`

### Turn 11 -- Retry B, pull D along

**AGENT:**
```
$ koto next coord --with-data '{"retry_failed": {"children": ["coord.issue-B"], "include_skipped": true}}'
```

**KOTO:**
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
      "total": 4, "completed": 2, "pending": 2,
      "success": 2, "failed": 0, "skipped": 0, "blocked": 1,
      "all_complete": false,
      "children": [
        {"name": "coord.issue-A", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-B", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.issue-C", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-D", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.issue-B"]}
      ]
    }
  }],
  "scheduler": {
    "spawned": [],
    "already": ["coord.issue-A", "coord.issue-B", "coord.issue-C"],
    "blocked": ["coord.issue-D"],
    "skipped": [],
    "deleted": ["coord.issue-D"]
  }
}
```

*What this tells the agent:* retry applied. B is back at `working`
(rewound via the Rewound event, fresh epoch). D's synthetic-skipped
state file was deleted, so D shows as `state: null, outcome: blocked`
again -- same as before B's first run. Parent transitioned back to
`plan_and_await`.

`[GAP: The scheduler outcome schema needs a "deleted" or "respawned"
list. Decision 5.4 says "atomically deletes the synthetic skipped
child state file so the next scheduler tick re-classifies the task as
NotYetSpawned". That delete happens WITHIN the retry tick, but the
outcome vocabulary as documented in walkthrough.md only has spawned /
already / blocked / skipped. Without a "deleted" entry, the agent sees
D disappear from children[] (or flip to state: null) with no
explanation. Recommend adding a "respawned" or "cleared" list to
SchedulerOutcome for this case.]`

**AGENT probe:** What if I submit retry_failed twice in a row without
a `koto next coord` in between? Like, immediately after this I send
another retry for C. What happens?

`[GAP: Decision 5.4 says retry_failed is intercepted in handle_next
BEFORE advance_until_stop, and then a clearing {"retry_failed": null}
event is appended. Each handle_next call is one CLI invocation -- there
is no "in between". A second invocation is a second handle_next call,
which will see no unconsumed retry_failed (already cleared by the
first) and then see the new retry_failed in the submitted evidence.
But: the second submission targets children that may not be in a
retriable state (C is success, not failure). The design doesn't say
what happens when retry_failed.children includes a non-failed,
non-skipped child. Error? Ignore? Rewind anyway? Recommend adding an
explicit rule: "retry_failed.children entries that are not in a
failure or skipped state are rejected with BatchError::InvalidRetry,
the handler is NOT invoked, and no clearing event is appended."]`

### Turn 12 -- B runs again, with a twist

**AGENT probe:** If B's NEW epoch also fails, does D get skipped twice?
What's the state file look like for D across the two skips?

`[GAP: D's state file was DELETED in turn 11 (per Decision 5.4's
delete-and-respawn path). If B fails again on the new epoch, the next
scheduler tick will re-synthesize D's skipped marker -- a fresh state
file. So "twice" is really "first time again". But the parent's event
log has history of BOTH D skips via the sequence of EvidenceSubmitted
and the no-longer-present state files, which means audit trail for D
across retries is lost (the deleted state file is gone). Decision 5.4
acknowledges this via "delete-and-respawn" but doesn't flag the
observability loss. Recommend: before deletion, copy the old state
file to a sibling .retired-<epoch> path, or append an event to the
PARENT log recording "synthesized_skip_cleared: coord.issue-D" so the
history isn't lost.]`

Proceeding: B this time succeeds.

**AGENT:**
```
$ koto next coord.issue-B
```

**KOTO:** working-state evidence_required (fresh epoch).

**AGENT:**
```
$ koto next coord.issue-B --with-data '{"status": "complete"}'
```

**KOTO:**
```json
{
  "action": "done",
  "state": "done",
  "directive": "Issue #202 implemented successfully.",
  "is_terminal": true
}
```

### Turn 13 -- Re-tick, D spawns for real this time

**AGENT:**
```
$ koto next coord
```

**KOTO:**
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
      "total": 4, "completed": 3, "pending": 1,
      "success": 3, "failed": 0, "skipped": 0, "blocked": 0,
      "all_complete": false,
      "children": [
        {"name": "coord.issue-A", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-B", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-C", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.issue-D", "state": "working", "complete": false, "outcome": "pending"}
      ]
    }
  }],
  "scheduler": {
    "spawned": ["coord.issue-D"],
    "already": ["coord.issue-A", "coord.issue-B", "coord.issue-C"],
    "blocked": [],
    "skipped": []
  }
}
```

**AGENT probe:** Wait -- what if I'd done the retry differently and
NOT included D (`include_skipped: false`)? D's skipped_marker state
file would still be on disk. Then B succeeds. Does the scheduler now
see B as success but D as skipped, and count the batch as "done with
one skip" (all_complete: true)? That seems wrong -- the whole point of
retrying B was to un-block D.

`[GAP: Decision 5.4 item 2 says "include_skipped: true extends the set
to include dependents that were skipped because of a failure in the
retry set". If include_skipped is false (default), the design does not
say what happens on the next scheduler tick when the retried failure
now succeeds but the skipped dependents remain on disk. Reading
literally, classify_task would see D as Skipped (skipped_marker on
disk), B as Terminal-success, and the batch would report
all_complete=true, success=3, skipped=1. That is almost certainly not
what the agent wants. Recommend: the scheduler should DETECT this case
(a skipped task whose dependency chain is now all-success) and either
(a) auto-clear the skipped marker and re-materialize, or (b) report a
new outcome enum value "stale_skip" with a suggested action, or (c)
document that include_skipped: true is the default and false is for
advanced users who know what they're doing. Whichever resolution, the
default behavior should not leave stale skips unreachable.]`

### Turn 14 -- Drive D, finish batch

**AGENT:**
```
$ koto next coord.issue-D --with-data '{"status": "complete"}'
```

**KOTO:**
```json
{
  "action": "done",
  "state": "done",
  "directive": "Issue #204 implemented successfully.",
  "is_terminal": true
}
```

**AGENT:**
```
$ koto next coord
```

**KOTO:**
```json
{
  "action": "done",
  "state": "summarize",
  "directive": "All issues are complete. Write a summary of what was implemented.",
  "is_terminal": true
}
```

Batch complete after retry. All 4 issues at terminal-success.

---

## Section 2: Findings

### Finding 1 -- Retry unreachable from canonical template

- **Observation:** The `coord.md` template in walkthrough.md has a
  single transition `plan_and_await -> summarize` on
  `gates.done.all_complete: true`. With Decision 5.3's tightened
  `all_complete = pending==0 AND blocked==0`, this fires on batches
  with failures and slams the parent into a terminal state. The
  `retry_failed` evidence action described in Decision 5.4 has no
  state in the canonical template that would accept it.
- **Location in design:** Decision 5.4 (retry mechanics) intersecting
  with Decision 1 (single-state fan-out) and the canonical template
  in walkthrough.md.
- **Severity:** blocker.
- **Proposed resolution:** Ship the canonical template with two
  transitions -- one to `analyze_failures` guarded on
  `failed + skipped > 0`, one to `summarize` guarded on
  `failed + skipped == 0`. This requires either (a) exposing
  `gates.done.failed` and `gates.done.skipped` as transition-guard
  reachable fields (they're in the structured output but the template
  DSL for `when:` clauses needs to accept them), or (b) a synthetic
  aggregate field like `gates.done.all_success: true`. Update
  walkthrough.md and the reference template in lockstep.

### Finding 2 -- `retry_failed` is discoverable only via skill docs

- **Observation:** Decision 5.4 makes `retry_failed` a reserved
  top-level evidence key AND prohibits it from appearing in the
  template's `accepts` block. The `expects.fields` response therefore
  cannot mention it. An agent without the koto-user skill loaded sees
  no indication this action exists.
- **Location in design:** Decision 5.4 paragraph 1, interacting with
  Decision 7 (directive + details as the guidance surface) and
  Decision 8 (item_schema generation for tasks).
- **Severity:** should-fix.
- **Proposed resolution:** Have `derive_expects` emit a synthetic
  section for reserved actions when the current state has a
  `materialize_children` hook and the gate reports
  `failed + skipped > 0`. Example:
  `"reserved_actions": {"retry_failed": {"schema": {"children": "array<string>", "include_skipped": "boolean"}}}`.
  Keep the accepts-block prohibition intact.

### Finding 3 -- `all_complete: true` with failures is ambiguous

- **Observation:** Decision 5.3 says `all_complete == pending==0 AND
  blocked==0`, so a batch with 1 success and 3 failures is
  `all_complete: true`. The name misleads readers of the gate output
  to think "the batch succeeded." The transition-guard semantics
  depend on this naming being right.
- **Location in design:** Decision 5.3.
- **Severity:** should-fix.
- **Proposed resolution:** Either rename to `all_terminal: true`
  (accurate) or add a paired `all_success: true` field that template
  authors will typically want in their guard expressions. Document
  the distinction prominently.

### Finding 4 -- Scheduler outcome vocabulary insufficient for retry

- **Observation:** The `SchedulerOutcome` response fields documented
  in walkthrough.md are `spawned / already / blocked / skipped`. The
  retry path (Decision 5.4) deletes skipped children's state files
  and re-runs the scheduler, but there's no vocabulary for "deleted
  this tick" or "respawned after skip-marker cleared". On retry tick,
  D disappears from prior `already` list and reappears as `blocked`
  with no explanation of what happened.
- **Location in design:** Decision 5.4 step 3 (delete-and-respawn),
  relative to the outcome schema in Decision 6 / walkthrough.md.
- **Severity:** should-fix.
- **Proposed resolution:** Add `"cleared": [names...]` to
  `SchedulerOutcome` -- children whose synthetic skip markers were
  deleted this tick. Document that a cleared entry will reappear in
  `blocked` or `spawned` (depending on dependency state) on the same
  tick.

### Finding 5 -- Child terminal response hides `failure: true`

- **Observation:** When a child terminates via a `failure: true`
  state, the response JSON is `{"action": "done", "state":
  "done_blocked", "is_terminal": true}`. No flag indicates this was a
  failure. The agent has to either know the child template's schema
  or parse state names.
- **Location in design:** Decision 5.1 introduces `failure: bool` on
  `TemplateState`, but there is no response-shape change to surface
  it.
- **Severity:** should-fix.
- **Proposed resolution:** Add `"failure": true` to the terminal
  response when the state has `failure: true`. Additive, backward
  compatible. Mirrors how `skipped_because` is surfaced in the
  gate output.

### Finding 6 -- `pending` counter semantics inconsistent across examples

- **Observation:** In walkthrough.md Interaction 3 (3-task batch, 1
  running, 2 blocked): `pending: 3, blocked: 2`. This implies pending
  includes blocked. But Decision 5.3's invariant is `all_complete ==
  pending==0 AND blocked==0`, which would be redundant if pending
  already included blocked. Both cannot be true.
- **Location in design:** Decision 5.3 children-complete output
  schema, vs the example responses in walkthrough.md.
- **Severity:** should-fix.
- **Proposed resolution:** Pick one: either `pending` = strictly
  non-terminal not-blocked (Decision 5.3 reads this way), or
  `pending` = all non-terminal (walkthrough examples read this way).
  Fix every response example to match. Personally prefer `pending`
  and `blocked` as disjoint, with a derived `not_complete = pending +
  blocked` if needed; the invariant then simplifies to
  `all_complete == not_complete==0`.

### Finding 7 -- `failure_reason` source and default unclear in practice

- **Observation:** Decision 6 says `reason` is pulled from a context
  key `failure_reason` that the child writes before entering a
  terminal-failure state, defaulting to the state name if unset. The
  reference `impl-issue.md` template does not write `failure_reason`
  anywhere -- it only accepts `status: {complete, blocked}`. So in
  the canonical example, `reason` will always be `"done_blocked"`,
  which tells the observer nothing.
- **Location in design:** Decision 6 (Source of the reason field),
  interacting with the reference child template.
- **Severity:** should-fix.
- **Proposed resolution:** Either (a) update the reference
  `impl-issue.md` to demonstrate writing `failure_reason` via a
  `reason` field on `accepts` when `status == "blocked"`, or (b)
  auto-capture the triggering evidence value on failure transitions
  (e.g., write `failure_reason: "blocked"` when status=blocked
  triggers done_blocked). Option (a) is more in line with the
  "template author is responsible" stance of Decision 6.

### Finding 8 -- Synthetic skipped children leave observability holes

- **Observation:** `koto status coord.issue-D` on a synthetic-skipped
  child returns a response shaped against the synthetic template, not
  the parent's configured child template. If the user later deletes
  the skip marker via retry (Decision 5.4), the synthetic state file
  is gone forever and any audit trail of the skip is lost.
- **Location in design:** Decision 5.2 (synthetic state file) and
  Decision 5.4 (delete-and-respawn).
- **Severity:** nice-to-have (correct behavior, poor observability).
- **Proposed resolution:** Before deletion, append an event to the
  PARENT event log recording
  `{"event": "skip_cleared", "child": "coord.issue-D", "prior_reason": "coord.issue-B"}`.
  This preserves history even after delete-and-respawn, and
  `koto status coord` (parent) can reconstruct the full skip/retry
  timeline without depending on the deleted child's state file.

### Finding 9 -- Stale skip markers on partial retry (`include_skipped: false`)

- **Observation:** If the agent submits
  `retry_failed: {children: ["coord.issue-B"], include_skipped: false}`
  and B succeeds on retry, D's skipped_marker state file is still on
  disk. The next scheduler tick classifies D as `Skipped` per
  Decision 5 ("child exists and has skipped_marker: true"), and the
  batch reports `all_complete: true, skipped: 1` -- even though D's
  dependency chain is now fully successful.
- **Location in design:** Decision 5.4 item 2 and the task
  classification rules in the Data Flow section.
- **Severity:** should-fix.
- **Proposed resolution:** Either (a) make `include_skipped: true`
  the default (safest, least surprising), or (b) on each scheduler
  tick, scan skipped children and auto-clear any whose failing
  dependency now shows Terminal-success, emitting a `cleared` outcome
  (see Finding 4). Option (a) is simpler and matches the documented
  mental model of "retry the failed ones" (Decision 5.4 rationale).

### Finding 10 -- `spawned` vs `skipped` overlap in scheduler outcome

- **Observation:** In Turn 8 (D synthesized as skipped), D appears in
  both `scheduler.spawned` and `scheduler.skipped`. The design uses
  `init_state_file` for both real spawns and skipped synthesis, but
  the outcome vocabulary doesn't distinguish them. Agents reading
  "spawned" may try to `koto next coord.issue-D` expecting a working
  child -- they'd get a terminal response instead.
- **Location in design:** Decision 5.2 and the scheduler outcome
  schema.
- **Severity:** nice-to-have.
- **Proposed resolution:** Rename or split: `spawned_active` for real
  children ready to drive; `spawned_skipped` (or `skipped`, the
  existing field) for synthetic markers. Remove overlap so each
  child appears in exactly one bucket per tick.

### Finding 11 -- `retry_failed` validation on non-failed children is silent

- **Observation:** Decision 5.4 says the handler computes the
  transitive closure and rewinds every child in the set, but does
  not specify behavior when the retry set includes a currently-
  successful or currently-pending child. Rewinding a successful
  child would undo valid work.
- **Location in design:** Decision 5.4 item 3.
- **Severity:** should-fix.
- **Proposed resolution:** Explicitly reject `retry_failed` whose
  `children` list contains any entry that is not currently in a
  Failed or Skipped state. Return `BatchError::InvalidRetry` with
  a per-child classification so the agent can fix the submission.
  Do NOT append the clearing event if validation fails; the
  evidence remains unconsumed and the next tick surfaces the error
  as a gate-blocked response with an error field.
