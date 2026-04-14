# Round 1 resumption plan

## Status at pause

- **Branch:** `docs/batch-child-spawning`
- **Design doc:** `docs/designs/DESIGN-batch-child-spawning.md`, status `Proposed`
- **Existing coordination:** `wip/design_batch-child-spawning_coordination.json`
  has 8 decisions complete, cross_validation passed, round 0.
- **Walkthrough round 1:** 9 pair simulations complete, synthesis at
  `wip/walkthrough/round1_synthesis.md`. Nine rough-edge clusters (A-I)
  identified; four are blocker-class, five are should-fix or nice-to-have.
- **Rounds 2+:** not yet started. Pending the decisions below.

## The plan

We are restarting at the **beginning of the `/shirabe:design` process**
with the 9 clusters queued as new decision questions. Order of operations:

1. Re-enter the design workflow for `batch-child-spawning` (Phase 0 handoff
   reading this file plus `round1_synthesis.md` as context).
2. Run `/shirabe:decision` **once per cluster**, in the order below. Each
   cluster becomes Decision 9-17 on the coordination manifest. Round counter
   bumps to 1.
3. After all 9 new decisions settle, run cross-validation across Decisions
   1-17 (the existing 8 plus the 9 new).
4. Revise the design doc end-to-end: rewrite sections affected by new
   decisions, update the reference template in `walkthrough.md`, refresh the
   coordination manifest, bump design to a second `Proposed` iteration.
5. Run walkthrough Round 2 (another 9 pairs across refreshed shapes).
6. Iterate until walkthroughs stop surfacing blocker-class findings.

## Decisions to run (in order)

**Mode: combined decisions** — fewer decision docs with bigger scope per
decision. Six combined decisions instead of nine per-cluster decisions.
Each entry below is the question for one `/shirabe:decision` invocation.

### Combined Decision 9 — Retry path end-to-end (Clusters A + B)

**Scope:** the full retry story — reachability, mechanism, edges, synthetic
template review.

Sub-questions:
- **Reachability:** how does a template author's reference pattern reach
  the retry path instead of routing failed batches straight to terminal?
  What guard-field vocabulary (e.g., `gates.done.failed`,
  `gates.done.success`) is exposed alongside `all_complete`? How does the
  reference `coord.md` change?
- **Mechanism:** pick one authoritative story for `retry_failed`. Either
  (a) handler transitions parent directly (Decision 5.4 step 1), or
  (b) handler writes state + rewinds, advance loop picks up a template-
  declared transition on next tick (per Data Flow). Delete the other.
- **Discovery:** how is `retry_failed` surfaced to agents in the response
  (synthetic `reserved_actions` block on failed-batch responses)?
- **Edges:** double retry without intervening tick; retry on running child;
  retry with a successful child in the set; closure direction (downward
  only, default `include_skipped: true`); mixed `retry_failed + other
  evidence` payload; stale skip markers after partial retry.
- **Synthetic template:** replace Decision 5.2's synthetic-template-per-
  skipped-child with runtime reclassification, or keep it? Either choice
  cascades through the rest of the decision.

**Affects:** Decision 5 (entire), Decision 5.3 `all_complete` invariant,
Decision 6 batch-view emission, `walkthrough.md` reference template,
response shape for terminal `done` responses.

### Combined Decision 10 — Mutation semantics + dynamic-addition primitives (Cluster C)

**Scope:** the full contract for resubmissions.

Sub-questions:
- Spawn-time immutability: reject any change to `vars`, `waits_on`, or
  `template` on task entries whose child already exists on disk?
  (Runtime rule R8.)
- Union vs replace semantics: append-only task set, or full replace?
- Removal: explicit `cancel_tasks` reserved evidence action, or not
  supported in v1?
- Renaming: explicitly rejected (breaks child-name determinism) or
  silently accepted with duplicate spawn? (Currently the latter.)
- Cross-epoch duplicate resolution: first-wins, last-wins, or rejection?
- Identical resubmission: no-op and do-not-append-event, or append for
  audit?
- Feedback: `ignored` field on `SchedulerOutcome` listing no-op task
  entries — in or out?

**Affects:** Decision 1 schema + validation, Data Flow Step 4,
"Dynamic additions" exploration constraint, Decision 5 retry interaction.

### Combined Decision 11 — Error envelope, validation timing, validation edges (Clusters D + I)

**Scope:** everything about how errors flow out of koto and the edge
validation rules.

Sub-questions:
- JSON shape of an error response from `koto next` (new `action: "error"`?
  field layout?).
- Pre-append vs post-append validation. Commit one; delete the
  contradicting text. Recommend pre-append.
- Reconcile design's `NextError::Batch { kind, message }` (line 1610)
  with the existing `NextError { code, message, details }` shape at
  `src/cli/next_types.rs:283-289`.
- Scheduler: halt-on-first-error, or per-task-accumulate-with-partial-
  success? If the latter, add `errored` field to `SchedulerOutcome` and
  `spawn_failed` variant to `BatchTaskView.outcome`.
- `SchedulerRan` event in the parent log: yes or no?
- Empty task list `{"tasks": []}`: error, or valid immediately-complete
  batch?
- `template: null` vs omitted key: equivalent or different?
- `InvalidBatchDefinition.reason`: free string or typed enum covering
  R1-R7?
- Reserved-name validation (R9: `task.name` regex, non-empty, not
  `retry_failed` / `cancel_tasks`).
- `LimitExceeded.which` as enum.
- `BatchError::InvalidRetryRequest` variant for premature retry.
- `..` in template paths: silent accept (current) or logged warning?

**Affects:** Decision 1 (validation rules), Decision 5 (gate output),
Decision 4 (path resolution), Key Interfaces section,
`src/cli/next_types.rs` contract.

### Combined Decision 12 — Concurrency model hardening (Clusters E + G)

**Scope:** the full concurrency enforcement and observability story.

Sub-questions:
- Rename `scheduler.spawned` to `scheduler.spawned_this_tick`, add a
  separate `materialized_children` ledger field — in or out?
- `renameat2(RENAME_NOREPLACE)` + `link`/`unlink` portable fallback for
  `LocalBackend::init_state_file` — in or out?
- Advisory lockfile per parent workflow during `handle_next` — in or
  out, and does it contradict the "stateless CLI" driver?
- `koto session resolve`: reconcile per-child state files, not just
  parent log?
- `sync_status` / `machine_id` fields on responses under cloud sync?
- Ordering of `retry_failed` steps: clearing event to parent before
  child Rewound events, or after?
- `repair_half_initialized_children`: sweep leaked `.koto-*.tmp` files?
- Walkthrough language change: "any caller can drive the parent" →
  "the coordinator drives the parent; workers drive only their own
  children."

**Affects:** Decision 2 atomic init, Concurrency Model section,
Decision 5.4 retry ordering, walkthrough prose.

### Combined Decision 13 — Post-completion observability (Cluster F)

**Scope:** what the observer sees after the batch finishes.

Sub-questions:
- Preserve last-known batch view in `koto status` after the parent
  leaves the batched state: carry through terminal, carry through all
  subsequent states, or store as `batch_final_view` on a `Batched`
  marker event?
- `done` / `workflow_complete` response shape: include `batch_final_view`?
- Synthetic skipped children: expose `kind: "skip_marker"` in
  `koto status`? What does `koto next <synthetic-child>` return
  (immediate terminal, error, or directive "this child was synthesized
  as skipped, nothing to do")?
- Transitive skip attribution: add `skipped_because_chain: [...]` array
  alongside singular `skipped_because`?
- `failure_reason` compile warning (W5): warn when a `failure: true`
  state has no `default_action` writing `failure_reason` to context?

**Affects:** Decision 6, Decision 5.2 synthetic children, template
compile warnings list.

### Combined Decision 14 — Path resolution contradictions (Cluster H)

**Scope:** close the path-resolution gaps and contradictions.

Sub-questions:
- Halt-vs-per-task on bad child template: design contradicts itself
  between R1 / Data Flow (halt submission) and BatchError docstring
  (per-task). Pick one. Recommend per-task.
- `template_source_dir == None` fallback: skip straight to
  `submitter_cwd`? Warning surfaced to agent?
- Split `TemplateResolveFailed` into `TemplateNotFound` (with
  `paths_tried`) and `TemplateCompileFailed` (with `path`,
  `compile_error`)?
- DAG depth: longest root-to-leaf path, longest any-to-any, or max
  transitive-closure chain? Recommend longest root-to-leaf.
- Cloud-sync portability doc note: captured-at-init absolute paths
  limit multi-machine deployments.

**Affects:** Decision 4, Decision 1 R6 (depth), scheduler per-task
spawn loop, Security Considerations.

## Decision dependencies

- **CD9 comes first.** Its retry-mechanism choice cascades through CD10
  (mutation during retry) and CD13 (batch-view preservation).
- **CD10 depends on CD9.** Mutation rules during retry reference the
  chosen retry mechanism.
- **CD11 is largely independent** but should settle before CD12 since
  CD12 will want to return well-shaped errors from new concurrency
  guardrails.
- **CD12, CD13, CD14 are independent of each other** once CD9-CD11 land.

Recommended execution order: **CD9 → CD10 → CD11 → (CD12, CD13, CD14 in
any order or parallel).**

## Cross-validation after decisions

After D9-D17 settle, the cross-validation pass must at minimum verify:

- D9's guard-field vocabulary is consistent with D5's gate output fields.
- D10's chosen retry transition model is consistent with D5.4's event
  sequence and D11's mutation rules.
- D11's spawn-time immutability + D5.2's synthetic template (if retained)
  don't conflict — synthetic templates are per-task; immutability rules
  apply per-task.
- D12's error envelope is consistent with every place the existing doc
  shows a sample response.
- D13's ledger is consistent with D6's `derive_batch_view` output.
- D15's concurrency story doesn't contradict the "stateless CLI" driver.

## Inputs for the next `/shirabe:design` run

Point the design workflow at:

- `wip/walkthrough/round1_synthesis.md` — the clustered findings.
- `wip/walkthrough/simulation_round1_pair*.md` — the 9 transcripts with
  inline `[GAP: ...]` markers and per-pair findings.
- The existing design doc for the 8 settled decisions (do not re-decide
  those unless a round-1 finding forces it).
- This file for sequencing and scope.

## Reminder for future me

- Do not touch the design doc or coordination manifest until all 9 new
  decisions settle — partial edits will confuse cross-validation.
- Do not re-open Decisions 1-8 unless a round-1 finding makes a
  previously-settled choice inoperative. Flag any such case in the new
  decision's Context section.
- After revising the doc, run walkthrough Round 2 with fresh pair shapes
  (not a replay of Round 1) before declaring the design stable.
