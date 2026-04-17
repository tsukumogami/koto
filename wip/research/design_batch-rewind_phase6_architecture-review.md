# Architecture Review: DESIGN-batch-rewind

## 1. Is the architecture clear enough to implement?

**Yes.** The design is precise about what changes and where. The two modification sites are well-identified:

- `handle_rewind` in `src/cli/mod.rs` (lines 1299-1373): currently a pure log-append operation with no side-effect awareness. The design adds a conditional cleanup block after the `Rewound` event append.
- `augment_snapshots_with_child_completed` in `src/cli/batch.rs` (lines 1422-1464): currently iterates all `ChildCompleted` events unconditionally. The design adds a seq-based epoch filter.

The data flow diagram (lines 147-168) maps cleanly to the existing code paths.

### Minor gap: template loading in handle_rewind

The design says "reads the compiled template from the state file header's `template_path`" but doesn't specify the exact loading sequence. Looking at the existing pattern in `handle_cancel` (lines 3501-3514), the implementer needs to:
1. Call `derive_machine_state(&header, &events)` to get the `template_path`.
2. Read and deserialize the compiled template.
3. Look up `template.states[from_state].materialize_children`.

This is straightforward to infer from the `handle_cancel` precedent, but worth calling out: the design says "reads the compiled template from the state file header's `template_path`" when it's actually from the `WorkflowInitialized` event payload (via `derive_machine_state`). The header doesn't carry the template path -- it carries the hash.

## 2. Are there missing components or interfaces?

**No missing components.** The design correctly identifies that all infrastructure exists:

- `backend.cleanup()` -- confirmed in `src/session/local.rs:76` (local) and `src/session/cloud.rs:671` (cloud, chains to `sync_delete_session`).
- `backend.list()` -- returns `Vec<SessionInfo>` with `parent_workflow` field for filtering.
- `Event.seq` -- confirmed at `src/engine/types.rs:374`, monotonic u64.
- `materialize_children` on `CompiledState` -- confirmed at `src/template/types.rs:74`.
- `derive_machine_state` in `src/engine/persistence.rs:395` -- already used in `handle_cancel`.

### One interface detail to verify during implementation

The design says `augment_snapshots_with_child_completed` gains an `epoch_boundary_seq: Option<u64>` parameter. This function is called from `snapshot_existing_children` and the parallel path in `build_children_complete_output`. Both callers need updating. The design mentions both (lines 133-136) but the implementer should trace all call sites to avoid a missed caller.

Callers of `augment_snapshots_with_child_completed` found:
- `src/cli/batch.rs` internal calls (scheduler snapshot path)
- `build_children_complete_output` at line 2149

Both are in the same file, so the blast radius is contained.

## 3. Are the implementation phases correctly sequenced?

**Yes, with one observation.**

- Phase 1 (batch-aware rewind) is the right starting point -- it's the core behavioral change.
- Phase 2 (epoch filter) must land before or with Phase 1 for correctness. The design acknowledges the race window: a child completing between `Rewound` event append and cleanup writes a `ChildCompleted` that poisons the next epoch. Phase 1 without Phase 2 leaves this race open.

**Recommendation:** Phases 1 and 2 should ship in the same PR. The design's "Option A" analysis (lines 75-76) explicitly calls out that cleanup-only (Phase 1 alone) leaves "silent data corruption" in the race window. Shipping them separately, even briefly, creates a correctness gap.

Phase 3 (cloud verification + docs) is correctly deferred -- it's validation of existing behavior plus documentation.

## 4. Are there simpler alternatives overlooked?

**No.** The design evaluates the right alternatives:

- **Cleanup-only (no epoch filter):** Rejected correctly -- race condition is real.
- **Idempotent re-submission:** Rejected correctly -- epoch tagging throughout the event model is disproportionate.
- **Full hybrid:** Rejected correctly -- unnecessary complexity when cleanup removes stale children.

The chosen approach is the minimal correct fix. It uses existing `backend.cleanup()` infrastructure and adds a small, well-scoped filter function.

### One simplification worth considering

The `last_rewind_seq` helper scans the full event list for the last `Rewound` event. Since `derive_state_from_log` already processes events to determine current state (and already handles `Rewound` events), there may be an opportunity to compute the rewind boundary seq as a side effect of state derivation rather than a separate scan. However, this is a minor optimization -- the event list is small, and a separate helper is clearer. Not a blocker.

## Structural Fit Assessment

The design fits the existing architecture well:

1. **No dispatch bypass.** The cleanup goes through `backend.cleanup()`, the standard session backend interface. No inline file/S3 operations.
2. **No new types or schema changes.** The `Event` and `ChildCompleted` structures are unchanged. The epoch filter uses existing `seq` field.
3. **No new CLI surface.** The rewind command gains behavior, not flags. The output gains a `children_cleaned` count field, which is additive.
4. **Follows established patterns.** Template loading in `handle_rewind` mirrors `handle_cancel`. Child enumeration via `backend.list()` mirrors `query_children`.
5. **Dependencies flow correctly.** `cli` -> `engine` -> `session` direction is preserved. No new cross-package imports.

## Findings Summary

### Blocking

None.

### Advisory

1. **Phases 1 and 2 should ship together.** The race condition the epoch filter closes is real (design lines 75-76 call it "silent data corruption"). Shipping Phase 1 alone, even in a dev branch, creates a window where the race can trigger.

2. **Template path source mismatch in prose.** The design says "state file header's `template_path`" but the path comes from the `WorkflowInitialized` event payload via `derive_machine_state`, not the header. The header carries `template_hash`. This won't confuse an implementer who reads `handle_cancel`, but the prose should be precise.

3. **`handle_rewind` currently calls `query_children` after appending the Rewound event (line 1362).** After the design's cleanup block runs, `query_children` would return an empty list (all children cleaned up). The response JSON at line 1366 includes `"children": children` -- this would show zero children after a batch rewind. The design's Phase 3 mentions adding `children_cleaned: N` to the response, but doesn't mention that the existing `children` field will go empty. Worth documenting the response shape change explicitly.
