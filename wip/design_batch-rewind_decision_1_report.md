# Decision 1: Local Batch Cleanup on Rewind

## Question

When a batch parent rewinds past a materialize_children state, what happens to the child sessions, the stale task-list evidence, and any ChildCompleted events written by those children?

## Options Evaluated

### Option A: Cancel and Clean All Children on Rewind (Fresh Start)

**Description**: When `handle_rewind` detects that the rewind crosses back past a `materialize_children` state, it iterates all child sessions of the parent and calls `backend.cleanup(child)` for each. The next `koto next` call on the rewound state starts with a blank batch -- no on-disk children, no stale evidence in scope.

**Correctness**: Strong. Stale children are gone from disk, so `snapshot_existing_children` returns nothing. `ChildCompleted` events from the old epoch are still in the parent's log, but `extract_tasks` returns `None` after rewind (no `EvidenceSubmitted` for the rewound-to state yet), so `augment_snapshots_with_child_completed` has no `name_to_task` map to match against -- stale events are effectively invisible. Once the agent submits a fresh task list, the scheduler starts from zero. No race with late-arriving `ChildCompleted` writes because the child sessions no longer exist on disk (cleanup removes the state file, so the child's `handle_next` cannot append to a non-existent session).

**Interaction with ChildCompleted events**: Stale `ChildCompleted` events remain in the parent log but are harmless: they only matter when `name_to_task` includes a matching task name (filtered by `augment_snapshots_with_child_completed`). If the new submission reuses task names, the stale event could match. However, on-disk snapshots take precedence over events, so newly spawned children override any stale event data. The only window for confusion is between new evidence submission and first scheduler tick for a reused task name, during which the stale `ChildCompleted` could project a false terminal status. This is a narrow window and can be eliminated by filtering `ChildCompleted` events to only those with seq > the epoch boundary (the most recent `Rewound` event's seq).

**Complexity**: Low. Requires changes to `handle_rewind` only: detect whether the rewound-from state has a `materialize_children` hook, list children, cleanup each. The template must be loaded to check the hook, which means reading the compiled template from the `WorkflowInitialized` event's path -- already done elsewhere in the codebase.

**Risk**: Moderate. Progress in completed children is lost. If a batch had 50 tasks and 48 completed, rewinding to fix the task list definition forces all 50 to re-run. For CloudBackend, `cleanup` already handles S3 deletion, so cloud parity is maintained.

**Code changes needed**:
1. `src/cli/mod.rs` -- `handle_rewind`: after appending the `Rewound` event, load the compiled template, check if the `from` state has `materialize_children`, and if so, list+cleanup all children.
2. Optionally: add epoch-aware filtering to `augment_snapshots_with_child_completed` so stale `ChildCompleted` events from before the rewind boundary are skipped. This is defense-in-depth against reused task names.

### Option B: No Child Cleanup; Make Re-Submission Idempotent (Reconcile)

**Description**: Rewind stays a pure log operation. Instead, the scheduler is made idempotent: when a new `EvidenceSubmitted` event for the batch state arrives, `extract_tasks` reads it (latest wins). The scheduler compares the new task list against on-disk children and reconciles: children matching the new list are kept, children not in the new list are flagged as orphans (or cleaned up), and new tasks are spawned.

**Correctness**: Conditional. If the new task list is identical to the old one, existing children are reused -- good for the "rewind to re-run the gate" case. If the task list changed, orphan detection must correctly identify and remove stale children. `ChildCompleted` events from old children whose task names still appear in the new list could incorrectly mark freshly needed tasks as terminal. The reconciliation logic must distinguish "same task name, old child" from "same task name, new child" -- currently there's no epoch marker on `ChildCompleted` events to support this.

**Interaction with ChildCompleted events**: Dangerous. A child that was running from the old batch and completes AFTER the rewind writes a new `ChildCompleted` event with a fresh seq number above the rewind boundary. This event appears valid in the new epoch. If the new task list includes the same task name, the scheduler sees it as terminal (via `augment_snapshots_with_child_completed`) and skips spawning a new child. This is a silent correctness bug that is hard to detect and hard to reproduce (requires a timing race between rewind and child completion).

**Complexity**: High. Requires:
- Epoch-aware `extract_tasks` (filter events to current epoch only).
- Orphan detection and cleanup in the scheduler (partially exists as `orphan_candidates` but not actionable cleanup).
- Epoch tagging on `ChildCompleted` events, or a different mechanism to invalidate stale completion signals.
- Spawn-entry comparison (R8) to decide whether an existing child matches the new task spec or needs respawning.

**Risk**: High. The late-arriving `ChildCompleted` race is the critical flaw. A running child from the old batch has no knowledge of the parent's rewind. When it reaches terminal and writes `ChildCompleted` to the parent's log, it poisons the new epoch. Fixing this requires either (a) preventing children from writing to a parent that has rewound past their spawn epoch, or (b) epoch-tagging every `ChildCompleted` event and filtering at read time. Both add significant complexity.

**Code changes needed**:
1. `src/cli/batch.rs` -- `extract_tasks`: add epoch-boundary awareness.
2. `src/cli/batch.rs` -- `augment_snapshots_with_child_completed`: add epoch filtering.
3. `src/engine/types.rs` -- `ChildCompleted`: add `epoch` or `parent_seq_at_spawn` field.
4. `src/cli/mod.rs` -- `emit_child_completed_to_parent`: populate epoch field.
5. `src/cli/batch.rs` -- `run_batch_scheduler`: reconcile orphaned children.
6. Scheduler: handle the "child still running from old batch" case.

### Option C: Clean Children AND Make Re-Submission Idempotent (Defense-in-Depth)

**Description**: Combine A and B. Rewind cleans all children (Option A), AND the scheduler is made idempotent with epoch-aware filtering (Option B). This means that even if cleanup fails partially (e.g., cloud sync lag, filesystem error), the scheduler won't be confused by stale state.

**Correctness**: Strongest. Two independent correctness barriers: disk cleanup removes the primary source of stale state, and epoch filtering removes the secondary source (stale events in the log).

**Interaction with ChildCompleted events**: Fully addressed. Cleanup prevents new `ChildCompleted` writes from old children (their sessions are gone). Epoch filtering ignores any that were written before cleanup ran. Belt and suspenders.

**Complexity**: Highest. All changes from both A and B. The epoch-filtering work in B is substantial and adds ongoing maintenance burden to every code path that reads `ChildCompleted` events.

**Risk**: Low correctness risk, but high implementation risk. The epoch-filtering changes touch multiple hot paths in the scheduler and gate evaluator. Bugs in the filtering logic could cause valid `ChildCompleted` events to be dropped, breaking the happy-path batch completion flow.

**Code changes needed**: Union of A and B changes.

### Option D: Clean Children on Rewind + Epoch-Filter ChildCompleted Only (Targeted Hybrid)

**Description**: Like Option A (clean all children on rewind), but add a single targeted fix: filter `ChildCompleted` events in `augment_snapshots_with_child_completed` to ignore events with seq <= the rewind boundary. This addresses the narrow race where a child writes `ChildCompleted` between the rewind and the cleanup, without the full reconciliation machinery of Option B.

**Correctness**: Strong. Cleanup handles the primary case. The epoch filter on `ChildCompleted` handles the race window. No changes to `extract_tasks` needed because the stale `EvidenceSubmitted` is harmless after cleanup (scheduler sees no children to match).

**Interaction with ChildCompleted events**: The filter is simple: find the seq of the last `Rewound` event, skip any `ChildCompleted` with seq < that boundary. This is a read-time filter in two functions (`augment_snapshots_with_child_completed` and the parallel logic in `batch_view`). No schema changes to `ChildCompleted` itself.

**Complexity**: Low-moderate. Option A's changes plus a small filter addition in two locations.

**Risk**: Low. The filter is conservative (only ignores events from before the rewind, not all old events). The cleanup is the primary mechanism; the filter is a safety net.

**Code changes needed**:
1. Everything from Option A.
2. `src/cli/batch.rs` -- `augment_snapshots_with_child_completed`: accept an `epoch_boundary_seq` parameter, skip events with seq <= boundary.
3. `src/cli/batch.rs` -- batch view logic (line ~2277): same epoch filter.
4. Helper to derive `epoch_boundary_seq` from the event log (find last `Rewound` event's seq).

## Recommendation

**Option D: Clean Children on Rewind + Epoch-Filter ChildCompleted Only**

Option A alone is nearly sufficient but leaves a real (if narrow) race condition: a child completing between the `Rewound` event append and the cleanup loop could write a `ChildCompleted` with a post-rewind seq number. Option D closes this gap with minimal additional complexity -- a seq-based filter in two functions, no schema changes, no reconciliation machinery.

Option B is rejected because the late-arriving `ChildCompleted` race is a fundamental correctness problem that requires schema changes and epoch tagging throughout the event model. The complexity is disproportionate to the benefit, especially since "preserve child progress across rewind" is a nice-to-have, not a requirement.

Option C is rejected because it carries the full implementation cost of B for marginal benefit over D. The epoch-aware `extract_tasks` and full reconciliation logic are unnecessary when cleanup removes stale children from disk.

The key insight from the code: `extract_tasks` walks all events backward without epoch filtering, and `augment_snapshots_with_child_completed` scans all `ChildCompleted` events without epoch filtering. Both functions trust that the event log represents a single consistent timeline. Rewind breaks this assumption. Option D restores the invariant at the narrowest possible point -- only the `ChildCompleted` augmentation needs epoch awareness, because that's the only path where post-rewind writes from old children can inject stale data.

## Assumptions

1. **Children do not write to the parent log except via `ChildCompleted`**. If other cross-session writes exist, they would need similar epoch filtering.
2. **`backend.cleanup()` is synchronous and removes the child's state file before returning**. This prevents the child from appending further events after cleanup. Verified: `LocalBackend::cleanup` removes the directory, and `CloudBackend::cleanup` removes locally then attempts S3 deletion.
3. **The rewind target is always one state back** (current `handle_rewind` behavior). If rewind-to-arbitrary-state is added later, the `materialize_children` detection must check all states between the target and the current state, not just the current state.
4. **CloudBackend propagation of child cleanup is eventually consistent**. A cloud-synced child may still appear in remote listings briefly after local cleanup. This is acceptable because the parent's local state is authoritative and the epoch filter prevents stale events from affecting correctness.
5. **Reused task names across rewind boundaries are a supported use case**. The epoch filter must handle this correctly (ignore old `ChildCompleted` for a task name that appears in both old and new submissions).

## Rejected Alternatives

**Option A (Clean only, no epoch filter)**: Nearly correct but leaves the `ChildCompleted` race window open. A child that completes after the `Rewound` event is appended but before `cleanup` runs can write a `ChildCompleted` with a valid post-rewind seq number. If the new task list reuses that task name, the scheduler incorrectly treats it as already terminal. The race window is small (milliseconds in practice) but the consequence is silent data corruption -- the wrong option to leave unaddressed.

**Option B (Reconcile, no cleanup)**: The late-arriving `ChildCompleted` problem is fundamental and requires schema changes to `ChildCompleted` (adding an epoch field) plus filtering in every consumer. The reconciliation logic (matching old children to new task specs) adds substantial complexity to the scheduler hot path. The benefit -- preserving child progress -- is not a decision driver and is outweighed by the correctness risk and maintenance burden.

**Option C (Full hybrid)**: All of B's complexity for marginal benefit over D. The epoch-aware `extract_tasks` change is unnecessary when cleanup removes stale children. The full reconciliation machinery is unnecessary when rewind means "start this batch over."
