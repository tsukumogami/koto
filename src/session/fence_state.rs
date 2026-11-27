//! What this process knows that the whole-binding fence asks about
//! (`crate::session::rebind_fence::check_whole`): the directory it runs in,
//! and the rebind epoch it first read of each session log.
//!
//! The directory is read and canonicalized once. The epoch is the first one
//! read: a later read of the same log doesn't replace it, so a process that
//! read a session before a whole-binding move and reads it again afterwards
//! is still refused. A `koto next` reads its log under `tick.lock`, which a
//! move takes exclusive, so a tick's first read is the epoch it runs at. A
//! write with no read before it records the epoch it finds, so the next
//! write in the process is measured against that.
//!
//! The cloud backend keeps its own epoch, refreshed on every read
//! (`CloudBackend::note_epoch_read`), since each of its reads pulls first.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::session::rebind_fence::{self, SessionRebound};

/// The canonical directory this process runs in, `None` when it can't be
/// read or canonicalized (a removed directory). Read once.
pub fn cwd() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(dir) = TEST_CWD.with(|c| c.borrow().clone()) {
        return Some(dir);
    }
    static CWD: OnceLock<Option<PathBuf>> = OnceLock::new();
    CWD.get_or_init(|| {
        std::env::current_dir()
            .ok()
            .and_then(|d| std::fs::canonicalize(d).ok())
    })
    .clone()
}

/// The first epoch read of each log, by its canonical path.
fn epochs() -> &'static Mutex<HashMap<PathBuf, u64>> {
    static EPOCHS: OnceLock<Mutex<HashMap<PathBuf, u64>>> = OnceLock::new();
    EPOCHS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The key a log is recorded under: its canonical path, else the path as
/// given.
fn key(state_path: &Path) -> PathBuf {
    std::fs::canonicalize(state_path).unwrap_or_else(|_| state_path.to_path_buf())
}

/// Record `epoch` as the one this process read of the log at `state_path`,
/// unless it already read one.
pub fn note_read(state_path: &Path, epoch: u64) {
    epochs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(key(state_path))
        .or_insert(epoch);
}

/// The epoch this process first read of the log at `state_path`, recording
/// `current` as that read when there was none.
fn seen_or(state_path: &Path, current: u64) -> u64 {
    *epochs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(key(state_path))
        .or_insert(current)
}

/// Refuse a write of session `name` to the log at `state_path`, whose header
/// line is `line`, when a whole-binding move fenced it off from this process
/// ([`rebind_fence::check_whole`]). A header with no whole-binding move
/// passes without a lookup.
pub fn check(name: &str, state_path: &Path, line: &str) -> Result<(), SessionRebound> {
    let fence = rebind_fence::Fence::of_header_line(line);
    if fence.whole_epoch == 0 {
        return Ok(());
    }
    let seen = seen_or(state_path, fence.epoch);
    rebind_fence::check_whole(name, line, cwd().as_deref(), Some(seen))
}

/// [`check`] for a write that doesn't read the log itself (a context key):
/// the header line is read from `state_path`, and a session with no log
/// there passes.
pub fn check_log(name: &str, state_path: &Path) -> Result<(), SessionRebound> {
    use std::io::BufRead as _;
    let Ok(file) = std::fs::File::open(state_path) else {
        return Ok(());
    };
    let mut line = String::new();
    if std::io::BufReader::new(file).read_line(&mut line).is_err() {
        return Ok(());
    }
    check(name, state_path, &line)
}

#[cfg(test)]
thread_local! {
    static TEST_CWD: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Run `f` as though this process ran in `cwd`, on this thread only, since
/// a unit test can't change the process's directory under its neighbours.
#[cfg(test)]
pub(crate) fn with_cwd<T>(cwd: &Path, f: impl FnOnce() -> T) -> T {
    let before = TEST_CWD.with(|c| c.replace(Some(cwd.to_path_buf())));
    let out = f();
    TEST_CWD.with(|c| *c.borrow_mut() = before);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moved_line(epoch: u64) -> String {
        format!(
            r#"{{"workflow":"wf","execution_dir":"/ws/new","origin":{{"anchor":"/ws/new","store":{{"kind":"local","base":"/s"}},"anchor_epoch":{epoch},"whole_epoch":{epoch},"retired_anchors":["/ws/old"]}}}}"#
        )
    }

    #[test]
    fn a_later_read_doesnt_replace_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("wf.state.jsonl");
        note_read(&log, 1);
        note_read(&log, 2);
        let refused = with_cwd(Path::new("/ws/new"), || check("wf", &log, &moved_line(2)));
        assert!(!refused.unwrap_err().from_retired);
    }

    #[test]
    fn the_mover_records_the_epoch_it_wrote_over_the_one_it_read() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("wf.state.jsonl");
        note_read(&log, 1);
        note_mover(&log, 2);
        // The mover's own later writes pass at the epoch it committed.
        assert!(with_cwd(Path::new("/ws/new"), || check("wf", &log, &moved_line(2))).is_ok());
        assert_eq!(recorded(&log), Some(2));
    }

    fn recorded(log: &Path) -> Option<u64> {
        epochs()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key(log))
            .copied()
    }
}
