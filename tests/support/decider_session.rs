//! Drive `koto next` against the `std::net` decider stub.
//!
//! Include it next to the stub:
//!
//! ```ignore
//! #[path = "support/decider_stub.rs"]
//! mod decider_stub;
//! #[path = "support/decider_session.rs"]
//! mod decider_session;
//! ```
//!
//! A [`Harness`] owns a temp directory with its own `HOME` (so no real
//! `~/.koto/config.toml` is read), a sessions base, a template file, and a
//! running [`DeciderStub`]. Every command it builds strips the
//! `KOTO_DECIDER*` variables first; a test opts in by naming the mode, and
//! the key and endpoint always point at the stub. No test reaches a real
//! provider.
//!
//! [`produce_applied_session_log`] is the fixture later compatibility
//! checks read: a session log holding a `decider_consulted` event and a
//! `source: "decider"` evidence event, written by a real `koto next`.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

use crate::decider_stub::{DeciderStub, Reply};

/// The key every opted-in command sends. Distinctive, so a test can assert
/// it appears nowhere in a log.
pub const KEY: &str = "sk-koto-consult-SECRET-4a7c91";

/// Content seeded under `outline.md`. Distinctive, for the same reason.
pub const OUTLINE: &str = "OUTLINE-7f3a: add a --dry-run flag to the sync command";

/// Default of the `PLAN_DOC` variable.
pub const PLAN_DOC: &str = "docs/plan-7c1e.md";

/// Session name every harness uses.
pub const WF: &str = "wf";

/// The standard template. `review` declares `verdict` (`proceed` in
/// `PMODE`, `exit` in `EMODE`) with an outline input from the context
/// store and the `PLAN_DOC` variable; `gather` gates on the outline and
/// auto-advances into `review` once it exists.
pub const STANDARD: &str = r#"---
name: consult
version: "1.0"
initial_state: gather
variables:
  PLAN_DOC:
    description: Plan path
    default: docs/plan-7c1e.md
states:
  gather:
    gates:
      outline:
        type: context-exists
        key: outline.md
    transitions:
      - target: review
        when:
          gates.outline.exists: true
  review:
    accepts:
      verdict:
        type: enum
        values: [proceed, exit]
        required: true
        description: Is the outline item clear enough to implement?
        decider:
          answers:
            proceed: {description: "Names a concrete change with checkable criteria.", mode: PMODE}
            exit: {description: "Vague, contradictory, or needs design first.", mode: EMODE}
          escape: {value: unclear, description: "Missing, truncated, or unjudgeable."}
          inputs:
            - {context: outline.md, label: outline_item}
            - {var: PLAN_DOC, label: plan_path}
    transitions:
      - target: work
        when:
          verdict: proceed
      - target: rethink
        when:
          verdict: exit
  work:
    accepts:
      done:
        type: boolean
        required: true
        description: Is the work done?
    transitions:
      - target: finished
        when:
          done: true
      - target: review
        when:
          done: false
  rethink:
    accepts:
      again:
        type: boolean
        required: true
        description: Try again?
    transitions:
      - target: review
        when:
          again: true
      - target: finished
        when:
          again: false
  finished:
    terminal: true
---

## gather

Gather the outline item.

## review

Review the outline item for {{PLAN_DOC}} and decide whether it is clear.

## work

Do the work.

## rethink

Rethink the item.

## finished

Finished.
"#;

/// [`STANDARD`] with both modes filled in.
pub fn standard(proceed_mode: &str, exit_mode: &str) -> String {
    STANDARD
        .replace("PMODE", proceed_mode)
        .replace("EMODE", exit_mode)
}

/// A Jev answer to the `verdict` question.
pub fn verdict(proceed: f64, exit: f64, unclear: f64) -> Reply {
    answers(json!({
        "verdict": {
            "type": "choice",
            "choice": "proceed",
            "probabilities": {"proceed": proceed, "exit": exit, "unclear": unclear},
            "confidence": proceed
        }
    }))
}

/// A Jev response with `answers` and a fixed model.
pub fn answers(answers: Value) -> Reply {
    Reply::json(&json!({"model": "jev-test-1.2.3", "answers": answers, "usage": {}}))
}

pub struct Harness {
    _tmp: Option<tempfile::TempDir>,
    pub dir: PathBuf,
    pub stub: DeciderStub,
}

impl Harness {
    /// A harness in a fresh temp directory with `template` written to
    /// `template.md`.
    pub fn new(template: &str) -> Self {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let dir = tmp.path().to_path_buf();
        let mut h = Self::in_dir(&dir, template);
        h._tmp = Some(tmp);
        h
    }

    /// A harness rooted at an existing directory the caller owns.
    pub fn in_dir(dir: &Path, template: &str) -> Self {
        std::fs::create_dir_all(dir.join("home")).expect("home");
        std::fs::create_dir_all(dir.join("sessions")).expect("sessions");
        std::fs::write(dir.join("template.md"), template).expect("template");
        Harness {
            _tmp: None,
            dir: dir.to_path_buf(),
            stub: DeciderStub::start(),
        }
    }

    pub fn home(&self) -> PathBuf {
        self.dir.join("home")
    }

    pub fn session_dir(&self) -> PathBuf {
        self.dir.join("sessions").join(WF)
    }

    pub fn state_path(&self) -> PathBuf {
        self.session_dir().join(format!("koto-{}.state.jsonl", WF))
    }

    pub fn lock_path(&self) -> PathBuf {
        self.session_dir().join("decider.lock")
    }

    /// Write `~/.koto/config.toml` for this harness's HOME.
    pub fn user_config(&self, body: &str) {
        let d = self.home().join(".koto");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("config.toml"), body).unwrap();
    }

    /// Write `.koto/config.toml` in the working directory.
    pub fn project_config(&self, body: &str) {
        let d = self.dir.join(".koto");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("config.toml"), body).unwrap();
    }

    /// A `koto` command with no decider env set at all.
    pub fn koto(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_koto"));
        cmd.current_dir(&self.dir);
        cmd.env("HOME", self.home());
        cmd.env("KOTO_SESSIONS_BASE", self.dir.join("sessions"));
        cmd.env_remove("AWS_ACCESS_KEY_ID");
        cmd.env_remove("AWS_SECRET_ACCESS_KEY");
        cmd.env_remove("KOTO_DECIDER");
        cmd.env_remove("KOTO_DECIDER_API_KEY");
        cmd.env_remove("KOTO_DECIDER_ENDPOINT");
        // Template commands that call `koto` reach this build.
        let bin_dir = Path::new(env!("CARGO_BIN_EXE_koto")).parent().unwrap();
        let path = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", format!("{}:{}", bin_dir.display(), path));
        cmd
    }

    /// A `koto` command with `KOTO_DECIDER=mode` and the key and endpoint
    /// from the environment, pointed at the stub.
    pub fn koto_mode(&self, mode: &str) -> Command {
        let mut cmd = self.koto();
        cmd.env("KOTO_DECIDER", mode);
        cmd.env("KOTO_DECIDER_API_KEY", KEY);
        cmd.env("KOTO_DECIDER_ENDPOINT", self.stub.url());
        cmd
    }

    pub fn init(&self) {
        self.init_with(&[]);
    }

    pub fn init_with(&self, extra: &[&str]) {
        let mut cmd = self.koto();
        cmd.args(["init", WF, "--template", "template.md"]);
        cmd.args(extra);
        let out = cmd.output().unwrap();
        assert!(out.status.success(), "init: {}", describe(&out));
    }

    pub fn context_add(&self, key: &str, content: &str) {
        use std::io::Write as _;
        let mut child = self
            .koto()
            .args(["context", "add", WF, key])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(content.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "context add: {}", describe(&out));
    }

    /// `init` and seed the outline, so the first `koto next` advances into
    /// `review`.
    pub fn ready(&self) {
        self.init();
        self.context_add("outline.md", OUTLINE);
    }

    /// Run `koto next wf` with `cmd`'s environment.
    pub fn next(&self, mut cmd: Command) -> Output {
        cmd.args(["next", WF]).output().unwrap()
    }

    pub fn next_mode(&self, mode: &str) -> Output {
        self.next(self.koto_mode(mode))
    }

    pub fn next_with(&self, mode: &str, data: &str) -> Output {
        let mut cmd = self.koto_mode(mode);
        cmd.args(["next", WF, "--with-data", data]);
        cmd.output().unwrap()
    }

    /// Every event line of the state log, header excluded.
    pub fn events(&self) -> Vec<Value> {
        let body = std::fs::read_to_string(self.state_path()).unwrap();
        body.lines()
            .skip(1)
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    pub fn events_of(&self, ty: &str) -> Vec<Value> {
        self.events()
            .into_iter()
            .filter(|e| e["type"] == ty)
            .collect()
    }

    pub fn consultations(&self) -> Vec<Value> {
        self.events_of("decider_consulted")
            .into_iter()
            .map(|e| e["payload"].clone())
            .collect()
    }

    pub fn raw_log(&self) -> String {
        std::fs::read_to_string(self.state_path()).unwrap()
    }
}

/// stdout parsed as JSON.
pub fn json_out(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON ({}): {}", e, describe(out)))
}

pub fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

pub fn describe(out: &Output) -> String {
    format!(
        "status={:?} stdout={} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Produce a session log holding one `decider_consulted` event (outcome
/// `applied`) and one `evidence_submitted` event with `source: "decider"`,
/// by running a real `koto next` in auto mode against a stub that answers
/// `proceed` at 0.95. Everything is written under `dir`; the returned path
/// is the session's state file.
///
/// The on-disk shape is what an older koto (v0.12.2) must read as an
/// `Unknown` event followed by ordinary evidence.
pub fn produce_applied_session_log(dir: &Path) -> PathBuf {
    let h = Harness::in_dir(dir, &standard("auto", "never"));
    h.ready();
    h.stub.push(verdict(0.95, 0.03, 0.02));
    let out = h.next_mode("auto");
    assert!(out.status.success(), "next: {}", describe(&out));
    assert_eq!(h.stub.request_count(), 1);
    let types: Vec<String> = h
        .events()
        .iter()
        .map(|e| e["type"].as_str().unwrap().to_string())
        .collect();
    assert!(
        types.iter().any(|t| t == "decider_consulted"),
        "{:?}",
        types
    );
    assert!(
        h.events_of("evidence_submitted")
            .iter()
            .any(|e| e["payload"]["source"] == "decider"),
        "{:?}",
        types
    );
    h.state_path()
}
