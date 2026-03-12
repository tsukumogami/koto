# Crystallize Decision: unified-koto-next

## Chosen Type

PRD

## Rationale

The exploration answered "what" clearly enough to write requirements, but the user chose PRD
over Design Doc because wip/ artifacts are cleaned before the PR merges. The use cases and
requirements discovered during exploration need to be captured in a permanent artifact before
that happens. A PRD captures what the unified `koto next` must do and why, giving a design
doc a stable foundation to build on afterward.

## Signal Evidence

### Signals Present

- **Single coherent feature emerged**: a unified `koto next` command as the sole state-evolution
  interface, with koto-owned integrations and evidence-based branching
- **Requirements known but undocumented**: use cases (delegation, evidence submission, approval
  gates, branching workflows) are clear from exploration but live only in wip/ artifacts
- **Preservation need**: wip/ is cleaned before merge; requirements need a permanent home

### Anti-Signals Checked

- **Requirements already clear and agreed on**: present, but the user explicitly chose PRD to
  document them before they're lost — the permanence need outweighs this anti-signal

## Alternatives Considered

- **Design Doc**: the natural next step after PRD; will follow once requirements are locked
- **Plan**: no upstream artifact to decompose yet
- **No artifact**: requirements would be lost when wip/ is cleaned

## Deferred Types

None applicable.
