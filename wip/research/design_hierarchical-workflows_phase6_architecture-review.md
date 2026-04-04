# Architecture Review: Hierarchical Workflows Design

## Reviewer: architecture-review
## Date: 2026-04-04
## Document: docs/designs/DESIGN-hierarchical-workflows.md

---

## Question 1: Is the architecture clear enough to implement?

**Verdict: Yes, with caveats.**

The design is implementable. Each phase has concrete deliverables listing specific files. The key interfaces section specifies input/output contracts, error behavior, and the step-by-step evaluation algorithm for `children-complete`. The data flow diagram clearly shows the parent-child interaction sequence.

**Caveats:**

1. **Gate evaluator closure signature change is under-specified.** The `evaluate_gates` function (src/gate.rs:56) currently takes `(gates, working_dir, context_store, session)`. The `children-complete` gate needs access to `SessionBackend` to call `list()`. The design says "wire session backend through gate evaluator closure" (Phase 4) but doesn't specify whether this means:
   - Adding `backend: Option<&dyn SessionBackend>` as a new parameter to `evaluate_gates()`
   - Changing the closure signature in `advance_until_stop()` (which currently takes `G: Fn(&BTreeMap<String, Gate>) -> BTreeMap<String, StructuredGateResult>`)
   - Capturing the backend in the closure at the CLI handler level (src/cli/mod.rs:1649-1654)

   The third option is already used for `context_store` and `session` in the existing closure, so it's the natural fit. But this should be explicit since it affects whether `evaluate_gates()` itself changes or only the CLI-level closure does.

2. **`koto status` implementation path is incomplete.** The design says to call `derive_machine_state()` and load the compiled template. But `derive_machine_state()` (src/engine/persistence.rs:395) requires `(header, events)`, meaning it reads the full event log. The design doesn't mention that `status` needs to call `backend.read_events()` -- it implies the operation is lightweight. For large event logs this could be slow. Consider whether `read_header()` alone (which already has `template_hash`) plus a simpler state-derivation path would suffice, or document the full-log-read cost.

3. **`WorkflowMetadata` location mismatch.** The component diagram places `WorkflowMetadata` in `src/discover.rs`, but the struct lives in `src/engine/types.rs` (line 376). The `discover.rs` module imports and maps to it. The design's Phase 1 deliverable says to modify `src/discover.rs` for the `parent_workflow` field, but the actual struct modification happens in `src/engine/types.rs`. A developer following the design literally would look in the wrong file.

## Question 2: Are there missing components or interfaces?

**Two gaps identified:**

1. **Missing: `template_path` storage for `koto status`.** The design's `koto status` output includes `template_path`, which comes from the `WorkflowInitialized` event payload. But to get this, `koto status` must parse events -- not just the header. This dependency isn't called out. Alternatively, `template_path` could be stored in `StateFileHeader` (it's currently only in the init event), which would make `koto status` genuinely lightweight.

2. **Missing: `evaluate_gates` needs the current workflow name.** The `children-complete` evaluator must know which workflow is the "current" one to filter children by `parent_workflow == current_workflow`. The current `evaluate_gates()` function doesn't receive the workflow name. The CLI closure captures it, but the design doesn't specify how the evaluator for `children-complete` gets it. Options: pass it via the gate definition (add a runtime-injected field), or add it as a parameter to `evaluate_gates()`.

**No missing major components.** The four-primitive stack (lineage, gate, status, advisory lifecycle) covers the stated use cases. The `SessionBackend` trait doesn't need new methods -- `list()`, `exists()`, `read_header()`, and `read_events()` already provide what's needed.

## Question 3: Are the implementation phases correctly sequenced?

**Yes, dependencies are respected.**

- Phase 1 (lineage) is independent -- adds fields to header/metadata, no gate changes.
- Phase 2 (discovery flags) depends on Phase 1's `parent_workflow` field in SessionInfo.
- Phase 3 (`koto status`) depends on Phase 1 for the `parent_workflow` field in output, but could technically run in parallel.
- Phase 4 (`children-complete` gate) depends on Phase 1 (parent_workflow in headers to filter) and Phase 2 (list filtering logic, though the gate does its own filtering).
- Phase 5 (advisory lifecycle) depends on Phase 1 (child discovery via parent_workflow).

**One optimization:** Phases 2 and 3 are independent of each other and could run in parallel. Phase 3 doesn't depend on Phase 2's filter flags.

**One risk:** Phase 4 is the largest and most complex phase. It touches five files and introduces the only new gate type since v0.6.0. The design correctly places it after the simpler phases, but it bundles four distinct changes (gate evaluator, Gate struct fields, compiler validation, BlockingCondition.category). Consider splitting into 4a (Gate struct + compiler) and 4b (evaluator + wiring + category).

## Question 4: Are there simpler alternatives we overlooked?

**The design already chose the simplest viable approach.** The "Decisions Already Made" section explicitly records why action-based, state-level, and directory-nesting approaches were rejected. The gate-based approach reuses existing infrastructure.

**One alternative worth noting that wasn't discussed:**

**Polling-based child status via context keys instead of a new gate type.** Children could write their terminal state to a well-known context key in the parent's context store (e.g., `children/<child-name>/status`). The parent uses existing `context-exists` or `context-matches` gates to check. This requires zero new gate types, zero new Gate fields, zero advance loop changes, and zero compiler changes.

Downsides: (a) children must cooperate (write their status), (b) no automatic per-child status aggregation, (c) the "total/completed/pending" summary must be computed by the agent. The design's Decision 3 alternatives mention this as "Context-only (no new commands)" but frame it as fragile because children might crash before writing. This is fair, but the approach could work as a Phase 0 stopgap -- agents can use this pattern today with no code changes, then migrate to `children-complete` when it ships.

**No simpler alternative exists for the full feature.** The design's approach is already minimal: two optional fields on Gate, one optional field on StateFileHeader, one optional field on BlockingCondition, one new CLI command, three new CLI flags.

## Question 5: Does the component diagram accurately reflect the codebase structure?

**Mostly accurate, with corrections needed:**

1. **Correct:** Gate struct is in `src/template/types.rs`, gate evaluator is in `src/gate.rs`, advance loop is in `src/engine/advance.rs`, persistence (including `derive_machine_state`) is in `src/engine/persistence.rs`, SessionBackend trait is in `src/session/mod.rs`, LocalBackend is in `src/session/local.rs`.

2. **Incorrect:** The diagram lists `src/engine/types.rs` for `StateFileHeader` -- this is correct. But it lists `src/discover.rs` for `WorkflowMetadata` -- the struct is actually defined in `src/engine/types.rs` (line 376). `src/discover.rs` only imports and maps to it.

3. **Incorrect:** The diagram lists `src/session/` for `list() returns parent_workflow in SessionInfo`. `SessionInfo` is defined in `src/session/mod.rs` (line 17). The diagram should reference `src/session/mod.rs` specifically.

4. **Missing from diagram:** The `validate_session_id` function in `src/session/validate.rs` already accepts dots in session IDs (line 21: allows `.`, `_`, `-`), confirming the design's claim that `parent.child` naming convention works without code changes.

5. **Structural note:** The diagram implies the changes are spread across the "four layers" (Template, Engine, Session, CLI). This is accurate. The advance loop (`src/engine/advance.rs`) is correctly absent from the change list -- the design's zero-advance-loop-changes claim is verified. The advance loop's gate evaluator is a closure injected from the CLI handler, so the new gate type plugs in at the CLI level (capturing the backend) without modifying `advance_until_stop()`.

## Additional Findings

### Verified Claims

- **"seven-step pipeline"**: The advance loop (src/engine/advance.rs:197-315) has exactly seven numbered steps: (1) signal check, (2) chain limit, (3) terminal, (4) integration, (5) action, (6) gates, (7) transition resolution. Confirmed.

- **"dot-separated names are already valid"**: `validate_session_id()` in src/session/validate.rs allows `.` characters. Confirmed.

- **"list() already reads all headers"**: LocalBackend::list() (src/session/local.rs:74-121) reads every state file header via `persistence::read_header()`. Confirmed. This means adding `parent_workflow` to the header is sufficient for child discovery without a secondary index.

- **"derive_machine_state() exists but has no CLI exposure"**: Confirmed. The function exists in persistence.rs:395 but no CLI command calls it directly.

### Potential Issues

1. **Vacuous pass prevention may cause UX issues.** The design says "if zero children match, return Failed (prevent vacuous pass)". This means a parent workflow that reaches a `children-complete` gate before any children are initialized will block. The agent must spawn at least one child before the parent's gate can evaluate. This is correct semantically but should be documented in the gate's blocking_condition output so agents understand why they're blocked.

2. **Name reuse after cleanup creates phantom children.** The Security Considerations section mentions this: if parent "P" is cleaned up and a new workflow reuses name "P", orphaned children appear as children of the new "P". The design suggests `--orphaned` to detect this. An additional mitigation would be to include `parent_workflow_template_hash` alongside `parent_workflow` in the child header, enabling detection of mismatched lineage.

3. **`BlockingCondition.category` backward compatibility.** Adding `category` to the serialized JSON output of `BlockingCondition` is backward-compatible for consumers that ignore unknown fields. But the design should specify the default value when category is absent (for existing gate types during the rollout period). The design says `"corrective"` is the default for all non-`children-complete` gates, but it should be explicit that this field is always present in output (not optional) to avoid ambiguity.

---

## Summary of Recommendations

| # | Severity | Recommendation |
|---|----------|----------------|
| 1 | Medium | Specify which approach to use for passing SessionBackend to the gate evaluator (capture in CLI closure, not new parameter) |
| 2 | Medium | Clarify that `koto status` reads the full event log, or add `template_path` to StateFileHeader |
| 3 | Low | Fix WorkflowMetadata location: `src/engine/types.rs`, not `src/discover.rs` |
| 4 | Low | Consider splitting Phase 4 into 4a (struct/compiler) and 4b (evaluator/wiring) |
| 5 | Low | Document vacuous-pass blocking message in gate output contract |
| 6 | Low | Make `BlockingCondition.category` always-present (not optional) in the spec |
