# Phase 2 Research: Current-State Analyst

## Lead 6: Caller decision tree

### Current State

Today, callers have three sources of guidance for handling `koto next` responses:

**Source 1: AGENTS.md (primary, agent-consumed)**

AGENTS.md documents five response shapes with action guidance:

| Shape | How AGENTS.md tells callers to handle it |
|-------|----------------------------------------|
| `action: "execute"` + `expects` present | Execute directive, submit evidence matching `expects.fields` via `--with-data` |
| `action: "execute"` + `blocking_conditions` present | Fix blocking conditions, call `koto next` again (don't submit evidence) |
| `action: "execute"` + no expects + no blocking | "The state auto-advances" -- one sentence, no explicit caller action |
| `action: "done"` | Workflow finished, stop |
| `action: "confirm"` | Review `action_output`, submit evidence if state accepts it |

AGENTS.md also documents error handling via exit codes (0/1/2/3) with a table mapping exit codes to agent actions (process/retry/change approach/report).

The implicit caller loop from AGENTS.md's walkthrough examples:

```
1. koto next <name>
2. If action == "done" -> stop
3. If blocking_conditions present -> fix conditions, goto 1
4. If expects present -> do work, submit evidence via --with-data, goto 1
5. If action == "confirm" -> review action_output, submit evidence, goto 1
6. Otherwise -> call koto next again (auto-advance case)
```

**Source 2: cli-usage.md (secondary, human-consumed)**

cli-usage.md documents six response variants (splitting Integration into Integration/IntegrationUnavailable) with a field-presence matrix and dispatcher classification order. It provides a "typical agent workflow" bash loop that checks only two conditions:

```bash
if [ "$action" = "done" ]; then break; fi
if [ -n "$expects" ]; then submit evidence; fi
```

This loop doesn't handle:
- `action: "confirm"` (not mentioned at all in cli-usage.md)
- `blocking_conditions` (no gate-blocked handling)
- `integration` field (no integration handling)
- Error responses (no exit code checking)

**Source 3: SKILL.md files (tertiary, per-workflow)**

The hello-koto SKILL.md shows a hardcoded interaction pattern: init, next, execute, transition, next (check done). It uses `koto transition eternal` -- a command that doesn't exist anymore (replaced by `koto next --to eternal`). It doesn't demonstrate a general-purpose loop.

The Cursor rules file (koto.mdc) also uses the stale `koto transition` command.

**What callers actually encounter today (the 14 outcomes):**

Based on the engine's `StopReason` and `AdvanceError` variants, callers can see these 14 distinct outcomes:

| # | Outcome | action | advanced | expects | Other notable fields | Exit code |
|---|---------|--------|----------|---------|---------------------|-----------|
| 1 | EvidenceRequired (fresh state) | `"execute"` | `false` | object | -- | 0 |
| 2 | EvidenceRequired (after auto-advance) | `"execute"` | `true` | object | -- | 0 |
| 3 | GateBlocked | `"execute"` | `false` | `null` | `blocking_conditions` | 0 |
| 4 | GateBlocked (after partial auto-advance) | `"execute"` | `true` | `null` | `blocking_conditions` | 0 |
| 5 | Integration (available) | `"execute"` | varies | object or `null` | `integration: {name, output}` | 0 |
| 6 | IntegrationUnavailable | `"execute"` | varies | object or `null` | `integration: {name, available: false}` | 0 |
| 7 | Terminal (just arrived) | `"done"` | `true` | `null` | -- | 0 |
| 8 | Terminal (already done) | `"done"` | `false` | `null` | -- | 0 |
| 9 | ActionRequiresConfirmation | `"confirm"` | `false` | object or `null` | `action_output: {command, exit_code, stdout, stderr}` | 0 |
| 10 | CycleDetected | -- | -- | -- | `error.code: "precondition_failed"` | 2 |
| 11 | ChainLimitReached | -- | -- | -- | `error.code: "precondition_failed"` | 2 |
| 12 | UnresolvableTransition | -- | -- | -- | `error.code: "precondition_failed"` | 2 |
| 13 | SignalReceived | degrades to #1-#8 | varies | varies | indistinguishable from normal | 0 |
| 14 | AdvanceError (Ambiguous/DeadEnd/Unknown/Persistence) | -- | -- | -- | `error.code: "precondition_failed"` | 2 |

Plus standard errors (invalid_submission, terminal_state, etc.) from bad caller input.

### Gaps and Confusion Points

**Gap 1: `advanced: true` + `expects` present is never shown.**
AGENTS.md shows `advanced: false` in all execute examples and `advanced: true` only in the done example. Outcome #2 -- where the engine auto-advances through states and lands on one requiring evidence -- produces `advanced: true` with `expects` populated. No documentation shows this combination. Callers who interpret `advanced: true` as "this phase is handled" would skip submitting evidence.

**Gap 2: `advanced: false` on Terminal is never shown.**
Outcome #8 -- calling `koto next` on an already-terminal workflow -- returns `action: "done"` with `advanced: false`. AGENTS.md only shows Terminal with `advanced: true`. Callers checking `advanced` to determine if they caused completion would misinterpret repeated done queries.

**Gap 3: `action: "confirm"` is absent from cli-usage.md.**
The typical agent workflow loop in cli-usage.md doesn't handle `action: "confirm"`. Any caller following that guide as a template would hit an unknown action value.

**Gap 4: No caller guidance for Integration/IntegrationUnavailable.**
AGENTS.md doesn't document Integration or IntegrationUnavailable at all. cli-usage.md shows the JSON shape but doesn't say what callers should do.

**Gap 5: Auto-advance with no expects/blocking has no explicit caller action.**
AGENTS.md says "the state auto-advances" and "you'll see `advanced: true`" but doesn't say "call `koto next` again to continue." Callers hitting a state where auto-advancement is expected but gates block (outcome #3/#4 with `advanced: true`) have no guidance.

**Gap 6: Error responses from template-structural problems are undifferentiated.**
Outcomes #10-#12 and #14 all produce `precondition_failed` with exit code 2. Callers can't programmatically distinguish "the template has a cycle" from "I passed bad flags" from "disk is full." AGENTS.md says exit code 2 means "change your approach" -- but the correct response to CycleDetected (template bug) is fundamentally different from the correct response to invalid_submission (fix evidence).

**Gap 7: SignalReceived is invisible.**
Outcome #13 degrades silently into one of the normal outcomes. The caller has no way to know the advancement chain was interrupted. This is by design but never documented.

**Gap 8: Gate-with-evidence fallback is completely undocumented.**
When gates fail on a state with an `accepts` block, the engine returns EvidenceRequired (not GateBlocked). The caller sees `expects` populated and no `blocking_conditions`. They can submit evidence to override the gate failure. No document mentions this pattern.

**Gap 9: `--to` doesn't chain auto-advancement.**
A directed transition lands on the target state and returns its shape, but doesn't trigger the advancement loop. If the target is a passthrough state, the caller must call `koto next` again. This differs from bare `koto next` and `--with-data`, which both run the advancement loop.

**Gap 10: SKILL.md and koto.mdc reference nonexistent commands.**
Both use `koto transition` which was replaced by `koto next --to`. Any agent following these instructions fails immediately.

### Proposed Decision Tree

The proposed tree is organized by what the caller should inspect in priority order: exit code first, then `action`, then presence of specific fields. This mirrors the engine's dispatcher classification order.

```
koto next <name> [--with-data JSON] [--to TARGET]
  |
  +-- Exit code != 0?
  |     |
  |     +-- Exit code 1 (transient): retry after fixing
  |     |     Inspect error.code:
  |     |       "gate_blocked" -> fix blocking conditions, retry koto next
  |     |       "integration_unavailable" -> integration runner missing, retry or skip
  |     |
  |     +-- Exit code 2 (caller error): change approach
  |     |     Inspect error.code:
  |     |       "invalid_submission" -> fix evidence JSON per error.details, resubmit
  |     |       "precondition_failed" -> inspect error.message:
  |     |         contains "cycle detected" -> template bug, report to user
  |     |         contains "chain limit" -> template bug, report to user
  |     |         contains "ambiguous transition" -> template bug, report to user
  |     |         contains "dead end" -> template bug, report to user
  |     |         contains "already running" -> concurrent access, wait and retry
  |     |         contains "no accepts block" -> template bug, report to user
  |     |         contains "mutually exclusive" -> fix flags (don't combine --with-data and --to)
  |     |         other -> report to user
  |     |       "terminal_state" -> workflow already done, stop
  |     |
  |     +-- Exit code 3 (infrastructure): report to user
  |           Inspect error.code for details, but caller can't fix these
  |
  +-- Exit code 0 (success):
        |
        +-- action == "done"?
        |     YES -> Workflow complete. Stop.
        |     (advanced: true means you just caused the terminal transition;
        |      advanced: false means it was already terminal. Either way, stop.)
        |
        +-- action == "confirm"?
        |     YES -> A default action ran and needs review.
        |     1. Read action_output.stdout and action_output.stderr
        |     2. Read directive for context on what was run
        |     3. If expects is present:
        |        Submit evidence via --with-data to confirm/reject the action result
        |     4. If expects is null:
        |        Call koto next again (engine will re-evaluate)
        |
        +-- action == "execute"?
              |
              +-- integration field present?
              |     |
              |     +-- integration.available == true?
              |     |     Integration ran. Check integration.output for results.
              |     |     If expects present: submit evidence acknowledging output
              |     |     If expects null: call koto next again
              |     |
              |     +-- integration.available == false?
              |           Integration runner missing. Options:
              |           a) Install/configure the runner, call koto next again
              |           b) If expects present: submit evidence to proceed manually
              |           c) If expects null: call koto next again (will keep failing)
              |           d) Use --to to skip to a different state
              |
              +-- blocking_conditions present?
              |     Gates are blocking. Read each condition's name and status.
              |     Fix the blocking conditions (create files, switch branches, etc.)
              |     Do NOT submit evidence -- call koto next again.
              |     The engine re-evaluates gates automatically.
              |     (advanced: true here means the engine auto-advanced through some
              |      states before hitting this block. The state field tells you where
              |      you are now.)
              |
              +-- expects present (object, not null)?
              |     The state needs evidence.
              |     1. Read directive -- it tells you what to do
              |     2. Do the work described in the directive
              |     3. Construct evidence JSON matching expects.fields:
              |        - Check each field's type, required flag, and values (for enums)
              |        - Check expects.options to understand which evidence routes where
              |     4. Submit via: koto next <name> --with-data '<evidence JSON>'
              |     (advanced: true here means the engine auto-advanced through earlier
              |      states to reach this one. You still must submit evidence.
              |      Do NOT interpret advanced: true as "this work is already done.")
              |
              +-- expects null, no blocking_conditions, no integration?
                    Auto-advance candidate. The engine should have advanced
                    through this state already. If you see this:
                    - Call koto next again (the engine will attempt to advance)
                    - This typically means you're at a passthrough state after
                      a --to directed transition (which doesn't run the advancement loop)
```

**Per-outcome mapping to the tree:**

| # | Outcome | Tree path | Caller action |
|---|---------|-----------|---------------|
| 1 | EvidenceRequired (fresh) | execute -> expects present | Do work, submit evidence |
| 2 | EvidenceRequired (after auto-advance) | execute -> expects present | Do work, submit evidence (ignore advanced: true) |
| 3 | GateBlocked (fresh) | execute -> blocking_conditions | Fix conditions, call koto next again |
| 4 | GateBlocked (after partial advance) | execute -> blocking_conditions | Fix conditions, call koto next again |
| 5 | Integration (available) | execute -> integration -> available | Process output, submit evidence or call next |
| 6 | IntegrationUnavailable | execute -> integration -> unavailable | Install runner or use --to to skip |
| 7 | Terminal (just arrived) | done | Stop |
| 8 | Terminal (already done) | done | Stop |
| 9 | ActionRequiresConfirmation | confirm | Review action_output, submit evidence |
| 10 | CycleDetected | exit 2 -> precondition_failed -> cycle | Report template bug to user |
| 11 | ChainLimitReached | exit 2 -> precondition_failed -> chain limit | Report template bug to user |
| 12 | UnresolvableTransition | exit 2 -> precondition_failed -> no accepts | Report template bug to user |
| 13 | SignalReceived | transparent -- handled as whatever shape it degraded to | Follow normal tree for the received shape |
| 14 | AdvanceError | exit 2 -> precondition_failed -> various | Report to user (except concurrent access: retry) |

**Key fields callers should inspect per action value:**

| action | Always inspect | Conditionally inspect | Ignore |
|--------|---------------|----------------------|--------|
| `"execute"` | `state`, `directive`, `expects` | `blocking_conditions` (if present), `integration` (if present) | `advanced` (informational only) |
| `"done"` | `state` | -- | `advanced`, `expects` (always null) |
| `"confirm"` | `state`, `directive`, `action_output`, `expects` | -- | `advanced` (always false) |

**When to call `koto next` again vs. submit evidence vs. stop:**

| Condition | Action |
|-----------|--------|
| `action: "done"` | Stop |
| `expects` is a non-null object | Submit evidence via `--with-data` |
| `blocking_conditions` present | Fix conditions, call `koto next` again |
| `action: "confirm"` with `expects` | Review output, submit evidence via `--with-data` |
| `action: "confirm"` without `expects` | Review output, call `koto next` again |
| `action: "execute"`, no expects, no blocking, no integration | Call `koto next` again (passthrough state) |
| Error with exit code 1 | Fix the transient issue, call `koto next` again |
| Error with exit code 2 (invalid_submission) | Fix evidence, resubmit via `--with-data` |
| Error with exit code 2 (template bugs) | Stop, report to user |
| Error with exit code 3 | Stop, report to user |

### Open Questions

1. **Should `advanced` be removed from the decision tree entirely?** The prior research (lead on AGENTS.md contract) found that the post-implementation note on DESIGN-unified-koto-next.md says "the response variant is the authoritative signal, not `advanced`." The proposed tree treats `advanced` as informational only. Should the PRD formally deprecate it as a decision-making input?

2. **What should callers do with the gate-with-evidence fallback?** When gates fail on a state with an `accepts` block, callers see EvidenceRequired (outcome #1 or #2) with no indication that gates failed. Should the PRD introduce a way to surface this (e.g., include both `expects` and `blocking_conditions` in the response), or is the current "submit evidence to override" behavior the intended contract?

3. **Should `--to` trigger auto-advancement?** The current behavior requires an extra `koto next` call after a directed transition to a passthrough state. The PRD should specify whether this is intentional or a gap.

4. **How should callers distinguish template bugs from caller errors under `precondition_failed`?** The proposed tree relies on parsing error.message strings, which is fragile. Should the PRD introduce distinct error codes (e.g., `cycle_detected`, `chain_limit_reached`, `template_error`)?

5. **Should Integration/IntegrationUnavailable be documented in AGENTS.md?** Currently only cli-usage.md covers these shapes. If templates using integrations are expected to be common, agents need guidance.

6. **What is the intended caller behavior for concurrent access ("already running")?** The proposed tree says "wait and retry" but there's no documented backoff strategy or timeout.

## Summary

Callers today rely primarily on AGENTS.md, which covers 5 of 14 possible outcomes and provides reasonable but incomplete action guidance -- it never shows `advanced: true` with `expects` present, omits Integration/IntegrationUnavailable entirely, and its one-sentence treatment of auto-advancement leaves callers guessing what to do next. The proposed decision tree covers all 14 outcomes by dispatching on exit code, then `action`, then field presence (expects, blocking_conditions, integration), and deliberately sidelines the `advanced` field as informational rather than decisional -- aligning with the post-implementation finding that the response variant, not `advanced`, is the authoritative signal for caller behavior.
