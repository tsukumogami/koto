//! Cloud-backed session storage using S3-compatible object storage.
//!
//! `CloudBackend` wraps `LocalBackend` so all filesystem operations happen
//! locally first (fast, works offline). After each mutating operation it
//! syncs the affected files to S3. S3 failures are non-fatal: the local
//! operation succeeds and a warning is printed to stderr.

use std::path::{Path, PathBuf};

use anyhow::Context;
use s3::creds::Credentials;
use s3::{Bucket, Region};

use crate::config::CloudConfig;
use crate::session::context::ContextStore;
use crate::session::local::{repo_id, LocalBackend};
use crate::session::{state_file_name, SessionBackend, SessionInfo};

/// S3-backed session storage that delegates to `LocalBackend` for all
/// filesystem operations and syncs state to an S3-compatible bucket.
pub struct CloudBackend {
    local: LocalBackend,
    bucket: Box<Bucket>,
    prefix: String,
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
        })
    }

    /// Construct a `CloudBackend` with an explicit `LocalBackend` and bucket.
    ///
    /// Intended for tests that need to control both the local storage
    /// location and the S3 bucket.
    #[cfg(test)]
    pub fn with_parts(local: LocalBackend, bucket: Box<Bucket>, prefix: String) -> Self {
        Self {
            local,
            bucket,
            prefix,
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

    /// S3 key for a context artifact.
    fn context_key(&self, session: &str, key: &str) -> String {
        format!("{}/{}/ctx/{}", self.prefix, session, key)
    }

    /// S3 key for a session's context manifest.
    fn manifest_key(&self, session: &str) -> String {
        format!("{}/{}/ctx/manifest.json", self.prefix, session)
    }

    /// Upload the state file to S3. Non-fatal on failure.
    fn sync_push_state(&self, id: &str) {
        let state_path = self.local.session_dir(id).join(state_file_name(id));
        if !state_path.exists() {
            return;
        }
        let data = match std::fs::read(&state_path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("warning: cloud sync: failed to read state file: {}", e);
                return;
            }
        };
        let key = self.state_key(id);
        if let Err(e) = self.put_object(&key, &data) {
            eprintln!("warning: cloud sync failed for state upload: {}", e);
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

    /// Upload a context key to S3. Non-fatal on failure.
    fn sync_push_context(&self, session: &str, key: &str) {
        let content_path = self.local.session_dir(session).join("ctx").join(key);
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
        let s3_key = self.context_key(session, key);
        if let Err(e) = self.put_object(&s3_key, &data) {
            eprintln!(
                "warning: cloud sync failed for context upload '{}': {}",
                key, e
            );
        }
    }

    /// Upload the context manifest to S3. Non-fatal on failure.
    fn sync_push_manifest(&self, session: &str) {
        let manifest_path = self
            .local
            .session_dir(session)
            .join("ctx")
            .join("manifest.json");
        if !manifest_path.exists() {
            return;
        }
        let data = match std::fs::read(&manifest_path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("warning: cloud sync: failed to read manifest: {}", e);
                return;
            }
        };
        let key = self.manifest_key(session);
        if let Err(e) = self.put_object(&key, &data) {
            eprintln!("warning: cloud sync failed for manifest upload: {}", e);
        }
    }

    /// Delete a context key from S3. Non-fatal on failure.
    fn sync_delete_context(&self, session: &str, key: &str) {
        let s3_key = self.context_key(session, key);
        if let Err(e) = self.bucket.delete_object(&s3_key) {
            eprintln!(
                "warning: cloud sync: failed to delete context key '{}': {}",
                key, e
            );
        }
    }

    /// Wrapper around `bucket.put_object` that returns a Result.
    fn put_object(&self, key: &str, data: &[u8]) -> anyhow::Result<()> {
        self.bucket
            .put_object(key, data)
            .with_context(|| format!("S3 PUT failed for key: {}", key))?;
        Ok(())
    }
}

impl SessionBackend for CloudBackend {
    fn create(&self, id: &str) -> anyhow::Result<PathBuf> {
        let path = self.local.create(id)?;
        self.sync_push_state(id);
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
                });
            }
        }

        local_sessions.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(local_sessions)
    }
}

impl ContextStore for CloudBackend {
    fn add(&self, session: &str, key: &str, content: &[u8]) -> anyhow::Result<()> {
        self.local.add(session, key, content)?;
        self.sync_push_context(session, key);
        self.sync_push_manifest(session);
        Ok(())
    }

    fn get(&self, session: &str, key: &str) -> anyhow::Result<Vec<u8>> {
        self.local.get(session, key)
    }

    fn ctx_exists(&self, session: &str, key: &str) -> bool {
        self.local.ctx_exists(session, key)
    }

    fn remove(&self, session: &str, key: &str) -> anyhow::Result<()> {
        self.local.remove(session, key)?;
        self.sync_delete_context(session, key);
        self.sync_push_manifest(session);
        Ok(())
    }

    fn list_keys(&self, session: &str, prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
        self.local.list_keys(session, prefix)
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

    // -- ContextStore: add delegates to local, S3 sync is non-fatal --

    #[test]
    fn context_add_delegates_to_local() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        fs::create_dir_all(tmp.path().join("sess")).unwrap();

        backend.add("sess", "scope.md", b"hello").unwrap();
        let retrieved = backend.get("sess", "scope.md").unwrap();
        assert_eq!(retrieved, b"hello");
    }

    // -- ContextStore: remove delegates to local --

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

    // -- ContextStore: list_keys delegates to local --

    #[test]
    fn context_list_keys_delegates_to_local() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        fs::create_dir_all(tmp.path().join("sess")).unwrap();

        backend.add("sess", "alpha.md", b"a").unwrap();
        backend.add("sess", "beta.md", b"b").unwrap();

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

    #[test]
    fn context_key_format() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        assert_eq!(
            backend.context_key("wf", "scope.md"),
            "test-prefix/wf/ctx/scope.md"
        );
    }

    #[test]
    fn manifest_key_format() {
        let tmp = TempDir::new().unwrap();
        let backend = test_cloud_backend(tmp.path());
        assert_eq!(
            backend.manifest_key("wf"),
            "test-prefix/wf/ctx/manifest.json"
        );
    }

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
        backend.sync_push_context("sess", "missing.md");
    }
}
