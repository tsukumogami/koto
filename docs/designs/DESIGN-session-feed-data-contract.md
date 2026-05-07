---
status: Proposed
problem: |
  koto sessions produce JSONL event logs read by multiple planned consumers — a local
  dashboard, an S3-backed dashboard, and a hosted relay. Without an explicit data
  contract, each consumer couples directly to koto's internal Rust types, breaks
  whenever the schema evolves, and independently re-derives audience classification
  for every event type. The schema_version field present in every log file is never
  read by any code, making it a versioning signal with no semantics. The custom
  EventPayload deserializer hard-errors on unknown event type strings, meaning any
  old reader loading a log where a newer koto appended a new event type fails with a
  corruption error rather than graceful degradation.
decision: |
  Publish a markdown reference spec at docs/reference/session-feed.md that defines the
  event envelope, all current event types with canonical field names, a three-tier
  audience classification, reader guarantees, and forward-compatibility rules. Activate
  schema_version as the in-band contract version signal (bumped only for structural
  breaks; additive fields never require a bump). Add an Unknown catch-all variant to the
  EventPayload deserializer so koto's own tools degrade gracefully when reading logs from
  newer koto versions. Breaking changes within an event type use a new type name
  (e.g., transitioned_v2) rather than modifying the existing type.
rationale: |
  A markdown spec fits koto's existing documentation convention and can encode behavioral
  guarantees (ordering, atomicity, seq gaps, single-writer) that JSON Schema cannot.
  schema_version is already in every log at line 1 — activation is one constant and one
  validation guard, costing nothing in new wire format. The Unknown catch-all directly
  fixes the sharpest forward-compat gap identified in the codebase (hard-fail on unknown
  type strings) while preserving typed dispatch for all known variants. A tiered event
  classification prevents three independent consumers from independently re-deriving
  audience intent with no shared guidance.
---

# DESIGN: Session-Feed Data Contract

## Status

Proposed

## Context and Problem Statement

koto records AI agent workflow sessions as append-only JSONL event logs. The first line
of each file is a `StateFileHeader` carrying session metadata; all subsequent lines are
`Event` objects with a monotonically increasing `seq`, a millisecond-precision RFC 3339
timestamp, a `type` string, and a `payload` object. The current schema now includes
session UUIDs, millisecond timestamps, `context_added` events, and optional rationale on
directed transitions (added in the preceding session schema hygiene work).

Multiple consumers are planned: a local dashboard, an S3-backed dashboard, and a hosted
relay. Without an explicit contract, each consumer must:

1. Reverse-engineer the schema from Rust source to know what field names to expect
2. Independently decide which event types are relevant to display
3. Independently decide what to do when a field is missing or a new event type appears

Four concrete problems make an explicit contract necessary now, before any consumer has
adopted the format:

**Dormant schema_version.** The `schema_version` field is present in every log file at
value `1`. It is written at ~20 construction sites but read at zero. Any code that could
check compatibility before parsing fails silently because no check exists.

**Hard-fail on unknown event types.** The `Event` custom deserializer returns an error
when it encounters a type string it doesn't recognize. An old koto binary reading a log
produced by a newer koto that added one new event type fails with `StateFileCorrupted`
rather than skipping the unknown event and continuing. This makes forward-compatibility
fragile for all consumers — including koto's own tools.

**No shared audience guidance.** The 15 current event types range from pure user-facing
progress events (`transitioned`, `directed_transition`) to internal batch-scheduler audit
records (`scheduler_ran`). Without a classification, each consumer decides independently.
Three consumers would produce three independent audience maps with no guarantee they
agree on what constitutes a required display item.

**Undocumented values and field semantics.** Several field values have no public
enumeration: `condition_type` on `transitioned` has three values (`"auto"`, `"gate"`,
`"skip_if"`) but only two appear in tests or documentation. `batch_finalized.superseded_by`
is documented in source as "written as None at append time" — it never appears in raw
JSONL files but would surprise a consumer expecting a field the schema includes.

## Decision Drivers

- **Three planned consumers need shared guidance.** Divergent classification decisions
  across the local dashboard, S3 dashboard, and relay would be hard to correct after each
  consumer has shipped.
- **Forward-compatibility matters before adoption, not after.** The hard-fail on unknown
  event types is fixable now while there are zero external consumers. After adoption it
  becomes a breaking API contract.
- **The spec must be readable without koto source access.** Dashboard developers should
  be able to implement against a published document. Behavioral guarantees (ordering,
  atomicity, single-writer) cannot be inferred from the type definitions alone.
- **Overhead must be proportionate to scale.** One maintainer team, no external API
  consumers yet, ~2-5 eventual dashboard implementers. Heavy tooling (Confluent Schema
  Registry, AsyncAPI) is out of scope.
- **Additive changes must remain non-breaking.** The existing `#[serde(default)]` pattern
  for optional fields is correct and must be preserved and formalized.
- **Raw log and rendered views must be distinguished.** Some fields exist in the type
  definitions but are never written to the raw JSONL file (e.g., `batch_finalized.superseded_by`
  is populated only by rendering code). The contract covers only what appears on disk.

## Considered Options

### Decision 1: Contract versioning mechanism

The contract needs a way to signal to consumers which schema evolution they can expect
from a given log file, so they can decide whether to parse it or reject it early.

**Option A: Activate schema_version as the in-band contract version (chosen)**

Define `CURRENT_SCHEMA_VERSION = 1`. Add a validation guard in `read_header` that warns
(or returns an error, per the rejection policy below) when `header.schema_version >
CURRENT_SCHEMA_VERSION`. The schema_version field is already present in every log at line
1, so no wire format change is required. Semantics: bumped when a new event type is added
(because old koto binaries hard-fail on unknown types), when a required field is removed
from an existing event type, or when the event envelope structure changes (`seq`, `type`,
`timestamp`, `payload` key names). Additive optional fields on existing event types never
require a bump.

**Option B: Per-event-type versioning**

Each event JSON object carries a version field: `{"seq":1,"type":"transitioned","ver":1,
"payload":{...}}`. Readers can handle `transitioned@v1` and `transitioned@v2` independently.
Ruled out: five additive field expansions shipped without a single intra-event-type
breaking change. The version field would add overhead to every event in every log with no
near-term payoff. koto's `#[serde(default)]` pattern already handles intra-type evolution
without any version signal.

**Option C: Spec-only version document, no in-band signal**

The contract version lives only in the spec document. Log files carry no machine-readable
version. Ruled out: a consumer reading a log has no in-band signal that the log predates
feature X. Debugging mismatches requires correlating log timestamps against spec version
publish dates — fragile. This option works as documentation but cannot be validated
programmatically.

**Option D: Independent spec semantic versioning**

The spec is versioned independently (e.g., `session-feed-spec@1.2.0`). Ruled out:
requires coordinated release processes between the spec and the implementation.
Disproportionate overhead with one maintainer team and no external SDK consumers.
Becomes worth it when koto publishes client libraries that independently version against
the spec.

### Decision 2: Unknown event type handling

An old reader encountering a log with a new event type should not fail. The contract must
specify consumer behavior, and koto's own deserializer should match best practice.

**Option A: Consumers MUST skip, koto adds Unknown catch-all variant (chosen)**

The contract requires that readers MUST skip unknown event type strings rather than
failing. Koto's own `Event` deserializer adds an `Unknown` catch-all arm in the custom
Deserialize match block:
```rust
Unknown { type_name: String, raw_payload: serde_json::Value }
```
This prevents koto's own tools (`koto status`, `koto query`) from breaking when reading
logs produced by a newer koto version. It also sets the right precedent for external
consumers. The `type_name()` method returns `"unknown"` for this variant; it is never
serialized (no koto write path produces an Unknown event).

**Option B: Hard-fail on unknown types (current behavior)**

Retain `return Err(...)` in the custom Deserializer match block. Ruled out: any old koto
binary loading a log where a newer koto appended a new event type fails with
`StateFileCorrupted`. This is the sharpest forward-compat gap in the current design and
is fixable before external consumers exist. Retaining it would make every new event type
a breaking change for all existing koto installations.

**Option C: Consumers skip, koto keeps hard-fail**

The contract documents that consumers must skip unknown types, but koto's own
deserializer retains the error behavior. Ruled out: koto's own tools would break on
their own newer-format logs. Sets a conflicting precedent where the spec says one thing
and the reference implementation does another.

**Option D: schema_version bump for new event types only, no catch-all**

Bump schema_version whenever a new event type is added. Readers check the version before
parsing and refuse files above their supported version. Ruled out: this prevents consumers
from reading any events from a log they don't fully support, even events they recognize.
An observer that only cares about `transitioned` events would be locked out of all newer
logs even if `transitioned` itself hasn't changed. Overly restrictive for an observability
consumer.

### Decision 3: Event audience classification

Three consumers (local dashboard, S3 dashboard, relay) need shared guidance on which
events to display and which to skip for end-users.

**Option A: Three-tier classification in the contract (chosen)**

Define three tiers:
- **Tier 1 (required):** Events a consumer MUST surface to give an accurate picture of
  session progress. Omitting any Tier 1 event leaves users with an incomplete or
  misleading view.
- **Tier 2 (optional):** Events that add audit depth or detail. A minimal consumer may
  omit them without being misleading.
- **Tier 3 (internal):** Events intended for developer tooling and debug views, not
  end-user dashboards.

This eliminates the need for three independent classification decisions and establishes a
shared vocabulary for the observability layer.

**Option B: Flat specification (all events equal)**

The contract defines all event types without any audience guidance. Consumers decide what
to show. Ruled out: without guidance, three consumers would independently re-derive
classification with no guarantee they agree. `scheduler_ran` would likely appear in user
dashboards at least sometimes; `workflow_cancelled` would likely be accidentally omitted
in at least one implementation. Shared guidance costs nothing in spec complexity and
prevents common mistakes.

**Option C: Required vs. optional only (two tiers)**

No distinction between enrichment events and internal-only events. `scheduler_ran` is
optional, not internal. Ruled out: `scheduler_ran` is a batch-scheduler audit record with
no user-facing interpretation. Grouping it with `context_added` or `decision_recorded`
(enrichment events with clear display value) incorrectly implies it belongs in
end-user views.

### Decision 4: Artifact form

The contract needs a document form that serves the intended audience and can encode the
full range of required content.

**Option A: Markdown reference document (chosen)**

A markdown file at `docs/reference/session-feed.md` defines the header, event envelope,
all current event type payloads with field-level documentation, versioning semantics,
reader guarantees, and forward-compatibility rules. This fits koto's existing
documentation convention (all other contracts are specified in markdown: CLI output
contract, error codes, template format). Behavioral guarantees — ordering, atomicity,
single-writer requirement, partial-write recovery — cannot be expressed in JSON Schema
and require prose regardless.

**Option B: JSON Schema at a stable URL**

A formal JSON Schema document at `docs/schema/session-feed-v1.json`. Enables off-the-shelf
validation tooling. Ruled out: JSON Schema cannot encode behavioral guarantees (ordering,
atomicity, epoch boundaries, seq gap semantics). A prose supplement is required regardless,
making JSON Schema a second artifact to maintain. The `#[serde(untagged)]` enum pattern
koto uses requires complex `oneOf`/`if-then-else` chains in JSON Schema. No JSON Schema
tooling exists in koto's CI today. A JSON Schema can be added as a supplement once the
markdown spec is stable; the markdown spec is the right primary artifact.

**Option C: AsyncAPI document**

AsyncAPI is designed for networked async APIs with explicit channels and protocol bindings.
Ruled out: the session feed is a local filesystem artifact, not a networked channel.
Mapping a local JSONL file to AsyncAPI concepts is a category mismatch. No tooling benefit
for the intended audience (dashboard developers reading local files or S3 objects).

### Decision 5: Breaking changes within event type payloads

When a field in an existing event type must be changed in a way that breaks existing
consumers (rename, type change, removal), how should the change be published?

**Option A: New event type name for breaking changes (chosen)**

Create a new event type name (e.g., `transitioned_v2`) rather than modifying the existing
type. Both old and new types coexist in the log until the old type is formally deprecated.
This is the industry convention (event-driven.io, EventSourcingDB). It makes version
history visible in the event stream itself. It is consistent with koto's existing practice
of adding new event types rather than modifying existing ones. Consumers that only
understand the old type still parse the old events correctly.

**Option B: schema_version bump for payload-level breaking changes**

Bump schema_version when a field within an existing event type changes in a breaking way.
Readers that check schema_version would refuse the newer logs entirely. Ruled out:
prevents consumers from reading any events from a newer log, even events in types that
weren't modified. A consumer that only reads `transitioned` would lose all access to logs
with an unrelated breaking change in `evidence_submitted`.

## Decision Outcome

Five decisions, all high-confidence:

1. **Activate schema_version as the in-band contract version.** Define
   `CURRENT_SCHEMA_VERSION = 1`. Add validation in `read_header`. Bump only for
   structural breaks: new event type added (old readers hard-fail), required field
   removed from existing type, or event envelope keys changed. Additive optional fields
   never require a bump.

2. **Add Unknown catch-all variant to EventPayload deserializer.** Contract requires
   consumers MUST skip unknown event types. koto's own deserializer implements this
   via a catch-all `Unknown { type_name: String, raw_payload: serde_json::Value }` arm.
   No koto write path produces Unknown events — it exists only for deserialization.

3. **Three-tier event classification.** Tier 1 (required), Tier 2 (optional), Tier 3
   (internal). Classification is in the spec; each consumer decides its own rendering
   priority within tiers but must not omit Tier 1 events from user-visible views.

4. **Markdown reference document as primary artifact.** `docs/reference/session-feed.md`.
   No JSON Schema in this iteration; may be added as a supplement later.

5. **New type name for breaking payload changes.** Additive optional fields are always
   non-breaking. Removing required fields, changing field types, or renaming fields
   requires a new type name (e.g., `transitioned_v2`), not a schema_version bump.

## Solution Architecture

### Contract scope: raw JSONL only

This contract defines the format of raw JSONL log files as written by koto. It does not
cover rendered or projected views. Specifically:
- `batch_finalized.superseded_by` is always absent in raw JSONL (it is populated by
  rendering code, not by the writer). Consumers reading raw files will never see it.
  Rendering layer specifications may extend the contract for their own views.

### File structure

A session log file is a sequence of UTF-8 encoded JSONL lines. Two record types appear:

```
<header-line>
<event-line>
<event-line>
...
```

- **Line 1** is always the header. It is written once at session initialization and
  rewritten during workflow rename operations (`relocate()`). It has no `seq` field.
- **Lines 2+** are events. Each has `seq`, `timestamp`, `type`, and `payload`.
- An empty file or a file with a malformed line 1 must be treated as corrupted.

### Header record

```json
{
  "schema_version": 1,
  "workflow": "my-workflow",
  "template_hash": "abc123...",
  "created_at": "2026-05-06T12:00:00.000Z",
  "session_id": "550e8400-e29b-41d4-a716-446655440000",
  "parent_workflow": "parent-wf",
  "template_source_dir": "/path/to/templates"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `schema_version` | u32 | Yes | Contract version. Currently `1`. Readers that encounter a value higher than their supported version SHOULD refuse to parse further events and report a version mismatch. |
| `workflow` | String | Yes | Workflow name. Matches the state filename stem. Updated in place during rename. |
| `template_hash` | String | Yes | SHA-256 hex digest of the compiled template JSON at init time. |
| `created_at` | String | Yes | RFC 3339 UTC timestamp with millisecond precision: `YYYY-MM-DDTHH:MM:SS.mmmZ`. Session wall-clock creation time. |
| `session_id` | String | No (default: empty) | UUID v4 in lowercase hyphenated form. Stable across renames. Empty string in sessions created before session schema hygiene. Readers MUST treat empty string as "no identifier assigned" rather than an invalid UUID. |
| `parent_workflow` | String | No | Name of the parent workflow if this session was spawned as a batch child. Absent for top-level sessions. |
| `template_source_dir` | String | No | Directory from which the template was loaded at init time. An internal hint used by the batch scheduler's path resolver. Consumers may ignore this field. |

### Event envelope

```json
{
  "seq": 1,
  "timestamp": "2026-05-06T12:00:01.234Z",
  "type": "transitioned",
  "payload": { ... }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `seq` | u64 | Yes | Monotonically increasing, starting at 1 for the first event in a session. A gap between consecutive seq values indicates log corruption; readers MUST treat a gap as a hard error. |
| `timestamp` | String | Yes | RFC 3339 UTC with millisecond precision. Wall-clock time at event creation. Not guaranteed to be monotonically increasing (clock skew is possible). Not validated by the writer for monotonicity. |
| `type` | String | Yes | Discriminant for payload shape. All known type strings are listed below. Readers MUST skip events with unknown `type` values rather than failing. |
| `payload` | Object | Yes | Type-specific payload. Fields within a known event type follow the schema below. Readers MUST ignore unknown fields within a known payload. |

### Reader guarantees

**Ordering:** `seq` values are monotonically increasing. A reader reconstructing session
history MUST use `seq` for ordering, not `timestamp`. Two events MAY share a timestamp
(same-millisecond writes); they will have distinct seq values.

**Durability:** Every successfully committed event has been flushed to stable storage
(`sync_data()` after every append). A reader that observes an event can trust it is
not an uncommitted kernel buffer artifact.

**Atomicity:** The initial state file (header + first events) is written atomically via
tempfile-then-rename. Either the full bundle is visible at the canonical path or it is
not visible at all. Subsequent appends use `O_APPEND` at the OS level, which prevents
byte interleaving between concurrent writers but does not prevent seq duplication if
multiple writers race (see single-writer constraint).

**Partial-write recovery:** A crash between a write syscall and its subsequent `sync_data`
may leave a truncated final line. Readers encountering a malformed final line MUST discard
it and treat all prior lines as complete. A malformed non-final line is hard corruption.

**Single-writer requirement:** The persistence layer does not hold a file lock during
ordinary appends. Seq assignment is a read-then-write sequence with a TOCTOU window.
Callers MUST ensure only one writer appends to a given session file at a time. Concurrent
writes may produce duplicate seq values, which `read_events` will reject as corruption.
koto's own tools enforce this via workflow convention (one koto process per workflow);
relay or dashboard implementations that inject events MUST acquire the advisory
`flock(LOCK_EX)` provided by the `SessionBackend` trait before appending.

**Old format compatibility:** Fields absent in older logs (pre-existing `session_id`,
`parent_workflow`, `template_source_dir`, or any new optional field added after the log
was created) deserialize to their zero/None defaults. Readers MUST NOT fail on absent
optional fields.

**Unknown-version handling:** When `header.schema_version` exceeds the reader's supported
version, the reader SHOULD report a version mismatch to the caller. It MAY attempt a
best-effort parse of known event types, skipping unknowns. Hard rejection is also
acceptable for strict consumers.

**Terminal state detection:** There is no `workflow_completed` or `workflow_succeeded`
event type. Consumers that need to detect workflow completion must read the most recent
`transitioned`, `directed_transition`, `rewound`, or `workflow_cancelled` event and
cross-reference the destination state against the template's terminal state set. A
consumer without template access cannot programmatically determine workflow completion
from the event log alone.

### Forward-compatibility rules

| Scenario | Writer behavior | Reader MUST |
|----------|----------------|-------------|
| New optional field on known event type | Field present in new logs; absent in old | Default absent field; not fail |
| New required field on existing event type | Requires new type name (e.g., `transitioned_v2`) | See "New event type" row |
| Removed field from existing event type | Requires new type name | See "New event type" row |
| New event type | schema_version bumped; new type string appears | Skip the event; continue parsing |
| Unknown `type` string | Produced by a newer writer | MUST skip, MUST NOT fail |
| Unknown field within known payload | Produced by a newer writer | MUST ignore |
| `schema_version` > reader's max | New writer | SHOULD reject or warn; MAY best-effort |

### Event type registry

#### Tier 1: Required display

Consumers MUST surface Tier 1 events in any user-visible session view. Omitting a Tier 1
event results in an incomplete or misleading representation of session progress.

---

**`workflow_initialized`**

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
| `template_path` | String | Yes | Path to the compiled template JSON in koto's cache directory. This is the cache-internal path, not the source template file path. |
| `variables` | Object | No (default: `{}`) | Variable bindings active at init time. String-to-string map. |
| `spawn_entry` | Object | No | Present only for batch-spawned child sessions. Carries `template` (source template path), `vars` (variable bindings as BTreeMap), and `waits_on` (dependency list). Absent for top-level sessions. |

---

**`transitioned`**

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
| `from` | String or null | Yes | Source state name. `null` for the initial transition from no state. |
| `to` | String | Yes | Destination state name. |
| `condition_type` | String | Yes | Transition trigger. One of: `"auto"` (auto-advance condition met), `"gate"` (gate evaluation passed), `"skip_if"` (skip_if condition matched). |
| `skip_if_matched` | Object or null | No | Present when `condition_type` is `"skip_if"`. Carries the key-value pairs from the template's `skip_if` map that triggered the transition. |

---

**`directed_transition`**

Records an explicit state override issued via `koto next --to <state>`.

```json
{
  "type": "directed_transition",
  "payload": {
    "from": "implement",
    "to": "review",
    "rationale": "skipping gate: CI is known-broken on this branch"
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `from` | String | Yes | Source state name. Always non-null (unlike `transitioned.from`). |
| `to` | String | Yes | Destination state name. |
| `rationale` | String | No | Human-readable reason for the directed transition. Absent when `--rationale` was not provided. |

---

**`rewound`**

Records a rollback to a prior state via `koto rewind`.

```json
{
  "type": "rewound",
  "payload": {
    "from": "review",
    "to": "implement",
    "rationale": "reviewer found a bug that needs fixing"
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `from` | String | Yes | State being rewound from. |
| `to` | String | Yes | State being rewound to. |
| `rationale` | String | No | Human-readable reason for the rewind. Absent when `--rationale` was not provided. |

---

**`evidence_submitted`**

Records what an agent submitted for a state.

```json
{
  "type": "evidence_submitted",
  "payload": {
    "state": "implement",
    "fields": {"pr_url": "https://github.com/...", "summary": "..."},
    "submitter_cwd": "/home/user/project"
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `state` | String | Yes | State machine state name. |
| `fields` | Object | Yes | Agent-provided key-value evidence. Values are arbitrary JSON. |
| `submitter_cwd` | String | No | Working directory of the submitting process. Internal hint used by the batch scheduler's path resolver. Dashboards and relays MAY ignore this field. |

---

**`workflow_cancelled`**

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
| `state` | String | Yes | State the workflow was in when cancelled. |
| `reason` | String | Yes | Human-readable cancellation reason. |

---

**`gate_override_recorded`**

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
    "timestamp": "2026-05-06T12:01:00.000Z"
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `state` | String | Yes | State containing the overridden gate. |
| `gate` | String | Yes | Gate identifier string (e.g., `"ci-passes"`). |
| `rationale` | String | Yes | Human-readable reason for the override. |
| `override_applied` | Object | Yes | The value substituted as if the gate had produced it. Schema is gate-type-specific. |
| `actual_output` | Object | Yes | The gate's actual output at override time. Schema is gate-type-specific. |
| `timestamp` | String | Yes | RFC 3339 UTC timestamp with millisecond precision. Matches the outer `Event.timestamp`; included in the payload for consumers reading the payload without the outer envelope. |

---

**`batch_finalized`**

Emitted when a batch's `children-complete` gate first reports `all_complete: true`. The
most recent `batch_finalized` event drives `koto status` batch display after children
are auto-cleaned.

**Note:** The `superseded_by` field, present in the `BatchFinalized` Rust type, is always
absent in raw JSONL files. It is populated only by rendering code that processes the event
log after the fact. Readers of raw JSONL MUST NOT expect this field to be present. This
contract covers the raw log only.

```json
{
  "type": "batch_finalized",
  "payload": {
    "state": "materialize_children",
    "view": {
      "all_complete": true,
      "total": 5,
      "completed": 5,
      "success": 4,
      "failure": 1,
      "skipped": 0
    },
    "timestamp": "2026-05-06T12:05:00.000Z"
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `state` | String | Yes | The `materialize_children` state the batch finalized from. |
| `view` | Object | Yes | Frozen snapshot of the `children-complete` gate output at finalization time. Contains aggregate counts and per-child statuses. Schema matches the `children-complete` gate evaluator output. |
| `timestamp` | String | Yes | RFC 3339 UTC timestamp. Matches the outer `Event.timestamp`. |

*Note: `batch_finalized` appears only in parent sessions that use the batch `materialize_children` state. It does not appear in simple (non-batch) workflows.*

---

#### Tier 2: Optional display

Consumers MAY surface Tier 2 events for enriched audit trails, detailed execution
history, and debugging views. A minimal viable consumer may omit them without producing
a misleading session view.

---

**`integration_invoked`**

Records when a named integration (external system call) ran during a state.

```json
{
  "type": "integration_invoked",
  "payload": {
    "state": "implement",
    "integration": "github",
    "output": { ... }
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `state` | String | Yes | State during which the integration ran. |
| `integration` | String | Yes | Integration name identifier. The set of valid names depends on the template's integration configuration. |
| `output` | Any | Yes | Integration-specific output. Schema varies by integration. |

---

**`context_added`**

Emitted by `koto context add` after a context artifact is successfully stored. Allows
consumers to determine which context artifacts were available at any state transition by
comparing `seq` values: all `context_added` events with `seq < transition.seq` were
available before that transition.

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
| `key` | String | Yes | Hierarchical context key (e.g., `scope.md`, `research/r1/lead-foo.md`). |
| `hash` | String | Yes | SHA-256 hex digest of the artifact content at add time. 64 hex characters. The digest algorithm is SHA-256 and is not carried on the wire. |
| `size` | u64 | Yes | Byte length of the artifact content at add time. |

---

**`default_action_executed`**

Records when a state's automatic shell command ran. The name "default action" refers to
koto's `default_action` template field — a shell command that runs automatically on state
entry without agent intervention.

```json
{
  "type": "default_action_executed",
  "payload": {
    "state": "lint",
    "command": "cargo clippy -- -D warnings",
    "exit_code": 0,
    "stdout": "...",
    "stderr": ""
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `state` | String | Yes | State where the command ran. |
| `command` | String | Yes | Shell command string as configured in the template. |
| `exit_code` | i32 | Yes | Process exit code. |
| `stdout` | String | Yes | Standard output. May be large. Consumers displaying this should truncate or paginate. |
| `stderr` | String | Yes | Standard error. May be large. |

---

**`decision_recorded`**

Records a structured agent decision captured mid-state via `koto decisions record`.

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
| `state` | String | Yes | State in which the decision was recorded. |
| `decision` | Any | Yes | Free-form JSON value. No schema enforced by koto. |

---

**`gate_evaluated`**

Records each gate check result. Multiple `gate_evaluated` events may appear for the same
gate in the same state (e.g., during a polling sequence). The final `gate_evaluated`
before a `transitioned` event is the one that unblocked the transition.

```json
{
  "type": "gate_evaluated",
  "payload": {
    "state": "implement",
    "gate": "ci-passes",
    "output": {"exit_code": 0, "error": null},
    "outcome": "passed",
    "timestamp": "2026-05-06T12:01:00.000Z"
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `state` | String | Yes | State containing the gate. |
| `gate` | String | Yes | Gate identifier (e.g., `"ci-passes"`). |
| `output` | Object | Yes | Gate evaluator output. Schema is gate-type-specific: command gates emit `{"exit_code": N, "error": String-or-null}`; context gates emit a different structure. |
| `outcome` | String | Yes | Gate result. One of: `"passed"`, `"failed"`. |
| `timestamp` | String | Yes | RFC 3339 UTC timestamp. Matches the outer `Event.timestamp`. |

---

**`child_completed`**

Written to the **parent** session's log when a child workflow reaches a terminal state
and is about to be auto-cleaned. Serves as a fallback for event-log-only consumers
reconstructing batch outcomes after child state files are removed.

Consumers with access to live child state files MAY ignore this event (BatchFinalized
covers the same ground). Consumers performing historical replay of the event log (without
live state access) should use this event to reconstruct which children finished and with
what outcome.

```json
{
  "type": "child_completed",
  "payload": {
    "child_name": "parent-wf.task-1",
    "task_name": "task-1",
    "outcome": "success",
    "final_state": "complete"
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `child_name` | String | Yes | Full composed session name (e.g., `"parent-wf.task-1"`). Dot-delimited: `{parent}.{task_name}`. |
| `task_name` | String | Yes | Short task name — the segment after the parent prefix dot. |
| `outcome` | String | Yes | Terminal outcome. One of: `"success"`, `"failure"`, `"skipped"`. |
| `final_state` | String | Yes | The child's terminal state name. |

---

#### Tier 3: Internal

Tier 3 events are intended for developer tooling and audit purposes. End-user dashboards
SHOULD NOT surface them in user-visible session views.

---

**`scheduler_ran`**

Per-tick audit record from the batch scheduler. Emitted only on non-trivial ticks (at
least one child spawned, reclassified, errored, or skipped). Pure no-op ticks are
suppressed to prevent log bloat. This event carries scheduling mechanics, not user-visible
session progress.

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
    "timestamp": "2026-05-06T12:01:00.000Z"
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `state` | String | Yes | State the scheduler ran against. |
| `tick_summary.spawned_count` | u32 | Yes | Children spawned this tick. |
| `tick_summary.errored_count` | u32 | Yes | Children that errored this tick. |
| `tick_summary.skipped_count` | u32 | Yes | Children skipped this tick. |
| `tick_summary.reclassified` | bool | Yes | Whether any child's classification changed during this tick. |
| `timestamp` | String | Yes | RFC 3339 UTC timestamp. Matches the outer `Event.timestamp`. |

---

#### Unknown events

Readers MUST NOT fail when encountering an event type string not listed in this contract.
They MUST skip the event and continue parsing subsequent lines. Log files are
append-only; a newer koto version may have written event types that an older reader does
not know about.

## Implementation Approach

Two parallel tracks: the spec document (documentation), and the koto implementation
changes (code). Neither blocks the other, but both must ship in the same PR to keep
the contract and the implementation consistent.

### Track 1: Spec document

Write `docs/reference/session-feed.md` containing the header spec, event envelope,
reader guarantees, forward-compatibility rules, and all event type definitions with
their tier classification. This is the primary contract artifact.

The doc draws from this design doc's Solution Architecture section. Include concrete
JSON examples for every event type. Explicitly call out the known gaps: no
`workflow_completed` event, gate output schema is gate-type-specific.

### Track 2: Implementation changes

**1. Unknown catch-all variant in EventPayload**

Add an `Unknown` variant to `EventPayload`:

```rust
/// Catch-all for event type strings not recognized by this koto version.
/// Produced only during deserialization; never written by any koto writer.
Unknown {
    /// The raw type string from the JSONL line.
    type_name: String,
    /// The raw payload JSON, preserved for logging or forwarding.
    raw_payload: serde_json::Value,
},
```

In the custom `Deserialize` match block, replace the `other =>` hard-error arm with:

```rust
other => EventPayload::Unknown {
    type_name: other.to_string(),
    raw_payload: payload,
},
```

Update `type_name()` to return `"unknown"` for this variant (used in log output, not in
any write path). No `Serialize` arm needed — `Unknown` events are never written.

The advance loop, `read_events`, `koto status`, and `koto query` must handle `Unknown`
gracefully: skip it in any match that drives state machine logic; include it in raw event
dumps (`koto query --events`) for diagnostic purposes.

**2. Activate schema_version**

Add to `src/engine/types.rs`:

```rust
/// Contract version of the session JSONL format. Bumped when a new event type
/// is added (old readers hard-fail on unknown types) or when the header envelope
/// or event envelope structure changes. Additive optional fields within existing
/// event types do not require a bump.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;
```

Add a validation check in `persistence::read_header` (or `local::read_header`):

```rust
if header.schema_version > CURRENT_SCHEMA_VERSION {
    return Err(SessionError::IncompatibleSchemaVersion {
        found: header.schema_version,
        max_supported: CURRENT_SCHEMA_VERSION,
    });
}
```

Existing `StateFileHeader` construction sites already write `schema_version: 1` — no
change needed there. The constant formalizes the value and provides a single update
point for future bumps.

**3. No schema_version bump in this PR**

This PR adds neither new event types nor structural changes to the envelope. The
`Unknown` catch-all is a deserializer addition, not a schema change. `schema_version`
stays at 1.

### Test coverage

- Unit test: `Unknown` variant round-trips through deserialization (unknown type string
  produces `Unknown` payload; does not fail).
- Unit test: reading a log with an unknown event type in a non-final position does not
  return `StateFileCorrupted`.
- Unit test: `schema_version` validation guard: log with `schema_version > 1` returns
  `IncompatibleSchemaVersion` error.
- Existing integration tests: verify no regression in the advance loop when unknown
  events appear in the event stream.

## Security Considerations

**Unknown catch-all and event swallowing.** The `Unknown` catch-all silently accepts
event type strings that are not recognized. If a malicious writer injects a crafted
event with an unknown type string, it would be silently skipped by koto readers. The
risk is bounded: koto's session files are owned by the local user (mode 0600) and are
not exposed to untrusted writers. The relay append model (injecting management events
between agent invocations) must validate event type strings before injection; this is
the relay's responsibility, not koto's deserializer's.

**Log tamper-evidence.** The session JSONL log is append-only by convention, not by
enforcement. There is no HMAC, digital signature, or hash chain. `context_added.hash`
records content integrity for context artifacts at add time but does not make the log
itself tamper-evident. Consumers must assume the log is trusted from the local
environment; relay and dashboard implementations that store and re-serve logs must apply
their own integrity guarantees (e.g., S3 object versioning, storage-layer checksums).

**session_id as a tracking identifier, not a security principal.** `session_id` is a
UUID v4 for session tracking and deduplication. It is not an authentication credential.
A consumer must not use `session_id` for access control decisions. Two sessions with the
same `session_id` should not occur (UUID v4 from CSPRNG has negligible collision probability),
but a collision or a replayed session ID does not constitute an authentication bypass.

**schema_version rejection.** Adding a version check that rejects `schema_version >
CURRENT_SCHEMA_VERSION` is a correctness guard, not a security boundary. It prevents
miscommunication between incompatible koto versions; it does not prevent a malicious
writer from crafting any `schema_version` value.

**Single-writer enforcement.** Concurrent writes to the same session file without
holding the advisory flock can corrupt seq sequencing. The corruption manifests as a
`StateFileCorrupted` error on the next read — a denial-of-service to the reader, not
a data injection. The flock is available; callers that write from non-koto paths (e.g.,
a relay injecting events) must acquire it.

## Consequences

### Positive

- Dashboard and relay implementers have a single, authoritative document to build against.
  Field names, value enumerations (e.g., `condition_type` values), and reader behavior
  are no longer reverse-engineered from Rust source.
- Shared event tier classification prevents divergent audience decisions across three
  independent consumers.
- The `Unknown` catch-all removes the sharpest forward-compat gap: koto's own tools
  no longer hard-fail when reading logs from a newer koto version.
- schema_version activation gives consumers a machine-readable compatibility signal
  at line 1, before parsing any events.
- The raw-log vs. rendered-view distinction (`batch_finalized.superseded_by`) prevents
  consumers from building against a field that is never present in raw files.

### Negative

- The `Unknown` catch-all silently swallows events that, under the old behavior, would
  have surfaced as errors. A consumer that reads a log from a version with a critical
  new event type (e.g., a future `PauseRequested` event) will silently skip those events
  rather than alerting.
- Adding a schema_version validation guard creates a new failure mode: koto binaries
  older than the guard version will reject newer-format log files if they implement the
  check. This is the correct behavior but must be communicated to users who mix koto
  versions.
- The markdown spec introduces a second artifact to keep in sync with `src/engine/types.rs`.
  A field rename in Rust without updating the spec silently diverges. No CI check enforces
  this alignment (a JSON Schema companion would enable one, but is deferred).
- The `template_path` semantic confusion (cache path, not source path) is documented
  but not fixed. Adding a `source_template_path` field to `workflow_initialized` would
  require a new event type or an additive field addition; this is deferred to a follow-on
  change.
- There is no `workflow_completed` event. Consumers that need terminal state detection
  must have template knowledge or use the timestamp of the last transition event as a
  proxy. This limitation is documented but not fixed in this design.

### Mitigations

- The `Unknown` catch-all risk is mitigated by the schema_version guard: a consumer that
  has not been updated to support the new version should refuse to parse rather than
  silently skip. Consumers that implement both the version guard and the catch-all get the
  right behavior: reject when the format is too new, degrade gracefully within a
  compatible format version.
- The spec-code divergence risk is mitigated by co-locating the implementation PR with
  the spec commit. Future PRs that add or rename EventPayload fields must include a spec
  update as part of the checklist (enforced by the PR template, not by CI).
