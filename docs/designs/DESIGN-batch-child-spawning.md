---
status: Proposed
problem: |
  Koto v0.7.0 lets a parent workflow spawn and wait for children, but the
  consumer has to run the spawn loop themselves: query which children are
  ready, spawn them, check for completion, spawn the next wave. For
  workflows where the full task set is known upfront (e.g. a plan parsed
  into a DAG of GitHub-issue children), this loop forces every consumer to
  re-implement scheduling in SKILL.md prose, which is brittle beyond a
  handful of tasks and blocks shirabe's adoption of koto for hierarchical
  templates (tsukumogami/shirabe#67). This design specifies a declarative
  alternative: the parent submits a task list as evidence, and koto owns
  materialization, dependency-ordered scheduling, completion detection,
  and failure routing end to end.
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
  module `src/engine/batch.rs` holds the scheduler; `handle_next` calls
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

### Decision 3: Forward-compat diagnosability

A batch-hook template compiled against a pre-batch koto binary
silently no-ops today: `CompiledTemplate` does not set
`#[serde(deny_unknown_fields)]`, so serde ignores the unknown
`materialize_children` field. The user sees their workflow "not
spawning children" with no error or warning.

#### Chosen: narrow `deny_unknown_fields` on `SourceState` and `TemplateState`

Add the attribute to `SourceState` and `TemplateState` only, in the
same PR as the new `materialize_children` field. Gate on a pre-merge
audit of existing template fixtures to confirm no templates rely on
unknown fields as annotations.

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

**5.4 `retry_failed` evidence action.** The parent submits:

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
3. For each child in the closure, calls an internal
   `rewind_to_initial(name)` helper — reusing the existing
   `Rewound` event machinery from `handle_rewind` — to create a
   new epoch that drops prior evidence.
4. Appends a clearing `{"retry_failed": null}` event to the parent
   so the next scheduler tick doesn't re-rewind.

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

Both extensions call a shared `derive_batch_view` helper in
`src/engine/batch.rs` that reuses the scheduler's `classify_task`
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

3. **One scheduler PR.** Adds `src/engine/batch.rs`, wires
   `run_batch_scheduler` into `handle_next` after `advance_until_stop`,
   extends `evaluate_children_complete`'s output schema with the
   Decision 5 fields, adds `derive_batch_view` for Decision 6,
   implements the `retry_failed` evidence action, and updates
   `koto status` / `koto workflows --children` to expose batch
   metadata.

See Implementation Approach below for the phased landing sequence.
