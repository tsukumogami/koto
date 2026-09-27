//! The per-session wake signal: one append-only file per session name
//! that changes every time something the session may be waiting on
//! changes.
//!
//! ```text
//! ~/.koto/wakes/<session>   one opaque line per wake, never renamed
//! ```
//!
//! # What a wake means
//!
//! Only "look again". A line carries a token that exists so no two lines
//! are byte-identical; nothing in koto reads it as input to a decision. A
//! session that is woken re-reads its own log and the request store, so a
//! lost wake costs latency and a duplicate costs one tick that finds
//! nothing new.
//!
//! # Who writes it
//!
//! The request store rings the requester and coordinator of record after
//! every leg result, leg abandonment and request close
//! ([`crate::engine::request_store`]), and the wake-candidates pass rings
//! through `SignalWaker` in [`crate::engine::wake`]. Both run in the
//! process that made the change, so the file has changed before that
//! command returns.
//!
//! # Why the file is appended, never replaced
//!
//! Subscribers watch the path: `koto request watch` polls it, a harness
//! may put a native file watcher on it or run `tail -F`. Replacing the
//! file by rename would give the path a new inode and silence a watcher
//! registered on the old one, so a ring opens the one file with
//! `O_APPEND` and writes one short line in a single `write`, well under
//! `PIPE_BUF`, so concurrent rings never interleave. There is no lock and
//! no fsync: the wake has to be visible when the ringing command returns,
//! not durable across a crash.
//!
//! Once the file reaches [`TRUNCATE_AT`] bytes, a ring truncates it in
//! place before appending, so it stays far under [`MAX_WAKE_FILE_BYTES`]
//! whatever the wake volume. A poller comparing sizes can miss a wake
//! across a truncation, which is why the cursor also carries the last
//! token.
//!
//! # Hostile paths
//!
//! A principal is validated as a session id before it is joined onto a
//! path. The directory is refused if it is a symlink, the file is opened
//! with `O_NOFOLLOW`, and the open is non-blocking and must yield a
//! regular file, so a FIFO planted at the path can neither block a
//! worker's terminal tick nor receive the write.

use std::fmt;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::engine::atomic_fs::create_private_dir;
use crate::engine::types::ValidatedSessionId;

/// Directory under the koto root holding every wake file.
pub const WAKES_DIR: &str = "wakes";

/// Size at which a ring truncates the file before appending.
pub const TRUNCATE_AT: u64 = 32 * 1024;

/// The bound a wake file stays under. Reaching it would take hundreds of
/// concurrent rings that all read a size under [`TRUNCATE_AT`] before any
/// of them truncated.
pub const MAX_WAKE_FILE_BYTES: u64 = 64 * 1024;

/// How far back from the end a cursor read looks for the last line.
const TAIL_WINDOW: u64 = 256;

/// Longest token a cursor carries.
const MAX_TOKEN_LEN: usize = 64;

/// Per-process counter that keeps two rings from one process distinct
/// even when the clock reads the same nanosecond twice.
static RING_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Why a ring failed.
#[derive(Debug, Error)]
pub enum WakeSignalError {
    /// The principal is not a valid session id, so it names no wake file.
    #[error("{principal:?} is not a valid session id: {reason}")]
    InvalidPrincipal { principal: String, reason: String },

    /// Something other than a regular file sits at the wake path.
    #[error("{} is not a regular file", path.display())]
    NotRegularFile { path: PathBuf },

    /// Anything the filesystem refused.
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
}

impl WakeSignalError {
    fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}

/// `<koto_root>/wakes`.
pub fn wakes_dir(koto_root: &Path) -> PathBuf {
    koto_root.join(WAKES_DIR)
}

/// `<koto_root>/wakes/<session>`.
pub fn wake_path(koto_root: &Path, session: &ValidatedSessionId) -> PathBuf {
    wakes_dir(koto_root).join(session.as_str())
}

/// Append one wake line to `principal`'s wake file.
///
/// Refuses a principal that is not a valid session id before any path is
/// built. Every other failure is an I/O error the caller reports; nothing
/// here retries.
pub fn ring(koto_root: &Path, principal: &str) -> Result<(), WakeSignalError> {
    let session =
        ValidatedSessionId::new(principal).map_err(|e| WakeSignalError::InvalidPrincipal {
            principal: principal.chars().take(64).collect(),
            reason: e.to_string(),
        })?;
    let dir = wakes_dir(koto_root);
    create_private_dir(&dir)
        .map_err(|e| WakeSignalError::io(format!("preparing {}", dir.display()), e))?;
    let path = wake_path(koto_root, &session);

    let mut file = open_for_ring(&path)
        .map_err(|e| WakeSignalError::io(format!("opening {}", path.display()), e))?;
    let meta = file
        .metadata()
        .map_err(|e| WakeSignalError::io(format!("reading {}", path.display()), e))?;
    if !meta.is_file() {
        return Err(WakeSignalError::NotRegularFile { path });
    }
    if meta.len() >= TRUNCATE_AT {
        file.set_len(0)
            .map_err(|e| WakeSignalError::io(format!("truncating {}", path.display()), e))?;
    }

    let line = format!("{}\n", next_token());
    file.write_all(line.as_bytes())
        .map_err(|e| WakeSignalError::io(format!("appending to {}", path.display()), e))
}

/// Ring each distinct principal a request names.
///
/// Never fails the caller: a principal that is not a valid session id is
/// skipped, and a ring that fails is reported, each as one warning on
/// stderr naming the principal. The write that triggered the ring has
/// already succeeded by the time this runs.
pub fn ring_principals(koto_root: &Path, requested_by: &str, coordinator_of_record: &str) {
    let mut principals = vec![coordinator_of_record];
    if requested_by != coordinator_of_record {
        principals.push(requested_by);
    }
    for principal in principals {
        match ring(koto_root, principal) {
            Ok(()) => {}
            Err(WakeSignalError::InvalidPrincipal { principal, .. }) => {
                eprintln!("warning: no wake delivered to {principal:?}: not a valid session id");
            }
            Err(e) => {
                eprintln!("warning: could not deliver a wake to {principal:?}: {e}");
            }
        }
    }
}

/// A subscriber's position in one session's wake file.
///
/// Written as `w1:<length>:<last token>`. Two reads of an unchanged file
/// give equal cursors, and every ring changes the cursor, including a
/// ring that truncated the file and left it at a length seen before,
/// because the last token is unique per ring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeCursor {
    len: u64,
    token: String,
}

impl WakeCursor {
    /// The cursor of a file that has never been rung.
    pub fn empty() -> Self {
        Self {
            len: 0,
            token: String::new(),
        }
    }
}

impl fmt::Display for WakeCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "w1:{}:{}", self.len, self.token)
    }
}

/// A `--since` value that is not a cursor this build wrote.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("not a wake cursor: expected w1:<length>:<token>")]
pub struct WakeCursorParseError;

impl FromStr for WakeCursor {
    type Err = WakeCursorParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let rest = s.strip_prefix("w1:").ok_or(WakeCursorParseError)?;
        let (len, token) = rest.split_once(':').ok_or(WakeCursorParseError)?;
        if len.is_empty() || !len.bytes().all(|b| b.is_ascii_digit()) {
            return Err(WakeCursorParseError);
        }
        let len: u64 = len.parse().map_err(|_| WakeCursorParseError)?;
        if !is_token(token) || (len == 0) != token.is_empty() {
            return Err(WakeCursorParseError);
        }
        Ok(Self {
            len,
            token: token.to_string(),
        })
    }
}

/// Read the current cursor of `session`'s wake file.
///
/// An absent file reads as [`WakeCursor::empty`]. A symlink or any other
/// non-regular file at the path is an error, as for a ring.
pub fn read_cursor(koto_root: &Path, session: &ValidatedSessionId) -> std::io::Result<WakeCursor> {
    let path = wake_path(koto_root, session);
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(WakeCursor::empty()),
        Err(e) => return Err(e),
        Ok(md) if !md.file_type().is_file() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{} is not a regular file", path.display()),
            ))
        }
        Ok(_) => {}
    }
    let mut file = match open_for_read(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(WakeCursor::empty()),
        other => other?,
    };
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} is not a regular file", path.display()),
        ));
    }
    let len = meta.len();
    if len == 0 {
        return Ok(WakeCursor::empty());
    }
    let start = len.saturating_sub(TAIL_WINDOW);
    file.seek(SeekFrom::Start(start))?;
    let mut tail = Vec::with_capacity((len - start) as usize);
    file.take(len - start).read_to_end(&mut tail)?;
    Ok(WakeCursor {
        len,
        token: last_token(&tail),
    })
}

/// The token of the last complete line in `tail`, normalised so it always
/// satisfies [`is_token`]. A line koto did not write is replaced by a
/// digest of itself, so the cursor still changes when the file does.
fn last_token(tail: &[u8]) -> String {
    let body = match tail.iter().rposition(|&b| b == b'\n') {
        Some(end) => &tail[..end],
        // No complete line in the window; the bytes still identify it.
        None => tail,
    };
    let line = match body.iter().rposition(|&b| b == b'\n') {
        Some(start) => &body[start + 1..],
        None => body,
    };
    match std::str::from_utf8(line) {
        Ok(s) if is_token(s) && !s.is_empty() => s.to_string(),
        _ => {
            let digest = Sha256::digest(line);
            format!("x{}", &hex::encode(digest)[..16])
        }
    }
}

/// Whether `s` is a token a cursor may carry.
fn is_token(s: &str) -> bool {
    s.len() <= MAX_TOKEN_LEN
        && s.bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase() || b == b'.')
}

/// A fresh token for this process's next ring.
fn next_token() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    token_for(nanos, std::process::id())
}

/// `<nanos>.<pid>.<counter>`, with the counter drawn from this process.
fn token_for(nanos: u128, pid: u32) -> String {
    let n = RING_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos}.{pid}.{n}")
}

#[cfg(unix)]
fn open_for_ring(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_for_ring(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
}

#[cfg(unix)]
fn open_for_read(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_for_read(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sid(s: &str) -> ValidatedSessionId {
        ValidatedSessionId::new(s).expect("valid id")
    }

    fn lines(root: &Path, session: &str) -> Vec<String> {
        std::fs::read_to_string(wake_path(root, &sid(session)))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn a_ring_appends_one_line_naming_this_process() {
        let tmp = tempfile::tempdir().unwrap();
        ring(tmp.path(), "coord").unwrap();
        let got = lines(tmp.path(), "coord");
        assert_eq!(got.len(), 1);
        let parts: Vec<&str> = got[0].split('.').collect();
        assert_eq!(parts.len(), 3, "token is nanos.pid.counter: {}", got[0]);
        assert_eq!(parts[1], std::process::id().to_string());
        ring(tmp.path(), "coord").unwrap();
        assert_eq!(lines(tmp.path(), "coord").len(), 2);
    }

    #[test]
    fn two_tokens_in_the_same_nanosecond_still_differ() {
        let a = token_for(42, 7);
        let b = token_for(42, 7);
        assert_ne!(a, b);
        assert!(a.starts_with("42.7.") && b.starts_with("42.7."));
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_0600_in_a_0700_directory() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        ring(tmp.path(), "coord").unwrap();
        let dir_mode = std::fs::metadata(wakes_dir(tmp.path()))
            .unwrap()
            .permissions()
            .mode();
        let file_mode = std::fs::metadata(wake_path(tmp.path(), &sid("coord")))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(dir_mode & 0o777, 0o700);
        assert_eq!(file_mode & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_at_the_path_is_refused_without_blocking() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(wakes_dir(tmp.path())).unwrap();
        let path = wake_path(tmp.path(), &sid("coord"));
        let c = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        // SAFETY: a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        // With no reader, a non-blocking write open fails outright; were
        // it blocking, this test would hang here.
        assert!(ring(tmp.path(), "coord").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_with_a_reader_is_refused_as_not_regular() {
        use std::os::unix::fs::OpenOptionsExt;
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(wakes_dir(tmp.path())).unwrap();
        let path = wake_path(tmp.path(), &sid("coord"));
        let c = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        // SAFETY: a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let _reader = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path)
            .unwrap();
        let err = ring(tmp.path(), "coord").expect_err("a FIFO is not a wake file");
        assert!(
            matches!(err, WakeSignalError::NotRegularFile { .. }),
            "{err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_wakes_directory_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, wakes_dir(tmp.path())).unwrap();
        assert!(ring(tmp.path(), "coord").is_err());
        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_wake_file_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(wakes_dir(tmp.path())).unwrap();
        let target = tmp.path().join("target");
        std::fs::write(&target, b"").unwrap();
        std::os::unix::fs::symlink(&target, wake_path(tmp.path(), &sid("coord"))).unwrap();
        assert!(ring(tmp.path(), "coord").is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"");
    }

    #[cfg(unix)]
    #[test]
    fn the_cap_truncates_in_place_and_keeps_the_inode() {
        use std::os::unix::fs::MetadataExt;
        let tmp = tempfile::tempdir().unwrap();
        ring(tmp.path(), "coord").unwrap();
        let path = wake_path(tmp.path(), &sid("coord"));
        let inode = std::fs::metadata(&path).unwrap().ino();
        let mut max = 0;
        for _ in 0..10_000 {
            ring(tmp.path(), "coord").unwrap();
            max = max.max(std::fs::metadata(&path).unwrap().len());
        }
        assert!(max <= MAX_WAKE_FILE_BYTES, "grew to {max}");
        assert!(
            max >= TRUNCATE_AT,
            "never reached the cap, so never truncated"
        );
        assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode);
    }

    #[test]
    fn an_absent_file_reads_as_the_empty_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        let c = read_cursor(tmp.path(), &sid("coord")).unwrap();
        assert_eq!(c, WakeCursor::empty());
        assert_eq!(c.to_string(), "w1:0:");
    }

    #[test]
    fn a_cursor_round_trips_and_changes_on_every_ring() {
        let tmp = tempfile::tempdir().unwrap();
        let mut seen = vec![read_cursor(tmp.path(), &sid("coord")).unwrap()];
        for _ in 0..5 {
            ring(tmp.path(), "coord").unwrap();
            let c = read_cursor(tmp.path(), &sid("coord")).unwrap();
            assert_eq!(c.to_string().parse::<WakeCursor>().unwrap(), c);
            assert!(!seen.contains(&c), "cursor repeated: {c}");
            seen.push(c);
        }
        let unchanged = read_cursor(tmp.path(), &sid("coord")).unwrap();
        assert_eq!(&unchanged, seen.last().unwrap());
    }

    #[test]
    fn a_truncation_back_to_a_seen_length_still_changes_the_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        ring(tmp.path(), "coord").unwrap();
        let before = read_cursor(tmp.path(), &sid("coord")).unwrap();
        // Simulate a truncate-and-append cycle that lands on the same
        // length: replace the file's content with one different line of
        // the same size.
        let path = wake_path(tmp.path(), &sid("coord"));
        let old = std::fs::read_to_string(&path).unwrap();
        let mut replacement = old.trim_end().to_string();
        let last = replacement.pop().unwrap();
        replacement.push(if last == '9' { '8' } else { '9' });
        replacement.push('\n');
        std::fs::write(&path, &replacement).unwrap();
        let after = read_cursor(tmp.path(), &sid("coord")).unwrap();
        assert_eq!(before.len, after.len);
        assert_ne!(before, after);
    }

    #[test]
    fn a_foreign_line_still_yields_a_parseable_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(wakes_dir(tmp.path())).unwrap();
        std::fs::write(wake_path(tmp.path(), &sid("coord")), "Hello: World!\n").unwrap();
        let c = read_cursor(tmp.path(), &sid("coord")).unwrap();
        assert_eq!(c.to_string().parse::<WakeCursor>().unwrap(), c);
    }

    #[test]
    fn malformed_cursors_do_not_parse() {
        for bad in [
            "",
            "w1",
            "w1:",
            "w1:5",
            "w2:5:1.2.3",
            "w1:x:1.2.3",
            "w1:0:1.2.3",
            "w1:5:",
            "w1:5:ABC",
            "w1:5:a b",
            "w1:-1:1.2",
        ] {
            assert!(bad.parse::<WakeCursor>().is_err(), "parsed {bad:?}");
        }
        assert!("w1:0:".parse::<WakeCursor>().is_ok());
        assert!("w1:12:1700.42.0".parse::<WakeCursor>().is_ok());
    }

    #[test]
    fn an_invalid_principal_is_refused_before_any_path_is_built() {
        let tmp = tempfile::tempdir().unwrap();
        let err = ring(tmp.path(), "../escape").expect_err("must refuse");
        assert!(matches!(err, WakeSignalError::InvalidPrincipal { .. }));
        assert!(!wakes_dir(tmp.path()).exists());
    }

    #[test]
    fn ring_principals_rings_a_shared_name_once() {
        let tmp = tempfile::tempdir().unwrap();
        ring_principals(tmp.path(), "coord", "coord");
        assert_eq!(lines(tmp.path(), "coord").len(), 1);
    }

    #[test]
    fn ring_principals_rings_both_distinct_names() {
        let tmp = tempfile::tempdir().unwrap();
        ring_principals(tmp.path(), "asker", "coord");
        assert_eq!(lines(tmp.path(), "asker").len(), 1);
        assert_eq!(lines(tmp.path(), "coord").len(), 1);
        assert!(lines(tmp.path(), "bystander").is_empty());
    }

    #[test]
    fn ring_principals_skips_an_invalid_name_and_still_rings_the_other() {
        let tmp = tempfile::tempdir().unwrap();
        ring_principals(tmp.path(), "asker", "../bad");
        assert_eq!(lines(tmp.path(), "asker").len(), 1);
        ring_principals(tmp.path(), "../bad", "coord");
        assert_eq!(lines(tmp.path(), "coord").len(), 1);
    }
}
