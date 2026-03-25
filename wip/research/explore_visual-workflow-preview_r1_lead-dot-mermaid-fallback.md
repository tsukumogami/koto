# Lead: DOT/Mermaid export as simpler fallback

## Findings

### Mermaid: GitHub-native rendering

GitHub renders `mermaid` code blocks natively in markdown files (READMEs, issues, PRs, wiki pages excluded). This means a `koto template export --format mermaid` command could produce output that's immediately viewable on GitHub without any tooling. The `stateDiagram-v2` diagram type maps directly to koto's compiled template structure:

```
stateDiagram-v2
    direction LR
    [*] --> gather
    gather --> plan : auto
    plan --> implement : when decision=proceed
    plan --> done : when decision=skip
    implement --> review
    review --> done
    done --> [*]
```

Key `stateDiagram-v2` features relevant to koto:
- **Composite states**: states can nest other states (useful for grouping phases)
- **Choice pseudo-states**: `<<choice>>` nodes for conditional routing
- **Fork/join**: `<<fork>>` and `<<join>>` for parallel paths
- **Notes**: annotations attachable to states (left or right)
- **Direction control**: `direction LR` / `direction TB` for layout orientation
- **Styling**: `classDef` for custom node styles (e.g., terminal states)

### Mermaid: Limitations and scaling

**Character limit**: Mermaid defaults to 50,000 characters per diagram. GitHub uses this default. A 17-state template like shirabe's work-on (with directives, gates, evidence schemas, and conditional transitions) would produce roughly 2,000-4,000 characters of Mermaid syntax -- well within limits. Even a 50-state workflow with verbose annotations would likely stay under 15,000 characters.

**Layout quality at scale**: Mermaid uses Dagre as its default layout algorithm, with ELK (Eclipse Layout Kernel) available for more complex graphs. For 30+ states with conditional transitions, readability degrades because:
- Edge crossings increase and Dagre doesn't optimize aggressively for them
- Long labels on transitions (e.g., `when decision=proceed AND priority=high`) get truncated or overlap
- No built-in progressive disclosure -- all detail is visible at once
- No pan/zoom in GitHub's static rendering

**Annotation support**: Notes exist but are limited to simple text. You can't embed structured data (evidence schemas, gate commands) in a readable way. Workaround: abbreviate and link to full details elsewhere.

**No interactivity in GitHub rendering**: GitHub renders Mermaid to static SVG. No hover, no click-to-expand, no zoom. This is the fundamental limitation compared to the HTML approach.

### DOT/Graphviz: What it adds over Mermaid

DOT offers advantages in specific scenarios:

1. **Layout quality**: Graphviz's `dot` engine produces consistently better layouts for directed graphs, especially with many edges. At 30+ states, DOT output is typically more readable than Mermaid's Dagre-based rendering.

2. **Headless rendering**: `dot -Tsvg` or `dot -Tpng` runs without a browser, making it suitable for CI pipelines that generate documentation artifacts. Mermaid CLI (`mmdc`) requires a headless Chromium instance.

3. **Deterministic output**: Same input always produces same output. Useful for diffing graph changes across template versions.

4. **Richer attribute model**: DOT supports arbitrary key-value attributes on nodes and edges, making it natural to encode koto metadata (gate types, evidence schemas, transition conditions) as tooltip text or record-style node labels.

**However**: DOT does not render natively on GitHub. Users need Graphviz installed locally or a CI step to convert to SVG/PNG. This eliminates the "zero-tooling viewable" advantage that Mermaid has.

### DOT/Graphviz: Rust crate ecosystem

Several Rust crates for DOT generation:

- **`graphviz-rust`** (crates.io): Full-featured, supports building graphs programmatically with macros, includes a printer module for DOT string serialization. Most actively maintained option. Can also invoke `dot` to render SVG/PNG if Graphviz is installed.
- **`dot-rust`**: Simpler API for DOT generation, fewer features.
- **`layout`** (github.com/nadavrot/layout): Pure Rust Graphviz renderer -- can render DOT to SVG without external Graphviz installation. This is notable because it eliminates the system dependency on Graphviz.

For koto's needs, DOT generation is trivially implementable without any crate -- the format is simple enough that `format!()` calls suffice for state machine graphs. A dedicated crate would only matter if we needed the rendering step too.

### Terminal rendering

Tools that render graph formats in terminal:

- **`mermaid-ascii`** (github.com/AlexanderGrooff/mermaid-ascii): Renders Mermaid to ASCII art in the terminal. Quality is acceptable for small graphs (under 10 states) but breaks down for larger ones.
- **Graphviz in terminal**: The `kitty` terminal can display SVG/PNG inline. Sixel-capable terminals (xterm, mlterm, foot) can show rendered DOT output. But these require specific terminal emulators.
- **No universal terminal graph renderer exists**: For broad compatibility, the best "terminal" output is just printing the text-based format itself (Mermaid or DOT source), which users can paste into online editors.

### Mermaid as MVP before HTML

The incremental delivery argument is strong:

1. **Mermaid export is ~50-100 lines of Rust**: Walk `CompiledTemplate.states`, emit `stateDiagram-v2` syntax. No new dependencies. No JS bundling. No HTML templating.
2. **Immediately useful on GitHub**: Paste output into a README or issue, get a rendered diagram.
3. **Validates the graph model**: Building the Mermaid exporter forces decisions about how to represent conditional transitions, gates, and terminal states visually -- decisions that carry over to the HTML version.
4. **Ships in a single PR**: No frontend toolchain, no asset embedding, no browser-launch logic.

The HTML preview is a significantly larger effort (JS library selection, asset bundling into single-file HTML, browser-launch cross-platform logic, interactive features). Mermaid export could ship weeks earlier.

### What Mermaid cannot replace

Mermaid export does NOT satisfy the core requirements from issue #86:
- No interactivity (hover for gate conditions, click to expand evidence schemas)
- No progressive disclosure for large graphs
- No pan/zoom
- Not self-contained HTML viewable on GitHub Pages
- No way to show runtime state overlay (current position in workflow)

Mermaid is a complement to HTML preview, not a replacement.

## Implications

1. **Mermaid export should ship first as an MVP**: It's low-effort, immediately useful, and validates graph representation decisions. The command could be `koto template export --format mermaid <template.json>` (or similar).

2. **DOT export adds marginal value if HTML preview exists**: Once the interactive HTML preview ships, DOT becomes redundant for most users. The exception is CI-generated documentation where a headless SVG render is needed -- but even there, Mermaid CLI (`mmdc`) or the pure-Rust `layout` crate could handle it.

3. **DOT export is worth including only if it's near-zero cost**: Since DOT syntax is trivial to generate (no crate needed), adding `--format dot` alongside `--format mermaid` costs almost nothing. It becomes a free bonus rather than a strategic investment.

4. **Terminal rendering is not a priority**: No tool produces good terminal output for graphs of koto's complexity. The practical terminal workflow is: export Mermaid text, paste into mermaid.live or a local editor. This is acceptable.

5. **Mermaid's 50,000-character limit is not a concern**: Even large koto templates (30+ states) will produce Mermaid output well under this threshold.

6. **No new dependencies needed**: Both Mermaid and DOT are plain text formats. Generation is string concatenation against the existing `CompiledTemplate` struct. Zero crate additions to Cargo.toml.

## Surprises

1. **Pure-Rust Graphviz rendering exists**: The `layout` crate can render DOT to SVG without system Graphviz. This could enable `koto template export --format svg` without requiring users to install anything. However, this adds a dependency and the SVG output lacks interactivity, so it's not clearly better than the HTML approach.

2. **GitHub Wiki does not render Mermaid**: Only markdown files in repos (READMEs, docs, issues, PRs) get Mermaid rendering. Wiki pages do not. This is a documented limitation.

3. **Mermaid's `stateDiagram-v2` has no edge label wrapping or truncation**: Long transition conditions will overlap neighboring elements. koto would need to abbreviate conditions (e.g., `decision=proceed` instead of the full `when` clause) to keep diagrams readable.

4. **`mermaid-ascii` exists but is immature**: Terminal ASCII rendering of Mermaid is possible but quality drops sharply above ~8-10 nodes.

## Open Questions

1. **How should conditional transitions be labeled in Mermaid?** Full `when` clauses can be verbose. Should we abbreviate to field=value pairs, or use numeric references with a legend?

2. **Should gates be shown as annotations (notes) or as pseudo-states?** Notes keep the graph simpler but are easy to miss. Pseudo-states (diamond shapes via `<<choice>>`) make gates visible but inflate the node count.

3. **Should terminal states get special visual treatment?** Mermaid supports `classDef` for styling, and `[*]` for start/end pseudo-states. Terminal states in koto could connect to `[*]` end markers.

4. **Where does the Mermaid output go?** Options: stdout (pipe-friendly), a `.md` file with a mermaid code block, or a `.mmd` raw file. Each serves different workflows.

## Summary

Mermaid export is a high-value, low-cost MVP that can ship weeks before the interactive HTML preview -- it requires roughly 50-100 lines of Rust, no new dependencies, and produces output that renders natively on GitHub. DOT/Graphviz adds marginal value over Mermaid (slightly better layout for large graphs, deterministic output for CI) but doesn't render on GitHub, making it a "free bonus if trivial to add" rather than a strategic feature. Neither format replaces the interactive HTML preview for the core use cases in issue #86, but Mermaid export validates graph representation decisions and delivers immediate utility while the HTML work proceeds.
