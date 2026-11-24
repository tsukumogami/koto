//! Where the dashboard reads sessions from: the listing trait a session
//! store implements, how a program finds the sessions directory koto uses,
//! the store's layout marker, the hardened listing a [`LocalDirSource`]
//! reads a sessions directory with, and the hardened reads of one session's
//! log ([`read_session_events`]) and one context value ([`read_context`]).

use std::fmt;
use std::path::{Path, PathBuf};

use crate::engine::context_assign::outstanding_assignments;
use crate::engine::errors::EngineError;
use crate::engine::persistence::{parse_header_bytes, parse_state_log};
use crate::engine::types::{Event, StateFileHeader};
use crate::fs_read::{read_store_file, read_store_first_line, ReadMode};
use crate::koto_root::{koto_root, KOTO_ROOT_UNRESOLVED};
use crate::session::validate::{validate_context_key, validate_session_id};
use crate::session::{state_file_name, SessionInfo};

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

// ---------------------------------------------------------------------------
// The layout marker
// ---------------------------------------------------------------------------

/// Why [`check_layout`] refused a store. Neither variant holds any of the
/// marker's bytes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LayoutError {
    /// The marker names a layout newer than this reader reads.
    #[error(
        "session store layout marker {} says layout {found}; this reader supports layout {supported}",
        path.display()
    )]
    Newer {
        /// The marker's path.
        path: PathBuf,
        /// The layout the marker names.
        found: u64,
        /// The layout this reader reads, [`STORE_LAYOUT_VERSION`].
        supported: u32,
    },
    /// The marker can't be read as a layout number. `category` is one of
    /// "not a regular file", "a symlink", "larger than 64 bytes", "not a
    /// layout number" and "unreadable".
    #[error("session store layout marker {} is unusable: {category}", path.display())]
    Unusable {
        /// The marker's path.
        path: PathBuf,
        /// What is wrong with it, a fixed string.
        category: &'static str,
    },
}

/// The layout number a marker's bytes name: UTF-8 digits, surrounding
/// whitespace allowed. The error is a fixed category string.
fn parse_layout_marker(bytes: &[u8]) -> Result<u64, &'static str> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
        .ok_or("not a layout number")
}

/// The category a refused marker read falls in.
fn marker_read_category(err: &std::io::Error) -> &'static str {
    #[cfg(unix)]
    if err.raw_os_error() == Some(libc::ELOOP) {
        return "a symlink";
    }
    match err.kind() {
        std::io::ErrorKind::InvalidInput => "not a regular file",
        std::io::ErrorKind::InvalidData => "larger than 64 bytes",
        _ => "unreadable",
    }
}

/// Check that this reader reads the store's layout.
///
/// Reads `<sessions_dir>/.koto-store-layout` through the hardened read,
/// capped at [`LAYOUT_MARKER_READ_CAP`] bytes. A missing marker is layout 1
/// (nothing writes one yet). A marker naming a layout up to
/// [`STORE_LAYOUT_VERSION`] passes; a higher one, or a marker that is a
/// symlink, not a regular file, over the cap or not a number, refuses the
/// store. Never blocks: a FIFO at the marker's path opens without waiting
/// and is refused.
pub fn check_layout(sessions_dir: &Path) -> Result<(), LayoutError> {
    let path = sessions_dir.join(STORE_LAYOUT_MARKER);
    let bytes = match read_store_file(
        &path,
        LAYOUT_MARKER_READ_CAP,
        ReadMode::InStore { root: sessions_dir },
    ) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(LayoutError::Unusable {
                category: marker_read_category(&e),
                path,
            })
        }
    };
    let found = parse_layout_marker(&bytes).map_err(|category| LayoutError::Unusable {
        path: path.clone(),
        category,
    })?;
    if found > u64::from(STORE_LAYOUT_VERSION) {
        return Err(LayoutError::Newer {
            path,
            found,
            supported: STORE_LAYOUT_VERSION,
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The hardened listing
// ---------------------------------------------------------------------------

/// What reading one session directory's state-file header found.
enum ListedHeader {
    /// No state file: the directory isn't a session.
    Missing,
    /// A state file that was refused or whose header doesn't parse.
    Unreadable,
    /// The header.
    Read(Box<StateFileHeader>),
}

/// Read the first line of `<sessions_dir>/<name>/koto-<name>.state.jsonl`
/// through the hardened read and parse it as a header.
fn listed_header(sessions_dir: &Path, name: &str) -> ListedHeader {
    let path = sessions_dir.join(name).join(state_file_name(name));
    match read_store_first_line(
        &path,
        STATE_LOG_READ_CAP,
        ReadMode::InStore { root: sessions_dir },
    ) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ListedHeader::Missing,
        Err(_) => ListedHeader::Unreadable,
        Ok(line) => match parse_header_bytes(&line) {
            Ok(header) => ListedHeader::Read(Box::new(header)),
            Err(_) => ListedHeader::Unreadable,
        },
    }
}

/// Every session under `sessions_dir` whose state-file header reads,
/// sorted by id.
///
/// The filters are koto's own listing's: a missing `sessions_dir` lists
/// nothing; only real directories count (a symlinked session directory is
/// passed by); names that aren't UTF-8 and names starting with
/// [`STAGING_PREFIX`] are passed by; a directory with no state file isn't a
/// session. Each header is read through the hardened first-line read, so a
/// FIFO, a symlink or a file over [`STATE_LOG_READ_CAP`] at a state file's
/// path never blocks the listing: that session is left out, and
/// [`count_unreadable_sessions`] counts it, as it counts a header that
/// doesn't parse or names a newer `schema_version`. Names holding `~` are
/// listed; the dashboard scan drops them.
///
/// `template_source_status` is always `None`: the listing reads nothing
/// beyond the header.
pub fn list_sessions(sessions_dir: &Path) -> anyhow::Result<Vec<SessionInfo>> {
    let mut results = Vec::new();
    let entries = match std::fs::read_dir(sessions_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(results),
        Err(e) => {
            return Err(anyhow::anyhow!(
                "failed to read directory {}: {}",
                sessions_dir.display(),
                e
            ))
        }
    };

    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if name.starts_with(STAGING_PREFIX) {
            continue;
        }
        if let ListedHeader::Read(header) = listed_header(sessions_dir, &name) {
            let header = *header;
            let session_id = (!header.session_id.is_empty()).then_some(header.session_id);
            results.push(SessionInfo {
                id: name,
                created_at: header.created_at,
                template_hash: header.template_hash,
                parent_workflow: header.parent_workflow,
                template_source_status: None,
                execution_dir: header.execution_dir,
                session_id,
                kind: header.kind,
            });
        }
    }

    results.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(results)
}

/// How many session directories under `sessions_dir` hold a state file
/// [`list_sessions`] leaves out: one the hardened read refuses, or whose
/// header doesn't parse.
///
/// Walks with [`list_sessions`]'s filters and also passes by names holding
/// `~`, as the dashboard scan does. A directory with no state file isn't
/// counted, and a `sessions_dir` that can't be read counts 0.
pub fn count_unreadable_sessions(sessions_dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(sessions_dir) else {
        return 0;
    };
    let mut count = 0;
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if name.starts_with(STAGING_PREFIX) || name.contains('~') {
            continue;
        }
        if let ListedHeader::Unreadable = listed_header(sessions_dir, &name) {
            count += 1;
        }
    }
    count
}

/// A sessions directory read as a [`SessionSource`], for a program other
/// than koto: the hardened listing over one directory, opened only once its
/// layout marker passes [`check_layout`].
#[derive(Debug, Clone)]
pub struct LocalDirSource {
    sessions_dir: PathBuf,
}

impl LocalDirSource {
    /// Open `sessions_dir`, refusing a store whose layout marker this
    /// reader doesn't read. The directory need not exist (it lists
    /// nothing).
    pub fn new(sessions_dir: impl Into<PathBuf>) -> Result<Self, LayoutError> {
        let sessions_dir = sessions_dir.into();
        check_layout(&sessions_dir)?;
        Ok(Self { sessions_dir })
    }
}

impl SessionSource for LocalDirSource {
    fn sessions_dir(&self) -> &Path {
        &self.sessions_dir
    }

    fn list(&self) -> anyhow::Result<Vec<SessionInfo>> {
        list_sessions(&self.sessions_dir)
    }

    fn session_dir(&self, id: &str) -> PathBuf {
        self.sessions_dir.join(id)
    }

    fn count_unreadable(&self) -> usize {
        count_unreadable_sessions(&self.sessions_dir)
    }
}

// ---------------------------------------------------------------------------
// One session's log
// ---------------------------------------------------------------------------

/// Why [`read_session_events`] couldn't read a session's log: a fixed
/// category and the log's path. It holds none of the log's bytes, and its
/// `Display` never carries a parser's or the OS's error text, which can
/// quote them.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{category}: {}", path.display())]
pub struct SessionReadError {
    /// What went wrong.
    pub category: SessionReadCategory,
    /// The state log's path.
    pub path: PathBuf,
}

/// The fixed categories a [`SessionReadError`] falls in. Each displays as
/// a fixed string; [`NewerSchema`](Self::NewerSchema) adds its two numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionReadCategory {
    /// The id isn't a session name, so no path inside the store is built
    /// from it.
    InvalidId,
    /// No state log at the path.
    NotFound,
    /// The state log is a symlink.
    Symlink,
    /// The state log isn't a regular file (a FIFO, a device, a directory).
    NotRegular,
    /// The state log is larger than [`STATE_LOG_READ_CAP`].
    TooLarge,
    /// The state log's path resolves outside the sessions directory.
    OutsideStore,
    /// The state log couldn't be read for another reason.
    Unreadable,
    /// The state log doesn't parse.
    Malformed,
    /// The state log was written with a newer `schema_version` than this
    /// reader reads.
    NewerSchema {
        /// The log's `schema_version`.
        found: u32,
        /// The newest `schema_version` this reader reads.
        supported: u32,
    },
}

impl fmt::Display for SessionReadCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidId => f.write_str("session id is not a session name"),
            Self::NotFound => f.write_str("state log not found"),
            Self::Symlink => f.write_str("state log is a symlink"),
            Self::NotRegular => f.write_str("state log is not a regular file"),
            Self::TooLarge => f.write_str("state log is larger than 512 MiB"),
            Self::OutsideStore => f.write_str("state log resolves outside the sessions directory"),
            Self::Unreadable => f.write_str("state log is unreadable"),
            Self::Malformed => f.write_str("state log is malformed"),
            Self::NewerSchema { found, supported } => write!(
                f,
                "state log has schema_version {found}; this reader supports up to {supported}"
            ),
        }
    }
}

/// The category a refused state-log read falls in.
fn log_read_category(err: &std::io::Error) -> SessionReadCategory {
    #[cfg(unix)]
    if err.raw_os_error() == Some(libc::ELOOP) {
        return SessionReadCategory::Symlink;
    }
    match err.kind() {
        std::io::ErrorKind::NotFound => SessionReadCategory::NotFound,
        std::io::ErrorKind::InvalidInput => SessionReadCategory::NotRegular,
        std::io::ErrorKind::InvalidData => SessionReadCategory::TooLarge,
        std::io::ErrorKind::PermissionDenied => SessionReadCategory::OutsideStore,
        _ => SessionReadCategory::Unreadable,
    }
}

/// The category a state log that won't parse falls in.
fn log_parse_category(err: &anyhow::Error) -> SessionReadCategory {
    match err.downcast_ref::<EngineError>() {
        Some(EngineError::IncompatibleSchemaVersion {
            found,
            max_supported,
        }) => SessionReadCategory::NewerSchema {
            found: *found,
            supported: *max_supported,
        },
        _ => SessionReadCategory::Malformed,
    }
}

/// Read and parse session `id`'s state log from `source`.
///
/// `id` is checked against the session-name grammar first. The log is
/// `<source.session_dir(id)>/koto-<id>.state.jsonl`, read through the
/// hardened read inside `source.sessions_dir()` and capped at
/// [`STATE_LOG_READ_CAP`], so a FIFO, a symlink or an over-cap file at the
/// path is refused without blocking. The bytes are parsed as koto's own
/// commands parse a log, a torn final line recovered quietly. Takes no lock
/// and writes nothing.
pub fn read_session_events(
    source: &dyn SessionSource,
    id: &str,
) -> Result<(StateFileHeader, Vec<Event>), SessionReadError> {
    let path = source.session_dir(id).join(state_file_name(id));
    let refuse = |category| SessionReadError {
        category,
        path: path.clone(),
    };
    validate_session_id(id).map_err(|_| refuse(SessionReadCategory::InvalidId))?;
    let bytes = read_store_file(
        &path,
        STATE_LOG_READ_CAP,
        ReadMode::InStore {
            root: source.sessions_dir(),
        },
    )
    .map_err(|e| refuse(log_read_category(&e)))?;
    parse_state_log(&bytes).map_err(|e| refuse(log_parse_category(&e)))
}

// ---------------------------------------------------------------------------
// One context value
// ---------------------------------------------------------------------------

/// What [`read_context`] found for one key.
///
/// A reason is a fixed category string and a path (or, for
/// [`Unusable`](Self::Unusable), the key asked for), never bytes from the
/// store. `Debug` prints a [`Present`](Self::Present) value's length and
/// source, never its bytes.
#[derive(Clone, PartialEq, Eq)]
pub enum ContextRead {
    /// The key can't name a context value: it fails the key grammar, or is
    /// one of koto's own files directly under `ctx/`.
    Unusable(String),
    /// No value: `ctx/`, an intermediate directory or the value's file
    /// doesn't exist.
    Absent,
    /// A value is there but this reader won't read it: a symlink at any
    /// component, a leaf that isn't a regular file, a value over
    /// [`CONTEXT_VALUE_READ_CAP`], or an I/O error.
    Unreadable(String),
    /// The value, whole.
    Present {
        /// The value's bytes, unredacted.
        bytes: Vec<u8>,
        /// Where they came from.
        from: ContextSource,
    },
}

impl fmt::Debug for ContextRead {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unusable(reason) => f.debug_tuple("Unusable").field(reason).finish(),
            Self::Absent => f.write_str("Absent"),
            Self::Unreadable(reason) => f.debug_tuple("Unreadable").field(reason).finish(),
            Self::Present { bytes, from } => f
                .debug_struct("Present")
                .field("len", &bytes.len())
                .field("from", from)
                .finish(),
        }
    }
}

/// Where a [`ContextRead::Present`] value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextSource {
    /// A transition assigned the key and nothing in the log has superseded
    /// it since, so the value is what the log says, whatever `ctx/` holds.
    Log,
    /// The value's file under `ctx/`.
    Store,
}

/// `Unreadable` with a fixed category and the path it applies to.
fn unreadable(category: &str, path: &Path) -> ContextRead {
    ContextRead::Unreadable(format!("{category}: {}", path.display()))
}

/// What a directory on the way to a context value says, `None` when it is
/// a real directory the walk can go into.
fn check_context_dir(dir: &Path) -> Option<ContextRead> {
    match std::fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(ContextRead::Absent),
        Err(_) => Some(unreadable("context value path is unreadable", dir)),
        Ok(meta) if meta.file_type().is_symlink() => {
            Some(unreadable("context value path holds a symlink", dir))
        }
        Ok(meta) if !meta.is_dir() => {
            Some(unreadable("context value path holds a non-directory", dir))
        }
        Ok(_) => None,
    }
}

/// The category a refused context-value read falls in.
fn context_read_category(err: &std::io::Error) -> &'static str {
    #[cfg(unix)]
    if err.raw_os_error() == Some(libc::ELOOP) {
        return "context value path holds a symlink";
    }
    match err.kind() {
        std::io::ErrorKind::InvalidInput => "context value is not a regular file",
        std::io::ErrorKind::InvalidData => "context value is larger than 16 MiB",
        std::io::ErrorKind::PermissionDenied => {
            "context value resolves outside the session's context directory"
        }
        _ => "context value is unreadable",
    }
}

/// Read context key `key` of the session in `session_dir`, as `koto
/// context get` answers it, taking no lock, recording no read and writing
/// nothing.
///
/// In order:
///
/// 1. A key that fails the context-key grammar, or is `manifest.json` or
///    `manifest.lock` (koto's own files directly under `ctx/`), is
///    [`ContextRead::Unusable`].
/// 2. Given the session's `events` (from [`read_session_events`]), a key
///    whose latest writer in the log is a transition's assignment is
///    [`ContextRead::Present`] from [`ContextSource::Log`], the assigned
///    value, whatever `ctx/` holds.
/// 3. `ctx/` and each directory down to the value's file are checked with
///    `symlink_metadata`: a missing one is [`ContextRead::Absent`]; a
///    symlink at any component, an intermediate that isn't a directory, or
///    a directory where the value's file belongs is
///    [`ContextRead::Unreadable`].
/// 4. The file is read through the hardened read, capped at
///    [`CONTEXT_VALUE_READ_CAP`] and contained in the session's `ctx/`: a
///    FIFO or device is refused without blocking.
///
/// Refusing symlinks is a deliberate difference from `koto context get`,
/// which follows them; koto never writes one. Two limits: a read racing
/// koto's unlocked content write can see a torn value, as `koto context
/// get` can, and under the cloud backend the value is this host's mirror.
pub fn read_context(session_dir: &Path, key: &str, events: Option<&[Event]>) -> ContextRead {
    if validate_context_key(key).is_err() {
        return ContextRead::Unusable(format!("context key fails the key grammar: {key:?}"));
    }
    if key == MANIFEST_FILE || key == MANIFEST_LOCK_FILE {
        return ContextRead::Unusable(format!(
            "context key names one of koto's own files: {key:?}"
        ));
    }

    if let Some(value) = events.and_then(|events| outstanding_assignments(events).remove(key)) {
        return ContextRead::Present {
            bytes: value.into_bytes(),
            from: ContextSource::Log,
        };
    }

    let ctx = session_dir.join(CTX_DIR);
    let mut components: Vec<&str> = key.split('/').collect();
    let Some(leaf_name) = components.pop() else {
        return ContextRead::Unusable(format!("context key fails the key grammar: {key:?}"));
    };

    // `ctx/` and each intermediate directory must exist as a directory, not
    // a symlink to one.
    let mut dir = ctx.clone();
    if let Some(refused) = check_context_dir(&dir) {
        return refused;
    }
    for name in components {
        dir.push(name);
        if let Some(refused) = check_context_dir(&dir) {
            return refused;
        }
    }

    let leaf = dir.join(leaf_name);
    match std::fs::symlink_metadata(&leaf) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ContextRead::Absent,
        Err(_) => return unreadable("context value is unreadable", &leaf),
        Ok(meta) if meta.file_type().is_symlink() => {
            return unreadable("context value path holds a symlink", &leaf)
        }
        Ok(meta) if meta.is_dir() => return unreadable("context value is a directory", &leaf),
        Ok(_) => {}
    }

    match read_store_file(
        &leaf,
        CONTEXT_VALUE_READ_CAP,
        ReadMode::InStore { root: &ctx },
    ) {
        Ok(bytes) => ContextRead::Present {
            bytes,
            from: ContextSource::Store,
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ContextRead::Absent,
        Err(e) => unreadable(context_read_category(&e), &leaf),
    }
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

    /// One log line of type `event_type` with `payload`, parsed as the
    /// reader parses a state log.
    fn event(seq: u64, event_type: &str, payload: &str) -> crate::engine::types::Event {
        let line = format!(
            r#"{{"seq":{seq},"timestamp":"2026-05-24T00:00:00Z","type":"{event_type}","payload":{payload}}}"#
        );
        serde_json::from_str(&line).unwrap()
    }

    /// An event of a listed type, with the payload it carries in a state log;
    /// each touches key `k`.
    fn listed_event(seq: u64, event_type: &str) -> crate::engine::types::Event {
        let payload = match event_type {
            "transitioned" => {
                r#"{"from":"a","to":"b","condition_type":"auto","context_assignments":{"k":"v2"}}"#
            }
            "context_added" => r#"{"key":"k","hash":"00","size":0}"#,
            "context_removed" => r#"{"key":"k"}"#,
            "context_cleared" => r#"{"state":"b","keys":["k"],"entry_seq":1}"#,
            other => panic!("CONTEXT_KEY_EVENTS lists {other}, which this test can't build"),
        };
        event(seq, event_type, payload)
    }

    #[test]
    fn each_context_key_event_changes_the_outstanding_assignments() {
        use crate::engine::context_assign::outstanding_assignments;

        // A transition that assigned `k = v1`: the outstanding set holds it.
        let base = vec![event(
            1,
            "transitioned",
            r#"{"from":null,"to":"a","condition_type":"auto","context_assignments":{"k":"v1"}}"#,
        )];
        let before = outstanding_assignments(&base);
        assert_eq!(before.get("k").map(String::as_str), Some("v1"));

        for event_type in CONTEXT_KEY_EVENTS {
            let listed = listed_event(2, event_type);
            assert_eq!(
                listed.payload.type_name(),
                event_type,
                "CONTEXT_KEY_EVENTS names {event_type}, which must be the payload's type name"
            );
            let mut events = base.clone();
            events.push(listed);
            assert_ne!(
                outstanding_assignments(&events),
                before,
                "a {event_type} event must change the outstanding assignments"
            );
        }

        // A type the list doesn't name leaves the set as it was.
        assert!(!CONTEXT_KEY_EVENTS.contains(&"evidence_submitted"));
        let mut events = base.clone();
        events.push(event(
            2,
            "evidence_submitted",
            r#"{"state":"a","fields":{"k":"v3"}}"#,
        ));
        assert_eq!(events[1].payload.type_name(), "evidence_submitted");
        assert_eq!(outstanding_assignments(&events), before);
    }

    #[test]
    fn parse_layout_marker_reads_a_number_with_surrounding_whitespace() {
        assert_eq!(parse_layout_marker(b"1"), Ok(1));
        assert_eq!(parse_layout_marker(b"1\n"), Ok(1));
        assert_eq!(parse_layout_marker(b" 2 \r\n"), Ok(2));
        assert_eq!(parse_layout_marker(b"0"), Ok(0));
        assert_eq!(parse_layout_marker(b"18446744073709551615"), Ok(u64::MAX));
    }

    #[test]
    fn parse_layout_marker_refuses_anything_else() {
        for bytes in [
            &b""[..],
            b"\n",
            b"abc",
            b"-1",
            b"+",
            b"1.0",
            b"1 2",
            b"0x1",
            b"18446744073709551616",
            &[b'1', 0xff],
        ] {
            assert_eq!(
                parse_layout_marker(bytes),
                Err("not a layout number"),
                "{bytes:?}"
            );
        }
    }

    #[test]
    fn a_refused_marker_read_falls_in_a_fixed_category() {
        use std::io::{Error, ErrorKind};
        #[cfg(unix)]
        assert_eq!(
            marker_read_category(&Error::from_raw_os_error(libc::ELOOP)),
            "a symlink"
        );
        let category = |kind| marker_read_category(&Error::new(kind, "ZQX-PLANTED"));
        assert_eq!(category(ErrorKind::InvalidInput), "not a regular file");
        assert_eq!(category(ErrorKind::InvalidData), "larger than 64 bytes");
        assert_eq!(category(ErrorKind::PermissionDenied), "unreadable");
        assert_eq!(category(ErrorKind::Other), "unreadable");
    }

    #[test]
    fn context_read_debug_shows_a_present_value_length_and_source_only() {
        let read = ContextRead::Present {
            bytes: b"ZQX-PLANTED-7F3A".to_vec(),
            from: ContextSource::Store,
        };
        let debug = format!("{read:?}");
        assert_eq!(debug, "Present { len: 16, from: Store }");
        let from_log = ContextRead::Present {
            bytes: b"ZQX".to_vec(),
            from: ContextSource::Log,
        };
        assert_eq!(format!("{from_log:?}"), "Present { len: 3, from: Log }");
        assert_eq!(format!("{:?}", ContextRead::Absent), "Absent");
        assert_eq!(
            format!("{:?}", ContextRead::Unreadable("r".into())),
            r#"Unreadable("r")"#
        );
        assert_eq!(
            format!("{:?}", ContextRead::Unusable("u".into())),
            r#"Unusable("u")"#
        );
    }

    #[test]
    fn read_context_refuses_keys_that_name_no_value_before_any_io() {
        // A session directory that doesn't exist: an unusable key is
        // refused without looking at it.
        let dir = Path::new("/nonexistent-koto-session-dir");
        for key in [
            "",
            "../x",
            "a//b",
            "/a",
            "a/",
            "a/./b",
            MANIFEST_FILE,
            MANIFEST_LOCK_FILE,
        ] {
            assert!(
                matches!(read_context(dir, key, None), ContextRead::Unusable(_)),
                "{key:?}"
            );
        }
        // A nested manifest.json is an ordinary key.
        assert_eq!(
            read_context(dir, "a/manifest.json", None),
            ContextRead::Absent
        );
        assert_eq!(read_context(dir, "k", None), ContextRead::Absent);
    }

    #[test]
    fn read_context_answers_from_the_log_before_the_store() {
        let events = vec![event(
            1,
            "transitioned",
            r#"{"from":null,"to":"a","condition_type":"auto","context_assignments":{"k":"v1"}}"#,
        )];
        let dir = Path::new("/nonexistent-koto-session-dir");
        assert_eq!(
            read_context(dir, "k", Some(&events)),
            ContextRead::Present {
                bytes: b"v1".to_vec(),
                from: ContextSource::Log,
            }
        );
        assert_eq!(
            read_context(dir, "other", Some(&events)),
            ContextRead::Absent
        );
        // A later add supersedes the assignment: the store answers.
        let mut events = events;
        events.push(listed_event(2, "context_added"));
        assert_eq!(read_context(dir, "k", Some(&events)), ContextRead::Absent);
    }

    #[test]
    fn a_refused_log_read_falls_in_a_fixed_category() {
        use std::io::{Error, ErrorKind};
        #[cfg(unix)]
        assert_eq!(
            log_read_category(&Error::from_raw_os_error(libc::ELOOP)),
            SessionReadCategory::Symlink
        );
        let category = |kind| log_read_category(&Error::new(kind, "ZQX-PLANTED"));
        assert_eq!(category(ErrorKind::NotFound), SessionReadCategory::NotFound);
        assert_eq!(
            category(ErrorKind::InvalidInput),
            SessionReadCategory::NotRegular
        );
        assert_eq!(
            category(ErrorKind::InvalidData),
            SessionReadCategory::TooLarge
        );
        assert_eq!(
            category(ErrorKind::PermissionDenied),
            SessionReadCategory::OutsideStore
        );
        assert_eq!(category(ErrorKind::Other), SessionReadCategory::Unreadable);
    }

    #[test]
    fn a_log_that_will_not_parse_falls_in_a_fixed_category() {
        let newer = parse_state_log(
            br#"{"schema_version":99,"workflow":"w","template_hash":"h","created_at":"2026-05-24T00:00:00Z"}"#,
        )
        .unwrap_err();
        assert_eq!(
            log_parse_category(&newer),
            SessionReadCategory::NewerSchema {
                found: 99,
                supported: crate::engine::types::CURRENT_SCHEMA_VERSION,
            }
        );
        let garbled = parse_state_log(b"{\"ZQX-PLANTED\n").unwrap_err();
        assert_eq!(log_parse_category(&garbled), SessionReadCategory::Malformed);
    }

    #[test]
    fn session_read_error_display_is_the_category_and_the_path() {
        let err = SessionReadError {
            category: SessionReadCategory::Malformed,
            path: PathBuf::from("/s/w/koto-w.state.jsonl"),
        };
        assert_eq!(
            err.to_string(),
            "state log is malformed: /s/w/koto-w.state.jsonl"
        );
        let newer = SessionReadError {
            category: SessionReadCategory::NewerSchema {
                found: 4,
                supported: 3,
            },
            path: PathBuf::from("/p"),
        };
        assert_eq!(
            newer.to_string(),
            "state log has schema_version 4; this reader supports up to 3: /p"
        );
    }

    #[test]
    fn layout_error_display_names_the_path_and_the_numbers() {
        let path = PathBuf::from("/s/.koto-store-layout");
        let newer = LayoutError::Newer {
            path: path.clone(),
            found: 2,
            supported: STORE_LAYOUT_VERSION,
        };
        assert_eq!(
            newer.to_string(),
            "session store layout marker /s/.koto-store-layout says layout 2; \
             this reader supports layout 1"
        );
        let unusable = LayoutError::Unusable {
            path,
            category: "a symlink",
        };
        assert_eq!(
            unusable.to_string(),
            "session store layout marker /s/.koto-store-layout is unusable: a symlink"
        );
    }
}
