# Phase 3 Research: Controller Loop and Stopping Conditions

## Questions Investigated

1. What does `controller.go` currently look like? What methods, what state, what dependencies?
2. How does the existing `Next()` method determine the current state and build a `Directive`?
3. What config does the controller hold that the advancement loop will need? (delegation rules, template path, integration config)
4. The existing `DelegateChecker` interface pattern — what does it look like? How is it injected? This is the pattern `IntegrationRunner` should follow.
5. What does a `Directive` currently look like? What fields? How would `AdvanceResult` differ from/extend it?
6. How does the controller currently load the engine? (constructor pattern, engine options)
7. What would the stopping condition for "processing integration" look like in practice? How does the controller know whether the current state has a processing integration configured?

## Findings

### Controller Structure (pkg/controller/controller.go)

The controller is minimal and focused:
- **Type**: `Controller` struct holding two fields:
  - `eng *engine.Engine`: the state machine engine
  - `tmpl *template.Template`: optional parsed template (file:16-17)
- **Methods**: Only two public methods:
  - `New(eng, tmpl)`: Constructor with template hash verification (file:31-44)
  - `Next()`: Returns a single `Directive` (file:51-90)
- **No existing fields for**: delegation rules, integration config, or external dependencies beyond engine and template

### Directive Type (pkg/controller/controller.go)

Directive structure is simple:
```go
type Directive struct {
  Action    string // "execute" or "done"
  State     string // current state name
  Directive string // instruction text (execute only)
  Message   string // completion message (done only)
}
```
(file:20-25)

**Key observations**:
- Only two action types: "execute" or "done"
- No fields for gate status, next-state hints, or expectations
- No HATEOAS-style schema of allowed transitions
- `Next()` returns a flat structure with no metadata about what state is coming next

### Next() Method Implementation (file:51-90)

The method:
1. Calls `c.eng.CurrentState()` to get current state name
2. Looks up the state in `machine.States[current]`
3. Checks `ms.Terminal` flag:
   - If terminal: returns `Directive{Action: "done", State: current, Message: "workflow complete"}`
   - If not terminal: builds directive text and returns `Directive{Action: "execute", State: current, Directive: ...}`
4. Directive text comes from `c.tmpl.Sections[current]` with template interpolation (file:73-82)
5. Falls back to generic stub if no template provided

**No stopping-condition logic exists**: the method has no knowledge of gates, integrations, or advancement loops.

### Template and MachineState Types

**Template** (pkg/template/template.go, file:35-44):
- `Sections map[string]string`: state name → raw markdown content
- `Variables map[string]string`: default variable values
- `Hash string`: SHA-256 of template file
- `Machine *engine.Machine`: parsed state machine definition

**MachineState** (pkg/engine/types.go, file:46-50):
```go
type MachineState struct {
  Transitions []string
  Terminal    bool
  Gates       map[string]*GateDecl
}
```

**Key finding**: `MachineState` already has a `Gates` field for exit conditions, but:
- No field for "processing integration" configuration
- No field for delegation/delegation checking
- Gates are only evaluated at transition time (engine.go, file:190-206)

### GateDecl Type (pkg/engine/types.go, file:54-60)

Gates support three types:
- `"field_not_empty"`: evidence key must be non-empty
- `"field_equals"`: evidence key must equal a specific value
- `"command"`: shell command must exit 0

Timeout optional (defaults to 30s).

**Does NOT support**:
- Integration-based gates
- Processing directives
- Delegation checks

### No DelegateChecker Pattern Found

**Grep search result**: No `DelegateChecker` interface exists in the current codebase. The design document proposes injecting `IntegrationRunner` following a pattern that doesn't yet exist in the code.

This means the pattern must be designed as part of Phase 3.

### Engine Construction Pattern (pkg/engine/engine.go)

Engine is constructed via:
- `Init(statePath, machine, meta)`: Creates new workflow (file:53-92)
- `Load(statePath, machine)`: Loads existing workflow (file:95-127)

**No factory pattern or options-builder pattern** — both are direct constructors. The machine definition is passed explicitly; there's no engine-held config.

### Integration-Related Fields: None Currently Exist

**State** type (pkg/engine/types.go, file:9-17):
- `SchemaVersion`, `Workflow`, `Version`, `CurrentState`, `Variables`, `Evidence`, `History`
- No fields for integration config, delegation rules, or processing state

**MachineState** type:
- `Transitions`, `Terminal`, `Gates`
- No fields for: integration_type, delegation_rule, processing, requires_delegation, etc.

This means integration config must be designed and added to either `MachineState` or to a new type layer above it.

### Controller Does NOT Load Engine

The controller does **not** construct the engine — it receives an already-loaded `*engine.Engine` from the caller (file:31-44). This pattern is correct for Phase 3: the `Advance()` loop in the controller will receive fully-loaded state, but the stopping-condition logic needs to inspect `MachineState` for integration metadata that doesn't yet exist.

### What AdvanceOpts Should Contain (Inference)

Based on existing `TransitionOption` pattern (engine.go, file:38-49):
- `WithEvidence(map)` already exists
- `AdvanceOpts` should likely include:
  - `To string`: optional directed transition (when agent picks a specific next state)
  - `WithData map[string]string`: evidence to attach to each transition
  - Possibly: stopping behavior options (e.g., "StopAtProcessingIntegration bool")

### Stopping Conditions: How to Detect Them

From the design document and engine code:

1. **Terminal state**: Check `machine.States[currentState].Terminal` (already works)
2. **Unsatisfied gate**: Attempt `Transition()`, catch `ErrGateFailed` (already works)
3. **Processing integration**: Would need:
   - A new field on `MachineState` (e.g., `Processing string` or `ProcessingIntegration *IntegrationConfig`)
   - Checked in `Advance()` before calling `Transition()`: `if machine.States[current].Processing != "" { return ... }`
4. **Visited-state cycle**: Track visited states in a local `map[string]bool` per `Advance()` call

### Evidence Field: Accumulation Semantics

From engine.go (file:224-231):
- Evidence is merged into state at each transition
- New keys added, existing keys overwritten
- Recorded in history entry for audit trail
- **Clearing atomically**: Would need to happen in `Controller.Advance()` after detecting a processing integration, before returning

## Implications for Design

1. **No DelegateChecker pattern exists yet** — it must be invented as part of Phase 3. The suggested approach of injecting an interface is sound but unmarked in code.

2. **IntegrationRunner interface is completely new** — no existing equivalent. It should follow the injection pattern by being a parameter to `Controller.New()` or `Controller.Advance()`.

3. **MachineState needs integration metadata** — currently has only `Transitions`, `Terminal`, `Gates`. Must add:
   - Field identifying the integration type (string name or pointer to IntegrationConfig)
   - Possibly: integration-specific parameters

4. **Template format needs integration declarations** — currently YAML only specifies state name, transitions, and gates. Must add syntax for marking a state as "processing" and specifying the integration type.

5. **Directive type may be sufficient as-is** — or `AdvanceResult` could extend it with `NextState` and `StoppingReason` fields.

6. **AdvanceOpts should follow TransitionOption pattern** — functional options that build a config struct before the loop executes.

7. **Evidence clearing is straightforward** — after detecting a processing integration, call `Engine.Evidence()` to copy the current evidence, then use it somewhere safe (log, pass to integration runner, or return in `AdvanceResult`).

## Surprises

1. **No interface or dependency injection yet** — the controller is purely structural; all external behavior comes via passed-in engine and template. Injecting `IntegrationRunner` is a new concept.

2. **Gates already exist and work for evidence-based stops** — the design document mentions evidence gates as "Phase 2," but they're already implemented and functional (field_not_empty, field_equals, command types). This means Phase 3 can reuse the gate mechanism for parts of the advancement logic.

3. **No state-machine options or configuration object** — the engine is fully constructed before the controller sees it. This is clean but means all integration config must live in the template or be injected at a higher layer.

4. **Controller.Next() is read-only** — it never mutates engine state, only reads and formats. This is good: `Advance()` will do all mutations while `Next()` stays a pure query.

## Summary

The controller is deliberately minimal: `Engine` + optional `Template`, two public methods (`New`, `Next`). There is no existing `DelegateChecker` interface or integration config; both must be designed from scratch in Phase 3. `MachineState` already supports gates but lacks fields for "processing integration" metadata. The `Next()` method determines the current state and returns a flat `Directive` with no metadata about expected stops or delegation requirements; `Advance()` will add the loop logic, stopping-condition checking, and integration runner invocation.

