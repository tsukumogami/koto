//! The count of records a session could not send, kept beside the session.
//!
//! `<session dir>/telemetry.json` holds `{"dropped": N}`: how many of the
//! session's records were dropped before a sink took them (a chunk the
//! flush never reached, a call the sink refused or koto had to kill, a
//! record too long to send). The file is only ever replaced whole, by a
//! temporary file and a rename, mode 0600, the way the `run-journal.json`
//! sidecar is written.
//!
//! Two processes can drop records of one session at once, so every change
//! is made under a lock: a separate, data-free `telemetry.lock` beside the
//! file, opened without following a symlink and never deleted. Locking the
//! data file itself would not survive the rename that replaces it. A lock
//! is waited for at most [`LOCK_WAIT`], and there is no unlocked fallback:
//! a count that can't be locked is not written. Losing a count that way is
//! acceptable; counting one twice is not.
//!
//! The wait is bounded twice over. A caller that writes several sessions'
//! counts in one go gives them one start time, so [`LOCK_WAIT`] covers them
//! all together. And once a wait has run out anywhere in the process, later
//! changes try their lock once and don't wait, until one is taken again: a
//! holder that has stalled costs the process one wait, not one per change.
//!
//! The file is read defensively. It sits in the user's home, so one that
//! isn't a regular file, is too large, or doesn't parse as an object whose
//! `dropped` is a non-negative integer reads as 0, and the next write
//! replaces it. A count above [`MAX_COUNT`] is clamped.
//!
//! Neither file is session state. Cloud sync and `koto session import`
//! carry the state log, version, template and context files only.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

/// The count's file name inside a session directory.
pub(crate) const COUNT_FILE: &str = "telemetry.json";

/// The lock's file name inside a session directory.
pub(crate) const LOCK_FILE: &str = "telemetry.lock";

/// The longest a change waits for the lock.
pub(crate) const LOCK_WAIT: Duration = Duration::from_millis(50);

/// The largest count file that is read; a real one is under 40 bytes.
const MAX_FILE_BYTES: u64 = 4096;

/// The largest count kept or reported, so a forged file can't claim an
/// absurd number of drops.
pub(crate) const MAX_COUNT: u64 = 1 << 32;

/// Whether a wait for some session's lock has run out in this process. One
/// stalled holder then costs one wait: later changes try the lock once and
/// don't wait. Cleared by the next lock that is taken.
#[cfg(not(test))]
static GAVE_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
thread_local! {
    static GAVE_UP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(not(test))]
fn gave_up() -> bool {
    GAVE_UP.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(not(test))]
fn set_gave_up(value: bool) {
    GAVE_UP.store(value, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(test)]
fn gave_up() -> bool {
    GAVE_UP.with(|g| g.get())
}

#[cfg(test)]
fn set_gave_up(value: bool) {
    GAVE_UP.with(|g| g.set(value));
}

/// The held lock of one session's count. Dropping it releases the lock;
/// the lock file stays.
struct Lock {
    _file: std::fs::File,
}

impl Lock {
    /// Take `dir`'s lock, waiting until [`LOCK_WAIT`] has passed since
    /// `started` (not at all once a wait has run out in this process).
    /// `None` when the directory is gone, the lock file can't be opened, or
    /// the wait ran out.
    ///
    /// A caller that changes several sessions' counts in one go passes the
    /// same `started` for each, so [`LOCK_WAIT`] bounds them all together
    /// and not one by one. The lock is always tried once.
    fn acquire(dir: &Path, started: Instant) -> Option<Lock> {
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::io::AsRawFd;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(dir.join(LOCK_FILE))
            .ok()?;
        let wait = if gave_up() { Duration::ZERO } else { LOCK_WAIT };
        match crate::engine::jsonl_append::lock_exclusive(file.as_raw_fd(), Some(wait), started) {
            Ok(()) => {
                set_gave_up(false);
                Some(Lock { _file: file })
            }
            Err(_) => {
                set_gave_up(true);
                None
            }
        }
    }
}

fn count_path(dir: &Path) -> PathBuf {
    dir.join(COUNT_FILE)
}

/// The count `dir` holds, 0 for anything that isn't a well-formed count
/// file. Never follows a symlink and never reads a large file.
fn read(dir: &Path) -> u64 {
    let path = count_path(dir);
    let Ok(meta) = std::fs::symlink_metadata(&path) else {
        return 0;
    };
    if !meta.file_type().is_file() || meta.len() > MAX_FILE_BYTES {
        return 0;
    }
    let Ok(bytes) = std::fs::read(&path) else {
        return 0;
    };
    serde_json::from_slice::<Value>(&bytes)
        .ok()
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|o| o.get("dropped"))
        .and_then(Value::as_u64)
        .map_or(0, |n| n.min(MAX_COUNT))
}

/// Replace `dir`'s count with `count`, by temporary file and rename.
fn write(dir: &Path, count: u64) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(&serde_json::json!({ "dropped": count.min(MAX_COUNT) }))
        .map_err(std::io::Error::other)?;
    // tempfile creates the file 0600 on unix.
    let mut tmp = tempfile::Builder::new()
        .prefix(".telemetry-")
        .suffix(".tmp")
        .tempfile_in(dir)?;
    tmp.write_all(&bytes)?;
    tmp.persist(count_path(dir)).map_err(|e| e.error)?;
    Ok(())
}

/// Add `n` dropped records to the count of the session in `dir`, waiting
/// for its lock only until [`LOCK_WAIT`] has passed since `started`.
///
/// A flush that counts drops for several sessions passes one `started` for
/// all of them, so the waits can't add up past the one [`LOCK_WAIT`] the
/// flush's time bound allows.
///
/// Best-effort and silent: a session directory that is gone, a lock that
/// can't be taken in time, or a write that fails loses the `n`, which is
/// the failure this count tolerates. Returns whether the count was written.
pub(crate) fn add_within(dir: &Path, n: u64, started: Instant) -> bool {
    if n == 0 {
        return true;
    }
    let Some(_lock) = Lock::acquire(dir, started) else {
        return false;
    };
    let total = read(dir).saturating_add(n).min(MAX_COUNT);
    write(dir, total).is_ok()
}

/// [`add_within`] with a wait of its own.
#[cfg(test)]
pub(crate) fn add(dir: &Path, n: u64) -> bool {
    add_within(dir, n, Instant::now())
}

/// The count of the session in `dir`, read without the lock: for a test or
/// a report, never as the first half of a change.
#[cfg(test)]
pub(crate) fn peek(dir: &Path) -> u64 {
    read(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn raw(dir: &Path) -> String {
        std::fs::read_to_string(count_path(dir)).unwrap()
    }

    #[test]
    fn add_creates_the_count_and_adds_to_it() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(read(tmp.path()), 0);
        assert!(add(tmp.path(), 3));
        assert_eq!(raw(tmp.path()), r#"{"dropped":3}"#);
        assert!(add(tmp.path(), 250));
        assert_eq!(read(tmp.path()), 253);
        // Adding nothing writes nothing.
        let before = std::fs::metadata(count_path(tmp.path()))
            .unwrap()
            .modified()
            .unwrap();
        assert!(add(tmp.path(), 0));
        assert_eq!(
            std::fs::metadata(count_path(tmp.path()))
                .unwrap()
                .modified()
                .unwrap(),
            before
        );
    }

    #[test]
    fn both_files_are_owner_only_and_no_tempfile_is_left() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(add(tmp.path(), 1));
        assert!(add(tmp.path(), 1));
        let mut names: Vec<String> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, vec![COUNT_FILE.to_string(), LOCK_FILE.to_string()]);
        for name in names {
            let mode = std::fs::metadata(tmp.path().join(&name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "{name}");
        }
        // The lock file holds no data.
        assert_eq!(
            std::fs::metadata(tmp.path().join(LOCK_FILE)).unwrap().len(),
            0
        );
    }

    #[test]
    fn a_malformed_oversized_or_non_regular_count_reads_as_zero_and_is_healed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = count_path(tmp.path());
        for bad in [
            "not json",
            "[]",
            "7",
            r#"{"dropped":-1}"#,
            r#"{"dropped":"9"}"#,
            r#"{"dropped":1.5}"#,
            r#"{"other":4}"#,
        ] {
            std::fs::write(&path, bad).unwrap();
            assert_eq!(read(tmp.path()), 0, "{bad}");
        }
        std::fs::write(
            &path,
            format!("{{\"dropped\":5,\"pad\":\"{}\"}}", "x".repeat(5000)),
        )
        .unwrap();
        assert_eq!(read(tmp.path()), 0, "oversized");
        // The next change replaces whatever was there.
        assert!(add(tmp.path(), 2));
        assert_eq!(raw(tmp.path()), r#"{"dropped":2}"#);

        // A directory, or a symlink to a count elsewhere, is not read.
        std::fs::remove_file(&path).unwrap();
        let elsewhere = tmp.path().join("elsewhere.json");
        std::fs::write(&elsewhere, r#"{"dropped":99}"#).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
        assert_eq!(read(tmp.path()), 0);
        // The rename replaces the link itself; its target is untouched.
        assert!(add(tmp.path(), 1));
        assert_eq!(raw(tmp.path()), r#"{"dropped":1}"#);
        assert!(!std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_to_string(&elsewhere).unwrap(),
            r#"{"dropped":99}"#
        );
    }

    #[test]
    fn a_forged_count_is_clamped_and_adds_saturate() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            count_path(tmp.path()),
            r#"{"dropped":18446744073709551615}"#,
        )
        .unwrap();
        assert_eq!(read(tmp.path()), MAX_COUNT);
        assert!(add(tmp.path(), 10));
        assert_eq!(read(tmp.path()), MAX_COUNT);
    }

    #[test]
    fn a_missing_directory_or_a_symlinked_lock_is_not_written() {
        let tmp = tempfile::tempdir().unwrap();
        let gone = tmp.path().join("gone");
        assert!(!add(&gone, 1));
        assert!(!gone.exists());

        let dir = tmp.path().join("session");
        std::fs::create_dir(&dir).unwrap();
        let target = tmp.path().join("target.lock");
        std::fs::write(&target, "").unwrap();
        std::os::unix::fs::symlink(&target, dir.join(LOCK_FILE)).unwrap();
        assert!(!add(&dir, 1), "a lock that is a symlink was followed");
        assert!(!count_path(&dir).exists());
    }

    /// Take the lock of the session in `dir`, as another process would.
    fn hold(dir: &Path) -> std::fs::File {
        use std::os::unix::io::AsRawFd;
        assert!(add(dir, 1));
        let holder = std::fs::File::open(dir.join(LOCK_FILE)).unwrap();
        // Blocking, for the reason given in the test below.
        // SAFETY: the descriptor belongs to `holder`, which outlives the call.
        assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX) }, 0);
        holder
    }

    #[test]
    fn several_sessions_in_one_go_share_one_wait() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs: Vec<PathBuf> = (0..6)
            .map(|i| {
                let dir = tmp.path().join(format!("s{i}"));
                std::fs::create_dir(&dir).unwrap();
                dir
            })
            .collect();
        // Every other session's lock is held, so a wait that restarted for
        // each session would cost three waits: a timeout, then a success
        // that lets the next one wait again.
        let _held: Vec<std::fs::File> = dirs.iter().step_by(2).map(|d| hold(d)).collect();
        let started = Instant::now();
        let written: Vec<bool> = dirs.iter().map(|d| add_within(d, 4, started)).collect();
        let took = started.elapsed();
        assert_eq!(written, vec![false, true, false, true, false, true]);
        assert!(took >= LOCK_WAIT, "{took:?}");
        assert!(took < LOCK_WAIT * 2, "the waits added up: {took:?}");
        // The held sessions kept the 1 `hold` wrote; the free ones gained 4.
        let counts: Vec<u64> = dirs.iter().map(|d| read(d)).collect();
        assert_eq!(counts, vec![1, 4, 1, 4, 1, 4]);
    }

    #[test]
    fn a_held_lock_costs_one_bounded_wait_and_loses_the_count() {
        use std::os::unix::io::AsRawFd;
        let tmp = tempfile::tempdir().unwrap();
        assert!(add(tmp.path(), 1));
        let holder = std::fs::File::open(tmp.path().join(LOCK_FILE)).unwrap();
        // A blocking lock: a child process another test thread is starting
        // can hold a copy of the descriptor `add` just closed until its
        // exec, and with it the lock, for a moment.
        // SAFETY: the descriptor belongs to `holder`, which outlives the call.
        assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX) }, 0);
        let started = Instant::now();
        assert!(!add(tmp.path(), 5));
        let first = started.elapsed();
        assert!(first >= LOCK_WAIT, "{first:?}");
        assert!(first < Duration::from_secs(2), "{first:?}");
        // The process has given up once, so the next change doesn't wait.
        let started = Instant::now();
        assert!(!add(tmp.path(), 5));
        assert!(started.elapsed() < LOCK_WAIT, "{:?}", started.elapsed());
        assert_eq!(read(tmp.path()), 1);
        // Once the holder lets go, a change goes through and waits again
        // are allowed.
        drop(holder);
        assert!(add(tmp.path(), 5));
        assert_eq!(read(tmp.path()), 6);
        assert!(!gave_up());
    }
}
