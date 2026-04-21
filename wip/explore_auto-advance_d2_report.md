<!-- decision:start id="skip-if-context-exists-v1" status="assumed" -->
### Decision: Context-Key Existence Predicates in v1

**Context**

Three condition types were identified for skip_if: template variable existence (`vars.VARNAME: {is_set: true}`), context-key existence (`context.md` present in the context store), and gate output value (`gates.NAME.FIELD: value`). Template variables and gate outputs are already accessible inside `advance_until_stop()`. Context-key existence requires threading `ContextStore` into `advance_until_stop()`, which currently only the gate-evaluator closure receives.

The motivating use case for context-key existence is `plan_context_injection`: auto-advance when `context.md` exists. This can already be expressed as a gate (`context-exists` gate type) plus skip_if referencing the gate output (`gates.context_file.exists: true`).

**Assumptions**

- The `context-exists` gate type already works correctly and is available to template authors.
- The workaround (gate + skip_if-gate-reference) is not meaningfully more verbose than a direct context predicate.
- No other v1 use case strictly requires a direct context-key predicate that cannot be expressed via a gate.

**Chosen: Defer -- require gate + gate-output reference**

Do not add direct context-key predicates to skip_if in v1. Template authors who need context-key checks add a `context-exists` gate to the state and reference its output in skip_if (`gates.context_file.exists: true`). The `advance_until_stop()` signature does not change.

**Rationale**

The workaround is one gate declaration plus a `gates.*` reference in skip_if -- not a significant authoring burden. The alternative requires threading `ContextStore` through `advance_until_stop()`, refactoring the deeply-nested `evaluate_context_exists_gate` function into a reusable utility, and extending the skip_if predicate evaluator. This is moderate scope that adds complexity with no unique capability: every context-key check that matters for skip_if can be expressed as a gate. Deferring to v2 keeps the v1 engine change minimal.

One side-effect: states using context-exists-via-gate for skip_if must set `has_gates_routing` correctly. The implementation must ensure gate output is synthesized when skip_if references `gates.*` keys, even if transitions don't. This is a separate engineering constraint captured in the design.

**Alternatives Considered**

- **Include in v1 -- thread ContextStore**: Supports a slightly more ergonomic syntax (`context.md: exists: true` directly in skip_if without a gate). Rejected because the ergonomic gain is marginal and the refactoring risk is non-trivial. The `ContextStore` trait is only passed to the gate evaluator today; widening its scope across `advance_until_stop()` requires care around lifetimes and test coverage.

**Consequences**

- `advance_until_stop()` signature is unchanged.
- Template authors needing context-key skip_if must declare a `context-exists` gate.
- The implementation must ensure `has_gates_routing` synthesis fires when skip_if (not just transitions) references `gates.*` keys.
- Direct `context.*` predicates can be added in v2 with the ContextStore threading work.
<!-- decision:end -->
