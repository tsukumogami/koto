# Lead: What JS graph rendering libraries produce self-contained, interactive HTML?

## Findings

### Cytoscape.js

**Self-contained HTML**: Yes. Can be loaded via a single `<script>` tag with no build system required. The UMD bundle is ~365 kB minified, ~112 kB gzipped. Extensions (dagre, elk, popper) add more but remain embeddable.

**Directed graph layout**: Yes, through extensions. `cytoscape-dagre` wraps the dagre library for traditional hierarchical DAG layout. `cytoscape-elk` wraps ELK.js for more advanced layered layout with port support. Both register automatically in plain HTML/JS environments.

**Rich node content**: Native labels are text-only, but `cytoscape-node-html-label` allows arbitrary HTML overlays on nodes with click event support (`enablePointerEvents: true`). Tooltips via `cytoscape-popper` + Tippy.js (the recommended modern approach; the older qtip extension is deprecated). This means tooltips can contain arbitrary HTML for gate conditions, evidence schemas, etc.

**Pan/zoom**: Built-in. Cytoscape.js has native pan, zoom, box selection, and fit-to-viewport.

**Bundle size for self-contained HTML**: Core (~365 kB min) + dagre extension (~30 kB) + node-html-label (~10 kB) + popper/tippy (~30 kB) = roughly 435 kB minified, ~140 kB gzipped. With ELK instead of dagre, add ~1.3 MB for elkjs WASM, bringing total to ~1.7 MB minified.

**Layout quality**: Dagre produces clean top-down or left-right hierarchical layouts well-suited for state machines. ELK's layered algorithm is higher quality for complex graphs with ports and edge routing but costs significantly more in bundle size.

**Maintenance**: Very active. Version 3.33.0 released July 2025 (latest 3.33.1). Over 10k GitHub stars. Strong academic backing (published in Bioinformatics journal).

**Sources**: [Cytoscape.js](https://js.cytoscape.org/), [cytoscape-dagre](https://github.com/cytoscape/cytoscape.js-dagre), [cytoscape-elk](https://github.com/cytoscape/cytoscape.js-elk), [cytoscape-node-html-label](https://github.com/kaluginserg/cytoscape-node-html-label), [cytoscape-popper](https://github.com/cytoscape/cytoscape.js-popper), [size snapshot](https://github.com/cytoscape/cytoscape.js/blob/unstable/.size-snapshot.json)

---

### vis-network (vis.js)

**Self-contained HTML**: Yes. Provides a standalone UMD build (`vis-network.min.js`) with CSS auto-injected. Single script tag, no build tools needed.

**Directed graph layout**: Yes. Built-in hierarchical layout with configurable direction (UD, DU, LR, RL), `sortMethod: "directed"` which places "from" nodes above "to" nodes. No external layout extension needed.

**Rich node content**: Supports custom shapes, images, icons, and multi-line labels. HTML inside nodes is not natively supported -- labels are canvas-rendered text. Tooltips are built-in (`title` property on nodes/edges renders as browser tooltip). For richer popups, you handle click events and create custom DOM overlays.

**Pan/zoom**: Built-in. Native pan, zoom, and fit.

**Bundle size**: The standalone minified build is estimated at ~500-600 kB minified. The npm unpacked package is 82.9 MB (includes source, docs, examples), but the actual distributable is much smaller. Gzipped likely ~150-180 kB.

**Layout quality**: The hierarchical layout is functional but not as refined as dagre or ELK for complex DAGs. Edge routing is simpler. Physics-based stabilization can cause jitter on initial load.

**Maintenance**: Moderately active. Latest version 10.0.2. Community-maintained after the original vis.js project split. Less active than Cytoscape.js.

**Sources**: [vis-network docs](https://visjs.github.io/vis-network/docs/network/layout.html), [vis-network GitHub](https://github.com/visjs/vis-network), [standalone example](https://visjs.github.io/vis-network/examples/network/basic_usage/standalone.html)

---

### D3.js + dagre / dagre-d3

**Self-contained HTML**: Yes. dagre-d3 provides a full bundle (`dagre-d3.js`) with all dependencies included, ready for `<script>` tag use. D3 itself is also available as a single UMD bundle.

**Directed graph layout**: Yes. Dagre is specifically designed for directed graph layout -- it implements the Sugiyama algorithm for layered/hierarchical graphs.

**Rich node content**: D3's SVG manipulation allows arbitrary SVG/HTML content inside nodes. You have full control over rendering. However, this means you write all the rendering code yourself.

**Pan/zoom**: D3 has `d3-zoom` for pan/zoom, but you wire it up manually.

**Bundle size**: D3 full bundle is ~280 kB minified. dagre-d3 adds another ~100 kB. Total ~380 kB minified.

**Layout quality**: Dagre produces good hierarchical layouts. However, it has known issues with edge routing on complex graphs.

**Maintenance**: dagre and dagre-d3 are effectively unmaintained. The latest dagre-d3 release (0.6.4) was published 6+ years ago. The `@dagrejs` scoped packages are also stale (8 years). D3 itself is actively maintained (v7.x), but the dagre integration layer is abandoned. Community recommends migrating to ELK.js or using Cytoscape.js with dagre extension instead.

**Sources**: [dagre GitHub](https://github.com/dagrejs/dagre), [dagre-d3 GitHub](https://github.com/dagrejs/dagre-d3), [alternatives discussion](https://github.com/dagrejs/dagre/issues/318)

---

### ELK.js (standalone)

**Self-contained HTML**: Partially. `elk.bundled.js` can be dropped into a `<script>` tag. However, ELK.js is a layout engine only -- it computes node/edge positions but does not render anything. You need a separate rendering library (D3, SVG manipulation, or a canvas library).

**Directed graph layout**: Excellent. ELK's flagship is the "layered" algorithm, specifically designed for directed node-link diagrams with inherent direction. Also includes mrtree, stress, and force algorithms.

**Rich node content**: N/A -- no rendering. You handle all rendering yourself.

**Pan/zoom**: N/A -- no rendering.

**Bundle size**: ~1.3 MB minified for the full bundle (600 kB for core + layered, 100 kB for remaining algorithms, plus GWT runtime overhead from Java-to-JS transpilation). This is the heaviest option.

**Layout quality**: Best-in-class for directed/layered graphs. Supports ports (explicit edge attachment points), edge routing, compound nodes, and extensive configuration. Academic-grade algorithms from the Eclipse ecosystem.

**Maintenance**: Active. Maintained by the Kiel University / Eclipse Foundation. Latest versions published regularly on npm.

**Sources**: [elkjs GitHub](https://github.com/kieler/elkjs), [elkjs npm](https://www.npmjs.com/package/elkjs), [ELK layout options](https://eclipse.dev/elk/reference/options.html)

---

### Mermaid

**Self-contained HTML**: Yes. Can be loaded via a single `<script>` tag and renders diagrams from text descriptions embedded in the page. However, the bundle is very large: ~2.0-2.8 MB minified depending on version and variant. The `@mermaid-js/tiny` package trims to ~1.95 MB by excluding some diagram types.

**Directed graph layout**: Yes. Supports state diagrams (`stateDiagram-v2`) and flowcharts with directed edges. Uses dagre internally (and optionally ELK via flowchart-elk).

**Rich node content**: Limited. Content is defined through Mermaid's text DSL, not arbitrary HTML. Node labels support basic markdown in some diagram types. Tooltips are supported but have a known bug (since August 2025) where they render at the bottom of the page instead of near the hovered node. Click callbacks exist for flowcharts and were recently added for state diagrams via PR #6423, but they're limited to URL navigation or simple JS callbacks -- not rich interactive overlays.

**Pan/zoom**: Not built-in. Mermaid renders static SVGs. Pan/zoom requires wrapping the output in a separate pan/zoom library (like svg-pan-zoom).

**Bundle size**: ~2.05 MiB minified. The bulk comes from multiple layout engines (ELK, cytoscape) bundled for different diagram types. This is the largest option by far.

**Layout quality**: Good for simple diagrams. Uses dagre internally. Complex state diagrams with many conditional transitions can produce cluttered layouts.

**Maintenance**: Very active. 74k+ GitHub stars, v11.12+ as of early 2026. Major ecosystem adoption (GitHub, VS Code 2026, Obsidian, etc.).

**Sources**: [Mermaid docs](https://mermaid.js.org/), [state diagrams](https://mermaid.js.org/syntax/stateDiagram.html), [bundle size discussion](https://github.com/orgs/mermaid-js/discussions/4314), [tooltip bug](https://github.com/mermaid-js/mermaid/issues/6810), [click for states PR](https://github.com/mermaid-js/mermaid/pull/6423)

---

### @hpcc-js/wasm-graphviz (Graphviz WASM)

**Self-contained HTML**: Partially. The WASM module needs to be loaded (either inlined as base64 or fetched as a separate .wasm file). True single-file embedding requires base64-encoding the WASM blob.

**Directed graph layout**: Excellent. Graphviz's `dot` engine is the gold standard for directed graph layout (Sugiyama-style). Decades of refinement.

**Rich node content**: Limited to Graphviz's label system (HTML-like labels with tables, fonts, colors). No JavaScript interactivity built in -- output is static SVG. Interactivity requires post-processing the SVG with D3 or custom JS (d3-graphviz does this).

**Pan/zoom**: Not built-in. SVG output needs wrapping.

**Bundle size**: The WASM module is ~2-3 MB. Significant for embedding.

**Layout quality**: Best-in-class for static directed graph layout. Decades of algorithmic refinement.

**Maintenance**: Active. @hpcc-js/wasm-graphviz v2.32.3 published within the last week.

**Sources**: [hpcc-js-wasm GitHub](https://github.com/hpcc-systems/hpcc-js-wasm), [d3-graphviz](https://github.com/magjac/d3-graphviz), [@hpcc-js/wasm-graphviz npm](https://www.npmjs.com/package/@hpcc-js/wasm-graphviz)

---

### React Flow / XyFlow

**Not recommended for this use case.** React Flow requires a React runtime and build toolchain. It does not produce self-contained HTML files without a full bundling step. It's designed for interactive editors, not static documentation artifacts. Layout is not built-in -- it delegates to dagre or ELK.js.

**Sources**: [React Flow](https://reactflow.dev/), [XyFlow](https://xyflow.com/)

---

## Comparison Table

| Library | Self-contained HTML | Directed Layout | Rich Nodes | Pan/Zoom | Bundle (min) | Layout Quality | Active |
|---------|-------------------|-----------------|------------|----------|-------------|---------------|--------|
| **Cytoscape.js + dagre** | Yes | Yes (ext) | Yes (ext) | Built-in | ~435 kB | Good | Yes |
| **Cytoscape.js + ELK** | Yes | Yes (ext) | Yes (ext) | Built-in | ~1.7 MB | Excellent | Yes |
| **vis-network** | Yes | Built-in | Limited | Built-in | ~500 kB | Adequate | Moderate |
| **D3 + dagre-d3** | Yes | Yes | Full control | Manual | ~380 kB | Good | No (dagre abandoned) |
| **ELK.js (standalone)** | Layout only | Excellent | N/A | N/A | ~1.3 MB | Best | Yes |
| **Mermaid** | Yes | Yes | Limited (DSL) | No | ~2 MB | Good | Yes |
| **Graphviz WASM** | Difficult | Excellent | Limited | No | ~2-3 MB | Best | Yes |
| **React Flow** | No | Via ext | Full | Built-in | N/A | Via ext | Yes |

## Implications

**Cytoscape.js with the dagre extension is the strongest candidate** for koto's use case. It hits every requirement:
- Single self-contained HTML file (~435 kB with dagre, popper, and HTML labels)
- Clean hierarchical layout for directed state graphs
- Rich interactive overlays via popper/tippy for gate conditions and evidence schemas
- Built-in pan/zoom for large 30+ state workflows
- Active maintenance with a stable API

The main design decision is **dagre vs ELK for layout**. Dagre keeps the bundle small (~435 kB total) and produces good results for typical state machines. ELK produces superior layouts for complex graphs with many crossing edges but adds ~1.3 MB. A practical approach: start with dagre, add ELK as an optional "high-quality" mode if layout quality becomes a problem at 30+ states.

**Mermaid is tempting but wrong for this use case.** Its text DSL cannot express the rich metadata in compiled templates (gate commands, evidence schemas, conditional transitions with `when` clauses). The interactivity story is weak -- tooltips are buggy, click handlers are limited to URL navigation, and there's no pan/zoom. The 2 MB bundle is also the largest option for less capability.

**D3 + dagre-d3 should be avoided** despite its small bundle. The dagre rendering layer is abandoned, and building equivalent interactivity from scratch with D3 would be significant effort.

For the **dual-purpose requirement** (debugging preview + GH Pages documentation):
- The self-contained HTML approach works for both. A single `.html` file with inlined JS serves as a local preview opened in a browser and as a GH Pages artifact.
- The Go CLI (`koto template compile --visualize`) would generate this HTML file alongside the compiled JSON.
- Interactive features (hover tooltips, click-to-expand) work identically in both contexts since it's the same file.

## Surprises

1. **Mermaid's bundle size is enormous** (~2 MB) because it internally bundles both ELK.js and Cytoscape.js for different diagram types. Using Cytoscape.js directly is smaller while providing more control.

2. **dagre-d3 is effectively dead.** The last release was 6+ years ago. Despite dagre still being used internally by many tools (including Mermaid), the D3 rendering wrapper is abandoned. Cytoscape.js has absorbed dagre as a maintained extension.

3. **Mermaid's tooltip implementation has a known unfixed bug** since August 2025 where tooltips render at the bottom of the page. Click support for state diagrams was only added recently via a community PR. The interactivity story is less mature than it appears.

4. **ELK.js's size comes from GWT transpilation** (Java to JavaScript). The algorithms are excellent but carry runtime overhead from the compilation approach. This is an inherent tradeoff that won't improve without a rewrite.

5. **Cytoscape.js has academic-grade credibility** -- it's published in Bioinformatics (Oxford Academic) and widely used in computational biology for graph visualization. This means the core rendering and interaction model is battle-tested on very large graphs.

## Open Questions

1. **Edge label rendering**: How well does Cytoscape.js handle labels on edges (for transition conditions / `when` clauses)? Edge labels in directed graphs can cause layout issues. Needs a prototype.

2. **Compound/nested nodes**: Koto templates don't currently have nested states, but if they ever do, Cytoscape.js supports compound (parent-child) nodes natively. Worth keeping in mind for forward compatibility.

3. **Inlining strategy**: Should the JS libraries be inlined as base64 data URIs, or concatenated directly into the HTML? Base64 adds ~33% overhead. Direct concatenation in a `<script>` tag is smaller and simpler.

4. **Dark mode**: Should the visualization support both light and dark themes for GH Pages display? Cytoscape.js styles are fully programmable, so this is straightforward but needs a design decision.

5. **Go template vs. static generation**: Should the HTML be generated from a Go `html/template` with the graph data injected as JSON, or should the Go code write raw HTML with string concatenation? Template approach is cleaner and more maintainable.

## Summary

Cytoscape.js with the dagre layout extension is the clear best fit -- it produces self-contained interactive HTML at ~435 kB with built-in pan/zoom, rich HTML tooltips via popper/tippy, and clean hierarchical layout for directed state graphs. Mermaid, despite its popularity, is poorly suited because its text DSL cannot express compiled template metadata, its interactivity is limited and buggy, and its bundle is 4-5x larger. The main open question is whether dagre's layout quality holds up at 30+ states or whether the heavier ELK engine will be needed.
