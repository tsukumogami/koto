# Decision 2: Mermaid Representation

## Chosen: Option A -- Minimal mapping

## Rationale

Mermaid's `stateDiagram-v2` is a secondary, lightweight export format (the design already chose Cytoscape.js for the rich interactive view). The Mermaid output's job is to render inline in GitHub PRs, READMEs, and issue comments -- contexts where brevity wins over completeness.

Option A maps the core graph structure cleanly:

- Each `TemplateState` becomes a Mermaid state with its name. The directive text is set as the state description (truncated to ~60 chars to avoid wrapping issues).
- `[*] --> initial_state` marks the entry point.
- Each `Transition` becomes an arrow. Unconditional transitions get no label. Conditional transitions use the `when` map serialized as a compact label, e.g., `route: setup`.
- Terminal states get `terminal_state --> [*]`.
- Gates are shown as `note left of` annotations, listing gate names and commands.
- Evidence schemas and default actions are omitted entirely.

This keeps the output valid on GitHub (no custom CSS, no classDef -- GitHub strips both), readable at all scales, and fast to generate. Evidence schemas, default actions, integrations, and variables don't belong on a graph meant for structural overview -- they're inspection-level detail better served by the Cytoscape.js interactive view or `koto query`.

For a 5-state template with conditional routing, the output looks like:

```
stateDiagram-v2
    direction LR
    [*] --> explore
    explore : explore
    explore --> evaluate
    evaluate : evaluate
    evaluate --> implement : route: build
    evaluate --> research : route: investigate
    implement : implement
    implement --> done
    research : research
    research --> evaluate
    done : done
    done --> [*]
    note left of explore : gate: check-repo (test -d .git)
```

## Alternatives Considered

**Option B: Rich mapping with notes** -- Rejected. Adding evidence schemas as right-notes doubles the visual footprint. On a 15-state workflow, left-notes for gates plus right-notes for evidence creates a wall of text that obscures the graph structure. Mermaid's note rendering is also inconsistent across renderers (GitHub vs. Mermaid Live vs. local CLI), and multi-line notes often break alignment. The interactive HTML view already covers detailed inspection.

**Option C: Choice nodes for branching** -- Rejected. Mermaid's `<<choice>>` pseudo-states add a diamond node with no label, then require separate arrows from the diamond to each target. For a state with two conditional transitions, this turns one state + two labeled arrows into one state + one unlabeled arrow + one diamond + two labeled arrows -- tripling the visual elements with no information gain. The `when` condition on the arrow label already communicates the branching logic. Choice nodes also interact poorly with `note` annotations, since you can't attach notes to pseudo-states.

**Option D: Composite states for phases** -- Rejected. Automatic grouping by naming prefix is fragile (what if states are named `setup`, `setup-review`, and `setup-ci` -- is `setup` its own group or a prefix?). Composite states in Mermaid also have rendering bugs on GitHub: nested state labels can overlap, and transitions crossing composite boundaries sometimes misroute visually. This would require the user to annotate phases in the template, which adds schema complexity for a marginal visual improvement. Not worth it for a lightweight export format.

## Assumptions

- GitHub will continue to support `stateDiagram-v2` without `classDef` or custom styling (their Mermaid renderer strips CSS classes).
- Directive text can be safely truncated to ~60 characters without losing meaning for the graph overview use case.
- Users who need evidence schema details, gate commands, or action configurations will use the interactive HTML preview or `koto query` rather than the Mermaid diagram.
- The `when` condition maps are small enough (typically 1-2 fields) to fit as arrow labels without breaking Mermaid's layout.
- Most templates stay under 30 states. The design document already deferred ELK.js for edge cases beyond that threshold.

## Confidence: High

The mapping is straightforward, GitHub-compatible, and well-scoped for a secondary export format. The primary interactive view (Cytoscape.js) handles the rich metadata use case, so there's no pressure to overload the Mermaid output.
