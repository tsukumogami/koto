---
status: Proposed
spawned_from:
  issue: 48
  repo: tsukumogami/koto
  parent_design: docs/designs/DESIGN-unified-koto-next.md
problem: |
  koto's current `koto next` is a read-only stub that returns the current state's
  directive and transition targets. It can't submit evidence, evaluate gates, advance
  state, or perform directed transitions. Agents have no way to drive workflow forward
  through the CLI -- the command that was supposed to replace `koto transition` doesn't
  yet do anything beyond reading state.
decision: |
  (To be determined after approach investigation.)
rationale: |
  (To be determined after approach investigation.)
---

# DESIGN: koto CLI Output Contract

## Status

Proposed

## Upstream Design Reference

Parent: `docs/designs/DESIGN-unified-koto-next.md` (Phase 3: CLI Output Contract)

This tactical design implements the CLI output contract specified in the strategic
design's Phase 3. The strategic design defines the event-sourced state machine
architecture; this design specifies the exact JSON output schema, flag behavior,
gate evaluation, and advancement mechanics for `koto next`.

## Context and Problem Statement

`koto next` is the sole interface between agents and the workflow engine. After #45
(Rust CLI foundation), #46 (event log format), and #47 (template evidence routing),
the infrastructure exists to support evidence submission, conditional transitions,
and gate evaluation. But `koto next` itself remains a stub -- it reads state and
returns a directive, nothing more.

The problem has three parts:

1. **No evidence submission.** Templates can declare `accepts` blocks and `when`
   conditions, but there's no CLI mechanism to submit data. The `--with-data` flag
   doesn't exist yet.

2. **No state advancement.** `koto transition` was removed in #45. Its replacement
   (`koto next --to` for directed transitions, auto-advancement for conditional
   transitions) hasn't been implemented. Agents can read state but can't change it.

3. **No gate evaluation.** Command gates are declared in templates but never
   executed. States with gates always appear passable.

The output format also needs to become self-describing: agents should know from a
single `koto next` response what they can do next (submit evidence, wait for gates,
handle integration output) without external knowledge of the template structure.

## Decision Drivers

- **Agent autonomy**: agents must be able to drive workflows using only `koto next`
  output, without reading templates or state files directly
- **Correctness**: evidence validation, gate evaluation, and state transitions must
  be atomic and consistent with the event log model
- **Self-describing output**: the JSON response must tell the agent exactly what to
  do next, including what evidence fields to submit and what options are available
- **Error clarity**: structured errors with codes, not just messages, so agents can
  branch on failure type programmatically
- **Scope boundary**: auto-advancement loop, integration runner, `koto cancel`, and
  signal handling are deferred to #49 -- this design covers gate evaluation and
  single-step advancement only
