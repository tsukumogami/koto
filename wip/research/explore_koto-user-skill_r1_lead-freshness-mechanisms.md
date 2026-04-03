# Lead: skill freshness enforcement mechanisms

## Findings

### CI pipeline overview

The koto repo has seven workflow files in `.github/workflows/`:

- `validate.yml` — runs on every push/PR to main: `cargo test`, `cargo fmt --check`, `cargo clippy`, `cargo audit`, coverage upload, and a tsuku distributed install smoke test. Also runs a `check-artifacts` job that rejects PRs containing any files in `wip/`.
- `validate-plugins.yml` — triggers only when `plugins/**` or `.claude-plugin/**` change. Runs template compilation (finds all `koto-templates/*.md` under `plugins/koto-skills/skills/` and calls `koto template compile` on each), a hook smoke test, and JSON schema validation for `plugin.json` and `marketplace.json`. Uses path filters, so it only runs when plugin files are modified.
- `eval-plugins.yml` — triggers only when `plugins/**` change. Runs `plugins/koto-skills/eval.sh` if `ANTHROPIC_API_KEY` is configured in secrets. Gracefully skips if the key is absent.
- `check-template-freshness.yml` — a reusable `workflow_call` workflow that verifies Mermaid and HTML exports are fresh. Called by downstream repos that use koto templates.
- `finalize.yml`, `prepare-release.yml`, `release.yml` — release pipeline workflows.

### (a) CLAUDE.md trigger list

Already implemented in `CLAUDE.local.md`. The "koto-skills Plugin Maintenance" section lists the exact categories of koto changes that should trigger a skill review: template format changes, compiler checks, gate behavior, evidence submission, `koto next` response schema, override mechanism, and CLI subcommand changes.

**Feasibility**: Already in place; zero cost to add.

**False-positive rate**: N/A — it's a reminder, not an automated check. It only fires when a human reads the file.

**Maintenance burden**: Low if the list is coarse-grained (categories not specific files), but the categories may drift from reality over time. The list is currently accurate.

**Assessment**: Effective for committed, disciplined reviewers. Does not catch anything automatically. Useful as the primary signal but cannot serve as the only mechanism.

### (b) CI file-change heuristics

`validate-plugins.yml` already uses GitHub Actions `paths:` filtering: it only runs when `plugins/**` or `.claude-plugin/**` change. This is the right model for heuristic enforcement.

A path-based skill-freshness check could require that any PR touching specific source files (e.g., `src/engine/advance.rs`, `src/gate.rs`, `src/template/types.rs`, `src/cli/**`) must also modify at least one file under `plugins/koto-skills/skills/`. The check would be implemented as an additional job in `validate-plugins.yml` or a new workflow.

**Feasibility**: High. GitHub Actions `paths:` filtering is already used in `validate-plugins.yml`. A new job could use `github.event.pull_request` and the GitHub API (or `git diff --name-only`) to inspect which files changed in the PR and assert that skill files are among them.

**False-positive rate**: High. Not every engine change requires a skill update. A refactor to `advance.rs` that doesn't change observable behavior (response shape, CLI flags, gate semantics) would trigger the check unnecessarily. Suppression would require either a PR label (e.g., `skip-skill-check`) or a commit message tag, both of which add process overhead.

**Maintenance burden**: Medium. The list of source files that trigger the check needs updating whenever new engine files are added. It's another artifact that drifts.

**Assessment**: Architecturally sound but high false-positive rate makes it noisy. The heuristic can't distinguish a refactor from a behavior change. Worth considering only if paired with a cheap suppression mechanism (e.g., a label).

### (c) LLM-in-test

`eval-plugins.yml` already demonstrates this pattern. It calls `eval.sh`, which sends `SKILL.md` content plus a user prompt to the Anthropic Messages API (`claude-sonnet-4-20250514`) and checks the model's response against regex patterns in `patterns.txt`. `ANTHROPIC_API_KEY` is stored in repository secrets and the job gracefully skips if the key is absent.

A skill-accuracy eval could use the same harness: provide the skill content as the system prompt, provide a description of a specific koto behavior as the user message, and assert that the model's response contains accurate information about that behavior. For example:

- Prompt: "I called `koto next` and got a response with `action: gate_blocked`. What should I do?"
- Expected pattern: `blocking_conditions` and `koto next` (retry)

The key limitation is that this tests whether the *model* produces correct output given the skill, not whether the skill *text itself* is accurate. A skill that is missing a concept entirely would not be flagged if the model fills the gap from its own training knowledge. To detect omissions, the eval would need to explicitly probe for specific content — effectively becoming a structured coverage test (approach (d)).

**Feasibility**: High infrastructure-wise — the harness and API key already exist. Designing evals that reliably detect drift (not just model knowledge) is harder.

**False-positive rate**: Low for behavior tests (the model either uses `koto rewind` correctly or doesn't), but the evals don't currently exist for the koto-user skill. They would need to be written.

**Cost**: Each eval case costs ~$0.01–0.03 (noted in `eval.sh` comments). A set of 5–10 eval cases targeting key concepts costs under $0.30 per PR run. The `eval-plugins.yml` workflow only runs when `plugins/**` changes, so the cost per PR is bounded.

**Maintenance burden**: Each eval case is a directory with `prompt.txt`, `patterns.txt`, and `skill_path.txt`. Adding a new concept to the skill requires a new eval case. Cases go stale when behavior changes — a pattern that matched the old response no longer matches after a format change.

**Assessment**: Appropriate for this codebase. Infrastructure already exists. Best used to verify that the skill correctly guides model behavior for key workflows (init, evidence submission, gate handling, rewind), not to exhaustively verify every sentence.

### (d) Structured coverage tests

This approach asserts that specific strings or markers appear in skill files. Examples:

- `gates.` appears in `references/template-format.md` (verifies structured gate routing is documented)
- `blocking_conditions` appears in `AGENTS.md` or `SKILL.md`
- `koto decisions record` appears somewhere in the skill content
- `--allow-legacy-gates` appears in template-format.md

These could be implemented as a simple shell script in CI or as a Rust test that reads the skill files and checks for the presence of required strings.

**Feasibility**: High. Trivially implemented as a `grep`-based script. No API keys or external dependencies.

**False-positive rate**: Low — strings either appear or they don't. False negatives (a string present but semantically wrong) are the real risk.

**Maintenance burden**: High relative to its value. Every new concept requires adding a new marker to check. The marker list is itself an artifact that drifts. It's essentially a lightweight form of spec compliance, and the spec is the ever-evolving koto source. The check answers "is `blocking_conditions` mentioned?" but not "is `blocking_conditions` explained correctly?" A skill that mentions the term in passing but describes it incorrectly passes the check.

**Assessment**: Good as a floor — catching total omissions of key concepts. Not useful for detecting semantic drift. Works best for high-value, stable concepts (e.g., `koto decisions record`, `blocking_conditions`, `koto rewind`) that are unlikely to be renamed.

### Existing eval harness — no eval cases yet

The `eval.sh` script expects an `evals/` directory under `plugins/koto-skills/` containing subdirectories with `prompt.txt`, `skill_path.txt`, and `patterns.txt`. That directory does not currently exist. The `eval-plugins.yml` workflow runs `eval.sh` with no arguments, which would exit with "No evals/ directory found" — but this is guarded by the `ANTHROPIC_API_KEY` check, so CI currently skips the step silently. The infrastructure is ready; the test cases are not written.

### Hybrid approach

A layered approach combines strengths:

1. **CLAUDE.md trigger list** (already in place) — primary signal for humans reviewing PRs.
2. **Structured coverage tests** — CI floor check for total omissions of key concepts. Low cost, no API dependency.
3. **LLM evals for key workflows** — CI behavioral tests for the scenarios most likely to break if skills drift (init, evidence submission, gate handling, rewind). Use the existing harness in `eval-plugins.yml`.
4. **File-change heuristics** — optional, add only if the false-positive rate can be tolerated or suppressed with a label.

The LLM evals and coverage tests are complementary: coverage tests catch missing concepts, evals verify that the model uses those concepts correctly when guided by the skill.

## Implications

The eval infrastructure already exists and is designed for exactly this problem. The missing piece is eval cases. Writing 5–8 eval cases targeting the core koto-user workflows (which don't exist yet as a skill) and the key koto-author concepts would give the highest return for the smallest effort. These cases should focus on behaviors most likely to drift: gate-blocked handling, evidence submission with `blocking_conditions`, `koto decisions record`, and `koto rewind`.

Structured coverage tests are worth adding for a small set of high-value, stable markers. The maintenance burden grows with the marker list, so the list should be kept short and cover only concepts where total omission would be a serious failure.

File-change heuristics add process overhead without reliable signal. Deferring or skipping this approach is reasonable.

## Surprises

The eval harness (`eval.sh`) is more complete and production-ready than expected — it handles API errors, truncates responses for display, has clear cost documentation, and supports both inline skill content and path references. It was clearly built with CI use in mind. The missing piece (no `evals/` directory) is a gap in test coverage, not a gap in infrastructure.

`eval-plugins.yml` only triggers on `plugins/**` changes. This means evals would not run on a PR that changes only `src/engine/advance.rs` without touching plugin files. If the skill accuracy check is the goal, the workflow's path filter would need to be expanded or a separate workflow added.

`validate-plugins.yml` already validates that every template in the skills directory compiles. This means structural template drift (invalid frontmatter, broken state references) is caught automatically. Semantic drift (wrong behavior described in prose) is not.

## Open Questions

1. Should LLM evals run on source-only PRs (not just plugin PRs)? Expanding the `eval-plugins.yml` path filter to include `src/**` would cause evals to run on every koto source change, catching regressions before the skill is updated — but at a cost increase and with no guarantee the evals will fail (model may compensate from training data).

2. What is the right set of coverage markers? A short list of 5–10 terms covering the most critical concepts (`blocking_conditions`, `koto decisions record`, `koto rewind`, `gates.*`, `--allow-legacy-gates`) would be defensible. Who decides when a concept is important enough to add a marker?

3. Who writes the eval cases? Eval cases capture expected behavior. They need to be written by someone who understands current koto behavior — meaning they're most accurately written right after implementing a feature. Should eval-case authorship be added to the PR checklist?

4. Is the "graceful skip" in `eval-plugins.yml` appropriate long-term? Skipping when `ANTHROPIC_API_KEY` is absent means the check is silently non-blocking for contributors who fork the repo. This is acceptable for open-source but means the check only runs in the main repo's CI.

## Summary

The koto repo already has an LLM-in-CI eval harness (`eval.sh` + `eval-plugins.yml`) with the Anthropic API key configured in secrets, but no eval cases have been written yet — the infrastructure is ready and unused. The highest-leverage action is writing 5–8 eval cases targeting core koto-user workflows (which don't exist as a skill) and key koto-author behaviors, since this approach catches semantic drift with low false-positive rate using existing infrastructure. The biggest open question is whether evals should also trigger on source-only PRs, which would expand coverage but require updating the path filter and accepting the cost of running evals on every engine change.
