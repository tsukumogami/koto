# Maintainer Review: Issue #2 -- Evidence Validation and Expects Derivation

## Files Reviewed

- `src/engine/evidence.rs` (new)
- `src/engine/mod.rs` (modified -- added `pub mod evidence`)
- `src/cli/next_types.rs` (modified -- added `derive_expects()` and tests)

## Findings

### 1. FieldError / ErrorDetail structural duplication -- intentional, but undocumented

**File:** `src/engine/evidence.rs:8` (`FieldError`) and `src/cli/next_types.rs:218` (`ErrorDetail`)
**Severity:** Advisory

Both types have identical fields: `field: String, reason: String`. The next developer will see these two types and wonder whether to use one or the other, or whether they should be the same type. The separation is intentional per the design (domain error vs CLI output type), but neither type's doc comment mentions the other or explains why they're distinct. When Issue 4 implements the mapping between them, adding a one-line comment like "// Maps FieldError -> ErrorDetail" at the conversion site would prevent confusion.

### 2. `validate_evidence` returns early for non-object but collects all other errors

**File:** `src/engine/evidence.rs:47-58`
**Severity:** Advisory

The function's doc comment says "checks are collected without short-circuiting," but it does short-circuit on non-object payloads (returns immediately at line 54). This is the correct behavior -- you can't check fields on a non-object -- but the doc comment creates a slight misread. A reader skimming the doc might think "no short-circuit ever" and then be surprised by the early return. The comment could say "checks are collected without short-circuiting once the payload is confirmed to be an object" to be precise.

### 3. `derive_expects` lives in `next_types.rs` -- reasonable but worth noting

**File:** `src/cli/next_types.rs:229`
**Severity:** Advisory

The function is placed in the types file rather than a separate module. For now this is fine -- it's a small pure function closely tied to `ExpectsSchema`. If more derivation/mapping functions accumulate here (e.g., the dispatcher mapping in Issue 4), the file will grow into "types + logic" territory. The current placement matches the acceptance criteria, so no action needed now.

### 4. Test quality and coverage

All tests are well-named and test exactly what their names promise. The `multiple_errors_collected` test at line 329 is particularly good -- it verifies the no-short-circuit behavior across unknown fields, missing required fields, wrong types, and enum mismatches simultaneously. The `derive_expects` tests cover all three cases specified in the acceptance criteria (no accepts, accepts with conditional transitions, accepts without conditional transitions) plus the mixed case.

### 5. `description` field dropped in `derive_expects`

**File:** `src/cli/next_types.rs:229-262`
**Severity:** Advisory

`FieldSchema` has a `description` field, but `derive_expects` doesn't carry it into `ExpectsFieldSchema`. This is correct per the design doc (the `ExpectsFieldSchema` type doesn't include `description`), but the next developer adding a field to the agent-facing schema might not realize the description was deliberately excluded. A brief comment in `derive_expects` noting "description is template-internal, not surfaced to agents" would prevent someone from cargo-culting a description field into ExpectsFieldSchema later.

## Overall Assessment

The code is clean and well-structured. The evidence validator and expects derivation are both pure functions with no hidden side effects, good error messages, and complete test coverage. The separation between domain errors (`EvidenceValidationError`) and CLI errors (`NextError`) follows the design doc's dual-error-path decision correctly. The naming is accurate throughout -- `validate_evidence` validates, `derive_expects` derives, no surprises.

No blocking findings.
