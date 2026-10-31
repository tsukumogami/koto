//! The dashboard's read of sessions other machines pushed to the bucket.
//!
//! [`RemoteReader::scan`] lists the bucket's top level for workspace
//! prefixes, lists each one with the delimiter `/ctx/`, which returns every
//! session's top-level objects (state file, `summary.json`, markers) with
//! their size, ETag and LastModified and folds its whole context tree into
//! one common prefix, and reads each session's [`SessionSummary`] and its
//! `migrated.json` marker when the listing shows one. It never fetches a
//! state file, so showing a remote row never costs the size of its history,
//! and it reads nothing under `ctx/`. The reader has no write method: it
//! lists and gets, and works with keys that grant only those.
//!
//! What the scan found is then decided by one pure function, [`classify`],
//! which drops removed, internal and locally held sessions, hides migrated
//! sources, and lists a session it can't trust a summary for as `unknown`.
//!
//! The interactive dashboard reads through a [`RemoteFeed`]: one background
//! thread that owns its own reader, scans on a fixed interval, and hands each
//! complete [`RemoteSnapshot`] to the UI thread over a channel, so no key
//! press or redraw ever waits on the bucket.
//!
//! Every request the reader makes fails after [`REQUEST_TIMEOUT`], and every
//! failure falls in one [`FailureClass`] ([`failure_class`]). A listing is
//! asked again only after a throttle; one that timed out isn't, so a bucket
//! that never answers costs a pass one request timeout. A failed top-level
//! listing ends the pass with no further request.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use s3::error::S3Error;
use s3::serde_types::ListBucketResult;
use s3::Bucket;

use crate::session::cloud::{
    is_workspace_prefix, list_retrying_when, marker_target_str, MIGRATED_MARKER,
};
use crate::session::state_file_name;
use crate::session::summary::{is_internal_name, SessionSummary, MAX_SUMMARY_BYTES, SUMMARY_FILE};
use crate::session::sync_stamp::normalize_etag;
use crate::session_view::sanitize_capped;

/// The delimiter that folds a session's context tree into one entry.
pub const CTX_DELIMITER: &str = "/ctx/";

/// How long one bucket request may take before it fails.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a summary may name a state ETag other than the listed one before
/// the session reads as `unknown`: the ordinary gap between a push's state
/// PUT and its summary PUT stays inside it, a release that pushed the state
/// without refreshing the summary doesn't.
pub const STALE_SUMMARY_WINDOW: Duration = Duration::from_secs(60);

/// The largest `migrated.json` the reader fetches, by listed size and body.
pub const MAX_MARKER_BYTES: usize = 64 * 1024;

/// How many workspace listings are in flight at once.
const FANOUT: usize = 64;

/// One top-level object of a session directory, as the listing reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedObject {
    /// The listed size in bytes.
    pub size: u64,
    /// The listed ETag, without its quotes.
    pub etag: String,
    /// The listed LastModified; `None` when it didn't parse.
    pub last_modified: Option<SystemTime>,
}

/// A session directory as one `/ctx/`-delimited listing shows it: its
/// top-level objects by file name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionListing {
    /// Top-level file name to what the listing says about it.
    pub objects: BTreeMap<String, ListedObject>,
}

impl SessionListing {
    /// The state file of session `name`, when the listing holds one.
    pub fn state(&self, name: &str) -> Option<&ListedObject> {
        self.objects.get(&state_file_name(name))
    }
}

/// Group a workspace listing's objects into session directories.
///
/// `keys` are `(key, object)` pairs from listings of `<workspace>/` with the
/// [`CTX_DELIMITER`]; only objects directly inside a session directory
/// (`<workspace>/<name>/<file>`) count, so nothing under a deeper path
/// becomes a row fact.
pub fn group_sessions<'a>(
    workspace: &str,
    keys: impl IntoIterator<Item = (&'a str, ListedObject)>,
) -> BTreeMap<String, SessionListing> {
    let prefix = format!("{workspace}/");
    let mut sessions: BTreeMap<String, SessionListing> = BTreeMap::new();
    for (key, object) in keys {
        let Some(rest) = key.strip_prefix(&prefix) else {
            continue;
        };
        let Some((name, file)) = rest.split_once('/') else {
            continue;
        };
        if name.is_empty() || file.is_empty() || file.contains('/') {
            continue;
        }
        sessions
            .entry(name.to_string())
            .or_default()
            .objects
            .insert(file.to_string(), object);
    }
    sessions
}

/// What reading a session's `summary.json` gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SummaryRead {
    /// The listing shows no summary (or none was asked for).
    Absent,
    /// Listed but not usable; the reason (`oversized`, `unfetched`,
    /// `malformed`, `schema`, `key-mismatch`).
    Unreadable(&'static str),
    /// Read and checked against its key.
    Read(Box<SessionSummary>),
}

/// Where a source's `migrated.json` says its copy went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigratedTo {
    /// The copy's workspace prefix.
    pub prefix: String,
    /// The copy's session name.
    pub session: String,
    /// The copy's `session_id`, when the marker records one.
    pub session_id: Option<String>,
}

/// One session directory a scan found, with what was read for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedSession {
    /// The workspace prefix the directory is under.
    pub workspace: String,
    /// The session name (the directory's name).
    pub name: String,
    /// Its top-level objects.
    pub listing: SessionListing,
    /// Its summary.
    pub summary: SummaryRead,
    /// Its `migrated.json`, when one was listed and read.
    pub marker: Option<MigratedTo>,
}

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

/// What a scan needs to know about this machine's own sessions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalView {
    /// The `session_id` of every local session.
    pub session_ids: HashSet<String>,
    /// `(workspace prefix, name)` of every local session whose home prefix
    /// is known.
    pub homes: HashSet<(String, String)>,
    /// The source each imported local session names.
    pub imports: Vec<ImportedSource>,
}

/// How a remote row is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowStatus {
    /// From its summary.
    Shown(Box<SessionSummary>),
    /// With status `unknown`: no summary, an unreadable one, or one that no
    /// longer describes the listed state file. Nothing but the listing's own
    /// facts is shown.
    Unknown {
        /// Why (`no-summary`, `stale`, or the summary's unreadable reason).
        reason: &'static str,
        /// The state file's listed LastModified, which the row's idle time
        /// is measured from.
        state_modified: Option<SystemTime>,
        /// The `session_id` a stale summary named; `None` otherwise.
        session_id: Option<String>,
    },
}

/// One session another machine pushed, as the dashboard lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRow {
    /// The workspace id (16-hex prefix) the session's copy lives under.
    pub workspace: String,
    /// The session name.
    pub name: String,
    /// What the row shows.
    pub status: RowStatus,
}

/// What [`classify`] decided for a scan.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Classified {
    /// The rows to list, in workspace then name order.
    pub rows: Vec<RemoteRow>,
    /// Migrated sources left out in favour of their copy.
    pub hidden_sources: usize,
    /// Rows listed as `unknown`.
    pub unreadable: usize,
}

/// Decide every session directory of a scan, in this order (the first rule
/// that matches decides):
///
/// 1. no state object: dropped, the session was removed;
/// 2. an internal name: dropped;
/// 3. a session this machine holds a copy of: dropped, it shows as its local
///    row. Matched by `session_id` when its summary was read, else by its
///    workspace prefix and name;
/// 4. a migrated source: hidden and counted, when its own `migrated.json`
///    names another session directory present in this scan (and, when that
///    copy's summary was read, by the copy's `session_id`), or a local
///    session's latest `session_imported` names it (by `session_id` when its
///    summary was read, else by workspace prefix and name);
/// 5. no summary: `unknown`, counted;
/// 6. an unreadable summary: `unknown`, counted;
/// 7. a summary whose `state_etag` isn't the listed state file's ETag, once
///    that file is more than [`STALE_SUMMARY_WINDOW`] old by `now` (or its
///    LastModified didn't parse): `unknown`, counted;
/// 8. otherwise shown from its summary.
pub fn classify(scan: &[ScannedSession], local: &LocalView, now: SystemTime) -> Classified {
    let present: HashMap<(&str, &str), &ScannedSession> = scan
        .iter()
        .filter(|s| s.listing.state(&s.name).is_some())
        .map(|s| ((s.workspace.as_str(), s.name.as_str()), s))
        .collect();
    let mut out = Classified::default();
    for session in scan {
        let Some(state) = session.listing.state(&session.name) else {
            continue;
        };
        if is_internal_name(&session.name) {
            continue;
        }
        let read = match &session.summary {
            SummaryRead::Read(summary) => Some(summary.as_ref()),
            _ => None,
        };
        let here = (session.workspace.clone(), session.name.clone());
        let held = match read {
            Some(summary) => local.session_ids.contains(&summary.session_id),
            None => local.homes.contains(&here),
        };
        if held {
            continue;
        }
        if is_migrated(session, read, &present, local) {
            out.hidden_sources += 1;
            continue;
        }
        let unknown = |reason, session_id: Option<&str>| RowStatus::Unknown {
            reason,
            state_modified: state.last_modified,
            session_id: session_id.map(str::to_string),
        };
        let status = match &session.summary {
            SummaryRead::Absent => unknown("no-summary", None),
            SummaryRead::Unreadable(reason) => unknown(reason, None),
            SummaryRead::Read(summary) if is_stale(summary, state, now) => {
                unknown("stale", Some(&summary.session_id))
            }
            SummaryRead::Read(summary) => RowStatus::Shown(summary.clone()),
        };
        if matches!(status, RowStatus::Unknown { .. }) {
            out.unreadable += 1;
        }
        out.rows.push(RemoteRow {
            workspace: session.workspace.clone(),
            name: session.name.clone(),
            status,
        });
    }
    out.rows.sort_by(|a, b| {
        (a.workspace.as_str(), a.name.as_str()).cmp(&(b.workspace.as_str(), b.name.as_str()))
    });
    out
}

/// Rule 4 of [`classify`]: whether `session` is a source whose copy this
/// scan or this machine holds.
fn is_migrated(
    session: &ScannedSession,
    read: Option<&SessionSummary>,
    present: &HashMap<(&str, &str), &ScannedSession>,
    local: &LocalView,
) -> bool {
    let by_marker = session.marker.as_ref().is_some_and(|to| {
        let is_self = to.prefix == session.workspace && to.session == session.name;
        !is_self
            && present
                .get(&(to.prefix.as_str(), to.session.as_str()))
                .is_some_and(|copy| match (&copy.summary, &to.session_id) {
                    (SummaryRead::Read(summary), Some(id)) => &summary.session_id == id,
                    _ => true,
                })
    });
    by_marker
        || local.imports.iter().any(|source| match read {
            Some(summary) => source.session_id == summary.session_id,
            None => {
                source.prefix.as_deref() == Some(session.workspace.as_str())
                    && source.name == session.name
            }
        })
}

/// Rule 7 of [`classify`]: the summary names another state ETag and the
/// listed state file is older than [`STALE_SUMMARY_WINDOW`].
fn is_stale(summary: &SessionSummary, state: &ListedObject, now: SystemTime) -> bool {
    if normalize_etag(&summary.state_etag) == state.etag {
        return false;
    }
    match state.last_modified {
        Some(modified) => now
            .duration_since(modified)
            .is_ok_and(|age| age > STALE_SUMMARY_WINDOW),
        None => true,
    }
}

/// One workspace's scan: the session directories it holds.
type WorkspaceScan = Result<Vec<ScannedSession>, S3Error>;

/// How a bucket request the reader made failed. Every failure is one of
/// these four; [`failure_class`] decides which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    /// The connection was refused, the host didn't resolve, or the answer
    /// wasn't a usable reply: a 5xx other than 503, or a body that isn't
    /// the listing asked for.
    Unreachable,
    /// HTTP 401 or 403.
    Denied,
    /// No answer within [`REQUEST_TIMEOUT`].
    Timeout,
    /// HTTP 429 or 503, after koto's usual retries.
    Throttled,
}

impl FailureClass {
    /// The class as the status line and the stderr note name it.
    pub fn name(self) -> &'static str {
        match self {
            FailureClass::Unreachable => "unreachable",
            FailureClass::Denied => "denied",
            FailureClass::Timeout => "timeout",
            FailureClass::Throttled => "throttled",
        }
    }
}

/// The class of a failed bucket request: 401 and 403 are `denied`, 429 and
/// 503 `throttled`, a connection that timed out (the request deadline, or a
/// read that waited out its socket timeout) `timeout`, and anything else,
/// a refused connection, a host that didn't resolve, another 5xx or a reply
/// that didn't parse, `unreachable`.
pub fn failure_class(e: &S3Error) -> FailureClass {
    match e {
        S3Error::HttpFailWithBody(401 | 403, _) => FailureClass::Denied,
        S3Error::HttpFailWithBody(429 | 503, _) => FailureClass::Throttled,
        S3Error::Atto(e) => match e.kind() {
            attohttpc::ErrorKind::Io(io) if is_timed_out(io) => FailureClass::Timeout,
            _ => FailureClass::Unreachable,
        },
        S3Error::Io(io) if is_timed_out(io) => FailureClass::Timeout,
        _ => FailureClass::Unreachable,
    }
}

/// Whether an I/O error is a wait that ran out: the request deadline
/// (`TimedOut`) or a socket read timeout (`WouldBlock` on Unix).
fn is_timed_out(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    )
}

/// The longest a host is shown, in characters.
const HOST_CAP: usize = 256;

/// The host of an endpoint authority as [`s3::Region::host`] gives it (no
/// scheme): no path, no userinfo, no port. Unsanitized.
fn host_of(authority: &str) -> String {
    let authority = authority.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit('@').next().unwrap_or_default();
    let host = match authority.strip_prefix('[') {
        Some(rest) => match rest.find(']') {
            Some(end) => &authority[..end + 2],
            None => authority,
        },
        None => match authority.rsplit_once(':') {
            Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => host,
            _ => authority,
        },
    };
    host.to_string()
}

/// Why a pass couldn't read some or all of the bucket: the failure's class,
/// the endpoint's host, and which workspaces it left unread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadFailure {
    /// The top-level listing's class, or the first failed workspace's (in
    /// workspace order).
    pub class: FailureClass,
    /// The endpoint's host, as configured. Shown only through
    /// [`ReadFailure::line`], which sanitizes it.
    pub host: String,
    /// The top-level listing failed, so no workspace was read; a holder
    /// keeps every remote row it last had.
    pub top_level: bool,
    /// Workspaces whose listing failed, sorted; a holder keeps the rows it
    /// last had for them. Empty when `top_level`.
    pub workspaces: Vec<String>,
}

impl ReadFailure {
    /// Whether a holder keeps the rows it last had for `workspace`.
    pub fn keeps(&self, workspace: &str) -> bool {
        self.top_level || self.workspaces.iter().any(|w| w == workspace)
    }

    /// The failure on one line, its class and host passed through the
    /// dashboard's sanitizer: `<class> at <host>: the bucket couldn't be
    /// listed` when the top-level listing failed, else `<n> workspace(s)
    /// couldn't be read; the first: <class> at <host>`.
    pub fn line(&self) -> String {
        let class = sanitize_capped(self.class.name(), HOST_CAP);
        let host = sanitize_capped(&self.host, HOST_CAP);
        if self.top_level {
            return format!("{class} at {host}: the bucket couldn't be listed");
        }
        let n = self.workspaces.len();
        format!(
            "{n} workspace{} couldn't be read; the first: {class} at {host}",
            if n == 1 { "" } else { "s" }
        )
    }
}

/// One pass over the bucket.
#[derive(Debug, Default)]
pub struct RemoteSnapshot {
    /// Every row listed, in workspace then name order.
    pub rows: Vec<RemoteRow>,
    /// Why the pass couldn't read some or all of the bucket, when it
    /// couldn't.
    pub failure: Option<ReadFailure>,
    /// Rows listed as `unknown`: no summary, or one that couldn't be read
    /// or no longer describes its state file.
    pub unreadable: usize,
    /// Migrated sources left out in favour of their copy.
    pub hidden_sources: usize,
}

impl RemoteSnapshot {
    /// Whether a holder keeps the rows it last had for `workspace`: its
    /// listing, or the top-level one, failed.
    pub fn keeps(&self, workspace: &str) -> bool {
        self.failure.as_ref().is_some_and(|f| f.keeps(workspace))
    }
}

/// A bucket reader for the dashboard: a copy of the configured bucket
/// client with a [`REQUEST_TIMEOUT`] on every request, reached only through
/// listings and GETs.
pub struct RemoteReader {
    bucket: Box<Bucket>,
    host: String,
}

impl RemoteReader {
    /// A reader of `bucket`, the client koto already resolved from its
    /// configuration. Constructing it makes no request.
    pub fn new(bucket: &Bucket) -> anyhow::Result<Self> {
        Ok(Self {
            bucket: bucket.with_request_timeout(REQUEST_TIMEOUT)?,
            host: host_of(&bucket.region().host()),
        })
    }

    /// A listing of `prefix`, asked again only while the bucket throttles
    /// it (koto's usual retries), never after a timeout or a refusal.
    fn list(
        &self,
        prefix: String,
        delimiter: Option<&str>,
    ) -> Result<Vec<ListBucketResult>, S3Error> {
        list_retrying_when(&self.bucket, prefix, delimiter, |e| {
            failure_class(e) == FailureClass::Throttled
        })
    }

    /// A failure of this reader's bucket.
    fn failure(&self, class: FailureClass, workspaces: Vec<String>) -> ReadFailure {
        ReadFailure {
            class,
            host: self.host.clone(),
            top_level: workspaces.is_empty(),
            workspaces,
        }
    }

    /// The bucket's workspace prefixes, from one top-level listing.
    fn workspaces(&self) -> Result<Vec<String>, S3Error> {
        let pages = self.list(String::new(), Some("/"))?;
        Ok(pages
            .iter()
            .filter_map(|page| page.common_prefixes.as_ref())
            .flatten()
            .filter_map(|cp| cp.prefix.strip_suffix('/'))
            .filter(|p| is_workspace_prefix(p))
            .map(str::to_string)
            .collect())
    }

    /// The session directories under `workspace`, from its
    /// `/ctx/`-delimited listing, every page followed.
    pub fn list_workspace(
        &self,
        workspace: &str,
    ) -> Result<BTreeMap<String, SessionListing>, S3Error> {
        let pages = self.list(format!("{workspace}/"), Some(CTX_DELIMITER))?;
        Ok(group_sessions(
            workspace,
            pages
                .iter()
                .flat_map(|page| page.contents.iter())
                .map(|obj| {
                    (
                        obj.key.as_str(),
                        ListedObject {
                            size: obj.size,
                            etag: obj.e_tag.as_deref().map(normalize_etag).unwrap_or_default(),
                            last_modified: crate::engine::wake::parse_rfc3339_millis(
                                &obj.last_modified,
                            ),
                        },
                    )
                }),
        ))
    }

    /// GET `<workspace>/<name>/<file>`; `None` when it fails or doesn't
    /// answer 200.
    fn get(&self, workspace: &str, name: &str, file: &str) -> Option<Vec<u8>> {
        let response = self
            .bucket
            .get_object(format!("{workspace}/{name}/{file}"))
            .ok()?;
        (response.status_code() == 200).then(|| response.bytes().to_vec())
    }

    /// Read one session's summary.
    fn read_summary(&self, workspace: &str, name: &str) -> SummaryRead {
        match self.get(workspace, name, SUMMARY_FILE) {
            None => SummaryRead::Unreadable("unfetched"),
            Some(bytes) => match SessionSummary::parse(&bytes, workspace, name) {
                Ok(summary) => SummaryRead::Read(Box::new(summary)),
                Err(reason) => SummaryRead::Unreadable(reason),
            },
        }
    }

    /// Read one session's `migrated.json`; `None` when it can't be read or
    /// doesn't name a copy.
    fn read_marker(&self, workspace: &str, name: &str) -> Option<MigratedTo> {
        let bytes = self.get(workspace, name, MIGRATED_MARKER)?;
        if bytes.len() > MAX_MARKER_BYTES {
            return None;
        }
        Some(MigratedTo {
            prefix: marker_target_str(&bytes, "prefix")?,
            session: marker_target_str(&bytes, "session")?,
            session_id: marker_target_str(&bytes, "session_id"),
        })
    }

    /// The session directories of one workspace, with the summary and
    /// marker of each one that has a state object and isn't internal. A
    /// summary over [`MAX_SUMMARY_BYTES`] by its listed size isn't fetched.
    fn scan_workspace(&self, workspace: &str) -> WorkspaceScan {
        let mut sessions = Vec::new();
        for (name, listing) in self.list_workspace(workspace)? {
            let wanted = listing.state(&name).is_some() && !is_internal_name(&name);
            let summary = match listing.objects.get(SUMMARY_FILE) {
                Some(_) if !wanted => SummaryRead::Absent,
                Some(obj) if obj.size > MAX_SUMMARY_BYTES as u64 => {
                    SummaryRead::Unreadable("oversized")
                }
                Some(_) => self.read_summary(workspace, &name),
                None => SummaryRead::Absent,
            };
            let marker = match listing.objects.get(MIGRATED_MARKER) {
                Some(obj) if wanted && obj.size <= MAX_MARKER_BYTES as u64 => {
                    self.read_marker(workspace, &name)
                }
                _ => None,
            };
            sessions.push(ScannedSession {
                workspace: workspace.to_string(),
                name,
                listing,
                summary,
                marker,
            });
        }
        Ok(sessions)
    }

    /// One pass over the bucket: every workspace prefix, each listed with
    /// the `/ctx/` delimiter, [`FANOUT`] at a time, then [`classify`]d as a
    /// whole against `local`. A failed top-level listing ends the pass with
    /// no further request. A failed workspace leaves the others' rows
    /// standing and is named in `failure`, which carries the class of the
    /// first one in workspace order.
    pub fn scan(&self, local: &LocalView) -> RemoteSnapshot {
        let workspaces = match self.workspaces() {
            Ok(w) => w,
            Err(e) => {
                return RemoteSnapshot {
                    failure: Some(self.failure(failure_class(&e), Vec::new())),
                    ..RemoteSnapshot::default()
                }
            }
        };
        let next = AtomicUsize::new(0);
        let results: Mutex<Vec<(String, WorkspaceScan)>> = Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for _ in 0..workspaces.len().min(FANOUT) {
                scope.spawn(|| {
                    while let Some(ws) = workspaces.get(next.fetch_add(1, Ordering::Relaxed)) {
                        let result = self.scan_workspace(ws);
                        results
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .push((ws.clone(), result));
                    }
                });
            }
        });
        let mut snapshot = RemoteSnapshot::default();
        let mut scanned = Vec::new();
        let mut failed: Vec<(String, FailureClass)> = Vec::new();
        for (ws, result) in results.into_inner().unwrap_or_else(|e| e.into_inner()) {
            match result {
                Ok(sessions) => scanned.extend(sessions),
                Err(e) => failed.push((ws, failure_class(&e))),
            }
        }
        failed.sort_by(|a, b| a.0.cmp(&b.0));
        if let Some(&(_, class)) = failed.first() {
            let workspaces = failed.into_iter().map(|(ws, _)| ws).collect();
            snapshot.failure = Some(self.failure(class, workspaces));
        }
        let classified = classify(&scanned, local, SystemTime::now());
        snapshot.rows = classified.rows;
        snapshot.unreadable = classified.unreadable;
        snapshot.hidden_sources = classified.hidden_sources;
        snapshot
    }
}

/// How long the feed thread waits between the end of one scan and the
/// start of the next.
pub const RESCAN_INTERVAL: Duration = Duration::from_secs(55);

/// How often a waiting feed thread looks at its stop flag.
const STOP_POLL: Duration = Duration::from_millis(100);

/// The interactive dashboard's view of the bucket: a background thread that
/// scans at start and then every `interval`, and sends each complete
/// snapshot to the thread holding this handle.
///
/// The thread owns its own [`RemoteReader`]; the holder never touches a
/// bucket. Dropping the handle tells the thread to stop, and nothing waits
/// for it: a thread stuck in a request that never answers ends with the
/// process.
pub struct RemoteFeed {
    snapshots: Receiver<RemoteSnapshot>,
    local: Arc<Mutex<LocalView>>,
    stop: Arc<AtomicBool>,
}

impl RemoteFeed {
    /// Start the feed thread on a copy of `bucket`. `local` describes the
    /// sessions this machine holds, which every scan leaves to their local
    /// rows; [`RemoteFeed::set_local`] replaces it. Starting makes no
    /// request on the calling thread.
    pub fn spawn(bucket: &Bucket, local: LocalView, interval: Duration) -> anyhow::Result<Self> {
        let reader = RemoteReader::new(bucket)?;
        let (tx, snapshots) = mpsc::channel();
        let local = Arc::new(Mutex::new(local));
        let stop = Arc::new(AtomicBool::new(false));
        let (view, stopped) = (Arc::clone(&local), Arc::clone(&stop));
        std::thread::Builder::new()
            .name("koto-remote-feed".to_string())
            .spawn(move || loop {
                if stopped.load(Ordering::Relaxed) {
                    return;
                }
                let view = view.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let snapshot = reader.scan(&view);
                if stopped.load(Ordering::Relaxed) || tx.send(snapshot).is_err() {
                    return;
                }
                let until = Instant::now() + interval;
                while Instant::now() < until {
                    if stopped.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(
                        STOP_POLL.min(until.saturating_duration_since(Instant::now())),
                    );
                }
            })?;
        Ok(Self {
            snapshots,
            local,
            stop,
        })
    }

    /// The newest snapshot the thread has sent since the last call, if any;
    /// older ones still queued are dropped. Never blocks.
    pub fn latest(&self) -> Option<RemoteSnapshot> {
        let mut latest = None;
        loop {
            match self.snapshots.try_recv() {
                Ok(snapshot) => latest = Some(snapshot),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return latest,
            }
        }
    }

    /// Replace what the next scan knows about this machine's sessions.
    pub fn set_local(&self, local: LocalView) {
        *self.local.lock().unwrap_or_else(|e| e.into_inner()) = local;
    }
}

impl Drop for RemoteFeed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WS: &str = "0123456789abcdef";
    const OTHER: &str = "fedcba9876543210";

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }

    fn obj(etag: &str, age_secs: u64) -> ListedObject {
        ListedObject {
            size: 10,
            etag: etag.to_string(),
            last_modified: Some(now() - Duration::from_secs(age_secs)),
        }
    }

    fn summary(ws: &str, name: &str, id: &str, state_etag: &str) -> SessionSummary {
        SessionSummary {
            schema: crate::session::summary::SUMMARY_SCHEMA,
            session_id: id.to_string(),
            name: name.to_string(),
            home: ws.to_string(),
            kind: None,
            parent: None,
            created_at: "2026-10-31T00:00:00Z".to_string(),
            template_name: Some("t".to_string()),
            current_state: Some("check".to_string()),
            is_terminal: false,
            is_blocked: true,
            failed: false,
            intent: None,
            salient_var: None,
            is_batch_task: false,
            is_request_leg: false,
            last_event_at: None,
            last_seq: 2,
            state_etag: state_etag.to_string(),
            machine: Some("machine-a".to_string()),
            sources: Vec::new(),
        }
    }

    /// A session directory with a state file (ETag `s`, 600 s old) and,
    /// when `read` is given, a summary describing it.
    fn session(ws: &str, name: &str, summary_read: SummaryRead) -> ScannedSession {
        let mut listing = SessionListing::default();
        listing.objects.insert(state_file_name(name), obj("s", 600));
        if !matches!(summary_read, SummaryRead::Absent) {
            listing
                .objects
                .insert(SUMMARY_FILE.to_string(), obj("x", 600));
        }
        ScannedSession {
            workspace: ws.to_string(),
            name: name.to_string(),
            listing,
            summary: summary_read,
            marker: None,
        }
    }

    fn shown(ws: &str, name: &str, id: &str) -> ScannedSession {
        session(
            ws,
            name,
            SummaryRead::Read(Box::new(summary(ws, name, id, "s"))),
        )
    }

    fn names(c: &Classified) -> Vec<(&str, &str, bool)> {
        c.rows
            .iter()
            .map(|r| {
                (
                    r.workspace.as_str(),
                    r.name.as_str(),
                    matches!(r.status, RowStatus::Shown(_)),
                )
            })
            .collect()
    }

    #[test]
    fn group_sessions_keeps_only_top_level_objects_with_their_facts() {
        let o = |size| ListedObject {
            size,
            etag: "e".to_string(),
            last_modified: None,
        };
        let keys = [
            ("0123456789abcdef/review/koto-review.state.jsonl", o(10)),
            ("0123456789abcdef/review/summary.json", o(5)),
            ("0123456789abcdef/review/backrefs/x.json", o(1)),
            ("0123456789abcdef/other/koto-other.state.jsonl", o(3)),
            ("fedcba9876543210/review/summary.json", o(5)),
            ("0123456789abcdef/loose.json", o(1)),
        ];
        let sessions = group_sessions(WS, keys);
        assert_eq!(sessions.keys().collect::<Vec<_>>(), vec!["other", "review"]);
        let review = &sessions["review"].objects;
        assert_eq!(review.len(), 2);
        assert_eq!(review.get("summary.json"), Some(&o(5)));
        assert!(sessions["review"].state("review").is_some());
    }

    #[test]
    fn rule_1_a_directory_without_a_state_object_is_dropped() {
        let mut s = shown(WS, "gone", "id");
        s.listing.objects.remove(&state_file_name("gone"));
        let c = classify(&[s], &LocalView::default(), now());
        assert_eq!(c, Classified::default());
    }

    #[test]
    fn rule_2_an_internal_name_is_dropped_even_without_a_summary() {
        let scan = [
            session(WS, "wf~1", SummaryRead::Absent),
            session(WS, "anchor-77d6", SummaryRead::Absent),
            session(WS, "anchor-77d6-closed", SummaryRead::Absent),
        ];
        let c = classify(&scan, &LocalView::default(), now());
        assert_eq!(c, Classified::default());
    }

    #[test]
    fn rule_3_a_local_copy_is_dropped_by_session_id() {
        let local = LocalView {
            session_ids: HashSet::from(["id".to_string()]),
            ..LocalView::default()
        };
        let c = classify(&[shown(WS, "wf", "id")], &local, now());
        assert_eq!(c, Classified::default());
    }

    #[test]
    fn rule_3_a_local_copy_without_a_readable_summary_is_dropped_by_home_and_name() {
        let local = LocalView {
            homes: HashSet::from([(WS.to_string(), "wf".to_string())]),
            ..LocalView::default()
        };
        let scan = [
            session(WS, "wf", SummaryRead::Absent),
            session(WS, "wf2", SummaryRead::Absent),
            session(OTHER, "wf", SummaryRead::Absent),
        ];
        let c = classify(&scan, &local, now());
        assert_eq!(names(&c), vec![(WS, "wf2", false), (OTHER, "wf", false)]);

        let truncated = [session(WS, "wf", SummaryRead::Unreadable("malformed"))];
        assert_eq!(classify(&truncated, &local, now()), Classified::default());

        // A readable summary is matched by session_id only.
        let c = classify(&[shown(WS, "wf", "other-id")], &local, now());
        assert_eq!(names(&c), vec![(WS, "wf", true)]);
    }

    #[test]
    fn rule_4_a_marker_naming_a_copy_in_the_scan_hides_the_source() {
        // The source has no summary, so rule 5 would list it as unknown.
        let mut source = session(WS, "wf", SummaryRead::Absent);
        source.marker = Some(MigratedTo {
            prefix: OTHER.to_string(),
            session: "copy".to_string(),
            session_id: Some("copy-id".to_string()),
        });
        let scan = [source, shown(OTHER, "copy", "copy-id")];
        let c = classify(&scan, &LocalView::default(), now());
        assert_eq!(c.hidden_sources, 1);
        assert_eq!(c.unreadable, 0);
        assert_eq!(names(&c), vec![(OTHER, "copy", true)]);
    }

    #[test]
    fn rule_4_a_marker_whose_copy_is_not_in_the_scan_lists_the_source() {
        let mut source = shown(WS, "wf", "id");
        source.marker = Some(MigratedTo {
            prefix: OTHER.to_string(),
            session: "copy".to_string(),
            session_id: None,
        });
        // A copy directory with no state object isn't a copy in the scan.
        let mut stateless = shown(OTHER, "copy", "copy-id");
        stateless.listing.objects.remove(&state_file_name("copy"));
        let c = classify(&[source, stateless], &LocalView::default(), now());
        assert_eq!(c.hidden_sources, 0);
        assert_eq!(names(&c), vec![(WS, "wf", true)]);
    }

    #[test]
    fn rule_4_a_marker_naming_itself_or_a_copy_with_another_id_hides_nothing() {
        let mut source = shown(WS, "wf", "id");
        source.marker = Some(MigratedTo {
            prefix: OTHER.to_string(),
            session: "copy".to_string(),
            session_id: Some("copy-id".to_string()),
        });
        let mut selfish = shown(WS, "self", "self-id");
        selfish.marker = Some(MigratedTo {
            prefix: WS.to_string(),
            session: "self".to_string(),
            session_id: None,
        });
        let scan = [source, selfish, shown(OTHER, "copy", "someone-else")];
        let c = classify(&scan, &LocalView::default(), now());
        assert_eq!(c.hidden_sources, 0);
        assert_eq!(c.rows.len(), 3);
    }

    #[test]
    fn rule_4_a_local_import_hides_its_source() {
        let local = LocalView {
            imports: vec![
                ImportedSource {
                    prefix: Some("1111111111111111".to_string()),
                    name: "elsewhere".to_string(),
                    session_id: "src-id".to_string(),
                },
                ImportedSource {
                    prefix: Some(WS.to_string()),
                    name: "legacy".to_string(),
                    session_id: "unknown-to-us".to_string(),
                },
            ],
            ..LocalView::default()
        };
        let scan = [
            shown(WS, "wf", "src-id"),
            session(WS, "legacy", SummaryRead::Absent),
            shown(WS, "legacy2", "other"),
        ];
        let c = classify(&scan, &local, now());
        assert_eq!(c.hidden_sources, 2);
        assert_eq!(names(&c), vec![(WS, "legacy2", true)]);
    }

    #[test]
    fn rule_5_no_summary_lists_unknown_with_the_state_files_time() {
        let c = classify(
            &[session(WS, "old", SummaryRead::Absent)],
            &LocalView::default(),
            now(),
        );
        assert_eq!(c.unreadable, 1);
        assert_eq!(
            c.rows[0].status,
            RowStatus::Unknown {
                reason: "no-summary",
                state_modified: Some(now() - Duration::from_secs(600)),
                session_id: None,
            }
        );
    }

    #[test]
    fn rule_6_an_unreadable_summary_lists_unknown() {
        let scan = [
            session(WS, "a", SummaryRead::Unreadable("malformed")),
            session(WS, "b", SummaryRead::Unreadable("oversized")),
        ];
        let c = classify(&scan, &LocalView::default(), now());
        assert_eq!(c.unreadable, 2);
        assert_eq!(names(&c), vec![(WS, "a", false), (WS, "b", false)]);
    }

    #[test]
    fn rule_7_a_stale_summary_reads_unknown_only_past_60_seconds() {
        let at = |age: u64, etag: &str| {
            let mut s = shown(WS, "wf", "id");
            s.listing
                .objects
                .insert(state_file_name("wf"), obj(etag, age));
            classify(&[s], &LocalView::default(), now())
        };
        let fresh = at(10, "new");
        assert_eq!(
            (fresh.unreadable, names(&fresh)),
            (0, vec![(WS, "wf", true)])
        );
        let edge = at(60, "new");
        assert_eq!(edge.unreadable, 0, "60 s is still inside the window");
        let old = at(120, "new");
        assert_eq!((old.unreadable, names(&old)), (1, vec![(WS, "wf", false)]));
        assert!(matches!(
            &old.rows[0].status,
            RowStatus::Unknown { reason: "stale", session_id: Some(id), .. } if id == "id"
        ));
        let same = at(120, "s");
        assert_eq!(same.unreadable, 0, "a matching ETag is never stale");

        // A quoted ETag in the summary matches its unquoted listing.
        let mut s = session(
            WS,
            "q",
            SummaryRead::Read(Box::new(summary(WS, "q", "id", "\"s\""))),
        );
        s.listing
            .objects
            .insert(state_file_name("q"), obj("s", 120));
        assert_eq!(classify(&[s], &LocalView::default(), now()).unreadable, 0);

        // A LastModified that didn't parse counts as old.
        let mut s = shown(WS, "wf", "id");
        s.listing.objects.insert(
            state_file_name("wf"),
            ListedObject {
                size: 1,
                etag: "new".to_string(),
                last_modified: None,
            },
        );
        assert_eq!(classify(&[s], &LocalView::default(), now()).unreadable, 1);
    }

    #[test]
    fn rule_8_otherwise_the_row_is_shown_from_its_summary_in_order() {
        let scan = [
            shown(OTHER, "b", "1"),
            shown(WS, "z", "2"),
            shown(WS, "a", "3"),
        ];
        let c = classify(&scan, &LocalView::default(), now());
        assert_eq!(
            names(&c),
            vec![(WS, "a", true), (WS, "z", true), (OTHER, "b", true)]
        );
        assert_eq!((c.unreadable, c.hidden_sources), (0, 0));
    }

    // -- failure classes ----------------------------------------------------

    fn atto(kind: std::io::ErrorKind) -> S3Error {
        S3Error::Atto(attohttpc::Error::from(std::io::Error::from(kind)))
    }

    /// A client of a bucket at `endpoint` whose requests fail after
    /// `timeout`.
    fn bucket_at(endpoint: &str, timeout: Duration) -> Box<Bucket> {
        let region = s3::Region::Custom {
            region: "us-east-1".to_string(),
            endpoint: endpoint.to_string(),
        };
        let credentials =
            s3::creds::Credentials::new(Some("k"), Some("s"), None, None, None).unwrap();
        Bucket::new("b", region, credentials)
            .unwrap()
            .with_path_style()
            .with_request_timeout(timeout)
            .unwrap()
    }

    fn list_error(bucket: &Bucket) -> S3Error {
        bucket
            .list(String::new(), Some("/".to_string()))
            .expect_err("the listing fails")
    }

    #[test]
    fn failure_class_unreachable_for_a_refused_connection_dns_a_5xx_or_a_bad_body() {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let refused = list_error(&bucket_at(
            &format!("http://127.0.0.1:{port}"),
            Duration::from_secs(5),
        ));
        assert!(matches!(refused, S3Error::Atto(_)), "{refused:?}");
        for e in [
            refused,
            atto(std::io::ErrorKind::ConnectionRefused),
            S3Error::Atto(attohttpc::Error::from(std::io::Error::other(
                "failed to lookup address information",
            ))),
            S3Error::HttpFailWithBody(500, String::new()),
            S3Error::HttpFailWithBody(502, String::new()),
            S3Error::HttpFailWithBody(504, String::new()),
            S3Error::HttpFail,
            S3Error::Io(std::io::Error::from(std::io::ErrorKind::UnexpectedEof)),
        ] {
            assert_eq!(failure_class(&e), FailureClass::Unreachable, "{e:?}");
        }
        assert_eq!(FailureClass::Unreachable.name(), "unreachable");
    }

    #[test]
    fn failure_class_denied_for_401_and_403() {
        for status in [401, 403] {
            let e = S3Error::HttpFailWithBody(status, "<Error/>".to_string());
            assert_eq!(failure_class(&e), FailureClass::Denied, "{status}");
        }
        assert_eq!(FailureClass::Denied.name(), "denied");
    }

    #[test]
    fn failure_class_timeout_for_a_request_that_gets_no_answer() {
        // A listener that accepts and never answers: the request deadline
        // ends the request.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let held: Vec<_> = listener.incoming().take(4).collect();
            std::thread::sleep(Duration::from_secs(30));
            drop(held);
        });
        let silent = list_error(&bucket_at(&endpoint, Duration::from_millis(200)));
        for e in [
            silent,
            atto(std::io::ErrorKind::TimedOut),
            atto(std::io::ErrorKind::WouldBlock),
            S3Error::Io(std::io::Error::from(std::io::ErrorKind::TimedOut)),
        ] {
            assert_eq!(failure_class(&e), FailureClass::Timeout, "{e:?}");
        }
        assert_eq!(FailureClass::Timeout.name(), "timeout");
    }

    #[test]
    fn failure_class_throttled_for_429_and_503() {
        for status in [429, 503] {
            let e = S3Error::HttpFailWithBody(status, "<Error/>".to_string());
            assert_eq!(failure_class(&e), FailureClass::Throttled, "{status}");
        }
        assert_eq!(FailureClass::Throttled.name(), "throttled");
    }

    #[test]
    fn host_of_drops_userinfo_port_and_path() {
        assert_eq!(host_of("127.0.0.1:9000"), "127.0.0.1");
        assert_eq!(
            host_of("user:secret@minio.example:9000/base"),
            "minio.example"
        );
        assert_eq!(host_of("s3.amazonaws.com"), "s3.amazonaws.com");
        assert_eq!(host_of("[::1]:9000"), "[::1]");
        assert_eq!(
            host_of(
                &bucket_at("http://k:s@10.0.0.7:9000/", REQUEST_TIMEOUT)
                    .region()
                    .host()
            ),
            "10.0.0.7"
        );
    }

    #[test]
    fn a_read_failure_line_names_the_class_and_the_sanitized_host() {
        let top = ReadFailure {
            class: FailureClass::Denied,
            host: "evil\u{1b}[2J\u{202e}host\nx".to_string(),
            top_level: true,
            workspaces: Vec::new(),
        };
        let line = top.line();
        assert_eq!(
            line,
            "denied at evil\\u{1B}[2J\\u{202E}host x: the bucket couldn't be listed"
        );
        assert!(!line.chars().any(|c| c.is_control()), "{line:?}");
        assert!(top.keeps(WS) && top.keeps(OTHER));

        let some = ReadFailure {
            class: FailureClass::Unreachable,
            host: "127.0.0.1".to_string(),
            top_level: false,
            workspaces: vec![WS.to_string()],
        };
        assert_eq!(
            some.line(),
            "1 workspace couldn't be read; the first: unreachable at 127.0.0.1"
        );
        assert!(some.keeps(WS) && !some.keeps(OTHER));
        let two = ReadFailure {
            workspaces: vec![WS.to_string(), OTHER.to_string()],
            ..some
        };
        assert!(two.line().starts_with("2 workspaces couldn't be read"));
    }
}
