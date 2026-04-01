# Testability Review

## Verdict: PASS

The acceptance criteria are specific enough to drive a test plan with minor gaps.

## Untestable Criteria

1. **"Passing gates produce their structured output and transition routing works without agent interaction"**: The phrase "without agent interaction" is ambiguous -- does it mean no `koto next` call at all (fully automatic), or that the agent calls `koto next` but doesn't submit evidence? The interaction examples show `koto next` is still called. -> Clarify whether "without agent interaction" means the engine auto-advances after gate evaluation, or the agent still calls `koto next` but doesn't need `--with-data`.

2. **"Override events survive rewind and are visible in `koto overrides list`"**: "Survive rewind" is underspecified. Does rewind to a state before the override keep the event? Does rewind to the overridden state itself keep it? Does re-evaluation after rewind clear the override? -> Specify: "After `koto rewind` to a state before the override, `koto overrides list` still returns the override event."

## Missing Test Coverage

1. **R2: How gate types produce structured data.** No AC tests what happens when a gate command returns output that doesn't match the declared `output_schema`. Is it a runtime error? Does the gate fail? The PRD says "out of scope" for stdout parsing, but the engine still needs to validate output against schema at runtime. At minimum, an AC should cover schema-mismatch at evaluation time.

2. **R4: Override default type validation at runtime.** The compiler validates override defaults match the schema (AC exists). But there's no AC for what happens if the override default contains extra fields not in the schema, or is missing fields from the schema. The compiler ACs cover "doesn't match" but the boundary conditions are unclear.

3. **R5: `--override-rationale` without any gates failing.** AC says "on a non-blocked state is a no-op" -- but what about a state that has gates but all passed? Is that the same as "non-blocked"? And what about a state with no gates at all? These are distinct scenarios that the single AC conflates.

4. **R5a: Overriding a gate that already passed.** The AC says `--gate nonexistent_gate` is silently ignored. But what about `--gate already_passing_gate`? R5a says "if a named gate isn't actually failing, it's ignored" but there's no explicit AC for this case. A tester might miss it.

5. **R7: Namespace collision prevention.** No AC verifies that `gates.*` namespacing actually prevents collisions. A test should confirm that an `accepts` block with a field called `gates.ci_check.status` (or just `status`) doesn't collide with gate output.

6. **R9: Compiler validation -- missing granular ACs for each rule.** The compiler has four validation rules but only three are covered by explicit ACs:
   - Gate has `output_schema` but no `override_default` -- covered
   - Override default doesn't match schema -- covered
   - All override defaults lead to no valid transition -- covered
   - `when` clause references nonexistent gate/field -- covered (warns)

   However, there's no AC for: a `when` clause references a valid gate name but an invalid field name (vs. a completely nonexistent gate name). These may warrant separate ACs since the compiler behavior could differ. Also missing: what happens when `when` clauses reference gate output from a gate that has no `output_schema` (backward-compat gate)?

7. **R10: Backward compatibility edge cases.** The AC says "compile and run without changes." But what about a template that mixes gates-with-schemas and gates-without-schemas on the same state? What does the transition resolver do when one gate has structured output and another doesn't? The backward-compat AC is too coarse.

8. **R11: Event ordering.** No AC covers the event ordering requirement. There's no criterion verifying that `EvidenceSubmitted` appears before `GateOverrideRecorded` when both `--with-data` and `--override-rationale` are used in the same call.

9. **R12: Rationale size limit.** No AC covers the 1MB size limit on `--override-rationale`. Empty string is covered, but exceeding the size limit is not.

10. **R5a: Multiple selective overrides across separate calls.** The examples show overriding gate A, then gate B in a second call. There's no AC confirming that the override from the first call persists when the second call is made -- i.e., the agent doesn't need to re-override gate A.

## Summary

The acceptance criteria cover the core functional paths well -- gate schemas, transition routing, selective overrides, compiler validation, and backward compatibility all have testable ACs. The main gaps are in edge cases: event ordering (R11) and rationale size limits (R12) have zero ACs, backward compatibility with mixed schema/no-schema gates needs sharper criteria, and the "override a gate that already passed" scenario from R5a isn't explicitly tested despite being defined in the requirements. The compiler validation ACs (R9) are adequate but could be more granular about field-level vs. gate-level reference errors.
