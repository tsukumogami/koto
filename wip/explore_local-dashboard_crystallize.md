# Crystallize Decision: local-dashboard

## Chosen Type
PRD

## Rationale

Seven research leads mapped the full problem space. Requirements for a fully-functional dashboard emerged during this exploration — they were not given as input. The core question was "what should the dashboard show and why?", which is exactly the PRD signal. User stories and acceptance criteria for all five improvement areas (information hierarchy, narrative context, tree visualization, detail pane redesign, CLI augmentation) are missing and need to be written before implementation begins. Multiple contributors will build from these requirements, so a written contract is needed.

The Design Doc ranked second but was demoted: "what to build is still unclear" is a live anti-signal, and the tiebreaker rule confirms that requirements identified by /explore should go to PRD before a design doc can begin.

## Signal Evidence

### Signals Present
- **Single coherent feature emerged**: the dashboard UX is a bounded, scoped feature with well-understood boundaries
- **Requirements are unclear or contested**: five distinct improvement areas (information hierarchy, narrative context, tree visualization, detail pane, CLI augmentation) each need acceptance criteria
- **Core question is "what should we build?"**: the exploration answered this question — now that answer needs to be captured as requirements
- **User stories missing**: no user stories or acceptance criteria exist for the identified gaps
- **Others need documentation to build from**: this is a public project; implementation will happen across multiple issues by multiple contributors

### Anti-Signals Checked
- **Requirements were provided as input**: not present — requirements emerged during exploration rounds, not before
- **Multiple independent features that don't share scope**: not present — all five improvement areas share a single scope (the dashboard) and inform each other

## Alternatives Considered

- **Design Doc**: Partially fit (three architectural decisions were made during exploration), but the anti-signal "what to build is still unclear" demotes it below PRD. The PRD tiebreaker confirms: requirements identified by /explore → PRD first.
- **Plan**: Does not fit — no upstream PRD or design doc exists to sequence from. Two anti-signals present.
- **No Artifact**: Does not fit — three anti-signals present (decisions were made, others need documentation, multiple contributors).

## Deferred Types
None scored above 0.
