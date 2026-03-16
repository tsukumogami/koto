# Advocate: Coexistence with Precedence

## Approach Description

Field gates and accepts blocks both allowed on the same state. Gates evaluate first
as prerequisites; when conditions evaluate after all gates pass. No compiler
restriction on combining them.

## Strengths

- Maximum expressiveness: environmental preconditions + evidence routing
- Preserves v1 patterns unchanged
- Follows strategic design data flow literally
- No arbitrary restrictions for template authors

## Weaknesses

- Complex mental model: two evaluation phases
- Semantic ambiguity: field required by gate but optional in accepts, or vice versa
- Unclear expects output: agent sees schema it can't submit to while gates block
- Validation complexity: must detect gate/accepts contradictions
- Contradicts explore research recommendation (Option C)

## Deal-Breaker Risks

- Evidence submission UX: agent sees expects but can't submit while gates block
- Timing confusion between gate-blocked and evidence-waiting states

## Implementation Complexity

- Files to modify: 3 (types.rs, compile.rs, cli/mod.rs)
- Estimated scope: Medium (more validation logic)

## Summary

Coexistence is practical but introduces a complex mental model and semantic
ambiguities. The main risk is degraded self-describing output: agents see an
expects field they can't submit to while gates block.
