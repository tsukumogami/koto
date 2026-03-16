# Advocate: Unified Model

## Approach Description

Remove field gates (field_not_empty, field_equals) entirely from the v2 format.
Everything they express can be expressed through accepts/when. A field_not_empty
gate becomes a required field in the accepts schema. A field_equals gate becomes a
when condition. Only command gates survive in v2.

koto has no users, so removing field gates is a clean break.

## Investigation

Read src/template/types.rs. The Gate struct has three types. field_not_empty checks
if a field exists in an evidence map. field_equals checks if a field equals a value.
Both operate on the same evidence data that accepts/when operates on.

command gates are fundamentally different: they run shell commands and check exit
codes. They check the environment, not agent-submitted data.

The overlap is clear: field_not_empty and field_equals are just less expressive
versions of accepts + when conditions.

## Strengths

- **Simplest mental model**: Two orthogonal concepts. Command gates check
  environment. Accepts/when handles agent evidence. No overlap.
- **No Case 4 ambiguity**: Field gates don't exist, so the question of how they
  interact with accepts never arises.
- **Less code**: Remove field gate types, their validation, and their tests.
  Net deletion of code.
- **Clean break**: No users to migrate. The types simply don't exist in v2.
- **Compile-time safety improves**: Everything about evidence is in one place
  (accepts/when). No split between gates and accepts for the same data.

## Weaknesses

- **Expressiveness gap for multi-transition gate-only states**: If a v1 template has
  a gate-only state with multiple transitions, removing field gates forces those into
  accepts/when even when evidence doesn't functionally drive routing.
- **Less concise for simple cases**: field_not_empty: decision is shorter than
  accepts: {decision: {type: string, required: true}}.
- **Validation distinction blurs**: v1 field gates are "koto checks this"; accepts
  is "agent submits this." Removing field gates means everything about evidence
  validation goes through accepts, losing the "koto verifies" semantic.

## Deal-Breaker Risks

- **Multi-transition gate-only states**: If real templates need gate-only states with
  multiple outgoing transitions, they become awkward without field gates. But this is
  unlikely in practice: multi-transition routing is exactly what when conditions are
  designed for.
- **No real deal-breakers identified**: The approach is sound for koto's current needs.

## Implementation Complexity

- Files to modify: 3 (types.rs, compile.rs, cli/mod.rs)
- New infrastructure: No
- Estimated scope: Small (net code reduction from removing field gate types)
- Remove ~10 lines from types.rs (field gate constants, validation)
- Remove ~40 lines from compile.rs (field gate compilation)
- Simplify ~20 test cases

## Summary

The Unified Model removes field gates entirely, leaving only command gates and
accepts/when. This is the simplest approach with the cleanest mental model, but
sacrifices some conciseness for simple field-check patterns. No deal-breaker risks
since accepts/when covers every use case field gates handled. The net result is
less code and fewer concepts for template authors to learn.
