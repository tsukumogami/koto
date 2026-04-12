# Lead: Evidence shape and materialization trigger

## Question

What is the concrete shape of the declarative task list the parent submits,
and what template-level mechanism converts that list into materialized child
workflows? Answering this pins down (a) the JSON schema a parent agent passes
on `koto next --with-data`, (b) where in the template the author declares
"this state spawns children from a batch," and (c) what the compiler and
runtime must validate.

The v0.7.0 foundation already landed: `parent_workflow` lives on the state
file header (`src/engine/types.rs:24`), `koto init --parent` stamps it
(`src/cli/mod.rs:1118`), and the `children-complete` gate discovers children
by filtering `backend.list()` on that header field
(`src/cli/mod.rs:2498`). The primitive missing from the loop is a way for
the engine to *create* children itself rather than requiring the parent
agent to shell out and call `koto init --parent` once per child. Issue #129
is that primitive.

## Scope of this lead

This lead is about one thing: the contract between the parent agent and the
engine at the moment a batch is submitted. It does not decide whether the
engine also spawns the agent process (that remains out of scope --
koto-as-contract-layer stays; see DESIGN-hierarchical-workflows.md decision
driver "koto is a contract layer, not an execution engine"), nor does it
decide the failure-routing vocabulary (covered by the failure-routing lead)
or the dynamic-append semantics in depth (covered by the dynamic-additions
lead). It only answers: what does the submission look like on the wire, and
what in the template says "when this submission lands, materialize."

## 1. Task entry schema

A single task entry is a JSON object. The batch is a JSON array of these
entries.

```json
{
  "name": "issue-3",
  "template": "impl.md",
  "vars": {"ISSUE_NUMBER": "3"},
  "waits_on": ["issue-1", "issue-2"],
  "trigger_rule": "all_success"
}
```

### Field-by-field

| Field | Type | Required | Default | Purpose |
|-------|------|----------|---------|---------|
| `name` | string | yes | -- | Child workflow name. Used as `koto init --name` and as the join key for `waits_on` references. Subject to `validate_workflow_name()` (`src/discover.rs`). |
| `template` | string | yes | -- | Path to the child template source file. Resolved relative to the parent workflow's working directory (same rule `koto init` uses when reading the `--template` argument at `src/cli/mod.rs:1074`). |
| `vars` | object (map of string to string) | no | `{}` | Forwarded verbatim to `koto init --var KEY=VALUE`. Must resolve cleanly against the child template's `variables` block or materialization fails with the same error `resolve_variables` produces today (`src/cli/mod.rs:1096`). |
| `waits_on` | array of string | no | `[]` | Names of sibling tasks in the same batch that must complete before this one is materialized. An empty (or absent) list means the task is immediately ready. |
| `trigger_rule` | string enum | no | `"all_success"` | How to interpret the outcome of the tasks in `waits_on`. Values TBD -- the failure-routing sibling lead owns the exact vocabulary. At minimum `"all_success"` (default), `"all_done"`, and `"none_failed"` are likely. |

### Required vs optional -- rationale

- **`name` and `template`** are the only things the engine absolutely needs
  to call the same code path `handle_init` uses today. Everything else can
  be derived, defaulted, or inherited. Keep the required surface minimal so
  the happy path JSON is compact.
- **`vars`** is optional because some child templates declare no variables
  (or have all defaults). When a required variable is missing, the failure
  is raised at child-init time by `resolve_variables`, which gives a
  targeted error message per child rather than a batched-up compile-time
  guess.
- **`waits_on`** is optional because the degenerate case -- a flat parallel
  batch -- is common enough to deserve zero-boilerplate support. Omit the
  field for "run immediately."
- **`trigger_rule`** is optional because most batches want the same rule
  ("proceed if all prerequisites succeeded"). Requiring it on every entry
  would bloat plans like Example B below from ~20 lines to ~35 for no gain.
  Flagged as TBD here since the exact values come from the
  failure-routing lead.

### What's deliberately absent

- **No child-scoped overrides of `completion`.** The `completion` field on
  the `children-complete` gate (`src/template/types.rs:116`) is a
  parent-wide setting. Per-task completion semantics would force the
  parent's gate evaluator to group children by completion rule, which is a
  larger change than warranted. If a batch really needs heterogeneous
  completion conditions, authors can use multiple parent states with
  different gates and `name_filter` values.
- **No `priority` or `concurrency_limit` per task.** Concurrency control is
  an execution concern and koto isn't an executor. Whatever spawns the
  agent processes already has its own limits.
- **No `parent_workflow` field in the entry.** Implicit -- the engine uses
  the submitting workflow's name.

## 2. Template declaration of materialization

Four candidate shapes for how the template says "materialize children from
the submitted batch." Each is evaluated on four dimensions: compiler
friendliness, resume friendliness, composability with the existing
`children-complete` gate, and intrusiveness to koto's current model.

### Candidate (a): Frontmatter field on the template

Add a top-level frontmatter field that names the batch-submission state.

```yaml
---
name: fanout-parent
version: "1.0"
initial_state: plan
batch_spawn_state: plan
states:
  plan:
    accepts:
      tasks:
        type: string       # placeholder -- see section 3
        required: true
    transitions:
      - target: await_children
  await_children:
    gates:
      done:
        type: children-complete
    transitions:
      - target: review
        when:
          gates.done.all_complete: true
---
```

- **Compiler friendly**: add one optional field to `SourceFrontmatter`
  (`src/template/compile.rs:15`), validate it names a declared state with
  an `accepts` block, done. No new AST nodes.
- **Resume friendly**: neutral -- the materialization trigger is
  submission, which is already persisted in an `EvidenceSubmitted` event,
  so resume behavior doesn't change.
- **Composable**: the rest of the template is unchanged. A downstream
  `children-complete` gate behaves exactly as today.
- **Intrusive**: low -- one field, no new compiler concepts. But it smells
  wrong: every other state-level behavior (gates, accepts, transitions,
  integration) is declared *on the state itself*, not in frontmatter.
  Frontmatter is for workflow-global shape. Putting a state reference up
  there reverses the direction the rest of the format points.

### Candidate (b): State-level action verb

Introduce a new default-action-like concept (`materialize_children`) that
fires when the state's accepts block receives the batch field.

```yaml
states:
  plan:
    accepts:
      tasks:
        type: string      # placeholder
        required: true
    materialize_children:
      from_field: tasks
    transitions:
      - target: await_children
```

- **Compiler friendly**: adds a new `SourceState` field, a new compiled
  AST node (`MaterializeChildrenDecl`), and a validation rule (`from_field`
  must be a declared accepts field). Moderate surface expansion.
- **Resume friendly**: works. The engine triggers materialization at the
  same moment evidence is persisted; on resume, it reads the log, sees the
  evidence was already processed (because materialized children show up in
  `backend.list()`), and skips.
- **Composable**: plays nicely with `children-complete` -- they live in
  different states.
- **Intrusive**: medium. It introduces a "side-effect action" concept to
  koto, which today has only `default_action` (a shell command, not a
  state-machine primitive). That's a new category, not just a new variant.

### Candidate (c): New gate type (`batch-materialize`)

Model the materialization check as a gate. The gate is "true" when the
children have been created from the submitted batch.

```yaml
states:
  plan:
    accepts:
      tasks:
        type: string
        required: true
    gates:
      spawned:
        type: batch-materialize
        from_field: tasks
    transitions:
      - target: await_children
        when:
          gates.spawned.all_spawned: true
```

- **Compiler friendly**: extends the gate schema registry at
  `src/template/types.rs:193`. New required fields (`from_field`). The D3
  reachability check already verifies that every gate is routed; that
  keeps the pattern consistent.
- **Resume friendly**: yes -- gates are evaluated on every `koto next`, so
  a half-materialized batch from a crashed run re-evaluates on retry.
- **Composable**: awkward. A gate is logically a *precondition check*, not
  an action that mutates external state. Every existing gate is side-
  effect-free. `evaluate_gates` (`src/gate.rs:62`) would need to mutate
  backend state (create child sessions), which breaks the mental model
  and makes the blocking-conditions category (`src/gate.rs:258`) unclear:
  "temporal" and "corrective" don't describe "I need to create something
  on first call."
- **Intrusive**: high. Gates are read-only in the current design; turning
  one into a side-effecting action is a category violation.

### Candidate (d): Implicit -- any evidence key matching a reserved name triggers

The engine treats a reserved evidence key (e.g. `__tasks`, or `_spawn`) as
a special case: on submission, parse and materialize.

```yaml
states:
  plan:
    accepts:
      _spawn:
        type: string        # still a placeholder
        required: true
    transitions:
      - target: await_children
```

- **Compiler friendly**: trivially -- no new fields or validation. Just
  document that `_spawn` is reserved.
- **Resume friendly**: works like (b).
- **Composable**: fine, but the reservation is invisible at the template
  level. Authors have to know the magic name, and the compiler can't tell
  them they're using it correctly unless it special-cases the key.
- **Intrusive**: low mechanically, but high in the "hidden contract" sense.
  It mirrors how `gates` is already a reserved key on evidence submissions
  (`src/cli/mod.rs:542`), so there's some precedent. But the existing
  reservation *rejects* a name; this one *adds behavior*. That asymmetry
  is a red flag.

### Comparison

| Criterion | (a) frontmatter | (b) action verb | (c) new gate | (d) implicit |
|-----------|----------------|-----------------|--------------|--------------|
| Compiler surface | tiny | medium | medium | tiny |
| Validation locality | poor (ref from frontmatter to state) | excellent | excellent | poor (no explicit marker) |
| Resume safety | neutral | good | good | good |
| Matches current model | poor | good | poor (gates are pure) | poor |
| Discoverability | ok | excellent | excellent | poor |
| Template-format consistency | poor | excellent | fair | poor |

## 3. CLI surface for initial submission

### What exists today

`handle_next` takes a single `--with-data` argument (`src/cli/mod.rs:1263`)
that must be a JSON string. Size cap is 1 MB (`MAX_WITH_DATA_BYTES`,
`src/cli/mod.rs:29`, referenced at `src/cli/mod.rs:1297`). The payload is
parsed as `serde_json::Value` and the top-level must be a JSON object
(`validate_with_data_payload`, `src/cli/mod.rs:554`, and
`validate_evidence`, `src/engine/evidence.rs:51`).

Three blockers for the batch use case:

1. **`--with-data` accepts only raw JSON strings.** There is no `@file.json`
   syntax. Agents would have to shell-escape the entire plan, which breaks
   for anything non-trivial. Grep for `@` near `with_data` in
   `src/cli/mod.rs` finds nothing.

2. **The accepts schema only supports scalar field types.** The valid field
   types are `enum`, `string`, `number`, `boolean` (`VALID_FIELD_TYPES` in
   `src/template/types.rs:293`, enforced in `validate_field_type` at
   `src/engine/evidence.rs:107`). There is no `array` or `object` type.
   A task list is an array of objects, so today you can't declare
   `tasks: { type: array, items: ... }` in an accepts block.

3. **Nested JSON already works at parse time.** `validate_with_data_payload`
   accepts any `serde_json::Value`; it only rejects the reserved top-level
   `gates` key. Nested arrays and objects pass the JSON parser fine. The
   blocker is schema validation, not parsing.

### What needs to change

Two things, both small:

1. **Extend accepts field types with `json` (or `object` / `array`).** Add
   one new variant to `VALID_FIELD_TYPES` that matches any JSON value
   other than `null`. This is a surgical extension -- one line in the
   allow-list and one branch in `validate_field_type`. This unlocks every
   template feature that wants to submit structured data, not just batch
   spawning. (A more constrained `array-of-object` type is tempting, but
   it drags schema-of-schema into the compiler and is better deferred.)

2. **Add `@file` reading to `--with-data`.** Before calling
   `validate_with_data_payload`, check if the argument begins with `@` and,
   if so, read the remainder as a file path. This mirrors how `curl -d`
   and `gh api -f` work and is agent-friendly. A five-line wrapper around
   `std::fs::read_to_string` is enough.

### Exact command users would type

Using a file (recommended for batches beyond a handful of tasks):

```bash
koto next parent --with-data @plan-tasks.json
```

Where `plan-tasks.json` contains:

```json
{
  "tasks": [
    {"name": "issue-1", "template": "impl.md", "vars": {"N": "1"}},
    {"name": "issue-2", "template": "impl.md", "vars": {"N": "2"},
     "waits_on": ["issue-1"]}
  ]
}
```

Using an inline string (fine for two or three tasks):

```bash
koto next parent --with-data '{"tasks":[{"name":"issue-1","template":"impl.md","vars":{"N":"1"}},{"name":"issue-2","template":"impl.md","vars":{"N":"2"},"waits_on":["issue-1"]}]}'
```

The 1 MB cap (`MAX_WITH_DATA_BYTES`) stays and applies to the file contents
after the `@` is resolved. That's plenty for hundreds of tasks.

## 4. Compiler validation

What `koto template compile` should catch at load time, when the chosen
shape is in place. Most of these assume candidate (b) -- a
`materialize_children` block on a state -- because that's where validation
localizes best.

### Catchable at compile time

1. **The declaring state has an `accepts` block, and `from_field` names
   one of its declared fields.** Straightforward lookup against
   `TemplateState::accepts` (`src/template/types.rs:56`). Emit a clear
   error if the reference dangles.

2. **The referenced field uses the new `json` (or `array`) type.** Once
   accepts supports the new type, the compiler rejects materialization
   that points at a `string` field, for example. Keeps the template
   honest.

3. **The declaring state is not terminal.** Same check the existing
   evidence-submission path does at `src/cli/mod.rs:1568` (run-time), but
   at compile time to fail fast.

4. **A `children-complete` gate is reachable from the declaring state in
   the transition graph.** "Reachable" here means: there exists a state on
   some path from the materialization state to a terminal state that
   contains a `children-complete` gate. This is analogous to the D4 gate
   reachability check that already exists for `gates.*` routing
   (`src/template/types.rs:822` area, `validate_evidence_routing`). A
   materialization without a downstream convergence point is a bug -- the
   parent would spawn children and then immediately finish without
   joining. A warning is sufficient; it shouldn't block compilation
   because there might be esoteric cases (fire-and-forget) the author
   knows about.

5. **Cyclic `waits_on` cannot be detected at compile time** because tasks
   arrive via evidence, not the template. That's a runtime check at
   materialization. Document it, point users at the error message, move
   on.

6. **Template files referenced in task entries cannot be validated at
   compile time either** -- they come from evidence. Runtime materialization
   invokes the same compile-and-cache path `handle_init` uses
   (`compile_cached`, `src/cli/mod.rs:1074`) and surfaces compile errors
   per-task.

### Error messages

The compile errors should follow the existing `validation error: ...`
wrapper (`src/template/compile.rs:287`) so tooling can grep for them. Good
examples to mimic: `"state {:?} gate {:?}: unknown completion prefix"` at
`src/template/compile.rs:366`.

## 5. Worked examples

### Example A: minimal two-task batch with linear dependency

**Parent template** (`coord.md`):

```yaml
---
name: coord
version: "1.0"
initial_state: plan
states:
  plan:
    accepts:
      tasks:
        type: json
        required: true
    materialize_children:
      from_field: tasks
    transitions:
      - target: await
  await:
    gates:
      done:
        type: children-complete
    transitions:
      - target: finish
        when:
          gates.done.all_complete: true
      - target: await
        when:
          gates.done.all_complete: false
  finish:
    terminal: true
---

## plan

Submit the task plan. Read the design doc, break the work into two steps,
and call `koto next coord --with-data @plan.json`.

## await

Waiting for child workflows to finish.

## finish

Done.
```

**Child template** (`step.md`): a minimal linear template with a `N`
variable; elided for brevity.

**Evidence submission** (`plan.json`):

```json
{
  "tasks": [
    {"name": "coord.step1", "template": "step.md", "vars": {"N": "1"}},
    {"name": "coord.step2", "template": "step.md", "vars": {"N": "2"},
     "waits_on": ["coord.step1"]}
  ]
}
```

**Walkthrough:**

1. `koto init coord --template coord.md` -- parent starts in `plan`.
2. `koto next coord` -- returns directive: "submit the task plan."
3. Agent writes `plan.json`, then runs
   `koto next coord --with-data @plan.json`. The engine:
   - Reads `plan.json`.
   - Validates the payload against the `plan` state's accepts schema (the
     `tasks` field is declared `json`, so the array is accepted).
   - Appends `EvidenceSubmitted { fields: {"tasks": [...]} }` to the event
     log.
   - The `plan` state has a `materialize_children` declaration pointing at
     `tasks`. The engine iterates the array. `coord.step1` has empty
     `waits_on`, so it's immediately ready: the engine calls the same
     code path as `handle_init` with `--parent coord` to create it. The
     child session is now in `backend.list()`.
   - `coord.step2` has `waits_on: ["coord.step1"]`, so it's deferred.
     Nothing happens yet. The engine records a `BatchTaskDeferred` event
     or similar (exact shape is a separate lead's question).
   - `plan` has an unconditional transition to `await`; advance loop
     moves there.
4. The agent launches an implementation subprocess for `coord.step1`,
   which calls `koto next coord.step1` and so on.
5. Meanwhile, the agent polls `koto next coord`. The `await` state's
   `children-complete` gate evaluates: one child, not yet terminal, so
   `all_complete=false` and the self-loop fires. Blocking condition is
   returned with `category: "temporal"`.
6. When `coord.step1` reaches its terminal state, the next `koto next
   coord` call sees the child as complete. The engine now re-evaluates the
   deferred task list, finds `coord.step2` ready (its prerequisite is
   done), and materializes it. Gate still blocks (step2 is fresh, not
   terminal).
7. Eventually `coord.step2` terminates. `koto next coord` sees both
   children complete, the gate passes, transition to `finish` fires.

The key insight: re-evaluation of the deferred list happens every time
`koto next` hits the `await` state, and it's a pure function of
(submitted tasks in the log) + (current `backend.list()` output). No
scheduler daemon, no persisted cursor.

### Example B: diamond DAG of five GitHub-issue children

Five issues: `seed` (no deps), `api` and `db` (depend on `seed`), `ui`
(depends on `api`), and `qa` (depends on `api` and `db`). Classic diamond
with a tail.

**Parent template**: the same two-state `plan` / `await` shape as Example
A. Not shown.

**Evidence submission** (`plan.json`):

```json
{
  "tasks": [
    {"name": "rel.seed", "template": "issue.md", "vars": {"ISSUE": "101"}},
    {"name": "rel.api",  "template": "issue.md", "vars": {"ISSUE": "102"},
     "waits_on": ["rel.seed"]},
    {"name": "rel.db",   "template": "issue.md", "vars": {"ISSUE": "103"},
     "waits_on": ["rel.seed"]},
    {"name": "rel.ui",   "template": "issue.md", "vars": {"ISSUE": "104"},
     "waits_on": ["rel.api"]},
    {"name": "rel.qa",   "template": "issue.md", "vars": {"ISSUE": "105"},
     "waits_on": ["rel.api", "rel.db"]}
  ]
}
```

**Walkthrough:**

- Submission materializes `rel.seed` immediately (empty `waits_on`).
  `rel.api`, `rel.db`, `rel.ui`, `rel.qa` are deferred.
- After `rel.seed` terminates: next `koto next rel` re-checks deferred
  tasks. `rel.api` and `rel.db` are both ready (their shared dependency
  is done). Both materialize in the same `koto next` call. `rel.ui` is
  still deferred (`rel.api` ran but hasn't terminated yet). `rel.qa` is
  still deferred.
- After `rel.api` terminates: `rel.ui` becomes ready, materializes.
  `rel.qa` is still waiting on `rel.db`.
- After `rel.db` terminates: `rel.qa` becomes ready only if `rel.api`
  is also done (which it is), materializes.
- All five eventually terminate; the gate passes; parent transitions
  out of `await`.

Notice the engine never builds a full topological sort. It only asks,
per pending task, "are all your `waits_on` complete?" This is correct
because tasks can be added mid-flight (Example C) and any upfront sort
would go stale.

### Example C: growing batch (mid-flight append)

A running child discovers it needs to spawn two sibling tasks that weren't
in the original plan.

**Initial submission** (parent `root`):

```json
{
  "tasks": [
    {"name": "root.discover", "template": "discover.md"}
  ]
}
```

**Mid-flight: the `root.discover` child, while running, calls**

```bash
koto next root --with-data @extra.json
```

with `extra.json`:

```json
{
  "tasks": [
    {"name": "root.analyze", "template": "analyze.md",
     "waits_on": ["root.discover"]},
    {"name": "root.report",  "template": "report.md",
     "waits_on": ["root.analyze"]}
  ]
}
```

What happens:

- The engine validates that `root` is still in the `plan` state (or
  whichever state declared `materialize_children`). If it's not -- for
  example, already in `await` -- the submission should still work
  provided the `await` state also has an accepts block with the `tasks`
  field. Authors who want mid-flight growth declare `accepts` + `tasks`
  on the same state as the `children-complete` gate so it can re-accept.
  This is the "single-state fan-out pattern" that template-format.md
  already endorses for the simple case -- the same pattern generalizes.
- The new tasks are appended to the persisted evidence log. Since
  evidence is merged per-epoch (`merge_epoch_evidence`, called at
  `src/cli/mod.rs:1705`), subsequent gate evaluations see the union of
  the original plus appended tasks.
- When the parent-agent (or another child) next calls `koto next root`,
  the `await` state's `children-complete` gate now knows about all
  three tasks, not just one. The re-evaluation logic from Example A
  kicks in and materializes as dependencies resolve.

**What the parent sees on the next `koto next` call:**

The next `koto next root` response carries the `children-complete` gate
output with updated totals. Before the append, the gate output had
`{"total": 1, "completed": 1, "all_complete": true}`. After, it reports
`{"total": 3, "completed": 1, "all_complete": false}` and the parent
stays in `await`. The `category: "temporal"` signal tells the parent
agent to keep polling.

The event log is the source of truth: everything the engine needs to
rebuild the deferred set on resume is already persisted.

## 6. Recommendation

**Recommended shape: (b) state-level action verb (`materialize_children`).**

### Justification

Candidate (b) wins because it's the only shape that:

- Localizes declaration and validation on the state where the behavior
  actually happens (unlike (a), which split-brains between frontmatter
  and state).
- Respects the current model's distinction between gates (read-only
  preconditions) and actions (side-effecting things the engine does on
  state entry / evidence), rather than muddying it like (c).
- Is explicitly named in the template, so compilers and authors can
  reason about it directly, unlike (d) where a reserved field name
  silently changes engine behavior.
- Composes cleanly with the existing `children-complete` gate: the
  action decl goes on state X, the convergence gate goes on state Y,
  and they never need to know about each other. The same parent
  template gets the bonus that the `children-complete` gate's name
  filter and override mechanisms (already shipped) work unchanged.

### Why the other three are rejected

- **(a) frontmatter field**: breaks the principle that state behavior is
  declared on the state. Forces readers to look in two places to
  understand what a state does. Tolerable for workflow-global settings
  like `initial_state`, but "where do children materialize" is emphatically
  local.
- **(c) new gate type**: gates in koto are pure functions of state, as
  every existing gate type demonstrates (`src/gate.rs:62`). Introducing a
  side-effecting gate would force `evaluate_gates` to carry a mutable
  backend and forces the blocking-condition category system to add
  categories that don't fit. The override flow (`koto overrides record`)
  is particularly awkward -- what does "override a materialization gate"
  mean?
- **(d) implicit reserved key**: fails the discoverability test. Authors
  would have to learn the magic name from docs; the compiler would
  either special-case it (making the rule explicit, which is just
  candidate (b) with a worse spelling) or not (letting bugs slip through
  unnoticed). The `gates` evidence-key reservation is a bad precedent to
  follow because that's a *rejection* rule, not a *dispatch* rule.

### What still needs resolution

This lead deliberately leaves three things open:

1. **The failure-routing sibling lead picks the `trigger_rule` vocabulary.**
   This lead just notes the field exists and defaults to `all_success`.
2. **The dynamic-additions sibling lead specifies the exact
   re-derivation and persistence contract** for appended tasks (is every
   submission a full replacement, or an append? does the engine dedupe
   by name? what happens if a name collides with a previously-
   materialized child?). Example C above suggests append-with-dedupe,
   but that call isn't mine to make.
3. **The accepts-schema `json` field type** needs its own tiny spike --
   mostly confirming there are no surprising interactions with the D2
   override-default validation at `src/template/types.rs:350`, which
   currently expects scalar-shaped gate outputs. `materialize_children`
   evidence doesn't use `override_default`, so the interaction should be
   clean, but worth confirming.

Everything else -- the schema, the trigger, the CLI shape, the validation
rules -- is answered above, grounded in the code paths that already
exist in v0.7.0.
