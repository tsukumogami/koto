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
  Model the five koto next output variants as a NextResponse Rust enum with typed
  fields per variant, and the six error codes as a NextError enum. A pure dispatcher
  function classifies (current state + CLI flags + gate results) into the appropriate
  variant. Custom serde serialization produces the correct JSON shape per variant.
  The orchestration layer in cli/mod.rs stays simple -- load state, evaluate gates,
  call dispatcher, serialize, exit.
rationale: |
  The output contract is the core deliverable of this design. Modeling it as typed
  enums means the code is the contract -- NextResponse variants map 1:1 to JSON
  output shapes, and the compiler enforces exhaustiveness. The codebase already uses
  this pattern (EventPayload, EngineError), so it's a familiar convention, not a new
  abstraction. The monolithic handler alternative works but scatters the output
  contract across ad-hoc serde_json::json! calls where field omission bugs are silent.
  The pipeline approach solves a sequencing problem that doesn't exist -- stages are
  conditional and the auto-advancement loop in #49 breaks the linear model.
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

## Considered Options

### Decision 1: Implementation architecture for koto next

**Context:** How to structure the `koto next` implementation -- the handler logic,
output formatting, error handling, and the relationship between CLI and engine layers.

**Chosen: Response type dispatch.**

The five output variants and six error codes are the core deliverable of this design.
Modeling them as `NextResponse` and `NextError` enums gives compile-time
exhaustiveness checks, prevents field-omission bugs in JSON output, and makes the
output contract testable as a pure function without CLI integration. The codebase
already uses this pattern -- `EventPayload` is a six-variant enum with custom
serialization, `EngineError` uses thiserror. The dispatcher function is a pure
function that takes loaded state and returns a typed result; the CLI handler just
serializes and exits.

*Alternative rejected: Monolithic handler.* A single `run_next()` function matches
the existing handler style and minimizes new abstractions. But it scatters the output
contract across ad-hoc `serde_json::json!` calls where field name typos and missing
keys are silent runtime bugs. As the function grows to 200-300 lines, the five output
variants and their triggering conditions become harder to trace. The approach works
but doesn't leverage Rust's type system for the thing that matters most -- output
correctness.

*Alternative rejected: Pipeline stages.* Breaking processing into stage functions
with a shared `NextContext` struct provides per-stage testability. But the stages are
conditional (skip gates if none, skip evidence if no `--with-data`), which undermines
the clean sequential model. The `NextContext` accumulates `Option<T>` fields where
later stages unwrap what earlier stages guaranteed -- a type-safety regression the
pipeline was supposed to improve. The auto-advancement loop in #49 further breaks
linearity. The approach adds ceremony without the compile-time guarantees that
response dispatch provides.

## Decision Outcome

`koto next` will be implemented using typed response enums with a pure dispatcher
function. The architecture has three layers:

1. **CLI handler** (`src/cli/mod.rs`): Parse flags, load state, call dispatcher,
   serialize response, exit with correct code. Simple and thin.

2. **Dispatcher** (new module): Pure function that takes loaded state, template,
   flags, and gate results. Returns `Result<NextResponse, NextError>`. No I/O.

3. **Response types** (new module): `NextResponse` enum (five variants),
   `NextError` enum (six error codes), `ExpectsSchema`, `BlockingCondition`, and
   supporting types. Custom serde serialization for the `action` field.

Key properties:
- Output contract is self-documenting: read `NextResponse` to know all possible outputs
- Compiler enforces exhaustiveness on every match
- Dispatcher testable without spawning processes or setting up state files
- Gate evaluation and evidence validation are helper functions called before dispatch
- Exit codes derived from response/error variant, not computed separately
