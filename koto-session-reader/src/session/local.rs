//! The workspace prefix a session's import record names, as the dashboard
//! reads it. koto's `session::local` is the local session store.

use std::fs;
use std::path::Path;

use crate::cache::sha256_hex;

/// Derive the remote prefix of a workspace that may not exist on this
/// machine.
///
/// `koto session import --from <path>` names the workspace a session was
/// created in, usually a path on another host. When the path resolves here
/// this is exactly koto's `repo_id`; when it doesn't, the literal path is hashed
/// the same way, which matches the creating host's `repo_id` whenever the
/// caller passes the canonical path that host recorded. The path is only
/// hashed, never opened beyond the canonicalization attempt.
///
/// A relative path is refused: it would be resolved against whatever
/// directory the import happens to run from, not the source host's.
#[doc(hidden)]
pub fn prefix_for_workspace(path: &Path) -> anyhow::Result<String> {
    if !path.is_absolute() {
        anyhow::bail!("workspace path must be absolute, got '{}'", path.display());
    }
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let hash = sha256_hex(resolved.to_string_lossy().as_bytes());
    Ok(hash[..16].to_string())
}
