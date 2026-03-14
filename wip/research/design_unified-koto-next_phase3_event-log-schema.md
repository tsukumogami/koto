# Phase 3 Research: Event Log Schema and State File Design

## Questions Investigated

1. What does the current `State` struct look like?
2. What does a `HistoryEntry` look like today?
3. What is currently in the state file that must be preserved in the event log?
4. What event types are needed to cover all PRD-required operations?
5. What should the event log file format look like?
6. How should format/schema version be expressed?
7. Atomicity and durability: append vs. temp-file-rename?
8. Performance impact of full replay for long workflows?
9. Migration path from current format to event log?

## Findings

### Current `State` struct (`pkg/engine/types.go:9-17`)

Fields: `SchemaVersion`, `Workflow` (metadata), `Version` (counter), `CurrentState` string,
`Variables map[string]string`, `Evidence map[string]string`, `History []HistoryEntry`.

The state file is a single JSON object written atomically via temp-file-then-rename
(`engine.go:470-511`). Every `Transition()` call overwrites the entire file.

### Current `HistoryEntry` (`pkg/engine/types.go:28-34`)

Fields: `From string`, `To string`, `Timestamp`, `Type string` ("transition" or "rewind"),
`Evidence map[string]string` (optional snapshot of evidence at transition time).

History is an append-only slice within the mutable state document. It captures from/to
states and archived evidence, but not the full event payload or a sequence number.

### What must be preserved in the event log

- Workflow identity (name, template hash)
- Complete sequence of transitions (from/to, timestamp)
- Evidence submitted per state (currently in `State.Evidence`, archived to `HistoryEntry.Evidence`)
- Rewind events (currently `HistoryEntry.Type = "rewind"`)
- Variables (currently in `State.Variables` — static, set at init)

Current state and current evidence are both derivable from the log: current state is the
`to` field of the last `transitioned` event; current evidence is the union of
`evidence_submitted` events whose `state` field matches the current state.

### Event types needed (from PRD operations)

| Event type | Triggered by | Payload |
|-----------|-------------|---------|
| `workflow_initialized` | `koto init` | workflow name, template hash, variables |
| `transitioned` | auto-advancement | from, to, condition_type ("gate"/"auto") |
| `evidence_submitted` | `--with-data` | state, fields map |
| `directed_transition` | `--to` | from, to, directed:true |
| `integration_invoked` | processing integration | state, integration name, output |
| `rewound` | `koto rewind` | from, to, reason |

All events share: `seq int`, `timestamp RFC3339`, `type string`.

### File format: JSONL vs. JSON array

**JSONL** (one JSON object per line): append is a simple write + newline; no need to
rewrite existing content; corrupt last line is detectable (incomplete JSON); standard
tooling (jq, grep) works line-by-line. Drawback: harder to read as a whole document.

**JSON array**: human-readable as a complete document; standard JSON parsers handle it;
but appending requires rewriting the closing `]` — not a true append, requires read-modify-write
or a separate in-progress marker.

**Recommendation**: JSONL with a header line. First line is a JSON header object
(`schema_version`, `workflow`, `template_hash`, `created_at`). Subsequent lines are events.
This keeps the format append-friendly while retaining a structured header for version detection.

### Format/schema versioning

Schema version belongs in the header line, not per-event. Version detection on load reads
only the first line. Current state files have `schema_version` at the top level — the same
field name can be used in the JSONL header for a clean migration signal.

### Atomicity: append vs. temp-file-rename

The current temp-file-rename pattern provides atomicity for full-document overwrites.
For event appends, `write + fsync` gives the same guarantee: either the full event bytes
are in the file or they're not (detectable by checking the last line is valid JSON).
A sequence number gap (last event seq N, next event seq N+2) signals a partial write.

For multi-state auto-advancement chains, each event is independently fsynced — consistent
with PRD R2 (each transition independently committed; crash mid-chain recovers from last
committed state).

### Performance: replay for long workflows

Full replay on every `koto next` call is O(n) in the number of events. For typical agent
workflows (tens of transitions), this is negligible. For long-running workflows (hundreds
of transitions), replay latency grows.

**Snapshot mechanism**: periodically write a `snapshot` event containing the derived
current state. On load, find the last `snapshot` event and replay only from there. The
snapshot event contains: `current_state`, `variables`, and the current state's accumulated
evidence. Snapshots are optional and not required for correctness — they're a performance
optimization. An initial implementation can skip snapshots and add them if workflows grow
long enough to need them.

### Migration path

Two-phase approach:
1. **Format detection on load**: if the state file is a JSON object with `CurrentState`
   field (old format), read it using the old parser and synthesize a minimal event log
   in memory (one `workflow_initialized` event + one `transitioned` event per history entry).
2. **Write in new format**: after loading the old format, re-persist in JSONL event log
   format. On next load, the new format is used directly.

This means the migration happens automatically on the first `koto next` call after upgrading.
No explicit migration tool required for in-flight workflows; the engine handles it.

## Implications for Design

- **JSONL header + events** is the file format: first line is header, remaining lines are events
- **Six event types** cover all PRD operations; the event taxonomy should be finalized in the
  tactical sub-design before the other sub-designs begin (template declarations and CLI output
  both reference event type names)
- **Snapshots are optional** in the initial implementation; design should leave room for them
  without requiring them
- **Automatic migration** on first load is feasible; no separate migration tool needed
- **Sequence numbers** on every event provide both ordering guarantees and partial-write detection

## Surprises

1. The current `HistoryEntry` already captures from/to and archived evidence — the event log
   is largely a restructuring of what already exists, not net-new data
2. The `Variables` map is set at `Init()` and never mutated after that; it can live in the
   header line rather than being repeated in every event
3. `koto rewind` already exists as a feature — the event log must support a `rewound` event
   type from day one, not treat rewind as an edge case

## Summary

The current state file is a mutable JSON snapshot with evidence, history, and current state
as top-level fields. The event log restructures this as JSONL: a header line with workflow
metadata and schema version, followed by one line per event. Current state and current
evidence are derived from the log rather than stored directly. Migration is automatic on
first load — the engine detects old format and re-persists in the new format. Six event
types cover all PRD-required operations; the event taxonomy must be finalized first since
both template declarations and CLI output reference event type names.
