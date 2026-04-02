# Design Summary: gate-backward-compat

## Input context (Phase 0)

**Source:** Freeform topic (GitHub issue #119)
**Upstream:** docs/prds/PRD-gate-transition-contract.md (R10)
**Problem:** Feature 1 left legacy gate behavior implicit and unguarded. New templates
can accidentally use it. The compiler has no way to distinguish intentional legacy mode
from accidental omission. The only known legacy template (shirabe work-on) needs to keep
working until it migrates.
**Constraints:**
- New templates must fail compilation unless they declare legacy mode explicitly
- `koto init` must work and only warn (no error)
- Legacy marker must be self-contained and easily removable per-template
- D4 warnings must be suppressed for declared-legacy templates
- Engine must not inject gate output into resolver evidence for legacy states
- Single existing consumer: shirabe work-on template (pure legacy, no gates.* routing)

## Decisions to make

1. **D1: Legacy mode declaration and compiler enforcement** — where the opt-in
   lives (frontmatter field vs CLI flag), compiler error/warning behavior, D4
   warning suppression, koto init behavior
2. **D2: Evidence injection for legacy states** — whether gate output is excluded
   from resolver evidence for legacy states (matches R10 AC precisely) or remains
   injected but unused (current behavior)

## Current status

**Phase:** 0 - Setup complete
**Last updated:** 2026-04-02
