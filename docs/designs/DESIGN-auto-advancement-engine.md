---
status: Proposed
problem: |
  koto next evaluates one state and returns. States with no accepts block, passing
  gates, and unconditional transitions require the agent to manually chain through
  each intermediate state via koto next --to, turning automatic advancement into
  tedious back-and-forth. The missing pieces are the advancement loop, integration
  runner interface, signal handling for clean shutdown, and koto cancel for workflow
  abandonment.
decision: |
  An engine-layer advancement function in src/engine/advance.rs takes I/O closures
  for gate evaluation, event appending, and integration invocation. It loops through
  states using a visited-set for cycle detection, resolves transitions by matching
  evidence against when conditions, and returns a StopReason enum. Advisory flock
  prevents concurrent access. Signal handling checks an AtomicBool between iterations.
  Integration runner is a closure interface with config system deferred.
rationale: |
  The engine-layer approach balances testability and simplicity. Injected closures
  make the loop unit-testable without filesystem or process spawning, while avoiding
  the protocol complexity of an action-yielding state machine. Placing the loop in
  src/engine/ matches the codebase's existing architecture where the engine module
  owns workflow logic and the CLI handler does I/O setup. The handler-layer
  alternative was rejected because it would grow an already 340-line function without
  structural improvement.
---

# DESIGN: Auto-Advancement Engine

## Status

Proposed

## Upstream Design Reference

Parent: `docs/designs/DESIGN-unified-koto-next.md` (Phase 4: Auto-advancement engine)

Relevant sections: Solution Architecture > Data Flow (advancement loop pseudocode),
Sub-Design Boundaries (scope definition), Security Considerations (signal handling,
integration invocation).

## Context and Problem Statement

`koto next` currently evaluates one state and returns. If the current state has no
`accepts` block, passing gates, and an unconditional transition, the agent gets back
a response that says "you can advance" but doesn't actually advance. The agent must
call `koto next --to <target>` manually for every intermediate state, turning what
should be automatic chaining into a tedious back-and-forth.

The strategic design specifies an advancement loop that chains through states until
hitting a stopping condition (terminal, gate blocked, evidence required, integration,
or cycle). This design covers that loop, plus the integration runner that the
strategic design deferred, signal handling for clean shutdown, and `koto cancel` for
workflow abandonment.

The existing codebase has solid foundations: event types are defined, the gate
evaluator has process group isolation, evidence validation works, and the pure
`dispatch_next` function classifies states correctly. What's missing is the loop
that ties them together.

## Decision Drivers

- **Correctness over speed**: the loop must never corrupt the event log, even on
  SIGTERM mid-chain
- **Pure function preservation**: `dispatch_next` stays pure (no I/O); the loop
  lives in the handler layer
- **Reuse existing infrastructure**: gate evaluator, evidence validation, and
  persistence layer are battle-tested; don't rewrite them
- **Integration runner must degrade gracefully**: a missing or misconfigured
  integration returns `IntegrationUnavailable`, not a crash
- **Cycle detection must be simple**: visited-state set per invocation; no need
  for graph analysis

## Considered Options

### Decision: Where does the advancement loop live?

**Context:** The advancement loop chains through auto-advanceable states, calling
existing I/O functions (gate evaluation, event appending, integration invocation)
between iterations. The question is where to place the loop relative to the existing
`dispatch_next` (pure classifier) and `handle_next` (I/O handler) split.

**Chosen: Engine-Layer Advancement.**

A new `src/engine/advance.rs` module exposes `advance_until_stop()`, which takes
the current state, compiled template, and closures for I/O operations. The engine
owns cycle detection (visited-state `HashSet`), transition resolution (matching
evidence against `when` conditions), and stopping condition evaluation. The handler
sets up the I/O closures and calls the engine, then translates the `StopReason`
result into a `NextResponse` for serialization.

This approach fits the codebase's existing architecture: `src/engine/` already owns
persistence, evidence validation, and type definitions. The advancement loop is
workflow logic, not CLI logic, and belongs alongside those modules. It preserves
`dispatch_next` as a pure function (called within the engine's loop body), makes
the loop testable through injected I/O callbacks without touching the filesystem,
and maps 1:1 to the upstream design's pseudocode.

*Alternative rejected: Handler-Layer Loop.* Adding the loop directly into
`handle_next` is the simplest change (~300-400 lines vs ~600-800) and requires no
new abstractions. It was rejected because `handle_next` is already 340 lines of
inline logic and adding the loop would push it further without structural
improvement. The loop logic isn't testable in isolation -- it requires integration
tests that set up state files and spawn processes. For a correctness-critical loop
that must handle cycle detection, signal interruption, and five stopping conditions,
unit-testable engine logic is worth the extra abstraction.

*Alternative rejected: Action-Yielding State Machine.* An iterator yielding typed
directives (`EvaluateGates`, `AppendTransitioned`, etc.) that the handler executes
in a ping-pong protocol. This maximizes testability -- every step is observable and
assertable. It was rejected because Rust lacks native generators, requiring manual
coroutine state encoding that adds boilerplate and a new class of protocol bugs
(calling the wrong `feed_*` method). The loop has five stopping conditions and
three I/O operations; this level of machinery is more than the problem warrants.
The engine-layer approach captures most of the testability benefit through injected
closures without the protocol complexity.

### Decision 2: Concurrent access protection

**Context:** The advancement loop holds the state file logically open for the
duration of the chain, which could span multiple gate evaluations and event appends.
Without protection, a concurrent `koto next` call could interleave writes and
produce duplicate sequence numbers. The existing `read_events` catches sequence
gaps on the next read, but the corruption has already happened.

**Chosen: Advisory flock around the loop.**

Acquire an exclusive advisory lock (`flock`) on the state file at the start of the
advancement loop, release on exit. A second `koto next` call gets an immediate error
(non-blocking lock attempt) rather than waiting or silently corrupting the log.

*Alternative rejected: Defer to a future issue.* The sequence gap detection provides
post-hoc detection, but the advancement loop's longer execution window makes
concurrent access more likely than with single-shot dispatch. Fixing it later means
debugging corrupted state files in the interim.

*Alternative rejected: PID file approach.* More complex to implement correctly
(stale PID cleanup, race conditions) and doesn't provide the same file-level
guarantee as flock.

### Decision 3: Flock behavior on contention

**Context:** When a second `koto next` call tries to acquire the flock while the
advancement loop is running, it could either wait (blocking) or fail immediately
(non-blocking).

**Chosen: Non-blocking with immediate error.**

A second caller gets an error exit (code 1, transient) telling it the workflow is
currently being advanced. The agent can retry later. This prevents accidental
deadlocks if a crashed process leaves a stale lock (advisory locks are released on
process exit, but blocking waits are harder to reason about in signal-heavy code).

*Alternative rejected: Blocking wait.* Simpler for the caller (just waits), but
creates hidden latency. The agent doesn't know why the call is slow, and if the
first process is killed without releasing the lock (shouldn't happen with advisory
locks, but defense in depth), the second process hangs.

### Decision 4: Integration runner scope

**Context:** The upstream design specifies an integration configuration system
(project config or plugin manifest) for resolving integration names to executable
runners. This is the biggest open question for the integration runner.

**Chosen: Defer config, design the closure interface only.**

The engine takes an integration closure (`Fn(&str) -> Result<Value, IntegrationError>`).
The CLI handler passes a stub that always returns `IntegrationUnavailable`. The
config system becomes a separate issue. The engine is ready for a real runner when
it arrives -- the closure signature is the contract.

*Alternative rejected: Include minimal config system.* Designing a config format
(e.g., `.koto/integrations.toml`) and subprocess runner adds scope that isn't needed
for the advancement loop itself. The closure interface means the config system can
be built independently without changing the engine.

## Decision Outcome

The auto-advancement engine lives in `src/engine/advance.rs` as a function that
takes I/O callbacks, iterates through states using the existing `dispatch_next`
classifier, and returns a structured `StopReason` when it hits a stopping condition.

Key properties:
- `dispatch_next` stays pure; the engine calls it per-iteration for classification
- I/O operations (gate evaluation, event appending, integration invocation) are
  injected as closures, making the loop unit-testable with mocks
- Cycle detection uses a `HashSet<String>` scoped to the invocation
- Signal handling checks an `AtomicBool` between iterations; the last fsync'd event
  is always durable before the check
- `StopReason` maps to `NextResponse` in the handler, keeping CLI serialization
  concerns out of the engine
- Advisory flock on the state file prevents concurrent advancement loops
- Integration runner is a closure interface; config system deferred to a separate issue
- `koto cancel` is a new subcommand that appends a `workflow_cancelled` event

## Solution Architecture

### Overview

The advancement engine is a loop function in `src/engine/advance.rs` that chains
through workflow states until hitting a stopping condition. It sits between the
existing pure classifier (`dispatch_next`) and the CLI handler (`handle_next`),
owning the iteration logic while delegating all I/O to injected closures.

The handler acquires an advisory flock on the state file, sets up I/O closures
(closing over the working directory for gate evaluation), registers a signal
handler, and calls `advance_until_stop()`. The engine does its own classification
per iteration (terminal check, integration check, gate evaluation, transition
resolution) rather than calling `dispatch_next` -- because `dispatch_next`
classifies stopping conditions but doesn't resolve transitions. The engine returns
a `StopReason` that the handler maps to a `NextResponse` for JSON serialization.

### Components

```
handle_next (src/cli/mod.rs)
  │
  ├─ Acquires flock on state file
  ├─ Registers SIGTERM/SIGINT → AtomicBool via signal-hook
  ├─ Sets up I/O closures:
  │   ├─ append_event closure (wraps existing append_event + in-memory event list)
  │   ├─ evaluate_gates closure (wraps existing evaluate_gates)
  │   └─ invoke_integration closure (stub returning IntegrationUnavailable)
  │
  ├─ Calls advance_until_stop()
  │       │
  │       ├─ advance_until_stop (src/engine/advance.rs)
  │       │   ├─ Maintains visited: HashSet<String>
  │       │   ├─ Maintains current_state: String
  │       │   ├─ Per iteration:
  │       │   │   ├─ Check shutdown flag
  │       │   │   ├─ Check visited set (cycle detection)
  │       │   │   ├─ Check terminal
  │       │   │   ├─ Check integration (invoke runner, stop)
  │       │   │   ├─ Evaluate gates (via closure)
  │       │   │   ├─ Resolve transition (via resolve_transition)
  │       │   │   ├─ Append transitioned event (via closure)
  │       │   │   └─ Continue loop
  │       │   └─ Returns AdvanceResult { final_state, stop_reason }
  │       │
  │       └─ resolve_transition (src/engine/advance.rs)
  │           ├─ Pure function
  │           ├─ Merges evidence (last-write-wins per field)
  │           ├─ Matches against when conditions (exact JSON equality)
  │           └─ Returns TransitionResolution enum
  │
  ├─ Maps StopReason → NextResponse
  └─ Serializes and exits

handle_cancel (src/cli/mod.rs)
  ├─ Loads state file
  ├─ Appends WorkflowCancelled event
  └─ Prints confirmation
```

### Key Interfaces

**advance_until_stop signature:**

```rust
pub fn advance_until_stop<F, G, I>(
    current_state: &str,
    template: &CompiledTemplate,
    evidence: &BTreeMap<String, serde_json::Value>,
    append_event: &mut F,
    evaluate_gates: &G,
    invoke_integration: &I,
    shutdown: &AtomicBool,
) -> Result<AdvanceResult, AdvanceError>
where
    F: FnMut(&EventPayload) -> Result<(), PersistenceError>,
    G: Fn(&BTreeMap<String, Gate>) -> BTreeMap<String, GateResult>,
    I: Fn(&str) -> Result<serde_json::Value, IntegrationError>,
```

**AdvanceResult and StopReason:**

```rust
pub struct AdvanceResult {
    pub final_state: String,
    pub advanced: bool,  // true if any transitions were made
    pub stop_reason: StopReason,
}

pub enum StopReason {
    Terminal,
    GateBlocked(BTreeMap<String, GateResult>),
    EvidenceRequired,
    Integration { name: String, output: serde_json::Value },
    IntegrationUnavailable { name: String },
    CycleDetected { state: String },
    ChainLimitReached,
    SignalReceived,
}
```

**TransitionResolution:**

```rust
pub enum TransitionResolution {
    Resolved(String),           // target state name
    NeedsEvidence,              // conditional transitions exist but none match
    Ambiguous(Vec<String>),     // multiple matches (runtime error)
    NoTransitions,              // dead-end state (runtime error)
}
```

**resolve_transition signature:**

```rust
pub fn resolve_transition(
    template_state: &TemplateState,
    evidence: &BTreeMap<String, serde_json::Value>,
) -> TransitionResolution
```

Resolution logic:
1. Collect conditional transitions (those with `when: Some(...)`)
2. For each, check if all `when` fields match the evidence (exact JSON equality)
3. If exactly one matches, return `Resolved(target)`
4. If multiple match, return `Ambiguous` (runtime error for multi-field overlap)
5. If none match and an unconditional transition exists, return `Resolved(fallback)`
6. If none match and no unconditional fallback, return `NeedsEvidence`
7. If no transitions at all, return `NoTransitions`

**Evidence merging:**

Before calling `resolve_transition`, evidence from the current epoch's
`evidence_submitted` events is merged into a single `BTreeMap`. Later submissions
for the same field override earlier ones (last-write-wins within the epoch).

**Re-invocation prevention for integrations:**

Before invoking the integration runner, the engine checks whether an
`integration_invoked` event already exists in the current epoch for the current
state. If so, it skips re-invocation and returns `Integration` with the stored
output from the existing event.

### Data Flow

```
koto next [--with-data JSON] [--to target]
  │
  ├─ Load state file, derive machine state
  ├─ Load compiled template, verify hash
  ├─ Acquire flock (non-blocking; fail if locked)
  ├─ Register signal handler → AtomicBool
  │
  ├─ If --to: directed transition (existing behavior, no loop)
  │
  ├─ If --with-data: validate + append evidence_submitted
  │
  ├─ Merge evidence from current epoch events
  │
  ├─ advance_until_stop(current_state, template, evidence, closures, flag):
  │   ├─ visited := {current_state}
  │   └─ loop:
  │       ├─ if shutdown flag set: return SignalReceived
  │       ├─ if terminal: return Terminal
  │       ├─ if integration_invoked exists in epoch: return Integration(stored)
  │       ├─ if integration configured: invoke runner
  │       │   ├─ success: append integration_invoked, return Integration
  │       │   └─ error: return IntegrationUnavailable
  │       ├─ evaluate gates: if any fail → return GateBlocked
  │       ├─ resolve_transition(template_state, evidence):
  │       │   ├─ Resolved(target): append transitioned, update current, continue
  │       │   ├─ NeedsEvidence: return EvidenceRequired
  │       │   ├─ Ambiguous: return error
  │       │   └─ NoTransitions: return error
  │       ├─ if visited[target]: return CycleDetected
  │       ├─ visited.insert(target)
  │       └─ current_state = target
  │
  ├─ Map StopReason → NextResponse
  ├─ Release flock
  └─ Print JSON, exit
```

**Maximum chain length:** The engine enforces a maximum of 100 transitions per
invocation as a safety bound against template bugs (e.g., a template with
hundreds of linearly chaining states). If the limit is reached, the engine
returns `StopReason::ChainLimitReached`. This is defense-in-depth; well-formed
templates shouldn't hit it.

**Cancellation detection:** `handle_next` checks for a `WorkflowCancelled` event
in the event log before entering the advancement loop. If found, it returns an
error (`NextErrorCode::TerminalState`) with the message "workflow has been
cancelled." The engine itself does not check for cancellation -- the pre-loop
check in the handler prevents the engine from running on a cancelled workflow.
`koto cancel` also checks before appending, to prevent double-cancellation.

**Signal check granularity:** The shutdown flag is checked between loop
iterations (between states), not between individual gate evaluations within a
state. A state with multiple gates each timing out at 30 seconds could delay
shutdown by `gate_count * 30s`. This is acceptable because gates within a state
are part of one atomic classification step. If faster shutdown is needed, a
future enhancement can add a shutdown flag parameter to the gate evaluator.

**`koto cancel` data flow:**

```
koto cancel <workflow-name>
  │
  ├─ Load state file
  ├─ Check for existing WorkflowCancelled event (reject double-cancel)
  ├─ Verify workflow is not already terminal
  ├─ Append WorkflowCancelled event
  └─ Print confirmation JSON
```

### Functional Test Scenario

A workflow template with four states: `plan`, `implement`, `verify`, `done`.
`plan` and `implement` are auto-advance states (no `accepts` block, unconditional
transitions, no gates). `verify` has an `accepts` block requiring a decision field.
`done` is terminal.

```yaml
states:
  plan:
    directive: "Create implementation plan."
    transitions:
      - target: implement
  implement:
    directive: "Write the code."
    transitions:
      - target: verify
  verify:
    directive: "Review the implementation."
    accepts:
      decision:
        type: enum
        values: [approve, reject]
        required: true
    transitions:
      - target: done
        when:
          decision: approve
      - target: implement
        when:
          decision: reject
  done:
    directive: "Work complete."
    terminal: true
```

**Before this design (current behavior):**

```
$ koto next my-workflow
{"action":"execute","state":"plan","directive":"Create implementation plan.",
 "expects":{"event_type":"evidence_submitted","fields":{},"options":[]}, ...}

# Agent sees empty expects, knows it should auto-advance, but koto didn't.
# Agent must manually call:
$ koto next my-workflow --to implement
$ koto next my-workflow
# Again empty expects, agent must call:
$ koto next my-workflow --to verify
$ koto next my-workflow
# Finally sees the accepts block with decision field.
```

Total: 5 CLI calls to reach the first state that needs agent input.

**After this design (expected behavior):**

```
$ koto next my-workflow
{"action":"execute","state":"verify","directive":"Review the implementation.",
 "advanced":true,
 "expects":{"event_type":"evidence_submitted",
   "fields":{"decision":{"type":"enum","values":["approve","reject"],"required":true}},
   "options":[
     {"target":"done","when":{"decision":"approve"}},
     {"target":"implement","when":{"decision":"reject"}}
   ]},
 "error":null}
```

Total: 1 CLI call. The engine auto-advanced through `plan` and `implement`,
appending `transitioned` events for each, and stopped at `verify` where agent
input is required. The event log contains:

```jsonl
{"seq":2,"type":"transitioned","payload":{"from":"plan","to":"implement","condition_type":"auto"}}
{"seq":3,"type":"transitioned","payload":{"from":"implement","to":"verify","condition_type":"auto"}}
```

**Evidence submission then triggers another auto-advance chain:**

```
$ koto next my-workflow --with-data '{"decision":"reject"}'
{"action":"execute","state":"verify","directive":"Review the implementation.",
 "advanced":true, "expects":{...}, "error":null}
```

The engine matched `decision: reject` to the `implement` transition, advanced to
`implement`, then auto-advanced back to `verify` (which needs evidence again).
The event log gained three new entries: `evidence_submitted`, `transitioned`
(implement), `transitioned` (verify).

This scenario is the definition of done: a single `koto next` call chains through
auto-advanceable states and stops at the first state requiring agent input,
evidence submission, or an external gate.

## Implementation Approach

### Phase 1: Transition resolution and advance engine

Build the core engine without signal handling or file locking.

Deliverables:
- `src/engine/advance.rs`: `resolve_transition()`, `advance_until_stop()`,
  `AdvanceResult`, `StopReason`, `TransitionResolution` types
- Unit tests for transition resolution (unconditional, conditional, fallback,
  ambiguous, no-transitions cases)
- Unit tests for the advancement loop using mock closures (auto-advance chain,
  gate-blocked stop, evidence-required stop, cycle detection, integration stop)

### Phase 2: Handler integration

Wire `advance_until_stop()` into `handle_next`, replacing the single-shot dispatch.

Deliverables:
- Refactor `handle_next` to call `advance_until_stop()` with I/O closures
- `StopReason` → `NextResponse` mapping in the handler
- Evidence merging from epoch events before calling the engine
- Integration tests verifying multi-state auto-advancement via CLI

### Phase 3: Signal handling and file locking

Add signal-based shutdown and concurrent access protection.

Deliverables:
- `signal-hook` dependency in `Cargo.toml`
- `AtomicBool` registration for SIGTERM/SIGINT in `handle_next`
- Advisory flock acquisition/release around the advancement loop
- Tests for signal-interrupted advancement and concurrent access rejection

### Phase 4: koto cancel

Add the workflow cancellation subcommand.

Deliverables:
- `WorkflowCancelled` variant in `EventPayload`
- `Command::Cancel` variant and handler
- `dispatch_next` / engine recognizes cancelled state as terminal
- Integration tests for cancel behavior

## Consequences

### Positive

- Auto-advancement removes the tedious agent back-and-forth for intermediate
  states. A 5-state workflow with 3 auto-advance states goes from 7 CLI calls
  to 1.
- The engine is unit-testable without filesystem or process spawning, catching
  loop logic bugs before they reach integration tests.
- Advisory flock prevents a class of corruption bugs that would be painful to
  debug in production.
- The closure interface means the integration runner can be built independently
  without modifying the engine.

### Negative

- `handle_next` refactor is nontrivial. The 340-line function needs restructuring
  to set up closures and call the engine, which touches the most complex code path
  in the CLI.
- Two classification systems coexist: `dispatch_next` returns `NextResponse`,
  the engine returns `StopReason`. The mapping between them is straightforward
  but adds a translation layer.
- Gate evaluation can delay signal-based shutdown by up to 30 seconds (the default
  gate timeout). Fast shutdown would require killing the child process group from
  the signal handler.
- `append_event` re-reads the entire file on every call to determine the next
  sequence number. The advancement loop calls it multiple times per invocation,
  making this progressively slower for long workflows.

### Mitigations

- The `handle_next` refactor is isolated to one function. The engine is new code
  with clean interfaces. Risk is contained.
- The `StopReason` → `NextResponse` mapping is a single match expression. The
  types are aligned by design.
- Gate timeout delay is acceptable per the upstream design ("complete the
  in-progress atomic append before exiting"). Workflows sensitive to shutdown
  latency can use shorter gate timeouts.
- The `append_event` performance concern can be addressed by passing the expected
  next sequence number as a parameter, avoiding the file read. This is a targeted
  optimization for a later issue.

## Security Considerations

### Gate evaluation amplification

Auto-advancement can trigger multiple gate evaluations per `koto next` invocation.
Previously, each gate evaluation required a separate CLI call. This doesn't change
the trust boundary -- the same template author controls which shell commands run in
gates -- but it widens the blast radius per invocation.

Gate commands run with the user's full privileges, isolated only by process group
separation and timeout. This is an existing constraint, not introduced by this
design. If koto ever loads templates from untrusted sources, gate command sandboxing
must be implemented before auto-advancement is enabled for those templates.

### Directive interpolation escaping

The upstream design requested that this sub-design specify escaping rules for
directive rendering. The engine layer (`advance_until_stop`) does not perform
interpolation -- it returns typed `StopReason` variants with raw field values. The
handler maps these to `NextResponse` JSON, which the agent consumes directly.

The escaping boundary: `StopReason` fields contain raw, unescaped data. Any layer
that interpolates evidence values or integration output into rendered text (such as
directive template rendering in the controller layer) must treat those values as
untrusted strings and escape them appropriately. The engine's contract is to return
structured data, not rendered text.

The current CLI handler serializes `NextResponse` to JSON, which inherently escapes
strings. The escaping gap only opens if a future rendering layer interpolates
evidence or integration output into non-JSON contexts (Markdown, shell). A follow-up
issue should specify escaping rules in the controller/template rendering layer before
the integration runner is implemented, since integration output comes from arbitrary
subprocesses and compounds the injection risk.

### Download verification

Not applicable. The auto-advancement engine operates on local state files and
compiled templates. It does not download external artifacts. The future integration
runner config system (deferred) will need its own download verification review.

### User data exposure

Evidence and integration output are persisted as plaintext JSON in the event log.
The existing `append_event` function applies 0600 file permissions on creation,
limiting access to the file owner. The auto-advancement engine uses the same
persistence path via injected closures and does not introduce new file access
patterns.
