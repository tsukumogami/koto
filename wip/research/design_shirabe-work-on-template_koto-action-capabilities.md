# Research: koto Action Capabilities for Autonomous Template Execution

**Command:** design
**Phase:** koto-action-capabilities
**Role:** koto-engine-researcher

## Question

What koto capabilities would be needed for a template to execute deterministic
work autonomously (without agent involvement)? The goal is to invert the current
pattern: instead of the agent running "create git branch" and koto verifying it
via gate, koto runs the command and the agent is the fallback if it fails.

---

## 1. Does koto have any mechanism to execute commands on state entry?

**Answer: No.**

The engine has no concept of "entry actions." The relevant source files are:

- `src/engine/advance.rs` — the advancement loop
- `src/engine/types.rs` — event log types
- `src/template/types.rs` — the compiled template schema
- `src/gate.rs` — the gate evaluator

The advancement loop in `advance_until_stop()` (`src/engine/advance.rs`) iterates
through states in this fixed order per iteration:

1. Check shutdown flag
2. Check chain limit
3. Check terminal
4. Check integration (invoke runner, stop)
5. Evaluate gates (stop if any fail)
6. Resolve transition (append event, continue loop)

Steps 4 and 5 both involve external command execution, but neither is
"on state entry" in the template-author sense:

- **Gates** (step 5) are read-only exit-code checks. The gate evaluator in
  `src/gate.rs` spawns `sh -c <command>` and checks exit code. It captures
  nothing from stdout. It runs after the engine has entered the state and
  blocks advancement if any gate fails.

- **Integration** (step 4) is an injected closure that takes a string name and
  returns a JSON value. The current CLI handler always returns
  `IntegrationUnavailable` — there is no config system or subprocess runner
  for integrations. The integration field on a state is a string tag only;
  what it means is deferred.

Neither construct supports "run this command on entry and record its output."

The `TemplateState` struct (`src/template/types.rs`) fields are:

```rust
pub struct TemplateState {
    pub directive: String,
    pub transitions: Vec<Transition>,
    pub terminal: bool,
    pub gates: BTreeMap<String, Gate>,
    pub accepts: Option<BTreeMap<String, FieldSchema>>,
    pub integration: Option<String>,
}
```

There is no `on_enter`, `run`, `exec`, `actions`, or equivalent field.

The compiled template JSON format (`format_version: 1`) has the same schema.
The compiler (`src/template/compile.rs`) parses YAML front-matter with the same
fields.

---

## 2. Does the template schema have any field for commands to execute?

**Answer: No. The only command execution field is `gates`, which is read-only.**

The template source YAML (`SourceState` in `src/template/compile.rs`) supports:

```yaml
states:
  my_state:
    transitions: [...]
    terminal: bool
    gates:
      gate_name:
        type: command
        command: "./check.sh"
        timeout: 30
    accepts:
      field_name:
        type: enum
        values: [...]
        required: true
    integration: "some-runner-name"
```

Gates (`type: command`) are the only mechanism that runs shell commands. The
evaluator runs the command in a subprocess, waits for exit, and returns
`GateResult::Passed | Failed | TimedOut | Error`. Stdout and stderr are not
captured; there is no mechanism to route the output anywhere.

The `integration` field is a string tag only. No subprocess runner is wired up.

**What was recently removed:** The legacy `field_not_empty` and `field_equals`
gate types were explicitly removed. The error messages in both the compiler and
the type validator redirect users to `accepts/when`. This means only `command`
gates survive, reinforcing the gate-as-environment-check (not
gate-as-action-executor) design intent.

---

## 3. Conceptual architecture for "state entry actions"

### What would need to exist

State entry actions are commands that koto runs when entering a state, before
the agent sees the directive. They differ from gates:

| | Gates | Entry Actions |
|---|---|---|
| Purpose | Verify environment | Mutate environment |
| Evaluation | Exit code only | Exit code + stdout |
| On failure | Block advancement | Fallback to agent |
| Direction | Read-only check | Write operation |

### Where they would fit in the advancement loop

The most natural insertion point in `advance_until_stop()` is between state
entry and gate evaluation. The loop currently proceeds:

```
enter state
  → check terminal
  → check integration          ← current execution hook
  → evaluate gates             ← read-only checks
  → resolve transition
```

With entry actions, it would become:

```
enter state
  → check terminal
  → run entry actions          ← new: mutate environment
  → evaluate gates             ← unchanged: verify result
  → resolve transition
```

This ordering preserves the existing semantics: gates still verify that the
environment is in the expected state before advancing. Entry actions are the
mechanism that produces that state. If an action fails, the engine would stop
and surface a new `StopReason` (e.g., `ActionFailed { name, exit_code, stderr }`),
giving the agent a diagnostic message and an opportunity to intervene.

### Template schema changes required

A new `actions` field on `TemplateState` would be needed:

```yaml
states:
  create_branch:
    actions:
      create_git_branch:
        type: command
        command: "git checkout -b feature/{{BRANCH_NAME}}"
        timeout: 10
        on_failure: agent_fallback   # or: abort
    gates:
      branch_exists:
        type: command
        command: "git rev-parse --verify feature/{{BRANCH_NAME}}"
    transitions:
      - target: next_state
```

**Key design questions for the schema:**

1. **`on_failure` behavior**: `agent_fallback` stops the loop and returns the
   failure to the agent with a new response variant. `abort` treats the action
   failure as fatal. Both are needed; the template author chooses per-action.

2. **Action output routing**: stdout from the action command is currently not
   captured by the gate evaluator. Action commands that produce output (e.g.,
   `gh issue create --json number`) need a way to route that output into the
   evidence map or into variable substitution for subsequent states. This is
   a significant new capability — it's not present anywhere in the current engine.

3. **Idempotency and re-entry**: koto's event log records state transitions but
   not action executions. If the engine re-enters a state (after a rewind, or
   on re-invocation of `koto next`), it would re-run the action. Template authors
   must either write idempotent actions or the engine must record `action_executed`
   events and skip re-execution within the same epoch.

4. **Variable substitution in commands**: the `{{VAR_NAME}}` interpolation in
   directives is not implemented for gate commands (confirmed in the shirabe
   work-on design doc: "`--var` CLI support is not implemented"). Actions would
   need this to pass workflow context into commands (e.g., branch names, issue
   numbers). This is a prerequisite gap.

---

## 4. Is there a concept of structured output routing?

**Answer: No. Exit code is the only routing signal from koto-executed commands.**

Current routing signals and their sources:

| Signal | Source | Routing Use |
|---|---|---|
| Exit code 0 / non-0 | Gate command stdout | Pass/fail only |
| Evidence fields | Agent via `--with-data` | Conditional transitions via `when` |
| Integration output JSON | Integration closure | Stored in `integration_invoked` event |

The integration mechanism (`EventPayload::IntegrationInvoked`) does capture
structured output — it stores a `serde_json::Value` in the event log. But:

1. The integration runner is not implemented (always returns `Unavailable`).
2. Even when invoked, integration output is stored in the event log but not
   automatically routed into transition evaluation. The agent gets the output
   in the `Integration` response variant and must submit evidence manually.

To add structured output routing from command stdout, the engine would need:

1. A way for actions to write structured output (JSON to stdout, or a specific
   format like `KOTO_OUTPUT={"field": "value"}`).
2. The engine parses and validates this output against a schema declared in the
   template.
3. The output is injected into the evidence map for the current state, allowing
   `when` conditions to route based on command results.

This would effectively make actions a first-class evidence producer: instead of
the agent submitting `{"decision": "approved"}`, the integration command outputs
`{"decision": "approved"}` and koto routes the transition automatically.

This is the minimal architecture for "koto runs the command, agent is the
fallback" — the command produces structured output that drives transitions, and
only on failure does the agent see the state.

---

## 5. Existing capabilities at the boundary

### hello-koto template (`plugins/koto-skills/skills/hello-koto/hello-koto.md`)

The hello-koto template shows the current ceiling of template capabilities:

```yaml
states:
  awakening:
    transitions:
      - target: eternal
    gates:
      greeting_exists:
        type: command
        command: "test -f wip/spirit-greeting.txt"
```

The gate checks file existence. The agent creates the file. koto verifies.
This is the exact pattern that state entry actions would invert: instead of
telling the agent to create the file, an action would create it, and the gate
would remain as verification.

### What the current integration field enables (but doesn't deliver)

The `integration: "some-runner-name"` field was designed to be the bridge to
external tool execution. The `IntegrationInvoked` event captures structured
JSON output. The architecture was designed with the integration runner as the
mechanism for "koto runs something and records the result." The runner is just
not wired up.

If the integration config system were implemented, it would look similar to
state entry actions from the template author's perspective: declare a tool name,
the engine runs it, the output is available. The difference is that integrations
currently stop the loop (returning `StopReason::Integration`), whereas entry
actions should not stop the loop on success — they should run, succeed, and let
the loop continue to gate evaluation.

---

## 6. Scope estimate for "state entry actions"

Adding state entry actions to koto requires changes across multiple layers.
Ordered by dependency:

### Layer 1: Variable substitution in commands (prerequisite)

**Files:** `src/gate.rs`, `src/template/types.rs`, `src/cli/mod.rs`
**Scope:** Medium (1–2 days)

Gate command strings currently have no variable substitution. Entry actions
(and useful gates) need to interpolate workflow variables (`{{BRANCH_NAME}}`,
`{{ISSUE_NUMBER}}`). This requires:
- Tracking resolved variables in workflow state
- A substitution function applied before command execution
- Compiler validation that referenced variables exist

### Layer 2: Template schema — `actions` field

**Files:** `src/template/types.rs`, `src/template/compile.rs`
**Scope:** Small (half day)

Add `actions: BTreeMap<String, Action>` to `TemplateState` and `SourceState`.
`Action` struct mirrors `Gate` but adds `on_failure` enum and optional `output`
schema. Compiler validates the new fields; validator rejects unsupported types.

### Layer 3: Action executor with stdout capture

**Files:** `src/gate.rs` (or new `src/action.rs`)
**Scope:** Small–Medium (1 day)

The existing gate evaluator in `src/gate.rs` runs commands but doesn't capture
stdout. A new action executor would:
- Run command with process group isolation (reuse the `setpgid` pattern)
- Capture stdout (pipe, read to string)
- Return `ActionResult { exit_code, stdout, stderr }`

### Layer 4: Structured output parsing

**Files:** New `src/action.rs` or in the executor
**Scope:** Small (half day)

Parse stdout as JSON and validate against the action's declared `output` schema.
On parse failure or schema mismatch, treat as action failure (surfaced per
`on_failure` policy).

### Layer 5: Advancement loop integration

**Files:** `src/engine/advance.rs`
**Scope:** Medium (1–2 days)

Add a pre-gate step in `advance_until_stop()`:
- Before gate evaluation, check if `template_state.actions` is non-empty
- Run all actions via the action executor closure
- On success: inject stdout JSON into the evidence map for the current epoch
- On failure with `on_failure: agent_fallback`: return new `StopReason::ActionFailed`
- On failure with `on_failure: abort`: return `StopReason::ActionAborted`

New `EventPayload::ActionExecuted { state, action, output }` event for audit log
(prevents re-execution in the same epoch).

### Layer 6: `koto next` output contract

**Files:** `src/cli/next_types.rs`, `src/cli/mod.rs`
**Scope:** Small (half day)

New `NextResponse::ActionFailed` variant (or extend `GateBlocked`) for surfaces
where the agent needs to intervene after action failure. Maps from new
`StopReason::ActionFailed`.

### Total scope

| Layer | Scope | Depends on |
|---|---|---|
| Variable substitution | Medium | — |
| Template schema | Small | — |
| Action executor | Small–Medium | Template schema |
| Output parsing | Small | Action executor |
| Advancement loop | Medium | Action executor, output parsing |
| CLI output | Small | Advancement loop |

**Rough total: 1–2 engineer-weeks** for a minimal but complete implementation.
The variable substitution prerequisite is independently useful and could be
delivered first.

---

## Summary

koto has no mechanism for state entry actions. Gates are the only command
execution primitive and they are read-only exit-code checks — stdout is not
captured and there is no routing based on command output. The integration field
exists as a string tag but the runner is not implemented; even when it is,
integrations stop the loop rather than continuing it.

Structured output routing does not exist for koto-executed commands. The agent
is the only source of evidence that drives conditional transitions. The
integration event type (`IntegrationInvoked`) does capture JSON output to the
event log, but it is not automatically fed into transition evaluation.

Adding state entry actions requires six layers of work: variable substitution in
commands (prerequisite), template schema additions, an action executor with
stdout capture, structured output parsing, advancement loop changes, and CLI
output changes. The estimated scope is 1–2 engineer-weeks. The architectural
fit is clean — actions slot naturally before gate evaluation in the existing
loop, and the existing gate executor provides the process isolation pattern
to follow.
