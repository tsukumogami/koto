# Crystallize Decision: visual-workflow-preview

## Chosen Type
Design Doc

## Rationale
Exploration made concrete architectural decisions that need permanent documentation:
rendering library (Cytoscape.js + dagre), delivery strategy (CDN-loaded, not inlined),
tooltip implementation (vanilla JS, not tippy/popper), Mermaid as a separate MVP,
and browser launching via the opener crate. These decisions affect future contributors
who need to understand why alternatives were eliminated and what constraints apply.

The remaining implementation design (CLI integration details, dark mode, Mermaid
representation, badge annotations) also fits a design doc's scope.

## Signal Evidence
### Signals Present
- What to build is clear: issue #86 defines the goal, acceptance criteria, and user stories
- How to build it required investigation: three rendering approaches prototyped and compared
- Technical decisions made between approaches: Cytoscape CDN chosen over server-side Rust layout and inlined libraries
- Multiple viable implementation paths explored: server-side SVG, Cytoscape inlined, Cytoscape CDN
- Architectural decisions made during exploration that should be on record: library choice, CDN strategy, tooltip approach, Mermaid as separate deliverable

### Anti-Signals Checked
- "What to build is still unclear": Not present. Goal and acceptance criteria are well-defined.
- "No meaningful technical risk or trade-offs": Not present. File size, offline capability, and layout quality are real trade-offs.
- "Problem is operational, not architectural": Not present. This is an architectural decision about rendering strategy.

## Alternatives Considered
- **PRD**: Score -2. Requirements were provided as input (issue #86), not discovered during exploration. The "what" was never in question.
- **Plan**: Score 1 (demoted). Work is decomposable but no upstream artifact captures the architectural decisions yet. A plan without a design doc would scatter rationale across issue descriptions.
- **No Artifact**: Score -3. Multiple architectural decisions were made that need permanent documentation. wip/ cleanup would erase all decision context.

## Deferred Types
- **Decision Record**: Partially fits (core pattern was choosing between rendering approaches). Design Doc chosen instead because it captures the decision with fuller context and also accommodates remaining implementation design work.
