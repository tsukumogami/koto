---
status: Planned
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
  that points at a required `tasks`-typed accepts field. When evidence is
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
  The retry path is routed through a template-declared transition with
  a discovery surface, spawned-task fields are frozen at spawn time,
  rejections carry typed discriminators via an extended `NextError`
  envelope with pre-append validation, child init uses
  `renameat2(RENAME_NOREPLACE)` on Linux with a POSIX fallback and an
  advisory flock serializes parent ticks, batch views persist past the
  batched state via a `BatchFinalized` event and terminal responses,
  and path resolution handles absent headers with per-task
  resolve-error variants so bad child templates don't abort siblings.
  Workers dispatch only on children whose `ready_to_drive` flag is
  true, `all_complete` and `needs_attention` fold `spawn_failed` into
  their aggregates, and outer `retry_failed` on a nested-batch child
  is rejected with `InvalidRetryReason::ChildIsBatchParent`.
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

Planned

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

The implementation sketch:

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

## Foundational choices

These foundational choices anchor the rest of the design. Each is
presented in the same format as the later decisions so a reader can
follow the full reasoning.

### Decision E1: Flat declarative batch vs nested batches

Two distinct readings of "a task spawns a sibling or grandchild
mid-flight" are different architectural models rather than variants
of the same approach. Picking one is load-bearing for every
subsequent decision because it determines where dependency ordering
lives, what the scheduler owns, and whether `children-complete`
recurses.

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

**Retry boundaries between outer and inner batches.** When a
Reading A batch nests a Reading B child (a
coordinator whose child is itself a batch parent), retry stays at the
level where the failure happened. `retry_failed` submitted at the
outer level on a nested-batch child rejects with
`InvalidRetryReason::ChildIsBatchParent`: the outer rewind machinery
cannot safely cascade into an inner batch's state without leaving
stale per-leaf children behind. Agents drive the inner coordinator to
retry its own failed leaves, then return to the outer parent. v1.1
may add cascading retry; v1 treats level-crossing retry as explicitly
unsupported.

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

Tasks are submitted as a JSON array of task entries, but how
agents actually pass that array to `koto next` is a separate
question. Today `--with-data` takes a JSON string argument; task
lists beyond a few entries become shell-escaping nightmares.

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

### Decision E7: Purpose-built `tasks` type replaces generic `json`

Koto's `accepts` schema today restricts evidence field types to
`enum`, `string`, `number`, and `boolean` (`VALID_FIELD_TYPES` in
`src/template/types.rs`). A batch task list is an array of objects,
so without schema support for structured data, a template cannot
declare `tasks: { type: ... }` at all.

#### Chosen: add a `tasks` field type that koto fully understands

Instead of a generic `json` type — which creates a hole in the
contract where the compiler can't validate the schema — koto adds
`type: tasks` to `VALID_FIELD_TYPES`. The compiler knows the exact
schema for `tasks` fields the same way it knows `enum` fields must
have `values`. Templates declare `tasks: { type: tasks, required:
true }` and the compiler validates the field type while the engine
validates the payload structure at runtime.

The `evidence_required` response auto-generates an `item_schema`
object on any `tasks`-typed field, describing the exact shape of
each task entry. This mirrors how `enum`-typed fields include
`values` in the response — the type implies the schema.

**Rationale.** A purpose-built type gives the compiler full
knowledge of what the field contains. There's no ambiguity about
the payload shape, no need for a separate schema mechanism, and
the `item_schema` in the response is generated automatically from
the type definition. The type is strictly additive: no existing
template that uses `enum`/`string`/`number`/`boolean` fields is
affected.

#### Alternatives considered

- **Generic `json` type that accepts any non-null JSON value.**
  Rejected: creates a hole in the contract. The compiler accepts
  the field but knows nothing about its structure. A separate
  schema mechanism would be needed to describe the payload to
  agents. The batch feature is the only use case for structured
  evidence right now — a generic escape hatch is premature.
- **Inline schema definition on a generic `array`/`object` type.**
  Rejected as over-engineering. Dragging "schema of schema" into
  the template compiler serves only one use case today. If future
  features need generic structured types, they can be added then.
- **Stringly-typed JSON: declare `tasks: { type: string }` and
  parse the JSON in the materialization step.** Rejected: double
  parsing on every scheduler tick, no compile-time validation of
  the payload shape.

### Decision E8: Per-task `trigger_rule` vocabulary is out of scope for v1

Airflow and Argo expose per-task failure rules (`all_success`,
`all_done`, `none_failed`, `one_success`, etc.) that let each
dependent task decide for itself whether to run based on its
upstream results. An alternative is a simpler per-batch
`failure_policy`.

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

The decisions in this section build on the foundational choices
above.

### Decision 1: Task list schema, template hook, and compiler validation

The batch feature needs an authoring contract with three facets: the
exact shape of one task entry, the template-level hook that declares
materialization, and what the compiler validates at load time. Four
candidate shapes for the hook were considered.

#### Chosen: state-level `materialize_children` block on `TemplateState`

A new optional field on `TemplateState` alongside `gates`,
`default_action`, and `accepts`. The canonical pattern is
**single-state fan-out**: the same state declares `accepts` (to
receive the task list), `materialize_children` (to spawn), and the
`children-complete` gate (to wait). The scheduler runs on the
state where the hook lives, which equals the state where the
advance loop is parked after processing evidence:

```yaml
states:
  plan_and_await:
    directive: |
      If you haven't submitted a task list yet, read the plan
      and submit one via `koto next parent --with-data @tasks.json`.
      Otherwise wait for children to complete.
    accepts:
      tasks:
        type: tasks
        required: true
    materialize_children:
      from_field: tasks
      failure_policy: skip_dependents   # default; also accepts 'continue'
      default_template: impl-issue.md   # compiler-validated child template
    gates:
      done:
        type: children-complete
    transitions:
      - target: summarize
        when:
          gates.done.all_complete: true
  summarize:
    terminal: true
```

**Why the single-state fan-out is required, not a preference.**
The scheduler runs on the state the advance loop is parked at
when it returns. If the hook were on a state that the advance
loop transitions *through* (e.g. `plan → await` with the hook on
`plan` and an unconditional transition), the scheduler would
never see `plan` — by the time it runs, the advance loop is at
`await`, which has no hook. Putting the hook and the gate on the
same state guarantees the advance loop parks there until all
children are terminal, giving the scheduler a stable place to
run on every tick. The compiler enforces this: E10 rejects any
state that declares `materialize_children` without also declaring
a `children-complete` gate on the same state.

**Task entry schema:**

| Field | Type | Required | Default | Purpose |
|-------|------|----------|---------|---------|
| `name` | string | yes | — | Short task name. Child workflow name is `<parent>.<name>`. Passes `validate_workflow_name()`. |
| `template` | string | no | hook's `default_template` | Path to child template (resolution covered in Decision 4). Optional when `default_template` is set on the hook. Per-task override wins. |
| `vars` | object (string → string) | no | `{}` | Forwarded to child's `resolve_variables()`. |
| `waits_on` | array of string | no | `[]` | Sibling task names that must complete first. |
| `trigger_rule` | string enum | no | `all_success` | Reserved for v2; only `all_success` accepted in v1. |

**Compiler validation (errors E1–E10, warnings W1–W5, runtime checks
R0–R9):**

| Rule | Level | Check |
|------|-------|-------|
| E1 | error | `from_field` is non-empty |
| E2 | error | `from_field` names a declared accepts field |
| E3 | error | Referenced field has `type: tasks` |
| E4 | error | Referenced field has `required: true` |
| E5 | error | Declaring state is not terminal |
| E6 | error | `failure_policy` is `skip_dependents` or `continue` |
| E7 | error | State has at least one outgoing transition |
| E8 | error | No two states reference the same `from_field` (copy-paste guard) |
| E9 | error | `default_template` is non-empty and resolves to a compilable template |
| E10 | error | State with `materialize_children` must also declare a `children-complete` gate (single-state fan-out) |
| F5 | compile | Child template referenced as `default_template` or per-task override must declare at least one terminal state with `skipped_marker: true` reachable via a scheduler-writable transition (Decision 9). Warning, not error, because batch-eligibility isn't statically knowable at child-compile time. |
| W1 | warning | A `children-complete` gate is reachable from the declaring state |
| W2 | warning | If `children-complete.name_filter` is set, it starts with `<parent>.` |
| W3 | warning | Terminal state whose name contains "block", "fail", or "error" lacks `failure: true` |
| W4 | warning | State with `materialize_children` routes only on `all_complete: true` without a second transition handling `any_failed > 0` or `any_skipped > 0`. Failed or skipped children would route to the success branch silently. (Decision 9) |
| W5 | warning | Terminal state with `failure: true` has no path that writes `failure_reason` to context. The batch view's per-child `reason` field falls back to the state name. (Decision 13) |
| R0 | runtime | Task list is non-empty (`tasks.len() >= 1`). |
| R1 | runtime | **Per-task:** child template resolvable and compilable. Failures surface in `SchedulerOutcome.errored` and as `BatchTaskView.outcome: spawn_failed`; siblings continue. |
| R2 | runtime | Per-task: `vars` resolve against the child template. Per-task failures in `SchedulerOutcome.errored`. |
| R3 | runtime | **Whole-submission:** `waits_on` is a DAG (no cycles). Rejects pre-append with `InvalidBatchReason::Cycle`. |
| R4 | runtime | **Whole-submission:** `waits_on` has no dangling references. |
| R5 | runtime | **Whole-submission:** task names are unique within the submission. |
| R6 | runtime | Hard limits: `tasks.len() <= 1000`, `waits_on.len() <= 10` per task, DAG depth `<= 50` where depth is the node count along the longest root-to-leaf path. Per-limit `InvalidBatchReason` variants. |
| R7 | runtime | No collisions with existing siblings (enforced at init via `renameat2(RENAME_NOREPLACE)`; see Decision 12). |
| R8 | runtime | **Spawn-time immutability.** For each task whose child `<parent>.<task.name>` already exists on disk, the submitted entry's `template`, `vars`, and `waits_on` must match the `spawn_entry` snapshot recorded on the child's `WorkflowInitialized` event. Mismatches reject pre-append with `InvalidBatchReason::SpawnedTaskMutated`. (Decision 10) |
| R9 | runtime | Task name matches `^[A-Za-z0-9_-]+$`, 1-64 chars, not in reserved set `{retry_failed, cancel_tasks}`. |

All R0-R9 checks are **pre-append** (see Decision 11): validation runs as a pure function of the submitted payload before any `EvidenceSubmitted` write. Rejected submissions leave zero state on the parent's event log.

**`failure_policy` placement on the hook, not in the payload.** The
policy is a parent-template contract — the `await` state's transitions
and recovery routes are written assuming a specific failure behavior.
Letting agents override it per-submission would invalidate the parent
template's promises. One batch per template in v1 makes "per-hook" and
"per-batch" equivalent.

**Related decisions.** Decision 11 commits all runtime rules to
pre-append execution and pins R0 (non-empty task list) and R9 (name
regex + reserved-name set). Decision 10 adds R8 (spawn-time
immutability) and defines "union by name" precisely: un-spawned
entries are last-write-wins, spawned entries are locked against the
recorded `spawn_entry`. Decision 9 adds W4 (transitions that swallow
failure) and F5 (batch-eligible child templates must declare a
scheduler-reachable `skipped_marker` state). Decision 13 adds W5
(`failure: true` states that produce no `failure_reason`). Decision
14 makes R1 per-task so a single bad child template surfaces as
`BatchTaskView.outcome: spawn_failed` rather than aborting the
submission, and pins R6's depth definition to node count along the
longest root-to-leaf path.

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

**Crash-failure analysis.** Every crash point between the three
original backend calls (`create`, `append_header`, `append_event`) is
covered by the atomic bundle: a crash before `rename(2)` leaves only
a `.koto-*.tmp` file (invisible to `exists()` and `list()`, cleaned
up at next `backend.cleanup` or by the tempfile sweep from Decision
12); a crash after `rename(2)` produces a complete state file. Every
case produces a recoverable state with no operator action required.

**Related decisions.** Decision 10 extends the `WorkflowInitialized`
event with an optional `spawn_entry` snapshot capturing the exact
task entry the scheduler used (template, vars, waits_on in canonical
form). The field is additive and marked
`#[serde(default, skip_serializing_if = "Option::is_none")]`, so
children predating this field deserialize cleanly. Decision 12
strengthens the final-rename step: Linux uses
`renameat2(RENAME_NOREPLACE)` for a single-syscall fail-if-exists
check, and other Unixes use POSIX `link()` followed by `unlink()` on
the tempfile. The tempfile + rename bundle itself is unchanged. The
sequence is now "atomic create-only" rather than "atomic replace,"
which closes the `init_state_file` TOCTOU window between two
concurrent ticks seeing the same ready task.

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

**Handling corner cases (see Decision 14).** The resolution order
above covers present-and-valid `template_source_dir` cleanly, but two
corners need explicit handling:

- **Absent `template_source_dir`** (state files predating this field)
  skips step (b) entirely. The scheduler falls straight through to
  step (c) and emits `SchedulerWarning::MissingTemplateSourceDir`
  once per tick.
- **Present but stale `template_source_dir`** (the directory doesn't
  exist on the current machine, e.g., after a cross-home-layout
  migration) emits `SchedulerWarning::StaleTemplateSourceDir` before
  falling through to `submitter_cwd`.

Both warnings surface on `SchedulerOutcome.warnings` as a sibling to
`errored` (see Decision 11). Agents see actionable diagnostics
rather than a generic "file not found."

Decision 14 also splits `BatchError::TemplateResolveFailed` into two
variants so agents can distinguish path failure from compile failure:

- `TemplateNotFound { task, paths_tried }` — every configured base was
  tried; the file exists at none of them.
- `TemplateCompileFailed { task, path, compile_error }` — the file was
  found and read, but template compilation failed.

Both variants surface per-task through `SchedulerOutcome.errored`,
never as a whole-submission abort (R1 is per-task per Decision 14).

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

#### Chosen: first-class failure + skip-marker state files + extended gate + `retry_failed` evidence

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

**5.2 Skipped-child representation: child's real template plus runtime
reclassification.** When task X is skipped because its dependency
failed, the scheduler init-spawns the child via the same atomic bundle
from Decision 2 using the **child's real template** (not a synthetic
mini-template), and the initial events route the child directly to a
state where `skipped_marker: true`. The skip reason lives in a context
key (`skipped_because: <failed_task_name>`), not a new event type.

```yaml
states:
  skipped_due_to_dep_failure:
    terminal: true
    failure: false
    skipped_marker: true
```

Preserves unified discovery via `backend.list()` — all children
(success, failure, skipped) show up the same way. No new event types.
No hidden synthetic-template artifact.

**Why runtime reclassification rather than synthetic templates (see
Decision 9).** An earlier approach used a hardcoded synthetic
mini-template for skipped children. That approach fails two cases:

1. A real-template running child whose upstream flipped to failure
   had no legal transition into a *different* template's
   `skipped_marker` state (cross-template transitions are not an
   engine feature and adding them is out of scope).
2. Stale skip markers (a child was skipped because B failed; later B
   was retried and succeeded) could not be cleared without an
   explicit "refresh" primitive that costs a new event type.

Runtime reclassification replaces both paths with one sweep. On every
scheduler tick:

- **Skip markers on the real template are re-evaluated.** If any
  `waits_on` dependency still reports `failure`, the marker stays. If
  all dependencies are now `success` or in progress, the marker is
  stale — the scheduler deletes the child state file and respawns the
  task with its real template at initial state, per Decision 2's
  atomic init.
- **Real-template running children whose upstream flips to failure
  are delete-and-respawned as skip markers.** The child's in-progress
  work is invalidated (which is correct: its dependency failed) and
  the child is respawned directly into its `skipped_marker` terminal.

This is why Decision 1's F5 warning fires: every batch-eligible child
template must declare a reachable `skipped_marker: true` state so the
scheduler has somewhere to route.

The delete-and-respawn path is keyed on `skipped_marker: true` on the
child's current state — never on a hidden marker file or template
hash. Decision 10's `spawn_entry` snapshot on `WorkflowInitialized` is
preserved across delete-and-respawn: the scheduler uses the most
recently accepted task entry, which under Decision 10's rules is
stable (R8 locks `spawn_entry` against the submitted payload).

**`spawn_entry` on respawn and `ready_to_drive` dispatch gate.**
Three clarifications close the reclassification race:

1. **`spawn_entry` on respawn.** When the scheduler delete-and-respawns
   a child, the new state file's `WorkflowInitialized.spawn_entry`
   snapshot is the CURRENT submission's entry for that task name — the
   version of the entry that caused the respawn to be valid under R8.
   Prior entries survive only as history in the respawned child's
   event log under the bumped epoch. Agents reading `spawn_entry` on
   disk after a respawn see the entry they most recently submitted,
   not a pre-respawn value.

2. **`ready_to_drive` on every materialized child.** The
   `materialized_children` ledger is not a dispatch signal on its own:
   a respawned child is present on disk (and therefore in the ledger)
   before its `waits_on` dependencies complete. Workers MUST dispatch
   only on children whose `ready_to_drive` flag is `true`:
   `ready_to_drive == (child is not terminal) AND (every waits_on dep
   from the child's current `spawn_entry` is terminal-success)`. The
   flag is derived fresh each tick. See Decision 10's `MaterializedChild`
   shape for the serialized field.

3. **Retry-induced respawns commit atomically per tick.** On the retry
   tick, every respawned dependent's new state file is written in a
   single atomic batch at the end of the tick. The `materialized_children`
   ledger returned to the agent reflects the post-commit view: no
   respawned child is reported with `ready_to_drive: true` until its
   `waits_on` ancestors' outcomes have actually settled.

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
blocked | spawn_failed`. `all_complete` requires `pending == 0 AND
blocked == 0 AND spawn_failed == 0`: a batch with never-spawned tasks
must not silently transition to success.
`evaluate_children_complete` must also receive `parent_events` so it
can look up the batch definition when computing `blocked` and
un-spawned task entries.

**Derived route booleans (see Decision 9).** `all_complete` alone is
not a useful route guard for failed batches because it is `true` even
when every child failed or was skipped — that would ship the agent
straight to a summary state with no retry window. The gate output
exposes derived booleans alongside `all_complete`:

| Field | Definition |
|-------|------------|
| `all_complete` | `pending == 0 AND blocked == 0 AND spawn_failed == 0` |
| `all_success` | `all_complete AND failed == 0 AND skipped == 0 AND spawn_failed == 0` |
| `any_failed` | `failed > 0` |
| `any_skipped` | `skipped > 0` |
| `any_spawn_failed` | `spawn_failed > 0` |
| `needs_attention` | `any_failed > 0 OR any_skipped > 0 OR any_spawn_failed > 0` |

Templates route on these booleans via equality-based when-clauses. W4
(Decision 9) fires at compile time if a `materialize_children` state
routes only on `all_complete: true` without a second transition
guarding `any_failed` or `needs_attention`.

Decision 11 adds `spawn_failed` to the per-child `outcome` enum so
per-task spawn errors (bad child template, per-task R1/R2 failures)
surface through the gate output the same way real failures do.
Per-child entries may carry a `spawn_error` object with
`SpawnErrorKind` and optional `paths_tried` for diagnosis.

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

**Authoritative retry mechanics (see Decision 9).**
`handle_retry_failed` never appends `Transitioned` to the parent. The
advance loop fires a template-declared transition on
`when: evidence.retry_failed: present`.

The canonical sequence (Decision 9, with Decision 12's cloud-sync
reordering applied when `CloudBackend` is active):

```
a. validate R10 (retry set: non-empty; each named child exists and
   has outcome `failure` or `skipped`; atomicity — all-or-nothing)
b. append EvidenceSubmitted { retry_failed: <payload> } to parent log
c. append EvidenceSubmitted { retry_failed: null } clearing event to parent
   (under CloudBackend, sync_push_state the parent log here; on push
   failure, return; no child writes have happened yet)
d. for each child in the downward closure of the retry set:
   - if outcome is `failure`: append Rewound targeting the initial state
   - if outcome is `skipped` (current state has `skipped_marker: true`):
     delete-and-respawn — the scheduler will re-materialize on the
     next tick based on current dependency outcomes
e. advance loop runs; it reads the unmerged submission evidence and
   fires the template transition on evidence.retry_failed: present,
   routing the parent back to the awaiting state
```

Step (c) pushes the clearing event before child writes so a
cloud-sync race that loses the parent log branch cannot leave phantom
`Rewound` events on children referencing a retry that the resolved
parent log never records (Decision 12 Q6).

**Why the clearing event lands before child writes under cloud sync.**
A crash between steps (c) and (d) leaves the parent with
`retry_failed: null` in merged evidence and the children untouched.
Re-running `handle_retry_failed` sees `no_retry_in_progress` and
rejects cleanly. The user resubmits. This converts Decision 9's
"transparent re-apply on crash" semantics into "user re-submits on
crash," which is the trade-off Decision 12 accepts to eliminate
phantom child epochs.

**Closure direction is downward.** The retry set extends to
*dependents* of the named children, not ancestors. `include_skipped:
true` (the default) propagates the rewind to skipped dependents of
the retry set. To retry an ancestor, the user names the ancestor
explicitly. The principle: the user names what they mean.

**Edge cases (Decision 9).** Every retry corner has explicit
behavior:

| Edge | Behavior |
|------|----------|
| Double `retry_failed` without intervening tick | Advisory flock serializes; the loser observes `BatchError::ConcurrentTick`. `InvalidRetryReason::RetryAlreadyInProgress` is reserved for non-flocked futures. |
| Retry on a running child (outcome `pending`) | Rejected with `ChildNotEligible`; no rewinds written |
| Retry on a successful child | Rejected with `ChildNotEligible` |
| Retry on an unknown child name | Rejected with `InvalidRetryReason::UnknownChildren { children }` |
| Retry on a `spawn_failed` child | Accepted as retry-respawn: next tick re-attempts `init_state_file` using the current submission's entry for that name |
| Retry naming a child that is itself a batch parent | Rejected with `InvalidRetryReason::ChildIsBatchParent`; cross-level retry is unsupported in v1 |
| Mixed retry set (some retryable, some not) | All-or-nothing: whole submission rejected |
| Mixed payload (`retry_failed` + other evidence keys) | Rejected with `InvalidRetryReason::MixedWithOtherEvidence`; `extra_fields` names the offending keys |
| Stale skip markers after partial retry | Decision 9 Part 5: runtime reclassification deletes stale markers on next tick |
| Concurrent `retry_failed` from two callers | Decision 12's advisory flock serializes parent ticks; loser gets `BatchError::ConcurrentTick` |
| Target child mid-respawn during a concurrent submission | Feedback entry carries `EntryOutcome::Respawning`; submission is not rejected, but the next tick re-evaluates R8 against the newly-committed `spawn_entry` |

**Discovery: `reserved_actions` response field.** Decision 9 Part 3
adds a top-level `reserved_actions` array to responses where
`any_failed`, `any_skipped`, or `any_spawn_failed` is true. Each entry carries the action
name, a payload schema, an `applies_to` list of currently-retryable
children, and a ready-to-run `invocation` string. Agents without the
koto-user skill can construct a correct retry submission by reading
this field. `reserved_actions` is synthesized by `handle_next` after
gate evaluation; it does not flow through `expects.fields` because
reserved actions bypass the advance loop's evidence validator.

**Interception point.** `retry_failed` is intercepted in `handle_next`
BEFORE `advance_until_stop` runs. Only the child side effects and
parent evidence writes happen in `handle_retry_failed`; the actual
parent transition fires in the subsequent advance-loop pass, driven
by the template. Pre-Decision-12, this was "intercept and then let the
advance loop read merged evidence." Post-Decision-12, step (c)'s
clearing-event-first ordering means the advance loop reads the
clearing value; the `evidence.<field>: present` matcher must evaluate
the un-merged submission payload's presence to fire the transition.

Subsequent `koto next parent` calls tick the scheduler, which sees
rewound children as non-terminal and reclassifies them as `Running`
or `NotYetSpawned` (they exist on disk but at their initial state),
and the normal flow takes over.

**5.5 Resume under four crash scenarios.**

- **Mid-skip-synthesis crash.** Recovered by the atomic init bundle
  from Decision 2 plus a `repair_half_initialized_children` pre-pass
  that sweeps any leftover `.koto-*.tmp` files.
- **Clean mid-batch crash.** Recovered by pure re-derivation from
  disk. The scheduler reclassifies every task on the next tick using
  `backend.list()` plus the event log; children that were spawned
  show up terminal or running, and un-spawned tasks recompute as
  `Ready` or `BlockedByDep`.
- **Mid-retry crash.** Under Decision 12's push-parent-first
  ordering, a crash after the clearing event but before child writes
  leaves children untouched. On resume, `handle_retry_failed` sees
  `no_retry_in_progress` and rejects; the user resubmits the retry.
- **Parent-transition crash.** Single-event appends are atomic, so
  the parent state file is either at the pre-transition state or at
  the post-transition state on resume; never a partial write.

Skip markers live on the child's real template at a
`skipped_marker: true` state, so resume classifies them via the same
"current state has `skipped_marker: true`" predicate observers use
(Decision 9). Runtime reclassification sweeps every tick, making
stale markers a non-issue on resume.

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
  storage explicitly rejected in Decision E2 on where batch state
  lives on disk.
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

**Extensions for post-completion reads (see Decision 13).** The
`batch` section drops the moment the parent leaves the batched
state, but consumers asked to write summary directives or diagnose
failures need the view to survive that transition. Decision 13 adds
four additive extensions:

- **`BatchFinalized` event.** When `children-complete` first evaluates
  `all_complete: true` on a state with `materialize_children`, the
  advance loop appends a `BatchFinalized` event to the parent log
  carrying the final `BatchView` snapshot. Subsequent `koto status
  <parent>` calls replay from this event when the parent's current
  state has no hook, labeling the section `batch.phase: "final"` (vs
  `batch.phase: "current"` for live views from Decision 6). Re-
  entering a batched state (for example after `retry_failed`) appends
  a new `BatchFinalized` event on the next pass, superseding the
  prior one.

- **`batch_final_view` on terminal and post-batch `done` responses.**
  The `done` response gains an optional `batch_final_view` field
  (suppressed via `skip_serializing_if`) when the parent log contains
  at least one `BatchFinalized` event. The payload matches the
  `batch` section shape. This eliminates the two-call pattern
  (`koto next` + `koto status`) on the terminal tick.

- **`synthetic: true` marker.** `koto status <child>` and the per-row
  shape in `koto workflows --children <parent>` emit an explicit
  `synthetic: true` field when the child's current state has
  `skipped_marker: true`. The predicate is derived from state, not
  from a template hash or sidecar flag, so it works under Decision
  9's runtime-reclassification model. `koto next <synthetic-child>`
  returns an immediate terminal `done` response with a directive
  explaining the skip rather than an error or silent blank.

- **`skipped_because_chain` array.** Retain the singular
  `skipped_because` field (direct blocker). Add
  `skipped_because_chain: [<direct>, ..., <root-failure>]` alongside
  it, walking upstream through `waits_on` to the first failed (non-
  skipped) ancestor. Diamond scenarios pick the shortest chain,
  tie-breaking alphabetically for determinism.

- **`reason_source` projection.** When the batch view's per-child
  `reason` engages the `failure_reason` context key, emit
  `reason_source: "failure_reason"`. When it falls back to the state
  name, emit `reason_source: "state_name"`. Omitted for successful
  or non-terminal children. Pairs with compile warning W5 so
  template authors catch the opaque-reason case before ship.

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

### Decision 7: How the agent knows what to do when children are spawned

When the parent's `children-complete` gate blocks with spawned
children, the `gate_blocked` response carries machine-readable data
(`scheduler.spawned_this_tick`, `scheduler.materialized_children`,
`blocking_conditions[].output`) but nothing
tells the agent in plain English what to do — drive children in
parallel? delegate? wait for CI? poll? The question is whether
this needs a new engine feature or whether the existing directive
mechanism handles it.

#### Chosen: existing `directive` + `details` mechanism (no new features)

The `GateBlocked` response already includes the `directive` field
— template-authored prose from the state's markdown body section.
This directive appears on every gate-blocked tick. The existing
mechanism provides everything needed:

1. **Directive prose (always present).** The template author writes
   batch-aware text in the state's markdown body:
   ```markdown
   ## plan_and_await

   Children have been spawned from your task list. For each child
   in the `scheduler.spawned_this_tick` list below, start a sub-agent that
   drives it via `koto next <child-name>`. Re-check the parent
   with `koto next parent-42` after each child completes to spawn
   newly-unblocked tasks.
   ```

2. **Details text (first visit).** The `<!-- details -->` marker
   lets authors include extended batch instructions on first
   visit — how to interpret scheduler output, when to re-tick the
   parent, whether to run children in parallel — that disappear on
   repeat visits to reduce noise.

3. **Structured data alongside prose.** The `scheduler` field
   carries `spawned_this_tick`/`materialized_children`/`blocked`/
   `skipped` lists as machine-readable JSON, and
   `blocking_conditions[].output` carries per-child status with
   outcome enums. The agent reads the prose to
   understand the intent; it reads the JSON to know the specifics.

4. **Skill-level documentation.** The koto-user skill's
   response-shapes reference documents the batch response pattern,
   teaching agents how to interpret the scheduler outcome alongside
   the directive. Agents without the skill can still read the
   directive prose.

**No code changes needed.** The guidance mechanism is a template
authoring concern and the authoring tools already exist.

**Concrete response example.** After submitting a batch and one
child completing, `koto next parent-42` returns:

```json
{
  "action": "gate_blocked",
  "state": "plan_and_await",
  "directive": "Children have been spawned from your task list. For each child in the scheduler.spawned_this_tick list below, start a sub-agent that drives it via `koto next <child-name>`. Re-check the parent after each child completes.",
  "details": "The batch scheduler runs on every `koto next parent-42` call. It spawns tasks whose dependencies are all terminal. Drive children in parallel when possible. Each child is an independent workflow with its own state file.",
  "blocking_conditions": [{
    "name": "done",
    "type": "children-complete",
    "category": "temporal",
    "output": {
      "total": 10, "completed": 1, "pending": 9,
      "success": 1, "failed": 0, "skipped": 0, "blocked": 6,
      "all_complete": false,
      "children": [
        {"name": "parent-42.issue-1", "outcome": "success"},
        {"name": "parent-42.issue-2", "outcome": "pending"},
        {"name": "parent-42.issue-4", "outcome": "blocked", "blocked_by": ["parent-42.issue-2"]}
      ]
    }
  }],
  "scheduler": {
    "spawned_this_tick": ["parent-42.issue-4"],
    "already": ["parent-42.issue-1", "parent-42.issue-2", "parent-42.issue-3"],
    "blocked": ["parent-42.issue-5", "parent-42.issue-6", "..."]
  }
}
```

The directive and details text are entirely template-authored. A
different template might say "wait for CI to complete each child"
or "delegate to team members via the task system." koto doesn't
prescribe; the template author does.

#### Alternatives considered

- **Hardcoded engine-generated `suggested_action`.** Rejected:
  generic prose can't fit every use case (drive vs delegate vs
  wait). Engine-generated behavioral suggestions contradict koto's
  "contract layer" philosophy — the engine doesn't know whether
  children should run in parallel, sequentially, or be delegated.
- **Template-authored `batch_directive` on `materialize_children`
  hook.** Rejected as redundant: the state already has a
  directive. Adding a second prose field creates unclear
  precedence and two places to maintain guidance text.
- **Extend directive with batch-aware interpolation
  (`{{#if batch.spawned}}`, `{{batch.spawned_names}}`).** Rejected:
  introduces a template engine dependency (Handlebars/Tera) and
  conditional syntax into a system where directives are currently
  plain text. Runtime state (child names, counts) is already in
  `scheduler` and `blocking_conditions` — duplicating it in prose
  adds no decision-relevant information.
- **Structured `action_hint` on gate output.** Rejected: new
  concept in koto's gate output vocabulary that overlaps with the
  existing directive. Would be a third place to write agent
  guidance (alongside directive and details) with no clear
  precedence.

### Decision 8: How the agent discovers the task entry schema and child template path

Two gaps need to be closed. First, the `materialize_children` hook
says nothing about what child template to use — the path is
hardcoded in directive prose, not compile-time validated, and not
discoverable from koto's structured response. Second, while the
`tasks` field type tells the compiler what shape to expect, the
agent needs the schema surfaced in the response to know
what each entry requires — `name`, `template`, `vars`, `waits_on`.

#### Chosen: `default_template` on the hook + koto-generated `item_schema` in the response

Two changes close both gaps. The template author writes only
`default_template`; koto generates the rest.

**1. `default_template` (required) on `MaterializeChildrenSpec`.**
The compiler validates the path resolves at compile time. Task entries
can omit `template` (the default is used); per-task overrides win.

```yaml
materialize_children:
  from_field: tasks
  failure_policy: skip_dependents
  default_template: impl-issue.md    # compiler-validated
```

**2. koto auto-generates `item_schema` in the `expects` response.**
The `tasks` type implies a fixed schema — koto knows exactly what
shape a task entry has because the batch feature defines it. When
`derive_expects` in `src/cli/next_types.rs` encounters a `tasks`-typed
field, it includes a structured `item_schema` object in the response,
the same way `enum`-typed fields include `values`:

```json
{
  "expects": {
    "fields": {
      "tasks": {
        "type": "tasks",
        "required": true,
        "item_schema": {
          "name": { "type": "string", "required": true, "description": "Child workflow short name" },
          "template": { "type": "string", "required": false, "default": "impl-issue.md" },
          "vars": { "type": "object", "required": false },
          "waits_on": { "type": "array", "required": false, "default": [] },
          "trigger_rule": { "type": "string", "required": false, "default": "all_success" }
        }
      }
    }
  }
}
```

The `template.default` value is derived from `default_template` on
the hook — the only template-authored input. Everything else is
generated by koto because it's invariant across all batch templates.
The template author writes no description, no schema annotation, no
prose about the task shape. koto describes what koto validates.

**Why `item_schema` is generated, not authored.** The `tasks` type
has a fixed schema defined by koto itself — the template author
doesn't write it, can't override it, and doesn't need to know the
details. `derive_expects` generates `item_schema` from the type
definition. No template-authored `item_schema` field exists on
accepts; it's purely a response-side artifact.

#### Alternatives considered

- **Prose `description` field on the accepts block.** Rejected: an
  inlined JSON schema as a prose string is fragile, ugly, and forces
  the template author to re-describe something koto already knows. The schema drifts from the actual
  validation rules whenever the feature evolves.
- **`default_template` only; rely on skill docs for schema.** Rejected:
  closes Gap 1 but leaves Gap 2 open. An agent without the koto-user
  skill has no structured signal about the JSON shape.
- **Prose only (no engine changes).** Rejected: both gaps remain, no
  compile-time validation of child template paths.
- **Template-authored `item_schema` annotation on the accepts
  type.** Rejected per Decision E7: extending the type system with
  user-authored item schemas is over-engineering for v1. But this
  decision's chosen approach is the opposite — koto generates the
  schema, not the template author.

### Decision 9: Retry path end-to-end

Four blocker-class gaps in a naive retry story must be closed
together: failed batches might never reach a state where the agent
can submit `retry_failed` (a template routing on `all_complete: true`
transitions out immediately on a fully-failed batch); it must be
unambiguous whether `handle_retry_failed` transitions the parent
directly or via the advance loop; agents without skill documentation
need a way to discover `retry_failed` (it's reserved, so it never
appears in `expects.fields`); and any synthetic-template mechanism
has two unfixable edges (a real-template running child can't legally
transition into a different template's `skipped_marker` state, and
stale skip markers after partial retry have no cleanup primitive).

The fix is a five-part package; each part is load-bearing for the
others.

**Key assumptions:**

- Agents consume a new top-level `reserved_actions` response field
  on any response where the gate reports `any_failed` or
  `any_skipped`.
- Template authors tolerate compile warning W4 against
  `materialize_children` states that route only on `all_complete`.
- The when-clause engine supports an `evidence.<field>: present`
  matcher, or adds it as part of this decision's PR (a Phase 3
  prerequisite).
- Parent ticks are serialized by Decision 12's advisory flock.
- Delete-and-respawn of a real-template running child whose upstream
  flips to failure is the correct outcome; the child's work was
  already invalidated.

#### Chosen: Template-routed retry, extended gate vocabulary, discovery via `reserved_actions`, runtime reclassification

**Part 1 — Reachability.** Alongside `all_complete`, the gate output
exposes derived booleans: `all_success`, `any_failed`, `any_skipped`,
`any_spawn_failed`, `needs_attention`. Templates route on these using
equality-based when-clauses. Compile warning W4 fires when a
`materialize_children` state routes only on `all_complete: true`
without a second transition guarding `any_failed`, `any_spawn_failed`,
or `needs_attention`. The reference `coord.md` template adds an
`analyze_failures` intermediate state that the agent reaches on
`needs_attention: true` and from which the agent can submit
`retry_failed` (reserved) or `acknowledge_failures` (regular
evidence). Because `needs_attention` now folds in `any_spawn_failed`,
the existing routing on `needs_attention: true` already covers
submissions where one task failed to spawn; no template change is
required on top of the wider definition.

**Part 2 — Mechanism.** `handle_retry_failed` never appends
`Transitioned` to the parent. The canonical sequence is: validate
R10; append `EvidenceSubmitted { retry_failed: <payload> }` to the
parent; append the clearing event; write `Rewound` to closure
children (delete-and-respawn for skip markers); let the advance loop
fire a template-declared transition on
`when: evidence.retry_failed: present`. Decision 12 Q6 reorders this
for cloud-sync safety: push the parent log (both events) before
touching children, so a resolved-away parent log branch cannot leave
phantom child `Rewound` events behind.

**Part 3 — Discovery.** Responses where `any_failed`, `any_skipped`,
or `any_spawn_failed` is true carry a synthesized `reserved_actions`
array:

```json
{
  "reserved_actions": [
    {
      "name": "retry_failed",
      "description": "Re-queue failed and skipped children. Dependents are included by default.",
      "payload_schema": {
        "children": {"type": "array<string>", "required": true},
        "include_skipped": {"type": "boolean", "required": false, "default": true}
      },
      "applies_to": ["coord.issue-B"],
      "invocation": "koto next coord --with-data '{\"retry_failed\": {\"children\": [\"coord.issue-B\"]}}'"
    }
  ]
}
```

`reserved_actions` is a sibling of `expects.fields`, not a member of
it. Reserved actions bypass the advance loop's evidence validator;
conflating them with regular evidence fields would confuse agents
that scan `expects.fields` to decide what to submit. The field is
also emitted on terminal `done` responses when the final batch view
still reports failures or skips.

**Part 4 — Edges.** R10 (new validation rule) covers retry payloads:
non-empty `children` array; each name exists in the declared task
set; each named child exists on disk with outcome `failure`,
`skipped`, or `spawn_failed`; `include_skipped` optional boolean; no
other top-level fields. Mixed payloads (`retry_failed` + other
evidence) reject with `InvalidRetryReason::MixedWithOtherEvidence`.
Double retry (second submission before the first's tick) rejects with
`InvalidRetryReason::RetryAlreadyInProgress`. Retries on running or
successful children reject with
`InvalidRetryReason::ChildNotEligible` listing each child's current
outcome. Unknown child names (not present on disk) reject with
`InvalidRetryReason::UnknownChildren` listing the offending names.
All rejections are atomic: any non-retryable child in the set
rejects the whole submission.

**Retry on `spawn_failed` is retry-respawn, not retry-rewind.** A
`spawn_failed` task never produced a state file; there is no event
log to rewind. Retry on such a task re-attempts `init_state_file` on
the next tick using the CURRENT submission's entry for that name
(which is how a prior typo'd template path gets corrected: the agent
resubmits the entry with the fixed path and then retries). The
`Rewound` event machinery from the `failure` / `skipped` case does
not apply.

**Retry on a nested-batch child is unsupported in v1.** If any child
named in `retry_failed.children` is itself a batch parent (its own
state file declares or has declared a `materialize_children` hook),
the submission rejects with
`InvalidRetryReason::ChildIsBatchParent { children: [...] }`. The v0
retry machinery rewinds the named child's event log but not its
inner children; silently succeeding here would leave stale inner-
batch state behind a rewound outer child. Agents must retry at the
level where the failure actually occurred (drive the inner
coordinator, then bubble up). A future v1.1 enhancement may add
cascading retry that traverses nested batches; this decision reserves
the name and leaves the feature out of scope for v1.

**Part 5 — Runtime reclassification.** Skip markers use the child's
real template (F5 compile warning ensures each batch-eligible child
template declares a reachable `skipped_marker: true` state). Every
tick, the scheduler re-evaluates every skip marker against current
dependency outcomes. If no `waits_on` dep is still in `failure`, the
marker is stale — the scheduler deletes the child state file and
respawns the task from scratch. Real-template running children whose
upstream flips to failure are also delete-and-respawned, this time
as skip markers. The same predicate (`skipped_marker: true` on
current state) drives both observability (Decision 13's `synthetic:
true` marker) and scheduler bookkeeping.

#### Alternatives considered

- **Template author adds manual failure branches in every template
  (status quo).** Rejected: authors consistently omit this in
  practice, producing unreachable retry paths. W4 formalizes the
  check.
- **Change `all_complete` to mean `pending == 0 AND failed == 0`.**
  Rejected: breaks backward compatibility for non-batch consumers
  and conflates "nothing to do" with "everything succeeded," losing
  a useful discriminator.
- **Direct parent transition from `handle_retry_failed`.** Rejected:
  hides the retry in implementation code where template authors
  can't read the routing. Also conflates CLI-layer evidence handling
  with engine-layer state advancement. Two layers, one story is
  cleaner.
- **Advance-loop-only retry via `accepts`.** Rejected: would flow
  `retry_failed` through the evidence validator, which would need to
  special-case the reserved key and re-open the `deny_unknown_fields`
  decision.
- **Agents learn retry via koto-user skill only.** Rejected:
  violates the constraint that retry be discoverable without the
  skill. The `reserved_actions` block provides a machine-readable
  signal plus a ready-to-run invocation string.
- **Embed retry hint in `directive` text.** Rejected: directives are
  template-author-controlled; koto cannot inject text without
  surprising the author. Also not machine-readable.
- **Lenient edge handling (silently ignore non-retryable children
  in the set).** Rejected: produces partial-success ambiguity and
  clutter in the audit log (accepted retries that did nothing).
  Atomicity is clearer.
- **Auto-extend closure upward to the nearest failed ancestor.**
  Rejected: changes the semantic of a named retry. The user names
  what they mean.
- **Keep synthetic-per-skipped-child template with explicit rules
  closing the edges.** Rejected: requires cross-template transition
  support (a new engine feature) to handle a real-template running
  child whose upstream just failed, plus a dedicated refresh
  primitive for stale markers. Runtime reclassification covers both
  with one sweep and no new event types.
- **Hybrid: synthetic template for initial skip, delete-and-respawn
  only on retry.** Rejected: two code paths for the same conceptual
  operation; doubles surface area.

### Decision 10: Mutation semantics and dynamic-addition primitives

The Context section talks about dynamic additions — "a running child
adds siblings mid-flight" — as a natural extension of the scheduler.
A resubmitted task entry may have different `vars`, `waits_on`, or
`template` from the originally spawned entry, which surfaces nine
concrete mutation-pressure gaps. Without explicit handling, every
variant fails silently or incoherently: vars mutations drop at
spawn, waits_on mutations corrupt the gate output, task removal is
impossible (union semantics never decrements), renames silently
duplicate work, identical resubmissions bloat the log with no
feedback, and agents have no per-entry signal for which resubmitted
entries took effect.

This decision specifies what koto accepts at submission-validation
time, what each accepted entry does, what signal the agent gets back,
and how the rules compose with Decision 9's retry path and Decision
11's error envelope. Three constraints anchor the choice: append-only
state forbids retroactive edits of prior events; disk-derived
scheduling means a submission that conflicts with disk state has no
recoverable interpretation; and agents must get explicit signals
(silent no-ops are a common root cause of bad orchestration state).

**Key assumptions:**

- Decision 11's pre-append validation commitment holds; R8 runs
  before any `EvidenceSubmitted` append.
- `WorkflowInitialized` carries a `spawn_entry` snapshot (template,
  vars, waits_on in canonical form) — Decision 2 amendment.
- Agents accept that `cancel_tasks` is deferred to v1.1, documented
  as a non-feature in v1.
- Decision 12 serializes parent ticks via advisory flock; concurrent
  submissions cannot both R8-reject in a split-brain manner.
- Canonical-form comparison (sorted `waits_on`, null ≡ omitted for
  `template`, per-key `vars` diff) is the contract agents design
  against.

#### Chosen: Strict spawn-time immutability, union by name, per-entry scheduler feedback, `cancel_tasks` deferred

**R8 — Spawn-time immutability.** For each task entry whose computed
child name `<parent>.<task.name>` already exists on disk as a spawned
child, the entry's `template`, `vars`, and `waits_on` must match
field-for-field the entry under which the child was originally
spawned. The comparison is on the `spawn_entry` snapshot recorded on
the child's `WorkflowInitialized` event. Mismatch rejects pre-append
with `InvalidBatchReason::SpawnedTaskMutated { task, changed_fields }`
where `changed_fields` enumerates each differing field with its
`spawned_value` and `submitted_value`. One mismatched entry rejects
the whole submission.

**Union by name.** The effective task set is the union across all
accepted `EvidenceSubmitted` events. For un-spawned names, the latest
submission's entry wins (last-write-wins allows pre-spawn
correction). For spawned names, R8 locks the entry. For new names,
the entry is included as-is.

**Removal is deferred.** A submission that omits a previously-named
task does not remove it — omission is a no-op, not a cancellation
signal. `cancel_tasks` is a reserved evidence action planned for
v1.1; the design space (closure direction, running-child handling,
re-add semantics, interaction with retry) is sibling-complexity to
Decision 9 and out of scope for v1. Operators needing immediate
removal in v1 can manually delete the child's state file; this
leaves the parent's view inconsistent and is an unsupported escape
hatch.

**Renaming surfaces `orphan_candidates`.** When a new task entry has
a byte-identical `vars` + `waits_on` signature to an already-spawned
task under a different name, the scheduler emits
`scheduler.feedback.orphan_candidates` listing the match. The
signature comparison runs BEFORE the new child is spawned. When a
pre-spawn match fires, the scheduler pauses the spawn and marks the
feedback entry `errored` with
`kind: "orphan_candidate_pending"`; the agent must either rename the
entry, remove the duplicate, or (as a v1.1 future) acknowledge the
match before the scheduler will spawn. A post-spawn match — a
signature appearing only after both children already exist —
remains advisory-only and does not retroactively block work.

**Identical resubmission appends for audit.** A byte-identical
submission passes validation trivially, appends an
`EvidenceSubmitted` event, and runs the scheduler. The tick typically
finds every task `already` spawned (no-op). Suppression would hide
"the agent kept polling" from forensic investigation; the feedback
map already tells the agent per-task no-op-ness, so suppression adds
no value.

**Per-entry feedback.** `SchedulerOutcome::Scheduled` gains a
`feedback` field:

```rust
pub struct SchedulerFeedback {
    pub entries: BTreeMap<String, EntryOutcome>,
    pub orphan_candidates: Vec<OrphanCandidate>,
}

#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum EntryOutcome {
    Accepted,
    /// Child already exists and is non-terminal.
    AlreadyRunning,
    /// Child already exists at a terminal `skipped_marker: true` state.
    AlreadySkipped,
    /// Child already exists in a terminal non-failure state. Present
    /// for completeness on resubmission after partial or full batch
    /// completion.
    AlreadyTerminal,
    Blocked { waits_on: Vec<String> },
    Errored { kind: String },
    /// Target child is mid-respawn this tick; R8 comparison is
    /// vacuous until the new state file commits. Agents retry; the
    /// next tick re-evaluates against the new `spawn_entry`.
    Respawning,
}
```

Keyed by the agent-submitted short name. Every submitted entry gets
an outcome. No silent cases remain. `AlreadyRunning`, `AlreadySkipped`,
and `AlreadyTerminal` let agents distinguish "a worker is already
driving this task" from "this task is parked in a skip marker waiting
for upstream reclassification" from "this task finished normally."
`Respawning` is the transient retry-window variant described above.

**Mixed `retry_failed` + `tasks` payloads reject** with
`InvalidRetryReason::MixedWithOtherEvidence` (Decision 9 Part 4).
Agents serialize the two into separate `koto next` calls.

**R8 and runtime reclassification never interfere.** R8 runs at
submission-validation time (pre-append). Decision 9's runtime
reclassification operates on committed state using the recorded
`spawn_entry` snapshot. They are disjoint phases of `handle_next`; the
scheduler never re-validates.

**R4-before-R8 ordering.** R4 (dangling `waits_on` refs) runs before
R8 (spawn-time immutability) so an agent that typos a dependency name
does not first see a spawn-mutation rejection on an unrelated spawned
task. Data Flow documents the complete order (R0, R3, R4, R5, R6, R8,
R9 as whole-submission gates); this note pins the R4-before-R8
relationship locally for readers reasoning about mutation edge cases.

**R8-vacuous window during retry-induced respawn.** Between the moment
a retry tick deletes an old state file and the moment the new one is
written, the on-disk `spawn_entry` for that task name is transiently
absent. Another submission arriving in the same window cannot
R8-compare against a spawned entry that is mid-rewrite. When the
scheduler detects this condition (target child name present in the
current tick's respawn set but its replacement not yet written), R8
comparison for that entry is skipped and the entry's feedback carries
`outcome: "respawning"` via `EntryOutcome::Respawning`. The submission
is not rejected; the next tick re-evaluates against the committed new
`spawn_entry`. Agents treat `respawning` as a brief-transient state
equivalent to "retry back in a moment." Under Decision 12's advisory
flock, this window exists but is confined to a single tick.

**Canonical-form `default_template` resolution is spawn-time.** When a
task entry omits `template`, the canonical-form value used for R8
comparison is resolved against the `default_template` on the parent
state's `materialize_children` hook AS IT STOOD AT THE SPAWNING TICK
(stored on the child's `WorkflowInitialized.spawn_entry`), not against
the live hook. A later parent-template edit that changes
`default_template` does not trigger R8 false-positives on already-
spawned children.

**Redaction sentinel.** The secret-scrubbing in `changed_fields` uses
the literal string `"[REDACTED]"` as the replacement value. Agents
pattern-matching on redacted cells key on that exact string, not on an
object like `{"redacted": true}`.

#### Alternatives considered

- **Silently ignore mutations on spawned children (status quo).**
  Rejected: the root cause of multiple silent-failure modes in
  dynamic-addition scenarios. Violates the explicit-signal constraint.
- **Apply mutations retroactively via header edit.** Rejected:
  requires a mutation primitive (`HeaderMutated` event) on
  append-only state. Breaks the invariant that makes cloud sync and
  `expected_seq` work.
- **Full replace semantics (drop omitted names).** Rejected: removal
  by omission has no coherent representation under append-only. The
  cleanest rule is "omission is a no-op; cancellation requires the
  primitive."
- **`cancel_tasks` as a v1 reserved action.** Rejected: sibling-
  complexity to Decision 9's retry story. Deferred to v1.1 with a
  documented escape hatch.
- **Reject submissions that omit a previously-named task.**
  Rejected: makes every resubmission high-friction (the agent must
  echo every prior name forever).
- **Implicit rename via a `replaces` field.** Rejected: collapses to
  `cancel_tasks + submit`, so it's v1.1 work.
- **First-wins cross-epoch duplicate resolution.** Rejected:
  prevents pre-spawn correction. R8 already handles the post-spawn
  case.
- **Suppress identical-resubmission appends.** Rejected: special-
  case cost nearly equals R8; obscures the audit trail; redundant
  with per-task feedback.
- **Single `ignored` list for feedback.** Rejected: agents need
  positive signals per entry ("yes, C was accepted"), not just
  negative drops.
- **Support mixed `retry_failed + tasks` via two-phase commit.**
  Rejected: complexity not justified for a use case the agent can
  serialize naturally.

### Decision 11: Error envelope, validation timing, and batch-edge validation

Twelve adjoining gaps cluster around error-response shape, validation
timing, and rule coverage. An enum-shaped `NextError::Batch { kind,
message }` would break the existing `NextError` at
`src/cli/next_types.rs:283-289`, which is a struct with `code`,
`message`, and `details`. Appending `EvidenceSubmitted` before
R1-R7 runs contradicts Phase 3's pre-append commitment. Without a
dedicated `error` action variant on the response envelope, rejected
submissions have nowhere to land cleanly. Rule coverage must cover
empty task lists, name validation, and reserved-name collisions; a
`LimitExceeded.which: &'static str` field is typed by convention
only.

The twelve sub-questions decide together because envelope shape,
validation phase, and enum discriminators are tightly coupled.
Treating them as one commitment lets reviewers check one contract
instead of twelve.

**Key assumptions:**

- `NextError` is a stable v0.7.0 public contract. Breaking its
  struct shape requires a major-version bump.
- Agents pattern-match on snake_case string literals at multiple
  nesting levels.
- Decision 9 populates `InvalidRetryReason` variants; Decision 10
  populates `SpawnedTaskMutated`.
- Decision 12 renames `scheduler.spawned` → `spawned_this_tick`.

#### Chosen: Unified `action: "error"` envelope, pre-append validation, typed enums throughout

**Envelope shape.** `NextResponse` gains a seventh variant: `action:
"error"`. `NextError` is extended with an optional `batch` field
alongside `details`:

```json
{
  "action": "error",
  "state": "plan_and_await",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "Batch definition rejected: cycle in waits_on graph",
    "details": [{"field": "tasks", "reason": "cycle"}],
    "batch": {
      "kind": "invalid_batch_definition",
      "reason": "cycle",
      "cycle": ["issue-A", "issue-B", "issue-A"]
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

The existing `code`, `message`, `details` continue to work for every
current consumer. `error.batch` is new, optional, and populated only
when the error comes from batch logic. `advanced: false` always on
error responses — rejection never advances state. `scheduler: null`
preserves the additive-field invariant.

Mapping from `BatchError` variants to `NextErrorCode`:

- `InvalidBatchDefinition`, `LimitExceeded` → `invalid_submission`
  (exit 2)
- `TemplateCompileFailed` at compile time → `template_error` (exit 3)
- `SpawnFailed` and runtime `TemplateNotFound` /
  `TemplateCompileFailed` → per-task, surfaced through
  `scheduler.errored`; top-level `error` is never populated
- `BackendError` → `integration_unavailable` (exit 1, retryable)
- `InvalidRetryRequest` → `invalid_submission`
- `ConcurrentTick` → `integration_unavailable` (exit 1, retryable;
  typed variant rather than a free-string `concurrent_tick` detail)

**Pre-append validation.** R0-R9 (and R10 for retry payloads) run as
pure functions of the submitted payload *before*
`append_event(EvidenceSubmitted)`. On rejection, no state file writes
occur; the response carries the error and the parent workflow is
exactly as it was before the call. Crash-resume is trivially correct:
re-running the same submission produces the same rejection from the
same event log. No "poison" `SubmissionRejected` markers, no
permanent log entries for failed submissions.

Rejected submissions are ephemeral — their existence is observable
only in the response of the tick that produced them. Accepted-state
audit lives in the log via `EvidenceSubmitted`, `Transitioned`, and
the new `SchedulerRan` event (see below).

**Per-task accumulation, never halt.** The scheduler iterates all
ready tasks. Per-task spawn failures collect in
`SchedulerOutcome::Scheduled.errored: Vec<TaskSpawnError>`. A single
bad child template doesn't kneecap a 500-task batch. Tick-wide
failures (backend list failure during classification) still produce
`SchedulerOutcome::Error`.

```rust
pub struct TaskSpawnError {
    pub task: String,
    pub kind: SpawnErrorKind,
    pub paths_tried: Option<Vec<String>>,
    pub message: String,
    // Additional fields (full shape under Key Interfaces):
    pub template_source: Option<TemplateSource>,
    pub compile_error: Option<CompileError>,
}

#[serde(rename_all = "snake_case")]
pub enum SpawnErrorKind {
    TemplateNotFound,
    TemplateCompileFailed,
    Collision,
    BackendUnavailable,
    PermissionDenied,
    IoError,
}
```

`BatchTaskView.outcome` gains `spawn_failed` so per-task spawn errors
surface through the gate output with `outcome: "spawn_failed"` and an
optional `spawn_error` object.

**`SchedulerRan` event.** On every non-trivial tick (at least one of
`spawned`, `skipped`, or `errored` non-empty), the scheduler appends
a `SchedulerRan` event to the parent log with per-tick outcome
counts. `koto query --events` shows it alongside `EvidenceSubmitted`
and `Transitioned`, giving a complete audit trail. No-op ticks
(everything `already` or `blocked`) skip the append to prevent log
bloat.

**Empty task list rejects.** R0: `tasks.len() >= 1`. Empty submissions
reject with `InvalidBatchReason::EmptyTaskList`. This prevents the
silent-advance footgun when an agent mis-submits.

**`template: null` ≡ omitted.** Both inherit the hook's
`default_template`. An empty string is a validation error.

**Typed enum discriminators throughout.** `InvalidBatchReason`,
`InvalidNameDetail`, `LimitKind`, `SpawnErrorKind`, and
`InvalidRetryReason` replace every free-string or `&'static str`
field. JSON wire shape uses snake_case serde renaming.

**Name validation (R9).** `task.name` must match `^[A-Za-z0-9_-]+$`,
be 1-64 characters, and not collide with the reserved set
`{retry_failed, cancel_tasks}`. Validation applies to the short name
the agent wrote, not the computed `<parent>.<name>` full name —
error messages point at what the agent submitted.

**`..` in template paths is silently accepted.** Decision 4 commits
to the trusted-submitter model; surfacing a warning here would
half-retract that decision. Security Considerations documents the
behavior.

#### Alternatives considered

- **Enum-shaped `NextError::Batch { kind, message }`.** Rejected:
  breaks the existing public struct. Every consumer of `NextError`
  today unpacks `code` / `message` / `details`; an enum-variant
  alongside a struct-shape is not a valid Rust type.
- **Reuse `NextError`; squash batch fields into `details[0].reason`
  as a JSON string.** Rejected: forces agents to parse strings
  inside strings.
- **Post-append validation** (append `EvidenceSubmitted` first, run
  R0-R9, append `SubmissionRejected` on failure). Rejected: every
  downstream read path must filter rejected events forever; crash-
  resume semantics become contorted.
- **Halt on first per-task spawn error.** Rejected: one typo'd
  `template:` kills a 500-task batch. Agents forced to bisect.
- **No `SchedulerRan` event; scheduler decisions ephemeral.**
  Rejected: partial spawn failures become un-auditable.
- **Empty task list as immediately-complete batch.** Rejected:
  collapses "I forgot to add tasks" with "I intentionally want zero
  tasks." Silent-advance footgun.
- **`template: null` is an error.** Rejected: pedantic. JSON
  producers naturally serialize absent Optional fields either way.
- **Keep `InvalidBatchDefinition.reason: String`.** Rejected: fails
  the machine-parseable constraint; typos are inevitable over time.
- **Keep `LimitExceeded.which: &'static str`.** Rejected: typed by
  coincidence, not by contract. Typed enum gives serde renaming and
  exhaustive match at zero cost.
- **Roll premature-retry into `InvalidBatchReason`.** Rejected:
  retry-request problems are not batch-definition problems. Sibling
  `InvalidRetryRequest` with its own reason enum keeps the
  responsibilities clean.
- **Surface a warning for `..` in template paths.** Rejected:
  inconsistent with Decision 4's trusted-submitter commitment.
- **Strict kebab-case-only name regex.** Rejected: excludes
  legitimate mixed-case and underscore-separated names template
  authors already use.

### Decision 12: Concurrency model hardening

A "callers must serialize" rule without instruments to satisfy it
leaves real hazards in place. Ten concrete gaps bite under
concurrent operation: worker double-dispatch under per-tick `spawned`
observation, silent overwrite of child state files on concurrent
`init_state_file` calls, orphan children after cloud-sync conflict
resolution, split-brain observers with no sync-status signal, leaked
tempfiles after crashes, and permissive language ("any caller can
drive the parent") that actively invites invariant violations.

This decision is an eight-part hardening package. Each part
addresses specific findings, and the parts compose — single-machine
correctness from parts 1-3 and 7, cross-machine observability from
parts 4-5, safe retry under cloud sync from part 6, and a mental
model from part 8.

**Key assumptions:**

- Agents key on `materialized_children` for idempotent dispatch.
- Linux kernel ≥ 3.15 for `renameat2` (release notes pin this).
- `flock` is available on all supported Unix targets (already used
  by `LocalBackend` for `ContextStore` writes).
- `CloudBackend::check_sync` can compute a three-way `sync_status`
  cheaply from data it already produces.
- A 60-second threshold on the tempfile sweep bounds leak duration
  without disturbing in-flight ticks.

#### Chosen: Eight-part hardening package

**Q1 — `spawned_this_tick` + `materialized_children` ledger.** Rename
`scheduler.spawned` to `spawned_this_tick` (a per-tick *observation*;
concurrent ticks can each report the same child). Add
`materialized_children` as the *ledger* — the complete set of
children that exist on disk right now, with outcome and state. Agents
doing idempotent worker dispatch key on `materialized_children`,
taking the set difference against their last-known-dispatched set.
Both fields emit on every non-null `scheduler` value.

**Q2 — Kernel-level atomic init.** `LocalBackend::init_state_file`
uses `renameat2(RENAME_NOREPLACE)` on Linux and `link()` +
`unlink()` on other Unixes. The tempfile + rename bundle from
Decision 2 is unchanged; only the final rename step is replaced.
Collisions surface as `SpawnErrorKind::Collision` through per-task
`SchedulerOutcome.errored`. The previous "two ticks silently
overwrite the same child state file" window is closed at the kernel.

**Q3 — Advisory flock per batch parent.** `handle_next` acquires a
non-blocking `LOCK_EX | LOCK_NB` on
`<session_dir>/<workflow>.lock` for the duration of the call,
scoped to batch parents only (detected by current state's
`materialize_children` hook or a prior `SchedulerRan` /
`BatchFinalized` event in the log). Contention returns a
`concurrent_tick` error shaped per Decision 11
(`integration_unavailable`, exit 1, retryable). The lock is released on function exit (implicit file-
handle drop) — no persistent state, no daemon. This is the same
`flock` primitive `LocalBackend` already uses for `ContextStore`
writes. Read paths (`koto status`, `koto query`, `koto workflows
--children`) do not take the lock.

**Q4 — `koto session resolve` reconciles children.** The command
now iterates children of the resolved parent and reconciles
divergent child state files via `CloudBackend::check_sync`. Trivial
divergence (one side is a strict prefix of the other) auto-resolves;
non-trivial divergence requires per-child `koto session resolve
<child>`. Flags `--children=auto|skip|accept-remote|accept-local`
control behavior.

**Q5 — `sync_status` and `machine_id` response fields.** Added
conditionally: emitted only when `CloudBackend` is configured for
the workflow. Values for `sync_status`: `fresh`, `stale`,
`local_only`, `diverged`. Observers see divergence in the response
before writing, so they can run `koto session resolve` preemptively.

**Q6 — Push-parent-first retry ordering under cloud sync.** Decision
9's canonical `handle_retry_failed` sequence is reordered under
`CloudBackend`: push the parent log (both the submit and clearing
events) before writing `Rewound` to children. A push failure
returns the error with zero child writes. This eliminates the
"phantom child epoch" failure mode where a cloud-sync loser branch
leaves `Rewound` events on children that the resolved parent log
never references. Local-mode behavior collapses to the same
ordering; crash recovery becomes "user re-submits" (acceptable at
submission granularity) instead of "transparent re-apply"
(unsafe under cloud sync).

**Q7 — Tempfile sweep in `repair_half_initialized_children`.** Each
scheduler tick on a batch parent runs a pre-pass scoped to the
current parent, removing `.koto-*.tmp` files older than 60 seconds.
Q2 eliminates *races* to leaked tempfiles, but crashes (OOM, SIGKILL,
disk-full) still leave temp files. The sweep is the janitor.

**Q8 — Caller contract.** "The coordinator drives the parent;
workers drive only their own children." The coordinator-owns-parent,
workers-own-children partition is a caller contract, enforced at
runtime by Q3's lock. Both the koto-author and koto-user skills
document this convention.

**Two-hat intermediate children.** A child that
runs its own sub-batch is simultaneously a worker to its parent and
a coordinator to its sub-batch. The rule composes: each level's
`koto next <name>` is driven by exactly one caller at a time, and
that caller's role depends on which level they are acting on. An
agent running the outer parent is a coordinator relative to the
outer parent and a supervisor relative to its inner-batch children;
from the inner batch's perspective, the intermediate child IS the
coordinator. Agents detect the two-hat case via the
`MaterializedChild.role` field: when `role == Some(Coordinator)`,
the per-child `subbatch_status` summary is non-null and the child
should be driven with inner-coordinator responsibilities on the
inner parent's lock. Skill docs cover the pattern in the
"coordinator of coordinators" section.

#### Alternatives considered

- **Keep `spawned`, document per-tick-ness.** Rejected: the word
  "spawned" reasonably suggests "new this moment, dispatch now." The
  cost of renaming is one find-and-replace plus a documentation
  pass; the cost of preserving the misleading name is perpetual
  documentation warning about a footgun.
- **Remove `spawned` entirely; require diffing
  `materialized_children` between ticks.** Rejected: observably-new
  spawns are information, not noise. Per-tick audit streams and
  interactive agents benefit from knowing "this tick I spawned D."
- **Keep the `init_state_file` race and document it.** Rejected on
  data-preservation grounds: the "caller-should-not-race" framing
  doesn't account for silent child-state corruption when a caller
  does race. `RENAME_NOREPLACE` converts "silently overwrite" into
  "one process wins, the other reports cleanly."
- **Lockfile only, no kernel atomicity.** Rejected: `flock` is
  unreliable over NFS and doesn't cover a second process that
  bypasses the lock by mistake. Kernel atomicity is cheap insurance.
- **Caller-serializes with diagnostic-only detection.** Rejected:
  the diagnostic fires after corruption. Preventative guard (the
  lock) is strictly better.
- **Add `--children` flag to `session resolve`, default skip.**
  Rejected: preserves silent cross-machine divergence. The correct
  default is "reconcile what you can, flag what you can't."
- **Add `sync_status` unconditionally on all responses.** Rejected:
  under local mode the values are constant (`fresh` + hostname) and
  provide no information. Conditional emission for conditionally-
  present subsystems is idiomatic.
- **Defer `sync_status` / `machine_id`.** Rejected: known hazard,
  low cost, still-`Proposed` design — add it now.
- **Make the clearing event a pre-condition of child rewinds (via
  scheduler guard).** Rejected: ad-hoc runtime guard; enforcing
  ordering at write time is cleaner.
- **Rely on resume idempotency for retry under cloud sync.**
  Rejected: collapses to "split-brain-is-fine" when the local log
  is the losing branch, which leaves phantom child epochs
  referencing a retry that the resolved parent log never records.
- **No tempfile sweep.** Rejected: race elimination does not
  eliminate crash-leak paths (SIGKILL bypasses drop handlers).
- **Global tempfile sweep across the session directory.** Rejected:
  overreach. Scoping to the current parent is sufficient.
- **Hard rule only in Concurrency Model section, preserve casual
  narrative language elsewhere.** Rejected: mental model is set by
  earlier prose; a later hard rule doesn't repair it.

### Decision 13: Post-completion observability

Decision 6 extended `koto status <parent>` and `koto workflows
--children <parent>` with batch metadata, but only while the parent's
current state declares a `materialize_children` hook. Five gaps
surface the moment the batch terminates or the parent advances past
the batched state:

1. `koto status` drops the `batch` section the instant the parent
   leaves the batched state — exactly when consumers need it to
   write summary directives or diagnose failures.
2. The minimal `done` response drops `blocking_conditions` and
   `scheduler`, so batch detail evaporates on the terminal tick.
3. Synthetic skipped children are shape-indistinguishable in `koto
   status` and `koto workflows --children` from real terminal work.
4. Transitive skip attribution is singular (`skipped_because: X`);
   diamond chains like B-failed → D-skipped → E-skipped lose
   context about the root cause.
5. When `failure_reason` is unset, the batch view's `reason` falls
   back to the opaque terminal state name silently.

All five gaps are observability-only and additive. The fix lands as
read-only on the query paths; none of the extensions changes
existing field semantics.

**Key assumptions:**

- Decision 9's on-disk representation of skipped children exposes a
  single well-known predicate. This decision names it
  `skipped_marker: true` on the child's current state.
- Cloud sync tolerates a new `BatchFinalized` event type under
  existing append-only rules.
- `batch_final_view` payload stays bounded by Decision 1's R6 caps.
- W-level warnings (W1-W4) have an existing surfacing / suppression
  convention that W5 reuses.
- Adding optional top-level response fields (`batch_final_view`,
  `synthetic`, `skipped_because_chain`, `reason_source`) is
  backward-compatible.

#### Chosen: Persist `batch_final_view`, extend terminal responses, mark synthetic children, record transitive skip chain, add W5

**`BatchFinalized` event.** When `children-complete` first evaluates
`all_complete: true` on a state with `materialize_children`, the
advance loop appends a `BatchFinalized` event to the parent log
containing the final `BatchView` snapshot. Subsequent `koto status
<parent>` calls — regardless of the parent's current state — emit
the `batch` section by replaying the most recent `BatchFinalized`
event, labeled `batch.phase: "final"` (versus `batch.phase: "active"`
for a state whose current hook has not yet produced a
`BatchFinalized` event). Re-entering a batched state (after
`retry_failed`, for example) invalidates the prior finalization: the
next finalization appends a new `BatchFinalized` event, and
`batch_final_view` on terminal responses always reflects the MOST
RECENT `BatchFinalized` event.

**`batch.phase` semantics.** `"active"` means the current
state carries a `materialize_children` hook and no `BatchFinalized`
event has been appended yet for this pass; the batch is still
producing work. `"final"` means a `BatchFinalized` event has been
appended for the most recent pass. The transient single-tick window
where the `BatchFinalized` event is appended but the parent has not
yet left the batched state is classified as `"final"`: the event's
existence is load-bearing, the parent's current state is not. A
retry tick that re-enters the batched state does not immediately
revert `phase` — the old `BatchFinalized` event remains on the log
until a new one supersedes it, and `koto status` reports `"final"`
with the previous snapshot until the new finalization lands.

Event-based storage round-trips through cloud sync identically to the
rest of the append-only log and survives the `retry_failed` evidence-
clearing write that wipes reserved context keys.

**`batch_final_view` on `done` responses.** The `done` response
shape gains an optional `batch_final_view` field, present when the
parent log contains at least one `BatchFinalized` event. The payload
matches the `batch` section shape. This gives agents on the terminal
tick — exactly when they're asked to write a summary directive — the
full batch snapshot without a second command.

**`synthetic: true` marker.** `koto status <child>` and the per-row
shape in `koto workflows --children <parent>` add an explicit boolean
`synthetic: true` when the child's current state has `skipped_marker:
true`. The predicate is computed from state, not from a template
hash, so it works under Decision 9's runtime-reclassification model.
`koto next <synthetic-child>` returns an immediate terminal `done`
response:

```json
{
  "action": "done",
  "state": "skipped_due_to_dep_failure",
  "directive": "This task was skipped because dependency '<skipped_because>' did not succeed. No action required.",
  "is_terminal": true,
  "synthetic": true,
  "skipped_because": "<name>",
  "skipped_because_chain": [...]
}
```

Error semantics on a legitimate state read would be hostile; a
silent blank directive is worse. Explicit `synthetic: true` plus an
interpolated directive gives both machine-readable and human-readable
answers.

**`skipped_because_chain` array.** Retain the singular
`skipped_because: <name>` field (the *direct* upstream blocker). Add
a parallel `skipped_because_chain: [<direct>, ..., <root-failure>]`
array recording the full attribution path. For B-failed → D-skipped
→ E-skipped:

- D: `skipped_because: "B"`, `skipped_because_chain: ["B"]`.
- E: `skipped_because: "D"`, `skipped_because_chain: ["D", "B"]`.

The chain walks upstream through `waits_on` until a failed (non-
skipped) ancestor. Diamonds pick the shortest chain, alphabetical
tie-break for determinism. Both fields land in the batch view, in
`koto status` output, in `--children` rows, and in the synthetic
child's own responses.

**Tie-break for `skipped_because`.** In diamond skip
scenarios where two `waits_on` ancestors both failed, singular
`skipped_because` names the EARLIEST-IN-SUBMISSION-ORDER failed
ancestor. `skipped_because_chain` lists all unique failed ancestors
walked upstream in topological order (closest ancestor first, root
failure last). Submission order is deterministic: the first task
list submission to the parent sets a stable index that persists
across re-submissions under Decision 10's union-by-name rules.

**W5 compile warning.** Terminal state with `failure: true` has no
path that writes `failure_reason` to context. The batch view's
`reason` falls back to the state name, which is uninformative. The
compiler fires W5 when none of the following holds: (a) the state's
`accepts` block declares a `failure_reason` field; (b) the state's
`default_action` writes `failure_reason`; (c) an upstream transition
carries a `context_assignments` entry writing `failure_reason`.

**`reason_source` projection.** When the batch view's per-child
`reason` engages the `failure_reason` context key, emit
`reason_source: "failure_reason"`. When it falls back to the state
name for failed children, emit `reason_source: "state_name"`.
The full enum:

- `"failure_reason"` — child wrote the `failure_reason` context key.
- `"state_name"` — fallback for failed children whose template does
  not write `failure_reason` (W5 warns at compile time).
- `"skipped"` — child carries `skipped_marker: true` on its current
  state; `reason` echoes `skipped_because_chain[-1]` (the root
  failure).
- `"not_spawned"` — child outcome is `spawn_failed`; `reason` echoes
  the `TaskSpawnError.kind`.

Omitted for successful or not-yet-terminal children.

**`batch_final_view` is frozen-at-BatchFinalized.** The snapshot is
captured at the moment the `BatchFinalized` event appends and does
NOT update if a later retry changes an outcome. Agents reading
`batch_final_view` see the state at last finalization; live drift
during a retry pass is observable via the current gate output (and
`koto status` with `batch.phase: "active"`). When a retry tick
eventually produces a new `BatchFinalized` event, that event carries
the post-retry snapshot and supersedes the prior one for all
subsequent `batch_final_view` reads. Agents needing continuous live
state during retry should read the gate output rather than
`batch_final_view`.

**`batch_final_view` is per-level, not recursive.** A Reading A
parent's `batch_final_view` carries its direct children's outcomes.
It does NOT recursively embed the `batch_final_view` of any child
that itself drives a sub-batch. Outer-level observers wanting a
glimpse of inner-batch progress use the per-child `subbatch_status`
field on `MaterializedChild` (a `BatchSummary` snapshot) rather
than recursive finalization data.

#### Alternatives considered

- **Carry last-known batch view through subsequent states via an
  in-memory cache.** Rejected: doesn't survive process restarts;
  inconsistent with koto's "pure function of disk state" model;
  breaks on cloud-sync machine handoffs.
- **Leave batch-view preservation to the consumer via evidence.**
  Rejected: duplicates work koto already does via
  `derive_batch_view`. The consumer's snapshot would ride on
  context keys (polluting template-author namespace) or a sidecar
  file (violating the append-only-log invariant).
- **Keep `done` minimal; expect consumers to call `koto status` for
  batch detail.** Rejected: a known-frequent access pattern (write-
  summary-on-terminal-tick) shouldn't require two commands when one
  suffices.
- **`kind: "skip_marker"` string field instead of `synthetic: bool`.**
  Rejected: more expressive than the single category observers need.
  Boolean is simpler; a string enum can be added later without
  breaking anything.
- **Error on `koto next <synthetic-child>`.** Rejected: errors
  imply caller malformation. A synthetic child is a legitimate
  workflow that happens to have nothing to do.
- **Switch singular `skipped_because` to name the root cause (B)
  instead of direct blocker (D).** Rejected: breaking change on an
  existing field; direct blocker is more locally useful for "which
  task do I retry to unblock this?"
- **Replace singular with plural array only.** Rejected: breaking
  change on existing consumers.
- **Make missing `failure_reason` writer a hard error (E11).**
  Rejected: overly strong. A template author may legitimately
  decide the state name is sufficient (e.g., `done_cancelled_by_user`).
  Warning respects authorial intent while flagging the common
  mistake.
- **Add no warning; rely on skill-level documentation.** Rejected:
  leaves the silent-fallback gap unflagged at authoring time.

### Decision 14: Path resolution contradictions

Five gaps against Decision 4's path resolution and the `BatchError`
enum need explicit handling:

1. R1 and Data Flow step 4 said a single bad child template fails
   the whole submission, but `BatchError::TemplateResolveFailed
   { task, ... }` variant shape implied per-task scope.
2. Decision 4's fallback text said "on ENOENT" — but an absent
   `template_source_dir` (pre-Decision-4 state files) never reaches
   the ENOENT check at all.
3. `TemplateResolveFailed` conflated "file not found" with "file
   found but failed to compile." `paths_tried` is meaningless for
   the compile case.
4. "DAG depth of 50" had three plausible definitions (edges on
   longest path, nodes on longest path, any-to-any).
5. Security Considerations noted `template_source_dir` exposure but
   didn't surface the cross-machine portability limitation.

Decision 4's core mechanism stands: `template_source_dir` on the
header, `submitter_cwd` on `EvidenceSubmitted` events, resolution
order absolute → template_source_dir → submitter_cwd → error. This
decision resolves the five gaps within that mechanism.

**Key assumptions:**

- Decision 11's envelope accommodates per-task scheduler errors and
  a warnings vector on `SchedulerOutcome`.
- Agents understand that a per-task failure does not abort siblings.
- `Path::exists()` per tick is a cheap probe.
- Same-layout cross-machine is the common case.

#### Chosen: Per-task failures, absent-source-dir skips cleanly, split variant, node-count depth, runtime warning on staleness

**R1 is per-task, not whole-submission.** Child-template compile and
resolve failures produce per-task errors in `SchedulerOutcome.errored`
and do not abort sibling spawns. Whole-submission failures restrict
to graph properties: R3 (cycles), R4 (dangling refs), R5 (duplicate
names), R6 (limits), R8 (spawn-time mutation), R9 (invalid name).
These reject pre-append with `InvalidBatchReason` variants. Data
Flow Step 4 is rewritten to enumerate only R3/R4/R5/R6/R8/R9 as
whole-submission failures.

**Absent `template_source_dir` skips step (b) and emits a warning.**
The resolution order has an explicit absent-case branch:

- (a) Absolute paths pass through.
- (b) If `template_source_dir` is `Some(dir)`, join the relative path
  against it. If the file exists, use it. If ENOENT, fall through.
- **(b') If `template_source_dir` is `None`, skip (b) entirely. Emit
  `SchedulerWarning::MissingTemplateSourceDir` once per tick. Fall
  through to (c).**
- (c) Join against `submitter_cwd`. If the file exists, use it.
- (d) Return `TemplateNotFound { task, paths_tried }` listing every
  attempted path.

**Present-but-stale `template_source_dir` emits a warning.** When
`Path::new(template_source_dir).exists()` is false at scheduler
start, emit `SchedulerWarning::StaleTemplateSourceDir` and fall
through to `submitter_cwd`. Deduplicated per `template_source_dir`
value per tick.

**Split `TemplateResolveFailed` into two variants.**

```rust
pub enum BatchError {
    // ... other variants ...
    TemplateNotFound { task: String, paths_tried: Vec<String> },
    TemplateCompileFailed { task: String, path: String, compile_error: String },
}
```

Agents programmatically distinguish "my path is wrong" from "my
template file is broken" and can render targeted recovery
suggestions.

**DAG depth = longest root-to-leaf path, counted in nodes.** A
**root** is a task with empty `waits_on`; a **leaf** is a task no
sibling's `waits_on` references. Depth is the node count along the
longest root-to-leaf path. A linear chain of 51 tasks has depth 51
and exceeds the limit. `BatchError::LimitExceeded` messages for
depth now say "Longest dependency chain has N tasks; limit is 50."

Node count matches user intuition ("I wrote 51 tasks") without the
off-by-one surprise edge-count depth would produce at the 50/51
boundary.

**Cross-machine portability documented + runtime warning.** Security
Considerations gains a paragraph describing when
`template_source_dir` and `submitter_cwd` become stale (cross-home-
layout migrations, Linux ↔ macOS, different usernames, container
paths). The paragraph points at the two warnings above and notes
that a future `koto session retarget` subcommand may land in a
separate design.

**Known limitations.**

- **State files predating the `template_source_dir` header keep
  emitting `MissingTemplateSourceDir` every tick.** There is no
  header-rewrite primitive in v1, so a parent upgraded mid-workflow
  accumulates one warning per scheduler tick until it terminates. A
  future `koto session rehome <parent>` subcommand — scope for v1.1
  or a successor design — would let operators patch the header
  without losing event-log history. Agents may deduplicate the
  warning locally.

- **Absolute child-template paths bypass the path-resolution
  warnings at submission time.** `StaleTemplateSourceDir` fires only
  when a relative path falls through to `template_source_dir`. An
  absolute path valid on machine A but missing on machine B surfaces
  as a per-task `TemplateNotFound` when the child-bearing scheduler
  tick runs on B, not as a submission-time diagnostic. Security
  Considerations lists this as a known limitation under cross-machine
  drift.

- **`paths_tried` canonicalization.** The absolute paths echoed in
  `TemplateNotFound.paths_tried` and `TaskSpawnError.paths_tried` are
  canonicalized (`..` segments resolved) before serialization so
  consumers read clean paths instead of literal compositions of the
  resolution base plus the agent-supplied relative path.

#### Alternatives considered

- **Whole-submission halt on bad child template.** Rejected:
  inconsistent with the variant's `task: String` field; kills
  partial-success needed by dynamic additions.
- **Hybrid per-task for compile, whole for not-found.** Rejected:
  arbitrary — not-found is no more a graph property than compile-
  failed.
- **Error at submission when `template_source_dir` is absent.**
  Rejected: breaks backward-compatibility with state files that
  predate the header field and would reject them on first batch
  submission after upgrade.
- **Silent fallback on absent `template_source_dir`.** Rejected:
  hides diagnostic signal agents need to understand surprise
  failures.
- **Keep one `TemplateResolveFailed` variant with a `kind`
  discriminator.** Rejected: bloats the variant with fields
  meaningful to only one kind; still forces agents to pattern-match
  a nested discriminator.
- **Edge-count DAG depth.** Rejected: matches CS convention but
  produces off-by-one surprises at the boundary ("I wrote 51 tasks
  and koto says depth 50 is the limit — which is it?").
- **"Any-to-any" longest path.** Rejected: in a DAG, every path
  extends to a root and leaf, so any-to-any collapses to root-to-
  leaf. Not a real alternative.
- **Doc-only portability note, no runtime warning.** Rejected:
  leaves agents with no runtime signal when cross-machine
  resolution is about to fail.
- **`koto session retarget` subcommand to rewrite header fields.**
  Real fix but out of Decision 14's scope; noted as a future
  extension.
- **Repo-relative `template_source_dir` alongside the absolute
  path.** Real fix but requires git-root detection and a new header
  field; out of scope for v1.

## Decision Outcome

The fourteen decisions interlock into one coherent implementation. The
batch feature lands as a single atomic change set with the following
architectural thesis:

**The parent declares a DAG via the `materialize_children` hook, koto
derives the scheduling state entirely from disk on every `koto next`
tick, and failures route through first-class terminal-failure and
skipped-marker states that the existing `children-complete` gate
picks up with minimal extension.**

All fourteen decisions are consistent with the foundational choices
(Reading A as primary, disk-derived storage, CLI-level scheduler
tick, deterministic child naming, skip-dependents default). They
share three cross-cutting PR landing requirements:

1. **One schema-layer PR.** `TemplateState` grows three new fields
   (`materialize_children`, `failure`, `skipped_marker`) and accepts
   gains a `tasks` field type. The narrow `deny_unknown_fields`
   attribute from Decision 3 lands in the same PR.

2. **One init-safety PR (can precede the schema PR).** Decision 2's
   `init_state_file` refactor extracts the backend-trait method,
   converts `handle_init` to use it, and is independently shippable.
   Three call sites will consume it later (regular init, scheduler
   spawn, runtime reclassification — delete-and-respawn of skipped
   children per Decision 9).

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
   spawn path, and runtime reclassification (delete-and-respawn of
   skipped children per Decision 9).
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
│                          src/cli/batch.rs  (NEW)             │
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
│  types.rs: accepts field type gains `tasks` variant         │
│                                                             │
│  compile.rs: new validator for materialize_children         │
│    enforcing E1-E10 errors and W1-W5 warnings (plus F5)     │
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
    pub default_template: String,  // compiler-validated path
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
violate the engine's I/O-free invariant.

All new types below carry
`#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]` unless
otherwise noted. Enums use `#[serde(rename_all = "snake_case")]`
and, where tagged, `#[serde(tag = "kind")]` or similar as documented
per-enum.

```rust
pub enum SchedulerOutcome {
    NoBatch,
    Scheduled {
        /// Per-tick observation: children whose state file THIS tick created.
        /// Decision 12 Q1: renamed from `spawned`. Concurrent ticks can each
        /// return the same child; use `materialized_children` for idempotent
        /// dispatch.
        spawned_this_tick: Vec<String>,
        /// Ledger: every child that exists on disk for this parent right now,
        /// with outcome and current state. Decision 12 Q1.
        materialized_children: Vec<MaterializedChild>,
        already: Vec<String>,
        blocked: Vec<String>,
        skipped: Vec<SkippedEntry>,
        /// Per-task spawn errors accumulated during the tick. Decision 11 Q4:
        /// scheduler never halts on one bad task; siblings keep spawning.
        errored: Vec<TaskSpawnError>,
        /// Non-fatal warnings the scheduler emitted this tick. Decision 14.
        warnings: Vec<SchedulerWarning>,
        /// Per-entry feedback keyed by the agent-submitted short name.
        /// Decision 10.
        feedback: SchedulerFeedback,
    },
    /// Reserved for tick-wide failures (backend list failure during
    /// classification; nothing can be known, so nothing can be reported).
    Error { reason: BatchError },
}

pub struct MaterializedChild {
    pub name: String,
    pub outcome: TaskOutcome,
    pub state: Option<String>,
    /// True iff this child is non-terminal AND every `waits_on` dep
    /// from its current `spawn_entry` is terminal-success. Decision 9:
    /// workers dispatch only on `ready_to_drive == true`. False for
    /// newly-respawned children whose upstream deps haven't settled
    /// yet.
    pub ready_to_drive: bool,
    /// Present when this child's current state carries a
    /// `materialize_children` hook of its own (i.e., the child is a
    /// batch parent for its own sub-batch). Agents use this to render
    /// a two-hat marker on intermediate coordinators. See Decision 12
    /// Q8.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<ChildRole>,
    /// When `role == Some(Coordinator)`, carries a summary of the
    /// sub-batch the child is driving. Omitted otherwise. See Decision
    /// 13: gives outer-level observers visibility without a recursive
    /// `batch_final_view`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subbatch_status: Option<BatchSummary>,
}

#[serde(rename_all = "snake_case")]
pub enum ChildRole {
    /// Child is a regular batch worker.
    Worker,
    /// Child carries its own `materialize_children` hook and is
    /// coordinating a sub-batch.
    Coordinator,
}

/// Per-task outcome discriminator, shared by MaterializedChild and
/// BatchTaskView. Decision 11's "typed enum discriminators throughout"
/// commitment applies here.
#[serde(rename_all = "snake_case")]
pub enum TaskOutcome {
    Success,
    Failure,
    Skipped,
    Pending,
    Blocked,
    SpawnFailed,
}

/// Named entry in `SchedulerOutcome::Scheduled.skipped`. Replaces the
/// earlier `(String, String)` tuple to keep the JSON shape
/// agent-readable.
pub struct SkippedEntry {
    pub task: String,
    pub reason: String,
}

pub struct SchedulerFeedback {
    /// Keyed by short task name (agent-submitted, not <parent>.<name>).
    pub entries: BTreeMap<String, EntryOutcome>,
    /// Signature-match detections (Decision 10: rename detection).
    pub orphan_candidates: Vec<OrphanCandidate>,
}

#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum EntryOutcome {
    Accepted,
    /// Child already exists and is non-terminal.
    AlreadyRunning,
    /// Child already exists as a terminal skip marker.
    AlreadySkipped,
    /// Child already exists in a terminal non-failure state. Present
    /// for completeness on resubmission after batch completion.
    AlreadyTerminal,
    Blocked { waits_on: Vec<String> },
    Errored { kind: String },
    /// Target child is mid-respawn this tick (R8 vacuous window).
    /// Agents retry; the next tick re-evaluates.
    Respawning,
}

pub struct OrphanCandidate {
    pub new_task: String,
    pub signature_match: String,
    pub confidence: String,   // "exact" | "fuzzy"
    pub message: String,
}

#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SchedulerWarning {
    MissingTemplateSourceDir,
    /// Enriched with `machine_id` (when cloud sync is configured) and
    /// the path the scheduler fell back to, so agents don't need to
    /// recompose the context from other fields.
    StaleTemplateSourceDir {
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        machine_id: Option<String>,
        falling_back_to: PathBuf,
    },
    /// Emitted when a submission omits a task name that appeared in a
    /// prior submission for this parent. Informational only (omission
    /// is not a cancellation signal per Decision 10), but agents are
    /// told rather than left to infer silently.
    OmittedPriorTask { task: String },
}

pub struct TaskSpawnError {
    pub task: String,
    pub kind: SpawnErrorKind,
    /// Absolute paths the scheduler probed during template
    /// resolution. Canonicalized (no `..` segments) before
    /// serialization.
    pub paths_tried: Option<Vec<String>>,
    pub message: String,
    /// Whether the template path came from the agent's `template`
    /// field or was inherited from the hook's `default_template`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template_source: Option<TemplateSource>,
    /// Typed compile-error detail when
    /// `kind == TemplateCompileFailed`; matches the shape used by
    /// `BatchError::TemplateCompileFailed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compile_error: Option<CompileError>,
}

#[serde(rename_all = "snake_case")]
pub enum TemplateSource {
    /// The task entry carried an explicit `template` field.
    Override,
    /// The task entry inherited `default_template` from the hook.
    Default,
}

#[serde(rename_all = "snake_case")]
pub enum SpawnErrorKind {
    TemplateNotFound,
    TemplateCompileFailed,
    Collision,              // EEXIST on init_state_file
    BackendUnavailable,
    PermissionDenied,
    IoError,
}
```

**Response serialization.** The `scheduler` field appears as a
top-level key in the serialized JSON response, alongside `action`,
`state`, `directive`, `blocking_conditions`, etc. It is NOT inside any
specific `NextResponse` enum variant. Implementation: `handle_next`
serializes the `NextResponse` variant to JSON, then merges the
`SchedulerOutcome` as an additional top-level `scheduler` key before
returning. This avoids modifying every `NextResponse` variant to carry
an optional scheduler field.

**New `action: "error"` response variant** (Decision 11). Rejected
submissions return a seventh `NextResponse` variant with envelope:

```json
{
  "action": "error",
  "state": "plan_and_await",
  "advanced": false,
  "error": {
    "code": "invalid_submission",
    "message": "<human-readable>",
    "details": [{"field": "<path>", "reason": "<snake_case>"}],
    "batch": {
      "kind": "invalid_batch_definition",
      "reason": "<InvalidBatchReason tag>",
      ... typed fields per variant ...
    }
  },
  "blocking_conditions": [],
  "scheduler": null
}
```

**New `reserved_actions` sibling field** (Decision 9). Emitted on any
response whose gate output reports `any_failed` or `any_skipped`:

```json
{
  "reserved_actions": [{
    "name": "retry_failed",
    "description": "...",
    "payload_schema": { "children": {...}, "include_skipped": {...} },
    "applies_to": ["<retryable child names>"],
    "invocation": "koto next <parent> --with-data '{\"retry_failed\": {...}}'"
  }]
}
```

**New `batch_final_view` field on `done` responses** (Decision 13).
Present when the parent log contains at least one `BatchFinalized`
event. Payload shape matches the `batch` section of `koto status`.

**New `sync_status` / `machine_id` top-level fields** (Decision 12
Q5). Emitted only under `CloudBackend`. `sync_status` values:
`"fresh"`, `"stale"`, `"local_only"`, `"diverged"`.

```rust
pub enum BatchError {
    /// Task list failed whole-submission validation (graph properties,
    /// spawn-time immutability, name validity). Limit violations are
    /// carried by the sibling `LimitExceeded` variant, not by nested
    /// `InvalidBatchReason::LimitExceeded*` variants.
    InvalidBatchDefinition { reason: InvalidBatchReason },
    /// backend.create / init_state_file failed for a specific task.
    /// Surfaces per-task via SchedulerOutcome.errored; never a top-level
    /// NextError.
    SpawnFailed { task: String, kind: SpawnErrorKind, message: String },
    /// Template path didn't resolve against any configured base.
    /// Decision 14: split from the former TemplateResolveFailed.
    TemplateNotFound { task: String, paths_tried: Vec<String> },
    /// Template found and read, but compilation failed.
    /// Decision 14: split from the former TemplateResolveFailed.
    /// `compile_error` is the typed `CompileError` struct shared with
    /// per-task `TaskSpawnError` so agents render one shape for
    /// compile failures regardless of surface.
    TemplateCompileFailed { task: String, path: String, compile_error: CompileError },
    /// Backend list/read failed during classification. Tick-wide.
    BackendError { message: String, retryable: bool },
    /// Submission exceeds a hard limit. One variant covers every
    /// limit kind; the optional `task` is populated only for per-task
    /// limits such as `WaitsOn`.
    LimitExceeded { which: LimitKind, limit: usize, actual: usize, task: Option<String> },
    /// Retry submission failed validation. Decision 9, Decision 11 Q11.
    InvalidRetryRequest { reason: InvalidRetryReason },
    /// Another `koto next` tick is holding the advisory flock on this
    /// parent. Typed variant rather than a free-string
    /// `concurrent_tick` detail.
    ConcurrentTick { holder_pid: Option<u32> },
}

/// Typed compile-error detail shared by `BatchError::TemplateCompileFailed`
/// and `TaskSpawnError` when the scheduler surfaces a runtime template
/// compile failure. Typed rather than a free string.
pub struct CompileError {
    /// Short, machine-parseable discriminator (e.g. `yaml_parse`,
    /// `missing_field`, `state_reference`).
    pub kind: String,
    /// Human-readable message from the compiler.
    pub message: String,
    /// Optional source location if the compiler emits one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<CompileErrorLocation>,
}

pub struct CompileErrorLocation {
    pub line: u32,
    pub column: u32,
}

#[serde(tag = "reason", rename_all = "snake_case")]
pub enum InvalidBatchReason {
    EmptyTaskList,                                        // R0
    Cycle { cycle: Vec<String> },                         // R3
    DanglingRefs { entries: Vec<DanglingRef> },           // R4
    DuplicateNames { duplicates: Vec<String> },           // R5
    /// Inner discriminator is `kind` (matches serde idiom; avoids
    /// double-nesting under `detail: { detail: ... }`).
    InvalidName { task: String, kind: InvalidNameDetail }, // R9
    ReservedNameCollision { task: String, reserved: String }, // R9
    TriggerRuleUnsupported { task: String, rule: String },
    /// Decision 10 R8: submission tried to mutate a spawned child's fields.
    SpawnedTaskMutated { task: String, changed_fields: Vec<MutatedField> },
    // Limits surface via the sibling
    // `BatchError::LimitExceeded { which, limit, actual, task }`
    // variant with `which` carrying the typed `LimitKind`, not via
    // nested variants on this enum.
}

#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InvalidNameDetail {
    Empty,
    InvalidChars { pattern: String },
    TooLong { limit: usize, actual: usize },
}

#[serde(rename_all = "snake_case")]
pub enum LimitKind {
    Tasks,
    WaitsOn,
    Depth,
    PayloadBytes,
}

#[serde(tag = "reason", rename_all = "snake_case")]
pub enum InvalidRetryReason {
    NoBatchMaterialized,                          // premature retry
    EmptyChildList,
    /// Named children exist on disk but are not in a retryable state
    /// (running or already-successful). The `current_outcome` field
    /// on `ChildEligibility` never carries the sentinel `"unknown"`
    /// — unknown names go to `UnknownChildren` below.
    ChildNotEligible { children: Vec<ChildEligibility> },
    /// Named children do not exist on disk for this parent.
    UnknownChildren { children: Vec<String> },
    /// A child named in `retry_failed.children` is itself a batch
    /// parent (its state carries or has declared a
    /// `materialize_children` hook). v1 rejects cross-level retry;
    /// agents drive the inner coordinator instead.
    ChildIsBatchParent { children: Vec<String> },
    /// Reserved for non-flocked futures. Under Decision 12's advisory
    /// flock, a concurrent retry submission surfaces as
    /// `BatchError::ConcurrentTick` at a lower layer. This variant is
    /// kept in the enum so the name is not reused for a different
    /// meaning later.
    RetryAlreadyInProgress,
    MixedWithOtherEvidence { extra_fields: Vec<String> },
}

pub struct MutatedField {
    pub field: String,   // "template" | "vars" | "waits_on" | "vars.<key>"
    pub spawned_value: serde_json::Value,
    pub submitted_value: serde_json::Value,
}

pub struct ChildEligibility {
    pub name: String,
    /// The child's current outcome; one of `failure`, `skipped`,
    /// `spawn_failed`, `pending`, or `success`. There is no
    /// `"unknown"` sentinel — unknown names surface through
    /// `InvalidRetryReason::UnknownChildren` instead.
    pub current_outcome: String,
}

// BatchError maps to the existing NextError struct (Decision 11 Q3).
// There is NO `NextError::Batch` variant; batch-specific context lives
// in a sibling `error.batch` object alongside `details`. Mapping:
//
//   InvalidBatchDefinition / LimitExceeded / InvalidRetryRequest
//     → NextError { code: InvalidSubmission, ... }
//   TemplateCompileFailed at compile time
//     → NextError { code: TemplateError, ... }
//   ConcurrentTick → NextError { code: IntegrationUnavailable, ... }
//   SpawnFailed / TemplateNotFound / TemplateCompileFailed at runtime
//     → NOT promoted to top-level NextError; surface per-task via
//       SchedulerOutcome.errored.
//   BackendError → NextError { code: IntegrationUnavailable, ... }

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

pub struct BatchView {
    pub summary: BatchSummary,
    pub tasks: Vec<BatchTaskView>,
    pub ready: Vec<String>,
    pub blocked: Vec<String>,
    pub skipped: Vec<String>,
    pub failed: Vec<String>,
}

pub struct BatchSummary {
    pub total: usize,
    pub success: usize,
    pub failed: usize,
    pub skipped: usize,
    pub pending: usize,
    pub blocked: usize,
    /// Decision 11 Q4: per-task spawn failures that didn't abort siblings.
    pub spawn_failed: usize,
}

pub struct BatchTaskView {
    pub name: String,
    pub child: Option<String>,
    /// Decision 11 Q4: per-task outcome, including `spawn_failed`.
    pub outcome: TaskOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Decision 13: "failure_reason" (context key present) vs
    /// "state_name" (fallback). Omitted for successful / non-terminal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_source: Option<String>,
    /// Decision 13: explicit marker for scheduler-authored skip children.
    /// Computed from `skipped_marker: true` on the child's current state.
    #[serde(default, skip_serializing_if = "is_false")]
    pub synthetic: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waits_on: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_because: Option<String>,
    /// Decision 13: full attribution path from direct blocker back to the
    /// first failed (non-skipped) ancestor. Empty when this task is not
    /// skipped.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped_because_chain: Vec<String>,
    /// Per-task spawn-error detail when outcome == "spawn_failed".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spawn_error: Option<TaskSpawnError>,
}
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

**End-to-end sequence for a 10-task batch.** This walks from parent
init through batch completion, emphasizing the handshake between
the agent and koto. The parent template uses the single-state
fan-out pattern shown in Decision 1 above.

**Step 1: Parent init.** Agent runs `koto init parent-42 --template
parent.md`. Parent's state file is created with initial state
`plan_and_await`.

**Step 2: Agent gets the planning directive.** `koto next parent-42`:
- advance loop at `plan_and_await`
- no evidence yet; `tasks` is `required: true` and absent
- response: `evidence_required` with the directive "read the plan
  and submit a task list" and the accepts schema
- agent receives and interprets

**Step 3: Agent does the planning work.** Reads the source plan
document, constructs `plan-tasks.json` with 10 task entries (3 with
empty `waits_on`, 7 with dependencies forming a DAG).

**Step 4: Agent submits the task list.**
`koto next parent-42 --with-data @plan-tasks.json`:
- `handle_next` reads state file, compiles template
- advance loop at `plan_and_await`
- validates the evidence against the accepts schema
- appends `EvidenceSubmitted { fields: { tasks: [...], submitter_cwd: "..." } }`
- re-evaluates the `children-complete` gate: 0 matching children
  on disk, but the gate reads the batch definition from the just-
  appended evidence and reports `total: 10, completed: 0,
  pending: 10, ready: 3, blocked: 7` (the updated
  `evaluate_children_complete` no longer errors on "no children
  found" when the state has a `materialize_children` hook)
- gate returns `Failed`; the transition's `when` clause
  (`gates.done.all_complete: true`) doesn't match; advance loop
  stops at `plan_and_await`
- `handle_next` calls `run_batch_scheduler(final_state =
  "plan_and_await", ...)`
- scheduler finds `materialize_children` on `plan_and_await`
- parses the task list from the latest-epoch `EvidenceSubmitted`
- builds DAG, runs whole-submission validation (R3, R4, R5, R6, R8,
  R9); R0 already ran pre-append; R1/R2 run per-task and accumulate
  in `SchedulerOutcome.errored`
- classifies: all 10 tasks `NotYetSpawned`; 3 with empty
  `waits_on` are `Ready`; 7 are `BlockedByDep`
- for each `Ready` task, calls
  `init_state_file("parent-42.issue-1", header, initial_events)`
  atomically. The initial event list is
  `[WorkflowInitialized, Transitioned → <child initial state>]`.
- returns `SchedulerOutcome::Scheduled { spawned_this_tick:
  ["parent-42.issue-1", "parent-42.issue-3", "parent-42.issue-5"],
  blocked: [...7...] }`
- response serialized: outer shape is `gate_blocked`, attached
  `scheduler` field lists what was spawned, batch view shows
  `ready: 0, blocked: 7, in_progress: 3, completed: 0`
- response returned to the agent

**Step 5: Agent starts driving children — the parent-to-child
handshake.** There is no koto-level "jump to child" command. The
agent simply starts calling `koto next` on a different workflow
name. From koto's perspective, `parent-42.issue-1` is just another
workflow — it has its own state file, its own event log, and its
own directive loop. In the parallel-batch pattern, the coordinator
agent spawns N worker sub-agents (one per ready child). Each
worker runs its own drive loop:

```
loop:
  response = koto next parent-42.issue-1
  if response.action == "done": break
  if response is evidence_required: do the work, collect evidence
  koto next parent-42.issue-1 --with-data @work.json
```

Three workers run in parallel for the three initially-ready
children. Each operates on its own state file — no shared state,
no races, no locks.

**Step 6: A child terminates.** Worker for `parent-42.issue-1`
reaches a terminal response. Its drive loop exits. The worker
signals the coordinator (application-level, e.g., Task tool
return value) that child-1 is done.

**Step 7: Child-to-parent handshake.** Same shape in reverse:
the coordinator calls `koto next parent-42`. There's no "switch
back to parent" command — the coordinator just starts driving
the parent's workflow name again. Inside koto:
- advance loop at `plan_and_await` (state hasn't changed)
- gate re-evaluates: now sees `parent-42.issue-1` on disk in a
  terminal non-failure state. Output: `total: 10, completed: 1,
  pending: 9, in_progress: 2, ready: 1, blocked: 6`
- gate still `Failed`; advance loop stays at `plan_and_await`
- scheduler runs: re-classifies all 10 tasks. Issue-2, which
  depended only on issue-1, is now `Ready`. Issue-2 gets
  `init_state_file`'d.
- response: `gate_blocked` with updated batch view, the
  scheduler outcome shows `parent-42.issue-2` as newly spawned
- coordinator spawns a new worker for issue-2

**Step 8: Steps 5–7 repeat.** Each time a child terminates, the
coordinator re-ticks the parent, the scheduler spawns newly-
unblocked tasks, the coordinator starts new workers. Up to 10
workers run in parallel at peak (minus however many are already
terminal). The coordinator serializes its own `koto next
parent-42` calls (one at a time) per the Concurrency Model
section; the workers run independently on their own state files.

**Step 9: Last child terminates, batch completes.** Coordinator
ticks parent. Gate re-evaluates: all 10 children terminal,
`all_complete: true`. The transition `when: { gates.done.
all_complete: true }` matches. Advance loop transitions
`plan_and_await → summarize`. `summarize` is terminal.
`handle_next` calls `run_batch_scheduler` on `summarize`, which
finds no hook and returns `NoBatch`. Response carries `action: "done"`.

**Protocol summary.**

- **The agent explicitly picks which child to drive.** Each
  child is a named workflow (e.g., `parent-42.issue-1`) driven
  by `koto next parent-42.issue-1` like any other workflow. The
  `.` in the name is a naming convention, not a hierarchy
  operator. When the scheduler spawns 3 ready children, the
  agent decides which to work on, in what order, and whether to
  run them in parallel or sequentially. koto materializes state
  files; the agent executes.
- **No session context switching.** There is no `koto enter-child`
  or `koto return-to-parent` command that changes a session
  context. To "switch" from driving the parent to driving a
  child, the agent simply calls `koto next` with a different
  workflow name. To "return" to the parent after a child
  finishes, the agent calls `koto next parent-42` again. koto
  does not track which workflow the agent is currently working on.
- **The batch view tells the agent what's available.** The
  `koto status parent-42` response (and the scheduler outcome
  attached to each `koto next parent-42` response) lists which
  children are in progress (`in_progress`), which are newly
  ready to be driven (`ready` — they've been spawned but not
  started yet), and which are blocked waiting on dependencies
  (`blocked`). The agent reads these lists and makes its own
  scheduling decisions.
- **Parent and child state machines are independent.** A child
  can itself be a parent of its own batch (via `koto init
  --parent <child-name>` — the v0.7.0 primitive) without the
  outer parent needing to know.

**Details from the Step 4 walkthrough, in code terms:**

1. Agent writes `plan.json` with the task list and calls
   `koto next parent --with-data @plan.json`.
2. `handle_next` reads the state file, compiles the parent template,
   runs the advance loop. The advance loop validates the evidence
   against the `accepts` schema (the `tasks` field is declared
   `type: tasks`, `required: true`), appends an `EvidenceSubmitted`
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
   - Builds the DAG and runs whole-submission validation (R3, R4,
     R5, R6, R8, R9); R0 already ran pre-append; R1/R2 run per-task
     and accumulate in `SchedulerOutcome.errored`. Cycles, dangling
     refs, and duplicate names fail the whole submission with
     `BatchError::InvalidBatchDefinition`. For dynamic
     additions where the cycle emerges only from the merge of
     original + appended tasks, the scheduler rejects the
     resubmission before any new spawn happens; already-spawned
     children from earlier submissions are untouched.
   - Classifies each task: `Terminal` (child exists and is terminal
     non-failure), `Failed` (child exists and is terminal with
     `failure: true`), `Skipped` (child exists and has
     `skipped_marker: true`), `SpawnFailed` (a prior tick's
     `init_state_file` call errored and no child state file exists;
     surfaces with `outcome: spawn_failed` and feeds the
     `spawn_failed` aggregate), `Running` (child exists but not
     terminal), `NotYetSpawned` but `Ready` (dependencies all
     Terminal), `NotYetSpawned` but `BlockedByDep` (waits on
     non-Terminal task), or `NotYetSpawned` but `ShouldBeSkipped` (a
     dependency is Failed and `failure_policy` is `skip_dependents`).
     Whole-submission validation runs R0, R3, R4, R5, R6, R8, R9 in
     that order — R4's dangling-ref check precedes R8's spawn-time
     immutability so a mistyped `waits_on` does not shadow as a
     mutation rejection against an unrelated spawned entry.
   - For each `Ready` task, calls `init_state_file` via a helper
     refactored from `handle_init`, passing the parent's
     `template_source_dir` as the resolution base. The child name is
     `<parent>.<task.name>`.
   - For each `ShouldBeSkipped` task, calls `init_state_file` with
     the child's real template and initial events
     `[WorkflowInitialized, Transitioned → <skipped_marker_state>]`
     plus a context write `skipped_because: <failed_task>`. F5
     (Decision 1 compile rule) ensures every batch-eligible child
     template declares a reachable `skipped_marker: true` state.
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

**Retry:** See Decision 5.4 for the canonical sequence. In brief:

1. Parent is at a post-batch analysis state (e.g., `analyze_results`)
   after `children-complete` passed with some failures and skips.
2. Agent submits
   `koto next parent --with-data '{"retry_failed": {"children": ["parent.issue-2"], "include_skipped": true}}'`.
3. `handle_retry_failed` intercepts in `handle_next` BEFORE
   `advance_until_stop` runs, following the sequence at Decision 5.4:
   a. Validate R10 (retry set non-empty; each named child exists and
      has outcome `failure` or `skipped`; all-or-nothing).
   b. Append `EvidenceSubmitted { retry_failed: <payload> }` to the
      parent log.
   c. Append the clearing `EvidenceSubmitted { retry_failed: null }`
      event to the parent (under `CloudBackend`, `sync_push_state`
      the parent log here — push-parent-first per Decision 12 Q6
      eliminates phantom child epochs on sync failure).
   d. For each child in the downward closure of the retry set:
      append `Rewound` targeting the initial state when the child's
      outcome is `failure`; delete-and-respawn when the child's
      current state carries `skipped_marker: true` (a `Rewound`
      event would not reach a non-skipped-marker state).
   e. Return control to the advance loop, which fires the
      template-declared transition on
      `when: evidence.retry_failed: present` on the next tick.
4. The normal scheduler tick then runs on the rewound or respawned
   children, sees them as non-terminal, and the usual flow (wait,
   gate re-evaluates, terminate, etc.) resumes.

### Concurrency model

A common consumer pattern is for an orchestrator agent (e.g.,
shirabe's `work-on-plan.md`) to spawn multiple sub-agents, each
driving one child workflow in parallel. This design supports that
pattern with serialization enforced at the koto layer, not at the
caller.

**Parallelism where it works naturally.** Each child workflow has
its own state file. Once the scheduler has spawned N ready
children in a tick, the orchestrator can drive them concurrently —
`koto next child-1`, `koto next child-2`, ..., `koto next child-N`
all operate on distinct state files with distinct event logs. No
shared state, no races, no locks needed. This is where the
parallelism lives.

**Serialization for batch parents (Decision 12).** Two concurrent
`koto next parent` calls can race on two surfaces: the parent's
append-only event log (sequence collision) and the child
`init_state_file` rename (TOCTOU between `exists` and `rename`).
Decision 12 closes both at the koto layer:

1. **Advisory flock on batch parents (Q3).** `handle_next` takes a
   non-blocking advisory lock on `<session>/<parent>.lock` before
   reading the parent's log. A concurrent tick hits the lock and
   returns a typed `concurrent_tick` error immediately; callers
   can back off and retry. Non-batch workflows are unaffected —
   the lock is scoped to parents whose current state carries a
   `materialize_children` hook.
2. **`renameat2(RENAME_NOREPLACE)` on Linux; `link()` + `unlink()`
   fallback on other Unixes (Q2).** The `init_state_file` rename
   step fails loudly on collision instead of silently overwriting,
   so concurrent spawns can never clobber a child that already
   exists on disk.
3. **`materialized_children` ledger (Q1).** Renamed the per-tick
   `spawned` field to `spawned_this_tick` and added the
   `materialized_children` ledger — every child on disk for this
   parent, with outcome and current state. Consumers dispatch off
   the ledger for idempotency rather than reacting to the per-tick
   observation.
4. **Push-parent-first retry ordering (Q6).** `handle_retry_failed`
   appends the parent's clearing event (and pushes it under cloud
   sync) before any child `Rewound` or respawn write, so a sync
   failure can never leave phantom child epochs referencing a
   retry the resolved parent log does not record.
5. **Tempfile sweep on resume (Q7).** `repair_half_initialized_children`
   removes any `.koto-*.tmp` files left behind by a crashed
   `init_state_file` call before the scheduler classifies children
   on the next tick.

The invariant is now koto-enforced for batch parents: a second
concurrent tick sees a typed error, not a silent race. Callers
running a coordinator + N workers pattern still get the natural
parallelism on the child side — workers each hold their own child
state file — and they see a clean error when two ticks race on the
parent.

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
(minus however many are already terminal). The coordinator
naturally serializes its own `koto next parent` calls; if two
ticks ever overlap, the advisory flock ensures the second returns
`concurrent_tick` rather than racing. This scales linearly with
task count on the child side and stays O(1) on the parent side.

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
- **Decision 12 Q2:** `renameat2(RENAME_NOREPLACE)` on Linux,
  `link()` + `unlink()` fallback on other Unixes for the final
  rename step in `init_state_file`. Release notes pin Linux ≥ 3.15
  as the minimum supported kernel.
- **Decision 12 Q3:** advisory non-blocking `flock` on
  `<session_dir>/<workflow>.lock` acquired in `handle_next` for
  batch parents (detected by current state's `materialize_children`
  hook or any `SchedulerRan` / `BatchFinalized` event in the log).
  Lock released on function exit via file-handle drop; lock file
  contents are empty — process-lifetime mutex, not persistent
  state.
- **Decision 10 / 2 amendment:** `WorkflowInitialized` event gains
  a `spawn_entry: Option<SpawnEntrySnapshot>` field capturing the
  canonical-form task entry (template, vars, waits_on). Additive
  and `#[serde(default, skip_serializing_if = "Option::is_none")]`
  for round-trip compatibility with pre-Decision-10 children.

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
- Accepts schema `VALID_FIELD_TYPES` gains `tasks`
- `#[serde(deny_unknown_fields)]` on `SourceState` only (Decision 3);
  pre-merge audit confirms no source templates rely on unknown fields
- Tests for round-trip compat with v0.7.0 state files and templates

**Sub-phase 2b — template vocabulary for batch:**
- `TemplateState` gains `materialize_children`, `failure`,
  `skipped_marker` fields
- `MaterializeChildrenSpec` (with required `default_template`) and
  `FailurePolicy` types
- `derive_expects` updated to auto-generate `item_schema` on
  accepts fields linked to a `materialize_children` hook
  (Decision 8); the schema is koto-generated, not template-authored
- `CompiledTemplate::validate` extended with E1–E10 errors and W1–W5
  warnings for `materialize_children`. W4 (Decision 9): warn when a
  batch state routes only on `all_complete: true`. W5 (Decision 13):
  warn when a `failure: true` state has no path writing
  `failure_reason`. F5 (Decision 9): warn when a batch-eligible
  child template lacks a scheduler-reachable `skipped_marker: true`
  state.
- **Decision 11:** R0-R9 runtime rules run as pre-append pure
  functions of the submitted payload. R0 (non-empty tasks), R8
  (spawn-time immutability against `spawn_entry` snapshot), R9
  (name regex + reserved names), R6 depth definition (node count
  along longest root-to-leaf path).
- Typed enums replace free-string fields in `BatchError`,
  `InvalidBatchReason`, `LimitKind`, `SpawnErrorKind`,
  `InvalidRetryReason`. Generated from Decision 11 Q8-Q11.
- `failure_reason` context-key convention documented in koto-author
  skill (paired with W5 warning).
- Tests for each compiler rule and each field default

**No batch scheduler yet.** This PR unlocks the template vocabulary
but doesn't actually spawn children from evidence. Authors can
declare `materialize_children` and the compiler will validate it,
but runtime is a no-op.

### Phase 3: Scheduler, retry, and observability (Decisions 5, 6)

Wire up the actual scheduler and observability.

**Prerequisite:** extend the when-clause engine with an
`evidence.<field>: present` matcher (Decision 9 Part 2 relies on
this for the template-declared retry transition). In-scope for this
phase's first PR; failing this prerequisite blocks retry routing.

**Deliverables:**
- New module `src/cli/batch.rs` with `run_batch_scheduler`,
  `derive_batch_view`, `handle_retry_failed`, `classify_task`,
  `build_dag`, DAG cycle detector, and `BatchError` enum
- **Decision 11:** new `NextResponse::Error` variant in
  `src/cli/next_types.rs` emitting `action: "error"`. `NextError`
  gains an optional sibling `batch: Option<BatchErrorContext>`
  field. No new `NextError::Batch` variant — the existing struct
  shape is preserved.
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
  `skipped`, `blocked`, `spawn_failed`, per-child `outcome`,
  `failure_mode`, `skipped_because`, `blocked_by`), plus the
  Decision 9 derived booleans (`all_success`, `any_failed`,
  `any_skipped`, `needs_attention`).
- **Runtime reclassification** in the scheduler tick (Decision 9
  Part 5). Each tick walks existing children: stale skip markers
  (dependency outcomes no longer failed) are delete-and-respawned;
  real-template running children whose upstream just flipped to
  failure are delete-and-respawned as skip markers. Guard against
  thrashing by reclassifying only on dep-outcome change.
- **`BatchFinalized` event** (Decision 13). Appended when the
  `children-complete` gate first evaluates `all_complete: true` on
  a state with `materialize_children`. Carries the final
  `BatchView` snapshot.
- **`batch_final_view` on `done` responses** (Decision 13),
  populated from the most recent `BatchFinalized` event.
- `koto status` extended with optional `batch` section; emits
  `batch.phase: "current"` when the current state has a hook,
  `batch.phase: "final"` when replaying from `BatchFinalized`.
  `synthetic: true` marker on children whose current state has
  `skipped_marker: true`. `skipped_because_chain` alongside
  singular `skipped_because`. `reason_source` disambiguation on
  per-child `reason`.
- `koto workflows --children` extended with per-row batch metadata
  (same fields).
- **`retry_failed` advance-loop integration** (Decision 9 Part 2):
  CLI-layer interception before `advance_until_stop`; template-
  declared transition fires on `when: evidence.retry_failed:
  present`; clearing event writes per Decision 12 Q6 ordering.
- **`reserved_actions` sibling response field** (Decision 9 Part 3):
  synthesized by `handle_next` after gate evaluation when
  `any_failed` or `any_skipped` is true. Carries ready-to-run
  `invocation` strings.
- **`scheduler.feedback` map** (Decision 10): per-entry outcomes
  keyed by agent-submitted short name, plus `orphan_candidates`
  for rename detection.
- **`materialized_children` ledger on scheduler output** (Decision
  12 Q1): complete set of on-disk children with outcome and state.
  Agents key on this for idempotent worker dispatch.
- **`SchedulerRan` event appended on non-trivial ticks** (Decision
  11 Q5): per-tick `spawned_this_tick`, `already`, `blocked`, `skipped`,
  `errored`. Skips append when every task is `already`/`blocked`.
- **Decision 14:** per-task spawn error accumulation via
  `SchedulerOutcome.errored` (never halt); split `BatchError` into
  `TemplateNotFound` and `TemplateCompileFailed`;
  `SchedulerWarning::MissingTemplateSourceDir` and
  `StaleTemplateSourceDir` emit via `SchedulerOutcome.warnings`;
  `Path::exists()` probe once per tick on `template_source_dir`.
- **Decision 12 Q4-Q7:** `koto session resolve <parent>` reconciles
  children by default; `sync_status` and `machine_id` response
  fields emit under `CloudBackend`; push-parent-first ordering in
  `handle_retry_failed` under `CloudBackend`; per-tick tempfile
  sweep in `repair_half_initialized_children` scoped to the
  current parent, threshold 60 seconds.
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
  atomically re-initializes them. Also runs the tempfile sweep
  above.
- **Submission-time hard limit enforcement.** At evidence
  submission, reject task lists exceeding hard caps: 1000 tasks,
  10 `waits_on` entries per task, DAG depth of 50. Return
  per-limit `InvalidBatchReason` variants with actual and limit
  values. These are hard rejections, not soft recommendations —
  easier to loosen limits in v2 than to tighten them after users
  rely on larger batches.
- Integration tests for: linear batch, diamond DAG, mid-flight
  append, failure with skip-dependents, `retry_failed` recovery
  (including `reserved_actions` discovery), crash-resume
  walkthrough from Decision 5's section 5.5, limit-exceeded
  rejection, concurrent-tick `flock` contention, cloud-sync
  push-parent-first retry ordering, runtime reclassification
  sweep, `spawn_failed` per-task accumulation.
- koto-author and koto-user skill updates covering
  `materialize_children`, the extended gate output, the
  `retry_failed` action, `materialized_children` as the dispatch
  ledger, the coordinator-owns-parent / workers-own-children
  partition (Decision 12 Q8), `sync_status` interpretation,
  `failure_reason` convention, and the `synthetic: true` marker.
- **Documentation update.** Documentation must use "the coordinator
  drives the parent; workers drive only their own children" framing
  rather than "any caller can drive the parent." The partition is a
  caller contract enforced at runtime by the advisory flock.

**End of Phase 3, the feature is complete.** The shirabe
work-on-plan design (tsukumogami/shirabe#67) can begin rewriting its
spawn loop against the new surface.

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
- **`cancel_tasks` reserved evidence action** (Decision 10). Removal
  of a spawned task is deferred to v1.1. v1 documents the non-
  feature: omitting a previously-named task from a later submission
  is a no-op, not a cancellation signal. Operators needing immediate
  removal can manually delete the child's state file; this leaves
  the parent's view inconsistent and is an unsupported escape hatch.
- **`koto session retarget`** for rewriting `template_source_dir` on
  state file headers after cross-machine migration (Decision 14).
  Cross-machine portability is documented with a runtime warning;
  the mechanism fix is future work.

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
- **Retry, mutation, and concurrency hazards are closed.** The retry
  path is reachable out of the box (Decision 9): `analyze_failures`
  intermediate state, W4 compile warning, template-declared
  transition, `reserved_actions` discovery surface. Mutation
  semantics are well-defined (Decision 10): R8 locks spawned task
  entries, union-by-name is precise, per-entry feedback removes
  silent drops. Rejected submissions carry typed discriminators
  through a unified `action: "error"` envelope and validation runs
  pre-append (Decision 11). Concurrency TOCTOU is closed at the
  kernel level via `renameat2(RENAME_NOREPLACE)` / POSIX `link()`
  fallback plus an advisory flock (Decision 12). Batch views survive
  terminal transitions via `BatchFinalized` events and
  `batch_final_view` fields (Decision 13). Path-resolution
  contradictions are resolved: per-task failures, absent-source-dir
  fallback, split variant, node-count depth (Decision 14).
- **Path-resolution diagnostics.** New `SchedulerWarning::
  MissingTemplateSourceDir` and `SchedulerWarning::
  StaleTemplateSourceDir` variants surface cross-machine
  portability issues at run time through `SchedulerOutcome.warnings`,
  so agents see the root cause instead of a generic
  `TemplateNotFound` per task.

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
- **When-clause engine extension is a prerequisite** for retry
  routing. Decision 9 Part 2 relies on an
  `evidence.<field>: present` matcher. Phase 3's first PR adds
  this to the when-clause evaluator; if it slips, retry-routing
  templates cannot fire their transitions.
- **F5 puts authoring burden on child templates.** Any template
  that participates in a batch must declare a reachable
  `skipped_marker: true` terminal state. The F5 warning is advisory
  because batch-eligibility isn't statically knowable at child-
  compile time, but authors who ignore it hit a scheduler error at
  first skip.
- **Linux kernel 3.15+ requirement.** `renameat2` (Decision 12 Q2)
  needs a 2014-era kernel. Non-Linux Unixes (macOS, BSD, illumos)
  use a POSIX `link()` + `unlink()` fallback. Release notes must
  pin the minimum.
- **Response envelope surface grows.** New top-level fields
  (`reserved_actions`, `batch_final_view`, `sync_status`,
  `machine_id`) plus new scheduler-object fields
  (`materialized_children`, `feedback`, `errored`, `warnings`,
  `spawned_this_tick` rename) expand the envelope. Consumers that
  pattern-match by field name are unaffected; consumers that
  compare whole-response shapes would need to tolerate additions.
- **`cancel_tasks` deferral.** Operators who mis-submit a task name
  have no v1 recovery path other than manual state-file deletion.
  v1.1 closes this gap.
- **Retry-induced respawns serialize within a single tick.** The
  `ready_to_drive` flag on `MaterializedChild` is required reading
  for workers: a retry tick can insert newly-respawned dependents
  into the ledger before their `waits_on` ancestors have finished
  running. Agents that dispatch on bare ledger presence (without the
  flag) risk starting a dependent against stale upstream state. This
  is a contract workers must honor; the typed field makes it
  enforceable rather than discoverable-by-feedback-only.
- **`orphan_candidates` detection is advisory and post-spawn.** The
  scheduler flags signature-matching entries by comparing the new
  task's canonical-form `vars` + `waits_on` signature against
  already-spawned children. The detection runs before the new child
  is spawned in the current tick: when the match fires pre-spawn,
  the scheduler PAUSES the new spawn and emits the warning with
  `outcome: "errored", kind: "orphan_candidate_pending"` on the
  feedback entry so the agent can investigate before duplicate work
  lands. Agents resolve by resubmitting with the duplicate removed
  (or by explicitly acknowledging the match in a future v1.1
  primitive). Post-spawn detection remains advisory when a
  previously-unseen match surfaces after both children exist.
- **Cross-level retry is unsupported in v1.** `retry_failed` on a
  child that is itself a batch parent rejects with
  `InvalidRetryReason::ChildIsBatchParent`. Users of nested batches
  retry at the inner level first, then bubble up. v1.1 may relax
  this with cascading retry.

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
- **Surface common authoring mistakes at compile time.** W4
  (`materialize_children` states that route only on
  `all_complete`) and W5 (`failure: true` states with no
  `failure_reason` writer) catch two frequent authoring mistakes
  before templates ship. F5 warns on batch-eligible child
  templates that lack a scheduler-reachable `skipped_marker`
  state.
- **Per-task accumulation recovers from spawn failures.** A
  submission with 10 valid tasks and 1 bad template resolves: the
  10 spawn; the 1 surfaces as `BatchTaskView.outcome:
  spawn_failed` with a `spawn_error` payload. The agent fixes the
  single entry and resubmits.
- **Reference template demonstrates retry-reachable routing.**
  The shipped `coord.md` uses `analyze_failures` + `any_failed`
  guards so authors who copy it get the retry path for free. Both
  skills carry matching worked examples.

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

- **Multi-agent shared-parent information flow.** Multiple agents
  driving the same parent workflow (e.g., a coordinator plus worker
  sub-agents) all see the parent's response bodies. An R8
  `SpawnedTaskMutated` error fired by agent B's resubmission reveals
  the `template` and `vars` fields agent A originally submitted.
  Inside the trusted-collaborator model, this is not a violation —
  both agents are already authorized to read the parent's event log.
  Documented here so observers understand that submission content
  is visible across agents sharing a parent.

- **Secret-rotation gotcha under R8 rejection.** A `SpawnedTaskMutated`
  error response (Decision 11) includes the full `changed_fields` array
  with both the spawned value and the submitted value per field. If an
  agent attempts to rotate a secret held in `vars` by resubmitting the
  task list with a new value, both the old and new values appear in the
  error response body. Response logs, shell histories, and any observer
  consuming the response will see the old secret. Agents rotating
  secrets should not do so via resubmission of an already-spawned task.
  A best-effort mitigation lives in the scheduler: values for `vars`
  keys matching any of `*_TOKEN`, `*_SECRET`, `*_KEY`, `*_PASSWORD`,
  `*_COOKIE`, `*_AUTH`, `*_BEARER`, `*_ACCESS_*`, `*_REFRESH_*`, or
  literal `DATABASE_URL` / `DATABASE_PASSWORD` are redacted in the
  diff payload before serialization. The redaction is keyed-name
  based; secrets embedded inside non-matching values (e.g., a token
  nested in a JSON blob under a `config` key) are NOT redacted.
  Template authors remain responsible for not submitting secrets
  they don't want logged. The replacement value is the literal
  string `"[REDACTED]"`, not an object like `{"redacted": true}`.
  Agents can key on that exact string when parsing `changed_fields`
  diffs.

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

- **Absolute `template` paths in task entries bypass stale-path
  warnings.** Decision 14's `StaleTemplateSourceDir` fires only for
  relative-path fall-through through `template_source_dir`. If a task
  entry carries an absolute path valid on the submitting machine
  (e.g., `/Users/alice/src/repo/templates/child.md`), that path rides
  the evidence log unchanged; a sibling machine receiving the synced
  log sees the absolute path, attempts resolution, and emits
  `TemplateNotFound` per-task instead of `StaleTemplateSourceDir`.
  Known limitation in v1: cross-machine portability checks cover
  header-based relative resolution only. Submitters coordinating
  across machines should prefer relative paths rooted at the parent's
  `template_source_dir`.

### Observer-visible output

- **The `reason` field is sourced from an explicit context key.**
  Documented in Decision 6 above: batch output's per-child `reason`
  comes from the `failure_reason` context key written by the child,
  not from scraped stderr or raw tool output. Template authors
  writing failure-state handlers must write a sanitized message to
  this context key. This prevents accidental leakage of paths, env
  var values, or secrets into batch status responses that observers
  (other agents, human operators) consume.

- **Error bodies echo agent-submitted content.** Decisions 11 and 14
  expand the error-response surface: `TemplateNotFound.paths_tried`
  lists the absolute paths the scheduler attempted, which include the
  parent's `template_source_dir` and the `submitter_cwd` captured at
  submission time. `TemplateCompileFailed.compile_error` echoes the
  compiler's diagnostic text for the failing child template, which may
  include snippets of that template's body. These are the same classes
  of data already persisted in state files per the "Persisted path
  information" subsection above, but the consumer set is broader:
  response logs, CI output, shared debugging transcripts. Treat the
  error-response body as equivalently sensitive to the state file.

- **`machine_id` on cloud-mode responses.** Decision 12 adds
  `machine_id` and `sync_status` as optional top-level fields on
  responses when `CloudBackend` is active. `machine_id` is a stable
  per-machine identifier drawn from the cloud-sync configuration (same
  value cloud sync already uploads with every state-file push), not a
  new capability or a hostname leak beyond what cloud-sync observers
  already see. Users sharing response transcripts for debugging should
  be aware that `machine_id` reveals which of their configured devices
  produced the response.

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

Decision 12's `koto session resolve --children=auto` default extends
the existing conflict-resolution path to reconcile per-child state
files alongside the parent log. The `auto` mode trusts the remote
version for both parent and child reconciliation, matching the
existing parent-log behavior. Users with partial bucket-integrity
concerns can run `--children=skip` to leave child state files
untouched, or `--children=accept-local` / `--children=accept-remote`
for explicit side selection. The trust model is unchanged — a
compromised cloud-sync bucket can already modify parent state files
under the existing design. The operational surface grows because
children are now in scope for `resolve`.

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

No new direct dependencies are introduced. The design uses `tempfile`
(pre-existing), `serde` (pre-existing), and the existing koto
session backend. The `init_state_file` refactor moves existing crate
usage to new call sites.

Decision 12's `renameat2(RENAME_NOREPLACE)` call on Linux uses `libc`,
which is already a direct dependency in `Cargo.toml` — not a new
entry. The macOS and BSD fallback uses `std::fs::hard_link` followed
by `std::fs::remove_file` from the standard library; no dependency
expansion. Decision 12's advisory `flock` on the parent's session
lockfile calls `libc::flock` directly, the same primitive already
used by `LocalBackend`'s `ContextStore` writes
(`src/session/local.rs:211-244`), again no new direct dependency.

The `flock` is advisory: POSIX flock only blocks cooperating callers.
A local process not participating in the locking protocol can still
open and modify the parent state file. Inside koto's trust model this
is already equivalent to filesystem-level access to the session
directory, which grants full control over workflow state regardless
of any lock. The lock's purpose is to serialize concurrent `koto
next` invocations by well-behaved callers, not to defend against
local-root or same-UID attackers.
