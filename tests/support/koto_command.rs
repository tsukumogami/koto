//! The one place a test names the koto binary.
//!
//! Include it from a test file with
//!
//! ```ignore
//! #[path = "support/koto_command.rs"]
//! mod koto_command;
//! ```
//!
//! koto reads its session backend from `KOTO_SESSION_*` before the project
//! config a test writes, so a koto run that inherits the environment of a
//! shell with the cloud backend configured (`KOTO_SESSION_BACKEND=cloud` and
//! the `KOTO_SESSION_CLOUD_*` values) stores its sessions in that shell's
//! bucket, whatever the test set up. Every command built here has each
//! `KOTO_SESSION_*` variable removed; a test that means to exercise a
//! backend sets it on the command it gets back.
//!
//! A test that runs koto through another process (a shell, a tracer) passes
//! that process's command through [`scrub`], since koto inherits what its
//! parent has. `tests/koto_command_env.rs` fails the suite when a test file
//! names the binary anywhere but here, and proves that a cloud backend in
//! the environment never reaches a command built here.
//!
//! It also includes `test_home.rs`, so every test that runs koto starts with
//! a `HOME` of its own.

#![allow(dead_code)]

// A crate that also includes `host_session.rs` loads it twice; the second
// copy finds the home already redirected and does nothing.
#[allow(clippy::duplicate_mod)]
#[path = "test_home.rs"]
mod test_home;

use std::ffi::OsString;
use std::path::PathBuf;

/// The prefix every session-backend variable koto reads shares.
pub const SESSION_ENV_PREFIX: &str = "KOTO_SESSION_";

/// The `KOTO_SESSION_*` names set in this process's environment, which a
/// child would otherwise inherit.
pub fn session_env_names() -> Vec<OsString> {
    std::env::vars_os()
        .map(|(name, _)| name)
        .filter(|name| name.to_string_lossy().starts_with(SESSION_ENV_PREFIX))
        .collect()
}

/// The path of the koto binary under test, for a template command or a
/// `PATH` entry. Build the process that runs it with [`koto`], [`koto_std`]
/// or [`scrub`].
pub fn koto_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_koto"))
}

/// A koto command with no `KOTO_SESSION_*` variable inherited.
pub fn koto() -> assert_cmd::Command {
    let mut cmd = assert_cmd::Command::new(koto_bin());
    for name in session_env_names() {
        cmd.env_remove(name);
    }
    cmd
}

/// [`koto`] as a `std::process::Command`, for a test that spawns koto and
/// keeps the child running.
pub fn koto_std() -> std::process::Command {
    let mut cmd = std::process::Command::new(koto_bin());
    scrub(&mut cmd);
    cmd
}

/// Removes every inherited `KOTO_SESSION_*` variable from `cmd`, for a
/// process that runs koto itself.
pub fn scrub(cmd: &mut std::process::Command) -> &mut std::process::Command {
    for name in session_env_names() {
        cmd.env_remove(name);
    }
    cmd
}
