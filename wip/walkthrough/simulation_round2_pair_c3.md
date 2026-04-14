# Round 2, Pair C3 — Nested batches (Shape C) compose with CD9-CD14

Scenario: 2-level hierarchy. Outer parent `coord-outer` has three
linear tasks A, B, C. B's template is `impl-issue-with-subbatch.md`,
which itself declares a `materialize_children` hook. When B runs it
submits its own task list (B1, B2 in parallel) and spawns an inner
batch. Terminal success on every leaf.

Deterministic naming per Decision E4:
- Level 1: `coord-outer.A`, `coord-outer.B`, `coord-outer.C`
- Level 2: `coord-outer.B.B1`, `coord-outer.B.B2`

Round 2's job is NOT to re-litigate Decision E1 (flat vs nested).
Reading A is chosen; Reading B (nested batches via `koto init
--parent <running-child>`) remains "available unchanged from v0.7.0."
This simulation verifies that CD9-CD14's new surface (gate-derived
booleans, `reserved_actions`, delete-and-respawn, R8, advisory flock,
`BatchFinalized`/`batch_final_view`, `synthetic: true`,
`skipped_because_chain`) does not silently break the nested-batch
composition.

---

## Section 1: Transcript

All `koto status` omissions assume `LocalBackend` (no `sync_status`
/ `machine_id` fields).

### Turn 1 — AGENT: init outer + first tick

```bash
koto init coord-outer --template coord.md --var plan_path=PLAN-nested.md
koto next coord-outer
```

KOTO response to `init`:
```json
{"action":"initialized","workflow":"coord-outer","state":"plan_and_await","template":"coord.md"}
```

KOTO response to `koto next coord-outer`:
- Advisory flock acquired on `<session>/coord-outer.lock` (CD12 Q3;
  outer state has `materialize_children`).
- Advance loop at `plan_and_await`. `tasks` required but no evidence.
- Gate zero-fills. Scheduler: `NoBatch`.

```json
{
  "action": "evidence_required",
  "state": "plan_and_await",
  "directive": "Read the plan document at PLAN-nested.md. ...",
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "tasks": {"type":"tasks","required":true,"item_schema":{...}}
    }
  },
  "blocking_conditions": [{
    "name":"done","type":"children-complete","category":"temporal",
    "output":{"total":0,"completed":0,"pending":0,"success":0,
      "failed":0,"skipped":0,"blocked":0,"spawn_failed":0,
      "all_complete":false,"all_success":false,
      "any_failed":false,"any_skipped":false,"needs_attention":false,
      "children":[]}
  }],
  "scheduler": null
}
```

Gloss: "Outer parent is waiting on a task list. No knowledge yet
that B's template will itself spawn a sub-batch — that's per-child
template business and invisible at this layer."

### Turn 2 — AGENT: submit outer task list

AGENT writes `tasks-outer.json`:
```json
{"tasks": [
  {"name":"A","template":"impl-issue.md","vars":{"ISSUE_NUMBER":"101"}},
  {"name":"B","template":"impl-issue-with-subbatch.md","vars":{"ISSUE_NUMBER":"102"},"waits_on":["A"]},
  {"name":"C","template":"impl-issue.md","vars":{"ISSUE_NUMBER":"103"},"waits_on":["B"]}
]}
```

```bash
koto next coord-outer --with-data @tasks-outer.json
```

KOTO: validates R1-R10 (including R8 vacuously — no prior spawns);
appends `EvidenceSubmitted{tasks}`; scheduler runs, spawns A;
gate re-evaluates.

```json
{
  "action":"gate_blocked","state":"plan_and_await",
  "blocking_conditions":[{
    "name":"done","type":"children-complete","category":"temporal",
    "output":{"total":3,"completed":0,"pending":1,"success":0,
      "failed":0,"skipped":0,"blocked":2,"spawn_failed":0,
      "all_complete":false,"all_success":false,
      "any_failed":false,"any_skipped":false,"needs_attention":false,
      "children":[
        {"name":"coord-outer.A","state":"working","complete":false,"outcome":"pending"},
        {"name":"coord-outer.B","state":null,"complete":false,"outcome":"blocked","blocked_by":["coord-outer.A"]},
        {"name":"coord-outer.C","state":null,"complete":false,"outcome":"blocked","blocked_by":["coord-outer.B"]}
      ]}
  }],
  "scheduler":{
    "spawned_this_tick":["coord-outer.A"],
    "materialized_children":[
      {"name":"coord-outer.A","outcome":"pending","state":"working"}
    ],
    "already":[],"blocked":["coord-outer.B","coord-outer.C"],
    "skipped":[],"errored":[],"warnings":[],
    "feedback":{"entries":{
      "A":{"outcome":"accepted"},
      "B":{"outcome":"blocked","waits_on":["A"]},
      "C":{"outcome":"blocked","waits_on":["B"]}
    },"orphan_candidates":[]}
  }
}
```

Gloss: "A spawned. B and C blocked. Outer flock released at handler
exit."

### Turn 3 — AGENT: drive A to completion

```bash
koto next coord-outer.A --with-data '{"status":"complete"}'
```

A's handle_next: `coord-outer.A` is NOT a batch parent (its current
state has no `materialize_children` hook and no prior
`SchedulerRan`/`BatchFinalized` in its log) → **no flock taken**
(CD12 Q3 scopes lock to batch parents). Response:
```json
{"action":"done","state":"done","directive":"Issue #101 implemented successfully.","is_terminal":true}
```

Gloss: "Leaf worker done. No interference with outer." Note that
outer's lock is independent; a worker driving a leaf child never
contends with outer's tick.

### Turn 4 — AGENT: re-tick outer; B spawns

```bash
koto next coord-outer
```

Outer flock acquired. Scheduler runs on `plan_and_await`. A is
success. B's `waits_on: [A]` satisfied → `init_state_file` on
`coord-outer.B` with parent_workflow header `coord-outer` and
`spawn_entry` snapshot (per CD10's amendment to Decision 2). B's
initial state is `plan_and_await` (of `impl-issue-with-subbatch.md`),
which declares `materialize_children`.

```json
{
  "action":"gate_blocked","state":"plan_and_await",
  "blocking_conditions":[{
    "name":"done","type":"children-complete","category":"temporal",
    "output":{"total":3,"completed":1,"pending":1,"success":1,
      "failed":0,"skipped":0,"blocked":1,"spawn_failed":0,
      "all_complete":false,"all_success":false,
      "any_failed":false,"any_skipped":false,"needs_attention":false,
      "children":[
        {"name":"coord-outer.A","state":"done","complete":true,"outcome":"success"},
        {"name":"coord-outer.B","state":"plan_and_await","complete":false,"outcome":"pending"},
        {"name":"coord-outer.C","state":null,"complete":false,"outcome":"blocked","blocked_by":["coord-outer.B"]}
      ]}
  }],
  "scheduler":{
    "spawned_this_tick":["coord-outer.B"],
    "materialized_children":[
      {"name":"coord-outer.A","outcome":"success","state":"done"},
      {"name":"coord-outer.B","outcome":"pending","state":"plan_and_await"}
    ],
    "already":["coord-outer.A"],"blocked":["coord-outer.C"],
    "skipped":[],"errored":[],"warnings":[],
    "feedback":{"entries":{
      "A":{"outcome":"already"},
      "B":{"outcome":"accepted"},
      "C":{"outcome":"blocked","waits_on":["B"]}
    },"orphan_candidates":[]}
  }
}
```

**[GAP 1: visibility of nested-batch-ness is nil]** Outer's gate
classifies B as `outcome: "pending"` with state `plan_and_await`.
The response gives the AGENT no signal that B is itself a batch
parent that will need its own tick to progress. CD9's
`reserved_actions` discovery story assumes the agent reads the
top-level response; here the outer response says nothing about "B
will demand a sub-batch submission of its own." An agent unfamiliar
with B's template could call `koto next coord-outer.B` and see
`evidence_required` on `tasks` and be confused: is that a retry
signal? A misconfiguration? Nothing in the design's round-1 surface
marks an intermediate child as "itself a batch coordinator." This
is a pre-existing primitive-composition gap, not a CD9-CD14
regression.

### Turn 5 — AGENT: drive B (now a sub-coordinator)

```bash
koto next coord-outer.B
```

B's handle_next: B's current state is `plan_and_await` with
`materialize_children` hook → **flock on `<session>/coord-outer.B.lock`**
(CD12 Q3 applies). Note: this is a DIFFERENT lockfile from outer's
`coord-outer.lock`. A worker driving B can acquire its lock even
while another process holds outer's.

B's advance loop: `tasks` required, not submitted. Gate zero-fills.

```json
{
  "action":"evidence_required","state":"plan_and_await",
  "directive":"Submit the sub-batch task list...",
  "expects":{"event_type":"evidence_submitted","fields":{
    "tasks":{"type":"tasks","required":true,"item_schema":{...}}
  }},
  "blocking_conditions":[{
    "name":"done","type":"children-complete","category":"temporal",
    "output":{"total":0,"completed":0,"pending":0,"success":0,
      "failed":0,"skipped":0,"blocked":0,"spawn_failed":0,
      "all_complete":false,"all_success":false,
      "any_failed":false,"any_skipped":false,"needs_attention":false,
      "children":[]}
  }],
  "scheduler":null
}
```

Gloss: "B looks exactly like a top-level coord — no hint it's
nested. That's consistent with 'Reading B unchanged from v0.7.0.'"

### Turn 6 — AGENT: submit B's sub-batch

`tasks-B.json`:
```json
{"tasks":[
  {"name":"B1","template":"impl-issue.md","vars":{"ISSUE_NUMBER":"102-a"}},
  {"name":"B2","template":"impl-issue.md","vars":{"ISSUE_NUMBER":"102-b"}}
]}
```

```bash
koto next coord-outer.B --with-data @tasks-B.json
```

KOTO: B's flock acquired. Scheduler spawns `coord-outer.B.B1` and
`coord-outer.B.B2` (no `waits_on`, so both ready immediately).
Name-composition works cleanly because `.` is legal and CD4 says
"parents can't be renamed anyway." B is the parent of its own
children; its name `coord-outer.B` prefixes them, producing
`coord-outer.B.B1`.

```json
{
  "action":"gate_blocked","state":"plan_and_await",
  "blocking_conditions":[{
    "name":"done","type":"children-complete","category":"temporal",
    "output":{"total":2,"completed":0,"pending":2,"success":0,
      "failed":0,"skipped":0,"blocked":0,"spawn_failed":0,
      "all_complete":false,"all_success":false,
      "any_failed":false,"any_skipped":false,"needs_attention":false,
      "children":[
        {"name":"coord-outer.B.B1","state":"working","complete":false,"outcome":"pending"},
        {"name":"coord-outer.B.B2","state":"working","complete":false,"outcome":"pending"}
      ]}
  }],
  "scheduler":{
    "spawned_this_tick":["coord-outer.B.B1","coord-outer.B.B2"],
    "materialized_children":[
      {"name":"coord-outer.B.B1","outcome":"pending","state":"working"},
      {"name":"coord-outer.B.B2","outcome":"pending","state":"working"}
    ],
    "already":[],"blocked":[],"skipped":[],"errored":[],"warnings":[],
    "feedback":{"entries":{
      "B1":{"outcome":"accepted"},"B2":{"outcome":"accepted"}
    },"orphan_candidates":[]}
  }
}
```

**[GAP 2: outer gate is blind to inner progress]** If AGENT now
calls `koto next coord-outer`, outer's gate evaluates B's outcome
via the shared v0.7.0 predicate: B is non-terminal → outer sees B
as `outcome: "pending"`. This is semantically correct (B hasn't
terminated), but it means outer's aggregate counters, scheduler
feedback, and any `batch_final_view` at outer level never see any
detail about B1/B2. `children-complete` does not recurse (design
rationale in E1 alternatives: "outer parent never waits for
grandchildren"). That's the chosen semantics — but it means CD13's
"batch_final_view gives agents the full snapshot" is only true per
level. An agent summarizing at outer level has no direct access to
which inner grandchildren succeeded.

Gloss: "Outer sees B pending. Inner sees B1, B2 pending. Two
independent batch views."

### Turn 7 — AGENT: drive B1, B2 to completion

```bash
koto next coord-outer.B.B1 --with-data '{"status":"complete"}'
koto next coord-outer.B.B2 --with-data '{"status":"complete"}'
```

Both return:
```json
{"action":"done","state":"done","directive":"Issue #... implemented successfully.","is_terminal":true}
```

Gloss: "Leaves done. No flock taken for either (not batch
parents)."

### Turn 8 — AGENT: re-tick B; inner batch finalizes

```bash
koto next coord-outer.B
```

B's flock acquired. Advance loop at `plan_and_await`. Gate: both
children terminal, `all_success: true`. Transition fires:
`plan_and_await → summarize` (B's own terminal). Per CD13, advance
loop appends `BatchFinalized` to B's log on this first
`all_complete: true` pass.

Important: the `BatchFinalized` event lives in `coord-outer.B`'s
event log — NOT outer's. There are two `BatchFinalized` events
across the hierarchy (eventually), one per batch parent.

```json
{
  "action":"done","state":"summarize","is_terminal":true,
  "directive":"Sub-batch for issue #102 complete. The batch_final_view field in this response carries the snapshot...",
  "batch_final_view":{
    "phase":"final",
    "summary":{"total":2,"success":2,"failed":0,"skipped":0,"pending":0,"blocked":0,"spawn_failed":0},
    "tasks":[
      {"name":"B1","child":"coord-outer.B.B1","outcome":"success"},
      {"name":"B2","child":"coord-outer.B.B2","outcome":"success"}
    ],
    "ready":[],"blocked":[],"skipped":[],"failed":[]
  }
}
```

**[GAP 3: inner batch_final_view doesn't propagate]** Outer's
terminal `done` response (Turn 12 below) will carry outer's OWN
`batch_final_view` — a 3-task view listing A, B, C as success. It
will NOT contain B's inner batch_final_view recursively. Under E1
the design is explicit that children-complete doesn't recurse;
CD13 inherits that restriction. An agent wanting "full tree of
what happened" must walk the hierarchy via
`koto workflows --children` and query each batch parent
individually. Design is silent on whether this is intentional or
whether a top-level aggregate would be useful. Probably
intentional — recursive aggregation inflates response sizes and
violates the per-level isolation that makes each parent's scheduler
self-contained. But the design does not say so explicitly.

### Turn 9 — AGENT: re-tick outer; B now success, C unblocks

```bash
koto next coord-outer
```

Outer flock acquired. Scheduler runs. B is now terminal-success
(`state: summarize`, not failure, not skip marker). C's
`waits_on: [B]` satisfied → spawn C.

```json
{
  "action":"gate_blocked","state":"plan_and_await",
  "blocking_conditions":[{
    "name":"done","type":"children-complete","category":"temporal",
    "output":{"total":3,"completed":2,"pending":1,"success":2,
      "failed":0,"skipped":0,"blocked":0,"spawn_failed":0,
      "all_complete":false,"all_success":false,
      "any_failed":false,"any_skipped":false,"needs_attention":false,
      "children":[
        {"name":"coord-outer.A","state":"done","complete":true,"outcome":"success"},
        {"name":"coord-outer.B","state":"summarize","complete":true,"outcome":"success"},
        {"name":"coord-outer.C","state":"working","complete":false,"outcome":"pending"}
      ]}
  }],
  "scheduler":{
    "spawned_this_tick":["coord-outer.C"],
    "materialized_children":[
      {"name":"coord-outer.A","outcome":"success","state":"done"},
      {"name":"coord-outer.B","outcome":"success","state":"summarize"},
      {"name":"coord-outer.C","outcome":"pending","state":"working"}
    ],
    "already":["coord-outer.A","coord-outer.B"],"blocked":[],
    "skipped":[],"errored":[],"warnings":[],
    "feedback":{"entries":{
      "A":{"outcome":"already"},"B":{"outcome":"already"},
      "C":{"outcome":"accepted"}
    },"orphan_candidates":[]}
  }
}
```

Gloss: "Outer gate reads B's `state: summarize` + B's terminal +
not-failure → outcome=success. Exactly the v0.7.0
classification."

### Turn 10 — AGENT: drive C; final outer tick

```bash
koto next coord-outer.C --with-data '{"status":"complete"}'
koto next coord-outer
```

C done. Outer gate: all_success → transition to `summarize`.
Outer's own `BatchFinalized` appended.

```json
{
  "action":"done","state":"summarize","is_terminal":true,
  "directive":"Write a summary... batch_final_view carries the snapshot.",
  "batch_final_view":{
    "phase":"final",
    "summary":{"total":3,"success":3,"failed":0,"skipped":0,"pending":0,"blocked":0,"spawn_failed":0},
    "tasks":[
      {"name":"A","child":"coord-outer.A","outcome":"success"},
      {"name":"B","child":"coord-outer.B","outcome":"success"},
      {"name":"C","child":"coord-outer.C","outcome":"success"}
    ],
    "ready":[],"blocked":[],"skipped":[],"failed":[]
  }
}
```

Gloss: "Outer's view is the 3-task outer batch. Inner's view
stays with B. No cross-contamination."

---

## Section 2: Edge-case probes

### Probe E1 — Two lockfiles, no deadlock

Outer flock path: `<session>/coord-outer.lock`. Inner flock path:
`<session>/coord-outer.B.lock`. Different paths, independent file
handles. A worker driving B holds B's lock; it can run to
completion without ever touching outer's lock. CD12 Q3 says the
lock is held "for the duration of the call" — so a single
`koto next coord-outer.B` call does NOT cascade into an outer
tick. Outer is advanced only on explicit `koto next coord-outer`.

**Verification:** no deadlock is possible under the design. The
coordinator-owns-parent partition from CD12 Q8 ("coordinator drives
the parent; workers drive only their own children") still holds,
with the wrinkle that B is both a coordinator (of B1, B2) and a
child (of coord-outer). **In this shape the AGENT is itself the
coordinator for BOTH levels** because a single agent drives the
whole hierarchy. CD12 Q8's language doesn't say "one coordinator
per level" — it says one coordinator per batch parent. Two levels
= two batch parents = two (nominally distinct) coordinators. In
practice the same AGENT plays both roles, sequentially.

**[GAP 4: CD12 Q8 coordinator-partition extends poorly to nesting]**
The walkthrough language CD12 Q8 mandates says "the coordinator
drives the parent; workers drive only their own children." Under
a 2-level hierarchy, who drives B? B is a child of outer (so a
worker), but also a batch parent (so a coordinator). Under strict
partitioning, no role can legally drive B. In practice agents fuse
the roles when a single human/agent orchestrates the whole tree,
but the skill-level prose from CD12 Q8 doesn't acknowledge the
two-hat case. Nice-to-have to document.

### Probe E2 — `batch_final_view` nesting semantics

Not recursive. Outer's `batch_final_view` shows B as `outcome:
"success"` with no inner detail. Inner's `batch_final_view`
lives on B's own terminal response.

**Verification:** CD13's design preserves the per-parent isolation
from E1. An agent consuming outer's final view knows B succeeded
but must query B separately to see which grandchildren ran. This
is consistent, but undocumented — CD13 should explicitly note
that `batch_final_view` is per-level and not recursive.

### Probe E3 — `reserved_actions` at inner level

Hypothetical: B1 fails. Inner B's gate reports
`any_failed: true` → inner B's `analyze_failures` transition fires
(assuming B's template mirrors the reference `coord.md`'s routing).
Inner B's response carries `reserved_actions[0]` with
`invocation: "koto next coord-outer.B --with-data '{\"retry_failed\":{\"children\":[\"coord-outer.B.B1\"]}}'"`.
The `applies_to` array lists `coord-outer.B.B1`.

When AGENT next ticks outer (`koto next coord-outer`), outer's
gate sees B as `outcome: "pending"` — B is at `analyze_failures`,
not terminal. No `reserved_actions` at outer level (outer's gate
reports `needs_attention: false` because no OUTER child is
failed/skipped). Outer is simply blocked awaiting B.

**Verification:** `reserved_actions` is per-level. Retry scope is
per-batch. An inner failure never surfaces as a retry opportunity
at outer level. This is consistent — the outer parent can't
retry an inner batch; that's B's own concern. But it means the
AGENT has to read the inner response to discover the retry path.
If the AGENT is only watching outer, it'll see B stuck at pending
indefinitely with no signal about why.

**[GAP 5: outer gate report is opaque when inner is stuck]**
Outer's gate says `pending` for B, but doesn't expose that B
itself is in `analyze_failures` awaiting retry. Outer's
`children[*].state` field DOES carry `analyze_failures`, so the
raw state name is visible. But there's no derived signal
(`blocked_on_inner_failure`, `awaiting_user_decision`, etc.)
indicating B needs human attention. An agent keying on `outcome`
alone sees `pending` and assumes B is progressing.

**Proposal:** the outer gate's per-child row could carry an
optional `subbatch_status` field when a child is itself a batch
parent, summarizing inner aggregate (`any_failed: true` →
`subbatch_needs_attention: true`). This is additive, follows
CD13's observability-only constraint, and solves the "why is B
stuck" question without recursing the full batch view. Design
silent on this today.

### Probe E4 — R8 across levels

Outer's R8 scope: task entries in outer's batch (A, B, C). If
AGENT resubmits tasks to outer after B has spawned but changes
B's `vars`, R8 rejects at outer level with
`SpawnedTaskMutated{task:"B",changed_fields:[...]}`.

Inner's R8 scope: task entries in B's batch (B1, B2). Submitted
as part of B's `EvidenceSubmitted`, not outer's. Outer's
submission can't touch inner's entries; they live in a different
workflow's log entirely. Cross-level R8 conflict is
structurally impossible.

**Verification:** R8 composes cleanly across levels. Each batch
parent owns its own `spawn_entry` snapshots on its own children's
`WorkflowInitialized` events. No shared state. This is a positive
composition result.

### Probe E5 — Retry across levels

Scenario: B1 fails. Inner B transitions to `analyze_failures`.
AGENT submits retry_failed to OUTER for B (not to B for B1):
```bash
koto next coord-outer --with-data '{"retry_failed":{"children":["coord-outer.B"]}}'
```

R10 validates:
- B exists on disk: yes.
- B's outcome: `pending` (B is non-terminal at `analyze_failures`).
- R10 requires outcome `failure` or `skipped` → **rejected with
  `ChildNotEligible`**.

So the outer-level retry doesn't fire. Correct.

Alternative scenario: B's internal recovery fails and B
transitions to a terminal failure state (e.g., AGENT submits
`{"decision":"give_up"}` to B, routing B to a `summarize` or
`failed` terminal). If `summarize` is marked `failure: true`, B's
outcome at outer level becomes `failure`. THEN an outer retry
targeting B is eligible under R10.

What happens? `handle_retry_failed` appends `Rewound` targeting
B's initial state (`plan_and_await` of
`impl-issue-with-subbatch.md`). B rewinds. But the inner
children B1, B2 — they still exist on disk. B's advance loop
runs, sees its state back at `plan_and_await`, and the scheduler
on B... picks up the existing children via
`backend.exists()` idempotency.

**[GAP 6: rewinding a nested batch parent doesn't cascade
rewinds]** When B rewinds to `plan_and_await`, B1 and B2 are in
whatever state they ended up in. If B1 was `done_blocked`
(failure), it's still `done_blocked` on disk — B's rewind event
touched only B's log, not B1's. Under CD9's runtime
reclassification, B's scheduler on the next tick would see B1's
outcome `failure` and potentially reclassify B2 as a skip
marker if `waits_on` dependencies demand it (they don't in this
scenario — B2 has no waits_on). So B's batch state post-rewind
looks identical to pre-rewind: B1 still failure, B2 still
success. The retry is cosmetic at inner level — B is back at
`plan_and_await` but its children haven't moved.

For the outer-level retry to MEAN anything for the inner batch,
the outer `handle_retry_failed` would have to recurse: rewind B
AND rewind B's batch children. Design doesn't do this, nor does
it explicitly rule it out. Decision 9 Part 2's "downward closure
of the retry set" is scoped to the parent's own `waits_on`
graph — outer's graph knows about A, B, C, not about B1, B2.

**Severity: blocker for nested retry semantics.** An agent
submitting outer-level retry for B expects "redo B from
scratch." Under current design, B restarts but inherits its
broken inner batch. Either:
(a) Document that outer-level retry of a nested batch parent
    does NOT cascade and recommend agents retry at the inner
    level first (then submit at outer level with `acknowledge`),
    OR
(b) Extend CD9 to recurse: a rewound batch-parent child's retry
    closure includes its inner batch's failed/skipped
    descendants, which are also delete-and-respawned.

Option (a) is cheaper and respects the "each level is
independent" invariant from E1. Option (b) is nicer for
agents but adds cross-level coupling the design has avoided
everywhere else.

### Probe E6 — Synthetic skips across levels

Scenario: A fails at outer. Outer's gate marks B as skipped
(via `waits_on: [A]` + `skip_dependents` policy). Scheduler
delete-and-respawns B (via CD9 Part 5) into B's real template's
`skipped_marker: true` state. But wait — B's template is
`impl-issue-with-subbatch.md`. Does that template declare a
reachable `skipped_marker: true` state?

Per F5 (compile warning): any batch-eligible child template must
declare a reachable `skipped_marker: true` state. B's template
is batch-eligible (it's listed as a child template under outer's
`materialize_children`). F5 fires at compile time if
`impl-issue-with-subbatch.md` lacks such a state.

Assume it does (some `skipped_due_to_dep_failure` state with
`skipped_marker: true`). Scheduler spawns B directly into that
state. B never reaches `plan_and_await`; B's own `materialize_children`
hook never fires. B1, B2 never materialize.

C's `waits_on: [B]`. B's outcome is `skipped`. Per
`skip_dependents`, C is also skip-reclassified.

**Verification:** works cleanly. Skip markers prevent nested
batch materialization because the child never reaches the state
that has the hook. CD9 Part 5 + F5 compose correctly with
nested batches.

**[GAP 7: synthetic skip's `skipped_because_chain` doesn't
recurse]** B's skip record: `skipped_because: "coord-outer.A"`,
`skipped_because_chain: ["coord-outer.A"]`. If the agent
afterward asked about B1 (which never materialized), B1 has no
state file at all — `koto status coord-outer.B.B1` returns a
"not found" error. So there's no B1-level skip chain to trace.
That's correct (nothing to trace), but the AGENT walking the
hierarchy top-down might expect to see "B1 was skipped because
B's template never ran because A failed." No such record
exists. The hierarchy just stops at B.

This isn't a design bug per se — it's consistent with "skip
marker = child respawned into terminal, no batch expansion."
But design should document that skipped batch-parent children
do NOT produce grandchildren on disk. An agent tracing
provenance across levels needs to know the hierarchy
terminates at the first skip.

### Probe E7 — Flock inheritance / interference

`handle_next coord-outer` acquires `coord-outer.lock`.
`handle_next coord-outer.B` acquires `coord-outer.B.lock`.
Different paths. No interference.

A pathological case: AGENT calls `koto next coord-outer` (holds
outer lock) and in parallel calls `koto next coord-outer.B`
(acquires inner lock). Outer's scheduler, running on outer's
tick, may call `backend.list()` to enumerate children including
B. B's state may be changing concurrently (inner tick is
advancing B through its own state machine). Outer's gate reads
B's log via a separate file handle; the append-only JSONL + seq
discipline (from the existing design) ensures outer gets a
consistent prefix read even if B is being written to.

**Verification:** under CD12's per-parent lock scope, cross-
level concurrency is safe by design. Outer and inner locks are
fully independent. The design's "pure function of disk state"
model guarantees outer's read-of-B and inner's write-of-B can
interleave without corrupting observed aggregates (modulo the
existing CloudBackend `sync_status` signal from CD12 Q5).

### Probe E8 — Stale `batch_final_view` under nested retry

Scenario from Probe E5 extended: B's terminal-failure
`BatchFinalized` has already been appended to B's log. Outer
retries B. B rewinds to `plan_and_await`. B's log now contains:
```
WorkflowInitialized
Transitioned → plan_and_await
EvidenceSubmitted { tasks: ... }
SchedulerRan { ... }
... (inner batch history) ...
BatchFinalized { summary: {success:1, failed:1, ...} }
Transitioned → analyze_failures
EvidenceSubmitted { decision: "give_up" }
Transitioned → summarize
Rewound → plan_and_await    <-- from outer's retry
```

`koto status coord-outer.B` would replay the **most recent
BatchFinalized** (CD13 says "most recent") and emit a `batch`
section showing the old inner failure. But B is now live at
`plan_and_await` — its batch state is being redefined. Same
GAP 2 from Pair A1: stale `BatchFinalized` during retry. Now
compounded across levels: inner B's stale view is visible via
`koto status coord-outer.B` at the same time outer's live retry
is ticking.

**Verification:** CD13's known stale-view gap (flagged in
Pair A1's F2) propagates without amplification across levels.
Not a nested-specific regression, but nested composition
makes it easier to hit — the agent is now juggling two views
during a nested retry.

---

## Section 3: Probing the "things to probe" list

### P1 — Documentation for nested vs flat choice

Design does warn in Risks section (line 3728-3733) that
"some use cases... are expressible under Reading B... but not
under the flat batch model's single-hook restriction." And
line 3785-3787: "koto-author skill updates note the E8
compile-time check and explain when to use nested batches
(Reading B) instead." So authoring guidance exists AT THE
KOTO-AUTHOR SKILL LEVEL — not in the design doc itself beyond
a mitigation note. Runtime warnings to the agent ("you're
nesting; consider flattening") do NOT exist anywhere. The
design treats nesting as a legitimate pattern, not a
discouraged one.

**Verdict:** guidance is adequate for template authors. No
runtime nudge to agents, which seems fine — agents shouldn't
be restructuring plans.

### P2 — Shared scheduler state between inner and outer?

None. Outer scheduler reads outer's log + outer's children
(A, B, C) via `backend.list()` filtered by `parent_workflow
== coord-outer`. Inner scheduler reads B's log + B's children
(B1, B2) filtered by `parent_workflow == coord-outer.B`.
Disjoint filters. Different event logs. Independent
`BatchFinalized` events. No shared cursors, caches, or
indices.

**Verdict:** fully independent. Clean composition. This is
the single biggest win of the disk-derivation strategy from
E2 — it makes nested composition near-free.

### P3 — `skipped_because_chain` across levels

Chain is intra-batch. Outer's chain: if D skipped because C
skipped because B failed, `D.skipped_because_chain: ["C",
"B"]`. Chain walks `waits_on` within outer's task graph only.
At no point does it cross a `coord-outer.X` → `coord-outer.X.Y`
boundary. CD13's algorithm "walks upstream through `waits_on`
until a failed (non-skipped) ancestor" — `waits_on` is a
within-batch concept, so the walk is structurally confined.

**Verdict:** consistent with E1's per-level isolation.
Documented gap (see Probe E6, GAP 7): an agent walking the
full tree sees the chain terminate at skip markers; no
grandchild records exist.

---

## Section 4: Findings

### F1 — Verified: flock scoping composes cleanly across levels

- **Observation:** CD12 Q3's per-parent advisory flock uses
  `<session>/<parent>.lock`. For `coord-outer` and
  `coord-outer.B`, those are different paths. No deadlock, no
  interference, no cascade on a single `handle_next` call.
- **Verifies round-1 claim?:** Yes. CD12's concurrency
  hardening was designed against single-level batch races; it
  happens to compose with nesting because the lock path derives
  from the workflow name, which is hierarchical by E4's
  naming rule.
- **Severity:** nice-to-have (positive verification).
- **Proposed resolution:** no change. Optionally add a note in
  CD12 Q3 or the walkthrough that the flock-per-parent model
  scales to nested batches without modification.

### F2 — Verified: R8 immutability composes across levels

- **Observation:** R8 scope is `handle_retry_failed` on the
  submitting parent's task entries. Outer and inner have
  different parents, different logs, different `spawn_entry`
  records. Cross-level R8 conflict is structurally
  impossible.
- **Verifies round-1 claim?:** Yes. CD10's R8 was scoped
  per-submission; nested composition doesn't change that.
- **Severity:** nice-to-have (positive verification).
- **Proposed resolution:** no change.

### F3 — Verified: skip markers block nested batch expansion

- **Observation:** When B is skip-reclassified (because its
  outer `waits_on` upstream failed), B is respawned directly
  into its `skipped_marker: true` state. B's
  `plan_and_await` never runs. B's `materialize_children`
  hook never fires. B1, B2 never exist on disk. C cascades
  to skip.
- **Verifies round-1 claim?:** Yes. CD9 Part 5's skip
  representation works correctly with batch-parent child
  templates because skip routes to a state that has no
  `materialize_children` hook.
- **Severity:** nice-to-have (positive verification).
- **Proposed resolution:** no change. Add a note to the
  design that skip markers terminate the hierarchy at that
  node — grandchildren are not materialized. This is
  already implicit but would help agents tracing provenance.

### F4 — New issue: outer retry of nested batch parent doesn't cascade

- **Observation:** If B reaches terminal failure at inner
  level (e.g., inner give-up routes to a `failure: true`
  terminal), outer-level `retry_failed` targeting B rewinds
  B to its initial state but does not rewind B's children.
  B's scheduler on next tick sees the existing children via
  idempotent `backend.exists()` and makes no progress. The
  retry is cosmetic.
- **Verifies round-1 claim?:** No — this is a **new issue**
  arising from the composition of CD9's retry model with
  nested batches. CD9 specified downward closure within the
  submitting parent's task graph; it didn't address "a
  retried child is itself a batch parent."
- **Severity:** blocker for nested retry semantics. Silently
  incorrect behavior — the retry appears to succeed but
  accomplishes nothing.
- **Proposed resolution:** two options:
  (a) **Document as unsupported.** Extend CD9 Part 4 to add
      `InvalidRetryReason::ChildIsBatchParent` OR require
      agents to retry at the inner level first, then submit
      an outer-level `acknowledge_failures` decision if
      still needed. Cheapest, most consistent with E1's
      per-level isolation principle.
  (b) **Cascade rewinds.** When `handle_retry_failed`
      targets a child that is itself a batch parent (detected
      via the child's log containing a `BatchFinalized` or
      a state with `materialize_children`), recursively
      delete-and-respawn the inner batch's
      failed/skipped descendants. Adds cross-level
      coupling and a new complexity surface.
  Option (a) is preferred.

### F5 — New gap: outer gate opaque when inner child is stuck

- **Observation:** When inner B is in `analyze_failures`
  (inner batch failed, awaiting retry or decision), outer's
  gate reports B as `outcome: "pending"`. The raw state name
  is visible in `children[*].state` but no derived signal
  indicates "this child is awaiting user attention on its
  own batch."
- **Verifies round-1 claim?:** No — new gap from CD9/CD13
  composition with nesting. The `needs_attention` boolean is
  per-batch; it doesn't propagate upward.
- **Severity:** should-fix. Agents watching outer alone
  won't know why B is stuck.
- **Proposed resolution:** add an optional
  `subbatch_status` field on per-child rows in outer's gate
  output when the child is detectable as a batch parent
  (has `BatchFinalized` in its log or current state carries
  `materialize_children`). Field summarizes inner aggregate:
  `{"needs_attention": true, "failed": 1, "skipped": 0}`.
  Additive, observability-only, consistent with CD13's
  philosophy. Agents opt in by reading the new field.

### F6 — New gap: CD12 Q8 coordinator-partition poorly defined for nesting

- **Observation:** CD12 Q8's walkthrough language says "the
  coordinator drives the parent; workers drive only their own
  children." Under nested batches, an intermediate child (B)
  is simultaneously a worker (relative to outer) and a
  coordinator (relative to B1, B2). The partition doesn't
  name this case.
- **Verifies round-1 claim?:** No — new gap. CD12 was
  designed against single-level batches.
- **Severity:** nice-to-have. Skill prose ambiguity, not a
  runtime bug.
- **Proposed resolution:** extend CD12 Q8's skill update to
  say "each batch parent has exactly one coordinator; when a
  coordinator's own tasks include batch-parent children, the
  coordinator for those children may be the same agent (if
  single-agent orchestration) or a distinct agent (if
  delegated)." Koto-author and koto-user skills both get the
  same clarification.

### F7 — New gap: `batch_final_view` is per-level, not documented

- **Observation:** Outer's `batch_final_view` shows B as
  `outcome: success` with no inner detail. Inner's
  `batch_final_view` lives on B's terminal response. Agents
  seeking a full tree of outcomes must walk the hierarchy
  manually.
- **Verifies round-1 claim?:** Partial — CD13 provides
  `batch_final_view` but doesn't say it's per-level. E1's
  "children-complete doesn't recurse" is the implicit
  source, but CD13 inherits the restriction without
  documentation.
- **Severity:** nice-to-have. Documentation-only.
- **Proposed resolution:** add a sentence to CD13 noting
  that `batch_final_view` reflects only the direct batch and
  that nested inner views must be queried per-parent. Mirror
  in koto-user skill.

### F8 — New gap: stale `BatchFinalized` during nested retry

- **Observation:** When outer retries B (under F4 option
  (b)'s hypothetical cascade model, or under straightforward
  inner retry), B's log retains its prior `BatchFinalized`
  until a new one is appended. `koto status coord-outer.B`
  during the retry window replays the stale one. Same root
  cause as Pair A1's F2, but nested composition makes it
  more likely the agent hits it.
- **Verifies round-1 claim?:** Same issue as Pair A1's F2,
  now visible in a second context.
- **Severity:** should-fix. Same proposal as Pair A1's F2:
  append a `BatchInvalidated` event on retry, or label
  `batch.phase` as `superseded` until the next
  `BatchFinalized`.
- **Proposed resolution:** see Pair A1 F2.

### F9 — New issue: nested-batch-ness is invisible at outer level

- **Observation:** Outer's response at Turn 4 (where B
  first spawns) gives the agent no signal that B is itself
  a batch parent that will require its own sub-batch
  submission. The raw state `plan_and_await` is visible,
  but that state name is template-specific and the agent
  may not know B's template has a `materialize_children`
  hook.
- **Verifies round-1 claim?:** No — new gap. Pre-existing
  primitive (v0.7.0 hierarchical workflows) never had to
  distinguish "child that's a leaf" from "child that's a
  sub-coordinator." Under the Reading-A batch model,
  agents have started to rely on
  `scheduler.spawned_this_tick` for dispatch; that field
  doesn't indicate "this spawned child needs orchestration,
  not just a worker."
- **Severity:** nice-to-have. Agents with template
  knowledge know what to do; agents treating the response
  as a black box won't.
- **Proposed resolution:** add an optional
  `role: "coordinator" | "worker"` marker to the per-child
  row when the child's template has a
  `materialize_children` hook. Marker is additive and
  discoverable at compile time. Agents consuming it can
  route accordingly.

---

## Summary note

Three findings are verifications (F1, F2, F3): CD12 Q3's
flock, CD10's R8, and CD9 Part 5's skip markers all compose
cleanly with nested v0.7.0 batches. Six findings are new
(F4-F9), none catastrophic at the engine level but one
(F4) is a semantic correctness issue: outer-level retry of
a nested batch parent accomplishes nothing. Two (F5, F9)
are observability gaps where agents watching only the outer
level can't see or react to nested structure. The rest are
documentation-only.

The nested/round-1 composition is structurally sound
because E2's disk-derivation strategy makes each level
fully independent. What breaks is the agent-facing story:
round-1 decisions (CD9's retry, CD13's observability)
were designed for a single level and don't speak to how
agents navigate a hierarchy. All six new findings can be
resolved with additive response-field extensions and
documentation updates — no core design changes required.
