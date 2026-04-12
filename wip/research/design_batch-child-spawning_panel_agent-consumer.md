# Agent-Consumer Review: Batch Child Spawning Design

**Perspective:** AI agent driving a batch workflow using only the
koto-user skill and `koto next` response data. The question is
whether the design produces responses I can act on without
hardcoded knowledge of the batch protocol.

---

## 1. Can I parse the batch response?

Looking at Interaction 3 in the walkthrough (after task list
submission), the response is `action: gate_blocked` with two new
pieces of data: a `scheduler` object and an extended
`blocking_conditions[0].output`.

**(a) Can I tell children were spawned?**

Yes. `scheduler.spawned` is an explicit list of child workflow
names. This is unambiguous. The field is `null` when no scheduler
ran (non-batch states), and an object with `spawned`, `already`,
`blocked`, `skipped` arrays when the scheduler did run. I can
distinguish "no batch" (`scheduler: null`) from "batch ran but
nothing to spawn" (`scheduler.spawned: []`).

**(b) Which ones to drive?**

`scheduler.spawned` tells me what was just created. I drive those.
But there is a subtlety: what about children that were spawned on
a previous tick and are still running? They appear in
`scheduler.already` and in `blocking_conditions[0].output.children`
with `outcome: "pending"`. I know they exist, but the skill
doesn't tell me whether I should check on them, re-drive them, or
leave them alone. The directive prose would need to cover this --
and in the walkthrough template it does -- but from the JSON alone,
I'd need to infer that `already` means "already spawned, don't
re-spawn."

**(c) When to re-tick the parent?**

The walkthrough says "after any child completes, re-check the
parent." The directive prose says the same. But structurally, there
is no field in the child's `workflow_complete` response that says
"now go re-tick your parent." The child doesn't know it has a
parent (from the response perspective). The agent must remember
the parent-child relationship on its own. This is acceptable
because the agent submitted the task list and knows the parent
name, but it's worth noting that the protocol doesn't provide a
"return to parent" signal.

**What's ambiguous:** The `scheduler` field is entirely new and
not documented in the koto-user skill's response-shapes.md. An
agent with only the current skill loaded would not know what
`scheduler` is, what its sub-fields mean, or that it even exists.
The field name itself is reasonably self-documenting, but
`already` vs `spawned` vs `blocked` semantics require
documentation.

---

## 2. Is the `item_schema` in `expects` useful?

Yes, significantly. The `item_schema` tells me:

- `name` is required (string)
- `template` is optional with a specific default
- `vars` is an optional object
- `waits_on` is an optional array defaulting to `[]`
- `trigger_rule` is optional, defaults to `all_success`

This is enough to construct a valid task list entry without
reading the directive prose at all. The schema is self-describing
in the same way `enum` fields include `values`. I would use both:
the directive tells me *what* to put in the fields (e.g., "map
issue N to name issue-N"), and the schema tells me the *shape*.

One gap: `vars` has `type: "object"` but no indication of what
keys the child template expects. The agent would need to either
read the child template directly or rely on the directive prose to
know that `ISSUE_NUMBER` is a required variable. This is
inherent to the design -- koto can't know what arbitrary child
templates require at the parent's compile time -- but it means
the `item_schema` alone is not sufficient for producing
semantically correct (vs structurally valid) task entries.

The `trigger_rule` field appearing in the schema but being
restricted to `all_success` at runtime is a minor confusion risk.
An agent seeing it in the schema might try `one_success` and get
a runtime error. The schema doesn't communicate "reserved, don't
use." A `description` field on `trigger_rule` saying "reserved;
only all_success accepted" would help.

---

## 3. Do I know when I'm done?

Walking through the full lifecycle with the signals at each step:

**Step: Submit task list.** Response: `action: gate_blocked`,
`scheduler.spawned` lists ready children. Signal is clear: go
drive those children.

**Step: Drive child to completion.** Response from child:
`action: workflow_complete`. Signal is clear: this child is done.

**Step: Re-tick parent.** Response: `action: gate_blocked` with
updated `scheduler.spawned` (newly unblocked children). Signal
is clear: drive the new children.

**Step: All children done, re-tick parent.** Response:
`action: workflow_complete` on the parent. Signal is clear: batch
is done.

At every step, the `action` field tells me the next move. The
lifecycle is clean. There is one edge case worth examining:

**Edge: All children done but some failed.** The walkthrough's
failure scenario shows `all_complete: true` but `failed: 1`. The
parent transitions to `summarize` because the transition condition
is `gates.done.all_complete: true`. The agent sees
`workflow_complete` on the parent. But what if the template
routes to an analysis state instead? Then the agent sees
`evidence_required` with presumably a `retry_failed`-related
directive. This transition depends entirely on how the template
author wired it -- the protocol itself is clear, but the agent
needs the directive to know whether to retry or accept partial
success.

The signal chain is unambiguous at every step. No step leaves the
agent guessing about what to do next.

---

## 4. Failure handling from the agent's perspective

**Scenario: child returns `workflow_complete` with state
`done_blocked`.** The child response is:

```json
{
  "action": "workflow_complete",
  "state": "done_blocked",
  "is_terminal": true
}
```

I know the child is done. I know to re-tick the parent (from my
memory of the batch protocol, not from this response). When I
re-tick the parent, I get `gate_blocked` with the child listed as
`outcome: "failure"`. This is clear.

**Do I know about `retry_failed`?**

No. The current koto-user skill has zero mention of `retry_failed`.
The `response-shapes.md` reference doesn't document it. The
`command-reference.md` doesn't document it. The SKILL.md doesn't
mention it. If a template routes to an analysis state and the
directive says "submit retry_failed evidence," I'd follow the
directive -- but I wouldn't know the exact JSON shape without the
directive spelling it out.

The design says `retry_failed` is a reserved evidence key like
`gates`. This means it bypasses the normal `accepts` schema. An
agent used to the pattern "read `expects.fields`, submit matching
JSON" would not know how to produce a `retry_failed` payload
because it wouldn't appear in `expects`. The directive prose is
the only guidance path.

**What would I do if a child fails and the template doesn't
mention retry?** I'd re-tick the parent, see the failure in the
blocking conditions, read the directive, and follow whatever the
template author wrote. If the template just transitions to
`summarize` with `all_complete: true`, I'd proceed to
summarization. The protocol handles this fine -- the agent
doesn't need to know about retry if the template doesn't offer it.

**Where would I learn about retry?** Today, nowhere in the skill.
The design doc's walkthrough mentions it briefly. The template's
directive prose is the intended channel. This is consistent with
Decision 7's philosophy (directive + details carry the behavioral
guidance), but it does mean an agent's ability to retry depends
entirely on the template author writing good prose.

---

## 5. What's missing from the koto-user skill?

### SKILL.md gaps

1. **No mention of `scheduler` field.** The action dispatch table
   and the evidence_required/gate_blocked handling sections don't
   mention that responses can include a `scheduler` object. The
   Hierarchy section talks about `children-complete` temporal
   blocks but not about batch spawning or the scheduler outcome.

2. **No batch lifecycle guidance.** The three-step pattern (init,
   action loop, completion) doesn't cover the batch variant:
   submit task list, drive children, re-tick parent, repeat. The
   Hierarchy section says "koto doesn't launch child agents -- you
   do that yourself" but doesn't describe the batch-specific
   submit-spawn-drive-retick loop.

3. **No `retry_failed` documentation.** The reserved evidence key
   is entirely absent.

4. **The `children-complete` gate output in the Hierarchy section
   is outdated.** It shows `total`, `completed`, `pending`,
   `all_complete`, `children[]` with `name`, `state`, `complete`.
   It's missing the new fields: `success`, `failed`, `skipped`,
   `blocked`, and per-child `outcome`, `failure_mode`,
   `skipped_because`, `blocked_by`.

5. **`@file` prefix for `--with-data` is not documented.** The
   command reference shows `--with-data '<json>'` but not
   `--with-data @file.json`.

6. **`type: tasks` in expects is not documented.** The expects
   handling in SKILL.md only covers `enum`, `string`, etc. A
   `tasks`-typed field with `item_schema` is new and needs
   explanation.

### response-shapes.md gaps

7. **No scenario for batch gate_blocked.** Scenario (j) covers
   `children-complete` temporal blocks but uses the v0.7.0 output
   schema (no `outcome`, no `success`/`failed`/`skipped`/`blocked`
   aggregates, no `scheduler` field). A new scenario or an update
   to (j) is needed showing the batch-extended output plus the
   `scheduler` attachment.

8. **No scenario for `workflow_complete` with failure.** The `done`
   scenario (h) shows a clean completion. A child reaching a
   terminal-failure state (`failure: true`) still returns
   `action: "done"` (per the walkthrough it shows
   `action: "workflow_complete"` -- this action name difference
   itself is a discrepancy that needs reconciling). The response
   shape for terminal-failure should be documented.

9. **`item_schema` sub-object is not documented.** The expects
   schema section describes `fields`, `event_type`, and `options`.
   A `tasks`-typed field's `item_schema` key is new and needs an
   annotated example.

### command-reference.md gaps

10. **`koto status` output doesn't show the `batch` section.**
    Decision 6 adds an optional `batch` object to status output.
    The command reference shows only `name`, `current_state`,
    `template_path`, `template_hash`, `is_terminal`.

11. **`koto workflows --children` output doesn't show batch
    metadata.** Decision 6 adds per-row `task_name`, `waits_on`,
    `reason_code`, `reason`, `skip_reason`.

---

## Action name discrepancy

The walkthrough uses `action: "workflow_complete"` for terminal
states. The current skill documents `action: "done"`. These must
be reconciled -- either the design is introducing a new action
name, or the walkthrough is using the wrong name. If
`workflow_complete` is the real action name post-batch, the entire
action dispatch table in SKILL.md needs updating. If `done` is
still correct, the walkthrough should be fixed. This is a
potential source of serious agent confusion.

---

## Top 3 issues

1. **The `scheduler` field is invisible to agents.** It's the
   primary signal for "what children to drive" and it appears in
   zero skill documents. An agent with only the koto-user skill
   loaded would not know the field exists, what its sub-fields
   mean, or how to use it to drive children. Without this, batch
   workflows require the agent to rely entirely on directive
   prose -- defeating the design's structured-data value
   proposition.

2. **`action: "workflow_complete"` vs `action: "done"` is
   unresolved.** The walkthrough uses one name; the skill
   documents another. If the agent dispatches on `action` (as
   the skill instructs), and the real response uses a different
   string, the agent's dispatch logic breaks. This must be
   reconciled before the batch feature ships.

3. **`retry_failed` has no skill-level documentation path.** An
   agent encountering a failed batch has no structured way to
   learn about retry. The evidence key bypasses `expects`, so the
   agent can't discover it from the response schema. The directive
   prose is the only channel, making retry capability entirely
   dependent on template-author quality. At minimum, the skill
   should document that `retry_failed` is a reserved evidence key
   with a known schema, so agents can recognize and construct it
   when directed.
