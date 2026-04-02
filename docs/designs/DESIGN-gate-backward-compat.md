# DESIGN: Gate backward compatibility

## Status

Proposed

## Upstream design reference

This is Feature 4 of the gate-transition contract. Feature 1 design:
[DESIGN-structured-gate-output](current/DESIGN-structured-gate-output.md).
Feature 2 design: [DESIGN-gate-override-mechanism](current/DESIGN-gate-override-mechanism.md).
Feature 3 design: [DESIGN-gate-contract-compiler-validation](current/DESIGN-gate-contract-compiler-validation.md).

## Context and problem statement

Features 1–3 of the gate-transition contract introduced structured gate output,
the override mechanism, and compiler validation. Feature 1 preserved the old
gate behavior implicitly: when no `when` clause in a state references `gates.*`
fields, the advance loop falls back to boolean pass/block behavior — the gate
runs, and if it fails the state blocks; routing happens entirely through agent
evidence submitted via `accepts` blocks. This fallback path was designed in but
never formalized.

The problem is that this implicit path is invisible to the compiler. Template
authors who accidentally write gates without `gates.*` routing get the old
behavior silently — no warning, no error, nothing in the template that signals
their gates aren't feeding into transition logic. The compiler validation added
in Feature 3 warns about unreferenced gate fields, but that warning fires for
intentionally-legacy templates too, which creates noise that discourages
migration.

Two concrete issues remain unresolved after Feature 3:

First, the compiler has no distinction between "intentionally using legacy mode"
and "accidentally omitting `gates.*` references." Both look the same. The PRD
acceptance criterion for R10 says legacy states should produce "no structured
output" — but the advance loop currently injects gate output into the resolver
evidence for all states regardless.

Second, the only known template in this codebase using legacy gate behavior is
the shirabe work-on template, which uses gates as pure pass/fail blockers and
routes entirely on agent evidence. It needs to keep working until it's migrated
to structured routing, but it should carry a visible marker that it's on the
legacy path.

## Decision drivers

- Template authors must not accidentally opt into legacy mode. New templates
  using gates without `gates.*` routing should fail compilation by default.
- The legacy marker must be easy to locate and remove. When a template migrates
  to structured routing, the migration PR should consist of removing one
  frontmatter line and updating the transitions — nothing else.
- `koto init` must continue to initialize any template without error. Init is
  for scaffolding; strict gate validation happens at compile time.
- D4's unreferenced-field warning must not fire for templates that have
  explicitly declared legacy mode. The warning is for structured-mode templates
  where gate output is meant to drive routing but some fields are never checked.
- The engine's behavior for legacy states must match R10 precisely: gate output
  should not enter the resolver's evidence map for legacy states.
- The legacy code path must be self-contained and deletable. When the last
  legacy template migrates, removing the compat code should be a contained change.
