//! Anchors: one koto session per agent session, which every session the
//! agent session opens or ticks names in a held-by link
//! (DESIGN-session-retention.md, Decision 2).
//!
//! An anchor is an ordinary session in the store whose header carries
//! `kind: "anchor"`. Its one-state template never finishes and is embedded
//! in koto, so an anchor never depends on the template cache or a plugin
//! path. `koto session anchor open` creates or continues it, and every open
//! appends an `anchor_started` event. `koto init` and `koto next` read
//! `KOTO_ANCHOR` and write a held-by link to it in the session they opened
//! or ticked, plus a throttled `anchor_activity` event in the anchor.
//!
//! The anchor for harness session `S` is `anchor-<S>`. An anchor that reads
//! closed by an observed close, or that was migrated, gets a successor,
//! `anchor-<S>-s2`, `-s3` and so on, holding a dispatched-by link to the one
//! before it. A `/clear`, which gives the agent session a new id, and a
//! resume under a new id open a new anchor linked to its predecessor.
//!
//! An observed close never writes into the anchor: it writes a separate
//! close session, `<anchor>-closed` (`kind: "anchor-close"`), staged under
//! [`STAGING_PREFIX`] and renamed into place so no reader sees a partial
//! one. [`close_check`] is the one place that decides whether an anchor
//! reads closed.
//!
//! Everything here reads the local store directly when it scans (no pull,
//! no lock), and writes through the store's own methods.

use std::io::{BufRead, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::engine::types::{
    generate_session_id, iso8601_at, now_iso8601, AnchorActivity, AnchorStarted, Event,
    EventPayload, SessionKind, StateFileHeader, ANCHOR_STARTED,
};
use crate::session::context::ContextStore;
use crate::session::context_log::{added_event, append_to_session_best_effort, WRITER_KOTO};
use crate::session::links::{self, LinkHints, LinkKind};
use crate::session::local::LocalBackend;
use crate::session::{state_file_name, SessionBackend, SessionError};

/// The anchor's embedded template: one state that never finishes.
pub const ANCHOR_TEMPLATE: &str = include_str!("anchor_template.md");

/// The close session's embedded template: its initial state is terminal.
pub const ANCHOR_CLOSE_TEMPLATE: &str = include_str!("anchor_close_template.md");

/// Every anchor's name starts with this.
pub const ANCHOR_PREFIX: &str = "anchor-";

/// A close session is its anchor's name plus this.
pub const CLOSED_SUFFIX: &str = "-closed";

/// Reserved prefix of a close session being staged in the session
/// directory. No session name can start with a dot, so listing, prune and
/// every other walk of the store already pass these by; [`sweep_staging`]
/// removes a leftover one once it is older than [`STAGING_SWEEP_AGE`].
pub const STAGING_PREFIX: &str = ".koto-staging-";

/// How old a staging entry must be before a close sweeps it. A fresher one
/// may be a close still in progress.
pub const STAGING_SWEEP_AGE: Duration = Duration::from_secs(10 * 60);

/// The anchor's observed marker: the key whose reap closes it.
pub const OBSERVED_KEY: &str = "koto/anchor/observed";

/// The close session's record of what it closes.
pub const CLOSE_RECORD_KEY: &str = "koto/anchor-close";

/// The variable naming the running agent session's anchor. koto reads this
/// one only, never `KOTO_DISPATCHER_ANCHOR`.
pub const ANCHOR_ENV: &str = "KOTO_ANCHOR";

/// The idle period's environment override (`retention.idle_period`).
pub const IDLE_PERIOD_ENV: &str = "KOTO_RETENTION_IDLE_PERIOD";

/// The idle period when nothing sets one.
pub const DEFAULT_IDLE_PERIOD: Duration = Duration::from_secs(7 * 86_400);

/// How long after a `/clear` the start hook keeps copying the
/// predecessor's marker with `observe --inherit`.
pub const INHERIT_WINDOW: Duration = Duration::from_secs(180);

/// The lock file, in the directory of a family's first anchor, that every
/// open and observe of the family holds.
const FAMILY_LOCK_FILE: &str = ".anchor-open.lock";

/// Hex characters kept from a SHA-256 digest of the process token or the
/// transcript path.
const DIGEST_HEX_LEN: usize = 32;

/// A successor opened because its predecessor read closed.
pub const REASON_CLOSED: &str = "closed";
/// A successor opened because its predecessor was migrated.
pub const REASON_MIGRATED: &str = "migrated";
/// A new anchor after a resume under a new id, linked through a hint.
pub const REASON_RESUME: &str = "resume";
/// A new anchor after a `/clear`, linked through the process token.
pub const REASON_CLEAR: &str = "clear";

/// A caller's mistake: a malformed id, key or name, or a name held by a
/// session that isn't an anchor. The CLI exits 2 on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorRefusal(pub String);

impl std::fmt::Display for AnchorRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AnchorRefusal {}

fn refuse<T>(message: impl Into<String>) -> anyhow::Result<T> {
    Err(AnchorRefusal(message.into()).into())
}

/// Whether `value` matches `^[A-Za-z0-9_-]{1,128}$`, the pattern harness
/// session ids and reap keys share with the start hook and niwa.
pub fn valid_token(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Whether `name` ends in a successor suffix, `-s<n>`.
fn successor_suffix(name: &str) -> Option<u32> {
    let (_, n) = name.rsplit_once("-s")?;
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) || n.starts_with('0') {
        return None;
    }
    n.parse().ok().filter(|n| *n >= 2)
}

/// Check a harness session id for `open`: the shared pattern, and no
/// ending that would make the anchor's name read as a close session or a
/// successor.
pub fn validate_harness_session(id: &str) -> Result<(), AnchorRefusal> {
    if !valid_token(id) {
        return Err(AnchorRefusal(format!(
            "harness session id '{}' must match ^[A-Za-z0-9_-]{{1,128}}$",
            preview(id)
        )));
    }
    if id.ends_with(CLOSED_SUFFIX) {
        return Err(AnchorRefusal(format!(
            "harness session id '{id}' ends in '{CLOSED_SUFFIX}', which names a close session"
        )));
    }
    if successor_suffix(id).is_some() {
        return Err(AnchorRefusal(format!(
            "harness session id '{id}' ends in '-s<n>', which names a successor anchor"
        )));
    }
    Ok(())
}

fn preview(value: &str) -> String {
    value.chars().take(64).collect()
}

/// The anchor for harness session `harness`.
pub fn anchor_name(harness: &str) -> String {
    format!("{ANCHOR_PREFIX}{harness}")
}

/// The `n`th successor of the family whose first anchor is `base`.
pub fn successor_name(base: &str, n: u32) -> String {
    format!("{base}-s{n}")
}

/// The close session of `anchor`.
pub fn close_name(anchor: &str) -> String {
    format!("{anchor}{CLOSED_SUFFIX}")
}

/// The first anchor of the family `anchor` belongs to: the name with any
/// successor suffix removed.
pub fn family_base(anchor: &str) -> &str {
    match successor_suffix(anchor) {
        Some(n) => &anchor[..anchor.len() - format!("-s{n}").len()],
        None => anchor,
    }
}

/// A SHA-256 prefix of `value`, so a process token or transcript path is
/// never stored in clear text.
pub fn digest(value: &str) -> String {
    let mut hex = crate::cache::sha256_hex(value.as_bytes());
    hex.truncate(DIGEST_HEX_LEN);
    hex
}

/// The idle period: [`IDLE_PERIOD_ENV`] when it parses as a non-zero
/// duration, [`DEFAULT_IDLE_PERIOD`] otherwise. A malformed value never
/// fails a command that reads it.
pub fn idle_period() -> Duration {
    std::env::var(IDLE_PERIOD_ENV)
        .ok()
        .and_then(|v| crate::session::duration::parse_age(v.trim()))
        .filter(|d| !d.is_zero())
        .unwrap_or(DEFAULT_IDLE_PERIOD)
}

// ---------------------------------------------------------------------
// Reading anchors
// ---------------------------------------------------------------------

/// The sessions directory `store` keeps its sessions in.
fn base_dir<S: SessionBackend + ?Sized>(store: &S) -> PathBuf {
    let probe = store.session_dir("x");
    probe.parent().map(Path::to_path_buf).unwrap_or(probe)
}

fn state_path<S: SessionBackend + ?Sized>(store: &S, name: &str) -> PathBuf {
    store.session_dir(name).join(state_file_name(name))
}

/// The header of `name`'s local state file, read straight from the file.
fn local_header<S: SessionBackend + ?Sized>(store: &S, name: &str) -> Option<StateFileHeader> {
    if crate::discover::validate_workflow_name(name).is_err() {
        return None;
    }
    crate::engine::persistence::read_header(&state_path(store, name)).ok()
}

/// What `name`'s header says it is, or `None` when it has no readable
/// local header.
pub fn kind_of<S: SessionBackend + ?Sized>(store: &S, name: &str) -> Option<SessionKind> {
    local_header(store, name).map(|h| h.session_kind())
}

/// Whether `name` is an anchor in the local store.
pub fn is_anchor<S: SessionBackend + ?Sized>(store: &S, name: &str) -> bool {
    kind_of(store, name) == Some(SessionKind::Anchor)
}

/// The `anchor_started` payloads in the state file at `path`, oldest
/// first, and whether the log holds a `session_imported` event. Only the
/// lines that can hold either are parsed.
fn scan_log(path: &Path) -> Option<(Vec<AnchorStarted>, bool)> {
    let file = std::fs::File::open(path).ok()?;
    let mut started = Vec::new();
    let mut imported = false;
    for line in std::io::BufReader::new(file).lines().skip(1) {
        let Ok(line) = line else { break };
        if line.contains("\"session_imported\"") {
            imported = true;
            continue;
        }
        if !line.contains(ANCHOR_STARTED) {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(|t| t.as_str()) != Some(ANCHOR_STARTED) {
            continue;
        }
        if let Some(payload) = value.get("payload") {
            if let Ok(p) = serde_json::from_value::<AnchorStarted>(payload.clone()) {
                started.push(p);
            }
        }
    }
    Some((started, imported))
}

/// The `anchor_started` payloads `anchor` holds, oldest first.
pub fn started_events<S: SessionBackend + ?Sized>(store: &S, anchor: &str) -> Vec<AnchorStarted> {
    scan_log(&state_path(store, anchor))
        .map(|(s, _)| s)
        .unwrap_or_default()
}

/// The predecessor `anchor`'s own `anchor_started` recorded when it was
/// created, with the reason it was opened.
pub fn recorded_predecessor<S: SessionBackend + ?Sized>(
    store: &S,
    anchor: &str,
) -> Option<(String, Option<String>)> {
    started_events(store, anchor)
        .into_iter()
        .find_map(|s| s.predecessor.map(|p| (p, s.reason)))
}

/// Whether `anchor` was migrated: its log holds a `session_imported`
/// event, so it is a copy carried here from another store.
fn is_migrated<S: SessionBackend + ?Sized>(store: &S, anchor: &str) -> bool {
    scan_log(&state_path(store, anchor))
        .map(|(_, imported)| imported)
        .unwrap_or(false)
}

/// The observed marker an anchor holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedMarker {
    pub v: u32,
    pub key: String,
    pub at: String,
}

/// `anchor`'s observed marker, when it holds one the manifest lists and
/// whose key passes [`valid_token`].
pub fn read_marker<S: ContextStore + ?Sized>(store: &S, anchor: &str) -> Option<ObservedMarker> {
    store.meta(anchor, OBSERVED_KEY)?;
    let bytes = store.get(anchor, OBSERVED_KEY).ok()?;
    if bytes.len() > links::MAX_LINK_BODY_BYTES {
        return None;
    }
    let marker: ObservedMarker = serde_json::from_slice(&bytes).ok()?;
    valid_token(&marker.key).then_some(marker)
}

/// Write `key` as `anchor`'s observed marker, through koto's writer.
fn write_marker<S: ContextStore + SessionBackend>(
    store: &S,
    anchor: &str,
    key: &str,
) -> anyhow::Result<()> {
    let marker = ObservedMarker {
        v: 1,
        key: key.to_string(),
        at: now_iso8601(),
    };
    let content = serde_json::to_vec(&marker)?;
    store.add_with_writer(anchor, OBSERVED_KEY, &content, WRITER_KOTO)?;
    append_to_session_best_effort(
        store,
        anchor,
        &added_event(OBSERVED_KEY, &content, WRITER_KOTO),
    );
    Ok(())
}

/// The record a close session holds under [`CLOSE_RECORD_KEY`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseRecord {
    pub v: u32,
    pub anchor: String,
    pub anchor_session_id: String,
    pub kind: String,
    pub key: String,
    pub at: String,
}

/// What [`close_check`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseCheck {
    /// `<anchor>-closed` is a valid record of an observed close.
    Closed,
    /// No close session exists, or `anchor` isn't an anchor.
    Open,
    /// A close session exists and fails a rule, named here. It reads as
    /// not closed.
    Tainted(String),
}

impl CloseCheck {
    pub fn is_closed(&self) -> bool {
        matches!(self, CloseCheck::Closed)
    }
}

/// The one check that decides whether `anchor` reads closed by an observed
/// close. `<anchor>-closed` counts only when its header has
/// `kind: "anchor-close"`, its name is `<anchor>-closed`, the manifest
/// lists its record, the record names `anchor` with `anchor_session_id`
/// equal to the anchor's header id, and the record's key equals the
/// anchor's observed marker. Anything else is [`CloseCheck::Tainted`].
pub fn close_check<S: ContextStore + SessionBackend + ?Sized>(
    store: &S,
    anchor: &str,
) -> CloseCheck {
    let Some(anchor_header) = local_header(store, anchor) else {
        return CloseCheck::Open;
    };
    if anchor_header.session_kind() != SessionKind::Anchor {
        return CloseCheck::Open;
    }
    let closed = close_name(anchor);
    if crate::discover::validate_workflow_name(&closed).is_err()
        || !state_path(store, &closed).exists()
    {
        return CloseCheck::Open;
    }
    let taint = |why: &str| CloseCheck::Tainted(why.to_string());
    let Some(header) = local_header(store, &closed) else {
        return taint("the close session's header can't be read");
    };
    if header.session_kind() != SessionKind::AnchorClose {
        return taint("the close session's header has no kind \"anchor-close\"");
    }
    if header.workflow != closed {
        return taint("the close session's header names another session");
    }
    if store.meta(&closed, CLOSE_RECORD_KEY).is_none() {
        return taint("the close session's manifest doesn't list its record");
    }
    let record = store
        .get(&closed, CLOSE_RECORD_KEY)
        .ok()
        .filter(|b| b.len() <= links::MAX_LINK_BODY_BYTES)
        .and_then(|b| serde_json::from_slice::<CloseRecord>(&b).ok());
    let Some(record) = record else {
        return taint("the close record can't be read");
    };
    if record.anchor != anchor {
        return taint("the close record names another anchor");
    }
    if anchor_header.session_id.is_empty() || record.anchor_session_id != anchor_header.session_id {
        return taint("the close record's anchor_session_id isn't the anchor's id");
    }
    match read_marker(store, anchor) {
        Some(marker) if marker.key == record.key => CloseCheck::Closed,
        Some(_) => taint("the close record's key isn't the anchor's observed marker"),
        None => taint("the anchor holds no observed marker"),
    }
}

/// The current anchor of the family whose first anchor is `base`: the
/// highest suffix contiguous from `-s2` whose dispatched-by link names the
/// one before it, or `base` itself.
pub fn current_of_family<S: ContextStore + SessionBackend + ?Sized>(
    store: &S,
    base: &str,
) -> String {
    let mut current = base.to_string();
    let mut n = 2;
    loop {
        let next = successor_name(base, n);
        if !is_anchor(store, &next)
            || !store.ctx_exists(&next, &links::link_key(LinkKind::DispatchedBy, &current))
        {
            return current;
        }
        current = next;
        n += 1;
    }
}

/// Every anchor in the local store, by name.
fn list_anchors(base: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| n.starts_with(ANCHOR_PREFIX) && !n.ends_with(CLOSED_SUFFIX))
        .filter(|n| crate::discover::validate_workflow_name(n).is_ok())
        .collect();
    names.sort();
    names
}

// ---------------------------------------------------------------------
// Creating sessions from the embedded templates
// ---------------------------------------------------------------------

/// Compile `template` into `name`'s session directory and create the
/// session with `kind` in its header and `extra` after the initial
/// transition, in one atomic create.
fn create_kind_session<S: SessionBackend + ?Sized>(
    store: &S,
    name: &str,
    template: &str,
    kind: &str,
    execution_dir: Option<PathBuf>,
    extra: Vec<EventPayload>,
) -> Result<StateFileHeader, SessionError> {
    let session_dir = store.create(name).map_err(SessionError::Other)?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".koto-template-")
        .suffix(".md")
        .tempfile_in(&session_dir)
        .map_err(SessionError::Io)?;
    tmp.write_all(template.as_bytes())
        .and_then(|_| tmp.flush())
        .map_err(SessionError::Io)?;
    let (artifact, hash) = crate::cache::compile_cached_into(tmp.path(), &session_dir, true)
        .map_err(SessionError::Other)?;
    drop(tmp);
    let compiled: crate::template::types::CompiledTemplate = std::fs::read_to_string(&artifact)
        .map_err(anyhow::Error::from)
        .and_then(|c| serde_json::from_str(&c).map_err(anyhow::Error::from))
        .map_err(SessionError::Other)?;

    let ts = now_iso8601();
    let mut header: StateFileHeader = serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "workflow": name,
        "template_hash": hash,
        "created_at": ts,
    }))
    .map_err(|e| SessionError::Other(e.into()))?;
    header.session_id = generate_session_id();
    header.template_name = Some(compiled.name.clone()).filter(|n| !n.is_empty());
    header.execution_dir = execution_dir;
    header.kind = Some(kind.to_string());

    let relative = artifact
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut payloads = vec![
        EventPayload::WorkflowInitialized {
            template_path: relative,
            variables: Default::default(),
            spawn_entry: None,
        },
        EventPayload::Transitioned {
            from: None,
            to: compiled.initial_state.clone(),
            condition_type: "auto".to_string(),
            skip_if_matched: None,
            context_assignments: None,
            vars_matched: None,
        },
    ];
    payloads.extend(extra);
    let events = payloads
        .into_iter()
        .enumerate()
        .map(|(i, payload)| Event {
            seq: i as u64 + 1,
            timestamp: ts.clone(),
            event_type: payload.type_name().to_string(),
            payload,
            idempotency_hash: None,
        })
        .collect();
    store.init_state_file(name, header.clone(), events)?;
    Ok(header)
}

// ---------------------------------------------------------------------
// The family lock
// ---------------------------------------------------------------------

/// An exclusive `flock` on a family's lock file, released on drop.
struct FamilyLock {
    _file: std::fs::File,
}

fn lock_family<S: SessionBackend + ?Sized>(store: &S, base: &str) -> anyhow::Result<FamilyLock> {
    let dir = store.create(base)?;
    let path = dir.join(FAMILY_LOCK_FILE);
    crate::engine::atomic_fs::reject_symlink(&path)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("failed to open the anchor lock {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        // SAFETY: the descriptor belongs to `file`, which outlives the call.
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if ret != 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("failed to lock {}", path.display()));
        }
    }
    Ok(FamilyLock { _file: file })
}

// ---------------------------------------------------------------------
// open
// ---------------------------------------------------------------------

/// Where an agent session's start came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Startup,
    Resume,
    Compact,
    Clear,
    Other,
}

impl Source {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "startup" => Source::Startup,
            "resume" => Source::Resume,
            "compact" => Source::Compact,
            "clear" => Source::Clear,
            "other" => Source::Other,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Source::Startup => "startup",
            Source::Resume => "resume",
            Source::Compact => "compact",
            Source::Clear => "clear",
            Source::Other => "other",
        }
    }
}

/// One `koto session anchor open`.
#[derive(Debug, Clone)]
pub struct OpenRequest {
    pub harness_session: String,
    pub source: Source,
    /// The process token, `<host>:<pid>:<epoch-start>`.
    pub process: Option<String>,
    pub dispatched_by: Option<String>,
    pub observed_key: Option<String>,
    pub workspace: Option<PathBuf>,
    /// The harness's transcript path.
    pub transcript_path: Option<String>,
}

/// What an open did to the anchor it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opened {
    New,
    Continued,
    Reopened,
    Successor,
}

impl Opened {
    pub fn as_str(self) -> &'static str {
        match self {
            Opened::New => "new",
            Opened::Continued => "continued",
            Opened::Reopened => "reopened",
            Opened::Successor => "successor",
        }
    }
}

/// The result of an open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenOutcome {
    pub anchor: String,
    pub opened: Opened,
    pub observed: bool,
    /// After a `/clear` that found its predecessor: the predecessor.
    pub previous: Option<String>,
    /// After such a `/clear`: until when the start hook keeps copying the
    /// predecessor's marker.
    pub inherit_until: Option<String>,
}

/// The process start a token carries, `<host>:<pid>:<epoch-start>`, as a
/// time. `None` when its last field isn't whole seconds.
fn token_start(token: &str) -> Option<SystemTime> {
    let (_, secs) = token.rsplit_once(':')?;
    let secs: u64 = secs.parse().ok()?;
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
}

/// The anchor a `/clear` continues from: among the anchors under `base`
/// whose log changed since the process start in `token`, newest first, the
/// first whose `anchor_started` carries the token's digest. A token with no
/// readable start time finds nothing.
pub fn find_clear_predecessor(base: &Path, token: &str) -> Option<String> {
    let start = token_start(token)?;
    let want = digest(token);
    let entries = std::fs::read_dir(base).ok()?;
    let mut recent: Vec<(SystemTime, String)> = Vec::new();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !name.starts_with(ANCHOR_PREFIX) || name.ends_with(CLOSED_SUFFIX) {
            continue;
        }
        if crate::discover::validate_workflow_name(&name).is_err() {
            continue;
        }
        let path = entry.path().join(state_file_name(&name));
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified >= start {
            recent.push((modified, name));
        }
    }
    recent.sort_by(|a, b| b.cmp(a));
    for (_, name) in recent {
        let path = base.join(&name).join(state_file_name(&name));
        if log_carries_process(&path, &want) {
            return Some(name);
        }
    }
    None
}

/// Whether the anchor log at `path` (its header saying `kind: "anchor"`)
/// holds an `anchor_started` whose `process` is `want`.
fn log_carries_process(path: &Path, want: &str) -> bool {
    let mut text = String::new();
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    if file.read_to_string(&mut text).is_err() {
        return false;
    }
    let mut lines = text.lines();
    let Some(header) = lines.next() else {
        return false;
    };
    let is_anchor = serde_json::from_str::<serde_json::Value>(header)
        .ok()
        .and_then(|h| h.get("kind").and_then(|k| k.as_str()).map(str::to_string))
        .as_deref()
        == Some(SessionKind::ANCHOR);
    if !is_anchor {
        return false;
    }
    lines
        .filter(|l| l.contains(ANCHOR_STARTED) && l.contains(want))
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .any(|v| {
            v.get("type").and_then(|t| t.as_str()) == Some(ANCHOR_STARTED)
                && v.pointer("/payload/process").and_then(|p| p.as_str()) == Some(want)
        })
}

/// The predecessor of a resume under a new id, through hints only: the
/// newest anchor whose `anchor_started` recorded the transcript path's
/// digest, or else the current anchor of the family whose harness id is
/// the transcript path's stem.
fn find_resume_predecessor<S: ContextStore + SessionBackend + ?Sized>(
    store: &S,
    harness: &str,
    transcript: &str,
) -> Option<String> {
    let base = base_dir(store);
    let want = digest(transcript);
    let mut matches: Vec<(SystemTime, String)> = list_anchors(&base)
        .into_iter()
        .filter(|name| is_anchor(store, name))
        .filter(|name| {
            started_events(store, name)
                .iter()
                .any(|s| s.transcript.as_deref() == Some(want.as_str()))
        })
        .map(|name| {
            let modified = std::fs::metadata(state_path(store, &name))
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            (modified, name)
        })
        .collect();
    matches.sort_by(|a, b| b.cmp(a));
    if let Some((_, name)) = matches.into_iter().next() {
        return Some(name);
    }
    let stem = Path::new(transcript).file_stem()?.to_str()?;
    if stem == harness || validate_harness_session(stem).is_err() {
        return None;
    }
    let family = anchor_name(stem);
    if !is_anchor(store, &family) {
        return None;
    }
    let recorded = started_events(store, &family)
        .iter()
        .any(|s| s.harness_session == stem);
    recorded.then(|| current_of_family(store, &family))
}

/// Whether a missing first anchor `base` existed before: its close session
/// or a successor is still there, or the start is one that only an
/// existing agent session makes (a compaction, or a resume of the same
/// conversation).
fn existed_before<S: SessionBackend + ?Sized>(store: &S, base: &str, req: &OpenRequest) -> bool {
    if state_path(store, &close_name(base)).exists()
        || state_path(store, &successor_name(base, 2)).exists()
    {
        return true;
    }
    match req.source {
        Source::Compact => true,
        Source::Resume => {
            req.transcript_path
                .as_deref()
                .and_then(|t| Path::new(t).file_stem().and_then(|s| s.to_str()))
                == Some(req.harness_session.as_str())
        }
        _ => false,
    }
}

/// The next free successor name after `current` in `base`'s family.
fn next_successor<S: SessionBackend + ?Sized>(store: &S, base: &str, current: &str) -> String {
    let mut n = if current == base {
        2
    } else {
        successor_suffix(current).map_or(2, |n| n + 1)
    };
    loop {
        let name = successor_name(base, n);
        if !store.session_dir(&name).exists() {
            return name;
        }
        n += 1;
    }
}

/// Open or continue the anchor for `req.harness_session`, under the
/// family's lock. Never reads `KOTO_ANCHOR`.
pub fn open<S: ContextStore + SessionBackend>(
    store: &S,
    req: &OpenRequest,
) -> anyhow::Result<OpenOutcome> {
    validate_harness_session(&req.harness_session)?;
    if let Some(d) = &req.dispatched_by {
        if let Err(e) = links::validate_peer(LinkKind::DispatchedBy, d) {
            return refuse(format!("--dispatched-by: {e}"));
        }
    }
    if let Some(k) = &req.observed_key {
        if !valid_token(k) {
            return refuse(format!(
                "--observed-key '{}' must match ^[A-Za-z0-9_-]{{1,128}}$",
                preview(k)
            ));
        }
    }
    let base = anchor_name(&req.harness_session);
    if let Some(kind) = kind_of(store, &base) {
        if kind != SessionKind::Anchor {
            return refuse(format!(
                "session '{base}' exists and isn't an anchor; koto won't open an anchor under its \
                 name"
            ));
        }
    } else if state_path(store, &base).exists() {
        return refuse(format!(
            "session '{base}' exists and its header can't be read; koto won't open an anchor \
             under its name"
        ));
    }
    if req.dispatched_by.as_deref() == Some(base.as_str()) {
        return refuse(format!(
            "--dispatched-by: anchor '{base}' can't be recorded as dispatched by itself"
        ));
    }

    let _lock = lock_family(store, &base)?;

    let mut started = AnchorStarted {
        source: req.source.as_str().to_string(),
        harness_session: req.harness_session.clone(),
        opened: String::new(),
        process: req.process.as_deref().map(digest),
        workspace: req
            .workspace
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        transcript: req.transcript_path.as_deref().map(digest),
        predecessor: None,
        reason: None,
        dispatched_by: req.dispatched_by.clone(),
    };

    if store.exists(&base) {
        let current = current_of_family(store, &base);
        let reason = if close_check(store, &current).is_closed() {
            Some(REASON_CLOSED)
        } else if is_migrated(store, &current) {
            Some(REASON_MIGRATED)
        } else {
            None
        };
        if let Some(reason) = reason {
            let name = next_successor(store, &base, &current);
            started.opened = Opened::Successor.as_str().to_string();
            started.predecessor = Some(current.clone());
            started.reason = Some(reason.to_string());
            create_anchor(store, &name, req, started, Some(&current), None)?;
            return Ok(OpenOutcome {
                observed: read_marker(store, &name).is_some(),
                anchor: name,
                opened: Opened::Successor,
                previous: None,
                inherit_until: None,
            });
        }
        // A continue: the event records the inherited dispatcher, and no
        // link is written.
        started.opened = Opened::Continued.as_str().to_string();
        store.append_event(
            &current,
            &EventPayload::AnchorStarted(started),
            &now_iso8601(),
        )?;
        if let Some(key) = &req.observed_key {
            if read_marker(store, &current).is_none() {
                write_marker(store, &current, key)?;
            }
        }
        return Ok(OpenOutcome {
            observed: read_marker(store, &current).is_some(),
            anchor: current,
            opened: Opened::Continued,
            previous: None,
            inherit_until: None,
        });
    }

    let predecessor = match (req.source, &req.process, &req.transcript_path) {
        (Source::Clear, Some(token), _) => {
            find_clear_predecessor(&base_dir(store), token).map(|p| (p, REASON_CLEAR))
        }
        (Source::Resume, _, Some(transcript)) => {
            find_resume_predecessor(store, &req.harness_session, transcript)
                .map(|p| (p, REASON_RESUME))
        }
        _ => None,
    };
    if let Some((pred, reason)) = predecessor {
        started.opened = Opened::Successor.as_str().to_string();
        started.predecessor = Some(pred.clone());
        started.reason = Some(reason.to_string());
        // Only a `/clear` copies the predecessor's marker: a resume hint can
        // be wrong, so a resume successor takes a marker only from its own
        // environment or its own watcher.
        let inherited = if reason == REASON_CLEAR {
            read_marker(store, &pred).map(|m| m.key)
        } else {
            None
        };
        create_anchor(store, &base, req, started, Some(&pred), inherited)?;
        let clear = reason == REASON_CLEAR;
        return Ok(OpenOutcome {
            observed: read_marker(store, &base).is_some(),
            anchor: base,
            opened: Opened::Successor,
            previous: clear.then(|| pred.clone()),
            inherit_until: clear.then(|| iso8601_at(SystemTime::now() + INHERIT_WINDOW)),
        });
    }

    let opened = if existed_before(store, &base, req) {
        Opened::Reopened
    } else {
        Opened::New
    };
    started.opened = opened.as_str().to_string();
    create_anchor(store, &base, req, started, None, None)?;
    Ok(OpenOutcome {
        observed: read_marker(store, &base).is_some(),
        anchor: base,
        opened,
        previous: None,
        inherit_until: None,
    })
}

/// Create the anchor `name` with its first `anchor_started`, then its
/// dispatched-by links (to `predecessor` and to the caller's dispatcher)
/// and its observed marker: `--observed-key` first, else `inherited`.
fn create_anchor<S: ContextStore + SessionBackend>(
    store: &S,
    name: &str,
    req: &OpenRequest,
    started: AnchorStarted,
    predecessor: Option<&str>,
    inherited: Option<String>,
) -> anyhow::Result<()> {
    create_kind_session(
        store,
        name,
        ANCHOR_TEMPLATE,
        SessionKind::ANCHOR,
        req.workspace.clone(),
        vec![EventPayload::AnchorStarted(started)],
    )
    .map_err(|e| anyhow::anyhow!("failed to create anchor '{name}': {e}"))?;
    for peer in predecessor.into_iter().chain(req.dispatched_by.as_deref()) {
        if peer == name {
            continue;
        }
        let hints = LinkHints::for_peer(store, peer);
        links::write_link(store, name, LinkKind::DispatchedBy, peer, &hints)?;
    }
    if let Some(key) = req.observed_key.clone().or(inherited) {
        write_marker(store, name, &key)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------
// observe
// ---------------------------------------------------------------------

/// What `observe` writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observe {
    /// A key the watcher read.
    Key(String),
    /// The marker of the predecessor the anchor's own `anchor_started`
    /// recorded.
    Inherit,
}

/// The result of an observe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObserveOutcome {
    /// Whether the anchor holds a marker afterwards.
    pub observed: bool,
    /// Whether this call wrote it.
    pub written: bool,
}

/// Write an observed marker into `anchor`, and only there. An anchor that
/// already holds a marker keeps it, so a repeat changes nothing.
pub fn observe<S: ContextStore + SessionBackend>(
    store: &S,
    anchor: &str,
    what: &Observe,
) -> anyhow::Result<ObserveOutcome> {
    if let Observe::Key(key) = what {
        if !valid_token(key) {
            return refuse(format!(
                "--key '{}' must match ^[A-Za-z0-9_-]{{1,128}}$",
                preview(key)
            ));
        }
    }
    if crate::discover::validate_workflow_name(anchor).is_err() || !is_anchor(store, anchor) {
        return refuse(format!(
            "'{}' isn't an anchor: observe writes only into a session whose header has kind \
             \"anchor\"",
            preview(anchor)
        ));
    }
    let _lock = lock_family(store, family_base(anchor))?;
    if read_marker(store, anchor).is_some() {
        return Ok(ObserveOutcome {
            observed: true,
            written: false,
        });
    }
    let key = match what {
        Observe::Key(key) => Some(key.clone()),
        Observe::Inherit => recorded_predecessor(store, anchor)
            .and_then(|(pred, _)| read_marker(store, &pred))
            .map(|m| m.key),
    };
    match key {
        Some(key) => {
            write_marker(store, anchor, &key)?;
            Ok(ObserveOutcome {
                observed: true,
                written: true,
            })
        }
        None => Ok(ObserveOutcome {
            observed: false,
            written: false,
        }),
    }
}

// ---------------------------------------------------------------------
// close
// ---------------------------------------------------------------------

/// The anchors a close with `key` for `harness` records: the family's first
/// anchor, when its `anchor_started` recorded that harness id, and the
/// successors reached from it through `predecessor` after a `/clear` or a
/// resume under a new id; each only when its observed marker is `key`.
/// The walk doesn't pass an anchor whose marker differs.
pub fn close_targets<S: ContextStore + SessionBackend>(
    store: &S,
    key: &str,
    harness: &str,
) -> Vec<String> {
    let root = anchor_name(harness);
    if !is_anchor(store, &root)
        || !started_events(store, &root)
            .iter()
            .any(|s| s.harness_session == harness)
    {
        return Vec::new();
    }
    let marker_matches = |name: &str| read_marker(store, name).is_some_and(|m| m.key == key);
    if !marker_matches(&root) {
        return Vec::new();
    }

    // Who succeeds whom, through a `/clear` or a resume.
    let mut children: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for name in list_anchors(&base_dir(store)) {
        if !is_anchor(store, &name) {
            continue;
        }
        if let Some((pred, reason)) = recorded_predecessor(store, &name) {
            if matches!(reason.as_deref(), Some(REASON_CLEAR) | Some(REASON_RESUME)) {
                children.entry(pred).or_default().push(name);
            }
        }
    }

    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut queue = vec![root];
    while let Some(name) = queue.pop() {
        if !seen.insert(name.clone()) || !marker_matches(&name) {
            continue;
        }
        if let Some(next) = children.get(&name) {
            queue.extend(next.iter().cloned());
        }
        out.push(name);
    }
    out.sort();
    out
}

/// What `close` did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CloseOutcome {
    /// The anchors this call recorded as closed.
    pub recorded: Vec<String>,
    /// The anchors that already held a valid close session.
    pub already: Vec<String>,
}

/// Remove staging entries under `base` older than [`STAGING_SWEEP_AGE`] at
/// `now`. An entry dated later than `now` has its time set back to `now`,
/// so it is swept once the age has passed rather than never.
pub fn sweep_staging(base: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !name.starts_with(STAGING_PREFIX) {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified > now && !meta.file_type().is_symlink() {
            let _ = std::fs::File::open(&path).and_then(|f| f.set_modified(now));
            continue;
        }
        if now.duration_since(modified).unwrap_or_default() < STAGING_SWEEP_AGE {
            continue;
        }
        let _ = if meta.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
    }
}

/// Record every anchor [`close_targets`] names as closed, each in its own
/// close session. Never writes into an anchor.
pub fn close<S: ContextStore + SessionBackend>(
    store: &S,
    key: &str,
    harness: &str,
) -> anyhow::Result<CloseOutcome> {
    close_with_hook(store, key, harness, &|_| Ok(()))
}

/// [`close`], calling `before_rename` with each staged close session's path
/// just before it is renamed into place. Tests use it to interrupt a close.
pub fn close_with_hook<S: ContextStore + SessionBackend>(
    store: &S,
    key: &str,
    harness: &str,
    before_rename: &dyn Fn(&Path) -> anyhow::Result<()>,
) -> anyhow::Result<CloseOutcome> {
    if !valid_token(key) {
        return refuse(format!(
            "--key '{}' must match ^[A-Za-z0-9_-]{{1,128}}$",
            preview(key)
        ));
    }
    if !valid_token(harness) {
        return refuse(format!(
            "--harness-session '{}' must match ^[A-Za-z0-9_-]{{1,128}}$",
            preview(harness)
        ));
    }
    let base = base_dir(store);
    sweep_staging(&base, SystemTime::now());

    let mut outcome = CloseOutcome::default();
    for anchor in close_targets(store, key, harness) {
        if close_check(store, &anchor).is_closed() {
            outcome.already.push(anchor);
            continue;
        }
        if close_one(store, &base, &anchor, key, before_rename)? {
            outcome.recorded.push(anchor);
        } else {
            outcome.already.push(anchor);
        }
    }
    Ok(outcome)
}

/// Stage `anchor`'s close session and rename it into place. `Ok(false)`
/// when a valid one appeared meanwhile, which another close wrote.
fn close_one<S: ContextStore + SessionBackend>(
    store: &S,
    base: &Path,
    anchor: &str,
    key: &str,
    before_rename: &dyn Fn(&Path) -> anyhow::Result<()>,
) -> anyhow::Result<bool> {
    let closed = close_name(anchor);
    crate::discover::validate_workflow_name(&closed)
        .map_err(|e| anyhow::anyhow!("close session name '{closed}': {e}"))?;
    let anchor_header =
        local_header(store, anchor).with_context(|| format!("anchor '{anchor}' can't be read"))?;

    let staging_root = base.join(format!(
        "{STAGING_PREFIX}{closed}-{}",
        &generate_session_id()[..8]
    ));
    std::fs::create_dir_all(&staging_root)
        .with_context(|| format!("failed to create {}", staging_root.display()))?;
    let staged_store = LocalBackend::with_base_dir(staging_root.clone());
    let staged = staged_store.session_dir(&closed);
    let result = (|| -> anyhow::Result<bool> {
        create_kind_session(
            &staged_store,
            &closed,
            ANCHOR_CLOSE_TEMPLATE,
            SessionKind::ANCHOR_CLOSE,
            anchor_header.execution_dir.clone(),
            Vec::new(),
        )
        .map_err(|e| anyhow::anyhow!("failed to stage the close session of '{anchor}': {e}"))?;
        let hints = LinkHints::for_peer(store, anchor);
        links::write_link(&staged_store, &closed, LinkKind::Closes, anchor, &hints)?;
        let record = CloseRecord {
            v: 1,
            anchor: anchor.to_string(),
            anchor_session_id: anchor_header.session_id.clone(),
            kind: "observed".to_string(),
            key: key.to_string(),
            at: now_iso8601(),
        };
        let content = serde_json::to_vec(&record)?;
        staged_store.add_with_writer(&closed, CLOSE_RECORD_KEY, &content, WRITER_KOTO)?;
        append_to_session_best_effort(
            &staged_store,
            &closed,
            &added_event(CLOSE_RECORD_KEY, &content, WRITER_KOTO),
        );

        before_rename(&staged)?;
        let target = store.session_dir(&closed);
        loop {
            if !staged.join(state_file_name(&closed)).exists() {
                anyhow::bail!(
                    "the staged close session of '{anchor}' at {} is missing; run the close \
                     again",
                    staged.display()
                );
            }
            if target.exists() {
                if close_check(store, anchor).is_closed() {
                    return Ok(false);
                }
                // A stale close session: one rename swaps the fresh one in,
                // and the stale one lands in the staging entry.
                crate::engine::atomic_fs::exchange_dirs(&staged, &target).with_context(|| {
                    format!(
                        "failed to replace the stale close session {}",
                        target.display()
                    )
                })?;
                return Ok(true);
            }
            match crate::engine::atomic_fs::atomic_rename_dir(&staged, &target) {
                Ok(()) => return Ok(true),
                Err(crate::engine::atomic_fs::AtomicCreateError::Collision) => continue,
                Err(crate::engine::atomic_fs::AtomicCreateError::Io(e)) => {
                    return Err(anyhow::Error::new(e).context(format!(
                        "failed to rename the close session of '{anchor}' into place"
                    )))
                }
            }
        }
    })();
    match &result {
        // A failed run leaves its staging entry for a later close to sweep
        // once it is old enough, so a reader never meets a half-written
        // close session and a concurrent close's entry is never touched.
        Err(_) => {}
        Ok(_) => {
            let _ = std::fs::remove_dir_all(&staging_root);
        }
    }
    result
}

// ---------------------------------------------------------------------
// held-by and activity at init and next
// ---------------------------------------------------------------------

/// What [`hold`] did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HoldOutcome {
    /// The anchors `session` now holds held-by links to that it didn't
    /// before.
    pub linked: Vec<String>,
    /// The anchor that got an `anchor_activity` event.
    pub activity: Option<String>,
}

/// The anchor `KOTO_ANCHOR` names, when it is set to a session name.
pub fn anchor_from_env() -> Option<String> {
    let value = std::env::var(ANCHOR_ENV).ok()?;
    let value = value.trim();
    (!value.is_empty() && crate::discover::validate_workflow_name(value).is_ok())
        .then(|| value.to_string())
}

/// Record that the agent session whose anchor is `anchor` opened or ticked
/// `session` (`command` is `init` or `next`): a held-by link in `session`
/// when it has none, and, when `anchor` reads closed by an observed close
/// and its family has a later anchor, a held-by link to that one too. Then
/// an `anchor_activity` event in the live anchor, at most once per tenth
/// of the idle period. A name that isn't an anchor, or is `session`
/// itself, writes nothing.
pub fn hold<S: ContextStore + SessionBackend>(
    store: &S,
    session: &str,
    anchor: &str,
    command: &str,
) -> anyhow::Result<HoldOutcome> {
    let mut outcome = HoldOutcome::default();
    if anchor == session || !is_anchor(store, anchor) {
        return Ok(outcome);
    }
    if kind_of(store, session).is_some_and(|k| k.is_koto_owned()) {
        return Ok(outcome);
    }
    let link = |peer: &str, outcome: &mut HoldOutcome| -> anyhow::Result<()> {
        let hints = LinkHints::for_peer(store, peer);
        if links::write_link(store, session, LinkKind::HeldBy, peer, &hints)?.written() {
            outcome.linked.push(peer.to_string());
        }
        Ok(())
    };
    link(anchor, &mut outcome)?;
    let mut live = anchor.to_string();
    if close_check(store, anchor).is_closed() {
        let current = current_of_family(store, family_base(anchor));
        if current != anchor && current != session {
            link(&current, &mut outcome)?;
            live = current;
        }
    }
    if record_activity(
        store,
        &live,
        session,
        command,
        idle_period(),
        SystemTime::now(),
    )? {
        outcome.activity = Some(live);
    }
    Ok(outcome)
}

/// The time of the last event in the log at `path`, clamped to `now`.
fn last_event_time(path: &Path, now: SystemTime) -> Option<SystemTime> {
    let text = std::fs::read_to_string(path).ok()?;
    let line = text.lines().rev().find(|l| !l.trim().is_empty())?;
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let ts = value.get("timestamp")?.as_str()?;
    let at = crate::engine::wake::parse_rfc3339_millis(ts)?;
    Some(at.min(now))
}

/// Append an `anchor_activity` event to `anchor` unless its last event is
/// less than a tenth of `idle` old. The throttle keeps no state of its own.
pub fn record_activity<S: SessionBackend + ?Sized>(
    store: &S,
    anchor: &str,
    session: &str,
    command: &str,
    idle: Duration,
    now: SystemTime,
) -> anyhow::Result<bool> {
    let gap = idle / 10;
    if let Some(last) = last_event_time(&state_path(store, anchor), now) {
        if now.duration_since(last).unwrap_or_default() < gap {
            return Ok(false);
        }
    }
    store.append_event(
        anchor,
        &EventPayload::AnchorActivity(AnchorActivity {
            session: session.to_string(),
            command: command.to_string(),
        }),
        &iso8601_at(now),
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests;
