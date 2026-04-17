---
status: Proposed
problem: |
  koto rewind rolls back the parent's event log but does not touch child sessions
  spawned as a side effect of batch evidence submission. After rewinding past a
  materialize_children state, stale children remain on disk and in cloud sync; the
  next tasks submission appends to the stale batch rather than replacing it, leaving
  the orchestration in an inconsistent state that cannot be recovered without a full
  cancel+cleanup cycle.
decision: |
  When handle_rewind detects that the rewound-from state has a materialize_children
  hook, it cleans up all child sessions via backend.cleanup() (which chains to S3
  deletion under CloudBackend). A seq-based epoch filter in
  augment_snapshots_with_child_completed ignores ChildCompleted events written before
  the rewind boundary, closing the race where a child completes between the Rewound
  event append and the cleanup loop.
rationale: |
  Cleanup-on-rewind is the minimal correct fix: it removes stale children from disk
  and cloud in one operation using existing backend.cleanup() infrastructure, and the
  epoch filter closes the only remaining race at two call sites with no schema changes.
  The alternative of making re-submission idempotent (reconciling old children with a
  new task list) was rejected because the late-arriving ChildCompleted race is a
  fundamental correctness problem that requires schema changes and epoch tagging
  throughout the event model — disproportionate complexity for a "start over" operation.
---

# DESIGN: Batch-Aware Rewind

## Status

Proposed

## Context and Problem Statement

`koto rewind` is a simple log-truncation operation: it appends an `EpochBoundary` event that causes state derivation to ignore events after the rewind point. It has no concept of side effects. The batch scheduler, introduced in 0.8.0, creates real child sessions on disk (and pushes them to cloud under `CloudBackend`) as a side effect of processing a `tasks`-typed evidence submission. These sessions survive rewind because rewind only touches the parent's event log.

After rewinding past a `materialize_children` state:
- `koto status` still shows the stale child workflows.
- The scheduler's `extract_tasks` reads the latest `EvidenceSubmitted` event. If that event is from a prior epoch (before the rewind point), the stale task list is still "latest" and the scheduler appends new children to the existing batch rather than replacing it.
- `ChildCompleted` events (from #134) written by children to the parent's log may reference tasks that no longer belong to the current epoch.
- Under `CloudBackend`, stale child sessions may have been pushed to S3 and would need reconciliation after a local rewind.

The workaround today is `koto cancel --cleanup` followed by `koto init` to start fresh, discarding all orchestration state. This is destructive — it loses any progress made before the bad submission.

## Decision Drivers

- **Correctness**: After rewind, the batch state must be consistent. No stale children should appear in the gate, scheduler, or status output.
- **Cloud parity**: Local state changes from rewind must propagate correctly when consumed through the cloud backend. Stale child sessions on S3 must not leak into the gate's view.
- **Minimal blast radius**: Non-batch rewind must remain unchanged — a simple, fast log operation.
- **Recoverability**: The design should preserve as much prior progress as possible. A rewind past a bad task submission should not force loss of all orchestration history.
- **Simplicity**: The rewind operation should remain understandable to users. Side-effect cleanup should be predictable and well-documented.

## Considered Options

### Decision 1: Local Batch Cleanup on Rewind

What happens to child sessions, stale task-list evidence, and ChildCompleted events when a batch parent rewinds past a `materialize_children` state?

Key assumptions:
- Children do not write to the parent log except via `ChildCompleted`.
- `backend.cleanup()` is synchronous and removes the state file before returning, preventing further child events.
- The rewind target is always one state back (current `handle_rewind` behavior).
- Reused task names across rewind boundaries are a supported use case.

#### Chosen: Clean children on rewind + epoch-filter ChildCompleted

When `handle_rewind` detects that the rewound-from state has a `materialize_children` hook, it lists all children of the parent and calls `backend.cleanup()` for each. This gives the next submission a clean slate.

On top of cleanup, a seq-based epoch filter is added to `augment_snapshots_with_child_completed` (and the parallel batch-view logic). The filter finds the seq of the last `Rewound` event and ignores any `ChildCompleted` event with seq <= that boundary. This closes the narrow race where a child completes between the `Rewound` event append and the cleanup loop, writing a `ChildCompleted` with a post-rewind seq number.

No schema changes to `ChildCompleted`. No changes to `extract_tasks` — after cleanup, stale evidence is harmless because the scheduler finds no on-disk children to match against.

#### Alternatives Considered

**Option A: Clean children only (no epoch filter).** Nearly correct, but leaves a real race condition: a child completing between the `Rewound` event append and the cleanup loop writes a `ChildCompleted` with a valid post-rewind seq. If the new task list reuses that name, the scheduler incorrectly treats it as terminal. The race window is milliseconds, but the consequence is silent data corruption.

**Option B: No cleanup; make re-submission idempotent.** The scheduler reconciles old children with the new task list. Rejected because the late-arriving `ChildCompleted` race is a fundamental correctness problem: a running child from the old batch has no knowledge of the parent's rewind, so when it terminates it poisons the new epoch's event stream. Fixing this requires epoch tagging on every `ChildCompleted` event and filtering in every consumer — disproportionate complexity for what is a "start over" operation.

**Option C: Full hybrid (A + B).** Union of all changes from cleanup and reconciliation. Strongest correctness, but the epoch-aware `extract_tasks` and full reconciliation logic are unnecessary when cleanup removes stale children. Implementation risk is high — the filtering touches hot paths in the scheduler and gate evaluator.

### Decision 2: Cloud Propagation of Batch Rewind

How does the cloud backend handle locally-rewound batch state, given that child sessions may have been pushed to S3 before the local rewind?

Key assumptions:
- `CloudBackend::cleanup()` chains local deletion with `sync_delete_session()` to remove S3 objects.
- `sync_delete_session`'s best-effort semantics (warn-and-continue on S3 errors) are acceptable, matching the existing cleanup contract.
- The v1 limitation where `CloudBackend::list()` produces placeholder entries with empty `parent_workflow` for S3-only sessions provides accidental safety — stale remote-only children are invisible to the gate.

#### Chosen: Immediate S3 cleanup via existing `backend.cleanup()`

Since Decision 1 calls `backend.cleanup()` for each stale child, cloud propagation happens automatically with zero additional code. `CloudBackend::cleanup()` already chains local deletion with `sync_delete_session()`. The parent's `Rewound` event is pushed to S3 via `append_event` → `sync_push_state` before child cleanup begins, preserving the "push parent before child mutation" ordering from the batch child-spawning design.

For the network-failure case: if S3 is unreachable during rewind, children are removed locally but persist on S3. The parent's `Rewound` event is committed locally and will be pushed on the next successful sync. Stale children on S3 are filtered from the gate by the v1 `parent_workflow: None` limitation. For explicit recovery, `koto session resolve --children` already handles per-child reconciliation.

#### Alternatives Considered

**Option B: Lazy reconciliation via `session resolve --children`.** Don't touch S3 during rewind; rely on explicit resolve. Rejected because it creates a mandatory manual step after every cloud-backend rewind and relies on a known v1 bug for correctness — when `CloudBackend::list()` is fixed to download remote headers, stale children would leak into the gate.

**Option C: Epoch-tagged sessions.** Tag child sessions with the parent's epoch; the gate ignores mismatches. Rejected as over-engineered: introduces epoch counters across the event model, header format, and gate evaluator, and still needs a GC mechanism for S3 storage cleanup. `backend.cleanup()` achieves the same correctness with less code.

**Option D: Push parent rewind + deferred child GC.** Push the parent's `Rewound` event to S3, then filter stale children at gate query time using epoch awareness. Rejected because it adds complexity to `build_children_complete_output` (already ~200 lines) when the storage-layer cleanup from Option A is simpler and already tested.

## Decision Outcome

The two decisions compose cleanly: Decision 1 handles local semantics (cleanup children + epoch-filter ChildCompleted), and Decision 2 confirms that `backend.cleanup()` propagates the local changes to S3 automatically.

After rewind past a `materialize_children` state:
1. `handle_rewind` appends the `Rewound` event (pushed to S3 under cloud backend).
2. `handle_rewind` loads the compiled template, checks if the rewound-from state has a `materialize_children` hook.
3. If yes: lists all children of the parent via `backend.list()`, calls `backend.cleanup()` for each.
4. The epoch filter in `augment_snapshots_with_child_completed` ignores any `ChildCompleted` event with seq <= the `Rewound` event's seq.
5. The next `koto next <parent>` call starts with a blank batch — no on-disk children, no stale evidence in scope.

Non-batch rewind is unchanged: step 2 finds no hook, steps 3-4 are skipped.

## Solution Architecture

### Overview

The fix is localized to two files: `src/cli/mod.rs` (rewind handler) and `src/cli/batch.rs` (epoch filter). No new types, no schema changes, no new CLI flags.

### Components

**`handle_rewind` extension** (`src/cli/mod.rs`). After appending the `Rewound` event and before printing the response, the handler:
1. Reads the compiled template path from `derive_machine_state` (which extracts it from the `WorkflowInitialized` event payload).
2. Checks if the `from` state (the state being rewound from) has a `materialize_children` hook.
3. If yes: calls `backend.list()`, filters to children whose `parent_workflow` matches the parent's name, and calls `backend.cleanup(child_id)` for each.

The template is already loaded elsewhere in the codebase (`handle_cancel`, `handle_next`); the pattern is established. The `from` state is available in the `Rewound` event payload.

**Epoch-boundary seq helper** (`src/cli/batch.rs`). A small function `last_rewind_seq(events: &[Event]) -> Option<u64>` that scans the event list for the last `Rewound` event and returns its `seq` field. Returns `None` if no rewind has occurred.

**`augment_snapshots_with_child_completed` filter** (`src/cli/batch.rs`). When iterating `ChildCompleted` events, skip any with `event.seq <= boundary_seq` (where `boundary_seq` comes from the helper above). Applied in two locations:
1. `augment_snapshots_with_child_completed` (used by the scheduler's snapshot path).
2. The parallel block in `build_children_complete_output` (used by the gate output builder).

### Key Interfaces

No new public interfaces. The changes are internal to existing functions:
- `handle_rewind(backend, name)` gains batch-awareness via a conditional block.
- `augment_snapshots_with_child_completed` gains an `epoch_boundary_seq: Option<u64>` parameter.
- `build_children_complete_output` calls `last_rewind_seq` and passes it through.

### Data Flow

```
koto rewind <parent>
  │
  ├─ append Rewound event to parent log
  │   └─ (cloud: push parent log to S3)
  │
  ├─ load compiled template from state header
  │
  ├─ check: does `from` state have materialize_children?
  │   ├─ NO  → done (non-batch rewind, unchanged)
  │   └─ YES → list children, cleanup each
  │            └─ (cloud: delete each child from S3)
  │
  └─ print rewind response (with children_cleaned count)

koto next <parent>  (after rewind)
  │
  ├─ extract_tasks: no EvidenceSubmitted in current epoch → empty task list
  ├─ augment_snapshots: epoch filter skips stale ChildCompleted events
  ├─ scheduler: no tasks, no children → NoBatch
  └─ agent submits new tasks → fresh batch starts from zero
```

## Implementation Approach

### Phase 1: Batch-aware rewind + epoch filter

Phases 1 and 2 must ship as a single commit. The design identifies the Phase-1-only race (a child completing between the `Rewound` event and the cleanup loop) as "silent data corruption" — shipping cleanup without the epoch filter creates a correctness gap, even if it's narrow.

Modify `handle_rewind` in `src/cli/mod.rs` to detect `materialize_children` on the rewound-from state and cleanup children. Simultaneously add the `last_rewind_seq` helper and epoch filter to both ChildCompleted consumers.

Note: after batch cleanup, the existing `query_children` call in `handle_rewind` returns an empty array. The `children` field in the rewind response changes from populated to empty after a batch rewind — this is correct behavior (stale children are gone) but agents parsing the response should not be surprised.

Deliverables:
- `src/cli/mod.rs`: conditional cleanup block in `handle_rewind`
- `src/cli/batch.rs`: `last_rewind_seq` helper, epoch filter in `augment_snapshots_with_child_completed` and `build_children_complete_output`
- Integration tests: rewind past batch state, verify children are gone, verify re-init succeeds
- Unit tests for the epoch filter (events before/after boundary)

### Phase 2: Cloud + documentation

Verify cloud propagation works end-to-end (cleanup already chains to S3). Update the rewind response to include `children_cleaned: N` so agents know batch cleanup occurred. Update `koto-user` skill docs.

Deliverables:
- `tests/`: cloud integration test (if cloud test infra supports it) or manual verification
- `plugins/koto-skills/skills/koto-user/references/command-reference.md`: document rewind behavior for batch parents
- `plugins/koto-skills/skills/koto-user/references/batch-workflows.md`: document rewind as a recovery path

## Security Considerations

This design modifies file-system and S3 operations during rewind but does not introduce new attack surface:
- **No external inputs**: Rewind operates on the parent's own event log and enumerates children from `backend.list()`, which reads only from the sessions directory controlled by koto.
- **No permission escalation**: `backend.cleanup()` removes session directories that koto created; no new filesystem permissions required.
- **No data exposure**: Child sessions contain workflow state, not credentials. Cleanup deletes them.
- **S3 deletion**: `sync_delete_session` uses the same authenticated S3 client as existing cleanup paths; no new credentials or scopes.

No new attack surface. The S3 data-retention failure mode (stale children persist on S3 after a network failure during rewind) is documented in Consequences; it is a correctness concern, not a security one, and `koto session resolve --children` provides explicit recovery.

## Consequences

### Positive
- Rewind past a `materialize_children` state produces a clean slate. Agents can fix a bad task submission without `cancel --cleanup` + `koto init`.
- Cloud state stays consistent: stale children are deleted from S3 in the same operation.
- Non-batch rewind is unchanged — the conditional block is skipped when no `materialize_children` hook is present.

### Negative
- Progress in completed children is lost on rewind. If a batch had 50 tasks and 48 completed, rewinding to fix the task list forces all 50 to re-run.
- The `sync_delete_session` best-effort semantics mean S3 children may persist after a network failure during rewind.

### Mitigations
- Progress loss is inherent to rewind semantics ("go back and start over"). The workaround (`cancel --cleanup`) already loses all progress. This design loses less (parent history is preserved).
- For the S3 failure case, `koto session resolve --children` provides explicit recovery. The epoch filter prevents stale ChildCompleted events from affecting correctness regardless of S3 state.
