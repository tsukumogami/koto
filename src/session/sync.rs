//! Per-key incremental sync helpers for `CloudBackend`.
//!
//! Provides push/pull functions that transfer individual context keys and
//! manifests between the local filesystem and S3. A short-lived TTL cache
//! on the remote manifest reduces redundant GETs during rapid sequential
//! operations (e.g., multiple `koto context add` calls in a burst).

use std::cell::RefCell;
use std::time::{Duration, Instant};

use s3::Bucket;

use crate::session::context::{ContextStore, Manifest};
use crate::session::local::LocalBackend;
use crate::session::SessionBackend;

/// TTL for cached remote manifests. Manifest GETs that occur within this
/// window after a previous fetch reuse the cached copy.
const MANIFEST_TTL: Duration = Duration::from_secs(5);

/// Cached remote manifest for a single session.
///
/// Uses interior mutability (`RefCell`) because `ContextStore` methods
/// take `&self`. The cache is single-session: if the caller switches
/// sessions, the cache is invalidated automatically.
pub struct ManifestCache {
    inner: RefCell<CacheInner>,
}

struct CacheInner {
    manifest: Option<Manifest>,
    fetched_at: Option<Instant>,
    session: String,
}

impl Default for ManifestCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ManifestCache {
    pub fn new() -> Self {
        Self {
            inner: RefCell::new(CacheInner {
                manifest: None,
                fetched_at: None,
                session: String::new(),
            }),
        }
    }

    /// Return a cached manifest if it belongs to `session` and is within TTL.
    pub fn get(&self, session: &str) -> Option<Manifest> {
        let inner = self.inner.borrow();
        if inner.session != session {
            return None;
        }
        let fetched = inner.fetched_at?;
        if fetched.elapsed() < MANIFEST_TTL {
            inner.manifest.clone()
        } else {
            None
        }
    }

    /// Store a manifest in the cache for the given session.
    pub fn set(&self, session: &str, manifest: Manifest) {
        let mut inner = self.inner.borrow_mut();
        inner.session = session.to_string();
        inner.manifest = Some(manifest);
        inner.fetched_at = Some(Instant::now());
    }

    /// Invalidate the cache (e.g., after a push that changes the manifest).
    pub fn invalidate(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.manifest = None;
        inner.fetched_at = None;
    }
}

// SAFETY: ManifestCache uses RefCell which is !Sync. CloudBackend must be
// Send + Sync (required by ContextStore). We use RefCell because concurrent
// access to the cache is not a concern in koto's single-threaded CLI model.
// The ContextStore trait requires Sync, but koto never shares CloudBackend
// across threads. This unsafe impl satisfies the trait bound.
unsafe impl Sync for ManifestCache {}

/// Fetch the remote manifest from S3, using the cache if available.
///
/// Returns `None` if the fetch fails (S3 unreachable, key missing, etc.).
pub fn fetch_remote_manifest(
    bucket: &Bucket,
    manifest_key: &str,
    session: &str,
    cache: &ManifestCache,
) -> Option<Manifest> {
    // Check cache first.
    if let Some(cached) = cache.get(session) {
        return Some(cached);
    }

    // Fetch from S3.
    let response = bucket.get_object(manifest_key).ok()?;
    if response.status_code() != 200 {
        return None;
    }
    let mut manifest: Manifest = serde_json::from_slice(response.bytes()).ok()?;
    // A manifest an older koto pushed can list a host-local key, often
    // without its object; this host answers those keys from its own store.
    manifest.drop_host_local();
    cache.set(session, manifest.clone());
    Some(manifest)
}

/// The manifest at `path` as the bucket gets it: the file's bytes, or,
/// when it lists a host-local key
/// ([`is_host_local_key`](crate::session::context::is_host_local_key)),
/// the manifest re-serialized without them. `None` when there is no file.
///
/// Every manifest koto uploads from a local store goes through this, so the
/// bucket never lists a key whose object it was never sent.
pub fn bucket_manifest_bytes(path: &std::path::Path) -> anyhow::Result<Option<Vec<u8>>> {
    use anyhow::Context;
    let data = match std::fs::read(path) {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let mut manifest: Manifest = serde_json::from_slice(&data)
        .with_context(|| format!("failed to parse manifest: {}", path.display()))?;
    if !manifest.drop_host_local() {
        return Ok(Some(data));
    }
    Ok(Some(
        serde_json::to_vec_pretty(&manifest).context("failed to serialize manifest")?,
    ))
}

/// Push a single context key and the manifest to S3.
///
/// Reads the content file and manifest from `local`, uploads both to S3.
/// Failures are logged to stderr but do not propagate.
pub fn push_context_key(
    local: &LocalBackend,
    bucket: &Bucket,
    prefix: &str,
    session: &str,
    key: &str,
    cache: &ManifestCache,
) {
    if crate::session::context::is_host_local_key(key) {
        return;
    }
    let ctx_dir = local.session_dir(session).join("ctx");

    // Upload the content file.
    let content_path = ctx_dir.join(key);
    if !content_path.exists() {
        return;
    }
    let data = match std::fs::read(&content_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!(
                "warning: cloud sync: failed to read context key '{}': {}",
                key, e
            );
            return;
        }
    };
    let s3_key = format!("{}/{}/ctx/{}", prefix, session, key);
    if let Err(e) = bucket.put_object(&s3_key, &data) {
        eprintln!(
            "warning: cloud sync failed for context upload '{}': {}",
            key, e
        );
        return;
    }

    // Upload the manifest.
    let manifest_data = match bucket_manifest_bytes(&ctx_dir.join("manifest.json")) {
        Ok(Some(d)) => d,
        Ok(None) => return,
        Err(e) => {
            eprintln!("warning: cloud sync: failed to read manifest: {:#}", e);
            return;
        }
    };
    let manifest_s3_key = format!("{}/{}/ctx/manifest.json", prefix, session);
    if let Err(e) = bucket.put_object(&manifest_s3_key, &manifest_data) {
        eprintln!("warning: cloud sync failed for manifest upload: {}", e);
        return;
    }

    // Invalidate the cache since we just changed the remote manifest.
    cache.invalidate();
}

/// Pull a context key from S3 if the remote version differs from local.
///
/// Compares the hash in the remote manifest against the local manifest.
/// If they differ (or the key is missing locally), downloads the content
/// and writes it locally with writer `sync`.
///
/// Returns whether it wrote the local store, so the caller can log the
/// write (a `context_added` with `writer: "sync"`).
///
/// `before_download` runs once a download is due and before it starts; its
/// error is the only one returned, and nothing is written after it. Every
/// other failure is a warning and `Ok(false)`, as before.
pub fn pull_context_if_newer(
    local: &LocalBackend,
    bucket: &Bucket,
    prefix: &str,
    session: &str,
    key: &str,
    cache: &ManifestCache,
    before_download: &dyn Fn() -> anyhow::Result<()>,
) -> anyhow::Result<bool> {
    let manifest_key = format!("{}/{}/ctx/manifest.json", prefix, session);
    let remote_manifest = match fetch_remote_manifest(bucket, &manifest_key, session, cache) {
        Some(m) => m,
        None => return Ok(false),
    };

    let remote_meta = match remote_manifest.keys.get(key) {
        Some(m) => m,
        None => return Ok(false), // Key doesn't exist remotely.
    };

    // Check if local hash matches remote hash.
    let local_manifest = local.read_manifest(session).unwrap_or_default();
    if let Some(local_meta) = local_manifest.keys.get(key) {
        if local_meta.hash == remote_meta.hash {
            return Ok(false); // Hashes match, no download needed.
        }
    }
    before_download()?;

    // Download the content from S3.
    let s3_key = format!("{}/{}/ctx/{}", prefix, session, key);
    let response = match bucket.get_object(&s3_key) {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "warning: cloud sync: failed to download context key '{}': {}",
                key, e
            );
            return Ok(false);
        }
    };
    if response.status_code() != 200 {
        eprintln!(
            "warning: cloud sync: unexpected status {} for key '{}'",
            response.status_code(),
            key
        );
        return Ok(false);
    }

    // Write locally, naming the pull as the writer.
    match local.add_with_writer(
        session,
        key,
        response.bytes(),
        crate::session::context_log::WRITER_SYNC,
    ) {
        Ok(()) => Ok(true),
        Err(e) => {
            eprintln!(
                "warning: cloud sync: failed to write downloaded key '{}': {}",
                key, e
            );
            Ok(false)
        }
    }
}

/// The remote manifest's metadata for `key`, if the remote store has it.
pub fn remote_key_meta(
    bucket: &Bucket,
    prefix: &str,
    session: &str,
    key: &str,
    cache: &ManifestCache,
) -> Option<crate::session::context::KeyMeta> {
    let manifest_key = format!("{}/{}/ctx/manifest.json", prefix, session);
    let mut manifest = fetch_remote_manifest(bucket, &manifest_key, session, cache)?;
    manifest.keys.remove(key)
}

/// Delete a context key from S3 and upload the updated manifest.
///
/// Failures are logged to stderr but do not propagate.
pub fn delete_context_key(
    local: &LocalBackend,
    bucket: &Bucket,
    prefix: &str,
    session: &str,
    key: &str,
    cache: &ManifestCache,
) {
    let s3_key = format!("{}/{}/ctx/{}", prefix, session, key);
    if let Err(e) = bucket.delete_object(&s3_key) {
        eprintln!(
            "warning: cloud sync: failed to delete context key '{}': {}",
            key, e
        );
    }

    // Upload the updated manifest (the local remove already happened).
    let manifest_path = local.session_dir(session).join("ctx").join("manifest.json");
    if let Ok(Some(data)) = bucket_manifest_bytes(&manifest_path) {
        let manifest_s3_key = format!("{}/{}/ctx/manifest.json", prefix, session);
        if let Err(e) = bucket.put_object(&manifest_s3_key, &data) {
            eprintln!("warning: cloud sync failed for manifest upload: {}", e);
        }
    }

    cache.invalidate();
}

/// Check if a key exists in the remote manifest.
pub fn remote_key_exists(
    bucket: &Bucket,
    prefix: &str,
    session: &str,
    key: &str,
    cache: &ManifestCache,
) -> Option<bool> {
    let manifest_key = format!("{}/{}/ctx/manifest.json", prefix, session);
    let manifest = fetch_remote_manifest(bucket, &manifest_key, session, cache)?;
    Some(manifest.keys.contains_key(key))
}

/// List keys from the remote manifest, optionally filtered by prefix.
pub fn remote_list_keys(
    bucket: &Bucket,
    s3_prefix: &str,
    session: &str,
    key_prefix: Option<&str>,
    cache: &ManifestCache,
) -> Option<Vec<String>> {
    let manifest_key = format!("{}/{}/ctx/manifest.json", s3_prefix, session);
    let manifest = fetch_remote_manifest(bucket, &manifest_key, session, cache)?;
    let keys: Vec<String> = manifest
        .keys
        .keys()
        .filter(|k| match key_prefix {
            Some(p) => k.starts_with(p),
            None => true,
        })
        .cloned()
        .collect();
    Some(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manifest with no host-local key goes up byte for byte; one with
    /// such a key goes up without it; no file is `None`.
    #[test]
    fn bucket_manifest_bytes_drops_only_host_local_keys() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("manifest.json");
        assert!(bucket_manifest_bytes(&path).unwrap().is_none());

        let entry = r#"{"created_at":"t","size":1,"hash":"h"}"#;
        let plain = format!(r#"{{"keys":{{"notes.md":{entry}}}}}"#);
        std::fs::write(&path, &plain).unwrap();
        assert_eq!(
            bucket_manifest_bytes(&path).unwrap().unwrap(),
            plain.as_bytes()
        );

        let with_location =
            format!(r#"{{"keys":{{"notes.md":{entry},"workflows/publish-location":{entry}}}}}"#);
        std::fs::write(&path, &with_location).unwrap();
        let pushed: Manifest =
            serde_json::from_slice(&bucket_manifest_bytes(&path).unwrap().unwrap()).unwrap();
        assert_eq!(pushed.keys.keys().collect::<Vec<_>>(), ["notes.md"]);
    }

    #[test]
    fn manifest_cache_returns_none_when_empty() {
        let cache = ManifestCache::new();
        assert!(cache.get("sess").is_none());
    }

    #[test]
    fn manifest_cache_returns_cached_manifest() {
        let cache = ManifestCache::new();
        let manifest = Manifest::default();
        cache.set("sess", manifest);
        assert!(cache.get("sess").is_some());
    }

    #[test]
    fn manifest_cache_returns_none_for_different_session() {
        let cache = ManifestCache::new();
        cache.set("sess-a", Manifest::default());
        assert!(cache.get("sess-b").is_none());
    }

    #[test]
    fn manifest_cache_invalidate_clears_cache() {
        let cache = ManifestCache::new();
        cache.set("sess", Manifest::default());
        cache.invalidate();
        assert!(cache.get("sess").is_none());
    }

    #[test]
    fn manifest_cache_set_overwrites_previous() {
        let cache = ManifestCache::new();
        cache.set("sess-a", Manifest::default());
        cache.set("sess-b", Manifest::default());
        assert!(cache.get("sess-a").is_none());
        assert!(cache.get("sess-b").is_some());
    }
}
