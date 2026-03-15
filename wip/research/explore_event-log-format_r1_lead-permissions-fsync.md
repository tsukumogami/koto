# Research: File Permissions and fsync

## Summary

The current `append_event` function is missing two critical features: mode 0600
permissions on file creation, and fsync after each write. Both are required per the
upstream design's security and atomicity specifications. The fix uses
`std::os::unix::fs::OpenOptionsExt::mode(0o600)` for permissions (Unix-only, which
matches koto's target platforms) and `File::sync_data()` after each write. Sequence
number assignment should be writer-managed: the writer reads the last event's seq
and appends `max_seq + 1`.

## Current implementation gaps

`src/engine/persistence.rs`, `append_event` (lines 10-24):

```rust
let mut file = OpenOptions::new()
    .create(true)
    .append(true)
    .open(path)  // No mode set — inherits umask, typically 0644
```

Missing:
1. No `.mode(0o600)` — file inherits umask, may be readable by group/other
2. No fsync after `writeln!` — write is buffered; OS crash before flush loses the event
3. The `Event` struct has no `seq` field — sequence numbers not yet implemented

## How to set mode 0600 in Rust

```rust
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

let mut opts = OpenOptions::new();
opts.create(true).append(true);
#[cfg(unix)]
opts.mode(0o600);
let mut file = opts.open(path)?;
```

**Platform notes:**
- `.mode()` applies only on file creation; existing files keep their current permissions
- koto targets linux/darwin only (per `.github/workflows/release.yml`): `x86_64-unknown-linux-gnu`,
  `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`
- All four are Unix platforms; `#[cfg(unix)]` is the right guard
- No Windows build target, so no Windows compat concern

## fsync after append: sync_data vs sync_all

**Use `sync_data()`.**

- `sync_all()` flushes data AND inode metadata (timestamps, size, permissions)
- `sync_data()` flushes only data, not metadata

For event durability, only the data matters. The sequence number in the event
itself serves as the integrity marker — a gap detects partial writes. Inode
metadata sync is unnecessary overhead.

```rust
writeln!(file, "{}", line)?;
file.sync_data()
    .map_err(|e| anyhow::anyhow!("failed to fsync state file {}: {}", path.display(), e))?;
```

## Fsync performance consideration

Fsync latency is typically 1-10ms per call depending on storage hardware. A workflow
with 50 transitions = 50-500ms in fsync overhead. This is acceptable for koto's use
case — workflows are human-paced, not latency-sensitive.

**Why selective fsync is wrong:** The upstream design says "each event is appended
with fsync" and uses sequence number gaps to detect partial writes. If certain event
types (e.g., `evidence_submitted`) skip fsync, a crash mid-append produces a gap
indistinguishable from a real partial write. This breaks the gap detection semantics.
All events must fsync, no exceptions.

**Future optimization:** If fsync cost becomes a bottleneck (e.g., automated workflows
advancing 100s of times/second), batch-append APIs could write multiple events in one
fsync. This is an implementation detail; the design spec (each event is durable before
the next starts) would still hold.

## Seq number assignment

**Option A — caller-provided:** `append_event(path, event, seq)`. Caller reads last
event, computes next seq, passes it. Simple but error-prone (caller bugs produce
duplicate seqs, which are harder to detect than gaps).

**Option B — writer-managed:** `append_event` reads the last line of the file to get
the current max seq, appends `max_seq + 1`. Reading only the last line is O(line length):
open, seek to EOF, scan backward to last `\n`. On a new file (no events yet), seq = 1.

**Recommendation: Option B (writer-managed).** Seq assignment and write are co-located;
callers can't produce invalid sequences. Implementation:

```rust
fn next_seq(path: &Path) -> anyhow::Result<u64> {
    if !path.exists() {
        return Ok(1);
    }
    // Read last line of file to get current max seq
    // Use BufReader::seek or manual backward scan
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let mut last_seq = 0u64;
    for line in reader.lines() {
        let line = line?;
        if let Ok(obj) = serde_json::from_str::<serde_json::Value>(&line) {
            if let Some(seq) = obj.get("seq").and_then(|v| v.as_u64()) {
                last_seq = seq;
            }
        }
    }
    Ok(last_seq + 1)
}
```

Note: the header line (first line) has no `seq` field, so it's correctly skipped
by this logic.

## Recommended implementation approach

1. Add `seq: u64` to the `Event` struct (non-optional; all events have seq)
2. Update `append_event` to: (a) determine next seq via writer-managed read, (b) set
   mode 0600 on creation, (c) serialize event with seq, (d) write, (e) `sync_data()`
3. Update `read_events` to validate seq monotonicity: error on gaps, warn-and-accept
   on truncated final line (see seq-gap-semantics research)
4. Update all callers of `append_event` to not pass seq (writer manages it)
5. Update test helpers (`make_event`) to include seq fields, or let tests use
   `append_event` which will assign seq automatically
