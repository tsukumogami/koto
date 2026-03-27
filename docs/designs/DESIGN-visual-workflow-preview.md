---
status: Proposed
upstream: docs/prds/PRD-template-visual-tooling.md
problem: |
  Compiled koto workflows are JSON state graphs that are difficult to review
  visually. Four user groups need visual tooling: template authors debugging
  stuck workflows, PR reviewers assessing structural changes, documentation
  readers browsing templates on GitHub or project websites, and repo
  maintainers enforcing diagram freshness via CI.
decision: |
  A single koto template export subcommand with --format mermaid|html, --check
  for CI freshness verification, and --open for browser convenience. HTML via
  Cytoscape.js + dagre from CDN with include_str! embedding. A reusable GHA
  workflow for drift detection across repos.
rationale: |
  Unified export collapses the original export/preview split because the
  documentation reader use case makes HTML a build artifact, not just a local
  debugging tool. The --open flag isolates the browser side effect. --check
  follows cargo fmt --check precedent for CI freshness. The GHA workflow
  downloads a release binary so consumers don't need a Rust toolchain.
---

# DESIGN: Visual Workflow Preview

## Status

Proposed

## Context and Problem Statement

Workflow templates compile to a directed graph of states with transitions,
gates, evidence schemas, and variables. Reviewing this structure from raw JSON
is tedious and error-prone, especially as workflows grow past 10-15 states.

Issue #86 requested a visual representation. Exploration evaluated rendering
technologies (Cytoscape.js, D3/dagre, server-side SVG, Mermaid), delivery
strategies (inlined bundles, CDN-loaded, server-side layout), and information
density patterns for graphs ranging from 2-3 to 30+ states.

Three prototypes were built and compared: server-side SVG with vanilla JS
(15-45 KB, fully offline), Cytoscape.js with inlined libraries (~435 KB), and
Cytoscape.js with CDN-loaded dependencies (~15-30 KB). The CDN approach was
chosen for its automatic layout quality, small file size, and rich
interactivity.

The accepted PRD (`docs/prds/PRD-template-visual-tooling.md`) defines four
personas, 15 requirements (R1-R15), and 39 acceptance criteria. This design
addresses the technical architecture for all requirements.

## Decision Drivers

- **Dual purpose**: HTML output serves both local debugging (with `--open`)
  and deployed documentation on project websites (GitHub Pages)
- **CI enforcement**: diagrams must be verifiable for freshness via a
  built-in `--check` flag and a reusable GHA workflow
- **File size discipline**: HTML files under 30 KB (CDN-loaded, not inlined)
- **Automatic layout**: Cytoscape.js + dagre eliminates manual positioning
- **Deterministic output**: byte-identical across runs and platforms for
  reliable CI drift detection (LF line endings unconditionally)
- **Format extensibility**: `--format` flag extends to DOT or PlantUML later
  without CLI schema changes
- **Minimal CLI surface**: one `export` subcommand instead of separate
  `export` + `preview` commands

## Decisions Already Made

These choices were settled during exploration and should be treated as
constraints, not reopened:

- **Cytoscape.js + dagre via CDN** over server-side Rust layout
- **Mermaid eliminated for interactive HTML** (retained as separate text format)
- **Tippy.js/Popper dropped**: vanilla JS tooltips work fine
- **D3 + dagre-d3 eliminated**: dagre-d3 rendering layer abandoned 6+ years
- **ELK.js deferred**: 1.3 MB too heavy for default
- **Browser launching via opener crate**: graceful fallback prints the path
- **No local server for v1**: single-file HTML generation is sufficient
- **include_str! with placeholder replacement** for HTML generation
- **Minimal Mermaid mapping**: states, transitions, gate notes, [*] markers

## Considered Options

### Decision 1: CLI command structure (revised)

The original design proposed separate `export` and `preview` subcommands. The
PRD's documentation reader use case changed this: HTML for project websites is
a pure file-write operation, same as Mermaid. The side effect (browser launch)
is a convenience, not the defining characteristic. The PRD requires a unified
command (R1).

#### Chosen: Unified export with post-parse flag validation

A single `koto template export` subcommand with `--format mermaid|html`,
`--output`, `--open`, and `--check` flags. Flag compatibility is enforced
by a `validate_export_flags()` function after clap parsing.

```rust
#[derive(Clone, Debug, PartialEq, clap::ValueEnum)]
pub enum ExportFormat {
    Mermaid,
    Html,
}

#[derive(clap::Args)]
pub struct ExportArgs {
    /// Path to template source (.md) or compiled template (.json)
    pub input: String,

    /// Output format
    #[arg(long, default_value = "mermaid", value_enum)]
    pub format: ExportFormat,

    /// Write output to file path (required for html format)
    #[arg(long)]
    pub output: Option<String>,

    /// Open generated file in default browser (html format only)
    #[arg(long)]
    pub open: bool,

    /// Verify existing file matches what would be generated
    #[arg(long)]
    pub check: bool,
}

fn validate_export_flags(args: &ExportArgs) -> Result<(), String> {
    if args.format == ExportFormat::Html && args.output.is_none() {
        return Err("--format html requires --output <path>".into());
    }
    if args.open && args.format != ExportFormat::Html {
        return Err("--open is only valid with --format html".into());
    }
    if args.open && args.check {
        return Err("--open and --check are mutually exclusive".into());
    }
    if args.check && args.output.is_none() {
        return Err("--check requires --output <path>".into());
    }
    Ok(())
}
```

Post-parse validation was chosen over clap attributes because:
- Error messages are exact domain sentences, not clap-generated phrasing
- All four rules live in one unit-testable function
- Matches existing koto pattern (e.g., `resolve_variables` validates post-parse)
- clap can't cleanly express "required when another flag equals a specific value"

#### Alternatives considered

**clap attribute validation**: partially feasible but can't express
format-conditional constraints without custom validation anyway, resulting in
split validation logic.

**Format-specific subcommands** (`export mermaid` / `export html`): eliminates
cross-flag validation but deviates from the PRD's `--format` flag design,
creates three-level nesting, and duplicates `--check` logic across variants.

**Separate export + preview** (original design): rejected because the
documentation reader use case makes HTML a build artifact. The side effect
is the browser launch, not the format. `--open` isolates it.

### Decision 2: Mermaid representation

*(Unchanged from exploration.)*

Each `TemplateState` becomes a Mermaid state. `[*]` marks initial and terminal
states. Transitions use `when` conditions as arrow labels. Gates appear as
`note left of` annotations containing the gate name. Evidence schemas, default
actions, integrations, and variables are omitted.

Example for a 5-state template:

```
stateDiagram-v2
    direction LR
    [*] --> explore
    explore --> evaluate
    evaluate --> implement : route: build
    evaluate --> research : route: investigate
    implement --> done
    research --> evaluate
    done --> [*]
    note left of explore : gate: check-repo
```

Alternatives considered: rich mapping with notes (too noisy), choice nodes
(triple visual elements), composite states (rendering bugs on GitHub).

### Decision 3: HTML generation architecture

*(Unchanged from exploration.)*

Embed a complete HTML file via `include_str!("preview.html")`. A single
placeholder replacement injects the compiled template's JSON data:

```javascript
const GRAPH_DATA = /*KOTO_GRAPH_DATA*/{};
```

CDN versions pinned: cytoscape@3.30.4, dagre@0.8.5, cytoscape-dagre@2.5.0.
Updates happen via PRs that bump the template file and SRI hashes.

Alternatives considered: split template (loses browser-preview during dev),
Rust template engine (one injection point doesn't justify a dependency),
programmatic HTML construction (painful CSS/JS iteration).

### Decision 4: Visual design

*(Unchanged from exploration.)*

Dark mode via `prefers-color-scheme`, `[*]` start marker, color-coded state
types (blue initial, green terminal, orange dashed gated, yellow branching),
system fonts, no badges. Click-to-highlight traces direct incoming/outgoing
edges (one hop).

### Decision 5: --check freshness implementation

The PRD requires a `--check` flag that compares generated output against an
existing file without writing (R8). The question is how to implement the
comparison.

#### Chosen: In-memory byte comparison

Generate output to a `Vec<u8>`, read existing file to a `Vec<u8>`, compare
with `==`. Three result states: Fresh, Stale, Missing.

```rust
enum CheckResult {
    Fresh,   // File exists and matches
    Stale,   // File exists but differs
    Missing, // File does not exist
}

fn check_freshness(generated: &[u8], path: &Path) -> io::Result<CheckResult> {
    match std::fs::read(path) {
        Ok(existing) => {
            if generated == existing.as_slice() {
                Ok(CheckResult::Fresh)
            } else {
                Ok(CheckResult::Stale)
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(CheckResult::Missing),
        Err(e) => Err(e),
    }
}
```

Error output goes to plain stderr (not JSON), since `export` is
developer-facing. Exit code 1 for both stale and missing. The error message
includes the fix command:

```
error: docs/workflow.mermaid.md is out of date
run: koto template export workflow.md --format mermaid --output docs/workflow.mermaid.md
```

#### Alternatives considered

**Tempfile + diff**: requires tempfile crate, subprocess, platform-dependent
diff availability. No benefit for files under 100 KB.

**Streaming hash comparison**: adds sha2 dependency for comparison, can't show
any diff detail. Strictly less capable than byte comparison.

### Decision 6: GHA reusable workflow architecture

The PRD requires a reusable GHA workflow (R9, R10) that consumers add with
minimal configuration.

#### Chosen: Reusable workflow with gh release download

A `on: workflow_call` workflow at
`.github/workflows/check-template-freshness.yml` in the koto repo. Downloads
a koto release binary via `gh release download` (pre-installed on runners, no
secrets needed for public repos).

**Inputs:**

| Input | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `template-paths` | string | yes | -- | Glob pattern for template `.md` files |
| `koto-version` | string | no | `latest` | Release tag or `latest` |
| `check-html` | boolean | no | `false` | Also verify HTML freshness |
| `html-output-dir` | string | no | `docs` | Directory for HTML outputs |

The workflow expands the glob with `compgen -G`, loops through templates,
runs `koto template export --check` for each, and uses `::error` annotations
with actionable fix commands. Fails if any template is stale or missing.

Callers pin `@v1` (tag or branch). Breaking input changes bump the major
version. Non-breaking additions (new optional inputs) update `v1`.

#### Alternatives considered

**Composite action**: can't define its own `runs-on`, requires caller to
handle checkout and runner, more boilerplate. The PRD says "callable via
`uses:` with a tag reference" which maps directly to workflow_call.

## Decision Outcome

**Chosen: unified export, in-memory --check, reusable GHA workflow**

### Summary

The feature ships as a single `koto template export` subcommand. `--format
mermaid` (default) produces `stateDiagram-v2` text to stdout or file.
`--format html` produces an interactive Cytoscape.js file. `--open` launches
the browser for local debugging. `--check` verifies freshness without writing.

The HTML is generated from `preview.html` embedded via `include_str!`. A
single placeholder replacement injects the compiled template's JSON data.
CDN versions are pinned with SRI integrity hashes. Output uses LF line
endings unconditionally for cross-platform determinism.

A reusable GHA workflow at `.github/workflows/check-template-freshness.yml`
downloads a koto release binary and runs `--check` for each template matching
a configurable glob. Consumers add 5 lines of YAML to get CI enforcement.

### Rationale

The unified export command reflects the PRD's insight that HTML is a build
artifact (documentation for project websites), not just a local debugging
tool. Separating `--open` as an opt-in flag isolates the browser side effect
while keeping the default behavior pure (write file, print path). `--check`
follows the `cargo fmt --check` pattern that CI users already know.

In-memory byte comparison is the simplest correct approach for files under
100 KB. The GHA workflow uses `gh release download` because it's pre-installed,
handles authentication, and works with the release asset naming convention
already established in `release.yml`.

## Solution Architecture

### Overview

One new subcommand under `koto template` produces visual representations in
two formats. The `export` subcommand handles format dispatch, flag validation,
freshness checking, and optional browser opening.

### Components

```
src/cli/mod.rs
  TemplateSubcommand::Export(ExportArgs)
  ExportFormat enum (Mermaid, Html)
  validate_export_flags()

src/export/
  mod.rs          -- re-exports, format dispatch
  mermaid.rs      -- CompiledTemplate -> stateDiagram-v2 text
  html.rs         -- CompiledTemplate -> HTML (include_str! + replace)
  check.rs        -- freshness comparison (CheckResult, check_freshness)

src/export/preview.html
  -- Self-contained HTML template with Cytoscape.js CDN refs + SRI hashes
  -- Contains /*KOTO_GRAPH_DATA*/ placeholder
  -- Valid HTML that renders with empty data during development

.github/workflows/check-template-freshness.yml
  -- Reusable workflow for CI freshness enforcement
```

### Key interfaces

**Source resolution helper:**
```rust
/// Resolve a source argument to a CompiledTemplate.
/// Accepts either a .md source (compiled via compile_cached) or
/// a .json pre-compiled template.
fn resolve_template(source: &str) -> anyhow::Result<CompiledTemplate>
```

**Mermaid export function:**
```rust
/// Generate stateDiagram-v2 Mermaid text from a compiled template.
/// Output uses LF line endings unconditionally.
pub fn to_mermaid(template: &CompiledTemplate) -> String
```

**HTML generation function:**
```rust
/// Generate interactive HTML from a compiled template.
/// Returns the generated HTML as bytes. The caller writes to disk.
/// Escapes </ as <\/ to prevent script context injection.
/// Output uses LF line endings unconditionally.
pub fn generate_html(template: &CompiledTemplate) -> Vec<u8>
```

Note: `generate_html` returns bytes rather than writing to a file, so
`--check` can compare without a temp file.

**Freshness check:**
```rust
/// Compare generated content against an existing file.
pub fn check_freshness(generated: &[u8], path: &Path) -> io::Result<CheckResult>
```

**Flag validation:**
```rust
/// Validate export flag combinations (R15).
/// Returns Ok(()) or an error describing the invalid combination.
fn validate_export_flags(args: &ExportArgs) -> Result<(), String>
```

**Error handling:** Export is developer-facing, not agent-consumed. Errors
go to stderr as plain text (not JSON). Exit code 2 for flag validation
errors, exit code 1 for stale/missing check results.

### Data flow

```
Template source (.md or .json)
  |
  v
resolve_template() -> CompiledTemplate
  |
  +---> to_mermaid() -> String (bytes)
  |                        |
  |                        +---> --check? -> check_freshness() -> exit 0/1
  |                        +---> --output? -> write to file
  |                        +---> else -> stdout
  |
  +---> generate_html() -> Vec<u8>
                             |
                             +---> --check? -> check_freshness() -> exit 0/1
                             +---> write to --output path
                             +---> --open? -> opener::open()
```

### CLI handler flow

```rust
// 1. Parse args (clap)
// 2. Validate flag combinations
validate_export_flags(&args)?;

// 3. Resolve template
let compiled = resolve_template(&args.input)?;

// 4. Generate output
let output_bytes = match args.format {
    ExportFormat::Mermaid => to_mermaid(&compiled).into_bytes(),
    ExportFormat::Html => generate_html(&compiled),
};

// 5. Check mode: compare and exit
if args.check {
    let path = Path::new(args.output.as_ref().unwrap());
    match check_freshness(&output_bytes, path)? {
        CheckResult::Fresh => std::process::exit(0),
        CheckResult::Stale | CheckResult::Missing => {
            // print error + fix command to stderr
            std::process::exit(1);
        }
    }
}

// 6. Write output
if let Some(ref output_path) = args.output {
    std::fs::write(output_path, &output_bytes)?;
    println!("{}", output_path);
} else {
    // mermaid to stdout (html always has --output)
    io::stdout().write_all(&output_bytes)?;
}

// 7. Open browser if requested
if args.open {
    if let Err(e) = opener::open(args.output.as_ref().unwrap()) {
        eprintln!("Could not open browser: {}", e);
    }
}
```

## Implementation Approach

### Phase 1: Export command with Mermaid format

Add `koto template export` with `--format mermaid` (default). Implement
`to_mermaid()`, `resolve_template()`, `validate_export_flags()`, and the CLI
handler. Output to stdout by default, `--output` for file.

Deliverables:
- `src/export/mod.rs`, `src/export/mermaid.rs`
- `ExportArgs`, `ExportFormat`, `validate_export_flags()` in `src/cli/mod.rs`
- Integration test: export fixture template as Mermaid, verify output
- Unit tests for `validate_export_flags()` covering all R15 combinations
- Determinism test: export same template twice, assert byte-identical output

### Phase 2: --check flag

Implement `check_freshness()` in `src/export/check.rs`. Wire into the CLI
handler. Verify with mermaid format first.

Deliverables:
- `src/export/check.rs` with `CheckResult` and `check_freshness()`
- Integration tests: fresh file (exit 0), stale file (exit 1), missing file
  (exit 1), error message includes fix command
- Verify the fix command resolves the drift when executed

### Phase 3: HTML format

Create `src/export/preview.html` with Cytoscape.js + dagre CDN setup, CSS
(including dark mode), JS for graph construction, tooltips, click-to-highlight,
and the `[*]` start marker. Implement `generate_html()` in `src/export/html.rs`.
Add `opener` to Cargo.toml for `--open`.

Deliverables:
- `src/export/preview.html` -- complete, browser-previewable HTML template
- `src/export/html.rs` -- HTML generation with placeholder replacement
- `opener` dependency in Cargo.toml
- Integration tests: HTML contains template data, CDN script tags, SRI hashes
- Determinism test: export same template as HTML twice, assert byte-identical
- Manual verification: open in browser with sample data, test dark mode

### Phase 4: GHA reusable workflow

Create `.github/workflows/check-template-freshness.yml` with the reusable
workflow. Test by calling it from the koto repo's own CI against the plugin
templates.

Deliverables:
- `.github/workflows/check-template-freshness.yml`
- Caller example in documentation or README
- Test by adding a freshness check job to `validate-plugins.yml`

## Consequences

### Positive

- Single `export` command covers all visual output (mermaid, html, future formats)
- `--check` enables CI freshness enforcement without git-level glue
- Reusable GHA workflow gives consumers 5-line CI setup
- HTML files work as both local debugging tools and deployed documentation
- Mermaid renders natively on GitHub for source-browsing readers
- No heavy new dependencies (only `opener` crate added)

### Negative

- CDN dependency: HTML doesn't work offline
- Pinned CDN versions require manual updates via PRs
- Dark mode relies on system preference only (no manual toggle)
- Mermaid export omits evidence schemas and default actions
- `--open` is format-conditional (only valid with `--format html`), adding a
  flag interaction rule

### Mitigations

- Offline use: a future `--inline` flag could bundle JS. Not needed for v1.
- CDN staleness: only Cytoscape.js is actively maintained (dagre frozen since
  2016). Low maintenance burden.
- Mermaid completeness: HTML covers detailed inspection. Mermaid is intentionally
  a structural overview.
- Flag interactions: `validate_export_flags()` catches all invalid combinations
  with clear error messages.

## Security Considerations

### CDN script integrity

Generated HTML loads JavaScript from unpkg.com. All `<script>` tags include
Subresource Integrity (SRI) hashes computed against pinned versions. The
browser refuses to execute scripts whose content doesn't match the hash.

```html
<script src="https://unpkg.com/cytoscape@3.30.4/dist/cytoscape.min.js"
        integrity="sha384-<hash>" crossorigin="anonymous"></script>
```

SRI hashes are committed in `preview.html`. Version bumps require new hashes.

### Script context injection

The compiled template JSON is injected into a `<script>` block via string
replacement. `serde_json` doesn't escape `</script>` sequences. The
`generate_html()` function replaces `</` with `<\/` in the serialized JSON
before insertion. This is a required implementation detail.

### Embedded template data

HTML files contain the full compiled template in plaintext, including gate
commands, action commands, directive text, and variable declarations. Users
should treat HTML export files with the same sensitivity as source templates.

For sharing structure without sensitive details, use `--format mermaid` --
it includes only state names, transitions, and gate names.

### --check mode safety

`--check` never writes to disk. It reads the existing file and compares in
memory. There's no race condition between check and write because they're
separate invocations. The `check_freshness()` function returns `Err` for
I/O errors other than `NotFound`, preventing silent failures.

### File write scope

The `--output` flag controls the write path. No default file derivation is
used for HTML (it requires explicit `--output`). For mermaid, output defaults
to stdout. This prevents accidental file writes to unexpected locations.

### GHA workflow permissions

The reusable workflow needs only `contents: read` to check out the repo and
download a public release. It doesn't need write permissions, secrets, or
token elevation. The `github.token` default is sufficient.
