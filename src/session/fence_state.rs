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
//! The cloud backend records its reads here too
//! (`CloudBackend::note_epoch_read`), so both backends measure the
//! whole-binding rule against the same first read. It keeps a second
//! record, refreshed on every read, for the plain rebind's prefix rule
//! alone. A move records the epoch it committed with [`note_mover`],
//! replacing its own first read: that one call is the only overwrite.

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

/// Record `epoch` as this process's epoch for the log at `state_path`,
/// replacing the one it read. Only a whole-binding move calls this, once its
/// commit wrote `epoch`: the move read the session before it bumped the
/// epoch, and its own later writes (the held-by link, the carried keys) must
/// not be refused by the fence it just raised.
///
/// It also marks this process as the log's mover at `epoch`, which lifts
/// the path rule for it while the header's `whole_epoch` is that epoch: a
/// move run from the directory it retires still finishes its later parts.
/// A later move by another process raises `whole_epoch` and the lift ends.
pub fn note_mover(state_path: &Path, epoch: u64) {
    let key = key(state_path);
    epochs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key.clone(), epoch);
    moved()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, epoch);
}

/// Lift the path rule for this process's writes of the log at
/// `state_path` while its header's `whole_epoch` is `whole_epoch`, the one
/// it holds before a move commits. Only a whole-binding move calls this,
/// holding `tick.lock` exclusive, just before its commit: a move back into
/// a directory an earlier move retired runs from there, and its own commit
/// must not be refused by the fence it is about to lower. The epoch half
/// still applies.
///
/// The lift lasts as long as the returned [`TakingOver`]: dropping it ends
/// the lift unless [`note_mover`] has replaced it with the epoch the commit
/// wrote. So a commit that fails or is refused leaves no lift behind, even
/// in a process that goes on running.
#[must_use = "the lift ends when the guard is dropped"]
pub fn note_taking_over(state_path: &Path, whole_epoch: u64) -> TakingOver {
    let key = key(state_path);
    moved()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key.clone(), whole_epoch);
    TakingOver { key, whole_epoch }
}

/// The lift [`note_taking_over`] put up, ended when this is dropped.
pub struct TakingOver {
    key: PathBuf,
    whole_epoch: u64,
}

impl Drop for TakingOver {
    fn drop(&mut self) {
        let mut moved = moved().lock().unwrap_or_else(|e| e.into_inner());
        // Only the pre-commit lift. The movers drop this before their own
        // note_mover, which then records the epoch the commit wrote. One
        // process makes at most one whole move and takes its lift before
        // that note_mover, so this never meets an earlier record at the
        // same epoch; if it ever did, removing it would refuse the write,
        // never admit one.
        if moved.get(&self.key) == Some(&self.whole_epoch) {
            moved.remove(&self.key);
        }
    }
}

/// The whole-binding epoch this process committed of each log, by its
/// canonical path: the logs it moved.
fn moved() -> &'static Mutex<HashMap<PathBuf, u64>> {
    static MOVED: OnceLock<Mutex<HashMap<PathBuf, u64>>> = OnceLock::new();
    MOVED.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Whether this process moved the log at `state_path` to `whole_epoch`.
fn moved_at(state_path: &Path, whole_epoch: u64) -> bool {
    moved()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key(state_path))
        == Some(&whole_epoch)
}

/// Whether the path rule is lifted for this process's writes of the log at
/// `state_path`, whose header line is `line`: it made the whole-binding move
/// the header records ([`note_mover`]), or is making the next one
/// ([`note_taking_over`]). The cloud backend asks this of the plain rebind's
/// prefix rule, the same path rule by prefix, so the one lift covers both.
pub fn is_mover(state_path: &Path, line: &str) -> bool {
    let fence = rebind_fence::Fence::of_header_line(line);
    fence.whole_epoch > 0 && moved_at(state_path, fence.whole_epoch)
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
/// passes without a lookup. The process that made the move the header
/// records is asked only the epoch half ([`note_mover`]).
pub fn check(name: &str, state_path: &Path, line: &str) -> Result<(), SessionRebound> {
    let fence = rebind_fence::Fence::of_header_line(line);
    if fence.whole_epoch == 0 {
        return Ok(());
    }
    let seen = seen_or(state_path, fence.epoch);
    let cwd = if moved_at(state_path, fence.whole_epoch) {
        None
    } else {
        cwd()
    };
    rebind_fence::check_whole(name, line, cwd.as_deref(), Some(seen))
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

    #[test]
    fn the_mover_passes_from_the_directory_it_retired_until_another_move() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("wf.state.jsonl");
        note_read(&log, 2);
        let from_old = || with_cwd(Path::new("/ws/old"), || check("wf", &log, &moved_line(2)));
        // Before it records the move, the path rule refuses it like anyone.
        assert!(from_old().unwrap_err().from_retired);
        note_mover(&log, 2);
        assert!(from_old().is_ok());
        // A later move, by another process, ends the lift.
        let refused = with_cwd(Path::new("/ws/old"), || check("wf", &log, &moved_line(3)));
        assert!(refused.is_err());
    }

    #[test]
    fn the_movers_own_process_is_refused_once_another_move_raises_whole_epoch() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("wf.state.jsonl");
        note_read(&log, 1);
        note_mover(&log, 2);
        let at = |cwd: &str, epoch: u64| {
            with_cwd(Path::new(cwd), || check("wf", &log, &moved_line(epoch)))
        };
        assert!(at("/ws/old", 2).is_ok());
        assert!(at("/ws/new", 2).is_ok());
        // Another process moved the session on to 3: the mover holds 2, so
        // the epoch half refuses it from anywhere, and the path half no
        // longer lets it past the retired directory.
        assert!(!at("/ws/new", 3).unwrap_err().from_retired);
        assert!(at("/ws/old", 3).is_err());
        assert!(!is_mover(&log, &moved_line(3)));
    }

    #[test]
    fn a_lift_taken_before_a_commit_that_never_lands_ends_with_its_guard() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("wf.state.jsonl");
        note_read(&log, 2);
        let from_old = || with_cwd(Path::new("/ws/old"), || check("wf", &log, &moved_line(2)));
        assert!(from_old().unwrap_err().from_retired);

        let lift = note_taking_over(&log, 2);
        assert!(from_old().is_ok());
        assert!(is_mover(&log, &moved_line(2)));
        // The commit failed or was refused: nothing called note_mover, and
        // the process goes on running without the lift.
        drop(lift);
        assert!(from_old().unwrap_err().from_retired);
        assert!(!is_mover(&log, &moved_line(2)));
    }

    #[test]
    fn a_lift_taken_before_a_commit_that_lands_stays_at_the_epoch_it_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("wf.state.jsonl");
        note_read(&log, 2);
        let lift = note_taking_over(&log, 2);
        note_mover(&log, 3);
        drop(lift);
        assert!(is_mover(&log, &moved_line(3)));
        assert!(with_cwd(Path::new("/ws/old"), || check("wf", &log, &moved_line(3))).is_ok());
    }

    fn recorded(log: &Path) -> Option<u64> {
        epochs()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key(log))
            .copied()
    }
}
