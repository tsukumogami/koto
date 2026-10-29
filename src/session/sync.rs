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
    // The pin records are objects of their own, read by the pin guard.
    manifest.drop_host_local();
    manifest.drop_pin_records();
    cache.set(session, manifest.clone());
    Some(manifest)
}

/// The manifest at `path` as the bucket gets it: the file's bytes, or,
/// when it lists a host-local key
/// ([`is_host_local_key`](crate::session::context::is_host_local_key)) or a
/// pin record ([`Manifest::drop_pin_records`]), the manifest re-serialized
/// without them. `None` when there is no file. Bytes that don't parse go up
/// as they are, as every manifest did before: they name no key this could
/// drop, and refusing would leave a batch open.
///
/// Every manifest koto uploads from a local store goes through this, so no
/// manifest koto pushes names a host-local key or a pin record.
///
/// `withheld` names keys the push leaves out besides: a continues relation
/// whose back-reference beside its predecessor isn't confirmed yet
/// ([`crate::session::continues_bucket::unpushable_keys`]), which reaches
/// the bucket only once the pointer does.
pub fn bucket_manifest_bytes(
    path: &std::path::Path,
    withheld: &[String],
) -> anyhow::Result<Option<Vec<u8>>> {
    use anyhow::Context;
    let data = match std::fs::read(path) {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let Ok(mut manifest) = serde_json::from_slice::<Manifest>(&data) else {
        return Ok(Some(data));
    };
    let host_local = manifest.drop_host_local();
    let before = manifest.keys.len();
    manifest.keys.retain(|key, _| !withheld.contains(key));
    let withheld_any = manifest.keys.len() != before;
    if !manifest.drop_pin_records() && !host_local && !withheld_any {
        return Ok(Some(data));
    }
    Ok(Some(
        serde_json::to_vec_pretty(&manifest).context("failed to serialize manifest")?,
    ))
}

/// The keys a push of `session` leaves out of the bucket for now: its
/// continues relation while the back-reference it calls for isn't
/// confirmed.
pub fn withheld_keys(local: &LocalBackend, prefix: &str, session: &str) -> Vec<String> {
    crate::session::continues_bucket::unpushable_keys(&local.session_dir(session), prefix, session)
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
    let withheld = withheld_keys(local, prefix, session);
    match try_push_context_key(local, bucket, prefix, session, key, &withheld) {
        Ok(()) => cache.invalidate(),
        Err(e) => eprintln!("warning: {e:#}"),
    }
}

/// [`push_context_key`] for a caller that must know the bucket took the
/// key and the manifest: the first upload that fails is returned. A key
/// the push leaves out (host-local, a pin record, or in `withheld`), or
/// one with no content file, pushes nothing and succeeds. The caller
/// invalidates the manifest cache.
pub fn try_push_context_key(
    local: &LocalBackend,
    bucket: &Bucket,
    prefix: &str,
    session: &str,
    key: &str,
    withheld: &[String],
) -> anyhow::Result<()> {
    if crate::session::context::is_host_local_key(key)
        || crate::session::context::is_pin_record_key(key)
        || withheld.iter().any(|w| w == key)
    {
        return Ok(());
    }
    let ctx_dir = local.session_dir(session).join("ctx");

    // Upload the content file.
    let content_path = ctx_dir.join(key);
    if !content_path.exists() {
        return Ok(());
    }
    let data = std::fs::read(&content_path)
        .map_err(|e| anyhow::anyhow!("cloud sync: failed to read context key '{}': {}", key, e))?;
    let s3_key = format!("{}/{}/ctx/{}", prefix, session, key);
    bucket.put_object(&s3_key, &data).map_err(|e| {
        anyhow::anyhow!(
            "cloud sync failed for context upload '{}': {}",
            key,
            crate::session::cloud::without_url_userinfo(&e.to_string())
        )
    })?;

    // Upload the manifest.
    let manifest_data = match bucket_manifest_bytes(&ctx_dir.join("manifest.json"), withheld)
        .map_err(|e| anyhow::anyhow!("cloud sync: failed to read manifest: {:#}", e))?
    {
        Some(d) => d,
        None => return Ok(()),
    };
    let manifest_s3_key = format!("{}/{}/ctx/manifest.json", prefix, session);
    bucket
        .put_object(&manifest_s3_key, &manifest_data)
        .map_err(|e| {
            anyhow::anyhow!(
                "cloud sync failed for manifest upload: {}",
                crate::session::cloud::without_url_userinfo(&e.to_string())
            )
        })?;
    Ok(())
}

/// What [`pull_context_if_newer`] did with a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pull {
    /// The local copy stands and matches what the bucket has, or the bucket
    /// has nothing to give.
    Unchanged,
    /// The bucket's copy was written over the local one.
    Wrote,
    /// The copies differ and the local one was written later, so it stands
    /// and the bucket lacks it (koto#520).
    LocalNewer,
}

/// Whether `local` was written later than `remote`, both timestamps in the
/// form koto writes them (`YYYY-MM-DDTHH:MM:SS.mmmZ`, or without the
/// milliseconds). A timestamp in any other form never counts as later, so
/// the pull goes ahead as it did before.
///
/// The stamps come from the clocks of the hosts that wrote them (a pull
/// stamps the time it pulled), so two writes made on different hosts within
/// their clocks' skew can sort the wrong way round.
fn written_later(local: &str, remote: &str) -> bool {
    match (sortable_timestamp(local), sortable_timestamp(remote)) {
        (Some(local), Some(remote)) => local > remote,
        _ => false,
    }
}

/// `ts` as `YYYY-MM-DDTHH:MM:SS.mmmZ`, which sorts as text, or `None` when
/// it is in neither form koto writes.
fn sortable_timestamp(ts: &str) -> Option<String> {
    if !ts.is_ascii() {
        return None;
    }
    let ts = match ts.len() {
        20 => format!("{}.000Z", &ts[..19]),
        24 => ts.to_string(),
        _ => return None,
    };
    let b = ts.as_bytes();
    let well_formed = b.iter().enumerate().all(|(i, c)| match i {
        4 | 7 => *c == b'-',
        10 => *c == b'T',
        13 | 16 => *c == b':',
        19 => *c == b'.',
        23 => *c == b'Z',
        _ => c.is_ascii_digit(),
    });
    well_formed.then_some(ts)
}

/// Pull a context key from S3 if the remote version differs from local.
///
/// Compares the hash in the remote manifest against the local manifest.
/// If they differ (or the key is missing locally), downloads the content
/// and writes it locally with writer `sync` -- unless the local copy was
/// written later than the remote one, which then stands untouched
/// ([`Pull::LocalNewer`]). A session whose home moved can find an older
/// copy under its new home than the one it holds (koto#520); a write from
/// another host after this one's still comes down.
///
/// Returns [`Pull::Wrote`] when it wrote the local store, so the caller can
/// log the write (a `context_added` with `writer: "sync"`).
///
/// `before_download` runs once a download is due and before it starts, and
/// nothing is written after its error.
///
/// Once the bucket has said its copy of the key may differ from this host's,
/// a step that fails is an error, never a fall-back to the local copy, which
/// may be stale (koto#457): a manifest the session's listing showed
/// (`manifest_listed`) that can't be fetched, a key whose hash differs and
/// whose object can't be fetched (a 404 from an object the manifest names
/// but the bucket lacks, say), or downloaded bytes that can't be written. A
/// manifest the listing didn't show, or no listing at all (an unreachable
/// bucket), leaves the local copy standing, [`Pull::Unchanged`], as before.
#[allow(clippy::too_many_arguments)]
pub fn pull_context_if_newer(
    local: &LocalBackend,
    bucket: &Bucket,
    prefix: &str,
    session: &str,
    key: &str,
    cache: &ManifestCache,
    manifest_listed: bool,
    before_download: &dyn Fn() -> anyhow::Result<()>,
) -> anyhow::Result<Pull> {
    let manifest_key = format!("{}/{}/ctx/manifest.json", prefix, session);
    let remote_manifest = match fetch_remote_manifest(bucket, &manifest_key, session, cache) {
        Some(m) => m,
        None if manifest_listed => anyhow::bail!(
            "cloud sync: the bucket lists a context manifest for session '{}' but fetching it \
             failed, so the local copy of '{}' may be stale; run the command again",
            session,
            key
        ),
        None => return Ok(Pull::Unchanged),
    };

    let remote_meta = match remote_manifest.keys.get(key) {
        Some(m) => m,
        None => return Ok(Pull::Unchanged), // Key doesn't exist remotely.
    };

    // Check if local hash matches remote hash.
    let local_manifest = local.read_manifest(session).unwrap_or_default();
    if let Some(local_meta) = local_manifest.keys.get(key) {
        if local_meta.hash == remote_meta.hash {
            return Ok(Pull::Unchanged); // Hashes match, no download needed.
        }
        if written_later(&local_meta.created_at, &remote_meta.created_at) {
            return Ok(Pull::LocalNewer);
        }
    }
    before_download()?;

    // Download the content from S3.
    let s3_key = format!("{}/{}/ctx/{}", prefix, session, key);
    let response = bucket.get_object(&s3_key).map_err(|e| {
        anyhow::anyhow!(
            "cloud sync: the bucket holds a different version of context key '{}' in session \
             '{}' but fetching it failed, so the local copy may be stale: {}",
            key,
            session,
            crate::session::cloud::without_url_userinfo(&e.to_string())
        )
    })?;
    if response.status_code() != 200 {
        anyhow::bail!(
            "cloud sync: the bucket holds a different version of context key '{}' in session \
             '{}' but fetching it answered {}, so the local copy may be stale",
            key,
            session,
            response.status_code()
        );
    }

    // Write locally, naming the pull as the writer.
    local
        .add_with_writer(
            session,
            key,
            response.bytes(),
            crate::session::context_log::WRITER_SYNC,
        )
        .map_err(|e| {
            anyhow::anyhow!(
                "cloud sync: failed to write the downloaded context key '{}' of session '{}', \
                 so the local copy may be stale: {}",
                key,
                session,
                e
            )
        })?;
    Ok(Pull::Wrote)
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
    let withheld = withheld_keys(local, prefix, session);
    if let Ok(Some(data)) = bucket_manifest_bytes(&manifest_path, &withheld) {
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
        assert!(bucket_manifest_bytes(&path, &[]).unwrap().is_none());

        let entry = r#"{"created_at":"t","size":1,"hash":"h"}"#;
        let plain = format!(r#"{{"keys":{{"notes.md":{entry}}}}}"#);
        std::fs::write(&path, &plain).unwrap();
        assert_eq!(
            bucket_manifest_bytes(&path, &[]).unwrap().unwrap(),
            plain.as_bytes()
        );

        let with_location =
            format!(r#"{{"keys":{{"notes.md":{entry},"workflows/publish-location":{entry}}}}}"#);
        std::fs::write(&path, &with_location).unwrap();
        let pushed: Manifest =
            serde_json::from_slice(&bucket_manifest_bytes(&path, &[]).unwrap().unwrap()).unwrap();
        assert_eq!(pushed.keys.keys().collect::<Vec<_>>(), ["notes.md"]);
    }

    /// No manifest koto pushes names a pin record: the pin and the release
    /// record live in the bucket as objects of their own, and a manifest
    /// without them removes neither.
    #[test]
    fn bucket_manifest_bytes_drops_the_pin_records() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("manifest.json");
        let entry = r#"{"created_at":"t","size":1,"hash":"h"}"#;
        std::fs::write(
            &path,
            format!(
                r#"{{"keys":{{"koto/links/held-by/a":{entry},"koto/links/pin":{entry},"koto/pin-release":{entry},"notes.md":{entry}}}}}"#
            ),
        )
        .unwrap();
        let pushed: Manifest =
            serde_json::from_slice(&bucket_manifest_bytes(&path, &[]).unwrap().unwrap()).unwrap();
        assert_eq!(
            pushed.keys.keys().collect::<Vec<_>>(),
            ["koto/links/held-by/a", "notes.md"]
        );
    }

    /// A local copy counts as later only when both timestamps are in a form
    /// koto writes; anything else lets the pull go ahead.
    #[test]
    fn written_later_compares_koto_timestamps() {
        assert!(written_later(
            "2026-10-28T17:49:00.001Z",
            "2026-10-26T09:00:00.000Z"
        ));
        assert!(!written_later(
            "2026-10-26T09:00:00.000Z",
            "2026-10-28T17:49:00.001Z"
        ));
        assert!(!written_later(
            "2026-10-28T17:49:00.000Z",
            "2026-10-28T17:49:00.000Z"
        ));
        // Without milliseconds, as older manifests carry it.
        assert!(written_later(
            "2026-10-28T17:49:00.500Z",
            "2026-10-28T17:49:00Z"
        ));
        assert!(!written_later(
            "2026-10-28T17:49:00Z",
            "2026-10-28T17:49:00.500Z"
        ));
        assert!(!written_later("t", "2026-10-28T17:49:00.000Z"));
        assert!(!written_later("2026-10-28T17:49:00.000Z", ""));
        assert!(!written_later(
            "2026-10-28 17:49:00.000Z",
            "2026-01-01T00:00:00.000Z"
        ));
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
