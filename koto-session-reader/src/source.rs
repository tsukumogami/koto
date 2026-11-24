//! Where the dashboard reads sessions from: the listing trait a session
//! store implements, and how a program finds the sessions directory koto
//! uses.

use std::path::{Path, PathBuf};

use crate::koto_root::{koto_root, KOTO_ROOT_UNRESOLVED};
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

// ---------------------------------------------------------------------------
// The store's layout names
// ---------------------------------------------------------------------------
//
// Each name is defined once, here, and koto's writer imports it, so the
// reader and the writer can't drift apart. docs/STABILITY.md's "Session store
// read surface" section promises them, and koto-stability-tests pins each as
// a literal, so a rename fails there rather than orphaning older readers.
// The state file's name is `crate::session::state_file_name`, the leg
// pointer's `crate::engine::leg_pointer::POINTER_FILENAME`.

/// Reserved prefix of a close session being staged in the sessions
/// directory. No session name can start with a dot, so a directory with this
/// prefix is never a session; every walk of the store passes it by.
pub const STAGING_PREFIX: &str = ".koto-staging-";

/// The directory under a session's directory that holds its context values,
/// one file per key, hierarchical keys as subdirectories.
pub const CTX_DIR: &str = "ctx";

/// koto's own index of a session's context values, directly under
/// [`CTX_DIR`]. Its shape is not promised; no context key can name it.
pub const MANIFEST_FILE: &str = "manifest.json";

/// The lock koto takes on a session's context writes, directly under
/// [`CTX_DIR`]. No context key can name it, and the reader never takes it.
pub const MANIFEST_LOCK_FILE: &str = "manifest.lock";

/// The layout marker's name, directly under the sessions directory. A
/// missing marker means layout 1; nothing writes one yet.
pub const STORE_LAYOUT_MARKER: &str = ".koto-store-layout";

/// The store layout this reader reads. A marker naming a higher number
/// makes the reader refuse the store.
pub const STORE_LAYOUT_VERSION: u32 = 1;

/// The koto release this crate was built with: the crate and koto share one
/// version.
pub const KOTO_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The event types that set or supersede a context key: a `transitioned`
/// event's `context_assignments` set keys, and `context_added`,
/// `context_removed` and `context_cleared` supersede them. A fifth would be
/// a breaking change to the read surface.
pub const CONTEXT_KEY_EVENTS: [&str; 4] = [
    "transitioned",
    "context_added",
    "context_removed",
    "context_cleared",
];

/// The most a state log may hold before a hardened read refuses it.
pub const STATE_LOG_READ_CAP: u64 = 512 * 1024 * 1024;

/// The most a context value may hold before `read_context` refuses it.
pub const CONTEXT_VALUE_READ_CAP: u64 = 16 * 1024 * 1024;

/// The most the layout marker may hold before `check_layout` refuses it.
pub const LAYOUT_MARKER_READ_CAP: u64 = 64;

// ---------------------------------------------------------------------------
// Finding the store
// ---------------------------------------------------------------------------

/// The variable that moves the local backend's sessions directory.
const SESSIONS_BASE_ENV: &str = "KOTO_SESSIONS_BASE";

/// The sessions directory's name under the koto root.
const SESSIONS_UNDER_ROOT: &str = "sessions";

/// Which of koto's session backends a sessions directory is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// The local backend, whose directory `KOTO_SESSIONS_BASE` can move.
    Local,
    /// The cloud backend, whose local mirror is always under the koto root.
    Cloud,
}

/// Where a sessions directory came from. koto's backend constructors tell
/// the two apart: a store under the koto root journals into the root, a
/// moved one inside its own base.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionsDir {
    /// `KOTO_SESSIONS_BASE`, as set.
    SessionsBase(PathBuf),
    /// `<root>/sessions`.
    UnderRoot {
        /// The koto root.
        root: PathBuf,
        /// `<root>/sessions`.
        dir: PathBuf,
    },
}

impl SessionsDir {
    /// The sessions directory itself.
    pub fn dir(&self) -> &Path {
        match self {
            SessionsDir::SessionsBase(dir) => dir,
            SessionsDir::UnderRoot { dir, .. } => dir,
        }
    }
}

/// The sessions directory rule, apart from the process environment.
///
/// `sessions_base` is `KOTO_SESSIONS_BASE` (set and UTF-8, an empty value
/// included) and `root` the koto root. `Some(Cloud)` gives `<root>/sessions`
/// whatever the base says; `Some(Local)` and `None` give the base when it is
/// set, else `<root>/sessions`. `None` only when the answer needs a root and
/// there is none.
#[doc(hidden)]
pub fn resolve_sessions_dir(
    backend: Option<BackendKind>,
    sessions_base: Option<String>,
    root: Option<PathBuf>,
) -> Option<SessionsDir> {
    let under_root = |root: Option<PathBuf>| {
        root.map(|root| SessionsDir::UnderRoot {
            dir: root.join(SESSIONS_UNDER_ROOT),
            root,
        })
    };
    match backend {
        Some(BackendKind::Cloud) => under_root(root),
        Some(BackendKind::Local) | None => match sessions_base {
            Some(base) => Some(SessionsDir::SessionsBase(PathBuf::from(base))),
            None => under_root(root),
        },
    }
}

/// [`resolve_sessions_dir`] over this process's environment. The koto root
/// is resolved only when the answer needs it, so a set `KOTO_SESSIONS_BASE`
/// resolves with no home directory.
#[doc(hidden)]
pub fn locate_sessions_dir(backend: Option<BackendKind>) -> anyhow::Result<SessionsDir> {
    let sessions_base = match backend {
        Some(BackendKind::Cloud) => None,
        Some(BackendKind::Local) | None => std::env::var(SESSIONS_BASE_ENV).ok(),
    };
    let root = if sessions_base.is_some() {
        None
    } else {
        koto_root()
    };
    resolve_sessions_dir(backend, sessions_base, root)
        .ok_or_else(|| anyhow::anyhow!(KOTO_ROOT_UNRESOLVED))
}

/// The sessions directory koto uses, found by koto's own rule.
///
/// The koto root is `$KOTO_HOME` when it is set and not empty (a relative
/// value taken against the current directory), else `~/.koto`.
/// `Some(BackendKind::Cloud)` gives `<root>/sessions`, the cloud backend's
/// local mirror. `Some(BackendKind::Local)` and `None` give
/// `KOTO_SESSIONS_BASE` when it is set, else `<root>/sessions`.
///
/// A program reading koto's store passes `None`, which is exact inside every
/// koto tick, and outside one except when the cloud backend is configured
/// and `KOTO_SESSIONS_BASE`, a variable koto documents for testing, was
/// exported by hand. A caller that wants certainty and has koto on `PATH`
/// reads the `sessions` field of `koto session dir --json`, which comes from
/// this function. Reads only the environment: nothing is opened or created.
pub fn sessions_dir(backend: Option<BackendKind>) -> anyhow::Result<PathBuf> {
    Ok(match locate_sessions_dir(backend)? {
        SessionsDir::SessionsBase(dir) => dir,
        SessionsDir::UnderRoot { dir, .. } => dir,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::koto_root::resolve_koto_root;
    use std::ffi::OsString;

    fn root_of(koto_home: Option<&str>, home: Option<&str>, cwd: Option<&str>) -> Option<PathBuf> {
        resolve_koto_root(
            koto_home.map(OsString::from),
            home.map(PathBuf::from),
            cwd.map(Path::new),
        )
    }

    fn dir_of(found: Option<SessionsDir>) -> Option<PathBuf> {
        found.map(|d| d.dir().to_path_buf())
    }

    #[test]
    fn cloud_is_under_the_root_even_with_a_base_set() {
        let root = root_of(Some("/k"), Some("/h"), None);
        assert_eq!(
            resolve_sessions_dir(Some(BackendKind::Cloud), Some("/base".into()), root),
            Some(SessionsDir::UnderRoot {
                root: PathBuf::from("/k"),
                dir: PathBuf::from("/k/sessions"),
            })
        );
    }

    #[test]
    fn local_and_none_take_the_base_when_set() {
        for backend in [Some(BackendKind::Local), None] {
            let root = root_of(Some("/k"), Some("/h"), None);
            assert_eq!(
                resolve_sessions_dir(backend, Some("/base".into()), root),
                Some(SessionsDir::SessionsBase(PathBuf::from("/base"))),
                "{backend:?}"
            );
        }
    }

    #[test]
    fn an_empty_base_still_counts_as_set() {
        assert_eq!(
            dir_of(resolve_sessions_dir(
                None,
                Some(String::new()),
                root_of(None, Some("/h"), None)
            )),
            Some(PathBuf::new())
        );
    }

    #[test]
    fn local_and_none_are_under_the_root_without_a_base() {
        for backend in [Some(BackendKind::Local), None] {
            let root = root_of(Some("/k"), Some("/h"), None);
            assert_eq!(
                dir_of(resolve_sessions_dir(backend, None, root)),
                Some(PathBuf::from("/k/sessions")),
                "{backend:?}"
            );
        }
    }

    #[test]
    fn a_base_resolves_with_no_root() {
        assert_eq!(
            resolve_sessions_dir(None, Some("/base".into()), None),
            Some(SessionsDir::SessionsBase(PathBuf::from("/base")))
        );
        assert_eq!(resolve_sessions_dir(None, None, None), None);
        assert_eq!(
            resolve_sessions_dir(Some(BackendKind::Cloud), Some("/base".into()), None),
            None
        );
    }

    #[test]
    fn the_default_root_is_dot_koto_under_home() {
        assert_eq!(
            dir_of(resolve_sessions_dir(
                None,
                None,
                root_of(None, Some("/h"), Some("/w"))
            )),
            Some(PathBuf::from("/h/.koto/sessions"))
        );
    }

    #[test]
    fn an_empty_koto_home_reads_as_unset() {
        assert_eq!(
            dir_of(resolve_sessions_dir(
                Some(BackendKind::Cloud),
                None,
                root_of(Some(""), Some("/h"), Some("/w"))
            )),
            Some(PathBuf::from("/h/.koto/sessions"))
        );
    }

    #[test]
    fn a_relative_koto_home_is_taken_against_the_current_directory() {
        assert_eq!(
            dir_of(resolve_sessions_dir(
                Some(BackendKind::Local),
                None,
                root_of(Some("k"), Some("/h"), Some("/w"))
            )),
            Some(PathBuf::from("/w/k/sessions"))
        );
    }
}
