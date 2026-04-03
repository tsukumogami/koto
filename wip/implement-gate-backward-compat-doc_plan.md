# Documentation Plan: Gate backward compatibility

Generated from: docs/plans/PLAN-gate-backward-compat.md
Issues analyzed: 3
Total entries: 3

---

## doc-1: docs/guides/cli-usage.md
**Section**: template compile
**Prerequisite issues**: #2
**Update type**: modify
**Status**: pending
**Details**: Add `--allow-legacy-gates` flag to the `koto template compile` command reference. Document that the flag suppresses the legacy-gate error and D4 unreferenced-field warnings, and note that it's a transitory flag intended for templates migrating to structured `gates.*` routing. Include the error message shown when the flag is absent on a legacy-gate template (state name, gate name, hint to add a `when` clause or pass the flag). Also document that `koto init` always compiles in permissive mode and emits a warning to stderr for legacy-gate templates rather than failing.

---

## doc-2: docs/guides/custom-skill-authoring.md
**Section**: Step 2: Validate the template
**Prerequisite issues**: #2
**Update type**: modify
**Status**: pending
**Details**: The guide tells template authors to run `koto template compile` as their validation loop. Add a note covering the new default behavior: templates with gates but no `gates.*` routing references now fail compilation with a D5 error. If the template intentionally uses legacy boolean pass/block behavior, authors must pass `--allow-legacy-gates`. Clarify that this flag is for migration scaffolding and should be removed once the template adopts structured routing.

---

## doc-3: docs/designs/DESIGN-gate-backward-compat.md
**Section**: Status
**Prerequisite issues**: #1, #2, #3
**Update type**: modify
**Status**: pending
**Details**: Update the front-matter `status` field from `Planned` to `Implemented` once all three issues land.
