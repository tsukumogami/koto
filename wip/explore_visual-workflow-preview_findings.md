# Exploration Findings: visual-workflow-preview

## Core Question

How should koto generate visual, interactive representations of compiled
workflows that serve both as debugging aids during template authoring and as
committable documentation viewable on GitHub Pages?

## Round 1

### Key Insights

- **Cytoscape.js + dagre is the best client-side option** (lead: js-graph-libraries). ~435 KB inlined, but only ~15-30 KB per file when loaded from CDN. Built-in pan/zoom, HTML tooltips, clean hierarchical layout for directed state graphs. Actively maintained with academic-grade credibility.
- **CDN loading eliminates the file size argument** (lead: single-file-html). Both use cases (local dev preview, GH Pages docs) are served over HTTPS. Per-file size drops to just graph data + styles + script tags. Git diffs are meaningful since only your code changes between versions.
- **"Write file, open with opener crate" is the Rust CLI standard** (lead: cli-browser-patterns). cargo doc and mdBook use it. Graceful fallback: print the path when browser can't open (SSH, CI, headless).
- **Mermaid export is a high-value MVP** (lead: dot-mermaid-fallback). ~50-100 lines of Rust, no dependencies, renders natively on GitHub. Ships weeks before the HTML preview and validates graph representation decisions.
- **Progressive disclosure is solved** (lead: info-density). Three-level semantic zoom, sidebar inspector, minimap. But for v1, hover tooltips + click-to-highlight neighborhood covers the core need.
- **Server-side layout in Rust produces tiny files (15-45 KB) but requires ~300-400 lines of layout code** (lead: single-file-html). Not worth the investment when dagre handles layout automatically and CDN eliminates the per-file overhead.

### Tensions

- **CDN dependency vs offline capability**: CDN-loaded HTML doesn't work offline or over file://. This is acceptable for the two target use cases but means the visualization is not fully self-contained. Could offer an `--inline` flag later for offline use.
- **Interactivity depth for v1**: Full feature set (semantic zoom, sidebar, minimap) vs minimal (pan/zoom, tooltips, click-to-highlight). Prototyping showed the minimal set is already useful. Richer features can layer on incrementally.

### Gaps

- No prototype tested at 30+ states — dagre layout quality at scale is assumed, not validated
- Edge label rendering for dense conditional transitions untested
- Dark mode and badge annotations not yet added to the chosen approach
- Mermaid export representation decisions (how to show gates, conditions) not explored in detail

### Decisions

- Cytoscape.js + dagre via CDN chosen over server-side Rust layout
- Mermaid eliminated for interactive HTML; retained as separate lightweight export
- Tippy.js/Popper dropped in favor of vanilla JS tooltips
- D3 + dagre-d3 eliminated (abandoned), ELK.js deferred (too heavy for default)

### User Focus

User prototyped all three approaches (server-side SVG, Cytoscape inlined, Cytoscape CDN) and chose Cytoscape CDN based on visual quality and automatic layout. File size concern resolved by CDN loading. Wants dual-purpose output: debugging preview + committable GH Pages docs.

## Accumulated Understanding

The visualization feature has two deliverables: a Mermaid export MVP (~50-100 lines of Rust, ships fast, renders on GitHub natively) and an interactive HTML preview using Cytoscape.js + dagre loaded from CDN. The HTML preview file contains only graph data, styles, and CDN script tags — keeping it small and diff-friendly. koto embeds an HTML template via `include_str!`, injects compiled workflow JSON at generation time, writes the file, and opens it with the `opener` crate.

For v1 interactivity: hover tooltips (gate conditions, evidence schemas, transitions), click-to-highlight neighborhood, pan/zoom, and visual encoding by state type (initial, terminal, gated, branching). Richer features (semantic zoom, sidebar inspector, minimap) can layer on later as the graph sizes grow.

Open areas: Mermaid export representation details, dark mode support, and validation that dagre handles 30+ state workflows acceptably.
