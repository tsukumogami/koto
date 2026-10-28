pub mod anchor;
pub mod batch;
pub mod cloud;
pub mod context;
pub mod context_log;
pub mod continues_bucket;
pub mod duration;
pub mod finished;
pub mod hierarchy;
pub mod import_links;
pub mod inventory;
pub mod links;
pub mod local;
pub mod needed;
pub mod pin;
pub mod read_spool;
pub mod rebind_fence;
pub mod recover;
pub mod state_log;
pub mod sync;
pub mod sync_stamp;
pub mod validate;
pub mod version;

use std::fmt;
use std::path::PathBuf;

use self::context::ContextStore;
use self::local::LocalBackend;

use crate::engine::template_source_status::TemplateSourceStatus;
use crate::engine::types::{Event, EventPayload, StateFileHeader};

/// Typed error for session-scoped operations.
///
/// Introduced primarily so callers of `SessionBackend::init_state_file`
/// can discriminate a collision (state file already present at the
/// target path) from other I/O failures without relying on downcasting
/// an `anyhow::Error`. The collision variant lets upstream batch-spawn
/// logic map "someone else already initialised this workflow" to a
/// specific outcome (e.g., respawn or skip) while treating other I/O
/// failures as retryable infrastructure errors.
#[derive(Debug)]
pub enum SessionError {
    /// A state file already exists at the target path. Emitted when the
    /// atomic create-or-fail rename sees the destination occupied.
    Collision,

    /// Another process or thread already holds the advisory flock on
    /// the target state file. Emitted when
    /// `SessionBackend::lock_state_file` attempts a non-blocking
    /// `LOCK_EX | LOCK_NB` and the kernel reports `EWOULDBLOCK`.
    ///
    /// `holder_pid` is populated on a best-effort basis. Today this
    /// variant is surfaced directly by the flock `EWOULDBLOCK` path
    /// which carries no peer-pid information, so the field is always
    /// `None`; it is reserved so a future `F_OFD_GETLK`-based probe can
    /// attach the holder's PID without a breaking change to the API.
    Locked { holder_pid: Option<u32> },

    /// An I/O error from the underlying storage backend that isn't a
    /// collision. Preserves the original `io::Error` so callers can
    /// inspect its `kind()` for retry decisions.
    Io(std::io::Error),

    /// Fallback for failures that don't fit the variants above.
    Other(anyhow::Error),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionError::Collision => {
                write!(f, "state file already exists at the target path")
            }
            SessionError::Locked { holder_pid } => match holder_pid {
                Some(pid) => {
                    write!(f, "state file is already locked by pid {}", pid)
                }
                None => write!(f, "state file is already locked by another process"),
            },
            SessionError::Io(e) => write!(f, "session I/O error: {}", e),
            SessionError::Other(e) => write!(f, "session error: {}", e),
        }
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SessionError::Io(e) => Some(e),
            SessionError::Other(e) => Some(e.as_ref()),
            SessionError::Collision | SessionError::Locked { .. } => None,
        }
    }
}

impl From<std::io::Error> for SessionError {
    fn from(e: std::io::Error) -> Self {
        // ErrorKind::AlreadyExists is the canonical signal for "the
        // create-or-fail rename saw the destination occupied". Map it
        // to the dedicated Collision variant so callers don't have to
        // peek at `kind()` themselves.
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            SessionError::Collision
        } else {
            SessionError::Io(e)
        }
    }
}

/// A cloud read refused because the session was imported elsewhere.
///
/// `koto session import` leaves a `migrated.json` object beside the source
/// session's remote state file. The cloud backend checks for it before it
/// reads the session's state, and returns this error when it is there, so
/// the old copy refuses instead of forking the session.
///
/// The message begins with the error code, `session_migrated`, so every
/// command's existing error path carries it; `koto next` and `koto status`
/// downcast to this type to choose their exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMigrated {
    /// The session that was read.
    pub name: String,
    /// The session it was imported as.
    pub target: String,
    /// The workspace path the import ran in.
    pub workspace: String,
}

impl fmt::Display for SessionMigrated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "session_migrated: session '{}' was migrated to '{}' in {}; continue it there",
            self.name, self.target, self.workspace
        )
    }
}

impl std::error::Error for SessionMigrated {}

/// Where a session's own copy lives in the bucket: the prefix every key of
/// the session is built under, from
/// [`crate::session::cloud::CloudBackend::home_prefix`].
///
/// A session's own copy is the one under the prefix recorded when it was
/// created (`origin.store.prefix`), whichever directory a command runs in.
/// The local backend has no prefix: every session is in its own workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    /// The bucket prefix of the session's own copy; empty on the local
    /// backend.
    pub prefix: String,
    /// Whether `prefix` is the prefix of the directory the command runs in.
    /// Never for a [`HomeSource::Located`] home, whatever its prefix.
    /// A copy found any other way is read and written strictly: a failed
    /// bucket request is an error, never a fall-back to the local copy, and
    /// a push never replaces another session's copy there.
    pub own_workspace: bool,
    /// How the prefix was found.
    pub via: HomeSource,
}

/// How a session's [`Home`] was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeSource {
    /// The `origin.store.prefix` its local header records.
    Recorded,
    /// The prefix of the directory the command runs in: the local backend,
    /// a session with no local copy on this host, or one recording no
    /// prefix whose own directory this is.
    Caller,
    /// The one candidate holding a copy of a session that records no
    /// prefix, read from outside its own directory: its anchor's prefix or
    /// the prefix this host's sync stamp names. The copy is taken as the
    /// session's own only when it carries the local header's `session_id`,
    /// which the first pull checks.
    Located,
}

/// A command refused because koto can't tell which bucket copy is the
/// named session's own, so it reads and writes neither.
///
/// The message begins with the error code, `session_unlocated`, as
/// [`SessionMigrated`]'s does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unlocated {
    /// The session that was named.
    pub session: String,
    /// Why no copy could be taken as its own.
    pub reason: UnlocatedReason,
}

/// Why a session is [`Unlocated`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnlocatedReason {
    /// This host has no copy of the session, and the bucket holds none
    /// under the prefix of the directory the command runs in.
    NoLocalCopy {
        /// The prefix that was asked.
        prefix: String,
    },
    /// The session was created before koto recorded its prefix, the
    /// command runs outside the directory its records name, and its
    /// candidate copies didn't locate it ([`Fallback`]).
    NoRecordedPrefix {
        /// The directory the session is bound to, when its header names one.
        anchor: Option<String>,
        /// Why the candidates didn't locate it.
        fallback: Fallback,
    },
    /// The copy under the recorded prefix is missing, or is another
    /// session's (another `session_id`).
    RecordedCopyMismatch {
        /// The recorded prefix.
        prefix: String,
        /// The `session_id` the copy there carries; `None` when it is
        /// missing.
        found: Option<String>,
    },
}

/// Why the candidate copies of a session that records no prefix didn't
/// locate it. The candidates are its anchor's prefix and the prefix this
/// host's sync stamp names, listed once each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fallback {
    /// No candidate holds a copy of the session.
    NoCopy {
        /// The prefixes listed, in the order they were asked.
        candidates: Vec<String>,
    },
    /// Both candidates hold a copy, so neither can be taken as its own.
    Ambiguous {
        /// The two prefixes.
        prefixes: Vec<String>,
    },
    /// The one candidate's copy carries another `session_id`, or one side
    /// carries none to confirm it by.
    Mismatch {
        /// The candidate.
        prefix: String,
        /// The `session_id` the copy there carries, when it carries one.
        found: Option<String>,
    },
    /// A candidate's listing failed, so koto can't tell what it holds.
    Unreadable {
        /// The candidate.
        prefix: String,
        /// What the bucket answered.
        error: String,
    },
}

impl UnlocatedReason {
    /// The reason as the error JSON's `reason` field carries it.
    pub fn as_str(&self) -> &'static str {
        match self {
            UnlocatedReason::NoLocalCopy { .. } => "no_local_copy",
            UnlocatedReason::RecordedCopyMismatch { .. } => "recorded_copy_mismatch",
            UnlocatedReason::NoRecordedPrefix { fallback, .. } => match fallback {
                Fallback::NoCopy { .. } => "no_candidate_copy",
                Fallback::Ambiguous { .. } => "ambiguous_copies",
                Fallback::Mismatch { .. } => "candidate_copy_mismatch",
                Fallback::Unreadable { .. } => "candidate_unreadable",
            },
        }
    }

    /// The bucket prefixes the reason names, for the error JSON's
    /// `locations` field.
    pub fn locations(&self) -> Vec<String> {
        match self {
            UnlocatedReason::NoLocalCopy { prefix }
            | UnlocatedReason::RecordedCopyMismatch { prefix, .. } => vec![prefix.clone()],
            UnlocatedReason::NoRecordedPrefix { fallback, .. } => match fallback {
                Fallback::NoCopy { candidates } => candidates.clone(),
                Fallback::Ambiguous { prefixes } => prefixes.clone(),
                Fallback::Mismatch { prefix, .. } | Fallback::Unreadable { prefix, .. } => {
                    vec![prefix.clone()]
                }
            },
        }
    }
}

impl fmt::Display for Unlocated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = &self.session;
        match &self.reason {
            UnlocatedReason::NoLocalCopy { prefix } => write!(
                f,
                "session_unlocated: session '{name}' has no copy on this host and the bucket \
                 holds none under this directory's prefix {prefix}, so nothing was read; run \
                 the command from the directory the session was started in"
            ),
            UnlocatedReason::NoRecordedPrefix { anchor, fallback } => {
                write!(
                    f,
                    "session_unlocated: session '{name}' records no prefix for its own copy, "
                )?;
                match fallback {
                    Fallback::NoCopy { candidates } => write!(
                        f,
                        "and the bucket holds no copy of it under {}",
                        candidates.join(" or ")
                    )?,
                    Fallback::Ambiguous { prefixes } => write!(
                        f,
                        "and the bucket holds a copy of it under both {}, and koto can't tell \
                         which is its own",
                        prefixes.join(" and ")
                    )?,
                    Fallback::Mismatch {
                        prefix,
                        found: Some(id),
                    } => write!(
                        f,
                        "and the copy under prefix {prefix} is another session's (session_id \
                         {id})"
                    )?,
                    Fallback::Mismatch {
                        prefix,
                        found: None,
                    } => write!(
                        f,
                        "and the copy under prefix {prefix} can't be confirmed as its own, \
                         since one side carries no session_id"
                    )?,
                    Fallback::Unreadable { prefix, error } => write!(
                        f,
                        "and listing its copy under prefix {prefix} failed ({error})"
                    )?,
                }
                let at = anchor
                    .as_deref()
                    .unwrap_or("the directory the session was started in");
                write!(
                    f,
                    ", so nothing was read or written; run the command from {at}, where it reads \
                     as before"
                )
            }
            UnlocatedReason::RecordedCopyMismatch {
                prefix,
                found: None,
            } => write!(
                f,
                "session_unlocated: session '{name}' records its copy under prefix {prefix}, \
                 but the bucket holds no copy of it there, so nothing was read or written"
            ),
            UnlocatedReason::RecordedCopyMismatch {
                prefix,
                found: Some(id),
            } => write!(
                f,
                "session_unlocated: session '{name}' records its copy under prefix {prefix}, \
                 but the copy there is another session's (session_id {id}), so nothing was \
                 read or written"
            ),
        }
    }
}

impl std::error::Error for Unlocated {}

/// A bounded prefix read of one context key's local content, from
/// [`Backend::read_context_prefix`].
#[derive(Debug, Clone)]
pub struct PrefixRead {
    /// At most the requested bound of leading bytes.
    pub bytes: Vec<u8>,
    /// Whether `bytes` is the whole file.
    pub complete: bool,
}

/// RAII guard for an advisory `flock(LOCK_EX)` held on a session's
/// state file.
///
/// Acquired via [`SessionBackend::lock_state_file`]. The lock is
/// released automatically when the guard is dropped (the underlying
/// file handle closes, which the kernel converts into a `LOCK_UN`).
/// Process death also releases the lock through the same kernel
/// mechanism, so a crash between acquisition and drop cannot strand
/// the parent state file.
///
/// The guard owns the open `File`. Callers should hold it for the
/// duration of the critical section and let it drop at the end of
/// scope; there is no explicit `release` method because the guard's
/// entire job is to tie the lock lifetime to the Rust scope.
#[cfg(unix)]
#[derive(Debug)]
pub struct SessionLock {
    // The field is intentionally read-only and unused outside the
    // Drop impl: its sole purpose is to keep the file descriptor open
    // so the kernel-level flock remains held.
    _file: std::fs::File,
    // Debug builds record the holder so an idempotent append on the same
    // thread and file panics instead of deadlocking (see `lock_state_file`).
    #[cfg(debug_assertions)]
    _held: crate::engine::persistence::held_state_locks::Held,
}

/// Non-Unix fallback. koto does not support Windows today; this stub
/// lets downstream code reference the type without a `cfg(unix)` gate
/// everywhere, while `lock_state_file` itself returns
/// `SessionError::Other` on non-Unix targets.
#[cfg(not(unix))]
#[derive(Debug)]
pub struct SessionLock {
    _private: (),
}

/// Information about an existing session.
#[derive(serde::Serialize)]
pub struct SessionInfo {
    /// Session identifier (same as the workflow name).
    pub id: String,

    /// RFC 3339 UTC timestamp of session creation (from state file header).
    pub created_at: String,

    /// SHA-256 hash of the compiled template (from state file header).
    pub template_hash: String,

    /// Name of the parent workflow, if this workflow was created as a child.
    pub parent_workflow: Option<String>,

    /// Whether the session's recorded `template_source_dir` (if any) still
    /// resolves on this machine, and on whose.
    ///
    /// Populated from the state file header already read into memory while
    /// building this `SessionInfo` -- backends never perform extra I/O just
    /// to fill this field.
    ///
    /// `None` is ambiguous by design and conflates two distinct cases that
    /// this field does not (and is not required to) distinguish between:
    ///
    /// - The session's header never recorded a `template_source_dir` --
    ///   there is nothing to check.
    /// - No header was available to check at all. This is the case for
    ///   `CloudBackend`'s remote-only placeholder rows (a session that
    ///   exists in S3 but has not been synced locally): there is no header
    ///   in scope, so the field is unconditionally `None` there regardless
    ///   of what the session's real (remote) header might say once synced.
    ///
    /// This ambiguity is a known, accepted limitation -- not a bug to fix
    /// here. See DESIGN-orphaned-session-detection.md for the rationale.
    pub template_source_status: Option<TemplateSourceStatus>,

    /// The directory the session's ticks run in, from the header
    /// (`execution_dir`). `None` when the header records none, and for the
    /// cloud backend's remote-only placeholder rows, which have no header.
    pub execution_dir: Option<PathBuf>,

    /// The header's `session_id`. `None` for a header written before the
    /// field existed, and for remote-only placeholder rows.
    pub session_id: Option<String>,

    /// The header's `kind` (`anchor`, `anchor-close`), omitted for an
    /// ordinary session. `koto session list` shows it; the workflow views
    /// leave out every session that carries a kind koto knows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

impl SessionInfo {
    /// What the session's header says it is.
    pub fn session_kind(&self) -> crate::engine::types::SessionKind {
        crate::engine::types::SessionKind::of(self.kind.as_deref())
    }
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
    /// # Stability: Stage 1 — Frozen (Issue 19 / Decision 5)
    ///
    /// `create` is part of the four-method `SessionBackend` surface
    /// bunki BK2 imports. Breaking changes — signature changes,
    /// removal, or rename — require a 6-week deprecation window per
    /// `docs/STABILITY.md`. Adding NEW methods to the trait is
    /// permitted in minor releases (additive-only policy).
    ///
    /// Create a new session directory. Returns the path.
    fn create(&self, id: &str) -> anyhow::Result<PathBuf>;

    /// # Stability: additive-only (Issue 19 / Decision 5)
    ///
    /// Not part of the Stage 1 frozen four; downstream consumers
    /// should not depend on this method's exact signature across
    /// minor releases. Additive evolution of the trait is permitted.
    ///
    /// Return the session directory path (no I/O, just path computation).
    fn session_dir(&self, id: &str) -> PathBuf;

    /// # Stability: additive-only (Issue 19 / Decision 5)
    ///
    /// Not part of the Stage 1 frozen four; signature evolution is
    /// permitted in minor releases.
    ///
    /// Check if a session exists (state file present, not just directory).
    fn exists(&self, id: &str) -> bool;

    /// # Stability: additive-only (Issue 19 / Decision 5)
    ///
    /// Not part of the Stage 1 frozen four; signature evolution is
    /// permitted in minor releases.
    ///
    /// Remove all session artifacts. Idempotent on missing directories.
    fn cleanup(&self, id: &str) -> anyhow::Result<()>;

    /// # Stability: Stage 1 — Frozen (Issue 19 / Decision 5)
    ///
    /// `list` is part of the four-method `SessionBackend` surface
    /// bunki BK2 imports. Breaking changes require a 6-week
    /// deprecation window per `docs/STABILITY.md`.
    ///
    /// List all sessions with metadata extracted from state file headers.
    fn list(&self) -> anyhow::Result<Vec<SessionInfo>>;

    /// # Stability: additive-only (Issue 19 / Decision 5)
    ///
    /// Count session directories whose state file is present but whose header
    /// fails to parse. These sessions are silently excluded from [`list`], so
    /// surfacing them as a tally lets callers (e.g. the dashboard) report
    /// unreadable sessions instead of dropping them without a trace.
    ///
    /// The default implementation returns `0` for backends that cannot
    /// enumerate raw directories.
    ///
    /// [`list`]: SessionBackend::list
    fn count_unreadable(&self) -> usize {
        0
    }

    /// # Stability: additive-only
    ///
    /// The identity of this session store -- backend kind and canonical
    /// sessions directory -- recorded in a new session's origin record
    /// (`StateFileHeader.origin`) and compared by `koto init
    /// --attach-live`.
    ///
    /// The default returns `None` for backends with no stable identity;
    /// sessions they create carry no origin record, and `--attach-live`
    /// refuses them.
    fn store_identity(&self) -> Option<crate::engine::types::SessionStoreIdentity> {
        None
    }

    /// # Stability: additive-only (Issue 19 / Decision 5)
    ///
    /// Not part of the Stage 1 frozen four; signature evolution is
    /// permitted in minor releases.
    ///
    /// Append a header line to a new state file.
    fn append_header(&self, id: &str, header: &StateFileHeader) -> anyhow::Result<()>;

    /// # Stability: additive-only (Issue 19 / Decision 5)
    ///
    /// Not part of the Stage 1 frozen four; signature evolution is
    /// permitted in minor releases.
    ///
    /// Append an event to the state file.
    ///
    /// Fails, writing nothing, unless the log already exists and its first
    /// line is a header. Creating a session's log belongs to
    /// [`SessionBackend::init_state_file`]; an append that created one would
    /// leave a log no reader can parse (koto#236).
    fn append_event(&self, id: &str, payload: &EventPayload, timestamp: &str)
        -> anyhow::Result<()>;

    /// Append a `context_read` that its reader already judged to be `id`'s
    /// own (the CLI's `read_log_target` answered `Session`: the read ran in
    /// `id`'s own tick) to `id`'s log.
    ///
    /// [`SessionBackend::append_event`] on the cloud backend keeps a read
    /// made outside `id`'s own tick out of the log, in the read spool, since
    /// it can't tell an own read from another reader's; the reader can, so it
    /// says so here and the read goes into the log like any other event.
    /// Every other backend appends as `append_event` does.
    fn append_own_read(
        &self,
        id: &str,
        payload: &EventPayload,
        timestamp: &str,
    ) -> anyhow::Result<()> {
        self.append_event(id, payload, timestamp)
    }

    /// Atomically create a session's state file with the given header and
    /// initial events.
    ///
    /// Bundles the header line and all initial events into a single
    /// tempfile-then-rename operation. The rename is "fail if exists" so
    /// two racing callers on the same path cannot silently overwrite each
    /// other: exactly one wins and the other receives
    /// `SessionError::Collision`. Other I/O failures surface as
    /// `SessionError::Io` so callers can distinguish a racing spawn from
    /// a disk-full / permission / storage-layer error.
    ///
    /// A crash between the tempfile write and the rename leaves no
    /// partially-written state file at the target path. The tempfile
    /// itself may remain and is garbage-collected by a separate sweep.
    ///
    /// Implementations store the tempfile in the same directory as the
    /// target using the filename convention defined by
    /// `INIT_TMP_PREFIX` / `INIT_TMP_SUFFIX` in `session::local`. Sweep
    /// tooling that cleans up crashed initialisations relies on this
    /// convention.
    ///
    /// On Linux this uses `renameat2(RENAME_NOREPLACE)` for a single-
    /// syscall fail-if-exists check. On other Unixes it uses POSIX
    /// `link()` followed by `unlink()` of the tempfile.
    ///
    /// # Stability: Stage 1 — Frozen (Issue 19 / Decision 5)
    ///
    /// `init_state_file` is part of the four-method `SessionBackend`
    /// surface bunki BK2 imports. Breaking changes — signature
    /// changes, removal, or rename — require a 6-week deprecation
    /// window per `docs/STABILITY.md`.
    fn init_state_file(
        &self,
        id: &str,
        header: StateFileHeader,
        initial_events: Vec<Event>,
    ) -> Result<(), SessionError>;

    /// # Stability: Stage 1 — Frozen (Issue 19 / Decision 5)
    ///
    /// `read_events` is part of the four-method `SessionBackend`
    /// surface bunki BK2 imports. Breaking changes require a 6-week
    /// deprecation window per `docs/STABILITY.md`.
    ///
    /// Read all events from the state file.
    fn read_events(&self, id: &str) -> anyhow::Result<(StateFileHeader, Vec<Event>)>;

    /// # Stability: additive-only
    ///
    /// Read all events from the local copy of the state file, without any
    /// remote sync first.
    ///
    /// For a backend with no remote half this is [`read_events`]. A
    /// backend that pulls before reading (`CloudBackend`) overrides it to
    /// read only the local file, so a caller holding a lock (the decider's
    /// per-session `decider.lock`) never puts a network round trip inside
    /// it.
    ///
    /// [`read_events`]: SessionBackend::read_events
    fn read_events_local(&self, id: &str) -> anyhow::Result<(StateFileHeader, Vec<Event>)> {
        self.read_events(id)
    }

    /// # Stability: additive-only (Issue 19 / Decision 5)
    ///
    /// Not part of the Stage 1 frozen four; signature evolution is
    /// permitted in minor releases.
    ///
    /// Read just the header from the state file.
    fn read_header(&self, id: &str) -> anyhow::Result<StateFileHeader>;

    /// # Stability: additive-only
    ///
    /// Not part of the Stage 1 frozen four; signature evolution is
    /// permitted in minor releases.
    ///
    /// Replace the state file's header with `f` applied to it, keeping the
    /// event log verbatim, and sync the result the way
    /// [`append_event`](Self::append_event) syncs an event.
    ///
    /// Every header rewrite in the CLI goes through here. A rewrite that
    /// wrote the local file directly would be undone on a backend that pulls
    /// before reading: the remote copy still carries the old header, and the
    /// next read restores it (koto#310). The two rewrites in
    /// `src/engine/claim.rs` (the assignment claim and the re-delegation
    /// epoch bump) take a state-file path and no backend; nothing outside
    /// tests reaches them today.
    ///
    /// The default rewrites the state file under
    /// [`session_dir`](Self::session_dir) in place (temp file, then rename)
    /// and then calls [`ensure_pushed`](Self::ensure_pushed), printing a
    /// warning to stderr if the push fails rather than failing the rewrite.
    /// A backend with no remote half gets a no-op push. A backend that syncs
    /// gets its rewrite pushed through its strict push, which is enough to
    /// keep the rewrite from being lost; it overrides this only to push the
    /// way its other writes do, as `CloudBackend` does.
    fn rewrite_header(
        &self,
        id: &str,
        f: &dyn Fn(StateFileHeader) -> StateFileHeader,
    ) -> anyhow::Result<()> {
        let path = self.session_dir(id).join(state_file_name(id));
        crate::engine::claim::rewrite_header_atomically(&path, f)?;
        if let Err(e) = self.ensure_pushed(id) {
            eprintln!("warning: session sync failed for state upload: {}", e);
        }
        Ok(())
    }

    /// Push any pending local state for `id` to durable remote storage
    /// and fail if the push cannot be confirmed.
    ///
    /// This exists so batch operations that must enforce "push parent
    /// before child mutation" ordering (see Decision 12 Q6 in the
    /// batch-child-spawning design) have a strict fail-fast probe. The
    /// default `append_event` path on `CloudBackend` treats S3 errors as
    /// warnings so single-writer workflows stay responsive when the
    /// network flaps; the retry-failed dispatcher cannot tolerate that
    /// laxity because a silently-dropped parent push lets child writes
    /// race ahead of a stale parent log on S3.
    ///
    /// # Semantics
    ///
    /// * `LocalBackend`: no-op. Returns `Ok(())` because the local
    ///   `append_event` write is already durable on the filesystem by
    ///   the time the caller reaches this method.
    /// * `CloudBackend`: performs a synchronous PUT of the local state
    ///   file to S3 and returns `Err(SessionError::Io)` /
    ///   `Err(SessionError::Other)` on any transport or non-success
    ///   response.
    ///
    /// Callers that do NOT care about cross-host ordering should
    /// continue to use `append_event`; this method is specifically for
    /// the narrow retry-failed / batch-resolve paths.
    ///
    /// # Stability: additive-only (Issue 19 / Decision 5)
    ///
    /// Not part of the Stage 1 frozen four; signature evolution is
    /// permitted in minor releases.
    fn ensure_pushed(&self, id: &str) -> Result<(), SessionError>;

    /// Rename a session from `from` to `to`, updating the state file
    /// header to reflect the new identity.
    ///
    /// Renames the session directory, renames the state file within it,
    /// and rewrites the header's `workflow` field to `to` and
    /// `parent_workflow` to the parent derived from `to` (everything
    /// before the last `.` separator, or `None` if `to` contains no
    /// `.`).
    ///
    /// Returns `Err` if `from` doesn't exist or `to` already exists.
    ///
    /// # Stability: additive-only (Issue 19 / Decision 5)
    ///
    /// Not part of the Stage 1 frozen four; signature evolution is
    /// permitted in minor releases.
    fn relocate(&self, from: &str, to: &str) -> anyhow::Result<()>;

    /// # Stability: additive-only
    ///
    /// Whether the store's remote half holds nothing at all under `id`'s
    /// prefix, a migration marker included, asked by a checked listing of
    /// that prefix ending in `/` (so `x_r1` doesn't match `x_r10`). `Err`
    /// means the listing failed and nothing can be said. The local store is
    /// the caller's to check.
    ///
    /// The default, for a backend with no remote half, is `Ok(true)`.
    fn remote_prefix_free(&self, _id: &str) -> anyhow::Result<bool> {
        Ok(true)
    }

    /// # Stability: additive-only
    ///
    /// [`relocate`](Self::relocate) that fails rather than warns: the local
    /// session is renamed (with the header identity `relocate` writes), and
    /// a remote half gets a copy of every object under the new prefix,
    /// keeping the old ones, failing on any object it couldn't copy. The
    /// state log, whose object name embeds the session name, and a
    /// migration marker are not copied; the caller pushes the log under the
    /// new name with [`ensure_pushed`](Self::ensure_pushed).
    ///
    /// The default, for a backend with no remote half, is `relocate`.
    fn relocate_checked(&self, from: &str, to: &str) -> anyhow::Result<()> {
        self.relocate(from, to)
    }

    /// # Stability: additive-only
    ///
    /// Rename a session in the local store only, touching no remote copy:
    /// how a move that failed partway is undone, while the old remote
    /// prefix still holds everything. The header identity is written as
    /// `relocate` writes it; the caller restores the rest.
    ///
    /// The default, for a backend with no remote half, is `relocate`.
    fn relocate_local_only(&self, from: &str, to: &str) -> anyhow::Result<()> {
        self.relocate(from, to)
    }

    /// # Stability: additive-only
    ///
    /// Delete every object under `id`'s remote prefix except a migration
    /// marker, failing (after trying the rest) when the listing or any
    /// delete fails. The local copy is untouched.
    ///
    /// The default, for a backend with no remote half, does nothing.
    fn delete_remote_checked(&self, _id: &str) -> anyhow::Result<()> {
        Ok(())
    }

    /// # Stability: additive-only
    ///
    /// What the store's remote half holds of `id`'s pin: the bucket check
    /// the removal guard makes before every delete
    /// ([`crate::session::pin::guard_removal`]). A fresh listing of the
    /// session's `ctx/koto/` objects, never a cached one, then a read of the
    /// pin object when it is there, or of the release record when there is
    /// none and `local_pin_id` names this host's pin, to tell whether that
    /// pin was released on another host.
    ///
    /// The default, for a backend with no remote half, is `NotApplicable`.
    fn bucket_pin(&self, _id: &str, _local_pin_id: Option<&str>) -> crate::session::pin::BucketPin {
        crate::session::pin::BucketPin::NotApplicable
    }

    /// # Stability: additive-only
    ///
    /// What the store's remote half holds beside `id` that the continues
    /// relation's check of a removal reads
    /// ([`crate::session::continues_bucket`]): the session's `ctx/koto/`
    /// listing, shared with [`SessionBackend::bucket_pin`] when the pin guard
    /// just made one (the pin and release keys and sizes, and the
    /// validated back-references its successors keep there), one delimiter
    /// listing of the session's prefix (a migration marker, the state
    /// object's ETag), and the same `ctx/koto/` listing under the session's
    /// `origin.store.prefix` when that differs.
    ///
    /// The default, for a backend with no remote half, is `None`.
    fn bucket_removal_facts(
        &self,
        _id: &str,
    ) -> Option<crate::session::continues_bucket::BucketRemovalFacts> {
        None
    }

    /// # Stability: additive-only
    ///
    /// The cloud backend this is, for the continues relation's bucket walk;
    /// `None` for a backend with no bucket.
    fn as_cloud(&self) -> Option<&cloud::CloudBackend> {
        None
    }

    /// # Stability: additive-only
    ///
    /// Where `id`'s own copy lives, asked of the local copy alone with no
    /// request: its [`Home`], or [`Unlocated`] when no copy can be taken as
    /// its own.
    ///
    /// The default, for a backend with no remote half, is the caller's own
    /// workspace for every session.
    fn home_of(&self, _id: &str) -> Result<Home, Unlocated> {
        Ok(Home {
            prefix: String::new(),
            own_workspace: true,
            via: HomeSource::Caller,
        })
    }

    /// # Stability: additive-only
    ///
    /// After `old` moved to `new` for good (the same-name replace's
    /// move-aside, once every rename and push went through): carry the
    /// back-references `old`'s successors keep beside it to `new`, and take
    /// back the one `new`'s own relation named under its old name. Returns
    /// what wasn't carried or taken back, for the caller to report.
    ///
    /// The default, for a backend with no remote half, does nothing.
    fn repoint_after_move(&self, _old: &str, _new: &str) -> Vec<String> {
        Vec::new()
    }

    /// # Stability: additive-only
    ///
    /// Whether `id` is a migrated source: `koto session import` moved it to
    /// another workspace and left a migration marker under its prefix. Its
    /// pin is inert, since the moved copy holds it. Answered the way a write
    /// is checked: from this process's own listing, or a tick's check its
    /// local copy still matches, never from an answer another process lent.
    /// A check that can't be made answers `false`, so the pin keeps.
    ///
    /// The default, for a backend with no remote half, is `false`.
    fn is_migrated_source(&self, _id: &str) -> bool {
        false
    }

    /// Acquire a non-blocking advisory `flock(LOCK_EX | LOCK_NB)` on
    /// the session's state file.
    ///
    /// Intended for batch parents that must serialize concurrent tick
    /// calls (see Decision 12 in the batch-child-spawning design).
    /// Non-batch workflows never invoke this method -- the happy path
    /// is unchanged for them.
    ///
    /// # Semantics
    ///
    /// - On success, returns a [`SessionLock`] RAII guard. Dropping the
    ///   guard releases the lock. Process death releases the lock via
    ///   the kernel's `flock` semantics, so a crashed holder cannot
    ///   permanently strand the state file.
    /// - On contention, returns `SessionError::Locked { holder_pid }`
    ///   immediately -- this is a non-blocking acquisition, so callers
    ///   can surface a typed `ConcurrentTick` error without spinning.
    ///   `holder_pid` is best-effort; today it is always `None`.
    /// - If the state file does not yet exist, the method returns a
    ///   `SessionError::Io` with `ErrorKind::NotFound`. Callers are
    ///   expected to acquire the lock only after initialisation has
    ///   committed a state file.
    ///
    /// # Cloud backend
    ///
    /// `flock` is local-to-the-host. On `CloudBackend`, two processes
    /// running on different hosts both see the same remote state file
    /// in S3, but neither sees the other's file-descriptor lock.
    /// `lock_state_file` therefore only serializes intra-host
    /// contention under `CloudBackend`. Cross-host coordination is
    /// handled by the broader "push parent before child mutation"
    /// ordering described in the design's Decision 12 (Q6), not by
    /// this lock.
    ///
    /// # Not with an idempotent append on the same thread
    ///
    /// [`append_event_idempotent`](crate::engine::persistence::append_event_idempotent)
    /// takes a blocking `flock` on this same state file (then the sidecar
    /// append lock). `flock` locks belong to the open file, so a thread
    /// holding this guard that made an idempotent append to the same
    /// session would block on itself forever. Plain appends
    /// ([`append_event`](Self::append_event)) take only the sidecar lock and
    /// are safe under the guard. No production path makes that call today,
    /// and none may: debug builds record the holder in the guard and panic
    /// on the combination rather than hang.
    ///
    /// # Windows
    ///
    /// Not supported. koto targets Unix-like systems; on non-Unix
    /// targets this method returns a `SessionError::Other` describing
    /// the platform limitation.
    ///
    /// # Stability: additive-only (Issue 19 / Decision 5)
    ///
    /// Not part of the Stage 1 frozen four; signature evolution is
    /// permitted in minor releases.
    fn lock_state_file(&self, id: &str) -> Result<SessionLock, SessionError>;
}

/// Unified backend that dispatches to either `LocalBackend` or
/// `CloudBackend` depending on configuration.
///
/// This enum allows `build_backend()` to return a single concrete type
/// that implements both `SessionBackend` and `ContextStore`, regardless
/// of which backend is selected. One `Backend` is built per command, so the
/// size of the cloud variant costs nothing worth boxing it for.
#[allow(clippy::large_enum_variant)]
pub enum Backend {
    Local(LocalBackend),
    Cloud(cloud::CloudBackend),
}

impl Backend {
    /// Whether this backend supports cross-machine cloud sync.
    ///
    /// Delegates to `CloudBackend::is_cloud()` for the `Cloud` variant;
    /// always `false` for `Local`. Used to gate wording on new
    /// `template_source_dir` staleness surfaces (`koto status`, `koto
    /// init`'s collision path, `koto session list`) since a
    /// cloud-synced session can legitimately be resumed on a different
    /// machine than the one that recorded its `template_source_dir` --
    /// see `format_stale_template_source_note` in
    /// `crate::engine::template_source_status`.
    pub fn is_cloud(&self) -> bool {
        matches!(self, Backend::Cloud(_))
    }

    /// Start a tick of `id`: give it a new id, the window a cloud read made
    /// by its commands treats a session it already checked as fresh in
    /// (`sync_stamp`). The local backend asks no bucket and keeps none.
    pub fn begin_tick(&self, id: &str) {
        if let Backend::Cloud(b) = self {
            b.begin_tick(id);
        }
    }

    /// Before a command writes or pushes `id` under the cloud backend: PUT
    /// the back-reference its continues relation calls for when one is owed
    /// (a relation recorded under the local backend, or by an earlier
    /// build, or under another name). A failure warns and keeps the
    /// relation out of the session's pushes. The local backend writes no
    /// back-reference.
    pub fn ensure_backref(&self, id: &str) {
        if let Backend::Cloud(b) = self {
            b.ensure_backref(id);
        }
    }

    /// Push the writes the commands of a tick on `id` deferred
    /// ([`batch`]); `koto next` calls it before it exits. The local backend
    /// has no remote half and nothing to push.
    pub fn flush_pending(&self, id: &str) -> anyhow::Result<()> {
        match self {
            Backend::Local(_) => Ok(()),
            Backend::Cloud(b) => b.flush_pending(id),
        }
    }

    /// The local directory sessions are stored under.
    ///
    /// Both variants have one: `CloudBackend` keeps a `LocalBackend` for
    /// every filesystem operation and syncs on top of it. Maintenance verbs
    /// that address the store as a whole rather than one session -- `koto
    /// session recover` walking the migration quarantine -- need it under
    /// either backend, because the quarantine is written by the migration
    /// that runs for both.
    pub fn local_base_dir(&self) -> &std::path::Path {
        match self {
            Backend::Local(b) => b.base_dir(),
            Backend::Cloud(b) => b.local_base_dir(),
        }
    }

    /// Where this backend's run journal is written: the journal root of the
    /// local store it is, or, for the cloud backend, of the local store it
    /// keeps its copies in. `None` writes no journal. See `crate::run_journal`.
    pub(crate) fn journal_root(&self) -> Option<&std::path::Path> {
        match self {
            Backend::Local(b) => b.journal_root(),
            Backend::Cloud(b) => b.journal_root(),
        }
    }

    /// Refuse with [`SessionMigrated`] when `id` was imported into another
    /// workspace. Always `Ok` on the local backend, which has no remote
    /// copy to import from and makes no request.
    pub fn check_not_migrated(&self, id: &str) -> anyhow::Result<()> {
        match self {
            Backend::Local(_) => Ok(()),
            Backend::Cloud(b) => b.check_not_migrated(id),
        }
    }

    /// Refuse with [`Unlocated`] when koto can't tell which bucket copy is
    /// `id`'s own. Always `Ok` on the local backend, whose one store answers
    /// a name the same from every directory.
    pub fn check_located(&self, id: &str) -> anyhow::Result<()> {
        match self {
            Backend::Local(_) => Ok(()),
            Backend::Cloud(b) => b.check_located(id),
        }
    }

    /// Read at most `bound` leading bytes of a key's LOCAL content, for
    /// bounded display (`crate::session_view`). Never a pull and never a
    /// remote request on either variant: a key with no local content is
    /// `Ok(None)`, whatever the remote holds. Refuses anything but a
    /// regular file, checked before the open (an `open` on a FIFO blocks
    /// before any handle check could run), and a path whose resolved
    /// parent leaves the session's `ctx/` directory.
    pub fn read_context_prefix(
        &self,
        session: &str,
        key: &str,
        bound: usize,
    ) -> anyhow::Result<Option<PrefixRead>> {
        match self {
            Backend::Local(b) => b.read_context_prefix(session, key, bound),
            Backend::Cloud(b) => b.read_context_prefix(session, key, bound),
        }
    }

    /// The local context manifest and, on the cloud backend, the remote
    /// one from the cached fetch its metadata paths already make (`None`
    /// on the local backend or when the fetch fails). One request at
    /// most, shared with the backend's own 5-second manifest cache.
    pub fn manifest_pair(&self, session: &str) -> (context::Manifest, Option<context::Manifest>) {
        match self {
            Backend::Local(b) => (b.read_manifest(session).unwrap_or_default(), None),
            Backend::Cloud(b) => b.manifest_pair(session),
        }
    }
}

impl SessionBackend for Backend {
    fn create(&self, id: &str) -> anyhow::Result<PathBuf> {
        match self {
            Backend::Local(b) => b.create(id),
            Backend::Cloud(b) => b.create(id),
        }
    }

    fn session_dir(&self, id: &str) -> PathBuf {
        match self {
            Backend::Local(b) => b.session_dir(id),
            Backend::Cloud(b) => b.session_dir(id),
        }
    }

    fn exists(&self, id: &str) -> bool {
        match self {
            Backend::Local(b) => b.exists(id),
            Backend::Cloud(b) => b.exists(id),
        }
    }

    fn cleanup(&self, id: &str) -> anyhow::Result<()> {
        match self {
            Backend::Local(b) => b.cleanup(id),
            Backend::Cloud(b) => b.cleanup(id),
        }
    }

    fn list(&self) -> anyhow::Result<Vec<SessionInfo>> {
        match self {
            Backend::Local(b) => b.list(),
            Backend::Cloud(b) => b.list(),
        }
    }

    fn count_unreadable(&self) -> usize {
        match self {
            Backend::Local(b) => b.count_unreadable(),
            Backend::Cloud(b) => b.count_unreadable(),
        }
    }

    fn store_identity(&self) -> Option<crate::engine::types::SessionStoreIdentity> {
        match self {
            Backend::Local(b) => b.store_identity(),
            Backend::Cloud(b) => b.store_identity(),
        }
    }

    fn append_header(&self, id: &str, header: &StateFileHeader) -> anyhow::Result<()> {
        match self {
            Backend::Local(b) => b.append_header(id, header),
            Backend::Cloud(b) => b.append_header(id, header),
        }
    }

    fn append_event(
        &self,
        id: &str,
        payload: &EventPayload,
        timestamp: &str,
    ) -> anyhow::Result<()> {
        match self {
            Backend::Local(b) => b.append_event(id, payload, timestamp),
            Backend::Cloud(b) => b.append_event(id, payload, timestamp),
        }
    }

    fn append_own_read(
        &self,
        id: &str,
        payload: &EventPayload,
        timestamp: &str,
    ) -> anyhow::Result<()> {
        match self {
            Backend::Local(b) => b.append_own_read(id, payload, timestamp),
            Backend::Cloud(b) => b.append_own_read(id, payload, timestamp),
        }
    }

    fn read_events(&self, id: &str) -> anyhow::Result<(StateFileHeader, Vec<Event>)> {
        match self {
            Backend::Local(b) => b.read_events(id),
            Backend::Cloud(b) => b.read_events(id),
        }
    }

    fn read_events_local(&self, id: &str) -> anyhow::Result<(StateFileHeader, Vec<Event>)> {
        match self {
            Backend::Local(b) => b.read_events_local(id),
            Backend::Cloud(b) => b.read_events_local(id),
        }
    }

    fn read_header(&self, id: &str) -> anyhow::Result<StateFileHeader> {
        match self {
            Backend::Local(b) => b.read_header(id),
            Backend::Cloud(b) => b.read_header(id),
        }
    }

    fn rewrite_header(
        &self,
        id: &str,
        f: &dyn Fn(StateFileHeader) -> StateFileHeader,
    ) -> anyhow::Result<()> {
        match self {
            Backend::Local(b) => b.rewrite_header(id, f),
            Backend::Cloud(b) => b.rewrite_header(id, f),
        }
    }

    fn init_state_file(
        &self,
        id: &str,
        header: StateFileHeader,
        initial_events: Vec<Event>,
    ) -> Result<(), SessionError> {
        match self {
            Backend::Local(b) => b.init_state_file(id, header, initial_events),
            Backend::Cloud(b) => b.init_state_file(id, header, initial_events),
        }
    }

    fn relocate(&self, from: &str, to: &str) -> anyhow::Result<()> {
        match self {
            Backend::Local(b) => b.relocate(from, to),
            Backend::Cloud(b) => b.relocate(from, to),
        }
    }

    fn remote_prefix_free(&self, id: &str) -> anyhow::Result<bool> {
        match self {
            Backend::Local(b) => b.remote_prefix_free(id),
            Backend::Cloud(b) => b.remote_prefix_free(id),
        }
    }

    fn relocate_checked(&self, from: &str, to: &str) -> anyhow::Result<()> {
        match self {
            Backend::Local(b) => b.relocate_checked(from, to),
            Backend::Cloud(b) => b.relocate_checked(from, to),
        }
    }

    fn relocate_local_only(&self, from: &str, to: &str) -> anyhow::Result<()> {
        match self {
            Backend::Local(b) => b.relocate_local_only(from, to),
            Backend::Cloud(b) => b.relocate_local_only(from, to),
        }
    }

    fn bucket_pin(&self, id: &str, local_pin_id: Option<&str>) -> crate::session::pin::BucketPin {
        match self {
            Backend::Local(b) => b.bucket_pin(id, local_pin_id),
            Backend::Cloud(b) => b.bucket_pin(id, local_pin_id),
        }
    }

    fn is_migrated_source(&self, id: &str) -> bool {
        match self {
            Backend::Local(b) => b.is_migrated_source(id),
            Backend::Cloud(b) => b.is_migrated_source(id),
        }
    }

    fn bucket_removal_facts(
        &self,
        id: &str,
    ) -> Option<crate::session::continues_bucket::BucketRemovalFacts> {
        match self {
            Backend::Local(b) => b.bucket_removal_facts(id),
            Backend::Cloud(b) => b.bucket_removal_facts(id),
        }
    }

    fn as_cloud(&self) -> Option<&cloud::CloudBackend> {
        match self {
            Backend::Local(_) => None,
            Backend::Cloud(b) => Some(b),
        }
    }

    fn home_of(&self, id: &str) -> Result<Home, Unlocated> {
        match self {
            Backend::Local(b) => b.home_of(id),
            Backend::Cloud(b) => b.home_of(id),
        }
    }

    fn repoint_after_move(&self, old: &str, new: &str) -> Vec<String> {
        match self {
            Backend::Local(b) => b.repoint_after_move(old, new),
            Backend::Cloud(b) => b.repoint_after_move(old, new),
        }
    }

    fn delete_remote_checked(&self, id: &str) -> anyhow::Result<()> {
        match self {
            Backend::Local(b) => b.delete_remote_checked(id),
            Backend::Cloud(b) => b.delete_remote_checked(id),
        }
    }

    fn lock_state_file(&self, id: &str) -> Result<SessionLock, SessionError> {
        match self {
            Backend::Local(b) => b.lock_state_file(id),
            Backend::Cloud(b) => b.lock_state_file(id),
        }
    }

    fn ensure_pushed(&self, id: &str) -> Result<(), SessionError> {
        match self {
            Backend::Local(b) => b.ensure_pushed(id),
            Backend::Cloud(b) => b.ensure_pushed(id),
        }
    }
}

impl ContextStore for Backend {
    fn add(&self, session: &str, key: &str, content: &[u8]) -> anyhow::Result<()> {
        match self {
            Backend::Local(b) => b.add(session, key, content),
            Backend::Cloud(b) => b.add(session, key, content),
        }
    }

    fn add_with_writer(
        &self,
        session: &str,
        key: &str,
        content: &[u8],
        writer: &str,
    ) -> anyhow::Result<()> {
        match self {
            Backend::Local(b) => b.add_with_writer(session, key, content, writer),
            Backend::Cloud(b) => b.add_with_writer(session, key, content, writer),
        }
    }

    fn meta(&self, session: &str, key: &str) -> Option<context::KeyMeta> {
        match self {
            Backend::Local(b) => b.meta(session, key),
            Backend::Cloud(b) => b.meta(session, key),
        }
    }

    fn get(&self, session: &str, key: &str) -> anyhow::Result<Vec<u8>> {
        match self {
            Backend::Local(b) => b.get(session, key),
            Backend::Cloud(b) => b.get(session, key),
        }
    }

    fn ctx_exists(&self, session: &str, key: &str) -> bool {
        match self {
            Backend::Local(b) => b.ctx_exists(session, key),
            Backend::Cloud(b) => b.ctx_exists(session, key),
        }
    }

    fn remove(&self, session: &str, key: &str) -> anyhow::Result<()> {
        match self {
            Backend::Local(b) => b.remove(session, key),
            Backend::Cloud(b) => b.remove(session, key),
        }
    }

    fn list_keys(&self, session: &str, prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
        match self {
            Backend::Local(b) => b.list_keys(session, prefix),
            Backend::Cloud(b) => b.list_keys(session, prefix),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_migrated_message_leads_with_its_code_and_names_the_target() {
        let err = SessionMigrated {
            name: "coord".to_string(),
            target: "coord".to_string(),
            workspace: "/srv/ws-b".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "session_migrated: session 'coord' was migrated to 'coord' in /srv/ws-b; \
             continue it there"
        );
        // Through anyhow, as the backend returns it, the type survives for
        // callers that pick an exit code by it.
        let wrapped = anyhow::Error::new(err.clone());
        assert_eq!(wrapped.downcast_ref::<SessionMigrated>(), Some(&err));
    }

    #[test]
    fn local_backend_is_not_cloud() {
        let tmp = tempfile::TempDir::new().unwrap();
        let backend = Backend::Local(LocalBackend::with_base_dir(tmp.path().to_path_buf()));
        assert!(!backend.is_cloud());
    }

    #[test]
    fn cloud_backend_is_cloud() {
        // A dummy bucket pointing at a non-routable endpoint is enough
        // to exercise the discriminant -- no S3 call happens here.
        let tmp = tempfile::TempDir::new().unwrap();
        let local = LocalBackend::with_base_dir(tmp.path().to_path_buf());
        let region = s3::Region::Custom {
            region: "us-east-1".to_string(),
            endpoint: "http://192.0.2.1:19000".to_string(), // RFC 5737 TEST-NET
        };
        let credentials =
            s3::creds::Credentials::new(Some("test-key"), Some("test-secret"), None, None, None)
                .unwrap();
        let bucket = s3::Bucket::new("test-bucket", region, credentials).unwrap();
        let cloud_backend =
            cloud::CloudBackend::with_parts(local, bucket, "test-prefix".to_string());
        let backend = Backend::Cloud(cloud_backend);
        assert!(backend.is_cloud());
    }
}
