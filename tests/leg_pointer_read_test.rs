//! `read_pointer` reads through koto-session-reader's hardened store read.
//!
//! A regular pointer reads exactly as it did through a plain open, including
//! when the session directory is reached through a symlinked path. A FIFO, a
//! symlink or a file over the 64 KiB cap at the pointer's path is refused,
//! and the FIFO is refused without blocking. The crate can't write files in
//! its own tests, so the fixtures are built here and the read is reached
//! through koto's re-export.

#![cfg(unix)]

#[path = "support/host_session.rs"]
mod host_session;

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use koto::engine::leg_pointer::{
    pointer_path, read_pointer, write_pointer, LegPointer, POINTER_READ_CAP,
};

fn sample() -> LegPointer {
    LegPointer {
        request_id: "req-abcdef".to_string(),
        leg_name: "reviewer-a".to_string(),
        bound_at: "2026-07-29T00:00:00.000Z".to_string(),
    }
}

/// What the read returned before the hardening: the file's text parsed as
/// TOML.
fn plain_read(path: &Path) -> LegPointer {
    toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Run `read_pointer` on a thread and fail the test if it hasn't returned
/// within ten seconds.
fn read_without_blocking(session_dir: &Path) -> anyhow::Result<Option<LegPointer>> {
    let (tx, rx) = mpsc::channel();
    let dir = session_dir.to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(read_pointer(&dir));
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("read_pointer blocked")
}

fn mkfifo(path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
}

/// A pointer body padded with a TOML comment to exactly `len` bytes, so it
/// still parses and only its size can refuse it.
fn padded_pointer(len: usize) -> String {
    let mut body = toml::to_string(&sample()).unwrap();
    body.push('#');
    while body.len() < len - 1 {
        body.push('x');
    }
    body.push('\n');
    assert_eq!(body.len(), len);
    body
}

#[test]
fn a_regular_pointer_reads_as_before() {
    let dir = tempfile::tempdir().unwrap();
    write_pointer(dir.path(), &sample()).unwrap();
    let before = plain_read(&pointer_path(dir.path()));
    assert_eq!(read_pointer(dir.path()).unwrap(), Some(before));
    assert_eq!(read_pointer(dir.path()).unwrap(), Some(sample()));
}

#[test]
fn a_session_without_a_pointer_still_reads_as_none() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(read_pointer(dir.path()).unwrap(), None);
}

#[test]
fn a_session_reached_through_a_symlinked_path_still_reads_its_pointer() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real-session");
    std::fs::create_dir(&real).unwrap();
    write_pointer(&real, &sample()).unwrap();
    let linked = dir.path().join("linked-session");
    std::os::unix::fs::symlink(&real, &linked).unwrap();
    assert_eq!(read_pointer(&linked).unwrap(), Some(sample()));
}

#[test]
fn a_pointer_of_exactly_the_cap_reads() {
    let dir = tempfile::tempdir().unwrap();
    let path = pointer_path(dir.path());
    std::fs::write(&path, padded_pointer(POINTER_READ_CAP as usize)).unwrap();
    assert_eq!(read_pointer(dir.path()).unwrap(), Some(plain_read(&path)));
}

#[test]
fn a_fifo_at_the_pointer_path_is_refused_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    mkfifo(&pointer_path(dir.path()));
    let err = read_without_blocking(dir.path()).expect_err("a FIFO must be refused");
    assert!(
        err.to_string().contains("not a regular file"),
        "unexpected error: {err}"
    );
}

#[test]
fn a_symlink_at_the_pointer_path_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("elsewhere.toml");
    std::fs::write(&target, toml::to_string(&sample()).unwrap()).unwrap();
    std::os::unix::fs::symlink(&target, pointer_path(dir.path())).unwrap();
    assert!(
        read_without_blocking(dir.path()).is_err(),
        "a symlink to a valid pointer must still be refused"
    );
}

#[test]
fn a_symlink_to_a_fifo_at_the_pointer_path_is_refused_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("elsewhere.fifo");
    mkfifo(&fifo);
    std::os::unix::fs::symlink(&fifo, pointer_path(dir.path())).unwrap();
    assert!(read_without_blocking(dir.path()).is_err());
}

#[test]
fn a_pointer_over_the_cap_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = pointer_path(dir.path());
    std::fs::write(&path, padded_pointer(POINTER_READ_CAP as usize + 1)).unwrap();
    let err = read_without_blocking(dir.path()).expect_err("an over-cap pointer must be refused");
    assert!(
        err.to_string().contains("larger than 65536 bytes"),
        "unexpected error: {err}"
    );
}

#[test]
fn a_directory_at_the_pointer_path_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(pointer_path(dir.path())).unwrap();
    assert!(read_without_blocking(dir.path()).is_err());
}
