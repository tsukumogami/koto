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
/// `withheld` names keys the push leaves out besides: a continues relation
/// whose back-reference beside its predecessor isn't confirmed yet
/// ([`crate::session::continues_bucket::unpushable_keys`]), which reaches
/// the bucket only once the pointer does.
///
/// Private: every upload builds its manifest through [`bucket_manifest_of`],
/// which leaves out the owed keys too.
fn bucket_manifest_bytes(
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

/// The manifest of the session whose local directory is `session_dir`, as
/// the bucket gets it ([`bucket_manifest_bytes`]), leaving out besides
/// every key whose object the bucket hasn't confirmed
/// ([`crate::session::sync_stamp::OwedKeys`]) (koto#646). An owed record
/// that doesn't parse is an error, never read as empty.
///
/// Every manifest koto uploads from a local store goes through this, so no
/// manifest koto pushes names a host-local key, a pin record, or an object
/// the bucket may lack.
pub fn bucket_manifest_of(
    session_dir: &std::path::Path,
    withheld: &[String],
) -> anyhow::Result<Option<Vec<u8>>> {
    let owed = crate::session::sync_stamp::OwedKeys::load(session_dir)?;
    let mut left_out = withheld.to_vec();
    left_out.extend(owed.keys.into_keys());
    bucket_manifest_bytes(&session_dir.join("ctx").join("manifest.json"), &left_out)
}

/// How many times a context key's upload asks the bucket to store its
/// object, the first included, before it reads the object back
/// ([`put_confirmed`]). Each ask is a request plus rust-s3's own retry of
/// it a second later, so two asks bound what an outage costs a write.
pub(crate) const CONTEXT_UPLOAD_ATTEMPTS: u32 = 2;

/// [`put_confirmed`]'s wait before its second ask, doubled before any ask
/// after that.
const CONTEXT_UPLOAD_BACKOFF: Duration = Duration::from_millis(250);

/// PUT `data` under `s3_key`, asked up to [`CONTEXT_UPLOAD_ATTEMPTS`] times
/// while the failure is [transient](crate::session::cloud::is_transient).
/// A last failure the bucket may have stored anyway (a lost reply, a 500
/// after the write) is checked by reading the object back
/// ([`object_holds`]); a refusal such as a 403 is not.
pub(crate) fn put_confirmed(
    bucket: &Bucket,
    s3_key: &str,
    data: &[u8],
) -> Result<(), s3::error::S3Error> {
    use crate::session::cloud::is_transient;
    let mut wait = CONTEXT_UPLOAD_BACKOFF;
    let mut attempt = 1;
    let failure = loop {
        match bucket.put_object(s3_key, data) {
            Ok(_) => return Ok(()),
            Err(e) if attempt < CONTEXT_UPLOAD_ATTEMPTS && is_transient(&e) => {
                std::thread::sleep(wait);
                wait *= 2;
                attempt += 1;
            }
            Err(e) => break e,
        }
    };
    if is_transient(&failure) && object_holds(bucket, s3_key, data) {
        return Ok(());
    }
    Err(failure)
}

/// Whether the bucket holds exactly `data` under `s3_key`: a listing of the
/// key shows it at `data`'s size, and a GET of it returns `data`. A listing
/// first, so an object that isn't there costs no 404 and none of rust-s3's
/// retry sleep; the bytes, not the ETag, since an ETag needn't be a hash of
/// the content (it isn't one under SSE-KMS). Any failure is a no.
fn object_holds(bucket: &Bucket, s3_key: &str, data: &[u8]) -> bool {
    let listed = bucket.list(s3_key.to_string(), None).is_ok_and(|pages| {
        pages
            .iter()
            .flat_map(|page| page.contents.iter())
            .any(|obj| obj.key == s3_key && obj.size == data.len() as u64)
    });
    listed
        && bucket
            .get_object(s3_key)
            .is_ok_and(|reply| reply.status_code() == 200 && reply.bytes() == data)
}

/// Upload the objects of the session's owed keys
/// ([`crate::session::sync_stamp::OwedKeys`]), each once, before a manifest
/// goes up, and clear each the bucket took. A key gone from this host, or
/// one no push sends (host-local, a pin record, not a valid key), is dropped from the
/// record; a key in `withheld` stays owed and is not sent. The first
/// transient failure stops the round, so an outage costs one request a
/// command; the next push tries again.
///
/// Returns each key that stays owed with why, for the caller to warn about.
pub fn push_owed(
    local: &LocalBackend,
    bucket: &Bucket,
    prefix: &str,
    session: &str,
    withheld: &[String],
) -> Vec<(String, String)> {
    use crate::session::sync_stamp::{clear_owed, forget_owed, sha256_hex, OwedKeys};
    let dir = local.session_dir(session);
    let owed = match OwedKeys::load(&dir) {
        Ok(owed) => owed,
        Err(e) => {
            return vec![(
                crate::session::sync_stamp::OWED_FILE.to_string(),
                format!("{e:#}"),
            )]
        }
    };
    let mut missed = Vec::new();
    let mut keys = owed.keys.into_iter();
    for (key, marked) in keys.by_ref() {
        // An entry that isn't a key (a record edited by hand) names no file
        // of the session's, and is never read.
        if crate::session::validate::validate_context_key(&key).is_err()
            || crate::session::context::is_host_local_key(&key)
            || crate::session::context::is_pin_record_key(&key)
        {
            if let Err(e) = forget_owed(&dir, &key) {
                missed.push((key, format!("{e:#}")));
            }
            continue;
        }
        if withheld.contains(&key) {
            continue;
        }
        let data = match std::fs::read(dir.join("ctx").join(&key)) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Err(e) = forget_owed(&dir, &key) {
                    missed.push((key, format!("{e:#}")));
                }
                continue;
            }
            Err(e) => {
                missed.push((key, format!("reading it failed: {e}")));
                continue;
            }
        };
        let s3_key = format!("{}/{}/ctx/{}", prefix, session, key);
        match bucket.put_object(&s3_key, &data) {
            // Cleared for the copy that went up, or for the mark as read
            // above when nothing marked the key again since: a write that
            // marked the key and then failed or stopped never wrote the
            // marked bytes, and the copy this host holds is the one that
            // counts. A mark made since, for another copy, stays.
            Ok(_) => {
                let cleared = clear_owed(&dir, &key, &sha256_hex(&data))
                    .and_then(|()| clear_owed(&dir, &key, &marked));
                if let Err(e) = cleared {
                    missed.push((key, format!("{e:#}")));
                }
            }
            Err(e) => {
                let transient = crate::session::cloud::is_transient(&e);
                missed.push((
                    key,
                    crate::session::cloud::without_url_userinfo(&e.to_string()),
                ));
                if transient {
                    break;
                }
            }
        }
    }
    missed.extend(keys.map(|(key, _)| (key, "an earlier owed upload failed first".to_string())));
    missed
}

/// [`push_owed`], each key that stays owed a warning on stderr.
pub fn push_owed_warning(
    local: &LocalBackend,
    bucket: &Bucket,
    prefix: &str,
    session: &str,
    withheld: &[String],
) {
    for (key, why) in push_owed(local, bucket, prefix, session, withheld) {
        eprintln!(
            "warning: cloud sync: context key '{}' of session '{}' is still owed to the bucket, \
             which leaves it out of the manifest until a later push uploads it: {}",
            key, session, why
        );
    }
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
///
/// The object goes up through [`put_confirmed`]. One the bucket doesn't
/// confirm is recorded owed ([`crate::session::sync_stamp::mark_owed`]) and
/// fails the push before any manifest goes up, so no manifest names it
/// (koto#646); one it confirms is cleared. The other owed keys go up next
/// ([`push_owed`]), then the manifest, which leaves out any still owed.
pub fn try_push_context_key(
    local: &LocalBackend,
    bucket: &Bucket,
    prefix: &str,
    session: &str,
    key: &str,
    withheld: &[String],
) -> anyhow::Result<()> {
    use crate::session::sync_stamp::{clear_owed, mark_owed, sha256_hex};
    if crate::session::context::is_host_local_key(key)
        || crate::session::context::is_pin_record_key(key)
        || withheld.iter().any(|w| w == key)
    {
        return Ok(());
    }
    let dir = local.session_dir(session);
    let ctx_dir = dir.join("ctx");

    // Upload the content file.
    let content_path = ctx_dir.join(key);
    if !content_path.exists() {
        return Ok(());
    }
    let data = std::fs::read(&content_path)
        .map_err(|e| anyhow::anyhow!("cloud sync: failed to read context key '{}': {}", key, e))?;
    let hash = sha256_hex(&data);
    let s3_key = format!("{}/{}/ctx/{}", prefix, session, key);
    if let Err(e) = put_confirmed(bucket, &s3_key, &data) {
        let failed = format!(
            "cloud sync failed for context upload '{}': {}",
            key,
            crate::session::cloud::without_url_userinfo(&e.to_string())
        );
        if let Err(m) = mark_owed(&dir, key, &hash) {
            anyhow::bail!("{failed}; recording the key as owed failed too: {m:#}");
        }
        anyhow::bail!("{failed}");
    }
    clear_owed(&dir, key, &hash).map_err(|e| {
        anyhow::anyhow!(
            "cloud sync: context key '{}' went up, but clearing it from the owed keys failed: \
             {:#}",
            key,
            e
        )
    })?;
    push_owed_warning(local, bucket, prefix, session, withheld);

    // Upload the manifest.
    let manifest_data = match bucket_manifest_of(&dir, withheld)
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
    crate::session::sync_stamp::note_synced(
        &local.session_dir(session),
        prefix,
        [(key, crate::session::sync_stamp::sha256_hex(&data).as_str())],
    );
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
    /// The copies differ and the local one is the newer
    /// ([`newer_locally`]), so it stands and the bucket lacks it (koto#520).
    LocalNewer,
    /// The home's manifest doesn't list the key, and this host never synced
    /// the copy it holds with the home, so the copy stands and the home
    /// lacks it (koto#524).
    LocalOnly,
}

/// What the session's listing said of the home's manifest, for a pull whose
/// fetch of it failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listing {
    /// The listing showed a manifest.
    Shows,
    /// The listing worked and showed no manifest: the home lists no key.
    Lacks,
    /// No listing to go by (the bucket unreachable, say).
    Unknown,
}

/// Whether this host's copy of a key, `local`, is the newer of two that
/// differ, against the home's copy `remote`. `synced` is the hash this host
/// last knew the home to hold ([`crate::session::sync_stamp::SyncedKeys`]).
/// A local copy still at it hasn't changed here, so the home's moved on; a
/// home still at it hasn't changed there, so this host's copy moved on.
/// Neither depends on the hosts' clocks. Only when both moved, or this host
/// never synced the key with this home, are the copies' stamps compared
/// ([`written_later`]).
pub(crate) fn newer_locally(
    local: &crate::session::context::KeyMeta,
    remote: &crate::session::context::KeyMeta,
    synced: Option<&str>,
) -> bool {
    match synced {
        Some(hash) if hash == local.hash => false,
        Some(hash) if hash == remote.hash => true,
        _ => written_later(&local.created_at, &remote.created_at),
    }
}

/// Whether `local` was written later than `remote`, both timestamps in the
/// form koto writes them (`YYYY-MM-DDTHH:MM:SS.mmmZ`, or without the
/// milliseconds). A timestamp in any other form never counts as later, so
/// the pull goes ahead as it did before.
///
/// The stamps come from the clocks of the hosts that wrote them (a pull
/// stamps the time it pulled), so two writes made on different hosts within
/// their clocks' skew can sort the wrong way round: [`newer_locally`] asks
/// this only when the synced hashes can't say.
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
/// and writes it locally with writer `sync` -- unless the local copy is the
/// newer ([`newer_locally`]), which then stands untouched
/// ([`Pull::LocalNewer`]). A session whose home moved can find an older
/// copy under its new home than the one it holds (koto#520); a write from
/// another host after this one's still comes down, whatever either host's
/// clock says (koto#524). A key the home's manifest doesn't list stands
/// too, and is [`Pull::LocalOnly`] unless this host synced that very copy
/// with the home, which then lacks it because it was removed there since.
///
/// Every outcome that finds the copies equal, or pulls, records the home's
/// hash as synced.
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
/// ([`Listing::Shows`]) that can't be fetched, a key whose hash differs and
/// whose object can't be fetched (a 404 from an object the manifest names
/// but the bucket lacks, say), or downloaded bytes that can't be written. A
/// listing that worked and showed no manifest ([`Listing::Lacks`]) is a
/// home that lists no key; no listing at all (an unreachable bucket) leaves
/// the local copy standing, [`Pull::Unchanged`], as before.
#[allow(clippy::too_many_arguments)]
pub fn pull_context_if_newer(
    local: &LocalBackend,
    bucket: &Bucket,
    prefix: &str,
    session: &str,
    key: &str,
    cache: &ManifestCache,
    listing: Listing,
    before_download: &dyn Fn() -> anyhow::Result<()>,
) -> anyhow::Result<Pull> {
    let manifest_key = format!("{}/{}/ctx/manifest.json", prefix, session);
    let remote_manifest = match fetch_remote_manifest(bucket, &manifest_key, session, cache) {
        Some(m) => m,
        None if listing == Listing::Shows => anyhow::bail!(
            "cloud sync: the bucket lists a context manifest for session '{}' but fetching it \
             failed, so the local copy of '{}' may be stale; run the command again",
            session,
            key
        ),
        None if listing == Listing::Lacks => Manifest::default(),
        None => return Ok(Pull::Unchanged),
    };

    let dir = local.session_dir(session);
    let synced = crate::session::sync_stamp::SyncedKeys::load(&dir, prefix);
    let local_manifest = local.read_manifest(session).unwrap_or_default();
    let Some(remote_meta) = remote_manifest.keys.get(key) else {
        // The home lists no such key. A copy this host synced with it was
        // removed there since; any other copy the home never had, unless
        // it is one a push leaves out.
        let lacking = local_manifest.keys.get(key).is_some_and(|meta| {
            synced.hash(key) != Some(meta.hash.as_str())
                && !crate::session::context::is_pin_record_key(key)
                && dir.join("ctx").join(key).is_file()
                && !withheld_keys(local, prefix, session)
                    .iter()
                    .any(|w| w == key)
        });
        return Ok(if lacking {
            Pull::LocalOnly
        } else {
            Pull::Unchanged
        });
    };

    // Check if local hash matches remote hash.
    if let Some(local_meta) = local_manifest.keys.get(key) {
        if local_meta.hash == remote_meta.hash {
            crate::session::sync_stamp::note_synced(
                &dir,
                prefix,
                [(key, local_meta.hash.as_str())],
            );
            return Ok(Pull::Unchanged); // Hashes match, no download needed.
        }
        if newer_locally(local_meta, remote_meta, synced.hash(key)) {
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
    crate::session::sync_stamp::note_synced(
        &dir,
        prefix,
        [(
            key,
            crate::session::sync_stamp::sha256_hex(response.bytes()).as_str(),
        )],
    );
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

    // Upload the updated manifest (the local remove already happened),
    // after the objects of any owed keys, which it leaves out while owed.
    let withheld = withheld_keys(local, prefix, session);
    push_owed_warning(local, bucket, prefix, session, &withheld);
    if let Ok(Some(data)) = bucket_manifest_of(&local.session_dir(session), &withheld) {
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

    /// koto#646: an owed key stays out of the manifest the bucket gets, on
    /// top of the host-local keys, the pin records and the withheld keys,
    /// and goes back in once cleared. A record that doesn't parse fails the
    /// build rather than read as empty.
    #[test]
    fn bucket_manifest_of_leaves_out_the_owed_keys() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(bucket_manifest_of(dir.path(), &[]).unwrap().is_none());
        std::fs::create_dir_all(dir.path().join("ctx")).unwrap();
        let entry = r#"{"created_at":"t","size":1,"hash":"h"}"#;
        std::fs::write(
            dir.path().join("ctx").join("manifest.json"),
            format!(
                r#"{{"keys":{{"a.md":{entry},"b.md":{entry},"c.md":{entry},"koto/links/pin":{entry},"workflows/publish-location":{entry}}}}}"#
            ),
        )
        .unwrap();
        let pushed = |withheld: &[String]| -> Vec<String> {
            let m: Manifest =
                serde_json::from_slice(&bucket_manifest_of(dir.path(), withheld).unwrap().unwrap())
                    .unwrap();
            m.keys.into_keys().collect()
        };
        assert_eq!(pushed(&[]), ["a.md", "b.md", "c.md"]);

        crate::session::sync_stamp::mark_owed(dir.path(), "a.md", "h").unwrap();
        assert_eq!(pushed(&[]), ["b.md", "c.md"]);
        assert_eq!(pushed(&["c.md".to_string()]), ["b.md"]);

        crate::session::sync_stamp::clear_owed(dir.path(), "a.md", "h").unwrap();
        assert_eq!(pushed(&[]), ["a.md", "b.md", "c.md"]);

        std::fs::write(
            dir.path().join(crate::session::sync_stamp::OWED_FILE),
            b"not json",
        )
        .unwrap();
        assert!(bucket_manifest_of(dir.path(), &[]).is_err());
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

    /// The synced hash says which copy moved whatever the stamps say; the
    /// stamps decide only when it can't.
    #[test]
    fn newer_locally_asks_the_synced_hash_before_the_clocks() {
        let meta = |hash: &str, created_at: &str| crate::session::context::KeyMeta {
            created_at: created_at.to_string(),
            size: 1,
            hash: hash.to_string(),
            writer: None,
        };
        // This host's clock runs ahead: its stamp is the later one.
        let local = meta("mine", "2999-01-01T00:00:00.000Z");
        let remote = meta("theirs", "2030-01-01T00:00:00.000Z");
        assert!(!newer_locally(&local, &remote, Some("mine")));
        assert!(newer_locally(&remote, &local, Some("mine")));
        // Both moved, or never synced here: the stamps.
        assert!(newer_locally(&local, &remote, Some("other")));
        assert!(newer_locally(&local, &remote, None));
        assert!(!newer_locally(&remote, &local, None));
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
