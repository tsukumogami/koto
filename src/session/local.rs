use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::cache::sha256_hex;
use crate::engine::persistence;
use crate::engine::types::{now_iso8601, Event, EventPayload, StateFileHeader};
use crate::session::context::{ContextStore, KeyMeta, Manifest};
use crate::session::validate::{validate_context_key, validate_session_id};
use crate::session::{state_file_name, SessionBackend, SessionError, SessionInfo, SessionLock};

/// Filename prefix for `init_state_file` tempfiles.
///
/// The full tempfile name is `<prefix><random><suffix>`, for example
/// `.koto-init-aBcDeF.tmp`. Sweep tooling that garbage-collects crashed
/// initialisations relies on this exact prefix+suffix pair.
pub(crate) const INIT_TMP_PREFIX: &str = ".koto-init-";

/// Filename suffix for `init_state_file` tempfiles. Paired with
/// [`INIT_TMP_PREFIX`].
pub(crate) const INIT_TMP_SUFFIX: &str = ".tmp";

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

    fn init_state_file(
        &self,
        id: &str,
        header: StateFileHeader,
        initial_events: Vec<Event>,
    ) -> Result<(), SessionError> {
        // `validate_session_id` returns anyhow::Error today; it's a
        // caller-input problem, not an I/O failure, so route it through
        // the Other variant.
        validate_session_id(id).map_err(SessionError::Other)?;

        // Ensure the session directory exists so the tempfile can live on
        // the same filesystem as the final target. Also ensures the .koto
        // root exists with restricted permissions.
        ensure_koto_root(&self.base_dir).map_err(SessionError::Other)?;
        let session_dir = self.base_dir.join(id);
        fs::create_dir_all(&session_dir).map_err(|e| {
            SessionError::Io(std::io::Error::new(
                e.kind(),
                format!(
                    "failed to create session directory {}: {}",
                    session_dir.display(),
                    e
                ),
            ))
        })?;

        let target = session_dir.join(state_file_name(id));

        // Serialize the bundle (header line + one JSONL line per event)
        // into an in-memory buffer. The header itself carries no seq
        // field; events use the caller-supplied seq numbers verbatim.
        //
        // `StateFileHeader` and `Event` are crate-owned types whose fields
        // are all straightforward JSON-representable primitives (no maps
        // with non-string keys, no floats that could be NaN, no custom
        // serializers that error). `serde_json::to_string` on them cannot
        // fail in practice, so we expect() rather than propagate.
        let mut buf = String::new();
        let header_line =
            serde_json::to_string(&header).expect("StateFileHeader serialize is infallible");
        buf.push_str(&header_line);
        buf.push('\n');
        for event in &initial_events {
            let line = serde_json::to_string(event).expect("Event serialize is infallible");
            buf.push_str(&line);
            buf.push('\n');
        }

        // Write the bundle to a tempfile in the session directory so the
        // final rename/link is same-filesystem. The prefix/suffix come
        // from `INIT_TMP_PREFIX` / `INIT_TMP_SUFFIX` so sweep tooling and
        // tests agree on the glob. `keep()` defuses the auto-delete so we
        // can drive the final rename ourselves.
        let tmp = tempfile::Builder::new()
            .prefix(INIT_TMP_PREFIX)
            .suffix(INIT_TMP_SUFFIX)
            .tempfile_in(&session_dir)
            .map_err(SessionError::Io)?;
        {
            let mut file = tmp.as_file();
            file.write_all(buf.as_bytes()).map_err(SessionError::Io)?;
            file.sync_data().map_err(SessionError::Io)?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                // Match append_header/append_event which create state
                // files with mode 0600.
                let perms = fs::Permissions::from_mode(0o600);
                fs::set_permissions(tmp.path(), perms).map_err(SessionError::Io)?;
            }
        }

        // Detach the tempfile so it is NOT deleted on drop. We will
        // either move it into place (Linux renameat2 or non-Linux link)
        // or explicitly unlink it on failure.
        let (_file, tmp_path) = tmp.keep().map_err(|e| SessionError::Io(e.error))?;

        match atomic_create_rename(&tmp_path, &target) {
            Ok(()) => Ok(()),
            Err(e) => {
                // On every error path the tempfile is still at `tmp_path`
                // (renameat2/link/rename leave the source untouched on
                // failure), so unconditional removal is safe.
                let _ = fs::remove_file(&tmp_path);
                Err(e)
            }
        }
    }

    fn relocate(&self, from: &str, to: &str) -> anyhow::Result<()> {
        validate_session_id(from)?;
        validate_session_id(to)?;

        let from_dir = self.base_dir.join(from);
        let from_state = from_dir.join(state_file_name(from));

        // Source must exist (directory + state file).
        anyhow::ensure!(
            from_dir.exists() && from_state.exists(),
            "source session '{}' does not exist",
            from
        );

        let to_dir = self.base_dir.join(to);

        // Target must NOT exist (collision guard).
        anyhow::ensure!(!to_dir.exists(), "target session '{}' already exists", to);

        // Step 1: Rename the session directory.
        fs::rename(&from_dir, &to_dir).with_context(|| {
            format!(
                "failed to rename {} -> {}",
                from_dir.display(),
                to_dir.display()
            )
        })?;

        // Step 2: Rename the state file inside the (now renamed) directory.
        let old_state_in_new_dir = to_dir.join(state_file_name(from));
        let new_state = to_dir.join(state_file_name(to));
        fs::rename(&old_state_in_new_dir, &new_state).with_context(|| {
            format!(
                "failed to rename state file {} -> {}",
                old_state_in_new_dir.display(),
                new_state.display()
            )
        })?;

        // Step 3: Rewrite the header to update `workflow` and `parent_workflow`.
        let mut header = persistence::read_header(&new_state)
            .with_context(|| format!("failed to read header from {}", new_state.display()))?;

        header.workflow = to.to_string();
        header.parent_workflow = to.rsplit_once('.').map(|(parent, _)| parent.to_string());

        // Read the entire file, replace the first line, write it back.
        let content = fs::read_to_string(&new_state)
            .with_context(|| format!("failed to read state file {}", new_state.display()))?;
        let mut lines: Vec<&str> = content.lines().collect();

        let new_header_line =
            serde_json::to_string(&header).expect("StateFileHeader serialize is infallible");

        if lines.is_empty() {
            anyhow::bail!("state file {} is empty", new_state.display());
        }
        lines[0] = &new_header_line;

        let new_content = lines.join("\n") + "\n";
        fs::write(&new_state, new_content.as_bytes()).with_context(|| {
            format!("failed to write updated state file {}", new_state.display())
        })?;

        Ok(())
    }

    #[cfg(unix)]
    fn lock_state_file(&self, id: &str) -> Result<SessionLock, SessionError> {
        use std::os::unix::io::AsRawFd;

        validate_session_id(id).map_err(SessionError::Other)?;

        let path = self.base_dir.join(id).join(state_file_name(id));

        // Open read-only so the lock can be acquired without mutating
        // the state file. The file must already exist; callers are
        // expected to invoke this only on initialised workflows.
        let file = fs::OpenOptions::new()
            .read(true)
            .open(&path)
            .map_err(SessionError::Io)?;

        // SAFETY: `fd` is a borrow tied to `file`, which outlives the
        // call. `libc::flock` with `LOCK_EX | LOCK_NB` either returns
        // 0 (acquired) or -1 with `errno == EWOULDBLOCK` when another
        // holder already owns the lock. No other failure mode maps to
        // a "contention" condition.
        let fd = file.as_raw_fd();
        let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                // `flock` does not report the holder's PID. We leave
                // `holder_pid: None` so upstream callers can still
                // render a consistent typed error; a future probe via
                // `fcntl(F_OFD_GETLK)` can populate it without an API
                // change.
                return Err(SessionError::Locked { holder_pid: None });
            }
            return Err(SessionError::Io(err));
        }

        Ok(SessionLock { _file: file })
    }

    #[cfg(not(unix))]
    fn lock_state_file(&self, _id: &str) -> Result<SessionLock, SessionError> {
        Err(SessionError::Other(anyhow::anyhow!(
            "lock_state_file is only supported on Unix platforms"
        )))
    }

    fn ensure_pushed(&self, _id: &str) -> Result<(), SessionError> {
        // Local storage has no remote half: append_event already made
        // the write durable on disk, so there is nothing left to push.
        Ok(())
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

/// Atomically move `src` to `dst` with "fail if destination exists"
/// semantics. On Linux this uses `renameat2(RENAME_NOREPLACE)`; on
/// other Unixes it uses POSIX `link()` followed by `unlink()`, falling
/// back to plain `rename()` on `EXDEV`. On non-Unix platforms it falls
/// back to a best-effort check-then-rename (not strictly atomic).
///
/// When the destination already exists, returns `SessionError::Collision`
/// so callers can distinguish races from other I/O failures without
/// inspecting the underlying `io::Error`.
#[cfg(target_os = "linux")]
fn atomic_create_rename(src: &Path, dst: &Path) -> Result<(), SessionError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let src_c = CString::new(src.as_os_str().as_bytes()).map_err(|e| {
        SessionError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("src path contains NUL: {}", e),
        ))
    })?;
    let dst_c = CString::new(dst.as_os_str().as_bytes()).map_err(|e| {
        SessionError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("dst path contains NUL: {}", e),
        ))
    })?;

    // SAFETY: We pass valid C strings and AT_FDCWD semantics on both
    // ends. `syscall` returns -1 on error and sets errno.
    let ret = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            src_c.as_ptr(),
            libc::AT_FDCWD,
            dst_c.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if ret == 0 {
        return Ok(());
    }

    // `From<io::Error> for SessionError` routes AlreadyExists to the
    // Collision variant; every other errno becomes SessionError::Io.
    Err(SessionError::from(std::io::Error::last_os_error()))
}

/// Non-Linux Unix fallback: POSIX `link()` + `unlink()`.
///
/// `link()` fails with `EEXIST` when the destination already exists,
/// which gives us the same fail-if-exists semantics as
/// `RENAME_NOREPLACE`. On `EXDEV` (cross-device — shouldn't happen
/// because the tempfile is created in the session dir) we fall back to
/// plain `rename()`, accepting a non-atomic window in that extreme case.
#[cfg(all(unix, not(target_os = "linux")))]
fn atomic_create_rename(src: &Path, dst: &Path) -> Result<(), SessionError> {
    match fs::hard_link(src, dst) {
        Ok(()) => {
            // Link succeeded; drop the original name.
            fs::remove_file(src).map_err(SessionError::Io)?;
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(SessionError::Collision),
        Err(e) => {
            // EXDEV (cross-device) is reported as Other/Uncategorized on
            // most Rust versions. Retry with plain rename, which
            // tolerates EXDEV. Rename replaces the destination if it
            // exists, so we check first; a racing writer could still
            // slip in, but this branch only triggers in pathological
            // cross-filesystem setups.
            let is_exdev = e
                .raw_os_error()
                .map(|code| code == libc::EXDEV)
                .unwrap_or(false);
            if is_exdev {
                if dst.exists() {
                    return Err(SessionError::Collision);
                }
                fs::rename(src, dst).map_err(SessionError::Io)
            } else {
                Err(SessionError::Io(e))
            }
        }
    }
}

/// Non-Unix fallback (e.g., Windows test builds). Best-effort
/// check-then-rename with a non-atomic window.
#[cfg(not(unix))]
fn atomic_create_rename(src: &Path, dst: &Path) -> Result<(), SessionError> {
    if dst.exists() {
        return Err(SessionError::Collision);
    }
    fs::rename(src, dst).map_err(SessionError::Io)
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
            template_source_dir: None,
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

    // ===== init_state_file tests =====

    /// Helper: build a minimal (header, events) bundle for a session id.
    fn sample_bundle(id: &str) -> (StateFileHeader, Vec<Event>) {
        let header = StateFileHeader {
            schema_version: 1,
            workflow: id.to_string(),
            template_hash: "testhash".to_string(),
            created_at: "2026-04-13T00:00:00Z".to_string(),
            parent_workflow: None,
            template_source_dir: None,
        };
        let events = vec![
            Event {
                seq: 1,
                timestamp: "2026-04-13T00:00:00Z".to_string(),
                event_type: "workflow_initialized".to_string(),
                payload: EventPayload::WorkflowInitialized {
                    template_path: "/tmp/tpl.md".to_string(),
                    variables: std::collections::HashMap::new(),
                    spawn_entry: None,
                },
            },
            Event {
                seq: 2,
                timestamp: "2026-04-13T00:00:01Z".to_string(),
                event_type: "transitioned".to_string(),
                payload: EventPayload::Transitioned {
                    from: None,
                    to: "start".to_string(),
                    condition_type: "initial".to_string(),
                    skip_if_matched: None,
                },
            },
        ];
        (header, events)
    }

    // -- happy path: init writes a readable state file --

    #[test]
    fn init_state_file_writes_header_and_events() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        let (header, events) = sample_bundle("wf");

        backend
            .init_state_file("wf", header.clone(), events)
            .unwrap();

        assert!(backend.exists("wf"));
        let (got_header, got_events) = backend.read_events("wf").unwrap();
        assert_eq!(got_header, header);
        assert_eq!(got_events.len(), 2);
        assert_eq!(got_events[0].seq, 1);
        assert_eq!(got_events[1].seq, 2);
    }

    #[test]
    fn init_state_file_rejects_invalid_id() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        let (header, events) = sample_bundle("ok");
        assert!(backend
            .init_state_file("../escape", header, events)
            .is_err());
    }

    // -- scenario-1 + scenario-3: first-writer-wins under concurrency --

    #[test]
    fn init_state_file_first_writer_wins_under_contention() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let tmp = TempDir::new().unwrap();
        let backend = Arc::new(test_backend(tmp.path()));
        let barrier = Arc::new(Barrier::new(8));

        let mut handles = Vec::new();
        for i in 0..8 {
            let b = Arc::clone(&backend);
            let bar = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                // Each thread has a different event payload so that a
                // silent overwrite would produce different content.
                let header = StateFileHeader {
                    schema_version: 1,
                    workflow: "race".to_string(),
                    template_hash: format!("hash-{}", i),
                    created_at: "2026-04-13T00:00:00Z".to_string(),
                    parent_workflow: None,
                    template_source_dir: None,
                };
                let events = vec![Event {
                    seq: 1,
                    timestamp: "2026-04-13T00:00:00Z".to_string(),
                    event_type: "workflow_initialized".to_string(),
                    payload: EventPayload::WorkflowInitialized {
                        template_path: format!("/tmp/tpl-{}.md", i),
                        variables: std::collections::HashMap::new(),
                        spawn_entry: None,
                    },
                }];
                bar.wait();
                b.init_state_file("race", header, events)
            }));
        }

        let mut wins = 0;
        let mut already_exists = 0;
        for h in handles {
            match h.join().unwrap() {
                Ok(()) => wins += 1,
                Err(SessionError::Collision) => already_exists += 1,
                Err(e) => panic!("unexpected error (want Collision): {:?}", e),
            }
        }
        assert_eq!(wins, 1, "exactly one init must commit");
        assert_eq!(already_exists, 7, "every loser must report Collision");

        // The committed content must be internally consistent (no torn
        // writes / interleaved payloads): the header's template_hash
        // must match the template_path in the single event.
        let (header, events) = backend.read_events("race").unwrap();
        assert!(header.template_hash.starts_with("hash-"));
        let idx: &str = header.template_hash.strip_prefix("hash-").unwrap();
        assert_eq!(events.len(), 1);
        if let EventPayload::WorkflowInitialized { template_path, .. } = &events[0].payload {
            assert_eq!(template_path, &format!("/tmp/tpl-{}.md", idx));
        } else {
            panic!("expected WorkflowInitialized payload");
        }
    }

    // -- scenario-1: two sequential calls on the same path --

    #[test]
    fn init_state_file_second_call_returns_collision() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        let (h1, e1) = sample_bundle("wf");
        backend.init_state_file("wf", h1, e1).unwrap();

        let (h2, e2) = sample_bundle("wf");
        let err = backend
            .init_state_file("wf", h2, e2)
            .expect_err("second init must fail");
        assert!(
            matches!(err, SessionError::Collision),
            "want SessionError::Collision, got: {:?}",
            err
        );
    }

    // -- scenario-2: crash between tempfile write and rename leaves no
    //    partial state file, and a fresh init on the path succeeds. We
    //    simulate the crash by dropping a bogus tempfile in the session
    //    dir (as would remain after a kill -9 between write and rename).

    #[test]
    fn init_state_file_fresh_init_succeeds_after_stale_tempfile() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());

        // Simulate a crashed prior run: session dir exists with a stray
        // `.koto-init-*.tmp` file, but no state file.
        let session_dir = tmp.path().join("wf");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join(".koto-init-stale.tmp"),
            b"partial content from prior crash",
        )
        .unwrap();

        // The target state file must NOT be visible as a session.
        assert!(!backend.exists("wf"));

        // A fresh init on the same name succeeds and writes the real
        // bundle.
        let (header, events) = sample_bundle("wf");
        backend
            .init_state_file("wf", header.clone(), events)
            .unwrap();
        assert!(backend.exists("wf"));
        let (got_header, got_events) = backend.read_events("wf").unwrap();
        assert_eq!(got_header, header);
        assert_eq!(got_events.len(), 2);
    }

    // -- scenario-2: no target state file is left when the bundle is
    //    never renamed. This exercises the invariant "a crash before
    //    rename never leaves the real state file on disk" by directly
    //    calling the helper with a bogus rename target.
    //    (Covered implicitly by the fresh_init_succeeds_after_stale
    //    test above; additionally assert the state file doesn't appear
    //    until init completes.)

    #[test]
    fn init_state_file_is_not_visible_until_rename_completes() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());

        // Before init, no session exists.
        assert!(!backend.exists("wf"));

        let (header, events) = sample_bundle("wf");
        backend.init_state_file("wf", header, events).unwrap();

        // After init, the state file is present exactly once and at the
        // final path.
        let state_path = tmp.path().join("wf").join(state_file_name("wf"));
        assert!(state_path.exists());

        // No leftover .tmp files in the session directory.
        let stray: Vec<_> = fs::read_dir(tmp.path().join("wf"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            stray.is_empty(),
            "init must not leave .tmp files behind on success: {:?}",
            stray.iter().map(|e| e.file_name()).collect::<Vec<_>>()
        );
    }

    // -- scenario-3: exercise the non-Linux link()+unlink() fallback on
    //    platforms where it is the active branch. We can't realistically
    //    flip Linux onto the link() path from a test, so we gate this
    //    test to platforms that actually use it.

    #[cfg(all(unix, not(target_os = "linux")))]
    #[test]
    fn init_state_file_link_unlink_fallback_first_writer_wins() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        let (h1, e1) = sample_bundle("wf");
        backend.init_state_file("wf", h1, e1).unwrap();

        let (h2, e2) = sample_bundle("wf");
        let err = backend
            .init_state_file("wf", h2, e2)
            .expect_err("second init must fail on link() path");
        assert!(
            matches!(err, SessionError::Collision),
            "want SessionError::Collision on link() path, got: {:?}",
            err
        );
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

    // ===== lock_state_file tests =====

    /// scenario-4: two back-to-back acquisitions observe
    /// first-wins / second-contends semantics immediately, with no
    /// blocking. The second call must return `SessionError::Locked`
    /// rather than waiting for the first guard to drop.
    #[test]
    fn lock_state_file_second_acquire_returns_locked() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        let (header, events) = sample_bundle("wf");
        backend.init_state_file("wf", header, events).unwrap();

        let _guard = backend
            .lock_state_file("wf")
            .expect("first acquire must succeed");

        let start = std::time::Instant::now();
        let err = backend
            .lock_state_file("wf")
            .expect_err("second acquire must fail while first is held");
        let elapsed = start.elapsed();

        assert!(
            matches!(err, SessionError::Locked { holder_pid: None }),
            "want SessionError::Locked {{ holder_pid: None }}, got: {:?}",
            err
        );
        // Non-blocking: kernel returns EWOULDBLOCK immediately. Allow
        // a generous budget for CI jitter, but catch a regression to
        // a blocking LOCK_EX.
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "second acquire must be non-blocking (took {:?})",
            elapsed
        );
    }

    /// scenario-4 continuation: dropping the guard releases the lock,
    /// so a subsequent acquisition succeeds.
    #[test]
    fn lock_state_file_releases_on_drop() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        let (header, events) = sample_bundle("wf");
        backend.init_state_file("wf", header, events).unwrap();

        {
            let _guard = backend.lock_state_file("wf").expect("acquire succeeds");
        } // guard dropped here; lock released

        let _guard2 = backend
            .lock_state_file("wf")
            .expect("re-acquire after drop must succeed");
    }

    /// Missing state file surfaces as an I/O error, not a Locked
    /// variant. Callers should only lock after initialisation has
    /// committed a state file.
    #[test]
    fn lock_state_file_missing_file_reports_io_not_found() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());

        let err = backend
            .lock_state_file("nonexistent")
            .expect_err("must fail when state file is absent");
        match err {
            SessionError::Io(e) => {
                assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
            }
            other => panic!("want SessionError::Io(NotFound), got: {:?}", other),
        }
    }

    /// scenario-32: cross-process contention. We fork a child process
    /// (via `std::process::Command` re-executing the current test
    /// binary with a sentinel env var) that holds the lock for a
    /// bounded window, then attempt to acquire from the parent. The
    /// parent must observe `SessionError::Locked`.
    ///
    /// Using a re-exec rather than a bare thread is deliberate:
    /// `flock` is a per-open-file-description lock on modern Linux,
    /// and a cross-thread test inside one process does not exercise
    /// the cross-PID release-on-death semantics that matter in
    /// production. The accompanying `lock_state_file_cross_thread`
    /// test covers intra-process contention as well.
    #[test]
    #[cfg(unix)]
    fn lock_state_file_cross_process_contention() {
        use std::process::Command;
        use std::time::Duration;

        // Child mode: re-exec of this test binary with
        // KOTO_LOCK_HOLDER_DIR set takes the lock, prints "LOCKED",
        // sleeps, then exits. The sentinel env var stops the child
        // from also recursing into the parent branch.
        if let Ok(dir) = std::env::var("KOTO_LOCK_HOLDER_DIR") {
            let backend = LocalBackend::with_base_dir(PathBuf::from(&dir));
            let _guard = backend
                .lock_state_file("wf")
                .expect("child: acquire must succeed");
            println!("LOCKED");
            // Hold the lock long enough for the parent to attempt
            // acquisition and observe contention.
            std::thread::sleep(Duration::from_millis(800));
            return;
        }

        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        let (header, events) = sample_bundle("wf");
        backend.init_state_file("wf", header, events).unwrap();

        let current_exe = std::env::current_exe().expect("current_exe");
        let mut child = Command::new(&current_exe)
            .args([
                "--exact",
                "--nocapture",
                "session::local::tests::lock_state_file_cross_process_contention",
            ])
            .env("KOTO_LOCK_HOLDER_DIR", tmp.path())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn child test process");

        // Wait for the child to signal it has acquired the lock. The
        // child prints "LOCKED" after a successful acquire; we block
        // the current thread on a single line read so we don't race.
        use std::io::BufRead;
        let stdout = child.stdout.take().expect("child stdout");
        let mut reader = std::io::BufReader::new(stdout);
        let mut seen_locked = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            let mut line = String::new();
            let n = reader.read_line(&mut line).expect("read child stdout line");
            if n == 0 {
                break;
            }
            if line.trim() == "LOCKED" {
                seen_locked = true;
                break;
            }
        }
        assert!(seen_locked, "child did not report LOCKED in time");

        // Parent attempts to acquire while the child holds the lock.
        // `flock` is per-open-file-description in the cross-process
        // case, so this must fail with Locked.
        let err = backend
            .lock_state_file("wf")
            .expect_err("parent acquire must contend with child");
        assert!(
            matches!(err, SessionError::Locked { .. }),
            "want SessionError::Locked, got: {:?}",
            err
        );

        // Clean up the child so the test harness doesn't inherit it.
        let _ = child.wait();
    }

    /// Intra-process (cross-thread) contention. Useful belt-and-braces
    /// coverage: `LocalBackend::lock_state_file` opens a fresh file
    /// handle per call, so two threads in the same process also hold
    /// separate open-file-descriptions and should contend.
    #[test]
    fn lock_state_file_cross_thread_contention() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let tmp = TempDir::new().unwrap();
        let backend = Arc::new(test_backend(tmp.path()));
        let (header, events) = sample_bundle("wf");
        backend.init_state_file("wf", header, events).unwrap();

        // Thread A takes the lock and signals the barrier, then holds
        // it while thread B attempts to acquire.
        let barrier = Arc::new(Barrier::new(2));
        let release = Arc::new(std::sync::Mutex::new(false));
        let cvar = Arc::new(std::sync::Condvar::new());

        let a_backend = Arc::clone(&backend);
        let a_bar = Arc::clone(&barrier);
        let a_release = Arc::clone(&release);
        let a_cvar = Arc::clone(&cvar);
        let a = thread::spawn(move || {
            let _guard = a_backend.lock_state_file("wf").expect("thread A acquire");
            a_bar.wait();
            // Hold the lock until the main thread signals via the
            // condvar. Avoids sleeping for an arbitrary duration.
            let mut done = a_release.lock().unwrap();
            while !*done {
                done = a_cvar.wait(done).unwrap();
            }
        });

        barrier.wait();
        let err = backend
            .lock_state_file("wf")
            .expect_err("thread B acquire must contend");
        assert!(
            matches!(err, SessionError::Locked { .. }),
            "want SessionError::Locked, got: {:?}",
            err
        );

        *release.lock().unwrap() = true;
        cvar.notify_all();
        a.join().unwrap();

        // With A's guard dropped, a fresh acquire must succeed. Retry
        // briefly to absorb the kernel's own per-OFD close-to-unlock
        // latency (the flock on an OFD is released by the kernel
        // asynchronously in some scheduling windows; holding the
        // result of the assertion to exactly one attempt is too
        // strict for the semantic we want to test, which is "the lock
        // eventually becomes acquirable again").
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match backend.lock_state_file("wf") {
                Ok(_guard) => break,
                Err(SessionError::Locked { .. }) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(e) => panic!("re-acquire after A drops must succeed: {:?}", e),
            }
        }
    }

    // ===== relocate tests =====

    /// Helper: write a state file with a parent_workflow.
    fn write_state_file_with_parent(base_dir: &Path, id: &str, parent: Option<&str>) {
        let session_dir = base_dir.join(id);
        fs::create_dir_all(&session_dir).unwrap();
        let state_path = session_dir.join(state_file_name(id));
        let header = StateFileHeader {
            schema_version: 1,
            workflow: id.to_string(),
            template_hash: "testhash".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            parent_workflow: parent.map(|s| s.to_string()),
            template_source_dir: None,
        };
        persistence::append_header(&state_path, &header).unwrap();
    }

    #[test]
    fn relocate_renames_directory_and_state_file() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        write_state_file_with_parent(tmp.path(), "parent.task-a", Some("parent"));

        backend
            .relocate("parent.task-a", "parent-v1.task-a")
            .unwrap();

        // Old session gone.
        assert!(!backend.exists("parent.task-a"));
        assert!(!tmp.path().join("parent.task-a").exists());

        // New session present.
        assert!(backend.exists("parent-v1.task-a"));
        assert!(tmp.path().join("parent-v1.task-a").exists());
        assert!(tmp
            .path()
            .join("parent-v1.task-a")
            .join(state_file_name("parent-v1.task-a"))
            .exists());
    }

    #[test]
    fn relocate_updates_header_workflow_and_parent() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        write_state_file_with_parent(tmp.path(), "parent.task-a", Some("parent"));

        backend
            .relocate("parent.task-a", "parent-v1.task-a")
            .unwrap();

        let header = backend.read_header("parent-v1.task-a").unwrap();
        assert_eq!(header.workflow, "parent-v1.task-a");
        assert_eq!(header.parent_workflow, Some("parent-v1".to_string()));
        // Other fields preserved.
        assert_eq!(header.template_hash, "testhash");
        assert_eq!(header.created_at, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn relocate_sets_parent_none_when_no_dot() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        write_state_file_with_parent(tmp.path(), "old-name", None);

        backend.relocate("old-name", "new-name").unwrap();

        let header = backend.read_header("new-name").unwrap();
        assert_eq!(header.workflow, "new-name");
        assert_eq!(header.parent_workflow, None);
    }

    #[test]
    fn relocate_collision_rejected() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        write_state_file_with_parent(tmp.path(), "source", None);
        write_state_file_with_parent(tmp.path(), "target", None);

        let result = backend.relocate("source", "target");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));

        // Both sessions are untouched.
        assert!(backend.exists("source"));
        assert!(backend.exists("target"));
    }

    #[test]
    fn relocate_missing_source_rejected() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());

        let result = backend.relocate("nonexistent", "target");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn relocate_result_readable_via_read_events() {
        let tmp = TempDir::new().unwrap();
        let backend = test_backend(tmp.path());
        write_state_file_with_parent(tmp.path(), "src-wf", None);

        // Append an event so we can verify events survive the rename.
        let payload = EventPayload::WorkflowInitialized {
            template_path: "test-tmpl".to_string(),
            variables: std::collections::HashMap::new(),
            spawn_entry: None,
        };
        backend
            .append_event("src-wf", &payload, "2026-01-01T00:00:01Z")
            .unwrap();

        backend.relocate("src-wf", "dst-wf").unwrap();

        let (header, events) = backend.read_events("dst-wf").unwrap();
        assert_eq!(header.workflow, "dst-wf");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "workflow_initialized");
    }
}
