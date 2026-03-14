# Exploration Findings: migrate-koto-go-to-rust

## Core Question

What does a complete Rust rewrite of koto look like? koto is a pre-release Go CLI
for orchestrating AI agent workflows via a state machine. The Go codebase will be
deleted and replaced with a functioning Rust CLI that preserves the external command
surface. No incremental migration — big bang replacement.

## Round 1

### Key Insights

- **Scope is larger than the issue body implies**: Nine Go commands exist
  (`version`, `init`, `transition`, `next`, `query`, `status`, `rewind`, `cancel`,
  `workflows`). The issue only mentions five. `cancel` and `workflows` aren't in
  the issue's "what stays the same" list but exist in the codebase.
  *(Source: go-implementation-audit)*

- **Single crate is correct**: pup (datadog-labs/pup), a comparable Rust AI agent
  CLI with 200+ commands, uses a single crate. koto at ~2,100 lines with 9 commands
  is well within single-crate territory. *(Source: rust-workspace-structure)*

- **Dependency surface is unusually small**: One external Go dependency
  (`gopkg.in/yaml.v3`). Rust equivalent: ~7 crates, no async runtime. `wait-timeout`
  is needed because `std::process::Command` has no built-in process timeout — a
  non-obvious gap. *(Source: rust-dependencies)*

- **CI replacement is mostly mechanical; release pipeline is the real work**:
  `validate.yml` maps 1:1 (3 jobs → 3 jobs). Plugin validation only needs the build
  step swapped. GoReleaser → cargo-dist is the largest change.
  *(Source: ci-pipeline)*

- **`koto transition` must not be ported**: The event-sourced refactor removes it.
  Porting it just to "preserve the Go surface" would mean implementing something
  immediately thrown away. *(Source: go-implementation-audit + user direction)*

### Tensions

- **Full Go port vs. forward-looking skeleton**: A complete Go-to-Rust port replicates
  code (evidence, version conflict detection, legacy YAML parsing) that the
  event-sourced refactor immediately replaces. A skeleton that implements only what's
  certain to survive is simpler and less wasteful.

- **`serde_yml` vs `serde_yaml`**: `serde_yaml` is the most-installed crate but
  unmaintained. `serde_yml` is the maintained fork with lower adoption. Moot for #45
  since legacy YAML parsing is excluded entirely.

### Gaps

- Testing strategy: `assert_cmd` identified but current Go integration test coverage
  not mapped to Rust equivalents.
- cargo-dist output format not verified against what tsuku recipes expect.

### User Focus

- Big bang replacement; no incremental migration strategy needed (pre-release).
- CLI skeleton proves architecture, not full workflow functionality.
- State: simple JSONL (one event per line, current state = last event's `state` field).
  No evidence, no variables, no version conflict detection.
- Removed from #45: `koto transition`, full state file schema, legacy template parsing,
  evidence accumulation, gate evaluation, command gates, variable interpolation.
- Between #45 and #48: CLI cannot advance state. This is intentional — the skeleton
  proves the architecture; full functionality lands with #48 (unified `koto next`).

## Accumulated Understanding

The Rust rewrite is not a feature-for-feature Go port. It's a foundation built
deliberately toward the event-sourced architecture. The six Go commands not in scope
for #45 (`transition`, `status`, `query`, `cancel`, plus the two implicit ones) are
either going away permanently (`transition`), made redundant by the new `koto next`
unified behavior (`status`, `query`), or simple enough to add later (`cancel`).

The CLI in #45 can: initialize a workflow (`init`), read the current state's directive
(`next`), undo the last event (`rewind`), list active workflows (`workflows`), and
compile templates for CI (`template compile`). It cannot advance state — that requires
the unified `koto next` implementation in #48.

Each subsequent issue (#46–#49) is design + implementation: write the design doc, get
it accepted, then implement it in the same issue. This ensures the plan completes to
a fully functional Rust CLI, not just a set of accepted design documents.

## Decision: Crystallize

**Outcome**: No new design doc needed. The exploration findings are actionable as
direct issue updates. The existing DESIGN-unified-koto-next.md already captures the
architecture. Required actions:
1. Update #45 issue body with the skeleton scope
2. Update #46–#49 to add implementation scope alongside their design doc deliverables
3. Update PLAN-unified-koto-next.md if issue counts or descriptions change
