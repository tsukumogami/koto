# Crystallize Decision: koto-implement-workflow-gaps

## Chosen Type

Design Doc

## Rationale

What to build is clear: a merged work-on/just-do-it skill in shirabe backed by a koto
template, with optional GitHub issue input. How to build it is not: the koto template
structure (state list, evidence schema, skip pattern), shirabe's invocation mechanics
(SessionStart hook, koto init, directive loop), and session resume behavior all need
architectural decisions before implementation can begin. Multiple decisions were made
during exploration that must go on record — agent-as-integration, no integration runner
for phase 1, optional issue input — and these need a permanent home before wip/ is
cleaned.

## Signal Evidence

### Signals Present

- What to build is clear, how to build it is not: merged skill + koto template are
  decided; template state list, evidence schema, skip pattern, shirabe invocation are not
- Technical decisions remain open: skip pattern expression, evidence schema design,
  session resume mechanics, shirabe directive loop
- Architectural decisions made during exploration that should be on record:
  agent-as-integration, no integration runner needed for phase 1, optional issue input
- Architecture questions remain: shirabe invocation mechanics (SessionStart hook,
  koto init, directive loop)
- Core question is "how should we build this?": yes, the template design is the open question

### Anti-Signals Checked

- What to build is still unclear: not present — what to build is settled
- No meaningful technical risk or trade-offs: not present — skip pattern and shirabe
  invocation have open questions
- Problem is operational, not architectural: not present — template design is architectural

## Alternatives Considered

- **Plan**: No upstream design doc exists yet; template structure has open questions
  that need to be decided before issues can be sequenced. Demoted.
- **PRD**: Requirements were provided as input (user knew what to build before
  exploration started). Demoted.
- **No Artifact**: Four anti-signals present — architectural decisions made, scope
  shifted across rounds, others need docs to build from, multiple repos involved.
  Demoted.
