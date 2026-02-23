# Documentation Plan: koto-cli-tooling

Generated from: docs/designs/DESIGN-koto-cli-tooling.md
Issues analyzed: 4
Total entries: 3

---

## doc-1: docs/guides/cli-usage.md
**Section**: Commands
**Prerequisite issues**: #20
**Update type**: modify
**Status**: updated
**Details**: Add a new `koto template compile` subsection under Commands, documenting the `<path>` positional argument, `--output <file>` flag, stdout/stderr behavior, and exit codes. Also add the `template` subcommand group to the commands list. Place after `workflows` and before `version`.

---

## doc-2: docs/guides/library-usage.md
**Section**: From a template file, Using the controller
**Prerequisite issues**: #19, #22
**Update type**: modify
**Status**: pending
**Details**: Replace the two `template.Parse()` code examples (lines ~52-61 and ~227-228) with the compiler path: `compile.Compile(sourceBytes)` followed by `template.ParseJSON(compiledJSON)` and `CompiledTemplate.ToTemplate()`. Add the `compile` package to the import table. Note that `Parse()` is deprecated. Keep the `Interpolate` section unchanged since that helper is unaffected.

---

## doc-3: README.md
**Section**: Key concepts
**Prerequisite issues**: #20
**Update type**: modify
**Status**: updated
**Details**: Add a brief mention of `koto template compile` as the authoring/validation tool for template development, in the existing "Templates" paragraph or as a new short paragraph after it. Also add it to the Documentation section's CLI usage guide link description so users know the guide covers template authoring commands.
