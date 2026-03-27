# Documentation Plan: visual-workflow-preview

Generated from: docs/plans/PLAN-visual-workflow-preview.md
Issues analyzed: 7
Total entries: 4

---

## doc-1: docs/guides/cli-usage.md
**Section**: template export (new subsection under "### template")
**Prerequisite issues**: #1, #3, #4
**Update type**: modify
**Status**: pending
**Details**: Add a `#### template export` subsection documenting the `koto template export` command. Cover: positional `<source>` argument accepting `.md` or `.json` input, `--format mermaid|html` flag (mermaid default), `--output` flag, `--check` flag with exit code behavior, `--open` flag (html only). Include examples for mermaid to stdout, mermaid to file, html to file with --open, and --check for CI freshness. Note that export errors go to stderr as plain text (not JSON), unlike other koto commands. Document flag compatibility rules (R15) inline.

---

## doc-2: docs/reference/error-codes.md
**Section**: template export (new subsection after "### template validate")
**Prerequisite issues**: #1, #3, #4
**Update type**: modify
**Status**: pending
**Details**: Add a `### template export` section documenting error conditions and exit codes. Export differs from other commands: errors are plain text to stderr, not JSON. Document: flag validation errors (exit 2) for the four R15 invalid combinations, non-existent input file (exit 2), malformed JSON input, template compilation failure, stale/missing check result (exit 1). Include the fix-command format shown in stale check output.

---

## doc-3: docs/guides/cli-usage.md
**Section**: template export --check and CI freshness (new content within the template export subsection from doc-1)
**Prerequisite issues**: #3, #6
**Update type**: modify
**Status**: pending
**Details**: Within the template export subsection, add a focused paragraph or sub-section on CI freshness enforcement. Show the --check workflow: generate once, commit the artifact, verify in CI with --check. Mention the reusable GHA workflow at `.github/workflows/check-template-freshness.yml` with a minimal caller example (template-paths glob, koto-version input). Reference the `.gitattributes` recommendation for `*.mermaid.md text eol=lf`. Keep the GHA coverage brief since the workflow file itself will contain usage comments.

---

## doc-4: docs/guides/custom-skill-authoring.md
**Section**: visual documentation (potential new subsection)
**Prerequisite issues**: #1, #4
**Update type**: modify
**Status**: pending
**Details**: If the guide covers template development workflow, add a brief note that `koto template export` can generate visual diagrams for templates. One or two sentences pointing readers to the CLI usage guide for details. Skip this entry if the guide doesn't cover template authoring workflow -- verify by reading the file before writing.
