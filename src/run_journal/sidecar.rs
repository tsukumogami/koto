//! The per-session run journal sidecar, `<session dir>/run-journal.json`.
//!
//! It caches the session's run id, so journal records need no walk up the
//! parent chain after the first, and the driver last recorded for the
//! session: the creating driver at first, then each new one a `driver_seen`
//! record names. It is local to the host, lives in the session directory,
//! and goes when the session is removed. Every failure here is non-fatal:
//! without the cache the run id is derived again from the header, and a
//! driver is recorded again as seen.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The sidecar's file name inside a session directory.
pub(crate) const SIDECAR_FILE: &str = "run-journal.json";

/// What the sidecar holds. A missing value is written as `null`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Sidecar {
    #[serde(default)]
    pub(crate) run_id: Option<String>,
    #[serde(default)]
    pub(crate) driver: Option<String>,
}

fn path(session_dir: &Path) -> PathBuf {
    session_dir.join(SIDECAR_FILE)
}

/// Read the sidecar, or `None` when it is absent or unreadable. Values that
/// are not id-shaped are dropped: the file is in the user's home and is not
/// trusted to hold what koto wrote.
pub(crate) fn read(session_dir: &Path) -> Option<Sidecar> {
    let bytes = std::fs::read(path(session_dir)).ok()?;
    let mut s: Sidecar = serde_json::from_slice(&bytes).ok()?;
    s.run_id = s.run_id.filter(|v| super::is_id(v));
    s.driver = s.driver.filter(|v| super::is_id(v));
    Some(s)
}

/// Write the sidecar by atomic rename, mode 0600. The session directory
/// must exist; nothing here creates it.
pub(crate) fn write(session_dir: &Path, sidecar: &Sidecar) -> std::io::Result<()> {
    if !session_dir.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "session directory is missing",
        ));
    }
    let bytes = serde_json::to_vec(sidecar).map_err(std::io::Error::other)?;
    // tempfile creates the file 0600 on unix.
    let mut tmp = tempfile::Builder::new()
        .prefix(".run-journal-")
        .suffix(".tmp")
        .tempfile_in(session_dir)?;
    tmp.write_all(&bytes)?;
    tmp.as_file().sync_data()?;
    tmp.persist(path(session_dir)).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_writes_nulls_for_missing_values() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Sidecar {
            run_id: Some("run-1".into()),
            driver: None,
        };
        write(tmp.path(), &s).unwrap();
        assert_eq!(read(tmp.path()), Some(s));
        let raw = std::fs::read_to_string(tmp.path().join(SIDECAR_FILE)).unwrap();
        assert_eq!(raw, r#"{"run_id":"run-1","driver":null}"#);
    }

    #[cfg(unix)]
    #[test]
    fn is_written_0600_and_leaves_no_tempfile() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), &Sidecar::default()).unwrap();
        write(tmp.path(), &Sidecar::default()).unwrap();
        let mode = std::fs::metadata(tmp.path().join(SIDECAR_FILE))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        let names: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, vec![SIDECAR_FILE.to_string()]);
    }

    #[test]
    fn a_missing_garbled_or_hostile_sidecar_reads_as_absent_or_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(read(tmp.path()), None);
        std::fs::write(tmp.path().join(SIDECAR_FILE), "not json").unwrap();
        assert_eq!(read(tmp.path()), None);
        std::fs::write(
            tmp.path().join(SIDECAR_FILE),
            r#"{"run_id":"/etc/passwd","driver":"a\nb"}"#,
        )
        .unwrap();
        assert_eq!(read(tmp.path()), Some(Sidecar::default()));
    }

    #[test]
    fn a_missing_session_directory_is_an_error_not_a_creation() {
        let tmp = tempfile::tempdir().unwrap();
        let gone = tmp.path().join("gone");
        assert!(write(&gone, &Sidecar::default()).is_err());
        assert!(!gone.exists());
    }
}
