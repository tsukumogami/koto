# Exploration Decisions: visual-workflow-preview

## Round 1
- Cytoscape.js + dagre via CDN over server-side Rust layout: automatic layout is the killer feature, and CDN eliminates the file size penalty. Both use cases (local dev, GH Pages) are online. Not worth implementing and maintaining a layout algorithm in Rust.
- Mermaid eliminated for interactive HTML: 2 MB bundle, buggy tooltips, text DSL can't express koto's rich metadata. Mermaid remains valuable as a separate lightweight export format.
- Tippy.js/Popper dropped: vanilla JS tooltips work fine, avoids two extra CDN dependencies with no meaningful loss in quality.
- No local server needed: "write file, open with opener crate" pattern is sufficient. Live-reload is a future concern.
- D3 + dagre-d3 eliminated: dagre-d3 rendering layer is abandoned (6+ years stale). Cytoscape.js absorbed dagre as a maintained extension.
- ELK.js deferred: 1.3 MB is too heavy for default use. Could be added later if dagre's layout quality breaks down at 30+ states.
