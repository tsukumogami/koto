# Lead: Single-file HTML viability for dual local/GH Pages use

## Findings

### 1. Can a single HTML file work on both file:// and GitHub Pages?

Yes, with one critical constraint: **no ES module imports and no external resource fetches**. Modern browsers treat `file://` origins as opaque/null, blocking all cross-origin requests and ES module loading. But classic `<script>` tags with inline JavaScript execute fine on both `file://` and `https://`. A single HTML file with all JS/CSS inlined in `<script>` and `<style>` tags works identically when double-clicked locally and when served from GitHub Pages.

The key restrictions:
- No `<script type="module">` (blocked on file://)
- No `fetch()` or `XMLHttpRequest` to external URLs
- No `<link rel="stylesheet" href="...">` to external CSS
- Inline `<script>` and `<style>` blocks work on both protocols
- SVG embedded directly in the HTML DOM works everywhere
- Data URIs for images work everywhere

### 2. JavaScript library sizes for graph visualization

Approximate minified sizes for libraries that could render state machine diagrams:

| Library | Minified | Gzipped | Notes |
|---------|----------|---------|-------|
| Mermaid.js | ~2.7 MB | ~800 KB | Full diagramming; enormous for inline use |
| D3 (full) | ~270 KB | ~85 KB | Overkill; most modules unused for graphs |
| D3 (subset: d3-selection + d3-shape + d3-transition) | ~30-50 KB | ~10-15 KB | Enough for SVG manipulation |
| Cytoscape.js | ~280 KB | ~90 KB | Full graph library with layout |
| Dagre (@dagrejs/dagre) | ~30 KB | ~10 KB | Layout only, no rendering |
| ELK.js | ~1.2 MB+ | ~400 KB+ | GWT-compiled WASM; too large |
| Vanilla JS + SVG | 0 KB | 0 KB | Custom layout, custom rendering |

The standout finding: **Mermaid and ELK are disqualified by size alone.** A self-contained HTML inlining Mermaid would be 2.7+ MB before any workflow data. That's viable but wasteful -- every compiled template preview would add ~3 MB to the repository.

### 3. Server-side layout vs client-side layout

Two fundamentally different approaches:

**Approach A: koto computes layout at compile time, emits static SVG**
- The Go/Rust binary computes node positions and edge routing
- The HTML file contains pre-positioned SVG with minimal JS for interactivity (tooltips, click-to-highlight)
- JS payload: under 5 KB for pan/zoom/tooltip behavior
- Total HTML file: 10-50 KB depending on workflow complexity
- Requires implementing a layout algorithm in Rust

**Approach B: koto emits JSON data, browser computes layout**
- The HTML file bundles a JS layout library (dagre: ~30 KB) and rendering code
- Browser computes positions on load, renders to SVG or Canvas
- JS payload: 40-60 KB (dagre + rendering logic)
- Total HTML file: 50-100 KB
- Layout quality depends on the JS library

Approach A produces dramatically smaller files and works without JavaScript (SVG renders natively). Approach B is simpler to implement but bundles 30+ KB of layout code into every preview file.

### 4. GitHub Pages constraints

- Individual files: hard limit at 100 MB (warned at 50 MB)
- Repository: recommended under 1 GB
- Serves static files only; no server-side processing
- Automatically applies gzip compression when serving (not maximum compression, but decent)
- No support for pre-compressed .gz files or Brotli
- No CORS issues for same-origin inline content

For koto's use case, even the heaviest approach (inlining Mermaid at ~3 MB) stays well under file limits. The real concern is git repository bloat -- if a project has 20 templates, each with a 3 MB preview, that's 60 MB of generated HTML in the repo. With server-side layout (Approach A), 20 templates at 30 KB each is 600 KB total.

### 5. Git performance thresholds

- Files under 1 MB: no performance impact
- Files 1-10 MB: noticeable on clone/fetch but manageable
- Files over 50 MB: GitHub warns on push
- Files over 100 MB: GitHub rejects the push

For generated preview files, staying under 500 KB each keeps them firmly in the "no one notices" range. Under 100 KB is ideal.

### 6. Base64 encoding overhead

Base64 adds 33% size overhead. Inlining images as data URIs is costly. For koto's case, the visualization is SVG (text-based), so no base64 is needed -- SVG goes directly in the HTML DOM. This is a non-issue.

### 7. Compression characteristics

GitHub Pages serves content with on-the-fly gzip. JavaScript and SVG compress extremely well (typically 60-80% reduction). A 50 KB HTML file with inline JS would transfer as ~15 KB over the network. This means the transfer size for Approach B (dagre-based) would be ~20-30 KB, and for Approach A (static SVG) would be ~5-10 KB.

However, gzip does not help with git storage -- git stores objects with its own compression, and generated HTML with embedded JS doesn't delta-compress well between versions.

### 8. Real-world precedents

**Plotly** produces self-contained HTML files by inlining its entire ~3 MB library. This is the industry standard for "offline-capable data viz" but results in files that are painful to commit. Plotly offers a `include_plotlyjs='cdn'` option for smaller files, acknowledging the tradeoff.

**Graphviz/go-graphviz** (goccy/go-graphviz) embeds Graphviz as WASM and can produce SVG server-side in Go/Rust. This is Approach A -- compute layout at build time, emit static SVG.

**state-machine-cat** produces static SVG from state machine definitions. It computes layout server-side (via Graphviz dot) and outputs SVG.

### 9. What koto's workflows actually look like

Based on the codebase, koto workflows are directed graphs with:
- 2-17 states (based on hello-koto at 2 states, shirabe work-on at 17)
- Transitions with optional conditions (when clauses)
- Gates on states
- Terminal states marked explicitly
- One initial state

This is a small graph. Even 17 nodes with ~25 edges is trivially layoutable. No library like ELK or Cytoscape is needed for graphs this size. A simple layered layout algorithm (Sugiyama-style) in ~200-400 lines of Rust would handle it, or dagre's ~30 KB JS bundle would work on the client side.

### 10. Interactivity requirements

For a debugging/documentation tool, useful interactivity includes:
- Hover to see state details (directive text, gates, transitions)
- Click to expand state information
- Pan and zoom for larger workflows
- Highlight the current state (if workflow state is provided)
- Color-coding for terminal states, gated states, decision points

Pan/zoom on SVG: ~50 lines of vanilla JS. Tooltips: ~30 lines. Click-to-expand: ~50 lines. Total interactivity layer: well under 5 KB unminified.

## Implications

### Recommended approach for koto

**Server-side layout (Approach A) is the clear winner.** The reasons compound:

1. **File size**: 10-50 KB per preview vs 50-100 KB (Approach B) or 2-3 MB (Mermaid). With templates potentially committed to repos, this matters.

2. **No JS dependency risk**: Static SVG renders even with JS disabled. The interactivity layer is optional enhancement, not required for viewing.

3. **koto already has the data**: The compiled template JSON contains the full graph structure. Computing positions is a natural extension of compilation.

4. **Graph size is bounded**: Workflows have 2-20 states. A simple layered layout handles this without needing a general-purpose graph layout engine.

5. **Diffability**: When a template changes and is recompiled, the SVG diff is meaningful (node positions shift, edges change). A diff of an inlined 30 KB JS bundle is noise.

### Implementation sketch

The `koto template compile --preview` command would:
1. Parse and compile the template (already exists)
2. Run a Sugiyama-style layered layout on the state graph (new code, ~300-400 lines of Rust)
3. Emit an HTML file containing:
   - `<style>` block with node/edge styles (~2 KB)
   - `<svg>` element with positioned nodes and edges (varies, ~5-30 KB)
   - `<script>` block with pan/zoom and tooltip interactivity (~3 KB)
   - State metadata as a JSON blob in a `<script>` tag for tooltip content (~1-5 KB)

Estimated total file size: **15-45 KB** for a 17-state workflow.

### Alternative: hybrid approach

If implementing layout in Rust feels premature, a stepping stone:
1. Emit the HTML with dagre (~30 KB) inlined
2. Include the graph data as inline JSON
3. Browser computes layout on load, renders SVG

This gets the feature shipping faster at the cost of ~30 KB per file. Migration to server-side layout can happen later without changing the output format (still a single HTML file).

## Surprises

1. **Mermaid is enormous.** At 2.7 MB minified, it's not a viable inline dependency. The common advice to "just use Mermaid" doesn't account for self-contained offline files.

2. **file:// works fine for inline content.** The CORS restrictions on file:// only apply to fetches, module imports, and cross-origin resources. Inline scripts and SVG work without issues. This makes the single-file approach fully viable for local preview.

3. **The workflow graph scale is tiny.** With a maximum of ~20 nodes and ~30 edges, this isn't a "graph visualization problem" that needs a sophisticated library. It's closer to "draw a flowchart" -- manageable with basic layout algorithms.

4. **GitHub Pages gzips automatically.** The network transfer size is much smaller than the file size on disk. A 50 KB file transfers as ~15 KB. This lessens the urgency of minimizing file size for viewing purposes, though git storage still matters.

5. **Go-graphviz exists as pure-Go with WASM-embedded Graphviz.** If koto were in Go, this would be a turnkey solution for server-side layout. The Rust ecosystem is thinner here, but the layout algorithm for small directed graphs is straightforward to implement.

## Open Questions

1. **Should the preview include workflow runtime state?** If `--preview` also accepts a state file, it could highlight the current state, show completed transitions, and display evidence. This would make it a live debugging tool, not just a static template visualization.

2. **Should the layout algorithm be a separate crate?** A small Sugiyama layout implementation could be useful beyond koto. But extracting it prematurely adds overhead.

3. **What rendering style looks good?** Rounded rectangles for states, arrows for transitions -- but the visual design (colors, fonts, spacing) affects usability. Should this be configurable or opinionated?

4. **SVG vs Canvas for the rendering target?** SVG is the obvious choice for this scale (DOM-accessible, style-able with CSS, searchable text). Canvas would only matter at hundreds of nodes, which koto won't hit.

5. **Should the HTML include the full template source?** Embedding the markdown source in a collapsible section would make the preview file a complete reference document, not just a diagram.

6. **What about dark mode?** CSS `prefers-color-scheme` media queries work in inline styles. Supporting both light and dark mode adds ~1 KB of CSS but significantly improves the viewing experience.

## Summary

A single-file HTML artifact works on both file:// and GitHub Pages without restrictions, as long as all JavaScript and CSS are inlined and no external resources are fetched. For koto's workflow graphs (2-20 nodes), server-side layout in Rust producing static SVG with a thin interactivity layer yields files of 15-45 KB -- small enough to commit freely and diff meaningfully. The main alternative, inlining a client-side layout library like dagre (~30 KB), is a viable faster-to-ship stepping stone but adds per-file overhead that compounds across repositories with many templates.
