# work-on extension: koto

koto's verification map for shirabe's `/work-on` definition-of-done gate. Schema:
`skills/work-on/references/verification-map.md` in the shirabe repo. The default runs only when
no entry matches any changed file. The plugin checks copy step bodies from `validate-plugins.yml`
and `eval-plugins.yml`, which point back here: change both together. Deliberate differences, and
checks PR CI does not run, are marked. Paths that carry behavior and that no command here
examines have their own entry at the end, which makes the gate cannot-verify rather than letting
the default pass them; `.claude/settings.json` and this file are knowingly left to the default,
since halting every map edit would make the map painful to maintain. This file is `@`-imported
on every `/work-on` run, so it stays short; the reasons are in its commit history.

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
- `test/functional/**` -> `make -C test/functional test-functional` (not in PR CI: the Go feature suite)
- `benches/**`, `Cargo.toml`, `Cargo.lock` -> `cargo bench --no-run` (not in PR CI: `cargo test` does not build bench targets)
- `.tsuku-recipes/**` -> `tsuku validate .tsuku-recipes/koto.toml` (not in PR CI: CI's step calls `tsuku recipe validate`, a subcommand tsuku does not have, and skips; it checks structure only, so a download or checksum change passes it)
- `.github/**` other than `.github/pull_request_template.md`, `install.sh`, `scripts/**` other than
  `scripts/check-evals-exist.sh`, `.release/**`, `.goreleaser.yaml`, `.cargo/**`,
  `plugins/koto-skills/hooks/*.sh` -> no local check exists. Still run every other selected
  command; if one fails the outcome is failed, otherwise it is cannot-verify: no command here
  reads these files and most are not read by PR CI either, so the run stops for a person to check
  them. (shirabe's schema has no form for such an entry yet: tsukumogami/shirabe#373.)

### Default verification command (when no map entry matches; all must pass)

- `cargo test -- --test-threads=1`
- `cargo test -p koto-stability-tests -- --test-threads=1`
- `cargo fmt --check`
- `cargo clippy -- -D warnings`
