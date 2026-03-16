# Documentation Plan: Template Evidence Routing

Generated from: docs/plans/PLAN-template-evidence-routing.md
Issues analyzed: 5
Total entries: 3

---

## doc-1: docs/guides/custom-skill-authoring.md
**Section**: (multiple sections)
**Prerequisite issues**: Issue 5
**Update type**: modify
**Status**: pending
**Details**: Update the hello-koto template example in "Step 1: Write the workflow template" to use structured transition syntax (`- target: eternal` instead of `transitions: [eternal]`). Update the compiled JSON output in "Step 2: Validate the template" to show structured transitions in the compiled output. Update the "Worked example: hello-koto" section's template description to match. The gate examples (command type) remain unchanged. Remove or update any references to `field_not_empty` or `field_equals` gate types if present in gate documentation text.

---

## doc-2: docs/reference/error-codes.md
**Section**: template compile
**Prerequisite issues**: Issue 1, Issue 2
**Update type**: modify
**Status**: pending
**Details**: Add new compiler error examples for evidence routing validation failures: rejected field gate types (`field_not_empty` and `field_equals` now produce errors pointing to `accepts`/`when`), invalid `when` conditions (references undeclared fields, non-exclusive conditions, empty `when` blocks), and invalid `accepts` schemas (unknown field types, enum without values). These are new error conditions the user may encounter during `koto template compile`.

---

## doc-3: docs/designs/current/DESIGN-koto-template-format.md
**Section**: Status, Decision 4
**Prerequisite issues**: Issue 1, Issue 2
**Update type**: modify
**Status**: pending
**Details**: Add a note at the top of "Decision 4: Evidence Gate Declarations" indicating that `field_not_empty` and `field_equals` gate types have been removed and replaced by the `accepts`/`when` system. Link to `DESIGN-template-evidence-routing.md` for the replacement design. The template examples in the doc still use old syntax; add a note that they reflect the original design and the current syntax uses structured transitions. This preserves the design doc as historical record while preventing confusion.
