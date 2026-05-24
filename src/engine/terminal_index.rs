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
}
