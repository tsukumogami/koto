# Exploration Decisions: content-ownership

## Round 1
- MVP scope is replace-only operations (add/get/exists): read-modify-replace by agents covers accumulation patterns since orchestrators serialize writes. Append and field-update operations are future optimizations.
- Context submission decoupled from state advancement: agents submit context independently without calling `koto next`. Multiple agents can build up research before orchestrator advances state.
- Shared session model for MVP: one koto session spans the full skill pipeline (explore→design→plan→implement). Session chaining/inheritance is future work.
- "Context" not "evidence": the CLI subcommand should reflect that these are cumulative workflow context, not just evidence for gates.
- Demand is maintainer-driven (pre-release project): no external user validation, but the architectural insight is well-grounded in the session storage implementation experience.
