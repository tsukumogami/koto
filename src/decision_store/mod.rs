//! The host-level decision store.
//!
//! Decisions outlive the sessions that make them, so they live beside the
//! request store rather than in any session directory:
//!
//! ```text
//! ~/.koto/decisions/          mode 0700, owned by the current user
//!   <YYYY-MM>.jsonl           every decision except reversals, by the UTC
//!                             month of its own time
//!   reversals.jsonl           every `reverse` decision, for all time
//!   imports.jsonl             one line per imported source entry
//!   store.lock                flock target for every write
//! ```
//!
//! The store resolves from `$HOME`, never from `KOTO_SESSIONS_BASE`: that
//! variable moves session directories, and decisions must not move with
//! them. Every file is opened with mode 0600 and `O_NOFOLLOW`, so a symlink
//! planted at any of them makes the open fail instead of being followed.
//!
//! Every write goes through one locked writer ([`Store::append`]): it
//! repairs a torn tail, skips an identical line already present, runs a
//! check closure that can append, skip or refuse, appends and syncs.
//! Readers take no lock, skip a torn final line and any line they can't
//! read, and report how many they skipped.

pub mod authority;
pub mod decider;
pub mod dual_write;
pub mod enforce;
pub mod import;
pub mod lineage;
pub mod reader;
pub mod record;
pub mod report;
pub mod writer;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use thiserror::Error;

pub use record::{
    Authority, Chain, Compensating, DecidedBy, DeciderKind, Decision, FieldError, Kind, Original,
    Slot, Source, Sourced, Subject, DECISION_FORMAT_VERSION,
};
pub use writer::{Check, LockedStore, WriteOutcome};

/// The store's directory under `~/.koto`.
pub const DECISIONS_DIR: &str = "decisions";
/// The all-time reversal log.
pub const REVERSALS_FILE: &str = "reversals.jsonl";
/// The import ledger.
pub const IMPORTS_FILE: &str = "imports.jsonl";
/// The flock target.
pub const LOCK_FILE: &str = "store.lock";

/// The largest decision line koto writes, in bytes, newline excluded.
pub const MAX_LINE_BYTES: usize = 64 * 1024;
/// The most `basis` identifiers one decision may name.
pub const MAX_BASIS: usize = 64;
/// The most evidence pointers one decision may carry.
pub const MAX_EVIDENCE: usize = 64;

/// Every way a store call can fail.
#[derive(Debug, Error)]
pub enum StoreError {
    /// There is no home directory to put the store under.
    #[error("no home directory: the decision store lives under $HOME/.koto/decisions")]
    NoHome,

    /// The store directory exists but isn't safe to use.
    #[error("refusing to use the decision store at {path}: {reason}")]
    UnsafeDir { path: String, reason: String },

    /// A file in the store couldn't be opened, read or written.
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },

    /// The store lock wasn't acquired before the deadline.
    #[error("the decision store lock was not acquired within {waited_ms} ms")]
    LockContention { waited_ms: u64 },

    /// A line with this identifier is already in the store with different
    /// content.
    #[error("decision {id} is already on the record with different content")]
    IdConflict { id: String },

    /// The decision is malformed, oversized or names a field wrongly.
    #[error("{0}")]
    Invalid(FieldError),

    /// This platform has no flock, so the store can't be written safely.
    #[error("the decision store requires flock, which this platform does not provide")]
    Unsupported,
}

impl StoreError {
    pub(crate) fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        StoreError::Io {
            context: context.into(),
            source,
        }
    }

    /// The field a validation error names, when it is one.
    pub fn field(&self) -> Option<&str> {
        match self {
            StoreError::Invalid(e) => Some(&e.field),
            _ => None,
        }
    }
}

impl From<FieldError> for StoreError {
    fn from(e: FieldError) -> Self {
        StoreError::Invalid(e)
    }
}

/// `$HOME/.koto/decisions`, or `None` with no home directory.
pub fn default_root() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".koto").join(DECISIONS_DIR))
}

/// A decision store rooted at one directory.
pub struct Store {
    root: PathBuf,
    redactor: crate::redact::Redactor,
}

impl Store {
    /// The store at `root`, redacting with `redactor` on every write.
    pub fn new(root: impl Into<PathBuf>, redactor: crate::redact::Redactor) -> Self {
        Self {
            root: root.into(),
            redactor,
        }
    }

    /// The store at `$HOME/.koto/decisions`, redacting the host's known
    /// credentials.
    pub fn open_default() -> Result<Self, StoreError> {
        let root = default_root().ok_or(StoreError::NoHome)?;
        Ok(Self::new(root, host_redactor()))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn redactor(&self) -> &crate::redact::Redactor {
        &self.redactor
    }
}

/// The redactor for decision text: the credentials koto knows about from
/// its configuration and from the environment variables that carry them.
pub fn host_redactor() -> crate::redact::Redactor {
    let mut known: Vec<(String, String)> = Vec::new();
    if let Ok(config) = crate::config::resolve::load_config() {
        known.extend(crate::config::redaction_keys(&config));
    }
    let names = crate::engine::command_env::CREDENTIAL_CARRIERS
        .iter()
        .chain(crate::config::resolve::ENV_SECRET_NAMES.iter());
    for name in names {
        if let Ok(value) = std::env::var(name) {
            if !value.trim().is_empty() {
                known.push((name.to_string(), value));
            }
        }
    }
    crate::redact::Redactor::new(known)
}

// ===== Identifiers and time =====

/// Mint a decision identifier for a decision made at `time`
/// (`YYYY-MM-...`): `d-YYYYMM-<32 hex>`, 128 random bits.
pub fn new_id(time: &str) -> Result<String, FieldError> {
    let (y, m) = month_of(time).ok_or_else(|| {
        FieldError::new("time", format!("'{}' is not a UTC time koto writes", time))
    })?;
    let mut buf = [0u8; 16];
    {
        use std::io::Read;
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut buf))
            .map_err(|e| FieldError::new("id", format!("reading random bytes: {}", e)))?;
    }
    Ok(format!("d-{}{}-{}", y, m, hex::encode(buf)))
}

/// The month an identifier names, as `YYYY-MM`, when it is well formed.
pub fn id_month(id: &str) -> Option<String> {
    let rest = id.strip_prefix("d-")?;
    let (ym, random) = rest.split_once('-')?;
    if ym.len() != 6 || !ym.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if random.len() != 32
        || !random
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    let month: u32 = ym[4..].parse().ok()?;
    if !(1..=12).contains(&month) {
        return None;
    }
    Some(format!("{}-{}", &ym[..4], &ym[4..]))
}

/// `(YYYY, MM)` of a time that starts `YYYY-MM-`.
fn month_of(time: &str) -> Option<(&str, &str)> {
    let b = time.as_bytes();
    if b.len() < 8 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let (y, m) = (&time[..4], &time[5..7]);
    if !y.bytes().all(|c| c.is_ascii_digit()) || !m.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let month: u32 = m.parse().ok()?;
    (1..=12).contains(&month).then_some((y, m))
}

/// The `YYYY-MM` month of a time.
pub fn time_month(time: &str) -> Option<String> {
    month_of(time).map(|(y, m)| format!("{}-{}", y, m))
}

/// The current UTC time to the second: `YYYY-MM-DDTHH:MM:SSZ`.
pub fn now_to_second() -> String {
    let full = crate::engine::types::now_iso8601();
    // `YYYY-MM-DDTHH:MM:SS.mmmZ` -> `YYYY-MM-DDTHH:MM:SSZ`
    format!("{}Z", &full[..19])
}

/// Normalize a window bound to `YYYY-MM-DDTHH:MM:SSZ`.
///
/// Accepts `YYYY-MM`, `YYYY-MM-DD`, `YYYY-MM-DDTHH:MM:SSZ` and the same
/// with fractional seconds. Times compare as strings once normalized.
pub fn normalize_time(input: &str) -> Option<String> {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let ranged = |s: &str, lo: u32, hi: u32| s.parse::<u32>().is_ok_and(|v| v >= lo && v <= hi);
    let ymd = |s: &str| -> Option<String> {
        let parts: Vec<&str> = s.split('-').collect();
        match parts.as_slice() {
            [y, m] if y.len() == 4 && digits(y) && m.len() == 2 && ranged(m, 1, 12) => {
                Some(format!("{}-{}-01", y, m))
            }
            [y, m, d]
                if y.len() == 4
                    && digits(y)
                    && m.len() == 2
                    && ranged(m, 1, 12)
                    && d.len() == 2
                    && ranged(d, 1, 31) =>
            {
                Some(format!("{}-{}-{}", y, m, d))
            }
            _ => None,
        }
    };
    match input.split_once('T') {
        None => ymd(input).map(|d| format!("{}T00:00:00Z", d)),
        Some((date, rest)) => {
            let date = ymd(date)?;
            if date.len() != 10 {
                return None;
            }
            let rest = rest.strip_suffix('Z')?;
            let hms = rest.split('.').next()?;
            let p: Vec<&str> = hms.split(':').collect();
            match p.as_slice() {
                [h, mi, s]
                    if h.len() == 2
                        && ranged(h, 0, 23)
                        && mi.len() == 2
                        && ranged(mi, 0, 59)
                        && s.len() == 2
                        && ranged(s, 0, 60) =>
                {
                    Some(format!("{}T{}:{}:{}Z", date, h, mi, s))
                }
                _ => None,
            }
        }
    }
}

// ===== Links and redaction =====

/// Check a link koto will store: an absolute `https` URL with no
/// credentials and no query string. koto stores links and never fetches
/// them.
pub fn validate_link(field: &str, link: &str) -> Result<(), FieldError> {
    let parsed = url::Url::parse(link).map_err(|e| {
        FieldError::new(field, format!("'{}' is not an absolute URL: {}", field, e))
    })?;
    if parsed.scheme() != "https" {
        return Err(FieldError::new(
            field,
            format!("'{}' must be an https URL", field),
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(FieldError::new(
            field,
            format!("'{}' must not carry credentials", field),
        ));
    }
    if parsed.query().is_some() {
        return Err(FieldError::new(
            field,
            format!("'{}' must not carry a query string", field),
        ));
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(FieldError::new(
            field,
            format!("'{}' must name a host", field),
        ));
    }
    Ok(())
}

/// Pass every free-text field a person or agent supplies through the
/// redactor: the reason, links, handles, evidence pointers and the
/// subject's free text.
pub fn redact_decision(d: &mut Decision, redactor: &crate::redact::Redactor) {
    if redactor.is_empty() {
        return;
    }
    let r = |s: &mut String| *s = crate::redact::redact_str(s, redactor).into_string();
    let ro = |s: &mut Option<String>| {
        if let Some(v) = s.as_mut() {
            *v = crate::redact::redact_str(v, redactor).into_string();
        }
    };
    ro(&mut d.reason);
    ro(&mut d.replacement);
    if let Some(c) = d.compensating.as_mut() {
        ro(&mut c.link);
    }
    d.evidence.iter_mut().for_each(r);
    ro(&mut d.decided_by.link);
    d.decided_by.handle.iter_mut().for_each(|h| r(&mut h.value));
    d.decided_by
        .relayed_person
        .iter_mut()
        .for_each(|h| r(&mut h.value));
    // A person named as a dispatcher carries a handle too.
    d.chain
        .links
        .iter_mut()
        .chain(std::iter::once(&mut d.authority.dispatcher))
        .flatten()
        .filter(|v| v.value.starts_with("person:"))
        .for_each(|v| r(&mut v.value));
    redact_subject(&mut d.subject, redactor);
}

fn redact_subject(s: &mut Subject, redactor: &crate::redact::Redactor) {
    for field in [
        &mut s.source_link,
        &mut s.pointer,
        &mut s.what,
        &mut s.question,
        &mut s.refusal_reason,
    ] {
        if let Some(v) = field.as_mut() {
            *v = crate::redact::redact_str(v, redactor).into_string();
        }
    }
    if let Some(inner) = s.attempted.as_mut() {
        redact_subject(inner, redactor);
    }
}
