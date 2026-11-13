//! Retention for koto's entries in a Claude Code `/workflows` directory.
//!
//! koto writes one `koto-<session-id>.json` per session into a directory it
//! shares with Claude Code's own `wf_*.json` files, and no koto command ever
//! learns that directory again once the session is gone. So the only place
//! that can bound it is the write itself: [`apply`] runs at the end of every
//! entry write (`try_materialize`) and sweeps the directory it just wrote
//! into.
//!
//! The sweep keeps at most [`KEEP`] koto-owned entries. It removes finished
//! entries (a top-level `status` other than `running`) oldest `startTime`
//! first, and never removes a live entry or the entry just written, so a
//! directory with more than [`KEEP`] live sessions holds more until they
//! finish. A file is koto's only when its name is `koto-<id>.json` with a
//! session-id-shaped id and its content carries koto's marker (see [`owned`]);
//! anything else is left exactly as it was and doesn't count toward the
//! bound, since a false removal in a shared directory can't be undone.
//!
//! A session whose driver died leaves its entry `running` forever, so the
//! sweep also reconciles stale entries: an owned entry still `running` whose
//! last write is more than [`STALE_AFTER`] ago is rewritten as `killed` (see
//! [`reconciled`] for the exact edit) and from then on counts as finished.
//! koto can't tell a dead driver from a quiet one; the window is the
//! dashboard's own threshold for calling a session stalled, and the session's
//! next commit rewrites its entry from its projection, `running` again.
//!
//! Every step is best-effort: the sweep returns counts, never an error, so it
//! can't change the outcome of the commit that triggered it.

use std::io::Read;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::contract::{cut_escaped, ScreenStatus, CONTRACT_VERSION};
use super::materialize::stage;

/// How many koto-owned entries a `/workflows` directory keeps.
///
/// Ten koto rows beside Claude Code's own entries fit one terminal screen,
/// and the full history stays in `koto dashboard` and `koto session list`.
/// It is a constant rather than a setting; [`Limits`] carries it so tests can
/// run the sweep with a smaller bound.
pub const KEEP: usize = 10;

/// How long an entry can go unwritten and still read `running`.
///
/// The dashboard calls a session silent this long stalled
/// (`cli::dashboard_data::liveness::STALLED_THRESHOLD`); a test pins the two
/// equal so the file and the dashboard never disagree about the same session.
pub const STALE_AFTER: Duration = Duration::from_secs(2 * 60 * 60);

/// The most bytes the sweep reads from one file. An entry koto writes is a
/// few kilobytes; a larger file is not one of koto's.
const READ_CAP: u64 = 1024 * 1024;

/// The limits a sweep runs under. [`Limits::default`] is what every koto
/// write uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Owned entries the directory keeps, the entry just written included.
    pub keep: usize,
    /// How long a `running` entry may go unwritten before it is reconciled.
    pub stale_after: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            keep: KEEP,
            stale_after: STALE_AFTER,
        }
    }
}

/// What one sweep did, for tests and diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sweep {
    /// Regular files whose name matches `koto-<id>.json`, the entry just
    /// written included.
    pub matched: usize,
    /// Files the sweep opened and read. Zero on the fast path.
    pub read: usize,
    /// Owned entries before removal, the entry just written included.
    /// Zero on the fast path, where nothing is read.
    pub owned: usize,
    /// Stale entries rewritten as `killed`.
    pub reconciled: usize,
    /// Entries removed.
    pub removed: usize,
}

/// One owned entry other than the one just written.
struct Candidate {
    name: String,
    /// The ordering key: `startTime` when the entry has a numeric one, else
    /// the file's modification time, both in epoch milliseconds.
    key: u64,
}

/// A name-matching file from the listing, other than the one just written.
struct Listed {
    name: String,
    id: String,
    /// The listing's modification time, read without following a link. Only
    /// decides whether the fast path holds; classification uses the time
    /// read from the opened file.
    modified: Option<SystemTime>,
}

/// Sweep `dir` after koto wrote `written` into it, at time `now`.
///
/// Lists `dir` once, with the modification time of each file that matches
/// koto's entry name. When at most `limits.keep` match and none but `written`
/// was last written more than `limits.stale_after` before `now`, returns
/// having opened nothing. Otherwise reads each name-matching file except
/// `written` and keeps the owned ones. A stale one (`running`, last written
/// more than the window ago) is rewritten as `killed` and joins the finished
/// entries; then finished owned entries are removed from the oldest end
/// (`startTime`, else modification time, then filename in ascending byte
/// order) until `limits.keep` owned entries remain or no finished one is
/// left. A removal that finds the file already gone counts as done; any other
/// failure skips that file.
pub fn apply(dir: &Path, written: &Path, now: SystemTime, limits: &Limits) -> Sweep {
    let mut sweep = Sweep::default();
    let Ok(listing) = std::fs::read_dir(dir) else {
        return sweep;
    };
    let written_name = written.file_name();

    let mut others = Vec::new();
    for entry in listing.flatten() {
        // `DirEntry::file_type` doesn't follow links, so a symlink or a
        // directory named like an entry is never matched.
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let Some(id) = entry_id(name) else {
            continue;
        };
        sweep.matched += 1;
        if Some(file_name.as_os_str()) == written_name {
            continue;
        }
        // `DirEntry::metadata` doesn't follow links either; a stat, not an
        // open, so the fast path still reads no file.
        let modified = entry.metadata().ok().and_then(|m| m.modified().ok());
        others.push(Listed {
            name: name.to_string(),
            id: id.to_string(),
            modified,
        });
    }

    let any_old = others.iter().any(|o| {
        o.modified
            .is_some_and(|m| is_stale(m, now, limits.stale_after))
    });
    if sweep.matched <= limits.keep && !any_old {
        return sweep;
    }

    // The entry just written is owned: koto wrote it a moment ago.
    sweep.owned = 1;
    let mut candidates = Vec::new();
    for listed in others {
        sweep.read += 1;
        let Some(entry) = owned(&dir.join(&listed.name), &listed.id) else {
            continue;
        };
        sweep.owned += 1;
        // A stale entry that is reconciled is finished from here on; one
        // whose rewrite didn't land is still live and stays.
        let finished = if entry.status != "running" {
            true
        } else if is_stale(entry.modified, now, limits.stale_after)
            && reconcile(dir, &listed.name, &entry)
        {
            sweep.reconciled += 1;
            true
        } else {
            false
        };
        if finished {
            candidates.push(Candidate {
                key: entry.start_time.unwrap_or_else(|| millis(entry.modified)),
                name: listed.name,
            });
        }
    }

    // Oldest first; equal keys in ascending filename order. The names are
    // ASCII (they passed the session-id check), so `str` order is byte order.
    candidates.sort_by(|a, b| a.key.cmp(&b.key).then_with(|| a.name.cmp(&b.name)));

    let mut remaining = sweep.owned;
    for candidate in candidates {
        if remaining <= limits.keep {
            break;
        }
        match std::fs::remove_file(dir.join(&candidate.name)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                eprintln!(
                    "warning: /workflows retention left {}: {:?}",
                    candidate.name,
                    e.kind()
                );
                continue;
            }
        }
        sweep.removed += 1;
        remaining -= 1;
    }
    sweep
}

/// Whether a file last written at `last_write` is stale at `now`: written
/// more than `window` ago. A write exactly `window` ago, or one in the future
/// (a clock that moved back, a file from another machine), is live.
fn is_stale(last_write: SystemTime, now: SystemTime, window: Duration) -> bool {
    now.duration_since(last_write).is_ok_and(|age| age > window)
}

fn millis(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `t` as UTC to the minute: `YYYY-MM-DD HH:MM UTC`.
fn utc_minute(t: SystemTime) -> String {
    // `iso8601_at` writes `YYYY-MM-DDTHH:MM:SS.mmmZ`, all ASCII.
    let iso = crate::engine::types::iso8601_at(t);
    format!("{} {} UTC", &iso[..10], &iso[11..16])
}

/// The entry `entry` rewritten as stale, its session last heard from at
/// `last_write`.
///
/// Three changes and a version: the top-level `status` becomes `killed`, the
/// `summary` gains ` · no activity since <YYYY-MM-DD HH:MM UTC>`, the `koto`
/// block gains `staleSince` (the last write in epoch milliseconds), and
/// `koto.contractVersion` becomes [`CONTRACT_VERSION`], since the entry now
/// carries a version-6 field whatever version wrote the rest. Everything else
/// is kept, `koto.status` included: koto's last classification of the
/// session's log still stands, as the log hasn't changed.
///
/// The summary is cut before the suffix when the two together would pass the
/// fact cap, as a blocked summary is cut before its gate. An entry written
/// before contract version 4 has no summary; it gets the suffix's words alone.
pub fn reconciled(entry: &Value, last_write: SystemTime) -> Value {
    use crate::session_view::{CUT_MARKER, FACT_CAP};
    let mut value = entry.clone();
    let Some(object) = value.as_object_mut() else {
        return value;
    };
    let words = format!("no activity since {}", utc_minute(last_write));
    let summary = match object.get("summary").and_then(Value::as_str) {
        Some(summary) => {
            let suffix = format!(" \u{b7} {words}");
            if summary.chars().count() + suffix.chars().count() <= FACT_CAP {
                format!("{summary}{suffix}")
            } else {
                let budget =
                    FACT_CAP.saturating_sub(suffix.chars().count() + CUT_MARKER.chars().count());
                let (kept, _) = cut_escaped(summary, budget);
                format!("{kept}{CUT_MARKER}{suffix}")
            }
        }
        None => words,
    };
    object.insert("summary".to_string(), Value::String(summary));
    object.insert(
        "status".to_string(),
        serde_json::to_value(ScreenStatus::Killed).unwrap_or(Value::Null),
    );
    if let Some(koto) = object.get_mut("koto").and_then(Value::as_object_mut) {
        koto.insert("staleSince".to_string(), Value::from(millis(last_write)));
        koto.insert("contractVersion".to_string(), Value::from(CONTRACT_VERSION));
    }
    value
}

/// Rewrite the stale entry `name` in `dir` as [`reconciled`] says; `true`
/// when the rewrite landed.
///
/// The new content is staged in a temp file in the same directory, the way
/// every koto entry write is, so a reader never sees half a file and the
/// rename replaces the path rather than writing through it. Right before the
/// rename the entry's modification time and length are read again: if either
/// changed since the sweep judged it, its session committed meanwhile and the
/// entry is left as that commit wrote it. What remains between that check and
/// the rename is a few system calls; a commit landing there reads `killed`
/// until the session's next commit. Any failure leaves the entry as it was.
fn reconcile(dir: &Path, name: &str, entry: &OwnedEntry) -> bool {
    let path = dir.join(name);
    let Ok(mut bytes) = serde_json::to_vec_pretty(&reconciled(&entry.value, entry.modified)) else {
        return false;
    };
    bytes.push(b'\n');
    let staged = match stage(dir, &bytes) {
        Ok(staged) => staged,
        Err(e) => {
            eprintln!(
                "warning: /workflows retention could not reconcile {name}: {:?}",
                e.kind()
            );
            return false;
        }
    };
    let unchanged = std::fs::symlink_metadata(&path).is_ok_and(|m| {
        m.is_file() && m.len() == entry.len && m.modified().ok() == Some(entry.modified)
    });
    if !unchanged {
        // Dropping the staged file removes it.
        return false;
    }
    match staged.persist(&path) {
        Ok(_) => true,
        Err(e) => {
            eprintln!(
                "warning: /workflows retention could not reconcile {name}: {:?}",
                e.error.kind()
            );
            false
        }
    }
}

/// The session id in an entry's file name, when the name is
/// `koto-<id>.json` and `<id>` uses only the characters a session id can
/// hold (letters, digits, `.`, `_`, `~`, `-`), which admit no path
/// separator.
///
/// The id is the session's init-time UUID, which can start with a digit, so
/// this is the character allowlist of `validate_session_id` without its
/// leading-letter rule (a session *name* must start with a letter; the UUID
/// in the file name needn't).
fn entry_id(name: &str) -> Option<&str> {
    let id = name.strip_prefix("koto-")?.strip_suffix(".json")?;
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '-');
    if id.is_empty() || id == "." || id == ".." || !id.chars().all(allowed) {
        return None;
    }
    Some(id)
}

/// What the sweep needs from an owned entry.
struct OwnedEntry {
    /// The parsed entry, which a reconcile edits.
    value: Value,
    status: String,
    start_time: Option<u64>,
    /// The opened file's modification time and length, which a reconcile
    /// compares against right before its rename.
    modified: SystemTime,
    len: u64,
}

/// Read `path` and return it when it is koto's entry for session `id`.
///
/// The marker every contract version koto has written carries: a JSON object
/// whose `id` is `koto-<id>`, whose `koto.sessionId` is `<id>`, whose
/// `koto.contractVersion` is an unsigned integer and whose top-level
/// `status` is a string. A file that can't be read, is larger than
/// [`READ_CAP`], or lacks any part of the marker is not owned.
fn owned(path: &Path, id: &str) -> Option<OwnedEntry> {
    let file = std::fs::File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    let modified = metadata.modified().ok()?;
    let len = metadata.len();
    let mut bytes = Vec::new();
    file.take(READ_CAP + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > READ_CAP {
        return None;
    }
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let koto = value.get("koto")?;
    let marked = value.get("id")?.as_str()? == format!("koto-{id}")
        && koto.get("sessionId")?.as_str()? == id
        && koto.get("contractVersion")?.as_u64().is_some();
    if !marked {
        return None;
    }
    Some(OwnedEntry {
        status: value.get("status")?.as_str()?.to_string(),
        start_time: value.get("startTime").and_then(Value::as_u64),
        modified,
        len,
        value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use filetime::{set_file_mtime, FileTime};
    use serde_json::json;
    use std::path::PathBuf;

    /// The fixed clock: 2026-11-13 04:58:00 UTC.
    const NOW_SECS: i64 = 1_794_545_880;
    /// Three hours before [`NOW_SECS`]: 2026-11-13 01:58:00 UTC.
    const QUIET_SINCE_SECS: i64 = NOW_SECS - 3 * 60 * 60;

    fn at(secs: i64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs as u64)
    }

    fn now() -> SystemTime {
        at(NOW_SECS)
    }

    /// A current-contract entry for session `id`, as koto's projection writes
    /// it (minus the phase detail).
    fn entry(id: &str, status: &str, koto_status: &str, start_time: u64) -> Value {
        json!({
            "id": format!("koto-{id}"),
            "name": id,
            "workflowName": id,
            "summary": format!("koto \u{b7} {id} \u{b7} implement"),
            "status": status,
            "startTime": start_time,
            "koto": {
                "sessionId": id,
                "workflow": id,
                "currentState": "implement",
                "contractVersion": 5,
                "status": koto_status,
            },
        })
    }

    /// Write `value` as session `id`'s entry in `dir`, last written at
    /// `mtime_secs`, and return its path. Every seeded file gets an explicit
    /// modification time, so the fixed clock alone decides what is stale.
    fn seed(dir: &Path, id: &str, value: &Value, mtime_secs: i64) -> PathBuf {
        let path = dir.join(format!("koto-{id}.json"));
        std::fs::write(&path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
        set_file_mtime(&path, FileTime::from_unix_time(mtime_secs, 0)).unwrap();
        path
    }

    fn read(path: &Path) -> Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    fn temp_files(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".koto-wf-"))
            .collect()
    }

    #[test]
    fn the_stale_window_is_the_dashboards_stalled_threshold() {
        assert_eq!(
            STALE_AFTER,
            crate::cli::dashboard_data::liveness::STALLED_THRESHOLD
        );
        assert_eq!(Limits::default().stale_after, STALE_AFTER);
        assert_eq!(Limits::default().keep, KEEP);
    }

    /// A write from another session reconciles an entry quiet for three
    /// hours: `killed`, the summary says since when, `staleSince` carries the
    /// same instant, and koto's own classification and every other field
    /// stay.
    #[test]
    fn a_write_from_another_session_reconciles_a_quiet_entry() {
        let dir = tempfile::tempdir().unwrap();
        let quiet = seed(
            dir.path(),
            "quiet",
            &entry("quiet", "running", "blocked", 1_000),
            QUIET_SINCE_SECS,
        );
        let written = seed(
            dir.path(),
            "busy",
            &entry("busy", "running", "running", 2_000),
            NOW_SECS,
        );

        let sweep = apply(dir.path(), &written, now(), &Limits::default());

        assert_eq!(sweep.reconciled, 1);
        assert_eq!(sweep.removed, 0);
        let v = read(&quiet);
        assert_eq!(v["status"], "killed");
        assert_eq!(
            v["summary"],
            "koto \u{b7} quiet \u{b7} implement \u{b7} no activity since 2026-11-13 01:58 UTC"
        );
        assert_eq!(v["koto"]["staleSince"], 1_794_535_080_000u64);
        assert_eq!(v["koto"]["status"], "blocked");
        assert_eq!(v["koto"]["contractVersion"], 6);
        assert_eq!(v["koto"]["contractVersion"], CONTRACT_VERSION);
        assert_eq!(v["startTime"], 1_000);
        assert_eq!(v["workflowName"], "quiet");
        assert_eq!(v["koto"]["currentState"], "implement");
        assert_eq!(read(&written)["status"], "running");
        assert!(temp_files(dir.path()).is_empty());
    }

    /// Exactly the window is live, one second past it is stale, and a last
    /// write in the future is live.
    #[test]
    fn the_window_is_strict_and_a_future_write_is_live() {
        let dir = tempfile::tempdir().unwrap();
        let window = STALE_AFTER.as_secs() as i64;
        let exactly = seed(
            dir.path(),
            "exactly",
            &entry("exactly", "running", "running", 1),
            NOW_SECS - window,
        );
        let past = seed(
            dir.path(),
            "past",
            &entry("past", "running", "running", 2),
            NOW_SECS - window - 1,
        );
        let future = seed(
            dir.path(),
            "future",
            &entry("future", "running", "running", 3),
            NOW_SECS + 60 * 60,
        );
        let written = seed(
            dir.path(),
            "written",
            &entry("written", "running", "running", 4),
            NOW_SECS,
        );

        let sweep = apply(dir.path(), &written, now(), &Limits::default());

        assert_eq!(sweep.reconciled, 1);
        assert_eq!(read(&exactly)["status"], "running");
        assert_eq!(read(&past)["status"], "killed");
        assert_eq!(read(&future)["status"], "running");
        assert!(read(&exactly)["koto"].get("staleSince").is_none());
        assert!(read(&future)["koto"].get("staleSince").is_none());
    }

    /// A finished entry, however old, is never rewritten; a directory within
    /// the bound with nothing old is not read at all.
    #[test]
    fn only_a_running_entry_is_reconciled_and_a_fresh_directory_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let done = entry("done", "completed", "completed", 1);
        let done_path = seed(dir.path(), "done", &done, QUIET_SINCE_SECS);
        let before = std::fs::read(&done_path).unwrap();
        let written = seed(
            dir.path(),
            "written",
            &entry("written", "running", "running", 2),
            NOW_SECS,
        );

        let sweep = apply(dir.path(), &written, now(), &Limits::default());
        assert_eq!(
            sweep.read, 1,
            "an old file takes the sweep off the fast path"
        );
        assert_eq!(sweep.reconciled, 0);
        assert_eq!(std::fs::read(&done_path).unwrap(), before);

        set_file_mtime(&done_path, FileTime::from_unix_time(NOW_SECS - 60, 0)).unwrap();
        let sweep = apply(dir.path(), &written, now(), &Limits::default());
        assert_eq!(sweep.read, 0);
    }

    /// The owning session commits after the sweep judged its entry stale and
    /// before the rewrite: the rewrite is dropped and the entry holds the
    /// commit's content, still `running`.
    #[test]
    fn a_commit_between_judgment_and_rewrite_wins() {
        let dir = tempfile::tempdir().unwrap();
        let path = seed(
            dir.path(),
            "quiet",
            &entry("quiet", "running", "running", 1_000),
            QUIET_SINCE_SECS,
        );
        let judged = owned(&path, "quiet").unwrap();
        assert!(is_stale(judged.modified, now(), STALE_AFTER));

        let mut committed = entry("quiet", "running", "running", 1_000);
        committed["koto"]["currentState"] = json!("review");
        let committed_bytes = serde_json::to_vec_pretty(&committed).unwrap();
        std::fs::write(&path, &committed_bytes).unwrap();

        assert!(!reconcile(dir.path(), "koto-quiet.json", &judged));
        assert_eq!(std::fs::read(&path).unwrap(), committed_bytes);
        assert_eq!(read(&path)["status"], "running");
        assert!(
            temp_files(dir.path()).is_empty(),
            "the staged rewrite is removed"
        );
    }

    /// Over the bound, a reconciled entry is finished like any other and goes
    /// in `startTime` order.
    #[test]
    fn a_reconciled_entry_is_evicted_in_start_time_order() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = NOW_SECS - 60;
        let first = seed(
            dir.path(),
            "first",
            &entry("first", "completed", "completed", 10),
            fresh,
        );
        let stale = seed(
            dir.path(),
            "stale",
            &entry("stale", "running", "running", 20),
            QUIET_SINCE_SECS,
        );
        let third = seed(
            dir.path(),
            "third",
            &entry("third", "completed", "completed", 30),
            fresh,
        );
        let fourth = seed(
            dir.path(),
            "fourth",
            &entry("fourth", "failed", "failed", 40),
            fresh,
        );
        let written = seed(
            dir.path(),
            "written",
            &entry("written", "running", "running", 50),
            NOW_SECS,
        );
        let limits = Limits {
            keep: 3,
            ..Limits::default()
        };

        let sweep = apply(dir.path(), &written, now(), &limits);

        assert_eq!(sweep.reconciled, 1);
        assert_eq!(sweep.removed, 2);
        assert!(!first.exists());
        assert!(
            !stale.exists(),
            "the reconciled entry goes after startTime 10"
        );
        assert!(third.exists() && fourth.exists() && written.exists());
    }

    /// A rewrite that can't land (the directory is read-only) leaves the
    /// entry exactly as it was, and the sweep still returns its counts.
    #[cfg(unix)]
    #[test]
    fn a_failed_rewrite_leaves_the_entry_unchanged() {
        use std::os::unix::fs::PermissionsExt;
        // Root ignores the mode bits, so the rewrite would land.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let quiet = seed(
            dir.path(),
            "quiet",
            &entry("quiet", "running", "running", 1),
            QUIET_SINCE_SECS,
        );
        let written = seed(
            dir.path(),
            "written",
            &entry("written", "running", "running", 2),
            NOW_SECS,
        );
        let before = std::fs::read(&quiet).unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();

        let sweep = apply(dir.path(), &written, now(), &Limits::default());

        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(sweep.reconciled, 0);
        assert_eq!(sweep.removed, 0);
        assert_eq!(std::fs::read(&quiet).unwrap(), before);
        assert!(temp_files(dir.path()).is_empty());
    }

    /// A summary near the fact cap is cut before the suffix, which survives
    /// whole within the cap.
    #[test]
    fn a_long_summary_is_cut_before_the_suffix() {
        use crate::session_view::{CUT_MARKER, FACT_CAP};
        let mut value = entry("long", "running", "running", 1);
        value["summary"] = json!("s".repeat(FACT_CAP));
        let out = reconciled(&value, at(QUIET_SINCE_SECS));
        let summary = out["summary"].as_str().unwrap();
        assert!(summary.chars().count() <= FACT_CAP, "{}", summary.len());
        assert!(
            summary.ends_with(&format!(
                "{CUT_MARKER} \u{b7} no activity since 2026-11-13 01:58 UTC"
            )),
            "{summary}"
        );
    }

    /// An entry from before contract version 4 has no summary; it gets the
    /// suffix's words, and the version moves to 6.
    #[test]
    fn an_entry_without_a_summary_gets_the_words_alone() {
        let v1 = json!({
            "id": "koto-old",
            "name": "old",
            "status": "running",
            "startTime": 1,
            "koto": {"sessionId": "old", "workflow": "old", "currentState": "s", "contractVersion": 1},
        });
        let out = reconciled(&v1, at(QUIET_SINCE_SECS));
        assert_eq!(out["summary"], "no activity since 2026-11-13 01:58 UTC");
        assert_eq!(out["status"], "killed");
        assert_eq!(out["koto"]["contractVersion"], CONTRACT_VERSION);
        assert!(out["koto"].get("status").is_none());
    }
}
