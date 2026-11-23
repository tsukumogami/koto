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

/// How many of a pool of `size`'s slots are free: `size` minus the slot
/// files held, of any index, never below 0. A pool shrunk below its holders
/// has none free until fewer than `size` are held (PRD R8).
pub fn free_slots(dir: &Path, size: u32) -> usize {
    let held = slot_files(dir)
        .iter()
        .filter(|(_, path)| probe_held(path))
        .count();
    (size as usize).saturating_sub(held)
}

/// Take a free slot of a pool of `size`, writing `holder` into it.
///
/// Every slot file present is probed first, whatever its index, so a pool
/// shrunk below its holders admits nobody until fewer than `size` are held
/// (PRD R8). Returns `None` when no slot is free.
pub fn try_acquire(dir: &Path, size: u32, holder: &Holder) -> std::io::Result<Option<Slot>> {
    if free_slots(dir, size) == 0 {
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

/// A free slot reserved for one waiter until `deadline` (PRD R11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    pub ticket: u64,
    /// Unix seconds after which the claim passes to the next waiter.
    pub deadline: u64,
}

/// `queue.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queue {
    #[serde(default)]
    pub next_ticket: u64,
    #[serde(default)]
    pub entries: Vec<QueueEntry>,
    /// The waiter a free slot is reserved for, if any. A file written before
    /// claims existed reads as none.
    #[serde(default)]
    pub claim: Option<Claim>,
}

impl Queue {
    /// The index of `session`'s entry from `workspace`.
    pub fn position_of(&self, session: &str, workspace: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.session == session && e.workspace == workspace)
    }

    /// Drop every waiter that hasn't ticked for more than `lapse_secs`
    /// (PRD R12). The waiters behind one move up. Returns whether any went.
    pub fn sweep_lapsed(&mut self, now: u64, lapse_secs: u32) -> bool {
        let before = self.entries.len();
        self.entries
            .retain(|e| now.saturating_sub(e.last_seen) <= u64::from(lapse_secs));
        self.entries.len() != before
    }

    /// Find this session's entry and mark it seen now, or add one at the
    /// back. Returns the entry's index and whether it was added.
    pub fn join_or_refresh(&mut self, session: &str, workspace: &str, now: u64) -> (usize, bool) {
        if let Some(i) = self.position_of(session, workspace) {
            self.entries[i].last_seen = now;
            return (i, false);
        }
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        self.entries.push(QueueEntry {
            ticket,
            session: session.to_string(),
            workspace: workspace.to_string(),
            joined_at: now,
            last_seen: now,
        });
        (self.entries.len() - 1, true)
    }

    /// Bring the claim up to date for `free` slots (PRD R11, DESIGN "Taking
    /// a slot", step 4).
    ///
    /// With no free slot there is nothing to reserve, so the claim goes; the
    /// next slot that frees starts a new one on the head waiter, which is
    /// how a waiter that was skipped keeps first place. With a free slot,
    /// a claim whose waiter has left goes, a claim whose deadline passed
    /// moves to the waiter after it with a fresh deadline (the skipped one
    /// keeps its place), and with none the head waiter gets one. A claim
    /// starts at the first edit that sees the slot free, whether its holder
    /// exited cleanly or was killed: nothing depends on the holder saying
    /// it left.
    pub fn settle_claim(&mut self, free: usize, claim_secs: u32, now: u64) {
        if free == 0 || self.entries.is_empty() {
            self.claim = None;
            return;
        }
        let fresh = |ticket| Claim {
            ticket,
            deadline: now.saturating_add(u64::from(claim_secs)),
        };
        let claimed = self
            .claim
            .and_then(|c| self.entries.iter().position(|e| e.ticket == c.ticket));
        self.claim = match (self.claim, claimed) {
            (Some(c), Some(_)) if now <= c.deadline => Some(c),
            (Some(_), Some(i)) => Some(fresh(self.entries[(i + 1) % self.entries.len()].ticket)),
            _ => Some(fresh(self.entries[0].ticket)),
        };
    }

    /// Whether the entry at `index` may take one of `free` slots: the
    /// claimed waiter and the `free - 1` after it, in queue order (wrapping
    /// to the front), so a pool with several free slots admits that many
    /// waiters at once. Call after [`Queue::settle_claim`].
    pub fn may_take(&self, index: usize, free: usize) -> bool {
        let len = self.entries.len();
        if free == 0 || index >= len {
            return false;
        }
        let start = self
            .claim
            .and_then(|c| self.entries.iter().position(|e| e.ticket == c.ticket))
            .unwrap_or(0);
        (index + len - start) % len < free
    }

    /// Remove the entry at `index`, and the claim with it when it was that
    /// entry's.
    pub fn remove_at(&mut self, index: usize) {
        let entry = self.entries.remove(index);
        if self.claim.is_some_and(|c| c.ticket == entry.ticket) {
            self.claim = None;
        }
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

/// Remove `session`'s entry from `workspace` in the queue of pool `name`
/// under `root`, if the pool's directory exists; nothing is created.
/// `Ok(true)` when the queue no longer holds the entry (it was removed or
/// was never there), `Ok(false)` when `queue.lock` stayed busy, so the
/// entry may remain until it lapses.
pub fn leave_queue(
    root: &Path,
    name: &str,
    session: &str,
    workspace: &str,
) -> std::io::Result<bool> {
    if !crate::template::types::is_valid_pool_name(name) {
        return Ok(true);
    }
    let dir = root.join("pools").join(name);
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if meta.file_type().is_dir() => {}
        _ => return Ok(true),
    }
    let left = edit_queue(&dir, |queue| match queue.position_of(session, workspace) {
        Some(i) => {
            queue.remove_at(i);
            ((), true)
        }
        None => ((), false),
    })?;
    Ok(left.is_some())
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

    fn waiters(names: &[&str], now: u64) -> Queue {
        let mut q = Queue::default();
        for name in names {
            q.join_or_refresh(name, "w", now);
        }
        q
    }

    fn claimed(q: &Queue) -> Option<&str> {
        let c = q.claim?;
        q.entries
            .iter()
            .find(|e| e.ticket == c.ticket)
            .map(|e| e.session.as_str())
    }

    #[test]
    fn a_free_slot_is_claimed_for_the_head_waiter_only() {
        let mut q = waiters(&["w1", "w2", "w3"], 100);
        q.settle_claim(1, 5, 100);
        assert_eq!(claimed(&q), Some("w1"));
        assert_eq!(q.claim.unwrap().deadline, 105);
        assert!(q.may_take(0, 1));
        assert!(!q.may_take(1, 1));
        assert!(!q.may_take(2, 1));
        // Two free slots admit the two at the front.
        assert!(q.may_take(1, 2));
        assert!(!q.may_take(2, 2));
    }

    #[test]
    fn an_expired_claim_passes_on_and_the_skipped_waiter_keeps_first_place() {
        let mut q = waiters(&["w1", "w2"], 100);
        q.settle_claim(1, 5, 100);
        // Within the period the claim stays.
        q.settle_claim(1, 5, 105);
        assert_eq!(claimed(&q), Some("w1"));
        q.settle_claim(1, 5, 106);
        assert_eq!(claimed(&q), Some("w2"));
        assert_eq!(q.claim.unwrap().deadline, 111);
        assert!(q.may_take(1, 1));
        assert!(!q.may_take(0, 1));
        // w2 takes the slot and leaves; w1 is still first.
        q.remove_at(1);
        assert!(q.claim.is_none());
        assert_eq!(q.entries[0].session, "w1");
        q.settle_claim(1, 5, 120);
        assert_eq!(claimed(&q), Some("w1"));
    }

    #[test]
    fn no_free_slot_means_no_claim() {
        let mut q = waiters(&["w1"], 100);
        q.settle_claim(1, 5, 100);
        q.settle_claim(0, 5, 101);
        assert!(q.claim.is_none());
        assert!(!q.may_take(0, 0));
    }

    #[test]
    fn a_claim_whose_waiter_left_goes_to_the_head() {
        let mut q = waiters(&["w1", "w2"], 100);
        q.settle_claim(1, 5, 100);
        q.entries.remove(0);
        q.settle_claim(1, 5, 101);
        assert_eq!(claimed(&q), Some("w2"));
    }

    #[test]
    fn lapsed_waiters_are_swept_and_those_behind_move_up() {
        let mut q = waiters(&["w1", "w2"], 100);
        q.entries[1].last_seen = 104;
        assert!(q.sweep_lapsed(106, 5));
        assert_eq!(q.entries.len(), 1);
        assert_eq!(q.position_of("w2", "w"), Some(0));
        // Joining again goes to the back with a new ticket.
        let (i, added) = q.join_or_refresh("w1", "w", 106);
        assert!(added);
        assert_eq!(i, 1);
    }

    #[test]
    fn a_queue_file_without_a_claim_reads_as_none() {
        let q: Queue = serde_json::from_str(r#"{"next_ticket":1,"entries":[]}"#).unwrap();
        assert!(q.claim.is_none());
    }

    #[test]
    fn leaving_a_missing_pool_creates_nothing() {
        let root = tempfile::tempdir().unwrap();
        assert!(leave_queue(root.path(), "p", "s", "w").unwrap());
        assert!(!root.path().join("pools").exists());
        let dir = pool_dir(root.path(), "p").unwrap();
        edit_queue(&dir, |q| {
            q.join_or_refresh("s", "w", 1);
            ((), true)
        })
        .unwrap();
        assert!(leave_queue(root.path(), "p", "s", "w").unwrap());
        assert!(read_queue(&dir).entries.is_empty());
    }

    #[test]
    fn sanitize_strips_control_characters_and_cuts() {
        assert_eq!(sanitize("a\u{1b}[31mb\n"), "a[31mb");
        assert_eq!(sanitize(&"x".repeat(300)).len(), MAX_SHOWN_CHARS);
    }
}
