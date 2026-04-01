---
status: Proposed
upstream: docs/prds/PRD-gate-transition-contract.md
problem: |
  Gate evaluation is one-directional: the engine runs the gate and the result is final.
  When a gate fails, an agent's only options are to wait or to work around the failure by
  submitting an override enum value via an accepts block. The workaround has no audit trail
  -- the engine can't distinguish "genuinely resolved" from "bypassed without explanation."
  Session replay and human review can't reconstruct which gates were overridden, when, or
  with what justification.
decision: |
  Add a first-class override mechanism: agents call `koto overrides record` to substitute a
  gate's output with default or explicit values, attaching mandatory rationale. Override events
  are sticky within the current epoch and are read by the advance loop during gate evaluation,
  which skips gate execution for any gate with an active override and injects the override value
  directly into the gates.* evidence map. `koto overrides list` queries the override history
  across the full session. A new GateEvaluated event makes last-known gate output queryable at
  override-record time, satisfying the audit requirement without re-running gates.
rationale: |
  Skipping gate execution on override (vs. re-executing then substituting) matches the
  koto decisions record precedent, avoids latency and non-idempotent side effects, and produces
  a stable audit trail -- actual_output is captured once at override-record time rather than
  differing between koto next calls. Treating overridden gates as unconditional passes (rather
  than re-evaluating override_applied against the pass condition) lets template authors use
  non-passing override values to drive routing, which the PRD explicitly requires. All five
  design decisions compose without conflict; the only scope addition beyond the roadmap
  description is GateEvaluated as the anchor event for actual_output capture.
---

# DESIGN: Gate override mechanism

## Status

Proposed

## Context and Problem Statement

Gate evaluation is one-directional: the engine runs the gate and the result is final. When a
gate fails, an agent's only options are to wait (if the gate condition may change) or to work
around the failure by submitting an `override` enum value via an `accepts` block. The workaround
has no audit trail -- the engine can't distinguish "the agent genuinely resolved the blocking
condition" from "the agent bypassed the gate without explaining why." Session replay and human
review of a completed workflow can't reconstruct which gates were overridden, when, or with what
justification.

This is Feature 2 of the gate-transition contract roadmap. Feature 1 (merged in #120) gives
gates structured output. Feature 2 adds a first-class override mechanism: agents call
`koto overrides record` to substitute a gate's output with default or explicit values, attaching
mandatory rationale. Override events are sticky within the current epoch and read by the advance
loop during gate evaluation, so subsequent `koto next` calls see the substituted data in the
`gates.*` evidence map. `koto overrides list` queries the override history across the session.

## Decision Drivers

- Override rationale must be captured in the event log and queryable by `koto overrides list`
  (R6, R8). Silent gate bypasses must not be possible.
- Override defaults per gate type (built-in) and per gate instance (`override_default` in
  template) must let template authors control what "override" means for each gate without
  requiring agents to supply `--with-data` (R4). When `--with-data` is supplied, values are
  validated against the gate type's schema (R5).
- Each `koto overrides record` call targets one gate with one rationale (R5a). Multiple gates
  in a state each need their own `overrides record` call.
- Override events must be sticky within the current epoch -- they persist until the state
  transitions -- and accumulate across multiple `overrides record` calls in the same epoch (R5).
- The `gates` evidence key must be reserved: agents may not submit `gates.*` keys via
  `koto next --with-data`, preventing injection of fake gate data (R7).
- Rationale and `--with-data` payloads are subject to the same 1MB size limit as other
  `--with-data` payloads (R12).
- The mechanism mirrors `koto decisions record` / `koto decisions list` to keep the CLI surface
  consistent (R5).

## Considered Options

### D1: Override execution model

**Chosen: Skip execution (Option A).** When `derive_overrides` returns an override for a gate in
the current epoch, the advance loop skips that gate entirely and injects `override_applied`
directly into `gate_evidence_map`. No gate re-execution happens during `koto next`.

**Rejected: Execute then substitute (Option B).** Re-executing the gate before substituting its
output adds latency, risks non-idempotent side effects (a gate that moves a Jira ticket or
triggers CI would run twice), and produces a misleading audit trail when the gate condition has
changed since the override was recorded. The `koto decisions record` precedent is a command that
records without running logic; overrides should follow the same pattern.

The key insight: "actual gate output" in R6 means the output from the blocking run the agent
observed, not a re-run triggered by override application. This output is captured at
`koto overrides record` time from the event log.

### D2: override_default declaration

**Chosen: Optional field on Gate struct (Option A).** `override_default: Option<serde_json::Value>`
is added directly to the `Gate` struct in `template/types.rs`. Template authors declare it inline,
adjacent to the rest of the gate's configuration. Built-in type defaults exist in code as
constants: command → `{"exit_code": 0, "error": ""}`, context-exists → `{"exists": true, "error":
""}`, context-matches → `{"matches": true, "error": ""}`. Resolution order: instance
`override_default` → built-in type default. The call to `koto overrides record` without
`--with-data` never fails for any gate of a known type.

**Rejected: Separate override_defaults block at state level (Option B).** A parallel map at the
state level requires cross-validating two independent keys at compile time, and the `agent_actionable`
flag check must reach back to the parent state's map rather than staying at the gate level. No
reuse benefit -- `Gate` is not a shared type.

**Rejected: Type defaults only, no instance override_default (Option C).** Directly contradicts
PRD R4, which requires template authors to declare a custom `override_default` per gate to route
overrides to a different transition.

### D3: Override event persistence and advance loop integration

**GateOverrideRecorded fields (Chosen: full audit, Option B).** Six fields: `state`, `gate`,
`rationale`, `override_applied`, `actual_output`, `timestamp`. `actual_output` is required by D1
and PRD R6. `state` is required for `koto overrides list` self-description -- every other
epoch-anchored event payload carries `state`, and omitting it forces `derive_overrides_all` to
reconstruct state by scanning surrounding transition events.

**Advance loop integration (Chosen: pre-check, Option A).** `derive_overrides` is called once
before gate iteration. Gates with an override are skipped and their `override_applied` is injected
directly; the remainder are passed to `evaluate_gates` unchanged. This leaves the `evaluate_gates`
closure signature intact and makes the skip-execution logic explicit at a single call site.

**any_failed semantics (Chosen: unconditional pass, Option B).** Any gate with an override in
the current epoch contributes `GateOutcome::Passed` unconditionally. The override value still
enters `gate_evidence_map` for transition routing. Re-evaluating `override_applied` against the
pass condition (Option C) creates a trap: a template author who uses `override_default:
{exit_code: 1}` to route to a `manual_review` transition would find the gate still blocking,
contradicting PRD R4's custom-override-default requirement.

**agent_actionable (Chosen: instance OR type default, Option A).** The flag is set true when
`g.override_default.is_some() || built_in_default_exists(&g.gate_type)`. Since built-in defaults
exist for all three known gate types, this is effectively always true at runtime. The check encodes
the precise semantic: "a default is available, so the agent can call `koto overrides record`
without needing to know the gate's schema."

### D4: koto overrides CLI structure and derive_overrides scope

**CLI (Chosen: separate flags, Option A).** `koto overrides record <name> --gate <gate_name>
--rationale "reason" [--with-data '...']`. Separate `--gate` and `--rationale` flags give Clap
field-presence validation at parse time with native error messages, matching the PRD's examples
verbatim across six independent usage scenarios.

**Scope (Chosen: two functions, Option A).** `derive_overrides` mirrors `derive_decisions` exactly
(current epoch, used by advance loop). `derive_overrides_all` returns all `GateOverrideRecorded`
events across the full session (used by `koto overrides list`). R8 requires cross-epoch visibility
for audit; the advance loop must not see overrides from previous epochs. Separating the two
callers' needs into two functions keeps each caller's query correct without forcing post-filtering.

### D5: gates namespace reservation enforcement

**Chosen: Both CLI and engine layers (Option C).** The CLI layer (`handle_next`) checks for a
top-level `"gates"` key after JSON parse and before `validate_evidence`, returning `InvalidSubmission`
with a clear message. The engine layer (`advance_until_stop`) converts the existing silent overwrite
to an explicit assertion (warning in release, panic in debug). The engine check becomes effectively
dead code once the CLI check lands, but it documents the invariant and handles non-CLI writes to
the state file.

Context store (`koto context set`) is excluded: context and evidence are structurally separate
namespaces with no collision risk.

## Decision Outcome

The five decisions compose into a coherent design with one scope addition beyond the roadmap
description: a new `GateEvaluated` event type must be added so `koto overrides record` can read
last-known gate output from the event log.

The full override flow:

1. The agent calls `koto next`, which returns a `gate_blocked` response. The advance loop has
   emitted a `GateEvaluated` event for each gate that ran, recording its structured output.
2. The agent calls `koto overrides record myflow --gate ci_check --rationale "..."`. The handler
   reads the most recent `GateEvaluated` event for `ci_check` in the current epoch, uses its
   output as `actual_output`, and appends a `GateOverrideRecorded` event.
3. On the next `koto next`, `derive_overrides` returns the active override for `ci_check`. The
   advance loop skips executing that gate, inserts a synthetic `StructuredGateResult` with
   `GateOutcome::Passed`, and injects `override_applied` into `gate_evidence_map`. The remaining
   gates run normally. If all gates now pass, the workflow advances.
4. `koto overrides list myflow` calls `derive_overrides_all` and returns all overrides across
   the full session with their rationale, override values, and actual output.

## Solution Architecture

### New types and events

**`GateEvaluated` event payload** (in `src/engine/types.rs`):

```rust
GateEvaluated {
    state: String,
    gate: String,
    output: serde_json::Value,   // structured output per gate type schema
    outcome: String,             // "passed", "failed", "error"
    timestamp: String,
}
```

Emitted by the advance loop for every gate that executes. Provides the anchor for
`actual_output` capture in `koto overrides record`.

**`GateOverrideRecorded` event payload** (in `src/engine/types.rs`):

```rust
GateOverrideRecorded {
    state: String,
    gate: String,
    rationale: String,
    override_applied: serde_json::Value,
    actual_output: serde_json::Value,
    timestamp: String,
}
```

Appended by `koto overrides record`. `actual_output` is read from the most recent
`GateEvaluated` event for the named gate in the current epoch.

**`override_default` field on `Gate` struct** (in `src/template/types.rs`):

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub override_default: Option<serde_json::Value>,
```

Optional; follows the same serde pattern as other optional Gate fields.

**Built-in defaults** (in `src/engine/gate.rs` or a new `src/engine/override_defaults.rs`):

```rust
pub fn built_in_default(gate_type: &str) -> Option<serde_json::Value> {
    match gate_type {
        "command"        => Some(json!({"exit_code": 0, "error": ""})),
        "context-exists" => Some(json!({"exists": true, "error": ""})),
        "context-matches"=> Some(json!({"matches": true, "error": ""})),
        _                => None,
    }
}
```

### Advance loop changes (src/engine/advance.rs)

The gate evaluation block gains a pre-check for epoch overrides:

```
1. Call derive_overrides(all_events) → epoch_overrides: BTreeMap<String, Value>
2. For each gate in template_state.gates:
   a. If gate name is in epoch_overrides:
      - Insert override_applied into gate_evidence_map
      - Insert synthetic StructuredGateResult { outcome: Passed, output: override_applied } into gate_results
   b. Otherwise: add to gates_to_evaluate
3. Call evaluate_gates(&gates_to_evaluate) for non-overridden gates
4. Merge live results into gate_evidence_map and gate_results
5. Compute any_failed from gate_results (overridden gates have Passed, don't contribute)
6. blocking_conditions_from_gates uses gate_results and gate defs for agent_actionable
```

The advance loop also emits `GateEvaluated` for each gate that runs (step 3 above), before
returning the advance result.

### Persistence changes (src/engine/persistence.rs)

Two new functions mirroring the `derive_decisions` pattern:

- `derive_overrides(events: &[Event]) -> Vec<&Event>` — epoch-scoped, mirrors `derive_decisions`
  exactly (epoch boundary + state-field filter). Used by the advance loop.
- `derive_overrides_all(events: &[Event]) -> Vec<&Event>` — returns all `GateOverrideRecorded`
  events regardless of epoch. Used by `koto overrides list`.

Helper function for actual_output capture:

- `derive_last_gate_evaluated(events: &[Event], gate: &str) -> Option<serde_json::Value>` —
  scans the current epoch for the most recent `GateEvaluated` event for the named gate, returns
  its `output` field.

### CLI changes (src/cli/mod.rs and src/cli/overrides.rs)

New `OverridesSubcommand` enum (Clap):

```rust
#[derive(Subcommand)]
pub enum OverridesSubcommand {
    Record {
        name: String,
        #[arg(long)]
        gate: String,
        #[arg(long)]
        rationale: String,
        #[arg(long)]
        with_data: Option<String>,
    },
    List {
        name: String,
    },
}
```

`handle_overrides_record`:
1. Derive current state from event log; verify the named gate exists in `template_state.gates`.
   Return an error if the gate is not present in the current state (phantom gate prevention).
2. Parse and validate `--with-data` JSON if provided (key presence, value types, no extra keys,
   size ≤ 1MB per R5 and R12).
3. Resolve override value: `--with-data` → instance `override_default` → built-in type default.
4. Validate rationale length ≤ 1MB (R12).
5. Read `actual_output` via `derive_last_gate_evaluated`.
6. Append `GateOverrideRecorded` event.

`handle_overrides_list`:
1. Call `derive_overrides_all`.
2. Return JSON: `{"state": "...", "overrides": {"count": N, "items": [...]}}`

Each item: `{"state", "gate", "rationale", "override_applied", "actual_output", "timestamp"}`.

### gates namespace reservation (src/cli/mod.rs)

In `handle_next`, after JSON parse, before `validate_evidence`:

```rust
if let Some(obj) = data.as_object() {
    if obj.contains_key(GATES_EVIDENCE_NAMESPACE) {
        return Err(KotoError::InvalidSubmission {
            reason: format!(
                "\"{}\" is a reserved field; agent submissions must not include this key",
                GATES_EVIDENCE_NAMESPACE
            ),
        });
    }
}
```

In `advance_until_stop`, convert the silent overwrite to an explicit assertion:

```rust
debug_assert!(
    !current_evidence.contains_key(GATES_EVIDENCE_NAMESPACE),
    "gates key found in current_evidence; CLI reservation check should have prevented this"
);
```

## Implementation Approach

### Phase 1: Event infrastructure

1. Add `GateEvaluated` and `GateOverrideRecorded` variants to `EventPayload` in `src/engine/types.rs`.
2. Implement `derive_overrides`, `derive_overrides_all`, and `derive_last_gate_evaluated` in
   `src/engine/persistence.rs`.
3. Add `built_in_default` function.
4. Unit tests for the new persistence functions.

### Phase 2: Advance loop integration

1. Add `override_default` field to `Gate` struct in `src/template/types.rs`.
2. Modify the advance loop in `src/engine/advance.rs` to:
   - Emit `GateEvaluated` events after each gate evaluation.
   - Pre-check `derive_overrides` before gate iteration.
   - Build synthetic `StructuredGateResult` entries for overridden gates.
   - Update `any_failed` and `blocking_conditions` computation.
3. Update `blocking_conditions_from_gates` in `src/cli/next_types.rs` to set `agent_actionable`
   using the instance + type default check.
4. Unit tests for override injection, any_failed semantics, and agent_actionable.

### Phase 3: CLI and namespace enforcement

1. Add `OverridesSubcommand` to the Clap command tree and implement `handle_overrides_record`
   and `handle_overrides_list` in `src/cli/mod.rs` (or a new `src/cli/overrides.rs`).
2. Add the `gates` namespace pre-check in `handle_next`.
3. Add the `debug_assert` in `advance_until_stop`.
4. Functional tests covering the full override flow, including `koto overrides list` output
   and the namespace rejection error.

## Security Considerations

### Rationale injection

`--rationale` is a free-text string stored verbatim in the event log. The 1MB size limit (R12)
prevents storage exhaustion. The rationale is never executed or evaluated as code -- it is only
read back by `koto overrides list` and serialized as a JSON string field. No sanitization beyond
the size check is required.

### Size limit scope

`--rationale` and `--with-data` are each limited to 1MB independently (R12 applies per payload,
consistent with other `--with-data` size limits in the codebase). This allows up to ~2MB per
`GateOverrideRecorded` event in the worst case (maximum rationale and maximum override data), which
is acceptable given these are rare human-authored events. The limit is checked before appending
the event.

### gates namespace reservation

An agent submitting `{"gates": {...}}` via `koto next --with-data` gains no ability to override
gate output in the advance loop, because the engine merge unconditionally overwrites any
agent-provided `"gates"` key. Feature 2 adds an explicit CLI-layer rejection so the agent
receives a clear error rather than silent discard. The defense-in-depth engine assertion catches
any state file written outside the CLI. Neither path allows fake gate data to influence routing.

An agent submitting a flat key `"gates.ci_check"` (with the literal dot) passes the top-level
reservation check, but this causes no security issue: the `when` clause resolver uses dot-path
traversal on nested JSON objects, not flat-key lookup. A flat key `"gates.ci_check"` in the
evidence map cannot be reached by `resolve_transition` traversing `gates → ci_check`. The
reservation ensures the nested `{"gates": {...}}` structure is never present in agent evidence;
flat keys with embedded dots are distinct and unreachable via the dot-path traversal model.

### --with-data schema validation

Override values provided via `--with-data` are validated against the gate type's schema (R5),
using the same type-checking logic as evidence validation: key presence, value types (string,
integer, boolean), and no extra keys. An agent cannot substitute a value whose field types differ
from the gate schema. Values that fail schema validation are rejected before the
`GateOverrideRecorded` event is appended.

### Phantom gate override prevention

`handle_overrides_record` must validate that the named gate exists in the current state's
template. An agent submitting an override for a gate in a different state, or a nonexistent gate,
would produce a `GateOverrideRecorded` event with no routing effect, but it would appear in
`koto overrides list` and mislead human reviewers. The handler derives the current state from
the event log and checks that the gate name is present in `template_state.gates` before appending
the event. If the gate is not found in the current state, the call returns an error.

### Event log integrity

`GateOverrideRecorded` events are appended to the same atomic JSONL state file as all other
events, using the existing write path. No new integrity surface is introduced. Override events
are append-only and sticky within the epoch; they cannot be retracted without rewind (which
starts a new epoch and makes the prior override invisible to `derive_overrides`).

### Audit completeness

`actual_output` is populated from the `GateEvaluated` event in the current epoch, not from
agent-provided data. An agent cannot claim a different actual output than what the engine
observed. If no `GateEvaluated` event exists for the named gate (e.g., the gate was never
evaluated in this epoch), `actual_output` is `null` and the override is still recorded with
a note indicating no prior evaluation was found.

### Cross-epoch leakage

`derive_overrides` scopes to the current epoch by finding the most recent state-changing event
(`Transitioned`, `DirectedTransition`, `Rewound`) and returning only `GateOverrideRecorded`
events after that boundary whose `state` field matches the current state. A rewind starts a new
epoch, making prior overrides invisible to `derive_overrides`. Overrides from a previous visit
to the same state cannot bleed into the current visit because both the epoch boundary and the
`state` field filter must match.

## Consequences

### Positive

- Silent gate bypasses are eliminated. Every override leaves a `GateOverrideRecorded` event
  with mandatory rationale, gate name, substituted values, and last-known actual output.
- The `koto overrides list` command provides full session-wide override history for audit
  and replay.
- Template authors can control override routing via `override_default` per gate without
  requiring agents to know the gate type's schema.
- `agent_actionable: true` in `blocking_conditions` tells agents they can call
  `koto overrides record` without `--with-data` for any gate with a known type.
- The existing `accepts` block workaround pattern continues to work (Feature 4 backward
  compatibility is independent of this feature).

### Negative

- Scope addition: `GateEvaluated` is a new event type not in the original roadmap description.
  It adds one event per gate evaluation to the state file. For states with many gates, this
  increases state file size linearly with gate count.
- The advance loop now calls `derive_overrides` on every `koto next` invocation, even when no
  overrides exist. This is a full scan of the epoch's event slice -- cheap for typical session
  sizes but worth noting.

### Mitigations

- `GateEvaluated` events are small (gate name + structured output + metadata). Gate counts per
  state are bounded by template design; the PRD does not define a maximum but typical usage is
  1-5 gates per state.
- `derive_overrides` is O(epoch length) and mirrors `derive_decisions`, which already runs on
  every `koto next`. The cost is proportional to session length, consistent with existing
  persistence query patterns.
