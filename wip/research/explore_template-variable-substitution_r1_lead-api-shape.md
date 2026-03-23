# Lead: Where should substitution live and what API shape works?

## Findings

### Module hierarchy
```
src/lib.rs exports: cache, cli, discover, engine, gate, template
src/cli/mod.rs imports: gate, engine/*, template/types
src/gate.rs: evaluates commands; imports Gate from template::types
src/engine/advance.rs: calls closure-injected evaluate_gates
src/engine/persistence.rs: reads variables from WorkflowInitialized event
src/engine/types.rs: stores variables in EventPayload::WorkflowInitialized
```

### Call sites for substitution
1. **Gate evaluation** (`src/gate.rs:69`): `cmd.arg("-c").arg(&gate.command)` — needs
   substituted command string before shell execution
2. **Directive retrieval** (`src/cli/next.rs:63`): `template_state.directive.clone()` —
   needs substituted directive text before returning to user
3. **Future action execution** (#71): similar to gates, substitutes in action commands

### Variable flow through the system
- Stored in `EventPayload::WorkflowInitialized { variables }` at init time
- Retrieved via event log replay in `persistence.rs` — but no `derive_variables`
  function exists yet
- Needed at runtime in `handle_next` before gates and directives are processed

### Gate closure pattern
`advance_until_stop` in `src/engine/advance.rs` accepts a closure for gate evaluation.
Substitution can wrap the real `evaluate_gates` without modifying `gate.rs` itself:
```rust
let gate_closure = |gates| {
    let substituted = substitute_gates(gates, &variables);
    evaluate_gates(&substituted, &working_dir)
};
```

### API options evaluated

**Option 1: Standalone function**
```rust
pub fn substitute(template: &str, vars: &HashMap<String, String>) -> Result<String>
```
- No coupling, reusable, easy to test
- Caller must extract variables from events each time

**Option 2: Variables newtype with method**
```rust
pub struct Variables(HashMap<String, String>);
impl Variables {
    pub fn from_events(events: &[Event]) -> Self { ... }
    pub fn substitute(&self, template: &str) -> Result<String> { ... }
}
```
- Encapsulates extraction + substitution
- Constructed once in handle_next, passed to closures
- Clean call site: `vars.substitute(&gate.command)`

**Option 3: Trait** — overengineered. Strings don't need polymorphism.

### Module placement
`src/engine/substitute.rs` is the right location:
- Variables are engine concepts (stored in events, part of state machine)
- Gate evaluation is unix-only; substitution should be cross-platform
- Future action execution (#71) also lives in engine/advance
- Persistence already manages event-derived state

## Implications

Option 2 (Variables newtype) with placement in `src/engine/substitute.rs` gives the
cleanest API. The `from_events` constructor encapsulates variable extraction, and the
`substitute` method is reusable across gates, directives, and future actions.

The gate closure pattern already exists in advance.rs, so substitution integrates
without modifying gate.rs itself.

## Surprises

1. No `derive_variables` function exists in persistence.rs despite the event carrying
   variables — all other event-derived state has extraction helpers.
2. The gate closure injection pattern in advance.rs makes integration clean — no need
   to thread variables through the gate module.

## Open Questions

1. Should `Variables::from_events` live on the struct or be a standalone function in
   persistence.rs? Keeping it on the struct is more cohesive.
2. Does the substitution function need to know about `VariableDecl` (for detecting
   references to undeclared variables), or just the resolved values map?

## Summary

A `Variables` newtype in `src/engine/substitute.rs` with `from_events` constructor and
`substitute` method is the recommended API shape. It encapsulates variable extraction
from the event log and provides a reusable substitution interface for gates, directives,
and future action execution (#71). The existing gate closure pattern in advance.rs
enables clean integration without modifying gate.rs.
