<!-- decision:start id="session-update-intent-concurrency" status="confirmed" -->
### Decision: How `koto session update --intent` safely mutates session metadata under concurrent engine execution

**Context**

The `koto session update <name> --intent "<text>"` command needs to add or overwrite an `intent` field on a session. The session's state file (`koto-<name>.state.jsonl`) is a JSONL file whose first line is a `StateFileHeader` JSON object and whose subsequent lines are append-only event records.

The challenge is concurrent access: `koto next` may be running against the same session file simultaneously. Research into the codebase revealed two key facts that constrain the options:

1. The advisory `flock(LOCK_EX | LOCK_NB)` that `SessionBackend::lock_state_file` provides is **only acquired for batch-scoped states**. Normal single-writer `koto next` invocations hold no file lock. Any strategy that relies on the existing lock to exclude a concurrent `koto next` will fail for the common (non-batch) case.

2. The engine's `append_event` opens the state file with `O_APPEND`. POSIX `O_APPEND` writes are atomic for payloads well under the kernel atomic write limit (each JSON event line is under 4 KB; the limit on Linux is at least 4096 bytes). This means two concurrent `O_APPEND` writers — the engine and a hypothetical second appender — produce a correctly interleaved log without corruption.

Any approach that rewrites the file body (read-all, modify line 0, write-all) introduces a window between the read and the write during which a concurrent engine append could be overwritten.

**Assumptions**

- `intent` will be treated as user-supplied context that can legitimately change at any point in the workflow lifecycle — including during active engine execution.
- Readers (dashboard, `koto query`) already replay the full event log; scanning for the last `IntentUpdated` event adds no meaningful overhead.
- Cloud backend S3 append semantics follow the same event-log append contract; this decision targets local backend, and the cloud case is out of scope for this question.

**Chosen: Append an `IntentUpdated` event to the JSONL event log**

When `koto session update <name> --intent "<text>"` is called, the command appends a new event of type `intent_updated` to the session's JSONL log using the same `O_APPEND` path that `append_event` already uses. The event payload carries the new intent string. Readers derive the current intent by scanning the event log for the last `intent_updated` event; if none is present, intent is absent.

Implementation sketch (~30 lines):
- Add `IntentUpdated { intent: String }` to `EventPayload` with a `skip_deserializing_if` guard so older readers treat it as `Unknown`.
- Add `handle_update` to `src/cli/session.rs` that calls `backend.append_event(name, &EventPayload::IntentUpdated { intent }, timestamp)`.
- Add a `derive_intent(events: &[Event]) -> Option<String>` helper in `persistence.rs` that finds the last `IntentUpdated` payload.
- Wire `koto session update <name> --intent` in the CLI command enum.

**Rationale**

The log-event approach is the only option that is safe under all concurrent-access patterns — both batch and non-batch `koto next` — because it uses the same append atomicity guarantee the engine already relies on. The existing lock infrastructure cannot be used as a general write serializer for this command because non-batch engine runs hold no lock.

The key trade-off is that intent is no longer readable from the header alone; callers must process the event log. This is acceptable because every current consumer of session state (`koto query`, the dashboard, `derive_machine_state`) already replays the full event log. The `koto session list` path reads only the header and does not need to surface intent.

The approach also generalizes naturally: future mutable metadata fields (`label`, `tag`, `owner`) can each be represented as new `*Updated` event types without touching the header struct or introducing new file types.

**Alternatives Considered**

- **Read-modify-write under the existing session lock**: Acquire `flock(LOCK_EX | LOCK_NB)` before rewriting line 0 of the state file. Rejected because non-batch `koto next` holds no lock, so the lock does not actually exclude a concurrent engine run. The read-modify-write window remains open for the common case.

- **Sidecar metadata file**: Write a `koto-<name>.meta.json` alongside the state file using atomic rename. Safe from the concurrency perspective (engine never touches the sidecar), but requires every reader to know about a second file type, adds a permanent data-model artifact, and splits session metadata across two files. The log-event approach achieves the same safety with less permanent complexity.

- **Full state file rewrite without lock** (atomic rename, no lock acquisition): Read, modify line 0, write to tmp, rename. The rename is atomic, but does not prevent data loss: an engine process that opened the file with `O_APPEND` before the rename continues to write to the old inode. If it appended an event between the read and the rename, that event is silently discarded when the new file wins the name. This is the worst option for correctness.

**Consequences**

- Reading `intent` requires a log scan, not just a header read. The `read_header`-only path (used by `session list`) will not surface intent, which is acceptable for listing use cases.
- The event log grows by one line per `koto session update --intent` call. This is negligible.
- The `IntentUpdated` event type must be added to `EventPayload` and handled (as a recognized no-op) by the engine's advance loop to avoid spurious `Unknown` warnings.
- Future mutable metadata fields have a clear pattern: add a new `*Updated` event type and a corresponding `derive_*` helper.
<!-- decision:end -->
