//! `koto __run-action <session> <run_id>`: the runner of a background
//! action (DESIGN-host-resource-pools.md, "The runner").
//!
//! A tick starts it, detached in its own session, with the run's
//! `runner.lock`, `run.lock` and (for a pooled action) slot descriptors
//! inherited and a spec on standard input. The runner holds `runner.lock`
//! for its life and keeps it out of the command; the command and anything
//! it starts inherit the slot and `run.lock`, so the slot frees only when
//! every process of the run has gone. It runs the command with the tick's
//! environment and redaction, cuts it at its timeout, writes the output and
//! `result.json`, and exits. It never writes the session log.
//!
//! Hidden from help: nothing but a tick starts it.

use std::io::Read;
use std::os::fd::RawFd;
use std::path::PathBuf;

use crate::engine::background::{self, RunResult, RunSpec, BACKGROUND_OF_ENV, MAX_SPEC_BYTES};

/// The exit status of a runner that refused to start. It wrote nothing.
pub const EXIT_REFUSED: i32 = 2;

/// Run the runner. Returns the process exit status.
pub fn run(session: &str, run_id: &str) -> i32 {
    let Some(spec) = accept(session, run_id) else {
        return EXIT_REFUSED;
    };

    // `runner.lock` stays with this process: the command must not inherit
    // it, or a command that outlived the runner would read as running.
    if set_cloexec(spec.runner_lock_fd).is_err() {
        return EXIT_REFUSED;
    }

    // The command gets the tick's environment, except that it isn't inside
    // a tick: `KOTO_TICK_SESSION` gives way to `KOTO_BACKGROUND_OF`, so the
    // command may tick another session but not the one that started it.
    let tick = crate::engine::reentrancy::TICK_SESSION_ENV;
    std::env::remove_var(tick);
    let mut vars: Vec<(String, String)> =
        spec.vars.into_iter().filter(|(n, _)| n != tick).collect();
    vars.push((BACKGROUND_OF_ENV.to_string(), spec.session.clone()));
    let redactor = crate::redact::Redactor::new(spec.redact);
    let env = if spec.inherit {
        crate::action::CommandEnv::inherited(vars, redactor)
    } else {
        crate::action::CommandEnv::cleared(vars, redactor)
    };

    let output =
        crate::action::run_shell_command(&spec.command, &spec.working_dir, spec.timeout_secs, &env);

    // The streams first, then the result that says they are complete.
    let dir = &spec.run_dir;
    let _ = background::write_atomic(dir, "stdout", output.stdout.as_str().as_bytes());
    let _ = background::write_atomic(dir, "stderr", output.stderr.as_str().as_bytes());
    let result = RunResult::from_output(&output);
    match serde_json::to_vec(&result) {
        Ok(bytes) if background::write_atomic(dir, "result.json", &bytes).is_ok() => 0,
        _ => 1,
    }
}

/// Read and check the spec. `None` (refuse, writing nothing) unless the
/// arguments are well formed, the spec names this run, `run.json` agrees,
/// and every descriptor the spec names is open on the file it should be.
fn accept(session: &str, run_id: &str) -> Option<RunSpec> {
    if !background::is_valid_run_id(run_id) {
        return None;
    }
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(MAX_SPEC_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_SPEC_BYTES {
        return None;
    }
    let spec: RunSpec = serde_json::from_slice(&bytes).ok()?;
    if spec.session != session || spec.run_id != run_id {
        return None;
    }
    if spec.run_dir.file_name().and_then(|n| n.to_str()) != Some(run_id) {
        return None;
    }
    let record = background::read_run_record(&spec.run_dir)?;
    if record.session != session || record.run_id != run_id {
        return None;
    }
    let dir = &spec.run_dir;
    let mut expected: Vec<(RawFd, PathBuf)> = vec![
        (spec.runner_lock_fd, dir.join("runner.lock")),
        (spec.run_lock_fd, dir.join("run.lock")),
    ];
    expected.extend(spec.slot.clone());
    expected
        .iter()
        .all(|(fd, path)| background::fd_is_file(*fd, path))
        .then_some(spec)
}

/// Mark `fd` close-on-exec.
fn set_cloexec(fd: RawFd) -> std::io::Result<()> {
    // SAFETY: fcntl on a descriptor this process inherited for the purpose.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: as above.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
