//! koto v0.33.0, the last release before the machine stamp, can rewrite the
//! header of a session this build created without losing the machine the
//! session records: the name lives on the session's `workflow_initialized`
//! event, never in the header, and v0.33.0 rewrites a header by replacing
//! its first line and copying every event line verbatim.
//!
//! The first test always runs. It replaces the header of a session this
//! build created with the header v0.33.0 wrote for the checked-in store in
//! `tests/fixtures/sessions-v0.33.0/`, carrying over the session's own
//! identity, and checks that `--detail` still reports the same machine.
//!
//! The second runs v0.33.0 itself. Opt in by setting
//! `KOTO_COMPAT_V0_33_BIN` to a koto v0.33.0 release binary; without it the
//! test passes having checked nothing, and says so. The binary must match
//! one of the SHA-256 digests in `tests/fixtures/koto-v0.33.0.sha256`, the
//! release's own `checksums.txt`. Both binaries run in a scratch home on the
//! local backend, every `KOTO_SESSION_*` variable removed.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const BIN_VAR: &str = "KOTO_COMPAT_V0_33_BIN";

const WORK: &str = r#"---
name: machine-compat
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      marker:
        type: enum
        required: true
        values: [done]
    transitions:
      - target: done
        when:
          marker: done
  done:
    terminal: true
---

## work

Work.

## done

Done.
"#;

struct Store {
    _tmp: TempDir,
    root: PathBuf,
    home: PathBuf,
    work: PathBuf,
}

impl Store {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let home = root.join("home");
        let work = root.join("work");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(work.join("work.md"), WORK).unwrap();
        Store {
            _tmp: tmp,
            root,
            home,
            work,
        }
    }

    fn setup(&self, cmd: &mut std::process::Command, dir: &Path) {
        koto_command::scrub(cmd);
        cmd.current_dir(dir)
            .env("HOME", &self.home)
            .env("KOTO_HOME", self.home.join(".koto"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("KOTO_MACHINE_NAME", "machine-a")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("KOTO_SESSIONS_BASE")
            .env_remove("KOTO_ANCHOR")
            .env_remove("KOTO_AUTHORITY")
            .env_remove("KOTO_DISPATCHER_ANCHOR")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("KOTO_WORKFLOWS_DIR");
    }

    fn run(&self, mut cmd: std::process::Command, dir: &Path, args: &[&str], who: &str) -> String {
        self.setup(&mut cmd, dir);
        let out = cmd.args(args).output().unwrap();
        assert!(
            out.status.success(),
            "{who} koto {args:?}: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Run this build of koto, which must succeed.
    fn new_koto(&self, args: &[&str]) -> String {
        self.run(koto_command::koto_std(), &self.work, args, "this build's")
    }

    fn state_path(&self, name: &str) -> PathBuf {
        self.home
            .join(".koto/sessions")
            .join(name)
            .join(format!("koto-{name}.state.jsonl"))
    }

    fn lines(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.state_path(name))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn detail_machine(&self, name: &str) -> Value {
        let out = self.new_koto(&["dashboard", "--detail", name]);
        let v: Value = serde_json::from_str(out.trim()).unwrap();
        v["machine"].clone()
    }
}

/// The header line koto v0.33.0 wrote for the checked-in store.
fn v0_33_header() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sessions-v0.33.0/sessions/v033/koto-v033.state.jsonl");
    let text = std::fs::read_to_string(path).unwrap();
    serde_json::from_str(text.lines().next().unwrap()).unwrap()
}

#[test]
fn a_v0_33_header_over_this_builds_events_keeps_the_machine() {
    let s = Store::new();
    s.new_koto(&["init", "wf", "--template", "work.md"]);
    assert_eq!(s.detail_machine("wf"), "machine-a");

    // The header v0.33.0 writes, with this session's identity and paths:
    // only the fields v0.33.0 knows, in the order it writes them.
    let lines = s.lines("wf");
    let ours: Value = serde_json::from_str(&lines[0]).unwrap();
    let mut header = v0_33_header();
    for (key, value) in header.as_object_mut().unwrap().iter_mut() {
        if let Some(v) = ours.get(key) {
            *value = v.clone();
        }
    }
    assert!(header.get("machine").is_none());
    let mut rewritten = vec![serde_json::to_string(&header).unwrap()];
    rewritten.extend(lines[1..].iter().cloned());
    std::fs::write(s.state_path("wf"), rewritten.join("\n") + "\n").unwrap();

    assert_eq!(s.detail_machine("wf"), "machine-a");
}

/// The v0.33.0 binary the run opted in with, checked against the release's
/// digests; `None` when the run didn't opt in.
fn old_binary() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os(BIN_VAR)?);
    let digests = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/koto-v0.33.0.sha256"),
    )
    .unwrap();
    let digest = hex::encode(Sha256::digest(std::fs::read(&path).unwrap()));
    assert!(
        digests
            .lines()
            .filter_map(|l| l.split_whitespace().next())
            .any(|d| d == digest),
        "{} (sha256 {digest}) is not a koto v0.33.0 release binary",
        path.display()
    );
    Some(path)
}

#[test]
fn koto_v0_33_rewrites_the_header_and_the_machine_stays() {
    let Some(old) = old_binary() else {
        eprintln!("{BIN_VAR} is not set; the v0.33.0 compatibility check did not run");
        return;
    };
    let s = Store::new();
    s.new_koto(&["init", "wf", "--template", "work.md"]);
    assert_eq!(s.detail_machine("wf"), "machine-a");
    let before = s.lines("wf");

    // v0.33.0's rebind rewrites the header and appends its event.
    let moved = s.root.join("moved");
    std::fs::create_dir_all(&moved).unwrap();
    let moved = std::fs::canonicalize(moved).unwrap();
    s.run(
        std::process::Command::new(&old),
        &s.work,
        &["session", "rebind", "wf", "--to", moved.to_str().unwrap()],
        "v0.33.0",
    );
    let after = s.lines("wf");
    assert_ne!(after[0], before[0], "v0.33.0 left the header as it was");
    assert_eq!(
        after[1..before.len()],
        before[1..],
        "v0.33.0 rewrote events"
    );

    assert_eq!(s.detail_machine("wf"), "machine-a");
}
