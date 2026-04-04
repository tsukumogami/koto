# Exploration Decisions: hierarchical-workflows

## Round 1
- Gate-based fan-out over action-based or state-level declaration: gate approach requires zero advance loop changes, reuses existing infrastructure (blocking_conditions, gates.* routing, overrides), and can be layered with declarative syntax later if needed
- Header-only lineage over dual-event or directory nesting: minimal code changes, backward-compatible, satisfies primary query patterns; parent-side event deferred until crash-recovery requirements are concrete
- Flat storage with metadata filtering over directory-based isolation: preserves the flat session model both backends depend on; directory nesting would require reworking the entire SessionBackend trait
- Naming convention (parent.child) as ergonomic default alongside metadata (parent header field): convention needs zero code changes for MVP, metadata provides correctness guarantees
- Abandon as default parent close policy: parent agent manages child lifecycle, koto shouldn't force child termination
- External child templates, no implicit state sharing: validated by all major workflow engine prior art; children pass results explicitly at completion
