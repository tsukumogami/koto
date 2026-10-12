//! One bounded, atomic JSONL append.
//!
//! For a non-durable signal file see [`crate::engine::wake_signal::ring`].
//!
//! [`append_bounded_line`] is the append discipline the workspace-wide
//! JSONL files share: the terminal index (`_terminal_index.jsonl`) and the
//! decider ledger (`_decider_ledger.jsonl`). The run journal
//! (`_run_journal.jsonl`) uses [`append_bounded_lines_no_follow`], the same
//! discipline with symlinks refused, under a lock that lets it recover from
//! a partial last line, for several lines at a time and with the flush to
//! disk left to the caller. On unix it goes through
//! [`append_bounded_lines_current`], which adds what a file that can be
//! rotated away needs.
//!
//! ## Atomicity
//!
//! The file is opened with `OpenOptions::append(true)` (POSIX `O_APPEND`)
//! and the line goes out in one `write_all`, followed by `sync_data`. Under
//! POSIX, writes of at most `PIPE_BUF` bytes (4 KiB on Linux) to a
//! descriptor opened with `O_APPEND` are atomic with respect to each other:
//! the kernel resolves the offset and performs the write as one syscall. A
//! caller passes a `max` within that bound, and a line over it is refused
//! before the file is opened, so concurrent appenders never interleave.
//! Nothing here seeks or writes at an offset; don't add either.
//!
//! The discipline holds on local ext4, xfs, APFS, and NTFS. Network
//! filesystems are out of scope.
//!
//! ## Permissions
//!
//! A file this function creates is mode 0600 on unix. A file that already
//! exists keeps whatever mode it has.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

/// Append `line` plus a trailing newline to `path` as one atomic write.
///
/// `line` must not contain the newline; this function adds it. The whole
/// write (newline included) must fit in `max` bytes, or the call fails
/// before `path` is opened. `dir` is created (with its parents) when
/// absent; it is normally `path`'s parent.
pub fn append_bounded_line(dir: &Path, path: &Path, line: &str, max: usize) -> Result<()> {
    let len = line.len() + 1;
    if len > max {
        anyhow::bail!(
            "line for {} exceeds the PIPE_BUF append bound ({} bytes > {})",
            path.display(),
            len,
            max
        );
    }

    if !dir.exists() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
    }

    let mut buf = String::with_capacity(len);
    buf.push_str(line);
    buf.push('\n');

    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Applies only when the file is created; an existing file's mode is
        // left alone.
        options.mode(0o600);
    }
    // O_APPEND: the kernel picks the offset per write. Never seek() or
    // write_at() on this handle.
    let mut file = options
        .open(path)
        .with_context(|| format!("failed to open {} for append", path.display()))?;
    file.write_all(buf.as_bytes())
        .with_context(|| format!("failed to append to {}", path.display()))?;
    file.sync_data()
        .with_context(|| format!("failed to fsync {}", path.display()))?;
    Ok(())
}

/// [`append_bounded_line`] for a file that must not be reached through a
/// symlink and may hold a partial last line.
///
/// The one-line, always-flushed form of [`append_bounded_lines_no_follow`],
/// which the run journal (`_run_journal.jsonl`) writes through. On unix the
/// file is opened with `O_NOFOLLOW`, so a symlink planted at `path` makes
/// the open fail instead of appending to whatever the link points at. The
/// append holds an exclusive advisory lock (`flock`), and under it a file
/// that doesn't end in a newline (a write cut short by a crash or a full
/// disk) gets one before the line, so the line starts on its own and only
/// the partial line is lost. The lock is what makes that check safe:
/// without it, a line another writer is still appending would look partial.
/// The bound, the `sync_data` and mode 0600 on create are the same as
/// [`append_bounded_line`].
pub fn append_bounded_line_no_follow(
    dir: &Path,
    path: &Path,
    line: &str,
    max: usize,
) -> Result<()> {
    append_bounded_lines_no_follow(dir, path, &[line], max, true).map(|_| ())
}

/// [`append_bounded_line_no_follow`] for several lines at once, flushed to
/// disk only when `flush` is set.
///
/// The lines go out in order under one lock, each in its own `write` (so
/// each is within `max` and atomic on its own), after the same partial-line
/// repair. Every line is checked against `max` before `path` is opened, and
/// one over it refuses the whole call. With `flush`, one `sync_data` follows
/// the last line; without it the lines reach the kernel and are made
/// durable by the next flush of the file, or by the kernel's own writeback.
/// A process that dies after the call returns loses nothing either way.
///
/// Returns whether it flushed. No lines is a no-op that opens nothing.
pub fn append_bounded_lines_no_follow(
    dir: &Path,
    path: &Path,
    lines: &[&str],
    max: usize,
    flush: bool,
) -> Result<bool> {
    append_bounded_lines_no_follow_within(dir, path, lines, max, flush, None)
}

/// [`append_bounded_lines_no_follow`], waiting at most `lock_wait` for the
/// lock when it is set: the lock is tried without blocking, again every few
/// milliseconds, and the call fails without writing anything once
/// `lock_wait` has passed with the lock still held elsewhere. `None` waits
/// for the lock as long as it takes. For a writer that must never stall
/// behind another one, such as a stopped process still holding the lock.
pub fn append_bounded_lines_no_follow_within(
    dir: &Path,
    path: &Path,
    lines: &[&str],
    max: usize,
    flush: bool,
    lock_wait: Option<std::time::Duration>,
) -> Result<bool> {
    for line in lines {
        let len = line.len() + 1;
        if len > max {
            anyhow::bail!(
                "line for {} exceeds the PIPE_BUF append bound ({} bytes > {})",
                path.display(),
                len,
                max
            );
        }
    }
    if lines.is_empty() {
        return Ok(false);
    }

    if !dir.exists() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
    }

    let mut options = OpenOptions::new();
    options.read(true).create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        options.custom_flags(libc::O_NOFOLLOW);
    }
    // O_APPEND: the kernel picks the offset per write. Never seek() or
    // write_at() on this handle.
    let mut file = options
        .open(path)
        .with_context(|| format!("failed to open {} for append", path.display()))?;

    #[cfg(unix)]
    let mut repair = {
        use std::os::unix::fs::FileExt;
        use std::os::unix::io::AsRawFd;
        lock_exclusive(file.as_raw_fd(), lock_wait, std::time::Instant::now())
            .with_context(|| format!("failed to lock {}", path.display()))?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        let mut last = [0u8; 1];
        size > 0 && file.read_at(&mut last, size - 1).is_ok() && last[0] != b'\n'
    };
    #[cfg(not(unix))]
    let mut repair = {
        // No advisory lock is taken off unix, so there is nothing to wait for.
        let _ = lock_wait;
        false
    };
    let mut buf = String::with_capacity(max + 1);
    for line in lines {
        buf.clear();
        // The repair newline rides with the first line, as the single-line
        // append has always written it.
        if std::mem::take(&mut repair) {
            buf.push('\n');
        }
        buf.push_str(line);
        buf.push('\n');
        file.write_all(buf.as_bytes())
            .with_context(|| format!("failed to append to {}", path.display()))?;
    }
    if flush {
        file.sync_data()
            .with_context(|| format!("failed to fsync {}", path.display()))?;
    }
    Ok(flush)
}

/// Take an exclusive `flock` on `fd`: blocking when `wait` is `None`,
/// otherwise tried without blocking until `wait` has passed since `started`.
/// A caller that may lock more than one file in turn passes the same
/// `started` each time, so `wait` bounds them all together. The lock is
/// always tried once, even when the time is already up.
#[cfg(unix)]
fn lock_exclusive(
    fd: std::os::unix::io::RawFd,
    wait: Option<std::time::Duration>,
    started: std::time::Instant,
) -> Result<()> {
    use std::time::{Duration, Instant};
    // SAFETY (each call below): flock on a descriptor borrowed for the
    // call; it neither closes nor retains it. The lock is released when the
    // caller's file closes.
    let Some(wait) = wait else {
        if unsafe { libc::flock(fd, libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        return Ok(());
    };
    const STEP: Duration = Duration::from_millis(5);
    let deadline = started + wait;
    loop {
        if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(());
        }
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::WouldBlock {
            return Err(err.into());
        }
        let now = Instant::now();
        if now >= deadline {
            anyhow::bail!(
                "the lock is held by another writer (waited {} ms)",
                wait.as_millis()
            );
        }
        std::thread::sleep(STEP.min(deadline - now));
    }
}

/// What a [`append_bounded_lines_current`] caller decided while holding the
/// lock on the file it is about to append to.
#[cfg(unix)]
pub enum UnderLock {
    /// Append to this file.
    Write,
    /// The file was moved away under the lock: open the path again.
    Reopen,
}

/// How many times [`append_bounded_lines_current`] opens the path before it
/// gives up on a file that keeps being moved. With a bounded lock wait it
/// keeps opening the path until the bound has passed as well.
#[cfg(unix)]
const REOPEN_ATTEMPTS: usize = 8;

/// Append `lines` to whatever file is at `path` once the lock is held, never
/// to a file that was renamed away in between.
///
/// [`append_bounded_lines_no_follow`]'s open, lock, per-line writes and
/// `flush`, plus two things a file
/// that can be rotated needs. After the lock is taken, the descriptor's
/// `(dev, ino)` is compared with the path's (`lstat`): a writer that opened
/// the file just before another process moved it away sees the mismatch and
/// opens the path again, so no line lands in a file after its rename. And
/// `under_lock` runs with the lock held and the file's size known, before
/// anything is written, so a caller can move the file away there (and answer
/// [`UnderLock::Reopen`]); the batch then goes whole into the next file.
/// Each line is bounded by `max` on its own; the partial-last-line repair is
/// the same as [`append_bounded_lines_no_follow`]'s.
///
/// `lock_wait` bounds the wait for the lock as in
/// [`append_bounded_lines_no_follow_within`], over the whole call: a file
/// opened again after a move gets what is left of the bound (and always one
/// try), not a fresh one. Past the bound the call fails without writing
/// anything; `under_lock` has then run only for files whose lock was taken.
/// The bound covers waiting for locks only: once a lock is held, the work
/// under it (`under_lock`, the writes, the flush) is never timed or cut
/// short. A file moved away while this call waits is opened again for as
/// long as the bound lasts, and at least [`REOPEN_ATTEMPTS`] times; without
/// a bound, exactly that many.
///
/// Returns whether it flushed. No lines is a no-op that opens nothing, so
/// `under_lock` doesn't run either.
#[cfg(unix)]
pub fn append_bounded_lines_current(
    dir: &Path,
    path: &Path,
    lines: &[&str],
    max: usize,
    flush: bool,
    lock_wait: Option<std::time::Duration>,
    under_lock: &mut dyn FnMut(&std::fs::File, u64) -> UnderLock,
) -> Result<bool> {
    use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
    use std::os::unix::io::AsRawFd;

    for line in lines {
        let len = line.len() + 1;
        if len > max {
            anyhow::bail!(
                "line for {} exceeds the PIPE_BUF append bound ({} bytes > {})",
                path.display(),
                len,
                max
            );
        }
    }
    if lines.is_empty() {
        return Ok(false);
    }
    if !dir.exists() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
    }

    let started = std::time::Instant::now();
    let mut attempts = 0;
    loop {
        // A bounded wait gives up on a file that keeps moving when its time
        // is up, not after a count: a writer polling for the lock can see
        // several rotations go by while it waits, each moving the file it
        // opened, and the bound is what promises it a fair wait.
        let out_of_time = lock_wait.is_none_or(|wait| started.elapsed() >= wait);
        if attempts >= REOPEN_ATTEMPTS && out_of_time {
            anyhow::bail!(
                "{} was moved away {} times while opening it",
                path.display(),
                attempts
            );
        }
        attempts += 1;
        let mut file = OpenOptions::new()
            .read(true)
            .create(true)
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .with_context(|| format!("failed to open {} for append", path.display()))?;
        lock_exclusive(file.as_raw_fd(), lock_wait, started)
            .with_context(|| format!("failed to lock {}", path.display()))?;
        let held = file
            .metadata()
            .with_context(|| format!("failed to stat {}", path.display()))?;
        match std::fs::symlink_metadata(path) {
            Ok(at_path) if at_path.dev() == held.dev() && at_path.ino() == held.ino() => {}
            // Moved away, or replaced, between the open and the lock.
            _ => continue,
        }
        if let UnderLock::Reopen = under_lock(&file, held.len()) {
            continue;
        }

        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        let mut last = [0u8; 1];
        let mut repair = size > 0 && file.read_at(&mut last, size - 1).is_ok() && last[0] != b'\n';
        let mut buf = String::with_capacity(max + 1);
        for line in lines {
            buf.clear();
            if std::mem::take(&mut repair) {
                buf.push('\n');
            }
            buf.push_str(line);
            buf.push('\n');
            file.write_all(buf.as_bytes())
                .with_context(|| format!("failed to append to {}", path.display()))?;
        }
        if flush {
            file.sync_data()
                .with_context(|| format!("failed to fsync {}", path.display()))?;
        }
        return Ok(flush);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_one_line_with_a_newline_and_creates_the_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a").join("b");
        let path = dir.join("f.jsonl");
        append_bounded_line(&dir, &path, r#"{"x":1}"#, 4096).unwrap();
        append_bounded_line(&dir, &path, r#"{"x":2}"#, 4096).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\"x\":1}\n{\"x\":2}\n"
        );
    }

    #[test]
    fn the_bound_counts_the_newline_and_refuses_before_opening() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("never-made");
        let path = dir.join("f.jsonl");
        // 9 bytes plus the newline is 10: exactly at the bound is fine.
        let tmp2 = tempfile::tempdir().unwrap();
        let ok = tmp2.path().join("ok.jsonl");
        append_bounded_line(tmp2.path(), &ok, "123456789", 10).unwrap();
        // One more byte is refused, and nothing is created.
        let err = append_bounded_line(&dir, &path, "1234567890", 10).unwrap_err();
        assert!(err.to_string().contains("11 bytes > 10"), "{}", err);
        assert!(!dir.exists());
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_created_file_is_0600_and_an_existing_mode_is_kept() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let fresh = tmp.path().join("fresh.jsonl");
        append_bounded_line(tmp.path(), &fresh, "{}", 4096).unwrap();
        let mode = std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        let existing = tmp.path().join("existing.jsonl");
        std::fs::write(&existing, "").unwrap();
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o644)).unwrap();
        append_bounded_line(tmp.path(), &existing, "{}", 4096).unwrap();
        let mode = std::fs::metadata(&existing).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644);
        assert_eq!(std::fs::read_to_string(&existing).unwrap(), "{}\n");
    }

    #[cfg(unix)]
    #[test]
    fn no_follow_refuses_a_symlink_and_leaves_its_target_alone() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target.txt");
        std::fs::write(&target, "untouched\n").unwrap();
        let link = tmp.path().join("journal.jsonl");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let err = append_bounded_line_no_follow(tmp.path(), &link, "{}", 4096).unwrap_err();
        assert!(err.to_string().contains("failed to open"), "{}", err);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched\n");

        // A regular file is appended to and created 0600, as
        // `append_bounded_line` does.
        let fresh = tmp.path().join("fresh.jsonl");
        append_bounded_line_no_follow(tmp.path(), &fresh, "{}", 4096).unwrap();
        append_bounded_line_no_follow(tmp.path(), &fresh, "{}", 4096).unwrap();
        assert_eq!(std::fs::read_to_string(&fresh).unwrap(), "{}\n{}\n");
        let mode = std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        let err = append_bounded_line_no_follow(tmp.path(), &fresh, "123456789", 9).unwrap_err();
        assert!(err.to_string().contains("10 bytes > 9"), "{}", err);
    }

    #[cfg(unix)]
    #[test]
    fn a_batch_writes_its_lines_in_order_after_repairing_a_partial_line() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("j.jsonl");
        std::fs::write(&path, "{\"a\":1}\n{\"torn\":").unwrap();
        let flushed = append_bounded_lines_no_follow(
            tmp.path(),
            &path,
            &["{\"b\":2}", "{\"c\":3}"],
            64,
            false,
        )
        .unwrap();
        assert!(!flushed);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\"a\":1}\n{\"torn\":\n{\"b\":2}\n{\"c\":3}\n"
        );
        // A batch asked to flush says so.
        assert!(append_bounded_lines_no_follow(tmp.path(), &path, &["{}"], 64, true).unwrap());
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .ends_with("{\"c\":3}\n{}\n"));
    }

    #[cfg(unix)]
    #[test]
    fn a_batch_with_a_line_over_the_bound_writes_nothing_and_no_lines_opens_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("never-made");
        let path = dir.join("j.jsonl");
        let err = append_bounded_lines_no_follow(&dir, &path, &["ok", "1234567890"], 10, true)
            .unwrap_err();
        assert!(err.to_string().contains("11 bytes > 10"), "{}", err);
        assert!(!dir.exists());
        assert!(!append_bounded_lines_no_follow(&dir, &path, &[], 10, true).unwrap());
        assert!(!dir.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_batch_refuses_a_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target.txt");
        std::fs::write(&target, "untouched\n").unwrap();
        let link = tmp.path().join("j.jsonl");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(append_bounded_lines_no_follow(tmp.path(), &link, &["{}"], 64, false).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched\n");
    }

    /// With the lock held elsewhere, a bounded wait gives up after about
    /// its bound and writes nothing; once the lock is free it writes.
    #[cfg(unix)]
    #[test]
    fn a_bounded_lock_wait_gives_up_while_another_writer_holds_the_lock() {
        use std::os::unix::io::AsRawFd;
        use std::time::{Duration, Instant};
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("j.jsonl");
        std::fs::write(&path, "{\"a\":1}\n").unwrap();
        let holder = std::fs::File::open(&path).unwrap();
        // SAFETY: flock on a descriptor `holder` owns for the whole test.
        assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX) }, 0);
        let wait = Duration::from_millis(30);
        let started = Instant::now();
        let err = append_bounded_lines_no_follow_within(
            tmp.path(),
            &path,
            &["{}"],
            64,
            false,
            Some(wait),
        )
        .unwrap_err();
        let took = started.elapsed();
        assert!(
            format!("{err:#}").contains("held by another writer"),
            "{err:#}"
        );
        assert!(took >= wait, "{took:?}");
        assert!(took < Duration::from_secs(2), "{took:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":1}\n");
        drop(holder);
        append_bounded_lines_no_follow_within(tmp.path(), &path, &["{}"], 64, false, Some(wait))
            .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":1}\n{}\n");
    }

    /// A bounded wait covers the whole call: when the file is moved away
    /// under the lock and the fresh one at the path is locked elsewhere, the
    /// call gives up after about the bound, not the bound again, and writes
    /// nothing to either file.
    #[cfg(unix)]
    #[test]
    fn a_bounded_wait_covers_the_file_opened_again_after_a_move() {
        use std::os::unix::io::AsRawFd;
        use std::time::{Duration, Instant};
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("j.jsonl");
        let moved = tmp.path().join("moved.jsonl");
        std::fs::write(&path, "{\"a\":1}\n").unwrap();
        let mut holder: Option<std::fs::File> = None;
        let mut calls = 0;
        let wait = Duration::from_millis(30);
        let started = Instant::now();
        let err = append_bounded_lines_current(
            tmp.path(),
            &path,
            &["{}"],
            64,
            false,
            Some(wait),
            &mut |_, _| {
                calls += 1;
                std::fs::rename(&path, &moved).unwrap();
                std::fs::write(&path, "").unwrap();
                let h = std::fs::File::open(&path).unwrap();
                // SAFETY: flock on a descriptor `h` owns; `holder` keeps it
                // open until the test ends.
                assert_eq!(unsafe { libc::flock(h.as_raw_fd(), libc::LOCK_EX) }, 0);
                holder = Some(h);
                UnderLock::Reopen
            },
        )
        .unwrap_err();
        let took = started.elapsed();
        assert!(
            format!("{err:#}").contains("held by another writer"),
            "{err:#}"
        );
        assert_eq!(calls, 1);
        assert!(took >= wait, "{took:?}");
        assert!(took < Duration::from_secs(2), "{took:?}");
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "{\"a\":1}\n");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        drop(holder);
    }

    /// A move that takes longer than the bound doesn't cost the batch: the
    /// fresh file's lock is still tried once, and a free one is taken.
    #[cfg(unix)]
    #[test]
    fn a_bounded_wait_spent_under_the_lock_still_tries_the_fresh_file() {
        use std::time::Duration;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("j.jsonl");
        let moved = tmp.path().join("moved.jsonl");
        std::fs::write(&path, "{\"a\":1}\n").unwrap();
        let wait = Duration::from_millis(10);
        let mut calls = 0;
        append_bounded_lines_current(
            tmp.path(),
            &path,
            &["{\"b\":2}", "{\"c\":3}"],
            64,
            false,
            Some(wait),
            &mut |_, _| {
                calls += 1;
                if calls > 1 {
                    return UnderLock::Write;
                }
                std::thread::sleep(wait * 3);
                std::fs::rename(&path, &moved).unwrap();
                UnderLock::Reopen
            },
        )
        .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "{\"a\":1}\n");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\"b\":2}\n{\"c\":3}\n"
        );
    }

    /// A writer whose file keeps being moved away while it waits opens the
    /// path again for as long as its bound lasts, past the attempt count an
    /// unbounded call stops at, and writes its batch whole to the file it
    /// finally holds.
    #[cfg(unix)]
    #[test]
    fn a_bounded_wait_keeps_reopening_a_moved_file_until_its_time_is_up() {
        use std::time::Duration;
        let moves = REOPEN_ATTEMPTS + 3;
        let run = |lock_wait: Option<Duration>| {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("j.jsonl");
            let mut calls = 0;
            let result = append_bounded_lines_current(
                tmp.path(),
                &path,
                &["{\"b\":2}", "{\"c\":3}"],
                64,
                false,
                lock_wait,
                &mut |_, _| {
                    calls += 1;
                    if calls > moves {
                        return UnderLock::Write;
                    }
                    let moved = tmp.path().join(format!("moved-{calls}.jsonl"));
                    std::fs::rename(&path, moved).unwrap();
                    UnderLock::Reopen
                },
            );
            let written = std::fs::read_to_string(&path).unwrap_or_default();
            (result.map_err(|e| format!("{e:#}")), calls, written)
        };

        let (result, calls, written) = run(Some(Duration::from_secs(5)));
        assert_eq!(result, Ok(false));
        assert_eq!(calls, moves + 1);
        assert_eq!(written, "{\"b\":2}\n{\"c\":3}\n");

        let (result, calls, written) = run(None);
        assert!(result.unwrap_err().contains("moved away"));
        assert_eq!(calls, REOPEN_ATTEMPTS);
        assert_eq!(written, "");
    }

    #[test]
    fn n_threads_produce_n_whole_lines() {
        use std::sync::Arc;
        use std::thread;

        let tmp = tempfile::tempdir().unwrap();
        let dir = Arc::new(tmp.path().to_path_buf());
        let n = 32_usize;
        let handles: Vec<_> = (0..n)
            .map(|i| {
                let dir = Arc::clone(&dir);
                thread::spawn(move || {
                    let line = format!(r#"{{"i":{},"pad":"{}"}}"#, i, "x".repeat(3000));
                    append_bounded_line(&dir, &dir.join("f.jsonl"), &line, 4096).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let body = std::fs::read_to_string(dir.join("f.jsonl")).unwrap();
        let mut seen: Vec<u64> = body
            .lines()
            .map(|l| {
                serde_json::from_str::<serde_json::Value>(l).unwrap()["i"]
                    .as_u64()
                    .unwrap()
            })
            .collect();
        seen.sort();
        assert_eq!(seen, (0..n as u64).collect::<Vec<_>>());
    }
}
