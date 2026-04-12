# Panel Review: Template Author Perspective

Reviewer role: template author evaluating whether the batch-child-spawning
design is learnable from the template-format reference and walkthrough
examples alone, without reading the full design doc.

## 1. Can I write a batch template from the examples?

The `coord.md` parent template in the walkthrough is a reasonable
starting point, but several fields require guessing or reading the
design doc to understand.

**What's clear:**
- `accepts` with `type: tasks` and `required: true` -- follows the
  existing pattern for enum/string/etc. Easy to understand once you
  know `tasks` is a valid type.
- `transitions` with `when: gates.done.all_complete: true` -- the
  gate-output routing pattern is already documented in template-format.md.
- `terminal: true` on the `summarize` state -- obvious.

**What's unclear without the design doc:**

- `materialize_children` -- this block appears with zero explanation in
  the walkthrough template. A template author encountering it for the
  first time has to guess what `from_field`, `failure_policy`, and
  `default_template` mean. The template-format reference does not
  mention `materialize_children` at all.

- `from_field: tasks` -- the name suggests it references the `tasks`
  accepts field, but nothing confirms this. Is it the field name from
  `accepts`? Is it a key in the JSON payload? The linkage between
  `accepts.tasks` and `materialize_children.from_field` is implicit.

- `failure_policy: skip_dependents` -- what does this do? What are the
  alternatives? The walkthrough never explains the behavior. You'd have
  to read the design doc's Decision E5 to learn that `continue` is the
  only other option and that `skip_dependents` marks transitive
  dependents as skipped.

- `default_template: impl-issue.md` -- is this a path? Relative to
  what? The template-format reference says nothing about child template
  resolution. The design doc's Decision 4 explains a three-step
  resolution chain (absolute, template source dir, submitter cwd) but
  this is invisible to the template author.

- `type: tasks` -- the template-format reference lists four field types
  (enum, string, number, boolean). `tasks` is absent. A template author
  reading the reference would conclude this is invalid.

**Verdict:** You can copy-paste the walkthrough template and make it
work, but you cannot write one from scratch using the reference alone.
The reference is missing every batch-related concept.

## 2. Is the single-state fan-out pattern intuitive?

No. A template author's natural instinct is to split the workflow into
distinct phases:

```yaml
states:
  submit_plan:
    accepts:
      tasks:
        type: tasks
        required: true
    transitions:
      - target: awaiting_children

  awaiting_children:
    gates:
      done:
        type: children-complete
    materialize_children:
      from_field: tasks
    transitions:
      - target: summarize
        when:
          gates.done.all_complete: true

  summarize:
    terminal: true
```

This feels more natural -- "first I submit, then I wait" -- but it
doesn't work. The design doc explains why: the scheduler runs on the
state where `advance_until_stop` parks, and if you put the hook on
`submit_plan` with an unconditional transition to `awaiting_children`,
the advance loop passes through `submit_plan` and never gives the
scheduler a chance to run there.

The constraint is architectural, not logical. Template authors think
in terms of workflow phases, not advance-loop parking behavior. The
design doc devotes a full paragraph to "why single-state fan-out is
required, not a preference" -- which signals the authors knew this
was non-obvious.

**What would help:** The template-format reference should include an
explicit "do this, not that" callout:

> The `materialize_children` hook, the `accepts` field it references,
> and the `children-complete` gate must all be on the same state.
> Splitting them across states will silently prevent spawning.

Without this, every template author will make the split-state mistake
first, get no compiler error (the compiler validates each field
individually), and debug a "children never spawn" problem.

**Missing compiler check:** There is no E-rule that catches the
split-state pattern. E5 checks that the declaring state isn't terminal,
E7 checks it has transitions, but nothing checks that the
`children-complete` gate is on the same state as the hook. W1 warns
if a `children-complete` gate is "reachable from the declaring state"
but reachable-from is weaker than same-state, and it's a warning, not
an error. A template author could put the gate on a downstream state,
get W1, ignore it (it's a warning), and then be stuck debugging.

## 3. Error messages

The E1-E9 compiler errors are well-designed. Each maps to a specific
template mistake and most have obvious fixes:

| Error | Clear? | Notes |
|-------|--------|-------|
| E1 | Yes | `from_field` must not be empty |
| E2 | Yes | `from_field` must name an `accepts` field |
| E3 | Yes | Field must be `type: tasks` |
| E4 | Yes | Field must be `required: true` |
| E5 | Yes | State must not be terminal |
| E6 | Mostly | Lists valid values, but "skip_dependents" vs "continue" semantics aren't obvious from the error alone |
| E7 | Yes | State needs transitions |
| E8 | Yes | Prevents copy-paste duplication |
| E9 | Mostly | "resolves to a compilable template" -- what if the template is valid but in the wrong directory? |

**Missing checks that would catch common mistakes:**

1. No check for the split-state pattern (discussed above). If
   `materialize_children` and `children-complete` are on different
   states, the compiler should error, not warn.

2. No compile-time check that the child template's `variables`
   match the `vars` shape that callers will submit. This is
   necessarily runtime (R-level), but a note in the error message
   for R-rules would help: "child template X requires variable Y
   but the task entry for Z does not provide it."

3. E9 validates `default_template` at compile time, but per-task
   `template` overrides are only validated at runtime (R1). A
   template author might test with the default template and miss
   a broken per-task override until the batch is half-spawned.

## 4. Child template authoring

The `impl-issue.md` child template uses `failure: true` on
`done_blocked`. This is the key mechanism for signaling failure to
the parent's scheduler.

**Is it obvious?** No. The template-format reference documents
`terminal: true` but not `failure: true`. A template author writing
a child template would have no reason to add `failure: true` unless
they'd read the design doc or seen the walkthrough example.

**What happens if you forget it?** The child reaches `done_blocked`
(a terminal state), the scheduler classifies it as `success` (because
`failure` defaults to `false`), and dependents proceed as if nothing
went wrong. This is a silent correctness bug. The template compiles
fine. The workflow runs. The results are wrong.

This is the single most dangerous omission in the current design
surface. A template author who writes:

```yaml
done_blocked:
  terminal: true
```

instead of:

```yaml
done_blocked:
  terminal: true
  failure: true
```

gets no warning, no error, and incorrect behavior.

**Mitigation ideas:**
- W3 warning: terminal state name contains "fail", "block", "error",
  or "reject" but `failure: true` is not set.
- Document `failure: true` prominently in template-format.md with an
  explanation of what it controls.

## 5. What's missing from the template-format reference?

The template-format reference needs a new section -- call it "Layer 4:
Batch child spawning" or fold it into Layer 3 under a "Batch
orchestration" heading. Specific additions:

### 5.1 New field type: `tasks`

The "Field types" table needs a fifth row:

| Type | Requires | Notes |
|------|----------|-------|
| `tasks` | -- | Structured task list for batch child spawning |

Plus a brief explanation of the item schema (name, template, vars,
waits_on, trigger_rule) and a note that the `evidence_required`
response auto-generates an `item_schema` object.

### 5.2 The `materialize_children` hook

A new subsection documenting:
- Field placement (on a state, alongside `gates` and `accepts`)
- Fields: `from_field`, `failure_policy`, `default_template`
- The single-state fan-out constraint (must co-locate with
  `children-complete` gate and the `accepts` field it references)
- A minimal example (the `coord.md` pattern)

### 5.3 The `failure: true` field on terminal states

A new row in the states table:

| Field | Type | Purpose |
|-------|------|---------|
| `failure` | bool | Marks a terminal state as a failure outcome |

Plus an explanation that this field controls how the batch scheduler
and `children-complete` gate classify the child's result.

### 5.4 The `skipped_marker: true` field

Document briefly. Template authors writing child templates rarely
need it (the scheduler creates synthetic skipped children
automatically), but authors need to know the concept exists so they
can understand the gate output.

### 5.5 Extended `children-complete` gate output

The gate output table needs new fields: `success`, `failed`,
`skipped`, `blocked`, and the per-child `outcome` enum. The current
table shows `total`, `completed`, `pending`, `all_complete`, and
`children` but not the failure-aware fields.

### 5.6 Task submission example

A concrete example of the `@file.json` syntax and the task list
JSON structure. The walkthrough has this, but the reference should
too.

### 5.7 Retry mechanics

Brief documentation of `retry_failed` as a reserved evidence key,
what it does, and how to structure a template that supports retry
(route to an analysis state, then back to the fan-out state).

## Top 3 issues

1. **`failure: true` is invisible and its absence is a silent
   correctness bug.** A child template that omits `failure: true` on
   its failure terminal state will be classified as successful by the
   scheduler. Dependents will proceed. The template compiles. No
   warning fires. This will be the single most common and most
   damaging mistake template authors make. Fix: add a compiler
   warning for terminal states whose names suggest failure, and add
   prominent documentation in the template-format reference.

2. **The single-state fan-out constraint is non-obvious and
   unenforced.** Template authors will naturally split submit and wait
   into separate states. The compiler doesn't catch this. The failure
   mode is "children never spawn" with no error message. Fix: promote
   W1 to an error (or add a new error) that requires
   `materialize_children` and `children-complete` to be on the same
   state, and add a "do this, not that" callout in the reference.

3. **The template-format reference is completely silent on batch
   features.** `type: tasks`, `materialize_children`, `failure: true`,
   `skipped_marker: true`, the extended gate output schema, and
   `retry_failed` are all absent from the reference. A template author
   reading only the reference cannot write a batch template. Fix: add
   a "Batch orchestration" section covering all five new concepts with
   a minimal working example.
