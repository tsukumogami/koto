# Phase 2 Research: Codebase Analyst

## Lead 1: Current compile contract and integration points

### Findings

#### What `compile` does step by step

The `compile()` function in `src/template/compile.rs` takes a `&Path` to a source `.md` file and returns `anyhow::Result<CompiledTemplate>`:

1. **Read** the source file as a string.
2. **Split front-matter**: Parse the `---`-delimited YAML block from the markdown body using `split_frontmatter()`.
3. **Deserialize YAML**: Parse the front-matter into `SourceFrontmatter` (name, version, description, initial_state, variables, states).
4. **Validate required fields**: name, version, initial_state must be non-empty; states must be non-empty.
5. **Extract directives**: Scan the markdown body for `## <state-name>` headings; content between headings becomes the state's directive text. Only headings matching declared state names in the front-matter are treated as state boundaries.
6. **Build compiled states**: For each source state, compile gates (only `command` type supported), transitions (structured with optional `when` conditions), accepts blocks (field schemas), and default actions.
7. **Validate transition targets**: Every transition target must reference a declared state.
8. **Validate initial_state**: Must be a declared state name.
9. **Build compiled variables**: Transform source variable declarations into `VariableDecl` structs.
10. **Construct `CompiledTemplate`**: Assemble the struct with `format_version: 1`.
11. **Run `template.validate()`**: Runs all schema validation rules (see below).
12. **Return** the `CompiledTemplate`.

The function is pure -- it reads one file and returns a struct. No side effects, no file writes.

#### What `compile_cached` returns and where it writes

`compile_cached()` in `src/cache.rs`:

1. Calls `compile()` to get the `CompiledTemplate`.
2. Serializes it to pretty-printed JSON via `serde_json::to_string_pretty()`.
3. Computes SHA256 of the JSON bytes.
4. Cache path: `$XDG_CACHE_HOME/koto/<sha256>.json` (defaults to `~/.cache/koto/`).
5. If the cache file already exists, returns early (cache hit).
6. On cache miss: creates a temp file in the cache dir, writes JSON, atomically renames via `persist()`.
7. Returns `(PathBuf, String)` -- the cache path and the SHA256 hash.

The cache is content-addressed: the filename IS the hash. Two identical templates always produce the same cache entry.

#### The CompiledTemplate struct

```rust
pub struct CompiledTemplate {
    pub format_version: u32,        // Always 1
    pub name: String,
    pub version: String,
    pub description: String,        // Optional, skip_serializing_if empty
    pub initial_state: String,
    pub variables: BTreeMap<String, VariableDecl>,  // Optional, skip if empty
    pub states: BTreeMap<String, TemplateState>,
}
```

Where `TemplateState` contains:
- `directive: String` -- the markdown body text for this state
- `transitions: Vec<Transition>` -- each has `target` and optional `when` conditions
- `terminal: bool`
- `gates: BTreeMap<String, Gate>` -- command gates with type, command, timeout
- `accepts: Option<BTreeMap<String, FieldSchema>>` -- evidence schema
- `integration: Option<String>` -- integration name
- `default_action: Option<ActionDecl>` -- auto-execute command with optional polling

For Mermaid generation, the critical data is:
- `initial_state` -- the entry point
- `states` keys -- node names
- `transitions[].target` and `transitions[].when` -- edges with optional labels
- `terminal` -- identifies end nodes
- `gates` -- can be shown as annotations
- `accepts` -- can label what evidence a state expects

#### How validate works

`CompiledTemplate::validate()` in `src/template/types.rs` checks:
1. `format_version == 1`
2. Required fields non-empty (name, version, initial_state)
3. States non-empty; initial_state references a declared state
4. Per-state checks:
   - Directive non-empty
   - Transition targets exist in states map
   - Gates are only `command` type with non-empty command
   - Accepts field types are valid (enum, string, number, boolean)
   - Enum fields have non-empty values lists
   - Evidence routing validation (when conditions reference accepts fields, enum values in allowlist, mutual exclusivity)
   - Variable references (`{{KEY}}`) in directives, gate commands, action commands, and working_dir must be declared
   - Cannot have both `integration` and `default_action`
   - Action commands non-empty; polling timeout > 0

Returns `Result<(), String>` -- a single error string on failure.

#### Where Mermaid generation could hook in

Three viable integration points:

1. **After `compile()`, before cache write (inside `compile_cached`)**: The `CompiledTemplate` struct is fully built and validated. A Mermaid string could be generated here and written alongside the JSON. However, this would change the cache contract.

2. **As a separate CLI subcommand (`koto template diagram`)**: Takes a source path, calls `compile()`, generates Mermaid from the `CompiledTemplate`, prints to stdout. This is the cleanest separation of concerns and matches the existing pattern where `compile` prints the cache path and `validate` prints nothing on success.

3. **As a `--diagram` flag on `compile`**: Could emit the Mermaid alongside the cache path. Awkward because `compile` currently prints only the cache path to stdout (machine-parseable output).

Option 2 (separate subcommand) is the strongest fit because:
- It follows the existing CLI pattern of one-responsibility subcommands
- The compile output contract (prints cache path to stdout) stays untouched
- It can accept either source `.md` or compiled `.json` as input
- The Mermaid generation function can be a pure function on `CompiledTemplate`

#### Error handling pattern

All CLI commands use JSON-formatted errors printed to stdout, then `std::process::exit()` with specific codes:
- Exit 0: success
- Exit 1: transient/retryable errors
- Exit 2: caller errors (invalid input)
- Exit 3: infrastructure errors (corrupted state)

Pattern: `exit_with_error(serde_json::json!({"error": msg, "command": "..."}))`.

The `compile` subcommand currently prints the cache path on success (line to stdout), or a JSON error on failure.

### Implications for Requirements

1. **Output format for `template diagram`**: Should print Mermaid text to stdout (consistent with compile printing the path to stdout). Errors should use the JSON error pattern.
2. **Input flexibility**: The diagram subcommand should accept either a source `.md` or a compiled `.json` file. The `CompiledTemplate` struct is the same either way; accepting `.json` means users can diagram cached templates without re-compiling.
3. **Cache interaction**: Mermaid output should NOT be cached alongside compiled JSON. The cache is content-addressed by compiled JSON hash -- adding Mermaid would break this or require a separate cache namespace.
4. **No side effects**: Following the compile pattern, the diagram command should be a pure read-and-print operation.
5. **The `CompiledTemplate` struct has everything needed for Mermaid generation**: state names, transitions with labels, terminal markers, initial state. No additional compilation metadata is required.
6. **Validation is already embedded in compile**: Any template that compiles successfully is guaranteed to have valid transition targets, so the Mermaid generator doesn't need its own graph validation.

### Open Questions

1. Should `template diagram` also accept `--format` for future output formats (e.g., DOT, PlantUML)?
2. Should the diagram command write to a file (e.g., `--output <path>`) or always print to stdout?
3. For the HTML preview, should it be a separate command (`template preview`) or a format option on `diagram`?
4. Should gates and accepts blocks be shown in the diagram? They add useful information but increase visual complexity.

## Lead 2: Template format and source structure

### Findings

#### Source template .md file structure

Templates are Markdown files with YAML front-matter delimited by `---`. The format is:

```
---
name: <kebab-case-name>
version: "<semver>"
description: <optional description>
initial_state: <state-name>

variables:
  VAR_NAME:
    description: <text>
    required: true/false
    default: <value>

states:
  state_name:
    transitions:
      - target: <other-state>
        when:
          field: value
    terminal: true/false
    gates:
      gate_name:
        type: command
        command: "<shell command>"
    accepts:
      field_name:
        type: enum/string/number/boolean
        values: [a, b, c]
        required: true/false
    integration: <name>
    default_action:
      command: "<shell command>"
---

## state_name

Directive text for this state. Can include {{VAR_NAME}} references.

## other_state

Another directive.
```

#### Existing fenced code blocks

The template body is free-form markdown. Directives can contain any markdown content including fenced code blocks. The compiler treats all text between `## <state-name>` headings as the directive string -- it does NOT parse markdown structure within the directive. Adding a mermaid fenced code block to the source template would be treated as part of the nearest state's directive text. This means:

- Embedding a `mermaid` code block in the template source would NOT conflict with compilation.
- However, it would be misleading because the Mermaid block would become part of a state's directive, not a standalone diagram.
- The diagram should be generated as a separate artifact, not embedded in the source template.

#### Where compiled .json files get written

Compiled JSON goes to the content-addressed cache: `~/.cache/koto/<sha256>.json`. There is no convention for co-located artifacts (no `.mermaid` file next to the `.json`). The CLI `template compile` command prints only the cache path to stdout.

#### Convention for co-located artifacts

There is no existing convention. Templates live in plugin directories (e.g., `plugins/koto-skills/skills/hello-koto/hello-koto.md`), and compiled output goes to a global cache. There is no mechanism to write artifacts back to the source directory.

#### Template examples found

1. **`plugins/koto-skills/skills/hello-koto/hello-koto.md`**: Simple 2-state template (awakening -> eternal) with a command gate and variable substitution.

2. **`test/functional/fixtures/templates/multi-state.md`**: 4-state template (entry -> setup/work -> done) with evidence routing (`accepts` + `when`), command gates, and enum fields.

3. **`test/functional/fixtures/templates/simple-gates.md`**, **`decisions.md`**, **`var-substitution.md`**: Additional test fixtures covering various template features.

Templates range from trivial (2 states) to moderately complex (4+ states with branching). The multi-state example demonstrates the kind of graph that would most benefit from visual representation.

### Implications for Requirements

1. **Mermaid is a separate artifact**: It should not be embedded in source templates. The source `.md` format treats all body content as directive text within state sections.
2. **No existing co-location convention**: A new convention is needed if diagrams should live alongside source templates. Options:
   - Print to stdout (let the caller redirect to a file)
   - Accept `--output <path>` flag
   - Write to a conventional path (e.g., `<source>.mermaid.md`) -- this would be a new pattern
3. **Graph complexity is moderate**: The largest templates have ~4-6 states with branching. Mermaid stateDiagram-v2 handles this scale well.
4. **Test fixtures provide ready-made test cases**: The functional test templates can serve as integration test inputs for Mermaid generation.

### Open Questions

1. For CI enforcement (diagrams stay in sync), where should the committed diagram file live relative to the source template?
2. Should the diagram include directive text as state descriptions, or just state names and transitions?
3. Should evidence-routing conditions (`when` blocks) be rendered as edge labels?

## Lead 3: GHA workflows

### Findings

The repo has four workflow files:

#### `validate.yml` (main CI)
- Triggers on push to `main` and PRs targeting `main`.
- Jobs: `check-artifacts` (wip/ dir empty), `unit-tests` (`cargo test`), `fmt` (`cargo fmt --check`), `clippy` (`cargo clippy -- -D warnings`), `audit` (`cargo audit`), `coverage` (`cargo-llvm-cov`), `tsuku-distributed-install` (tests the tsuku recipe for installing koto).
- Uses a rollup `validate` job that checks all upstream results.
- Uses `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2`.

#### `release.yml`
- Triggers on `v*` tags.
- Cross-compiles for linux-amd64, linux-arm64, darwin-amd64, darwin-arm64.
- Creates GitHub release with checksums.

#### `validate-plugins.yml`
- Triggers on PRs modifying `plugins/**` or `.claude-plugin/**`.
- Jobs: `template-compilation` (builds koto, compiles all template .md files), `hook-smoke-test`, `schema-validation`.
- **This is the closest existing pattern to what a diagram sync check would look like**: it builds koto, then uses it to validate committed artifacts.

#### `eval-plugins.yml`
- Not read in detail but exists for plugin evaluation.

#### Patterns observed

- All workflows use `actions/checkout@v4`, `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2`.
- Build-then-use pattern: `validate-plugins.yml` builds koto from source, then uses the built binary to validate templates. A diagram sync workflow would follow the same pattern.
- Rollup jobs: each workflow has a final job that aggregates results of upstream jobs.
- `check-artifacts` job enforces that `wip/` is clean before merge.
- Path-filtered triggers: `validate-plugins.yml` only runs when plugin files change.

### Implications for Requirements

1. **A reusable workflow for diagram sync should follow the existing pattern**: build koto, run `koto template diagram` on all templates, compare output against committed diagrams.
2. **Path filtering**: The workflow should trigger on changes to template files and/or committed diagram files.
3. **The build-then-use pattern means the diagram command must be in the koto binary**: It can't be an external tool. This reinforces that the Mermaid generation should be a built-in subcommand.
4. **Rollup pattern**: Any new validation job should be added to the rollup aggregator or create its own rollup.
5. **Reusable workflow consideration**: Making it a reusable workflow (callable from other repos) would require it to be in a `.github/workflows/` file with `workflow_call` trigger. The koto repo could host this for downstream consumers.

### Open Questions

1. Should the diagram sync check be added to `validate-plugins.yml` (since it already validates templates) or be a separate workflow?
2. Should the reusable workflow accept inputs for template glob patterns and diagram output locations?
3. For repos consuming koto, should they install koto via tsuku recipe or download a release binary?

## Summary

The `CompiledTemplate` struct contains all information needed for Mermaid diagram generation: state names, transitions with optional `when` labels, terminal markers, gates, accepts blocks, and the initial state. The cleanest integration point is a new `koto template diagram` subcommand that accepts either source `.md` or compiled `.json` and prints Mermaid to stdout, following the existing one-responsibility-per-subcommand pattern. The `validate-plugins.yml` workflow provides a direct template for building a CI-based diagram sync check (build koto, run it against committed artifacts).
