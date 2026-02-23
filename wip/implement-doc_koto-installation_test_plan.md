# Test Plan: koto-installation

Generated from: docs/designs/DESIGN-koto-installation.md
Issues covered: 4
Total scenarios: 14

---

## Scenario 1: GoReleaser config file exists and parses correctly
**ID**: [x] scenario-1
**Testable after**: #25
**Category**: infrastructure
**Commands**:
- `test -f .goreleaser.yaml`
- `grep -q 'version: 2' .goreleaser.yaml`
- `goreleaser check` (if goreleaser available)
**Expected**: `.goreleaser.yaml` exists at the koto repo root, contains `version: 2`, and passes GoReleaser's config validation.
**Status**: pending

---

## Scenario 2: GoReleaser config contains required build settings
**ID**: [x] scenario-2
**Testable after**: #25
**Category**: infrastructure
**Commands**:
- `grep -q 'main: ./cmd/koto' .goreleaser.yaml`
- `grep -q 'binary: koto-{{ .Os }}-{{ .Arch }}' .goreleaser.yaml`
- `grep -q 'format: binary' .goreleaser.yaml`
- `grep -q 'no_unique_dist_dir: true' .goreleaser.yaml`
- `grep -q 'CGO_ENABLED=0' .goreleaser.yaml`
- `grep -q 'internal/buildinfo.version' .goreleaser.yaml`
- `grep -q 'internal/buildinfo.commit' .goreleaser.yaml`
**Expected**: Config specifies raw binary format (no archives), correct entry point, binary naming convention matching `koto-{os}-{arch}`, CGO disabled, and ldflags targeting `internal/buildinfo` package variables.
**Status**: pending

---

## Scenario 3: GoReleaser config targets correct platforms (no Windows)
**ID**: [x] scenario-3
**Testable after**: #25
**Category**: infrastructure
**Commands**:
- `grep -q 'linux' .goreleaser.yaml`
- `grep -q 'darwin' .goreleaser.yaml`
- `grep -q 'amd64' .goreleaser.yaml`
- `grep -q 'arm64' .goreleaser.yaml`
- `! grep -q 'windows' .goreleaser.yaml`
**Expected**: Config targets linux/amd64, linux/arm64, darwin/amd64, darwin/arm64. Windows is explicitly absent.
**Status**: pending

---

## Scenario 4: Release workflow has two-job pipeline structure
**ID**: [x] scenario-4
**Testable after**: #25
**Category**: infrastructure
**Commands**:
- `test -f .github/workflows/release.yml`
- `grep -q 'push:' .github/workflows/release.yml`
- `grep -q '"v*"' .github/workflows/release.yml`
- `grep -q 'contents: write' .github/workflows/release.yml`
- `grep -q 'release:' .github/workflows/release.yml`
- `grep -q 'finalize-release:' .github/workflows/release.yml`
- `grep -q 'needs:' .github/workflows/release.yml`
**Expected**: Workflow triggers on `v*` tag push, has `contents: write` permission, defines two jobs (`release` and `finalize-release`), and `finalize-release` depends on `release`.
**Status**: pending

---

## Scenario 5: Release job contains GoReleaser, mod tidy check, and tag annotation steps
**ID**: [x] scenario-5
**Testable after**: #25
**Category**: infrastructure
**Commands**:
- `grep -q 'fetch-depth: 0' .github/workflows/release.yml`
- `grep -q 'go-version-file' .github/workflows/release.yml`
- `grep -q 'go mod tidy' .github/workflows/release.yml`
- `grep -q 'goreleaser' .github/workflows/release.yml`
- `grep -q 'gh release edit' .github/workflows/release.yml`
**Expected**: Release job checks out with full history, uses Go version from go.mod, verifies `go mod tidy` produces no diff, runs GoReleaser, and updates the release body with the tag annotation.
**Status**: pending

---

## Scenario 6: Finalize job verifies artifacts and publishes
**ID**: [x] scenario-6
**Testable after**: #25
**Category**: infrastructure
**Commands**:
- `grep -q 'koto-linux-amd64' .github/workflows/release.yml`
- `grep -q 'koto-linux-arm64' .github/workflows/release.yml`
- `grep -q 'koto-darwin-amd64' .github/workflows/release.yml`
- `grep -q 'koto-darwin-arm64' .github/workflows/release.yml`
- `grep -q 'sha256sum' .github/workflows/release.yml`
- `grep -q 'checksums.txt' .github/workflows/release.yml`
- `grep -q -- '--draft=false' .github/workflows/release.yml`
**Expected**: Finalize job verifies all 4 platform binaries are present, regenerates unified checksums.txt, and publishes the draft release.
**Status**: pending

---

## Scenario 7: Published v0.1.0 release has correct assets
**ID**: [x] scenario-7
**Testable after**: #26
**Category**: use-case
**Environment**: manual -- requires real CI run triggered by tag push
**Commands**:
- `RELEASE_JSON=$(gh api repos/tsukumogami/koto/releases/tags/v0.1.0)`
- `echo "$RELEASE_JSON" | jq -r '.draft'` -- should be `false`
- `echo "$RELEASE_JSON" | jq '.assets | length'` -- should be `5`
- `echo "$RELEASE_JSON" | jq -r '.assets[].name'` -- should list all 4 binaries plus checksums.txt
**Expected**: Release `v0.1.0` is published (not draft), contains exactly 5 assets: `koto-linux-amd64`, `koto-linux-arm64`, `koto-darwin-amd64`, `koto-darwin-arm64`, and `checksums.txt`.
**Status**: passed

---

## Scenario 8: Checksums file is valid and matches downloaded binary
**ID**: [x] scenario-8
**Testable after**: #26
**Category**: use-case
**Environment**: manual -- requires published release on GitHub
**Commands**:
- `gh release download v0.1.0 --repo tsukumogami/koto --pattern "checksums.txt" --dir "$TMPDIR"`
- `grep -cE '^[a-f0-9]{64}  .+$' "$TMPDIR/checksums.txt"` -- should be 4
- `gh release download v0.1.0 --repo tsukumogami/koto --pattern "koto-linux-amd64" --dir "$TMPDIR"`
- `sha256sum "$TMPDIR/koto-linux-amd64"` -- should match the entry in checksums.txt
**Expected**: `checksums.txt` contains exactly 4 lines in valid SHA-256 format. Downloading a binary and computing its checksum matches the value in `checksums.txt`.
**Status**: passed

---

## Scenario 9: Downloaded binary reports correct version
**ID**: [x] scenario-9
**Testable after**: #26
**Category**: use-case
**Environment**: manual -- requires published release and matching platform binary
**Commands**:
- `gh release download v0.1.0 --repo tsukumogami/koto --pattern "koto-$(uname -s | tr '[:upper:]' '[:lower:]')-$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')" --dir "$TMPDIR"`
- `chmod +x "$TMPDIR/koto-*"`
- `"$TMPDIR/koto-*" version`
**Expected**: Binary version output contains `0.1.0`. This validates that GoReleaser ldflags correctly injected the version into `internal/buildinfo`.
**Status**: passed

---

## Scenario 10: Install script has correct structure and static properties
**ID**: scenario-10
**Testable after**: #27
**Category**: infrastructure
**Commands**:
- `test -f install.sh`
- `head -2 install.sh | grep -q '#!/bin/bash'`
- `head -3 install.sh | grep -q 'set -euo pipefail'`
- `grep -q 'api.github.com/repos/tsukumogami/koto/releases/latest' install.sh`
- `grep -q 'GITHUB_TOKEN' install.sh`
- `grep -q 'sha256sum' install.sh && grep -q 'shasum' install.sh`
- `grep -q 'KOTO_INSTALL_DIR' install.sh`
- `grep -q '.koto/env' install.sh`
- `grep -q -- '--no-modify-path' install.sh`
- `grep -q 'mktemp -d' install.sh`
- `! grep -q 'sudo' install.sh`
**Expected**: Script uses bash with strict mode, queries the GitHub API for latest release, supports GITHUB_TOKEN, has checksum verification with sha256sum/shasum fallback, supports KOTO_INSTALL_DIR override, writes env file, supports --no-modify-path, uses temp dir with cleanup, and does not require sudo.
**Status**: pending

---

## Scenario 11: Install script shell rc file handling is idempotent
**ID**: scenario-11
**Testable after**: #27
**Category**: infrastructure
**Commands**:
- `grep -q '.bashrc' install.sh`
- `grep -q '.zshenv' install.sh`
- verify the script checks for existing source line before appending (grep for a pattern like `grep -q` or `contains` check before writing to rc files)
**Expected**: Script modifies `.bashrc`, `.bash_profile`/`.profile` for bash users and `.zshenv` for zsh users. It checks whether the source line already exists before appending to prevent duplication on repeated runs.
**Status**: pending

---

## Scenario 12: Install script downloads, verifies, and installs koto end-to-end
**ID**: scenario-12
**Testable after**: #27
**Category**: use-case
**Environment**: manual -- requires published release on GitHub and a linux or darwin machine
**Commands**:
- `export KOTO_INSTALL_DIR="$(mktemp -d)/koto"`
- `bash install.sh --no-modify-path`
- `"$KOTO_INSTALL_DIR/koto" version`
- `rm -rf "$KOTO_INSTALL_DIR"`
**Expected**: Running install.sh with a custom install directory and --no-modify-path downloads the binary for the current platform, verifies the checksum, installs to the specified directory, and the installed binary reports a version containing `0.1.0`. No shell rc files are modified.
**Status**: pending

---

## Scenario 13: tsuku recipe file has correct structure
**ID**: scenario-13
**Testable after**: #28
**Category**: infrastructure
**Commands**:
- `test -f recipes/k/koto.toml` (in tsuku repo)
- `grep -q 'name = "koto"' recipes/k/koto.toml`
- `grep -q 'action = "github_file"' recipes/k/koto.toml`
- `grep -q 'repo = "tsukumogami/koto"' recipes/k/koto.toml`
- `grep -q 'asset_pattern = "koto-{os}-{arch}"' recipes/k/koto.toml`
- `grep -q 'binary = "koto"' recipes/k/koto.toml`
- `grep -q 'command = "koto version"' recipes/k/koto.toml`
**Expected**: Recipe exists at `recipes/k/koto.toml` in the tsuku repo, uses `github_file` action pointing to `tsukumogami/koto`, asset pattern matches GoReleaser binary naming, and the verify section runs `koto version`.
**Status**: pending

---

## Scenario 14: tsuku install koto works end-to-end
**ID**: scenario-14
**Testable after**: #28
**Category**: use-case
**Environment**: manual -- requires tsuku installed, published koto release on GitHub
**Commands**:
- `tsuku install koto`
- `~/.tsuku/bin/koto version`
**Expected**: `tsuku install koto` downloads the koto binary from GitHub releases, installs it to `~/.tsuku/bin/`, and `koto version` reports a version containing `0.1.0`. This validates both the tsuku recipe and koto's GitHub release assets as a distribution source.
**Status**: pending

---

## Notes

### Environment dependencies

Scenarios 1-6, 10-11, and 13 are **automatable** -- they validate file contents and can run in CI or locally against the checked-out code with no external dependencies.

Scenarios 7-9 and 12 require a **published GitHub release** (produced by the real CI pipeline triggered by a tag push). These cannot be simulated locally. Issue #26 is an operational step (tag + push), not a code change, so its validation is inherently environment-dependent.

Scenario 14 requires both a **published release** and **tsuku installed**. Since this targets a different repository (tsukumogami/tsuku), it must be validated in the tsuku repo context.

### Cross-repository testing

Issue #28 delivers code to `tsukumogami/tsuku`, not `tsukumogami/koto`. Scenarios 13-14 run against the tsuku repo. The test executor must switch repo context for these scenarios.

### CI vs. manual split

| Category | Scenarios | Notes |
|----------|-----------|-------|
| Automatable (CI) | 1, 2, 3, 4, 5, 6, 10, 11, 13 | Static file checks, can run in any checkout |
| Manual (real CI + release) | 7, 8, 9, 12, 14 | Need published GitHub release and/or real tool installed |
