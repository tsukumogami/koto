use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Metadata for a single context key in the manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KeyMeta {
    pub created_at: String,
    pub size: u64,
    pub hash: String,
    /// Who wrote the key's current content: `agent`, `transition`, `koto` or
    /// `sync` (an open vocabulary). Absent for a key written before writers
    /// were recorded, or through plain [`ContextStore::add`]; it then reads
    /// as unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer: Option<String>,
}

/// On-disk manifest format stored at `ctx/manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Manifest {
    pub keys: BTreeMap<String, KeyMeta>,
}

impl Manifest {
    /// Drop the [host-local](is_host_local_key) keys, leaving the copy a
    /// bucket holds. Returns whether any were there.
    pub fn drop_host_local(&mut self) -> bool {
        let before = self.keys.len();
        self.keys.retain(|key, _| !is_host_local_key(key));
        self.keys.len() != before
    }

    /// Take out the pin and its release record. Under the cloud backend
    /// each lives as its own object (DESIGN-session-pin.md, Decision 3), so
    /// no manifest koto pushes to the bucket names them and none it takes
    /// from the bucket adds them to a local index: a push of a manifest
    /// without the pin removes nothing, and the pin object stays the truth.
    /// Returns whether either was listed.
    pub fn drop_pin_records(&mut self) -> bool {
        let before = self.keys.len();
        self.keys.retain(|key, _| !is_pin_record_key(key));
        self.keys.len() != before
    }
}

/// Whether `key` is the pin or its release record, or lies under the pin's
/// key (an unreadable pin that is a directory).
pub fn is_pin_record_key(key: &str) -> bool {
    crate::session::pin::is_pin_key(key) || key == crate::session::pin::RELEASE_KEY
}

/// Whether `key` describes the host a session runs on rather than the
/// session, so the cloud backend keeps it out of the bucket.
///
/// A host-local key is written to the local store only, no manifest pushed
/// to the bucket names it, and an import skips it, even when a manifest an
/// older koto pushed lists it. The destination resolves its own value on its
/// first commit. The one such key is `workflows/publish-location`, which
/// names a Claude Code `/workflows` folder on this host and is meaningless
/// on any other.
pub fn is_host_local_key(key: &str) -> bool {
    key == crate::workflows_surface::PUBLISH_LOCATION_KEY
}

/// Content store for workflow context artifacts.
///
/// Agents submit and retrieve context through this trait instead of
/// writing directly to the filesystem. Content is keyed by hierarchical
/// path strings (e.g. `scope.md`, `research/r1/lead-cli-ux.md`).
///
/// `LocalBackend` implements this trait alongside `SessionBackend`.
///
/// # The key grammar is part of the contract
///
/// A key must satisfy `crate::session::validate::validate_context_key`:
/// `/`-separated components, each starting with a letter or digit and
/// continuing in letters, digits, `.`, `_` and `-`. An implementation is
/// expected to honour that grammar rather than invent its own, because callers
/// outside this module now depend on it -- a context gate checks a substituted
/// key against it so it can report *why* an unusable key failed instead of
/// letting the answer arrive as a bare "absent" (Issue #222).
///
/// Note the asymmetry a caller has to live with: this grammar is narrower than
/// the variable-value allowlist, so a legal `--var` value can hold a space, a
/// `:`, an `@` or a `+` and produce an illegal key. The two are not going to
/// converge (Issue #227). A value is content and admits those four so it can
/// hold a title, a filter expression or a path; a key is an address, and one of its uses is as
/// an argument in the `koto context add` and `koto context get` commands
/// templates run, where a space would word-split. What the asymmetry gets
/// instead of convergence is a shared wording:
/// [`crate::session::validate::unusable_context_key_reason`] is how a caller
/// turns a refusal into something an operator can act on.
pub trait ContextStore: Send + Sync {
    /// Store content under the given key, creating or replacing it.
    fn add(&self, session: &str, key: &str, content: &[u8]) -> anyhow::Result<()>;

    /// [`add`](Self::add), recording `writer` in the key's metadata
    /// ([`KeyMeta::writer`]). Every production write goes through this; the
    /// default body ignores the writer and calls `add`, so a store that keeps
    /// no metadata needs nothing more.
    ///
    /// This writes the store only. Appending the matching `context_added`
    /// event is the caller's job, because only the caller knows whether the
    /// write is one the log records (a transition's assignment is recorded by
    /// the `transitioned` event itself).
    ///
    /// The agent writer is refused any key under the reserved `koto/` prefix
    /// ([`crate::session::links::check_writer_key`]); an implementation that
    /// overrides this method makes the same check before it writes.
    fn add_with_writer(
        &self,
        session: &str,
        key: &str,
        content: &[u8],
        writer: &str,
    ) -> anyhow::Result<()> {
        crate::session::links::check_writer_key(key, writer)?;
        self.add(session, key, content)
    }

    /// The stored metadata for `key`, or `None` when the key is absent, fails
    /// the key grammar, or the store keeps no metadata (the default).
    fn meta(&self, session: &str, key: &str) -> Option<KeyMeta> {
        let _ = (session, key);
        None
    }

    /// Retrieve content for the given key.
    fn get(&self, session: &str, key: &str) -> anyhow::Result<Vec<u8>>;

    /// Check whether a key exists in the context store.
    ///
    /// A key that fails the grammar above is reported absent rather than as an
    /// error, so this returning `false` does not distinguish "no such key" from
    /// "not a key at all". A caller that needs to tell them apart checks the
    /// key itself first, with
    /// [`crate::session::validate::unusable_context_key_reason`] -- which is
    /// what the context gates and `koto context exists` do, and is why this
    /// signature stays a bool rather than growing a result every caller would
    /// have to handle.
    fn ctx_exists(&self, session: &str, key: &str) -> bool;

    /// Remove a key and its content from the store.
    fn remove(&self, session: &str, key: &str) -> anyhow::Result<()>;

    /// [`remove`](Self::remove) on behalf of `writer`. The agent writer is
    /// refused any key under the reserved `koto/` prefix, before anything is
    /// removed ([`crate::session::links::check_writer_key`]).
    fn remove_with_writer(&self, session: &str, key: &str, writer: &str) -> anyhow::Result<()> {
        crate::session::links::check_writer_key(key, writer)?;
        self.remove(session, key)
    }

    /// List all keys, optionally filtered by prefix.
    fn list_keys(&self, session: &str, prefix: Option<&str>) -> anyhow::Result<Vec<String>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manifest entry written before writers were recorded (koto v0.14.1)
    /// reads with no writer, and one with no writer writes back without the
    /// field.
    #[test]
    fn key_meta_without_a_writer_round_trips_unchanged() {
        let old = r#"{"created_at":"2026-01-01T00:00:00Z","size":3,"hash":"ab"}"#;
        let meta: KeyMeta = serde_json::from_str(old).unwrap();
        assert_eq!(meta.writer, None);
        assert_eq!(serde_json::to_string(&meta).unwrap(), old);

        let new = r#"{"created_at":"2026-01-01T00:00:00Z","size":3,"hash":"ab","writer":"sync"}"#;
        let meta: KeyMeta = serde_json::from_str(new).unwrap();
        assert_eq!(meta.writer.as_deref(), Some("sync"));
        assert_eq!(serde_json::to_string(&meta).unwrap(), new);
    }

    /// A store that implements only the original five methods, as existing
    /// test doubles do.
    #[derive(Default)]
    struct Minimal {
        added: std::sync::Mutex<Vec<(String, Vec<u8>)>>,
    }

    impl ContextStore for Minimal {
        fn add(&self, _session: &str, key: &str, content: &[u8]) -> anyhow::Result<()> {
            self.added
                .lock()
                .unwrap()
                .push((key.to_string(), content.to_vec()));
            Ok(())
        }
        fn get(&self, _session: &str, _key: &str) -> anyhow::Result<Vec<u8>> {
            anyhow::bail!("absent")
        }
        fn ctx_exists(&self, _session: &str, _key: &str) -> bool {
            false
        }
        fn remove(&self, _session: &str, _key: &str) -> anyhow::Result<()> {
            Ok(())
        }
        fn list_keys(&self, _session: &str, _prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn drop_host_local_keeps_every_other_key() {
        let meta = KeyMeta {
            created_at: "2026-01-01T00:00:00Z".to_string(),
            size: 1,
            hash: "ab".to_string(),
            writer: None,
        };
        let mut manifest = Manifest::default();
        for key in [
            "workflows/publish-location",
            "workflows/other",
            "batch_final_view",
        ] {
            manifest.keys.insert(key.to_string(), meta.clone());
        }
        assert!(manifest.drop_host_local());
        assert_eq!(
            manifest.keys.keys().collect::<Vec<_>>(),
            ["batch_final_view", "workflows/other"]
        );
        assert!(!manifest.drop_host_local());
    }

    #[test]
    fn default_bodies_write_through_add_and_report_no_metadata() {
        let store = Minimal::default();
        store.add_with_writer("s", "k", b"v", "koto").unwrap();
        assert_eq!(
            *store.added.lock().unwrap(),
            vec![("k".to_string(), b"v".to_vec())]
        );
        assert!(store.meta("s", "k").is_none());
    }
}
