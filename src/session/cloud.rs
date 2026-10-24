//! Cloud-backed session storage using S3-compatible object storage.
//!
//! `CloudBackend` wraps `LocalBackend` so all filesystem operations happen
//! locally first (fast, works offline). After each mutating operation it
//! syncs the affected files to S3. S3 failures are non-fatal: the local
//! operation succeeds and a warning is printed to stderr.
//!
//! Context sync is per-key incremental: only the changed content file and
//! an updated manifest are transferred on each `add`/`remove`. A short TTL
//! cache on the remote manifest reduces redundant S3 GETs during rapid
//! sequential calls.
//!
//! The exception is a write a `koto next` tick's command makes to the tick's
//! own session: it goes to the local copy and a pending-writes journal, and
//! the tick flushes the whole batch once (see `crate::session::batch` and
//! [`CloudBackend::flush_pending`]).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Context;
use s3::creds::Credentials;
use s3::error::S3Error;
use s3::{Bucket, Region};

use crate::config::CloudConfig;
use crate::engine::reentrancy;
use crate::session::batch::{self, BatchLock, Journal, Pending};
use crate::session::context::{is_host_local_key, ContextStore};
use crate::session::local::{repo_id, LocalBackend};
use crate::session::sync::{self, ManifestCache};
use crate::session::sync_stamp::{self, Stamp};
use crate::session::{
    state_file_name, SessionBackend, SessionError, SessionInfo, SessionLock, SessionMigrated,
};

/// File name of the marker `koto session import` leaves beside a source
/// session's remote objects. Only `koto session prune --migrated-from`
/// deletes one, together with everything else under the source session's
/// prefix and only after the rest is gone (see
/// [`CloudBackend::delete_migrated_source`]).
pub(crate) const MIGRATED_MARKER: &str = "migrated.json";

/// File name of the compiled template a cloud session's init pushes beside
/// its state file, for `koto session import --trust-template`.
pub(crate) const TEMPLATE_OBJECT: &str = "template.json";

/// File name of the object that says a session's host holds writes the
/// bucket lacks: a batch of deferred writes is open
/// ([`crate::session::batch`]). Written with the batch's first write and
/// deleted by the flush that closes it, so `koto session import` can refuse
/// a source a stopped tick left mid-batch instead of carrying a copy without
/// those writes.
pub(crate) const BATCH_MARKER: &str = "batch-open.json";

/// Nanoseconds since the Unix epoch, which sets one writer's batch-open
/// marker apart from another's ([`CloudBackend::clear_unopened_batch`]
/// deletes only a marker whose bytes it wrote).
fn marker_nonce() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
        .to_string()
}

/// Per-child outcome emitted by [`CloudBackend::reconcile_child`].
///
/// Each variant corresponds to a concrete action or a refusal, and is
/// serialized into the JSON response body produced by `koto session
/// resolve --children`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum ChildResolution {
    /// Local and remote were identical. No action taken.
    Identical,
    /// Remote bytes extended local via the strict-prefix rule; local
    /// was updated (or the explicit `accept-remote` policy was used).
    AcceptedRemote,
    /// Local bytes extended remote via the strict-prefix rule; remote
    /// was updated (or the explicit `accept-local` policy was used).
    AcceptedLocal,
    /// The `skip` policy was applied — neither side was touched.
    Skipped,
    /// Strict-prefix classification saw divergence on both sides. A
    /// per-child `koto session resolve <child>` is required.
    Conflict,
    /// An I/O or network failure prevented reconciliation for this
    /// child. Other children still process.
    Errored {
        /// Human-readable error describing why this child could not be
        /// reconciled.
        message: String,
    },
}

/// What [`CloudBackend::remote_copy_check`] found in the bucket's copy of a
/// session, set against the local one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteCopy {
    /// No remote copy, or one that is no newer than the local copy.
    Safe,
    /// The remote copy is ahead of the local one or diverged from it; the
    /// string says which.
    NotSafe(String),
    /// The check couldn't be made: the listing failed, the local log or a
    /// version file couldn't be read, or a version file didn't parse. The
    /// string says which.
    Unreadable(String),
}

/// A migrated source copy, as [`CloudBackend::migrated_source_copy`] reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceCopy {
    /// The objects under the copy's prefix, the marker included.
    pub objects: usize,
    /// Its log's event lines, verbatim; none when it has no state file.
    pub events: Vec<String>,
}

/// What a migration marker's target holds, as
/// [`CloudBackend::marker_target_status`] finds it in the bucket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetStatus {
    /// The target carries a marker of its own: it moved on again.
    Migrated,
    /// The target's state file header names the marker's session id, and
    /// its whole log reads with no error and begins with every event of the
    /// source's log, unchanged.
    Present,
    /// A state file is there, but its header names another session id (a
    /// later session under the same name).
    Replaced(String),
    /// No state file under the target's prefix.
    Absent,
    /// The header names the marker's session id, but the copy can't be shown
    /// complete: the target's log doesn't read to the end, ends before the
    /// source's, holds a different event at some seq the source's reaches,
    /// or the source copy can't be listed or its log can't be read. The
    /// string says which.
    Unverified(String),
    /// The listing or the read failed, or the header couldn't be read; the
    /// string says which.
    Unknown(String),
}

impl TargetStatus {
    /// The word the prune prints for this status.
    pub fn as_str(&self) -> &'static str {
        match self {
            TargetStatus::Migrated => "migrated",
            TargetStatus::Present => "present",
            TargetStatus::Replaced(_) => "replaced",
            TargetStatus::Absent => "absent",
            TargetStatus::Unverified(_) => "unverified",
            TargetStatus::Unknown(_) => "unknown",
        }
    }
}

/// Whether `prefix` has the shape of a workspace prefix koto derives: 16
/// lowercase hex digits. A marker's `target.prefix` comes from the bucket,
/// so it is checked before any key is built from it.
pub(crate) fn is_workspace_prefix(prefix: &str) -> bool {
    prefix.len() == 16
        && prefix
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// S3-backed session storage that delegates to `LocalBackend` for all
/// filesystem operations and syncs state to an S3-compatible bucket.
///
/// Context operations use per-key incremental sync via the helpers in
/// `sync.rs`. A `ManifestCache` avoids redundant remote manifest GETs
/// when multiple operations happen within a short window.
pub struct CloudBackend {
    local: LocalBackend,
    bucket: Box<Bucket>,
    prefix: String,
    manifest_cache: ManifestCache,
    /// Per-process answers of [`CloudBackend::check_not_migrated`], by
    /// session id: `Some` when the session carries a migration marker.
    /// A command reads a session's state many times; the marker costs one
    /// request per session per process.
    /// Each answer is kept with whether a listing this process made (or a
    /// tick check its local copy still matches) confirmed it, rather than a
    /// read borrowing it (koto#439); a write asks again for a borrowed one.
    migration_checks: Mutex<HashMap<String, (Option<SessionMigrated>, bool)>>,
    /// Per-process answers of the local half of the migration check
    /// ([`CloudBackend::local_guard`]): `Some` when the local copy at the
    /// name is a session imported from this very workspace's copy, which a
    /// same-host import leaves behind. Each answer is kept with the
    /// identity of the state file it was read from, so a takeover that
    /// swaps the directory under a running process is noticed. Kept apart
    /// from `migration_checks`, which only the bucket's marker fills.
    /// Boxed so the backend stays small beside `LocalBackend` in `Backend`.
    local_migration_checks: Box<Mutex<HashMap<String, LocalCheck>>>,
    /// The sessions a tick has pulled in this process. A tick pulls its own
    /// session's state file once: after that, the local copy is the one its
    /// commands write to, and a second pull would only fetch what it pushed.
    /// Boxed for the same reason as `local_migration_checks`.
    pulled: Box<Mutex<HashSet<String>>>,
    /// Per-process answers of [`CloudBackend::probe`], the check window, and
    /// whether the backend only reads. Boxed for the same reason as
    /// `local_migration_checks`.
    probes: Box<Probes>,
}

/// One probe answer: the listing, and whether it was another koto
/// process's, reused for the check window, which only a read takes.
type ProbeAnswer = (sync_stamp::Probe, bool);

/// This process's listings of sessions (`crate::session::sync_stamp`).
struct Probes {
    /// By session id: one listing of the session's prefix, which the
    /// migration check, the state pull and the manifest fetch all read.
    /// `None` when the listing failed: the process doesn't ask again.
    answers: Mutex<HashMap<String, Option<ProbeAnswer>>>,
    /// How long a listing another koto process on this host made of a
    /// session stands in for a new one (`session.cloud.check_window_secs`,
    /// koto#439). Zero, which [`CloudBackend::with_parts`] leaves it at,
    /// lists in every process as before.
    window: std::time::Duration,
    /// A reader of another workspace's prefix (`koto --workspace`,
    /// [`workspace_read`]): every write to the bucket and every append to a
    /// log refuses, and no tick marker in the environment applies, since a
    /// tick in the reader's own workspace never runs a session in this
    /// prefix. Kept here, in the box, so the backend stays small.
    read_only: bool,
}

impl CloudBackend {
    /// Construct a `CloudBackend` from a working directory and cloud config.
    ///
    /// The working directory is used to derive the repo-id (same as
    /// `LocalBackend`). The cloud config provides S3 endpoint, bucket
    /// name, region, and credentials.
    pub fn new(working_dir: &Path, cloud_config: &CloudConfig) -> anyhow::Result<Self> {
        let local = LocalBackend::new()?;
        let prefix = repo_id(working_dir)?;
        let bucket = create_bucket(cloud_config)?;
        let mut backend = Self::with_parts(local, bucket, prefix);
        backend.probes.window = std::time::Duration::from_secs(cloud_config.check_window_secs());
        Ok(backend)
    }

    /// Construct a `CloudBackend` with an explicit `LocalBackend` and bucket.
    ///
    /// Intended for tests that need to control both the local storage
    /// location and the S3 bucket. Exposed to integration tests (not
    /// just unit tests) so the `tests/batch_session_resolve_test.rs`
    /// fixture can stand up a cloud backend pointed at an unreachable
    /// endpoint without duplicating internal plumbing.
    #[doc(hidden)]
    pub fn with_parts(local: LocalBackend, bucket: Box<Bucket>, prefix: String) -> Self {
        Self {
            local,
            bucket,
            prefix,
            manifest_cache: ManifestCache::new(),
            migration_checks: Mutex::new(HashMap::new()),
            local_migration_checks: Box::new(Mutex::new(HashMap::new())),
            pulled: Box::new(Mutex::new(HashSet::new())),
            probes: Box::new(Probes {
                answers: Mutex::new(HashMap::new()),
                window: std::time::Duration::ZERO,
                read_only: false,
            }),
        }
    }

    /// Refuse a write when this backend only reads another workspace's
    /// prefix (`koto --workspace`).
    fn refuse_if_read_only(&self, what: &str) -> anyhow::Result<()> {
        if self.probes.read_only {
            anyhow::bail!(
                "{} refused: this command reads workspace {} with --workspace, which never writes",
                what,
                self.prefix
            );
        }
        Ok(())
    }

    /// The session a `koto next` tick this process runs under advances
    /// ([`reentrancy::enclosing_tick`]); never one for a read-only backend,
    /// whose prefix no tick of this host's workspace runs.
    fn enclosing_tick(&self) -> Option<String> {
        if self.probes.read_only {
            return None;
        }
        reentrancy::enclosing_tick()
    }

    /// Delegate to the local store's bounded prefix read; never a remote
    /// request. See [`crate::session::Backend::read_context_prefix`].
    pub(crate) fn read_context_prefix(
        &self,
        session: &str,
        key: &str,
        bound: usize,
    ) -> anyhow::Result<Option<crate::session::PrefixRead>> {
        self.local.read_context_prefix(session, key, bound)
    }

    /// The local manifest plus the remote one, from the same cached fetch
    /// `list_keys` and `meta` use. A failed fetch is `None`, never an
    /// error: the metadata view degrades to local-only exactly as the
    /// store's own reads do.
    pub(crate) fn manifest_pair(
        &self,
        session: &str,
    ) -> (
        crate::session::context::Manifest,
        Option<crate::session::context::Manifest>,
    ) {
        let local = self.local.read_manifest(session).unwrap_or_default();
        let manifest_key = format!("{}/{}/ctx/manifest.json", self.prefix, session);
        let remote =
            sync::fetch_remote_manifest(&self.bucket, &manifest_key, session, &self.manifest_cache);
        (local, remote)
    }

    /// Refuse when `id` was imported into another workspace.
    ///
    /// The first time a process asks about `id`, the answer comes from the
    /// session's probe ([`CloudBackend::probe`]): one listing of
    /// `<prefix>/<id>/`, which the state pull and the manifest read share,
    /// so the check adds no request of its own. The listing answers 200
    /// whether or not the marker is there, so a session that was never
    /// migrated costs no retry (a GET of a missing object is a 404, which
    /// rust-s3 retries after a one-second sleep). Only when the listing
    /// shows the marker is its body fetched. The answer is remembered for
    /// the process.
    ///
    /// Inside a tick that already checked `id` (its stamp names the tick,
    /// and the local state file is still the stamped bytes), the bucket
    /// isn't asked again: the marker was absent when the tick checked, and
    /// an import another host makes later in the tick is refused at the
    /// tick's push, which asks the bucket uncached before it writes
    /// ([`CloudBackend::flush_pending`]). The stamp is also written by this
    /// host's own push of `id`, after which the bucket holds what this host
    /// wrote.
    ///
    /// A marker returns [`SessionMigrated`] naming the session it was
    /// imported as and that session's workspace. A listing that fails means
    /// the check can't tell: it warns and lets the read proceed, so an
    /// unreachable bucket doesn't stop a host from working on its local
    /// copy.
    ///
    /// A marker whose body can't be read or parsed still refuses: its
    /// presence is the signal, and a marker written by a newer koto must
    /// not fork the session on an older one.
    pub fn check_not_migrated(&self, id: &str) -> anyhow::Result<()> {
        self.check_migration(id, false)
    }

    /// [`CloudBackend::check_not_migrated`] for a write. Its answer comes
    /// from a listing this process made, or from a tick that checked `id`
    /// and whose local copy is still what it checked, as every write's has;
    /// never from a check another process shared or from the tick's batch
    /// standing in for the copy it checked. Those shortcuts are for reads
    /// (koto#439): a write on a borrowed answer could reach a session
    /// imported elsewhere in the meantime.
    fn check_not_migrated_to_write(&self, id: &str) -> anyhow::Result<()> {
        self.check_migration(id, true)
    }

    /// Before a push of `id`'s state file on its own: when this process's
    /// migration answer for `id` was borrowed by a read, confirm it with a
    /// listing first. A process holding no answer, or a confirmed one, is
    /// unchanged by this.
    fn confirm_borrowed_migration_check(&self, id: &str) -> anyhow::Result<()> {
        let borrowed = self
            .migration_checks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .is_some_and(|(_, confirmed)| !confirmed);
        if borrowed {
            self.check_not_migrated_to_write(id)?;
        }
        Ok(())
    }

    /// The migration check behind [`CloudBackend::check_not_migrated`] and
    /// [`CloudBackend::check_not_migrated_to_write`]. Each process keeps its
    /// answer per session with whether a listing of its own confirmed it; a
    /// write asks again when the answer it finds was borrowed.
    fn check_migration(&self, id: &str, write: bool) -> anyhow::Result<()> {
        self.local_guard(id)?;
        let mut checks = self
            .migration_checks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let cached = checks
            .get(id)
            .filter(|(_, confirmed)| *confirmed || !write)
            .map(|(answer, _)| answer.clone());
        let answer = match cached {
            Some(answer) => answer,
            None => {
                // A tick that already checked `id` saw no marker; otherwise
                // the session's probe says, in the listing the state pull
                // and the manifest read take their ETags from.
                let stamp = self.stamp(id);
                let (checked, confirmed) = if self.tick_fresh(id, stamp.as_ref(), false) {
                    (Ok(None), true)
                } else if !write && self.tick_fresh(id, stamp.as_ref(), true) {
                    (Ok(None), false)
                } else {
                    match self.probe_for(id, !write) {
                        Ok((probe, shared)) => (self.marker_in(id, &probe), !shared),
                        Err(e) => (Err(e), true),
                    }
                };
                let answer = match checked {
                    Ok(answer) => answer,
                    Err(e) => {
                        eprintln!(
                            "warning: cloud sync: migration check failed: {}",
                            without_url_userinfo(&format!("{:#}", e))
                        );
                        None
                    }
                };
                checks.insert(id.to_string(), (answer.clone(), confirmed));
                answer
            }
        };
        match answer {
            Some(migrated) => Err(anyhow::Error::new(migrated)),
            None => Ok(()),
        }
    }

    /// [`CloudBackend::read_migration_marker_of`] through the session's
    /// probe: the marker's body is fetched only when the listing shows it.
    fn marker_in(
        &self,
        id: &str,
        probe: &sync_stamp::Probe,
    ) -> anyhow::Result<Option<SessionMigrated>> {
        if !probe.marker {
            return Ok(None);
        }
        Ok(match self.fetch_object(&self.marker_key(id)) {
            Ok(Some(bytes)) => Some(migrated_from_marker(id, &bytes)),
            Ok(None) => None,
            Err(_) => Some(migrated_from_marker(id, b"")),
        })
    }

    /// Ask the bucket, uncached, whether `id` carries a migration marker.
    /// `Err` when the listing failed and the answer is unknown.
    fn read_migration_marker_of(&self, id: &str) -> anyhow::Result<Option<SessionMigrated>> {
        let key = self.marker_key(id);
        if !self.object_listed(&key)? {
            return Ok(None);
        }
        Ok(match self.fetch_object(&key) {
            Ok(Some(bytes)) => Some(migrated_from_marker(id, &bytes)),
            // Removed between the listing and the GET.
            Ok(None) => None,
            Err(_) => Some(migrated_from_marker(id, b"")),
        })
    }

    /// The local half of the migration check, which never makes a request.
    ///
    /// On one host every workspace shares the local store, so a same-host
    /// import leaves the imported session at the name the source's copy
    /// used. Two things refuse here, before any pull could write over the
    /// other side's copy:
    ///
    /// - a `.takeover-<id>` directory in the store: an import of `id` is
    ///   between its push and its last step (or stopped there), and neither
    ///   workspace may read or write the name until it is run again;
    /// - a local copy whose latest `session_imported` event names this
    ///   backend's own workspace and this name as its source: the copy is
    ///   the session this workspace's copy was imported into, so this
    ///   workspace refuses with `session_migrated` even when the bucket's
    ///   marker can't be listed.
    ///
    /// The second answer is cached per process, like the marker's.
    pub fn local_guard(&self, id: &str) -> anyhow::Result<()> {
        let pending = takeover_dir(self.local.base_dir(), id);
        if std::fs::symlink_metadata(&pending).is_ok() {
            anyhow::bail!(
                "session '{}' is being imported on this host and the import hasn't finished \
                 ({} exists); run the same `koto session import {}` again from the importing \
                 workspace to finish it",
                id,
                pending.display(),
                id
            );
        }
        let file = file_identity(&self.local.session_dir(id).join(state_file_name(id)));
        let mut checks = self
            .local_migration_checks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let answer = match checks.get(id) {
            Some((seen, answer)) if *seen == file => answer.clone(),
            _ => {
                let answer = self.local_import_from_here(id);
                checks.insert(id.to_string(), (file, answer.clone()));
                answer
            }
        };
        match answer {
            Some(migrated) => Err(anyhow::Error::new(migrated)),
            None => Ok(()),
        }
    }

    /// `Some` when the local copy at `id` is an import of this workspace's
    /// own copy of `id`.
    fn local_import_from_here(&self, id: &str) -> Option<SessionMigrated> {
        let bytes = std::fs::read(self.local.session_dir(id).join(state_file_name(id))).ok()?;
        let imported = ImportedAs::read(&bytes)?;
        if imported.from_session != id {
            return None;
        }
        let from = crate::session::local::prefix_for_workspace(Path::new(&imported.from_workspace))
            .ok()?;
        if from != self.prefix {
            return None;
        }
        let workspace = header_anchor(&bytes).unwrap_or_else(|| "another workspace".to_string());
        Some(SessionMigrated {
            name: id.to_string(),
            target: id.to_string(),
            workspace,
        })
    }

    /// S3 key of a session's migration marker under this backend's prefix.
    fn marker_key(&self, id: &str) -> String {
        format!("{}{}", self.session_prefix(id), MIGRATED_MARKER)
    }

    // -----------------------------------------------------------------
    // Freshness (koto#428): see `crate::session::sync_stamp`.
    // -----------------------------------------------------------------

    /// S3 key of a session's context manifest.
    fn manifest_key(&self, id: &str) -> String {
        format!("{}ctx/manifest.json", self.session_prefix(id))
    }

    /// One listing of `id`'s prefix: whether the migration marker is there,
    /// and the bucket's ETags for the state file and the manifest. Asked
    /// once per session per process; [`CloudBackend::forget_probe`] drops
    /// the answer after this process writes the session.
    ///
    /// An object listed without an ETag reads as present with an empty one,
    /// which no stamp matches, so it is always fetched. A listing that
    /// failed fails every later probe of `id` in the process without a
    /// request: an unreachable bucket costs a process one failed listing,
    /// not one per read.
    fn probe(&self, id: &str) -> anyhow::Result<sync_stamp::Probe> {
        self.probe_for(id, true).map(|(probe, _)| probe)
    }

    /// [`CloudBackend::probe`], with whether the answer was another koto
    /// process's listing, reused for the check window (koto#439). Only a
    /// read takes a shared answer (`allow_shared`); a write's probe is a
    /// listing this process made, as it always was.
    fn probe_for(&self, id: &str, allow_shared: bool) -> anyhow::Result<ProbeAnswer> {
        if let Some(answer) = self
            .probes
            .answers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .filter(|answer| allow_shared || !answer.as_ref().is_some_and(|(_, shared)| *shared))
        {
            return answer.clone().ok_or_else(|| {
                anyhow::anyhow!("listing {} failed earlier", self.session_prefix(id))
            });
        }
        // Another koto process on this host listed the session within the
        // check window: its answer stands in for a new listing (koto#439).
        let dir = self.local.session_dir(id);
        let now = std::time::SystemTime::now();
        if allow_shared {
            if let Some(shared) = sync_stamp::load_check(
                &dir,
                &self.bucket.url(),
                &self.state_key(id),
                self.probes.window,
                now,
            ) {
                self.probes
                    .answers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(id.to_string(), Some((shared.clone(), true)));
                return Ok((shared, true));
            }
        }
        let prefix = self.session_prefix(id);
        let results = match self.bucket.list(prefix.clone(), None) {
            Ok(results) => results,
            Err(e) => {
                self.probes
                    .answers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(id.to_string(), None);
                anyhow::bail!("listing {} failed: {}", prefix, e);
            }
        };
        let (marker, state, manifest) = (
            self.marker_key(id),
            self.state_key(id),
            self.manifest_key(id),
        );
        let mut probe = sync_stamp::Probe::default();
        for obj in results.iter().flat_map(|page| page.contents.iter()) {
            let etag = || {
                Some(
                    obj.e_tag
                        .as_deref()
                        .map(sync_stamp::normalize_etag)
                        .unwrap_or_default(),
                )
            };
            if obj.key == marker {
                probe.marker = true;
            } else if obj.key == state {
                probe.state_etag = etag();
            } else if obj.key == manifest {
                probe.manifest_etag = etag();
            }
        }
        if !self.probes.window.is_zero() {
            sync_stamp::save_check(&dir, &self.bucket.url(), &self.state_key(id), &probe, now);
        }
        self.probes
            .answers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string(), Some((probe.clone(), false)));
        Ok((probe, false))
    }

    /// Start a tick of `id`: write the id its commands read as the current
    /// tick ([`CloudBackend::current_tick`]). Nothing is written for a
    /// session the local guard refuses (an import of it is pending, or it is
    /// the other side of a same-host import): the tick refuses it before
    /// touching it.
    pub fn begin_tick(&self, id: &str) {
        if self.local_guard(id).is_ok() {
            sync_stamp::begin_tick(&self.local.session_dir(id));
        }
    }

    /// Drop this process's probe of `id`, and the check this host's other
    /// processes would reuse, after a write that changed what the bucket
    /// holds for it.
    fn forget_probe(&self, id: &str) {
        self.probes
            .answers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        sync_stamp::forget_check(&self.local.session_dir(id));
    }

    /// After this host pushed `id`'s manifest: drop the probe, and forget the
    /// manifest cached beside the session, so no later read, in this process
    /// or another command of the same tick, takes the manifest from before
    /// the push. The next read fetches the manifest again.
    fn forget_remote_manifest(&self, id: &str) {
        self.forget_probe(id);
        if let Some(mut stamp) = self.stamp(id) {
            if stamp.manifest_etag.take().is_some() {
                stamp.save(&self.local.session_dir(id));
            }
        }
    }

    /// `id`'s stamp, when it is for `id`'s state key.
    fn stamp(&self, id: &str) -> Option<Stamp> {
        Stamp::load(&self.local.session_dir(id)).filter(|s| s.state_key == self.state_key(id))
    }

    /// The id of the tick this process runs under, read from the ticked
    /// session's directory; `None` outside a tick.
    fn current_tick(&self) -> Option<String> {
        let tick = self.enclosing_tick()?;
        sync_stamp::tick_id(&self.local.session_dir(&tick))
    }

    /// Whether the current tick already checked `id` against the bucket and
    /// its local state file is still the bytes it checked: the tick is the
    /// freshness window, so no request is made.
    ///
    /// With `batch_counts`, which only a read passes, a command of `id`'s own
    /// tick whose batch is open is fresh too: the tick's own writes, kept in
    /// that batch, are what changed the local copy, not the bucket, and the
    /// tick checks the bucket again before it pushes them (koto#439). A
    /// write doesn't pass it, so each write still checks for a migration
    /// marker that landed during the tick.
    fn tick_fresh(&self, id: &str, stamp: Option<&Stamp>, batch_counts: bool) -> bool {
        let Some(stamp) = stamp else {
            return false;
        };
        let Some(tick) = self.current_tick() else {
            return false;
        };
        if stamp.verified_tick.as_deref() != Some(tick.as_str()) {
            return false;
        }
        if batch_counts && self.defers(id) && self.journal(id).is_open() {
            return true;
        }
        let path = self.local.session_dir(id).join(state_file_name(id));
        std::fs::read(path).is_ok_and(|bytes| stamp.matches(&bytes))
    }

    /// Stamp `id` with the current tick, after a probe found its local copy
    /// current.
    fn mark_verified(&self, id: &str, mut stamp: Stamp) {
        stamp.verified_tick = self.current_tick();
        stamp.save(&self.local.session_dir(id));
    }

    /// Stamp `id` as holding `bytes`, which the bucket holds under `etag`:
    /// after a pull or a push of its state file.
    fn record_synced(&self, id: &str, etag: Option<&str>, bytes: &[u8]) {
        let dir = self.local.session_dir(id);
        let previous = self.stamp(id);
        let stamp = match etag.filter(|e| !e.is_empty()) {
            Some(etag) => {
                let mut stamp =
                    Stamp::for_bytes(previous.as_ref(), &self.state_key(id), etag, bytes);
                stamp.verified_tick = self.current_tick();
                stamp
            }
            // No ETag to compare with: drop the stamp, so the next read
            // fetches the state file.
            None => Stamp {
                state_key: self.state_key(id),
                manifest_etag: previous.and_then(|p| p.manifest_etag),
                ..Stamp::default()
            },
        };
        stamp.save(&dir);
    }

    /// PUT `id`'s state file and stamp what went up.
    fn put_state(&self, id: &str, data: &[u8]) -> anyhow::Result<()> {
        let key = self.state_key(id);
        self.refuse_if_read_only(&format!("S3 PUT of {}", key))?;
        let response = self
            .bucket
            .put_object(&key, data)
            .with_context(|| format!("S3 PUT failed for key: {}", key))?;
        self.forget_probe(id);
        self.record_synced(id, response_etag(&response).as_deref(), data);
        Ok(())
    }

    /// Put the remote manifest of `id` in this process's manifest cache
    /// before a context read consults it, from the copy cached beside the
    /// session when the bucket's manifest is still the one cached, so a
    /// read whose local copy is current fetches no manifest. Inside a tick
    /// that already checked `id`, the cached copy is used without asking
    /// the bucket. When the bucket has no manifest, an empty one is cached,
    /// so the read asks for nothing more. Anything that fails here leaves
    /// the cache empty, and the read fetches the manifest as it always has.
    fn prime_manifest(&self, id: &str) {
        if self.manifest_cache.get(id).is_some() {
            return;
        }
        let dir = self.local.session_dir(id);
        let stamp = self.stamp(id);
        let cached = |etag: &str| -> Option<crate::session::context::Manifest> {
            if etag.is_empty() || stamp.as_ref()?.manifest_etag.as_deref() != Some(etag) {
                return None;
            }
            let mut manifest: crate::session::context::Manifest =
                serde_json::from_slice(&sync_stamp::load_manifest(&dir)?).ok()?;
            manifest.drop_host_local();
            Some(manifest)
        };
        if self.tick_fresh(id, stamp.as_ref(), true) {
            if let Some(etag) = stamp.as_ref().and_then(|s| s.manifest_etag.clone()) {
                if let Some(manifest) = cached(&etag) {
                    self.manifest_cache.set(id, manifest);
                    return;
                }
            }
        }
        let Ok(probe) = self.probe(id) else {
            return;
        };
        let Some(etag) = probe.manifest_etag else {
            self.manifest_cache
                .set(id, crate::session::context::Manifest::default());
            return;
        };
        if let Some(manifest) = cached(&etag) {
            self.manifest_cache.set(id, manifest);
            return;
        }
        let Ok(response) = self.bucket.get_object(self.manifest_key(id)) else {
            return;
        };
        if response.status_code() != 200 {
            return;
        }
        let Ok(mut manifest) =
            serde_json::from_slice::<crate::session::context::Manifest>(response.bytes())
        else {
            return;
        };
        sync_stamp::save_manifest(&dir, response.bytes());
        let mut stamp = stamp.unwrap_or_else(|| Stamp {
            state_key: self.state_key(id),
            ..Stamp::default()
        });
        // The listing's ETag, which the next probe is compared with.
        stamp.manifest_etag = Some(etag);
        stamp.save(&dir);
        manifest.drop_host_local();
        self.manifest_cache.set(id, manifest);
    }

    /// Whether an object exists, asked by listing with its key as the
    /// prefix rather than by HEAD or GET.
    ///
    /// rust-s3 is built with `fail-on-err`, so a missing object's 404
    /// arrives as an `Err`, and the crate retries every `Err` once after a
    /// one-second sleep. A listing answers 200 with zero or one matching
    /// key either way, so asking about an object that is expected to be
    /// absent costs one request and no sleep. `Err` means the listing
    /// itself failed: the caller can't tell either way.
    fn object_listed(&self, key: &str) -> anyhow::Result<bool> {
        let results = self
            .bucket
            .list(key.to_string(), None)
            .map_err(|e| anyhow::anyhow!("listing {} failed: {}", key, e))?;
        Ok(results
            .iter()
            .any(|page| page.contents.iter().any(|obj| obj.key == key)))
    }

    /// GET an object expected to exist, telling a missing one (`Ok(None)`)
    /// apart from a request that failed (`Err`). Only a 404 counts as
    /// missing. Callers that expect the object may be absent probe with
    /// [`CloudBackend::object_listed`] first, or use
    /// [`CloudBackend::get_object_if_present`], so that no expected 404
    /// pays rust-s3's retry sleep.
    fn fetch_object(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        match self.bucket.get_object(key) {
            Ok(response) => match response.status_code() {
                200 => Ok(Some(response.bytes().to_vec())),
                404 => Ok(None),
                status => Err(anyhow::anyhow!("GET {} returned status {}", key, status)),
            },
            Err(e) if is_not_found(&e) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("GET {} failed: {}", key, e)),
        }
    }

    /// Fetch an object that may well be absent: list for it, and GET it
    /// only when the listing shows it.
    fn get_object_if_present(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        if !self.object_listed(key)? {
            return Ok(None);
        }
        self.fetch_object(key)
    }

    /// The local directory sessions are stored under. Reached through
    /// `Backend::local_base_dir`; see its docs for why a cloud backend has
    /// one at all.
    pub(crate) fn local_base_dir(&self) -> &Path {
        self.local.base_dir()
    }

    /// The run journal root of the local store this backend keeps its
    /// copies in. Reached through `Backend::journal_root`.
    pub(crate) fn journal_root(&self) -> Option<&Path> {
        self.local.journal_root()
    }

    /// S3 key for a session's state file.
    fn state_key(&self, id: &str) -> String {
        format!("{}/{}/{}", self.prefix, id, state_file_name(id))
    }

    /// S3 key prefix for a session (all artifacts).
    fn session_prefix(&self, id: &str) -> String {
        format!("{}/{}/", self.prefix, id)
    }

    /// Upload the state file to S3. Non-fatal on failure.
    ///
    /// With a batch open, the upload is the batch's flush, which pushes the
    /// state file after the keys its events name. A flush that fails leaves
    /// the state file unpushed too: on its own it would name keys the
    /// bucket doesn't hold.
    fn sync_push_state(&self, id: &str) {
        if let Err(e) = self.strict_push_state(id) {
            if is_hard_sync_error(&e) {
                eprintln!("error: cloud sync: {:#}", e);
            } else {
                eprintln!("warning: cloud sync failed for state upload: {:#}", e);
            }
        }
    }

    /// Strict variant of [`sync_push_state`] that surfaces the `Result`.
    ///
    /// Used by [`CloudBackend::ensure_pushed`] to enforce "push parent
    /// before child mutation" ordering (Decision 12 Q6). Callers that
    /// cannot tolerate a swallowed S3 error must use this path; the
    /// best-effort `sync_push_state` is retained for the single-writer
    /// happy path.
    fn strict_push_state(&self, id: &str) -> anyhow::Result<()> {
        self.local_guard(id)?;
        // An open batch is pushed whole, from any process: a state file
        // pushed on its own would name keys the bucket doesn't hold, and a
        // caller of `ensure_pushed` needs the bucket to have everything,
        // even when it is a command the tick that owns the batch runs.
        if self.journal(id).is_open() {
            return self.flush_batch(id);
        }
        // No batch is open, so this push goes up on its own; a marker a
        // stopped opener left would still make imports refuse. A migration
        // answer this process only borrowed for a read is confirmed first.
        self.confirm_borrowed_migration_check(id)?;
        self.try_clear_unopened_batch(id);
        let state_path = self.local.session_dir(id).join(state_file_name(id));
        if !state_path.exists() {
            // Nothing to push; mirrors the old no-op behavior. Callers
            // relying on ordering ensure the append happened before the
            // probe, so this path is reachable only in degenerate tests.
            return Ok(());
        }
        let data = std::fs::read(&state_path)
            .with_context(|| format!("reading local state file: {}", state_path.display()))?;
        self.put_state(id, &data)
    }

    /// S3 key of a session's compiled template.
    fn template_key(&self, id: &str) -> String {
        format!("{}{}", self.session_prefix(id), TEMPLATE_OBJECT)
    }

    /// Upload the compiled template a session was initialized with, as
    /// `template.json`, so `koto session import --trust-template` on
    /// another host can take it. Non-fatal on failure, like every other
    /// push: the import then needs the template compiled on its host.
    ///
    /// `template_path` is the one the `workflow_initialized` event records,
    /// resolved against the session directory the way every state reader
    /// resolves it.
    fn push_template(&self, id: &str, template_path: &str) {
        let session_dir = self.local.session_dir(id);
        let path = crate::engine::persistence::resolve_template_path_in_session(
            template_path,
            &session_dir,
        );
        let result = std::fs::read(&path)
            .with_context(|| format!("reading compiled template {}", path))
            .and_then(|bytes| self.put_object(&self.template_key(id), &bytes));
        if let Err(e) = result {
            eprintln!("warning: cloud sync failed for template upload: {:#}", e);
        }
    }

    /// Before a context key is downloaded over the local copy: the bucket's
    /// state file for `session` must not be the other side of an import of
    /// the local one ([`refuse_cross_import_pull`]). A state file the bucket
    /// can't serve lets the pull go on, as the state pull does.
    ///
    /// Unlike the local guards ([`CloudBackend::local_guard`]), which make no
    /// request, this check reads the bucket's state file: the identity it
    /// compares lives there. It is asked only when a download is already due,
    /// after the manifests' hashes differ, so a read whose hashes match makes
    /// no extra request, and the cross-host carrier's recorded requests are
    /// unchanged.
    fn check_context_pull(&self, session: &str) -> anyhow::Result<()> {
        let state_path = self
            .local
            .session_dir(session)
            .join(state_file_name(session));
        match self.bucket.get_object(self.state_key(session)) {
            Ok(response) if response.status_code() == 200 => {
                refuse_cross_import_pull(session, &state_path, response.bytes())
            }
            _ => Ok(()),
        }
    }

    /// Download the state file from S3 to the local session directory.
    /// Non-fatal on failure: if S3 is unreachable, local state is used as-is.
    ///
    /// The one error is [`refuse_cross_import_pull`]'s: the bucket's copy
    /// and the local one are the two sides of an import, which only a
    /// same-host import can bring together, and writing one over the other
    /// would destroy a session.
    ///
    /// Four cases make no request. A command a tick on `id` runs reads the
    /// local copy as it is: the tick pulled it, and the writes it and its
    /// commands made since are there and not yet in the bucket. A tick
    /// pulls its own session once per process. A command of a tick that
    /// already checked `id` reads the local copy as that check left it
    /// (`crate::session::sync_stamp`). And a pull never writes over an open
    /// batch: the batch is flushed instead, after which the local copy is
    /// the bucket's; a flush that fails leaves the local copy as it is, and
    /// refuses when the failure is a migration marker or a version conflict.
    ///
    /// Otherwise the session's probe lists its prefix, and the state file is
    /// fetched only when the bucket's ETag moved since this host's stamp, or
    /// the local file is no longer the stamped bytes.
    ///
    /// A batch begun and never opened (its process killed, or its marker
    /// PUT failed) is cleared first ([`CloudBackend::clear_unopened_batch`]):
    /// the only requests a pull adds, and only when such a batch is there.
    fn sync_pull_state(&self, id: &str) -> anyhow::Result<()> {
        if self.defers(id) {
            return Ok(());
        }
        let own_tick =
            reentrancy::is_tick_process() && self.enclosing_tick().as_deref() == Some(id);
        self.try_clear_unopened_batch(id);
        if self.journal(id).is_open() {
            match self.flush_pending(id) {
                Ok(()) => {}
                Err(e) if is_hard_sync_error(&e) => return Err(e),
                Err(e) => {
                    eprintln!(
                        "warning: cloud sync: kept this host's unpushed writes and skipped the \
                         pull: {:#}",
                        e
                    );
                    return Ok(());
                }
            }
            if own_tick {
                self.mark_pulled(id);
            }
            return Ok(());
        }
        if own_tick && !self.mark_pulled(id) {
            return Ok(());
        }
        // The state file is fetched only when the bucket's copy moved since
        // this host last pulled or pushed it, or the local file is no longer
        // what it pulled or pushed (`crate::session::sync_stamp`). Inside a
        // tick that already checked `id`, the bucket isn't asked at all.
        let stamp = self.stamp(id);
        if self.tick_fresh(id, stamp.as_ref(), true) {
            return Ok(());
        }
        let remote_etag = match self.probe(id) {
            Ok(probe) => match probe.state_etag {
                Some(etag) => etag,
                None => return Ok(()), // Not in the bucket: use local.
            },
            Err(e) => {
                eprintln!("warning: cloud sync pull failed: {}", e);
                return Ok(());
            }
        };
        let state_path = self.local.session_dir(id).join(state_file_name(id));
        if let Some(stamp) = stamp {
            if !remote_etag.is_empty()
                && stamp.state_etag.as_deref() == Some(remote_etag.as_str())
                && std::fs::read(&state_path).is_ok_and(|local| stamp.matches(&local))
            {
                self.mark_verified(id, stamp);
                return Ok(());
            }
        }
        let key = self.state_key(id);
        match self.bucket.get_object(&key) {
            Ok(response) if response.status_code() == 200 => {
                refuse_cross_import_pull(id, &state_path, response.bytes())?;
                if let Err(e) = std::fs::write(&state_path, response.bytes()) {
                    eprintln!("warning: cloud sync: failed to write pulled state: {}", e);
                } else {
                    self.record_synced(id, Some(&remote_etag), response.bytes());
                }
            }
            Ok(_) => {} // Not found or other status, use local
            Err(e) => {
                eprintln!("warning: cloud sync pull failed: {}", e);
            }
        }
        Ok(())
    }

    /// Delete all objects under a session's S3 prefix except its migration
    /// marker. Non-fatal on failure.
    ///
    /// The marker stays because it is what makes every other host refuse
    /// the session once it has been imported elsewhere: a cleanup or a
    /// terminal tick on the old host must not be able to lift it.
    fn sync_delete_session(&self, id: &str) {
        let prefix = self.session_prefix(id);
        // List and delete objects under the prefix.
        match self.bucket.list(prefix.clone(), None) {
            Ok(results) => {
                for list in &results {
                    for obj in &list.contents {
                        if is_migration_marker(&prefix, &obj.key) {
                            continue;
                        }
                        if let Err(e) = self.delete_object(&obj.key) {
                            eprintln!("warning: cloud sync: failed to delete {}: {}", obj.key, e);
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "warning: cloud sync: failed to list prefix {}: {}",
                    prefix, e
                );
            }
        }
    }

    /// [`CloudBackend::sync_delete_session`] for a caller that must know
    /// whether the remote copy is gone: every object under the session's
    /// prefix except its migration marker is deleted, and the first failure
    /// (the listing, or any delete) is returned after the rest were tried.
    /// `koto session prune` removes the local copy only after this succeeds,
    /// so a failure leaves something a later prune can retry.
    pub fn delete_remote_session_checked(&self, id: &str) -> anyhow::Result<()> {
        let prefix = self.session_prefix(id);
        let results = self.bucket.list(prefix.clone(), None).map_err(|e| {
            anyhow::anyhow!(
                "listing {} failed: {}",
                prefix,
                without_url_userinfo(&e.to_string())
            )
        })?;
        let mut first: Option<anyhow::Error> = None;
        for list in &results {
            for obj in &list.contents {
                if is_migration_marker(&prefix, &obj.key) {
                    continue;
                }
                if let Err(e) = self.delete_object(&obj.key) {
                    first.get_or_insert_with(|| {
                        anyhow::anyhow!(
                            "deleting {} failed: {}",
                            obj.key,
                            without_url_userinfo(&e.to_string())
                        )
                    });
                }
            }
        }
        match first {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Whether the bucket's copy of session `id` may hold anything its local
    /// copy doesn't. One listing of the session's prefix answers two
    /// questions. A remote state log longer than the local one has events
    /// this host never saw (another host advanced or rewound the session).
    /// And the two `version.json` files, compared with
    /// [`crate::session::version::check_sync`], show a context write this
    /// host never saw. A remote copy with neither is no newer than the local
    /// one. Anything that can't be read, or a remote version the local copy
    /// has nothing to compare with, can't be shown to be no newer, so it
    /// isn't `Safe`.
    pub fn remote_copy_check(&self, id: &str) -> RemoteCopy {
        use crate::session::version::{check_sync, SessionVersion, SyncCheck};

        let prefix = self.session_prefix(id);
        let listing = match self.bucket.list(prefix.clone(), None) {
            Ok(listing) => listing,
            Err(e) => {
                return RemoteCopy::Unreadable(format!(
                    "listing {} failed: {}",
                    prefix,
                    without_url_userinfo(&e.to_string())
                ))
            }
        };
        let size_of = |key: &str| {
            listing
                .iter()
                .flat_map(|page| page.contents.iter())
                .find(|obj| obj.key == key)
                .map(|obj| obj.size)
        };

        if let Some(remote_len) = size_of(&self.state_key(id)) {
            let local_path = self.local.session_dir(id).join(state_file_name(id));
            let local_len = match std::fs::metadata(&local_path) {
                Ok(meta) => meta.len(),
                Err(e) => {
                    return RemoteCopy::Unreadable(format!("the local log can't be read: {e}"))
                }
            };
            if remote_len > local_len {
                return RemoteCopy::NotSafe(format!(
                    "the remote log is ahead ({remote_len} bytes against {local_len} here)"
                ));
            }
        }

        let version_key = self.version_key(id);
        if size_of(&version_key).is_none() {
            return RemoteCopy::Safe;
        }
        let remote = match self.fetch_object(&version_key) {
            // Removed between the listing and the GET.
            Ok(None) => return RemoteCopy::Safe,
            Ok(Some(bytes)) => match serde_json::from_slice::<SessionVersion>(&bytes) {
                Ok(v) => v,
                Err(e) => {
                    return RemoteCopy::Unreadable(format!(
                        "the remote version.json doesn't parse: {e}"
                    ))
                }
            },
            Err(e) => {
                return RemoteCopy::Unreadable(without_url_userinfo(&format!("{e:#}")));
            }
        };
        let local = match SessionVersion::load(&self.local_version_path(id)) {
            Ok(Some(v)) => v,
            Ok(None) => {
                return RemoteCopy::NotSafe(
                    "the remote copy has a version and the local copy has none".to_string(),
                )
            }
            Err(e) => {
                return RemoteCopy::Unreadable(format!(
                    "the local version.json can't be read: {e:#}"
                ))
            }
        };
        match check_sync(&local, Some(&remote)) {
            SyncCheck::Safe => RemoteCopy::Safe,
            SyncCheck::RemoteNewer => RemoteCopy::NotSafe(format!(
                "the remote copy is ahead (version {} against local {})",
                remote.version, local.version
            )),
            SyncCheck::Conflict {
                local_version,
                remote_version,
                ..
            } => RemoteCopy::NotSafe(format!(
                "the remote copy diverged (version {remote_version} against local {local_version})"
            )),
        }
    }

    /// The names of the sessions under workspace prefix `source_prefix` that
    /// carry a migration marker. One paginated listing of `<prefix>/`
    /// collects exactly the keys `<prefix>/<name>/migrated.json` whose
    /// `<name>` is a valid session id; a context key named `migrated.json`
    /// sits deeper and is not one. Sorted.
    pub(crate) fn migration_marker_names(
        &self,
        source_prefix: &str,
    ) -> anyhow::Result<Vec<String>> {
        let listing_prefix = format!("{}/", source_prefix);
        let suffix = format!("/{}", MIGRATED_MARKER);
        let results = self
            .bucket
            .list(listing_prefix.clone(), None)
            .map_err(|e| {
                anyhow::anyhow!(
                    "listing {} failed: {}",
                    listing_prefix,
                    without_url_userinfo(&e.to_string())
                )
            })?;
        let mut names = std::collections::BTreeSet::new();
        for page in &results {
            for obj in &page.contents {
                let Some(name) = obj
                    .key
                    .strip_prefix(&listing_prefix)
                    .and_then(|rest| rest.strip_suffix(&suffix))
                else {
                    continue;
                };
                if crate::session::validate::validate_session_id(name).is_ok() {
                    names.insert(name.to_string());
                }
            }
        }
        Ok(names.into_iter().collect())
    }

    /// GET the migration marker of session `name` under `source_prefix`:
    /// `Ok(None)` when it is gone.
    pub(crate) fn read_migration_marker(
        &self,
        source_prefix: &str,
        name: &str,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        crate::session::validate::validate_session_id(name)?;
        self.fetch_object(&format!("{}/{}/{}", source_prefix, name, MIGRATED_MARKER))
            .map_err(|e| anyhow::anyhow!("{}", without_url_userinfo(&format!("{e:#}"))))
    }

    /// What the bucket holds of the migrated source copy `<source_prefix>/<name>/`:
    /// how many objects a removal would delete, the marker included, and the
    /// event lines of its log. The log is read to the end under the import's
    /// own rule (no unparseable line, a final one included, and no seq gap).
    /// A copy with no state file left has no events to lose: only a removal
    /// that stopped part way leaves it so, because the import never deletes
    /// the source's log and a removal deletes the marker last, and a retry
    /// must be able to finish. `Err` says why the copy can't be read.
    pub(crate) fn migrated_source_copy(
        &self,
        source_prefix: &str,
        name: &str,
    ) -> Result<SourceCopy, String> {
        crate::session::validate::validate_session_id(name).map_err(|e| format!("{e:#}"))?;
        let session = format!("{}/{}/", source_prefix, name);
        let listing = self.bucket.list(session.clone(), None).map_err(|e| {
            format!(
                "listing {} failed: {}",
                session,
                without_url_userinfo(&e.to_string())
            )
        })?;
        let keys: Vec<&str> = listing
            .iter()
            .flat_map(|page| page.contents.iter())
            .map(|obj| obj.key.as_str())
            .filter(|key| key.starts_with(&session))
            .collect();
        let state_key = format!("{}{}", session, state_file_name(name));
        let events = if keys.contains(&state_key.as_str()) {
            match self.fetch_object(&state_key) {
                Ok(Some(bytes)) => {
                    parse_source_log(&bytes)
                        .map_err(|e| format!("the source's log doesn't read to the end: {e}"))?
                        .event_lines
                }
                // Removed between the listing and the GET.
                Ok(None) => Vec::new(),
                Err(e) => return Err(without_url_userinfo(&format!("{e:#}"))),
            }
        } else {
            Vec::new()
        };
        Ok(SourceCopy {
            objects: keys.len(),
            events,
        })
    }

    /// What the bucket holds for a marker's target: one listing of
    /// `<target_prefix>/<target_session>/` (the trailing slash keeps `foo`
    /// from matching `foo-2`), and a GET of the state file when there is one.
    /// Its header's `session_id` tells the session apart from a later one
    /// under the name, and its whole log must then read with no error and
    /// begin with every event line of `source`, the migrated source copy,
    /// unchanged (the import carries them verbatim, and the target only
    /// appends after them) before the target counts as present. A truncated
    /// or corrupt copy, one behind the source, or one that differs from it
    /// at any seq is `Unverified`, and so is any target when the source's
    /// log can't be read (`Err`, saying why).
    pub(crate) fn marker_target_status(
        &self,
        target_prefix: &str,
        target_session: &str,
        target_session_id: &str,
        source: Result<&SourceCopy, &str>,
    ) -> TargetStatus {
        if !is_workspace_prefix(target_prefix)
            || crate::session::validate::validate_session_id(target_session).is_err()
        {
            return TargetStatus::Unknown(
                "the marker's target isn't a workspace prefix and a session name".to_string(),
            );
        }
        let session = format!("{}/{}/", target_prefix, target_session);
        let listing = match self.bucket.list(session.clone(), None) {
            Ok(listing) => listing,
            Err(e) => {
                return TargetStatus::Unknown(format!(
                    "listing {} failed: {}",
                    session,
                    without_url_userinfo(&e.to_string())
                ))
            }
        };
        let keys: Vec<&str> = listing
            .iter()
            .flat_map(|page| page.contents.iter())
            .map(|obj| obj.key.as_str())
            .collect();
        if keys.iter().any(|k| is_migration_marker(&session, k)) {
            return TargetStatus::Migrated;
        }
        let state_key = format!("{}{}", session, state_file_name(target_session));
        if !keys.contains(&state_key.as_str()) {
            return TargetStatus::Absent;
        }
        let bytes = match self.fetch_object(&state_key) {
            Ok(Some(bytes)) => bytes,
            // Removed between the listing and the GET.
            Ok(None) => return TargetStatus::Absent,
            Err(e) => {
                return TargetStatus::Unknown(without_url_userinfo(&format!("{e:#}")));
            }
        };
        match header_session_id(&bytes) {
            Some(id) if id == target_session_id => {}
            Some(id) => return TargetStatus::Replaced(id),
            None => {
                return TargetStatus::Unknown(
                    "the target's state file header doesn't parse or has no session_id".to_string(),
                )
            }
        }
        let source = match source {
            Ok(source) => source,
            Err(why) => {
                return TargetStatus::Unverified(format!("the source copy can't be read: {why}"))
            }
        };
        let target = match parse_source_log(&bytes) {
            Ok(log) => log,
            Err(e) => {
                return TargetStatus::Unverified(format!(
                    "the target's log doesn't read to the end: {e}"
                ))
            }
        };
        // Both logs start at seq 1 with no gap, so index i holds seq i + 1.
        if target.event_lines.len() < source.events.len() {
            return TargetStatus::Unverified(format!(
                "the target's log ends at seq {} and the source's runs to seq {}",
                target.event_lines.len(),
                source.events.len()
            ));
        }
        match target
            .event_lines
            .iter()
            .zip(&source.events)
            .position(|(target, source)| target != source)
        {
            Some(i) => TargetStatus::Unverified(format!(
                "the target's event at seq {} differs from the source's",
                i + 1
            )),
            None => TargetStatus::Present,
        }
    }

    /// Delete every object under `<source_prefix>/<name>/`, the migration
    /// marker last. The first delete that fails stops the removal and is
    /// returned, so the marker is still there and the old copy still
    /// refuses; running it again finishes the job.
    pub(crate) fn delete_migrated_source(
        &self,
        source_prefix: &str,
        name: &str,
    ) -> anyhow::Result<()> {
        crate::session::validate::validate_session_id(name)?;
        let session = format!("{}/{}/", source_prefix, name);
        let marker = format!("{}{}", session, MIGRATED_MARKER);
        let results = self.bucket.list(session.clone(), None).map_err(|e| {
            anyhow::anyhow!(
                "listing {} failed: {}",
                session,
                without_url_userinfo(&e.to_string())
            )
        })?;
        let delete = |key: &str| {
            self.delete_object(key).map(|_| ()).map_err(|e| {
                anyhow::anyhow!(
                    "deleting {} failed: {}",
                    key,
                    without_url_userinfo(&e.to_string())
                )
            })
        };
        for page in &results {
            for obj in &page.contents {
                // The listing's prefix already holds every key under
                // `session`; the check keeps a key some endpoint returned
                // anyway from ever reaching a delete.
                if obj.key == marker || !obj.key.starts_with(&session) {
                    continue;
                }
                delete(&obj.key)?;
            }
        }
        delete(&marker)
    }

    /// List session IDs present in S3 under this backend's prefix. A listing
    /// that fails warns and reads as none.
    fn s3_list_sessions(&self) -> Vec<String> {
        self.try_list_sessions().unwrap_or_else(|e| {
            eprintln!("warning: cloud sync: failed to list sessions: {}", e);
            Vec::new()
        })
    }

    /// The session IDs under this backend's prefix, one per
    /// `<prefix>/<session-id>/` the bucket lists. `Err` when the listing
    /// failed, which a caller that must tell an empty workspace from an
    /// unreadable bucket keeps apart from an empty `Ok`.
    pub(crate) fn try_list_sessions(&self) -> anyhow::Result<Vec<String>> {
        let prefix = format!("{}/", self.prefix);
        let results = self
            .bucket
            .list(prefix.clone(), Some("/".to_string()))
            .map_err(|e| anyhow::anyhow!("listing {} failed: {}", prefix, e))?;
        let mut ids = Vec::new();
        for list in &results {
            if let Some(ref prefixes) = list.common_prefixes {
                for cp in prefixes {
                    // Common prefix looks like "<prefix>/<session-id>/"
                    if let Some(name) = cp
                        .prefix
                        .strip_prefix(&prefix)
                        .and_then(|s| s.strip_suffix('/'))
                    {
                        if !name.is_empty() {
                            ids.push(name.to_string());
                        }
                    }
                }
            }
        }
        Ok(ids)
    }

    /// Check if a session exists in S3 by looking for its state file.
    fn s3_session_exists(&self, id: &str) -> bool {
        let key = self.state_key(id);
        self.bucket.head_object(&key).is_ok()
    }

    /// DELETE `key`. Every delete this backend makes goes through here, as
    /// every PUT goes through [`CloudBackend::put_object`], so a read-only
    /// backend refuses all of them in one place.
    fn delete_object(&self, key: &str) -> anyhow::Result<()> {
        self.refuse_if_read_only(&format!("S3 DELETE of {}", key))?;
        self.bucket
            .delete_object(key)
            .map(|_| ())
            .map_err(anyhow::Error::from)
    }

    /// Wrapper around `bucket.put_object` that returns a Result.
    fn put_object(&self, key: &str, data: &[u8]) -> anyhow::Result<()> {
        self.refuse_if_read_only(&format!("S3 PUT of {}", key))?;
        self.bucket
            .put_object(key, data)
            .with_context(|| format!("S3 PUT failed for key: {}", key))?;
        Ok(())
    }

    /// S3 key for a session's version.json file.
    fn version_key(&self, id: &str) -> String {
        format!("{}/{}/version.json", self.prefix, id)
    }

    /// Path to the local version.json for a session.
    fn local_version_path(&self, id: &str) -> PathBuf {
        self.local.session_dir(id).join("version.json")
    }

    /// Read the local SessionVersion, creating it if it doesn't exist.
    fn load_or_create_local_version(
        &self,
        id: &str,
    ) -> anyhow::Result<crate::session::version::SessionVersion> {
        use crate::session::version::{get_or_create_machine_id, SessionVersion};

        let path = self.local_version_path(id);
        if let Some(v) = SessionVersion::load(&path)? {
            return Ok(v);
        }
        let machine_id = get_or_create_machine_id()?;
        let v = SessionVersion::new(machine_id);
        v.save(&path)?;
        Ok(v)
    }

    /// Fetch the remote SessionVersion from S3. Returns None if not found.
    fn fetch_remote_version(&self, id: &str) -> Option<crate::session::version::SessionVersion> {
        self.read_remote_version(id).ok().flatten()
    }

    /// The remote SessionVersion: `Ok(None)` when the bucket has none (or
    /// one that doesn't parse), `Err` when the bucket couldn't be asked.
    ///
    /// Listed before it is fetched, so a session that has never pushed one
    /// costs a listing rather than a 404 and rust-s3's one-second retry.
    fn read_remote_version(
        &self,
        id: &str,
    ) -> anyhow::Result<Option<crate::session::version::SessionVersion>> {
        Ok(self
            .get_object_if_present(&self.version_key(id))?
            .and_then(|bytes| serde_json::from_slice(&bytes).ok()))
    }

    /// Upload the local version.json to S3.
    fn push_version(&self, id: &str) -> anyhow::Result<()> {
        let path = self.local_version_path(id);
        let data = std::fs::read(&path)
            .with_context(|| format!("reading version file: {}", path.display()))?;
        let key = self.version_key(id);
        self.put_object(&key, &data)
    }

    /// Check versions before a sync push. Returns Ok(()) if safe to proceed,
    /// or an error describing the conflict.
    ///
    /// On success, increments the local version counter. The caller must call
    /// `finalize_version_after_push` after a successful S3 upload to update
    /// `last_sync_base`.
    pub fn check_and_increment_version(&self, id: &str) -> anyhow::Result<()> {
        use crate::session::version::{check_sync, conflict_message, SyncCheck};

        let mut local = self.load_or_create_local_version(id)?;
        let remote = self.fetch_remote_version(id);

        match check_sync(&local, remote.as_ref()) {
            SyncCheck::Safe => {
                local.version += 1;
                local.save(&self.local_version_path(id))?;
                Ok(())
            }
            SyncCheck::RemoteNewer => {
                // TODO: pull remote state first, then apply local op.
                // For now, treat as safe and proceed.
                local.version += 1;
                local.save(&self.local_version_path(id))?;
                Ok(())
            }
            SyncCheck::Conflict {
                local_version,
                remote_version,
                local_machine,
                remote_machine,
            } => {
                anyhow::bail!(
                    "{}",
                    conflict_message(
                        local_version,
                        remote_version,
                        &local_machine,
                        &remote_machine
                    )
                );
            }
        }
    }

    /// Update `last_sync_base` to match the current version after a
    /// successful push. Also uploads the updated version.json to S3.
    pub fn finalize_version_after_push(&self, id: &str) {
        let path = self.local_version_path(id);
        if let Ok(Some(mut v)) = crate::session::version::SessionVersion::load(&path) {
            v.last_sync_base = v.version;
            if let Err(e) = v.save(&path) {
                eprintln!("warning: failed to update version after sync: {}", e);
                return;
            }
            if let Err(e) = self.push_version(id) {
                eprintln!("warning: failed to push version to S3: {}", e);
            }
        }
    }

    /// The version check before a write: a conflict refuses the write, and
    /// anything else that stops the check (the bucket unreachable, say) is
    /// a warning and the write goes on, as it always has.
    fn version_guard(&self, id: &str) -> anyhow::Result<()> {
        if let Err(e) = self.check_and_increment_version(id) {
            if is_hard_sync_error(&e) {
                return Err(e);
            }
            eprintln!("warning: cloud sync: version check failed: {}", e);
        }
        Ok(())
    }

    // -----------------------------------------------------------------
    // Deferred writes (koto#376): see `crate::session::batch`.
    // -----------------------------------------------------------------

    /// The pending-writes journal in `id`'s local directory.
    fn journal(&self, id: &str) -> Journal {
        Journal::new(&self.local.session_dir(id))
    }

    /// Whether this process's writes to `id` wait for the tick that runs
    /// it ([`reentrancy::runs_inside_tick_of`]).
    fn defers(&self, id: &str) -> bool {
        !self.probes.read_only && reentrancy::runs_inside_tick_of(id)
    }

    /// Whether this process is `id`'s own tick or a command it runs: the
    /// tick marker names `id`.
    fn is_own_tick(&self, id: &str) -> bool {
        self.enclosing_tick().as_deref() == Some(id)
    }

    /// Append `payload` to `id`'s log locally, then push the state file;
    /// inside a tick on `id`, the append joins the tick's batch instead and
    /// the tick pushes it. The caller has run `local_guard`.
    fn append_logged(
        &self,
        id: &str,
        payload: &crate::engine::types::EventPayload,
        timestamp: &str,
    ) -> anyhow::Result<()> {
        self.refuse_if_read_only(&format!("appending to the log of '{}'", id))?;
        if self.defers(id) {
            if let Some((_journal, _lock)) = self.open_batch(id)? {
                return self.local.append_event(id, payload, timestamp);
            }
        }
        self.local.append_event(id, payload, timestamp)?;
        self.sync_push_state(id);
        Ok(())
    }

    /// Record that this process pulled `id`; false when it already had.
    fn mark_pulled(&self, id: &str) -> bool {
        self.pulled
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string())
    }

    /// S3 key of `id`'s batch-open marker.
    fn batch_marker_key(&self, id: &str) -> String {
        format!("{}{}", self.session_prefix(id), BATCH_MARKER)
    }

    /// Hold `id`'s batch open for one deferred write: the lock, with the
    /// journal created.
    ///
    /// The first write of a batch makes the checks the batch's later writes
    /// skip: the version check, whose conflict refuses the write as it
    /// always has, and the batch-open marker's PUT. `Ok(None)` means the
    /// write isn't deferred after all and pushes at once: the session has
    /// no local directory, or the marker couldn't be written, and a batch
    /// an import can't see must not be opened.
    ///
    /// The opening file, holding the marker's bytes, goes to disk before the
    /// marker goes to the bucket, and gives way to the journal only after,
    /// so a process killed between the two leaves this host something to
    /// find ([`CloudBackend::clear_unopened_batch`]). A marker PUT that
    /// fails leaves the opening file too: the PUT may have landed with its
    /// reply lost, and the next command deletes it if it did. That delete
    /// tells this writer's marker from another's by its bytes, so the marker
    /// carries the writer's pid and a nanosecond nonce beside its
    /// one-second `opened_at`: two writers would have to share a pid and a
    /// nanosecond to collide.
    fn open_batch(&self, id: &str) -> anyhow::Result<Option<(Journal, BatchLock)>> {
        let journal = self.journal(id);
        let Some(lock) = journal.lock()? else {
            return Ok(None);
        };
        if journal.is_open() {
            return Ok(Some((journal, lock)));
        }
        self.version_guard(id)?;
        let marker = serde_json::json!({
            "session": id,
            "opened_at": crate::engine::types::now_iso8601(),
            "pid": std::process::id(),
            "nonce": marker_nonce(),
        })
        .to_string();
        journal.begin(&lock, marker.as_bytes())?;
        if let Err(e) = self.put_object(&self.batch_marker_key(id), marker.as_bytes()) {
            eprintln!(
                "warning: cloud sync: couldn't mark a batch open, so this write is pushed at once: \
                 {:#}",
                e
            );
            return Ok(None);
        }
        journal.open(&lock)?;
        Ok(Some((journal, lock)))
    }

    /// Clear what a process that stopped while it opened `id`'s batch left
    /// (killed, or given up after its marker PUT failed): an opening file
    /// with no journal, and the batch-open marker it may have put in the
    /// bucket, which would make every import refuse with nothing on this
    /// host to flush it (koto#381). A no-op without an opening file, which
    /// costs a `stat`; with one, a listing, a GET and a DELETE at most.
    ///
    /// Only under the batch lock, which the opening process held from the
    /// opening file to the journal and lost when it stopped: a live process
    /// still opening the batch makes this wait, and then finds a journal and
    /// leaves the batch to the flush. The caller must not hold the lock
    /// itself. The marker is deleted only when it has the bytes the opening
    /// file holds; any other marker at the key was written since, by
    /// another process or host, and is left to it. The marker goes first,
    /// so a failed request keeps the opening file for the next command to
    /// retry.
    fn clear_unopened_batch(&self, id: &str) -> anyhow::Result<()> {
        let journal = self.journal(id);
        if !journal.is_opening() {
            return Ok(());
        }
        let Some(lock) = journal.lock()? else {
            return Ok(());
        };
        if !journal.is_opening() {
            return Ok(());
        }
        let Some(ours) = journal.opening_marker()? else {
            return Ok(());
        };
        let key = self.batch_marker_key(id);
        let in_bucket = self
            .get_object_if_present(&key)
            .with_context(|| format!("reading the batch-open marker of '{}'", id))?;
        if in_bucket.as_deref() == Some(ours.as_slice()) {
            self.delete_object(&key)
                .with_context(|| format!("deleting the batch-open marker of '{}'", id))?;
        }
        journal.abandon(&lock)
    }

    /// [`CloudBackend::clear_unopened_batch`] for a caller with its own work
    /// to do whatever happens: a failure is a warning.
    fn try_clear_unopened_batch(&self, id: &str) {
        if let Err(e) = self.clear_unopened_batch(id) {
            eprintln!("warning: cloud sync: {:#}", e);
        }
    }

    /// Push everything an open batch of `id` owes the bucket, then close it.
    /// A no-op in a command a tick on `id` runs, whose batch is the tick's
    /// to flush. With no batch open there is nothing to push, and the only
    /// work is clearing a batch begun and never opened, if one is there.
    ///
    /// The bucket is asked first, uncached, whether `id` was imported
    /// elsewhere while the batch was open (a marker refuses) and whether
    /// another host's version moved past this one's base (a conflict
    /// refuses). Either leaves the batch open and the bucket untouched.
    ///
    /// The writes then go in an order a stop between any two leaves the
    /// bucket readable: each written key's content, the manifest naming
    /// them, the state file whose events name them, `version.json`, then
    /// the objects of removed keys (orphans until then, which nothing
    /// reads) and last the batch-open marker. A flush that stops part way
    /// leaves the batch open, and the next one pushes it all again.
    pub fn flush_pending(&self, id: &str) -> anyhow::Result<()> {
        if self.defers(id) {
            return Ok(());
        }
        self.flush_batch(id)
    }

    /// [`CloudBackend::flush_pending`] without the exemption for a tick's
    /// commands: what `ensure_pushed` runs, from whatever process calls it.
    fn flush_batch(&self, id: &str) -> anyhow::Result<()> {
        use crate::session::version::{check_sync, conflict_message, SyncCheck};

        let journal = self.journal(id);
        if !journal.is_open() {
            // Nothing is owed: a marker this can't delete yet is a warning,
            // and the opening file stays for the next command.
            self.try_clear_unopened_batch(id);
            return Ok(());
        }
        let Some(lock) = journal.lock()? else {
            return Ok(());
        };
        // Another process flushed it while this one waited for the lock.
        let Some(pending) = journal.pending()? else {
            return Ok(());
        };
        self.local_guard(id)?;
        if let Some(migrated) = self.read_migration_marker_of(id)? {
            return Err(anyhow::Error::new(migrated));
        }

        let version_path = self.local_version_path(id);
        let mut local = self.load_or_create_local_version(id)?;
        // An open batch holds writes the bucket lacks, so this side moved
        // past its base whether or not the batch's first write got its
        // increment to disk (reading or saving the local version can fail
        // there, and that is only a warning). Counted before the check, a
        // remote that also moved reads as the conflict it is rather than as
        // a newer copy, which this push would write over (koto#381). Saved
        // once the check passes.
        let unsaved_increment = local.version == local.last_sync_base;
        if unsaved_increment {
            local.version += 1;
        }
        let remote = self.read_remote_version(id)?;
        if let SyncCheck::Conflict {
            local_version,
            remote_version,
            local_machine,
            remote_machine,
        } = check_sync(&local, remote.as_ref())
        {
            anyhow::bail!(
                "{}",
                conflict_message(
                    local_version,
                    remote_version,
                    &local_machine,
                    &remote_machine
                )
            );
        }
        if unsaved_increment {
            local.save(&version_path)?;
        }

        let manifest = self.local.read_manifest(id).unwrap_or_default();
        let (present, gone): (Vec<&String>, Vec<&String>) = pending
            .keys
            .iter()
            .filter(|key| !is_host_local_key(key))
            .partition(|key| manifest.keys.contains_key(key.as_str()));
        let ctx_dir = self.local.session_dir(id).join("ctx");
        let ctx_key = |key: &str| format!("{}/{}/ctx/{}", self.prefix, id, key);
        in_parallel(&present, |key| {
            let path = ctx_dir.join(key.as_str());
            let data =
                std::fs::read(&path).with_context(|| format!("reading context key '{}'", key))?;
            self.put_object(&ctx_key(key), &data)
        })?;
        if !pending.keys.is_empty() {
            if let Some(data) = sync::bucket_manifest_bytes(&ctx_dir.join("manifest.json"))? {
                self.put_object(&ctx_key("manifest.json"), &data)?;
            }
        }
        let state_path = self.local.session_dir(id).join(state_file_name(id));
        if state_path.exists() {
            let data = std::fs::read(&state_path)
                .with_context(|| format!("reading local state file: {}", state_path.display()))?;
            self.put_state(id, &data)?;
        }
        local.last_sync_base = local.version;
        local.save(&version_path)?;
        self.push_version(id)?;
        let _ = in_parallel(&gone, |key| {
            if let Err(e) = self.delete_object(&ctx_key(key)) {
                eprintln!(
                    "warning: cloud sync: failed to delete context key '{}': {}",
                    key, e
                );
            }
            Ok(())
        });
        self.manifest_cache.invalidate();
        self.forget_remote_manifest(id);
        // The batch stays open until its marker is gone: with the journal
        // closed, nothing would come back to delete a marker left behind,
        // and every import would refuse the session for good. The next
        // flush pushes the batch again, which changes nothing, and retries.
        self.delete_object(&self.batch_marker_key(id))
            .with_context(|| format!("deleting the batch-open marker of '{}'", id))?;
        journal.close(&lock)
    }

    /// Before a write that pushes at once: flush a batch another process
    /// left open, since the manifest this write pushes names the batch's
    /// keys too. A migration marker or a version conflict refuses the
    /// write; any other failure is a warning, as the push itself would be.
    fn settle_batch(&self, id: &str) -> anyhow::Result<()> {
        match self.flush_pending(id) {
            Err(e) if is_hard_sync_error(&e) => Err(e),
            Err(e) => {
                eprintln!("warning: cloud sync: {:#}", e);
                Ok(())
            }
            Ok(()) => Ok(()),
        }
    }

    /// Drop `id`'s open batch without pushing it, once `koto session
    /// resolve` has replaced one side with the other wholesale. The marker
    /// goes first: a failed delete leaves the batch open, so the next flush
    /// deletes it. With no batch open, clears a batch begun and never
    /// opened, if one is there.
    fn discard_batch(&self, id: &str) -> anyhow::Result<()> {
        let journal = self.journal(id);
        if !journal.is_open() {
            self.try_clear_unopened_batch(id);
            return Ok(());
        }
        let Some(lock) = journal.lock()? else {
            return Ok(());
        };
        self.delete_object(&self.batch_marker_key(id))
            .with_context(|| format!("deleting the batch-open marker of '{}'", id))?;
        journal.close(&lock)
    }

    /// The keys whose local copy answers a read of `session` because an
    /// open batch wrote them, so a read never pulls the bucket's stale copy
    /// over one. Outside the tick that owns the batch, the batch is flushed
    /// first and nothing is left to keep local; a flush that fails keeps
    /// the batch's keys local all the same.
    fn pending_keys(&self, session: &str) -> Option<Pending> {
        let journal = self.journal(session);
        if !journal.is_open() {
            return None;
        }
        if !self.defers(session) {
            match self.flush_pending(session) {
                Ok(()) => return None,
                Err(e) => eprintln!("warning: cloud sync: {:#}", e),
            }
        }
        match journal.pending() {
            Ok(pending) => pending,
            Err(e) => {
                eprintln!("warning: cloud sync: {:#}", e);
                None
            }
        }
    }

    /// Resolve a version conflict by keeping local or remote state.
    pub fn resolve_conflict(&self, id: &str, keep: &str) -> anyhow::Result<()> {
        use crate::session::version::{get_or_create_machine_id, resolved_version, SessionVersion};

        // Not the bucket's marker: `--keep local` is how a source pushes its
        // last writes before an import, and must work while one is marked.
        // Only the local guard, which a same-host import's other side trips.
        self.local_guard(id)?;

        let local_path = self.local_version_path(id);
        let local = self.load_or_create_local_version(id)?;
        let remote = self
            .fetch_remote_version(id)
            .unwrap_or_else(|| SessionVersion::new("unknown".to_string()));

        let machine_id = get_or_create_machine_id()?;
        let new_version = resolved_version(&local, &remote, &machine_id);

        match keep {
            "local" => {
                // Force-upload entire local session to S3.
                new_version.save(&local_path)?;
                self.force_push_session(id)?;
            }
            "remote" => {
                // Download entire remote session to local.
                self.force_pull_session(id)?;
                new_version.save(&local_path)?;
                // Upload the new version.json to S3 so both sides agree.
                self.push_version(id)?;
            }
            _ => unreachable!(), // Validated by caller.
        }

        // Either side won whole, which settles whatever an open batch owed:
        // its writes went up with the rest of the local copy, or the remote
        // replaced them. Only now, so a resolution that fails part way
        // leaves the batch, and its marker, for the next flush.
        self.discard_batch(id)
    }

    /// Make the bucket hold `id` exactly as this host's local copy has it:
    /// how `koto init --attach-live` adopts a session the local store
    /// started, once the host runs on the cloud backend over the same
    /// sessions directory. A `koto next` there pushes only the state file,
    /// so the whole copy goes up the way `--keep local` sends it. A session
    /// another host imported is refused. The whole-session push only warns
    /// on a failed state upload, so this fails unless the bucket ends up
    /// with the local state file byte for byte.
    pub fn adopt_local_copy(&self, id: &str) -> anyhow::Result<()> {
        self.check_not_migrated_to_write(id)?;
        self.resolve_conflict(id, "local")?;
        let local = self
            .read_local_state_bytes(id)
            .ok_or_else(|| anyhow::anyhow!("session '{id}' has no local state file"))?;
        match self.fetch_remote_state_bytes(id)? {
            Some(remote) if remote == local => Ok(()),
            _ => anyhow::bail!(
                "after the upload, the bucket doesn't hold the state of session '{id}'"
            ),
        }
    }

    /// Read the local state file bytes for a session, if present.
    fn read_local_state_bytes(&self, id: &str) -> Option<Vec<u8>> {
        let path = self.local.session_dir(id).join(state_file_name(id));
        std::fs::read(&path).ok()
    }

    /// Fetch the remote state file bytes for a session.
    ///
    /// Distinguishes three outcomes so callers can avoid treating a
    /// transient S3 failure as "remote absent" (which would let the
    /// strict-prefix classifier silently overwrite remote state under
    /// `auto`):
    ///
    /// * `Ok(Some(bytes))` — remote object exists and was fetched.
    /// * `Ok(None)` — remote object is confirmed absent (HTTP 404).
    /// * `Err(..)` — transient / unknown failure. Callers MUST NOT treat
    ///   this as "absent"; under `auto` they should surface an
    ///   [`ChildResolution::Errored`] rather than run the AcceptedLocal
    ///   branch and risk overwriting a remote object we simply couldn't
    ///   reach.
    ///
    /// A non-404 non-200 status is treated as transient — we only trust
    /// 404 as a positive "absent" signal because some S3-compatible
    /// endpoints return 403 or 5xx for objects that actually exist when
    /// auth is misconfigured or the backend is briefly unhealthy.
    fn fetch_remote_state_bytes(&self, id: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let key = self.state_key(id);
        match self.bucket.get_object(&key) {
            Ok(response) => {
                let status = response.status_code();
                if status == 200 {
                    Ok(Some(response.bytes().to_vec()))
                } else if status == 404 {
                    Ok(None)
                } else {
                    Err(anyhow::anyhow!(
                        "remote state fetch returned unexpected status {} for key {}",
                        status,
                        key
                    ))
                }
            }
            Err(e) => Err(anyhow::anyhow!(
                "remote state fetch failed for key {}: {}",
                key,
                e
            )),
        }
    }

    /// Classify a reconciliation decision from already-fetched bytes.
    ///
    /// Split out of [`reconcile_child`] so tests can cover every branch
    /// of the strict-prefix rule without needing a reachable S3
    /// endpoint. The I/O-touching public method reads local and remote
    /// bytes, then hands them to this pure classifier. Returns the
    /// intended action (`Identical`, `AcceptedRemote`, `AcceptedLocal`,
    /// `Skipped`, `Conflict`) or a placeholder `Errored { .. }` for
    /// unknown policy strings. Callers still execute the action — this
    /// function never touches disk or S3.
    pub fn classify_reconciliation(
        local: Option<&[u8]>,
        remote: Option<&[u8]>,
        policy: &str,
    ) -> ChildResolution {
        use crate::session::version::{strict_prefix_classify, StrictPrefixOutcome};

        match policy {
            "skip" => ChildResolution::Skipped,
            "accept-remote" => match remote {
                Some(_) => ChildResolution::AcceptedRemote,
                None => ChildResolution::Errored {
                    message: "remote state not found or unreachable".to_string(),
                },
            },
            "accept-local" => match local {
                Some(_) => ChildResolution::AcceptedLocal,
                None => ChildResolution::Errored {
                    message: "local state not found".to_string(),
                },
            },
            "auto" => match strict_prefix_classify(local, remote) {
                StrictPrefixOutcome::Identical => ChildResolution::Identical,
                StrictPrefixOutcome::AcceptLocal => ChildResolution::AcceptedLocal,
                StrictPrefixOutcome::AcceptRemote => ChildResolution::AcceptedRemote,
                StrictPrefixOutcome::Conflict => ChildResolution::Conflict,
                StrictPrefixOutcome::OneSideMissing => match (local.is_some(), remote.is_some()) {
                    (true, false) => ChildResolution::AcceptedLocal,
                    (false, true) => ChildResolution::AcceptedRemote,
                    _ => ChildResolution::Identical,
                },
            },
            other => ChildResolution::Errored {
                message: format!("unknown children policy: '{}'", other),
            },
        }
    }

    /// Reconcile a single child's state file using the strict-prefix
    /// rule and the provided policy, returning the action taken.
    ///
    /// Intended for use by `session resolve --children=<policy>`. The
    /// parent's lock/version reconciliation happens in
    /// `resolve_conflict`; this helper handles the per-child leg so
    /// callers can iterate over the parent's direct children.
    ///
    /// `policy` maps directly to the `--children` flag:
    /// * `"auto"` — apply the strict-prefix classification and act on
    ///   the result. `Conflict` surfaces as `ChildResolution::Conflict`
    ///   without touching either side.
    /// * `"accept-remote"` — pull remote over local unconditionally.
    /// * `"accept-local"` — push local over remote unconditionally.
    /// * `"skip"` — return `ChildResolution::Skipped` without touching
    ///   either side.
    pub fn reconcile_child(&self, id: &str, policy: &str) -> ChildResolution {
        let local = self.read_local_state_bytes(id);

        // `skip` is a pure decision with no I/O: short-circuit so a
        // transient S3 fetch error cannot convert an explicit skip into
        // an Errored outcome.
        if policy == "skip" {
            return ChildResolution::Skipped;
        }

        // Probe remote. `Ok(None)` is a confirmed 404 (safe to treat as
        // absent); `Err(..)` is transient/unknown and MUST short-circuit
        // to Errored so `auto` never fires AcceptedLocal over a remote
        // object we couldn't reach.
        let remote = match self.fetch_remote_state_bytes(id) {
            Ok(bytes) => bytes,
            Err(e) => {
                return ChildResolution::Errored {
                    message: format!("remote state unreachable: {}", e),
                };
            }
        };
        let decision = Self::classify_reconciliation(local.as_deref(), remote.as_deref(), policy);

        // Execute the classified action. `classify_reconciliation`
        // returned the *intent*; this block performs the matching I/O.
        // Any per-child I/O failure converts to `Errored` so sibling
        // reconciliations still process.
        match decision {
            ChildResolution::Identical
            | ChildResolution::Skipped
            | ChildResolution::Conflict
            | ChildResolution::Errored { .. } => decision,
            ChildResolution::AcceptedRemote => match remote {
                Some(bytes) => match self.write_local_state_bytes(id, &bytes) {
                    Ok(()) => ChildResolution::AcceptedRemote,
                    Err(e) => ChildResolution::Errored {
                        message: format!("failed to write local state: {}", e),
                    },
                },
                None => ChildResolution::Errored {
                    message: "accept-remote classified but remote bytes were unavailable"
                        .to_string(),
                },
            },
            ChildResolution::AcceptedLocal => match self.push_local_state_bytes(id) {
                Ok(()) => ChildResolution::AcceptedLocal,
                Err(e) => ChildResolution::Errored {
                    message: format!("failed to push local state: {}", e),
                },
            },
        }
    }

    /// Overwrite the local state file with the given bytes, creating
    /// the session directory if needed.
    fn write_local_state_bytes(&self, id: &str, bytes: &[u8]) -> anyhow::Result<()> {
        let dir = self.local.session_dir(id);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(state_file_name(id));
        std::fs::write(&path, bytes)?;
        Ok(())
    }

    /// PUT the local state file to S3. Unlike `sync_push_state` this
    /// surfaces a `Result` so the caller can distinguish success from
    /// silent failure — `session resolve --children` needs the typed
    /// outcome to report `accepted-local` vs `errored` per child.
    fn push_local_state_bytes(&self, id: &str) -> anyhow::Result<()> {
        let path = self.local.session_dir(id).join(state_file_name(id));
        let data = std::fs::read(&path)
            .with_context(|| format!("reading local state file: {}", path.display()))?;
        self.put_state(id, &data)
    }

    /// Return true if cloud sync is available for callers that want to
    /// gate a feature (e.g., `sync_status` / `machine_id` response
    /// fields) on the backend being `Cloud`.
    #[inline]
    pub fn is_cloud(&self) -> bool {
        true
    }

    /// Best-effort probe: does this session's state file exist on S3?
    ///
    /// Used by `session resolve` to decide whether the post-resolve
    /// parent state is `"fresh"` (local and remote both present after
    /// reconciliation) or `"local_only"` (we wrote locally but S3 was
    /// unreachable during the final push). Returns `false` on any
    /// network or non-success response.
    pub fn remote_state_exists(&self, id: &str) -> bool {
        self.s3_session_exists(id)
    }

    /// Force-upload the local session to S3: every context key the bucket
    /// doesn't already hold as this host has it, then the manifest, the
    /// state file and `version.json` (`koto session resolve --keep local`).
    fn force_push_session(&self, id: &str) -> anyhow::Result<()> {
        let session_dir = self.local.session_dir(id);
        if !session_dir.exists() {
            anyhow::bail!(
                "session directory does not exist: {}",
                session_dir.display()
            );
        }

        // One pass (koto#428): the session's prefix is listed once and its
        // manifest read once, and a context key goes up unless the bucket
        // both holds its object and names it in the manifest with this
        // host's hash: this is a repair, so an object the manifest names but
        // the bucket lost goes up too. (An object whose bytes changed while
        // the manifest kept this host's hash is not compared and not sent.)
        // The keys go in parallel; then the
        // manifest, the state file and `version.json`, in the order the
        // batch flush uses. Host-local keys stay on this host, and files in
        // `ctx/` the manifest doesn't name (its lock) are not context. A key
        // the manifest names with no content file here refuses the push
        // rather than send a manifest naming it; a failed upload is a
        // warning, as every push here always was. When the listing or the
        // manifest can't be read, every key goes up.
        // A fresh probe and manifest, not this process's cached ones: the
        // comparison below decides what is skipped.
        self.manifest_cache.invalidate();
        self.forget_probe(id);
        self.prime_manifest(id);
        let remote = self.manifest_cache.get(id).unwrap_or_default();
        let prefix = self.session_prefix(id);
        let listed: HashSet<String> = self
            .bucket
            .list(prefix.clone(), None)
            .map(|pages| {
                pages
                    .iter()
                    .flat_map(|page| page.contents.iter())
                    .map(|obj| obj.key.clone())
                    .collect()
            })
            .unwrap_or_default();
        let ctx_dir = session_dir.join("ctx");
        let manifest = self.local.read_manifest(id)?;
        let mut uploads: Vec<(String, Vec<u8>)> = Vec::new();
        for (key, meta) in &manifest.keys {
            let held = listed.contains(&format!("{}ctx/{}", prefix, key))
                && remote.keys.get(key).is_some_and(|r| r.hash == meta.hash);
            if is_host_local_key(key) || held {
                continue;
            }
            let data = std::fs::read(ctx_dir.join(key))
                .with_context(|| format!("reading context key '{}'", key))?;
            uploads.push((key.clone(), data));
        }
        let _ = in_parallel(&uploads, |(name, data)| {
            let s3_key = format!("{}/{}/ctx/{}", self.prefix, id, name);
            if let Err(e) = self.put_object(&s3_key, data) {
                eprintln!("warning: cloud sync: failed to upload ctx/{}: {}", name, e);
            }
            Ok(())
        });
        if let Some(data) = sync::bucket_manifest_bytes(&ctx_dir.join("manifest.json"))? {
            let s3_key = format!("{}/{}/ctx/manifest.json", self.prefix, id);
            if let Err(e) = self.put_object(&s3_key, &data) {
                eprintln!(
                    "warning: cloud sync: failed to upload ctx/manifest.json: {}",
                    e
                );
            }
        }
        self.manifest_cache.invalidate();
        self.forget_remote_manifest(id);

        // The state file directly, not through `sync_push_state`, which
        // would flush an open batch and refuse on the very conflict this
        // upload resolves; the context files above cover the batch's keys.
        if let Err(e) = self.push_local_state_bytes(id) {
            eprintln!("warning: cloud sync failed for state upload: {:#}", e);
        }

        self.push_version(id)?;

        Ok(())
    }

    /// Download the entire remote session from S3 to local.
    fn force_pull_session(&self, id: &str) -> anyhow::Result<()> {
        let session_dir = self.local.session_dir(id);
        std::fs::create_dir_all(&session_dir)?;

        // Download state file.
        let state_key = self.state_key(id);
        if let Ok(response) = self.bucket.get_object(&state_key) {
            if response.status_code() == 200 {
                let state_path = session_dir.join(state_file_name(id));
                refuse_cross_import_pull(id, &state_path, response.bytes())?;
                std::fs::write(&state_path, response.bytes())?;
            }
        }

        // Download all context files by listing the ctx/ prefix. Host-local
        // keys aren't the bucket's to replace: this host keeps its own, and
        // keeps them in the manifest it takes from the bucket.
        let kept: Vec<(String, crate::session::context::KeyMeta)> = self
            .local
            .read_manifest(id)
            .map(|m| {
                m.keys
                    .into_iter()
                    .filter(|(key, _)| is_host_local_key(key))
                    .collect()
            })
            .unwrap_or_default();
        let ctx_prefix = format!("{}/{}/ctx/", self.prefix, id);
        if let Ok(results) = self.bucket.list(ctx_prefix.clone(), None) {
            let ctx_dir = session_dir.join("ctx");
            std::fs::create_dir_all(&ctx_dir)?;
            for list in &results {
                for obj in &list.contents {
                    if let Some(file_name) = obj.key.strip_prefix(&ctx_prefix) {
                        if file_name.is_empty() || is_host_local_key(file_name) {
                            continue;
                        }
                        if let Ok(response) = self.bucket.get_object(&obj.key) {
                            if response.status_code() == 200 {
                                let local_path = ctx_dir.join(file_name);
                                if let Some(parent) = local_path.parent() {
                                    std::fs::create_dir_all(parent)?;
                                }
                                let mut data = response.bytes().to_vec();
                                if file_name == "manifest.json" {
                                    data = with_host_local_keys(data, &kept);
                                }
                                std::fs::write(&local_path, data)?;
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

/// A manifest pulled from the bucket, as this host keeps it: whatever
/// host-local keys it lists dropped, and this host's own (`kept`) put back.
/// Bytes that don't parse as a manifest are kept as they are.
fn with_host_local_keys(
    pulled: Vec<u8>,
    kept: &[(String, crate::session::context::KeyMeta)],
) -> Vec<u8> {
    let Ok(mut manifest) = serde_json::from_slice::<crate::session::context::Manifest>(&pulled)
    else {
        return pulled;
    };
    if !manifest.drop_host_local() && kept.is_empty() {
        return pulled;
    }
    manifest.keys.extend(kept.iter().cloned());
    serde_json::to_vec_pretty(&manifest).expect("Manifest serialize is infallible")
}

/// Build a placeholder `SessionInfo` for a session that exists only in S3
/// and has no local copy to read full metadata from.
///
/// `template_source_status` is unconditionally `None` here: there is no
/// header in scope to check, so this can never be inferred from -- or
/// overridden by -- any other session's recorded status for the same id.
fn placeholder_session_info(id: String) -> SessionInfo {
    SessionInfo {
        id,
        created_at: String::new(),
        template_hash: String::new(),
        parent_workflow: None,
        template_source_status: None,
        execution_dir: None,
        session_id: None,
        kind: None,
    }
}

impl SessionBackend for CloudBackend {
    fn create(&self, id: &str) -> anyhow::Result<PathBuf> {
        let path = self.local.create(id)?;
        Ok(path)
    }

    fn session_dir(&self, id: &str) -> PathBuf {
        self.local.session_dir(id)
    }

    fn store_identity(&self) -> Option<crate::engine::types::SessionStoreIdentity> {
        let base = self.local.base_dir();
        Some(crate::engine::types::SessionStoreIdentity {
            kind: "cloud".to_string(),
            base: std::fs::canonicalize(base).unwrap_or_else(|_| base.to_path_buf()),
            bucket: Some(self.bucket.name()),
        })
    }

    fn exists(&self, id: &str) -> bool {
        if self.local.exists(id) {
            return true;
        }
        // Fall back to S3 check.
        self.s3_session_exists(id)
    }

    fn cleanup(&self, id: &str) -> anyhow::Result<()> {
        self.refuse_if_read_only(&format!("removing '{}'", id))?;
        // A workspace whose copy was imported on this host must not delete
        // the imported session that now holds the name.
        self.local_guard(id)?;
        self.local.cleanup(id)?;
        self.sync_delete_session(id);
        Ok(())
    }

    fn list(&self) -> anyhow::Result<Vec<SessionInfo>> {
        let mut local_sessions = self.local.list()?;
        let local_ids: std::collections::HashSet<String> =
            local_sessions.iter().map(|s| s.id.clone()).collect();

        // Merge in any sessions that exist only in S3.
        let remote_ids = self.s3_list_sessions();
        for remote_id in remote_ids {
            if !local_ids.contains(&remote_id) {
                // We can't extract full metadata without downloading the
                // state file, so provide placeholder values.
                local_sessions.push(placeholder_session_info(remote_id));
            }
        }

        local_sessions.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(local_sessions)
    }

    fn append_header(
        &self,
        id: &str,
        header: &crate::engine::types::StateFileHeader,
    ) -> anyhow::Result<()> {
        self.local_guard(id)?;
        self.local.append_header(id, header)?;
        self.sync_push_state(id);
        Ok(())
    }

    /// Append locally, then push the state file; inside a tick on `id`, the
    /// append joins the tick's batch instead and the tick pushes it.
    fn append_event(
        &self,
        id: &str,
        payload: &crate::engine::types::EventPayload,
        timestamp: &str,
    ) -> anyhow::Result<()> {
        self.local_guard(id)?;
        // An anchorless read of `id` by anything but its own tick, and the
        // pull of a changed key such a read made, are kept on this host, in
        // the read spool, and never written into `id`'s log or pushed: the
        // reader must not become a writer of a session it only read. Nothing
        // that decides from the log misses the pull: clear-on-entry already
        // passes over `sync` writes (`crate::engine::clear_on_entry`).
        //
        // For reads this is the backstop for a caller that doesn't route
        // them. `koto context get` and `exists` do: they spool an anchorless
        // read themselves, send an own read through `append_own_read`,
        // which skips this, and write a read from another agent session into
        // the reader's anchor, carrying the session it read in `session`.
        // That one is the anchor's own write and goes into its log like any
        // other; `read_spool::spooled` is false for it.
        if crate::session::read_spool::spooled(payload) && !self.is_own_tick(id) {
            return crate::session::read_spool::append(
                &self.local.session_dir(id),
                timestamp,
                payload,
            );
        }
        self.append_logged(id, payload, timestamp)
    }

    /// The read is `id`'s own, as its reader judged: into the log and
    /// pushed, like any event, with no spool backstop.
    fn append_own_read(
        &self,
        id: &str,
        payload: &crate::engine::types::EventPayload,
        timestamp: &str,
    ) -> anyhow::Result<()> {
        self.local_guard(id)?;
        self.append_logged(id, payload, timestamp)
    }

    fn read_events(
        &self,
        id: &str,
    ) -> anyhow::Result<(
        crate::engine::types::StateFileHeader,
        Vec<crate::engine::types::Event>,
    )> {
        // Before the pull, so a migrated session's local file is left
        // exactly as it was.
        self.check_not_migrated(id)?;
        self.sync_pull_state(id)?;
        self.local.read_events(id)
    }

    /// Read the local state file only: no pull from S3. The decider reads
    /// the log through this while it holds `decider.lock`, so the lock is
    /// never held across a network round trip.
    fn read_events_local(
        &self,
        id: &str,
    ) -> anyhow::Result<(
        crate::engine::types::StateFileHeader,
        Vec<crate::engine::types::Event>,
    )> {
        self.local_guard(id)?;
        self.local.read_events(id)
    }

    fn read_header(&self, id: &str) -> anyhow::Result<crate::engine::types::StateFileHeader> {
        self.check_not_migrated(id)?;
        self.sync_pull_state(id)?;
        self.local.read_header(id)
    }

    /// Rewrite the local header, then push the state file through
    /// `sync_push_state`, the same best-effort push `append_event` makes.
    ///
    /// Overridden rather than left to the trait default, which pushes
    /// through `ensure_pushed` (the strict probe built for "push parent
    /// before child mutation" ordering) and turns its error into a warning.
    /// Going through `sync_push_state` keeps a header rewrite on the same
    /// push path, and the same warning, as the event write it follows.
    ///
    /// The risk is accepted, not handled: when the push fails (offline, or
    /// the bucket refuses it) the rewrite stays local and a warning goes to
    /// stderr. The next successful push of this session carries it; until
    /// then, a pull from another command can restore the old header. Rebind
    /// and anchor adoption take that risk; a caller that can't follows up
    /// with `ensure_pushed`, as the command-environment adoption does.
    fn rewrite_header(
        &self,
        id: &str,
        f: &dyn Fn(crate::engine::types::StateFileHeader) -> crate::engine::types::StateFileHeader,
    ) -> anyhow::Result<()> {
        self.local_guard(id)?;
        if self.defers(id) {
            if let Some((_journal, _lock)) = self.open_batch(id)? {
                return self.local.rewrite_header(id, f);
            }
        }
        self.local.rewrite_header(id, f)?;
        self.sync_push_state(id);
        Ok(())
    }

    fn init_state_file(
        &self,
        id: &str,
        header: crate::engine::types::StateFileHeader,
        initial_events: Vec<crate::engine::types::Event>,
    ) -> Result<(), SessionError> {
        // Delegate the atomic bundle to LocalBackend. On success, do a
        // single S3 PUT that replaces the three pushes the old
        // header+event sequence required.
        //
        // NOTE for callers relying on "push parent before child
        // mutation" ordering: `sync_push_state` runs AFTER the local
        // atomic rename has committed. A network / S3 failure at that
        // point leaves the local state file intact (the init has
        // succeeded from the caller's perspective) but the remote is
        // stale until the next successful push. Downstream logic that
        // needs remote-visibility guarantees must reconcile locally-
        // committed state with a best-effort remote sync; this method
        // does not block on the upload.
        let template_path =
            initial_events.iter().find_map(|e| match &e.payload {
                crate::engine::types::EventPayload::WorkflowInitialized {
                    template_path, ..
                } if !template_path.is_empty() => Some(template_path.clone()),
                _ => None,
            });
        self.local.init_state_file(id, header, initial_events)?;
        self.sync_push_state(id);
        if let Some(template_path) = template_path {
            self.push_template(id, &template_path);
        }
        Ok(())
    }

    fn relocate(&self, from: &str, to: &str) -> anyhow::Result<()> {
        self.local_guard(from)?;
        self.local_guard(to)?;
        // Local rename is authoritative; S3 propagation is best-effort.
        self.local.relocate(from, to)?;

        // Propagate to S3: copy objects from old prefix to new, then
        // delete the old objects. Mirrors the pattern in
        // sync_delete_session. Failures are logged but don't fail the
        // operation since local state is the source of truth.
        let old_prefix = self.session_prefix(from);
        match self.bucket.list(old_prefix.clone(), None) {
            Ok(results) => {
                for list in &results {
                    for obj in &list.contents {
                        // Derive the new key by replacing the old session
                        // id segment with the new one.
                        let suffix = match obj.key.strip_prefix(&old_prefix) {
                            Some(s) => s,
                            None => continue,
                        };
                        // The marker stays where it is, as it does through
                        // a cleanup: moving it would lift the refusal from
                        // the name the other hosts know the session by.
                        if is_migration_marker(&old_prefix, &obj.key) {
                            continue;
                        }
                        let new_key = format!("{}{}", self.session_prefix(to), suffix);

                        // Copy old -> new, then delete old.
                        match self.bucket.get_object(&obj.key) {
                            Ok(response) if response.status_code() == 200 => {
                                if let Err(e) = self.put_object(&new_key, response.bytes()) {
                                    eprintln!(
                                        "warning: cloud sync: relocate copy failed for {}: {}",
                                        obj.key, e
                                    );
                                    continue;
                                }
                                if let Err(e) = self.delete_object(&obj.key) {
                                    eprintln!(
                                        "warning: cloud sync: relocate delete failed for {}: {}",
                                        obj.key, e
                                    );
                                }
                            }
                            Ok(_) => {}
                            Err(e) => {
                                eprintln!(
                                    "warning: cloud sync: relocate get failed for {}: {}",
                                    obj.key, e
                                );
                            }
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "warning: cloud sync: relocate list failed for prefix {}: {}",
                    old_prefix, e
                );
            }
        }

        // Push the updated local state file to S3 under the new key.
        self.sync_push_state(to);

        Ok(())
    }

    fn remote_prefix_free(&self, id: &str) -> anyhow::Result<bool> {
        // The prefix ends in `/`, so `x_r1/` never matches `x_r10/...`.
        let prefix = self.session_prefix(id);
        let results = self.bucket.list(prefix.clone(), None).map_err(|e| {
            anyhow::anyhow!(
                "listing {} failed: {}",
                prefix,
                without_url_userinfo(&e.to_string())
            )
        })?;
        Ok(!results
            .iter()
            .any(|page| page.contents.iter().any(|o| o.key.starts_with(&prefix))))
    }

    fn relocate_checked(&self, from: &str, to: &str) -> anyhow::Result<()> {
        self.local_guard(from)?;
        self.local_guard(to)?;
        let old_prefix = self.session_prefix(from);
        let new_prefix = self.session_prefix(to);
        // List before renaming, so a failed listing changes nothing.
        let results = self.bucket.list(old_prefix.clone(), None).map_err(|e| {
            anyhow::anyhow!(
                "listing {} failed: {}",
                old_prefix,
                without_url_userinfo(&e.to_string())
            )
        })?;
        self.local.relocate(from, to)?;
        // The state log's object name embeds the session name, so it is
        // not copied: the caller pushes it under the new name. The marker
        // stays where it is, as it does through a cleanup.
        let old_state = self.state_key(from);
        for page in &results {
            for obj in &page.contents {
                let Some(suffix) = obj.key.strip_prefix(&old_prefix) else {
                    continue;
                };
                if obj.key == old_state || is_migration_marker(&old_prefix, &obj.key) {
                    continue;
                }
                let bytes = self.fetch_object(&obj.key)?.ok_or_else(|| {
                    anyhow::anyhow!("{} was listed but could not be fetched", obj.key)
                })?;
                self.put_object(&format!("{new_prefix}{suffix}"), &bytes)
                    .with_context(|| format!("copying {} under {}", obj.key, new_prefix))?;
            }
        }
        Ok(())
    }

    fn relocate_local_only(&self, from: &str, to: &str) -> anyhow::Result<()> {
        self.local_guard(from)?;
        self.local_guard(to)?;
        self.local.relocate(from, to)
    }

    fn delete_remote_checked(&self, id: &str) -> anyhow::Result<()> {
        self.delete_remote_session_checked(id)
    }

    fn lock_state_file(&self, id: &str) -> Result<SessionLock, SessionError> {
        // `flock` is strictly a local, per-host primitive. Cloud
        // instances running on different hosts cannot observe each
        // other's locks; the design's cross-host coordination story
        // relies on "push parent before child mutation" ordering
        // (Decision 12 Q6) rather than on this lock. Here we simply
        // delegate to the local backend so intra-host contention is
        // still serialized cleanly -- e.g., two `koto next` invocations
        // on the same developer machine, or a scheduler tick racing a
        // manual CLI call.
        self.local.lock_state_file(id)
    }

    fn ensure_pushed(&self, id: &str) -> Result<(), SessionError> {
        // Strict variant of the cloud sync: fail fast on any S3 error
        // so callers enforcing "push parent before child mutation" can
        // abort before any child write commits. The plain append_event
        // path still uses the warning-only sync_push_state; only the
        // retry-failed dispatcher (and similar ordering-sensitive call
        // sites) route through this probe.
        self.strict_push_state(id)
            .map_err(|e| SessionError::Other(e.context("strict parent state push failed")))
    }
}

impl CloudBackend {
    /// Write locally (recording `writer` when given), then push the key. A
    /// host-local key is written locally only: it has nothing to push and
    /// joins no batch. The agent writer is refused the reserved `koto/` area
    /// before anything is recorded, written or pushed.
    fn add_as(
        &self,
        session: &str,
        key: &str,
        content: &[u8],
        writer: Option<&str>,
    ) -> anyhow::Result<()> {
        self.refuse_if_read_only(&format!("storing '{}' in '{}'", key, session))?;
        if let Some(writer) = writer {
            crate::session::links::check_writer_key(key, writer)?;
        }
        self.check_not_migrated_to_write(session)?;
        let write = || match writer {
            Some(w) => self.local.add_with_writer(session, key, content, w),
            None => self.local.add(session, key, content),
        };
        if is_host_local_key(key) {
            return write();
        }

        // Inside a tick on this session: record the key, then write it, and
        // leave the push to the tick. Recorded first, so a write that stops
        // half way is still one the flush looks at.
        if self.defers(session) {
            if let Some((journal, lock)) = self.open_batch(session)? {
                journal.record(
                    &lock,
                    &batch::Op::Add {
                        key: key.to_string(),
                    },
                )?;
                return write();
            }
        }
        self.settle_batch(session)?;
        write()?;

        // Check version before pushing. Conflicts are hard errors;
        // S3 connectivity failures are non-fatal (version check is skipped).
        self.version_guard(session)?;

        sync::push_context_key(
            &self.local,
            &self.bucket,
            &self.prefix,
            session,
            key,
            &self.manifest_cache,
        );
        self.forget_remote_manifest(session);

        // Update last_sync_base after successful push.
        self.finalize_version_after_push(session);

        Ok(())
    }
}

/// Every method asks [`CloudBackend::check_not_migrated`] first, so a
/// session imported elsewhere neither reads nor writes its keys here: the
/// methods that return a `Result` refuse with `SessionMigrated`, while
/// `ctx_exists` answers false and `meta` none, having no room for a
/// reason. The answer is cached per session, so a command that touches
/// many keys pays for it once.
impl ContextStore for CloudBackend {
    fn add(&self, session: &str, key: &str, content: &[u8]) -> anyhow::Result<()> {
        self.add_as(session, key, content, None)
    }

    fn add_with_writer(
        &self,
        session: &str,
        key: &str,
        content: &[u8],
        writer: &str,
    ) -> anyhow::Result<()> {
        self.add_as(session, key, content, Some(writer))
    }

    /// The local metadata, or for a key only the remote store has, the
    /// remote manifest's.
    ///
    /// A migrated session has no metadata to give: `None`.
    fn meta(&self, session: &str, key: &str) -> Option<crate::session::context::KeyMeta> {
        if self.check_not_migrated(session).is_err() {
            return None;
        }
        if let Some(meta) = self.local.meta(session, key) {
            return Some(meta);
        }
        if !crate::session::context_log::loggable_key(key) || is_host_local_key(key) {
            return None;
        }
        if self
            .pending_keys(session)
            .is_some_and(|pending| pending.touches(key))
        {
            return None;
        }
        self.prime_manifest(session);
        sync::remote_key_meta(
            &self.bucket,
            &self.prefix,
            session,
            key,
            &self.manifest_cache,
        )
    }

    fn get(&self, session: &str, key: &str) -> anyhow::Result<Vec<u8>> {
        self.check_not_migrated(session)?;
        // A key an open batch wrote is read as the batch left it, and a
        // host-local key only ever lives here.
        if is_host_local_key(key)
            || self
                .pending_keys(session)
                .is_some_and(|pending| pending.touches(key))
        {
            return self.local.get(session, key);
        }
        // Pull from remote if a newer version exists. A pull that wrote the
        // local store is logged as a `sync` write -- before the read the
        // caller is about to log, which it produced. From anything but the
        // session's own tick, `append_event` spools it beside the read
        // instead, so only this host's mirror of the key changes.
        self.prime_manifest(session);
        let pulled = sync::pull_context_if_newer(
            &self.local,
            &self.bucket,
            &self.prefix,
            session,
            key,
            &self.manifest_cache,
            &|| self.check_context_pull(session),
        )?;
        if pulled {
            if let Some(meta) = self.local.meta(session, key) {
                crate::session::context_log::append_to_session_best_effort(
                    self,
                    session,
                    &crate::session::context_log::added_event_from_meta(
                        key,
                        &meta,
                        crate::session::context_log::WRITER_SYNC,
                    ),
                );
            }
        }
        self.local.get(session, key)
    }

    /// A migrated session holds no key here: `false`. Callers that must
    /// tell the operator why ask [`CloudBackend::check_not_migrated`].
    fn ctx_exists(&self, session: &str, key: &str) -> bool {
        if self.check_not_migrated(session).is_err() {
            return false;
        }
        if self.local.ctx_exists(session, key) {
            return true;
        }
        if is_host_local_key(key)
            || self
                .pending_keys(session)
                .is_some_and(|pending| pending.touches(key))
        {
            return false;
        }
        // Fall back to checking remote manifest.
        self.prime_manifest(session);
        sync::remote_key_exists(
            &self.bucket,
            &self.prefix,
            session,
            key,
            &self.manifest_cache,
        )
        .unwrap_or(false)
    }

    fn remove(&self, session: &str, key: &str) -> anyhow::Result<()> {
        self.refuse_if_read_only(&format!("removing '{}' from '{}'", key, session))?;
        self.check_not_migrated_to_write(session)?;
        if is_host_local_key(key) {
            return self.local.remove(session, key);
        }
        if self.defers(session) {
            if let Some((journal, lock)) = self.open_batch(session)? {
                journal.record(
                    &lock,
                    &batch::Op::Remove {
                        key: key.to_string(),
                    },
                )?;
                return self.local.remove(session, key);
            }
        }
        self.settle_batch(session)?;
        let held_locally = self.local.ctx_exists(session, key);
        self.local.remove(session, key)?;

        // A key neither copy has leaves nothing to delete or to drop from
        // the manifest: the removal is a no-op, as it is locally, and costs
        // the one manifest read that says so. A manifest that can't be read
        // goes the long way round.
        if !held_locally
            && sync::remote_key_exists(
                &self.bucket,
                &self.prefix,
                session,
                key,
                &self.manifest_cache,
            ) == Some(false)
        {
            return Ok(());
        }

        // Check version before pushing deletion.
        self.version_guard(session)?;

        sync::delete_context_key(
            &self.local,
            &self.bucket,
            &self.prefix,
            session,
            key,
            &self.manifest_cache,
        );
        self.forget_remote_manifest(session);

        self.finalize_version_after_push(session);

        Ok(())
    }

    fn list_keys(&self, session: &str, prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
        self.check_not_migrated(session)?;
        let mut keys = self.local.list_keys(session, prefix)?;
        let pending = self.pending_keys(session).unwrap_or_default();
        // Merge in remote-only keys, leaving out those an open batch
        // removed here.
        self.prime_manifest(session);
        if let Some(remote_keys) = sync::remote_list_keys(
            &self.bucket,
            &self.prefix,
            session,
            prefix,
            &self.manifest_cache,
        ) {
            for k in remote_keys {
                if !keys.contains(&k) && !pending.touches(&k) {
                    keys.push(k);
                }
            }
        }
        keys.sort();
        Ok(keys)
    }
}

// ---------------------------------------------------------------------------
// Import: `koto session import <name> --from <workspace-path>`
// ---------------------------------------------------------------------------

/// Why `koto session import` stopped, as the code it prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportErrorCode {
    /// The backend is local: there is no remote copy to read.
    RequiresCloud,
    /// The source workspace's prefix holds no state file for the session.
    SourceNotFound,
    /// The source already carries a migration marker.
    SourceMigrated,
    /// The source is a child session, which isn't imported on its own.
    SourceIsChild,
    /// The source is an anchor's close session (`kind: "anchor-close"`),
    /// which records a close on its own host and is never imported.
    SourceIsAnchorClose,
    /// The name is taken on this machine or under this workspace's prefix.
    NameTaken,
    /// This machine's template cache has no usable compiled template with
    /// the session's hash.
    TemplateUnavailable,
    /// A source object couldn't be read, or failed validation.
    SourceUnreadable,
    /// Building the imported session here, pushing it under this
    /// workspace's prefix or moving it into place failed, or something the
    /// import needs on this side couldn't be read: this machine's id, the
    /// listing that checks this workspace's prefix for the name, or the
    /// manifest of a target an earlier run left here. A failure while
    /// staging, pushing or moving removes the staging directory and takes
    /// back what the run pushed; the others come before anything is
    /// written, so there is nothing to take back.
    PushFailed,
    /// The imported session is in place and pushed, but the marker on the
    /// source couldn't be written.
    Unmarked,
    /// A same-host import found another process holding the source's lock.
    SourceRunning,
    /// The source's host holds writes the bucket lacks: a same-host import
    /// found the source's local copy ahead of the bucket, or any import
    /// found the source's batch-open marker.
    SourceUnpushed,
    /// A same-host import found a local session naming the source as its
    /// parent.
    SourceHasChildren,
}

impl ImportErrorCode {
    /// The code as printed in the error object.
    pub fn as_str(self) -> &'static str {
        match self {
            ImportErrorCode::RequiresCloud => "import_requires_cloud",
            ImportErrorCode::SourceNotFound => "import_source_not_found",
            ImportErrorCode::SourceMigrated => "import_source_migrated",
            ImportErrorCode::SourceIsChild => "import_source_is_child",
            ImportErrorCode::SourceIsAnchorClose => "import_source_is_anchor_close",
            ImportErrorCode::NameTaken => "import_name_taken",
            ImportErrorCode::TemplateUnavailable => "import_template_unavailable",
            ImportErrorCode::SourceUnreadable => "import_source_unreadable",
            ImportErrorCode::PushFailed => "import_push_failed",
            ImportErrorCode::Unmarked => "import_unmarked",
            ImportErrorCode::SourceRunning => "import_source_running",
            ImportErrorCode::SourceUnpushed => "import_source_unpushed",
            ImportErrorCode::SourceHasChildren => "import_source_has_children",
        }
    }

    /// 2 for a refusal the caller acts on (pick another name, compile the
    /// template, import from the right workspace), 1 for a failure.
    pub fn exit_code(self) -> i32 {
        match self {
            ImportErrorCode::RequiresCloud
            | ImportErrorCode::SourceNotFound
            | ImportErrorCode::SourceMigrated
            | ImportErrorCode::SourceIsChild
            | ImportErrorCode::SourceIsAnchorClose
            | ImportErrorCode::NameTaken
            | ImportErrorCode::TemplateUnavailable
            | ImportErrorCode::SourceRunning
            | ImportErrorCode::SourceUnpushed
            | ImportErrorCode::SourceHasChildren => 2,
            ImportErrorCode::SourceUnreadable
            | ImportErrorCode::PushFailed
            | ImportErrorCode::Unmarked => 1,
        }
    }
}

/// An import refusal or failure: a code and a message for the operator.
#[derive(Debug)]
pub struct ImportError {
    pub code: ImportErrorCode,
    pub message: String,
}

impl ImportError {
    pub fn new(code: ImportErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: without_url_userinfo(&message.into()),
        }
    }
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for ImportError {}

/// Where an import took the session's compiled template from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateOrigin {
    /// This machine's template cache, which `koto template compile` fills.
    LocalCache,
    /// The source's `template.json` in the bucket, under `--trust-template`.
    Bucket,
    /// None: a re-run that found the target already built took no
    /// template, and the one in the target's directory is unchanged.
    Unchanged,
}

impl TemplateOrigin {
    /// The value of the import output's `template` field.
    pub fn as_str(self) -> &'static str {
        match self {
            TemplateOrigin::LocalCache => "local-cache",
            TemplateOrigin::Bucket => "bucket",
            TemplateOrigin::Unchanged => "unchanged",
        }
    }
}

/// What `koto session import` was asked to do.
#[derive(Debug, Clone, Copy)]
pub struct ImportRequest<'a> {
    /// The session's name in the source workspace.
    pub name: &'a str,
    /// The name it gets here: `--as`, else `name`.
    pub target: &'a str,
    /// The source workspace, as `--from` gave it.
    pub from: &'a Path,
    /// The canonical current directory, which anchors the new session.
    pub anchor: &'a Path,
    /// `--trust-template`: take the compiled template from the bucket.
    pub trust_template: bool,
    /// `--as` was given. A same-host takeover of the source's own copy runs
    /// only without it; with it the import behaves as it always has.
    pub explicit_as: bool,
    /// The importing command's `KOTO_ANCHOR`: the anchor a held-by link
    /// whose anchor isn't open here is rewritten to (Decision 6 of
    /// DESIGN-session-retention.md). `None`, or a name that is no anchor
    /// here, drops such a link instead.
    pub importer_anchor: Option<&'a str>,
}

/// What a completed import did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportOutcome {
    /// The new local session's name.
    pub name: String,
    /// The source workspace, as recorded in the `session_imported` event.
    pub from_workspace: String,
    /// The source session's name.
    pub from_session: String,
    /// How many context keys came across.
    pub keys: usize,
    /// Where the compiled template came from.
    pub template: TemplateOrigin,
}

/// The source's state log, parsed for validation but kept as text so its
/// events are carried byte for byte.
struct SourceLog {
    header: crate::engine::types::StateFileHeader,
    /// Every event line, verbatim, without its line terminator.
    event_lines: Vec<String>,
    /// The last event's `seq`.
    last_seq: u64,
}

/// Parse and check a source state file.
///
/// Refuses (with the reason) a log this build can't carry intact: not
/// UTF-8, no header, a `schema_version` other than 1, an event line that
/// doesn't parse, or a gap in the sequence numbers. Blank lines are
/// dropped. The `template_hash` is checked by the caller, after it has
/// refused a child session, whose hash may legitimately be empty.
///
/// The marker prune reads both a migrated source's log and its target's
/// with it too, so "reads to the end" means the same there as here; the
/// name and [`SourceLog`] say "source" because the import came first.
fn parse_source_log(bytes: &[u8]) -> Result<SourceLog, String> {
    use crate::engine::types::{Event, StateFileHeader};

    let text = std::str::from_utf8(bytes).map_err(|_| "the state file is not UTF-8".to_string())?;
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header_line = lines.next().ok_or("the state file is empty")?;
    let header: StateFileHeader = serde_json::from_str(header_line)
        .map_err(|e| format!("the state file's header doesn't parse: {}", e))?;
    if header.schema_version != 1 {
        return Err(format!(
            "the state file has schema_version {}; this koto reads version 1",
            header.schema_version
        ));
    }
    let mut event_lines = Vec::new();
    let mut last_seq = 0;
    for (i, line) in lines.enumerate() {
        let event: Event = serde_json::from_str(line)
            .map_err(|e| format!("event {} doesn't parse: {}", i + 1, e))?;
        if event.seq != last_seq + 1 {
            return Err(format!(
                "event {} has seq {}, expected {}",
                i + 1,
                event.seq,
                last_seq + 1
            ));
        }
        last_seq = event.seq;
        event_lines.push(line.to_string());
    }
    Ok(SourceLog {
        header,
        event_lines,
        last_seq,
    })
}

/// The marker an import writes beside the source session.
#[derive(serde::Serialize)]
struct MigrationMarker<'a> {
    schema: u32,
    target: MarkerTarget<'a>,
    machine_id: &'a str,
    migrated_at: String,
}

#[derive(serde::Serialize)]
struct MarkerTarget<'a> {
    session: &'a str,
    session_id: &'a str,
    workspace: String,
    prefix: &'a str,
}

/// The source session an import reads: where its objects are and its
/// parsed log.
struct ImportSource {
    /// The session's name in the source workspace.
    name: String,
    /// The source workspace, canonical when it resolves here.
    workspace: String,
    /// `<source prefix>/<name>/`, the source session's key prefix.
    session: String,
    /// The source workspace's prefix alone.
    prefix: String,
    log: SourceLog,
    /// The source's marker, when it names this workspace's prefix and the
    /// import's target: an earlier run of this import may have written it
    /// and then been told the PUT failed.
    own_marker: Option<Vec<u8>>,
}

impl ImportSource {
    /// The bucket key of `rest` under the source session.
    fn key(&self, rest: &str) -> String {
        format!("{}{}", self.session, rest)
    }

    /// The refusal for a source object that couldn't be read or failed
    /// validation.
    fn unreadable(&self, what: &str, e: &dyn std::fmt::Display) -> ImportError {
        unreadable_source(&self.name, &self.workspace, what, e)
    }
}

fn unreadable_source(
    name: &str,
    workspace: &str,
    what: &str,
    e: &dyn std::fmt::Display,
) -> ImportError {
    ImportError::new(
        ImportErrorCode::SourceUnreadable,
        format!(
            "could not read {} of session '{}' from workspace {}: {}",
            what, name, workspace, e
        ),
    )
}

/// The refusal for a source that already carries another import's marker.
fn already_migrated(name: &str, workspace: &str, marker: &[u8]) -> ImportError {
    let migrated = migrated_from_marker(name, marker);
    ImportError::new(
        ImportErrorCode::SourceMigrated,
        format!(
            "session '{}' in workspace {} was already migrated to '{}' in {}",
            name, workspace, migrated.target, migrated.workspace
        ),
    )
}

/// What already holds the import's target name.
enum ExistingTarget {
    /// Nothing, or only an earlier run of this same import that pushed the
    /// target but never moved it into place here (it stopped between the
    /// push and the rename). Either way the import runs in full, writing
    /// over any such copy key for key.
    Free,
    /// An earlier run of this same import, complete on this machine
    /// (it stopped at `import_unmarked`). Only the marker is missing.
    Local {
        /// The `session_id` that run gave the target.
        session_id: String,
    },
}

/// The `session_imported` event a log ends its imports with, and the
/// header's `session_id`: enough to tell whether a log is an earlier run of
/// a given import.
struct ImportedAs {
    session_id: String,
    from_workspace: String,
    from_session: String,
    from_session_id: String,
}

impl ImportedAs {
    /// Read from a state log's bytes; `None` when the log doesn't parse or
    /// records no import.
    fn read(bytes: &[u8]) -> Option<Self> {
        use crate::engine::types::{Event, EventPayload, StateFileHeader};

        let text = std::str::from_utf8(bytes).ok()?;
        let mut lines = text.lines().filter(|l| !l.trim().is_empty());
        let header: StateFileHeader = serde_json::from_str(lines.next()?).ok()?;
        let mut last = None;
        for line in lines {
            if let Ok(Event {
                payload:
                    EventPayload::SessionImported {
                        from_workspace,
                        from_session,
                        from_session_id,
                        ..
                    },
                ..
            }) = serde_json::from_str::<Event>(line)
            {
                last = Some((from_workspace, from_session, from_session_id));
            }
        }
        let (from_workspace, from_session, from_session_id) = last?;
        Some(ImportedAs {
            session_id: header.session_id,
            from_workspace,
            from_session,
            from_session_id,
        })
    }

    /// Whether this log's latest import is of `source`. Workspace, name and
    /// session id must all agree: a log written before koto recorded
    /// session ids has an empty one, which alone would match any other.
    fn is_of(&self, source: &ImportSource) -> bool {
        self.from_workspace == source.workspace
            && self.from_session == source.name
            && self.from_session_id == source.log.header.session_id
    }
}

/// The directory an import builds its target in, inside the session store.
/// Removed when dropped, unless it was moved into place.
///
/// A process killed mid-import can leave one behind as
/// `<sessions>/.import-<target>-<random>/`. Nothing reads it and no
/// session is named like it (a session name starts with a letter), so it
/// is safe to delete by hand.
struct Staging {
    dir: PathBuf,
    moved: bool,
}

/// What [`CloudBackend::local_guard`] last answered for a name, and the
/// state file it read.
type LocalCheck = (Option<(u64, u64)>, Option<SessionMigrated>);

/// The device and inode of `path`, or `None` when it can't be read or the
/// platform has none.
fn file_identity(path: &Path) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.moved {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// Create the directory `path`, mode 0700, failing when anything is
/// already there: not recursive, so the directory is the caller's alone.
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

#[path = "cloud_takeover.rs"]
mod takeover;

#[path = "cloud_workspace_read.rs"]
pub mod workspace_read;

impl CloudBackend {
    /// Import session `req.name` from the workspace at `req.from` into this
    /// workspace as `req.target`, anchored at `req.anchor`.
    ///
    /// Reads and checks the source's remote objects; builds the target in
    /// a staging directory inside the session store, with the source's
    /// events plus a `session_imported` event, its context keys and
    /// manifest verbatim, and the compiled template; pushes it under this
    /// workspace's prefix; renames it into place; and only then writes
    /// `migrated.json` beside the source. A failure before the rename
    /// removes the staging directory and exactly the objects this run
    /// pushed. The import never writes any other object under the
    /// source's prefix.
    ///
    /// Rollback ends at the rename: once the target is in place, the
    /// marker step never takes anything back, whatever it reports.
    ///
    /// A re-run of an import that stopped at `import_unmarked` writes only
    /// the marker; one whose target reached the bucket but not this
    /// machine builds and pushes it again.
    ///
    /// It is defined for a stopped source: no process advancing the
    /// session, and its last write on the remote. Nothing here can tell a
    /// source that is still moving.
    pub fn import_session(&self, req: &ImportRequest<'_>) -> Result<ImportOutcome, ImportError> {
        let source = self.read_import_source(req)?;
        let outcome = |keys: usize, template: TemplateOrigin| ImportOutcome {
            name: req.target.to_string(),
            from_workspace: source.workspace.clone(),
            from_session: source.name.clone(),
            keys,
            template,
        };

        // The source is marked for this very target: an earlier run's
        // marker PUT landed though it reported a failure. With that run's
        // target here, the import is already complete; nothing is written.
        if let Some(marker) = &source.own_marker {
            // A same-host takeover that marked the source but stopped before
            // the exchange: the source's own copy still holds the name.
            // Finish it with the session id the marker recorded.
            if self.is_takeover(req, &source) {
                let session_id = marker_target_field(marker, "session_id");
                return self.takeover(req, &source, Some(&session_id));
            }
            return match self.local_import_of(req.target, &source) {
                Some(session_id) if session_id == marker_target_field(marker, "session_id") => {
                    // A takeover that stopped after the exchange leaves its
                    // `.takeover-<name>` to journal and delete.
                    if std::fs::symlink_metadata(takeover_dir(self.local.base_dir(), req.target))
                        .is_ok()
                    {
                        self.finish_takeover(req.target, &source, None);
                    }
                    Ok(outcome(
                        self.local_key_count(req.target)?,
                        TemplateOrigin::Unchanged,
                    ))
                }
                _ => {
                    let local = self.local.session_dir(req.target);
                    let (what, hint) = if local.exists() {
                        (
                            "the session here is a different one".to_string(),
                            format!(
                                "the conflict is the local session at {}: import again under \
                                 --as <new-name>, or remove that session if it is not wanted",
                                local.display()
                            ),
                        )
                    } else {
                        (
                            "that session is missing here".to_string(),
                            format!(
                                "remove '{}' from this workspace's prefix in the bucket, or \
                                 import again under --as <new-name>",
                                req.target
                            ),
                        )
                    };
                    Err(ImportError::new(
                        ImportErrorCode::SourceMigrated,
                        format!(
                            "session '{}' in workspace {} carries this import's own marker, \
                             naming '{}' in this workspace, but {}; {}",
                            source.name, source.workspace, req.target, what, hint
                        ),
                    ))
                }
            };
        }

        if self.is_takeover(req, &source) {
            return self.takeover(req, &source, None);
        }

        let (session_id, machine_id, keys, template_origin) =
            match self.existing_target(req, &source)? {
                ExistingTarget::Local { session_id } => (
                    session_id,
                    import_machine_id()?,
                    self.local_key_count(req.target)?,
                    TemplateOrigin::Unchanged,
                ),
                ExistingTarget::Free => {
                    // The manifest first: it refuses a close session as a
                    // source before the template is looked for.
                    let manifest = self.read_source_manifest(&source)?;
                    let template = self.import_template(req, &source)?;
                    let machine_id = import_machine_id()?;
                    let (mut staging, session_id, manifest) =
                        self.stage_import(req, &source, &template, manifest, &machine_id, None)?;
                    let pushed = self.push_staged(req.target, &staging.dir, &source, &manifest)?;
                    self.move_into_place(req.target, &mut staging, &pushed)?;
                    // The session is in place: journal it as a new run, with
                    // the state its carried log leaves it in. This comes
                    // before the marker, since a marker failure leaves the
                    // imported session where it is, and a retry that adopts
                    // it takes the `Local` branch above and journals nothing
                    // further. A rollback never reaches here.
                    if let Ok((header, events)) = self.local.read_events(req.target) {
                        let current = crate::engine::persistence::derive_state_from_log(&events);
                        crate::run_journal::imported(
                            self.local.journal_root(),
                            &self.local,
                            req.target,
                            &header,
                            current.as_deref(),
                            &source.log.header.session_id,
                        );
                    }
                    let origin = if req.trust_template {
                        TemplateOrigin::Bucket
                    } else {
                        TemplateOrigin::LocalCache
                    };
                    (session_id, machine_id, carried_key_count(&manifest), origin)
                }
            };

        self.mark_source(req, &source, &session_id, &machine_id)?;
        Ok(outcome(keys, template_origin))
    }

    /// The `session_id` of the local session `target` when its log is an
    /// import of `source`.
    fn local_import_of(&self, target: &str, source: &ImportSource) -> Option<String> {
        let bytes =
            std::fs::read(self.local.session_dir(target).join(state_file_name(target))).ok()?;
        let imported = ImportedAs::read(&bytes)?;
        imported.is_of(source).then_some(imported.session_id)
    }

    /// How many keys the local session `target`'s manifest lists, leaving
    /// out the host-local ones the import never carries, so a re-run reports
    /// what the first run did. A manifest that can't be read fails the
    /// import rather than reporting none.
    fn local_key_count(&self, target: &str) -> Result<usize, ImportError> {
        self.local
            .read_manifest(target)
            .map(|mut m| {
                m.drop_host_local();
                carried_key_count(&m)
            })
            .map_err(|e| {
                ImportError::new(
                    ImportErrorCode::PushFailed,
                    format!(
                        "could not read the context manifest of the imported session '{}': {:#}",
                        target, e
                    ),
                )
            })
    }

    /// Read and check the source: refuse a marked one, then fetch its state
    /// file and parse all of it before anything is written.
    fn read_import_source(&self, req: &ImportRequest<'_>) -> Result<ImportSource, ImportError> {
        use ImportErrorCode::*;

        let name = req.name;
        let source_prefix = crate::session::local::prefix_for_workspace(req.from)
            .map_err(|e| ImportError::new(SourceNotFound, format!("--from: {}", e)))?;
        let workspace = std::fs::canonicalize(req.from)
            .unwrap_or_else(|_| req.from.to_path_buf())
            .to_string_lossy()
            .into_owned();
        let session = format!("{}/{}/", source_prefix, name);

        // A source already marked was imported before. A marker naming this
        // workspace's prefix and this import's target may be this import's
        // own; that is settled once the source's log is read.
        let marker = self
            .get_object_if_present(&format!("{}{}", session, MIGRATED_MARKER))
            .map_err(|e| unreadable_source(name, &workspace, "the migration marker", &e))?;
        let own_marker = match &marker {
            None => None,
            Some(bytes) => {
                if marker_target_field(bytes, "prefix") != self.prefix
                    || marker_target_field(bytes, "session") != req.target
                {
                    return Err(already_migrated(name, &workspace, bytes));
                }
                Some(bytes.clone())
            }
        };

        // A batch the source's host hasn't flushed: its copy in the bucket
        // lacks writes the host holds, and an import would carry the copy
        // without them and mark the source, silently. It refuses rather than
        // flushes: the writes are in a journal on the source's host, out of
        // reach from here. A source this import already marked is past that
        // point.
        if own_marker.is_none()
            && self
                .object_listed(&format!("{}{}", session, BATCH_MARKER))
                .map_err(|e| unreadable_source(name, &workspace, "the batch-open marker", &e))?
        {
            return Err(ImportError::new(
                SourceUnpushed,
                format!(
                    "session '{}' in workspace {} has writes its host hasn't pushed to the \
                     bucket yet (a `koto next` stopped before it pushed them); run any koto \
                     command on the session in that workspace, such as `koto status {}`, to \
                     push them, then import again",
                    name, workspace, name
                ),
            ));
        }

        let state_bytes = self
            .get_object_if_present(&format!("{}{}", session, state_file_name(name)))
            .map_err(|e| unreadable_source(name, &workspace, "the state file", &e))?;
        let state_bytes = match (state_bytes, &marker) {
            (Some(bytes), _) => bytes,
            (None, Some(bytes)) => return Err(already_migrated(name, &workspace, bytes)),
            (None, None) => {
                return Err(ImportError::new(
                    SourceNotFound,
                    format!(
                        "workspace {} has no session '{}' in the bucket; pass the source \
                         workspace's absolute path as it is on the host that created the session",
                        workspace, name
                    ),
                ))
            }
        };
        let log = parse_source_log(&state_bytes)
            .map_err(|e| unreadable_source(name, &workspace, "the state file", &e))?;
        if let Some(parent) = &log.header.parent_workflow {
            return Err(ImportError::new(
                SourceIsChild,
                format!(
                    "session '{}' is a child of '{}'; a child session is not imported on its own",
                    name, parent
                ),
            ));
        }
        // The hash becomes a file name, so it must be one.
        if !crate::engine::persistence::is_template_hash(&log.header.template_hash) {
            return Err(unreadable_source(
                name,
                &workspace,
                "the state file",
                &"its header's template_hash is not 64 lowercase hex digits",
            ));
        }
        Ok(ImportSource {
            name: name.to_string(),
            workspace,
            session,
            prefix: source_prefix,
            log,
            own_marker,
        })
    }

    /// Whether the target name is free, here and under this workspace's
    /// prefix. A session holding it is an earlier run of this import when
    /// the latest `session_imported` event in its log names this source;
    /// anything else holding it refuses `import_name_taken`.
    fn existing_target(
        &self,
        req: &ImportRequest<'_>,
        source: &ImportSource,
    ) -> Result<ExistingTarget, ImportError> {
        use ImportErrorCode::*;

        let target = req.target;
        let taken = |place: &str| {
            ImportError::new(
                NameTaken,
                format!(
                    "{} already has a session named '{}', and it is not an import of session \
                     '{}' from {}; pass --as <new-name> to import it under another name",
                    place, target, source.name, source.workspace
                ),
            )
        };

        let dir = self.local.session_dir(target);
        if dir.exists() {
            return match std::fs::read(dir.join(state_file_name(target)))
                .ok()
                .and_then(|bytes| ImportedAs::read(&bytes))
            {
                Some(imported) if imported.is_of(source) => Ok(ExistingTarget::Local {
                    session_id: imported.session_id,
                }),
                _ => Err(taken("this machine")),
            };
        }

        self.remote_name_free(req, source)?;
        Ok(ExistingTarget::Free)
    }

    /// Whether this workspace's prefix in the bucket leaves the target name
    /// free: nothing there, or only an earlier run of this same import.
    /// Anything else refuses `import_name_taken`.
    fn remote_name_free(
        &self,
        req: &ImportRequest<'_>,
        source: &ImportSource,
    ) -> Result<(), ImportError> {
        use ImportErrorCode::*;

        let target = req.target;
        let taken = |place: &str| {
            ImportError::new(
                NameTaken,
                format!(
                    "{} already has a session named '{}', and it is not an import of session \
                     '{}' from {}; pass --as <new-name> to import it under another name",
                    place, target, source.name, source.workspace
                ),
            )
        };
        let key = self.state_key(target);
        let could_not_check = |e: &dyn std::fmt::Display| {
            ImportError::new(
                PushFailed,
                format!(
                    "could not check this workspace's prefix for a session named '{}': {}",
                    target, e
                ),
            )
        };
        match self.object_listed(&key) {
            Ok(false) => Ok(()),
            Ok(true) => match self.fetch_object(&key) {
                Ok(bytes) => match bytes.as_deref().and_then(ImportedAs::read) {
                    Some(imported) if imported.is_of(source) => Ok(()),
                    _ => Err(taken("this workspace's prefix in the bucket")),
                },
                Err(e) => Err(could_not_check(&e)),
            },
            Err(e) => Err(could_not_check(&e)),
        }
    }

    /// The compiled template, checked against the header's `template_hash`
    /// and parsed: from this machine's cache, or under `--trust-template`
    /// from the source's `template.json`. Without the flag the bucket's
    /// copy is never fetched.
    fn import_template(
        &self,
        req: &ImportRequest<'_>,
        source: &ImportSource,
    ) -> Result<Vec<u8>, ImportError> {
        use ImportErrorCode::*;

        let hash = &source.log.header.template_hash;
        let usable = |bytes: Vec<u8>| -> Result<Vec<u8>, String> {
            let actual = crate::cache::sha256_hex(&bytes);
            if &actual != hash {
                return Err(format!("its contents hash to {}", actual));
            }
            serde_json::from_slice::<crate::template::types::CompiledTemplate>(&bytes)
                .map_err(|e| format!("it doesn't parse as a compiled template: {}", e))?;
            Ok(bytes)
        };

        if req.trust_template {
            let key = source.key(TEMPLATE_OBJECT);
            let bytes = self
                .get_object_if_present(&key)
                .map_err(|e| source.unreadable("the compiled template", &e))?;
            return bytes
                .ok_or_else(|| "the source has none in the bucket".to_string())
                .and_then(usable)
                .map_err(|why| {
                    ImportError::new(
                        TemplateUnavailable,
                        format!(
                            "the source's {} can't be used as the compiled template with hash \
                             {}: {}; import without --trust-template after running \
                             `koto template compile` here on the session's template",
                            TEMPLATE_OBJECT, hash, why
                        ),
                    )
                });
        }

        let template_file = crate::cache::cache_dir().join(format!("{}.json", hash));
        let cached = std::fs::read(&template_file)
            .map_err(|_| "it is not there".to_string())
            .and_then(usable);
        // An anchor's template is embedded in koto and never reaches the
        // cache: this build's copy serves when its hash is the anchor's.
        let cached = match cached {
            Err(why)
                if source.log.header.session_kind()
                    == crate::engine::types::SessionKind::Anchor =>
            {
                crate::session::anchor::compiled_anchor_template()
                    .map_err(|e| e.to_string())
                    .and_then(usable)
                    .map_err(|_| why)
            }
            other => other,
        };
        cached.map_err(|why| {
            let source_file = source
                .log
                .header
                .template_source_file
                .as_deref()
                .map(|f| format!(" ({})", f))
                .unwrap_or_default();
            ImportError::new(
                TemplateUnavailable,
                format!(
                    "this machine has no usable compiled template with hash {} at {}: {}; \
                         run `koto template compile` here on the session's template{} from a \
                         checkout that matches it, then import again",
                    hash,
                    template_file.display(),
                    why,
                    source_file
                ),
            )
        })
    }

    /// The source's context manifest without the keys the import leaves
    /// behind, with every key name checked before any key is fetched.
    ///
    /// It refuses a source whose header has `kind: "anchor-close"`: a close
    /// session records a close on its own host, and a copy elsewhere could
    /// only close an anchor early. The bucket is untrusted for the reserved
    /// area, so besides the host-local keys it drops the observed marker,
    /// any close record, every `koto/links/closes/*` link and any
    /// `koto/imported-from` (see [`crate::session::import_links`]).
    fn read_source_manifest(
        &self,
        source: &ImportSource,
    ) -> Result<crate::session::context::Manifest, ImportError> {
        use crate::session::context::Manifest;
        use crate::session::validate::validate_context_key;

        if source.log.header.session_kind() == crate::engine::types::SessionKind::AnchorClose {
            return Err(ImportError::new(
                ImportErrorCode::SourceIsAnchorClose,
                format!(
                    "session '{}' in workspace {} is an anchor's close session (its header has \
                     kind \"anchor-close\"); a close session records a close on its own host and \
                     is never imported",
                    source.name, source.workspace
                ),
            ));
        }
        let bytes = self
            .get_object_if_present(&source.key("ctx/manifest.json"))
            .map_err(|e| source.unreadable("the context manifest", &e))?;
        let mut manifest: Manifest = match &bytes {
            Some(bytes) => serde_json::from_slice(bytes)
                .map_err(|e| source.unreadable("the context manifest", &e))?,
            None => Manifest::default(),
        };
        // An older koto listed host-local keys, often without their
        // objects. They describe the source's host, so the import leaves
        // them behind and the new session resolves its own.
        manifest.drop_host_local();
        crate::session::import_links::drop_from_manifest(&mut manifest);
        for key in manifest.keys.keys() {
            validate_context_key(key)
                .map_err(|e| source.unreadable(&format!("context key {:?}", key), &e))?;
        }
        Ok(manifest)
    }

    /// Build the target in `<sessions>/.import-<target>-<random>/`, created
    /// exclusively with mode 0700: the state log, the context keys (each
    /// checked against the manifest), the manifest, the compiled template
    /// and a fresh version record. Returns the staging directory, removed
    /// when dropped, the target's new `session_id`, and the manifest the
    /// copy holds.
    ///
    /// After every key is checked against the source's manifest and before
    /// the staged manifest is written, the copy's reserved area is settled
    /// (see [`crate::session::import_links`]): each link body is validated,
    /// the `koto/links/held-by/*` links are kept, rewritten to
    /// `req.importer_anchor` or dropped, and `koto/imported-from` is
    /// written. The manifest is then serialised from what the copy holds, so
    /// the push and any later verification see the changed keys.
    ///
    /// A source with no manifest gets an empty one, so every imported
    /// session has a `ctx/` manifest.
    fn stage_import(
        &self,
        req: &ImportRequest<'_>,
        source: &ImportSource,
        template: &[u8],
        mut manifest: crate::session::context::Manifest,
        machine_id: &str,
        into: Option<(&Path, Option<&str>)>,
    ) -> Result<(Staging, String, crate::session::context::Manifest), ImportError> {
        use crate::engine::types::{
            generate_session_id, now_iso8601, Event, EventPayload, SessionOrigin,
        };
        use ImportErrorCode::*;

        let target = req.target;
        let write_failed = |what: &Path, e: &dyn std::fmt::Display| {
            ImportError::new(
                PushFailed,
                format!("could not write {}: {}", what.display(), e),
            )
        };

        let base = self.local.base_dir();
        crate::session::local::ensure_koto_root(base)
            .and_then(|()| std::fs::create_dir_all(base).map_err(anyhow::Error::from))
            .map_err(|e| write_failed(base, &e))?;
        // A same-host takeover passes its own staging directory, which it
        // renames to `.takeover-<name>` once complete, and may reuse the
        // session id its marker already names; every other import stages in
        // a fresh `.import-<target>-<random>` with a new id.
        let dir = match into {
            Some((dir, _)) => dir.to_path_buf(),
            None => base.join(format!(".import-{}-{}", target, generate_session_id())),
        };
        create_private_dir(&dir).map_err(|e| write_failed(&dir, &e))?;
        let staging = Staging { dir, moved: false };
        let dir = staging.dir.as_path();

        // Context keys, one fetched and written at a time. The manifest says
        // each exists, so each is one GET with no listing first.
        let ctx_dir = dir.join("ctx");
        std::fs::create_dir_all(&ctx_dir).map_err(|e| write_failed(&ctx_dir, &e))?;
        for (key, meta) in &manifest.keys {
            let what = format!("context key {:?}", key);
            let bytes = self
                .fetch_object(&source.key(&format!("ctx/{}", key)))
                .map_err(|e| source.unreadable(&what, &e))?
                .ok_or_else(|| {
                    source.unreadable(&what, &"the manifest lists it but the object is missing")
                })?;
            if bytes.len() as u64 != meta.size || crate::cache::sha256_hex(&bytes) != meta.hash {
                return Err(
                    source.unreadable(&what, &"its size or SHA-256 doesn't match the manifest")
                );
            }
            let path = ctx_dir.join(key);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| write_failed(parent, &e))?;
            }
            std::fs::write(&path, &bytes).map_err(|e| write_failed(&path, &e))?;
        }
        crate::session::import_links::Settlement {
            local_base: base,
            importer_anchor: req.importer_anchor,
            target,
            from_session: &source.name,
            from_session_id: &source.log.header.session_id,
        }
        .apply(&ctx_dir, &mut manifest)
        .map_err(|e| write_failed(&ctx_dir, &e))?;
        let manifest_bytes =
            serde_json::to_vec_pretty(&manifest).expect("Manifest serialize is infallible");
        let path = ctx_dir.join("manifest.json");
        std::fs::write(&path, manifest_bytes).map_err(|e| write_failed(&path, &e))?;

        let hash = &source.log.header.template_hash;
        let path = dir.join(format!("{}.json", hash));
        std::fs::write(&path, template).map_err(|e| write_failed(&path, &e))?;

        let path = dir.join("version.json");
        crate::session::version::SessionVersion::new(machine_id.to_string())
            .save(&path)
            .map_err(|e| write_failed(&path, &e))?;

        // The header is the source's, renamed, re-identified and
        // re-anchored here, with a command environment taken on this host.
        let session_id = into
            .and_then(|(_, id)| id)
            .map(str::to_string)
            .unwrap_or_else(generate_session_id);
        let mut header = source.log.header.clone();
        header.workflow = target.to_string();
        header.session_id = session_id.clone();
        header.execution_dir = Some(req.anchor.to_path_buf());
        header.origin = self.store_identity().map(|store| SessionOrigin {
            anchor: req.anchor.to_path_buf(),
            store,
        });
        header.command_environment = Some(crate::engine::command_env::record_from_process(false).0);

        let payload = EventPayload::SessionImported {
            from_workspace: source.workspace.clone(),
            from_session: source.name.clone(),
            from_session_id: source.log.header.session_id.clone(),
            machine_id: machine_id.to_string(),
        };
        let imported = Event {
            seq: source.log.last_seq + 1,
            timestamp: now_iso8601(),
            event_type: payload.type_name().to_string(),
            payload,
            idempotency_hash: None,
        };

        let mut log =
            serde_json::to_string(&header).expect("StateFileHeader serialize is infallible");
        log.push('\n');
        for line in &source.log.event_lines {
            log.push_str(line);
            log.push('\n');
        }
        log.push_str(&serde_json::to_string(&imported).expect("Event serialize is infallible"));
        log.push('\n');
        let path = dir.join(state_file_name(target));
        std::fs::write(&path, log).map_err(|e| write_failed(&path, &e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| write_failed(&path, &e))?;
        }

        Ok((staging, session_id, manifest))
    }

    /// Push the staged target under this workspace's prefix with the strict
    /// push: state file, template, each key, manifest, version record. On a
    /// failure, delete exactly what this run pushed and refuse
    /// `import_push_failed`. Returns the keys pushed.
    fn push_staged(
        &self,
        target: &str,
        staged: &Path,
        source: &ImportSource,
        manifest: &crate::session::context::Manifest,
    ) -> Result<Vec<String>, ImportError> {
        let prefix = self.session_prefix(target);
        let hash = &source.log.header.template_hash;
        let mut uploads = vec![
            (self.state_key(target), staged.join(state_file_name(target))),
            (
                self.template_key(target),
                staged.join(format!("{}.json", hash)),
            ),
        ];
        for key in manifest.keys.keys() {
            uploads.push((
                format!("{}ctx/{}", prefix, key),
                staged.join("ctx").join(key),
            ));
        }
        uploads.push((
            format!("{}ctx/manifest.json", prefix),
            staged.join("ctx").join("manifest.json"),
        ));
        uploads.push((self.version_key(target), staged.join("version.json")));

        let mut pushed = Vec::new();
        for (key, path) in uploads {
            let result = std::fs::read(&path)
                .with_context(|| format!("reading {}", path.display()))
                .and_then(|data| self.put_object(&key, &data));
            if let Err(e) = result {
                self.take_back(&pushed);
                return Err(ImportError::new(
                    ImportErrorCode::PushFailed,
                    format!("could not push the imported session: {:#}", e),
                ));
            }
            pushed.push(key);
        }
        Ok(pushed)
    }

    /// Delete objects an import pushed before it failed.
    ///
    /// On a re-run over a target an earlier run left only in the bucket,
    /// the keys deleted can be ones that run pushed too, the state file
    /// included: this run wrote over them. The next run then finds the name
    /// free and imports afresh over whatever is left.
    fn take_back(&self, pushed: &[String]) {
        for key in pushed {
            if let Err(e) = self.delete_object(key) {
                eprintln!(
                    "warning: cloud sync: failed to remove {} after a failed import: {}",
                    key, e
                );
            }
        }
    }

    /// Rename the staging directory to `<sessions>/<target>/`. No lock is
    /// taken: the staging directory is private and complete, and the rename
    /// is atomic, so nothing can open the target before it exists whole.
    ///
    /// The rename never replaces anything, not even an empty directory: a
    /// target that appeared while the import ran refuses
    /// `import_name_taken`. On any failure, take back what this run pushed;
    /// the staging directory goes when `staging` drops.
    fn move_into_place(
        &self,
        target: &str,
        staging: &mut Staging,
        pushed: &[String],
    ) -> Result<(), ImportError> {
        use crate::engine::atomic_fs::{atomic_rename_dir, AtomicCreateError};

        let target_dir = self.local.session_dir(target);
        match atomic_rename_dir(&staging.dir, &target_dir) {
            Ok(()) => {
                staging.moved = true;
                Ok(())
            }
            Err(AtomicCreateError::Collision) => {
                self.take_back(pushed);
                Err(ImportError::new(
                    ImportErrorCode::NameTaken,
                    format!(
                        "{} appeared while the import ran; what this import pushed was taken \
                         back. Pass --as <new-name> to import under another name",
                        target_dir.display()
                    ),
                ))
            }
            Err(AtomicCreateError::Io(e)) => {
                self.take_back(pushed);
                Err(ImportError::new(
                    ImportErrorCode::PushFailed,
                    format!(
                        "could not move the imported session to {}: {}",
                        target_dir.display(),
                        e
                    ),
                ))
            }
        }
    }

    /// Mark the source as migrated to the target, last. The marker is read
    /// again first: when another import marked the source while this one
    /// ran, this one keeps its target and refuses `import_source_migrated`
    /// naming the other, leaving the operator to remove one of the two. The
    /// re-read narrows that window but doesn't close it; the stopped-source
    /// rule is what rules out two imports at once.
    ///
    /// A marker that can't be read or written refuses `import_unmarked`,
    /// keeping the target: running the same import again writes only the
    /// marker.
    fn mark_source(
        &self,
        req: &ImportRequest<'_>,
        source: &ImportSource,
        session_id: &str,
        machine_id: &str,
    ) -> Result<(), ImportError> {
        use ImportErrorCode::*;

        let unmarked = |e: &dyn std::fmt::Display| {
            ImportError::new(
                Unmarked,
                format!(
                    "session '{}' was imported and pushed as '{}', but the marker on the source \
                     could not be written, so the source's host will not refuse it: {}; run the \
                     same import again to write it",
                    source.name, req.target, e
                ),
            )
        };

        let key = source.key(MIGRATED_MARKER);
        match self.get_object_if_present(&key) {
            Ok(None) => {}
            Ok(Some(bytes)) => {
                let other = migrated_from_marker(&source.name, &bytes);
                return Err(ImportError::new(
                    SourceMigrated,
                    format!(
                        "while this import ran, another import marked session '{}' in workspace \
                         {} as migrated to '{}' in {}; this import's copy, '{}' in {}, is kept. \
                         Continue one of the two and remove the other with \
                         `koto session cleanup`",
                        source.name,
                        source.workspace,
                        other.target,
                        other.workspace,
                        req.target,
                        req.anchor.display()
                    ),
                ));
            }
            Err(e) => return Err(unmarked(&format!("{:#}", e))),
        }

        let marker = MigrationMarker {
            schema: 1,
            target: MarkerTarget {
                session: req.target,
                session_id,
                workspace: req.anchor.to_string_lossy().into_owned(),
                prefix: &self.prefix,
            },
            machine_id,
            migrated_at: crate::engine::types::now_iso8601(),
        };
        let bytes = serde_json::to_vec(&marker).expect("marker serialize is infallible");
        self.put_object(&key, &bytes)
            .map_err(|e| unmarked(&format!("{:#}", e)))
    }
}

/// The import output's `keys`: the context keys the copy holds, leaving out
/// the `koto/imported-from` record the import itself writes, so the count
/// is of what came across.
fn carried_key_count(manifest: &crate::session::context::Manifest) -> usize {
    manifest
        .keys
        .keys()
        .filter(|k| k.as_str() != crate::session::import_links::IMPORTED_FROM_KEY)
        .count()
}

/// This machine's id, for the `session_imported` event, the target's
/// version record and the marker.
fn import_machine_id() -> Result<String, ImportError> {
    crate::session::version::get_or_create_machine_id().map_err(|e| {
        ImportError::new(
            ImportErrorCode::PushFailed,
            format!("could not read this machine's id: {}", e),
        )
    })
}

/// `text` with the userinfo (`user:password@`) taken out of every URL in
/// it, so an endpoint configured with credentials in its URL never prints
/// them.
pub(crate) fn without_url_userinfo(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("://") {
        let (head, tail) = rest.split_at(i + 3);
        out.push_str(head);
        let end = tail
            .find(|c: char| {
                matches!(c, '/' | '?' | '#' | '"' | '\'' | '<' | '>') || c.is_whitespace()
            })
            .unwrap_or(tail.len());
        let authority = &tail[..end];
        out.push_str(match authority.rfind('@') {
            Some(at) => &authority[at + 1..],
            None => authority,
        });
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// Construct an S3 `Bucket` from cloud configuration.
fn create_bucket(config: &CloudConfig) -> anyhow::Result<Box<Bucket>> {
    let region = Region::Custom {
        region: config.region.clone().unwrap_or_default(),
        endpoint: config.endpoint.clone().unwrap_or_default(),
    };
    let credentials = Credentials::new(
        config.access_key.as_deref(),
        config.secret_key.as_deref(),
        None,
        None,
        None,
    )?;
    let bucket = Bucket::new(
        config.bucket.as_deref().unwrap_or("koto-sessions"),
        region,
        credentials,
    )?;
    if config.path_style == Some(true) {
        return Ok(bucket.with_path_style());
    }
    Ok(bucket)
}

/// Whether an S3 error is a 404: the object (or bucket) isn't there.
fn is_not_found(e: &S3Error) -> bool {
    matches!(e, S3Error::HttpFailWithBody(404, _))
}

/// A response's `ETag` header, without its quotes; `None` when it has none.
fn response_etag(response: &s3::request::ResponseData) -> Option<String> {
    response
        .headers()
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("etag"))
        .map(|(_, value)| sync_stamp::normalize_etag(value))
        .filter(|etag| !etag.is_empty())
}

/// How many of a flush's independent requests (one per context key) are in
/// flight at once. Each waits a full round trip, so a batch of many keys
/// would otherwise cost the flush that many round trips in a row.
const FLUSH_PARALLELISM: usize = 8;

/// Run `f` on every item, [`FLUSH_PARALLELISM`] at a time, and return the
/// first error once every started call has finished.
fn in_parallel<T: Sync>(
    items: &[T],
    f: impl Fn(&T) -> anyhow::Result<()> + Sync,
) -> anyhow::Result<()> {
    for chunk in items.chunks(FLUSH_PARALLELISM) {
        let results: Vec<anyhow::Result<()>> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk.iter().map(|item| scope.spawn(|| f(item))).collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("a cloud sync request panicked")))
                })
                .collect()
        });
        results.into_iter().collect::<anyhow::Result<()>>()?;
    }
    Ok(())
}

/// Whether a sync failure must stop the command rather than warn: the
/// session was imported elsewhere, or another host's writes conflict with
/// this one's. Everything else (an unreachable bucket, a refused request)
/// leaves the local write standing and the push for later.
fn is_hard_sync_error(e: &anyhow::Error) -> bool {
    e.downcast_ref::<SessionMigrated>().is_some() || e.to_string().starts_with("session conflict:")
}

/// Whether `key` is the migration marker of the session whose objects live
/// under `session_prefix` (`<prefix>/<id>/`). Exactly that key: a context
/// key that happens to be named `migrated.json` lives under `ctx/` and is
/// not one.
fn is_migration_marker(session_prefix: &str, key: &str) -> bool {
    key.strip_prefix(session_prefix) == Some(MIGRATED_MARKER)
}

/// `<sessions>/.takeover-<id>`: where a same-host import of `id` stages the
/// new session, and where the source's old copy sits after the exchange
/// until the import deletes it. Its presence means the import of `id` has
/// not finished. Hidden from listings, and no session can be named like it
/// (a session name starts with a letter).
pub(crate) fn takeover_dir(sessions_base: &Path, id: &str) -> PathBuf {
    sessions_base.join(format!(".takeover-{}", id))
}

/// Refuse to write `remote` over the local state file at `state_path` when
/// the two are the two sides of an import: different session ids, and one
/// log's latest `session_imported` names the other's id as its source.
fn refuse_cross_import_pull(id: &str, state_path: &Path, remote: &[u8]) -> anyhow::Result<()> {
    let Ok(local) = std::fs::read(state_path) else {
        return Ok(());
    };
    let (Some(local_id), Some(remote_id)) = (header_session_id(&local), header_session_id(remote))
    else {
        return Ok(());
    };
    if local_id == remote_id || local_id.is_empty() || remote_id.is_empty() {
        return Ok(());
    }
    let imports =
        |log: &[u8], other: &str| ImportedAs::read(log).is_some_and(|i| i.from_session_id == other);
    if imports(remote, &local_id) || imports(&local, &remote_id) {
        anyhow::bail!(
            "refusing to pull session '{}' from the bucket over the local copy at {}: one is \
             the session the other was imported into (local session id {}, bucket session id \
             {}); run koto from the workspace the import went to",
            id,
            state_path.display(),
            local_id,
            remote_id
        );
    }
    Ok(())
}

/// The workspace a state log's header anchors it in: its origin anchor,
/// else its execution directory.
fn header_anchor(state: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(state);
    let line = text.lines().find(|l| !l.trim().is_empty())?;
    let header: serde_json::Value = serde_json::from_str(line).ok()?;
    header
        .pointer("/origin/anchor")
        .or_else(|| header.get("execution_dir"))?
        .as_str()
        .map(str::to_string)
}

/// The `session_id` in a state file's header line (its first non-empty
/// line), or `None` when the header doesn't parse or has none.
///
/// Deliberately looser than [`parse_source_log`]: the marker judge reads
/// the header with this first, to tell a target that is another session
/// (`Replaced`) or has no usable header (`Unknown`) from the right session
/// with a damaged log (`Unverified`). Reading it with the strict parser
/// would fold the first two into the third.
fn header_session_id(state: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(state);
    let line = text.lines().find(|l| !l.trim().is_empty())?;
    let header: serde_json::Value = serde_json::from_str(line).ok()?;
    header.get("session_id")?.as_str().map(str::to_string)
}

/// Build the refusal a marker's bytes describe for session `id`.
///
/// Read leniently: a field that is missing or of another shape reads as
/// `unknown` rather than failing, because the refusal must hold whatever a
/// marker's writer put in it.
fn migrated_from_marker(id: &str, bytes: &[u8]) -> SessionMigrated {
    SessionMigrated {
        name: id.to_string(),
        target: marker_target_field(bytes, "session"),
        workspace: marker_target_field(bytes, "workspace"),
    }
}

/// The string field `name` of a marker's `target`, read leniently: a marker
/// that doesn't parse, or a field that is missing or of another shape,
/// reads as `unknown`.
fn marker_target_field(bytes: &[u8], name: &str) -> String {
    serde_json::from_slice::<serde_json::Value>(bytes)
        .ok()
        .and_then(|m| m.get("target")?.get(name)?.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

/// An in-memory S3 stand-in and the fixtures that point a `CloudBackend` at
/// it, shared by the tests here and by the CLI tests that run a command
/// against a cloud-backed session.
#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use s3::creds::Credentials;
    use s3::{Bucket, Region};

    use super::CloudBackend;
    use crate::engine::types::StateFileHeader;
    use crate::session::local::LocalBackend;
    use crate::session::state_file_name;

    /// The key prefix `cloud_backend_at` gives its backend.
    pub(crate) const PREFIX: &str = "test-prefix";

    /// One request the endpoint answered: method, path (query stripped) and
    /// body.
    #[derive(Debug, Clone)]
    pub(crate) struct Request {
        pub(crate) method: String,
        pub(crate) path: String,
        pub(crate) body: Vec<u8>,
    }

    /// A running endpoint: its URL and every request it has answered.
    pub(crate) struct Endpoint {
        pub(crate) url: String,
        requests: Arc<Mutex<Vec<Request>>>,
    }

    impl Endpoint {
        /// Every request answered so far, in arrival order.
        pub(crate) fn requests(&self) -> Vec<Request> {
            self.requests.lock().unwrap().clone()
        }

        /// The body of the last PUT whose path ends with `suffix`.
        pub(crate) fn last_put(&self, suffix: &str) -> Option<Vec<u8>> {
            self.requests()
                .into_iter()
                .rev()
                .find(|r| r.method == "PUT" && r.path.ends_with(suffix))
                .map(|r| r.body)
        }
    }

    /// A ListObjectsV2 body naming `keys`, as rust-s3 parses it.
    pub(crate) fn list_body(keys: &[String]) -> Vec<u8> {
        let tagged: Vec<(String, String)> =
            keys.iter().map(|k| (k.clone(), String::new())).collect();
        list_body_tagged(&tagged)
    }

    /// [`list_body`] with each key's ETag; an empty one is left out.
    pub(crate) fn list_body_tagged(keys: &[(String, String)]) -> Vec<u8> {
        let mut xml = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult>\
             <Name>test-bucket</Name><IsTruncated>false</IsTruncated>",
        );
        for (key, etag) in keys {
            let etag = if etag.is_empty() {
                String::new()
            } else {
                format!("<ETag>\"{}\"</ETag>", etag)
            };
            xml.push_str(&format!(
                "<Contents><Key>{}</Key><LastModified>2026-01-01T00:00:00.000Z</LastModified>\
                 {}<Size>1</Size></Contents>",
                key, etag
            ));
        }
        xml.push_str("</ListBucketResult>");
        xml.into_bytes()
    }

    /// The ETag the endpoint gives `bytes`: a hash of the content, so a
    /// changed object gets a new one.
    pub(crate) fn etag_of(bytes: &[u8]) -> String {
        crate::session::sync_stamp::sha256_hex(bytes)[..16].to_string()
    }

    /// The key a stored object answers a listing of `prefix` with, when it
    /// falls under it. Objects are stored under the end of their path:
    /// a seeded `/wf/koto-wf.state.jsonl`, or a PUT's whole
    /// `/test-bucket/<key>`. The stored name is lined up with the end of
    /// `/test-bucket/<prefix>` at the longest overlap, and what follows the
    /// overlap is the rest of the key.
    pub(crate) fn listed_key(prefix: &str, name: &str) -> Option<String> {
        let full = format!("/test-bucket/{}", prefix);
        (1..=name.len())
            .rev()
            .filter(|&j| name.is_char_boundary(j))
            .find(|&j| full.ends_with(&name[..j]))
            .map(|j| format!("{}{}", prefix, &name[j..]))
    }

    /// The `prefix` of a ListObjectsV2 request target, or `None` when the
    /// target isn't a listing.
    pub(crate) fn listing_prefix(target: &str) -> Option<String> {
        let query = target.split_once('?')?.1;
        let pairs: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
        if !pairs.iter().any(|(k, v)| k == "list-type" && v == "2") {
            return None;
        }
        Some(
            pairs
                .into_iter()
                .find(|(k, _)| k == "prefix")
                .map(|(_, v)| v)
                .unwrap_or_default(),
        )
    }

    /// Serve `objects` from memory on a `127.0.0.1` port.
    ///
    /// A GET for a path ending in an object's name answers its bytes, and
    /// any other GET answers 404. A listing answers every stored object that
    /// falls under its prefix ([`listed_key`]): a listing of a session's
    /// prefix (the probe every read makes first) sees the objects seeded for
    /// it, and a listing of an object's whole key sees that object alone.
    /// Each listed object, each GET and each PUT's reply carries an ETag
    /// that hashes the content ([`etag_of`]). A PUT
    /// stores its body under its path, replacing whatever a GET of that
    /// path answered before, so a pull after a push reads back what was
    /// pushed. Every other request gets an empty 200. Each request is
    /// recorded, its path stripped of the query.
    ///
    /// rust-s3 retries a 404 once after a second's sleep, so seed every
    /// object a test GETs.
    pub(crate) fn serve(objects: Vec<(String, Vec<u8>)>) -> Endpoint {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        std::thread::spawn(move || {
            let mut objects = objects;
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut content_length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    let lower = line.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("content-length:") {
                        content_length = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; content_length];
                let _ = reader.read_exact(&mut body);
                let mut parts = request_line.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let target = parts.next().unwrap_or("");
                let path = target.split('?').next().unwrap_or("").to_string();
                let listing = listing_prefix(target);
                let mut etag = None;
                let (status, payload): (&str, Vec<u8>) = match (method.as_str(), listing) {
                    (_, Some(prefix)) => {
                        let mut keys: Vec<(String, String)> = objects
                            .iter()
                            .filter_map(|(name, bytes)| {
                                Some((listed_key(&prefix, name)?, etag_of(bytes)))
                            })
                            .collect();
                        keys.sort();
                        keys.dedup_by(|a, b| a.0 == b.0);
                        ("200 OK", list_body_tagged(&keys))
                    }
                    ("GET", None) => match objects
                        .iter()
                        .find(|(name, _)| path.ends_with(name.as_str()))
                    {
                        Some((_, bytes)) => {
                            etag = Some(etag_of(bytes));
                            ("200 OK", bytes.clone())
                        }
                        None => ("404 Not Found", Vec::new()),
                    },
                    ("PUT", None) => {
                        objects.retain(|(name, _)| !path.ends_with(name.as_str()));
                        objects.push((path.clone(), body.clone()));
                        etag = Some(etag_of(&body));
                        ("200 OK", Vec::new())
                    }
                    _ => ("200 OK", Vec::new()),
                };
                let etag = etag
                    .map(|e| format!("ETag: \"{}\"\r\n", e))
                    .unwrap_or_default();
                recorded
                    .lock()
                    .unwrap()
                    .push(Request { method, path, body });
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\n{etag}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                );
                let _ = stream.write_all(&payload);
            }
        });
        Endpoint {
            url: format!("http://{addr}"),
            requests,
        }
    }

    /// A `CloudBackend` storing sessions under `base_dir` and syncing to
    /// `endpoint` (path-style, under [`PREFIX`]).
    pub(crate) fn cloud_backend_at(base_dir: &Path, endpoint: String) -> CloudBackend {
        let region = Region::Custom {
            region: "us-east-1".to_string(),
            endpoint,
        };
        let credentials =
            Credentials::new(Some("test-key"), Some("test-secret"), None, None, None).unwrap();
        let bucket = Bucket::new("test-bucket", region, credentials)
            .unwrap()
            .with_path_style();
        let local = LocalBackend::with_base_dir(base_dir.to_path_buf());
        CloudBackend::with_parts(local, bucket, PREFIX.to_string())
    }

    /// The suffix of the remote key holding `id`'s state file.
    pub(crate) fn state_key_suffix(id: &str) -> String {
        format!("/{}/{}/{}", PREFIX, id, state_file_name(id))
    }

    /// Write a header-only state file for `id` under `base_dir`, anchored at
    /// `execution_dir`, and return its bytes so a test can seed the remote
    /// copy with them (the state `koto init` would have pushed).
    pub(crate) fn seed_session(
        base_dir: &Path,
        id: &str,
        execution_dir: Option<PathBuf>,
    ) -> Vec<u8> {
        let session_dir = base_dir.join(id);
        std::fs::create_dir_all(&session_dir).unwrap();
        let state_path = session_dir.join(state_file_name(id));
        let header = StateFileHeader {
            command_environment: None,
            schema_version: 1,
            workflow: id.to_string(),
            template_hash: "testhash".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            parent_workflow: None,
            template_source_dir: None,
            template_source_file: None,
            origin: None,
            execution_dir,
            session_id: String::new(),
            intent: None,
            template_name: None,
            needs_agent: None,
            role: None,
            inputs: None,
            coordinator_of_record: None,
            requested_by: None,
            assignment_claim: None,
            dispatch_epoch: 0,
            priority: None,
            deadline: None,
            retry_count: None,
            agent_config: None,
            kind: None,
            root_session_id: None,
            parent_session_id: None,
            respawn_generation: None,
        };
        crate::engine::persistence::append_header(&state_path, &header).unwrap();
        std::fs::read(&state_path).unwrap()
    }

    /// The header line of a state file's bytes.
    pub(crate) fn header_of(state: &[u8]) -> StateFileHeader {
        let text = std::str::from_utf8(state).expect("state file is UTF-8");
        let first = text.lines().next().expect("state file has a header line");
        serde_json::from_str(first).expect("header line parses")
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{
        cloud_backend_at, header_of, list_body, listing_prefix, seed_session, serve,
        state_key_suffix, Endpoint,
    };
    use super::*;
    use crate::engine::persistence::append_header;
    use crate::engine::types::StateFileHeader;
    use crate::session::context::ContextStore;
    use crate::session::SessionBackend;
    use std::fs;
    use tempfile::TempDir;

    /// Create a CloudBackend backed by a temp directory and a bucket pointing
    /// at a fake endpoint. S3 operations will fail, exercising the non-fatal
    /// error handling paths.
    fn test_cloud_backend(base_dir: &Path) -> CloudBackend {
        let local = LocalBackend::with_base_dir(base_dir.to_path_buf());
        // Use a dummy bucket pointing at a non-routable endpoint.
        // All S3 calls will fail, which is fine -- we're testing that:
        //   1. Local operations succeed
        //   2. S3 failures are swallowed and logged to stderr
        let region = Region::Custom {
            region: "us-east-1".to_string(),
            endpoint: "http://192.0.2.1:19000".to_string(), // RFC 5737 TEST-NET
        };
        let credentials =
            Credentials::new(Some("test-key"), Some("test-secret"), None, None, None).unwrap();
        let bucket = Bucket::new("test-bucket", region, credentials).unwrap();
        CloudBackend::with_parts(local, bucket, "test-prefix".to_string())
    }

    /// Helper: write a minimal state file header into a session directory.
    fn write_state_file(base_dir: &Path, id: &str, created_at: &str) {
        let session_dir = base_dir.join(id);
        fs::create_dir_all(&session_dir).unwrap();
        let state_path = session_dir.join(state_file_name(id));
        let header = StateFileHeader {
            command_environment: None,
            schema_version: 1,
            workflow: id.to_string(),
            template_hash: "testhash".to_string(),
            created_at: created_at.to_string(),
            parent_workflow: None,
            template_source_dir: None,
            template_source_file: None,
            origin: None,
            execution_dir: None,
            session_id: String::new(),
            intent: None,
            template_name: None,
            needs_agent: None,
            role: None,
            inputs: None,
            coordinator_of_record: None,
            requested_by: None,
            assignment_claim: None,
            dispatch_epoch: 0,
            priority: None,
            deadline: None,
            retry_count: None,
            agent_config: None,
            kind: None,
            root_session_id: None,
            parent_session_id: None,
            respawn_generation: None,
        };
        append_header(&state_path, &header).unwrap();
    }

    // -- SessionBackend: create delegates to local --

    #[test]
    fn create_delegates_to_local() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        let path = backend.create("myworkflow").unwrap();
        assert!(path.is_dir());
        assert_eq!(path, tmp.path().join("myworkflow"));
    }

    #[test]
    fn create_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        let p1 = backend.create("wf").unwrap();
        let p2 = backend.create("wf").unwrap();
        assert_eq!(p1, p2);
    }

    // -- SessionBackend: session_dir --

    #[test]
    fn session_dir_delegates_to_local() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        assert_eq!(backend.session_dir("wf"), tmp.path().join("wf"));
    }

    // -- SessionBackend: exists checks local first --

    #[test]
    fn exists_true_when_local_state_file_present() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        write_state_file(tmp.path(), "present", "2026-01-01T00:00:00Z");
        assert!(backend.exists("present"));
    }

    #[test]
    fn exists_false_when_neither_local_nor_s3() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        // S3 call will fail (unreachable endpoint) and return false.
        assert!(!backend.exists("ghost"));
    }

    // -- SessionBackend: cleanup removes local then attempts S3 delete --

    #[test]
    fn cleanup_removes_local_directory() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        write_state_file(tmp.path(), "doomed", "2026-01-01T00:00:00Z");
        assert!(tmp.path().join("doomed").exists());

        // cleanup succeeds even though S3 delete fails.
        backend.cleanup("doomed").unwrap();
        assert!(!tmp.path().join("doomed").exists());
    }

    #[test]
    fn cleanup_idempotent_on_missing() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        assert!(backend.cleanup("ghost").is_ok());
    }

    // -- SessionBackend: init_state_file delegates to local, sync is non-fatal --

    #[test]
    fn init_state_file_delegates_to_local_and_tolerates_s3_failure() {
        use crate::engine::types::{Event, EventPayload};

        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        let header = StateFileHeader {
            command_environment: None,
            schema_version: 1,
            workflow: "wf".to_string(),
            template_hash: "testhash".to_string(),
            created_at: "2026-04-13T00:00:00Z".to_string(),
            parent_workflow: None,
            template_source_dir: None,
            template_source_file: None,
            origin: None,
            execution_dir: None,
            session_id: String::new(),
            intent: None,
            template_name: None,
            needs_agent: None,
            role: None,
            inputs: None,
            coordinator_of_record: None,
            requested_by: None,
            assignment_claim: None,
            dispatch_epoch: 0,
            priority: None,
            deadline: None,
            retry_count: None,
            agent_config: None,
            kind: None,
            root_session_id: None,
            parent_session_id: None,
            respawn_generation: None,
        };
        let events = vec![Event {
            seq: 1,
            timestamp: "2026-04-13T00:00:00Z".to_string(),
            event_type: "workflow_initialized".to_string(),
            payload: EventPayload::WorkflowInitialized {
                template_path: "/tmp/tpl.md".to_string(),
                variables: Default::default(),
                spawn_entry: None,
            },
            idempotency_hash: None,
        }];

        // S3 push will fail (unreachable endpoint) but the call should
        // still succeed because local write committed.
        backend
            .init_state_file("wf", header.clone(), events)
            .unwrap();
        assert!(backend.exists("wf"));

        let got = backend.read_header("wf").unwrap();
        assert_eq!(got.workflow, "wf");
    }

    #[test]
    fn init_state_file_second_call_returns_collision() {
        use crate::engine::types::{Event, EventPayload};

        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        let header = StateFileHeader {
            command_environment: None,
            schema_version: 1,
            workflow: "wf".to_string(),
            template_hash: "testhash".to_string(),
            created_at: "2026-04-13T00:00:00Z".to_string(),
            parent_workflow: None,
            template_source_dir: None,
            template_source_file: None,
            origin: None,
            execution_dir: None,
            session_id: String::new(),
            intent: None,
            template_name: None,
            needs_agent: None,
            role: None,
            inputs: None,
            coordinator_of_record: None,
            requested_by: None,
            assignment_claim: None,
            dispatch_epoch: 0,
            priority: None,
            deadline: None,
            retry_count: None,
            agent_config: None,
            kind: None,
            root_session_id: None,
            parent_session_id: None,
            respawn_generation: None,
        };
        let events = vec![Event {
            seq: 1,
            timestamp: "2026-04-13T00:00:00Z".to_string(),
            event_type: "workflow_initialized".to_string(),
            payload: EventPayload::WorkflowInitialized {
                template_path: "/tmp/tpl.md".to_string(),
                variables: Default::default(),
                spawn_entry: None,
            },
            idempotency_hash: None,
        }];
        backend
            .init_state_file("wf", header.clone(), events.clone())
            .unwrap();
        let err = backend
            .init_state_file("wf", header, events)
            .expect_err("second init must fail");
        assert!(
            matches!(err, SessionError::Collision),
            "want SessionError::Collision, got: {:?}",
            err
        );
    }

    // -- SessionBackend: lock_state_file delegates to local --

    #[test]
    fn lock_state_file_delegates_to_local() {
        use crate::engine::types::{Event, EventPayload};

        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        let header = StateFileHeader {
            command_environment: None,
            schema_version: 1,
            workflow: "wf".to_string(),
            template_hash: "testhash".to_string(),
            created_at: "2026-04-13T00:00:00Z".to_string(),
            parent_workflow: None,
            template_source_dir: None,
            template_source_file: None,
            origin: None,
            execution_dir: None,
            session_id: String::new(),
            intent: None,
            template_name: None,
            needs_agent: None,
            role: None,
            inputs: None,
            coordinator_of_record: None,
            requested_by: None,
            assignment_claim: None,
            dispatch_epoch: 0,
            priority: None,
            deadline: None,
            retry_count: None,
            agent_config: None,
            kind: None,
            root_session_id: None,
            parent_session_id: None,
            respawn_generation: None,
        };
        let events = vec![Event {
            seq: 1,
            timestamp: "2026-04-13T00:00:00Z".to_string(),
            event_type: "workflow_initialized".to_string(),
            payload: EventPayload::WorkflowInitialized {
                template_path: "/tmp/tpl.md".to_string(),
                variables: Default::default(),
                spawn_entry: None,
            },
            idempotency_hash: None,
        }];
        backend.init_state_file("wf", header, events).unwrap();

        // First acquire succeeds; second observes contention. This
        // exercises the intra-host serialization guarantee that
        // CloudBackend is documented to provide.
        let _guard = backend.lock_state_file("wf").expect("first acquire");
        let err = backend
            .lock_state_file("wf")
            .expect_err("second acquire must contend");
        assert!(
            matches!(err, SessionError::Locked { .. }),
            "want SessionError::Locked, got: {:?}",
            err
        );
    }

    // -- SessionBackend: list returns local sessions --

    #[test]
    fn list_returns_local_sessions() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        write_state_file(tmp.path(), "beta", "2026-02-01T00:00:00Z");
        write_state_file(tmp.path(), "alpha", "2026-01-01T00:00:00Z");

        let sessions = backend.list().unwrap();
        // S3 list will fail silently, so we only get local sessions.
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].id, "alpha");
        assert_eq!(sessions[1].id, "beta");
    }

    // -- SessionBackend: remote-only placeholder rows never carry a
    // template_source_status --
    //
    // `test_cloud_backend` points S3 at an unroutable address, so
    // `s3_list_sessions()` always returns empty in this test suite and the
    // remote-only merge branch in `list()` can't be exercised end-to-end.
    // Test the extracted `placeholder_session_info` helper directly instead.
    #[test]
    fn placeholder_session_info_has_no_template_source_status() {
        let info = placeholder_session_info("remote-only".to_string());
        assert_eq!(info.id, "remote-only");
        assert_eq!(
            info.template_source_status, None,
            "remote-only placeholder rows must always report None, regardless \
             of what a real session with this id might have recorded"
        );
    }

    // -- ContextStore: add writes locally then attempts sync --

    #[test]
    fn context_add_delegates_to_local() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        fs::create_dir_all(tmp.path().join("sess")).unwrap();

        backend.add("sess", "scope.md", b"hello").unwrap();
        let retrieved = backend.get("sess", "scope.md").unwrap();
        assert_eq!(retrieved, b"hello");
    }

    // -- ContextStore: get pulls from remote if newer, then reads locally --

    #[test]
    fn context_get_reads_local_when_s3_unreachable() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        fs::create_dir_all(tmp.path().join("sess")).unwrap();

        backend.add("sess", "scope.md", b"local-data").unwrap();
        // get() tries to pull from remote (fails silently), then reads local.
        let retrieved = backend.get("sess", "scope.md").unwrap();
        assert_eq!(retrieved, b"local-data");
    }

    // -- ContextStore: ctx_exists checks local first, falls back to remote --

    #[test]
    fn context_ctx_exists_true_when_local() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        fs::create_dir_all(tmp.path().join("sess")).unwrap();

        backend.add("sess", "scope.md", b"data").unwrap();
        assert!(backend.ctx_exists("sess", "scope.md"));
    }

    #[test]
    fn context_ctx_exists_false_when_neither_local_nor_remote() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        // S3 remote check will fail (unreachable), returns false.
        assert!(!backend.ctx_exists("sess", "missing.md"));
    }

    // -- ContextStore: remove delegates to local, then syncs delete --

    #[test]
    fn context_remove_delegates_to_local() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        fs::create_dir_all(tmp.path().join("sess")).unwrap();

        backend.add("sess", "scope.md", b"data").unwrap();
        assert!(backend.ctx_exists("sess", "scope.md"));

        backend.remove("sess", "scope.md").unwrap();
        assert!(!backend.ctx_exists("sess", "scope.md"));
    }

    // -- ContextStore: list_keys merges local and remote --

    #[test]
    fn context_list_keys_returns_local_when_s3_unreachable() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        fs::create_dir_all(tmp.path().join("sess")).unwrap();

        backend.add("sess", "alpha.md", b"a").unwrap();
        backend.add("sess", "beta.md", b"b").unwrap();

        // remote_list_keys returns None (S3 unreachable), so only local keys.
        let keys = backend.list_keys("sess", None).unwrap();
        assert_eq!(keys, vec!["alpha.md", "beta.md"]);
    }

    // -- S3 key construction --

    #[test]
    fn state_key_format() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        assert_eq!(
            backend.state_key("wf"),
            "test-prefix/wf/koto-wf.state.jsonl"
        );
    }

    #[test]
    fn session_prefix_format() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        assert_eq!(backend.session_prefix("wf"), "test-prefix/wf/");
    }

    // context_key and manifest_key construction is tested indirectly through
    // sync module functions that build the same S3 key format.

    // -- Non-fatal S3 errors: sync methods don't panic --

    #[test]
    fn sync_push_state_non_fatal_on_missing_file() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        // No state file exists, should silently return.
        backend.sync_push_state("nonexistent");
    }

    #[test]
    fn sync_push_state_non_fatal_on_s3_failure() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        write_state_file(tmp.path(), "wf", "2026-01-01T00:00:00Z");
        // S3 upload will fail (unreachable endpoint), should not panic.
        backend.sync_push_state("wf");
    }

    #[test]
    fn sync_delete_session_non_fatal_on_s3_failure() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        // S3 list will fail, should not panic.
        backend.sync_delete_session("wf");
    }

    #[test]
    fn sync_push_context_non_fatal_on_missing_file() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        // No file, should silently return.
        sync::push_context_key(
            &backend.local,
            &backend.bucket,
            &backend.prefix,
            "sess",
            "missing.md",
            &backend.manifest_cache,
        );
    }

    // -- Sync: pull is non-fatal when S3 is unreachable --

    #[test]
    fn sync_pull_non_fatal_on_s3_failure() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        fs::create_dir_all(tmp.path().join("sess")).unwrap();

        backend.add("sess", "scope.md", b"local").unwrap();
        // pull attempt fails silently, local data is unaffected.
        let pulled = sync::pull_context_if_newer(
            &backend.local,
            &backend.bucket,
            &backend.prefix,
            "sess",
            "scope.md",
            &backend.manifest_cache,
            &|| Ok(()),
        )
        .unwrap();
        assert!(!pulled);
        let data = backend.local.get("sess", "scope.md").unwrap();
        assert_eq!(data, b"local");
    }

    // -- read_events_local reads the local file with no pull --

    #[test]
    fn read_events_local_makes_no_s3_request() {
        use std::net::TcpListener;

        let tmp = TempDir::new().unwrap();
        // An S3 endpoint that accepts connections but never answers: any
        // pull attempt would show up as a pending connection.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let region = Region::Custom {
            region: "us-east-1".to_string(),
            endpoint: format!("http://{}", listener.local_addr().unwrap()),
        };
        let credentials =
            Credentials::new(Some("test-key"), Some("test-secret"), None, None, None).unwrap();
        let bucket = Bucket::new("test-bucket", region, credentials).unwrap();
        let local = LocalBackend::with_base_dir(tmp.path().to_path_buf());
        let backend = CloudBackend::with_parts(local, bucket, "test-prefix".to_string());

        write_state_file(tmp.path(), "wf", "2026-01-01T00:00:00Z");
        backend
            .local
            .append_event(
                "wf",
                &crate::engine::types::EventPayload::Transitioned {
                    from: None,
                    to: "start".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                    context_assignments: None,
                    vars_matched: None,
                },
                "2026-01-01T00:00:01Z",
            )
            .unwrap();

        let started = std::time::Instant::now();
        let (header, events) = backend.read_events_local("wf").unwrap();
        assert_eq!(header.workflow, "wf");
        assert_eq!(events.len(), 1);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        match listener.accept() {
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            other => panic!("read_events_local contacted S3: {:?}", other.map(|_| ())),
        }
    }

    // -- Sync: delete is non-fatal when S3 is unreachable --

    #[test]
    fn sync_delete_context_non_fatal_on_s3_failure() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        // S3 delete will fail, should not panic.
        sync::delete_context_key(
            &backend.local,
            &backend.bucket,
            &backend.prefix,
            "sess",
            "gone.md",
            &backend.manifest_cache,
        );
    }

    // -- Sync: remote_key_exists returns None when S3 is unreachable --

    #[test]
    fn remote_key_exists_returns_none_on_s3_failure() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        let result = sync::remote_key_exists(
            &backend.bucket,
            &backend.prefix,
            "sess",
            "scope.md",
            &backend.manifest_cache,
        );
        assert!(result.is_none());
    }

    // -- Sync: remote_list_keys returns None when S3 is unreachable --

    #[test]
    fn remote_list_keys_returns_none_on_s3_failure() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        let result = sync::remote_list_keys(
            &backend.bucket,
            &backend.prefix,
            "sess",
            None,
            &backend.manifest_cache,
        );
        assert!(result.is_none());
    }

    // ------------------------------------------------------------------
    //  scenario-29: classify_reconciliation covers the three non-conflict
    //  auto paths plus the conflict path (Issue #19).
    // ------------------------------------------------------------------

    #[test]
    fn classify_auto_local_extends_remote_is_accept_local() {
        let remote = b"header\nevt1\n";
        let local = b"header\nevt1\nevt2\n";
        assert_eq!(
            CloudBackend::classify_reconciliation(Some(local), Some(remote), "auto"),
            ChildResolution::AcceptedLocal
        );
    }

    #[test]
    fn classify_auto_remote_extends_local_is_accept_remote() {
        let local = b"header\nevt1\n";
        let remote = b"header\nevt1\nevt2\n";
        assert_eq!(
            CloudBackend::classify_reconciliation(Some(local), Some(remote), "auto"),
            ChildResolution::AcceptedRemote
        );
    }

    #[test]
    fn classify_auto_equal_is_identical() {
        let bytes = b"header\nevt1\n";
        assert_eq!(
            CloudBackend::classify_reconciliation(Some(bytes), Some(bytes), "auto"),
            ChildResolution::Identical
        );
    }

    #[test]
    fn classify_auto_divergent_is_conflict() {
        let local = b"header\nevtA\n";
        let remote = b"header\nevtB\n";
        assert_eq!(
            CloudBackend::classify_reconciliation(Some(local), Some(remote), "auto"),
            ChildResolution::Conflict
        );
    }

    #[test]
    fn classify_skip_never_touches_bytes() {
        let a = b"aaa";
        let b = b"bbb";
        // skip must ignore bytes entirely — divergent, identical, and
        // missing all collapse to Skipped.
        assert_eq!(
            CloudBackend::classify_reconciliation(Some(a), Some(b), "skip"),
            ChildResolution::Skipped
        );
        assert_eq!(
            CloudBackend::classify_reconciliation(None, None, "skip"),
            ChildResolution::Skipped
        );
    }

    #[test]
    fn classify_accept_remote_requires_remote_bytes() {
        assert_eq!(
            CloudBackend::classify_reconciliation(Some(b"l"), Some(b"r"), "accept-remote"),
            ChildResolution::AcceptedRemote
        );
        assert!(matches!(
            CloudBackend::classify_reconciliation(Some(b"l"), None, "accept-remote"),
            ChildResolution::Errored { .. }
        ));
    }

    #[test]
    fn classify_accept_local_requires_local_bytes() {
        assert_eq!(
            CloudBackend::classify_reconciliation(Some(b"l"), Some(b"r"), "accept-local"),
            ChildResolution::AcceptedLocal
        );
        assert!(matches!(
            CloudBackend::classify_reconciliation(None, Some(b"r"), "accept-local"),
            ChildResolution::Errored { .. }
        ));
    }

    #[test]
    fn classify_unknown_policy_errors() {
        assert!(matches!(
            CloudBackend::classify_reconciliation(Some(b"x"), Some(b"y"), "nonsense"),
            ChildResolution::Errored { .. }
        ));
    }

    #[test]
    fn classify_auto_one_side_missing_mirrors_the_other() {
        assert_eq!(
            CloudBackend::classify_reconciliation(Some(b"x"), None, "auto"),
            ChildResolution::AcceptedLocal
        );
        assert_eq!(
            CloudBackend::classify_reconciliation(None, Some(b"x"), "auto"),
            ChildResolution::AcceptedRemote
        );
        assert_eq!(
            CloudBackend::classify_reconciliation(None, None, "auto"),
            ChildResolution::Identical
        );
    }

    // ------------------------------------------------------------------
    //  reconcile_child: skip policy is the only path that doesn't need
    //  a reachable S3 endpoint. Other paths either depend on remote
    //  bytes (which the test backend cannot fetch) or on pushing, which
    //  the test backend cannot complete. For the full matrix see the
    //  classify_* tests above.
    // ------------------------------------------------------------------

    #[test]
    fn reconcile_child_skip_never_touches_files_or_network() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        write_state_file(tmp.path(), "child", "2026-01-01T00:00:00Z");
        let before =
            std::fs::read(tmp.path().join("child").join(state_file_name("child"))).unwrap();

        let outcome = backend.reconcile_child("child", "skip");
        assert_eq!(outcome, ChildResolution::Skipped);

        let after = std::fs::read(tmp.path().join("child").join(state_file_name("child"))).unwrap();
        assert_eq!(before, after, "skip policy must leave local bytes intact");
    }

    #[test]
    fn reconcile_child_unknown_policy_errors() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        write_state_file(tmp.path(), "child", "2026-01-01T00:00:00Z");
        assert!(matches!(
            backend.reconcile_child("child", "garbage"),
            ChildResolution::Errored { .. }
        ));
    }

    // Regression for Issue #19 Blocker 3: a transient remote fetch
    // failure must surface as ChildResolution::Errored under `auto`, not
    // silently classify as AcceptedLocal and overwrite the remote.
    // test_cloud_backend points at an unreachable endpoint so the fetch
    // always Errs, letting us stand in for the transient-failure case.
    #[test]
    fn reconcile_child_auto_errors_on_transient_fetch_failure() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        write_state_file(tmp.path(), "child", "2026-01-01T00:00:00Z");
        let before =
            std::fs::read(tmp.path().join("child").join(state_file_name("child"))).unwrap();

        let outcome = backend.reconcile_child("child", "auto");
        match outcome {
            ChildResolution::Errored { .. } => {}
            other => panic!(
                "transient fetch failure must not produce AcceptedLocal under auto; got {:?}",
                other
            ),
        }

        // Local bytes are untouched; no AcceptedLocal write happened.
        let after = std::fs::read(tmp.path().join("child").join(state_file_name("child"))).unwrap();
        assert_eq!(
            before, after,
            "transient fetch failure must not mutate local state"
        );
    }

    // Blocker 3 companion: the same guarantee under `accept-remote`.
    // A transient fetch error must not appear to "succeed" via a
    // misclassified absent remote.
    #[test]
    fn reconcile_child_accept_remote_errors_on_transient_fetch_failure() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        write_state_file(tmp.path(), "child", "2026-01-01T00:00:00Z");
        assert!(matches!(
            backend.reconcile_child("child", "accept-remote"),
            ChildResolution::Errored { .. }
        ));
    }

    // -- A cloud pull is a recorded write with writer `sync` --

    #[test]
    fn a_pull_records_writer_sync_in_the_store_and_the_log() {
        use crate::cache::sha256_hex;
        use crate::engine::types::EventPayload;
        use crate::session::context::{KeyMeta, Manifest};

        let content = b"pulled from the remote store".to_vec();
        let meta = |c: &[u8], writer: &str| KeyMeta {
            created_at: "2026-01-01T00:00:00Z".to_string(),
            size: c.len() as u64,
            hash: sha256_hex(c),
            writer: Some(writer.to_string()),
        };
        let mut manifest = Manifest::default();
        manifest
            .keys
            .insert("notes.md".to_string(), meta(&content, "agent"));
        manifest
            .keys
            .insert("remote-only.md".to_string(), meta(b"elsewhere", "agent"));
        let endpoint = serve(vec![
            (
                "/ctx/manifest.json".to_string(),
                serde_json::to_vec(&manifest).unwrap(),
            ),
            ("/ctx/notes.md".to_string(), content.clone()),
        ])
        .url;

        let tmp = TempDir::new().unwrap();
        write_state_file(tmp.path(), "wf", "2026-01-01T00:00:00Z");
        let backend = cloud_backend_at(tmp.path(), endpoint);

        // A key only the remote store has reads its metadata from there.
        let remote = backend.meta("wf", "remote-only.md").expect("remote meta");
        assert_eq!(remote.hash, sha256_hex(b"elsewhere"));

        assert_eq!(backend.get("wf", "notes.md").unwrap(), content);

        let local_meta = backend.local.meta("wf", "notes.md").unwrap();
        assert_eq!(local_meta.writer.as_deref(), Some("sync"));

        // The read is made outside any tick of `wf`, so its pull is spooled
        // beside the session, as the read is, and the log is left alone
        // (koto#431).
        let (_, events) = backend.local.read_events("wf").unwrap();
        assert!(
            !events
                .iter()
                .any(|e| matches!(&e.payload, EventPayload::ContextAdded { .. })),
            "the pull is not in the read session's log"
        );
        let spool_lines = || {
            std::fs::read_to_string(
                backend
                    .local
                    .session_dir("wf")
                    .join(crate::session::read_spool::SPOOL_FILE),
            )
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .collect::<Vec<_>>()
        };
        let pulls: Vec<_> = spool_lines()
            .into_iter()
            .filter(|l| l["type"] == "context_added")
            .map(|l| (l["payload"]["hash"].clone(), l["payload"]["writer"].clone()))
            .collect();
        assert_eq!(
            pulls,
            vec![(sha256_hex(&content).into(), "sync".into())],
            "one spooled context_added with writer sync for the pull"
        );

        // A second read finds the hashes equal: no pull, nothing logged or
        // spooled for one.
        backend.get("wf", "notes.md").unwrap();
        let (_, again) = backend.local.read_events("wf").unwrap();
        assert_eq!(again.len(), events.len());
        assert_eq!(
            spool_lines()
                .iter()
                .filter(|l| l["type"] == "context_added")
                .count(),
            1
        );
    }

    // -- A header rewrite reaches the remote copy (koto#310) --

    #[test]
    fn rewrite_header_pushes_the_new_header_and_a_later_read_keeps_it() {
        let tmp = TempDir::new().unwrap();
        let seeded = seed_session(tmp.path(), "wf", None);
        let endpoint = serve(vec![(state_key_suffix("wf"), seeded)]);
        let backend = cloud_backend_at(tmp.path(), endpoint.url.clone());
        let anchor = tmp.path().join("anchor");

        backend
            .rewrite_header("wf", &|mut h| {
                h.execution_dir = Some(anchor.clone());
                h
            })
            .unwrap();

        let pushed = endpoint
            .last_put(&state_key_suffix("wf"))
            .expect("the rewrite pushed the state file");
        assert_eq!(header_of(&pushed).execution_dir, Some(anchor.clone()));

        // The next read finds the bucket holding what the rewrite pushed
        // (koto#428): it lists the session's prefix and fetches nothing,
        // and the rewrite survives.
        assert_eq!(
            backend.read_header("wf").unwrap().execution_dir,
            Some(anchor)
        );
        let last = endpoint.requests().pop().unwrap();
        assert_eq!(
            (
                last.method.as_str(),
                last.path.ends_with(&state_key_suffix("wf"))
            ),
            ("GET", false),
            "the read made no GET of the state file it pushed"
        );
    }

    // -- session.cloud.path_style --

    #[test]
    fn create_bucket_addresses_the_bucket_path_style_when_asked() {
        let mut config = CloudConfig {
            endpoint: Some("http://127.0.0.1:9000".to_string()),
            bucket: Some("sessions".to_string()),
            region: Some("us-east-1".to_string()),
            access_key: Some("k".to_string()),
            secret_key: Some("s".to_string()),
            path_style: Some(true),
            marker_retain_days: None,
            check_window_secs: None,
        };
        let bucket = create_bucket(&config).unwrap();
        assert!(bucket.is_path_style());
        assert_eq!(bucket.url(), "http://127.0.0.1:9000/sessions");

        config.path_style = None;
        assert!(!create_bucket(&config).unwrap().is_path_style());
        config.path_style = Some(false);
        assert!(!create_bucket(&config).unwrap().is_path_style());
    }

    // -- the migration marker on state reads --

    fn marker_bytes(target: &str, workspace: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "schema": 1,
            "target": {
                "session": target,
                "session_id": "id-b",
                "workspace": workspace,
                "prefix": "0123456789abcdef",
            },
            "machine_id": "m",
            "migrated_at": "2026-01-01T00:00:00Z",
        }))
        .unwrap()
    }

    #[test]
    fn a_marker_refuses_reads_and_leaves_the_local_file_alone() {
        let endpoint = serve(vec![(
            "/wf/migrated.json".to_string(),
            marker_bytes("wf", "/srv/ws-b"),
        )])
        .url;
        let tmp = TempDir::new().unwrap();
        write_state_file(tmp.path(), "wf", "2026-01-01T00:00:00Z");
        let state = tmp.path().join("wf").join(state_file_name("wf"));
        let before = fs::read(&state).unwrap();
        let backend = cloud_backend_at(tmp.path(), endpoint);

        let err = backend.read_events("wf").unwrap_err();
        let migrated = err
            .downcast_ref::<SessionMigrated>()
            .expect("a typed SessionMigrated");
        assert_eq!(migrated.target, "wf");
        assert_eq!(migrated.workspace, "/srv/ws-b");
        assert!(err.to_string().starts_with("session_migrated:"), "{err}");

        let err = backend.read_header("wf").unwrap_err();
        assert!(err.downcast_ref::<SessionMigrated>().is_some(), "{err}");

        assert_eq!(fs::read(&state).unwrap(), before);
    }

    #[test]
    fn a_missing_marker_lets_reads_proceed() {
        // The stand-in answers every GET it has no object for with 404.
        let endpoint = serve(vec![]).url;
        let tmp = TempDir::new().unwrap();
        write_state_file(tmp.path(), "wf", "2026-01-01T00:00:00Z");
        let backend = cloud_backend_at(tmp.path(), endpoint);
        assert_eq!(backend.read_header("wf").unwrap().workflow, "wf");
        backend.read_events("wf").unwrap();
    }

    #[test]
    fn an_unreachable_bucket_warns_and_lets_reads_proceed() {
        let tmp = TempDir::new().unwrap();
        write_state_file(tmp.path(), "wf", "2026-01-01T00:00:00Z");
        let backend = test_cloud_backend(tmp.path());
        backend.check_not_migrated("wf").unwrap();
        assert_eq!(backend.read_header("wf").unwrap().workflow, "wf");
    }

    #[test]
    fn a_marker_that_does_not_parse_still_refuses() {
        let endpoint = serve(vec![(
            "/wf/migrated.json".to_string(),
            b"not json".to_vec(),
        )])
        .url;
        let tmp = TempDir::new().unwrap();
        write_state_file(tmp.path(), "wf", "2026-01-01T00:00:00Z");
        let backend = cloud_backend_at(tmp.path(), endpoint);
        let err = backend.check_not_migrated("wf").unwrap_err();
        let migrated = err.downcast_ref::<SessionMigrated>().unwrap();
        assert_eq!(migrated.target, "unknown");
        assert_eq!(migrated.workspace, "unknown");
    }

    #[test]
    fn the_marker_check_is_answered_once_per_session_per_process() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        // Count connections: every request is one, since the stand-in
        // closes each after answering. A listing names the marker; any
        // other request gets the marker's body.
        let hits = Arc::new(AtomicUsize::new(0));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let counter = Arc::clone(&hits);
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader, Write};
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                counter.fetch_add(1, Ordering::SeqCst);
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                let _ = reader.read_line(&mut request_line);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                }
                let target = request_line.split_whitespace().nth(1).unwrap_or("");
                // A listing of the session's prefix (the probe) or of the
                // marker's own key shows the marker.
                let body = match listing_prefix(target) {
                    Some(prefix) if prefix.ends_with('/') => {
                        list_body(&[format!("{prefix}{MIGRATED_MARKER}")])
                    }
                    Some(prefix) => list_body(&[prefix]),
                    None => marker_bytes("wf", "/srv/ws-b"),
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            }
        });
        let tmp = TempDir::new().unwrap();
        let backend = cloud_backend_at(tmp.path(), format!("http://{addr}"));

        for _ in 0..3 {
            let err = backend.check_not_migrated("wf").unwrap_err();
            assert_eq!(
                err.downcast_ref::<SessionMigrated>().unwrap().workspace,
                "/srv/ws-b"
            );
        }
        // The listing and the marker's body, once.
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_missing_marker_costs_one_listing_and_no_retry() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        // Answers every request with an empty listing, and counts them.
        let hits = Arc::new(AtomicUsize::new(0));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let counter = Arc::clone(&hits);
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader, Write};
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                counter.fetch_add(1, Ordering::SeqCst);
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                }
                let body = list_body(&[]);
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            }
        });
        let tmp = TempDir::new().unwrap();
        let backend = cloud_backend_at(tmp.path(), format!("http://{addr}"));

        let started = std::time::Instant::now();
        backend.check_not_migrated("wf").unwrap();
        // rust-s3 sleeps a full second before retrying a failed request.
        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "the check took {:?}",
            started.elapsed()
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn only_the_sessions_own_marker_counts_as_one() {
        assert!(is_migration_marker("p/wf/", "p/wf/migrated.json"));
        assert!(!is_migration_marker("p/wf/", "p/wf/koto-wf.state.jsonl"));
        // A context key named migrated.json is a context key: cleanup
        // deletes it like any other.
        assert!(!is_migration_marker("p/wf/", "p/wf/ctx/migrated.json"));
        assert!(!is_migration_marker("p/wf/", "p/other/migrated.json"));
    }

    #[test]
    fn a_marker_target_prefix_must_be_sixteen_lowercase_hex() {
        assert!(is_workspace_prefix("0123456789abcdef"));
        for bad in [
            "",
            "0123456789ABCDEF",
            "0123456789abcde",
            "0123456789abcdef0",
            "../../0123456789",
            "0123456789abcde/",
        ] {
            assert!(!is_workspace_prefix(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_target_session_id_is_read_from_the_header_line() {
        assert_eq!(
            header_session_id(b"\n{\"workflow\":\"foo\",\"session_id\":\"id-1\"}\n{\"seq\":1}\n"),
            Some("id-1".to_string())
        );
        assert_eq!(header_session_id(b"{\"workflow\":\"foo\"}\n"), None);
        assert_eq!(header_session_id(b"not json\n"), None);
        assert_eq!(header_session_id(b""), None);
    }

    // -- the import's source checks --

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn source_log(header: serde_json::Value, events: &[serde_json::Value]) -> Vec<u8> {
        let mut out = serde_json::to_string(&header).unwrap();
        out.push('\n');
        for e in events {
            out.push_str(&serde_json::to_string(e).unwrap());
            out.push('\n');
        }
        out.into_bytes()
    }

    fn header_json(hash: &str) -> serde_json::Value {
        serde_json::json!({
            "schema_version": 1,
            "workflow": "wf",
            "template_hash": hash,
            "created_at": "2026-01-01T00:00:00Z",
            "session_id": "id-a",
        })
    }

    fn event_json(seq: u64) -> serde_json::Value {
        serde_json::json!({
            "seq": seq,
            "timestamp": "2026-01-01T00:00:00Z",
            "type": "intent_updated",
            "payload": {"intent": "x"},
        })
    }

    #[test]
    fn parse_source_log_keeps_event_lines_verbatim() {
        // Keys in an order serde wouldn't produce, to prove the line is
        // carried rather than re-serialized.
        let line = r#"{"payload":{"intent":"x"},"type":"intent_updated","timestamp":"2026-01-01T00:00:00Z","seq":1}"#;
        let mut bytes = serde_json::to_vec(&header_json(HASH)).unwrap();
        bytes.extend_from_slice(b"\n");
        bytes.extend_from_slice(line.as_bytes());
        bytes.extend_from_slice(b"\n\n");
        let log = parse_source_log(&bytes).unwrap();
        assert_eq!(log.event_lines, vec![line.to_string()]);
        assert_eq!(log.last_seq, 1);
        assert_eq!(log.header.session_id, "id-a");
    }

    #[test]
    fn parse_source_log_refuses_what_it_cannot_carry() {
        let mut v2 = header_json(HASH);
        v2["schema_version"] = serde_json::json!(2);
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("empty", Vec::new()),
            ("schema 2", source_log(v2, &[])),
            (
                "seq gap",
                source_log(header_json(HASH), &[event_json(1), event_json(3)]),
            ),
            ("not utf-8", vec![0xff, 0xfe]),
        ];
        for (what, bytes) in cases {
            assert!(parse_source_log(&bytes).is_err(), "{what} must be refused");
        }
        let mut garbage = source_log(header_json(HASH), &[event_json(1)]);
        garbage.extend_from_slice(b"{not an event\n");
        assert!(parse_source_log(&garbage).is_err());
    }

    /// A target directory that appears while the import runs, even an
    /// empty one, is never renamed over: the move refuses
    /// `import_name_taken`, takes back exactly what was pushed, and the
    /// staging directory goes.
    #[test]
    fn a_target_that_appeared_meanwhile_is_not_renamed_over() {
        let endpoint = serve(vec![]);
        let tmp = TempDir::new().unwrap();
        let backend = cloud_backend_at(tmp.path(), endpoint.url.clone());
        let staged = tmp.path().join(".import-wf-test");
        create_private_dir(&staged).unwrap();
        fs::write(staged.join(state_file_name("wf")), b"{}\n").unwrap();
        // Created by someone else between the push and the rename.
        fs::create_dir(tmp.path().join("wf")).unwrap();

        let pushed = vec![
            "test-prefix/wf/koto-wf.state.jsonl".to_string(),
            "test-prefix/wf/version.json".to_string(),
        ];
        let mut staging = Staging {
            dir: staged.clone(),
            moved: false,
        };
        let err = backend
            .move_into_place("wf", &mut staging, &pushed)
            .unwrap_err();
        assert_eq!(err.code, ImportErrorCode::NameTaken, "{}", err);
        assert!(err.message.contains("--as"), "{}", err);
        drop(staging);

        assert!(!staged.exists(), "the staging directory was left");
        assert_eq!(
            fs::read_dir(tmp.path().join("wf")).unwrap().count(),
            0,
            "the other directory was written into"
        );
        let seen: Vec<String> = endpoint
            .requests()
            .iter()
            .map(|r| format!("{} {}", r.method, r.path))
            .collect();
        assert_eq!(
            seen,
            vec![
                "DELETE /test-bucket/test-prefix/wf/koto-wf.state.jsonl".to_string(),
                "DELETE /test-bucket/test-prefix/wf/version.json".to_string(),
            ]
        );
    }

    /// The staging directory is created mode 0700, inside the session
    /// store, and creating it where anything already is fails.
    #[cfg(unix)]
    #[test]
    fn the_staging_directory_is_private_and_exclusive() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".import-wf-x");
        create_private_dir(&dir).unwrap();
        let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "mode {:o}", mode);

        let err = create_private_dir(&dir).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        // Not even an empty directory is reused, nor a file.
        let file = tmp.path().join(".import-wf-y");
        fs::write(&file, b"").unwrap();
        assert_eq!(
            create_private_dir(&file).unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
    }

    #[test]
    fn userinfo_is_taken_out_of_every_url() {
        assert_eq!(
            without_url_userinfo(
                "GET http://user:pass@127.0.0.1:9000/b/k failed; see https://key@host/x"
            ),
            "GET http://127.0.0.1:9000/b/k failed; see https://host/x"
        );
        assert_eq!(
            without_url_userinfo("endpoint \"http://a:b@host\" refused"),
            "endpoint \"http://host\" refused"
        );
        // An @ past the authority is the path's, not userinfo.
        assert_eq!(
            without_url_userinfo("http://host/p@q and no url here"),
            "http://host/p@q and no url here"
        );
        let err = ImportError::new(
            ImportErrorCode::SourceUnreadable,
            "listing at http://AKIA:secret@10.0.0.1:9000/b failed",
        );
        assert_eq!(err.message, "listing at http://10.0.0.1:9000/b failed");
    }

    #[test]
    fn import_codes_print_and_exit_as_documented() {
        use ImportErrorCode::*;
        for (code, text, exit) in [
            (RequiresCloud, "import_requires_cloud", 2),
            (SourceNotFound, "import_source_not_found", 2),
            (SourceMigrated, "import_source_migrated", 2),
            (SourceIsChild, "import_source_is_child", 2),
            (NameTaken, "import_name_taken", 2),
            (TemplateUnavailable, "import_template_unavailable", 2),
            (SourceUnreadable, "import_source_unreadable", 1),
            (PushFailed, "import_push_failed", 1),
            (Unmarked, "import_unmarked", 1),
        ] {
            assert_eq!(code.as_str(), text);
            assert_eq!(code.exit_code(), exit, "{text}");
        }
    }

    // -- The local guards a same-host import relies on --------------------

    /// A log for session `id` with header session id `sid`, whose last
    /// event is a `session_imported` from `from_workspace`'s `from_session`
    /// (session id `from_sid`); `None` writes a plain log.
    fn guard_log(id: &str, sid: &str, imported: Option<(&str, &str, &str)>) -> Vec<u8> {
        let header = serde_json::json!({
            "schema_version": 1,
            "workflow": id,
            "template_hash": "a".repeat(64),
            "created_at": "2026-01-01T00:00:00Z",
            "session_id": sid,
            "origin": {"anchor": "/srv/ws-b", "store": {"kind": "cloud", "base": "/x"}},
        });
        let mut lines = vec![header.to_string(), event_json(1).to_string()];
        if let Some((ws, from, from_sid)) = imported {
            lines.push(
                serde_json::json!({
                    "seq": 2,
                    "timestamp": "2026-01-01T00:00:00Z",
                    "type": "session_imported",
                    "payload": {
                        "from_workspace": ws,
                        "from_session": from,
                        "from_session_id": from_sid,
                        "machine_id": "m",
                    },
                })
                .to_string(),
            );
        }
        let mut out = lines.join("\n");
        out.push('\n');
        out.into_bytes()
    }

    fn write_log(base: &Path, id: &str, bytes: &[u8]) -> PathBuf {
        let dir = base.join(id);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(state_file_name(id));
        fs::write(&path, bytes).unwrap();
        path
    }

    /// A backend whose prefix is the one koto derives for `ws`, against an
    /// unreachable endpoint: the local guards must answer before any
    /// request is tried.
    fn backend_for_workspace(base: &Path, ws: &Path) -> CloudBackend {
        let prefix = crate::session::local::prefix_for_workspace(ws).unwrap();
        let region = Region::Custom {
            region: "us-east-1".to_string(),
            endpoint: "http://192.0.2.1:19000".to_string(),
        };
        let credentials =
            Credentials::new(Some("test-key"), Some("test-secret"), None, None, None).unwrap();
        let bucket = Bucket::new("test-bucket", region, credentials).unwrap();
        CloudBackend::with_parts(
            LocalBackend::with_base_dir(base.to_path_buf()),
            bucket,
            prefix,
        )
    }

    #[test]
    fn the_source_workspace_refuses_the_session_imported_from_it() {
        let tmp = TempDir::new().unwrap();
        let ws_a = tmp.path().join("ws-a");
        fs::create_dir_all(&ws_a).unwrap();
        let ws_a = fs::canonicalize(&ws_a).unwrap();
        let base = tmp.path().join("sessions");
        let log = guard_log("wf", "id-b", Some((ws_a.to_str().unwrap(), "wf", "id-a")));
        let state = write_log(&base, "wf", &log);
        let a = backend_for_workspace(&base, &ws_a);

        for err in [
            a.read_events("wf").unwrap_err(),
            a.read_header("wf").map(|_| ()).unwrap_err(),
            a.check_not_migrated("wf").unwrap_err(),
            SessionBackend::cleanup(&a, "wf").unwrap_err(),
            a.resolve_conflict("wf", "local").unwrap_err(),
            a.append_event(
                "wf",
                &crate::engine::types::EventPayload::IntentUpdated {
                    intent: "x".to_string(),
                },
                "2026-01-01T00:00:00Z",
            )
            .unwrap_err(),
            a.add("wf", "k", b"v").unwrap_err(),
        ] {
            let migrated = err
                .downcast_ref::<SessionMigrated>()
                .unwrap_or_else(|| panic!("a typed SessionMigrated, got {err}"));
            assert_eq!(migrated.workspace, "/srv/ws-b");
        }
        // Nothing touched the imported session's files.
        assert_eq!(fs::read(&state).unwrap(), log);
        assert!(!base.join("wf").join("ctx").exists());
    }

    #[test]
    fn another_workspace_passes_the_local_guard() {
        let tmp = TempDir::new().unwrap();
        let ws_a = tmp.path().join("ws-a");
        let ws_b = tmp.path().join("ws-b");
        fs::create_dir_all(&ws_a).unwrap();
        fs::create_dir_all(&ws_b).unwrap();
        let ws_a = fs::canonicalize(&ws_a).unwrap();
        let ws_b = fs::canonicalize(&ws_b).unwrap();
        let base = tmp.path().join("sessions");
        // The importing workspace itself, and a log imported under another
        // name, and a plain log: none of them is refused locally.
        write_log(
            &base,
            "wf",
            &guard_log("wf", "id-b", Some((ws_a.to_str().unwrap(), "wf", "id-a"))),
        );
        backend_for_workspace(&base, &ws_b)
            .local_guard("wf")
            .unwrap();
        write_log(
            &base,
            "renamed",
            &guard_log(
                "renamed",
                "id-c",
                Some((ws_a.to_str().unwrap(), "wf", "id-a")),
            ),
        );
        backend_for_workspace(&base, &ws_a)
            .local_guard("renamed")
            .unwrap();
        write_log(&base, "plain", &guard_log("plain", "id-p", None));
        backend_for_workspace(&base, &ws_a)
            .local_guard("plain")
            .unwrap();
    }

    #[test]
    fn a_pending_takeover_refuses_reads_and_writes_of_the_name() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        let ws = fs::canonicalize(&ws).unwrap();
        let base = tmp.path().join("sessions");
        let state = write_log(&base, "wf", &guard_log("wf", "id-a", None));
        fs::create_dir_all(takeover_dir(&base, "wf")).unwrap();
        let backend = backend_for_workspace(&base, &ws);
        for err in [
            backend.read_header("wf").map(|_| ()).unwrap_err(),
            backend.add("wf", "k", b"v").unwrap_err(),
            backend.remove("wf", "k").unwrap_err(),
        ] {
            assert!(err.to_string().contains("being imported"), "{err}");
            assert!(err.to_string().contains("koto session import wf"), "{err}");
        }
        assert_eq!(fs::read(&state).unwrap(), guard_log("wf", "id-a", None));
        assert!(!base.join("wf").join("ctx").exists());
        // Another name is untouched by it.
        write_log(&base, "other", &guard_log("other", "id-o", None));
        backend.local_guard("other").unwrap();
    }

    #[test]
    fn a_pull_never_writes_one_side_of_an_import_over_the_other() {
        // The bucket holds the source (id-a); the local copy is the
        // session imported from it (id-b), from a workspace other than
        // this backend's, so only the pull's own check stands in the way.
        let remote = guard_log("wf", "id-a", None);
        let local = guard_log("wf", "id-b", Some(("/srv/elsewhere", "wf", "id-a")));
        for direction in ["local-is-import", "remote-is-import"] {
            let (remote_bytes, local_bytes) = if direction == "local-is-import" {
                (remote.clone(), local.clone())
            } else {
                (local.clone(), remote.clone())
            };
            let endpoint = serve(vec![
                ("/wf/koto-wf.state.jsonl".to_string(), remote_bytes),
                ("/wf/version.json".to_string(), b"{}".to_vec()),
            ])
            .url;
            let tmp = TempDir::new().unwrap();
            let state = write_log(tmp.path(), "wf", &local_bytes);
            let backend = cloud_backend_at(tmp.path(), endpoint);
            for err in [
                backend.read_events("wf").map(|_| ()).unwrap_err(),
                backend.read_header("wf").map(|_| ()).unwrap_err(),
                backend.force_pull_session("wf").unwrap_err(),
            ] {
                assert!(
                    err.to_string().contains("refusing to pull"),
                    "{direction}: {err}"
                );
            }
            assert_eq!(fs::read(&state).unwrap(), local_bytes, "{direction}");
        }
    }

    #[test]
    fn a_context_pull_never_writes_one_side_of_an_import_over_the_other() {
        // The bucket's state is the source (id-a) and holds a key the local
        // copy, the session imported from it (id-b), lacks.
        let remote = guard_log("wf", "id-a", None);
        let local = guard_log("wf", "id-b", Some(("/srv/elsewhere", "wf", "id-a")));
        let content = b"bucket bytes".to_vec();
        let hash = {
            use sha2::Digest;
            hex::encode(sha2::Sha256::digest(&content))
        };
        let manifest = serde_json::json!({
            "keys": {"k": {"created_at": "2026-01-01T00:00:00Z", "size": content.len(), "hash": hash}}
        });
        let endpoint = serve(vec![
            ("/wf/koto-wf.state.jsonl".to_string(), remote),
            (
                "/wf/ctx/manifest.json".to_string(),
                manifest.to_string().into_bytes(),
            ),
            ("/wf/ctx/k".to_string(), content),
        ])
        .url;
        let tmp = TempDir::new().unwrap();
        let state = write_log(tmp.path(), "wf", &local);
        let backend = cloud_backend_at(tmp.path(), endpoint);
        let err = ContextStore::get(&backend, "wf", "k").unwrap_err();
        assert!(err.to_string().contains("refusing to pull"), "{err}");
        assert_eq!(fs::read(&state).unwrap(), local);
        assert!(!tmp.path().join("wf").join("ctx").join("k").exists());
    }

    #[test]
    fn a_context_pull_of_the_same_session_still_downloads() {
        let log = guard_log("wf", "id-a", None);
        let content = b"bucket bytes".to_vec();
        let hash = {
            use sha2::Digest;
            hex::encode(sha2::Sha256::digest(&content))
        };
        let manifest = serde_json::json!({
            "keys": {"k": {"created_at": "2026-01-01T00:00:00Z", "size": content.len(), "hash": hash}}
        });
        let endpoint = serve(vec![
            ("/wf/koto-wf.state.jsonl".to_string(), log.clone()),
            (
                "/wf/ctx/manifest.json".to_string(),
                manifest.to_string().into_bytes(),
            ),
            ("/wf/ctx/k".to_string(), content.clone()),
        ])
        .url;
        let tmp = TempDir::new().unwrap();
        write_log(tmp.path(), "wf", &log);
        let backend = cloud_backend_at(tmp.path(), endpoint);
        assert_eq!(ContextStore::get(&backend, "wf", "k").unwrap(), content);
    }

    #[test]
    fn a_pull_of_the_same_session_still_overwrites() {
        let remote = guard_log("wf", "id-a", Some(("/srv/x", "old", "id-0")));
        let endpoint = serve(vec![(
            "/wf/koto-wf.state.jsonl".to_string(),
            remote.clone(),
        )])
        .url;
        let tmp = TempDir::new().unwrap();
        let state = write_log(tmp.path(), "wf", &guard_log("wf", "id-a", None));
        let backend = cloud_backend_at(tmp.path(), endpoint);
        backend.read_header("wf").unwrap();
        assert_eq!(fs::read(&state).unwrap(), remote);
    }

    /// `ensure_pushed` pushes an open batch whole, in the flush's order, and
    /// closes it, whichever process asks: a state file pushed on its own
    /// would name keys the bucket doesn't hold (koto#376).
    #[test]
    fn ensure_pushed_flushes_an_open_batch_in_order() {
        let endpoint = serve(Vec::new());
        let tmp = TempDir::new().unwrap();
        write_state_file(tmp.path(), "wf", "2026-01-01T00:00:00Z");
        // A version past its base, so the flush needs no machine id.
        fs::write(
            tmp.path().join("wf").join("version.json"),
            r#"{"version":1,"last_sync_base":0,"machine_id":"testhost"}"#,
        )
        .unwrap();
        let backend = cloud_backend_at(tmp.path(), endpoint.url.clone());
        let journal = backend.journal("wf");
        {
            let lock = journal.lock().unwrap().unwrap();
            journal.open(&lock).unwrap();
            journal
                .record(
                    &lock,
                    &batch::Op::Add {
                        key: "notes.md".to_string(),
                    },
                )
                .unwrap();
            backend.local.add("wf", "notes.md", b"deferred").unwrap();
            journal
                .record(
                    &lock,
                    &batch::Op::Remove {
                        key: "gone.md".to_string(),
                    },
                )
                .unwrap();
        }

        backend.ensure_pushed("wf").unwrap();

        assert!(!journal.is_open());
        let writes: Vec<String> = endpoint
            .requests()
            .into_iter()
            .filter(|r| r.method == "PUT" || r.method == "DELETE")
            .map(|r| {
                let rest = r.path.rsplit_once("/wf/").map(|(_, rest)| rest);
                format!("{} {}", r.method, rest.unwrap_or(&r.path))
            })
            .collect();
        assert_eq!(
            writes,
            vec![
                "PUT ctx/notes.md".to_string(),
                "PUT ctx/manifest.json".to_string(),
                format!("PUT {}", state_file_name("wf")),
                "PUT version.json".to_string(),
                "DELETE ctx/gone.md".to_string(),
                format!("DELETE {}", BATCH_MARKER),
            ]
        );
        assert_eq!(endpoint.last_put("/ctx/notes.md").unwrap(), b"deferred");
    }

    /// The bucket path of each request that wrote to `wf`'s copy.
    fn writes_to_wf(endpoint: &Endpoint) -> Vec<String> {
        endpoint
            .requests()
            .into_iter()
            .filter(|r| r.method == "PUT" || r.method == "DELETE")
            .map(|r| {
                let rest = r.path.rsplit_once("/wf/").map(|(_, rest)| rest);
                format!("{} {}", r.method, rest.unwrap_or(&r.path))
            })
            .collect()
    }

    /// A deferred write reaches the disk before it returns (koto#381):
    /// opening the batch syncs the opening file and its directory, then the
    /// directory again once the journal replaces the opening file, and
    /// recording the write syncs the journal.
    #[test]
    fn a_deferred_write_is_synced_before_it_returns() {
        let endpoint = serve(Vec::new());
        let tmp = TempDir::new().unwrap();
        write_state_file(tmp.path(), "wf", "2026-01-01T00:00:00Z");
        fs::write(
            tmp.path().join("wf").join("version.json"),
            r#"{"version":1,"last_sync_base":1,"machine_id":"testhost"}"#,
        )
        .unwrap();
        let backend = cloud_backend_at(tmp.path(), endpoint.url.clone());
        let dir = tmp.path().join("wf");

        batch::synced::take();
        let (journal, lock) = backend.open_batch("wf").unwrap().unwrap();
        assert_eq!(
            batch::synced::take(),
            vec![
                dir.join(batch::OPENING_FILE),
                dir.clone(),
                dir.join(batch::JOURNAL_FILE),
                dir.clone()
            ]
        );
        assert_eq!(
            writes_to_wf(&endpoint),
            vec![format!("PUT {}", BATCH_MARKER)]
        );
        assert!(journal.is_open());
        assert!(!dir.join(batch::OPENING_FILE).exists());

        journal
            .record(
                &lock,
                &batch::Op::Add {
                    key: "notes.md".to_string(),
                },
            )
            .unwrap();
        assert_eq!(batch::synced::take(), vec![dir.join(batch::JOURNAL_FILE)]);
    }

    /// A batch a killed process began and never opened (the opening file,
    /// no journal) is cleared by the next flush (koto#381): the marker it
    /// put in the bucket is deleted when it is the one the opening file
    /// holds, then the opening file goes, and nothing else is written. A
    /// marker with other bytes was written since by another writer, and is
    /// left to it.
    #[test]
    fn a_batch_begun_and_never_opened_is_cleared() {
        let ours = br#"{"session":"wf","opened_at":"2026-01-01T00:00:00Z","pid":1}"#;
        let theirs = br#"{"session":"wf","opened_at":"2026-01-01T00:00:01Z","pid":2}"#;
        for (in_bucket, deleted) in [(ours, true), (theirs, false)] {
            let endpoint = serve(vec![(format!("/wf/{}", BATCH_MARKER), in_bucket.to_vec())]);
            let tmp = TempDir::new().unwrap();
            write_state_file(tmp.path(), "wf", "2026-01-01T00:00:00Z");
            let backend = cloud_backend_at(tmp.path(), endpoint.url.clone());
            let journal = backend.journal("wf");
            {
                let lock = journal.lock().unwrap().unwrap();
                journal.begin(&lock, ours).unwrap();
            }

            backend.flush_pending("wf").unwrap();

            assert!(!journal.is_opening());
            assert!(!journal.is_open());
            let want: Vec<String> = if deleted {
                vec![format!("DELETE {}", BATCH_MARKER)]
            } else {
                Vec::new()
            };
            assert_eq!(writes_to_wf(&endpoint), want);

            // With nothing begun, a flush asks nothing of the bucket.
            let before = endpoint.requests().len();
            backend.flush_pending("wf").unwrap();
            assert_eq!(endpoint.requests().len(), before);
        }
    }
}
