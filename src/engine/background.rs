//! Background actions: run records and the runner a tick starts
//! (DESIGN-host-resource-pools.md, Decision 2).
//!
//! A `default_action` with `background: true` doesn't run inside the tick.
//! The tick makes a run directory under the session's directory, takes its
//! two locks, writes `run.json`, and starts `koto __run-action`, a detached
//! runner that supervises the command and writes `result.json`. Later ticks
//! read the run by probing its locks, and the first one that finds the
//! result delivers it as the action's outcome.
//!
//! ```text
//! <session dir>/runs/<run_id>/
//!   run.json      what the run is: state, redacted command, timeout, pool,
//!                 slot, host, start time, runner pid
//!   runner.lock   held by the runner only, close-on-exec in the command
//!   run.lock      held by every process of the run (inherited)
//!   result.json   exit status, failure kind, duration, truncation
//!   stdout        the command's output, redacted, at most 64 KiB
//!   stderr
//! ```
//!
//! The runner never writes the session log. Only ticks of the session do,
//! so the log keeps its one writer.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::action::{CommandOutput, FailureKind};
use crate::engine::advance::ActionResult;
use crate::engine::types::{
    BackgroundFinished, BackgroundStarted, Event, EventPayload, PoolAcquired, PoolReleased,
};
use crate::gate::{GateOutcome, StructuredGateResult};
use crate::pool::config::{BackgroundInvalid, BackgroundSpec};
use crate::pool::guard::Requester;
use crate::pool::store::{self, DirUnavailable, Slot};
use crate::redact::Redactor;

/// The directory under a session's directory that holds its runs.
pub const RUNS_DIR: &str = "runs";
/// The most `run.json` or `result.json` may be.
pub const MAX_RECORD_BYTES: u64 = 64 * 1024;
/// The most the spec a tick hands the runner may be.
pub const MAX_SPEC_BYTES: u64 = 4 * 1024 * 1024;
/// The most `retry_after_secs` a running condition carries.
pub const MAX_RETRY_AFTER_SECS: u64 = 15;
/// The hidden subcommand that is the runner.
pub const RUNNER_SUBCOMMAND: &str = "__run-action";
/// What a background command gets in place of `KOTO_TICK_SESSION`: the
/// session whose tick started it. `koto next` on that session refuses.
pub const BACKGROUND_OF_ENV: &str = "KOTO_BACKGROUND_OF";

/// Whether `id` is a run id: `^[a-z0-9-]{1,64}$`. Checked before an id,
/// including one read back from the session log, reaches a path.
pub fn is_valid_run_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A fresh run id.
pub fn new_run_id() -> String {
    crate::engine::types::generate_session_id().to_ascii_lowercase()
}

/// This boot's id, where the host has one (Linux). A run started under
/// another boot id is not this host's to act on.
pub fn boot_id() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// `run.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    pub session: String,
    pub run_id: String,
    pub state: String,
    /// The command with known credentials redacted.
    pub command: String,
    pub timeout_secs: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<usize>,
    pub machine: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_id: Option<String>,
    /// Unix seconds the tick started the run.
    pub started_at: u64,
    /// Set once the runner has been spawned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_pid: Option<u32>,
    /// The runner's start time as [`process_start_token`] read it right
    /// after the spawn. koto signals the runner only while the process at
    /// `runner_pid` still has this start time, so a reused pid is never
    /// signalled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_start: Option<u64>,
    /// The template's `timeout_secs` was cut to `[background]
    /// max_timeout_secs`; `timeout_secs` holds the cut value.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub timeout_capped: bool,
}

/// A token for when process `pid` started, which changes if the pid is
/// reused: on Linux the start time in clock ticks since boot from
/// `/proc/<pid>/stat`, on macOS the start time in microseconds. `None` when
/// the process is gone or the platform has no such reading, and koto then
/// signals nothing.
pub fn process_start_token(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // Field 22, counted after the `)` that ends the command name: the
        // state is the first field there, so the start time is the 20th.
        let rest = &stat[stat.rfind(')')? + 1..];
        rest.split_whitespace().nth(19)?.parse().ok()
    }
    #[cfg(target_os = "macos")]
    {
        // SAFETY: an all-zero proc_bsdinfo is a valid out-buffer.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        // SAFETY: proc_pidinfo writes at most `size` bytes into `info`.
        let n = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDTBSDINFO,
                0,
                &mut info as *mut _ as *mut libc::c_void,
                size,
            )
        };
        (n == size).then(|| info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

/// `result.json`, written by the runner when the command has ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunResult {
    pub exit_code: i32,
    /// `None` on exit 0, else [`FailureKind::as_str`].
    #[serde(default)]
    pub failure_kind: Option<String>,
    pub duration_ms: u64,
    #[serde(default)]
    pub stdout_truncated: bool,
    #[serde(default)]
    pub stderr_truncated: bool,
    #[serde(default)]
    pub stderr_ends_with_note: bool,
}

impl RunResult {
    /// The result of `output`, for the runner to write.
    pub fn from_output(output: &CommandOutput) -> Self {
        RunResult {
            exit_code: output.exit_code,
            failure_kind: output.failure_kind.map(|k| k.as_str().to_string()),
            duration_ms: output.duration_ms,
            stdout_truncated: output.stdout_truncated,
            stderr_truncated: output.stderr_truncated,
            stderr_ends_with_note: output.stderr_ends_with_note,
        }
    }

    /// The failure kind, read back. A kind this build doesn't know reads as
    /// a wait failure: something went wrong that koto didn't classify.
    pub fn failure(&self) -> Option<FailureKind> {
        self.failure_kind.as_deref().map(|k| match k {
            "nonzero_exit" => FailureKind::NonzeroExit,
            "timed_out" => FailureKind::TimedOut,
            "spawn_failed" => FailureKind::SpawnFailed,
            _ => FailureKind::WaitFailed,
        })
    }

    /// The `outcome` of `background_finished` and the `reason` of
    /// `pool_released` for this result.
    pub fn outcome(&self) -> &'static str {
        match self.failure() {
            None => "done",
            Some(FailureKind::TimedOut) => "timed_out",
            Some(_) => "failed",
        }
    }
}

/// What the tick hands the runner on its standard input. Never written to
/// disk: it carries the unredacted command and the values redaction
/// searches for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSpec {
    pub run_dir: PathBuf,
    pub session: String,
    pub run_id: String,
    pub command: String,
    pub working_dir: PathBuf,
    pub timeout_secs: u32,
    /// The tick's command environment.
    pub vars: Vec<(String, String)>,
    pub inherit: bool,
    /// The tick's known credentials, as [`Redactor::entries`].
    pub redact: Vec<(String, String)>,
    /// The descriptor numbers the runner inherits.
    pub runner_lock_fd: RawFd,
    pub run_lock_fd: RawFd,
    /// The pool slot's descriptor and file, when the run holds a slot.
    #[serde(default)]
    pub slot: Option<(RawFd, PathBuf)>,
}

/// `<session dir>/runs/`, created private when missing.
pub fn runs_dir(session_dir: &Path) -> Result<PathBuf, DirUnavailable> {
    let dir = session_dir.join(RUNS_DIR);
    store::ensure_private_dir(&dir)?;
    Ok(dir)
}

/// Open a new file in a run directory, refusing one that exists.
fn create_new(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

/// Replace `name` in `dir` with `bytes`, by a temporary file and a rename.
pub fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&tmp, dir.join(name))
}

/// Read a JSON record of at most [`MAX_RECORD_BYTES`].
fn read_record<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .ok()?;
    let mut text = String::new();
    file.take(MAX_RECORD_BYTES + 1)
        .read_to_string(&mut text)
        .ok()?;
    if text.len() as u64 > MAX_RECORD_BYTES {
        return None;
    }
    serde_json::from_str(&text).ok()
}

/// Read a run's `run.json`.
pub fn read_run_record(run_dir: &Path) -> Option<RunRecord> {
    read_record(&run_dir.join("run.json"))
}

/// Read a run's `result.json`.
pub fn read_run_result(run_dir: &Path) -> Option<RunResult> {
    read_record(&run_dir.join("result.json"))
}

/// Let processes spawned from now on inherit `fd`.
fn clear_cloexec(fd: RawFd) -> std::io::Result<()> {
    // SAFETY: fcntl on a descriptor the caller owns.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: as above.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// What the tick needs to start a run.
pub struct StartRequest<'a> {
    pub session_dir: &'a Path,
    pub session: &'a str,
    pub state: &'a str,
    /// A fresh id from [`new_run_id`].
    pub run_id: &'a str,
    /// The substituted command, unredacted.
    pub command: &'a str,
    pub working_dir: &'a Path,
    pub timeout_secs: u32,
    /// `timeout_secs` is the template's, cut to `max_timeout_secs`.
    pub timeout_capped: bool,
    pub env: &'a crate::action::CommandEnv,
    /// The pool and the slot taken in it, when the action is pooled.
    pub slot: Option<(&'a str, Slot)>,
    /// The koto root, to find the slot file again.
    pub koto_root: Option<&'a Path>,
}

/// Start a background run: make its directory and locks, write `run.json`,
/// and spawn the runner, handing it the slot, `runner.lock` and `run.lock`.
///
/// Returns the record once the runner is spawned. The tick's own copies of
/// the descriptors close when this returns, so from then on the run's
/// processes alone hold the slot and the locks.
pub fn start(req: StartRequest<'_>) -> Result<RunRecord, String> {
    let runs = runs_dir(req.session_dir).map_err(|e| e.to_string())?;
    if !is_valid_run_id(req.run_id) {
        return Err(format!("'{}' is not a run id", req.run_id));
    }
    let run_id = req.run_id.to_string();
    let run_dir = runs.join(&run_id);
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&run_dir)
        .map_err(|e| format!("can't create {}: {e}", run_dir.display()))?;

    let lock = |name: &str| -> Result<File, String> {
        let path = run_dir.join(name);
        let file =
            create_new(&path).map_err(|e| format!("can't create {}: {e}", path.display()))?;
        if !store::try_flock(&file) {
            return Err(format!("can't lock {}", path.display()));
        }
        Ok(file)
    };
    let runner_lock = lock("runner.lock")?;
    let run_lock = lock("run.lock")?;

    let (pool, slot) = match req.slot {
        Some((pool, slot)) => (Some(pool.to_string()), Some(slot)),
        None => (None, None),
    };
    let mut record = RunRecord {
        session: req.session.to_string(),
        run_id: run_id.clone(),
        state: req.state.to_string(),
        command: crate::redact::redact_str(req.command, req.env.redactor()).into_string(),
        timeout_secs: req.timeout_secs,
        pool: pool.clone(),
        slot: slot.as_ref().map(|s| s.index),
        machine: crate::session::version::machine_name(),
        boot_id: boot_id(),
        started_at: store::now_secs(),
        runner_pid: None,
        runner_start: None,
        timeout_capped: req.timeout_capped,
    };
    let to_json = |r: &RunRecord| serde_json::to_vec(r).map_err(|e| e.to_string());
    write_atomic(&run_dir, "run.json", &to_json(&record)?)
        .map_err(|e| format!("can't write run.json: {e}"))?;

    let slot_file = match (&pool, &slot, req.koto_root) {
        (Some(pool), Some(slot), Some(root)) => Some((
            slot.raw_fd(),
            root.join("pools")
                .join(pool)
                .join(format!("slot-{}.lock", slot.index)),
        )),
        _ => None,
    };
    let spec = RunSpec {
        run_dir: run_dir.clone(),
        session: req.session.to_string(),
        run_id: run_id.clone(),
        command: req.command.to_string(),
        working_dir: req.working_dir.to_path_buf(),
        timeout_secs: req.timeout_secs,
        vars: req.env.vars().to_vec(),
        inherit: req.env.inherits(),
        redact: req.env.redactor().entries(),
        runner_lock_fd: runner_lock.as_raw_fd(),
        run_lock_fd: run_lock.as_raw_fd(),
        slot: slot_file,
    };

    // The three descriptors are made inheritable just before the spawn and
    // close with this function, so no other process koto starts gets them.
    for fd in [runner_lock.as_raw_fd(), run_lock.as_raw_fd()]
        .into_iter()
        .chain(slot.as_ref().map(|s| s.raw_fd()))
    {
        clear_cloexec(fd).map_err(|e| format!("can't pass a descriptor to the runner: {e}"))?;
    }

    let exe = std::env::current_exe().map_err(|e| format!("can't find the koto binary: {e}"))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg(RUNNER_SUBCOMMAND)
        .arg(req.session)
        .arg(&run_id)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // The runner starts its own session, so it outlives the tick and no
    // signal sent to the tick's terminal or process group reaches it.
    // SAFETY: setsid is async-signal-safe and has no preconditions.
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("can't start the runner: {e}"))?;
    record.runner_pid = Some(child.id());
    // Read while the runner is this process's unreaped child, so the pid
    // can't have been reused yet.
    record.runner_start = process_start_token(child.id());

    let spec_bytes = serde_json::to_vec(&spec).map_err(|e| e.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        // A runner that died before reading leaves a run that reads as lost.
        let _ = stdin.write_all(&spec_bytes);
    }
    // The pid is recorded for `koto cancel`; a failed rewrite leaves the
    // record without it, which is still a run.
    let _ = write_atomic(&run_dir, "run.json", &to_json(&record)?);
    // The child is never waited for: it is detached and outlives the tick.
    drop(child);
    drop(runner_lock);
    drop(run_lock);
    drop(slot);
    Ok(record)
}

/// What a run reads as, from its locks and files ("Reading a run").
#[derive(Debug)]
pub enum RunState {
    /// The runner holds `runner.lock`.
    Running(RunRecord),
    /// The runner wrote its result and exited. `run_lock_held` is true while
    /// a process of the run survives and keeps the slot.
    Finished {
        record: RunRecord,
        result: RunResult,
        run_lock_held: bool,
    },
    /// The runner died without a result while processes of the run live on.
    Orphaned(RunRecord),
    /// Nothing of the run is left to read: another machine's run, a pruned
    /// one, or a runner that died before writing anything.
    Lost,
}

/// Read run `run_id` of the session whose directory is `session_dir`.
pub fn read_run(session_dir: &Path, run_id: &str) -> RunState {
    if !is_valid_run_id(run_id) {
        return RunState::Lost;
    }
    let run_dir = session_dir.join(RUNS_DIR).join(run_id);
    match std::fs::symlink_metadata(&run_dir) {
        Ok(meta) if meta.file_type().is_dir() => {}
        _ => return RunState::Lost,
    }
    let Some(record) = read_run_record(&run_dir) else {
        return RunState::Lost;
    };
    if store::probe_held(&run_dir.join("runner.lock")) {
        return RunState::Running(record);
    }
    let run_lock_held = store::probe_held(&run_dir.join("run.lock"));
    match read_run_result(&run_dir) {
        Some(result) => RunState::Finished {
            record,
            result,
            run_lock_held,
        },
        None if run_lock_held => RunState::Orphaned(record),
        None => RunState::Lost,
    }
}

/// The finished run's output, as the command runner would have returned
/// it. The streams were redacted by the runner; they go through the tick's
/// redactor again on the way in, so nothing reaches the response without
/// it.
pub fn load_output(
    session_dir: &Path,
    run_id: &str,
    result: &RunResult,
    redactor: &Redactor,
) -> CommandOutput {
    let run_dir = session_dir.join(RUNS_DIR).join(run_id);
    let stream = |name: &str| -> String {
        let Ok(file) = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(run_dir.join(name))
        else {
            return String::new();
        };
        let mut bytes = Vec::new();
        let _ = file
            .take(crate::action::MAX_ACTION_OUTPUT_BYTES as u64 * 2)
            .read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    };
    CommandOutput {
        exit_code: result.exit_code,
        stdout: crate::redact::redact_str(&stream("stdout"), redactor),
        stderr: crate::redact::redact_str(&stream("stderr"), redactor),
        failure_kind: result.failure(),
        stdout_truncated: result.stdout_truncated,
        stderr_truncated: result.stderr_truncated,
        truncated: result.stdout_truncated || result.stderr_truncated,
        stderr_ends_with_note: result.stderr_ends_with_note,
        duration_ms: result.duration_ms,
    }
}

/// The temporal `__action__` condition a tick stops on while a run is in
/// flight: `status` is `running` or `orphaned`.
pub fn in_flight_condition(
    state: &str,
    status: &str,
    run_id: &str,
    record: Option<&RunRecord>,
    timeout_secs: u32,
) -> BTreeMap<String, StructuredGateResult> {
    let now = store::now_secs();
    let elapsed = record.map_or(0, |r| now.saturating_sub(r.started_at));
    let timeout = record.map_or(timeout_secs, |r| r.timeout_secs);
    let mut output = serde_json::json!({
        "status": status,
        "state": state,
        "run_id": run_id,
        "elapsed_secs": elapsed,
        "timeout_secs": timeout,
    });
    let retry_after = match status {
        // Tick again around when the run could have ended, but at least
        // every 15 seconds.
        "running" => (u64::from(timeout).saturating_sub(elapsed)).clamp(1, MAX_RETRY_AFTER_SECS),
        _ => MAX_RETRY_AFTER_SECS,
    };
    output["retry_after_secs"] = serde_json::json!(retry_after);
    if let Some(record) = record {
        if let Some(pool) = &record.pool {
            output["pool"] = serde_json::json!(pool);
        }
        if record.timeout_capped {
            output["timeout_capped"] = serde_json::json!(true);
        }
        if status == "orphaned" {
            if let Some(pid) = record.runner_pid {
                output["runner_pid"] = serde_json::json!(pid);
            }
        }
    }
    let mut map = BTreeMap::new();
    map.insert(
        crate::template::types::ACTION_CONDITION_NAME.to_string(),
        StructuredGateResult {
            outcome: GateOutcome::Pending,
            output,
            ..Default::default()
        },
    );
    map
}

/// What a tick reaching a background action does next.
pub enum TickStep {
    /// Stop with this result: the run is in flight, the pool queued the
    /// session, or the run was lost.
    Stop(ActionResult),
    /// The run ended (or its runner couldn't be started): treat `output` as
    /// the command's output, after appending `events`.
    Finished {
        output: CommandOutput,
        events: Vec<EventPayload>,
    },
}

/// What [`tick`] needs.
pub struct TickRequest<'a> {
    /// The session log as it stands now, this tick's appends included.
    pub events: &'a [Event],
    pub session_dir: &'a Path,
    pub session: &'a str,
    pub state: &'a str,
    /// The substituted command, unredacted.
    pub command: &'a str,
    pub working_dir: &'a Path,
    pub timeout_secs: u32,
    pub env: &'a crate::action::CommandEnv,
    /// The action's pool, and who is asking for its slot.
    pub pool: Option<(&'a str, Requester)>,
    /// The user config's `[background]` table, and the file it was read
    /// from, to name in a refusal.
    pub settings: Result<BackgroundSpec, BackgroundInvalid>,
    pub config_path: PathBuf,
}

/// `step` with `events` recorded before whatever it records itself.
fn with_events(events: Vec<EventPayload>, step: TickStep) -> TickStep {
    if events.is_empty() {
        return step;
    }
    match step {
        TickStep::Stop(ActionResult::WithEvents {
            events: inner,
            result,
        }) => TickStep::Stop(ActionResult::WithEvents {
            events: events.into_iter().chain(inner).collect(),
            result,
        }),
        TickStep::Stop(result) => TickStep::Stop(ActionResult::WithEvents {
            events,
            result: Box::new(result),
        }),
        TickStep::Finished {
            output,
            events: inner,
        } => TickStep::Finished {
            output,
            events: events.into_iter().chain(inner).collect(),
        },
    }
}

/// Whether a run recorded by `started` was started on this machine and
/// boot. Only such a run is read, signalled or released here (PRD R24).
fn is_local(started: &BackgroundStarted) -> bool {
    started.machine == crate::session::version::machine_name()
        && !matches!((&started.boot_id, boot_id()), (Some(a), Some(b)) if *a != b)
}

/// How long koto waits for a runner it signalled to be gone (PRD R23).
pub const STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Send SIGTERM to the live runner of run `run_id` and wait up to
/// [`STOP_WAIT`] for it, and then for the run's other processes, to be
/// gone. Returns whether the runner was signalled and has exited.
///
/// koto signals only the runner, and only after checking that the process
/// at the recorded pid has the recorded start time; the caller has already
/// checked that the run is this machine's and reads as running. Survivors
/// of an orphaned run are never signalled: they hold no `runner.lock`, so
/// such a run never reaches here.
pub fn stop_runner(session_dir: &Path, run_id: &str, record: &RunRecord) -> bool {
    let (Some(pid), Some(start)) = (record.runner_pid, record.runner_start) else {
        return false;
    };
    if pid == 0 || pid > i32::MAX as u32 || process_start_token(pid) != Some(start) {
        return false;
    }
    let run_dir = session_dir.join(RUNS_DIR).join(run_id);
    if !store::probe_held(&run_dir.join("runner.lock")) {
        return false;
    }
    // SAFETY: SIGTERM to the verified runner pid.
    if unsafe { libc::kill(pid as i32, libc::SIGTERM) } != 0 {
        return false;
    }
    let deadline = std::time::Instant::now() + STOP_WAIT;
    let mut runner_gone = false;
    while std::time::Instant::now() < deadline {
        runner_gone = runner_gone || !store::probe_held(&run_dir.join("runner.lock"));
        if runner_gone && !store::probe_held(&run_dir.join("run.lock")) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    runner_gone
}

/// What [`settle_unfinished`] did to a session's runs that no tick has
/// delivered.
#[derive(Default)]
pub struct Settled {
    /// `background_finished` and `pool_released` for every run it settled.
    pub events: Vec<EventPayload>,
    /// A run it could not settle: orphaned (its survivors are never
    /// signalled) or a runner that didn't stop. `(status, run_id, record)`.
    pub in_flight: Option<(&'static str, String, RunRecord)>,
}

/// Settle every run of the session that `background_started` recorded and
/// no `background_finished` has: stop a live runner (recorded as
/// `cancelled`), and record a run that ended or was lost as such. `only`
/// limits it to runs of the states it accepts. Used by `koto cancel`,
/// session removal, leaving a state, and a tick about to start a new run,
/// so an earlier run never keeps going beside the next one (PRD R22, R23).
pub fn settle_unfinished(
    events: &[Event],
    session_dir: &Path,
    only: &dyn Fn(&str) -> bool,
) -> Settled {
    let mut settled = Settled::default();
    let finished: std::collections::BTreeSet<&str> = events
        .iter()
        .filter_map(|e| match &e.payload {
            EventPayload::BackgroundFinished(f) => Some(f.run_id.as_str()),
            _ => None,
        })
        .collect();
    for started in events.iter().filter_map(|e| match &e.payload {
        EventPayload::BackgroundStarted(b) if !finished.contains(b.run_id.as_str()) => Some(b),
        _ => None,
    }) {
        if !only(&started.state) {
            continue;
        }
        let mut record = |outcome: &str, release: bool| {
            settled
                .events
                .push(EventPayload::BackgroundFinished(BackgroundFinished {
                    state: started.state.clone(),
                    run_id: started.run_id.clone(),
                    outcome: outcome.to_string(),
                }));
            if let (Some(pool), true) = (&started.pool, release) {
                settled
                    .events
                    .push(EventPayload::PoolReleased(PoolReleased {
                        state: started.state.clone(),
                        pool: pool.clone(),
                        reason: outcome.to_string(),
                        run_id: Some(started.run_id.clone()),
                    }));
            }
        };
        let state = if is_local(started) {
            read_run(session_dir, &started.run_id)
        } else {
            RunState::Lost
        };
        match state {
            RunState::Running(rec) => {
                if stop_runner(session_dir, &started.run_id, &rec) {
                    // A process that outlived the runner's kill still holds
                    // the slot: the release waits for `survivor_releases`.
                    let run_lock = session_dir
                        .join(RUNS_DIR)
                        .join(&started.run_id)
                        .join("run.lock");
                    record("cancelled", !store::probe_held(&run_lock));
                } else if settled.in_flight.is_none() {
                    settled.in_flight = Some(("running", started.run_id.clone(), rec));
                }
            }
            RunState::Orphaned(rec) => {
                if settled.in_flight.is_none() {
                    settled.in_flight = Some(("orphaned", started.run_id.clone(), rec));
                }
            }
            RunState::Finished {
                result,
                run_lock_held,
                ..
            } => record(result.outcome(), !run_lock_held),
            RunState::Lost => record("lost", true),
        }
    }
    settled
}

/// `pool_released` for every run whose result was delivered while
/// processes of it survived and kept the slot, once they have all exited:
/// `run.lock` is free or the run directory is gone. Only this machine's
/// runs are looked at.
pub fn survivor_releases(events: &[Event], session_dir: &Path) -> Vec<EventPayload> {
    let mut out = Vec::new();
    for started in events.iter().filter_map(|e| match &e.payload {
        EventPayload::BackgroundStarted(b) if b.pool.is_some() => Some(b),
        _ => None,
    }) {
        let Some(outcome) = events.iter().find_map(|e| match &e.payload {
            EventPayload::BackgroundFinished(f) if f.run_id == started.run_id => {
                Some(f.outcome.clone())
            }
            _ => None,
        }) else {
            continue;
        };
        let released = events.iter().any(|e| {
            matches!(&e.payload, EventPayload::PoolReleased(r)
                if r.run_id.as_deref() == Some(started.run_id.as_str()))
        });
        if released || !is_local(started) || !is_valid_run_id(&started.run_id) {
            continue;
        }
        let run_lock = session_dir
            .join(RUNS_DIR)
            .join(&started.run_id)
            .join("run.lock");
        if !store::probe_held(&run_lock) {
            out.push(EventPayload::PoolReleased(PoolReleased {
                state: started.state.clone(),
                pool: started.pool.clone().unwrap_or_default(),
                reason: outcome,
                run_id: Some(started.run_id.clone()),
            }));
        }
    }
    out
}

/// The run of this session whose `runner.lock` or `run.lock` is held, if
/// any: a run in flight, which a move must not leave behind (PRD R24).
pub fn run_in_flight(session_dir: &Path) -> Option<String> {
    let entries = std::fs::read_dir(session_dir.join(RUNS_DIR)).ok()?;
    entries.flatten().find_map(|entry| {
        let name = entry.file_name().to_str()?.to_string();
        if !is_valid_run_id(&name) {
            return None;
        }
        let dir = entry.path();
        (store::probe_held(&dir.join("runner.lock")) || store::probe_held(&dir.join("run.lock")))
            .then_some(name)
    })
}

/// The message of a move refused while run `run_id` is in flight.
pub fn in_flight_refusal(session: &str, run_id: &str) -> String {
    format!(
        "background_run_in_flight: session '{session}' has background run '{run_id}' in flight \
         on this machine; wait for it to end (or `koto cancel {session}`) before moving the \
         session"
    )
}

/// One tick's handling of a background action ("Tick flow for a pooled
/// action").
///
/// The run this visit to the state started, if any, is the latest
/// `background_started` since the state was entered. While it is in flight
/// the tick reports it and touches no pool, so no second copy starts. When
/// it has finished, its result is the action's outcome, delivered once:
/// `background_finished` marks it read, and the visit's next tick starts a
/// new run. With no run to read, the tick takes a slot (or is queued, as
/// an ordinary pooled action is) and starts one.
pub fn tick(req: TickRequest<'_>) -> TickStep {
    let visit = crate::engine::persistence::epoch_slice(req.events, req.state);
    let latest = visit.iter().rev().find_map(|e| match &e.payload {
        EventPayload::BackgroundStarted(b) if b.state == req.state => Some(b),
        _ => None,
    });
    // Releases owed by earlier runs whose surviving processes have since
    // exited.
    let owed = survivor_releases(req.events, req.session_dir);
    if let Some(started) = latest {
        let read = visit.iter().any(|e| {
            matches!(&e.payload, EventPayload::BackgroundFinished(f) if f.run_id == started.run_id)
        });
        if !read {
            return with_events(owed, read_started(&req, started));
        }
    }
    // A new run is about to start. Any earlier run nobody delivered, a
    // previous visit's still running say, is settled first: a live runner
    // is stopped, so the session never runs two copies (PRD R22).
    let settled = settle_unfinished(req.events, req.session_dir, &|_| true);
    let mut events = owed;
    events.extend(settled.events);
    if let Some((status, run_id, record)) = settled.in_flight {
        return with_events(
            events,
            TickStep::Stop(ActionResult::PoolBlocked(in_flight_condition(
                &record.state,
                status,
                &run_id,
                Some(&record),
                req.timeout_secs,
            ))),
        );
    }
    with_events(events, start_run(&req))
}

/// Read the visit's run `started`, which no tick has delivered yet.
fn read_started(req: &TickRequest<'_>, started: &BackgroundStarted) -> TickStep {
    let finished = |outcome: &str| {
        EventPayload::BackgroundFinished(BackgroundFinished {
            state: req.state.to_string(),
            run_id: started.run_id.clone(),
            outcome: outcome.to_string(),
        })
    };
    let released = |pool: &str, reason: &str| {
        EventPayload::PoolReleased(PoolReleased {
            state: req.state.to_string(),
            pool: pool.to_string(),
            reason: reason.to_string(),
            run_id: Some(started.run_id.clone()),
        })
    };
    // A run started on another machine, or before this host's last boot,
    // isn't this host's to read: whatever its directory holds, it is lost.
    let state = if is_local(started) {
        read_run(req.session_dir, &started.run_id)
    } else {
        RunState::Lost
    };
    match state {
        RunState::Running(record) => {
            TickStep::Stop(ActionResult::PoolBlocked(in_flight_condition(
                req.state,
                "running",
                &started.run_id,
                Some(&record),
                req.timeout_secs,
            )))
        }
        RunState::Orphaned(record) => {
            TickStep::Stop(ActionResult::PoolBlocked(in_flight_condition(
                req.state,
                "orphaned",
                &started.run_id,
                Some(&record),
                req.timeout_secs,
            )))
        }
        RunState::Finished {
            record,
            result,
            run_lock_held,
        } => {
            let mut events = vec![finished(result.outcome())];
            // While a process of the run survives, the slot is still held,
            // so no release is recorded here: a later tick records it once
            // they have exited (`survivor_releases`).
            if let (Some(pool), false) = (&record.pool, run_lock_held) {
                events.push(released(pool, result.outcome()));
            }
            TickStep::Finished {
                output: load_output(
                    req.session_dir,
                    &started.run_id,
                    &result,
                    req.env.redactor(),
                ),
                events,
            }
        }
        RunState::Lost => {
            let mut events = vec![finished("lost")];
            if let Some(pool) = &started.pool {
                events.push(released(pool, "lost"));
            }
            // The runner is gone with no result, and nothing of the run
            // holds the slot: the action failed as `lost`, with no exit
            // status, and the visit's next tick starts a new run (PRD R21,
            // R22).
            TickStep::Finished {
                output: CommandOutput {
                    exit_code: -1,
                    stdout: Default::default(),
                    stderr: crate::redact::RedactedText::koto_note(format!(
                        "background run {} was lost: its runner is gone and left no result",
                        started.run_id
                    )),
                    failure_kind: Some(FailureKind::Lost),
                    stdout_truncated: false,
                    stderr_truncated: false,
                    truncated: false,
                    stderr_ends_with_note: true,
                    duration_ms: 0,
                },
                events,
            }
        }
    }
}

/// The `__action__` condition a background action stops on when the
/// operator's `[background]` settings refuse it: an error naming its code,
/// so the action runs nothing until the setting changes.
fn settings_condition(output: serde_json::Value) -> BTreeMap<String, StructuredGateResult> {
    let mut map = BTreeMap::new();
    map.insert(
        crate::template::types::ACTION_CONDITION_NAME.to_string(),
        StructuredGateResult {
            outcome: GateOutcome::Error,
            output,
            ..Default::default()
        },
    );
    map
}

/// Take the action's slot, if it is pooled, and start a run.
fn start_run(req: &TickRequest<'_>) -> TickStep {
    // The operator's switch and cap come before the pool, so a machine
    // that has background actions off never queues for one.
    let spec = match &req.settings {
        Ok(spec) => *spec,
        Err(invalid) => {
            return TickStep::Stop(ActionResult::PoolBlocked(settings_condition(
                serde_json::json!({
                    "status": "invalid",
                    "error_code": "pool_config_invalid",
                    "state": req.state,
                    "config_file": req.config_path.display().to_string(),
                    "key": invalid.key,
                    "reason": invalid.reason,
                }),
            )))
        }
    };
    if !spec.enabled {
        return TickStep::Stop(ActionResult::PoolBlocked(settings_condition(
            serde_json::json!({
                "status": "disabled",
                "error_code": "background_disabled",
                "state": req.state,
                "config_file": req.config_path.display().to_string(),
                "key": "background.enabled",
                "reason": "background actions are switched off on this machine",
            }),
        )));
    }
    let timeout_capped = req.timeout_secs > spec.max_timeout_secs;
    let timeout_secs = req.timeout_secs.min(spec.max_timeout_secs);

    let mut slot = None;
    // What the guard did to the session's place in the queue, recorded
    // before anything about the slot or the run.
    let mut events = Vec::new();
    if let Some((pool, requester)) = &req.pool {
        let guarded = crate::pool::guard::acquire(pool, requester);
        events = guarded.queue_events(pool, requester);
        if let Some(conditions) = guarded.outcome.blocking_condition(pool) {
            let blocked = ActionResult::PoolBlocked(conditions);
            if events.is_empty() {
                return TickStep::Stop(blocked);
            }
            return TickStep::Stop(ActionResult::WithEvents {
                events,
                result: Box::new(blocked),
            });
        }
        if let crate::pool::guard::GuardOutcome::Acquired(s) = guarded.outcome {
            slot = Some((*pool, s, requester.koto_root.clone()));
        }
    }
    let run_id = new_run_id();
    let slot_index = slot.as_ref().map(|(_, s, _)| s.index);
    let pool = slot.as_ref().map(|(p, _, _)| p.to_string());
    let koto_root = slot.as_ref().map(|(_, _, root)| root.clone());
    let started = start(StartRequest {
        session_dir: req.session_dir,
        session: req.session,
        state: req.state,
        run_id: &run_id,
        command: req.command,
        working_dir: req.working_dir,
        timeout_secs,
        timeout_capped,
        env: req.env,
        slot: slot.map(|(p, s, _)| (p, s)),
        koto_root: koto_root.as_deref(),
    });
    let acquired = |pool: &str, index: usize| {
        EventPayload::PoolAcquired(PoolAcquired {
            state: req.state.to_string(),
            pool: pool.to_string(),
            slot: index,
            run_id: Some(run_id.clone()),
        })
    };
    match started {
        Ok(record) => {
            if let (Some(pool), Some(index)) = (&pool, slot_index) {
                events.push(acquired(pool, index));
            }
            events.push(EventPayload::BackgroundStarted(BackgroundStarted {
                state: req.state.to_string(),
                run_id: run_id.clone(),
                machine: record.machine.clone(),
                boot_id: record.boot_id.clone(),
                pool: record.pool.clone(),
            }));
            TickStep::Stop(ActionResult::WithEvents {
                events,
                result: Box::new(ActionResult::PoolBlocked(in_flight_condition(
                    req.state,
                    "running",
                    &run_id,
                    Some(&record),
                    timeout_secs,
                ))),
            })
        }
        // Nothing was started, so this is the action's spawn failure, and
        // the slot went back when `start` dropped it. Taking it left the
        // queue, so the log says so: a later tick must not read the
        // session as still waiting.
        Err(message) => {
            if let (Some(pool), Some(index)) = (&pool, slot_index) {
                events.push(acquired(pool, index));
                events.push(EventPayload::PoolReleased(PoolReleased {
                    state: req.state.to_string(),
                    pool: pool.clone(),
                    reason: "failed".to_string(),
                    run_id: Some(run_id.clone()),
                }));
            }
            TickStep::Finished {
                output: CommandOutput {
                    exit_code: -1,
                    stdout: Default::default(),
                    stderr: crate::redact::RedactedText::koto_note(format!(
                        "failed to start the background runner: {message}"
                    )),
                    failure_kind: Some(FailureKind::SpawnFailed),
                    stdout_truncated: false,
                    stderr_truncated: false,
                    truncated: false,
                    stderr_ends_with_note: true,
                    duration_ms: 0,
                },
                events,
            }
        }
    }
}

/// Whether the descriptor `fd` is open on the file at `path` (same device
/// and inode), so a runner can tell it got the descriptors it expects.
pub fn fd_is_file(fd: RawFd, path: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    // SAFETY: an all-zero stat is a valid out-buffer for fstat.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat writes into `st`; a closed fd returns an error.
    if unsafe { libc::fstat(fd, &mut st) } != 0 {
        return false;
    }
    st.st_dev as u64 == meta.dev() && st.st_ino as u64 == meta.ino()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_ids_follow_the_grammar() {
        let id = new_run_id();
        assert!(is_valid_run_id(&id), "{id}");
        assert!(!is_valid_run_id(""));
        assert!(!is_valid_run_id("../x"));
        assert!(!is_valid_run_id("A"));
        assert!(!is_valid_run_id(&"a".repeat(65)));
    }

    #[test]
    fn a_missing_run_reads_as_lost() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(read_run(dir.path(), "abc"), RunState::Lost));
        assert!(matches!(read_run(dir.path(), "../etc"), RunState::Lost));
    }

    #[test]
    fn a_run_reads_by_its_locks_and_result() {
        let dir = tempfile::tempdir().unwrap();
        let runs = runs_dir(dir.path()).unwrap();
        let run_dir = runs.join("r1");
        std::fs::create_dir(&run_dir).unwrap();
        let record = RunRecord {
            session: "s".into(),
            run_id: "r1".into(),
            state: "test".into(),
            command: "true".into(),
            timeout_secs: 5,
            pool: None,
            slot: None,
            machine: "m".into(),
            boot_id: None,
            started_at: store::now_secs(),
            runner_pid: None,
            runner_start: None,
            timeout_capped: false,
        };
        write_atomic(&run_dir, "run.json", &serde_json::to_vec(&record).unwrap()).unwrap();
        let runner = create_new(&run_dir.join("runner.lock")).unwrap();
        let run = create_new(&run_dir.join("run.lock")).unwrap();
        assert!(store::try_flock(&runner) && store::try_flock(&run));
        assert!(matches!(read_run(dir.path(), "r1"), RunState::Running(_)));
        drop(runner);
        assert!(matches!(read_run(dir.path(), "r1"), RunState::Orphaned(_)));
        let result = RunResult {
            exit_code: 3,
            failure_kind: Some("nonzero_exit".into()),
            duration_ms: 10,
            stdout_truncated: false,
            stderr_truncated: false,
            stderr_ends_with_note: false,
        };
        write_atomic(
            &run_dir,
            "result.json",
            &serde_json::to_vec(&result).unwrap(),
        )
        .unwrap();
        match read_run(dir.path(), "r1") {
            RunState::Finished {
                run_lock_held,
                result,
                ..
            } => {
                assert!(run_lock_held);
                assert_eq!(result.outcome(), "failed");
            }
            other => panic!("expected finished, got {other:?}"),
        }
        drop(run);
        match read_run(dir.path(), "r1") {
            RunState::Finished { run_lock_held, .. } => assert!(!run_lock_held),
            other => panic!("expected finished, got {other:?}"),
        }
        std::fs::remove_file(run_dir.join("result.json")).unwrap();
        assert!(matches!(read_run(dir.path(), "r1"), RunState::Lost));
    }
}
