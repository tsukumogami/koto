//! Every test process runs with a `HOME` of its own, from
//! `tests/support/test_home.rs`, so no test writes the `~/.koto/config.toml`
//! or the `~/.cache/koto` of the user running the suite.
//!
//! The first test reads the test crates and fails on one that doesn't get
//! the module. The others run this test binary again with `HOME` and
//! `XDG_CACHE_HOME` at an empty directory standing in for that user's home,
//! where the probe writes a machine id in-process and runs `koto init`,
//! which compiles its template into the cache. The directory must stay
//! empty. The control runs the same probe with the redirect marked as
//! already done, so it keeps that `HOME`, and shows that the probe does
//! write there, so an empty directory is the module's doing.

#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;
#[allow(clippy::duplicate_mod)]
#[path = "support/test_home.rs"]
mod test_home;

use std::path::{Path, PathBuf};
use test_home::TEST_HOME_ENV;

/// Set on a re-run of this binary to a scratch directory, so the probe
/// runs only there.
const PROBE: &str = "KOTO_TEST_HOME_PROBE";

const TEMPLATE: &str = r#"---
name: test-home-probe
version: "1.0"
initial_state: start
states:
  start:
    terminal: true
---

## start

Done.
"#;

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_under(&path, out);
        }
        out.push(path);
    }
}

/// The support modules a file includes, read with its whitespace removed,
/// as rustfmt may split the attribute.
fn includes(path: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap();
    let squashed: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    regex::Regex::new(r#"#\[path="(?:\.\./tests/)?(?:support/)?(\w+)\.rs"\]"#)
        .unwrap()
        .captures_iter(&squashed)
        .map(|c| c[1].to_string())
        .collect()
}

#[test]
fn every_test_crate_gets_the_test_home_module() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for carrier in ["koto_command", "host_session"] {
        assert!(
            includes(&root.join(format!("tests/support/{carrier}.rs")))
                .contains(&"test_home".to_string()),
            "tests/support/{carrier}.rs no longer includes test_home.rs, so the \
             tests that include it run with the real HOME"
        );
    }
    assert!(
        includes(&root.join("src/lib.rs")).contains(&"test_home".to_string()),
        "src/lib.rs no longer includes tests/support/test_home.rs under \
         #[cfg(test)], so the unit tests run with the real HOME"
    );

    let mut crates: Vec<PathBuf> = std::fs::read_dir(root.join("tests"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
        .collect();
    crates.sort();
    assert!(crates.len() > 1, "found no test crate under tests/");
    let missing: Vec<String> = crates
        .iter()
        .filter(|file| {
            !includes(file)
                .iter()
                .any(|m| ["test_home", "koto_command", "host_session"].contains(&m.as_str()))
        })
        .map(|file| file.strip_prefix(root).unwrap().display().to_string())
        .collect();
    assert!(
        missing.is_empty(),
        "these test crates run with the HOME of the user running the suite; \
         include tests/support/koto_command.rs, tests/support/host_session.rs or\n\n\
         #[path = \"support/test_home.rs\"]\nmod test_home;\n\nin each of:\n  {}",
        missing.join("\n  ")
    );
}

/// The probe the tests below run in a child of this binary. Elsewhere it
/// does nothing.
#[test]
fn probe_writes_a_machine_id_and_a_compiled_template() {
    let Some(scratch) = std::env::var_os(PROBE) else {
        return;
    };
    let scratch = PathBuf::from(scratch);
    koto::session::version::get_or_create_machine_id().unwrap();
    std::fs::write(scratch.join("probe.md"), TEMPLATE).unwrap();
    koto_command::koto()
        .current_dir(&scratch)
        .env("KOTO_SESSIONS_BASE", scratch.join("sessions"))
        .args(["init", "probe", "--template", "probe.md"])
        .assert()
        .success();
}

/// Runs the probe in a child of this binary with `HOME` and
/// `XDG_CACHE_HOME` at an empty directory, and returns what the child left
/// there. With `redirected`, the child inherits the marker and keeps that
/// `HOME`.
fn probe_home(redirected: bool) -> Vec<PathBuf> {
    let scratch = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
    koto_command::scrub(&mut cmd)
        .env("HOME", home.path())
        .env("XDG_CACHE_HOME", home.path())
        .env(PROBE, scratch.path())
        .args([
            "--exact",
            "probe_writes_a_machine_id_and_a_compiled_template",
            "--test-threads=1",
        ]);
    if redirected {
        cmd.env(TEST_HOME_ENV, home.path());
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
    let mut left = Vec::new();
    files_under(home.path(), &mut left);
    left.iter()
        .map(|p| p.strip_prefix(home.path()).unwrap().to_path_buf())
        .collect()
}

#[test]
fn a_test_process_leaves_the_home_it_started_with_alone() {
    let left = probe_home(false);
    assert!(
        left.is_empty(),
        "the probe wrote under the HOME the test process started with: {left:?}"
    );
}

#[test]
fn control_a_process_that_keeps_its_home_writes_there() {
    let left = probe_home(true);
    assert!(
        left.contains(&PathBuf::from(".koto/config.toml")),
        "the control probe wrote no machine id, so an empty home proves \
         nothing: {left:?}"
    );
    assert!(
        left.iter()
            .any(|p| p.starts_with("koto") && p.extension().is_some_and(|e| e == "json")),
        "the control probe cached no compiled template, so an empty home \
         proves nothing: {left:?}"
    );
}
