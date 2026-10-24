//! Pins: a record in a session that keeps it from every removal by rule
//! (DESIGN-session-pin.md).
//!
//! A pin is the one peerless reserved key [`PIN_KEY`], `koto/links/pin`, in
//! the pinned session, holding one JSON object ([`Pin`]) of at most
//! [`MAX_PIN_BYTES`]. It names no peer, so it is never an edge: the
//! retention engine reads it as a per-session fact ([`PinFact`]) beside the
//! links, and it keeps exactly its own session. A koto build from before
//! pins reads the key as one that isn't a link key, a problem that keeps
//! the session's hierarchy, so an older build keeps a pinned session too.
//!
//! The release record, [`RELEASE_KEY`] (`koto/pin-release`), sits outside
//! `koto/links/` so no build reads it as a link. An unpin writes it before
//! it removes the pin, so an unpin interrupted between the two leaves the
//! pin in force.
//!
//! Both records are standalone: [`pin`] and [`unpin`] write them under the
//! session's lock through [`crate::session::links::write_reserved_record`],
//! which logs no event and moves no clock. They are the one named exception
//! to session retention's one-writer rule (koto#445): they write into the
//! named session, whoever it belongs to, and only these two records.
//!
//! This build supports pins on the local backend. Under the cloud backend
//! both verbs refuse, since a pin that reached only one host's copy would
//! be believed there and missed everywhere else.

use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::engine::types::SessionKind;
use crate::session::{state_file_name, Backend, SessionBackend, SessionError, SessionLock};

/// The pin's key in the pinned session's context.
pub const PIN_KEY: &str = "koto/links/pin";

/// The latest release record's key.
pub const RELEASE_KEY: &str = "koto/pin-release";

/// The largest pin or release record read; a larger one is unreadable.
pub const MAX_PIN_BYTES: usize = 4 * 1024;

/// The most Unicode scalar values a reason may hold.
pub const MAX_REASON: usize = 500;

/// The `v` a pin and a release record carry today.
pub const RECORD_VERSION: u32 = 1;

/// The authority a caller under an anchor needs to unpin.
pub const RELEASE_PIN: &str = "release-pin";

/// How long [`pin`] and [`unpin`] wait for the session's lock.
const LOCK_WAIT: Duration = Duration::from_secs(10);

/// A readable pin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    pub v: u32,
    /// Random, set when the pin is written; a release record copies it, so
    /// a pin set, released and set again within one second reads apart.
    pub id: String,
    pub reason: String,
    /// The anchor of the agent session that set it, or `None` by hand.
    pub set_by: Option<String>,
    /// When it was set, RFC 3339 UTC.
    pub set_at: String,
}

/// Why a pin that is there can't be read. Each keeps its session, like a
/// readable pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinProblem {
    /// Its content isn't a pin this koto knows.
    Unparseable,
    /// Its content is larger than [`MAX_PIN_BYTES`].
    TooLarge,
    /// A symlink, a directory or a special file sits at its path.
    NotAFile,
    /// The index lists it and no content is there.
    MissingContent,
}

impl PinProblem {
    pub fn as_str(self) -> &'static str {
        match self {
            PinProblem::Unparseable => "unparseable",
            PinProblem::TooLarge => "too_large",
            PinProblem::NotAFile => "not_a_file",
            PinProblem::MissingContent => "missing_content",
        }
    }
}

/// What one session's pin key holds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PinFact {
    /// No pin.
    #[default]
    None,
    Present(Pin),
    Unreadable(PinProblem),
}

impl PinFact {
    /// Whether the session is pinned, readably or not.
    pub fn is_pinned(&self) -> bool {
        !matches!(self, PinFact::None)
    }
}

/// The release record an unpin writes, replacing the one before.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseRecord {
    pub v: u32,
    /// The releaser's anchor, or `None` by hand.
    pub released_by: Option<String>,
    /// The authority the release used, or `None` by hand.
    pub authority: Option<String>,
    pub reason: Option<String>,
    pub released_at: String,
    /// The pin it released; each field `None` when that pin was unreadable.
    pub pin: ReleasedPin,
}

/// The released pin, as a release record keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleasedPin {
    pub id: Option<String>,
    pub reason: Option<String>,
    pub set_by: Option<String>,
    pub set_at: Option<String>,
}

/// Check a reason: not empty after trimming whitespace, at most
/// [`MAX_REASON`] Unicode scalar values, and no character in Unicode
/// category Cc (tab, newline and the other C0 and C1 controls).
pub fn validate_reason(reason: &str) -> Result<(), String> {
    if reason.trim().is_empty() {
        return Err("the reason is empty".to_string());
    }
    let count = reason.chars().count();
    if count > MAX_REASON {
        return Err(format!(
            "the reason is {count} characters long; at most {MAX_REASON} are allowed"
        ));
    }
    if let Some(c) = reason.chars().find(|c| c.is_control()) {
        return Err(format!(
            "the reason holds the control character U+{:04X}; tabs, newlines and other \
             control characters aren't allowed",
            c as u32
        ));
    }
    Ok(())
}

/// Parse a pin's content, or `None` when it isn't a pin this koto knows.
fn parse_pin(bytes: &[u8]) -> Option<Pin> {
    serde_json::from_slice::<Pin>(bytes)
        .ok()
        .filter(|p| p.v == RECORD_VERSION && !p.id.is_empty())
}

/// Read at most one byte more than [`MAX_PIN_BYTES`] of the file at `path`.
fn read_bounded(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    fs::File::open(path)?
        .take(MAX_PIN_BYTES as u64 + 1)
        .read_to_end(&mut buf)?;
    Ok(buf)
}

/// The pin of the session whose directory is `session_dir`, read from disk
/// with `lstat` first, so a symlink is never followed. `listed` says
/// whether the session's context index lists the pin key or any key under
/// it: a listed pin with nothing at its path is `missing_content`, while a
/// non-empty pin file the index doesn't list is a readable pin. An empty
/// unlisted file is no pin (the store's own lock files are empty).
pub fn read_at(session_dir: &Path, listed: bool) -> PinFact {
    let path = session_dir.join("ctx").join(PIN_KEY);
    let meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return if listed {
                PinFact::Unreadable(PinProblem::MissingContent)
            } else {
                PinFact::None
            };
        }
        Err(_) => return PinFact::Unreadable(PinProblem::NotAFile),
    };
    if !meta.file_type().is_file() {
        return PinFact::Unreadable(PinProblem::NotAFile);
    }
    if meta.len() > MAX_PIN_BYTES as u64 {
        return PinFact::Unreadable(PinProblem::TooLarge);
    }
    let bytes = match read_bounded(&path) {
        Ok(bytes) => bytes,
        Err(_) => return PinFact::Unreadable(PinProblem::Unparseable),
    };
    if bytes.len() > MAX_PIN_BYTES {
        return PinFact::Unreadable(PinProblem::TooLarge);
    }
    if bytes.is_empty() && !listed {
        return PinFact::None;
    }
    match parse_pin(&bytes) {
        Some(pin) => PinFact::Present(pin),
        None => PinFact::Unreadable(PinProblem::Unparseable),
    }
}

/// Whether a manifest key is the pin key or lies under it.
pub fn is_pin_key(key: &str) -> bool {
    key == PIN_KEY
        || key
            .strip_prefix(PIN_KEY)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// The pin of session `name` under `base`, read from disk now. The index
/// is read for the cross-check of [`read_at`]; an index that can't be read
/// counts as listing the pin, so a missing pin file then reads as
/// `missing_content` and keeps the session.
pub fn read_local(base: &Path, name: &str) -> PinFact {
    let dir = base.join(name);
    let manifest = dir.join("ctx").join("manifest.json");
    let listed = match fs::symlink_metadata(&manifest) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Ok(meta) if meta.file_type().is_file() => fs::read(&manifest)
            .ok()
            .and_then(|b| serde_json::from_slice::<crate::session::context::Manifest>(&b).ok())
            .is_none_or(|m| m.keys.keys().any(|k| is_pin_key(k))),
        _ => true,
    };
    read_at(&dir, listed)
}

/// The latest release record of session `name` under `base`, when one is
/// there and readable.
pub fn read_release(base: &Path, name: &str) -> Option<ReleaseRecord> {
    let path = base.join(name).join("ctx").join(RELEASE_KEY);
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_file() => {}
        _ => return None,
    }
    read_bounded(&path)
        .ok()
        .filter(|b| b.len() <= MAX_PIN_BYTES)
        .and_then(|b| serde_json::from_slice::<ReleaseRecord>(&b).ok())
        .filter(|r| r.v == RECORD_VERSION)
}

/// Who is calling, as `KOTO_ANCHOR` says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// No `KOTO_ANCHOR`, or an empty one: a call by hand.
    ByHand,
    /// `KOTO_ANCHOR` names this, as set (trimmed), whatever it names.
    Anchor(String),
}

/// The caller of this process.
pub fn caller() -> Caller {
    match std::env::var(crate::session::anchor::ANCHOR_ENV) {
        Ok(v) if !v.trim().is_empty() => Caller::Anchor(v.trim().to_string()),
        _ => Caller::ByHand,
    }
}

/// A refusal or failure of [`pin`] or [`unpin`], as the CLI prints it.
#[derive(Debug, Clone)]
pub struct PinError {
    /// The typed error code.
    pub code: &'static str,
    /// The exit code: 2 for a refusal, 3 for a store that can't be used.
    pub exit: i32,
    pub message: String,
    /// Fields the error carries beside `code` and `message`.
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl PinError {
    fn refusal(code: &'static str, message: impl Into<String>) -> Self {
        PinError {
            code,
            exit: 2,
            message: message.into(),
            extra: serde_json::Map::new(),
        }
    }

    fn failure(code: &'static str, message: impl Into<String>) -> Self {
        PinError {
            code,
            exit: 3,
            message: message.into(),
            extra: serde_json::Map::new(),
        }
    }

    fn with(mut self, field: &str, value: serde_json::Value) -> Self {
        self.extra.insert(field.to_string(), value);
        self
    }

    /// The `{"error": {...}}` object the CLI prints.
    pub fn to_json(&self) -> serde_json::Value {
        let mut error = serde_json::Map::new();
        error.insert("code".to_string(), serde_json::json!(self.code));
        error.insert("message".to_string(), serde_json::json!(self.message));
        for (k, v) in &self.extra {
            error.insert(k.clone(), v.clone());
        }
        serde_json::json!({ "error": error })
    }
}

/// What [`pin`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinOutcome {
    /// The session had no pin; this one is now recorded.
    Set(Pin),
    /// The session already held this readable pin; nothing changed.
    Existing(Pin),
}

impl PinOutcome {
    pub fn pin(&self) -> &Pin {
        match self {
            PinOutcome::Set(p) | PinOutcome::Existing(p) => p,
        }
    }
}

/// The local store a pin is written to, or the refusal for a backend this
/// build can't pin on.
fn local_store(backend: &Backend) -> Result<&crate::session::local::LocalBackend, PinError> {
    match backend {
        Backend::Local(local) => Ok(local),
        Backend::Cloud(_) => Err(PinError::failure(
            "unsupported_backend",
            "pins aren't supported under the cloud backend in this koto; nothing was written",
        )),
    }
}

/// The checks pin and unpin share before they lock: a session name, a
/// session in the store, not a migrated source.
fn check_session(backend: &Backend, name: &str) -> Result<(), PinError> {
    if let Err(e) = crate::session::validate::validate_session_id(name) {
        return Err(PinError::refusal("no_session", format!("{e:#}")));
    }
    if !backend.exists(name) {
        return Err(PinError::refusal(
            "no_session",
            format!("session '{name}' does not exist"),
        ));
    }
    if let Err(e) = backend.check_not_migrated(name) {
        return Err(PinError::refusal(
            "not_pinnable",
            format!("{e:#}; its moved copy holds what it held"),
        ));
    }
    Ok(())
}

/// Take session `name`'s lock, waiting up to [`LOCK_WAIT`] for a koto
/// command that holds it.
fn lock(backend: &Backend, name: &str) -> Result<SessionLock, PinError> {
    let started = Instant::now();
    loop {
        match backend.lock_state_file(name) {
            Ok(lock) => return Ok(lock),
            Err(SessionError::Locked { .. }) if started.elapsed() < LOCK_WAIT => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(SessionError::Locked { .. }) => {
                return Err(PinError::failure(
                    "locked",
                    format!(
                        "another koto process held session '{name}''s lock for {}s; nothing \
                         was written, run the command again",
                        LOCK_WAIT.as_secs()
                    ),
                ))
            }
            Err(SessionError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(PinError::refusal(
                    "no_session",
                    format!("session '{name}' does not exist"),
                ))
            }
            Err(e) => {
                return Err(PinError::failure(
                    "session_unreadable",
                    format!("session '{name}' can't be locked: {e}"),
                ))
            }
        }
    }
}

/// The kind session `name`'s header records.
fn kind_of(base: &Path, name: &str) -> Result<SessionKind, PinError> {
    crate::engine::persistence::read_header(&base.join(name).join(state_file_name(name)))
        .map(|h| h.session_kind())
        .map_err(|e| {
            PinError::failure(
                "session_unreadable",
                format!("session '{name}''s header can't be read: {e:#}"),
            )
        })
}

/// The anchor a pin set now records as its setter: `KOTO_ANCHOR`, when it
/// names an anchor in the local store; `None` otherwise, as by hand.
fn setter(base: &Path, caller: &Caller) -> Option<String> {
    let Caller::Anchor(name) = caller else {
        return None;
    };
    crate::discover::validate_workflow_name(name).ok()?;
    let path = base.join(name).join(state_file_name(name));
    crate::engine::persistence::read_header(&path)
        .ok()
        .filter(|h| h.session_kind() == SessionKind::Anchor)
        .map(|_| name.clone())
}

/// A new pin id: 128 random bits, as 32 lowercase hex digits.
fn new_id() -> String {
    let mut buf = [0u8; 16];
    let read = fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf));
    if read.is_err() {
        // No urandom: fall back to the clock and the process, which still
        // tells apart pins set at different instants on one host.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        buf = (nanos ^ (u128::from(std::process::id()) << 96)).to_be_bytes();
    }
    hex::encode(buf)
}

/// `koto session pin <name> --reason <reason>`: record a pin in session
/// `name`, under its lock, with no log event. A session that already holds
/// a readable pin keeps it unchanged ([`PinOutcome::Existing`]); one whose
/// pin can't be read is refused, since an unpin clears it.
pub fn pin(backend: &Backend, name: &str, reason: &str) -> Result<PinOutcome, PinError> {
    validate_reason(reason).map_err(|why| PinError::refusal("invalid_reason", why))?;
    let local = local_store(backend)?;
    check_session(backend, name)?;
    let base = local.base_dir();
    let _lock = lock(backend, name)?;
    match kind_of(base, name)? {
        SessionKind::Ordinary => {}
        SessionKind::Anchor | SessionKind::AnchorClose => {
            return Err(PinError::refusal(
                "not_pinnable",
                format!(
                    "session '{name}' is an agent session's anchor or its close record, which \
                     can't be pinned"
                ),
            ))
        }
    }
    match read_local(base, name) {
        PinFact::Present(existing) => return Ok(PinOutcome::Existing(existing)),
        PinFact::Unreadable(problem) => {
            return Err(PinError::refusal(
                "pin_unreadable",
                format!(
                    "session '{name}' holds a pin that can't be read ({}); unpin it to clear \
                     it, then pin it again",
                    problem.as_str()
                ),
            )
            .with("pin_unreadable", serde_json::json!(problem.as_str())))
        }
        PinFact::None => {}
    }
    let pin = Pin {
        v: RECORD_VERSION,
        id: new_id(),
        reason: reason.to_string(),
        set_by: setter(base, &caller()),
        set_at: crate::engine::types::now_iso8601(),
    };
    let bytes = serde_json::to_vec(&pin).expect("a pin always serializes");
    crate::session::links::write_reserved_record(local, name, PIN_KEY, &bytes).map_err(|e| {
        PinError::failure(
            "pin_not_written",
            format!("the pin wasn't written into '{name}': {e:#}"),
        )
    })?;
    Ok(PinOutcome::Set(pin))
}

/// `koto session unpin <name> [--reason <reason>]`: release session
/// `name`'s pin, readable or not, under its lock and with no log event.
/// The release record is written first and the pin removed second. A
/// session with no pin changes nothing (`Ok(None)`), whoever the caller is.
///
/// A caller by hand (no `KOTO_ANCHOR`) may release. A caller under an
/// anchor needs the `release-pin` authority its agent session's start
/// recorded in the anchor; this build records none, so it refuses every
/// such caller with `authority_required` and writes nothing.
pub fn unpin(
    backend: &Backend,
    name: &str,
    reason: Option<&str>,
) -> Result<Option<ReleaseRecord>, PinError> {
    if let Some(reason) = reason {
        validate_reason(reason).map_err(|why| PinError::refusal("invalid_reason", why))?;
    }
    let local = local_store(backend)?;
    check_session(backend, name)?;
    let base = local.base_dir();
    let _lock = lock(backend, name)?;
    let released = match read_local(base, name) {
        PinFact::None => return Ok(None),
        PinFact::Present(pin) => ReleasedPin {
            id: Some(pin.id),
            reason: Some(pin.reason),
            set_by: pin.set_by,
            set_at: Some(pin.set_at),
        },
        PinFact::Unreadable(_) => ReleasedPin {
            id: None,
            reason: None,
            set_by: None,
            set_at: None,
        },
    };
    if let Caller::Anchor(anchor) = caller() {
        let shown = crate::session::hierarchy::display_text(&anchor);
        return Err(PinError::refusal(
            "authority_required",
            format!(
                "releasing the pin on '{name}' needs the {RELEASE_PIN} authority: KOTO_ANCHOR \
                 names anchor '{shown}', and the authority it was handed couldn't be read from \
                 it. Authority is handed to an agent session at its start, through \
                 KOTO_AUTHORITY; nothing was written"
            ),
        )
        .with("authority", serde_json::json!(RELEASE_PIN))
        .with("anchor", serde_json::json!(shown)));
    }
    let record = ReleaseRecord {
        v: RECORD_VERSION,
        released_by: None,
        authority: None,
        reason: reason.map(str::to_string),
        released_at: crate::engine::types::now_iso8601(),
        pin: released,
    };
    let bytes = serde_json::to_vec(&record).expect("a release record always serializes");
    crate::session::links::write_reserved_record(local, name, RELEASE_KEY, &bytes).map_err(
        |e| {
            PinError::failure(
                "pin_not_released",
                format!("the release record wasn't written into '{name}', so the pin stays: {e:#}"),
            )
        },
    )?;
    crate::session::links::remove_reserved_record(local, name, PIN_KEY).map_err(|e| {
        PinError::failure(
            "pin_not_released",
            format!("the pin wasn't removed from '{name}' and stays in force: {e:#}"),
        )
    })?;
    Ok(Some(record))
}

#[cfg(test)]
mod tests;
