---
status: Planned
upstream: docs/designs/DESIGN-unified-koto-next.md
problem: |
  koto is ~2,100 lines of Go across engine, controller, template, cache, and CLI
  packages. The planned event-sourced refactor will rewrite nearly all core logic.
  Migrating to Rust before that refactor begins avoids implementing the architectural
  changes twice — once in Go, once in Rust.
decision: |
  Replace the Go codebase with a single-crate Rust binary delivering a forward-looking
  skeleton: five commands (version, init, next, rewind, workflows) plus template
  compile, backed by a simple JSONL state file. Features that the event-sourced
  refactor (#46–#49) will replace entirely are intentionally excluded.
rationale: |
  Big bang replacement is safe pre-release (no users, no backward compatibility
  constraints). A skeleton scope avoids implementing evidence accumulation, version
  conflict detection, and the full JSON state schema that #46–#49 replace with
  fundamentally different mechanisms. The simple JSONL format is structurally
  forward-compatible with the full event log designed in #46.
---

# DESIGN: Migrate koto from Go to Rust

## Status

Planned

## Upstream Design Reference

[DESIGN-unified-koto-next.md](DESIGN-unified-koto-next.md) — the event-sourced
architecture that follows this migration. The required tactical sub-designs
(#46–#49) all target the Rust implementation delivered here.

## Context and Problem Statement

koto is a Go CLI (~2,100 lines) for orchestrating AI agent workflows via a state
machine. The accepted design `DESIGN-unified-koto-next.md` introduces a
near-complete rewrite of core logic: a JSONL event log replacing the mutable JSON
state file, a new template format, and a unified `koto next` command replacing the
current `koto next` + `koto transition` pair.

Migrating the language and implementing the architectural changes are effectively
the same cost. Staying in Go means implementing the event-sourced changes in Go
and then rewriting in Rust — two rewrites instead of one. Switching at the same
time costs little extra because most of the Go code won't survive the architectural
change regardless.

## Decision Drivers

- Pre-release: no existing users, no backward compatibility constraints
- Event-sourced refactor (#46–#49) will replace the persistence layer, template
  format, and core CLI behavior immediately after this migration
- External CLI contract (command names, JSON output) must be preserved; agents
  must not notice the language change
- Skeleton scope reduces risk: fewer things to get right before #46–#49 take over

## Considered Options

### Migration strategy: big bang vs. incremental port

**Big bang (chosen):** Delete the Go source, replace with a Rust binary in a
single PR. Safe because koto is pre-release with no users. The external CLI
contract is the only constraint — agents only care about command names and JSON
output shape.

**Incremental port:** Maintain both Go and Rust implementations during migration,
gradually shifting traffic. Adds integration complexity (two binaries, feature
parity verification) with no user benefit at pre-release stage.

### Crate structure: single crate vs. workspace

**Single crate (chosen):** Internal modules (`engine/`, `template/`, `cli/`,
`cache.rs`, `discover.rs`) replicate Go's package structure without workspace
overhead. pup (datadog-labs/pup), a Rust AI agent CLI with 200+ commands, uses
a single crate — confirming this approach scales for CLI tools of this type.

**Workspace:** Worth revisiting only if the event-sourced refactor introduces a
pluggable storage backend or koto-engine becomes a standalone library. Neither is
likely in the near term.

### CLI scope: full Go port vs. forward-looking skeleton

**Forward-looking skeleton (chosen):** Implement only what's certain to survive
the event-sourced refactor: `version`, `init`, `next`, `rewind`, `workflows`, and
`template compile`. Exclude `koto transition` (being removed), the full JSON state
schema (being replaced), evidence accumulation, gate evaluation, and command gates
(all replaced by #46–#49).

**Full Go port:** Preserves all nine existing commands and their behaviors exactly.
Requires implementing evidence, version conflict detection, legacy YAML template
parsing, and command gates that #46–#49 will immediately replace. Higher
implementation cost with no long-term value.

### State format: simple JSONL vs. full event schema from the start

**Simple JSONL (chosen):** One event per line, current state = last event's `state`
field. File named `koto-<name>.state.jsonl`. Minimal but structurally
forward-compatible — the full event types designed in #46 are additions to this
foundation, not a replacement of a conflicting format.

```jsonl
{"type":"init","state":"gather","timestamp":"...","template":"path/to/template.json","template_hash":"abc123"}
{"type":"rewind","state":"gather","timestamp":"..."}
```

**Full event schema from the start:** Implement all six event types, `seq` counter,
epoch boundary, and evidence replay in this issue. Tightly couples the Rust
migration with the event log design (#46), which hasn't been written yet. Design
decisions made here may conflict with what #46 specifies.

### Async runtime: sync vs. tokio

**Sync only (chosen):** koto has no networking, no concurrency, and no long-running
background tasks. File I/O and process execution are sequential within a single
command invocation. `std::fs` and `std::process::Command` cover all needs.

**tokio:** Adds compile-time and complexity overhead with no benefit. pup uses
tokio because it makes HTTP requests — koto does not.

### YAML dependency: serde_yml vs. serde_yaml

**serde_yml (chosen):** Actively maintained fork of `serde_yaml`. Lower adoption
but safer long-term. Used for `koto template compile` (YAML source → compiled JSON).

**serde_yaml:** Most-installed crate in its category but unmaintained. Works today
but accumulates risk over time.

## Decision Outcome

Replace the Go codebase with a single-crate Rust binary using a forward-looking
skeleton. The skeleton delivers the minimum command surface needed to prove the
architecture and keep plugin CI working, while excluding all features that #46–#49
will replace with fundamentally different implementations.

`koto transition` is removed permanently. It will not be ported and will not
return. Its replacement (`koto next --to`) is implemented in #48.

Between this issue and #48, the CLI cannot advance workflow state. This is
intentional: `koto init` establishes the workflow, `koto next` shows the current
directive, and `koto rewind` undoes the last event. Full workflow advancement lands
with #48.

## Solution Architecture

### Crate layout

```
Cargo.toml
src/
  main.rs              # thin entry point, calls app::run()
  lib.rs               # re-exports, wires modules
  cli/
    mod.rs             # clap App and Subcommand enums
    commands/
      version.rs
      init.rs
      next.rs
      rewind.rs
      workflows.rs
      template.rs      # compile + validate subcommands
  engine/
    mod.rs             # state derivation from event log
    types.rs           # MachineState, Event structs
    errors.rs          # typed engine errors (thiserror)
    persistence.rs     # JSONL read/write
  template/
    mod.rs             # compiled template loading
    compile.rs         # YAML source → FormatVersion=1 JSON
    types.rs           # CompiledTemplate, State, Gate structs
  cache.rs             # SHA256(compiled JSON) → cached file
  discover.rs          # glob koto-*.state.jsonl
  buildinfo.rs         # version metadata via env!() macros
```

### Dependencies

```toml
[dependencies]
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yml = "0.7"
thiserror = "1"
anyhow = "1"
tempfile = "3"

[target.'cfg(unix)'.dependencies]
wait-timeout = "0.2"   # std::process::Command has no built-in timeout; needed for #48

[dev-dependencies]
assert_cmd = "2"
assert_fs = "1"
tempfile = "3"
```

`thiserror` is used for typed engine errors (`TransitionError`, `PersistenceError`).
`anyhow` is used at the CLI layer for error collection and JSON output.
`wait-timeout` is included now for command gate execution in `koto next` (#48);
it is Unix-only and correctly declared as a target-specific dependency.

### Commands implemented

| Command | Behavior |
|---|---|
| `koto version` | Print build version from `buildinfo.rs` (env! macros for tag/commit/date) |
| `koto init <name> --template <path>` | Compile or load template; write `koto-<name>.state.jsonl` with `init` event |
| `koto next <name>` | Read last event's `state` from JSONL; load template; return JSON with `state`, `directive`, `transitions` |
| `koto rewind <name>` | Append `rewind` event pointing to previous state; error if at initial state |
| `koto workflows` | Glob `koto-*.state.jsonl` in current directory; print names as JSON array |
| `koto template compile <source>` | Compile YAML template source to FormatVersion=1 JSON |
| `koto template validate <path>` | Validate compiled template JSON against schema |

### What is not implemented

| Feature | Status | Covered by |
|---|---|---|
| State advancement | `koto transition` removed permanently | `koto next --to` in #48 |
| Evidence accumulation | Excluded | #46 |
| Gate evaluation | Excluded | #48 |
| Variable interpolation (`{{KEY}}`) | Removed permanently | — |
| Version conflict detection | Excluded | #46 (append-only log) |
| SchemaVersion 1/2 state file | Excluded (pre-release) | — |
| Legacy YAML template parsing | Removed permanently | — |
| `koto status`, `koto query`, `koto cancel` | Excluded | #48 or later |
| Command gates (shell execution) | Excluded | #48 |

## Implementation Approach

**Phase 1:** Crate scaffold — `Cargo.toml`, module layout, CI workflows
(`validate.yml` with cargo test/fmt/clippy/audit; `release.yml` with cargo-dist
replacing GoReleaser). Delete Go source (`go.mod`, `go.sum`, `cmd/`, `pkg/`,
`internal/`).

**Phase 2:** Template layer — `template/compile.rs` (YAML → FormatVersion=1 JSON)
and `template/types.rs`. This unblocks plugin CI (`validate-plugins.yml` runs
`koto template compile`).

**Phase 3:** Engine + persistence — `engine/types.rs` (Event, MachineState),
`engine/persistence.rs` (JSONL append/read), `engine/mod.rs` (current state
derivation from last event).

**Phase 4:** CLI commands — implement all seven commands using the engine and
template layers. Wire up clap structs in `cli/mod.rs`.

**Phase 5:** Tests — `assert_cmd` integration tests for each command; unit tests
for state derivation and template compilation.

## Security Considerations

**Template path handling:** `koto template compile` and `koto init --template`
accept file paths. Paths are resolved relative to the working directory. No
user-controlled path traversal beyond normal filesystem permissions — the same
user running koto controls the templates. No network access involved.

**State file write:** JSONL append uses standard file operations. No atomic
rename required for append-only semantics (appended lines are either written
or not; a partial final line is detectable and ignorable). No elevated
permissions required.

**Command gate execution:** Not implemented in this issue. Command gates
(shell execution with timeout) are deferred to #48, where process group
isolation and timeout handling will be specified.

**Compiled template JSON:** Template output is written to a user-controlled
cache directory (`~/.cache/koto/` or equivalent). No execution of compiled
template content — it is read-only structured data.

**Supply chain:** Seven production dependencies, all well-established in the
Rust ecosystem. `cargo audit` in CI checks against the RustSec advisory
database on every PR.

## Consequences

**Positive:**
- Go toolchain removed from the repository; Rust toolchain is the single build
  dependency
- Plugin CI continues to work unchanged (only the build step changes)
- Simple JSONL state is structurally forward-compatible with the full event
  schema in #46 — no migration tooling needed
- `koto transition` removed cleanly; its absence is documented rather than
  hidden behind a deprecation flag

**Negative:**
- Between this issue and #48, the CLI cannot advance workflow state. Workflows
  can be initialized and inspected but not progressed.
- `koto status`, `koto query`, and `koto cancel` are not available until #48
  or later.

**Accepted trade-offs:**
- Skeleton scope means the CLI is not fully functional until #48. This is
  intentional and documented in PLAN-unified-koto-next.md.
- FormatVersion=1 compiled templates will be replaced in #47. The template
  compiler implemented here has a limited lifespan.
