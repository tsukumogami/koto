# Design Summary: koto-cli-output-contract

## Input Context (Phase 0)
**Source:** Issue #48 (spawned from DESIGN-unified-koto-next.md Phase 3)
**Problem:** `koto next` is a read-only stub. It needs evidence submission (`--with-data`), directed transitions (`--to`), gate evaluation, auto-advancement, and self-describing JSON output with five variants and structured errors.
**Constraints:**
- Must replace `koto transition` entirely (removed in #45)
- Event log model: all mutations are append-only events
- Evidence validated against `accepts` schema, routed by `when` conditions
- Command gates only (field gates removed in #47)
- Integration runner and `koto cancel` deferred to #49
- Exit codes: 0 success, 1 transient, 2 caller error, 3 config error

## Scope
**In scope:**
- `--with-data <json>` flag for evidence submission
- `--to <target>` flag for directed transitions
- Gate evaluation (command gates with timeout, process group kill)
- Auto-advancement loop (advance through states until stopping condition)
- Five JSON output variants (evidence required, gate-blocked, integration, terminal, integration unavailable)
- `expects` field derivation from `accepts` + `when`
- Six error codes with structured error format
- Exit code mapping

**Out of scope (deferred to #49):**
- Integration runner invocation
- `koto cancel`
- Signal handling (SIGTERM/SIGINT)
- Cycle detection in advancement loop

## Current Status
**Phase:** 0 - Setup
**Last Updated:** 2026-03-16
