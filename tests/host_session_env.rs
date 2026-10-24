//! Every test that runs koto's library in-process includes
//! `tests/support/host_session.rs`, which removes the hosting Claude Code
//! session's variables from the test process as it loads. Without it, a
//! suite run inside Claude Code reads that session's id, and every event a
//! test appends through `LocalBackend` writes a workflow file into that
//! session's `/workflows` directory.
//!
//! The first test reads every test file and fails on one that names the
//! `koto` crate without including the module. The others run this test
//! binary again with the variables set and `HOME` in a temporary directory
//! that holds a transcript for the session id, so a leak lands there and
//! nowhere else, and check what an in-process append wrote. The control
//! runs set each variable again after the binary loaded and show that the
//! same append does write there, so an empty directory is the module's
//! doing.

#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;

use host_session::HOST_SESSION_ENV;
use koto::session::local::LocalBackend;
use koto::session::SessionBackend;
use std::path::{Path, PathBuf};

/// Set on a re-run of this binary to the run's root, so the probe runs
/// only there.
const PROBE: &str = "KOTO_HOST_SESSION_PROBE";

/// On a control run, the probe sets the variable named after this prefix
/// to its value, after the binary loaded.
const LEAK_PREFIX: &str = "KOTO_HOST_SESSION_PROBE_LEAK_";

/// An id-shaped Claude Code session id.
const SESSION_ID: &str = "4a6f2c1e-0433-4c4b-9d2e-5f0a1b2c3d4e";

const WORKFLOW: &str = "probe";

const TEMPLATE: &str = r#"---
name: host-session-probe
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

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[test]
fn every_test_that_names_koto_includes_the_host_session_module() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let support = tests.join("support");
    let names_koto = regex::Regex::new(r"\bkoto\s*::").unwrap();
    // Matched on the file with its whitespace removed, as rustfmt may split
    // the attribute.
    let includes = regex::Regex::new(r#"#\[path="support/(\w+)\.rs"\]"#).unwrap();

    let mut crates: Vec<PathBuf> = std::fs::read_dir(&tests)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
        .collect();
    crates.sort();

    let mut linked = 0;
    let mut found = Vec::new();
    for file in &crates {
        let text = std::fs::read_to_string(file).unwrap();
        let squashed: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        let modules: Vec<&str> = includes
            .captures_iter(&squashed)
            .map(|c| c.get(1).unwrap().as_str())
            .collect();
        let names = names_koto.is_match(&text)
            || modules.iter().any(|m| {
                std::fs::read_to_string(support.join(format!("{m}.rs")))
                    .is_ok_and(|t| names_koto.is_match(&t))
            });
        if !names {
            continue;
        }
        linked += 1;
        if !modules.contains(&"host_session") {
            found.push(
                file.strip_prefix(env!("CARGO_MANIFEST_DIR"))
                    .unwrap_or(file)
                    .display()
                    .to_string(),
            );
        }
    }
    assert!(
        linked > 1,
        "found no test file under {} that names the koto crate",
        tests.display()
    );
    assert!(
        found.is_empty(),
        "these test files run koto's library in-process without \
         tests/support/host_session.rs, so a suite run inside Claude Code reads \
         the hosting session's CLAUDE_CODE_SESSION_ID and writes into its \
         /workflows directory; add\n\n#[path = \"support/host_session.rs\"]\n\
         mod host_session;\n\nto each of:\n  {}",
        found.join("\n  ")
    );
}

/// The probe the tests below run in a child of this binary: it appends
/// through `LocalBackend` in-process, as the session update the hosting
/// session's id once leaked into did. Elsewhere it does nothing.
#[test]
fn probe_appends_in_process_under_a_hosting_session() {
    let Some(root) = std::env::var_os(PROBE) else {
        return;
    };
    let root = PathBuf::from(root);
    let sessions = root.join("sessions");
    let mut init = koto_command::koto();
    for name in HOST_SESSION_ENV {
        init.env_remove(name);
    }
    init.current_dir(&root)
        .env("HOME", root.join("home"))
        .env("KOTO_SESSIONS_BASE", &sessions)
        .args(["init", WORKFLOW, "--template", "probe.md"])
        .assert()
        .success();

    for name in HOST_SESSION_ENV {
        match std::env::var_os(format!("{LEAK_PREFIX}{name}")) {
            Some(value) => std::env::set_var(name, value),
            None => assert!(
                std::env::var_os(name).is_none(),
                "{name} is still set after the binary loaded"
            ),
        }
    }
    let backend = LocalBackend::with_base_dir(sessions);
    koto::cli::session::handle_update(&backend, WORKFLOW, "probe intent").unwrap();
}

/// One run of the probe in a child of this binary, with the hosting
/// session's variables set in its environment.
struct Probe {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Probe {
    /// Runs the probe, setting `leak` again after load when given.
    fn run(leak: Option<&str>) -> Probe {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::write(root.join("probe.md"), TEMPLATE).unwrap();
        let project = root.join("home/.claude/projects/probe-project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join(format!("{SESSION_ID}.jsonl")), "").unwrap();
        let probe = Probe { _dir: dir, root };

        let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
        koto_command::scrub(&mut cmd)
            .current_dir(&probe.root)
            .env("HOME", probe.root.join("home"))
            .env(PROBE, &probe.root)
            .env("CLAUDE_CODE_SESSION_ID", SESSION_ID)
            .env("KOTO_WORKFLOWS_DIR", probe.handoff())
            .args([
                "--exact",
                "probe_appends_in_process_under_a_hosting_session",
                "--test-threads=1",
            ]);
        if let Some(name) = leak {
            let value = match name {
                "CLAUDE_CODE_SESSION_ID" => SESSION_ID.into(),
                "KOTO_WORKFLOWS_DIR" => probe.handoff(),
                other => panic!("no control value for {other}"),
            };
            cmd.env(format!("{LEAK_PREFIX}{name}"), value);
        }
        let out = cmd.output().unwrap();
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
        probe
    }

    /// The directory handed over in `KOTO_WORKFLOWS_DIR`.
    fn handoff(&self) -> PathBuf {
        self.root.join("handoff")
    }

    /// The hosting session's `/workflows` directory, beside its transcript.
    fn session_workflows(&self) -> PathBuf {
        self.root
            .join("home/.claude/projects/probe-project")
            .join(SESSION_ID)
            .join("workflows")
    }

    fn files(&self, dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        files_under(dir, &mut out);
        out
    }
}

#[test]
fn an_in_process_append_leaves_the_hosting_session_alone() {
    let probe = Probe::run(None);

    // `koto init` caches the compiled template under the home's `.cache`,
    // so only Claude Code's directory is checked.
    let claude = probe.files(&probe.root.join("home/.claude"));
    assert_eq!(
        claude,
        vec![probe
            .root
            .join("home/.claude/projects/probe-project")
            .join(format!("{SESSION_ID}.jsonl"))],
        "the in-process append wrote into the hosting session's directory"
    );
    assert!(
        !probe.handoff().exists(),
        "the in-process append wrote into KOTO_WORKFLOWS_DIR: {:?}",
        probe.files(&probe.handoff())
    );

    let sessions = probe.root.join("sessions");
    for file in probe.files(&sessions) {
        let bytes = std::fs::read(&file).unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(SESSION_ID),
            "{} records the hosting session's id",
            file.display()
        );
    }
    let (_, events) = LocalBackend::with_base_dir(sessions)
        .read_events(WORKFLOW)
        .unwrap();
    assert_eq!(
        events.last().map(|e| e.event_type.as_str()),
        Some("intent_updated")
    );
}

#[test]
fn control_a_session_id_set_after_load_writes_into_its_workflows() {
    let probe = Probe::run(Some("CLAUDE_CODE_SESSION_ID"));
    assert!(
        !probe.files(&probe.session_workflows()).is_empty(),
        "the control append wrote no workflow file under {}, so an empty one \
         proves nothing",
        probe.session_workflows().display()
    );
}

#[test]
fn control_a_workflows_dir_set_after_load_is_written() {
    let probe = Probe::run(Some("KOTO_WORKFLOWS_DIR"));
    assert!(
        !probe.files(&probe.handoff()).is_empty(),
        "the control append wrote no workflow file under {}, so an empty one \
         proves nothing",
        probe.handoff().display()
    );
}
