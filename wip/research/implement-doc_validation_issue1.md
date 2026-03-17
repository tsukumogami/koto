# Validation Report: Issue 1

## Summary

All 9 scenarios passed. Tests executed via `cargo test` -- all 84 unit tests and 17 integration tests passed (0 failed, 0 ignored).

## Scenario Results

### scenario-1: NextResponse EvidenceRequired serializes correct JSON shape
**Status**: PASSED
**Test**: `cli::next_types::tests::serialize_evidence_required`
**Verification**: Test constructs an EvidenceRequired variant with state="review", directive, advanced=false, and an expects schema with fields and options. Asserts:
- `action` = "execute"
- `state`, `directive`, `advanced` present with correct values
- `expects` present as object with `event_type`, `fields`, `options`
- `error` = null
- `blocking_conditions` absent
- `integration` absent

### scenario-2: NextResponse GateBlocked serializes correct JSON shape
**Status**: PASSED
**Test**: `cli::next_types::tests::serialize_gate_blocked`
**Verification**: Test constructs a GateBlocked variant with two blocking conditions. Asserts:
- `action` = "execute"
- `state`, `directive`, `advanced` present
- `blocking_conditions` present as array with 2 entries
- `expects` = null
- `error` = null
- `integration` absent

### scenario-3: NextResponse Integration serializes correct JSON shape
**Status**: PASSED
**Tests**: `cli::next_types::tests::serialize_integration`, `serialize_integration_with_expects`
**Verification**: Two tests cover Integration variant -- one with `expects: None` (serializes as null) and one with `expects: Some(...)` (serializes as object). Both assert:
- `action` = "execute"
- `state`, `directive`, `advanced` present
- `integration` present as object with `name` and `output`
- `error` = null
- `blocking_conditions` absent

### scenario-4: NextResponse IntegrationUnavailable serializes correct JSON shape
**Status**: PASSED
**Tests**: `cli::next_types::tests::serialize_integration_unavailable`, `serialize_integration_unavailable_with_expects`
**Verification**: Two tests cover IntegrationUnavailable variant. Both assert:
- `action` = "execute"
- `integration` present with `available: false`
- `error` = null
- `blocking_conditions` absent
- One test verifies `expects: null`, the other verifies `expects` as object

### scenario-5: NextResponse Terminal serializes correct JSON shape
**Status**: PASSED
**Tests**: `cli::next_types::tests::serialize_terminal`, `serialize_terminal_not_advanced`
**Verification**: Two tests cover Terminal variant with advanced=true and advanced=false. Asserts:
- `action` = "done"
- `state` present
- `advanced` present
- `error` = null
- `expects` = null
- `directive` absent
- `blocking_conditions` absent
- `integration` absent

### scenario-6: NextErrorCode serializes as snake_case strings
**Status**: PASSED
**Test**: `cli::next_types::tests::error_code_serializes_as_snake_case`
**Verification**: All 6 variants tested:
- GateBlocked -> "gate_blocked"
- InvalidSubmission -> "invalid_submission"
- PreconditionFailed -> "precondition_failed"
- IntegrationUnavailable -> "integration_unavailable"
- TerminalState -> "terminal_state"
- WorkflowNotInitialized -> "workflow_not_initialized"

### scenario-7: NextErrorCode exit code mapping
**Status**: PASSED
**Tests**: `cli::next_types::tests::exit_code_transient_errors`, `exit_code_caller_errors`
**Verification**:
- Transient (exit 1): GateBlocked, IntegrationUnavailable
- Caller (exit 2): InvalidSubmission, PreconditionFailed, TerminalState, WorkflowNotInitialized

### scenario-8: ExpectsSchema omits options when empty
**Status**: PASSED
**Tests**: `cli::next_types::tests::expects_schema_omits_empty_options`, `expects_schema_includes_options_when_present`
**Verification**:
- Empty options vec -> `options` key absent from JSON
- Non-empty options vec -> `options` key present as array

### scenario-9: ExpectsFieldSchema serializes field_type as "type" and omits empty values
**Status**: PASSED
**Tests**: `cli::next_types::tests::expects_field_schema_type_rename`, `expects_field_schema_with_values`
**Verification**:
- `field_type` field serializes as JSON key `"type"`, not `"field_type"`
- Empty `values` vec -> `values` key absent from JSON
- Non-empty `values` vec -> `values` key present as array
