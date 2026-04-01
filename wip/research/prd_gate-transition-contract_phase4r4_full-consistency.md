# Full Consistency Review: PRD-gate-transition-contract

Review date: 2026-03-30
Reviewer role: Fresh-reader consistency check

## Summary

**Verdict: CONDITIONAL PASS -- 14 issues found (3 high, 5 medium, 6 low)**

The PRD is internally consistent on the core model: gates produce structured data, overrides substitute gate output, the resolver routes on merged evidence. The 6+ revision rounds cleaned up most contradictions. The remaining issues are concentrated in (1) stale pass/fail language in the problem statement and user stories that doesn't match the current "gates always return data" model, (2) Example 4 introducing semantics (`contains`) not grounded in any requirement, and (3) minor cross-reference and coverage gaps.

---

## Issue 1 (HIGH): Stale language -- front-matter `problem` field

**Location:** Lines 3-8 (YAML front-matter)

> Gates in koto are boolean pass/fail checks completely decoupled from transition routing. Gate results don't produce structured data...

This describes the *current* state (pre-PRD), which is correct for a problem statement. However, line 4 says "Gates in koto are boolean pass/fail checks" as a present-tense assertion. This is fine as problem framing, but it should be verified against the same language in the body.

**Verdict:** Acceptable -- this is problem framing, not requirements. No change needed.

---

## Issue 2 (HIGH): Stale language -- "gate failure" / "failed gate" in user stories

**Location:** Lines 89, 95

> "I want to override a **failed** gate via `koto overrides record`" (line 89)

> "the **gate failure** context" (line 95)

The requirements (R1, line 328-330) explicitly say: "Gates always run and always return data -- there's no separate 'failure' state." The pass condition determines auto-advance vs. stopping for agent input. The user stories still use "failed gate" language from the earlier boolean model.

**Recommended fix:** Replace "override a failed gate" with "override a gate that didn't pass" or "override a gate whose output doesn't satisfy its pass condition." Replace "gate failure context" with "the gate's actual output."

---

## Issue 3 (HIGH): Example 4 introduces `contains` semantics not in any requirement

**Location:** Lines 206-235

Example 4 uses `gates.labels.contains: bug` in a `when` clause. The gate type `list_github_issue_labels` produces `{labels: [string]}`. The `contains` field appears in the `when` clause but is not a field in the output schema -- it's an operator applied to an array field.

- R3 says `when` clauses use "dot-path keys to reference nested values" -- `contains` is not a nested value, it's a filtering operation.
- R9 says the compiler validates that `when` clauses "reference valid gate names and fields from the gate type's schema" -- `contains` is not a field in the schema.
- The `--with-data '{"contains": "bug"}'` in the override (line 229) also uses `contains` as a field name in the substituted data, which doesn't match the gate's output schema of `{labels: [string]}`.

This example introduces array-matching semantics that no requirement defines. The `contains` syntax would need its own requirement or the example needs to use a different approach (e.g., a gate type that returns `{has_bug: boolean, has_feature: boolean}`).

**Recommended fix:** Either (a) add a requirement for array operators in `when` clauses, (b) change the example to use a gate type with a flat schema, or (c) explicitly note this example shows a future capability beyond the current requirements.

---

## Issue 4 (MEDIUM): Example 1 JSON response uses `gate_blocked`

**Location:** Line 135

> `{"action": "gate_blocked", ...}`

The term `gate_blocked` appears in the `koto next` JSON response. This is consistent with the AC at line 548-550 which says `koto next` response for a "gate-blocked state" includes structured output. However, the requirements never formally define the `gate_blocked` action value or its response schema. The AC references it but no requirement specifies what `koto next` returns when gates don't pass.

This isn't stale language per se -- `gate_blocked` is a reasonable action name since the gate is blocking advancement. But it could be confused with the old "gates block/unblock" model. The PRD should either formally define this response format in a requirement or note it as inherited behavior.

**Recommended fix:** Consider adding language to R3 or a new requirement specifying the `koto next` response format when gates don't pass, including the `blocking_conditions` structure shown in examples.

---

## Issue 5 (MEDIUM): Example 4 override_default is referenced but not declared

**Location:** Lines 236-239

> "Without `--with-data`, the override would substitute `override_default`, which might not match any transition."

The gate definition in Example 4 (lines 208-212) doesn't show an `override_default` declaration. R4 says template authors "can declare a custom `override_default` per gate" and gate types provide a default one. But the `list_github_issue_labels` gate type is a future type not listed in R1's initial or future tables, so its default `override_default` is undefined.

**Recommended fix:** Either add `override_default` to the gate definition in the YAML, or note that the type's built-in default would apply and explain what it would be.

---

## Issue 6 (MEDIUM): D3 references issue #108 but front-matter also says source_issue: 108

**Location:** Lines 16, 618-622

D3 says this PRD "supersedes PRD-override-gate-rationale" from issue #108. The front-matter says `source_issue: 108`. This creates ambiguity: is this PRD *for* issue #108 or does it *supersede* the earlier PRD that was also for #108?

**Recommended fix:** Clarify in D3 or front-matter whether this PRD replaces the earlier one under the same issue, or whether a new issue should track this broader scope.

---

## Issue 7 (MEDIUM): R10 says "no structured output enters the resolver" for legacy gates, but R1 says "gates always run and always return data"

**Location:** Lines 328-330 vs. 457-460

R1: "Gates always run and always return data -- there's no separate 'failure' state."

R10: "When transition `when` clauses don't reference `gates.*` fields, gates behave as today (boolean pass/fail, no structured output enters the resolver)."

These are in tension. R1 says gates always produce structured data. R10 says structured output doesn't enter the resolver for legacy templates. The gate still produces the data internally (satisfying R1), but the resolver doesn't receive it (satisfying R10). This distinction is implicit -- R10 should clarify that gates still produce structured output internally but the resolver only receives it when `when` clauses reference it.

**Recommended fix:** Add a clarifying sentence to R10: "Gates still produce structured output per R1, but the resolver only injects `gates.*` namespaced data when `when` clauses reference gate fields."

---

## Issue 8 (MEDIUM): AC gap -- no AC for R4's template-level override_default declaration

R4 says: "Template authors can declare a custom `override_default` per gate to route overrides to a different transition."

The ACs cover compiler rejection of invalid override_default (line 484-485) and substitution of override_default (line 499-500), but there's no AC verifying that a custom `override_default` declared in the template is actually used instead of the gate type's built-in default.

**Recommended fix:** Add an AC: "A gate with a custom `override_default` in the template uses that value (not the gate type's built-in default) when overridden without `--with-data`."

---

## Issue 9 (LOW): Problem statement line 29 says "boolean pass/fail checks"

**Location:** Line 29

> "Gates are boolean pass/fail checks."

This is the problem statement describing the current (broken) state, so it's technically correct. No change needed, but flagging for awareness since it matches the stale pattern.

**Verdict:** Acceptable -- problem framing, not requirements.

---

## Issue 10 (LOW): Example 7 override audit shows per-gate `gates_overridden` array with single element

**Location:** Lines 299-318

Each override event has `gates_overridden` as an array (plural) but R5a says "each call targets exactly one gate." The array will always have exactly one element. The field name `gates_overridden` (plural) is slightly misleading.

**Recommended fix:** Either rename to `gate_overridden` (singular object, not array) or add a note explaining the array is for forward compatibility.

---

## Issue 11 (LOW): R2 repeats R1 content

**Location:** Lines 364-373

R2 ("Gate evaluation produces structured data") largely restates what R1 already covers. R1 defines the schemas and says gates produce structured output. R2 adds implementation detail (the engine contains parsing logic) but the boundary between R1 and R2 is blurry.

**Verdict:** Not a bug, but could be tightened. R2 could focus solely on the parsing-logic-in-engine aspect.

---

## Issue 12 (LOW): Numbering -- Examples are numbered 1-7 sequentially

**Location:** Lines 104, 150, 179, 199, 242, 279, 297

Examples 1 through 7 are numbered sequentially. Requirements R1-R12 are sequential (R1-R10 functional, R11-R12 non-functional, with R5a as a sub-requirement). Decisions D1-D7 are sequential. All numbering is correct.

**Verdict:** No issues.

---

## Issue 13 (LOW): Known Limitation about dot-path traversal duplicates R3

**Location:** Lines 591-594

The known limitation about "dot-path traversal is new resolver capability" restates what R3 already requires. It's not contradictory -- it adds implementation context -- but it slightly blurs the line between limitations and implementation notes.

**Verdict:** Acceptable but could be trimmed.

---

## Issue 14 (LOW): Out of Scope mentions "evidence verification by koto" -- this borders on R9

**Location:** Lines 567-569

> "Evidence verification by koto. Future capability where koto independently validates agent-submitted evidence using gates."

R9 already has the compiler validating the contract. The out-of-scope item is about runtime verification, not compile-time, so it's correctly scoped out. But the phrasing "validates agent-submitted evidence using gates" could be confused with the gate-transition contract itself.

**Verdict:** Acceptable -- the distinction (compile-time vs. runtime) is clear enough on close reading.

---

## Cross-reference verification

| Reference | Location | Target | Correct? |
|-----------|----------|--------|----------|
| "per D6" | Not found in text | N/A | N/A -- no dangling references |
| "R5, R6, R8" in D3 | Line 621 | R5 (override command), R6 (override event), R8 (cross-epoch query) | Correct |
| "R12" in AC | Line 514 | R12 (rationale size limit) | Correct |
| "R11" in AC | Line 537 | R11 (event ordering) | Correct |

No dangling cross-references found.

---

## Requirement-to-AC coverage matrix

| Requirement | Has AC? | Notes |
|-------------|---------|-------|
| R1 (gate types with schemas) | Yes | Lines 479-482, 530-543 |
| R2 (structured evaluation) | Yes | Covered by R1's ACs (same behavior) |
| R3 (gate output in routing) | Yes | Lines 492-493, 515-518, 529 |
| R4 (override defaults) | Partial | Compiler rejection covered (484-485), but custom override_default usage not covered (Issue 8) |
| R5 (override command) | Yes | Lines 498-500, 504-506 |
| R5a (one gate one rationale) | Yes | Lines 508-509 |
| R6 (override event) | Yes | Lines 500-502 |
| R7 (coexistence) | Yes | Lines 494-496, 546-547 |
| R8 (cross-epoch query) | Yes | Lines 503, 525 |
| R9 (compiler validation) | Yes | Lines 484-485, 519-522 |
| R10 (backward compat) | Yes | Lines 523-526 |
| R11 (event ordering) | Yes | Line 537 |
| R12 (rationale size limit) | Yes | Line 514 |

---

## Summary of actionable issues

| # | Severity | Summary |
|---|----------|---------|
| 2 | HIGH | User stories use "failed gate" / "gate failure" -- stale language from boolean model |
| 3 | HIGH | Example 4 introduces `contains` array operator not defined in any requirement |
| 4 | MEDIUM | `gate_blocked` action value and `blocking_conditions` response schema not formally required |
| 5 | MEDIUM | Example 4 references `override_default` for an undefined future gate type |
| 6 | MEDIUM | D3 and front-matter both reference issue #108 with ambiguous relationship |
| 7 | MEDIUM | R1 vs R10 tension on whether gates "always return data" vs. "no structured output enters resolver" |
| 8 | MEDIUM | Missing AC for custom `override_default` usage from template |
| 10 | LOW | `gates_overridden` array always has one element per R5a |
| 11 | LOW | R2 largely duplicates R1 |
| 13 | LOW | Known limitation on dot-path traversal duplicates R3 |
| 14 | LOW | Out-of-scope "evidence verification" phrasing could confuse with R9 |
