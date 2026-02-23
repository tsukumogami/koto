---
status: Proposed
problem: |
  koto has a working engine, template compiler, and cache, but no way to install the binary. Users must clone the repo and run go build. There are no pre-built binaries, no release process, and no standard installation channel.
decision: |
  Release automation mirrors tsuku's existing pipeline: GoReleaser builds raw binaries (format: binary, no archives) on tag push, creates a draft GitHub release with tag-annotation release notes, and a finalize job verifies artifacts and publishes. A self-contained install script (curl|sh, served from GitHub) is the primary installation channel, installing to ~/.koto/bin/ with checksum verification and PATH setup. tsuku gets a recipe for dogfooding. Third-party package managers are deferred until stable.
rationale: |
  Mirroring tsuku's release infrastructure means we don't reinvent the wheel and can grow in complexity alongside it. A self-contained install script gives koto full control over the installation experience without depending on third-party package ecosystems. The script follows the same pattern as tsuku and Claude Code (curl|sh, checksum verification, shell PATH integration), served directly from GitHub since we don't own a custom domain yet. The tsuku recipe eats our own dogfood. Deferring Homebrew and other package managers avoids maintaining tap repos and cross-repo secrets while the CLI surface is still changing.
---
# DESIGN: koto Installation and Distribution

## Status

**Proposed**

## Context and Problem Statement

koto is a Go CLI binary with a working engine, template compiler, and cache. But there's no way to install it. Someone who wants to use koto has to clone the repository and run `go build`. There are no pre-built binaries, no release process, and no standard installation channel. (Version embedding and the `koto version` subcommand already exist in `internal/buildinfo/`, but they don't help without a distribution mechanism.)

Getting koto working end-to-end requires three steps:

1. **Binary on the machine** -- the user installs koto
2. **Integration into the project** -- something puts koto-aware configuration into the project so agents discover and use koto
3. **Agent invokes koto** -- the agent calls `koto next`, `koto transition`, etc. during workflow execution

This design covers **step 1 only**: how koto is released and how users get the binary on their machine. Step 3 is the engine (already built). Step 2 -- how agents discover koto in a project, how templates are distributed and installed, and what the first-run experience looks like -- is unsolved and covered by a separate design (the agent integration design).

### Scope

**In scope:**
- Release automation via GoReleaser and GitHub Actions (mirroring tsuku's pipeline)
- Self-contained install script served from GitHub (`curl -fsSL <github-raw-url> | sh`)
- tsuku recipe for koto (eating our own dogfood)
- Version embedding (already implemented, needs GoReleaser wiring)

**Deferred to agent integration design:**
- Built-in template distribution (go:embed, template search path) -- how templates are installed and discovered depends on how agents integrate with koto, which is not yet designed. Embedding templates in the binary is premature until we know whether a template is usable without agent-side configuration.
- Template search path (project-local, user-level, built-in)
- First-run experience and `koto init` scaffolding

**Deferred until stable:**
- Third-party package managers (Homebrew, apt, Scoop, WinGet) -- GoReleaser already produces the binaries they'd need, so adding channels later is straightforward.

**Out of scope:**
- Agent integration (how agents discover koto in a project -- separate design)
- Template authoring UX (covered by the template format design)
- `go install` as a primary channel (works by default for Go users, no design needed)

## Decision Drivers

- **Single static binary**: koto has no runtime dependencies and no CGO. Distribution is straightforward.
- **Mirror tsuku**: Reuse tsuku's release infrastructure patterns so both projects grow in complexity together rather than diverging.
- **Own the installation experience**: A self-contained installer gives full control over PATH setup, version management, and future auto-update without depending on third-party package ecosystems.
- **Existing conventions**: The `~/.koto/` directory and `KOTO_HOME` env var are already established by the cache system.
- **Security**: Install script must verify checksums before installing.
- **Early stage**: The CLI surface is still changing. Avoid committing to third-party package manager maintenance until stable.

## Considered Options

### Decision 1: Release Automation

koto needs to go from a git tag to published binaries across platforms. This happens on every release and must be reliable.

#### Chosen: GoReleaser via GitHub Actions (mirroring tsuku)

The release pipeline mirrors tsuku's existing infrastructure so both projects grow in complexity together rather than diverging. A push to a semver tag (e.g., `v0.1.0`) triggers a GitHub Actions workflow with two jobs:

1. **`release` job**: Runs GoReleaser to build binaries, then replaces the auto-generated release body with the git tag annotation message (release notes live in the tag, not a CHANGELOG file or GoReleaser config).
2. **`finalize-release` job**: Verifies all expected artifacts are present, regenerates a unified `checksums.txt`, and publishes the draft.

Key GoReleaser configuration choices (all matching tsuku):
- **`format: binary`**: Raw executables, no tar.gz archives. Simpler for the install script and tsuku recipe to consume.
- **Binary naming**: `koto-{{ .Os }}-{{ .Arch }}` with `no_unique_dist_dir: true`
- **Draft release**: `make_latest: true`, `prerelease: auto`. The release stays draft until the finalize job publishes it.
- **Changelog disabled**: Release notes come from git tag annotations, set via `gh release edit` in the release job.
- **ldflags**: `-s -w` (strip debug info), `-X github.com/tsukumogami/koto/internal/buildinfo.version={{.Version}}`, `-X github.com/tsukumogami/koto/internal/buildinfo.commit={{.Commit}}`
- **Build flags**: `-trimpath`, `-buildvcs=false`, `CGO_ENABLED=0`, `mod_timestamp` set to commit timestamp -- all for reproducible builds.
- **Platforms**: linux/darwin, amd64/arm64. No Windows.

Version information is already embedded at build time via ldflags into `internal/buildinfo` package variables. The `koto version` subcommand and `debug.BuildInfo` fallback are implemented. The GoReleaser ldflags target `internal/buildinfo`, not `main`.

#### Alternatives Considered

**Manual release scripts**: Shell scripts that call `go build` for each platform, compute checksums, and upload via `gh release create`. Rejected because it duplicates what GoReleaser does with more maintenance burden. GoReleaser is standard practice for Go CLIs.

**GitHub Actions only (no GoReleaser)**: Use `actions/setup-go` + `go build` in a matrix job, then `actions/upload-artifact`. Rejected because it requires reimplementing archive creation, checksum generation, and changelog features that GoReleaser handles declaratively.

### Decision 2: Primary Installation Channel

Users need a way to install koto that doesn't require Go tooling or a third-party package manager.

#### Chosen: Self-contained install script

A shell script hosted in the koto repository and served via GitHub's raw content URL. It downloads the correct binary for the user's platform, verifies its checksum, and installs it to `~/.koto/bin/`. Usage:

```bash
curl -fsSL https://raw.githubusercontent.com/tsukumogami/koto/main/install.sh | sh
```

This follows the same pattern used by tsuku, Claude Code, Deno, Bun, and Rust/rustup. We don't own a custom domain for koto yet, so the script is served directly from GitHub. If a custom domain is acquired later, the URL can change without affecting the script's behavior.

The script:
1. **Detects platform**: `uname -s` for OS (linux/darwin), `uname -m` for arch (amd64/arm64), with normalization
2. **Resolves latest version**: Queries the GitHub API for the latest release tag
3. **Downloads binary + checksums**: From GitHub Release assets, to a temp directory with trap cleanup
4. **Verifies checksum**: SHA-256 verification using `sha256sum` or `shasum -a 256` (macOS fallback). Warns if neither tool is available (matching tsuku's behavior).
5. **Installs to `~/.koto/bin/`**: Creates the directory if needed, copies binary
6. **Sets up PATH**: Writes `~/.koto/env` containing the PATH export, sources it from shell rc files (`.bashrc`, `.zshenv`, etc.). Idempotent -- checks for existing source line before appending. Skippable with `--no-modify-path`.

Installing to `~/.koto/bin/` reuses the existing `~/.koto/` directory convention established by the cache system. The `env` file pattern (used by tsuku and rustup) means future PATH changes only need one file updated.

This channel also opens the door for auto-update in the future. The install script's `~/.koto/bin/` location is controlled by koto, so a future `koto update` command could replace the binary in-place without conflicting with a system package manager.

#### Alternatives Considered

**Homebrew tap as primary**: Create `tsukumogami/homebrew-tap` with an auto-updated formula via GoReleaser. Rejected for now because it requires maintaining a separate repository, configuring cross-repo secrets (`HOMEBREW_TOKEN`), and updating the formula on every release -- overhead that doesn't pay off while the tool is pre-stable. A Homebrew tap can be added later since GoReleaser already produces the release assets Homebrew needs.

**`go install` as primary**: `go install github.com/tsukumogami/koto/cmd/koto@latest`. Rejected as the *primary* channel because it requires the Go toolchain, which not all koto users will have. It works fine as a secondary channel for Go developers and needs no design work -- it just works.

**Direct binary download only**: Point users to the GitHub Releases page and let them download manually. Rejected because it provides no PATH setup, no checksum verification guidance, and a poor first-run experience. Fine as a fallback for users who don't trust curl-pipe-sh.

### Decision 3: tsuku Recipe

koto is built by the same organization as tsuku. A tsuku recipe for koto validates our own distribution pipeline and gives tsuku users a familiar installation path.

#### Chosen: Standard tsuku recipe with GitHub releases provider

A TOML recipe in tsuku's `recipes/` directory that uses the GitHub releases version provider to resolve the latest version, downloads the platform-appropriate binary from GitHub Release assets, verifies the checksum, and installs to `~/.tsuku/bin/`.

```toml
[metadata]
name = "koto"
description = "Workflow orchestration engine for AI agents"
homepage = "https://github.com/tsukumogami/koto"

[version]
provider = "github-releases"
owner = "tsukumogami"
repo = "koto"

[install]
actions = [
    { type = "download", url = "https://github.com/tsukumogami/koto/releases/download/v{{version}}/koto-{{os}}-{{arch}}" },
    { type = "chmod", mode = "755" },
    { type = "install_binaries", pattern = "koto-*" },
]
```

Since GoReleaser uses `format: binary` (raw executables, no archives), the recipe downloads the binary directly -- no extract step needed. This matches how tsuku's own binary is distributed.

The tsuku channel and the install script are independent. tsuku installs to `~/.tsuku/bin/`, the install script installs to `~/.koto/bin/`. Users pick one. If they have tsuku, `tsuku install koto` is the natural choice. If not, the install script works without dependencies.

## Decision Outcome

### Summary

koto gets a release pipeline mirroring tsuku's and two installation channels. GoReleaser builds raw binaries (no archives) on tag push, creates a draft GitHub release with tag-annotation release notes, and a finalize job verifies artifacts and publishes. A self-contained install script (served from GitHub) downloads the right binary, verifies its checksum, installs to `~/.koto/bin/`, and sets up PATH. A tsuku recipe provides a second channel for tsuku users. Third-party package managers (Homebrew, apt, etc.) are deferred until the CLI surface stabilizes.

Template distribution (built-in templates, template search path, first-run experience) is deferred to the agent integration design. Until we know how agents discover and use koto in a project, we can't be confident that embedding templates in the binary makes them usable.

This design covers getting the binary installed (step 1). How agents discover and use koto in a project (step 2) is a separate design problem.

### Rationale

Mirroring tsuku's release infrastructure means we don't reinvent the wheel. Both projects use GoReleaser with the same conventions (raw binary format, draft releases, tag-annotation notes, finalize job). As koto grows in complexity (e.g., companion binaries, additional platforms), we can follow the same patterns tsuku already solved.

A self-contained install script gives koto full control over the installation experience during early development. Serving it from GitHub avoids the need for a custom domain while still being a stable, auditable URL. The `~/.koto/bin/` install location reuses the existing `~/.koto/` convention and opens the door for a future `koto update` command.

The tsuku recipe validates koto's GitHub releases as a distribution source and eats our own dogfood. It's zero additional infrastructure -- just a TOML file in tsuku's existing registry.

Deferring Homebrew and other package managers avoids maintaining formula repos and cross-repo secrets while the CLI surface is changing. GoReleaser already produces the release assets those channels need, so adding them later is a configuration change, not a redesign.

## Solution Architecture

### Components

```
.goreleaser.yaml         -- Build matrix, binary config (mirrors tsuku)
.github/workflows/
  release.yml            -- Tag-triggered: release + finalize-release jobs
install.sh               -- Self-contained install script
internal/
  buildinfo/             -- Already exists: version, commit, BuildInfo fallback
```

Plus a tsuku recipe at `tsukumogami/tsuku/recipes/koto.toml` (in the tsuku repo, not koto).

### Version Embedding (already implemented)

`internal/buildinfo/version.go` already provides version variables with `debug.BuildInfo` fallback, and `cmd/koto/main.go` already has a `version` subcommand. GoReleaser's ldflags will target `github.com/tsukumogami/koto/internal/buildinfo.version` and `.commit`.

No new code is needed for version infrastructure. The existing output format may differ slightly from what GoReleaser produces, and will be adjusted when GoReleaser is configured.

### Install Script

The install script lives at `install.sh` in the koto repo root. It's served via GitHub raw URL since we don't own a custom domain yet. It's a standalone bash script with `set -euo pipefail`, closely mirroring tsuku's `install.sh`.

Key behaviors (all matching tsuku's installer):
- **Platform detection**: `uname -s` + `uname -m`, normalized to `linux`/`darwin` and `amd64`/`arm64`. Fails fast on unsupported platforms.
- **Version resolution**: Queries `api.github.com/repos/tsukumogami/koto/releases/latest`. Supports `$GITHUB_TOKEN` to avoid rate limiting.
- **Download URL**: Matches GoReleaser binary naming: `koto-${OS}-${ARCH}` (raw binary, no archive).
- **Checksum verification**: Downloads `checksums.txt` from the release, greps for the binary, verifies with `sha256sum` or `shasum -a 256`. Warns if neither tool is available (matching tsuku's behavior).
- **Install directory**: `$KOTO_INSTALL_DIR` or `~/.koto/bin/koto`. Creates dirs with `mkdir -p`.
- **PATH setup**: Writes `~/.koto/env` with `export PATH="${KOTO_HOME:-$HOME/.koto}/bin:$PATH"`. Sources from shell rc files based on `$SHELL` (bash: `.bashrc` + `.bash_profile`/`.profile`; zsh: `.zshenv`). Idempotent. Skippable with `--no-modify-path`.
- **Cleanup**: `trap 'rm -rf "$TEMP_DIR"' EXIT`

### GoReleaser Configuration

Mirrors tsuku's `.goreleaser.yaml`:

- **Platforms**: linux/amd64, linux/arm64, darwin/amd64, darwin/arm64. No Windows.
- **Format**: `binary` (raw executables, no tar.gz). Binary naming: `koto-{{ .Os }}-{{ .Arch }}`.
- **ldflags**: `-s -w -X ...buildinfo.version={{.Version}} -X ...buildinfo.commit={{.Commit}}`
- **Build flags**: `-trimpath`, `-buildvcs=false`, `CGO_ENABLED=0`
- **`mod_timestamp`**: Commit timestamp for reproducibility
- **Release**: Draft, `make_latest: true`, `prerelease: auto`
- **Changelog**: Disabled (release notes come from git tag annotations)
- **Checksums**: `checksums.txt` with sha256

### Release Workflow

Mirrors tsuku's two-job pipeline:

```
git tag -a v0.1.0 -m "Release notes here..."
git push origin v0.1.0
  --> GitHub Actions: .github/workflows/release.yml

  Job 1: release
    --> GoReleaser: build binaries + checksums + draft GitHub Release
    --> gh release edit: replace body with tag annotation message

  Job 2: finalize-release (depends on: release)
    --> Verify all 4 expected artifacts present (koto-{linux,darwin}-{amd64,arm64})
    --> Regenerate unified checksums.txt
    --> Publish draft: gh release edit --draft=false
```

After the release is published, users can install immediately via the install script or `tsuku install koto` (both query the GitHub API for the latest release).

## Implementation Approach

### Phase 1: Version infrastructure -- already complete

`internal/buildinfo/version.go` and `cmd/koto/main.go` version subcommand already exist. No work needed.

### Phase 2: GoReleaser and release workflow
- Add `.goreleaser.yaml` mirroring tsuku's config (format: binary, ldflags targeting `internal/buildinfo`, draft release, changelog disabled)
- Add `.github/workflows/release.yml` with two jobs: `release` (GoReleaser + tag-annotation notes) and `finalize-release` (verify artifacts, unified checksums, publish)

### Phase 3: First release (v0.1.0)
- Tag `v0.1.0` and push to trigger the release workflow
- Validate the full pipeline: GoReleaser builds, draft release created, artifacts verified, checksums regenerated, release published
- Verify the published release has all 4 expected binaries and a valid `checksums.txt`
- Manually download and run a binary to confirm version output and basic functionality
- This release is a prerequisite for both the install script and the tsuku recipe -- they need real release assets to download from

### Phase 4: Install script
- Write `install.sh` adapting tsuku's install script (platform detection, binary naming, checksum verification, PATH setup to `~/.koto/bin/`)
- Test on linux and darwin: verify it downloads the v0.1.0 binary, verifies checksum, installs correctly, and sets up PATH
- Test `--no-modify-path` flag
- Test with `$KOTO_INSTALL_DIR` override

### Phase 5: tsuku recipe
- Add `koto.toml` recipe to tsuku's `recipes/` directory (in the tsuku repo)
- Test `tsuku install koto` against the published v0.1.0 release
- Verify the installed binary reports the correct version and runs correctly from `~/.tsuku/bin/`

## Security Considerations

### Download Verification

The finalize-release job regenerates a unified `checksums.txt` file containing SHA-256 hashes for every release artifact. The install script downloads this file and verifies the binary's checksum before installation. On checksum mismatch the script exits with an error (matching tsuku's behavior, which warns but continues if no checksum tool is available).

`go install` builds from source, so download verification isn't applicable.

Cosign-based artifact signing is planned for v0.2.0. For v0.1.0, checksum verification provides integrity but not authenticity. A compromised GitHub account could publish malicious binaries with valid checksums. This risk is elevated for koto because the binary operates in an AI agent's toolchain -- a compromised koto could manipulate workflow state, inject directives, or exfiltrate evidence data, and automated agents may not catch the difference. This makes signing a priority rather than a convenience.

### Install Script Security

The curl-pipe-sh pattern has known risks: network MITM could serve a different script, and the script runs with the user's full permissions. Mitigations:

- The script is served from `raw.githubusercontent.com` over HTTPS. `curl -fsSL` fails on HTTP errors.
- The script only downloads from `github.com` (a trusted domain) and verifies checksums from the same release.
- The script does not require `sudo` or elevated permissions. It installs to the user's home directory.
- The script source is committed to the koto repository and can be audited before running.
- Users who don't trust curl-pipe-sh can download the binary manually from GitHub Releases and verify checksums themselves.

The script doesn't phone home or collect telemetry. It makes three HTTPS requests: GitHub API (version resolution), GitHub Releases (binary download), and GitHub Releases (checksums.txt).

### Execution Isolation

koto runs as the current user with no elevated permissions. It reads template files, writes state files and cache entries to `~/.koto/`, and executes shell commands defined in command gates. The binary itself requires only filesystem read/write access to the working directory and `~/.koto/`.

Command gates (shell execution in templates) are the highest-risk surface. These are defined by template authors, not koto itself, and run with the user's full permissions. This is by design and documented in the template format specification. koto doesn't sandbox gate commands.

### Supply Chain Risks

Binaries are built by GitHub Actions from the public `tsukumogami/koto` repository. The build is reproducible given the same Go toolchain version and source commit. GoReleaser logs the exact build environment in each release.

Both the install script and the tsuku recipe download from GitHub Release assets, not a third-party CDN. The release assets are produced by the same CI pipeline from the same source.

`go install` builds from source, eliminating binary supply chain concerns entirely. Users can audit the code before building.

### User Data Exposure

koto reads template files and writes state files. It doesn't phone home, collect telemetry, or transmit data externally. The cache stores compiled template JSON locally. State files contain workflow state (current state, evidence, variables) which may include user-provided values.

The default path (`~/.koto/`) is created with `0700` permissions, matching the existing cache directory behavior. Note that `MkdirAll` only sets permissions on newly created directories -- if `~/.koto/` already exists with more permissive modes, they won't be tightened. The implementation should check and warn if existing permissions are more permissive than `0700`.

## Consequences

### Positive
- Users can install koto with one command (`curl ... | sh` or `tsuku install koto`)
- Install script handles platform detection, checksum verification, and PATH setup automatically
- No third-party infrastructure to maintain during early development
- Release pipeline mirrors tsuku's -- shared patterns, no wheel reinvention
- `~/.koto/bin/` install location opens the door for future auto-update
- tsuku recipe validates our own distribution pipeline

### Negative
- curl-pipe-sh has trust concerns (mitigated by HTTPS + checksum verification + auditability)
- No presence in Homebrew or other package managers reduces discoverability
- Two installation locations (`~/.koto/bin/` vs `~/.tsuku/bin/`) could confuse users who use both
- Installing the binary alone doesn't make koto usable -- agent integration (step 2) is still needed

### Mitigations
- The release process is lightweight (tag + push), so frequent releases are cheap
- Homebrew and other package managers can be added later as a configuration change (GoReleaser already produces what they need)
- Users who prefer package managers can download binaries directly from GitHub Releases
- The install script and tsuku install to different directories by design -- they're independent channels, not competing ones
