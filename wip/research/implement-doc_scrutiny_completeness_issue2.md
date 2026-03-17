# Scrutiny Review: Completeness -- Issue 2

## AC-to-Mapping Coverage

All 13 acceptance criteria from the issue body are present in the requirements mapping. No missing ACs. No phantom ACs.

## Evidence Verification

### AC 1: `validate_evidence` function signature and location
- **Claim:** implemented
- **Verdict:** Confirmed. `src/engine/evidence.rs:41-94` has `pub fn validate_evidence(data: &serde_json::Value, accepts: &BTreeMap<String, FieldSchema>) -> Result<(), EvidenceValidationError>`. Exact signature matches the AC.

### AC 2: `EvidenceValidationError` is a domain error in `src/engine/`
- **Claim:** implemented
- **Verdict:** Confirmed. `src/engine/evidence.rs:18-20` defines `EvidenceValidationError` with `field_errors: Vec<FieldError>`. It is in `src/engine/`, not in `src/cli/`. The coder's key_decisions note confirms CLI maps `FieldError` to `ErrorDetail`. The CLI-side mapping code isn't in this diff (that's Issue 4's job), but the domain error itself is correctly placed.

### AC 3: Rejects missing required fields
- **Claim:** implemented
- **Verdict:** Confirmed. `src/engine/evidence.rs:73-79` checks `schema.required` and pushes a `FieldError`. Test `missing_required_field` at line 217 covers this.

### AC 4: Rejects type mismatches (string, number, boolean, enum)
- **Claim:** implemented
- **Verdict:** Confirmed. `validate_field_type` at lines 97-158 handles all four types. Tests `wrong_type_string`, `wrong_type_number`, `wrong_type_boolean`, `enum_value_mismatch`, and `enum_wrong_type` cover each case.

### AC 5: Rejects unknown fields
- **Claim:** implemented
- **Verdict:** Confirmed. Lines 61-68 iterate `obj.keys()` and push errors for keys not in `accepts`. Test `unknown_field_rejected` at line 316 covers this.

### AC 6: Collects all errors (no short-circuit)
- **Claim:** implemented
- **Verdict:** Confirmed. Errors are pushed to a `Vec<FieldError>` throughout, only checked at the end (lines 87-93). Test `multiple_errors_collected` at line 329 asserts 4 errors from a single payload.

### AC 7: `derive_expects` function signature and location
- **Claim:** implemented
- **Verdict:** Confirmed. `src/cli/next_types.rs:229-262` has `pub fn derive_expects(state: &TemplateState) -> Option<ExpectsSchema>`. Matches the AC.

### AC 8: Returns `None` without accepts
- **Claim:** implemented
- **Verdict:** Confirmed. Line 230: `let accepts = state.accepts.as_ref()?;`. Test `derive_expects_no_accepts_returns_none` at line 756 covers this.

### AC 9: Sets `event_type` to `"evidence_submitted"`
- **Claim:** implemented
- **Verdict:** Confirmed. Line 258: `event_type: "evidence_submitted".to_string()`. Tests assert this value.

### AC 10: Maps FieldSchema to ExpectsFieldSchema
- **Claim:** implemented
- **Verdict:** Confirmed. Lines 232-244 map each field. The `field_type` -> `"type"` rename is handled by `#[serde(rename = "type")]` on `ExpectsFieldSchema` (line 178). Test at line 806 verifies field mapping.

### AC 11: Populates options from `when` conditions
- **Claim:** implemented
- **Verdict:** Confirmed. Lines 246-255 filter transitions with `when` into `TransitionOption`s. Tests cover conditional, unconditional, and mixed cases (lines 762, 840, 871).

### AC 12: Evidence validation unit tests
- **Claim:** implemented
- **Verdict:** Confirmed. Tests in `src/engine/evidence.rs` cover: missing required field, wrong type for each type (string, number, boolean, enum), unknown field, valid payload, enum mismatch, multiple errors. 14 tests total in the evidence module.

### AC 13: Expects derivation unit tests
- **Claim:** implemented
- **Verdict:** Confirmed. Tests in `src/cli/next_types.rs` cover: state with accepts and conditional transitions, state with accepts and no conditional transitions, state without accepts. Also includes a mixed conditional/unconditional test.

## Summary

All 13 ACs verified against the diff. No blocking or advisory findings.
