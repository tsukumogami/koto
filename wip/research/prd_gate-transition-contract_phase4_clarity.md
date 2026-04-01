# Clarity Review

## Verdict: FAIL

The PRD is well-structured and substantially clearer than average, but contains several ambiguities that could lead two developers to build different things -- particularly around namespace serialization, gate evaluation semantics, and backward compatibility behavior.

## Ambiguities Found

1. **R3 / Examples / D1: `gates.<gate_name>.<field>` namespace serialization is unspecified.**
   The PRD uses dot-delimited paths like `gates.ci_check.status` in YAML `when` clauses. It is unclear whether this is:
   - A literal flat key string `"gates.ci_check.status"` in a map
   - A nested JSON path resolved against `{"gates": {"ci_check": {"status": "passed"}}}`
   - A dot-path syntax specific to the transition resolver that gets parsed into segments

   The transition resolver needs to know how to look this up. One developer might implement nested maps; another might implement flat keys with dot separators. The difference matters for serialization in events, state files, and the `koto query` output.
   -> **Suggested clarification:** State explicitly how gate output is represented in the evidence map (flat key vs. nested structure) and whether `gates.x.y` in `when` is a lookup syntax or a literal key.

2. **R2: "The engine populates gate output fields based on the gate type's evaluation result" -- how?**
   The PRD says gate evaluation produces structured data matching `output_schema`, but the mechanism is declared out of scope ("how command gates produce structured data from command output is a design concern"). This creates ambiguity for the contract boundary: does the engine parse command stdout as JSON? Does each gate type have a hardcoded mapping? The `output_schema` declares the shape, but nothing says how raw gate output becomes that shape.
   -> **Suggested clarification:** While full stdout-parsing design is out of scope, state the contract: each gate type implementation is responsible for returning data conforming to the schema, and the engine validates conformance at runtime (or not -- say which).

3. **R2: What constitutes a "passing" vs "failing" gate under the structured model?**
   The boolean model has clear pass/fail. The structured model replaces pass/fail with structured data, but R5 says `--override-rationale` applies to "failing gates." How does the engine determine a gate is "failing" when its output is structured (e.g., `{status: "warnings"}`)? Is there a `pass_when` condition per gate? Is any output that doesn't match a transition considered "failing"? The examples imply that gate output always feeds transitions directly, yet R5 and Example 4 talk about "blocking" and "failing" gates.
   -> **Suggested clarification:** Define explicitly what makes a gate "pass" or "fail" in the structured model. Options: (a) gates declare a `pass_when` condition, (b) any gate output that leads to at least one matching transition is "passing," or (c) gates still have a separate boolean pass/fail signal alongside structured output.

4. **R5a: "If a named gate isn't actually failing, it's ignored (no error)" -- what about nonexistent gates?**
   R5a says non-failing gates are ignored. A separate acceptance criterion says `--gate nonexistent_gate` is silently ignored. But R5a doesn't mention nonexistent gates. These are different cases (gate exists but passing vs. gate doesn't exist at all), and one developer might throw an error for nonexistent gates while another silently ignores both.
   -> **Suggested clarification:** R5a should explicitly state behavior for `--gate` naming a gate that doesn't exist in the current state's definition.

5. **R10: Backward compatibility -- what happens to transition routing for schema-less gates?**
   R10 says gates without schemas behave as today (boolean pass/fail). But the new transition resolver routes on `gates.<name>.<field>`. If a gate has no schema, what does it contribute to the evidence map? Nothing? A synthetic `{passed: true/false}`? The PRD doesn't say. A template author mixing schema-less and schema-bearing gates in `when` clauses could get different behavior depending on the implementation.
   -> **Suggested clarification:** State what (if anything) schema-less gates contribute to the transition resolver's evidence map, and whether `when` clauses can reference schema-less gates via `gates.<name>.*`.

6. **R9: "When override defaults are applied to all failing gates, at least one transition resolves" -- does this include agent evidence?**
   The compiler validation says "at least one transition resolves" when overrides are applied. But transitions can also depend on agent evidence (`accepts` fields). If a transition requires both `gates.lint.status: clean` AND `decision: approve`, the compiler can verify the gate side but not the agent side. Does the compiler assume agent evidence is satisfied, or does it only check transitions that depend solely on gate data?
   -> **Suggested clarification:** State whether compiler dead-end validation considers agent evidence fields as unconstrained (wildcard) or omits them. Either interpretation changes what "at least one transition resolves" means.

7. **R11: "Strict sequence" of events -- which comes first?**
   R11 says `EvidenceSubmitted` and `GateOverrideRecorded` are emitted in "strict sequence" but doesn't say which order. Does evidence get submitted first, then override is recorded? Or the reverse? The order could matter for consumers that process the event log.
   -> **Suggested clarification:** State the explicit ordering: evidence-then-override or override-then-evidence.

8. **R5: "--override-rationale on a non-blocked state is a no-op" (acceptance criterion) -- what about partially blocked?**
   The acceptance criteria say override on a non-blocked state is a no-op. But a state could be "not blocked by gates" while still waiting for agent evidence. The PRD doesn't clarify whether "non-blocked" means "no failing gates" or "state is fully ready to transition." An agent submitting `--override-rationale` alongside `--with-data` on a state where gates pass but evidence is missing -- is that a no-op for the override part?
   -> **Suggested clarification:** Define "non-blocked state" precisely. Likely means "no failing gates," but state it explicitly.

9. **Example 1: `koto next` without `--with-data` triggers transition -- is this a new behavior?**
   In Example 1, `koto next my-workflow` (with no flags) shows gate output driving an automatic transition. Currently `koto next` returns the directive. Is the PRD implying that `koto next` now auto-advances when gates resolve? Or does the agent still need to call `koto transition`? The example shows `koto next` returning `{"action": "done", "advanced": true}` which suggests auto-advance behavior, but this isn't called out as a new requirement.
   -> **Suggested clarification:** State whether gate-driven transitions happen automatically (engine auto-advances when gates resolve) or require an explicit `koto next` / `koto transition` call. If auto-advance, add it as a requirement.

10. **Example 2: Transition with `gates.lint.status: warnings` and `decision: approve` -- what about `gates.lint.status: errors`?**
    The example has three transitions but no transition for `{status: errors, decision: approve}` or `{status: errors, decision: request_changes}`. Would the compiler flag this as a dead end? The PRD's R9 only validates that override defaults lead to a valid transition, not that all gate output combinations are covered. This isn't strictly an ambiguity in the PRD, but it's worth noting the compiler validation scope is narrower than a reader might assume.
    -> **Suggested clarification:** Consider noting that the compiler validates override-reachability, not exhaustive gate output coverage. Or if exhaustive coverage is intended, add that requirement.

11. **Acceptance criterion: "Override events survive rewind" -- what does "survive" mean?**
    Does this mean override events are never deleted from the log, even if the workflow rewinds past the state where the override occurred? Or that the `koto overrides list` query still returns them? If the event log is append-only, this is trivially true. If rewind truncates the log, it's a real constraint.
    -> **Suggested clarification:** State whether the event log is append-only (making this trivially true) or whether rewind can truncate events (making this a real requirement).

## Summary

The PRD is thorough in its examples and acceptance criteria, and the interaction examples do heavy lifting to disambiguate intent. The most critical ambiguity is #3 -- the definition of "passing" vs "failing" in the structured gate model -- because it affects the core override mechanism (R5) and every acceptance criterion that mentions "failing gates." The namespace serialization question (#1) is the second most impactful since it affects implementation across multiple packages. Resolving these two would eliminate most downstream interpretation risk.
