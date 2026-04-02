# Decision Report: gates.* When Clause Validation Depth

**Decision question:** How deep should gates.* when clause validation go?

**Status:** DECIDED
**Chosen option:** B — Gate + field existence

---

## Context

The current `validate_evidence_routing()` in `src/template/types.rs` splits when clause keys into agent fields and `gates.*` fields. For `gates.*` fields it only checks that the comparison value is a JSON scalar (lines 398–406). It does not verify that the gate name exists in the state's `gates:` block or that the field name is valid for the gate type.

PRD R9 requires: "Transition when clauses that reference gates.* fields reference valid gate names and fields from the gate type's schema."

Gate type output schemas are fixed and small:
- `command`: `exit_code` (number), `error` (string)
- `context-exists`: `exists` (boolean), `error` (string)
- `context-matches`: `matches` (boolean), `error` (string)

---

## Options Evaluated

### Option A: Gate existence only

Verify the gate name segment names a gate declared in the state's `gates:` block. Field names are not checked.

**Rejected.** Satisfies neither the letter nor the spirit of PRD R9. A reference like `gates.ci_check.exitt_code` (typo) passes silently and produces a when condition that can never match at runtime — the engine won't emit a field that doesn't exist. This is a latent correctness bug that the compiler is positioned to catch. Option A adds implementation complexity without completing the job.

### Option B: Gate + field existence (chosen)

Verify the gate name exists in the state's `gates:` block AND that the field name appears in that gate type's output schema.

**Chosen.** This is the minimum that fully satisfies PRD R9. The gate output schemas are small, static, and already well-defined — encoding them as a lookup table in the compiler is straightforward. It catches the most common authoring errors (wrong gate name, field name typo) at compile time rather than silently at runtime. The implementation cost is low: a static map from gate type string to allowed field names, plus two lookups per `gates.*` key.

### Option C: Gate + field existence + value type compatibility

Option B plus verifying the JSON type of the comparison value matches the schema field type (e.g., `exit_code: "zero"` → error because `exit_code` expects a number).

**Rejected for now, not permanently.** The added safety is real but marginal given how the engine actually evaluates when conditions. The engine performs a JSON equality check between the stored gate output value and the when clause value. If an author writes `exit_code: "0"` (string) instead of `exit_code: 0` (number), the condition simply won't match — same observable outcome as a field typo, but slightly less confusing because the gate name and field name are correct. Type checking is a good future extension once the core gate/field validation is in place and the schemas are encoded as structured types rather than a plain string set. Implementing type checking now couples the validator to the JSON type system before the schema representation is settled.

---

## Malformed Path Policy

A `gates.*` key has exactly three dot-separated segments: `gates`, `<gate_name>`, `<field_name>`.

**`gates.foo` (2 segments, missing field name):** Treat as a compile error. The `gates.` prefix signals intent to reference gate output. A two-segment key cannot be a valid gate output reference, and accepting it silently would mask an authoring mistake. Error message should name the key and explain the expected format.

**`gates.foo.bar.baz` (4+ segments, too deep):** Also treat as a compile error. No gate output field uses nested paths. If this were silently accepted, it would never match anything at runtime. The same reasoning applies: the `gates.` prefix signals intent, so an unexpected shape is an authoring error, not an unknown namespace.

**Rationale for strict path policy:** The agent fields path already enforces that keys match declared `accepts` fields exactly. Consistency argues for equally strict treatment of `gates.*` keys. Silently ignoring malformed keys would hide typos and produce transitions that can never fire.

---

## Summary

| Concern | Policy |
|---|---|
| Gate name not declared in state's gates block | Compile error |
| Field name not in gate type's output schema | Compile error |
| Value type mismatch (e.g., string for number field) | Deferred to future extension |
| 2-segment path (gates.foo) | Compile error |
| 4+-segment path (gates.foo.bar.baz) | Compile error |

The chosen option (B) closes the gap identified by PRD R9 with minimal implementation risk, leaves a clear extension point for type checking, and treats malformed paths as errors consistent with how agent field validation works today.
