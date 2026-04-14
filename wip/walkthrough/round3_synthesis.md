# Round 3 synthesis: 9 pairs verifying round-2 fixes

## Purpose

Round 2 (9 pairs against the post-round-1 design) surfaced three
blockers + five should-fix clusters. Round-3 revision pass folded all
of them into the design. Round-3 verification (this cycle) ran 9 fresh
pairs to confirm the fixes land cleanly and flush out any new gaps the
fixes may have introduced.

## Pairs

| Pair | Focus | Targets |
|------|-------|---------|
| A1 | `ready_to_drive` dispatch gate | Round-2 F1 (reclassification race) |
| A2 | `spawn_entry` through retry cascades | Round-2 F2 (spawn_entry lifecycle) |
| A3 | Typed envelope polish | Cluster 1 fixes |
| B1 | `spawn_failed` lifecycle to terminal | Round-2 B2 blocker |
| B2 | Retry-respawn + typed envelope | Cluster 1 + retry edges |
| B3 | `BatchFinalized` invalidation on retry | Cluster 3 (CD13 vocabulary) |
| C1 | Nested-batch retry rejection | Round-2 C3 blocker |
| C2 | Path resolution polish | Cluster 4 (CD14 polish) |
| C3 | End-to-end happy path sanity check | Over-engineering probe |

Transcripts: `simulation_round3_pair_{a1,a2,a3,b1,b2,b3,c1,c2,c3}.md`.

## Round-2 blockers: verification

| Round-2 blocker | Round-3 fix | Verdict |
|-----------------|-------------|---------|
| Reclassification race (A1-F1) | `ready_to_drive: bool` gate on materialized_children; respawn `spawn_entry` = current submission entry; `EntryOutcome::Respawning` for transient | **Closed** (verified A1, A2) |
| spawn_entry lifecycle (C2-F2) | Respawn carries current submission entry; multi-tick cascade well-defined | **Closed** (verified A2) |
| spawn_failed gate integration (B2) | Tightened `all_complete = ... AND spawn_failed == 0`; added `any_spawn_failed`; loosened R10 for retry-respawn | **Partially closed — gate works, but exposes new architectural gap (see below)** |
| Outer retry on nested-batch child (C3-F4) | `InvalidRetryReason::ChildIsBatchParent` | **Closed** (verified C1) |

Three of four round-2 blockers fully closed. The fourth revealed a
new gap below.

## New blocker surfaced

### B1 (architectural): spawn_failed templates cannot be repaired via task-list correction

**Surfaced by:** Pair B1 (explicit workflow analysis)

The round-3 spawn_failed gate fix stops silent-success (the old bug).
But it doesn't close the complementary workflow: when a task
persistently fails to spawn because its `template` path is wrong,
the agent has no v1 path to correct the template and retry.

The trap:
- `tasks`-typed evidence is only accepted at `plan_and_await`.
- `any_spawn_failed: true` triggers `needs_attention`, routing
  parent to `analyze_failures` on each tick.
- At `analyze_failures`, `tasks` is NOT an accepted field.
- `retry_failed` on a spawn_failed child runs retry-respawn using
  the CURRENT submission's entry (which still points at the broken
  template).
- Mixed payloads `{"retry_failed": ..., "tasks": ...}` are
  rejected with `MixedWithOtherEvidence`.
- The only v1 exits are: retry-in-a-loop (never converges because
  the template is still broken) or `decision: give_up` (abandons
  those children permanently in `batch_final_view`).

**Round-2 note:** Pair B2 flagged this as an undefined lifecycle
issue. Round 3 tightened the gate (which was the right fix for
that specific bug), but the tightening made this workflow hole
structurally visible.

**Proposed resolutions** (pick one for a round-4 decision):

1. **Allow `tasks` at `analyze_failures` for un-spawned entries.**
   Accept a narrow `tasks` submission at `analyze_failures` whose
   entries match only spawn_failed children (no new names, no
   mutations to spawned children). Scheduler then re-runs
   `init_state_file` on the corrected entries.
2. **Add an `update_tasks` reserved evidence action.**
   Parallel to `retry_failed` and `cancel_tasks` (deferred). Agent
   submits `{"update_tasks": {"entries": [{"name": "D", "template":
   "fixed.md"}]}}`. Scheduler updates the stored submission and
   re-runs spawn attempts on next tick.
3. **Document `give_up` as the only v1 exit.** Explicitly constrain
   v1 scope. Users with persistent template errors must start a
   fresh batch. Clean spec but frustrating for common user errors.

Option 2 is cleanest (new primitive parallel to existing retry
apparatus, minimal template-author burden). Option 1 is smaller
(reuses existing accepts surface). Option 3 is smallest-scope but
frustrates the canonical "I typed the wrong template path" case.

## Should-fix clusters

### Cluster 1: Retry-path observability (A1-F5, A2-A, A2-B, B2-F1, B2-F2)

- **A1-F5:** Retry step (d) is a loop; commit semantics on partial
  failure (B rewound, but D respawn fails) are unspecified. Pin to
  "per-child accumulation, never halt" like Decision 11's scheduler
  contract.
- **A2-A:** `EntryOutcome::AlreadyTerminal` doc says "non-failure,"
  but `done_blocked` (terminal failure) children also match. Either
  broaden the doc or split into `AlreadyTerminalSuccess` /
  `AlreadyTerminalFailure`.
- **A2-B:** `AlreadyRunning` conflates "actively being driven" vs
  "exists on disk, non-terminal." Post-retry children with
  `ready_to_drive: false` still return `AlreadyRunning`. Document
  explicitly in koto-user skill.
- **B2-F1:** Retry response doesn't tell the agent which path
  (rewind / respawn-of-skip / respawn-of-spawn_failed) fired per
  child. Add per-child `retry_action: rewind | respawn_skipped |
  respawn_failed` in the response.
- **B2-F2:** No `MultipleReasons` aggregate when a retry submission
  hits two distinct InvalidRetryReason variants (unknown +
  batch-parent). Currently forces multi-round-trip recovery.
  Precedence rule unspecified.

### Cluster 2: Envelope polish (A3-1, A3-2, A3-8, C2)

- **A3-1:** Renaming `InvalidNameDetail`'s inner tag to `kind` fixed
  double-nesting, but introduced a collision at the flattened JSON
  level: `InvalidBatchReason::InvalidName { task, kind:
  InvalidNameDetail }` has an outer `kind` that collides with
  `error.batch.kind: "invalid_batch_definition"`. Rename the outer
  field from `kind` to `name_rule`.
- **A3-2:** `paths_tried: null` lacks `skip_serializing_if`.
- **A3-8:** `CompileError.kind` and `ChildEligibility.current_outcome`
  remain untyped `String` fields.
- **C2:** `TaskSpawnError.path` asymmetry with
  `BatchError::TemplateCompileFailed.path` — add `path` to
  `TaskSpawnError` for parity.

### Cluster 3: Response shape canonicalness (C3-F5, C3-F7, C3-F9, F5 across pairs)

- **C3-F5:** Same fact is learnable three or four ways
  (`materialized_children` vs gate `children` vs `feedback.entries`
  vs `scheduler.errored`). Walkthrough should add a
  "canonical-source-per-question" table.
- **C3-F7:** Terminal `done` response drops `scheduler` key
  entirely; non-terminals emit `scheduler: null`. Pick one.
- **C3-F9:** Post-terminal `koto status` drops the
  `ready/blocked/skipped/failed` name vectors. Undocumented delta.

### Cluster 4: Role + subbatch semantics (C1-F3, C1-F5)

- **C1-F3:** `role` defined as "current state carries the
  `materialize_children` hook" — but when B transitions to
  `analyze_failures` (no hook), `role` flips back to worker right
  when outer observers most need to see it's a coordinator.
  Propose "sticky coordinator" semantics: role becomes
  `coordinator` on first SchedulerRan append and stays sticky.
- **C1-F5:** Rejection precedence order not pinned. When a child
  is both a batch parent AND `pending`, which
  `InvalidRetryReason` variant fires? Proposed order:
  UnknownChildren → ChildIsBatchParent → ChildNotEligible →
  Mixed → AlreadyInProgress.

### Cluster 5: Minor polish

- **B3 / F1:** No `superseded_by` marker on stale `BatchFinalized`
  events; replay tools infer last-wins by log order.
- **C2 / F2:** Cross-tick warning suppression missing
  (pre-D4 warning fires every tick forever).
- **A2-C:** No machine-readable signal for "child reclassified this
  tick" — propose `scheduler.reclassified_this_tick`.

## Verdicts

- **Round 2's three blockers: 3/3 closed.** (The partial closure on
  B2 is a different issue — see B1 architectural gap below.)
- **Round-3 fixes compose cleanly.** Happy-path sanity check (C3)
  confirms no over-engineering; failure-path fields self-omit.
- **One new blocker surfaced (B1).** Round-3 gate tightening made
  a workflow hole structurally visible. Needs one of three
  options decided before v1 ships.
- **~20 should-fix items across four clusters.** Each is localized
  polish; no new structural changes.

## Round-4 scope

**Required:**
- Decide the B1 resolution (Option 1, 2, or 3). Brief single-
  decision work plus small design-doc edits.

**Recommended:**
- Apply ~20 should-fix items from Clusters 1-5 as surgical edits.
  Half-day of prose work.

**Stop-condition candidates:**
- Accept the design as-is with B1 Option 3 documented as a known
  v1 limitation. Ship. Address remaining should-fix items in
  implementation PRs.
- Do one more round-4 revision pass (like round 3), then stop.

## Round

Round 3. No blocker regressions from earlier rounds; one new
architectural gap surfaced from the round-3 fix itself.
