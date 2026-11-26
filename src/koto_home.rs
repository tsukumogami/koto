//! The koto root: the directory every path koto keeps for itself lives under.
//!
//! Sessions (`<root>/sessions`), the request store (`<root>/requests`), the
//! user config (`<root>/config.toml`), the terminal index, coordinator cursors,
//! the decider ledger and the run journal are all built from [`koto_home`].
//! `KOTO_HOME` moves all of them at once, and the compiled-template cache
//! with them (`<root>/cache`, see `crate::cache`); unset, the root is
//! `~/.koto` and the cache stays on `XDG_CACHE_HOME`.
//!
//! The rule itself lives in `koto_session_reader::koto_root`, so koto and a
//! program reading koto's store resolve the same root. That module holds the
//! one home-directory lookup in either crate; `clippy.toml` disallows the
//! lookup everywhere else, so a new path built from the home directory fails
//! `cargo clippy -D warnings` instead of quietly escaping `KOTO_HOME`. The
//! lint can't see a read of the `HOME` variable itself; the two that exist
//! are the cache fallback and Claude Code's own `~/.claude` directory,
//! neither a koto root path.
//!
//! `KOTO_SESSIONS_BASE` keeps its meaning: it moves only the local backend's
//! session directory, and wins over `<root>/sessions` when both are set
//! (`koto_session_reader::source::sessions_dir`).

use std::path::{Path, PathBuf};

/// The variable that sets the koto root.
pub use koto_session_reader::koto_root::KOTO_HOME_ENV;

/// The koto root: `$KOTO_HOME` when it is set and not empty, else `~/.koto`.
///
/// A relative `KOTO_HOME` is taken against the current directory of each koto
/// process, so two runs from different directories resolve different roots;
/// the commands a tick runs are given the absolute root, so they don't. `None`
/// when `KOTO_HOME` is unset and there is no home directory, or the current
/// directory can't be read for a relative value.
pub fn koto_home() -> Option<PathBuf> {
    koto_session_reader::koto_root::koto_root()
}

/// [`koto_home`], or an error naming both ways the root is found.
pub fn koto_home_or_err() -> anyhow::Result<PathBuf> {
    koto_home().ok_or_else(|| anyhow::anyhow!(koto_session_reader::koto_root::KOTO_ROOT_UNRESOLVED))
}

/// The root `KOTO_HOME` sets, or `None` when it is unset or empty and the root
/// is the default. A command koto runs gets this value, so a koto it starts
/// uses the same root.
pub fn koto_home_override() -> Option<PathBuf> {
    match std::env::var_os(KOTO_HOME_ENV) {
        Some(v) if !v.is_empty() => koto_home(),
        _ => None,
    }
}

/// Create `root` (and its parents) if it doesn't exist, giving it mode 0700
/// when this call creates it, as koto does for `~/.koto`.
pub fn ensure_private_root(root: &Path) -> std::io::Result<()> {
    let needs_create = !root.exists();
    std::fs::create_dir_all(root)?;
    if needs_create {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}
