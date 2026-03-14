# Explore Scope: migrate-koto-go-to-rust

## Core Question

What does a complete Rust rewrite of koto look like? koto is a pre-release Go CLI
for orchestrating AI agent workflows via a state machine. The Go codebase will be
deleted and replaced with a functioning Rust CLI that preserves the external command
surface (koto init, koto next, koto status, koto rewind, koto template compile).
No migration tooling or incremental cutover — the result is a working Rust binary.

## Context

koto is ~2,100 lines of Go across engine, controller, template, cache, and cmd/koto
packages. The event-sourced refactor (DESIGN-unified-koto-next.md) is a near-complete
rewrite of core logic anyway; switching languages at the same time costs little extra.
The external CLI contract must be preserved; agents must not notice the language
change. Pre-release — no existing users, no backward compat concerns.

Upstream: docs/designs/DESIGN-unified-koto-next.md (Planned)
Source issue: tsukumogami/koto#45

## In Scope

- Rust workspace layout and crate structure
- Dependency selection (CLI, serialization, async/sync, error handling, testing)
- CI pipeline replacement (Go toolchain out, Rust toolchain in)
- Testing approach for the Rust CLI
- Audit of current Go implementation (what behavior must be preserved)

## Out of Scope

- Cutover strategy / incremental porting
- Migration tooling
- The event-sourced architecture changes (tracked in #46–#49)
- Feature changes of any kind

## Research Leads

1. **What does the current Go implementation actually do, and what must the Rust CLI preserve?**
   Map the existing packages, command surface, state file format, and template parsing
   behavior. This defines the functional spec for the Rust rewrite.

2. **What Rust workspace structure and crate layout fits koto's package boundaries?**
   The Go code has engine, controller, template, cache, and cmd/koto packages. How
   should these map to Rust crates? Single crate vs. workspace with multiple crates?
   What are the trade-offs given the planned architectural changes?

3. **Which Rust dependencies are the right choices for koto's needs?**
   Evaluate: clap v4 (derive macro) for CLI, serde/serde_json for serialization,
   sync vs. tokio async I/O, error handling (thiserror vs. anyhow), testing utilities.
   What patterns does pup (datadog-labs/pup) use that are relevant?

4. **What does the CI pipeline look like after the Go toolchain is removed?**
   Map current Go CI steps (go test, golangci-lint, govulncheck, gofmt, go vet) to
   Rust equivalents. What's the right linting setup for a Rust CLI project?
