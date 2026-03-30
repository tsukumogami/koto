# Crystallize Decision: koto-template-authoring-skill

## Chosen Type
Design Doc

## Rationale
The exploration established clear requirements (a meta-skill for authoring skills-with-koto-templates) but the implementation approach needs design work. Multiple technical decisions were made during exploration (validation via compile, distribution via marketplace, layered teaching) that need to be recorded permanently. The core question is "how should we build this?" which is the Design Doc's sweet spot.

## Signal Evidence
### Signals Present
- What to build is clear, but how to build it is not: we know the output (paired SKILL.md + bundled template) but workflow phases, reference material structure, and integration points need design
- Technical decisions need to be made between approaches: compile vs eval validation, teaching layers, workflow phases
- Architectural decisions were made during exploration that should be on record: validation via `koto template compile`, distribution via marketplace, layered teaching (linear -> evidence routing -> advanced)
- The core question is "how should we build this?": confirmed throughout exploration

### Anti-Signals Checked
- "What to build is still unclear": not present -- the user was clear about what they want
- "No meaningful technical risk or trade-offs": not present -- multiple design decisions remain
- "Problem is operational, not architectural": not present -- this is architectural

## Alternatives Considered
- **PRD**: ranked lower because requirements were provided as input (the user told us what to build), not discovered during exploration. Anti-signal triggered demotion.
- **Plan**: ranked lower because no upstream design doc exists yet, and open architectural decisions remain. Can't sequence work that hasn't been designed.
- **No Artifact**: ranked lower because architectural decisions made during exploration need permanent documentation. wip/ artifacts are cleaned before merge.
- **Rejection Record**: no signals matched. We're not rejecting this work.
