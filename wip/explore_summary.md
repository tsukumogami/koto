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

## Decision (Phase 5)

**Problem:**
koto's template compiler and evidence system are fully implemented as Go libraries
but unreachable from the command line. Users can't compile templates, find templates
by name, or supply evidence during transitions. The CLI's init command still uses
the legacy parser that can't handle gates or nested YAML. This gap blocks all
real-world usage of the template format.

**Decision:**
Implicit compilation at init time with an explicit compile command for debugging.
Templates are found via a three-level search path (explicit path, project-local
./templates/, user-global ~/.koto/templates/). The transition command gets --evidence
flags. A template inspect command shows compiled output for debugging. LLM-assisted
validation is deferred entirely to a future design.

**Rationale:**
Implicit compilation keeps the simple case simple (koto init --template quick-task
just works) while the explicit compile command serves debugging and CI. The search
path follows the same convention as PATH, Go modules, and npm: local overrides
global. Deferring LLM validation avoids designing an integration surface we don't
need yet. The evidence flag is the minimal change that unblocks gate-based workflows.

## Current Status
**Phase:** 5 - Decision
**Last Updated:** 2026-02-22
