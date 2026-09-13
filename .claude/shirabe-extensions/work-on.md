# work-on extension: koto

koto's verification map for shirabe's `/work-on` definition-of-done gate. Schema:
`skills/work-on/references/verification-map.md` in the shirabe repo. The default runs only when
no entry matches any changed file. The plugin checks mirror step bodies in `validate-plugins.yml`
and `eval-plugins.yml`, which point back here: change both together. Two checks PR CI does not
run are marked. No entry covers `scripts/` (bar the evals check), `install.sh`, `.github/` or
release config, and the SessionStart hook script is only scanned for names. This file is
`@`-imported on every `/work-on` run, so it stays short; the reasons are in its commit history.

## Verification map

- `src/**`, `tests/**`, `test/functional/fixtures/**`, `benches/**`, `koto-stability-tests/**`,
  `build.rs`, `Cargo.toml`, `Cargo.lock`, `README.md`, `CLAUDE.md` -> every default command below
- `plugins/**`, `.claude-plugin/**`, `scripts/check-evals-exist.sh` -> all of:
  - `cargo test --test doc_names` (the only test that reads `plugins/`)
  - `bash scripts/check-evals-exist.sh`
  - `for d in $(find plugins/*/skills/*/ -maxdepth 0 -type d 2>/dev/null); do test ! -f "$d/hooks.json" || exit 1; done`
  - `P=plugins/koto-skills/.claude-plugin/plugin.json && test -f "$P" && jq -e ".name and (.name | length > 0)" "$P" >/dev/null && jq -e ".version and (.version | length > 0)" "$P" >/dev/null && jq -e ".skills and (.skills | length > 0)" "$P" >/dev/null`
  - `M=.claude-plugin/marketplace.json && test -f "$M" && jq -e ".name and (.name | length > 0)" "$M" >/dev/null && jq -e ".owner and (.owner | length > 0)" "$M" >/dev/null && jq -e ".plugins and (.plugins | length > 0)" "$M" >/dev/null`
  - `cargo build --release && T=$(find plugins/koto-skills/skills/ -path "*/koto-templates/*.md" ! -name "*.mermaid.md" -type f) && test -n "$T" && printf "%s\n" "$T" | xargs -r -n1 ./target/release/koto template compile` (unlike CI, fails on an empty list, so it can't pass having compiled nothing)
  - `cargo build --release -q && ( S=$(mktemp -d) && mkdir -p "$S/mock" && printf '{"schema_version":1,"workflow":"mock","template_hash":"abc123","created_at":"2025-01-01T00:00:00Z"}\n{"seq":1,"timestamp":"2025-01-01T00:00:00Z","type":"workflow_initialized","payload":{"template_path":"mock","variables":{}}}\n{"seq":2,"timestamp":"2025-01-01T00:00:00Z","type":"transitioned","payload":{"from":null,"to":"test-state","condition_type":"auto"}}\n' > "$S/mock/koto-mock.state.jsonl" && HOOK_CMD=$(jq -r ".hooks.Stop[0].command" plugins/koto-skills/hooks.json) && export PATH="$PWD/target/release:$PATH" && export KOTO_SESSIONS_BASE="$S" && eval "$HOOK_CMD" 2>/dev/null | grep -q "Active koto workflow detected" && export KOTO_SESSIONS_BASE=$(mktemp -d) && test -z "$(eval "$HOOK_CMD" 2>/dev/null || true)" )`
- `docs/**` -> all of:
  - `cargo test --test doc_names` and `cargo test --lib shipped_spec` (the only tests that read `docs/`)
  - `B=$(git merge-base origin/main HEAD) && git diff --name-only --diff-filter=ACMR "$B" -- :/docs/ | grep -vE "(^|/)(evals|tests)/fixtures/" | xargs -r shirabe validate --visibility=public` (errors out when `origin/main` is missing: that is cannot-verify, not a failed change)
  - `shirabe validate --visibility=public --lifecycle . --mode=draft` (not `ready`: an in-flight `/execute` chain keeps its PLAN, which the ready posture rejects)
- `test/functional/**` -> `make -C test/functional test-functional` (not in PR CI: the Go feature suite)
- `benches/**`, `Cargo.toml`, `Cargo.lock` -> `cargo bench --no-run` (not in PR CI: `cargo test` does not build bench targets)

### Default verification command (when no map entry matches; all must pass)

- `cargo test -- --test-threads=1`
- `cargo test -p koto-stability-tests -- --test-threads=1`
- `cargo fmt --check`
- `cargo clippy -- -D warnings`
