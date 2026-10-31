//! The summary a push publishes beside a session's state file in the bucket.
//!
//! After every successful push of its own state, the writing machine PUTs
//! `<home>/<id>/summary.json`: a small JSON object carrying the session's
//! row ([`crate::session::row::derive_row`]), the ETag of the state file it
//! describes and the producing machine. Another machine's dashboard reads it
//! instead of the event log, so showing a remote row never costs the size of
//! the session's history.
//!
//! The summary lives inside the session's own bucket directory, so every
//! release's delete of that directory removes it with the session, and the
//! listing that finds it also returns the state file's ETag beside it.
//! Nothing in it is read from the session's context store (`ctx/`).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::engine::persistence::parse_state_log;
use crate::session::row::derive_row;

/// The summary's file name inside the session's bucket directory.
pub const SUMMARY_FILE: &str = "summary.json";

/// The summary schema this release writes and reads.
pub const SUMMARY_SCHEMA: u32 = 1;

/// The largest summary a reader accepts, by listed size and by body.
pub const MAX_SUMMARY_BYTES: usize = 64 * 1024;

/// Per-field caps, in bytes: name, template name, state and salient
/// variable.
pub const TEXT_FIELD_CAP: usize = 256;
/// The intent's cap, in bytes.
pub const INTENT_CAP: usize = 1024;
/// The machine name's cap, in bytes.
pub const MACHINE_CAP: usize = crate::session::version::MACHINE_NAME_MAX;

/// A session koto keeps for its own bookkeeping, by name: an epoch branch
/// (`~`), an execution anchor (`anchor-` prefix) or an anchor's close
/// session (`-closed` suffix). No summary is written for one, and a reader
/// drops one it meets.
pub fn is_internal_name(name: &str) -> bool {
    name.contains('~') || name.starts_with("anchor-") || name.ends_with("-closed")
}

/// One session another session continues: an import's source or a
/// superseded predecessor. Written empty by this release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummarySource {
    pub relation: String,
    pub prefix: String,
    pub name: String,
    pub session_id: String,
}

/// `summary.json`, schema 1. Parsed into this closed shape: an unknown
/// field or a missing required one makes the summary unreadable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSummary {
    pub schema: u32,
    pub session_id: String,
    pub name: String,
    pub home: String,
    pub kind: Option<String>,
    pub parent: Option<String>,
    pub created_at: String,
    pub template_name: Option<String>,
    pub current_state: Option<String>,
    pub is_terminal: bool,
    pub is_blocked: bool,
    pub failed: bool,
    pub intent: Option<String>,
    pub salient_var: Option<String>,
    pub is_batch_task: bool,
    pub is_request_leg: bool,
    pub last_event_at: Option<String>,
    pub last_seq: u64,
    pub state_etag: String,
    pub machine: Option<String>,
    pub sources: Vec<SummarySource>,
}

/// Cut `s` to at most `cap` bytes on a character boundary.
fn cap(s: String, cap: usize) -> String {
    if s.len() <= cap {
        return s;
    }
    let mut end = cap;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

impl SessionSummary {
    /// The summary of the state log `data` that the bucket now holds under
    /// `state_etag`, for session `id` under workspace prefix `home`.
    ///
    /// Built from the bytes the push holds in memory, so the log is never
    /// read again. `session_dir` is the local copy's directory, which the
    /// row derivation reads the compiled template and the request-leg
    /// pointer from. `None` for an internal session (by name or by the
    /// header's kind) and for a log that doesn't parse.
    pub fn build(
        id: &str,
        home: &str,
        data: &[u8],
        session_dir: &Path,
        state_etag: &str,
    ) -> Option<Self> {
        if is_internal_name(id) {
            return None;
        }
        let (header, events) = parse_state_log(data).ok()?;
        if header.session_kind().is_koto_owned() {
            return None;
        }
        let is_request_leg = crate::engine::leg_pointer::read_pointer(session_dir)
            .ok()
            .flatten()
            .is_some();
        let row = derive_row(&header, &events, session_dir, is_request_leg);
        let text = |s: Option<String>| s.map(|s| cap(s, TEXT_FIELD_CAP));
        Some(Self {
            schema: SUMMARY_SCHEMA,
            session_id: header.session_id.clone(),
            name: id.to_string(),
            home: home.to_string(),
            kind: text(header.kind.clone()),
            parent: text(row.parent),
            created_at: cap(header.created_at.clone(), TEXT_FIELD_CAP),
            template_name: text(row.template_name),
            current_state: text(row.current_state),
            is_terminal: row.is_terminal,
            is_blocked: row.is_blocked,
            failed: row.failed,
            intent: row.intent.map(|s| cap(s, INTENT_CAP)),
            salient_var: text(row.salient_var),
            is_batch_task: row.is_batch_task,
            is_request_leg: row.is_request_leg,
            last_event_at: text(row.last_event_at),
            last_seq: events.last().map(|e| e.seq).unwrap_or(0),
            state_etag: cap(state_etag.to_string(), TEXT_FIELD_CAP),
            machine: row.machine.map(|s| cap(s, MACHINE_CAP)),
            sources: Vec::new(),
        })
    }

    /// The summary's bytes. Every field is capped, so the body stays far
    /// under [`MAX_SUMMARY_BYTES`].
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Parse a summary read from the bucket at `<home>/<name>/summary.json`.
    ///
    /// Refused (`Err` with the reason) when the body is over
    /// [`MAX_SUMMARY_BYTES`], doesn't parse into the closed schema, carries
    /// another schema version, or names a session or workspace other than
    /// the key it was read from.
    pub fn parse(bytes: &[u8], home: &str, name: &str) -> Result<Self, &'static str> {
        if bytes.len() > MAX_SUMMARY_BYTES {
            return Err("oversized");
        }
        let summary: Self = serde_json::from_slice(bytes).map_err(|_| "malformed")?;
        if summary.schema != SUMMARY_SCHEMA {
            return Err("schema");
        }
        if summary.name != name || summary.home != home || summary.session_id.is_empty() {
            return Err("key-mismatch");
        }
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SessionSummary {
        SessionSummary {
            schema: SUMMARY_SCHEMA,
            session_id: "sid".into(),
            name: "review".into(),
            home: "0123456789abcdef".into(),
            kind: None,
            parent: None,
            created_at: "2026-10-31T00:00:00Z".into(),
            template_name: Some("t".into()),
            current_state: Some("wait".into()),
            is_terminal: false,
            is_blocked: true,
            failed: false,
            intent: None,
            salient_var: None,
            is_batch_task: false,
            is_request_leg: false,
            last_event_at: None,
            last_seq: 3,
            state_etag: "e".into(),
            machine: Some("machine-a".into()),
            sources: Vec::new(),
        }
    }

    #[test]
    fn internal_names_are_recognised() {
        assert!(is_internal_name("wf~1"));
        assert!(is_internal_name("anchor-77d6"));
        assert!(is_internal_name("anchor-77d6-closed"));
        assert!(!is_internal_name("review"));
    }

    #[test]
    fn cap_cuts_on_a_character_boundary() {
        assert_eq!(cap("ab\u{e9}".into(), 3), "ab");
        assert_eq!(cap("abc".into(), 3), "abc");
    }

    #[test]
    fn parse_round_trips_and_checks_the_key() {
        let bytes = sample().to_bytes();
        assert_eq!(
            SessionSummary::parse(&bytes, "0123456789abcdef", "review").unwrap(),
            sample()
        );
        assert_eq!(
            SessionSummary::parse(&bytes, "0123456789abcdef", "other"),
            Err("key-mismatch")
        );
        assert_eq!(
            SessionSummary::parse(&bytes, "fedcba9876543210", "review"),
            Err("key-mismatch")
        );
    }

    #[test]
    fn parse_refuses_another_schema_unknown_fields_and_oversize() {
        let mut other = sample();
        other.schema = 2;
        assert_eq!(
            SessionSummary::parse(&other.to_bytes(), "0123456789abcdef", "review"),
            Err("schema")
        );
        let mut value: serde_json::Value = serde_json::from_slice(&sample().to_bytes()).unwrap();
        value["extra"] = serde_json::json!(1);
        assert_eq!(
            SessionSummary::parse(
                &serde_json::to_vec(&value).unwrap(),
                "0123456789abcdef",
                "review"
            ),
            Err("malformed")
        );
        let big = vec![b' '; MAX_SUMMARY_BYTES + 1];
        assert_eq!(
            SessionSummary::parse(&big, "0123456789abcdef", "review"),
            Err("oversized")
        );
    }
}
