# Simulation Round 2, Shape C, Pair 2: Dynamic Additions x Retry Interaction

Scenario: linear A -> B -> C chain, mid-flight additions (A1 parallel, D
tail-extension, E post-retry), with B failing and recovering via
`retry_failed`. Probes the interaction between CD10 (mutation semantics,
union-by-name, `scheduler.feedback.entries`, `orphan_candidates`) and
CD9 (template-routed retry, runtime reclassification, delete-and-respawn
of stale skip markers and invalidated running children).

Parent template: `coord.md` from `wip/walkthrough/walkthrough.md` --
states `plan_and_await`, `analyze_failures`, `summarize`. Child template:
`impl-issue.md`. Parent workflow name: `coord`.

---

## Section 1: Transcript

### Turn 1 -- AGENT

```
koto init coord --template coord.md --var plan_path=PLAN-linear.md
```

### Turn 2 -- KOTO

```json
{
  "action": "initialized",
  "workflow": "coord",
  "state": "plan_and_await",
  "template": "coord.md"
}
```

### Turn 3 -- AGENT

```
koto next coord
```

### Turn 4 -- KOTO

Standard `evidence_required` with `expects.fields.tasks` + `item_schema`
(elided -- identical to walkthrough Interaction 2). `scheduler: null`,
no `reserved_actions` (no failures yet).

### Turn 5 -- AGENT

Submits the initial 3-task linear chain.

```json
{
  "tasks": [
    {"name": "A", "vars": {"ISSUE_NUMBER": "201"}},
    {"name": "B", "vars": {"ISSUE_NUMBER": "202"}, "waits_on": ["A"]},
    {"name": "C", "vars": {"ISSUE_NUMBER": "203"}, "waits_on": ["B"]}
  ]
}
```

```
koto next coord --with-data @tasks-v1.json
```

### Turn 6 -- KOTO

Pre-append validation passes (R0, R3-R6, R8 vacuous, R9). Appends
`EvidenceSubmitted`. Scheduler spawns `coord.A`; B and C are
`BlockedByDep`.

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 3, "completed": 0, "pending": 3,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 2, "spawn_failed": 0,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false, "needs_attention": false,
      "children": [
        {"name": "coord.A", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.B", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.A"]},
        {"name": "coord.C", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.B"]}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": ["coord.A"],
    "materialized_children": [
      {"name": "coord.A", "outcome": "pending", "state": "working"}
    ],
    "already": [], "blocked": ["coord.B", "coord.C"],
    "skipped": [], "errored": [], "warnings": [],
    "feedback": {
      "entries": {
        "A": {"outcome": "accepted"},
        "B": {"outcome": "blocked", "waits_on": ["A"]},
        "C": {"outcome": "blocked", "waits_on": ["B"]}
      },
      "orphan_candidates": []
    }
  }
}
```

### Turn 7-8 -- AGENT drives A

```
koto next coord.A
# evidence_required for status
koto next coord.A --with-data '{"status": "complete"}'
# done
```

### Turn 9 -- AGENT re-ticks parent

```
koto next coord
```

### Turn 10 -- KOTO

A is terminal-success, scheduler spawns B.

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done", "type": "children-complete", "category": "temporal",
    "output": {
      "total": 3, "completed": 1, "pending": 2,
      "success": 1, "failed": 0, "skipped": 0, "blocked": 1, "spawn_failed": 0,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false, "needs_attention": false,
      "children": [
        {"name": "coord.A", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.B", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.C", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.B"]}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": ["coord.B"],
    "materialized_children": [
      {"name": "coord.A", "outcome": "success", "state": "done"},
      {"name": "coord.B", "outcome": "pending", "state": "working"}
    ],
    "already": ["coord.A"], "blocked": ["coord.C"],
    "skipped": [], "errored": [], "warnings": [],
    "feedback": {"entries": {}, "orphan_candidates": []}
  }
}
```

Note: `feedback.entries` is empty on this tick because no `tasks`
payload was submitted -- per CD10, `feedback` keys on the current
submission's short names. On ticks with no submission, the map is
empty. (See Finding 1.)

### Turn 11 -- AGENT (mid-flight discovery)

While `coord.B` is still running, the agent re-reads the plan and
realizes:
- A pre-work task `A1` was missed (no deps, parallelizable).
- A cleanup task `D` is needed after C.

Agent leaves `coord.B` parked and resubmits with the full 5-task set.
Per CD10 R8, already-spawned entries (A, B) must match field-for-field
the `spawn_entry` snapshot. The agent re-emits them verbatim.

```json
{
  "tasks": [
    {"name": "A",  "vars": {"ISSUE_NUMBER": "201"}},
    {"name": "B",  "vars": {"ISSUE_NUMBER": "202"}, "waits_on": ["A"]},
    {"name": "C",  "vars": {"ISSUE_NUMBER": "203"}, "waits_on": ["B"]},
    {"name": "A1", "vars": {"ISSUE_NUMBER": "204"}},
    {"name": "D",  "vars": {"ISSUE_NUMBER": "205"}, "waits_on": ["C"]}
  ]
}
```

```
koto next coord --with-data @tasks-v2.json
```

### Turn 12 -- KOTO

R8 per-entry check: A and B already on disk -- entries match their
`spawn_entry` -> pass. C is spawned? No -- C has no state file yet
(was `blocked` only). For un-spawned names, R8 is vacuous. A1 and D
are new names, accepted as-is. R3 (cycle) passes. R4 (dangling)
passes.

`EvidenceSubmitted` appended. Scheduler classifies:
- A: Terminal (success)
- B: Running
- C: Blocked on B
- A1: Ready (no deps) -> spawn
- D: Blocked on C

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done", "type": "children-complete", "category": "temporal",
    "output": {
      "total": 5, "completed": 1, "pending": 4,
      "success": 1, "failed": 0, "skipped": 0, "blocked": 2, "spawn_failed": 0,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false, "needs_attention": false,
      "children": [
        {"name": "coord.A",  "state": "done",    "complete": true,  "outcome": "success"},
        {"name": "coord.B",  "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.A1", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.C",  "state": null,      "complete": false, "outcome": "blocked", "blocked_by": ["coord.B"]},
        {"name": "coord.D",  "state": null,      "complete": false, "outcome": "blocked", "blocked_by": ["coord.C"]}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": ["coord.A1"],
    "materialized_children": [
      {"name": "coord.A",  "outcome": "success", "state": "done"},
      {"name": "coord.B",  "outcome": "pending", "state": "working"},
      {"name": "coord.A1", "outcome": "pending", "state": "working"}
    ],
    "already": ["coord.A", "coord.B"],
    "blocked": ["coord.C", "coord.D"],
    "skipped": [], "errored": [], "warnings": [],
    "feedback": {
      "entries": {
        "A":  {"outcome": "already"},
        "B":  {"outcome": "already"},
        "C":  {"outcome": "blocked", "waits_on": ["B"]},
        "A1": {"outcome": "accepted"},
        "D":  {"outcome": "blocked", "waits_on": ["C"]}
      },
      "orphan_candidates": []
    }
  }
}
```

**What this tells the agent:** dynamic additions landed. Every
submitted entry has an explicit feedback outcome -- no silent drops.
A1 is live; C and D are queued.

### Turn 13 -- AGENT drives A1 in parallel with B

A1 completes successfully.

```
koto next coord.A1 --with-data '{"status": "complete"}'
# done
```

B fails: the agent hits an unrecoverable blocker.

```
koto next coord.B --with-data '{"status": "blocked"}'
```

### Turn 14 -- KOTO (child B)

```json
{
  "action": "done",
  "state": "done_blocked",
  "directive": "Issue #202 is blocked and cannot proceed.",
  "is_terminal": true
}
```

`coord.B` terminal with `failure: true`. `failure_reason` is written
by `default_action.context_assignments`.

### Turn 15 -- AGENT re-ticks parent

```
koto next coord
```

### Turn 16 -- KOTO

B is terminal-failure. With `failure_policy: skip_dependents`, the
scheduler materializes skip markers for B's transitive closure on the
task DAG: C (direct), and D (via C, transitive). Both are
delete-and-respawn into `impl-issue.md`'s `skipped_due_to_dep_failure`
terminal state (CD9 Part 5). `BatchFinalized` is appended because
`all_complete: true` now holds. Aggregate `needs_attention: true`
fires the transition to `analyze_failures`.

```json
{
  "action": "evidence_required",
  "state": "analyze_failures",
  "directive": "At least one child failed or was skipped. Inspect the batch view in the response or in `koto status coord`. Submit retry_failed to re-queue, or {\"decision\": \"give_up\"} / {\"decision\": \"acknowledge\"} to route to summarize.",
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "decision": {"type": "enum", "values": ["give_up", "acknowledge"], "required": false}
    }
  },
  "blocking_conditions": [{
    "name": "done", "type": "children-complete", "category": "temporal",
    "output": {
      "total": 5, "completed": 5, "pending": 0,
      "success": 2, "failed": 1, "skipped": 2, "blocked": 0, "spawn_failed": 0,
      "all_complete": true, "all_success": false,
      "any_failed": true, "any_skipped": true, "needs_attention": true,
      "children": [
        {"name": "coord.A",  "state": "done",                         "complete": true, "outcome": "success"},
        {"name": "coord.A1", "state": "done",                         "complete": true, "outcome": "success"},
        {"name": "coord.B",  "state": "done_blocked",                 "complete": true, "outcome": "failure", "failure_mode": true, "reason": "Issue 202 hit an unresolvable blocker during implementation.", "reason_source": "failure_reason"},
        {"name": "coord.C",  "state": "skipped_due_to_dep_failure",   "complete": true, "outcome": "skipped", "skipped_because": "coord.B", "skipped_because_chain": ["coord.B"]},
        {"name": "coord.D",  "state": "skipped_due_to_dep_failure",   "complete": true, "outcome": "skipped", "skipped_because": "coord.C", "skipped_because_chain": ["coord.C", "coord.B"]}
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
      "applies_to": ["coord.B", "coord.C", "coord.D"],
      "invocation": "koto next coord --with-data '{\"retry_failed\": {\"children\": [\"coord.B\"]}}'"
    }
  ],
  "scheduler": null
}
```

Note: `skipped_because_chain` for D correctly walks upstream through
`waits_on` to the failed ancestor (CD13). `reserved_actions[0].applies_to`
lists all retryable children including the transitively-skipped D.

### Turn 17 -- AGENT submits retry for B only

Per CD9 Part 4, `include_skipped: true` (default) closes the retry
downward -- naming B re-queues C and D.

```
koto next coord --with-data '{"retry_failed": {"children": ["coord.B"]}}'
```

### Turn 18 -- KOTO

R10 validates: `coord.B` exists, outcome `failure`, eligible. No mixed
payload. `handle_retry_failed` runs:

1. Append `EvidenceSubmitted { retry_failed: {...} }` to coord.
2. Append clearing `EvidenceSubmitted { retry_failed: null }`.
3. Under cloud sync, push both before touching children (CD12 Q6).
4. Downward closure of B = {B, C, D}. For each:
   - `coord.B` outcome `failure` -> append `Rewound` targeting
     `working`. The child's state file is rewound in place.
   - `coord.C` outcome `skipped` + `skipped_marker: true` -> delete
     the child state file, then re-spawn from the task entry
     (delete-and-respawn). C was previously respawned as skip via
     runtime reclassification; the task entry is still on the parent's
     merged task set (still in the most-recent `EvidenceSubmitted.tasks`
     from Turn 11), so respawn uses that entry. But C's `waits_on:
     ["B"]` is still unsatisfied (B is now pending again) -- so the
     respawn actually classifies C as `BlockedByDep`, and the
     scheduler does NOT materialize a new state file for C; it just
     removes the stale skip marker. (Contrast with D below.)
   - `coord.D` outcome `skipped` -> delete skip marker; D was
     transitively skipped. Its `waits_on: ["C"]` is unsatisfied.
     Scheduler removes the skip marker; D returns to `BlockedByDep`.

Advance loop: at `analyze_failures`. Transition with
`when: evidence.retry_failed: present` matches the un-merged payload
-> `plan_and_await`.

Scheduler runs on `plan_and_await`. B is now `Running` (rewound to
`working`). C and D have no state files (deleted). Nothing to spawn
this tick.

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done", "type": "children-complete", "category": "temporal",
    "output": {
      "total": 5, "completed": 2, "pending": 3,
      "success": 2, "failed": 0, "skipped": 0, "blocked": 2, "spawn_failed": 0,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false, "needs_attention": false,
      "children": [
        {"name": "coord.A",  "state": "done",    "complete": true,  "outcome": "success"},
        {"name": "coord.A1", "state": "done",    "complete": true,  "outcome": "success"},
        {"name": "coord.B",  "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.C",  "state": null,      "complete": false, "outcome": "blocked", "blocked_by": ["coord.B"]},
        {"name": "coord.D",  "state": null,      "complete": false, "outcome": "blocked", "blocked_by": ["coord.C"]}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": [],
    "materialized_children": [
      {"name": "coord.A",  "outcome": "success", "state": "done"},
      {"name": "coord.A1", "outcome": "success", "state": "done"},
      {"name": "coord.B",  "outcome": "pending", "state": "working"}
    ],
    "already": ["coord.A", "coord.A1", "coord.B"],
    "blocked": ["coord.C", "coord.D"],
    "skipped": [], "errored": [], "warnings": [],
    "feedback": {"entries": {}, "orphan_candidates": []}
  }
}
```

**What this tells the agent:** retry rewound B; stale skip markers for
C and D are gone (they don't appear in `materialized_children`); their
`blocked_by` shows they're waiting again. Drive B.

### Turn 19 -- AGENT drives B to success this time

```
koto next coord.B --with-data '{"status": "complete"}'
# done
koto next coord
```

### Turn 20 -- KOTO

B terminal-success. Scheduler sees C is `Ready` (B satisfied); spawns
C. D still blocked on C.

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done", "type": "children-complete", "category": "temporal",
    "output": {
      "total": 5, "completed": 3, "pending": 2,
      "success": 3, "failed": 0, "skipped": 0, "blocked": 1, "spawn_failed": 0,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false, "needs_attention": false,
      "children": [
        {"name": "coord.A",  "state": "done",    "complete": true,  "outcome": "success"},
        {"name": "coord.A1", "state": "done",    "complete": true,  "outcome": "success"},
        {"name": "coord.B",  "state": "done",    "complete": true,  "outcome": "success"},
        {"name": "coord.C",  "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.D",  "state": null,      "complete": false, "outcome": "blocked", "blocked_by": ["coord.C"]}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": ["coord.C"],
    "materialized_children": [
      {"name": "coord.A",  "outcome": "success", "state": "done"},
      {"name": "coord.A1", "outcome": "success", "state": "done"},
      {"name": "coord.B",  "outcome": "success", "state": "done"},
      {"name": "coord.C",  "outcome": "pending", "state": "working"}
    ],
    "already": ["coord.A", "coord.A1", "coord.B"],
    "blocked": ["coord.D"],
    "skipped": [], "errored": [], "warnings": [],
    "feedback": {"entries": {}, "orphan_candidates": []}
  }
}
```

### Turn 21 -- AGENT (mid-late addition: E)

While C is running, the agent discovers one more task: `E` that waits
on D. Submit the 6-task list.

```json
{
  "tasks": [
    {"name": "A",  "vars": {"ISSUE_NUMBER": "201"}},
    {"name": "B",  "vars": {"ISSUE_NUMBER": "202"}, "waits_on": ["A"]},
    {"name": "C",  "vars": {"ISSUE_NUMBER": "203"}, "waits_on": ["B"]},
    {"name": "A1", "vars": {"ISSUE_NUMBER": "204"}},
    {"name": "D",  "vars": {"ISSUE_NUMBER": "205"}, "waits_on": ["C"]},
    {"name": "E",  "vars": {"ISSUE_NUMBER": "206"}, "waits_on": ["D"]}
  ]
}
```

```
koto next coord --with-data @tasks-v3.json
```

### Turn 22 -- KOTO

R8 passes: A, A1, B all spawned -- entries match. C is now spawned
too (Turn 20); entry still matches. D was delete-and-respawned in
Turn 18, then is currently *un-spawned* (blocked, no state file) --
so R8 is vacuous. E is new.

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done", "type": "children-complete", "category": "temporal",
    "output": {
      "total": 6, "completed": 3, "pending": 3,
      "success": 3, "failed": 0, "skipped": 0, "blocked": 2, "spawn_failed": 0,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false, "needs_attention": false,
      "children": [
        {"name": "coord.A",  "state": "done",    "complete": true,  "outcome": "success"},
        {"name": "coord.A1", "state": "done",    "complete": true,  "outcome": "success"},
        {"name": "coord.B",  "state": "done",    "complete": true,  "outcome": "success"},
        {"name": "coord.C",  "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.D",  "state": null,      "complete": false, "outcome": "blocked", "blocked_by": ["coord.C"]},
        {"name": "coord.E",  "state": null,      "complete": false, "outcome": "blocked", "blocked_by": ["coord.D"]}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": [],
    "materialized_children": [
      {"name": "coord.A",  "outcome": "success", "state": "done"},
      {"name": "coord.A1", "outcome": "success", "state": "done"},
      {"name": "coord.B",  "outcome": "success", "state": "done"},
      {"name": "coord.C",  "outcome": "pending", "state": "working"}
    ],
    "already": ["coord.A", "coord.A1", "coord.B", "coord.C"],
    "blocked": ["coord.D", "coord.E"],
    "skipped": [], "errored": [], "warnings": [],
    "feedback": {
      "entries": {
        "A":  {"outcome": "already"},
        "B":  {"outcome": "already"},
        "C":  {"outcome": "already"},
        "A1": {"outcome": "already"},
        "D":  {"outcome": "blocked", "waits_on": ["C"]},
        "E":  {"outcome": "blocked", "waits_on": ["D"]}
      },
      "orphan_candidates": []
    }
  }
}
```

**What this tells the agent:** the 6th task was accepted. The full
task set is feedback-visible (6 entries). The `materialized_children`
ledger (CD12 Q1) is the authoritative "what has a state file" view;
E is in `blocked`, not in the ledger.

### Turn 23-25 -- Agent drives C, then D, then E in sequence

Each re-ticks the parent after completion. C spawns D; D spawns E;
E completes.

### Turn 26 -- KOTO (final)

All 6 success. `BatchFinalized` event appended (superseding the prior
one from Turn 16). Aggregate `all_success: true` fires transition to
`summarize`.

```json
{
  "action": "done",
  "state": "summarize",
  "directive": "All issues are complete. Write a summary of what was implemented.",
  "is_terminal": true,
  "batch_final_view": {
    "phase": "final",
    "total": 6, "success": 6, "failed": 0, "skipped": 0,
    "all_success": true, "any_failed": false, "any_skipped": false,
    "children": [
      {"name": "coord.A",  "outcome": "success"},
      {"name": "coord.A1", "outcome": "success"},
      {"name": "coord.B",  "outcome": "success"},
      {"name": "coord.C",  "outcome": "success"},
      {"name": "coord.D",  "outcome": "success"},
      {"name": "coord.E",  "outcome": "success"}
    ]
  },
  "reserved_actions": []
}
```

---

## Section 2: Edge case probes

### Probe A -- Identical resubmit during reclassification

> Between Turn 15 (the tick that made B fail) and Turn 17 (agent
> submits retry), an overlapping process tries to resubmit the 5-task
> list verbatim.

Scheduler runtime reclassification (CD9 Part 5) runs inside the tick
that ingests B's failure -- i.e., it's part of the Turn 16 `handle_next`
execution. CD12 Q3 advisory flock on `coord.lock` serializes ticks.
The second submission blocks on flock (non-blocking `LOCK_EX | LOCK_NB`
-> fails fast with `concurrent_tick` / `integration_unavailable`, exit
1 retryable per CD11). The agent retries after Turn 16 completes.

On the retry: all 5 names exist. R8 per-entry check: A/B/A1 all
spawned with matching `spawn_entry`. C and D were delete-and-respawned
as skip markers. R8 compares the new submission's entry to the child's
recorded `spawn_entry` *on the current child state file*. Per CD10,
the `spawn_entry` is written at `WorkflowInitialized` time. Runtime
reclassification deletes the old state file and spawns a new one --
does the new skip-marker state file carry a `spawn_entry`?

**GAP G1: spawn_entry continuity across delete-and-respawn.** CD9 Part 5
describes delete-and-respawn but doesn't specify whether the re-init
reuses the same `spawn_entry` from the task list (yes, it must -- the
task DAG is unchanged) or writes a fresh one (in which case the values
are identical because the merged task set is unchanged). Either way,
R8 should pass because the task entry is the same. But the design
doesn't spell this out; an implementer could skip `spawn_entry` on
skip-marker init and break R8 on the next resubmit.

Assuming `spawn_entry` is always written on every `init_state_file`
(including skip-marker spawns), R8 passes. `feedback.entries` for C
and D reports `already` -- because they have state files on disk, even
if those files are skip markers. This is subtly surprising: the agent
sees `already` on a skip marker and may think "C ran," but it didn't.
The batch view's `outcome: "skipped"` disambiguates, but the scheduler
feedback `outcome: already` conflates "already spawned as a real
running child" with "already materialized as skip marker."

**GAP G2: `EntryOutcome::Already` is ambiguous between running and
skip-marker states.** Agents watching feedback alone cannot tell the
two apart. Fix: add an optional `state: String` or
`kind: "running" | "skipped_marker"` field to `Already`, or document
that feedback is advisory and agents must cross-reference
`materialized_children`.

### Probe B -- Resubmit after skip-marker delete-and-respawn

> After Turn 18 (retry rewound B, deleted skip markers for C and D),
> the agent resubmits the 5-task list. R8 for C and D: they no longer
> have state files. Does the scheduler consider them "new" or bound
> by their prior `spawn_entry`?

Per CD10 R8 literal reading: "For each task entry whose computed
child name already exists on disk as a spawned child..." C and D no
longer exist on disk (deleted in Turn 18). So R8 is vacuous for them
-- the agent could legitimately submit a DIFFERENT entry for C or D
now (change `vars`, change `waits_on`). This is semantically reasonable
for retry scenarios (the agent *should* be able to fix a bad task
entry during the retry round), but the design narrative doesn't flag
it.

**GAP G3: retry as a backdoor for R8 mutation.** After `retry_failed`
closes a skip marker via delete-and-respawn, the affected names become
R8-mutable until the scheduler respawns them. If the agent submits a
mutated entry for C in the same tick as the retry (mixed payload --
but CD9 rejects mixed `retry_failed + tasks`), or in the tick
*immediately after* retry (before the respawn of the cleared name), the
mutation lands. For `impl-issue.md` children the re-respawn is the
very next scheduler classification pass, which happens inside the same
`handle_next` -- so the window is likely zero in practice. But for a
child like D that depends on C, D's respawn waits until C succeeds;
so D's entry is R8-mutable for the entire interval between retry and
C's success. This is a subtle hole in "strict spawn-time immutability."
The design should either document this deliberately or strengthen R8
to compare against the *most-recent* `spawn_entry` across the entire
log history for that name (not just the current state file).

### Probe C -- Add a task that depends on a failed task

> After Turn 16 (B failed, parent at `analyze_failures`) but before
> retry, the agent submits:

```json
{
  "tasks": [
    {"name": "A", "..."}, {"name": "B", "..."}, {"name": "C", "..."},
    {"name": "A1", "..."}, {"name": "D", "..."},
    {"name": "X", "vars": {"ISSUE_NUMBER": "299"}, "waits_on": ["B"]}
  ]
}
```

R0, R3, R4 all pass -- X is new, its dep B exists (as a failed
child). R8 for A/B/C/A1/D: all match. X: new, accepted.

But wait -- parent is at `analyze_failures`, not `plan_and_await`. Can
the agent even submit `tasks` evidence at this state? Per `coord.md`,
`analyze_failures` has `accepts: { decision }` only. `tasks` is not in
the state's accepts schema -> the advance loop rejects as an unknown
field.

**GAP G4: dynamic additions unreachable from `analyze_failures`.** The
only state that accepts `tasks` is `plan_and_await`. Once the batch
fails, the agent cannot grow the task set until after retry (when the
state re-enters `plan_and_await`). This is defensible (you shouldn't
plan new work while recovering) but not documented. Workaround: agent
submits `retry_failed` first to re-enter `plan_and_await`, then
submits `tasks` with X appended on the next tick.

Follow-up: once back in `plan_and_await` (Turn 18 response), submit:

```json
{"tasks": [..., {"name": "X", "waits_on": ["B"]}]}
```

R8 passes (B now pending, matches spawn_entry). X is accepted, blocked
on B. When B succeeds, X becomes `Ready` and spawns.

But what if X had been added BEFORE retry, while B was still failed?
If we hypothetically allowed it: X would be `BlockedByDep` on a
terminal-failure B. Per CD9 runtime reclassification, the scheduler
evaluates skip markers against current dependency outcomes -- but X
has no state file yet (just `blocked` in the classification). Would
the scheduler materialize X as a skip marker immediately (because B is
failed and `failure_policy: skip_dependents`)? Per CD9 Part 5, yes:
the failure-policy cascade materializes skip markers for transitive
dependents on the tick that observes the failure. So X would spawn
directly as a skip marker.

**GAP G5: failure-policy cascade on newly-added tasks.** If
`analyze_failures` DID accept `tasks`, a new task with a failed
ancestor would materialize as a skip marker on the same tick. That's
semantically coherent but non-obvious. G4 (tasks not accepted in
`analyze_failures`) makes G5 moot in practice, but the design should
document the interaction explicitly.

### Probe D -- Rename attempt combined with legit additions

> The agent submits `[A, B-renamed, C, A1, D]` -- B renamed to
> `B-renamed` (same vars, same `waits_on: ["A"]`). B is running.

R8 per-entry: A matches. B is on disk as `coord.B` but the submission
has no `B` entry. CD10 "Removal is deferred" -- omission is a no-op,
so B stays in the effective task set. `B-renamed` is a new name; R8 is
vacuous (no `coord.B-renamed` on disk). C, A1, D: standard.

Orphan detection: `B-renamed`'s signature (`template: impl-issue.md`,
`vars: {ISSUE_NUMBER: "202"}`, `waits_on: ["A"]`) is byte-identical
to `coord.B`'s `spawn_entry`. Scheduler emits
`orphan_candidates: [{new_task: "B-renamed", signature_match: "B",
confidence: "exact", message: "..."}]`.

The submission is accepted. The scheduler spawns `B-renamed` as a new
child (its deps are satisfied -- A is done). Result: TWO running
children doing the same work. The agent sees `orphan_candidates` and
must decide:
- Let both run (wasteful, possibly conflicting).
- Manually delete `coord.B`'s state file (CD10 escape hatch, "leaves
  the parent's view inconsistent").
- Wait for `cancel_tasks` in v1.1.

**GAP G6: orphan_candidates is advisory but duplicate work is already
happening.** By the time the agent sees `orphan_candidates`, the new
child is spawned. The detection happens AT the scheduling tick, not
before. An agent that genuinely intended a rename now has two running
children. The advisory signal fires after the cost is incurred.

What if `B-renamed`'s signature DIFFERS from B's spawn_entry (e.g.,
different `vars`)? Then no `orphan_candidate`. The agent has
duplicated the task with changed parameters and no warning. This is
CD10's documented behavior, but it's a sharp edge for round-2
validation purposes.

### Probe E -- Rename with signature mismatch on the SAME name

> Could the agent instead try to rename B *in place* by submitting
> `B` with changed `waits_on: ["A1"]`?

R8 for B: child exists, submitted entry differs from `spawn_entry`
(`waits_on` field mismatch). Reject with
`InvalidBatchReason::SpawnedTaskMutated { task: "B", changed_fields:
[{field: "waits_on", spawned_value: ["A"], submitted_value: ["A1"]}] }`.
One mismatch rejects the whole submission. A1 and D additions in the
same payload are discarded because of the atomic reject. The agent
must re-submit without the B mutation.

**Behavior: correct. Atomic reject is explicit.** No gap -- this is
the well-specified CD10 path.

### Probe F -- Add a task after full terminal success

> At Turn 26 the batch is `done` at `summarize`. Agent tries:

```
koto next coord --with-data '{"tasks": [..., {"name": "F", ...}]}'
```

Per CD9 / CD11, `summarize` is terminal. `koto next` on a terminal
workflow returns `action: "done"` with `is_terminal: true` and does
not process evidence. The `--with-data` payload is effectively ignored
(or returns an error about submitting to a terminal workflow).

**GAP G7: error surface for post-terminal submission.** Is this an
error envelope (CD11 `action: "error"`) or a silent `action: "done"`
that drops the payload? The design narrative doesn't specify. Best
resolution: reject with `code: "invalid_submission"`, message "workflow
is terminal; cannot submit evidence" -- surface it explicitly so agents
don't silently lose data.

---

## Section 3: Findings

### Finding 1: `feedback.entries` is empty on no-submission ticks

- **Observation:** In Turns 10 and 18, a `koto next coord` call that
  does not carry a `tasks` payload returns `scheduler.feedback.entries:
  {}`. The agent loses the per-entry snapshot of the full task set
  exactly on the ticks that matter most (post-child-completion
  classification and post-retry). The agent must cross-reference
  `blocking_conditions[0].output.children` (the batch view) to see
  state. This is consistent with CD10 ("feedback is per-submission")
  but surprising.
- **Location in design:** CD10, `SchedulerFeedback`; Step 4 walkthrough
  Interaction 6 has the same empty-feedback pattern.
- **Severity:** nice-to-have (documentation).
- **Proposed resolution:** Document explicitly that
  `feedback.entries` is keyed to the *current submission's* tasks
  array. On no-submission ticks it is `{}`. Agents needing a full
  task-set view should read `blocking_conditions[0].output.children`
  (batch view) or `scheduler.materialized_children` (ledger). Update
  `koto-user` skill.

### Finding 2: `spawn_entry` lifecycle through delete-and-respawn is unspecified

- **Observation:** CD9 Part 5 describes runtime reclassification
  (delete-and-respawn of skip markers and of real-template running
  children whose upstream flipped to failure). CD10 R8 relies on the
  child's `spawn_entry` snapshot for mutation comparison. The
  interaction: when a skip marker is delete-and-respawned, is the new
  state file written with a `spawn_entry`? Is it the original
  spawn_entry (from the first `WorkflowInitialized`) or the current
  merged-task-set entry? These are functionally identical when the
  task DAG is unchanged, but an implementation that skips
  `spawn_entry` on skip-marker inits breaks R8 on subsequent
  resubmits.
- **Location in design:** CD9 Part 5; CD10 R8; CD2 (spawn_entry
  snapshot).
- **Severity:** blocker (cross-decision contract).
- **Proposed resolution:** Add to CD9 Part 5: "Every `init_state_file`
  call -- including skip-marker spawns and delete-and-respawn paths --
  writes a `spawn_entry` snapshot derived from the current merged
  task set. R8 comparison always reads the child's current on-disk
  `spawn_entry`." This closes the loop between CD9 reclassification
  and CD10 R8.

### Finding 3: `EntryOutcome::Already` conflates "running" and "skip marker"

- **Observation:** After runtime reclassification materializes a
  skip marker for C, a subsequent resubmit with C in the tasks list
  reports `feedback.entries.C.outcome: already`. Same outcome as a
  normally-running child. Agents watching feedback alone cannot
  distinguish the two.
- **Location in design:** CD10 `EntryOutcome` enum.
- **Severity:** should-fix.
- **Proposed resolution:** Extend `Already` with a variant-internal
  `kind` or promote to two variants: `AlreadyRunning`,
  `AlreadySkipped`. Or add an optional `state: String` field.
  Alternatively, document that feedback is advisory and agents must
  cross-reference `materialized_children[].outcome` for authoritative
  state (the batch view disambiguates via `outcome: success | failure
  | pending | skipped | blocked`).

### Finding 4: Retry windows open R8-mutability for downstream names

- **Observation:** When `retry_failed` triggers delete-and-respawn
  of a skip marker (C deleted in Turn 18), C's task entry becomes
  R8-mutable for the interval between the retry tick and C's next
  respawn. For transitively-closed names like D (whose respawn waits
  on C succeeding), that window can be long. The agent can
  accidentally mutate `vars` or `waits_on` for D during this window
  and R8 won't catch it.
- **Location in design:** CD9 Part 5; CD10 R8 "For each task entry
  whose computed child name already exists on disk..."
- **Severity:** should-fix.
- **Proposed resolution:** Option 1 (strict): R8 compares against
  the most-recent `spawn_entry` EVER recorded for this name in the
  log, not just the current state file. This preserves immutability
  through delete-and-respawn. Option 2 (documented): explicitly
  flag this as "retry resets R8 for rewound/respawned names;
  agents can legitimately amend bad entries during retry rounds."
  Option 1 is safer for the "no silent drops" constraint; Option 2
  aligns with "retry is a do-over."

### Finding 5: `tasks` evidence is unreachable at `analyze_failures`

- **Observation:** The `coord.md` reference template has `accepts:
  { decision }` at `analyze_failures` and `accepts: { tasks }` at
  `plan_and_await`. The agent cannot add new tasks while a batch is
  recovering -- must retry first, return to `plan_and_await`, then
  add. Defensible, but the walkthrough doesn't mention it.
- **Location in design:** Walkthrough `coord.md`; CD9 Part 1
  (state routing).
- **Severity:** nice-to-have (documentation + skill guidance).
- **Proposed resolution:** Document in walkthrough: "Dynamic
  additions are accepted only at the batched state
  (`plan_and_await`). From `analyze_failures`, submit `retry_failed`
  first (or `decision: acknowledge`) to return to a batched state."
  Update `koto-author` to mention this when authoring
  `analyze_failures`-equivalent recovery states.

### Finding 6: `orphan_candidates` fires after the duplicate spawn

- **Observation:** In Probe D, `B-renamed` is spawned as a new child
  on the same tick that `orphan_candidates` flags it as a possible
  rename of B. Detection is advisory, but the duplicate is already
  running. Agents that genuinely intended a rename now have two
  live workers on the same logical issue.
- **Location in design:** CD10 "Renaming surfaces
  `orphan_candidates`."
- **Severity:** should-fix (round-1 Pair 2 Finding 1 gave same
  concern; CD10 addresses the *signal* but not the *cost*).
- **Proposed resolution:** Two options:
  1. Gate spawning of orphan-candidate children behind an explicit
     `confirm_rename: true` field or similar opt-in. If
     `orphan_candidates` would fire, spawn is deferred and the agent
     must re-submit with the confirmation. High-friction, low
     false-positive.
  2. Document the duplicate-work cost explicitly; agents expecting
     rename behavior must use the (v1.1) `cancel_tasks` primitive or
     the manual-delete escape hatch. Current design implicitly does
     option 2.
  Prefer option 1 for safety; option 2 is CD10's current posture.

### Finding 7: Post-terminal `tasks` submission error shape unspecified

- **Observation:** Submitting `tasks` after the parent has reached
  `summarize` (terminal). Is the response `action: "done"` (ignoring
  the payload) or `action: "error"` (rejecting it)? Probe F hit this
  gap.
- **Location in design:** CD11 error envelope; no explicit
  post-terminal submission case.
- **Severity:** nice-to-have.
- **Proposed resolution:** Add an explicit rule: `koto next` on a
  terminal workflow with `--with-data` returns `action: "error"`,
  `code: "invalid_submission"`, `batch: null` (not a batch-specific
  rejection), message "workflow is terminal; cannot submit evidence."

### Finding 8: CD12 Q3 flock contention under identical resubmit

- **Observation:** Probe A showed the flock correctly serializes
  concurrent submitters, returning `concurrent_tick` /
  `integration_unavailable` (exit 1, retryable). Retry semantics:
  the client retries and gets a second, identical-payload tick once
  the first releases. CD10 says "identical resubmission appends for
  audit" -- so the retried call *also* appends an `EvidenceSubmitted`
  event even though the first already did the work. The event log
  gains a duplicate entry for what the agent views as "one
  submission." This is consistent with CD10's "append for audit"
  posture but produces a surprising log pattern under concurrent
  retry.
- **Location in design:** CD10 "Identical resubmission appends for
  audit"; CD12 Q3 advisory flock.
- **Severity:** nice-to-have.
- **Proposed resolution:** Document the interaction: "If a retry
  after a `concurrent_tick` rejection carries the same payload as
  the winning call, the log will contain two `EvidenceSubmitted`
  events for the same submission intent. This is intentional for
  forensic auditability." Update `koto-user` skill accordingly.

---

## Section 4: CD9 x CD10 interaction surface (summary)

The retry + dynamic-additions interaction surface has two load-bearing
contracts that need explicit cross-referencing in the design:

1. **`spawn_entry` must persist across delete-and-respawn** (Finding 2).
   Without this, R8 silently breaks after any retry round that touches
   skip markers or invalidated running children.

2. **R8's "exists on disk" predicate is tick-granular.** When
   delete-and-respawn removes a state file, R8 becomes vacuous for
   that name until the next scheduler respawn. For names in the
   transitive closure of a retried failure, the window can span many
   ticks (Finding 4).

Both gaps are about the temporal boundary between reclassification
and R8 evaluation. CD10 says "R8 and runtime reclassification never
interfere. R8 runs at submission-validation time (pre-append). Runtime
reclassification operates on committed state... they are disjoint
phases of handle_next." This is true *within one tick*. Across ticks,
the reclassification from tick N can change the R8 surface at tick
N+1 in ways the design doesn't explicitly address.

The scenario in this simulation DID drive to `all_success: true`
correctly through the full 6-task cycle with B failing and recovering.
The mechanics work. The gaps are on silent-error-path edges, not on
the happy path.
