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

## Approaches Investigated (Phase 1)
- **Monolithic handler**: Single run_next() function with branching. Matches existing codebase style, minimal blast radius, ~300-400 lines new code.
- **Pipeline stages**: Composable stage functions with shared NextContext struct. Each stage independently testable, natural processing order, Option-heavy context.
- **Response type dispatch**: NextResponse enum with five variants + NextError enum. Compile-time exhaustiveness, serialization correctness by construction, ~600-650 lines new code.

## Selected Approach (Phase 2)
Response type dispatch: NextResponse enum with five typed variants, NextError enum with six error codes, pure dispatcher function. Matches existing EventPayload/EngineError patterns. Chosen for compile-time output contract enforcement.

## Investigation Findings (Phase 3)
- **Serialization**: Custom `impl Serialize` using `serialize_map` (same pattern as `Event`). No `Deserialize` needed. Six supporting types. Errors use plain `#[derive(Serialize)]`.
- **Gate evaluation**: `wait-timeout` (already a dep) + `libc::setpgid`/`killpg` via `pre_exec`. AND semantics (all gates evaluated). New `src/gate.rs` module. Evaluates in CLI handler, dispatcher receives `BTreeMap<String, GateResult>`.
- **Expects derivation**: Structural assembly, not computation. `accepts` -> `fields` (rename `field_type` to `type`), conditional transitions -> `options`, `event_type` = constant. Gap: policy for `options` when state has `accepts` but only unconditional transitions (omit `options`).

## Security Review (Phase 5)
**Outcome:** Option 2 (document considerations)
**Summary:** Small security surface. Two additions: payload size limit at parse time, environment inheritance documentation for gate commands. No design changes needed.

## Current Status
**Phase:** 5 - Security
**Last Updated:** 2026-03-16
