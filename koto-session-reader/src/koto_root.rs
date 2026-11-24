//! The koto root: the directory every path koto keeps for itself lives under.
//!
//! koto builds its sessions directory (`<root>/sessions`), its request
//! store, its user config and the rest from this root, and a program that
//! reads koto's store finds it the same way, through
//! [`crate::source::sessions_dir`]. `KOTO_HOME` moves the root; unset or
//! empty, the root is `~/.koto`.
//!
//! This is the one place the crate asks for the user's home directory. The
//! crate's `clippy.toml` bans the home-directory lookup everywhere, and the
//! single call below carries the crate's second sanctioned `#[allow]` (the
//! first is `fs_read.rs`); `scripts/check-reader-readonly.sh` checks that
//! this file holds exactly that one allow, on that one call.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The variable that sets the koto root.
pub const KOTO_HOME_ENV: &str = "KOTO_HOME";

/// What a caller says when the root can't be resolved, naming both ways the
/// root is found.
pub const KOTO_ROOT_UNRESOLVED: &str = "could not determine the koto root: KOTO_HOME is unset and there is no home directory, or KOTO_HOME is relative and the current directory can't be read";

/// The koto root rule, apart from the process environment.
///
/// `koto_home` is the value of `KOTO_HOME`, `home` the user's home
/// directory and `cwd` the current directory. A non-empty `KOTO_HOME` is
/// the root, taken against `cwd` when it is relative; an empty one counts
/// as unset, and the root is then `<home>/.koto`. `None` when neither gives
/// a root: `KOTO_HOME` unset with no home directory, or relative with no
/// current directory.
pub fn resolve_koto_root(
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

/// The koto root of this process: [`resolve_koto_root`] over `KOTO_HOME`,
/// the user's home directory and the current directory.
///
/// A relative `KOTO_HOME` is taken against the current directory of each
/// process, so two runs from different directories resolve different roots.
/// `None` when the root can't be resolved; [`KOTO_ROOT_UNRESOLVED`] says why.
pub fn koto_root() -> Option<PathBuf> {
    #[allow(clippy::disallowed_methods)]
    let home = dirs::home_dir();
    resolve_koto_root(
        std::env::var_os(KOTO_HOME_ENV),
        home,
        std::env::current_dir().ok().as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_is_dot_koto_under_home() {
        assert_eq!(
            resolve_koto_root(None, Some(PathBuf::from("/h")), Some(Path::new("/w"))),
            Some(PathBuf::from("/h/.koto"))
        );
    }

    #[test]
    fn empty_counts_as_unset() {
        assert_eq!(
            resolve_koto_root(Some(OsString::new()), Some(PathBuf::from("/h")), None),
            Some(PathBuf::from("/h/.koto"))
        );
    }

    #[test]
    fn absolute_koto_home_is_the_root() {
        assert_eq!(
            resolve_koto_root(
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
            resolve_koto_root(Some("/k".into()), None, None),
            Some(PathBuf::from("/k"))
        );
    }

    #[test]
    fn relative_koto_home_is_taken_against_the_current_directory() {
        assert_eq!(
            resolve_koto_root(
                Some("k".into()),
                Some(PathBuf::from("/h")),
                Some(Path::new("/w"))
            ),
            Some(PathBuf::from("/w/k"))
        );
        assert_eq!(
            resolve_koto_root(Some("k".into()), Some(PathBuf::from("/h")), None),
            None
        );
    }

    #[test]
    fn no_root_without_either() {
        assert_eq!(resolve_koto_root(None, None, Some(Path::new("/w"))), None);
    }
}
