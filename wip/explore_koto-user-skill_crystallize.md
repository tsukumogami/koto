# Crystallize Decision: koto-user-skill

## Chosen Type

PRD

## Rationale

Three workstreams (koto-author update, koto-user creation, root AGENTS.md) emerged from
exploration, each with distinct requirements that need consolidation before design begins.
The requirements were partially given (the user described what they want) and partially
identified during exploration (specific gap list, AGENTS.md disposition, eval harness
approach). A PRD gathers all of these into a single contract before moving to design and
implementation.

The Design Doc was the framework's top scorer, but the user's decision reflects a real
need: the requirements across the three workstreams are distributed across research files
and findings — they haven't been stated in one place, and the koto-user skill structure
(what it should cover, how deep, how it relates to AGENTS.md) isn't fully specified yet.
A PRD materializes that specification before design decisions are made.

## Signal Evidence

### Signals Present

- Requirements partially unclear: koto-user skill structure (SKILL.md depth, references
  layout, eval case scope) is not yet specified — exploration surfaced the knowledge domain
  but not the requirements for the skill itself.
- Multiple independent features: three workstreams (koto-author update, koto-user, root
  AGENTS.md) don't obviously share a single scope boundary.
- User stories and acceptance criteria missing: no "when a koto-user agent does X, the
  skill should guide it to Y" criteria exist in any durable artifact.

### Anti-Signals Checked

- Requirements provided as input: partially true — the user said "we need a koto-user
  skill" but the skill requirements themselves weren't specified. Anti-signal applies only
  to the high-level goal, not the skill requirements.

## Alternatives Considered

- **Design Doc**: Would be appropriate if requirements were fully specified. The user's
  decision recognizes that requirements consolidation comes first.
- **Plan**: Ruled out — technical approach for koto-user is still open.
- **No Artifact**: Ruled out — architectural decisions from exploration need permanent record.

## Deferred Types (if applicable)

None.
