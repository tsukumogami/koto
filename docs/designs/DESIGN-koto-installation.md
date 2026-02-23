---
status: Proposed
problem: |
  koto has a working engine, template compiler, and cache, but no way to install the binary. Users must clone the repo and run go build. There are also no built-in templates, so the first-run experience requires authoring a template before trying anything. Both problems block adoption.
decision: |
  GoReleaser builds multi-platform binaries on tag push and auto-updates a Homebrew formula. Built-in templates are compiled into the binary via go:embed. A three-layer template search path in pkg/resolve checks project-local, user-level, and built-in locations with override detection and a --no-local safety flag. Version infrastructure already exists in internal/buildinfo.
rationale: |
  GoReleaser, go:embed, and the three-layer search path reinforce each other. GoReleaser produces a hermetic binary that includes embedded templates, so every installation method gets the same built-in templates without extra steps. The search path's override model works because go:embed makes the built-in layer always present. Placing the resolver in pkg/ keeps it available to library consumers, and accepting fs.FS rather than embedding directly keeps it testable.
---
# DESIGN: koto Installation and Distribution

## Status

**Proposed**

## Context and Problem Statement

koto is a Go CLI binary with a working engine, template compiler, and cache. But there's no way to install it. Someone who wants to use koto has to clone the repository and run `go build`. There are no pre-built binaries, no release process, and no standard installation channel. (Version embedding and the `koto version` subcommand already exist in `internal/buildinfo/`, but they don't help without a distribution mechanism.)

This is a blocker for adoption. The engine and template format are ready for users, but users can't get the binary. koto also has no mechanism for shipping built-in templates, which means the first-run experience requires users to author a template before they can try anything. These are two separable problems -- installation works independently of built-in templates -- but they share a release vehicle, so this design covers both.

### Scope

**In scope:**
- Binary distribution via GoReleaser (GitHub Releases, checksums)
- Installation channels: `go install`, Homebrew tap
- Version embedding (version, commit SHA, build date)
- `koto version` command
- Built-in template distribution via `go:embed`
- Template search path (project-local, user-level, built-in)
- Release automation via GitHub Actions

**Out of scope:**
- Template authoring UX (covered by the template format design)
- Agent integration (`koto generate`, AGENTS.md)
- `koto init` scaffolding (separate design)
- Windows-specific distribution (Scoop, WinGet)
- Linux package repos (apt, rpm)

## Decision Drivers

- **Single static binary**: koto has no runtime dependencies and no CGO. Distribution is straightforward.
- **Existing conventions**: The `~/.koto/` directory and `KOTO_HOME` env var are already established by the cache system.
- **Security**: Template search path must not include world-writable or shared directories.
- **Zero-config first run**: Built-in templates should work without any setup.
- **Standard Go ecosystem**: Follow established patterns (`go install`, GoReleaser, Homebrew tap).

## Considered Options

### Decision 1: Release Automation

koto needs to go from a git tag to published binaries across platforms. This happens on every release and must be reliable.

#### Chosen: GoReleaser via GitHub Actions

GoReleaser is the standard tool for Go CLI releases. A push to a semver tag (e.g., `v0.1.0`) triggers a GitHub Actions workflow that builds binaries for linux/darwin (amd64/arm64), creates archives with checksums, publishes to GitHub Releases, and auto-updates the Homebrew formula.

The `.goreleaser.yml` config is declarative. It specifies the build matrix, archive format (tar.gz for Unix, zip for Windows), ldflags for version embedding, and the Homebrew tap repository. GoReleaser handles changelog generation from commits since the previous tag.

Version information is already embedded at build time via ldflags into `internal/buildinfo` package variables. The `koto version` subcommand and `debug.BuildInfo` fallback are implemented. GoReleaser's ldflags target `github.com/tsukumogami/koto/internal/buildinfo.version` and `.commit` (not `main.*`).

#### Alternatives Considered

**Manual release scripts**: Shell scripts that call `go build` for each platform, compute checksums, and upload via `gh release create`. Rejected because it duplicates what GoReleaser does with more maintenance burden and no Homebrew integration. Every Go project of note has moved to GoReleaser or something similar.

**GitHub Actions only (no GoReleaser)**: Use `actions/setup-go` + `go build` in a matrix job, then `actions/upload-artifact`. Rejected because it requires reimplementing archive creation, checksum generation, and Homebrew formula updates that GoReleaser handles declaratively.

### Decision 2: Built-in Template Distribution

koto needs to ship with at least one usable template so new users can try it without writing their own. The question is how templates get from the repository into the binary.

#### Chosen: go:embed for built-in templates

Built-in templates are embedded into the binary at compile time using Go's `//go:embed` directive. A `templates/` directory in the source tree contains the built-in template files (starting with `quick-task`, the first template being designed separately). The embed directive packages these into the binary as an `embed.FS`.

At runtime, template loading checks a search path. Built-in templates are the last layer, after project-local and user-level templates. This means users can override any built-in template by placing a file with the same name earlier in the search path.

The embedded filesystem is read-only and immutable after compilation. Updating built-in templates requires a new release. This is a feature, not a limitation: it guarantees that `koto@v0.2.0` always ships the same templates regardless of what's on the user's filesystem.

#### Alternatives Considered

**Filesystem-only (no embedding)**: Ship templates as separate files alongside the binary, or download them on first run. Rejected because it breaks the single-binary property. Users would need to manage template files separately, and `go install` can't distribute non-Go files.

**Hybrid with runtime download**: Embed a minimal set, download updates from a registry. Rejected as premature. A template registry adds complexity (versioning, caching, authentication, CDN) for a problem that doesn't exist yet. When koto has enough templates to justify a registry, that's a separate design.

### Decision 3: Template Search Path

When koto loads a template by name, it needs to know where to look. The search path determines precedence and security boundaries.

#### Chosen: Three-layer search (project, user, built-in)

The template search path checks three locations in order:

1. **Project-local**: `.koto/templates/` relative to the git repository root (detected via the existing `gitRepoRoot()` mechanism in `engine.go`), falling back to the current working directory if not in a git repo. This is where project-specific templates live.
2. **User-level**: `$KOTO_HOME/templates/` (defaults to `~/.koto/templates/`). This is where user-authored or third-party templates go.
3. **Built-in**: The embedded `embed.FS` from the binary. Ships with koto's default templates.

The first match wins. A project-local template with the same name as a built-in template takes precedence. No directory in the default search path is world-writable or shared. `KOTO_HOME` lets users relocate the user-level directory but doesn't add search locations.

#### Alternatives Considered

**XDG Base Directory layout**: Use `~/.config/koto/` for config, `~/.local/share/koto/` for templates, `~/.cache/koto/` for cache. Rejected because koto already uses `~/.koto/` with `KOTO_HOME` override (established by the cache system in `pkg/cache/`, used by existing installations). Switching to XDG would break the cache path contract for anyone already using koto. The existing convention is simpler and already documented.

**`KOTO_TEMPLATE_PATH` environment variable**: A colon-separated search path (like `PATH` or `GOPATH`) that lets users add arbitrary directories. Rejected because arbitrary directories introduce security risk -- shared or team directories could contain malicious templates with command gates. The three-layer search covers the common cases (project customization, user defaults, built-ins) without exposing a configurable search path.

**Single directory (no search path)**: Only look in one place, require explicit paths for everything else. Rejected because it forces users to copy built-in templates into their project or home directory to use them, and it prevents the override pattern that makes customization natural.

## Decision Outcome

### Summary

koto gets a standard Go open-source release pipeline. GoReleaser builds multi-platform binaries on tag push, publishes to GitHub Releases with checksums, and auto-updates a Homebrew formula in `tsukumogami/homebrew-tap`. Version info (tag, commit SHA, build date) is embedded via ldflags and reported by `koto version`.

Built-in templates are compiled into the binary via `go:embed`. A three-layer template search path checks project-local (`.koto/templates/`), user-level (`$KOTO_HOME/templates/`), and built-in (embedded) locations in that order. The first match wins. No world-writable directories appear in the default path.

Users install via `brew install tsukumogami/tap/koto`, `go install github.com/tsukumogami/koto/cmd/koto@latest`, or by downloading a binary from GitHub Releases. All three methods produce the same single static binary with identical behavior. The `go install` path uses `debug.BuildInfo` for version detection rather than ldflags.

### Rationale

GoReleaser, `go:embed`, and the three-layer search path reinforce each other. GoReleaser produces a hermetic binary that includes embedded templates, so every installation method gets the same built-in templates without extra steps. The search path's override model works because `go:embed` makes the built-in layer always present. And `KOTO_HOME` serves both the cache (already shipped) and the template search path, keeping the configuration surface small.

Starting with just Homebrew and `go install` covers macOS developers and Go users without building package infrastructure for every platform. Windows and Linux package managers can be added later as separate work since GoReleaser already produces the binaries they'd need.

## Solution Architecture

### Components

```
.goreleaser.yml          -- Build matrix, archive config, Homebrew tap
.github/workflows/
  release.yml            -- Tag-triggered GoReleaser workflow
cmd/koto/
  main.go                -- embed.FS declaration + passes to resolver
internal/
  buildinfo/             -- Already exists: version, commit, BuildInfo fallback
templates/
  quick-task/            -- Built-in template (designed separately)
    template.md
pkg/
  resolve/
    resolve.go           -- Template search path resolution
    resolve_test.go
```

### Version Embedding (already implemented)

`internal/buildinfo/version.go` already provides version variables with `debug.BuildInfo` fallback, and `cmd/koto/main.go` already has a `version` subcommand. GoReleaser's ldflags will target `github.com/tsukumogami/koto/internal/buildinfo.version` and `.commit`.

No new code is needed for version infrastructure. The existing output format may differ slightly from what GoReleaser produces, and will be adjusted when GoReleaser is configured.

### Template Search Path Resolution

The `//go:embed` directive lives in `cmd/koto/main.go` (the only package at the right level to reach the top-level `templates/` directory). The embedded `embed.FS` is passed to the resolver as an `fs.FS` parameter, keeping the resolver package testable and decoupled from the binary's embed.

The `pkg/resolve` package implements template lookup. It's in `pkg/` (not `internal/`) because library consumers embedding koto's engine need the same name-based template resolution:

```go
func NewResolver(builtins fs.FS) *Resolver
func (r *Resolver) FindTemplate(name string) ([]byte, string, error)
```

Returns the template source bytes, the origin it was loaded from (e.g., `.koto/templates/quick-task/template.md` or `"built-in"`), and an error. The function checks:

1. `.koto/templates/<name>/template.md` at the git repo root (via `gitRepoRoot()`), falling back to the working directory
2. `$KOTO_HOME/templates/<name>/template.md` (default: `~/.koto/templates/`)
3. Embedded `templates/<name>/template.md` from the `fs.FS`

If no match is found in any layer, it returns an error listing the locations checked.

When a project-local template overrides a built-in or user-level template of the same name, the resolver prints a notice to stderr: `note: using project-local template .koto/templates/<name>/template.md (overrides built-in)`. A `--no-local` flag on `koto init` skips the project-local layer entirely, allowing users to bypass untrusted overrides.

The resolver returns raw source bytes. Callers feed these into the existing compile/cache/validate pipeline (currently in `cmdInit` and `loadTemplateFromState`). Phase 3 should extract that pipeline into a shared function so name-based and path-based template loading converge on a single code path.

Each template is a directory containing a `template.md` file (e.g., `quick-task/template.md`). This directory-based convention exists to support future multi-file templates and matches the structure used by the existing filesystem-based template loading.

### GoReleaser Configuration

Target platforms: `linux/amd64`, `linux/arm64`, `darwin/amd64`, `darwin/arm64`. Windows deferred.

Archives: `tar.gz` for all platforms. Checksum file (`checksums.txt`) published alongside release assets.

Homebrew: Auto-generated formula pushed to `tsukumogami/homebrew-tap` repository via GoReleaser's brew integration. Requires a `HOMEBREW_TOKEN` secret with repo access to the tap.

### Release Workflow

```
git tag v0.1.0
git push origin v0.1.0
  --> GitHub Actions: .github/workflows/release.yml
    --> GoReleaser: build + archive + checksum + GitHub Release
    --> GoReleaser: update tsukumogami/homebrew-tap formula
```

## Implementation Approach

### Phase 1: Version infrastructure -- already complete

`internal/buildinfo/version.go` and `cmd/koto/main.go` version subcommand already exist. No work needed.

### Phase 2: GoReleaser and release workflow
- Add `.goreleaser.yml` configuration (ldflags targeting `internal/buildinfo`)
- Add `.github/workflows/release.yml`
- Create `tsukumogami/homebrew-tap` repository (with branch protection, fine-grained PAT scoped to tap repo only)
- Tag and test first release (`v0.1.0`) -- this release ships without template search path or built-ins, which is intentional for validating the release pipeline

### Phase 3: Template search path
- Extract compile/cache/validate pipeline from `cmdInit` and `loadTemplateFromState` into a shared function
- Create `pkg/resolve` package with `NewResolver(builtins fs.FS)` constructor
- Add `//go:embed` directive in `cmd/koto/main.go` for `templates/` directory
- Implement three-layer search (project, user, built-in) with override notice
- Add `--no-local` flag to `koto init`
- Define `--template` flag semantics: if value contains a path separator or ends in `.md`, treat as path; otherwise treat as name and resolve via `FindTemplate`
- Tests for search order, override behavior, override warning, `--no-local`, missing templates

### Phase 4: Built-in template
- Add `templates/quick-task/template.md` (depends on the quick-task template design, issue #313 in the vision milestone)
- Verify the full path: install via `go install`, run `koto init quick-task`, confirm the built-in template loads

## Security Considerations

### Download Verification

GoReleaser generates a `checksums.txt` file containing SHA-256 hashes for every release artifact. Users who download binaries directly can verify integrity:

```bash
sha256sum --check checksums.txt
```

Homebrew verifies checksums automatically through the formula's `sha256` fields. `go install` builds from source, so download verification isn't applicable.

Cosign-based artifact signing is planned for v0.2.0. For v0.1.0, checksum verification provides integrity but not authenticity. A compromised GitHub account could publish malicious binaries with valid checksums. This risk is elevated for koto because the binary operates in an AI agent's toolchain -- a compromised koto could manipulate workflow state, inject directives, or exfiltrate evidence data, and automated agents may not catch the difference. This makes signing a priority rather than a convenience.

### Execution Isolation

koto runs as the current user with no elevated permissions. It reads template files, writes state files and cache entries to `~/.koto/`, and executes shell commands defined in command gates. The binary itself requires only filesystem read/write access to the working directory and `~/.koto/`.

Command gates (shell execution in templates) are the highest-risk surface. These are defined by template authors, not koto itself, and run with the user's full permissions. This is by design and documented in the template format specification. koto doesn't sandbox gate commands.

### Supply Chain Risks

Binaries are built by GitHub Actions from the public `tsukumogami/koto` repository. The build is reproducible given the same Go toolchain version and source commit. GoReleaser logs the exact build environment in each release.

The Homebrew formula points to GitHub Release assets, not a third-party CDN. The tap repository (`tsukumogami/homebrew-tap`) is controlled by the same organization. The tap repo should have branch protection on `main`, and `HOMEBREW_TOKEN` should be a fine-grained PAT scoped to the `homebrew-tap` repository with contents write permission only.

`go install` builds from source, eliminating binary supply chain concerns entirely. Users can audit the code before building.

### User Data Exposure

koto reads template files and writes state files. It doesn't phone home, collect telemetry, or transmit data externally. The cache stores compiled template JSON locally. State files contain workflow state (current state, evidence, variables) which may include user-provided values.

The template search path doesn't include world-writable directories. The default path (`~/.koto/`) is created with `0700` permissions, matching the existing cache directory behavior. Note that `MkdirAll` only sets permissions on newly created directories -- if `~/.koto/` already exists with more permissive modes, they won't be tightened. The implementation should check and warn if existing permissions are more permissive than `0700`.

### Template Search Path Poisoning

The project-local layer (`.koto/templates/`) exists in directories that may be shared via git, controlled by other contributors. A malicious template override is equivalent to arbitrary code execution via command gates. The mitigation is twofold: (1) the resolver prints a notice to stderr when a project-local template shadows a built-in, and (2) `--no-local` lets users skip the project-local layer entirely. Users should review `.koto/templates/` contents in shared repositories the same way they'd review scripts in `.github/workflows/`.

### Environment Variable Trust

koto trusts the value of `KOTO_HOME`. If this variable is set by an untrusted source (e.g., a `.env` file in a shared repo, CI environment injection), both the template search path and the cache are compromised. This is not a new attack surface -- the cache system already uses `KOTO_HOME` -- but the template search path amplifies the impact since templates compile into executable directives. Environment variable integrity is the user's responsibility, consistent with how other CLI tools treat `HOME`, `PATH`, and similar variables.

### Built-in Template Review

Built-in templates embedded via `go:embed` are part of the trusted computing base. Changes to `templates/` should receive the same scrutiny as Go source code changes, since a malicious template merged via PR would ship in every subsequent release. CI should flag PRs modifying `templates/` for required review.

## Consequences

### Positive
- Users can install koto with one command (`brew install` or `go install`)
- Built-in templates work immediately after install, no setup required
- Version reporting helps with bug reports and compatibility checks
- Template override model lets users customize without forking

### Negative
- Homebrew tap requires a separate repository (`tsukumogami/homebrew-tap`) and a `HOMEBREW_TOKEN` secret
- Built-in template updates require a new release (can't hot-patch)
- No Windows or Linux package manager support in initial release

### Mitigations
- Homebrew tap setup is a one-time cost, and GoReleaser automates ongoing maintenance
- The release process is lightweight (tag + push), so frequent releases are cheap
- Windows and Linux users can use `go install` or download binaries directly from GitHub Releases
