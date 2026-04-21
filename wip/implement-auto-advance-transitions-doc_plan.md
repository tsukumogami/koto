# Documentation Plan: Auto-Advance Transitions via skip_if

Generated from: docs/plans/PLAN-auto-advance-transitions.md
Issues analyzed: 5
Total entries: 4

---

## doc-1: plugins/koto-skills/skills/koto-author/references/template-format.md
**Section**: Layer 3: Advanced features
**Prerequisite issues**: #1, #3
**Update type**: modify
**Status**: pending
**Details**: Add a new `skip_if` subsection under Layer 3. Cover the field syntax (flat dict, same dot-path keys as `when` clauses), the three motivating condition types (gate output via `gates.NAME.field`, template variable existence via `vars.NAME: {is_set: true}`, and direct evidence values), and the context-exists gate workaround pattern. Document the four compile-time rules: E-SKIP-TERMINAL (error on terminal states), E-SKIP-NO-TRANSITIONS (error when no transitions declared), E-SKIP-AMBIGUOUS (error when skip_if values match zero or more than one conditional transition), and W-SKIP-GATE-ABSENT (warning when a `gates.NAME.*` key references an undeclared gate). Include the three YAML examples from the design doc (gate-backed, vars-based, and evidence-value-based). Note that consecutive skip_if states chain within a single `koto next` call. Also update the "Compile and runtime rule vocabulary" table at the bottom of the file to add E-SKIP-TERMINAL, E-SKIP-NO-TRANSITIONS, E-SKIP-AMBIGUOUS, and W-SKIP-GATE-ABSENT to the rule ID listing.

---

## doc-2: plugins/koto-skills/skills/koto-user/references/response-shapes.md
**Section**: Scenario (c): evidence_required — auto-advance candidate (empty expects)
**Prerequisite issues**: #2, #3
**Update type**: modify
**Details**: The existing scenario (c) describes states that auto-advance without skip_if. Extend or add a new scenario describing what agents see after a skip_if fires. Key points to document: `advanced: true` is returned when any skip_if transition fired during the call; the JSONL log contains a `Transitioned` event with `"condition_type": "skip_if"` and a non-null `skip_if_matched` map (relevant for agents that inspect the log directly); consecutive skip_if states chain in a single `koto next` call so the agent may land several states ahead of where it started. Note in the "Checking for absent fields" section that `skip_if_matched` is absent when `condition_type` is not `"skip_if"`.
**Status**: pending

---

## doc-3: docs/reference/error-codes.md
**Section**: template compile
**Prerequisite issues**: #1
**Update type**: modify
**Status**: updated
**Details**: Expand the `template compile` error section to document the four new compile-time diagnostic codes. For each code, provide the error text format koto emits and what the template author must do to fix it: E-SKIP-TERMINAL (remove skip_if or remove terminal: true), E-SKIP-NO-TRANSITIONS (add at least one transition), E-SKIP-AMBIGUOUS (ensure skip_if values route to exactly one conditional transition — include what the error message identifies: which transitions matched and which values caused the ambiguity), and W-SKIP-GATE-ABSENT (add the referenced gate name to the state's gates block or correct the skip_if key). Distinguish errors (build fails) from warnings (build succeeds with diagnostic).

---

## doc-4: docs/designs/DESIGN-auto-advance-transitions.md
**Section**: Status (frontmatter and body)
**Prerequisite issues**: #1, #2, #3, #4, #5
**Update type**: modify
**Status**: pending
**Details**: Update the `status` frontmatter field from `Planned` to `Implemented` and the `## Status` body section accordingly. This should happen after all five issues are merged. No other content in the design doc needs to change — the architecture, decisions, and consequences sections accurately describe the implemented design.
