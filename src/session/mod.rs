pub mod cloud;
pub mod context;
pub mod local;
pub mod sync;
pub mod validate;
pub mod version;

use std::fmt;
use std::path::PathBuf;

use self::context::ContextStore;
use self::local::LocalBackend;

use crate::engine::types::{Event, EventPayload, StateFileHeader};

/// Typed error for session-scoped operations.
///
/// Introduced primarily so callers of `SessionBackend::init_state_file`
/// can discriminate a collision (state file already present at the
/// target path) from other I/O failures without relying on downcasting
/// an `anyhow::Error`. The collision variant lets upstream batch-spawn
/// logic map "someone else already initialised this workflow" to a
/// specific outcome (e.g., respawn or skip) while treating other I/O
/// failures as retryable infrastructure errors.
#[derive(Debug)]
pub enum SessionError {
    /// A state file already exists at the target path. Emitted when the
    /// atomic create-or-fail rename sees the destination occupied.
    Collision,

    /// Another process or thread already holds the advisory flock on
    /// the target state file. Emitted when
    /// `SessionBackend::lock_state_file` attempts a non-blocking
    /// `LOCK_EX | LOCK_NB` and the kernel reports `EWOULDBLOCK`.
    ///
    /// `holder_pid` is populated on a best-effort basis. Today this
    /// variant is surfaced directly by the flock `EWOULDBLOCK` path
    /// which carries no peer-pid information, so the field is always
    /// `None`; it is reserved so a future `F_OFD_GETLK`-based probe can
    /// attach the holder's PID without a breaking change to the API.
    Locked { holder_pid: Option<u32> },

    /// An I/O error from the underlying storage backend that isn't a
    /// collision. Preserves the original `io::Error` so callers can
    /// inspect its `kind()` for retry decisions.
    Io(std::io::Error),

    /// Fallback for failures that don't fit the variants above.
    Other(anyhow::Error),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionError::Collision => {
                write!(f, "state file already exists at the target path")
            }
            SessionError::Locked { holder_pid } => match holder_pid {
                Some(pid) => {
                    write!(f, "state file is already locked by pid {}", pid)
                }
                None => write!(f, "state file is already locked by another process"),
            },
            SessionError::Io(e) => write!(f, "session I/O error: {}", e),
            SessionError::Other(e) => write!(f, "session error: {}", e),
        }
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SessionError::Io(e) => Some(e),
            SessionError::Other(e) => Some(e.as_ref()),
            SessionError::Collision | SessionError::Locked { .. } => None,
        }
    }
}

impl From<std::io::Error> for SessionError {
    fn from(e: std::io::Error) -> Self {
        // ErrorKind::AlreadyExists is the canonical signal for "the
        // create-or-fail rename saw the destination occupied". Map it
        // to the dedicated Collision variant so callers don't have to
        // peek at `kind()` themselves.
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            SessionError::Collision
        } else {
            SessionError::Io(e)
        }
    }
}

/// RAII guard for an advisory `flock(LOCK_EX)` held on a session's
/// state file.
///
/// Acquired via [`SessionBackend::lock_state_file`]. The lock is
/// released automatically when the guard is dropped (the underlying
/// file handle closes, which the kernel converts into a `LOCK_UN`).
/// Process death also releases the lock through the same kernel
/// mechanism, so a crash between acquisition and drop cannot strand
/// the parent state file.
///
/// The guard owns the open `File`. Callers should hold it for the
/// duration of the critical section and let it drop at the end of
/// scope; there is no explicit `release` method because the guard's
/// entire job is to tie the lock lifetime to the Rust scope.
#[cfg(unix)]
#[derive(Debug)]
pub struct SessionLock {
    // The field is intentionally read-only and unused outside the
    // Drop impl: its sole purpose is to keep the file descriptor open
    // so the kernel-level flock remains held.
    _file: std::fs::File,
}

/// Non-Unix fallback. koto does not support Windows today; this stub
/// lets downstream code reference the type without a `cfg(unix)` gate
/// everywhere, while `lock_state_file` itself returns
/// `SessionError::Other` on non-Unix targets.
#[cfg(not(unix))]
#[derive(Debug)]
pub struct SessionLock {
    _private: (),
}

/// Information about an existing session.
#[derive(serde::Serialize)]
pub struct SessionInfo {
    /// Session identifier (same as the workflow name).
    pub id: String,

    /// RFC 3339 UTC timestamp of session creation (from state file header).
    pub created_at: String,

    /// SHA-256 hash of the compiled template (from state file header).
    pub template_hash: String,

    /// Name of the parent workflow, if this workflow was created as a child.
    pub parent_workflow: Option<String>,
}

/// Return the state file name for a given session ID.
///
/// The naming convention (`koto-<id>.state.jsonl`) is a free function
/// because it doesn't vary across backends.
pub fn state_file_name(id: &str) -> String {
    format!("koto-{}.state.jsonl", id)
}

/// Abstraction over where session artifacts are stored.
///
/// All backends produce a local filesystem path that agents can use
/// with file tools. Future backends (cloud, git) will implement this
/// trait alongside sync methods added when those features ship.
pub trait SessionBackend: Send + Sync {
    /// Create a new session directory. Returns the path.
    fn create(&self, id: &str) -> anyhow::Result<PathBuf>;

    /// Return the session directory path (no I/O, just path computation).
    fn session_dir(&self, id: &str) -> PathBuf;

    /// Check if a session exists (state file present, not just directory).
    fn exists(&self, id: &str) -> bool;

    /// Remove all session artifacts. Idempotent on missing directories.
    fn cleanup(&self, id: &str) -> anyhow::Result<()>;

    /// List all sessions with metadata extracted from state file headers.
    fn list(&self) -> anyhow::Result<Vec<SessionInfo>>;

    /// Append a header line to a new state file.
    fn append_header(&self, id: &str, header: &StateFileHeader) -> anyhow::Result<()>;

    /// Append an event to the state file.
    fn append_event(&self, id: &str, payload: &EventPayload, timestamp: &str)
        -> anyhow::Result<()>;

    /// Atomically create a session's state file with the given header and
    /// initial events.
    ///
    /// Bundles the header line and all initial events into a single
    /// tempfile-then-rename operation. The rename is "fail if exists" so
    /// two racing callers on the same path cannot silently overwrite each
    /// other: exactly one wins and the other receives
    /// `SessionError::Collision`. Other I/O failures surface as
    /// `SessionError::Io` so callers can distinguish a racing spawn from
    /// a disk-full / permission / storage-layer error.
    ///
    /// A crash between the tempfile write and the rename leaves no
    /// partially-written state file at the target path. The tempfile
    /// itself may remain and is garbage-collected by a separate sweep.
    ///
    /// Implementations store the tempfile in the same directory as the
    /// target using the filename convention defined by
    /// `INIT_TMP_PREFIX` / `INIT_TMP_SUFFIX` in `session::local`. Sweep
    /// tooling that cleans up crashed initialisations relies on this
    /// convention.
    ///
    /// On Linux this uses `renameat2(RENAME_NOREPLACE)` for a single-
    /// syscall fail-if-exists check. On other Unixes it uses POSIX
    /// `link()` followed by `unlink()` of the tempfile.
    fn init_state_file(
        &self,
        id: &str,
        header: StateFileHeader,
        initial_events: Vec<Event>,
    ) -> Result<(), SessionError>;

    /// Read all events from the state file.
    fn read_events(&self, id: &str) -> anyhow::Result<(StateFileHeader, Vec<Event>)>;

    /// Read just the header from the state file.
    fn read_header(&self, id: &str) -> anyhow::Result<StateFileHeader>;

    /// Push any pending local state for `id` to durable remote storage
    /// and fail if the push cannot be confirmed.
    ///
    /// This exists so batch operations that must enforce "push parent
    /// before child mutation" ordering (see Decision 12 Q6 in the
    /// batch-child-spawning design) have a strict fail-fast probe. The
    /// default `append_event` path on `CloudBackend` treats S3 errors as
    /// warnings so single-writer workflows stay responsive when the
    /// network flaps; the retry-failed dispatcher cannot tolerate that
    /// laxity because a silently-dropped parent push lets child writes
    /// race ahead of a stale parent log on S3.
    ///
    /// # Semantics
    ///
    /// * `LocalBackend`: no-op. Returns `Ok(())` because the local
    ///   `append_event` write is already durable on the filesystem by
    ///   the time the caller reaches this method.
    /// * `CloudBackend`: performs a synchronous PUT of the local state
    ///   file to S3 and returns `Err(SessionError::Io)` /
    ///   `Err(SessionError::Other)` on any transport or non-success
    ///   response.
    ///
    /// Callers that do NOT care about cross-host ordering should
    /// continue to use `append_event`; this method is specifically for
    /// the narrow retry-failed / batch-resolve paths.
    fn ensure_pushed(&self, id: &str) -> Result<(), SessionError>;

    /// Rename a session from `from` to `to`, updating the state file
    /// header to reflect the new identity.
    ///
    /// Renames the session directory, renames the state file within it,
    /// and rewrites the header's `workflow` field to `to` and
    /// `parent_workflow` to the parent derived from `to` (everything
    /// before the last `.` separator, or `None` if `to` contains no
    /// `.`).
    ///
    /// Returns `Err` if `from` doesn't exist or `to` already exists.
    fn relocate(&self, from: &str, to: &str) -> anyhow::Result<()>;

    /// Acquire a non-blocking advisory `flock(LOCK_EX | LOCK_NB)` on
    /// the session's state file.
    ///
    /// Intended for batch parents that must serialize concurrent tick
    /// calls (see Decision 12 in the batch-child-spawning design).
    /// Non-batch workflows never invoke this method -- the happy path
    /// is unchanged for them.
    ///
    /// # Semantics
    ///
    /// - On success, returns a [`SessionLock`] RAII guard. Dropping the
    ///   guard releases the lock. Process death releases the lock via
    ///   the kernel's `flock` semantics, so a crashed holder cannot
    ///   permanently strand the state file.
    /// - On contention, returns `SessionError::Locked { holder_pid }`
    ///   immediately -- this is a non-blocking acquisition, so callers
    ///   can surface a typed `ConcurrentTick` error without spinning.
    ///   `holder_pid` is best-effort; today it is always `None`.
    /// - If the state file does not yet exist, the method returns a
    ///   `SessionError::Io` with `ErrorKind::NotFound`. Callers are
    ///   expected to acquire the lock only after initialisation has
    ///   committed a state file.
    ///
    /// # Cloud backend
    ///
    /// `flock` is local-to-the-host. On `CloudBackend`, two processes
    /// running on different hosts both see the same remote state file
    /// in S3, but neither sees the other's file-descriptor lock.
    /// `lock_state_file` therefore only serializes intra-host
    /// contention under `CloudBackend`. Cross-host coordination is
    /// handled by the broader "push parent before child mutation"
    /// ordering described in the design's Decision 12 (Q6), not by
    /// this lock.
    ///
    /// # Windows
    ///
    /// Not supported. koto targets Unix-like systems; on non-Unix
    /// targets this method returns a `SessionError::Other` describing
    /// the platform limitation.
    fn lock_state_file(&self, id: &str) -> Result<SessionLock, SessionError>;
}

/// Unified backend that dispatches to either `LocalBackend` or
/// `CloudBackend` depending on configuration.
///
/// This enum allows `build_backend()` to return a single concrete type
/// that implements both `SessionBackend` and `ContextStore`, regardless
/// of which backend is selected.
pub enum Backend {
    Local(LocalBackend),
    Cloud(cloud::CloudBackend),
}

impl SessionBackend for Backend {
    fn create(&self, id: &str) -> anyhow::Result<PathBuf> {
        match self {
            Backend::Local(b) => b.create(id),
            Backend::Cloud(b) => b.create(id),
        }
    }

    fn session_dir(&self, id: &str) -> PathBuf {
        match self {
            Backend::Local(b) => b.session_dir(id),
            Backend::Cloud(b) => b.session_dir(id),
        }
    }

    fn exists(&self, id: &str) -> bool {
        match self {
            Backend::Local(b) => b.exists(id),
            Backend::Cloud(b) => b.exists(id),
        }
    }

    fn cleanup(&self, id: &str) -> anyhow::Result<()> {
        match self {
            Backend::Local(b) => b.cleanup(id),
            Backend::Cloud(b) => b.cleanup(id),
        }
    }

    fn list(&self) -> anyhow::Result<Vec<SessionInfo>> {
        match self {
            Backend::Local(b) => b.list(),
            Backend::Cloud(b) => b.list(),
        }
    }

    fn append_header(&self, id: &str, header: &StateFileHeader) -> anyhow::Result<()> {
        match self {
            Backend::Local(b) => b.append_header(id, header),
            Backend::Cloud(b) => b.append_header(id, header),
        }
    }

    fn append_event(
        &self,
        id: &str,
        payload: &EventPayload,
        timestamp: &str,
    ) -> anyhow::Result<()> {
        match self {
            Backend::Local(b) => b.append_event(id, payload, timestamp),
            Backend::Cloud(b) => b.append_event(id, payload, timestamp),
        }
    }

    fn read_events(&self, id: &str) -> anyhow::Result<(StateFileHeader, Vec<Event>)> {
        match self {
            Backend::Local(b) => b.read_events(id),
            Backend::Cloud(b) => b.read_events(id),
        }
    }

    fn read_header(&self, id: &str) -> anyhow::Result<StateFileHeader> {
        match self {
            Backend::Local(b) => b.read_header(id),
            Backend::Cloud(b) => b.read_header(id),
        }
    }

    fn init_state_file(
        &self,
        id: &str,
        header: StateFileHeader,
        initial_events: Vec<Event>,
    ) -> Result<(), SessionError> {
        match self {
            Backend::Local(b) => b.init_state_file(id, header, initial_events),
            Backend::Cloud(b) => b.init_state_file(id, header, initial_events),
        }
    }

    fn relocate(&self, from: &str, to: &str) -> anyhow::Result<()> {
        match self {
            Backend::Local(b) => b.relocate(from, to),
            Backend::Cloud(b) => b.relocate(from, to),
        }
    }

    fn lock_state_file(&self, id: &str) -> Result<SessionLock, SessionError> {
        match self {
            Backend::Local(b) => b.lock_state_file(id),
            Backend::Cloud(b) => b.lock_state_file(id),
        }
    }

    fn ensure_pushed(&self, id: &str) -> Result<(), SessionError> {
        match self {
            Backend::Local(b) => b.ensure_pushed(id),
            Backend::Cloud(b) => b.ensure_pushed(id),
        }
    }
}

impl ContextStore for Backend {
    fn add(&self, session: &str, key: &str, content: &[u8]) -> anyhow::Result<()> {
        match self {
            Backend::Local(b) => b.add(session, key, content),
            Backend::Cloud(b) => b.add(session, key, content),
        }
    }

    fn get(&self, session: &str, key: &str) -> anyhow::Result<Vec<u8>> {
        match self {
            Backend::Local(b) => b.get(session, key),
            Backend::Cloud(b) => b.get(session, key),
        }
    }

    fn ctx_exists(&self, session: &str, key: &str) -> bool {
        match self {
            Backend::Local(b) => b.ctx_exists(session, key),
            Backend::Cloud(b) => b.ctx_exists(session, key),
        }
    }

    fn remove(&self, session: &str, key: &str) -> anyhow::Result<()> {
        match self {
            Backend::Local(b) => b.remove(session, key),
            Backend::Cloud(b) => b.remove(session, key),
        }
    }

    fn list_keys(&self, session: &str, prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
        match self {
            Backend::Local(b) => b.list_keys(session, prefix),
            Backend::Cloud(b) => b.list_keys(session, prefix),
        }
    }
}
