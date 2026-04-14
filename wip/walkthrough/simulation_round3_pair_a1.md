# Round 3, Pair A1 — verifying the F1 fix (ready_to_drive + Respawning)

Scenario: 4-issue diamond `A → B, A → C, B → D, C → D` with
`failure_policy: skip_dependents`. Same templates as the canonical
reference (`coord.md` + `impl-issue.md`). Drives through initial
success of A, failure of B, synthesis of D as skip marker, then
exercises the revised retry path. Probes the design's response to
Round 2's F1 blocker: that a retry-induced respawn of D would land
in `materialized_children` with `ready_to_drive: false` until B's
re-run settles, plus the `EntryOutcome::Respawning` variant, plus
the supersede-on-retry semantics of `BatchFinalized`.

Pre-state established off-camera (same as Round 2 Pair A1, Turns 1-6):
- `coord` at `analyze_failures`
- `coord.issue-A`, `coord.issue-C` terminal-success
- `coord.issue-B` terminal-failure (`done_blocked`, `failure_mode: true`)
- `coord.issue-D` synthesized as skip marker at
  `skipped_due_to_dep_failure` with `skipped_marker: true`
- One `BatchFinalized` event on the parent log reflecting the
  post-first-pass snapshot

---

## Section 1: Transcript

### Turn 1 — AGENT: submit retry_failed

```bash
koto next coord --with-data '{"retry_failed": {"children": ["coord.issue-B"], "include_skipped": true}}'
```

KOTO: `handle_retry_failed` intercepts before `advance_until_stop`.
R10 validates: `coord.issue-B` exists, outcome `failure`; single-
child retry set is non-empty; no mixed keys. Canonical sequence
runs: (a) append `EvidenceSubmitted { retry_failed: {...} }`, (b)
append clearing `EvidenceSubmitted { retry_failed: null }`, (c) for
each child in the downward closure of `{B}`:
- B (outcome `failure`) → append `Rewound { target_state: "working" }`
  on `coord.issue-B`. New epoch.
- D (outcome `skipped`, `skipped_marker: true`, pulled in via
  `include_skipped: true` closure) → delete-and-respawn. The new
  state file's `WorkflowInitialized.spawn_entry` snapshots the
  entry that caused this respawn (i.e., the most recent accepted
  `tasks` entry for D from the original submission — vars
  `{"ISSUE_NUMBER": "104"}`, waits_on `["issue-B", "issue-C"]`).

Advance loop runs at `analyze_failures`: the uncleared submission's
`evidence.retry_failed: present` matcher fires the transition back
to `plan_and_await`. Scheduler evaluates at `plan_and_await`:
classify — A: terminal-success; B: non-terminal (working, new
epoch post-rewind); C: terminal-success; D: non-terminal (working,
just respawned). Nothing to spawn; nothing to skip.

`ready_to_drive` computation per Decision 9 clause:
- A: terminal → `false` (first conjunct fails)
- B: non-terminal; `waits_on: [A]`; A is terminal-success → `true`
- C: terminal → `false`
- D: non-terminal; `waits_on: [B, C]`; B is NOT terminal-success
  (it's running under a fresh epoch) → `false`

Response:
```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "Read the plan document at PLAN-diamond.md. ...",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 4, "completed": 2, "pending": 1, "success": 2,
      "failed": 0, "skipped": 0, "blocked": 1, "spawn_failed": 0,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false,
      "any_spawn_failed": false, "needs_attention": false,
      "children": [
        {"name":"coord.issue-A","state":"done","complete":true,"outcome":"success"},
        {"name":"coord.issue-B","state":"working","complete":false,"outcome":"pending"},
        {"name":"coord.issue-C","state":"done","complete":true,"outcome":"success"},
        {"name":"coord.issue-D","state":"working","complete":false,"outcome":"blocked","blocked_by":["coord.issue-B"]}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": [],
    "materialized_children": [
      {"name":"coord.issue-A","outcome":"success","state":"done","ready_to_drive":false},
      {"name":"coord.issue-B","outcome":"pending","state":"working","ready_to_drive":true},
      {"name":"coord.issue-C","outcome":"success","state":"done","ready_to_drive":false},
      {"name":"coord.issue-D","outcome":"pending","state":"working","ready_to_drive":false}
    ],
    "already": ["coord.issue-A","coord.issue-B","coord.issue-C","coord.issue-D"],
    "blocked": [], "skipped": [], "errored": [], "warnings": [],
    "feedback": {"entries": {}, "orphan_candidates": []}
  }
}
```

Gloss: "Retry accepted. Parent routed back to `plan_and_await`. The
ledger shows D present on disk but `ready_to_drive: false` because
B is not yet terminal-success. My worker pool filters on
`ready_to_drive: true`, so it picks B (ready) and leaves D alone.
F1 closed: D won't be dispatched prematurely."

**[GAP 1]** The gate's per-child `outcome` for D disagrees with the
child's own `state`: the gate reports `outcome: "blocked",
blocked_by: ["coord.issue-B"]` even though `state: "working"`. This
is the revised design's intent (gate re-classifies at runtime based
on dependency status, not on existence of the state file), but it
means "state: working" now co-exists with "outcome: blocked" in the
same child row, which is a shape that didn't exist pre-fix. The
walkthrough reference doesn't show this combination; agents should
be warned the `state` field reflects the child's own state machine
while `outcome` reflects the gate's scheduling verdict.

### Turn 2 — AGENT: worker-pool probe on D

AGENT's worker pool reads `materialized_children` and filters on
`ready_to_drive: true`. D is excluded (flag is `false`). To confirm,
AGENT runs a manual status check on D:

```bash
koto status coord.issue-D
```

KOTO:
```json
{
  "workflow": "coord.issue-D",
  "state": "working",
  "is_terminal": false,
  "synthetic": false
}
```

`synthetic: false` because D's current state is no longer a
`skipped_marker: true` state — it's `working` on the real
`impl-issue.md` template, post-respawn.

Gloss: "If my worker pool ignored `ready_to_drive` and dispatched D,
`koto next coord.issue-D` would hand out a `working` directive and
the worker would start implementing issue #104 against an unbuilt B.
The design relies on workers respecting `ready_to_drive`; there is
no koto-side guard."

### Turn 3 — AGENT: dual-submit R8-vacuous probe (SAME vars)

Between turns 1 and 4, AGENT (or a sibling agent on the same host —
unlikely, but tested) submits a `tasks` payload while D is mid-
respawn. Here the submission happens AFTER the retry tick's child
writes committed, so the R8-vacuous window is conceptually closed.
The design's `EntryOutcome::Respawning` variant only fires when
submission and respawn overlap within a single tick under the
advisory flock. To exercise the window, assume AGENT submits during
the retry tick itself (simulated here as a submission racing the
clearing-event write, defeated by the flock; the flock loser is
what the test actually observes).

```bash
koto next coord --with-data @tasks.json
```

where `tasks.json` is the ORIGINAL submission:
```json
{"tasks": [
  {"name":"issue-A","vars":{"ISSUE_NUMBER":"101"}},
  {"name":"issue-B","vars":{"ISSUE_NUMBER":"102"},"waits_on":["issue-A"]},
  {"name":"issue-C","vars":{"ISSUE_NUMBER":"103"},"waits_on":["issue-A"]},
  {"name":"issue-D","vars":{"ISSUE_NUMBER":"104"},"waits_on":["issue-B","issue-C"]}
]}
```

Under the advisory flock (Decision 12), the second caller hits the
lock and returns `integration_unavailable / concurrent_tick`
BEFORE any R8 work runs. So the actual `EntryOutcome::Respawning`
response is only observable in a hypothetical world where the
flock isn't held. The design says "this window is confined to a
single tick" under Decision 12; per-task feedback `Respawning`
becomes **unreachable in the LocalBackend single-host case**.

To reach it, assume CloudBackend: two machines, each holding its
own fcntl lock locally, race-submitting in the same wall-clock
window. Machine 1 runs the retry tick and, between steps (d) and
(e) of the canonical sequence, deletes D's old state file but has
not yet written the new one. Machine 2 submits a fresh `tasks`
payload including D. Machine 2's scheduler runs; classification
sees D-absent (deleted), evaluates R8 for an entry whose name
matches — but there is no `spawn_entry` to compare against. The
design's R8-vacuous clause skips R8 for D and emits:

```json
{
  "scheduler": {
    "feedback": {
      "entries": {
        "issue-A": {"outcome":"already_terminal"},
        "issue-B": {"outcome":"already_running"},
        "issue-C": {"outcome":"already_terminal"},
        "issue-D": {"outcome":"respawning"}
      },
      "orphan_candidates": []
    }
  }
}
```

Gloss: "Under CloudBackend, D's feedback says `respawning` — retry
in a moment. Under LocalBackend, the flock makes this unreachable
and I see `concurrent_tick` instead. Design mentions both paths but
doesn't flag that `Respawning` is effectively CloudBackend-only."

**[GAP 2]** The design's `EntryOutcome::Respawning` claim ("submission
not rejected, feedback entry carries `Respawning`") is reachable
only when the advisory flock is NOT the serialization point — i.e.,
across machines under CloudBackend. For LocalBackend, the flock
preempts the race entirely. Single-host single-process agents will
never observe `Respawning`; the variant is purely a CloudBackend
feature. Decision 10 doesn't state this scope limitation.

### Turn 4 — AGENT: dual-submit with DIFFERENT vars (R8 fire test)

AGENT is now BACK in a non-racing single-tick world. B has been
rewound, D has been respawned (with the ORIGINAL vars
`{"ISSUE_NUMBER": "104"}` per the preserved spawn_entry), the retry
tick has fully committed. AGENT submits a tasks payload mutating
D's vars:

```bash
koto next coord --with-data '{"tasks": [
  {"name":"issue-A","vars":{"ISSUE_NUMBER":"101"}},
  {"name":"issue-B","vars":{"ISSUE_NUMBER":"102"},"waits_on":["issue-A"]},
  {"name":"issue-C","vars":{"ISSUE_NUMBER":"103"},"waits_on":["issue-A"]},
  {"name":"issue-D","vars":{"ISSUE_NUMBER":"999"},"waits_on":["issue-B","issue-C"]}
]}'
```

KOTO: R8 runs pre-append. D's disk spawn_entry snapshot has
`vars.ISSUE_NUMBER: "104"`; submitted entry has `"999"`. Mismatch
→ whole-submission reject:

```json
{
  "action": "error",
  "state": "plan_and_await",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Spawned task 'issue-D' has mutated fields",
    "details": [{"field":"tasks[3].vars.ISSUE_NUMBER","reason":"spawned_task_mutated"}],
    "batch": {
      "kind": "spawned_task_mutated",
      "task": "issue-D",
      "changed_fields": [
        {"field":"vars.ISSUE_NUMBER","spawned_value":"104","submitted_value":"999"}
      ]
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

Gloss: "R8 fires against the respawn's spawn_entry, not against the
pre-retry snapshot. Respawn correctly bumped the snapshot to the
current submission's values, so the only way to mutate D is before
the respawn commits — which is R8-vacuous window territory, which
LocalBackend doesn't expose."

### Turn 5 — AGENT: resubmit with SAME vars (R8 pass)

```bash
koto next coord --with-data @tasks.json
```

(original tasks.json from Turn 3 — SAME vars as the preserved
spawn_entry for D).

KOTO: R8 passes. Submission accepts; `EvidenceSubmitted` appends
with the `tasks` payload. Scheduler runs at `plan_and_await`. A, B,
C, D all already materialized. Feedback per-entry:

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "Read the plan document at PLAN-diamond.md. ...",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 4, "completed": 2, "pending": 1, "success": 2,
      "failed": 0, "skipped": 0, "blocked": 1, "spawn_failed": 0,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false,
      "any_spawn_failed": false, "needs_attention": false,
      "children": [
        {"name":"coord.issue-A","state":"done","complete":true,"outcome":"success"},
        {"name":"coord.issue-B","state":"working","complete":false,"outcome":"pending"},
        {"name":"coord.issue-C","state":"done","complete":true,"outcome":"success"},
        {"name":"coord.issue-D","state":"working","complete":false,"outcome":"blocked","blocked_by":["coord.issue-B"]}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": [],
    "materialized_children": [
      {"name":"coord.issue-A","outcome":"success","state":"done","ready_to_drive":false},
      {"name":"coord.issue-B","outcome":"pending","state":"working","ready_to_drive":true},
      {"name":"coord.issue-C","outcome":"success","state":"done","ready_to_drive":false},
      {"name":"coord.issue-D","outcome":"pending","state":"working","ready_to_drive":false}
    ],
    "already": ["coord.issue-A","coord.issue-B","coord.issue-C","coord.issue-D"],
    "blocked": [], "skipped": [], "errored": [], "warnings": [],
    "feedback": {
      "entries": {
        "issue-A": {"outcome":"already_terminal"},
        "issue-B": {"outcome":"already_running"},
        "issue-C": {"outcome":"already_terminal"},
        "issue-D": {"outcome":"already_running"}
      },
      "orphan_candidates": []
    }
  }
}
```

Gloss: "Per-entry feedback differentiates `already_terminal`
(success/skip — nothing to drive) from `already_running` (worker
may be mid-task). D is `already_running`: the respawned child is
present, its spawn_entry matches my submission, no work to spawn."

**[GAP 3]** `AlreadyRunning` is reported for D even though D's
`ready_to_drive: false`. A naive agent might read the feedback
map ("D is already running, a worker has it") and decide "all
good." But `ready_to_drive: false` says otherwise. Under Decision
10 the split is clean: `AlreadyRunning` describes entry lifecycle
(child exists, non-terminal), while `ready_to_drive` describes
dispatch readiness. The design doesn't explicitly call out that
these two signals are orthogonal and that `AlreadyRunning` by
itself is not a green light to expect progress.

### Turn 6 — AGENT: drive B to success, re-tick parent

AGENT runs the B worker:
```bash
koto next coord.issue-B
koto next coord.issue-B --with-data '{"status":"complete"}'
```

Second returns `done`. Parent re-tick:
```bash
koto next coord
```

KOTO: classify — A success, B success (just finished), C success,
D non-terminal. `ready_to_drive` for D: `waits_on: [B, C]`; B is
now terminal-success, C is terminal-success → `true`.

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "Read the plan document at PLAN-diamond.md. ...",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 4, "completed": 3, "pending": 1, "success": 3,
      "failed": 0, "skipped": 0, "blocked": 0, "spawn_failed": 0,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false,
      "any_spawn_failed": false, "needs_attention": false,
      "children": [
        {"name":"coord.issue-A","state":"done","complete":true,"outcome":"success"},
        {"name":"coord.issue-B","state":"done","complete":true,"outcome":"success"},
        {"name":"coord.issue-C","state":"done","complete":true,"outcome":"success"},
        {"name":"coord.issue-D","state":"working","complete":false,"outcome":"pending"}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": [],
    "materialized_children": [
      {"name":"coord.issue-A","outcome":"success","state":"done","ready_to_drive":false},
      {"name":"coord.issue-B","outcome":"success","state":"done","ready_to_drive":false},
      {"name":"coord.issue-C","outcome":"success","state":"done","ready_to_drive":false},
      {"name":"coord.issue-D","outcome":"pending","state":"working","ready_to_drive":true}
    ],
    "already": ["coord.issue-A","coord.issue-B","coord.issue-C","coord.issue-D"],
    "blocked": [], "skipped": [], "errored": [], "warnings": [],
    "feedback": {"entries": {}, "orphan_candidates": []}
  }
}
```

Gloss: "D's `ready_to_drive` flipped to `true` on this tick because
B settled to `success`. My worker pool picks D up now and drives
it. Gate also switched D from `blocked` to `pending` — the two
fields move together." F1 fix verified.

### Turn 7 — Edge case probe #1: retry on a child mid-respawn

Hypothesis: immediately after Turn 1 commits but before any
subsequent tick, AGENT issues a SECOND `retry_failed` naming D.

```bash
koto next coord --with-data '{"retry_failed": {"children": ["coord.issue-D"]}}'
```

KOTO: R10 validates. `coord.issue-D` exists. Current outcome:
- Gate-level outcome is `blocked` (`waits_on` contains non-terminal B).
- On-disk lifecycle: non-terminal `working`.
- Neither `failure` nor `skipped`.

Reject with `InvalidRetryReason::ChildNotEligible`:
```json
{
  "action": "error",
  "state": "plan_and_await",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "retry_failed rejected: one or more children are not eligible for retry",
    "details": [{"field":"retry_failed.children","reason":"child_not_eligible"}],
    "batch": {
      "kind": "invalid_retry_request",
      "reason": "child_not_eligible",
      "children": [
        {"name":"coord.issue-D","current_outcome":"blocked"}
      ]
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

**[GAP 4]** The rejection names `current_outcome: "blocked"` for D,
but D's state-file-level status (`working`) is unmentioned. Agents
trying to reason "why can't I retry D?" see `blocked` and might
expect an `unblock` action rather than understanding that the retry
of B in progress will eventually drive D. No transient-respawn
signal surfaces in `ChildNotEligible`; a dedicated
`InvalidRetryReason::ChildInFlightAfterRetry` would help.

### Turn 8 — Edge case probe #2: worker skips ready_to_drive filter

Suppose a malfunctioning worker pool (or a human operator) ignores
`ready_to_drive` and dispatches D manually during Turn 1's state
(before B re-runs):

```bash
koto next coord.issue-D
```

KOTO: D's state file exists at `working` on the real `impl-issue.md`
template.  Per-child tick returns the normal working directive:
```json
{
  "action": "evidence_required",
  "state": "working",
  "directive": "Implement issue #104. ...",
  "expects": {"event_type":"evidence_submitted","fields":{"status":{"type":"enum","values":["complete","blocked"],"required":true}}},
  "blocking_conditions":[],
  "scheduler":null
}
```

koto has no knowledge that "B is mid-retry and D should wait." The
child's own template does not encode waits_on semantics — that's
purely a parent-level scheduler concern. The worker runs D against
an un-rebuilt B. If B later fails again and the scheduler runs, it
will delete-and-respawn D AGAIN as a skip marker, discarding the
worker's wasted work.

**Design stance**: this is an efficiency concern, not a correctness
bug. The parent-level gate will reclassify D correctly on the next
tick regardless of what the worker did. The design explicitly puts
`ready_to_drive` filtering on the worker side (Reading guide in
walkthrough.md lines 1225-1231).

**[GAP 5]** There is no koto-side enforcement preventing a child
`koto next` call while the child is `ready_to_drive: false`. The
design documents this as "workers MUST skip" but makes it
convention-only. A defensive option: per-child `koto next` could
check the parent's gate output and reject with a
`DispatchedNotReady` warning. Design chooses convention for
simplicity; flag for reviewers.

### Turn 9 — Edge case probe #3: respawn fails (init_state_file errors)

Simulate: during Turn 1's retry tick, the delete-and-respawn of D
succeeds at `unlink` but fails at `init_state_file` (e.g., disk
full, EACCES, or backend collision). Decision 2 guarantees
atomicity via `tempfile::persist`, so half-written state files are
impossible. What we get instead is a per-task error in the retry
tick's scheduler output:

Hypothetical response for the retry tick if respawn failed:
```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "output": {
      "total": 4, "completed": 2, "pending": 1, "success": 2,
      "failed": 0, "skipped": 0, "blocked": 0, "spawn_failed": 1,
      "all_complete": false, "all_success": false,
      "any_failed": false, "any_skipped": false,
      "any_spawn_failed": true, "needs_attention": true,
      "children": [
        {"name":"coord.issue-A","state":"done","complete":true,"outcome":"success"},
        {"name":"coord.issue-B","state":"working","complete":false,"outcome":"pending"},
        {"name":"coord.issue-C","state":"done","complete":true,"outcome":"success"},
        {"name":"coord.issue-D","state":null,"complete":false,"outcome":"spawn_failed","spawn_error":{"task":"issue-D","kind":"io_error","message":"ENOSPC: no space left on device"},"reason_source":"not_spawned"}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": [],
    "materialized_children": [
      {"name":"coord.issue-A","outcome":"success","state":"done","ready_to_drive":false},
      {"name":"coord.issue-B","outcome":"pending","state":"working","ready_to_drive":true},
      {"name":"coord.issue-C","outcome":"success","state":"done","ready_to_drive":false}
    ],
    "already": ["coord.issue-A","coord.issue-B","coord.issue-C"],
    "blocked": [], "skipped": [],
    "errored": [
      {"task":"issue-D","kind":"io_error","message":"ENOSPC: no space left on device"}
    ],
    "warnings": [],
    "feedback": {"entries":{},"orphan_candidates":[]}
  }
}
```

Gloss: "D is not in `materialized_children` (no state file exists).
Gate row shows `outcome: spawn_failed`. `needs_attention: true`
re-routes to `analyze_failures` on the NEXT tick. AGENT can reissue
`retry_failed { children: [coord.issue-D] }` — that's retry-respawn
per Decision 9's eligibility table, which accepts `spawn_failed` as
a retry target."

**[GAP 6]** The response above mixes pre-retry state (B `working`)
with a tick that also failed to respawn D. There's an ordering
question: if Turn 1's respawn fails mid-step (d), does the parent
still advance through step (e) to `plan_and_await`, or does the
retry tick abort and leave the parent at `analyze_failures`?

- Step (d) is "for each child in the downward closure: rewind or
  respawn." If one of these writes errors, Decision 9 doesn't say
  whether the tick rolls back or continues.
- Decision 12's advisory flock just serializes ticks — it doesn't
  offer rollback.
- B was rewound BEFORE D's respawn failed (steps run in order over
  the closure). So at the end of a partially-failed retry tick, B
  is rewound but D has no state file.

This is a real concern: if the tick continues through step (e),
the parent advances to `plan_and_await`, but the retry is
partially committed. If the tick aborts at step (d) with the
respawn error, B is rewound but the parent stays at
`analyze_failures` (inconsistent: B's log says "Rewound to
working" but the parent isn't in the batched state). The design
is silent on which.

### Turn 10 — `BatchFinalized` invalidation on retry terminal

After Turn 6, AGENT drives D:
```bash
koto next coord.issue-D
koto next coord.issue-D --with-data '{"status":"complete"}'
```

Parent re-tick:
```bash
koto next coord
```

Classify: A, B, C, D all terminal-success. `all_success: true`.
Transition to `summarize`. `BatchFinalized` appends (a NEW event,
superseding the one from Turn 6-original in Round 2 pair).

Response:
```json
{
  "action": "done",
  "state": "summarize",
  "directive": "Write a summary covering which issues succeeded, which failed, and why. The batch_final_view field in this response carries the full snapshot so you don't need a second command.",
  "is_terminal": true,
  "batch_final_view": {
    "phase": "final",
    "summary": {"total":4,"success":4,"failed":0,"skipped":0,"pending":0,"blocked":0,"spawn_failed":0},
    "tasks": [
      {"name":"issue-A","child":"coord.issue-A","outcome":"success"},
      {"name":"issue-B","child":"coord.issue-B","outcome":"success"},
      {"name":"issue-C","child":"coord.issue-C","outcome":"success"},
      {"name":"issue-D","child":"coord.issue-D","outcome":"success"}
    ],
    "ready":[], "blocked":[], "skipped":[], "failed":[]
  }
}
```

And `koto status coord`:
```json
{
  "workflow": "coord",
  "state": "summarize",
  "is_terminal": true,
  "batch": {
    "phase": "final",
    "summary": {"total":4,"success":4,"failed":0,"skipped":0,"pending":0,"blocked":0,"spawn_failed":0},
    "tasks": [
      {"name":"issue-A","child":"coord.issue-A","outcome":"success"},
      {"name":"issue-B","child":"coord.issue-B","outcome":"success"},
      {"name":"issue-C","child":"coord.issue-C","outcome":"success"},
      {"name":"issue-D","child":"coord.issue-D","outcome":"success"}
    ]
  }
}
```

Gloss: "`batch_final_view` reflects the MOST RECENT `BatchFinalized`
event, not the first. The old pre-retry finalization (which said B
failed, D skipped) is superseded. CD13's supersede-on-retry works."

---

## Section 2: Findings

### F1 — `ready_to_drive: false` on D during retry closes the F1 blocker

- **Observation**: Turn 1's response puts D in
  `materialized_children` with `ready_to_drive: false` because its
  `waits_on: [B]` contains a non-terminal child. Workers filtering
  on the flag correctly exclude D. Turn 6 shows the flag flipping
  to `true` once B settles to terminal-success.
- **Verifies round-2 fix?**: **Yes, fully.**
- **Severity**: nice-to-have (positive verification).
- **Proposed resolution**: no change.

### F2 — Gate `outcome: "blocked"` with child `state: "working"` is new ambiguity

- **Observation**: During the retry window, D's gate row is
  `outcome: "blocked", blocked_by: ["coord.issue-B"]` while its
  own `state: "working"`. Pre-fix, a `state: working` child was
  always `outcome: pending`. The walkthrough reference shows no
  example of this combination; agents may mis-read it as
  inconsistent.
- **Verifies round-2 fix?**: partial — the fix is semantically
  correct but introduces a new field combination.
- **Severity**: should-fix (documentation gap).
- **Proposed resolution**: add a sentence to walkthrough.md and
  Decision 5.2's Reading guide: "When `waits_on` dependencies flip
  back to non-terminal (e.g., during retry), a non-terminal child
  appears with `outcome: blocked` AND `state: <its own in-progress
  state>`. The gate row's `outcome` is the scheduling verdict;
  `state` is the child's own state machine."

### F3 — `EntryOutcome::Respawning` is CloudBackend-only in practice

- **Observation**: Decision 10's `Respawning` variant requires
  submission and respawn to overlap within a single tick. Under
  LocalBackend's advisory flock (Decision 12), concurrent ticks
  are serialized; the second submission hits the lock and returns
  `concurrent_tick` before reaching R8 evaluation. `Respawning` is
  therefore only reachable via CloudBackend where each machine
  holds its own local flock.
- **Verifies round-2 fix?**: partial — the variant exists and the
  contract is specified, but the scope is narrower than the design
  text implies.
- **Severity**: should-fix.
- **Proposed resolution**: add a single-sentence scope note to
  Decision 10's R8-vacuous window paragraph:
  "`EntryOutcome::Respawning` is reachable only under CloudBackend;
  under LocalBackend the advisory flock serializes submissions
  before R8 runs, so the second caller observes `concurrent_tick`
  instead." Prevents implementers from writing LocalBackend tests
  that can never trigger the path.

### F4 — `AlreadyRunning` feedback is orthogonal to `ready_to_drive`

- **Observation**: Turn 5 shows D with
  `feedback.entries["issue-D"].outcome: "already_running"` while
  `materialized_children[...].ready_to_drive: false`. An agent
  reading only feedback would conclude "D is already being
  driven"; reading only `ready_to_drive` would conclude "don't
  dispatch D." Both are correct, but the feedback map doesn't
  signal dispatchability.
- **Verifies round-2 fix?**: no — new issue surfaced by the fix
  itself.
- **Severity**: nice-to-have.
- **Proposed resolution**: either (a) add a sentence to Decision 10
  noting that `AlreadyRunning` describes the child's lifecycle
  state on disk, not its dispatchability, which lives exclusively
  on `MaterializedChild.ready_to_drive`; or (b) extend
  `EntryOutcome` with a variant distinguishing
  `AlreadyRunningDispatchable` from `AlreadyRunningBlocked`. Option
  (a) is cheaper and matches the "single source of truth" design
  spirit.

### F5 — Partial-respawn failure mid-retry-tick has undefined commit semantics

- **Observation**: Decision 9's canonical retry sequence runs step
  (d) as a loop over the closure ({B, D} here). If B's rewind
  succeeds but D's respawn fails (step d iteration 2), Decision 9
  does not specify whether the tick aborts (B is rewound but
  parent stays at `analyze_failures`, the clearing event is
  already written, so `handle_retry_failed` rejects on next run
  with `no_retry_in_progress`) or continues (B rewound, D
  `spawn_failed`, parent advances to `plan_and_await`).
- **Verifies round-2 fix?**: no — gap in the revised design's
  error-path specification.
- **Severity**: should-fix. Implementers need a commit model.
- **Proposed resolution**: specify that step (d) accumulates
  per-child errors (rewinds always succeed — append-only; respawns
  can error at `init_state_file`) and step (e) always runs; errored
  respawns surface as `spawn_failed` per Decision 11's per-task
  accumulation. Crash mid-step-(d) is already handled by the
  clearing-event idiom (resume sees the clearing event, concludes
  `no_retry_in_progress`, user resubmits). The spec needs to pin
  "step (d) does NOT abort the tick; accumulate and report" as the
  contract, parallel to the "per-task accumulation, never halt"
  commitment in Decision 11.

### F6 — `ChildNotEligible` with `current_outcome: "blocked"` lacks transient context

- **Observation**: Turn 7's rejection names D with
  `current_outcome: "blocked"` but doesn't indicate that the block
  is transient (due to a retry in progress on B). Agents seeing
  "blocked" might look for an unblock action rather than
  understanding B's rewind is the real driver.
- **Verifies round-2 fix?**: no — new gap in the retry-error
  envelope.
- **Severity**: nice-to-have.
- **Proposed resolution**: enrich `ChildNotEligible` details with
  the upstream driver when the block is due to a sibling's
  in-progress retry: `{"name":"coord.issue-D","current_outcome":"blocked","reason":"upstream_retry","awaiting":["coord.issue-B"]}`.
  Agent now knows "wait for B; don't try to retry D separately."

### F7 — No koto-side guard against dispatching a `ready_to_drive: false` child

- **Observation**: Turn 8 shows that `koto next coord.issue-D`
  returns a normal working directive even when the parent's gate
  says D shouldn't be dispatched. The design puts the filtering
  burden on workers via convention (walkthrough.md lines
  1225-1231).
- **Verifies round-2 fix?**: partial — the fix relies on workers
  cooperating; koto doesn't enforce it.
- **Severity**: nice-to-have. The design explicitly accepts this
  as an efficiency concern not a correctness concern (the
  scheduler reclassifies on the next parent tick), and there's a
  reasonable argument that child `koto next` shouldn't know about
  parent state.
- **Proposed resolution**: two options. (a) Leave as-is; document
  prominently in the koto-user skill that worker pools must filter
  on `ready_to_drive`. (b) Optionally emit a warning in the child's
  response when the scheduler last computed `ready_to_drive: false`
  for the child (e.g., a `parent_scheduling_hint: "not_ready"`
  field). Option (a) matches the current design spirit; (b) adds
  defence-in-depth but introduces parent-child coupling koto has
  been careful to avoid.

### F8 — Verified: `BatchFinalized` supersede-on-retry

- **Observation**: Turn 10 shows a fresh `BatchFinalized` written
  on the retry-success terminal tick. Both the terminal `done`
  response's `batch_final_view` and the subsequent `koto status`
  call return the POST-RETRY snapshot (all four children
  success), not the pre-retry one (B failed, D skipped).
- **Verifies round-2 fix?**: **Yes.** Decision 13's supersede
  rule fires correctly.
- **Severity**: nice-to-have (positive verification).
- **Proposed resolution**: no change.

### F9 — Verified: `spawn_entry` preserved across retry-respawn

- **Observation**: Turn 4 (R8 fire on vars mutation) and Turn 5
  (R8 pass on identical resubmission) jointly confirm that the
  respawn bumped D's on-disk `spawn_entry` to the entry values
  that were in force at respawn time. R8 evaluates against the
  NEW spawn_entry, not a pre-retry ghost.
- **Verifies round-2 fix?**: **Yes.** Decision 10's "spawn_entry
  on respawn is the current submission's entry" clause is
  enforced by construction.
- **Severity**: nice-to-have (positive verification).
- **Proposed resolution**: no change.

---

## Summary note

Round 2's F1 blocker is closed by `ready_to_drive` + respawn-time
`spawn_entry` preservation (F1, F8, F9 all verify). Three new
should-fix items surfaced: a documentation gap where `outcome:
blocked` co-exists with `state: working` (F2), the scope limitation
of `EntryOutcome::Respawning` to CloudBackend (F3), and undefined
commit semantics for partial respawn failure mid-retry-tick (F5).
Three nice-to-haves cluster around signal orthogonality (F4),
error-envelope richness (F6), and optional child-side enforcement
(F7). No blockers remain against the revised design; F5 is the
closest to load-bearing and deserves a one-paragraph spec patch
before implementation.
