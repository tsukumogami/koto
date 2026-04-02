# Decision Report: Gate Type Schema Location for Compile-Time Validation

**Decision question:** Where does gate type schema information live for compile-time validation?

**Status:** COMPLETE
**Chosen option:** A — Static GateTypeSchema registry in template/types.rs

---

## Context

The compiler (`validate()` in `src/template/types.rs`) needs to know each gate type's output field names and value types to validate `override_default` values and `when` clause field references at compile time.

The key constraint is that `template/types.rs` cannot import from `gate.rs` — gate.rs already imports `Gate`, `GATE_TYPE_COMMAND`, `GATE_TYPE_CONTEXT_EXISTS`, and `GATE_TYPE_CONTEXT_MATCHES` from `template/types.rs`. A reverse import would create a circular dependency.

### Codebase state

- `src/template/types.rs` already owns the GATE_TYPE_* constants and the `validate()` method. It has a `VALID_FIELD_TYPES` constant and a `GateSchemaFieldType`-adjacent pattern (the `VALID_FIELD_TYPES: &[&str]` slice). Adding a typed enum and a schema lookup function here is a natural extension of what's already there.
- `src/gate.rs` owns `built_in_default()`, which returns `serde_json::Value`. The schemas are implicit in that function's JSON literals: `exit_code` (number), `error` (string), `exists`/`matches` (boolean), `error` (string).
- `gate.rs` is the gate evaluator module — it imports from `template/types.rs`, not the other way around.

---

## Option Analysis

### Option A: Static GateTypeSchema registry in template/types.rs

Add a `GateSchemaFieldType` enum (Number, String, Boolean) and a `gate_type_schema(gate_type: &str) -> Option<&'static [(&'static str, GateSchemaFieldType)]>` function alongside the existing GATE_TYPE_* constants.

**Strengths:**
- No circular dependency. `template/types.rs` already owns the GATE_TYPE_* constants; schema knowledge belongs alongside them.
- Follows the existing pattern in the file: `VALID_FIELD_TYPES` is a static slice used by validation. A static schema table is the same idiom.
- The `validate()` method can call `gate_type_schema()` directly without any new module boundaries.
- Future gate types are added in one place (constants + schema entry), keeping the contract co-located.
- No runtime allocation; a `&'static [(str, enum)]` is zero-cost.

**Weaknesses:**
- Introduces a `GateSchemaFieldType` enum that lives in `template/types.rs` rather than near the evaluators. This is a minor concern because the enum only carries compile-time meaning; it has no runtime evaluation role.

### Option B: Move built_in_default to template/types.rs (or template/schema.rs)

Move `built_in_default()` out of `gate.rs` and derive field types from JSON value shapes at runtime.

**Strengths:**
- Consolidates default values and schema in one place.

**Weaknesses:**
- `built_in_default()` is used by the override recording path, which is a runtime concern. Moving it into `template/` separates it from the evaluators it supports.
- Deriving types from JSON value shapes at compile time is awkward: `serde_json::Value` type matching requires runtime inspection even when the values are constants. A schema table (Option A) is statically typed and avoids this.
- Splitting `built_in_default()` from the rest of `gate.rs`'s evaluator logic creates a maintenance hazard: when a new gate type is added, the developer must update both `gate.rs` (evaluator) and `template/types.rs` (defaults), with no compiler enforcement linking them.

### Option C: Inline schema in validate() match arms

Hardcode field names and types directly in the `validate()` match arms.

**Strengths:**
- Simplest to implement immediately.

**Weaknesses:**
- `validate()` already has three match arms for gate types; adding schema checks inline makes each arm longer and duplicates field names if the same check is needed in multiple validation paths (e.g., `override_default` and `when` clause validation are separate passes).
- No shared abstraction means future gate types require surgical edits in multiple places inside `validate()`.
- The lack of a `GateSchemaFieldType` enum means type errors produce string comparisons rather than exhaustive match coverage, weakening correctness guarantees.

---

## Decision

**Option A** is the right choice.

`template/types.rs` already owns the GATE_TYPE_* constants and all compile-time gate validation. Adding a static `GateSchemaFieldType` enum and `gate_type_schema()` function extends that pattern without introducing new module boundaries or circular imports. It produces a statically typed, zero-allocation schema table that `validate()` can call directly, and it keeps gate type schema co-located with gate type constants — the natural home for compile-time contract knowledge.

Option B fragments the `built_in_default()` function away from the evaluator code it supports, and deriving types from JSON values at compile time is indirection without benefit. Option C avoids abstraction at the cost of duplication and weaker type safety.

---

## Rejected Options

| Option | Reason |
|--------|--------|
| B | Moves `built_in_default()` away from the evaluators it supports, creates a maintenance gap between defaults and evaluators, and requires runtime JSON value inspection to derive types that could be declared statically. |
| C | No shared abstraction forces duplication across multiple validation passes and future gate type additions. Loses exhaustive match coverage that a typed enum provides. |
