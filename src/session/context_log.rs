//! Logging context reads and context writers (DESIGN-koto-failure-reporting.md,
//! Decision 4).
//!
//! Every logged read of a context key appends one `context_read` event
//! carrying the key, who read it, the workflow's current state, whether the
//! key was there and, when it was, the SHA-256 of its content. No read ever
//! carries content or size. Every write names its writer, in the log
//! (`context_added.writer`) and in the store's own metadata
//! ([`KeyMeta::writer`](super::context::KeyMeta::writer)).
//!
//! The join a log reader performs: the write that produced a read of key K
//! with hash H is the write of K with the highest `seq` below the read's whose
//! hash equals H -- a `context_added` event's `hash`, or the SHA-256 of a
//! `transitioned` event's assigned string. No match means the writer is
//! unknown.
//!
//! Reads and the writes koto logs on its own behalf are best-effort: they go
//! through [`append_best_effort`], which warns on stderr and never fails the
//! command that performed them.

use std::sync::Mutex;

use crate::cache::sha256_hex;
use crate::engine::types::EventPayload;
use crate::session::context::{ContextStore, KeyMeta};
use crate::session::SessionBackend;

/// `reader` of a read a context gate performed.
pub const READER_GATE: &str = "gate";
/// `reader` of `koto context get` and `koto context exists`.
pub const READER_CLI: &str = "cli";
/// `reader` of a terminal result map's `${context.<key>}` and
/// `failure_reason` reads.
pub const READER_RESULT: &str = "result";
/// `reader` of a decider's context input.
pub const READER_DECIDER: &str = "decider";

/// `writer` of `koto context add` and `koto context remove`.
pub const WRITER_AGENT: &str = "agent";
/// `writer` of a transition's `context_assignments` and the repairs that
/// restore them.
pub const WRITER_TRANSITION: &str = "transition";
/// `writer` of koto's own keys: the batch final view and the published
/// `/workflows` location.
pub const WRITER_KOTO: &str = "koto";
/// `writer` of a key a cloud pull brought down from the remote store.
pub const WRITER_SYNC: &str = "sync";

/// `access` of a read that saw the content.
pub const ACCESS_CONTENT: &str = "content";
/// `access` of a read that only asked whether the key exists.
pub const ACCESS_PRESENCE: &str = "presence";

/// One read of a context key, before it is given a reader and a state.
///
/// Built only for keys that pass the key grammar; a read of anything else is
/// not a read of a key and logs nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextReadRecord {
    pub key: String,
    pub present: bool,
    /// Lowercase hex SHA-256 of the content, exactly when `present`.
    pub hash: Option<String>,
    /// Whether the read only asked whether the key exists.
    pub presence_only: bool,
}

impl ContextReadRecord {
    /// A read that fetched the key's content: `content` is what the store
    /// returned, `None` when the key wasn't there. `None` for a key that
    /// fails the key grammar.
    pub fn content(key: &str, content: Option<&[u8]>) -> Option<Self> {
        if !loggable_key(key) {
            return None;
        }
        Some(ContextReadRecord {
            key: key.to_string(),
            present: content.is_some(),
            hash: content.map(sha256_hex),
            presence_only: false,
        })
    }

    /// A read that asked only whether the key exists. The hash comes from
    /// the store's metadata, or from the content when there is none. `None`
    /// for a key that fails the key grammar, and for a present key with no
    /// hash to be had.
    pub fn presence(
        store: &dyn ContextStore,
        session: &str,
        key: &str,
        present: bool,
    ) -> Option<Self> {
        if !loggable_key(key) {
            return None;
        }
        let hash = if present {
            match store.meta(session, key) {
                Some(meta) => Some(meta.hash),
                None => store.get(session, key).ok().map(|c| sha256_hex(&c)),
            }
        } else {
            None
        };
        // A key the store says is there but that has neither metadata nor
        // readable content has no hash to record, and `hash` is present
        // exactly when `present` is: that read logs nothing rather than an
        // event breaking the rule.
        if present && hash.is_none() {
            return None;
        }
        Some(ContextReadRecord {
            key: key.to_string(),
            present,
            hash,
            presence_only: true,
        })
    }

    /// The `context_read` event for this read by `reader` in `state`,
    /// naming `gate` when a gate performed it.
    pub fn into_event(self, reader: &str, state: &str, gate: Option<&str>) -> EventPayload {
        EventPayload::ContextRead {
            key: self.key,
            reader: reader.to_string(),
            state: state.to_string(),
            present: self.present,
            hash: if self.present { self.hash } else { None },
            access: Some(
                if self.presence_only {
                    ACCESS_PRESENCE
                } else {
                    ACCESS_CONTENT
                }
                .to_string(),
            ),
            gate: gate.map(str::to_string),
        }
    }
}

/// Whether reads of `key` are logged: only keys that pass the key grammar.
pub fn loggable_key(key: &str) -> bool {
    crate::session::validate::validate_context_key(key).is_ok()
}

/// The `context_added` event for a write of `content` to `key` by `writer`.
pub fn added_event(key: &str, content: &[u8], writer: &str) -> EventPayload {
    EventPayload::ContextAdded {
        key: key.to_string(),
        hash: sha256_hex(content),
        size: content.len() as u64,
        writer: Some(writer.to_string()),
    }
}

/// The `context_added` event for a write the store already describes in
/// `meta`, recorded as `writer`.
pub fn added_event_from_meta(key: &str, meta: &KeyMeta, writer: &str) -> EventPayload {
    EventPayload::ContextAdded {
        key: key.to_string(),
        hash: meta.hash.clone(),
        size: meta.size,
        writer: Some(writer.to_string()),
    }
}

#[cfg(test)]
thread_local! {
    static FAIL_BEST_EFFORT_APPENDS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Test hook: while the returned guard lives, every append made through
/// [`append_best_effort`] on this thread fails without writing, as a full
/// disk or an unwritable log would make it.
#[cfg(test)]
pub fn fail_best_effort_appends() -> FailAppendsGuard {
    FAIL_BEST_EFFORT_APPENDS.with(|f| f.set(true));
    FailAppendsGuard
}

/// Restores best-effort appends when dropped.
#[cfg(test)]
pub struct FailAppendsGuard;

#[cfg(test)]
impl Drop for FailAppendsGuard {
    fn drop(&mut self) {
        FAIL_BEST_EFFORT_APPENDS.with(|f| f.set(false));
    }
}

/// Append `payload` through `append`, warning on stderr instead of failing
/// when the append does not land. Returns whether it landed.
///
/// Used for every `context_read` and for the writes koto logs on its own
/// behalf (the batch final view, the published `/workflows` location, a
/// cloud pull): a command whose job was something else must not fail because
/// the record of a read or of a side write could not be kept.
pub fn append_best_effort<E: std::fmt::Display>(
    payload: &EventPayload,
    append: impl FnOnce(&EventPayload) -> Result<(), E>,
) -> bool {
    #[cfg(test)]
    if FAIL_BEST_EFFORT_APPENDS.with(|f| f.get()) {
        warn_unrecorded(payload, "test hook: append refused");
        return false;
    }
    match append(payload) {
        Ok(()) => true,
        Err(e) => {
            warn_unrecorded(payload, &e.to_string());
            false
        }
    }
}

fn warn_unrecorded(payload: &EventPayload, error: &str) {
    let key = match payload {
        EventPayload::ContextRead { key, .. } | EventPayload::ContextAdded { key, .. } => {
            key.as_str()
        }
        _ => "",
    };
    eprintln!(
        "warning: failed to record {} for context key {:?}: {}",
        payload.type_name(),
        key,
        error
    );
}

/// [`append_best_effort`] to `session`'s log through `backend`.
pub fn append_to_session_best_effort(
    backend: &dyn SessionBackend,
    session: &str,
    payload: &EventPayload,
) -> bool {
    append_best_effort(payload, |p| {
        backend.append_event(session, p, &crate::engine::types::now_iso8601())
    })
}

/// Append each read in `reads` to `session`'s log as a `context_read` by
/// `reader` in `state`, best-effort.
pub fn append_reads(
    backend: &dyn SessionBackend,
    session: &str,
    reads: Vec<ContextReadRecord>,
    reader: &str,
    state: &str,
) {
    for read in reads {
        append_to_session_best_effort(backend, session, &read.into_event(reader, state, None));
    }
}

/// A [`ContextStore`] that records every content read made through it, for
/// callers that hand a store to code that doesn't know reads are logged
/// (a terminal result map's resolution). Writes and presence checks pass
/// straight through and record nothing.
pub struct RecordingStore<'a> {
    inner: &'a dyn ContextStore,
    reads: Mutex<Vec<ContextReadRecord>>,
}

impl<'a> RecordingStore<'a> {
    pub fn new(inner: &'a dyn ContextStore) -> Self {
        RecordingStore {
            inner,
            reads: Mutex::new(Vec::new()),
        }
    }

    /// The reads made so far, in order.
    pub fn into_reads(self) -> Vec<ContextReadRecord> {
        self.reads.into_inner().unwrap_or_default()
    }
}

impl ContextStore for RecordingStore<'_> {
    fn add(&self, session: &str, key: &str, content: &[u8]) -> anyhow::Result<()> {
        self.inner.add(session, key, content)
    }

    fn add_with_writer(
        &self,
        session: &str,
        key: &str,
        content: &[u8],
        writer: &str,
    ) -> anyhow::Result<()> {
        self.inner.add_with_writer(session, key, content, writer)
    }

    fn get(&self, session: &str, key: &str) -> anyhow::Result<Vec<u8>> {
        let result = self.inner.get(session, key);
        if let Some(read) = ContextReadRecord::content(key, result.as_deref().ok()) {
            if let Ok(mut reads) = self.reads.lock() {
                reads.push(read);
            }
        }
        result
    }

    fn ctx_exists(&self, session: &str, key: &str) -> bool {
        self.inner.ctx_exists(session, key)
    }

    fn remove(&self, session: &str, key: &str) -> anyhow::Result<()> {
        self.inner.remove(session, key)
    }

    fn list_keys(&self, session: &str, prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
        self.inner.list_keys(session, prefix)
    }

    fn meta(&self, session: &str, key: &str) -> Option<KeyMeta> {
        self.inner.meta(session, key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_read_hashes_the_content_and_an_absent_key_has_no_hash() {
        let r = ContextReadRecord::content("k", Some(b"hello")).unwrap();
        assert_eq!(
            r.hash.as_deref(),
            Some("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824")
        );
        let e = r.into_event(READER_CLI, "s", None);
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["access"], "content");
        assert_eq!(json["present"], true);
        assert!(json.get("gate").is_none());

        let r = ContextReadRecord::content("k", None).unwrap();
        let json = serde_json::to_value(r.into_event(READER_CLI, "s", None)).unwrap();
        assert_eq!(json["present"], false);
        assert!(json.get("hash").is_none());
    }

    #[test]
    fn keys_failing_the_grammar_log_nothing() {
        assert!(ContextReadRecord::content("../etc", Some(b"x")).is_none());
        assert!(ContextReadRecord::content("has space", None).is_none());
        assert!(ContextReadRecord::content("", None).is_none());
    }

    #[test]
    fn best_effort_append_warns_and_reports_failure() {
        let p = ContextReadRecord::content("k", None)
            .unwrap()
            .into_event(READER_CLI, "s", None);
        assert!(!append_best_effort(&p, |_| Err::<(), _>("disk full")));
        assert!(append_best_effort(&p, |_| Ok::<(), String>(())));
        let _guard = fail_best_effort_appends();
        let mut called = false;
        assert!(!append_best_effort(&p, |_| {
            called = true;
            Ok::<(), String>(())
        }));
        assert!(!called, "the hook refuses before the append runs");
    }
}
