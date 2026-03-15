---
status: Proposed
problem: |
  koto's state model (from issue #45) uses a minimal JSONL format with no sequence
  numbers, no typed payloads, and no header line. This is intentionally incomplete —
  #45 established the Rust skeleton, not the production schema. The upcoming CLI
  contract (#48) and auto-advancement engine (#49) depend on a full event taxonomy
  being defined and accepted first. Without it, evidence scoping, atomicity guarantees,
  and log replay semantics are all undefined.
decision: |
  Replace the simple JSONL schema with a full event log format: a header line followed
  by typed events with monotonic sequence numbers. Six event types cover all workflow
  operations. Current state is derived by replaying the log. Evidence is scoped to the
  most recent epoch — the arrival at the current state via any state-changing event.
  Sequence gaps signal corruption and halt with an error. File permissions are 0600;
  every write is followed by sync_data().
rationale: |
  The event taxonomy is the shared foundation for #47, #48, and #49 — all three depend
  on the same six event types and their semantics. Defining it here as a standalone
  design means the sub-designs can reference an accepted spec rather than a moving
  target. The key decisions (epoch boundary, gap behavior, state derivation with rewind,
  header schema) have concrete correctness implications; capturing them in a design doc
  makes them reviewable before implementation.
---

# DESIGN: Event Log Format

## Status

Proposed

## Upstream Design Reference

This is a tactical sub-design spawned from `docs/designs/DESIGN-unified-koto-next.md`
(status: Planned), which establishes the event-sourced state machine architecture.
That design defines the six event types, the JSONL state file structure, and the epoch
boundary concept. This document fills in the details, resolves ambiguities in the
upstream spec, and specifies the Rust implementation model.

Relevant sections in the upstream design: "Event Taxonomy", "State File Format", and
"Implementation Approach — Phase 1".

## Context and Problem Statement

Issue #45 (Rust CLI foundation) established a minimal JSONL event schema:

```json
{"type":"init","state":"gather","timestamp":"2026-01-01T00:00:00Z"}
{"type":"rewind","state":"gather","timestamp":"2026-01-01T00:00:00Z"}
```

This was deliberate — #45 proved the architecture without committing to the full schema.
But the upcoming issues (#47 template format, #48 unified `koto next`, #49
auto-advancement engine) all depend on a shared event taxonomy. Without it, key
questions have no answers:

- How does `koto rewind` change the current state? The current rule ("last event's
  `state` field") works for the simple schema but breaks if `rewound` events don't
  update state the same way.
- When evidence is submitted to a state and the workflow later returns to that state
  (via loop or rewind), which evidence is active?
- What happens if koto crashes mid-write? The current format gives no way to detect
  partial writes.
- What metadata does `koto workflows` expose, and where does it come from?

These questions must be settled before #47, #48, and #49 can proceed. This design
doc settles them.

## Decision Drivers

- **Shared foundation for sub-designs**: #47, #48, and #49 must reference the same
  event types and their semantics; an accepted design doc is the right artifact
- **Atomicity without full rewrites**: append-only writes are simpler than the
  temp-file-rename pattern the Go engine used; sequence numbers make partial writes
  detectable
- **Correctness of evidence scoping**: looping workflows and rewound workflows must
  not contaminate evidence across visits to the same state
- **Consistency of state derivation**: `koto rewind` writes a `rewound` event that
  changes the current state; the state derivation rule must include it
- **No migration story needed**: koto has no released users; the schema change from
  #45 is a clean break

## Considered Options

### Decision 1: How to handle sequence gaps

A sequence number gap (e.g., events go seq 1, 2, 4 with no seq 3) indicates
something went wrong during a previous write — either a partial write, a crash
before fsync, or a concurrent writer violating the single-writer assumption.

**Chosen: Halt-and-error on any gap**

The calling code receives an error (`state_file_corrupted`) and exits with code 3.
Recovery requires manual intervention: delete the state file and re-initialize.
The one exception is a malformed final line (truncated JSON), which indicates a
crash during write of the last event — this is recoverable by treating the file
as ending at the last valid seq.

**Alternatives considered**

*Warn-and-skip*: silently skip invalid or out-of-sequence lines, matching the
current `read_events` behavior. Rejected because it masks genuine corruption —
a gap may mean a valid event was lost. Any decision made using a corrupted log
(a transition that should not have been taken) becomes part of the persistent
history.

*Warn-and-truncate to last valid seq*: truncate the log at the last valid event
and proceed. Tempting, but automatic truncation could destroy a valid event that
was just out of order due to a bug in the seq assignment code. Manual recovery
is the safer default; a future `koto repair` command could implement truncation
explicitly.

---

### Decision 2: Does `rewound` change current state?

The upstream design's state derivation rule ("`to` field of the last `transitioned`
or `directed_transition` event") doesn't mention `rewound`. The `rewound` event
carries `from` and `to` fields — it is a state change. Leaving it out of the
derivation rule would mean `koto rewind` does not actually change the current state,
which is wrong.

**Chosen: Include `rewound` in the state derivation rule**

Current state = `to` field of the last event whose type is `transitioned`,
`directed_transition`, or `rewound`.

This is the only interpretation consistent with `koto rewind`'s purpose. The
upstream design's omission was an oversight; this tactical design corrects it.

---

### Decision 3: Evidence epoch boundary

The upstream design defines evidence scoping as "events occurring after the most
recent `transitioned` event whose `payload.to` matches the current state." This
defines epochs only in terms of `transitioned` events. Two cases break this rule:

- A `directed_transition` event (`koto next --to <target>`) moves to a new state.
  Is that a new epoch? The upstream rule doesn't say.
- A `rewound` event moves back to a prior state. Is the prior evidence for that
  state re-activated?

**Chosen: All state-changing events create new evidence epochs**

An epoch begins at the most recent event — of any type that changes state — whose
`to` field matches the current state. State-changing event types are:
`transitioned`, `directed_transition`, `rewound`.

This is consistent with PRD requirement R3: "when the workflow transitions out of a
state by any means, evidence is committed and the next state starts with an empty
evidence map." Rewind is a state change; arriving at a state via rewind is a fresh
arrival with no active evidence.

**Alternatives considered**

*Only `transitioned` events create epochs*: prior evidence from a rewound-to state
would re-activate on rewind. This was considered and rejected — it means an agent
that submitted stale evidence before a transition could have that evidence
re-evaluated after a rewind. The correct behavior is a clean slate on every arrival
regardless of how you arrived.

---

### Decision 4: Header line schema and `koto workflows` output

The upstream design shows a header line with four fields. The question is whether
to add more (template path, current state) and whether `koto workflows` should
return those fields.

**Chosen: Minimal four-field header; `koto workflows` returns header metadata**

Header: `schema_version`, `workflow`, `template_hash`, `created_at`. No more.

- `template_path` is a cache path (`~/.cache/koto/<hash>.json`) — it encodes local
  filesystem layout, changes between machines, and belongs only in the
  `workflow_initialized` event payload where it serves as an audit record
- `current_state` would require updating the header on every transition, breaking
  the append-only model

`koto workflows` changes from returning an array of strings to returning an array
of objects (name + header fields). This is a breaking change from #45, acceptable
since koto has no released users.

---

### Decision 5: `koto query` / `koto log` in this issue

**Chosen: Defer to post-#49**

The Go implementation had `koto query` returning a full snapshot (current state,
evidence, history, variables). The new event log provides the same information via
replay, but the shape of a query response depends on `expects` field decisions
(#47) and the unified output contract (#48). Adding `koto query` in #46 forces
those decisions prematurely. The JSONL log is directly machine-readable; agents can
inspect it without a dedicated command. This is intentional — the migration design
explicitly listed `koto query` as "Excluded | #48 or later."

## Decision Outcome

**Chosen: Full JSONL event log with typed payloads, sequence numbers, and header**

### Summary

The state file changes from simple JSONL (a flat sequence of `{type, state,
timestamp}` events) to a structured event log: a header line followed by typed
events with monotonic sequence numbers and type-specific payloads.

State derivation is a log replay: the current state is the `to` field of the last
event that changes state (`transitioned`, `directed_transition`, or `rewound`).
Current evidence is the set of `evidence_submitted` events occurring after the most
recent state-changing event that arrived at the current state. This makes per-state
evidence scoping a structural property of the log, not a policy enforced at write time.

`koto init` writes two lines: the header and a `workflow_initialized` event. `koto
rewind` appends a `rewound` event. `koto next` replays the log to derive state.
`koto workflows` reads only the header line from each state file. Writes use append
mode with mode 0600 on creation and `sync_data()` after every event.

### Rationale

The sequence number and gap detection semantics give koto strong durability
guarantees with minimal complexity — appending one JSON line is simpler than the
Go engine's full-document rewrite and provides a cleaner crash recovery story.
The epoch boundary rule derived from all state-changing events (not just
`transitioned`) is the correct generalization of the upstream design's intent; it
matches the PRD requirement and is simpler to implement (no special cases).

## Solution Architecture

### State File Format

Every state file is JSONL: one JSON object per line, append-only.

**Line 1: Header**
```json
{"schema_version":1,"workflow":"my-workflow","template_hash":"abc123...","created_at":"2026-03-15T14:30:00Z"}
```

**Lines 2+: Events**
```json
{"seq":1,"timestamp":"2026-03-15T14:30:00Z","type":"workflow_initialized","payload":{"template_path":"/home/user/.cache/koto/abc123.json","variables":{}}}
{"seq":2,"timestamp":"2026-03-15T14:30:01Z","type":"transitioned","payload":{"from":null,"to":"gather_info","condition_type":"auto"}}
{"seq":3,"timestamp":"2026-03-15T14:31:00Z","type":"evidence_submitted","payload":{"state":"gather_info","fields":{"input_file":"results.json"}}}
{"seq":4,"timestamp":"2026-03-15T14:31:01Z","type":"transitioned","payload":{"from":"gather_info","to":"analyze","condition_type":"gate"}}
```

**Header fields:**

| Field | Type | Description |
|-------|------|-------------|
| `schema_version` | integer | Format version; currently `1` |
| `workflow` | string | Workflow name; must match the state filename |
| `template_hash` | string | SHA-256 hex of the compiled template JSON |
| `created_at` | string | RFC 3339 UTC timestamp of workflow creation |

The header has no `seq` field. It is not an event. It is metadata about the log
itself, written once by `koto init` and never modified.

### Event Taxonomy

All events share a common envelope:

| Field | Type | Description |
|-------|------|-------------|
| `seq` | integer | Monotonic counter starting at 1; no gaps |
| `timestamp` | string | RFC 3339 UTC; seconds precision |
| `type` | string | One of the six event types below |
| `payload` | object | Type-specific fields (see below) |

**Six event types:**

| Type | Written by | Payload fields |
|------|-----------|---------------|
| `workflow_initialized` | `koto init` | `template_path`, `variables` (map) |
| `transitioned` | auto-advancement (future, #49) | `from`, `to`, `condition_type` |
| `evidence_submitted` | `koto next --with-data` (future, #48) | `state`, `fields` (map) |
| `directed_transition` | `koto next --to` (future, #48) | `from`, `to` |
| `integration_invoked` | auto-advancement (future, #49) | `state`, `integration`, `output` |
| `rewound` | `koto rewind` | `from`, `to` |

For this issue (#46), only `workflow_initialized` and `rewound` are implemented.
The other four types are defined here so #47, #48, and #49 can reference the
accepted taxonomy.

**State-changing event types:** `transitioned`, `directed_transition`, `rewound`.
These are the only types that affect current state derivation and epoch boundaries.

### State Derivation Rules

**Current state:**
The `to` field of the last event whose type is `transitioned`, `directed_transition`,
or `rewound`. If no such event exists (only a `workflow_initialized` event has been
written), the current state is the initial state from the `workflow_initialized`
payload — wait, see note below.

> **Note on initial state**: The current #45 implementation records the initial state
> directly in the init event's `state` field. In the new schema, `workflow_initialized`
> carries a `template_path` payload but not a `to` field. The first `transitioned`
> event (written with `from: null, to: <initial_state>`) records the initial arrival.
> To maintain backward compatibility with `koto init`'s behavior (a workflow
> immediately has a current state after init), `koto init` writes both a
> `workflow_initialized` event AND an initial `transitioned` event in the same atomic
> write (two appended lines before any fsync, or two sequential synced writes).

**Current evidence:**
The set of `evidence_submitted` events occurring after the most recent
state-changing event whose `to` field matches the current state.

Example: if the log contains `transitioned(to:gather)` at seq 2, then
`evidence_submitted(state:gather)` at seq 3, then `transitioned(to:analyze)` at seq 4,
then `rewound(to:gather)` at seq 5 — the current state is `gather`, but the most recent
state-changing event with `to:gather` is seq 5 (the rewind). Evidence is events after
seq 5. Seq 3 is archived (before the rewind). Current evidence: empty.

### Sequence Number Semantics

Seq is monotonically increasing by 1, starting at 1. The header line has no seq.

**On read (`read_events`):**
- If a non-final line fails to parse as JSON → `state_file_corrupted` error
- If seq is not exactly `prev_seq + 1` → `state_file_corrupted` error with gap location
- If the final line fails to parse → warn to stderr, return events up to last valid
  event (this is a recoverable partial write, not a gap)

**On write (`append_event`):**
- Writer reads the last event's seq from the file before each append
- New event gets `last_seq + 1`; `workflow_initialized` gets seq 1

### File Format Detection

`read_events` detects the format before parsing:

1. Read the first line
2. If the first line has a `current_state` or `CurrentState` field → old Go format;
   error with message directing user to delete and re-init
3. If the first line has a `type` field (but not `schema_version`) → #45 simple JSONL;
   error with message directing user to delete and re-init
4. If the first line has `schema_version: 1` → new format; proceed
5. If the first line is not parseable JSON → corrupted; error

### `koto workflows` Output

The command reads the header line of each `koto-*.state.jsonl` file and returns:

```json
[
  {"name":"my-workflow","created_at":"2026-03-15T14:30:00Z","template_hash":"abc123..."},
  {"name":"task-42","created_at":"2026-03-14T09:00:00Z","template_hash":"def456..."}
]
```

Files with unreadable or missing headers are skipped with a warning to stderr.
Results are sorted by name. This is a breaking change from the #45 string-array output.

### Persistence Layer Changes

**`append_event`:**
- Add `#[cfg(unix)] .mode(0o600)` on file creation
- Add `file.sync_data()` after every write
- Writer-managed seq: reads last event's seq before appending, uses `max_seq + 1`

**`read_events`:**
- Parse and validate header line (new first step)
- Validate seq monotonicity
- Halt-and-error on gaps; recover gracefully from truncated final line

**New structs:**
- `StateFileHeader` — `schema_version`, `workflow`, `template_hash`, `created_at`
- `EventPayload` — enum or typed variants for each of the six event types
- `WorkflowMetadata` — `name`, `created_at`, `template_hash` (for `koto workflows`)

**Updated `Event` struct:**
- Add `seq: u64`
- Change `state: String` to derive from payload (not a top-level field)
- Add `payload: EventPayload`

## Implementation Approach

### What changes in this issue

1. **`src/engine/types.rs`**: Add `seq` field to `Event`; add `StateFileHeader`,
   `EventPayload` types; update `MachineState` to derive from log replay using
   the corrected state derivation rule
2. **`src/engine/persistence.rs`**: Update `append_event` (mode 0600, sync_data,
   writer-managed seq); rewrite `read_events` (header parsing, gap detection,
   format detection); add `derive_state_from_log` function (revised rule including
   `rewound`); add `derive_evidence` function (epoch boundary rule)
3. **`src/discover.rs`**: Add `find_workflows_with_metadata` returning header fields;
   update existing `find_workflows` or replace with the new function
4. **`src/cli/mod.rs`**: Update `Init` to write header + `workflow_initialized` +
   initial `transitioned` event; update `Rewind` to write `rewound` event with
   `from`/`to` fields; update `Next` to derive state via log replay; update
   `Workflows` to call `find_workflows_with_metadata`
5. **`tests/integration_test.rs`**: Update tests expecting old state format or
   `workflows` output shape
6. **Unit tests** in `persistence.rs`: Rewrite `make_event` helper and raw JSONL
   fixtures to use new schema; add tests for header parsing, gap detection, epoch
   boundary rule, and rewind scenarios

### What does NOT change

- Template format (`accepts`, `when` blocks) — #47
- `koto next` output contract (`expects` field, five response variants) — #48
- Auto-advancement loop — #49
- `transitioned`, `evidence_submitted`, `directed_transition`, `integration_invoked`
  events — defined here in the taxonomy but written by #48 and #49

## Security Considerations

### State File Permissions

State files are created with mode 0600 (owner read/write only). This matches the
upstream design's requirement. State files may contain evidence submitted by agents
(API keys, analysis output, sensitive data). Mode 0600 limits access to the owning
user.

The `mode(0o600)` call is wrapped in `#[cfg(unix)]`. koto targets linux/darwin only
(per `.github/workflows/release.yml`), so no Windows conditional is needed. The call
applies only on file creation; existing files keep their permissions.

### Write Durability

`sync_data()` is called after every event write. This ensures each event is durable
on disk before the write returns. A crash between two events leaves the log ending
at the last fsynced event, which is detectable as a normal log boundary (no gap).
`sync_data()` is preferred over `sync_all()` — it flushes data without inode
metadata, which is the correct tradeoff for event durability.

### Template Hash Verification

The header's `template_hash` ties the event log to the exact compiled template it
was initialized with. `koto next` verifies that the cached template's hash matches
the header before loading the template. A modified template produces a different hash
and is rejected. This matches the approach from #45.

### Input Validation

The `fields` map in `evidence_submitted` events contains agent-submitted data. This
data is stored verbatim in the event log and should be validated against the `accepts`
schema (specified in #47) before storage. At this stage (#46), no `accepts` schema
exists — evidence submission is not yet implemented. When #48 implements
`--with-data`, it must validate against the template's `accepts` block before
appending the event.

## Consequences

### Positive

- **All sub-designs share a common vocabulary**: once this design is accepted, #47,
  #48, and #49 can reference specific event types by name
- **Evidence scoping is structural**: per-state, per-epoch evidence is a property of
  the log, not a policy enforced at write time; this eliminates an entire category
  of correctness bugs
- **Corruption is detectable**: sequence gaps make partial writes visible; `koto next`
  fails cleanly on a corrupted log rather than silently producing wrong state
- **Rewind semantics are unambiguous**: `rewound` events participate in state
  derivation and epoch boundary rules the same way as `transitioned` events — no
  special cases

### Negative

- **Breaking change from #45**: existing state files (`koto-*.state.jsonl` written
  by the #45 foundation) are no longer valid; they lack the header line and seq
  fields. Detection code rejects them with a clear error. Since koto has no released
  users, there are no external files to migrate; the integration tests are the only
  migration surface.
- **Replay cost grows with log length**: replaying 100 events on every `koto next`
  call adds latency. For short-to-medium workflows (under 50 transitions), this is
  imperceptible. For long-running workflows, a snapshot mechanism would help — but
  it's reserved in the event taxonomy for a future issue, not implemented here.
- **Fsync cost per event**: `sync_data()` on every append adds ~1-5ms per write on
  typical SSDs. For human-paced workflows, this is acceptable. Automated high-frequency
  workflows would notice it; batch-append APIs are the mitigation path if needed.

### Mitigations

- **Breaking change**: the format detection code in `read_events` rejects old files
  with a clear error message telling users to delete and re-init. No silent data loss.
- **Replay latency**: the `integration_invoked` event type reserves space in the
  taxonomy for a future snapshot event. Adding it later doesn't require a schema change.
