use std::fs;
use std::path::{Path, PathBuf};

use crate::cache::sha256_hex;
use crate::engine::persistence::read_header;
use crate::session::validate::validate_session_id;
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

            match read_header(&state_path) {
                Ok(header) => {
                    results.push(SessionInfo {
                        id: dir_name,
                        created_at: header.created_at,
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
}

/// Derive a repo-id from a working directory path.
///
/// Canonicalizes the path, hashes with SHA-256, and returns the first
/// 16 hex characters. Two paths that resolve to the same canonical
/// location produce the same repo-id.
pub fn repo_id(working_dir: &Path) -> anyhow::Result<String> {
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
    use crate::engine::persistence::append_header;
    use crate::engine::types::StateFileHeader;
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
        };
        append_header(&state_path, &header).unwrap();
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
}
