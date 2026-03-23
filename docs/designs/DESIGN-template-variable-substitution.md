---
status: Current
spawned_from:
  issue: 67
  repo: tsukumogami/koto
  parent_design: docs/designs/DESIGN-shirabe-work-on-template.md
problem: |
  koto's template engine declares variables (VariableDecl in src/template/types.rs)
  and carries a variables field in the WorkflowInitialized event, but neither is wired
  up. koto init accepts no --var flag, the event's variables map is always empty, and
  nothing substitutes {{KEY}} at runtime. Gate commands that need instance-specific
  values (like checking whether an issue artifact exists) have no way to reference them.
  This design covers CLI integration, validation, storage, runtime substitution, input
  sanitization, and the reusable API surface that #71 (default action execution) needs.
decision: |
  Add --var KEY=VALUE to koto init with init-time allowlist sanitization
  (^[a-zA-Z0-9._/-]+$), compile-time variable reference validation in the template
  compiler, and a Variables newtype in src/engine/substitute.rs that provides a
  reusable substitute() method for gates, directives, and future action commands.
  The event type narrows from HashMap<String, serde_json::Value> to
  HashMap<String, String>. Runtime substitution happens in handle_next: the gate
  closure captures Variables for command substitution, and a helper function
  substitutes directive text across all NextResponse branches.
rationale: |
  The allowlist approach eliminates command injection without the fragility of shell
  escaping or the complexity of environment variable indirection. Compile-time
  reference validation catches template typos early, matching the compiler's existing
  pattern of structural validation. The Variables newtype keeps the engine I/O-free
  (substitution stays in the CLI layer via closure capture) while providing the
  reusable interface that #71 needs. Runtime re-validation of values loaded from the
  state file provides defense in depth against file tampering.
---

# DESIGN: Template variable substitution

## Status

Proposed

## Context and problem statement

Issue #67 requires `--var KEY=VALUE` support on `koto init` so templates can reference
runtime values like issue numbers and artifact path prefixes in gate commands and
directive text. The parent design (DESIGN-shirabe-work-on-template.md) identifies this
as Phase 0a, a prerequisite for the work-on template (#72).

The type scaffolding exists: `VariableDecl` has `required`, `default`, and
`description` fields; `WorkflowInitialized` carries a `variables` map. But no code
populates or consumes these. The feature spans five areas: CLI flag parsing, validation
against template declarations, event storage, runtime substitution in gates and
directives, and input sanitization to prevent command injection through gate commands.

Downstream, #71 (default action execution) needs the same substitution interface for
action commands: default action commands reference `{{ISSUE_NUMBER}}` just like gate
commands do. The parent design notes that Phase 0b's design should coordinate with
Phase 0a on the substitution interface. The API must be reusable across gates,
directives, and default actions, not inlined into any one call site.

The parent design also identifies specific directive scenarios that need substitution:
the `done_blocked` state's directive references issue-specific recovery paths, and
override/failure directives on deterministic states reference issue-specific artifacts.
These aren't hypothetical, they're concrete requirements from the 17-state template.

## Decision drivers

- **Security**: variable values are interpolated into shell commands (`sh -c`). Command
  injection is the primary risk. The sanitization approach must eliminate it.
- **Reusability**: #71 (default action execution) needs the same substitution for
  action commands. The parent design explicitly requires Phase 0b to coordinate with
  Phase 0a on the substitution interface. It can't be inlined into gate evaluation.
- **Simplicity**: the feature is straightforward string replacement. Don't overengineer
  with traits or polymorphism.
- **Consistency with existing types**: `VariableDecl.default` is `String`, not
  `serde_json::Value`. The storage type should match.
- **Strict error handling**: undefined variable references must produce errors, not
  silent empty-string substitution. This matches koto's explicit state management
  philosophy.

## Decisions already made

These choices were settled during exploration and should be treated as constraints:

- **Substitution syntax**: `{{KEY}}` as already used in templates and design docs.
  No spaces inside braces, unclosed patterns pass through literally, no escape mechanism
  needed initially.
- **Sanitization strategy**: allowlist at init time. Character set `[a-zA-Z0-9._/-]`.
  Reject values with characters outside this set. Escaping and env-var-only approaches
  were evaluated and rejected (escaping is fragile; env vars don't work for directive text).
- **API shape**: `Variables` newtype in `src/engine/substitute.rs` with `from_events()`
  constructor and `substitute()` method. Standalone function and trait alternatives were
  evaluated; newtype provides the right balance of encapsulation and simplicity.
- **Value typing**: narrow the event field from `HashMap<String, serde_json::Value>` to
  `HashMap<String, String>`. The field is unused so this is non-breaking. Everything
  in the system is string-typed (template defaults, CLI input, shell commands, directive text).
- **Undefined references**: error at runtime, not empty string. Matches parent design
  requirement.
- **Duplicate `--var` keys**: error, not last-wins. Prevents silent override bugs.
- **Workflow name validation**: out of scope for this design. The parent design lists
  it as a separate targeted engine change (names in state file paths must be validated
  against a strict pattern to prevent path traversal). Can be implemented alongside
  `--var` or independently.

Note: the parent design also suggests `TEST_COMMAND` as a template variable with a
default of `go test ./...`, confirming that the default-value path on `VariableDecl`
is a first-class use case, not just required variables with explicit `--var` flags.

## Considered options

### Decision 1: advance loop integration

Variable substitution needs to reach two code paths: gate commands (before shell
execution) and directive text (before returning to the user). The engine's
`advance_until_stop` function already accepts closures for gate evaluation and event
appending, so the question is where `Variables` gets constructed and how it flows.

Key assumptions:
- `substitute()` is a pure string transformation — compile-time validation (Decision 2)
  guarantees all references resolve, so substitution is infallible at runtime
- #71 (default action execution) won't need substitution inside the advance loop itself;
  it'll get its own closure or modify the integration closure

#### Chosen: construct in handle_next, capture in gate closure

Construct `Variables` once in `handle_next` (where the event log is already loaded).
The gate evaluation closure captures `&variables` and substitutes gate command strings
before passing them to `evaluate_gates`. After `advance_until_stop` returns, directive
text is substituted separately before building `NextResponse`.

This follows the existing closure-capture pattern in `handle_next` — the gate closure
already captures `current_dir`, and the append closure captures `state_path_clone`.
Two substitution sites (gate closure + directive retrieval) both live in the same
function, within ~150 lines. The `advance_until_stop` signature stays unchanged,
preserving its 9 existing tests and keeping the engine module I/O-free.

#### Alternatives considered

- **Pass Variables as parameter to advance_until_stop**: changes the public signature,
  breaks 9 tests, and pushes a caller concern into the engine module which is
  deliberately I/O-free and operates on abstract closures.
- **Generic string transformer closure**: adds a closure parameter for string
  transformation that `advance_until_stop` would call on both gate commands and
  directives. Overengineered — conflates shell-command substitution with display-text
  substitution under one interface when they may diverge.

### Decision 2: variable reference validation

When a gate command contains `{{UNKNOWN}}`, the error could surface at runtime (when
the state is reached) or at compile time (when the template is compiled). This affects
what the `Variables` type needs to carry and where validation happens.

Key assumptions:
- Templates are always compiled before use (no raw markdown loaded at runtime)
- Variable names follow `[A-Z][A-Z0-9_]*` — matchable by a simple regex
- Init-time validation ensures every declared variable has a value (required or defaulted)

#### Chosen: compile-time validation of variable references

During `koto template compile`, scan directive text and gate command strings for
`{{KEY}}` patterns. Reject any reference to a variable name not in the template's
`variables` block. This is ~15 lines in `CompiledTemplate::validate()`, matching the
existing pattern that already validates transition targets, `when` fields, and enum
values.

At runtime, the `Variables` type carries only `HashMap<String, String>`. Since
compile-time validation guarantees all references are declared, and init-time validation
guarantees all declared variables have values, substitution at runtime is a simple
lookup-and-replace with no error path for well-formed templates.

#### Alternatives considered

- **Validate against resolved values map only (runtime)**: works but surfaces errors
  late — a typo in a rarely-reached state's gate command could go undetected for weeks.
  Inconsistent with the compiler's existing behavior of catching all structural errors
  early.
- **Validate against template declarations at runtime**: better diagnostics than
  values-only, but unreachable in practice. Init-time validation already ensures every
  declared variable has a value, so the distinction between "not declared" and "declared
  but missing" can't occur. Adds complexity (threading declarations to substitution
  sites) for zero benefit.

### Decision 3: event type migration

The `WorkflowInitialized` event's `variables` field is typed as
`HashMap<String, serde_json::Value>` but everything in the system is string-typed.

Key assumptions:
- No external tools or forks have written state files with populated variables
- `#[serde(default)]` remains on the field
- Future typed variables would be an additive schema change

#### Chosen: in-place type change

Change `HashMap<String, serde_json::Value>` to `HashMap<String, String>` directly in
both `EventPayload::WorkflowInitialized` (line 34) and `WorkflowInitializedPayload`
(line 224) in `src/engine/types.rs`. The field is unused — every call site passes
`HashMap::new()`, and no state file has ever contained populated variables. Empty maps
deserialize identically for both types under serde.

#### Alternatives considered

- **Keep Value, convert at API boundary**: adds a conversion layer for a type mismatch
  that doesn't exist in practice. The event type would misrepresent what it stores.
- **Custom deserializer**: solves a problem (gracefully handling non-string values)
  that can't occur. Overengineered for a field that's never been populated.

## Decision outcome

The three decisions compose cleanly. Compile-time validation (Decision 2) guarantees
all variable references are valid, which makes runtime substitution infallible — this
validates Decision 1's assumption that `substitute()` can be a pure string
transformation with no error propagation through the advance loop. The in-place type
change (Decision 3) gives the whole system a consistent `String` type from event
storage through substitution to output.

The data flow is: template compilation validates `{{KEY}}` references against
declarations → `koto init` validates `--var` values against declarations, applies
defaults, sanitizes, stores `HashMap<String, String>` in the event → `koto next`
constructs `Variables` from the event log, substitutes gate commands in the closure
and directive text after advance returns.

## Solution architecture

### Overview

The feature adds variable substitution at three points in koto's pipeline: compile-time
reference validation, init-time value validation and storage, and runtime substitution
in gate commands and directive text. Each point has a single responsibility, and the
`Variables` newtype provides the reusable runtime interface that #71 will also use.

### Components

**`src/template/compile.rs` — compile-time reference validation**

Add a validation pass in `CompiledTemplate::validate()` that scans all directive
strings and gate command strings for `{{KEY}}` patterns (regex:
`\{\{([A-Z][A-Z0-9_]*)\}\}`). For each match, check that the key exists in the
template's `variables` BTreeMap. Reject with an error naming the undeclared variable
and the state where it appears.

**`src/cli/mod.rs` — CLI flag and init-time validation**

Add `--var KEY=VALUE` as a repeatable clap flag on the `Init` command:
```rust
#[arg(long = "var", value_name = "KEY=VALUE")]
vars: Vec<String>,
```

Init-time validation sequence:
1. Parse each `--var` string by splitting on first `=`. Error if no `=` or empty key.
2. Reject duplicate keys (error, not last-wins).
3. Load the compiled template's `variables` declarations.
4. Reject unknown keys not in the template's variables block.
5. Check required variables are provided.
6. Apply default values for optional variables not provided.
7. Sanitize all values against the allowlist `[a-zA-Z0-9._/-]`. Error on forbidden
   characters, naming the specific character and variable.
8. Store the resolved `HashMap<String, String>` in the `WorkflowInitialized` event.

**`src/engine/types.rs` — event type narrowing**

Change `variables: HashMap<String, serde_json::Value>` to
`variables: HashMap<String, String>` in both `EventPayload::WorkflowInitialized` and
`WorkflowInitializedPayload`. Remove the `serde_json` dependency from the variables
field.

**`src/engine/substitute.rs` — new module: Variables newtype**

```rust
pub struct Variables {
    vars: HashMap<String, String>,
}

impl Variables {
    /// Extract variables from the WorkflowInitialized event in the log.
    /// Re-validates all values against the allowlist regex as defense in depth —
    /// the state file is writable, so init-time validation alone isn't sufficient.
    pub fn from_events(events: &[Event]) -> Result<Self, SubstitutionError> {
        let vars = events.iter().find_map(|e| {
            if let EventPayload::WorkflowInitialized { variables, .. } = &e.payload {
                Some(variables.clone())
            } else {
                None
            }
        }).unwrap_or_default();
        // Re-validate every value against ^[a-zA-Z0-9._/-]+$
        // If any value fails, return error (state file may be corrupted)
        for (key, value) in &vars {
            validate_value(key, value)?;
        }
        Ok(Variables { vars })
    }

    /// Replace {{KEY}} patterns in the input string with variable values.
    /// Panics on undefined references (should not occur with compiled templates).
    pub fn substitute(&self, input: &str) -> String {
        // Regex: \{\{([A-Z][A-Z0-9_]*)\}\}
        // For each match, look up in self.vars and replace.
        // Undefined references panic — compile-time validation prevents this
        // for well-formed templates. A panic here indicates a corrupted state
        // file or a bug in the compiler.
    }
}
```

The `substitute` method is infallible for well-formed templates (compile-time validation
guarantees all references are declared, init-time validation guarantees all declared
variables have values). The panic on undefined references is a defensive assertion, not
a user-facing error path.

**`src/cli/next.rs` (`handle_next`) — integration**

```rust
// Construct Variables once from the event log
let variables = Variables::from_events(&events);

// Gate closure captures &variables
let gate_closure = |gates: &BTreeMap<String, Gate>| -> BTreeMap<String, GateResult> {
    let substituted: BTreeMap<String, Gate> = gates.iter().map(|(name, gate)| {
        let mut g = gate.clone();
        g.command = variables.substitute(&g.command);
        (name.clone(), g)
    }).collect();
    evaluate_gates(&substituted, &current_dir)
};

// After advance_until_stop returns, substitute directive text.
// handle_next has 6 branches that build NextResponse variants with directives.
// Extract a helper to avoid scatter:
fn substituted_directive(state: &TemplateState, vars: &Variables) -> String {
    vars.substitute(&state.directive)
}

// The --to code path (directed transitions) also calls dispatch_next, which
// reads template_state.directive. Substitution must happen in dispatch_next
// or its caller before the directive reaches the response.
```

### Key interfaces

| Interface | Signature | Used by |
|-----------|-----------|---------|
| `Variables::from_events` | `fn from_events(events: &[Event]) -> Self` | `handle_next` |
| `Variables::substitute` | `fn substitute(&self, input: &str) -> String` | gate closure, directive retrieval, future #71 |
| `validate_variable_refs` | added to `CompiledTemplate::validate()` | `koto template compile` |
| `--var KEY=VALUE` | clap repeatable flag | `koto init` |

The `substitute` method is the interface #71 needs. When default action execution is
implemented, the action closure in `handle_next` will call the same
`variables.substitute(&action.command)` to resolve variables in action command strings.

### Data flow

```
Template authoring          Compilation              Init                    Runtime
─────────────────          ────────────             ────                    ───────

{{ISSUE_NUMBER}} in    →   validate refs     →   --var ISSUE_NUMBER=42  →  substitute()
  gate commands            against declared        validate against          in gate closure
  and directives           variables block          declarations             and directive
                           (compile error           apply defaults            retrieval
                            if undeclared)          sanitize values
                                                   store in event
```

## Implementation approach

### Phase 1: type changes and substitution module

Change the event type from `Value` to `String`. Create `src/engine/substitute.rs` with
the `Variables` struct, `from_events`, and `substitute`. Add compile-time variable
reference validation to `CompiledTemplate::validate()`. Add unit tests for the
substitution regex, edge cases (unclosed braces, empty values), and the compile-time
validator.

Deliverables:
- `src/engine/types.rs` — type narrowing (update existing serialization and round-trip
  tests for the `HashMap<String, String>` change)
- `src/engine/substitute.rs` — new module with `Variables`, `from_events`, `substitute`,
  and the re-validation logic
- `src/template/compile.rs` — validation addition
- Unit tests for substitution regex, edge cases, compile-time validator, and
  re-validation in `from_events`

### Phase 2: CLI flag and init-time validation

Add `--var KEY=VALUE` to the `Init` command. Implement the validation sequence: parse,
reject duplicates, check against template, enforce required, apply defaults, sanitize.
Store in the `WorkflowInitialized` event. Add integration tests with a test template
that declares variables.

Deliverables:
- `src/cli/mod.rs` — `--var` flag and validation logic
- Integration tests for init with variables

### Phase 3: runtime integration

Wire substitution into `handle_next`: construct `Variables` from events, substitute in
the gate closure, substitute directive text before returning `NextResponse`. Add
end-to-end tests that init a workflow with variables, advance through states, and verify
substituted gate commands and directives.

Deliverables:
- `src/cli/next.rs` — integration in `handle_next`
- End-to-end tests

## Security considerations

Variable values are interpolated into shell commands passed to `sh -c` in gate
evaluation. Without sanitization, a value like `; rm -rf /` injected into
`test -f wip/issue_{{ISSUE_NUMBER}}_context.md` would execute arbitrary commands.

**Mitigation: init-time allowlist validation.** Values are validated against an
anchored regex `^[a-zA-Z0-9._/-]+$` at `koto init` time, before storage. The regex
must be anchored to ensure the entire value matches, not just a substring. The `+`
quantifier rejects empty values — an empty string substituted into a gate command
could change command semantics (e.g., `test -f ` with no argument). Characters
outside the set cause an immediate error. The allowlist covers all known use cases
(numeric issue numbers, kebab-case slugs, file paths with forward slashes) while
excluding every shell metacharacter (`;`, `|`, `&`, `$`, backticks, quotes, spaces,
parentheses, redirects, newlines, null bytes).

**Defense in depth: compile-time reference validation.** Template authors can't
accidentally expose substitution points — every `{{KEY}}` must correspond to a
declared variable. This prevents a template typo from creating an unresolved reference
that might interact poorly with sanitization.

**Defense in depth: runtime re-validation.** The state file (`koto-<name>.state.jsonl`)
is a writable file on disk. A manual edit, buggy script, or compromised tool could
modify variable values after init-time validation. To prevent this from feeding
unsanitized values into `sh -c`, `Variables::from_events` re-validates all values
against the allowlist regex before returning. This makes the security property local
to the `Variables` type rather than depending on a system-wide assumption that state
files aren't modified.

**Plaintext storage.** Variable values are stored in plaintext in the state file,
which is committed to feature branches. Don't store secrets (API keys, tokens,
passwords) as variable values. The CLI help text and template authoring docs should
note this.

**Single-pass substitution invariant.** Substitution must be single-pass: the output
of `substitute()` is never processed by another substitution or template expansion.
If a future feature adds a second pass, values could be crafted to inject new `{{KEY}}`
patterns. This invariant must be maintained.

**Residual risks (low severity):**

- *Path traversal via `../`*: the allowlist includes `.` and `/`, so values like
  `../../etc/passwd` are syntactically valid. This doesn't enable command injection,
  but a template command that treats a variable value as a trusted path could read
  or test unintended files. The risk depends on template authoring, not the framework.
  Template authors should use `--` before variable-derived arguments in gate commands
  where the value appears in an argument position (e.g.,
  `check-staleness.sh -- {{ISSUE_NUMBER}}`).
- *Flag injection via `-` prefix*: a value like `--help` could alter command behavior
  if substituted into an argument position. Same mitigation: template authors use `--`
  to separate flags from arguments.
- *Empty default values*: if a `VariableDecl` has `required: false` and `default: ""`
  (empty string), the `+` quantifier in the validation regex would reject it. This is
  the intended behavior — empty defaults should be explicitly set to a meaningful value
  in the template, not silently produce empty substitutions.

**Scope limitation:** this design doesn't cover workflow name validation (preventing
path traversal in state file names). The parent design lists that as a separate
targeted engine change.

## Consequences

### Positive

- Templates become instance-specific: gate commands and directives reference runtime
  values instead of requiring workarounds (glob patterns, wrapper scripts, env vars)
- The `Variables::substitute` interface is reusable by #71 (default action execution)
  with zero additional design work — action commands call the same method
- Compile-time reference validation catches template authoring errors early, consistent
  with the compiler's existing structural validation
- The type narrowing from `Value` to `String` makes the event schema honest about what
  it stores

### Negative

- The allowlist `[a-zA-Z0-9._/-]` is restrictive. Values with spaces, colons, or
  other characters are rejected. If a future use case needs broader values (e.g., a
  variable containing a commit message), the allowlist would need expansion with
  careful security review.
- `substitute()` panics on undefined references rather than returning a Result. This
  is by design (compile-time validation prevents it), but a corrupted state file
  could trigger it.
- No escape mechanism for literal `{{` in gate commands or directives. If a template
  needs to check for a file literally named `{{something}}`, there's no way to express
  it. No known use case exists.

### Mitigations

- The allowlist can be expanded in a future release by adding characters to the regex
  and documenting the security implications. The expansion is additive.
- The panic in `substitute()` includes a descriptive message pointing to the likely
  cause (corrupted state file or compiler bug). A future release could add a
  `try_substitute` that returns Result if callers need graceful handling.
- If literal `{{` is ever needed, an escape sequence (e.g., `\{\{`) can be added to
  both the compiler validator and the substitution regex.
