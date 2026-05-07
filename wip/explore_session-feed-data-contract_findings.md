# Exploration Findings: session-feed-data-contract

## Core Question

How should koto's JSONL session event log be formalized into a stable, versioned data
contract that external consumers (dashboards, relay) can build against without coupling
to koto's internal Rust types?

## Round 1

### Key Insights

- **schema_version is dormant** (lead-versioning-strategy): Written as `1` in every log
  file at ~20 sites, but read at zero. No code dispatches on it. Activation as an in-band
  contract version requires only a constant + validation guard in `read_header`. This is
  the right mechanism because it's already present in every log at line 1.

- **Hard-fail on unknown event types is the sharpest forward-compat gap**
  (lead-versioning-strategy, lead-ecosystem-patterns): koto's custom Deserializer returns
  `StateFileCorrupted` when it encounters an unknown type string. Any old reader loading
  a log where a newer koto appended a new event type fails completely. Industry best
  practice (CloudEvents, OTLP, event sourcing) is to skip unknown event types, not error.

- **Three-tier event classification works and has evidence** (lead-consumer-event-classification):
  8 events required, 6 optional enrichment, 1 internal (SchedulerRan). Three planned
  consumers (local dashboard, S3 dashboard, relay) benefit from shared guidance.

- **Markdown spec is the right artifact form** (lead-ecosystem-patterns): JSON Schema
  can't encode behavioral guarantees (ordering, atomicity, seq gaps). Fits koto's
  existing pattern. Primary audience is 2-5 implementers who read the spec to understand
  the format.

- **`batch_finalized.superseded_by` is a computed projection never written to raw JSONL**
  (lead-event-type-canonicalization): Always `None` at append time. Higher-level rendering
  code may populate it. A reader of raw JSONL will never see it populated. The contract
  must distinguish raw log (the contract's subject) from rendered views.

- **Several field names need documentation, not renaming** (lead-event-type-canonicalization):
  `condition_type` has 3 undocumented values (`auto`, `gate`, `skip_if`); `view` in
  `batch_finalized` is a frozen gate-output snapshot; `submitter_cwd` in `evidence_submitted`
  is an internal path-resolver hint with no display value.

- **Strong durability, weak concurrency** (lead-reader-guarantees): `sync_data()` after
  every append guarantees durability. Atomic init via `renameat2`. But `append_event`
  has no file lock — seq assignment is read-then-write with a TOCTOU window. Single-writer
  is a caller convention, not enforced by the persistence layer.

- **No `workflow_completed` event** (lead-event-type-canonicalization): Terminal state
  detection requires template knowledge (`terminal: true` on the state). An event-log-only
  reader cannot detect workflow completion without parsing the template. The contract must
  document this gap.

### Tensions

- **schema_version activation vs. Unknown catch-all**: Two overlapping solutions to the
  unknown-event-type problem. A version bump lets readers gate on compatibility. An Unknown
  catch-all lets old koto binaries skip new events instead of crashing. These are
  complementary: the contract can require both.

- **GateEvaluated tier**: Consumer classification agent says optional; the planned F3
  dashboard spec explicitly mentions gate evaluations as a display item. Resolution: Tier 2
  (optional) in the contract with a note that F3 implementations treat it as required.

- **`submitter_cwd` on `EvidenceSubmitted`**: The event is Tier 1 (required) but this
  field is internal. Field-level annotation is more precise but complicates the spec.
  Resolution: note it as an internal-hint field within an otherwise Tier 1 event.

### Gaps

- GateEvaluated outcome values not fully enumerated (`passed`, `failed` — are there others?)
- Gate output schema per gate type not defined (command gate vs. context gate produce
  different output shapes)
- `hash` in `context_added` has no algorithm identifier on the wire
- Timestamp monotonicity not validated by the reader — clock skew could produce
  non-monotonic timestamps

### Decisions

- Auto mode: crystallize to Design Doc without further rounds. Evidence is sufficient
  to write the full design. All five decision questions have clear answers from research.

## Decision: Crystallize

## Accumulated Understanding

koto's JSONL session event log has a clear structure (header + events) with strong
durability guarantees per write but weak concurrency enforcement. The 15 current event
types are readable on the wire but several field names and undocumented values need
specification. The versioning story is dormant but activatable with minimal code change.
Industry patterns align on: skip unknown types, default unknown fields, additive optional
fields are non-breaking, markdown is the right spec form for this audience.

The design doc needs to decide: versioning mechanism, unknown-event-type handling,
event tier classification, artifact form, and breaking-change convention. All five have
clear evidence-backed answers from the research.
