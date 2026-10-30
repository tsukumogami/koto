//! A test process starts with no koto session store variable in its
//! environment: `tests/support/test_home.rs` removes `KOTO_SESSIONS_BASE`
//! and every `KOTO_SESSION_*` variable as the binary loads. koto's store
//! follows them, so a suite started by a koto gate (which sets
//! `KOTO_SESSIONS_BASE` on every command it runs) or from a shell with the
//! cloud backend configured would otherwise run against that store, from
//! code a test runs in-process and from every child it starts.
//!
//! The probe runs in a child of this binary started with both set to
//! scratch values, and records what it sees in its own environment and in
//! a plain child's. The control starts it marked as already redirected, as
//! a re-run of a test binary is, and shows the probe does see them there,
//! so a clean environment is the module's doing.

#[path = "support/test_home.rs"]
mod test_home;

use std::path::Path;
use test_home::TEST_HOME_ENV;

/// Set on a re-run of this binary to a scratch directory, so the probe
/// runs only there. Its name stays outside the prefix the module removes.
const PROBE: &str = "KOTO_TEST_STORE_ENV_PROBE";

/// The scratch store variables the probe is started with.
const SCRATCH_VARS: [&str; 3] = [
    "KOTO_SESSIONS_BASE",
    "KOTO_SESSION_BACKEND",
    "KOTO_SESSION_CLOUD_BUCKET",
];

fn is_store_var(name: &str) -> bool {
    name == "KOTO_SESSIONS_BASE" || name.starts_with("KOTO_SESSION_")
}

/// The probe the tests below run in a child of this binary. Elsewhere it
/// does nothing.
#[test]
fn probe_records_the_store_variables_it_sees() {
    let Some(scratch) = std::env::var_os(PROBE) else {
        return;
    };
    let mut seen: Vec<String> = std::env::vars_os()
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .filter(|name| is_store_var(name))
        .map(|name| format!("in-process {name}"))
        .collect();
    let out = std::process::Command::new("sh")
        .args(["-c", "env | cut -d= -f1"])
        .output()
        .unwrap();
    assert!(out.status.success(), "the plain child failed");
    seen.extend(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|name| is_store_var(name))
            .map(|name| format!("child {name}")),
    );
    std::fs::write(Path::new(&scratch).join("seen"), seen.join("\n")).unwrap();
}

/// Runs the probe in a child of this binary with the store variables set
/// to scratch values, and returns what it saw. With `redirected`, the child
/// inherits the marker and keeps the environment it was given.
fn probe(redirected: bool) -> Vec<String> {
    let scratch = tempfile::tempdir().unwrap();
    let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
    cmd.env("KOTO_SESSIONS_BASE", scratch.path().join("sessions"))
        .env("KOTO_SESSION_BACKEND", "local")
        .env("KOTO_SESSION_CLOUD_BUCKET", "koto-test-scratch-bucket")
        .env(PROBE, scratch.path())
        .args([
            "--exact",
            "probe_records_the_store_variables_it_sees",
            "--test-threads=1",
        ]);
    if redirected {
        cmd.env(TEST_HOME_ENV, scratch.path());
    } else {
        cmd.env_remove(TEST_HOME_ENV);
    }
    let out = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("1 passed"),
        "the probe failed or didn't run:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::read_to_string(scratch.path().join("seen"))
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn a_test_process_starts_with_no_session_store_variable() {
    let seen = probe(false);
    assert!(
        seen.is_empty(),
        "a test process started with KOTO_SESSIONS_BASE and KOTO_SESSION_* set \
         still has them, so a suite run from a koto gate or a cloud-configured \
         shell reaches that store: {seen:?}"
    );
}

#[test]
fn control_a_process_that_keeps_its_environment_sees_them() {
    let seen = probe(true);
    for name in SCRATCH_VARS {
        for place in ["in-process", "child"] {
            let entry = format!("{place} {name}");
            assert!(
                seen.contains(&entry),
                "the control probe didn't see {name} ({place}), so an empty \
                 result proves nothing: {seen:?}"
            );
        }
    }
}
