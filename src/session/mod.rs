pub mod cloud;
pub mod context;
pub mod local;
pub mod sync;
pub mod validate;
pub mod version;

use std::path::PathBuf;

use self::context::ContextStore;
use self::local::LocalBackend;

/// Information about an existing session.
#[derive(serde::Serialize)]
pub struct SessionInfo {
    /// Session identifier (same as the workflow name).
    pub id: String,

    /// RFC 3339 UTC timestamp of session creation (from state file header).
    pub created_at: String,

    /// SHA-256 hash of the compiled template (from state file header).
    pub template_hash: String,
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
