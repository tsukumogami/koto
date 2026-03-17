# Review: Issue #2 - Evidence Validation and Expects Derivation

## Focus: Pragmatic (simplicity, YAGNI, KISS)

### Finding 1: `validate_field_type` helper -- Advisory

`src/engine/evidence.rs:97` -- `validate_field_type()` is called from exactly one place (`validate_evidence`). However, it's reasonably sized (~55 lines of match arms) and named clearly, so inlining would make `validate_evidence` harder to read. Not blocking.

**Severity: Advisory** -- the extraction is justified by the match arm size.

### Finding 2: Defensive handling of unsupported field types -- Advisory

`src/engine/evidence.rs:149-157` -- The `_` arm in `validate_field_type` handles unsupported field types. The comment says "this should be caught by template validation" which is correct -- `template/types.rs:90` defines `VALID_FIELD_TYPES` and `CompiledTemplate::validate()` rejects anything outside that set. This is impossible-case handling per heuristic 3.

However, it's a single match arm that produces a reasonable error message. In a validation function consuming external JSON, defensive handling here is cheap insurance if someone bypasses template validation or adds a new field type to `VALID_FIELD_TYPES` without updating evidence validation. Not worth blocking over.

**Severity: Advisory** -- small and inert.

### Finding 3: `description` field dropped in `derive_expects` -- Not a finding

`src/cli/next_types.rs:229-262` -- `derive_expects()` maps `FieldSchema` to `ExpectsFieldSchema`, dropping the `description` field. This matches the acceptance criteria which specifies only `field_type`, `required`, and `values` in `ExpectsFieldSchema`. Correct behavior.

### Finding 4: No scope creep detected

The diff adds exactly two things: `src/engine/evidence.rs` (validate_evidence + EvidenceValidationError) and `derive_expects()` in `src/cli/next_types.rs`. Both are specified in Issue 2's acceptance criteria. The `pub mod evidence;` line in `src/engine/mod.rs` is the only wiring change. No unrelated refactors, no new utilities, no extra docstrings on pre-existing code.

### Finding 5: Test coverage is complete and proportionate

Evidence validation tests cover all paths called out in the acceptance criteria: missing required field, wrong type for each supported type (string, number, boolean, enum), unknown field rejection, valid payload acceptance, enum value mismatch, multiple errors in one payload, non-object payload. Expects derivation tests cover: state with accepts + conditional transitions, state with accepts + no conditional transitions, state without accepts, mixed conditional/unconditional.

No over-testing -- each test targets a distinct behavior.

### Summary

Clean implementation. No blocking findings. Two advisory notes: (1) `validate_field_type` is a single-caller helper but justified by size, (2) the unsupported field type arm is technically impossible-case handling but cheap and reasonable as defense-in-depth.
