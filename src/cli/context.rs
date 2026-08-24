use std::fs;
use std::io::Read;
use std::path::PathBuf;

use anyhow::Result;

use crate::cache::sha256_hex;
use crate::engine::types::{now_iso8601, EventPayload};
use crate::session::context::ContextStore;
use crate::session::SessionBackend;

/// Read content from stdin and store it under the given key, then emit a
/// `context_added` event to the session log.
///
/// When `from_file` is provided, reads from that path instead of stdin.
pub fn handle_add(
    store: &dyn ContextStore,
    backend: &dyn SessionBackend,
    session: &str,
    key: &str,
    from_file: Option<&str>,
) -> Result<()> {
    let content = match from_file {
        Some(path) => {
            fs::read(path).map_err(|e| anyhow::anyhow!("failed to read file '{}': {}", path, e))?
        }
        None => {
            let mut buf = Vec::new();
            std::io::stdin()
                .read_to_end(&mut buf)
                .map_err(|e| anyhow::anyhow!("failed to read stdin: {}", e))?;
            buf
        }
    };

    store.add(session, key, &content)?;

    let hash = sha256_hex(&content);
    let size = content.len() as u64;
    let event = EventPayload::ContextAdded {
        key: key.to_string(),
        hash,
        size,
    };
    backend.append_event(session, &event, &now_iso8601())?;

    Ok(())
}

/// Retrieve stored content and write it to stdout.
///
/// When `to_file` is provided, writes to that path instead of stdout.
pub fn handle_get(
    store: &dyn ContextStore,
    session: &str,
    key: &str,
    to_file: Option<&str>,
) -> Result<()> {
    let content = store.get(session, key)?;

    match to_file {
        Some(path) => {
            if let Some(parent) = PathBuf::from(path).parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent).map_err(|e| {
                        anyhow::anyhow!("failed to create parent directory for '{}': {}", path, e)
                    })?;
                }
            }
            fs::write(path, &content)
                .map_err(|e| anyhow::anyhow!("failed to write file '{}': {}", path, e))?;
        }
        None => {
            use std::io::Write;
            std::io::stdout()
                .write_all(&content)
                .map_err(|e| anyhow::anyhow!("failed to write to stdout: {}", e))?;
        }
    }

    Ok(())
}

/// What the store can say about a key: it is here, it is not here, or it is not
/// a key at all.
///
/// The third arm is why this is not a bool. `ctx_exists` reports a key that
/// fails the grammar as absent, so the two negatives used to arrive
/// indistinguishable and a caller probing with a substituted key would decide a
/// key was missing when koto had in fact refused to look for it (Issue #227).
pub enum KeyPresence {
    /// The key is in the store.
    Present,
    /// The key is a usable key and the store does not have it.
    Absent,
    /// The key is not usable, carrying the reason from
    /// [`crate::session::validate::unusable_context_key_reason`].
    Unusable(String),
}

/// Check whether a key exists, distinguishing a key the store will not accept
/// from one it accepts and does not have.
///
/// The key is checked here rather than in the store because the store's own
/// answer is a bool with nowhere to put a reason, and because the context gate
/// already checks caller-side -- one mechanism for the question rather than two.
/// The wording is not composed here either: both callers share
/// [`crate::session::validate::unusable_context_key_reason`] so they cannot
/// drift into describing the same key differently.
///
/// The caller is responsible for mapping the outcome to exit codes.
pub fn handle_exists(store: &dyn ContextStore, session: &str, key: &str) -> KeyPresence {
    if let Some(reason) = crate::session::validate::unusable_context_key_reason(key) {
        return KeyPresence::Unusable(reason);
    }
    if store.ctx_exists(session, key) {
        KeyPresence::Present
    } else {
        KeyPresence::Absent
    }
}

/// Remove a key and its content from the store, then emit a `context_removed`
/// event to the session log.
///
/// Idempotent: removing a key that is not there succeeds, matching
/// `ContextStore::remove`'s own contract and the usual shape of a delete verb.
/// A caller that needs to distinguish the two cases probes with
/// `context exists` first.
///
/// The event is emitted unconditionally, including on the idempotent no-op.
/// The log is the authoritative record of what happened to a session, and
/// `add` already writes one; a removal that left no trace would leave a log
/// asserting a key was added and never saying it went away, which is worse than
/// an occasional event for a key that was already absent.
pub fn handle_remove(
    store: &dyn ContextStore,
    backend: &dyn SessionBackend,
    session: &str,
    key: &str,
) -> Result<()> {
    store.remove(session, key)?;

    let event = EventPayload::ContextRemoved {
        key: key.to_string(),
    };
    backend.append_event(session, &event, &now_iso8601())?;

    Ok(())
}

/// List all keys as a JSON array, optionally filtered by prefix.
pub fn handle_list(store: &dyn ContextStore, session: &str, prefix: Option<&str>) -> Result<()> {
    let keys = store.list_keys(session, prefix)?;
    println!("{}", serde_json::to_string(&keys)?);
    Ok(())
}
