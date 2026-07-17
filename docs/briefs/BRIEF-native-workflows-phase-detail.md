---
schema: brief/v1
status: Done
problem: |
  Feature 1 made a koto session appear in Claude Code's `/workflows` screen,
  but the entry carries only a bare status -- name plus running/done. The
  operator can see *that* a koto session is running, not *what it is doing*:
  which phase it is in, what the current step is asking for, what the last
  completed step produced, or whether it is stuck waiting on a failed gate. The
  session's real structure is already in koto's model and shown in koto's
  dashboard; it is simply not projected into `/workflows`.
outcome: |
  The operator drives a multi-phase koto session inside Claude Code, opens
  `/workflows`, and reads the session's real structure: its phases in order
  with the active one marked, the active phase's directive, and each completed
  phase's evidence or gate outcome. A session blocked on a failed gate reads as
  blocked -- not running, not done. No new command, skill, or window; the
  richer detail rides the same file F1 already writes.
---

# BRIEF: real phase/agent detail in the rendered `/workflows` entry

## Status

Done

Framing for Feature 2 of the koto-agent-surface-legibility roadmap -- the first
value-adding increment over the Feature 1 walking skeleton. The surface
decision is settled upstream (koto produces `/workflows`'s native artifacts; no
skill or reader), and F1 landed the foundation this feature extends: the
`workflows_surface` module, materialization on the commit funnel, the
context-store publish/discover, and the extensible `koto-<uuid>.json` contract.
This brief captures F2's framing; the requirements belong to
`PRD-native-workflows-phase-detail` and the field mapping to
`DESIGN-native-workflows-phase-detail`.

## Problem Statement

Feature 1 proved the end-to-end path: a hosting Claude Code session publishes a
`/workflows` directory, a koto session writes `koto-<uuid>.json` there on every
state-commit, and `/workflows` renders it. But F1 deliberately carried the
thinnest projection -- the session's name, its current state, and a
running/done/failed status. That answers "is a koto session running?" It does
not answer the question the operator actually has while watching a workflow:
"where is it, and what is it doing right now?"

koto already holds the answer. Its per-session model has the ordered phases
(the template's states), the active phase's directive, the evidence each phase
submitted, the gate outcomes, and human-readable labels -- and koto's own
dashboard renders exactly this detail. Claude Code's `/workflows` screen is
itself built to show a run as a tree of phases and steps with their outcomes.
What is missing is the projection between the two: F1 emits a bare status where
the screen is ready to render structure koto already has.

This feature closes that gap for one session. It does not add hierarchies
(Feature 3), hardening (Feature 4), or lifecycle (Feature 5); it enriches the
single-session projection F1 emits from a status into the session's real
structure.

## User Outcome

An operator drives a multi-phase koto session inside a Claude Code session and
opens `/workflows`. Instead of a single running row, the entry shows the
session's phases in order -- each of the workflow's states as a phase -- with
the phase the session is currently in marked as active. The active phase shows
its directive, so the operator reads what the current step is asking for. Each
already-completed phase shows what it produced: the evidence it submitted or the
gate outcome it cleared. If the session is blocked on a gate that did not pass,
the entry reads *blocked* -- distinct from a still-advancing *running* and from
a finished *done*. When the operator reopens `/workflows` after the session
advances, the marked phase moves and the newly completed phase shows its
outcome. Nothing else about F1 changes: the same file, written on the same
commit funnel, opt-in by the same published location, with koto's default path
untouched when no host participates.

## User Journeys

### Read where a running session is

An operator has a multi-phase koto session advancing inside Claude Code.
Trigger: opening `/workflows` mid-run. Outcome shape: the entry lists the
workflow's phases in order, the current phase is marked active, and its
directive is legible -- the operator sees the session's position and current
step without leaving the TUI or opening the dashboard.

### Read what a completed phase produced

The same session has finished several phases. Trigger: reopening `/workflows`.
Outcome shape: each completed phase shows its evidence or gate outcome, so the
operator can read the trail of what the session did, phase by phase.

### See a gate-blocked session as blocked

A koto session hits a gate that does not pass and cannot advance until the
condition is met. Trigger: opening `/workflows` while the session is
gate-blocked. Outcome shape: the entry reads *blocked* -- not a spinning
*running* that hides the stall, and not *done*.

### Advance and watch the detail move

The operator advances the session another phase. Trigger: each state-commit,
then reopening `/workflows`. Outcome shape: the active-phase marker moves to the
new phase, the previously-active phase now shows its outcome, and the active
directive updates.

## Scope Boundary

**In:**

- Enriching the single-session `koto-<uuid>.json` projection from a bare status
  to the session's real structure: ordered phases with the active one marked,
  the active phase's directive/label, per-phase evidence and gate outcomes, and
  human-readable labels.
- Mapping koto's blocked-in-current-epoch (a non-terminal session whose most
  recent current-epoch gate did not pass) to a `blocked` render status,
  distinct from running and done.
- Deriving the richer fields by reusing koto's existing per-session detail read
  seam (the dashboard's `read_detail` derivation), in the same layer -- a
  reuse, not a second derivation.
- Extending the F1 file contract *additively*: new fields, a bumped contract
  version, F1's shape and render preserved.
- Noting/extending the contract's shape fixture so Feature 4's guard can
  validate the enriched shape (the roadmap's stated soft coupling).
- Extending F1's end-to-end verification harness to exercise the new
  properties.

**Out:**

- Hierarchies -- coordinator and delegates each rendering as their own entries
  (Feature 3).
- Hardening: the version/fixture guard over the undocumented surface and the
  rendered smoke check (Feature 4). F2 documents the extended shape and extends
  the fixture so F4 can pick it up, but does not build the guard.
- File lifecycle: retention/rotation and crash-staleness (Feature 5).
- Nested single-run rendering (delegates as agents under one run), MCP, and any
  koto skill, reader, or parallel surface (settled out by the ADR).
- Re-deciding the settled surface, or reopening F1's commit-funnel hook,
  context-store publish/discover, atomic write, or opt-in mechanism.

## References

- `ROADMAP-koto-agent-surface-legibility` (Feature 2) -- the roadmap feature
  this brief frames.
- `BRIEF-native-workflows-render` / `PRD-native-workflows-render` /
  `DESIGN-native-workflows-render` -- Feature 1's chain, whose foundation this
  feature extends.
- `PRD-native-workflows-phase-detail` -- the requirements derived from this
  brief.
- `DESIGN-native-workflows-phase-detail` -- the field mapping that satisfies
  them.
