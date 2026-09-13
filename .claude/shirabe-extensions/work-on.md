# work-on extension: koto

koto's verification map for shirabe's `/work-on` definition-of-done gate. Schema:
`skills/work-on/references/verification-map.md` in the shirabe repo. Every command below
is one CI already runs (`.github/workflows/validate.yml`, `validate-plugins.yml`,
`eval-plugins.yml`). This file is `@`-imported on every `/work-on` run, so it stays short.

## Verification map

- `src/**`, `tests/**`, `test/**`, `benches/**`, `koto-stability-tests/**`, `build.rs`,
  `Cargo.toml`, `Cargo.lock` -> every default command below
- `plugins/**` -> all of:
  - `cargo test -- --test-threads=1` (`tests/doc_names.rs` scans `plugins/koto-skills`)
  - `bash scripts/check-evals-exist.sh`
  - `cargo build --release && find plugins/koto-skills/skills -path '*/koto-templates/*.md' ! -name '*.mermaid.md' -type f -print0 | xargs -0 -n1 ./target/release/koto template compile`

### Default verification command (when no map entry matches; all must pass)

- `cargo test -- --test-threads=1`
- `cargo test -p koto-stability-tests -- --test-threads=1`
- `cargo fmt --check`
- `cargo clippy -- -D warnings`
