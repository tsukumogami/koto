---
schema: prd/v1
status: Accepted
upstream: docs/briefs/BRIEF-native-workflows-phase-detail.md
problem: |
  Feature 1 renders a koto session in Claude Code's `/workflows` as a bare
  status (name + running/done). The operator cannot read what the session is
  doing -- its phases, where it is, what the active step asks for, what the last
  step produced, or whether it is gate-blocked. That structure is already in
  koto's per-session model and is exactly what `/workflows` is built to render.
goals: |
  Enrich the single-session projection F1 emits from a status into the
  session's real structure: ordered phases with the active one marked, the
  active phase's directive/label, per-phase evidence and gate outcomes, and a
  gate-blocked -> blocked status distinct from running and done. The richer
  fields are derived by reusing koto's dashboard detail read seam and are added
  to the F1 file contract additively (new fields, bumped version, F1's render
  preserved), with the contract's shape fixture extended so Feature 4's guard
  can validate the enriched shape.
---

# PRD: real phase/agent detail in the rendered `/workflows` entry

## Status

Accepted

Requirements for Feature 2 of the koto-agent-surface-legibility roadmap,
derived from the Accepted `BRIEF-native-workflows-phase-detail`. The surface
decision is settled upstream, and Feature 1's foundation (the
`workflows_surface` module, commit-funnel materialization, context-store
publish/discover, the extensible `koto-<uuid>.json` contract, atomic write,
opt-in-by-published-presence) is fixed and not reopened here. This PRD fixes the
WHAT and the acceptance contract; the koto-model -> `/workflows` field mapping
is settled downstream in `DESIGN-native-workflows-phase-detail`.

## Problem Statement

Feature 1's walking skeleton proved the path and deliberately carried the
thinnest projection: a session's name, its current state, and a
running/done/failed status. That is enough to answer "is a koto session
running?" but not "where is it and what is it doing?" -- the question an
operator has while watching a workflow advance.

The information to answer it already exists. koto's per-session model, exposed
through the dashboard's detail read seam (`read_detail` -> `DetailData`), holds
the ordered phases (the template's states), the active phase's directive, the
evidence each phase submitted, gate outcomes, and human-readable labels. Claude
Code's `/workflows` screen renders a run as a tree of phases and steps with
their outcomes and supports a `blocked` status alongside running and completed.
The gap is purely the projection: F1 emits a status where the screen can render
structure koto already derives.

This feature enriches the single-session projection to close that gap. It is
one of four independent increments over the F1 skeleton; it does not add
hierarchies, hardening, or lifecycle.

## Goals

- The single-session `/workflows` entry shows the session's phases in order,
  with the phase the session is currently in marked as active.
- The active phase's directive/label is legible in the entry.
- Each completed phase shows what it produced: its submitted evidence and/or its
  gate outcome.
- A session blocked on a gate that did not pass renders as *blocked*, distinct
  from *running* and *done*.
- The richer fields are a derivation over koto's model reusing the dashboard's
  existing detail read seam -- not a second store and not a bespoke re-read.
- The enriched file extends F1's contract additively: F1's shape and its render
  are preserved, new fields are namespaced/versioned, and the contract version
  is bumped so Feature 4's guard has a stable anchor.
- Feature 1's four "Verified when" checks continue to pass (no regression).

## User Stories

- As an operator watching a multi-phase koto session in Claude Code, I want the
  entry to list the workflow's phases in order with the current one marked, so
  that I can see where the session is without opening the dashboard.
- As that operator, I want the active phase's directive to be legible, so that I
  can read what the current step is asking for.
- As that operator, I want each completed phase to show its evidence or gate
  outcome, so that I can read the trail of what the session did.
- As that operator, I want a gate-blocked session to read *blocked*, so that a
  stalled run is not misrepresented as still advancing or as finished.
- As a koto maintainer, I want the enriched fields added to F1's contract
  additively with a bumped version and an extended shape fixture, so that
  Feature 4's guard validates the new shape and F1's readers do not break.

## Requirements

### Functional

- **R1 -- Ordered phases with the active one marked.** The entry carries the
  session's phases in a stable, meaningful order derived from the workflow's
  template, with the phase corresponding to the session's current state marked
  as the active phase and already-visited phases marked as completed.
- **R2 -- Active directive/label legible.** The entry surfaces the active
  phase's directive text (from the compiled template for the current state) and
  human-readable phase labels, so the operator reads what the current step
  asks for.
- **R3 -- Per-phase evidence and gate outcomes.** Each completed phase carries
  what it produced: the evidence submitted while in that phase and/or the
  outcome of the gate evaluated for that phase (pass/fail). The per-phase
  outcomes are derived from the session's full event log, not only the current
  epoch.
- **R4 -- Gate-blocked renders as blocked.** A non-terminal session whose most
  recent gate evaluation in the current epoch did not pass renders with a
  `blocked` status -- the same blocked-in-current-epoch classification koto's
  dashboard uses -- distinct from `running` and from the terminal
  `completed`/`failed`.
- **R5 -- Reuse the dashboard detail read seam.** The richer fields (phases,
  directive, evidence, gates, labels, blocked) are derived by reusing koto's
  existing per-session detail derivation (`read_detail` -> `DetailData` and its
  pure helpers), in the same layer -- a reuse that keeps koto's model the single
  source of truth, not a parallel derivation.
- **R6 -- Additive contract extension.** The enriched fields are added to the
  F1 `koto-<uuid>.json` contract without breaking F1's shape or its render: F1's
  top-level fields (`id`, `name`, `status`, `startTime`) and `koto` block are
  preserved, the new fields are namespaced/additive, and `contractVersion` is
  bumped.
- **R7 -- Shape-change obligation for Feature 4.** Because F2 changes the file
  shape, the contract's shape fixture / contract check is extended (or, if F1
  shipped none, the extended shape is documented in the contract) so Feature 4's
  guard can validate the enriched shape per the roadmap's stated soft coupling.

### Non-functional

- **R8 -- Preserve F1's foundation unchanged.** The commit-funnel
  materialization point, the context-store publish/discover
  (`workflows/publish-location` key + self-then-ancestor walk), the atomic
  temp-then-rename write plus mkdir, the UUID filename, and opt-in-by-
  published-presence are unchanged. F2 adds derivation and fields, not new
  seams.
- **R9 -- No regression.** Feature 1's four "Verified when" checks
  (single-session render, update on reopen, done-on-completion,
  default-path-untouched) continue to pass. `cargo build`, `cargo test`,
  `cargo clippy -- -D warnings`, and `cargo fmt --check` all pass.
- **R10 -- Best-effort, non-breaking derivation.** The enriched derivation is
  best-effort like F1's: any per-field derivation failure degrades gracefully
  (the field is omitted or empty) and never fails the commit or produces an
  invalid file.

## Acceptance Criteria

- [ ] **AC1 (F2 verified-when #1).** Opening `/workflows` on a multi-phase koto
  session shows its phases in order with the active one marked, and each
  completed phase's evidence/gate outcome visible. (Where a live-TUI check is
  not automatable in CI, a scripted verification exercises the same property
  against the emitted `koto-<uuid>.json`: a multi-phase session's file carries
  ordered phases, the current phase marked, and completed phases' outcomes.)
- [ ] **AC2 (F2 verified-when #2).** The active phase's directive/label is
  legible. (Scripted: the emitted file carries the current state's directive
  text and a human-readable active-phase label.)
- [ ] **AC3 (F2 verified-when #3).** A gate-blocked session renders as blocked,
  not running and not done. (Scripted: a session whose latest current-epoch gate
  did not pass emits `status: blocked`.)
- [ ] **AC4 (no regression).** Feature 1's four checks still pass: the F1
  verification harness (or its F2-extended successor) passes, covering
  single-session render, update on reopen, done-on-completion, and
  default-path-untouched.
- [ ] **AC5.** The enriched file preserves F1's shape and render: F1's top-level
  fields and `koto` block are present and unchanged in meaning; the new fields
  are additive; `contractVersion` is bumped.
- [ ] **AC6.** The contract's shape fixture / documented contract is extended to
  cover the enriched shape so Feature 4's guard can validate it.
- [ ] **AC7.** `cargo build`, `cargo test`, `cargo clippy -- -D warnings`, and
  `cargo fmt --check` all pass; the koto-user / koto-author skills are assessed
  and updated if F2 changes any surface they document.

## Out of Scope

- Hierarchies: coordinator and delegates each rendering as their own entries
  (Feature 3). F2 is one non-hierarchical session.
- The version/fixture guard and rendered smoke check over the undocumented
  surface (Feature 4). F2 extends the fixture and documents the shape so F4 can
  build the guard; it does not build the guard.
- Retention/rotation and crash-staleness (Feature 5).
- Nested single-run rendering, MCP, and any koto skill, reader, or parallel
  surface (settled out by the ADR).
- Reopening F1's commit-funnel hook, context-store publish/discover, atomic
  write, mkdir, or opt-in mechanism.
- The finer terminal-inference edge cases F1 carried forward as known
  limitations (unnamed-failure states beyond the failure heuristic, missing/
  changed template) beyond what the reused read seam already handles.

## Decisions and Trade-offs

- **Reuse the detail read seam, do not build a new derivation.** The richer
  fields are exactly what koto's dashboard already derives via `read_detail`;
  reusing that derivation (and lifting any private helpers to a shared spot)
  keeps koto's model the single source of truth and matches the F1 driver. A
  bespoke re-read would invite drift against koto's terminal/blocked semantics.
- **Per-phase outcomes need the full log, blocked needs the current epoch.**
  `read_detail` is current-epoch-scoped (directive, current evidence, latest
  gate, blocked). Per-phase completed outcomes (R3) additionally require a
  full-log per-state bucketing. F2 reuses the same event-payload derivations for
  both scopes rather than inventing new ones.
- **Additive contract, bumped version.** F1 defined a minimal valid shape and a
  `contractVersion`; F2 adds fields and bumps the version rather than reshaping,
  so F1 readers and F4's guard both anchor on a stable, versioned contract.
- **The shape-change obligation is discharged here.** The roadmap makes F2/F3
  responsible for extending F4's guard fixture whenever they change the file
  shape. F2 extends the fixture / documents the enriched shape so F4 is not left
  under-covering.
- **Phase-ordering strategy is a DESIGN decision.** How koto states become an
  ordered phase list (structural traversal vs runtime-visited order vs hybrid)
  is the load-bearing open question; it is settled in the DESIGN's considered
  options, not here.
