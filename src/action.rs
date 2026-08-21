//! Shared shell command execution with process-group isolation, timeout,
//! and output capture. Used by both gate evaluation and default action
//! execution.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::Duration;

use wait_timeout::ChildExt;

const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Maximum number of bytes retained from each of stdout and stderr (64 KB).
///
/// The reader threads keep draining past this bound and retain only the
/// first `MAX_ACTION_OUTPUT_BYTES`; stopping the read at the bound would
/// reintroduce the pipe-buffer deadlock for anything larger. The bound
/// applies to gate commands and action commands alike.
pub const MAX_ACTION_OUTPUT_BYTES: usize = 64 * 1024;

/// Size of the chunk each reader thread pulls from its pipe.
const READ_CHUNK_BYTES: usize = 8 * 1024;

/// Why a shell command did not succeed.
///
/// `exit_code: -1` used to mean spawn failure, timeout, or wait error, and
/// callers told them apart by searching stderr. This discriminator names the
/// outcome directly; `exit_code` keeps its previous values so evidence
/// written on the success path is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// The command ran to completion and exited non-zero.
    NonzeroExit,
    /// The child process could not be spawned.
    SpawnFailed,
    /// The command did not finish within the timeout and its process group
    /// was killed.
    TimedOut,
    /// Waiting for the child failed, so no exit status was ever obtained.
    WaitFailed,
}

impl FailureKind {
    /// Wire name for this kind, used in gate evidence and event payloads.
    pub fn as_str(self) -> &'static str {
        match self {
            FailureKind::NonzeroExit => "nonzero_exit",
            FailureKind::SpawnFailed => "spawn_failed",
            FailureKind::TimedOut => "timed_out",
            FailureKind::WaitFailed => "wait_failed",
        }
    }
}

/// Output captured from a shell command execution.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    /// `None` when the command exited zero; otherwise names why it failed.
    pub failure_kind: Option<FailureKind>,
    /// True when retention dropped bytes from stdout or stderr.
    pub truncated: bool,
}

/// What one reader thread produced: the retained bytes and whether it saw
/// more than it retained.
struct Capture {
    bytes: Vec<u8>,
    truncated: bool,
}

/// Read `reader` to end on a dedicated thread, retaining the first `limit`
/// bytes.
///
/// The thread keeps reading after `limit` is reached and discards the excess,
/// so the child never blocks writing into a full pipe. It ends when the pipe
/// closes, which happens when the child exits or its process group is killed.
fn spawn_reader<R>(mut reader: R, limit: usize) -> JoinHandle<Capture>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut bytes: Vec<u8> = Vec::new();
        let mut truncated = false;
        let mut chunk = [0u8; READ_CHUNK_BYTES];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    let room = limit.saturating_sub(bytes.len());
                    let keep = room.min(n);
                    if keep > 0 {
                        bytes.extend_from_slice(&chunk[..keep]);
                    }
                    if keep < n {
                        truncated = true;
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        Capture { bytes, truncated }
    })
}

/// Decode captured bytes, dropping a trailing partial UTF-8 sequence.
///
/// Retention cuts at a byte count, which can split a multi-byte character.
/// An incomplete tail is dropped; any other invalid byte is replaced, so a
/// command emitting binary still yields readable output rather than nothing.
fn decode_capture(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        // `error_len() == None` means the input ended mid-character.
        Err(e) if e.error_len().is_none() => {
            String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned()
        }
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Join a reader thread, treating a panicked reader as empty output.
fn join_reader(handle: Option<JoinHandle<Capture>>) -> (String, bool) {
    match handle.and_then(|h| h.join().ok()) {
        Some(capture) => (decode_capture(&capture.bytes), capture.truncated),
        None => (String::new(), false),
    }
}

/// Append `note` to captured stderr without losing what the command wrote.
fn append_note(stderr: String, note: String) -> String {
    if stderr.is_empty() {
        note
    } else if stderr.ends_with('\n') {
        format!("{}{}", stderr, note)
    } else {
        format!("{}\n{}", stderr, note)
    }
}

/// Run a shell command with process-group isolation, timeout, and output capture.
///
/// The command runs via `sh -c` in its own process group. If `timeout_secs` is 0,
/// a default of 30 seconds is used. On timeout the entire process group is killed.
///
/// Both pipes are drained on their own threads for the whole life of the
/// child, so a command emitting more than the kernel pipe buffer never
/// blocks on write. Output retained before a timeout kill is returned with
/// the timeout result rather than discarded.
pub fn run_shell_command(command: &str, working_dir: &Path, timeout_secs: u32) -> CommandOutput {
    let timeout = if timeout_secs == 0 {
        Duration::from_secs(DEFAULT_TIMEOUT_SECS)
    } else {
        Duration::from_secs(u64::from(timeout_secs))
    };

    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .current_dir(working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // SAFETY: setpgid(0, 0) puts the child into its own process group so we
    // can kill the entire group on timeout without affecting the parent.
    unsafe {
        cmd.pre_exec(|| {
            libc::setpgid(0, 0);
            Ok(())
        });
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return CommandOutput {
                exit_code: -1,
                stdout: String::new(),
                stderr: format!("failed to spawn command: {}", e),
                failure_kind: Some(FailureKind::SpawnFailed),
                truncated: false,
            };
        }
    };

    // Start draining before waiting. A command that writes more than the
    // kernel pipe buffer blocks on write until someone reads, so waiting
    // first would deadlock until the timeout fired.
    let stdout_reader = child
        .stdout
        .take()
        .map(|pipe| spawn_reader(pipe, MAX_ACTION_OUTPUT_BYTES));
    let stderr_reader = child
        .stderr
        .take()
        .map(|pipe| spawn_reader(pipe, MAX_ACTION_OUTPUT_BYTES));

    let wait_result = child.wait_timeout(timeout);

    // Kill the group on any non-exit outcome. That closes the pipes, which
    // ends the readers, which lets the joins below return.
    let note = match &wait_result {
        Ok(Some(_)) => None,
        Ok(None) => Some(format!(
            "command timed out after {} seconds",
            timeout.as_secs()
        )),
        Err(e) => Some(format!("error waiting for command: {}", e)),
    };
    if note.is_some() {
        let pid = child.id() as i32;
        // SAFETY: killpg sends SIGKILL to the process group we created.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
        // Reap the child so we don't leave a zombie.
        let _ = child.wait();
    }

    let (stdout, stdout_truncated) = join_reader(stdout_reader);
    let (stderr, stderr_truncated) = join_reader(stderr_reader);
    let truncated = stdout_truncated || stderr_truncated;

    match wait_result {
        Ok(Some(status)) => {
            let exit_code = status.code().unwrap_or(1);
            CommandOutput {
                exit_code,
                stdout,
                stderr,
                failure_kind: (exit_code != 0).then_some(FailureKind::NonzeroExit),
                truncated,
            }
        }
        Ok(None) => CommandOutput {
            exit_code: -1,
            stdout,
            stderr: append_note(stderr, note.unwrap_or_default()),
            failure_kind: Some(FailureKind::TimedOut),
            truncated,
        },
        Err(_) => CommandOutput {
            exit_code: -1,
            stdout,
            stderr: append_note(stderr, note.unwrap_or_default()),
            failure_kind: Some(FailureKind::WaitFailed),
            truncated,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn captures_stdout() {
        let dir = tmp_dir();
        let out = run_shell_command("echo hello", dir.path(), 5);
        assert_eq!(out.exit_code, 0);
        assert_eq!(out.stdout.trim(), "hello");
        assert!(out.stderr.is_empty());
        assert_eq!(out.failure_kind, None);
        assert!(!out.truncated);
    }

    #[test]
    fn captures_stderr() {
        let dir = tmp_dir();
        let out = run_shell_command("echo oops >&2", dir.path(), 5);
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.is_empty());
        assert_eq!(out.stderr.trim(), "oops");
        assert_eq!(out.failure_kind, None);
    }

    #[test]
    fn captures_exit_code() {
        let dir = tmp_dir();
        let out = run_shell_command("exit 42", dir.path(), 5);
        assert_eq!(out.exit_code, 42);
        assert_eq!(out.failure_kind, Some(FailureKind::NonzeroExit));
    }

    #[test]
    fn timeout_returns_negative_exit_code() {
        let dir = tmp_dir();
        let out = run_shell_command("sleep 60", dir.path(), 1);
        assert_eq!(out.exit_code, -1);
        assert!(out.stderr.contains("timed out"));
        assert_eq!(out.failure_kind, Some(FailureKind::TimedOut));
    }

    #[test]
    fn timeout_keeps_output_written_before_the_kill() {
        let dir = tmp_dir();
        let out = run_shell_command("echo partial; echo noticed >&2; sleep 60", dir.path(), 1);
        assert_eq!(out.exit_code, -1);
        assert_eq!(out.failure_kind, Some(FailureKind::TimedOut));
        assert_eq!(out.stdout.trim(), "partial");
        assert!(out.stderr.contains("noticed"));
        assert!(out.stderr.contains("timed out"));
    }

    #[test]
    fn spawn_failure_reports_spawn_failed() {
        let out = run_shell_command("echo hi", Path::new("/nonexistent/dir/xyz_12345"), 5);
        assert_eq!(out.exit_code, -1);
        assert_eq!(out.failure_kind, Some(FailureKind::SpawnFailed));
        assert!(out.stderr.contains("failed to spawn command"));
        assert!(!out.truncated);
    }

    #[test]
    fn runs_in_working_dir() {
        let dir = tmp_dir();
        std::fs::write(dir.path().join("marker.txt"), "found").unwrap();
        let out = run_shell_command("cat marker.txt", dir.path(), 5);
        assert_eq!(out.exit_code, 0);
        assert_eq!(out.stdout.trim(), "found");
    }

    #[test]
    fn default_timeout_used_when_zero() {
        let dir = tmp_dir();
        let out = run_shell_command("exit 0", dir.path(), 0);
        assert_eq!(out.exit_code, 0);
    }

    #[test]
    fn output_above_the_pipe_buffer_does_not_deadlock() {
        let dir = tmp_dir();
        // Well above the ~64 KB kernel pipe buffer but at the retention
        // bound, so nothing is dropped: 1024 lines of 63 chars plus newline.
        let out = run_shell_command(
            "for i in $(seq 1 1024); do printf '%063d\\n' \"$i\"; done",
            dir.path(),
            10,
        );
        assert_eq!(out.exit_code, 0);
        assert_eq!(out.failure_kind, None);
        assert_eq!(out.stdout.len(), MAX_ACTION_OUTPUT_BYTES);
        assert!(!out.truncated);
    }

    #[test]
    fn stdout_above_the_bound_is_truncated_and_flagged() {
        let dir = tmp_dir();
        let out = run_shell_command(
            "for i in $(seq 1 4096); do printf '%063d\\n' \"$i\"; done",
            dir.path(),
            10,
        );
        assert_eq!(out.exit_code, 0);
        assert!(out.truncated);
        assert_eq!(out.stdout.len(), MAX_ACTION_OUTPUT_BYTES);
    }

    #[test]
    fn stderr_above_the_bound_is_truncated_and_flagged() {
        let dir = tmp_dir();
        let out = run_shell_command(
            "for i in $(seq 1 4096); do printf '%063d\\n' \"$i\" >&2; done",
            dir.path(),
            10,
        );
        assert_eq!(out.exit_code, 0);
        assert!(out.truncated);
        assert_eq!(out.stderr.len(), MAX_ACTION_OUTPUT_BYTES);
        assert!(out.stdout.is_empty());
    }

    #[test]
    fn failure_kind_wire_names() {
        assert_eq!(FailureKind::NonzeroExit.as_str(), "nonzero_exit");
        assert_eq!(FailureKind::SpawnFailed.as_str(), "spawn_failed");
        assert_eq!(FailureKind::TimedOut.as_str(), "timed_out");
        assert_eq!(FailureKind::WaitFailed.as_str(), "wait_failed");
    }

    #[test]
    fn decode_capture_drops_a_split_multibyte_tail() {
        // "é" is two bytes; cutting after the first leaves an incomplete tail.
        let bytes = [b'a', 0xC3];
        assert_eq!(decode_capture(&bytes), "a");
    }

    #[test]
    fn decode_capture_replaces_invalid_bytes_mid_stream() {
        let bytes = [b'a', 0xFF, b'b'];
        let decoded = decode_capture(&bytes);
        assert!(decoded.starts_with('a'));
        assert!(decoded.ends_with('b'));
    }
}
