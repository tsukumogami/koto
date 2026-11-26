//! Data layer for the `koto dashboard` command, local half: the session
//! scan, the mtime-based incremental diff, the per-session read and the
//! liveness classification. koto's `cli::dashboard_data` adds the bucket
//! overlay on top.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Result;

use crate::engine::persistence::{
    derive_last_gate_evaluated, derive_machine_state, derive_state_from_log, parse_header_bytes,
    parse_state_log_at,
};
use crate::engine::types::parse_timestamp_to_systemtime;
use crate::engine::types::StateFileHeader;
use crate::fs_read::{read_store_file, ReadMode};
use crate::session::row::{derive_row, latest_import};
use crate::session::state_file_name;
use crate::source;

pub use liveness::{attention_key, classify_liveness, Liveness};

/// A source a local session was imported from, by its latest
/// `session_imported` event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedSource {
    /// The source's workspace prefix; `None` when its workspace path
    /// couldn't be turned into one.
    pub prefix: Option<String>,
    /// The source's session name.
    pub name: String,
    /// The source's `session_id`.
    pub session_id: String,
}

/// Read-time liveness classification for dashboard sessions.
///
/// Liveness is derived entirely from data already loaded into [`CachedSession`]
/// (the events' last timestamp, plus `is_terminal` / `is_blocked` /
/// `current_state`). Nothing is written to disk and nothing migrates: every
/// classification is a pure function of the append-only log the dashboard
/// already reads.
pub mod liveness {
    use std::cmp::Reverse;
    use std::time::{Duration, SystemTime};

    use super::CachedSession;
    use crate::engine::persistence::is_failed_state;

    /// A session younger than this is freshly active.
    pub const ACTIVE_WINDOW: Duration = Duration::from_secs(5 * 60);
    /// A session that advanced but has been silent at least this long is stalled.
    pub const STALLED_THRESHOLD: Duration = Duration::from_secs(2 * 60 * 60);
    /// An idle session older than this is treated as abandoned (recedes).
    pub const ABANDONED: Duration = Duration::from_secs(7 * 24 * 60 * 60);

    /// Closed liveness vocabulary for a session, derived at read time.
    ///
    /// The three `NeedsYou*` variants are the attention band: a human decision
    /// is what unblocks them. `Active`/`Idle` are healthy live work. `Pending`
    /// is a run that never advanced (cruft, not a stuck decision). `Done` is
    /// terminal success.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Liveness {
        /// Waiting on a gate that has not passed. Needs a human.
        NeedsYouBlocked,
        /// Terminal in a failed/error state. Needs a human.
        NeedsYouFailed,
        /// Advanced, then went silent past the stalled threshold. Needs a human.
        NeedsYouStalled,
        /// Non-terminal and recently active (idle < active window).
        Active,
        /// Non-terminal, idle between the active window and the stalled threshold.
        Idle,
        /// Never advanced past `WorkflowInitialized` (`current_state == None`).
        Pending,
        /// Terminal, not failed.
        Done,
    }

    /// Idle duration for a session: `now - last_event_at`, clamped to zero on
    /// clock skew / future timestamps. Falls back to the file mtime when the log
    /// carried no parseable event timestamp.
    pub fn idle_for(session: &CachedSession, now: SystemTime) -> Duration {
        let reference = session.last_event_at.unwrap_or(session.mtime);
        now.duration_since(reference).unwrap_or(Duration::ZERO)
    }

    /// Classify a session's liveness using the design precedence D1-D7.
    ///
    /// Terminal, failed, blocked, and never-started are all resolved BEFORE any
    /// idle test (the load-bearing R4 rule): a gate-blocked or never-advanced
    /// session is never classified as stalled.
    pub fn classify_liveness(session: &CachedSession, now: SystemTime) -> Liveness {
        // D1 / D2: terminal resolves first (Done vs failed).
        if session.is_terminal {
            if is_failed_state(session.current_state.as_deref()) {
                return Liveness::NeedsYouFailed;
            }
            return Liveness::Done;
        }

        // D3: blocked beats any idle test.
        if session.is_blocked {
            return Liveness::NeedsYouBlocked;
        }

        // D4: never advanced (only a WorkflowInitialized event). Never needs-you.
        if session.current_state.is_none() {
            return Liveness::Pending;
        }

        // From here current_state == Some and the session is live; consult idle.
        let idle = idle_for(session, now);

        // D5: advanced then went silent past the stalled threshold.
        if idle >= STALLED_THRESHOLD {
            return Liveness::NeedsYouStalled;
        }

        // D6: recently active.
        if idle < ACTIVE_WINDOW {
            return Liveness::Active;
        }

        // D7: between the active window and the stalled threshold.
        Liveness::Idle
    }

    /// Attention sort key `(band, Reverse(idle))`.
    ///
    /// Band order: NeedsYou* (0) -> Active (1) -> Idle + fresh Pending (2) ->
    /// receded (3). Within a band, longest-idle sorts first (most-waiting /
    /// most-dead first). A fresh `Pending` (idle < active window) stays in band 2
    /// so a brand-new session is visible; an older `Pending` recedes to band 3.
    pub fn attention_key(liveness: Liveness, idle: Duration) -> (u8, Reverse<Duration>) {
        let band = match liveness {
            Liveness::Done => 3,
            // Any session silent past the abandoned threshold is cruft, not an
            // attention item -- it recedes regardless of its needs-you sub-state.
            // A run blocked or stalled for weeks is abandoned, not "waiting on
            // you"; this keeps the needs-you band to genuinely recent items.
            // (Receded sessions remain reachable via `--all` / the reveal toggle.)
            _ if idle >= ABANDONED => 3,
            Liveness::NeedsYouBlocked | Liveness::NeedsYouFailed | Liveness::NeedsYouStalled => 0,
            Liveness::Active => 1,
            Liveness::Idle => 2,
            Liveness::Pending if idle < ACTIVE_WINDOW => 2,
            Liveness::Pending => 3,
        };
        (band, Reverse(idle))
    }

    /// True when a session's liveness places it in the receded (hidden-by-default)
    /// band: Done, an abandoned Idle, or a stale (non-fresh) Pending.
    pub fn is_receded(liveness: Liveness, idle: Duration) -> bool {
        attention_key(liveness, idle).0 == 3
    }

    /// A session's attention key by `now`: [`attention_key`] of its liveness
    /// and idle time, except that a remote row another remote summary names
    /// as a source ([`CachedSession::receded_by_source`]) sits in the
    /// receded band whatever its liveness, so it leaves the default view
    /// and the needs-you band and shows with `--all` and the `a` key.
    pub fn session_attention(session: &CachedSession, now: SystemTime) -> (u8, Reverse<Duration>) {
        let idle = idle_for(session, now);
        if session.receded_by_source {
            return (3, Reverse(idle));
        }
        attention_key(classify_liveness(session, now), idle)
    }

    /// True when `session` is in the receded band by
    /// [`session_attention`].
    pub fn is_session_receded(session: &CachedSession, now: SystemTime) -> bool {
        session_attention(session, now).0 == 3
    }

    // `is_failed_state` now lives in `crate::engine::persistence` as the single
    // shared implementation (imported above); the `/workflows` projection
    // writer reuses the same function.

    /// The most salient `WorkflowInitialized` variable; the rule lives with
    /// the shared row derivation ([`crate::session::row::salient_var`]).
    pub use crate::session::row::salient_var;
}

/// Lightweight snapshot of one session's derived state, held in the tree.
pub struct CachedSession {
    /// Full header from the first line of the state file.
    ///
    /// When `current_state` is `None` (parse failed), this may be a zero-value
    /// placeholder produced by `make_empty_header()`, in which case `header.workflow`
    /// is an empty string. Callers should use the tree key (session name) for display
    /// purposes rather than `header.workflow` when `current_state` is `None`.
    pub header: StateFileHeader,
    /// Current state derived from the event log, or `None` on parse error.
    pub current_state: Option<String>,
    /// Whether the current state is a terminal state in the template.
    pub is_terminal: bool,
    /// Whether the session is waiting on a gate that has not yet passed.
    ///
    /// True when the most recent `GateEvaluated` event in the current state's
    /// epoch has an outcome other than `"passed"`. Always `false` for terminal
    /// sessions and sessions with no recorded state.
    pub is_blocked: bool,
    /// Intent derived from IntentUpdated events, or from header fallback.
    pub intent: Option<String>,
    /// Last-modified time of the state file; used for cache invalidation.
    pub mtime: SystemTime,
    /// Path to the state file; used for re-reads on mtime change.
    pub state_path: PathBuf,
    /// Wall-clock time of the final event in the log, parsed from its RFC 3339
    /// timestamp. Drives read-time liveness (idle = now - last_event_at). `None`
    /// when the log has no parseable events; liveness then falls back to `mtime`.
    pub last_event_at: Option<SystemTime>,
    /// The most salient `WorkflowInitialized.variables` value, chosen by the
    /// key-priority list in [`liveness::salient_var`]. Folded into the label by
    /// `derive_label`; `None` when no priority key was present.
    pub salient_var: Option<String>,
    /// True when the session's header could not be parsed (corrupt/unreadable
    /// state file). Such sessions are surfaced in the `unreadable` tally rather
    /// than silently dropped.
    pub is_unreadable: bool,
    /// True when the session was spawned as a batch task, i.e. its
    /// `WorkflowInitialized` event carries a `spawn_entry`.
    ///
    /// Batch membership is not readable off the header, so it is derived
    /// during the same replay that computes `current_state` rather than by
    /// re-reading the log (DESIGN-request-lifecycle.md Decision 7).
    pub is_batch_task: bool,
    /// True when a leg pointer sits in the session directory, i.e. the
    /// session is bound to a request leg.
    ///
    /// Cached alongside the rest of the session because `visible_rows` runs
    /// on every tick and every keystroke. A bind writes only the sidecar and
    /// never the child's own log, so this flag can lag by one refresh of the
    /// state file — the same denormalization the pointer itself documents.
    pub is_request_leg: bool,
    /// The machine that produced the session, from its own events (a local
    /// row) or its summary (a remote row); `None` when none is recorded.
    pub machine: Option<String>,
    /// The workspace id (16-hex bucket prefix) of this copy, set only under
    /// the cloud backend; `None` on the local backend.
    pub workspace: Option<String>,
    /// Read from another machine's summary in the bucket rather than from
    /// this machine's session store. A remote row has no local state file.
    pub remote: bool,
    /// The source the local session's latest `session_imported` event
    /// names, which a remote scan hides; `None` for a session never
    /// imported and for a remote row.
    pub imported_from: Option<ImportedSource>,
    /// A remote row another remote row's summary names among its `sources`
    /// ([`RemoteRow::receded_by_source`]): it recedes whatever its liveness
    /// ([`liveness::session_attention`]). Always `false` for a local row.
    pub receded_by_source: bool,
}

/// Hierarchical view of all sessions visible to the dashboard.
///
/// `F` is the error a bucket overlay records in
/// [`remote_failure`](SessionTree::remote_failure); the local scan never
/// sets it.
pub struct SessionTree<F> {
    /// All sessions indexed by row key: the session name for a local row,
    /// `name@workspace` for a remote one ([`remote_key`]), its name as the
    /// bucket listed it. A remote key is never shown; [`row_name`] is.
    pub sessions: HashMap<String, CachedSession>,
    /// Names of root sessions: those with no parent, whose parent is absent,
    /// or whose parent chain loops back to them ([`SessionTree::detached`]).
    pub roots: Vec<String>,
    /// Row keys whose parent link is ignored because following it loops
    /// back to the row: a session naming itself as its parent, or each of a
    /// set of sessions naming each other. Each one is a root, and none is
    /// another's child, so a forged parent can't make a row vanish or nest
    /// without end.
    pub detached: HashSet<String>,
    /// Count of session dirs whose header failed to parse on the last refresh.
    /// Surfaced as a trailing note (TUI) / count (`--once`) instead of dropping
    /// the sessions silently.
    pub unreadable: usize,
    /// Count of other machines' summaries the last feed snapshot listed but
    /// couldn't read. Kept apart from `unreadable`, which every local refresh
    /// recomputes; the TUI's note shows the sum.
    pub remote_unreadable: usize,
    /// Count of other machines' sessions the last feed snapshot hid as
    /// migrated sources whose copy is listed.
    pub remote_hidden: usize,
    /// Why the last feed snapshot couldn't read some or all of the bucket;
    /// `None` once a snapshot reads it without failure.
    pub remote_failure: Option<F>,
    /// The last feed snapshot reached a listing ceiling, so some of the
    /// bucket isn't shown; `false` once a snapshot reaches none.
    pub remote_truncated: bool,
}

impl<F> SessionTree<F> {
    /// Construct an empty tree.
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            roots: Vec::new(),
            detached: HashSet::new(),
            unreadable: 0,
            remote_unreadable: 0,
            remote_hidden: 0,
            remote_failure: None,
            remote_truncated: false,
        }
    }

    /// The row key the row `key` nests under: its [`parent_key`], unless
    /// its parent chain loops back to it ([`SessionTree::detached`]).
    pub fn parent_of(&self, key: &str, session: &CachedSession) -> Option<String> {
        if self.detached.contains(key) {
            return None;
        }
        parent_key(session)
    }

    /// Rebuild [`SessionTree::roots`] and [`SessionTree::detached`] from
    /// the session map.
    pub fn rebuild_roots(&mut self) {
        self.detached = cyclic_keys(&self.sessions);
        self.roots = rebuild_roots(&self.sessions, &self.detached);
    }
}

impl<F> Default for SessionTree<F> {
    fn default() -> Self {
        Self::new()
    }
}

/// A single evidence entry from an `EvidenceSubmitted` event.
pub struct EvidenceEntry {
    /// The state this evidence was submitted for.
    pub state: String,
    /// The fields submitted as evidence.
    pub fields: serde_json::Value,
}

/// A single event entry for the History tab.
pub struct HistoryEntry {
    /// Event type string (e.g. "transitioned", "evidence_submitted").
    pub event_type: String,
    /// RFC 3339 timestamp.
    pub timestamp: String,
    /// Human-readable summary line.
    pub summary: String,
    /// Gate condition text for GateEvaluated events (from compiled template).
    pub gate_condition: Option<String>,
}

/// Gate-evaluation detail data loaded on demand for the focused session.
pub struct DetailData {
    /// The session this detail was loaded from.
    pub session_id: String,
    /// The identifier (name) of the most recently evaluated gate (e.g., "my-build-gate").
    pub gate_name: Option<String>,
    /// The command, if the gate was a command gate.
    pub command: Option<String>,
    /// The result of the gate evaluation ("PASS" or "FAIL").
    pub result: Option<String>,
    /// Time elapsed since the gate evaluation timestamp.
    pub elapsed: Duration,
    /// Evidence entries from the current epoch, newest-first.
    pub evidence: Vec<EvidenceEntry>,
    /// Current state name.
    pub current_state: Option<String>,
    /// Directive text from the compiled template for the current state.
    pub directive: Option<String>,
    /// Intent from last IntentUpdated event or header fallback.
    pub intent: Option<String>,
    /// Template name from header.
    pub template_name: Option<String>,
    /// All events from the current epoch, in chronological order, for the History tab.
    pub history: Vec<HistoryEntry>,
    /// Unvisited state names from the compiled template, in topological order, for the Remaining tab.
    pub remaining: Vec<String>,
}

/// Enumerate sessions from `source`, filtering epoch-branched names.
///
/// Returns a list of `(session_name, state_file_path)` pairs for sessions
/// whose names do not contain `~` (epoch-branched session indicator).
///
/// Fails first with the store's [`source::LayoutError`], as the outer
/// error, when its layout marker names a layout this reader doesn't read.
pub fn scan_sessions(source: &dyn source::SessionSource) -> Result<Vec<(String, PathBuf)>> {
    source::check_layout(source.sessions_dir())?;
    let session_list = source.list()?;
    let mut result = Vec::new();
    for info in session_list {
        // Filter out epoch-branched sessions (those containing `~`).
        if info.id.contains('~') {
            continue;
        }
        // Anchors and close sessions are not workflows.
        if info.session_kind().is_koto_owned() {
            continue;
        }
        let dir = source.session_dir(&info.id);
        let state_path = dir.join(state_file_name(&info.id));
        result.push((info.id, state_path));
    }
    Ok(result)
}

/// Result of an mtime-based diff against the session tree.
pub struct SessionDiff {
    /// Sessions present in the scan but not in the tree (new on disk).
    pub adds: Vec<(String, PathBuf)>,
    /// Sessions present in the tree but not in the scan (removed from disk).
    pub removes: Vec<String>,
    /// Sessions present in both, but whose file mtime has advanced.
    pub changed: Vec<(String, PathBuf)>,
}

/// Compute which sessions need to be added, removed, or re-read.
///
/// Compares the in-memory `tree` against the freshly-scanned `session_paths`
/// and returns a `SessionDiff` describing the delta.
pub fn stat_and_diff<F>(tree: &SessionTree<F>, session_paths: &[(String, PathBuf)]) -> SessionDiff {
    let mut adds = Vec::new();
    let mut changed = Vec::new();

    // Build a set of names from the scan for efficient lookup.
    let scan_set: HashMap<&str, &PathBuf> = session_paths
        .iter()
        .map(|(name, path)| (name.as_str(), path))
        .collect();

    // Find removes: in tree but not in scan. Remote rows never come from
    // the local scan; the feed replaces them whole.
    let removes: Vec<String> = tree
        .sessions
        .iter()
        .filter(|(name, s)| !s.remote && !scan_set.contains_key(name.as_str()))
        .map(|(name, _)| name.clone())
        .collect();

    // Find adds and changed.
    for (name, path) in session_paths {
        if let Some(cached) = tree.sessions.get(name) {
            // Already in tree: check if mtime has advanced.
            let current_mtime = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .unwrap_or(cached.mtime);
            if current_mtime > cached.mtime {
                changed.push((name.clone(), path.clone()));
            }
        } else {
            // New session: needs to be read.
            adds.push((name.clone(), path.clone()));
        }
    }

    SessionDiff {
        adds,
        removes,
        changed,
    }
}

/// The store root a state file at `path` must stay inside: by the layout
/// rule a state file is `<sessions dir>/<id>/koto-<id>.state.jsonl`, so the
/// root is its directory's parent. Every path the scan passes is built that
/// way under the source's sessions directory, so this is the source's own
/// root; a symlinked session directory still resolves outside it.
fn store_root(path: &Path) -> &Path {
    path.parent()
        .and_then(Path::parent)
        .filter(|root| !root.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// The state log at `path`, read whole through the hardened read: capped at
/// [`source::STATE_LOG_READ_CAP`], never blocking on a FIFO, refusing a
/// symlink and a path outside the store. `None` when refused.
fn read_state_log(path: &Path) -> Option<Vec<u8>> {
    read_store_file(
        path,
        source::STATE_LOG_READ_CAP,
        ReadMode::InStore {
            root: store_root(path),
        },
    )
    .ok()
}

/// Read a session from disk and return a `CachedSession`.
///
/// On any parse error, returns a `CachedSession` with `current_state = None`
/// and `is_terminal = false` rather than propagating the error.
pub fn read_session(path: &Path) -> CachedSession {
    // Attempt to read the mtime first; use UNIX_EPOCH as fallback.
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);

    // Leg membership: the pointer sidecar in the session's own directory
    // is the child-side record of a bind. Read here, alongside the state
    // file, so the row builder stays free of IO. Unlike the CLI read paths
    // this one swallows a malformed pointer silently: the dashboard owns
    // the terminal, and a warning line would corrupt the frame.
    let session_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let is_request_leg = crate::engine::leg_pointer::read_pointer(session_dir)
        .ok()
        .flatten()
        .is_some();

    // Read the log once, through the hardened read. A refused read leaves
    // no bytes, which no header parses: the unreadable row below.
    let log = read_state_log(path).unwrap_or_default();

    // Try to read the header alone for the fallback case.
    let fallback_header = match parse_header_bytes(&log) {
        Ok(h) => h,
        Err(_) => {
            return CachedSession {
                header: make_empty_header(),
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent: None,
                mtime,
                state_path: path.to_path_buf(),
                last_event_at: None,
                salient_var: None,
                is_unreadable: true,
                is_batch_task: false,
                is_request_leg,
                machine: None,
                workspace: None,
                remote: false,
                imported_from: None,
                receded_by_source: false,
            };
        }
    };

    // Try to read events.
    let (header, events) = match parse_state_log_at(&log, path) {
        Ok(pair) => pair,
        Err(_) => {
            let intent = fallback_header.intent.clone();
            return CachedSession {
                header: fallback_header,
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent,
                mtime,
                state_path: path.to_path_buf(),
                last_event_at: None,
                salient_var: None,
                is_unreadable: false,
                is_batch_task: false,
                is_request_leg,
                machine: None,
                workspace: None,
                remote: false,
                imported_from: None,
                receded_by_source: false,
            };
        }
    };

    // One derivation of the row's facts, shared with the summary a push
    // publishes, so a remote row equals what this reader shows locally.
    let row = derive_row(&header, &events, session_dir, is_request_leg);

    // Liveness measures idle from the tail event's timestamp, so a rewound
    // session (whose tail is the post-rewind event) reads as fresh.
    let last_event_at = row
        .last_event_at
        .as_deref()
        .and_then(parse_timestamp_to_systemtime);

    // The source this session was imported from, by its workspace prefix:
    // a remote scan hides that source rather than list it beside its copy.
    let imported_from = latest_import(&events).map(|record| ImportedSource {
        prefix: crate::session::local::prefix_for_workspace(Path::new(&record.from_workspace)).ok(),
        name: record.from_session,
        session_id: record.from_session_id,
    });

    CachedSession {
        header,
        current_state: row.current_state,
        is_terminal: row.is_terminal,
        is_blocked: row.is_blocked,
        intent: row.intent,
        mtime,
        state_path: path.to_path_buf(),
        last_event_at,
        salient_var: row.salient_var,
        is_unreadable: false,
        is_batch_task: row.is_batch_task,
        is_request_leg,
        machine: row.machine,
        workspace: None,
        remote: false,
        imported_from,
        receded_by_source: false,
    }
}

/// Derive a human-readable label for a session row.
///
/// Total fallback chain (each rung checked non-empty before falling through);
/// never panics and never returns a bare session id when a template name is
/// known:
///
/// 1. `intent` (last `IntentUpdated` event, including the init default)
/// 2. `template_name · salient_var · current_state`
/// 3. `template_name · current_state`
/// 4. `untitled (template_name)`
/// 5. `session_id` — only when even `template_name` is empty (corrupt header)
///
/// `session_id` is the display id (the tree key / session name), used solely as
/// the last-resort rung.
pub fn derive_label(session: &CachedSession, session_id: &str) -> String {
    const SEP: &str = " \u{b7} "; // " · "

    // Rung 1: explicit intent.
    if let Some(intent) = session.intent.as_deref() {
        if !intent.is_empty() {
            return intent.to_string();
        }
    }

    let template_name = session.header.template_name.as_deref().unwrap_or("");

    // Rungs 2-4 require a non-empty template name.
    if !template_name.is_empty() {
        let current_state = session.current_state.as_deref().unwrap_or("");
        let salient = session.salient_var.as_deref().unwrap_or("");

        // Rung 2: template · salient_var · current_state.
        if !salient.is_empty() && !current_state.is_empty() {
            return format!("{template_name}{SEP}{salient}{SEP}{current_state}");
        }
        // Rung 3: template · current_state.
        if !current_state.is_empty() {
            return format!("{template_name}{SEP}{current_state}");
        }
        // Rung 4: untitled (template).
        return format!("untitled ({template_name})");
    }

    // Rung 5: bare id, only when the header carried no template name.
    session_id.to_string()
}

/// Construct an empty `StateFileHeader` for error fallback cases.
#[doc(hidden)]
pub fn make_empty_header() -> StateFileHeader {
    StateFileHeader {
        command_environment: None,
        schema_version: 0,
        workflow: String::new(),
        template_hash: String::new(),
        created_at: String::new(),
        parent_workflow: None,
        template_source_dir: None,
        template_source_file: None,
        origin: None,
        execution_dir: None,
        session_id: String::new(),
        intent: None,
        template_name: None,
        needs_agent: None,
        role: None,
        inputs: None,
        coordinator_of_record: None,
        requested_by: None,
        assignment_claim: None,
        dispatch_epoch: 0,
        respawn_generation: None,
        priority: None,
        deadline: None,
        retry_count: None,
        agent_config: None,
        kind: None,
        root_session_id: None,
        parent_session_id: None,
    }
}

/// The row key of a remote row: `name@workspace`, with `name` as the
/// bucket listed it, unique within its workspace. Session names allow only
/// `[a-zA-Z0-9._-]` and `~`, so the key can't collide with a local row's,
/// which is the bare name. The key may hold raw bucket text, so it is
/// never shown: [`row_name`] is.
pub fn remote_key(name: &str, workspace: &str) -> String {
    format!("{name}@{workspace}")
}

/// The session name to show for the row at key `key`: the key itself for a
/// local row, and the sanitized `header.workflow` for a remote one, never
/// its key.
pub fn row_name<'a>(key: &'a str, session: &'a CachedSession) -> &'a str {
    if session.remote {
        &session.header.workflow
    } else {
        key
    }
}

/// The row key of the parent `session` nests under, in the same location:
/// a remote row's parent is the remote row of that name in its own
/// workspace, never a same-named local one. `None` when it records no
/// parent. The parent's name is the sanitized one the row carries, so for
/// an ordinary name it is that row's key; a parent whose listed name holds
/// a character the sanitizer replaced isn't found, and the row is a root.
pub fn parent_key(session: &CachedSession) -> Option<String> {
    let parent = session.header.parent_workflow.as_deref()?;
    Some(if session.remote {
        remote_key(parent, session.workspace.as_deref().unwrap_or(""))
    } else {
        parent.to_string()
    })
}

/// Rebuild the `roots` Vec from the current session map.
///
/// A session is a root if it has no parent, if its parent's row key
/// ([`parent_key`]) is not present in the current session map, or if it is
/// in `detached` ([`cyclic_keys`]), its parent chain looping back to it.
#[doc(hidden)]
pub fn rebuild_roots(
    sessions: &HashMap<String, CachedSession>,
    detached: &HashSet<String>,
) -> Vec<String> {
    let mut roots: Vec<String> = sessions
        .iter()
        .filter(|(key, cached)| {
            detached.contains(key.as_str())
                || match parent_key(cached) {
                    None => true,
                    Some(parent) => !sessions.contains_key(&parent),
                }
        })
        .map(|(name, _)| name.clone())
        .collect();
    roots.sort();
    roots
}

/// The row keys whose parent chain loops back to themselves: a session
/// whose parent is itself, and every member of a ring of sessions naming
/// each other. A session merely nested under such a ring isn't one of them.
///
/// Every row names at most one parent, so walking each chain once, and
/// stopping at a row an earlier walk already settled, finds every ring in
/// time linear in the number of rows.
#[doc(hidden)]
pub fn cyclic_keys(sessions: &HashMap<String, CachedSession>) -> HashSet<String> {
    let parent =
        |key: &str| parent_key(sessions.get(key)?).filter(|parent| sessions.contains_key(parent));
    let mut settled: HashSet<String> = HashSet::new();
    let mut cyclic = HashSet::new();
    for start in sessions.keys() {
        let mut path: Vec<String> = Vec::new();
        let mut on_path: HashMap<String, usize> = HashMap::new();
        let mut at = Some(start.clone());
        while let Some(key) = at {
            if settled.contains(&key) {
                break;
            }
            if let Some(&from) = on_path.get(&key) {
                cyclic.extend(path[from..].iter().cloned());
                break;
            }
            on_path.insert(key.clone(), path.len());
            at = parent(&key);
            path.push(key);
        }
        settled.extend(path);
    }
    cyclic
}

/// Refresh the session tree from `source` alone, by scanning it and
/// re-reading changed sessions. Remote rows are left as they are; see
/// [`refresh_with_overlay`].
///
/// Orchestrates `scan_sessions`, `stat_and_diff`, and `read_session` to keep the
/// tree up to date with minimal I/O. Rebuilds `roots` whenever the session set changes.
pub fn refresh_scan<F>(
    tree: &mut SessionTree<F>,
    source: &dyn source::SessionSource,
) -> Result<()> {
    let session_paths = scan_sessions(source)?;
    let diff = stat_and_diff(tree, &session_paths);

    let session_set_changed = !diff.adds.is_empty() || !diff.removes.is_empty();

    // Remove sessions that are no longer on disk.
    for name in &diff.removes {
        tree.sessions.remove(name);
    }

    // Add new sessions.
    for (name, path) in &diff.adds {
        let cached = read_session(path);
        tree.sessions.insert(name.clone(), cached);
    }

    // Re-read sessions whose mtime has changed.
    for (name, path) in &diff.changed {
        let cached = read_session(path);
        tree.sessions.insert(name.clone(), cached);
    }

    // Rebuild roots whenever the session set changed (adds or removes).
    // Also rebuild if any mtime-changed session could have had its parent change.
    if session_set_changed || !diff.changed.is_empty() {
        tree.rebuild_roots();
    }

    // Recompute the unreadable tally directly from the source: sessions whose
    // header fails to parse are excluded from `source.list()` (and thus from
    // the tree), so they must be counted at the directory level to be surfaced
    // rather than silently dropped.
    tree.unreadable = source.count_unreadable();

    Ok(())
}

/// Load detailed gate-evaluation data for a session on demand.
///
/// Returns `None` only on I/O errors reading the event log. When there is no
/// gate evaluation in the current epoch, returns `Some(DetailData)` with
/// `gate_name` and `result` set to `None` and `elapsed` set to `Duration::ZERO`.
pub fn read_detail(path: &Path, session_id: &str) -> Option<DetailData> {
    let (header, events) = parse_state_log_at(&read_state_log(path)?, path).ok()?;

    // Find the current state (optional — no short-circuit).
    let current_state = derive_state_from_log(&events);

    // Find the epoch boundary for the current state (optional).
    let epoch_start_idx = current_state.as_ref().and_then(|cs| {
        events.iter().enumerate().rev().find_map(|(idx, e)| {
            let to = match &e.payload {
                crate::engine::types::EventPayload::Transitioned { to, .. } => Some(to.as_str()),
                crate::engine::types::EventPayload::DirectedTransition { to, .. } => {
                    Some(to.as_str())
                }
                crate::engine::types::EventPayload::Rewound { to, .. } => Some(to.as_str()),
                _ => None,
            };
            if to == Some(cs.as_str()) {
                Some(idx)
            } else {
                None
            }
        })
    });

    let epoch_events: &[crate::engine::types::Event] = match epoch_start_idx {
        Some(idx) => &events[idx + 1..],
        None => &events,
    };

    // Load compiled template once (best-effort, None on any error),
    // through the hardened template read, so a FIFO, a directory or a file
    // over the cap leaves the detail without its template parts rather
    // than blocking or reading without bound.
    // The state file's parent directory is the session dir, the
    // resolution base for a session-relative template_path.
    let session_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let compiled = derive_machine_state(&header, &events, session_dir).and_then(|ms| {
        crate::fs_read::read_store_file(
            Path::new(&ms.template_path),
            crate::source::TEMPLATE_READ_CAP,
            crate::fs_read::ReadMode::Template,
        )
        .ok()
        .and_then(|bytes| {
            serde_json::from_slice::<crate::template::types::CompiledTemplate>(&bytes).ok()
        })
    });

    // Find the most recent GateEvaluated event (optional).
    let gate_event = epoch_events.iter().rev().find_map(|e| {
        if let crate::engine::types::EventPayload::GateEvaluated {
            gate,
            outcome,
            timestamp,
            ..
        } = &e.payload
        {
            Some((gate.clone(), outcome.clone(), timestamp.clone()))
        } else {
            None
        }
    });

    // Extract gate_name, result, elapsed from gate_event (all optional).
    let gate_name = gate_event.as_ref().map(|(g, _, _)| g.clone());

    let result = gate_event.as_ref().map(|(_, outcome, _)| {
        if outcome == "passed" {
            "PASS".to_string()
        } else {
            "FAIL".to_string()
        }
    });

    let elapsed = gate_event
        .as_ref()
        .map(|(_, _, ts)| compute_elapsed_since(ts))
        .unwrap_or(Duration::ZERO);

    // Get the last evaluated output for this gate (if any).
    let gate_output = gate_name
        .as_ref()
        .and_then(|gn| derive_last_gate_evaluated(&events, gn));

    // Extract the command from the gate output if present.
    let command = gate_output
        .as_ref()
        .and_then(|v| v.get("command").or_else(|| v.get("cmd")))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Gather evidence entries from the current epoch.
    let evidence: Vec<EvidenceEntry> = epoch_events
        .iter()
        .rev()
        .filter_map(|e| {
            if let crate::engine::types::EventPayload::EvidenceSubmitted { state, fields, .. } =
                &e.payload
            {
                Some(EvidenceEntry {
                    state: state.clone(),
                    fields: serde_json::Value::Object(
                        fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                    ),
                })
            } else {
                None
            }
        })
        .collect();

    // Directive from compiled template for current state.
    let directive = current_state.as_ref().and_then(|cs| {
        compiled
            .as_ref()?
            .states
            .get(cs.as_str())
            .map(|s| s.directive.clone())
    });

    // Intent: the shared rule (last intent_updated event, header fallback).
    let intent = crate::engine::types::effective_intent(&header, &events);

    // Template name from header.
    let template_name = header.template_name.clone();

    // Build history from epoch events (chronological order).
    let history: Vec<HistoryEntry> = epoch_events
        .iter()
        .map(|e| {
            let summary = build_event_summary(e);
            let gate_condition = if let crate::engine::types::EventPayload::GateEvaluated {
                gate,
                ..
            } = &e.payload
            {
                build_gate_condition(gate, compiled.as_ref())
            } else {
                None
            };
            HistoryEntry {
                event_type: e.event_type.clone(),
                timestamp: e.timestamp.clone(),
                summary,
                gate_condition,
            }
        })
        .collect();

    // Build remaining states: all states from compiled template NOT in visited states.
    let visited: std::collections::HashSet<&str> = events
        .iter()
        .filter_map(|e| match &e.payload {
            crate::engine::types::EventPayload::Transitioned { to, .. } => Some(to.as_str()),
            crate::engine::types::EventPayload::DirectedTransition { to, .. } => Some(to.as_str()),
            _ => None,
        })
        .collect();

    let remaining: Vec<String> = match &compiled {
        Some(t) => t
            .states
            .keys()
            .filter(|name| !visited.contains(name.as_str()))
            .cloned()
            .collect(),
        None => vec![],
    };

    Some(DetailData {
        session_id: session_id.to_string(),
        gate_name,
        command,
        result,
        elapsed,
        evidence,
        current_state,
        directive,
        intent,
        template_name,
        history,
        remaining,
    })
}

/// Build a human-readable summary line for a single event.
fn build_event_summary(e: &crate::engine::types::Event) -> String {
    use crate::engine::types::EventPayload;
    match &e.payload {
        EventPayload::Transitioned { from, to, .. } => {
            format!("{} \u{2192} {}", from.as_deref().unwrap_or("(none)"), to)
        }
        EventPayload::EvidenceSubmitted { state, fields, .. } => {
            format!("evidence: {} ({} fields)", state, fields.len())
        }
        EventPayload::GateEvaluated { gate, outcome, .. } => {
            format!("gate: {} [{}]", gate, outcome)
        }
        EventPayload::DecisionRecorded { state, .. } => {
            format!("decision recorded in {}", state)
        }
        EventPayload::DirectedTransition { from, to, .. } => {
            format!("directed: {} \u{2192} {}", from, to)
        }
        EventPayload::Rewound { from, to, .. } => {
            format!("rewind: {} \u{2192} {}", from, to)
        }
        EventPayload::GateOverrideRecorded { gate, .. } => {
            format!("gate override: {}", gate)
        }
        EventPayload::ContextAdded { key, .. } => {
            format!("context: {}", key)
        }
        EventPayload::DefaultActionExecuted {
            state, exit_code, ..
        } => {
            format!("action in {} (exit {})", state, exit_code)
        }
        EventPayload::IntentUpdated { intent } => {
            format!("intent updated: {}", intent)
        }
        other => other.type_name().to_string(),
    }
}

/// Build a gate condition description from the compiled template for a gate name.
fn build_gate_condition(
    gate_name: &str,
    compiled: Option<&crate::template::types::CompiledTemplate>,
) -> Option<String> {
    let t = compiled?;
    for state in t.states.values() {
        if let Some(gate) = state.gates.get(gate_name) {
            let cond = match gate.gate_type.as_str() {
                "command" => format!("cmd: {}", gate.command),
                "context-exists" => format!("key: {}", gate.key),
                "context-matches" => format!("key: {}  pattern: {}", gate.key, gate.pattern),
                "children-complete" => "children: ? complete".to_string(),
                "request-leg" => format!("request: {}  leg: {}", gate.request, gate.leg),
                other => format!("type: {}", other),
            };
            return Some(cond);
        }
    }
    None
}

/// Compute the elapsed time since an ISO 8601 UTC timestamp string.
///
/// Falls back to `Duration::ZERO` on any parse error or future timestamp.
#[doc(hidden)]
pub fn compute_elapsed_since(timestamp: &str) -> Duration {
    parse_timestamp_to_systemtime(timestamp)
        .and_then(|event_time| SystemTime::now().duration_since(event_time).ok())
        .unwrap_or(Duration::ZERO)
}
