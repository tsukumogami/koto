use anyhow::Result;

use crate::session::Backend;
use crate::session::SessionBackend;

/// Print the absolute session directory path.
pub fn handle_dir(backend: &dyn SessionBackend, name: &str) -> Result<()> {
    let dir = backend.session_dir(name);
    println!("{}", dir.display());
    Ok(())
}

/// Print all sessions as a JSON array.
pub fn handle_list(backend: &dyn SessionBackend) -> Result<()> {
    let sessions = backend.list()?;
    println!("{}", serde_json::to_string_pretty(&sessions)?);
    Ok(())
}

/// Remove a session directory. Idempotent: succeeds even if the session doesn't exist.
pub fn handle_cleanup(backend: &dyn SessionBackend, name: &str) -> Result<()> {
    backend.cleanup(name)?;
    Ok(())
}

/// Resolve a session version conflict by keeping either the local or remote state.
pub fn handle_resolve(backend: &Backend, name: &str, keep: &str) -> Result<()> {
    match keep {
        "local" | "remote" => {}
        other => anyhow::bail!(
            "invalid --keep value: '{}'. Must be 'local' or 'remote'.",
            other
        ),
    }

    let cloud = match backend {
        Backend::Cloud(c) => c,
        Backend::Local(_) => {
            anyhow::bail!("session resolve requires cloud backend (session.backend = \"cloud\")")
        }
    };

    cloud.resolve_conflict(name, keep)
}
