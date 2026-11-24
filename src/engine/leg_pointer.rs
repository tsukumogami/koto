//! The leg pointer's write half: [`write_pointer`]. The pointer type and
//! its readers live in koto-session-reader and are re-exported here; see
//! that module for why the pointer is a sidecar and not a header field.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

pub use koto_session_reader::engine::leg_pointer::*;

/// Write the pointer via temp-and-rename.
///
/// The rename is what makes a concurrent reader see either the old
/// pointer or the new one and never a half-written file. It also
/// replaces anything already at the path, including a planted symlink.
pub fn write_pointer(session_dir: &Path, pointer: &LegPointer) -> Result<()> {
    let path = pointer_path(session_dir);
    let body = toml::to_string(pointer).context("failed to serialize the leg pointer")?;
    let tmp = session_dir.join(format!(".{}.tmp.{}", POINTER_FILENAME, std::process::id()));

    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts
            .open(&tmp)
            .with_context(|| format!("failed to open temp {}", tmp.display()))?;
        file.write_all(body.as_bytes())
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to fsync {}", tmp.display()))?;
    }

    fs::rename(&tmp, &path).with_context(|| {
        // Leave no temp behind when the rename is the thing that failed.
        let _ = fs::remove_file(&tmp);
        format!("failed to rename {} to {}", tmp.display(), path.display())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> LegPointer {
        LegPointer {
            request_id: "req-abcdef".to_string(),
            leg_name: "reviewer-a".to_string(),
            bound_at: "2026-07-29T00:00:00.000Z".to_string(),
        }
    }

    #[test]
    fn a_session_with_no_pointer_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_pointer(dir.path()).unwrap(), None);
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        write_pointer(dir.path(), &sample()).unwrap();
        assert_eq!(read_pointer(dir.path()).unwrap(), Some(sample()));
    }

    #[test]
    fn a_second_write_replaces_the_first_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        write_pointer(dir.path(), &sample()).unwrap();
        let mut second = sample();
        second.leg_name = "reviewer-b".to_string();
        write_pointer(dir.path(), &second).unwrap();
        assert_eq!(read_pointer(dir.path()).unwrap(), Some(second));

        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != POINTER_FILENAME)
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp-and-rename must leave nothing behind, found {leftovers:?}"
        );
    }

    #[test]
    fn the_pointer_carries_no_dispatch_epoch() {
        let body = toml::to_string(&sample()).unwrap();
        assert!(
            !body.contains("dispatch_epoch") && !body.contains("epoch"),
            "a readable epoch would let a displaced agent present the current value \
             and defeat the fence: {body}"
        );
    }

    #[test]
    fn a_malformed_pointer_is_an_error_not_a_none() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(pointer_path(dir.path()), "this is not toml = = =").unwrap();
        assert!(read_pointer(dir.path()).is_err());
        assert_eq!(
            read_pointer_best_effort(dir.path()),
            None,
            "the best-effort read still degrades to no-pointer"
        );
    }

    #[test]
    fn a_different_request_or_leg_is_recognized_as_different() {
        let p = sample();
        assert!(!p.names_a_different_leg("req-abcdef", "reviewer-a"));
        assert!(p.names_a_different_leg("req-abcdef", "reviewer-b"));
        assert!(p.names_a_different_leg("req-999999", "reviewer-a"));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_at_the_pointer_path_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("elsewhere.toml");
        std::fs::write(&target, toml::to_string(&sample()).unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, pointer_path(dir.path())).unwrap();
        assert!(
            read_pointer(dir.path()).is_err(),
            "O_NOFOLLOW must refuse a planted symlink"
        );
    }
}
