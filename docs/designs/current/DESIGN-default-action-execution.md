---
status: Current
spawned_from:
  issue: 71
  repo: tsukumogami/koto
  parent_design: docs/designs/DESIGN-shirabe-work-on-template.md
problem: |
  koto's engine can verify outcomes via gates but can't execute deterministic work
  itself. Five states in the work-on template need default actions (run a script,
  create a branch, poll CI) that should auto-execute without agent involvement on
  the happy path. Without this capability, agents perform mechanical work that koto
  should handle, keeping ~42% of skill instructions that could be eliminated.
decision: |
  Add a default_action field to TemplateState with an ActionDecl struct (command,
  working_dir, requires_confirmation, optional polling config). A new fourth closure
  on advance_until_stop handles execution between integration and gate evaluation.
  Output is captured in a new DefaultActionExecuted event type. Override evidence
  in the current epoch prevents execution. Polling wraps the same execute-then-check
  loop with interval and timeout for ci_monitor.
rationale: |
  The advance loop chains through multiple states in a single koto next call, so
  action execution must happen inside the loop (not before it). A dedicated closure
  keeps the engine I/O-free while giving it the right call point. The integration
  closure has wrong semantics (always halts, returns output). A unified action model
  with optional polling avoids a separate execution_model discriminator for one
  polling state out of five. The evidence epoch system already solves override
  timing without new schema.
---

# DESIGN: Default action execution

## Status

Proposed

## Context and problem statement

Issue #71 requires koto's engine to execute a default action when entering a
deterministic state, capture the command's output, and evaluate the state's gate
against the result. The parent design (DESIGN-shirabe-work-on-template.md, Phase 0b)
specifies this as the mechanism that makes the automation-first principle concrete:
koto runs deterministic work itself instead of delegating to an agent.

The engine already has the pieces: `advance_until_stop` in `src/engine/advance.rs`
drives state transitions through closure-injected gate evaluation, and an integration
closure stub exists in `handle_next` (line 888) that always returns `Unavailable`.
Gate evaluation in `src/gate.rs` already handles process isolation, timeouts, and
output capture. What's missing is the schema declaration, the execution trigger, and
the wiring between action output, gate evaluation, and the advance loop.

Two execution models are needed: one-shot (run once, check result) for four states
(`context_injection`, `setup_issue_backed`, `setup_free_form`, `staleness_check`) and
polling/retry (run repeatedly until success or timeout) for `ci_monitor`.

A safety constraint governs auto-execution: only reversible actions run by default.
Irreversible actions (PR creation, posting comments) require agent confirmation via
the template schema. The three-path model (Decision 6+8 in the parent design) defines
the default/override/failure paths this capability enables.

The `--var` substitution interface (implemented in #67, now merged) is available for
action commands via `Variables::substitute()`. Action commands referencing
`{{ISSUE_NUMBER}}` or `{{ARTIFACT_PREFIX}}` resolve through the same mechanism as
gate commands.

## Decision drivers

- **Automation-first**: every deterministic step that koto can execute should be
  executed by koto, not delegated to an agent
- **Two execution models**: one-shot (4 states) and polling/retry (1 state) have
  different semantics but should share infrastructure where possible
- **Safety via reversibility**: only reversible actions auto-execute; irreversible
  actions require agent confirmation
- **Output capture**: action stdout/stderr/exit-code must be persisted in the event
  log and available to the agent on fallback (gate-with-evidence-fallback from #69)
- **Override prevention**: evidence submitted before state entry prevents default
  action execution
- **Variable substitution**: action commands use `{{VAR}}` patterns resolved by the
  existing Variables::substitute() interface
- **Minimal engine API change**: avoid restructuring advance_until_stop's core loop
- **Template schema clarity**: template authors need a clear, declarative way to
  specify default actions per state

## Considered options

### Decision 1: schema declaration and engine execution point

Default actions need a template schema declaration and an execution point inside the
engine's advance loop. The advance loop chains through multiple states in a single
`koto next` call — any execution point outside the loop would miss actions on
intermediate states.

Key assumptions:
- A state won't have both `integration` and `default_action` (integration preempts)
- `requires_confirmation` stops the loop after execution, returning a new StopReason
- Action commands use the same `sh -c` model as gates

#### Chosen: new ActionDecl field on TemplateState, new action closure on advance_until_stop

Add `default_action: Option<ActionDecl>` to `TemplateState`. Add a fourth closure
parameter to `advance_until_stop` for action execution. The engine calls it after
the integration check but before gate evaluation, when the current state has a
default_action and no override evidence exists.

The advance loop already takes three closures (`append_event`, `evaluate_gates`,
`invoke_integration`). A fourth follows the same injection pattern. The engine
decides when to call it based on template state; the closure implementation lives
in `handle_next` where it has access to Variables, working directory, and the
event appender.

Execution order within the loop after this change:

```
1. Signal check
2. Chain limit check
3. Terminal check
4. Integration check (existing, stops loop)
5. Override check: if current_evidence non-empty, skip to step 7
6. Action execution: call action closure, append DefaultActionExecuted event
   - If requires_confirmation: stop loop with ActionRequiresConfirmation
7. Gate evaluation (existing, stops loop on failure)
8. Transition resolution (existing)
```

#### Alternatives considered

- **Execute via integration closure**: the integration closure returns
  `Result<Value, IntegrationError>` and always halts the loop. Actions need different
  behavior — side effects, conditional halting, no return value as StopReason.
  Overloading the closure conflates two distinct concepts.
- **Execute in handle_next before advance loop**: fatally flawed — the advance loop
  chains through multiple states, so pre-loop execution misses actions on intermediate
  states. The five use cases include states reached via auto-advancement.

### Decision 2: polling/retry execution model

The `ci_monitor` state polls CI status repeatedly. Four other states run once. The
execution model must handle both without unnecessary complexity.

Key assumptions:
- Only 1 of 5 states needs polling; the model should be parameter-driven, not type-driven
- The polling loop uses the same signal/shutdown checks as the advance loop
- Gates stay single-shot predicates; polling is action-level orchestration

#### Chosen: unified action model with optional polling parameters

`ActionDecl` has `polling: Option<PollingConfig>` where `PollingConfig` contains
`interval_secs` and `timeout_secs`. When `polling` is `None`, the action runs once.
When `Some`, the action closure loops: execute command, evaluate gates, sleep interval,
repeat until gates pass or timeout.

The type system serves as the discriminator — `Option` is the execution model selector.
No separate `execution_model` field needed.

```rust
pub struct PollingConfig {
    pub interval_secs: u32,
    pub timeout_secs: u32,
}
```

The polling loop lives in the action closure implementation, not in the engine. The
advance loop calls the action closure once; the closure internally handles polling if
the ActionDecl has polling config. The advance loop sees the same interface regardless.

#### Alternatives considered

- **Separate execution models with enum discriminator**: redundant — the presence of
  polling config already implies the model. More schema surface and code duplication
  for one polling state.
- **Polling via gate retry**: conflates gate evaluation (bounded, single-shot) with
  orchestration (unbounded, repeated). Gates have a 30s default timeout; making them
  block for 30 minutes breaks their contract.

### Decision 3: output capture and override prevention

Action output must persist in the event log for audit and for agent fallback. Override
evidence must prevent execution to support the three-path model.

Key assumptions:
- The event log is the single source of truth; sidecar files break atomicity
- Evidence epoch scoping already exists via `derive_evidence()` and `merge_epoch_evidence()`
- Action stdout/stderr is bounded (short-lived commands, not streaming)

#### Chosen: new DefaultActionExecuted event type + epoch evidence check

**Output capture**: a new `EventPayload` variant:

```rust
DefaultActionExecuted {
    state: String,
    command: String,
    exit_code: i32,
    stdout: String,
    stderr: String,
}
```

Appended to the event log after each action execution. The `command` field records
what ran (after variable substitution) for auditability. Stdout/stderr truncated at
64KB with a note if exceeded.

**Override prevention**: before running the action, check if evidence exists in the
current epoch. `advance_until_stop` receives `evidence` as a parameter — if it's
non-empty, the agent has spoken and the action is skipped. No new schema needed;
the rule is universal.

#### Alternatives considered

- **Reuse IntegrationInvoked event**: wrong payload shape (single `output: Value` vs
  separate stdout/stderr/exit_code fields). Mixing action and integration events
  requires additional discrimination logic everywhere events are queried.
- **Sidecar files for output**: breaks the single-file atomicity invariant of JSONL
  state. If sidecar write succeeds but event append fails, state becomes inconsistent.
- **Pre-action gate check for overrides**: adds schema complexity for something the
  engine can determine at runtime. The override rule doesn't vary per state.

## Decision outcome

The three decisions form a coherent execution model. ActionDecl on TemplateState
declares what to run. The action closure in advance_until_stop handles when to run
(after integration, before gates, only if no override evidence). DefaultActionExecuted
events capture what happened. The polling wrapper is parameter-driven, not
type-driven.

The data flow: template declares `default_action` → compiler validates and includes
in compiled JSON → engine enters state → checks for override evidence → calls action
closure → closure runs command (with variable substitution), captures output →
appends DefaultActionExecuted event → engine evaluates gates against the result →
on gate failure with accepts block, agent sees action output in fallback directive.

## Solution architecture

### Overview

The feature adds four components: a schema extension for action declarations, an
execution engine in the CLI layer (via closure), a new event type for audit and
fallback, and a polling wrapper for repeated execution.

### Components

**`src/template/types.rs` — schema extension**

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActionDecl {
    pub command: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub working_dir: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub requires_confirmation: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polling: Option<PollingConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PollingConfig {
    pub interval_secs: u32,
    pub timeout_secs: u32,
}
```

Add `default_action: Option<ActionDecl>` to `TemplateState`. Compile-time validation:
reject states with both `integration` and `default_action`; validate `{{VAR}}`
references in action commands (reuse `extract_refs` from template/types.rs).

**`src/template/compile.rs` — YAML parsing**

Add `default_action` to `SourceState`:

```rust
#[serde(default)]
default_action: Option<SourceActionDecl>,
```

Map to `ActionDecl` during compilation. Validate action commands aren't empty.

**`src/engine/advance.rs` — new closure parameter**

Add a fourth closure parameter to `advance_until_stop`:

```rust
pub fn advance_until_stop<F, G, I, A>(
    // ... existing params ...
    execute_action: &A,
    // ...
) -> Result<AdvanceResult, AdvanceError>
where
    A: Fn(&str, &ActionDecl) -> ActionResult,
```

`ActionResult` is a new enum:

```rust
pub enum ActionResult {
    Executed { exit_code: i32, stdout: String, stderr: String },
    Skipped,        // override evidence existed
    RequiresConfirmation { exit_code: i32, stdout: String, stderr: String },
}
```

New `StopReason` variant: `ActionRequiresConfirmation { state, output }` — returned
when `requires_confirmation` is true.

Execution order: after integration check (step 4), before gate evaluation (step 7).
If evidence is non-empty, the closure returns `Skipped` and the engine proceeds to
gates. If the action ran and `requires_confirmation`, the engine stops.

**`src/engine/types.rs` — new event type**

```rust
DefaultActionExecuted {
    state: String,
    command: String,
    exit_code: i32,
    stdout: String,
    stderr: String,
}
```

Add `type_name()` mapping, deserialization helper, and serialization support following
the existing pattern.

**`src/cli/mod.rs` — action closure in handle_next**

The closure captures `&variables`, `&current_dir`, and `&mut append_closure`:

```rust
let action_closure = |state_name: &str, action: &ActionDecl| -> ActionResult {
    // Check override evidence (passed by engine, or check epoch)
    // Substitute variables in command
    let command = variables.substitute(&action.command);
    // Determine working dir
    let wd = if action.working_dir.is_empty() {
        current_dir.clone()
    } else {
        PathBuf::from(variables.substitute(&action.working_dir))
    };
    // Execute (one-shot or polling)
    if let Some(polling) = &action.polling {
        execute_polling(&command, &wd, polling, &gates, &variables, &shutdown)
    } else {
        execute_oneshot(&command, &wd)
    }
    // Append DefaultActionExecuted event
    // Return ActionResult
};
```

**`src/action.rs` — new module for command execution**

Factor the shell command execution from `gate.rs` into a shared utility:

```rust
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn run_shell_command(command: &str, working_dir: &Path, timeout_secs: u32) -> CommandOutput
```

Both `gate.rs` and the action executor use this. The action executor adds stdout/stderr
capture (gates currently discard output).

### Key interfaces

| Interface | Signature | Used by |
|-----------|-----------|---------|
| `ActionDecl` | struct on `TemplateState` | template compiler, advance loop |
| `execute_action` closure | `Fn(&str, &ActionDecl) -> ActionResult` | advance_until_stop |
| `ActionResult` | enum: Executed, Skipped, RequiresConfirmation | advance loop, handle_next |
| `DefaultActionExecuted` | EventPayload variant | event log, fallback directive |
| `run_shell_command` | shared utility | gate.rs, action executor |

### Data flow

```
Template YAML         Compilation          Init              Runtime (koto next)
─────────────        ────────────         ────              ───────────────────

default_action:  →   ActionDecl in    →   (no change)   →  advance loop enters state
  command: "..."     compiled JSON                          → override check (evidence epoch)
  polling: ...                                              → action closure executes command
                                                            → DefaultActionExecuted event
                                                            → gate evaluation
                                                            → transition or fallback
```

## Implementation approach

### Phase 1: shared command execution and schema

Factor `run_shell_command` from gate.rs into `src/action.rs`. Add `ActionDecl`,
`PollingConfig` to template types. Add YAML parsing in compile.rs. Add compile-time
validation (no integration+action, variable refs in commands). Add
`DefaultActionExecuted` event type.

Deliverables:
- `src/action.rs` — shared command execution with output capture
- `src/template/types.rs` — ActionDecl, PollingConfig, TemplateState field
- `src/template/compile.rs` — YAML parsing for default_action
- `src/engine/types.rs` — DefaultActionExecuted event variant
- Refactored `src/gate.rs` to use shared utility
- Unit tests for all new types

### Phase 2: engine integration

Add the fourth closure parameter to `advance_until_stop`. Add `ActionResult` enum and
`StopReason::ActionRequiresConfirmation`. Implement the override evidence check and
action execution call in the advance loop. Update existing tests with the new closure
parameter.

Deliverables:
- `src/engine/advance.rs` — fourth closure, execution order, new StopReason
- Updated tests for advance_until_stop

### Phase 3: CLI wiring and polling

Implement the action closure in `handle_next`. Add one-shot execution. Add polling
loop with interval, timeout, and shutdown checks. Map ActionRequiresConfirmation to
a new NextResponse variant. Add end-to-end tests.

Deliverables:
- `src/cli/mod.rs` — action closure in handle_next, NextResponse mapping
- End-to-end tests for one-shot, polling, override, requires_confirmation

## Security considerations

Action commands run via `sh -c` with the user's credentials, same as gate commands.
The threat model is identical to gates: commands are static strings in the compiled
template with `{{VAR}}` substitution. Variable values are sanitized at init time
via the allowlist from #67.

**New risk: action output in event log.** Action stdout/stderr is persisted in the
state file, which is committed to feature branches. If an action command's output
contains secrets (e.g., a script that prints an API key), those secrets end up in
the event log. Mitigation: document that action commands should not produce
sensitive output. Truncation at 64KB limits exposure from unexpectedly verbose
commands.

**Reversibility constraint.** The `requires_confirmation` flag prevents irreversible
actions from auto-executing. The engine enforces this at the loop level — when the
flag is set, the loop stops and returns to the caller. The template author is
responsible for setting this flag correctly; the engine can't determine reversibility
automatically.

**Polling timeout.** The polling loop has a configurable timeout. A missing or
excessively long timeout could block the workflow indefinitely. Compile-time
validation should enforce a maximum timeout (e.g., 1 hour) and require the field
when polling is declared.

## Consequences

### Positive

- Five deterministic states auto-advance on the happy path without agent involvement
- Action output captured for audit and agent fallback — no information lost
- The three-path model (default/override/failure) is fully operational
- Polling is parameter-driven, not a separate execution model
- The shared run_shell_command utility eliminates duplication between gates and actions

### Negative

- advance_until_stop gains a fourth closure parameter, breaking all existing test
  call sites (mechanical fix: add a no-op closure)
- Action output in the event log increases state file size, though bounded by
  truncation
- The polling loop blocks the CLI process during polling. If koto needs concurrent
  workflow advancement, this would need restructuring. Current use case (one workflow
  at a time) is fine.

### Mitigations

- Test updates are mechanical — a `noop_action` closure matches the existing
  `noop_gates` and `unavailable_integration` test helpers
- Output truncation at 64KB prevents unbounded growth
- Polling timeout enforcement at compile time prevents indefinite blocking

## Future direction: actions vs integrations

This design builds a low-level primitive: actions are "run an arbitrary shell command."
They're template-declared, dumb, and generic. The existing `integration` field on
TemplateState is a separate concept reserved for a higher-level abstraction: named
capabilities with typed inputs and structured outputs.

The distinction matters for cases like CI monitoring. Today, `ci_monitor` is modeled
as an action with polling (shell command `gh pr checks` in a retry loop). A future
GitHub CI integration would be a typed connector that calls the API directly,
understands check statuses natively, and returns structured data — not a shell command
whose output gets string-parsed.

The migration path is clean: a template state switches from `default_action` (with
polling) to `integration: github-ci` when the integration exists. The design enforces
mutual exclusivity between the two fields, so there's no ambiguity about which
mechanism runs. The integration closure's signature already returns structured
`serde_json::Value`, which is the right shape for typed connectors.

Integrations would bring domain knowledge into koto (or add-ons): how to talk to
GitHub's API, how to parse plan/design doc formats, how to manage artifacts. Actions
don't have this knowledge — they just run commands. Both mechanisms coexist on the
same execution point in the advance loop (integration checks run before action checks),
so adding integrations later doesn't require rearchitecting the action system.
