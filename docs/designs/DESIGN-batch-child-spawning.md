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

- **Backward compatibility for old state files and templates.**
  Pre-batch templates must compile and run unchanged. Pre-batch state
  files must read unchanged. The migration story for adding batch
  support to existing templates should be additive (a new optional
  field), not a format bump unless the forward-compat diagnosability
  problem forces one.

## Decisions Already Made

These were settled during exploration and should be treated as
constraints, not reopened.

- **Primary model: Reading A (flat declarative batch with `waits_on`).**
  The parent owns a task list with sibling-level dependencies. Reading B
  (nested via `koto init --parent`) remains unchanged from v0.7.0 for
  genuinely hierarchical work. The GH-issue use case requires sibling
  ordering, which nesting cannot express; Reading A is the answer to
  #129.

- **Storage strategy: full derivation from on-disk child state + event
  log.** The parent persists nothing new. The batch definition lives
  in the existing `EvidenceSubmitted` event; spawn records are child
  state files discovered via `backend.list()` filtered by
  `parent_workflow`. Idempotency is the existing `backend.exists()`
  check.

- **Insertion point: CLI-level scheduler tick in `handle_next`,
  post-`advance_until_stop`.** The advance loop in
  `src/engine/advance.rs` stays pure (I/O-free, closure-driven). A new
  module `src/cli/batch.rs` holds the scheduler; `handle_next` calls
  it once the advance loop has settled on the final state.

- **Child naming: deterministic `<parent>.<task>`.** Couples child name
  to parent name; parents can't be renamed, so the coupling is
  acceptable. Gives free idempotency via `backend.exists()`.

- **Default failure policy: skip-dependents, per-batch configurable.**
  Alternatives (`fail-fast`, `continue-independent`, `pause-on-failure`)
  are opt-ins declared in the submitted evidence's `failure_policy`
  field. No global config, no per-task `trigger_rule` in v1.

- **CLI extension: `--with-data @file.json` prefix.** Mirrors
  `curl -d @file` and `gh api -f`. Size cap (1 MB) applies to resolved
  content.

- **Template extension: new `json` field type in accepts schema.**
  Extends the `VALID_FIELD_TYPES` allow-list to permit array/object
  evidence. Unlocks structured evidence beyond batch spawn.

- **Template declaration: state-level hook pointing at an accepts
  field.** The exact name (`batch`, `materialize_children`,
  `batch_spawn`) is a surface detail to be picked in this design. What's
  settled: the declaration lives on `TemplateState`, references an
  accepts field, and is validated at compile time.

- **Per-task `trigger_rule` vocabulary deferred.** The simpler per-batch
  `failure_policy` ships first. Airflow-style per-task rules can be
  added later if real use cases need the granularity.

- **Adversarial demand-validation lead was skipped.** Issue #129 has
  a known blocked consumer (shirabe PR #67) and a clear acceptance
  criteria list. Demand is self-evident.

## Considered Options

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
