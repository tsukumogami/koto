<!-- decision:start id="task-schema-discovery" status="confirmed" -->
### Decision 8: How does the agent discover the task entry schema and child template path?

**Context**

Two gaps surfaced during the interactive walkthrough. First, the
`materialize_children` hook (Decision 1) tells koto which evidence field
holds the task list but says nothing about the child template. The
template author hardcodes a path like `template="impl-issue.md"` in
directive prose; if the template is renamed, the agent discovers the
mismatch at runtime when the scheduler fails to compile the referenced
path. Second, the `expects` block advertises `tasks: { type: json,
required: true }` but `json` is opaque -- the agent doesn't learn that
each entry needs `name`, `template`, `vars`, `waits_on` from anything
in the response. Both gaps force the agent to rely on out-of-band
knowledge (skill docs, prose) to construct a valid task list.

The question: should koto close one or both gaps structurally, and if
so, how?

**Assumptions**

- Decision E7 rejected full JSON Schema in the accepts type system as
  over-engineering for v1. Any solution that reintroduces JSON Schema
  by another name would violate that constraint.
- The batch task entry schema is fixed by the feature itself (name,
  template, vars, waits_on, trigger_rule). It's not per-template.
  This means a bespoke `task_schema` response field would be documenting
  a single, invariant schema -- the same schema every time.
- Template authors already write directives as prose alongside
  structured fields like `expects`. Adding structured data next to
  prose is consistent with koto's patterns.
- Compile-time validation of referenced template paths is strongly
  preferred (decision driver).

**Research findings**

Examining the source revealed a gap the five provided options didn't
account for. `FieldSchema` (in `src/template/types.rs:75`) has a
`description: String` field that template authors can set on any
accepts field. But `derive_expects` in `src/cli/next_types.rs:455`
drops it -- `ExpectsFieldSchema` carries `field_type`, `required`, and
`values` but not `description`. This is an existing extension point
that's silently discarded. Surfacing it costs one line change in the
struct and one in the mapping function, with zero new concepts.

This doesn't replace `default_template` (descriptions can't be
compile-time validated), but it solves the schema-discovery gap through
the same channel agents already read (`expects`) without introducing
a new top-level response field or extending the type system.

**Chosen: Option 1 modified -- `default_template` on the hook + surface
existing `description` field in `expects` (not a new `task_schema`
response field)**

Two changes:

1. **`default_template` on `MaterializeChildrenSpec`.** Required field.
   The compiler validates the path resolves to a compiled template in
   the same template set. When present, task entries may omit `template`
   (the default is used). When a task entry supplies `template`, the
   per-task value wins. The compiler emits an error if
   `materialize_children` is present and `default_template` doesn't
   resolve.

   Hook shape after this change:
   ```yaml
   materialize_children:
     from_field: tasks
     failure_policy: skip_dependents
     default_template: impl-issue.md
   ```

2. **Surface `FieldSchema.description` in `ExpectsFieldSchema`.**
   Add `description: String` (with `skip_serializing_if =
   "String::is_empty"`) to `ExpectsFieldSchema`. Update `derive_expects`
   to copy it through. This is a one-line struct addition and a one-line
   mapping change.

   Template authors then write:
   ```yaml
   accepts:
     tasks:
       type: json
       required: true
       description: >
         JSON array of task objects. Each entry: { name: string (required),
         template: string (optional, defaults to impl-issue.md),
         vars: object (optional), waits_on: string[] (optional),
         trigger_rule: string (optional, only all_success in v1) }
   ```

   The agent sees this in the `evidence_required` response:
   ```json
   {
     "action": "evidence_required",
     "state": "plan_and_await",
     "directive": "Read the plan and submit a task list...",
     "advanced": true,
     "expects": {
       "event_type": "evidence_submitted",
       "fields": {
         "tasks": {
           "type": "json",
           "required": true,
           "description": "JSON array of task objects. Each entry: { name: string (required), template: string (optional, defaults to impl-issue.md), vars: object (optional), waits_on: string[] (optional), trigger_rule: string (optional, only all_success in v1) }"
         }
       },
       "options": []
     },
     "blocking_conditions": [],
     "error": null
   }
   ```

   An agent without the koto-user skill can read the `description`
   field to learn the schema. The information is right next to the
   field it describes, in the same `expects` block the agent already
   parses. No new response concepts.

**Why not a dedicated `task_schema` response field (Option 1 as
originally stated)?**

The batch task entry schema is invariant -- it's the same five fields
every time. A dedicated `task_schema` object in every `evidence_required`
response adds a new response concept that agents must learn to look for,
for a schema that never changes. The `description` field approach
carries the same information through an existing channel and works for
any `json`-typed field, not just batch tasks. It's more general, less
surface area, and doesn't introduce a response field that only matters
for one feature.

**Why not `item_schema` on the accepts type (Option 4)?**

Decision E7 was cautious about extending the accepts type system.
`item_schema: batch_task` introduces a new concept on the type system
(named built-in schemas), which is exactly the kind of machinery E7
deferred. The `description` field is a freeform string, not a type
system extension.

**Why not prose-only (Option 3)?**

Both gaps remain open. The child template path isn't compile-time
validated. The agent has no structured signal about the JSON shape.
This is strictly worse on every evaluation axis except implementation
effort.

**Why not `default_template` only (Option 2)?**

Closes Gap 1 but leaves Gap 2 open. The schema is still only in prose
and skill docs. Given that surfacing `description` is a one-line
change to an existing struct, there's no reason to leave the gap open.

**Alternatives Considered**

- **Option 1 (original): `default_template` + new `task_schema`
  response field.** Rejected. Adds a response concept for an invariant
  schema. The `description` field carries the same information through
  an existing channel.

- **Option 2: `default_template` only.** Rejected. Leaves the schema
  gap open for no savings -- surfacing `description` is trivial.

- **Option 3: Prose only.** Rejected. Both gaps remain. No compile-time
  validation of child template paths.

- **Option 4: `default_template` + `item_schema` annotation on accepts
  type.** Rejected. Extends the type system in ways Decision E7 deferred.

- **Option 5 (code-derived): Surface `description` alone without
  `default_template`.** Rejected. Descriptions aren't compile-time
  validated. The child template path gap still needs a structural
  solution.

**Consequences**

- `MaterializeChildrenSpec` gains a required `default_template: String`
  field. The compiler validates it. Existing templates without
  `materialize_children` are unaffected.
- `ExpectsFieldSchema` gains an optional `description: String` field.
  Responses for existing templates are unchanged (empty descriptions
  are skipped during serialization). Templates that already set
  `description` on accepts fields will start seeing it in responses --
  this is a pure improvement, not a breaking change.
- Template authors writing batch workflows declare the child template
  once in the hook and describe the task schema once in the accepts
  `description`. Both pieces of information flow through to the agent
  via existing response channels.
- A skill-less agent reading an `evidence_required` response for a
  batch state gets (a) the default child template from the description
  text, and (b) the entry schema from the same description. Not as
  machine-parseable as a dedicated `task_schema` object, but
  sufficient -- and it works without introducing new response concepts.
- The koto-user skill can still document the canonical pattern for
  agents that have it. The description field provides the fallback for
  agents that don't.

**Specifics: `default_template` behavior**

| Scenario | Behavior |
|----------|----------|
| Hook has `default_template`, task entry omits `template` | Default is used |
| Hook has `default_template`, task entry supplies `template` | Per-task wins |
| Hook missing `default_template` | Compiler error (field is required) |
| `default_template` path doesn't resolve at compile time | Compiler error |
| Per-task `template` doesn't resolve at runtime | Scheduler error for that task; failure_policy applies |

Per-task `template` can't be compile-time validated because the value
comes from agent-submitted evidence. Runtime validation catches it when
the scheduler tries to compile the referenced path, and the failure
policy routes the error.
<!-- decision:end -->

```yaml
decision_result:
  status: "COMPLETE"
  chosen: "default_template on hook + surface existing description field in expects"
  confidence: "high"
  rationale: >
    Closes both gaps with minimal new surface. default_template gets
    compile-time validation for the common case. The description field
    is already on FieldSchema but silently dropped by derive_expects --
    surfacing it is a one-line change that helps any json-typed field,
    not just batch tasks. No new response concepts needed.
  assumptions:
    - "Freeform description text is sufficient for agents to parse the task entry schema; machine-readable schema can be added later if agents struggle"
    - "Template authors writing batch workflows will populate the description field -- this is convention, not enforced"
  rejected:
    - name: "New task_schema response field"
      reason: "Adds a response concept for an invariant schema; description field carries the same information through an existing channel"
    - name: "default_template only"
      reason: "Leaves schema discovery gap open for zero savings"
    - name: "Prose only"
      reason: "Neither gap is closed; no compile-time validation"
    - name: "item_schema type annotation"
      reason: "Extends accepts type system in ways Decision E7 deferred"
  report_file: "wip/design_batch-child-spawning_decision_8_report.md"
```
