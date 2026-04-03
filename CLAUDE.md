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

## Key Commands

| Command | Description |
|---------|-------------|
| `koto init <name> --template <path>` | Initialize a workflow from a template |
| `koto next <name>` | Get the current state directive (returns JSON) |
| `koto next <name> --with-data '<json>'` | Submit evidence and advance |
| `koto next <name> --to <state>` | Force transition to a named state |
| `koto overrides record <name> --gate <gate> --rationale <text>` | Override a blocked gate |
| `koto overrides list <name>` | List recorded overrides |
| `koto decisions record <name> --with-data '<json>'` | Record a decision |
| `koto rewind <name>` | Roll back to the previous state |
| `koto workflows` | List active workflows in the current directory |
| `koto template compile <path>` | Validate and compile a template |

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

**Review these skills after any PR that changes:**
- Template format (new frontmatter fields, new gate types, new state options)
- Compiler checks (new errors/warnings, new flags like `--allow-legacy-gates`)
- Gate behavior (structured output, `gates.*` routing, `blocking_conditions` response fields)
- Evidence submission format or validation rules
- `koto next` response schema (`action`, `directive`, `expects`, `blocking_conditions`)
- Override mechanism (`koto overrides record/list`, rationale, `--with-data`)
- Any CLI subcommand added, changed, or removed

Check both skills before closing the milestone.
