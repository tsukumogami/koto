---
status: Proposed
problem: |
  Koto v0.7.0 lets a parent workflow spawn and wait for children, but the
  consumer has to run the spawn loop themselves: query which children are
  ready, spawn them, check for completion, spawn the next wave. For
  workflows where the full task set is known upfront (e.g. a plan parsed
  into a DAG of GitHub-issue children), this loop forces every consumer to
  re-implement scheduling in SKILL.md prose, which is brittle beyond a
  handful of tasks and blocks shirabe's adoption of koto for hierarchical
  templates (tsukumogami/shirabe#67). This design specifies a declarative
  alternative: the parent submits a task list as evidence, and koto owns
  materialization, dependency-ordered scheduling, completion detection,
  and failure routing end to end.
---

# DESIGN: batch-child-spawning

## Status

Proposed

## Context and Problem Statement

Issue #129 asks for declarative batch child spawning. The v0.7.0
hierarchical-workflows feature gave parents the primitives to spawn
children (`koto init --parent`), wait for them (`children-complete`
gate), and query hierarchy (`koto workflows --roots / --children /
--orphaned`). What v0.7.0 did *not* provide is a way for the parent to
hand koto a structured task list and let koto schedule the spawning
itself.

The gap manifests in consumers as spawn loops written in SKILL.md prose:
"query children state, compute next-ready from a dependency graph, spawn,
repeat." Shirabe's in-progress `work-on-plan.md` design
(tsukumogami/shirabe#67) hit this friction and stalled — natural-language
orchestration over a DAG of more than a handful of tasks is unreliable
and untestable. That PR is blocked on #129 and will be revisited once
this design ships.

The exploration surfaced a coherent implementation sketch:

- Parents submit a task list (name, template, vars, waits_on) as evidence.
- Koto's scheduler — a new CLI-level step in `handle_next` — parses the
  list on every `koto next` call, builds a DAG, computes which tasks are
  ready (all waits_on dependencies terminal), and spawns each ready task
  by reusing the same code path as `koto init --parent`.
- The `children-complete` gate (unchanged from v0.7.0) handles waiting.
- Child workflows are named `<parent>.<task>` deterministically, so the
  existing `backend.exists()` check in `handle_init` gives free
  idempotency on resume.
- Nothing new is persisted at the parent: the scheduler derives its
  working set from the latest evidence event plus on-disk child state
  files. Resume is the same code path as first invocation.
- Default failure policy is skip-dependents: a failed child marks its
  direct and transitive dependents as skipped, independent branches
  continue, and recovery happens via a `retry_failed` evidence action.
- Dynamic additions (a running child adds siblings mid-flight) work by
  evidence resubmission — `merge_epoch_evidence` unions the new tasks
  with the existing set, and the scheduler picks them up on the next
  tick.

Several decisions remain open at the design level: the atomic-write
window in `handle_init`'s header/event sequence, forward-compat
diagnosability when a batch template runs on a pre-batch koto binary,
child-template path resolution when the parent and its children spawn
from different working directories, the exact mechanics of the
`retry_failed` evidence action, and how `koto status` and
`koto workflows --children` report batch state to observers.

Exploration is documented in `wip/explore_batch-child-spawning_*.md` and
the five research files in `wip/research/`. Those artifacts are the
primary input for this design.

## Decision Drivers

- **Composition with v0.7.0 primitives.** The design must reuse
  `parent_workflow`, `koto init --parent`, and the `children-complete`
  gate without modification. New logic layers on; it doesn't replace.
  Regressing v0.7.0 behavior is out of scope.

- **Stateless CLI model.** Koto has no running daemon. Every `koto next`
  reads state from disk, acts, writes state back. The scheduler must fit
  that model — no persistent cursors, no background workers, no
  in-memory DAG that survives between calls. Every invocation is a pure
  function of the evidence log and on-disk child state.

- **Append-only state files.** The state file is JSONL, strictly
  append-only after the header. Cloud-sync incremental uploads, rewind,
  and the `expected_seq` read-check all depend on this. The design must
  not introduce header mutations or mid-file edits.

- **GitHub-issue use case is canonical.** The user's stated use case is
  implementing a plan of GitHub issues with inter-issue dependencies.
  That's sibling-level ordering, not nesting. The primary model must
  support "issue 3 waits for issues 1 and 2" where all three share a
  parent. Nested batches (a child spawns its own sub-batch) are a
  complementary capability, already supported unchanged by v0.7.0.

- **Dynamic additions are required.** The task set is not frozen at
  submission time. A running child must be able to append tasks that
  the parent's scheduler picks up on the next tick. Resume must handle
  appends correctly across crashes.

- **Failure routing should be safe-by-default.** The autonomous-agent
  use case can't afford to cascade failures silently or halt unrelated
  work on a single failure. Skip-dependents is the recommended default
  because it isolates faults, maximizes parallelism, and offers clean
  recovery. Alternatives are opt-in per batch.

- **Template compile-time validation should catch as much as possible.**
  Runtime errors halfway through a batch are much worse than
  compile-time errors at template load. The design should push
  validation forward — state shape, evidence reference, reachability —
  while accepting that per-task template paths and cyclic `waits_on`
  are necessarily runtime checks.

- **Observability through existing commands.** `koto status <name>` and
  `koto workflows --children <name>` should naturally report batch
  state without new subcommands. Batch-specific reporting (e.g. "task X
  is blocked waiting on Y") is valuable but should be additive, not a
  new command surface.

- **Backward compatibility for old state files and templates.**
  Pre-batch templates must compile and run unchanged. Pre-batch state
  files must read unchanged. The migration story for adding batch
  support to existing templates should be additive (a new optional
  field), not a format bump unless the forward-compat diagnosability
  problem forces one.

## Decisions Already Made

These were settled during exploration and should be treated as
constraints, not reopened.

- **Primary model: Reading A (flat declarative batch with `waits_on`).**
  The parent owns a task list with sibling-level dependencies. Reading B
  (nested via `koto init --parent`) remains unchanged from v0.7.0 for
  genuinely hierarchical work. The GH-issue use case requires sibling
  ordering, which nesting cannot express; Reading A is the answer to
  #129.

- **Storage strategy: full derivation from on-disk child state + event
  log.** The parent persists nothing new. The batch definition lives
  in the existing `EvidenceSubmitted` event; spawn records are child
  state files discovered via `backend.list()` filtered by
  `parent_workflow`. Idempotency is the existing `backend.exists()`
  check.

- **Insertion point: CLI-level scheduler tick in `handle_next`,
  post-`advance_until_stop`.** The advance loop in
  `src/engine/advance.rs` stays pure (I/O-free, closure-driven). A new
  module `src/engine/batch.rs` holds the scheduler; `handle_next` calls
  it once the advance loop has settled on the final state.

- **Child naming: deterministic `<parent>.<task>`.** Couples child name
  to parent name; parents can't be renamed, so the coupling is
  acceptable. Gives free idempotency via `backend.exists()`.

- **Default failure policy: skip-dependents, per-batch configurable.**
  Alternatives (`fail-fast`, `continue-independent`, `pause-on-failure`)
  are opt-ins declared in the submitted evidence's `failure_policy`
  field. No global config, no per-task `trigger_rule` in v1.

- **CLI extension: `--with-data @file.json` prefix.** Mirrors
  `curl -d @file` and `gh api -f`. Size cap (1 MB) applies to resolved
  content.

- **Template extension: new `json` field type in accepts schema.**
  Extends the `VALID_FIELD_TYPES` allow-list to permit array/object
  evidence. Unlocks structured evidence beyond batch spawn.

- **Template declaration: state-level hook pointing at an accepts
  field.** The exact name (`batch`, `materialize_children`,
  `batch_spawn`) is a surface detail to be picked in this design. What's
  settled: the declaration lives on `TemplateState`, references an
  accepts field, and is validated at compile time.

- **Per-task `trigger_rule` vocabulary deferred.** The simpler per-batch
  `failure_policy` ships first. Airflow-style per-task rules can be
  added later if real use cases need the granularity.

- **Adversarial demand-validation lead was skipped.** Issue #129 has
  a known blocked consumer (shirabe PR #67) and a clear acceptance
  criteria list. Demand is self-evident.
