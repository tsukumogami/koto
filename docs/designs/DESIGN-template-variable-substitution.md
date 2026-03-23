---
status: Proposed
problem: |
  koto's template engine declares variables (VariableDecl in src/template/types.rs)
  and carries a variables field in the WorkflowInitialized event, but neither is wired
  up. koto init accepts no --var flag, the event's variables map is always empty, and
  nothing substitutes {{KEY}} at runtime. Gate commands that need instance-specific
  values (like checking whether an issue artifact exists) have no way to reference them.
  This design covers CLI integration, validation, storage, runtime substitution, input
  sanitization, and the reusable API surface that #71 (default action execution) needs.
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
