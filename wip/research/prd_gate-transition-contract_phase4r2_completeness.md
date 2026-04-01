# PRD Review: Gate-Transition Contract -- Completeness (Round 2)

**Reviewer role:** Completeness
**Round:** 2 (post-fix pass)
**Verdict:** Conditional Pass
**Issues found:** 6

---

## Summary

The PRD addressed the 9 issues from round 1 effectively. Gate types now have documented schemas, override defaults are well-specified, backward compatibility is clear, and the interaction examples are strong. This round focuses on gaps that emerged from the new gate-type-as-building-block model and its interactions with existing engine behavior.

---

## Issues

### C2-1: Error/timeout structured output schema is unspecified (Severity: Medium)

The acceptance criteria (lines 432-435) require that timeouts produce `{error: "timed_out"}` and errors produce structured output that doesn't match the pass condition. But R1's schema table only documents the happy-path output schema for each gate type. There's no specification of the error output schema.

Questions the PRD doesn't answer:
- Does every gate type share a single error schema (`{error: string}`), or does each gate type define its own error variant?
- For a command gate, is a timeout `{exit_code: -1}` or `{error: "timed_out"}`? The acceptance criteria say `{error: "timed_out"}` but the current `GateResult::TimedOut` variant has no exit_code. These are different schemas -- one has `exit_code`, the other has `error`.
- Can a `when` clause route on `gates.ci_check.error`? If so, `error` is part of the output schema and should be documented in R1's table. If not, the PRD should say error outputs bypass transition routing entirely (gate stays failed, no `when` match possible).
- When overriding a timed-out gate, the `actual_output` in the `GateOverrideRecorded` event would need to use this error schema. What does it look like?

**Recommendation:** Add an error output row to R1's table, or add an R2 sub-clause specifying the error schema convention. Something like: "All gate types produce `{error: string}` on failure modes that aren't part of their normal output schema (timeout, spawn failure, invalid config). Error output never satisfies any gate type's pass condition. The `error` field is available in `when` clauses for routing on failure modes."

### C2-2: pass_condition is hardcoded in the gate type -- no template-author override (Severity: Medium)

R4 says: "The compiler validates that override defaults... satisfy the gate type's built-in pass condition." This means an override default *must* represent a passing value. But the PRD doesn't address whether this is a deliberate design constraint or an oversight.

Scenario: A template author wants a command gate with `override_default: {exit_code: 2}` -- meaning "when overridden, treat it as if the command returned exit code 2, and let the `when` clause route on that." This is impossible because the compiler would reject exit_code 2 as not satisfying the pass condition (exit_code == 0).

This matters because:
- The override default is conceptually "what value to inject into routing," not "what value means the gate passed."
- The pass condition determines whether the gate blocks. Override already bypasses blocking. Requiring the override default to also satisfy the pass condition is redundant -- the override mechanism already means "I know it failed, proceed anyway."
- A template author who wants to route overridden gates to a specific non-happy-path transition can't do it.

The PRD should either:
1. Explicitly state this constraint as a deliberate limitation in the Known Limitations section with rationale, or
2. Decouple the override default validation from the pass condition -- only require schema match, not pass condition satisfaction.

### C2-3: Transition resolver doesn't support dot-path traversal today (Severity: Medium)

R3 specifies that `when` clauses use `gates.ci_check.exit_code` with dot-path traversal into a nested JSON map. But the current `resolve_transition` implementation (advance.rs lines 410-414) does exact key lookup on the evidence BTreeMap:

```rust
let all_match = conditions
    .iter()
    .all(|(field, expected)| evidence.get(field) == Some(expected));
```

This does `evidence.get("gates.ci_check.exit_code")` -- a flat string key lookup. It doesn't traverse `evidence["gates"]["ci_check"]["exit_code"]`.

The PRD should specify which approach is intended:
- **Flat keys with dots in the name**: Gate output is injected as `{"gates.ci_check.exit_code": 0}` -- works with current resolver, but collides with the R3 statement that the namespace is "a nested JSON map."
- **Nested map with dot-path traversal**: Gate output is injected as `{"gates": {"ci_check": {"exit_code": 0}}}` -- requires a new traversal function in the resolver.

This is an implementation detail, but the PRD's R3 language about "nested JSON map" and "traverses the dot-separated path" implies nested maps, which means the transition resolver needs a breaking change. The PRD should be explicit about which approach to take and whether this is a resolver change or a flat-key injection.

### C2-4: Extensibility path for future gate types is incomplete (Severity: Low)

R1 and R2 say future gate types "extend the system by registering new types with their own schemas and parsing logic." The future gate types table lists json-command, http, and jira. But the PRD doesn't specify:

- **Registration mechanism**: Is there a gate type registry interface? A trait? A match arm? The current code uses a match on string literals in `evaluate_gates`. The PRD should say whether the extensibility path is "add a new match arm" (simple, internal) or "implement a trait and register" (pluggable, external).
- **Schema validation at compile time**: If future gate types are added by third parties (plugins), the compiler needs to discover their schemas. Is this in scope or explicitly out of scope?
- **The `data: object` type in json-command's schema**: This is an open-ended object. How does the compiler validate `when` clauses that reference `gates.check.data.coverage`? The compiler can't know the shape of `data` at compile time. This should be called out as a known limitation of the json-command type specifically.

This is low severity because it's future work, but the PRD should at least state the extensibility mechanism (even if just "new match arm in the engine") so implementers know the intended direction.

### C2-5: No AC for gate output in `koto next` JSON response (Severity: Low)

Example 1 shows `koto next` returning `{"action": "gate_blocked", "blocking_conditions": [{"gate": "ci_check", "output": {"exit_code": 1}}]}`. This structured output in the CLI response is the primary way agents discover what gates produced. But there's no acceptance criterion verifying this response shape.

The ACs cover: gate produces structured data (internal), transition routing works (internal), override events contain output (audit). None verify that `koto next` surfaces gate output in its response to the calling agent. An implementer could satisfy all current ACs while returning the old boolean-only response format.

**Recommendation:** Add an AC: "When a gate blocks, `koto next` response includes each gate's structured output in the `blocking_conditions` array."

### C2-6: Interaction example 2 has an inconsistency (Severity: Low)

Example 2's YAML shows:
```yaml
when:
  gates.lint.exit_code: 0
  decision: approve
```

But the prose below says "Gate output (`gates.lint.status`) and agent evidence (`decision`) coexist..." -- referencing `gates.lint.status` which doesn't exist in the command gate schema. The schema produces `exit_code`, not `status`. This was likely left over from an earlier draft.

---

## Items verified as resolved from Round 1

- GateResult-to-structured-output mapping: now addressed by R2's per-type parsing logic
- Backward compatibility: R10 clearly specifies legacy behavior
- Override persistence: Example 4 explicitly states overrides aren't sticky
- Override default validation: R4 + R9 cover compiler validation
- Gate-type schemas: R1's table documents all three initial types

## Verdict

**Conditional Pass.** The PRD is solid for implementation with two caveats:

1. **C2-1 (error schema)** and **C2-3 (dot-path traversal)** need resolution before implementation -- they affect the data model and resolver design respectively. An implementer would need to make assumptions the PRD should make explicit.
2. **C2-2 (pass_condition constraint on overrides)** deserves a Known Limitations entry if the constraint is deliberate.

The remaining issues (C2-4, C2-5, C2-6) are minor and can be addressed during implementation or as follow-up.
