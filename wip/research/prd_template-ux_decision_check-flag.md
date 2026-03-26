# Decision: --check flag on export command

## Alternatives Evaluated

### 1. No --check flag

**How it works:** The `export` command only generates output. CI drift detection uses the standard `git diff --exit-code` pattern:

```yaml
- run: koto template export --output templates/workflow.mermaid.md templates/workflow.md
- run: git diff --exit-code templates/workflow.mermaid.md
```

**Pros:**
- Simplest implementation -- export does one thing (generate output)
- `git diff --exit-code` is a well-understood CI primitive; every developer knows what it means
- No new flag to document, test, or maintain
- Works identically for the "new template, no diagram committed" case -- `git diff` shows untracked/modified files the same way
- The reusable GHA workflow from the PRD scope already wraps this pattern, so individual users don't write it by hand

**Cons:**
- Two commands instead of one in CI steps
- Drift check depends on git being available (always true in CI, but technically an external dependency)
- Error output is a raw git diff, not a purpose-built message

### 2. --check flag

**How it works:** `koto template export --output file.mermaid.md --check` generates the diagram in memory, compares it to the existing file on disk, and exits non-zero if they differ. Modeled after `cargo fmt --check`, `prettier --check`, `terraform fmt -check`, `gofmt -l`.

```yaml
- run: koto template export --output templates/workflow.mermaid.md --check
```

**Pros:**
- Single command for CI -- cleaner workflow definitions
- Self-documenting intent: "check freshness" is explicit in the command
- Can produce a tailored error message ("diagram is stale, re-run `koto template export`")
- Follows a pattern established by widely-used tools; users expect it

**Cons:**
- Additional code path to implement and test (comparison logic, "file doesn't exist yet" edge case)
- Modest complexity: `--check` and `--output` interact (does `--check` without `--output` compare to stdout? does it write the file or only compare?)
- The "new template, no diagram file yet" case needs a clear answer: exit non-zero with "file missing" is the right behavior, but it's a design decision that must be documented

**Edge case resolution:** When the target file doesn't exist, `--check` exits non-zero with a message like `"diagram file not found: templates/workflow.mermaid.md"`. This correctly catches the case where a contributor adds a template without generating its diagram.

### 3. Separate verify subcommand

**How it works:** A dedicated `koto template verify-diagram` command that takes the expected output path and source template, re-generates the diagram, and compares.

```yaml
- run: koto template verify-diagram templates/workflow.md templates/workflow.mermaid.md
```

**Pros:**
- Clear separation of concerns: "generate" vs. "verify" are distinct commands
- Verification could do more than byte-compare in the future (structural equivalence, partial checks)

**Cons:**
- Adds a full subcommand to the CLI surface for a capability that's really a mode of the existing command
- `verify-diagram` is specific to Mermaid export; if koto later exports other formats, we'd need `verify-X` for each one or a generic `verify` that re-implements `--check`
- Breaks the pattern established by `cargo fmt`, `prettier`, and `terraform fmt`, which all use flags rather than separate commands for the "check without writing" mode
- More code to maintain than a flag on the existing command

## Recommendation

**Use `--check` flag on the export command (Alternative 2).**

The `--check` flag is the right choice because it follows strong prior art (`cargo fmt --check`, `prettier --check`, `terraform fmt -check`, `gofmt -l`), keeps the CLI surface minimal (one command, one flag), and makes the CI workflow self-documenting. The implementation cost is low -- it's a byte comparison against the target file before writing.

The `git diff --exit-code` alternative (1) works fine, but it pushes CI-specific glue onto every user of the reusable GHA workflow. Since the PRD already scopes a reusable workflow, the GHA can call `koto template export --check` in one step instead of orchestrating generate-then-diff. The separate subcommand (3) over-indexes on separation of concerns for what amounts to a dry-run mode.

Key design details for the flag:
- `--check` requires `--output` (comparing against stdout is meaningless)
- When the target file doesn't exist, exit non-zero with a clear message
- When content differs, print a one-line message pointing to the stale file (not a full diff -- that's what `git diff` is for)
- Exit code: 0 = fresh, 1 = stale or missing

## Impact on PRD

The PRD should specify `--check` as a flag on `koto template export`, not as a separate command. Relevant sections:

1. **CLI surface**: Document `koto template export --output <path> [--check] <source>` with the flag's semantics (exit 0 if fresh, exit 1 if stale/missing, no file written when `--check` is set).

2. **GHA reusable workflow**: The drift-detection step should use `koto template export --output <path> --check` rather than generate + `git diff`. This keeps the workflow simpler and decoupled from git internals.

3. **User journey (repo maintainer)**: When documenting the CI setup, show the `--check` flag as the primary drift-detection mechanism. Mention `git diff --exit-code` as an alternative for users who prefer it.
