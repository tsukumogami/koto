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
  parameter to advance_until_stop. Override neutralizes gate failure
  (gate_failed = false), letting normal transition resolution proceed. A
  purpose-built derive_overrides function provides cross-epoch queries. One
  flag, one event type, one query function -- no template schema changes.
rationale: |
  Direct parameter threading is minimal and matches the existing evidence
  pattern. Neutralizing gate_failed reuses existing transition resolution
  logic with no new code path. This eliminates the template workaround where
  authors add accepts blocks with override enum values on deterministic gate
  states. Purpose-built query follows the derive_visit_counts precedent.
---

# DESIGN: Override gate rationale

## Status

Proposed

## Context and problem statement

The koto engine's advance loop (`advance_until_stop` in `src/engine/advance.rs`)
has no built-in gate override mechanism. Today, template authors work around
this by adding `accepts` blocks with `override` enum values and matching
conditional transitions on deterministic gate states -- boilerplate that
exists solely to give agents a way past a failed gate. Without that
workaround, the agent's only option on a gate-blocked state is `--to`, which
bypasses everything with no audit trail.

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
   fail, the engine should emit `GateOverrideRecorded` and then treat gates
   as passed -- letting normal transition resolution proceed. This is simple:
   the override neutralizes the gate failure, and the existing transition
   logic handles routing.

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

### Decision 2: advance loop override behavior

When `--override-rationale` is present and gates fail, how should the advance
loop behave? The current code has two paths depending on whether the state
has an `accepts` block: with accepts, it falls through to transition
resolution; without accepts, it returns `GateBlocked` immediately. The
override mechanism needs to work identically in both cases.

#### Chosen: neutralize gate failure, let normal transition resolution proceed

When `override_rationale` is `Some` and gates fail, the engine emits
`GateOverrideRecorded` and then sets `gate_failed = false` before calling
`resolve_transition`. This is the same thing as treating the gates as if they
had passed. Normal transition resolution handles routing from there --
unconditional fallbacks fire, conditional transitions match against evidence
if `--with-data` was also provided.

This is simple because it doesn't introduce a new code path. The existing
transition resolution logic already handles every combination of evidence,
conditionals, and fallbacks. The override just removes the gate-failure
block that would have prevented transition resolution from running.

There's no special handling for "states with accepts" vs "states without
accepts." Today, template authors add `accepts` blocks with `override` enum
values on deterministic gate states as a workaround for the lack of an engine
override. With `--override-rationale`, that workaround is unnecessary.
Templates can be simplified to just gates + transitions, and the override
works the same way regardless.

#### Alternatives considered

**Separate override path with explicit target resolution**: treat override as
a distinct code path that resolves transitions differently from the normal
path. Rejected because the existing transition resolution logic already
handles all cases correctly once `gate_failed` is neutralized. Adding a
separate path would duplicate logic and create divergence risk.

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
when gates fail and the override parameter is present, the engine emits a
`GateOverrideRecorded` event and then neutralizes the gate failure
(`gate_failed = false`). Normal transition resolution proceeds as if gates
had passed. There's no branching based on whether the state has an `accepts`
block -- the override behavior is identical in all cases.

This eliminates the template workaround where authors add `accepts` blocks
with `override` enum values on deterministic gate states. Templates can be
simplified to just gates + transitions. The engine handles override.

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
rationale strings are rejected at CLI validation.

### Rationale

These three decisions reinforce each other: the direct parameter approach (D1)
gives the advance loop immediate access to the rationale at the exact point
where gate failure is detected. The neutralize-gate-failure approach (D2)
is simple because it reuses existing transition resolution logic -- no new
code path, no branching on accepts. And the purpose-built query function (D3)
follows the pattern already established by `derive_visit_counts`.

The combination achieves the PRD's key goal -- engine universality (R6) --
without template schema changes. Every gate-blocked state becomes overridable.
Template authors no longer need `accepts` block workarounds on deterministic
gate states. The audit trail is self-contained in a single event type,
queryable across the full session.

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
    - Set `gates_failed = false` (neutralize the failure)
    - Fall through to normal transition resolution -- identical to the
      gates-passed path
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
  |     +-- set gate_failed = false (neutralize failure)
  |     |
  |     v
  |   Normal transition resolution (same as gates-passed path)
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
the override event, set `gate_failed = false`, and let normal transition
resolution proceed. No branching on accepts -- the override neutralizes gate
failure and the existing logic handles everything from there.

Deliverables:
- Modified `advance_until_stop` signature and gate evaluation block
- Override neutralizes `gate_failed` for transition resolution
- Updated all callers to pass `None` for existing code paths
- Unit tests for override on states with and without accepts blocks

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
teach two things: (1) `--override-rationale` is the primary override mechanism,
and (2) deterministic gate states no longer need `accepts` blocks with
`override` enum values -- that pattern was a workaround the engine now handles
natively.

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
- States without any resolvable transition (no unconditional fallback, no
  matching conditional) still can't advance -- but this is a template
  validation concern, not an override limitation
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
- States without resolvable transitions are a template validation concern.
  A future template compiler check could warn about non-terminal states with
  no progression path
- For the evidence gap in override events: the PRD scoped this as acceptable
  (R4 says "self-contained" for rationale + gate context, not evidence). Future
  work can add an `evidence_ref` field if needed
