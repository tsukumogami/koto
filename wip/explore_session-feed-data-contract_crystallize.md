# Crystallize Decision: session-feed-data-contract

## Chosen Type

Design Doc

## Rationale

Requirements are given (the issue body specifies the contract scope: all current event
types, versioning, reader guarantees, forward-compatibility). The open questions are
architectural: which versioning mechanism, how to handle unknown event types, whether to
classify events by audience, what artifact form to use. Five independent technical
decisions need to be made and recorded in a permanent document. wip/ will be cleaned
before merge — the research findings must be codified in a design doc or they are lost.

## Signal Evidence

### Signals Present

- What to build is clear, how to build it is not: the issue specifies scope; the technical
  approach has competing options (schema_version activation vs. per-event versioning,
  tiered vs. flat spec, markdown vs. JSON Schema)
- Technical decisions between approaches: 5 distinct, resolvable design questions
- Architectural questions remain: versioning semantics, reader behavior contract,
  event classification tier model
- Decisions made during exploration that should be on record: all 6 decisions in
  the decisions file need a permanent home
- Core question is "how should we build this?": correct, the what is given

### Anti-Signals Checked

- "What to build is still unclear": not present. Issue body is precise about scope.
- "No meaningful trade-offs": not present. Each decision has 3+ viable options.

## Alternatives Considered

- **PRD**: Requirements were given as input (the issue body), not discovered during
  exploration. The PRD tiebreaker favors Design Doc when requirements are given.
- **Decision Record**: Multiple interrelated decisions exist (5 total), which
  pushes toward a design doc over an isolated decision record.
- **Plan**: No upstream design doc exists for this topic yet, so Plan would be
  premature.

## Auto Mode Note

Crystallize decision made automatically in --auto mode. Evidence is clear and
unambiguous: needs-design label on the source issue, design doc explicitly requested
in acceptance criteria, five technical decisions with competing viable options, all
requiring permanent documentation. Confidence: high.
