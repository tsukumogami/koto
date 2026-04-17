---
status: Current
problem: |
  koto rewind rolls back the parent's event log but does not touch child sessions
  spawned as a side effect of batch evidence submission. After rewinding past a
  materialize_children state, stale children remain on disk and in cloud sync; the
  next tasks submission appends to the stale batch rather than replacing it, leaving
  the orchestration in an inconsistent state that cannot be recovered without a full
  cancel+cleanup cycle. The work done by those children is real (commits exist in the
  repo) and agents need to inspect the koto context to understand it.
decision: |
  Rewind relocates children to a superseded branch rather than deleting them. Each
  child session is renamed from parent.task to parent~N.task (where N is the epoch
  counter), and its parent_workflow header is updated to parent~N. The current branch
  gets clean names for re-submission. Superseded branches remain fully queryable via
  standard koto commands (status, query, workflows --children parent~N).
rationale: |
  Cleanup-on-rewind destroys the only context that explains why commits in the repo
  exist. The branching model preserves history while giving the current batch a clean
  namespace. The epoch convention (tilde-N suffix) reuses existing session naming and
  requires no schema changes to the event model — only a new relocate operation on
  the session backend and epoch-aware filtering in the scheduler and gate.
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

The workaround today is `koto cancel --cleanup` followed by `koto init` to start fresh, discarding all orchestration state. This is destructive — it loses both parent history and child context. The work done by the children (code changes, test runs, decisions recorded) persists in the repo, but the koto state that explains *why* that work was done is gone.

## Decision Drivers

- **Context preservation**: The work done by children is real — commits exist in the repo. Agents need the koto state to understand what decisions were made, what evidence was submitted, and how the workflow progressed. Rewind must not destroy this context.
- **Correctness**: After rewind, the current batch must be consistent. Superseded children must not appear in the gate, scheduler, or status output for the current branch.
- **Queryability**: Superseded branches must remain inspectable through standard koto commands. An agent asking "what happened in the previous attempt?" should be able to find it.
- **Cloud parity**: Local state changes from rewind must propagate correctly when consumed through the cloud backend.
- **Minimal blast radius**: Non-batch rewind must remain unchanged — a simple, fast log operation.
- **Simplicity**: The branching model must be intuitive. Users already understand rewind as "go back." The branch metaphor adds "and the work you did is over there."

## Considered Options

### Decision 1: What happens to child sessions on rewind?

What happens to child sessions, stale task-list evidence, and ChildCompleted events when a batch parent rewinds past a `materialize_children` state?

Key assumptions:
- Children do not write to the parent log except via `ChildCompleted`.
- Reused task names across rewind boundaries are a supported use case.
- Agents need to inspect superseded children to understand prior work.
- The session naming convention `parent.task` is the only namespace collision point.

#### Chosen: Relocate children to a superseded branch (epoch-based renaming)

On rewind past a `materialize_children` state:

1. Compute the epoch counter N from the number of `Rewound` events in the parent's log (the rewind being appended is epoch N).
2. For each child session whose `parent_workflow == parent_name`:
   - Rename the session directory from `parent.task` to `parent~N.task`.
   - Update the state file header's `parent_workflow` field from `parent` to `parent~N`.
3. The current branch now has no children (they've all moved to `parent~N.*`). The scheduler starts fresh.
4. `ChildCompleted` events from the old epoch are filtered by a seq-based epoch boundary (events with seq <= the `Rewound` event's seq are ignored), closing the race where a child completes between the Rewound event and the relocate loop.

The `~N` naming convention:
- Chosen because `~` is not valid in user-provided workflow names (it's a namespace separator for internal use).
- Visually communicates "branch N of parent" without ambiguity.
- Sorts naturally in directory listings (`parent~1.task-a` before `parent~2.task-a`).

#### Alternatives Considered

**Option A: Clean children on rewind (delete).** Removes stale children from disk and cloud. Correct and simple, but destroys the koto context that explains why commits in the repo exist. An agent that needs to understand what happened in the previous batch attempt has no state to inspect. Rejected because context preservation is a primary driver.

**Option B: No cleanup; make re-submission idempotent.** The scheduler reconciles old children with the new task list. The late-arriving `ChildCompleted` race is a fundamental correctness problem that requires schema changes and epoch tagging throughout the event model. Children that were completed in the old batch but need to re-run in the new batch would appear as already-terminal, silently skipping work. Rejected for complexity and correctness risk.

**Option C: Epoch-tag children in place (no rename).** Add an epoch field to each child's header; the scheduler filters by epoch match. Avoids the rename, but causes name collisions: the new batch tries to spawn `parent.task-1` and hits the existing session. The collision guard (from #133) rejects it. To work, this approach requires the scheduler to delete-and-recreate sessions that exist from old epochs — which is cleanup under a different name, and loses the context just the same. Rejected because it doesn't solve the name collision without destroying context.

### Decision 2: Cloud propagation of batch rewind

How does the cloud backend handle locally-relocated batch children?

Key assumptions:
- `CloudBackend` stores sessions as S3 prefix `<session-name>/`.
- The existing `sync_push_state` and `sync_delete_session` APIs are available.
- Network failures during rewind must not corrupt state.

#### Chosen: Relocate on S3 via copy-then-delete, with deferred fallback

During the relocate loop, for each child:
1. **Local**: `fs::rename(old_dir, new_dir)` — atomic on the same filesystem.
2. **Cloud**: copy the child's S3 objects from `old-prefix/` to `new-prefix/`, then delete the old prefix. This is a multi-step S3 operation (list + copy + delete) and inherently non-atomic.
3. **Header update**: rewrite the state file's first line (header) with the updated `parent_workflow` field before the S3 copy, so the remote copy arrives with the correct metadata.

For network failures: if the S3 copy fails, the local rename has already succeeded. The child is locally at `parent~N.task` but remotely still at `parent.task`. This is a transient inconsistency handled by `koto session resolve --children`:
- Local has `parent~N.task` (with `parent_workflow: parent~N`).
- Remote has `parent.task` (with `parent_workflow: parent`).
- The strict-prefix classifier detects the mismatch and flags it for resolution.

The parent's `Rewound` event is pushed to S3 first (via `append_event` → `sync_push_state`), preserving the "push parent before child mutation" ordering.

#### Alternatives Considered

**Option B: Lazy reconciliation only.** Don't touch S3 during rewind; rely on `session resolve` later. Creates a mandatory manual step and leaves stale children visible on S3 in the interim. Rejected because agents on other machines would see an inconsistent state.

**Option C: Delete from S3, re-create under new name.** Equivalent to cleanup-on-S3 + create. Loses the S3 history if the copy fails midway. The copy-then-delete approach is safer because the old data persists until the copy is confirmed. Rejected for lower resilience.

## Decision Outcome

The two decisions compose into a branching model for batch rewind:

1. `handle_rewind` appends the `Rewound` event (pushed to S3 under cloud backend).
2. Computes epoch counter N from the parent's rewind history.
3. Checks if the rewound-from state has a `materialize_children` hook.
4. If yes: relocates each child from `parent.<task>` to `parent~N.<task>` (local rename + cloud copy-then-delete + header update).
5. Epoch filter in `augment_snapshots_with_child_completed` ignores `ChildCompleted` events with seq <= the Rewound event's seq.
6. The next `koto next <parent>` starts with a clean batch. The superseded children remain at `parent~N.*`, fully queryable.

Non-batch rewind is unchanged: step 3 finds no hook, steps 4-5 are skipped.

## Solution Architecture

### Overview

Three changes: a `relocate` operation on the session backend, batch-aware rewind in `handle_rewind`, and epoch filtering in the ChildCompleted consumer. Plus a query surface so agents can discover and inspect superseded branches.

### Components

**`SessionBackend::relocate(from, to)`** (`src/session/mod.rs`, `local.rs`, `cloud.rs`). New trait method that renames a session and updates its state header. Local implementation uses `fs::rename`. Cloud implementation does local rename, then S3 copy-then-delete (best-effort; failure leaves local state correct and remote recoverable via `session resolve`).

**`handle_rewind` extension** (`src/cli/mod.rs`). After appending the `Rewound` event:
1. Loads the compiled template via `derive_machine_state`.
2. Checks if the `from` state has a `materialize_children` hook.
3. If yes: computes epoch N, lists children by `parent_workflow`, calls `backend.relocate(child, new_name)` for each, updates each child's `parent_workflow` header to `parent~N`.
4. Response includes `superseded_branch: "parent~N"` and `children_relocated: count`.

**Epoch filter** (`src/cli/batch.rs`). `last_rewind_seq(events) -> Option<u64>` helper. Both `augment_snapshots_with_child_completed` and `build_children_complete_output` skip `ChildCompleted` events with seq <= the boundary.

**Query surface for superseded branches**:
- `koto status parent~N.task-1` — works out of the box (it's a normal session with a funny name).
- `koto query parent~N.task-1` — works out of the box.
- `koto workflows --children parent~N` — lists children of the superseded branch (matches `parent_workflow == parent~N`).
- `koto workflows --children parent` — lists only current-branch children (matches `parent_workflow == parent`). Superseded children are excluded because their `parent_workflow` was updated to `parent~N`.
- `koto status parent` gains a `superseded_branches` field listing `["parent~1", "parent~2", ...]` derived from the parent's `Rewound` events. This is the discovery surface — an agent sees the branches exist and can drill into any of them.

### Key Interfaces

**New trait method:**
```rust
fn relocate(&self, from: &str, to: &str) -> anyhow::Result<()>;
```
Renames a session directory and updates the state file header's `parent_workflow` (and optionally `name`) field. Returns `Err` if `from` doesn't exist or `to` already exists.

**Response additions:**
- `koto rewind` response: `superseded_branch: Option<String>`, `children_relocated: usize`.
- `koto status` response (batch parent only): `superseded_branches: Vec<String>`.

### Data Flow

```
koto rewind <parent>
  |
  +-- append Rewound event to parent log
  |     (cloud: push parent log to S3)
  |
  +-- load compiled template
  |
  +-- check: does `from` state have materialize_children?
  |     NO  -> done (non-batch rewind, unchanged)
  |     YES -> compute epoch N
  |             for each child:
  |               relocate parent.task -> parent~N.task
  |               update header: parent_workflow = parent~N
  |               (cloud: copy to new S3 prefix, delete old)
  |
  +-- print rewind response
        { superseded_branch: "parent~N", children_relocated: 3 }

koto next <parent>  (after rewind)
  |
  +-- extract_tasks: no EvidenceSubmitted in current epoch -> empty
  +-- augment_snapshots: epoch filter skips stale ChildCompleted
  +-- scheduler: no tasks, no children -> NoBatch
  +-- agent submits new tasks -> fresh batch starts from zero

koto status parent
  { ..., superseded_branches: ["parent~1"] }

koto workflows --children parent~1
  [ { name: "parent~1.task-a", state: "done", ... } ]

koto status parent~1.task-a
  { name: "parent~1.task-a", state: "done", ... }
```

## Implementation Approach

### Phase 1: Session relocate primitive

Add `SessionBackend::relocate(from, to)` to the trait with local and cloud implementations. The local side is `fs::rename` + header rewrite. The cloud side is copy-then-delete with best-effort S3 semantics.

Deliverables:
- `src/session/mod.rs`: trait method
- `src/session/local.rs`: local implementation
- `src/session/cloud.rs`: cloud implementation
- Unit tests for relocate (name change, header update, collision rejection)

### Phase 2: Batch-aware rewind + epoch filter

This must ship as a single change — the epoch filter closes the ChildCompleted race that exists without it.

Modify `handle_rewind` to detect `materialize_children`, compute epoch, relocate children. Add `last_rewind_seq` helper and epoch filter to both ChildCompleted consumers.

Deliverables:
- `src/cli/mod.rs`: conditional relocate block in `handle_rewind`
- `src/cli/batch.rs`: `last_rewind_seq` helper, epoch filter
- Integration tests: rewind past batch state, verify children relocated, verify re-init succeeds, verify superseded branch queryable

### Phase 3: Query surface + documentation

Add `superseded_branches` to `koto status` for batch parents. Update `koto-user` skill docs with the branching model, the query surface, and rewind-as-recovery for batch workflows.

Deliverables:
- `src/cli/mod.rs`: `handle_status` gains `superseded_branches` field
- `plugins/koto-skills/skills/koto-user/references/command-reference.md`: document rewind for batch parents
- `plugins/koto-skills/skills/koto-user/references/batch-workflows.md`: document branching, query surface

## Security Considerations

This design modifies file-system and S3 operations during rewind but does not introduce new attack surface:
- **No external inputs**: Rewind operates on the parent's own event log and enumerates children from `backend.list()`.
- **No permission escalation**: `relocate` renames directories that koto created; no new permissions.
- **Session name validation**: The `~N` epoch suffix must be validated by the same `validate_session_id` function used elsewhere, preventing path traversal.
- **S3 copy-then-delete**: Uses the same authenticated S3 client as existing operations; no new credentials or scopes.

No new attack surface. The S3 data-retention failure mode (old-prefix objects may persist if copy-then-delete fails partway) is a correctness concern, not a security one.

## Consequences

### Positive
- Rewind past `materialize_children` produces a clean namespace without destroying history. Agents can fix a bad task submission and still inspect prior work.
- Superseded branches are fully queryable with standard koto commands — no new CLI surface needed for basic inspection.
- Non-batch rewind is unchanged.
- The branching model is intuitive: "your old work is at `parent~1.*`, carry on."

### Negative
- `SessionBackend::relocate` is new API surface on the trait. Both local and cloud backends need implementations.
- S3 relocate is non-atomic (copy-then-delete). Partial failures leave the old prefix intact, creating temporary duplicates.
- The `~N` naming convention consumes namespace. Multiple rewinds produce `parent~1`, `parent~2`, etc. These persist until explicitly cleaned up.

### Mitigations
- The local relocate is a single `fs::rename` — atomic and fast. The cloud side is best-effort with `session resolve` as the recovery path, matching existing patterns.
- Superseded branches can be cleaned up with `koto session cleanup parent~N` when no longer needed. A future `koto gc` could automate this.
- The `~` character is reserved in session naming validation, preventing user-created sessions from colliding with epoch branches.
