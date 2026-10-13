//! Filesystem primitives shared by the two stores that create a file
//! exactly once, and the private-directory helpers the request store and
//! the wake signal both use.
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

/// Move the directory `src` to `dst`, failing with
/// [`Collision`](AtomicCreateError::Collision) when anything is at `dst`,
/// an empty directory included (a plain `rename` silently replaces one).
/// Linux uses `renameat2(RENAME_NOREPLACE)`, as
/// [`atomic_create_rename`] does.
#[cfg(target_os = "linux")]
pub fn atomic_rename_dir(src: &Path, dst: &Path) -> Result<(), AtomicCreateError> {
    atomic_create_rename(src, dst)
}

/// macOS: `renamex_np(RENAME_EXCL)`. `link()`, the file fallback, can't
/// link a directory.
#[cfg(target_vendor = "apple")]
pub fn atomic_rename_dir(src: &Path, dst: &Path) -> Result<(), AtomicCreateError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c = |p: &Path| {
        CString::new(p.as_os_str().as_bytes()).map_err(|e| {
            AtomicCreateError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("path contains NUL: {}", e),
            ))
        })
    };
    let (src_c, dst_c) = (c(src)?, c(dst)?);
    // SAFETY: both are valid C strings; -1 sets errno.
    let ret = unsafe { libc::renamex_np(src_c.as_ptr(), dst_c.as_ptr(), libc::RENAME_EXCL) };
    if ret == 0 {
        return Ok(());
    }
    Err(from_io(std::io::Error::last_os_error()))
}

/// Elsewhere: best-effort check-then-rename, with a non-atomic window.
#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
pub fn atomic_rename_dir(src: &Path, dst: &Path) -> Result<(), AtomicCreateError> {
    if dst.exists() {
        return Err(AtomicCreateError::Collision);
    }
    std::fs::rename(src, dst).map_err(AtomicCreateError::Io)
}

/// Refuse to read or write through a symlink at `path`.
///
/// A planted symlink would let a foothold redirect a write into a file the
/// operator never meant to touch. An absent path is fine: the caller is
/// about to create it.
pub fn reject_symlink(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(md) if md.file_type().is_symlink() => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("refusing to follow the symlink at {}", path.display()),
        )),
        _ => Ok(()),
    }
}

/// Create a directory with mode 0700, rather than relying on the home
/// directory's mode having been set correctly once. A symlink at `path`
/// is refused, and an existing directory is left as it is.
pub fn create_private_dir(path: &Path) -> std::io::Result<()> {
    reject_symlink(path)?;
    if path.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Swap two existing directories (or files) in one call: afterwards `a`
/// holds what `b` held and `b` what `a` held, and at no instant is either
/// name empty. Linux uses `renameat2(RENAME_EXCHANGE)`, macOS
/// `renamex_np(RENAME_SWAP)`; elsewhere it reports `Unsupported`.
///
/// The same-host import takeover hands a session name from the source's
/// copy to the imported session with it (DESIGN-session-migration's
/// cloud-session-config amendment): with two renames the name would be
/// empty between them, and a concurrent command could re-create it.
#[cfg(target_os = "linux")]
pub fn exchange_dirs(a: &Path, b: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = |p: &Path| {
        CString::new(p.as_os_str().as_bytes()).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("path contains NUL: {}", e),
            )
        })
    };
    let (a_c, b_c) = (c(a)?, c(b)?);
    // SAFETY: valid C strings, AT_FDCWD on both ends; -1 sets errno.
    let ret = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            a_c.as_ptr(),
            libc::AT_FDCWD,
            b_c.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    if ret == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// macOS: `renamex_np(RENAME_SWAP)`, the call [`atomic_rename_dir`] makes
/// with `RENAME_EXCL`.
#[cfg(target_vendor = "apple")]
pub fn exchange_dirs(a: &Path, b: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = |p: &Path| {
        CString::new(p.as_os_str().as_bytes()).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("path contains NUL: {}", e),
            )
        })
    };
    let (a_c, b_c) = (c(a)?, c(b)?);
    // SAFETY: both are valid C strings; -1 sets errno.
    let ret = unsafe { libc::renamex_np(a_c.as_ptr(), b_c.as_ptr(), libc::RENAME_SWAP) };
    if ret == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Elsewhere there is no atomic exchange.
#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
pub fn exchange_dirs(_a: &Path, _b: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no atomic directory exchange on this platform",
    ))
}

/// Whether [`exchange_dirs`] works inside `dir`, checked without touching
/// anything but two empty scratch directories it creates and removes.
///
/// `false` when `dir` isn't a directory owned by the invoking user (on
/// Unix), when the scratch directories can't be made, or when the
/// filesystem refuses the exchange (CIFS/SMB, FAT and hfsplus accept a
/// no-replace rename but not an exchange; NFS refuses both).
pub fn exchange_supported(dir: &Path) -> bool {
    let md = match std::fs::symlink_metadata(dir) {
        Ok(md) if md.is_dir() => md,
        _ => return false,
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions and can't fail.
        if md.uid() != unsafe { libc::geteuid() } {
            return false;
        }
    }
    #[cfg(not(unix))]
    let _ = md;
    let tag = format!(
        ".exchange-probe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let a = dir.join(format!("{}-a", tag));
    let b = dir.join(format!("{}-b", tag));
    let made = std::fs::create_dir(&a).is_ok() && std::fs::create_dir(&b).is_ok();
    let swapped = made && exchange_dirs(&a, &b).is_ok();
    let _ = std::fs::remove_dir(&a);
    let _ = std::fs::remove_dir(&b);
    swapped
}

/// Route `EEXIST` to [`AtomicCreateError::Collision`] and everything
/// else to the I/O bucket.
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
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
    fn a_directory_is_never_renamed_over_an_existing_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("f"), b"x").unwrap();
        // An empty directory: what a plain rename would replace.
        fs::create_dir(&dst).unwrap();
        assert!(matches!(
            atomic_rename_dir(&src, &dst),
            Err(AtomicCreateError::Collision)
        ));
        assert!(src.join("f").exists());
        assert_eq!(fs::read_dir(&dst).unwrap().count(), 0);

        fs::remove_dir(&dst).unwrap();
        atomic_rename_dir(&src, &dst).unwrap();
        assert!(dst.join("f").exists() && !src.exists());
    }

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

    #[test]
    fn exchange_swaps_two_populated_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        fs::write(a.join("from-a"), b"a").unwrap();
        fs::write(b.join("from-b"), b"b").unwrap();
        exchange_dirs(&a, &b).expect("exchange");
        assert!(a.join("from-b").exists() && !a.join("from-a").exists());
        assert!(b.join("from-a").exists() && !b.join("from-b").exists());
    }

    #[test]
    fn exchange_with_a_missing_side_fails_and_moves_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("a");
        fs::create_dir(&a).unwrap();
        fs::write(a.join("f"), b"x").unwrap();
        assert!(exchange_dirs(&a, &dir.path().join("missing")).is_err());
        assert!(a.join("f").exists());
        assert!(!dir.path().join("missing").exists());
    }

    #[test]
    fn the_probe_answers_true_in_a_private_temp_dir_and_leaves_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(exchange_supported(dir.path()));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn the_probe_refuses_what_is_not_a_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let f = dir.path().join("f");
        fs::write(&f, b"x").unwrap();
        assert!(!exchange_supported(&f));
        assert!(!exchange_supported(&dir.path().join("missing")));
    }

    #[cfg(unix)]
    #[test]
    fn the_probe_refuses_a_directory_another_user_owns() {
        // `/` belongs to root; a test suite doesn't run as root.
        // SAFETY: geteuid can't fail.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        assert!(!exchange_supported(Path::new("/")));
    }
}
