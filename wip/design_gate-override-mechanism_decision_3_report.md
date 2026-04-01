# Decision 3: Override Event Persistence and Advance Loop Integration

**Decision ID:** 3
**Topic:** gate-override-mechanism
**Depends on:** D1 (chosen: Option A — skip execution), D2 (chosen: Option A — optional
`override_default` on Gate struct), D4 (chosen: two functions — `derive_overrides` current epoch,
`derive_overrides_all` cross-epoch), D5 (chosen: both CLI and engine enforce `gates` namespace)

---

## Context

D1 resolved that the advance loop skips gate execution when `derive_overrides` returns an override
for a gate in the current epoch. Gate outputs are never re-run at override time; the advance loop
reads the last-known gate output from the event log at the point the agent called
`koto overrides record`. This means `GateOverrideRecorded` must carry that last-known output as
`actual_output` (captured when the event is written, not at advance time).

D2 resolved that `override_default` is `Option<serde_json::Value>` on the `Gate` struct. Built-in
defaults always exist for the three known gate types. D4 resolved that `derive_overrides` mirrors
`derive_decisions` exactly — it is epoch-scoped and is the function the advance loop calls.

This decision covers four sub-questions:

1. What fields does `GateOverrideRecorded` carry?
2. How does override data enter the `gates.*` evidence map?
3. How is `any_failed` computed when some gates are overridden?
4. How does `agent_actionable` get set true when a gate has an `override_default`?

A fifth question — how `derive_overrides` scopes to the current epoch — is answered by the D4
choice and the existing `derive_decisions` implementation, so it is resolved by application rather
than deliberation; the analysis section explains the mechanics.

---

## Sub-question 1: GateOverrideRecorded event fields

### Option A — Minimal fields

```
GateOverrideRecorded {
    gate: String,
    rationale: String,
    override_applied: serde_json::Value,
    timestamp: String,
}
```

### Option B — Full audit fields (D1-required)

```
GateOverrideRecorded {
    state: String,
    gate: String,
    rationale: String,
    override_applied: serde_json::Value,
    actual_output: serde_json::Value,
    timestamp: String,
}
```

### Analysis

**Option A** omits `state` and `actual_output`. Omitting `state` is a concrete regression: the
event log is append-only JSONL, and a `GateOverrideRecorded` event that lacks its own `state` field
can only be interpreted by traversing backwards through preceding transition events. Every other
epoch-anchored payload in `EventPayload` — `EvidenceSubmitted`, `DecisionRecorded`,
`DefaultActionExecuted`, `IntegrationInvoked` — carries a `state` field. Diverging here makes the
event non-self-describing, which matters for `koto overrides list` (D4): `derive_overrides_all`
returns cross-epoch items, and each item in the `koto overrides list` output carries a `state` field
(see D4 output format). If the payload lacks `state`, the list handler must reconstruct it from
surrounding transition events — a fragile dependency on log structure.

Omitting `actual_output` directly contradicts D1's conclusion. D1 chose skip-execution explicitly
on the precondition that `GateOverrideRecorded` would carry the gate's last-known output captured
at `koto overrides record` time. PRD R6 states the event must contain "the gate's actual output."
D1 deferred the mechanics of capturing `actual_output` to this decision. Without this field, the
audit trail cannot distinguish "gate was overridden while still failing" from "gate may have passed
before the override was recorded."

**Option B** satisfies all requirements and has no cost beyond two additional fields. Both are known
at the time `koto overrides record` runs: `state` is read from the event log via
`derive_state_from_log`, and `actual_output` is read from the most recent gate evaluation event for
the named gate in the current epoch. The timestamp field is set by the standard
`append_event` call (the engine provides it via the same path used for all other events). Including
it in the payload is consistent with the D4 list format, which shows `timestamp` per override item.

The `timestamp` field belongs in the payload, not only in the `Event` wrapper, for the same reason
`state` does: `derive_overrides_all` returns cross-epoch items where the receiver needs the
timestamp without having to cross-reference the `Event.timestamp` wrapper.

### Chosen: Option B

Option A is rejected because it omits `actual_output` (contradicts D1 and PRD R6) and omits `state`
(breaks `koto overrides list` self-description and diverges from the established pattern across all
other epoch-anchored event payloads).

---

## Sub-question 2: How override data enters the gates.* evidence map

### Option A — derive_overrides called once before gate iteration; overridden gates skipped entirely

The advance loop calls `derive_overrides(events)` once before entering the gate evaluation block.
It builds a `BTreeMap<String, serde_json::Value>` of `gate_name -> override_applied`. For each gate
in `template_state.gates`, it checks whether a key exists in that map. If yes, skip gate execution
and use `override_applied` as the gate's contribution to `gate_evidence_map`. If no, call the gate
evaluator normally.

### Option B — Each gate checks for an override during its individual evaluation step

The override lookup is woven into the per-gate iteration inside the gate evaluation loop. Each gate
calls into a shared function or checks the override map inline before deciding to execute.

### Analysis

**Option B** is a structural question about where the lookup happens, not what happens. Functionally
it reaches the same outcome as Option A when implemented correctly. The distinction matters for
clarity and consistency with how the action skip is implemented.

In `advance.rs`, the existing skip-execution precedent for `execute_action` (line 268-293) is a
pre-check at the call site: `let has_evidence = !current_evidence.is_empty(); let result =
execute_action(&state, action, has_evidence);`. The execute_action closure internally decides
whether to skip. For gates, the equivalent pre-check is cleaner at the batch level because:

1. `evaluate_gates` is called as a closure: `G: Fn(&BTreeMap<String, Gate>) ->
   BTreeMap<String, StructuredGateResult>`. It receives the full gate map and returns a result map.
   The current caller at line 303 (`let gate_results = evaluate_gates(&template_state.gates)`)
   does not thread override data into the closure. To implement Option B, this closure signature
   must change to accept the override map, or the lookup must happen in the advance loop before
   calling the closure.
2. If the closure is modified to accept overrides, the real evaluator and all test fakes must
   be updated — a larger surface change than a pre-check in the loop.

Under Option A, `derive_overrides` runs once before line 303 and produces the override map. The
loop at lines 308-310 that builds `gate_evidence_map` is extended: for each gate name, check the
override map first, use `override_applied` if present (no gate execution call), otherwise add the
gate to a sub-map passed to `evaluate_gates`. The `gate_evidence_map` is assembled from both
sources. This keeps the `evaluate_gates` closure signature unchanged and makes the skip-execution
logic explicit and grep-able at a single call site.

The D1 decision also describes this: "derive_overrides is called once before gate evaluation; if
a gate has an override, skip it entirely and inject override_applied as its gates.* evidence." This
is a direct statement of Option A.

### Chosen: Option A

Option B would require changing the `evaluate_gates` closure signature or duplicating logic. Option A
leaves the existing closure contract intact, mirrors the pre-check pattern used for `execute_action`,
and is what D1's description specifies.

---

## Sub-question 3: any_failed computation when some gates are overridden

### Option A — Overridden gate treated as passing if override_applied satisfies the when clause

After building `gate_evidence_map` (which includes `override_applied` values for overridden gates),
compute `any_failed` the same way as today: check each gate result's `GateOutcome`. Overridden gates
contribute a synthetic result with `GateOutcome::Passed` when their `override_applied` satisfies
the gate type's pass condition.

This requires the advance loop to evaluate the pass condition against `override_applied` to determine
whether to assign `Passed` or `Failed` as the synthetic outcome.

### Option B — Overridden gate always treated as passing regardless of override_applied value

Any gate with an override in the current epoch contributes `GateOutcome::Passed` unconditionally.
`override_applied` still enters `gate_evidence_map` as evidence for transition routing, but the
pass/fail determination ignores its content.

### Option C — Override only affects evidence injection; any_failed is recomputed against override_applied using the real pass condition

Functionally identical to Option A for all built-in gate types, but the framing shifts: the loop
always evaluates the pass condition — whether against real gate output or `override_applied`. The
pass condition evaluation function is shared, not duplicated.

### Analysis

**The PRD establishes the intended behavior via Example 5** (partial overrides): two gates fail;
the agent overrides `schema_check`; the next `koto next` returns `gate_blocked` with only
`size_check` in `blocking_conditions`. The overridden gate disappears from the blocking set. Both
the PRD and the Example 5 trace treat the override as causing the gate to stop blocking — the gate
is gone from `blocking_conditions` once overridden.

The question is: does an override with a *failing* `override_applied` (e.g., `{exit_code: 1,
error: ""}`) still unblock the gate? Options A and C diverge from Option B here.

**Option B** treats every overridden gate as passing, full stop. The agent's rationale is the
sufficient condition; the value of `override_applied` doesn't matter for gate blocking. This is
the simplest model and has an argument: the whole point of an override is that the agent has
accepted responsibility for the gate outcome. If the agent overrides with a failing value, that
is an unusual but deliberate choice (perhaps routing to a specific transition via the `when` clause),
and it should not cause the state to remain blocked.

The counter-argument is that the `when` clause routing model (scenarios 14-16 in the
structured-gate-output PRD) allows templates to route based on gate output values, not just
pass/fail. A template might have:

```
transitions:
  - target: manual_review
    when: {gates.ci_check.exit_code: 1}
  - target: deploy
    when: {gates.ci_check.exit_code: 0}
```

Under Option B, even if `override_applied` is `{exit_code: 1, error: ""}`, the gate is marked
Passed and the advance loop proceeds to transition resolution, where `gates.ci_check.exit_code`
would equal `1` in the evidence map and route to `manual_review`. This works correctly because
transition routing reads the evidence map (which has the `override_applied` value), not the outcome
field. The outcome field only governs `any_failed`. So Option B produces correct routing regardless.

**Option C** (and effectively Option A) evaluates the pass condition against `override_applied`.
For command gates, the pass condition is `exit_code == 0`. For context-exists, it is `exists ==
true`. For context-matches, it is `matches == true`. These conditions mirror the built-in defaults
— the "passing" override value is the same value the built-in default uses. So in practice, if the
agent calls `koto overrides record` without `--with-data`, the built-in default always produces a
passing value, and `any_failed` is false for that gate. If the agent provides `--with-data` with a
failing value, the gate remains in the blocking set unless the template has transition routing that
handles it.

However, Option C introduces a dependency on the pass condition evaluation function, which currently
lives inside the gate evaluator's output logic — the `GateOutcome` returned by `evaluate_command_gate`
etc. is determined by comparing the command's exit code to 0 (lines 64-84 in gate.rs). To replicate
this for `override_applied` in the advance loop, the pass condition must be callable from the advance
loop without executing the gate. This requires either:
- Extracting a `passes(gate_type, output) -> bool` function reachable from advance.rs, or
- Having the advance loop compute `any_failed` by passing `override_applied` through a synthetic
  `StructuredGateResult` with an outcome computed by the same logic.

This is doable but adds coupling. More importantly, what does it buy? If the agent explicitly
provides `--with-data` with a failing value, the intent is ambiguous — it could be routing (they
want to hit a `when` clause matching a failure), or it could be a mistake. Option B treats this as
an unconditional pass (removing the gate from `blocking_conditions`), which means the agent can
always unblock a gate by overriding it, regardless of `override_applied`. This matches the semantic
of "override": the agent is asserting they've handled this gate out-of-band. The value they provide
is for routing and audit, not for re-evaluating the blocking condition.

**Option B is the correct choice.** The override mechanism exists precisely to let agents bypass
gates that cannot pass on their own. Re-evaluating the pass condition against `override_applied`
would create a trap: an agent who wants to route to `manual_review` by providing
`{exit_code: 1, error: ""}` would find the gate still blocking, forcing them to use the default
value and lose the routing signal. This contradicts PRD R4's statement that "template authors can
declare a custom `override_default` per gate to route overrides to a different transition."

The name `override_applied` makes the semantics clear: this is the value that gets applied to the
evidence map. Whether it looks like a pass or fail is irrelevant to unblocking — the override event
itself is the unblocking signal.

### Chosen: Option B

Option C / A rejected because they re-evaluate the pass condition against `override_applied`, which
creates a trap for template authors who use non-passing values to drive routing, contradicting PRD R4.
Option B is correct: the existence of an override in the current epoch removes the gate from
`any_failed` unconditionally. The value still flows into `gate_evidence_map` for transition routing.

**Implementation note on failed_gate_results**: `blocking_conditions_from_gates` in `next_types.rs`
(lines 403-429) builds `BlockingCondition` entries by filtering gate results where outcome is not
Passed. Under Option B, overridden gates must contribute a synthetic `StructuredGateResult` with
`GateOutcome::Passed` so they are filtered out of `blocking_conditions`. The advance loop must build
this synthetic result when inserting an overridden gate's contribution into the gate results map.

---

## Sub-question 4: agent_actionable flag

### Option A — Set true when gate has override_default (instance OR type default always exists for known types)

Since D2 established that built-in type defaults always exist for the three known gate types, every
gate of a known type has a default. `agent_actionable` is true for all non-passed gates of any
known type.

### Option B — Set true only when gate has instance override_default (not type default)

`agent_actionable` is true only when `gate.override_default.is_some()`. Gates without an instance
default are marked `agent_actionable: false`, meaning agents must supply `--with-data` to override
them.

### Option C — Set true always for any failed gate regardless of override_default

`agent_actionable` is always true when a gate is in the blocking set. The field communicates
"can the agent do anything about this" — and an agent can always call `koto overrides record`, so
the answer is always yes.

### Analysis

**Option B** would mean a gate declared as:

```yaml
gates:
  ci_check:
    type: command
    command: "run_ci.sh"
```

...produces `agent_actionable: false` even though D2 established that the built-in type default
`{exit_code: 0, error: ""}` is always available for command gates. An agent seeing
`agent_actionable: false` would believe it cannot override the gate, which is incorrect. This is
a misleading interface.

**Option C** is the correct semantic: any failed gate can be overridden, because `koto overrides
record` works for all gates. The `agent_actionable` field's comment in `next_types.rs` (line 361)
says: "Feature 2 sets this true when the gate has an override_default, signaling the agent can call
`koto overrides record` to substitute gate output with the default." But the comment dates from
before D2 established that type defaults always exist. The intent is to signal "the agent can call
`koto overrides record`" — and that is always true, because there is always a default value for
any gate of a known type.

**Option A** is equivalent to Option C in the steady state (for all gates the engine supports
today), since known gate types always have a type default. But they differ for hypothetical future
unknown gate types or custom types. Option A would set `agent_actionable: false` for a gate whose
type the runtime doesn't recognize (which already produces a `GateOutcome::Error` via the `other`
arm in `evaluate_gates`). Option C would set it true even for a gate the engine can't evaluate
or override predictably.

For now, D2 says unknown gate types are caught at compile time (Feature 3), so at runtime all
gates have known types. The practical difference between A and C is zero for current gate types.

However, the comment in `next_types.rs` says the field signals "the agent can call
`koto overrides record`." The right encoding of this signal is: "a default exists, therefore
the agent can call `koto overrides record` without `--with-data`." If no default existed, the
agent would need `--with-data` and may not know the schema — a materially different situation.
Option A makes this distinction explicit. Option C papers over it.

**Option A is the right choice** because it encodes the precise condition: `agent_actionable` is
true when a default value is available (instance or type default), which is always true for known
gate types at runtime. The boolean communicates "can the agent call `koto overrides record` without
needing to know the gate's schema?" — and the answer is yes if and only if a default exists.

The check in `blocking_conditions_from_gates` becomes:

```rust
let agent_actionable = gate_defs
    .get(name)
    .map(|g| {
        g.override_default.is_some()
            || built_in_default_exists(&g.gate_type)
    })
    .unwrap_or(false);
```

Where `built_in_default_exists` returns true for `"command"`, `"context-exists"`, and
`"context-matches"` — the three types the engine knows. An unknown type with no instance
`override_default` would return false, which is correct: the agent would need `--with-data` and
the engine can't validate or supply a default.

### Chosen: Option A

Option B rejected: incorrectly treats type-level defaults as absent, producing `agent_actionable:
false` for all gates without an instance `override_default` even though a default always exists for
known types.

Option C rejected: loses the specificity of the flag. The intended signal is "a default is available
— you can call `koto overrides record` without `--with-data`." A gate whose type the engine doesn't
recognize at runtime would have `agent_actionable: true` under Option C, which is misleading.

---

## Sub-question 5: derive_overrides epoch scoping (D4 application)

D4 resolved this by choosing two functions. The epoch scoping mechanics are a direct application
of the `derive_decisions` pattern already in `engine/persistence.rs` (lines 273-303):

1. Call `derive_state_from_log(events)` to get the current state name.
2. Scan backwards for the most recent state-changing event (`Transitioned`, `DirectedTransition`,
   `Rewound`) whose `to` field matches the current state. This is the epoch boundary index.
3. Return only `GateOverrideRecorded` events after that index whose `state` field matches the
   current state.

The current-state filter on `state` (not just the epoch boundary index) is important: after a rewind
and re-entry into the same state, the epoch boundary is the rewind event, not the original entry.
Without the `state` field filter, `GateOverrideRecorded` events from a prior visit to the same state
(before the rewind) might appear after the rewind event in the log if the state was re-entered in
the same log. The `state` field on the payload (Option B in sub-question 1) makes this filter
straightforward and mirrors how `derive_decisions` filters on `state` at line 301.

---

## Consolidated View

### GateOverrideRecorded payload shape

Add to `EventPayload` in `src/engine/types.rs`:

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

`actual_output` is populated by the `koto overrides record` handler, which reads the most recent
gate output for the named gate from the current epoch's event log. This requires either a
`GateEvaluated` event (emitted by the advance loop when a gate runs) or the gate's output in
the blocking condition response. The D1 report recommends `GateEvaluated` as the anchor event.
`koto overrides record` reads the latest `GateEvaluated` for the named gate in the current epoch
to populate `actual_output`.

The `timestamp` in the payload duplicates `Event.timestamp` by intent: `derive_overrides_all`
returns cross-epoch items whose timestamp must be self-contained in the payload for serialization
without the wrapping `Event` struct.

Add to `EventPayload::type_name()`:

```rust
EventPayload::GateOverrideRecorded { .. } => "gate_override_recorded",
```

### Advance loop pseudocode (gate evaluation section)

```
// Load overrides for current epoch once before gate iteration
let epoch_overrides: BTreeMap<String, serde_json::Value> =
    derive_overrides(all_events)   // epoch-scoped, mirrors derive_decisions
        .into_iter()
        .map(|e| match &e.payload {
            EventPayload::GateOverrideRecorded { gate, override_applied, .. } =>
                (gate.clone(), override_applied.clone()),
            _ => unreachable!(),
        })
        .collect();

// Split gates into overridden (skip execution) and live (run normally)
let mut gate_evidence_map: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
let mut gate_results: BTreeMap<String, StructuredGateResult> = BTreeMap::new();

let mut gates_to_evaluate: BTreeMap<String, Gate> = BTreeMap::new();
for (name, gate) in &template_state.gates {
    if let Some(override_val) = epoch_overrides.get(name) {
        // D1: skip execution; inject override_applied directly
        // D3-Q2: use override_applied as evidence value
        gate_evidence_map.insert(name.clone(), override_val.clone());
        // D3-Q3: overridden gate is always treated as passing (synthetic result)
        gate_results.insert(name.clone(), StructuredGateResult {
            outcome: GateOutcome::Passed,
            output: override_val.clone(),
        });
    } else {
        gates_to_evaluate.insert(name.clone(), gate.clone());
    }
}

// Evaluate non-overridden gates
if !gates_to_evaluate.is_empty() {
    let live_results = evaluate_gates(&gates_to_evaluate);
    for (name, result) in &live_results {
        gate_evidence_map.insert(name.clone(), result.output.clone());
        gate_results.insert(name.clone(), result.clone());
    }
}

// Compute any_failed from combined results
// D3-Q3: overridden gates have GateOutcome::Passed, so they do not contribute to any_failed
let any_failed = gate_results
    .values()
    .any(|r| !matches!(r.outcome, GateOutcome::Passed));

// Build blocking_conditions for non-passing gates only
// D3-Q4: agent_actionable = instance override_default OR built-in type default exists
//         => true for all known gate types
let blocking_conditions = blocking_conditions_from_gates(&gate_results, &template_state.gates);
```

### blocking_conditions_from_gates change

In `next_types.rs`, `blocking_conditions_from_gates` currently hardcodes `agent_actionable: false`
(line 424). Change to:

```rust
let agent_actionable = gate_defs
    .get(name)
    .map(|g| g.override_default.is_some() || built_in_default_exists(&g.gate_type))
    .unwrap_or(false);
```

Where `built_in_default_exists(gate_type: &str) -> bool` returns `true` for `"command"`,
`"context-exists"`, and `"context-matches"`.

### derive_overrides in persistence.rs

```rust
pub fn derive_overrides(events: &[Event]) -> Vec<&Event> {
    let current_state = match derive_state_from_log(events) {
        Some(s) => s,
        None => return Vec::new(),
    };

    let epoch_start_idx = events.iter().enumerate().rev().find_map(|(idx, e)| {
        let to = match &e.payload {
            EventPayload::Transitioned { to, .. } => Some(to),
            EventPayload::DirectedTransition { to, .. } => Some(to),
            EventPayload::Rewound { to, .. } => Some(to),
            _ => None,
        };
        if to.map(|t| t == &current_state).unwrap_or(false) {
            Some(idx)
        } else {
            None
        }
    });

    let start = match epoch_start_idx {
        Some(idx) => idx + 1,
        None => return Vec::new(),
    };

    events[start..]
        .iter()
        .filter(|e| matches!(
            &e.payload,
            EventPayload::GateOverrideRecorded { state, .. } if state == &current_state
        ))
        .collect()
}

pub fn derive_overrides_all(events: &[Event]) -> Vec<&Event> {
    events
        .iter()
        .filter(|e| matches!(&e.payload, EventPayload::GateOverrideRecorded { .. }))
        .collect()
}
```

This is a line-for-line mirror of `derive_decisions` / the cross-epoch equivalent needed for
`koto overrides list`. The only difference from `derive_decisions` is the payload variant being
filtered.

---

## Decision summary

| Sub-question | Chosen option | Key reason |
|---|---|---|
| GateOverrideRecorded fields | Option B (full audit) | `actual_output` required by D1 + R6; `state` required by self-describing payloads and `koto overrides list` |
| Advance loop integration | Option A (pre-check, one-shot) | Keeps `evaluate_gates` closure signature unchanged; explicit skip site; matches D1 description |
| any_failed with overrides | Option B (override = unconditional pass) | Re-evaluating pass condition against `override_applied` traps template authors routing via failing values (contradicts R4) |
| agent_actionable | Option A (instance OR type default) | Encodes the precise semantics: "default available, no --with-data required"; gates with unknown types correctly return false |
| derive_overrides epoch scoping | D4 application of derive_decisions pattern | No deliberation required; epoch-scoping logic is identical, new payload type is the only change |
