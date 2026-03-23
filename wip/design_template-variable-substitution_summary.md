# Design summary: template-variable-substitution

## Input context (Phase 0)
**Source:** /explore handoff
**Problem:** koto templates declare variables and the WorkflowInitialized event carries
a variables field, but nothing is wired up. Gate commands and directive text can't
reference instance-specific values like issue numbers or artifact prefixes.
**Constraints:** must be reusable by #71 (default action execution), must prevent
command injection in gate commands, must match existing `{{KEY}}` syntax convention

## Decisions (Phase 2-3)
1. Construct Variables in handle_next, capture in gate closure (Option B)
2. Compile-time validation of variable references (Option C)
3. In-place type change from Value to String (Option A)

Cross-validation: passed, no conflicts.

## Security review (Phase 5)
**Outcome:** Option 2 (document considerations)
**Summary:** Allowlist approach is sound. Added: anchored regex requirement,
single-pass substitution invariant, residual risks (path traversal, flag injection).

## Current status
**Phase:** 5 - Security
**Last Updated:** 2026-03-22
