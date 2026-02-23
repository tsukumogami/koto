---
status: Proposed
problem: |
  koto's template compiler and evidence system are fully implemented as Go libraries
  but unreachable from the command line. Templates reach users through two distinct
  paths -- deployed by skills/plugins (Path 1) or authored manually (Path 2) -- but
  neither path has CLI support. The init command uses a legacy parser, the transition
  command has no evidence flag, and there are no authoring tools.
decision: |
  koto init compiles templates implicitly and caches the result for deployed templates
  (Path 1). Template authors (Path 2) get a feedback loop via koto template compile,
  inspect, and list commands. A search path (project-local, user-global) serves
  name-based resolution for authored templates. koto transition gets --evidence flags.
  LLM-assisted validation is deferred to a future design.
rationale: |
  Separating the two distribution paths avoids forcing one path's UX on the other.
  Deployed templates want silent, cached compilation. Authored templates want feedback.
  Both paths use the same compiler; the difference is what koto does around the
  compilation. The evidence flag is the minimum change that unblocks gate-based workflows.
---

# DESIGN: koto CLI and Template Tooling

## Status

**Proposed**

## Upstream Design Reference

This design builds on [DESIGN-koto-template-format.md](DESIGN-koto-template-format.md), which specifies the source format, compiled format, and compilation rules. That design explicitly deferred CLI/tooling concerns:

> The CLI/tooling layer (compile commands, search paths, linter) needs its own design that builds on this format specification.

Relevant decisions from the upstream design:
- Source format is YAML frontmatter + markdown body (Decision 3)
- Compiled format is JSON, parsed with Go stdlib only (Decision 2)
- LLMs must never be in the compilation path (Design Boundary)
- Compilation is deterministic: same source always produces same output

## Context and Problem Statement

The template format specification is fully implemented: the compiler converts source templates to JSON, the engine evaluates gates (field checks and commands), and evidence accumulates across transitions. But none of this is reachable from the CLI.

Today, `koto init --template <path>` loads templates through a legacy parser (`pkg/template/Parse()`) that predates the format specification. This parser can't handle evidence gates, nested YAML structures, or the source/compiled separation. Meanwhile, `koto transition <target>` has no `--evidence` flag, so states with gates are unreachable from the command line. The only way to use the new template features is through Go code.

Three specific gaps block real-world usage:

1. **No compilation path in the CLI.** The compiler exists as `pkg/template/compile.Compile()` but there's no way to invoke it from `koto`. Users can't compile templates, inspect compiled output, or validate source files.

2. **No template discovery.** `--template` requires a file path. There's no way to say `koto init --template quick-task` and have koto find the template. Projects can't ship templates alongside their code in a discoverable way.

3. **No evidence from the CLI.** The engine's `WithEvidence()` option works, but `koto transition` doesn't expose it. Any state with a `field_not_empty` or `field_equals` gate is a dead end from the command line.

### Distribution Paths

Before designing the CLI, we need to understand how templates reach the user's machine. There are two distinct paths, and each has different UX needs.

**Path 1: Deployed templates (installed from an external source)**

A template ships as part of a larger package -- a Claude Code plugin, a project's `.claude/` directory, or a shared skill library. The template file is a supporting artifact alongside the skill/command that uses it. The skill already knows the template's location (it's a sibling file or a path in its configuration). Example: a `/work-on` skill installed via a Claude Code plugin includes a `work-on.md` template in its own directory.

In this path:
- The template is expected to be valid (tested by the author before distribution)
- The caller passes an explicit path to `koto init --template <path>`
- Name-based search is irrelevant -- the skill knows where its template lives
- Compilation should be fast and can be cached (the template doesn't change between runs)
- Error messages are unexpected; when they occur, they indicate a broken installation

**Path 2: User-authored templates (created manually)**

A user writes a template for their own project or for personal use. They're iterating: writing the source, compiling, hitting errors, fixing them, inspecting the output. The template might live in a project directory or in `~/.koto/templates/`.

In this path:
- The template is expected to have errors (the user is developing it)
- Compilation feedback is the primary value -- clear errors, warnings, and a way to inspect the result
- Name-based search matters for convenience (`koto init --template my-workflow` instead of a full path)
- Caching is counterproductive (the source changes on every edit)
- `koto template compile`, `inspect`, and `list` are the authoring tools

These two paths have different priorities but share the same underlying compilation machinery. The design should handle both without forcing one path's UX on the other.

### Scope

**In scope:**
- How `koto init` compiles and loads templates (implicit compilation, both paths)
- Compilation caching for deployed templates (Path 1)
- Template authoring tools: `koto template compile`, `inspect`, `list` (Path 2)
- Template search path resolution for user-authored templates (Path 2)
- `koto transition --evidence key=value` flag (both paths)
- Deprecation of the legacy `Parse()` path

**Out of scope:**
- LLM-assisted validation or linting (deferred to its own design)
- Template distribution mechanism (plugins, registries -- that's the distribution platform's job)
- Built-in template content (that's the quick-task template, separate work)
- Template versioning or dependency resolution
- Changes to the engine or compiler packages (those are done)

## Decision Drivers

- **Deployed templates stay invisible**: For Path 1, `koto init --template <path>` should compile and start the workflow with no user-visible overhead. The caller (a skill or script) passes an explicit path; koto compiles, caches, and gets out of the way.
- **Authoring templates provides feedback**: For Path 2, the user is iterating. Compilation errors, warnings, and inspection tools are the primary value. The feedback loop matters more than speed.
- **Both paths use the same compiler**: One compilation pipeline, two UX modes. No separate "dev mode" compiler.
- **No new dependencies in the engine**: The engine reads compiled JSON with stdlib only. All new dependencies (if any) stay in the CLI or compiler packages.
- **Backward compatibility**: Existing `koto init` invocations that pass a file path should keep working during the transition.
- **Evidence accessibility**: Gate-based workflows need evidence from the CLI. This is the minimum viable interface.

## Considered Options

### Decision 1: Compilation Flow

`koto init --template <path>` needs to compile the source template before starting the workflow. The two distribution paths have different needs: deployed templates (Path 1) should compile silently with optional caching, while user-authored templates (Path 2) should provide feedback and never cache stale results.

#### Chosen: Implicit compilation with optional caching

`koto init` always compiles the template automatically. The caller passes a source path (or name, resolved via search path for Path 2), koto compiles it to JSON, validates it, and starts the workflow.

**Path 1 (deployed):** The compiled output can be cached in `~/.koto/cache/` keyed by source file hash. On subsequent runs, if the source hash matches a cached entry, koto skips compilation and uses the cached JSON. This is a performance optimization for templates that don't change between runs -- a skill calling `koto init` repeatedly with the same template shouldn't pay compilation cost each time. Cache invalidation is by source hash: if the source changes, the cache misses and recompilation happens.

**Path 2 (authoring):** No caching by default. The user is editing the source between runs, so the cache would miss every time anyway. The `koto template compile` and `koto template inspect` commands provide the feedback loop: compile to see errors, inspect to see the result.

The existing `koto validate` command (which checks template hash against state file) continues to work. It recompiles the source template at validation time and compares the hash.

#### Alternatives Considered

**Explicit compilation required**: Users run `koto template compile` first, then `koto init --compiled output.json`. This is how many build systems work, but it adds a mandatory step to Path 1 (where the caller just wants to start a workflow) and adds friction to Path 2 (where the user is iterating).
Rejected because it makes both paths harder without clear benefit.

**Cache next to source file**: `koto init` writes `.compiled.json` files alongside the source template. This pollutes the user's project directory and plugin directories with side effects. Path 1 callers shouldn't need to grant write access to the template directory.
Rejected because caching belongs in koto's own directory, not the template's directory.

### Decision 2: Template Search Path

Path 1 (deployed) always passes explicit paths -- the skill knows where its template lives. Search paths exist for Path 2 (authoring), where a user wants to say `--template my-workflow` and have koto find it in a known location.

#### Chosen: Explicit path detection with project-local and user-global search

koto resolves template names through a fixed search order:

1. **Explicit path**: If `--template` contains a `/` or ends in `.md`, treat it as a file path (absolute or relative to CWD). No search. This is the only mode Path 1 uses.
2. **Project-local**: Look for `./templates/<name>.md` and `./.koto/templates/<name>.md` relative to the git root (or CWD if not in a git repo).
3. **User-global**: Look for `~/.koto/templates/<name>.md`.

First match wins. If no match is found, return an error listing the paths searched.

The `.md` extension is added automatically when searching by name. `--template my-workflow` looks for `my-workflow.md`.

#### Alternatives Considered

**Configurable search path (environment variable)**: A `KOTO_TEMPLATE_PATH` variable listing directories to search, like `PATH` or `GOPATH`. Premature -- Path 2 users authoring their own templates don't have enough variety to justify custom search paths. Users with unusual directory layouts pass explicit paths (which is what Path 1 does anyway).
Rejected because the fixed hierarchy covers Path 2 without configuration burden.

**Explicit paths only**: No search path, always require a file path. This is already how Path 1 works. For Path 2, it means `koto init --template ./templates/my-workflow.md` instead of `koto init --template my-workflow`. The extra typing is minor, but establishing project-local conventions (`./templates/`) lets `koto template list` show what's available.
Rejected because the search path has low implementation cost and enables `template list`.

### Decision 3: LLM-Assisted Validation

The upstream design mentions "optional LLM-assisted validation" for helping authors write correct source. Should this design include a `koto template lint` command?

#### Chosen: Defer entirely

LLM validation is out of scope for this design. The compiler already validates source files and produces clear error messages. A linter that uses LLMs to suggest fixes (ambiguous headings, missing states, unclear directives) is a separate feature with its own design surface: model selection, API keys, cost, offline behavior, prompt engineering.

The `koto template compile` command serves as a basic validator. If the source is invalid, compilation fails with a specific error. If it produces warnings (heading collisions), those are printed. This covers the mechanical validation needs.

#### Alternatives Considered

**Design a lint command stub**: Define `koto template lint` now with deterministic checks only, leaving LLM integration for later. Adds a command that duplicates what `compile` already does (source validation). Having both `compile` and `lint` with overlapping scope is confusing.
Rejected because compile already validates, and the LLM surface is the interesting part of linting.

**Full linter design**: Design the LLM integration surface now. Premature -- we don't know which LLM checks are useful, what the prompt engineering looks like, or whether users want inline suggestions vs batch validation.
Rejected because we'd be designing around unknowns.

### Decision 4: Template Authoring Commands

Path 2 users need tools for the feedback loop: compile to see errors, inspect to see the result, list to see what's available. These are authoring tools that Path 1 callers (skills, scripts) don't need. How should they be organized?

#### Chosen: `koto template` subcommand group

Authoring operations live under `koto template <subcommand>`:

- `koto template compile <path>` -- compile source to JSON, write to stdout. The primary feedback tool: shows errors, warnings, and the compiled output.
- `koto template inspect <path>` -- human-readable summary of a template. Shows states, transitions, gates, and variables without the full JSON.
- `koto template list` -- show available templates from the search path (project-local and user-global).

This groups authoring tools together and keeps the top-level namespace reserved for workflow operations (`init`, `transition`, `next`, etc.). Path 1 callers never need these commands.

#### Alternatives Considered

**Flat commands**: `koto compile`, `koto inspect`, `koto list-templates`. Fewer keystrokes, but clutters the top-level namespace with authoring tools that most users (Path 1) don't use.
Rejected because the top-level namespace should be reserved for workflow operations.

## Decision Outcome

### Summary

Templates reach users through two distribution paths, and the CLI serves each differently.

**Path 1 (deployed templates):** A skill or script calls `koto init --template <explicit-path>`. koto compiles the source, caches the compiled JSON in `~/.koto/cache/` (keyed by source hash), and starts the workflow. On subsequent runs with the same template, the cache eliminates compilation. The caller never interacts with template authoring tools.

**Path 2 (user-authored templates):** A user developing their own template uses `koto template compile` and `koto template inspect` as their feedback loop. When ready, they run `koto init --template my-workflow` and koto finds the template via search path (project-local, then user-global). No caching -- the source changes on every edit.

**Both paths:** `koto transition` gets a `--evidence key=value` flag (repeatable) that passes evidence to the engine's `WithEvidence()` option. This unblocks all gate-based workflows from the CLI. The legacy `Parse()` function is deprecated; `koto init` switches to the compiler path internally.

### Rationale

Separating the two distribution paths avoids forcing one path's UX on the other. Path 1 callers (skills, scripts) want silent compilation with no overhead -- caching serves this. Path 2 users (template authors) want feedback -- authoring tools serve this. Both paths use the same compiler; the difference is in what koto does around the compilation (cache vs feedback).

The search path exists for Path 2 only. Path 1 always uses explicit paths because the caller already knows where the template is. This keeps the search path simple and avoids confusing interactions with plugin directories.

Deferring LLM validation avoids designing around unknowns. The compiler's error messages are the validation layer for Path 2 authoring. When we understand which LLM checks are useful, that becomes its own design.

### Trade-offs Accepted

- **Cache adds filesystem state**: `~/.koto/cache/` accumulates compiled templates. This is bounded (one file per unique source hash, small files) and can be cleared manually. The alternative is recompiling on every `koto init`, which is fast but unnecessary for unchanged templates.
- **No configurable search path**: The fixed hierarchy can't be customized. Path 1 doesn't need it (explicit paths), and Path 2 users with unusual layouts pass explicit paths.
- **No linter**: Template authors get compiler errors but no suggestions or LLM-powered fixes. The compiler's error messages are specific enough for Phase 1.
- **Legacy parser not removed**: `Parse()` is deprecated but not deleted. Removing it is a separate cleanup once all callers are migrated.

## Solution Architecture

### Modified Commands

#### `koto init`

The init command changes internally but keeps the same interface:

```
koto init --template <path-or-name> --name <workflow-name> [--var KEY=VALUE]... [--state-dir <dir>]
```

New behavior:
1. Resolve `--template` argument (path detection vs name search)
2. Hash the source file
3. Check `~/.koto/cache/` for a cached compilation matching the source hash
4. On cache miss: compile via `compile.Compile(sourceBytes)`, write to cache
5. Validate via `template.ParseJSON(compiledJSON)`
6. Build `engine.Machine` from `CompiledTemplate.BuildMachine()`
7. Compute compiled hash via `compile.Hash(compiledJSON)` for state file
8. Call `engine.Init()` with the machine and metadata

The `--template` argument resolution:
- Contains `/` or ends in `.md` → treat as file path (Path 1: deployed templates always use this)
- Otherwise → search by name: `<name>.md` through the search path (Path 2: user-authored)

Template names are flat strings (no directory nesting). `--template myorg/quick-task` triggers path mode because of the `/`. Namespaced template names aren't supported in Phase 1.

#### `koto transition`

Adds `--evidence` flag:

```
koto transition <target> [--evidence KEY=VALUE]... [--state <path>] [--state-dir <dir>]
```

The `--evidence` flag is repeatable. Each `KEY=VALUE` pair is parsed and passed to `engine.Transition()` via `WithEvidence()`.

#### `koto validate`

Currently checks template hash. Updated to use the compiler path:
1. Read state file to get `template_path`
2. Recompile the source template
3. Compare hash against state file's `template_hash`

### New Commands (Path 2: Authoring Tools)

These commands serve template authors during development. Path 1 callers (skills, scripts) don't use them.

#### `koto template compile`

```
koto template compile <path> [--output <file>]
```

Compiles a source template and writes the compiled JSON to stdout (or `--output` file). Prints warnings to stderr. Exits non-zero on compilation error.

This is the primary feedback tool for Path 2. The author edits their template, runs `compile`, sees errors or the compiled output, and iterates.

Use cases:
- Authoring feedback: see compilation errors and warnings as you develop
- CI: validate templates as part of a build pipeline
- Debug: pipe to `jq` for ad-hoc queries on the compiled structure

#### `koto template inspect`

```
koto template inspect <path>
```

Shows a human-readable summary of a template:

```
Name:          quick-task
Version:       1.0
Description:   A focused task workflow
Initial State: assess
States:        assess, plan, implement, done, escalate
Variables:     TASK (required), REPO (default: ".")
Gates:         assess/task_defined (field_not_empty), implement/tests_pass (command)
Terminal:      done, escalate
```

Useful for reviewing what a template does without reading the raw source or full compiled JSON.

#### `koto template list`

```
koto template list [--search-dir <dir>]
```

Lists user-authored templates from the search path:

```
NAME          VERSION  DESCRIPTION                LOCATION
quick-task    1.0      A focused task workflow     ./templates/quick-task.md
code-review   1.0      Code review workflow        ~/.koto/templates/code-review.md
```

The `--search-dir` flag adds an additional search directory (useful for testing). This command only scans the search path directories (project-local and user-global). It doesn't find deployed templates inside plugin directories -- those are managed by the distribution platform, not koto.

### Compilation Cache

Compiled templates are cached in `~/.koto/cache/` to avoid redundant compilation. This primarily benefits Path 1 (deployed templates that don't change between runs) but works for both paths.

Cache key: SHA-256 of the source file contents. Cache value: compiled JSON.

```
~/.koto/cache/
├── a1b2c3d4...json    # compiled output, keyed by source hash
├── e5f6g7h8...json
└── ...
```

Behavior:
- `koto init`: checks cache before compiling, writes to cache on miss
- `koto template compile`: always compiles fresh (authoring tool, skips cache)
- `koto template inspect`: always compiles fresh (authoring tool, skips cache)
- `koto validate`: always compiles fresh (needs to detect source changes)

The cache has no expiration. Old entries accumulate but are small (compiled JSON is a few KB). A `koto cache clear` command (or just `rm -rf ~/.koto/cache/`) handles cleanup if needed.

### Template Search Path

Resolution order (first match wins):

1. **Explicit path**: Input contains `/` or ends in `.md`
2. **Project-local**:
   - `<git-root>/templates/<name>.md`
   - `<git-root>/.koto/templates/<name>.md`
3. **User-global**:
   - `~/.koto/templates/<name>.md`

Git root is detected via `git rev-parse --show-toplevel`. If not in a git repo, CWD is used instead.

When resolving by name, if a project-local match shadows a user-global template with the same name, koto prints a note to stderr: `note: using <local-path> (shadows <global-path>)`. This makes the precedence visible without breaking scripts that parse stdout.

For `template list`, all search path directories are scanned. Duplicate names show only the first match (same precedence as resolution).

### Package Changes

#### `cmd/koto/main.go`

- Add `template` subcommand dispatcher
- Add `cmdTemplateCompile`, `cmdTemplateInspect`, `cmdTemplateList` handlers
- Modify `cmdInit` to use compiler path with cache lookup
- Add `--evidence` flag to `cmdTransition`
- Add `resolveTemplatePath()` for search path logic

#### `pkg/template/template.go`

- Add deprecation comment to `Parse()` function
- No code changes (backward compat)

#### New: `pkg/resolve/resolve.go`

Template search path resolution:

```go
package resolve

// Resolve finds a template by name or path.
// If input contains "/" or ends in ".md", it's treated as a path.
// Otherwise, search: project-local then user-global.
func Resolve(input string) (string, error)

// List returns all templates found in the search path.
func List() ([]TemplateInfo, error)

// TemplateInfo holds metadata about a discovered template.
type TemplateInfo struct {
    Name        string
    Version     string
    Description string
    Path        string
}
```

The `List()` function reads YAML frontmatter from each `.md` file in the search directories to extract name, version, and description. It doesn't compile the templates.

#### New: `pkg/cache/cache.go`

Compilation cache backed by `~/.koto/cache/`:

```go
package cache

// Get returns the cached compiled JSON for the given source hash, or nil on miss.
func Get(sourceHash string) ([]byte, error)

// Put stores compiled JSON keyed by source hash.
func Put(sourceHash string, compiledJSON []byte) error

// Clear removes all cached compilations.
func Clear() error
```

### Hash Migration

The legacy `Parse()` hashes the raw source file. The compiler hashes the compiled JSON. These produce different values for the same template. Workflows initialized with the legacy parser will have old-format hashes, and the new compiler path will compute different ones.

This is a breaking change. The migration strategy:

1. `koto init` always uses the new compiler hash going forward. New workflows get compiler hashes.
2. `koto validate` and `loadTemplateFromState()` try the compiler hash first. If it doesn't match, fall back to hashing the raw source file (legacy mode). If the legacy hash matches, print a warning: `note: workflow uses legacy template hash; re-init to upgrade`.
3. No automatic migration. Users who need the new hash can re-initialize their workflow.

This dual-hash check is confined to the hash comparison logic. Once all legacy workflows age out, the fallback can be removed.

### Post-Init Command Migration

Six commands besides `init` load templates through `loadTemplateFromState()`: `transition`, `next`, `rewind`, `validate`, `cancel`, and `status`. Today this function calls the legacy `Parse()`. It must migrate to the compiler path atomically with `cmdInit` -- if init stores compiler hashes but post-init commands compute legacy hashes, every operation fails with `template_mismatch`.

The migration updates `loadTemplateFromState()` to:
1. Read the source file from the path stored in the state file
2. Compile via `compile.Compile(sourceBytes)`
3. Validate via `template.ParseJSON(compiledJSON)`
4. Build the controller's `Template` via `CompiledTemplate.ToTemplate()` (see below)
5. Hash comparison uses the dual-hash strategy described above

### Controller Adapter

The `controller.Controller` takes a `*template.Template`, but the compiler produces a `*template.CompiledTemplate`. Rather than changing the controller interface, add an adapter method:

```go
// ToTemplate converts a CompiledTemplate to a legacy Template struct
// for use with the controller. Sections are populated from StateDecl.Directive
// fields, Variables from VariableDecl, and the Machine from BuildMachine().
func (ct *CompiledTemplate) ToTemplate() (*Template, error)
```

This keeps the controller unchanged and confines the migration to the CLI and template packages.

### Evidence Parsing

The `--evidence` flag accepts `KEY=VALUE` format. Parsing splits on the first `=`:

```go
func parseEvidence(flags []string) (map[string]string, error)
```

- `key=value` → `{"key": "value"}`
- `key=` → `{"key": ""}` (empty value, valid)
- `key` (no `=`) → error: `"invalid evidence format %q: expected KEY=VALUE"`
- `=value` (empty key) → error: `"invalid evidence format %q: key must not be empty"`

## Implementation Approach

### Phase 1a: Compiler Path Migration (atomic)

This phase must land as a single unit -- `cmdInit` and `loadTemplateFromState()` must migrate together to avoid hash mismatches. After this phase, both distribution paths use the compiler.

- Add `CompiledTemplate.ToTemplate()` adapter in `pkg/template/`
- Modify `cmdInit` to use compiler path (`compile.Compile` + `template.ParseJSON` + `BuildMachine`)
- Modify `loadTemplateFromState()` to use compiler path with `ToTemplate()` adapter
- Implement dual-hash comparison (compiler hash with legacy fallback)
- Update `koto validate` to use compiler path

### Phase 1b: Evidence Flag

- Add `--evidence` flag to `cmdTransition`
- Implement `parseEvidence()` for `KEY=VALUE` parsing
- Wire evidence into `engine.Transition()` via `WithEvidence()`

### Phase 2a: Template Search Path (Path 2 support)

- Create `pkg/resolve/` package for template resolution
- Add `resolveTemplatePath()` with search path logic and shadow warnings
- Wire search path into `cmdInit` for name-based resolution

### Phase 2b: Template Authoring Commands (Path 2 tools)

- Add `template` subcommand dispatcher
- Add `koto template compile` command (always fresh, no cache)
- Add `koto template inspect` command
- Add `koto template list` command

### Phase 3: Compilation Cache (Path 1 optimization)

- Create `pkg/cache/` package for `~/.koto/cache/`
- Integrate cache into `cmdInit` (check before compile, store on miss)
- Authoring commands (`compile`, `inspect`) bypass cache

### Phase 4: Cleanup

- Add deprecation notice to `Parse()` function
- Update integration tests to use new source format

## Security Considerations

### Download Verification

Not applicable. This design doesn't download templates from external sources. Templates are local files read from the filesystem. The template search path looks only at local directories (project-local and user home). No network requests are made.

### Execution Isolation

Template compilation doesn't execute any code. The compiler parses YAML and extracts markdown -- no shell commands, no file writes beyond the state file.

Command gates (defined in the template) execute during transitions, but that's the engine's responsibility (already implemented in #17). The CLI's `--evidence` flag only passes data to the engine; it doesn't execute commands.

The template search path reads files from predictable locations (`./templates/`, `~/.koto/templates/`). It follows symlinks by default (Go's `os.ReadFile` behavior). The search path is not configurable via environment variables, limiting the attack surface for path injection.

### Supply Chain Risks

Templates are local files, not downloaded packages. The trust model matches Makefiles: the project owner defines templates, users review them before use. There's no template registry, no automatic updates, and no dependency resolution.

The risk is that a user runs `koto init` with a template they haven't reviewed. This is the same risk as running `make` in an untrusted repo. Mitigation: `koto template inspect` shows what a template does before init. Command gates are visible in the inspect output.

### User Data Exposure

The `--evidence` flag passes key-value pairs to the engine's state file. Evidence is stored in the local state file (JSON on disk). No evidence is transmitted externally.

Template compilation reads the source file and produces JSON. No data leaves the machine. The compiled output contains only what's in the source file.

`koto template list` reads YAML frontmatter from files in the search path. It doesn't transmit any data.

### Evidence and Directive Interpolation

The controller merges evidence into the interpolation context used for directive text (`{{KEY}}` placeholders). The namespace collision check blocks evidence from shadowing declared variables, but undeclared placeholder keys in directives can be set via `--evidence`. In the single-user CLI, this is fine -- the user controls both the evidence and the template. But if koto is used in pipelines where evidence comes from untrusted external sources, this becomes a way to rewrite agent instructions. This risk is noted for future design consideration; no code changes are needed for single-user CLI usage.

### Mitigations

| Risk | Mitigation | Residual Risk |
|------|------------|---------------|
| Malicious template in search path | `koto template inspect` shows gates before init | User must remember to inspect |
| Symlink in template directory | Go's os.ReadFile follows symlinks | Template could reference unexpected file |
| Evidence containing sensitive data | State files created with 0600 permissions (via os.CreateTemp) | Sensitive evidence still on disk |
| Search path collision (local shadows global) | Shadow warning printed to stderr | User may ignore the warning |
| Evidence as interpolation vector | Namespace collision check blocks declared vars | Undeclared placeholders can be set via evidence |

## Consequences

### Positive

- Gate-based workflows become usable from the CLI (the `--evidence` flag unblocks them)
- Path 1 (deployed): compilation cache eliminates redundant work for skills that call `koto init` repeatedly
- Path 2 (authoring): `compile`, `inspect`, and `list` provide a feedback loop for template development
- The compiler path replaces the legacy parser, using the validated format specification
- Both distribution paths use the same compiler, avoiding divergent behavior

### Negative

- Two template loading paths exist during the transition (legacy `Parse()` and new compiler)
- Compilation cache adds filesystem state (`~/.koto/cache/`) that can grow unbounded
- No LLM validation means template authors rely on compiler error messages only

### Mitigations

- Legacy `Parse()` gets a deprecation notice pointing to the compiler path
- Cache files are small and clearable (`rm -rf ~/.koto/cache/` or future `koto cache clear`)
- Compiler error messages are already specific and actionable (13 validation rules)
