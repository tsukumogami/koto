# Lead: CI pipeline changes

## Findings

### Current CI workflows

**validate.yml** (3 jobs, runs on PR and push):
1. `check-artifacts` — fails if `wip/` directory contains files (pre-merge gate)
2. `unit-tests` — `go test -short -race -coverprofile=coverage.out ./...` + codecov upload on push
3. `lint-tests` — runs Go test functions: `TestGolangCILint`, `TestGoFmt`, `TestGoModTidy`, `TestGoVet`, `TestGovulncheck`

Note: lint is invoked via Go test wrapper functions in `lint_test.go`, not as direct CLI calls. The Rust CI should use direct `cargo` commands instead of wrapping them in test functions.

**validate-plugins.yml** (3 jobs, on plugin path changes):
1. `template-compilation` — builds koto binary, runs `koto template compile` on all plugins/*/templates/*.md
2. `hook-smoke-test` — creates mock state, tests Stop hook
3. `schema-validation` — validates plugin.json and marketplace.json with jq

**release.yml** (on tags):
- GoReleaser cross-compiles for linux/darwin × amd64/arm64
- Produces checksums, publishes GitHub release

**eval-plugins.yml** (on plugin changes, requires API key):
- Builds koto, runs prompt evaluation script

### Go → Rust CI mapping

| Current Go step | Rust equivalent | Notes |
|----------------|-----------------|-------|
| `go build` | `cargo build` | |
| `go test -short -race ./...` | `cargo test` | Rust detects races in debug by default via borrow checker; no flag needed |
| `go test -coverprofile` | `cargo llvm-cov` or `cargo tarpaulin` | llvm-cov produces LCOV/HTML; integrates with codecov |
| `gofmt -l` | `cargo fmt --check` | Built-in; no external tool |
| `go vet ./...` | `cargo clippy --all-targets` | Clippy covers vet + more |
| `golangci-lint run` | `cargo clippy -- -D warnings` | Deny-warnings mode matches lint-as-error behavior |
| `govulncheck ./...` | `cargo audit` | Checks RustSec advisory DB; or `cargo-deny` for stricter policy |
| `go mod tidy -diff` | n/a (verify Cargo.lock committed) | Cargo.lock should be committed for binaries |

### Recommended Rust validate.yml structure

```yaml
jobs:
  check-artifacts:
    # unchanged — shell check for wip/ contents

  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo test --all-targets

  lint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --check
      - run: cargo clippy --all-targets -- -D warnings
      - run: cargo audit
```

`dtolnay/rust-toolchain` is the standard action for pinning Rust toolchain. `Swatinem/rust-cache` caches `~/.cargo` and `target/` for fast rebuilds.

### validate-plugins.yml impact

This workflow builds the koto binary and invokes `koto template compile`. After the Rust rewrite, the binary invocation stays identical — only the build step changes from `go build` to `cargo build --release`. The plugin validation logic is unaffected.

### release.yml impact

GoReleaser handles cross-compilation, binary naming, checksums, and GitHub release creation. Rust replacement options:
- **cargo-dist**: opinionated, handles cross-compilation + GitHub releases in one config; growing adoption
- **Manual**: `cargo build --release --target <triple>` per platform + artifact upload via `softprops/action-gh-release`

cargo-dist is the more maintainable path for a project of this size.

### Coverage

`cargo llvm-cov` is preferred over `cargo tarpaulin`:
- llvm-cov works on stable Rust; tarpaulin requires nightly or Linux-specific ptrace
- llvm-cov output integrates directly with codecov (LCOV format)
- Command: `cargo llvm-cov --lcov --output-path lcov.info`

### Rust version pinning

Pin to `stable` via `dtolnay/rust-toolchain@stable`. No need for nightly. No need to test multiple Rust versions unless koto becomes a library; for a binary, stable is sufficient.

## Implications

The validate.yml replacement is straightforward — 3 jobs become 3 jobs, step counts are similar, and the `check-artifacts` job is unchanged. The biggest work is replacing GoReleaser in release.yml; cargo-dist is the recommended path. Plugin validation workflows need only the build command updated.

## Surprises

- Lint tests in Go are wrapped in `go test` functions (`TestGolangCILint` calls `exec.Command("golangci-lint", ...)`). This is a Go-specific pattern with no Rust equivalent; Rust CI calls `cargo clippy` directly.
- `eval-plugins.yml` depends on the koto binary to run prompt evaluations — this workflow is entirely unaffected by the language change as long as the external CLI contract is preserved.
- cargo-dist is newer but already used by high-profile Rust projects (axum, tokio); it's a safe choice for release automation.
- Rust's borrow checker eliminates data races at compile time; no `-race` flag equivalent is needed in CI.

## Open Questions

- Should release CI build for Windows as well? GoReleaser currently only targets linux/darwin.
- cargo-dist vs. manual release: cargo-dist is simpler but opinionated about release format. Worth reviewing its output format before committing.
- Should clippy use `--deny warnings` in CI but allow warnings locally? Standard pattern is deny in CI via `-- -D warnings` flag, not in Cargo.toml.

## Summary

The Go CI maps cleanly to Rust equivalents: cargo test replaces go test, cargo fmt + clippy replaces gofmt + golangci-lint, and cargo audit replaces govulncheck. The validate-plugins.yml and eval-plugins.yml workflows need only their `go build` step replaced with `cargo build --release`; all koto invocations remain identical. The largest change is the release pipeline, where cargo-dist is the recommended GoReleaser replacement.
