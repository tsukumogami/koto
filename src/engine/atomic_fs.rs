//! Filesystem primitives shared by the two stores that create a file
//! exactly once.
//!
//! # Why this module is neutral
//!
//! The temp-and-rename-with-no-replace pattern lived in the session
//! backend first. The request store needs exactly the same primitive
//! for its own atomic creation, and the engine must not reach into the
//! session layer, so the platform code lives here with a neutral error
//! type and each caller maps it into its own vocabulary
//! (DESIGN-request-lifecycle.md Decision 9).
//!
//! Duplicating it instead would mean three `#[cfg]` branches of
//! syscall code in two files, which is the kind of thing that drifts.

use std::path::Path;

/// Why an atomic create failed.
///
/// [`Collision`](AtomicCreateError::Collision) is separated from the
/// I/O bucket because it is the primitive's whole point: the caller
/// needs to distinguish "someone else got there first" from "the disk
/// is unhappy" without inspecting an errno.
#[derive(Debug)]
pub enum AtomicCreateError {
    /// The destination already existed.
    Collision,
    /// Anything else.
    Io(std::io::Error),
}

impl std::fmt::Display for AtomicCreateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AtomicCreateError::Collision => write!(f, "destination already exists"),
            AtomicCreateError::Io(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for AtomicCreateError {}

/// Atomically move `src` to `dst` with "fail if destination exists"
/// semantics. On Linux this uses `renameat2(RENAME_NOREPLACE)`; on
/// other Unixes it uses POSIX `link()` followed by `unlink()`, falling
/// back to plain `rename()` on `EXDEV`. On non-Unix platforms it falls
/// back to a best-effort check-then-rename (not strictly atomic).
#[cfg(target_os = "linux")]
pub fn atomic_create_rename(src: &Path, dst: &Path) -> Result<(), AtomicCreateError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let src_c = CString::new(src.as_os_str().as_bytes()).map_err(|e| {
        AtomicCreateError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("src path contains NUL: {}", e),
        ))
    })?;
    let dst_c = CString::new(dst.as_os_str().as_bytes()).map_err(|e| {
        AtomicCreateError::Io(std::io::Error::new(
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

    Err(from_io(std::io::Error::last_os_error()))
}

/// Non-Linux Unix fallback: POSIX `link()` + `unlink()`.
///
/// `link()` fails with `EEXIST` when the destination already exists,
/// which gives us the same fail-if-exists semantics as
/// `RENAME_NOREPLACE`. On `EXDEV` (cross-device — shouldn't happen
/// because callers create the tempfile in the target directory) we
/// fall back to plain `rename()`, accepting a non-atomic window in
/// that extreme case.
#[cfg(all(unix, not(target_os = "linux")))]
pub fn atomic_create_rename(src: &Path, dst: &Path) -> Result<(), AtomicCreateError> {
    match std::fs::hard_link(src, dst) {
        Ok(()) => {
            // Link succeeded; drop the original name.
            std::fs::remove_file(src).map_err(AtomicCreateError::Io)?;
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(AtomicCreateError::Collision)
        }
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
                    return Err(AtomicCreateError::Collision);
                }
                std::fs::rename(src, dst).map_err(AtomicCreateError::Io)
            } else {
                Err(AtomicCreateError::Io(e))
            }
        }
    }
}

/// Non-Unix fallback (e.g., Windows test builds). Best-effort
/// check-then-rename with a non-atomic window.
#[cfg(not(unix))]
pub fn atomic_create_rename(src: &Path, dst: &Path) -> Result<(), AtomicCreateError> {
    if dst.exists() {
        return Err(AtomicCreateError::Collision);
    }
    std::fs::rename(src, dst).map_err(AtomicCreateError::Io)
}

/// Route `EEXIST` to [`AtomicCreateError::Collision`] and everything
/// else to the I/O bucket.
#[cfg(target_os = "linux")]
fn from_io(e: std::io::Error) -> AtomicCreateError {
    if e.kind() == std::io::ErrorKind::AlreadyExists {
        AtomicCreateError::Collision
    } else {
        AtomicCreateError::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn creates_when_destination_is_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::write(&src, b"payload").expect("write src");

        atomic_create_rename(&src, &dst).expect("rename must succeed");

        assert_eq!(fs::read(&dst).expect("read dst"), b"payload");
        assert!(!src.exists(), "source name must be gone");
    }

    #[test]
    fn refuses_an_existing_destination() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::write(&src, b"new").expect("write src");
        fs::write(&dst, b"old").expect("write dst");

        let err = atomic_create_rename(&src, &dst).expect_err("must refuse");
        assert!(
            matches!(err, AtomicCreateError::Collision),
            "want Collision, got {err:?}"
        );
        // The destination is untouched and the source survives for the
        // caller to clean up.
        assert_eq!(fs::read(&dst).expect("read dst"), b"old");
        assert!(src.exists(), "source must survive a refused rename");
    }
}
