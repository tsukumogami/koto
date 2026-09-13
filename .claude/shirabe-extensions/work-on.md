# work-on extension: koto

koto's verification map for shirabe's `/work-on` definition-of-done gate. Schema:
`skills/work-on/references/verification-map.md` in the shirabe repo. Commands are the ones PR CI
runs (`validate.yml`, `validate-plugins.yml`, `eval-plugins.yml`, `validate-docs.yml`,
`lifecycle.yml`), plus two repo checks it does not (marked). Paths no entry names get the
default, which checks Rust code only: `scripts/`, `install.sh`, `.github/`, release config and the
SessionStart hook script, among others, are not checked. CI builds `shirabe` from its main branch;
the docs commands use the installed one. This file is `@`-imported on every `/work-on` run, so it
stays short.

## Verification map

- `src/**`, `tests/**`, `test/functional/fixtures/**`, `benches/**`, `koto-stability-tests/**`,
  `build.rs`, `Cargo.toml`, `Cargo.lock` -> every default command below
- `plugins/**`, `.claude-plugin/**`, `scripts/check-evals-exist.sh` -> all of:
  - `cargo test -- --test-threads=1` (`tests/doc_names.rs` scans `plugins/koto-skills`)
  - `bash scripts/check-evals-exist.sh`
  - `test -z "$(find plugins/*/skills/*/ -maxdepth 1 -name hooks.json)"`
  - `jq -e '.name and (.name|length>0) and .version and (.version|length>0) and .skills and (.skills|length>0)' plugins/koto-skills/.claude-plugin/plugin.json`
  - `jq -e '.name and (.name|length>0) and .owner and (.owner|length>0) and .plugins and (.plugins|length>0)' .claude-plugin/marketplace.json`
  - `cargo build --release && find plugins/koto-skills/skills -path '*/koto-templates/*.md' ! -name '*.mermaid.md' -type f -print0 | xargs -r -0 -n1 ./target/release/koto template compile`
- `plugins/koto-skills/hooks.json` -> the Stop-hook smoke test: `cargo build --release -q && ( S=$(mktemp -d) && H=$(jq -r ".hooks.Stop[0].command" plugins/koto-skills/hooks.json) && export PATH="$PWD/target/release:$PATH" KOTO_SESSIONS_BASE="$S" && test -z "$(bash -c "$H")" && koto init mock --template test/functional/fixtures/templates/multi-state.md >/dev/null && bash -c "$H" | grep -q "Active koto workflow detected" )`
- `docs/**` -> all of:
  - `cargo test -- --test-threads=1` (`tests/doc_names.rs` scans `docs/guides`, `docs/reference`, `docs/testing`)
  - `B=$(git merge-base origin/main HEAD) && git diff --name-only --diff-filter=ACMR "$B" -- docs/ | grep -vE "(^|/)(evals|tests)/fixtures/" | xargs -r shirabe validate --visibility=public` (fails if `origin/main` is missing rather than validating nothing)
  - `shirabe validate --lifecycle . --mode=ready`
- `test/functional/**` -> `make -C test/functional test-functional` (not in PR CI: the Go feature suite)
- `benches/**` -> `cargo bench --no-run` (not in PR CI: bench targets are `test = false`)

### Default verification command (when no map entry matches; all must pass)

- `cargo test -- --test-threads=1`
- `cargo test -p koto-stability-tests -- --test-threads=1`
- `cargo fmt --check`
- `cargo clippy -- -D warnings`
