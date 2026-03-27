use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Metadata for a single context key in the manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KeyMeta {
    pub created_at: String,
    pub size: u64,
    pub hash: String,
}

/// On-disk manifest format stored at `ctx/manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Manifest {
    pub keys: BTreeMap<String, KeyMeta>,
}

/// Content store for workflow context artifacts.
///
/// Agents submit and retrieve context through this trait instead of
/// writing directly to the filesystem. Content is keyed by hierarchical
/// path strings (e.g. `scope.md`, `research/r1/lead-cli-ux.md`).
///
/// `LocalBackend` implements this trait alongside `SessionBackend`.
pub trait ContextStore: Send + Sync {
    /// Store content under the given key, creating or replacing it.
    fn add(&self, session: &str, key: &str, content: &[u8]) -> anyhow::Result<()>;

    /// Retrieve content for the given key.
    fn get(&self, session: &str, key: &str) -> anyhow::Result<Vec<u8>>;

    /// Check whether a key exists in the context store.
    fn ctx_exists(&self, session: &str, key: &str) -> bool;

    /// Remove a key and its content from the store.
    fn remove(&self, session: &str, key: &str) -> anyhow::Result<()>;

    /// List all keys, optionally filtered by prefix.
    fn list_keys(&self, session: &str, prefix: Option<&str>) -> anyhow::Result<Vec<String>>;
}
