# Lead: sibling drift checkers in koto and shirabe

## Findings

### 1. koto's `scripts/` is small, and only one script is a checker

Complete inventory of `scripts/` in koto (there are exactly five files):

| File | Size | What it is |
|---|---|---|
| `scripts/check-evals-exist.sh` | 2.1K | The only validating checker. CI-wired. |
| `scripts/run-evals.sh` | 15.7K | Operator tool. Spawns `claude -p` sessions. Not CI-wired. |
| `scripts/verify-native-workflows.sh` | 7.2K | End-to-end assertion script. **Not CI-wired** (see below). |
| `scripts/install-hooks.sh` | 262B | Symlinks `scripts/pre-commit` into `.git/hooks`. `#!/bin/sh`. |
| `scripts/pre-commit` | 513B | Runs `cargo fmt` on staged `.rs` files. `#!/bin/sh`. No `set -e`. |

There is no `scripts/lib/`, no `*_test.sh`, no allowlist file, and no `scripts/` README.

#### `check-evals-exist.sh` in detail

Header (`scripts/check-evals-exist.sh:1-13`):

```bash
#!/usr/bin/env bash
# check-evals-exist.sh - CI check: every skill must have at least 1 eval
#
# Usage: scripts/check-evals-exist.sh
# Exit code: 0 if all skills have evals, 1 if any are missing
#
# Skills are discovered from plugins/*/skills/*/.
# Skills with disable-model-invocation: true are exempt (reference-only skills).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
```

- **What it checks**: for every `plugins/*/skills/*/` with a `SKILL.md`, that `evals/evals.json` exists and parses to a non-empty `evals` array. The count is read by shelling to python3: `count=$(python3 -c "import json; print(len(json.load(open('$evals_file')).get('evals', [])))" 2>/dev/null || echo "0")` (line 47).
- **Invocation**: no arguments accepted at all. Paths are derived from `BASH_SOURCE`, so it can be run from anywhere but always scans the repo it lives in. There is no `--fix`, no `--list`, no path arguments, no environment overrides.
- **Failure output**: it accumulates into three arrays (`missing`, `exempt`, `passing`) and prints a three-section report **after** the whole scan — it does not fail fast. Format (lines 56-87):

```
=== Skill Eval Check ===

Passing:
  + koto-user (18 evals) [koto-skills]

Exempt (disable-model-invocation):
  ~ some-skill [koto-skills]

MISSING EVALS:
  ! koto-author [koto-skills] -- needs .../evals/evals.json

Every user-invocable skill must have at least 1 eval scenario.
```

Everything goes to **stdout**, not stderr. It does not use GitHub's `::error::` annotation syntax (the workflows that inline their checks do — see §3).
- **Exclusion mechanism**: a single in-band signal — `grep -q "disable-model-invocation: true" "$skill_md"` (line 34). No sidecar allowlist file. Exempt skills are printed, so the exemption is visible in the run output rather than silent. Note the grep is unanchored and reads the whole file, not just frontmatter.

#### `verify-native-workflows.sh` in detail

Different shape and worth studying because it is the repo's only *behavioral* assertion script. `set -euo pipefail`, takes an optional binary path argument (`scripts/verify-native-workflows.sh [path-to-koto-binary]`), defaults to `./target/debug/koto` then `./target/release/koto`. It defines the pair:

```bash
fail() { echo "FAIL: $1" >&2; exit 1; }
pass() { echo "PASS: $1"; }
```

Note: `fail()` **exits immediately** — this script is fail-fast, unlike `check-evals-exist.sh`. Findings are labelled against acceptance criteria (`AC1:`, `AC2:`, `AC3:`, `render#1:`) and it ends with `echo "ALL CHECKS PASSED: ..."`. It shells to `python3 -c` for every JSON read (lines 57-64).

**It is not run by any workflow.** `grep -rn "scripts/" .github/` returns exactly one hit repo-wide:

```
.github/workflows/eval-plugins.yml:19:        run: bash scripts/check-evals-exist.sh
```

`verify-native-workflows.sh` is referenced only from `docs/guides/native-workflows-verification.md`, `docs/designs/current/DESIGN-native-workflows-phase-detail.md`, and `plugins/koto-skills/skills/koto-user/SKILL.md`. Its own header says the live-TUI half "is documented as a manual procedure ... since CI cannot drive the TUI" — but the CI-runnable half is also not in CI.

### 2. shirabe's checker family is much larger and is the real house style

`scripts/` in shirabe: 11 `check-*.sh` / `validate-*.sh` checkers, 9 `*_test.sh` companions, one `.allow` file, and a `scripts/lib/` with shared readers. Every checker relevant here:

| Script | Checks | Exclusion mechanism |
|---|---|---|
| `check-skill-injection.sh` | `!`-at-column-0 injected commands in SKILL.md satisfy three rules (exit-0 guard, allowed-tools coverage, no redirection) | none — hard-coded `ALWAYS_ZERO` / `ALLOWED_REDIRECTS` word lists, deliberately short |
| `check-template-interpolation.sh` | koto gate `command:`/`working_dir:` fields contain no `$NAME`/`${NAME}` | none; three forms structurally excluded (`{{KEY}}`, `${evidence.*}`, `$(...)`) |
| `check-template-directives.sh` | two koto authoring shapes the compiler accepts and the engine punishes silently | **`check-template-directives.allow`** — tab-separated, issue-reference required |
| `validate-template-mermaid.sh` | 4 checks: state parity vs `.mermaid.md`, `default_template:` refs resolve, no hardcoded workflow name, gate commands agree across templates | none; check 1 skipped when no `.mermaid.md` companion |
| `check-skill-requires.sh` | 8 checks over `skills/*/requires.tsv`, incl. flag parity against extracted call sites | **in-band comment records**: `#not-a-call-site<TAB>PATH<TAB>TOOL<TAB>FLAG<TAB>REASON` |
| `check-tool-diagnostic-discards.sh` | every discarded tool diagnostic is enumerated in `references/tool-diagnostic-discards.md` | the reference doc *is* the enumeration; fails in **both** directions |
| `check-no-duplicate-rule-list.sh` | writing-style rule terms aren't copied out of `rules.yaml` | `EXEMPT_PREFIXES` tuple hard-coded in the embedded python |
| `check-no-fixture-design-leak.sh` | eval-fixture DESIGNs don't land in `docs/designs/current/` | none |
| `check-sentinel.sh` | plugin manifest versions carry a `-dev` suffix | suffix configurable by `$1` or `DEV_SUFFIX` |
| `check-bash-floor.sh` | runs named suites under bash 3.2 (docker or system backend) | suite registry with documented exemptions |

Two exclusion designs stand out and are directly relevant:

**(a) Allowlist with a mandatory ticket.** `scripts/check-template-directives.allow:1-23`:

```
# Known findings for scripts/check-template-directives.sh, deferred rather than
# suppressed. Each record names the issue that will close it.
#
# Format, tab-separated, five fields:
#
#   <rule>  <template>  <subject>  <issue>  <reason>
#
#   issue     owner/repo#N -- required; a record without one is an error
```

and the script's own header explains why the allowlist exists at all:

> "Why these four are deferred and not fixed in the same change that introduces the check: all four are behaviour changes to shipped skills. ... a lint that cannot land until the defects it finds are fixed does not land."

**(b) Bidirectional enumeration.** `check-tool-diagnostic-discards.sh` and `check-skill-requires.sh` both fail when an allowlist/exemption entry no longer matches anything. From `check-skill-requires.sh:88-90`:

> "An exemption matching no extracted flag fails as stale, in the same both-directions spirit as the discard enumeration: the list cannot rot into a permanent allowlist."

That is the strongest available answer to "how do you stop the allowlist becoming a graveyard", and it is a stated house principle, not an accident.

#### The "why not put it in the Rust validator" precedent

`check-template-interpolation.sh:30-33` argues explicitly for the shell-script-plus-workflow shape over folding a check into `shirabe validate`:

```
# This lives as a grep in a workflow rather than as a check inside
# `shirabe validate` on purpose. It is a statement about this repository's own
# file layout, not about an artifact schema the validator owns, and folding it
# in would put a Rust change and a release in front of every adjustment to it.
```

This is directly load-bearing for our decision: a doc/code-drift check is a statement about *this repository's own file layout*, not about koto's engine semantics, so the same argument says shell script, not `koto` subcommand.

### 3. House style, concretely

Distilled from all 16 scripts read across both repos.

**Shebang and options.** `#!/usr/bin/env bash` universally (only koto's two git-hook scripts use `#!/bin/sh`). `set -euo pipefail` is the default; two deliberate exceptions, both documented in place:
- `check-skill-requires.sh:113` uses `set -uo pipefail` (no `-e`) — it must survive a failing sub-check to report all findings.
- `koto/scripts/run-evals.sh:24`: `# Note: no set -e; we handle errors explicitly for --all resilience`.

**Path resolution.** The identical two lines open nearly every script:

```bash
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
```

**Comment header.** This is the most distinctive part of the style and it is not optional. Every checker opens with a long prose block that states, in order: *what fails*, *the concrete incident that motivated it*, *what is deliberately NOT flagged and why*, *usage*, and *exit codes*. Some run to 100+ lines (`check-skill-requires.sh` header is 110 lines; `check-template-directives.sh` is 105). They name real defects — "That is exactly how the `worktree_discipline_check` gate came to test `wip/work-on__impact.json` for every plan", "Two live defects in `skills/inflight/SKILL.md` came from that, both now fixed". A checker written without this header would read as foreign.

**Output format.** Two conventions coexist:
- `FAIL:` / `PASS:` line prefixes — `check-sentinel.sh`, `check-no-fixture-design-leak.sh`, `check-skill-injection.sh`, `check-template-interpolation.sh`, `verify-native-workflows.sh`. This matches the workspace `bash-development` skill's stated rule: *"Output `PASS:` or `FAIL:` prefix on result lines"*.
- `ERROR:` / `WARNING:` prefixes — `validate-template-mermaid.sh` only.

The `+ / ~ / !` bullet style in koto's `check-evals-exist.sh` is a one-off; nothing else uses it.

**Multiple failures.** Accumulate into an `errors` (or `ERRORS`) counter, print every finding, then exit once at the end with a count line. Universal, except `verify-native-workflows.sh` which is fail-fast. Typical tail:

```bash
if [[ $errors -gt 0 ]]; then
  echo ""
  echo "check-template-interpolation: $errors offending field(s)"
  exit 1
fi
echo "check-template-interpolation: OK"
exit 0
```

The success line names the script (`check-skill-injection: OK (12 SKILL.md scanned, 3 injected line(s))`) and reports coverage counts, so a check that silently scanned nothing is visible.

**Findings go to stderr** in the newer shirabe checkers (`report() { echo "FAIL: $1" >&2; ... }` in `check-skill-injection.sh:244`); older ones and koto's use stdout. Mixed.

**Every finding carries a fix.** Not just the violation — the remediation. `check-template-interpolation.sh:100-103`:

```
  echo "  Fix: declare the variable in the template's 'variables:' block and"
  echo "  reference it as {{NAME}}, passing it with --var at koto init."
```

**Exit codes.** `0` clean, `1` findings, `2` usage error (a named path does not exist). The `2` convention appears in `check-skill-injection.sh`, `check-skill-requires.sh`, `check-bash-floor.sh`.

**Arguments.** Checkers take optional path arguments defaulting to a repo-relative root — `check-skill-injection.sh [PATH...]`, `validate-template-mermaid.sh [file ...]`, `check-template-interpolation.sh [template-glob-root]`. This is what makes them testable. Environment overrides exist for the allowlist path (`TEMPLATE_DIRECTIVES_ALLOWLIST`, "tests use it").

**No `--fix` mode anywhere.** Zero of the 11 shirabe checkers and neither koto checker offers auto-fix. `--list` exists only on `check-bash-floor.sh` (lists suites) and `run-evals.sh` (lists skills). The house pattern is: report, name the fix in prose, let a human apply it.

**bash 3.2 floor** (shirabe only): "no associative arrays, no namerefs, no mapfile. macOS ships 3.2.57 as `/bin/bash`". koto has no such constraint stated and no macOS leg.

**No shellcheck.** `grep -rn "shellcheck\|shfmt"` over both repos' `.github/` returns nothing. The workspace `bash-development` skill acknowledges this: *"CI doesn't run shellcheck currently, but scripts should pass it anyway."*

**python3 is freely used.** Both repos shell out to `python3 -c` for JSON, and `check-no-duplicate-rule-list.sh` is a bash wrapper around an `exec python3 - <<'PY'` heredoc. So a check needing real parsing is not forced into awk.

### 4. Testing: shirabe has a harness, koto has none

**koto has no bash test harness of any kind.** `find` for `*_test.sh`, `*.bats`, `bats`, `shunit*` under koto returns zero results. `scripts/` in koto is untested — `check-evals-exist.sh` has no test, and neither does anything else. The Rust integration tests under `tests/` (42 files, incl. a 261K `integration_test.rs`) shell out to the built binary but never to `scripts/`.

**shirabe's harness is a hand-rolled convention, not a framework.** Nine `*_test.sh` files sitting beside the script they test, same directory, same `_test.sh` suffix. The shape (from `check-template-interpolation_test.sh`):

```bash
#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECK_SCRIPT="$SCRIPT_DIR/check-template-interpolation.sh"
TEST_DIR=""
PASS_COUNT=0
FAIL_COUNT=0

setup()    { TEST_DIR=$(mktemp -d); mkdir -p "$TEST_DIR/skills/demo/koto-templates"; }
teardown() { rm -rf "$TEST_DIR"; }
fail()     { echo "FAIL: $1 - $2" >&2; FAIL_COUNT=$((FAIL_COUNT + 1)); }
pass()     { echo "PASS: $1" >&2; PASS_COUNT=$((PASS_COUNT + 1)); }
```

Each test is a `test_<name>()` function with a one-line prose comment above it naming the case (`# The defect this check exists for: ${NAME} in a gate command.`), building a fixture in `$TEST_DIR` via a heredoc, running the check against `$TEST_DIR`, and asserting the exit status. Tests are invoked by a bare list of calls at the bottom, then:

```bash
echo "Results: $PASS_COUNT passed, $FAIL_COUNT failed" >&2
if [[ $FAIL_COUNT -gt 0 ]]; then exit 1; fi
exit 0
```

No bats, no shunit2, no discovery — the function list at the bottom is the registry. This is why the checker must accept a path argument.

**Two shirabe checks run the harness *instead of* the check in CI**, because the harness runs the check against the real tree as its first case:

```yaml
# check-skill-injection.yml:20-23
# The test harness runs the check against the tree as its first case, so this
# one step covers both the scan and its own regression suite.
- name: Check injected SKILL.md commands
  run: bash scripts/check-skill-injection_test.sh
```

Others run both steps (`check-templates.yml:39-43`: run the tests, then run the check).

### 5. CI wiring in koto

Eleven workflow files. Full accounting:

| Workflow | Trigger | Jobs | Runs a `scripts/` script? |
|---|---|---|---|
| `validate.yml` | push main; PR (opened/sync/reopened/ready/draft) | check-artifacts, unit-tests, stability-tests, fmt, clippy, audit, coverage, tsuku-distributed-install, cloud-integration, + `validate` aggregator | **No** |
| `eval-plugins.yml` | PR, `paths: plugins/**` | eval-coverage, no-hooks, + `eval-plugins` aggregator | **Yes** — `bash scripts/check-evals-exist.sh` |
| `validate-plugins.yml` | PR, `paths: plugins/**`, `.claude-plugin/**` | template-compilation, hook-smoke-test, schema-validation, + aggregator | No (inline bash) |
| `validate-docs.yml` | PR, `paths: docs/**` | delegates to `tsukumogami/shirabe/.github/workflows/validate-docs.yml@main` | No |
| `lifecycle.yml` | PR (no paths filter) | delegates to shirabe `lifecycle.yml@main` | No |
| `validate-pr-body.yml` | PR (no paths filter) | delegates to shirabe `pr-body.yml@main` | No |
| `check-template-freshness.yml` | `workflow_call` only | reusable; koto **publishes** it, does not call it on itself | No |
| `benches.yml` | nightly cron + push main | discovery-bench, recursion-caps-bench | No |
| `release.yml`, `prepare-release.yml`, `finalize.yml` | release plumbing | — | No |

Key structural facts for our decision:

1. **There is no `docs` job and no `lint` job in koto's own CI.** `validate.yml` is Rust-toolchain-only (fmt/clippy/audit/test/coverage). `validate-docs.yml` is a two-line delegation to shirabe's per-file *format* validator (frontmatter, sections, issues-table rows) and gates only on `paths: docs/**`. Neither is a natural home for a content check that must run when *source* changes.

2. **The aggregator pattern is mandatory-looking.** Three workflows end in a job named after the workflow that `needs:` all the others with `if: always()` and re-checks each `.result`. A new job inside `validate.yml` would have to be added to both the `needs:` list and the `if` chain (lines 189, 195-202) or it would not gate anything.

3. **The `paths:` filter is the trap.** `eval-plugins.yml` fires only on `paths: plugins/**`. The `koto session rebind` defect was a change to `src/cli/mod.rs` — it would not have triggered `eval-plugins.yml`, `validate-plugins.yml`, or `validate-docs.yml`. A doc/code-drift check must have **no `paths:` filter**, or one that includes `src/**`. shirabe's own `check-templates.yml:8-15` shows awareness of exactly this and comments on why its filter is wide:

```yaml
      # The directives check reads the scripts a gate invokes, so a change to
      # one of those changes what the check sees without touching a template.
```

4. **Draft-PR exemption is near-universal**: `if: ${{ github.event.pull_request.draft != true }}` guards check-artifacts, cloud-integration, and every job in `validate-plugins.yml` and `eval-plugins.yml`. The Rust jobs (fmt/clippy/test) are not guarded.

5. **GitHub annotations.** Inline workflow bash uses `echo "::error::..."` and `echo "::error file=${output}::..."`. Extracted scripts do not — `check-evals-exist.sh` emits plain text. So a new script can use plain `FAIL:` lines and stay consistent.

### 6. Is an existing checker extensible enough to host this?

**No, and none is close.** Concretely:

- **`check-evals-exist.sh`** — scope is `plugins/*/skills/*/evals/evals.json` existence. It has no argument parsing at all (no `$1`, no `getopts`), no file-content scanning beyond one `grep -q` on `SKILL.md`, and no notion of the source tree. Adding a "does what the docs name exist in the code" mode would mean a new scanner, a new reporter, and a new exclusion mechanism sharing nothing with the existing 87 lines but `REPO_ROOT`. Its CI home (`eval-plugins.yml`, `paths: plugins/**`) is also the wrong trigger — see §5.3.

- **`verify-native-workflows.sh`** — a behavioral end-to-end driver that builds sessions and asserts on emitted JSON. Different category entirely, and it isn't in CI.

- **shirabe's `check-skill-requires.sh`** is the closest *conceptual* sibling anywhere: its PARITY check already extracts `koto <verb> --flag` from command lines inside skill files and demands each flag be declared. It even carries the extractor narrowings we'd need (skip `evals/`, skip `*_test.sh`, require Markdown commands be in a code span, skip `#` comment lines in `.sh`). But it lives in shirabe, validates shirabe's `requires.tsv` sidecars against `scripts/lib/tool-routes.tsv`, and checks *declaration completeness*, not *existence in koto's source*. Its extractor is worth copying; the script is not worth extending across a repo boundary.

- **`validate-template-mermaid.sh`** is the only existing checker with a "documented reference resolves to a real file" check (check 2, `default_template:` refs). That is the same *shape* as our problem — a name in a doc must resolve to something on disk — but it is scoped to koto templates, lives in shirabe, and its check 2 is 17 lines. It is a precedent, not a host.

The stronger argument for a new script is the one the house already wrote down: `check-template-interpolation.sh:30-33` says a check that is "a statement about this repository's own file layout" belongs in a workflow-invoked script rather than inside the compiled validator, because folding it in "would put a Rust change and a release in front of every adjustment to it". The drift check will need frequent adjustment to its exclusion rules (that is the whole lesson of the `#not-a-call-site` machinery). It should be a new `scripts/check-*.sh` in **koto**, with its own workflow and no `paths:` filter, plus a `*_test.sh` companion — which would be koto's first, establishing the harness shirabe already has.

### 7. koto's CLAUDE.md documentation-sync rules — quoted exactly

Yes, and they are entirely manual. `CLAUDE.md:49-73`:

> ## koto-skills Plugin Maintenance
>
> Two skills in `plugins/koto-skills/skills/` guide agents authoring and running koto-backed workflows. They drift silently when koto changes without a corresponding skill update.
>
> | Skill | Path | Scope |
> |-------|------|-------|
> | `koto-author` | `plugins/koto-skills/skills/koto-author/` | Guides agents writing koto templates |
> | `koto-user` | `plugins/koto-skills/skills/koto-user/` | Guides agents running koto-backed workflows |
>
> **After completing any source change in `src/` or `cmd/`, assess both skills before closing the work:**
>
> 1. **Broken contracts** -- read the diff and each skill, then ask: does anything the skill currently documents no longer match the code? Look for changed flag names, renamed fields, removed subcommands, altered response shapes, or behavior that works differently than described.
>
> 2. **New surface** -- ask: does this change add CLI flags, subcommands, response fields, gate types, or behavior that neither skill mentions? New surface that agents will encounter belongs in the relevant skill.
>
> If either question surfaces gaps, update the skill in the same PR. A separate skill-update PR is acceptable only when the scope is large enough to warrant it -- document the gap in the PR description so it isn't lost.
>
> Source areas most likely to require skill updates:
>
> | Area | Relevant skill |
> |------|---------------|
> | `src/cli/` -- subcommands, flags, JSON output types | both |
> | `src/engine/` -- advance loop, action values, response schema | koto-user |
> | `src/gate/` -- gate types, structured output fields | both |
> | `src/template/` -- frontmatter fields, compiler errors/warnings | koto-author |

And `CLAUDE.md:105`:

> CI enforces that every skill has at least one eval (`check-evals-exist.sh`). Running the evals themselves is manual -- they require an Anthropic API key and spawn Claude sessions.

The rule literally says "removed subcommands" is a thing to look for. It did not catch `koto session rebind`. That is the case for mechanization in one sentence.

## Implications

1. **The new check should be a new script in koto, not a mode on an existing one.** Nothing in koto is extensible; the closest thing anywhere is in the wrong repo. And the house has already written the argument for a script over a compiled-in check (`check-template-interpolation.sh:30-33`).

2. **It needs its own workflow with no `paths:` filter.** Every existing koto workflow that runs a check is gated on `plugins/**` or `docs/**`. The motivating defect was a `src/cli/` change. A `paths:`-filtered check would have missed it. shirabe's `check-templates.yml` and `check-tool-diagnostic-discards.yml` both use wide filters and comment on why — follow that.

3. **It will be koto's first tested shell script.** Copy shirabe's `*_test.sh` convention verbatim: same directory, `_test.sh` suffix, `mktemp -d` fixtures, `test_<name>()` functions listed at the bottom, `PASS_COUNT`/`FAIL_COUNT`, `Results: N passed, M failed`. That in turn requires the checker to accept a path/root argument, which every shirabe checker does and `check-evals-exist.sh` does not.

4. **The exclusion mechanism is the hard design decision, and there are two proven answers.** A tab-separated `.allow` file with a *mandatory* `owner/repo#N` field (`check-template-directives.allow`), or in-band comment records inside the declaration being checked (`check-skill-requires.sh`'s `#not-a-call-site`). Both fail on stale entries. The `.allow` file's header states the reason a lint needs one at all: "a lint that cannot land until the defects it finds are fixed does not land." Given the five known instances, we will need one.

5. **A doc naming something that does not exist is sometimes correct.** koto's `command-reference.md:696` is literally titled `## koto session rebind — not implemented` and explains the workaround. Whatever the check greps for, that section must pass — which argues for scoping the check to *affirmative* claims (a command shown in a fenced block, a path in a tree diagram) rather than any mention of a verb.

6. **The check's scope crosses a repo boundary.** `koto/CLAUDE.local.md` is generated from `dot-niwa`'s `repos/koto.md` (per the workspace CLAUDE.md: "A repo's `CLAUDE.local.md` is generated — edit its source in `dot-niwa`, not the generated file"). Its content is badly drifted (see Surprises). A check inside koto can *detect* the drift but cannot fix the source, and koto's `.gitignore` status for that file needs confirming before the check treats it as in scope.

## Surprises

1. **`koto/CLAUDE.local.md` describes koto as a Go project.** koto is Rust. The file documents:

   ```bash
   go build -o koto ./cmd/koto
   go test ./...
   go install ./cmd/koto
   go vet ./...
   ```

   and a tree with `internal/`, `pkg/cache/`, `pkg/controller/`, `pkg/discover/`, `pkg/engine/`, `pkg/template/`. None of those directories exist; the repo has `src/` with `Cargo.toml`. It also lists a Key Commands table containing **`koto transition <state>`** and **`koto query`** — neither is a subcommand. The real top-level set from `src/cli/mod.rs:88` (`pub enum Command`) is: `Version, Init, Next, Cancel, Rewind, Workflows, Template, Session, Context, Status, Decisions, Overrides, Config, Workspace, Request, Dashboard`. This is a sixth and seventh drift instance, larger than any in the stated five, and it lives in a *generated* file whose source is in another repo.

2. **`koto/CLAUDE.md` itself is drifted** — the file the exploration is reading for guidance. Line 11 documents `cmd/koto/        # CLI entry point`; there is no `cmd/` directory. Line 15 documents `│   ├── gate/        # Gate evaluators`; `src/gate.rs` is a 36K file, not a directory. And the maintenance rule at line 58 says "any source change in `src/` or **`cmd/`**" — instructing agents to watch a directory that does not exist.

3. **`CLAUDE.md`'s skill table is incomplete.** It names two skills (`koto-author`, `koto-user`) and says "Two skills in `plugins/koto-skills/skills/`". There are three — `koto-adhoc` also exists, with its own `evals/evals.json` and fixtures. `check-evals-exist.sh` globs `plugins/*/skills/*/` so it *does* cover koto-adhoc; the prose does not. The maintenance rule therefore instructs agents to "assess both skills" while a third goes unassessed.

4. **`verify-native-workflows.sh` was written to be CI-runnable and is not in CI.** Its own header says it "is the CI/CLI-runnable proof of the properties", distinguishing itself from the manual TUI check. No workflow invokes it. So koto has two checkers and only one is wired.

5. **koto publishes a reusable freshness workflow it does not run on itself.** `check-template-freshness.yml` is `workflow_call`-only; shirabe calls it (`check-templates.yml:25`) against shirabe's templates. koto's own `plugins/koto-skills/skills/*/koto-templates/*.mermaid.md` companions are not freshness-checked by koto's CI — `validate-plugins.yml` compiles templates but skips `*.mermaid.md` (line 31).

6. **The reusable workflow pins a koto version in its own default**: `check-template-freshness.yml:18` defaults `koto-version` to `'v0.12.0'` — the release that shipped the `rebind` error message. That default is itself a drift surface nothing checks.

7. **`plugins/koto-skills/skills/koto-user/references/command-reference.md` already contains a hand-written drift record.** Section `## koto session rebind — not implemented` (line 696) states "**The subcommand does not exist.**" and lists the real `koto session` verbs — `start`, `dir`, `list`, `cleanup`, `recover`, `resolve`, `update`, which matches `SessionCommand` in `src/cli/mod.rs:387` exactly. Meanwhile `src/cli/mod.rs:3473`, `src/cli/mod.rs:3489`, and `src/cli/next_types.rs:179` still emit `koto session rebind` to users. The documentation was corrected; the code was not. So the *doc* is right and the *error string* is the drift — which inverts the usual direction and means a check that only reads docs would find nothing here.

8. **No shellcheck in either repo's CI**, despite the workspace `bash-development` skill saying scripts "should pass it anyway".

## Open Questions

1. Is `CLAUDE.local.md` gitignored in koto? It is generated by niwa from `dot-niwa`. If it is not committed, it is out of scope for a CI check; if it is, the check would need to flag content whose source lives in another repo and cannot be fixed in the same PR — a case the allowlist would have to carry permanently, which the both-directions principle disallows.
2. What is the authoritative extraction of koto's CLI surface for the check to compare against? Options: parse `src/cli/mod.rs`'s `Command`/`SessionCommand`/etc. enums with awk (brittle to a clap refactor), or run `koto <verb> --help` against a built binary (requires a build in the job, which `validate-plugins.yml` already does — `cargo build --release`, and `check-template-freshness.yml` installs a release binary instead).
3. Does the check own the error-string direction? Finding #7 above is a drift instance where the code, not the doc, is wrong. Scanning `src/**` for `koto <verb>` strings inside user-facing messages is a different scan from scanning `docs/**` and `plugins/**` for commands. Deciding whether one script does both changes its shape substantially.
4. Which workflow does it join — a new `check-doc-drift.yml`, or a new job in `validate.yml`? The latter means editing the aggregator's `needs:` and `if` chain (`validate.yml:189, 195-202`), and inherits `validate.yml`'s no-`paths:`-filter trigger for free.
5. Are the five stated instances the full set? I found at least four more while reading (Go-vs-Rust in `CLAUDE.local.md`, `transition`/`query`, `cmd/` and `gate/` in `CLAUDE.md`, the two-vs-three skills count). Someone should enumerate before the check's rules are fixed, because the rules should be derived from the corpus.

## Summary

koto has exactly one CI-wired checker — `scripts/check-evals-exist.sh`, run only by `eval-plugins.yml` under `paths: plugins/**` — plus `verify-native-workflows.sh`, which was written to be CI-runnable and is not in any workflow; koto has no bash test harness at all and no docs or lint job that a new check would naturally join. The real house style lives in shirabe's eleven checkers: `#!/usr/bin/env bash` + `set -euo pipefail`, a long prose header naming the incident that motivated the check and what is deliberately not flagged, an optional path argument so the script is testable, accumulate-then-report with `FAIL:`/`PASS:` prefixes and a fix suggestion on every finding, exit 0/1/2, a `*_test.sh` sibling with `mktemp -d` fixtures and a `Results: N passed, M failed` tail, and — where deferral is needed — a tab-separated allowlist whose records require an `owner/repo#N` and which fails in both directions so it cannot rot. No existing checker in either repo is extensible enough to host this: `check-evals-exist.sh` takes no arguments and scans one JSON file, and the closest conceptual sibling (shirabe's `check-skill-requires.sh` flag-parity extractor) is in the wrong repo checking a different invariant, so the new check should be a new koto script with its own unfiltered workflow and koto's first `_test.sh`.
