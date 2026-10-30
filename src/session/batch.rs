//! The pending-writes journal behind deferred cloud writes.
//!
//! A command a `koto next` tick runs as its default action, or as a gate, is
//! its own process, and each `koto context add` it makes is another. On the
//! cloud backend every one of those used to pull the state file and push the
//! content, the manifest, `version.json` and the state file again: about
//! seven sequential bucket requests per call (koto#376). A coordinator-like
//! action making eighteen calls ran past the action cap on the round trips
//! alone.
//!
//! So a cloud write made from inside a tick, on the tick's own session, goes
//! to the local store only and is recorded here. That includes the tick's
//! own writes, its event appends, header rewrites and the evidence it copies
//! into context, which used to push the whole state file once per event
//! (koto#548). The batch is flushed once,
//! by [`crate::session::cloud::CloudBackend::flush_pending`], before the tick
//! returns (and at every other point where another process or host could
//! read the bucket: see that method's callers).
//!
//! A caller outside a tick gets the same batch by opening a write window
//! (koto#443): every koto process it runs with [`WINDOW_ENV`] naming a
//! session defers its writes to that session as a tick's commands do, and
//! `koto session flush <name>` closes the window with one push. A process
//! finds the window by inheriting the variable, as a tick's commands
//! inherit the tick marker, so a caller killed with its window open takes
//! the window with it: the journal stays, and the next koto command on the
//! session without the variable flushes it, as it does a killed tick's.
//!
//! The journal lives in the session's local directory, because the writers
//! are separate processes:
//!
//! - `cloud-batch.jsonl`: one line per deferred context write, in order.
//!   Its presence means a batch is open: the session's local copy holds
//!   writes the bucket lacks. A batch with only event appends is an empty
//!   file.
//! - `cloud-batch.opening`: the batch's first write, part way through
//!   opening it, holding the exact bytes of the batch-open marker it puts in
//!   the bucket. Written before the marker goes up and removed only after
//!   the journal exists, so a marker in the bucket always has this file or
//!   the journal beside it on the host that wrote it. Found with no
//!   journal, it is what a process killed between the two steps (or one
//!   whose marker PUT failed) left: the marker may be in the bucket with
//!   nothing locally to flush or close it, and the next command clears it
//!   (koto#381), but only when the bucket's marker has these bytes, so a
//!   marker another writer put there since is left alone.
//! - `cloud-batch.lock`: an advisory lock every writer and the flush take,
//!   so a flush never pushes half of a write. It outlives the batch; a
//!   lock file that comes and goes would let two processes lock two inodes.
//!   The lock is `flock`, so the guarantee is Unix's; `koto next`, the only
//!   thing that runs commands, is Unix-only too. A process opening a batch
//!   holds it from the opening file to the journal, and the kernel releases
//!   it when the process dies, so holding it is how a command knows no live
//!   process owns an opening file it found. Nothing takes it twice: a
//!   second `flock` on a new descriptor in the same process would wait on
//!   the first forever.
//!
//! The journal and the opening file are fsynced, and the directory with
//! them, before a deferred write returns: the marker says the host holds
//! writes the bucket lacks, and a crash must not drop the record of them.
//!
//! The contract the cloud backend keeps, for anything that writes a session
//! or pushes it:
//!
//! - A write from a tick, or a tick's command, to the tick's session, or
//!   from inside a write window on the session (a context add or remove, an
//!   event append, a header rewrite), opens the batch and records itself
//!   under the lock; it never pushes.
//! - The tick pulls its own session once, and that pull flushes a batch an
//!   earlier process left open. After it the tick never pulls its session
//!   again, so nothing replaces the tick's writes with the bucket's older
//!   copy; it pushes the batch once on its way out, however it exits.
//! - Every other push of a session flushes its open batch first and pushes
//!   nothing of its own while the batch is open: a state file or manifest
//!   pushed alone would name keys the bucket doesn't hold. `ensure_pushed`
//!   flushes from any process, a tick's command included, since its caller
//!   needs the bucket to hold everything.
//! - A read from a process that doesn't defer flushes the batch before it
//!   reads the bucket, so it can come before the tick's own flush or the
//!   window's close; a read from the tick or a tick's command, or from
//!   inside the window, answers a key the batch wrote from the local copy.
//! - A pull never writes over an open batch.
//! - A pull, push, flush or resolve that finds a batch begun and never
//!   opened deletes its marker, when the bucket's marker is the one the
//!   opening file holds, and then the opening file, under the lock.
//! - A flush counts an open batch as a change on this host, so a remote
//!   version past the base is a conflict even when the batch's first write
//!   couldn't save its increment.
//! - `koto session resolve` settles a batch by replacing one side with the
//!   other, and closes it only once that succeeded.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// File name of the journal inside a session's local directory.
pub const JOURNAL_FILE: &str = "cloud-batch.jsonl";

/// File name of a batch being opened inside a session's local directory.
pub const OPENING_FILE: &str = "cloud-batch.opening";

/// File name of the lock inside a session's local directory.
pub const LOCK_FILE: &str = "cloud-batch.lock";

/// Environment variable naming the session a caller's write window is open
/// on. A value naming no session opens no window.
pub const WINDOW_ENV: &str = "KOTO_WRITE_WINDOW";

/// Whether this process writes `session` inside a write window: the
/// variable names `session` and this process isn't a `koto next` tick,
/// which pushes its own session's batch whatever its environment says.
pub fn in_window_of(session: &str) -> bool {
    !crate::engine::reentrancy::is_tick_process()
        && std::env::var(WINDOW_ENV).is_ok_and(|value| value == session)
}

/// One deferred context write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// The key was written locally; its content and the manifest are due.
    Add { key: String },
    /// The key was removed locally; its object is due for deletion.
    Remove { key: String },
}

/// The keys an open batch wrote. The flush reads each one's local copy to
/// decide what it owes: content to upload when the local manifest has the
/// key, an object to delete when it doesn't. Deciding from the local copy
/// rather than from the recorded operation keeps the flush right when a
/// writer stopped between recording a write and making it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pending {
    pub keys: BTreeSet<String>,
}

impl Pending {
    /// Whether the batch has a write for `key`: its local copy, present or
    /// absent, is the answer, and the bucket's is stale.
    pub fn touches(&self, key: &str) -> bool {
        self.keys.contains(key)
    }
}

/// The journal of one session's local directory.
pub struct Journal {
    dir: PathBuf,
}

impl Journal {
    pub fn new(session_dir: &Path) -> Self {
        Self {
            dir: session_dir.to_path_buf(),
        }
    }

    fn path(&self) -> PathBuf {
        self.dir.join(JOURNAL_FILE)
    }

    fn opening_path(&self) -> PathBuf {
        self.dir.join(OPENING_FILE)
    }

    /// Whether a batch is open. Read without the lock: a caller that acts on
    /// the answer takes the lock and asks again.
    pub fn is_open(&self) -> bool {
        self.path().exists()
    }

    /// Whether a batch was begun and never opened: the opening file with no
    /// journal. Read without the lock, like [`Journal::is_open`]; under the
    /// lock, no process is still opening it (the one that began it died, or
    /// gave up when its marker PUT failed).
    pub fn is_opening(&self) -> bool {
        self.opening_path().exists() && !self.is_open()
    }

    /// The marker bytes a batch begun and never opened was putting in the
    /// bucket, as [`Journal::begin`] wrote them: `None` with no opening file.
    /// Bytes cut short by a crash match no marker, which is right: the PUT
    /// comes only after they are on disk.
    pub fn opening_marker(&self) -> anyhow::Result<Option<Vec<u8>>> {
        match fs::read(self.opening_path()) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {}", self.opening_path().display())),
        }
    }

    /// Take the batch lock, blocking until it is free. `None` when the
    /// session has no local directory to hold it.
    pub fn lock(&self) -> anyhow::Result<Option<BatchLock>> {
        if !self.dir.is_dir() {
            return Ok(None);
        }
        BatchLock::acquire(&self.dir.join(LOCK_FILE)).map(Some)
    }

    /// Begin opening the batch, before its marker goes to the bucket: write
    /// the opening file, holding the marker's bytes, durably. The caller
    /// holds the same lock through [`Journal::open`] or
    /// [`Journal::abandon`], so no other process sees the batch half open.
    pub fn begin(&self, _lock: &BatchLock, marker: &[u8]) -> anyhow::Result<()> {
        let path = self.opening_path();
        let mut file =
            fs::File::create(&path).with_context(|| format!("creating {}", path.display()))?;
        file.write_all(marker)
            .with_context(|| format!("writing {}", path.display()))?;
        sync_file(&file, &path)?;
        sync_dir(&self.dir)
    }

    /// Open the batch, once its marker is in the bucket: create the journal
    /// empty, then remove the opening file. In that order, so a crash
    /// between them leaves an open batch (which [`Journal::close`] clears of
    /// both files) rather than neither file beside a marker. A no-op when
    /// the batch is open.
    pub fn open(&self, _lock: &BatchLock) -> anyhow::Result<()> {
        if self.is_open() {
            return Ok(());
        }
        let path = self.path();
        let file =
            fs::File::create(&path).with_context(|| format!("creating {}", path.display()))?;
        sync_file(&file, &path)?;
        remove_if_present(&self.opening_path())?;
        sync_dir(&self.dir)
    }

    /// Append one write to the open batch, durably.
    pub fn record(&self, _lock: &BatchLock, op: &Op) -> anyhow::Result<()> {
        let mut line = serde_json::to_string(op)?;
        line.push('\n');
        let path = self.path();
        let created = !path.exists();
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        file.write_all(line.as_bytes())
            .with_context(|| format!("writing {}", path.display()))?;
        sync_file(&file, &path)?;
        if created {
            sync_dir(&self.dir)?;
        }
        Ok(())
    }

    /// Drop a batch that was begun and never opened, once its marker is
    /// gone from the bucket.
    pub fn abandon(&self, _lock: &BatchLock) -> anyhow::Result<()> {
        remove_if_present(&self.opening_path())
    }

    /// What the open batch owes, or `None` when no batch is open. A line
    /// that doesn't parse is an error: guessing would push the wrong keys.
    pub fn pending(&self) -> anyhow::Result<Option<Pending>> {
        let text = match fs::read_to_string(self.path()) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {}", self.path().display())),
        };
        let mut pending = Pending::default();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let op: Op = serde_json::from_str(line)
                .with_context(|| format!("parsing {}", self.path().display()))?;
            let (Op::Add { key } | Op::Remove { key }) = op;
            pending.keys.insert(key);
        }
        Ok(Some(pending))
    }

    /// Close the batch: everything it owed is in the bucket, or was
    /// replaced wholesale (`koto session resolve`). Removes an opening file
    /// too, which an open crashed part way through can leave beside the
    /// journal.
    pub fn close(&self, _lock: &BatchLock) -> anyhow::Result<()> {
        remove_if_present(&self.path())?;
        remove_if_present(&self.opening_path())
    }
}

fn remove_if_present(path: &Path) -> anyhow::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

/// Flush a file's contents and metadata to disk.
fn sync_file(file: &fs::File, path: &Path) -> anyhow::Result<()> {
    file.sync_all()
        .with_context(|| format!("syncing {}", path.display()))?;
    #[cfg(test)]
    synced::note(path);
    Ok(())
}

/// Flush a directory's entries to disk, so a file created in it or renamed
/// into it survives a crash. Unix only: elsewhere a directory can't be
/// opened as a file, and `koto next`, which defers writes, is Unix-only.
fn sync_dir(dir: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    fs::File::open(dir)
        .and_then(|d| d.sync_all())
        .with_context(|| format!("syncing {}", dir.display()))?;
    #[cfg(test)]
    synced::note(dir);
    Ok(())
}

/// The paths this thread synced, in order, so a test can see what reached
/// the disk before a write returned.
#[cfg(test)]
pub(crate) mod synced {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    thread_local! {
        static LOG: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn note(path: &Path) {
        LOG.with(|log| log.borrow_mut().push(path.to_path_buf()));
    }

    /// The paths synced since the last call.
    pub(crate) fn take() -> Vec<PathBuf> {
        LOG.with(|log| std::mem::take(&mut *log.borrow_mut()))
    }
}

/// An exclusive advisory lock on a session's batch, released on drop.
pub struct BatchLock {
    _file: fs::File,
}

impl BatchLock {
    #[cfg(unix)]
    fn acquire(path: &Path) -> anyhow::Result<Self> {
        use std::os::unix::io::AsRawFd;

        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .with_context(|| format!("opening {}", path.display()))?;
        // SAFETY: the descriptor belongs to `file`, which outlives the call;
        // the lock is released when `file` is closed on drop.
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if ret != 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("locking {}", path.display()));
        }
        Ok(Self { _file: file })
    }

    #[cfg(not(unix))]
    fn acquire(path: &Path) -> anyhow::Result<Self> {
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .with_context(|| format!("opening {}", path.display()))?;
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(key: &str) -> Op {
        Op::Add {
            key: key.to_string(),
        }
    }

    fn remove(key: &str) -> Op {
        Op::Remove {
            key: key.to_string(),
        }
    }

    #[test]
    fn no_journal_is_no_batch() {
        let dir = tempfile::TempDir::new().unwrap();
        let journal = Journal::new(dir.path());
        assert!(!journal.is_open());
        assert_eq!(journal.pending().unwrap(), None);
    }

    #[test]
    fn an_opened_batch_with_no_writes_owes_nothing_but_is_open() {
        let dir = tempfile::TempDir::new().unwrap();
        let journal = Journal::new(dir.path());
        let lock = journal.lock().unwrap().unwrap();
        journal.open(&lock).unwrap();
        assert!(journal.is_open());
        assert_eq!(journal.pending().unwrap(), Some(Pending::default()));
    }

    #[test]
    fn every_key_written_is_pending_once() {
        let dir = tempfile::TempDir::new().unwrap();
        let journal = Journal::new(dir.path());
        let lock = journal.lock().unwrap().unwrap();
        for op in [add("a"), remove("a"), add("b"), remove("c"), add("c")] {
            journal.record(&lock, &op).unwrap();
        }
        let pending = journal.pending().unwrap().unwrap();
        assert_eq!(pending.keys.iter().collect::<Vec<_>>(), ["a", "b", "c"]);
        assert!(pending.touches("a"));
        assert!(!pending.touches("d"));
    }

    #[test]
    fn closing_ends_the_batch_and_keeps_the_lock_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let journal = Journal::new(dir.path());
        let lock = journal.lock().unwrap().unwrap();
        journal.record(&lock, &add("a")).unwrap();
        journal.close(&lock).unwrap();
        journal.close(&lock).unwrap();
        assert!(!journal.is_open());
        assert!(dir.path().join(LOCK_FILE).exists());
    }

    #[test]
    fn a_garbled_journal_is_an_error_not_a_guess() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(dir.path().join(JOURNAL_FILE), "{\"op\":\"add\"}\n").unwrap();
        assert!(Journal::new(dir.path()).pending().is_err());
    }

    #[test]
    fn a_begun_batch_is_opening_until_it_is_opened_or_abandoned() {
        let dir = tempfile::TempDir::new().unwrap();
        let journal = Journal::new(dir.path());
        let lock = journal.lock().unwrap().unwrap();
        journal.begin(&lock, b"marker").unwrap();
        assert!(journal.is_opening());
        assert!(!journal.is_open());
        assert_eq!(journal.opening_marker().unwrap().unwrap(), b"marker");

        journal.open(&lock).unwrap();
        assert!(journal.is_open());
        assert!(!journal.is_opening());
        assert_eq!(journal.opening_marker().unwrap(), None);
        assert_eq!(journal.pending().unwrap(), Some(Pending::default()));

        journal.close(&lock).unwrap();
        journal.begin(&lock, b"marker").unwrap();
        journal.abandon(&lock).unwrap();
        journal.abandon(&lock).unwrap();
        assert!(!journal.is_opening());
        assert!(!journal.is_open());
    }

    /// An open that stopped between creating the journal and removing the
    /// opening file is an open batch, and closing it removes both.
    #[test]
    fn closing_clears_an_opening_file_left_beside_the_journal() {
        let dir = tempfile::TempDir::new().unwrap();
        let journal = Journal::new(dir.path());
        let lock = journal.lock().unwrap().unwrap();
        journal.begin(&lock, b"marker").unwrap();
        fs::write(dir.path().join(JOURNAL_FILE), "").unwrap();
        assert!(journal.is_open());
        assert!(!journal.is_opening());
        journal.close(&lock).unwrap();
        assert!(!dir.path().join(OPENING_FILE).exists());
        assert!(!journal.is_open());
    }

    /// The opening file and the journal reach the disk, with the directory
    /// entries naming them, before the call that wrote them returns
    /// (koto#381).
    #[test]
    fn every_step_is_synced_with_its_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let journal = Journal::new(dir.path());
        let lock = journal.lock().unwrap().unwrap();
        let file = dir.path().join(JOURNAL_FILE);
        let at = dir.path().to_path_buf();

        synced::take();
        journal.begin(&lock, b"marker").unwrap();
        assert_eq!(
            synced::take(),
            vec![dir.path().join(OPENING_FILE), at.clone()]
        );
        journal.open(&lock).unwrap();
        assert_eq!(synced::take(), vec![file.clone(), at.clone()]);
        journal.record(&lock, &add("a")).unwrap();
        assert_eq!(synced::take(), vec![file.clone()]);

        // A journal created by its first record syncs its directory too.
        journal.close(&lock).unwrap();
        journal.record(&lock, &add("b")).unwrap();
        assert_eq!(synced::take(), vec![file, at]);
    }

    #[test]
    fn a_session_with_no_directory_has_no_lock() {
        let dir = tempfile::TempDir::new().unwrap();
        let journal = Journal::new(&dir.path().join("missing"));
        assert!(journal.lock().unwrap().is_none());
    }
}
