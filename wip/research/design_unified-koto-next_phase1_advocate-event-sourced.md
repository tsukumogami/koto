# Advocate: Event-Sourced State Machine

## Approach Description

Model state transitions as typed, immutable events with declared input schemas. The state
file is an append-only event log; current state is derived by replaying events in sequence.
Evidence submission becomes event submission — the agent submits a typed event payload
validated against a schema declared in the template. The `expects` field in `koto next`
output describes the next expected event type and its input schema. History is first-class,
not an afterthought.

**Example state file as event log:**

```json
{
  "schema_version": 2,
  "workflow": "my-workflow",
  "events": [
    {
      "seq": 1,
      "type": "transition",
      "from": null,
      "to": "gather_info",
      "timestamp": "2026-03-13T10:00:00Z",
      "payload": {}
    },
    {
      "seq": 2,
      "type": "evidence_submitted",
      "state": "gather_info",
      "timestamp": "2026-03-13T10:05:00Z",
      "payload": { "input_file": "results.json" }
    },
    {
      "seq": 3,
      "type": "transition",
      "from": "gather_info",
      "to": "analyze_results",
      "timestamp": "2026-03-13T10:05:01Z",
      "payload": { "evidence": { "input_file": "results.json" } }
    }
  ],
  "current_state": "analyze_results"
}
```

Replaying events yields the current state. Each `evidence_submitted` event is scoped to the
state in which it was submitted. Per-state evidence scoping is a structural property, not
a policy — there's no global evidence map to contaminate.

**Example `koto next` output under this model:**

```json
{
  "state": "analyze_results",
  "action": "execute",
  "directive": "Review the test output and determine next steps.",
  "advanced": true,
  "expects": {
    "event_type": "transition_intent",
    "schema": {
      "decision": { "type": "enum", "values": ["proceed", "escalate"] },
      "rationale": { "type": "string", "required": true }
    },
    "transitions": [
      { "target": "deploy", "when": { "decision": "proceed" } },
      { "target": "escalate_review", "when": { "decision": "escalate" } }
    ]
  }
}
```

## Investigation

The current state file (`State` struct in `pkg/engine/types.go`) is mutable: `CurrentState`,
`Evidence` map, `Version` counter, `History []HistoryEntry`. The engine reads state,
modifies it in place, and atomically writes the full struct via `persist()`.

An event-sourced model replaces the mutable state file with an append-only event log. The
engine appends events and derives current state by replay. `Evidence` disappears as a
top-level field — it's contained within events. Version conflicts are detected by sequence
numbers. The `HistoryEntry` array becomes the event log itself.

The `persist()` function changes from a full-document overwrite to an event append operation,
which is simpler and safer — an append either succeeds or fails; a partial write is detectable
by comparing the last event's sequence number to what was being written.

Human-directed transitions (`koto next --to`) become manual transition events with a
`directed: true` marker — same event model, different payload semantics.

Per-state evidence scoping emerges naturally: each `evidence_submitted` event carries a
`state` field; evidence is scoped by definition to the state it was submitted in. No special
clearing logic is needed.

## Strengths

- **Evidence scoping is structural**: per-state scoping is a natural consequence of the
  event log model, not a policy enforced by clearing logic; there's no global evidence
  map to accidentally contaminate
- **Audit trail is first-class**: the event log is the state file; every action taken in
  the workflow is preserved in replay order with full payloads; this is the kind of history
  that makes debugging and recovery trivial
- **`expects` emerges naturally**: the current state's expected next event type and schema
  derives directly from the state machine definition; no separate derivation logic needed
- **Atomic writes become simpler**: appending an event is atomic by construction (file
  systems provide atomic append semantics); no need for temp-file-and-rename patterns
- **Recovery is well-defined**: replaying the event log from the last valid event is
  standard event-sourcing crash recovery; no edge cases around partial state mutations
- **Directed transitions are first-class**: `--to` maps to a manual transition event with
  `directed: true`, recorded in the log like any other event; no special bypass logic

## Weaknesses

- **Breaking migration is the largest of all approaches**: every existing state file must
  be transformed from mutable-state format to event log format; this is non-trivial for
  workflows that are mid-execution at migration time
- **Event replay performance**: for long-running workflows with many transitions, replaying
  the full event log on every `koto next` call adds latency; mitigated by snapshotting
  (store a derived state snapshot alongside the log) but adds complexity
- **Schema versioning is harder**: once an event is appended to the log, its schema is
  fixed; if the template's event schema changes, old log entries may not replay correctly
  against the new schema; this requires event versioning and schema evolution strategies
- **Mental model shift for template authors**: thinking in "events with types and schemas"
  is more complex than "states with transitions and gates"; the template format must express
  event schemas alongside transition definitions

## Deal-Breaker Risks

- **Schema mismatch leaves evidence unrecoverable**: in the current mutable model, a
  wrong evidence key can be corrected by resubmitting; in the event log model, an
  already-appended event with wrong payload is permanent. This requires careful validation
  before appending — which the engine must enforce, not the template. If validation is
  incomplete, recovery requires manual event log surgery.
- **Event replay consistency**: the template must remain compatible with the event log it
  generated; if a template is updated mid-workflow (state names changed, conditions
  changed), replaying old events against the new template may fail. A mitigation is to
  snapshot the compiled template alongside the log, but this adds complexity.

## Sub-Design Boundaries

1. **Event log format design**: the state file schema — event types, payload schemas, sequence
   numbers, snapshot format; migration path from current state file format
2. **Template format design**: declaring event schemas alongside transition conditions and
   evidence requirements; compilation to event type definitions
3. **CLI contract design**: `koto next` output with `expects.event_type` and schema;
   `--with-data` as event payload submission; error codes for schema validation failures
4. **Engine execution design**: event append semantics, replay, current-state derivation,
   auto-advancement loop, snapshot management

## Implementation Complexity

- Scope: **Large** (largest of the four approaches due to state file migration)
- CLI: add structured output with `expects.event_type` and schema; map events to directives
- State model: complete replacement of mutable-state file with event log; migration tool
  for existing state files; optional snapshot mechanism for performance
- Template format: add event schema declarations; compilation to event type definitions

## Summary

The event-sourced model aligns the three interdependent systems around a single principle —
every state change is an immutable, typed event — which makes per-state evidence scoping
structural rather than enforced, history first-class rather than appended, and recovery
well-defined rather than ad hoc. The cost is the largest migration burden of any approach
and a more complex template authoring model. It's the right long-term architecture if koto
workflows are expected to grow in complexity and longevity; it's potentially over-engineered
if most workflows are short-lived and simple.
