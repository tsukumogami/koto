use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::cache::sha256_hex;
use crate::engine::persistence;
use crate::engine::types::{now_iso8601, Event, EventPayload, StateFileHeader};
use crate::session::context::{ContextStore, KeyMeta, Manifest};
use crate::session::validate::{validate_context_key, validate_session_id};
use crate::session::{state_file_name, SessionBackend, SessionInfo};

/// Filesystem-backed session storage.
///
/// Stores sessions at `<base_dir>/<id>/` where `base_dir` is typically
/// `~/.koto/sessions/<repo-id>/`.
pub struct LocalBackend {
    base_dir: PathBuf,
}

impl LocalBackend {
    /// Create a backend rooted at `~/.koto/sessions/<repo-id>/`.
    ///
    /// The `working_dir` is canonicalized and hashed to produce a stable
    /// repo-id that scopes sessions per project.
    pub fn new(working_dir: &Path) -> anyhow::Result<Self> {
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("could not determine home directory"))?;
        let id = repo_id(working_dir)?;
        Ok(Self {
            base_dir: home.join(".koto").join("sessions").join(id),
        })
    }

    /// Create a backend with an explicit base directory.
    ///
    /// Intended for tests that need to control the storage location.
    pub fn with_base_dir(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }
}

impl SessionBackend for LocalBackend {
    fn create(&self, id: &str) -> anyhow::Result<PathBuf> {
        validate_session_id(id)?;
        let dir = self.base_dir.join(id);

        // Ensure the koto root (~/.koto/) exists with restricted permissions.
        // Walk up from base_dir to find the .koto directory and set 0700 on it.
        ensure_koto_root(&self.base_dir)?;

        fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    fn session_dir(&self, id: &str) -> PathBuf {
        self.base_dir.join(id)
    }

    fn exists(&self, id: &str) -> bool {
        self.base_dir.join(id).join(state_file_name(id)).exists()
    }

    fn cleanup(&self, id: &str) -> anyhow::Result<()> {
        validate_session_id(id)?;
        let dir = self.base_dir.join(id);
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }

    fn list(&self) -> anyhow::Result<Vec<SessionInfo>> {
        let mut results = Vec::new();

        let entries = match fs::read_dir(&self.base_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(results),
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "failed to read directory {}: {}",
                    self.base_dir.display(),
                    e
                ))
            }
        };

        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }

            let dir_name = match entry.file_name().to_str() {
                Some(n) => n.to_string(),
                None => continue,
            };

            let state_path = entry.path().join(state_file_name(&dir_name));
            if !state_path.exists() {
                continue;
            }

            match persistence::read_header(&state_path) {
                Ok(header) => {
                    results.push(SessionInfo {
                        id: dir_name,
                        created_at: header.created_at,
                        template_hash: header.template_hash,
                        parent_workflow: header.parent_workflow,
                    });
                }
                Err(e) => {
                    eprintln!("warning: skipping session {}: {}", state_path.display(), e);
                }
            }
        }

        results.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(results)
    }

    fn append_header(&self, id: &str, header: &StateFileHeader) -> anyhow::Result<()> {
        let path = self.base_dir.join(id).join(state_file_name(id));
        persistence::append_header(&path, header)
    }

    fn append_event(
        &self,
        id: &str,
        payload: &EventPayload,
        timestamp: &str,
    ) -> anyhow::Result<()> {
        let path = self.base_dir.join(id).join(state_file_name(id));
        persistence::append_event(&path, payload, timestamp)?;
        Ok(())
    }

    fn read_events(&self, id: &str) -> anyhow::Result<(StateFileHeader, Vec<Event>)> {
        let path = self.base_dir.join(id).join(state_file_name(id));
        persistence::read_events(&path)
    }

    fn read_header(&self, id: &str) -> anyhow::Result<StateFileHeader> {
        let path = self.base_dir.join(id).join(state_file_name(id));
        persistence::read_header(&path)
    }
}

impl LocalBackend {
    /// Return the `ctx/` directory path for a session.
    fn ctx_dir(&self, session: &str) -> PathBuf {
        self.base_dir.join(session).join("ctx")
    }

    /// Return the manifest path for a session's context store.
    fn manifest_path(&self, session: &str) -> PathBuf {
        self.ctx_dir(session).join("manifest.json")
    }

    /// Return the manifest lock file path.
    fn manifest_lock_path(&self, session: &str) -> PathBuf {
        self.ctx_dir(session).join("manifest.lock")
    }

    /// Return the content file path for a given key.
    fn content_path(&self, session: &str, key: &str) -> PathBuf {
        self.ctx_dir(session).join(key)
    }

    /// Read the manifest, returning a default empty manifest if the file
    /// doesn't exist.
    pub(crate) fn read_manifest(&self, session: &str) -> anyhow::Result<Manifest> {
        let path = self.manifest_path(session);
        match fs::read(&path) {
            Ok(data) => serde_json::from_slice(&data)
                .with_context(|| format!("failed to parse manifest: {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Manifest::default()),
            Err(e) => Err(anyhow::anyhow!(
                "failed to read manifest {}: {}",
                path.display(),
                e
            )),
        }
    }

    /// Write the manifest atomically using temp-file-then-rename.
    fn write_manifest(&self, session: &str, manifest: &Manifest) -> anyhow::Result<()> {
        let path = self.manifest_path(session);
        let json =
            serde_json::to_string_pretty(manifest).context("failed to serialize manifest")?;

        let ctx_dir = self.ctx_dir(session);
        let mut tmp = tempfile::Builder::new()
            .suffix(".tmp")
            .tempfile_in(&ctx_dir)
            .with_context(|| format!("failed to create temp file in: {}", ctx_dir.display()))?;
        tmp.write_all(json.as_bytes())
            .context("failed to write manifest temp file")?;
        tmp.persist(&path).map_err(|e| {
            anyhow::anyhow!(
                "failed to rename manifest temp file to {}: {}",
                path.display(),
                e.error
            )
        })?;
        Ok(())
    }

    /// Acquire an exclusive advisory flock on the given path (blocking).
    ///
    /// Creates the lock file if it doesn't exist. Returns the open File
    /// handle; the lock is released when the file is dropped.
    #[cfg(unix)]
    fn acquire_flock(path: &Path) -> anyhow::Result<fs::File> {
        use std::os::unix::io::AsRawFd;

        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("failed to open lock file: {}", path.display()))?;

        let fd = file.as_raw_fd();
        let ret = unsafe { libc::flock(fd, libc::LOCK_EX) };
        if ret != 0 {
            return Err(anyhow::anyhow!(
                "failed to acquire flock on {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
        Ok(file)
    }

    /// Release an advisory flock. Called explicitly for clarity, though
    /// dropping the file handle also releases the lock.
    #[cfg(unix)]
    fn release_flock(file: &fs::File) {
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        unsafe { libc::flock(fd, libc::LOCK_UN) };
    }
}

impl ContextStore for LocalBackend {
    fn add(&self, session: &str, key: &str, content: &[u8]) -> anyhow::Result<()> {
        validate_context_key(key)?;

        let ctx_dir = self.ctx_dir(session);
        fs::create_dir_all(&ctx_dir)
            .with_context(|| format!("failed to create ctx directory: {}", ctx_dir.display()))?;

        // Step 1: Write the content file (create parent dirs for hierarchical keys).
        let content_path = self.content_path(session, key);
        if let Some(parent) = content_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory: {}", parent.display()))?;
        }

        // Per-key advisory flock prevents concurrent writes to the same key.
        let key_lock_path = content_path.with_extension(
            content_path
                .extension()
                .map(|e| format!("{}.lock", e.to_string_lossy()))
                .unwrap_or_else(|| "lock".to_string()),
        );
        #[cfg(unix)]
        let _key_lock = Self::acquire_flock(&key_lock_path)?;

        fs::write(&content_path, content)
            .with_context(|| format!("failed to write content file: {}", content_path.display()))?;

        // Step 2: Lock manifest, read-modify-write, unlock.
        let manifest_lock_path = self.manifest_lock_path(session);
        #[cfg(unix)]
        let manifest_lock = Self::acquire_flock(&manifest_lock_path)?;

        let mut manifest = self.read_manifest(session)?;
        manifest.keys.insert(
            key.to_string(),
            KeyMeta {
                created_at: now_iso8601(),
                size: content.len() as u64,
                hash: sha256_hex(content),
            },
        );
        self.write_manifest(session, &manifest)?;

        #[cfg(unix)]
        Self::release_flock(&manifest_lock);

        Ok(())
    }

    fn get(&self, session: &str, key: &str) -> anyhow::Result<Vec<u8>> {
        validate_context_key(key)?;

        let content_path = self.content_path(session, key);
        fs::read(&content_path).with_context(|| {
            format!(
                "failed to read context key '{}' for session '{}': {}",
                key,
                session,
                content_path.display()
            )
        })
    }

    fn ctx_exists(&self, session: &str, key: &str) -> bool {
        if validate_context_key(key).is_err() {
            return false;
        }
        self.content_path(session, key).exists()
    }

    fn remove(&self, session: &str, key: &str) -> anyhow::Result<()> {
        validate_context_key(key)?;

        let content_path = self.content_path(session, key);

        // Remove the content file if it exists.
        if content_path.exists() {
            fs::remove_file(&content_path).with_context(|| {
                format!("failed to remove content file: {}", content_path.display())
            })?;
        }

        // Remove the per-key lock file if it exists.
        let key_lock_path = content_path.with_extension(
            content_path
                .extension()
                .map(|e| format!("{}.lock", e.to_string_lossy()))
                .unwrap_or_else(|| "lock".to_string()),
        );
        let _ = fs::remove_file(&key_lock_path);

        // Update the manifest under lock.
        let manifest_lock_path = self.manifest_lock_path(session);
        let ctx_dir = self.ctx_dir(session);
        if ctx_dir.exists() {
            #[cfg(unix)]
            let manifest_lock = Self::acquire_flock(&manifest_lock_path)?;

            let mut manifest = self.read_manifest(session)?;
            manifest.keys.remove(key);
            self.write_manifest(session, &manifest)?;

            #[cfg(unix)]
            Self::release_flock(&manifest_lock);
        }

        Ok(())
    }

    fn list_keys(&self, session: &str, prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
        let manifest = self.read_manifest(session)?;
        let mut keys: Vec<String> = manifest
            .keys
            .keys()
            .filter(|k| match prefix {
                Some(p) => k.starts_with(p),
                None => true,
            })
            .cloned()
            .collect();
        keys.sort();
        Ok(keys)
    }
}

/// Derive a repo-id from a working directory path.
///
/// Canonicalizes the path, hashes with SHA-256, and returns the first
/// 16 hex characters. Two paths that resolve to the same canonical
/// location produce the same repo-id.
pub(crate) fn repo_id(working_dir: &Path) -> anyhow::Result<String> {
    let canonical = fs::canonicalize(working_dir)
        .map_err(|e| anyhow::anyhow!("failed to canonicalize {}: {}", working_dir.display(), e))?;
    let hash = sha256_hex(canonical.to_string_lossy().as_bytes());
    Ok(hash[..16].to_string())
}

/// Ensure the `.koto` ancestor directory exists with mode 0700.
///
/// Walks up from `base_dir` to find a component named `.koto` and sets
/// restrictive permissions on it. Creates all intermediate directories
/// if needed.
fn ensure_koto_root(base_dir: &Path) -> anyhow::Result<()> {
    // Find the .koto directory in the path ancestry.
    let mut koto_dir = None;
    let mut current = base_dir.to_path_buf();
    loop {
        if current.file_name().map(|n| n == ".koto").unwrap_or(false) {
            koto_dir = Some(current);
            break;
        }
        if !current.pop() {
            break;
        }
    }

    if let Some(koto_path) = koto_dir {
        let needs_create = !koto_path.exists();
        fs::create_dir_all(&koto_path)?;

        if needs_create {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&koto_path, fs::Permissions::from_mode(0o700))?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Helper: create a LocalBackend using a temp directory as base.
    fn test_backend(dir: &Path) -> LocalBackend {
        LocalBackend::with_base_dir(dir.to_path_buf())
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
        };
        persistence::append_header(&state_path, &header).unwrap();
    }

    // -- scenario 1: create session directory --

    #[test]
    fn create_makes_directory() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        let path = backend.create("myworkflow").unwrap();
        assert!(path.is_dir());
        assert_eq!(path, tmp.path().join("myworkflow"));
    }

    #[test]
    fn create_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        let p1 = backend.create("wf").unwrap();
        let p2 = backend.create("wf").unwrap();
        assert_eq!(p1, p2);
        assert!(p1.is_dir());
    }

    // -- scenario 2: validation rejects invalid IDs --

    #[test]
    fn create_rejects_invalid_id() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        assert!(backend.create("1bad").is_err());
        assert!(backend.create(".hidden").is_err());
        assert!(backend.create("..").is_err());
        assert!(backend.create("a/b").is_err());
        assert!(backend.create("").is_err());
    }

    // -- scenario 3: validation accepts valid IDs --

    #[test]
    fn create_accepts_valid_ids() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        assert!(backend.create("alpha").is_ok());
        assert!(backend.create("my-workflow").is_ok());
        assert!(backend.create("v2.1").is_ok());
        assert!(backend.create("test_run").is_ok());
        assert!(backend.create("A").is_ok());
    }

    // -- scenario 4: repo_id stability and format --

    #[test]
    fn repo_id_produces_16_hex_chars() {
        let tmp = TempDir::new().unwrap();
        let id = repo_id(tmp.path()).unwrap();
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn repo_id_is_stable() {
        let tmp = TempDir::new().unwrap();
        let id1 = repo_id(tmp.path()).unwrap();
        let id2 = repo_id(tmp.path()).unwrap();
        assert_eq!(id1, id2);
    }

    #[test]
    fn repo_id_differs_for_different_dirs() {
        let tmp1 = TempDir::new().unwrap();
        let tmp2 = TempDir::new().unwrap();
        let id1 = repo_id(tmp1.path()).unwrap();
        let id2 = repo_id(tmp2.path()).unwrap();
        assert_ne!(id1, id2);
    }

    #[test]
    fn repo_id_fails_for_nonexistent_dir() {
        let result = repo_id(Path::new("/nonexistent/path/that/does/not/exist"));
        assert!(result.is_err());
    }

    // -- scenario 5: exists checks for state file --

    #[test]
    fn exists_false_when_no_directory() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        assert!(!backend.exists("nosuch"));
    }

    #[test]
    fn exists_false_when_directory_but_no_state_file() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        backend.create("empty").unwrap();
        assert!(!backend.exists("empty"));
    }

    #[test]
    fn exists_true_when_state_file_present() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        write_state_file(tmp.path(), "present", "2026-01-01T00:00:00Z");
        assert!(backend.exists("present"));
    }

    // -- scenario 6: cleanup removes directory --

    #[test]
    fn cleanup_removes_session_directory() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        write_state_file(tmp.path(), "doomed", "2026-01-01T00:00:00Z");
        assert!(tmp.path().join("doomed").exists());

        backend.cleanup("doomed").unwrap();
        assert!(!tmp.path().join("doomed").exists());
    }

    // -- scenario 7: cleanup idempotent on missing --

    #[test]
    fn cleanup_idempotent_on_missing() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        // Should not error even though "ghost" was never created.
        assert!(backend.cleanup("ghost").is_ok());
    }

    // -- scenario 8: list returns sessions with metadata --

    #[test]
    fn list_returns_sessions_with_metadata() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());

        write_state_file(tmp.path(), "beta", "2026-02-01T00:00:00Z");
        write_state_file(tmp.path(), "alpha", "2026-01-01T00:00:00Z");

        let sessions = backend.list().unwrap();
        assert_eq!(sessions.len(), 2);
        // Sorted by id
        assert_eq!(sessions[0].id, "alpha");
        assert_eq!(sessions[0].created_at, "2026-01-01T00:00:00Z");
        assert_eq!(sessions[1].id, "beta");
        assert_eq!(sessions[1].created_at, "2026-02-01T00:00:00Z");
    }

    // -- scenario 9: list skips directories without state files --

    #[test]
    fn list_skips_directories_without_state_files() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());

        // Create a directory without a state file
        fs::create_dir_all(tmp.path().join("orphan")).unwrap();

        // Create a proper session
        write_state_file(tmp.path(), "valid", "2026-01-01T00:00:00Z");

        let sessions = backend.list().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "valid");
    }

    #[test]
    fn list_returns_empty_when_base_dir_missing() {
        let backend = LocalBackend::with_base_dir(PathBuf::from("/tmp/nonexistent-koto-test-dir"));
        let sessions = backend.list().unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn list_skips_files_in_base_dir() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());

        // Write a regular file (not a directory) in the base dir
        fs::write(tmp.path().join("stray-file.txt"), "hello").unwrap();

        write_state_file(tmp.path(), "real", "2026-01-01T00:00:00Z");

        let sessions = backend.list().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "real");
    }

    // -- session_dir --

    #[test]
    fn session_dir_returns_correct_path() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        let dir = backend.session_dir("myworkflow");
        assert_eq!(dir, tmp.path().join("myworkflow"));
    }

    // -- state_file_name --

    #[test]
    fn state_file_name_format() {
        assert_eq!(state_file_name("hello"), "koto-hello.state.jsonl");
        assert_eq!(
            state_file_name("my-workflow"),
            "koto-my-workflow.state.jsonl"
        );
    }

    // ===== ContextStore tests =====

    // -- scenario-1: add/get round-trip --

    #[test]
    fn ctx_add_get_round_trip() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        fs::create_dir_all(tmp.path().join("sess")).unwrap();

        let content = b"hello context";
        backend.add("sess", "scope.md", content).unwrap();

        let retrieved = backend.get("sess", "scope.md").unwrap();
        assert_eq!(retrieved, content);
    }

    #[test]
    fn ctx_add_overwrites_existing() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        fs::create_dir_all(tmp.path().join("sess")).unwrap();

        backend.add("sess", "scope.md", b"v1").unwrap();
        backend.add("sess", "scope.md", b"v2").unwrap();

        let retrieved = backend.get("sess", "scope.md").unwrap();
        assert_eq!(retrieved, b"v2");
    }

    // -- scenario-8: hierarchical key creates directory structure --

    #[test]
    fn ctx_hierarchical_key_creates_dirs() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        fs::create_dir_all(tmp.path().join("sess")).unwrap();

        let content = b"deep content";
        backend
            .add("sess", "research/r1/lead-cli-ux.md", content)
            .unwrap();

        // Verify directory structure was created.
        assert!(tmp
            .path()
            .join("sess/ctx/research/r1/lead-cli-ux.md")
            .exists());
        assert!(tmp.path().join("sess/ctx/research/r1").is_dir());

        let retrieved = backend.get("sess", "research/r1/lead-cli-ux.md").unwrap();
        assert_eq!(retrieved, content);
    }

    // -- ctx_exists --

    #[test]
    fn ctx_exists_true_when_present() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        fs::create_dir_all(tmp.path().join("sess")).unwrap();

        backend.add("sess", "scope.md", b"data").unwrap();
        assert!(backend.ctx_exists("sess", "scope.md"));
    }

    #[test]
    fn ctx_exists_false_when_missing() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        assert!(!backend.ctx_exists("sess", "missing.md"));
    }

    #[test]
    fn ctx_exists_false_for_invalid_key() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        assert!(!backend.ctx_exists("sess", "../escape"));
    }

    // -- remove --

    #[test]
    fn ctx_remove_deletes_content_and_manifest_entry() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        fs::create_dir_all(tmp.path().join("sess")).unwrap();

        backend.add("sess", "scope.md", b"data").unwrap();
        assert!(backend.ctx_exists("sess", "scope.md"));

        backend.remove("sess", "scope.md").unwrap();
        assert!(!backend.ctx_exists("sess", "scope.md"));

        // Manifest should no longer list the key.
        let keys = backend.list_keys("sess", None).unwrap();
        assert!(keys.is_empty());
    }

    #[test]
    fn ctx_remove_idempotent_on_missing() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        // No ctx dir at all -- should not error.
        assert!(backend.remove("sess", "scope.md").is_ok());
    }

    // -- list_keys --

    #[test]
    fn ctx_list_keys_returns_all_sorted() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        fs::create_dir_all(tmp.path().join("sess")).unwrap();

        backend.add("sess", "beta.md", b"b").unwrap();
        backend.add("sess", "alpha.md", b"a").unwrap();
        backend.add("sess", "research/r1/lead.md", b"r").unwrap();

        let keys = backend.list_keys("sess", None).unwrap();
        assert_eq!(keys, vec!["alpha.md", "beta.md", "research/r1/lead.md"]);
    }

    #[test]
    fn ctx_list_keys_with_prefix() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        fs::create_dir_all(tmp.path().join("sess")).unwrap();

        backend.add("sess", "scope.md", b"s").unwrap();
        backend.add("sess", "research/r1/a.md", b"a").unwrap();
        backend.add("sess", "research/r1/b.md", b"b").unwrap();

        let keys = backend.list_keys("sess", Some("research/")).unwrap();
        assert_eq!(keys, vec!["research/r1/a.md", "research/r1/b.md"]);
    }

    #[test]
    fn ctx_list_keys_empty_when_no_manifest() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        let keys = backend.list_keys("sess", None).unwrap();
        assert!(keys.is_empty());
    }

    // -- manifest integrity --

    #[test]
    fn ctx_manifest_tracks_metadata() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        fs::create_dir_all(tmp.path().join("sess")).unwrap();

        let content = b"hello world";
        backend.add("sess", "scope.md", content).unwrap();

        let manifest = backend.read_manifest("sess").unwrap();
        let meta = manifest.keys.get("scope.md").expect("key should exist");
        assert_eq!(meta.size, content.len() as u64);
        assert_eq!(meta.hash, sha256_hex(content));
        assert!(!meta.created_at.is_empty());
    }

    // -- scenario-9: key validation rejects path traversal --

    #[test]
    fn ctx_add_rejects_path_traversal() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        fs::create_dir_all(tmp.path().join("sess")).unwrap();

        assert!(backend.add("sess", "../escape.md", b"x").is_err());
        assert!(backend.add("sess", "foo/../../etc/passwd", b"x").is_err());
        assert!(backend.add("sess", "./current.md", b"x").is_err());
    }

    // -- scenario-10: key exceeding 255 chars rejected --

    #[test]
    fn ctx_add_rejects_long_key() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        fs::create_dir_all(tmp.path().join("sess")).unwrap();

        let long_key = "a".repeat(256);
        assert!(backend.add("sess", &long_key, b"x").is_err());
    }

    // -- get on missing key --

    #[test]
    fn ctx_get_missing_key_returns_error() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        assert!(backend.get("sess", "nonexistent.md").is_err());
    }

    // -- concurrent writes to different keys --

    #[test]
    fn ctx_concurrent_writes_to_different_keys() {
        use std::sync::Arc;
        use std::thread;

        let tmp = TempDir::new().unwrap();
        let backend = Arc::new(test_backend(tmp.path()));
        fs::create_dir_all(tmp.path().join("sess")).unwrap();

        let mut handles = Vec::new();
        for i in 0..4 {
            let b = Arc::clone(&backend);
            let key = format!("file{}.md", i);
            let content = format!("content-{}", i);
            handles.push(thread::spawn(move || {
                b.add("sess", &key, content.as_bytes()).unwrap();
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        // All keys should be present.
        let keys = backend.list_keys("sess", None).unwrap();
        assert_eq!(keys.len(), 4);
        for i in 0..4 {
            let key = format!("file{}.md", i);
            let expected = format!("content-{}", i);
            let got = backend.get("sess", &key).unwrap();
            assert_eq!(got, expected.as_bytes());
        }
    }

    // -- manifest crash recovery: orphaned content without manifest entry --

    #[test]
    fn ctx_orphaned_content_without_manifest_is_harmless() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());

        // Simulate a crash after writing content but before updating manifest:
        // manually create a content file without a manifest entry.
        let ctx_dir = tmp.path().join("sess").join("ctx");
        fs::create_dir_all(&ctx_dir).unwrap();
        fs::write(ctx_dir.join("orphan.md"), b"orphaned").unwrap();

        // list_keys only reports what's in the manifest.
        let keys = backend.list_keys("sess", None).unwrap();
        assert!(keys.is_empty());

        // A new add should work fine, creating or updating the manifest.
        backend.add("sess", "real.md", b"real content").unwrap();
        let keys = backend.list_keys("sess", None).unwrap();
        assert_eq!(keys, vec!["real.md"]);

        // The orphaned file is still readable directly but not tracked.
        assert!(ctx_dir.join("orphan.md").exists());
    }
}
