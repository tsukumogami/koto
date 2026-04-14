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

    /// An I/O error from the underlying storage backend that isn't a
    /// collision. Preserves the original `io::Error` so callers can
    /// inspect its `kind()` for retry decisions.
    Io(std::io::Error),

    /// A serialization failure (serde, JSON formatting, etc.). String
    /// payload because `serde_json::Error` doesn't round-trip cleanly.
    Serialization(String),

    /// Fallback for failures that don't fit the variants above.
    Other(anyhow::Error),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionError::Collision => {
                write!(f, "state file already exists at the target path")
            }
            SessionError::Io(e) => write!(f, "session I/O error: {}", e),
            SessionError::Serialization(s) => write!(f, "session serialization error: {}", s),
            SessionError::Other(e) => write!(f, "session error: {}", e),
        }
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SessionError::Io(e) => Some(e),
            SessionError::Other(e) => Some(e.as_ref()),
            _ => None,
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

impl From<serde_json::Error> for SessionError {
    fn from(e: serde_json::Error) -> Self {
        SessionError::Serialization(e.to_string())
    }
}

impl From<anyhow::Error> for SessionError {
    fn from(e: anyhow::Error) -> Self {
        // If the underlying cause is an io::Error, preserve the typed
        // variant so `?` through `anyhow` chains in implementation code
        // still surfaces Collision correctly.
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            if io.kind() == std::io::ErrorKind::AlreadyExists {
                return SessionError::Collision;
            }
        }
        SessionError::Other(e)
    }
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
    /// target with the prefix `.koto-init-` and the suffix `.tmp` (for
    /// example `.koto-init-aBcDeF.tmp`). Sweep tooling that cleans up
    /// crashed initialisations relies on this convention.
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
