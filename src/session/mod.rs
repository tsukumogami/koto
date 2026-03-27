pub mod local;
pub mod validate;

use std::path::PathBuf;

/// Information about an existing session.
pub struct SessionInfo {
    /// Session identifier (same as the workflow name).
    pub id: String,

    /// RFC 3339 UTC timestamp of session creation (from state file header).
    pub created_at: String,
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
