# Pragmatic Review: Issue 1 -- Variables substitution module and compile-time validation

**Verdict: Approve** (with advisory notes)

## Findings

### 1. Regex recompilation on every call -- Advisory

`src/engine/substitute.rs:63,95,112` -- `Regex::new()` is called on every invocation of `substitute()`, `validate_value()`, and `extract_refs()`. These are constant patterns. Use `std::sync::LazyLock` (stable since 1.80) or compile once and pass as a parameter. Not blocking because the call volume is low (per-state, not per-line), but it's free perf left on the table.

### 2. `Variables::is_empty()` has no non-test caller -- Advisory

`src/engine/substitute.rs:87-89` -- `is_empty()` is called only from test assertions within `substitute.rs` itself. No production code uses it. If Issue 2 or 3 needs it, add it then. Inert and small, so advisory only.

### 3. `SubstitutionError` doesn't implement `std::error::Error` -- Advisory

`src/engine/substitute.rs:21-25` -- The struct implements `Display` but not `Error`. This will force Issue 3 to work around it when propagating through `anyhow`. The project already depends on `thiserror`; a one-line derive would fix it. Not blocking because Issue 3 will surface this naturally.

### 4. No scope creep detected

The PR stays within Issue 1's acceptance criteria: type narrowing in `types.rs`, new `substitute.rs` module with `Variables`/`from_events`/`substitute`, compile-time validation in `template/types.rs`, and unit tests. No unrelated refactors, no speculative features.

### 5. `regex` dependency is new but justified

`Cargo.toml:21` -- `regex = "1"` was added. The crate had no prior `Regex::new` calls anywhere. The dependency is proportionate to the need (matching `{{KEY}}` patterns in arbitrary strings). No concern.

### 6. Compile-time validation is correctly placed

The variable ref checks in `CompiledTemplate::validate()` (lines 176-196 of `template/types.rs`) follow the existing validation pattern exactly -- same loop, same error style. Clean integration, no new abstraction.

## Summary

Straightforward implementation that matches the design without over-engineering. The `Variables` newtype is justified by its two confirmed callers (gate closure and directive retrieval in Issue 3, plus #71 downstream). No dead code beyond `is_empty()` which is trivial. The regex recompilation is the only thing worth fixing before merge but isn't blocking.
