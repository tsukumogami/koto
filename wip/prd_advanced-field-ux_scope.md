# /prd Scope: advanced-field-ux

## Problem Statement

The koto auto-advancement engine's caller-facing behavioral contract was never specified. Design docs cover engine internals thoroughly, but no document defines what callers should see or do when `koto next` returns a response. The `advanced` field has three different meanings depending on the code path, 9 of 14 possible outcomes are undocumented in user-facing guides, and callers misinterpret response shapes because no decision tree exists. This has already caused real confusion (issue #102) and will continue to as more templates and agent callers are built.

## Initial Scope

### In Scope
- Complete response shape catalog: all `NextResponse` variants callers can receive, with field presence and semantics
- `advanced` field contract: formal definition covering all code paths (`--to`, `--with-data`, bare `koto next`), or a replacement/deprecation path
- Caller decision tree: for each response shape, what callers should do
- Error code contract: whether `precondition_failed` remains a catch-all or splits into distinct codes for cycle detection, chain limits, ambiguous transitions, dead-end states, persistence errors
- StopReason -> NextResponse mapping: the translation rules from engine outcomes to caller-visible JSON
- Edge case contracts: SignalReceived degradation, gate-with-evidence-fallback, ActionRequiresConfirmation, `--to` skipping auto-advancement and gates

### Out of Scope
- Engine internals or refactoring (the engine works correctly)
- Template authoring guidance
- Stale `koto transition` reference cleanup (separate issue)
- AGENTS.md vs. cli-usage.md consolidation (implementation concern, not requirements)

## Research Leads

1. **What should `advanced` mean, or should it be replaced?** Three options emerged: (a) rename to something unambiguous like `state_changed`, (b) redefine with precise per-variant documentation, (c) deprecate and add a new field. The PRD should pick one and specify acceptance criteria.

2. **Should `precondition_failed` be split into distinct error codes?** Six structurally different failures collapse into one code. Callers can't programmatically distinguish "I passed bad flags" from "the template has a cycle." The PRD should decide whether callers need programmatic distinction or if message-parsing is acceptable.

3. **What is the complete set of response shapes callers can see today?** The exploration cataloged 14 outcomes but didn't verify edge case JSON shapes against actual CLI output. The PRD needs the authoritative list with exact JSON examples.

4. **Should `--to` trigger auto-advancement after landing?** Currently it doesn't, forcing an extra `koto next` call if the target is a passthrough state. The PRD should specify whether this is the intended contract.

5. **Should gate-with-evidence-fallback be visible in the response?** Currently callers seeing EvidenceRequired after a gate failure have no way to know gates failed. The PRD should decide whether this is acceptable or whether the response should carry both expects and blocking_conditions.

6. **What should callers do with each response shape?** No decision tree exists. The PRD should define the complete caller action for each response variant, including combinations like `advanced: true` + `expects` present.

## Coverage Notes

The exploration thoroughly mapped the current state but deliberately avoided prescribing solutions. The PRD process should resolve:
- Whether the contract is versioned independently of the CLI version
- Whether ActionRequiresConfirmation (`action: "confirm"`) needs distinct caller guidance beyond what AGENTS.md currently provides
- Whether SignalReceived should produce a visible signal (e.g., `interrupted: true`) or continue degrading silently
- Whether PersistenceError should map to exit code 3 (infrastructure) instead of exit code 2 (caller error)
- The canonical audience split between AGENTS.md (agent-consumed) and cli-usage.md (human-consumed)
