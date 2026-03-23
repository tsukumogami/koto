# Exploration Findings: template-variable-substitution

## Core Question

How should koto implement `--var KEY=VALUE` support for `koto init`, covering
CLI parsing, validation against template declarations, event storage, runtime
substitution in gate commands and directive text, input sanitization, and a
reusable substitution interface that downstream features (#71 default action
execution) can share?

## Round 1
### Key Insights
- `{{KEY}}` syntax is already established in templates, design docs, and test
  fixtures. No syntax decision needed — implement what exists with strict
  undefined-reference errors. (substitution-syntax lead)
- Allowlist sanitization at init time is the clear winner. Character set
  `[a-zA-Z0-9._/-]` covers all known use cases (issue numbers, artifact
  prefixes, file paths) while eliminating shell injection risk. Escaping is
  fragile; env vars don't work for directive text. (sanitization lead)
- A `Variables` newtype in `src/engine/substitute.rs` with `from_events()`
  constructor and `substitute()` method provides clean reuse across gates,
  directives, and future action execution (#71). The gate closure pattern in
  `advance.rs` enables integration without modifying `gate.rs`. (api-shape lead)
- Variables should be stored as `HashMap<String, String>`, not
  `HashMap<String, serde_json::Value>`. Template declarations are string-typed,
  CLI input is strings, and the substitution context (shell commands, directive
  text) is inherently string-based. The field is unused so the type change is
  non-breaking. (value-typing lead)
- Validation sequence: parse --var strings, reject unknown keys, enforce
  required, apply defaults, sanitize values, store in event. Error messages
  should include the variable's description from the template declaration.
  (init-validation lead)

### Tensions
- None significant. The mismatch between `serde_json::Value` in the event type
  and `String` everywhere else is the only inconsistency, resolved by narrowing
  to String.

### Gaps
- `koto query` integration: variables should probably be visible when inspecting
  workflow state, but this wasn't investigated.
- Escape mechanism for literal `{{` in gate commands or directives: not needed
  now, but the design should acknowledge the limitation.

### Decisions
- See wip/explore_template-variable-substitution_decisions.md

### User Focus
Findings are sufficient and consistent. Ready to decide on artifact type.

## Accumulated Understanding

The `--var` feature is well-scoped by the parent design and issue #67. The
codebase has the type scaffolding (`VariableDecl`, event `variables` field) but
no wiring. Five research leads converged on a consistent implementation approach:

1. **Syntax**: `{{KEY}}` with strict error on undefined references
2. **Sanitization**: Init-time allowlist `[a-zA-Z0-9._/-]`
3. **API**: `Variables` newtype in `engine/substitute.rs` with `substitute()` method
4. **Typing**: Narrow event field from `serde_json::Value` to `String`
5. **Validation**: Parse, cross-reference template, sanitize, store

No major unknowns remain. The feature spans CLI (clap flag), engine types (event
payload), a new substitution module, and integration points in gate evaluation
and directive retrieval. The substitution interface must be reusable for #71
(default action execution).

## Decision: Crystallize
