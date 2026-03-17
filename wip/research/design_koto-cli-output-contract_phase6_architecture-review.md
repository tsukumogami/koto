# Architecture Review: DESIGN-koto-cli-output-contract

Reviewer: architect-reviewer
Date: 2026-03-16

## Summary

The design is well-structured, follows existing codebase conventions, and is implementable as specified. The core decision -- typed response enums with a pure dispatcher -- fits the established pattern (cf. `EventPayload` in `engine/types.rs`, `EngineError` in `engine/errors.rs`). No blocking architectural violations found. Several advisory items and one design gap worth addressing.

## Findings

### 1. Module placement of response types -- Advisory

The design places `NextResponse`, `NextError`, and all supporting types in `src/cli/next_types.rs`, and the dispatcher in `src/cli/next.rs`. Both live under `src/cli/`.

This is the right call for now. The response types are output-only (no `Deserialize`), used exclusively by the CLI handler. If a future consumer (e.g., a library API in `pkg/`) needs these types, they'd move to `src/engine/` or a new `src/next/` module. But that's a future concern -- no action needed.

**Verdict: Fits the architecture. No change needed.**

### 2. Gate evaluator at `src/gate.rs` -- Advisory

The design places `gate.rs` as a top-level module under `src/`, not under `src/engine/` or `src/cli/`. This is appropriate: gate evaluation is I/O (process spawning) that doesn't belong in the pure engine layer, and it's not CLI-specific either. A top-level module is the right home.

The `wait-timeout` crate is already in `Cargo.toml` under `[target.'cfg(unix)'.dependencies]`, so the design doesn't introduce a new dependency -- it uses an existing one. `libc` is a new dependency but gated behind `cfg(unix)`, consistent with the existing pattern.

**Verdict: Fits the architecture. No change needed.**

### 3. Evidence validation location -- Advisory

The design mentions `src/engine/evidence.rs` or inline for evidence validation. This should be pinned to `src/engine/evidence.rs`. Evidence validation checks field types, required flags, and enum constraints against the template's `accepts` schema -- this is domain logic, not CLI logic. Placing it in `src/engine/` keeps the dependency direction correct (cli -> engine, not the reverse).

The existing `derive_evidence()` function in `persistence.rs` already operates on evidence events. A new `evidence.rs` module in `src/engine/` that handles validation would be a clean parallel.

**Verdict: Pin to `src/engine/evidence.rs`. Minor clarification, not a structural issue.**

### 4. `NextError` as struct vs enum -- Advisory

The design defines `NextError` as a struct with a `code: NextErrorCode` field, not as an enum. This diverges from the `EngineError` pattern (which is a thiserror enum). The divergence is justified: `NextError` is a serialization type (output JSON), while `EngineError` is a Rust error type for control flow. Different purposes, different shapes.

However, the CLI currently uses `anyhow::Error` with `EngineError` downcast for exit code selection (`exit_code_for_engine_error`). The design should clarify: does `NextError` replace the ad-hoc `serde_json::json!({"error": ...})` calls in the `Next` handler, or does it coexist with the `anyhow` + `EngineError` pattern for config/IO errors?

Reading the design more carefully: exit code 3 maps to "config error" (template missing, hash mismatch, corrupt state). These are currently handled by `exit_with_error` / `exit_with_error_code` before the dispatcher is called. The dispatcher only returns `NextError` for the six domain error codes. So both patterns coexist: anyhow for pre-dispatch I/O errors, `NextError` for domain errors from the dispatcher.

**Verdict: The dual error path is correct but should be made explicit in the design. The pre-dispatch errors (template load failure, hash mismatch, corrupt state) continue using the existing `exit_with_error_code` pattern. Only the dispatcher's domain errors use `NextError`.**

### 5. Exit code overlap between existing and new patterns -- Advisory

The existing `exit_code_for_engine_error` returns 3 for `StateFileCorrupted` and 1 for everything else. The design's exit code table adds code 2 for caller errors. Currently no handler uses exit code 2.

The design introduces a three-way exit code scheme (0 = success, 1 = transient/retryable, 2 = caller error, 3 = config/corruption). This is a good semantic split, but other commands (`init`, `rewind`, `workflows`) all exit with code 1 for any error. This creates an inconsistency in exit code semantics across commands.

Not blocking because exit codes for other commands can be aligned later, and per the feedback memory, koto has no users so there's no backwards-compat concern.

**Verdict: Document that exit code 2 is introduced by this design and may be adopted by other commands later.**

### 6. Missing: how `--to` interacts with gate evaluation -- Design gap

The data flow diagram shows `--to` validates against transitions and appends a `directed_transition` event. But the design doesn't specify whether gates are evaluated for directed transitions. The dispatch flow shows gate evaluation happening after evidence/transition event appending, which means:

- Agent submits `--to plan`
- `directed_transition` event appended
- Gates evaluated
- If gates fail, the state file now has a transition event followed by a `GateBlocked` response

This is a state contract issue. If the transition event is already appended but gates block, the workflow is in the target state but the agent gets a "blocked" response about that state's gates. The design should clarify: are gates evaluated *before* or *after* appending the transition event? For `--to`, gates on the *target* state should probably be evaluated after transition (they're preconditions for the target, not the source). For evidence submission with conditional routing, gates on the target should also be post-transition.

Actually, re-reading more carefully: the design says "For this issue (#48), the flow is single-step: evaluate current state and return." The auto-advancement loop is #49. So for #48, `--to` appends the directed_transition, then the *next* call to `koto next` evaluates the new state's gates. This is coherent but means `--to` is a "fire and forget" transition that doesn't check destination gates. The design should state this explicitly.

**Verdict: Clarify that `--to` does not evaluate gates on the target state. Gate evaluation happens on the *current* state at the time `koto next` is called. This is the correct behavior for single-step (#48) but the text is ambiguous.**

### 7. `IntegrationUnavailableMarker` type -- Advisory

The design includes `IntegrationUnavailableMarker { name: String, available: bool }` where `available` is "always false." A unit struct or a type alias would be simpler, but the design explicitly wants the JSON to contain `"available": false` for self-describing output. The boolean field that's always false is ugly but serves the serialization contract. Fine.

**Verdict: Acceptable. The field exists for the JSON contract, not the Rust type system.**

### 8. `BTreeMap` vs `HashMap` for `ExpectsSchema.fields` -- Advisory

The design uses `BTreeMap<String, ExpectsFieldSchema>` for `fields` in `ExpectsSchema`. The existing codebase uses `BTreeMap` for template types (deterministic key ordering in serialized output) and `HashMap` for event payloads (where ordering doesn't matter). Using `BTreeMap` for output types is correct -- it gives deterministic JSON output for testing.

**Verdict: Fits the convention.**

### 9. Phase sequencing is correct

Phase 1 (types) -> Phase 2 (validation + expects) -> Phase 3 (gates) -> Phase 4 (dispatcher + wiring) has no dependency violations. Each phase produces a testable artifact. Phase 3 (gates) could be parallelized with Phase 2 since they're independent, but sequential is fine for a single implementer.

**Verdict: Phases are correctly sequenced.**

### 10. No parallel pattern introduction

The design doesn't create a second way to do something the codebase already handles:
- Custom `Serialize` for `NextResponse` follows the `Event` pattern
- `NextErrorCode` enum follows the `EngineError` pattern (different shape, same principle)
- Gate evaluation is genuinely new functionality
- Evidence validation is genuinely new functionality

No parallel pattern concerns.

## Simpler alternatives considered

**Could the dispatcher be skipped entirely?** The CLI handler could match on state properties directly and build JSON inline. This is the "monolithic handler" alternative the design rejected. Given that the handler already has 100 lines of boilerplate (lines 208-306 in `cli/mod.rs`) and is about to triple in complexity, extracting the classification logic into a testable function is justified. The dispatcher isn't premature abstraction -- it's extracting a function.

**Could `NextResponse` be `#[derive(Serialize)]` instead of custom?** No. The design needs flat JSON with variant-specific field presence (some fields absent vs null). Serde's built-in enum serialization (tagged, adjacent, untagged) can't express "field X present in variant A, absent in variant B, null in variant C." Custom `serialize_map` is the right tool.

**Could gate evaluation use `std::process::Command` without process groups?** Yes, but timeout would kill only the parent process, leaving children running. The design's `setpgid`/`killpg` approach is the standard Unix solution. Not over-engineering.

## Overall Assessment

The design is implementable, follows existing patterns, and introduces no structural violations. The typed response enum is the right abstraction for an output contract that agents will parse. The pure dispatcher function is testable and composable.

Three items worth clarifying before implementation:
1. Pin evidence validation to `src/engine/evidence.rs`
2. Explicitly state that `--to` does not evaluate target-state gates (single-step model)
3. Document the dual error path (anyhow for pre-dispatch I/O, NextError for domain errors)

None of these are blocking. Implementation can proceed.
