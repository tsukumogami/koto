# Pragmatic Review: Issue 1 (Response Types and Serialization)

## Review Focus: Simplicity, YAGNI, KISS

## Files Reviewed

- `src/cli/next_types.rs` (new, 693 lines)
- `src/cli/mod.rs` (1 line added: `pub mod next_types;`)

## Findings

No blocking or advisory findings.

### Analysis

**Scope**: The implementation matches Issue 1's acceptance criteria exactly. No scope creep -- no extra refactors, no utility modules, no docstring additions beyond the types themselves.

**Abstractions**: All types are directly required by the design's field presence table. No speculative generality -- every field, variant, and type maps to a specific JSON output requirement.

**Potential concerns evaluated and dismissed**:

1. `IntegrationUnavailableMarker.available` (always `false`) and `BlockingCondition.agent_actionable` (always `false` for command gates) look like dead-weight booleans. However, they're part of the output contract -- the JSON schema requires these fields for agent consumption. The design explicitly specifies them. Not over-engineering.

2. Custom `impl Serialize for NextResponse` (~80 lines) is more code than `#[derive(Serialize)]` with `#[serde(flatten)]` or tag-based approaches. But this is the correct approach: the JSON output has variant-specific field presence (some fields absent vs null depending on variant), which derive-based serialization can't express. The design doc and codebase precedent (`Event` serialization) justify this.

3. Test coverage is thorough without being excessive. Each variant has at least one test asserting field presence/absence. The `skip_serializing_if` behaviors are explicitly tested. No gold-plated edge case tests.

**Types introduced**: 9 types total (`NextResponse`, `NextError`, `NextErrorCode`, `ExpectsSchema`, `ExpectsFieldSchema`, `TransitionOption`, `BlockingCondition`, `IntegrationOutput`, `IntegrationUnavailableMarker`, `ErrorDetail`). Each maps 1:1 to a concept in the design's JSON output specification. No wrapper types, no intermediate abstractions, no builder patterns.
