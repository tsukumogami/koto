# Advocate: Coarse-grained checkpoint template

## Approach Description

3–4 high-level checkpoint states (Setup → Implement → PR → Done). Each state covers multiple
skill phases the agent handles internally based on the directive. koto enforces milestone
boundaries, not every phase transition. The agent has full autonomy within each checkpoint.

## Investigation

### Phase Mapping

work-on's 7 phases collapse into 4 checkpoints:
- Setup: context injection + branch creation + staleness check + introspection (if needed)
- Implement: analysis + planning + implementation + finalization
- PR: PR creation + CI monitoring
- Done: terminal state

just-do-it's 6 phases collapse similarly:
- Setup: jury validation + research + branch creation
- Implement: analysis + implementation
- PR: PR creation + CI monitoring
- Done: terminal state

### Key Finding: Staleness Pattern

The skip pattern (staleness check → route to analysis or introspection) cannot be expressed
cleanly within a coarse-grained Setup state. Options:
1. Run staleness check before koto init (outside the template) — audit trail misses the decision
2. Add a staleness sub-state to the Setup checkpoint — defeats coarse-grained simplicity
3. Always run introspection — loses the optimization the skip was designed for

The compiler's mutual exclusivity validation doesn't apply at coarse granularity: the agent
decides what happens inside Setup, and koto doesn't see it.

## Strengths

- Simple template: ~50 lines of YAML, minimal `accepts` blocks
- Agent flexibility within checkpoints: can handle iterative implementation, retries, subagent
  spawning without template-level complexity
- Low overhead: agent submits evidence only at 3-4 meaningful boundaries
- Easy to author and reason about

## Weaknesses

- Loses critical enforcement: staleness detection, introspection/analysis ordering, and the
  skip pattern are all outside koto's visibility
- koto's event log misses staleness decisions — auditing is incomplete
- The separation of just-do-it juries (needs-design / needs-breakdown / ready) happens inside
  a checkpoint, making routing invisible to koto
- If an agent skips introspection when stale, koto cannot detect or block it
- Coarse states make session resume ambiguous: "in Setup" doesn't say whether branch was
  created, baseline done, or staleness checked

## Deal-Breaker Risks

The staleness/introspection check is the design's critical enforcement point. At coarse
granularity, this check runs inside the Setup directive with no koto visibility. Agents
could skip it silently. The audit trail would show "Setup completed" without recording
whether staleness was assessed or introspection was performed. This fragments the auditability
promise and is a fundamental mismatch with the decision to enforce phase structure via koto.

## Implementation Complexity

- Files to modify: small (template only, minimal skill changes)
- New infrastructure: none
- Estimated scope: small

## Summary

The coarse-grained checkpoint approach trades strict phase enforcement for agent autonomy,
winning on template simplicity but losing critical enforcement at the transitions that
matter most — staleness detection and introspection/analysis ordering. The skip-staleness
pattern can't be expressed cleanly at coarse granularity without either fragmenting the
audit trail or defeating the simplicity advantage. Viable only if enforcement of the
staleness branch is explicitly out of scope for koto.
