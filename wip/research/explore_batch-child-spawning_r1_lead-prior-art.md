# Lead: Prior Art in Workflow Engines

## Question

How do other workflow engines handle declarative DAG spawning with dynamic
additions and failures, and which patterns fit koto's file-based state-machine
model?

## Systems Surveyed

### 1. Temporal Child Workflows

- **Task model**: Parent imperatively calls `ExecuteChildWorkflow` in code;
  children get their own event history and parent-close policies.
- **Dependencies**: Hierarchical parent-child only; dependencies between
  siblings are expressed in parent code using futures.
- **Dynamic addition**: No declarative DAG; all child spawning is imperative
  Go/TypeScript/Java code in the parent workflow function.
- **Failure routing**: `ParentClosePolicy` (terminate, abandon, request-cancel)
  decides child fate when parent closes. Child failures bubble as exceptions
  in the parent's workflow code.
- **Persistence**: Event-sourced log with a running daemon (Temporal Service).
  Incompatible with koto's stateless CLI model.
- **Verdict**: Take the concept that child workflows are first-class. Discard
  everything else.

### 2. Airflow Dynamic Task Mapping

- **Task model**: Static task template + runtime-expanded parameter set
  via `.expand()` and `.partial()`. TaskGroups organize related work.
- **Dependencies**: `set_upstream`/`set_downstream` or `>>`/`<<` operators;
  implicit via XCom.
- **Dynamic addition**: Yes — dynamic task mapping creates N instances from
  an iterable computed at runtime. But: the scheduler has to expand them
  before execution, which assumes a running scheduler daemon.
- **Failure routing**: `trigger_rule` per task: `all_success` (default),
  `all_failed`, `all_done`, `none_failed`, `one_success`, `one_failed`,
  `none_skipped`, `always`. Rich vocabulary covers most needs.
- **Persistence**: Metadata DB + scheduler daemon polling.
- **Verdict**: `trigger_rule` vocabulary is directly portable. Dynamic
  mapping assumes a daemon, so we can't borrow the expansion mechanism.

### 3. Argo Workflows DAG Templates

- **Task model**: YAML declares tasks with `name`, `template`, `arguments`,
  `dependencies` (a flat list of task names).
- **Dependencies**: Explicit flat list per task: `dependencies: [task-a, task-b]`.
  Enhanced `depends:` supports boolean expressions over task results
  (`.AnySucceeded`, `.AllFailed`).
- **Dynamic addition**: `withParam` / `withItems` expands a task into N
  parallel instances from a JSON array. Still declared upfront, not truly
  runtime-grown.
- **Failure routing**: `continueOn` allows downstream tasks to run even when
  upstream failed; `onExit` handlers run at workflow completion.
- **Persistence**: Kubernetes custom resources; workflow state lives in the
  cluster. Not directly file-compatible but the schema is.
- **Verdict**: Closest declarative syntax to what koto needs. `dependencies:
  [a, b]` translates 1:1 to `waits_on: [a, b]`.

### 4. GitHub Actions (Jobs + Matrix + Reusable Workflows)

- **Task model**: Job definitions in YAML with `needs: [job1, job2]` and
  `strategy.matrix` for parallel expansion.
- **Dependencies**: `needs:` list per job. Very simple.
- **Dynamic addition**: No. Matrix is declared upfront from static values.
  Reusable workflows can be called, but not dynamically added to a running
  workflow.
- **Failure routing**: `strategy.fail-fast` cancels sibling matrix jobs when
  one fails. `continue-on-error` at the step or job level keeps going.
  `if: failure() | success() | always()` controls conditional execution.
- **Persistence**: GitHub-managed; restart-safe via run context.
- **Verdict**: Simple and declarative but too limited — no runtime DAG
  growth. The `needs:` syntax is a good naming precedent.

### 5. Prefect Flows and Subflows

- **Task model**: Python functions decorated `@flow` / `@task`. Subflows
  are called from within a flow function like regular function calls.
- **Dependencies**: Implicit via Python function composition. The DAG is
  lazy — tasks only materialize when executed.
- **Dynamic addition**: Yes, via in-process code (`if x: run_subflow()`).
  Requires the whole flow to run in one Python process.
- **Failure routing**: Flow state (`COMPLETED`, `FAILED`, `CRASHED`) cascades.
  Tasks support `retries`, `retry_delay_seconds`.
- **Persistence**: Prefect cloud or self-hosted server tracks state.
- **Verdict**: Lazy DAG is interesting but in-process mutation doesn't
  translate to koto's stateless CLI.

### 6. Make / Ninja (Contrast)

- **Task model**: Static rules with inputs and outputs. Dependencies implicit
  via filenames.
- **Dependencies**: Derived from file targets; no explicit list syntax.
- **Dynamic addition**: No. The whole build file is known before execution.
- **Failure routing**: Stop on first error (default). `-k` keeps going on
  unrelated targets.
- **Persistence**: Stateless. Re-running computes dirty targets from
  filesystem timestamps.
- **Verdict**: Teaches a valuable lesson — stateless re-execution with
  filesystem-derived state is a coherent model. Koto already leans this way.

## Patterns That Translate to Koto

1. **Flat `waits_on` list per task (Argo / GH Actions style).**
   Each task in the batch declares `waits_on: [task-1, task-2]`. Simple to
   serialize, trivial to cycle-check, human-readable.

2. **`trigger_rule` vocabulary (Airflow).**
   Reuse the terminology: `all_success` (default), `all_done`, `none_failed`,
   `one_success`. The vocabulary is mature and users who know Airflow will
   recognize it. Koto maps "success" to a child reaching `terminal: true`
   with no blocked substates, "failed" to reaching `done_blocked`, "skipped"
   to a new terminal-skipped marker.

3. **Append-only task submission.**
   Instead of Airflow's upfront expansion, koto accepts tasks one-batch-at-a-
   time via evidence submission. The running state's event log records each
   submission. Resume reads the log, unions all submitted tasks, and computes
   the ready set. This avoids needing a scheduler daemon.

4. **Make-style stateless re-derivation.**
   On every `koto next`, rebuild the ready set from (a) the submitted task
   list and (b) on-disk child states. Don't cache "what to spawn next" in
   the parent state file — derive it. This makes resume trivial.

## Patterns That Do NOT Translate

- **Event-sourced replay** (Temporal): requires a daemon to guarantee
  consistency and log ordering. Koto's one-CLI-call-at-a-time model can't
  replay events as a stream.
- **Upfront eager expansion** (Airflow dynamic mapping): needs a scheduler
  that walks the DAG before execution starts. Koto's stateless CLI model
  expands lazily on each `koto next`.
- **In-process DAG mutation** (Prefect): assumes the whole flow runs in one
  process. Koto can't assume that.
- **Matrix declarations upfront** (GH Actions): too limited — user explicitly
  requires dynamic additions.

## Specific Recommendations

### Task data model

```json
{
  "name": "issue-3",
  "template": "impl.md",
  "vars": {"ISSUE_NUMBER": "3"},
  "waits_on": ["issue-1", "issue-2"],
  "trigger_rule": "all_success"
}
```

Borrow Argo's flat list for `waits_on` and Airflow's `trigger_rule`
vocabulary. Make `trigger_rule` optional with `all_success` as default.

### Dependency declaration

Flat `waits_on: [name, name]` array per task. Cycle detection at
materialization time. Names must be unique within a batch.

### Dynamic addition mechanism

Append-only: a running child submits evidence to its parent that includes
additional tasks. The parent's materialization step re-reads the full task
list (original + appended) from its event log on every `koto next` and
spawns newly-ready tasks. No batch-id management needed at the CLI level —
the parent IS the batch.

### Failure routing vocabulary

Adopt Airflow's terms for the initial set:
- `all_success` (default): wait until all upstream tasks reach terminal
  non-blocked; skip if any reach `done_blocked`
- `all_done`: run when all upstream reach any terminal state (success or
  blocked)
- `none_failed`: run if no upstream reached `done_blocked`; skipped counts
  as ok
- `one_success`: run as soon as any upstream succeeds

Cover the common cases without inventing new vocabulary.

## Summary

Three patterns from prior art are worth adopting as-is: Argo's flat
`waits_on` dependency list, Airflow's `trigger_rule` vocabulary for failure
routing, and Make's stateless re-derivation model. What koto shouldn't
borrow: event-sourced replay, upfront eager expansion, and in-process DAG
mutation — all incompatible with the file-based stateless-CLI model. The
clean synthesis is: declarative task list with `waits_on` + `trigger_rule`,
append-only submission via evidence, and lazy expansion on each `koto next`.
