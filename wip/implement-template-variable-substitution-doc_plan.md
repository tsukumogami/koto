# Documentation Plan: template-variable-substitution

Generated from: docs/plans/PLAN-template-variable-substitution.md
Issues analyzed: 3
Total entries: 4

---

## doc-1: docs/guides/cli-usage.md
**Section**: Commands > init
**Prerequisite issues**: #2
**Update type**: modify
**Status**: pending
**Details**: Add `--var KEY=VALUE` as a repeatable optional flag to the `init` command section. Document the flag's parsing behavior (split on first `=`), error conditions (missing `=`, empty key, duplicate keys, unknown keys, missing required variables, forbidden characters), and the allowlist regex `^[a-zA-Z0-9._/-]+$`. Include an example showing `koto init task-42 --template workflow.md --var ISSUE_NUMBER=42`.

---

## doc-2: docs/guides/cli-usage.md
**Section**: Commands > template compile
**Prerequisite issues**: #1
**Update type**: modify
**Status**: pending
**Details**: Add a note that `koto template compile` now validates `{{KEY}}` variable references in directive text and gate commands against the template's `variables` block. Undeclared references produce a compilation error naming the variable and the state where it appears.

---

## doc-3: docs/reference/error-codes.md
**Section**: Error conditions by command > init, Error conditions by command > next
**Prerequisite issues**: #2, #3
**Update type**: modify
**Status**: pending
**Details**: Add new error conditions under `init`: invalid `--var` format (no `=` or empty key), duplicate `--var` keys, unknown variable key, missing required variable, forbidden characters in value. Add a new error condition under `next`: variable re-validation failure (state file contains values that don't match the allowlist, indicating file tampering or corruption). Include example JSON for each error and note the exit codes.

---

## doc-4: README.md
**Section**: Quick start
**Prerequisite issues**: #2, #3
**Update type**: modify
**Status**: pending
**Details**: Update the "Initialize a workflow" example to pass `--var PR_URL=https://github.com/example/repo/pull/1` on `koto init`. Update the "Get the current directive" example so the `directive` field shows the substituted value instead of raw `{{PR_URL}}`.
