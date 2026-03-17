# Scrutiny Review: Justification -- Issue 1

**Issue:** #1 feat(koto): implement response types and serialization
**Focus:** justification (quality of deviation explanations)
**Reviewer perspective:** Are deviations genuine and well-reasoned, or do they disguise shortcuts?

## Summary

No deviations were reported. All 11 acceptance criteria are mapped as "implemented." The justification focus evaluates deviation quality, so the primary task is verifying that no hidden deviations are disguised as "implemented" claims.

## Analysis

### Hidden deviation check

Reviewed each "implemented" AC against the actual code in `src/cli/next_types.rs` and `src/cli/mod.rs`:

1. **File exists and included in mod.rs** -- Confirmed. `pub mod next_types;` on line 1 of `src/cli/mod.rs`.

2. **Five variants** -- Confirmed. `EvidenceRequired`, `GateBlocked`, `Integration`, `IntegrationUnavailable`, `Terminal` all present.

3. **Custom serialize_map** -- Confirmed. Manual `impl Serialize for NextResponse` at line 46, uses `serialize_map` for each variant with correct `action` field and `error: null`.

4. **NextError struct** -- Confirmed. Line 129, has `code`, `message`, `details` fields.

5. **NextErrorCode snake_case** -- Confirmed. `#[serde(rename_all = "snake_case")]` at line 137, six variants present.

6. **Supporting types** -- Confirmed. All seven supporting types present with correct field renames (`field_type` -> `"type"`, `condition_type` -> `"type"`).

7. **skip_serializing_if on options and values** -- Confirmed. Lines 169 and 179.

8. **Exit code derivation** -- Confirmed. `exit_code()` method at line 152, returns 1 for transient, 2 for caller errors.

9. **Unit tests** -- Confirmed. Tests cover all five variants, error serialization, snake_case codes, exit codes, and supporting type renames.

### Avoidance pattern check

No deviations means no "too complex for this scope" or "can be added later" rationalizations to examine. The mapping is straightforward: every AC has a direct implementation.

### Proportionality check

11 ACs, all implemented. The implementation is ~220 lines of types and serialization plus ~470 lines of tests. The ratio is appropriate for a types-and-serialization issue.

### Cross-issue enablement (informational)

Issue 2 depends on `ExpectsSchema`, `ExpectsFieldSchema`, `TransitionOption` for `derive_expects()` -- all present and public.
Issue 3 depends on nothing from this module directly (gate evaluator is independent).
Issue 4 depends on `NextResponse`, `NextError`, `NextErrorCode`, and `exit_code()` -- all present and public.

The types provide a sufficient foundation for all downstream issues.

## Findings

No blocking or advisory findings. All ACs are implemented without deviation, and no hidden deviations were detected.
