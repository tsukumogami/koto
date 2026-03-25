# Lead: Information density approaches for large state graphs

## Findings

### 1. Semantic Zoom (Zoom-Dependent Detail Levels)

Semantic zoom changes *what* is shown at each zoom level, not just the scale. Unlike geometric zoom (uniform magnification), semantic zoom adapts node representations qualitatively. Research from Fraunhofer and others shows this approach separates information into discrete levels of detail while preserving the user's mental map.

**Three-level model for koto states:**

| Zoom Level | Node Content | Edge Content |
|------------|-------------|--------------|
| Far (overview) | State name + type icon (terminal, gated, branching) | Simple arrows, no labels |
| Mid (working) | Name + transition targets + gate count badge | Arrows with `when` condition summaries |
| Near (detail) | Full directive excerpt, gate list, evidence schema fields, default action | Full condition text on edges |

React Flow implements this directly via its `useStore` hook -- custom nodes read the viewport zoom level and conditionally render content. The pattern is lightweight: a selector like `s.transform[2] >= 0.9` returns a boolean that toggles between a skeleton/summary view and full detail. No custom zoom handlers needed.

**Key principle:** Information introduced at a zoom level must persist at all deeper levels (semantic consistency). A gate badge visible at mid-zoom becomes a gate list at near-zoom, never disappears.

### 2. Collapsible Groups / Subgraph Clustering

Airflow's TaskGroup feature demonstrates the dominant pattern for DAG complexity management: hierarchical grouping with collapse/expand. States are organized into named groups that render as a single compound node when collapsed, expanding inline when clicked. Airflow's Graph View defaults to all groups expanded, but a pending feature request (issue #55305) proposes per-group and per-DAG default collapse state -- evidence that "expand all by default" doesn't scale.

For koto, natural groupings could be:
- **Phase-based**: States sharing a prefix or declared phase (e.g., `planning_*`, `implementation_*`)
- **Linear chains**: Sequences of states with single transitions could auto-collapse into a "pipeline" node showing first/last state
- **Template-declared groups**: An optional `group` field in template state declarations

Cambridge Intelligence's "Combos" pattern (from their KeyLines SDK) extends this further: combos are visual groupings that can be opened, closed, and *nested*, giving detail-on-demand without removing data from the chart. The nested aspect matters -- a 30+ state graph might have 3-4 top-level groups, each containing sub-groups.

### 3. Detail-on-Demand: Sidebar Panel vs. Inline Expansion

Two competing patterns exist for showing metadata when a node is selected:

**Sidebar panel (inspector pattern):**
- XState Visualizer uses this: clicking a state opens a side panel showing full state definition, events, and context
- GitHub Actions' workflow editor VSCode extension uses a side panel for job properties
- Pros: unlimited space for complex metadata, doesn't disturb graph layout, can show code/JSON
- Cons: splits attention between graph and panel, loses spatial context

**Inline expansion:**
- Node physically grows to reveal content (accordions within the node)
- Airflow's grid view uses inline expand for task details
- Pros: metadata stays co-located with the node, no context switching
- Cons: pushes other nodes around, breaks layout stability for graphs over ~15 nodes

**Hybrid approach (recommended for koto):**
- Hover: tooltip with gate summary and transition list (first-layer disclosure)
- Click: sidebar panel with full detail (directive text, evidence schema table, gate definitions, default action config)
- The graph itself never changes layout on interaction -- only visual emphasis (highlight connected states, dim unrelated ones)

### 4. Minimap / Overview+Detail

The overview+detail pattern provides two simultaneous views: a thumbnail overview showing the full graph, and the main viewport showing the working area. Research consistently shows users prefer this (18/20 in one usability study). Implementations exist in:

- React Flow (built-in MiniMap component)
- diagram-js (bpmn-io/diagram-js-minimap)
- JointJS (collapsible minimap)
- G6 (Ant Group's graph framework)

The minimap serves as both a navigation aid (click to teleport) and an orientation aid (where am I in the full graph?). For 30+ state graphs, it's nearly essential -- GitHub Actions' own visualization was criticized (community discussion #18035) because large workflows start zoomed out with unreadable text and no overview mechanism.

### 5. Visual Encoding for State Types

Color and shape encoding reduces the need to read labels. Patterns from XState and AWS Step Functions:

| State Type | Visual Encoding |
|------------|----------------|
| Initial state | Bold border or double circle |
| Terminal state | Filled/dark node or double border (UML convention) |
| Gated state | Lock icon badge or dashed border |
| Branching state (multiple transitions) | Diamond shape or fork icon |
| State with evidence schema | Document icon badge |
| State with default action | Lightning/auto icon badge |
| State with integration hint | Plug/link icon badge |

XState uses blue highlighting for the active state and grays out unreachable states. For koto's debugging use case, color should encode *runtime* status (current, completed, available, unreachable) while *shape/icon* encodes the structural type.

### 6. Filtering and Search

Cambridge Intelligence's five-step approach: filter early, aggregate, choose matching visual models, declutter, then apply layouts. For koto:

- **Filter by state type**: Show only gated states, only terminal states, only states with evidence schemas
- **Filter by reachability**: From a given state, what's reachable? Dim unreachable nodes
- **Search**: Text search across state names, directive text, gate commands -- highlight matches
- **Centrality**: Identify hub states (many incoming/outgoing transitions) for navigation anchors

### 7. Edge Routing and Layout

For directed state graphs specifically:
- **Dagre** (used by React Flow's default layout) handles hierarchical directed graphs well up to ~50-100 nodes
- **Edge bundling** groups edges traveling similar paths, reducing visual clutter for branching-heavy graphs
- **Orthogonal routing** (right-angle edges) reads better than curved edges for state machines because transition paths are easier to trace
- Conditional transitions (`when` clauses) should use different line styles (dashed, colored) to distinguish from unconditional transitions

## Implications

**For koto's design, four patterns are must-haves for 30+ state support:**

1. **Semantic zoom with three levels** is the highest-impact single feature. It solves the core density problem without requiring user interaction. React Flow's contextual zoom example provides a direct implementation path. Each koto `TemplateState` node renders differently based on viewport zoom: name-only, summary, or full detail.

2. **Sidebar inspector panel** for selected-state detail is strongly preferred over inline expansion for koto's graph sizes. The `TemplateState` struct contains too many optional fields (gates, accepts, integration, default_action) to expand inline without destroying layout. The sidebar can render the directive as markdown, the evidence schema as a typed field table, gates as a checklist, and the default action as a code block.

3. **Minimap** is essential at 30+ states. React Flow includes a built-in `MiniMap` component, making this nearly free if React Flow is the rendering layer.

4. **Visual encoding of state types** through icons/badges (terminal, gated, has-evidence, has-action) lets users scan the graph structure without zooming in. The `TemplateState` fields map directly to badge types: `terminal: true` -> filled node, `gates` non-empty -> lock badge, `accepts` present -> document badge, `default_action` present -> auto badge.

**Collapsible groups are a nice-to-have**, not a must-have for v1. They require either template authors to declare groups or a heuristic grouping algorithm. Semantic zoom + minimap handles 30-50 states adequately without grouping. Groups become necessary at 50+ states or when templates have clear phase structure.

**Filtering is valuable but secondary.** For the debugging use case, semantic zoom and the sidebar cover most needs. Filtering matters more for the documentation use case where readers want to understand specific subflows.

## Surprises

1. **GitHub Actions' own graph visualization is widely criticized for large workflows.** The visualization box dominates the page, text is unreadable when zoomed to fit, and there's no progressive disclosure. This is a cautionary tale: rendering the full graph at once without semantic zoom fails at scale.

2. **Airflow defaults to expanding all TaskGroups**, which is the wrong default for large DAGs. The open feature request to collapse by default (issue #55305) confirms what the research predicts: users need the overview first, detail second. Koto should default to mid-zoom, not max detail.

3. **Inline expansion is actively harmful above ~15 nodes.** Multiple sources confirm that nodes changing size during interaction disrupts the user's spatial memory of the graph. The sidebar panel pattern won, which is good news -- it's simpler to implement than layout-preserving inline expansion.

4. **React Flow's contextual zoom is surprisingly simple.** The entire semantic zoom pattern is a ~5 line selector function and a conditional render in the node component. This is not a major engineering lift.

## Open Questions

1. **Static rendering for GitHub Pages**: Semantic zoom and sidebar panels require JavaScript interactivity. What's the fallback for static/printable views? A fixed mid-zoom level with a state detail appendix? Mermaid export for non-interactive contexts?

2. **Grouping heuristic**: If koto doesn't add a `group` field to templates, can useful groups be inferred from transition structure (e.g., connected components, linear chains, states sharing a common prefix)?

3. **Mobile/small viewport**: The sidebar panel pattern assumes sufficient screen width. What happens on narrow viewports -- does the sidebar become a bottom sheet? An overlay?

4. **Edge label density**: For states with multiple conditional transitions, `when` conditions on edges can create their own density problem. Should edge labels also follow semantic zoom levels, or should they always be detail-on-demand (hover only)?

5. **Color accessibility**: The visual encoding scheme relies on color + icon. Has the icon-only path been validated for color-blind users? Do the badge shapes provide sufficient differentiation without color?

## Summary

The most effective pattern for 30+ state graphs with metadata is a three-level semantic zoom (name-only, summary with badges, full detail) combined with a sidebar inspector panel for selected-state deep-dive, a minimap for orientation, and icon/badge encoding for state types. React Flow's contextual zoom provides a direct, lightweight implementation path -- the selector-based pattern requires minimal code while solving the core density problem. Collapsible groups and filtering are valuable additions but not required for the first iteration; semantic zoom plus minimap handles 30-50 states without grouping heuristics.
