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
  TBD
rationale: |
  TBD
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
