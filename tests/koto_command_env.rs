//! Every test that runs koto builds its command through
//! `tests/support/koto_command.rs`, which removes each `KOTO_SESSION_*`
//! variable the test process inherited. Without it, `cargo test` in a shell
//! with the cloud session backend configured sends the tests' koto runs to
//! that shell's bucket instead of the store each test set up.
//!
//! The first test reads every file under `tests/` and fails on one that
//! builds a koto command any other way. The second runs this test binary
//! again with a cloud backend in its environment, aimed at an in-process
//! endpoint, and checks that the koto runs the helper built sent it nothing.

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/koto_command.rs"]
mod koto_command;

use fake_s3::FakeS3;
use std::path::{Path, PathBuf};

const BUCKET: &str = "koto-command-env";

/// Set on the re-run of this binary, so the probe runs only there.
const PROBE: &str = "KOTO_COMMAND_ENV_PROBE";

const TEMPLATE: &str = r#"---
name: env-probe
version: "1.0"
initial_state: start
states:
  start:
    accepts:
      go:
        type: enum
        values: ["yes"]
        required: true
    transitions:
      - target: done
        when:
          go: "yes"
  done:
    terminal: true
---

## start

Submit go.

## done

Done.
"#;

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The ways a test could build a koto command without the helper: naming
/// the binary (`assert_cmd`'s lookup or Cargo's path variable), or handing
/// the helper's path straight to a `Command`. Matched on the file with its
/// whitespace removed, so a call rustfmt split across lines still counts.
/// The literals are split so this file doesn't match itself.
fn bypasses() -> Vec<(regex::Regex, &'static str)> {
    vec![
        (
            regex::Regex::new(concat!("cargo_bin", r#"\("koto"\)"#)).unwrap(),
            "names the binary through assert_cmd",
        ),
        (
            regex::Regex::new(concat!("CARGO_BIN_EXE", "_koto")).unwrap(),
            "names the binary through Cargo's path variable",
        ),
        (
            regex::Regex::new(concat!(
                r"Command::new\(&?(crate::)?koto_command::",
                r"koto_bin\(\)"
            ))
            .unwrap(),
            "builds a Command on the helper's path",
        ),
    ]
}

#[test]
fn every_test_builds_its_koto_command_through_the_helper() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let helper = tests.join("support").join("koto_command.rs");
    let mut files = Vec::new();
    rs_files(&tests, &mut files);
    files.sort();
    assert!(
        files.len() > 1,
        "no test files found under {}",
        tests.display()
    );

    let mut found = Vec::new();
    for file in files.iter().filter(|f| **f != helper) {
        let text = std::fs::read_to_string(file).unwrap();
        let squashed: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        for (pattern, what) in bypasses() {
            if pattern.is_match(&squashed) {
                found.push(format!(
                    "{} {what}",
                    file.strip_prefix(env!("CARGO_MANIFEST_DIR"))
                        .unwrap_or(file)
                        .display()
                ));
            }
        }
    }
    assert!(
        found.is_empty(),
        "these test files build a koto command that inherits KOTO_SESSION_* from \
         the shell running cargo test, which can send their sessions to a real \
         bucket; build it with koto_command::koto() or koto_command::koto_std() \
         from tests/support/koto_command.rs, and pass a process that runs koto \
         itself through koto_command::scrub():\n  {}",
        found.join("\n  ")
    );
}

/// The probe that [`a_cloud_backend_in_the_environment_never_reaches_the_helpers_commands`]
/// runs in a child of this binary. Elsewhere it does nothing.
#[test]
fn probe_runs_koto_under_an_inherited_cloud_backend() {
    if std::env::var_os(PROBE).is_none() {
        return;
    }
    let inherited = koto_command::session_env_names();
    assert!(
        inherited.iter().any(|n| n == "KOTO_SESSION_BACKEND"),
        "the probe ran without the cloud backend in its environment: {inherited:?}"
    );

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("probe.md"), TEMPLATE).unwrap();
    let sessions = root.join("sessions");
    let run = |args: &[&str]| {
        let mut cmd = koto_command::koto();
        cmd.env_remove("CLAUDE_CODE_SESSION_ID")
            .current_dir(root)
            .env("HOME", root)
            .env("KOTO_SESSIONS_BASE", &sessions)
            .args(args)
            .assert()
            .success();
    };
    run(&["init", "wf", "--template", "probe.md"]);
    run(&["next", "wf"]);

    let out = koto_command::koto_std()
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .current_dir(root)
        .env("HOME", root)
        .env("KOTO_SESSIONS_BASE", &sessions)
        .args(["next", "wf", "--with-data", r#"{"go": "yes"}"#])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    #[cfg(unix)]
    {
        let path = format!(
            "{}:{}",
            koto_command::koto_bin().parent().unwrap().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let out = koto_command::scrub(&mut std::process::Command::new("sh"))
            .args(["-c", "koto status wf"])
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .current_dir(root)
            .env("PATH", path)
            .env("HOME", root)
            .env("KOTO_SESSIONS_BASE", &sessions)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn cloud_env(s3: &FakeS3) -> Vec<(&'static str, String)> {
    vec![
        ("KOTO_SESSION_BACKEND", "cloud".to_string()),
        ("KOTO_SESSION_CLOUD_ENDPOINT", s3.endpoint()),
        ("KOTO_SESSION_CLOUD_BUCKET", s3.bucket().to_string()),
        ("KOTO_SESSION_CLOUD_REGION", "us-east-1".to_string()),
        ("KOTO_SESSION_CLOUD_PATH_STYLE", "true".to_string()),
        (
            "KOTO_SESSION_CLOUD_ACCESS_KEY",
            "koto-access-key-id".to_string(),
        ),
        (
            "KOTO_SESSION_CLOUD_SECRET_KEY",
            "koto-secret-key-value".to_string(),
        ),
        ("KOTO_SESSION_CLOUD_MARKER_RETAIN_DAYS", "1".to_string()),
    ]
}

#[test]
fn a_cloud_backend_in_the_environment_never_reaches_the_helpers_commands() {
    let s3 = FakeS3::start(BUCKET);

    // Control: the same koto run with the backend set on its command does
    // reach the endpoint, so a silent endpoint below is the helper's doing.
    let control = tempfile::tempdir().unwrap();
    std::fs::write(control.path().join("probe.md"), TEMPLATE).unwrap();
    koto_command::koto()
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .current_dir(control.path())
        .env("HOME", control.path())
        .envs(cloud_env(&s3))
        .args(["init", "wf", "--template", "probe.md"])
        .assert()
        .success();
    assert!(
        !s3.requests().is_empty(),
        "the control run made no request, so a silent endpoint proves nothing"
    );
    s3.clear_requests();

    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .envs(cloud_env(&s3))
        .env(PROBE, "1")
        .args([
            "--exact",
            "probe_runs_koto_under_an_inherited_cloud_backend",
            "--test-threads=1",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "the probe failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("1 passed"),
        "the probe didn't run:\n{stdout}"
    );
    assert_eq!(
        s3.requests().len(),
        0,
        "koto commands built by the helper reached the cloud endpoint in their \
         inherited environment: {:?}",
        s3.requests()
    );
}
