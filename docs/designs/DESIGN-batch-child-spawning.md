---
status: Proposed
problem: |
  Koto v0.7.0 lets a parent workflow spawn and wait for children, but the
  consumer has to run the spawn loop themselves: query which children are
  ready, spawn them, check for completion, spawn the next wave. For
  workflows where the full task set is known upfront (e.g. a plan parsed
  into a DAG of GitHub-issue children), this loop forces every consumer
  to re-implement scheduling in SKILL.md prose, which is brittle beyond
  a handful of tasks and blocks shirabe's adoption of koto for
  hierarchical templates (tsukumogami/shirabe#67). This design specifies
  a declarative alternative: the parent submits a task list as evidence,
  and koto owns materialization, dependency-ordered scheduling,
  completion detection, and failure routing end to end.
decision: |
  The parent template declares a state-level `materialize_children` hook
  that points at a required `json`-typed accepts field. When evidence is
  submitted with that field populated as a JSON array of task specs
  (name, template, vars, waits_on), a new CLI-level scheduler in
  `handle_next` (post-`advance_until_stop`) builds the DAG, classifies
  tasks from on-disk child state files, and spawns each ready task via
  a new atomic `SessionBackend::init_state_file` method. Child names are
  deterministic `<parent>.<task>`, giving free idempotency via the
  existing `backend.exists()` check. Failure routing defaults to
  `skip_dependents` via new first-class `failure: bool` and
  `skipped_marker: bool` fields on `TemplateState`; the existing
  `children-complete` gate is extended with per-child `outcome` enum,
  `success/failed/skipped/blocked` aggregates, and failure-reason
  fields. A `retry_failed` evidence action re-runs failed chains via
  the existing rewind machinery. Observability lands as optional
  `batch` sections on `koto status` and per-row metadata on
  `koto workflows --children`, sharing a `derive_batch_view` helper.
rationale: |
  The state-level hook is the only shape of four candidates that
  localizes declaration, validation, and execution to a single state
  block while matching koto's existing `gates` and `default_action`
  pattern. Disk-derived scheduling means no persistent cursor, no new
  event types, and free resume via pure re-derivation from child state
  files plus the parent's event log. Atomic `init_state_file` closes a
  pre-existing crash window in `handle_init` that affects v0.7.0 too,
  so it's independently valuable. Placing `failure_policy` on the hook
  rather than per-task or in the payload keeps parent-template
  contracts intact. Skip-dependents matches Argo and Airflow's
  safe-by-default precedent and aligns with the canonical GitHub-issue
  use case. Narrow `deny_unknown_fields` gives a better error message
  than a format_version bump with less migration surface. The whole
  feature lands in three sequential PRs (atomic init, schema layer,
  scheduler + observability) without breaking v0.7.0 behavior.
---

# DESIGN: batch-child-spawning

## Status

Proposed

## Context and Problem Statement

Issue #129 asks for declarative batch child spawning. The v0.7.0
hierarchical-workflows feature gave parents the primitives to spawn
children (`koto init --parent`), wait for them (`children-complete`
gate), and query hierarchy (`koto workflows --roots / --children /
--orphaned`). What v0.7.0 did *not* provide is a way for the parent to
hand koto a structured task list and let koto schedule the spawning
itself.

The gap manifests in consumers as spawn loops written in SKILL.md prose:
"query children state, compute next-ready from a dependency graph, spawn,
repeat." Shirabe's in-progress `work-on-plan.md` design
(tsukumogami/shirabe#67) hit this friction and stalled — natural-language
orchestration over a DAG of more than a handful of tasks is unreliable
and untestable. That PR is blocked on #129 and will be revisited once
this design ships.

The exploration surfaced a coherent implementation sketch:

- Parents submit a task list (name, template, vars, waits_on) as evidence.
- Koto's scheduler — a new CLI-level step in `handle_next` — parses the
  list on every `koto next` call, builds a DAG, computes which tasks are
  ready (all waits_on dependencies terminal), and spawns each ready task
  by reusing the same code path as `koto init --parent`.
- The `children-complete` gate (unchanged from v0.7.0) handles waiting.
- Child workflows are named `<parent>.<task>` deterministically, so the
  existing `backend.exists()` check in `handle_init` gives free
  idempotency on resume.
- Nothing new is persisted at the parent: the scheduler derives its
  working set from the latest evidence event plus on-disk child state
  files. Resume is the same code path as first invocation.
- Default failure policy is skip-dependents: a failed child marks its
  direct and transitive dependents as skipped, independent branches
  continue, and recovery happens via a `retry_failed` evidence action.
- Dynamic additions (a running child adds siblings mid-flight) work by
  evidence resubmission — `merge_epoch_evidence` unions the new tasks
  with the existing set, and the scheduler picks them up on the next
  tick.

Several decisions remain open at the design level: the atomic-write
window in `handle_init`'s header/event sequence, forward-compat
diagnosability when a batch template runs on a pre-batch koto binary,
child-template path resolution when the parent and its children spawn
from different working directories, the exact mechanics of the
`retry_failed` evidence action, and how `koto status` and
`koto workflows --children` report batch state to observers.

Exploration is documented in `wip/explore_batch-child-spawning_*.md` and
the five research files in `wip/research/`. Those artifacts are the
primary input for this design.

## Decision Drivers

- **Composition with v0.7.0 primitives.** The design must reuse
  `parent_workflow`, `koto init --parent`, and the `children-complete`
  gate without modification. New logic layers on; it doesn't replace.
  Regressing v0.7.0 behavior is out of scope.

- **Stateless CLI model.** Koto has no running daemon. Every `koto next`
  reads state from disk, acts, writes state back. The scheduler must fit
  that model — no persistent cursors, no background workers, no
  in-memory DAG that survives between calls. Every invocation is a pure
  function of the evidence log and on-disk child state.

- **Append-only state files.** The state file is JSONL, strictly
  append-only after the header. Cloud-sync incremental uploads, rewind,
  and the `expected_seq` read-check all depend on this. The design must
  not introduce header mutations or mid-file edits.

- **GitHub-issue use case is canonical.** The user's stated use case is
  implementing a plan of GitHub issues with inter-issue dependencies.
  That's sibling-level ordering, not nesting. The primary model must
  support "issue 3 waits for issues 1 and 2" where all three share a
  parent. Nested batches (a child spawns its own sub-batch) are a
  complementary capability, already supported unchanged by v0.7.0.

- **Dynamic additions are required.** The task set is not frozen at
  submission time. A running child must be able to append tasks that
  the parent's scheduler picks up on the next tick. Resume must handle
  appends correctly across crashes.

- **Failure routing should be safe-by-default.** The autonomous-agent
  use case can't afford to cascade failures silently or halt unrelated
  work on a single failure. Skip-dependents is the recommended default
  because it isolates faults, maximizes parallelism, and offers clean
  recovery. Alternatives are opt-in per batch.

- **Template compile-time validation should catch as much as possible.**
  Runtime errors halfway through a batch are much worse than
  compile-time errors at template load. The design should push
  validation forward — state shape, evidence reference, reachability —
  while accepting that per-task template paths and cyclic `waits_on`
  are necessarily runtime checks.

- **Observability through existing commands.** `koto status <name>` and
  `koto workflows --children <name>` should naturally report batch
  state without new subcommands. Batch-specific reporting (e.g. "task X
  is blocked waiting on Y") is valuable but should be additive, not a
  new command surface.

- **Backward compatibility for pre-batch templates.** Templates
  authored before this feature must continue to compile and run
  unchanged. The migration story for adding batch support to
  existing templates should be additive (a new optional field),
  not a format bump unless the forward-compat diagnosability
  problem forces one. State file compatibility is not a hard
  constraint — koto is pre-1.0 and state file schemas may evolve
  across releases — but the design should prefer additive
  `#[serde(default, skip_serializing_if = "Option::is_none")]`
  field additions where practical to minimize user disruption.

## Decisions Settled During Exploration

These decisions were evaluated during `/explore` (see
`wip/explore_batch-child-spawning_*.md` and the five research leads
in `wip/research/`) before decomposition ran, so they are treated as
constraints by the design-phase decisions in the next section. Each
is documented here in the same format as the design-phase decisions
so a future reader can see the full reasoning without re-reading the
exploration artifacts.

### Decision E1: Flat declarative batch vs nested batches

The exploration surfaced two distinct readings of "a task spawns a
sibling or grandchild mid-flight," which turned out to be different
architectural models rather than variants of the same approach.
Picking one is load-bearing for every subsequent decision because
it determines where dependency ordering lives, what the scheduler
owns, and whether `children-complete` recurses.

#### Chosen: Reading A (flat declarative batch with sibling-level `waits_on`)

The parent owns a flat task list. Dependencies are expressed as
sibling-level `waits_on` references between entries in the same
batch. The parent's `children-complete` gate waits for every declared
task to reach terminal state. Reading B (nested batches via
`koto init --parent <running-child>`) remains available unchanged
from v0.7.0 for genuinely hierarchical work, but it is not the
answer to #129.

**Rationale.** The user's canonical use case is orchestrating
GitHub-issue implementation where "issue 3 depends on issues 1 and 2
being merged first." Issues 1, 2, and 3 are siblings — they share a
parent — and the dependency is sibling-level ordering, not a nesting
relationship. There is no natural reading under Reading B where
"issue 3 is a child of issue 1" makes sense; issue 3 and issue 1 are
peers. Forcing the GH-issue case into nested batches would require
the consumer to invert the dependency graph into a tree, which
re-introduces the prose spawn loop that #129 set out to eliminate.

Reading A can express everything Reading B expresses (a child of a
batch task can start its own batch under `koto init --parent
<running-child>` unchanged), so the two compose rather than compete.
Reading B handles hierarchical decomposition; Reading A handles
same-level ordering.

#### Alternatives considered

- **Reading B (nested batches only, no sibling-level ordering).**
  Rejected because it cannot express the GH-issue use case without
  forcing users to restructure their plan. A batch of 10 issues
  with arbitrary dependencies becomes a 10-level-deep tree under
  Reading B, and `children-complete` only sees direct children —
  the outer parent never waits for grandchildren, so the
  "all 10 issues done" signal is unreachable without a separate
  consumer-maintained aggregation. Solves the wrong problem.
- **Hybrid model: parent declares a DAG of nested batches.**
  Rejected because it introduces a third layer of orchestration
  (outer parent, batch, nested batch) and inherits the downsides
  of both readings. The user's use case is flat; a hybrid model
  is premature generalization.

### Decision E2: Where batch state lives on disk

Once Reading A is chosen, the next question is where the batch's
scheduling state (task list, which tasks have been spawned, which
are waiting) physically lives on disk. The answer has direct
consequences for append-only invariants, cloud sync, resume, and
idempotency.

#### Chosen: full derivation from on-disk child state files + parent's event log

Nothing new is persisted at the parent beyond what the existing
event log already contains. The batch definition lives in an
existing `EvidenceSubmitted` event payload. Spawn records are child
state files on disk, discovered via `backend.list()` filtered by
`parent_workflow == parent_name` — the same discovery mechanism
the v0.7.0 `children-complete` gate already uses. "Which tasks are
spawned" is a pure function of (batch definition) ∩ (on-disk
children). Idempotency on resume is the existing
`backend.exists()` check.

**Rationale.** This strategy preserves append-only state file
semantics (no header mutation, no mid-file edits), requires zero
new event types, adds zero new cloud-sync paths, and needs zero
new cleanup hooks. Resume is the same code path as first
invocation — the scheduler tick is stateless and idempotent. It
is the only strategy that lets the design avoid both a new
persistence surface AND a scheduler cursor.

#### Alternatives considered

- **New `batch` section on the parent's `StateFileHeader`.**
  Rejected because the header is currently written once at init
  and never touched afterward. Mutating it to update
  `batch.spawned` would force rewriting the whole state file on
  every scheduler tick, which invalidates `read_last_seq`, breaks
  the `expected_seq` integrity check in `read_events`, and forces
  cloud sync to full-reupload on every tick. It would undo the
  append-only guarantee that makes state files safe to tail and
  sync incrementally.
- **Separate `<parent>.batch.jsonl` sidecar in the parent's session
  directory.** Rejected as workable but introduces a second log
  file that must stay in sync with the main log on rewind,
  cleanup, and cancel. `handle_rewind` and `session::handle_cleanup`
  would need to know about the sidecar. The added coupling is
  avoidable because strategy (c) already has all the information
  it needs without a new file.

### Decision E3: Where in the code the scheduler runs

Given the storage decision (nothing new persisted), the next
question is where the scheduling logic physically lives in the
codebase — which file, which function, which point in the execution
flow.

#### Chosen: CLI-level scheduler tick in `handle_next`, post-`advance_until_stop`

A new function `run_batch_scheduler` lives in `src/cli/batch.rs` and
is called from `handle_next` immediately after `advance_until_stop`
returns. It receives the session backend, the compiled template,
the final state name, the parent workflow name, and the full event
slice. It produces a `SchedulerOutcome` that attaches to the CLI
response without changing the advance loop's own return shape.

**Rationale.** `advance_until_stop` in `src/engine/advance.rs` is
deliberately I/O-free: it takes closures (`append_event`,
`evaluate_gates`, `execute_action`, etc.) for every side effect so
the state machine stays pure and testable. The spawn path needs
exactly what the advance loop lacks: the concrete `&dyn
SessionBackend`, the compile cache, and the `handle_init`
variable-resolution machinery. Threading these through yet another
closure would inflate `advance_until_stop`'s signature (already nine
parameters, already tagged `#[allow(clippy::too_many_arguments)]`)
and bleed session concerns into the pure state-machine core.

At the CLI layer, all three inputs are already in scope. Placing
the new module in `src/cli/batch.rs` (not `src/engine/`) reflects
this: the scheduler is CLI-layer orchestration over a pure engine,
not engine logic itself.

#### Alternatives considered

- **New gate type `batch-materialize` that fires on state entry.**
  Rejected because gates in koto are pure functions of
  `(state, evidence, context)` with no side effects. Giving a gate
  type write access to the session backend breaks the idempotency
  invariant that lets gates be re-run across epochs, and forces
  the blocking-condition category taxonomy to grow a category with
  no natural name ("I need to create something on first call").
- **New action/effect verb on `TemplateState`.** Rejected because
  actions are executed inside `advance_until_stop` via the
  `execute_action` closure, which has the same I/O-free constraint
  as gates. `ActionResult` only carries process-exit data, not
  "spawn children and return their session handles."
- **New step in the advance loop between transition and return.**
  Rejected for the same reason as (a) and (b): the advance loop
  has no session backend access by design.

### Decision E4: How children are named

The scheduler needs an idempotency key: a deterministic way to
answer "has this task already been spawned?" on every tick without
maintaining a cursor. The child workflow name is the natural place
for that key.

#### Chosen: deterministic `<parent>.<task>` naming

Child workflow names are computed as `<parent>.<task>` where
`<parent>` is the submitting workflow's name and `<task>` is the
value of the task entry's `name` field. The existing
`backend.exists()` check in `handle_init` becomes the idempotency
check: if the child already exists, skip; otherwise, call
`init_state_file`.

**Rationale.** The naming rule gives the scheduler a free
idempotency check with no extra state. Resume after a crash is
trivial — the scheduler classifies each task by looking up its
deterministic child name on disk. The rule couples child names to
parent names, which is fine because parents can't be renamed
anyway (the name is the session identifier). The child's existing
`parent_workflow` header field provides the back-pointer, so no
additional state is needed to trace a child to its batch.

`.` is legal in workflow names today (verified via
`validate_workflow_name`), so the rule doesn't require a workflow
name syntax change.

#### Alternatives considered

- **User-provided child workflow names.** Rejected because it
  makes idempotency the user's problem: the agent has to pick
  unique names, and collisions between separate batch invocations
  on the same parent (or across parents) are silent. There is
  also no natural back-pointer from a batch task to its child
  without an external spawn map — defeating the disk-derivation
  strategy from Decision E2.
- **Batch ID plus task index (`<batch-uuid>.<task-index>`).**
  Rejected because it requires generating and persisting a batch
  ID per submission, which adds state (where does the batch ID
  live?) and makes the names user-hostile. Integers-as-indices
  also drift if the task list is edited between submissions.

### Decision E5: Default failure-routing policy

When a child in a DAG fails, the scheduler has to decide what
happens to its dependents. There are four defensible policies, each
with different trade-offs for recovery, parallelism, and the
agent's mental model.

#### Chosen: skip-dependents as the default, per-batch configurable

A failed child's direct and transitive dependents are marked
skipped (with a reason code). Independent branches continue running.
The batch completes with a partial-success result. Recovery is
explicit: the parent submits `retry_failed` evidence to re-queue
the failed child and its skipped dependents. Alternative policies
(`continue` in v1; `fail_fast`, `pause_on_failure` deferred) are
opt-ins declared on the `materialize_children` hook.

**Rationale.** The canonical use case is "implement a batch of GH
issues with inter-issue dependencies." If issue 1 fails (tests
reject the PR), issue 3 (which depends on issue 1) cannot
legitimately proceed — running it would be either wasted work or
incorrect work. But issue 2 (independent) should still run and
merge. Skip-dependents is the only policy that does both: isolates
the failed chain, maximizes parallelism on independent branches,
and recovers cleanly via a single retry action.

Prior art corroborates: Airflow's `all_success` default, Argo
Workflows' pause-on-failure default, and GitHub Actions' `needs:`
semantics all behave this way. Pause-on-failure would freeze
independent work and demand human intervention on every failure —
too conservative for autonomous agents. Fail-fast wastes
parallelism. Continue-independent ignores dependencies entirely
and produces confusing downstream cascades.

Per-batch configurability (on the hook, not per-task) keeps the
parent template's recovery transitions written against a known
policy. Agents cannot invalidate the template's guarantees by
editing the payload.

#### Alternatives considered

- **Fail-fast.** Rejected as a default: one PR failing its tests
  would cancel every queued issue in the batch, even issues with
  no dependency on the failed one. Wasteful and contradicts the
  user's explicit preference for isolating failures to dependent
  chains. Available as an opt-in in a later version.
- **Pause-on-failure (freeze all scheduling; human decides).**
  Rejected as a default because it halts unrelated parallel work
  on every failure. Creates a "call your team" moment on every
  test flake. Valuable for all-or-nothing release workflows but
  too strong for the default. Deferred.
- **Continue-independent (run every branch regardless of
  dependency state).** Rejected because it contradicts the
  semantics of `waits_on`: if task 3 waits on task 1 and task 1
  failed, running task 3 anyway produces either a duplicate
  failure or an incorrect success. Downstream agents would have
  to post-hoc reason about which failures are induced versus
  inherent. Confusing and unsafe.

### Decision E6: CLI surface for submitting a task list

The exploration established that tasks are submitted as a JSON
array of task entries, but it did not pin down how agents actually
pass that array to `koto next`. Today `--with-data` takes a JSON
string argument; task lists beyond a few entries become
shell-escaping nightmares.

#### Chosen: `@file.json` prefix on `--with-data`

Extend `--with-data` to recognize a leading `@` as a file-reference
prefix: `koto next parent --with-data @plan-tasks.json` reads the
named file and uses its contents as the evidence payload. The 1 MB
size cap applies to the resolved content, not the argument string.

**Rationale.** This mirrors the idiom in `curl -d @file` and
`gh api -f @file`, which agents already recognize. It is a
five-line extension to `validate_with_data_payload` in
`src/cli/mod.rs` and unlocks task lists of any realistic size
without requiring the agent to embed hundreds of lines of JSON in
a shell command. The feature is also useful outside batch
spawning for any structured evidence payload.

#### Alternatives considered

- **Inline JSON strings only (no file prefix).** Rejected because
  shell-escaping a task list of 20 GH issues produces an
  unreadable command line that agents cannot reliably construct.
  Hard to debug and hard to log.
- **A new `--with-data-file` flag alongside `--with-data`.**
  Rejected as two flags where one suffices. The `@` prefix is a
  well-known convention; adding a parallel flag doubles the
  surface without new expressive power.

### Decision E7: Accepts schema supports structured JSON evidence

Koto's `accepts` schema today restricts evidence field types to
`enum`, `string`, `number`, and `boolean` (`VALID_FIELD_TYPES` in
`src/template/types.rs`). A batch task list is an array of objects,
so without schema support for structured data, a template cannot
declare `tasks: { type: ... }` at all.

#### Chosen: add a `json` field type that accepts any non-null JSON value

Extend `VALID_FIELD_TYPES` with a new `json` variant that matches
any `serde_json::Value` other than `null`. Templates can then
declare `tasks: { type: json, required: true }` and the compiler
validates the schema while the engine stores the payload intact.

**Rationale.** This is the smallest schema change that unlocks the
batch feature — one line in the allow-list, one branch in
`validate_field_type`. It's also strictly additive: no existing
template that uses `enum`/`string`/`number`/`boolean` fields is
affected. And it's reusable: any future feature that wants to
submit structured evidence benefits, not just batch spawning.

#### Alternatives considered

- **Separate `array` and `object` types with item schemas.**
  Rejected as over-engineering for v1. Per-element schema
  validation in accepts would drag "schema of schema" into the
  template compiler and overlap with what the task-list runtime
  validation already does. Easy to add later if real templates
  need it.
- **Stringly-typed JSON: declare `tasks: { type: string }` and
  parse the JSON in the materialization step.** Rejected as a
  foot-gun — the evidence is parsed by `--with-data` once and
  then re-parsed as JSON on every scheduler tick. The compiler
  cannot validate that the string is well-formed JSON, let alone
  the expected shape.

### Decision E8: Per-task `trigger_rule` vocabulary is out of scope for v1

Airflow and Argo expose per-task failure rules (`all_success`,
`all_done`, `none_failed`, `one_success`, etc.) that let each
dependent task decide for itself whether to run based on its
upstream results. The exploration's prior-art lead proposed
borrowing this vocabulary; the failure-routing lead proposed a
simpler per-batch `failure_policy`.

#### Chosen: per-batch `failure_policy` only in v1; per-task `trigger_rule` deferred

The `trigger_rule` field is reserved on task entries but any value
other than `"all_success"` is a runtime error in v1. The task list
schema carries the field so a future version can light it up
without breaking existing submissions.

**Rationale.** Shipping the simpler model first and extending later
is cheaper than shipping both and discovering which one users
actually need. Per-batch `failure_policy` covers the user's stated
GH-issue use case cleanly; the prior-art precedent for per-task
rules comes from workflow engines with much richer DAG semantics
(Airflow's task groups, Argo's `when` expressions) that koto does
not yet have. Reserving the field avoids a future schema migration.

#### Alternatives considered

- **Adopt Airflow `trigger_rule` vocabulary in v1.** Rejected as
  premature generalization. The value of `trigger_rule` emerges
  when DAGs are large and recovery paths are subtle; v1 users
  will run <100-task batches with simple recovery. Ship the
  simple model, measure, extend if needed.
- **Do not reserve the field at all.** Rejected because
  retroactively adding `trigger_rule` in v2 would then need a
  migration path. Reserving it now is free — the schema simply
  rejects non-default values at runtime.

## Considered Options

The decisions in this section were made during the design phase
(`/shirabe:design`) through parallel decision agents and cross-
validation. They build on the exploration-phase decisions above
and are the primary new contribution of this design doc.

### Decision 1: Task list schema, template hook, and compiler validation

The batch feature needs an authoring contract with three facets: the
exact shape of one task entry, the template-level hook that declares
materialization, and what the compiler validates at load time. Four
candidate shapes for the hook were considered, all grounded in the
exploration's `lead-evidence-shape` research.

#### Chosen: state-level `materialize_children` block on `TemplateState`

A new optional field on `TemplateState` alongside `gates`,
`default_action`, and `accepts`:

```yaml
states:
  plan:
    accepts:
      tasks:
        type: json
        required: true
    materialize_children:
      from_field: tasks
      failure_policy: skip_dependents   # default; also accepts 'continue'
    transitions:
      - target: await
```

**Task entry schema:**

| Field | Type | Required | Default | Purpose |
|-------|------|----------|---------|---------|
| `name` | string | yes | — | Short task name. Child workflow name is `<parent>.<name>`. Passes `validate_workflow_name()`. |
| `template` | string | yes | — | Path to child template (resolution covered in Decision 4). |
| `vars` | object (string → string) | no | `{}` | Forwarded to child's `resolve_variables()`. |
| `waits_on` | array of string | no | `[]` | Sibling task names that must complete first. |
| `trigger_rule` | string enum | no | `all_success` | Reserved for v2; only `all_success` accepted in v1. |

**Compiler validation (errors E1–E8, warnings W1–W2, runtime checks
R1–R7):**

| Rule | Level | Check |
|------|-------|-------|
| E1 | error | `from_field` is non-empty |
| E2 | error | `from_field` names a declared accepts field |
| E3 | error | Referenced field has `type: json` |
| E4 | error | Referenced field has `required: true` |
| E5 | error | Declaring state is not terminal |
| E6 | error | `failure_policy` is `skip_dependents` or `continue` |
| E7 | error | State has at least one outgoing transition |
| E8 | error | No two states reference the same `from_field` (copy-paste guard) |
| W1 | warning | A `children-complete` gate is reachable from the declaring state |
| W2 | warning | If `children-complete.name_filter` is set, it starts with `<parent>.` |
| R1–R7 | runtime | Child template compilable; vars resolve; `waits_on` is a DAG; no dangling refs; task names unique; names pass `validate_workflow_name`; no collisions with existing siblings |

**`failure_policy` placement on the hook, not in the payload.** The
policy is a parent-template contract — the `await` state's transitions
and recovery routes are written assuming a specific failure behavior.
Letting agents override it per-submission would invalidate the parent
template's promises. One batch per template in v1 makes "per-hook" and
"per-batch" equivalent.

#### Alternatives considered

- **(a) Frontmatter field `batch_spawn_state: plan`.** Rejected:
  splits state behavior across two declaration sites. Readers would
  have to cross-reference frontmatter with `states.plan` to understand
  what happens at that state.
- **(b') Same shape, named `batch` or `batch_spawn`.** Rejected on
  naming grounds: `batch` is too generic (koto may grow other batch
  features), `batch_spawn` implies koto launches agent processes. The
  feature is about children, matching `parent_workflow`,
  `children-complete`, `koto init --parent`, `koto workflows --children`.
- **(c) New gate type `batch-materialize`.** Rejected: gates in koto
  are pure functions of `(state, evidence, context)` with no side
  effects. Giving a gate type write access to the session backend
  breaks the idempotency invariant and forces the blocking-condition
  category taxonomy to grow a category with no good name.
- **(d) Implicit reserved evidence key `_spawn`.** Rejected: invisible
  contract, no compiler feedback, asymmetric with the existing `gates`
  reserved-key rule (which rejects, not dispatches).
- **`failure_policy` per-task or in-payload.** Rejected: both let
  agents invalidate parent-template guarantees at runtime.

### Decision 2: Atomic child-spawn window

`handle_init` today creates a child workflow in three sequential
backend calls: `backend.create()`, `backend.append_header()`, and
`backend.append_event()` for the initial `WorkflowInitialized`. A
crash between `append_header` and the first `append_event` leaves a
header-only state file. `backend.exists()` returns true, but
`handle_next` on that child errors on empty events. Downstream tasks
are blocked until manual cleanup. This is a critical correctness
issue for the batch scheduler, where crashes mid-materialization must
be recoverable without intervention.

#### Chosen: atomic init bundle via new `init_state_file` method on `SessionBackend`

Write the header plus the `WorkflowInitialized` event plus the initial
`Transitioned` event to a sibling `.tmp` file in the session
directory, then atomically `rename(2)` into place:

- New trait method: `SessionBackend::init_state_file(name, header,
  initial_events) -> Result<()>`
- `LocalBackend` implements it via `tempfile::NamedTempFile::persist`
  (same pattern already used by `write_manifest` in
  `src/session/local.rs:189–209`)
- `CloudBackend` wraps local + one `sync_push_state` call — a complete
  file or nothing, never partial. Reduces three S3 PUTs per child
  spawn to one as a bonus.
- `handle_init` at `src/cli/mod.rs:1112–1150` replaces its three
  backend calls with one `init_state_file` call.

**Append-only preserved.** After init, every subsequent event uses the
unchanged `append_event` (`O_APPEND`). The file on disk is byte-
identical to the old sequential path. `read_events` and `expected_seq`
semantics don't change.

**`backend.exists()` still works.** It checks the final state file
path. The temp file uses a `.koto-*.tmp` prefix that `list()` already
ignores.

**Crash-failure walkthrough** (in `wip/design_batch-child-spawning_decision_2_report.md`):
every crash point is enumerated in a table. Every case produces a
recoverable state with no operator action required. Worst case is a
leaked `.koto-*.tmp` file, invisible to `exists()` and `list()`,
cleaned up at next `backend.cleanup`.

#### Alternatives considered

- **Combine header + first event in one `append_header` write call.**
  Rejected: a single `write(2)` is not atomic; a crash mid-write can
  still leave a truncated file. Making it atomic requires
  tmp+rename anyway, and the bundle form (which also covers the
  initial `Transitioned` event) is strictly better.
- **Repair subcommand.** Rejected: pushes a correctness problem onto
  operators. Unattended batch spawning can't depend on humans running
  `koto session repair` after every crash.
- **Make `handle_init` idempotent.** Rejected: requires classifying
  every "already exists" case into sub-states and substantial new
  logic in the init path. Doesn't close the crash window, just patches
  around it. Can't recover the original `--var` flags on retry.

### Scheduler-tick ordering on first submission

`handle_next` runs `advance_until_stop` before `run_batch_scheduler`.
This creates a two-call contract on the first batch submission:

1. **Call 1.** Agent submits the task list:
   `koto next parent --with-data @tasks.json`. The advance loop
   processes the evidence, the parent transitions to
   `awaiting_children`, the `children-complete` gate evaluates
   against zero children and returns `Failed` (all tasks are
   unspawned; `evaluate_children_complete` sees no matching
   sessions). The response is `gate_blocked` with a count of 0/N
   children ready. The scheduler then runs after the advance loop
   and spawns all tasks whose `waits_on` is empty. The spawned
   count appears in the scheduler outcome attached to the
   response, but the gate result is already finalized.

2. **Call 2.** Agent invokes `koto next parent` again. The advance
   loop re-evaluates the gate, which now sees the newly-spawned
   children from Call 1. The gate output reflects actual batch
   progress.

This contract is acceptable because agents are already in a polling
loop against `koto next`, but it must be documented so consumers
don't expect Call 1's response to reflect the spawn. Two concrete
requirements:

- `evaluate_children_complete`'s "no matching children found" error
  branch at `src/cli/mod.rs:2507-2519` must be updated to handle the
  batch-state case. When the current state has a
  `materialize_children` hook and no children exist yet, it returns
  `Failed` with `total: <task_count_from_evidence>` and
  `completed: 0`, not an error. The un-spawned tasks are visible
  to the caller as `pending` entries.
- The `SchedulerOutcome::Scheduled` field in the response response
  serialization must always render, even when the outer response is
  `gate_blocked`, so agents have visible evidence that spawn
  happened on this tick.

### Decision 3: Forward-compat diagnosability

A batch-hook template compiled against a pre-batch koto binary
silently no-ops today: `CompiledTemplate` does not set
`#[serde(deny_unknown_fields)]`, so serde ignores the unknown
`materialize_children` field. The user sees their workflow "not
spawning children" with no error or warning.

#### Chosen: narrow `deny_unknown_fields` on `SourceState` only

Add the attribute to `SourceState` — the markdown parsing
intermediate — NOT to `TemplateState` — the compiled AST loaded
from cached JSON. Gate on a pre-merge audit of existing template
source files to confirm no templates rely on unknown fields as
annotations.

**Why not also on `TemplateState`.** `TemplateState` deserializes
from cached compiled JSON written by previous koto binaries. A user
upgrading their koto binary may have a compile cache with JSON
fields the new binary doesn't know about (unlikely, but possible
after a downgrade/upgrade cycle). Adding `deny_unknown_fields` to
`TemplateState` would fail the cache load with no migration path.
Keeping it scoped to `SourceState` gives the forward-compat signal
we want (authors writing new markdown templates get a clear error
on old binaries) without risking cache-load failures on
binary-version churn. The compile cache key already invalidates on
binary version, but belt-and-suspenders avoidance is cheap here.

**Why this beats a format_version bump:** neither v0.6.0 (structured
gate output) nor v0.7.0 (hierarchical workflows) bumped
`format_version`. Both relied on the existing `compile_gate`
`other =>` match arm at `src/template/compile.rs:393` to reject
unknown gate types cleanly. The current `format_version` check at
`src/template/types.rs:310` only runs when loading pre-compiled JSON;
`SourceFrontmatter` has no `format_version` field at all, so markdown
templates never get version-checked at parse time today. A working
format_version bump requires adding the field to `SourceFrontmatter`,
validating it, updating every example, updating the template format
reference, and threading it through the cache key — all to catch one
bug class.

**Serde produces a better error than a version mismatch would.**
`unknown field 'materialize_children', expected one of 'transitions',
'terminal', 'gates', 'accepts', 'integration', 'default_action'` is
localized to the offending state and implicitly names the problem.
`unsupported format version: 2` is coarser and doesn't point at the
offending field.

#### Alternatives considered

- **Bump `format_version` to 2.** Rejected: disproportionate change
  for one bug class; precedent is against format bumps for additive
  features; requires threading the field through parsing, caching,
  and documentation.
- **Compile-time warning without version bump.** Rejected: the new
  binary would warn, but users on old binaries see nothing. Doesn't
  address the actual failure mode.
- **Runtime feature detection.** Rejected: same problem — old binaries
  can't warn about a feature they don't know about.
- **Nothing + docs.** Rejected: silent no-op is exactly what we're
  trying to eliminate.

### Decision 4: Child template path resolution

When a parent submits a task list from one working directory and the
scheduler later spawns a child from a different cwd, how does the
child's `template: <path>` field resolve? Today `koto init --template`
resolves relative to the current working directory, and the original
source path is never persisted — only the cached JSON absolute path
ends up in the `WorkflowInitialized` event.

#### Chosen: parent template source directory as primary base, submitter cwd fallback

Three additive changes:

1. **`StateFileHeader` gains `template_source_dir: Option<String>`.**
   Populated at `handle_init` time by canonicalizing the `--template`
   argument's parent directory.
2. **`EventPayload::EvidenceSubmitted` gains `submitter_cwd:
   Option<String>`.** Captured from the existing
   `std::env::current_dir()` call at `src/cli/mod.rs:1312` in
   `handle_next`, which is already there but currently unused for
   template resolution.
3. **Batch scheduler resolves each task's `template` field in order:**
   (a) absolute paths pass through, (b) relative paths join against
   `template_source_dir`, (c) on `ENOENT`, fall back to
   `submitter_cwd.join(...)`, (d) error listing both attempts if
   still not found.

Both fields use `#[serde(default, skip_serializing_if =
"Option::is_none")]` for backward compat. Existing templates and
state files round-trip unchanged. Only batch-scheduler consumers
read the new fields.

**Cloud sync compatibility:** both bases (parent's template source
dir, submitter cwd) point at repo content, not koto cache. Koto
already assumes repo checkouts have stable paths across machines.

**`..` segments are permitted.** Relative paths that escape the base
directory via `..` are accepted without rejection. Koto's threat model
treats the invoking user as trusted (see Security Considerations), and
enforcing a sandbox on template reads would break legitimate use cases
like a parent template in `templates/parent.md` referencing a shared
helper at `../shared/helper.md`. Users sharing a machine across trust
boundaries should avoid running koto as a more-privileged account
against task lists produced by a less-privileged account.

#### Alternatives considered

- **Absolute paths only.** Rejected: non-portable, brittle across
  machines, forces agents to resolve paths themselves.
- **Template registry / `KOTO_TEMPLATE_ROOT` env var.** Rejected: new
  config surface, requires consistent setup across machines, doesn't
  compose with shirabe's existing template layout.
- **Relative to parent's session dir (bundled templates).** Rejected:
  forces every batch to bundle its templates, breaking the common
  case where multiple batches share one template file.
- **Name-based lookup via registry.** Rejected: biggest change, most
  new concepts, least compatible with existing template authoring.

### Decision 5: Failed/skipped representation and retry mechanics

The largest and most interconnected decision. Four sub-questions
must be answered together: what "failed" means at the protocol level;
how a skipped dependent is represented on disk; what the
`children-complete` gate output schema looks like; and what evidence
action the parent submits to retry a failed chain.

#### Chosen: first-class failure + synthetic skipped state files + extended gate + `retry_failed` evidence

**5.1 Terminal success vs terminal failure.** New optional `failure:
bool` field on `TemplateState`, meaningful only when `terminal: true`.
Default is `false` so existing templates upgrade silently.

```yaml
states:
  done:
    terminal: true
    # failure: false (default)
  done_blocked:
    terminal: true
    failure: true
```

The scheduler and gate both read `tmpl.states[state].failure`
directly. Scheduler behavior is no longer coupled to template naming
conventions.

**5.2 Skipped-child representation: synthetic state file via new
`skipped_marker: bool` terminal-state field.** When task X is skipped
because its dependency failed, the scheduler init-spawns the child
via the same atomic bundle from Decision 2 and the bundle includes a
`Transitioned → skipped_marker_state` event. The skip reason lives in
a context key (`skipped_because: <failed_task_name>`), not a new
event type.

```yaml
states:
  skipped_due_to_dep_failure:
    terminal: true
    failure: false
    skipped_marker: true
```

Preserves unified discovery via `backend.list()` — all children
(success, failure, skipped) show up the same way. No new event
types.

**5.3 Extended `children-complete` gate output schema.** Additive
changes to the existing output:

```json
{
  "total": 10,
  "completed": 7,
  "pending": 3,
  "success": 5,
  "failed": 1,
  "skipped": 2,
  "blocked": 1,
  "all_complete": false,
  "children": [
    {
      "name": "parent.issue-1",
      "state": "done",
      "complete": true,
      "outcome": "success"
    },
    {
      "name": "parent.issue-2",
      "state": "done_blocked",
      "complete": true,
      "outcome": "failure",
      "failure_mode": true
    },
    {
      "name": "parent.issue-3",
      "state": "skipped_due_to_dep_failure",
      "complete": true,
      "outcome": "skipped",
      "skipped_because": "parent.issue-2"
    },
    {
      "name": "parent.issue-5",
      "state": null,
      "complete": false,
      "outcome": "blocked",
      "blocked_by": ["parent.issue-4"]
    }
  ]
}
```

`outcome` enum values: `success | failure | skipped | pending |
blocked`. `all_complete` tightens to `pending == 0 AND blocked == 0`.
`evaluate_children_complete` must also receive `parent_events` so it
can look up the batch definition when computing `blocked` and
un-spawned task entries.

**5.4 `retry_failed` evidence action.** `retry_failed` becomes a
reserved top-level evidence key, treated like `gates` — the
existing `evidence_has_reserved_gates_key` check at
`src/cli/mod.rs:537-545` is generalized to reject any template
that declares `retry_failed` in `accepts` with a clear compile
error (so a template author can't accidentally collide with
scheduler semantics).

The parent submits:

```json
{"retry_failed": {"children": ["parent.issue-2"], "include_skipped": true}}
```

On submission, a new handler in the scheduler:

1. Transitions the parent (no rewind at parent level) from the
   post-analysis state back to `awaiting_children`.
2. Computes the transitive closure of retry-set children through
   the DAG (`include_skipped: true` extends the set to include
   dependents that were skipped because of a failure in the retry
   set).
3. For each child in the closure, calls
   `internal_rewind_to_initial(name)`. For **failed children**,
   this appends a `Rewound` event targeting the initial state,
   starting a fresh epoch (prior evidence invisible). For
   **skipped children**, the situation is different: a skipped
   child's event log is only `WorkflowInitialized` plus one
   `Transitioned → skipped_marker_state`, and
   `handle_rewind` at `src/cli/mod.rs:1198-1204` errors with
   "already at initial state, cannot rewind" on single-transition
   chains. The helper detects this case and, instead of rewinding,
   atomically deletes the synthetic skipped child state file so
   the next scheduler tick re-classifies the task as
   `NotYetSpawned` and re-materializes it from scratch. The
   delete-and-respawn path is only taken for children whose state
   files carry `skipped_marker: true` on their current state; real
   failed children always take the rewind path.
4. Appends a clearing `{"retry_failed": null}` event to the parent
   so the next scheduler tick doesn't re-rewind. The null-clearing
   idiom depends on `merge_epoch_evidence` treating null values
   as "unset"; this must be documented with a prominent code
   comment where the handler lives so future maintainers don't
   accidentally break it.

Subsequent `koto next parent` calls tick the scheduler, which sees
the rewound children as non-terminal and reclassifies them as
`Running` or `NotYetSpawned` (they exist on disk but at their
initial state), and the normal flow takes over.

**5.5 Resume walkthrough** (full detail in
`wip/design_batch-child-spawning_decision_5_report.md`, section
"Walkthrough: 10-task batch with failure + crash"): covers four
crash scenarios including mid-skip-synthesis (recovered by the
atomic init bundle from Decision 2 plus a
`repair_half_initialized_children` pre-pass), clean mid-batch
crashes (recovered by pure re-derivation from disk), mid-retry
rewind (idempotent via name-based classification), and
parent-transition crashes (single-event appends are atomic).

#### Alternatives considered

- **Reserved state-name convention** (`done_blocked` always means
  failure). Rejected: couples protocol to naming, brittle, forces
  every template to adopt the convention.
- **Enum field replacing `terminal: bool`.** Rejected: breaking schema
  change for existing templates; `failure: bool` alongside `terminal`
  is additive.
- **Parent-side skip records** (no child state file for skipped
  tasks). Rejected: two sources of truth (`backend.list()` no longer
  sees all children), abuses evidence semantics, breaks
  `koto workflows --children` for skipped tasks.
- **Marker file per skipped task.** Rejected: reintroduces sidecar
  storage explicitly rejected in the exploration's storage-strategy
  decision.
- **Whole-batch retry.** Rejected: wasteful, re-runs successful
  tasks, doesn't match the user's mental model ("retry the failed
  ones").
- **Recreate via delete+reinit.** Rejected: breaks append-only
  semantics, loses audit trail.
- **Parent-level rewind.** Rejected: clears the `retry_failed`
  evidence being acted on.

### Decision 6: Batch observability surface

`koto status <parent>` and `koto workflows --children <parent>`
should report batch state without forcing observers to duplicate DAG
computation. Today, `status` returns the parent's current state via
`derive_machine_state` and `--children` lists direct child sessions.
Neither exposes the batch task graph.

#### Chosen: extend both commands

**`koto status <parent>`** gains an optional `batch` section when the
current state has a `materialize_children` hook:

```json
{
  "workflow": "parent",
  "state": "awaiting_children",
  "is_terminal": false,
  "batch": {
    "summary": {
      "total": 10,
      "success": 5,
      "failed": 1,
      "skipped": 2,
      "pending": 1,
      "blocked": 1
    },
    "tasks": [
      {"name": "issue-1", "child": "parent.issue-1", "outcome": "success"},
      {"name": "issue-2", "child": "parent.issue-2", "outcome": "failure", "reason": "tests failed"},
      {"name": "issue-3", "child": "parent.issue-3", "outcome": "skipped", "skipped_because": "parent.issue-2"},
      {"name": "issue-4", "child": null, "outcome": "blocked", "waits_on": ["issue-2"]}
    ],
    "ready": [],
    "blocked": ["issue-4"],
    "skipped": ["issue-3"],
    "failed": ["issue-2"]
  }
}
```

**`koto workflows --children <parent>`** gains optional per-row
metadata when the parent has a batch: `task_name`, `waits_on`,
`reason_code`, `reason`, `skip_reason`.

**Source of the `reason` field.** For failed children, `reason` is
pulled from an explicit context key the child writes before entering
its terminal-failure state — by convention the key `failure_reason`.
It is NOT scraped from stderr, task output, or any raw tool output.
Template authors writing failure-state handlers are responsible for
writing a sanitized message to this context key. This keeps observer-
visible output under template-author control and prevents accidental
leakage of paths, env var values, or secrets into batch status
responses. When `failure_reason` is unset, `reason` defaults to the
failure state's name (e.g., `"done_blocked"`).

Both extensions call a shared `derive_batch_view` helper in
`src/cli/batch.rs` that reuses the scheduler's `classify_task`
and `build_dag` functions. The function is side-effect-free and
callable from read-only paths.

**Downstream fit (shirabe work-on-plan).** The consumer's hot path
collapses to one `koto status work-on-plan-42` call per poll:
`is_terminal` answers "done?", `batch.ready` answers "what's next?",
`batch.failed` plus per-task `reason` answers "what broke?", and
`batch.summary` drives progress rendering. The consumer never calls
`backend.list()` equivalents or recomputes the DAG.

#### Alternatives considered

- **Extend only `koto status`.** Rejected: observers iterating over
  many children need per-row metadata, which is structurally a
  `--children` concern.
- **Extend only `koto workflows --children`.** Rejected:
  `--children` can't show un-spawned ready tasks (they have no child
  session on disk).
- **New `koto batch status` subcommand.** Rejected: contradicts the
  "observability through existing commands" design driver; costs
  discoverability without benefit.
- **Do nothing; observers compute the DAG.** Rejected: directly
  violates the "don't force observers to duplicate DAG computation"
  constraint and makes shirabe's consumer expensive.

## Decision Outcome

The six decisions interlock into one coherent implementation. The
batch feature lands as a single atomic change set with the following
architectural thesis:

**The parent declares a DAG via the `materialize_children` hook, koto
derives the scheduling state entirely from disk on every `koto next`
tick, and failures route through first-class terminal-failure and
skipped-marker states that the existing `children-complete` gate
picks up with minimal extension.**

All six decisions are consistent with the five exploration-time
constraints (Reading A as primary, disk-derived storage, CLI-level
scheduler tick, deterministic child naming, skip-dependents default).
They share three cross-cutting PR landing requirements:

1. **One schema-layer PR.** `TemplateState` grows three new fields
   (`materialize_children`, `failure`, `skipped_marker`) and accepts
   gains a `json` field type. The narrow `deny_unknown_fields`
   attribute from Decision 3 lands in the same PR.

2. **One init-safety PR (can precede the schema PR).** Decision 2's
   `init_state_file` refactor extracts the backend-trait method,
   converts `handle_init` to use it, and is independently shippable.
   Three call sites will consume it later (regular init, scheduler
   spawn, skipped-marker synthesis).

3. **One scheduler PR.** Adds `src/cli/batch.rs`, wires
   `run_batch_scheduler` into `handle_next` after `advance_until_stop`,
   extends `evaluate_children_complete`'s output schema with the
   Decision 5 fields, adds `derive_batch_view` for Decision 6,
   implements the `retry_failed` evidence action, and updates
   `koto status` / `koto workflows --children` to expose batch
   metadata.

See Implementation Approach below for the phased landing sequence.

## Solution Architecture

### Overview

Koto gains one new engine module, four new template fields, two new
state-file / event-log fields, one new `SessionBackend` trait method,
and an extended `children-complete` gate output schema. No existing
components are removed or restructured. The advance loop, persistence
layer, and gate evaluator core are unchanged.

The new surface divides into three concerns:

1. **Declarative materialization** — the template-level hook
   (`materialize_children`), its compiler validation, and the scheduler
   tick that reads it and spawns children.
2. **Atomic child-spawn safety** — the `init_state_file` backend method
   that bundles header + initial events into one atomic `rename(2)`.
   Used by three call sites: regular `koto init`, the scheduler's
   spawn path, and the skipped-marker synthesis path.
3. **Failure and retry** — first-class terminal-failure, synthetic
   skipped-marker state files, extended gate output, `retry_failed`
   evidence action, and observability through extended `koto status`
   and `koto workflows --children` output.

### Components

```
┌─────────────────────────────────────────────────────────────┐
│  src/cli/mod.rs                                             │
│                                                             │
│  handle_next ────────┬──> advance_until_stop (unchanged)    │
│                      │                                      │
│                      └──> run_batch_scheduler  (NEW)         │
│                                    │                        │
│                                    ▼                        │
│                          src/engine/batch.rs  (NEW)          │
│                          ┌──────────────────────────┐        │
│                          │ run_batch_scheduler      │        │
│                          │   1. derive_evidence     │        │
│                          │   2. build_dag           │        │
│                          │   3. classify_task (all) │        │
│                          │   4. spawn ready tasks   │        │
│                          │   5. synthesize skips    │        │
│                          │                          │        │
│                          │ derive_batch_view        │        │
│                          │   (read-only for status) │        │
│                          │                          │        │
│                          │ handle_retry_failed      │        │
│                          │   (rewind + reclassify)  │        │
│                          └──────────────────────────┘        │
│                                    │                        │
│                                    ▼                        │
│  handle_init ──────┐      ┌────────────────────────┐        │
│  handle_status ────┼────> │ init_state_file        │        │
│  handle_workflows ─┘      │   (atomic bundle)      │        │
│                           └────────────────────────┘        │
└─────────────────────────────────────────────────────────────┘
          │
          ▼
┌─────────────────────────────────────────────────────────────┐
│  src/session/  (SessionBackend trait)                       │
│                                                             │
│  local.rs: init_state_file via tempfile::persist (NEW)      │
│  cloud.rs: delegate to local + one sync_push_state (NEW)    │
│  (mod.rs: add init_state_file method to trait)              │
└─────────────────────────────────────────────────────────────┘
          ▲
          │
┌─────────────────────────────────────────────────────────────┐
│  src/gate.rs / src/cli/mod.rs::evaluate_children_complete   │
│                                                             │
│  - Extends output schema: success, failed, skipped,         │
│    blocked, per-child outcome enum (EXTENDED)               │
│  - Reads TemplateState.failure for classification           │
│  - Takes parent_events to resolve batch definition          │
└─────────────────────────────────────────────────────────────┘
          ▲
          │
┌─────────────────────────────────────────────────────────────┐
│  src/template/                                              │
│                                                             │
│  types.rs: TemplateState gains                              │
│    - materialize_children: Option<MaterializeChildrenSpec>  │
│    - failure: bool (meaningful when terminal)               │
│    - skipped_marker: bool (synthetic-marker indicator)      │
│    - #[serde(deny_unknown_fields)] attribute                │
│                                                             │
│  types.rs: accepts field type gains `json` variant          │
│                                                             │
│  compile.rs: new validator for materialize_children         │
│    enforcing E1-E8 errors and W1-W2 warnings                │
└─────────────────────────────────────────────────────────────┘
          ▲
          │
┌─────────────────────────────────────────────────────────────┐
│  src/engine/types.rs                                        │
│                                                             │
│  StateFileHeader gains                                      │
│    template_source_dir: Option<String> (Decision 4)         │
│                                                             │
│  EventPayload::EvidenceSubmitted gains                      │
│    submitter_cwd: Option<String> (Decision 4)               │
└─────────────────────────────────────────────────────────────┘
```

### Key Interfaces

**New template fields** (`src/template/types.rs`):

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TemplateState {
    // ... existing fields ...

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub materialize_children: Option<MaterializeChildrenSpec>,

    #[serde(default, skip_serializing_if = "is_false")]
    pub failure: bool,

    #[serde(default, skip_serializing_if = "is_false")]
    pub skipped_marker: bool,
}

// Note: `deny_unknown_fields` is applied to `SourceState` (the
// markdown intermediate), NOT to `TemplateState` (which loads from
// the compile cache and must tolerate fields from newer binaries
// during version churn). See Decision 3.

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaterializeChildrenSpec {
    pub from_field: String,
    #[serde(default = "default_failure_policy")]
    pub failure_policy: FailurePolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FailurePolicy {
    SkipDependents,
    Continue,
}
```

**New session backend method** (`src/session/mod.rs`):

```rust
pub trait SessionBackend: Send + Sync {
    // ... existing methods ...

    /// Atomically create a session with the given header and initial events.
    /// Writes to a temp file and renames into place. Replaces the three-call
    /// sequence (create + append_header + append_event) used by handle_init.
    fn init_state_file(
        &self,
        name: &str,
        header: StateFileHeader,
        initial_events: Vec<Event>,
    ) -> Result<(), SessionError>;
}
```

**New CLI module** (`src/cli/batch.rs`). Placed under `src/cli/`, not
`src/engine/`, because the scheduler needs `SessionBackend` and
produces CLI response shapes — putting it in `src/engine/` would
violate the engine's I/O-free invariant:

```rust
pub enum SchedulerOutcome {
    NoBatch,
    Scheduled {
        spawned: Vec<String>,
        skipped: Vec<(String, String)>,  // (task, reason)
        already: Vec<String>,
        blocked: Vec<String>,
    },
    Error { reason: String },
}

pub enum BatchError {
    /// Task list failed validation (size, cycles, unique names, refs).
    InvalidBatchDefinition { reason: String },
    /// backend.create / init_state_file failed for a specific task.
    SpawnFailed { task: String, source: SessionError },
    /// Resolved template path doesn't exist or fails to compile.
    TemplateResolveFailed { task: String, paths_tried: Vec<String> },
    /// Backend list/read failed during classification.
    BackendError { source: SessionError },
    /// Submission exceeds hard limits (task count, edge count, depth).
    LimitExceeded { which: &'static str, limit: usize, actual: usize },
}

// BatchError maps to NextError::Batch { kind, message } for CLI
// response serialization. The NextError variant is added in the
// same PR that introduces this module.

pub fn run_batch_scheduler(
    backend: &dyn SessionBackend,
    template: &CompiledTemplate,
    current_state: &str,
    parent_name: &str,
    events: &[Event],
) -> Result<SchedulerOutcome, BatchError>;

pub fn derive_batch_view(
    backend: &dyn SessionBackend,
    template: &CompiledTemplate,
    current_state: &str,
    parent_name: &str,
    events: &[Event],
) -> Result<Option<BatchView>, BatchError>;

pub fn handle_retry_failed(
    backend: &dyn SessionBackend,
    template: &CompiledTemplate,
    parent_name: &str,
    events: &[Event],
    retry_set: &[String],
    include_skipped: bool,
) -> Result<(), BatchError>;
```

**Task list schema** (submitted as evidence JSON):

```json
{
  "tasks": [
    {
      "name": "issue-1",
      "template": "impl-issue.md",
      "vars": {"ISSUE_NUMBER": "1"},
      "waits_on": []
    },
    {
      "name": "issue-2",
      "template": "impl-issue.md",
      "vars": {"ISSUE_NUMBER": "2"},
      "waits_on": ["issue-1"]
    }
  ]
}
```

**Extended `children-complete` gate output** — see Decision 5 above
for the full JSON example.

**Extended `koto status` response** — see Decision 6 above for the
full JSON example.

### Data Flow

**Initial submission and materialization:**

1. Agent writes `plan.json` with the task list and calls
   `koto next parent --with-data @plan.json`.
2. `handle_next` reads the state file, compiles the parent template,
   runs the advance loop. The advance loop validates the evidence
   against the `accepts` schema (the `tasks` field is declared
   `type: json`, `required: true`), appends an `EvidenceSubmitted`
   event with the task list plus the captured `submitter_cwd`, and
   transitions normally.
3. After `advance_until_stop` returns, `handle_next` calls
   `run_batch_scheduler` with the final state, parent name, template,
   and full event list.
4. `run_batch_scheduler`:
   - Reads the `materialize_children` hook on the current state. If
     absent, returns `NoBatch`.
   - Extracts the task list from the latest epoch's
     `EvidenceSubmitted` event via `derive_evidence` +
     `merge_epoch_evidence`.
   - Builds the DAG and runs runtime validation (R1–R7). Cycles,
     dangling refs, and duplicate names fail the whole submission
     with `BatchError::InvalidBatchDefinition`. For dynamic
     additions where the cycle emerges only from the merge of
     original + appended tasks, the scheduler rejects the
     resubmission before any new spawn happens; already-spawned
     children from earlier submissions are untouched.
   - Classifies each task: `Terminal` (child exists and is terminal
     non-failure), `Failed` (child exists and is terminal with
     `failure: true`), `Skipped` (child exists and has
     `skipped_marker: true`), `Running` (child exists but not
     terminal), `NotYetSpawned` but `Ready` (dependencies all
     Terminal), `NotYetSpawned` but `BlockedByDep` (waits on
     non-Terminal task), or `NotYetSpawned` but `ShouldBeSkipped` (a
     dependency is Failed and `failure_policy` is `skip_dependents`).
   - For each `Ready` task, calls `init_state_file` via a helper
     refactored from `handle_init`, passing the parent's
     `template_source_dir` as the resolution base. The child name is
     `<parent>.<task.name>`.
   - For each `ShouldBeSkipped` task, synthesizes a skipped child:
     calls `init_state_file` with a header pointing at the parent
     template and an initial-events list containing
     `WorkflowInitialized` plus `Transitioned → <skipped_marker_state>`
     plus a context write (`skipped_because: <failed_task>`).
   - Returns `SchedulerOutcome::Scheduled` with per-task counts.
5. `handle_next` maps `SchedulerOutcome` into the response JSON. The
   outer response still reflects the advance loop's decision (gate
   blocked, evidence required, etc.); the scheduler result is an
   additive field for observability.

**Resume:**

On every subsequent `koto next parent` call, the same code path runs.
`run_batch_scheduler` is a pure function of (parent event log) +
(children on disk). No persisted cursor. Task status is derived fresh
every time. Crashes are recoverable because:

- `init_state_file` is atomic — no half-initialized state files.
- Children that were spawned before the crash show up in
  `backend.list()` on resume; the scheduler classifies them as
  `Running` or `Terminal` based on their event logs.
- Tasks that hadn't been spawned yet are re-computed as `Ready` or
  `BlockedByDep` from current disk state.
- `EvidenceSubmitted` events are append-only; the task list is
  reconstructed identically on every call.

**Retry:**

1. Parent is at a post-batch analysis state (e.g., `analyze_results`)
   after `children-complete` passed with some failures and skips.
2. Agent submits
   `koto next parent --with-data '{"retry_failed": {"children": ["parent.issue-2"], "include_skipped": true}}'`.
3. The advance loop sees the `retry_failed` evidence and transitions
   the parent back to `awaiting_children` via a template-defined
   route.
4. `handle_next` then calls the scheduler. Before the normal tick,
   the scheduler detects the unconsumed `retry_failed` evidence and
   runs `handle_retry_failed`:
   - Computes transitive closure of the retry set through the DAG.
   - For each child in the closure, appends a `Rewound` event to its
     state file (reusing `handle_rewind`'s machinery via an
     `internal_rewind_to_initial` helper). This creates a new epoch;
     prior evidence is invisible to the current epoch.
   - Appends a clearing `{"retry_failed": null}` evidence event to
     the parent to mark the action consumed.
5. The normal scheduler tick then runs on the now-rewound children,
   sees them as non-terminal, and the usual flow (wait, gate
   re-evaluates, terminate, etc.) resumes.

### Concurrency model

A common consumer pattern is for an orchestrator agent (e.g.,
shirabe's `work-on-plan.md`) to spawn multiple sub-agents, each
driving one child workflow in parallel. This design supports that
pattern, with one caller-side invariant.

**Parallelism where it works naturally.** Each child workflow has
its own state file. Once the scheduler has spawned N ready
children in a tick, the orchestrator can drive them concurrently —
`koto next child-1`, `koto next child-2`, ..., `koto next child-N`
all operate on distinct state files with distinct event logs. No
shared state, no races, no locks needed. This is where the
parallelism lives.

**Caller invariant: serialize scheduler ticks on the parent.**
`koto next parent` (the scheduler tick) is not reentrant-safe
against concurrent invocation. Two concurrent scheduler ticks can
race on two surfaces:

1. **Parent event log append.** Both ticks read the parent's log
   at sequence N and both try to append sequence N+1. The existing
   `expected_seq` integrity check catches this (or worse, silently
   accepts duplicates and fails the next read). This is a
   pre-existing v0.7.0 property of koto's append-only event log,
   not something introduced by batch spawning, but the batch
   scheduler inherits it.

2. **`init_state_file` TOCTOU.** Both ticks see task-4 as ready,
   both call `backend.exists("parent.task-4")` (returns false),
   both call `init_state_file`. Unix `rename(2)` has no
   "fail if exists" semantics, so the second rename silently
   overwrites the first. If the child received events between
   the two ticks, those events are lost.

The invariant consumers must enforce: **only one `koto next parent`
call may run at a time.** The typical pattern is one coordinator
task that owns the parent (submits the batch, polls for progress,
calls `retry_failed` on failures) plus N worker sub-agents that
each drive one child. The coordinator serializes parent-side
operations; the workers run in parallel on their own state files.

**When to call `koto next parent` in a parallel batch.** The
coordinator typically calls `koto next parent` (a) after
submitting the initial task list, (b) when any child reaches a
terminal state (to let the scheduler spawn newly-unblocked
tasks), and (c) periodically as a fallback poll. A simple
implementation has each worker sub-agent signal the coordinator
on child-terminal, and the coordinator debounces to one tick per
signal. A lazier implementation just polls on a timer.

**Why this design doesn't serialize at the koto layer.** Adding
file locking or `renameat2(RENAME_NOREPLACE)` at the
`SessionBackend` level is tempting but has two costs: (1) it
requires Linux-specific syscalls or portable lockfile dancing,
and (2) it papers over a caller bug rather than surfacing it.
Koto's stateless-CLI model already assumes "one process writes
to one workflow at a time" for all non-batch operations; batch
inherits that assumption unchanged rather than making batch
special. If the existing v0.7.0 assumption needs to be tightened
(e.g., for multi-machine cloud sync scenarios), that's a separate
design concern affecting all of koto, not just batch spawning.

**Concrete worked example.** A coordinator agent submits a
20-task batch. First scheduler tick spawns 5 ready tasks
(those with no `waits_on`). The coordinator starts 5 worker
sub-agents, each calling `koto next child-N` in its own loop,
driving its child to terminal state. As each child finishes,
the worker returns control to the coordinator, which calls
`koto next parent` once to re-tick the scheduler. The scheduler
classifies: 1 terminal, 4 running, 15 un-spawned; of the 15,
3 are now ready (their `waits_on` includes the finished task).
The scheduler spawns those 3 in the same tick. The coordinator
starts 3 more workers. And so on.

At peak, up to 20 worker sub-agents could be running in parallel
(minus however many are already terminal) while the coordinator
serializes its own `koto next parent` calls. This scales
linearly with task count on the child side and stays O(1) on
the parent side.

## Implementation Approach

The design lands in three sequential PRs. Each PR is independently
reviewable, testable, and shippable.

### Phase 1: Atomic init bundle (Decision 2)

Ship the atomicity fix first. It's independently valuable for
v0.7.0's existing `koto init` path and unblocks the scheduler
implementation.

**Deliverables:**
- New `SessionBackend::init_state_file` trait method in
  `src/session/mod.rs`
- `LocalBackend::init_state_file` using
  `tempfile::NamedTempFile::persist` in `src/session/local.rs`
- `CloudBackend::init_state_file` delegating to local + one
  `sync_push_state` in `src/session/cloud.rs`
- `handle_init` in `src/cli/mod.rs:1112-1150` refactored to use
  `init_state_file`
- Extract `init_child_from_parent(backend, child_name, parent_name,
  template_path, vars)` helper from `handle_init`. This is a
  meaningful refactor, not a trivial move: it must re-run
  `resolve_variables` against the child template's `variables`
  block (not the parent's), compile-cache the child template, and
  return a typed `Result` so the scheduler can surface errors
  per-task instead of calling `exit_with_error`. The existing
  `handle_init` becomes a thin wrapper that maps the `Result` to
  exit codes.
- New test: crash between `create` and `append_event` leaves no
  header-only state file
- New test: resume after simulated crash via temp-file remnants works
- New test: `init_child_from_parent` resolves child-template
  variables correctly when the parent template has different
  variables

**No user-visible change.** Existing `koto init` behavior is
preserved; only the internal sequence is tightened.

### Phase 2: Schema-layer changes (Decisions 1, 3, 4, 5)

Land all template-format and type changes. This is large and can be
split into two sub-PRs if review velocity stalls (see Mitigations).

**Sub-phase 2a — evidence and path infra (can ship first):**
- `StateFileHeader` gains `template_source_dir` (Decision 4)
- `EventPayload::EvidenceSubmitted` gains `submitter_cwd` (Decision 4)
- `handle_init` captures `template_source_dir` from the canonicalized
  `--template` argument's parent directory
- `handle_next` writes `submitter_cwd` into `EvidenceSubmitted` events
- `--with-data @file.json` prefix added to `validate_with_data_payload`
- Accepts schema `VALID_FIELD_TYPES` gains `json`
- `#[serde(deny_unknown_fields)]` on `SourceState` only (Decision 3);
  pre-merge audit confirms no source templates rely on unknown fields
- Tests for round-trip compat with v0.7.0 state files and templates

**Sub-phase 2b — template vocabulary for batch:**
- `TemplateState` gains `materialize_children`, `failure`,
  `skipped_marker` fields
- `MaterializeChildrenSpec` and `FailurePolicy` types
- `CompiledTemplate::validate` extended with E1–E8 errors and W1–W2
  warnings for `materialize_children`
- Tests for each compiler rule and each field default

**No batch scheduler yet.** This PR unlocks the template vocabulary
but doesn't actually spawn children from evidence. Authors can
declare `materialize_children` and the compiler will validate it,
but runtime is a no-op.

### Phase 3: Scheduler, retry, and observability (Decisions 5, 6)

Wire up the actual scheduler and observability.

**Deliverables:**
- New module `src/cli/batch.rs` with `run_batch_scheduler`,
  `derive_batch_view`, `handle_retry_failed`, `classify_task`,
  `build_dag`, DAG cycle detector, and `BatchError` enum
- New `NextError::Batch` variant in `src/cli/next_types.rs` with
  `From<BatchError>` impl for CLI response serialization
- `handle_next` in `src/cli/mod.rs` calls `run_batch_scheduler`
  after `advance_until_stop`
- **Update `evaluate_children_complete`'s "no children found"
  branch.** When the current state has a `materialize_children`
  hook and no children exist yet, return `Failed` with
  `total: <task_count_from_evidence>`, not an error. The function
  also takes `parent_events` as a new argument so it can resolve
  the batch definition.
- `evaluate_children_complete` extended to emit the Decision 5
  gate output schema (additive fields: `success`, `failed`,
  `skipped`, `blocked`, per-child `outcome`, `failure_mode`,
  `skipped_because`, `blocked_by`)
- `koto status` extended with optional `batch` section
- `koto workflows --children` extended with per-row batch metadata
- `retry_failed` evidence action wired through the advance loop to
  `handle_retry_failed`. The `null`-clearing-event idiom must be
  documented with a prominent code comment referencing
  `merge_epoch_evidence` semantics so future maintainers don't
  accidentally break it.
- **New `internal_rewind_to_initial(backend, name)` helper.** This
  is new machinery, not an extraction from `handle_rewind` — the
  existing rewind command rewinds one step, not back to initial
  state. The helper writes a new `Rewound` event targeting the
  child's initial state, creating a fresh epoch. Shares
  fsync/atomicity semantics with `handle_rewind`.
- New `repair_half_initialized_children` pre-pass in
  `run_batch_scheduler`: detects any child state file with a
  header but no events (shouldn't happen after Phase 1's atomic
  init bundle, but defense-in-depth for crashes in the older
  code path that ran before Phase 1 shipped). Either deletes or
  atomically re-initializes them.
- **Submission-time hard limit enforcement.** At evidence
  submission, reject task lists exceeding hard caps: 1000 tasks,
  10 `waits_on` entries per task, DAG depth of 50. Return
  `BatchError::LimitExceeded` with actual and limit values.
  These are hard rejections, not soft recommendations — easier
  to loosen limits in v2 than to tighten them after users rely
  on larger batches.
- Integration tests for: linear batch, diamond DAG, mid-flight
  append, failure with skip-dependents, `retry_failed` recovery,
  crash-resume walkthrough from Decision 5's section 5.5,
  limit-exceeded rejection
- koto-author and koto-user skill updates covering
  `materialize_children`, the extended gate output, and the
  `retry_failed` action

**End of Phase 3, the feature is complete.** The shirabe work-on-plan
design (tsukumogami/shirabe#67) can begin rewriting its spawn loop
against the new surface.

### Not in scope for v1

These items are explicitly deferred:

- Per-task `trigger_rule` vocabulary (Airflow-style granular failure
  rules). The `trigger_rule` field is reserved but only
  `all_success` is accepted.
- Multiple `materialize_children` blocks per template. E8 enforces
  one hook per template in v1. Multi-batch support is a future
  extension.
- Pause-on-failure and fail-fast failure policies. Only
  `skip_dependents` and `continue` are accepted in v1.
- Automatic retry (exponential backoff, max attempts). Retry is
  always explicit via `retry_failed` evidence.
- Cross-batch dependency edges (a task in batch A waiting on a task
  in batch B). Out of scope.
- A new `koto batch` subcommand namespace. All observability flows
  through existing `koto status` and `koto workflows --children`.

## Consequences

### Positive

- **Eliminates prose spawn loops in SKILL.md consumers.** Shirabe
  PR #67 can replace its `spawn_and_execute` state's prose
  orchestration with a declarative `materialize_children` hook.
  Future consumers (including the koto-user skill's own
  documentation) get a reliable, testable primitive.
- **Free idempotency on resume.** The disk-derivation storage
  strategy plus deterministic `<parent>.<task>` naming means the
  scheduler is idempotent by construction. No persisted cursor, no
  "replay protection" bookkeeping, no race conditions on retry.
- **First-class failure semantics.** The new `failure: bool` field
  on terminal states unblocks not just batch scheduling but any
  future feature that needs to distinguish success from failure at
  the protocol level (exit codes for `koto status`, CI gate
  decisions, downstream automation).
- **Atomic `init_state_file` fixes a pre-existing correctness bug.**
  Phase 1 lands a fix that's valuable even if batch spawning never
  shipped: v0.7.0's existing `koto init` path gets safer on crash.
- **Extended `children-complete` gate output is backward compatible.**
  Existing consumers that ignore unknown JSON fields continue to
  work. New consumers get richer information.
- **Shared `derive_batch_view` keeps `status` and `workflows --children`
  in lockstep.** One helper, two read-only call sites, no
  computation drift.

### Negative

- **Schema PR is large.** Decision 2's atomicity fix is clean, but
  Phase 2 bundles six decisions' worth of schema changes into one
  PR: three new `TemplateState` fields, a new accepts field type,
  `deny_unknown_fields`, a new header field, a new event field.
  Reviewers have a lot to track.
- **`deny_unknown_fields` is a one-time migration risk.** Any
  template source file that has ever relied on unknown fields as
  free-form annotations breaks with a clear error, but still
  breaks. The pre-merge audit must be thorough. Scoped to
  `SourceState` only, so the compile cache is unaffected.
- **Batch observability has a read-time cost.** `koto status`
  with a batch section calls `backend.list()` plus per-child reads.
  For large batches (50+ children) on cloud sync, the per-call
  cost is non-trivial. Not a regression for existing users (who
  don't run batch templates), but a new cost class worth
  monitoring.
- **Retry semantics require template authors to understand rewind.**
  `retry_failed` re-runs rewound children via the existing epoch
  mechanism. Authors writing recovery transitions need to know
  that `retry_failed` does not clear completed children from the
  gate's count.
- **Single-batch-per-template in v1 is a real constraint.** Some
  use cases (e.g., parallel independent fanouts in different parent
  states) are expressible under Reading B (nested `koto init
  --parent`) but not under the flat batch model's single-hook
  restriction. Users who hit this can fall back to Reading B
  manually until multi-batch support ships.
- **`template_source_dir` assumes the parent's template file
  location is stable.** If a user moves the parent template
  between submission and scheduler tick, relative resolution
  fails. The `submitter_cwd` fallback covers the common case but
  not all cases.

### Mitigations

- **Split Phase 2 if review velocity stalls.** The Decision 3
  `deny_unknown_fields` attribute and Decision 4's header/event
  fields can ship as separate small PRs before the main
  `materialize_children` PR if reviewers prefer smaller chunks.
- **Audit tooling for pre-merge check.** Add a grep-based
  pre-merge check (or a one-time audit script committed to
  `scripts/audit-unknown-fields.sh`) that scans all template
  fixtures for the fields covered by `deny_unknown_fields`. Run it
  once before merging Phase 2.
- **Benchmark batch observability.** Phase 3 integration tests
  should include a 50-task batch scenario to catch pathological
  `backend.list()` + per-child-read costs. If the cost is
  unacceptable, cache the classification within a single
  `koto status` call.
- **Document `retry_failed` + rewind interaction.** koto-user skill
  updates in Phase 3 include a worked example showing how
  `retry_failed` interacts with the `children-complete` gate
  output, so consumer agents build the right mental model.
- **Document the single-batch restriction.** koto-author skill
  updates note the E8 compile-time check and explain when to use
  nested batches (Reading B) instead.
- **Provide a clear error for missing child templates.** The
  scheduler's resolution fallback (parent template dir →
  submitter cwd → error) produces an error message listing both
  attempted paths, so users immediately see what was tried.

## Security Considerations

Koto is a local-user tool for personal or small-team use. Agents
submitting evidence are trusted collaborators, not anonymous
attackers. Templates are authored by developers, not end users. This
section documents the security-relevant surface added by batch child
spawning within that model.

### Trust boundaries

- **Task lists are agent-supplied.** The `--with-data @file.json`
  input carries `template` paths and `vars` values chosen by the
  submitting agent. An agent with evidence-submission access can
  point `template` at any template file readable by the invoking
  user, so evidence submission is a privilege equivalent to template
  authoring for the purpose of spawning children. Treat the agent as
  a trusted collaborator; do not use batch spawning to sandbox
  untrusted input.

- **`vars` have the same trust level as `--var` flags on `koto init`.**
  Agent-supplied `vars` are interpolated into the child's
  `resolve_variables()` and may land inside `default_action` shell
  strings. Template authors must quote variable expansions exactly
  as they would for `koto init --var` inputs. Do not place secrets
  in `vars` unless you are comfortable with them being persisted in
  the append-only event log (and, if cloud sync is enabled, uploaded
  to the sync bucket). This matches the existing persistence
  behavior of `koto init --var KEY=SECRET`; it is not a regression.

### Resource bounds

- **Task list size.** `--with-data` is capped at 1 MB of resolved
  content. The scheduler additionally enforces hard limits at
  submission time: no more than 1000 tasks per batch, no more
  than 10 `waits_on` entries per task, and DAG depth no deeper
  than 50. Submissions exceeding any limit are rejected with
  `BatchError::LimitExceeded`. Limits are hard rather than soft
  because the scheduler re-classifies all tasks plus calls
  `backend.list()` on every `koto next` tick, and cost is
  quadratic-ish in task count — easier to loosen the caps in a
  later version than to tighten them after users rely on larger
  batches.

- **Retry throttling.** `retry_failed` submissions are not rate-
  limited. Each retry appends a `Rewound` event per targeted child
  plus a clearing evidence event on the parent, so state files grow
  linearly with retries. Self-DoS by rapid retry is possible but
  limited to the invoking user's own workflow.

### Path resolution

Relative `template` paths are resolved against the parent's
canonicalized `template_source_dir` first, then against
`submitter_cwd`. The scheduler does not reject `..` segments (see
Decision 4). Koto treats the invoking user as trusted and does not
enforce a sandbox on template reads. The practical blast radius of
a path-traversal in a task entry is the same as the invoking user
manually `cat`ing the file — no privilege boundary is crossed.

### Persisted path information

- **New fields carry local absolute paths.** `StateFileHeader` gains
  `template_source_dir` and `EventPayload::EvidenceSubmitted` gains
  `submitter_cwd`. Both are absolute paths under the user's home
  directory and are persisted in state files (and, if cloud sync is
  enabled, uploaded to the sync bucket). Users sharing state files
  for debugging should be aware that directory structure will be
  visible in the shared file. This is an incremental exposure over
  v0.7.0, which already persists absolute paths to cached template
  JSON in event payloads, but the surface grows.

### Observer-visible output

- **The `reason` field is sourced from an explicit context key.**
  Documented in Decision 6 above: batch output's per-child `reason`
  comes from the `failure_reason` context key written by the child,
  not from scraped stderr or raw tool output. Template authors
  writing failure-state handlers must write a sanitized message to
  this context key. This prevents accidental leakage of paths, env
  var values, or secrets into batch status responses that observers
  (other agents, human operators) consume.

### Cloud sync concurrent submission

If two machines submit different task lists to the same parent
workflow in quick succession while cloud sync is enabled, the
losing side's write conflicts with the winning side's full-file
upload (`sync_push_state` is a PUT, not a merge). The existing
conflict-resolution path (`koto session resolve`, driven by
`src/session/version.rs::check_sync`) surfaces the divergence to
the user, but does not merge task lists automatically: whichever
version wins replaces the other, and any children the losing side
already spawned locally become orphaned relative to the merged
parent state.

This matches v0.7.0's behavior for concurrent evidence submission
generally (cloud sync doesn't merge event logs, it detects and
surfaces conflicts). Batch spawning doesn't introduce new concurrency
risks beyond this pre-existing limitation, but the failure mode is
more visible because batch workflows are more likely to run across
machines in parallel. Consumers running batched workflows on multiple
machines simultaneously should coordinate submissions externally
(e.g., by running all submissions on one coordinator machine).

### Symlink and directory assumptions

- **Session directory is assumed to be user-owned and not world-
  writable.** The atomic init bundle (Decision 2) uses
  `tempfile::NamedTempFile::persist`, which creates the temp file
  with `O_EXCL` inside the target directory before calling
  `rename(2)`. A pre-existing symlink at the final name would cause
  `rename` to overwrite the symlink itself (not its target), so
  there's no symlink-follow attack against `init_state_file`. This
  matches the pattern already used by `write_manifest` in
  `src/session/local.rs`.

### Supply chain

No new dependencies are introduced. The design uses `tempfile`
(pre-existing), `serde` (pre-existing), and the existing koto
session backend. The `init_state_file` refactor moves existing crate
usage to new call sites.
