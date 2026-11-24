//! The session store read surface as a program other than koto sees it:
//! `koto_session_reader`'s public items, on stores built at run time.
//!
//! The layout marker: `check_layout` and `LocalDirSource::new` accept a
//! store with no marker or one naming layout 1, and refuse one naming a
//! newer layout or a marker that can't be read as a number (a FIFO, a
//! symlink, a directory, an over-cap file, text), each with a
//! `LayoutError` naming the marker's path.

#![cfg(unix)]

#[path = "support/golden_store.rs"]
mod golden_store;
#[path = "support/host_session.rs"]
mod host_session;

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use koto_session_reader::source::{
    check_layout, LayoutError, LocalDirSource, LAYOUT_MARKER_READ_CAP, STORE_LAYOUT_MARKER,
    STORE_LAYOUT_VERSION,
};
use tempfile::TempDir;

/// How long one marker check may take before the test calls it blocked.
const DEADLINE: Duration = Duration::from_secs(10);

/// The golden store in `<tmp>/sessions`.
fn store() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let sessions = tmp.path().join("sessions");
    golden_store::build(&sessions, SystemTime::now());
    (tmp, sessions)
}

fn marker(sessions: &Path) -> PathBuf {
    sessions.join(STORE_LAYOUT_MARKER)
}

fn mkfifo(path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
}

/// `check_layout` and `LocalDirSource::new` on `sessions`, on a thread,
/// failing the test if they haven't returned within [`DEADLINE`]. Both must
/// give the same answer.
fn layout_of(sessions: &Path) -> Result<(), LayoutError> {
    let (tx, rx) = mpsc::channel();
    let dir = sessions.to_path_buf();
    std::thread::spawn(move || {
        let checked = check_layout(&dir);
        let opened = LocalDirSource::new(&dir).map(|_| ());
        let _ = tx.send((checked, opened));
    });
    let (checked, opened) = rx.recv_timeout(DEADLINE).expect("the marker check blocked");
    assert_eq!(
        checked, opened,
        "check_layout and LocalDirSource::new disagree"
    );
    checked
}

/// Checks that `sessions` is refused as unusable with `category`, the
/// message naming the marker's path.
fn assert_unusable(sessions: &Path, category: &'static str) {
    let path = marker(sessions);
    let err = layout_of(sessions).expect_err("the store was accepted");
    assert_eq!(
        err,
        LayoutError::Unusable {
            path: path.clone(),
            category,
        }
    );
    assert_eq!(
        err.to_string(),
        format!(
            "session store layout marker {} is unusable: {category}",
            path.display()
        )
    );
}

#[test]
fn a_store_with_no_marker_is_layout_1() {
    let (_tmp, sessions) = store();
    assert_eq!(layout_of(&sessions), Ok(()));
}

#[test]
fn a_sessions_directory_that_does_not_exist_is_layout_1() {
    let tmp = TempDir::new().unwrap();
    assert_eq!(layout_of(&tmp.path().join("missing")), Ok(()));
}

#[test]
fn a_marker_naming_layout_1_is_accepted() {
    let (_tmp, sessions) = store();
    for text in ["1", "1\n", " 1 \n"] {
        std::fs::write(marker(&sessions), text).unwrap();
        assert_eq!(layout_of(&sessions), Ok(()), "{text:?}");
    }
}

#[test]
fn a_marker_of_exactly_the_cap_is_read() {
    let (_tmp, sessions) = store();
    let text = format!("1{}", " ".repeat(LAYOUT_MARKER_READ_CAP as usize - 1));
    assert_eq!(text.len() as u64, LAYOUT_MARKER_READ_CAP);
    std::fs::write(marker(&sessions), text).unwrap();
    assert_eq!(layout_of(&sessions), Ok(()));
}

#[test]
fn a_marker_naming_layout_2_is_refused_naming_both_numbers() {
    let (_tmp, sessions) = store();
    std::fs::write(marker(&sessions), "2\n").unwrap();
    let err = layout_of(&sessions).expect_err("a layout 2 store was accepted");
    assert_eq!(
        err,
        LayoutError::Newer {
            path: marker(&sessions),
            found: 2,
            supported: STORE_LAYOUT_VERSION,
        }
    );
    assert_eq!(STORE_LAYOUT_VERSION, 1);
    assert_eq!(
        err.to_string(),
        format!(
            "session store layout marker {} says layout 2; this reader supports layout 1",
            marker(&sessions).display()
        )
    );
}

#[test]
fn a_fifo_marker_is_refused_without_blocking() {
    let (_tmp, sessions) = store();
    mkfifo(&marker(&sessions));
    assert_unusable(&sessions, "not a regular file");
}

#[test]
fn a_symlinked_marker_is_refused() {
    let (tmp, sessions) = store();
    let target = tmp.path().join("layout-1");
    std::fs::write(&target, "1\n").unwrap();
    std::os::unix::fs::symlink(&target, marker(&sessions)).unwrap();
    assert_unusable(&sessions, "a symlink");
}

#[test]
fn a_directory_marker_is_refused() {
    let (_tmp, sessions) = store();
    std::fs::create_dir(marker(&sessions)).unwrap();
    assert_unusable(&sessions, "not a regular file");
}

#[test]
fn a_marker_over_64_bytes_is_refused() {
    let (_tmp, sessions) = store();
    // A number, so only the size refuses it.
    let text = format!("1{}", " ".repeat(LAYOUT_MARKER_READ_CAP as usize));
    assert_eq!(text.len(), 65);
    std::fs::write(marker(&sessions), text).unwrap();
    assert_unusable(&sessions, "larger than 64 bytes");
}

#[test]
fn a_marker_that_is_not_a_number_is_refused() {
    let (_tmp, sessions) = store();
    std::fs::write(marker(&sessions), "abc\n").unwrap();
    assert_unusable(&sessions, "not a layout number");
}
