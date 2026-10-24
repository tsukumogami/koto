//! Keeps the hosting Claude Code session out of a test that runs koto's
//! library in-process.
//!
//! Include it from every test file that names the `koto` crate, itself or
//! through a support module it includes:
//!
//! ```ignore
//! #[path = "support/host_session.rs"]
//! mod host_session;
//! ```
//!
//! Claude Code sets `CLAUDE_CODE_SESSION_ID` in every process it starts, and
//! a `SessionStart` hook can hand koto a `/workflows` directory in
//! `KOTO_WORKFLOWS_DIR`. koto reads both on every event appended through
//! `LocalBackend`: the run journal records the id as the session's driver,
//! and the `/workflows` renderer writes the session's workflow file into the
//! hosting session's directory, then appends a `context_added` event
//! publishing it. koto's unit tests read these variables through a seam
//! they control, but an integration test links the library without
//! `cfg(test)`, so it reads the real environment.
//!
//! Including this module removes both variables from the test process as
//! its binary loads, before the harness starts a test thread, so no test
//! has to clear them and none races another that reads them. A test that
//! wants one set gives it to the child process it starts.
//! `tests/host_session_env.rs` fails the suite when a test file that names
//! the `koto` crate doesn't include this module, and proves that an
//! in-process append in a binary that does include it reads neither
//! variable and writes nothing to the hosting session's `/workflows`.

#![allow(dead_code)]

/// The hosting session's variables that koto's library reads in-process.
pub const HOST_SESSION_ENV: [&str; 2] = ["CLAUDE_CODE_SESSION_ID", "KOTO_WORKFLOWS_DIR"];

extern "C" fn clear_host_session_env() {
    for name in HOST_SESSION_ENV {
        std::env::remove_var(name);
    }
}

// Registered as a load-time constructor, which the loader runs before
// `main` while the process has one thread: Mach-O's `__mod_init_func`, the
// CRT's initializer table on Windows, `.init_array` on ELF targets.
#[used]
#[cfg_attr(target_vendor = "apple", link_section = "__DATA,__mod_init_func")]
#[cfg_attr(windows, link_section = ".CRT$XCU")]
#[cfg_attr(
    not(any(target_vendor = "apple", windows)),
    link_section = ".init_array"
)]
static CLEAR_ON_LOAD: extern "C" fn() = clear_host_session_env;
