# Advocate: Auto-advancing with minimal evidence gates

## Approach Description

~7–8 states, most of which auto-advance through koto's advancement loop. Evidence gates
are placed only at genuine decision points: the staleness assessment, the scope/plan
review, and CI/approval. Between decision points, calling `koto next` triggers
auto-advancement through intermediate states, stopping at the next evidence-required
condition. The agent doesn't call koto between auto-advancing states — one call advances
through them all.

## Investigation

### Auto-advancement mechanics

koto's auto-advancement engine (DESIGN-auto-advancement-engine.md) runs a loop when
`koto next` is called:
1. Check if current state is terminal — stop if so
2. Check gates — stop if any gate is blocked
3. Check `accepts` block — stop if evidence is required (no matching `when` condition yet)
4. If no `accepts` block and gates pass — append `transitioned` event and loop
5. Continue until a stopping condition

This means states without `accepts` blocks auto-advance as long as their gates pass. A
template can have 5 consecutive auto-advancing states; the agent calls `koto next` once
and advances through all of them, stopping at the first evidence-gated state.

### State List (Auto-advancing approach)

Approximately 8 states, with 3 evidence gates:

1. `initialized` — auto-advance (no gates, no accepts)
2. `context_loaded` — auto-advance (directive: load issue context or task description)
3. `setup` — auto-advance (directive: create branch, establish baseline)
4. `staleness_check` — **evidence gate** (accepts: staleness_signal enum, routes to
   `analysis` or `introspection`)
5. `introspection` — auto-advance (directive: run introspection; only reached if stale)
6. `analysis` — **evidence gate** (accepts: plan_approved boolean; directive: research
   and create plan, submit when approved)
7. `implementation` — auto-advance (directive: implement the plan)
8. `finalization` — auto-advance (directive: cleanup, summary)
9. `pr_created` — **evidence gate** (accepts: ci_status enum; directive: create PR,
   monitor CI, submit when passing)
10. `done` — terminal

### The staleness skip

When the agent submits `staleness_signal: fresh`, the `when` condition routes directly
to `analysis`, bypassing `introspection`. When stale, it routes to `introspection`.
Auto-advancement then carries through `introspection` automatically, stopping at
`analysis` for plan approval. The skip is clean.

### What auto-advancement means for enforcement

Auto-advancing states still have directives. The agent reads the directive, does the
work, then calls `koto next` to advance. koto doesn't verify that the work was done —
it trusts the agent. But koto does enforce that the agent can't reach `analysis` without
passing through `staleness_check` (which requires evidence). The evidence gate is the
enforcement point; auto-advancing states between gates are trusted execution.

### Jury routing

Just-do-it's jury phases (initial validation, scope assessment) can be modeled as
auto-advancing states. The directive instructs the agent to run the jury, assess the
result, and proceed. If the jury says "needs design," the agent can call `koto next
--to done` (directed transition) to terminate the workflow early. This is a legitimate
use of directed transitions for exception handling.

## Strengths

- Right level of enforcement: evidence gates where routing branches, auto-advance elsewhere
- Minimal evidence overhead: 3 `--with-data` calls vs. 9 in fine-grained approach
- Template is ~150 lines vs. 500+ for fine-grained
- Auto-advancement handles trivial phases without agent ceremony
- Staleness skip is clean and compiler-validated
- Resume is unambiguous: event log records every state transition, including auto-advances

## Weaknesses

- Auto-advancing states produce no evidence — if the agent fails to do the work in an
  auto-advancing phase, koto can't detect it (though the agent must still call `koto next`
  to advance, so it can't silently skip)
- No structured evidence for non-decision steps: whether the agent created a good baseline
  or a poor one is not captured in the event log
- The distinction between "auto-advancing" and "evidence-gated" must be designed correctly
  at authoring time; misclassifying a state is harder to catch than in fine-grained

## Deal-Breaker Risks

None identified. The core risk (agent skipping auto-advancing phases) is bounded: the agent
can't advance past an evidence gate without actually submitting valid evidence. The only
way to skip is to call `koto next --to <target>` directly, which is a deliberate directed
transition and leaves an explicit `directed_transition` event in the log. The risk is
contained and auditable.

## Implementation Complexity

- Files to modify: template (new), skill integration code (moderate changes)
- New infrastructure: none
- Estimated scope: small-medium (~1 week)

## Summary

Auto-advancing with minimal evidence gates is the right level of abstraction: enforce
where routing branches (staleness, plan approval, CI status), trust the agent for
execution phases. Auto-advancement handles trivial phases without ceremony, the staleness
skip is clean via `when` conditions, and the template is roughly one-third the size of
the fine-grained approach. The primary risk — agents not doing work in auto-advancing
phases — is bounded by the fact that evidence gates block all progress until the agent
actively submits valid data.
