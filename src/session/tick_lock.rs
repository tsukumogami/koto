//! Opening a session's `tick.lock` ([`crate::session::TICK_LOCK_FILE`]), the
//! one way every taker opens it: a tick (shared), the import probe and a
//! whole-binding move (exclusive), and `koto init`, which creates it.
//!
//! The file is created in the session's directory when it is missing, but
//! the directory never is. A link planted at the lock's name is not followed
//! (`O_NOFOLLOW`), so no taker creates or locks a file elsewhere. The bytes
//! are never truncated: the lock is the file's identity, not its content.
//!
//! A same-host import can swap the session's directory while a taker waits
//! for the lock, which then sits on the old copy's file. [`still_current`]
//! tells a taker that holds it whether the file at the lock's path is still
//! the one it holds. [`take_exclusive`] takes the lock exclusive within a
//! wait, taking it again at its path when the file it got was replaced.

use std::fs::File;
use std::path::Path;
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

/// How often a lock that is waited for is asked for again.
#[cfg(unix)]
const POLL: Duration = Duration::from_millis(50);

/// The lock was still held elsewhere once the wait had passed.
#[derive(Debug)]
pub(crate) struct Busy {
    /// How long the taker waited before giving up.
    pub(crate) waited_for: Duration,
}

impl std::fmt::Display for Busy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "tick.lock was still held elsewhere after {} ms",
            self.waited_for.as_millis()
        )
    }
}

impl std::error::Error for Busy {}

/// Why [`take_exclusive`] didn't take the lock.
#[derive(Debug)]
pub(crate) enum TakeError {
    /// The session's directory doesn't exist; nothing was created.
    Missing,
    /// Still held elsewhere once the wait had passed.
    // `koto init --replaces` reads `waited_for` in its busy refusal; until
    // it lands, only the tests read the field.
    Busy(#[allow(dead_code)] Busy),
    /// The lock file couldn't be opened or locked.
    Failed(anyhow::Error),
    /// The platform has no `flock`.
    #[cfg(not(unix))]
    Unsupported,
}

/// Take `dir`'s `tick.lock` exclusive, asking every 50 ms until `wait` has
/// passed, and say whether it had to wait. A lock file a same-host import
/// replaced while the taker waited is taken again at its path, in what is
/// left of the same wait. A `wait` of zero asks once.
#[cfg(unix)]
pub(crate) fn take_exclusive(dir: &Path, wait: Duration) -> Result<(File, bool), TakeError> {
    use anyhow::Context as _;
    let path = dir.join(crate::session::TICK_LOCK_FILE);
    let started = Instant::now();
    let mut waited = false;
    loop {
        let Some(file) = open(dir)
            .with_context(|| format!("failed to open {}", path.display()))
            .map_err(TakeError::Failed)?
        else {
            return Err(TakeError::Missing);
        };
        match flock_within(&file, wait, started).map_err(TakeError::Failed)? {
            Some(w) => waited |= w,
            None => {
                return Err(TakeError::Busy(Busy {
                    waited_for: started.elapsed(),
                }))
            }
        }
        if still_current(&file, &path) {
            return Ok((file, waited));
        }
    }
}

#[cfg(not(unix))]
pub(crate) fn take_exclusive(_dir: &Path, _wait: Duration) -> Result<(File, bool), TakeError> {
    Err(TakeError::Unsupported)
}

/// Ask for `file`'s exclusive `flock` every 50 ms until `wait` has passed
/// since `started`: `Some(waited)` once held, where `waited` says whether
/// the first ask failed, and `None` when it was still held elsewhere then.
/// A `wait` of zero asks once.
#[cfg(unix)]
pub(crate) fn flock_within(
    file: &File,
    wait: Duration,
    started: Instant,
) -> anyhow::Result<Option<bool>> {
    use std::os::unix::io::AsRawFd;
    let mut waited = false;
    loop {
        // SAFETY: flock on a descriptor `file` owns, for the duration of the
        // call; it neither closes nor keeps the descriptor.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(Some(waited));
        }
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EWOULDBLOCK) {
            return Err(anyhow::anyhow!("failed to lock: {err}"));
        }
        waited = true;
        let elapsed = started.elapsed();
        if elapsed >= wait {
            return Ok(None);
        }
        std::thread::sleep(POLL.min(wait - elapsed));
    }
}

/// Open `dir`'s `tick.lock` for reading and writing, creating the file when
/// it is missing. `Ok(None)` when `dir` doesn't exist, with nothing created;
/// an error when the file can't be opened, a link at its name included.
#[cfg(unix)]
pub fn open(dir: &Path) -> std::io::Result<Option<File>> {
    use std::os::unix::fs::OpenOptionsExt;
    if !dir.is_dir() {
        return Ok(None);
    }
    match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join(crate::session::TICK_LOCK_FILE))
    {
        Ok(file) => Ok(Some(file)),
        // The directory went between the check and the open.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Whether the file at `path` is still `file` (same device and inode): false
/// once it was removed or replaced, or can't be read.
#[cfg(unix)]
pub fn still_current(file: &File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let held = file.metadata().ok().map(|m| (m.dev(), m.ino()));
    let now = std::fs::symlink_metadata(path)
        .ok()
        .map(|m| (m.dev(), m.ino()));
    held.is_some() && held == now
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::session::TICK_LOCK_FILE;
    use std::time::{Duration, Instant};

    #[test]
    fn open_creates_the_lock_in_an_existing_directory_and_keeps_its_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(TICK_LOCK_FILE);
        std::fs::write(&path, b"kept").unwrap();
        let file = open(dir.path()).unwrap().expect("the directory exists");
        assert!(still_current(&file, &path));
        assert_eq!(std::fs::read(&path).unwrap(), b"kept");
    }

    #[test]
    fn open_in_a_missing_directory_is_none_and_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("gone");
        assert!(open(&gone).unwrap().is_none());
        assert!(!gone.exists());
    }

    #[test]
    fn open_refuses_a_link_planted_at_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("elsewhere");
        std::os::unix::fs::symlink(&elsewhere, dir.path().join(TICK_LOCK_FILE)).unwrap();
        assert!(open(dir.path()).is_err());
        assert!(!elsewhere.exists());
    }

    #[test]
    fn a_lock_whose_file_was_replaced_is_no_longer_current() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(TICK_LOCK_FILE);
        let file = open(dir.path()).unwrap().unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"").unwrap();
        assert!(!still_current(&file, &path));
    }

    #[test]
    fn take_exclusive_returns_busy_after_the_wait() {
        use std::os::unix::io::AsRawFd;
        let dir = tempfile::tempdir().unwrap();
        let held = open(dir.path()).unwrap().unwrap();
        // SAFETY: flock on a descriptor `held` owns, for the duration of the call.
        assert_eq!(
            unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        let wait = Duration::from_millis(150);
        let started = Instant::now();
        let busy = match take_exclusive(dir.path(), wait) {
            Err(TakeError::Busy(busy)) => busy,
            other => panic!("expected Busy, got {other:?}"),
        };
        assert!(started.elapsed() >= wait);
        assert!(busy.waited_for >= wait);
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from(TICK_LOCK_FILE)]);
        drop(held);
    }

    #[test]
    fn take_exclusive_takes_a_free_lock_without_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let (file, waited) = take_exclusive(dir.path(), Duration::from_millis(150)).unwrap();
        assert!(!waited);
        assert!(still_current(&file, &dir.path().join(TICK_LOCK_FILE)));
    }
}
