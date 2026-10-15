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
//! to the local store only and is recorded here. The batch is flushed once,
//! by [`crate::session::cloud::CloudBackend::flush_pending`], before the tick
//! returns (and at every other point where another process or host could
//! read the bucket: see that method's callers).
//!
//! The journal lives in the session's local directory, because the writers
//! are separate processes:
//!
//! - `cloud-batch.jsonl`: one line per deferred context write, in order.
//!   Its presence means a batch is open: the session's local copy holds
//!   writes the bucket lacks. A batch with only event appends is an empty
//!   file.
//! - `cloud-batch.lock`: an advisory lock every writer and the flush take,
//!   so a flush never pushes half of a write. It outlives the batch; a
//!   lock file that comes and goes would let two processes lock two inodes.
//!   The lock is `flock`, so the guarantee is Unix's; `koto next`, the only
//!   thing that runs commands, is Unix-only too.
//!
//! The contract the cloud backend keeps, for anything that writes a session
//! or pushes it:
//!
//! - A write from a tick's command to the tick's session (a context add or
//!   remove, an event append, a header rewrite) opens the batch and records
//!   itself under the lock; it never pushes.
//! - Every other push of a session flushes its open batch first and pushes
//!   nothing of its own while the batch is open: a state file or manifest
//!   pushed alone would name keys the bucket doesn't hold. `ensure_pushed`
//!   flushes from any process, a tick's command included, since its caller
//!   needs the bucket to hold everything.
//! - A read from outside the tick flushes the batch before it reads the
//!   bucket, so it can come before the tick's own flush; a read from a
//!   tick's command answers a key the batch wrote from the local copy.
//! - A pull never writes over an open batch.
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

/// File name of the lock inside a session's local directory.
pub const LOCK_FILE: &str = "cloud-batch.lock";

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

    /// Whether a batch is open. Read without the lock: a caller that acts on
    /// the answer takes the lock and asks again.
    pub fn is_open(&self) -> bool {
        self.path().exists()
    }

    /// Take the batch lock, blocking until it is free. `None` when the
    /// session has no local directory to hold it.
    pub fn lock(&self) -> anyhow::Result<Option<BatchLock>> {
        if !self.dir.is_dir() {
            return Ok(None);
        }
        BatchLock::acquire(&self.dir.join(LOCK_FILE)).map(Some)
    }

    /// Open the batch: create the journal empty. A no-op when it is open.
    pub fn open(&self, _lock: &BatchLock) -> anyhow::Result<()> {
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path())
            .with_context(|| format!("opening {}", self.path().display()))?;
        Ok(())
    }

    /// Append one write to the open batch.
    pub fn record(&self, _lock: &BatchLock, op: &Op) -> anyhow::Result<()> {
        let mut line = serde_json::to_string(op)?;
        line.push('\n');
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path())
            .with_context(|| format!("opening {}", self.path().display()))?;
        file.write_all(line.as_bytes())
            .with_context(|| format!("writing {}", self.path().display()))?;
        Ok(())
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
    /// replaced wholesale (`koto session resolve`).
    pub fn close(&self, _lock: &BatchLock) -> anyhow::Result<()> {
        match fs::remove_file(self.path()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("removing {}", self.path().display())),
        }
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
    fn a_session_with_no_directory_has_no_lock() {
        let dir = tempfile::TempDir::new().unwrap();
        let journal = Journal::new(&dir.path().join("missing"));
        assert!(journal.lock().unwrap().is_none());
    }
}
