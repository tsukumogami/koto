//! The write half of state-file persistence: every append, the locks it
//! takes, whole-log replacement, the supersede fence's append, and the split
//! of a merged last line. The read half is the parent module.

use std::fs::OpenOptions;
use std::io::Write;

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::*;

/// Serialize one header line (no trailing newline).
///
/// The single definition of "what a header line looks like on disk",
/// shared by the append path and by callers that buffer a header plus
/// their first events into one atomic write.
pub fn serialize_header_line<H: Serialize>(header: &H) -> anyhow::Result<String> {
    serde_json::to_string(header).map_err(|e| anyhow::anyhow!("failed to serialize header: {}", e))
}

/// Write the header line to a new state file.
///
/// Creates the file with mode 0600 on unix. The header has no seq field.
///
/// When the state file did not exist on entry, this call ALSO fsyncs the
/// parent directory after the header write so the new file's directory
/// entry is durable across a crash. Subsequent calls (which reopen an
/// existing file) skip the parent-dir fsync: the directory entry is
/// already committed and `sync_data()` is sufficient. The parent-dir
/// fsync is best-effort — some filesystems reject `O_RDONLY` directory
/// fsync with `EINVAL`; we tolerate that for parity with the existing
/// pattern in `discovery.rs`.
pub fn append_header(path: &Path, header: &StateFileHeader) -> anyhow::Result<()> {
    append_header_line(path, header)
}

/// Header-type-generic form of [`append_header`].
///
/// Same durability discipline; see that function's doc comment.
pub fn append_header_line<H: Serialize>(path: &Path, header: &H) -> anyhow::Result<()> {
    let is_first_write = !path.exists();

    let mut opts = OpenOptions::new();
    opts.create(true).write(true).truncate(false);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }

    let mut file = opts
        .open(path)
        .map_err(|e| anyhow::anyhow!("failed to open state file {}: {}", path.display(), e))?;

    let mut line = serialize_header_line(header)?;
    line.push('\n');

    file.write_all(line.as_bytes())
        .map_err(|e| anyhow::anyhow!("failed to write header to {}: {}", path.display(), e))?;

    file.sync_data()
        .map_err(|e| anyhow::anyhow!("failed to sync state file {}: {}", path.display(), e))?;

    if is_first_write {
        fsync_parent_dir(path);
    }

    Ok(())
}

/// Best-effort fsync of `path`'s parent directory. Tolerates `EINVAL`
/// on filesystems that reject `O_RDONLY` directory fsync (mirrors
/// `discovery.rs:173-183`). On success the file's directory entry is
/// durable; on failure the prior `sync_data()` on the file is still
/// the truth-of-record and the directory entry will be flushed at the
/// filesystem's next journal commit. Called by [`append_header`] only
/// when a brand-new state file was created — later appends do not
/// change the directory entry and can skip this step.
fn fsync_parent_dir(path: &Path) {
    if let Some(parent) = path.parent() {
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }
}

/// Append one event to a session state file, auto-assigning its seq number.
///
/// Reads the file to find the last event's seq (or 0 if no events yet),
/// then appends with `max_seq + 1`. Calls `sync_data()` after every write.
///
/// Refuses, writing nothing, unless the file already exists and its first
/// line is a session header. An append is never how a log comes into being:
/// creating a session (init, session start, a batch spawn) writes the header
/// and the first events together in one atomic file. An append that
/// created the file, or extended one that had lost its header, would leave a
/// log whose first line is an event, which every reader rejects from then on
/// (koto#236, koto#200).
pub fn append_event(path: &Path, payload: &EventPayload, timestamp: &str) -> anyhow::Result<u64> {
    append_event_in::<StateFileHeader>(path, payload, timestamp)
}

/// Header-type-generic form of [`append_event`]: the first line must parse
/// as an `H`.
pub fn append_event_in<H: LogHeader>(
    path: &Path,
    payload: &EventPayload,
    timestamp: &str,
) -> anyhow::Result<u64> {
    append_event_reading::<H>(path, payload, timestamp).map(|(seq, _)| seq)
}

/// What one committed append to a session's log tells the code that runs
/// after it, all taken from the bytes the append read under its lock.
///
/// Holds no journal types: the run journal is its one reader today, but
/// nothing here knows about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitInfo {
    /// The committed event's `seq`.
    pub seq: u64,
    /// The log as it stood before this event: the bytes the append read to
    /// find the next `seq`, moved rather than copied. When the append split
    /// a merged last line first, these are the split bytes, so every event
    /// in them has a line of its own.
    pub prior_log: String,
}

impl CommitInfo {
    /// The `seq` of the event that opened the visit the log was in before
    /// this event, by [`visit_start`]'s rule; `None` for a log with no state
    /// entry. Computed from [`prior_log`](Self::prior_log) on each call, so
    /// a commit nobody asks about pays nothing for it.
    pub(crate) fn prior_visit_seq(&self) -> Option<u64> {
        visit_start(&self.prior_log)
    }

    /// The visit `payload` (the committed event) belongs to: its own `seq`
    /// when it opens a visit itself, otherwise the visit the log was in.
    pub(crate) fn visit_seq_for(&self, payload: &EventPayload) -> Option<u64> {
        if opens_visit(payload) {
            Some(self.seq)
        } else {
            self.prior_visit_seq()
        }
    }
}

/// [`append_event`], returning what the append read as well as the `seq` it
/// took (see [`CommitInfo`]). Writes exactly what [`append_event`] writes.
pub(crate) fn append_event_tracked(
    path: &Path,
    payload: &EventPayload,
    timestamp: &str,
) -> anyhow::Result<CommitInfo> {
    let (seq, prior_log) = append_event_reading::<StateFileHeader>(path, payload, timestamp)?;
    Ok(CommitInfo { seq, prior_log })
}

/// The one append: returns the `seq` taken and the log's bytes as read
/// under the lock, before the new line.
fn append_event_reading<H: LogHeader>(
    path: &Path,
    payload: &EventPayload,
    timestamp: &str,
) -> anyhow::Result<(u64, String)> {
    // Held from reading the last seq until the line is synced, so two
    // processes appending at once can't both take the same seq.
    let _append_lock = AppendLock::acquire(path);
    append_under_lock::<H>(path, payload, timestamp)
}

/// [`append_event_tracked`] for a caller that already holds the log's append
/// lock through `held`, from [`hold_log_lock`]: the append runs under that
/// lock rather than taking it again, which would wait on `held` itself (see
/// [`HeldLogLock`]). Refused, writing nothing, when `held` is the lock of a
/// log other than `path`.
pub(crate) fn append_event_tracked_held(
    held: &HeldLogLock,
    path: &Path,
    payload: &EventPayload,
    timestamp: &str,
) -> anyhow::Result<CommitInfo> {
    held.check(path)?;
    let (seq, prior_log) = append_under_lock::<StateFileHeader>(path, payload, timestamp)?;
    Ok(CommitInfo { seq, prior_log })
}

/// The body of every append, run with the log's append lock held by the
/// caller.
fn append_under_lock<H: LogHeader>(
    path: &Path,
    payload: &EventPayload,
    timestamp: &str,
) -> anyhow::Result<(u64, String)> {
    debug_assert!(
        !matches!(payload, EventPayload::Unknown { .. }),
        "Unknown events must not be passed to append_event"
    );
    let tail = prepare_append::<H>(path)?;
    // Checked on the bytes read under the lock, so no write slips in after
    // a fence another process just appended.
    refuse_if_fenced(path, &tail.content, payload)?;
    refuse_if_rebound(path, &tail.content, payload)?;

    let event = Event {
        seq: tail.next_seq,
        timestamp: timestamp.to_string(),
        event_type: payload.type_name().to_string(),
        payload: payload.clone(),
        idempotency_hash: None,
    };
    write_event_line(path, &event, tail.unterminated)?;

    Ok((tail.next_seq, tail.content))
}

/// Append `event` to the log at `path` as one line, in one write, and sync it.
///
/// The line and its newline go out in a single `write_all` of one buffer.
/// Written as two writes (`writeln!` on an unbuffered file issues the line and
/// the `\n` separately), a writer killed between them -- a timed-out action's
/// process group gets SIGKILL -- leaves a line with no terminator, and the next
/// append lands on the same line (koto#419). `unterminated` is the log's own
/// last line lacking its newline, from a writer that died that way before this
/// fix: the buffer then starts with the missing `\n`, so this event still gets
/// a line of its own.
fn write_event_line(path: &Path, event: &Event, unterminated: bool) -> anyhow::Result<()> {
    // No `create`: if the log disappeared since it was read, the open fails
    // rather than starting a new, headerless one.
    let mut file = OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|e| anyhow::anyhow!("failed to open state file {}: {}", path.display(), e))?;
    write_event_to(&mut file, event, unterminated)
        .map_err(|e| anyhow::anyhow!("failed to write event to {}: {:#}", path.display(), e))?;
    file.sync_data()
        .map_err(|e| anyhow::anyhow!("failed to sync state file {}: {}", path.display(), e))?;
    Ok(())
}

/// The body of [`write_event_line`] past the open: `event` serialized and
/// written to `w` as one line, in one `write_all` of one buffer. Separate
/// from the open so a test can count the writes.
fn write_event_to(w: &mut impl Write, event: &Event, unterminated: bool) -> anyhow::Result<()> {
    let line = serde_json::to_string(event)
        .map_err(|e| anyhow::anyhow!("failed to serialize event: {}", e))?;
    let mut buf = String::with_capacity(line.len() + 2);
    if unterminated {
        buf.push('\n');
    }
    buf.push_str(&line);
    buf.push('\n');
    w.write_all(buf.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod single_write_tests {
    use super::*;
    use crate::engine::types::{Event, EventPayload};

    /// Accepts every write whole and keeps each call's bytes apart.
    #[derive(Default)]
    struct Writes(Vec<Vec<u8>>);

    impl Write for Writes {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.push(buf.to_vec());
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn event() -> Event {
        let payload = EventPayload::ContextAdded {
            key: "k".to_string(),
            hash: "0".repeat(64),
            size: 1,
            writer: None,
        };
        Event {
            seq: 7,
            timestamp: "2025-01-01T00:00:00Z".to_string(),
            event_type: payload.type_name().to_string(),
            payload,
            idempotency_hash: None,
        }
    }

    /// One state-log event goes out in exactly one write: the whole line
    /// and its newline, never the line in pieces or the newline on its own
    /// (koto#419, koto#728).
    #[test]
    fn an_event_line_goes_out_in_one_write() {
        let event = event();
        let line = serde_json::to_string(&event).unwrap();
        for (unterminated, want) in [(false, format!("{line}\n")), (true, format!("\n{line}\n"))] {
            let mut w = Writes::default();
            write_event_to(&mut w, &event, unterminated).unwrap();
            assert_eq!(
                w.0.len(),
                1,
                "unterminated={unterminated}: {} writes: {:?}",
                w.0.len(),
                w.0.iter()
                    .map(|b| String::from_utf8_lossy(b).into_owned())
                    .collect::<Vec<_>>()
            );
            assert_eq!(String::from_utf8_lossy(&w.0[0]), want);
        }
    }
}

/// Outcome of an idempotent append.
///
/// Distinguishes "this call wrote a new event" from "this call observed
/// a prior identical event and short-circuited". Both return the
/// authoritative seq number the caller should reference; only
/// [`AppendOutcome::Written`] increments the on-disk event count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendOutcome {
    /// A new event was appended at the returned seq. The file gained
    /// one line and one fsync.
    Written { seq: u64 },
    /// An identical prior event was found via the idempotency hash;
    /// no write or fsync occurred. The returned seq points at the
    /// prior event so the caller can reference it.
    Idempotent { seq: u64 },
}

impl AppendOutcome {
    /// Return the seq number regardless of whether a write occurred.
    pub fn seq(&self) -> u64 {
        match self {
            AppendOutcome::Written { seq } | AppendOutcome::Idempotent { seq } => *seq,
        }
    }
}

/// Compute the SHA-256 hex digest of canonical-JSON serialization of
/// `(state_name, payload)`.
///
/// "Canonical" means: serde-serialize the tuple, parse the result as a
/// [`serde_json::Value`], recursively sort all object keys, then
/// re-serialize without whitespace. Two payloads that differ only in
/// key order or whitespace produce identical hashes, matching the R17
/// idempotency contract.
///
/// `state_name` is included alongside the payload so identical payloads
/// at different states do not short-circuit each other. For payload
/// variants whose serialized form already embeds a `state` field, this
/// is redundant-but-defensive; for payloads with no `state` (e.g.
/// `WorkflowInitialized`, `IntentUpdated`), the explicit `state_name`
/// preserves the hash-domain contract.
pub fn idempotency_hash(state_name: &str, payload: &EventPayload) -> String {
    let payload_value = serde_json::to_value(payload).expect("EventPayload is always serializable");
    let domain = serde_json::json!({
        "state_name": state_name,
        "payload": payload_value,
    });
    let canonical = canonicalize_json(domain);
    let serialized = serde_json::to_string(&canonical).expect("canonical JSON is serializable");
    let mut hasher = Sha256::new();
    hasher.update(serialized.as_bytes());
    hex::encode(hasher.finalize())
}

/// Recursively sort object keys in a [`serde_json::Value`] so the
/// re-serialized form is deterministic regardless of input key order.
///
/// Arrays preserve their order (semantic). Objects are rebuilt with
/// sorted keys (canonicalization).
fn canonicalize_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut sorted: std::collections::BTreeMap<String, serde_json::Value> =
                std::collections::BTreeMap::new();
            for (k, v) in map {
                sorted.insert(k, canonicalize_json(v));
            }
            // BTreeMap iterates in sorted key order; build a
            // serde_json::Map from the sorted entries.
            let mut out = serde_json::Map::with_capacity(sorted.len());
            for (k, v) in sorted {
                out.insert(k, v);
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(canonicalize_json).collect())
        }
        other => other,
    }
}

/// Append an event with idempotency-hash short-circuit semantics.
///
/// Behavior depends on `hash`:
///
/// - `None`: identical to [`append_event`]. The event is appended with no
///   hash field, and the same header check applies.
/// - `Some(h)`: scan the on-disk log for an event whose stored
///   `idempotency_hash` equals `h`.
///   - If found AND its payload bytes-equal the new payload, return
///     [`AppendOutcome::Idempotent`] with the prior seq; no write, no
///     fsync (PRD R17).
///   - If found AND its payload differs, return
///     [`EngineError::ConcurrentSubmissionConflict`] with the supplied
///     `state_name`; no write, no fsync (PRD OQ8).
///   - If not found, append a new event with `idempotency_hash: Some(h)`
///     and return [`AppendOutcome::Written`].
///
/// The hash-vs-payload bytes-equal check is belt-and-suspenders: SHA-256
/// preimage collisions are cryptographically negligible, but if a future
/// schema change makes the hash domain narrower than the payload (an
/// unintended regression) the bytes-equal check catches it.
///
/// Concurrent identical retries are serialized via [`acquire_state_flock`]
/// so the race-condition AC (N=32 concurrent identical retries → 1
/// event on disk) holds. The flock guards the read-then-write window;
/// readers outside this path are unaffected.
pub fn append_event_idempotent(
    path: &Path,
    payload: &EventPayload,
    timestamp: &str,
    state_name: &str,
    hash: Option<&str>,
) -> anyhow::Result<AppendOutcome> {
    append_event_idempotent_in::<StateFileHeader>(path, payload, timestamp, state_name, hash)
}

/// Header-type-generic form of [`append_event_idempotent`]. Like
/// [`append_event_in`], it refuses a log whose first line isn't an `H`.
pub fn append_event_idempotent_in<H: LogHeader>(
    path: &Path,
    payload: &EventPayload,
    timestamp: &str,
    state_name: &str,
    hash: Option<&str>,
) -> anyhow::Result<AppendOutcome> {
    debug_assert!(
        !matches!(payload, EventPayload::Unknown { .. }),
        "Unknown events must not be passed to append_event_idempotent"
    );

    // No hash → fall back to the non-idempotent path.
    let Some(h) = hash else {
        let seq = append_event_in::<H>(path, payload, timestamp)?;
        return Ok(AppendOutcome::Written { seq });
    };

    // Take an exclusive lock on the state file for the duration of the
    // read-then-write window. The lock file is the same as the state
    // file; concurrent readers via `read_events` do NOT take a lock and
    // are unaffected (advisory).
    let _guard = acquire_state_flock(path)?;
    // The append lock every append path shares, held through the seq read,
    // the scan and the write, so a plain `append_event` from another process
    // can't take the seq this append is about to use.
    let _append_lock = AppendLock::acquire(path);

    // Check the header first, so a log that is missing, empty or headerless
    // is refused whether or not the scan below finds a prior event. Doing it
    // after the scan would let an idempotent hit report success on a log no
    // reader can parse. The scan below reads the bytes this read returns,
    // under the lock taken above.
    let tail = prepare_append::<H>(path)?;
    refuse_if_fenced(path, &tail.content, payload)?;
    refuse_if_rebound(path, &tail.content, payload)?;
    let next_seq = tail.next_seq;

    // Scan for a prior event with the same hash. A malformed line in the
    // middle of the log is skipped, the way `read_events` skips it. A
    // malformed *final* line has been dealt with by this point: the read
    // above split it if it held merged events, and failed otherwise, since
    // it parses that line to work out the next seq. Callers that expect a
    // torn tail repair it before appending, as the request store does. Line 1 is the
    // header, checked above, so events start at line 2 (1-indexed).
    for line in tail.content.lines().skip(1) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let prior_hash = parsed.get("idempotency_hash").and_then(|v| v.as_str());
        if prior_hash != Some(h) {
            continue;
        }
        let prior_seq = parsed
            .get("seq")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("prior event missing seq"))?;
        // Bytes-equal payload check (belt-and-suspenders).
        let prior_payload = parsed
            .get("payload")
            .ok_or_else(|| anyhow::anyhow!("prior event missing payload"))?;
        let new_payload_value =
            serde_json::to_value(payload).expect("EventPayload is always serializable");
        if prior_payload == &new_payload_value {
            return Ok(AppendOutcome::Idempotent { seq: prior_seq });
        } else {
            return Err(EngineError::ConcurrentSubmissionConflict {
                session_id: extract_session_id_from_path(path),
                state_name: state_name.to_string(),
            }
            .into());
        }
    }

    // No prior hash hit → append a new event with the hash stored.
    let event = Event {
        seq: next_seq,
        timestamp: timestamp.to_string(),
        event_type: payload.type_name().to_string(),
        payload: payload.clone(),
        idempotency_hash: Some(h.to_string()),
    };
    write_event_line(path, &event, tail.unterminated)?;

    Ok(AppendOutcome::Written { seq: next_seq })
}

// Lock order. A process holding more than one of a session's locks takes
// them in this order, never the reverse: the state-file lock a batch tick
// holds, `environment.lock` or `decider.lock`, the context manifest lock
// (`ctx/manifest.lock`), the append lock below, then `cloud-batch.lock`. A
// lock held for a whole tick, if one is added, goes before all of them.

/// File name of the per-log append lock, a sidecar in the log's directory.
/// It holds no data.
pub const APPEND_LOCK_FILE: &str = "append.lock";

/// An exclusive `flock` on the sidecar [`APPEND_LOCK_FILE`] beside a log,
/// released on drop.
///
/// Every append path takes it, and only around reading the last seq, writing
/// the line and syncing it. A writer that replaces the log whole (a header
/// rewrite, a cloud pull) takes it through [`with_log_replace_lock`] and holds
/// it from reading the log to renaming the replacement over it, which is
/// longer than an append but still only file I/O. `koto context add` holds it
/// through [`hold_log_lock`] from its header check, across its store write,
/// to its append, so no replacement lands between them; on the cloud backend
/// that span includes the pull's requests and the content's upload, which is
/// why every replacement's wait is bounded. None holds it while a command
/// runs, so a gate's own `koto context get` can't wait on the tick that is
/// running it; keep slow work out from under it. It is a
/// dedicated file rather than the log itself, so it never contends with the
/// state-file lock a batch tick holds for the whole tick.
///
/// Where the lock can't be had -- no `flock` on this platform, a directory
/// the sidecar can't be created in, a filesystem that refuses the call --
/// the append proceeds unlocked, as appends did before the lock existed. A
/// wait the kernel cuts short with `EINTR` is not one of those: a signal
/// arriving while another process holds the lock retries the wait, so a
/// signal can't let an append through unlocked.
struct AppendLock {
    #[cfg(unix)]
    _file: Option<std::fs::File>,
}

impl AppendLock {
    #[cfg(unix)]
    fn acquire(log: &Path) -> Self {
        use std::os::fd::AsRawFd;
        let Some(dir) = log.parent() else {
            return AppendLock { _file: None };
        };
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(APPEND_LOCK_FILE))
        {
            Ok(f) => f,
            Err(_) => return AppendLock { _file: None },
        };
        loop {
            // SAFETY: `fd` is borrowed from `file`, which outlives the call.
            let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if ret == 0 {
                return AppendLock { _file: Some(file) };
            }
            if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                return AppendLock { _file: None };
            }
        }
    }

    #[cfg(not(unix))]
    fn acquire(_log: &Path) -> Self {
        AppendLock {}
    }

    /// [`acquire`](Self::acquire) with the wait bounded by `wait`: `None`
    /// when another process still holds the lock once `wait` has passed.
    /// Where the lock can't be had at all, the fallback is the one
    /// `acquire` takes: an unlocked guard.
    #[cfg(unix)]
    fn acquire_within(log: &Path, wait: std::time::Duration) -> Option<Self> {
        use std::os::fd::AsRawFd;
        let Some(dir) = log.parent() else {
            return Some(AppendLock { _file: None });
        };
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(APPEND_LOCK_FILE))
        {
            Ok(f) => f,
            Err(_) => return Some(AppendLock { _file: None }),
        };
        match crate::engine::jsonl_append::lock_exclusive(
            file.as_raw_fd(),
            Some(wait),
            std::time::Instant::now(),
        ) {
            Ok(()) => Some(AppendLock { _file: Some(file) }),
            Err(e) if crate::engine::jsonl_append::lock_timeout(&e).is_some() => None,
            Err(_) => Some(AppendLock { _file: None }),
        }
    }

    #[cfg(not(unix))]
    fn acquire_within(_log: &Path, _wait: std::time::Duration) -> Option<Self> {
        Some(AppendLock {})
    }
}

// ===== Whole-log replacement =====

/// How long a writer that replaces a whole state log (a header rewrite, a
/// cloud pull) waits for the log's append lock before it gives up with
/// [`StateLogBusy`], having written nothing.
pub const LOG_REPLACE_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// A writer that replaces a whole state log could not take the log's append
/// lock within its wait: another process was writing the log. Nothing was
/// written, and running the command again is safe.
#[derive(Debug)]
pub struct StateLogBusy {
    path: std::path::PathBuf,
    waited: std::time::Duration,
}

impl std::fmt::Display for StateLogBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "state log {} is busy: another koto process is writing it (waited {} ms); \
             nothing was written, run the command again",
            self.path.display(),
            self.waited.as_millis()
        )
    }
}

impl std::error::Error for StateLogBusy {}

impl StateLogBusy {
    /// The log at `path` stayed busy for `waited`: for a writer whose wait
    /// ran out on a lock it takes before the append lock, as
    /// `koto context add` takes the session's manifest lock.
    pub(crate) fn new(path: std::path::PathBuf, waited: std::time::Duration) -> Self {
        StateLogBusy { path, waited }
    }
}

/// The [`StateLogBusy`] `e` failed with, whatever context was added to it.
pub fn state_log_busy(e: &anyhow::Error) -> Option<&StateLogBusy> {
    e.chain()
        .find_map(|cause| cause.downcast_ref::<StateLogBusy>())
}

/// Run `f` holding the append lock of the log at `path`, the same sidecar
/// lock every append takes, waiting for it at most `wait`.
///
/// For a writer that reads the log and then replaces it whole: with the lock
/// held from the read to the replace, an append can't land in between and be
/// dropped by the replacement. A lock still held elsewhere once `wait` has
/// passed is [`StateLogBusy`], and `f` doesn't run. Where the lock can't be
/// had at all, `f` runs unlocked, as an append does.
///
/// `f` must not append to this log or take this lock again (see
/// [`HeldLogLock`]). A caller that already holds the lock runs its
/// replacement through [`with_held_log_lock`] instead.
pub fn with_log_replace_lock<T>(
    path: &Path,
    wait: std::time::Duration,
    f: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let _held = hold_log_lock(path, wait)?;
    f()
}

/// [`with_log_replace_lock`] for a caller that already holds the lock of the
/// log at `path` through `held`: `f` runs under it, with no second
/// acquisition. Refused, running nothing, when `held` locks another log.
pub fn with_held_log_lock<T>(
    held: &HeldLogLock,
    path: &Path,
    f: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    held.check(path)?;
    f()
}

/// The append lock of one log, held by this process until the guard is
/// dropped, for a caller that runs several steps on the log (a check, a
/// write elsewhere, an append) and must keep every other writer of it out
/// until the last one.
///
/// The steps take the guard rather than the lock. The lock belongs to the
/// open file, so a second acquisition in this process, on a file of its own,
/// waits on this one: an append's wait has no bound and never returns, and a
/// bounded wait runs out as [`StateLogBusy`]. Steps under the guard append
/// through [`append_event_tracked_held`] and replace the log through
/// [`with_held_log_lock`].
pub struct HeldLogLock {
    path: std::path::PathBuf,
    _lock: AppendLock,
}

impl HeldLogLock {
    /// The log whose lock this is.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Refuse a step on a log other than the one this guard locks: it would
    /// run unlocked while looking locked.
    fn check(&self, path: &Path) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.path == path,
            "internal error: the append lock held is that of {}, not of {}",
            self.path.display(),
            path.display()
        );
        Ok(())
    }
}

/// Take the append lock of the log at `path`, the same sidecar lock every
/// append takes, waiting for it at most `wait`, and hold it until the guard
/// is dropped. A lock still held elsewhere once `wait` has passed is
/// [`StateLogBusy`], with nothing done. Where the lock can't be had at all
/// (no `flock`, or no directory to put the sidecar in), the guard holds
/// nothing and the steps under it run unlocked, as an append does.
pub fn hold_log_lock(path: &Path, wait: std::time::Duration) -> anyhow::Result<HeldLogLock> {
    let started = std::time::Instant::now();
    let Some(lock) = AppendLock::acquire_within(path, wait) else {
        return Err(StateLogBusy {
            path: path.to_path_buf(),
            waited: started.elapsed(),
        }
        .into());
    };
    Ok(HeldLogLock {
        path: path.to_path_buf(),
        _lock: lock,
    })
}

/// [`hold_log_lock`] waiting as long as it takes, as a plain append waits:
/// for an append path with other work to do under the lock before the line
/// goes in.
pub(crate) fn hold_log_lock_blocking(path: &Path) -> HeldLogLock {
    HeldLogLock {
        path: path.to_path_buf(),
        _lock: AppendLock::acquire(path),
    }
}

/// Replace the log at `path` with `bytes`: written to a temp file beside it,
/// synced, then renamed over it, so a reader never finds it truncated or
/// half written. Call it under [`with_log_replace_lock`].
pub fn replace_log_bytes(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("state file path has no parent: {}", path.display()))?;
    let tmp = parent.join(format!(
        ".{}.tmp.{}",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("state"),
        std::process::id()
    ));
    {
        let mut opts = OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts
            .open(&tmp)
            .map_err(|e| anyhow::anyhow!("failed to open temp {}: {}", tmp.display(), e))?;
        file.write_all(bytes)
            .map_err(|e| anyhow::anyhow!("failed to write {}: {}", tmp.display(), e))?;
        file.sync_all()
            .map_err(|e| anyhow::anyhow!("failed to fsync {}: {}", tmp.display(), e))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        anyhow::anyhow!(
            "failed to rename {} -> {}: {}",
            tmp.display(),
            path.display(),
            e
        )
    })
}

/// Test support: take the append lock of the log at `log` on a file of its
/// own, as another process would, held until the file is dropped.
#[cfg(all(test, unix))]
pub(crate) fn hold_append_lock(log: &Path) -> std::fs::File {
    use std::os::fd::AsRawFd;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(log.parent().unwrap().join(APPEND_LOCK_FILE))
        .unwrap();
    // SAFETY: `fd` is borrowed from `file`, which outlives the call.
    assert_eq!(unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) }, 0);
    file
}

/// Test support: hold the append lock of the log at `log` for `hold` from
/// another thread, returning once it is held.
#[cfg(all(test, unix))]
pub(crate) fn hold_append_lock_for(
    log: &Path,
    hold: std::time::Duration,
) -> std::thread::JoinHandle<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let log = log.to_path_buf();
    let handle = std::thread::spawn(move || {
        let _lock = hold_append_lock(&log);
        tx.send(()).unwrap();
        std::thread::sleep(hold);
    });
    rx.recv().unwrap();
    handle
}

#[cfg(all(test, unix))]
mod log_replace_lock_tests {
    use super::*;
    use std::time::Duration;

    const HEADER: &str = r#"{"schema_version":1,"workflow":"wf","template_hash":"abc","created_at":"2025-01-01T00:00:00Z"}"#;

    /// A writer that replaces the log waits for the append lock, then
    /// reports the log busy, retryably, without running (koto#239).
    #[test]
    fn a_held_append_lock_makes_a_log_replacement_busy() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("koto-wf.state.jsonl");
        std::fs::write(&log, "{}\n").unwrap();
        let held = hold_append_lock(&log);
        let mut ran = false;
        let err = with_log_replace_lock(&log, Duration::from_millis(200), || {
            ran = true;
            Ok(())
        })
        .unwrap_err();
        assert!(!ran);
        assert!(state_log_busy(&err).is_some(), "{err:#}");
        assert!(err.to_string().contains("run the command again"), "{err}");
        drop(held);
        with_log_replace_lock(&log, Duration::from_millis(200), || Ok(())).unwrap();
    }

    /// An append waits for a replacement holding the lock, then lands in the
    /// replaced log rather than in the file the replacement renamed away.
    #[test]
    fn an_append_waits_for_a_replacement_and_lands_in_its_log() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("koto-wf.state.jsonl");
        std::fs::write(&log, format!("{HEADER}\n")).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let replacer = {
            let log = log.clone();
            std::thread::spawn(move || {
                with_log_replace_lock(&log, Duration::from_secs(5), || {
                    tx.send(()).unwrap();
                    std::thread::sleep(Duration::from_millis(200));
                    replace_log_bytes(&log, format!("{HEADER}\n").as_bytes())
                })
                .unwrap();
            })
        };
        rx.recv().unwrap();
        let payload = EventPayload::ContextAdded {
            key: "k".to_string(),
            hash: "0".repeat(64),
            size: 1,
            writer: None,
        };
        append_event(&log, &payload, "2025-01-01T00:00:00Z").unwrap();
        replacer.join().unwrap();
        let (_, events) = read_events(&log).unwrap();
        assert_eq!(events.len(), 1);
    }

    fn context_added(key: &str) -> EventPayload {
        EventPayload::ContextAdded {
            key: key.to_string(),
            hash: "0".repeat(64),
            size: 1,
            writer: None,
        }
    }

    /// Holding a log's lock waits for another holder, then reports the log
    /// busy, retryably, as a replacement does (koto#728).
    #[test]
    fn hold_log_lock_is_busy_while_another_process_holds_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("koto-wf.state.jsonl");
        std::fs::write(&log, format!("{HEADER}\n")).unwrap();
        let holder = hold_append_lock_for(&log, Duration::from_millis(600));
        let err = hold_log_lock(&log, Duration::from_millis(100))
            .err()
            .expect("the lock is held elsewhere");
        assert!(state_log_busy(&err).is_some(), "{err:#}");
        assert!(err.to_string().contains("is busy"), "{err}");
        holder.join().unwrap();
        let held = hold_log_lock(&log, Duration::from_millis(100)).unwrap();
        assert_eq!(held.path(), log.as_path());
    }

    /// An append through a held guard writes one line with the next seq,
    /// without waiting on the guard, and the lock stays held until the
    /// guard is dropped.
    #[test]
    fn an_append_through_a_held_guard_takes_the_next_seq() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("koto-wf.state.jsonl");
        std::fs::write(&log, format!("{HEADER}\n")).unwrap();
        append_event(&log, &context_added("a"), "2025-01-01T00:00:00Z").unwrap();

        let held = hold_log_lock(&log, Duration::from_millis(200)).unwrap();
        let commit =
            append_event_tracked_held(&held, &log, &context_added("b"), "2025-01-01T00:00:01Z")
                .unwrap();
        assert_eq!(commit.seq, 2);
        let replaced = with_log_replace_lock(&log, Duration::from_millis(100), || Ok(()));
        assert!(
            replaced
                .as_ref()
                .is_err_and(|e| state_log_busy(e).is_some()),
            "the guard still holds the lock: {replaced:?}"
        );
        drop(held);

        let content = std::fs::read_to_string(&log).unwrap();
        assert_eq!(content.lines().count(), 3, "{content}");
        assert!(content.ends_with('\n'));
        let (_, events) = read_events(&log).unwrap();
        let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![1, 2]);
        with_log_replace_lock(&log, Duration::from_millis(100), || Ok(())).unwrap();
    }

    /// A guard is the lock of one log: an append or a replacement of another
    /// log through it is refused, with nothing written or run.
    #[test]
    fn a_held_guard_for_another_log_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("koto-wf.state.jsonl");
        let other_dir = dir.path().join("other");
        std::fs::create_dir(&other_dir).unwrap();
        let other = other_dir.join("koto-other.state.jsonl");
        std::fs::write(&log, format!("{HEADER}\n")).unwrap();
        std::fs::write(&other, format!("{HEADER}\n")).unwrap();

        let held = hold_log_lock(&log, Duration::from_millis(200)).unwrap();
        let err =
            append_event_tracked_held(&held, &other, &context_added("k"), "2025-01-01T00:00:00Z")
                .unwrap_err();
        assert!(err.to_string().contains("not of"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&other).unwrap(),
            format!("{HEADER}\n")
        );
        let mut ran = false;
        with_held_log_lock(&held, &other, || {
            ran = true;
            Ok(())
        })
        .unwrap_err();
        assert!(!ran);
    }
}

/// How long `koto init --supersede` waits for a predecessor's append lock
/// before it gives up with `fence_unavailable`, having written nothing.
pub const FENCE_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// An append to a superseded session's log, refused with nothing written
/// (error kind `session_superseded`). `fence` is the session's own fence;
/// the CLI follows it forward to the newest session when it reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSupersededError {
    /// The session whose log refused the append.
    pub session: String,
    /// Its fence.
    pub fence: SupersedeFence,
}

impl std::fmt::Display for SessionSupersededError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A fence marked `replaced` was written by the close of a run another
        // run replaced, not by a supersede.
        let how = if self.fence.replaced {
            "replaced"
        } else {
            "superseded"
        };
        write!(
            f,
            "session '{}' was {how} by '{}' (session_superseded), so nothing was written; \
             continue as '{}'",
            self.session, self.fence.by, self.fence.by
        )
    }
}

impl std::error::Error for SessionSupersededError {}

/// The [`SessionSupersededError`] `e` failed with, whatever context was
/// added to it since.
pub fn session_superseded(e: &anyhow::Error) -> Option<&SessionSupersededError> {
    e.chain()
        .find_map(|cause| cause.downcast_ref::<SessionSupersededError>())
}

/// Refuse appending `payload` to a log whose bytes (`content`, read under
/// the append lock) hold a fence. A `context_read` passes: reads of a
/// superseded session keep working, and an anchored read records itself
/// in the session it read.
fn refuse_if_fenced(path: &Path, content: &str, payload: &EventPayload) -> anyhow::Result<()> {
    if matches!(payload, EventPayload::ContextRead { .. }) {
        return Ok(());
    }
    match superseded_by_in_log(content) {
        Some(fence) => Err(SessionSupersededError {
            session: extract_session_id_from_path(path),
            fence,
        }
        .into()),
        None => Ok(()),
    }
}

/// Refuse appending `payload` to a log whose header (the first line of
/// `content`, read under the append lock) records a whole-binding move that
/// fences this process off (`crate::session::fence_state::check`). A
/// `context_read` passes, as it does [`refuse_if_fenced`]: it records a
/// read, and reads of a session keep working wherever they run from.
///
/// Only `content`'s first line is read; `path` names the log whose first
/// read this process recorded, and is never opened. The cloud backend asks
/// it of the bucket's copy of a run it closes, as [`append_close`] asks it
/// of a local log.
pub(crate) fn refuse_if_rebound(
    path: &Path,
    content: &str,
    payload: &EventPayload,
) -> anyhow::Result<()> {
    if matches!(payload, EventPayload::ContextRead { .. }) {
        return Ok(());
    }
    let line = content.lines().next().unwrap_or_default();
    crate::session::fence_state::check(&extract_session_id_from_path(path), path, line)?;
    Ok(())
}

/// What [`append_supersede_fence`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FenceOutcome {
    /// The fence was appended at `seq`.
    Fenced { seq: u64 },
    /// The log already held this same fence (same successor name and
    /// session id): an earlier run of the same command wrote it.
    AlreadyOurs,
    /// The log already held a fence naming another successor, which won.
    SupersededByOther(SupersedeFence),
    /// The append lock was still held elsewhere when the wait ran out.
    /// Nothing was written.
    Unavailable,
}

/// Append `fence` to the session log at `path`, under the append lock every
/// append takes, held for this one append. The lock is waited for at most
/// `wait`; a lock still held then is [`FenceOutcome::Unavailable`] with
/// nothing written. Under the lock the log is checked for a fence again, so
/// of two successors the first append wins and the second reads it.
pub fn append_supersede_fence(
    path: &Path,
    fence: &SupersedeFence,
    timestamp: &str,
    wait: std::time::Duration,
) -> anyhow::Result<FenceOutcome> {
    let Some(_append_lock) = AppendLock::acquire_within(path, wait) else {
        return Ok(FenceOutcome::Unavailable);
    };
    let tail = prepare_append::<StateFileHeader>(path)?;
    if let Some(existing) = superseded_by_in_log(&tail.content) {
        return Ok(
            if existing.by == fence.by && existing.by_session_id == fence.by_session_id {
                FenceOutcome::AlreadyOurs
            } else {
                FenceOutcome::SupersededByOther(existing)
            },
        );
    }
    let payload = fence.to_payload();
    let event = Event {
        seq: tail.next_seq,
        timestamp: timestamp.to_string(),
        event_type: payload.type_name().to_string(),
        payload,
        idempotency_hash: None,
    };
    write_event_line(path, &event, tail.unterminated)?;
    Ok(FenceOutcome::Fenced { seq: tail.next_seq })
}

/// The fault-injection seam of `koto init --replaces`, for the binary's
/// tests: a debug build reads it, a release build never does, so it is not
/// a setting. Unset, nothing changes. [`append_close`] stops at
/// `torn-close`, having written half of what it owed.
pub(crate) const REPLACE_FAULT_ENV: &str = "KOTO_TEST_REPLACE_FAULT";

/// What [`append_close`] closes a replaced run's log with.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Close {
    /// Events owed ahead of the cancel, in order: a stopped runner's settle
    /// events and `pool_released` for each pool queue left. The caller works
    /// them out from the log; they are written as given.
    pub side_events: Vec<EventPayload>,
    /// Whether the run is owed a cancel. False for a run already in a
    /// terminal state, which gets the fence only.
    pub cancel: bool,
    /// The fence naming the replacement. It is written with `replaced`
    /// set, whatever this copy holds.
    pub fence: SupersedeFence,
}

/// What [`append_close`] did.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CloseOutcome {
    /// Every line owed went in, in one write. `commits` holds each event
    /// written with what its commit read, in order, for the after-commit
    /// steps; it is empty when nothing was owed. `torn_backup` is where the
    /// log was kept before a torn last line was trimmed.
    Closed {
        commits: Vec<(EventPayload, CommitInfo)>,
        torn_backup: Option<std::path::PathBuf>,
    },
    /// The log holds a fence other than this close's own replaced fence.
    /// Nothing was written.
    FencedByOther(SupersedeFence),
    /// The append lock was still held elsewhere when the wait ran out.
    /// Nothing was written.
    Unavailable,
}

/// Close the replaced run whose log is at `path`: append the events
/// `close` owes, the cancel and the replaced fence in one write and one
/// sync, under the log's append lock, waited for at most `wait`. The caller
/// holds the run's manifest lock and runs the after-commit steps for each of
/// the returned commits once it has released it.
///
/// What is owed is worked out from the bytes read under the lock, so a
/// re-run writes only what an earlier, interrupted one didn't. The cancel
/// is skipped when the last event that set where the run stands is already
/// a cancel; a cancel that a rewind followed doesn't count. The fence is
/// skipped when the log already holds it. The close may write past its own
/// replaced fence, never past any other.
///
/// A last line with no newline that doesn't parse is a write cut short. It
/// is trimmed, with the log as it was kept beside it as
/// `<log>.seq<N>-torn.bak`, and the close is written in its place. A last
/// line that ends in a newline and doesn't parse is refused, with nothing
/// written.
pub(crate) fn append_close(
    path: &Path,
    close: &Close,
    timestamp: &str,
    wait: std::time::Duration,
) -> anyhow::Result<CloseOutcome> {
    let Some(_append_lock) = AppendLock::acquire_within(path, wait) else {
        return Ok(CloseOutcome::Unavailable);
    };
    let (tail, torn) = prepare_close(path)?;
    let fence = SupersedeFence {
        replaced: true,
        ..close.fence.clone()
    };
    if let Some(existing) = superseded_by_in_log(&tail.content) {
        let ours = existing.replaced
            && existing.by == fence.by
            && existing.by_session_id == fence.by_session_id;
        if !ours {
            return Ok(CloseOutcome::FencedByOther(existing));
        }
    }
    refuse_if_rebound(path, &tail.content, &fence.to_payload())?;
    let payloads = close_payloads(&tail.content, close, &fence)?;
    if payloads.is_empty() && torn.is_none() {
        return Ok(CloseOutcome::Closed {
            commits: Vec::new(),
            torn_backup: None,
        });
    }

    let mut buf = String::new();
    if tail.unterminated {
        buf.push('\n');
    }
    let mut commits = Vec::with_capacity(payloads.len());
    let mut prior_log = tail.content;
    for (i, payload) in payloads.into_iter().enumerate() {
        let event = Event {
            seq: tail.next_seq + i as u64,
            timestamp: timestamp.to_string(),
            event_type: payload.type_name().to_string(),
            payload,
            idempotency_hash: None,
        };
        let line = serde_json::to_string(&event)
            .map_err(|e| anyhow::anyhow!("failed to serialize event: {}", e))?;
        buf.push_str(&line);
        buf.push('\n');
        // The first line follows the newline an unterminated log was owed.
        let sep = if i == 0 && tail.unterminated {
            "\n"
        } else {
            ""
        };
        let next_prior = format!("{prior_log}{sep}{line}\n");
        commits.push((
            event.payload,
            CommitInfo {
                seq: event.seq,
                prior_log,
            },
        ));
        prior_log = next_prior;
    }

    if cfg!(debug_assertions) && std::env::var(REPLACE_FAULT_ENV).as_deref() == Ok("torn-close") {
        tear_close(path, &buf);
    }
    let torn_backup = match torn {
        Some(torn) => {
            let backup = write_backup(path, &torn.log, tail.next_seq, "torn")?;
            rewrite_tail(path, torn.offset, &buf, "torn")?;
            eprintln!(
                "warning: the last line of {} was cut short partway through a write; trimmed \
                 it. The log as it was is kept at {}",
                path.display(),
                backup.display()
            );
            Some(backup)
        }
        None => {
            append_bytes(path, &buf)?;
            None
        }
    };
    Ok(CloseOutcome::Closed {
        commits,
        torn_backup,
    })
}

/// The `torn-close` fault point: append the first half of `buf`, cut inside
/// a line so the log ends in a write cut short, and stop as a killed
/// process would.
fn tear_close(path: &Path, buf: &str) -> ! {
    let mut cut = buf.len() / 2;
    if buf[..cut].ends_with('\n') {
        cut = cut.saturating_sub(2);
    }
    while !buf.is_char_boundary(cut) {
        cut -= 1;
    }
    let _ = append_bytes(path, &buf[..cut]);
    eprintln!("{REPLACE_FAULT_ENV}: stopping at torn-close");
    crate::exit::crash(86)
}

/// The events [`append_close`] owes the log whose bytes are `content`, in
/// the order they go in: the side events, the cancel unless the run already
/// stands cancelled, then `fence` unless the log holds it. The cloud
/// backend works out the lines it puts in a bucket copy with it too.
pub(crate) fn close_payloads(
    content: &str,
    close: &Close,
    fence: &SupersedeFence,
) -> anyhow::Result<Vec<EventPayload>> {
    let (_, events) = parse_state_log(content.as_bytes())?;
    let mut out = close.side_events.clone();
    let stands_cancelled = crate::session::inventory::last_standing_event(&events)
        .is_some_and(|e| matches!(e.payload, EventPayload::WorkflowCancelled { .. }));
    if close.cancel && !stands_cancelled {
        let state = derive_state_from_log(&events).ok_or_else(|| {
            EngineError::StateFileCorrupted(
                "cannot derive the current state, so the run was not closed".to_string(),
            )
        })?;
        out.push(EventPayload::WorkflowCancelled {
            state,
            reason: format!("replaced by {}", fence.by),
            replaced_by: Some(fence.by.clone()),
            replaced_by_session_id: Some(fence.by_session_id.clone()),
        });
    }
    if superseded_by_in_log(content).is_none() {
        out.push(fence.to_payload());
    }
    Ok(out)
}

/// A last line [`prepare_close`] will trim: it starts at byte `offset` of
/// `log`, the log's bytes as read.
struct TornTail {
    offset: usize,
    log: String,
}

/// [`prepare_append`] for [`append_close`]: the same read, header check
/// and merged-line split, except that a torn last line is not refused. It
/// comes back as the log's bytes without that line, with where the line
/// starts, and nothing is written for it yet.
fn prepare_close(path: &Path) -> anyhow::Result<(AppendTail, Option<TornTail>)> {
    let content = read_for_append(path)?;
    let refused = plan_append::<StateFileHeader>(&content, path).err();
    let Some(err) = refused else {
        return Ok((tail_for_append::<StateFileHeader>(path, content)?, None));
    };
    let Some(offset) = torn_tail_offset(&content) else {
        return Err(err);
    };
    let next_seq = match plan_append::<StateFileHeader>(&content[..offset], path) {
        Ok(AppendPlan {
            next_seq,
            merged: None,
            ..
        }) => next_seq,
        _ => return Err(err),
    };
    let tail = AppendTail {
        next_seq,
        content: content[..offset].to_string(),
        unterminated: false,
    };
    Ok((
        tail,
        Some(TornTail {
            offset,
            log: content,
        }),
    ))
}

/// Where the last line of `content` starts when it is a write cut short:
/// it has no newline, it isn't the header, and it doesn't parse, as one
/// value or as several written together.
fn torn_tail_offset(content: &str) -> Option<usize> {
    if content.ends_with('\n') {
        return None;
    }
    let offset = content.rfind('\n')? + 1;
    let line = content[offset..].trim();
    if line.is_empty() {
        return None;
    }
    let mut values = serde_json::Deserializer::from_str(line).into_iter::<serde_json::Value>();
    if values.all(|v| v.is_ok()) {
        return None;
    }
    Some(offset)
}

/// Append `bytes` to the log at `path` in one write, and sync it.
fn append_bytes(path: &Path, bytes: &str) -> anyhow::Result<()> {
    let mut file = OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|e| anyhow::anyhow!("failed to open state file {}: {}", path.display(), e))?;
    file.write_all(bytes.as_bytes())
        .map_err(|e| anyhow::anyhow!("failed to write events to {}: {}", path.display(), e))?;
    file.sync_data()
        .map_err(|e| anyhow::anyhow!("failed to sync state file {}: {}", path.display(), e))?;
    Ok(())
}

/// Acquire an exclusive `flock(LOCK_EX)` on the state file. The lock
/// is released when the returned `File` is dropped.
///
/// Used by [`append_event_idempotent`] to serialize the read-then-write
/// window so concurrent identical retries collapse to a single write
/// rather than racing past the hash scan.
///
/// # Lock order and the invariant it relies on
///
/// [`append_event_idempotent_in`] takes this lock first and the sidecar
/// [`AppendLock`] second. This is the same file
/// [`SessionBackend::lock_state_file`](crate::session::SessionBackend::lock_state_file)
/// locks for a whole batch tick, and `flock` locks belong to the open file,
/// not the process: a thread holding that tick lock that reached this
/// blocking call on the same state file would wait on itself forever. No
/// production caller does -- the only idempotent append today writes a
/// request store's log, a different file -- and nothing may start to: an
/// idempotent append to a session's state file must not run on a thread
/// holding that session's tick lock. Debug builds check this with
/// [`held_state_locks`] and panic instead of hanging.
#[cfg(unix)]
fn acquire_state_flock(path: &Path) -> anyhow::Result<std::fs::File> {
    use std::os::fd::AsRawFd;
    #[cfg(debug_assertions)]
    debug_assert!(
        !held_state_locks::held_by_this_thread(path),
        "acquire_state_flock on {} while this thread holds its lock_state_file lock \
         would deadlock",
        path.display()
    );
    // Open without `create`: a log is created when its session is created,
    // and a lock open that created the file would leave an empty log behind
    // for the header check in `prepare_append` to refuse.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| {
            anyhow::anyhow!(
                "failed to open state file for lock {}: {}",
                path.display(),
                e
            )
        })?;
    let fd = file.as_raw_fd();
    loop {
        let ret = unsafe { libc::flock(fd, libc::LOCK_EX) };
        if ret == 0 {
            return Ok(file);
        }
        // A signal during the wait is not a failure to lock: wait again.
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::Interrupted {
            return Err(anyhow::anyhow!(
                "failed to acquire flock on {}: {}",
                path.display(),
                err
            ));
        }
    }
}

#[cfg(not(unix))]
fn acquire_state_flock(_path: &Path) -> anyhow::Result<std::fs::File> {
    // Non-unix targets: idempotency check is best-effort without a
    // cross-process lock. Concurrent retries on Windows are extremely
    // unlikely in koto's single-coordinator model; falling through
    // produces correct semantics in the no-contention case.
    Err(anyhow::anyhow!("flock not available on this platform"))
}

/// Debug-build record of which threads hold a
/// [`SessionBackend::lock_state_file`](crate::session::SessionBackend::lock_state_file)
/// lock on which state file, so [`acquire_state_flock`] can refuse the
/// same-thread re-lock that would deadlock. Release builds carry none of it.
#[cfg(debug_assertions)]
pub(crate) mod held_state_locks {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::thread::ThreadId;

    static HELD: Mutex<Vec<(ThreadId, PathBuf)>> = Mutex::new(Vec::new());

    fn identity(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    /// Marks `path` as locked by the current thread until dropped. Dropping
    /// it on another thread still removes the right entry.
    #[derive(Debug)]
    pub(crate) struct Held {
        entry: (ThreadId, PathBuf),
    }

    pub(crate) fn hold(path: &Path) -> Held {
        let entry = (std::thread::current().id(), identity(path));
        if let Ok(mut held) = HELD.lock() {
            held.push(entry.clone());
        }
        Held { entry }
    }

    impl Drop for Held {
        fn drop(&mut self) {
            if let Ok(mut held) = HELD.lock() {
                if let Some(i) = held.iter().position(|e| *e == self.entry) {
                    held.swap_remove(i);
                }
            }
        }
    }

    pub(crate) fn held_by_this_thread(path: &Path) -> bool {
        let entry = (std::thread::current().id(), identity(path));
        HELD.lock().map(|h| h.contains(&entry)).unwrap_or(false)
    }
}

/// Extract the session id (workflow name) from a state file path.
///
/// State files live at `<sessions_root>/<session_id>/koto-<session_id>.state.jsonl`.
/// The session id is the file's parent directory's basename. Used by
/// [`append_event_idempotent`] to populate the
/// [`EngineError::ConcurrentSubmissionConflict`] envelope; falls back to
/// `"<unknown>"` if the path doesn't follow the convention.
fn extract_session_id_from_path(path: &Path) -> String {
    path.parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "<unknown>".to_string())
}

/// fsync the three log files that must be durable BEFORE the
/// coordinator's `substrate_wake()` call (R19 / Issue 12 Decision 2
/// sub-question 2):
///
/// 1. The child's terminal-evidence log
/// 2. The coordinator's log (which carries the appended `ChildDispatched` event)
/// 3. The coordinator's log (which carries the appended `RequesterWoken` event)
///
/// Points 2 and 3 are the same file; the helper fsyncs it twice to
/// reflect the design's three-point discipline. The fsync of an
/// already-fsync'd file is a benign no-op on Linux/macOS.
///
/// `child_log_path` and `coord_log_path` are caller-supplied so the
/// helper can be unit-tested without a full backend; the wake-emission
/// path (Issue 15) resolves them from the backend's `session_dir(id)`.
///
/// Returns an error if either fsync fails; the wake-delivery primitive
/// must NOT be invoked when this helper returns `Err`.
pub fn fsync_wake_preconditions(
    child_log_path: &Path,
    coord_log_path: &Path,
) -> anyhow::Result<()> {
    fsync_log_file(child_log_path).map_err(|e| {
        anyhow::anyhow!(
            "fsync_wake_preconditions: child log {} fsync failed: {}",
            child_log_path.display(),
            e
        )
    })?;
    fsync_log_file(coord_log_path).map_err(|e| {
        anyhow::anyhow!(
            "fsync_wake_preconditions: coord log {} (post-ChildDispatched) fsync failed: {}",
            coord_log_path.display(),
            e
        )
    })?;
    fsync_log_file(coord_log_path).map_err(|e| {
        anyhow::anyhow!(
            "fsync_wake_preconditions: coord log {} (post-RequesterWoken) fsync failed: {}",
            coord_log_path.display(),
            e
        )
    })?;
    Ok(())
}

/// Open `path` and call `sync_all`. Missing files are NOT tolerated —
/// the caller must guarantee the log has been initialized before
/// invoking `fsync_wake_preconditions`.
fn fsync_log_file(path: &Path) -> std::io::Result<()> {
    let file = std::fs::File::open(path)?;
    file.sync_all()
}

/// What an append needs to know about the log it is about to extend, read
/// under the append lock.
struct AppendTail {
    /// The seq the appended event takes.
    next_seq: u64,
    /// The log's bytes as the append found them, or as the split of a merged
    /// last line left them.
    content: String,
    /// The log's last line has no newline: a complete event from a writer
    /// that died between writing a line and its `\n`, before appends became
    /// one write. The append supplies the newline first.
    unterminated: bool,
}

/// Read the log at `path` for an append: the seq the next event gets and the
/// bytes read, refusing a log that has no header. Usually only a read, but it
/// can write: see the split below. The caller must hold the append lock.
///
/// The file must exist and its first line must parse as an `H`. Checking
/// the header, rather than skipping line 1 unseen, is what stops an append
/// from ever producing a log koto can't read: a missing or empty file, or
/// one whose first line is an event, is refused before anything is
/// written. Skipping it unseen also made a headerless log hide its own
/// first event, so the next append reused `seq: 1` (koto#236).
///
/// Reads only the last non-empty line's seq, since in a valid file seq
/// equals line index. This avoids silently masking corruption where events
/// appear out of order.
///
/// A last line that doesn't parse fails the append, as a truncated one
/// always has, with one exception: a line holding two or more well-formed
/// events written with no newline between them (koto#419), whose seqs follow
/// the line before (the one case where that line's seq is read too). That
/// line is split on disk into one line per event, with the log as it was
/// kept beside it, and the append goes on (see [`split_merged_last_line`]).
/// Merged events whose seqs don't follow, or whose line before can't be
/// read, are refused and left as they are.
fn prepare_append<H: LogHeader>(path: &Path) -> anyhow::Result<AppendTail> {
    let content = read_for_append(path)?;
    tail_for_append::<H>(path, content)
}

/// The read [`prepare_append`] starts with: the log's bytes, or an error
/// naming a log that doesn't exist.
fn read_for_append(path: &Path) -> anyhow::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(anyhow::anyhow!(
            "state file {} does not exist, so nothing was appended; a session's log is written when the session is created, never by an append",
            path.display()
        )),
        Err(e) => Err(anyhow::anyhow!(
            "failed to read state file {}: {}",
            path.display(),
            e
        )),
    }
}

/// The rest of [`prepare_append`], on the bytes it read.
fn tail_for_append<H: LogHeader>(path: &Path, content: String) -> anyhow::Result<AppendTail> {
    let AppendPlan {
        next_seq,
        unterminated,
        merged,
    } = plan_append::<H>(&content, path)?;
    let Some(merged) = merged else {
        return Ok(AppendTail {
            next_seq,
            content,
            unterminated,
        });
    };
    let content = split_merged_last_line(
        path,
        &content,
        merged.offset,
        merged.line_number,
        &merged.pieces,
    )?;
    Ok(AppendTail {
        next_seq,
        content,
        unterminated: false,
    })
}

/// Split the merged last line of the log at `path`, which starts at byte
/// `offset` of `content` and holds `pieces`, into one line per event, and
/// return the log's new bytes.
///
/// The log as it was goes first to a backup beside it, named after the
/// log and the first merged seq (`<log>.seq<N>-merged.bak`), so the repair
/// can be checked or undone by hand. Each event keeps the exact text it was
/// written with. The caller holds the append lock, so no other append runs
/// in between; what was done is announced on stderr.
///
/// The split is written over the merged line in place, not by renaming a
/// new file over the log. A `koto next` holds `lock_state_file`'s `flock` on
/// the log's own inode for its whole tick, and a rename would leave that
/// lock on an orphaned inode while the next tick locked the new file at
/// once. Everything before the merged line is untouched, and the new tail is
/// written in one write. A lock-free reader that lands in the middle of it
/// sees a final line it can't parse and recovers the way it does from any
/// torn tail; a crash in the middle leaves the backup to restore from.
fn split_merged_last_line(
    path: &Path,
    content: &str,
    offset: usize,
    line_number: usize,
    pieces: &[(&str, Event)],
) -> anyhow::Result<String> {
    let mut tail = String::with_capacity(content.len() - offset + pieces.len());
    for (text, _) in pieces {
        tail.push_str(text);
        tail.push('\n');
    }

    let first_seq = pieces[0].1.seq;
    let backup = write_backup(path, content, first_seq, "merged")?;
    rewrite_tail(path, offset, &tail, "merged")?;
    let mut split = String::with_capacity(offset + tail.len());
    split.push_str(&content[..offset]);
    split.push_str(&tail);
    let seqs: Vec<u64> = pieces.iter().map(|(_, e)| e.seq).collect();
    eprintln!(
        "warning: line {} of {} held {} events written with no newline between them (seq {}); \
         split it into one line per event. The log as it was is kept at {}",
        line_number,
        path.display(),
        pieces.len(),
        join_seqs(&seqs),
        backup.display()
    );
    Ok(split)
}

/// Write `content` to a new file beside `log`, named for `first_seq` and
/// the repair it is kept for (`kind`: `merged` or `torn`), and sync it.
/// Never overwrites: a name already taken gets a numeric suffix.
fn write_backup(
    log: &Path,
    content: &str,
    first_seq: u64,
    kind: &str,
) -> anyhow::Result<std::path::PathBuf> {
    let name = log
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("state.jsonl");
    for n in 0..100u32 {
        let suffix = if n == 0 {
            String::new()
        } else {
            format!(".{n}")
        };
        let backup = log.with_file_name(format!("{name}.seq{first_seq}-{kind}{suffix}.bak"));
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = match opts.open(&backup) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                let repair = if kind == "torn" {
                    "trimming a torn last line"
                } else {
                    "splitting a merged line"
                };
                return Err(anyhow::anyhow!(
                    "failed to create backup {} before {repair}: {}",
                    backup.display(),
                    e
                ));
            }
        };
        file.write_all(content.as_bytes())
            .and_then(|()| file.sync_data())
            .map_err(|e| anyhow::anyhow!("failed to write backup {}: {}", backup.display(), e))?;
        return Ok(backup);
    }
    Err(anyhow::anyhow!(
        "no free backup name beside {} for seq {}",
        log.display(),
        first_seq
    ))
}

/// Write `tail` over the file at `path` from byte `offset` on, in one write,
/// cut the file to end where `tail` does, and sync it. The file keeps its
/// inode, mode and every byte before `offset`. `kind` (`merged` or `torn`)
/// names the repair in the error.
fn rewrite_tail(path: &Path, offset: usize, tail: &str, kind: &str) -> anyhow::Result<()> {
    use std::io::{Seek, SeekFrom};
    let mut file = OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| anyhow::anyhow!("failed to open state file {}: {}", path.display(), e))?;
    let end = (offset + tail.len()) as u64;
    file.seek(SeekFrom::Start(offset as u64))
        .and_then(|_| file.write_all(tail.as_bytes()))
        .and_then(|()| file.set_len(end))
        .and_then(|()| file.sync_data())
        .map_err(|e| {
            let what = if kind == "torn" {
                "trim the torn last line"
            } else {
                "split the last line"
            };
            anyhow::anyhow!("failed to {what} of {}: {}", path.display(), e)
        })
}

#[cfg(test)]
mod close_tests {
    use super::*;
    use crate::engine::types::SupersedeActor;
    use std::time::Duration;

    const HEADER: &str = r#"{"schema_version":1,"workflow":"wf","template_hash":"abc","created_at":"2025-01-01T00:00:00Z"}"#;
    const TS: &str = "2026-01-01T00:00:00Z";
    const WAIT: Duration = Duration::from_secs(5);

    /// The log of a run `old` standing in `work`, in a directory of `dir`.
    fn started_log(dir: &Path) -> std::path::PathBuf {
        let session = dir.join("old");
        std::fs::create_dir_all(&session).unwrap();
        let log = session.join("koto-old.state.jsonl");
        std::fs::write(&log, format!("{HEADER}\n")).unwrap();
        let to_work = EventPayload::Transitioned {
            from: None,
            to: "work".to_string(),
            condition_type: "auto".to_string(),
            skip_if_matched: None,
            context_assignments: None,
            vars_matched: None,
        };
        append_event(&log, &to_work, TS).unwrap();
        log
    }

    fn close() -> Close {
        Close {
            side_events: Vec::new(),
            cancel: true,
            fence: SupersedeFence {
                by: "new".to_string(),
                by_session_id: "id-new".to_string(),
                actor: SupersedeActor::default(),
                at: TS.to_string(),
                replaced: true,
            },
        }
    }

    /// The backups beside `log`, by name.
    fn backups(log: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(log.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".bak"))
            .collect();
        names.sort();
        names
    }

    fn closed(
        outcome: CloseOutcome,
    ) -> (Vec<(EventPayload, CommitInfo)>, Option<std::path::PathBuf>) {
        match outcome {
            CloseOutcome::Closed {
                commits,
                torn_backup,
            } => (commits, torn_backup),
            other => panic!("expected the close to be written, got {other:?}"),
        }
    }

    /// A last line that ends in a newline and doesn't parse isn't a write
    /// cut short: the close refuses it and writes nothing.
    #[test]
    fn append_close_refuses_a_terminated_unparseable_last_line() {
        let dir = tempfile::tempdir().unwrap();
        let log = started_log(dir.path());
        let mut bytes = std::fs::read_to_string(&log).unwrap();
        bytes.push_str("{\"seq\":2,\"timest\n");
        std::fs::write(&log, &bytes).unwrap();

        let err = append_close(&log, &close(), TS, WAIT).unwrap_err();
        assert!(
            err.to_string().contains("failed to parse last event line"),
            "{err:#}"
        );
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            bytes,
            "nothing was written"
        );
        assert!(backups(&log).is_empty(), "{:?}", backups(&log));
    }

    /// A last line cut short partway through a write is trimmed, with the
    /// log as it was kept in a `torn` backup, and the close goes in its
    /// place. The log reads cleanly after, and a later append lands.
    #[test]
    fn append_close_trims_a_torn_last_line_with_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let log = started_log(dir.path());
        let mut bytes = std::fs::read_to_string(&log).unwrap();
        bytes.push_str(r#"{"seq":2,"timestamp":"2026-01-01T00:00:00Z","type":"intent_upd"#);
        std::fs::write(&log, &bytes).unwrap();

        let (commits, torn_backup) = closed(append_close(&log, &close(), TS, WAIT).unwrap());
        let backup = torn_backup.expect("a torn backup");
        assert_eq!(
            backup.file_name().unwrap().to_string_lossy(),
            "koto-old.state.jsonl.seq2-torn.bak"
        );
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), bytes);
        assert_eq!(backups(&log), vec!["koto-old.state.jsonl.seq2-torn.bak"]);

        let seqs: Vec<u64> = commits.iter().map(|(_, c)| c.seq).collect();
        assert_eq!(seqs, vec![2, 3]);
        match &commits[0].0 {
            EventPayload::WorkflowCancelled {
                state,
                reason,
                replaced_by,
                replaced_by_session_id,
            } => {
                assert_eq!(state, "work");
                assert_eq!(reason, "replaced by new");
                assert_eq!(replaced_by.as_deref(), Some("new"));
                assert_eq!(replaced_by_session_id.as_deref(), Some("id-new"));
            }
            other => panic!("expected a cancel first, got {other:?}"),
        }

        let (_, events) = read_events(&log).unwrap();
        assert_eq!(events.len(), 3);
        let fence = superseded_by(&events).expect("a fence");
        assert!(fence.replaced);
        assert_eq!(fence.by, "new");
        assert!(std::fs::read_to_string(&log).unwrap().ends_with('\n'));

        let read = EventPayload::ContextRead {
            key: "record/a".to_string(),
            reader: "cli".to_string(),
            state: "work".to_string(),
            present: true,
            hash: None,
            access: None,
            gate: None,
            session: None,
        };
        assert_eq!(append_event(&log, &read, TS).unwrap(), 4);
    }

    /// The close writes its side events, the cancel and the fence once; a
    /// re-run with nothing owed writes nothing, and each commit's prior log
    /// is the log up to its own line.
    #[test]
    fn append_close_writes_only_what_is_owed() {
        let dir = tempfile::tempdir().unwrap();
        let log = started_log(dir.path());
        let mut first = close();
        first.side_events = vec![EventPayload::IntentUpdated {
            intent: "side".to_string(),
        }];

        let (commits, torn_backup) = closed(append_close(&log, &first, TS, WAIT).unwrap());
        assert_eq!(torn_backup, None);
        let types: Vec<&str> = commits.iter().map(|(p, _)| p.type_name()).collect();
        assert_eq!(
            types,
            vec!["intent_updated", "workflow_cancelled", "session_superseded"]
        );
        let after = std::fs::read_to_string(&log).unwrap();
        for (_, commit) in &commits {
            assert!(after.starts_with(&commit.prior_log));
            assert_eq!(last_seq(&commit.prior_log), Some(commit.seq - 1));
        }

        let (commits, _) = closed(append_close(&log, &close(), TS, WAIT).unwrap());
        assert!(commits.is_empty(), "{commits:?}");
        assert_eq!(std::fs::read_to_string(&log).unwrap(), after);
        assert!(backups(&log).is_empty());
    }

    /// A cancel that a rewind followed doesn't count: the close writes a
    /// fresh one. A run owed no cancel gets the fence only.
    #[test]
    fn append_close_cancels_a_rewound_run_again_and_fences_a_finished_one_only() {
        let dir = tempfile::tempdir().unwrap();
        let log = started_log(dir.path());
        let cancel = EventPayload::WorkflowCancelled {
            state: "work".to_string(),
            reason: "cancelled by user".to_string(),
            replaced_by: None,
            replaced_by_session_id: None,
        };
        let rewind = EventPayload::Rewound {
            from: "work".to_string(),
            to: "work".to_string(),
            rationale: None,
        };
        append_event(&log, &cancel, TS).unwrap();
        append_event(&log, &rewind, TS).unwrap();
        let (commits, _) = closed(append_close(&log, &close(), TS, WAIT).unwrap());
        let types: Vec<&str> = commits.iter().map(|(p, _)| p.type_name()).collect();
        assert_eq!(types, vec!["workflow_cancelled", "session_superseded"]);

        let other = tempfile::tempdir().unwrap();
        let log = started_log(other.path());
        let mut finished = close();
        finished.cancel = false;
        let (commits, _) = closed(append_close(&log, &finished, TS, WAIT).unwrap());
        let types: Vec<&str> = commits.iter().map(|(p, _)| p.type_name()).collect();
        assert_eq!(types, vec!["session_superseded"]);
    }

    /// A log fenced by anything but this close's own replaced fence, a
    /// plain supersede fence naming the same successor included, is left
    /// as it is.
    #[test]
    fn append_close_refuses_a_log_fenced_by_another_fence() {
        let dir = tempfile::tempdir().unwrap();
        let log = started_log(dir.path());
        let plain = SupersedeFence {
            replaced: false,
            ..close().fence
        };
        append_supersede_fence(&log, &plain, TS, WAIT).unwrap();
        let before = std::fs::read_to_string(&log).unwrap();

        let outcome = append_close(&log, &close(), TS, WAIT).unwrap();
        assert_eq!(outcome, CloseOutcome::FencedByOther(plain));
        assert_eq!(std::fs::read_to_string(&log).unwrap(), before);
    }

    /// A whole-binding move recorded in the header after this process first
    /// read the log refuses the close, with nothing written.
    #[test]
    fn append_close_refuses_a_log_rebound_after_its_first_read() {
        let dir = tempfile::tempdir().unwrap();
        let log = started_log(dir.path());
        crate::session::fence_state::note_read(&log, 0);
        let content = std::fs::read_to_string(&log).unwrap();
        let (first, rest) = content.split_once('\n').unwrap();
        let mut header: serde_json::Value = serde_json::from_str(first).unwrap();
        let new = dir.path().join("ws").join("new");
        header["execution_dir"] = serde_json::json!(new);
        header["origin"] = serde_json::json!({
            "anchor": new,
            "store": {"kind": "local", "base": dir.path()},
            "anchor_epoch": 1,
            "whole_epoch": 1,
            "retired_anchors": [dir.path().join("ws").join("old")],
        });
        std::fs::write(&log, format!("{header}\n{rest}")).unwrap();
        let before = std::fs::read(&log).unwrap();

        let err =
            crate::session::fence_state::with_cwd(&new, || append_close(&log, &close(), TS, WAIT))
                .unwrap_err();
        let rebound = err
            .chain()
            .find_map(|c| c.downcast_ref::<crate::session::rebind_fence::SessionRebound>())
            .unwrap_or_else(|| panic!("not session_rebound: {err:#}"));
        assert!(!rebound.from_retired, "{err:#}");
        assert_eq!(std::fs::read(&log).unwrap(), before, "nothing was written");
    }

    /// The log of `old` after a whole-binding move from a directory that
    /// no longer exists to `anchor`, at epoch 1: the move event, and a
    /// header recording it, whose `whole_parts_owed` field (the mark a
    /// whole move's commit writes) is set when `owed` is. `torn` leaves the
    /// header at epoch 0, as a commit stopped between its event and its
    /// header does.
    fn moved_log(dir: &Path, anchor: &Path, owed: bool, torn: bool) -> std::path::PathBuf {
        let session = dir.join("old");
        std::fs::create_dir_all(&session).unwrap();
        let log = session.join("koto-old.state.jsonl");
        let mut origin = serde_json::json!({
            "anchor": anchor,
            "store": {"kind": "local", "base": dir},
        });
        if !torn {
            origin["anchor_epoch"] = 1.into();
            origin["whole_epoch"] = 1.into();
            origin["retired_anchors"] = serde_json::json!(["/koto-test-gone/old"]);
            if owed {
                origin["whole_parts_owed"] = true.into();
            }
        }
        let execution_dir = if torn {
            serde_json::json!("/koto-test-gone/old")
        } else {
            serde_json::json!(anchor)
        };
        let header = serde_json::json!({
            "schema_version": 1, "workflow": "wf", "template_hash": "abc",
            "created_at": "2025-01-01T00:00:00Z",
            "execution_dir": execution_dir,
            "origin": origin,
        });
        std::fs::write(&log, format!("{header}\n")).unwrap();
        let to_work = EventPayload::Transitioned {
            from: None,
            to: "work".to_string(),
            condition_type: "auto".to_string(),
            skip_if_matched: None,
            context_assignments: None,
            vars_matched: None,
        };
        append_event(&log, &to_work, TS).unwrap();
        let moved = EventPayload::ExecutionAnchorReboundWhole(crate::engine::types::WholeRebound {
            from: Some("/koto-test-gone/old".into()),
            to: anchor.to_path_buf(),
            whole: true,
            epoch: Some(1),
        });
        append_event(&log, &moved, TS).unwrap();
        log
    }

    #[test]
    fn append_close_refuses_rebind_incomplete_while_a_whole_move_owes_its_parts() {
        let dir = tempfile::tempdir().unwrap();
        let anchor = dir.path().join("a");
        std::fs::create_dir_all(&anchor).unwrap();
        let log = moved_log(dir.path(), &anchor, true, false);
        let before = std::fs::read_to_string(&log).unwrap();
        let e = crate::session::fence_state::with_cwd(&anchor, || {
            append_close(&log, &close(), TS, WAIT)
        })
        .unwrap_err();
        assert!(format!("{e:#}").contains("rebind_incomplete"), "{e:#}");
        assert_eq!(std::fs::read_to_string(&log).unwrap(), before);

        // The same log with the mark clear closes.
        let dir = tempfile::tempdir().unwrap();
        let anchor = dir.path().join("a");
        std::fs::create_dir_all(&anchor).unwrap();
        let log = moved_log(dir.path(), &anchor, false, false);
        crate::session::fence_state::with_cwd(&anchor, || append_close(&log, &close(), TS, WAIT))
            .unwrap();
    }

    #[test]
    fn append_close_refuses_rebind_incomplete_for_a_torn_whole_move() {
        let dir = tempfile::tempdir().unwrap();
        let anchor = dir.path().join("a");
        std::fs::create_dir_all(&anchor).unwrap();
        let log = moved_log(dir.path(), &anchor, false, true);
        let before = std::fs::read_to_string(&log).unwrap();
        let e = crate::session::fence_state::with_cwd(&anchor, || {
            append_close(&log, &close(), TS, WAIT)
        })
        .unwrap_err();
        assert!(format!("{e:#}").contains("rebind_incomplete"), "{e:#}");
        assert_eq!(std::fs::read_to_string(&log).unwrap(), before);
    }
}
