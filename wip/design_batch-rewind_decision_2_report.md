# Decision 2: Cloud Propagation of Batch Rewind

## Question

How does the cloud backend handle locally-rewound batch state, given that child sessions may have been pushed to S3 before the local rewind?

## Options Evaluated

### Option A: Immediate S3 Cleanup During Rewind

When the local rewind cleans up stale child sessions, `CloudBackend::cleanup()` already deletes the child's S3 prefix via `sync_delete_session()`. If Decision 1 chooses to call `backend.cleanup(child_id)` for each stale child during rewind, the cloud leg fires automatically -- no new code needed for the S3 deletion itself.

**Correctness**: Strong. Stale children are removed from both local and remote storage in the same operation. The gate's `build_children_complete_output` calls `backend.list()`, which merges S3 sessions into the result set. Deleting from S3 prevents remote-only ghosts from appearing in the gate's child enumeration.

**Network-failure resilience**: Weak. `sync_delete_session` is best-effort -- S3 errors are logged to stderr and swallowed. If the network is down during rewind, children are removed locally but persist on S3. The next `backend.list()` call will surface those S3-only children as placeholder `SessionInfo` entries with empty `parent_workflow`, which currently causes them to be filtered out of the gate (the `apply_children_policy` comment at line 127-137 of `session.rs` documents this v1 limitation). However, `s3_list_sessions` still returns them in the raw session list, creating a visible inconsistency in `koto session list` output even if the gate accidentally ignores them.

**Complexity**: Low. `CloudBackend::cleanup()` already chains `local.cleanup()` + `sync_delete_session()`. Decision 1's local cleanup path gets the cloud leg for free.

**Interaction with existing sync model**: Good. The parent's `Rewound` event is appended via `append_event`, which calls `sync_push_state` to push the updated parent log. Child cleanup follows. The ordering is: parent log updated on S3 first, then children deleted -- consistent with the "push parent before child mutation" principle from Decision 12 Q6.

**Code changes**: Minimal. If Decision 1 calls `backend.cleanup()` per child, no additional cloud-specific code is needed. The only gap is the silent failure mode on network errors.

### Option B: Lazy Reconciliation via `session resolve --children`

Don't touch S3 during rewind. Stale child sessions remain on S3 until the next `koto session resolve --children` call reconciles them.

**Correctness**: Eventually correct, but has a window of inconsistency. Between rewind and the next resolve, `CloudBackend::list()` merges S3-only children into the session list. These stale children have `parent_workflow: None` (placeholder entries from line 688 of `cloud.rs`), so `build_children_complete_output` accidentally filters them out of the gate. But this is a known v1 bug, not a guarantee -- if a future revision downloads remote headers (as the comment at line 127 of `session.rs` suggests), those stale children would suddenly appear in the gate with valid `parent_workflow` fields, breaking the gate's view.

**Network-failure resilience**: Strong by design. No network call during rewind means no network failure can corrupt the rewind operation. Reconciliation happens later when the network is available.

**Complexity**: Medium. Requires documenting the reconciliation workflow and potentially adding a `--rewind-cleanup` flag or automatic stale-child detection to `session resolve`. Users must know to run resolve after a rewind under cloud backend.

**Interaction with existing sync model**: Awkward. The parent's log on S3 reflects the rewind (via the `Rewound` event pushed by `append_event`), but child sessions on S3 still reference the pre-rewind epoch. Any remote reader that doesn't run through the resolve path sees an inconsistent parent-child relationship.

**Code changes**: A new reconciliation mode or flag on `session resolve` to detect and clean up children that belong to a rewound epoch. The `reconcile_child` strict-prefix classifier doesn't currently understand epoch semantics, so this needs new logic.

### Option C: Epoch-Tagged Sessions

Tag each child session's `StateFileHeader` (or a new metadata field) with the parent's epoch counter at spawn time. The gate's `build_children_complete_output` checks the child's epoch tag against the parent's current epoch and ignores mismatches.

**Correctness**: Strong in theory. The gate filters at read time rather than relying on cleanup, so stale S3 children are invisible regardless of whether they've been deleted. This is a "logical delete" approach.

**Network-failure resilience**: Strong. No network calls are needed for the filtering to work -- epoch comparison is purely a function of the data already present in the event logs.

**Complexity**: High. Requires:
1. Adding an epoch counter to the parent's event model (today, rewind appends `Rewound { from, to }` but doesn't carry an explicit epoch number).
2. Storing the parent epoch in each child's header or init event at spawn time.
3. Modifying `build_children_complete_output` to compare epochs.
4. Modifying `CloudBackend::list()` to either download child headers from S3 (to read the epoch tag) or accept that undownloaded children are excluded from the gate.

**Interaction with existing sync model**: Orthogonal. Epoch tagging doesn't conflict with the sync model but also doesn't integrate with it -- stale children still consume S3 storage indefinitely unless a separate garbage collection mechanism removes them.

**Code changes**: Significant. New fields on `StateFileHeader` or `WorkflowInitialized` event, epoch derivation logic in the parent's event replay, epoch filtering in the gate and batch view, and a GC mechanism or manual cleanup command for S3.

### Option D: Push Parent Rewind + Deferred Child GC

A hybrid: during rewind, push only the parent's updated log to S3 (this already happens via `append_event` -> `sync_push_state`). Don't attempt to delete children from S3 during rewind. Instead, add a lightweight GC pass to `build_children_complete_output` that identifies children whose parent log shows a `Rewound` event past their creation point and excludes them from the gate output.

**Correctness**: Good. The gate's live classification already reads the parent's events and enumerates children. Adding epoch-awareness to the classification (using the existing `Rewound` events rather than a new epoch counter) filters stale children without requiring explicit deletion.

**Network-failure resilience**: Good. The parent push is best-effort (same as today). If it fails, local state is still correct. Remote consumers see the old parent log and the old children -- consistent with each other, just stale as a unit.

**Complexity**: Medium. The gate already iterates parent events to find `EvidenceSubmitted` entries. Adding a check for `Rewound` events that invalidate prior children is incremental.

**Interaction with existing sync model**: Good. Doesn't change the sync push/pull model. The gate becomes smarter about what it considers valid, rather than relying on storage-level cleanup.

**Code changes**: Modify `build_children_complete_output` to track which children were spawned before the most recent `Rewound` event that crossed their spawn state. Modify the batch view to use the same filter. Add a `koto session gc` or extend `session cleanup` to prune S3 children that are logically dead (optional, for storage hygiene).

## Recommendation

**Option A: Immediate S3 Cleanup During Rewind**, with a documented fallback to `session resolve --children` for the network-failure case.

Rationale:

1. **Simplest correct path.** `CloudBackend::cleanup()` already handles both local and S3 deletion. If Decision 1 calls `backend.cleanup()` for each stale child, the cloud propagation is automatic with zero additional code.

2. **Consistent with existing patterns.** `handle_cleanup` in `session.rs` and `handle_cancel` with `cleanup: true` both call `backend.cleanup()` which chains through to S3 deletion. Rewind follows the same pattern rather than inventing a new propagation mechanism.

3. **The network-failure gap is narrow and recoverable.** If S3 is unreachable during rewind, the parent's `Rewound` event is committed locally and will be pushed on the next successful sync. The stale children on S3 are filtered from the gate by the v1 `parent_workflow: None` limitation (they appear as placeholders with no parent link). For explicit cleanup, `koto session resolve --children` already handles per-child reconciliation. The "local exists, remote exists" case for a locally-deleted child maps to `StrictPrefixOutcome::OneSideMissing` which resolves correctly.

4. **Avoids new abstractions.** Options C and D introduce epoch tracking or gate-level filtering that adds complexity across the template compiler, event model, and gate evaluator. Option A reuses existing infrastructure.

5. **"Push parent before child mutation" ordering is preserved.** The parent's `Rewound` event is appended and pushed (via `append_event` -> `sync_push_state`) before child cleanup begins. This matches the Decision 12 Q6 ordering guarantee.

## Assumptions

- Decision 1 will choose to clean up stale child sessions locally during rewind (otherwise this decision's scope changes from "how to propagate cleanup" to "whether to propagate cleanup").
- The v1 limitation where `CloudBackend::list()` produces placeholder entries with empty `parent_workflow` for S3-only sessions will persist in the near term. This means S3-only stale children are accidentally invisible to the gate, which is safe-by-accident but should not be relied on for correctness.
- `sync_delete_session`'s best-effort semantics (warn-and-continue on S3 errors) are acceptable for child cleanup during rewind, matching the existing `cleanup()` contract.
- Network partitions during rewind are rare in practice. The expected use case is interactive (developer rewinding a workflow), not automated (batch orchestrator).

## Rejected Alternatives

**Option B (Lazy Reconciliation)**: Creates a mandatory manual step after every cloud-backend rewind. Users must know to run `session resolve --children`, which is an operational burden for what should be a single atomic operation. The window of inconsistency (stale children visible on S3 between rewind and resolve) is acceptable today only because of a known bug in the list() implementation. When that bug is fixed, stale children would leak into the gate.

**Option C (Epoch-Tagged Sessions)**: Over-engineered for the problem. Introduces a new concept (epoch counters) across the event model, header format, gate evaluator, and batch view. The storage leak problem (stale children consuming S3 space indefinitely) requires a separate GC mechanism anyway, so epoch tagging doesn't eliminate the need for cleanup -- it only defers it. The complexity cost is disproportionate to the benefit.

**Option D (Push Parent + Deferred GC)**: A reasonable middle ground, but the gate-level filtering adds complexity to `build_children_complete_output` (already one of the longest functions in the codebase at ~200 lines). Option A achieves the same correctness with less code by handling cleanup at the storage layer rather than the query layer. Option D would be the right choice if cleanup were destructive or risky, but `backend.cleanup()` is idempotent and well-tested.
