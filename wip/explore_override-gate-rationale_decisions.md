# Exploration Decisions: override-gate-rationale

## Round 1
- Override should be a first-class event, not implicit: all research and external patterns converge on this; implicit override via evidence presence is an outlier among workflow engines
- Existing decisions subsystem should not be overloaded for override capture: agent-initiated deliberation and engine-detected gate bypass are distinct concepts with different query needs
- Rationale should be mandatory on override: consistent with decisions subsystem requiring rationale, and aligned with audit/compliance patterns
- Visualization/redo use case scoped out of current work: persist the data now, build on it later
- Conditional validation (required_when) scoped out as general template feature: handle override rationale at the engine level, not as schema evolution
