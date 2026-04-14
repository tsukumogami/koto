# Decision 10: Mutation semantics + dynamic-addition primitives

**Prefix:** `design_batch-child-spawning_decision_10`
**Complexity:** critical
**Mode:** --auto (confirmed=false, assumptions recorded)
**Scope:** koto (public, tactical)
**Round:** 1 follow-up (supersedes ambiguities in D1/D5, composes with CD9 and CD11)

<!-- decision:start id="batch-child-spawning-mutation-semantics" status="assumed" -->

### Decision: Mutation semantics + dynamic-addition primitives

## Context

Round 1 pair simulations (Cluster C in `round1_synthesis.md`, transcripts
`simulation_round1_pair2.md` / `pair2b.md` / `pair2c.md`) surfaced nine
mutation-pressure gaps in D1's task-list schema. The design's Abstract
(line 93) says `merge_epoch_evidence` "unions the new tasks with the
existing set," but D1 never defines what "union" means when a task entry
whose `<parent>.<name>` child already exists on disk is resubmitted with
*different* `vars`, `waits_on`, or `template` fields. Pair 2b's seven
probes demonstrated that under today's spec every variant fails silently
or incoherently:

- **vars change** on a spawned child: the new EvidenceSubmitted event is
  appended, `backend.exists()` short-circuits the spawn, the vars
  mutation is silently dropped. Agent-visible surface is identical to a
  no-op resubmission.
- **waits_on change** on a spawned child: the gate output reports the
  child as `pending` *and* `blocked` on a task that the child was
  already running past. Internally inconsistent.
- **template change** on a spawned child: dropped at spawn; persists as
  dropped intent through `retry_failed` rewinds (rewind preserves the
  header template).
- **Removal** is impossible: union semantics means task sets are
  monotonic.
- **Rename** silently duplicates work: two children, two workers, two
  PRs for the same upstream issue.
- **Cross-epoch duplicates** (R5 only scopes within a single submission)
  behave inconsistently: sometimes a no-op, sometimes a silent
  divergence.
- **Identical resubmissions** append an EvidenceSubmitted event per
  tick, bloating the log.
- **retry_failed + task-list resubmission** in one payload has no
  specified ordering.
- **Responses lack per-entry feedback**: agents can't tell which entries
  took effect.

This decision is tactically bounded: it specifies *what koto accepts*
at submission-validation time, *what each accepted entry does*, *what
signal the agent gets back*, and *how the chosen rules compose with
CD9's retry path and CD11's error envelope*. Runtime reclassification,
child lifecycle, and error shapes are owned by CD9 and CD11
respectively; this decision uses their committed surfaces as fixtures.

Three constraints anchor the choice:

1. **Append-only.** No primitive can rewrite or delete prior events.
   Any "mutation" of an already-persisted task entry must either reject
   at validation or be expressed via a *new* event that shadows the old
   one.
2. **Disk-derived scheduling.** The scheduler re-derives classification
   from child state files on every tick. A task-entry change that
   conflicts with what's already on disk has no recoverable
   interpretation: either the disk state wins (the submission is a lie)
   or the submission wins (the disk state is orphaned). Both are bugs.
3. **Agents must get an explicit signal.** Round 1 transcripts showed
   silent no-ops cause real confusion — agents retry, see no change,
   and have no way to reason about why.

## Assumptions

- **A1.** CD11's pre-append validation commitment holds: R1-R9 (and new
  R8) run as pure functions of the submitted payload before any
  EvidenceSubmitted append. A rejected submission leaves zero state
  divergence.
- **A2.** CD11's `NextError` envelope is used verbatim: rejections emit
  `action: "error"` + `error.code: "invalid_submission"` +
  `error.batch.kind: "invalid_batch_definition"` +
  `error.batch.reason: <tag>` with typed payload. CD11's enum slot
  `InvalidBatchReason::SpawnedTaskMutated { task, changed_fields }`
  is reserved for this decision. This report defines the payload.
- **A3.** CD9's retry path appends `Rewound` events to individual
  children (delete-and-respawn for synthetic skip markers). Rewinding
  does not clear or rewrite prior EvidenceSubmitted events on the
  parent; it only opens a new epoch for the affected child. The
  parent's task-list evidence (the submitted `tasks` array) is
  therefore stable across retries.
- **A4.** CD12 will enforce or document the serialized-parent-tick
  invariant (one `koto next <parent>` at a time). This decision's
  validation semantics hold under that invariant; it does not need to
  defend against two concurrent submissions racing.
- **A5.** The task-list field's prior submission is recoverable from
  the parent's event log by walking EvidenceSubmitted events in
  reverse — the "prior entry" against which field-for-field matching
  happens is the first EvidenceSubmitted entry under which the child
  actually spawned (recorded in the child's `WorkflowInitialized`
  event as part of D2's atomic-init metadata), not merely the most
  recent submission. Without this anchoring, append-only state cannot
  answer "what fields did this child spawn under."
- **A6.** Agents accept that `cancel_tasks` (a new reserved evidence
  action) will not land in v1. Round 1 pair 2b identified removal as a
  high-severity gap but the work of specifying cancellation semantics
  (closure, running-child handling, dependent cascade) is sibling-
  sized to CD9's entire retry story. v1 documents the non-feature
  explicitly; v1.1 or a follow-up decision adds `cancel_tasks`.
- **A7.** Agents will read a new `scheduler.feedback` map (or similarly
  named field) on `SchedulerOutcome` that explains what happened to
  each submitted task entry this tick. The field is additive over
  CD11's `SchedulerOutcome` extensions and doesn't collide with
  `spawned_this_tick` / `already` / `blocked` / `skipped` / `errored`.

## Chosen: Strict spawn-time immutability (R8), union by name, defer removal/rename to cancel_tasks in v1.1, per-entry scheduler feedback, audit-preserving identical resubmission

A nine-part package, one rule per sub-question. The package is internally
consistent: R8 anchors cross-epoch identity; union-by-name is the only
primitive available under append-only; feedback gives agents a signal
for every entry; and all error paths route through CD11's envelope.

### 10.1 — Spawn-time immutability (R8): reject mutated fields on spawned children

**Rule R8 (new; runs pre-append, after R0-R9 core validation):** for
each task entry in the submitted `tasks` array whose computed child
name `<parent>.<task.name>` already exists on disk as a spawned child,
the entry's `template`, `vars`, and `waits_on` fields MUST match
field-for-field the entry under which the child was originally
spawned. Mismatch is rejected with CD11's
`InvalidBatchReason::SpawnedTaskMutated`.

**What "originally spawned under" means.** At child init (D2's atomic
init), the `WorkflowInitialized` event on the child records a
`spawn_entry` field capturing the *exact* task entry (as JSON
canonical form) that the scheduler used. R8 compares the submitted
entry to that recorded entry. The comparison is on the serialized
canonical form of the three fields only — `name` is the identity key,
other fields (if D1 adds any later) are extension points for future
R8 revision.

```rust
// Child state file additive field (D2 coordination):
struct WorkflowInitialized {
    // existing fields...
    spawn_entry: SpawnEntrySnapshot,  // NEW
}

struct SpawnEntrySnapshot {
    template: Option<String>,   // canonical: null == omitted
    vars: serde_json::Value,    // canonical JSON
    waits_on: Vec<String>,      // canonical: sorted
}
```

**Payload shape for SpawnedTaskMutated (extends CD11's envelope):**

```rust
#[derive(Serialize)]
pub struct SpawnedTaskMutatedDetail {
    pub task: String,                        // short name, not <parent>.<name>
    pub changed_fields: Vec<MutatedField>,   // one entry per differing field
}

#[derive(Serialize)]
pub struct MutatedField {
    pub field: String,              // "template" | "vars" | "waits_on" | "vars.<key>"
    pub spawned_value: serde_json::Value,   // what the child was spawned with
    pub submitted_value: serde_json::Value, // what the current submission tried to set
}
```

For `vars`, the diff is reported per-key (`vars.X`, `vars.Y`) when
possible, falling back to the whole `vars` object when the submitted
value is a non-object or the spawned value is a non-object. For
`waits_on`, the diff is the symmetric difference of the two sets; the
`submitted_value` is the submitted array and `spawned_value` is the
recorded array (both canonical-sorted). For `template`, the value is
the raw string (or null for "omitted == inherit default").

Wire shape on rejection:

```json
{
  "action": "error",
  "state": "plan_and_await",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Task 'issue-B' was spawned with different fields; resubmission would mutate 1 field(s)",
    "details": [{"field": "tasks[1].vars.GITHUB_TOKEN", "reason": "spawned_task_mutated"}],
    "batch": {
      "kind": "invalid_batch_definition",
      "reason": "spawned_task_mutated",
      "task": "issue-B",
      "changed_fields": [
        {"field": "vars.GITHUB_TOKEN",
         "spawned_value": "ghp_old...",
         "submitted_value": "ghp_new..."}
      ]
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

**Atomicity.** One mismatched entry rejects the whole submission. No
partial acceptance. The agent sees every mutation that would have
applied; fixing one field at a time reveals the next rejection on the
next submission (deterministic debugging).

**Why reject, not silently apply retroactively, not silently ignore.**

- *Silently ignoring* is the status quo and caused four round-1
  findings. It violates "explicit signal to agents."
- *Retroactively applying* (i.e., editing the child's header in place)
  violates append-only. It would require a new `HeaderMutated` event
  on the child, which introduces a mutation primitive that doesn't
  exist elsewhere in koto.
- *Rejecting* gives the agent the exact diff and leaves disk state
  untouched. The agent can either drop the mutation or issue a
  (future) `cancel_tasks` + resubmission to reset the child.

**Why field-for-field, not semantic equivalence.** Canonical form
comparison is cheap and deterministic. A "semantic" comparison
(e.g., "different vars but resolves to the same rendered value")
requires running the template engine in validation, which is
expensive and doesn't match what the agent submitted. The rule is:
submit what you spawned with, or get the exact diff back.

**Interaction with CD9's runtime reclassification.** CD9's retry path
deletes-and-respawns synthetic skip markers on every tick when
dependency outcomes change. This is koto-driven, not submitter-
driven. R8 applies only to *submitter-visible* entries at pre-append
validation time. CD9's reclassification operates below validation:
when the scheduler deletes a skip marker and respawns it as a real
child, the scheduler uses the *same* `spawn_entry` snapshot recorded
on the skip marker's WorkflowInitialized event (which was itself
derived from the most recently accepted task entry). No submitter is
involved; R8 is not consulted.

Equivalently: R8 is a validator rule in the `validate_submission`
step. Runtime reclassification is an action in the scheduler step.
They never run concurrently on the same tick, and the scheduler never
re-validates; it uses already-committed state. See `10.8` for the
crisper statement of this invariant.

### 10.2 — Union by name, strict field-for-field match

**Rule.** "Union" in `merge_epoch_evidence` is defined exactly:

1. The effective task set for a parent is the union (by `name`) of all
   task entries across all accepted EvidenceSubmitted events on the
   parent's log.
2. For each `name` that appears in multiple submissions:
   - If no child has spawned yet for that name: the *latest*
     submission's entry wins (last-write-wins on un-spawned entries,
     since no disk state is anchored yet).
   - If a child has spawned for that name: R8 applies; the submission
     is rejected if fields differ from the spawned entry.
3. Names appearing in only one submission are included as-is.

This is a precise specification of the Abstract's "unions" claim. It
is deterministic, derivable from the event log, and consistent with
append-only.

**Why last-write-wins for un-spawned entries (not first-write-wins).**
Pair 2b considered first-wins. Rejected: first-wins prevents the
legitimate "I mis-submitted, let me correct" use case before any
spawn has occurred. A task entry that has never yet materialized is
indistinguishable from a draft; allowing correction before spawn is
cheap and natural. After spawn, R8 locks the entry.

**Why this is not "replace semantics."** Full replace would allow the
submitter to omit a previously-submitted name, which under
append-only means "silently deprecate this entry." We reject that in
10.3: removal requires an explicit primitive.

### 10.3 — Removal: `cancel_tasks` deferred to v1.1, documented non-feature in v1

**v1 behavior.** A submission that omits a task name which appeared in
a prior submission is accepted; the omitted name stays in the
effective task set (union-by-name). Omission is not a cancellation
signal.

**Why not "reject submissions that omit a previously-named task."**
That makes every resubmission a high-friction operation (the agent
must echo back every prior name, forever). It also creates a weird
interaction: the agent resubmits the exact same list minus one name
— what happens? If reject, the agent can never drop a task from
their mental model. If accept, removal by omission is back on the
table. The cleanest rule is "omission is a no-op; cancellation
requires the primitive" — same shape as retry_failed.

**Why defer `cancel_tasks` to v1.1.** Designing cancel semantics is
sibling-complexity to CD9. It must specify:

- Closure: does cancelling a task cascade to its `waits_on` dependents?
- Running-child handling: does cancel abort an in-flight child, or
  refuse to cancel non-terminal children?
- Re-add: once cancelled, can a later submission revive the task
  under the same name, or is the name poisoned?
- Interaction with retry_failed: can a cancelled task be retried?
- Event shape: new `TaskCancelled` event on parent? `Cancelled`
  outcome on the child?
- Validation: are reserved names `retry_failed` and `cancel_tasks`
  mutually orthogonal, or does mixed-payload R10 (from CD9) extend?

CD11's `InvalidBatchReason` enum is open for extension; adding
`cancel_tasks` variants in v1.1 does not break the v1 envelope.

**v1 documentation.** The design's "Dynamic additions" section gains
one paragraph:

> Task sets are append-only by name. A task entry, once submitted, is
> part of the effective batch forever. Omitting a previously-submitted
> name from a later submission does not remove it. Cancellation is
> deferred to v1.1, which will introduce a reserved `cancel_tasks`
> evidence action paralleling `retry_failed`. Operators requiring
> immediate removal in v1 can manually delete the child's state file
> (`rm -rf ~/.koto/workflows/<parent>.<name>`); this leaves the parent's
> effective task-set view inconsistent and is not recommended except
> for disaster recovery.

**Severity triage.** Round 1 pair 2b rated removal "High." This
decision acknowledges the gap and commits to v1.1. The v1 escape
hatch (manual state-file deletion) is not supported but is available
for operators who accept the inconsistency cost.

### 10.4 — Renaming: accepted as-is with warning; agent bears the cost

**Rule.** A submission where a prior name is absent and a new name
appears is accepted under union semantics. The prior name's entry (if
its child spawned) remains in the effective set. The new name spawns
a new child. Two workers, two PRs for the same upstream work.

**Why not reject.** Rejecting "name X absent + name Y new" assumes
koto can distinguish "rename" from "I'm adding a new task and
omitting an unrelated old one." It cannot — rename intent is
semantic, not syntactic.

**Why not implicit rename via a `replaces` field.** A `replaces`
field would require:

- Defining what happens to the old-name child (cancel? leave running?)
- Cancel requires the v1.1 primitive (10.3).
- Leave-running reduces to the status quo.

So `replaces` collapses to `cancel_tasks` + submit, which is v1.1.

**v1 mitigation: `orphan_candidates` warning on scheduler feedback.**
When the scheduler observes that a submitted task entry has
vars-and-waits_on-signature byte-identical to an already-spawned task
under a different name, it surfaces this in
`scheduler.feedback.orphan_candidates`:

```json
{
  "scheduler": {
    "spawned_this_tick": ["coord.issue-A-renamed"],
    "feedback": {
      "orphan_candidates": [
        {
          "new_task": "issue-A-renamed",
          "signature_match": "issue-A",
          "confidence": "exact",
          "message": "Task 'issue-A-renamed' has identical vars and waits_on to the already-spawned 'issue-A'. If this is a rename, note that task removal is not supported in v1; 'issue-A' will continue running."
        }
      ]
    }
  }
}
```

This is advisory, not blocking. The submission proceeds; the agent
gets a readable signal to investigate. `confidence: "exact"` matches
byte-identical; a future iteration can add `confidence: "fuzzy"` for
near-matches (all same except `GITHUB_TOKEN`).

**Why surface this rather than silently accept.** Round 1 finding 5
rated rename as "High severity, silent duplicate execution." The
warning closes the silence without committing to a rename primitive.

### 10.5 — Cross-epoch duplicate name resolution: explicit rule, R8 enforces consistency

**Rule.** R5 (duplicate names within a single submission) is unchanged.
Cross-epoch collisions resolve per 10.2:

- If the name has not yet spawned: last-write-wins; the new
  submission's entry replaces the prior entry in the effective set.
- If the name has spawned: R8 enforces field-for-field match or
  rejects.

**Interaction with pair 2b Finding 6.** Pair 2b proposed
"first-wins by name across epochs." This decision adopts a
bifurcated rule keyed on spawn state, because first-wins-everywhere
breaks pre-spawn correction. The bifurcation is crisp and
derivable-from-disk: the scheduler reads the child's existence to
classify, and R8 is the deciding predicate for spawned entries.

**Concrete examples.**

- *Epoch 1: `[A(vars={x:1})]`, A does not spawn this tick (blocked).
  Epoch 2: `[A(vars={x:2})]`.* Both entries reference an unspawned A.
  Last-write-wins: effective entry is `A(vars={x:2})`. Scheduler
  later spawns with `vars={x:2}`.
- *Epoch 1: `[A(vars={x:1})]`, A spawns. Epoch 2: `[A(vars={x:2})]`.*
  R8 fires; submission rejected with `SpawnedTaskMutated`.
- *Epoch 1: `[A(vars={x:1})]`, A spawns. Epoch 2: `[A(vars={x:1})]`.*
  R8 passes (no mutation); submission accepted as a no-op on the
  task set. See 10.6 for audit handling.

**No separate "R5-extended" rule is needed.** The bifurcated rule
above is equivalent to "R5 is per-submission; cross-epoch identity
is enforced by R8 for spawned entries and last-write-wins for
unspawned entries." Consolidating reduces the rule surface agents
must remember.

### 10.6 — Identical resubmission: append the event for full audit, mark as no-op in feedback

**Rule.** A byte-identical submission (same task list, field-for-field,
including order-insensitive comparison for set-typed fields) passes
validation (trivially — no mutations to detect), appends a regular
EvidenceSubmitted event to the parent log, and runs the scheduler
tick. The tick will typically find every task `already` spawned
(idempotent no-op).

**Why append, not suppress.** Three reasons:

1. **Audit clarity.** Operators replaying the event log should see
   every submission the agent made, even no-ops. Suppression hides
   "the agent kept polling" from forensic investigation.
2. **Uniform validation.** Suppression adds a special case at
   `handle_next` pre-validation ("is this byte-identical to the last
   accepted submission?"). The special case must be computed
   pre-append, which means loading the prior submission and
   canonical-form-comparing. That's nearly the cost of R8 itself; the
   saving is not free.
3. **Feedback signal (see 10.7).** The feedback field already tells
   the agent "this task is `already` spawned." Agents that see
   repeated `already` responses know their submissions are no-ops
   without koto needing to suppress the append.

**Storage cost.** Each identical submission adds one EvidenceSubmitted
event. Assuming 1 KB per event (typical), 1000 no-op polls add 1 MB
to the log. `koto query` and `derive_batch_view` already iterate
events; this is not a correctness concern. Operators who care about
log size can cap their agents' polling frequency.

**Alternative considered (and rejected): append but mark `is_noop:
true`.** Adds a field that consumers must learn about. Doesn't save
storage. The `scheduler.feedback` field already communicates
no-op-ness per-task, which is strictly more useful than a
whole-submission boolean.

### 10.7 — Per-entry feedback: `scheduler.feedback` map keyed by task name

**Rule.** `SchedulerOutcome::Scheduled` gains a `feedback` field that
enumerates what happened to *each entry in this submission*. CD11's
existing fields (`spawned_this_tick`, `already`, `blocked`, `skipped`,
`errored`) remain; `feedback` is additive and indexable by the
agent's submission payload.

```rust
pub struct SchedulerOutcome {
    // ... CD11's fields ...
    pub feedback: SchedulerFeedback,   // NEW
}

#[derive(Serialize)]
pub struct SchedulerFeedback {
    // Keyed by short task name (agent-submitted, not <parent>.<name>).
    pub entries: BTreeMap<String, EntryOutcome>,
    // Detected signature matches (rename detection, 10.4).
    pub orphan_candidates: Vec<OrphanCandidate>,
}

#[derive(Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum EntryOutcome {
    Accepted,              // new entry, spawned this tick
    Already,               // entry matches existing spawned child, no-op this tick
    Blocked {              // new entry, has unmet deps (not yet spawnable)
        waits_on: Vec<String>,
    },
    Errored {              // per-task spawn failure (CD11's scheduler.errored)
        kind: String,      // SpawnErrorKind serialized
    },
    // R8 rejections never reach scheduler; they error at pre-append validation.
    // Ignored is not needed: under 10.3 omission isn't a submission entry at all.
}
```

**Variant coverage (addresses sub-question 7):**

| Submitted entry state                                | Variant    |
|------------------------------------------------------|------------|
| New name, deps satisfied, spawned this tick          | `accepted` |
| New name, deps satisfied, spawn failed (bad template)| `errored`  |
| New name, deps not yet satisfied                     | `blocked`  |
| Existing name, matches spawned entry, no-op          | `already`  |
| Existing name, mutated fields                        | (never reaches scheduler — R8 rejects pre-append) |

**Why not include `ignored`.** Round 1 suggested `ignored` for
silent-drop cases. Under R8, there are no silent drops — mutated
entries reject, not ignore. The `ignored` variant would only apply
if we accepted "submissions that mutate spawned entries are accepted
but ignored," which this decision rejects in 10.1. Removing the
variant simplifies the contract.

**Wire example** (agent submits `[A, B, C]` where A already spawned,
B is new with deps on A, C is new with no deps):

```json
{
  "scheduler": {
    "spawned_this_tick": ["coord.C"],
    "already": ["coord.A"],
    "blocked": ["coord.B"],
    "skipped": [],
    "errored": [],
    "feedback": {
      "entries": {
        "A": {"outcome": "already"},
        "B": {"outcome": "blocked", "waits_on": ["A"]},
        "C": {"outcome": "accepted"}
      },
      "orphan_candidates": []
    }
  }
}
```

**Why a map keyed by name rather than a parallel array.** Agents
submit by name; the feedback map is directly indexable. Parallel
arrays (spawned/already/blocked) remain for consumers who want a
flat list by category; the map is for "what happened to this
specific entry I submitted?"

**Why nested in `scheduler`, not a sibling top-level field.** The
feedback is scheduler output — it only exists when a scheduler tick
ran. On rejected submissions, `scheduler: null` per CD11. Nesting
keeps the envelope consistent.

### 10.8 — Interaction with CD9's retry and runtime reclassification

**Invariant.** R8 runs at submission-validation time (pre-append, per
A1 / CD11). CD9's retry mechanism and runtime reclassification operate
on accepted, committed state and never re-trigger validation. The two
never interfere because they're in different phases of the advance
loop:

```
koto next <parent> --with-data '<payload>'
  |
  v
[1] parse + schema-validate payload       (existing)
[2] R0-R9 pre-append validation           (CD11, this decision adds R8)
  |
  (if rejected: return error, zero state writes)
  |
  v
[3] handle_retry_failed (if payload has retry_failed)   (CD9)
     - validate R10 retry set
     - write EvidenceSubmitted {retry_failed: payload}
     - write Rewound to closure children
     - write EvidenceSubmitted {retry_failed: null}
[4] append EvidenceSubmitted {tasks: ...}  (append-phase)
[5] advance loop
     - template transitions (CD9's evidence.retry_failed: present)
     - materialize_children hook
       - scheduler tick (runtime reclassification for skip markers)
     - gate evaluation
[6] return response (scheduler outcome + feedback)
```

R8 fires only at step [2]. Steps [3]-[5] operate on already-validated
evidence. The scheduler's runtime reclassification (deleting and
respawning a skip marker when deps flip) is a koto-internal action
that reads the child's recorded `spawn_entry` snapshot — no submitter
payload is consulted.

**Cascade scenario 1: agent resubmits with different `template` for a
task that was just rewound via retry_failed.**

- T0: submit `[{name: A, template: v1.md}, {name: B, template: v1.md}]`.
  Both spawn. A: success. B: failure.
- T1: submit `{retry_failed: {children: [B]}}`.
  CD9's step [3] rewinds B (new epoch, same `spawn_entry` snapshot
  pointing at `template: v1.md`). B's WorkflowInitialized event for
  the new epoch carries the *same* spawn_entry as the original.
- T2: submit `[{name: A, template: v1.md}, {name: B, template: v2.md}]`.
  R8 fires at step [2] on B: `spawned_value: v1.md`,
  `submitted_value: v2.md`. Rejected. Rewind doesn't change the
  spawn_entry; that's the definition of spawn_entry (it's "what B was
  originally spawned with," not "what B is currently running with").

To actually migrate B to v2, the agent must (in v1) either (a) accept
B stays on v1.md forever, or (b) wait for v1.1's `cancel_tasks` +
resubmit. In v1 there is no v2-migration primitive.

**Cascade scenario 2: agent resubmits with same fields for a task that
was just rewound.**

- T0-T1: same as scenario 1.
- T2: submit `[{name: A, template: v1.md}, {name: B, template: v1.md}]`.
  R8 passes (no mutation). EvidenceSubmitted appends. Scheduler tick
  sees B already exists (in its new epoch from the rewind), classifies
  as `already` in feedback. No-op. Consistent with 10.6.

**Cascade scenario 3: runtime reclassification during a submission
tick.**

- T0: submit `[A, B, C(waits_on: [B])]`. A spawns, B spawns, C is
  blocked (marker: skipped pending deps). All good.
- T1: B fails.
- T2: submit identical `[A, B, C]`. R8 passes. At step [5]
  scheduler tick:
  - Scheduler re-classifies C: B failed, so C's skip marker stays
    (dep is still `failure`).
  - Feedback for C: `already` (the skip marker is the "already"
    state).

R8 and runtime reclassification never touch the same data structure:
R8 reads the submitted payload and the child's `spawn_entry`;
reclassification reads child state files and dep outcomes.

**Cascade scenario 4: agent submits a new task referencing a
to-be-retried child.**

- T0-T1: same as scenario 1 (B was in failure, just rewound).
- T2: submit `[A, B, D(waits_on: [B])]`. D is a new name.
  R8 passes on A and B (unchanged). D is new. Validation accepts.
  EvidenceSubmitted appends. Scheduler tick: D is blocked on B
  (B is in the rewind's new epoch, outcome `pending`). Feedback:
  `accepted` for D? No — D's outcome is `blocked`. Feedback:
  `{outcome: "blocked", waits_on: ["B"]}`.

This is the correct behavior: a new task can be added that depends
on an existing task mid-retry.

### 10.9 — Mixed `retry_failed + tasks` submission: rejected, confirming CD9

**Rule.** A submission payload containing both `retry_failed: {...}`
and a `tasks` key (whether the tasks differ from the last-accepted set
or not) is rejected with CD11's
`InvalidRetryReason::MixedWithOtherEvidence` (variant reserved by
CD9, see CD11's Q11).

**Why confirm CD9's recommendation.**

- `retry_failed` is intercepted at the CLI layer before the advance
  loop (CD9 Part 2). Evidence validation runs in the advance loop.
  Mixed payloads would require a three-way split: retry_failed goes
  to handler A, tasks go to validator B, and both must succeed or
  neither commits. This is a two-phase commit inside `handle_next`
  for a rare use case.
- Agents who want both effects submit two `koto next` calls. Natural
  serialization; no two-phase needed.
- The rejection message is clear: "Mixed payloads not supported;
  submit retry_failed and tasks in separate calls."

**Wire shape (per CD11's envelope):**

```json
{
  "action": "error",
  "state": "analyze_failures",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "retry_failed cannot be submitted with other evidence fields in the same payload",
    "details": [{"field": "retry_failed", "reason": "mixed_with_other_evidence"}],
    "batch": {
      "kind": "invalid_retry_request",
      "reason": "mixed_with_other_evidence",
      "extra_fields": ["tasks"]
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

The `extra_fields` list names every non-`retry_failed` key in the
submission payload, so the agent can see exactly what to drop.

## Rationale

**Tied to round-1 blockers (traceability to Cluster C):**

| Round-1 finding | Resolution |
|-----------------|------------|
| 2b F1 (vars mutation silently dropped) | 10.1 R8 rejects with `SpawnedTaskMutated` |
| 2b F2 (waits_on mutation, inconsistent DAG) | 10.1 R8 covers waits_on |
| 2b F3 (template mutation dropped, persists through retry) | 10.1 R8 + 10.8 scenario 1 (spawn_entry locked through rewind) |
| 2b F4 (task removal impossible) | 10.3 defer cancel_tasks to v1.1, document non-feature |
| 2b F5 (rename silently duplicates work) | 10.4 accept-with-warning via `orphan_candidates` in feedback |
| 2b F6 (cross-epoch duplicate-name resolution unspecified) | 10.5 bifurcated rule: last-wins pre-spawn, R8 post-spawn |
| 2b F7 (identical resubmission pollutes log) | 10.6 append for audit, feedback signals no-op |
| 2b F8 (retry_failed + task list ordering) | 10.9 reject mixed payloads (confirms CD9); 10.8 scenarios document each-in-isolation |
| 2b F9 (no resubmission feedback) | 10.7 `scheduler.feedback` map |

**Why the package is internally consistent.**

1. **R8 is the single anchor.** Every cross-epoch mutation question
   routes through "does the child exist on disk?" yes → R8 →
   field-match or reject; no → last-write-wins. One predicate, two
   branches, covers every sub-question on the mutation axis.
2. **Append-only is preserved.** No primitive edits events. R8
   rejections leave zero state. Identical resubmissions append (10.6)
   — additive only. `cancel_tasks` deferral (10.3) avoids introducing
   a tombstone primitive that would complicate event replay.
3. **Feedback is the explicit-signal channel.** 10.7's map covers
   every valid outcome; 10.1's error envelope covers every invalid
   outcome. Together the agent gets a signal for every entry in every
   submission — no silent cases remain.
4. **CD9 composes cleanly.** R8 at validation time + runtime
   reclassification at scheduler time are disjoint phases. 10.8
   walks through every interaction scenario; none require special-
   casing.
5. **CD11's envelope is additive.** All new error paths use CD11's
   reserved `SpawnedTaskMutated` and `MixedWithOtherEvidence` variants
   with payload shapes defined here. CD11 does not need revision;
   this decision fills in the slots CD11 left open.

**Why reject weaker alternatives.**

- *Silently ignore mutations* (status quo): four round-1 findings
  root-cause here. Violates "explicit signal."
- *Apply retroactively* (retroactive edit): requires a mutation
  primitive that doesn't exist. Breaks append-only.
- *Replace semantics (not union)*: allows removal by omission, which
  under append-only has no coherent representation. Forces either
  tombstones (v1.1 work) or silent drops (bad).
- *First-wins cross-epoch*: prevents pre-spawn correction.
- *`replaces` field for explicit rename*: collapses to
  `cancel_tasks + submit`, so it's v1.1 work.

## Alternatives Considered

### Sub-question 1 (spawn-time immutability)

- **A1a. Silently ignore mutations on spawned children.** Rejected:
  round-1 F1/F2/F3 all stem from this. Violates explicit-signal.
- **A1b. Apply mutations retroactively via header edit.** Rejected:
  requires mutation primitive on append-only state; introduces
  HeaderMutated event with complex replay semantics.
- **A1c. Chosen: reject with `SpawnedTaskMutated`, field-level diff
  in payload.** Append-only-safe, agent-debuggable.

### Sub-question 2 (union vs replace)

- **A2a. Full replace.** Rejected: allows removal by omission under
  append-only (no coherent representation).
- **A2b. Chosen: union by name, new-names appended, existing-name
  entries R8-gated.**

### Sub-question 3 (removal)

- **A3a. `cancel_tasks` reserved action in v1.** Rejected: sibling-
  sized to CD9's retry story (closure, running-child handling,
  cascade, interaction with retry, event shape, validation). Not
  feasible in this decision's scope.
- **A3b. Chosen: defer to v1.1, document as non-feature in v1.**
  Operators needing manual recovery use state-file deletion as an
  unsupported escape hatch.
- **A3c. Accept submissions that omit un-spawned tasks; reject if
  spawned.** Rejected: makes resubmission high-friction (agent must
  echo every prior name). Doesn't compose with last-write-wins on
  un-spawned entries.

### Sub-question 4 (renaming)

- **A4a. Accept as-is with no warning.** Rejected: round-1 F5 is
  "High severity, silent duplicate execution."
- **A4b. Reject if existing name absent and new name appears.**
  Rejected: can't distinguish rename from "add + unrelated omit."
- **A4c. Implicit rename via `replaces` field.** Rejected: collapses
  to cancel_tasks + submit (v1.1 work).
- **A4d. Chosen: accept with `orphan_candidates` warning in
  scheduler feedback when byte-identical signature detected.** Closes
  silence without committing to rename primitive.

### Sub-question 5 (cross-epoch duplicates)

- **A5a. Explicit R5-extended rule across epochs.** Rejected:
  redundant with R8. One anchor is cleaner than two.
- **A5b. First-wins cross-epoch.** Rejected: prevents pre-spawn
  correction.
- **A5c. Chosen: R8 for spawned, last-write-wins for unspawned.**
  One rule, two branches, covers every case.

### Sub-question 6 (identical resubmission)

- **A6a. Suppress append at handle_next.** Rejected: special-case cost
  nearly equals R8; obscures audit trail.
- **A6b. Append but mark `is_noop: true`.** Rejected: redundant with
  scheduler feedback.
- **A6c. Chosen: append for audit; feedback field signals no-op
  per-task.**

### Sub-question 7 (feedback field variants)

- **A7a. Single list of ignored entries.** Rejected: agents need
  positive signals too ("yes, C was accepted"), not just negative.
- **A7b. Parallel arrays (accepted, blocked, etc.).** Rejected:
  doesn't index directly by the agent's submission.
- **A7c. Chosen: map keyed by name + typed `EntryOutcome` enum +
  separate `orphan_candidates` array.** Name-indexed for agent
  convenience; typed enum for machine dispatch; orphan array for
  the signature-match case (not every task has an orphan candidate).

### Sub-question 8 (interaction with CD9)

- **A8a. R8 applies to all spawn events including reclassification
  respawns.** Rejected: reclassification is koto-driven; forcing R8
  would require koto to "submit to itself" which makes no semantic
  sense.
- **A8b. Chosen: R8 at submission-validation only; runtime
  reclassification operates on committed state using spawn_entry
  snapshot.**

### Sub-question 9 (mixed payloads)

- **A9a. Support mixed payloads via two-phase commit.** Rejected:
  complexity not justified for a use case the agent can serialize
  naturally.
- **A9b. Chosen: reject with `MixedWithOtherEvidence`, confirming
  CD9.** Agent submits in two calls.

## Consequences

### What becomes easier

- **One mental model for the task set.** Agents learn "names are
  sticky once spawned." Every other question resolves from that.
- **Debugging mutation errors is mechanical.** R8's `changed_fields`
  payload is a direct diff. The agent fixes exactly what the envelope
  names.
- **Per-entry feedback closes the silent-drop class.** Every entry in
  every submission gets an outcome. Agents that see `already` stop
  retrying that entry; agents that see `accepted` move on.
- **`cancel_tasks` work is scoped, not blocked.** The deferral is
  explicit, the v1.1 design space is named, and v1 operators have a
  manual escape hatch. Future decision can extend CD11's envelope
  enums without breaking v1 consumers.
- **Rename isn't silent anymore.** Operators see `orphan_candidates`
  warnings and can investigate before duplicate PRs open.
- **Cross-epoch identity is stable.** Once `<parent>.<X>` spawns, it
  refers to that one child forever (until v1.1's cancel_tasks or a
  retry_failed rewind — neither changes identity).

### What becomes harder

- **Agents must remember the fields they submitted.** Resubmitting
  with a missing `vars` key when the original had one triggers R8
  (missing key ≠ original value). The canonical comparison is
  key-sensitive.

  *Mitigation:* the SpawnedTaskMutated payload shows the expected
  `spawned_value`, so the agent can copy it back. Documentation
  should emphasize "resubmit the exact entry you submitted before."

- **`spawn_entry` must be recorded on WorkflowInitialized.** This is
  a D2 change: child init writes an additional snapshot field. One
  more field in the atomic-init write; no new write ordering.
- **Last-write-wins on un-spawned entries means batches can evolve
  mid-flight.** A task that hasn't yet spawned can have its entry
  updated by a later submission. This is a feature (correction use
  case), but the scheduler must use the *latest* entry when it
  eventually spawns, not the entry at original-submission time.
  Implementation: the scheduler, when spawning a task, reads the
  effective task entry from the union of all accepted
  EvidenceSubmitted events (last-write-wins for the `name`), not
  from the first submission alone.
- **Removal is deferred.** Operators who mis-submit a task name
  (e.g., typo, wrong issue number) cannot recover in v1 short of
  manual state-file deletion. Documentation must surface this as a
  known limitation.
- **Identical resubmission logs grow.** Operators who cap agent
  polling frequency avoid this; agents that implement exponential
  backoff on `already` responses mitigate naturally.
- **`scheduler.feedback` is a new response field.** Consumers
  parsing the envelope must tolerate new top-level keys under
  `scheduler`. Standard JSON-contract behavior, but doc it.

### Implementation touch points

- `src/engine/batch/validate.rs` (new or extension of
  `validate_submission`): add R8 check. Requires loading the
  `spawn_entry` snapshot for each task whose `<parent>.<name>` child
  exists. Canonical-form comparison on `template`, `vars`, `waits_on`.
- `src/engine/init.rs` (D2's atomic init): add `spawn_entry` field to
  `WorkflowInitialized` event. Populated from the task entry the
  scheduler is about to materialize.
- `src/engine/batch/scheduler.rs`: add `SchedulerFeedback` struct;
  populate per-entry outcomes during the tick. Include
  `orphan_candidates` detection: walk submitted entries, for each new
  name compare signature (vars + waits_on canonical form) against
  all spawned children's `spawn_entry`. Match → add to candidates.
- `src/cli/next_types.rs`: extend `SchedulerOutcome.Scheduled` with
  `feedback: SchedulerFeedback`. Extend CD11's `InvalidBatchReason`
  with `SpawnedTaskMutated { task, changed_fields: Vec<MutatedField> }`
  and `MutatedField { field, spawned_value, submitted_value }`. Extend
  `InvalidRetryReason` with `MixedWithOtherEvidence { extra_fields:
  Vec<String> }` (CD11 reserved this variant).
- `src/engine/batch/merge.rs`: implement union-by-name with
  last-write-wins for un-spawned entries, R8-gated for spawned
  entries.
- `docs/designs/DESIGN-batch-child-spawning.md`:
  - Decision 1: add R8 to runtime rules list.
  - Data Flow: insert R8 check before EvidenceSubmitted append.
  - Dynamic additions section: document union semantics precisely,
    document cancel_tasks deferral, document orphan_candidates.
  - Key Interfaces: add `scheduler.feedback` shape.
- `plugins/koto-skills/skills/koto-author/` and `koto-user/`: update
  both skills (CLAUDE.md section). koto-user documents the new
  feedback field and R8 rejection shapes; koto-author documents
  the "task entries are frozen after spawn" invariant.

### Coordination with parallel decisions

- **CD9 (retry path):** R8 locks `spawn_entry` across rewinds (10.8
  scenario 1). CD9's `reserved_actions` discovery surface already
  handles retry; no changes needed there. CD9's
  `InvalidRetryReason::MixedWithOtherEvidence` is confirmed (10.9).
- **CD11 (error envelope):** payload shapes for `SpawnedTaskMutated`
  (10.1) and `MixedWithOtherEvidence` (10.9) are defined here and
  should be noted in CD11's cross-validation sweep. CD11 reserved the
  slots; this decision fills them in.
- **CD12 (concurrency):** R8 assumes serialized parent ticks. Under
  CD12's advisory-lockfile mechanism, two concurrent submissions that
  both would R8-reject are serialized; no split-brain validation.
  CD12 has no dependency on this decision except the serialized-tick
  invariant it must uphold.
- **CD13 (post-completion observability):** `batch_final_view` is
  derived from the effective task set, which under this decision is
  stable after spawn (R8 prevents mutation). CD13's view lookup is
  unchanged.
- **CD14 (path resolution):** R8's template-field comparison uses
  the raw string as submitted (canonical form: absent and null are
  both normalized to null, per CD11 Q7). CD14's path-resolution
  logic runs only when a child actually spawns; R8 runs earlier
  (pre-append) and doesn't invoke resolution.

### Forward-looking notes (v1.1 hooks)

When `cancel_tasks` lands:

- New reserved evidence action, intercepted at CLI layer like
  retry_failed. Validation rule R11 specifies: child must be in
  `failure`, `skipped`, or `success` outcome (not `pending`); closure
  direction is downward (cancel the named task + its dependents);
  mixed with other evidence rejects with a new
  `InvalidCancelRequest::MixedWithOtherEvidence` variant.
- A cancelled child's state file is preserved but marked
  `cancelled: true`; the parent's effective task set excludes
  cancelled names from new-spawn consideration. `retry_failed` of a
  cancelled child is rejected with a new
  `InvalidRetryReason::ChildCancelled` variant.
- The `EntryOutcome` enum in `SchedulerFeedback` gains a `cancelled`
  variant so feedback remains complete.

These are not commitments; they're the shape of a future decision's
design space. Flagging them here prevents v1's choices from closing
off v1.1's natural extensions.

<!-- decision:end -->

---

## YAML Summary

```yaml
decision_result:
  status: COMPLETE
  chosen: >
    Mutation semantics are strict: (1) Rule R8 rejects any submission
    whose task entry's template/vars/waits_on differs from the entry
    under which the child was originally spawned, using CD11's
    SpawnedTaskMutated envelope with per-field diff payload. (2) Union
    semantics mean new names append and cross-epoch identity is stable
    post-spawn; last-write-wins for un-spawned entries allows pre-spawn
    correction. (3) Removal is deferred to v1.1's cancel_tasks reserved
    action; v1 documents non-support explicitly and provides no
    omission-as-deletion path. (4) Renaming is accepted as-is but
    surfaces an orphan_candidates warning in scheduler feedback when a
    new task's signature matches an already-spawned task under a
    different name. (5) Cross-epoch duplicate resolution is R8 for
    spawned names, last-write-wins for unspawned — one bifurcated rule,
    no separate R5-extended. (6) Identical resubmission appends an
    EvidenceSubmitted event for audit clarity; no suppression. (7)
    Feedback is a scheduler.feedback map keyed by task name with typed
    EntryOutcome variants (accepted / already / blocked / errored) plus
    a sibling orphan_candidates array. (8) R8 runs at submission-
    validation time only; CD9's runtime reclassification operates on
    committed state using the recorded spawn_entry snapshot, and the
    two phases never interfere. (9) Mixed retry_failed + tasks payloads
    are rejected per CD9's recommendation, using CD11's reserved
    MixedWithOtherEvidence variant with extra_fields list.
  confidence: high
  rationale: >
    Round-1 pair 2b enumerated nine concrete mutation-pressure gaps.
    The package resolves all nine without re-opening CD9 or CD11.
    Every sub-question's answer routes through one predicate — "does
    the child exist on disk?" — making the contract mechanical and
    learnable. Append-only semantics are preserved throughout: no
    primitive edits prior events, rejections leave zero state, and
    identical resubmissions merely append. The deferral of
    cancel_tasks to v1.1 is explicit and scoped, with the v1
    non-feature documented and a forward hook reserved in the enum.
    Runtime reclassification (CD9) composes cleanly because R8 runs
    in a disjoint phase; 10.8 walks through four interaction
    scenarios to demonstrate. CD11's envelope slots are filled with
    typed payloads; no CD11 revision is required.
  assumptions:
    - CD11's pre-append validation commitment holds and R8 runs in the same phase
    - WorkflowInitialized event extends to carry a spawn_entry snapshot (D2 coordination)
    - Agents accept cancel_tasks deferral to v1.1 and document the non-feature in v1
    - Agents read scheduler.feedback map; the additive field does not conflict with CD9/CD11 extensions
    - CD12 upholds the serialized-parent-tick invariant so R8 validation never races
    - Canonical-form comparison (sorted waits_on, null == omitted for template, per-key vars diff) is the contract agents design against
  rejected:
    - name: Silently ignore mutations on spawned children (status quo)
      reason: Root cause of four round-1 blockers; violates explicit-signal constraint
    - name: Apply mutations retroactively via header edit
      reason: Requires mutation primitive that breaks append-only semantics
    - name: Full replace semantics (drop omitted names)
      reason: Removal-by-omission has no coherent representation under append-only
    - name: cancel_tasks as a v1 reserved action
      reason: Sibling-complexity to CD9's retry story; not feasible in this decision's scope
    - name: Reject rename patterns (existing absent, new present)
      reason: Cannot distinguish rename from legitimate add + unrelated omit syntactically
    - name: Implicit rename via replaces field
      reason: Collapses to cancel_tasks + submit; blocked behind v1.1 work
    - name: First-wins cross-epoch duplicate resolution
      reason: Prevents pre-spawn correction; R8 already handles the post-spawn case
    - name: Suppress identical-resubmission appends at handle_next
      reason: Special-case cost nearly equals R8; obscures audit trail; redundant with feedback signals
    - name: Mark identical resubmissions with is_noop on the event
      reason: Redundant with scheduler.feedback per-task signal
    - name: Single ignored list for feedback
      reason: Agents need positive signals per-entry, not just negative drops
    - name: Support mixed retry_failed + tasks payloads via two-phase commit
      reason: Two-phase complexity not justified for a use case the agent can serialize
  report_file: wip/design_batch-child-spawning_decision_10_report.md
```
