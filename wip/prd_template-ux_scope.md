# /prd Scope: template-ux

## Problem Statement

Koto workflow templates compile to JSON state graphs that are hard to review
visually. Template authors can't quickly spot structural issues, PR reviewers
see raw YAML diffs without understanding the graph shape, and there's no CI
enforcement that committed diagrams stay in sync with their source templates.

## Initial Scope

### In Scope

- `koto template compile` optionally emitting a Mermaid diagram alongside (or inside) the compiled output
- `koto template preview` as a separate command for interactive HTML debugging (Cytoscape.js, not committed)
- A reusable GitHub Actions workflow that koto users add to their repos: compiles templates, validates output, enforces no Mermaid drift
- Clear user journeys for each capability (author, reviewer, repo maintainer)

### Out of Scope

- Mermaid auto-embedded inside the compiled JSON struct
- Preview HTML as a committed/CI artifact
- Local server or live-reload for preview
- ELK.js or alternative layout engines

## Research Leads

1. **Mermaid output strategy**: Three options — (a) separate `.mermaid.md` sibling file, (b) in-place update of a fenced code block in the source template `.md`, (c) flag-gated on compile. Each has trade-offs around brittleness, discoverability, and CI enforcement. Need to evaluate precedent from other tools and the koto template format constraints.

2. **GHA workflow contract**: What should the reusable workflow enforce? Compile + validate + mermaid freshness minimum. Should it produce PR comments with rendered diagrams? Look at patterns from other reusable workflows (buf, protoc, actionlint).

3. **Preview command interaction model**: Write-file-then-open for debugging use case. Should v1 support `--watch` or is re-run sufficient? Depends on compile step weight.

4. **Mermaid file location and naming**: Where does the diagram artifact live relative to source and compiled output? Affects GHA drift checks and repo organization.

## Coverage Notes

- User confirmed three personas: template author (debugging), PR reviewer, docs reader
- Primary moments: debugging stuck workflows and reviewing template PRs
- Interactive preview is a local dev tool, Mermaid is the committed/reviewable artifact
- User wants e2e usage — capabilities must be wired into real workflows, not standalone commands nobody runs
- User wants interactive preview to remain its own command, not a side-effect of compile
