# Simulation — Round 2, Pair C1: R8 mutation rejection, spawn_entry snapshot, feedback shape

Design under validation: `docs/designs/DESIGN-batch-child-spawning.md`
(revised; Decision 10 + Decision 11 in effect).

Reference: `wip/walkthrough/walkthrough.md`.

Prior round: `wip/walkthrough/simulation_round1_pair2b.md` surfaced
nine mutation-pressure gaps. Round 2's job is to verify the revised
design (CD10: R8 spawn-time immutability, `spawn_entry` snapshot,
`scheduler.feedback` per-entry map, canonical-form comparison,
`cancel_tasks` deferred to v1.1) rejects every mutation case
correctly and produces observable signals.

Scenario: 3-task batch A -> B -> C. A spawned and running. The
AGENT submits progressive mutations against A. KOTO responses are
generated from Decision 10 rules (R8, union-by-name, canonical-form
comparison, secret redaction in Security Considerations) and
Decision 11 envelope (`action: "error"`, typed `error.batch`).

`[GAP: ...]` markers flag places where the revised design is silent
or ambiguous despite Decision 10's intent.

---

## Section 1: Transcript

### Setup — baseline batch (identical to walkthrough Interaction 3)

Parent `coord` is parked at `plan_and_await`. Agent submits:

```json
{
  "tasks": [
    {"name": "A", "template": "impl-issue.md", "vars": {"ISSUE_NUMBER": "101"}},
    {"name": "B", "vars": {"ISSUE_NUMBER": "102"}, "waits_on": ["A"]},
    {"name": "C", "vars": {"ISSUE_NUMBER": "103"}, "waits_on": ["B"]}
  ]
}
```

KOTO spawns `coord.A`. `WorkflowInitialized` on `coord.A` records:

```json
{
  "event": "WorkflowInitialized",
  "spawn_entry": {
    "template": "impl-issue.md",
    "vars": {"ISSUE_NUMBER": "101"},
    "waits_on": []
  }
}
```

(Canonical form: `waits_on` sorted; `template` materialized to the
resolved value; `vars` a sorted key-value map.) A is `Running`.

Feedback map on the initial tick:

```json
{
  "feedback": {
    "entries": {
      "A":       {"outcome": "accepted"},
      "B":       {"outcome": "blocked", "waits_on": ["A"]},
      "C":       {"outcome": "blocked", "waits_on": ["B"]}
    },
    "orphan_candidates": []
  }
}
```

---

### Case 1a: `vars` mutation — plain key

#### AGENT

```json
{
  "tasks": [
    {"name": "A", "vars": {"ISSUE_NUMBER": "999"}},
    {"name": "B", "vars": {"ISSUE_NUMBER": "102"}, "waits_on": ["A"]},
    {"name": "C", "vars": {"ISSUE_NUMBER": "103"}, "waits_on": ["B"]}
  ]
}
```

#### KOTO

R8 fires at submission-validation (pre-append). `coord.A` exists on
disk; canonical-form comparison vs `spawn_entry` shows `vars.ISSUE_NUMBER`
differs. Whole-submission reject. Nothing appended; B and C are
untouched even though they are byte-identical.

```json
{
  "action": "error",
  "state": "plan_and_await",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Batch definition rejected: task 'A' cannot mutate fields of a spawned child",
    "details": [{"field": "tasks[0].vars.ISSUE_NUMBER", "reason": "spawned_task_mutated"}],
    "batch": {
      "kind": "invalid_batch_definition",
      "reason": "spawned_task_mutated",
      "task": "A",
      "changed_fields": [
        {"field": "vars.ISSUE_NUMBER", "spawned_value": "101", "submitted_value": "999"}
      ]
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

_Gloss: R8 cleanly rejects. `changed_fields` is keyed by
`vars.<key>` per the `MutatedField` type (src review: `"vars" |
"waits_on" | "template" | "vars.<key>"`). Atomic: the legitimate
B/C rows go nowhere._

### Case 1b: `vars` mutation — redacted key (`GITHUB_TOKEN`)

Parent's initial spawn had `vars.GITHUB_TOKEN = "ghp_old..."`; agent
resubmits A with `GITHUB_TOKEN = "ghp_new..."`.

#### KOTO

Security Considerations §"Secret-rotation gotcha under R8 rejection"
redacts values whose keys match `*_TOKEN` / `*_SECRET` / `*_KEY` /
`*_PASSWORD` / `DATABASE_URL` / `DATABASE_PASSWORD` in the diff
payload.

```json
{
  "action": "error",
  "error": {
    "code": "invalid_submission",
    "batch": {
      "kind": "invalid_batch_definition",
      "reason": "spawned_task_mutated",
      "task": "A",
      "changed_fields": [
        {"field": "vars.GITHUB_TOKEN", "spawned_value": "[REDACTED]", "submitted_value": "[REDACTED]"}
      ]
    }
  }
}
```

_Gloss: the rejection itself still fires; only the values are
redacted. The agent gets the signal "you tried to mutate
`vars.GITHUB_TOKEN`" without either secret leaking into logs or
response transcripts._

`[GAP C1-S1: Redaction sentinel shape is unspecified.]` CD10
references redaction but does not fix the wire form. Is it
`"[REDACTED]"`, `null`, an object like `{"redacted": true}`? The
distinction matters because agents parse `changed_fields` and
distinguishing "value legitimately changed from X to null" vs
"value redacted" needs a dedicated sentinel. Design should pin the
literal.

`[GAP C1-S2: Does redaction also apply on the success path?]` If A
is NOT yet spawned and the agent resubmits with a different
`GITHUB_TOKEN`, that is legitimate pre-spawn correction
(last-write-wins per CD10). No redaction is triggered because
nothing is diffed. OK. But `koto query coord --events` will show
`EvidenceSubmitted` events carrying the literal secret. Security
Considerations says "do not place secrets in vars" — confirm the
same redaction rule applies to `koto query` event viewers (it
does not per the current spec).

---

### Case 2: `waits_on` mutation — R8 vs R4 ordering

#### AGENT

```json
{
  "tasks": [
    {"name": "A", "vars": {"ISSUE_NUMBER": "101"}, "waits_on": ["Z"]},
    {"name": "B", "vars": {"ISSUE_NUMBER": "102"}, "waits_on": ["A"]},
    {"name": "C", "vars": {"ISSUE_NUMBER": "103"}, "waits_on": ["B"]}
  ]
}
```

Z does not exist in the task list. Two rules can fire: R4 (dangling
ref) and R8 (mutation of a spawned task's `waits_on`).

#### KOTO

Per §"Decision 11 / submission validation", the Flow Step 4
rewrite enumerates rules in a fixed order: R3, R4, R5, R6, R8, R9.
R4 runs before R8.

```json
{
  "action": "error",
  "state": "plan_and_await",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Batch definition rejected: waits_on references undefined task",
    "details": [{"field": "tasks[0].waits_on[0]", "reason": "dangling_ref"}],
    "batch": {
      "kind": "invalid_batch_definition",
      "reason": "dangling_refs",
      "entries": [{"task": "A", "waits_on": "Z"}]
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

_Gloss: R4 fires first. The agent sees the dangling-ref error, not
the mutation error. This is defensible — the DAG is malformed
regardless of R8 — but it means an agent who mutates A's waits_on to
include Z gets the "dangling" message and may not realize R8 would
also have fired._

`[GAP C1-S3: R3/R4/R5/R6/R8/R9 ordering is listed once (§Flow Step 4
rewrite, line 2427) but not restated near Decision 10/11.]` An
implementer reading only Decision 10 might sort R8 first (it's
cheap: name lookup + field diff). Design should state R4-before-R8
explicitly near the R8 prose so the fix-one-error-at-a-time UX
behavior is intentional and not an accident of implementation order.

#### Follow-up: `waits_on: ["C"]` (exists, but introduces a cycle)

Agent corrects to `waits_on: ["C"]`. Now R4 passes (C is defined),
but R3 (cycle) fires: A waits on C; C waits on B; B waits on A.

```json
{
  "error": {
    "batch": {
      "kind": "invalid_batch_definition",
      "reason": "cycle",
      "cycle": ["A", "C", "B", "A"]
    }
  }
}
```

R3 beats R8 again. Only once the agent submits a waits_on change
that passes R3-R7 does R8 catch it:

```json
{
  "tasks": [
    {"name": "A", "vars": {"ISSUE_NUMBER": "101"}, "waits_on": ["C"]},
    {"name": "B", "vars": {"ISSUE_NUMBER": "102"}},
    {"name": "C", "vars": {"ISSUE_NUMBER": "103"}}
  ]
}
```

Graph is now legal (A waits on C; B, C independent). R8:

```json
{
  "error": {
    "batch": {
      "kind": "invalid_batch_definition",
      "reason": "spawned_task_mutated",
      "task": "A",
      "changed_fields": [
        {"field": "waits_on", "spawned_value": [], "submitted_value": ["C"]}
      ]
    }
  }
}
```

_Gloss: `waits_on` is diffed as a whole field (sorted-array
equality). Canonical comparison sorts both sides before diffing, so
`["C", "B"]` vs `["B", "C"]` would compare equal._

`[GAP C1-S4: `changed_fields.field` for waits_on is `"waits_on"`,
not `"waits_on[0]"`, per the MutatedField type.]` The diff is
whole-array, not per-index. Agents displaying diffs need to render
`spawned_value: []` against `submitted_value: ["C"]` as a set
difference. Minor but worth documenting in agent-facing guidance.

---

### Case 3: `template: null` vs omitted

Original submission had `template: "impl-issue.md"` explicit.
Resubmission has `template: null` explicit.

#### AGENT

```json
{
  "tasks": [
    {"name": "A", "template": null, "vars": {"ISSUE_NUMBER": "101"}},
    {"name": "B", "vars": {"ISSUE_NUMBER": "102"}, "waits_on": ["A"]},
    {"name": "C", "vars": {"ISSUE_NUMBER": "103"}, "waits_on": ["B"]}
  ]
}
```

#### KOTO

Decision 11: `template: null` ≡ omitted, both inherit hook's
`default_template`. Hook declares `default_template: "impl-issue.md"`,
so canonical form of the submission is `template: "impl-issue.md"`,
matching `spawn_entry.template`. R8 does not fire. Submission
accepted; identical-resubmission audit path applies.

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "scheduler": {
    "spawned_this_tick": [],
    "materialized_children": [
      {"name": "coord.A", "outcome": "pending", "state": "working"}
    ],
    "already": ["coord.A"],
    "blocked": ["coord.B", "coord.C"],
    "skipped": [],
    "errored": [],
    "warnings": [],
    "feedback": {
      "entries": {
        "A": {"outcome": "already"},
        "B": {"outcome": "blocked", "waits_on": ["A"]},
        "C": {"outcome": "blocked", "waits_on": ["B"]}
      },
      "orphan_candidates": []
    }
  }
}
```

_Gloss: canonical-form comparison correctly treats `null` and
omitted as equivalent when `default_template` resolves to the same
string. The `SchedulerRan` event appends (submission was
non-trivial to validate)._

`[GAP C1-S5: If the hook's `default_template` changed between
the original spawn and the resubmission (agent reloaded the
template, or `default_template` was computed from evidence), the
canonical form of the resubmission may resolve to a different
string than `spawn_entry.template`.]` R8 would then falsely fire
on a submission the agent considers unchanged. The design says
canonical form "resolves defaults," but does not say whether
`default_template` is resolved against the live hook at
resubmission time or against the hook snapshot captured at A's
spawn. Resolving against the live hook is the simpler
implementation but risks false-positive R8 rejections. Worth
pinning.

---

### Case 4: Renaming (new name, old name absent)

#### AGENT

```json
{
  "tasks": [
    {"name": "A-renamed", "vars": {"ISSUE_NUMBER": "101"}},
    {"name": "B", "vars": {"ISSUE_NUMBER": "102"}, "waits_on": ["A-renamed"]},
    {"name": "C", "vars": {"ISSUE_NUMBER": "103"}, "waits_on": ["B"]}
  ]
}
```

#### KOTO

Per CD10 §"Renaming surfaces `orphan_candidates`": omission of `A`
is a no-op (removal deferred; `cancel_tasks` not in v1). A-renamed
is a brand new name. Signature check: A-renamed's canonical
`(vars, waits_on)` = `({"ISSUE_NUMBER":"101"}, [])` matches
`spawn_entry` of `coord.A` exactly. R8 does not fire (A-renamed is
not on disk). Submission accepted.

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "scheduler": {
    "spawned_this_tick": ["coord.A-renamed"],
    "materialized_children": [
      {"name": "coord.A",         "outcome": "pending", "state": "working"},
      {"name": "coord.A-renamed", "outcome": "pending", "state": "working"}
    ],
    "already": [],
    "blocked": ["coord.B", "coord.C"],
    "skipped": [],
    "errored": [],
    "warnings": [],
    "feedback": {
      "entries": {
        "A-renamed": {"outcome": "accepted"},
        "B":         {"outcome": "blocked", "waits_on": ["A-renamed"]},
        "C":         {"outcome": "blocked", "waits_on": ["B"]}
      },
      "orphan_candidates": [
        {
          "new_task": "A-renamed",
          "signature_match": "A",
          "confidence": "exact",
          "message": "new task 'A-renamed' has identical (vars, waits_on) signature to already-spawned 'A'; did you intend to rename?"
        }
      ]
    }
  }
}
```

_Gloss: CD10 is explicit — rename is advisory, not blocking.
`A-renamed` gets spawned. `coord.A` continues running. Both
children are live._

`[GAP C1-S6: `feedback.entries` is keyed by agent-submitted short
name.]` The **original** A is neither in the submission nor in
feedback. An agent iterating only `feedback.entries` to
"reconcile what I submitted vs. what happened" never sees A at
all. A is only visible via `scheduler.materialized_children`
(ledger) or `orphan_candidates[].signature_match`. The field is
stable-ordered (BTreeMap), but an agent needs to know to
cross-reference both. Agent-facing guidance should spell this out:
use `materialized_children` for "what children exist on disk" and
`feedback.entries` for "what the submission did."

`[GAP C1-S7: What happens to coord.A's state file?]` It keeps
running (as the design says — omission is a no-op). When
coord.A finishes, it counts toward the parent's
`children-complete` tally, inflating `total` beyond what the
latest submission declared. The agent sees `total: 4` against a
task list of 3 names. This is working as intended under CD10 but
is confusing; the walkthrough does not cover this case.

`[GAP C1-S8: Rename + cancel workaround.]` CD10 defers
`cancel_tasks` to v1.1. The "escape hatch" is documented as
"manually delete the child's state file," but this leaves the
parent's view inconsistent because `materialized_children` is
derived from `backend.list()` at tick time. A manually-deleted
A will vanish from the ledger, so the parent's `total` decreases.
No warning; no audit trail of the deletion in the parent log.
This behaves correctly but represents a stateful gotcha worth
flagging in the "operators needing immediate removal in v1"
paragraph.

---

### Case 5: Omission

#### AGENT

```json
{
  "tasks": [
    {"name": "B", "vars": {"ISSUE_NUMBER": "102"}, "waits_on": ["A"]},
    {"name": "C", "vars": {"ISSUE_NUMBER": "103"}, "waits_on": ["B"]}
  ]
}
```

A is omitted. B still waits on A.

#### KOTO

Per CD10 §"Removal is deferred": omission is a no-op. The effective
task set is the union across all accepted `EvidenceSubmitted`
events: A (from epoch 1) + B + C. B's `waits_on: ["A"]` resolves
against the union, so R4 does not fire. A continues running.

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "scheduler": {
    "spawned_this_tick": [],
    "materialized_children": [
      {"name": "coord.A", "outcome": "pending", "state": "working"}
    ],
    "already": [],
    "blocked": ["coord.B", "coord.C"],
    "skipped": [],
    "errored": [],
    "warnings": [],
    "feedback": {
      "entries": {
        "B": {"outcome": "blocked", "waits_on": ["A"]},
        "C": {"outcome": "blocked", "waits_on": ["B"]}
      },
      "orphan_candidates": []
    }
  }
}
```

_Gloss: no error; A is kept alive by the union rule. Feedback lists
only B and C because they are the only agent-submitted names this
tick. A is visible via materialized_children._

`[GAP C1-S9: No signal that A was NOT in the submission.]` An
agent who mistakenly believes omission-means-cancel will never get
corrective feedback. The design rejected "reject submissions that
omit a previously-named task" (too high-friction) but did not
add a soft warning. Consider `scheduler.warnings` entry
`OmittedPriorTask { task: "A" }` on first-occurrence, suppressed
on subsequent identical-shape resubmissions. This would close
the silent-no-op gap CD10 explicitly tried to eliminate.

---

### Case 6: Identical resubmission

Agent resubmits the exact same payload as the original.

#### KOTO

Validation trivially passes. `EvidenceSubmitted` appends for audit.
Scheduler tick runs. Every task classifies as `already` for
spawned, `blocked` for unspawned.

```json
{
  "scheduler": {
    "spawned_this_tick": [],
    "materialized_children": [
      {"name": "coord.A", "outcome": "pending", "state": "working"}
    ],
    "already": ["coord.A"],
    "blocked": ["coord.B", "coord.C"],
    "skipped": [],
    "errored": [],
    "warnings": [],
    "feedback": {
      "entries": {
        "A": {"outcome": "already"},
        "B": {"outcome": "blocked", "waits_on": ["A"]},
        "C": {"outcome": "blocked", "waits_on": ["B"]}
      },
      "orphan_candidates": []
    }
  }
}
```

_Gloss: observably identical to Case 3's response (template-null
acceptance) and distinguishable from "no submission at all" only
via the `EvidenceSubmitted` event in the parent log._

`[GAP C1-S10: SchedulerRan append rule under identical resubmit.]`
§"SchedulerRan event" says the event appends when at least one of
`spawned`, `skipped`, or `errored` is non-empty. All three are
empty for identical resubmit. No `SchedulerRan` appended. But
`EvidenceSubmitted` did append (for audit). So the parent log has
a lonely EvidenceSubmitted with no sibling SchedulerRan — an
auditor investigating "why did the agent think it needed to
resubmit?" sees the evidence but not the tick outcome. Consider
expanding the append rule to include "any evidence accepted this
tick" so identical-resubmits leave a visible audit pair.

---

### Case 7: Dynamic pure addition

#### AGENT

```json
{
  "tasks": [
    {"name": "A", "vars": {"ISSUE_NUMBER": "101"}},
    {"name": "B", "vars": {"ISSUE_NUMBER": "102"}, "waits_on": ["A"]},
    {"name": "C", "vars": {"ISSUE_NUMBER": "103"}, "waits_on": ["B"]},
    {"name": "D", "vars": {"ISSUE_NUMBER": "104"}, "waits_on": ["C"]}
  ]
}
```

#### KOTO

R8 passes (A byte-identical via canonical form). D is new. B and C
unchanged; union wins. Scheduler tick:

```json
{
  "scheduler": {
    "spawned_this_tick": [],
    "materialized_children": [
      {"name": "coord.A", "outcome": "pending", "state": "working"}
    ],
    "already": ["coord.A"],
    "blocked": ["coord.B", "coord.C", "coord.D"],
    "skipped": [],
    "errored": [],
    "warnings": [],
    "feedback": {
      "entries": {
        "A": {"outcome": "already"},
        "B": {"outcome": "blocked", "waits_on": ["A"]},
        "C": {"outcome": "blocked", "waits_on": ["B"]},
        "D": {"outcome": "blocked", "waits_on": ["C"]}
      },
      "orphan_candidates": []
    }
  }
}
```

_Gloss: D joins the graph. Accepted as `blocked`. Entry outcome
for D is `blocked`, not `accepted`, because it didn't spawn this
tick (C hasn't completed). This is correct per CD10's
EntryOutcome enum._

`[GAP C1-S11: `accepted` vs `blocked` for a new un-spawned task.]`
CD10 defines `EntryOutcome::Accepted` ("new, no deps, spawned
this tick"), `Already` ("spawned in a prior tick"), `Blocked`
("deps not terminal"), `Errored`. D is new AND blocked. Its
outcome is `Blocked`. There is no discriminator for "new and
accepted into the graph but blocked on deps" vs "already a
member of the task list and still blocked." Two submissions
apart, B's outcome was also `Blocked`. The agent cannot
distinguish "I just added D" from "D has been sitting blocked
for three ticks." Consider `EntryOutcome::AcceptedBlocked { waits_on
}` as a disjoint case, OR a top-level `scheduler.newly_added:
["D"]` sibling field. Minor UX polish; not a correctness gap.

---

### Case 8: Combined mutation + addition (atomicity probe)

#### AGENT

```json
{
  "tasks": [
    {"name": "A", "vars": {"ISSUE_NUMBER": "999"}},
    {"name": "B", "vars": {"ISSUE_NUMBER": "102"}, "waits_on": ["A"]},
    {"name": "C", "vars": {"ISSUE_NUMBER": "103"}, "waits_on": ["B"]},
    {"name": "D", "vars": {"ISSUE_NUMBER": "104"}, "waits_on": ["C"]}
  ]
}
```

A's vars mutated; D is new and legitimate.

#### KOTO

Per CD10 §R8: "One mismatched entry rejects the whole submission."
Whole-submission pre-append rejection. D never reaches the
scheduler. No `EvidenceSubmitted` appended.

```json
{
  "action": "error",
  "state": "plan_and_await",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Batch definition rejected: task 'A' cannot mutate fields of a spawned child",
    "details": [{"field": "tasks[0].vars.ISSUE_NUMBER", "reason": "spawned_task_mutated"}],
    "batch": {
      "kind": "invalid_batch_definition",
      "reason": "spawned_task_mutated",
      "task": "A",
      "changed_fields": [
        {"field": "vars.ISSUE_NUMBER", "spawned_value": "101", "submitted_value": "999"}
      ]
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

_Gloss: atomicity holds. To add D, the agent must resubmit with A
unmutated._

`[GAP C1-S12: Multi-task mutation reports only the first.]`
`InvalidBatchReason::SpawnedTaskMutated { task, changed_fields }`
is singular — one task, a list of fields. If the agent mutated A
AND B (both spawned), the response reports only A. The agent fixes
A, resubmits, gets a B error, fixes B, resubmits. Two round-trips
for one intent. Consider `MutatedTasks { violations: Vec<{task,
changed_fields}> }` to surface all R8 violations in one response.
Round-1 pair 2b Finding 9 ("resubmission has no success/failure
feedback for the mutation portion") is closed by the typed error,
but this sub-gap survives.

---

## Section 2: Probes on observability

### P1: Is `spawn_entry` observable via `koto query`?

Per CD10 §Decision 2 amendment: "`WorkflowInitialized` event gains
a `spawn_entry: Option<SpawnEntrySnapshot>` field." The field is
part of the event payload, so `koto query coord.A --events`
should surface it.

```
$ koto query coord.A --events --format=json
```

Expected:

```json
{
  "events": [
    {
      "type": "WorkflowInitialized",
      "header": { ... },
      "spawn_entry": {
        "template": "impl-issue.md",
        "vars": {"ISSUE_NUMBER": "101"},
        "waits_on": []
      }
    }
  ]
}
```

_Verdict:_ the design says the field is additive on the event, so
it IS observable via `koto query`. The walkthrough does not
explicitly demonstrate this — worth a line in the revised
walkthrough.

`[GAP C1-P1: Can the agent READ spawn_entry PROACTIVELY to
avoid R8 rejection?]` Yes, via `koto query coord.<task>
--events`. But the walkthrough and agent-facing skills do not
document this pattern. An agent authoring a resubmission has to
either (a) remember what it submitted originally, or (b) query
each spawned child's event log to reconstruct the spawn_entry
snapshot for each spawned name. Pattern (b) is cheap but requires
documentation. Consider a convenience: `koto query coord
--spawn-entries` returning a `{task_name: spawn_entry}` map at
the parent level.

### P2: `feedback.entries` iteration order and keying

Per CD10: `BTreeMap<String, EntryOutcome>` keyed by
agent-submitted short name. BTreeMap iteration is
lexicographically ordered by key. Agents iterate deterministically
regardless of submission order.

_Verdict:_ stable and deterministic. Docs already spec this.

### P3: Orphan state under rename

From Case 4: `coord.A` keeps running after the rename. It appears
in `materialized_children` but not in the resubmitted task list.
Its state file persists. On the next `koto next coord` tick, the
scheduler classifies `coord.A` as `already` via the ledger (every
child on disk is a member of the effective set).

`[GAP C1-P2: Classification of coord.A after rename.]` Is A
classified via the union-of-evidence (yes, A is in epoch 1's
submission) or via the ledger (yes, coord.A exists on disk)?
Answer: both. CD10 §"Union by name" says the effective task set
is the union across all accepted `EvidenceSubmitted` events.
So A is in the effective set. `feedback.entries` on the
next tick will NOT include A (the submitted names are only
A-renamed, B, C). The ledger via `materialized_children` will
include coord.A. The `children-complete` gate reads the ledger,
so A still counts toward `total`. This is consistent but
subtle; deserves a walkthrough example.

---

## Section 3: Findings

Numbered for easy reference. Severity: **Blocker** (design must
change), **Should-fix** (ambiguity that will cause a reimplementer
to diverge), **Polish** (UX improvement).

### Finding C1-1: Redaction sentinel is unspecified [Should-fix]

Source: Case 1b, `[GAP C1-S1]`.
CD10 Security Considerations says secret values are redacted in
`MutatedField`, but does not fix the sentinel literal. Agents
cannot reliably distinguish "redacted" from "null" or "empty
string" without a contract. **Recommendation:** pin
`spawned_value` / `submitted_value` to a tagged object
`{"redacted": true}` (or a string sentinel `"[REDACTED]"`) and
document under Decision 10. Add a test vector.

### Finding C1-2: Rule ordering (R4 before R8) is stated once but not near R8 [Should-fix]

Source: Case 2, `[GAP C1-S3]`.
Flow Step 4 rewrite lists R3/R4/R5/R6/R8/R9 order. Decision 10
discusses R8 behavior in isolation and does not cross-reference.
An implementer might order R8 before R4 (it's cheaper). The user
UX "fix one error at a time" behavior hinges on R3-first.
**Recommendation:** add a one-liner to Decision 10:
"R8 runs after R3/R4/R5/R6 in the fixed validation order
(Decision 11)." Already implied by §Flow Step 4 but not locally
visible.

### Finding C1-3: `default_template` resolution point for canonical form [Should-fix]

Source: Case 3, `[GAP C1-S5]`.
If the hook's `default_template` value changes between original
spawn and resubmission, canonical-form equality can silently flip
— R8 fires on a submission the agent considers unchanged.
**Recommendation:** pin canonical-form resolution to the
`spawn_entry` hook snapshot (immutable) rather than the live hook
at resubmission time. Document the rule under Decision 10's
canonical-form paragraph.

### Finding C1-4: Feedback key-space vs effective task set [Polish]

Source: Case 4, Case 5, `[GAP C1-S6]`.
`feedback.entries` is keyed by agent-submitted short name, so
names NOT in the current submission (but in the effective task set
via union) are invisible in feedback. Agents must cross-reference
`materialized_children`. Correct behavior, but the walkthrough
and skills should document this cross-reference pattern.
**Recommendation:** add a walkthrough interaction for the rename
case; update `koto-user` SKILL to cover "reading `feedback.entries`
vs `materialized_children`."

### Finding C1-5: Omission produces no signal [Should-fix]

Source: Case 5, `[GAP C1-S9]`.
CD10 explicitly targets silent-no-op elimination, but omission of
a prior name still produces zero agent-visible signal. An agent
believing omission-cancels-a-task gets no correction.
**Recommendation:** emit a `SchedulerWarning::OmittedPriorTask {
task }` on first occurrence of a name in the effective set that
the current submission does not re-declare. Suppress on
subsequent identical-shape resubmissions.

### Finding C1-6: Identical resubmit leaves lonely EvidenceSubmitted [Polish]

Source: Case 6, `[GAP C1-S10]`.
SchedulerRan append gate (one of `spawned`, `skipped`, `errored`
non-empty) excludes pure-`already` ticks. Identical resubmit
produces `EvidenceSubmitted` with no sibling `SchedulerRan`.
Auditors see evidence but not tick outcome.
**Recommendation:** expand the append rule to "any tick with
non-empty `feedback.entries` AND a preceding EvidenceSubmitted
this call." Or: always append on tick.

### Finding C1-7: R8 violation reporting is singular [Polish]

Source: Case 8, `[GAP C1-S12]`.
`SpawnedTaskMutated { task, changed_fields }` reports one task
per response. Multi-task violations force round-trips.
**Recommendation:** change to `SpawnedTaskMutated { violations:
Vec<{task, changed_fields}> }` (or wrap the singular case in a
`Vec` of length 1). No wire-shape cost; better UX. Call out as
v1 polish or defer to v1.1.

### Finding C1-8: Coord.A orphan semantics under rename deserve a walkthrough [Polish]

Source: Case 4, `[GAP C1-S7]`, `[GAP C1-P2]`.
Renamed-A + original-A coexist on disk and count toward
`children-complete`. Behavior is correct per CD10 but surprising.
**Recommendation:** add an explicit example to
`wip/walkthrough/walkthrough.md` showing the rename case with
`total` = 4 (A + A-renamed + B + C) and `orphan_candidates`
non-empty. Clarifies the "escape hatch" language under
Decision 10's "Removal is deferred" paragraph.

### Finding C1-9: New-and-blocked tasks look identical to already-blocked [Polish]

Source: Case 7, `[GAP C1-S11]`.
`EntryOutcome::Blocked` applies to both "just-added D, waiting
on C" and "B, has been waiting on A for three ticks." No
discriminator.
**Recommendation:** either split into `Blocked` /
`AcceptedBlocked`, OR emit `scheduler.newly_added: Vec<String>`
as a sibling field, OR add a timestamp/seq field to EntryOutcome.
Low priority.

### Finding C1-10: spawn_entry discoverability [Should-fix]

Source: P1, `[GAP C1-P1]`.
Agents can read `spawn_entry` via `koto query coord.<task>
--events` but no skill/walkthrough teaches the pattern.
**Recommendation:** document the pattern in `koto-user` SKILL.
Consider a parent-level convenience command `koto query coord
--spawn-entries` returning a `{task_name: spawn_entry}` map.
R8 rejections surface `spawn_entry` values in `changed_fields`
anyway, so the convenience is minor but speeds up resubmission
drafting.

---

## Section 4: Summary

Decision 10 closes every finding from round-1 pair 2b's mutation
cluster that is a correctness gap. R8 fires cleanly on every
mutation case (vars, waits_on, template explicit-change) and
correctly distinguishes canonical-equivalent resubmissions
(template: null ≡ omitted ≡ "impl-issue.md"). The
`spawn_entry` snapshot is observable via `koto query --events`
and serves as the diff reference. `feedback.entries` provides
the positive per-task signal CD10 promised, with stable BTreeMap
ordering.

**Residual gaps (none blocker-level):**

- Three Should-fix items that will cause implementation
  divergence if not pinned: redaction sentinel shape (C1-1),
  canonical-form `default_template` resolution point (C1-3),
  and omission-warning (C1-5). All are spec tightenings, not
  architectural changes.
- One Should-fix documentation item: `feedback.entries` vs
  `materialized_children` distinction (C1-4) and spawn_entry
  readback pattern (C1-10).
- Four Polish items: multi-task R8 reporting (C1-7),
  lonely EvidenceSubmitted (C1-6), new-vs-already-blocked
  discriminator (C1-9), rename walkthrough (C1-8).

R8 sequencing against R4/R3/R5/R6 is correct per §Flow Step 4
rewrite but deserves a local cross-reference in Decision 10
(C1-2).

No finding contradicts CD10's commitment to "explicit signals,
no silent no-ops." The residual items either polish UX or tighten
under-specified corners — none require redesign.
