//! Shared shell command execution with process-group isolation, timeout,
//! and output capture. Used by both gate evaluation and default action
//! execution.

use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Duration;

use wait_timeout::ChildExt;

use crate::redact::{redact_capture, RedactedText, Redactor};

const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Maximum number of bytes retained from each of stdout and stderr (64 KB).
///
/// The reader threads keep draining past this bound; stopping the read at
/// the bound would reintroduce the pipe-buffer deadlock for anything larger.
/// They retain the first `MAX_ACTION_OUTPUT_BYTES` plus, when the tick knows
/// credentials, enough lookahead to see a value that starts before the bound
/// whole (see [`Redactor::retention`]). The bound applies to gate commands
/// and action commands alike.
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

/// The shell every command runs under, by absolute path, so neither the
/// ticking shell's `PATH` nor a decoy `sh` on it can pick a different one.
pub const SHELL: &str = "/bin/sh";

/// How the last run of one gate or action ended, as the tick's notes need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandOutcome {
    Passed,
    /// `not_found` is true when the failure looks like a missing command.
    Failed {
        not_found: bool,
    },
}

/// The environment one tick's commands run with (DESIGN-koto-fixed-environment.md).
///
/// Built once per tick by the engine and passed to every gate and action, so
/// they can't disagree. It's never serialized, and its `Debug` form shows
/// variable names only: values can carry credentials.
///
/// It also keeps how each gate or action last ended on this tick, so the tick
/// can name a failure a stale record or a missing command explains without
/// changing the gate's evidence.
pub struct CommandEnv {
    vars: Vec<(String, String)>,
    inherit: bool,
    outcomes: Mutex<BTreeMap<String, CommandOutcome>>,
    /// The credentials this tick knows, replaced in every command's output.
    redactor: Redactor,
    /// The output each recorded gate or action produced, so a test can see
    /// what a command gate captured: its evidence doesn't carry the streams.
    #[cfg(test)]
    outputs: Mutex<BTreeMap<String, CommandOutput>>,
}

impl CommandEnv {
    /// Exactly `vars`, in order, with nothing inherited from this process,
    /// and `redactor` replacing known credentials in the output of every
    /// command run under it.
    ///
    /// A tick gets its environment from
    /// `crate::engine::command_env::for_tick`, which passes the tick's known
    /// set; the redactor is required here so no environment can be built
    /// without choosing one.
    pub fn cleared(vars: Vec<(String, String)>, redactor: Redactor) -> Self {
        Self {
            vars,
            inherit: false,
            outcomes: Mutex::new(BTreeMap::new()),
            redactor,
            #[cfg(test)]
            outputs: Mutex::new(BTreeMap::new()),
        }
    }

    /// The redactor for output captured under this environment.
    pub fn redactor(&self) -> &Redactor {
        &self.redactor
    }

    /// This process's environment with `vars` applied on top, for a session
    /// created with `--legacy-environment`, redacting output against
    /// `redactor` as [`CommandEnv::cleared`] does.
    pub fn inherited(vars: Vec<(String, String)>, redactor: Redactor) -> Self {
        Self {
            inherit: true,
            ..Self::cleared(vars, redactor)
        }
    }

    /// This process's environment unchanged, with no redaction: for callers
    /// outside a tick, which have no known set. A tick's commands never run
    /// under it; they use the environment
    /// `crate::engine::command_env::for_tick` builds.
    pub fn inherit() -> Self {
        Self::inherited(Vec::new(), Redactor::empty())
    }

    /// True when commands inherit this process's environment.
    pub fn inherits(&self) -> bool {
        self.inherit
    }

    /// The value set for `name`, if this value sets one.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// Record how the gate or action named `label` ended.
    pub fn record(&self, label: &str, output: &CommandOutput) {
        let outcome = if output.failure_kind.is_none() {
            CommandOutcome::Passed
        } else {
            CommandOutcome::Failed {
                not_found: looks_not_found(output),
            }
        };
        if let Ok(mut map) = self.outcomes.lock() {
            map.insert(label.to_string(), outcome);
        }
        #[cfg(test)]
        if let Ok(mut map) = self.outputs.lock() {
            map.insert(label.to_string(), output.clone());
        }
    }

    /// The output last recorded under `label`, for tests.
    #[cfg(test)]
    pub(crate) fn recorded_output(&self, label: &str) -> Option<CommandOutput> {
        self.outputs.lock().ok()?.get(label).cloned()
    }

    /// Every gate or action whose last run on this tick failed, with whether
    /// the failure looks like a missing command.
    pub fn failures(&self) -> Vec<(String, bool)> {
        match self.outcomes.lock() {
            Ok(map) => map
                .iter()
                .filter_map(|(label, outcome)| match outcome {
                    CommandOutcome::Failed { not_found } => Some((label.clone(), *not_found)),
                    CommandOutcome::Passed => None,
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }
}

impl std::fmt::Debug for CommandEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandEnv")
            .field(
                "names",
                &self
                    .vars
                    .iter()
                    .map(|(n, _)| n.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("inherit", &self.inherit)
            .field("redactor", &self.redactor)
            .finish()
    }
}

/// True when a failed command looks like it couldn't find a program: exit
/// status 127, or the shell's own message on stderr (`<name>: not found` from
/// dash, `command not found` from bash). The message matters because a script
/// whose inner command is missing usually exits with its own status. A tool's
/// own "not found" text ("repository not found", "file not found") doesn't
/// match, because the note it would trigger recommends a new session.
pub fn looks_not_found(output: &CommandOutput) -> bool {
    output.failure_kind == Some(FailureKind::NonzeroExit)
        && (output.exit_code == 127
            || output.stderr.contains(": not found")
            || output.stderr.contains("command not found"))
}

/// Output captured from a shell command execution.
///
/// `stdout` and `stderr` have been through the tick's redactor: every known
/// credential in them is a `[REDACTED:<source>]` marker, and code outside
/// the runner can't reach the raw bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: RedactedText,
    pub stderr: RedactedText,
    /// `None` when the command exited zero; otherwise names why it failed.
    pub failure_kind: Option<FailureKind>,
    /// True when stdout was cut at the bound.
    pub stdout_truncated: bool,
    /// True when stderr was cut at the bound.
    pub stderr_truncated: bool,
    /// `stdout_truncated || stderr_truncated`.
    pub truncated: bool,
    /// True when stderr's last line is a note koto wrote (a timeout, spawn,
    /// wait or polling note) rather than something the command printed.
    pub stderr_ends_with_note: bool,
    /// Wall-clock milliseconds from spawning the command to its exit or
    /// kill: the span the timeout covers.
    pub duration_ms: u64,
}

/// What one reader thread produced: the retained bytes and how many the
/// stream carried in all.
struct Capture {
    bytes: Vec<u8>,
    total: usize,
}

/// Read `reader` to end on a dedicated thread, retaining the first `retain`
/// bytes and counting every byte.
///
/// The thread keeps reading after `retain` is reached and discards the
/// excess, so the child never blocks writing into a full pipe. It ends when
/// the pipe closes, which happens when the child exits or its process group
/// is killed.
fn spawn_reader<R>(mut reader: R, retain: usize) -> JoinHandle<Capture>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut bytes: Vec<u8> = Vec::new();
        let mut total: usize = 0;
        let mut chunk = [0u8; READ_CHUNK_BYTES];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    total = total.saturating_add(n);
                    let room = retain.saturating_sub(bytes.len());
                    let keep = room.min(n);
                    if keep > 0 {
                        bytes.extend_from_slice(&chunk[..keep]);
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        Capture { bytes, total }
    })
}

/// Join a reader thread and redact what it kept, treating a panicked reader
/// as empty output. This is the one place raw capture becomes text.
fn join_reader(
    handle: Option<JoinHandle<Capture>>,
    killed: bool,
    redactor: &Redactor,
) -> (RedactedText, bool) {
    match handle.and_then(|h| h.join().ok()) {
        Some(capture) => redact_capture(
            &capture.bytes,
            capture.total,
            killed,
            MAX_ACTION_OUTPUT_BYTES,
            redactor,
        ),
        None => (RedactedText::default(), false),
    }
}

/// Append koto's `note` to captured stderr without losing what the command
/// wrote. The note is appended after redaction, so it is never searched.
fn append_note(mut stderr: RedactedText, note: String) -> RedactedText {
    if stderr.is_empty() {
        RedactedText::koto_note(note)
    } else {
        if !stderr.ends_with('\n') {
            stderr.push_koto_note("\n");
        }
        stderr.push_koto_note(&note);
        stderr
    }
}

/// Milliseconds since `started`, saturating.
pub fn elapsed_ms(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Run a shell command with process-group isolation, timeout, and output capture.
///
/// The command runs via `/bin/sh -c` in its own process group, with the
/// environment `env` describes and standard input at end of file. A missing
/// `/bin/sh` is reported as a spawn failure; there is no fallback to a `PATH`
/// search. If `timeout_secs` is 0,
/// a default of 30 seconds is used. On timeout the entire process group is killed.
///
/// Both pipes are drained on their own threads for the whole life of the
/// child, so a command emitting more than the kernel pipe buffer never
/// blocks on write. Output retained before a timeout kill is returned with
/// the timeout result rather than discarded.
pub fn run_shell_command(
    command: &str,
    working_dir: &Path,
    timeout_secs: u32,
    env: &CommandEnv,
) -> CommandOutput {
    let timeout = if timeout_secs == 0 {
        Duration::from_secs(DEFAULT_TIMEOUT_SECS)
    } else {
        Duration::from_secs(u64::from(timeout_secs))
    };

    let mut cmd = Command::new(SHELL);
    if !env.inherit {
        cmd.env_clear();
    }
    cmd.envs(env.vars.iter().map(|(n, v)| (n, v)))
        .arg("-c")
        .arg(command)
        .current_dir(working_dir)
        .stdin(Stdio::null())
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

    let started = std::time::Instant::now();
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return CommandOutput {
                exit_code: -1,
                stdout: RedactedText::default(),
                stderr: RedactedText::koto_note(format!("failed to spawn command: {}", e)),
                failure_kind: Some(FailureKind::SpawnFailed),
                stdout_truncated: false,
                stderr_truncated: false,
                truncated: false,
                stderr_ends_with_note: true,
                duration_ms: elapsed_ms(started),
            };
        }
    };

    // Start draining before waiting. A command that writes more than the
    // kernel pipe buffer blocks on write until someone reads, so waiting
    // first would deadlock until the timeout fired.
    let retain = env.redactor.retention(MAX_ACTION_OUTPUT_BYTES);
    let stdout_reader = child.stdout.take().map(|pipe| spawn_reader(pipe, retain));
    let stderr_reader = child.stderr.take().map(|pipe| spawn_reader(pipe, retain));

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
    let duration_ms = elapsed_ms(started);

    // Redact once, after both readers finish and before anything decodes,
    // cuts or reads the output. A killed process may have stopped mid-value.
    let killed = note.is_some();
    let (stdout, stdout_truncated) = join_reader(stdout_reader, killed, &env.redactor);
    let (stderr, stderr_truncated) = join_reader(stderr_reader, killed, &env.redactor);
    let truncated = stdout_truncated || stderr_truncated;

    match wait_result {
        Ok(Some(status)) => {
            let exit_code = status.code().unwrap_or(1);
            CommandOutput {
                exit_code,
                stdout,
                stderr,
                failure_kind: (exit_code != 0).then_some(FailureKind::NonzeroExit),
                stdout_truncated,
                stderr_truncated,
                truncated,
                stderr_ends_with_note: false,
                duration_ms,
            }
        }
        Ok(None) => CommandOutput {
            exit_code: -1,
            stdout,
            stderr: append_note(stderr, note.unwrap_or_default()),
            failure_kind: Some(FailureKind::TimedOut),
            stdout_truncated,
            stderr_truncated,
            truncated,
            stderr_ends_with_note: true,
            duration_ms,
        },
        Err(_) => CommandOutput {
            exit_code: -1,
            stdout,
            stderr: append_note(stderr, note.unwrap_or_default()),
            failure_kind: Some(FailureKind::WaitFailed),
            stdout_truncated,
            stderr_truncated,
            truncated,
            stderr_ends_with_note: true,
            duration_ms,
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
        let out = run_shell_command("echo hello", dir.path(), 5, &CommandEnv::inherit());
        assert_eq!(out.exit_code, 0);
        assert_eq!(out.stdout.trim(), "hello");
        assert!(out.stderr.is_empty());
        assert_eq!(out.failure_kind, None);
        assert!(!out.truncated);
    }

    #[test]
    fn captures_stderr() {
        let dir = tmp_dir();
        let out = run_shell_command("echo oops >&2", dir.path(), 5, &CommandEnv::inherit());
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.is_empty());
        assert_eq!(out.stderr.trim(), "oops");
        assert_eq!(out.failure_kind, None);
    }

    #[test]
    fn captures_exit_code() {
        let dir = tmp_dir();
        let out = run_shell_command("exit 42", dir.path(), 5, &CommandEnv::inherit());
        assert_eq!(out.exit_code, 42);
        assert_eq!(out.failure_kind, Some(FailureKind::NonzeroExit));
    }

    #[test]
    fn timeout_returns_negative_exit_code() {
        let dir = tmp_dir();
        let out = run_shell_command("sleep 60", dir.path(), 1, &CommandEnv::inherit());
        assert_eq!(out.exit_code, -1);
        assert!(out.stderr.contains("timed out"));
        assert_eq!(out.failure_kind, Some(FailureKind::TimedOut));
    }

    #[test]
    fn timeout_keeps_output_written_before_the_kill() {
        let dir = tmp_dir();
        let out = run_shell_command(
            "echo partial; echo noticed >&2; sleep 60",
            dir.path(),
            1,
            &CommandEnv::inherit(),
        );
        assert_eq!(out.exit_code, -1);
        assert_eq!(out.failure_kind, Some(FailureKind::TimedOut));
        assert_eq!(out.stdout.trim(), "partial");
        assert!(out.stderr.contains("noticed"));
        assert!(out.stderr.contains("timed out"));
    }

    #[test]
    fn spawn_failure_reports_spawn_failed() {
        let out = run_shell_command(
            "echo hi",
            Path::new("/nonexistent/dir/xyz_12345"),
            5,
            &CommandEnv::inherit(),
        );
        assert_eq!(out.exit_code, -1);
        assert_eq!(out.failure_kind, Some(FailureKind::SpawnFailed));
        assert!(out.stderr.contains("failed to spawn command"));
        assert!(!out.truncated);
    }

    #[test]
    fn runs_in_working_dir() {
        let dir = tmp_dir();
        std::fs::write(dir.path().join("marker.txt"), "found").unwrap();
        let out = run_shell_command("cat marker.txt", dir.path(), 5, &CommandEnv::inherit());
        assert_eq!(out.exit_code, 0);
        assert_eq!(out.stdout.trim(), "found");
    }

    #[test]
    fn default_timeout_used_when_zero() {
        let dir = tmp_dir();
        let out = run_shell_command("exit 0", dir.path(), 0, &CommandEnv::inherit());
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
            &CommandEnv::inherit(),
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
            &CommandEnv::inherit(),
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
            &CommandEnv::inherit(),
        );
        assert_eq!(out.exit_code, 0);
        assert!(out.truncated);
        assert_eq!(out.stderr.len(), MAX_ACTION_OUTPUT_BYTES);
        assert!(out.stdout.is_empty());
    }

    #[test]
    fn a_cleared_environment_holds_only_what_it_sets() {
        let dir = tmp_dir();
        let env = CommandEnv::cleared(
            vec![
                ("PATH".to_string(), "/usr/bin:/bin".to_string()),
                ("KOTO_TEST_SET".to_string(), "yes".to_string()),
            ],
            Redactor::empty(),
        );
        // Names only: the child reports which of these it can see.
        let out = run_shell_command(
            "[ -n \"${PATH+x}\" ] && echo PATH; \
             [ -n \"${KOTO_TEST_SET+x}\" ] && echo KOTO_TEST_SET; \
             [ -n \"${HOME+x}\" ] && echo HOME; \
             [ -n \"${CARGO+x}\" ] && echo CARGO; true",
            dir.path(),
            5,
            &env,
        );
        assert_eq!(out.exit_code, 0, "{}", out.stderr);
        assert_eq!(out.stdout, "PATH\nKOTO_TEST_SET\n");
    }

    #[test]
    fn standard_input_is_at_end_of_file() {
        let dir = tmp_dir();
        let out = run_shell_command(
            "if read -r line; then echo got; else echo eof; fi",
            dir.path(),
            5,
            &CommandEnv::inherit(),
        );
        assert_eq!(out.stdout, "eof\n");
    }

    #[test]
    fn the_shell_is_found_without_a_path() {
        let dir = tmp_dir();
        let out = run_shell_command(
            "echo ran",
            dir.path(),
            5,
            &CommandEnv::cleared(vec![], Redactor::empty()),
        );
        assert_eq!(out.stdout, "ran\n");
    }

    #[test]
    fn not_found_is_exit_127_or_the_shell_message() {
        let dir = tmp_dir();
        let env = CommandEnv::cleared(
            vec![("PATH".to_string(), "/usr/bin:/bin".to_string())],
            Redactor::empty(),
        );
        let missing = run_shell_command("koto_no_such_command_xyz", dir.path(), 5, &env);
        assert!(looks_not_found(&missing), "{:?}", missing.exit_code);
        let wrapped = run_shell_command("koto_no_such_command_xyz; exit 3", dir.path(), 5, &env);
        assert_eq!(wrapped.exit_code, 3);
        assert!(looks_not_found(&wrapped));
        let plain = run_shell_command("exit 1", dir.path(), 5, &env);
        assert!(!looks_not_found(&plain));
        let tool_message = run_shell_command(
            "echo 'fatal: repository not found' >&2; exit 128",
            dir.path(),
            5,
            &env,
        );
        assert!(!looks_not_found(&tool_message));
    }

    #[test]
    fn outcomes_keep_the_last_run_of_each_label() {
        let dir = tmp_dir();
        let env = CommandEnv::cleared(
            vec![("PATH".to_string(), "/usr/bin:/bin".to_string())],
            Redactor::empty(),
        );
        env.record("g", &run_shell_command("exit 1", dir.path(), 5, &env));
        env.record(
            "h",
            &run_shell_command("koto_no_such_command_xyz", dir.path(), 5, &env),
        );
        assert_eq!(
            env.failures(),
            vec![("g".to_string(), false), ("h".to_string(), true)]
        );
        env.record("g", &run_shell_command("true", dir.path(), 5, &env));
        assert_eq!(env.failures(), vec![("h".to_string(), true)]);
    }

    #[test]
    fn debug_shows_names_and_never_values() {
        let env = CommandEnv::cleared(
            vec![("GH_TOKEN".to_string(), "marker-value".to_string())],
            Redactor::empty(),
        );
        let shown = format!("{:?}", env);
        assert!(shown.contains("GH_TOKEN"));
        assert!(!shown.contains("marker-value"));
    }

    #[test]
    fn failure_kind_wire_names() {
        assert_eq!(FailureKind::NonzeroExit.as_str(), "nonzero_exit");
        assert_eq!(FailureKind::SpawnFailed.as_str(), "spawn_failed");
        assert_eq!(FailureKind::TimedOut.as_str(), "timed_out");
        assert_eq!(FailureKind::WaitFailed.as_str(), "wait_failed");
    }
}
