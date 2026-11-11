//! The listing seam the reader takes in place of koto's `SessionBackend`.
//!
//! koto's backend trait carries writes, locks and the cloud backend; the
//! reader needs three read-only answers from it. `LocalDirSource` answers
//! them straight from a sessions directory, the way `LocalBackend::list`
//! and `LocalBackend::count_unreadable` do, and opens no file for writing.

use std::fs;
use std::path::{Path, PathBuf};

use crate::engine::persistence;
use crate::engine::types::SessionKind;

/// koto's state file name convention, `koto-<id>.state.jsonl`.
pub fn state_file_name(id: &str) -> String {
    format!("koto-{}.state.jsonl", id)
}

/// koto's close-session staging prefix (`session::anchor::STAGING_PREFIX`).
const STAGING_PREFIX: &str = ".koto-staging-";

/// What the reader needs to know about one listed session.
pub struct SessionInfo {
    pub id: String,
    pub kind: Option<String>,
}

impl SessionInfo {
    pub fn session_kind(&self) -> SessionKind {
        SessionKind::of(self.kind.as_deref())
    }
}

/// The three read-only answers the dashboard data layer asks a backend for.
pub trait SessionSource {
    fn list(&self) -> anyhow::Result<Vec<SessionInfo>>;
    fn session_dir(&self, id: &str) -> PathBuf;
    fn count_unreadable(&self) -> usize;
}

/// A sessions directory read in place: `~/.koto/sessions`, or the cloud
/// backend's local mirror, which has the same layout.
pub struct LocalDirSource {
    base_dir: PathBuf,
}

impl LocalDirSource {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }
}

impl SessionSource for LocalDirSource {
    fn list(&self) -> anyhow::Result<Vec<SessionInfo>> {
        let mut results = Vec::new();
        let entries = match fs::read_dir(&self.base_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(results),
            Err(e) => anyhow::bail!(
                "failed to read directory {}: {}",
                self.base_dir.display(),
                e
            ),
        };
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let Some(dir_name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if dir_name.starts_with(STAGING_PREFIX) {
                continue;
            }
            let state_path = entry.path().join(state_file_name(&dir_name));
            if !state_path.exists() {
                continue;
            }
            if let Ok(header) = persistence::read_header(&state_path) {
                results.push(SessionInfo {
                    id: dir_name,
                    kind: header.kind,
                });
            }
        }
        results.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(results)
    }

    fn session_dir(&self, id: &str) -> PathBuf {
        self.base_dir.join(id)
    }

    fn count_unreadable(&self) -> usize {
        let Ok(entries) = fs::read_dir(&self.base_dir) else {
            return 0;
        };
        let mut count = 0;
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let Some(dir_name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if dir_name.contains('~') {
                continue;
            }
            let state_path = entry.path().join(state_file_name(&dir_name));
            if state_path.exists() && persistence::read_header(&state_path).is_err() {
                count += 1;
            }
        }
        count
    }
}
