# Architecture Review: koto Installation and Distribution

## Findings

### 1. `internal/resolve` bypasses existing template loading path -- Blocking

The design places the new template search path resolver in `internal/resolve/`, but the existing `cmdInit` in `cmd/koto/main.go` (line 110-238) already has a template loading pipeline: read source bytes, hash, cache lookup, compile, validate. The `loadTemplateFromState` helper (line 598-679) is a second copy of that same pipeline for non-init commands.

The design's `FindTemplate(name string) ([]byte, string, error)` returns raw source bytes and a path. After `FindTemplate` returns, the caller still needs the full compile/cache/validate pipeline. But the design doesn't show how `FindTemplate` integrates with the existing `cmdInit` flow or `loadTemplateFromState`. These two paths accept a filesystem path today; `FindTemplate` introduces a name-based lookup that returns bytes, which is a different interface contract.

This will create a third template loading path unless the design explicitly refactors the existing two into a shared function that `FindTemplate` feeds into. Without that, `cmdInit` will have its own path-based loading, `loadTemplateFromState` will have its own path-based loading, and name-based lookup will be a disconnected third path.

**Recommendation**: The design should specify that Phase 3 extracts the compile/cache/validate pipeline from `cmdInit` into a shared function (e.g., `loadTemplate(source []byte) (*template.CompiledTemplate, error)`), and `FindTemplate` feeds source bytes into that shared function. Alternatively, `FindTemplate` could return a `*template.CompiledTemplate` directly, encapsulating the full pipeline.

### 2. `internal/resolve` vs `pkg/` package placement -- Blocking

koto's public API lives in `pkg/` (engine, controller, template, cache, discover). The design puts the template resolver in `internal/resolve/`. But template search path resolution is a capability that library consumers need -- anyone embedding koto's engine in their own CLI needs to resolve template names the same way.

Every existing package that consumers interact with is in `pkg/`. Putting the resolver in `internal/` breaks the pattern and makes name-based template lookup unavailable to Go library users.

**Recommendation**: Place the resolver in `pkg/resolve/` (or `pkg/template/resolve/`). If there's a reason to keep it internal (e.g., it depends on `embed.FS` from the binary), the design should state that explicitly and explain why library consumers don't need this capability.

### 3. Version variables placed in `cmd/koto/` vs existing `internal/buildinfo` -- Advisory

The design says version variables go in `cmd/koto/` (line 136-143: "Three package-level variables in `cmd/koto/`"). But the codebase already has `internal/buildinfo/version.go` with `version` and `commit` variables, a `Version()` function, and `debug.BuildInfo` fallback logic. The existing implementation matches what the design describes almost exactly.

The design appears to have been written without awareness that Phase 1 is already implemented. The `koto version` subcommand already exists in `cmd/koto/main.go` (line 29-30) and calls `buildinfo.Version()`.

**Recommendation**: Acknowledge that Phase 1 is complete. The design's architecture section should reference `internal/buildinfo/` rather than proposing new variables in `cmd/koto/`. The GoReleaser ldflags should target `github.com/tsukumogami/koto/internal/buildinfo.version` and `github.com/tsukumogami/koto/internal/buildinfo.commit`, not `main.version` and `main.commit`.

### 4. `go:embed` directive placement unclear -- Advisory

The design says the `go:embed` directive embeds `templates/` from the source tree and that it's used in `internal/resolve/`. But `go:embed` can only reference files in or below the package directory that contains the directive. If `internal/resolve/resolve.go` contains the `//go:embed` directive, it can only embed files in `internal/resolve/` and its subdirectories -- not a top-level `templates/` directory.

The component list shows `templates/` at the repository root. To embed this from `internal/resolve/`, you'd need the embed variable declared in a package at or above the `templates/` directory (e.g., `cmd/koto/` or a package at the repo root), then passed to the resolver.

**Recommendation**: Specify where the `//go:embed` directive lives. The natural home is `cmd/koto/` (since it's at the right level in the directory tree), with the `embed.FS` passed to the resolver's constructor. The resolver package itself should accept an `fs.FS` parameter rather than directly embedding, which also makes it testable.

### 5. Phase sequencing is mostly correct but Phase 2 has a soft dependency on Phase 1 completeness -- Advisory

Phase 1 (version infrastructure) is already done. Phase 2 (GoReleaser + release workflow) depends on Phase 1's ldflags targets, which exist. Phase 3 (template search path) and Phase 4 (built-in template) are correctly ordered -- you need the search path before you can test built-in templates.

One gap: Phase 2 says "Tag and test first release (v0.1.0)" but doesn't specify what minimum functionality v0.1.0 needs. The template search path (Phase 3) isn't in v0.1.0 by this sequencing, which means the first release ships without the name-based template lookup or built-in templates. That's fine if intentional, but the design should state this explicitly so the version number doesn't create an expectation of completeness.

### 6. `--template` flag semantics change not addressed -- Advisory

Today, `koto init --template` takes a filesystem path (line 117 in main.go). With the template search path, users should also be able to pass a template name (e.g., `koto init --template quick-task`). The design doesn't specify whether `--template` accepts both names and paths, or whether a new flag (e.g., `--template-name`) is added.

This is a CLI surface concern. If `--template` starts accepting names, the resolution logic needs to distinguish paths from names (e.g., presence of `/` or `.md` extension). If a separate flag is added, the `koto init` help text changes.

**Recommendation**: Specify the CLI contract. A reasonable approach: if the value contains a path separator or ends in `.md`, treat it as a path; otherwise, treat it as a name and run through `FindTemplate`. Document this in the design so implementers don't have to guess.

### 7. No simpler alternatives were overlooked -- Informational

The design considered the right trade-offs. `go:embed` for built-in templates is the correct choice for a single-binary Go CLI. GoReleaser is standard practice. The three-layer search path is the minimum viable layering. The only alternative worth considering (XDG layout) was correctly rejected given the existing `~/.koto/` convention.

## Recommendations

1. **Refactor template loading before adding the resolver** (Finding 1). Extract the compile/cache/validate pipeline from `cmdInit` and `loadTemplateFromState` into a shared function. The resolver feeds into this function.

2. **Place the resolver in `pkg/`** (Finding 2). Use `pkg/resolve/` and accept an `fs.FS` parameter for the built-in layer, keeping the package testable and available to library consumers.

3. **Update the design to reflect Phase 1 completion** (Finding 3). Remove the Phase 1 implementation steps. Update the GoReleaser ldflags to target `internal/buildinfo`.

4. **Specify the `go:embed` location** (Finding 4). Declare the `embed.FS` in `cmd/koto/` and pass it to the resolver via an `fs.FS` interface.

5. **Define `--template` name vs path resolution** (Finding 6). Add a subsection to the CLI surface describing how `koto init` distinguishes template names from template paths.
