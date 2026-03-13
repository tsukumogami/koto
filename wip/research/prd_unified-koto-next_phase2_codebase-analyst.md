# Phase 2 Research: Codebase Analyst

## Lead 2: Gate model requirements for branching

### Findings

**Current gate data model** (`pkg/engine/types.go`):

Gates live on `MachineState`, not on individual transitions. The `MachineState` struct has a `Gates map[string]GateDecl` field — a flat map of gate name to gate declaration. There is no concept of "this gate guards transition X" vs. "this gate guards transition Y".

```go
type MachineState struct {
    Transitions []string
    Terminal    bool
    Gates       map[string]GateDecl
}
```

`GateDecl` has three gate types (`field_not_empty`, `field_equals`, `command`) but no `Target` field associating the gate with a specific outgoing transition.

**Current gate evaluation** (`pkg/engine/engine.go`):

`evaluateGates()` iterates all gates on the current state and applies AND logic — all gates must pass before any transition is allowed. There is no per-transition gate evaluation. The function returns pass/fail for the entire state's gate set.

```go
func (e *Engine) evaluateGates(state MachineState, evidence map[string]string) (bool, []string) {
    // iterates state.Gates — all or nothing
}
```

`Transition()` calls `evaluateGates()` once before accepting any transition target. If gates fail, the transition is rejected regardless of which target was requested.

**Template format** (`pkg/template/`):

The compiled template JSON schema has `additionalProperties: false` on `state_decl`. Gates compile from source YAML to a flat map on each state's compiled representation. No structural provision for transition-level gates exists.

**What transition-level gates require:**

To support branching via evidence, `Transitions` must become a structured list rather than a string slice:

```go
// Current
Transitions []string

// Required
Transitions []TransitionDecl
type TransitionDecl struct {
    Target string
    Gates  map[string]GateDecl  // gates specific to this transition
}
```

Gate evaluation must change from "evaluate all state gates, then check if target is valid" to "evaluate gates for the requested target transition specifically."

The template format (both source YAML and compiled JSON schema) must express this new structure. `format_version` may need bumping since the `transitions` field changes shape from a string array to an object array — the old and new formats are not silently compatible (unlike the additive tags change).

### Implications for Requirements

- R: The unified model requires transition-level gates. The PRD must specify that gates can be declared per-transition, not only per-state.
- R: The gate evaluation model must support "exactly one transition's gates satisfied at any time" as the mechanism for evidence-based branching.
- R: The template format must express transition-level gates. The PRD should state this requirement without prescribing the YAML syntax (that belongs in the design doc).
- R: The transition from state-level to transition-level gates is a breaking template format change. The PRD should acknowledge that existing templates will need migration (or the design doc must define a compatibility strategy).

### Open Questions

- Should state-level gates (as they exist today) be preserved as "gates that must pass before any transition is allowed," with transition-level gates layered on top? Or should state-level gates be deprecated entirely?
- What happens when a state has both state-level gates (legacy) and transition-level gates (new)? What's the evaluation order?

---

## Lead 4: Error and state feedback requirements

### Findings

**Current `Directive` struct** (`pkg/controller/controller.go`):

```go
type Directive struct {
    Action    string `json:"action"`
    State     string `json:"state"`
    Directive string `json:"directive,omitempty"`
    Message   string `json:"message,omitempty"`
}
```

No `expects` field. No gate status. No indication of what the agent should submit next. The agent cannot discover from the output alone whether the state expects evidence or what format it takes.

**Current error output** (`cmd/koto/main.go`):

For gate failures, the engine returns a `TransitionError` with a generic message. The CLI formats this as:
```json
{"error": "gates not satisfied: gate_name"}
```

The gate name is present but there's no detail about: which evidence field is required, what value was expected, what the agent actually provided, or whether the gate is a command gate (agent can't satisfy it directly) vs. a field gate (agent can submit evidence to satisfy it).

For invalid transitions:
```json
{"error": "invalid transition", "valid_transitions": ["plan", "escalate"]}
```

`valid_transitions` is returned — this is useful. But gate failure errors don't include equivalent structured data.

**What's missing for agents:**

1. **`expects` field on `Directive`**: tells the agent what the current state needs before gates will clear. For evidence gates: which fields, what format. For command gates: "koto will verify this; you don't need to submit anything." For transition-level gates: which transitions are available and what evidence satisfies each.

2. **Structured gate failure response**: when `koto next --submit <file>` fails because gates didn't clear after submission, the response must include: which gates are still unsatisfied, what they need, and what was provided.

3. **Auto-advancement signal**: under the unified model, `koto next` may return either the same directive (gates not yet clear) or a new directive (auto-advanced). The output must distinguish: "still in state X, waiting" vs. "advanced to state Y, here's the new directive." A new `advanced: bool` field or a distinct `op` field accomplishes this.

4. **Submission validation errors**: when `--submit <file>` provides data that fails schema validation (wrong shape, missing required fields), the error must distinguish this from a gate failure. The agent's recovery is different: fix the submission format vs. wait for conditions to change.

### Implications for Requirements

- R: `koto next` output must include an `expects` field describing what the current state accepts (if anything). When the state accepts no submission, `expects` is absent or null.
- R: Gate failure responses must include structured detail: which gates failed, what they require, and (for field gates) what was submitted.
- R: The output must signal whether `koto next` resulted in a state advance. Agents must not need to compare state names between calls to detect advancement.
- R: Submission validation errors (wrong format) must be distinguishable from gate evaluation failures (valid format, gates didn't clear).
- R: Command gates and field gates must be distinguishable in `expects` output — agents shouldn't try to submit evidence to satisfy a command gate.

### Open Questions

- Should `expects` be present on every read response, or only when the state has unsatisfied field gates?
- When auto-advancement occurs, does the response include both the previous state's completion and the new state's directive, or just the new directive?

---

## Summary

The current gate model is state-level AND-logic only, which cannot express branching — transition-level gates are required, and this is a breaking change to the template format and engine data model. The current error and feedback model lacks the structured output agents need: `koto next` must add an `expects` field for gate requirements, gate failure responses must include per-gate detail, and auto-advancement must be signaled explicitly rather than inferred by agents comparing state names across calls.
