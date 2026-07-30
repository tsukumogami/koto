---
status: Proposed
problem: |
  A koto session's state header already records the `template_source_dir`
  it was initialized from, but nothing reads that field back to check
  whether the directory still exists on `koto status`, `koto init`'s
  "already exists" collision path, or `koto session list`. A session
  created inside a working tree that's later torn down (reaped ephemeral
  sandbox, `git worktree remove`, container teardown) stays "live" from
  koto's point of view forever, and a later `koto init <same-name>` from an
  unrelated environment collides with it and fails with a generic
  "already exists" error indistinguishable from a real concurrent session
  (tsukumogami/koto#189). Koto already ships almost this exact mechanism --
  `SchedulerWarning::StaleTemplateSourceDir`, from Decision 14 in
  DESIGN-batch-child-spawning.md -- but scoped narrowly to the batch
  scheduler's per-tick child-template-path resolution, invisible to the
  three call sites where this issue actually bites. This design decides how
  to extend that existing pattern to `init`/`status`/`session list`: where
  the new signal lives, what it's named (given `koto workflows --orphaned`
  already means something unrelated), and how it behaves across the
  already-shipped local vs. cloud-sync backends.
---

# DESIGN: Orphaned Session Detection

## Status

Proposed

## Context and Problem Statement

<From exploration findings. Cover what prompted the exploration, what was
discovered, and what architectural or technical decisions remain open.>

## Decision Drivers

<From exploration findings. List the factors that should influence the
technical decision. Pull from tensions, constraints, and user priorities
surfaced during exploration.>

## Decisions Already Made

- **Direction 3 (an automatic sweep/gc/cleanup-by-staleness command) is out
  of scope for this design.** It's the only one of the issue's three
  candidate directions that requires a genuine new architecture decision
  (a whole-backend destructive sweep, which koto has no precedent for --
  the closest existing bulk-destructive verb, `koto workspace prune`, is
  root-scoped only), and it depends on this design's detection logic as a
  prerequisite. Deferred to a follow-up issue once directions 1+2 ship.
- **A bare `Path::exists()` check is sufficient for this design's scope.**
  Directions 1 and 2 are both read-only/informational (a status message, a
  list column/flag) -- koto's own existing `StaleTemplateSourceDir`
  precedent already accepts this same limitation for the same field, for
  the same reason: the failure modes a stronger check would catch
  (transient remounts, directory reuse, renames) matter for destructive
  action, which is out of scope per the decision above.
- **The fix should extend/reuse `StaleTemplateSourceDir`'s existing shape
  and `machine_id` vocabulary** (`src/engine/scheduler_warning.rs`,
  `src/engine/path_resolution.rs`) rather than invent a second,
  differently-shaped signal for the same underlying condition, so the
  codebase doesn't end up with two incompatible ways to say "this recorded
  directory doesn't exist here."
- **Whatever flag/column direction 2 adds must not be named `--orphaned`**,
  or must clearly disambiguate from the existing, differently-scoped
  `koto workflows --orphaned` flag (documented in
  PRD-hierarchical-workflows.md / DESIGN-hierarchical-workflows.md, meaning
  "parent workflow no longer exists") in both CLI help text and wire
  format.

See `wip/explore_orphaned-session-detection_findings.md` and
`wip/research/explore_orphaned-session-detection_r1_lead-*.md` for the full
exploration this design was handed off from, including the six research
leads' detailed findings on current plumbing, candidate-direction sizing,
staleness-check robustness, opt-in posture, and prior art (koto's own docs
and comparable tools).
