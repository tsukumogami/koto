# Koto template format catalog

Authoritative reference for implementers updating `koto-author`'s
`references/template-format.md`. All field names are taken verbatim from
`src/template/compile.rs` (`SourceFrontmatter`, `SourceState`, `SourceGate`,
`SourceTransition`, `SourceFieldSchema`, `SourceActionDecl`, `SourcePollingConfig`),
the compiled types in `src/template/types.rs`, and the validation logic in
`CompiledTemplate::validate()`.

---

## 1. Top-level frontmatter fields

A template file must begin with a YAML block delimited by `---`. The fields
below appear directly under the top-level key.

| YAML key | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `name` | string | **yes** | — | Machine-readable workflow name. Used in display, caching, and error messages. |
| `version` | string | **yes** | — | Author-controlled version string (e.g. `"1.0"`). Not interpreted by koto; carries to compiled JSON for reference. |
| `description` | string | no | `""` | Free-form description. Omitted from compiled JSON when empty. |
| `initial_state` | string | **yes** | — | Name of the state where every new instance starts. Must be a key in `states`. |
| `variables` | map[string → VariableDecl] | no | `{}` | Declared variables. Omitted from compiled JSON when empty. |
| `states` | map[string → StateDecl] | **yes** | — | The state machine. Must have at least one entry. |

Compilation fails immediately if `name`, `version`, or `initial_state` is
empty, or if `states` is empty, or if `initial_state` doesn't match a
declared state name.

### 1a. VariableDecl fields

Each entry under `variables:` is a map key (the variable name, e.g. `MY_VAR`)
whose value is:

| YAML key | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `description` | string | no | `""` | Human-readable explanation of what the variable represents. |
| `required` | bool | no | `false` | If `true`, `koto init` rejects the invocation when this variable is not supplied via `--var KEY=VALUE`. |
| `default` | string | no | `""` | Fallback value when the variable is not supplied and `required` is `false`. |

Variable names in the map must use `UPPER_SNAKE_CASE` (letters, digits,
underscores, starting with a letter). This is enforced by the substitution
regex `\{\{([A-Z][A-Z0-9_]*)\}\}` — references that don't match the pattern
are never substituted.

---

## 2. State declaration fields

Each key under `states:` is a state name. Its value is a `SourceState` with
the following fields (all optional in the YAML; the compiler applies defaults):

| YAML key | Type | Default | Description |
|----------|------|---------|-------------|
| `transitions` | list[TransitionDecl] | `[]` | Outbound transitions from this state. |
| `terminal` | bool | `false` | Marks the state as a terminal (accepting) state. No transitions should be declared on terminal states. |
| `gates` | map[string → GateDecl] | `{}` | Preconditions evaluated before any transition fires. |
| `accepts` | map[string → FieldSchema] | `{}` | Evidence schema: what structured data the agent may submit when in this state. Omitted from compiled JSON when empty. |
| `integration` | string or null | `null` | Integration name. Presence causes `koto next` to return `action: "integration"` or `action: "integration_unavailable"`. Cannot appear on the same state as `default_action`. |
| `default_action` | ActionDecl or null | `null` | Automatic action executed on state entry. When `requires_confirmation` is true, `koto next` returns `action: "confirm"` before advancing. Cannot appear on the same state as `integration`. |

### Directive body sections

Every state declared in `states:` must have a matching `## <state_name>`
heading in the markdown body below the frontmatter. The compiler extracts the
content under each heading as the state's **directive**. Missing headings are
a hard compiler error.

Within a body section, the HTML comment `<!-- details -->` on its own line
splits the section into two parts:

- Content **before** the marker → `directive` (returned on every `koto next` call).
- Content **after** the marker → `details` (returned only on first visit, or when `--full` is passed).

Only the first `<!-- details -->` marker counts; subsequent occurrences
are treated as ordinary content and remain in `details`. States without the
marker have an empty `details` field, which is omitted from compiled JSON.

### Feature-to-action mapping

| Template feature | `koto next` returns `action:` |
|-----------------|-------------------------------|
| State with `accepts` block | `"evidence_required"` |
| State with failing gates and no `accepts` block | `"gate_blocked"` |
| State with `integration` (integration available) | `"integration"` |
| State with `integration` (integration unavailable) | `"integration_unavailable"` |
| Terminal state (`terminal: true`) | `"done"` |
| State with `default_action` where `requires_confirmation: true` | `"confirm"` |

---

## 3. Transition fields

Each element in a state's `transitions` list is a structured object. The
compiler previously supported a bare-string shorthand but that variant was
removed; only the structured form is valid now.

| YAML key | Type | Required | Description |
|----------|------|----------|-------------|
| `target` | string | **yes** | Name of the destination state. Must be declared in `states`. |
| `when` | map[string → scalar] | no | Condition map. Transition fires only when all key–value pairs match the submitted evidence. Absent means unconditional. |

### `when` semantics

- All conditions in a single `when` block must match simultaneously (AND semantics).
- An unconditional transition (no `when`) acts as a catch-all fallback — it fires if no conditional transition matched.
- `when` keys fall into two namespaces:
  - **Agent evidence keys** — bare field names (e.g. `decision`, `severity`). Must reference fields declared in the state's `accepts` block.
  - **Gate output keys** — dotted paths in the form `gates.<gate_name>.<field>` (e.g. `gates.ci.exit_code`). Engine-injected; not submitted by agents.
- Mixed `when` blocks (agent evidence + gate output keys) are valid.
- `when` values must be JSON scalars (string, number, boolean). Arrays and objects are rejected.
- Empty `when` blocks (`when: {}`) are rejected.

---

## 4. Gate declaration fields

Each entry under a state's `gates:` map is a gate name whose value is a
`SourceGate`:

| YAML key | Type | Required | Description |
|----------|------|----------|-------------|
| `type` | string | **yes** | Gate type. One of `"command"`, `"context-exists"`, `"context-matches"`. Any other value is a hard compiler error. |
| `command` | string | type-dependent | Shell command for `command` gates. Required and non-empty for `command` type. Ignored by other types. |
| `timeout` | u32 | no (default 0) | Timeout in seconds for `command` gates. 0 means use the built-in default of 30 s. Unused by context gates. |
| `key` | string | type-dependent | Context key for `context-exists` and `context-matches` gates. Required and non-empty for those types. |
| `pattern` | string | type-dependent | Regex pattern for `context-matches` gates. Required and non-empty. Compiled and validated at compile time. |
| `override_default` | JSON object or null | no | Instance-level override default value used by `koto overrides record`. Must match the gate type's output schema exactly (see Section 8). |

### Gate types

| Type | Passes when | Required fields |
|------|-------------|-----------------|
| `command` | Shell command exits 0 | `command` (non-empty) |
| `context-exists` | Named context key is present | `key` (non-empty) |
| `context-matches` | Content for key matches regex | `key` (non-empty), `pattern` (non-empty, valid regex) |

### Gate output schemas

Each gate type injects structured output into the `gates.<name>` namespace.
Template `when` clauses reference these fields as `gates.<gate_name>.<field>`.

| Gate type | Output fields | Types |
|-----------|--------------|-------|
| `command` | `exit_code`, `error` | number, string |
| `context-exists` | `exists`, `error` | boolean, string |
| `context-matches` | `matches`, `error` | boolean, string |

### Built-in override defaults

When no `override_default` is declared on a gate, `koto overrides record`
uses the gate type's built-in default:

| Gate type | Built-in default |
|-----------|-----------------|
| `command` | `{"exit_code": 0, "error": ""}` |
| `context-exists` | `{"exists": true, "error": ""}` |
| `context-matches` | `{"matches": true, "error": ""}` |

### Legacy gate types (rejected)

`field_not_empty` and `field_equals` gate types were removed. Templates using
them receive the error:
> `unsupported gate type "…". Field-based gates (field_not_empty, field_equals)
> have been replaced by accepts/when. Use accepts blocks for evidence schema
> and when conditions for routing.`

---

## 5. The `accepts` block

`accepts` declares the evidence schema for a state — what fields an agent may
submit when calling `koto next --with-data`.

Each key under `accepts:` is a field name. Its value is a `SourceFieldSchema`:

| YAML key | Type | Required | Description |
|----------|------|----------|-------------|
| `type` | string | **yes** | Field type. One of `"enum"`, `"string"`, `"number"`, `"boolean"`. |
| `required` | bool | no (default `false`) | Whether the agent must include this field in every submission. |
| `values` | list[string] | yes for `enum` | Allowed values for enum fields. Must be non-empty when `type` is `"enum"`. |
| `description` | string | no (default `""`) | Human-readable description of the field. Omitted from compiled JSON when empty. |

### Relationship to evidence submission

When `accepts` is declared on a state, `koto next` returns:
- `action: "evidence_required"`
- An `expects` object describing the schema (field types, required flags, enum values)
- An `expects.options` list — one entry per conditional transition showing target and `when` conditions

The agent then calls `koto next <name> --with-data '<json>'` where the JSON
object contains the field–value pairs that satisfy the `when` conditions of
the desired transition.

Agent submissions that reference `gates.*` keys are rejected with an
`InvalidSubmission` error — the `gates` namespace is reserved for
engine-injected gate output.

---

## 6. Variable substitution

### Syntax

```
{{VARIABLE_NAME}}
```

`VARIABLE_NAME` must match `[A-Z][A-Z0-9_]*` (uppercase letters, digits,
underscores, starting with a letter). References that don't match this pattern
are never expanded.

### When it happens

Substitution occurs at **runtime**, when `koto next` builds the response. The
compiled template stores the raw `{{VAR}}` tokens. Values come from:

1. Variables supplied at `koto init` time via `--var KEY=VALUE`.
2. Runtime-injected variables (provided by the engine automatically — no
   declaration needed):
   - `{{SESSION_NAME}}` — the active session name.
   - `{{SESSION_DIR}}` — the session directory path.

### Which fields support substitution

| Location | Substitution supported |
|----------|----------------------|
| State directive text (body sections) | yes |
| State details text (after `<!-- details -->`) | yes |
| Gate `command` strings | yes |
| `default_action` `command` string | yes |
| `default_action` `working_dir` string | yes |
| Frontmatter YAML values (`name`, `version`, etc.) | no |
| `accepts` field names or values | no |
| Transition `when` values | no |

### Validation at compile time

The compiler checks variable references in directives and gate commands:
- Every `{{REF}}` must be declared in the template's `variables` block,
  **or** be one of the runtime-injected names (`SESSION_DIR`, `SESSION_NAME`).
- A reference to an undeclared variable is a hard compiler error.

---

## 7. Compiler diagnostics

The compiler runs in two modes:
- **Strict** (default, used by `koto template compile`) — all diagnostics are
  hard errors.
- **Permissive** (used by `koto template compile --allow-legacy-gates`) — D5
  is downgraded to a stderr warning; D4 is suppressed entirely.

### Structural errors (no D-code assigned in source)

These are hard errors in all modes:

| Condition | Error message fragment |
|-----------|----------------------|
| File has no YAML frontmatter delimiters | `"template must begin with YAML front-matter delimited by '---'"` |
| Frontmatter is invalid YAML | `"invalid YAML: failed to parse front-matter"` |
| `name` field is missing or empty | `"missing required field: name"` |
| `version` field is missing or empty | `"missing required field: version"` |
| `initial_state` field is missing or empty | `"missing required field: initial_state"` |
| `states` map is empty | `"template has no states"` |
| `initial_state` names a state not in `states` | `"initial_state "…" is not a declared state"` |
| A state has no `## state_name` section in the body | `"state "…" has no directive section in markdown body"` |
| A state's directive is empty | `"state "…" has empty directive"` |
| Transition `target` names an undeclared state | `"state "…" references undefined transition target "…""` |
| `command` gate has empty `command` | `"command must not be empty"` |
| `context-exists` gate has empty `key` | `"context-exists gate must have a non-empty key"` |
| `context-matches` gate has empty `key` | `"context-matches gate must have a non-empty key"` |
| `context-matches` gate has empty `pattern` | `"context-matches gate must have a non-empty pattern"` |
| `context-matches` `pattern` is an invalid regex | `"invalid regex pattern "…": …"` |
| Gate type is not one of the three valid types | `"unsupported gate type "…". Field-based gates … Use accepts blocks …"` |
| `accepts` field has an invalid `type` value | `"invalid field_type "…", must be one of: enum, string, number, boolean"` |
| `accepts` `enum` field has an empty `values` list | `"enum fields must have a non-empty values list"` |
| A directive or gate command references an undeclared variable | `"variable reference '{{…}}' is not declared in the template's variables block"` |
| `integration` and `default_action` both set on one state | `"cannot have both integration and default_action"` |
| `default_action.command` is empty | `"default_action command must not be empty"` |
| `default_action.polling.timeout_secs` is 0 | `"default_action polling.timeout_secs must be greater than 0"` |

### D2: override_default schema mismatch

`override_default` must be a JSON object whose keys and value types exactly
match the gate type's output schema. Errors:

- `"override_default is not a JSON object"` — value is not an object.
- `"override_default missing required field "…""` — a required schema field is absent.
- `"override_default has unknown field "…""` — an unexpected key is present.
- `"override_default field "…" has wrong type"` — value type doesn't match schema.

### D3: gates.* when-clause path validation

When a `when` block contains `gates.*` keys, the compiler validates:
- The path has exactly three dot-separated segments: `gates.<gate>.<field>`.
  Malformed paths produce `"when clause key "…" has invalid format; expected "gates.<gate>.<field>""`.
- The gate name in position 2 must be declared in this state's `gates` map.
  `"when clause references gate "…" which is not declared in this state"`.
- The field name in position 3 must be a valid field for that gate type's
  schema. `"when clause references unknown field "…"; … gate fields: …"`.
- When conditions that reference agent evidence fields require an `accepts`
  block on the state.
  `"when conditions require an accepts block on the state"`.
- Agent evidence `when` fields must be declared in the `accepts` block.
  `"when field "…" is not declared in accepts"`.
- Enum `when` values must appear in the field's `values` list.
  `"when value "…" for enum field "…" is not in allowed values …"`.
- `when` values must be scalars, not arrays or objects.
  `"when value for field "…" must be a scalar"`.

### Evidence routing mutual exclusivity (Rule 4)

Pairwise check: for every pair of conditional transitions from the same state,
at least one shared field must have different values. Violations produce:
- `"transitions to "…" and "…" are not mutually exclusive: transitions share no fields, so both could match the same evidence"`
- `"transitions to "…" and "…" are not mutually exclusive: all shared fields have identical values, so both transitions would match"`

### D4: gate reachability (strict mode only)

For states where all conditional transitions are pure-gate (`when` contains
only `gates.*` keys), the compiler simulates the gate defaults and verifies
that at least one transition fires. Error:
> `"state "…": no transition fires when all gates use override defaults"`

Unreferenced gate output fields (schema fields never used in any `when` clause)
produce a non-fatal `warning:` to stderr. D4 is **suppressed entirely** in
permissive mode (`--allow-legacy-gates`).

### D5: legacy gate detection

A state with one or more gates but no `when` clause referencing `gates.*` is
a "legacy gate" state (old boolean pass/block pattern). In strict mode this
is a hard error:
> `"state "…": gate "…" has no gates.* routing\n  add a when clause referencing
> gates.<name>.passed, gates.<name>.error, ...\n  or use --allow-legacy-gates
> to permit boolean pass/block behavior"`

In permissive mode the same condition prints a warning to stderr and
compilation continues.

---

## 8. The `--allow-legacy-gates` flag

**Command:** `koto template compile --allow-legacy-gates <path>`

**What it does:** passes `strict = false` to `validate()`. This has two effects:

1. **D5 downgraded** — states with gates but no `gates.*` routing emit a
   `warning:` to stderr instead of a hard error. Compilation returns 0.
2. **D4 suppressed** — gate reachability checks and unreferenced-field warnings
   are skipped entirely.

D1, D2, D3, and all structural errors remain hard errors regardless of this flag.

**When to use:** migrating a legacy template that uses boolean gate behavior
(no `gates.*` when-clause routing). Pass the flag while the migration is in
progress to keep CI green. Remove once all gate-bearing states use `gates.*`
routing.

**Transitory note:** the flag has a `TODO` comment in `src/cli/mod.rs` marking
it for removal once the shirabe `work-on` template migrates to structured gate
routing. It is not intended as a permanent escape hatch. New templates should
use `gates.*` routing and never need this flag.

**`koto init` behavior:** `koto init` (which compiles on cache miss) does not
expose `--allow-legacy-gates`. On a cache miss, legacy-gate templates will fail
to initialize unless the compiled JSON is cached from a prior `koto template compile
--allow-legacy-gates` run.

---

## 9. `default_action` fields

An optional block on a state that causes the engine to execute a command
automatically on state entry.

| YAML key | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `command` | string | **yes** | — | Shell command to run. Must be non-empty. Supports `{{VAR}}` substitution. |
| `working_dir` | string | no | `""` | Working directory for the command. Supports `{{VAR}}` substitution. |
| `requires_confirmation` | bool | no | `false` | If true, `koto next` returns `action: "confirm"` with the action output before advancing. |
| `polling` | PollingConfig or null | no | `null` | Polling configuration for commands that need repeated execution. |

### PollingConfig fields

| YAML key | Type | Required | Description |
|----------|------|----------|-------------|
| `interval_secs` | u32 | yes | Seconds between poll iterations. |
| `timeout_secs` | u32 | yes | Maximum total wait time in seconds. Must be > 0. |

---

## 10. Fields added or changed in recent PRs (#120–#125)

The following changes have been merged since the initial release of the template format:

### `<!-- details -->` marker (introduced in the gate-transition-contract work)

Before this addition, each state body section produced only a `directive`.
The `<!-- details -->` split was added as part of the broader gate/transition
contract improvements. The `details` field is omitted from compiled JSON when
empty, so older compiled templates are unaffected.

### `default_action` block (new state field)

The `default_action` block was added to support automatic command execution on
state entry, with optional polling and confirmation. It cannot coexist with
`integration` on the same state.

### `override_default` on gates (new gate field)

The `override_default` field on gate declarations was added to support the
`koto overrides record` mechanism. When present, it specifies the structured
output value substituted for the gate result when an operator records an
override. The compiler validates it against the gate type's output schema (D2).

### `gates.*` when-clause routing (new routing pattern)

`when` blocks can now reference `gates.<gate_name>.<field>` paths, enabling
transitions to branch on the structured output of a gate rather than just
pass/block. The compiler validates the path structure and field names (D3) and
checks reachability (D4). Templates that don't use this pattern produce the D5
legacy-gate diagnostic.

### D5 diagnostic and `--allow-legacy-gates` flag

Added to provide a migration path for templates authored before `gates.*`
routing existed. See Section 8.

### Runtime-injected variable `SESSION_DIR`

`SESSION_DIR` was added alongside the pre-existing `SESSION_NAME`. Both are
provided by the engine at runtime and do not need to be declared in the
template's `variables` block.

---

## 11. Complete field reference (YAML → compiled JSON names)

For implementers writing documentation: the YAML source names sometimes differ
from the compiled JSON field names due to `serde` renames.

| YAML source name | Compiled JSON name | Notes |
|------------------|--------------------|-------|
| `type` (on gate or field) | `type` | `serde(rename = "type")` in Rust |
| `field_type` (SourceFieldSchema) | `type` | renamed in FieldSchema via `serde(rename = "type")` |
| `gate_type` (SourceGate) | `type` | renamed in Gate via `serde(rename = "type")` |
| `initial_state` | `initial_state` | identity |
| `terminal` | `terminal` | omitted from JSON when `false` |
| `transitions` | `transitions` | omitted from JSON when empty |
| `gates` | `gates` | omitted from JSON when empty |
| `accepts` | `accepts` | omitted from JSON when `null` / empty |
| `integration` | `integration` | omitted from JSON when `null` |
| `default_action` | `default_action` | omitted from JSON when `null` |
| `details` (body section) | `details` | omitted from JSON when empty string |
| `override_default` | `override_default` | omitted from JSON when `null` |
| `timeout` | `timeout` | omitted from JSON when 0 |
| `key` | `key` | omitted from JSON when empty |
| `pattern` | `pattern` | omitted from JSON when empty |
| `command` (gate/action) | `command` | omitted from JSON when empty |
| `working_dir` | `working_dir` | omitted from JSON when empty |
| `requires_confirmation` | `requires_confirmation` | omitted from JSON when `false` |
| `polling` | `polling` | omitted from JSON when `null` |
| `required` (VariableDecl) | `required` | omitted from JSON when `false` |
| `default` (VariableDecl) | `default` | omitted from JSON when empty |
| `description` | `description` | omitted from JSON when empty |
| `values` (FieldSchema) | `values` | omitted from JSON when empty |

---

## 12. Sources

All findings are grounded in the following files:

- `src/template/compile.rs` — `SourceFrontmatter`, `SourceState`, `SourceGate`, `SourceTransition`, `SourceFieldSchema`, `SourceActionDecl`, `SourcePollingConfig`, `compile()`, `compile_gate()`
- `src/template/types.rs` — `CompiledTemplate`, `TemplateState`, `Transition`, `Gate`, `FieldSchema`, `ActionDecl`, `PollingConfig`, `VariableDecl`, `gate_type_schema()`, `gate_type_builtin_default()`, `CompiledTemplate::validate()`, `RUNTIME_VARIABLE_NAMES`, `VAR_REF_PATTERN`, `GATES_EVIDENCE_NAMESPACE`
- `src/cli/next_types.rs` — `NextResponse` variants, `BlockingCondition`, `ExpectsSchema`, feature-to-action mapping
- `src/cli/mod.rs` — `--allow-legacy-gates` flag declaration (line ~325)
- `test/functional/fixtures/templates/` — `structured-routing.md`, `structured-gates.md`, `mixed-routing.md`, `context-gate.md`, `legacy-gates.md`, `var-substitution.md`, `simple-gates.md`, `decisions.md`, `multi-state.md`
- `plugins/koto-skills/skills/koto-author/references/template-format.md` — current documentation baseline
