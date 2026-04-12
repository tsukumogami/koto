# Design Decisions: batch-child-spawning

## Phase 1 — Decomposition

Mode: `--auto` (session precedent; no `--interactive` flag passed)
Scope: Tactical (koto repo default)
Visibility: Public

### Decision Count

6 decisions after merging coupled questions. Scaling heuristic: 6-7 decisions
in `--auto` mode means "proceed, record high-priority assumption." Recording
the assumption below.

### Assumption

This design is being produced in `--auto` with 6 independent decisions. That
sits at the threshold where a human reviewer might prefer the work split
into two smaller designs (e.g., "scheduler and schema" vs "failure and
observability"). Proceeding as one document because all six decisions share
the same implementation surface (`src/engine/batch.rs`, `handle_next`,
`children-complete` gate output) and separating them would force cross-
document references to resolve shared types and helpers. If the reviewer
finds the single document unwieldy, the Implementation Approach section
will cleanly split into two phases that could be revisited as separate
designs.

### Decision List

1. **Task list schema, template hook shape, and compiler validation.**
   Couples: the exact shape of a task entry (required/optional fields), the
   name and placement of the template-level hook that declares
   materialization, and what `koto template compile` validates at load
   time. These are one contract with three facets.
   *Complexity:* standard. Options are concrete and reversible via
   format_version bump if we get it wrong.

2. **Atomic child-spawn window fix.**
   The integration lead flagged a narrow crash window between
   `backend.create` and the first `append_event` that leaves a header-only
   state file. Downstream tasks block until manual cleanup. Options must
   close the window without breaking append-only semantics.
   *Complexity:* critical. Wrong choice silently breaks resume across
   crashes.

3. **Forward-compat diagnosability.**
   A batch-hook template compiled against a pre-batch koto binary silently
   no-ops today (serde ignores unknown fields). Options: bump
   format_version to 2, add `deny_unknown_fields` retroactively, warn at
   compile time, or accept silent no-op.
   *Complexity:* standard. Reversible, bounded to template format layer.

4. **Child-template path resolution.**
   When the parent submits a task list and koto spawns children, how do
   relative template paths resolve? Options: absolute-only, parent-cwd-
   captured-at-submission, child-cwd-at-spawn, or a new root rule.
   *Complexity:* standard. Touches one helper but affects every template
   author.

5. **Retry mechanics and failed/skipped representation.**
   Couples: how a failed child is marked as failed (current terminal state
   is just "terminal: true"), how skipped dependents are represented
   (real state files with synthetic events vs parent-side records), what
   evidence action the parent submits to re-queue a failed chain, and
   what the `children-complete` gate output exposes about pass/fail/skip
   counts. All four couple because a choice on representation drives the
   gate output, which drives the retry surface.
   *Complexity:* critical. Wrong representation breaks both
   `koto workflows --children` and resume semantics.

6. **Batch observability surface.**
   How `koto status` and `koto workflows --children` report batch state
   (task graph, ready/blocked/skipped counts, dependency explanations).
   Options: extend existing JSON outputs additively, add a new
   `koto batch status` subcommand, rely on
   `koto workflows --children <parent>` alone.
   *Complexity:* standard. Reversible surface change; somewhat couples
   with Decision 5 but the API shape is separable.

### Independence Check

- Decisions 2 and 3 both touch template/state-file safety but address
  different layers (code-level atomicity vs protocol versioning). No
  coupling.
- Decisions 1 and 3 both touch template format. Merging would hide the
  format_version question inside schema work where it would get lost.
  Keep separate.
- Decisions 5 and 6 share a conceptual domain (what the parent sees) but
  the retry/representation decisions are upstream of the observability
  decisions. Representation choices in 5 constrain 6, but 6 has its own
  degrees of freedom (subcommand vs inline output). Keep separate with
  a noted dependency: execute 5 before 6.

### Execution Plan

Phase 2 launches one decision agent per question. Decisions 1, 2, 3, 4, 6
can run in parallel. Decision 5 runs in parallel because its outputs
don't block the others at the investigation stage — Phase 3 will cross-
validate any assumptions that leaked across decisions.
