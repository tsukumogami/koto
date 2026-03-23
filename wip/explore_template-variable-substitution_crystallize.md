# Crystallize Decision: template-variable-substitution

## Chosen Type
Design Doc

## Rationale
Issue #67 has the `needs-design` label and explicitly requires a child design doc
before implementation. Requirements are fully specified by the issue and parent
design (DESIGN-shirabe-work-on-template.md). The exploration's five leads all
converged on technical decisions about how to build the feature — API shape,
module placement, sanitization strategy, value typing, and validation flow.
These architectural choices need permanent documentation.

## Signal Evidence
### Signals Present
- What to build is clear, but how to build it is not: issue #67 specifies the
  feature; exploration investigated implementation approaches
- Technical decisions need to be made between approaches: sanitization (allowlist
  vs escaping vs env vars), API shape (standalone function vs newtype vs trait),
  value typing (String vs serde_json::Value)
- Architecture and system design questions remain: module placement
  (engine/substitute.rs), integration with gate closure pattern, event type changes
- Architectural decisions were made during exploration: allowlist sanitization,
  Variables newtype, String typing, strict undefined-reference errors
- Core question is "how should we build this?": yes

### Anti-Signals Checked
- What to build is still unclear: not present — requirements are clear
- No meaningful technical risk or trade-offs: not present — multiple trade-offs exist
- Problem is operational, not architectural: not present — it's architectural

## Alternatives Considered
- **Plan**: Ranked lower because the issue requires a design doc first, and
  technical decisions haven't been formally recorded yet. Plan would be premature.
- **PRD**: Ranked lower because requirements were given as input (issue #67 and
  parent design), not discovered during exploration.
- **No artifact**: Ranked lower because multiple architectural decisions were made
  during exploration that would be lost when wip/ is cleaned.
