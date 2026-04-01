# DESIGN: Gate override mechanism

## Status

Proposed

## Context and Problem Statement

Gate evaluation is one-directional: the engine runs the gate and the result is
final. When a gate fails, an agent's only options are to wait (if the gate
condition may change) or to work around the failure by submitting an `override`
enum value via an `accepts` block. The workaround has no audit trail — the
engine can't distinguish "the agent genuinely resolved the blocking condition"
from "the agent bypassed the gate without explaining why." Session replay and
human review of a completed workflow can't reconstruct which gates were
overridden, when, or with what justification.

This is Feature 2 of the gate-transition contract roadmap. Feature 1 (merged
in #120) gives gates structured output. Feature 2 adds a first-class override
mechanism: agents call `koto overrides record` to substitute a gate's output
with default or explicit values, attaching mandatory rationale. Override events
are sticky within the current epoch and read by the advance loop during gate
evaluation, so subsequent `koto next` calls see the substituted data in the
`gates.*` evidence map. `koto overrides list` queries the override history
across the session.

## Decision Drivers

- Override rationale must be captured in the event log and queryable by
  `koto overrides list` (R6, R8). Silent gate bypasses must not be possible.
- Override defaults per gate type (built-in) and per gate instance
  (`override_default` in template) must let template authors control what
  "override" means for each gate without requiring agents to supply `--with-data`
  (R4). When `--with-data` is supplied, values are validated against the gate
  type's schema (R5).
- Each `koto overrides record` call targets one gate with one rationale (R5a).
  Multiple gates in a state each need their own `overrides record` call.
- Override events must be sticky within the current epoch — they persist until
  the state transitions — and accumulate across multiple `overrides record`
  calls in the same epoch (R5).
- The `gates` evidence key must be reserved: agents may not submit `gates.*`
  keys via `koto next --with-data`, preventing injection of fake gate data (R7).
- Rationale and `--with-data` payloads are subject to the same 1MB size limit
  as other `--with-data` payloads (R12).
- The mechanism mirrors `koto decisions record` / `koto decisions list` to
  keep the CLI surface consistent (R5).
