# Architecture Review: DESIGN-template-variable-substitution

## Review scope

Reviewed the design document against the current codebase:
- `src/engine/types.rs` -- event types, `EventPayload::WorkflowInitialized`
- `src/engine/advance.rs` -- `advance_until_stop`, closure signatures
- `src/gate.rs` -- `evaluate_gates` function
- `src/cli/mod.rs` -- `handle_next`, `Command::Init`
- `src/cli/next.rs` -- `dispatch_next` pure dispatcher
- `src/template/compile.rs` -- compiler and `CompiledTemplate::validate()`
- `src/template/types.rs` -- `VariableDecl`, `CompiledTemplate`, `Gate`

## 1. Is the architecture clear enough to implement?

**Yes.** The design specifies exact file locations, function signatures, data flow, and integration points. The three decisions (advance loop integration, compile-time validation, event type migration) are well-separated and the pseudocode maps directly to the existing code structure.

One ambiguity: the design places `Variables` in `src/engine/substitute.rs` but says `handle_next` constructs it. The `engine` module is deliberately I/O-free (the design itself notes this). `Variables` is a pure data structure with no I/O, so placement in `engine/` is fine -- but the implementer should confirm it doesn't pull in any CLI or gate dependencies.

## 2. Are there missing components or interfaces?

### 2a. Gate closure substitution site -- misalignment with advance loop (Advisory)

The design proposes substituting gate commands inside the `gate_closure` in `handle_next` (line ~767 of `src/cli/mod.rs`). This works for the current code path. However, `advance_until_stop` calls the gate closure on every state it passes through during auto-advancement. The `Variables` captured by reference in the closure will apply consistently across all states in the chain. This is correct behavior -- just worth noting it's not limited to the initial state.

### 2b. Directive substitution across all response paths (Advisory)

The design says "substitute directive text before returning `NextResponse`." In the current `handle_next`, directive text appears in **six** response construction sites within the `StopReason` match block (lines 807-928): `GateBlocked`, `EvidenceRequired` (twice), `IntegrationUnavailable`, `Integration`, and `SignalReceived` branches. The design's pseudocode shows a single `let directive = variables.substitute(&template_state.directive)` but the implementation will need to apply this in every branch that reads `final_template_state.directive`. Missing any of these creates inconsistent substitution.

**Recommendation:** Extract a helper that takes `&TemplateState` and `&Variables` and returns a substituted directive string, then use it uniformly across all branches. This prevents the six-site scatter pattern.

### 2c. `dispatch_next` also receives raw directives (Advisory)

The `--to` code path (line ~590) calls `dispatch_next(target, target_template_state, true, &gate_results)`, which internally reads `template_state.directive` directly. Substitution needs to happen *before* this call (by mutating the directive in a cloned `TemplateState` or by substituting after the response is built). The design doesn't address the `--to` path explicitly.

### 2d. No interface for querying declared variables (Not blocking)

There's no `koto query variables` or equivalent for callers to discover what variables a template expects before calling `koto init --var`. The caller must inspect the template source or compiled JSON directly. This is fine for now but worth noting as a future UX gap.

## 3. Are the implementation phases correctly sequenced?

**Yes, with one refinement.**

Phase 1 (type changes + substitute module + compile-time validation) is correctly foundational. Phase 2 (CLI flag + init validation) depends on Phase 1's type change. Phase 3 (runtime integration) depends on both.

**Refinement:** Phase 1 should include updating the existing test in `src/engine/types.rs` (`event_serializes_type_and_payload` at line 371) which constructs `WorkflowInitialized` with `HashMap::new()`. The type change from `HashMap<String, serde_json::Value>` to `HashMap<String, String>` will require updating this and the round-trip test. The design mentions the type change but not the test updates -- they're mechanical but shouldn't be forgotten.

## 4. Are there simpler alternatives we overlooked?

### 4a. Substitution at the template-state level rather than per-string

Instead of calling `variables.substitute()` on individual gate commands and directive strings at multiple sites, an alternative is to produce a "resolved" `CompiledTemplate` (or resolved `TemplateState`) after loading events -- substituting all `{{KEY}}` patterns in all gate commands and directives in one pass at template-load time. The resolved template flows through the existing code unchanged.

**Trade-off:** This is simpler at the call sites (zero changes to `dispatch_next`, `advance_until_stop`, or the six response branches) but means the template in memory diverges from the template on disk. Given that `advance_until_stop` already works on a template reference, this could cause confusion if someone logs or serializes the in-memory template. The design's per-site approach is more explicit about where substitution happens. I'd call this a wash -- both are viable, neither is clearly better.

### 4b. Substitution via environment variables instead of string replacement

The design already considered and rejected this (env vars don't work for directive text). Confirmed: directive text is returned as JSON strings to stdout, not executed as shell commands. Env-var-only substitution wouldn't cover it.

### 4c. Panic vs. Result for undefined references

The design chooses panic for undefined references in `substitute()`, reasoning that compile-time + init-time validation makes it unreachable. This is reasonable but creates a sharp edge for programmatic callers who might construct `Variables` without going through the validated init path. A `debug_assert!` + graceful fallback (return the unsubstituted pattern) would be equally safe for the validated path and less dangerous for future callers. Not blocking -- the design's rationale is sound for the current use case.

## 5. Structural findings

### 5a. Module placement fits the dependency graph (No issue)

`src/engine/substitute.rs` depends on `src/engine/types.rs` (for `Event`, `EventPayload`). No upward dependency. The CLI imports the engine module. Dependencies flow downward. Correct.

### 5b. No parallel pattern introduction (No issue)

There's no existing substitution or templating mechanism in the codebase. The `Variables` newtype is a new capability, not a duplicate.

### 5c. Event type change is safe (Confirmed)

Searched all construction sites of `EventPayload::WorkflowInitialized`. The only call site is `src/cli/mod.rs:205-208`, which passes `HashMap::new()`. The `WorkflowInitializedPayload` helper struct (line 221-225) is used only for deserialization. Empty `HashMap<String, String>` serializes identically to empty `HashMap<String, serde_json::Value>`. No breaking change.

### 5d. Compile-time validation fits existing pattern (No issue)

`CompiledTemplate::validate()` already validates transition targets, gate types, evidence routing, and field schemas. Adding variable reference validation is a natural extension of the same method. Follows the existing pattern exactly.

## Summary

| Finding | Severity | Action |
|---------|----------|--------|
| Directive substitution needed in 6+ response branches | Advisory | Extract helper to avoid scatter |
| `--to` path in `handle_next` also needs substitution | Advisory | Address in Phase 3 implementation |
| Test updates needed for type change in Phase 1 | Advisory | Include in Phase 1 deliverables |
| Panic in `substitute()` for undefined refs | Advisory | Consider debug_assert + fallback |

No blocking findings. The design respects the existing architecture: engine stays I/O-free, substitution is a pure module, compile-time validation extends the existing validator, CLI integration uses the established closure-capture pattern. The implementation phases are correctly ordered and the decisions are well-grounded in the codebase's actual structure.
