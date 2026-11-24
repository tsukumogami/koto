//! Every file open in the crate.
//!
//! The crate's `clippy.toml` bans opening a file anywhere else, along with
//! every way to write, create, remove, link or lock one. This module holds
//! the only `#[allow]`s for those bans, and offers two reads:
//!
//! - [`open_plain`] is a plain `File::open`, kept for the reads koto's own
//!   commands make of their own session's log (the three log readers in
//!   `engine::persistence`) and for `generate_session_id`'s read of
//!   `/dev/urandom`. Until the template read moves to [`read_store_file`],
//!   `is_terminal_state` and the dashboard's template read use it too.
//!   `scripts/check-reader-readonly.sh` holds the list of allowed callers.
//! - [`read_store_file`] is the hardened read: it never blocks on a FIFO or
//!   device, never reads past a cap, and in [`ReadMode::InStore`] refuses a
//!   final-component symlink and a path that resolves outside the store.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// Open flags for a file inside the store: refuse a symlink as the final
/// component, and don't block opening a FIFO.
#[cfg(unix)]
pub const IN_STORE_OPEN_FLAGS: libc::c_int = libc::O_NOFOLLOW | libc::O_NONBLOCK;

/// Open flags for a compiled template, which `--template` may legitimately
/// name through a symlink outside the store: don't block opening a FIFO.
#[cfg(unix)]
pub const TEMPLATE_OPEN_FLAGS: libc::c_int = libc::O_NONBLOCK;

/// Where a [`read_store_file`] read comes from, which fixes its open flags
/// and whether it checks containment.
#[derive(Debug, Clone, Copy)]
pub enum ReadMode<'a> {
    /// A file in the session store. After the open, the path must still
    /// resolve inside `root`.
    InStore {
        /// The directory the file must stay inside.
        root: &'a Path,
    },
    /// A compiled template, which may live anywhere.
    Template,
}

/// The open flags a mode reads with: one of the two constants above.
#[cfg(unix)]
fn mode_flags(mode: &ReadMode<'_>) -> libc::c_int {
    match mode {
        ReadMode::InStore { .. } => IN_STORE_OPEN_FLAGS,
        ReadMode::Template => TEMPLATE_OPEN_FLAGS,
    }
}

/// Open `path` for reading with no extra flags, exactly as `File::open`.
#[allow(clippy::disallowed_methods)]
pub fn open_plain<P: AsRef<Path>>(path: P) -> io::Result<File> {
    File::open(path)
}

/// Read at most `cap` bytes of the regular file at `path`.
///
/// The open uses the mode's fixed flags, so a FIFO or device opens without
/// blocking and is then refused by the `fstat` check on the open handle. A
/// file longer than `cap` is refused rather than truncated. In
/// [`ReadMode::InStore`] the open refuses a symlink as the final component,
/// and the path is canonicalized after the open and must still sit inside
/// the canonicalized `root`.
///
/// Every refusal is an `io::Error`; a missing file keeps its `NotFound`
/// kind so a caller can treat it as absent.
#[allow(clippy::disallowed_methods)]
pub fn read_store_file(path: &Path, cap: u64, mode: ReadMode<'_>) -> io::Result<Vec<u8>> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(mode_flags(&mode));
    }
    let file = opts.open(path)?;

    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a regular file", path.display()),
        ));
    }
    if meta.len() > cap {
        return Err(over_cap(path, cap));
    }

    if let ReadMode::InStore { root } = mode {
        let resolved = std::fs::canonicalize(path)?;
        let root = std::fs::canonicalize(root)?;
        if !resolved.starts_with(&root) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{} resolves to {}, outside {}",
                    path.display(),
                    resolved.display(),
                    root.display()
                ),
            ));
        }
    }

    let mut buf = Vec::new();
    file.take(cap.saturating_add(1)).read_to_end(&mut buf)?;
    if buf.len() as u64 > cap {
        return Err(over_cap(path, cap));
    }
    Ok(buf)
}

fn over_cap(path: &Path, cap: u64) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{} is larger than {} bytes", path.display(), cap),
    )
}
