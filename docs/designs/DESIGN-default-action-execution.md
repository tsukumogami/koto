---
status: Proposed
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
  placeholder -- to be filled during design phases
rationale: |
  placeholder -- to be filled during design phases
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
- **Minimal engine API change**: the integration closure in handle_next is the natural
  injection point; avoid major changes to advance_until_stop's signature
- **Template schema clarity**: template authors need a clear, declarative way to
  specify default actions per state
