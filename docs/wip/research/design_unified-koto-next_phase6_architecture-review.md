# Architecture Review: Unified koto next Command

**Design:** DESIGN-unified-koto-next.md
**Reviewer role:** Architect (structural fit, not correctness or style)
**Date:** 2026-03-13

---

## Review Summary

The design is structurally sound. The chosen approach (controller-owned advancement loop) fits
the existing package layering correctly. The engine stays a single-transaction executor, the
controller takes the orchestration role it already holds, and no new packages are introduced.

The primary structural concern is not the overall shape — it is a set of specific under-specified
interfaces and missing pieces that, if left to implementers, will produce inconsistent decisions
across the four phases. These are called out below.

---

## Question-by-Question Findings

### 1. Is the architecture clear enough to implement? Missing components or interfaces?

**Mostly yes, with three gaps.**

**Gap A — `WithEvidence` injection scope is ambiguous (Advisory).**
The design says `Advance` injects evidence "into the first Transition() call via WithEvidence
option" (data flow diagram). But `AdvanceOpts.WithData` is `map[string]string` and the loop
may advance through multiple states. The current `WithEvidence` option (engine.go:45) merges
keys into `State.Evidence` which is then cleared on the next transition. The design's stated
intent is that `--with-data` evidence applies to the first transition only, then auto-advancement
continues with empty evidence. That semantic is correct given per-state scoping, but the design
doesn't say where the `visited[current] = true` guard should sit relative to the evidence
injection point. If evidence is consumed and the gate still fails, does `Advance` return
`StopGateBlocked` immediately, or does it retry? The pseudocode implies fail-fast (return
`StopGateBlocked`), but this needs to be explicit in the contract.

**Gap B — `AdvanceResult` is missing the integration output field (Blocking).**
The data flow for processing integration stop says the result includes "integration output,"
and the security section says the CLI formats output with `expects: {submit_with: "--with-data"}`.
But `AdvanceResult` as specified only has `{Directive, StoppedBecause, Advanced bool}`. There is
no field to carry the `map[string]string` returned by `IntegrationRunner.Run()`. The implementer
must add a field — e.g., `IntegrationData map[string]string` — but the design doesn't name it,
leaving the CLI formatter without a specified field to read.

**Gap C — No `Engine` interface for controller injection (Advisory).**
The design says `Controller.Advance()` is "unit-testable by injecting a mock engine." The current
`Controller` struct holds a concrete `*engine.Engine`, not an interface. The advocate report says
"inject mock engine" but neither the design nor the advocate specifies the interface surface that
mock engines would implement. The implementation will need to either extract an interface
(`Transitioner`? `EngineI`?) or accept concrete types and test through a real in-memory engine.
Either is viable architecturally, but the decision should be explicit before Phase 3 starts;
retrofitting an interface after Phase 3 is written touches every controller test.

---

### 2. Are the implementation phases correctly sequenced?

**Yes, with one dependency the sequencing implicitly handles but doesn't state.**

Phase 1 (engine type model) → Phase 2 (template format) → Phase 3 (controller) → Phase 4 (CLI)
is correct. No phase depends on a later phase's output.

One undocumented intra-phase dependency: `ParseJSON` in `compiled.go` currently rejects any
`format_version != 1` (line 49 of compiled.go). Phase 2 must update `ParseJSON` to accept
version 2 and reject version 1 with the migration message. The design mentions the format version
bump but doesn't explicitly call out that `ParseJSON` is the rejection gate — an implementer
focused on `compile.go` might miss it. The `loadTemplateFromState` function in `main.go` calls
`ParseJSON` on every command, so a v2 compiled template will fail all commands until `ParseJSON`
is updated. This is a sequencing note, not a blocker, since it's within Phase 2.

The `Machine()` deep copy in `engine.go` (lines 374-414) copies `Transitions []string` with
`copy()`. After Phase 1 changes `Transitions` to `[]TransitionDecl`, this copy logic must be
updated. The design doesn't call this out as a deliverable in Phase 1. It won't cause data
races (deep copy is defensive), but a shallow copy of a slice of structs containing a map
(`Gates map[string]*GateDecl`) would alias the gate pointers. The Phase 1 deliverables list
should include `Engine.Machine()` copy logic.

---

### 3. Design gaps in stopping conditions and evidence clearing?

**One structural gap: the interaction between `--with-data` and evidence clearing when
gate evaluation fails.**

The design specifies evidence is cleared "atomically with each transition commit." Clearing
happens inside `Engine.Transition()` at the persist step. If a gate fails, `Transition()` returns
an error before persisting, so evidence is not cleared — this is correct. But `Advance` injects
the `--with-data` evidence via `WithEvidence` as a `TransitionOption`, which means the evidence
only exists for that one `Transition()` call; it is never written to `State.Evidence` on a failed
gate. This is correct for the in-memory path.

The gap is a subtler case: what if `--with-data` is provided and the first transition succeeds
(consuming the evidence), but a subsequent auto-advance step encounters a gate that needed the
same evidence? The design says "auto-advancement continues from new state with empty evidence" —
this is the correct and intended behavior for per-state scoping. But the design doesn't say this
explicitly in the stopping-conditions section, only in the data flow diagram. The evidence
archiving path in `HistoryEntry` stores the `WithEvidence` keys (per engine.go:217-222), which
means the history does record what was submitted, even though the live evidence is cleared.
That is structurally correct and consistent.

The `--with-data` and `--to` combined case is not described. If both flags are provided: does
directed transition also inject evidence? The pseudocode says directed transitions skip gate
evaluation entirely, which means evidence would be injected into the transition but gates are
bypassed. The history entry would record the evidence even though gates didn't evaluate it.
This is probably the right behavior (evidence is still archived), but it needs a one-line
statement in the design.

---

### 4. Is the `IntegrationRunner` interface contract complete enough to implement against?

**Mostly, but two callsite behaviors are missing.**

The interface is:
```go
type IntegrationRunner interface {
    Run(integrationName string, state engine.State) (map[string]string, error)
}
```

What's missing:

**Missing: error handling contract.** If `Run` returns an error, what does `Advance` do? Return
`StopProcessingIntegration` with an error, or propagate the error directly to the caller? The
current design says "return `StopProcessingIntegration` with result" — but that is for the
success case. An integration runner error could mean "subprocess unavailable" (transient) or
"integration rejected the state" (permanent). Whether `Advance` surfaces this as a structured
`AdvanceResult` (with a stop reason) or as a raw error changes what the CLI must handle. This
needs one explicit statement.

**Missing: who provides the concrete `IntegrationRunner` at `New()`.** The design says
"implementations live in `internal/` or `cmd/`." The CLI (Phase 4) wires the concrete runner,
but Phase 3 must write the controller to accept it. The design doesn't say what the no-op or nil
case is: can `runner` be `nil` (fail if a state has `Processing != ""`), or must a no-op
implementation always be injected? This matters for Phase 3 tests that don't exercise
integrations — they need to know whether to inject a no-op runner or leave `runner` as nil.

The `state engine.State` parameter is the snapshot value type. This is fine — `Engine.Snapshot()`
already returns a `State` value. The implementer will need to call `eng.Snapshot()` before
calling `runner.Run()`, which requires `Engine` to expose `Snapshot()` through whatever interface
the controller holds.

---

### 5. Are `TransitionDecl` and `MachineState.Processing` fully specified?

**`TransitionDecl` is fully specified. `MachineState.Processing` has one gap.**

`TransitionDecl{Target string, Gates map[string]*GateDecl}` is clear. The two-phase gate
evaluation order (shared gates first, then per-transition gates) is explicit, and "fail fast"
means the first failing gate aborts. This is implementable.

`MachineState.Processing string` gap: the design doesn't specify what the value represents
at the engine layer. It says "name of processing integration to invoke," but the engine stores
it as a plain string with no validation against a registry. The controller resolves the name via
`IntegrationRunner.Run(ms.Processing, ...)`. This means:

1. The engine accepts any non-empty string for `Processing` — there is no validation at compile
   time that the name corresponds to a registered integration. Template compilation could warn
   on unrecognized integration names, but the design doesn't say whether compile-time validation
   is in scope.

2. A state can have both `Processing != ""` AND outgoing `Transitions`. The design pseudocode
   stops at the processing integration check before evaluating transitions. This implies a state
   with both is legal but the transitions are never auto-evaluated while the state has an active
   processing integration. The design should state this explicitly, because template authors need
   to know whether to model a "processing done → advance" transition as a separate follow-on
   state or as a transition from the processing state.

---

### 6. Is the template format v2 YAML structure fully defined?

**No. This is the most significant specification gap.**

The design describes the Go struct changes (`sourceStateDecl.Transitions` changes from
`[]string` to `[]sourceTransitionDecl{Target string, Gates map[string]sourceGateDecl}`), but
the actual YAML syntax a template author writes is never shown.

Current v1 YAML for a state:
```yaml
states:
  review:
    transitions: [approved, rejected]
    gates:
      review_complete:
        type: field_not_empty
        field: review_result
```

The design implies v2 would look like:
```yaml
states:
  review:
    processing: delegate_review   # new field
    transitions:
      - target: approved
        gates:
          approval_confirmed:
            type: field_equals
            field: review_result
            value: approved
      - target: rejected
        gates:
          rejection_confirmed:
            type: field_equals
            field: review_result
            value: rejected
```

But this is not shown anywhere in the design. Without a YAML example, the implementer of Phase 2
(`compile.go`) must infer the format from the Go struct names, and the implementer of the
migration guide has nothing to reference. Additionally, the YAML example would reveal a currently
unspecified question: can a state have **both** shared gates (on the state) AND per-transition
gates (on individual transitions)? The design says shared gates evaluate first, then
per-transition gates, so yes — but a template author has no example showing both.

This gap should be filled before Phase 2 starts. A single before/after YAML example in the
design document (or a companion template format guide) is sufficient.

---

### 7. Simpler alternatives or obvious shortcuts?

**One simplification was overlooked: `koto next --to` vs. `koto rewind --to` symmetry.**

`koto rewind` already uses `--to` for its target flag. The design adds `--to` on `koto next`
with different semantics (directed forward transition vs. rewind). Both will appear in `--help`
output and documentation. The flag name collision across commands is not a correctness issue, but
it is a usability footgun — users who know `koto rewind --to` and guess `koto next --to` is
similar will expect a backward-compatible mental model. The design doesn't acknowledge this
naming collision.

**The `StopDirected` stop reason may be unnecessary infrastructure.** The design says Advance
always stops after a directed transition. The caller (CLI) already knows it passed `--to` and
can infer the stop reason. `StopDirected` carries no additional information the CLI can act on
that `StopTerminal` or `StopGateBlocked` don't. Collapsing `StopDirected` into the existing
`StopGateBlocked` (or a new `StopRequested`) and having the CLI track "did I pass --to?" is
simpler. This is advisory — the current design is also fine.

**`Engine.Machine()` deep copy vs. passing `MachineState` by value.** The deep copy of
`Machine()` is already present and will grow more complex with `TransitionDecl`. The controller
calls `eng.Machine()` in `Next()` today, primarily to check `ms.Terminal`. If the controller
accessed terminal and processing state through purpose-built accessors on Engine
(`eng.IsTerminal(state string) bool`, `eng.ProcessingIntegration(state string) string`), the
`Machine()` deep copy method could stay simpler. This is advisory and not a blocking concern.

---

## Blocking Findings

**B1 — `AdvanceResult` missing integration output field.**
`IntegrationRunner.Run()` returns `map[string]string` but `AdvanceResult` has no field to carry
it. The CLI cannot format the processing integration stop output without this field. Add
`IntegrationData map[string]string` (or equivalent) to `AdvanceResult` before Phase 3 starts.

**B2 — Template format v2 YAML syntax not shown.**
Phase 2 (`compile.go`) must parse a new YAML structure, but the design never shows what the
YAML looks like from a template author's perspective. The implementer must infer format from Go
struct names, creating divergence risk. Specify at least one concrete YAML example covering a
state with per-transition gates before Phase 2 starts.

---

## Advisory Findings

**A1 — `Engine` interface for controller injection not specified.**
Decision should be made before Phase 3: extract `EngineI` interface or test through real
in-memory engine. Either is fine; the risk is Phase 3 being written one way and Phase 4
discovering the other way is needed.

**A2 — `IntegrationRunner.Run()` error handling contract missing.**
Does a runner error surface as `AdvanceResult` (structured) or as a raw `error` return from
`Advance()`? Specify before Phase 3.

**A3 — Processing state + outgoing transitions interaction not stated.**
A state with both `Processing != ""` and outgoing transitions: the controller stops before
evaluating transitions. Template authors need to know this to model processing states correctly.

**A4 — `--with-data` + `--to` combined behavior not described.**
One sentence needed: "When both flags are provided, evidence is injected and archived but gate
evaluation is skipped."

**A5 — `Engine.Machine()` deep copy missing `TransitionDecl` copy logic in Phase 1 deliverables.**
The existing `copy(transitions, ms.Transitions)` in `engine.go:376-378` copies `[]string`. After
Phase 1, it must copy `[]TransitionDecl` including the inner `Gates map[string]*GateDecl`. Add
to Phase 1 deliverables explicitly.

---

## Conclusion

The overall architecture fits. The controller-owned loop, engine as single-transaction executor,
and `IntegrationRunner` injection are all consistent with the existing codebase patterns. The
two blocking gaps (missing `AdvanceResult.IntegrationData` field; no v2 YAML syntax example)
are filling-in work, not redesign. Address those before implementation starts on Phases 2 and 3,
and the design is implementable without significant divergence between phases.
