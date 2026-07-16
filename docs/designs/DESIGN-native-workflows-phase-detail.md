---
schema: design/v1
status: Planned
upstream: docs/prds/PRD-native-workflows-phase-detail.md
problem: |
  Feature 1 emits a bare-status projection into Claude Code's `/workflows`
  (name + running/done/failed). Feature 2 must enrich that single-session
  projection into the session's real structure -- ordered phases with the
  active one marked, the active directive/label, per-phase evidence and gate
  outcomes, and a gate-blocked -> blocked status -- by reusing koto's dashboard
  detail read seam and extending F1's `koto-<uuid>.json` contract additively,
  without reopening F1's commit-funnel hook, publish/discover, atomic write, or
  opt-in.
decision: |
  Add a richer projection built by reusing the dashboard read seam
  (`read_detail` -> `DetailData`, plus a full-log per-state bucketing for
  completed-phase outcomes), and map it onto Claude Code's `/workflows` run
  schema by emitting an ordered `phases[]` array and a `workflowProgress[]`
  array (a `workflow_phase` marker per phase plus a `workflow_agent` step per
  visited/active phase carrying the directive and the phase's
  evidence/gate outcome). Order phases by a structural walk of the compiled
  template from `initial_state` following declared transitions (nearest-first,
  declared-transition order, dedup, self-loops skipped), appending any
  unreachable states in template order; the current state is the active phase.
  Map koto's blocked-in-current-epoch to a new `blocked` render status. All new
  fields are additive under a bumped `contractVersion: 2`; a committed shape
  fixture pins the enriched shape for Feature 4's guard.
rationale: |
  The dashboard already derives exactly these fields (`read_detail` returns
  directive, evidence, gate outcome, and the blocked classification), so
  reusing it keeps koto's model the single source of truth and matches F1's
  "reuse the read seam" driver; `read_detail` is current-epoch-scoped, so a
  small full-log per-state bucketing over the same event payloads supplies the
  completed-phase outcomes it does not carry. Claude Code's `/workflows` run
  schema renders `phases[]` + `workflowProgress[]` as an ordered phase tree with
  per-step outcomes and supports a `blocked` status (established empirically
  against the same surface F1 pinned), so emitting those fields is the native
  mapping -- and it stays additive over F1 because F1's top-level fields and
  `koto` block are untouched. Structural (template-order) phase ordering is
  chosen over runtime-visited order because the phase skeleton must be stable
  across the refresh-on-open lifetime of the entry: the operator sees a
  consistent list that fills in, not a list that reshuffles as history grows.
  A per-session file of phase-steps does not collide with Feature 3, which
  renders each session as its own separate entry (no nested single-run).
---

# DESIGN: real phase/agent detail in the rendered `/workflows` entry

## Status

Planned

Mechanism design for Feature 2 of the koto-agent-surface-legibility roadmap:
the koto-model -> `/workflows` field mapping that enriches Feature 1's
single-session projection. It settles the forks left open by the Accepted
`PRD-native-workflows-phase-detail`. The surface decision and F1's foundation
(commit-funnel hook, context-store publish/discover, extensible contract,
atomic write, opt-in) are settled and are not reopened here; this design pins
the derivation and the on-disk field mapping F2 adds on top.

## Context and Problem Statement

Feature 1 shipped the `workflows_surface` module (koto working tree):

- `contract.rs` -- the serde `WorkflowFile` (top-level `id`, `name`, `status`,
  `startTime`; nested `koto` block with `sessionId`, `workflow`,
  `currentState`, `contractVersion`), `CONTRACT_VERSION = 1`, and a
  `RenderStatus` enum with `Running`/`Completed`/`Failed`.
- `project.rs` -- `derive_minimal_projection(backend, session_id) -> Projection`,
  reusing the pure derivations `derive_state_from_log`, `derive_machine_state`,
  `is_terminal_state`, `is_failed_state` from `crate::engine::persistence`, plus
  a minimal display-label rung (`derive_display_name`).
- `discover.rs` -- the `workflows/publish-location` key, the
  self-then-ancestor walk, and the event-free publish.
- `materialize.rs` -- `materialize_after_commit`, wired into
  `LocalBackend::append_event` (the single commit funnel), the opt-in gate, the
  stable `startTime`, and the atomic temp-then-rename write + `create_dir_all`.

F1 deliberately emitted the thinnest projection. The information F2 needs is
already derived elsewhere in koto: the dashboard's detail read seam,
`read_detail(path, session_id) -> Option<DetailData>` in `src/cli/dashboard_data.rs`,
returns per-session structure -- `current_state`, `directive` (from the compiled
template for the current state), `evidence` (current-epoch `EvidenceSubmitted`
entries), the latest gate (`gate_name`, `result` = PASS/FAIL), `intent`,
`template_name`, `history`, and `remaining`. The dashboard's `read_session`
also computes `is_blocked` (a non-terminal session whose most recent
current-epoch `GateEvaluated` outcome != `passed`) and `classify_status` maps
that to the `blocked` bucket.

Claude Code's `/workflows` screen globs `<projectDir>/<sessionId>/workflows/*.json`,
`JSON.parse`s each file, defaults every field, sorts by `startTime`, and renders
each as a run entry (the surface F1 pinned empirically; unchanged here). Beyond
the top-level fields F1 emits, a run entry renders two structures that carry
phase/step detail (established empirically against the same Claude Code surface,
v2.1.x):

- **`phases`** -- an ordered array of `{ title, detail }`. `title` is the phase
  label; `detail` is a subtitle line.
- **`workflowProgress`** -- an ordered array mixing two node types:
  - `{ type: "workflow_phase", index, title }` -- a phase marker (1-based
    `index`), rendered as a phase header in the progress tree.
  - `{ type: "workflow_agent", index, label, phaseIndex, phaseTitle, state,
    promptPreview, resultPreview, ... }` -- a step under a phase; `state` is one
    of `done` / `progress` / `queued`; `promptPreview` and `resultPreview` are
    the step's input and output lines.

A run entry's top-level `status` renders `running`, `completed`, `failed`, and
`blocked`. The problem is to project koto's per-session detail onto these
fields -- for one non-hierarchical session -- additively over F1's shape.

Concrete seams this design builds on:

- Read seam: `read_detail` / `DetailData` and the pure derivations in
  `crate::engine::persistence` (`derive_state_from_log`, `derive_machine_state`,
  `is_terminal_state`, `is_failed_state`); `read_session`'s `is_blocked`
  computation and `classify_status`'s `blocked` bucket in `src/cli/dashboard.rs`.
- Compiled template: `crate::template::types::CompiledTemplate` (`initial_state`,
  `states: BTreeMap<String, TemplateState>`, each `TemplateState` carrying
  `directive`, `transitions: Vec<Transition>` with `target`, and `terminal`).
- F1 module: `WorkflowFile` / `KotoBlock` / `RenderStatus` in `contract.rs`,
  `Projection` in `project.rs`, the writer in `materialize.rs`.

## Decision Drivers

- **Reuse the read seam; koto's model stays the single source of truth.** The
  richer fields must be a derivation over the same helpers the dashboard uses,
  in the same layer -- not a parallel re-read that can drift from koto's
  terminal/blocked semantics (the PRD's R5, F1's carried-forward driver).
- **Additive over F1; F1's shape and render preserved.** F1 defined a minimal
  valid shape and a `contractVersion`; F2 adds fields and bumps the version, and
  never breaks F1's top-level fields or `koto` block (R6).
- **Native mapping onto the `/workflows` schema.** Emit the fields the screen
  already renders for phase/step detail (`phases`, `workflowProgress`,
  `status: blocked`) rather than inventing koto-only fields the screen ignores.
- **Stable phase skeleton across refresh-on-open.** The entry is
  refresh-on-open; the ordered phase list must not reshuffle as the session
  advances, so ordering is derived from the template structure, not from runtime
  history.
- **Do not box out Feature 3 or Feature 4.** The per-session phase-step model
  must not collide with F3's per-session-entry hierarchy model, and the enriched
  shape must be pinned by a fixture F4's guard can adopt (R7).
- **Best-effort, non-breaking.** Any per-field derivation failure degrades to an
  omitted/empty field and never fails the commit (R10, mirroring F1).

## Considered Options

### Fork A -- Phase-ordering strategy (the load-bearing open question)

koto states form a directed graph that can branch (e.g. `staging -> production`
or `rollback`), self-loop (wait states), and be re-entered by `koto rewind`.
"Phases in order" (PRD R1) must resolve that graph into a stable, legible
sequence. Three viable strategies were evaluated against legibility,
stability across refresh-on-open, determinism, and robustness to
branch/loop/rewind.

**Chosen: structural walk from `initial_state` following declared transitions.**
Order the phases by walking the compiled template from `initial_state`,
visiting each state's `transitions` targets in declared order, deduping
already-seen states, and skipping self-loops; append any states unreachable
from `initial_state` in template (`BTreeMap`) order as a deterministic tail.
Each koto state becomes exactly one phase; the current state is the active
phase, states visited in the event log are `done`, the rest are upcoming.

- *Score:* legibility high (matches the authored flow of the template),
  stability high (the phase list is a pure function of the template, so it does
  not reshuffle as the session advances or rewinds -- only the active marker and
  per-phase status move), determinism high (declared-transition order + a sorted
  tail is fully deterministic), robustness high (branches are visited in
  declared order; self-loops and rewinds change status, not the phase list).
- *Rejected: runtime-visited order* (order = the `to` sequence in the event
  log, then remaining states appended). Truthful about what happened, but the
  phase list is empty-but-one at the start and reshuffles/repeats as the session
  loops or rewinds -- an unstable skeleton under refresh-on-open, and it cannot
  show upcoming phases "in order" before they are visited. Fails the stability
  driver.
- *Rejected: hybrid* (visited states in runtime order, then reachable-remaining
  in structural order). Most faithful to "completed in the order they happened",
  but the past segment reshuffles as history grows and a looped/rewound state
  duplicates or moves; the extra complexity buys faithfulness the operator does
  not need, at the cost of the stability the refresh-on-open surface does. The
  per-phase status field already records what actually happened, so runtime
  order is not needed in the ordering itself.

The structural walk mirrors robustness patterns already in koto
(`measure_depth_from_parent`'s cycle-guarded walk; `batch.rs`'s DAG
topological order), so it is idiomatic, not novel. Known limitation carried
forward (not a regression): a branch/loop-heavy template has no single linear
truth; the walk emits a stable, sensible order and reflects loops/rewinds in
per-phase status rather than by duplicating phases. Finer per-branch rendering
is later-slice scope.

### Fork B -- Which `/workflows` fields to emit for a single session

**Chosen: emit both `phases[]` and `workflowProgress[]`.** `phases[]` gives the
ordered phase list with a `detail` subtitle; `workflowProgress[]` gives the
progress-tree the screen renders, with a `workflow_phase` marker per phase (so
the active phase is positionally marked) and a `workflow_agent` step per
visited-or-active phase carrying the phase's directive (`promptPreview`) and its
evidence/gate outcome (`resultPreview`), with `state` = `done` for completed
phases and `progress` (or the blocked case, see Fork D) for the active phase.
This is the mapping the reference surface renders as an ordered phase tree with
per-step outcomes, and it satisfies R1 (ordered + active marked), R2 (active
directive), and R3 (per-phase outcomes) together.

- *Rejected: `phases[]` only, no `workflowProgress[]`.* Simpler, and it renders
  a phase list, but it under-uses the surface: the progress tree and the
  per-step outcome lines (`resultPreview`) are exactly where "what the last step
  produced" reads best, and omitting `workflowProgress` leaves R3's per-phase
  outcomes cramped into a single `detail` string. Emitting both is only
  marginally more code (both are built from the same ordered phase list) and is
  the shape the surface is built to render.
- *Note on Feature 3.* The `workflow_agent` steps here represent the phases of
  one koto session -- not delegate sessions. Feature 3 renders each session
  (coordinator, delegates, grandchildren) as its own separate
  `koto-<uuid>.json` entry, per the settled "no nested single-run" decision, so
  F2's per-session phase-steps and F3's per-session entries do not collide.

### Fork C -- Where the completed-phase outcomes come from

**Chosen: reuse `read_detail` for the current epoch and add a full-log
per-state bucketing for completed phases.** `read_detail` already returns the
active phase's `directive`, current-epoch `evidence`, and the latest gate
outcome -- everything the active phase needs. It is current-epoch-scoped, so it
does not carry the outcomes of phases the session left behind. For those, F2
adds a small derivation over the *full* event log that buckets, per state, the
most recent `GateEvaluated` outcome and the `EvidenceSubmitted` entries recorded
while in that state, reusing the exact `EventPayload` matching `read_detail`
uses. This is a reuse of the same derivations at two scopes, not a second
model.

- *Rejected: widen `read_detail` to return all epochs.* `read_detail` is a
  dashboard seam with a current-epoch contract several dashboard callers depend
  on; widening it risks those callers. F2 adds its per-state bucketing in the
  `workflows_surface` layer (calling the same pure helpers), leaving the
  dashboard seam's contract intact.
- *Rejected: reimplement the derivations in the materializer.* Duplicates
  logic that must stay in lockstep with koto's evidence/gate semantics -- the
  drift the "single source of truth" driver forbids.

### Fork D -- Blocked-status mapping

**Chosen: reuse koto's blocked-in-current-epoch classification.** A non-terminal
session whose most recent current-epoch `GateEvaluated` outcome is not `passed`
renders top-level `status: blocked` (the same predicate `read_session`'s
`is_blocked` computes and `classify_status` buckets). The active phase's
`workflow_agent` step then renders with the failed gate outcome as its
`resultPreview`. `blocked` is added to the `RenderStatus` enum; it is evaluated
before the running fallback and after the terminal check, so the precedence is
terminal (completed/failed) > blocked > running -- matching the dashboard.

- *Rejected: infer blocked from evidence absence or a stuck timer.* koto already
  has a precise blocked signal (the failed-gate predicate); a heuristic would
  diverge from the dashboard's classification.

### Fork E -- Contract extension and the Feature 4 fixture

**Chosen: additive fields under a bumped `contractVersion: 2`, pinned by a
committed shape fixture.** The new fields (`phases`, `workflowProgress`, and the
`blocked` status value, plus any koto-block additions) are added to
`WorkflowFile` as additive serde fields; F1's `id`/`name`/`status`/`startTime`
and the `koto` block keep their shape and meaning. `CONTRACT_VERSION` bumps
1 -> 2. A committed JSON fixture of a representative enriched file (a multi-phase
session: some phases done with outcomes, one active with a directive, one
blocked variant) plus a contract test that asserts the emitted shape against it
gives Feature 4's guard a stable anchor (R7). F1's minimal shape remains a valid
`contractVersion: 1`-compatible subset for readers that ignore the new fields.

- *Rejected: reshape the file or rename F1 fields.* Breaks F1's render and F1's
  readers; the contract was designed to be extended, not reshaped.
- *Rejected: defer the fixture to Feature 4.* The roadmap makes F2 responsible
  for extending F4's fixture whenever it changes the shape; deferring would
  leave F4 under-covering the shape F2 introduced.

## Decision Outcome

On each `SessionBackend::append_event` (unchanged F1 funnel and opt-in gate),
the materializer builds the enriched projection and writes it into the resolved
`/workflows` directory as `koto-<uuid>.json` (unchanged filename, atomic write,
mkdir):

1. **Resolve the target directory** -- unchanged F1 path (env self-publish +
   self-then-ancestor walk; no location -> write nothing).
2. **Derive the enriched projection:**
   a. The F1 minimal fields (display name, current state, terminal/failed) via
      the existing `derive_minimal_projection` helpers.
   b. The active-phase detail (directive, current evidence, latest gate,
      blocked) via the `read_detail` read seam.
   c. The ordered phase list via the Fork A structural walk of the compiled
      template; per-phase status (done / active / upcoming) from the current
      state and the visited-state set.
   d. The per-completed-phase outcomes (latest gate outcome + evidence) via the
      Fork C full-log per-state bucketing.
3. **Map onto the file contract:** top-level `status` (with `blocked` added),
   `phases[]` (title = human phase label, detail = outcome/directive line), and
   `workflowProgress[]` (a `workflow_phase` marker per phase + a `workflow_agent`
   step per visited/active phase). All additive; `contractVersion: 2`.
4. **Write atomically** -- unchanged F1 writer.

This satisfies every PRD requirement: R1 (ordered phases via Fork A +
`workflow_phase` markers; active marked), R2 (active directive via `read_detail`
into the active step/phase detail), R3 (per-phase outcomes via Fork C into
per-phase `detail` / `resultPreview`), R4 (blocked via Fork D), R5 (reuse the
read seam, Forks C/D), R6 (additive contract, Fork E), R7 (fixture, Fork E),
R8/R9/R10 (F1 foundation and default path untouched; best-effort derivation).

## Solution Architecture

Changes are confined to the `workflows_surface` module plus the two dashboard
helpers it reuses (lifted to shared visibility if currently private to `cli/`).

- `contract.rs` -- extend `WorkflowFile` with additive `phases: Vec<Phase>` and
  `workflow_progress: Vec<ProgressNode>` (serialized as `phases` and
  `workflowProgress`); add `Blocked` to `RenderStatus`; bump
  `CONTRACT_VERSION` to 2. `Phase { title, detail }`; `ProgressNode` is an enum
  serializing the two `type`-tagged shapes (`workflow_phase` with
  `index`/`title`; `workflow_agent` with `index`/`label`/`phaseIndex`/
  `phaseTitle`/`state`/`promptPreview`/`resultPreview`). Serde `skip_serializing_if`
  keeps optional step fields absent when empty so the shape stays minimal.
- `project.rs` -- add an `EnrichedProjection` (or extend `Projection`) carrying
  the ordered phases, per-phase status/outcome, and the active-phase directive;
  add the Fork A `ordered_phases(&CompiledTemplate)` structural walk and the
  Fork C `per_state_outcomes(&[Event])` full-log bucketing; source the
  active-phase directive/evidence/gate/blocked from `read_detail`. Reuse
  `derive_display_name` for phase/entry labels.
- `materialize.rs` -- build the enriched `WorkflowFile` from the enriched
  projection (the resolve/gate/write path is otherwise unchanged).
- Read-seam reuse: call `read_detail` (and lift any needed private helper such
  as the blocked predicate to a shared location) so the materializer and the
  dashboard share one derivation. No change to the dashboard's public behavior.

Data/control flow on a commit (delta over F1 in *italic*):

```
koto next / --to / rewind / exit
  -> SessionBackend::append_event (LocalBackend)
       -> persistence::append_event  (state file written)
       -> materialize_after_commit(self, id)
            gate: opt-in?  no -> return                     (F1, unchanged)
            dir = resolve_publish_location(...)             (F1, unchanged)
            proj = derive_minimal_projection(...)           (F1, unchanged)
            detail = read_detail(state_path, id)            (new: active-phase detail)
            phases = ordered_phases(compiled_template)      (new: Fork A)
            outcomes = per_state_outcomes(events)           (new: Fork C)
            file = WorkflowFile{ ...F1 fields, status(+blocked),
                                 phases, workflowProgress }  (new: Forks B/D/E)
            atomic-write koto-<uuid>.json                   (F1, unchanged)
```

## Implementation Approach

Dependency-ordered, each step landing tests:

1. **Contract extension** (`contract.rs`): `Phase`, `ProgressNode`, the
   `phases`/`workflowProgress` fields, `RenderStatus::Blocked`, bump to
   `CONTRACT_VERSION = 2`; unit tests over the serialized shape (keys, the two
   progress node types, `status: blocked`, additive-over-F1). No behavior change
   to the writer yet.
2. **Ordered phases + per-state outcomes** (`project.rs`): the Fork A structural
   walk and the Fork C full-log bucketing, as pure functions over the compiled
   template and the event log; unit tests over fixtures (linear, branching,
   self-loop, rewound, unreachable-tail).
3. **Enriched projection + blocked** (`project.rs`): assemble the enriched
   projection reusing `read_detail` for the active-phase directive/evidence/
   gate and the blocked predicate; unit tests for running/active-directive,
   completed-phase-outcomes, and blocked classification.
4. **Materialize wiring** (`materialize.rs`): build the enriched `WorkflowFile`;
   extend the module's integration tests to assert AC1-AC3 against the emitted
   file (multi-phase ordered + active marked + completed outcomes; active
   directive present; gate-blocked -> `status: blocked`).
5. **Shape fixture + verification harness** (Fork E; `tests/` or `scripts/`): a
   committed enriched-shape JSON fixture and a contract test pinning it for F4;
   extend `scripts/verify-native-workflows.sh` and the verification guide to
   exercise the F2 properties alongside F1's four checks (no regression).

## Security Considerations

- **No new inputs or seams.** F2 adds derivation and fields over F1's existing
  commit-funnel side effect; it opens no new file, socket, env var, or CLI
  surface. The `/workflows` directory value, the filename, and the opt-in gate
  are F1's, unchanged.
- **Directive/evidence content is koto's own log data.** The directive comes
  from the compiled template; evidence and gate outcomes come from the session's
  own event log. They are serialized as JSON string *values* (never as keys or
  paths), so no injection or traversal surface opens through them. Long fields
  are the operator's own workflow content; truncation for display is a
  legibility concern, not a security one.
- **Undocumented-surface isolation preserved.** The coupling to Claude Code's
  `phases`/`workflowProgress` shape stays confined to `contract.rs` (and the
  fixture); koto's engine, event log, and dashboard do not depend on it. The
  guard that fails loudly on drift remains Feature 4; F2 pins the shape fixture
  F4 will guard against.
- **Best-effort containment.** A derivation failure (unreadable template,
  malformed epoch) degrades the affected field to empty/omitted and never fails
  the commit or writes an invalid file (R10), the same discipline F1 applies.

## Consequences

- The per-commit materialization now derives a richer projection (an added
  `read_detail` call, a template walk, and a full-log bucketing) on the
  enabled path; the default (no-location) path is unchanged -- it still returns
  after the cheap opt-in probe.
- `koto-<uuid>.json` grows from a status line to a phase tree. The file contract
  is now `contractVersion: 2`; F1's `contractVersion: 1` fields remain a valid
  subset, so a reader that ignores the new fields still renders F1's entry.
- A committed enriched-shape fixture now exists; Feature 4's guard adopts it as
  its anchor, discharging the roadmap's F2 shape-change obligation.
- Carried-forward limitations (not regressions): branch/loop-heavy templates
  have no single linear phase order (the structural walk emits a stable,
  sensible one and reflects loops/rewinds in per-phase status); the finer
  terminal-inference edge cases F1 noted remain later-slice scope; per-agent
  hierarchy detail is Feature 3.
- If a future Claude Code version changes the `phases`/`workflowProgress` shape,
  only `contract.rs` and the fixture change -- the core-isolation property F1
  established is preserved.

## References

- `PRD-native-workflows-phase-detail` -- the requirements this design satisfies.
- `DESIGN-native-workflows-render` -- Feature 1's design, whose
  `workflows_surface` module and contract this design extends.
- `ROADMAP-koto-agent-surface-legibility` (Feature 2, and the F2/F4 shape-change
  soft coupling) -- the roadmap feature and obligation this design implements.
- koto seams: `src/cli/dashboard_data.rs` (`read_detail`, `DetailData`,
  `read_session`'s `is_blocked`), `src/cli/dashboard.rs` (`classify_status`
  blocked bucket), `src/engine/persistence.rs` (pure state/terminal helpers),
  `src/template/types.rs` (`CompiledTemplate`, `TemplateState`, `Transition`),
  `src/workflows_surface/` (F1's contract/project/materialize).
- The `/workflows` `phases` / `workflowProgress` / `blocked` render fields were
  established empirically against the same undocumented Claude Code surface
  Feature 1 pinned (v2.1.x); the guard that makes drift fail loudly is Feature 4.
