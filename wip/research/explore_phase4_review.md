# Phase 4 Review: koto Installation and Distribution

## Problem Statement Assessment

The problem statement is clear and specific. It identifies two distinct blockers: (1) there is no way to get the binary other than cloning and building from source, and (2) there is no mechanism for shipping built-in templates, so the first-run experience requires authoring before trying.

Both are real. The codebase confirms no `templates/` directory exists, no `.goreleaser.yml` exists, and no release workflow exists. The `internal/buildinfo` package already exists with version variables and `debug.BuildInfo` fallback, which means Phase 1 of the design is partially implemented. The design document does not acknowledge this -- it describes Phase 1 as new work when the `version.go` and `version_test.go` files are already in place.

The problem statement is strong on the "distribution" side but weaker on the "built-in template" side. It says "koto has no mechanism for shipping built-in templates" but doesn't quantify the cost of the current experience. How many commands does a new user need to run today to see koto do something? That would sharpen the evaluation of whether `go:embed` is worth the complexity versus, say, a `koto init --example` flag that generates a template on disk.

One gap: the problem statement treats "installation" and "built-in template distribution" as a single design, but they are loosely coupled. Installation (GoReleaser, Homebrew, `go install`) works independently of whether templates are embedded. Combining them is fine for a design document, but the scope section should be clearer that these are two separable decisions that happen to share a release vehicle.

**Verdict**: Specific enough to evaluate solutions. Minor gap on acknowledging existing implementation.

## Missing Alternatives

### Decision 1: Release Automation

The two rejected alternatives (manual scripts, raw GitHub Actions) are the obvious ones. One missing alternative worth documenting:

**goreleaser-action only (no local GoReleaser)**: Some projects skip the local `.goreleaser.yml` and use `goreleaser/goreleaser-action` with inline configuration. This is a variant, not a separate option -- but the design could note whether `.goreleaser.yml` is intended for local use (e.g., `goreleaser release --snapshot` for testing) or solely CI-driven. This affects developer experience when debugging release issues.

No significant missing alternatives here. GoReleaser is the clear choice for Go CLIs.

### Decision 2: Built-in Template Distribution

Missing alternative: **`go generate` with a template copy step**. Instead of embedding raw template source, a `go generate` step could compile templates to JSON at build time and embed the compiled output. This would avoid runtime compilation of built-in templates and make the binary's behavior independent of the compiler code path for built-ins.

This matters because the current design embeds source `.md` files, which means the binary must include the full compiler (`pkg/template/compile/`) in the hot path for loading built-in templates. If built-in templates were pre-compiled JSON, the binary could load them through `template.ParseJSON()` directly, bypassing compilation entirely. The compiler is only needed for user-authored templates.

Whether this is worth the complexity depends on the template count and compilation cost. For a single `quick-task` template, it doesn't matter. For 10+ built-in templates, the difference in startup time and code path complexity could be meaningful.

Missing alternative: **Asset download from releases**. GoReleaser supports "extra files" in release archives. Templates could ship as separate files in the archive (e.g., `templates/quick-task/template.md` alongside the binary), with the binary checking its own directory for templates. This preserves the single-archive property while keeping templates editable after installation. Rejected because it breaks `go install`, but that tradeoff should be stated.

### Decision 3: Template Search Path

Missing alternative: **`KOTO_TEMPLATE_PATH` environment variable**. Several tools (like `PATH`, `GOPATH`, `FPATH`) allow users to prepend or append custom directories to a search path. The current design has `KOTO_HOME` controlling one directory and a hardcoded three-layer search. A `KOTO_TEMPLATE_PATH` would let users add arbitrary directories (e.g., a shared team templates directory). The design could reject this explicitly -- security concerns with shared directories are valid -- but the alternative should appear.

Missing consideration: **project root detection**. The design says ".koto/templates/ relative to the current directory (or project root if detectable)" but doesn't specify how project root is detected. The codebase uses `git rev-parse --show-toplevel` in `engine.go:gitRepoRoot()`. If the search path uses the same mechanism, it should say so. If it uses a different mechanism (e.g., walking up to find `.koto/`), that's a new pattern.

## Rejection Rationale Review

### Decision 1 rejections

**Manual release scripts**: "Every Go project of note has moved to GoReleaser or something similar" is opinion presented as fact. Kubernetes, for example, uses custom release tooling. But for a project of koto's size, the statement is directionally correct. The rejection is fair.

**GitHub Actions only**: "Requires reimplementing archive creation, checksum generation, and Homebrew formula updates" -- this is accurate and specific. Good rejection.

### Decision 2 rejections

**Filesystem-only**: "Breaks the single-binary property" -- correct and specific. "go install can't distribute non-Go files" -- correct. Fair rejection.

**Hybrid with runtime download**: "Premature" is a judgment call, but "a template registry adds complexity (versioning, caching, authentication, CDN)" correctly enumerates the costs. The rejection says "for a problem that doesn't exist yet." This is fair for v0.1.0 but could note when the problem *would* exist (e.g., when third-party templates emerge).

### Decision 3 rejections

**XDG Base Directory layout**: "koto already uses ~/.koto/ (established by the cache system in pkg/cache/)" -- confirmed by reading `cache.go:17-26`. The cache uses `KOTO_HOME` or `~/.koto/cache/`. The rejection is grounded in code.

However, the claim "XDG adds a dependency (adrg/xdg or manual env var handling)" is slightly misleading. XDG compliance requires checking 3-4 environment variables (`XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`) with fallbacks -- this is ~10 lines of code, not a library dependency. The real reason to reject XDG is the existing `~/.koto/` convention, which the rejection does state. The dependency argument is weak padding.

**Single directory**: "Forces users to copy built-in templates" -- correct and sufficient. Fair rejection.

No rejections appear unfairly dismissive.

## Unstated Assumptions

### 1. Template names are globally unique across all search layers

The search path uses "first match wins" on template name. If a user has `.koto/templates/quick-task/` and the binary embeds `templates/quick-task/`, the project-local one wins. But the design doesn't address what happens when a user wants to use *both* the built-in and a project-specific template with the same name. There's no namespace mechanism (e.g., `builtin:quick-task` vs `quick-task`). This is probably fine for now but should be stated as a known limitation.

### 2. Templates are directories, not files

The search path checks `<name>/template.md`, implying each template is a directory containing a `template.md` file. This is a convention the design introduces but doesn't justify. Why a directory? Is it to support future multi-file templates (e.g., with assets or config)? Or is it structural convention? The choice constrains how templates are organized and should be explicit.

### 3. `go install` users don't need Homebrew-style upgrade management

The design provides `brew install` (with `brew upgrade`) and `go install` (with `go install @latest`). But `go install` has no built-in mechanism to check for updates or notify users of new versions. The design assumes users will manage their own upgrades, which is standard for Go tools but worth stating given that built-in template updates require a new release.

### 4. The `internal/resolve` package won't need to import `pkg/` packages

The design places template resolution in `internal/resolve/`. This package needs to access `embed.FS` (from the `templates/` directory embed in `cmd/koto/` or a top-level package). The design doesn't specify where the `//go:embed` directive lives. If it's in `cmd/koto/`, then `internal/resolve` needs to receive the embedded filesystem as a parameter. If it's in `internal/resolve/` itself, the `templates/` directory must be accessible from that package's location. This is an architectural dependency that needs to be resolved.

This is actually a structural concern. The `templates/` directory is at the repo root per the component diagram. Go's `//go:embed` can only embed files in the same directory or subdirectories of the file containing the directive. If `internal/resolve/resolve.go` has the embed directive, `templates/` must be a subdirectory of `internal/resolve/` or symlinked -- which is awkward. More likely the embed lives in `cmd/koto/` and gets passed into the resolver, but the design doesn't show this.

### 5. The `tsukumogami` GitHub organization exists and has the required permissions

The design references `tsukumogami/homebrew-tap` and `tsukumogami/koto`. It assumes the org structure supports: (a) creating a new `homebrew-tap` repo, (b) a `HOMEBREW_TOKEN` secret with cross-repo access, and (c) GoReleaser having permission to push to the tap repo. These are operational prerequisites, not design decisions, but they should be listed as prerequisites in the implementation section.

### 6. No CGO now means no CGO ever

The design says "koto has no runtime dependencies and no CGO." This is true today (`go.mod` has only `gopkg.in/yaml.v3`). But if a future feature requires CGO (e.g., SQLite for state storage, or a scripting engine), the single-static-binary property and the cross-compilation matrix break. This is an implicit constraint on future development that the design presents as a current fact without noting the forward commitment.

## Strawman Check

None of the rejected alternatives appear designed to fail.

**Manual release scripts** and **GitHub Actions only** are real alternatives that some projects use. The rejections are specific about the maintenance cost rather than dismissing them as conceptually broken.

**Filesystem-only** and **hybrid with runtime download** are reasonable ends of a spectrum. The design picks the middle (embed) and rejects both extremes for real reasons.

**XDG** and **single directory** are genuine alternatives in the template path space. XDG is especially credible -- many Linux CLI tools use it. The rejection is grounded in the existing `~/.koto/` convention, not in a misrepresentation of XDG's properties.

The design is not rigged toward a predetermined conclusion. The chosen options are defensible on their merits.

## Recommendations

1. **Acknowledge existing implementation**: Phase 1 (version infrastructure) is already partially implemented. `internal/buildinfo/version.go` exists with ldflags variables and `debug.BuildInfo` fallback. `cmd/koto/main.go` already has a `version` subcommand. The design should note what is done vs. what remains (the version output format described in the design differs slightly from the current implementation, which outputs `koto dev-<hash>` rather than `koto version dev (...)`).

2. **Specify where `//go:embed` lives**: The embed directive has a Go constraint -- it can only access files in or below the package directory. The design should specify whether `templates/` is embedded in `cmd/koto/` (and passed to `internal/resolve` as a parameter) or in `internal/resolve/` (requiring the templates directory to be colocated). This affects the dependency direction between packages.

3. **Consider pre-compiled embedding**: Instead of embedding source `.md` files that require runtime compilation, embed pre-compiled JSON. This skips the compiler for built-in templates and makes the `pkg/template/compile/` package unnecessary for users who only use built-ins. Add this as a considered alternative even if rejected for v0.1.0.

4. **Clarify project root detection for search path**: The design says "project root if detectable" without specifying the detection mechanism. The codebase already has `gitRepoRoot()` in `engine.go`. State whether the search path reuses this mechanism or introduces a new one (e.g., walking up to find `.koto/`). If it's a new mechanism, that's a second pattern for root detection -- flag it for unification.

5. **Add `KOTO_TEMPLATE_PATH` as a rejected alternative**: Even if the answer is "no, security risk from shared directories," documenting the rejection prevents the question from recurring.

6. **Make the template-as-directory convention explicit**: The search path looks for `<name>/template.md`, not `<name>.md`. This directory-based convention has implications for template authoring and discovery. State the rationale (future multi-file support, or structural convention).

7. **List operational prerequisites**: The implementation section should list: (a) create `tsukumogami/homebrew-tap` repo, (b) configure `HOMEBREW_TOKEN` secret, (c) verify GoReleaser has cross-repo push access. These are not design decisions but they block Phase 2.
