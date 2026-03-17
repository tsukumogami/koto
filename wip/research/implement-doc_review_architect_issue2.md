# Architect Review: Issue #2 (Evidence Validation and Expects Derivation)

## Files Changed

- `src/engine/evidence.rs` (new) -- evidence validation logic
- `src/engine/mod.rs` (modified) -- registers `evidence` module
- `src/cli/next_types.rs` (modified) -- adds `derive_expects()` function

## Findings

### Finding 1: Module placement follows the design's layering correctly

**Severity:** Not a finding -- confirming structural alignment.

Evidence validation (`validate_evidence`) lives in `src/engine/evidence.rs` and depends only on `crate::template::types::FieldSchema`. It returns a domain error type (`EvidenceValidationError`) rather than a CLI error type. The design doc explicitly states: "EvidenceValidationError is a domain error in src/engine/ (not NextError); the CLI layer maps it to NextError with InvalidSubmission code."

Expects derivation (`derive_expects`) lives in `src/cli/next_types.rs` and depends on `crate::template::types::TemplateState`. This is CLI-layer code that transforms template data into agent-facing output types.

Both dependency directions flow downward: `cli -> template` and `engine -> template`. No circular or upward dependencies.

### Finding 2: FieldError vs ErrorDetail -- intentional separation, not duplication

**Severity:** Not a finding -- confirming pattern consistency.

`engine::evidence::FieldError` (domain) and `cli::next_types::ErrorDetail` (output contract) have identical shapes (`field: String`, `reason: String`). This looks like duplication but is correct: `FieldError` is an internal domain type with no serde traits, while `ErrorDetail` is part of the serialized output contract with `#[derive(Serialize)]`. The CLI layer will map between them in Issue 4. This follows the same pattern as the codebase's `EngineError` (domain) vs CLI error JSON (output).

### Finding 3: derive_expects placement in cli/next_types.rs

**Severity:** Advisory

`derive_expects()` is placed in `src/cli/next_types.rs` alongside the type definitions it produces. This is what the design doc specifies and the acceptance criteria require. The function takes a `&TemplateState` reference and returns `Option<ExpectsSchema>` -- it bridges template types to CLI output types, so its placement in the CLI layer is the right call.

One consideration: the file is already 900+ lines (mostly tests). When Issue 4 adds the dispatcher in a separate `next.rs`, it will import `derive_expects` from `next_types`. This is fine -- the types and their constructors co-locate well.

### Finding 4: No state contract drift

**Severity:** Not a finding -- confirming no violations.

No state file fields were added or removed. The `TemplateState` struct was not modified. Evidence validation reads from the existing `accepts` schema on `TemplateState`, and expects derivation reads from `accepts` and `transitions`. Both are consumers of existing schema fields, not producers of new ones.

## Summary

No blocking findings. The implementation follows the architecture map from the design doc precisely:

- Engine-level domain error (`EvidenceValidationError`) stays in `src/engine/`, separate from the CLI output type (`NextError`/`ErrorDetail`)
- Dependencies flow downward (`engine -> template`, `cli -> template`)
- No new modules bypass existing patterns or introduce parallel abstractions
- The `derive_expects` function and `validate_evidence` function are both pure, taking typed inputs and returning typed outputs with no I/O, matching the design's emphasis on testability without CLI integration
