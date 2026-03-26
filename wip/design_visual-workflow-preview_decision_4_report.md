# Decision 4: Visual Design

## Decisions:

### Dark mode: Yes, include prefers-color-scheme support
The server-side prototype already has a working dark-mode media query block. Port it to the Cytoscape HTML template. It's roughly 15 lines of CSS -- well within the "minimal file size" constraint -- and avoids a jarring white page for developers using dark system themes. No toggle needed; `prefers-color-scheme: dark` is automatic.

### Badge annotations: No, keep nodes clean
Gate and evidence counts are available on hover via the tooltip. Adding badge text to nodes increases visual noise, makes the dagre layout wider (longer labels need bigger boxes), and scales poorly at 30+ states. The tooltip already surfaces this information on demand.

### Edge labels: Rotated labels with opaque background, no further changes needed
The prototype's approach -- `text-rotation: autorotate` plus a near-opaque `text-background-color` -- handles overlap well enough for typical workflows. For dense branching states (5+ conditional edges from one node), dagre's edge separation (`edgeSep: 30`) pushes edges apart. No additional mitigation is needed now; if real templates expose overlap problems, a follow-up can switch to `taxi` curve style or increase `edgeSep`.

### Terminal markers: No UML end markers
The green fill plus the legend is sufficient. A synthetic `[*]` end node adds clutter, creates an extra edge for every terminal state, and doesn't carry information the color encoding already provides. Koto workflows commonly have a single terminal state, so the green node stands out on its own.

### Initial markers: Yes, add a small start marker
Add a synthetic `[*]` start node (small filled circle, no label) with an edge into the initial state. Unlike terminal markers, the initial state isn't always obvious from color alone -- a user scanning a large graph benefits from an unambiguous entry point. The marker is a single 12px filled circle, negligible layout cost.

### Fonts: System font stack for node labels, monospace for edge labels and tooltip code
Keep the existing split: `-apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif` for state names and body text, `'SF Mono', 'Fira Code', 'Cascadia Code', 'Consolas', monospace` for edge condition labels and code snippets in tooltips. All are system fonts or have a generic fallback, so no web font loading is needed. GitHub Pages compatibility is not a concern since no external font files are required.

## Rationale:
The prototype is close to production quality. The two additions -- dark mode CSS and the start marker -- address real usability gaps without adding complexity. Dark mode is a straight port from the server-side prototype. The start marker solves a genuine readability problem in larger graphs. Everything else (badges, terminal markers, font changes) would add visual weight without proportional value. Edge label styling works as-is and can be revisited if real-world templates surface overlap issues.

## Confidence: High
All six items have clear precedent from the prototypes, and none introduce architectural risk. The choices are easy to adjust later since they're purely CSS and a single synthetic node.
