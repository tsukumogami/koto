# Phase 6 Architecture Review — Round 1 (DESIGN-batch-child-spawning.md)

**Scope.** Review of the 14-decision revised design after round-1 amendments
(CD9–CD14) were folded in. Focus: does the doc hang together as one coherent
architecture, and is it implementable as specified?

**Verdict.** **Needs surgical fixes.** The round-1 decisions themselves
compose cleanly (cross-validation was right about that), and every new type
in the Key Interfaces block has a well-specified variant set. But the fold-in
pass did not propagate CD9, CD12, and the CD12 rename to several stale
prose sections that predate round 1. These sections now actively contradict
the round-1 decisions, not just lag them. The errors are all localized
(section-level prose rewrites, a handful of renames, a few missing serde
attributes) — no structural rework needed.

---

## High-severity findings

### H1. Data Flow "Retry" section contradicts Decision 5.4, 9.2, and 12.Q6

`docs/designs/DESIGN-batch-child-spawning.md:3259–3280` (Data Flow, "Retry:").
This prose survives from round 0 and now contradicts the canonical retry
sequence laid down in Decision 5.4 (lines 1162–1177) and the CD12 Q6
reordering.

Specific problems:
- Step 3 says "the advance loop sees the `retry_failed` evidence and
  transitions the parent … via a template-defined route." Then step 4
  says `handle_retry_failed` runs after the advance loop. CD9.2 specifies
  the opposite: `handle_retry_failed` intercepts BEFORE
  `advance_until_stop`; the advance loop fires the template transition on
  the next tick. Decision 5.4 at line 1221 ("Interception point. `retry_failed`
  is intercepted in `handle_next` BEFORE `advance_until_stop` runs") is
  correct; line 3265 is wrong.
- Step 4 orders "child Rewound events first, parent clearing event
  second." CD12 Q6 inverted this: the clearing event must be appended
  (and pushed) BEFORE any child writes to eliminate phantom child epochs.
  Decision 5.4's lettered sequence at lines 1162–1177 is correct;
  line 3272 (child-first) is the round-0 order.
- Step 4 references `internal_rewind_to_initial` for all children, but
  CD9.5 splits skipped children into a delete-and-respawn path (their
  current state carries `skipped_marker: true`; a Rewound event does
  not reach the correct state). The skipped-child path is missing here.

**Fix.** Replace the 22-line "Retry:" block with a 10-line pointer to
Decision 5.4's canonical sequence, or mirror its a/b/c/d/e letters
verbatim.

### H2. Concurrency Model section contradicts Decision 12

`docs/designs/DESIGN-batch-child-spawning.md:3282–3361`. This entire
section is untouched round-0 prose. Its core thesis — "only one `koto
next parent` call may run at a time; the invariant is caller-enforced,
koto does not serialize at the backend layer" — is the exact position
CD12 overturned.

Specific contradictions:
- Line 3317: "The invariant consumers must enforce: only one `koto next
  parent` call may run at a time." CD12 Q3 installs an advisory flock
  at the koto layer. The invariant is now koto-enforced for batch
  parents; consumers see a typed `concurrent_tick` error on contention.
- Lines 3310–3315: "Unix `rename(2)` has no 'fail if exists' semantics,
  so the second rename silently overwrites the first. If the child
  received events between the two ticks, those events are lost."
  CD12 Q2 specifically closes this window with
  `renameat2(RENAME_NOREPLACE)` on Linux and POSIX `link()`+`unlink()`
  fallback. The stated "silently overwrite" outcome no longer applies.
- Lines 3333–3343: "Why this design doesn't serialize at the koto
  layer" — an entire paragraph arguing against the choice CD12 made.
  Completely stale.

This section is load-bearing for reviewers: anyone reading it will walk
away with the wrong mental model of what the implementation guarantees.

**Fix.** Rewrite as "Concurrency model under Decision 12" summarizing
the flock (Q3) + `RENAME_NOREPLACE` (Q2) + `materialized_children`
ledger (Q1) + push-parent-first retry (Q6) + tempfile sweep (Q7)
story. The "Concrete worked example" at lines 3345–3361 can survive
with minor edits; it's factually still true that coordinators drive
parents and workers drive children, but the rationale block above
must be replaced.

### H3. Data Flow Step 4 still describes synthetic-parent-template skip synthesis

`docs/designs/DESIGN-batch-child-spawning.md:3232–3236`:

> For each `ShouldBeSkipped` task, synthesizes a skipped child:
> calls `init_state_file` with a header pointing at the parent
> template and an initial-events list containing
> `WorkflowInitialized` plus `Transitioned → <skipped_marker_state>`
> plus a context write (`skipped_because: <failed_task>`).

This is the Round 0 synthetic-template mechanism that CD9.5 explicitly
supersedes. Under CD9, skip markers live on the **child's real
template** at a state where `skipped_marker: true`. "Header pointing
at the parent template" is exactly the mechanism CD9 rejected because
cross-template transitions are not an engine feature.

Decision 5.2 at lines 1002–1059 gets this right; the Data Flow prose
at 3232 is stale.

**Fix.** Replace with: "For each `ShouldBeSkipped` task, calls
`init_state_file` with the child's real template and initial events
`[WorkflowInitialized, Transitioned → <skipped_marker_state>]` plus
a context write `skipped_because: <failed_task>`. The scheduler uses
F5 to ensure every batch-eligible child template declares a
reachable `skipped_marker: true` state. (Decision 9)"

### H4. Data Flow Step 4 and the Step-4 recap still say "R1–R7"

`docs/designs/DESIGN-batch-child-spawning.md:3091` and `3213`:

> builds DAG, runs runtime validation (R1–R7)

Decision 14 explicitly rewrote Data Flow Step 4's validation list to
"R3/R4/R5/R6/R8/R9" as the whole-submission failures (R1/R2 became
per-task; R0 is non-empty check; R7 is kernel-level atomic init; R8
and R9 are new). The two "R1–R7" references are stale.

**Fix.** Replace with "runs whole-submission validation (R3, R4, R5,
R6, R8, R9); R0 already ran pre-append; R1/R2 run per-task and
accumulate in `SchedulerOutcome.errored`."

---

## Medium-severity findings

### M1. `scheduler.spawned` → `spawned_this_tick` rename incomplete

CD12 Q1 renamed `scheduler.spawned` to `spawned_this_tick`. Six call
sites in the prose still use the old name:

- Line 1414 ("`scheduler.spawned`" in Decision 7 context)
- Line 1433 (directive example)
- Line 1446 (structured-data description)
- Line 1467 (example response "directive" string)
- Line 1485 (example response JSON: `"spawned": ["parent-42.issue-4"]`)
- Line 3098 (`SchedulerOutcome::Scheduled { spawned: ... }`)
- Line 3535 (Implementation Approach: "per-tick `spawned`, `already`...")

Line 1485 is the worst because it's a literal JSON example agents will
copy. Line 3535 describes the `SchedulerRan` event payload fields —
those field names need the rename too.

**Fix.** Find-and-replace `spawned` → `spawned_this_tick` in these six
places. Preserve intentional uses on lines 684, 801, 1797, 1800, 1808,
1810, etc., where "spawned" is a past-tense adjective, not the field
name.

### M2. W1–W3 in the Components diagram, should be W1–W5

`docs/designs/DESIGN-batch-child-spawning.md:2643`:

> compile.rs: new validator for materialize_children
>   enforcing E1-E10 errors and W1-W3 warnings

CD9 added W4, CD13 added W5. The diagram line was not updated.

**Fix.** Change to "E1-E10 errors and W1-W5 warnings (plus F5)".

### M3. "Eight decisions" language in Decision Outcome

`docs/designs/DESIGN-batch-child-spawning.md:2517, 2527, 3667`. "The
eight decisions interlock into one coherent implementation" — now
fourteen.

**Fix.** Update to "fourteen decisions" and (at line 2527) "consistent
with the eight exploration-time constraints" — actually the original
said five, and they are still the exploration-phase decisions, so
that number is fine. Just fix the two "eight decisions" and line
3667 ("Phase 2 bundles eight decisions' worth of schema changes" —
should be "Phase 2 bundles six decisions' worth" given Decisions 1,
3, 4, 5 plus CD9 portions plus CD10–CD13 schema additions).

### M4. `action: "done"` vs `workflow_complete` inconsistency

`docs/designs/DESIGN-batch-child-spawning.md:2305` uses `"action":
"done"` in the synthetic-child response example. Lines 3118 and 3162
use the older name `workflow_complete`. Decision 13's example (2305)
is the authoritative post-round-1 shape.

**Fix.** If `done` is the canonical action name, replace
`workflow_complete` at lines 3118 and 3162. If `workflow_complete` is
the real name, Decision 13's example is wrong. Confirm against
`src/cli/next_types.rs` before picking a side.

### M5. New interface types lack serde derive attributes

In the Key Interfaces block (lines 2723–2930), ~10 new public types
(`SchedulerOutcome`, `MaterializedChild`, `SchedulerFeedback`,
`OrphanCandidate`, `TaskSpawnError`, `BatchError`, `MutatedField`,
`ChildEligibility`, `BatchView`, `BatchSummary`, `BatchTaskView`) are
declared without `#[derive(Debug, Clone, Serialize, Deserialize,
PartialEq)]`. By contrast the template types at lines 2663–2697
carry full derives. Most of these structs serialize to JSON and
travel through the CLI response; missing derives are not merely
cosmetic — they are part of the protocol contract.

Specific gaps:
- `SchedulerOutcome` — serializes as the `scheduler` top-level field.
- `MaterializedChild`, `BatchView`, `BatchSummary`, `BatchTaskView` —
  serialize through `koto status` and `batch_final_view`.
- `BatchError` and its variant enums — serialize into `error.batch`
  (Decision 11 envelope).

**Fix.** Add a one-line note above the block: "All types carry
`#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]` unless
marked otherwise. Enums use `#[serde(tag=..., rename_all='snake_case')]`
as shown per-variant." Or, less preferred, decorate every declaration
individually.

### M6. `EntryOutcome` serde tag may conflict with struct-field wrapping

`EntryOutcome` is declared with `#[serde(tag = "outcome",
rename_all = "snake_case")]`. It lives inside
`SchedulerFeedback.entries: BTreeMap<String, EntryOutcome>`. The
internal tag "outcome" on an enum value wrapped by a map entry
produces:

```json
"entries": {
  "task-a": {"outcome": "accepted"},
  "task-b": {"outcome": "blocked", "waits_on": ["task-a"]}
}
```

which is reasonable. But `SchedulerOutcome::Scheduled` variant also
has a field named `skipped: Vec<(String, String)>` using a tuple,
which serializes as JSON arrays of 2-element arrays, unusual for
agent-readable shapes. And `MaterializedChild.outcome: String` uses
a free `String`, not an enum — Decision 11's typed-enum-discriminators
commitment is breached here. The six legal values are enumerated in
the doc-comment but not typed.

**Fix.** Change `MaterializedChild.outcome` and `BatchTaskView.outcome`
to a new enum (e.g., `TaskOutcome` with variants `Success`, `Failure`,
`Skipped`, `Pending`, `Blocked`, `SpawnFailed`) carrying
`#[serde(rename_all = "snake_case")]`. Decision 11's "typed enum
discriminators throughout" commitment extends naturally here. The
tuple in `skipped: Vec<(String, String)>` should be named:
`skipped: Vec<SkippedEntry>` with a two-field struct.

### M7. Decision Outcome still lists "three call sites" with "skipped-marker synthesis"

`docs/designs/DESIGN-batch-child-spawning.md:2540–2541` and `2571`:

> Three call sites will consume it later (regular init, scheduler
> spawn, skipped-marker synthesis).

Post-CD9, the third call site is no longer "synthesis" (which would
use a synthetic template). It is "runtime reclassification — delete
the child and re-spawn into the real template at a `skipped_marker`
state." Either framing is fine, but "skipped-marker synthesis" is
loaded terminology that carries the old mechanism's name.

**Fix.** Replace "skipped-marker synthesis" with "runtime
reclassification (delete-and-respawn of skipped children per Decision
9)".

---

## Low-severity findings

### L1. "Synthetic skipped state files" header language in Decision 5

Line 983: `#### Chosen: first-class failure + synthetic skipped state
files + extended gate + `retry_failed` evidence`. The body was
rewritten to runtime reclassification; the header still says
"synthetic." Minor, but search-visible.

**Fix.** "first-class failure + skip-marker state files + extended
gate + `retry_failed` evidence".

### L2. "Synthetic skipped children are shape-indistinguishable" framing at 2246

Line 2246 survives as a gap description; the gap is real, but the
word "synthetic" there means "scheduler-authored," which matches
Decision 13's final `synthetic: true` marker. Keep as-is; mentioning
for completeness.

### L3. Positive-Consequences bullet still omits CD14 path-resolution warnings as user-visible

Lines 3648–3662 (the "Round-1 findings now closed" bullet) lists the
new envelope, concurrency hardening, and observability wins but does
not call out the two `SchedulerWarning` variants as user-visible
diagnostics. Minor polish.

---

## Architectural-fit findings

### A1. `SchedulerOutcome.Scheduled` has grown large

The `Scheduled` variant now carries nine fields (`spawned_this_tick`,
`materialized_children`, `already`, `blocked`, `skipped`, `errored`,
`warnings`, `feedback`, implicit scheduler-ran context). The
`materialized_children` ledger + `spawned_this_tick` observation pair
is one concept (CD12 Q1); `already` and `blocked` are arguably
redundant now that `materialized_children` carries per-child state.

Consider collapsing `already`/`blocked`/`skipped` derivable views
against `materialized_children` into doc-only "derived views" rather
than independent serialized fields. This is not blocking — the doc
is internally consistent if they all ship — but it is a simpler
architecture.

**Advisory.** Not blocking. If a round-2 walkthrough asks "why do we
emit both `materialized_children` and `already`?" the answer should
be ready. (`spawned_this_tick` ∪ `already` = the already-spawned
children; `materialized_children` is a strictly richer view. The
only reason to keep `already` is backward-compat with pre-rename
consumers, which don't exist yet.)

### A2. `reserved_actions` is a sibling of `expects.fields`, not part of it

Explicit and correct per Decision 9 Part 3 (lines 1213–1219 and
2835–2847). No issue, but worth calling out in the skill update
(koto-user) that reserved actions are synthesized post-gate-eval
and do not flow through the accepts validator.

### A3. `sync_status` / `machine_id` — conditional-emission pattern is novel

Decision 12 Q5 emits these only when `CloudBackend` is configured. The
rest of the response schema uses optional fields (omitted when absent).
These two fields use feature-detection emission. The distinction is
real — agents can discriminate "local mode" from "cloud mode" by field
presence — but it's a new response-shape idiom.

**Advisory.** The idiom is fine; just document it in the koto-user
skill so consumers don't treat absence as "unknown sync state" (it
means "local mode, sync state not applicable").

### A4. Round-2 collapse candidates

The user asked whether any two follow-up decisions could collapse into
one surface. Candidates:

- **CD9 `reserved_actions` and CD13 `batch_final_view`** both emit on
  terminal-with-failures responses. They are structurally independent
  (one is a discovery surface for retry, the other is the final
  snapshot), but they share the triggering predicate ("`any_failed`
  or `any_skipped` at any point during batch"). If a round-2
  walkthrough surfaces "agents struggle to know which to read first,"
  a unified post-batch diagnostic block (`batch_final_view` carries
  both the view AND the reserved actions) is a cleaner surface.
  **Not recommended for round 1 — two discrete fields is clearer for
  consumers building rendering code.**

- **CD10 `scheduler.feedback.entries` and CD11
  `SchedulerOutcome.errored`** both carry per-task outcomes. Today,
  `feedback.entries[task].outcome == "errored"` AND `errored[].task ==
  task` will both be true for failed tasks. Two writes, two reads,
  same data. Merging into one source would remove the redundancy.
  **Mild recommendation: consider making `errored` a derived view
  of `feedback.entries` filtered to `Errored` variants, OR removing
  the `Errored` variant from `EntryOutcome` and referring agents to
  `errored` instead.** Either direction eliminates the dual-source.

- **CD12 Q1 `spawned_this_tick` and `materialized_children`** —
  covered in A1; marginally collapsible.

No collapse is blocking. CD9/CD13 serve distinct consumers; CD10/CD11
is cleanup worth a small issue after v1 ships.

---

## Phase sequencing

Phase 1 (atomic init + locks), Phase 2 (schema layer), Phase 3
(scheduler + observability). Reviewed for cross-phase dependencies:

- **Pull-forward: `spawn_entry` snapshot on `WorkflowInitialized`.**
  Currently listed in Phase 1 (line 3409–3413). This is correct —
  Phase 3's R8 validation relies on the snapshot being present on
  pre-existing children. Phase 1 must ship the field.
- **Pull-forward: flock for batch parents (CD12 Q3).** Currently in
  Phase 1. Correct. Without it, Phase 3's scheduler races.
- **Phase 2 prerequisite for Phase 3: `evidence.<field>: present`
  when-clause matcher (CD9 assumption a3).** Currently flagged as a
  Phase 3 prerequisite (line 3471–3474). This is fine — it's a
  small engine addition and Phase 3 owns the retry routing that
  consumes it.
- **Phase 2 → Phase 3 dependency: F5 warning.** F5 fires on child
  templates referenced as `default_template`. The child template must
  have been compiled. Phase 2 compiles templates; Phase 3 runs the
  scheduler. The warning itself can ship in Phase 2 (the compiler
  knows the `default_template` path), but the runtime check "can I
  respawn into a `skipped_marker` state?" is a Phase 3 concern.
  Confirm both halves are covered in Phase 2's deliverables.
- **Push-back risk: `BatchFinalized` event in Phase 3.** This event
  is written by the advance loop when `children-complete` first
  evaluates `all_complete: true`. The advance loop machinery lives in
  Phase 1/2 territory (`evaluate_children_complete` is modified in
  Phase 3 per line 3487). The `BatchFinalized` event write is Phase
  3's responsibility, consistent with the current plan. No issue.

**Phase sequencing is correct as written.** No work needs to move
phases.

---

## Summary of recommended edits

| # | Severity | Location | Edit |
|---|----------|----------|------|
| H1 | High | L3259–3280 (Data Flow Retry:) | Replace with pointer to Decision 5.4's a/b/c/d/e sequence |
| H2 | High | L3282–3343 (Concurrency model) | Rewrite to reflect CD12 Q1–Q7 (flock + RENAME_NOREPLACE at koto layer) |
| H3 | High | L3232–3236 (Data Flow step 4) | Replace "header pointing at the parent template" with child's real template + skipped_marker state |
| H4 | High | L3091, L3213 | Replace "R1–R7" with "R3/R4/R5/R6/R8/R9 (whole-submission); R1/R2 per-task" |
| M1 | Med | L1414, 1433, 1446, 1467, 1485, 3098, 3535 | Rename `spawned` → `spawned_this_tick` |
| M2 | Med | L2643 | "W1-W3" → "W1-W5 (plus F5)" |
| M3 | Med | L2517, 2527, 3667 | "eight decisions" → "fourteen decisions" |
| M4 | Med | L2305, 3118, 3162 | Reconcile `done` vs `workflow_complete` action name |
| M5 | Med | L2723–2930 | Add serde-derive preamble, or decorate each type |
| M6 | Med | L2751, L2992 | Type `MaterializedChild.outcome` and `BatchTaskView.outcome` as enums |
| M7 | Med | L2540, L2571 | "skipped-marker synthesis" → "runtime reclassification" |
| L1 | Low | L983 | Section header: drop "synthetic" |

---

## Internal-consistency summary

| Check | Result |
|-------|--------|
| D5 runtime reclassification defers to D9, no lingering synthetic-template refs | **Fail (H3, L1)** — Data Flow Step 4 at L3232 still describes synthetic-parent-template mechanism |
| D4 path resolution includes CD14's absent-template_source_dir fallback | **Pass** — L934–949 cleanly adds (b') skip and both warnings |
| D5.4 retry sequence matches CD12 Q6 (push parent first) | **Partial** — D5.4 prose (L1162–1177) is correct; Data Flow Retry (L3259–3280) is stale |
| `spawned` vs `spawned_this_tick` uniformly referenced | **Fail (M1)** — seven stale references |
| E1–E10, R0–R9, W1–W5 all present in Decision 1's tables | **Pass** — L642–669 covers them all (F5 also). Components diagram at L2643 is stale (M2). |

---

## Ready-for-review verdict

**"Needs surgical fixes."**

- The round-1 decisions themselves are sound and compose well.
- New Key Interface types (SchedulerOutcome.materialized_children,
  SchedulerFeedback, EntryOutcome, OrphanCandidate, SchedulerWarning,
  TaskSpawnError, SpawnErrorKind, InvalidBatchReason, InvalidRetryReason,
  MutatedField, ChildEligibility, LimitKind, InvalidNameDetail) have
  well-specified variants and payload shapes. The only gaps are
  missing derive attributes (M5) and free-string fields that should
  be typed (M6).
- Phase sequencing is correct; no work needs to move phases.
- But four high-severity internal contradictions (H1–H4) mean a
  careful reader following the Data Flow or Concurrency sections in
  order will build the **wrong** mental model. Fix those before
  calling the doc ready for implementation planning.

Estimated effort: half-day of careful prose surgery. No decisions need
to be revisited.
