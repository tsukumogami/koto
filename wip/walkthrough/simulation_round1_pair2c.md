# Simulation Round 1, Pair 2c — Dynamic Additions Under Concurrency

Scenario: 5-task batch with parallelism-of-3, DAG = {A, B, C independent; D
waits_on [A, B]; E waits_on [C]}. A COORD agent submits tasks; three
WORKERs (WORKER-A/B/C) drive children concurrently. Probes exercise the
"serialize scheduler ticks on the parent" invariant, `init_state_file`
TOCTOU, cloud-sync split-brain, mid-spawn crash, and a second coordinator.

Parent workflow name: `coord`. Child names: `coord.A`, `coord.B`, ...

## Section 1: Transcript

### Setup (not raced)

```
T0  COORD  $ koto init coord --template coord.md --var plan_path=PLAN.md
T0  KOTO   -> {"action":"initialized","workflow":"coord","state":"plan_and_await","template":"coord.md"}
T1  COORD  $ koto next coord
T1  KOTO   -> {"action":"evidence_required","state":"plan_and_await",
              "directive":"<plan_and_await directive>",
              "expects":{"event_type":"evidence_submitted","fields":{"tasks":{"type":"tasks","required":true, "item_schema":{...}}}},
              "blocking_conditions":[{"name":"done","type":"children-complete","category":"temporal",
                "output":{"total":0,"completed":0,"pending":0,"success":0,"failed":0,"skipped":0,"blocked":0,"all_complete":false,"children":[]}}],
              "scheduler":null}
T2  COORD  $ koto next coord --with-data @tasks.json   # A,B,C,D(A+B),E(C)
T2  KOTO   -> {"action":"gate_blocked","state":"plan_and_await",
              "blocking_conditions":[{"name":"done","type":"children-complete","category":"temporal",
                "output":{"total":5,"completed":0,"pending":5,"success":0,"failed":0,"skipped":0,"blocked":2,"all_complete":false,
                  "children":[
                    {"name":"coord.A","state":"working","complete":false,"outcome":"pending"},
                    {"name":"coord.B","state":"working","complete":false,"outcome":"pending"},
                    {"name":"coord.C","state":"working","complete":false,"outcome":"pending"},
                    {"name":"coord.D","state":null,"complete":false,"outcome":"blocked","blocked_by":["coord.A","coord.B"]},
                    {"name":"coord.E","state":null,"complete":false,"outcome":"blocked","blocked_by":["coord.C"]}]}}],
              "scheduler":{"spawned":["coord.A","coord.B","coord.C"],"already":[],"blocked":["coord.D","coord.E"],"skipped":[]}}
```

Gloss: parallelism-of-3 root: A, B, C spawned. COORD now dispatches
WORKER-A, WORKER-B, WORKER-C and can drive them in parallel (distinct
state files, distinct logs — invariant-safe).

---

### Probe 1 — Concurrent `koto next coord` from COORD and WORKER-B

Scenario: A finishes first. COORD wants to re-tick parent. WORKER-B also
finishes at virtually the same instant and — breaking the caller
invariant — calls `koto next coord` directly.

Timeline:

```
T10       WORKER-A  $ koto next coord.A --with-data '{"status":"complete"}'
T10       KOTO      -> {"action":"done","state":"done","is_terminal":true}
T11       WORKER-B  $ koto next coord.B --with-data '{"status":"complete"}'
T11       KOTO      -> {"action":"done","state":"done","is_terminal":true}

T12.000   COORD     $ koto next coord                         # scheduler tick #P1
T12.001   WORKER-B  $ koto next coord                         # scheduler tick #P2 (INVARIANT VIOLATED)

T12.002   KOTO(P1)  reads parent log @ seq=S; expected_seq=S
T12.003   KOTO(P2)  reads parent log @ seq=S; expected_seq=S       # same snapshot
T12.004   KOTO(P1)  classifies A=Terminal, B=Terminal, C=Running, D=Ready (A,B terminal), E=Blocked
T12.005   KOTO(P2)  classifies identically
T12.006   KOTO(P1)  init_state_file("coord.D", header, [WI, Transitioned->working])
          KOTO(P1)    -> tempfile::persist() -> rename(2) OK (file didn't exist)
T12.007   KOTO(P2)  backend.exists("coord.D") -> false  (TOCTOU: stale read in P2's path
                    — P2 read exists() before P1's rename landed, OR P2's rename
                    overwrites if it loses the race)
T12.008   KOTO(P2)  init_state_file("coord.D", header, [WI, Transitioned->working])
                    -> rename(2) SILENTLY OVERWRITES the file P1 just wrote
                    [design L1959-1964: "Unix rename(2) has no fail-if-exists semantics,
                    so the second rename silently overwrites the first"]
```

Now two racing writes on the parent log:

```
T12.009   KOTO(P1)  append_event(parent, ScheduledTask{coord.D}) at seq=S+1 OK
T12.010   KOTO(P2)  append_event(parent, ScheduledTask{coord.D}) at expected_seq=S+1
                    but actual seq is now S+2 -> expected_seq mismatch
```

Two outcomes, depending on how `append_event` resolves:

**Outcome 1a (expected_seq check rejects P2).** KOTO(P2) returns an
error to WORKER-B. WORKER-B sees a CLI error; COORD's response lands
normally. coord.D's state file was overwritten once but both writes
initialized it to the same initial epoch — no data loss on coord.D
because it had no intermediate events. Parent log is consistent.

```
T12.011   KOTO(P1)  -> {"action":"gate_blocked","state":"plan_and_await",
                       "blocking_conditions":[{"name":"done","type":"children-complete","category":"temporal",
                         "output":{"total":5,"completed":2,"pending":3,"success":2,"failed":0,"skipped":0,"blocked":1,"all_complete":false,
                           "children":[
                             {"name":"coord.A","state":"done","complete":true,"outcome":"success"},
                             {"name":"coord.B","state":"done","complete":true,"outcome":"success"},
                             {"name":"coord.C","state":"working","complete":false,"outcome":"pending"},
                             {"name":"coord.D","state":"working","complete":false,"outcome":"pending"},
                             {"name":"coord.E","state":null,"complete":false,"outcome":"blocked","blocked_by":["coord.C"]}]}}],
                       "scheduler":{"spawned":["coord.D"],"already":["coord.A","coord.B","coord.C"],"blocked":["coord.E"],"skipped":[]}}
T12.011   KOTO(P2)  -> stderr "error: state file append conflict (expected_seq S+1, found S+2); another writer advanced this workflow. Retry."
                       exit 2
```

WORKER-B retries `koto next coord` and sees `already:["coord.D", ...]`. Safe.

**Outcome 1b (worse case — duplicate append silently accepted).** Design
L1953-1955: "or worse, silently accepts duplicates and fails the next
read." Both `ScheduledTask{coord.D}` events land at seq S+1 and S+2.
Next reader of parent log observes two materializations for coord.D,
which derive_batch_view would count as... [GAP: the design does not
specify whether `ScheduledTask{coord.D}` is idempotent on replay when
duplicated in the log; does `derive_batch_view` dedupe by task name, and
does `expected_seq` treat the second append as a hard fail or a warning
the next reader hits?]

**Critically**, if coord.D had received any events between T12.006 and
T12.008 (impossible here because no WORKER was driving D yet — it was
just spawned microseconds ago), those would be silently destroyed by
P2's overwriting `rename`. The design acknowledges this at L1962-1964.

```
[GAP-1: concurrent parent ticks under a racing spawn produce a narrow
window where init_state_file TOCTOU + append_event race combine. If
WORKER-D is somehow dispatched off P1's response before P2's rename
lands and submits evidence in that window, WORKER-D's evidence event is
lost by P2's overwrite. The design assumes orchestrators can't dispatch
off a response until the response returns, which is true for
single-process callers but NOT guaranteed for async pipeline callers
that fire the child workers eagerly from a "spawned" notification.]
```

---

### Probe 2 — WORKER-A submits new tasks F, G to the parent

After A completes, WORKER-A does NOT return control — instead it calls:

```
T13  WORKER-A  $ koto next coord --with-data @new-tasks.json
              # new-tasks.json: {"tasks":[{"name":"F","waits_on":["A"]},{"name":"G","waits_on":["F"]}]}
```

Is this legal? The design doesn't explicitly forbid it — the walkthrough
says "the coordinator just starts driving the parent's workflow name
again", so any caller driving the parent's name is structurally
equivalent. However:

```
T13.001   KOTO  advance_loop(coord): plan_and_await is accepts-typed on `tasks` (required).
T13.002   KOTO  submission replaces previous evidence? Or merges? [GAP-2]
```

The walkthrough shows the first submission appends a single
`EvidenceSubmitted{fields:{tasks:[A,B,C,D,E]}}`. A second submission at
the same state with a different `tasks` payload appends a second
`EvidenceSubmitted{fields:{tasks:[F,G]}}`.

```
T13.003   KOTO  run_batch_scheduler reads parent events, reconstructs the "task list".
                [GAP-3: is the task list "last evidence wins", "union of all
                evidence events", or "first evidence wins"? The design's
                "Resume" section (L1905-1906) says "EvidenceSubmitted events are
                append-only; the task list is reconstructed identically on every
                call" but does not specify the reconstruction rule.]
```

If rule is **last-wins**: F, G become the task list; A, B, C, D, E are
dropped from the DAG but still exist on disk as children. `children-complete`
gate then sees {F (blocked_by A, but A not in DAG anymore -> dangling
ref)}. The existing children become orphaned observationally.

If rule is **union**: DAG now has 7 tasks. F waits_on A (terminal) ->
Ready. G waits_on F. Scheduler spawns coord.F. Fine.

The design's intent appears to be union (additive append mode) based on
the "mid-flight append" integration test (L2133) — but the semantics
aren't documented in the `plan_and_await` state's accepts schema.

Concurrency on top of this: if WORKER-A calls `koto next coord --with-data`
at T13 while COORD is ALSO calling `koto next coord` at T13.0005 (to
react to A finishing), both hit the same race surface as Probe 1 —
`expected_seq` mismatch or duplicate append.

```
[GAP-2/3: dynamic additions semantics (replace vs. merge vs. append
vs. reject) are not specified. Combined with the serialization
invariant being a caller responsibility, ANY caller that adds tasks
must ALSO own exclusive rights to `koto next coord`. The design's
invariant language implies a single coordinator but not exclusively.]
```

Response (assuming union + no race):

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done", "type": "children-complete", "category": "temporal",
    "output": {
      "total": 7, "completed": 1, "pending": 6,
      "success": 1, "failed": 0, "skipped": 0, "blocked": 3,
      "all_complete": false,
      "children": [
        {"name": "coord.A", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.B", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.C", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.D", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.B"]},
        {"name": "coord.E", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.C"]},
        {"name": "coord.F", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.G", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.F"]}
      ]
    }
  }],
  "scheduler": {"spawned": ["coord.F"], "already": ["coord.A","coord.B","coord.C"], "blocked": ["coord.D","coord.E","coord.G"], "skipped": []}
}
```

---

### Probe 3 — Cloud-sync split-brain

AGENT runs the workflow on Machine 1 (M1) and Machine 2 (M2) with cloud
sync enabled. M1 submits a task-list extension (adds F, G). M2, unaware,
submits a `retry_failed` on a previously failed child.

```
T20  M1   $ koto next coord --with-data @add-fg.json
T20  KOTO(M1)  local append: EvidenceSubmitted{tasks:[F,G]}
               local init_state_file("coord.F")
               sync_push_state(coord)        -- PUT whole file, wins upload race
               sync_push_state(coord.F)

T20  M2   $ koto next coord --with-data '{"retry_failed":{"children":["coord.E"]}}'
T20  KOTO(M2)  local append: EvidenceSubmitted{retry_failed:{...}}
               advance_loop: transitions via template route, runs handle_retry_failed
               appends Rewound to coord.E
               appends clearing {"retry_failed":null} to coord
               sync_push_state(coord)        -- PUT, conflicts with M1's earlier PUT
```

Design L2337-2357: `sync_push_state` is a PUT, not a merge. Losing side
(let's say M2) sees a conflict surfaced by `check_sync` on next tick:

```
T21  M2   $ koto next coord
T21  KOTO(M2)  check_sync detects divergence
          -> stderr: "sync conflict on workflow `coord`: remote advanced since your last push.
                      Run `koto session resolve coord`."
               exit 4
```

M2 runs resolve, which per design surfaces the divergence but does NOT
merge. User picks winning side. Suppose user picks M1 (task list
extension wins, F/G stay). M2's retry_failed is discarded.

```
T22  M2   $ koto session resolve coord --accept remote   [GAP-4: exact flags]
T22  KOTO  pulls M1's state as canonical; M2's local log is set aside.
T23  M2   $ koto next coord
T23  KOTO  reads reconciled log. Last `EvidenceSubmitted` is tasks:[F,G].
           retry_failed was NOT in the winning log.
           coord.E's state file locally on M2 has a Rewound event from T20 — but
           coord.E has been cloud-synced since then by whom? Neither pushed coord.E
           after the conflict. [GAP-5: child state files are per-workflow-synced,
           so coord.E on M1 is still at done_blocked, coord.E on M2 is rewound to
           working. The parent log on both now matches (M1's wins), but the child
           state files diverge across machines.]
```

Response M2 sees at T23:

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done", "type": "children-complete", "category": "temporal",
    "output": {
      "total": 7, "completed": 4, "pending": 3,
      "success": 3, "failed": 1, "skipped": 0, "blocked": 2,
      "all_complete": false,
      "children": [
        {"name": "coord.A", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.B", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.C", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.D", "state": "done", "complete": true, "outcome": "success"},
        {"name": "coord.E", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.F", "state": "working", "complete": false, "outcome": "pending"},
        {"name": "coord.G", "state": null, "complete": false, "outcome": "blocked", "blocked_by": ["coord.F"]}
      ]
    }
  }],
  "scheduler": {"spawned": [], "already": ["coord.A","coord.B","coord.C","coord.D","coord.E","coord.F"], "blocked": ["coord.G"], "skipped": []}
}
```

The observer on M2 sees coord.E as "working" (local view of child state
file, unsynced in either direction after conflict). M1's view of the
same child would show "done_blocked". Two observers, two truths.

Orphaned children? Any child M2 spawned from tasks that lost to M1's
winning task list exist as state files but are invisible to M1's DAG.
In this probe, both sides agreed on F, G, so no orphans — but a symmetric
scenario where M1 adds F and M2 adds F' with the same structural slot
(unlikely given unique `<parent>.<task>` naming, but possible if both
chose different names) produces unreferenced child state files on the
losing machine.

```
[GAP-4: `koto session resolve` flag/UX is not specified in the batch
design doc; design L2343 just references `src/session/version.rs::check_sync`.]
[GAP-5: per-child state files are synced per-workflow. After parent
log conflict resolution, child state files on the losing machine are
NOT rolled back — they diverge silently from the canonical parent
view. Gate output on M2 reflects M2's local child disk state; gate
output on M1 reflects M1's. Observers cannot tell which is canonical
without a machine tag in the response.]
[GAP-6: the observer has no `machine` or `sync_status` field in the
response to disambiguate. A split-brain observer can't know they're
reading from the losing side until their NEXT `koto next coord` trips
check_sync.]
```

---

### Probe 4 — Two workers each race the next-wave scheduler tick

Plan: initial root is parallelism-3, {A, B, C}. D waits_on [A, B].

```
T30.000   WORKER-A  $ koto next coord.A --with-data '{"status":"complete"}'
T30.005   WORKER-B  $ koto next coord.B --with-data '{"status":"complete"}'
T30.010   WORKER-A  $ koto next coord          # breaks invariant
T30.011   WORKER-B  $ koto next coord          # breaks invariant, concurrent with A's tick
```

Both ticks observe:

- coord.A: terminal, done (outcome success)
- coord.B: terminal, done (outcome success)
- coord.C: Running
- coord.D: waits_on [A, B], both Terminal -> Ready
- coord.E: waits_on [C], C Running -> BlockedByDep

Both call `backend.exists("coord.D")` -> false, both call
`init_state_file("coord.D", ...)`.

```
T30.020   KOTO(A)  tempfile::persist().rename(2)  -> coord.D created
T30.021   KOTO(B)  tempfile::persist().rename(2)  -> coord.D OVERWRITTEN
                   silent; coord.D reset to fresh initial epoch
```

If by some prior-probe weirdness coord.D had received an event
between T30.020 and T30.021 (e.g., some third WORKER-D fired eagerly
off A's response), that event is now lost. In this scenario no
WORKER-D existed yet, so no data loss — BUT both tick paths continue
and both try `append_event(parent, ScheduledTask{coord.D})`:

```
T30.030   KOTO(A)  append_event at expected_seq=S -> seq S+1 OK
T30.031   KOTO(B)  append_event at expected_seq=S -> mismatch -> error
                   OR silent duplicate at S+2 (design L1953-1955)
```

Both tick responses are returned to the workers. WORKER-A gets a clean
gate_blocked response with `spawned:["coord.D"]`. WORKER-B gets either
an error (good, visible), or a response claiming `spawned:["coord.D"]`
(bad — WORKER-B now believes IT spawned D, and so does WORKER-A).

```
[GAP-7: scheduler output may claim to have spawned a child that was
actually spawned by a concurrent tick. `scheduler.spawned` is not a
ledger-of-truth — it's a local-tick observation. Two concurrent ticks
returning {"spawned":["coord.D"]} both "claim" D. Any caller using
`spawned` for idempotency (e.g., a shirabe-style "start worker for
each newly spawned child") will double-dispatch WORKER-D.]
```

Result: two WORKER-Ds start against the same coord.D state file,
breaking single-writer invariant at the child level.

---

### Probe 5 — Child writes to parent

From the walkthrough: "the coordinator just starts driving the parent's
workflow name again — any caller can do it." So technically WORKER-A,
running inside coord.A context, can call `koto next coord`.

```
T40  WORKER-A (on coord.A side)  $ koto next coord.A --with-data '{"status":"complete"}'
T40  KOTO                        -> {"action":"done","state":"done","is_terminal":true}
T41  WORKER-A (same shell)       $ koto next coord       # children-complete re-eval + scheduler tick
```

If COORD is not also calling `koto next coord` at the same instant,
this is safe — it's just another serial call. The design's invariant
is "only one call at a time", not "only the coordinator calls". The
walkthrough's language supports this reading.

**But**: if WORKER-A calls `koto next coord` as part of its "I'm
done" handoff, AND COORD is also watching for child completions (via
`koto status coord` polling or observing `koto next coord.A`'s
response shape), COORD may ALSO call `koto next coord` to re-tick —
producing a Probe-1-style race. The design relies on the caller's
coordination protocol; it provides no koto-layer mutex.

```
[GAP-8: the walkthrough language "any caller" + the design's
invariant "only one at a time" combine to require a consumer-level
convention (e.g., "only the coordinator ticks the parent" OR "workers
signal via IPC; coordinator debounces") but koto ships neither the
convention nor a lockfile. Consumers who read only the walkthrough
will write unsafe patterns.]
```

---

### Probe 6 — Scheduler killed mid-spawn

COORD submits a batch; scheduler begins spawning 3 ready tasks (A, B, C).
After 2 successful `init_state_file` calls, before the 3rd, the scheduler
process receives SIGKILL.

```
T50  COORD  $ koto next coord --with-data @tasks.json
T50.001  KOTO  advance_loop OK, appends EvidenceSubmitted{tasks:[A,B,C,D,E]}
T50.002  KOTO  run_batch_scheduler begins
T50.003  KOTO  init_state_file("coord.A", ...)  -> rename OK
T50.004  KOTO  init_state_file("coord.B", ...)  -> rename OK
T50.005  KOTO  init_state_file("coord.C", ...)  -> tempfile created as /path/coord.C.koto-XXXX.tmp
T50.006  <SIGKILL>                              -- rename(2) never happened; .tmp file leaked
```

Disk state:
- parent log: seq=S+1 has EvidenceSubmitted{tasks:[A,B,C,D,E]}. No
  ScheduledTask events for A, B, C were appended before the crash —
  OR they were appended, depending on implementation ordering
  [GAP-9: does the scheduler append ScheduledTask events to the parent
  log BEFORE or AFTER calling init_state_file for each child? The design
  at L1877-1886 describes per-task calls but not the order of the
  parent-log append relative to the child init.]
- coord.A: fully initialized (WI + Transitioned->working).
- coord.B: fully initialized.
- coord.C: does not exist at final path; a `.koto-*.tmp` file leaks in
  the session dir. `backend.list()` ignores it (design L706-707).

Recovery:

```
T51  COORD  $ koto next coord
T51  KOTO   run_batch_scheduler is re-entered on the next tick.
            repair_half_initialized_children pre-pass (design L2120-2125):
              scans children, detects none with "header but no events" because
              init_state_file is atomic — coord.C doesn't exist at all, not
              half-initialized.
            Classifies: coord.A Running, coord.B Running, coord.C NotYetSpawned
              (no state file at final path), D Blocked, E Blocked.
            coord.C has empty waits_on -> Ready -> init_state_file -> OK.
            Gate re-evaluates.
T51  KOTO   -> {"action":"gate_blocked", ... scheduler:{"spawned":["coord.C"],"already":["coord.A","coord.B"],...}}
```

The leaked `.tmp` file remains on disk until `backend.cleanup` — design
L712-713 acknowledges this. Functionally harmless.

If the scheduler had ALSO crashed AFTER appending ScheduledTask events
but BEFORE the child init, repair_half_initialized_children would
detect nothing (child file doesn't exist) but the parent log would
claim it's been scheduled. [GAP-9 again: is the parent-log
ScheduledTask event source-of-truth, or is disk existence? The design
emphasizes disk-derivation at L1895-1896 ("pure function of parent
event log + children on disk"); if a task is ScheduledTask'd in the
log but not on disk, what is its state?]

---

### Probe 7 — Two coordinators against the same parent

User accidentally opens two shells and runs COORD-1 and COORD-2 against
the same `coord` name.

```
T60  COORD-2  $ koto init coord --template coord.md --var plan_path=PLAN-different.md
T60  KOTO     -> error: "workflow `coord` already exists" (handle_init is NOT idempotent;
                 design L725-728 rejects this)
                 exit 2
```

Good — init refuses. But COORD-2 can still `koto next coord
--with-data`:

```
T61  COORD-2  $ koto next coord --with-data @different-tasks.json
T61  KOTO     advance_loop: plan_and_await accepts `tasks`, validates, appends
              EvidenceSubmitted{tasks:[X,Y,Z]}. Scheduler tick spawns X,Y,Z.
```

Now the parent log has TWO EvidenceSubmitted{tasks} events if COORD-1
already submitted. Per GAP-3, reconstruction rule is unspecified. If
union: 8 tasks total. If last-wins: COORD-1's A,B,C,D,E are orphaned
children on disk but not in the scheduler DAG. If first-wins: COORD-2's
submission silently no-ops.

```
[GAP-10: the parent state file has no lock. A second process can submit
arbitrary evidence to the same parent at any time. Combined with GAP-3
(reconstruction rule), this is a multi-user correctness hazard on shared
filesystems. Design L1987-1989 explicitly inherits the v0.7.0 assumption
"one process writes to one workflow at a time" without enforcement.]
```

Response COORD-2 sees (assuming union + no race with COORD-1):

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "blocking_conditions": [{
    "name": "done", "type": "children-complete", "category": "temporal",
    "output": {
      "total": 8, "completed": 0, "pending": 8,
      "success": 0, "failed": 0, "skipped": 0, "blocked": 2,
      "all_complete": false,
      "children": [
        {"name":"coord.A","state":"working","complete":false,"outcome":"pending"},
        {"name":"coord.B","state":"working","complete":false,"outcome":"pending"},
        {"name":"coord.C","state":"working","complete":false,"outcome":"pending"},
        {"name":"coord.D","state":null,"complete":false,"outcome":"blocked","blocked_by":["coord.A","coord.B"]},
        {"name":"coord.E","state":null,"complete":false,"outcome":"blocked","blocked_by":["coord.C"]},
        {"name":"coord.X","state":"working","complete":false,"outcome":"pending"},
        {"name":"coord.Y","state":"working","complete":false,"outcome":"pending"},
        {"name":"coord.Z","state":"working","complete":false,"outcome":"pending"}
      ]
    }
  }],
  "scheduler": {"spawned":["coord.X","coord.Y","coord.Z"], "already":["coord.A","coord.B","coord.C"], "blocked":["coord.D","coord.E"], "skipped":[]}
}
```

COORD-1 is now unknowingly sharing the DAG with COORD-2. Both will dispatch
workers; both observers see "all 8" — but neither initially requested 8.

---

## Section 2: Findings

### Finding 1 — Reconstruction semantics for repeated `EvidenceSubmitted{tasks}` are unspecified
- **Observation**: Probes 2 and 7 both submit a second `tasks` evidence event on the parent. The design's "Resume" section says the task list reconstructs "identically" every call, but never defines whether repeated submissions replace, union, or are rejected. Behaviour visibly diverges (orphaned children vs. extended DAG vs. no-op).
- **Location**: DESIGN L1894-1906 (Resume), L1935-2010 (Concurrency model), parent template `accepts.tasks` semantics, and `materialize_children` spec L779-880.
- **Severity**: High. This affects dynamic additions (explicit design goal per "mid-flight append" test L2133) and multi-writer safety.
- **Proposed resolution**: Specify one rule. Recommend **union with de-dup by `name`, new names append, duplicate names rejected with a clear error**. Document in the `materialize_children` section and add a compile-time doc pointer from `accepts.tasks`.

### Finding 2 — `scheduler.spawned` is not a ledger and cannot be used for worker dispatch idempotency
- **Observation**: Under concurrent parent ticks (Probes 1, 4), both ticks can return `scheduler.spawned: ["coord.D"]` — either because the loser errored after claiming, or because the `init_state_file` TOCTOU overwrote silently. Callers using `spawned` to "start a worker for each new child" will double-dispatch.
- **Location**: DESIGN L1885-1890 (scheduler outcome), L1959-1964 (TOCTOU).
- **Severity**: High. Breaks single-writer invariant at the child level even when the caller invariant at the parent level is only softly violated.
- **Proposed resolution**: Either (a) implement `renameat2(RENAME_NOREPLACE)` + fallback lockfile at `init_state_file` (design rejects this at L1982-1992 but with a narrow rationale that doesn't account for child-level safety), or (b) document that `scheduler.spawned` is advisory-only and require consumers to diff against `already` from a previous tick before dispatching. Option (a) is the robust fix; option (b) pushes complexity onto every consumer.

### Finding 3 — No machine/sync disambiguation in responses under split-brain
- **Observation**: Probe 3 showed that after `koto session resolve`, the losing machine observes a child state that differs from the winning machine's — and nothing in the `koto next` response signals which machine the caller is on or whether the response is pre- or post-resolve. Observers can consume stale truth indefinitely.
- **Location**: DESIGN L2337-2357 (Cloud sync concurrent submission).
- **Severity**: Medium. Limited to multi-machine cloud sync users, but silent divergence is the nastiest bug class.
- **Proposed resolution**: Add `sync_status: "in_sync" | "behind" | "diverged"` and `machine_id` to the top-level `koto next` response. `check_sync` already computes this; surfacing it costs little. Additionally, after a `resolve`, explicitly check per-child state-file divergence and surface orphan/ghost children to the user.

### Finding 4 — Per-child state file divergence is not reconciled during `koto session resolve`
- **Observation**: Probe 3 showed that resolving the parent log does NOT roll back or reconcile the losing side's child state files. coord.E ends up with different terminal states on M1 vs. M2.
- **Location**: DESIGN L2337-2357. Also affects the `retry_failed` path since `Rewound` events land on per-child files independently of parent conflict.
- **Severity**: Medium-High. Silent data corruption across machines.
- **Proposed resolution**: `koto session resolve` must also identify and list children whose state files differ from the canonical parent's expectations; offer `--accept remote` / `--accept local` to extend to children, or at minimum warn and refuse to proceed without an explicit child-reconciliation flag.

### Finding 5 — No lock prevents a second process from submitting evidence to the same parent
- **Observation**: Probe 7 showed `koto init` correctly rejects a duplicate, but `koto next coord --with-data` does not. Any concurrent process can inject tasks.
- **Location**: DESIGN L1982-1992 inherits the "one process per workflow" v0.7.0 assumption without enforcement.
- **Severity**: Medium. Requires user error (two shells) or misbehaving agent, but the v1 shirabe integration plan explicitly runs multiple sub-agents per parent.
- **Proposed resolution**: Add a cheap advisory `.lock` file in the session dir acquired for the duration of `handle_next` on parent workflows. Release on exit. Document that this is advisory and caller is still responsible for overall coordination.

### Finding 6 — Order of `ScheduledTask` log append vs. `init_state_file` is unspecified
- **Observation**: Probe 6's mid-spawn crash recovery depends on whether the parent log records the intent-to-spawn before or after the child file lands. Current text at L1877-1886 does not pin this order, and L1895-1896 emphasizes "pure function of log + children on disk" which implies either order must yield the same classification.
- **Location**: DESIGN L1877-1896.
- **Severity**: Low-Medium. Implementation detail, but reviewers and future maintainers will hit the same question.
- **Proposed resolution**: State the rule explicitly: "Child init happens first; the parent log records no per-task `ScheduledTask` event. The scheduler derives spawn status purely from `backend.exists`. Response `scheduler.spawned` is a per-tick observation and not persisted." This aligns with L1895-1906's disk-derivation principle and simplifies recovery.

### Finding 7 — "Only one parent tick at a time" invariant is caller-held with no enforcement or diagnostic
- **Observation**: The caller invariant (L1946-1980) is load-bearing for correctness, but koto provides no mechanism to (a) detect a violation, (b) fail loudly, or (c) guide the consumer to the right pattern. The walkthrough says "any caller can do it" (driving the parent), which agents will read literally.
- **Location**: DESIGN L1931-2010 (Concurrency model); walkthrough line "the coordinator just starts driving the parent's workflow name again".
- **Severity**: High (compounds Findings 2 and 5).
- **Proposed resolution**: Pair the advisory lockfile from Finding 5 with a clear error message: "another process is currently running a scheduler tick against `coord` (PID=NNN, acquired HH:MM:SS); koto next parent must be serialized. See docs/CONCURRENCY.md." Update the walkthrough to say "the coordinator owns parent ticks; workers never call `koto next <parent>`."

### Finding 8 — `retry_failed` under cloud sync can silently desynchronize child epochs from parent epoch consensus
- **Observation**: Probe 3 showed a `retry_failed` on the losing machine appends a `Rewound` to the child's state file before the parent-log conflict is detected. If the parent loses the race and is reset via `resolve`, the child's Rewound stays — an epoch bump without a matching parent-side `retry_failed` consumption.
- **Location**: DESIGN L1907-1929 (Retry), L2337-2357 (Cloud sync).
- **Severity**: Medium. Produces phantom epochs where child evidence from the "pre-rewind" epoch is invisible but the parent doesn't remember why.
- **Proposed resolution**: Make `handle_retry_failed` write the clearing `retry_failed: null` evidence to the parent BEFORE appending Rewound to children, and have `check_sync` fail early (before the Rewounds) when the parent is behind remote. Alternative: version-stamp child Rewound events with the parent log's `expected_seq` at the time of retry, and have the scheduler detect and warn on epoch mismatches after resolve.

### Finding 9 — Mid-spawn crash recovery does not cover the TOCTOU-plus-stale-tmp interaction
- **Observation**: `repair_half_initialized_children` (L2120-2125) handles "header but no events," which Phase 1's atomic init makes impossible for normal paths. It does not cover a leaked `.tmp` file from a second-racer whose `rename` never ran. `backend.cleanup` eventually reaps but no guarantee on when.
- **Location**: DESIGN L709-713, L2120-2125.
- **Severity**: Low. The leak is benign (ignored by `list` and `exists`), but accumulates under repeated crashes and pollutes the session dir.
- **Proposed resolution**: Run a cheap tmp-file sweep at the start of every `run_batch_scheduler` tick: `for entry in list_tempfiles(session_dir) where mtime < now - 60s: remove`. Document in the backend-trait method.

### Finding 10 — Dynamic-addition evidence semantics interact with `children-complete` gate arithmetic
- **Observation**: If WORKER-A submits F, G before C, D, E complete, the gate's `total` jumps from 5 to 7 mid-run. A consumer watching `all_complete` transitions could observe `all_complete: true` momentarily (if at some instant C, D, E all terminal and F, G not yet appended in reader's snapshot) and then `all_complete: false` again — a spurious transition.
- **Location**: DESIGN L1894-1906 (Resume derivation), gate output fields L2103-2106.
- **Severity**: Medium. Hurts idempotency of consumers who trigger on `all_complete`.
- **Proposed resolution**: Specify gate evaluation semantics: `all_complete` is computed against the task-list snapshot AS OF this tick's evidence event stream. Document that consumers must not treat `all_complete: true` as final until the parent transitions out of `plan_and_await`. Alternatively, disallow task-list extension once the parent's gate has ever been observed as `all_complete: true`.
