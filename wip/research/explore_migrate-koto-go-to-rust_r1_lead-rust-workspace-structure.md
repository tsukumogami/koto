# Lead: Rust workspace structure

## Findings

### Go package dependency graph

```
cmd/koto  →  engine, controller, template, cache, discover
controller →  engine, template
engine     →  (no internal deps)
template   →  (no internal deps)
cache      →  template
discover   →  (no internal deps)
internal/buildinfo → (no internal deps)
```

Five logical layers: CLI entry point → controller → engine → template/cache/discover. Clean acyclic graph. The only external dependency is `gopkg.in/yaml.v3`.

### Single crate vs. workspace

**Recommendation: single crate with internal modules.**

Reasons:
- koto is ~2,100 lines with one external dependency — workspace overhead isn't justified
- pup (datadog-labs/pup, a comparable Rust CLI for AI agents with 200+ commands) uses a single crate
- Go's `internal/` convention has no Rust equivalent; privacy is controlled by `pub` visibility at the module level
- The event-sourced refactor changes internals, not package boundaries — no new deployment seams emerge that would justify separate crates now

Workspace becomes worth revisiting after the event-sourced refactor **only if**: the event store backend becomes pluggable (separate crate for trait + impls), or koto-engine is wanted as a standalone library. Neither is likely in the near term.

### Proposed module layout

```
Cargo.toml
src/
  main.rs              # CLI entry, thin — creates app and calls run()
  lib.rs               # pub re-exports; wires modules together
  cli/
    mod.rs             # clap structs (App, Subcommand enums)
    commands/
      init.rs
      next.rs
      transition.rs
      status.rs
      query.rs
      rewind.rs
      cancel.rs
      workflows.rs
      template.rs
  engine/
    mod.rs             # Engine struct, transition logic, gate evaluation
    types.rs           # MachineState, History, Evidence, etc.
    errors.rs          # TransitionError and variants
    persistence.rs     # Atomic write / version conflict detection
  template/
    mod.rs             # Template loading from compiled JSON
    compile.rs         # Source YAML → CompiledTemplate
    types.rs           # CompiledTemplate, State, Gate structs
  cache.rs             # Compilation cache (hash → cached JSON file)
  discover.rs          # Glob koto-*.state.json
  buildinfo.rs         # Version metadata (built with env!() macros)
```

This maps 1:1 to the Go package structure. Rust module naming is `snake_case`; the CLI commands mirror the Go subcommand names exactly.

### Accommodating the event-sourced refactor

The refactor replaces `engine/persistence.rs` (atomic JSON write) with `engine/eventlog.rs` (JSONL append). Everything above `engine/` is unaffected — the controller, CLI commands, and template layer don't care how state is persisted. The module boundary between engine and the rest of the codebase is already the right seam.

## Implications

Start with a single crate using the module layout above. The boundary between engine and the rest of the code is clean enough that the event-sourced refactor is a contained engine-layer change. No workspace needed now; revisit after refactor stabilizes if an external library crate makes sense.

## Surprises

pup uses a single crate despite 200+ commands — confirms single-crate approach scales for CLI tools. Rust's lack of `internal/` means privacy is enforced by `pub` visibility rules, not directory structure; this is simpler and sufficient.

## Open Questions

Will the event-sourced refactor introduce a pluggable storage backend? If yes, a `koto-store` library crate becomes valuable. If no (JSONL file is the only backend), single crate remains the right choice indefinitely.

## Summary

A single Rust crate with internal modules is the right structure for koto now — pup (a comparable Rust agent CLI) confirms this approach scales, and the Go package boundaries map cleanly to Rust submodules without needing workspace overhead. The event-sourced refactor touches only the engine layer, so the module structure chosen here doesn't need to change. The workspace question is worth revisiting only if a pluggable storage backend or external library crate emerges post-refactor.
