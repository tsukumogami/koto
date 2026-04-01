# Decision Consistency Review: PRD-gate-transition-contract

Review of D1-D7 against the rest of the document for internal consistency.

## Verdict: PASS WITH ISSUES

**5 issues found** (0 blocking, 3 moderate, 2 minor)

---

## Issue 1 (Moderate): R10 references `output_schema` -- a term no decision or requirement defines

R10 says "Existing templates without `output_schema` continue to compile" and "The transition resolver only receives `gates.*` namespaced data when the gate declares an `output_schema`." But nowhere in the PRD -- not in D5, not in any requirement, not in the interaction examples -- is `output_schema` defined as a template-level field. D5 says gate types own schemas as documented building blocks. R1 says each gate type defines a public output schema. Both describe schemas as properties of the gate type, not something template authors declare per-gate. Yet R10 treats `output_schema` as a per-gate template declaration that controls whether `gates.*` data enters the resolver. This contradicts D5's position that schemas belong to gate types, not templates, and the "Custom gate output schemas" out-of-scope item that says template authors can't declare custom output schemas.

Likely cause: R10 was written before D5 was added. It should be reworded to condition on whether `when` clauses reference `gates.*` fields (which is what D4 already says: "gates without `when` clauses referencing `gates.*` fields behave as today").

## Issue 2 (Moderate): D6 and the "Override always routes through passing path" limitation are consistent, but the D6 future extension note creates ambiguity

D6 says overrides substitute gate output, and the known limitation says "Override defaults must satisfy the gate's pass condition, so overrides always produce the 'gate passed' routing outcome." These align. However, D6's final paragraph mentions a "future extension could allow agents to inject explicit gate output values at override time, enabling non-passing override routing without bypassing the resolver." This is fine as a forward-looking note, but it partially contradicts R4's requirement that "The compiler validates that override defaults... satisfy the gate type's built-in pass condition." If a future extension allows non-passing override values, R4's compiler validation would reject them.

This isn't a contradiction today, but the future extension note undermines the clarity of the current constraint. A reader might wonder whether to design the compiler validation to be relaxed later.

## Issue 3 (Moderate): R10 backward compatibility mechanism conflicts with the gate type model

R10 says: "Gates without schemas behave as today: boolean pass/fail, the `gate_failed` flag controls transition resolution, and no `gates.*` data enters the transition resolver."

The phrase "gates without schemas" doesn't make sense in the D5 model where every gate type inherently has a schema (command always produces `{exit_code, error}`, context-exists always produces `{exists, error}`, etc.). A `command` gate always has a schema by virtue of its type. The backward compatibility condition should be about whether the template's `when` clauses reference `gates.*` fields, not whether the gate "has a schema."

D4 gets this right: "gates without `when` clauses referencing `gates.*` fields behave as today." R10 should use D4's framing.

## Issue 4 (Minor): D3 says this PRD supersedes PRD-override-gate-rationale, and the old PRD file is deleted

The old PRD file no longer exists in `docs/prds/`. This is correct. However, `wip/design_override-gate-rationale_summary.md` still has a `Source PRD: docs/prds/PRD-override-gate-rationale.md` link, and `wip/research/prd_gate-transition-contract_phase2_current-state.md` lists it as a reference. These are wip artifacts that will be cleaned before merge, so this is not a real problem -- just noting it for completeness.

## Issue 5 (Minor): Out-of-scope "Custom gate output schemas" phrasing could confuse after D5

The out-of-scope item says: "Template authors can't declare custom output schemas for existing gate types." After D5, this makes sense -- schemas are owned by gate types. But the phrasing "custom output schemas" could be confused with `output_schema` in R10 (Issue 1 above). If R10 is fixed to remove the `output_schema` per-gate concept, this out-of-scope item reads cleanly.

---

## Items checked and found consistent

- **D1 (namespaced gate output) vs R3, R7**: R3 defines the `gates.<name>.<field>` namespace. R7 reserves the `gates` top-level key. Both align with D1.
- **D2 (per-gate override defaults) vs R4**: R4 says "each gate in the template declares an `override_default`." Aligns with D2's per-gate choice.
- **D5 (gate types own schemas) vs R1**: R1 says "Each gate type defines a public output schema and a pass condition" and "Each gate type owns both the schema and the parsing logic." Perfect alignment with D5.
- **D6 (override substitutes gate output) vs R5**: R5 describes the override mechanism. The override applies `override_default` values and runs through normal transition resolution. Consistent with D6.
- **D7 (override is both command and flag) vs R5**: R5 explicitly describes both paths: `koto override` (command/primitive) and `koto next --override-rationale` (flag/shorthand). Direct match.
- **D6 vs known limitation "override always routes through passing path"**: Direct alignment. No requirement or AC allows non-passing override routing.
- **Problem statement and goals vs requirements after revisions**: The problem statement's three problems (manual coupling, no audit trail, no rich outcomes) are all addressed. Goals match R1-R9. The goals mention "per-gate output schemas" which aligns with D5's gate-type-owned schemas (the gate type defines the schema, the gate instance inherits it).
- **No contradictions between decisions**: D1-D7 are internally consistent. No decision undermines another.
- **Out-of-scope items after D5-D7**: All out-of-scope items remain genuinely out of scope. "Dynamic override values" is correctly deferred (D6 notes it as future). "Custom gate output schemas" aligns with D5. No item has been pulled into scope by the decisions.
- **Known limitations vs decisions**: All limitations align with their corresponding decisions. The "dot-path traversal" limitation correctly identifies new work needed for R3. The "compiler reachability" limitation correctly scopes R9.
