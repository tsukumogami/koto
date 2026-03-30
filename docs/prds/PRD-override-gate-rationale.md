---
status: Draft
problem: |
  When agents override gates in koto, the reasoning disappears. The override
  itself is implicit (evidence submission on a gate-failed state) and the event
  log captures the evidence but not why the agent chose to bypass the gate.
  Agents can separately call `koto decisions record`, but nothing connects the
  override to the rationale. Session visualization and human review of agent
  behavior depend on answering "why was this gate overridden?"
goals: |
  Make gate overrides first-class auditable events with mandatory rationale,
  queryable across the full session. Persist enough context for future
  visualization and redo capabilities without building those consumers now.
source_issue: 108
---

# PRD: Override gate rationale

## Status

Draft

## Problem statement

Gate overrides in koto are invisible. When an agent submits evidence on a
gate-failed state, the engine advances the workflow, but no event records that
a gate was bypassed or why the agent chose to proceed despite the failure.

Three things are broken:

1. **Overrides are implicit.** The engine infers override from evidence presence
   on a gate-failed state. There's no explicit "I'm overriding this gate" signal.

2. **Rationale is disconnected.** An agent can call `koto decisions record` to
   log reasoning, but that's a separate operation with no structural link to the
   override. Nothing forces it. The override and the rationale live in different
   events with no connection.

3. **No cross-session query surface.** `koto decisions list` is epoch-scoped
   (current state only). There's no way to ask "show me all overrides in this
   session" without parsing raw JSONL.

This matters because the north star is session visualization: a human reviewer
should be able to see every gate override an agent made, understand the
reasoning, and eventually force a redo when they disagree. Without structured
override data, none of that is possible.

## Goals

- Every gate override produces an auditable event with mandatory rationale
- Override events capture enough context to answer: what gate failed, why it
  failed, and why the agent proceeded anyway
- Override history is queryable across the full session, not just the current
  state epoch
- The data shape supports future visualization and redo consumers without
  requiring schema changes when those features arrive

## User stories

**As a workflow skill author**, I want gate overrides to automatically capture
rationale so I don't have to remember to call `koto decisions record` as a
separate step after overriding.

**As a human reviewer**, I want to query all gate overrides in a session so I
can audit agent behavior and identify questionable bypasses.

**As a template author**, I want override events to include which gate failed
and why so I can diagnose whether my gate conditions are too strict or agents
are bypassing legitimate checks.

**As a future visualization consumer**, I want override events to be
self-contained (gate failure context + rationale + evidence) so I can render an
override timeline without correlating multiple event types.

## Requirements

### Functional

**R1: First-class override event.** When an agent submits evidence on a
gate-blocked state and the evidence resolves a transition, the engine emits a
`GateOverrideRecorded` event in the JSONL log. The event is distinct from
`EvidenceSubmitted` and `DecisionRecorded`.

**R2: Mandatory rationale.** When evidence is submitted on a gate-blocked state,
the CLI requires a rationale string. Evidence submission without rationale on a
gate-blocked state is rejected with a validation error.

**R3: Gate failure context in the override event.** The `GateOverrideRecorded`
event includes: the state name, which gates failed (names and result details),
the rationale string, and the evidence fields that were submitted.

**R4: Override event is self-contained.** A consumer reading a single
`GateOverrideRecorded` event can answer: what state, which gate(s) failed, why
they failed (exit code, timeout, error), what evidence was provided, and why
the agent overrode. No correlation with other events is needed.

**R5: CLI rationale flag.** `koto next` accepts a `--rationale <string>` flag.
The flag is required when the current state is gate-blocked and evidence is
being submitted via `--with-data`. It's ignored (or optional) when the state
isn't gate-blocked.

**R6: Cross-epoch override query.** A `derive_overrides` function returns all
`GateOverrideRecorded` events across the full session, not scoped to the
current epoch. This follows the existing `derive_*` pattern in persistence.rs.

**R7: CLI query surface.** `koto overrides list` returns all override events
for a workflow, formatted as JSON. Supports the "all overrides in session"
query pattern.

**R8: Non-override evidence is unaffected.** Evidence submitted on states where
gates pass (or states without gates) doesn't require rationale and doesn't
produce override events. The override mechanism only triggers when gates have
actually failed.

**R9: Partial gate failure handling.** When a state has multiple gates and some
fail while others pass, the override event lists all failed gates. Evidence
overrides all failed gates simultaneously (no per-gate granularity).

### Non-functional

**R10: Backward compatibility.** Existing workflows without override events
continue to function. The `--rationale` flag is only required when gates are
currently blocked. Old state files without `GateOverrideRecorded` events are
valid.

**R11: Event ordering.** `EvidenceSubmitted` and `GateOverrideRecorded` are
emitted in strict sequence (evidence first, override second) within the same
`koto next` invocation. Sequence numbers preserve ordering.

## Acceptance criteria

- [ ] Submitting evidence on a gate-blocked state without `--rationale` returns
  a validation error (exit code 2)
- [ ] Submitting evidence on a gate-blocked state with `--rationale` emits both
  `EvidenceSubmitted` and `GateOverrideRecorded` events
- [ ] `GateOverrideRecorded` event contains: state, failed gate names, gate
  result details (exit code/timeout/error), rationale string, and evidence fields
- [ ] Submitting evidence on a non-gate-blocked state does not require
  `--rationale` and does not emit `GateOverrideRecorded`
- [ ] `koto overrides list` returns all override events across the full session
  as JSON
- [ ] Override events survive rewind: if state A is overridden, agent advances
  to B, then rewinds to A, the original override event is still in the log and
  visible via `koto overrides list`
- [ ] Re-overriding a state after rewind produces a new, separate override event
- [ ] Existing workflows without any `--rationale` usage continue to work
  (backward compatible on states where gates pass)
- [ ] State with 3 gates where 2 fail: override event lists both failed gates
  with their individual results
- [ ] `--rationale` on a non-blocked state is accepted without error (no-op,
  doesn't produce override event)
- [ ] `--rationale ""` (empty string) on a gate-blocked state returns a
  validation error -- rationale must be non-empty
- [ ] `EvidenceSubmitted` event has a lower sequence number than
  `GateOverrideRecorded` event within the same invocation (R11 ordering)
- [ ] `derive_overrides` returns all `GateOverrideRecorded` events across
  epochs, including events from states that were later rewound past
- [ ] Submitting evidence with `--rationale` on a gate-blocked state where the
  evidence doesn't match any transition does NOT emit `GateOverrideRecorded`
  (override event only on successful transition per D4)

## Out of scope

- **Visualization UI.** This PRD covers the data persistence and query layer.
  Visualization is a future consumer.
- **Redo/rewind triggered by override disagreement.** The override data enables
  this, but the redo mechanism is future work.
- **Evidence verification by koto.** Koto doesn't yet independently verify
  evidence (polling CI, parsing files). When it does, the override concept
  gains sharper meaning. For now, "override" means "agent submitted evidence
  on a gate-failed state."
- **`--to` directed transition tracking.** Directed transitions bypass all
  gates but are a separate mechanism (explicit state jump, no evidence
  submission). Tracking `--to` as an override-like event is deferred.
- **Action skip tracking.** Evidence presence causes default actions to be
  skipped (independent of gate state). Auditing action skips is related but
  distinct work.
- **`required_when` conditional validation.** General conditional field
  requirements in the template schema. The rationale requirement is handled at
  the engine/CLI level, not as schema evolution.
- **Per-gate override granularity.** When multiple gates fail, evidence
  overrides all of them. Selective per-gate override is deferred.

## Known limitations

- **Override detection depends on gate evaluation timing.** Gates are evaluated
  once per `koto next` invocation. If gate state changes between invocations
  (e.g., CI goes green), the second invocation won't see a gate failure and
  won't require rationale. The override event is a point-in-time snapshot.
- **Rationale quality is unvalidated.** The engine requires a non-empty string
  but can't assess whether the rationale actually justifies the override. This
  is a human review concern, not a machine validation concern.
- **No link between override and decision events.** Override rationale and
  `koto decisions record` entries are independent. An agent might record both
  for the same action. Deduplication is a consumer concern.

## Decisions and trade-offs

**D1: Dedicated event type, not reuse of decisions subsystem.** Override
rationale could flow through `DecisionRecorded` (agent-initiated, epoch-scoped)
for consistency. We chose a dedicated `GateOverrideRecorded` event because
overrides are engine-detected (not agent-initiated), need cross-epoch
queryability, and are conceptually distinct from agent deliberation. Mixing them
would complicate queries and blur the semantic boundary.

**D2: Separate `--rationale` flag, not embedded in evidence JSON.** Rationale
could be a reserved field in the evidence payload (e.g., `_rationale`).
We chose a separate CLI flag because it keeps evidence schema clean (no
reserved fields), follows the pattern set by `koto decisions record --rationale`,
and allows independent validation of evidence fields vs. rationale.

**D3: Scope to evidence-based gate overrides only.** The codebase has three
implicit override mechanisms: evidence on gate-failed states, action skipping
via evidence presence, and `--to` directed transitions. We scoped to the first
because it's the primary user need (issue #108), the most common pattern in
workflow skills, and the cleanest to define. Action skipping and `--to` tracking
are noted as future work.

**D4: Override event emitted only on successful transition.** An override event
could be emitted whenever rationale is provided (even if evidence doesn't match
a transition). We chose to emit only when the override succeeds (evidence
resolves a transition) because a failed submission doesn't actually override
anything. The agent will retry with different evidence, producing a new override
event on success.
