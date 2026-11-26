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
//! finish. A file is koto's only when it is a regular file named
//! `koto-<id>.json` with a session-id-shaped id and its content carries koto's
//! marker (see [`owned`]); anything else is left exactly as it was and doesn't
//! count toward the bound, since a false removal in a shared directory can't
//! be undone. A file the sweep judged is checked again right before it is
//! rewritten or removed, so one its session rewrote meanwhile stays as that
//! session wrote it.
//!
//! A session whose driver died leaves its entry `running` forever, so the
//! sweep also reconciles stale entries: an owned entry still `running` whose
//! last write is more than [`STALE_AFTER`] ago is rewritten as `killed` (see
//! [`reconciled`] for the exact edit) and from then on counts as finished.
//! koto can't tell a dead driver from a quiet one; the window is the
//! dashboard's own threshold for calling a session stalled, and the session's
//! next commit rewrites its entry from its projection, `running` again.
//!
//! A koto command killed between staging an entry and renaming it leaves a
//! `.koto-wf-*.tmp` file behind, so the sweep also clears those once they are
//! older than [`TEMP_MIN_AGE`].
//!
//! Every step is best-effort: the sweep returns counts, never an error, so it
//! can't change the outcome of the commit that triggered it.

use std::fs::{File, Metadata};
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

/// How old a `.koto-wf-*.tmp` file must be before the sweep removes it.
///
/// A write stages and renames its temp file within milliseconds, so one a
/// minute old was left by a koto command that died between the two. The
/// batch scheduler clears its own temp files after the same age
/// (`cli::batch::TEMPFILE_SWEEP_MIN_AGE`); a test pins the two equal.
pub const TEMP_MIN_AGE: Duration = Duration::from_secs(60);

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
    /// Files the sweep opened and read. Within the bound, only those last
    /// written more than the stale window ago.
    pub read: usize,
    /// Owned entries among those read, plus the entry just written. Zero
    /// when nothing was read.
    pub owned: usize,
    /// Stale entries rewritten as `killed`.
    pub reconciled: usize,
    /// Entries removed, by this sweep or by another process after this
    /// sweep listed them.
    pub removed: usize,
    /// koto's own `.koto-wf-*.tmp` files removed.
    pub temps_removed: usize,
}

/// Who a file is, as far as a later check can tell: its last write, its
/// length and, on Unix, its inode. koto replaces an entry by renaming a new
/// file over it, so a commit always changes the inode even within one
/// timestamp tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: SystemTime,
    len: u64,
    inode: u64,
}

impl Stamp {
    /// The stamp of a regular file; `None` for anything else, or when the
    /// platform can't give a modification time.
    fn of(metadata: &Metadata) -> Option<Stamp> {
        if !metadata.is_file() {
            return None;
        }
        #[cfg(unix)]
        let inode = std::os::unix::fs::MetadataExt::ino(metadata);
        #[cfg(not(unix))]
        let inode = 0;
        Some(Stamp {
            modified: metadata.modified().ok()?,
            len: metadata.len(),
            inode,
        })
    }

    /// The stamp `path` has now, read without following a link.
    fn at(path: &Path) -> std::io::Result<Option<Stamp>> {
        std::fs::symlink_metadata(path).map(|m| Stamp::of(&m))
    }
}

/// An owned, finished entry other than the one just written: a candidate
/// for removal.
struct Candidate {
    name: String,
    /// The ordering key: `startTime` when the entry has a numeric one, else
    /// the file's modification time, both in epoch milliseconds.
    key: u64,
    /// The file as the sweep judged it, checked again before removal.
    stamp: Stamp,
}

/// A name-matching file from the listing, other than the one just written.
struct Listed {
    name: String,
    id: String,
    /// The listing's modification time, read without following a link. Only
    /// decides which files get read; classification uses the time read from
    /// the opened file.
    modified: Option<SystemTime>,
}

/// What the one listing of a directory found.
struct Listing {
    /// Name-matching regular files, the entry just written included.
    matched: usize,
    /// Name-matching regular files other than the entry just written.
    others: Vec<Listed>,
    /// `.koto-wf-*.tmp` regular files and their modification times.
    temps: Vec<(String, SystemTime)>,
}

/// Sweep `dir` after koto wrote `written` into it, at time `now`.
///
/// Lists `dir` once, with a stat (not following links) of each regular file
/// named like an entry or like koto's temp file, and clears temp files older
/// than [`TEMP_MIN_AGE`]. Within the bound (at most `limits.keep` matching
/// files) it reads only the matching files last written more than
/// `limits.stale_after` before `now`, which are the only ones that can need a
/// rewrite; past the bound it reads every matching file except `written`.
/// Of the files read it keeps the owned ones. A stale one (`running`, last
/// written more than the window ago) is rewritten as `killed` and joins the
/// finished entries; then finished owned entries are removed from the oldest
/// end (`startTime`, else modification time, then filename in ascending byte
/// order) until `limits.keep` owned entries remain or no finished one is left.
/// A candidate rewritten since it was read is kept; one already gone counts
/// as removed; any other failure skips that file.
pub fn apply(dir: &Path, written: &Path, now: SystemTime, limits: &Limits) -> Sweep {
    match list(dir, written) {
        Some(listing) => finish(dir, listing, now, limits),
        None => Sweep::default(),
    }
}

/// Everything [`apply`] does after its listing.
fn finish(dir: &Path, listing: Listing, now: SystemTime, limits: &Limits) -> Sweep {
    let mut sweep = Sweep {
        matched: listing.matched,
        ..Sweep::default()
    };
    sweep.temps_removed = clear_temps(dir, &listing.temps, now);
    let candidates = classify(dir, listing.others, now, limits, &mut sweep);
    evict(dir, candidates, limits, &mut sweep);
    sweep
}

/// List `dir` once. `None` when it can't be listed.
fn list(dir: &Path, written: &Path) -> Option<Listing> {
    let entries = std::fs::read_dir(dir).ok()?;
    let written_name = written.file_name();
    let mut listing = Listing {
        matched: 0,
        others: Vec::new(),
        temps: Vec::new(),
    };
    for entry in entries.flatten() {
        // `DirEntry::file_type` doesn't follow links, so a symlink, a FIFO or
        // a directory named like an entry is never matched.
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        // `DirEntry::metadata` doesn't follow links either; a stat, not an
        // open.
        let modified = || entry.metadata().ok().and_then(|m| m.modified().ok());
        if is_temp(name) {
            if let Some(modified) = modified() {
                listing.temps.push((name.to_string(), modified));
            }
            continue;
        }
        let Some(id) = entry_id(name) else {
            continue;
        };
        listing.matched += 1;
        if Some(file_name.as_os_str()) == written_name {
            continue;
        }
        listing.others.push(Listed {
            name: name.to_string(),
            id: id.to_string(),
            modified: modified(),
        });
    }
    Some(listing)
}

/// Whether `name` is one of the temp files koto stages entries in
/// (`materialize::stage`).
fn is_temp(name: &str) -> bool {
    name.starts_with(".koto-wf-") && name.ends_with(".tmp")
}

/// Remove each listed temp file last written more than [`TEMP_MIN_AGE`]
/// before `now` and still the same regular file; return how many went.
fn clear_temps(dir: &Path, temps: &[(String, SystemTime)], now: SystemTime) -> usize {
    let mut removed = 0;
    for (name, modified) in temps {
        if !now
            .duration_since(*modified)
            .is_ok_and(|age| age > TEMP_MIN_AGE)
        {
            continue;
        }
        let path = dir.join(name);
        // A link or directory swapped in under the name since the listing
        // is not koto's temp file.
        let still = std::fs::symlink_metadata(&path)
            .is_ok_and(|m| m.is_file() && m.modified().ok() == Some(*modified));
        if still && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Read the listed files the sweep needs, reconcile the stale ones, and
/// return the finished owned entries.
fn classify(
    dir: &Path,
    others: Vec<Listed>,
    now: SystemTime,
    limits: &Limits,
    sweep: &mut Sweep,
) -> Vec<Candidate> {
    // Within the bound nothing is removed, so only a file old enough to be
    // stale can need anything; past it, every file is a possible candidate.
    let over = sweep.matched > limits.keep;
    let mut candidates = Vec::new();
    for listed in others {
        let old = listed
            .modified
            .is_some_and(|m| is_stale(m, now, limits.stale_after));
        if !over && !old {
            continue;
        }
        sweep.read += 1;
        let Some(entry) = owned(&dir.join(&listed.name), &listed.id) else {
            continue;
        };
        sweep.owned += 1;
        // A stale entry that is reconciled is finished from here on, with
        // the stamp of the file the rewrite put in place; one whose rewrite
        // didn't land is still live and stays.
        let stamp = if entry.status != "running" {
            Some(entry.stamp)
        } else if is_stale(entry.stamp.modified, now, limits.stale_after) {
            let rewritten = reconcile(dir, &listed.name, &entry);
            if rewritten.is_some() {
                sweep.reconciled += 1;
            }
            rewritten
        } else {
            None
        };
        if let Some(stamp) = stamp {
            candidates.push(Candidate {
                key: entry
                    .start_time
                    .unwrap_or_else(|| millis(entry.stamp.modified)),
                name: listed.name,
                stamp,
            });
        }
    }
    // The entry just written is owned: koto wrote it a moment ago.
    if sweep.read > 0 {
        sweep.owned += 1;
    }
    candidates
}

/// Remove finished entries oldest first until `limits.keep` owned entries
/// remain or no candidate is left.
fn evict(dir: &Path, mut candidates: Vec<Candidate>, limits: &Limits, sweep: &mut Sweep) {
    // Oldest first; equal keys in ascending filename order. The names are
    // ASCII (they passed the session-id check), so `str` order is byte order.
    candidates.sort_by(|a, b| a.key.cmp(&b.key).then_with(|| a.name.cmp(&b.name)));

    let mut remaining = sweep.owned;
    for candidate in candidates {
        if remaining <= limits.keep {
            break;
        }
        if remove_unchanged(dir, &candidate) {
            sweep.removed += 1;
            remaining -= 1;
        }
    }
}

/// Remove `candidate` when it is still the file the sweep judged; `true`
/// when it is gone.
///
/// A file whose stamp changed was rewritten since it was read, most likely
/// by its session committing again as `running`, and stays. What remains
/// between that check and the removal is one system call. A file already
/// gone (another koto command's sweep took it) counts as removed. Any other
/// failure leaves the file and warns once, naming it.
fn remove_unchanged(dir: &Path, candidate: &Candidate) -> bool {
    let path = dir.join(&candidate.name);
    match Stamp::at(&path) {
        Ok(stamp) if stamp == Some(candidate.stamp) => {}
        Ok(_) => return false,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
        Err(e) => {
            warn_left(&candidate.name, &e);
            return false;
        }
    }
    match std::fs::remove_file(&path) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(e) => {
            warn_left(&candidate.name, &e);
            false
        }
    }
}

fn warn_left(name: &str, e: &std::io::Error) {
    eprintln!("warning: /workflows retention left {name}: {:?}", e.kind());
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

/// Rewrite the stale entry `name` in `dir` as [`reconciled`] says; the stamp
/// of the file now in place when the rewrite landed.
///
/// The new content is staged in a temp file in the same directory, the way
/// every koto entry write is, so a reader never sees half a file and the
/// rename replaces the path rather than writing through it. A rename keeps
/// the staged file's stamp, so the one returned is read from the staged file
/// before the rename, not from the path after it, which a commit landing
/// right after the rename could already have replaced. Right before the
/// rename the entry's stamp is read again: if it changed since the sweep
/// judged the entry, its session committed meanwhile and the entry is left as
/// that commit wrote it. What remains between that check and the rename is a
/// few system calls; a commit landing there reads `killed` until the
/// session's next commit. Any failure leaves the entry as it was.
fn reconcile(dir: &Path, name: &str, entry: &OwnedEntry) -> Option<Stamp> {
    let path = dir.join(name);
    let mut bytes =
        serde_json::to_vec_pretty(&reconciled(&entry.value, entry.stamp.modified)).ok()?;
    bytes.push(b'\n');
    let staged = match stage(dir, &bytes) {
        Ok(staged) => staged,
        Err(e) => {
            eprintln!(
                "warning: /workflows retention could not reconcile {name}: {:?}",
                e.kind()
            );
            return None;
        }
    };
    // Dropping the staged file on any early return removes it.
    let stamp = staged
        .as_file()
        .metadata()
        .ok()
        .and_then(|m| Stamp::of(&m))?;
    if !Stamp::at(&path).is_ok_and(|now| now == Some(entry.stamp)) {
        return None;
    }
    match staged.persist(&path) {
        Ok(_) => Some(stamp),
        Err(e) => {
            eprintln!(
                "warning: /workflows retention could not reconcile {name}: {:?}",
                e.error.kind()
            );
            None
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
    /// The opened file's stamp, which a reconcile or a removal compares
    /// against right before it acts.
    stamp: Stamp,
}

/// Open `path` for reading without following a link and without blocking.
///
/// The listing saw a regular file, but the name can be swapped before the
/// open: `O_NOFOLLOW` fails the open on a symlink instead of reading
/// whatever it points at, and `O_NONBLOCK` makes the open of a FIFO return
/// at once instead of waiting for a writer, which would hang the koto
/// command. The caller checks the handle's type before reading.
#[cfg(unix)]
fn open_no_follow(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

/// Without `O_NOFOLLOW`, refuse a link by its own metadata before opening;
/// a swap between the two calls is the window this leaves.
#[cfg(not(unix))]
fn open_no_follow(path: &Path) -> std::io::Result<File> {
    if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(std::io::Error::other("symlink"));
    }
    File::open(path)
}

/// Read `path` and return it when it is koto's entry for session `id`.
///
/// The marker every contract version koto has written carries: a JSON object
/// whose `id` is `koto-<id>`, whose `koto.sessionId` is `<id>`, whose
/// `koto.contractVersion` is an unsigned integer and whose top-level
/// `status` is a string. A file that isn't a regular file when opened (see
/// [`open_no_follow`]), can't be read, is larger than [`READ_CAP`], or lacks
/// any part of the marker is not owned.
fn owned(path: &Path, id: &str) -> Option<OwnedEntry> {
    let file = open_no_follow(path).ok()?;
    // The handle's own metadata (`fstat`): what was opened, not whatever
    // the name points at by now.
    let stamp = Stamp::of(&file.metadata().ok()?)?;
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
        stamp,
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
        assert!(is_stale(judged.stamp.modified, now(), STALE_AFTER));

        let mut committed = entry("quiet", "running", "running", 1_000);
        committed["koto"]["currentState"] = json!("review");
        let committed_bytes = serde_json::to_vec_pretty(&committed).unwrap();
        std::fs::write(&path, &committed_bytes).unwrap();

        assert!(reconcile(dir.path(), "koto-quiet.json", &judged).is_none());
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
    ///
    /// This runs at the sweep, not through a koto command: a read-only
    /// directory fails the command's own entry write first, so the sweep
    /// would never run, and an unwritable entry file doesn't stop a rename
    /// over it. The command's result can't change anyway, since `apply`
    /// returns counts and `try_materialize` ignores them.
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

    /// Seed `n` finished entries `<prefix>NN` in `dir` with `startTime`s
    /// from `first_start` up, last written a minute before the fixed clock,
    /// and return their paths oldest first.
    fn seed_finished(dir: &Path, prefix: &str, n: usize, first_start: u64) -> Vec<PathBuf> {
        (0..n)
            .map(|i| {
                let id = format!("{prefix}{i:02}");
                seed(
                    dir,
                    &id,
                    &entry(&id, "completed", "completed", first_start + i as u64),
                    NOW_SECS - 60,
                )
            })
            .collect()
    }

    fn written_entry(dir: &Path) -> PathBuf {
        seed(
            dir,
            "written",
            &entry("written", "running", "running", 9_999),
            NOW_SECS,
        )
    }

    fn keep(n: usize) -> Limits {
        Limits {
            keep: n,
            ..Limits::default()
        }
    }

    #[cfg(unix)]
    fn mkfifo(path: &Path) {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
    }

    #[cfg(unix)]
    fn is_root() -> bool {
        unsafe { libc::geteuid() == 0 }
    }

    #[test]
    fn the_temp_file_age_is_the_batch_schedulers() {
        assert_eq!(TEMP_MIN_AGE, Duration::from_secs(60));
        assert_eq!(TEMP_MIN_AGE, crate::cli::batch::TEMPFILE_SWEEP_MIN_AGE);
    }

    /// A listed entry swapped for a FIFO before the sweep opens it: the open
    /// returns at once, the FIFO isn't read as an entry, and it stays.
    #[cfg(unix)]
    #[test]
    fn a_fifo_swapped_in_after_the_listing_neither_blocks_nor_goes() {
        use std::os::unix::fs::FileTypeExt;
        let dir = tempfile::tempdir().unwrap();
        let finished = seed_finished(dir.path(), "done", 3, 1);
        let written = written_entry(dir.path());
        let listing = list(dir.path(), &written).unwrap();
        assert_eq!(listing.matched, 4);

        let fifo = finished[0].clone();
        std::fs::remove_file(&fifo).unwrap();
        mkfifo(&fifo);

        let (tx, rx) = std::sync::mpsc::channel();
        let path = dir.path().to_path_buf();
        std::thread::spawn(move || {
            let _ = tx.send(finish(&path, listing, now(), &keep(1)));
        });
        let sweep = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the sweep blocked on a FIFO");

        assert_eq!(sweep.read, 3);
        assert_eq!(sweep.owned, 3, "the FIFO is not an owned entry");
        assert_eq!(sweep.removed, 2);
        assert!(std::fs::symlink_metadata(&fifo)
            .unwrap()
            .file_type()
            .is_fifo());
        assert!(written.exists());
    }

    /// A listed entry swapped for a link to an owned-looking, stale entry
    /// elsewhere: the link isn't followed, so the target is neither counted
    /// nor changed, and the link stays.
    #[cfg(unix)]
    #[test]
    fn a_symlink_swapped_in_after_the_listing_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let target = seed(
            elsewhere.path(),
            "swap",
            &entry("swap", "running", "running", 1),
            QUIET_SINCE_SECS,
        );
        let target_bytes = std::fs::read(&target).unwrap();
        let a = seed(
            dir.path(),
            "a",
            &entry("a", "completed", "completed", 2),
            NOW_SECS - 60,
        );
        let swap = seed(
            dir.path(),
            "swap",
            &entry("swap", "completed", "completed", 3),
            NOW_SECS - 60,
        );
        let written = written_entry(dir.path());
        let listing = list(dir.path(), &written).unwrap();

        std::fs::remove_file(&swap).unwrap();
        std::os::unix::fs::symlink(&target, &swap).unwrap();
        let sweep = finish(dir.path(), listing, now(), &keep(1));

        assert_eq!(sweep.read, 2);
        assert_eq!(sweep.owned, 2, "only `a` and the entry just written");
        assert_eq!(sweep.reconciled, 0);
        assert_eq!(sweep.removed, 1);
        assert!(!a.exists());
        assert_eq!(std::fs::read_link(&swap).unwrap(), target);
        assert_eq!(std::fs::read(&target).unwrap(), target_bytes);
    }

    /// koto's temp files are cleared once they are over a minute old; a
    /// fresh one, and a link or directory with the same name shape, stay.
    #[cfg(unix)]
    #[test]
    fn temp_files_over_a_minute_old_are_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let temp = |name: &str, age: i64| {
            let path = dir.path().join(name);
            std::fs::write(&path, b"partial").unwrap();
            set_file_mtime(&path, FileTime::from_unix_time(NOW_SECS - age, 0)).unwrap();
            path
        };
        let old = temp(".koto-wf-old.tmp", 61);
        let fresh = temp(".koto-wf-fresh.tmp", 59);
        let other = temp(".koto-other-old.tmp", 3600);
        let link = dir.path().join(".koto-wf-link.tmp");
        std::os::unix::fs::symlink(&other, &link).unwrap();
        filetime::set_symlink_file_times(
            &link,
            FileTime::from_unix_time(NOW_SECS - 3600, 0),
            FileTime::from_unix_time(NOW_SECS - 3600, 0),
        )
        .unwrap();
        let subdir = dir.path().join(".koto-wf-dir.tmp");
        std::fs::create_dir(&subdir).unwrap();
        set_file_mtime(&subdir, FileTime::from_unix_time(NOW_SECS - 3600, 0)).unwrap();
        let written = written_entry(dir.path());

        let sweep = apply(dir.path(), &written, now(), &Limits::default());

        assert_eq!(sweep.temps_removed, 1);
        assert!(!old.exists());
        assert!(fresh.exists());
        assert!(other.exists(), "not koto's /workflows temp file");
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(subdir.is_dir());
    }

    /// A finished candidate its session rewrote as `running` after the sweep
    /// read it is not removed, and the next oldest goes instead.
    #[test]
    fn a_candidate_rewritten_after_it_was_read_stays() {
        let dir = tempfile::tempdir().unwrap();
        let finished = seed_finished(dir.path(), "done", 2, 1);
        let written = written_entry(dir.path());
        let listing = list(dir.path(), &written).unwrap();
        let limits = keep(1);
        let mut sweep = Sweep {
            matched: listing.matched,
            ..Sweep::default()
        };
        let candidates = classify(dir.path(), listing.others, now(), &limits, &mut sweep);
        assert_eq!(candidates.len(), 2);

        let running = serde_json::to_vec_pretty(&entry("done00", "running", "running", 1)).unwrap();
        std::fs::write(&finished[0], &running).unwrap();
        evict(dir.path(), candidates, &limits, &mut sweep);

        assert_eq!(sweep.removed, 1);
        assert_eq!(std::fs::read(&finished[0]).unwrap(), running);
        assert!(!finished[1].exists());
    }

    /// A candidate another process removed after the listing counts as
    /// removed, and the sweep goes on to the next one.
    #[test]
    fn a_candidate_that_vanished_counts_as_removed() {
        let dir = tempfile::tempdir().unwrap();
        let finished = seed_finished(dir.path(), "done", 3, 1);
        let written = written_entry(dir.path());
        let listing = list(dir.path(), &written).unwrap();
        let limits = keep(2);
        let mut sweep = Sweep {
            matched: listing.matched,
            ..Sweep::default()
        };
        let candidates = classify(dir.path(), listing.others, now(), &limits, &mut sweep);

        std::fs::remove_file(&finished[0]).unwrap();
        evict(dir.path(), candidates, &limits, &mut sweep);

        assert_eq!(sweep.removed, 2);
        assert!(!finished[1].exists());
        assert!(finished[2].exists() && written.exists());
    }

    /// A removal that fails for a reason other than not-found (the directory
    /// is read-only) skips the file, and the sweep still returns.
    #[cfg(unix)]
    #[test]
    fn an_unremovable_candidate_is_skipped() {
        use std::os::unix::fs::PermissionsExt;
        // Root ignores the mode bits, so the removal would succeed.
        if is_root() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let finished = seed_finished(dir.path(), "done", 3, 1);
        let written = written_entry(dir.path());
        let before: Vec<Vec<u8>> = finished.iter().map(|p| std::fs::read(p).unwrap()).collect();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();

        let sweep = apply(dir.path(), &written, now(), &keep(1));

        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(sweep.read, 3);
        assert_eq!(sweep.removed, 0);
        for (path, bytes) in finished.iter().zip(&before) {
            assert_eq!(&std::fs::read(path).unwrap(), bytes);
        }
    }

    /// Two sweeps of one directory at once both return, the directory ends at
    /// the bound, and only the oldest finished entries go.
    #[test]
    fn two_sweeps_at_once_end_at_the_bound() {
        let dir = tempfile::tempdir().unwrap();
        let finished = seed_finished(dir.path(), "done", 15, 100);
        let live: Vec<PathBuf> = (0..2)
            .map(|i| {
                let id = format!("live{i}");
                seed(
                    dir.path(),
                    &id,
                    &entry(&id, "running", "running", i),
                    NOW_SECS - 60,
                )
            })
            .collect();
        let written = written_entry(dir.path());
        let claude = dir.path().join("wf_test.json");
        std::fs::write(&claude, br#"{"id":"wf_test","status":"completed"}"#).unwrap();
        let kept: Vec<(PathBuf, Vec<u8>)> = live
            .iter()
            .chain(&finished[8..])
            .chain([&written, &claude])
            .map(|p| (p.clone(), std::fs::read(p).unwrap()))
            .collect();

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let sweeps: Vec<_> = (0..2)
            .map(|_| {
                let barrier = barrier.clone();
                let dir = dir.path().to_path_buf();
                let written = written.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    apply(&dir, &written, now(), &Limits::default())
                })
            })
            .collect();
        let removed: usize = sweeps.into_iter().map(|t| t.join().unwrap().removed).sum();

        assert!(removed >= 8, "{removed}");
        for path in &finished[..8] {
            assert!(!path.exists(), "{}", path.display());
        }
        for (path, bytes) in &kept {
            assert_eq!(&std::fs::read(path).unwrap(), bytes, "{}", path.display());
        }
        let left = list(dir.path(), &written).unwrap().matched;
        assert_eq!(left, KEEP);
    }

    /// A directory that can't be listed is left alone.
    #[cfg(unix)]
    #[test]
    fn an_unlistable_directory_is_left_alone() {
        use std::os::unix::fs::PermissionsExt;
        if is_root() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        seed_finished(dir.path(), "done", KEEP + 2, 1);
        let written = written_entry(dir.path());
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o300)).unwrap();

        let sweep = apply(dir.path(), &written, now(), &keep(1));

        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(sweep, Sweep::default());
        assert_eq!(list(dir.path(), &written).unwrap().matched, KEEP + 3);
    }

    /// Within the bound the sweep reads only files old enough to be stale:
    /// one old finished entry among eight fresh ones costs one read per
    /// write. Past the bound every other matched file is read.
    #[test]
    fn within_the_bound_only_old_files_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let old = seed(
            dir.path(),
            "old",
            &entry("old", "completed", "completed", 1),
            QUIET_SINCE_SECS,
        );
        seed_finished(dir.path(), "fresh", 8, 10);
        let written = written_entry(dir.path());

        for _ in 0..2 {
            let sweep = apply(dir.path(), &written, now(), &Limits::default());
            assert_eq!(sweep.matched, KEEP);
            assert_eq!(sweep.read, 1);
            assert_eq!(sweep.removed, 0);
            assert!(old.exists());
        }

        seed_finished(dir.path(), "more", 1, 50);
        let sweep = apply(dir.path(), &written, now(), &Limits::default());
        assert_eq!(sweep.matched, KEEP + 1);
        assert_eq!(sweep.read, KEEP);
        assert_eq!(sweep.removed, 1);
        assert!(!old.exists());
    }
}
