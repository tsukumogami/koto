---
status: Accepted
upstream: docs/prds/PRD-gate-transition-contract.md
problem: |
  Gate evaluation is one-directional: the engine runs the gate and the result is final.
  When a gate fails, an agent's only options are to wait or to work around the failure by
  submitting an override enum value via an accepts block. The workaround has no audit trail
  -- the engine can't distinguish "genuinely resolved" from "bypassed without explanation."
  Session replay and human review can't reconstruct which gates were overridden, when, or
  with what justification.
decision: |
  Add a first-class override mechanism: agents call `koto overrides record` to substitute
  a gate's output with default or explicit values, attaching mandatory rationale. Override
  events are sticky within the current epoch and read by the advance loop during gate
  evaluation, which skips gate execution for any gate with an active override and injects
  the override value directly into the gates.* evidence map. `koto overrides list` queries
  the override history across the full session. A new GateEvaluated event makes last-known
  gate output queryable at override-record time, satisfying the audit requirement without
  re-running gates.
rationale: |
  Skipping gate execution on override matches the koto decisions record precedent, avoids
  latency and non-idempotent side effects, and produces a stable audit trail -- actual_output
  is captured once at override-record time rather than differing between koto next calls.
  Treating overridden gates as unconditional passes lets template authors use non-passing
  override values to drive routing, which the PRD explicitly requires. All five design
  decisions compose without conflict; the only scope addition beyond the roadmap description
  is GateEvaluated as the anchor event for actual_output capture.
---

# DESIGN: Gate override mechanism

## Status

Accepted

## Context and Problem Statement

Gate evaluation is one-directional: the engine runs the gate and the result is final. When a
gate fails, an agent's only options are to wait (if the gate condition may change) or work
around the failure by submitting an `override` enum value via an `accepts` block. The workaround
has no audit trail -- the engine can't distinguish "the agent genuinely resolved the blocking
condition" from "the agent bypassed the gate without explaining why." Session replay and human
review of a completed workflow can't reconstruct which gates were overridden, when, or with
what justification.

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
  requiring agents to supply `--with-data` (R4). When `--with-data` is supplied, it's validated
  against the gate type's schema (R5).
- Each `koto overrides record` call targets one gate with one rationale (R5a). Multiple gates
  in a state each need their own call.
- Override events must be sticky within the current epoch (the span of events from the most
  recent state-entry to the next state transition or rewind) -- they persist until the state
  transitions -- and accumulate across multiple `overrides record` calls in the same epoch (R5).
- The `gates` evidence key must be reserved: agents may not submit `gates.*` keys via
  `koto next --with-data`, preventing injection of fake gate data (R7).
- Rationale and `--with-data` payloads are subject to the same 1MB size limit as other
  `--with-data` payloads (R12).
- The mechanism mirrors `koto decisions record` / `koto decisions list` to keep the CLI
  surface consistent (R5).

## Considered Options

### Decision 1: Override execution model

When a `GateOverrideRecorded` event exists for a gate in the current epoch, the advance loop
faces a choice: skip gate execution and inject the override value directly, or run the gate
first to capture its current output for the audit trail and then substitute with override data.

PRD R6 says the event must contain "the gate's actual output." The word "actual" is doing a lot
of work here. The agent calls `koto overrides record` after they've seen the gate fail -- they're
recording a decision they've already made. At that point, the gate's "actual output" is the
output from the run that blocked the workflow, not a hypothetical re-run triggered by the
override command. Resolving this framing shapes the entire execution model.

#### Chosen: Skip execution

The advance loop skips any gate that has an active override in the current epoch. It injects
`override_applied` directly into `gate_evidence_map` without running the gate. The gate's
last-known output (captured at `koto overrides record` time by reading from the event log) is
stored as `actual_output` in the `GateOverrideRecorded` event.

This matches the `koto decisions record` precedent exactly. That command appends a
`DecisionRecorded` event without re-running any logic; overrides follow the same pattern.
The audit trail is stable: `actual_output` is recorded once when the agent calls
`koto overrides record`, not re-derived on each `koto next` call. If gates were ever evaluated
in parallel, skip-execution also avoids the ordering complexity that execute-then-substitute
would require.

Satisfying R6 under this model requires that gate outputs from blocking runs are persisted in
the event log. A new `GateEvaluated` event (emitted by the advance loop for each gate that
executes without an active override) provides the anchor. The `koto overrides record` handler
reads the most recent `GateEvaluated` for the named gate in the current epoch to populate
`actual_output`. `GateEvaluated` is not emitted for overridden gates -- their output is already
captured in `GateOverrideRecorded.actual_output` from when they last ran.

The full sequence from a blocked workflow to an advance:

```
# 1. Advance -- ci_check gate runs and fails, GateEvaluated event written
$ koto next myflow
{
  "status": "gate_blocked",
  "blocking_conditions": [
    {
      "name": "ci_check",
      "output": {"exit_code": 1, "error": "test_util_test failed"},
      "agent_actionable": false   // Feature 1 hardcodes false; Feature 2 (this design) sets true
    }
  ]
}

# 2. Record the override -- GateOverrideRecorded appended;
#    actual_output read from the GateEvaluated event above; ci_check not re-run
$ koto overrides record myflow --gate ci_check --rationale "CI flaky on unrelated test"
{"status": "recorded"}

# 3. Advance again -- ci_check skipped entirely; override_applied injected into gates.*
$ koto next myflow
{"status": "advanced", "transitioned_to": "deploy"}
```

Between steps 2 and 3, `ci_check` never runs. The gate's contribution to `gates.*` is
`{"exit_code": 0, "error": ""}` (the built-in default, since no `override_default` was
declared). The `GateOverrideRecorded` event carries the actual output from step 1 as
`actual_output`, so the audit trail reflects what the agent actually observed.

Note: this example uses a simplified template with no `override_default` on `ci_check` to
keep the skip-execution mechanic uncluttered. D2 introduces a richer template where
`ci_check` has `override_default: {exit_code: 1}` routing overrides to `manual_review`.
The D2 template is used in all subsequent examples.

#### Alternatives considered

**Execute then substitute**: Run the gate first to capture its current output, then discard
the result and use `override_applied` for routing. Rejected because gate re-execution adds
latency and risks non-idempotent side effects -- a gate that moves a Jira ticket or triggers
CI would run twice, wasting work that is discarded by construction. It also produces a
misleading audit trail: if the gate condition has changed since the override was recorded, the
re-run captures a different output than what the agent actually observed and acted on.

---

### Decision 2: override_default declaration

Template authors need a way to define what "override" means for a specific gate, so agents
don't have to know the gate's value schema to call `koto overrides record` without `--with-data`.
The question is where in the template this default lives: on the gate itself, or in a separate
map at the state level.

There are also built-in type defaults that apply when no template-level default is declared.
These cover the three current gate types and ensure `koto overrides record` without `--with-data`
never fails for a gate of a known type.

#### Chosen: Optional field on Gate struct

`override_default: Option<serde_json::Value>` is added directly to the `Gate` struct in
`template/types.rs`. Template authors declare it inline, adjacent to the rest of the gate's
configuration. The field uses `#[serde(default, skip_serializing_if = "Option::is_none")]`,
matching the pattern for other optional gate fields like `timeout`.

Built-in defaults exist in code for all known gate types: `command` →
`{"exit_code": 0, "error": ""}`, `context-exists` → `{"exists": true, "error": ""}`,
`context-matches` → `{"matches": true, "error": ""}`. Resolution order at override time:
instance `override_default` → built-in type default. Calls to `koto overrides record` without
`--with-data` always succeed for any gate of a known type.

Here's what this looks like in a template with two gates in the same state:

```yaml
states:
  review:
    gates:
      ci_check:
        type: command
        command: "run_ci.sh"
        override_default: {exit_code: 1, error: ""}   # routes to manual_review, not deploy
      schema_check:
        type: context-exists
        key: schema_version
        # no override_default: built-in {exists: true, error: ""} applies
    transitions:
      - target: deploy
        when: {gates.ci_check.exit_code: 0}
      - target: manual_review
        when: {gates.ci_check.exit_code: 1}
```

`ci_check` uses a custom `override_default` so that overriding it routes to `manual_review`
rather than `deploy`. `schema_check` omits `override_default` and falls back to the built-in
`context-exists` default, which satisfies any `when` clause checking `gates.schema_check.exists`.

Co-location also makes Feature 3 (compiler validation) straightforward: the compiler checks
`gate.override_default.is_some()` and validates the JSON against the gate type's schema without
traversing a parent structure.

The three cases an agent encounters at runtime, using the template above:

```
# ci_check has override_default: {exit_code: 1} -- routes to manual_review
$ koto overrides record myflow --gate ci_check --rationale "Taking to manual review"
# override_applied = {exit_code: 1, error: ""} → koto next routes to manual_review

# schema_check has no override_default -- falls back to built-in {exists: true, error: ""}
$ koto overrides record myflow --gate schema_check --rationale "Schema version set externally"
# override_applied = {exists: true, error: ""} → satisfies any when clause on schema_check.exists

# Agent can always supply --with-data to use a specific value regardless of defaults
$ koto overrides record myflow --gate ci_check \
    --rationale "CI passed in the rerun I triggered manually" \
    --with-data '{"exit_code": 0, "error": ""}'
# override_applied = {exit_code: 0, error: ""} → routes to deploy
```

In the first case, the template author's intent (route overrides to `manual_review`) is
expressed in the template, not in the agent's command. The agent doesn't need to know the
gate's schema or the routing logic.

#### Alternatives considered

**Separate override_defaults block at state level**: A parallel `BTreeMap<String, Value>` at
the state level, next to `gates`. The Gate struct stays unchanged. Rejected because it splits
a gate's specification across two keys -- a template author can write an `override_defaults`
entry for a gate name that doesn't exist, and vice versa. The compiler must cross-validate two
independent maps, a new failure mode that has no equivalent elsewhere in the template schema.
The `agent_actionable` flag check must also reach back to the parent state's map rather than
staying at the gate level. `Gate` is not a shared type, so there's no reuse argument for the
indirection.

**Type defaults only, no instance override_default**: Only the built-in per-type defaults exist.
Agents must use `--with-data` for any custom override value. Rejected because it directly
contradicts PRD R4, which requires template authors to declare a custom `override_default` per
gate to route overrides to a different transition. The `ci_check` example above would be
impossible without this capability.

---

### Decision 3: Advance loop integration and any_failed semantics

With skip-execution chosen (D1), the advance loop needs to handle three related questions: how
overrides enter the `gates.*` evidence map, what `any_failed` means when some gates are
overridden, and how `agent_actionable` gets set in `blocking_conditions`.

These questions are connected. The advance loop builds a unified `gate_evidence_map` and a
`gate_results` map, both of which feed downstream logic. The override integration point
determines whether overridden gates appear in `gate_results` at all, which in turn determines
how `any_failed` is computed.

#### Chosen: Pre-check with unconditional pass semantics

`derive_overrides` is called once before gate iteration, returning a map of
`gate_name → override_applied` for the current epoch. The advance loop then splits gates into
two sets: those with an override (inject `override_applied` directly, insert a synthetic
`StructuredGateResult { outcome: Passed, output: override_applied }`) and those without
(pass to `evaluate_gates` unchanged). The two sets are merged into `gate_evidence_map` and
`gate_results`. `any_failed` is computed from the combined results -- overridden gates have
`GateOutcome::Passed` and don't contribute.

The "unconditional pass" part is deliberate. An overridden gate always counts as passing,
regardless of whether `override_applied` looks like a passing value. The override event is the
unblocking signal; the value flows into `gate_evidence_map` for routing. This matters for the
`ci_check` example from D2: a template author who sets `override_default: {exit_code: 1, error: ""}` to route to `manual_review` would hit a trap if the advance loop re-evaluated
the pass condition against `override_applied` -- exit code 1 means "failed" for a command gate,
so the gate would stay blocking despite the override. The agent can always call
`koto overrides record` to unblock a gate; what value they use is a routing concern, not a
blocking concern.

`agent_actionable` in `blocking_conditions` is set true when the gate has an instance
`override_default` or a built-in type default exists for the gate's type. Since built-in
defaults exist for all three known gate types, this is effectively always true at runtime.
The check encodes a precise semantic: "a default is available, so the agent can call
`koto overrides record` without needing to know the gate's schema."

The partial override case shows why unconditional pass semantics matter. With two gates
failing, the agent overrides them one at a time:

```
# Both gates failing:
$ koto next myflow
{
  "status": "gate_blocked",
  "blocking_conditions": [
    {"name": "ci_check",   "output": {"exit_code": 1, "error": "..."}, "agent_actionable": true},
    {"name": "size_check", "output": {"exit_code": 1, "error": "..."}, "agent_actionable": true}
  ]
}

# Override ci_check only:
$ koto overrides record myflow --gate ci_check --rationale "Flaky on unrelated test"

# Advance -- ci_check is gone from blocking_conditions (unconditional pass);
# size_check still runs and still fails
$ koto next myflow
{
  "status": "gate_blocked",
  "blocking_conditions": [
    {"name": "size_check", "output": {"exit_code": 1, "error": "..."}, "agent_actionable": true}
  ]
}

# Override size_check:
$ koto overrides record myflow --gate size_check --rationale "Large file is intentional"

# All gates overridden -- workflow advances
$ koto next myflow
{"status": "advanced", "transitioned_to": "deploy"}
```

Both overrides are sticky for the rest of this epoch. A rewind would clear them.

#### Alternatives considered

**Per-gate override check inside evaluate_gates**: Thread the override map into the
`evaluate_gates` closure and handle the skip inside the evaluator. Rejected because the closure
currently takes `&BTreeMap<String, Gate>` and returns `BTreeMap<String, StructuredGateResult>`.
Modifying it to accept overrides requires changing both the real evaluator and all test fakes.
The pre-check approach leaves the closure signature intact.

**Re-evaluate pass condition against override_applied**: Compute `any_failed` by running the
gate type's pass condition against `override_applied`, treating it the same as real gate output.
Rejected because it creates the trap described above: a template author using
`override_default: {exit_code: 1}` to drive routing to a non-default transition finds the gate
still blocking. PRD R4 explicitly requires this routing pattern to work.

**agent_actionable only when instance override_default present**: Set `agent_actionable: false`
for gates that rely on the built-in type default. Rejected because D2 established that built-in
defaults always exist for known gate types, so this would mark `agent_actionable: false` for
every gate without an explicit `override_default` field, even though the agent can always call
`koto overrides record` without `--with-data`. Misleading interface.

---

### Decision 4: CLI structure and derive_overrides scope

The CLI surface follows the `koto decisions record` / `koto decisions list` naming pattern (R5).
However, the `decisions record` handler takes a single `--with-data` JSON blob containing
`choice`, `rationale`, and optionally `alternatives_considered`, while the PRD specifies a
different interface for `overrides record`. The question is whether to follow the same
implementation pattern or the PRD's specified interface.

The scope of `derive_overrides` also needs a decision. The advance loop needs only current-epoch
overrides (a rewind should reset which gates are overridden, just as it resets which decisions
are visible). But `koto overrides list` must return all overrides across the full session (R8).
These two callers have different requirements.

#### Chosen: Separate flags; two persistence functions

The CLI uses explicit named flags: `koto overrides record <name> --gate <gate_name> --rationale
"reason" [--with-data '...']`. This matches the PRD's examples verbatim across six independent
usage scenarios and gives Clap native field-presence validation -- "required argument --gate not
provided" rather than a custom JSON error the agent must parse.

Two persistence functions serve the two callers:
- `derive_overrides(events: &[Event]) -> Vec<&Event>` -- epoch-scoped, mirrors `derive_decisions`
  exactly. Used by the advance loop.
- `derive_overrides_all(events: &[Event]) -> Vec<&Event>` -- returns all `GateOverrideRecorded`
  events regardless of epoch. Used by `koto overrides list`.

`derive_overrides` uses the same epoch-boundary logic as `derive_decisions`: find the most recent
state-changing event (`Transitioned`, `DirectedTransition`, `Rewound`) whose `to` matches the
current state, then return only matching events after that index. The `GateOverrideRecorded`
payload's `state` field makes the filter direct (no backwards traversal needed).

`koto overrides list` output mirrors `koto decisions list`. The cross-epoch scope of
`derive_overrides_all` means the list persists across rewinds, unlike the advance loop's
epoch-scoped view:

```
# Record an override, advance to deploy:
$ koto overrides record myflow --gate ci_check --rationale "Flaky test"
$ koto next myflow
{"status": "advanced", "transitioned_to": "deploy"}

# Something goes wrong -- rewind back to review (new epoch begins)
$ koto rewind myflow

# The advance loop sees no active overrides in the new epoch:
$ koto next myflow
{
  "status": "gate_blocked",
  "blocking_conditions": [{"name": "ci_check", ...}]
}

# But the list shows the full history across all epochs:
$ koto overrides list myflow
{
  "state": "review",    // top-level: current workflow state
  "overrides": {
    "count": 1,
    "items": [
      {
        "state": "review",   // per-item: state when this override was recorded
        "gate": "ci_check",
        "rationale": "Flaky test",
        "override_applied": {"exit_code": 0, "error": ""},
        "actual_output": {"exit_code": 1, "error": "test_util_test failed"},
        "timestamp": "2026-04-01T10:00:00Z"
      }
    ]
  }
}
```

The override from before the rewind is visible in `koto overrides list` but invisible to the
advance loop. The agent needs to call `koto overrides record` again in the new epoch if they
want to unblock `ci_check` again.

#### Alternatives considered

**Unified --with-data blob**: Pack `gate` and `rationale` into a JSON blob, like decisions
record does internally. Rejected because the PRD defines a specific interface with separate
flags, and packaging required fields into JSON degrades the error experience -- Clap can no
longer validate field presence at parse time.

**Single derive_overrides returning all epochs**: One function for both callers, with the
advance loop post-filtering by epoch. Rejected because it pushes epoch-boundary logic into the
advance loop inline, disconnecting it from the tested `derive_*` pattern. The two callers have
genuinely different requirements; a single function can satisfy only one of them correctly.

---

### Decision 5: gates namespace reservation enforcement

The `gates` key is reserved in evidence (R7): agents may not submit `{"gates": {...}}` via
`koto next --with-data`. The engine already overwrites any agent-provided `"gates"` key
during the evidence merge, but silently -- the agent gets no feedback. R7 makes this an
explicit reservation that must be enforced with a clear error.

The question is where enforcement lives: at the CLI layer (before the event is persisted),
at the engine layer (before the merge), or both.

#### Chosen: Both CLI and engine layers

The CLI layer (`handle_next`) checks for a top-level `"gates"` key after JSON parse and before
`validate_evidence`. If found, it returns `InvalidSubmission` with a message: `"gates" is a
reserved field; agent submissions must not include this key`. This runs unconditionally,
regardless of what the template's `accepts` block declares.

The engine layer (`advance_until_stop`) converts the existing silent overwrite to an explicit
assertion. A `debug_assert!` confirms `current_evidence` doesn't contain `"gates"` when
building the merge map. In normal operation after the CLI check lands, this assertion never
fires. It documents the invariant and catches state files written outside the CLI.

The TODO comment at `advance.rs:356` anticipated exactly this: "once Feature 2 reserves the
`gates` namespace, the precedence comment above shifts from 'defense in depth' to 'invariant'."
This decision honors that expectation.

Context store (`koto context set`) is not affected. Context and evidence are structurally
separate -- context lives under `ctx/` as a filesystem key-value store and is never merged
into the evidence map. A context key named `"gates"` has no semantic overlap with the
`gates.*` evidence namespace.

An agent attempting to inject gate data directly via evidence submission gets a clear error:

```
# Attempting to fake gate output via koto next --with-data:
$ koto next myflow --with-data '{"gates": {"ci_check": {"exit_code": 0, "error": ""}}}'
error: invalid submission: "gates" is a reserved field; agent submissions must not include this key

# The correct way to substitute gate output is koto overrides record,
# which requires a rationale and records the actual gate output:
$ koto overrides record myflow --gate ci_check --rationale "CI passed in the manual rerun"
{"status": "recorded"}
```

The error fires before the event is persisted, so the state file is never written with a
`"gates"` key in `EvidenceSubmitted`.

#### Alternatives considered

**CLI layer only**: Sufficient for production, since the CLI check prevents `"gates"` from
ever appearing in `EvidenceSubmitted` events. Rejected because the engine's silent overwrite
becomes unexplained defensive code and the TODO comment in `advance.rs` becomes permanently
stale. The engine assertion is a one-line check; the cost is negligible.

**Engine layer only**: Already the current behavior (silent overwrite). Rejected because agents
receive no feedback. A submission appears to succeed, but the value is silently discarded. The
purpose of an explicit reservation is to surface the collision clearly, which only the CLI
layer can do.

## Decision Outcome

### Summary

The override mechanism is built around a new command that records a decision rather than
triggering computation. When an agent calls `koto overrides record myflow --gate ci_check
--rationale "CI was flaky"`, the handler reads the gate's last-known output from the event
log (via `derive_last_gate_evaluated`), validates the override value against the gate type's
schema if `--with-data` was provided, checks that the gate exists in the current state, and
appends a `GateOverrideRecorded` event. Nothing else runs. The next `koto next` call picks
up the override and skips executing `ci_check`, injecting `override_applied` into the
`gates.*` evidence map instead.

The event log grows by one new event type: `GateEvaluated`. The advance loop emits this for
every gate that runs, recording its structured output. This is what makes `actual_output`
capturable at override-record time without re-running the gate. It's the only scope addition
beyond the roadmap description.

For the advance loop, the change is a pre-check before gate iteration. `derive_overrides`
returns the set of active overrides for the current epoch. Gates in that set get a synthetic
`StructuredGateResult { outcome: Passed }` injected into `gate_results` and their
`override_applied` value injected into `gate_evidence_map`. The rest run normally. `any_failed`
is computed from the combined results, so overridden gates never block the workflow regardless
of what value `override_applied` contains.

The CLI gains a `koto overrides` subcommand with `record` and `list` variants, following the
`koto decisions` naming pattern. `koto overrides list` calls `derive_overrides_all` (not the
epoch-scoped `derive_overrides`) so the full session history is visible even after rewinds.
Template authors gain an `override_default` field on the `Gate` struct to declare custom
override values per gate instance; gates without one fall back to built-in type defaults.
Finally, `handle_next` gains an explicit rejection for any `--with-data` payload containing
a top-level `"gates"` key, enforcing the namespace reservation with a clear error message
rather than the current silent overwrite.

### Rationale

Skipping execution rather than re-running keeps the command model consistent: `koto overrides
record` is a record command, not an evaluation command. Re-executing gates would make
`koto next` secretly run shell commands whose results are thrown away, which violates the
principle that advance commands advance the workflow. It would also produce unstable audit
records if the gate condition changes between the override being recorded and `koto next` being
called.

Treating overridden gates as unconditional passes (rather than re-evaluating `override_applied`
against the pass condition) is required to support the routing use case the PRD describes.
Template authors need to express "when this gate is overridden, route to manual review rather
than the happy path." If the advance loop re-evaluated the pass condition, a non-passing
`override_applied` value would keep the gate blocking, making that routing pattern impossible.
The override event itself is the unblocking signal; the value is a routing input.

Two persistence functions rather than one keeps each caller correct without coupling them.
The advance loop must not see overrides from previous epochs; `koto overrides list` must see
all of them. These are genuinely different queries.

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

**Built-in defaults** (in `src/engine/gate.rs` or a new `src/engine/override_defaults.rs`):

```rust
pub fn built_in_default(gate_type: &str) -> Option<serde_json::Value> {
    match gate_type {
        "command"         => Some(json!({"exit_code": 0, "error": ""})),
        "context-exists"  => Some(json!({"exists": true, "error": ""})),
        "context-matches" => Some(json!({"matches": true, "error": ""})),
        _                 => None,
    }
}
```

### Advance loop changes (src/engine/advance.rs)

The gate evaluation block gains a pre-check for epoch overrides:

```
1. Call derive_overrides(all_events) → Vec<&Event> (epoch-scoped GateOverrideRecorded events)
   Transform into epoch_overrides: BTreeMap<String, Value> by extracting gate → override_applied
   from each event's payload
2. For each gate in template_state.gates:
   a. If gate name is in epoch_overrides:
      - Insert override_applied into gate_evidence_map
      - Insert a StructuredGateResult { outcome: Passed, output: override_applied }
        constructed directly (not from evaluate_gates) into gate_results
   b. Otherwise: add to gates_to_evaluate
3. Call evaluate_gates(&gates_to_evaluate) for non-overridden gates
   - Call append_event(GateEvaluated { ... }) for each result
   - GateEvaluated is NOT emitted for overridden gates
4. Merge live results into gate_evidence_map and gate_results
5. Compute any_failed from gate_results
6. Build blocking_conditions from non-passing gate_results
```

`blocking_conditions_from_gates` gains the `agent_actionable` check:

```rust
let agent_actionable = gate_defs
    .get(name)
    .map(|g| g.override_default.is_some() || built_in_default_exists(&g.gate_type))
    .unwrap_or(false);
```

### Persistence changes (src/engine/persistence.rs)

Three new functions mirroring the `derive_decisions` pattern:

- `derive_overrides(events: &[Event]) -> Vec<&Event>` -- epoch-scoped (epoch boundary +
  state-field filter). Used by the advance loop.
- `derive_overrides_all(events: &[Event]) -> Vec<&Event>` -- all `GateOverrideRecorded`
  events regardless of epoch. Used by `koto overrides list`.
- `derive_last_gate_evaluated(events: &[Event], gate: &str) -> Option<serde_json::Value>` --
  most recent `GateEvaluated` for the named gate in the current epoch. Used by
  `handle_overrides_record` to populate `actual_output`.

### CLI changes (src/cli/mod.rs and src/cli/overrides.rs)

New `OverridesSubcommand` Clap enum:

```rust
#[derive(Subcommand)]
pub enum OverridesSubcommand {
    Record {
        name: String,
        #[arg(long)] gate: String,
        #[arg(long)] rationale: String,
        #[arg(long)] with_data: Option<String>,
    },
    List { name: String },
}
```

`handle_overrides_record` steps:
1. Derive current state from event log; verify the named gate exists in
   `template_state.gates`. Return an error if not found.
2. Parse and validate `--with-data` JSON if provided (key presence, value types, no extra
   keys, size ≤ 1MB per R5 and R12).
3. Validate rationale length ≤ 1MB (R12).
4. Resolve override value: `--with-data` → instance `override_default` → built-in type
   default.
5. Read `actual_output` via `derive_last_gate_evaluated`.
6. Append `GateOverrideRecorded` event with fields: `state` (current state name), `gate`
   (from `--gate`), `rationale` (from `--rationale`), `override_applied` (resolved in step 4),
   `actual_output` (from step 5), `timestamp` (now).

`handle_overrides_list` calls `derive_overrides_all` and returns the JSON structure shown in
Decision 4 above.

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

In `advance_until_stop`, the silent overwrite becomes an assertion:

```rust
debug_assert!(
    !current_evidence.contains_key(GATES_EVIDENCE_NAMESPACE),
    "gates key in current_evidence; CLI reservation should have prevented this"
);
```

## Implementation Approach

### Phase 1: Event infrastructure

1. Add `GateEvaluated` and `GateOverrideRecorded` to `EventPayload` in `src/engine/types.rs`.
2. Implement `derive_overrides`, `derive_overrides_all`, and `derive_last_gate_evaluated` in
   `src/engine/persistence.rs`.
3. Add `built_in_default` function.
4. Unit tests for the new persistence functions.

### Phase 2: Advance loop integration

1. Add `override_default` to the `Gate` struct in `src/template/types.rs`.
2. Modify `src/engine/advance.rs` to emit `GateEvaluated` events, pre-check
   `derive_overrides`, inject synthetic results for overridden gates, and update `any_failed`.
3. Update `blocking_conditions_from_gates` in `src/cli/next_types.rs` for `agent_actionable`.
4. Unit tests for override injection, `any_failed` semantics, and `agent_actionable`.

### Phase 3: CLI and namespace enforcement

1. Add `OverridesSubcommand` to the Clap command tree; implement `handle_overrides_record`
   and `handle_overrides_list` in `src/cli/overrides.rs`.
2. Add the `gates` namespace pre-check in `handle_next`.
3. Add the `debug_assert` in `advance_until_stop`.
4. Functional tests: full override flow, `koto overrides list` output, namespace rejection.

## Security Considerations

### Rationale injection

`--rationale` is stored verbatim in the event log as a JSON string field. It's never executed
or evaluated as code -- only read back by `koto overrides list` and serialized. The 1MB limit
(R12) prevents storage exhaustion. No sanitization beyond the size check is needed.

### Size limit scope

`--rationale` and `--with-data` are each limited to 1MB independently (R12 applies per
payload, consistent with other `--with-data` size limits in the codebase). A single
`GateOverrideRecorded` event can therefore be ~2MB in the worst case. That's acceptable:
these are rare, human-authored events rather than high-frequency engine events.

### gates namespace reservation

An agent submitting `{"gates": {...}}` via `koto next --with-data` can't influence routing,
because the engine merge unconditionally overwrites any agent-provided `"gates"` key. The
CLI-layer check adds a clear error so the agent learns immediately rather than seeing a silent
discard. The engine assertion catches state files written outside the CLI.

A flat key `"gates.ci_check"` (literal dot in the key name) passes the top-level reservation
check, but it's not reachable by `resolve_transition`. The resolver uses dot-path traversal on
nested JSON objects: it looks up `"gates"` first, then navigates into the nested object. A flat
key with an embedded dot is a different key entirely. The reservation only needs to cover the
top-level `"gates"` key.

### Schema validation for --with-data

Override values provided via `--with-data` are validated against the gate type's schema,
checking both key presence and value types (string, integer, boolean), with no extra keys
allowed. Values that fail validation are rejected before the event is appended.

### Phantom gate overrides

`handle_overrides_record` verifies the named gate exists in the current state's template before
appending the event. An override for a nonexistent gate would appear in `koto overrides list`
and mislead human reviewers, even though it would have no routing effect. Returning an error
on unknown gate names prevents this.

### Audit completeness

`actual_output` comes from the `GateEvaluated` event in the current epoch, not from
agent-provided data. An agent can't claim a different actual output than what the engine
observed. If no `GateEvaluated` event exists for the named gate (the gate was never evaluated
in this epoch), `actual_output` is `null` and the override is still recorded.

### Cross-epoch leakage

`derive_overrides` scopes to the current epoch: it finds the most recent state-changing event
and returns only `GateOverrideRecorded` events after that boundary whose `state` field matches
the current state. A rewind starts a new epoch, making prior overrides invisible to
`derive_overrides`. A previous visit to the same state can't leak overrides into the current
visit because both the epoch boundary and the `state` field filter must match.

## Consequences

### Positive

- Silent gate bypasses are eliminated. Every override leaves a `GateOverrideRecorded` event
  with mandatory rationale, gate name, substituted values, and last-known actual output.
- `koto overrides list` provides full session-wide override history for audit and replay.
- Template authors can control override routing via `override_default` per gate without
  requiring agents to know the gate type's schema.
- `agent_actionable: true` in `blocking_conditions` tells agents they can call
  `koto overrides record` without `--with-data` for any gate with a known type.
- The existing `accepts` block workaround pattern continues to work (Feature 4 backward
  compatibility is independent).

### Negative

- `GateEvaluated` is a new event type not in the original roadmap description. It adds one
  event per gate evaluation to the state file, increasing state file size linearly with gate
  count over a session.
- `derive_overrides` runs on every `koto next` invocation, even when no overrides exist. It's
  O(epoch length) and consistent with `derive_decisions`, but it's a new scan per advance.

### Mitigations

- `GateEvaluated` events are small (gate name, structured output, outcome, metadata). Typical
  states have 1-5 gates; the size growth is bounded and proportional to workflow activity.
- The `derive_overrides` scan mirrors `derive_decisions`, which already runs on every
  `koto next`. Adding a second pass over the same event slice is unlikely to be measurable
  at typical session sizes.
