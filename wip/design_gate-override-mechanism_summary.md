# Design Summary: gate-override-mechanism

## Input Context (Phase 0)
**Source:** Freeform topic (GitHub issue #117)
**Upstream PRD:** docs/prds/PRD-gate-transition-contract.md (R4, R5, R5a, R6, R7, R8, R12)
**Roadmap:** docs/roadmaps/ROADMAP-gate-transition-contract.md (Feature 2)
**Problem:** Gate overrides have no audit trail; agents bypass failed gates by submitting workaround enum values with no rationale captured.
**Constraints:**
- Must mirror `koto decisions record` / `koto decisions list` CLI pattern
- Sticky within epoch (resets on state transition)
- 1MB rationale/--with-data size limit
- `gates` namespace reserved in evidence (R7)
- Builds on Feature 1 (StructuredGateResult, GATES_EVIDENCE_NAMESPACE)

## Current Status
**Phase:** 1 - Decision Decomposition
**Last Updated:** 2026-04-01
