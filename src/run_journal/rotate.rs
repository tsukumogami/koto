//! Rotation and retention for the run journal.
//!
//! Rotation is off unless `run_journal.rotate_bytes` is set (user config
//! only). With a bound set, the append that finds the journal at or over it
//! moves the file to the next `_run_journal.NNNNNNNNNN.jsonl` name, under the
//! journal's own lock, and the record goes to a fresh `_run_journal.jsonl`.
//! The move is a hard link to the new name (which fails rather than replace
//! anything) followed by an unlink of the old one, so the file keeps its
//! inode: a reader positioned in it finds it again by `(dev, inode)`. A
//! writer that opened the file before the move notices after taking the lock
//! and opens the path again (see
//! [`crate::engine::jsonl_append::append_bounded_lines_current`]), so nothing
//! is written to a file after its rename.
//!
//! Retention runs right after a rotation, under the same lock. It reads file
//! names, `stat` metadata and the position files readers keep in
//! `_run_journal.readers/`, never a journal record. A rotated file is removed
//! when it is older than `run_journal.retain_days` and no recorded position
//! still needs it. `docs/reference/run-journal.md` states the rules readers
//! follow.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::JOURNAL_FILE;
use crate::engine::jsonl_append::{append_bounded_lines_current, UnderLock};

/// The directory beside the journal where readers record their positions.
pub(crate) const READERS_DIR: &str = "_run_journal.readers";

/// The largest position file koto reads.
const MAX_POSITION_BYTES: u64 = 4096;

/// The three run-journal settings, resolved from user config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Settings {
    /// 0 turns rotation off.
    pub rotate_bytes: u64,
    pub retain_days: u64,
    pub reader_stale_days: u64,
}

impl Default for Settings {
    fn default() -> Self {
        let c = crate::config::RunJournalConfig::default();
        Settings {
            rotate_bytes: c.rotate_bytes,
            retain_days: c.retain_days,
            reader_stale_days: c.reader_stale_days,
        }
    }
}

/// The settings, read from config once per process. A config that can't be
/// read leaves the defaults, so rotation stays off.
#[cfg(not(test))]
fn settings() -> Settings {
    static SETTINGS: std::sync::OnceLock<Settings> = std::sync::OnceLock::new();
    *SETTINGS.get_or_init(|| match crate::config::resolve::load_config() {
        Ok(c) => Settings {
            rotate_bytes: c.run_journal.rotate_bytes,
            retain_days: c.run_journal.retain_days,
            reader_stale_days: c.run_journal.reader_stale_days,
        },
        Err(_) => Settings::default(),
    })
}

/// Print a rotation warning on stderr.
#[cfg(not(test))]
fn warn(message: &str) {
    eprintln!("warning: run journal: {}", message);
}

/// Whether a failed move already switched rotation off for this process.
#[cfg(not(test))]
fn rotation_disabled() -> bool {
    ROTATION_DISABLED.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(not(test))]
fn disable_rotation() {
    ROTATION_DISABLED.store(true, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(not(test))]
static ROTATION_DISABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

// In unit tests the settings, the warnings and the disabled flag are per
// thread, so each test sees only its own.
#[cfg(test)]
thread_local! {
    static TEST_SETTINGS: std::cell::Cell<Settings> = const {
        std::cell::Cell::new(Settings {
            rotate_bytes: 0,
            retain_days: 30,
            reader_stale_days: 90,
        })
    };
    static TEST_WARNINGS: std::cell::RefCell<Vec<String>> =
        const { std::cell::RefCell::new(Vec::new()) };
    static TEST_DISABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn settings() -> Settings {
    TEST_SETTINGS.with(|s| s.get())
}

#[cfg(test)]
fn warn(message: &str) {
    TEST_WARNINGS.with(|w| {
        w.borrow_mut()
            .push(format!("warning: run journal: {}", message))
    });
}

#[cfg(test)]
fn rotation_disabled() -> bool {
    TEST_DISABLED.with(|d| d.get())
}

#[cfg(test)]
fn disable_rotation() {
    TEST_DISABLED.with(|d| d.set(true));
}

/// Set this thread's settings, clearing its warnings and the disabled flag.
#[cfg(test)]
pub(crate) fn set_settings_for_test(s: Settings) {
    TEST_SETTINGS.with(|c| c.set(s));
    TEST_WARNINGS.with(|w| w.borrow_mut().clear());
    TEST_DISABLED.with(|d| d.set(false));
}

/// The rotation warnings this thread has printed since the last
/// `set_settings_for_test`.
#[cfg(test)]
pub(crate) fn warnings_for_test() -> Vec<String> {
    TEST_WARNINGS.with(|w| w.borrow().clone())
}

/// Append `lines` to the journal under `root` under one lock, rotating
/// first when the journal is at or over the configured bound, and flush them
/// to disk when `flush` is set. The size is checked once, under the lock,
/// before anything is written, so a batch is never split across two files.
/// Returns whether it flushed.
pub(crate) fn append(root: &Path, lines: &[&str], max: usize, flush: bool) -> anyhow::Result<bool> {
    let path = root.join(JOURNAL_FILE);
    let s = settings();
    append_bounded_lines_current(root, &path, lines, max, flush, &mut |file, size| {
        if s.rotate_bytes == 0 || size < s.rotate_bytes || rotation_disabled() {
            return UnderLock::Write;
        }
        match rotate_locked(root, &path, file, size) {
            Ok(()) => {
                retain(root, &s);
                UnderLock::Reopen
            }
            Err(e) => {
                warn(&format!(
                    "rotation of {} failed, appending without rotating: {}",
                    path.display(),
                    e
                ));
                disable_rotation();
                UnderLock::Write
            }
        }
    })
}

/// Move the locked journal to the next rotated name. On any failure the
/// journal is left where it was, under its own name.
fn rotate_locked(root: &Path, path: &Path, file: &std::fs::File, size: u64) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::FileExt;

    // A partial last line (a crash mid-write) gets its newline here, so no
    // rotated file ends mid-line and a reader of the next file never glues
    // two lines together.
    let mut last = [0u8; 1];
    if size > 0 && file.read_at(&mut last, size - 1).is_ok() && last[0] != b'\n' {
        let mut f = file;
        f.write_all(b"\n")?;
    }
    // Retention ages a rotated file by its modification time. Stamp it now so
    // the age counts from the rotation, not from the last append: a journal
    // left idle past the retention bound must not have its rotated file
    // removed the moment it rotates.
    file.set_modified(SystemTime::now())?;
    // Records written without a flush (check records) count on the next
    // flush of the journal to make them durable. That flush goes to the new
    // file, so flush this one before it stops being written.
    file.sync_data()?;

    let next = root.join(rotated_name(next_sequence(root)?));
    std::fs::hard_link(path, &next)?;
    if let Err(e) = std::fs::remove_file(path) {
        // Leave the file with one name, its old one.
        let _ = std::fs::remove_file(&next);
        return Err(e.into());
    }
    ensure_readers_dir(root);
    Ok(())
}

/// The name of rotated file number `seq`.
pub(crate) fn rotated_name(seq: u64) -> String {
    format!("_run_journal.{:010}.jsonl", seq)
}

/// The sequence number a file name carries, when it is a rotated journal.
pub(crate) fn rotated_sequence(name: &str) -> Option<u64> {
    let digits = name.strip_prefix("_run_journal.")?.strip_suffix(".jsonl")?;
    if digits.len() == 10 && digits.bytes().all(|b| b.is_ascii_digit()) {
        digits.parse().ok()
    } else {
        None
    }
}

/// One more than the highest rotated file's number, or 1 when there is none.
fn next_sequence(root: &Path) -> anyhow::Result<u64> {
    let mut highest = 0;
    for entry in std::fs::read_dir(root)?.flatten() {
        if let Some(seq) = entry.file_name().to_str().and_then(rotated_sequence) {
            highest = highest.max(seq);
        }
    }
    Ok(highest + 1)
}

/// The rotated files under `root`, oldest first.
fn rotated_files(root: &Path) -> Vec<(PathBuf, std::fs::Metadata)> {
    let mut files: Vec<(u64, PathBuf, std::fs::Metadata)> = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let Some(seq) = entry.file_name().to_str().and_then(rotated_sequence) else {
            continue;
        };
        match std::fs::symlink_metadata(entry.path()) {
            Ok(meta) if meta.file_type().is_file() => files.push((seq, entry.path(), meta)),
            _ => {}
        }
    }
    files.sort_by_key(|(seq, _, _)| *seq);
    files.into_iter().map(|(_, p, m)| (p, m)).collect()
}

fn ensure_readers_dir(root: &Path) {
    use std::os::unix::fs::DirBuilderExt;
    let dir = root.join(READERS_DIR);
    if std::fs::symlink_metadata(&dir).is_err() {
        let _ = std::fs::DirBuilder::new().mode(0o700).create(&dir);
    }
}

/// A position a reader recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Position {
    dev: u64,
    ino: u64,
    offset: u64,
}

/// Every position in the readers directory that is fresh and parses. Each
/// one that isn't is named in a warning and protects nothing.
fn positions(root: &Path, s: &Settings) -> Vec<Position> {
    let dir = root.join(READERS_DIR);
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if meta.file_type().is_dir() => {}
        _ => return Vec::new(),
    }
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let stale_after = Duration::from_secs(s.reader_stale_days.saturating_mul(86_400));
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(id) = name.strip_suffix(".json") else {
            continue;
        };
        if !valid_reader_id(id) {
            continue;
        }
        let path = entry.path();
        match read_position(&path, stale_after) {
            Ok(p) => out.push(p),
            Err(why) => warn(&format!(
                "reader position {} protects nothing: {}",
                path.display(),
                why
            )),
        }
    }
    out
}

fn valid_reader_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn read_position(path: &Path, stale_after: Duration) -> Result<Position, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.file_type().is_file() {
        return Err("not a regular file".to_string());
    }
    if meta.len() > MAX_POSITION_BYTES {
        return Err(format!("larger than {} bytes", MAX_POSITION_BYTES));
    }
    let modified = meta.modified().map_err(|e| e.to_string())?;
    if SystemTime::now()
        .duration_since(modified)
        .is_ok_and(|age| age > stale_after)
    {
        return Err("not updated within the stale bound".to_string());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("not JSON: {}", e))?;
    let field = |k: &str| value.get(k).and_then(serde_json::Value::as_u64);
    if field("v") != Some(1) {
        return Err("\"v\" is not 1".to_string());
    }
    match (field("dev"), field("inode"), field("offset")) {
        (Some(dev), Some(ino), Some(offset)) => Ok(Position { dev, ino, offset }),
        _ => Err("\"dev\", \"inode\" and \"offset\" must be non-negative integers".to_string()),
    }
}

/// Remove each rotated file that is older than the retention bound and that
/// no recorded position needs. A position needs the file it names and every
/// newer one, except that a position at or past a rotated file's end needs
/// only the newer ones.
fn retain(root: &Path, s: &Settings) {
    let files = rotated_files(root);
    if files.is_empty() {
        return;
    }
    // Index of the oldest rotated file some position still needs.
    let mut needed_from = files.len();
    for p in positions(root, s) {
        match files
            .iter()
            .position(|(_, m)| m.dev() == p.dev && m.ino() == p.ino)
        {
            Some(i) if p.offset >= files[i].1.len() => needed_from = needed_from.min(i + 1),
            Some(i) => needed_from = needed_from.min(i),
            None => {
                let active = std::fs::symlink_metadata(root.join(JOURNAL_FILE))
                    .is_ok_and(|m| m.dev() == p.dev && m.ino() == p.ino);
                if !active {
                    warn(&format!(
                        "a reader position (device {}, inode {}) names no journal file",
                        p.dev, p.ino
                    ));
                }
            }
        }
    }
    let keep_for = Duration::from_secs(s.retain_days.saturating_mul(86_400));
    // The newest rotated file is never removed: the next number is one past
    // the highest that exists, so keeping it is what keeps numbering from
    // restarting, and with it the rule that name order is rotation order.
    let needed_from = needed_from.min(files.len() - 1);
    for (i, (path, meta)) in files.iter().enumerate() {
        if i >= needed_from {
            break;
        }
        let old_enough = meta
            .modified()
            .ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .is_some_and(|age| age >= keep_for);
        if old_enough {
            if let Err(e) = std::fs::remove_file(path) {
                warn(&format!("could not remove {}: {}", path.display(), e));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const MAX: usize = 4096;

    fn on(bytes: u64) -> Settings {
        Settings {
            rotate_bytes: bytes,
            retain_days: 30,
            reader_stale_days: 90,
        }
    }

    fn journal(root: &Path) -> String {
        std::fs::read_to_string(root.join(JOURNAL_FILE)).unwrap_or_default()
    }

    fn rotated(root: &Path) -> Vec<String> {
        rotated_files(root)
            .into_iter()
            .map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    fn set_mtime_days_ago(path: &Path, days: u64) {
        let when = SystemTime::now() - Duration::from_secs(days * 86_400);
        let f = std::fs::File::options().write(true).open(path).unwrap();
        f.set_modified(when).unwrap();
    }

    #[test]
    fn rotated_names_order_by_sequence() {
        assert_eq!(rotated_name(1), "_run_journal.0000000001.jsonl");
        assert_eq!(rotated_sequence("_run_journal.0000000010.jsonl"), Some(10));
        assert_eq!(rotated_sequence("_run_journal.jsonl"), None);
        assert_eq!(rotated_sequence("_run_journal.123.jsonl"), None);
        assert!(rotated_name(9) < rotated_name(10));
    }

    #[test]
    fn rotation_off_never_moves_the_journal() {
        let tmp = TempDir::new().unwrap();
        set_settings_for_test(on(0));
        for _ in 0..50 {
            append(tmp.path(), &["{\"kind\":\"x\"}"], MAX, true).unwrap();
        }
        assert!(rotated(tmp.path()).is_empty());
        assert_eq!(journal(tmp.path()).lines().count(), 50);
    }

    #[test]
    fn one_byte_under_the_bound_does_not_rotate_and_at_it_does() {
        let line = "{\"kind\":\"x\"}"; // 13 bytes with its newline

        // 26 bytes against a bound of 27: one byte under, so the next
        // append writes to the same file.
        let under = TempDir::new().unwrap();
        set_settings_for_test(on(27));
        append(under.path(), &[line], MAX, true).unwrap();
        append(under.path(), &[line], MAX, true).unwrap();
        append(under.path(), &[line], MAX, true).unwrap();
        assert!(rotated(under.path()).is_empty());
        assert_eq!(journal(under.path()).len(), 39);

        // 26 bytes against a bound of 26: exactly at it, so the next append
        // rotates first.
        let at = TempDir::new().unwrap();
        set_settings_for_test(on(26));
        append(at.path(), &[line], MAX, true).unwrap();
        append(at.path(), &[line], MAX, true).unwrap();
        assert!(rotated(at.path()).is_empty());
        append(at.path(), &[line], MAX, true).unwrap();
        assert_eq!(rotated(at.path()), vec![rotated_name(1)]);
        let first = std::fs::read_to_string(at.path().join(rotated_name(1))).unwrap();
        assert_eq!(first.len(), 26);
        assert_eq!(journal(at.path()), format!("{line}\n"));
        assert!(at.path().join(READERS_DIR).is_dir());
    }

    #[test]
    fn a_batch_is_never_split_across_files() {
        let tmp = TempDir::new().unwrap();
        set_settings_for_test(on(5));
        append(tmp.path(), &["{\"n\":1}"], MAX, true).unwrap();
        // The journal is already over the bound: the batch goes, whole, to
        // the fresh file.
        append(
            tmp.path(),
            &["{\"n\":2}", "{\"n\":3}", "{\"n\":4}"],
            MAX,
            true,
        )
        .unwrap();
        let first = std::fs::read_to_string(tmp.path().join(rotated_name(1))).unwrap();
        assert_eq!(first, "{\"n\":1}\n");
        assert_eq!(journal(tmp.path()), "{\"n\":2}\n{\"n\":3}\n{\"n\":4}\n");
    }

    #[test]
    fn an_unflushed_batch_rotates_like_a_flushed_one_and_no_lines_opens_nothing() {
        let tmp = TempDir::new().unwrap();
        set_settings_for_test(on(5));
        assert!(!append(tmp.path(), &["{\"n\":1}", "{\"n\":2}"], MAX, false).unwrap());
        assert!(append(tmp.path(), &["{\"n\":3}"], MAX, true).unwrap());
        let first = std::fs::read_to_string(tmp.path().join(rotated_name(1))).unwrap();
        assert_eq!(first, "{\"n\":1}\n{\"n\":2}\n");
        assert_eq!(journal(tmp.path()), "{\"n\":3}\n");

        let empty = TempDir::new().unwrap();
        let root = empty.path().join("never-made");
        assert!(!append(&root, &[], MAX, true).unwrap());
        assert!(!root.exists());
    }

    #[test]
    fn a_partial_last_line_is_terminated_before_rotating() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(JOURNAL_FILE), "{\"n\":1}\n{\"tor").unwrap();
        set_settings_for_test(on(5));
        append(tmp.path(), &["{\"n\":2}"], MAX, true).unwrap();
        let first = std::fs::read_to_string(tmp.path().join(rotated_name(1))).unwrap();
        assert!(first.ends_with('\n'), "{first:?}");
        assert_eq!(journal(tmp.path()), "{\"n\":2}\n");
    }

    #[test]
    fn non_json_lines_rotate_like_any_other() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(JOURNAL_FILE), "not json at all\n").unwrap();
        set_settings_for_test(on(5));
        append(tmp.path(), &["also not json"], MAX, true).unwrap();
        assert_eq!(rotated(tmp.path()), vec![rotated_name(1)]);
    }

    #[test]
    fn a_failed_move_appends_to_the_unrotated_file_with_one_warning() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return; // root ignores the directory's mode
        }
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("koto");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(JOURNAL_FILE), "{\"n\":1}\n").unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o500)).unwrap();
        set_settings_for_test(on(5));
        let first = append(&root, &["{\"n\":2}"], MAX, true);
        let second = append(&root, &["{\"n\":3}"], MAX, true);
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        first.unwrap();
        second.unwrap();
        assert_eq!(journal(&root), "{\"n\":1}\n{\"n\":2}\n{\"n\":3}\n");
        assert!(rotated(&root).is_empty());
        assert_eq!(warnings_for_test().len(), 1, "{:?}", warnings_for_test());
    }

    #[test]
    fn retention_removes_old_files_no_position_needs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        set_settings_for_test(on(5));
        for n in 0..3 {
            append(root, &[&format!("{{\"n\":{n}}}")], MAX, true).unwrap();
        }
        // Two rotations so far; age the oldest past the bound.
        assert_eq!(rotated(root), vec![rotated_name(1), rotated_name(2)]);
        set_mtime_days_ago(&root.join(rotated_name(1)), 31);
        set_mtime_days_ago(&root.join(rotated_name(2)), 29);
        append(root, &["{\"n\":3}"], MAX, true).unwrap();
        // File 1 is gone; file 2 is inside the bound; file 3 is new.
        assert_eq!(rotated(root), vec![rotated_name(2), rotated_name(3)]);
    }

    #[test]
    fn the_newest_rotated_file_is_kept_so_numbering_never_restarts() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        set_settings_for_test(Settings {
            rotate_bytes: 5,
            retain_days: 0,
            reader_stale_days: 90,
        });
        for n in 0..6 {
            append(root, &[&format!("{{\"n\":{n}}}")], MAX, true).unwrap();
            if n > 0 {
                // With a zero-day bound only the newest survives each pass.
                assert_eq!(rotated(root), vec![rotated_name(n)]);
            }
        }
    }

    #[test]
    fn an_idle_journal_keeps_its_rotated_file_when_it_finally_rotates() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        set_settings_for_test(on(5));
        append(root, &["{\"n\":0}"], MAX, true).unwrap();
        append(root, &["{\"n\":1}"], MAX, true).unwrap();
        // File 1 exists; the active journal then sits idle for 40 days.
        set_mtime_days_ago(&root.join(JOURNAL_FILE), 40);
        set_mtime_days_ago(&root.join(rotated_name(1)), 41);
        append(root, &["{\"n\":2}"], MAX, true).unwrap();
        // File 1 is old and goes; file 2 was rotated just now, so it counts
        // as new whatever its last append's age.
        assert_eq!(rotated(root), vec![rotated_name(2)]);
        let age = SystemTime::now()
            .duration_since(
                std::fs::metadata(root.join(rotated_name(2)))
                    .unwrap()
                    .modified()
                    .unwrap(),
            )
            .unwrap();
        assert!(age < Duration::from_secs(60), "{age:?}");
    }

    fn write_position(root: &Path, id: &str, file: &Path, offset: u64) {
        let meta = std::fs::metadata(file).unwrap();
        let dir = root.join(READERS_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{id}.json")),
            format!(
                "{{\"v\":1,\"dev\":{},\"inode\":{},\"offset\":{offset}}}",
                meta.dev(),
                meta.ino()
            ),
        )
        .unwrap();
    }

    #[test]
    fn a_position_keeps_its_file_and_newer_ones_past_the_bound() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        set_settings_for_test(on(5));
        for n in 0..4 {
            append(root, &[&format!("{{\"n\":{n}}}")], MAX, true).unwrap();
        }
        // Rotated 1, 2, 3; a lagging reader mid-way through file 2, a
        // reader at the end of file 2 (which needs only 3 on).
        write_position(root, "lagging", &root.join(rotated_name(2)), 2);
        write_position(root, "caught-up", &root.join(rotated_name(3)), 0);
        for n in 1..=3 {
            set_mtime_days_ago(&root.join(rotated_name(n)), 40);
        }
        append(root, &["{\"n\":4}"], MAX, true).unwrap();
        assert_eq!(
            rotated(root),
            vec![rotated_name(2), rotated_name(3), rotated_name(4)]
        );

        // The lagging reader finishes file 2.
        let len = std::fs::metadata(root.join(rotated_name(2))).unwrap().len();
        write_position(root, "lagging", &root.join(rotated_name(2)), len);
        set_mtime_days_ago(&root.join(rotated_name(4)), 40);
        append(root, &["{\"n\":5}"], MAX, true).unwrap();
        // File 2 goes; 3 and everything newer stay, 4 included despite its
        // age, because the readers still need file 3 onward.
        assert_eq!(
            rotated(root),
            vec![rotated_name(3), rotated_name(4), rotated_name(5)]
        );
    }

    #[test]
    fn stale_or_broken_positions_protect_nothing_and_are_named() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        set_settings_for_test(on(5));
        for n in 0..2 {
            append(root, &[&format!("{{\"n\":{n}}}")], MAX, true).unwrap();
        }
        write_position(root, "stale", &root.join(rotated_name(1)), 0);
        set_mtime_days_ago(&root.join(READERS_DIR).join("stale.json"), 91);
        std::fs::write(root.join(READERS_DIR).join("broken.json"), "{not json").unwrap();
        set_mtime_days_ago(&root.join(rotated_name(1)), 40);
        append(root, &["{\"n\":2}"], MAX, true).unwrap();
        assert!(!root.join(rotated_name(1)).exists());
        let warnings = warnings_for_test().join("\n");
        assert!(warnings.contains("stale.json"), "{warnings}");
        assert!(warnings.contains("broken.json"), "{warnings}");
    }
}
