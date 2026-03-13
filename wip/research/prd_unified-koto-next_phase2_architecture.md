# Phase 2 Research: Architecture Perspective

## Lead 3: Integration ownership boundary

### Findings

**Current gate system (command gates):**
The engine already owns subprocess invocation for deterministic gates. From `pkg/engine/engine.go`, lines 603-649, `evaluateCommandGate()` runs shell commands via `sh -c` with a configurable timeout (default 30s) and captures exit codes. This happens during `Transition()` validation, between transition validation (line 203) and commit (line 234). Gates are exit conditions evaluated before leaving a state.

The gate types are:
- `field_not_empty`: check evidence map for non-empty field
- `field_equals`: check evidence value against expected
- `command`: execute shell command, pass if exit code 0

Gates are declared per-state in the template format (`pkg/engine/types.go` line 49, `MachineState.Gates map[string]*GateDecl`). Each gate has type, optional field/value/command strings, and timeout (seconds).

**Command gate contract:**
- koto runs the command itself via `sh -c`
- No variable interpolation in the command string (security boundary, line 616)
- Command executes from git repo root (or CWD if not in git, lines 621-622)
- Timeout is enforced (default 30s, configurable per gate)
- Only exit code 0 means pass; any other exit code or timeout fails the gate
- No stdout/stderr capture (lines 618-619: `cmd.Stdout = nil, cmd.Stderr = nil`)

**Delegation design (from DESIGN-cross-agent-delegation.md):**
The proposed delegation model has koto owning subprocess invocation for delegate CLIs, but in a different way than command gates. Key decision (lines 351-360): `koto delegate run --prompt-file <file>` is the subcommand the agent calls to hand a prompt to the delegate. koto then:

1. Reads the current state (same as any state-dependent command)
2. Re-resolves the delegation target from tags + config (same logic as `Next()`)
3. Pipes the prompt to the delegate CLI via stdin
4. Captures stdout with timeout (10 MB cap, configurable timeout default 300s, lines 364-367)
5. Returns structured JSON to stdout (lines 391-427): `{response, delegate, matched_tag, duration_ms, exit_code, success}`

The delegate interface contract (lines 370-387):
- Input: raw prompt text via stdin
- Output: raw text captured from stdout
- Working directory: same as koto (read-write access)
- Environment: full inheritance from koto
- Permissions: same user (no elevation, no sandboxing)
- Interaction model: synchronous, single exchange (prompt in, response out)

**Comparison: command gates vs. delegates:**

| Aspect | Command Gates | Delegates |
|--------|---------------|-----------|
| **Invocation timing** | During transition validation (gate evaluation, line 203) | After directive is returned, agent calls `koto delegate run` |
| **Purpose** | Block/allow state exit (exit conditions) | Process the directive (extended reasoning, specialized tools) |
| **Command source** | Template-declared (in state definition) | Config-declared (delegation rules map tags to targets) |
| **Input** | None (just runs, checks exit code) | Agent-supplied prompt via stdin |
| **Output** | Exit code only, not captured | Full stdout captured, returned as JSON |
| **Stdout/stderr handling** | Discarded (lines 618-619) | Stdout captured (up to 10 MB), stderr inherited to terminal |
| **Subprocess availability** | Assumed to exist; failure blocks gate | Checked via DelegateChecker interface before invocation |
| **Failure handling** | Gate fails, transition rejected | JSON response carries success/error; `koto delegate run` exits 0 if delegate was invoked |
| **Integration config** | None (command is in template) | Delegation config (targets, rules, allow_project_config, timeout) |

**Why koto owns both:**

Command gates (lines 586-649): koto evaluates these because they're exit conditions that determine whether transitions are allowed. They're part of the state machine semantics.

Delegates (delegation design lines 84-85, 439): koto owns delegate invocation because "delegate invocation is deterministic. Checking CLI availability, invoking a subprocess, capturing stdout, handling timeouts -- these don't require AI judgment. koto can own them." More specifically, lines 301-343 explain that putting delegation in `Next()` output lets agents know about delegation before doing work (vs. gates which run too late), and the DelegateChecker interface prevents side effects from breaking testability.

The unifying principle: **koto owns integrations it knows the deterministic contract for**. For command gates, the contract is "run this shell command, check exit code." For delegates, the contract is "invoke this CLI with a prompt, capture output, return it as JSON."

### Requirements implications

**1. Integration specification must distinguish ownership layers:**

Command gates are specified in the template (state-level declarations), so template authors directly express what koto should run. They're evaluated during `Transition()`, before any agent action, as part of gate semantics.

Delegates are specified in config (not the template), mapping tags (declared in template) to targets (defined in config). The template says "this state needs deep reasoning" (a tag); the config says "map deep-reasoning to the gemini CLI" (a rule). The controller checks tags against config at `Next()` time (lines 300-327 of delegation design).

**Requirement:** The PRD must specify that koto-owned integrations come in two categories:
- **Gate-owned** (template): integration deterministically gates transitions
- **Controller-owned** (config): integration processes the directive at the agent's discretion

**2. Integration response handling differs:**

Command gates have no response pathway — they block or allow. If a gate fails, the transition is rejected with a `gate_failed` error (engine.go line 560, 573, 594).

Delegates return structured responses that the agent processes. The agent reads the delegate response and calls `koto transition` to advance, supplying evidence from the delegate response. From delegation design lines 455-462, the flow is:
1. `koto next` returns delegation info
2. Agent crafts prompt
3. `koto delegate run --prompt-file` returns `{response, ...}`
4. Agent uses response, calls `koto transition`

**Requirement:** The PRD must specify that integration responses flow differently:
- Gate responses: deterministic, control-flow (block transition)
- Delegate responses: informational, supplied as evidence by agent

**3. Availability checking has different trust models:**

Command gates assume the command is available (no pre-check). If it's missing, the gate fails.

Delegates are checked for availability before invocation via the DelegateChecker interface (delegation design lines 335-343). If the CLI isn't in PATH, `Delegation.Available` is false in the response, and `Delegation.Fallback` may be true (agent handles directive without delegation). This opt-in fallback prevents surprises.

**Requirement:** The PRD must specify that availability checking differs:
- Gates: fail if command missing (assumption of availability)
- Delegates: checked in advance, fallback path available (optional delegation)

**4. Config separation:** The delegation design explicitly separates targets (what binary to run) from rules (tag to target mapping). Project config can only add rules, not targets (delegation design lines 255-260). This is a security boundary — a cloned repo can't specify an arbitrary binary for koto to execute.

**Requirement:** The PRD must specify that integration config lives in different locations:
- Gate commands: in template (author-controlled, reviewed at template authoring time)
- Delegate targets: in user config only (user-controlled, project config adds routing only)

**5. Timeout configuration:**

Command gates: timeout in template per-gate, default 30s (template format design, line 260)

Delegates: timeout in delegation config globally, default 300s (delegation design line 232, 435)

**Requirement:** The PRD must specify timeout scoping — gates have per-gate timeout (template), delegates have global timeout (config).

### Open questions

1. **Gate vs. controller ownership in unified `koto next`:**
   The unified-koto-next PRD scope says "`koto next` runs koto-owned integrations (CI checks, delegate CLIs) and advances automatically when all conditions are met." But gate evaluation and delegate invocation are conceptually different — gates determine if we *can* advance, delegates provide data the agent uses to advance. How should the unified model express this distinction in the command interface and JSON output?

   For example, should a gate failure cause `koto next` to return an error, or should `koto next` only fail on I/O errors? The current `Transition()` fails if gates don't pass, but in the unified model, if `koto next` auto-advances, what happens to unsatisfied gates?

2. **Evidence flow for delegate responses:**
   The delegation design shows the agent reading the delegate response from `koto delegate run` JSON and then supplying it as evidence in `koto transition`. But if `koto next` subsumes transitions, how does evidence from the delegate get into the workflow state? Does the agent pass `--evidence key=value` to `koto next`? Or does `koto delegate run` integrate directly with `koto next` to chain them?

3. **Fallback behavior when integrations aren't available:**
   Command gates fail hard if unavailable. Delegates have a fallback (`Delegation.Available = false, Fallback = true`). Should unified `koto next` support fallback for *all* integrations, or just delegates? What's the security implication of silently skipping a required check?

4. **Branching via evidence on different transitions:**
   The unified-koto-next scope mentions "branching workflows: agent controls branch via evidence that satisfies mutually exclusive gate conditions on transitions." This implies transition-level gates (not current state-level gates). But the delegation design shows tags on states (not transitions), and config maps tags to delegates. Should transitions also support tags for delegation? Or is delegation only state-level?

5. **Integration audit trail:**
   Command gates leave no audit trail (exit code only, not captured). Delegates return structured responses. Should the unified model store which integrations were invoked, with what inputs/outputs, for debugging? The state file already has a history of transitions — should it also have a history of integration invocations?

## Summary

koto's current design partitions integration ownership by execution timing and purpose: command gates are template-declared, evaluated during transition validation, and control state-exit semantics; delegate invocation is config-declared, invoked at agent discretion via a separate subcommand, and provides data for evidence-based advancement. The unified `koto next` model must clarify how these two patterns compose — whether gate failures block `koto next` entirely, whether delegate responses flow as evidence into `koto next`'s auto-advancement logic, and how the config system distinguishes integration types that koto can safely own from those requiring agent judgment.

