//! Gives every test process a home directory of its own, so no test reads
//! or writes the home of the user running the suite.
//!
//! koto keeps two things under the home directory: `get_or_create_machine_id`
//! writes `machine.id` into `~/.koto/config.toml`, the developer's live koto
//! config, and the template cache goes to `$XDG_CACHE_HOME/koto` or
//! `~/.cache/koto`, which the developer's own koto runs share. A test that
//! reaches either without a `HOME` of its own writes there.
//!
//! As its binary loads, this module creates a temporary directory, points
//! `HOME` at it and removes `XDG_CACHE_HOME`, so the cache follows `HOME`,
//! and removes the directory when the process exits. It runs before `main`
//! while the process has one thread, as `host_session.rs` does. A koto run a
//! test starts inherits that `HOME` unless the test gives it its own. It
//! also sets `KOTO_TEST_HOME`, which a re-run of the test binary inherits:
//! a process that finds it set keeps the `HOME` its parent gave it.
//!
//! `koto_command.rs` and `host_session.rs` include it, so every test that
//! runs koto gets it without naming it; a test crate that includes neither
//! includes it directly, and `src/lib.rs` includes it for the unit tests.
//! `tests/test_home_env.rs` fails the suite when a test crate doesn't get it,
//! and proves that a machine id written in-process and a template compiled
//! by the koto binary land outside the `HOME` the process started with.
//!
//! Unix only: on Windows `dirs::home_dir` doesn't read `HOME`.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::OnceLock;

/// Set once a process's home is redirected; its children inherit it.
pub const TEST_HOME_ENV: &str = "KOTO_TEST_HOME";

/// The directory this process created, removed at exit.
static CREATED: OnceLock<PathBuf> = OnceLock::new();

#[cfg(unix)]
extern "C" fn redirect_home() {
    if std::env::var_os(TEST_HOME_ENV).is_some() {
        return;
    }
    let dir = match tempfile::Builder::new().prefix("koto-test-home-").tempdir() {
        Ok(dir) => dir.keep(),
        Err(e) => {
            eprintln!("test_home: cannot create a home for this test process: {e}");
            std::process::abort();
        }
    };
    std::env::set_var("HOME", &dir);
    // A KOTO_HOME from the shell would move the koto root past this HOME.
    std::env::remove_var("KOTO_HOME");
    std::env::remove_var("XDG_CACHE_HOME");
    std::env::set_var(TEST_HOME_ENV, &dir);
    let _ = CREATED.set(dir);
    // SAFETY: registers a plain `extern "C"` function with no arguments.
    unsafe {
        libc::atexit(remove_home);
    }
}

#[cfg(unix)]
extern "C" fn remove_home() {
    if let Some(dir) = CREATED.get() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(unix)]
#[used]
#[cfg_attr(target_vendor = "apple", link_section = "__DATA,__mod_init_func")]
#[cfg_attr(not(target_vendor = "apple"), link_section = ".init_array")]
static REDIRECT_ON_LOAD: extern "C" fn() = redirect_home;
