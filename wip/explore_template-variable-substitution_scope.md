# Explore Scope: template-variable-substitution

## Core Question

How should koto implement `--var KEY=VALUE` support for `koto init`, covering
CLI parsing, validation against template declarations, event storage, runtime
substitution in gate commands and directive text, input sanitization, and a
reusable substitution interface that downstream features (#71 default action
execution) can share?

## Context

Issue #67 requires a child design doc before implementation. The parent design
(DESIGN-shirabe-work-on-template.md, PR #64) defines Phase 0a scope: the `--var`
feature enables issue-specific gate commands (`{{ISSUE_NUMBER}}`) and artifact-path
gates (`{{ARTIFACT_PREFIX}}`). The codebase already has `VariableDecl` in
`src/template/types.rs` and a `variables: HashMap<String, Value>` field in the
`WorkflowInitialized` event — both are defined but never populated or consumed.

Downstream dependencies: #71 (default action execution) needs the substitution
interface for action commands, #72 (work-on template) uses `{{ISSUE_NUMBER}}`
and `{{ARTIFACT_PREFIX}}` in gates and directives.

## In Scope

- `koto init --var KEY=VALUE` CLI flag (repeatable)
- Validation: reject unknown keys, enforce required, apply defaults
- Event storage: populate `WorkflowInitialized.variables`
- Runtime substitution in gate command strings (at evaluation time)
- Runtime substitution in directive text (at `koto next` time)
- Input sanitization: reject shell metacharacters in variable values
- Error behavior for undefined variable references
- Reusable substitution API surface for #71

## Out of Scope

- Default action execution (#71) — separate design
- The work-on template itself (#72) — consumes this feature
- Compile-time substitution (parent design specifies runtime)
- Workflow name validation (mentioned as separate concern in parent design)
- Structured/typed variable values beyond strings

## Research Leads

1. **What substitution syntax and edge-case behavior should koto use?**
   `{{KEY}}` is the convention in template text. Need to define: how to handle
   nested braces, literal `{{`, partial matches, and undefined variable references
   (error vs empty string). The parent design says undefined refs must error.

2. **What's the right sanitization strategy and safe character set?**
   Gate commands run in a shell. The parent design suggests rejecting metacharacters
   at init time. Need to decide exact charset (alphanumeric, hyphens, underscores,
   dots, forward slashes per issue #67) and error UX for rejected values.

3. **Where should substitution live in the code, and what API shape works for reuse?**
   #71 needs the same substitution for action commands. Options: standalone function
   in a shared module, method on a variables container, or trait. Need to evaluate
   module hierarchy and call sites.

4. **How should variable values be stored — strings or JSON Value?**
   The event field uses `serde_json::Value` but CLI input is strings. Template
   declarations have no type field. Need to decide whether to narrow to String
   or keep Value for future extensibility.

5. **How should `koto init` validate variables and report errors?**
   Required vs optional, defaults, unknown keys. The validation flow spans CLI
   parsing, template loading, and event creation. Error messages need to be
   actionable.
