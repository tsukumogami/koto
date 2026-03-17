# Scrutiny Review: Completeness - Issue 1

## Issue: feat(koto): implement response types and serialization

## AC-to-Mapping Coverage Analysis

The issue body contains 15 acceptance criteria. The requirements mapping contains 11 entries. Four ACs from the issue body have no explicit mapping entry, though all four are implemented in the code.

### Missing Mapping Entries (ACs present in issue, absent from mapping)

**AC 3: "Each variant carries the fields specified in the design's field presence table"**
- Status in code: Implemented. Enum variants match the design doc's field presence table exactly.
- Severity: Advisory (implemented but unmapped)

**AC 5: "Fields marked 'no' in the field presence table are absent from JSON output; fields marked 'null' serialize as null"**
- Status in code: Implemented. Custom Serialize impl omits "no" fields and writes null for "null" fields. Tests verify both behaviors (e.g., `serialize_gate_blocked` checks `json.get("integration").is_none()` for absent fields and `json["expects"].is_null()` for null fields).
- Severity: Advisory (implemented but unmapped)

**AC 7: "NextErrorCode enum with six variants: GateBlocked, InvalidSubmission, PreconditionFailed, IntegrationUnavailable, TerminalState, WorkflowNotInitialized"**
- Status in code: Implemented. All six variants present at lines 139-144 of next_types.rs.
- Severity: Advisory (implemented but unmapped)

**AC 14: "Unit tests for NextError serialization with error code and details"**
- Status in code: Implemented. `serialize_next_error` (line 568) and `serialize_next_error_no_details` (line 598) cover this.
- Severity: Advisory (implemented but unmapped)

### Mapped ACs - Verification

All 11 mapped ACs verified against the diff:

1. **File exists and included in mod.rs**: `pub mod next_types;` at line 1 of mod.rs. Confirmed.
2. **Five variants**: EvidenceRequired, GateBlocked, Integration, IntegrationUnavailable, Terminal. Confirmed.
3. **Custom Serialize with serialize_map**: Lines 46-125. Confirmed.
4. **NextError struct**: Lines 129-133. Confirmed.
5. **NextErrorCode snake_case**: `#[serde(rename_all = "snake_case")]` at line 137. Confirmed.
6. **Supporting types**: All seven types defined (ExpectsSchema, ExpectsFieldSchema, TransitionOption, BlockingCondition, IntegrationOutput, IntegrationUnavailableMarker, ErrorDetail). Confirmed.
7. **ExpectsSchema.options skip_serializing_if**: Line 169. Confirmed.
8. **ExpectsFieldSchema.values skip_serializing_if**: Line 179. Confirmed.
9. **Exit code derivation**: `exit_code()` method at lines 152-161. Confirmed.
10. **Unit tests for all variants**: Tests cover all five NextResponse variants plus edge cases (with/without expects, with/without options). Confirmed.
11. **All tests pass**: Claimed. Not independently verified in this review.

### Phantom ACs

None detected. All mapping entries correspond to real ACs from the issue body.

### Downstream Readiness

- Issue 2: `ExpectsSchema`, `ExpectsFieldSchema`, `TransitionOption` all present with correct field signatures.
- Issue 3: `BlockingCondition` present with correct fields.
- Issue 4: `NextResponse`, `NextError`, `NextErrorCode`, and all supporting types present. `exit_code()` method available for exit code derivation.

## Summary

No blocking findings. Four ACs are implemented but missing from the mapping (the mapping condensed 15 ACs into 11 entries). All implementation claims verified against the code.
