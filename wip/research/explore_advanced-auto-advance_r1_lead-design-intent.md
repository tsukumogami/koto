# Lead: Is the double-call pattern intentional or emergent?

## Findings

### The `advanced` Field Origin and Intent

The `advanced: bool` field was introduced in the **CLI output contract design** (DESIGN-unified-koto-next.md, committed March 17, 2026 as part of #56: "implement koto next CLI output contract"). The field appears in every NextResponse variant except Terminal, and according to the README and CLI usage guide:

> The `advanced` flag is `true` when the call itself caused a state change (via `--with-data` or `--to`).

This definition is **observational**, not prescriptive. It reports whether the *calling agent* triggered a transition via evidence submission or directed override, not whether the state machine itself advanced. The field was designed as **diagnostic feedback** to distinguish "I triggered this change" from "this was already the state."

**Key evidence:**
- PLAN-koto-cli-output-contract.md (Issue 4, line 109): "`advanced` field is populated by the caller (true when an event was appended before dispatching)"
- The field is present in all non-terminal response variants, consistently reporting whether an event *submission* (not auto-advancement) occurred
- README line 94: explicit definition limits the meaning to *agent-initiated* changes

### The Double-Call Pattern: Emergence, Not Design

The double-call pattern emerged when the **auto-advancement engine** (DESIGN-auto-advancement-engine.md, committed March 18, 2026) was introduced. The engine was designed to chain through unconditional/auto-advanceable states in a *single invocation*, returning `advanced: true` when one or more transitions were made. This created an asymmetry:

1. When an agent submits evidence (`--with-data`) that matches a condition, `advanced: true` means the agent caused that transition
2. When the auto-advancement engine chains through multiple states, `advanced: true` means the *engine* caused those transitions
3. There is no way to distinguish between them in the response

The scope document for this exploration (wip/explore_advanced-auto-advance_scope.md, line 13) explicitly identifies the issue:

> The skill's execution loop has an explicit workaround: "if `advanced: true`, run `koto next` again."

This workaround appears because:
- An agent submits evidence → `advanced: true` → state may have auto-advanced one or more steps
- Agent needs to know if it's now at a state requiring action or if auto-advancement stopped mid-chain
- Agent calls `koto next` again to verify

**The pattern is emergent because:**
- Auto-advancement was added *after* the `advanced` field was designed
- The field's meaning was overloaded to cover both agent-initiated changes and engine-initiated auto-advancement
- No design decision explicitly chose this behavior; it was a natural consequence of the architecture

### Evidence from Commit History

Chronologically, the designs show the progression:

1. **March 17, 2026** (commit a2cf9dc): CLI output contract design introduces `advanced: bool` as "true when an event was appended before dispatching" (agent-centric definition)
2. **March 18, 2026** (commit 7ec86a3): Auto-advancement engine design introduced with its own `advanced: bool` field in `AdvanceResult`, meaning "true if at least one transition was made" (engine-centric definition)
3. **March 23, 2026** (commit 6b0863f): Template variable substitution and default action execution adds a fourth closure to `advance_until_stop`, extending the same pattern

The designs are sequential, not concurrent. The auto-advancement engine design (section "Functional Test Scenario", line 493) shows `"advanced": true` in the expected output but **does not discuss or acknowledge** the semantic collision with the agent-initiated definition.

### Design Intent for Auto-Advancement

The auto-advancement engine was explicitly designed to **collapse multiple state transitions into a single response**. From DESIGN-auto-advancement-engine.md:

**Problem statement (line 44-47):**
> If the current state has no `accepts` block, passing gates, and an unconditional transition, the agent gets back a response that says "you can advance" but doesn't actually advance. The agent must call `koto next --to <target>` manually for every intermediate state, turning what should be automatic chaining into a tedious back-and-forth.

**Solution (line 472-485):**
> Total: 1 CLI call. The engine auto-advanced through `plan` and `implement`, appending `transitioned` events for each, and stopped at `verify` where agent input is required.

This design **intended** to eliminate the back-and-forth by having a single `koto next` call produce multiple transitions. However, it did not re-examine the semantics of `advanced: true` in the context of this new behavior. The field's meaning was left ambiguous.

### State Machine Philosophy and Architecture

The advanced field design sits at a boundary between two concerns:

1. **Engine concern**: State machine integrity and the audit trail. The event log records who caused each transition (auto-advancement produces `transitioned` events with `"condition_type": "auto"`). The engine knows whether transitions occurred.

2. **CLI concern**: Agent observability. The agent needs to know whether its action caused the response or whether it's reading state that was already there.

The `advanced` field conflates these by trying to serve both purposes with one boolean. The engine's auto-advancement feature works correctly (each transition is durable, and the audit trail is accurate), but the `advanced` field no longer uniquely identifies agent-initiated changes.

### Template and Skill Usage

The DESIGN-shirabe-work-on-template.md (commit 22307bf, status: Planned) shows how templates would use advanced phases:

- Deterministic states (context_injection, setup_issue_backed, staleness_check) are meant to auto-advance when gates pass
- The skill loop (plugins/koto-skills/AGENTS.md, lines 205-215) shows the workaround: check `advanced: true`, if so, call `koto next` again

The skill's workaround is **not documented in the design** as an intentional pattern. It's a practical response to the ambiguity.

## Implications

### For Issue #89 (Auto-advance past advanced: true phases)

The issue proposes eliminating the double-call by having auto-advancement continue deeper. However, this misinterprets the root cause:

**Root cause**: The `advanced` field is ambiguous about *who* caused the advancement. Agent-initiated vs. engine-initiated transitions are indistinguishable.

**Current symptoms**: After an agent submits evidence that triggers auto-advancement, `advanced: true` doesn't tell the agent whether it's at a checkpoint (awaiting input) or mid-chain.

**Proposed solution (Issue #89)**: Continue auto-advancing until hitting a state that requires agent input, then return.

**Problem with the proposal**: This doesn't add any new capability—the auto-advancement engine already does this. The issue might be conflating two different scenarios:
1. Agent submits evidence → auto-advances → returns at evidence-requiring state with `advanced: true`. Agent then calls `koto next` again to verify position.
2. Agent calls `koto next` with no input → gets an empty accepts block (no evidence needed) → returns with `advanced: true`. Agent must call again to proceed.

The second scenario (states with no accepts but auto-advanceable) is what the auto-advancement engine was designed to solve. If #89 is reporting the first scenario still requires a double-call, the issue is not that auto-advancement isn't happening—it's that the agent can't tell where it is after auto-advancement completes.

### Architectural Implications

The double-call pattern is **not** a violation of the state machine philosophy. It's a consequence of:

1. An intentional design decision to collapse multiple transitions into one response
2. An accidental ambiguity in the `advanced` field's semantics

To eliminate the double-call without changing the state machine, the solution is **semantic clarity**, not deeper auto-advancement. Options:

- **Option A**: Introduce a new field distinguishing agent-initiated from engine-initiated transitions (e.g., `advanced_by: "agent" | "engine"`)
- **Option B**: Return additional metadata: which state the agent is currently in and which state the auto-advancement stopped at
- **Option C**: Return a `transitions` array showing the chain of auto-advanced states
- **Option D**: Redefine `advanced` to mean "the current state changed during this call" (engine-centric), making it semantically consistent with the auto-advancement loop

Option D is architecturally simplest because it aligns `advanced` with the engine's actual behavior. But it's a breaking change to the current definition (agent-centric) and would require CLI layer adjustments.

## Surprises

1. **The auto-advancement engine was added *after* the `advanced` field was designed.** I expected the field to have been designed with auto-advancement in mind, but the chronology shows they were separate design efforts. The semantic collision is accidental.

2. **No explicit design discussion of the double-call pattern.** The DESIGN-shirabe-work-on-template.md (which uses auto-advancement heavily) mentions the pattern only as a side effect of how capabilities interact, not as an intentional design choice or workaround to be discussed.

3. **The distinction between "advanced by event submission" and "advanced by auto-loop" is structurally recorded in the event log** (different event types and `condition_type` fields) **but not reflected in the CLI response.** The engine has the information to disambiguate; the CLI layer chose to collapse it into a single boolean.

## Open Questions

1. **Is the goal to eliminate the double-call entirely, or to make it unnecessary?** Issue #89 asks to "auto-advance past advanced: true phases," but a skill could equally achieve the goal by having the agent check `advanced: true` and understand what it means (a transition occurred, but the agent should verify its position). Is the problem the double-call itself, or the cognitive load of disambiguating where the agent is?

2. **What is the intended use of `advanced: true` for library consumers?** The `advanced` field is part of the koto library's public API (returned by `dispatch_next`). Are library consumers (outside the CLI) expected to use it to decide whether to call `koto next` again? Or is it purely informational?

3. **Does deeper auto-advancement have downsides that were considered?** The auto-advancement engine has a 100-iteration limit and stops at evidence-requiring states by design. Removing those checkpoints (to avoid the double-call) would make the loop less observable. Was observability a reason the design chose to stop at evidence-required states?

4. **What does "advanced" mean post-integration invocation?** The design mentions that integration invocation returns as a stop reason. If an integration runs and `advanced: true` (the engine made transitions to reach it), does the agent understand that calling `koto next` again might invoke the integration again, or does it know the invocation is idempotent?

## Summary

The double-call pattern is **emergent, not intentional**. The `advanced` field was designed to report agent-initiated changes, but the auto-advancement engine (added later) overloaded it to also report engine-initiated transitions. This semantic collision forces agents and skills to double-call `koto next` to disambiguate their position. The state machine architecture is sound—transitions are durable and the audit trail is accurate—but the CLI's response semantics are ambiguous. **The core question is not whether auto-advancement should go deeper, but whether the `advanced` field should be disambiguated, redesigned, or supplemented with additional state metadata.**

