# Lead: Reader Guarantees and Header/Event Boundary

## Findings

### Seq Assignment

`append_event` in `persistence.rs` assigns seq by reading the last non-empty non-header line's `seq` field, then writing `last_seq + 1` (lines 45-49). The function reads the full file every call to find the last seq. This is a read-then-write pattern with no file lock protecting the gap between the read and the subsequent append. Two concurrent callers could read the same last seq and produce a duplicate seq value, which `read_events` would reject as a sequence gap on the second line.

Seq starts at 1 for the first event (when no events yet exist). The reader (`read_events`) enforces `expected_seq` starting at 1 and requires exactly `prev_seq + 1` for every subsequent event; any gap produces `StateFileCorrupted`.

### Append Mechanics

`append_event` opens with `OpenOptions::create(true).append(true)` — the OS-level `O_APPEND` flag (lines 58-59). This makes each `write` atomic at the kernel level: the offset is determined by the kernel at write time, so two concurrent `O_APPEND` writers cannot interleave bytes within a single `write` syscall. However, the read-determine-seq-then-open-then-append sequence is NOT atomic as a whole. There is a TOCTOU window between reading the file to determine next_seq and opening it for append.

`sync_data()` is called after every write for both `append_header` and `append_event` (lines 32-33, 77-78). This flushes kernel buffers to durable storage before returning success.

### Initialization Atomicity (init_state_file)

`init_state_file` in `local.rs` provides the strongest write guarantee. It:
1. Serializes the full bundle (header + all initial events) into memory.
2. Writes to a tempfile in the same session directory with `sync_data()`.
3. Sets permissions 0600 on the tempfile.
4. Renames the tempfile to the final path using `atomic_create_rename`.

On Linux, `atomic_create_rename` uses `renameat2(RENAME_NOREPLACE)` (lines 636-652), which is atomic and fails if the destination already exists. On non-Linux Unix, it falls back to POSIX `link()` + `unlink()`, which has the same fail-if-exists semantics. If a crash occurs between the tempfile write and the rename, the final state file is never visible — only a `.koto-init-*.tmp` orphan remains, which does not block a fresh init.

### Ongoing Append Locking

For ordinary appends (`append_event`), there is **no file lock held**. The OS `O_APPEND` flag prevents byte interleaving but does NOT prevent two processes from deriving the same next_seq concurrently. The `lock_state_file` method (exclusive advisory `flock(LOCK_EX | LOCK_NB)`) exists and is exposed via the `SessionBackend` trait but is NOT called internally by `append_event` or `append_header`. It is only called by the CLI (`cli/mod.rs`, lines 2295-2316) for batch-scoped states, as a guard against concurrent batch scheduler ticks racing the read-decide-write cycle.

For non-batch workflows, concurrent `append_event` calls from two processes or threads are therefore not serialized at the persistence layer. The de facto single-writer assumption is enforced by workflow convention (one koto process per workflow) rather than by file locking.

### Crash and Partial-Write Behavior

A crash mid-write can produce a truncated final line. `read_events` handles this: if the final line fails JSON parsing, it logs a warning to stderr and returns all prior events (lines 192-199). A malformed non-final line is treated as corruption and produces `StateFileCorrupted`.

`sync_data()` after every write means that on a successful return from `append_event`, the data is on stable storage. If the process is killed between the serialization and the `writeln!` call, nothing is written. If killed between `writeln!` and `sync_data()`, a partial line may exist in the kernel buffer and may or may not reach disk — this is the one unguarded window where a partial line could survive.

### Header vs. Event Structure

`StateFileHeader` (types.rs, lines 9-51) has no `seq` field. It carries: `schema_version`, `workflow`, `template_hash`, `created_at`, and optionally `parent_workflow`, `template_source_dir`, `session_id`. It is written exactly once, before any events, via `append_header`, which uses `create(true).write(true).truncate(false)` (not append mode).

`Event` (types.rs, lines 404-417) serializes with a custom `Serialize` impl that produces four fixed top-level keys in order: `seq`, `timestamp`, `type`, `payload`. Events never have `schema_version` or `workflow` fields.

The reader distinguishes line 1 from subsequent lines structurally: `read_events` calls `parse_header(lines[0])` explicitly, then iterates `lines[1..]` as events. There is no `seq` key or `type` key on the header. A reader that tries to parse line 1 as an `Event` will fail because `seq` is missing. A reader that tries to parse a subsequent line as a `StateFileHeader` will likely fail because `workflow` or `schema_version` is missing (though `#[serde(default)]` on optional fields means it might partially succeed — not tested in the codebase for this negative path).

The `relocate` operation rewrites line 1 in place: it reads the full file, replaces `lines[0]` with a re-serialized header, and writes back with `fs::write` (local.rs, lines 299-313). This is not atomic — a crash between the read and write would corrupt the file.

### File Permissions

Both `append_header` and `append_event` set `mode(0o600)` at file creation on Unix. `init_state_file` sets 0600 explicitly via `set_permissions` after tempfile creation. The `~/.koto/` root directory is created with mode 0700.

## Implications

**What a reader can rely on:**

1. Line 1 is always the header. It contains no `seq` field. Readers must parse it as `StateFileHeader` before treating any subsequent line as an `Event`.
2. Events start at line 2. Seq values begin at 1 and increase by exactly 1 per event. Any gap is corruption.
3. Every successfully committed event has been flushed to disk (`sync_data()` guarantees this). A reader that sees an event can trust it is not a half-buffered artifact from a normal (non-crash) write.
4. A truncated final line is recoverable: parse all prior lines; the malformed final line can be discarded. The reader (`read_events`) already does this.
5. The initial state file (header + first events) is written atomically: either the full bundle is visible at the final path, or it is not visible at all. No partial state file from `init_state_file` can be observed at the canonical path.
6. State files have mode 0600; the `.koto/` directory has 0700.

**Edge cases readers must handle:**

1. A truncated final line (crash after partial write, before `sync_data`). Parse and discard it; treat prior events as complete.
2. A sequence gap on a non-final line: this is hard corruption, not crash recovery. Readers should reject the file.
3. An empty file or a file with an empty first line: treat as corruption.
4. Old state files without `session_id`, `parent_workflow`, or `template_source_dir` fields: these deserialize cleanly due to `#[serde(default)]` on those fields.
5. The `relocate` operation rewrites line 1 non-atomically. An external reader that reads during a relocate could see a torn header. This is an existing gap.

**Header/event boundary for external readers:**

- Parse line 1 as a JSON object. If it contains `schema_version` but no `seq`, it is the header.
- Parse all subsequent non-empty lines as events. Each must have `seq`, `timestamp`, `type`, and `payload`.
- The `type` field (not `event_type`) is the discriminant for payload shape.

## Surprises

1. **No write lock on `append_event`.** The `lock_state_file` advisory flock exists as a method on `SessionBackend` but is not used internally by `append_event`. Only batch-scoped states acquire it (via CLI code in `cli/mod.rs`). For ordinary single-process workflows this is safe by convention, but nothing in the persistence layer enforces it. A second writer could corrupt the seq sequence.

2. **Seq is determined by reading the file, not by an in-memory counter.** Every `append_event` call reads the entire file to find the last seq, then appends. There is no memoization. This creates O(n) I/O growth and a TOCTOU window that concurrent writers could exploit.

3. **`relocate` is not atomic.** The header-rewrite operation (read file → replace line 0 → write back) has no atomic or locking protection. A crash or concurrent read during this window would produce a torn or corrupt header. This is the one place where an in-place mutation happens to the file rather than an append.

4. **Timestamps are not validated by the reader.** `read_events` does not verify that timestamps are monotonically increasing or parse as valid RFC 3339. It trusts the `timestamp` field as a raw string. External readers cannot assume timestamps are ordered or well-formed without their own validation.

5. **`EventPayload` uses `#[serde(untagged)]`** for its enum but the custom `Event` deserializer ignores this: it dispatches on the outer `type` field and deserializes the `payload` field into a typed struct per variant. The `#[serde(untagged)]` attribute on `EventPayload` is only relevant if `EventPayload` is deserialized directly (not through the `Event` wrapper). External readers should use the `type` field for dispatch, not attempt to infer the variant from payload field presence.

## Open Questions

1. **Should `append_event` acquire the advisory flock?** The existing `lock_state_file` primitive works correctly but is only used by batch workflows. If external consumers ever write events (e.g., a relay that appends synthetic events), they need to know whether they are expected to hold the lock. The contract needs to state this explicitly.

2. **Is the `relocate` non-atomicity an accepted known risk or a gap to fix?** It is not mentioned in any comments. A reader observing a file mid-relocate would see a torn header.

3. **Is the seq-from-file-read approach intentional or an early implementation detail?** The comment in `read_last_seq` (lines 86-88) says "avoids silently masking corruption" but an in-memory counter would achieve the same while removing the TOCTOU window. This is relevant to the contract because it means the writer reads before writing, which external readers should not emulate for concurrent workloads.

4. **External reader timestamp handling.** The contract should state whether timestamps are guaranteed to be monotonically increasing. The code generates them with `now_iso8601()` at call time but does not enforce monotonicity. Clock skew or same-millisecond events could produce non-monotonic or equal timestamps.

5. **`schema_version` versioning policy.** Currently hard-coded to `1`. The contract should define what version changes mean and how readers should handle unknown versions.

## Summary

The persistence layer makes strong durability guarantees per-write (`sync_data()` after every append) and provides fully atomic file creation via tempfile-then-`renameat2`, but ongoing appends are not protected by a file lock for ordinary (non-batch) workflows — only the OS `O_APPEND` flag serializes individual byte writes, leaving the seq-assignment read-then-write open to a race if two writers ever run concurrently. The header/event boundary is structurally unambiguous: line 1 has `schema_version` but no `seq`, and all subsequent lines have `seq` and `type` but no `schema_version`, so readers can distinguish them without a magic byte or explicit marker. The biggest open question for the contract is whether `append_event` callers are expected to hold the advisory flock — this is enforced in one call site (batch scheduler) but not in the persistence layer itself, and the contract must make the single-writer requirement explicit so relay and dashboard implementers know what they are allowed to assume.
