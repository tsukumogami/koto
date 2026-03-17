# Validation Report: Issue 2

## Summary

All 8 scenarios passed. Evidence validation and expects derivation are fully functional.

## Scenario Results

### scenario-10: Evidence validation rejects missing required fields
**Status**: PASSED
**Test**: `engine::evidence::tests::missing_required_field`
**Verification**: Submitting `{}` against a schema with required field `decision` returns an `EvidenceValidationError` with 1 field error naming `decision` with reason containing "required field missing".

### scenario-11: Evidence validation rejects type mismatches for each type
**Status**: PASSED
**Tests**: `wrong_type_string`, `wrong_type_number`, `wrong_type_boolean`, `enum_value_mismatch`, `enum_wrong_type`
**Verification**:
- `{"name": 42}` against string schema: error "expected string" + "number"
- `{"count": "not a number"}` against number schema: error "expected number" + "string"
- `{"active": "yes"}` against boolean schema: error "expected boolean" + "string"
- `{"decision": "invalid"}` against enum with ["proceed","escalate"]: error "not in allowed values"
- `{"decision": 42}` against enum: error "expected string for enum"

### scenario-12: Evidence validation rejects unknown fields
**Status**: PASSED
**Test**: `engine::evidence::tests::unknown_field_rejected`
**Verification**: Submitting `{"name": "test", "extra": "field"}` against schema declaring only `name` returns error with field `extra` and reason containing "unknown field".

### scenario-13: Evidence validation collects all errors without short-circuit
**Status**: PASSED
**Test**: `engine::evidence::tests::multiple_errors_collected`
**Verification**: Submitting `{"count": "not a number", "decision": "maybe", "unknown": true}` against schema requiring `name` (string), `count` (number), `decision` (enum: yes/no) produces 4 field errors covering: unknown field "unknown", missing "name", wrong type for "count", invalid enum for "decision".

### scenario-14: Evidence validation accepts valid payload
**Status**: PASSED
**Tests**: `valid_payload_accepted`, `valid_payload_optional_fields_omitted`, `enum_valid_value`
**Verification**:
- `{"name": "test", "count": 42, "active": true}` against matching schema: Ok(())
- `{"name": "test"}` with optional `notes` field omitted: Ok(())
- `{"decision": "proceed"}` against enum ["proceed","escalate"]: Ok(())

### scenario-15: derive_expects returns None for state without accepts
**Status**: PASSED
**Test**: `cli::next_types::tests::derive_expects_no_accepts_returns_none`
**Verification**: A `TemplateState` with `accepts: None` returns `None` from `derive_expects`.

### scenario-16: derive_expects populates options from conditional transitions
**Status**: PASSED
**Test**: `cli::next_types::tests::derive_expects_with_accepts_and_conditional_transitions`
**Verification**: State with `accepts` declaring `decision` (enum) and `notes` (string), plus two conditional transitions (proceed->implement, escalate->review), produces `ExpectsSchema` with:
- `event_type: "evidence_submitted"`
- 2 fields with correct types and required flags
- 2 options with correct targets and `when` maps
- Serialization uses `"type"` (not `"field_type"`)

### scenario-17: derive_expects omits options when no transitions have when
**Status**: PASSED
**Test**: `cli::next_types::tests::derive_expects_with_accepts_no_conditional_transitions`
**Verification**: State with `accepts` but only unconditional transition produces `ExpectsSchema` with empty `options`. Serialized JSON omits the `options` key entirely.

## Test Execution

```
cargo test: 102 unit tests + 17 integration tests = 119 total, all passed
```
