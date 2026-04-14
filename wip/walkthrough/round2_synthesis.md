# Round 2 synthesis: 9 pairs against the revised design

## Purpose

Round 2 verified that round-1's six follow-up decisions (CD9–CD14)
actually close the nine clusters of rough edges they claimed to close.
Each pair was assigned a shape + focus and asked to role-play an
unpredictable agent against the revised design, flagging which CD
claims held and which new gaps appeared.

## Pairs

| Pair | Shape | Focus | CD checked |
|------|-------|-------|------------|
| A1 | Retry | End-to-end flow | CD9 + CD13 |
| A2 | Retry | Edge-case atomicity | CD9 + CD11 |
| A3 | Retry | Observability across terminal | CD13 |
| B1 | Errors | Envelope structure | CD11 |
| B2 | Errors | Per-task spawn failures | CD11 + CD14 |
| B3 | Errors | Path resolution | CD14 |
| C1 | Mutation | R8 rejection | CD10 |
| C2 | Mutation | Dynamic additions × retry | CD9 × CD10 |
| C3 | Mutation | Nested batches | Round-0 E1 × round-1 additions |

Transcripts: `simulation_round2_pair_{a1,a2,a3,b1,b2,b3,c1,c2,c3}.md`.

## Round-1 claims: did they hold?

| CD | Round-1 claim | Round-2 verdict |
|----|---------------|-----------------|
| CD9 retry mechanism | Single authoritative CLI-intercept + template-routed retry flow | **Holds** (A1, A2) |
| CD9 reserved_actions | Discoverable retry without koto-user skill | **Holds** (A1 F5) |
| CD9 runtime reclassification | Replaces synthetic templates; deletes-and-respawns skip markers on state changes | **Fails** — race condition surfaced (A1 F1, C2 F2) |
| CD10 R8 immutability | Rejects every mutation case with typed diff payload | **Holds** (C1, all 8 cases) |
| CD10 spawn_entry snapshot | Readable from disk, enables canonical-form comparison | **Partial** — lifecycle through delete-and-respawn unspecified (C2 F2) |
| CD10 feedback.entries | Per-entry signal for accepted/already/blocked/errored | **Holds** (C1, C2) |
| CD11 error envelope | `action:"error"` + typed discriminators throughout | **Holds** (B1, A2) |
| CD11 pre-append validation | Rejected submissions leave zero state | **Holds** (B1, all 14 probes) |
| CD11 per-task accumulate | Scheduler spawns what it can, reports per-task failures | **Holds at spawn, fails at gate** (B2 F2-F8) |
| CD12 flock | Serializes parent ticks; returns concurrent_tick on contention | **Holds** (B1, C3) |
| CD12 materialized_children | Ledger for idempotent dispatch | **Fails under retry** (A1 F1) |
| CD13 BatchFinalized + batch_final_view | Preserves batch view across terminal transition | **Holds** (A3, A1 F6) |
| CD13 synthetic:true marker | Distinguishes skip markers from real work | **Holds** (A3 F7) |
| CD13 skipped_because_chain | Transitive attribution | **Holds** (A3) |
| CD13 W5 warning | Flags failure-reason source at authoring time | **Holds** (A3) |
| CD14 per-task template failures | Don't halt submission | **Holds** (B2, B3) |
| CD14 TemplateNotFound vs CompileFailed | Typed distinction | **Holds** (B3 Ep5) |
| CD14 node-count DAG depth | Longest root-to-leaf | **Holds** (B3 Ep6) |
| CD14 StaleTemplateSourceDir warning | Surfaces cross-machine drift | **Partial** (B3 Ep3: bypassed by absolute paths) |

Most round-1 claims hold. The convergent failures cluster around one
concrete structural gap (below).

## Blocker-class findings (3)

### B-1. `spawn_entry` lifecycle through runtime reclassification is unspecified

**Surfaced by:** A1 F1, C2 F2

CD9 Part 5 introduces runtime reclassification: the scheduler
delete-and-respawns children whose classification changes (a skip
marker becomes reachable when its blocker succeeds; a running child
becomes a skip marker when its upstream retroactively fails). CD10
relies on an on-disk `spawn_entry` snapshot on each child's
`WorkflowInitialized` event for R8 comparison and canonical-form
matching.

The interaction is undefined in three places:

1. **Race on retry-induced respawn.** Agent retries B. D is downstream
   of B. On the retry tick:
   - B is rewound (epoch bumps).
   - D is a skip marker whose blocker (B) is no longer failed →
     CD9 deletes D and respawns D into its real template's initial
     state.
   - `materialized_children` now reports D as `pending, state:
     working` BEFORE B completes its re-run.
   - Worker pool dispatching from `materialized_children` could pick
     up D and start it while B is still running, violating D's
     `waits_on: [B]` dependency.
   - Round 0's synthetic-template mechanism didn't have this window
     because skip markers stayed in the terminal skip state until
     explicitly delete-and-respawned by retry.

2. **`spawn_entry` write on respawn.** When the scheduler writes D's
   new state file via `init_state_file`, what `spawn_entry` does it
   store? The one from the current submission's merged task list? The
   one from D's original spawn (now invalidated)? If unspecified, a
   subsequent mutation submission gets either R8 false-positives or
   false-negatives depending on implementation choice.

3. **R8-vacuous window.** During retry, transitively-skipped tasks
   (like E downstream of D downstream of B) are in-flight
   delete-and-respawn across multiple ticks. Agents could submit a
   mutation during this window that silently succeeds because the
   on-disk `spawn_entry` is transiently absent.

**Proposed resolution (for round 3):**

- Materialized_children should distinguish "running" from
  "waiting-for-upstream" via a new `ready_to_drive: bool` field that
  is false for any child whose `waits_on` includes a non-terminal
  entry. Worker pools dispatch from `ready_to_drive: true` only.
- `spawn_entry` on respawn carries the CURRENT submission's entry for
  that name (the version that caused the respawn to be valid). Old
  entries remain in the respawned child's event log only as history
  (under the bumped epoch).
- R8 comparison during retry window: if `spawn_entry` is absent
  (child is mid-respawn), R8 is skipped and the entry is treated as
  pending. Agents see a new `EntryOutcome::Respawning` feedback
  variant.

Either fix is localized to CD9 + CD10 + CD12 sections; no rework
beyond. The semantic regression is real but the fix is small.

### B-2. `spawn_failed` lifecycle not gated in the extended gate vocabulary

**Surfaced by:** B2 F2-F4, F8

CD11's per-task spawn failure accumulates tasks with `outcome:
spawn_failed`. But the gate output's `all_complete` is defined as
`pending == 0 AND blocked == 0`, with no term for `spawn_failed`.
A submission with 5 tasks, 3 spawn+succeed, 2 spawn-fail:

- `pending` (not-yet-terminal children): 0 after all 3 complete
- `blocked` (waiting on deps): 0 (the 2 failed never had deps)
- `spawn_failed`: 2
- `all_complete`: TRUE (per the formula)

Parent transitions straight to `summarize`, batch reports success
despite 2 tasks never running. `retry_failed`'s R10 (`InvalidRetryReason::NonRetryableChildren`)
rejects children with `outcome: spawn_failed` because they're not
in the narrow "failed" or "skipped" set.

**Proposed resolution (for round 3):**

- Tighten `all_complete` to `pending == 0 AND blocked == 0 AND
  spawn_failed == 0`.
- Add `any_spawn_failed` boolean to gate output alongside
  `any_failed`, `any_skipped`.
- Extend `needs_attention` to `any_failed > 0 OR any_skipped > 0 OR
  any_spawn_failed > 0`.
- Loosen R10 to accept `spawn_failed` children in retry. Semantics:
  `retry_failed` on a `spawn_failed` task doesn't rewind (nothing to
  rewind), it re-attempts `init_state_file` using the CURRENT
  submission's entry.

The ecosystem change is small — four fields + enum-loosening — but
it touches the reference `coord.md` template because existing
routing on `all_complete: true` would start firing on previously-
reachable paths.

### B-3. Outer retry_failed on a nested-batch child is cosmetic

**Surfaced by:** C3 F4

Nested batches are a v0.7.0 primitive (Reading B, kept unchanged by
the round-1 work). A parent in one batch can itself be a batch
parent running a sub-batch. If the outer coordinator fires
`retry_failed: {"children": ["coord-outer.B"]}` where B is a batch
parent, CD9's mechanism rewinds B's event log but NOT B's children
(B1, B2). The inner batch state survives the rewind; B restarts
with the stale inner children still present.

**Proposed resolution (for round 3):**

Two viable directions:

- **Unsupported + documented:** add `InvalidRetryReason::ChildIsBatchParent`
  rejection. Agents must explicitly coordinate retries at the right
  level. Cheaper; matches "each level is independent" thesis.
- **Cascading:** outer retry on B cascades to B's children (all get
  rewound / delete-and-respawned). Requires additional traversal at
  retry time. More expensive; closer to the "retry should do the
  right thing" user expectation.

Recommend **documented-unsupported** for v1. Nested batches are the
exceptional path; most batches are flat. v1.1 can add cascading if
users complain.

## Should-fix clusters (round-3 candidates)

### Cluster 1: CD11 envelope cleanup (B1 findings)

- **Dual representation for limit violations.** `BatchError::LimitExceeded`
  (its own variant with `LimitKind`) vs. `InvalidBatchReason::LimitExceeded{Tasks,WaitsOn,Depth}`
  (variants of InvalidBatchDefinition reason enum). Drop the
  InvalidBatchReason variants; hoist `LimitExceeded` to a sibling
  `kind`.
- **No typed `InvalidRetryReason::UnknownChildren`.** Currently reuses
  `ChildNotEligible` with `current_outcome: "unknown"` sentinel;
  violates CD11's anti-sentinel commitment. Add the variant.
- **Double-nesting in `InvalidName`.** Rename inner serde tag from
  `detail` to `kind`.
- **`concurrent_tick` as free string.** Promote to typed
  `BatchError::ConcurrentTick { holder_pid: Option<u32> }` variant.
- **`RetryAlreadyInProgress` is dead code** under CD12 flock. Remove
  or document as "reserved for future non-flocked semantics."
- **`TaskSpawnError.compile_error` is a free string.** Promote to the
  typed `BatchError::TemplateCompileFailed { path, compile_error:
  CompileError }` shape.

### Cluster 2: CD9 × CD10 × CD12 interaction polish

- **Redaction sentinel literal.** Pin `"[REDACTED]"` vs `{"redacted":
  true}` as the canonical redacted-value representation.
- **R4-before-R8 ordering.** Documented in Data Flow but not in
  Decision 10; add a local cross-reference.
- **Canonical-form `default_template` resolution point.** Resolve
  against spawn-time hook snapshot, not live hook, to prevent R8
  false-positives when a parent template's `default_template`
  changes mid-batch.
- **`EntryOutcome::Already` conflates cases.** Running child vs.
  skip marker look the same in feedback. Split into
  `Already::Running` and `Already::Skipped`.
- **Omission emits no signal.** When a previously-submitted task
  name is absent from a resubmission, nothing tells the agent. Add
  `SchedulerWarning::OmittedPriorTask`.
- **`tasks` evidence unreachable at `analyze_failures`.** Dynamic
  additions post-failure require returning to `plan_and_await` first.
  Walkthrough.md doesn't flag this.
- **`orphan_candidates` fires AFTER duplicate work spawned.** By the
  time the agent sees the warning, the old child is still running
  and a new child has spawned. Consider pre-spawn detection.

### Cluster 3: CD13 vocabulary and edge cases

- **`batch.phase` ambiguity.** When `BatchFinalized` is appended but
  the parent is still on the batched state, phase is ambiguous.
  Pin semantics.
- **`reason_source` enum missing variants.** Walkthrough assumes
  values like `skipped`, `not_spawned` that CD13 doesn't define.
  Close the vocabulary.
- **`skipped_because` vs `skipped_because_chain` tie-break.** For
  diamond skip scenarios (two dependencies both failed), singular
  disambiguation is unspecified.
- **`batch_final_view` is a frozen snapshot.** Doesn't reflect
  post-finalization drift from retry. Document as frozen-at-first-
  terminal.
- **Stale `BatchFinalized` during retry.** Separately flagged by
  A1-F2 and C3-F8. Convergent with the `spawn_entry` lifecycle
  issue above; fix together.
- **Synthetic child directive prose.** Walkthrough has generic
  directive; CD13 example interpolates `{{skipped_because}}`.
  Pick one.

### Cluster 4: CD14 polish

- **Pre-D4 warning noise.** `MissingTemplateSourceDir` re-fires
  every tick; no header-rewrite path to clear it.
- **Absolute paths bypass CD14 warnings at submission time.**
  Staleness only surfaces when child ticks on the receiving
  machine.
- **`TaskSpawnError` doesn't tag `default_template` inheritance.**
  Agents can't render targeted recovery messaging.
- **`StaleTemplateSourceDir` enrichment.** Variant carries only
  `path`; agents must recompose `machine_id` and `falling_back_to`
  from other fields.
- **`paths_tried` canonicalization.** Literal `..` segments are
  echoed back rather than canonicalized — harder to read.

### Cluster 5: Nested-batch agent-navigation story

- **Two-hat intermediate child.** A child that is itself a batch
  coordinator needs agent-facing docs. CD12 Q8's "coordinator
  drives parent, workers drive children" doesn't cover this.
- **No `role: coordinator | worker` marker** on children.
- **`batch_final_view` is per-level, not recursive.** Undocumented.
- **`subbatch_status` field on per-child rows.** Propose adding for
  outer-level visibility into inner-batch state.

## Round-3 scope estimate

**Blockers (must fix before implementation):** 3
- Runtime reclassification race + spawn_entry lifecycle
- spawn_failed gate integration
- Nested retry semantics (pick documented-unsupported OR cascading)

**Should-fix clusters (5):** ~25 small spec/shape/vocabulary
fixes, all localized to CD9-CD14 sections. No new decisions
required; tightenings of existing commitments.

A single design-doc revision pass addresses all blockers and
should-fix items. Estimated effort: same size as round-1
fold-in (half-day prose surgery with a few small Rust type
changes).

Alternative framing: promote the 3 blockers to Combined
Decision 15 (a "lifecycle + gating" decision that covers
reclassification, spawn_failed, and nested-retry semantics as
one coherent package), leave the should-fix clusters to
inline edits.

## Round

Round 2. Cross-validation not re-run (no new decisions).
