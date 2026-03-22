# Phase 3 Research: Gate-with-Evidence-Fallback Mechanics

## Questions Investigated

- What is koto's current template format for `gates`?
- Does the auto-advancement engine support having both `gates` AND `accepts` on the same state?
- What happens when `koto next` is called on a state with a gate that fails — does it fall through to `accepts`?
- What changes to the advancement engine are needed to support "if gate fails, fall back to requiring evidence via `accepts`"?
- What does koto return when evidence is required? How does the agent know what fields to submit?
- Are there existing tests or examples of states with both gates and accepts?

---

## Findings

### 1. Current gate format

Gates are declared in a state's `gates:` block in the YAML front-matter. Each gate has:

```yaml
gates:
  gate_name:
    type: command
    command: "shell command"
    timeout: 30   # optional, seconds; 0 = use default 30s
```

Only `type: command` is supported. Field-based gate types (`field_not_empty`, `field_equals`) were explicitly removed and produce a compile-time error with the message: `"Field-based gates have been replaced by accepts/when."` (see `src/template/compile.rs:257` and `src/template/types.rs:143-149`).

The `Gate` struct (`src/template/types.rs:66-76`) stores `gate_type`, `command`, and `timeout`. Evaluation runs all gates without short-circuiting (`src/gate.rs:39-58`), returning a `BTreeMap<String, GateResult>` where each result is `Passed`, `Failed{exit_code}`, `TimedOut`, or `Error{message}`.

### 2. Can a state have both `gates` AND `accepts`?

Yes, and this is explicitly tested. The compile test `command_gate_alongside_accepts_when` (`src/template/compile.rs:739-782`) creates a state with both a `gates:` block (command type) and an `accepts:` block with conditional `when` transitions, and asserts it compiles successfully.

The `TemplateState` struct (`src/template/types.rs:32-44`) has both fields:

```rust
pub gates: BTreeMap<String, Gate>,
pub accepts: Option<BTreeMap<String, FieldSchema>>,
```

Template validation imposes no rule preventing co-existence.

### 3. What happens when `koto next` is called on a gated state and the gate fails?

The engine does **not** fall through to `accepts`. It stops with `StopReason::GateBlocked`.

Tracing the advancement loop in `src/engine/advance.rs:227-239`:

```rust
// 5. Evaluate gates
if !template_state.gates.is_empty() {
    let gate_results = evaluate_gates(&template_state.gates);
    let any_failed = gate_results
        .values()
        .any(|r| !matches!(r, GateResult::Passed));
    if any_failed {
        return Ok(AdvanceResult {
            final_state: state,
            advanced,
            stop_reason: StopReason::GateBlocked(gate_results),
        });
    }
}
```

When any gate fails, the loop returns immediately with `GateBlocked`. The `accepts` block is never consulted. This means gate failure and evidence requirement are entirely separate stop conditions — there is no current fallback path.

The CLI (`src/cli/mod.rs:812-835`) maps `StopReason::GateBlocked` to `NextResponse::GateBlocked`, which serializes without an `expects` field (it's null). The agent receives:

```json
{
  "action": "execute",
  "state": "...",
  "directive": "...",
  "advanced": false,
  "expects": null,
  "blocking_conditions": [{"name": "...", "type": "command", "status": "failed", "agent_actionable": false}],
  "error": null
}
```

Notably, `blocking_conditions[*].agent_actionable` is hardcoded to `false` for command gates (`src/cli/mod.rs:826` and `src/cli/next.rs:55`). This is accurate for the current model: the agent cannot resolve a failing command gate by submitting evidence.

### 4. What changes are needed to support gate-with-evidence-fallback?

The gate-with-evidence-fallback pattern — "try gate first; if gate fails, fall back to requiring evidence via `accepts`" — requires a new behavior mode. Currently, `GateBlocked` is unconditional: a failed gate always stops the loop and returns an agent-inaccessible blocking state.

Three changes are needed:

**a. A mode flag on the state or gate to opt into fallback behavior.**

The simplest approach: add an optional `fallback: evidence` (or `on_failure: evidence`) field to the gate declaration, or add a top-level `gate_mode: fallback` on the state. Without a flag, existing gate semantics are preserved (hard block).

Alternatively, the presence of both `gates` and `accepts` on the same state could implicitly mean "fallback to evidence on gate failure." This is the most template-author-friendly approach and requires no new YAML fields. The rule would be: if a state has both `gates` and `accepts`, gate failure routes to evidence rather than hard-blocking.

**b. Engine loop change.**

In `advance.rs` around line 228, the gate evaluation block needs to be conditional on the mode. For fallback mode, a gate failure would skip the hard stop and fall through to transition resolution (which will then reach `NeedsEvidence`/`EvidenceRequired` because the state has conditional `when` transitions). No evidence has been submitted yet at this point, so `resolve_transition` would return `NeedsEvidence`.

**c. CLI output change.**

When the agent receives `EvidenceRequired` after a gate failure (fallback mode), the `expects` schema must be present. The `blocking_conditions` field must also be surfaced — otherwise the agent has no way to know the gate failed and that this is an override situation rather than a normal evidence prompt.

This points to a new response variant or a modified `EvidenceRequired` variant that includes both `expects` and `blocking_conditions`. The current `NextResponse::EvidenceRequired` has no `blocking_conditions` field (`src/cli/next_types.rs:16-21`), so the agent cannot tell whether it is in normal evidence mode or gate-fallback mode.

Alternatively, the agent can be expected to infer from the `directive` text, but that is fragile. A cleaner model: produce a dedicated response like `GateBlockedWithFallback` that includes both the blocking conditions and the `expects` schema. Or extend `GateBlocked` to carry an optional `expects` when a fallback is available, with `agent_actionable: true` on those conditions.

### 5. How does the agent know what fields to submit?

When a state has an `accepts` block and the engine reaches `StopReason::EvidenceRequired`, the CLI calls `derive_expects(final_template_state)` (`src/cli/mod.rs:805`) to build an `ExpectsSchema`.

`derive_expects` (`src/cli/next_types.rs:229-262`) maps each `FieldSchema` to an `ExpectsFieldSchema` and populates `options` from conditional transitions with `when` blocks. The agent receives:

```json
{
  "action": "execute",
  "state": "...",
  "directive": "...",
  "advanced": false,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "field_name": {"type": "enum", "required": true, "values": ["a", "b"]}
    },
    "options": [
      {"target": "next_state", "when": {"field_name": "a"}}
    ]
  },
  "error": null
}
```

The `options` list tells the agent which field values route to which states. This is the mechanism for the agent to understand the decision semantics.

### 6. Existing tests with both gates and accepts

Two tests in the compile module cover this co-existence:

- `command_gate_alongside_accepts_when` (`src/template/compile.rs:739`): compiles a state with a command gate and an `accepts` block together; asserts no error.
- `command_gate_still_works` (`src/template/types.rs:372`): validates a command gate on a state (without `accepts`, but confirms the gate type is accepted).

No integration test or engine unit test exercises the combination at runtime. There is no test that verifies what happens when a command gate fails on a state that also has an `accepts` block — the behavior (hard block, no evidence fallback) is only implied by the engine code.

---

## Implications for Design

**The fallback is not free.** Co-locating `gates` and `accepts` on a state is already valid YAML and compiles. But the engine currently treats gate failure as a hard stop regardless of what else is on the state. Adding fallback behavior requires either:

- An explicit opt-in (a flag on the state or gate), or
- An implicit convention: the presence of both `gates` and `accepts` means "gate is a fast path; evidence is the fallback."

The implicit convention is cleaner for template authors. It avoids a new YAML key and makes the semantics self-documenting: if you have gates and accepts, the gate is the optimistic path.

**The CLI output contract needs extending.** The current `GateBlocked` response has `expects: null` and `agent_actionable: false`. For fallback mode, the agent needs both the gate failure details (so it knows why it's being asked for evidence rather than having the state auto-advance) and the `expects` schema (so it knows what to submit). The cleanest change: when a state has both `gates` and `accepts`, set `agent_actionable: true` on blocking conditions and populate `expects` in the `GateBlocked` response. This avoids a new response variant.

**Evidence routing is already wired.** Once the engine falls through to transition resolution, the existing `resolve_transition` + `accepts` validation machinery handles everything. The `NeedsEvidence` stop reason maps directly to `EvidenceRequired`, and `derive_expects` correctly builds the schema from the `accepts` block. No changes needed in that path.

**The `rationale` field design is user-space.** The design asks for a `rationale: string` field on the `accepts` schema. This is just a regular `FieldSchema` with `field_type: "string"` and optionally `required: false`. No engine changes are needed to support it. Template authors declare it like any other field.

---

## Surprises

**`agent_actionable` is always false.** The current code hardcodes `agent_actionable: false` for all command gate blocking conditions in both `src/cli/next.rs:55` and `src/cli/mod.rs:826`. This was a deliberate design choice: the agent can't fix a failing CI check. But in the fallback model, the agent absolutely can act on a failing gate — it submits evidence as an override. The flag will need to be flipped to `true` when fallback mode is in use.

**No `EvidenceRequired` variant for gate-blocked states.** The design describes a smooth fallback: "gate fails → surface `expects` schema." But today `GateBlocked` and `EvidenceRequired` are entirely separate response variants with incompatible shapes. The `GateBlocked` variant (`next_types.rs:22-27`) has no `expects` field. This means the output contract needs modification — not just the engine.

**The old field-gate removal was intentional.** Comments in the codebase (`src/template/types.rs:146-148`, `src/template/compile.rs:255-258`) explain that `field_not_empty` and `field_equals` gate types were removed precisely because `accepts`/`when` replaced them. This history means the design space for gate-with-evidence-fallback was already considered at some level — command gates and evidence gates were consciously separated, with command gates kept as environmental checks.

**States without `accepts` that have command gates already work as hard gates.** If the design only adds fallback for states that have both `gates` and `accepts`, existing templates with gates-only states are unaffected. This is a clean, backward-compatible extension.

---

## Summary

Koto already supports co-locating command `gates` and `accepts` blocks on the same state (schema allows it, compiler accepts it, one compile test verifies it). However, the advancement engine treats gate failure as an unconditional hard stop — it never falls through to the `accepts` block. Implementing gate-with-evidence-fallback requires one engine change (allow gate failure to route to `NeedsEvidence` when the state has an `accepts` block), one CLI output change (extend `GateBlocked` to carry `expects` and set `agent_actionable: true` so the agent knows it can override), and a convention decision (implicit vs. explicit opt-in). The evidence submission path, schema surfacing, and `rationale` field support are already fully wired and require no changes.
