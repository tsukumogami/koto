//! Data layer for the `koto dashboard` command.
//!
//! Responsible for reading session state from disk and maintaining an
//! up-to-date `SessionTree`. Full implementation in Issue 2.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;

use crate::cli::DashboardArgs;
use crate::session::SessionBackend;

/// Lightweight snapshot of one session's derived state, held in the tree.
///
/// Full field definitions arrive in Issue 2.
pub struct CachedSession {
    /// Placeholder — full struct defined in Issue 2.
    pub _placeholder: (),
}

/// Hierarchical view of all sessions visible to the dashboard.
pub struct SessionTree {
    /// All sessions indexed by name.
    pub sessions: HashMap<String, CachedSession>,
    /// Names of root sessions (those with no parent).
    pub roots: Vec<String>,
}

impl SessionTree {
    /// Construct an empty tree.
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            roots: Vec::new(),
        }
    }
}

impl Default for SessionTree {
    fn default() -> Self {
        Self::new()
    }
}

/// Placeholder for gate-evaluation detail data loaded on demand.
///
/// Full field definitions arrive in Issue 2.
pub struct DetailData {
    /// Placeholder — full struct defined in Issue 2.
    pub _placeholder: (),
}

/// Refresh the session tree by scanning the backend and re-reading changed
/// sessions.
///
/// Full implementation arrives in Issue 2.
pub fn refresh(_tree: &mut SessionTree, _backend: &dyn SessionBackend) -> Result<()> {
    Ok(())
}

/// Entry point called from the CLI dispatch in `src/cli/mod.rs`.
///
/// For now, prints a stub message and returns successfully so that
/// `koto dashboard --help` and basic invocation work before the full
/// implementation lands in Issue 5.
pub fn run(_args: DashboardArgs, _backend: &dyn SessionBackend) -> Result<()> {
    println!("dashboard not yet implemented");
    Ok(())
}

/// Resolve the session path for a given session name via the backend.
///
/// Stub — used by Issue 2 data functions.
pub fn session_path_for(name: &str, backend: &dyn SessionBackend) -> PathBuf {
    backend.session_dir(name)
}
