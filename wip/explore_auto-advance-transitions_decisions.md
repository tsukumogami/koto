# Exploration Decisions: auto-advance-transitions

## Round 1

- **Synthetic event format**: Use `Transitioned` with `condition_type: "skip_if"` and optional `skip_if_matched` field. Avoids new event type and `EvidenceSubmitted` semantic conflict; state-derivation and epoch-scoping logic unchanged.

- **Context-exists in v1 deferred**: Direct context-key predicates require ContextStore threading into `advance_until_stop()`. Workaround: `context-exists` gate + skip_if referencing `gates.NAME.exists: true`. Functional equivalence at lower implementation cost.

- **Transition target via synthetic-evidence injection**: skip_if condition values injected into merged evidence map before `resolve_transition()` call. Compile-time validation enforces exactly one transition match. Unconditional fallback still works as before.

- **Gate output synthesis extension required**: `has_gates_routing` check must scan skip_if condition keys for `gates.*` references in addition to transition `when` clauses. Otherwise the context-exists workaround silently breaks.

- **Scope confirmed**: Single-condition predicate (flat dict, all keys must match) for v1. AND/OR composition deferred. Introspection tooling out of scope.
