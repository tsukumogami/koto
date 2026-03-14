---
status: Proposed
upstream: docs/prds/PRD-unified-koto-next.md
problem: |
  To be completed after approach selection.
decision: |
  To be completed after approach selection.
rationale: |
  To be completed after approach selection.
---

# DESIGN: Unified koto next Command

## Status

Proposed

## Context and Problem Statement

koto's current architecture treats state reading and state advancement as separate
operations. Agents call `koto next` to read the current directive, then call
`koto transition <target>` to advance — and must know which to call and when. As new
capabilities are added (evidence submission, delegation, per-transition conditions),
this model breaks down: the set of valid operations at any point grows, agents must track
it themselves, and each new capability adds a new command.

Three systems need to change together to fix this. The **CLI contract** — what `koto next`
accepts and returns — must become self-describing so agents never need out-of-band
knowledge. The **state model** — what koto persists between calls — must support per-state
evidence scoping so evidence doesn't contaminate branching or looping workflows. The
**workflow definition format** — how developers author koto templates — must support
per-transition conditions so workflows can branch based on what agents submit without
agents naming target states.

These systems are interdependent: the CLI contract depends on what state is stored, and
state storage depends on what templates can declare. But each is large enough to warrant
its own tactical design. This document makes the three unifying high-level decisions and
defines the constraints each tactical design must satisfy.

## Decision Drivers

- **Agent contract stability**: the CLI surface (`koto next` input/output schema) must
  stay constant as capabilities are added; tactical sub-designs must not require flag or
  schema changes visible to agents
- **Self-describing output**: an agent that has never seen the workflow template must be
  able to determine its next action from the `koto next` response alone
- **Breaking change scope**: both state file format and template format changes are
  breaking; the design must make clear what migration is required and by whom
- **Sub-design independence**: tactical designs for CLI contract, state model, and
  template format must be implementable in sequence without circular dependencies
- **Evidence correctness**: per-state evidence scoping is required for correctness with
  directed transitions and looping workflows — the design must specify the scoping model,
  not defer it
- **Template authoring ergonomics**: workflow developers need a template format that's
  readable and writable without understanding koto internals
- **Testability**: every system boundary (CLI output, state persistence, template
  compilation) must be independently testable

## Considered Options

### Decision: Design philosophy for unifying CLI contract, state model, and workflow definition

koto needs three interdependent systems to change together: the CLI contract (what `koto
next` outputs and accepts), the state model (what koto persists between calls), and the
workflow definition format (what templates declare). The question isn't just what to change
in each — it's what philosophy should govern how they fit together. Choose the wrong anchor
and the three systems pull in different directions. Choose right and each system's design
follows naturally from the others.

The PRD mandates breaking changes to both the state file format and the template format, so
migration cost is unavoidable. The question is whether that migration buys a better
long-term architecture or just the minimum viable one.

#### Chosen: Event-Sourced State Machine

The state file becomes an append-only event log. Every state transition — whether triggered
by auto-advancement, evidence submission, or a human-directed override — is recorded as an
immutable typed event with a sequence number, timestamp, event type, and payload. Current
state is derived by reading the final transition event in the log; there is no mutable
`CurrentState` field to synchronize. Evidence is no longer a global accumulated map — it
lives inside the events that submitted it, scoped by definition to the state in which it
was submitted.

This changes the three systems in a coherent way. The **state model** becomes an event log
schema: what event types exist, what each carries, how sequence numbers work, how snapshots
accelerate replay for long logs. The **template format** declares event schemas: per-transition
conditions become per-event-type schemas that describe what payload triggers which transition.
The **CLI contract** describes what event the current state expects next: the `expects` field
in `koto next` output specifies an event type and its input schema, derived from the template's
event declarations.

Per-state evidence scoping — one of the most critical correctness requirements in the PRD —
is structural rather than enforced. There is no global evidence map to clear. Evidence
submitted while in state A is in the event log as "submitted while in state A." A transition
event that moves to state B doesn't reference state A's evidence; it simply doesn't contain
it. Template authors don't need to reason about clearing semantics because the log model
makes contamination impossible by construction.

The audit trail becomes first-class. The event log is the state file; replaying it yields
not just current state but the complete, timestamped record of every action taken in the
workflow. Recovery is replay from the last valid event — no partial-mutation edge cases.
Atomic writes simplify: appending an event is simpler than the current temp-file-rename
pattern because an append either succeeds fully or fails detectably (sequence number gap).

The migration cost is real: existing state files must transform from the mutable snapshot
format to event log format, and existing templates must be rewritten to declare event
schemas alongside transition conditions. A migration tool and format version detection are
required. Workflows mid-execution at migration time need careful handling.

#### Alternatives Considered

**Protocol-First**: Design the `koto next` JSON output schema first; derive state model and
template format from what the protocol needs to produce. Strong at creating a stable agent
contract, but doesn't solve the fundamental model problem — it specifies what the output
looks like without specifying how state is structured. The event-sourced model produces a
better protocol as a consequence of its structure, rather than treating the protocol as a
constraint to design around.

**Declarative Language First**: Design the template format as the primary artifact; derive
state model and CLI output from template declarations. Creates one source of truth for
workflow semantics, but evidence schema expressiveness is limited by what YAML can declare
cleanly. Complex schemas require an embedded DSL. The event-sourced model handles schema
declaration through event types, which are a more natural fit for the "what does this
transition accept?" question than YAML blocks.

**Minimal Extension**: Extend the existing model with the minimum changes required —
backward-compatible template additions, policy-based evidence clearing, optional `expects`
declarations. Lowest migration burden and fastest to ship. Rejected because the PRD already
mandates breaking changes to both format and state file; given that, minimal extension buys
lower scope at the cost of a weaker long-term model. Two coexisting transition syntaxes and
policy-based evidence clearing (rather than structural scoping) create ongoing maintenance
burden. When a breaking migration is unavoidable, the right question is what to buy with it.

## Decision Outcome

**Chosen: Event-Sourced State Machine**

### Summary

koto's state file changes from a mutable document (`CurrentState`, `Evidence`, `History`)
to an append-only event log. Every state change is a typed, immutable event: `workflow_initialized`,
`transitioned`, `evidence_submitted`, `directed_transition`. Each event carries a sequence
number, timestamp, event type, and type-specific payload. Current state is the `to` field
of the last `transitioned` event. Evidence for a state is the union of all
`evidence_submitted` events whose `state` field matches the current state. There is no
global evidence map; per-state scoping is a structural property of the log.

The template format adds event schema declarations alongside the existing state and
transition structure. A state that expects evidence submission declares the event schema —
what fields the `evidence_submitted` payload must contain and what per-transition `when`
conditions determine which outgoing transition fires. These declarations drive two things:
(1) the `koto next` output `expects` field, which is computed from the current state's
event schema and presented to the agent as a self-describing contract, and (2) payload
validation when the agent submits via `--with-data`.

`koto next` output gains `advanced: bool`, structured `error` (with code and message),
and `expects` (with event type, field schema, and per-transition options). The `--with-data`
flag submits an `evidence_submitted` event; `--to` submits a `directed_transition` event.
Both are appended to the log and trigger re-evaluation. Auto-advancement chains through
states by appending `transitioned` events until a stopping condition; each event in the
chain is independently durable. A crash mid-chain leaves the log at the last valid event;
resuming replays from there.

The three tactical sub-designs are: (1) event log format and state file schema, (2) template
format event schema declarations and compilation pipeline, (3) CLI contract — the `koto next`
output schema including `expects`, error codes, and integration output. A fourth sub-design
covers the auto-advancement engine: replay, current-state derivation, loop, stopping
conditions, and integration invocation. Each sub-design can proceed once the event log
format is accepted, since all three depend on the event type taxonomy.

### Rationale

The event-sourced model earns the migration cost the PRD already requires. Breaking changes
to state file format and template format are unavoidable; the question is what architecture
to buy with them. The mutable-state model requires policy-based evidence clearing (clear the
map on each transition) and careful atomicity to avoid partial mutations. The event log model
makes both unnecessary: evidence scoping is structural (events are immutable and state-tagged),
and writes are appends (simpler atomicity guarantees). These aren't minor improvements — they
eliminate entire categories of bugs and simplify the recovery story for long-running workflows.

The approach also aligns the three systems around a single concept. Every API boundary in
koto — what templates declare, what the state file stores, what `koto next` outputs — is
expressed in terms of events and their schemas. This makes the tactical sub-designs coherent:
each one is specifying a different view of the same event taxonomy.


