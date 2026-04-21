<!-- decision:start id="skip-if-schema" status="assumed" -->
### Decision: skip_if Template Schema and Validation

**Context**

koto templates declare states with `accepts` (evidence schema), `gates` (deterministic checks), and `transitions` (routing rules). A new `skip_if` field will auto-advance a state when conditions are met without requiring agent evidence. The advance loop evaluates `skip_if` after gate synthesis and before `resolve_transition()`. When `skip_if` fires, condition values are injected as synthetic evidence and `resolve_transition()` is called normally to select the target state.

The existing `when` clause on transitions uses `BTreeMap<String, serde_json::Value>` to match evidence fields via dot-path keys. Three condition types are supported:

- `verdict: proceed` — evidence field value equality
- `vars.SHARED_BRANCH: {is_set: true}` — template variable existence
- `gates.context_file.exists: true` — gate output field

The question is what YAML structure `skip_if` should use and what compile-time validation rules apply.

**Assumptions**

- v1 scope is a flat conjunction: all key-value pairs in `skip_if` must match for the predicate to fire. AND/OR composition is deferred.
- Direct context-key predicates are deferred for v1. The workaround is a `context-exists` gate referenced via `gates.NAME.exists: true` in `skip_if`.
- When `skip_if` fires, condition values are injected as synthetic evidence and `resolve_transition()` is called without modification.
- Compile-time validation must enforce that exactly one transition is reachable when `skip_if` fires.
- `skip_if` on a terminal state is a compile-time error.
- `skip_if` condition keys referencing `gates.*` with no matching declared gate produce a compile-time warning.
- `skip_if` condition keys referencing `vars.*` require no compile-time validation (variables are runtime).

**Chosen: Option A — Flat dict, reusing `when`-clause syntax**

`skip_if` is `Option<BTreeMap<String, serde_json::Value>>`, identical in type and semantics to `Transition.when`. The same dot-path keys and value matchers apply. The Rust representation in `TemplateState`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub skip_if: Option<BTreeMap<String, serde_json::Value>>,
```

The `SourceState` struct (with `deny_unknown_fields`) gains:

```rust
#[serde(default)]
skip_if: Option<HashMap<String, serde_json::Value>>,
```

Example YAML patterns:

```yaml
# Gate output condition
skip_if:
  gates.context_file.exists: true

# Evidence field equality
skip_if:
  verdict: proceed

# Template variable existence
skip_if:
  vars.SHARED_BRANCH:
    is_set: true
```

**Compile-time validation rules:**

1. **E-SKIP-TERMINAL**: `skip_if` present and `terminal: true` → compile error. A terminal state has no transitions; skip_if is unreachable.

2. **E-SKIP-NO-TRANSITIONS**: `skip_if` present and `transitions` is empty → compile error. No target to advance to.

3. **E-SKIP-AMBIGUOUS**: `skip_if` present and all transitions are conditional (`when` is `Some`) → evaluate each transition's `when` clause against the `skip_if` values at compile time. If zero transitions match, compile error (dead skip_if). If more than one transition matches, compile error (ambiguous routing). Exactly one must match.

4. **W-SKIP-GATE-ABSENT**: `skip_if` contains a key with `gates.NAME.*` prefix but no gate named `NAME` is declared on the state → compile warning. The gate output will be absent at runtime, making the condition silently unmatchable.

5. **GATES-ROUTING-EXTENSION**: The `has_gates_routing` detection in `advance.rs` must scan `skip_if` condition keys for `gates.*` references in addition to transition `when` clauses. Without this, the context-exists gate workaround fails silently because gate evidence is not injected into the merged map.

**Rationale**

Option A is the right choice because it eliminates a new syntax concept from the template language. Template authors already know the `when`-clause key-value syntax, including dot-path notation for gate output and `{is_set: true}` for variable existence. Reusing the same `BTreeMap<String, serde_json::Value>` type means:

- Zero new matchers to implement: the compile-time validation can call the same `evaluate_when_clause` logic used to check transition `when` clauses against synthetic evidence.
- The advance loop's synthetic-evidence injection strategy works directly: `skip_if` values become the evidence map, then `resolve_transition()` runs without changes.
- No new YAML keys to document beyond `skip_if` itself. Authors who know `when` already know the full syntax.

Option A also degrades predictably when authors misuse it. The flat conjunction with existing matchers covers all three supported condition types (`evidence`, `vars.*`, `gates.*`) without special-casing any of them.

**Alternatives Considered**

- **Option B (Structured predicate object with `condition` and optional `target` subfields)**: Rejected because the `target` subfield would bypass `resolve_transition()` rather than letting it run normally. This undermines the design principle that skip_if fires by injecting synthetic evidence and calling the existing resolver — it would require a separate resolution path. The `condition` nesting also adds an indentation level without adding meaning: the condition values themselves are already the only information needed, making the wrapper purely ceremonial. When an explicit `target` is desirable, an unconditional fallback transition achieves the same result more explicitly.

- **Option C (List of predicates with OR semantics)**: Rejected because OR composition is explicitly deferred for v1. Introducing a list type now locks in OR semantics even before the use cases are understood. If v1 only supports a single predicate, a list would be a more complex structure carrying a single-element list in every realistic template, with no immediate benefit. The flat dict of Option A can be extended to a list in a future minor version if OR semantics prove necessary; the reverse (collapsing a list back to a flat dict) would be a breaking change.

**Consequences**

- `SourceState` gains one field (`skip_if`) that passes through `deny_unknown_fields` cleanly.
- `TemplateState` gains one field (`skip_if`) with the same type as `Transition.when`.
- Compile-time validation adds four new checks (E-SKIP-TERMINAL, E-SKIP-NO-TRANSITIONS, E-SKIP-AMBIGUOUS, W-SKIP-GATE-ABSENT). These checks reuse the same when-clause evaluation logic, so no new matchers are required.
- The `has_gates_routing` scan in `advance.rs` must be extended to include `skip_if` keys. This is a one-line change to the detection pass.
- No changes to `resolve_transition()`. The function receives a pre-built evidence map and runs identically whether the caller is the normal evidence path or the skip_if path.
- The compile cache format (`CompiledTemplate`) gains the `skip_if` field. Because `TemplateState` intentionally omits `deny_unknown_fields`, older binaries reading a cache written by a newer binary will silently ignore the new field. This is the existing forward-compatibility contract.
<!-- decision:end -->
