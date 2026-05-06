---
status: Proposed
upstream: docs/prds/PRD-session-schema-hygiene.md
problem: |
  koto's JSONL session event log is missing four fields — session UUID, sub-second
  timestamps, context_added events, and rationale on directed transitions — that
  cannot be back-filled once external consumers begin reading sessions. The changes
  touch four separate struct definitions and three CLI command paths, and one of
  them (context_added event emission) requires plumbing SessionBackend access into
  a CLI path that currently has none.
decision: |
  TBD — populated after Phase 2 decisions complete.
rationale: |
  TBD — populated after Phase 2 decisions complete.
---

# DESIGN: Session Schema Hygiene

## Status

Proposed

## Context and Problem Statement

koto's JSONL event log is written by four distinct code paths: the engine's advance
loop (state transitions), the CLI's directed-transition handler (`koto next --to`),
the rewind handler (`koto rewind`), and the context store's add operation (`koto
context add`). Adding the four fields specified in the PRD requires touching each
of these paths without breaking existing readers of logs that predate the changes.

The technical challenges break down into four areas:

**Session identifier plumbing.** `StateFileHeader` is written once at `koto init`
and rewritten during `relocate()`. UUID v4 generation must either use the `uuid`
crate (a new dependency) or be implemented inline. The generated value must be
copied unchanged through `relocate()` — a behavioral contract the code must enforce
since no existing mechanism protects the header from modification.

**Timestamp function scope.** `now_iso8601()` is a single pure function called
from a dozen sites across the engine and CLI. Changing it from whole-second to
millisecond precision is low-risk in isolation, but the change must not break any
deserialization path that has hardcoded assumptions about the 20-character string
length or whole-second format.

**Context event emission path.** `koto context add` currently writes to the
`ContextStore` without touching the session's JSONL log or `SessionBackend`. Emitting
a `context_added` event requires either introducing `SessionBackend` into the
context-add CLI path or finding a different mechanism that preserves the PRD's
ordering guarantee (event `seq` less than any subsequent `koto next` event).

**Schema versioning.** `StateFileHeader.schema_version` is currently `1`. Four
additive fields are being added. The design must decide whether this warrants a
version bump, and what the reader contract is for logs at version 1 vs. version 2.

## Decision Drivers

- **Backward compatibility is non-negotiable.** Existing JSONL logs (without any
  new fields) must parse without failure after the change. No serde `deny_unknown_fields`
  is in use; additive fields with `#[serde(default)]` are the established pattern.
- **Minimal new dependencies.** koto's `now_iso8601()` was written without the
  `chrono` crate to keep the binary lean. The same principle applies here — external
  crates for trivial operations should be avoided where the inline implementation
  is not materially more complex.
- **Single PR delivery.** All four additions must ship together per PRD R1-R4.
  Staged delivery is not an option; external readers will see all fields or none.
- **Ordering guarantee is strict.** The PRD's R3.4 ordering guarantee (`context_added`
  seq < subsequent `koto next` seq) must be mechanically enforced, not advisory.
- **Existing test infrastructure.** No new test harnesses; changes should extend
  the existing integration test suite patterns.
