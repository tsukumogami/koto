//! The koto root: the directory every path koto keeps for itself lives under.
//!
//! Sessions (`<root>/sessions`), the request store (`<root>/requests`), the
//! user config (`<root>/config.toml`), the terminal index, coordinator cursors,
//! the decider ledger and the run journal are all built from [`koto_home`].
//! `KOTO_HOME` moves all of them at once; unset, the root is `~/.koto`.
//!
//! This is the one place koto asks for the user's home directory.
//! `clippy.toml` disallows `dirs::home_dir` everywhere else, so a new path
//! built from the home directory fails `cargo clippy -D warnings` instead of
//! quietly escaping `KOTO_HOME`.
//!
//! `KOTO_SESSIONS_BASE` keeps its meaning: it moves only the local backend's
//! session directory, and wins over `<root>/sessions` when both are set.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The variable that sets the koto root.
pub const KOTO_HOME_ENV: &str = "KOTO_HOME";

/// The koto root: `$KOTO_HOME` when it is set and not empty, else `~/.koto`.
///
/// A relative `KOTO_HOME` is taken against the current directory, so the root
/// doesn't move when a later step changes directory. `None` when `KOTO_HOME`
/// is unset and there is no home directory, or the current directory can't be
/// read for a relative value.
pub fn koto_home() -> Option<PathBuf> {
    #[allow(clippy::disallowed_methods)]
    let home = dirs::home_dir();
    resolve(
        std::env::var_os(KOTO_HOME_ENV),
        home,
        std::env::current_dir().ok().as_deref(),
    )
}

/// [`koto_home`], or an error naming both ways the root is found.
pub fn koto_home_or_err() -> anyhow::Result<PathBuf> {
    koto_home().ok_or_else(|| {
        anyhow::anyhow!(
            "could not determine the koto root: KOTO_HOME is unset and there is no home directory"
        )
    })
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

/// The resolution itself, apart from the process environment.
fn resolve(
    koto_home: Option<OsString>,
    home: Option<PathBuf>,
    cwd: Option<&Path>,
) -> Option<PathBuf> {
    match koto_home {
        Some(v) if !v.is_empty() => {
            let p = PathBuf::from(v);
            if p.is_absolute() {
                Some(p)
            } else {
                cwd.map(|c| c.join(p))
            }
        }
        _ => home.map(|h| h.join(".koto")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_is_dot_koto_under_home() {
        assert_eq!(
            resolve(None, Some(PathBuf::from("/h")), Some(Path::new("/w"))),
            Some(PathBuf::from("/h/.koto"))
        );
    }

    #[test]
    fn empty_counts_as_unset() {
        assert_eq!(
            resolve(Some(OsString::new()), Some(PathBuf::from("/h")), None),
            Some(PathBuf::from("/h/.koto"))
        );
    }

    #[test]
    fn absolute_koto_home_is_the_root() {
        assert_eq!(
            resolve(
                Some("/k".into()),
                Some(PathBuf::from("/h")),
                Some(Path::new("/w"))
            ),
            Some(PathBuf::from("/k"))
        );
    }

    #[test]
    fn koto_home_needs_no_home_directory() {
        assert_eq!(
            resolve(Some("/k".into()), None, None),
            Some(PathBuf::from("/k"))
        );
    }

    #[test]
    fn relative_koto_home_is_taken_against_the_current_directory() {
        assert_eq!(
            resolve(
                Some("k".into()),
                Some(PathBuf::from("/h")),
                Some(Path::new("/w"))
            ),
            Some(PathBuf::from("/w/k"))
        );
        assert_eq!(
            resolve(Some("k".into()), Some(PathBuf::from("/h")), None),
            None
        );
    }

    #[test]
    fn no_root_without_either() {
        assert_eq!(resolve(None, None, Some(Path::new("/w"))), None);
    }
}
