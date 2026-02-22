# Documentation Plan: koto-template-format

Generated from: docs/designs/DESIGN-koto-template-format.md
Issues analyzed: 5
Total entries: 7

---

## doc-1: README.md
**Section**: Quick start
**Prerequisite issues**: #14
**Update type**: modify
**Status**: pending
**Details**: Replace the template example in "Create a workflow template" to use the new source format (YAML frontmatter with `states:` block declaring transitions, instead of `**Transitions**` lines in the markdown body). The example currently shows the old single-format approach. Keep the example small -- just enough to show the structure.

---

## doc-2: README.md
**Section**: Key concepts
**Prerequisite issues**: #14, #15
**Update type**: modify
**Status**: pending
**Details**: Update the Templates paragraph to explain the source/compiled split: source files (.md with YAML frontmatter) are what authors write, a deterministic compiler produces JSON for the engine. Mention evidence as an additional concept -- data accumulated across transitions for gate evaluation. Add a sentence about gates as exit conditions on states.

---

## doc-3: docs/guides/cli-usage.md
**Section**: transition command
**Prerequisite issues**: #15, #16
**Update type**: modify
**Status**: pending
**Details**: Add the `--evidence KEY=VALUE` optional flag to the `transition` command section. Show it's repeatable. Add an example with evidence. Document that gate evaluation happens during transition and that `gate_failed` errors are returned when gates don't pass. Update the JSON output example for `query` to show `schema_version: 2` and the `evidence` field.

---

## doc-4: docs/guides/cli-usage.md
**Section**: Typical agent workflow
**Prerequisite issues**: #15
**Update type**: modify
**Status**: pending
**Details**: Update the agent workflow loop example to show passing `--evidence` on transitions where the workflow requires it. Keep it simple -- one evidence flag in the transition call is enough to show the pattern.

---

## doc-5: docs/guides/library-usage.md
**Section**: Building a Machine
**Prerequisite issues**: #13
**Update type**: modify
**Status**: pending
**Details**: Add `Gates` field to the `MachineState` example in programmatic construction. Show a simple `field_not_empty` gate. Add a new subsection "From compiled JSON" showing `ParseJSON()` to load a `CompiledTemplate` and build a Machine from it. Mention the `CompiledTemplate`, `VariableDecl`, `StateDecl`, and `GateDecl` types briefly.

---

## doc-6: docs/guides/library-usage.md
**Section**: Transitioning
**Prerequisite issues**: #15, #16
**Update type**: modify
**Status**: pending
**Details**: Update the `Transition` call to show the `TransitionOption` pattern: `eng.Transition("build", engine.WithEvidence(map[string]string{"result": "pass"}))`. Explain that gates are evaluated between validation and commit. Mention that `gate_failed` errors are returned as `*engine.TransitionError` with the existing error handling pattern.

---

## doc-7: docs/reference/error-codes.md
**Section**: gate_failed
**Prerequisite issues**: #16
**Update type**: modify
**Status**: pending
**Details**: Add a new `gate_failed` error code section following the existing format. Document when it occurs (transition called but one or more gates on the current state didn't pass), the JSON shape, and how to handle it (check which gate failed, supply required evidence, fix the condition the command gate checks). Also add the namespace collision error for when evidence keys shadow declared variables.
