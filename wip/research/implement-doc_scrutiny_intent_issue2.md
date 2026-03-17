# Scrutiny Review: Intent -- Issue 2

## Sub-check 1: Design Intent Alignment

### validate_evidence()

**Location:** `src/engine/evidence.rs`

The design doc (Solution Architecture, section 5) specifies:

> Validates a `--with-data` JSON payload against the current state's `accepts` schema:
> - All required fields present
> - Field types match (string, number, boolean, enum)
> - Enum values are in the allowed set
> - No unknown fields (strict validation)
> - Returns `Ok(())` or `Err(NextError)` with `InvalidSubmission` code and per-field `ErrorDetail` entries.

The last bullet says `Err(NextError)`, but the AC explicitly overrides this: "EvidenceValidationError is a domain error in src/engine/ (not NextError)." The implementation follows the AC correctly -- `EvidenceValidationError` is a standalone domain error in `src/engine/evidence.rs`, not coupled to `NextError`. The design doc's text about returning `Err(NextError)` is the higher-level sketch; the AC refines it to keep the engine layer independent of CLI types. This is the right call and matches the dependency direction (engine doesn't import CLI types).

**Function signature:** Matches the AC exactly: `validate_evidence(data: &serde_json::Value, accepts: &BTreeMap<String, FieldSchema>) -> Result<(), EvidenceValidationError>`.

**Field-level error collection:** Implemented. Errors are accumulated in a Vec without short-circuiting, including a non-object root check that returns early (correct -- can't check fields on a non-object).

**Type validation:** All four types covered (string, number, boolean, enum). Enum validates both that the value is a string and that it matches the allowed values. Unknown field types get a defensive error.

**Unknown field rejection:** Implemented.

**Assessment:** Full alignment with design intent. No gaps.

### derive_expects()

**Location:** `src/cli/next_types.rs:229-262`

The design doc (Solution Architecture, section 6) specifies:

> 1. If state has no `accepts` block: `expects = None`
> 2. If state has `accepts`:
>    - `event_type` = `"evidence_submitted"` (constant)
>    - `fields` = map each `FieldSchema` to `ExpectsFieldSchema`
>    - `options` = filter transitions to those with `when` conditions. Omit `options` entirely if no transitions have `when`.

The implementation matches this exactly:
- Uses `state.accepts.as_ref()?` for the None case
- Sets `event_type` to `"evidence_submitted"`
- Maps FieldSchema to ExpectsFieldSchema with field_type, required, values
- Filters transitions to those with `when` conditions via `filter_map`
- Options omission handled by `skip_serializing_if = "Vec::is_empty"` on ExpectsSchema (from Issue 1)

**Assessment:** Full alignment with design intent.

### EvidenceValidationError structure

The design doc describes per-field `ErrorDetail` entries. The implementation uses `FieldError` (with identical fields: `field` and `reason`). The naming difference is intentional -- `ErrorDetail` is the CLI-layer type in `next_types.rs`, while `FieldError` is the engine-layer type. Issue 4 (dispatcher) will need to map `FieldError` to `ErrorDetail` when constructing `NextError`. This is clean separation.

**Assessment:** No issue. The mapping between layers is straightforward and Issue 4 has what it needs.

## Sub-check 2: Cross-Issue Enablement

### Issue 4 (dispatcher) dependencies on Issue 2

Issue 4's ACs reference:
- "validate evidence if `--with-data`" -- `validate_evidence()` is available with the right signature
- "call dispatcher" -- the dispatcher needs `derive_expects()` to build EvidenceRequired/Integration/IntegrationUnavailable responses

**validate_evidence() -> NextError mapping:** Issue 4 needs to convert `EvidenceValidationError` to `NextError` with `InvalidSubmission` code. The `EvidenceValidationError.field_errors` (Vec of FieldError with field/reason) maps directly to `NextError.details` (Vec of ErrorDetail with field/reason). Issue 4 has a clean conversion path.

**derive_expects() availability:** The function takes `&TemplateState` and returns `Option<ExpectsSchema>`. Issue 4's dispatcher receives `template_state: &TemplateState`, so it can call `derive_expects(template_state)` directly for any variant that needs `expects`.

**Assessment:** No gaps. Both functions provide what Issue 4 needs.

## Backward Coherence

Issue 1 established:
- Response types in `src/cli/next_types.rs` with custom Serialize
- `ExpectsSchema`, `ExpectsFieldSchema`, `ErrorDetail` types
- `skip_serializing_if` patterns for optional fields

Issue 2 builds on these conventions without changing them. `derive_expects()` lives in `next_types.rs` alongside the types it constructs. `validate_evidence()` lives in `src/engine/evidence.rs`, respecting the layer boundary (engine doesn't import CLI). No renames, restructuring, or convention changes.

**Assessment:** Consistent with Issue 1's patterns.

## Findings

No blocking or advisory findings.
