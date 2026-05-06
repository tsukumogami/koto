---
status: In Progress
problem: |
  koto's JSONL session event log is missing four fields that cannot be added once
  external consumers adopt the schema. The event log is append-only: past events
  are never rewritten. If these fields are absent from the first public schema
  version, historical sessions will be permanently incomplete — no tooling or
  migration can reconstruct what was never recorded.
goals: |
  Lock in the complete set of schema fields that must ship before koto's event log
  format gains external consumers. Define each field's name, type, required/optional
  status, default behavior, and the structural guarantee that makes back-filling
  impossible after the fact.
---

# PRD: Session Schema Hygiene

## Status

In Progress

## Problem statement

koto records AI agent workflow sessions as JSONL event logs. Each line is an immutable
event appended at the moment it occurs. Once an external tool reads and stores these
logs — a dashboard, a relay, an analysis pipeline — the format is effectively frozen:
removing or renaming fields breaks readers, and adding fields that should have been
present from the start cannot be retroactively applied to events that have already been
read and stored downstream.

Four fields are in this category. None is present in today's schema. For each, the
structural reason it cannot be back-filled is specific: the session header is written
once and the data is gone after that write; timestamps record wall-clock instants
that cannot be reconstructed from lower-precision records; context additions are
tracked in a mutable sidecar with no log counterpart; agent decision rationale exists
only in the agent's active context, never in a recoverable artifact.

A PRD is the right artifact here because the question is what these fields must specify
— not how they are implemented. Implementation is straightforward in each case; the
risk is specifying the wrong contract and living with it permanently.

## Goals

- Every new session carries a universally unique identifier that survives rename operations
- All event timestamps distinguish concurrent sessions within a one-second window
- Context artifact additions appear in the event log with enough metadata to reconstruct
  what the agent knew at any transition
- Directed transitions and rewinds can optionally carry agent rationale, auditable
  after the fact
- Readers of logs written before this schema version can process them without failure
  (additive changes only; no existing fields renamed or removed)

## User stories

**As a developer building a koto session reader**, I want each session to carry a stable
unique identifier, so that I can track a session across renames, cleanups, and
re-ingestions without collision.

**As a developer correlating concurrent child sessions**, I want timestamps precise
enough to order events from parent and child sessions that overlap within a second, so
that I can reconstruct the exact sequence of events across a session hierarchy.

**As a developer auditing what an agent knew at a decision point**, I want context
artifact additions to appear in the event log, so that I can determine which context
was available before each state transition without reading a separate sidecar file.

**As a developer reviewing agent decisions**, I want directed transitions and rewinds to
optionally record why the agent made that choice, so that I can distinguish deliberate
overrides from accidental or unexplained state changes.

**As a koto maintainer**, I want these fields specified before the log format gains
external consumers, so that I'm not maintaining a compatibility layer for an
incompletely specified v1 schema.

## Requirements

### R1: Session identifier

**R1.1.** Every new session header must include a `session_id` field.

**R1.2.** `session_id` must be a UUID v4 string in lowercase hyphenated format
(`xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx`). No other UUID version is acceptable.

**R1.3.** `session_id` is generated at `koto init` time. The generating process must
use a cryptographically random source.

**R1.4.** `session_id` is immutable. When a session is renamed (e.g., the workflow
name changes), the header is rewritten with the original `session_id` value unchanged.
The `session_id` identifies the session, not its name.

**R1.5.** `session_id` is required on `StateFileHeader`. Readers that encounter a
header without `session_id` must treat it as a pre-schema-hygiene session and must
not fail. They may generate a synthetic local identifier for their own bookkeeping,
but must not write a `session_id` back to the log.

### R2: Sub-second timestamp precision

**R2.1.** All `timestamp` fields in the event log must use RFC 3339 UTC format with
millisecond precision: `YYYY-MM-DDTHH:MM:SS.mmmZ`.

**R2.2.** This applies to every field named `timestamp` across all event types and the
`StateFileHeader.created_at` field.

**R2.3.** Readers must tolerate timestamps in both the legacy whole-second format
(`YYYY-MM-DDTHH:MM:SSZ`) and the millisecond format. Readers must not fail or coerce
either format; they must parse both as valid RFC 3339.

**R2.4.** Timestamps are recorded at event creation time from the system clock. The
precision is best-effort — no synchronization guarantee across machines is implied.

### R3: Context-added event

**R3.1.** When a context artifact is added to a session, a `context_added` event must
be appended to the session's JSONL log.

**R3.2.** The `context_added` event payload carries exactly three fields:

| Field | Type | Description |
|-------|------|-------------|
| `key` | `String` | The context key under which the artifact is stored (hierarchical path, e.g., `scope.md`, `research/r1/lead-foo.md`) |
| `hash` | `String` | SHA-256 hex digest of the artifact content at the time of the add |
| `size` | `u64` | Byte length of the artifact content at the time of the add |

**R3.3.** The `context_added` event is emitted synchronously: it is appended to the
JSONL log during the same operation that writes the artifact to the context store,
before the operation returns to the caller.

**R3.4.** Ordering guarantee: the `context_added` event's sequence number (`seq`) is
less than the sequence number of any event appended by a subsequent `koto next`
invocation. A reader can determine which context was available at transition `T` by
collecting all `context_added` events with `seq` less than `T.seq`.

**R3.5.** If the context add fails before the artifact is durably written, no
`context_added` event is appended. If the artifact is durably written but the event
append fails, implementations must not emit a partial event; they must surface the
error to the caller.

**R3.6.** A `context_added` event is only emitted on add. Subsequent `koto context add`
calls that replace an existing key emit a new `context_added` event with the updated
`hash` and `size`. There is no `context_removed` event in this schema version.

### R4: Rationale on directed transitions and rewinds

**R4.1.** The `directed_transition` event payload may include an optional `rationale`
field.

**R4.2.** The `rewound` event payload may include an optional `rationale` field.

**R4.3.** `rationale` is a free-text `String`. No structure, no length limit.

**R4.4.** When `rationale` is absent, the field must be omitted from the serialized
JSON entirely — not serialized as `null`.

**R4.5.** Readers must tolerate both the presence and absence of `rationale`. A reader
that encounters `directed_transition` or `rewound` without `rationale` must not fail.

**R4.6.** The CLI commands that produce these events must accept `rationale` as an
optional input. When not provided, the field is omitted from the emitted event.

## Non-back-fillable justification

This section records why each addition cannot be retroactively applied to existing
sessions once external consumers have adopted the log format.

### Session identifier

The session header is the first line of the JSONL log file. It is written once at
session initialization. A downstream consumer that has already ingested a header
without `session_id` has no subsequent event to read it from — the header has no
sequence number and is not rebroadcast. Adding `session_id` to the header of a session
that already has external consumers means those consumers would see a new field on
the initial read of a new session but would have no way to associate it with any prior
read of the same session's header. The identifier's uniqueness property — its whole
value — is that every session carries it from the first byte written. A session
without it is permanently unidentifiable by UUID.

### Timestamp precision

Timestamps are recorded from the system clock at the moment an event is written. A
whole-second timestamp `2026-05-06T14:30:00Z` does not encode which millisecond
within that second the event occurred. There is no source from which to reconstruct
the sub-second offset for events already written. Any precision added to a historical
timestamp would be fabricated.

### Context-added event

The context store is a mutable sidecar: `koto context remove` deletes artifacts;
`koto context add` overwrites existing keys. The sidecar's current state is a snapshot
of the most recent version of each key, not a history. Once external readers have
adopted the event log as their authoritative record, they cannot reach back to the
sidecar to reconstruct the history of context changes — the history was never recorded
there. Any `context_added` event that should have appeared between two already-read
events cannot be inserted into the log because the log is append-only.

### Rationale on directed transitions and rewinds

Rationale for a directed transition or rewind reflects the agent's reasoning at the
moment it issued the command. That reasoning exists in the agent's active context —
not in any persistent artifact. Once the CLI call completes, the agent moves on. There
is no record to reconstruct the rationale from after the fact. An annotation added
later would be indistinguishable from a fabrication.

## Acceptance criteria

- [ ] `StateFileHeader` includes `session_id: String` (required, UUID v4 lowercase hyphenated)
- [ ] New sessions always emit a `session_id` in the header
- [ ] Session rename operations copy `session_id` unchanged to the rewritten header
- [ ] Readers that encounter a header without `session_id` do not fail
- [ ] All `timestamp` fields (events and header `created_at`) use millisecond RFC 3339 format
- [ ] Readers accept both whole-second and millisecond RFC 3339 timestamp strings
- [ ] `EventPayload` includes a `ContextAdded` variant with fields `key: String`, `hash: String`, `size: u64`
- [ ] `context_added` is emitted synchronously by `koto context add` after the artifact is written
- [ ] `context_added.seq` is less than the `seq` of any event appended by the subsequent `koto next`
- [ ] `DirectedTransition` payload includes `rationale: Option<String>` omitted when `None`
- [ ] `Rewound` payload includes `rationale: Option<String>` omitted when `None`
- [ ] `koto next --to <state> --rationale <text>` passes rationale to `DirectedTransition`
- [ ] `koto rewind <state> --rationale <text>` passes rationale to `Rewound`
- [ ] Existing JSONL logs (without any of these fields) are readable without failure after the change
