//! The on-disk pool under `<koto root>/pools/<name>/`.
//!
//! ```text
//! slot-<i>.lock   flock target; while held, its content is the holder's JSON
//! queue.lock      short flock around queue edits, waited for at most 2 s
//! queue.json      the waiters, replaced by write-to-temp then rename
//! ```
//!
//! A slot is held for as long as any descriptor of its open file holds the
//! `flock`, and the kernel drops the lock when the last one closes, so a
//! holder that dies, `kill -9` included, frees its slot without anyone
//! removing a file (DESIGN-host-resource-pools.md Decision 1). Nothing here
//! ever deletes a slot file.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// The most a holder record may be.
pub const MAX_HOLDER_BYTES: u64 = 4 * 1024;
/// The most `queue.json` may be.
pub const MAX_QUEUE_BYTES: u64 = 256 * 1024;
/// How long a queue edit waits for `queue.lock`.
pub const QUEUE_LOCK_WAIT: Duration = Duration::from_secs(2);
/// The most characters of a holder's session, workspace or state a report
/// shows.
pub const MAX_SHOWN_CHARS: usize = 128;

/// Why a pool's directory can't be used (`pool_dir_unavailable`).
#[derive(Debug)]
pub struct DirUnavailable {
    pub path: PathBuf,
    pub reason: String,
}

impl std::fmt::Display for DirUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.reason)
    }
}

/// Create `dir` with mode 0700 if it is missing, then refuse it unless it is
/// a real directory (not a symlink), owned by this user and writable by
/// nobody else.
pub fn ensure_private_dir(dir: &Path) -> Result<(), DirUnavailable> {
    let unavailable = |reason: String| DirUnavailable {
        path: dir.to_path_buf(),
        reason,
    };
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(unavailable(format!("can't be created: {e}"))),
    }
    let meta = std::fs::symlink_metadata(dir).map_err(|e| unavailable(e.to_string()))?;
    if !meta.file_type().is_dir() {
        return Err(unavailable("is not a directory".to_string()));
    }
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    if meta.uid() != uid {
        return Err(unavailable("is not owned by the current user".to_string()));
    }
    if meta.permissions().mode() & 0o022 != 0 {
        return Err(unavailable("is writable by group or others".to_string()));
    }
    Ok(())
}

/// The directory of pool `name` under `root`, created when missing. The
/// caller has checked `name` against the pool-name grammar.
pub fn pool_dir(root: &Path, name: &str) -> Result<PathBuf, DirUnavailable> {
    debug_assert!(crate::template::types::is_valid_pool_name(name));
    let pools = root.join("pools");
    // The root is koto's own and checked where it is made; only the
    // directories pools add are held to the private-directory rule.
    crate::koto_home::ensure_private_root(root).map_err(|e| DirUnavailable {
        path: root.to_path_buf(),
        reason: format!("can't be created: {e}"),
    })?;
    ensure_private_dir(&pools)?;
    let dir = pools.join(name);
    ensure_private_dir(&dir)?;
    Ok(dir)
}

/// Open (creating 0600) a file in a pool directory, refusing a symlink.
fn open_file(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

/// Try to `flock` `file` exclusively without waiting.
pub fn try_flock(file: &File) -> bool {
    // SAFETY: flock on a descriptor `file` owns for the call.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

/// Who holds a slot, as written into the slot file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holder {
    pub session: String,
    /// The workspace directory's name, not its path.
    pub workspace: String,
    pub state: String,
    pub pid: u32,
    /// Unix seconds when the slot was taken.
    pub since: u64,
}

/// A held slot. Dropping it closes koto's descriptor; the slot frees once
/// every process that inherited one has exited too.
#[derive(Debug)]
pub struct Slot {
    file: File,
    pub index: usize,
}

impl Slot {
    /// The slot file's descriptor, for handing it to a background action's
    /// runner by number.
    pub fn raw_fd(&self) -> std::os::fd::RawFd {
        self.file.as_raw_fd()
    }

    /// Let processes koto spawns from now on inherit this slot's descriptor,
    /// so the slot stays held while the command, or anything it leaves
    /// running, is alive (PRD R10). Call it just before spawning the command
    /// and drop the slot right after, so nothing else koto runs inherits it.
    pub fn inherit_into_children(&self) -> std::io::Result<()> {
        let fd = self.file.as_raw_fd();
        // SAFETY: fcntl on a descriptor `self` owns.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: as above.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

/// The slot files present in `dir`, as `(index, path)`, in index order.
fn slot_files(dir: &Path) -> Vec<(usize, PathBuf)> {
    let mut out: Vec<(usize, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            let index: usize = name
                .strip_prefix("slot-")?
                .strip_suffix(".lock")?
                .parse()
                .ok()?;
            Some((index, entry.path()))
        })
        .collect();
    out.sort();
    out
}

/// Whether the slot file at `path` is held by anyone, this process
/// included: a non-blocking try on a fresh descriptor, released at once. A
/// try that collides with another process's probe reads as held.
pub fn probe_held(path: &Path) -> bool {
    match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        // Closing the descriptor releases a lock the try took.
        Ok(file) => !try_flock(&file),
        Err(_) => false,
    }
}

/// Read a held slot's holder record, if it parses.
fn read_holder(path: &Path) -> Option<Holder> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .ok()?;
    let mut text = String::new();
    file.take(MAX_HOLDER_BYTES).read_to_string(&mut text).ok()?;
    serde_json::from_str(&text).ok()
}

/// The holders of every held slot in `dir`, of any index, with the index.
pub fn holders(dir: &Path) -> Vec<(usize, Option<Holder>)> {
    slot_files(dir)
        .into_iter()
        .filter(|(_, path)| probe_held(path))
        .map(|(index, path)| (index, read_holder(&path)))
        .collect()
}

/// Take a free slot of a pool of `size`, writing `holder` into it.
///
/// Every slot file present is probed first, whatever its index, so a pool
/// shrunk below its holders admits nobody until fewer than `size` are held
/// (PRD R8). Returns `None` when no slot is free.
pub fn try_acquire(dir: &Path, size: u32, holder: &Holder) -> std::io::Result<Option<Slot>> {
    let held = slot_files(dir)
        .iter()
        .filter(|(_, path)| probe_held(path))
        .count();
    if held >= size as usize {
        return Ok(None);
    }
    for index in 0..size as usize {
        let path = dir.join(format!("slot-{index}.lock"));
        let mut file = open_file(&path)?;
        if !try_flock(&file) {
            continue;
        }
        let mut record = serde_json::to_vec(holder).map_err(std::io::Error::other)?;
        record.truncate(MAX_HOLDER_BYTES as usize);
        file.set_len(0)?;
        file.rewind()?;
        file.write_all(&record)?;
        return Ok(Some(Slot { file, index }));
    }
    Ok(None)
}

/// One waiter in `queue.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEntry {
    pub ticket: u64,
    pub session: String,
    pub workspace: String,
    /// Unix seconds the session joined the queue.
    pub joined_at: u64,
    /// Unix seconds the session last ticked while waiting.
    pub last_seen: u64,
}

/// `queue.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queue {
    #[serde(default)]
    pub next_ticket: u64,
    #[serde(default)]
    pub entries: Vec<QueueEntry>,
}

impl Queue {
    /// The index of `session`'s entry from `workspace`.
    pub fn position_of(&self, session: &str, workspace: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.session == session && e.workspace == workspace)
    }
}

/// Read `queue.json`; a missing, oversized or unreadable one is an empty
/// queue.
pub fn read_queue(dir: &Path) -> Queue {
    let path = dir.join("queue.json");
    let Ok(file) = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
    else {
        return Queue::default();
    };
    let mut text = String::new();
    if file
        .take(MAX_QUEUE_BYTES + 1)
        .read_to_string(&mut text)
        .is_err()
        || text.len() as u64 > MAX_QUEUE_BYTES
    {
        return Queue::default();
    }
    serde_json::from_str(&text).unwrap_or_default()
}

/// Replace `queue.json` by writing a temporary file and renaming it over.
fn write_queue(dir: &Path, queue: &Queue) -> std::io::Result<()> {
    let tmp = dir.join(format!("queue.json.tmp-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&tmp)?;
    file.write_all(&serde_json::to_vec(queue).map_err(std::io::Error::other)?)?;
    file.sync_all()?;
    std::fs::rename(&tmp, dir.join("queue.json"))
}

/// `queue.lock`, held while the queue is edited.
pub struct QueueLock {
    _file: File,
}

/// Take `queue.lock`, waiting up to [`QUEUE_LOCK_WAIT`]. `None` when it
/// stayed busy; the caller counts that as "not yet".
pub fn lock_queue(dir: &Path) -> std::io::Result<Option<QueueLock>> {
    let file = open_file(&dir.join("queue.lock"))?;
    let started = Instant::now();
    loop {
        if try_flock(&file) {
            return Ok(Some(QueueLock { _file: file }));
        }
        if started.elapsed() >= QUEUE_LOCK_WAIT {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Edit the queue under `queue.lock`: read it, let `edit` change it, and
/// write it back when `edit` says it changed. `None` when the lock stayed
/// busy.
pub fn edit_queue<T>(
    dir: &Path,
    edit: impl FnOnce(&mut Queue) -> (T, bool),
) -> std::io::Result<Option<T>> {
    let Some(_lock) = lock_queue(dir)? else {
        return Ok(None);
    };
    let mut queue = read_queue(dir);
    let (out, changed) = edit(&mut queue);
    if changed {
        write_queue(dir, &queue)?;
    }
    Ok(Some(out))
}

/// `s` with control characters removed and cut to [`MAX_SHOWN_CHARS`], for a
/// value that reaches another session's agent.
pub fn sanitize(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(MAX_SHOWN_CHARS)
        .collect()
}

/// Unix seconds now.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holder(session: &str) -> Holder {
        Holder {
            session: session.to_string(),
            workspace: "ws".to_string(),
            state: "test".to_string(),
            pid: std::process::id(),
            since: now_secs(),
        }
    }

    #[test]
    fn a_size_one_pool_admits_one_holder_until_it_drops() {
        let root = tempfile::tempdir().unwrap();
        let dir = pool_dir(root.path(), "test-suite").unwrap();
        let first = try_acquire(&dir, 1, &holder("a"))
            .unwrap()
            .expect("free slot");
        assert!(try_acquire(&dir, 1, &holder("b")).unwrap().is_none());
        let held = holders(&dir);
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].1.as_ref().unwrap().session, "a");
        drop(first);
        assert!(holders(&dir).is_empty());
        assert!(try_acquire(&dir, 1, &holder("b")).unwrap().is_some());
    }

    #[test]
    fn a_shrunk_pool_admits_nobody_while_its_holders_reach_the_size() {
        let root = tempfile::tempdir().unwrap();
        let dir = pool_dir(root.path(), "p").unwrap();
        let _a = try_acquire(&dir, 2, &holder("a")).unwrap().unwrap();
        let _b = try_acquire(&dir, 2, &holder("b")).unwrap().unwrap();
        drop(_a);
        // slot-1 is still held; at size 1 nobody new is admitted.
        assert!(try_acquire(&dir, 1, &holder("c")).unwrap().is_none());
    }

    #[test]
    fn an_unsafe_directory_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let pools = root.path().join("pools");
        std::fs::create_dir(&pools).unwrap();
        std::fs::set_permissions(&pools, std::fs::Permissions::from_mode(0o777)).unwrap();
        let err = pool_dir(root.path(), "p").unwrap_err();
        assert!(err.reason.contains("writable"), "{err}");
    }

    #[test]
    fn queue_edits_round_trip() {
        let root = tempfile::tempdir().unwrap();
        let dir = pool_dir(root.path(), "p").unwrap();
        edit_queue(&dir, |q| {
            q.entries.push(QueueEntry {
                ticket: 0,
                session: "s".into(),
                workspace: "w".into(),
                joined_at: 1,
                last_seen: 1,
            });
            q.next_ticket = 1;
            ((), true)
        })
        .unwrap()
        .unwrap();
        let q = read_queue(&dir);
        assert_eq!(q.position_of("s", "w"), Some(0));
        assert_eq!(q.position_of("s", "other"), None);
    }

    #[test]
    fn sanitize_strips_control_characters_and_cuts() {
        assert_eq!(sanitize("a\u{1b}[31mb\n"), "a[31mb");
        assert_eq!(sanitize(&"x".repeat(300)).len(), MAX_SHOWN_CHARS);
    }
}
