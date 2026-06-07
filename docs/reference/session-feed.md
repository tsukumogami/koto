---
schema_version: 1

header:
  fields:
    schema_version:
      type: integer
      required: true
    workflow:
      type: string
      required: true
    template_hash:
      type: string
      required: true
    created_at:
      type: string
      required: true
      format: rfc3339
    session_id:
      type: string
      required: false
    parent_workflow:
      type: string
      required: false
      nullable: true
    template_source_dir:
      type: string
      required: false
      nullable: true

events:
  workflow_initialized:
    tier: 1
    fields:
      template_path:
        type: string
        required: true
      variables:
        type: object
        required: false
      spawn_entry:
        type: object
        required: false
        nullable: true

  transitioned:
    tier: 1
    fields:
      from:
        type: string
        required: true
        nullable: true
      to:
        type: string
        required: true
      condition_type:
        type: string
        required: true
        enum: ["auto", "gate", "skip_if"]
      skip_if_matched:
        type: object
        required: false
        nullable: true

  directed_transition:
    tier: 1
    fields:
      from:
        type: string
        required: true
      to:
        type: string
        required: true
      rationale:
        type: string
        required: false
        nullable: true

  rewound:
    tier: 1
    fields:
      from:
        type: string
        required: true
      to:
        type: string
        required: true
      rationale:
        type: string
        required: false
        nullable: true

  evidence_submitted:
    tier: 1
    fields:
      state:
        type: string
        required: true
      fields:
        type: object
        required: true
      submitter_cwd:
        type: string
        required: false
        nullable: true

  workflow_cancelled:
    tier: 1
    fields:
      state:
        type: string
        required: true
      reason:
        type: string
        required: true

  gate_override_recorded:
    tier: 1
    fields:
      state:
        type: string
        required: true
      gate:
        type: string
        required: true
      rationale:
        type: string
        required: true
      override_applied:
        type: object
        required: true
      actual_output:
        type: object
        required: true
      timestamp:
        type: string
        required: true
        format: rfc3339

  batch_finalized:
    tier: 1
    fields:
      state:
        type: string
        required: true
      view:
        type: object
        required: true
      timestamp:
        type: string
        required: true
        format: rfc3339

  integration_invoked:
    tier: 2
    fields:
      state:
        type: string
        required: true
      integration:
        type: string
        required: true
      output:
        type: any
        required: true

  context_added:
    tier: 2
    fields:
      key:
        type: string
        required: true
      hash:
        type: string
        required: true
      size:
        type: integer
        required: true

  default_action_executed:
    tier: 2
    fields:
      state:
        type: string
        required: true
      command:
        type: string
        required: true
      exit_code:
        type: integer
        required: true
      stdout:
        type: string
        required: true
      stderr:
        type: string
        required: true

  decision_recorded:
    tier: 2
    fields:
      state:
        type: string
        required: true
      decision:
        type: any
        required: true

  gate_evaluated:
    tier: 2
    fields:
      state:
        type: string
        required: true
      gate:
        type: string
        required: true
      output:
        type: object
        required: true
      outcome:
        type: string
        required: true
        enum: ["passed", "failed"]
      timestamp:
        type: string
        required: true
        format: rfc3339

  child_completed:
    tier: 2
    fields:
      child_name:
        type: string
        required: true
      task_name:
        type: string
        required: true
      outcome:
        type: string
        required: true
        enum: ["success", "failure", "skipped"]
      final_state:
        type: string
        required: true

  scheduler_ran:
    tier: 3
    fields:
      state:
        type: string
        required: true
      tick_summary:
        type: object
        required: true
      timestamp:
        type: string
        required: true
        format: rfc3339
---

# Session-Feed Data Contract

koto records workflow sessions as append-only JSONL files. Each file represents one
session from birth to termination. This document is the authoritative contract for
consumers building against the session feed.

## File Structure

A session log is a JSONL file with two record types:

- **Line 1**: The header record (`StateFileHeader`). No `seq` field.
- **Lines 2+**: Event records, one per line, in strict monotonic `seq` order starting at 1.

Files are written with append semantics and `sync_data()` after each write. Readers
may encounter an incomplete final line due to a crash mid-write; see Partial-Write
Recovery below.

## Header Record

The header is a flat JSON object on line 1. It carries workflow metadata and the schema
version signal.

```json
{
  "schema_version": 1,
  "workflow": "issue_42",
  "template_hash": "a3f5b2c1deadbeef",
  "created_at": "2026-05-07T10:00:00.000Z",
  "session_id": "550e8400-e29b-41d4-a716-446655440000",
  "parent_workflow": null,
  "template_source_dir": "/home/user/.claude/plugins/cache/shirabe/skills/work-on"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `schema_version` | integer | Yes | Format version. Currently `1`. Readers MUST reject files where this exceeds their supported maximum. |
| `workflow` | string | Yes | Workflow name. Matches the session filename prefix. |
| `template_hash` | string | Yes | SHA-256 hex digest of the compiled template JSON at init time. |
| `created_at` | string | Yes | RFC 3339 UTC timestamp of session creation. |
| `session_id` | string | No | UUID v4 generated at `koto init` time. Absent (empty string) in files written before this field existed. |
| `parent_workflow` | string | No | Name of the parent workflow for batch-spawned children. Absent for top-level sessions. |
| `template_source_dir` | string | No | Absolute path to the directory containing the source template at init time. Absent for stdin/inline templates and older files. |

## Event Envelope

Every event record shares a common outer envelope. The payload fields vary by type.

```json
{
  "seq": 1,
  "timestamp": "2026-05-07T10:00:01.234Z",
  "type": "workflow_initialized",
  "payload": { ... }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `seq` | integer | Yes | Monotonic sequence number starting at 1. Each event increments by exactly 1. |
| `timestamp` | string | Yes | RFC 3339 UTC timestamp with millisecond precision. |
| `type` | string | Yes | Event type string (see event catalogue below). |
| `payload` | object | Yes | Type-specific payload fields (see event catalogue). |

## Reader Guarantees

**Ordering**: Events are strictly monotonic on `seq`. A gap in `seq` indicates file
corruption. Readers MUST treat a non-final seq gap as a `StateFileCorrupted` error.

**Durability**: Each event is flushed with `sync_data()` before the next event is
written. A crash between writes leaves at most one incomplete final line.

**Atomicity**: Each event write is atomic at the line level. A line is either complete
and valid JSON, or truncated (partial write). There are no partial-JSON events in
mid-file positions.

**Partial-write recovery**: A truncated final line (incomplete JSON) is recoverable.
Readers SHOULD discard the truncated line and treat all preceding complete events as
valid. koto's own `read_events` does this with a stderr warning.

**Single-writer**: Only the koto process that owns a session writes to its log. No
external process should append to a session log while koto holds it.

**Old format compatibility**: Files with `schema_version: 1` produced by older koto
binaries remain valid. Additive optional fields added to the header or event payloads
in minor koto releases use `serde(default)` semantics — readers MUST tolerate absent
optional fields without error.

**Unknown-version handling**: Readers MUST reject files where `header.schema_version`
exceeds their known maximum with a clear error. The error format is:
`"incompatible schema version: found {N}, max supported {M}"`.

**Unknown event types**: Readers MUST NOT hard-fail on unrecognized `type` strings.
The correct behavior is to skip the event and continue. koto's own `read_events`
implements this via `EventPayload::Unknown`.

**Terminal state detection**: There is no `workflow_completed` event. Consumers
determine whether a session has reached a terminal state by inspecting the most recent
`transitioned` event and checking whether its `to` field matches the template's defined
terminal states. This is a known gap in the current contract.

## Forward-Compatibility Rules

| Change type | Schema version bump? | Consumer impact |
|-------------|---------------------|-----------------|
| New optional field on existing event | No | Readers tolerant of unknown fields see no impact. |
| New required field on existing event | No (new type name instead) | New event type name used (e.g., `transitioned_v2`). Old type unchanged. |
| Field removed from existing event | No (new type name instead) | New type name guarantees old parsers are not silently broken. |
| New event type added | No | Readers MUST skip unknown type strings; no version bump needed. |
| Unknown type string encountered | — | Skip the event. Do not error. |
| Unknown field within known payload | — | Ignore the field. Do not error. |
| `schema_version` > reader's maximum | Yes | Reject the file with `IncompatibleSchemaVersion`. |

## Event Catalogue

### Tier 1: Required Display

Consumers MUST surface Tier 1 events in any user-visible session view. Omitting a
Tier 1 event produces an incomplete or misleading representation of session progress.

---

#### `workflow_initialized`

Marks the birth of a session. Written once at `koto init` time.

```json
{
  "type": "workflow_initialized",
  "payload": {
    "template_path": "/home/user/.koto/cache/compiled/abc123.json",
    "variables": {"ISSUE_NUMBER": "42"},
    "spawn_entry": null
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `template_path` | string | Yes | Path to the compiled template JSON in koto's cache directory. |
| `variables` | object | No | Variable bindings active at init time. String-to-string map. Absent when no variables were set. |
| `spawn_entry` | object | No | Present only for batch-spawned child sessions. Carries `template` (source path), `vars` (bindings), and `waits_on` (sorted dependency list). Absent for top-level sessions. |

---

#### `transitioned`

Records every automatic or evidence-driven state change. The primary workflow progress event.

```json
{
  "type": "transitioned",
  "payload": {
    "from": "review",
    "to": "complete",
    "condition_type": "gate",
    "skip_if_matched": null
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `from` | string or null | Yes | Source state name. `null` for the initial transition from no prior state. |
| `to` | string | Yes | Destination state name. |
| `condition_type` | string | Yes | Transition trigger: `"auto"`, `"gate"`, or `"skip_if"`. |
| `skip_if_matched` | object | No | Present when `condition_type` is `"skip_if"`. Carries the key-value pairs from the `skip_if` map that triggered the transition. |

---

#### `directed_transition`

Records an explicit state override issued via `koto next --to <state>`.

```json
{
  "type": "directed_transition",
  "payload": {
    "from": "implement",
    "to": "review",
    "rationale": "skipping gate: CI is known-broken"
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `from` | string | Yes | Source state name. Always non-null (unlike `transitioned.from`). |
| `to` | string | Yes | Destination state name. |
| `rationale` | string | No | Human-readable reason. Absent when `--rationale` was not provided. |

---

#### `rewound`

Records a rollback to a prior state via `koto rewind`.

```json
{
  "type": "rewound",
  "payload": {
    "from": "review",
    "to": "implement",
    "rationale": "reviewer found a bug"
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `from` | string | Yes | State being rewound from. |
| `to` | string | Yes | State being rewound to. |
| `rationale` | string | No | Human-readable reason. Absent when not provided. |

---

#### `evidence_submitted`

Records what an agent submitted for a state.

```json
{
  "type": "evidence_submitted",
  "payload": {
    "state": "implement",
    "fields": {"pr_url": "https://github.com/..."},
    "submitter_cwd": "/home/user/project"
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `state` | string | Yes | State the evidence was submitted for. |
| `fields` | object | Yes | Agent-provided key-value evidence. Values are arbitrary JSON. |
| `submitter_cwd` | string | No | Working directory of the submitting process. Used internally by the batch scheduler. Consumers MAY ignore this field. |

---

#### `workflow_cancelled`

Records explicit workflow cancellation.

```json
{
  "type": "workflow_cancelled",
  "payload": {
    "state": "implement",
    "reason": "agent interrupted by user"
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `state` | string | Yes | State the workflow was in when cancelled. |
| `reason` | string | Yes | Human-readable cancellation reason. |

---

#### `gate_override_recorded`

Records when an agent bypassed a failing gate via `koto overrides record`.

```json
{
  "type": "gate_override_recorded",
  "payload": {
    "state": "implement",
    "gate": "ci-passes",
    "rationale": "CI infrastructure failure, not code failure",
    "override_applied": {"exit_code": 0, "error": null},
    "actual_output": {"exit_code": 1, "error": "timeout"},
    "timestamp": "2026-05-07T10:01:00.000Z"
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `state` | string | Yes | State containing the overridden gate. |
| `gate` | string | Yes | Gate identifier string. |
| `rationale` | string | Yes | Human-readable reason for the override. |
| `override_applied` | object | Yes | The value substituted as if the gate had produced it. Schema is gate-type-specific. |
| `actual_output` | object | Yes | The gate's actual output at override time. Schema is gate-type-specific. |
| `timestamp` | string | Yes | RFC 3339 UTC timestamp. Matches the outer envelope `timestamp`. |

---

#### `batch_finalized`

Emitted when a batch's `children-complete` gate first reports `all_complete: true`.
The most recent `batch_finalized` event drives `koto status` batch display after
children are auto-cleaned.

**Note**: The `superseded_by` field is always absent in raw JSONL. It is populated
only by rendering code that annotates the event log after the fact. Consumers MUST NOT
expect this field in raw log files.

```json
{
  "type": "batch_finalized",
  "payload": {
    "state": "materialize_children",
    "view": {"all_complete": true, "total": 5, "success": 4, "failure": 1, "skipped": 0},
    "timestamp": "2026-05-07T10:05:00.000Z"
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `state` | string | Yes | The `materialize_children` state the batch finalized from. |
| `view` | object | Yes | Frozen snapshot of the `children-complete` gate output at finalization time. |
| `timestamp` | string | Yes | RFC 3339 UTC timestamp. Matches the outer envelope `timestamp`. |

---

### Tier 2: Optional Display

Consumers MAY surface Tier 2 events for enriched audit trails, detailed execution
history, and debugging views. A minimal viable consumer may omit them without
producing a misleading session view.

---

#### `integration_invoked`

Records when a named integration (external system call) ran during a state.

```json
{
  "type": "integration_invoked",
  "payload": {
    "state": "implement",
    "integration": "github",
    "output": {"pr_number": 42}
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `state` | string | Yes | State during which the integration ran. |
| `integration` | string | Yes | Integration name identifier. |
| `output` | any | Yes | Integration-specific output. Schema varies by integration. |

---

#### `context_added`

Emitted by `koto context add` after a context artifact is stored. All `context_added`
events with `seq < transition.seq` were available before that transition.

```json
{
  "type": "context_added",
  "payload": {
    "key": "research/r1/lead-foo.md",
    "hash": "a3f5b2c1...",
    "size": 4096
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `key` | string | Yes | Context key (e.g., `scope.md`, `research/r1/lead-foo.md`). |
| `hash` | string | Yes | SHA-256 hex digest of the artifact content. 64 hex characters. |
| `size` | integer | Yes | Byte length of the artifact content. |

---

#### `default_action_executed`

Records when a state's automatic shell command ran.

```json
{
  "type": "default_action_executed",
  "payload": {
    "state": "lint",
    "command": "cargo clippy -- -D warnings",
    "exit_code": 0,
    "stdout": "    Finished dev profile",
    "stderr": ""
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `state` | string | Yes | State where the command ran. |
| `command` | string | Yes | Shell command string as configured in the template. |
| `exit_code` | integer | Yes | Process exit code. |
| `stdout` | string | Yes | Standard output. May be large. |
| `stderr` | string | Yes | Standard error. May be large. |

---

#### `decision_recorded`

Records a structured agent decision captured via `koto decisions record`.

```json
{
  "type": "decision_recorded",
  "payload": {
    "state": "design",
    "decision": {"choice": "option-a", "rationale": "..."}
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `state` | string | Yes | State in which the decision was recorded. |
| `decision` | any | Yes | Free-form JSON value. No schema enforced by koto. |

---

#### `gate_evaluated`

Records each gate check result. Multiple `gate_evaluated` events may appear for the
same gate in the same state (e.g., during a polling sequence).

```json
{
  "type": "gate_evaluated",
  "payload": {
    "state": "implement",
    "gate": "ci-passes",
    "output": {"exit_code": 0, "error": null},
    "outcome": "passed",
    "timestamp": "2026-05-07T10:01:00.000Z"
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `state` | string | Yes | State containing the gate. |
| `gate` | string | Yes | Gate identifier. |
| `output` | object | Yes | Gate evaluator output. Schema is gate-type-specific. |
| `outcome` | string | Yes | `"passed"` or `"failed"`. |
| `timestamp` | string | Yes | RFC 3339 UTC timestamp. Matches the outer envelope `timestamp`. |

---

#### `child_completed`

Written to the **parent** session's log when a child workflow reaches a terminal state
and is about to be auto-cleaned. Consumers replaying historical logs (without live
child state access) should use this event to reconstruct batch outcomes.

```json
{
  "type": "child_completed",
  "payload": {
    "child_name": "parent-wf.task-1",
    "task_name": "task-1",
    "outcome": "success",
    "final_state": "done"
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `child_name` | string | Yes | Full composed session name (`{parent}.{task_name}`). |
| `task_name` | string | Yes | Short task name — the segment after the parent prefix dot. |
| `outcome` | string | Yes | Terminal outcome: `"success"`, `"failure"`, or `"skipped"`. |
| `final_state` | string | Yes | The child's terminal state name. |

---

### Tier 3: Internal

Tier 3 events are intended for developer tooling and audit purposes. End-user
dashboards SHOULD NOT surface them in user-visible session views.

---

#### `scheduler_ran`

Per-tick audit record from the batch scheduler. Emitted only on non-trivial ticks
(at least one child spawned, reclassified, errored, or skipped). Pure no-op ticks
are suppressed to prevent log bloat.

```json
{
  "type": "scheduler_ran",
  "payload": {
    "state": "materialize_children",
    "tick_summary": {
      "spawned_count": 3,
      "errored_count": 0,
      "skipped_count": 0,
      "reclassified": false
    },
    "timestamp": "2026-05-07T10:01:00.000Z"
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `state` | string | Yes | State the scheduler ran against. |
| `tick_summary` | object | Yes | Per-tick outcome counts: `spawned_count` (integer), `errored_count` (integer), `skipped_count` (integer), `reclassified` (boolean). |
| `timestamp` | string | Yes | RFC 3339 UTC timestamp. Matches the outer envelope `timestamp`. |

---

## Dashboard `--once` Feed

`koto dashboard --once` prints one tab-separated line per session and exits (no TUI).
It is the scripting surface over the session feed. The dashboard derives all of the
columns below at read time from the data already in this contract (the event log,
header, and computed current state) — nothing new is written to disk.

### Columns

Lines are tab-separated. The first six columns are stable and positionally compatible
with earlier koto releases; columns 7 and 8 were appended additively.

| # | Column | Description |
|---|--------|-------------|
| 1 | `id` | Session name (the tree key). |
| 2 | `current_state` | Current state derived from the event log; empty when the run never advanced. |
| 3 | `elapsed` | Time since session creation (`created_at`), compact form (e.g. `2m5s`, `1h1m`). |
| 4 | `status` | Coarse bucket: `done`, `failed`, `blocked`, `running`, or `unknown`. |
| 5 | `intent` | Derived intent (last `IntentUpdated`, including the init default). |
| 6 | `template` | Template name from the header. |
| 7 | `idle` | **(appended)** Time since the last event (last activity), compact form. |
| 8 | `liveness` | **(appended)** Read-time liveness token (see vocabulary below). |

Appended fields are sanitized of tab, carriage-return, and newline characters so the
tab-separated contract holds. Rows are emitted in attention order (needs-you band
first, then active, then idle/fresh-pending), longest-idle first within a band.

A consumer reading only the first six fields is unaffected. A consumer that asserts a
strict column count must expect eight.

### Liveness Vocabulary

`liveness` is one of the following tokens, computed from last-activity recency with
blocked / terminal / never-started resolved before any idle threshold:

| Token | Meaning |
|-------|---------|
| `needs-you-blocked` | Waiting on a gate that has not passed. A human decision unblocks it. |
| `needs-you-failed` | Terminal in a failed/error state. Needs attention. |
| `needs-you-stalled` | Advanced, then went silent past the stalled threshold (default 2h). |
| `active` | Non-terminal and recently active (idle below the active window, default 5m). |
| `idle` | Non-terminal, idle between the active window and the stalled threshold. |
| `pending` | Never advanced past `workflow_initialized` (no current state). |
| `done` | Terminal, not failed. |

Default thresholds: active window 5m, stalled 2h, abandoned 7d.

### Filtering and the Receded Set

By default the feed excludes the *receded* set — terminal `done` sessions plus
abandoned/stale ones — matching the TUI's attention-first view. Flags:

| Flag | Effect |
|------|--------|
| `--all` | Include the receded set (done + abandoned + stale pending). |
| `--status <token>` | Emit only sessions whose `liveness` equals the given token. |
| `--needs-you` | Emit only sessions in the needs-you band (blocked / failed / stalled). |

Sessions whose state file header fails to parse are excluded from the feed (they are
not valid sessions) but are reported as a trailing `note:` on stderr, so they are
surfaced rather than silently dropped.

## Lifecycle Metadata Surface

Session-level liveness — whether a session is active, idle, blocked, stalled, failed,
pending, or done — is **derived at read time** by the dashboard from this contract's
event log (last-event recency plus the computed terminal/blocked state). It is not a
stored field and requires no migration; see the Dashboard `--once` Feed above for the
vocabulary.

Other session-level metadata — ownership, project tag, a stored summary — is not part
of this contract's current scope. Such fields would belong to a session registry layer
above the raw event log, not to the per-session JSONL file itself.

When such metadata is introduced, the `StateFileHeader` at line 1 is the natural
surface. Adding optional header fields (e.g., `owner`, `project`) follows the same
non-breaking rules as additive optional event fields: `serde(default)` ensures older
readers tolerate their absence.

Consumers should not expect stored lifecycle metadata in the event stream itself.

## Known Gaps

**No `workflow_completed` event**: There is no explicit terminal event. Consumers
determine completion by inspecting the most recent `transitioned` event and comparing
its `to` field against the template's defined terminal states. A log that ends with
the session still in a non-terminal state represents an incomplete or cancelled session.

**Gate output schema is gate-type-specific**: The `output` field on `gate_evaluated`
and `override_applied`/`actual_output` on `gate_override_recorded` carry gate-specific
JSON. Command gates emit `{"exit_code": N, "error": String-or-null}`. Context-exists
gates emit a different structure. No unified schema is enforced; consumers must handle
each gate type they care about and tolerate unknown gate output shapes.

**`batch_finalized.superseded_by` is not in raw JSONL**: The `superseded_by` field
appears in the Rust type and in `koto status` output but is never written to the raw
JSONL log. It is computed and injected by display code (`annotate_superseded_batch_finalized`)
when rendering stale batch events. Consumers reading raw JSONL MUST NOT expect it.
