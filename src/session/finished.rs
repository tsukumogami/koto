//! Finished records: what a session that koto removed itself ended as.
//!
//! koto removes a session at its terminal tick (and a parent removes its
//! kept terminal children), which used to leave `koto status` answering
//! `not found` for a run that finished. Just before that removal koto writes
//! one record per name to `<sessions base>/.finished/<name>.json`, so the
//! status of a removed session can still be read.
//!
//! The records are derived and safe to delete. They live beside the
//! sessions of the store that removed them, so a redirected store keeps its
//! own, and the cloud backend keeps them in its local store; they never
//! sync. Creating a session deletes its name's record, and
//! `koto session prune` removes them.
//!
//! The directory is created mode 0700 and is refused when it is a symlink;
//! each record is written mode 0600 to a temporary file and renamed into
//! place, so a reader sees a whole record or none. A read refuses a
//! symlinked record and one whose `name` differs from its file name.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// The directory under the sessions base that holds the records.
pub const FINISHED_DIR: &str = ".finished";

/// The largest record written. A result that would make it larger is cut to
/// its `status` and `summary`.
pub const MAX_RECORD_BYTES: usize = 64 * 1024;

/// The record format version.
pub const RECORD_VERSION: u32 = 1;

/// What removed the session.
pub const REMOVED_BY_TERMINAL: &str = "terminal";
pub const REMOVED_BY_PARENT: &str = "parent";

/// How a removed session ended.
pub const STATUS_TERMINAL: &str = "terminal";
pub const STATUS_CANCELLED: &str = "cancelled";

/// One removed session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FinishedRecord {
    pub v: u32,
    pub name: String,
    pub session_id: Option<String>,
    pub execution_dir: Option<PathBuf>,
    /// The state the session ended in: its terminal state, or the state it
    /// was cancelled in.
    pub state: String,
    /// `terminal` or `cancelled`.
    pub status: String,
    pub failure: bool,
    /// The result `koto status` reported for the session; null for a
    /// cancelled one.
    pub result: Option<serde_json::Value>,
    /// Set when the result was cut to fit [`MAX_RECORD_BYTES`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// When koto wrote the record, just before the removal.
    pub finished_at: String,
    /// `terminal` or `parent`.
    pub removed_by: String,
}

/// The records directory of the store whose session directories sit
/// directly under `base`.
pub fn finished_dir(base: &Path) -> PathBuf {
    base.join(FINISHED_DIR)
}

/// The sessions base of the store `session_dir` belongs to.
pub fn base_of(session_dir: &Path) -> Option<&Path> {
    session_dir.parent()
}

fn record_path(base: &Path, name: &str) -> PathBuf {
    finished_dir(base).join(format!("{name}.json"))
}

/// The records directory, refused when it is anything but a real directory.
/// `Ok(None)` when it doesn't exist.
fn checked_dir(base: &Path) -> Result<Option<PathBuf>> {
    let dir = finished_dir(base);
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if meta.file_type().is_dir() => Ok(Some(dir)),
        Ok(_) => anyhow::bail!("{} isn't a directory", dir.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", dir.display())),
    }
}

/// Serialize `record`, cutting its result to `status` and `summary` when the
/// whole would pass [`MAX_RECORD_BYTES`].
fn encode(record: &FinishedRecord) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(record)?;
    if bytes.len() <= MAX_RECORD_BYTES {
        return Ok(bytes);
    }
    let mut cut = record.clone();
    cut.result = record.result.as_ref().map(|r| {
        let mut kept = serde_json::Map::new();
        for key in ["status", "summary"] {
            if let Some(v) = r.get(key) {
                kept.insert(key.to_string(), v.clone());
            }
        }
        serde_json::Value::Object(kept)
    });
    cut.truncated = true;
    let bytes = serde_json::to_vec(&cut)?;
    if bytes.len() > MAX_RECORD_BYTES {
        // A summary alone past the cap: keep the status only.
        cut.result = record
            .result
            .as_ref()
            .and_then(|r| r.get("status").cloned())
            .map(|status| serde_json::json!({ "status": status }));
        return Ok(serde_json::to_vec(&cut)?);
    }
    Ok(bytes)
}

/// Write `record` for `record.name` under the store at `base`, replacing any
/// earlier record for the name.
#[cfg(unix)]
pub fn write(base: &Path, record: &FinishedRecord) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

    crate::session::validate::validate_session_id(&record.name)?;
    let dir = match checked_dir(base)? {
        Some(dir) => dir,
        None => {
            let dir = finished_dir(base);
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&dir)
                .or_else(|e| match e.kind() {
                    std::io::ErrorKind::AlreadyExists => Ok(()),
                    _ => Err(e),
                })
                .with_context(|| format!("failed to create {}", dir.display()))?;
            // Whoever won a race to create it, it must be a real directory.
            checked_dir(base)?.context("the records directory vanished")?
        }
    };
    let bytes = encode(record)?;
    let tmp = dir.join(format!(".{}.{}.tmp", record.name, std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&tmp)
            .with_context(|| format!("failed to create {}", tmp.display()))?;
        file.write_all(&bytes)
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        file.sync_data()
            .with_context(|| format!("failed to flush {}", tmp.display()))?;
        std::fs::rename(&tmp, record_path(base, &record.name))
            .with_context(|| format!("failed to move {} into place", tmp.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Read the record for `name` under the store at `base`. `Ok(None)` when
/// there is none.
pub fn read(base: &Path, name: &str) -> Result<Option<FinishedRecord>> {
    if crate::session::validate::validate_session_id(name).is_err() {
        return Ok(None);
    }
    if checked_dir(base)?.is_none() {
        return Ok(None);
    }
    let path = record_path(base, name);
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_file() => {}
        Ok(_) => anyhow::bail!("{} isn't a regular file", path.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
    let bytes =
        std::fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let record: FinishedRecord = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    if record.name != name {
        anyhow::bail!(
            "{} names session '{}', not '{}'",
            path.display(),
            record.name,
            name
        );
    }
    Ok(Some(record))
}

/// Delete the record for `name` under the store at `base`, if there is one.
pub fn remove(base: &Path, name: &str) -> Result<()> {
    crate::session::validate::validate_session_id(name)?;
    if checked_dir(base)?.is_none() {
        return Ok(());
    }
    match std::fs::remove_file(record_path(base, name)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("failed to remove the record for {name}")),
    }
}

/// Every record that reads under the store at `base`, by name. A file that
/// isn't a record (a temporary file, one that doesn't parse or names another
/// session) is skipped.
pub fn list(base: &Path) -> Result<Vec<FinishedRecord>> {
    let Some(dir) = checked_dir(base)? else {
        return Ok(Vec::new());
    };
    let mut records = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .with_context(|| format!("failed to read {}", dir.display()))?
        .flatten()
    {
        let file = entry.file_name();
        let Some(name) = file.to_str().and_then(|f| f.strip_suffix(".json")) else {
            continue;
        };
        if let Ok(Some(record)) = read(base, name) {
            records.push(record);
        }
    }
    records.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(records)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn record(name: &str) -> FinishedRecord {
        FinishedRecord {
            v: RECORD_VERSION,
            name: name.to_string(),
            session_id: Some("id-1".to_string()),
            execution_dir: Some(PathBuf::from("/work/project")),
            state: "done".to_string(),
            status: STATUS_TERMINAL.to_string(),
            failure: false,
            result: Some(serde_json::json!({"status": "success", "summary": "done"})),
            truncated: false,
            finished_at: "2026-10-12T00:00:00.000Z".to_string(),
            removed_by: REMOVED_BY_TERMINAL.to_string(),
        }
    }

    #[test]
    fn a_record_round_trips_with_private_modes() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path();
        write(base, &record("a")).unwrap();
        assert_eq!(read(base, "a").unwrap(), Some(record("a")));
        let dir_mode = std::fs::metadata(finished_dir(base))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(dir_mode & 0o777, 0o700);
        let file_mode = std::fs::metadata(record_path(base, "a"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(file_mode & 0o777, 0o600);
        assert_eq!(read(base, "b").unwrap(), None);
        assert_eq!(list(base).unwrap().len(), 1);
        remove(base, "a").unwrap();
        assert_eq!(read(base, "a").unwrap(), None);
        remove(base, "a").unwrap();
    }

    #[test]
    fn a_result_past_the_cap_keeps_status_and_summary() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut big = record("big");
        big.result = Some(serde_json::json!({
            "status": "success",
            "summary": "short",
            "payload": {"blob": "x".repeat(MAX_RECORD_BYTES)},
        }));
        write(tmp.path(), &big).unwrap();
        let got = read(tmp.path(), "big").unwrap().unwrap();
        assert!(got.truncated);
        assert_eq!(
            got.result,
            Some(serde_json::json!({"status": "success", "summary": "short"}))
        );
        assert!(
            std::fs::metadata(record_path(tmp.path(), "big"))
                .unwrap()
                .len() as usize
                <= MAX_RECORD_BYTES
        );
    }

    #[test]
    fn a_symlinked_directory_or_record_and_a_mismatched_name_are_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();

        let linked = tmp.path().join("linked");
        std::fs::create_dir_all(&linked).unwrap();
        std::os::unix::fs::symlink(&elsewhere, finished_dir(&linked)).unwrap();
        assert!(write(&linked, &record("a")).is_err());
        assert!(read(&linked, "a").is_err());
        assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none());

        let base = tmp.path().join("base");
        std::fs::create_dir_all(finished_dir(&base)).unwrap();
        std::os::unix::fs::symlink(elsewhere.join("x.json"), record_path(&base, "a")).unwrap();
        assert!(read(&base, "a").is_err());

        std::fs::write(
            record_path(&base, "b"),
            serde_json::to_vec(&record("c")).unwrap(),
        )
        .unwrap();
        assert!(read(&base, "b").is_err());
        // A listing skips what doesn't read.
        assert!(list(&base).unwrap().is_empty());
    }
}
