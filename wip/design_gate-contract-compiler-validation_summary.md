# Design Summary: gate-contract-compiler-validation

## Input Context (Phase 0)

**Source:** Freeform topic (issue #118)
**Problem:** Template compiler accepts gate contract declarations without schema
validation: override_default values are unchecked JSON, gates.* when clause
references aren't validated against declared gates or gate type schemas, and no
reachability check verifies that override defaults can satisfy at least one transition.
**Constraints:**
- No circular dependencies (template/types.rs → gate.rs would be circular)
- Reachability check must not false-positive on mixed gate+agent-evidence states
- Gate schema info must live in template/types.rs or a shared module

## Current Status

**Phase:** 1 - Decision Decomposition
**Last Updated:** 2026-04-01
