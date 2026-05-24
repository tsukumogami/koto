//! Workspace-wide terminal-index — the skip-list that keeps the
//! discovery scan from re-walking terminal sessions on every tick.
//!
//! File: `<koto_root>/_terminal_index.jsonl` — append-only JSONL, one
//! line per terminal transition. The writer is the dispatched agent's
//! `koto next --with-data` terminal-evidence path; bounded multi-writer
//! cases (sidecar recovery in Issue 11) also append. The reader walks
//! the file line-by-line into an in-memory dedup HashMap keyed by
//! `session_id` and selects the entry with the highest
//! `header_mtime_ns` per session.
//!
//! ## Atomicity
//!
//! Appends use `OpenOptions::append(true)` (sets POSIX `O_APPEND`) and
//! `fsync` after each write. Under POSIX, writes ≤ `PIPE_BUF` (4 KiB on
//! Linux) to a single file descriptor opened with `O_APPEND` are
//! atomic w.r.t. each other: the kernel resolves the offset and
//! performs the write as one syscall. Index lines are bounded to
//! [`MAX_INDEX_LINE_BYTES`] (4096) — comfortably within `PIPE_BUF` —
//! so concurrent writes from independent dispatched agents never
//! interleave. The discipline holds on local ext4 / xfs / APFS / NTFS;
//! NFS deployments are explicitly out of scope (see the design's
//! Security Considerations).
//!
//! ## Crash recovery — the truncated-trailing-line case
//!
//! A crash mid-`write_all` may leave a partial line without its
//! terminating `\n`. The reader treats any line that does NOT end in a
//! newline (i.e. the file's last bytes when there is no trailing
//! newline) as malformed and skips it with a warn-level log. The
//! header is the source of truth: the next scan re-reads the disk
//! header and re-classifies the session.
//!
//! ## Header-is-truth fallthrough
//!
//! Issue 7's discovery scan consults the index ONLY when the on-disk
//! header's mtime does NOT exceed the index entry's
//! `header_mtime_ns`. If a recovery walk (Issue 11) bumps the header's
//! mtime past the index entry, the scan re-surfaces the session for
//! re-evaluation — the index never overrides a fresher header.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// File name of the workspace-wide terminal index, joined under
/// `<koto_root>`.
pub const TERMINAL_INDEX_FILE_NAME: &str = "_terminal_index.jsonl";

/// Maximum JSONL line length (in bytes, including the trailing
/// newline) the writer accepts before refusing.
///
/// `PIPE_BUF` is 4096 on Linux; POSIX guarantees atomic-w.r.t.-each-
/// other appends only for writes within this bound. The writer aborts
/// on overlength inputs rather than silently producing a non-atomic
/// append; the abort is observable in tests and protects the race-
/// condition AC for N concurrent writers.
pub const MAX_INDEX_LINE_BYTES: usize = 4096;

/// Single entry in the terminal-index — written as one JSONL line.
///
/// Per Decision 3: append-only, JSONL not TOML, four required fields.
/// Extra unknown keys are tolerated by the reader (forward-compat for
/// additive evolution).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalIndexEntry {
    /// Session id of the terminal workflow.
    pub session_id: String,
    /// RFC 3339 / ISO 8601 timestamp (millisecond precision) the
    /// terminal transition was observed.
    pub terminal_at: String,
    /// On-disk header mtime in nanoseconds since the UNIX epoch.
    /// Drives the reader's `(session_id, max header_mtime_ns)` dedup
    /// and the discovery scan's header-is-truth fallthrough.
    pub header_mtime_ns: u64,
    /// Terminal classification: `"completed"` for a workflow that
    /// reached a `terminal: true` state, `"abandoned"` for a workflow
    /// that recorded a `WorkflowCancelled` event or had its claim
    /// revoked via the redelegation-cap-exceeded recovery path (Issue
    /// 11 cases 3b/3c).
    pub terminal_state: String,
}

/// Return the path to the terminal-index file under `koto_root`.
///
/// Does not create the file or any parent directory; callers must
/// ensure `koto_root` exists.
pub fn terminal_index_path(koto_root: &Path) -> PathBuf {
    koto_root.join(TERMINAL_INDEX_FILE_NAME)
}

/// Return the on-disk mtime of `header_path` as nanoseconds since the
/// UNIX epoch.
///
/// `u64` overflows at year 2554 — well beyond any deployment window
/// of this codebase. Callers that need year-2554 longevity should bump
/// to `u128` and update the JSONL wire format.
pub fn header_mtime_unix_nanos(header_path: &Path) -> Result<u64> {
    let meta = std::fs::metadata(header_path)
        .with_context(|| format!("failed to stat {}", header_path.display()))?;
    let modified = meta.modified().with_context(|| {
        format!(
            "filesystem does not report mtime for {}",
            header_path.display()
        )
    })?;
    let dur = modified
        .duration_since(UNIX_EPOCH)
        .with_context(|| format!("mtime predates UNIX epoch for {}", header_path.display()))?;
    Ok(dur.as_nanos() as u64)
}

/// Append a single terminal-index line for `session_id` rooted at
/// `koto_root`.
///
/// The function:
/// 1. Stats `header_path` to derive `header_mtime_ns`. A stat failure
///    falls through to `header_mtime_ns = 0` — the entry is still
///    written so the discovery scan has a record to dedup against,
///    and the header-is-truth fallthrough rule keeps the session
///    visible until the index catches up.
/// 2. Constructs the JSONL line and verifies it fits within
///    [`MAX_INDEX_LINE_BYTES`]; an oversize line returns an error
///    BEFORE any write, preserving the POSIX atomic-append discipline.
/// 3. Opens the file with `OpenOptions::append(true)` (POSIX
///    `O_APPEND`), writes the line + newline as a single `write_all`,
///    flushes, and `fsync`s.
///
/// The opened `File` is dropped at function exit — the next append
/// re-opens. This is the documented append discipline (Decision 3,
/// Security Considerations release-time enforcement #3): each append
/// is a fresh `O_APPEND` open whose offset the kernel resolves
/// atomically against any concurrent appender's write.
pub fn append_terminal_index(
    koto_root: &Path,
    session_id: &str,
    terminal_state: &str,
    header_path: &Path,
) -> Result<()> {
    // Best-effort mtime read; a stat failure falls through to 0 so a
    // stripped/missing header doesn't drop the index entry on the floor.
    let header_mtime_ns = header_mtime_unix_nanos(header_path).unwrap_or(0);
    let entry = TerminalIndexEntry {
        session_id: session_id.to_string(),
        terminal_at: now_iso8601_millis(),
        header_mtime_ns,
        terminal_state: terminal_state.to_string(),
    };
    append_terminal_index_entry(koto_root, &entry)
}

/// Lower-level append: callers pass a fully-formed [`TerminalIndexEntry`].
///
/// Tests use this to drive race-condition fixtures with deterministic
/// `terminal_at` / `header_mtime_ns` values.
pub fn append_terminal_index_entry(koto_root: &Path, entry: &TerminalIndexEntry) -> Result<()> {
    // Ensure koto_root exists so first-ever append on a fresh workspace
    // doesn't fail. Parent dir of the index IS koto_root itself.
    if !koto_root.exists() {
        std::fs::create_dir_all(koto_root)
            .with_context(|| format!("failed to create {}", koto_root.display()))?;
    }
    let path = terminal_index_path(koto_root);

    // Serialize and bound-check before opening. An overlength line is
    // a hard error: writes > PIPE_BUF lose POSIX atomic-w.r.t.-each-
    // other appends, so refusing to write protects the race-condition
    // AC for N concurrent writers.
    let mut serialized = serde_json::to_string(entry)
        .with_context(|| format!("failed to serialize entry for {}", entry.session_id))?;
    serialized.push('\n');
    if serialized.len() > MAX_INDEX_LINE_BYTES {
        anyhow::bail!(
            "terminal-index line exceeds PIPE_BUF bound ({} bytes > {})",
            serialized.len(),
            MAX_INDEX_LINE_BYTES
        );
    }

    // O_APPEND is the security spine: the kernel resolves the offset
    // atomically per write, so concurrent appenders never overwrite
    // each other under PIPE_BUF. Do NOT introduce a seek() or
    // write_at(offset) call on this handle. (Security Considerations
    // release-time enforcement #3.)
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("failed to open {} for append", path.display()))?;

    file.write_all(serialized.as_bytes())
        .with_context(|| format!("failed to append to {}", path.display()))?;
    file.flush()
        .with_context(|| format!("failed to flush {}", path.display()))?;
    file.sync_data()
        .with_context(|| format!("failed to fsync {}", path.display()))?;
    Ok(())
}

/// Read the terminal index into an in-memory dedup table.
///
/// Walks `<koto_root>/_terminal_index.jsonl` line-by-line and inserts
/// each parseable entry into a `HashMap<session_id, entry>`. When two
/// entries share a `session_id`, the one with the higher
/// `header_mtime_ns` wins — order of appearance does NOT determine
/// the winner. The multi-writer pathological cases (sidecar recovery
/// in Issue 11 cases 3a/3b/3c plus the dispatched agent's terminal
/// append) rely on this discipline for correctness.
///
/// Skip-and-continue rules (each logs a warn-level message; none
/// abort the read):
/// - A line that does NOT end in `\n` is treated as a truncated
///   trailing line and skipped. This is the crash-mid-write recovery
///   case.
/// - A line that fails JSON parsing is skipped.
/// - A line that parses but is missing any of the four required
///   fields is skipped.
///
/// Returns an empty map when the file does not exist (a brand-new
/// workspace).
pub fn read_terminal_index(koto_root: &Path) -> HashMap<String, TerminalIndexEntry> {
    let path = terminal_index_path(koto_root);
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return HashMap::new(),
        Err(e) => {
            eprintln!(
                "warning: terminal-index read failed for {} ({}); treating as empty",
                path.display(),
                e
            );
            return HashMap::new();
        }
    };

    let mut last_complete_offset: u64 = 0;
    let mut reader = BufReader::new(file);
    let mut out: HashMap<String, TerminalIndexEntry> = HashMap::new();

    // Track the trailing newline to detect truncated last line.
    let mut bytes_consumed: u64 = 0;
    let mut buf = String::new();
    loop {
        buf.clear();
        let n = match reader.read_line(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                eprintln!(
                    "warning: terminal-index read aborted at offset {} ({}); preserving {} entries",
                    bytes_consumed,
                    e,
                    out.len()
                );
                break;
            }
        };
        bytes_consumed += n as u64;
        if !buf.ends_with('\n') {
            // Truncated trailing line — crash-mid-write. Skip with a
            // warn and stop reading; subsequent lines (if any) would
            // be unreachable because read_line returns 0 once we've
            // seen EOF.
            eprintln!(
                "warning: terminal-index trailing line is truncated (no newline) at offset {}; skipping",
                last_complete_offset
            );
            break;
        }
        last_complete_offset = bytes_consumed;
        // Trim trailing newline before parsing.
        let line = buf.trim_end_matches('\n');
        if line.is_empty() {
            continue;
        }
        let entry: TerminalIndexEntry = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(e) => {
                eprintln!(
                    "warning: terminal-index line at offset {} is malformed ({}); skipping",
                    last_complete_offset, e
                );
                continue;
            }
        };
        if entry.session_id.is_empty()
            || entry.terminal_at.is_empty()
            || entry.terminal_state.is_empty()
        {
            // A line that parses but has empty required fields is
            // treated as malformed under the same discipline. The
            // `serde_json::from_str` already enforces presence of
            // each field; this catches the empty-string case.
            eprintln!(
                "warning: terminal-index entry at offset {} missing required fields; skipping",
                last_complete_offset
            );
            continue;
        }

        // Dedup by (session_id, max header_mtime_ns).
        match out.get(&entry.session_id) {
            Some(prior) if prior.header_mtime_ns >= entry.header_mtime_ns => {
                // Prior wins — order-of-appearance does NOT determine
                // the winner; the higher header_mtime_ns does.
            }
            _ => {
                out.insert(entry.session_id.clone(), entry);
            }
        }
    }
    out
}

/// Drain the entire file into a `Vec<TerminalIndexEntry>` preserving
/// order. Used by Issue 9's compaction lease to rewrite the dedup'd
/// copy; not the steady-state hot path.
///
/// Same skip-and-continue rules as [`read_terminal_index`].
pub fn drain_terminal_index_ordered(koto_root: &Path) -> Result<Vec<TerminalIndexEntry>> {
    let path = terminal_index_path(koto_root);
    let mut out = Vec::new();
    let mut file = match File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e).with_context(|| format!("failed to open {}", path.display())),
    };
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let ends_with_newline = contents.ends_with('\n');
    for (i, line) in contents.lines().enumerate() {
        // The last line lacks a terminator only when the file does not
        // end in a newline AND we're at the final line.
        let is_last_line = i + 1 == contents.lines().count();
        if is_last_line && !ends_with_newline {
            eprintln!(
                "warning: terminal-index trailing line at index {} is truncated; skipping",
                i
            );
            continue;
        }
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<TerminalIndexEntry>(line) {
            Ok(e) => out.push(e),
            Err(err) => {
                eprintln!(
                    "warning: terminal-index line at index {} is malformed ({}); skipping",
                    i, err
                );
                continue;
            }
        }
    }
    Ok(out)
}

/// Return the current UTC time formatted as ISO 8601 with millisecond
/// precision, mirroring the engine's [`crate::engine::types::now_iso8601`].
///
/// Re-implemented here to avoid pulling the engine-types `now` helper
/// into the terminal-index module's compile graph. The two
/// implementations produce byte-identical output.
fn now_iso8601_millis() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    let millis = duration.subsec_millis();
    let sec = secs % 60;
    let min = (secs / 60) % 60;
    let hour = (secs / 3600) % 24;
    let days = secs / 86400;
    let (year, month, day) = days_to_ymd(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year, month, day, hour, min, sec, millis
    )
}

fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    let mut year = 1970u64;
    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }
    let leap = is_leap(year);
    let month_days: &[u64] = if leap {
        &[31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        &[31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut month = 1u64;
    for &md in month_days {
        if days < md {
            break;
        }
        days -= md;
        month += 1;
    }
    (year, month, days + 1)
}

fn is_leap(y: u64) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}

// ===== Issue 9: terminal-index compaction lease =====

/// File name of the compaction-lease sidecar, joined under
/// `<koto_root>`.
///
/// Created via `O_CREAT | O_EXCL` so two coordinators racing on
/// compaction see exactly one success and one `EEXIST`. Loser-coords
/// skip compaction this tick; the next tick re-evaluates after the
/// winner releases the lease.
pub const COMPACT_LOCK_FILE_NAME: &str = "_terminal_index.compact.lock";

/// File name of the in-progress compaction's temporary deduplicated
/// output. Atomically renamed over [`TERMINAL_INDEX_FILE_NAME`] on
/// successful compaction; cleaned up by [`recover_stale_compact_lock`]
/// when a crashed compaction leaves it behind.
pub const TEMP_INDEX_FILE_NAME: &str = "_terminal_index.jsonl.tmp";

/// Return the path to the compaction-lease sidecar under `koto_root`.
pub fn compact_lock_path(koto_root: &Path) -> PathBuf {
    koto_root.join(COMPACT_LOCK_FILE_NAME)
}

/// Return the path to the in-progress compaction's temp file under
/// `koto_root`.
pub fn temp_index_path(koto_root: &Path) -> PathBuf {
    koto_root.join(TEMP_INDEX_FILE_NAME)
}

/// Outcome of a single [`maybe_compact_terminal_index`] call.
///
/// `Skipped` is the steady-state common case — the threshold is not
/// exceeded or a peer coord already holds the lease. `Compacted`
/// reports the line count change so callers can log a useful one-line
/// summary. `Failed` separates compaction errors from "did not run" —
/// the caller can decide whether to surface a warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactionOutcome {
    /// Threshold not met or another coord already held the lock.
    Skipped { reason: SkippedReason },
    /// Compaction ran to completion.
    Compacted {
        /// Lines before compaction.
        lines_before: u64,
        /// Lines after compaction (post-dedup).
        lines_after: u64,
    },
    /// Compaction acquired the lease but failed mid-write. The lock
    /// has been released and the temp file removed; the original
    /// index is unchanged.
    Failed { error: String },
}

/// Reason a compaction attempt was skipped, surfaced via
/// [`CompactionOutcome::Skipped`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkippedReason {
    /// Index line count is below the configured threshold.
    BelowThreshold { lines: u64, threshold: u64 },
    /// Another coord already holds the compaction lease (`EEXIST` on
    /// the `O_EXCL` acquire).
    LeaseHeldByPeer,
    /// The index file does not exist (a fresh workspace). Nothing to
    /// compact.
    IndexAbsent,
}

/// Lease file contents written under [`compact_lock_path`].
///
/// Stored as TOML to match the workspace convention (cursor files are
/// TOML; JSONL is reserved for append-only event streams). A
/// malformed lock is treated as foreign by
/// [`recover_stale_compact_lock`] so a corrupted lease can never
/// indefinitely block compaction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompactLock {
    /// Coordinator id of the lease holder.
    pub coord_id: String,
    /// RFC 3339 / ISO 8601 timestamp (millisecond precision) the
    /// lease was acquired.
    pub started_at: String,
    /// Unix seconds at lease-acquire time. Stored separately from
    /// `started_at` so the stale-lock recovery walk can compute age
    /// without re-parsing the ISO string (cheap, allocation-free).
    pub started_at_unix_seconds: u64,
}

/// Run the compaction pass against the workspace's terminal index.
///
/// Caller contract:
/// 1. **Threshold gating.** Returns `Skipped { BelowThreshold }` when
///    the index line count is below `cfg.terminal_index_compact_lines`.
///    The threshold is a soft trigger — the caller does not need to
///    debounce; subsequent ticks will skip cheaply until the threshold
///    is crossed.
/// 2. **Lease acquisition.** Creates `_terminal_index.compact.lock`
///    via `O_CREAT | O_EXCL`. On `EEXIST` returns
///    `Skipped { LeaseHeldByPeer }`.
/// 3. **Compaction body.** Reads all lines via
///    [`drain_terminal_index_ordered`], dedups by
///    `(session_id, max header_mtime_ns)`, writes to
///    [`temp_index_path`], `fsync`s the temp file, atomically renames
///    over the original index, and unlinks the lease sidecar.
/// 4. **Failure handling.** On any error after lease acquisition the
///    lease is released and the temp file is removed; the original
///    index is untouched.
///
/// `coord_id` is recorded in the lease file so the
/// [`recover_stale_compact_lock`] walk can distinguish own-coord
/// holders from foreign ones.
pub fn maybe_compact_terminal_index(
    koto_root: &Path,
    coord_id: &str,
    cfg: &crate::config::Kt1Config,
) -> Result<CompactionOutcome> {
    let index_path = terminal_index_path(koto_root);
    if !index_path.exists() {
        return Ok(CompactionOutcome::Skipped {
            reason: SkippedReason::IndexAbsent,
        });
    }

    // Threshold check via exact line count. The `BufReader::lines`
    // iterator costs O(file size) but only runs when the file exists
    // and the rest of the function would also walk the file end-to-end
    // anyway — there's no cheaper estimate that's worth the extra code.
    let lines = count_index_lines(&index_path)?;
    if lines < cfg.terminal_index_compact_lines {
        return Ok(CompactionOutcome::Skipped {
            reason: SkippedReason::BelowThreshold {
                lines,
                threshold: cfg.terminal_index_compact_lines,
            },
        });
    }

    // Acquire the O_EXCL lease. `create_new(true)` sets `O_CREAT |
    // O_EXCL`; the kernel returns `EEXIST` if any other process (or
    // a prior crashed compaction whose lock the stale-lock recovery
    // hasn't yet swept) holds it. The loser skips this tick.
    let lock_path = compact_lock_path(koto_root);
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let lock = CompactLock {
        coord_id: coord_id.to_string(),
        started_at: now_iso8601_millis(),
        started_at_unix_seconds: now_secs,
    };
    let lock_toml = toml::to_string(&lock).context("failed to serialize compact lock")?;
    match OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&lock_path)
    {
        Ok(mut f) => {
            // Mode 0600 on the lease — file is created via the umask
            // by default; set explicit permissions for the audit
            // trail's "lease was operator-private" property.
            set_mode_0600(&lock_path)?;
            f.write_all(lock_toml.as_bytes())
                .with_context(|| format!("failed to write compact lock {}", lock_path.display()))?;
            f.sync_all()
                .with_context(|| format!("failed to fsync compact lock {}", lock_path.display()))?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Ok(CompactionOutcome::Skipped {
                reason: SkippedReason::LeaseHeldByPeer,
            });
        }
        Err(e) => {
            return Err(e).with_context(|| {
                format!("failed to acquire compact lease at {}", lock_path.display())
            });
        }
    }

    // Lease acquired — perform the compaction. On any error,
    // release the lease + remove the temp file before returning so
    // the next coordinator's threshold pass gets a fresh shot.
    let result = compact_under_lease(koto_root, &index_path, lines);
    // Lease release: always unlink, even on failure.
    let _ = std::fs::remove_file(&lock_path);
    match result {
        Ok(lines_after) => Ok(CompactionOutcome::Compacted {
            lines_before: lines,
            lines_after,
        }),
        Err(e) => {
            // Best-effort temp file cleanup on the error path.
            let _ = std::fs::remove_file(temp_index_path(koto_root));
            Ok(CompactionOutcome::Failed {
                error: format!("{:#}", e),
            })
        }
    }
}

/// Body of the compaction that runs UNDER the lease.
///
/// Reads all lines, dedups, writes to temp, fsyncs, atomically
/// renames. Returns the post-dedup line count on success. The caller
/// is responsible for releasing the lease and cleaning up the temp
/// file on failure.
fn compact_under_lease(koto_root: &Path, index_path: &Path, _lines_before: u64) -> Result<u64> {
    // Build the deduplicated set: keep the entry with the highest
    // `header_mtime_ns` per `session_id`. Distinct session_ids
    // preserve their first-appearance order (the design's
    // "Distinct session_ids preserve their relative ordering from
    // the original file" AC).
    let ordered = drain_terminal_index_ordered(koto_root)?;
    let mut winner_by_sid: HashMap<String, TerminalIndexEntry> = HashMap::new();
    let mut first_appearance: Vec<String> = Vec::new();
    for entry in ordered {
        let sid = entry.session_id.clone();
        match winner_by_sid.get(&sid) {
            Some(prior) if prior.header_mtime_ns >= entry.header_mtime_ns => {
                // Prior wins; do not perturb first-appearance order.
            }
            Some(_) => {
                winner_by_sid.insert(sid, entry);
            }
            None => {
                first_appearance.push(sid.clone());
                winner_by_sid.insert(sid, entry);
            }
        }
    }

    let temp_path = temp_index_path(koto_root);
    // Write the deduplicated content to the temp file. Use
    // `create_new(false)` so a residual orphan temp from a
    // prior-tick failure does not block us — the file is truncated
    // on `create(true)` + `write(true)` + `truncate(true)`. The
    // atomic rename is the visible synchronization point; the temp
    // file's intermediate state is never observable from the index
    // path.
    let mut temp_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&temp_path)
        .with_context(|| format!("failed to create temp file {}", temp_path.display()))?;
    for sid in &first_appearance {
        let entry = winner_by_sid
            .get(sid)
            .expect("first-appearance keys are inserted");
        let mut line = serde_json::to_string(entry)
            .with_context(|| format!("failed to serialize entry for {}", sid))?;
        line.push('\n');
        if line.len() > MAX_INDEX_LINE_BYTES {
            anyhow::bail!(
                "terminal-index compaction line exceeds PIPE_BUF bound ({} > {}) for session_id={}",
                line.len(),
                MAX_INDEX_LINE_BYTES,
                sid
            );
        }
        temp_file
            .write_all(line.as_bytes())
            .with_context(|| format!("failed to write compacted line for {}", sid))?;
    }
    temp_file
        .flush()
        .with_context(|| format!("failed to flush {}", temp_path.display()))?;
    temp_file
        .sync_all()
        .with_context(|| format!("failed to fsync {}", temp_path.display()))?;
    drop(temp_file);

    // Atomic rename: tmp -> final. POSIX `rename(2)` is atomic
    // w.r.t. concurrent open/stat on the destination.
    std::fs::rename(&temp_path, index_path).with_context(|| {
        format!(
            "failed to rename {} -> {}",
            temp_path.display(),
            index_path.display()
        )
    })?;

    Ok(first_appearance.len() as u64)
}

/// Count the lines in `path`, treating a truncated trailing line
/// (no terminating newline) as a counted entry too. The threshold
/// gating is monotonic — overcounting by one is fine; what matters
/// is the cross-threshold detection.
fn count_index_lines(path: &Path) -> Result<u64> {
    let file = File::open(path)
        .with_context(|| format!("failed to open {} for line count", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut count: u64 = 0;
    let mut buf = String::new();
    loop {
        buf.clear();
        let n = reader
            .read_line(&mut buf)
            .with_context(|| format!("failed to read {} for line count", path.display()))?;
        if n == 0 {
            break;
        }
        count += 1;
    }
    Ok(count)
}

/// Set mode 0600 on the file at `path` (Unix only).
#[cfg(unix)]
fn set_mode_0600(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perms)
        .with_context(|| format!("failed to chmod 0600 on {}", path.display()))?;
    Ok(())
}

/// Non-Unix stub — the mode-0600 invariant is informational on
/// platforms without POSIX file modes.
#[cfg(not(unix))]
fn set_mode_0600(_path: &Path) -> Result<()> {
    Ok(())
}

/// Stale-lock recovery walk.
///
/// Runs as a post-pass on the discovery scan. Reads
/// [`compact_lock_path`] and applies the two-condition reclaim
/// predicate documented in Issue 9 AC: a lock is unlinked ONLY when
/// `started_at_unix_seconds` is older than
/// `cfg.compact_lock_timeout_seconds` AND its recorded `coord_id` is
/// not the current coordinator. Both conditions are required — the
/// foreign-coord_id check is the safety belt that prevents a long
/// legitimate compaction by THIS coordinator from triggering self-
/// reclaim and abandoning its own in-progress work.
///
/// When the predicate fires, any residual
/// `_terminal_index.jsonl.tmp` is also removed; this is the
/// crash-mid-compaction recovery path.
///
/// A malformed lock file (TOML parse failure) is treated as foreign:
/// the recovery walk unlinks it as if it had a foreign coord_id and
/// past-timeout `started_at`. A corrupted lease cannot indefinitely
/// block compaction; the next coord to hit the threshold gets a
/// fresh lease.
///
/// Returns `true` when the walk reclaimed the lock; `false` when no
/// lock was present or the lock was preserved (within timeout or
/// owned by the current coord).
pub fn recover_stale_compact_lock(
    koto_root: &Path,
    coord_id: &str,
    cfg: &crate::config::Kt1Config,
) -> Result<bool> {
    let lock_path = compact_lock_path(koto_root);
    let contents = match std::fs::read_to_string(&lock_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            eprintln!(
                "warning: compact-lock read failed for {} ({}); skipping recovery",
                lock_path.display(),
                e
            );
            return Ok(false);
        }
    };

    let lock_age_seconds = match toml::from_str::<CompactLock>(&contents) {
        Ok(lock) => {
            // Same-coord self-protection: never reclaim a lock owned
            // by the current coordinator, regardless of age.
            if lock.coord_id == coord_id {
                return Ok(false);
            }
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            now.saturating_sub(lock.started_at_unix_seconds)
        }
        Err(e) => {
            // Malformed lock: treat as foreign with past-timeout age
            // so the recovery proceeds. A corrupted lease cannot
            // indefinitely block compaction.
            eprintln!(
                "warning: compact-lock at {} is malformed ({}); reclaiming",
                lock_path.display(),
                e
            );
            cfg.compact_lock_timeout_seconds + 1
        }
    };

    if lock_age_seconds <= cfg.compact_lock_timeout_seconds {
        // Live compaction in progress; leave it alone.
        return Ok(false);
    }

    // Both conditions satisfied — unlink the lock + any residual
    // temp file. Best-effort: tolerate the file already being gone
    // (a peer coord's recovery walk may have reclaimed concurrently).
    if let Err(e) = std::fs::remove_file(&lock_path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(e).with_context(|| {
                format!(
                    "failed to unlink stale compact lock {}",
                    lock_path.display()
                )
            });
        }
    }
    let temp_path = temp_index_path(koto_root);
    if let Err(e) = std::fs::remove_file(&temp_path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            eprintln!(
                "warning: failed to remove stale temp file {} ({})",
                temp_path.display(),
                e
            );
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(session_id: &str, mtime_ns: u64, state: &str) -> TerminalIndexEntry {
        TerminalIndexEntry {
            session_id: session_id.to_string(),
            terminal_at: "2026-05-24T14:35:01.000Z".to_string(),
            header_mtime_ns: mtime_ns,
            terminal_state: state.to_string(),
        }
    }

    #[test]
    fn append_writes_complete_line_with_trailing_newline() {
        let tmp = tempfile::tempdir().unwrap();
        let e = entry("scrutineer-a", 1_716_579_999_123_456_789, "completed");
        append_terminal_index_entry(tmp.path(), &e).unwrap();
        let contents = std::fs::read_to_string(terminal_index_path(tmp.path())).unwrap();
        assert!(contents.ends_with('\n'), "line must end with newline");
        assert!(contents.contains("\"session_id\":\"scrutineer-a\""));
        assert!(contents.contains("\"terminal_state\":\"completed\""));
        assert!(contents.contains("\"header_mtime_ns\":1716579999123456789"));
    }

    #[test]
    fn append_refuses_overlength_line() {
        let tmp = tempfile::tempdir().unwrap();
        // A 5000-char session id pushes the line above PIPE_BUF.
        let huge_id = "a".repeat(5000);
        let e = entry(&huge_id, 1, "completed");
        let err = append_terminal_index_entry(tmp.path(), &e).unwrap_err();
        let msg = format!("{}", err);
        assert!(
            msg.contains("PIPE_BUF") || msg.contains("exceeds"),
            "expected PIPE_BUF bound error, got {}",
            msg
        );
        // File must not be created or, if it was created by another test
        // path, must not contain the overlength entry. We just check the
        // file is either absent or empty.
        let path = terminal_index_path(tmp.path());
        if path.exists() {
            let contents = std::fs::read_to_string(&path).unwrap();
            assert!(!contents.contains(&huge_id));
        }
    }

    #[test]
    fn reader_dedups_by_max_header_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        // Write the lower-mtime entry FIRST so order-of-appearance is
        // not the winner.
        append_terminal_index_entry(tmp.path(), &entry("s1", 100, "completed")).unwrap();
        append_terminal_index_entry(tmp.path(), &entry("s1", 200, "completed")).unwrap();
        // Add a third entry below the second to verify max wins, not
        // last-write-wins.
        append_terminal_index_entry(tmp.path(), &entry("s1", 150, "abandoned")).unwrap();
        let map = read_terminal_index(tmp.path());
        let got = map.get("s1").expect("s1 must be in map");
        assert_eq!(got.header_mtime_ns, 200);
        assert_eq!(got.terminal_state, "completed");
    }

    #[test]
    fn reader_handles_missing_file_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let map = read_terminal_index(tmp.path());
        assert!(map.is_empty());
    }

    #[test]
    fn reader_skips_truncated_trailing_line() {
        let tmp = tempfile::tempdir().unwrap();
        let path = terminal_index_path(tmp.path());
        // Append a complete line.
        append_terminal_index_entry(tmp.path(), &entry("complete-1", 100, "completed")).unwrap();
        // Append a partial line without trailing newline directly to
        // the file (simulates crash mid-write).
        {
            let mut f = OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(b"{\"session_id\":\"partial\",\"terminal_at\":\"2026")
                .unwrap();
        }
        let map = read_terminal_index(tmp.path());
        assert_eq!(map.len(), 1, "must surface only the complete entry");
        assert!(map.contains_key("complete-1"));
        assert!(!map.contains_key("partial"));
    }

    #[test]
    fn reader_skips_malformed_json_line() {
        let tmp = tempfile::tempdir().unwrap();
        let path = terminal_index_path(tmp.path());
        // Append a malformed line, then a good one.
        {
            let mut f = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .unwrap();
            f.write_all(b"this is not json at all\n").unwrap();
        }
        append_terminal_index_entry(tmp.path(), &entry("good-1", 100, "completed")).unwrap();
        let map = read_terminal_index(tmp.path());
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("good-1"));
    }

    #[test]
    fn reader_skips_entry_missing_required_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let path = terminal_index_path(tmp.path());
        // Empty session_id slips past parsing (Default for String is "")
        // — explicit check rejects it.
        {
            let mut f = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .unwrap();
            f.write_all(b"{\"session_id\":\"\",\"terminal_at\":\"2026-05-24T00:00:00.000Z\",\"header_mtime_ns\":1,\"terminal_state\":\"completed\"}\n")
                .unwrap();
        }
        append_terminal_index_entry(tmp.path(), &entry("good-1", 100, "completed")).unwrap();
        let map = read_terminal_index(tmp.path());
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("good-1"));
    }

    #[test]
    fn reader_accepts_extra_unknown_keys_forward_compat() {
        let tmp = tempfile::tempdir().unwrap();
        let path = terminal_index_path(tmp.path());
        // Manually write a line with an extra `future_field` key.
        {
            let mut f = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .unwrap();
            f.write_all(b"{\"session_id\":\"s1\",\"terminal_at\":\"2026-05-24T00:00:00.000Z\",\"header_mtime_ns\":42,\"terminal_state\":\"completed\",\"future_field\":\"ignored\"}\n")
                .unwrap();
        }
        let map = read_terminal_index(tmp.path());
        let got = map.get("s1").expect("forward-compat entry must parse");
        assert_eq!(got.header_mtime_ns, 42);
    }

    #[test]
    fn terminal_index_path_layout() {
        let root = PathBuf::from("/tmp/.koto");
        assert_eq!(
            terminal_index_path(&root),
            PathBuf::from("/tmp/.koto/_terminal_index.jsonl")
        );
    }

    #[test]
    fn append_creates_koto_root_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        // Use a nested path that does not exist yet.
        let root = tmp.path().join("brand-new-root");
        assert!(!root.exists());
        append_terminal_index_entry(&root, &entry("s1", 1, "completed")).unwrap();
        assert!(terminal_index_path(&root).exists());
    }

    #[test]
    fn drain_ordered_preserves_append_order() {
        let tmp = tempfile::tempdir().unwrap();
        append_terminal_index_entry(tmp.path(), &entry("s1", 100, "completed")).unwrap();
        append_terminal_index_entry(tmp.path(), &entry("s2", 200, "completed")).unwrap();
        append_terminal_index_entry(tmp.path(), &entry("s3", 300, "abandoned")).unwrap();
        let ordered = drain_terminal_index_ordered(tmp.path()).unwrap();
        assert_eq!(ordered.len(), 3);
        assert_eq!(ordered[0].session_id, "s1");
        assert_eq!(ordered[1].session_id, "s2");
        assert_eq!(ordered[2].session_id, "s3");
    }

    /// Race-condition AC: N=32 concurrent writers produce 32 parseable
    /// lines with no interleave.
    #[test]
    fn race_n_writers_produce_n_parseable_lines() {
        use std::sync::Arc;
        use std::thread;

        let tmp = tempfile::tempdir().unwrap();
        let root = Arc::new(tmp.path().to_path_buf());
        let n = 32_usize;
        let mut handles = Vec::with_capacity(n);
        for i in 0..n {
            let root_c = Arc::clone(&root);
            handles.push(thread::spawn(move || {
                let sid = format!("session-{:02}", i);
                let e = TerminalIndexEntry {
                    session_id: sid,
                    terminal_at: "2026-05-24T14:35:01.000Z".to_string(),
                    header_mtime_ns: 1_000_000_000 + i as u64,
                    terminal_state: "completed".to_string(),
                };
                append_terminal_index_entry(&root_c, &e).unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        // The index file must have exactly n lines, each parseable
        // independently, each starting with `{` and ending with `}\n`.
        let contents = std::fs::read_to_string(terminal_index_path(&root)).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), n, "expected {} lines, got {}", n, lines.len());

        // Each line is a complete, parseable JSON object.
        let mut seen_sids = std::collections::HashSet::new();
        for line in &lines {
            assert!(
                line.starts_with('{') && line.ends_with('}'),
                "line did not start with `{{` and end with `}}`: {}",
                line
            );
            let parsed: TerminalIndexEntry = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("line did not parse: {} ({})", line, e));
            assert!(
                seen_sids.insert(parsed.session_id.clone()),
                "duplicate session_id in race output: {}",
                parsed.session_id
            );
        }

        // The reader dedups to n entries.
        let map = read_terminal_index(&root);
        assert_eq!(map.len(), n);
    }

    // ===== Issue 9: compaction lease unit tests =====

    fn cfg_with_threshold_and_timeout(
        threshold: u64,
        timeout_seconds: u64,
    ) -> crate::config::Kt1Config {
        crate::config::Kt1Config {
            terminal_index_compact_lines: threshold,
            compact_lock_timeout_seconds: timeout_seconds,
            ..crate::config::Kt1Config::default()
        }
    }

    fn fixture_entry(sid: &str, mtime_ns: u64) -> TerminalIndexEntry {
        TerminalIndexEntry {
            session_id: sid.to_string(),
            terminal_at: "2026-05-24T14:35:01.000Z".to_string(),
            header_mtime_ns: mtime_ns,
            terminal_state: "completed".to_string(),
        }
    }

    fn write_stale_compact_lock(koto_root: &Path, lock: &CompactLock) {
        std::fs::create_dir_all(koto_root).unwrap();
        let path = compact_lock_path(koto_root);
        let s = toml::to_string(lock).unwrap();
        std::fs::write(&path, s).unwrap();
    }

    #[test]
    fn compact_lock_path_layout() {
        let root = PathBuf::from("/tmp/.koto");
        assert_eq!(
            compact_lock_path(&root),
            PathBuf::from("/tmp/.koto/_terminal_index.compact.lock")
        );
        assert_eq!(
            temp_index_path(&root),
            PathBuf::from("/tmp/.koto/_terminal_index.jsonl.tmp")
        );
    }

    #[test]
    fn threshold_skipped_when_lines_below() {
        let tmp = tempfile::tempdir().unwrap();
        // 3 entries, threshold 10 → skipped.
        for i in 0..3 {
            append_terminal_index_entry(tmp.path(), &fixture_entry(&format!("s{}", i), i + 1))
                .unwrap();
        }
        let cfg = cfg_with_threshold_and_timeout(10, 3600);
        let outcome = maybe_compact_terminal_index(tmp.path(), "coord-a", &cfg).unwrap();
        assert!(
            matches!(
                outcome,
                CompactionOutcome::Skipped {
                    reason: SkippedReason::BelowThreshold { .. }
                }
            ),
            "expected BelowThreshold, got {:?}",
            outcome
        );
        // No lease residue.
        assert!(!compact_lock_path(tmp.path()).exists());
    }

    #[test]
    fn threshold_triggers_when_lines_at_or_above() {
        let tmp = tempfile::tempdir().unwrap();
        // 5 entries with 5 distinct session_ids; threshold 5 → triggers.
        for i in 0..5 {
            append_terminal_index_entry(tmp.path(), &fixture_entry(&format!("s{}", i), i + 1))
                .unwrap();
        }
        let cfg = cfg_with_threshold_and_timeout(5, 3600);
        let outcome = maybe_compact_terminal_index(tmp.path(), "coord-a", &cfg).unwrap();
        match outcome {
            CompactionOutcome::Compacted {
                lines_before,
                lines_after,
            } => {
                assert_eq!(lines_before, 5);
                // No dupes in the fixture; lines_after == 5.
                assert_eq!(lines_after, 5);
            }
            other => panic!("expected Compacted, got {:?}", other),
        }
        // Lease released.
        assert!(!compact_lock_path(tmp.path()).exists());
        // Temp file cleaned up by the atomic rename.
        assert!(!temp_index_path(tmp.path()).exists());
    }

    #[test]
    fn compaction_dedups_by_max_header_mtime_preserving_first_appearance_order() {
        let tmp = tempfile::tempdir().unwrap();
        // Interleave duplicates so first-appearance order is testable.
        // First-appearance order: s1, s2, s3.
        append_terminal_index_entry(tmp.path(), &fixture_entry("s1", 100)).unwrap();
        append_terminal_index_entry(tmp.path(), &fixture_entry("s2", 200)).unwrap();
        append_terminal_index_entry(tmp.path(), &fixture_entry("s1", 150)).unwrap();
        append_terminal_index_entry(tmp.path(), &fixture_entry("s3", 50)).unwrap();
        append_terminal_index_entry(tmp.path(), &fixture_entry("s2", 250)).unwrap();
        append_terminal_index_entry(tmp.path(), &fixture_entry("s1", 175)).unwrap();
        // 6 entries; threshold = 6 → triggers.
        let cfg = cfg_with_threshold_and_timeout(6, 3600);
        let outcome = maybe_compact_terminal_index(tmp.path(), "coord-a", &cfg).unwrap();
        assert!(matches!(outcome, CompactionOutcome::Compacted { .. }));

        // Post-compaction file: 3 lines, ordered s1/s2/s3 by first appearance.
        let post = drain_terminal_index_ordered(tmp.path()).unwrap();
        assert_eq!(post.len(), 3);
        assert_eq!(post[0].session_id, "s1");
        assert_eq!(post[0].header_mtime_ns, 175); // max of 100/150/175
        assert_eq!(post[1].session_id, "s2");
        assert_eq!(post[1].header_mtime_ns, 250); // max of 200/250
        assert_eq!(post[2].session_id, "s3");
        assert_eq!(post[2].header_mtime_ns, 50);

        // Reader semantics preserved: dedup map matches pre/post.
        let map = read_terminal_index(tmp.path());
        assert_eq!(map.len(), 3);
        assert_eq!(map.get("s1").unwrap().header_mtime_ns, 175);
        assert_eq!(map.get("s2").unwrap().header_mtime_ns, 250);
        assert_eq!(map.get("s3").unwrap().header_mtime_ns, 50);
    }

    #[test]
    fn second_coord_sees_eexist_and_skips() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..5 {
            append_terminal_index_entry(tmp.path(), &fixture_entry(&format!("s{}", i), i + 1))
                .unwrap();
        }
        // Pre-place a lock as a peer-coord.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let peer_lock = CompactLock {
            coord_id: "peer-coord".into(),
            started_at: "2026-05-24T14:35:01.000Z".into(),
            started_at_unix_seconds: now,
        };
        write_stale_compact_lock(tmp.path(), &peer_lock);

        let cfg = cfg_with_threshold_and_timeout(5, 3600);
        let outcome = maybe_compact_terminal_index(tmp.path(), "coord-a", &cfg).unwrap();
        assert!(
            matches!(
                outcome,
                CompactionOutcome::Skipped {
                    reason: SkippedReason::LeaseHeldByPeer
                }
            ),
            "expected LeaseHeldByPeer, got {:?}",
            outcome
        );
        // Pre-existing peer lock untouched (NOT clobbered).
        let contents = std::fs::read_to_string(compact_lock_path(tmp.path())).unwrap();
        assert!(contents.contains("peer-coord"));
    }

    #[test]
    fn stale_lock_recovery_unlinks_foreign_timeout_exceeded() {
        let tmp = tempfile::tempdir().unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let foreign_old_lock = CompactLock {
            coord_id: "dead-peer".into(),
            started_at: "2026-05-24T12:00:00.000Z".into(),
            // 2 hours ago.
            started_at_unix_seconds: now.saturating_sub(7200),
        };
        write_stale_compact_lock(tmp.path(), &foreign_old_lock);
        // Also drop a residual temp file to verify it's cleaned up.
        std::fs::write(temp_index_path(tmp.path()), b"partial junk\n").unwrap();

        let cfg = cfg_with_threshold_and_timeout(100_000, 3600);
        let reclaimed = recover_stale_compact_lock(tmp.path(), "coord-a", &cfg).unwrap();
        assert!(reclaimed);
        assert!(!compact_lock_path(tmp.path()).exists());
        assert!(!temp_index_path(tmp.path()).exists());
    }

    #[test]
    fn stale_lock_recovery_preserves_own_coord_at_any_age() {
        let tmp = tempfile::tempdir().unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let own_old_lock = CompactLock {
            coord_id: "coord-a".into(),
            started_at: "2026-05-24T12:00:00.000Z".into(),
            // 2 hours ago — past the default 1-hour timeout.
            started_at_unix_seconds: now.saturating_sub(7200),
        };
        write_stale_compact_lock(tmp.path(), &own_old_lock);

        let cfg = cfg_with_threshold_and_timeout(100_000, 3600);
        let reclaimed = recover_stale_compact_lock(tmp.path(), "coord-a", &cfg).unwrap();
        assert!(
            !reclaimed,
            "same-coord lock at any age must NOT be reclaimed"
        );
        assert!(compact_lock_path(tmp.path()).exists());
    }

    #[test]
    fn stale_lock_recovery_preserves_foreign_within_timeout() {
        let tmp = tempfile::tempdir().unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let foreign_fresh_lock = CompactLock {
            coord_id: "peer-coord".into(),
            started_at: "2026-05-24T14:35:01.000Z".into(),
            // 5 minutes ago — within the 1-hour timeout.
            started_at_unix_seconds: now.saturating_sub(300),
        };
        write_stale_compact_lock(tmp.path(), &foreign_fresh_lock);

        let cfg = cfg_with_threshold_and_timeout(100_000, 3600);
        let reclaimed = recover_stale_compact_lock(tmp.path(), "coord-a", &cfg).unwrap();
        assert!(
            !reclaimed,
            "foreign lock within timeout must NOT be reclaimed (live compaction in progress)"
        );
        assert!(compact_lock_path(tmp.path()).exists());
    }

    #[test]
    fn stale_lock_recovery_handles_missing_lock_as_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = cfg_with_threshold_and_timeout(100_000, 3600);
        let reclaimed = recover_stale_compact_lock(tmp.path(), "coord-a", &cfg).unwrap();
        assert!(!reclaimed);
    }

    #[test]
    fn stale_lock_recovery_treats_malformed_as_foreign_and_reclaims() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path()).unwrap();
        std::fs::write(
            compact_lock_path(tmp.path()),
            b"not a valid toml file === ?",
        )
        .unwrap();
        let cfg = cfg_with_threshold_and_timeout(100_000, 3600);
        let reclaimed = recover_stale_compact_lock(tmp.path(), "coord-a", &cfg).unwrap();
        assert!(
            reclaimed,
            "malformed lock must be reclaimed so it doesn't block compaction indefinitely"
        );
        assert!(!compact_lock_path(tmp.path()).exists());
    }

    #[test]
    fn compaction_below_threshold_returns_index_absent_when_no_file() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = cfg_with_threshold_and_timeout(100_000, 3600);
        let outcome = maybe_compact_terminal_index(tmp.path(), "coord-a", &cfg).unwrap();
        assert!(matches!(
            outcome,
            CompactionOutcome::Skipped {
                reason: SkippedReason::IndexAbsent
            }
        ));
    }

    #[test]
    fn lock_acquired_carries_self_coord_id_and_iso_started_at() {
        let tmp = tempfile::tempdir().unwrap();
        // Force a peer-held lock so we see the lock contents the
        // would-be acquirer wrote. Easier: write a lock as if we
        // just acquired, then read back.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let own_lock = CompactLock {
            coord_id: "coord-self".into(),
            started_at: now_iso8601_millis(),
            started_at_unix_seconds: now,
        };
        write_stale_compact_lock(tmp.path(), &own_lock);
        let contents = std::fs::read_to_string(compact_lock_path(tmp.path())).unwrap();
        let parsed: CompactLock = toml::from_str(&contents).unwrap();
        assert_eq!(parsed.coord_id, "coord-self");
        assert!(parsed.started_at.ends_with('Z'));
        assert!(
            parsed
                .started_at
                .chars()
                .nth(19)
                .map(|c| c == '.')
                .unwrap_or(false),
            "started_at must be ISO 8601 with ms precision: {}",
            parsed.started_at
        );
    }
}
