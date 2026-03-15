# Explore Scope: event-log-format

## Core Question

Issue #46 needs a design doc (`docs/designs/DESIGN-event-log-format.md`) and Rust
implementation that upgrades koto's simple JSONL state schema from #45 to the full
event-sourced format specified in `DESIGN-unified-koto-next.md`. This is the
foundational tactical design: #47, #48, and #49 all depend on the event taxonomy
being accepted here. The scope also includes identifying any Go functionality lost
in the Rust conversion that belongs in this issue.

## Context

- Upstream design: `docs/designs/DESIGN-unified-koto-next.md` (status: Planned)
  defines 6 event types, the JSONL header line, the epoch boundary rule for
  evidence scoping, and atomicity via sequence numbers.
- Current Rust implementation (from #45): simple schema — `{type, state, timestamp,
  template?, template_hash?}`. No header line. No sequence numbers. No typed payloads.
- Go commands lost in Rust conversion relevant to this issue:
  - `koto query` — returned full JSON snapshot (current state, history, evidence,
    variables). With #46 implementing log replay, this may naturally re-emerge here.
  - `koto status` — human-readable status. Less relevant.
  - `koto cancel` — not related to event log format.

## In Scope

- Full event type taxonomy (6 types: `workflow_initialized`, `transitioned`,
  `evidence_submitted`, `directed_transition`, `integration_invoked`, `rewound`)
- Common envelope fields (`seq`, `timestamp`, `type`) and per-event payloads
- State file header line format (`schema_version`, `workflow`, `template_hash`,
  `created_at`)
- Rules for deriving current state from log replay
- Epoch boundary rule for evidence scoping (correctness with looping workflows)
- Sequence number gap detection semantics (atomicity)
- JSONL vs JSON-array trade-off evaluation
- Old format detection and rejection (Go mutable-JSON state files)
- File permission model (mode 0600)
- `koto init` updated to write `workflow_initialized` event
- `koto rewind` updated to write `rewound` event with `from`/`to`
- `koto next` updated to derive state via full log replay
- `koto workflows` updated to read header line for workflow metadata
- Whether `koto query` (log inspection command) belongs in this issue
- Integration tests for all log replay scenarios

## Out of Scope

- Template format (`accepts`/`when` blocks) — that's #47
- `koto next` full output contract (`expects` field) — that's #48
- Auto-advancement engine — that's #49
- `koto cancel` — not related to event log format
- Snapshot/compaction for long logs — reserved in taxonomy but not implemented here

## Research Leads

1. **Does `koto query` (or equivalent inspection command) belong in #46?**
   The Go version exposed full workflow state as JSON via `koto query`. With event
   log replay as the core mechanism in #46, adding a `koto log` or `koto query`
   command naturally piggybacks. Or is this deferred to a later issue?

2. **What are the exact sequence number gap detection semantics?**
   The upstream design says a seq gap detects partial writes, but doesn't specify
   behavior: halt-and-error, warn-and-truncate, or warn-and-skip? The design doc
   must make this unambiguous so implementations are consistent.

3. **How does the epoch boundary rule interact with `koto rewind`?**
   After a rewind, the current state may be one the workflow was in before. Does
   a new evidence epoch begin (fresh start), or does prior evidence for that state
   re-activate? The correctness of looping workflows depends on this.

4. **What's the complete header line schema and what does `koto workflows` need?**
   The issue says `koto workflows` should read the header line for metadata. The
   upstream design's header example has `schema_version`, `workflow`, `template_hash`,
   `created_at` — but no `template_path`. Is `template_path` needed in the header,
   or only in the `workflow_initialized` event payload?

5. **What's the file permission and atomicity implementation model in Rust?**
   The upstream design specifies mode 0600 and append-then-fsync. The current
   `OpenOptions::new().create(true).append(true)` doesn't set permissions. What's
   the correct Rust idiom, and what does fsync after each append cost in practice?

6. **How does old format detection work and what's the migration story for tests?**
   Old state files (Go era) have a top-level `CurrentState` field. New JSONL files
   have a header with `schema_version`. Integration tests used the old simple format
   from #45 — they need to migrate to the new schema or be replaced.
