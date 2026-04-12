<!-- decision:start id="batch-child-spawning-contract" status="assumed" -->
### Decision: Task list schema, template hook shape, and compiler validation for declarative batch child spawning

## Question

What is the exact shape of one task entry in a batch submission, where is the
batch-level `failure_policy` declared, what is the name and placement of the
template hook that tells the engine "materialize children from this evidence
field," and which compiler validation rules are errors vs warnings vs deferred
to runtime?

This is the v1 contract. It is scoped to the happy path, plus enough validation
to catch authoring mistakes early. It deliberately does not decide the
`trigger_rule` vocabulary (owned by the failure-routing decision), the exact
dynamic-append semantics (owned by the dynamic-additions decision), or whether
`format_version` bumps from 1 to 2 (owned by Decision 3).

## Options Considered

The exploration's lead-evidence-shape file enumerates four candidate shapes
for where the materialization trigger is declared in the template:

**(a) Frontmatter field** -- add a top-level field like `batch_spawn_state: plan`
to `CompiledTemplate`, naming the state whose evidence triggers materialization.
Tiny compiler surface (one optional field), but split-brains the state's
behavior between frontmatter and the state's own block.

**(b) State-level action verb** -- add an optional `materialize_children` block
to `TemplateState` alongside `gates`, `accepts`, and `default_action`. The block
carries one required field (`from_field`) naming the declared accepts field that
holds the task list. Local to the state, explicitly named, new AST node.

**(c) New gate type `batch-materialize`** -- model materialization as a gate that
is "true" once children have been created. Composes with existing gate
validation, but gates are pure read-only predicates in the current model;
`evaluate_gates` would need backend mutation authority, which breaks
idempotency and the blocking-condition category taxonomy.

**(d) Implicit reserved evidence key** -- dispatch on a magic field name
(`_spawn` or `__tasks`) in the accepts block. Trivial compiler surface, but the
behavior is invisible at the template level and the compiler can't give
authors feedback on misuse without special-casing.

A fifth name-only variant **(b')** -- the same state-level block but spelled
`batch` or `batch_spawn` instead of `materialize_children` -- was considered as a
sub-alternative.

## Chosen Option

**Candidate (b), spelled `materialize_children`, with schema and validation as
specified below.**

### Task entry schema

One task entry is a JSON object:

| Field | Type | Required | Default | Purpose |
|-------|------|----------|---------|---------|
| `name` | string | yes | -- | Short task name. Child workflow name is `<parent>.<name>`. Must pass `validate_workflow_name()`. |
| `template` | string | yes | -- | Path to child template source. Resolved relative to the parent workflow's working directory (same rule `koto init --template` uses). |
| `vars` | object (string to string) | no | `{}` | Forwarded verbatim to the child's `resolve_variables()` as if passed via `koto init --var KEY=VALUE`. |
| `waits_on` | array of string | no | `[]` | Flat list of sibling task `name` values that must complete before this task is materialized. |
| `trigger_rule` | string enum | no | `"all_success"` | Out of scope for v1. Field is reserved; any value other than `"all_success"` is a runtime error in v1. Full vocabulary owned by the failure-routing decision. |

Any field other than these five is a runtime error at materialization time
("unknown task field: X"). This matches the strict-by-default posture the
accepts schema already takes.

### Batch-level `failure_policy`

Declared on the **template hook**, not per task and not in the payload:

```yaml
states:
  plan:
    accepts:
      tasks:
        type: json
        required: true
    materialize_children:
      from_field: tasks
      failure_policy: skip_dependents   # default; only other v1 value is "continue"
```

Rationale: `failure_policy` is a template-author concern, not an agent concern.
The agent should not be able to change it by editing the JSON payload, because
the parent template's downstream states (the `await` state with the
`children-complete` gate, the recovery transitions) are written assuming a
specific policy. Burying it in the payload would let agents accidentally
invalidate the parent template's guarantees. Putting it on the hook keeps the
policy next to the parent state's transition graph where the author can see the
consequences.

`failure_policy` is optional and defaults to `skip_dependents` (the settled
default). The per-batch override is simply "omit or set to a non-default value on
the hook." This matches the "per-batch override" constraint exactly -- a given
parent template has exactly one batch, so "per batch" and "per hook" are the
same thing in v1.

### Template hook name and placement

**Name: `materialize_children`.** **Placement: a new optional field on
`TemplateState` at `src/template/types.rs:62`**, alongside `default_action`.

Why `materialize_children` over the alternatives:

- **`batch`** is too generic. Koto templates may eventually grow other
  batch-flavored features (batch evidence, batched integrations); reserving
  the bare word `batch` for this one feature is premature. It also reads
  poorly -- `batch: { from_field: tasks }` doesn't tell a reader what happens.
- **`batch_spawn`** is fine grammatically but implies that spawning is the
  observable primitive. Koto's model is "the engine creates (materializes) a
  child state file; whether anything runs is an orchestration concern." The
  verb `materialize` captures that distinction the explored leads pushed on:
  koto is a contract layer, not an execution engine. Calling the action
  `spawn` would suggest koto is launching the agent process.
- **`materialize_children`** reads as prose (`"this state materializes
  children from the tasks field"`) and mirrors the existing vocabulary:
  `children-complete` gate, `parent_workflow` header, `koto init --parent`.
  All three use "child" and "children" rather than "spawn" or "worker." The
  new hook should match.

Placement on `TemplateState` (not on `Transition`, not in frontmatter):

- State entry is when materialization should fire -- as soon as the state has
  received the evidence field, the children appear. Putting the hook on the
  state matches the timing.
- `TemplateState` already has two local authoring primitives (`gates`,
  `default_action`) that produce side effects when the state is entered.
  `materialize_children` joins that set.
- Frontmatter is rejected -- it splits the declaration across two places (see
  rejected alternative (a) below).

### Compile-time validation rules

Let **B** be the `materialize_children` block on state S, with field
`from_field = F`.

**Errors (compilation fails)**:

1. **E1 -- `from_field` is empty or missing.** The hook requires a target
   field; without it the engine has nothing to parse. Error:
   `state <S> materialize_children: missing required field 'from_field'`.
2. **E2 -- `from_field` does not name a declared accepts field.** Straightforward
   lookup in `TemplateState::accepts`. Error:
   `state <S> materialize_children: from_field <F> is not declared in the accepts block`.
3. **E3 -- the referenced accepts field has type != `json`.** The `json` type
   is the only accepts type that can hold an array of objects. Error:
   `state <S> materialize_children: field <F> must have type 'json' (found '<type>')`.
4. **E4 -- the referenced accepts field is not `required: true`.** An optional
   task list means the state can transition out with zero children, and the
   downstream `children-complete` gate will vacuously pass -- a silent foot-gun.
   Error: `state <S> materialize_children: field <F> must be required: true`.
5. **E5 -- the declaring state is terminal.** A terminal state never fires
   materialization because the engine stops there. Error:
   `state <S> materialize_children: state is terminal; materialization would never fire`.
6. **E6 -- `failure_policy` is set to an unknown value.** v1 accepts only
   `skip_dependents` (default) and `continue`. Error:
   `state <S> materialize_children: unknown failure_policy <value> (expected 'skip_dependents' or 'continue')`.
7. **E7 -- the state has no outgoing transition.** Without an outbound
   transition, materialized children appear but the parent never advances to
   an `await` state. Error: `state <S> materialize_children: state has no outgoing transitions`.
8. **E8 -- two `materialize_children` blocks on different states reference the
   same `from_field` name across the whole template.** This is a correctness
   issue: the accepts field is declared per-state, but duplicate field names
   across states that both want materialization is almost always a copy-paste
   bug. Error: `state <S> materialize_children: from_field <F> is also used by state <S2>`.

**Warnings (compilation succeeds, message on stderr under non-strict; hard
error under `--strict`)**:

1. **W1 -- No `children-complete` gate is reachable from S along any transition
   path.** The parent materializes children and then has no join point; almost
   always an author bug. This is a warning rather than an error because
   fire-and-forget is a legitimate (rare) pattern. Uses the same path-reachability
   walker as the existing D4 gate reachability check
   (`validate_evidence_routing` in `src/template/types.rs`).
2. **W2 -- The reachable `children-complete` gate has a `name_filter` that
   does not match the pattern `"<parent_name>."`.** The child naming rule is
   `<parent>.<task>`, so a filter that doesn't start with `<parent_name>.` is
   probably a typo. Warning because some authors may use filters deliberately
   to scope to a subset (e.g. `rel.api` only).

**Deferred to runtime (no compile check possible)**:

1. **R1 -- Child template file exists and compiles.** The `template` field on
   each task is agent-supplied and cannot be read at parent-compile time.
   Checked by the same `compile_cached` path `handle_init` already uses.
2. **R2 -- Each child template's `variables` block resolves cleanly against
   the supplied `vars`.** Uses the existing `resolve_variables` error path.
3. **R3 -- `waits_on` forms a DAG (no cycles).** The task list is evidence.
   A runtime check at materialization time that walks `waits_on` and rejects
   cycles with `batch submission: cycle detected: <task1> -> <task2> -> ... -> <task1>`.
4. **R4 -- All `waits_on` entries reference declared task `name`s in the same
   submission.** Runtime check; reject the whole submission if any dangling
   reference is found.
5. **R5 -- Task `name` values are unique within a submission.** Runtime check;
   duplicate names collapse silently at the `backend.exists()` idempotency
   level, so rejecting at submission time is clearer.
6. **R6 -- Task `name` values pass `validate_workflow_name()`.** Runtime;
   this is the same regex check `koto init --name` performs today, reused.
7. **R7 -- Collision with already-materialized children.** If a later batch
   append produces a task whose computed child name already exists in the
   backend, that's an append-semantics question owned by the dynamic-additions
   decision.

## Rationale

Six reasons, in order of weight:

**1. Locality of behavior.** The `materialize_children` block lives on the
state where materialization fires. A reader looking at state `plan` sees the
full story: accepts schema, materialize hook, transitions. Every other shape
forces readers to look in a second place.

**2. Consistency with existing template-format primitives.** `TemplateState`
already has `gates` (read-only preconditions) and `default_action` (side
effects on entry). `materialize_children` is the third kind of state-local
feature and slots in cleanly. Authors reading
`plugins/koto-skills/skills/koto-author/references/template-format.md` already
know that "state-level side effects go on the state."

**3. Compiler validation localizes.** E1 through E8 all turn into simple
lookups against the state's own data. No cross-file reasoning, no reference
chasing through frontmatter. `src/template/compile.rs` gets one new validator
function that mirrors the shape of the existing gate validators.

**4. `materialize_children` (not `batch` or `batch_spawn`) matches koto's
existing vocabulary.** The feature is about *children*; every other piece of
v0.7.0 parent/child machinery (the `parent_workflow` header, the
`children-complete` gate, `koto init --parent`, `koto workflows --children`)
uses "child" and "children." The new hook should not introduce a new word for
the same concept.

**5. `from_field` must point at a `required: true` field of type `json`.** This
pairing (E3 + E4) is the lynchpin that makes every other validation rule
possible. Once the compiler knows the field will be present and will hold
structured data, it can stop second-guessing and delegate everything else to
runtime.

**6. `failure_policy` on the hook (not per task, not in payload).** Policy is
a parent-template contract -- the `await` state's transitions and recovery
routes are written assuming a specific failure behavior. Letting the agent
override it per-submission would invalidate the parent template's promises.
Putting it on the hook keeps policy next to the states that consume its
consequences.

## Rejected Alternatives

- **(a) Frontmatter field `batch_spawn_state: plan`**: breaks the principle that
  state behavior is declared on the state. Readers have to cross-reference
  frontmatter with `states.plan` to understand what happens. The only thing
  this shape saves is one level of nesting, which is not worth the split
  declaration. Rejected.

- **(b') Same shape, named `batch` or `batch_spawn`**: `batch` is too generic
  (koto may grow other batch features later); `batch_spawn` implies koto
  launches agent processes, which contradicts koto's "contract layer, not
  execution engine" stance. `materialize_children` reads as prose and matches
  the existing `children-complete` / `parent_workflow` vocabulary. Rejected on
  naming grounds only; shape is the same.

- **(c) New gate type `batch-materialize`**: gates in koto today are pure
  functions of `(state, evidence, context)` with no side effects -- see
  `src/gate.rs:62 evaluate_gates`. Giving one gate type write access to the
  session backend breaks the invariant and forces the blocking-condition
  category taxonomy to grow a new category ("I need to create something on
  first evaluation") that has no good name. The override flow
  (`koto overrides record`) is particularly awkward: what does overriding a
  materialization gate mean? Rejected on model-consistency grounds.

- **(d) Implicit reserved evidence key `_spawn` or `__tasks`**: fails the
  discoverability test. Authors have to learn the magic name from docs; the
  compiler has to either special-case it (at which point it is (b) with a
  worse name) or leave bugs unreported. The existing `gates` reserved key is a
  *rejection* rule ("you cannot name a field this"); dispatch on a reserved
  key is asymmetric. Rejected.

- **`failure_policy` per task (not per hook)**: rejected. Each task having its
  own policy is more expressive than v1 needs, and it removes the ability for
  the parent template to guarantee a consistent failure story downstream. Can
  be added in a later version by the failure-routing decision if demand
  appears.

- **`failure_policy` in the JSON payload (alongside tasks)**: rejected for
  the same reason -- lets agents change contract-level behavior at runtime,
  which invalidates parent-template guarantees.

- **Allowing `from_field` to point at a `string`-typed accepts field (with
  JSON parsed out of the string)**: rejected. The `json` accepts type is
  already settled as part of a sibling decision. Stringly-typed JSON is a
  foot-gun: the engine would have to parse twice (once by `--with-data`, once
  by the hook), and the compiler cannot validate the inner shape at all.

## Assumptions

These assumptions affect downstream decisions; flag them in the design doc so
peer decisions can revise or corroborate:

1. **The `json` accepts field type is already settled.** The problem statement
   says so; this decision leans on it heavily (E3, E4). If that changes, E3
   must change with it.
2. **Child naming is `<parent>.<task>`.** Settled per the problem statement.
   W2's filter check depends on this. If `.` is forbidden in workflow names
   somewhere in `validate_workflow_name`, fall back to `__`.
3. **Scheduler runs at the CLI layer in `handle_next`.** Settled. E5's
   "terminal state" check relies on this: if the engine ran the scheduler
   before the advance loop instead of after, a transition from terminal to
   non-terminal would be detectable, but since it runs after, E5 is safe.
4. **`trigger_rule` vocabulary is out of scope for v1.** Reserving the field
   (and hard-erroring on non-default values at runtime) keeps the schema
   forward-compatible with the failure-routing decision.
5. **Default `failure_policy` is `skip_dependents`.** Settled per the problem
   statement. The alternative value in v1 is `continue`. Other values are
   added by the failure-routing decision.
6. **Only one `materialize_children` block per template in v1.** E8 enforces
   it. Multi-batch-per-template is plausible (e.g., two parallel independent
   fanouts in different states) but adds meaningful complexity to the
   "which children belong to which batch" question and is better left to a
   later version.
7. **`format_version` stays at 1.** Decision 3 may bump it. If it does, add a
   guard that `materialize_children` requires `format_version >= 2`. Nothing
   else in this decision is sensitive to that outcome.

## Open Questions

1. **How does a child discover the `parent_workflow` name at runtime?** Today
   it lives in the child's header but there is no template-level variable for
   it. A child template that wants to call `koto next <parent>` to signal
   completion-of-own-work needs some way to refer to the parent. Not blocking
   for this decision, but the dynamic-additions decision should know about it.

2. **Should `vars` support values other than strings?** The schema says
   "string to string" because `resolve_variables` takes strings. If the `json`
   accepts field type permits nested JSON in `vars`, the engine would have to
   either stringify or reject. Recommendation: reject at runtime with a clear
   error ("vars values must be strings; got <type> for key <K>"), document
   later, revisit if authors ask.

3. **Does the event log need a dedicated `MaterializationSubmitted` event
   type, or is plain `EvidenceSubmitted` enough?** This decision assumes
   plain `EvidenceSubmitted` (the batch is just evidence; the
   `materialize_children` hook on the state is what gives it meaning). The
   dynamic-additions decision may push back on this if append semantics need
   richer events.

4. **Should `waits_on` allow forward references (a task that lists a later
   sibling)?** Yes, probably -- the DAG check happens at submission time and
   is order-independent. Confirm with dynamic-additions decision; this is
   mostly a documentation call.

## Worked Examples

### Example A: minimal two-task linear batch

**Parent template** (`coord.md`):

```yaml
---
name: coord
version: "1.0"
initial_state: plan
states:
  plan:
    directive: "Break the work into two ordered steps and submit as tasks."
    accepts:
      tasks:
        type: json
        required: true
    materialize_children:
      from_field: tasks
      failure_policy: skip_dependents
    transitions:
      - target: await
  await:
    directive: "Wait for children to finish."
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
    directive: "All children done."
    terminal: true
---
```

**Child template** (`step.md`): standard single-state linear template with a
`N` variable in its `variables` block; body elided.

**Evidence submission** (`plan.json`):

```json
{
  "tasks": [
    {"name": "step1", "template": "step.md", "vars": {"N": "1"}},
    {"name": "step2", "template": "step.md", "vars": {"N": "2"},
     "waits_on": ["step1"]}
  ]
}
```

**Resulting child workflow names** (by the `<parent>.<task>` rule):
`coord.step1`, `coord.step2`.

**Walkthrough:**

1. `koto init coord --template coord.md` -- parent in `plan`.
2. `koto next coord --with-data @plan.json` -- accepts validates the `tasks`
   field as `json` (present, required, check passes). The
   `materialize_children` hook fires. The engine parses the task list, sees
   `step1` has empty `waits_on` (ready) and `step2` depends on `step1` (not
   ready). It calls `init_workflow` for `coord.step1` with `parent_workflow =
   "coord"`, skipping `coord.step2`. The unconditional transition fires and
   the parent moves to `await`.
3. Agent (or external scheduler) pulls `coord.step1` through its states.
4. `koto next coord` -- the `await` state's `children-complete` gate evaluates.
   Total=2 (both `coord.step1` and `coord.step2` are known from the persisted
   batch definition in evidence, even though `coord.step2` doesn't exist
   yet), completed=0. Gate output has `all_complete=false`; self-loop fires.
   Blocking category is `temporal`.
5. When `coord.step1` reaches terminal, next `koto next coord` re-runs the
   scheduler tick: `coord.step2`'s waits_on is now satisfied. The engine calls
   `init_workflow` for `coord.step2`. Gate output updates: total=2, completed=1,
   still not done.
6. When `coord.step2` reaches terminal, gate passes, transition to `finish`.

### Example B: diamond DAG of five children with explicit failure policy

**Parent template** (`release.md`): same `plan` / `await` / `finish` shape as
Example A, but the `materialize_children` block sets `failure_policy:
continue` because this release job wants to report partial results even if
one arm fails.

```yaml
states:
  plan:
    directive: "Submit the release DAG."
    accepts:
      tasks:
        type: json
        required: true
    materialize_children:
      from_field: tasks
      failure_policy: continue
    transitions:
      - target: await
```

**Child template** (`issue.md`): minimal single-state template with `ISSUE`
variable.

**Evidence submission** (`plan.json`):

```json
{
  "tasks": [
    {"name": "seed", "template": "issue.md", "vars": {"ISSUE": "101"}},
    {"name": "api",  "template": "issue.md", "vars": {"ISSUE": "102"},
     "waits_on": ["seed"]},
    {"name": "db",   "template": "issue.md", "vars": {"ISSUE": "103"},
     "waits_on": ["seed"]},
    {"name": "ui",   "template": "issue.md", "vars": {"ISSUE": "104"},
     "waits_on": ["api"]},
    {"name": "qa",   "template": "issue.md", "vars": {"ISSUE": "105"},
     "waits_on": ["api", "db"]}
  ]
}
```

**Resulting child workflow names**: `rel.seed`, `rel.api`, `rel.db`, `rel.ui`,
`rel.qa` (assuming the parent is named `rel`).

**Compile-time checks that pass:**

- E1-E3: `from_field: tasks` is present and references a `json`-typed
  accepts field.
- E4: `tasks` is `required: true`.
- E5: `plan` is not terminal.
- E6: `failure_policy: continue` is valid.
- E7: `plan` has a transition to `await`.
- E8: only one `materialize_children` block in the template.
- W1: `await` contains a `children-complete` gate and is reachable from `plan`.
- W2: no `name_filter` is set on the gate, so W2 does not fire.

**Runtime checks that pass at submission:**

- R3: the DAG `seed -> {api, db}; api -> ui; {api, db} -> qa` has no cycles.
- R4: every `waits_on` entry references a sibling task name that exists in
  the same submission.
- R5: the five names are unique.
- R6: all five pass `validate_workflow_name`.

**Walkthrough (abbreviated):**

- Submission materializes `rel.seed` (empty `waits_on`); defers the other
  four.
- After `rel.seed` terminates: `rel.api` and `rel.db` become ready on the
  next `koto next rel`. Both materialize in one scheduler tick.
- After `rel.api` terminates: `rel.ui` becomes ready. `rel.qa` still waits on
  `rel.db`.
- After `rel.db` terminates: `rel.qa` becomes ready (both its predecessors
  are now done), materializes.
- If `rel.ui` fails in a terminal-failure state, `failure_policy: continue`
  means `rel.qa` is not affected (its predecessors are `api` and `db`, not
  `ui`). The gate's `children-complete` output reports `completed=4,
  pending=0, all_complete=true` with one child in a failure terminal, and
  the parent transitions to `finish`. The parent template is responsible
  for inspecting child outcomes and deciding what to do.

### Example C: catching a compile error

**Buggy parent template** (`bad.md`):

```yaml
---
name: bad
version: "1.0"
initial_state: plan
states:
  plan:
    directive: "Submit plan."
    accepts:
      plan_body:
        type: string
        required: true
    materialize_children:
      from_field: plan_body
    transitions:
      - target: finish
  finish:
    directive: "Done."
    terminal: true
---
```

**`koto template compile bad.md` output:**

```
validation error: state "plan" materialize_children: field "plan_body" must have type 'json' (found 'string')
validation warning: state "plan" materialize_children: no children-complete gate reachable from this state
```

The first line is E3 (hard error). The second line is W1 (warning). In
non-strict mode the compiler exits with the error from E3. In strict mode,
even if the author fixes E3, the W1 warning would also become a hard error
and the author would be nudged to add an `await` state with a
`children-complete` gate.

Fixing E3 by changing `type: string` to `type: json` and adding an `await`
state with a `children-complete` gate produces a clean compile.

## Consequences

**Easier:**

- Authors writing parent templates see one uniform place for state-local
  behavior (`gates` + `default_action` + `materialize_children`).
- The compiler catches the most common authoring mistakes (E1-E8) at load
  time, before any runtime materialization.
- `from_field: tasks` + `type: json` + `required: true` makes the contract
  between schema and hook trivially enforceable in the compiler.
- The child-template universe is unchanged; existing child templates
  immediately become eligible to be spawned from a batch.

**Harder:**

- Adding per-task `failure_policy` or per-task `trigger_rule` in a later
  version requires either overloading the hook (back-compat issue) or
  growing a second hook variant. The forward path is: keep `failure_policy`
  on the hook, add `trigger_rule` per task when the failure-routing
  decision needs it.
- Mid-flight append semantics (Example C in the exploration lead) intersect
  with E8 ("only one `materialize_children` block") -- the dynamic-additions
  decision must either stick with one block or propose a multi-block
  variant, in which case E8 becomes per-state rather than per-template.
- Compile-time detection of cross-task correctness (cycles, dangling
  refs) is impossible because tasks arrive in evidence. All cross-task
  rules live in runtime (R3, R4, R5). Authors will see these errors only
  when a real submission is attempted.

**Unchanged:**

- `format_version` stays at 1 unless Decision 3 bumps it.
- The `children-complete` gate shape and `name_filter` semantics.
- The `koto init --parent` code path, now reused under the hood by the
  scheduler.
- Append-only state-file semantics -- materialization records are child
  state files on disk, not mutations of the parent header.
<!-- decision:end -->
