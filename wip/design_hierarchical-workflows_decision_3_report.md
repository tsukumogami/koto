<!-- decision:start id="cross-hierarchy-query-surface" status="assumed" -->
### Decision: Minimum CLI Surface for Cross-Hierarchy Queries

**Context**

koto's current CLI has no read-only state inspection command. The only way to learn a workflow's current state is `koto next`, which triggers gate evaluation, action execution, and state advancement -- unacceptable for a parent agent observing child progress. Meanwhile, `koto context get <session> <key>` already supports cross-session reads, and `koto workflows` (extended with parent/children metadata from Decision 2) handles child discovery.

The research identified three parent query patterns: (1) discover which children exist and their lifecycle state, (2) read results that children stored, and (3) check what state a child is currently in. Patterns 1 and 2 are covered by existing commands. Pattern 3 has no solution today -- the `derive_machine_state()` function exists in persistence.rs but nothing exposes it through the CLI.

**Assumptions**

- Decision 2 will add parent/children fields to WorkflowMetadata, enabling `koto workflows` to report child relationships. If this assumption is wrong, child discovery falls back to naming conventions alone.
- Children store final results in context keys before reaching terminal state. This is a convention adopted by the design, not enforced by koto. Templates should document expected context keys in their completion states.

**Chosen: koto status <name> + existing koto context get**

Add a single new command `koto status <name>` that returns read-only state metadata by replaying the event log. Output shape:

```json
{
  "name": "design.research-agent",
  "current_state": "synthesize",
  "template_path": ".koto/research.template.json",
  "template_hash": "a1b2c3...",
  "is_terminal": false
}
```

The implementation calls `derive_machine_state()` (already in persistence.rs) and adds a terminal check against the compiled template. No gates are evaluated, no actions run, no state changes occur.

The full cross-hierarchy query surface becomes three commands:
- `koto workflows` -- discover children (with Decision 2's parent/children metadata)
- `koto status <child>` -- check child state without side effects
- `koto context get <child> <key>` -- read child results

No `koto query` command is added. The name documented in CLAUDE.md should be corrected to match what actually ships.

**Rationale**

The context-only alternative (no new commands) technically works but relies on children writing status keys voluntarily. If a child crashes or is cancelled, its context store may not reflect its actual state. `koto status` derives state from the event log, which is always accurate and requires no convention compliance.

A full `koto query` command that dumps evidence, decisions, and visit counts exposes data scoped to the current state epoch -- meaningless to a parent reading cross-workflow. The extra output wastes agent context window budget. Starting with the minimal `koto status` keeps the door open: if a future use case needs more, the command can grow with additional flags (e.g., `--include-evidence`).

Extending `koto workflows --status` forces O(N) work to check one child, and conflates discovery with inspection. Separate commands match how agents actually use them: discover once, then poll specific children.

**Alternatives Considered**

- **Context-only (no new commands)**: Relies on conventions for state inspection. If a child crashes before writing its status context key, the parent cannot determine child state. Correct but fragile.
- **koto query <name> (full dump)**: Exposes ephemeral epoch-scoped data (evidence, decisions) that is not useful cross-workflow. Wastes agent context window. Overly broad for the actual need.
- **koto workflows --status**: Forces listing all workflows to check one child's state. O(N) when O(1) is needed. Mixes discovery and inspection concerns.

**Consequences**

Adding `koto status` means one new top-level command, roughly 30 lines of CLI glue. The derive_machine_state() function already handles log replay. The terminal check requires loading the compiled template to check if the current state is marked terminal -- this is the only new logic.

CLAUDE.md's reference to `koto query` should be updated to document `koto status` instead, or removed if the design ships before the docs are corrected.

The parent's query loop becomes: call `koto status <child>` to check if a child has reached its terminal state, then `koto context get <child> result` to read the output. This is two commands per child per poll cycle -- bounded and predictable.
<!-- decision:end -->
