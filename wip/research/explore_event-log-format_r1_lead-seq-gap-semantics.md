# Research: Sequence Gap Detection Semantics

## Summary

The upstream design specifies sequence numbers and fsync for atomicity, but leaves gap
behavior unspecified. Industry-standard append-only log systems (SQLite WAL, PostgreSQL
WAL, EventStore) all treat sequence gaps as fatal signals — they never silently skip.
The correct behavior for koto is **halt-and-error** on any gap detected during replay.

## Current implementation gaps

The current `Event` struct has no `seq` field. The `read_events()` function uses a
"skip malformed lines with warning" strategy — it hides atomicity failures by silently
continuing past unreadable lines. This is incompatible with gap-as-corruption-signal
semantics and must change: malformed lines that are not the final line are a corruption
indicator; a malformed final line may be a partial write (truncated on crash).

## Standard practice in append-only logs

- **SQLite WAL**: Hash-verifies each frame; on verification failure, truncates to the
  last valid frame. Never skips invalid frames in the middle of the log.
- **PostgreSQL WAL**: Truncates at the first invalid record. Recovery replays up to
  the truncation point, then marks the segment as the recovery end.
- **EventStore / Axon**: Halt-and-error on sequence gap; never silently skip. Gap
  means lost durability guarantee.
- **Kafka**: Sequence gaps at broker level trigger replication repair; consumers that
  detect gaps trigger re-fetch. Silent skips are never acceptable.
- **Critical principle**: A gap in a sequence-numbered log indicates lost durability
  guarantees and must never be silent.

## Failure mode analysis

**Partial JSON write (truncated last line):** The final line is syntactically incomplete
JSON — e.g., `{"seq":5,"type":"trans`. This is detectable as a malformed line at the
end of the file. Since it's the last line, it represents a partial write before crash.
The correct behavior is to treat the file as ending at seq 4 (truncate the partial last
line conceptually) and proceed. This is distinct from a gap — there's no missing seq,
just an incomplete final line.

**Crash after write but before fsync:** With fsync enabled, this case is eliminated by
definition — writes not fsynced are not durable. If the OS buffers a write and crashes,
the line may simply be absent from the recovered file (no partial line, just missing).
This shows up as a gap: seq goes 1, 2, 3, (5 never appears) 4 is present, 5 is missing.
Without fsync, this could also appear as truncated content.

**Concurrent process writes:** Two processes appending simultaneously can interleave bytes,
producing corrupted lines. Sequence gaps would appear as evidence of this. This is a
violation of the single-writer assumption koto requires.

## Recommendation: Gap detection behavior

**Halt-and-error on sequence gap.** Specifically:

1. During `read_events`, validate that each event's `seq` is exactly `previous_seq + 1`.
2. If the final line is malformed JSON (partial write), treat as truncated — warn and
   return events up to the last valid seq. This is distinct from a gap.
3. If any non-final line is malformed JSON, treat as corruption — error immediately.
4. If any seq value is out of order or skips a number, error immediately with
   `state_file_corrupted`.

Error code: `state_file_corrupted` with message indicating the gap location
(e.g., "sequence gap: expected seq 4, found seq 6 at line 5").

Recovery path: force manual repair (delete and re-init), not automatic. Automatic
truncation-to-last-valid could hide a race condition where a valid event was lost.

**koto next / koto init / koto rewind**: all propagate the error up, exit with code 3
(config/state error), return structured JSON error.

## Seq number assignment

The writer must read the current max seq before appending. Two options:

**Option A: Reader-provided.** `append_event` takes `seq: u64` from the caller. The
caller reads the last event's seq and passes `last_seq + 1`. Simple, but opens the door
to caller bugs producing duplicate seqs.

**Option B: Writer-managed.** `append_event` reads the last line of the file (without
reading the whole file — seek to EOF, scan backward) to find the current max seq, then
appends `max_seq + 1`. More complex, but correct by construction. Seeking to end of
file and scanning backward for the last `\n` is O(line_length), not O(file_size).

**Recommendation: Writer-managed (Option B)** for correctness. The writer opens in
append mode, seeks to the end, scans back to find the last newline, parses the seq from
that line, and uses `seq + 1`. This keeps the seq assignment logic co-located with the
write logic and prevents caller bugs.

**First event (workflow_initialized):** seq = 1.
