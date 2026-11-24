//! Where the dashboard reads sessions from: the listing trait a session
//! store implements.

use std::path::{Path, PathBuf};

use crate::session::SessionInfo;

/// A store of session directories the dashboard lists and reads, and
/// never writes.
///
/// The listing half of
/// [`SessionBackend`](crate::session::SessionBackend), which
/// [`LocalBackend`](crate::session::local::LocalBackend) implements, so
/// koto passes its local store (or the cloud backend's local mirror)
/// wherever a `&dyn SessionSource` is asked for.
pub trait SessionSource {
    /// The directory the sessions live under, one subdirectory each.
    fn sessions_dir(&self) -> &Path;
    /// Every session whose state file header reads, with the metadata
    /// that header carries.
    fn list(&self) -> anyhow::Result<Vec<SessionInfo>>;
    /// The directory of session `id` (no I/O, just path computation).
    fn session_dir(&self, id: &str) -> PathBuf;
    /// How many session directories hold a state file whose header
    /// fails to parse: the sessions [`list`](Self::list) leaves out.
    fn count_unreadable(&self) -> usize;
}
