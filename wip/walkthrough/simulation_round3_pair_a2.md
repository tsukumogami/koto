# Round 3, Pair A2 -- spawn_entry lifecycle through a deep cascade

Round-3 verification simulation. Round 2 Pair C2 (F2) flagged that the
`spawn_entry` lifecycle through delete-and-respawn was unspecified:
CD10's R8 relies on the on-disk `spawn_entry`, while CD9's runtime
reclassification blows that file away. Round 3 closed this by
clarifying (Decision 5.2 "`spawn_entry` on respawn and `ready_to_drive`
dispatch gate"):

- Every `init_state_file` call -- including skip-marker spawns and
  every delete-and-respawn -- writes a fresh `spawn_entry` derived from
  the CURRENT submission's task entry.
- Workers MUST dispatch on `ready_to_drive`, not on mere presence in
  `materialized_children`. `ready_to_drive == (not terminal) AND (every
  waits_on dep from the child's current spawn_entry is
  terminal-success)`.
- `EntryOutcome::Respawning` is the transient feedback outcome for the
  mid-respawn window (R8 vacuous this tick; next tick re-evaluates).

This simulation drives a 6-task linear chain `A -> B -> C -> D -> E ->
F` with `failure_policy: skip_dependents`, pushes the batch through a
full skip-cascade, retries B, watches the cascade unwind, then stresses
it with a second failure of B and a second retry to probe whether the
fix holds across re-entrant cascades. Parent template: `coord.md`.
Child template: `impl-issue.md` (both from the walkthrough).

---

## Section 1: Transcript

`LocalBackend` throughout; `sync_status` / `machine_id` omitted.

### Turn 1 -- AGENT: init + first tick

```bash
koto init coord --template coord.md --var plan_path=PLAN-chain6.md
koto next coord
```

KOTO (init):
```json
{"action":"initialized","workflow":"coord","state":"plan_and_await","template":"coord.md"}
```

KOTO (first tick) returns the standard `evidence_required` response
with `expects.fields.tasks` + `item_schema` (identical to walkthrough
Interaction 2; `scheduler: null`, no `reserved_actions`). Elided.

### Turn 2 -- AGENT submits the 6-task chain

```json
{
  "tasks": [
    {"name": "A", "vars": {"ISSUE_NUMBER": "301"}},
    {"name": "B", "vars": {"ISSUE_NUMBER": "302"}, "waits_on": ["A"]},
    {"name": "C", "vars": {"ISSUE_NUMBER": "303"}, "waits_on": ["B"]},
    {"name": "D", "vars": {"ISSUE_NUMBER": "304"}, "waits_on": ["C"]},
    {"name": "E", "vars": {"ISSUE_NUMBER": "305"}, "waits_on": ["D"]},
    {"name": "F", "vars": {"ISSUE_NUMBER": "306"}, "waits_on": ["E"]}
  ]
}
```

```
koto next coord --with-data @tasks.json
```

### Turn 3 -- KOTO

Pre-append validation (R0, R3-R6, R8 vacuous, R9) passes.
`EvidenceSubmitted` appended. Scheduler builds DAG: A `Ready`; B-F
`BlockedByDep`. Spawns `coord.A`. `coord.A`'s
`WorkflowInitialized.spawn_entry = {template: "impl-issue.md", vars:
{ISSUE_NUMBER: "301"}, waits_on: []}`.

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done", "type": "children-complete", "category": "temporal",
    "output": {
      "total": 6, "completed": 0, "pending": 6,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 5, "spawn_failed": 0,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false, "needs_attention": false,
      "children": [
        {"name": "coord.A", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.B", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.A"]},
        {"name": "coord.C", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.B"]},
        {"name": "coord.D", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.C"]},
        {"name": "coord.E", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.D"]},
        {"name": "coord.F", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.E"]}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": ["coord.A"],
    "materialized_children": [
      {"name": "coord.A", "outcome": "pending", "state": "working", "ready_to_drive": true}
    ],
    "already": [], "blocked": ["coord.B","coord.C","coord.D","coord.E","coord.F"],
    "skipped": [], "errored": [], "warnings": [],
    "feedback": {
      "entries": {
        "A": {"outcome": "accepted"},
        "B": {"outcome": "blocked", "waits_on": ["A"]},
        "C": {"outcome": "blocked", "waits_on": ["B"]},
        "D": {"outcome": "blocked", "waits_on": ["C"]},
        "E": {"outcome": "blocked", "waits_on": ["D"]},
        "F": {"outcome": "blocked", "waits_on": ["E"]}
      },
      "orphan_candidates": []
    }
  }
}
```

### Turn 4 -- AGENT drives A to success

```
koto next coord.A --with-data '{"status": "complete"}'   # -> done
koto next coord
```

### Turn 5 -- KOTO re-tick

A terminal-success. Scheduler spawns B. C-F stay blocked.
`coord.B.spawn_entry = {template: "impl-issue.md", vars: {ISSUE_NUMBER:
"302"}, waits_on: ["A"]}`.

(Response elided -- identical shape to Turn 3 with A/B now live, C-F
blocked, `feedback.entries: {}` because no `tasks` payload submitted
this tick.)

### Turn 6 -- AGENT drives B to FAILURE

```
koto next coord.B --with-data '{"status": "blocked"}'   # -> done_blocked
koto next coord
```

### Turn 7 -- KOTO: cascade tick

B is terminal-failure. Runtime reclassification (CD9 Part 5) under
`failure_policy: skip_dependents` walks the transitive closure of B on
the task DAG: `{C, D, E, F}`. For each dependent with no state file
yet, the scheduler materializes a skip marker directly via
`init_state_file` into `impl-issue.md`'s
`skipped_due_to_dep_failure` terminal state. Each skip marker gets a
fresh `spawn_entry` matching the CURRENT submission's entry for that
name (per the Round 3 fix). `BatchFinalized` appended (first
`all_complete: true`). Aggregate `needs_attention: true` fires the
transition `plan_and_await -> analyze_failures`.

```json
{
  "action": "evidence_required",
  "state": "analyze_failures",
  "directive": "At least one child failed or was skipped. ...",
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {"decision": {"type": "enum", "values": ["give_up","acknowledge"], "required": false}}
  },
  "blocking_conditions": [{
    "name": "done", "type": "children-complete", "category": "temporal",
    "output": {
      "total": 6, "completed": 6, "pending": 0,
      "success": 1, "failed": 1, "skipped": 4, "blocked": 0, "spawn_failed": 0,
      "all_complete": true, "all_success": false,
      "any_failed": true, "any_skipped": true, "needs_attention": true,
      "children": [
        {"name": "coord.A", "state": "done",                       "complete": true, "outcome": "success"},
        {"name": "coord.B", "state": "done_blocked",               "complete": true, "outcome": "failure", "failure_mode": true, "reason": "Issue 302 hit an unresolvable blocker during implementation.", "reason_source": "failure_reason"},
        {"name": "coord.C", "state": "skipped_due_to_dep_failure", "complete": true, "outcome": "skipped", "skipped_because": "coord.B", "skipped_because_chain": ["coord.B"]},
        {"name": "coord.D", "state": "skipped_due_to_dep_failure", "complete": true, "outcome": "skipped", "skipped_because": "coord.C", "skipped_because_chain": ["coord.C","coord.B"]},
        {"name": "coord.E", "state": "skipped_due_to_dep_failure", "complete": true, "outcome": "skipped", "skipped_because": "coord.D", "skipped_because_chain": ["coord.D","coord.C","coord.B"]},
        {"name": "coord.F", "state": "skipped_due_to_dep_failure", "complete": true, "outcome": "skipped", "skipped_because": "coord.E", "skipped_because_chain": ["coord.E","coord.D","coord.C","coord.B"]}
      ]
    }
  }],
  "reserved_actions": [{
    "name": "retry_failed",
    "description": "Re-queue failed and skipped children. Dependents are included by default.",
    "payload_schema": {
      "children": {"type": "array<string>", "required": true},
      "include_skipped": {"type": "boolean", "required": false, "default": true}
    },
    "applies_to": ["coord.B","coord.C","coord.D","coord.E","coord.F"],
    "invocation": "koto next coord --with-data '{\"retry_failed\": {\"children\": [\"coord.B\"]}}'"
  }],
  "scheduler": null
}
```

Observation (probe 1): `koto query coord.C --events` at this point shows
`WorkflowInitialized { spawn_entry: {template: "impl-issue.md", vars:
{ISSUE_NUMBER: "303"}, waits_on: ["B"]} }` followed by
`Transitioned { target: "skipped_due_to_dep_failure" }` in the same
atomic init bundle. Same shape for D/E/F. `spawn_entry` is present on
every skip marker. This is the Round 3 fix in action.

### Turn 8 -- AGENT submits retry of B

Per CD9 Part 4, `include_skipped: true` (default) closes the retry
downward: naming B re-queues the entire `{C, D, E, F}` closure.

```
koto next coord --with-data '{"retry_failed": {"children": ["coord.B"]}}'
```

### Turn 9 -- KOTO: retry tick, cascade unwind begins

`handle_retry_failed`:
1. Append `EvidenceSubmitted { retry_failed: {...} }` to coord.
2. Append clearing `EvidenceSubmitted { retry_failed: null }`.
3. Downward closure of B = `{B, C, D, E, F}`. For each:
   - `coord.B` outcome `failure` -> append `Rewound` targeting
     `working`. State file rewound in place (not deleted).
   - `coord.C` outcome `skipped`, `skipped_marker: true` -> delete
     state file, re-respawn from the CURRENT submission's entry for C
     (which is the last accepted `tasks` payload, Turn 2's). `waits_on:
     ["B"]` still unsatisfied (B is now `Running` again). Per Decision
     5.2.3 "retry-induced respawns commit atomically per tick," the new
     state file for C is written in the same atomic batch as B's
     rewind. C's new `spawn_entry` is identical to the Turn 2 entry
     (same task DAG). Because B is not terminal-success, C's
     `ready_to_drive: false`.
   - `coord.D`, `coord.E`, `coord.F` same treatment as C, each with the
     chain of dependencies. Each respawned as `working` with
     `ready_to_drive: false`.

Advance loop: at `analyze_failures`. `evidence.retry_failed: present`
matches the un-merged payload -> transition to `plan_and_await`.

Scheduler runs on `plan_and_await`. Reclassifies:
- A: Terminal success.
- B: Running (just rewound).
- C: Running on disk but `ready_to_drive: false` (waits_on B not
  terminal-success).
- D, E, F: same.

No fresh spawns this tick -- every name already has a state file.

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done", "type": "children-complete", "category": "temporal",
    "output": {
      "total": 6, "completed": 1, "pending": 5,
      "success": 1, "failed": 0, "skipped": 0, "blocked": 0, "spawn_failed": 0,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false, "needs_attention": false,
      "children": [
        {"name": "coord.A", "state": "done",    "complete": true,  "outcome": "success"},
        {"name": "coord.B", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.C", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.D", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.E", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.F", "state": "working", "complete": false, "outcome": "pending"}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": [],
    "materialized_children": [
      {"name": "coord.A", "outcome": "success", "state": "done",    "ready_to_drive": false},
      {"name": "coord.B", "outcome": "pending", "state": "working", "ready_to_drive": true},
      {"name": "coord.C", "outcome": "pending", "state": "working", "ready_to_drive": false},
      {"name": "coord.D", "outcome": "pending", "state": "working", "ready_to_drive": false},
      {"name": "coord.E", "outcome": "pending", "state": "working", "ready_to_drive": false},
      {"name": "coord.F", "outcome": "pending", "state": "working", "ready_to_drive": false}
    ],
    "already": ["coord.A","coord.B","coord.C","coord.D","coord.E","coord.F"],
    "blocked": [], "skipped": [], "errored": [], "warnings": [],
    "feedback": {"entries": {}, "orphan_candidates": []}
  }
}
```

Critical observation: gate output counts C/D/E/F as `pending` (state
`working`, not terminal) even though `ready_to_drive: false`. The batch
view and the ledger give two consistent-but-different cuts:
- `children[].outcome: pending` -> "on disk, not yet finished."
- `materialized_children[].ready_to_drive: false` -> "don't dispatch."

An agent dispatching on the ledger and respecting `ready_to_drive`
drives only B this tick. This is the Round 3 contract.

### Turn 10 -- AGENT drives B to success

```
koto next coord.B --with-data '{"status": "complete"}'   # -> done
koto next coord
```

### Turn 11 -- KOTO: C becomes ready

Scheduler reclassifies. B terminal-success; C's waits_on satisfied.
Per Decision 5.2.2, `ready_to_drive` is derived fresh each tick; C's
flag flips `false -> true`. C's state file is already on disk (from
Turn 9 respawn), so no new `init_state_file` call this tick. D/E/F
still waiting transitively.

```json
{
  "scheduler": {
    "spawned_this_tick": [],
    "materialized_children": [
      {"name": "coord.A", "outcome": "success", "state": "done",    "ready_to_drive": false},
      {"name": "coord.B", "outcome": "success", "state": "done",    "ready_to_drive": false},
      {"name": "coord.C", "outcome": "pending", "state": "working", "ready_to_drive": true},
      {"name": "coord.D", "outcome": "pending", "state": "working", "ready_to_drive": false},
      {"name": "coord.E", "outcome": "pending", "state": "working", "ready_to_drive": false},
      {"name": "coord.F", "outcome": "pending", "state": "working", "ready_to_drive": false}
    ],
    "already": ["coord.A","coord.B","coord.C","coord.D","coord.E","coord.F"],
    "blocked": [], "skipped": [], "errored": [], "warnings": [],
    "feedback": {"entries": {}, "orphan_candidates": []}
  }
}
```
(Gate output and aggregates elided; `needs_attention: false`,
`all_complete: false`.)

### Turn 12 -- AGENT drives C to success; re-ticks; D ready; drives D; ...

The ripple continues linearly: D -> E -> F. Each tick one
`ready_to_drive` flips `true`, agent drives it, re-ticks. No deletes,
no respawns -- the Turn 9 bundle already put every skip marker back on
the real-template happy path.

### Turn 13 -- KOTO final

After F completes, parent tick sees `all_success: true`. Fresh
`BatchFinalized` event (supersedes the one from Turn 7). Transition to
`summarize`.

```json
{
  "action": "done",
  "state": "summarize",
  "directive": "Write a summary ...",
  "is_terminal": true,
  "batch_final_view": {
    "phase": "final",
    "summary": {"total": 6, "success": 6, "failed": 0, "skipped": 0, "pending": 0, "blocked": 0, "spawn_failed": 0},
    "tasks": [
      {"name": "A", "child": "coord.A", "outcome": "success"},
      {"name": "B", "child": "coord.B", "outcome": "success"},
      {"name": "C", "child": "coord.C", "outcome": "success"},
      {"name": "D", "child": "coord.D", "outcome": "success"},
      {"name": "E", "child": "coord.E", "outcome": "success"},
      {"name": "F", "child": "coord.F", "outcome": "success"}
    ],
    "ready": [], "blocked": [], "skipped": [], "failed": []
  }
}
```

---

## Section 2: Edge-case probes

### Probe 1 -- `spawn_entry` persistence through multi-tick cascade

At Turn 7 (post-cascade), `koto query coord.C --events`:

```json
{
  "workflow": "coord.C",
  "events": [
    {"type": "WorkflowInitialized", "epoch": 1, "spawn_entry": {"template": "impl-issue.md", "vars": {"ISSUE_NUMBER": "303"}, "waits_on": ["B"]}},
    {"type": "Transitioned", "from": "working", "to": "skipped_due_to_dep_failure"}
  ]
}
```

At Turn 9 (post-retry), `koto query coord.C --events`:

```json
{
  "workflow": "coord.C",
  "events": [
    {"type": "WorkflowInitialized", "epoch": 1, "spawn_entry": {"template": "impl-issue.md", "vars": {"ISSUE_NUMBER": "303"}, "waits_on": ["B"]}},
    {"type": "Transitioned", "from": "working", "to": "skipped_due_to_dep_failure"},
    {"type": "EpochBumped", "from": 1, "to": 2},
    {"type": "WorkflowInitialized", "epoch": 2, "spawn_entry": {"template": "impl-issue.md", "vars": {"ISSUE_NUMBER": "303"}, "waits_on": ["B"]}}
  ]
}
```

Per Decision 5.2.1: the epoch-2 `WorkflowInitialized` carries a fresh
`spawn_entry` matching the CURRENT submission's entry for C. Because
the task DAG was unchanged, the two snapshots are byte-identical --
but they were independently written. The Round 3 fix holds.

R8 on a subsequent resubmit of the 6-task list at this point: for C,
the predicate "child exists on disk" is true (epoch-2 state file is
live), `spawn_entry` is present, comparison passes. Not vacuous. This
is the core correctness property the fix establishes.

Children mid-respawn: if an agent ticks the parent with a second
submission in the tiny window during Turn 9's atomic commit, the
scheduler sees C in the current tick's respawn set and its replacement
not yet committed -> R8 comparison skipped for C, feedback carries
`{outcome: "respawning"}`. Under the advisory flock, the second
submitter serializes behind Turn 9; by the time its advance loop runs,
the commit is done and R8 evaluates normally against the new epoch-2
snapshot. The `Respawning` outcome remains in the enum for a real
R8-vacuous condition that cannot arise on `LocalBackend` under the
flock but can under `CloudBackend` when the sync observer is mid-pull.

### Probe 2 -- R8 during cascade

Three submission points:

1. **Before retry, post-cascade (between Turn 7 and Turn 8).** All
   children have state files: B at `done_blocked`, C/D/E/F at
   `skipped_due_to_dep_failure` with their Turn-7-era `spawn_entry`
   snapshots. Resubmitting the identical 6-task list: R8 compares
   entry-by-entry against the on-disk `spawn_entry` on each child;
   every comparison passes. `feedback.entries` shows
   `{outcome: "already_skipped"}` for C/D/E/F (they are on skip
   markers, not running -- `AlreadySkipped` variant), `already_terminal`
   for A, `already_terminal (failure)` ... wait, B is
   terminal-failure. Per `EntryOutcome::AlreadyTerminal` doc "terminal
   non-failure state," B would NOT match `AlreadyTerminal`. B would
   match `AlreadyRunning`? No -- B is terminal. This reveals a small
   gap: B's terminal-failure state has no dedicated `Already*` variant
   (Accepted/AlreadyRunning/AlreadySkipped/AlreadyTerminal/Blocked/
   Errored/Respawning). The natural mapping is `AlreadyTerminal` with
   the doc generalized to "terminal state" without the non-failure
   qualifier -- see Gap G-A below.

   Attempting a mutation of B's `vars` in this resubmission: R8 fires
   `SpawnedTaskMutated` -- B is on disk, `spawn_entry` is present,
   mismatch rejected pre-append. Same for C/D/E/F. R8 applies to every
   row.

2. **During retry tick (inside Turn 9 execution).** No other submission
   can land inside the same tick under the advisory flock. An
   overlapping call fails fast with `concurrent_tick` / exit 1
   (retryable). On retry after Turn 9's commit, R8 evaluates against
   the freshly-written epoch-2 `spawn_entry` for C/D/E/F. Vacuous-
   mid-respawn is not observable on `LocalBackend`.

3. **After cascade settles (Turn 9 committed, Turn 10+).** Every child
   in the cascade has a fresh on-disk `spawn_entry`. R8 applies
   uniformly. Mutation attempts on B, C, D, E, or F all reject with
   `SpawnedTaskMutated`; mutation on A always rejects (A has its
   original epoch-1 `spawn_entry`).

**Finding:** the Round 3 fix eliminates the Turn 9 R8-vacuous window
that Round 2 C2 G3 identified. Under the flock, no submission arrives
during the atomic respawn bundle; after the bundle commits, R8 applies
against the new `spawn_entry`. The window Gap G3 flagged is closed for
`LocalBackend`; for `CloudBackend`, `Respawning` handles it.

### Probe 3 -- `EntryOutcome::Respawning` visibility window

The enum is documented as "target child is mid-respawn this tick; R8
comparison is vacuous until the new state file commits. Agents retry;
the next tick re-evaluates against the new `spawn_entry`."

Duration analysis:
- On `LocalBackend` under the CD12 Q3 advisory flock, the respawn
  bundle commits atomically inside a single `handle_next` call. No
  second submitter can observe the mid-commit state. Practical window:
  **zero ticks** for local users.
- On `CloudBackend`, Decision 12's push-parent-first ordering means the
  parent log commit precedes the child writes. A remote observer
  reading the parent log between the parent push and the child push
  sees `retry_failed` evidence but no new child state files. If that
  observer submits at that moment, the scheduler classifies the target
  child as "in respawn set, replacement not yet written" and emits
  `Respawning`. Duration: one sync cycle, bounded by the remote
  observer's pull cadence.

**The outcome is per-tick, not sticky.** The next `koto next parent`
tick re-derives classifications from disk; once the new state file is
committed, `Respawning` is replaced by the normal outcome
(`AlreadyRunning` for a respawned-and-now-driving C, `AlreadyTerminal`
for one that finished). No persistence; no timer. An agent that sees
`Respawning` simply re-ticks.

### Probe 4 -- `scheduler.feedback.entries` taxonomy during cascade

At Turn 9's tick, if the agent ALSO submitted `tasks` (they can't --
CD9 Part 4 rejects mixed payloads), what would feedback look like? The
question is moot for the retry tick but useful for the tick AFTER the
retry when the agent resubmits tasks to add a new name.

Hypothetical resubmission of the 6-task list (no new names) at Turn 10
(post-retry, B has not yet been driven):

```json
"feedback": {
  "entries": {
    "A": {"outcome": "already_terminal"},
    "B": {"outcome": "already_running"},
    "C": {"outcome": "already_running"},
    "D": {"outcome": "already_running"},
    "E": {"outcome": "already_running"},
    "F": {"outcome": "already_running"}
  },
  "orphan_candidates": []
}
```

Every entry gets a distinct outcome. `AlreadyRunning` covers C/D/E/F
because they're on disk at `working` (non-terminal), even though
`ready_to_drive: false` on C/D/E/F. This is a subtle point:
`AlreadyRunning` does NOT imply "a worker is currently driving it"; it
means "on disk, non-terminal." Workers key dispatch on
`ready_to_drive`, not on the feedback variant.

**Finding:** the taxonomy is clear per-entry, but the phrase
`AlreadyRunning` overloads "running = being driven." Agents must read
this as "already on disk, non-terminal" and consult
`materialized_children[].ready_to_drive` for dispatch eligibility.
Update `koto-user` skill to clarify.

Cascade-mid: for a 6-entry batch submitted AT Turn 7 (between cascade
materialization and retry), feedback shows:

```json
{
  "A": {"outcome": "already_terminal"},
  "B": {"outcome": "already_terminal"},   // or {outcome: "already_failed"} -- see Gap G-A
  "C": {"outcome": "already_skipped"},
  "D": {"outcome": "already_skipped"},
  "E": {"outcome": "already_skipped"},
  "F": {"outcome": "already_skipped"}
}
```

### Probe 5 -- Retry fails mid-cascade

Agent retries B (Turn 8), B rewinds, Turn 9 commits C/D/E/F respawn
bundle. Agent drives B. B fails AGAIN:

```
koto next coord.B --with-data '{"status": "blocked"}'   # -> done_blocked again
koto next coord
```

Cascade tick N+1: B is terminal-failure AGAIN. Runtime reclassification
walks B's closure: `{C, D, E, F}`. C is at `working` (non-terminal) on
its real template per Turn 9 respawn. C is NOT a skip marker --
`skipped_marker: true` is false on `working`. Per CD9 Part 5, the
reclassification for real-template running children whose upstream
just flipped to failure is: "stale respawn" detected -> delete the
child's state file, respawn as skip marker from the CURRENT submission
entry.

This is the CD9 Part 5 case the Round 1 synthesis was worried about:
"re-skip a running real-template child." The Round 3 fix makes it
concrete:
- C's current state file is at `working` with epoch-2
  `WorkflowInitialized.spawn_entry` from Turn 9. The scheduler deletes
  C's state file and runs `init_state_file` again, epoch 3, with a
  fresh `spawn_entry` from the current submission's C entry.
- The new epoch-3 init bundle transitions directly into
  `skipped_due_to_dep_failure`.
- D/E/F same (their prior state file was at `working`, transitive skip
  applies, new skip markers).

**Partial-work concern.** If C was mid-driven -- e.g., the agent had
started to run `coord.C`'s implementation steps but not submitted
evidence -- the delete-and-respawn is destructive of any in-progress
file changes in the agent's working tree. Koto itself has no partial
state to lose: evidence is only persisted on `koto next` calls. So if
C had evidence-submitted, C would be terminal (either `done` or
`done_blocked`) and the "running real-template child" case wouldn't
apply. But if C had *pending* agent-side work (code changes, running
tests) not yet submitted to koto, the agent's in-tree work is
orphaned. The scheduler doesn't know about that work and can't
preserve it.

**Finding:** CD9 Part 5's delete-and-respawn of a running
real-template child is NOT partial-work-preserving. Agents must treat
`reclassified-during-cascade` as "cancel any in-progress work on this
child." The `Respawning` outcome is the signal during the transient
window, but after commit, agents discover the respawn only by
re-querying the child. Suggest adding an explicit `Reclassified`
outcome or a `scheduler.respawned_this_tick` ledger alongside
`spawned_this_tick` so agents can notice mid-flight invalidations.

### Probe 6 -- CD9 Part 5 "re-skip a running real-template child"

Probe 5 above validates this. The runtime reclassification combined
with Round 3's "fresh `spawn_entry` on every respawn" correctly
handles it:
- C's old epoch-2 `spawn_entry` is preserved in the event log (audit
  trail remains intact).
- C's new epoch-3 `spawn_entry` is the basis for future R8 comparisons.
- `ready_to_drive` is derived fresh each tick; C post-respawn into a
  skip marker is terminal, so `ready_to_drive: false` (terminal
  children are never drivable).
- Agent observing the new cascade tick sees C at
  `skipped_due_to_dep_failure`, same as Turn 7. The scenario is
  cleanly repeatable.

**One subtle point:** the scheduler must detect "the current state
file's `waits_on` ancestor has flipped to failure AND this child is
non-terminal" on every tick. This is a loop over `materialized_children
x task DAG` per tick, O(children x waits_on). For a 6-child chain this
is trivial; for a 1000-child batch with long chains, worth budgeting.

### Probe 7 -- Multiple retries in sequence

After Probe 5's second B failure, the agent retries B again. Same
mechanics:

```
koto next coord --with-data '{"retry_failed": {"children": ["coord.B"]}}'
```

- R10 validates: B exists, outcome `failure`. `coord.C`, D, E, F are
  eligible too via `include_skipped: true`.
- `handle_retry_failed` runs. Downward closure of B = `{B, C, D, E, F}`.
  - B rewound in place (state file stays, appends `Rewound`).
  - C/D/E/F at `skipped_due_to_dep_failure` -> delete-and-respawn,
    epoch 4 (from the re-failure cascade which was epoch 3). New
    `spawn_entry` from the still-current task submission.

`BatchFinalized` interaction: the Turn-7 `BatchFinalized` and the
post-re-failure `BatchFinalized` (from the second cascade's
all_complete tick) are both in the event log. `handle_next` attaches
`batch_final_view` only when the CURRENT tick produced an
`all_complete: true` transition -- so on the retry tick, no fresh
`BatchFinalized` is written (the current tick does NOT reach
all_complete; it routes back to `plan_and_await`). The prior
`BatchFinalized` events are historical. When the batch finally reaches
`all_success: true` after the second successful retry, a NEW
`BatchFinalized` is appended (the Turn-13-analog), and the terminal
response carries THAT one's `batch_final_view`.

**Finding:** `BatchFinalized` is not "invalidated" by a subsequent
retry -- it's a historical event. The terminal-response attachment
rule is "the most recent one on the log," which is naturally the one
written on the successful path. This is the Round 2 Pair C2 Turn-26
behavior scaled up.

### Probe 8 -- Redaction sentinel

Suppose a task's `vars` carries a redactable field (e.g., `API_KEY`).
Round 1's `redaction_patterns` setting (Decision 10) strips the value
from `SpawnedTaskMutated.changed_fields` error payloads. Attempt a
mutation on `coord.B`'s `API_KEY`:

```json
{"tasks": [
  {"name": "A", "vars": {"ISSUE_NUMBER": "301"}},
  {"name": "B", "vars": {"ISSUE_NUMBER": "302", "API_KEY": "new-sk-..."}, "waits_on": ["A"]},
  ...
]}
```

R8 rejects pre-append:

```json
{
  "action": "error",
  "error": {
    "code": "invalid_submission",
    "message": "Spawned task mutated: coord.B",
    "batch": {
      "kind": "spawned_task_mutated",
      "task": "B",
      "changed_fields": [
        {"field": "vars.API_KEY", "spawned_value": "[REDACTED]", "submitted_value": "[REDACTED]"}
      ]
    }
  }
}
```

Per Decision 10 "Redaction sentinel": both `spawned_value` and
`submitted_value` are the literal string `"[REDACTED]"`. The sentinel
is symmetric -- the agent cannot learn either the old value or the
attempted new value. If the field is non-redactable (e.g.,
`ISSUE_NUMBER`), real values appear in both cells. Verified consistent
with design.

### Probe 9 -- `OmittedPriorTask` warning

Agent resubmits DURING the cascade unwind (Turn 10, between Probe-5
second retry commit and driving B) but OMITS `coord.F`:

```json
{
  "tasks": [
    {"name": "A", "..."},
    {"name": "B", "..."},
    {"name": "C", "..."},
    {"name": "D", "..."},
    {"name": "E", "..."}
  ]
}
```

Per CD10 "Removal is deferred": omission is a no-op. `coord.F` stays in
the effective task set (still on disk at `working`/skip/whatever from
the last respawn).

The scheduler emits a `SchedulerWarning::OmittedPriorTask { task: "F" }`
on this tick. Response:

```json
{
  "scheduler": {
    "...": "...",
    "warnings": [
      {"kind": "omitted_prior_task", "task": "F"}
    ],
    "feedback": {
      "entries": {
        "A": {"outcome": "already_terminal"},
        "B": {"outcome": "already_running"},
        "C": {"outcome": "already_running"},
        "D": {"outcome": "already_running"},
        "E": {"outcome": "already_running"}
      },
      "orphan_candidates": []
    }
  }
}
```

Per CD10: "Informational only (omission is not a cancellation signal
per Decision 10), but agents are told rather than left to infer
silently." The warning fires once per omitted name per tick; F is still
on disk and still counted in the gate output's `total: 6`. Verified.

---

## Section 3: Findings

### Finding A -- `EntryOutcome::AlreadyTerminal` docs exclude failure states

- **Observation:** `EntryOutcome::AlreadyTerminal` doc reads "Child
  already exists in a terminal non-failure state." But during a
  cascade probe (Probe 4, Turn-7 hypothetical resubmission), B is at
  `done_blocked` (terminal-failure). No variant in the enum covers
  "terminal-failure" distinctly -- `AlreadyRunning` is wrong (not
  running), `AlreadySkipped` is wrong (not skipped), `AlreadyTerminal`
  is excluded by the doc. Implementers will have to pick one.
- **Location:** design lines 1917-1932 (`EntryOutcome` definition).
- **Severity:** should-fix (mild; taxonomy ambiguity).
- **Proposed resolution:** Either (a) generalize `AlreadyTerminal` doc
  to "Child already exists in a terminal state" (success or failure
  both covered), or (b) add an `AlreadyFailed` variant. Option (a) is
  additive and matches the natural implementer reading; option (b) is
  more precise but expands the enum. Round 3 should pick one and
  update the doc.

### Finding B -- `AlreadyRunning` overloads "being driven" vs. "on disk non-terminal"

- **Observation:** After Turn 9 commits the cascade respawn, C/D/E/F
  are on disk at `working` with `ready_to_drive: false`. A resubmit of
  tasks reports `AlreadyRunning` for each -- but no worker is actually
  driving them; they're parked. Agents reading feedback without
  consulting `materialized_children[].ready_to_drive` will dispatch
  phantom workers.
- **Location:** design lines 1919-1920; `koto-user` skill guidance.
- **Severity:** should-fix (documentation + skill update).
- **Proposed resolution:** Rename the doc-string on `AlreadyRunning` to
  "Child already exists and is non-terminal (may or may not be ready to
  drive; consult `materialized_children[].ready_to_drive`)." Update
  `koto-user` skill to include an explicit "always key dispatch on
  ready_to_drive" rule. Do NOT split into two variants -- the
  drivability signal already lives on `MaterializedChild`.

### Finding C -- No explicit signal for "child was reclassified this tick"

- **Observation:** Probe 5 (second-failure cascade) deletes C/D/E/F's
  `working` state files and respawns them as skip markers. The
  scheduler's tick response includes the new materialized_children
  ledger but does NOT surface a dedicated "reclassified" list. Agents
  discover the reclassification only by comparing state against a
  prior snapshot, or by reading `children[].outcome` shift from
  `pending -> skipped`. For long-running agents watching
  `spawned_this_tick`, a mid-flight invalidation of C that required
  the agent to cancel its current C-work is silent.
- **Location:** design Decision 5 / CD9 Part 5; `Scheduler` response
  shape.
- **Severity:** nice-to-have.
- **Proposed resolution:** Add an optional
  `scheduler.reclassified_this_tick: Vec<{name, from_outcome,
  to_outcome, reason}>` field to the response. Empty on most ticks;
  populated during cascade re-cascades. Aids agent-side
  cancellation-of-in-progress-work flows. Non-blocking for v1.

### Finding D -- Partial-work loss on "re-skip of running real-template child"

- **Observation:** Probe 5 shows that when C was mid-driven (agent
  has uncommitted file edits, running tests, etc. but has not
  submitted evidence), the delete-and-respawn path cleanly removes
  C's state file. Koto itself is consistent; the agent's in-tree
  work is orphaned without warning. CD9 Part 5 is correct on the koto
  side; the agent-side cost is undocumented.
- **Location:** CD9 Part 5; `koto-user` skill "running koto-backed
  workflows."
- **Severity:** should-fix (documentation).
- **Proposed resolution:** Document in `koto-user` skill: "If a parent
  tick during cascade reclassifies a running child you are actively
  driving, your uncommitted work is orphaned; the skip marker does not
  preserve partial state. Drivers of batch children SHOULD check
  `ready_to_drive` before resuming work on a child they left parked,
  and be prepared to discard work if the outcome has flipped from
  `pending -> skipped`." Pair with Finding C's
  `reclassified_this_tick` so the signal is machine-readable.

### Finding E -- `BatchFinalized` sequence across retries is historical, not invalidated

- **Observation:** Probe 7 confirms `BatchFinalized` events accumulate
  across retry rounds. The terminal response attaches the most recent
  one. No explicit invalidation rule; no per-retry `BatchFinalized`
  supersession semantics beyond "most recent wins by order in log."
  This is correct behavior but implicit in the design.
- **Location:** Decision 2 / Decision 5; `handle_next` attachment
  rules.
- **Severity:** nice-to-have (documentation).
- **Proposed resolution:** Add a note in Decision 5.2 or Decision 5.5:
  "Multiple `BatchFinalized` events may exist in the log after a
  retry round that re-enters `plan_and_await` and later re-reaches
  `all_complete`. `handle_next` attaches `batch_final_view` using the
  event at the current tick's transition -- i.e., the one written on
  the successful terminal path. Historical `BatchFinalized` events
  remain for audit."

### Finding F -- `Respawning` outcome is cloud-only in practice

- **Observation:** Under Decision 12's advisory flock on
  `LocalBackend`, the mid-respawn window during which R8 would be
  vacuous is unobservable -- the second submitter serializes behind
  the retry tick. `Respawning` as a feedback variant is therefore
  reachable only under `CloudBackend` when a remote observer submits
  between push-parent and push-child. The design's wording ("another
  submission arriving in the same window") is geography-neutral, but
  in practice the outcome is cloud-specific.
- **Location:** design lines 1960-1972.
- **Severity:** nice-to-have.
- **Proposed resolution:** Clarify in the narrative: "Under
  `LocalBackend` with the advisory flock, this window is zero. Under
  `CloudBackend`, it is bounded by the remote observer's sync
  cadence." Helps readers build a correct mental model.

### Finding G -- The Round-2 F2 gap is closed

- **Observation:** Round 2 Pair C2 F2 raised: "spawn_entry lifecycle
  through delete-and-respawn unspecified; CD10's R8 relies on on-disk
  spawn_entry; CD9's runtime reclassification blows it away." Round 3
  Decision 5.2 clarifications (spawn_entry on respawn is the CURRENT
  submission's entry; ready_to_drive gate; atomic per-tick respawn
  commit) directly resolve this. Every probe above validates the fix:
  - Probe 1: skip markers carry `spawn_entry`. Verified.
  - Probe 2: R8 applies uniformly after cascade commits. Verified.
  - Probe 3: `Respawning` covers the only remaining vacuous window.
    Verified.
  - Probe 5: second-failure cascade cleanly re-respawns. Verified.
  - Probe 7: sequential retries use the same mechanism. Verified.
- **Severity:** resolved.
- **Proposed resolution:** None; the Round 3 fix holds.

---

## Section 4: Cross-decision interaction summary

The Round 3 fix establishes a three-part contract that closes Round 2
F2:

1. **`spawn_entry` on every respawn** (Decision 5.2.1). No tick
   produces a child without a fresh `spawn_entry`. R8's "exists on
   disk" predicate now reliably finds a snapshot to compare against
   for every non-deleted child.

2. **`ready_to_drive` dispatch gate** (Decision 5.2.2). Separates "on
   disk" from "drivable." This is load-bearing: without it, agents
   would dispatch phantom workers against respawned-but-parked
   children in the cascade-unwind window (Turn 9 -> Turn 13).

3. **Atomic per-tick respawn commit** (Decision 5.2.3). Combined with
   CD12 Q3's advisory flock, eliminates the cross-submitter
   R8-vacuous window on `LocalBackend`. `Respawning` covers the
   `CloudBackend` case.

Residual items (Findings A-F above) are taxonomy/documentation
polish, not correctness blockers. The 6-task deep-chain cascade with
two sequential failure-and-retry rounds drives cleanly from
`plan_and_await` through `analyze_failures` and back, then
terminal-success, with no ambiguity on the happy path and no
silent-error-path breaks on the failure path.

The only remaining *correctness* concern is Finding D (partial-work
loss on mid-flight re-skip), which is an agent-side concern koto
cannot solve unilaterally -- but agents need to know about it. That
belongs in `koto-user` skill updates rather than the design doc.
