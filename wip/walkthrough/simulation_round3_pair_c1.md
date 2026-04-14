# Round 3, Pair C1 — Nested batches post-fix: verify `ChildIsBatchParent` + `role` / `subbatch_status`

Scenario: same 2-level hierarchy as Round 2 Pair C3. Outer batch
`coord-outer` has three linear tasks A, B, C. B's template is
`impl-issue-with-subbatch.md`, which declares its own
`materialize_children` hook. When B runs, it submits its inner task
list (B1, B2 in parallel).

Round 3 additions under test:
- `InvalidRetryReason::ChildIsBatchParent` rejection for outer-level
  retry targeting a nested-batch child (closes Round 2 C3 F4).
- `MaterializedChild.role: Option<ChildRole>` where `ChildRole ∈
  {Worker, Coordinator}`.
- `MaterializedChild.subbatch_status: Option<BatchSummary>` populated
  when `role == Coordinator` and the child has materialized its own
  children.

Round 3 does NOT introduce cross-level skip chain propagation, nor a
recursive `batch_final_view`. Those remain per-level (Decision E1,
Decision 13).

Deterministic naming (Decision E4):
- Level 1: `coord-outer.A`, `coord-outer.B`, `coord-outer.C`
- Level 2: `coord-outer.B.B1`, `coord-outer.B.B2`

---

## Section 1: Transcript

All `koto status` omissions assume `LocalBackend` (no `sync_status` /
`machine_id` fields).

### Turn 1 — AGENT inits outer, submits task list, A spawns

```bash
koto init coord-outer --template coord.md --var plan_path=PLAN-nested.md
koto next coord-outer --with-data @tasks-outer.json
```

`tasks-outer.json`:
```json
{"tasks":[
  {"name":"A","template":"impl-issue.md","vars":{"ISSUE_NUMBER":"101"}},
  {"name":"B","template":"impl-issue-with-subbatch.md","vars":{"ISSUE_NUMBER":"102"},"waits_on":["A"]},
  {"name":"C","template":"impl-issue.md","vars":{"ISSUE_NUMBER":"103"},"waits_on":["B"]}
]}
```

KOTO response (same skeleton as Pair C3 Turn 2, but each
`MaterializedChild` now carries `role`; `subbatch_status` is omitted
when `role != Coordinator`, per `#[serde(skip_serializing_if = "Option::is_none")]`):

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
      {"name":"coord-outer.A","outcome":"pending","state":"working","ready_to_drive":true,"role":"worker"}
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

Gloss: "A spawned as `role: worker`. B and C blocked — not yet
materialized, so no rows for them in `materialized_children`. `role`
is only observable for children that exist on disk."

### Turn 2 — A succeeds; re-tick outer; B spawns

```bash
koto next coord-outer.A --with-data '{"status":"complete"}'
koto next coord-outer
```

Outer scheduler spawns B. B's initial state is `plan_and_await` of
`impl-issue-with-subbatch.md`, which declares a
`materialize_children` hook. Gate re-evaluates.

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
      {"name":"coord-outer.A","outcome":"success","state":"done","ready_to_drive":false,"role":"worker"},
      {"name":"coord-outer.B","outcome":"pending","state":"plan_and_await","ready_to_drive":true,"role":"coordinator"}
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

**[VERIFIED — Round 3 surface works]** B is now flagged `role:
coordinator` the instant it spawns, because its current state
(`plan_and_await`) carries a `materialize_children` hook. `subbatch_status`
is OMITTED because B has not yet submitted its own tasks — no inner
children exist on disk, nothing to summarize. The Key Interfaces
contract (`#[serde(skip_serializing_if = "Option::is_none")]` on
`subbatch_status`) produces a cleanly elided field rather than a
null. GAP 9 from Pair C3 (role invisibility) is now closed.

Gloss: "Outer agent sees B as role=coordinator without any inner
detail yet. That's the 'needs orchestration, not leaf-worker
dispatch' signal."

### Turn 3 — AGENT calls `koto status coord-outer`

```bash
koto status coord-outer
```

The `koto status` response echoes the `materialized_children` shape
from the gate, so:

```json
{
  "workflow":"coord-outer","state":"plan_and_await","is_terminal":false,
  "children":[
    {"name":"coord-outer.A","outcome":"success","state":"done","role":"worker"},
    {"name":"coord-outer.B","outcome":"pending","state":"plan_and_await","role":"coordinator"},
    {"name":"coord-outer.C","outcome":"blocked","blocked_by":["coord-outer.B"]}
  ],
  "batch":{"phase":"active",...}
}
```

C appears in the outer **gate** row as `blocked` but does NOT appear
in `materialized_children` because it has not yet been spawned on
disk (Decision 10 — `materialized_children` is a disk-derived
snapshot). This matches Pair C3 Turn 4.

### Turn 4 — AGENT drives B (inner coordinator)

```bash
koto next coord-outer.B
```

B's own advance loop: `plan_and_await`, tasks required but not
submitted. Gate zero-fills.

```json
{
  "action":"evidence_required","state":"plan_and_await",
  "directive":"Submit the sub-batch task list for issue #102.",
  "expects":{"event_type":"evidence_submitted","fields":{
    "tasks":{"type":"tasks","required":true,...}
  }},
  "blocking_conditions":[{
    "name":"done","type":"children-complete","category":"temporal",
    "output":{"total":0,"completed":0,...,"children":[]}
  }],
  "scheduler":null
}
```

Gloss: "Standard Reading A evidence-required surface at the inner
level. No special nested marker (per Decision E1 the inner level is
unchanged from v0.7.0)."

### Turn 5 — AGENT submits B's inner task list; B1, B2 spawn

```bash
koto next coord-outer.B --with-data @tasks-B.json
```

`tasks-B.json`:
```json
{"tasks":[
  {"name":"B1","template":"impl-issue.md","vars":{"ISSUE_NUMBER":"102-a"}},
  {"name":"B2","template":"impl-issue.md","vars":{"ISSUE_NUMBER":"102-b"}}
]}
```

Inner scheduler spawns `coord-outer.B.B1` and `coord-outer.B.B2`.
Response identical to Pair C3 Turn 6 in structure. B1 and B2 are
`role: worker` (their templates have no `materialize_children`
hook).

### Turn 6 — AGENT calls `koto status coord-outer` (outer view post inner-spawn)

```bash
koto status coord-outer
```

Outer status re-runs its scheduler+gate against current disk state.
B is still non-terminal at `plan_and_await`, so outer still reports
B as `pending`. BUT: per Round 3 Decision 13 addition, outer now
inspects B's log / children to populate `subbatch_status` on B's
row.

```json
{
  "workflow":"coord-outer","state":"plan_and_await","is_terminal":false,
  "children":[
    {"name":"coord-outer.A","outcome":"success","state":"done","role":"worker"},
    {"name":"coord-outer.B","outcome":"pending","state":"plan_and_await","role":"coordinator",
     "subbatch_status":{
       "total":2,"success":0,"failed":0,"skipped":0,"pending":2,"blocked":0,"spawn_failed":0
     }},
    {"name":"coord-outer.C","outcome":"blocked","blocked_by":["coord-outer.B"]}
  ],
  "batch":{"phase":"active",...}
}
```

**[VERIFIED — subbatch_status populates at expected point]**
`subbatch_status` is now populated because B has materialized at
least one inner child (B1 and B2 exist on disk under
`parent_workflow == coord-outer.B`). Shape matches `BatchSummary`
(same struct used in `batch_final_view.summary`). Decision 13's
"outer-level observers wanting a glimpse of inner-batch progress use
the per-child `subbatch_status` field" is exercised here as
intended.

**[AMBIGUITY A1 — populate trigger]** Design text says
`subbatch_status` "carries a summary of the sub-batch the child is
driving" when `role == Coordinator`. Two plausible triggers: (a)
populate whenever `role == Coordinator` (even pre-submission; zero
counters); (b) populate only after the child has materialized at
least one inner child. Option (a) keeps the shape predictable (the
field either is or isn't there, determined solely by `role`); option
(b) conflates "no children yet" with "no subbatch." Design prose
doesn't disambiguate. Recommend (a) — if `role: coordinator`,
`subbatch_status` is always present (with zero counters pre-
materialization). This simulation assumes (a) in the Turn 2
response (subbatch_status omitted at spawn because zeros are not
useful and the `skip_serializing_if` choice was "omit when null,"
not "always emit"). Flag for design confirmation.

Gloss: "Outer observer can now see inner progress at a glance
without a second command. Still no retry options exposed — that
stays inner."

### Turn 7 — AGENT calls `koto status coord-outer.B` (inner view)

```bash
koto status coord-outer.B
```

Standard batch status for B's batch:

```json
{
  "workflow":"coord-outer.B","state":"plan_and_await","is_terminal":false,
  "parent_workflow":"coord-outer",
  "children":[
    {"name":"coord-outer.B.B1","outcome":"pending","state":"working","role":"worker"},
    {"name":"coord-outer.B.B2","outcome":"pending","state":"working","role":"worker"}
  ],
  "batch":{"phase":"active",...}
}
```

`parent_workflow` field comes from B's state file header (Decision
E4 / Decision 2; present since v0.7.0). No `subbatch_status` on B1
or B2 (their `role` is `worker`, field elided).

### Turn 8 — B1 succeeds, B2 fails; inner gate reaches `needs_attention: true`

```bash
koto next coord-outer.B.B1 --with-data '{"status":"complete"}'
koto next coord-outer.B.B2 --with-data '{"status":"failed","reason":"tests failed"}'
koto next coord-outer.B
```

Inner scheduler runs, gate sees B1 terminal-success and B2
terminal-failure. `any_failed: true`, `needs_attention: true`. B's
template routes `plan_and_await → analyze_failures` on
`needs_attention`.

B's response carries Decision 9's `reserved_actions` block with a
retry invocation string. B is NOT terminal (it's at
`analyze_failures`, awaiting decision).

### Turn 9 — AGENT ticks outer; inner failure is visible via `subbatch_status`

```bash
koto next coord-outer
```

Outer gate evaluates. B is non-terminal (`state: analyze_failures`,
no terminal/failure marker at outer level yet) → outer classifies B
as `outcome: pending`. Outer `needs_attention` stays false (no
OUTER child is failed/skipped/spawn_failed).

BUT: per Round 3 `subbatch_status`, B's row now exposes the inner
failure aggregate:

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
        {"name":"coord-outer.A","state":"done","complete":true,"outcome":"success","role":"worker"},
        {"name":"coord-outer.B","state":"analyze_failures","complete":false,"outcome":"pending","role":"coordinator",
         "subbatch_status":{"total":2,"success":1,"failed":1,"skipped":0,"pending":0,"blocked":0,"spawn_failed":0}},
        {"name":"coord-outer.C","state":null,"complete":false,"outcome":"blocked","blocked_by":["coord-outer.B"]}
      ]}
  }],
  "scheduler":{...}
}
```

**[VERIFIED — Round 2 C3 F5 is closed]** The agent watching outer
can now tell B is blocked on an inner failure by reading
`subbatch_status.failed > 0`. No need to drill into
`koto status coord-outer.B` to discover that B is stuck.

**[AMBIGUITY A2 — inner `needs_attention` not promoted]** The
`BatchSummary` struct (used for both `batch_final_view.summary` and
`subbatch_status`) carries counts but NOT the derived
`needs_attention` / `any_failed` booleans — those are on the gate
row, not on `BatchSummary`. So the agent must compute
`subbatch_status.failed + subbatch_status.skipped + subbatch_status.spawn_failed > 0`
itself to derive "inner needs attention." Design doesn't explicitly
say whether the derived booleans should also surface here. A
lightweight add would be a derived `subbatch_needs_attention:
bool` alongside `subbatch_status` on the outer row. Not a blocker —
the raw counts carry the signal — but a nice-to-have for agent
ergonomics.

### Turn 10 — AGENT attempt 1: outer retries B (rejected)

```bash
koto next coord-outer --with-data '{"retry_failed":{"children":["coord-outer.B"]}}'
```

R10 validates. `handle_retry_failed` inspects each named child:

- `coord-outer.B` exists on disk: yes.
- `coord-outer.B`'s outcome: `pending` (B is non-terminal at
  `analyze_failures`). R10 requires `failure` / `skipped` /
  `spawn_failed` → **rejection candidate: `ChildNotEligible`.**
- **BUT** Round 3's additional check fires FIRST: is
  `coord-outer.B` a batch parent? YES (its log contains a
  `SchedulerRan` event from Turn 5, and/or its current state declares
  `materialize_children`). Submission rejects with
  `ChildIsBatchParent`.

Per design Key Interfaces (line 3279) and Decision 9 Part 4 text
(line 1752-1763), the check for batch-parenthood takes precedence
over general retry-eligibility so the agent gets the most specific
guidance.

KOTO response:

```json
{
  "action":"invalid_retry_request",
  "reason":{
    "kind":"child_is_batch_parent",
    "children":["coord-outer.B"]
  },
  "error":"Cannot retry 'coord-outer.B' at this level: the child is itself a batch parent. Cross-level retry is unsupported in v1. Drive the inner coordinator instead, e.g. `koto next coord-outer.B --with-data '{\"retry_failed\":{\"children\":[\"coord-outer.B.B2\"]}}'`."
}
```

**[VERIFIED — rejection fires with actionable guidance]** The typed
`InvalidRetryReason::ChildIsBatchParent { children }` is machine-
readable (agents can key off `reason.kind`). The error message
includes the ready-to-run inner-level retry invocation, which
matches Decision 9's discoverability commitment (reserved_actions,
human-readable invocation strings).

**[AMBIGUITY A3 — error message content is normative?]** Design
Key Interfaces defines the `InvalidRetryReason` variant but doesn't
pin down the human-readable `error` message. The skill docs
(koto-user) could own the agent-facing phrasing. The transcript
above assumes koto emits a suggestion string. Flag if design
intends the message to be fixed wording vs. generated from the
variant.

Also verify the rejection is **atomic** (Decision 9 Part 4: "any
non-retryable child in the set rejects the whole submission"). If
agent mixes `coord-outer.B` with an eligible outer child (say C
after C's failure), submission rejects wholesale with
`ChildIsBatchParent { children: ["coord-outer.B"] }` — C is not
retried either. Matches design.

### Turn 11 — AGENT attempt 2: retry at inner level

```bash
koto next coord-outer.B --with-data '{"retry_failed":{"children":["coord-outer.B.B2"]}}'
```

Inner B's `handle_retry_failed`:
- `coord-outer.B.B2` exists on disk: yes.
- `coord-outer.B.B2`'s outcome: `failure`. Eligible.
- Is `coord-outer.B.B2` a batch parent? No (its template has no
  `materialize_children` hook). Not rejected as
  `ChildIsBatchParent`.
- Rewound per Decision 9 Part 2.

Response:
```json
{
  "action":"gate_blocked","state":"plan_and_await",
  "blocking_conditions":[{...
    "children":[
      {"name":"coord-outer.B.B1","state":"done","complete":true,"outcome":"success","role":"worker"},
      {"name":"coord-outer.B.B2","state":"working","complete":false,"outcome":"pending","role":"worker"}
    ]}]
}
```

B2 rewound to initial state, ready for re-drive.

### Turn 12 — Drive B2 to success; B terminates; outer ticks; C spawns

```bash
koto next coord-outer.B.B2 --with-data '{"status":"complete"}'
koto next coord-outer.B    # inner B finalizes: all_success → summarize
koto next coord-outer       # outer sees B terminal-success, spawns C
```

Outer's final view at terminal: `batch_final_view.tasks` carries A,
B, C as success; no recursive embedding of B's inner view (Decision
13 "per-level, not recursive"). `role` fields on each row. Matches
Pair C3 Turn 10 with role fields added.

---

## Section 2: Probe resolutions

### P1 — `role` field semantics

**Question:** when is `role == Coordinator` set?

Design Key Interfaces (line 2989-2995): "Present when this child's
current state carries a `materialize_children` hook of its own."

This matches option (b) from the prompt: **dynamic, based on current
state**. Not the template as a whole (static), not "has ever
materialized" (historical).

**Consequence:** a child whose template has `materialize_children`
on state X but not on state Y reports `role: coordinator` only
while it's at state X. If the template routes through a non-hook
state (e.g., `analyze_failures` has no hook), `role` flips to... ?
Options: (1) absent/None, (2) still coordinator because it HAS
materialized, (3) new variant `FormerCoordinator`. Design says
"Present when... current state carries hook." Strict reading
implies `role` is None during non-hook states like
`analyze_failures`.

**[AMBIGUITY A4 — role during analyze_failures]** An intermediate
coordinator at `analyze_failures` has a non-hook current state. Per
strict reading, `role` becomes None and `subbatch_status`
disappears. But the inner failures are exactly what the outer agent
needs to see! Flipping the field off mid-failure is worst-case
timing.

**Proposed fix (design clarification needed):** `role` should be
"sticky coordinator": once a child has appended a `SchedulerRan`
event to its log (evidencing it IS a batch parent), `role` stays
`Coordinator` for the remainder of its lifecycle, regardless of
current state. This also matches `subbatch_status` staying
populated. The design prose ("current state carries the hook")
appears to under-specify this. Flag for Round 3's design author to
tighten.

### P2 — `subbatch_status` timing

**Question:** when does `subbatch_status` become populated?

Per A1 above, two plausible semantics. Assuming the sticky-
coordinator reading from A4:

- Populated from the moment `role == Coordinator` is first set
  (spawn time, if spawn lands at a hook state).
- Counts start at zero (no inner children yet).
- Increments as inner children materialize, reach terminal, etc.
- Frozen at inner `BatchFinalized` (matches
  `batch_final_view.summary` semantics from Decision 13).

This simulation's Turn 2 shows `subbatch_status` OMITTED at B's
spawn (before B submits inner tasks). That's the `skip_serializing_if
= None` behavior — but the underlying `Option` should probably be
`Some(BatchSummary{zeros})` rather than `None`, for consistency.
Design prose is compatible with either interpretation.

### P3 — Outer visibility of inner `reserved_actions`

Per Decision 13 "per-level, non-recursive," inner's `reserved_actions`
(the retry invocation block) is NOT surfaced at outer level. Outer
sees only `subbatch_status` counts (total/success/failed/…).

**Verified:** to retrieve the retry invocation string, AGENT must
call `koto next coord-outer.B` or `koto status coord-outer.B`.
Consistent with Decision 13's per-level scoping.

This means the flow is:
1. AGENT watching outer sees `subbatch_status.failed > 0` on B's
   row.
2. AGENT calls `koto status coord-outer.B` (or `koto next
   coord-outer.B`).
3. Inner response carries `reserved_actions` with the ready-to-run
   retry.
4. AGENT submits at inner.

This is ONE extra command versus having the outer response carry
inner `reserved_actions`, but it keeps outer's response schema
fixed-width and doesn't leak the inner lock into the outer query.
Acceptable tradeoff, well-aligned with E1.

### P4 — Two-hat coordinator prose in Decision 12 Q8

Read lines 2303-2322. Decision 12 Q8 explicitly documents the
two-hat case:

> "A child that runs its own sub-batch is simultaneously a worker
> to its parent and a coordinator to its sub-batch. The rule
> composes: each level's `koto next <name>` is driven by exactly
> one caller at a time..."

And: "Agents detect the two-hat case via the
`MaterializedChild.role` field: when `role == Some(Coordinator)`,
the per-child `subbatch_status` summary is non-null and the child
should be driven with inner-coordinator responsibilities on the
inner parent's lock."

**[VERIFIED]** Round 3 closed Round 2 C3 F6. The prose is present,
references the correct fields, and names the "coordinator of
coordinators" skill section. Language is clear.

Minor nit: Q8 says `subbatch_status` is "non-null" when
`role == Coordinator`. Combined with the `skip_serializing_if =
Option::is_none` attribute, this means "if role is Coordinator,
never serialize subbatch_status as None." That's a semantic
constraint that should be enforced by the producer (koto engine)
and relied on by consumers (agents). Worth making it a runtime
invariant with a test.

### P5 — Nested reference template in `walkthrough.md`

`walkthrough.md` currently contains a flat batch reference (single
level). It mentions nested batches in the cross-level retry bullet
(line 1268) and the two-hat bullet (line 1273). But there's no
end-to-end nested example showing template YAML + driver transcript.

**[GAP G1 — walkthrough reference is flat-only]** Agents encountering
a two-level hierarchy have design-doc prose + this simulation to
guide them, but no copy-pasteable reference template for the inner
coordinator (`impl-issue-with-subbatch.md`). Round 3 added the
ergonomic surfaces (`role`, `subbatch_status`, rejection) but did
not add a nested reference example. Recommend a follow-up to
extend `walkthrough.md` with a small nested section showing:

- Two templates (outer `coord.md`, inner `impl-issue-with-subbatch.md`).
- Sample `tasks-outer.json` and `tasks-B.json`.
- Abbreviated driver transcript (≤ 10 turns).
- The retry-at-inner pattern.

Severity: documentation-only, nice-to-have.

### P6 — Cross-level `skipped_because_chain`

Per Decision 13 and E1, `skipped_because_chain` is intra-batch
(within a single parent's `waits_on` graph). If outer A fails and B
is skipped at outer level, B's skip record: `skipped_because:
"coord-outer.A"`, `skipped_because_chain: ["coord-outer.A"]`. Chain
does NOT reach into B's inner hypothetical children (which don't
exist anyway because B is respawned as a skip marker directly and
never reaches its `materialize_children` hook — Pair C3 F3).

**[VERIFIED]** No change in Round 3. Chain stays per-level. Matches
design.

---

## Section 3: Edge-case probes

### E1 — Outer retry on a non-batch-parent outer task (normal retry)

Scenario: C fails at outer level (non-batch child). AGENT retries:

```bash
koto next coord-outer --with-data '{"retry_failed":{"children":["coord-outer.C"]}}'
```

`handle_retry_failed`: C exists on disk, outcome `failure`. Batch-
parent check: does `coord-outer.C`'s log contain `SchedulerRan` OR
does its current state declare `materialize_children`? C's
template is `impl-issue.md` (a leaf worker template, no hook) → **no**
→ `ChildIsBatchParent` does NOT fire. Retry proceeds per Decision 9
Part 2 (rewind, downward closure within outer's task graph).

**[VERIFIED]** `ChildIsBatchParent` does not false-trigger on leaf
workers. The detection predicate is specific (batch-parent-ness is
a property of the child's template/log, not of the parent's
submission).

### E2 — Retry on B before B has submitted inner tasks

Scenario: B spawned (Turn 2) but has NOT yet submitted its inner
tasks. At this point B's log contains `WorkflowInitialized`,
`Transitioned → plan_and_await`. No `SchedulerRan`, no inner
children, no `BatchFinalized`.

AGENT tries `koto next coord-outer --with-data
'{"retry_failed":{"children":["coord-outer.B"]}}'`. What does
batch-parent detection say?

Per Key Interfaces line 2989: role is "Present when this child's
current state carries a `materialize_children` hook." B's current
state (`plan_and_await`) DOES carry the hook. So B is flagged
`role: coordinator` even pre-submission. The batch-parent check
therefore fires **based on current state declaring the hook**, not
on historical `SchedulerRan`. Rejection: `ChildIsBatchParent`.

But note: B is also non-terminal (`pending`). Even without the
batch-parent check, R10 would reject it as `ChildNotEligible`. Which
rejection wins?

**[AMBIGUITY A5 — rejection precedence]** Design Key Interfaces
lists `InvalidRetryReason` variants but doesn't specify order of
checks. Proposed ordering:

1. `UnknownChildren` (name doesn't resolve on disk)
2. `ChildIsBatchParent` (structural, takes precedence)
3. `ChildNotEligible` (general outcome check)
4. `MixedWithOtherEvidence` / `RetryAlreadyInProgress` / …

The reason: `ChildIsBatchParent` is more informative (it tells the
agent WHERE to retry), whereas `ChildNotEligible` just says "this
child isn't retryable at this outcome." Design should pin this
ordering.

Impact on the prompt's scenario: if B is still `pending` and the
agent tries outer retry, the agent gets `ChildIsBatchParent` — the
most actionable signal. Good.

### E3 — Inner retry is itself at an intermediate state when outer retry is attempted

Scenario: inner B's retry has been submitted but has not yet been
ticked (B's log has `EvidenceSubmitted{retry_failed: ...}` but no
subsequent `SchedulerRan` / `Rewound`). The `RetryAlreadyInProgress`
guard (Decision 9 Part 4, line 1735) should prevent a double retry
at the inner level. But what if AGENT submits OUTER retry during
this window?

Outer's `handle_retry_failed` checks B (not B2). B is still a
batch parent. Outer retry rejects with `ChildIsBatchParent`. Inner
B's retry is unaffected.

**[VERIFIED]** Inner and outer retry machineries are fully
independent. `RetryAlreadyInProgress` is per-workflow; it doesn't
cross-level.

---

## Section 4: Findings

### F1 — Verified: `ChildIsBatchParent` rejection fires correctly

- **Observation:** Outer-level retry targeting B (nested batch
  parent) is rejected with
  `InvalidRetryReason::ChildIsBatchParent { children: ["coord-outer.B"] }`.
  Machine-readable variant. Actionable error message with the
  inner-level retry invocation suggestion.
- **Verifies round-2 blocker (C3 F4)?:** Yes. Round 3 close-out.
- **Severity:** positive verification.
- **Proposed resolution:** no change; see F4 below for error-
  message pinning.

### F2 — Verified: `role` field discriminates workers from coordinators

- **Observation:** `MaterializedChild.role` is `Worker` for leaf
  children (A, C) and `Coordinator` for intermediate children whose
  current state carries `materialize_children` (B). Field is
  elided (skip_serializing_if) on non-coordinator rows rather than
  serialized as `"worker"`.
- **Verifies round-2 gap (C3 F9)?:** Yes. Round 3 close-out.
- **Severity:** positive verification.
- **Proposed resolution:** no change. See F3 for `role` stickiness
  clarification.

### F3 — Ambiguity: `role` semantics during non-hook intermediate states

- **Observation:** Design says `role: Coordinator` is "Present when
  this child's current state carries a `materialize_children` hook."
  For an intermediate child at `analyze_failures` (a non-hook state
  in the reference template), strict reading says `role` reverts to
  `None` or `Worker`. That's exactly the worst timing for outer
  observers who need to see inner failure aggregates.
- **Verifies round-2 claim?:** Not previously raised; arises from
  the P1 probe in this round.
- **Severity:** should-fix. Documentation clarification + probable
  behavior change (sticky coordinator).
- **Proposed resolution:** amend the `role` field doc comment to:
  "Present as `Coordinator` once the child's log contains at least
  one `SchedulerRan` event OR its current state carries a
  `materialize_children` hook; stays `Coordinator` for the
  remainder of the child's lifecycle." This guarantees outer
  observers keep seeing inner detail even when the inner coordinator
  routes through a non-hook state during failure handling. Ensures
  `subbatch_status` also remains available throughout.

### F4 — Ambiguity: error-message pinning for `ChildIsBatchParent`

- **Observation:** Design defines the typed variant but not the
  human-readable message. The transcript above assumes koto emits
  an error string including the inner retry invocation. If the
  message is generated by koto (not the skill), multiple callers
  get the same guidance. If it's owned by the skill, the machine-
  readable variant suffices and the skill renders prose.
- **Verifies round-2 claim?:** No — new in Round 3.
- **Severity:** nice-to-have. Deterministic surface either way, but
  agents relying on the error text benefit from a pinned message.
- **Proposed resolution:** pick a contract and document. Either:
  (a) koto emits a canonical message template (parameterized by
      `children`) so all callers get identical guidance. OR
  (b) only the typed variant is canonical; skills render prose and
      agents keying off `error` string are on their own.
  Recommend (a) for agent-facing ergonomics.

### F5 — Ambiguity: rejection precedence order

- **Observation:** When a child is both a batch parent AND
  ineligible by outcome (e.g., still pending), which rejection
  fires? `ChildIsBatchParent` is more actionable
  (redirects the agent); `ChildNotEligible` is more literal.
- **Verifies round-2 claim?:** No — new in Round 3.
- **Severity:** should-fix. Determinism of typed rejections
  requires the order be pinned so agent retry logic is predictable.
- **Proposed resolution:** document precedence:
  `UnknownChildren` → `ChildIsBatchParent` → `ChildNotEligible` →
  `MixedWithOtherEvidence` → `RetryAlreadyInProgress`. Put the
  ordering in Key Interfaces near the `InvalidRetryReason` enum or
  in Decision 9 Part 4.

### F6 — Ambiguity: `subbatch_status` populate trigger

- **Observation:** Design says `subbatch_status` is populated when
  `role == Coordinator`. Unclear whether that means (a) always,
  from spawn onward (zero counts before first materialization), or
  (b) only once inner children exist on disk. Either produces a
  valid, but distinct, shape for the outer response.
- **Verifies round-2 claim?:** No — new in Round 3.
- **Severity:** nice-to-have. Agents code defensively against
  either shape if unspecified.
- **Proposed resolution:** pin option (a) — "`subbatch_status` is
  `Some` whenever `role == Coordinator`; counters reflect current
  disk state (zero if no inner children yet)." This pairs naturally
  with F3's sticky-coordinator proposal.

### F7 — Nice-to-have: `subbatch_needs_attention` derived boolean

- **Observation:** `subbatch_status` carries raw counts. Agents
  must compute `failed + skipped + spawn_failed > 0` to derive
  "inner needs attention." Top-level `needs_attention` on the gate
  covers outer, not inner.
- **Verifies round-2 claim?:** Partial — Round 2 C3 F5 flagged
  outer opacity; Round 3 closed it with `subbatch_status` counts,
  but the derived boolean is not emitted.
- **Severity:** nice-to-have. Additive ergonomic field.
- **Proposed resolution:** consider adding
  `subbatch_needs_attention: bool` alongside `subbatch_status`, or
  extend `BatchSummary` itself with the derived boolean (so it
  appears both here and in `batch_final_view.summary`). Not a
  blocker — the signal is already derivable.

### F8 — Documentation gap: nested reference in `walkthrough.md`

- **Observation:** `walkthrough.md` mentions nested batches in two
  bullets but contains no end-to-end nested example. Round 3
  closed the ergonomic gaps; the documentation reference for
  agents/authors has not caught up.
- **Verifies round-2 claim?:** Part of Pair C3's P5 probe,
  carried forward.
- **Severity:** nice-to-have. Documentation-only.
- **Proposed resolution:** add a short nested-batch section to
  `walkthrough.md` with two templates, two task-list JSONs, and an
  abbreviated driver transcript exercising the `role` /
  `subbatch_status` / `ChildIsBatchParent` surfaces.

---

## Summary note

Round 3's new surfaces (`ChildIsBatchParent` rejection,
`MaterializedChild.role`, `MaterializedChild.subbatch_status`) close
the three blockers flagged in Round 2 Pair C3: F4 (cosmetic outer
retry on nested batch parent), F5 (outer opacity to inner
failures), F9 (nested-batch-ness invisibility at outer level). Each
closure is verified in the transcript.

Six new findings surface from Round 3, none catastrophic:

- F1, F2 — positive verifications.
- F3 — `role` stickiness through non-hook intermediate states needs
  design clarification (should-fix).
- F4 — error-message pinning for `ChildIsBatchParent` (nice-to-have).
- F5 — rejection precedence ordering must be pinned (should-fix).
- F6 — `subbatch_status` populate-trigger ambiguity (nice-to-have).
- F7 — optional `subbatch_needs_attention` derived boolean
  (nice-to-have).
- F8 — nested reference missing from `walkthrough.md`
  (documentation).

The structural composition is sound. Two should-fix items (F3, F5)
are cheap documentation / invariant clarifications. Everything else
is additive or editorial. No changes to the engine model or the
typed surface are required to resolve any finding.
