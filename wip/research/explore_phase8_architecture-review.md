# Architecture Review: DESIGN-koto-engine.md

**Reviewer**: architect-reviewer
**Date**: 2026-02-21
**Document**: `docs/designs/DESIGN-koto-engine.md`
**Scope**: Solution Architecture + Implementation Approach sections

---

## 1. Is the architecture clear enough to implement?

**Yes, with two gaps that will cause implementation ambiguity.**

The design provides concrete Go type definitions, a well-specified transition validation sequence (8 steps), and clear package boundaries with stated import directions. An implementer can start writing code from what's here. Two areas will force implementation-time decisions that should be made in the design instead.

### Gap 1: Machine construction ownership

The design says `template.Parse()` returns a `*Template` that contains a `*engine.Machine`. But it doesn't specify who constructs the `Machine` -- does `Parse()` build it directly, or does it return intermediate types that the caller assembles into a `Machine`?

This matters because the `Machine` struct contains `Gates map[string]Gate`, where `Gate` is an interface. The template parser needs to know about all gate types to instantiate the right implementation from YAML declarations. That means `pkg/template/` must import every built-in gate type from `pkg/engine/`, creating a coupling between the template format and the engine's gate registry.

**Recommendation**: Define a gate construction function or map in `pkg/engine/` that the template parser can call:

```go
// In pkg/engine/
var GateBuilders = map[string]func(config map[string]string) (Gate, error){
    "field_not_empty": newFieldNotEmpty,
    "field_equals":    newFieldEquals,
    "command":         newCommand,
}
```

This gives library consumers a way to register custom gates that templates can reference, and keeps the template parser from needing to know the concrete gate types.

### Gap 2: Template file format

The design explicitly marks the template format as out of scope ("Template format specification (separate design)"), but `pkg/template/` is Phase 1, and `Parse()` is its primary function. The implementer needs to know the YAML header schema to write the parser. The design mentions "YAML header + markdown body" and "## STATE: sections" but doesn't define the YAML schema for states, transitions, gates, or variables.

This is a sequencing problem: you can't implement `Parse()` without the template format spec. Either the template design needs to ship before or alongside this one, or this design should include enough of the format to make the parser implementable.

**Recommendation**: At minimum, include a short example template showing the YAML header schema (states, transitions, gates, variables, metadata) and one `## STATE:` section body. This doesn't need to be a full template format spec -- just enough for the parser implementation to proceed.

---

## 2. Are there missing components or interfaces?

### 2a. Cancel operation

The design mentions `koto cancel` twice (in the template hash verification discussion and in the CLI subcommand list) but doesn't define what it does. Questions:
- Does it delete the state file?
- Does it write a terminal history entry before deleting?
- Does it add a `cancelled` terminal state, or just remove the file?

The implementation needs this specification. A `Cancel` method on `Engine` should be defined alongside `Transition` and `Rewind`.

### 2b. Status/query operations

The CLI subcommand list includes `query` and `status` but neither has an API definition. The engine has `CurrentState()`, `Evidence()`, `Variables()`, `History()`, and `Snapshot()` -- these cover raw data access. But the design doesn't specify what `koto status` outputs (is it a human-friendly summary, or structured JSON like `Snapshot()`?) or what `koto query` accepts as arguments.

For implementability, these are secondary to the core engine, but they should be specified before the CLI phase.

### 2c. Validate operation

The CLI lists `validate` but there's no `Validate` method on Engine or any specification of what it checks. The predecessor has a `Validate` function that checks required fields, valid statuses, and integrity hash. The koto equivalent would check: state references a valid machine state, evidence keys match expected types, version counter is positive, history is internally consistent. This needs a specification.

### 2d. Init parameters for workflows without templates

The `Init` function takes a `*Machine` and `InitMeta`. For library consumers who programmatically define state machines (no template file), `InitMeta.TemplateHash` and `InitMeta.TemplatePath` are meaningless. The design should note whether these can be empty for programmatic use, and whether template hash verification is skipped when the hash is empty.

---

## 3. Are the implementation phases correctly sequenced?

**Mostly yes. One dependency inversion.**

The sequencing is: engine -> template -> controller -> discover -> CLI. The import direction (`template` imports `engine`, `controller` imports both) confirms this order works.

### Problem: template before controller creates a testing gap

The design places `pkg/template/` second and `pkg/controller/` third. But the controller is the first consumer that actually exercises the template's `Machine` output -- without the controller, there's no way to integration-test that the `Machine` produced by `Parse()` works correctly with the engine's `Transition()` logic. Unit tests on the template parser will verify structure, but won't catch semantic mismatches (e.g., a template that produces gate names the engine doesn't recognize).

This isn't blocking because unit tests can use hand-constructed `Machine` instances to test the engine, and template unit tests can verify the output structure. But the integration gap should be acknowledged: the first real integration test happens when the controller is built.

### Discover ordering is correct

`pkg/discover/` depends only on reading state file headers. Placing it after controller is fine. It could even be built in parallel with controller since they don't depend on each other.

### CLI last is correct

The CLI is a thin wrapper. Building it last means all packages are available for integration testing.

---

## 4. Are there simpler alternatives we overlooked?

### 4a. Evidence as part of transition request vs. pre-merged

The current design merges evidence into the state before evaluating gates, then rolls back on failure. An alternative: evaluate gates against the *proposed* evidence (current evidence map + new evidence) without modifying the state, then merge only on success. This avoids the rollback path entirely.

```go
// Instead of merge-then-rollback:
proposed := mergeMaps(e.state.Evidence, newEvidence)
for _, gate := range targetState.Gates {
    if err := gate.Check(proposed); err != nil {
        return err  // no rollback needed, state was never modified
    }
}
// Gates passed, now merge for real
e.state.Evidence = proposed
```

This is functionally identical but eliminates an error-prone code path (the rollback). The design's approach works, but the simpler alternative should be considered during implementation.

### 4b. Template hash verification placement

The design says template hash is verified on every `koto next` and `koto transition`. The verification happens in two places: the controller's `New()` constructor checks it for `next`, and step 1 of the transition sequence checks it for `transition`.

A simpler alternative: verify the template hash once in `Engine.Load()` instead of on every operation. Since the engine holds the hash and the template file path, it can verify at load time. This centralizes the check and means every operation that uses a loaded engine is automatically protected.

The tradeoff is that a template modified after `Load()` but before `Transition()` wouldn't be caught. Given that koto is invoked as a short-lived CLI process (load, do one thing, exit), this window is negligible.

### 4c. The command gate is a significant complexity addition

The `Command` gate type runs `sh -c` during transition evaluation. This adds:
- Process spawning during what should be a data-validation step
- Timeout concerns (no timeout enforced, noted in security section)
- Platform-specific behavior (`sh` varies across systems)
- A trust boundary (templates can execute arbitrary commands)

For Phase 1, consider deferring the `command` gate. The `field_not_empty` and `field_equals` gates cover the evidence patterns in all current workflows. The `command` gate adds power but also adds security surface and implementation complexity that isn't needed for the initial release.

If the command gate stays, the design should specify: Does the command receive any arguments or environment variables (e.g., the evidence map)? Is it run from the state file's directory or the current working directory? What happens if the command hangs?

---

## 5. Go type definition review

### 5a. Gates map key semantics

`MachineState.Gates` is `map[string]Gate`. The string key is presumably the gate name (used in error messages). But the design doesn't say what the key represents or who sets it. In the template YAML, is the key the evidence field name, a human-readable label, or an arbitrary identifier?

This matters for the `gate_failed` error, which includes a `Gate` field. If the key is the evidence field name, the agent knows what to provide. If it's a human label, the agent needs the `gate_type` and additional context.

**Recommendation**: Specify that the map key is the evidence field name for `field_not_empty` and `field_equals` gates, and an arbitrary identifier for `command` gates. Document this in the `MachineState` struct comment.

### 5b. TransitionError is an error type but doesn't implement error

The design shows `TransitionError` as a struct with JSON tags but doesn't show an `Error() string` method. For it to work with Go's error handling (`if err != nil`, `errors.As`), it needs:

```go
func (e *TransitionError) Error() string {
    return e.Message
}
```

This is likely assumed but should be explicit, especially since the design specifies that "all engine errors implement a TransitionError type."

### 5c. Timestamp type

`HistoryEntry.Timestamp` and `WorkflowMeta.CreatedAt` are `string`. This works for JSON serialization but loses type safety in Go code. Consider using `time.Time` with a custom JSON marshaler, or at minimum document the expected format (RFC 3339).

The predecessor uses no timestamps at all, so this is a new concern. RFC 3339 (`time.RFC3339`) is the standard choice and should be specified.

### 5d. Machine has no validation method

The `Machine` struct is constructed by the template parser and passed to the engine. If the parser produces an invalid machine (e.g., a state references a transition target that doesn't exist in the `States` map, or the `InitialState` isn't in `States`), the engine will discover this at runtime.

A `Machine.Validate() error` method that checks internal consistency would catch these problems at parse time rather than at first transition. The design mentions that `Parse()` "returns an error if states reference undefined transitions" -- this implies validation happens in the parser, but centralizing it on `Machine` makes it available to library consumers who construct machines programmatically.

### 5e. No circular import risk

The import graph is:
```
engine (no imports from pkg/)
template -> engine
controller -> engine, template
discover -> engine
cmd/koto -> engine, template, controller, discover
```

This is a clean DAG. No circular dependency risk. The design got this right.

### 5f. Engine.Transition return type

`Engine.Transition()` returns `error`. The predecessor's `Transition()` returns `(int, error)` -- the issue number that was transitioned. In koto, there's no issue concept, so `error` alone is correct. But the caller might want to know the resulting state for confirmation logging. Consider returning `(*HistoryEntry, error)` or at minimum noting that callers should use `CurrentState()` after a successful transition.

---

## 6. Structural alignment with predecessor

The design addresses every anti-pattern identified in the predecessor:

| Predecessor problem | koto solution | Assessment |
|---|---|---|
| `os.WriteFile` (non-atomic) | write-to-temp-then-rename with fsync | Correct fix |
| No transition history | `[]HistoryEntry` with chronological append | Correct fix |
| No template hash | SHA-256 at init, verified on every operation | Correct fix |
| Dual evidence validation | Single path: gates on target state | Correct fix |
| Hardcoded variable allowlist | Variables from template + evidence map | Correct fix |
| Type-specific transition tables | Single machine from template | Correct fix |

The design also correctly identified and discarded patterns that work well in the predecessor (transition maps, evidence-as-contract) by keeping their essence while generalizing them.

---

## 7. Predecessor patterns NOT carried forward (verify intentional)

### 7a. Dependency graph / multi-issue orchestration

The predecessor's controller has significant dependency graph logic (computing blocked-by relationships, auto-skipping blocked issues, finding the next actionable issue). The koto design has none of this -- it's a single-workflow state machine.

This is correct for Phase 1. Multi-issue orchestration is a controller-level concern that can be built on top of the engine without modifying it. But the design should acknowledge this as a deliberate scope reduction, since users migrating from the predecessor will notice its absence.

### 7b. Force flag

The predecessor has `--force` to bypass transition and evidence validation. The koto design has no force flag and explicitly says "no override flag" for template hash verification. This is a deliberate design choice (noted in the decision outcome) that aligns with koto's enforcement philosophy. Correct.

### 7c. PR-level state machine

The predecessor has a two-level state machine (issue-level and PR-level). koto has a single flat state machine. The two-level model is a specific workflow pattern, not a general engine concern. Correct to exclude from the engine design.

### 7d. Bookkeeping verification

The predecessor has a pre-transition verification step that reads external files (PR body, design doc, test plan). koto's `command` gate type subsumes this: a template can define a command gate that runs verification logic. This is a cleaner separation. Correct.

---

## 8. Summary of findings

### Blocking (must resolve before implementation)

1. **Template format not specified**: `pkg/template/Parse()` is Phase 1 but the template YAML schema is out of scope. Either include enough of the format to implement the parser, or sequence the template format design to ship first.

2. **Cancel operation undefined**: Referenced twice, listed in CLI subcommands, but has no API definition or behavioral specification.

### Advisory (resolve during implementation)

3. **Gate construction pattern**: Define a builder map or factory function so template parser and library consumers can construct gates without knowing concrete types. Avoids tight coupling.

4. **TransitionError needs Error() method**: The design shows the struct but not the interface implementation. Explicit is better.

5. **Machine.Validate() method**: Centralizes consistency checking for both parser-constructed and programmatically-constructed machines.

6. **Evidence merge simplification**: Evaluate gates against proposed evidence instead of merge-then-rollback. Same semantics, no rollback path.

7. **Timestamp format**: Specify RFC 3339 for `Timestamp` and `CreatedAt` fields.

8. **Gates map key semantics**: Document what the `map[string]Gate` key represents.

9. **Command gate deferral**: Consider deferring to a later phase. Adds security surface and platform complexity for a capability no current workflow uses. `field_not_empty` and `field_equals` cover all existing evidence patterns.

### Out of scope (noted for completeness)

10. **Multi-issue orchestration**: Correctly excluded. Can be built on top of the engine later.

11. **Status/query/validate CLI commands**: Need specification before CLI implementation phase, but don't affect engine design.
