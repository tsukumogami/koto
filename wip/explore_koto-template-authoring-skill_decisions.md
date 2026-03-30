# Exploration Decisions: koto-template-authoring-skill

## Round 1
- Validation via `koto template compile`, not eval agents: the compiler is deterministic and catches structural errors; eval infrastructure is high-complexity for uncertain benefit in v1
- Distribution via koto's existing marketplace as a new plugin: infrastructure exists, standalone SKILL.md lacks versioning and discoverability
- Layered teaching (linear -> evidence routing -> advanced): evidence routing mutual exclusivity is the key non-obvious constraint, building up to it reduces authoring errors
- Proceed despite absent external demand: maintainer intent is the demand signal, `koto generate` deferral validates the space was considered and not rejected
