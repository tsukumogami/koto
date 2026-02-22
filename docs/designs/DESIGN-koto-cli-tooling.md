---
status: Proposed
problem: |
  koto's template compiler and evidence system are fully implemented as Go libraries
  but unreachable from the command line. Users can't compile templates, find templates
  by name, or supply evidence during transitions. The CLI's init command still uses
  the legacy parser that can't handle gates or nested YAML. This gap blocks all
  real-world usage of the template format.
decision: |
  Implicit compilation at init time with an explicit compile command for debugging.
  Templates are found via a three-level search path (explicit path, project-local
  ./templates/, user-global ~/.koto/templates/). The transition command gets --evidence
  flags. A template inspect command shows compiled output for debugging. LLM-assisted
  validation is deferred entirely to a future design.
rationale: |
  Implicit compilation keeps the simple case simple (koto init --template quick-task
  just works) while the explicit compile command serves debugging and CI. The search
  path follows the same convention as PATH, Go modules, and npm: local overrides
  global. Deferring LLM validation avoids designing an integration surface we don't
  need yet. The evidence flag is the minimal change that unblocks gate-based workflows.
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

### Scope

**In scope:**
- How `koto init` compiles and loads templates (implicit compilation)
- Template search path resolution (how koto finds templates by name)
- `koto template compile` and `koto template inspect` commands
- `koto transition --evidence key=value` flag
- Deprecation of the legacy `Parse()` path
- `koto template list` for discovering available templates

**Out of scope:**
- LLM-assisted validation or linting (deferred to its own design)
- Template registry or distribution mechanism
- Built-in template content (that's the quick-task template, separate work)
- Template versioning or dependency resolution
- Changes to the engine or compiler packages (those are done)

## Decision Drivers

- **Simple case stays simple**: `koto init --template quick-task` should compile, validate, and start a workflow without extra steps. Users shouldn't think about compilation.
- **Debuggability**: When something goes wrong, users need to inspect the compiled output and see what the compiler produced. An explicit compile command serves this need.
- **Discoverability**: Templates should be findable by name. Projects should be able to ship templates in a known location.
- **No new dependencies in the engine**: The engine reads compiled JSON with stdlib only. All new dependencies (if any) stay in the CLI or compiler packages.
- **Backward compatibility**: Existing `koto init` invocations that pass a file path should keep working during the transition.
- **Evidence accessibility**: Gate-based workflows need evidence from the CLI. This is the minimum viable interface.

## Considered Options

### Decision 1: Compilation Flow

When a user runs `koto init --template quick-task`, how does the template get compiled? This affects the default user experience. Most users won't think about compilation, so the default path matters.

#### Chosen: Implicit compilation at init, explicit command for debugging

`koto init` compiles the template automatically. The user passes a template source path (or name, resolved via search path), koto compiles it to JSON in memory, validates it, and starts the workflow. No intermediate file is written. The compiled JSON is ephemeral.

For debugging and CI, `koto template compile <path>` writes the compiled JSON to stdout (or a file with `--output`). `koto template inspect <path>` shows a human-readable summary. These are opt-in tools that don't affect the normal workflow.

The existing `koto validate` command (which checks template hash against state file) continues to work. It recompiles the source template at validation time and compares the hash.

#### Alternatives Considered

**Explicit compilation required**: Users run `koto template compile` first, then `koto init --compiled output.json`. This is how many build systems work, but it adds a step every user must learn. koto workflows are often started interactively, and forcing explicit compilation adds friction that most users don't need.
Rejected because it makes the simple case harder without clear benefit.

**Implicit with cached `.compiled.json` files**: `koto init` compiles and writes a cache file next to the source. Subsequent uses skip compilation if the source hasn't changed. Adds filesystem side effects (cached files in the user's project directory), introduces cache invalidation complexity, and templates are small enough that compilation is instant.
Rejected because the complexity isn't justified by the performance characteristics.

### Decision 2: Template Search Path

When a user says `--template quick-task` (a name, not a path), how does koto find the template? This determines how projects ship templates and how users organize their personal templates.

#### Chosen: Three-level search with explicit path override

koto resolves template names through a fixed search order:

1. **Explicit path**: If `--template` contains a `/` or ends in `.md`, treat it as a file path (absolute or relative to CWD). No search.
2. **Project-local**: Look for `./templates/<name>.md` and `./.koto/templates/<name>.md` relative to the git root (or CWD if not in a git repo).
3. **User-global**: Look for `~/.koto/templates/<name>.md`.

First match wins. If no match is found, return an error listing the paths searched.

The `.md` extension is added automatically when searching by name. `--template quick-task` looks for `quick-task.md`.

#### Alternatives Considered

**Configurable search path (environment variable)**: A `KOTO_TEMPLATE_PATH` variable listing directories to search, like `PATH` or `GOPATH`. Maximally flexible, but premature. koto doesn't have enough template variety to justify custom search paths. Users who need unusual locations can pass explicit paths.
Rejected because the fixed three-level hierarchy covers all foreseeable cases without configuration burden.

**Explicit paths only**: No search path, always require a file path. Simpler implementation, but `koto init --template ./templates/quick-task.md` is verbose for the common case. Projects can't establish conventions for template location.
Rejected because discoverability matters for adoption.

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

### Decision 4: Template Management Commands

How should template-related commands be organized? This affects command discoverability and help text.

#### Chosen: `koto template` subcommand group

Template operations live under `koto template <subcommand>`:

- `koto template compile <path>` -- compile source to JSON, write to stdout
- `koto template inspect <path>` -- human-readable summary of a template
- `koto template list` -- show available templates from search path

This groups related functionality and keeps the top-level namespace clean. The existing commands (`init`, `transition`, `next`, etc.) stay at the top level.

#### Alternatives Considered

**Flat commands**: `koto compile`, `koto inspect`, `koto list-templates`. Fewer keystrokes, but clutters the top-level namespace. As koto grows, flat commands don't scale.
Rejected because the top-level namespace should be reserved for workflow operations.

## Decision Outcome

### Summary

`koto init` gains implicit compilation: pass a template source file (or name) and it compiles, validates, and starts the workflow in one step. The compiled JSON is ephemeral -- held in memory, never written to disk. For debugging, `koto template compile` writes the compiled JSON to stdout and `koto template inspect` shows a human-readable summary.

Templates are found by name through a three-level search: explicit path, project-local (`./templates/` or `./.koto/templates/` from git root), then user-global (`~/.koto/templates/`). The `.md` extension is added automatically. First match wins.

`koto transition` gets a `--evidence key=value` flag (repeatable) that passes evidence to the engine's `WithEvidence()` option. This unblocks all gate-based workflows from the CLI.

The legacy `Parse()` function is deprecated. `koto init` switches to using `compile.Compile()` followed by `template.ParseJSON()` to build the engine's `Machine`. The transition is internal -- the `--template` flag interface doesn't change.

`koto template list` shows available templates from the search path with their names, descriptions, and locations.

### Rationale

Implicit compilation keeps the simple case simple. A user running `koto init --template quick-task` doesn't need to know about compilation, JSON, or format versions. They get a working workflow. The explicit `compile` and `inspect` commands exist for when things go wrong or when CI needs to validate templates.

The three-level search path follows conventions that developers already know: local overrides global, and explicit paths override search. It's the same model as `PATH`, Go module resolution, and npm's `node_modules` lookup.

Deferring LLM validation avoids designing around unknowns. The compiler's error messages are the validation layer for now. When we understand which LLM checks are useful, that becomes its own design.

### Trade-offs Accepted

- **No persistent compilation cache**: Templates are compiled on every `koto init`. This is fine because templates are small (kilobytes) and compilation is sub-millisecond. If templates grow or compilation slows, caching can be added without changing the interface.
- **No configurable search path**: The fixed three-level hierarchy can't be customized via environment variable. Users with unusual directory layouts pass explicit paths. This avoids premature generality.
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
2. Read the source file
3. Compile via `compile.Compile(sourceBytes)`
4. Validate via `template.ParseJSON(compiledJSON)`
5. Build `engine.Machine` from `CompiledTemplate.BuildMachine()`
6. Compute hash via `compile.Hash(compiledJSON)`
7. Call `engine.Init()` with the machine and metadata

The `--template` argument resolution:
- Contains `/` or ends in `.md` → treat as file path
- Otherwise → search by name: `<name>.md` through the search path

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

### New Commands

#### `koto template compile`

```
koto template compile <path> [--output <file>]
```

Compiles a source template and writes the compiled JSON to stdout (or `--output` file). Prints warnings to stderr. Exits non-zero on compilation error.

Use cases:
- Debug: see what the compiler produces
- CI: validate templates as part of a build pipeline
- Inspect: pipe to `jq` for ad-hoc queries

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

#### `koto template list`

```
koto template list [--search-dir <dir>]
```

Lists available templates from the search path:

```
NAME          VERSION  DESCRIPTION                LOCATION
quick-task    1.0      A focused task workflow     ./templates/quick-task.md
code-review   1.0      Code review workflow        ~/.koto/templates/code-review.md
```

The `--search-dir` flag adds an additional search directory (useful for testing).

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
- Modify `cmdInit` to use compiler path
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

This phase must land as a single unit -- `cmdInit` and `loadTemplateFromState()` must migrate together to avoid hash mismatches.

- Add `CompiledTemplate.ToTemplate()` adapter in `pkg/template/`
- Modify `cmdInit` to use compiler path (`compile.Compile` + `template.ParseJSON` + `BuildMachine`)
- Modify `loadTemplateFromState()` to use compiler path with `ToTemplate()` adapter
- Implement dual-hash comparison (compiler hash with legacy fallback)
- Update `koto validate` to use compiler path

### Phase 1b: Evidence and Search Path

- Add `--evidence` flag to `cmdTransition`
- Create `pkg/resolve/` package for template resolution
- Add `resolveTemplatePath()` with search path logic and shadow warnings
- Wire search path into `cmdInit`

### Phase 2: Template Subcommands

- Add `template` subcommand dispatcher
- Add `koto template compile` command
- Add `koto template inspect` command
- Add `koto template list` command

### Phase 3: Cleanup

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
- Templates are discoverable by name instead of requiring full paths
- The compiler path replaces the legacy parser, using the validated format specification
- Debug tooling (`compile`, `inspect`) gives users visibility into what the compiler produces
- Implicit compilation means the simple case stays simple

### Negative

- Two template loading paths exist during the transition (legacy `Parse()` and new compiler)
- Search path conventions must be documented and learned
- No LLM validation means template authors rely on compiler error messages only

### Mitigations

- Legacy `Parse()` gets a deprecation notice pointing to the compiler path
- Search path follows familiar conventions (local overrides global)
- Compiler error messages are already specific and actionable (13 validation rules)
