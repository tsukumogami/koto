# Lead: How do templates use `advanced` phases?

## Findings

### Survey of Existing Templates

A comprehensive search of the codebase reveals **only one production template currently in use**:

**File:** `/home/dgazineu/dev/workspace/tsuku/tsuku-7/public/shirabe/skills/work-on/koto-templates/work-on.md`

This is the primary workflow template for the work-on skill. Analysis shows:

1. **No `advanced: true` phase definitions in the template YAML.** The work-on template defines 14 states (entry, context_injection, task_validation, research, post_research_validation, setup_issue_backed, setup_free_form, staleness_check, introspection, analysis, implementation, finalization, pr_creation, ci_monitor, done, done_blocked) but **none include an `advanced: true` property** in their state definitions.

2. **The `advanced` semantics are CLI-level only.** The `advanced: true` field appears in the response from `koto next` (reported in DESIGN-koto-cli-output-contract.md) when transitions occur, but it is not a template-level phase configuration.

### How the work-on Skill Consumes Advanced Phases

The work-on skill's SKILL.md file (lines 69-82 of `/home/dgazineu/dev/workspace/tsuku/tsuku-7/public/shirabe/skills/work-on/SKILL.md`) shows the exact execution pattern:

```
Repeat:

1. Run `koto next <WF>`
2. If `action: "execute"` with `advanced: true` — run `koto next <WF>` again
3. If `action: "execute"` with `expects` — do the work described in `directive`,
   read any phase file it references, then submit evidence
```

**Line 72 is the critical workaround:** When the koto response has `action: "execute"` and `advanced: true`, the skill immediately invokes `koto next` again without inspecting state or making any decisions.

### What This Pattern Actually Represents

The double-call pattern occurs in these scenarios (based on analysis of work-on.md structure):

1. **Unconditional auto-advancing states:** States like `staleness_check` have gates that, when passing, auto-advance to the next state. After submitting evidence at such a state:
   - The agent's evidence submission advances the workflow
   - The auto-advancement engine (if present) chains forward one or more steps
   - `advanced: true` signals that *some* advancement occurred
   - But the agent doesn't know if it stopped at another state requiring action or if it went further

2. **Evidence gates that are satisfied:** When a state's gate passes (e.g., an artifact already exists), the state auto-advances without requiring evidence. The response has `advanced: true` and `action: "execute"` but no `expects` block—nothing for the agent to do, but the state changed.

### Actual Template Behavior: Deterministic States

Analyzing the work-on template's state sequence reveals that **many states are explicitly designed to auto-advance**:

- `context_injection` (lines 45-68): gate checks for artifact, unconditional fallback transition to `setup_issue_backed`
- `staleness_check` (lines 174-200): gate evaluates staleness; has unconditional fallback transition
- `setup_issue_backed` and `setup_free_form` (lines 124-172): gates check for branch/baseline; all transitions routes lead to next state with fallback

These states are **deterministic checkpoints**—they perform setup work, check preconditions, but don't require agent decision-making. The agent's role is to ensure the gate artifacts exist, then the workflow auto-advances.

### Whether Consumers Inspect Advanced Phases

**Finding: No.** The work-on skill, the only production consumer, treats `advanced: true` as an opaque signal meaning "call me again." The skill:

- Does NOT inspect the current state before calling `koto next` again
- Does NOT make decisions based on what state it might be in
- Does NOT log or report to the user about advanced phases
- Does NOT check whether the next state will require input

The double-call is purely mechanical: if `advanced: true`, invoke `koto next` without inspection.

### Test Fixtures Show No Advanced Phase Usage

The functional test fixtures (`simple-gates.md`, `multi-state.md`, `decisions.md`, `hello-koto.md`) contain no `advanced` property definitions. The `hello-koto` example demonstrates an auto-advancing state:

```yaml
awakening:
  transitions:
    - target: eternal
  gates:
    greeting_exists:
      type: command
      command: "test -f wip/spirit-greeting.txt"
```

This state auto-advances to `eternal` when the gate passes, but the template doesn't label it with `advanced: true` (that field doesn't exist in templates—it's CLI-level only).

### Template Philosophy: Explicit Checkpoints

The work-on template's design philosophy is **explicit state checkpoints**, not implicit advancement:

- States with gates are explicit checkpoints: "Check this condition. If it passes, proceed; if it fails, provide evidence."
- States with `accepts` blocks require agent input.
- States with neither are terminal or have deterministic transitions.

The `advanced: true` response is a **CLI implementation detail**, not a template-level concept. Templates don't define "advanced phases"—they define states, and the CLI layer reports when multiple states are traversed in one call.

## Implications

### For Issue #89 (Auto-advance past advanced: true phases)

The proposal to "auto-advance past advanced: true phases" misunderstands the problem:

1. **Templates don't define advanced phases.** There is no `advanced: true` property in templates. The work-on template (the only production use case) doesn't mark any state as special.

2. **The double-call isn't stopping at a meaningful checkpoint.** The work-on skill's execution loop (line 72) calls `koto next` again with no inspection. It's not pausing to let the agent decide whether to proceed. The pattern is entirely mechanical.

3. **Auto-advancement is already the default.** Unconditional transitions in templates like `staleness_check` → `analysis` already auto-advance when gates pass. The `advanced: true` response is just reporting that a transition occurred; it's not a gate stopping the loop.

4. **The real blocker is observability.** The skill calls `koto next` again because it needs to know: "Am I at a state that requires my attention, or did the workflow already advance past the checkpoint?" The `advanced: true` field doesn't answer that question clearly.

### If Auto-Advance Went Deeper

If `koto next` were changed to eliminate the double-call by auto-advancing completely until hitting a state with `expects`:

- **Behavior would not change for the work-on skill.** The skill would save one call per workflow execution (not a significant optimization).
- **Observability would decrease.** Currently, the agent can inspect the workflow state after each `koto next` call. With deeper auto-advance, the agent would see only the final state and lose visibility into the intermediate states traversed.
- **The audit trail would still record all transitions** (the engine logs each one), but the agent wouldn't see them unless it explicitly queries workflow state afterward.

### Whether Stopping at Advanced Phases Serves a Purpose

**Finding: No clear purpose for the skill consumers.** The work-on skill's double-call pattern serves **no decision-making purpose**:

1. The skill doesn't inspect the state returned by the second call before taking action.
2. It doesn't branch based on what the state is.
3. It doesn't report the state to the user for approval.
4. It just moves to the next iteration of its execution loop.

The double-call is a **mechanical consequence of ambiguous semantics**, not an intentional pause for agent deliberation.

## Surprises

1. **Templates have no `advanced` property at all.** This was the biggest surprise—the entire "advanced phase" concept doesn't exist in the template layer. It's purely a CLI response field. Issue #89's framing of the problem as "advanced: true phases" in templates is a misunderstanding of what templates are.

2. **The work-on skill is the only production consumer.** Only one skill uses koto templates in production, and it treats `advanced: true` as a signal to retry, not as information to act on.

3. **The auto-advancement engine was explicitly designed to collapse multiple transitions into one response.** From DESIGN-auto-advancement-engine.md (line 472-485), the solution was to "Total: 1 CLI call. The engine auto-advanced through `plan` and `implement`, appending `transitioned` events for each, and stopped at `verify`." But the work-on skill's loop still double-calls, suggesting the design didn't fully achieve its goal.

4. **No consumer of auto-advanced states actually inspects the state.** The work-on skill (the only known consumer) doesn't check `current_state` or any metadata before calling `koto next` again. This suggests either:
   - The feature (auto-advancement) isn't actually providing value to callers, or
   - Callers are unaware they can use the response state metadata to avoid the double-call

## Open Questions

1. **Does the work-on skill actually need the double-call?** The execution loop (line 72) shows `if advanced: true, run koto next again`, but it provides no rationale. Could the skill be simplified by removing this check and relying solely on the state's `expects` block to determine whether action is needed?

2. **Are there other consumers of koto templates not yet discovered?** Only one production template and four test fixtures were found. Are there internal-use-only templates, other plugins, or archived templates that consume `advanced` phases differently?

3. **What would auto-advancing past advanced: true actually accomplish?** If implemented, the skill would make one fewer call per workflow iteration. But would the observability loss (not seeing intermediate states) be acceptable? Should workflows that care about seeing intermediate checkpoints opt into a "verbose" mode?

4. **Is the `advanced` field obsolete?** If templates don't define advanced phases, and consumers don't use `advanced: true` as a decision point, should the field be deprecated in favor of explicitly returning `current_state` in the response so callers can make their own decisions?

## Summary

**No templates explicitly use `advanced` phases, and the only production skill consumer (work-on) treats the `advanced: true` response as a mechanical signal to retry, not as information to inspect or act on.** The double-call pattern in the work-on skill's execution loop is a workaround for ambiguous CLI semantics, not an intentional pause for agent decision-making. Auto-advancing deeper would save one koto call per workflow iteration but would reduce observability without providing new capability—the auto-advancement engine already chains through unconditional transitions. The real issue is not that auto-advancement stops too early, but that the `advanced` field's meaning is unclear, forcing callers to requery state to disambiguate where they are after a transition.

