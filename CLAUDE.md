# koto

Workflow orchestration engine for AI coding agents. Enforces execution order through a state machine, persists progress atomically, and makes every state transition recoverable.

## Repository Structure

```
koto/
├── cmd/koto/        # CLI entry point
├── src/             # Core library (engine, template, gate, CLI)
│   ├── engine/      # State machine and advance loop
│   ├── template/    # Template parsing and compilation
│   ├── gate/        # Gate evaluators (command, context-exists, context-matches)
│   └── cli/         # CLI subcommands and JSON output types
├── plugins/         # Agent skill plugins
│   └── koto-skills/ # koto-author and koto-user skills
├── docs/            # Design docs, PRDs, guides
├── test/            # Functional tests (Gherkin scenarios + fixtures)
└── .github/         # CI/CD pipelines
```

## Quick Reference

```bash
# Build
cargo build --release

# Test (unit + integration)
cargo test

# Functional tests
cargo test --test integration_test

# Lint
cargo clippy && cargo fmt --check
```

## Key Points

- All Rust code must pass `cargo fmt` and `cargo clippy`
- CI runs tests and linting on every PR
- Templates are markdown files with YAML frontmatter
- State files are written atomically to prevent corruption
- `wip/` must be empty before merging to main (CI enforces this)
- Never add AI attribution or co-author lines to commits or PRs

## koto-skills Plugin Maintenance

Two skills in `plugins/koto-skills/skills/` guide agents authoring and running koto-backed workflows. They drift silently when koto changes without a corresponding skill update.

| Skill | Path | Scope |
|-------|------|-------|
| `koto-author` | `plugins/koto-skills/skills/koto-author/` | Guides agents writing koto templates |
| `koto-user` | `plugins/koto-skills/skills/koto-user/` | Guides agents running koto-backed workflows |

**After completing any source change in `src/` or `cmd/`, assess both skills before closing the work:**

1. **Broken contracts** -- read the diff and each skill, then ask: does anything the skill currently documents no longer match the code? Look for changed flag names, renamed fields, removed subcommands, altered response shapes, or behavior that works differently than described.

2. **New surface** -- ask: does this change add CLI flags, subcommands, response fields, gate types, or behavior that neither skill mentions? New surface that agents will encounter belongs in the relevant skill.

If either question surfaces gaps, update the skill in the same PR. A separate skill-update PR is acceptable only when the scope is large enough to warrant it -- document the gap in the PR description so it isn't lost.

Source areas most likely to require skill updates:

| Area | Relevant skill |
|------|---------------|
| `src/cli/` -- subcommands, flags, JSON output types | both |
| `src/engine/` -- advance loop, action values, response schema | koto-user |
| `src/gate/` -- gate types, structured output fields | both |
| `src/template/` -- frontmatter fields, compiler errors/warnings | koto-author |
