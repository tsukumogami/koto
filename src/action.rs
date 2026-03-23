//! Shared shell command execution with process-group isolation, timeout,
//! and output capture. Used by both gate evaluation and default action
//! execution.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use wait_timeout::ChildExt;

const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Output captured from a shell command execution.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Run a shell command with process-group isolation, timeout, and output capture.
///
/// The command runs via `sh -c` in its own process group. If `timeout_secs` is 0,
/// a default of 30 seconds is used. On timeout the entire process group is killed.
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
            };
        }
    };

    match child.wait_timeout(timeout) {
        Ok(Some(status)) => {
            let stdout = child
                .stdout
                .take()
                .map(|mut s| {
                    let mut buf = String::new();
                    std::io::Read::read_to_string(&mut s, &mut buf).ok();
                    buf
                })
                .unwrap_or_default();
            let stderr = child
                .stderr
                .take()
                .map(|mut s| {
                    let mut buf = String::new();
                    std::io::Read::read_to_string(&mut s, &mut buf).ok();
                    buf
                })
                .unwrap_or_default();
            CommandOutput {
                exit_code: status.code().unwrap_or(1),
                stdout,
                stderr,
            }
        }
        Ok(None) => {
            // Timed out -- kill the entire process group.
            let pid = child.id() as i32;
            // SAFETY: killpg sends SIGKILL to the process group we created.
            unsafe {
                libc::killpg(pid, libc::SIGKILL);
            }
            // Reap the child so we don't leave a zombie.
            let _ = child.wait();
            CommandOutput {
                exit_code: -1,
                stdout: String::new(),
                stderr: format!("command timed out after {} seconds", timeout.as_secs()),
            }
        }
        Err(e) => CommandOutput {
            exit_code: -1,
            stdout: String::new(),
            stderr: format!("error waiting for command: {}", e),
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
    }

    #[test]
    fn captures_stderr() {
        let dir = tmp_dir();
        let out = run_shell_command("echo oops >&2", dir.path(), 5);
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.is_empty());
        assert_eq!(out.stderr.trim(), "oops");
    }

    #[test]
    fn captures_exit_code() {
        let dir = tmp_dir();
        let out = run_shell_command("exit 42", dir.path(), 5);
        assert_eq!(out.exit_code, 42);
    }

    #[test]
    fn timeout_returns_negative_exit_code() {
        let dir = tmp_dir();
        let out = run_shell_command("sleep 60", dir.path(), 1);
        assert_eq!(out.exit_code, -1);
        assert!(out.stderr.contains("timed out"));
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
}
