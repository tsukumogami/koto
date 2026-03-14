# Design Summary: unified-koto-next

## Input Context (Phase 0)
**Source PRD:** docs/prds/PRD-unified-koto-next.md
**Problem (implementation framing):** Three systems need to change together: the CLI
contract (self-describing output, expects schema, error codes), the state model (per-state
evidence scoping, structured audit trail), and the workflow definition format (per-transition
conditions, integration declarations). The design makes unifying high-level choices across
all three and spawns tactical sub-designs for each.

## Approaches Investigated (Phase 1)
- **Protocol-first**: design the JSON output contract first; state and template derive from it
- **Declarative language first**: template format is the source of truth; CLI and state derive from it
- **Minimal extension**: backward-compatible additions to the existing model; lowest migration burden
- **Event-sourced state machine**: append-only event log; per-state evidence scoping is structural

## Selected Approach (Phase 2)
Event-sourced state machine. State file becomes an append-only event log of typed, immutable
events. Per-state evidence scoping is structural (events are state-tagged, no global map to
clear). Template format adds event schema declarations. CLI `expects` field is derived from
the current state's event schema. Chosen over minimal extension because the PRD already
mandates breaking changes — the event log buys a stronger long-term model with that migration.
Chosen over protocol-first and declarative-language-first because it produces a better
protocol and template language as consequences of its structure, rather than treating either
as a design constraint.

## Investigation Findings (Phase 3)

- **event-log-schema**: Current state file is a mutable JSON snapshot. Event log replaces it
  with JSONL (header line + one event per line). Six event types cover all PRD operations.
  Current state and current evidence are both derivable from the log. Automatic migration
  on first load. Snapshots optional for performance.
- **template-event-schema**: New `accepts` block declares evidence field schemas per state.
  `when` conditions on transitions replace flat transition string list. `integration` field
  is a string tag. Format version bumps to 2. Mutual exclusivity detected at compile time
  for simple same-field conditions. Existing `gates` coexist with `accepts`/`when`.
- **cli-output-contract**: Current output has only `action`, `state`, `directive`/`message`.
  New output adds: `advanced` bool, `expects` (event_type + fields + options), `blocking_conditions`
  for gate-blocked states, `integration` field for processing integration output. Structured
  error with typed codes (gate_blocked, invalid_submission, precondition_failed, etc.).
  Exit codes 0/1/2/3 per PRD R20.

## Architecture Summary (Phase 4)

Six event types define the shared vocabulary across all three systems. State file is JSONL
(header + one event per line); current state and evidence derived by replay. Template format
v2 adds `accepts`/`when`/`integration` blocks; `gates` unchanged. CLI output gains `advanced`,
`expects`, `blocking_conditions`, `integration`, structured `error`. Auto-advancement loop
chains `transitioned` events with visited-state cycle detection; each event independently
fsynced. Four tactical sub-designs (Event Log Format, Template Format v2, CLI Output Contract,
Auto-Advancement Engine) each depends on the accepted event taxonomy.

## Security Review (Phase 5)
**Outcome:** Option 2 — Document considerations
**Summary:** No download or supply chain risks from this design. Integration invocation
(external subprocess via string tag) and evidence persistence in the event log are the
relevant dimensions. Both manageable via implementation constraints: integration names
must resolve from a closed set in config; event logs treated like secrets-containing files.

## Current Status
**Phase:** 6 - Final Review
**Last Updated:** 2026-03-14
