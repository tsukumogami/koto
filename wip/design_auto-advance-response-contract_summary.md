# Design summary: auto-advance-response-contract

## Input context (Phase 0)
**Source:** /explore handoff (3 rounds, 11 research agents)
**Problem:** `koto next` forces callers to double-call when auto-advancement occurs, because the `advanced: true` response flag is ambiguous about whether the caller needs to act.
**Constraints:** backward compatibility required, engine layer owns the fix, response stays lean

## Decisions (Phase 2)
1. Engine-layer accepts awareness with new StopReason::UnresolvableTransition variant
2. Add transition_count to AdvanceResult and all NextResponse variants, keep advanced unchanged

## Cross-validation (Phase 3)
No conflicts. Decisions are independent.

## Architecture review (Phase 6)
Three advisory items addressed:
- UnresolvableTransition CLI mapping: specified as exit code 2 (precondition failed)
- dispatch_next --to path: noted as out of scope (doesn't trigger double-call)
- Wrapper struct for transition_count: declined (staying consistent with advanced pattern)

## Security review (Phase 5)
**Outcome:** Option 2 (document considerations)
**Summary:** No new attack surface. transition_count exposes operational metadata already in event log.

## Current status
**Phase:** 6 - Final Review
**Last updated:** 2026-03-26
