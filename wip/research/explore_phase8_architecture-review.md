# Architecture Review: DESIGN-koto-template-format.md

Reviewer: architect-reviewer
Date: 2026-02-22

## Summary

The design is implementable and structurally sound. It respects the existing package boundaries, dependency direction, and extension points established by the engine design. Six findings below: two blocking issues that need resolution before implementation begins, and four advisory observations.

## Findings

### 1. BLOCKING: VariableDecl shorthand creates an unmarshaling problem with no specified solution

**Location:** Go Struct Definitions section, `VariableDecl` struct + shorthand description

The design says simple variables use shorthand `TASK = "default-value"` which should unmarshal as `{Default: "default-value"}`. But `VariableDecl` is a struct with three fields. When TOML encounters `TASK = "default-value"`, it will try to unmarshal a string into a `VariableDecl` struct and fail.

BurntSushi/toml doesn't support union types out of the box. The implementation will need either:

- A custom `UnmarshalTOML` method on `VariableDecl` that handles both string and table forms
- A two-pass parse: first attempt table unmarshal, then fall back to string
- An intermediate type (`map[string]interface{}`) with manual type-switching

The design should specify which approach to use, because this is the kind of implementation detail that produces different code shapes depending on the choice, and it touches the core parsing path. The `UnmarshalTOML` interface on `VariableDecl` is the cleanest option -- it keeps the complexity contained in the type.

**Recommendation:** Add a note that `VariableDecl` implements `toml.Unmarshaler` to handle the string shorthand. Provide the type-switch logic (string -> `VariableDecl{Default: s}`, table -> normal unmarshal).

### 2. BLOCKING: Evidence storage in engine.State is specified but the engine.Transition API has no way to accept evidence

**Location:** Interpolation Contract section, line "Evidence values are set via `koto transition <target> --evidence key=value`"

The design says evidence accumulates via `koto transition <target> --evidence key=value` and requires an `Evidence map[string]string` field on `engine.State`. But the current `Engine.Transition` method signature is:

```go
func (e *Engine) Transition(target string) error
```

The design doesn't specify how `Transition` accepts evidence. The options are:

- `Transition(target string, evidence map[string]string) error` -- breaking API change
- `Transition(target string, opts ...TransitionOption) error` -- functional options
- `SetEvidence(key, value string)` as a separate method before `Transition` -- separates concerns but introduces a two-step mutation

This is blocking because the Phase 4 implementation (Evidence Gate Types) depends on it, and the choice affects the engine's public API surface. Library consumers will import this.

The engine design explicitly says "evidence gates can be added without API changes" but that assumed evidence would only be checked, not supplied through the transition call. The design should specify the API change.

**Recommendation:** Specify the `TransitionOption` functional-options approach. It's backward-compatible (existing callers pass zero options) and extensible (future options like `WithTimeout` or `WithDryRun` fit the pattern). Define:

```go
type TransitionOption func(*transitionConfig)
func WithEvidence(evidence map[string]string) TransitionOption
```

### 3. ADVISORY: Gate evaluation timing is ambiguous for entry-vs-exit semantics

**Location:** Evidence Gate Declarations, paragraphs on gate evaluation

The design says "Gates are evaluated when entering a state (before the transition commits)" but the gate declarations are attached to the source state (the state you're leaving). This creates confusion: are gates on `states.plan` checked when entering `plan` or when leaving `plan`?

The TOML example has `[states.assess.gates.task_defined]` checking `field_not_empty` for `TASK`. If gates are evaluated on entry to `assess`, the check happens before the agent has done any work. If gates are evaluated on exit from `assess`, the check confirms work was done before leaving.

The contextual evidence (the alternative considered says "this state requires these conditions before you can leave it") and the description of how transition-level gates were rejected strongly suggest exit semantics. But the "entering a state" language contradicts this.

**Recommendation:** Clarify: "Gates on a state are evaluated when attempting to leave that state (before the transition to the target commits)." This matches the intended semantics and aligns with the design's own rejected-alternative rationale.

### 4. ADVISORY: Phase sequencing has a dependency gap between Phase 2 and Phase 4

**Location:** Implementation Approach, Phases 1-5

Phase 2 (Declared-State Section Parsing) updates `parseSections` to use the declared state set from the TOML header. Phase 4 (Evidence Gate Types) adds gate evaluation to the engine. But the `TemplateConfig` struct from Phase 1 already includes `States map[string]StateDecl` with nested `Gates map[string]GateDecl` -- meaning Phase 1 parses gate declarations, Phase 2 uses state names for section parsing, and Phase 4 evaluates gates.

The gap: Phase 1 parses gates into `GateDecl` structs, but these never reach `engine.MachineState` until Phase 4. Between Phases 1-3, the parsed `StateDecl.Gates` are silently discarded when building the `engine.Machine`. This isn't wrong -- it's a natural consequence of phased delivery -- but it means parse-time validation of gate declarations (Phase 1) will produce `GateDecl` structs that are validated but never consumed. An implementer who doesn't read all five phases will wonder why.

**Recommendation:** Add a note in Phase 1 that gate declarations are parsed and validated but not wired into `engine.MachineState` until Phase 4. The `Template` struct should carry the gate data (`TemplateConfig` or a derived form) so Phase 4 can wire it without re-parsing.

### 5. ADVISORY: The `validate` command semantics change silently

**Location:** Validation Contract section + current CLI implementation

The current `koto validate` only checks template hash integrity (template on disk matches stored hash). The design redefines `koto validate --template` to run semantic checks (reachability, cross-references) on a template file independent of any state file. These are two different operations:

- Current: "is my running workflow's template intact?" (requires state file)
- Proposed: "is this template well-formed?" (requires template file, no state file)

Both are useful. The design doesn't address whether the existing hash-check behavior moves to a different command or coexists.

**Recommendation:** Keep both. `koto validate` (with state file) does hash integrity check (existing behavior). `koto validate --template path.md` (no state file) does structural + semantic validation of a template file. The flag presence determines the mode.

### 6. ADVISORY: Schema version increment needs specification

**Location:** Interpolation Contract section, line "The state file schema version increments to accommodate this field"

The design says `schema_version` increments when `Evidence` is added to `engine.State`, but doesn't specify:

- What the new version number is (presumably 2)
- Whether the engine can load schema_version 1 files (backward compatibility)
- Whether loading a v1 file auto-migrates (adds empty Evidence map) or errors

Since koto has no public release, this is low-stakes. But the engine design established `schema_version` as a contract field, and the first consumer of that field should establish the migration pattern.

**Recommendation:** State that schema_version becomes 2 when Evidence is added. `engine.Load` accepts both v1 (no Evidence field, treated as empty map) and v2. `engine.Init` always writes v2. This establishes the migration pattern for future schema changes.

## Questions Addressed

### 1. Is the architecture clear enough to implement?

Yes, with the two caveats above. The Go struct definitions, TOML schema, and parsing rules are specific enough to write code from. The validation rules table is particularly well-done -- each check has an exact error message format. The `VariableDecl` shorthand unmarshal needs a specified implementation approach (Finding 1).

### 2. Are there missing interfaces between pkg/template/ and pkg/engine/?

The main gap is how evidence flows from CLI through engine to state file (Finding 2). The `Template` struct currently returns `Machine` and `Sections` and `Variables`. The design adds gate declarations to the TOML parse but doesn't show how parsed `GateDecl` values reach `engine.MachineState`. The `MachineState` struct currently has only `Transitions []string` and `Terminal bool` -- it needs a `Gates` field. The design implies this ("extend `MachineState`") but the extended struct isn't shown.

The controller needs an updated interpolation context that merges variables + evidence. Currently `controller.Next()` calls `template.Interpolate(section, c.eng.Variables())`. After the change, it needs `template.Interpolate(section, mergedContext)` where `mergedContext` includes `c.eng.Evidence()`. This is straightforward but the `Engine.Evidence()` accessor method isn't specified.

### 3. Are the implementation phases correctly sequenced?

Mostly. Phase 1 (TOML parser) can work independently -- it replaces `parseHeader` and `splitFrontMatter` while keeping the same `Template` struct output. Phase 2 depends on Phase 1 (needs parsed state names). Phase 3 (search path) is independent of Phases 1-2 since it lives in `cmd/koto/`. Phase 4 depends on Phases 1-2 (needs parsed gates from TOML, needs state names for section parsing). Phase 5 depends on everything.

One optimization: Phase 3 could be done in parallel with Phases 1-2 since it's purely CLI-layer code with no `pkg/template/` changes.

### 4. Are there simpler alternatives we overlooked?

The declared-state matching for heading collision resolution is elegant and simple. No simpler alternative exists that preserves templates as valid markdown.

For evidence storage, an alternative to a separate `Evidence` map would be merging evidence into the existing `Variables` map with a prefix convention (e.g., `evidence.tests_pass`). This avoids a schema change but pollutes the variable namespace. The separate map is the right call.

The three gate types (field_not_empty, field_equals, command) are the right starting set. A `file_exists` gate type could be useful (many workflows check for artifacts) but can be implemented as a `command` gate (`test -f path`) so it doesn't need to be a built-in type.

### 5. Does initial_state integrate cleanly with existing engine.Machine construction?

Yes. The current code sets `Machine.InitialState = stateNames[0]` (first heading). The design replaces this with `Machine.InitialState = config.InitialState` (from TOML). The `Machine` struct already has the `InitialState` field. The engine's `Init` method validates `machine.States[machine.InitialState]` exists. Clean integration, no structural changes needed.

### 6. Is the evidence storage extension well enough specified?

Partially. The `Evidence map[string]string` field on `engine.State` is clear. The accumulation semantics (evidence persists across transitions) are clear. The interpolation precedence (evidence wins over variables) is clear.

What's missing: how evidence is supplied (Finding 2), the `Engine.Evidence()` accessor method, and schema migration (Finding 6). The design also doesn't specify what happens to evidence on rewind -- is it preserved (full audit trail), cleared (reset to match the target state), or selectively cleared (remove evidence added after the rewind target)? The engine design's rewind section notes "when evidence gates are added, rewind semantics will need to account for evidence cleanup." This design should at minimum state the Phase 1 behavior (probably: evidence preserved on rewind, cleanup deferred to a later design).
