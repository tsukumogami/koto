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
/// `:` or an `@` and produce an illegal key. The two are not going to converge
/// (Issue #227). A value is content and admits those three so it can hold a
/// title or a filter expression; a key is an address, and one of its uses is as
/// an argument in the `koto context add` and `koto context get` commands
/// templates run, where a space would word-split. What the asymmetry gets
/// instead of convergence is a shared wording:
/// [`crate::session::validate::unusable_context_key_reason`] is how a caller
/// turns a refusal into something an operator can act on.
pub trait ContextStore: Send + Sync {
    /// Store content under the given key, creating or replacing it.
    fn add(&self, session: &str, key: &str, content: &[u8]) -> anyhow::Result<()>;

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

    /// List all keys, optionally filtered by prefix.
    fn list_keys(&self, session: &str, prefix: Option<&str>) -> anyhow::Result<Vec<String>>;
}
