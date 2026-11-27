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
//! the one it holds.

#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::path::Path;

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
}
