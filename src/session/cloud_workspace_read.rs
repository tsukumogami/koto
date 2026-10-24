//! Reading a session in another workspace's part of the bucket, in place:
//! `koto --workspace <id> status|context get|context exists|context list`
//! (koto#455).
//!
//! A session belongs to the workspace that created it, and its objects live
//! under that workspace's prefix. Reading it from another workspace used to
//! mean importing it, which moves it and marks the source migrated. A
//! [`WorkspaceReader`] reads it where it is instead: a [`CloudBackend`] on
//! the named workspace's prefix, whose local store is a private scratch
//! directory that goes away with the reader, and which is read-only.
//!
//! Read-only means the reader writes nothing to the bucket and nothing to
//! this host's session store:
//!
//! - every PUT and DELETE, every log append, every key write or removal and
//!   every cleanup the backend would make refuses (`CloudBackend::refuse_if_read_only`);
//! - a `koto next` tick marker in the environment doesn't apply, so a
//!   reader running inside a tick of a same-named session in its own
//!   workspace neither defers into a batch nor counts as that session's tick;
//! - whatever a read pulls (the state file, a context key, the compiled
//!   template) lands in the scratch directory, never in `~/.koto/sessions`,
//!   which every workspace on the host shares by name.
//!
//! The reads themselves are the backend's own: the session's one listing
//! ([`CloudBackend::probe`]) answers the migration check, the state pull and
//! the manifest, as it does for a session in this workspace. A session
//! imported elsewhere refuses with `session_migrated`, as it would here.
//!
//! [`WorkspaceReader::sessions`] is the strict listing of the workspace's
//! sessions, which tells an empty workspace from an unreadable bucket.

use std::path::Path;

use super::*;

/// A workspace named on the command line: its bucket prefix, and the text
/// the caller gave, for messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceId {
    prefix: String,
    given: String,
}

impl WorkspaceId {
    /// A workspace id (the 16 lowercase hex digits of its bucket prefix), or
    /// the workspace's absolute path, which hashes to its prefix as
    /// `koto session import --from` does. Anything else is refused.
    pub fn parse(given: &str) -> Result<Self, WorkspaceReadError> {
        let prefix = if is_workspace_prefix(given) {
            given.to_string()
        } else if Path::new(given).is_absolute() {
            crate::session::local::prefix_for_workspace(Path::new(given))
                .map_err(|e| WorkspaceReadError::new(WorkspaceReadErrorCode::InvalidWorkspace, e))?
        } else {
            return Err(WorkspaceReadError::new(
                WorkspaceReadErrorCode::InvalidWorkspace,
                format!(
                    "--workspace takes a workspace id (16 lowercase hex digits, the workspace's \
                     prefix in the bucket) or the workspace's absolute path, got {:?}",
                    given
                ),
            ));
        };
        Ok(Self {
            prefix,
            given: given.to_string(),
        })
    }

    /// The workspace's prefix in the bucket.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The workspace for a message: its id, and the path it came from when
    /// the caller gave one.
    pub fn describe(&self) -> String {
        if self.given == self.prefix {
            self.prefix.clone()
        } else {
            format!("{} ({})", self.prefix, self.given)
        }
    }
}

/// Why a read through `--workspace` stopped, as the code it prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceReadErrorCode {
    /// `--workspace` was given something that names no workspace.
    InvalidWorkspace,
    /// The session name isn't one koto could have created.
    InvalidSession,
    /// The bucket holds no session under the workspace's prefix.
    WorkspaceNotFound,
    /// The workspace has sessions in the bucket, but not this one.
    SessionNotFound,
    /// The bucket couldn't be read: a listing or a GET failed.
    Unreadable,
}

impl WorkspaceReadErrorCode {
    /// The `code` field of the error envelope.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidWorkspace => "invalid_workspace",
            Self::InvalidSession => "invalid_session",
            Self::WorkspaceNotFound => "workspace_not_found",
            Self::SessionNotFound => "session_not_found",
            Self::Unreadable => "workspace_unreadable",
        }
    }

    /// 2 for what the caller named (not there, or not a name), 3 for a
    /// bucket that couldn't be read, as koto's other commands exit.
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Unreadable => 3,
            _ => 2,
        }
    }
}

/// A refused read through `--workspace`.
#[derive(Debug, Clone)]
pub struct WorkspaceReadError {
    pub code: WorkspaceReadErrorCode,
    pub message: String,
}

impl WorkspaceReadError {
    pub fn new(code: WorkspaceReadErrorCode, message: impl std::fmt::Display) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }
}

impl std::fmt::Display for WorkspaceReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for WorkspaceReadError {}

/// A read-only [`CloudBackend`] on another workspace's prefix, with the
/// scratch directory it keeps its copies in.
pub struct WorkspaceReader {
    backend: CloudBackend,
    workspace: WorkspaceId,
    scratch: tempfile::TempDir,
}

impl WorkspaceReader {
    /// A reader of `workspace` in the bucket `cloud_config` names, keeping
    /// its copies in a new private directory under the system's temporary
    /// directory.
    pub fn open(cloud_config: &CloudConfig, workspace: WorkspaceId) -> anyhow::Result<Self> {
        Self::with_bucket(create_bucket(cloud_config)?, workspace)
    }

    /// [`WorkspaceReader::open`] with the bucket given, for tests.
    #[doc(hidden)]
    pub fn with_bucket(bucket: Box<Bucket>, workspace: WorkspaceId) -> anyhow::Result<Self> {
        let scratch = tempfile::Builder::new()
            .prefix("koto-workspace-read-")
            .tempdir()
            .context("creating the scratch directory for a --workspace read")?;
        let local = LocalBackend::with_base_dir(scratch.path().to_path_buf());
        let mut backend = CloudBackend::with_parts(local, bucket, workspace.prefix().to_string());
        backend.probes.read_only = true;
        Ok(Self {
            backend,
            workspace,
            scratch,
        })
    }

    /// The scratch directory the reader's copies are kept in, removed when
    /// the reader is dropped.
    pub fn scratch_dir(&self) -> &Path {
        self.scratch.path()
    }

    /// The backend, to read through, and the scratch directory, which must
    /// outlive it.
    pub fn into_parts(self) -> (CloudBackend, tempfile::TempDir) {
        (self.backend, self.scratch)
    }

    /// An unreadable bucket, as printed on stdout: the request's error with
    /// any credentials in the endpoint's URL removed.
    fn unreadable(&self, what: &str, e: impl std::fmt::Display) -> WorkspaceReadError {
        WorkspaceReadError::new(
            WorkspaceReadErrorCode::Unreadable,
            format!(
                "could not read {} of workspace {} from the bucket: {}",
                what,
                self.workspace.describe(),
                without_url_userinfo(&e.to_string())
            ),
        )
    }

    /// The names of the sessions the workspace holds in the bucket. An empty
    /// list is a workspace with no session there; a listing that failed is
    /// [`WorkspaceReadErrorCode::Unreadable`], never an empty list.
    pub fn sessions(&self) -> Result<Vec<String>, WorkspaceReadError> {
        self.backend
            .try_list_sessions()
            .map_err(|e| self.unreadable("the session listing", e))
    }

    /// Find session `name` under the workspace's prefix and bring its state
    /// file into the scratch directory, so the reads that follow find it.
    ///
    /// One listing of the session's prefix (the probe every read shares)
    /// says whether it is there. A session carrying a migration marker is
    /// left to the reads, which refuse it with `session_migrated` naming
    /// where it went. When the session isn't there, a listing of the
    /// workspace tells a workspace with no sessions from a missing session.
    /// A failed listing or GET is [`WorkspaceReadErrorCode::Unreadable`].
    pub fn locate(&self, name: &str) -> Result<(), WorkspaceReadError> {
        crate::session::validate::validate_session_id(name)
            .map_err(|e| WorkspaceReadError::new(WorkspaceReadErrorCode::InvalidSession, e))?;
        let probe = self
            .backend
            .probe(name)
            .map_err(|e| self.unreadable(&format!("session '{}'", name), e))?;
        if probe.marker {
            return Ok(());
        }
        let Some(etag) = probe.state_etag else {
            return Err(self.not_found(name));
        };
        let key = self.backend.state_key(name);
        let bytes = self
            .backend
            .fetch_object(&key)
            .map_err(|e| self.unreadable(&format!("the state file of '{}'", name), e))?
            .ok_or_else(|| self.not_found(name))?;
        let dir = self
            .backend
            .local
            .create(name)
            .map_err(|e| self.unreadable(&format!("session '{}'", name), e))?;
        std::fs::write(dir.join(state_file_name(name)), &bytes)
            .map_err(|e| self.unreadable(&format!("the state file of '{}'", name), e))?;
        self.backend.record_synced(name, Some(&etag), &bytes);
        Ok(())
    }

    /// Not found: the session, or the whole workspace when the bucket holds
    /// no session under its prefix. Unreadable when the listing that tells
    /// the two apart fails.
    fn not_found(&self, name: &str) -> WorkspaceReadError {
        match self.sessions() {
            Err(e) => e,
            Ok(names) if names.is_empty() => WorkspaceReadError::new(
                WorkspaceReadErrorCode::WorkspaceNotFound,
                format!(
                    "workspace {} has no sessions in the bucket",
                    self.workspace.describe()
                ),
            ),
            Ok(_) => WorkspaceReadError::new(
                WorkspaceReadErrorCode::SessionNotFound,
                format!(
                    "session '{}' not found in workspace {}",
                    name,
                    self.workspace.describe()
                ),
            ),
        }
    }

    /// Put the compiled template session `name` was initialized with where
    /// `koto status` looks for it, when this host doesn't have it: the
    /// session's `template.json` from the bucket, checked against the hash
    /// its header records, in the scratch copy of the session. A template
    /// this host already has, or a session that never pushed one, leaves
    /// the scratch directory as it is, and `status` reports what it finds.
    /// Call it after [`WorkspaceReader::locate`].
    pub fn fetch_template(&self, name: &str) -> Result<(), WorkspaceReadError> {
        let Ok((header, events)) = self.backend.local.read_events(name) else {
            return Ok(());
        };
        let Some(recorded) = events.iter().find_map(|e| match &e.payload {
            crate::engine::types::EventPayload::WorkflowInitialized { template_path, .. } => {
                Some(template_path.clone())
            }
            _ => None,
        }) else {
            return Ok(());
        };
        let session_dir = self.backend.local.session_dir(name);
        let resolved =
            crate::engine::persistence::resolve_template_path_in_session(&recorded, &session_dir);
        if resolved.is_empty() || Path::new(&resolved).exists() {
            return Ok(());
        }
        // An absolute path this host lacks resolves to the file of the same
        // name in the session directory; a relative one resolves there too.
        // The recorded path comes from the bucket, so a relative one is
        // taken only when every part of it is a plain name: a `..` would
        // still pass a prefix check on the joined path and write outside
        // the scratch directory.
        let recorded_path = Path::new(&recorded);
        let target = if recorded_path.is_absolute() {
            match recorded_path.file_name() {
                Some(file) => session_dir.join(file),
                None => return Ok(()),
            }
        } else if recorded_path
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
        {
            session_dir.join(recorded_path)
        } else {
            return Ok(());
        };
        let Some(bytes) = self
            .backend
            .get_object_if_present(&self.backend.template_key(name))
            .map_err(|e| self.unreadable(&format!("the compiled template of '{}'", name), e))?
        else {
            return Ok(());
        };
        if crate::cache::sha256_hex(&bytes) != header.template_hash {
            return Ok(());
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| self.unreadable(&format!("the compiled template of '{}'", name), e))?;
        }
        std::fs::write(&target, &bytes)
            .map_err(|e| self.unreadable(&format!("the compiled template of '{}'", name), e))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_taken_as_the_prefix() {
        let id = WorkspaceId::parse("0123456789abcdef").unwrap();
        assert_eq!(id.prefix(), "0123456789abcdef");
        assert_eq!(id.describe(), "0123456789abcdef");
    }

    #[test]
    fn an_absolute_path_hashes_to_its_prefix() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();
        let id = WorkspaceId::parse(path).unwrap();
        assert_eq!(
            id.prefix(),
            crate::session::local::prefix_for_workspace(dir.path()).unwrap()
        );
        assert!(id.describe().contains(path));
    }

    #[test]
    fn anything_else_is_refused_as_an_invalid_workspace() {
        for given in [
            "",
            "0123456789ABCDEF",
            "0123456789abcde",
            "relative/dir",
            "name:shared",
        ] {
            let err = WorkspaceId::parse(given).unwrap_err();
            assert_eq!(
                err.code,
                WorkspaceReadErrorCode::InvalidWorkspace,
                "{given:?}"
            );
            assert!(err.message.contains("--workspace"), "{}", err.message);
        }
    }

    /// A request error that names the endpoint's URL, credentials and all,
    /// reaches the printed envelope without them.
    #[test]
    fn an_unreadable_error_drops_the_endpoints_credentials() {
        let reader = WorkspaceReader::with_bucket(
            test_support::cloud_backend_at(Path::new("/nonexistent"), "http://127.0.0.1:1".into())
                .bucket,
            WorkspaceId::parse("0123456789abcdef").unwrap(),
        )
        .unwrap();
        let err = reader.unreadable(
            "the session listing",
            "error sending request for url (http://reader:hunter2@127.0.0.1:1/qa?list-type=2)",
        );
        assert_eq!(err.code, WorkspaceReadErrorCode::Unreadable);
        assert!(!err.message.contains("hunter2"), "{}", err.message);
        assert!(
            err.message.contains("http://127.0.0.1:1/qa"),
            "{}",
            err.message
        );
    }

    #[test]
    fn the_reader_is_read_only_and_ignores_a_tick_marker() {
        let reader = WorkspaceReader::with_bucket(
            test_support::cloud_backend_at(Path::new("/nonexistent"), "http://127.0.0.1:1".into())
                .bucket,
            WorkspaceId::parse("0123456789abcdef").unwrap(),
        )
        .unwrap();
        let backend = &reader.backend;
        assert!(backend.probes.read_only);
        let err = backend
            .put_object("0123456789abcdef/s/x", b"x")
            .unwrap_err();
        assert!(err.to_string().contains("--workspace"), "{err}");
        let err = backend.delete_object("0123456789abcdef/s/x").unwrap_err();
        assert!(err.to_string().contains("--workspace"), "{err}");
        let err = backend.add("s", "k", b"x").unwrap_err();
        assert!(err.to_string().contains("--workspace"), "{err}");
        let err = backend.remove("s", "k").unwrap_err();
        assert!(err.to_string().contains("--workspace"), "{err}");
        let err = backend.cleanup("s").unwrap_err();
        assert!(err.to_string().contains("--workspace"), "{err}");
        assert!(reader.scratch_dir().is_dir());
    }
}
