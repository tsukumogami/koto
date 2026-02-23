# Exploration Summary: koto CLI and Template Tooling

## Problem (Phase 1)
koto has a working compiler and engine but no way to invoke them from the command line. Users can't compile templates, discover available templates, or supply evidence during transitions without writing Go code.

## Decision Drivers (Phase 1)
- Implicit compilation at init time (users shouldn't need a separate compile step for basic usage)
- Template search paths for discoverability (project-local, user-global, explicit)
- Evidence CLI flag for transitions (the engine supports it, the CLI doesn't expose it)
- No LLMs in compile path (deferred to future linter design)
- Progressive complexity (simple usage = simple commands)

## Research Findings (Phase 2)
- Current CLI has init/transition/next/query/status/rewind/cancel/validate/workflows commands
- init --template takes a path but doesn't compile or search
- Transition command has no --evidence flag
- Two parser tracks: simple Parse() and formal Compile(). Design should deprecate Parse() in favor of Compile().
- Template discovery only finds state files, not templates
- Compiler produces deterministic JSON with SHA-256 hash

## Options (Phase 3)
- Decision 1 (Compile flow): Implicit at init vs explicit command vs both
- Decision 2 (Search paths): Fixed hierarchy vs configurable vs explicit-only
- Decision 3 (LLM validation): Defer entirely vs design stub vs full linter
- Decision 4 (Template commands): Flat vs subcommand group

## Distribution Paths (Phase 2 update)
- Path 1 (deployed): Template bundled with skill/plugin, explicit path, expected valid, cache compilation
- Path 2 (authored): User writes template manually, needs feedback loop, search path for convenience

## Decision (Phase 5, revised Phase 8)

**Problem:**
koto's template compiler and evidence system are fully implemented as Go libraries
but unreachable from the command line. Templates reach users through two distinct
paths -- deployed by skills/plugins (Path 1) or authored manually (Path 2) -- but
neither path has CLI support.

**Decision:**
koto init compiles templates implicitly and caches the result for deployed templates
(Path 1). Template authors (Path 2) get a feedback loop via koto template compile,
inspect, and list commands. A search path (project-local, user-global) serves
name-based resolution for authored templates. koto transition gets --evidence flags.

**Rationale:**
Separating the two distribution paths avoids forcing one path's UX on the other.
Deployed templates want silent, cached compilation. Authored templates want feedback.
Both paths use the same compiler; the difference is what koto does around the
compilation. The evidence flag is the minimum change that unblocks gate-based workflows.

## Current Status
**Phase:** 8 - Final Review
**Last Updated:** 2026-02-22
