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

use std::path::{Path, PathBuf};

use anyhow::Context;
use s3::creds::Credentials;
use s3::{Bucket, Region};

use crate::config::CloudConfig;
use crate::session::context::ContextStore;
use crate::session::local::{repo_id, LocalBackend};
use crate::session::sync::{self, ManifestCache};
use crate::session::{state_file_name, SessionBackend, SessionError, SessionInfo, SessionLock};

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
}

impl CloudBackend {
    /// Construct a `CloudBackend` from a working directory and cloud config.
    ///
    /// The working directory is used to derive the repo-id (same as
    /// `LocalBackend`). The cloud config provides S3 endpoint, bucket
    /// name, region, and credentials.
    pub fn new(working_dir: &Path, cloud_config: &CloudConfig) -> anyhow::Result<Self> {
        let local = LocalBackend::new(working_dir)?;
        let prefix = repo_id(working_dir)?;
        let bucket = create_bucket(cloud_config)?;
        Ok(Self {
            local,
            bucket,
            prefix,
            manifest_cache: ManifestCache::new(),
        })
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
        }
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
    fn sync_push_state(&self, id: &str) {
        if let Err(e) = self.strict_push_state(id) {
            eprintln!("warning: cloud sync failed for state upload: {}", e);
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
        let state_path = self.local.session_dir(id).join(state_file_name(id));
        if !state_path.exists() {
            // Nothing to push; mirrors the old no-op behavior. Callers
            // relying on ordering ensure the append happened before the
            // probe, so this path is reachable only in degenerate tests.
            return Ok(());
        }
        let data = std::fs::read(&state_path)
            .with_context(|| format!("reading local state file: {}", state_path.display()))?;
        let key = self.state_key(id);
        self.put_object(&key, &data)
    }

    /// Download the state file from S3 to the local session directory.
    /// Non-fatal on failure: if S3 is unreachable, local state is used as-is.
    fn sync_pull_state(&self, id: &str) {
        let key = self.state_key(id);
        match self.bucket.get_object(&key) {
            Ok(response) if response.status_code() == 200 => {
                let state_path = self.local.session_dir(id).join(state_file_name(id));
                if let Err(e) = std::fs::write(&state_path, response.bytes()) {
                    eprintln!("warning: cloud sync: failed to write pulled state: {}", e);
                }
            }
            Ok(_) => {} // Not found or other status, use local
            Err(e) => {
                eprintln!("warning: cloud sync pull failed: {}", e);
            }
        }
    }

    /// Delete all objects under a session's S3 prefix. Non-fatal on failure.
    fn sync_delete_session(&self, id: &str) {
        let prefix = self.session_prefix(id);
        // List and delete objects under the prefix.
        match self.bucket.list(prefix.clone(), None) {
            Ok(results) => {
                for list in &results {
                    for obj in &list.contents {
                        if let Err(e) = self.bucket.delete_object(&obj.key) {
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

    /// List session IDs present in S3 under this backend's prefix.
    fn s3_list_sessions(&self) -> Vec<String> {
        let prefix = format!("{}/", self.prefix);
        match self.bucket.list(prefix.clone(), Some("/".to_string())) {
            Ok(results) => {
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
                ids
            }
            Err(e) => {
                eprintln!("warning: cloud sync: failed to list sessions: {}", e);
                Vec::new()
            }
        }
    }

    /// Check if a session exists in S3 by looking for its state file.
    fn s3_session_exists(&self, id: &str) -> bool {
        let key = self.state_key(id);
        self.bucket.head_object(&key).is_ok()
    }

    /// Wrapper around `bucket.put_object` that returns a Result.
    fn put_object(&self, key: &str, data: &[u8]) -> anyhow::Result<()> {
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
        let key = self.version_key(id);
        let response = self.bucket.get_object(&key).ok()?;
        if response.status_code() != 200 {
            return None;
        }
        serde_json::from_slice(response.bytes()).ok()
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

    /// Resolve a version conflict by keeping local or remote state.
    pub fn resolve_conflict(&self, id: &str, keep: &str) -> anyhow::Result<()> {
        use crate::session::version::{get_or_create_machine_id, resolved_version, SessionVersion};

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

        Ok(())
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
        let key = self.state_key(id);
        self.put_object(&key, &data)
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

    /// Force-upload the entire local session directory to S3.
    fn force_push_session(&self, id: &str) -> anyhow::Result<()> {
        let session_dir = self.local.session_dir(id);
        if !session_dir.exists() {
            anyhow::bail!(
                "session directory does not exist: {}",
                session_dir.display()
            );
        }

        // Upload state file.
        self.sync_push_state(id);

        // Upload version.json.
        self.push_version(id)?;

        // Upload all context files.
        let ctx_dir = session_dir.join("ctx");
        if ctx_dir.exists() {
            for entry in std::fs::read_dir(&ctx_dir)? {
                let entry = entry?;
                let file_name = entry.file_name().to_string_lossy().to_string();
                let data = std::fs::read(entry.path())?;
                let s3_key = format!("{}/{}/ctx/{}", self.prefix, id, file_name);
                if let Err(e) = self.put_object(&s3_key, &data) {
                    eprintln!(
                        "warning: cloud sync: failed to upload ctx/{}: {}",
                        file_name, e
                    );
                }
            }
        }

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
                std::fs::write(&state_path, response.bytes())?;
            }
        }

        // Download all context files by listing the ctx/ prefix.
        let ctx_prefix = format!("{}/{}/ctx/", self.prefix, id);
        if let Ok(results) = self.bucket.list(ctx_prefix.clone(), None) {
            let ctx_dir = session_dir.join("ctx");
            std::fs::create_dir_all(&ctx_dir)?;
            for list in &results {
                for obj in &list.contents {
                    if let Some(file_name) = obj.key.strip_prefix(&ctx_prefix) {
                        if file_name.is_empty() {
                            continue;
                        }
                        if let Ok(response) = self.bucket.get_object(&obj.key) {
                            if response.status_code() == 200 {
                                let local_path = ctx_dir.join(file_name);
                                if let Some(parent) = local_path.parent() {
                                    std::fs::create_dir_all(parent)?;
                                }
                                std::fs::write(&local_path, response.bytes())?;
                            }
                        }
                    }
                }
            }
        }

        Ok(())
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

    fn exists(&self, id: &str) -> bool {
        if self.local.exists(id) {
            return true;
        }
        // Fall back to S3 check.
        self.s3_session_exists(id)
    }

    fn cleanup(&self, id: &str) -> anyhow::Result<()> {
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
                local_sessions.push(SessionInfo {
                    id: remote_id,
                    created_at: String::new(),
                    template_hash: String::new(),
                    parent_workflow: None,
                });
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
        self.local.append_header(id, header)?;
        self.sync_push_state(id);
        Ok(())
    }

    fn append_event(
        &self,
        id: &str,
        payload: &crate::engine::types::EventPayload,
        timestamp: &str,
    ) -> anyhow::Result<()> {
        self.local.append_event(id, payload, timestamp)?;
        self.sync_push_state(id);
        Ok(())
    }

    fn read_events(
        &self,
        id: &str,
    ) -> anyhow::Result<(
        crate::engine::types::StateFileHeader,
        Vec<crate::engine::types::Event>,
    )> {
        self.sync_pull_state(id);
        self.local.read_events(id)
    }

    fn read_header(&self, id: &str) -> anyhow::Result<crate::engine::types::StateFileHeader> {
        self.sync_pull_state(id);
        self.local.read_header(id)
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
        self.local.init_state_file(id, header, initial_events)?;
        self.sync_push_state(id);
        Ok(())
    }

    fn relocate(&self, from: &str, to: &str) -> anyhow::Result<()> {
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
                                if let Err(e) = self.bucket.delete_object(&obj.key) {
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

impl ContextStore for CloudBackend {
    fn add(&self, session: &str, key: &str, content: &[u8]) -> anyhow::Result<()> {
        self.local.add(session, key, content)?;

        // Check version before pushing. Conflicts are hard errors;
        // S3 connectivity failures are non-fatal (version check is skipped).
        if let Err(e) = self.check_and_increment_version(session) {
            let msg = e.to_string();
            if msg.starts_with("session conflict:") {
                return Err(e);
            }
            // S3 unreachable or version file missing -- proceed without version check.
            eprintln!("warning: cloud sync: version check failed: {}", e);
        }

        sync::push_context_key(
            &self.local,
            &self.bucket,
            &self.prefix,
            session,
            key,
            &self.manifest_cache,
        );

        // Update last_sync_base after successful push.
        self.finalize_version_after_push(session);

        Ok(())
    }

    fn get(&self, session: &str, key: &str) -> anyhow::Result<Vec<u8>> {
        // Pull from remote if a newer version exists.
        sync::pull_context_if_newer(
            &self.local,
            &self.bucket,
            &self.prefix,
            session,
            key,
            &self.manifest_cache,
        );
        self.local.get(session, key)
    }

    fn ctx_exists(&self, session: &str, key: &str) -> bool {
        if self.local.ctx_exists(session, key) {
            return true;
        }
        // Fall back to checking remote manifest.
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
        self.local.remove(session, key)?;

        // Check version before pushing deletion.
        if let Err(e) = self.check_and_increment_version(session) {
            let msg = e.to_string();
            if msg.starts_with("session conflict:") {
                return Err(e);
            }
            eprintln!("warning: cloud sync: version check failed: {}", e);
        }

        sync::delete_context_key(
            &self.local,
            &self.bucket,
            &self.prefix,
            session,
            key,
            &self.manifest_cache,
        );

        self.finalize_version_after_push(session);

        Ok(())
    }

    fn list_keys(&self, session: &str, prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
        let mut keys = self.local.list_keys(session, prefix)?;
        // Merge in remote-only keys.
        if let Some(remote_keys) = sync::remote_list_keys(
            &self.bucket,
            &self.prefix,
            session,
            prefix,
            &self.manifest_cache,
        ) {
            for k in remote_keys {
                if !keys.contains(&k) {
                    keys.push(k);
                }
            }
        }
        keys.sort();
        Ok(keys)
    }
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
    Ok(bucket)
}

#[cfg(test)]
mod tests {
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
            schema_version: 1,
            workflow: id.to_string(),
            template_hash: "testhash".to_string(),
            created_at: created_at.to_string(),
            parent_workflow: None,
            template_source_dir: None,
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
            schema_version: 1,
            workflow: "wf".to_string(),
            template_hash: "testhash".to_string(),
            created_at: "2026-04-13T00:00:00Z".to_string(),
            parent_workflow: None,
            template_source_dir: None,
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
            schema_version: 1,
            workflow: "wf".to_string(),
            template_hash: "testhash".to_string(),
            created_at: "2026-04-13T00:00:00Z".to_string(),
            parent_workflow: None,
            template_source_dir: None,
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
            schema_version: 1,
            workflow: "wf".to_string(),
            template_hash: "testhash".to_string(),
            created_at: "2026-04-13T00:00:00Z".to_string(),
            parent_workflow: None,
            template_source_dir: None,
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
        sync::pull_context_if_newer(
            &backend.local,
            &backend.bucket,
            &backend.prefix,
            "sess",
            "scope.md",
            &backend.manifest_cache,
        );
        let data = backend.local.get("sess", "scope.md").unwrap();
        assert_eq!(data, b"local");
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
}
