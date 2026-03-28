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
    let manifest: Manifest = serde_json::from_slice(response.bytes()).ok()?;
    cache.set(session, manifest.clone());
    Some(manifest)
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
    let manifest_path = ctx_dir.join("manifest.json");
    if !manifest_path.exists() {
        return;
    }
    let manifest_data = match std::fs::read(&manifest_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("warning: cloud sync: failed to read manifest: {}", e);
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
/// and writes it via `local.add()`.
pub fn pull_context_if_newer(
    local: &LocalBackend,
    bucket: &Bucket,
    prefix: &str,
    session: &str,
    key: &str,
    cache: &ManifestCache,
) {
    let manifest_key = format!("{}/{}/ctx/manifest.json", prefix, session);
    let remote_manifest = match fetch_remote_manifest(bucket, &manifest_key, session, cache) {
        Some(m) => m,
        None => return,
    };

    let remote_meta = match remote_manifest.keys.get(key) {
        Some(m) => m,
        None => return, // Key doesn't exist remotely.
    };

    // Check if local hash matches remote hash.
    let local_manifest = local.read_manifest(session).unwrap_or_default();
    if let Some(local_meta) = local_manifest.keys.get(key) {
        if local_meta.hash == remote_meta.hash {
            return; // Hashes match, no download needed.
        }
    }

    // Download the content from S3.
    let s3_key = format!("{}/{}/ctx/{}", prefix, session, key);
    let response = match bucket.get_object(&s3_key) {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "warning: cloud sync: failed to download context key '{}': {}",
                key, e
            );
            return;
        }
    };
    if response.status_code() != 200 {
        eprintln!(
            "warning: cloud sync: unexpected status {} for key '{}'",
            response.status_code(),
            key
        );
        return;
    }

    // Write locally via the local backend's add method.
    if let Err(e) = local.add(session, key, response.bytes()) {
        eprintln!(
            "warning: cloud sync: failed to write downloaded key '{}': {}",
            key, e
        );
    }
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
    if manifest_path.exists() {
        if let Ok(data) = std::fs::read(&manifest_path) {
            let manifest_s3_key = format!("{}/{}/ctx/manifest.json", prefix, session);
            if let Err(e) = bucket.put_object(&manifest_s3_key, &data) {
                eprintln!("warning: cloud sync failed for manifest upload: {}", e);
            }
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
