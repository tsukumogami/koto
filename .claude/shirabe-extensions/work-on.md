# work-on extension: koto

koto's verification map for shirabe's `/work-on` definition-of-done gate. Schema:
`skills/work-on/references/verification-map.md` in the shirabe repo. The default runs only when
no entry matches any changed file. The plugin checks copy step bodies from `validate-plugins.yml`,
`eval-plugins.yml` and `run-evals.yml`, which point back here: change each pair together. Deliberate differences, and
checks PR CI does not run, are marked. Paths that carry behavior where no command here checks
what they do have their own entry at the end, which makes the gate cannot-verify rather than
letting the default pass them; this file is knowingly left to the default, since halting every
map edit would make the map painful to maintain. This file is `@`-imported on every `/work-on`
run, so it stays short; the reasons are in its commit history.

**The gate itself reads `verification-map.json` beside this file, not this list; change the two
together.** The JSON carries only the commands PR CI runs, plus the decider purity check on
source changes. It has no entry for `.github/**`, `scripts/**`, `test/functional/**`,
`.tsuku-recipes/**` or `benches/**`-only checks, so a change confined to those paths falls to its
default list and passes the gate without the checks this file names for them: run those by hand.

## Verification map

- `src/**`, `tests/**`, `test/functional/fixtures/**`, `benches/**`, `koto-stability-tests/**`,
  `build.rs`, `Cargo.toml`, `Cargo.lock`, `README.md`, `CLAUDE.md` -> every default command below
- `plugins/**`, `.claude-plugin/**`, `scripts/check-evals-exist.sh` -> all of:
  - `cargo test --test doc_names` (the only test that reads `plugins/`)
  - `bash scripts/check-evals-exist.sh`
  - `for d in $(find plugins/*/skills/*/ -maxdepth 0 -type d 2>/dev/null); do test ! -f "$d/hooks.json" || exit 1; done`
  - `P=plugins/koto-skills/.claude-plugin/plugin.json && test -f "$P" && jq -e ".name and (.name | length > 0)" "$P" >/dev/null && jq -e ".version and (.version | length > 0)" "$P" >/dev/null && jq -e ".skills and (.skills | length > 0)" "$P" >/dev/null`
  - `M=.claude-plugin/marketplace.json && test -f "$M" && jq -e ".name and (.name | length > 0)" "$M" >/dev/null && jq -e ".owner and (.owner | length > 0)" "$M" >/dev/null && jq -e ".plugins and (.plugins | length > 0)" "$M" >/dev/null`
  - `cargo build --release && K="${CARGO_TARGET_DIR:-target}/release/koto" && { test -x "$K" || { echo "no koto binary at $K" >&2; exit 1; }; } && T=$(find plugins/koto-skills/skills/ -path "*/koto-templates/*.md" ! -name "*.mermaid.md" -type f) && test -n "$T" && printf "%s\n" "$T" | xargs -r -n1 "$K" template compile` (unlike CI: fails on an empty list, so it can't pass having compiled nothing, and finds the binary through `CARGO_TARGET_DIR`)
  - `cargo build --release -q && ( K="${CARGO_TARGET_DIR:-target}/release" && { test -x "$K/koto" || { echo "no koto binary at $K/koto" >&2; exit 1; }; } && S=$(mktemp -d) && mkdir -p "$S/mock" && printf '{"schema_version":1,"workflow":"mock","template_hash":"abc123","created_at":"2025-01-01T00:00:00Z"}\n{"seq":1,"timestamp":"2025-01-01T00:00:00Z","type":"workflow_initialized","payload":{"template_path":"mock","variables":{}}}\n{"seq":2,"timestamp":"2025-01-01T00:00:00Z","type":"transitioned","payload":{"from":null,"to":"test-state","condition_type":"auto"}}\n' > "$S/mock/koto-mock.state.jsonl" && HOOK_CMD=$(jq -r ".hooks.Stop[0].command" plugins/koto-skills/hooks.json) && export PATH="$(cd "$K" && pwd):$PATH" && export KOTO_SESSIONS_BASE="$S" && eval "$HOOK_CMD" 2>/dev/null | grep -q "Active koto workflow detected" && export KOTO_SESSIONS_BASE=$(mktemp -d) && test -z "$(eval "$HOOK_CMD" 2>/dev/null || true)" )` (unlike CI, finds the binary through `CARGO_TARGET_DIR` and fails if none was built, so it can't test an installed koto)
- `docs/**` -> all of:
  - `cargo test --test doc_names` and `cargo test --lib shipped_spec` (the only tests that read `docs/`)
  - `B=$(git merge-base origin/main HEAD) && git diff --name-only --diff-filter=ACMR "$B" -- :/docs/ | grep -vE "(^|/)(evals|tests)/fixtures/" | xargs -r shirabe validate --visibility=public` (errors out when `origin/main` is missing: that is cannot-verify, not a failed change)
  - `shirabe validate --visibility=public --lifecycle . --mode=draft` (not `ready`: an in-flight `/execute` chain keeps its PLAN, which the ready posture rejects)
- `scripts/run-evals.sh`, `scripts/run-evals_test.sh`, `scripts/classify-eval-session.py` ->
  `scripts/run-evals_test.sh` (the `run-evals` workflow's step)
- `test/functional/**` -> `make -C test/functional test-functional` (not in PR CI: the Go feature suite)
- `benches/**`, `Cargo.toml`, `Cargo.lock` -> `cargo bench --no-run` (not in PR CI: `cargo test` does not build bench targets)
- `.tsuku-recipes/**` -> `tsuku validate .tsuku-recipes/koto.toml` (not in PR CI: CI's step calls `tsuku recipe validate`, a subcommand tsuku does not have, and skips; it checks structure only, so a download or checksum change passes it)
- `.github/workflows/validate.yml`, `.github/workflows/run-evals.yml` -> both of, with
  `actionlint`, `yq` and `jq` installed first (`tsuku install actionlint yq jq`; if any can't be
  installed, the outcome is cannot-verify):
  - `scripts/check-workflow-reservations.sh`. It exits 1, printing why, for each change a person
    must still approve, and exit 1 here means cannot-verify, as for the next entry, not failed.
    It flags a changed trigger. It flags any change to a job that uses secrets, compared whole,
    step bodies included. It flags an added or removed line naming `secrets`, `permissions:`,
    `pull_request_target`, `continue-on-error`, an `if:` with `false`, an `|| echo`, `true`, `:`,
    `exit` or `{` fallback, or a call to another workflow. It flags a job dropped from the
    `validate` job's `needs:`, a needed job whose result that job doesn't check, and an
    unreadable base. Adding an ordinary job, in `needs:` and the result check, passes. It doesn't
    judge other logic, such as a rewritten `if:` or a job without secrets that stops testing
    anything. The reviewer reads that in the section below.
  - `SHELLCHECK_OPTS=--severity=warning actionlint` (not run by CI). It lints every workflow's
    syntax, expressions and `run:` shell. It doesn't check what a workflow's logic does.

  A workflow qualifies for this entry when its `pull_request` trigger has no `paths:` filter, or
  one that lists its own file. Every pull request to `main` that changes it then runs the changed
  version in that PR's CI, which is what exercises its logic. A PR against another base runs
  neither workflow, and `validate.yml`'s `check-artifacts` and `cloud-integration` jobs skip drafts.
  `validate.yml` passes storage secrets to `cloud-integration`, and a same-repository branch's
  run uses the edited file, so the check keeps any change to that job with a person.
  `lifecycle.yml` and `validate-pr-body.yml` don't qualify: they call shirabe's reusable
  workflows at `@main`, so a PR's CI never runs changed logic for them. The PR's reviewer reads a
  `## Workflow changes` section in the reviewer part of the description (below `---`). It names
  each changed workflow and what its logic now does, and states any change to secrets,
  permissions or `needs:` (or "None"). A maintainer approves it before merge. A new workflow stays
  in the next entry until someone checks its `on:` block against this rule and adds it here. The
  check script itself is under `scripts/**`, so a change to it stays with a person too.
- `.github/**` (except `.github/pull_request_template.md` and the two workflows in the previous
  entry); `install.sh`; `scripts/**` (except `scripts/check-evals-exist.sh` and the three
  run-evals files above); `.release/**`; `.goreleaser.yaml`; `.cargo/**`;
  `plugins/koto-skills/hooks/*.sh`; `.claude/settings.json` -> no local check exists. Still run
  every other selected command; if one fails the outcome is failed, otherwise it is cannot-verify:
  no command here checks what these files do and most are not read by PR CI either, so the run
  stops for a person to check them. (shirabe's schema has no form for such an entry yet:
  tsukumogami/shirabe#373.)

### Default verification command (when no map entry matches; all must pass)

Four of the checks `.github/workflows/validate.yml` runs on every PR, which point back here;
change both together. Its other jobs (audit, coverage, the tsuku install, cloud integration,
leftover-artifact checks) are left out on purpose.

- `cargo test --locked -- --test-threads=1`
- `cargo test -p koto-stability-tests -- --test-threads=1`
- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
