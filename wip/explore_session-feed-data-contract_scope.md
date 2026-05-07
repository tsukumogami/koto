# Explore Scope: session-feed-data-contract

## Visibility

Public

## Core Question

How should koto's JSONL session event log be formalized into a stable, versioned data
contract that external consumers (dashboards, relay) can build against without coupling
to koto's internal Rust types? The contract must define canonical field names, versioning
strategy, forward-compatibility rules, and reader guarantees for all current EventPayload
variants and the StateFileHeader.

## Context

koto records AI agent workflow sessions as append-only JSONL event logs. F1 (session
schema hygiene) is now complete: the schema has session_id, millisecond timestamps,
context_added events, and rationale on directed transitions. The next step (F2) is to
define a stable data contract before external consumers adopt the format.

F3 (local dashboard) and F4 (lifecycle metadata) both require this contract as a
prerequisite. The contract must cover all 15 current EventPayload variants plus
StateFileHeader, define what readers can rely on, and specify how schema evolution works.

Key constraints:
- Public repo: no private references in artifacts
- Backward-compatible with logs written before F1
- Must survive the koto codebase evolving without breaking published consumers
- The JSONL log is append-only (no rewrites); reader guarantees must reflect this

## In Scope

- All current EventPayload variants (WorkflowInitialized, Transitioned, EvidenceSubmitted,
  IntegrationInvoked, DirectedTransition, Rewound, ContextAdded, WorkflowCancelled,
  DefaultActionExecuted, DecisionRecorded, GateEvaluated, GateOverrideRecorded,
  SchedulerRan, BatchFinalized, ChildCompleted)
- StateFileHeader (first-line record, no seq, includes session_id)
- Canonical JSON field names for each event type
- Versioning strategy and version bump semantics
- Reader guarantees (ordering, atomicity, completeness, partial-write behavior)
- Forward-compatibility rules (unknown types, unknown fields, version mismatch)
- Classification of events by consumer relevance (dashboard vs. internal)
- Artifact form (what the contract document IS)

## Out of Scope

- New event types (those belong to F4 lifecycle metadata and future features)
- Dashboard or relay implementation details
- koto engine internals not visible in the JSONL feed
- Relay infrastructure, S3 backend specifics

## Research Leads

1. **What canonical JSON event type names and field names should the contract specify?**
   Are the current Rust-derived names (e.g., condition_type, skip_if_matched,
   GateOverrideRecorded) appropriate for a public API surface, or do any need aliasing?
   What does a consumer need to understand each event without reading koto source?

2. **What versioning strategy fits an append-only JSONL contract?**
   schema_version is already in the header at value 1. Should the contract version live
   there? Per-event-type versions? A separate spec version? What does a version bump
   mean for existing readers and existing logs?

3. **What reader guarantees can koto make and how should they be specified?**
   What does the persistence layer promise about ordering (seq monotonicity), atomicity
   of appends, completeness (partial write on crash), and the header/event boundary?
   What do readers need to know to correctly replay a session?

4. **Which events are consumer-relevant vs. internal to koto's batch scheduler?**
   Events like SchedulerRan, BatchFinalized, ChildCompleted serve batch orchestration.
   Should the contract classify events by audience (required, optional, internal)?
   Or is the contract flat and consumers decide what to display?

5. **What have industry event-log schemas learned, and what artifact form fits this contract?**
   CloudEvents, OpenTelemetry, and NDJSON specs have documented decisions on versioning,
   field canonicalization, and reader guarantees. What's applicable? And should this
   contract be a markdown spec in docs/, a JSON Schema, or another form?
