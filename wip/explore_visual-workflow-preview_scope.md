# Explore Scope: visual-workflow-preview

## Core Question

How should koto generate visual, interactive representations of compiled
workflows that serve both as debugging aids during template authoring and as
committable documentation viewable on GitHub Pages? The solution needs to scale
from 2-3 state templates to 30+ state workflows with rich metadata (gates,
evidence schemas, conditional transitions).

## Context

Issue #86 requests a visual workflow preview for compiled templates. Compiled
templates are JSON with a directed graph of states, conditional transitions
(with `when` clauses), gates (command-based), evidence schemas (`accepts`
blocks), variables, default actions, and integration hints.

Workflows range from trivial (2-3 states) to large (30+ states). The work-on
template in shirabe (PR #20) has 17 states and represents a "medium" workflow.

The user wants dual-purpose output: interactive preview for debugging (hover
for gate conditions, click to expand evidence schemas, pan/zoom) AND committable
artifacts displayable on GH Pages as template documentation.

## In Scope

- Rendering technology evaluation (JS graph libraries, formats)
- CLI integration pattern (--preview flag, export subcommand, or both)
- Self-contained output that works locally and on GH Pages
- Interactive features: hover, click-to-expand, pan/zoom
- Visualization of all template concepts (states, transitions, gates, evidence, variables)
- Scaling behavior from small to large graphs

## Out of Scope

- Runtime workflow visualization (watching a workflow execute live)
- Editing workflows through the visual interface
- Server-side rendering or hosted service

## Research Leads

1. **What JS graph rendering libraries produce self-contained, interactive HTML from directed graphs?**
   Evaluate D3/dagre, Cytoscape.js, Elk.js, vis.js, Mermaid and similar options
   for embeddability without external CDN dependencies, rich node/edge annotations,
   and layout quality for directed state graphs.

2. **How do other CLI tools handle "compile and preview in browser"?**
   Tools like cargo doc --open, terraform graph, Storybook, and mdBook solve
   similar problems. What cross-platform browser-launching patterns work well,
   and what pitfalls exist?

3. **What information density approaches work for 30+ state graphs with metadata?**
   At scale, showing all gates, evidence schemas, and conditions inline overwhelms
   the view. What progressive disclosure patterns exist (collapse/expand,
   zoom-dependent detail levels, sidebar panels)?

4. **Can a single-file HTML artifact serve both local preview and GH Pages?**
   Constraints: no build step at view time, no external CDN fetch (offline-capable),
   reasonable file size even for large workflows. What's the size/capability tradeoff?

5. **What does DOT/Mermaid export buy as a simpler fallback?**
   For terminal-only environments or quick debugging, a text-based graph format
   might still be valuable alongside the HTML preview. Worth understanding if this
   is a useful incremental step or just scope creep.
