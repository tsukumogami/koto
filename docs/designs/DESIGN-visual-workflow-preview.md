---
status: Proposed
problem: |
  Compiled koto workflows are JSON state graphs that are difficult to review
  visually. Developers need an interactive preview to spot structural issues
  (unreachable states, missing transitions, dead ends) and a committable
  artifact for template documentation on GitHub Pages.
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
