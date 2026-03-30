# Architecture Review: DESIGN-koto-next-output-contract

## Question 1: Is the architecture clear enough to implement?

**Verdict: Yes, with minor clarifications needed.**

The design maps cleanly to the codebase. Each of the six components in the Solution Architecture section corresponds to real files with accurate descriptions of their current state. The data flow diagram correctly traces template -> engine -> persistence -> serialization.

### Source reference accuracy

All four referenced source files exist and match the design's descriptions:

- **`src/cli/next_types.rs`**: `NextResponse` has six variants (the design says "five" in the doc comment at line 17, but the enum actually has six -- `ActionRequiresConfirmation` was added later). The custom `Serialize` impl writes `"execute"` for four variants exactly as described. `NextErrorCode` has six variants, all mapping to `PreconditionFailed` behavior for template/infrastructure errors as the design claims.

- **`src/engine/advance.rs`**: `StopReason::EvidenceRequired` is indeed a unit variant (line 57). `StopReason::GateBlocked` carries `BTreeMap<String, GateResult>` (line 54). The gate evaluation block at lines 291-312 confirms that `gate_results` is scoped inside the `if !template_state.gates.is_empty()` block, making it unavailable at the `EvidenceRequired` return on line 348 -- exactly the problem the design identifies.

- **`src/engine/persistence.rs`**: The existing `derive_*` functions (`derive_state_from_log`, `derive_evidence`, `derive_decisions`, `derive_machine_state`) all follow the pattern the design proposes for `derive_visit_counts`: pure functions taking `&[Event]` input. The proposed function fits naturally.

- **`src/template/compile.rs`**: `extract_directives` at line 385 collects lines per state section. The split-on-marker logic would insert cleanly after the `current_lines.join("\n")` call at line 403.

### Clarifications needed

1. **`TemplateState` location.** The design says `TemplateState.details` goes in `src/template/compile.rs` (Component 1), but `TemplateState` is defined in `src/template/types.rs` (line 47). The field addition happens in `types.rs`; only the population logic is in `compile.rs`. This is implied but worth stating explicitly.

2. **`with_substituted_directive` needs updating.** `NextResponse` has a `with_substituted_directive` method (next_types.rs lines 67-136) that maps over directives for variable substitution. When `details` is added to variants, this method must also substitute variables in `details`. The design doesn't mention this.

3. **`make_template_state` in tests.** The test helper at next_types.rs line 857 constructs `TemplateState` directly. Adding `details: String` to `TemplateState` will break all call sites that construct it directly (tests, compile.rs line 211). The design could note this as mechanical but necessary test churn.

## Question 2: Are there missing components or interfaces?

**Two gaps identified.**

### Gap 1: Variable substitution in details

The `details` field will contain template markdown with `{{VARIABLE}}` placeholders, just like `directive`. The existing variable substitution pass in the CLI handler (which calls `with_substituted_directive`) must also process `details`. The design omits this entirely.

**Recommendation:** Add to Phase 2 deliverables: "Update `with_substituted_directive` (or rename to `with_substituted_content`) to also transform the `details` field."

### Gap 2: `src/cli/next.rs` dispatch function

The design mentions code duplication in `src/cli/next.rs` (lines 42-55) and `src/cli/mod.rs` (lines 1684-1697) for `GateResult -> BlockingCondition` conversion. However, it doesn't address the `src/cli/next.rs` dispatch function as a whole.

This file contains a `dispatch_next_response` function (or similar) that converts engine results to `NextResponse` outside the main handler. It also handles the gate-with-evidence-fallback path (lines 60-73). When `StopReason::EvidenceRequired` gains `failed_gates`, the `src/cli/next.rs` logic that currently does its own gate evaluation and fallback may become partially redundant with the engine-level threading.

**Recommendation:** Clarify in Phase 2 whether `src/cli/next.rs`'s gate-fallback logic is replaced by the engine-threaded approach or still needed for the non-advance path.

## Question 3: Are the implementation phases correctly sequenced?

**Yes. The sequencing is sound.**

Phase 1 (engine and type changes) correctly comes first because it's purely additive -- new fields, new enum variant shapes, new derive function. Nothing breaks until Phase 2 changes the serialization.

Phase 2 (wire format changes) depends on all Phase 1 types being in place. The action rename, `details` conditional inclusion, and error code splitting all consume Phase 1 outputs.

Phase 3 (documentation) ships with Phase 2, which is correct since docs must reflect the new contract.

### One sequencing refinement

Within Phase 1, the `extract_directives` change should come before the `TemplateState.details` field addition, since the compiler needs to populate the field. The design lists them in the right order (deliverables list `TemplateState.details` first, `extract_directives` second), but implementation should actually do types.rs field first, then compile.rs population -- which matches the deliverable order. No change needed.

### Test strategy note

Phase 1 deliverables include "Updated tests for all of the above," which is correct. However, Phase 2's test scope is broad: integration tests, functional feature tests, and all existing serialization tests in `next_types.rs` (lines 386-1021) will need `action` assertion updates from `"execute"` to the new values. The design could call out the test update volume explicitly so implementers budget time appropriately. There are at least 12 test functions that assert `action == "execute"`.

## Question 4: Are there simpler alternatives we overlooked?

**The design already chose the simplest viable options. Two minor simplifications are possible but not recommended.**

### Considered: Skip the details field entirely

The simplest approach to the "directive repeats on every call" problem is to do nothing and let callers manage their own caching. But this punts the problem to every caller independently, and the PRD explicitly requires it. Not viable.

### Considered: Omit blocking_conditions from EvidenceRequired

Instead of threading gate results through `StopReason::EvidenceRequired`, the CLI handler could just return `EvidenceRequired` without `blocking_conditions` and let the caller infer from context. This is simpler but defeats the design's goal of making gate-with-evidence-fallback visible. The current approach (adding an `Option` field to the enum variant) is already minimal.

### Possible simplification: Use serde's built-in enum serialization instead of custom Serialize

The custom `Serialize` impl exists to control `action` values and field presence. With the rename from `"execute"` to descriptive names, each variant now has a unique action string. In theory, serde's `#[serde(tag = "action", rename_all = "snake_case")]` could replace the manual impl. However, the design's field presence rules (e.g., `expects` is `null` on GateBlocked, absent on Terminal; `blocking_conditions` present on some but not others) require fine-grained control that the custom impl provides. The custom approach is the right call.

### Possible simplification: HashMap<String, usize> vs counting only first-visit

`derive_visit_counts` returns full counts but only the `== 1` check matters now. A `HashSet<String>` returning "has been visited before" would be simpler. The design already considered and rejected this (D3 alternatives), and the reasoning is solid -- counts enable future features at zero extra cost. Agree with the design.

## Summary of findings

| Area | Status | Notes |
|------|--------|-------|
| Architecture clarity | Good | All source references verified accurate |
| Component completeness | Two gaps | Variable substitution in details; next.rs dispatch function fate |
| Phase sequencing | Correct | No reordering needed |
| Simpler alternatives | None viable | Design already chose minimal approaches |

### Recommendations

1. Add `with_substituted_directive` update to Phase 2 deliverables (variable substitution in `details`).
2. Clarify whether `src/cli/next.rs` gate-fallback logic is replaced or retained after engine-level gate threading.
3. Note that `TemplateState.details` field goes in `src/template/types.rs`, not `compile.rs`.
4. Call out the test update volume in Phase 2: ~12 serialization tests need `action` value changes.
5. Fix the stale doc comment in `next_types.rs` line 17 ("five possible responses" should be "six") as part of Phase 2.
