---
status: Proposed
problem: |
  Compiled koto workflows are JSON state graphs that are difficult to review
  visually. Developers need an interactive preview to spot structural issues
  (unreachable states, missing transitions, dead ends) and a committable
  artifact for template documentation on GitHub Pages.
decision: |
  Two new koto template subcommands: export (Mermaid text to stdout) and preview
  (interactive HTML via Cytoscape.js + dagre from CDN, opened in browser). HTML
  generated from an embedded template with placeholder replacement.
rationale: |
  Cytoscape.js + dagre via CDN provides automatic layout and rich interactivity
  at ~15-30 KB per file (vs ~435 KB inlined). Mermaid export ships first as a
  low-cost MVP that renders natively on GitHub. Separate subcommands keep
  composable text export distinct from side-effect-heavy browser preview.
---

# DESIGN: Visual Workflow Preview

## Status

Proposed

## Context and Problem Statement

Workflow templates compile to a directed graph of states with transitions,
gates, evidence schemas, and variables. Reviewing this structure from raw JSON
is tedious and error-prone, especially as workflows grow past 10-15 states.

Issue #86 requested a visual representation. Exploration via /explore evaluated
rendering technologies (Cytoscape.js, D3/dagre, server-side SVG, Mermaid),
delivery strategies (inlined bundles, CDN-loaded, server-side layout), and
information density patterns for graphs ranging from 2-3 to 30+ states.

Three prototypes were built and compared side-by-side: server-side SVG with
vanilla JS (15-45 KB, fully offline), Cytoscape.js with inlined libraries
(~435 KB), and Cytoscape.js with CDN-loaded dependencies (~15-30 KB). The
CDN approach was chosen for its automatic layout quality, small committed
file size, and rich interactivity.

## Decision Drivers

- **Dual purpose**: output must work as both a local debugging tool and as
  committable documentation viewable on GitHub Pages
- **File size discipline**: templates may be committed across many repos;
  per-file size should stay small
- **Automatic layout**: manual positioning or implementing layout algorithms
  in Rust is not worth the maintenance burden for bounded graph sizes
- **Interactivity**: hover tooltips, click-to-highlight, and pan/zoom are
  needed for inspecting gate conditions, evidence schemas, and transitions
- **Incremental delivery**: a simpler Mermaid export can ship before the
  full interactive preview

## Decisions Already Made

These choices were settled during exploration and should be treated as
constraints, not reopened:

- **Cytoscape.js + dagre via CDN** over server-side Rust layout: automatic
  layout eliminates ~300-400 lines of Rust layout code. CDN loading keeps
  committed files at ~15-30 KB. Both target use cases (local dev, GH Pages)
  are online.
- **Mermaid eliminated for interactive HTML**: 2 MB bundle, buggy tooltips,
  text DSL can't express koto's rich metadata. Retained as a separate
  lightweight export format.
- **Tippy.js/Popper dropped**: vanilla JS tooltips work fine, avoids two
  extra CDN dependencies with no quality loss.
- **D3 + dagre-d3 eliminated**: dagre-d3 rendering layer abandoned 6+ years.
  Cytoscape.js absorbed dagre as a maintained extension.
- **ELK.js deferred**: 1.3 MB too heavy for default. Can revisit if dagre
  layout quality degrades at 30+ states.
- **Browser launching via opener crate**: "write file, open with opener"
  pattern matches cargo doc and mdBook. Graceful fallback: print the file
  path.
- **No local server for v1**: single-file HTML generation is sufficient.

## Considered Options

### Decision 1: CLI command structure

The feature adds two new capabilities to koto: text-based graph export (Mermaid)
and interactive HTML preview. The question is where these live in the existing
`koto template` subcommand group, which currently has `compile` and `validate`.

Key assumptions: `preview` accepts both source templates (compiled on the fly)
and pre-compiled JSON. Mermaid export defaults to stdout for composability.

#### Chosen: Separate export and preview subcommands

Two new verbs under `koto template`:

- **`koto template export <source> --format mermaid`** produces text output to
  stdout by default. An `--output` flag writes to a file. The `--format` flag
  starts with `mermaid` and extends naturally to `dot` or `svg` later.
- **`koto template preview <source>`** generates a self-contained HTML file and
  opens it in the browser. Default output is `<template-stem>.preview.html` in
  the current directory, with `--output` for override.

This follows koto's existing one-verb-per-operation pattern. Each subcommand has
a clear purpose: `export` is pure text output (composable with pipes), `preview`
has side effects (file write + browser launch). Users running `koto template --help`
see four self-explanatory verbs: compile, validate, export, preview.

#### Alternatives considered

**Flags on compile** (`koto template compile --format mermaid`, `--preview`):
rejected because it overloads `compile`'s contract (YAML in, JSON out) and makes
`--preview` a surprising side effect on a pure-output command.

**Single visualize subcommand** (`koto template visualize --format html|mermaid`):
rejected because it conflates text export (pure, composable) with browser preview
(side-effect-heavy). `--format html` misleadingly suggests HTML on stdout.

**Flags on existing + new subcommand** (`compile --mermaid` + `preview`):
rejected because it splits format selection across two commands inconsistently
and doesn't extend well to additional formats.

### Decision 2: Mermaid representation

Mermaid's `stateDiagram-v2` is the secondary, lightweight export. It renders
inline on GitHub (PRs, READMEs, issues) without tooling. The question is how
much template metadata to include.

Key assumptions: directive text truncated to ~60 chars. GitHub continues to
support `stateDiagram-v2` without `classDef`. Evidence schemas and default
actions belong in the interactive view, not the structural overview.

#### Chosen: Minimal mapping

Each `TemplateState` becomes a Mermaid state with its name. `[*]` marks the
initial and terminal states. Transitions use `when` conditions as arrow labels
(e.g., `route: setup`). Gates appear as `note left of` annotations listing
gate names and commands. Evidence schemas, default actions, integrations, and
variables are omitted.

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
    note left of explore : gate: check-repo (test -d .git)
```

#### Alternatives considered

**Rich mapping with notes** (gates as left-notes, evidence as right-notes):
rejected because double-sided notes create a wall of text on 15+ state
workflows. Mermaid's note rendering is inconsistent across renderers.

**Choice nodes for branching** (`<<choice>>` pseudo-states): rejected because
they triple visual elements (state + diamond + extra arrows) with no
information gain over condition labels on edges.

**Composite states for phases** (grouping by name prefix): rejected because
automatic grouping is fragile and composite states have rendering bugs on
GitHub (overlapping labels, misrouted transitions across boundaries).

### Decision 3: HTML generation architecture

The interactive preview is a single HTML file with Cytoscape.js + dagre loaded
from CDN. The only dynamic content is the compiled template's JSON data. The
question is how the Rust binary produces this file.

Key assumptions: the HTML template stabilizes quickly. serde_json handles all
escaping. If the template grows to need multiple injection points, migration
to askama is straightforward.

#### Chosen: include_str! with placeholder replacement

Embed a complete HTML file via `include_str!("preview.html")` at compile time.
The file contains a placeholder inside a script tag:

```javascript
const GRAPH_DATA = /*KOTO_GRAPH_DATA*/{};
```

At runtime, `template_str.replace("/*KOTO_GRAPH_DATA*/", &json_data)` injects
the compiled template. The HTML file is a valid, browser-openable file during
development (renders with empty data).

CDN versions are hardcoded and pinned: cytoscape@3.30.4, dagre@0.8.5,
cytoscape-dagre@2.5.0. Updates happen via PRs that bump the template file.

Default output path: `<template-stem>.preview.html` in the current directory,
overridable with `--output`.

#### Alternatives considered

**Split template (head/middle/tail)**: rejected because developers lose the
ability to open the template file directly in a browser during iteration.

**Rust template engine (askama/tera)**: rejected because one injection point
doesn't justify a new proc-macro or runtime dependency. Revisit if the template
needs conditionals or loops.

**Programmatic HTML construction**: rejected because CSS and JS buried in
`format!()` calls is painful to iterate on and eliminates browser-preview
during development.

### Decision 4: Visual design

The Cytoscape CDN prototype established the baseline visual design. The
question is what to add, change, or leave alone for production.

#### Chosen: Add dark mode and start marker; keep everything else

**Dark mode**: yes. Port the `prefers-color-scheme: dark` media query from the
server-side prototype. ~15 lines of CSS, automatic (no toggle needed).

**Start marker**: yes. Add a small filled circle `[*]` node with an edge to
the initial state. The initial state isn't always obvious from color alone in
large graphs.

**Badge annotations on nodes**: no. Gate and evidence counts are available via
hover tooltips. Badges increase visual noise and force wider node boxes, which
scales poorly at 30+ states.

**Terminal end markers**: no. Green fill plus the legend is sufficient. Adding
`[*]` end nodes creates an extra edge per terminal state with no information
gain.

**Edge labels**: keep prototype styling (auto-rotated with opaque background).
Dagre's `edgeSep` handles separation for typical branching. Revisit if real
templates surface overlap problems.

**Fonts**: keep the system font stack for labels, monospace for edge conditions
and tooltip code. No web font loading needed.

## Decision Outcome

**Chosen: export + preview subcommands, minimal Mermaid, include_str! HTML, dark mode + start marker**

### Summary

The feature ships as two `koto template` subcommands. `export --format mermaid`
produces a lightweight `stateDiagram-v2` diagram to stdout, showing states,
transitions with condition labels, and gates as notes. It renders natively on
GitHub and ships first as an MVP. `preview` generates an interactive HTML file
using Cytoscape.js + dagre loaded from CDN, with hover tooltips for gates and
evidence schemas, click-to-highlight for tracing paths, and pan/zoom for large
graphs.

The HTML is generated from a real `preview.html` file embedded in the binary via
`include_str!`. A single placeholder replacement injects the compiled template's
JSON data. CDN versions are pinned (cytoscape@3.30.4, dagre@0.8.5,
cytoscape-dagre@2.5.0) for reproducibility. The output file defaults to
`<template-stem>.preview.html` in the current directory and opens in the browser
via the `opener` crate, with a graceful fallback that prints the file path.

Visual design builds on the prototype: color-coded state types (blue initial,
green terminal, orange dashed gated, yellow branching), a `[*]` start marker
for entry point clarity, dark mode via `prefers-color-scheme`, and system fonts
throughout. Badges are omitted in favor of tooltip-based inspection.

### Rationale

The decisions reinforce each other. Separating `export` from `preview` keeps
each command focused — text-format export is composable (pipe, redirect),
while HTML preview is inherently side-effect-heavy (file write, browser launch).
Minimal Mermaid mapping pairs with the rich interactive HTML: the Mermaid
diagram shows structure at a glance, the HTML preview shows full detail on
demand. Neither tries to do the other's job.

The `include_str!` approach for HTML generation preserves the development
workflow (edit HTML, refresh browser) while keeping the build simple (no
template engine dependency). Pinned CDN versions keep generated files
reproducible without bundling ~435 KB of JavaScript into every output file.
Dark mode and the start marker are the only additions to the prototype -- the
rest was already close to production quality.

## Solution Architecture

### Overview

Two new subcommands under `koto template` produce visual representations of
compiled workflows. `export` generates Mermaid text from the `CompiledTemplate`
struct. `preview` generates an interactive HTML file from an embedded template
with injected graph data, then opens it in the browser.

### Components

```
src/cli/mod.rs
  TemplateSubcommand::Export { source, format, output }
  TemplateSubcommand::Preview { source, output }

src/export/
  mod.rs          -- re-exports mermaid and preview modules
  mermaid.rs      -- CompiledTemplate -> stateDiagram-v2 text
  preview.rs      -- CompiledTemplate -> HTML (include_str! + replace)

src/export/preview.html
  -- Self-contained HTML template with Cytoscape.js CDN refs
  -- Contains /*KOTO_GRAPH_DATA*/ placeholder
  -- Valid HTML that renders with empty data during development
```

Note: `src/export/mod.rs` starts as a simple re-export (`pub mod mermaid;
pub mod preview;`). Add a format dispatch enum when a second export format
materializes, not before.

### Key interfaces

**Source resolution helper:**
```rust
/// Resolve a source argument to a CompiledTemplate.
/// Accepts either a .md source (compiled via compile_cached) or
/// a .json pre-compiled template.
fn resolve_template(source: &str) -> anyhow::Result<CompiledTemplate>
```
`compile_cached()` returns `(PathBuf, String)`, not a struct. This helper
reads and deserializes the cached file, reusing the existing
`load_compiled_template()` logic. Shared by both `export` and `preview`
handlers to avoid duplicating detection logic.

**Mermaid export function:**
```rust
pub fn to_mermaid(template: &CompiledTemplate) -> String
```
Walks `template.states`, emits `stateDiagram-v2` syntax. Returns the complete
Mermaid text. No dependencies beyond the `CompiledTemplate` type.

**HTML preview function:**
```rust
pub fn generate_preview(
    template: &CompiledTemplate,
    output_path: &Path,
) -> anyhow::Result<()>
```
Serializes template to JSON, escapes `</` as `<\/` to prevent script context
injection, replaces the placeholder in the embedded HTML, writes the file.
The caller (CLI handler) is responsible for opening the browser.

**Error handling:** Both `export` and `preview` are developer-facing commands
(not agent-consumed), so errors go to stderr as plain text, not JSON. This
differs from `compile` and `validate` which use `exit_with_error()` JSON
output for machine consumption.

**CLI handler for preview:**
```rust
// In the template preview match arm:
let html_path = generate_preview(&compiled, &output_path)?;
println!("{}", html_path.display());
if let Err(e) = opener::open(&html_path) {
    eprintln!("Could not open browser: {}", e);
}
```

### Data flow

```
Template source (.md)
  |
  v
compile_cached() -> CompiledTemplate JSON (existing)
  |
  +---> to_mermaid() -> stdout or file
  |
  +---> generate_preview() -> .preview.html
          |
          v
        opener::open() -> browser
```

Both `export` and `preview` accept either a source template path (compiled on
the fly via `compile_cached`) or a pre-compiled JSON path. The shared
`resolve_template()` helper detects which based on file extension (.md/.yaml
triggers compilation, .json loads directly).

## Implementation Approach

### Phase 1: Mermaid export (MVP)

Add `koto template export <source> --format mermaid`. Implement `to_mermaid()`
in `src/export/mermaid.rs`. Output to stdout by default, `--output` for file.

Deliverables:
- `src/export/mod.rs` -- module with format enum
- `src/export/mermaid.rs` -- Mermaid generation
- `TemplateSubcommand::Export` variant in `src/cli/mod.rs`
- Integration test: compile a fixture template, export as Mermaid, validate
  the output contains expected states and transitions

### Phase 2: HTML preview template

Create `src/export/preview.html` with Cytoscape.js + dagre CDN setup, CSS
(including dark mode), JS for graph construction, tooltips, click-to-highlight,
and the `[*]` start marker. Test in browser with hardcoded data.

Deliverables:
- `src/export/preview.html` -- complete, browser-previewable HTML template
- Manual verification: open the template with sample data in a browser

### Phase 3: Preview command

Add `koto template preview <source>`. Implement `generate_preview()` in
`src/export/preview.rs`. Add `opener` to Cargo.toml. Wire up CLI handler.

Deliverables:
- `src/export/preview.rs` -- HTML generation with placeholder replacement
- `TemplateSubcommand::Preview` variant in `src/cli/mod.rs`
- `opener` dependency in Cargo.toml
- Integration test: generate preview HTML, verify it contains the template
  data and CDN script tags

## Consequences

### Positive

- Developers can visually inspect workflow structure without reading JSON
- Mermaid export renders natively in GitHub PRs, READMEs, and issues
- Interactive HTML enables debugging large workflows (30+ states) with
  tooltips and click-to-highlight
- Preview files are small (~15-30 KB) and committable as documentation
- No new heavy dependencies (only `opener` crate added)

### Negative

- CDN dependency: preview HTML doesn't work offline or without internet
- Pinned CDN versions require manual updates via PRs
- Dark mode relies on system preference only (no manual toggle)
- Mermaid export omits evidence schemas and default actions

### Mitigations

- Offline use: a future `--inline` flag could bundle JS for offline-capable
  output. Not needed for v1 since both use cases are online.
- CDN staleness: CI could periodically check for new Cytoscape.js releases
  and open update PRs. Low priority since pinned versions work indefinitely.
- Mermaid completeness: the interactive HTML preview covers the detailed
  metadata inspection use case. Mermaid is intentionally a structural overview.

## Security Considerations

### CDN script integrity

The generated HTML loads JavaScript from unpkg.com. Without verification, a
compromised CDN could serve malicious code to anyone who opens a preview file.
All three `<script>` tags must include Subresource Integrity (SRI) hashes:

```html
<script src="https://unpkg.com/cytoscape@3.30.4/dist/cytoscape.min.js"
        integrity="sha384-<hash>" crossorigin="anonymous"></script>
```

SRI hashes are computed at development time against the pinned versions and
embedded in `preview.html`. When CDN versions are bumped, new hashes must be
computed and committed alongside the version update. The browser refuses to
execute any script whose content doesn't match the hash.

### Script context injection

The compiled template JSON is injected into a `<script>` block via string
replacement. `serde_json` produces valid JSON but doesn't escape `</script>`
sequences, which would break out of the HTML script context if a state
directive or gate command contained that string. The `generate_preview()`
function must replace `</` with `<\/` in the serialized JSON before insertion.
This is a required implementation detail, not optional.

### Embedded template data

Preview HTML files contain the full compiled template in plaintext, including
gate commands (shell commands), default action commands, directive text with
agent instructions, and variable declarations. Users should treat preview
files with the same sensitivity as source templates.

For sharing workflow structure (in PRs, READMEs, or public documentation),
prefer `koto template export --format mermaid` -- it includes only state
names, transitions, and gate names, not full commands or directives.

### File write scope

The preview command writes to the current directory using `Path::file_stem()`
for filename derivation. This prevents path traversal via `../` in template
paths. The output path is printed so users know exactly what was written.
