---
status: Proposed
upstream: docs/prds/PRD-unified-koto-next.md
problem: |
  koto's state evolution is split across multiple commands (koto next, koto transition,
  and the planned koto delegate run), each valid only at specific workflow points. The
  engine's gate model is state-level only, with no support for per-transition conditions
  needed for evidence-based branching. Evidence is globally accumulated across the
  workflow lifetime, which breaks when transitions skip states or when workflows loop.
  The CLI has no flags for evidence submission or human-directed overrides. There is no
  auto-advancement loop — the controller returns the current directive without chaining
  through states with satisfied conditions.
decision: |
  Restructure koto's command interface, engine, and state model around a single
  koto next command that auto-advances until blocked, accepts evidence via --with-data,
  and supports human-directed transitions via --to. Per-transition conditions replace
  the flat state-level gate map. Evidence becomes per-state scoped, cleared and archived
  on each transition. Auto-advancement chains through states using a visited-set to
  prevent cycles, stopping at unsatisfied conditions, processing integrations, or
  terminal states.
rationale: |
  A single command surface eliminates ordering errors and keeps the agent contract
  stable as new capabilities are added. Per-state evidence scoping is required for
  correctness with directed transitions and looping workflows — global evidence breaks
  when states are skipped or re-entered. Per-transition conditions are the minimum
  change needed to support evidence-based branching without agents naming target states.
  Independent transaction-per-transition atomicity is simpler than multi-state
  transactions and recovers safely from crashes by re-evaluating from the last committed
  state.
---

# DESIGN: Unified koto next Command

## Status

Proposed

## Context and Problem Statement

koto's current architecture separates state reading (`koto next`) from state advancement
(`koto transition`) into two commands that the agent must call in the right order. This
creates correctness risks — nothing prevents an agent from calling `koto transition` when
the engine expects an evidence submission — and the surface will grow further as new
capabilities like delegation are added.

The engine's data model compounds the problem. Gates live on `MachineState` as a flat
map, evaluated with AND logic before any transition. There is no structure for attaching
conditions to individual outgoing transitions, which is required for evidence-based
branching where the agent's submission determines which branch to take. Evidence is stored
in a single global map that accumulates across the entire workflow; this breaks when a
directed transition skips a state that would have collected evidence, and when looping
workflows re-enter branching states carrying stale values from a prior iteration.

The controller has no auto-advancement loop. `Next()` returns the current state's directive
without chaining through states whose conditions are already satisfied. This forces agents
to call `koto next` and `koto transition` in an alternating loop rather than letting koto
advance autonomously through states it can verify itself.

Three systems need to change together: the CLI (new flags, removed subcommand), the engine
(per-transition gate model, per-state evidence scoping, advancement loop with cycle
detection), and the template format (transitions change from a string list to structured
declarations with per-transition conditions).

## Decision Drivers

- **Correctness of evidence-based branching**: per-transition conditions require structural
  changes to `MachineState.Transitions` — the field must become a typed slice, not a string
  slice, to carry per-transition gate declarations
- **Evidence isolation**: global evidence breaks with directed transitions and looping
  workflows; the fix (clear evidence on transition) must be atomic with the transition commit
- **Cycle safety in auto-advancement**: the advancement loop needs visited-state tracking
  scoped to the current call to prevent infinite loops on cyclic templates
- **Crash recovery**: each transition must be independently committed so the workflow
  recovers to a valid state after a mid-chain crash; no multi-transition atomic transactions
- **Template format migration**: `transitions: []string` → `transitions: []TransitionDecl`
  is a breaking change; a format version bump is required
- **CLI surface stability**: all new capabilities go through `koto next` flags, not new
  subcommands; `koto transition` is removed
- **Testability**: auto-advancement and gate evaluation must remain unit-testable without
  subprocess side effects; processing integrations are injected via interface

## Considered Options

### Decision: Where does the auto-advancement loop live?

**Context:** koto next must chain through multiple state transitions in a single call,
stopping only when blocked. This loop requires visiting-state tracking, stopping-condition
evaluation, integration invocation, and evidence lifecycle management. The question is
which package owns this orchestration logic.

**Chosen: Controller-owned advancement loop (Approach A).**

The controller already holds the workflow's config context — delegation rules, template
path, integration configuration. Advancement is an orchestration concern, not a
transaction concern; the engine's job is to execute one correct transition atomically,
not to decide when to stop. Keeping orchestration in the controller preserves a minimal
engine package that is easy to reason about and test in isolation. The `IntegrationRunner`
interface injected into the controller follows the same pattern already established for
delegate checking in the delegation design. No new packages are introduced, and the loop
is unit-testable by injecting mock engine and integration runner implementations.

*Alternative rejected: Engine-owned advancement (Approach B).* The engine gains an
`Advance()` loop alongside `transitionOne()`, making it the single source of truth for
advancement. The appeal is that "how advancement works" is answerable by reading one
package. The cost is significant: the engine grows from ~700 to ~1100 lines and gains
awareness of integrations, config, and agent-facing concepts — concerns it currently has
no knowledge of. Mixing transaction logic (atomically commit one state change) with
orchestration logic (decide when to stop chaining) in the same package makes each harder
to navigate and the package harder to maintain. The engine package size concern is a
long-term maintainability risk; the controller approach avoids it without sacrificing
testability.

*Alternative rejected: New workflow orchestrator layer (Approach C).* A new `pkg/workflow`
package provides maximum architectural clarity — three explicit layers (engine, workflow,
controller) enforced at the package import level. The engine stays a pure single-transition
executor, and future capabilities extend `Workflow`, not the controller. The trade-off is
real: ~300-400 lines of new infrastructure for logic that fits naturally in the controller.
If the controller becomes a thin wrapper around `Workflow`, it loses its rationale as a
distinct type, and callers might as well use `Workflow` directly. The architectural clarity
benefit doesn't justify the added indirection at this stage — the controller already serves
as the orchestration layer between CLI and engine, and the mandatory engine and template
changes are identical regardless of approach.

## Decision Outcome

The controller owns the auto-advancement loop. `Engine.Transition()` is extended to support
`TransitionDecl` (per-transition gate declarations) and evidence clearing on commit, but
executes exactly one transition per call. The controller gains `Advance(opts AdvanceOpts)`
which loops — calling `Engine.Transition()`, evaluating stopping conditions, and invoking
an injected `IntegrationRunner` when a processing integration stops the chain. The CLI
adds `--with-data` and `--to` flags routed through the controller.

Key properties:
- Engine remains a single-transition executor with an extended type model
- Controller owns the advancement loop and integration invocation
- `IntegrationRunner` interface injected into the controller; no subprocess side effects
  in test paths
- Each transition is independently committed; crash mid-chain recovers from last committed
  state
- Visited-state set (per `Advance()` call) prevents infinite loops on cyclic templates
- Evidence is cleared atomically with each transition commit and archived to history
- No new packages introduced; changes are contained to existing packages

## Solution Architecture

### Overview

`koto next` becomes a single command that auto-advances through the state machine until
blocked, accepts evidence submissions via `--with-data`, and supports human-directed
transitions via `--to`. The CLI delegates to `Controller.Advance()`, which loops over
`Engine.Transition()` calls until a stopping condition is reached. Each transition is
independently committed; evidence is cleared atomically on each commit. A visited-state
set scoped to the current call prevents infinite loops on cyclic templates.

### Components

**`pkg/engine` — single-transition executor**

- `types.go`: `TransitionDecl{Target string, Gates map[string]*GateDecl}` replaces
  `MachineState.Transitions []string`. `MachineState` gains a `Processing string` field
  identifying the processing integration (empty means none). Shared gates remain on
  `MachineState.Gates`.
- `engine.go`: `Transition(target string, opts ...TransitionOption)` is updated to:
  1. Resolve `target` to a matching `TransitionDecl` in `ms.Transitions`
  2. Evaluate shared gates (`MachineState.Gates`) with AND logic — fail fast
  3. Evaluate per-transition gates (`TransitionDecl.Gates`) with AND logic — fail fast
  4. If `directed` option is set, skip gate evaluation entirely
  5. Build `HistoryEntry` — archive current `State.Evidence`
  6. Reset `State.Evidence = make(map[string]string)`
  7. Commit via `persist()` — single atomic write

**`pkg/controller` — advancement loop and integration orchestration**

- `types.go` (new or extended): `AdvanceOpts{WithData map[string]string, To string}`.
  `AdvanceResult{Directive Directive, StoppedBecause StopReason, Advanced bool}`.
  `IntegrationRunner` interface: `Run(integrationName string, state State) (map[string]string, error)`.
  `StopReason` enum: `StopGateBlocked`, `StopProcessingIntegration`, `StopTerminal`, `StopCycleDetected`, `StopDirected`.
- `controller.go`: `Controller` gains `runner IntegrationRunner` field injected at `New()`.
  `Advance(opts AdvanceOpts) (*AdvanceResult, error)` implements:
  ```
  visited := map[string]bool{}
  loop:
    current := eng.CurrentState()
    ms := machine.States[current]
    if visited[current]: return StopCycleDetected
    visited[current] = true
    if ms.Terminal: return StopTerminal
    if ms.Processing != "" && !opts.To:
      result, err := runner.Run(ms.Processing, state)
      return StopProcessingIntegration with result
    determine target (from opts.To, or resolve from satisfied TransitionDecl)
    if no satisfied target: return StopGateBlocked
    eng.Transition(target, evidence options...)
    if opts.To: return StopDirected (always stop after directed)
    continue loop
  ```

**`pkg/template` — format v2 compilation**

- `compile/compile.go`: `sourceStateDecl.Transitions` changes from `[]string` to
  `[]sourceTransitionDecl{Target string, Gates map[string]sourceGateDecl}`. Shared gates
  remain as `map[string]sourceGateDecl` on the state.
- `compiled.go`: `StateDecl.Transitions []string` → `[]engine.TransitionDecl`.
  `FormatVersion` bumps from 1 to 2. Parser rejects format version 1 with a migration
  error message.
- `StateDecl` gains `Processing string` field, compiled from `processing: <name>` in
  template YAML.

**Template format v2 YAML (before → after):**

```yaml
# Format v1 (current)
states:
  gather_info:
    transitions:
      - analyze
      - skip_to_output
    gates:
      has_data:
        type: field_not_empty
        field: input_file

# Format v2 (new)
states:
  gather_info:
    gates:                          # shared gates — evaluated before any transition
      workflow_ready:
        type: field_not_empty
        field: workflow_id
    transitions:
      - target: analyze
        gates:                      # per-transition gates — evaluated after shared gates
          has_data:
            type: field_not_empty
            field: input_file
      - target: skip_to_output      # no per-transition gates: always satisfied if shared pass
    processing: delegate_cli        # optional; if set, controller stops and invokes runner
```

A state with no `gates` and single `target` transition (no per-transition gates) is
always auto-advanced through when reached. The `processing` field and `transitions` can
coexist; `processing` takes priority (see Data Flow).

**`cmd/koto/main.go` — CLI entry point**

- `cmdTransition` removed. `cmdNext` gains `--with-data <file>` (reads JSON, passes as
  evidence) and `--to <transition>` (directed target). Both flags populate `AdvanceOpts`.
- `koto next` → `controller.Advance(opts)` → format `AdvanceResult` as JSON output.

### Key Interfaces

```go
// IntegrationRunner is injected into the controller at construction time.
// Implementations live in internal/ or cmd/.
type IntegrationRunner interface {
    Run(integrationName string, state engine.State) (map[string]string, error)
}

// AdvanceOpts controls a single Advance() call.
type AdvanceOpts struct {
    WithData map[string]string // evidence to inject into each transition
    To       string            // directed transition target; bypasses gates
}

// AdvanceResult is returned by Advance().
type AdvanceResult struct {
    Directive       Directive         // current state's directive (after stopping)
    StoppedBecause  StopReason
    Advanced        bool              // false if no transitions were taken
    IntegrationData map[string]string // non-nil when StoppedBecause == StopProcessingIntegration
}
// IntegrationRunner errors (non-nil error return) propagate directly as Advance() errors —
// they indicate a system failure, not a workflow stopping condition.

// TransitionDecl declares one outgoing transition (replacing []string).
// Lives in pkg/engine/types.go.
type TransitionDecl struct {
    Target string
    Gates  map[string]*GateDecl // per-transition gates; evaluated after shared gates
}

// MachineState gains Processing field (pkg/engine/types.go).
type MachineState struct {
    Transitions []TransitionDecl
    Terminal    bool
    Gates       map[string]*GateDecl // shared gates; evaluated before per-transition gates
    Processing  string               // non-empty: name of processing integration to invoke
}
```

### Data Flow

**Normal `koto next` call (no flags):**
```
CLI
  → Advance(AdvanceOpts{})
  → loop:
      CurrentState → check visited, terminal, processing
      evaluate shared gates → evaluate per-transition gates → find satisfied target
      if no target: return StopGateBlocked (return directive)
      Transition(target) → archive evidence → clear evidence → persist
      continue
  → return AdvanceResult to CLI
  → CLI formats as JSON
```

**Evidence submission (`koto next --with-data data.json`):**
```
CLI reads data.json → AdvanceOpts{WithData: {...}}
  → Advance injects evidence into first Transition() call via WithEvidence option
  → gate evaluation uses merged evidence → transition committed → evidence cleared
  → auto-advancement continues from new state with empty evidence
```

**Directed transition (`koto next --to <target>`):**
```
CLI → AdvanceOpts{To: "target_state"}
  → Advance skips gate evaluation for the target transition
  → Transition(target, WithDirected(true)) → HistoryEntry.Directed = true
  → return StopDirected immediately (always stop after directed)
```

**Processing integration stop:**
```
Advance detects ms.Processing != "" in current state
  → runner.Run(ms.Processing, state) → returns result data
  → return StopProcessingIntegration with integration output in AdvanceResult.IntegrationData
  → CLI formats output with expects: {submit_with: "--with-data"}
```

**`--with-data` + `--to` combined:**
```
CLI → AdvanceOpts{WithData: {...}, To: "target_state"}
  → evidence is merged into the directed Transition() call
  → gate evaluation is skipped (directed), but evidence is archived to history
  → return StopDirected immediately
```

**State with `Processing != ""` and outgoing transitions:**
```
Advance checks ms.Processing BEFORE evaluating any transitions
  → if Processing is set, runner is invoked and Advance stops (StopProcessingIntegration)
  → outgoing transitions are never evaluated
  → template authors: if a state has both Processing and transitions, the processing
    integration always takes precedence; the agent submits evidence via --with-data
    on the next call to advance through the outgoing transitions
```

**Nil IntegrationRunner:**
```
Controller.New() accepts a nil runner.
If Advance() reaches a state with Processing != "" and runner is nil:
  → return error("no IntegrationRunner configured for processing state <name>")
Non-integration code paths (no Processing states) work normally with nil runner.
```

## Implementation Approach

### Phase 1: Engine type model and evidence clearing

Update the engine's data model. This is the foundation all other changes depend on.

Deliverables:
- `pkg/engine/types.go`: Add `TransitionDecl` struct; change `MachineState.Transitions`
  from `[]string` to `[]TransitionDecl`; add `MachineState.Processing string`
- `pkg/engine/engine.go`: Update `Transition()` to resolve target via `TransitionDecl`
  slice; two-phase gate evaluation (shared then per-transition); evidence archive + clear
  before `persist()`; add `WithDirected` transition option
- `pkg/engine/engine.go`: Update `deepCopyMachine()` (or equivalent) to deep-copy
  `[]TransitionDecl` including inner `Gates map[string]*GateDecl` — naive slice copy
  after the type change would alias gate pointers across copies
- `pkg/engine/engine_test.go`: Update all tests that construct `MachineState` with
  `Transitions []string`; add tests for per-transition gates, evidence clearing, directed

### Phase 2: Template format v2

Update the template compilation pipeline to produce the new engine types.

Deliverables:
- `pkg/template/compile/compile.go`: Parse `transitions` as `[]sourceTransitionDecl`
  (YAML object with `target` and `gates`); parse `processing` field on states
- `pkg/template/compiled.go`: Update `StateDecl.Transitions` to `[]engine.TransitionDecl`;
  add `StateDecl.Processing string`; bump `FormatVersion` to 2; add v1 rejection with
  migration message
- `pkg/template/compile/compile_test.go`: Update fixtures for new format

### Phase 3: Controller advancement loop

Add `Advance()` to the controller with the full stopping-condition loop.

Deliverables:
- `pkg/controller/types.go` (new): `AdvanceOpts`, `AdvanceResult`, `StopReason`,
  `IntegrationRunner` interface
- `pkg/controller/controller.go`: Add `runner IntegrationRunner` to `Controller`;
  update `New()` to accept runner; implement `Advance(opts AdvanceOpts)`
- `pkg/controller/controller_test.go`: Tests for each stopping condition; visited-state
  cycle; directed transition; evidence injection

### Phase 4: CLI and command cleanup

Wire everything together at the CLI layer. Remove `koto transition`.

Deliverables:
- `cmd/koto/main.go`: Remove `cmdTransition`; update `cmdNext` with `--with-data` and
  `--to` flags; route to `controller.Advance()`; format `AdvanceResult` as JSON output
- Integration tests covering the full call path

## Security Considerations

### Download Verification

Not applicable. This design adds no download capability. All changes operate on local
state files and invoke local binaries. No external URLs are fetched, no artifacts
downloaded, no checksums needed.

### Execution Isolation

**Command gates** — Gate commands are declared as static strings in template YAML and
invoked with a timeout. Evidence values must not be interpolated into the command string
itself. The current `evaluateCommandGate` does not set `cmd.Env`, which means gate commands
inherit the full process environment. This is a required fix: implementers must explicitly
set `cmd.Env` to a minimal allowlist (PATH and HOME only) when invoking gate commands.
Templates containing command gates are trusted code; operators should apply the same review
processes to templates as to application code.

**IntegrationRunner** — The `Processing` field in a template specifies an integration name,
not a raw binary path. The `IntegrationRunner` implementation must resolve names through a
configured allowlist, not treat the field as an executable path. All integration invocations
should be logged.

**`--with-data` file reading** — The evidence file must be validated before injection: size
limit (1 MB), valid JSON structure, bounded key and value lengths. The file path must be
canonicalized to prevent symlink traversal: call `filepath.Abs` then `filepath.EvalSymlinks`,
and verify the resolved path is within an expected prefix before reading.

**State file permissions** — State files are written via atomic rename from a temp file.
`os.CreateTemp` inherits the process umask, which typically produces 0644 (world-readable).
Implementers must use `os.OpenFile` with explicit mode 0600 instead of `os.CreateTemp` to
prevent the temp file from being readable during the write window.

### Supply Chain Risks

koto templates are the primary supply chain artifact. Templates declare gate commands
(shell subprocesses) and integration runner names. Until template signing is implemented,
executing any koto template is equivalent to executing arbitrary commands with the invoking
user's environment — this is the current threat model. Templates should be distributed with
the same review processes as source code. The compiled template cache (SHA-256 keyed)
ensures integrity within a session but does not verify template authorship. Future work:
ECDSA template signing and verification before execution.

### User Data Exposure

Evidence submitted via `--with-data` is written to the state file and archived to history
entries on every transition. Evidence values may include sensitive data (tokens, intermediate
agent outputs). State files must be created with 0600 permissions (see Execution Isolation
for the required implementation approach).

The current controller interpolates evidence values into directive text by default. This
means any evidence key containing a sensitive value will appear in the formatted directive
returned to the caller. Template authors should avoid interpolating evidence keys that may
carry secrets; the implementation should document which evidence keys are interpolated so
callers can make informed decisions about what to submit via `--with-data`.

## Consequences

### Positive

- **Single command surface**: agents call `koto next` for all state progression, eliminating
  ordering errors and preventing invalid command sequences
- **Evidence correctness**: per-state scoping prevents contamination across branches and loop
  iterations; clearing is atomic with the transition commit
- **Crash recovery**: independent per-transition commits mean any crash leaves the workflow
  in a valid, recoverable state; no multi-state rollbacks needed
- **Testable at every layer**: engine tests cover single-transition atomics; controller tests
  cover the loop and stopping conditions via interface injection; no subprocess side effects
  in any unit test path
- **Minimal blast radius**: changes are contained to existing packages; no new packages
  introduced; one external CLI caller of `Transition()` needs updating

### Negative

- **Breaking template format change**: `transitions: [string]` → `transitions: [{target, gates}]`
  requires all existing template files to be migrated; format version 1 templates become invalid
- **`koto transition` removed**: callers using `koto transition` directly (scripts, documentation,
  existing workflows) must update to `koto next --to <target>`
- **Controller grows significantly**: `Next()` (~40 lines) is joined by `Advance()` (~150-200
  lines) plus helper methods; the controller package takes on more responsibility than before
- **Processing integration field is new infrastructure**: `MachineState.Processing` and the
  `IntegrationRunner` interface have no prior art in the codebase; their semantics must be
  documented carefully to prevent misuse

### Mitigations

- **Template migration**: format version 2 rejection includes a clear migration error message;
  a migration guide in the documentation covers the before/after format change
- **`koto transition` removal**: `koto next --to` is a direct replacement with identical
  semantics for the human-directed case; the CLI help text will document the change
- **Controller complexity**: `Advance()` is decomposed into named helper methods
  (`findSatisfiedTransition`, `isCycle`, `checkStoppingConditions`) to keep the loop body
  readable; the stopping conditions map directly to PRD requirements
- **IntegrationRunner documentation**: the interface contract is documented with a concrete
  example (delegate CLI invocation) and the `Processing` field semantics are specified in
  the template format guide

