# koto

Workflow orchestration engine for AI coding agents. Enforces execution order through a state machine, persists progress atomically, and makes every state transition recoverable.

## Repo Visibility: Public

## Repository Structure

```
koto/
├── src/             # Crate root: library plus the binary entry point
│   ├── main.rs      # CLI entry point (a shim over src/cli)
│   ├── engine/      # State machine and advance loop
│   ├── template/    # Template parsing and compilation
│   ├── gate.rs      # Gate evaluators (command, context-exists, context-matches)
│   └── cli/         # CLI subcommands and JSON output types
├── plugins/         # Agent skill plugins
│   └── koto-skills/ # koto-adhoc, koto-author, and koto-user skills
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

## Names must resolve

`cargo test --test doc_names` checks that every `koto <verb>` and every
repo-relative path written in code font, across `src/` and the user-facing
docs and skills, refers to something that exists. Verbs resolve against the live
clap tree, so nothing here is a list anyone maintains.

It exists because v0.12.0 shipped an error message telling users to run
`koto session rebind`, which does not exist, past fmt, clippy, the full suite,
and CI. When it fires, the output names the token, where it is, and what to do.
A name that is deliberately not built yet goes in `tests/doc_names.allow` with a
reason and an issue -- deleting the check is not the remedy, and a record that
stops matching is itself reported. `tests/doc_names.rs`'s header says what it
deliberately ignores and why; `tests/doc_names_evidence.md` carries the proof
that it catches the v0.12.0 case.

## koto-skills Plugin Maintenance

Three skills in `plugins/koto-skills/skills/` guide agents authoring and running koto-backed workflows. They drift silently when koto changes without a corresponding skill update.

| Skill | Path | Scope |
|-------|------|-------|
| `koto-adhoc` | `plugins/koto-skills/skills/koto-adhoc/` | Guides agents decomposing a one-off task and running it without a committed template |
| `koto-author` | `plugins/koto-skills/skills/koto-author/` | Guides agents writing koto templates |
| `koto-user` | `plugins/koto-skills/skills/koto-user/` | Guides agents running koto-backed workflows |

**After completing any source change in `src/`, assess all three skills before closing the work:**

1. **Broken contracts** -- read the diff and each skill, then ask: does anything the skill currently documents no longer match the code? Look for changed flag names, renamed fields, removed subcommands, altered response shapes, or behavior that works differently than described.

2. **New surface** -- ask: does this change add CLI flags, subcommands, response fields, gate types, or behavior that neither skill mentions? New surface that agents will encounter belongs in the relevant skill.

If either question surfaces gaps, update the skill in the same PR. A separate skill-update PR is acceptable only when the scope is large enough to warrant it -- document the gap in the PR description so it isn't lost.

Source areas most likely to require skill updates:

| Area | Relevant skill |
|------|---------------|
| `src/cli/` -- subcommands, flags, JSON output types | all three |
| `src/engine/` -- advance loop, action values, response schema | koto-user, koto-adhoc |
| `src/gate.rs` -- gate types, structured output fields | all three |
| `src/template/` -- frontmatter fields, compiler errors/warnings | koto-author, koto-adhoc |

### Running skill evals

Run evals after modifying any skill content (`SKILL.md`, reference files, or evals themselves):

```bash
# Run evals for one skill
scripts/run-evals.sh koto-user

# Run evals for all skills
scripts/run-evals.sh --all

# List skills with evals
scripts/run-evals.sh --list

# Re-validate latest results without re-running
scripts/run-evals.sh --validate koto-user
```

The script spawns a single `claude -p` session per skill that runs with-skill and without-skill agents for each eval, then grades against assertions.

**Include eval results in the PR description** when submitting skill changes. Use this format:

```
## Eval Results

| Skill | Assertions | with_skill | without_skill | Delta |
|-------|-----------|------------|---------------|-------|
| koto-user | 18/18 (100%) | 100% | 60% | +40pp |
```

CI enforces that every skill has at least one eval (`check-evals-exist.sh`). Running the evals themselves is manual -- they require an Anthropic API key and spawn Claude sessions.
