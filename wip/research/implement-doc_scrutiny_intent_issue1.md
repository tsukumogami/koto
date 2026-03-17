# Scrutiny Review: Intent -- Issue 1

## Sub-check 1: Design Intent Alignment

### Field Presence Table

The design doc specifies an exact field presence table (line 554-565 of DESIGN). The custom `Serialize` implementation matches every cell:

| Variant | Fields serialized | Fields absent | Verdict |
|---------|------------------|---------------|---------|
| EvidenceRequired | action(execute), state, directive, advanced, expects(object), error(null) | blocking_conditions, integration | Matches |
| GateBlocked | action(execute), state, directive, advanced, expects(null), blocking_conditions(array), error(null) | integration | Matches |
| Integration | action(execute), state, directive, advanced, expects(object/null), integration(object), error(null) | blocking_conditions | Matches |
| IntegrationUnavailable | action(execute), state, directive, advanced, expects(object/null), integration(object), error(null) | blocking_conditions | Matches |
| Terminal | action(done), state, advanced, expects(null), error(null) | directive, blocking_conditions, integration | Matches |

### Serialization Pattern

The design explicitly calls out the `Event` custom serialization in `engine/types.rs` as the precedent pattern (lines 98-99 of DESIGN). The implementation follows this pattern exactly -- `impl Serialize` with `serialize_map`, no derive. Consistent with codebase conventions.

### Type Definitions vs. Design Spec

All type definitions match the design's Solution Architecture section:

- `NextResponse` enum: 5 variants with correct fields per variant
- `NextError` struct: code, message, details fields
- `NextErrorCode` enum: 6 variants with snake_case serialization
- `ExpectsSchema`: event_type, fields (BTreeMap), options (Vec with skip_serializing_if)
- `ExpectsFieldSchema`: field_type (renamed to "type"), required, values (skip_serializing_if)
- `TransitionOption`: target, when (BTreeMap)
- `BlockingCondition`: name, condition_type (renamed to "type"), status, agent_actionable
- `IntegrationOutput`: name, output (serde_json::Value)
- `IntegrationUnavailableMarker`: name, available
- `ErrorDetail`: field, reason

### Exit Code Mapping

Design specifies (line 356-366):
- gate_blocked -> 1, integration_unavailable -> 1 (transient)
- invalid_submission -> 2, precondition_failed -> 2, terminal_state -> 2, workflow_not_initialized -> 2 (caller errors)

Implementation (`exit_code()` method, lines 152-161) matches exactly.

## Sub-check 2: Cross-Issue Enablement

### Issue 2 (Evidence Validation)

Issue 2 needs:
- `ExpectsSchema`, `ExpectsFieldSchema`, `TransitionOption` -- all defined as `pub` structs with correct fields. Available.
- `ErrorDetail` -- defined as `pub`. Issue 2's `EvidenceValidationError` domain error will contain field-level details; the CLI layer maps these to `NextError` with `InvalidSubmission` code + `ErrorDetail` entries. The `ErrorDetail` type has `field` and `reason` fields which align with what validation errors need to report. Available.
- `derive_expects()` is planned to live in `next_types.rs` per Issue 2's AC. The module exists and is registered. Available.

No gaps for Issue 2.

### Issue 3 (Gate Evaluator)

Issue 3 needs:
- `BlockingCondition` type to align `GateResult` status values. The `BlockingCondition.status` field is a `String`, which can represent `"failed"`, `"timed_out"`, or `"error"` -- the three non-passing `GateResult` variants. The dispatcher (Issue 4) will map `GateResult` variants to `BlockingCondition` instances. Available.

No gaps for Issue 3.

### Issue 4 (Dispatcher)

Issue 4 needs:
- `NextResponse` enum with all 5 variants -- defined.
- `NextError` struct -- defined.
- `NextErrorCode` with `exit_code()` method -- defined.
- All supporting types -- defined.
- `dispatch_next()` returning `Result<NextResponse, NextError>` -- types available for this signature.

No gaps for Issue 4.

## Backward Coherence

First issue in sequence; no previous summary to check against. Skipped.

## Findings

No blocking or advisory findings. The implementation captures the design's intent across all dimensions: the field presence table is correctly encoded in the custom serializer, the serialization pattern follows the codebase's established `Event` precedent, all types match the design specification, exit codes map correctly, and downstream issues have the public types they need.
