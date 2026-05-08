---
status: Proposed
upstream: docs/prds/PRD-local-dashboard.md
problem: |
  koto has no live visibility surface. The engine writes session state to JSONL files
  at `~/.koto/sessions/<repo-id>/<name>/koto-<name>.state.jsonl` on every advance, but
  reading that state requires either raw JSON parsing or invoking `koto status` per
  session. For users monitoring 100–1000 parallel child sessions in a batch pipeline,
  this forces either manual polling loops or staying blind. A terminal UI must be
  added to the existing synchronous Rust binary that can read session state, derive
  hierarchy from parent-child headers, poll for changes, and render a live tree — all
  without introducing an async runtime.
decision: |
  Add a `koto dashboard [<name>]` subcommand backed by a ratatui TUI. The implementation
  splits into three layers: a data layer that reuses existing persistence functions
  (`derive_machine_state`, `derive_last_gate_evaluated`) to read and derive session
  state; an application state layer that holds the full session tree and handles
  expand/collapse, cursor position, and poll timing; and a rendering layer built on
  ratatui widgets. The event loop uses crossterm's synchronous `poll`/`read` API
  (no async) with a tick-based poll cycle. A `--once` flag bypasses the TUI entirely
  and writes plain-text output for scripting.
rationale: |
  ratatui with crossterm is the only TUI option that stays fully synchronous — it uses
  `std::io` and blocking-with-timeout event reads, matching koto's existing architecture.
  Reusing `src/engine/persistence.rs` functions for state derivation avoids duplicating
  logic and stays consistent with `koto status` behavior. The three-layer separation
  (data / app state / render) keeps TUI rendering code isolated from session logic,
  enabling unit tests of the data and state layers without a PTY.
---

# DESIGN: Local Dashboard

## Status

Proposed

## Context and Problem Statement

koto sessions accumulate state in JSONL files — one file per session, written atomically
on every advance. The engine already provides functions to derive current state from these
files (`derive_machine_state`, `derive_state_from_log`, `derive_last_gate_evaluated` in
`src/engine/persistence.rs`), and `koto status` uses them to produce a one-shot JSON
snapshot. But there is no continuous view: users monitoring a batch run with 100+ parallel
child sessions must script their own polling or re-invoke `koto status` repeatedly.

The technical challenge is building a live terminal UI on top of a purely synchronous
codebase. koto has no async runtime (no tokio, no async-std), and adding one would be a
significant architectural change. The event loop for a TUI typically requires concurrent
I/O — waiting for keyboard input while also polling files. This must be accomplished with
synchronous primitives: a blocking-with-timeout event read from crossterm combined with a
wall-clock tick timer.

A secondary challenge is session hierarchy. Session files have no index; hierarchy is
reconstructed by reading all sessions and grouping by their `parent_workflow` header
field. For 100 sessions this requires 100 file header reads per poll cycle. The data layer
must be efficient enough to keep the UI responsive.

The design must also integrate cleanly with the existing CLI structure: a new `Dashboard`
variant in the `Command` enum, cleanly separated from existing session management code.

## Decision Drivers

- **No async runtime** (R19): the implementation must stay fully synchronous; crossterm's
  `poll()` with a timeout is the only viable event-loop primitive
- **Single binary** (R20): the dashboard extends the existing `koto` binary; no new
  binaries or background processes
- **Startup performance** (R16, R17): ≤1s for repo-wide with 100 sessions; ≤100ms for
  focused single-session view — data reading must be fast enough to meet these
- **Reuse existing persistence layer**: `derive_machine_state` and
  `derive_last_gate_evaluated` already implement terminal detection and gate result
  computation; the dashboard must not duplicate this logic
- **Testability without PTY**: the data layer and application state layer must be testable
  without a real terminal; only the rendering layer requires integration tests
- **V2 daemon compatibility**: `koto dashboard` as a top-level command leaves room for
  `koto daemon start/stop` in V2 without namespace collision
- **Graceful degradation**: truncated JSONL files, missing compiled templates, and unknown
  event types must not crash the dashboard (R15, R10)
- **Public codebase**: design doc, code, and tests must be written for external
  contributors; no internal references

## Considered Options

*(Decision agents will populate this section)*

## Decision Outcome

*(Decision agents will populate this section)*

## Solution Architecture

*(Populated after Phase 4)*

## Implementation Approach

*(Populated after Phase 4)*

## Security Considerations

*(Populated after Phase 5)*

## Consequences

*(Populated after Phase 4)*
