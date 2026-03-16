# Advocate: Strict Separation

## Approach Description

Field gates (field_not_empty, field_equals) are forbidden on states that have an
accepts block. Compiler emits an error if both are present. Two clean control models:
gate-gated states (auto-advance when gates pass) and evidence-routed states
(accepts/when). Command gates remain allowed on any state.

## Strengths

- Eliminates Case 4 ambiguity entirely: no evaluation order question
- Forces explicit intent: one model per state, no confusion
- Self-describing output: expects field contains full evidence schema, no gate noise
- Simple compiler validation: one check after parsing
- Command gates still work for environmental checks

## Weaknesses

- Restriction may feel arbitrary without documentation
- Slight verbosity: field_not_empty was concise, accepts schema is more verbose
- Documentation burden to explain the why

## Deal-Breaker Risks

None identified. Command gates and accepts schemas cover all real use cases.

## Implementation Complexity

- Files to modify: 3 (types.rs, compile.rs, cli/mod.rs)
- Estimated scope: Small (~150 lines new/changed)

## Summary

Strict Separation eliminates the field-gate/accepts ambiguity by forbidding their
coexistence, creating two clean control models with clear compiler validation. The
cost is removing field gates from evidence-routed states. No deal-breaker risks.
