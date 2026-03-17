# Architect Review: Issue 1 - Response Types and Serialization

## Summary

Issue 1 introduces `src/cli/next_types.rs` with the `NextResponse` enum (5 variants), `NextError` struct, `NextErrorCode` enum (6 variants), and all supporting types. The module is registered in `src/cli/mod.rs` via `pub mod next_types;`.

## Findings

### 1. Advisory: Module placement aligns with design but worth noting dependency direction

**File:** `src/cli/next_types.rs`
**Severity:** Advisory

The types are placed in `src/cli/` as the design specifies. This is the right call -- these are output-contract types specific to the CLI surface, not engine-level domain types. The module only depends on `std::collections::BTreeMap`, `serde`, and `serde_json::Value`, which keeps the dependency direction clean (cli depends on serde, nothing depends on cli).

No action needed. This is a positive observation.

### 2. Pattern Consistency: Custom Serialize follows established Event pattern

**File:** `src/cli/next_types.rs:46-125`
**Severity:** No issue

The custom `impl Serialize for NextResponse` using `serialize_map` matches the existing pattern in `src/engine/types.rs:98-108` where `Event` does the same thing. This is not a parallel pattern -- it's reuse of an established convention.

### 3. Advisory: `NextResponse` field presence vs. design field presence table

**File:** `src/cli/next_types.rs:46-125`
**Severity:** Advisory

The serialization implementation matches the design's field presence table with one subtlety worth verifying: the `Terminal` variant serializes `expects: null` (line 119), which matches the design table ("null" for Terminal/expects). The `GateBlocked` variant also serializes `expects: null` (line 75), matching the table. All "no" entries (absent fields) are correctly handled by not writing them.

The test at line 345 (`assert!(json["expects"].is_null())`) confirms GateBlocked serializes expects as null. The test at line 498 confirms Terminal does the same. Tests at lines 280, 357, 448 confirm the correct fields are absent.

No action needed -- the implementation is faithful to the design.

### 4. No Issues: Type definitions match design specification exactly

The enum variants, struct fields, serde attributes (`rename`, `skip_serializing_if`), and exit code mapping all match the design doc's specification. The types defined are:

- `NextResponse` (5 variants) -- matches design Section 2
- `NextError` / `NextErrorCode` (6 codes) -- matches design Section 2
- `ExpectsSchema`, `ExpectsFieldSchema`, `TransitionOption`, `BlockingCondition`, `IntegrationOutput`, `IntegrationUnavailableMarker`, `ErrorDetail` -- matches design Section 3

### 5. No Issues: Module registration is minimal and correct

`src/cli/mod.rs:1` adds `pub mod next_types;`. The types are exported but not yet consumed by the handler -- that wiring is Issue 4's scope. No premature integration.

## Overall Assessment

The implementation fits the architecture cleanly. It follows established patterns (custom `Serialize` via `serialize_map`, serde rename attributes), places types at the correct layer (cli, not engine), maintains correct dependency direction, and doesn't introduce parallel patterns. The types are a pure data contract with no I/O or side effects, making them a solid foundation for Issues 2-4.

No blocking findings.
