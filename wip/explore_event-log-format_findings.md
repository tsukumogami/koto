# Explore Findings: event-log-format

## Round 1 Research Summary

Six leads investigated. All decisions made. Ready to crystallize.

---

### Lead 1: koto query command scope
**File**: `wip/research/explore_event-log-format_r1_lead-query-command.md`

**Decision: Defer `koto query` — not in #46.**

The Go `koto query` was intentionally excluded from the Rust migration (listed as
"Excluded | #48 or later" in DESIGN-migrate-koto-go-to-rust.md). No agent skill or
integration test requires it. The output shape depends on `expects` schema decisions
from #47 and #48. The JSONL log is directly readable for debugging. Adding it in #46
is premature and creates coordination overhead.

---

### Lead 2: Sequence gap detection semantics
**File**: `wip/research/explore_event-log-format_r1_lead-seq-gap-semantics.md`

**Decision: Halt-and-error on any sequence gap.**

Industry standard (SQLite WAL, PostgreSQL WAL, EventStore) never skips gaps — they
truncate or error. For koto, the correct behavior is:
- Any non-final malformed line → immediate error (`state_file_corrupted`)
- A gap in seq numbers → immediate error
- Malformed final line only (partial write before crash) → warn and return events up
  to last valid seq (this is distinct from a gap)
- First event has `seq=1`; writer manages seq assignment (reads last event's seq,
  appends `max_seq + 1`)

Error code: `state_file_corrupted` with message naming the gap location.
Recovery: manual (delete and re-init), not automatic.

---

### Lead 3: Epoch boundary rule and rewind interaction
**File**: `wip/research/explore_event-log-format_r1_lead-epoch-rewind.md`

**Decision: Rewind creates a new evidence epoch; all state-changing events create epochs.**

The upstream design has two ambiguities that #46's design doc must resolve:

1. **State derivation**: the upstream rule "current state = `to` field of last
   `transitioned` or `directed_transition` event" is incomplete — `rewound` events
   also change state. **Corrected rule**: current state = `to` field of the last
   event of type `transitioned`, `directed_transition`, OR `rewound`.

2. **Epoch boundary**: the upstream rule defines epochs only in terms of `transitioned`
   events. **Corrected rule**: an epoch begins at the most recent event of type
   `transitioned`, `directed_transition`, or `rewound` whose `to` field matches the
   current state. Evidence submitted after that event is active; evidence from prior
   visits is archived.

This means a rewind to state X starts a fresh evidence epoch for X — evidence from
before the rewind is archived in the log but not active. This is correct per PRD R3:
"when the workflow transitions out of a state by any means, evidence is committed and
the next state starts with an empty evidence map." Rewind is a state change; rewind-to
is a fresh arrival.

---

### Lead 4: Header line schema and koto workflows
**File**: `wip/research/explore_event-log-format_r1_lead-header-schema.md`

**Decision: Minimal header, koto workflows returns metadata objects.**

**Header schema (first line of every state file):**
```json
{"schema_version":1,"workflow":"my-workflow","template_hash":"abc123...","created_at":"2026-03-15T14:30:00Z"}
```
All four fields required. No `template_path` in header (it's transient cache path;
lives only in `workflow_initialized` event payload). No `current_state` in header
(derived from log replay; caching it would require updating the header on every
transition, breaking the append-only model).

**koto workflows output change (Option B from research):**
```json
[
  {"name":"my-workflow","created_at":"2026-03-15T14:30:00Z","template_hash":"abc123..."},
  {"name":"task-42","created_at":"2026-03-14T09:00:00Z","template_hash":"def456..."}
]
```
Returns header fields only — no `current_state` (avoids O(file) replay per workflow
in the discovery command). Files with unreadable/missing headers are skipped with a
stderr warning. Breaking change from current string-array output, which is acceptable
(koto has no released users).

---

### Lead 5: File permissions and fsync
**File**: `wip/research/explore_event-log-format_r1_lead-permissions-fsync.md`

**Decision: mode 0600 on creation, sync_data() after every write, writer-managed seq.**

- Use `std::os::unix::fs::OpenOptionsExt::mode(0o600)` with `#[cfg(unix)]` guard.
  koto only ships linux/darwin, so no Windows conditional needed.
- Use `File::sync_data()` (not `sync_all()`) after each `writeln!` — data durability
  is what matters, not inode metadata.
- Fsync every event without exception — selective fsync breaks gap detection semantics.
- Seq assignment is writer-managed: `append_event` reads the last event's seq from the
  file and uses `max_seq + 1`. First event: seq = 1. Header line has no `seq` field
  (intentional; it's not an event).

---

### Lead 6: Old format detection and test migration
**File**: `wip/research/explore_event-log-format_r1_lead-format-detection.md`

**Decision: Three-tier detection in read_events; integration tests self-migrate.**

Three formats to distinguish:
1. **Old Go format**: first line is a JSON object with `current_state` field → error
   with "delete and re-init" message
2. **#45 simple JSONL**: first line is an event with `type` field but no header
   → error with "legacy format" message
3. **#46 new format**: first line has `schema_version` field → parse normally

**Test migration scope:**
- Integration tests (`tests/integration_test.rs`, 15 tests): self-migrate — they use
  the CLI, not direct persistence calls
- Unit tests (`src/engine/persistence.rs`, 8 tests): need rewriting — `make_event`
  helper and raw JSONL strings need `seq` fields and typed payloads
- The `read_events_skips_malformed_lines` test needs a full rewrite to use the new
  header+seq format

---

## Key Decisions Made

| Decision | Choice |
|----------|--------|
| `koto query` in #46? | No — defer to post-#49 |
| Gap detection behavior | Halt-and-error; only truncated final line is recoverable |
| Does `rewound` change current state? | Yes — include in state derivation rule |
| Does `rewound` create evidence epoch? | Yes — all state changes reset evidence |
| Header fields | `schema_version`, `workflow`, `template_hash`, `created_at` |
| `koto workflows` output | Array of objects (name + header fields) — breaking change |
| File permissions | mode 0600 on creation via OpenOptionsExt |
| fsync strategy | `sync_data()` after every event |
| Seq assignment | Writer-managed (reads last seq, appends max+1) |
| Format detection | Three-tier; #45 and Go formats rejected with clear errors |

## Design Gaps in Upstream Design

The upstream `DESIGN-unified-koto-next.md` has three ambiguities that #46's design
doc must resolve:

1. **State derivation rule missing `rewound`**: line 238 says "last `transitioned` or
   `directed_transition` event" — must include `rewound`
2. **Epoch boundary missing `directed_transition` and `rewound`**: epoch rule defined
   only for `transitioned` events — must include all three state-changing event types
3. **Seq gap behavior unspecified**: design says gaps detect partial writes but gives
   no behavior spec — must specify halt-and-error

## Decision: Crystallize

The artifact type is **design document** (`docs/designs/DESIGN-event-log-format.md`).
All open questions are resolved. Proceeding to produce the design doc.
