# work-on extension: koto

koto's verification map for shirabe's `/work-on` definition-of-done gate. Schema:
`skills/work-on/references/verification-map.md` in the shirabe repo. Commands are the ones CI
runs (`.github/workflows/validate.yml`, `validate-plugins.yml`, `eval-plugins.yml`), plus two
repo checks PR CI does not run, so that no path is reported verified by commands that never
touch it. This file is `@`-imported on every `/work-on` run, so it stays short.

## Verification map

- `src/**`, `tests/**`, `test/functional/fixtures/**`, `benches/**`, `koto-stability-tests/**`,
  `build.rs`, `Cargo.toml`, `Cargo.lock` -> every default command below
- `plugins/**`, `.claude-plugin/**` -> all of:
  - `cargo test -- --test-threads=1` (`tests/doc_names.rs` scans `plugins/koto-skills`)
  - `bash scripts/check-evals-exist.sh`
  - `test -z "$(find plugins/*/skills/*/ -maxdepth 1 -name hooks.json)"`
  - `jq -e '.name and (.name|length>0) and .version and (.version|length>0) and .skills and (.skills|length>0)' plugins/koto-skills/.claude-plugin/plugin.json`
  - `jq -e '.name and (.name|length>0) and .owner and (.owner|length>0) and .plugins and (.plugins|length>0)' .claude-plugin/marketplace.json`
  - `cargo build --release && find plugins/koto-skills/skills -path '*/koto-templates/*.md' ! -name '*.mermaid.md' -type f -print0 | xargs -r -0 -n1 ./target/release/koto template compile`
- `test/functional/**` -> `make -C test/functional test-functional` (the Go feature suite; no CI job runs it)
- `benches/**` -> `cargo bench --no-run` (bench targets are `test = false`; CI only builds them nightly)

### Default verification command (when no map entry matches; all must pass)

- `cargo test -- --test-threads=1`
- `cargo test -p koto-stability-tests -- --test-threads=1`
- `cargo fmt --check`
- `cargo clippy -- -D warnings`
