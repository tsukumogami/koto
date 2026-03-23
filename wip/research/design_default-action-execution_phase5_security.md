# Security Review: DESIGN-default-action-execution

**Design**: `docs/designs/DESIGN-default-action-execution.md`
**Reviewer role**: Security researcher
**Date**: 2026-03-22

## Executive summary

The design extends koto's engine to auto-execute shell commands defined in templates. The existing variable substitution allowlist (`^[a-zA-Z0-9._/\-]+$`) provides solid defense against command injection through `{{VAR}}` patterns. The primary residual risks are secret leakage through captured stdout/stderr and the trust model for template content itself. None of the findings are blocking; all are addressable within the proposed architecture.

**Recommended outcome**: OPTION 1 (proceed as designed), with the mitigations below folded into implementation.

---

## Dimension 1: External artifact handling

**Risk**: Action output (stdout/stderr) is persisted in the JSONL event log, which is committed to feature branches.

**Analysis**: The design acknowledges this and proposes a 64KB truncation limit. The concern is not size but content -- commands like `env`, `printenv`, or scripts that dump config could leak API keys, tokens, or credentials into git history. Once committed, secrets are recoverable even after branch deletion.

**Current mitigation in design**: Documentation guidance ("action commands should not produce sensitive output") and 64KB truncation.

**Recommended additional mitigations**:
- Scrub common secret patterns from captured output before persisting (e.g., strings matching `^(ghp_|sk-|AKIA|xox[bpas]-)`). This is defense-in-depth, not a guarantee.
- Consider a `redact_output: true` flag on ActionDecl that replaces stdout/stderr with exit-code-only in the event. Useful for commands where the agent doesn't need the output on fallback.
- Log a warning to stderr (visible to the operator but not persisted) when output exceeds a threshold or contains patterns that look like secrets.

**Severity**: Medium. Secrets in git history are a real incident vector. The design's documentation-only approach relies entirely on template author discipline.

---

## Dimension 2: Permission scope

**Risk**: Action commands run with the user's full credentials via `sh -c`, same as gate commands.

**Analysis**: This is an existing trust boundary, not a new one. Gate commands already have this access (`src/gate.rs` line 68-69). The design correctly identifies that the threat model is identical to gates. The `requires_confirmation` flag provides a stop point for irreversible operations but doesn't restrict what the command can do -- it only pauses the advance loop.

**Current mitigation in design**: `requires_confirmation` flag for irreversible actions. Template author responsibility.

**Gap**: There is no mechanism to restrict what a default action can do vs. what a gate can do. A gate is read-only by convention (it checks a condition); an action is write-capable by design (it creates branches, runs scripts). If a template author accidentally omits `requires_confirmation` on a destructive action, it auto-executes.

**Recommended additional mitigations**:
- Consider a compile-time lint that warns when an action command contains patterns associated with irreversible operations (`git push`, `gh pr create`, `curl -X POST`, `rm -rf`). This is heuristic, not a hard block -- just a warning during `koto template compile`.
- The design's separation of concerns (action vs. gate) is sound. No architectural change needed.

**Severity**: Low. Same trust boundary as existing gates. The `requires_confirmation` mechanism is sufficient if template authors use it correctly.

---

## Dimension 3: Supply chain / dependency trust

**Risk**: Templates define the commands that auto-execute. A malicious or compromised template could run arbitrary commands.

**Analysis**: Templates are compiled JSON derived from YAML source files. The trust chain is: template author writes YAML -> `koto template compile` produces JSON -> `koto init` instantiates a workflow -> `koto next` executes actions from the compiled JSON. There is no template signing, hash verification, or provenance tracking.

This is the same trust model that already exists for gate commands. The design doesn't change the trust boundary -- it adds more commands that run within it. However, the auto-execution nature of default actions makes compromised templates more dangerous than before: a malicious gate command only runs when an agent calls `koto next` on a gated state, while a malicious default action runs automatically when the engine enters the state.

**Current mitigation in design**: Templates are compiled at a known point; the compiled JSON is what executes. Variable values are validated against an allowlist.

**Recommended additional mitigations**:
- Record the sha256 of the compiled template in the `WorkflowInitialized` event. This doesn't prevent attacks but enables post-incident forensics.
- Document that templates should be reviewed with the same scrutiny as CI pipeline definitions -- they are executable infrastructure.

**Severity**: Low (pre-existing trust model). The design doesn't weaken it, but auto-execution widens the blast radius of a compromised template.

---

## Dimension 4: Data exposure

**Risk**: The `DefaultActionExecuted` event payload includes the fully-substituted command string, stdout, and stderr.

**Analysis**: The `command` field records the command after `{{VAR}}` substitution (design line 199: "after variable substitution"). Variable values are constrained to `^[a-zA-Z0-9._/\-]+$`, so the substituted command won't contain secrets injected via variables. However, the command itself (from the template) could contain hardcoded paths, internal hostnames, or other sensitive context.

The stdout/stderr exposure was covered in Dimension 1. The command-string exposure is lower risk because it comes from the template (author-controlled), not from runtime output.

**Current mitigation in design**: 64KB truncation on stdout/stderr.

**Recommended additional mitigations**:
- Same as Dimension 1 (output scrubbing).
- No action needed for the command field itself -- template authors control it.

**Severity**: Medium (due to stdout/stderr content, not the command string).

---

## Dimension 5: Command injection

**Risk**: Action commands use `{{VAR}}` substitution and run via `sh -c`. If variable values could contain shell metacharacters, an attacker could inject commands.

**Analysis**: This is the highest-value attack vector for this design, and it is **well-mitigated** by the existing implementation.

The variable substitution system (`src/engine/substitute.rs`) validates all variable values against `^[a-zA-Z0-9._/\-]+$` (line 9). This allowlist blocks:
- Semicolons (`;`) -- command chaining
- Backticks and `$()` -- command substitution
- Pipes (`|`) -- piping
- Whitespace -- argument injection
- Quotes (`'`, `"`) -- escaping out of string context
- Ampersands (`&`) -- background execution
- Newlines -- command separation

The validation happens at two points:
1. At `koto init` time when variables are stored in the `WorkflowInitialized` event
2. At `Variables::from_events()` time as defense-in-depth re-validation (line 39-55)

Substitution is single-pass (line 210-217 test: `substitute_single_pass_no_reprocessing`), preventing recursive expansion attacks.

Variable references only match `[A-Z][A-Z0-9_]*` (the `VAR_REF_PATTERN` regex), so `{{lowercase}}` patterns in command strings pass through untouched. This prevents accidental substitution of template syntax that wasn't intended as a variable reference.

**One gap**: The allowlist permits forward slashes (`/`). While necessary for paths like `org/repo`, this means a variable value like `../../etc/passwd` would pass validation. If an action command does something like `cat {{FILE}}`, path traversal is possible. However, this is a template design issue (don't use variables in file-read commands without scoping), not a substitution bug.

**Current mitigation in design**: Relies on the existing allowlist from #67.

**Recommended additional mitigations**:
- The existing allowlist is strong. No changes needed for the substitution layer.
- Consider documenting that `working_dir` fields with `{{VAR}}` substitution should be validated to be within expected directory trees. The design proposes `working_dir` on ActionDecl (line 248), and this is subject to the same path-traversal concern.

**Severity**: Low. The allowlist effectively neutralizes shell injection. The path traversal concern is theoretical and requires a template author to write a vulnerable command pattern.

---

## Dimension 6: Override bypass

**Risk**: Could an attacker trick the engine into running an action that should be blocked by override evidence?

**Analysis**: The override check is straightforward: "if evidence is non-empty, skip to gate evaluation" (design line 117, step 5). The evidence comes from `merge_epoch_evidence()` in `src/engine/advance.rs` (line 352-362), which scans `EvidenceSubmitted` events in the current epoch.

The advance loop receives evidence as a parameter (`evidence: &BTreeMap<String, serde_json::Value>`) and the engine clears it on auto-advance (`current_evidence = BTreeMap::new()` at line 267). This means:

1. For the initial state (entered by the agent calling `koto next`), override evidence from prior `koto evidence` calls is checked.
2. For auto-advanced states (entered during the advance loop), evidence is always empty, so the action always executes.

This is correct behavior: auto-advanced states have no prior agent interaction, so there's nothing to override. The override mechanism only applies when an agent has explicitly submitted evidence before calling `koto next`.

**Potential concern**: The `requires_confirmation` flag lives in the template (ActionDecl), not in runtime state. If the compiled JSON is modified on disk between `koto init` and `koto next`, the flag could be removed. But this is a general template integrity issue (Dimension 3), not specific to override bypass.

**Another concern**: The design says the action closure returns `Skipped` when evidence is non-empty. But the check is "non-empty" -- meaning *any* evidence submission blocks the action, even if the evidence is unrelated to the action. This is conservative (safe by default) but could surprise template authors who submit partial evidence expecting the action to still run. The design acknowledges this: "The rule is universal" (line 205).

**Severity**: Low. The override mechanism is simple and conservative. No bypass vector identified.

---

## Dimension 7: Polling DoS

**Risk**: Could polling parameters cause resource exhaustion or indefinite blocking?

**Analysis**: The `PollingConfig` has `interval_secs: u32` and `timeout_secs: u32`. The design proposes compile-time enforcement of a maximum timeout (e.g., 1 hour) and requires `timeout_secs` when polling is declared (line 449-450).

**Scenarios**:
- `interval_secs: 0` -- tight loop, burns CPU. The design doesn't mention a minimum interval.
- `timeout_secs: u32::MAX` (4294967295 seconds, ~136 years) -- effectively infinite if compile-time max isn't enforced.
- `interval_secs > timeout_secs` -- the action runs once, then times out before the next poll. Not harmful, just wasteful.
- Network-bound commands in the poll loop -- each poll spawns `sh -c`, which spawns the actual command. If the command connects to an external service, a tight poll loop could look like a DoS against that service.

The polling loop runs in the action closure, which blocks the CLI process. The advance loop's shutdown flag (`AtomicBool`) is checked, so SIGTERM/SIGINT can break the poll. But the design says the polling loop "uses the same signal/shutdown checks as the advance loop" (line 143) -- this needs to be verified in implementation. If the poll loop sleeps without checking the shutdown flag between iterations, a long `interval_secs` means delayed shutdown.

**Current mitigation in design**: Compile-time maximum timeout. Shutdown flag integration.

**Recommended additional mitigations**:
- Enforce a minimum `interval_secs` at compile time (e.g., 5 seconds). A sub-second poll loop is almost never correct for CI monitoring.
- Enforce that `timeout_secs` > 0 and `timeout_secs` <= 3600 (or whatever the chosen max is) at compile time.
- In the poll loop implementation, check the shutdown flag before sleeping, not just at the top of the loop. Use a cancellable sleep (e.g., check shutdown flag every second within the interval) rather than `std::thread::sleep(Duration::from_secs(interval))`.
- Consider logging each poll iteration to stderr so operators can see progress and detect runaway polls.

**Severity**: Medium. Without minimum interval enforcement, a misconfigured template could burn CPU or hammer an external service. The compile-time validation the design proposes is necessary but needs the additional constraints above.

---

## Summary of findings

| Dimension | Severity | Key finding |
|-----------|----------|-------------|
| External artifact handling | Medium | Stdout/stderr in event log can leak secrets to git history |
| Permission scope | Low | Same trust boundary as existing gates |
| Supply chain trust | Low | Pre-existing model; auto-execution widens blast radius |
| Data exposure | Medium | Captured output is the primary exposure vector |
| Command injection | Low | Allowlist is strong; single-pass substitution is correct |
| Override bypass | Low | Conservative "any evidence blocks action" rule is safe |
| Polling DoS | Medium | Needs minimum interval and cancellable sleep |

## Recommended outcome

**OPTION 1**: Proceed as designed.

The design's security posture is solid. The variable allowlist effectively prevents command injection, the override mechanism is conservative, and the `requires_confirmation` flag addresses irreversible actions. The three medium-severity findings (output leakage, polling bounds) are addressable as implementation details without architectural changes. The mitigations above should be folded into the implementation phases.
