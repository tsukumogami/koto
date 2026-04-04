# Crystallize Decision: hierarchical-workflows

## Chosen Type
Design Doc

## Rationale
Issue #127 provides clear requirements and acceptance criteria (the "what"), but the exploration surfaced multiple competing approaches for each major design dimension (fan-out primitive, lineage model, query interface, isolation model). Six architectural decisions were made during Round 1 that need permanent documentation. The core question is firmly "how should we build this?" -- the exploration answered it but those answers live only in wip/ artifacts that will be cleaned before merge.

## Signal Evidence
### Signals Present
- What to build is clear, but how to build it is not: #127 defines acceptance criteria; exploration evaluated 3 fan-out approaches, 4 lineage options, 4 query interfaces, 3 isolation models
- Technical decisions need to be made between approaches: gate vs action vs state-level, header-only vs dual-event vs directory nesting, metadata vs convention-based isolation
- Architecture, integration, or system design questions remain: polling semantics for temporal gates, CloudBackend S3 implications, parent close policy, cleanup cascading
- Exploration surfaced multiple viable implementation paths: all six leads produced concrete alternatives with trade-off analysis
- Architectural decisions were made during exploration: 6 decisions recorded (gate-based fan-out, header-only lineage, flat storage, naming convention, abandon policy, external templates)
- The core question is "how should we build this?": requirements are given, architecture is the gap

### Anti-Signals Checked
- What to build is still unclear: Not present -- #127 acceptance criteria are detailed
- No meaningful technical risk or trade-offs: Not present -- significant trade-offs in every dimension
- Problem is operational, not architectural: Not present -- this is core engine architecture

## Alternatives Considered
- **PRD**: Ranked lower (0, demoted) because requirements were provided as input to the exploration (#127), not discovered during it
- **Plan**: Ranked lower (-2, demoted) because no upstream design doc exists and open architectural decisions remain
- **No Artifact**: Ranked lower (-1, demoted) because 6 architectural decisions were made that need permanent record, and this is a public repo where others will build from the design
