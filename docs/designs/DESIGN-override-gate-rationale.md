---
status: Proposed
upstream: docs/prds/PRD-override-gate-rationale.md
problem: |
  Gate overrides in koto's advance loop are implicit and untracked. The engine
  needs a new event type, a CLI flag that threads rationale into the advance
  path, and a cross-epoch query function -- all without breaking existing
  evidence submission or template schemas.
decision: |
  Add --override-rationale flag to koto next, threading rationale as a direct
  parameter to advance_until_stop. Gate-only states use unconditional fallback
  transitions as override targets. A purpose-built derive_overrides function
  provides cross-epoch queries. One flag, one event type, one query function.
rationale: |
  Direct parameter threading is minimal and matches the existing evidence
  pattern. Unconditional fallbacks handle gate-only states without schema
  changes. Purpose-built query follows the derive_visit_counts precedent.
  All three decisions reinforce engine universality without template evolution.
---

# DESIGN: Override gate rationale

## Status

Proposed

## Context and problem statement

The koto engine's advance loop (`advance_until_stop` in `src/engine/advance.rs`)
handles gate failures through a fallback path: if gates fail and the state has
an `accepts` block, the engine returns `EvidenceRequired` instead of
`GateBlocked`, letting the agent submit evidence to resolve a conditional
transition. This implicit override works but produces no audit trail.

The technical challenge has four parts:

1. **New event type in the persistence layer.** `GateOverrideRecorded` must be
   added to `EventPayload` in `src/engine/types.rs`. The JSONL format uses
   untagged serialization with a `type` discriminant string, so the new variant
   needs a `type_name()` mapping and compatible deserialization. The event must
   carry gate failure context (from `BTreeMap<String, GateResult>` already
   available in the advance loop) and the rationale string.

2. **CLI flag threading.** A new `--override-rationale <string>` flag on
   `koto next` must reach the advance loop. Today, evidence flows through
   `--with-data` -> `validate_evidence` -> `EvidenceSubmitted` event ->
   `derive_evidence` -> `advance_until_stop`. The override flag needs a
   parallel path that doesn't go through evidence validation (it's not evidence)
   but reaches the point where gate failure is detected.

3. **Advance loop changes.** When `--override-rationale` is present and gates
   fail, the engine should bypass the gate-blocked stop reason, emit
   `GateOverrideRecorded`, and continue advancing. Today, gate-only states
   (no `accepts` block) return `GateBlocked` with no override path. The
   `--override-rationale` flag must work on these states too.

4. **Cross-epoch query function.** Existing `derive_*` functions in
   `src/engine/persistence.rs` are epoch-scoped (they filter events after the
   most recent state-changing event). `derive_overrides` needs to scan the full
   event log, which is a different pattern. Plus a `koto overrides list` CLI
   command to expose it.

## Decision drivers

- **Audit completeness**: override events must be self-contained for future
  visualization consumers (no cross-event correlation needed)
- **Backward compatibility**: existing `--with-data '{"status": "override"}'`
  patterns must keep working as plain evidence submission
- **Engine universality**: every gate-blocked state must be overridable,
  regardless of whether the template has an `accepts` block
- **Minimal surface area**: one flag (`--override-rationale`), one new event
  type, one new query function -- no additional template schema changes
- **Event ordering**: when combined with `--with-data`, evidence and override
  events must have deterministic sequence ordering
- **Forward compatibility**: the event shape should support future redo/replay
  without schema migration

## Considered options

### Decision 1: CLI flag threading

The `--override-rationale` flag value needs to reach the gate-failure detection
point inside `advance_until_stop`. Today, evidence flows through a validated
path (`--with-data` -> `validate_evidence` -> `EvidenceSubmitted` ->
`derive_evidence` -> advance loop parameter). Override rationale shouldn't flow
through evidence validation -- it's not evidence, and it shouldn't require an
`accepts` block to exist.

#### Chosen: direct parameter to advance_until_stop

Add `override_rationale: Option<&str>` as a parameter to `advance_until_stop`.
The CLI handler parses `--override-rationale`, passes the value through to the
advance function. Inside the loop, when gates fail and the parameter is `Some`,
the engine emits `GateOverrideRecorded` and continues advancing instead of
stopping.

This matches the existing evidence parameter pattern -- evidence is already
passed directly to the advance function. The parameter defaults to `None` for
all existing call sites, so backward compatibility is automatic. Event ordering
is deterministic because the override event is emitted during advancement (not
before it), after any `EvidenceSubmitted` from `--with-data`.

Key assumption: the signature change to `advance_until_stop` is acceptable. The
function already has 7+ parameters. One more `Option` parameter is standard
Rust.

#### Alternatives considered

**Pre-advance event with rationale**: append a `GateOverrideRequested` event
before calling `advance_until_stop`, then have the loop look backward in the
event log to find the rationale. Rejected because it introduces a two-phase
pattern where the intent event can be orphaned if gates don't actually fail,
and log correlation adds fragility.

**Context struct**: wrap evidence and rationale in an `AdvanceContext` struct
for extensibility. Rejected because it's premature abstraction -- only two
fields need threading today, and refactoring to a struct later is trivial if
more fields accumulate.

### Decision 2: advance loop override on gate-only states

States with gates and an `accepts` block already have an override path: the
engine falls through to transition resolution and the agent provides evidence.
But states with gates and NO `accepts` block return `GateBlocked` immediately
with no forward path. `--override-rationale` must work on these states too
(PRD R6). The question is: where does the engine transition to?

#### Chosen: use unconditional fallback transition

When gates fail on a gate-only state and `--override-rationale` is present,
check for an unconditional transition (one with no `when` condition). If found,
use it as the override target. If no unconditional transition exists, return an
error -- the state has no valid progression path.

This requires ~5-10 lines in the gate evaluation block. The existing
`resolve_transition` function already uses unconditional fallbacks when no
conditional matches, so this reuses proven semantics. Analysis of test fixtures
shows all gate-having states already have unconditional fallback transitions,
making this the de facto standard pattern.

Key assumption: templates with gates typically include an unconditional
transition as a natural progression path. Gate-only states without any
unconditional transition are edge cases that can't be overridden (the engine
doesn't know where to go).

#### Alternatives considered

**Explicit override_target annotation**: add an `override_target` field to the
template state schema. Template authors declare where overrides should go.
Rejected because it adds schema complexity for a case that unconditional
fallbacks already handle. Kept as a backup if gate-only states become common
enough to warrant explicit author intent.

**Forbid gate-only states via validation**: require every state with gates to
have an `accepts` block. Rejected because it blocks valid patterns like
informational gates and compliance checkpoints that don't need user override.

### Decision 3: cross-epoch query pattern

Existing `derive_evidence` and `derive_decisions` are epoch-scoped -- they
filter events after the most recent state-changing event. Override queries need
the full session log ("show all overrides"). `derive_visit_counts` is the
precedent for cross-epoch queries: it iterates the entire event log without
epoch filtering.

#### Chosen: purpose-built derive_overrides function

Add `derive_overrides(events: &[Event]) -> Vec<OverrideRecord>` following the
`derive_visit_counts` pattern. It scans the full event log, filters for
`GateOverrideRecorded` events, and returns structured records. The CLI exposes
it via `koto overrides list`.

This is the simplest option that follows existing patterns. No new abstractions,
no optional parameters, no generic filter framework. If future cross-epoch
queries are needed, they'll get their own purpose-built functions, just like
`derive_visit_counts` did.

#### Alternatives considered

**Generic cross-epoch filter function**: create a reusable closure-based filter
(`derive_events_all(events, |e| matches!(e, ...))`) for any event type.
Rejected as premature abstraction -- only one cross-epoch query is needed today,
and the generic pattern would be inconsistent with the rest of the `derive_*`
API.

**Optional epoch parameter**: add a `cross_epoch: bool` parameter to control
scoping. Rejected because overrides are inherently cross-epoch audit events.
Offering epoch-scoped behavior adds confusion ("why would I want only current-
epoch overrides?") with no use case.

## Decision outcome

### Summary

The override feature adds one CLI flag (`--override-rationale`), one event type
(`GateOverrideRecorded`), and one query function (`derive_overrides`) to koto.

The `--override-rationale` value threads from CLI argument parsing through to
`advance_until_stop` as an `Option<&str>` parameter. Inside the advance loop,
when gates fail on any state -- whether it has an `accepts` block or not -- the
engine checks for the override parameter. If present, it emits a
`GateOverrideRecorded` event carrying the failed gate names and results, the
rationale string, and the state name. Then instead of stopping with
`GateBlocked`, it continues advancing using the unconditional fallback
transition (for gate-only states) or the normal evidence-based transition
resolution (for states with `accepts` blocks).

When `--override-rationale` is combined with `--with-data`, evidence is
submitted and validated first (`EvidenceSubmitted` event), then the advance
loop runs. If gates fail during advancement, the override event is emitted
with a higher sequence number than the evidence event, preserving strict
ordering.

The `derive_overrides` function scans the full JSONL event log (no epoch
scoping) and returns all `GateOverrideRecorded` events. `koto overrides list`
exposes this to the CLI. Override events persist across rewinds -- they're
immutable log entries, not epoch-scoped state.

Edge cases: `--override-rationale` on a non-blocked state is a no-op. Empty
rationale strings are rejected at CLI validation. Gate-only states without
an unconditional fallback transition can't be overridden (no valid target
exists).

### Rationale

These three decisions reinforce each other: the direct parameter approach (D1)
gives the advance loop immediate access to the rationale at the exact point
where gate failure is detected, which is where it needs to decide whether to
bypass (D2) and emit the event. The unconditional fallback strategy (D2) works
because the advance loop already has this transition resolution logic -- the
override just unblocks it from firing on gate-only states. And the purpose-built
query function (D3) follows the pattern already established by
`derive_visit_counts`, keeping the persistence layer consistent.

The combination achieves the PRD's key goal -- engine universality (R6) --
without template schema changes. Every gate-blocked state that has a natural
progression path becomes overridable. The audit trail is self-contained in a
single event type, queryable across the full session.

## Solution architecture

### Overview

The feature touches five layers of the codebase: CLI argument parsing, the
advance loop, the event/persistence layer, a new CLI subcommand, and
caller-facing documentation (AGENTS.md, koto.mdc, cli-usage.md, the
koto-author skill). Data flows from the `--override-rationale` flag through
the advance loop, where it's emitted as a `GateOverrideRecorded` event
alongside gate failure context. A separate query path reads the full event log
to surface overrides.

### Components

**CLI layer** (`src/cli/mod.rs`, `cmd/koto/`)
- Parse `--override-rationale <string>` on `koto next`
- Validate: non-empty string, subject to `MAX_WITH_DATA_BYTES` size limit
  (same 1MB cap as `--with-data`), mutually exclusive with `--to`
- Pass value to the advance handler as `Option<&str>`
- New `koto overrides list <name>` subcommand

**Event types** (`src/engine/types.rs`)
- New `GateOverrideRecorded` variant on `EventPayload`:
  ```
  GateOverrideRecorded {
      state: String,
      gates_failed: BTreeMap<String, SerializableGateResult>,
      rationale: String,
  }
  ```
- `GateResult` doesn't derive `Serialize`/`Deserialize`, so the event payload
  uses a purpose-built `SerializableGateResult` type (similar to how
  `blocking_conditions_from_gates` in `src/cli/next_types.rs` converts
  `GateResult` into `BlockingCondition` for JSON output). This keeps gate
  evaluation free of serialization concerns and gives the event its own stable
  schema.
- `type_name()` returns `"gate_override_recorded"`
- Deserialization via the existing untagged discriminant pattern (manual match
  arm in the `Event` deserializer at `types.rs:156`)

**Advance loop** (`src/engine/advance.rs`)
- New parameter: `override_rationale: Option<&str>` on `advance_until_stop`
- At the gate evaluation block (~line 295-315):
  - If gates fail AND `override_rationale` is `Some`:
    - Emit `GateOverrideRecorded` event via `append_event`
    - For states with `accepts`: fall through to transition resolution
      (existing path, `gates_failed=true`)
    - For states without `accepts`: resolve unconditional fallback transition
      directly, bypassing the current `GateBlocked` return
  - If gates fail AND `override_rationale` is `None`: existing behavior
    (return `GateBlocked` or fall through to `EvidenceRequired`)

**Persistence layer** (`src/engine/persistence.rs`)
- New `derive_overrides(events: &[Event]) -> Vec<OverrideRecord>`:
  - Scans full event log (no epoch filtering)
  - Filters for `GateOverrideRecorded` events
  - Returns structured records with state, gates, rationale, timestamp

### Key interfaces

**CLI flag**:
```
koto next <name> --override-rationale "reason"
koto next <name> --override-rationale "reason" --with-data '{"field": "value"}'
```

`--override-rationale` is mutually exclusive with `--to` (directed transitions
have their own semantics). It can combine with `--with-data` for states that
need both gate bypass and evidence.

**Event payload** (JSONL):
```json
{
  "seq": 8,
  "timestamp": "2026-03-30T14:22:00Z",
  "type": "gate_override_recorded",
  "state": "verify",
  "gates_failed": {
    "ci_check": {"status": "failed", "exit_code": 1}
  },
  "rationale": "CI failure is flaky test, unrelated to this change"
}
```

**Query output** (`koto overrides list`):
```json
{
  "overrides": [
    {
      "state": "verify",
      "gates_failed": {"ci_check": {"result": "failed", "exit_code": 1}},
      "rationale": "CI failure is flaky test, unrelated to this change",
      "seq": 8,
      "timestamp": "2026-03-30T14:22:00Z"
    }
  ]
}
```

### Data flow

```
CLI: --override-rationale "reason"
  |
  v
CLI handler: parse flag, validate non-empty
  |
  +-- if --with-data also present: validate evidence, append EvidenceSubmitted
  |
  v
advance_until_stop(override_rationale: Some("reason"), ...)
  |
  v
Gate evaluation: gates fail
  |
  +-- override_rationale is Some:
  |     |
  |     +-- append GateOverrideRecorded event
  |     |
  |     +-- state has accepts? -> fall through to transition resolution
  |     +-- state has no accepts? -> resolve unconditional fallback
  |     |
  |     v
  |   Continue advancing (transition fires, loop continues)
  |
  +-- override_rationale is None:
        |
        +-- existing behavior (GateBlocked or EvidenceRequired)
```

## Implementation approach

### Phase 1: Event type and persistence

Add `GateOverrideRecorded` to `EventPayload` in `src/engine/types.rs`.
Implement serialization (`type_name`), deserialization, and round-trip tests.
Add `derive_overrides` to `src/engine/persistence.rs` with unit tests.

Deliverables:
- `GateOverrideRecorded` variant with state, gates_failed, rationale fields
- `derive_overrides` function returning `Vec<OverrideRecord>`
- Unit tests for serialization, deserialization, and derive function

### Phase 2: Advance loop changes

Add `override_rationale: Option<&str>` parameter to `advance_until_stop`.
Implement the gate-bypass logic: when override is present and gates fail, emit
the override event and continue advancing. Handle both states with `accepts`
(fall through to existing transition resolution) and states without `accepts`
(resolve unconditional fallback directly).

Deliverables:
- Modified `advance_until_stop` signature and gate evaluation block
- Gate-only state override via unconditional fallback
- Updated all callers to pass `None` for existing code paths
- Unit tests for override path on both state types

### Phase 3: CLI integration

Parse `--override-rationale` in the `koto next` handler. Thread the value to
`advance_until_stop`. Validate: non-empty, mutually exclusive with `--to`.
Add `koto overrides list` subcommand calling `derive_overrides`.

Deliverables:
- `--override-rationale` flag parsing and validation
- Threading to advance loop
- `koto overrides list` subcommand
- Integration tests for the full flow
- Functional tests (Cucumber features)

### Phase 4: Documentation and skill updates

Update all caller-facing documentation and the koto-author skill to reflect
the new override mechanism. Templates that currently use `override` as an
evidence enum value still work (backward compatible), but documentation should
teach the `--override-rationale` flag as the primary override mechanism.

Deliverables:
- `plugins/koto-skills/AGENTS.md`: add `--override-rationale` flag to command
  reference, document `koto overrides list`, add override to the agent dispatch
  loop ("if gate_blocked and override is appropriate, use --override-rationale")
- `plugins/koto-skills/.cursor/rules/koto.mdc`: add override flag to response
  handling examples
- `docs/guides/cli-usage.md`: add `--override-rationale` to koto next docs,
  document `koto overrides list`, add `GateOverrideRecorded` to event types
- `docs/guides/custom-skill-authoring.md`: update gates section to explain the
  override mechanism and when skill authors should expect agents to use it
- `plugins/koto-skills/skills/koto-author/references/template-format.md`:
  update the gates + evidence routing section to mention that
  `--override-rationale` provides a universal override path, reducing the need
  for explicit `override` enum values in accepts blocks
- `plugins/koto-skills/skills/koto-author/SKILL.md`: update the execution loop
  section to reference `--override-rationale` as the gate bypass mechanism
- Template fixtures: add a gate-only-with-fallback fixture for testing override
  on states without accepts blocks

## Security considerations

This feature adds a new event type to the JSONL log and a new CLI flag.

- **Rationale injection**: the rationale is a free-form string stored as-is in
  the event log. Unlike evidence fields (schema-validated, typed), rationale is
  unvalidated text with a wider injection surface. The HTML export path
  (`src/export/html.rs`) must escape rationale strings before rendering.
  Mitigation: apply the same `MAX_WITH_DATA_BYTES` size limit (1MB) to prevent
  unbounded input. Serde's JSON serialization handles escaping for the JSONL
  log.
- **Gate bypass authorization**: `--override-rationale` lets any caller bypass
  gates. This is by design -- koto doesn't have an authorization model. The
  rationale creates an audit trail, not an access control mechanism.
- **Caller identity**: override events don't capture who performed the
  override. For the stated goal of human audit ("why was this gate
  overridden?"), not answering "by whom" is a gap. This is a pre-existing
  limitation across all event types (no event carries caller identity), not
  specific to overrides. Documented as a known limitation.
- **Event log integrity**: override events are appended to the same JSONL file
  as all other events. No new file access patterns. The file is written
  atomically via the existing persistence layer.

## Consequences

### Positive

- Every gate-blocked state becomes overridable with an audit trail, closing the
  gap identified in issue #108
- Template authors don't need to change anything -- override is an engine
  capability, not a schema concern
- The override event is self-contained, enabling future visualization without
  cross-event correlation
- Follows existing patterns (parameter passing, derive_* functions, event types)
  so the codebase stays consistent

### Negative

- `advance_until_stop` gains another parameter, making an already-long
  signature longer
- Gate-only states without unconditional fallback transitions can't be
  overridden (the engine doesn't know where to go)
- The override event doesn't include evidence fields when `--with-data` isn't
  used, so visualization consumers may need to check the preceding
  `EvidenceSubmitted` event for full context in combined-flag cases
- No caller identity in events. Override events don't record who performed the
  override. This is a pre-existing gap across all koto event types, not
  specific to this feature. Future work on caller identity would benefit all
  events

### Mitigations

- If the parameter list becomes unwieldy, refactoring to a context struct is
  straightforward (the rejected Option C from Decision 1)
- Gate-only states without fallbacks are an edge case. If they become common,
  the explicit `override_target` annotation (rejected Option B from Decision 2)
  can be added without breaking changes
- For the evidence gap in override events: the PRD scoped this as acceptable
  (R4 says "self-contained" for rationale + gate context, not evidence). Future
  work can add an `evidence_ref` field if needed
