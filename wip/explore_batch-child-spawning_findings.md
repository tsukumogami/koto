# Exploration Findings: batch-child-spawning

## Core Question

How should koto let a parent workflow declare a DAG of children — possibly
growing at runtime — and own their spawning, ordering, and completion
detection, so consumers stop writing spawn loops in SKILL.md prose?

## Round 1

### Key Insights

- **The existing `children-complete` gate is the right convergence primitive
  unchanged.** Every lead that touched the gate concluded it already does the
  right thing: it lists sessions filtered by `parent_workflow`, aggregates
  state, and blocks until all are terminal. No gate-level changes are needed.
  (`lead-koto-integration`, `lead-evidence-shape`, `lead-dynamic-additions`)

- **Koto's file-based, stateless-CLI model points at one storage strategy:
  derive everything from disk.** The koto-integration lead examined three
  options (header mutation, sidecar JSONL, full derivation) and found only
  derivation preserves append-only semantics, keeps cloud sync simple, and
  requires zero new event types. Batch definition lives in an existing
  `EvidenceSubmitted` event; spawn records are child state files discovered
  via the same `backend.list()` + `parent_workflow` filter that
  `evaluate_children_complete` already uses. Idempotency comes free from the
  existing `backend.exists()` check in `handle_init`.
  (`lead-koto-integration`)

- **The right insertion point is CLI-level, not inside the advance loop.**
  The advance loop in `src/engine/advance.rs` is deliberately I/O-free (takes
  closures for all side effects). Adding spawn authority there would inflate
  its signature to ten arguments and bleed session concerns into the pure
  state-machine core. A new `run_batch_scheduler` call in `handle_next`
  immediately after `advance_until_stop` returns has all the inputs it needs
  (session backend, compiled template, epoch events) and keeps the engine
  pure. (`lead-koto-integration`)

- **Accepts schema needs a new `json` field type.** The `VALID_FIELD_TYPES`
  allow-list in `src/template/types.rs` is `[enum, string, number, boolean]`.
  Submitting a task list (array of objects) requires adding `json` (or
  `object`/`array`) as a valid type. It's a two-line extension that unlocks
  more than just batch spawn — any template that wants structured evidence
  benefits. (`lead-evidence-shape`)

- **`--with-data` needs `@file.json` syntax.** No `@`-prefix support exists
  today. Agents would have to shell-escape entire plans inline, which fails
  for anything beyond a few tasks. A five-line wrapper in
  `validate_with_data_payload` fixes it, mirroring `curl -d` and `gh api -f`.
  (`lead-evidence-shape`)

- **Prior art converges on three portable patterns.** Argo's flat `waits_on`
  dependency list per task; Airflow's `trigger_rule` vocabulary for failure
  routing; Make's stateless re-derivation model (compute ready set from
  current world state, don't cache it). Temporal (event-sourced daemon),
  Airflow's scheduler-daemon expansion, and Prefect's in-process mutation
  don't translate to koto's stateless CLI. GitHub Actions matrix is too
  limited (no runtime DAG growth). (`lead-prior-art`)

- **Skip-dependents is the right default failure policy.** When a child
  fails (reaches a terminal failure state), mark its direct and transitive
  dependents as skipped with a reason code, let independent branches continue,
  and report partial success. Maximizes parallelism, aligns with GH-issue
  semantics ("if issue 1 fails, issue 3 can't proceed but issue 2 might"),
  matches Argo's safe-by-default and Airflow's `all_success`. Pause-on-failure
  is too conservative for autonomous agents; fail-fast wastes parallelism;
  continue-independent ignores dependencies. Policy should be per-batch
  (declared in the evidence that submits the task list), not global or
  per-task. (`lead-failure-routing`)

- **Every batch op is a pure function of (evidence events) + (on-disk child
  state).** The scheduler is stateless. On every `koto next` call, it
  rebuilds the ready set from scratch: read the task list from the latest
  `EvidenceSubmitted` event, list children via `backend.list()`, classify
  each task (Terminal / Running / BlockedByDep / NotYetSpawned), spawn any
  unspawned task whose dependencies are terminal. No persisted cursor, no
  "scheduling state" beyond what's already on disk. Resume is trivial —
  it's just another invocation of the same pure function.
  (`lead-koto-integration`, `lead-prior-art`)

### Tensions

- **Flat batch (Reading A) vs nested batches (Reading B) for dynamic
  additions.** This is the biggest architectural question and the leads
  disagreed.

  - `lead-dynamic-additions` recommended Reading B: a running child spawns
    its own batch via `koto init --parent <running-child>`. No batch
    identity, no `waits_on` at the primitive level, nesting composes
    naturally with v0.7.0. Argues Reading A (append to same batch) has
    orphan risks, race conditions, and flattens the hierarchy.

  - `lead-evidence-shape` and `lead-koto-integration` assumed Reading A:
    the parent owns a declarative task list with sibling-level `waits_on`
    dependencies. Mid-flight additions happen by resubmitting evidence
    with more tasks — `merge_epoch_evidence` already handles the union.

  **Resolution.** Both are valid but for different shapes of work. The
  user's stated use case (GH issues with inter-issue dependencies) is
  squarely Reading A: "issue 3 depends on issues 1 and 2" is a sibling-
  level ordering, not a nesting relationship. You cannot express
  "issue 3 waits for issue 1" purely via parent-child nesting — issues
  1 and 3 are siblings, both children of the plan. Reading A is the
  primary model for #129. Reading B remains available unchanged — it's
  just v0.7.0 `koto init --parent` — for genuinely hierarchical work
  (a batch child spawns its own sub-batch). They compose, not compete.

  The orphan/race concern in Reading A is manageable with a rule: the
  state that materializes is the same state that waits on
  `children-complete`. Submission, scheduling, and waiting all happen at
  one state; the gate only passes when all declared tasks are complete
  AND the state has settled. This is the single-state fan-out pattern
  already endorsed in the koto-author template-format reference.

- **Where does the template declaration live?** Candidates from
  `lead-evidence-shape`: frontmatter field, state-level action verb, new
  gate type, or implicit reserved key. The lead recommended the state-level
  action verb (`materialize_children`) for locality and compiler
  friendliness. But `lead-koto-integration` proposed a state-level `batch`
  hook field serving the same role.

  **Resolution.** Both refer to the same thing — an optional declaration
  on `TemplateState` that names the evidence field holding the task list.
  The name is a surface detail (`materialize_children`, `batch`,
  `batch_spawn`); what matters is (a) it lives on the state, (b) it
  points at an accepts field, (c) the compiler validates the reference.
  Treat as decided in principle; name picks happen during design.

- **Do we need a `trigger_rule` vocabulary yet?** `lead-prior-art`
  proposed adopting Airflow's `trigger_rule` vocabulary (`all_success`,
  `all_done`, `none_failed`, `one_success`) per task. `lead-failure-routing`
  recommended a simpler per-batch `failure_policy` (`skip-dependents`,
  `fail-fast`, `continue-independent`, `pause-on-failure`). Per-task
  trigger rules offer finer control but more surface; per-batch policy is
  simpler.

  **Resolution.** Ship per-batch `failure_policy` in v1. Per-task
  `trigger_rule` is a reasonable follow-up once the simpler model is
  validated with real use cases. Don't gold-plate.

### Gaps

- **Atomic child-spawn sequence.** The integration lead flagged a narrow
  crash window: between `backend.append_header` and the first
  `append_event` (`WorkflowInitialized`), the child state file exists but
  has no events. `handle_next` errors on an empty event list. Downstream
  tasks are blocked until manual cleanup. Needs either atomic write (tmp +
  rename) or a repair subcommand. Easy fix but must be addressed in the
  design.

- **Forward-compat diagnosability.** `CompiledTemplate` does not use
  `deny_unknown_fields`, so a batch-hook template silently no-ops on a
  pre-batch koto binary. Either bump `format_version` to 2 when the feature
  lands (clean but coarse), or add `deny_unknown_fields` (breaking change
  risk). The design must pick.

- **Child-template path resolution relative to parent.** `handle_init`
  currently resolves `--template` relative to the working directory. When
  the scheduler spawns from a parent that's days old and the agent is in a
  different directory, how should child template paths resolve? Absolute
  paths, parent-relative, or a new template-resolution rule? The integration
  lead didn't cover this.

- **Retry semantics.** The failure-routing lead discussed `retry_failed` as
  a recovery path (parent submits evidence that re-queues a failed child
  and its skipped dependents). The mechanics — is it a new evidence action,
  a CLI flag, a template state routing decision — weren't pinned down.
  Design-level question.

- **Observability.** No lead covered how `koto status` and
  `koto workflows --children <name>` should report batch state. Probably
  just works (children show up naturally), but worth validating.

### Decisions

- **Primary model: Reading A (declarative flat batch with `waits_on`).**
  Reading B (nested via `koto init --parent`) remains available for
  hierarchical work but is not the answer to #129. The GH-issue use case
  requires sibling-level dependency ordering, which nesting cannot
  express.

- **Storage strategy: full derivation from on-disk state + event log.**
  Zero new storage, zero new event types, zero new cloud-sync paths.
  Idempotency via existing `backend.exists()`. Resume is a pure function
  of disk state.

- **Insertion point: CLI-level scheduler tick in `handle_next`.**
  Runs after `advance_until_stop` returns. The advance loop stays pure.
  New module `src/engine/batch.rs` holds the scheduler.

- **Child naming: `<parent>.<task>`.** Deterministic from parent name and
  task name. No batch-id management. Couples child name to parent name,
  but parents can't be renamed anyway.

- **Default failure policy: skip-dependents, per-batch configurable.**
  Aligns with GH-issue semantics and prior-art (Argo, Airflow). Other
  policies available as explicit opt-ins in the batch evidence.

- **CLI extension: `--with-data @file.json` prefix.** Needed for
  submitting task lists beyond a handful of entries.

- **Template extension: new `json` field type in accepts.** Needed to
  declare a state that receives a structured task array as evidence.

- **Declaration shape: state-level `batch` (or `materialize_children`)
  hook pointing at an accepts field.** Name picked during design; the
  placement and validation rules are settled.

### User Focus

The user entered `--auto` mode after confirming the core constraints
(dynamic additions required, dependencies mandatory, failure routing
deferred to exploration). No further user input in this round. Scoping
conversation emphasized:

- The feature is a blocker for shirabe's hierarchical template adoption
  (PR #67) and must solve the "SKILL.md prose spawn loop" friction.
- The task set grows at runtime — static batch declaration is
  insufficient.
- GH issues with inter-issue dependencies are the canonical use case.
- Failure routing tradeoffs need a recommendation, not just options.
- v0.7.0 primitives stay as the foundation — no regressions.

## Accumulated Understanding

Issue #129 asks for a primitive that lets a parent workflow declaratively
submit a DAG of child workflows and have koto handle spawning, ordering,
and completion. The exploration produced a coherent design sketch:

**Shape of the feature.** A template state declares a `batch` hook
pointing at one of its `accepts` fields. When evidence is submitted with
that field populated (as a JSON array of task specs), koto's new
CLI-level scheduler tick (invoked from `handle_next` after
`advance_until_stop`) parses the task list, builds a DAG, computes which
tasks are ready (all `waits_on` dependencies terminal), and spawns each
ready task by reusing the same code path as `koto init --parent`. Child
workflows are named deterministically `<parent>.<task>` so
`backend.exists()` gives free idempotency on resume. The parent's
`children-complete` gate — unchanged from v0.7.0 — waits for all
declared tasks to reach terminal state.

**Scheduler is stateless.** The scheduler holds no persistent state. On
every `koto next` call on the parent, it rebuilds the ready set from
scratch by reading the latest `EvidenceSubmitted` event (the task list)
and listing children on disk (the spawn records). There is no "what to
spawn next" cursor. Resume is the same code path as first invocation.
This follows the Make/Ninja model and fits koto's file-based,
stateless-CLI architecture.

**Dynamic additions work via evidence resubmission.** A running child
can submit additional tasks to its parent by calling
`koto next <parent> --with-data @more-tasks.json`. `merge_epoch_evidence`
unions the new tasks with the old; the scheduler re-evaluates on the
next tick; the `children-complete` gate picks up the new count. The
invariant that prevents orphaning: the state that materializes is the
same state that waits on `children-complete`. The gate can't pass until
all declared tasks — original + appended — are complete.

**Failure routing defaults to skip-dependents.** When a child reaches a
terminal failure state, its direct and transitive dependents are marked
skipped with a reason code. Independent branches continue. The batch
completes with partial success; the parent can advance via evidence
(`retry_failed` re-queues the failed chain, `proceed` treats skips as
final). Policy is per-batch via a `failure_policy` field in the
submitted evidence, defaulting to `skip-dependents`. `fail-fast`,
`continue-independent`, and `pause-on-failure` are alternative opt-ins.

**Nesting still works.** Reading B (a batch child spawns its own
batch via `koto init --parent <me>`) is unchanged from v0.7.0 and
composes cleanly. The primary batch handles sibling-level dependencies;
nested batches handle genuinely hierarchical work. They don't interact
at the primitive level — each level is its own scheduler tick on its own
parent.

**Required code changes are bounded.** Net delta: one new module
(`src/engine/batch.rs`), a refactor of `handle_init` into a reusable
helper, one new optional field in `TemplateState`, five new compiler
validation rules, two small CLI ergonomic extensions (`@file.json` prefix,
`json` accepts type), and one new call-site in `handle_next`. The
advance loop, persistence layer, session backend, and existing gates are
unchanged.

**Open gaps are small and design-time addressable.** Atomic child-spawn
window, forward-compat diagnosability, child-template path resolution,
retry mechanics, and observability reporting all need decisions but
none invalidate the sketch. They're design details, not architectural
risks.

The finding: this is a well-scoped feature with a clear implementation
path, strong composition with v0.7.0 primitives, no risky prior-art
tradeoffs, and an unambiguous default failure policy. The next step is
a design doc that turns the sketch into a specification.
