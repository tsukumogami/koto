//! One bounded, atomic JSONL append.
//!
//! [`append_bounded_line`] is the append discipline the workspace-wide
//! JSONL files share: the terminal index (`_terminal_index.jsonl`) and the
//! decider ledger (`_decider_ledger.jsonl`).
//!
//! ## Atomicity
//!
//! The file is opened with `OpenOptions::append(true)` (POSIX `O_APPEND`)
//! and the line goes out in one `write_all`, followed by `sync_data`. Under
//! POSIX, writes of at most `PIPE_BUF` bytes (4 KiB on Linux) to a
//! descriptor opened with `O_APPEND` are atomic with respect to each other:
//! the kernel resolves the offset and performs the write as one syscall. A
//! caller passes a `max` within that bound, and a line over it is refused
//! before the file is opened, so concurrent appenders never interleave.
//! Nothing here seeks or writes at an offset; don't add either.
//!
//! The discipline holds on local ext4, xfs, APFS, and NTFS. Network
//! filesystems are out of scope.
//!
//! ## Permissions
//!
//! A file this function creates is mode 0600 on unix. A file that already
//! exists keeps whatever mode it has.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

/// Append `line` plus a trailing newline to `path` as one atomic write.
///
/// `line` must not contain the newline; this function adds it. The whole
/// write (newline included) must fit in `max` bytes, or the call fails
/// before `path` is opened. `dir` is created (with its parents) when
/// absent; it is normally `path`'s parent.
pub fn append_bounded_line(dir: &Path, path: &Path, line: &str, max: usize) -> Result<()> {
    let len = line.len() + 1;
    if len > max {
        anyhow::bail!(
            "line for {} exceeds the PIPE_BUF append bound ({} bytes > {})",
            path.display(),
            len,
            max
        );
    }

    if !dir.exists() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
    }

    let mut buf = String::with_capacity(len);
    buf.push_str(line);
    buf.push('\n');

    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Applies only when the file is created; an existing file's mode is
        // left alone.
        options.mode(0o600);
    }
    // O_APPEND: the kernel picks the offset per write. Never seek() or
    // write_at() on this handle.
    let mut file = options
        .open(path)
        .with_context(|| format!("failed to open {} for append", path.display()))?;
    file.write_all(buf.as_bytes())
        .with_context(|| format!("failed to append to {}", path.display()))?;
    file.sync_data()
        .with_context(|| format!("failed to fsync {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_one_line_with_a_newline_and_creates_the_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a").join("b");
        let path = dir.join("f.jsonl");
        append_bounded_line(&dir, &path, r#"{"x":1}"#, 4096).unwrap();
        append_bounded_line(&dir, &path, r#"{"x":2}"#, 4096).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\"x\":1}\n{\"x\":2}\n"
        );
    }

    #[test]
    fn the_bound_counts_the_newline_and_refuses_before_opening() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("never-made");
        let path = dir.join("f.jsonl");
        // 9 bytes plus the newline is 10: exactly at the bound is fine.
        let tmp2 = tempfile::tempdir().unwrap();
        let ok = tmp2.path().join("ok.jsonl");
        append_bounded_line(tmp2.path(), &ok, "123456789", 10).unwrap();
        // One more byte is refused, and nothing is created.
        let err = append_bounded_line(&dir, &path, "1234567890", 10).unwrap_err();
        assert!(err.to_string().contains("11 bytes > 10"), "{}", err);
        assert!(!dir.exists());
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_created_file_is_0600_and_an_existing_mode_is_kept() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let fresh = tmp.path().join("fresh.jsonl");
        append_bounded_line(tmp.path(), &fresh, "{}", 4096).unwrap();
        let mode = std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        let existing = tmp.path().join("existing.jsonl");
        std::fs::write(&existing, "").unwrap();
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o644)).unwrap();
        append_bounded_line(tmp.path(), &existing, "{}", 4096).unwrap();
        let mode = std::fs::metadata(&existing).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644);
        assert_eq!(std::fs::read_to_string(&existing).unwrap(), "{}\n");
    }

    #[test]
    fn n_threads_produce_n_whole_lines() {
        use std::sync::Arc;
        use std::thread;

        let tmp = tempfile::tempdir().unwrap();
        let dir = Arc::new(tmp.path().to_path_buf());
        let n = 32_usize;
        let handles: Vec<_> = (0..n)
            .map(|i| {
                let dir = Arc::clone(&dir);
                thread::spawn(move || {
                    let line = format!(r#"{{"i":{},"pad":"{}"}}"#, i, "x".repeat(3000));
                    append_bounded_line(&dir, &dir.join("f.jsonl"), &line, 4096).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let body = std::fs::read_to_string(dir.join("f.jsonl")).unwrap();
        let mut seen: Vec<u64> = body
            .lines()
            .map(|l| {
                serde_json::from_str::<serde_json::Value>(l).unwrap()["i"]
                    .as_u64()
                    .unwrap()
            })
            .collect();
        seen.sort();
        assert_eq!(seen, (0..n as u64).collect::<Vec<_>>());
    }
}
