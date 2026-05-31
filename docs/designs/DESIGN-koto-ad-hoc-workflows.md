---
status: Proposed
problem: |
  koto can only run a workflow from a pre-authored, compiled template, so an
  agent facing a novel complex task cannot get koto's ordered, recoverable,
  auditable execution without a human (or a heavyweight durable-authoring step)
  writing the template first. There is no path for an agent to author a
  workflow inline and run it immediately.
decision: |
  Add a single-shot path that accepts a workflow definition on stdin, compiles
  and strictly validates it in-process, and initializes a session in one
  invocation — reusing koto's existing compile pipeline and state machine with
  no new execution semantics. Persist both the compiled artifact (so per-tick
  template-hash verification keeps succeeding) and the human-readable authored
  source (for audit) with the session. Ship a koto-skills skill that teaches
  decomposition and the run path, with a decomposition-quality bar and a
  tier-2 eval.
rationale: |
  The execution engine, audit log, and rewind already exist and are the value;
  the only gap is an authoring/ergonomics seam. A thin stdin entry over the
  existing pipeline keeps the blast radius small and inherits koto's guarantees
  unchanged. Persisting the compiled form is forced by koto re-verifying the
  template hash every tick; persisting the source closes the audit gap that
  compiled-only storage would leave.
---

# DESIGN: koto ad-hoc workflows

## Status

Proposed

## Context and Problem Statement

koto executes workflows defined as templates: a markdown/YAML source is
compiled to content-addressed JSON (`koto template compile`), and `koto init
<name> --template <file>` starts a session from a compiled template on disk.
Every `koto next` tick re-reads the compiled template and re-verifies its
SHA-256 hash, so the compiled artifact is a live, per-session dependency. The
only assisted authoring path today is the `koto-author` skill, which produces a
*durable, reusable* template + paired SKILL.md meant to be committed and run
many times.

This leaves a gap: an agent handed a novel, one-off complex task cannot obtain
koto's ordered/recoverable/auditable execution without first materializing a
template file (by hand or via the heavyweight durable-authoring flow). There is
no way for an agent to decompose the task in front of it into a workflow and run
it immediately, using koto alone.

The technical problem this design solves: provide a single-shot path from an
inline, agent-authored workflow definition to a running koto session, reusing
the existing compile-and-execute machinery, while (a) validating strictly with
agent-actionable errors, (b) keeping the compiled artifact alive for the
session's per-tick hash verification, and (c) preserving the human-readable
authored definition for the audit trail. The capability is paired with a
koto-skills teaching skill (decomposition guidance + quality bar) and a
behavioral eval.

*The requirements for this work are specified in an accepted PRD held in a
private planning tracker; this design restates the problem in implementation
terms and does not depend on that document.*

## Decision Drivers

- **Reuse the existing engine.** No new workflow-execution semantics, and no
  changes to the state-file format or gate model — the capability is a thin
  entry point over the current compile + init + state-machine path.
- **Hash-verification persistence.** koto re-verifies the compiled template's
  SHA-256 hash on every `koto next`; whatever the stdin path produces must
  persist for the session's lifetime or running workflows break mid-flight.
- **Auditability.** The human-readable authored definition must be recoverable
  from the session, not only the compiled JSON.
- **Strict, agent-actionable validation.** Definitions must meet koto's current
  template standard (structured gate routing; no legacy patterns), and errors
  must name the failing element so an agent can self-correct.
- **Single-shot ergonomics.** No scratch file and no separate compile step on
  the authoring path.
- **Teachable and evaluable.** The path must be exercisable by a koto-skills
  skill and verifiable by a tier-2 (execution-based) eval, including a
  decomposition-quality assertion.
- **Keep ephemeral distinct from durable.** The path must not become a backdoor
  substitute for durable, reusable templates authored via `koto-author`.
