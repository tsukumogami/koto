# Round 1 synthesis: 9 pairs across 3 shapes

## Pairs run

| Pair | Shape | Focus |
|------|-------|-------|
| 1a | Diamond + retry | generic probing |
| 1b | Diamond + retry | retry-edge cases (double, partial, during running) |
| 1c | Diamond + retry | observability (status, workflows, failure_reason) |
| 2a | Dynamic additions | generic probing |
| 2b | Dynamic additions | field-mutation semantics |
| 2c | Dynamic additions | concurrency (TOCTOU, split-brain, re-entrance) |
| 3a | Errors | generic probing |
| 3b | Errors | response-envelope JSON shape |
| 3c | Errors | path resolution + limit boundaries |

Transcripts: `simulation_round1_pair{1,1b,1c,2,2b,2c,3,3b,3c}.md` in this folder.

## Clusters (by convergence across pairs)

### Cluster A — Retry path is unreachable from the canonical reference template

**Flagged by:** 1a (blocker), 1c (F1, F2, F3), 1b (indirectly)

`coord.md` in `walkthrough.md` has a single transition `plan_and_await → summarize` on
`gates.done.all_complete: true`. Decision 5.3 defines `all_complete` as
`pending == 0 AND blocked == 0`, which is TRUE for a batch with failures and
skipped children. So the canonical template routes **straight to `summarize`**
the first time the batch reaches "everything terminal" — including the failed
state. The agent never sees a window in which to submit `retry_failed`.

Compounding:

- **1c F2:** `done` responses in `walkthrough.md` drop `blocking_conditions`
  and `scheduler`. When the agent lands on `summarize`, all batch-outcome
  detail is gone from the response.
- **1c F3:** `koto status` only emits the `batch` section when the current
  state has a `materialize_children` hook. Once the parent has advanced past
  `plan_and_await`, batch state vanishes from `status` too.
- **1c F1:** no compile-time warning against a single-transition template
  that swallows failures.

**Root cause:** the reference template doesn't distinguish "complete with all
success" from "complete with failures and/or skips," and the response surface
doesn't preserve batch data across the transition.

**Proposed resolution:**

1. Rewrite the reference `coord.md` in the walkthrough to route failure and
   skip cases to an `analyze_failures` state with an accepts field for
   `retry_failed` or a terminal-summary transition.
2. Expose `gates.done.failed`, `gates.done.skipped`, and `gates.done.success`
   as guardable fields alongside `all_complete`, so templates can route on
   "all complete AND failed == 0" vs "all complete AND failed > 0".
3. Preserve the batch view on the parent through terminal states: either keep
   the last-known batch view in `koto status` regardless of current state
   having a hook, or have `done` responses carry `batch_final_view` when the
   parent was batched during its lifetime.
4. Add compile warning W4: a template with `materialize_children` whose
   transitions only route on `all_complete: true` without also handling
   `failed > 0` emits a warning.

### Cluster B — retry_failed transition mechanism self-contradicts

**Flagged by:** 1a (#2), 1b (blocker + six should-fix), 1c indirectly

- **1b blocker 1:** Decision 5.4 step 1 says `handle_retry_failed` transitions
  the parent directly (from post-analysis state back to `awaiting_children`).
  Data Flow's "Retry" walkthrough says the *advance loop* routes via a
  template-defined transition. These cannot both be true.
- **1b blocker 2:** when a running child's upstream flips to Failure after
  retry (e.g., B re-fails during its retry epoch), D (still running in its
  real-template child) has no legal transition path to `skipped_due_to_dep_failure`
  because D is not a synthetic-template instance.
- **1a #2:** `retry_failed` is reserved and prohibited from `accepts`, so
  `expects.fields` never describes it. Agents without the koto-user skill
  can't discover it from the response.
- **1b should-fix:** double `retry_failed` without intervening action is
  unspecified (idempotent? two Rewound events?).
- **1b should-fix:** retry on a running child is unguarded — rewinds
  in-flight work.
- **1b should-fix:** retry with a successful child in the set is
  unspecified.
- **1b should-fix:** closure direction through the DAG is ambiguous.
  `include_skipped` extends downward; there's no upward expansion, so
  retrying only a skipped node without its failed ancestor re-thrashes
  in one tick.
- **1b should-fix:** mixed payload (`retry_failed` + other required fields)
  interception is unstated.
- **1a #5:** stale skip markers when `include_skipped: false` — successful
  retry of B leaves D permanently marked skipped.

**Proposed resolution:**

1. Pick one path for the parent transition. The cleaner option is
   template-defined: require templates to have a transition from
   analysis state back to the batched state on a guard like
   `gates.done.retry_submitted: true`. `handle_retry_failed` writes the
   child rewinds + clearing event; the *next* `koto next` tick runs the
   advance loop which picks up the transition naturally. Delete 5.4 step 1's
   direct-transition claim.
2. Surface `retry_failed` in the response as a synthetic `reserved_actions`
   block on gate-blocked responses where the gate reports any `failed > 0`
   or `skipped > 0`.
3. Add explicit behavior for all six retry edges: guard against retry on
   running children (error), reject retry set containing non-failed
   non-skipped children (error), reject double retry_failed before the
   clearing event is consumed (idempotent: second is no-op), define closure
   direction as **downward only** (dependents of the retry set) with
   `include_skipped: true` as default, and reject mixed payloads.
4. Replace the "synthetic template with skipped_marker state" mechanism
   with **runtime reclassification**: on every scheduler tick, re-evaluate
   skipped status from current disk-derived dependency outcomes. If a
   skipped marker's blocker is no longer failed, delete-and-respawn with
   the real template. This closes the "running child needs to become
   skipped" gap and the "stale skip after partial retry" gap simultaneously.

### Cluster C — Mutation semantics on already-spawned tasks

**Flagged by:** 2a (blocker), 2b (F1-F6), 2c (F1), 3c (F9)

"merge_epoch_evidence unions the new tasks with the existing set" collapses
under mutation pressure. Specific gaps:

- **`vars` change** on a Running child: currently silently dropped (backend
  idempotency skips respawn).
- **`waits_on` change** on a Running child: creates inconsistent gate
  output (Running child appears blocked on another task).
- **`template` change** on a Running child: silently dropped, persists
  through retry because rewind preserves original header template.
- **Task removal**: impossible under union semantics. No cancel primitive.
- **Renaming**: `issue-A` → `issue-A-renamed`: old-name child keeps
  running, new-name child spawns. Two workers, two PRs for same issue.
- **Cross-epoch duplicate names**: R5 catches within-submission duplicates
  but not across submissions.
- **Identical resubmission**: no-op pollutes event log.
- **Retry + resubmission ordering**: interleaving unspecified.
- **No feedback**: response doesn't signal which parts of a resubmission
  took effect vs. were ignored.

**Proposed resolution:**

1. Add runtime rule **R8: spawn-time immutability** — task entries whose
   `<parent>.<name>` child already exists on disk have their fields frozen
   at first spawn. Any change to `vars`, `waits_on`, or `template` for
   such a task in a subsequent submission is rejected with
   `InvalidBatchDefinition { sub_kind: "SpawnedTaskMutated" }`.
2. Define union semantics narrowly: **append-only task set, no mutation,
   no removal.** New task entries (new names) are unioned in. Entries
   with existing on-disk children must match prior entries field-for-field.
3. Introduce a `cancel_tasks` reserved evidence action as the explicit
   counterpart to `retry_failed` for deprecation/removal.
4. Add an `ignored` field to `SchedulerOutcome` listing task entries that
   were present in the submission but had no effect (e.g., identical
   resubmission of an already-spawned task).
5. On identical resubmission, optionally short-circuit the EvidenceSubmitted
   append — but this is a nice-to-have; the blocker is the mutation gap.

### Cluster D — Error response envelope, pre/post-append validation

**Flagged by:** 3a (two blockers), 3b (F1, F2, F3, F4, F8, F9, F11), 3c (F8)

- **3a blocker 1:** error response JSON envelope is entirely unspecified.
  No `action: "error"` variant exists anywhere in the design.
- **3a blocker 2:** pre-append vs post-append validation contradicts
  itself. Phase 3 Implementation: "submission-time hard limit enforcement"
  (pre-append). Data Flow Step 4: `EvidenceSubmitted` is appended *before*
  the scheduler runs R1-R7.
- **3b F1:** design's `NextError::Batch { kind, message }` contradicts
  existing `NextError { code, message, details }` in
  `src/cli/next_types.rs:283-289`.
- **3b F2:** scheduler halt-on-error vs partial-accumulation unspecified.
  `SchedulerOutcome::Error { reason: String }` is single-error; no
  per-task vector exists.
- **3b F3:** `InvalidBatchDefinition.reason` is a free string; R1-R7 all
  collapse into one opaque bucket — agents can't pattern-match.
- **3b F8:** no `SchedulerRan` event — partial spawn failures leave no
  audit trail.
- **3b F11:** `BatchTaskView.outcome` has no `spawn_failed` variant;
  failed-to-spawn tasks look identical to "not yet processed".
- **3a #4:** empty task list `{"tasks": []}` semantics undefined.
- **3a #5 / 3c F12:** `template: null` vs omitted key undefined.

**Proposed resolution:**

1. Add an `action: "error"` response variant to the design's response
   envelope list. Shape:
   ```json
   {
     "action": "error",
     "state": "plan_and_await",
     "error": {
       "code": "InvalidBatchDefinition",
       "sub_kind": "Cycle",
       "message": "...",
       "details": { "cycle_path": ["A", "B", "A"] }
     },
     "scheduler": null
   }
   ```
2. Commit to **pre-append** validation for all batch-definition checks
   (R1-R7, limits). Delete Data Flow Step 4's implication of post-append.
   Evidence is appended only on accepted submissions. This matches
   Phase 3's stated behavior and keeps audit logs clean.
3. Replace `NextError::Batch { kind, message }` with reuse of existing
   `NextError { code, message, details }` + `details[0].batch_kind` as
   discriminator. One envelope, not two.
4. Add `SchedulerOutcome::Scheduled { errored: Vec<(String, SpawnError)>, ... }`
   so partial-success ticks are representable. Per-task spawn errors
   don't halt the tick.
5. Replace `InvalidBatchDefinition.reason: String` with a typed enum
   listing R1-R7 plus cycle-from-merge, dangling-ref, etc.
6. Add `BatchTaskView.outcome: "spawn_failed"` variant; include
   `spawn_error` field.
7. Append `SchedulerRan` event to the parent log on every tick that
   makes non-trivial decisions (spawn, skip-synthesis, spawn-failure).
8. Define empty task list as a validation error (R0: non-empty). Reject.
9. Define `template: null` as equivalent to omitted (inherits
   `default_template`). Document explicitly.

### Cluster E — `scheduler.spawned` is observation, not ledger

**Flagged by:** 2c F2, 2c F7

`scheduler.spawned` reports what *this tick* materialized. Two concurrent
`koto next parent` ticks can both see task-4 as ready, both spawn it (one
wins the rename race, the other's bundle gets clobbered), and *both report
spawning it* in their responses. Consumers that use `spawned` for idempotent
worker dispatch will double-dispatch.

Related: 2c F7 — "serialize parent ticks" is a caller-held invariant with
zero enforcement, and the walkthrough's "any caller can drive the parent"
language actively invites violation.

**Proposed resolution:**

1. Rename `scheduler.spawned` to `scheduler.spawned_this_tick` in docs
   and response JSON to make per-tick-ness explicit.
2. Add a `scheduler.materialized_children` field (or reuse `children` in
   gate output) that is the ledger — every child that exists on disk
   right now, with its outcome. Consumers needing idempotent dispatch
   use this, not `spawned`.
3. Use `renameat2(RENAME_NOREPLACE)` in `LocalBackend::init_state_file`
   to close the TOCTOU at the kernel level on Linux; fall back to
   `link()` + `unlink()` on platforms without it.
4. Add an advisory lockfile per parent workflow in `handle_next`. The
   "stateless CLI" principle is about persistence, not mutual exclusion;
   a short-held flock during the advance+scheduler cycle is compatible.

### Cluster F — Post-completion observability

**Flagged by:** 1c F2, F3, F4, F6, F8, F10

- Terminal responses drop batch detail.
- `koto status` drops batch section once parent advances past the
  batched state.
- Synthetic skipped children look identical to real terminal work in
  `koto status`.
- Transitive skip attribution is singular (`skipped_because: X`) —
  undefined whether X is the direct blocker or root cause.
- `reason: "done_blocked"` (state-name fallback when `failure_reason`
  unset) is opaque.
- No compile warning for `failure: true` states missing a
  `failure_reason` writer.

**Proposed resolution:**

1. Persist last-known batch view on the parent through terminal states
   (see Cluster A fix).
2. Add `synthetic: true` or `kind: "skip_marker"` field to
   `koto status` output for synthetic children; `koto next
   <synthetic-child>` returns a terminal response immediately with no
   directive.
3. Record `skipped_because_chain: ["D", "B"]` (array of attribution
   path) in addition to the singular `skipped_because`.
4. Add compile warning W5: `failure: true` states with no
   `default_action` writing `failure_reason` to context emit a warning.

### Cluster G — Concurrency enforcement gaps

**Flagged by:** 2c F3-F10

- No advisory lock on parent state file.
- `koto session resolve` reconciles parent log but not per-child state
  files.
- Responses lack `sync_status` / `machine_id`, so split-brain observers
  can't detect they're reading the losing side.
- `retry_failed` appends child `Rewound` events before parent-log push
  — losing cloud-sync push leaves orphan epochs.
- `repair_half_initialized_children` doesn't sweep leaked `.koto-*.tmp`
  files from rename losers.
- Task-list extension mid-run can cause `all_complete: true` to
  flicker, breaking consumers triggering on that edge.

**Proposed resolution:**

1. Adopt Cluster E fix #3 (RENAME_NOREPLACE) + #4 (advisory lock).
2. `koto session resolve` must reconcile child state files, not just
   the parent log. Document in Decision 2 or add a new consideration.
3. Order `retry_failed` as: compute retry set → write clearing event
   to parent → sync_push parent → write Rewound events to children.
   If parent push fails, children are untouched.
4. Sweep `.koto-*.tmp` files in `repair_half_initialized_children`.
5. Soften the walkthrough's language: "the coordinator drives the
   parent. Workers drive children. A worker SHOULD NOT drive the
   parent directly."

### Cluster H — Path resolution contradictions

**Flagged by:** 3c F1, F3, F6, F7, F10, F11

- **3c F7 (high):** design contradicts itself on whether a single bad
  child template fails the whole submission (R1 + Data Flow) or just
  that task (BatchError docstring).
- **3c F3:** `template_source_dir == None` fallback path not specified.
  Design says "on ENOENT, fall back to submitter_cwd" — but absent is
  different from ENOENT.
- **3c F6:** `TemplateResolveFailed` conflates not-found with
  compile-failed; `paths_tried` is meaningless for the compile case.
- **3c F1, F11:** absolute paths + cloud-sync portability: both
  `template_source_dir` and `submitter_cwd` capture machine-specific
  absolute paths.
- **3c F10:** "DAG depth 50" has three plausible definitions (longest
  root-to-leaf path, longest path from any node, max transitive-closure
  chain).

**Proposed resolution:**

1. Decide halt-vs-accumulate (Cluster D fix #4 applies). Recommend
   per-task: scheduler spawns what it can, reports `spawn_failed` for
   the rest; batch submission itself is not failed.
2. Define: if `template_source_dir` is absent in the header, resolution
   uses only `submitter_cwd`. Explicitly document.
3. Split `TemplateResolveFailed` into `TemplateNotFound
   { paths_tried }` and `TemplateCompileFailed { path, compile_error }`.
4. Document the cloud-sync portability limitation explicitly in
   Security Considerations (already sort of there, but it should note
   the `template_source_dir` captured-at-init implication).
5. Define DAG depth as **longest path from any root (node with empty
   waits_on) to any leaf**. Reject at R6.

### Cluster I — Small validation edge cases

**Flagged by:** 3a (#1, #6, #8, #10), 3c (F2, F12)

- Reserved name collisions (task named `retry_failed`, empty name,
  name == `..`).
- `LimitExceeded.which` is `&'static str`; needs enum.
- Premature `retry_failed` (no batch materialized) — no specified
  error variant.
- `..` traversal in template paths is silent.
- `template: null` vs omitted key.

**Proposed resolution:**

1. R-rules for name validation: R9 — task.name matches
   `[a-zA-Z0-9_-]+`, is non-empty, does not equal `retry_failed` or
   `cancel_tasks`.
2. `LimitExceeded.which: LimitKind` (enum).
3. New variant `BatchError::InvalidRetryRequest { reason }`; covers
   premature retry, empty child list, non-failed children.
4. `template: null` = inherit default (documented).
5. Add brief note in directive prose for a bad path (`..` accepted
   but unsanitized) — optional.

## Severity summary

| Cluster | Severity | Convergence |
|---------|----------|-------------|
| A. Retry unreachable from reference template | Blocker | 3 pairs |
| B. retry_failed transition contradiction | Blocker | 3 pairs |
| C. Mutation semantics on spawned tasks | Blocker | 4 pairs |
| D. Error envelope + pre/post-append | Blocker | 3 pairs |
| E. scheduler.spawned ledger vs observation | Should-fix | 1 pair |
| F. Post-completion observability | Should-fix | 1 pair |
| G. Concurrency enforcement | Should-fix | 1 pair |
| H. Path resolution contradictions | Should-fix | 1 pair |
| I. Small validation edges | Nice-to-have | 2 pairs |

## Recommended debate order

1. **Cluster A + Cluster B together** — they're the retry-path story.
   Either retry is reachable and mechanically sound, or the whole
   `failure_policy: skip_dependents + retry_failed` apparatus is
   ornamental.
2. **Cluster C** — mutation semantics on spawned tasks. Biggest
   unspecified surface area. Blocks the "dynamic additions" feature as
   documented.
3. **Cluster D** — error envelope + validation timing. Two
   independent issues bundled because they share the error-flow
   surface.
4. **Cluster E** — scheduler.spawned ledger. Small fix, high impact
   for consumers.
5. **Clusters F, G, H, I** — bundled cleanup.
