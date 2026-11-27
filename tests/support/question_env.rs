//! A scratch koto home for the question reads' tests: the sessions of the
//! escalation ladder, raising questions with stored content, and recording
//! answers to them, a person's from the accepted fixture under
//! `tests/fixtures/prompt/`, never from a live prompt.
//!
//! Include it beside `koto_command.rs` and `prompt_fixture.rs`:
//!
//! ```ignore
//! #[path = "support/koto_command.rs"]
//! mod koto_command;
//! #[path = "support/prompt_fixture.rs"]
//! mod prompt_fixture;
//! #[path = "support/question_env.rs"]
//! mod question_env;
//! ```
//!
//! Every command runs the real binary with `HOME`, `KOTO_HOME` and
//! `KOTO_SESSIONS_BASE` in the environment's own temporary directory, which
//! is removed when it is dropped, and with the variables that would point
//! koto anywhere else removed.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::koto_command;
use super::prompt_fixture as fx;
use serde_json::Value;

const CLEARED_ENV: [&str; 6] = [
    "CLAUDE_CODE_SESSION_ID",
    "KOTO_ANCHOR",
    "KOTO_AGENT_ID",
    "KOTO_RUN_MODE",
    "KOTO_AUTHORITY",
    "KOTO_TEST_DECISIONS_FAULT",
];

pub const TEMPLATE: &str = r#"---
name: work
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      go:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: done
        when:
          go: go
  done:
    terminal: true
---

## work

Work.

## done

Done.
"#;

pub struct Env {
    _records: fx::Records,
    _tmp: tempfile::TempDir,
    pub root: PathBuf,
    sessions: PathBuf,
    koto_home: PathBuf,
    calls: std::cell::Cell<u32>,
}

pub struct Out {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Out {
    pub fn json(&self) -> Value {
        serde_json::from_str(self.stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON: {e}\n{}\n{}", self.stdout, self.stderr))
    }

    pub fn error(&self) -> String {
        self.json()["error"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    pub fn all(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
    }
}

/// A raised question: its session, id, the session's id and the prompt
/// header its content was given.
#[derive(Clone)]
pub struct Q {
    pub session: String,
    pub id: String,
    pub session_id: String,
    pub header: String,
}

impl Q {
    pub fn identity(&self) -> String {
        format!("{}/{}", self.session_id, self.id)
    }
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

impl Env {
    pub fn new() -> Self {
        Self::at("sessions", "home")
    }

    /// An environment whose sessions directory is `home/.koto/sessions`,
    /// where the cloud backend keeps its local copy, and whose koto root is
    /// the default `home/.koto`, so the same store can be read through
    /// either backend.
    pub fn cloud_copy() -> Self {
        Self::at("home/.koto/sessions", "home/.koto")
    }

    fn at(sessions: &str, koto_home: &str) -> Self {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        for dir in ["home", "ws", sessions] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("work.md"), TEMPLATE).unwrap();
        Env {
            _records: fx::Records::new(),
            _tmp: tmp,
            sessions: root.join(sessions),
            koto_home: root.join(koto_home),
            root,
            calls: std::cell::Cell::new(0),
        }
    }

    pub fn sessions(&self) -> PathBuf {
        self.sessions.clone()
    }

    /// The koto command for `args` in this scratch home, not yet run.
    pub fn command(&self, envs: &[(&str, &str)], args: &[&str]) -> std::process::Command {
        let mut cmd = koto_command::koto_std();
        for name in CLEARED_ENV {
            cmd.env_remove(name);
        }
        for (k, v) in envs {
            cmd.env(k, v);
        }
        cmd.current_dir(self.root.join("ws"));
        cmd.env("HOME", self.root.join("home"));
        cmd.env("KOTO_HOME", &self.koto_home);
        cmd.env("KOTO_SESSIONS_BASE", self.sessions());
        cmd.args(args);
        cmd
    }

    pub fn run_env(&self, envs: &[(&str, &str)], args: &[&str]) -> Out {
        let output = self.command(envs, args).output().unwrap();
        Out {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    pub fn run(&self, args: &[&str]) -> Out {
        self.run_env(&[], args)
    }

    pub fn ok(&self, args: &[&str]) -> Out {
        let out = self.run(args);
        assert_eq!(out.code, 0, "expected success from {args:?}\n{}", out.all());
        out
    }

    pub fn init(&self, name: &str, extra: &[&str]) {
        let template = self.root.join("work.md");
        let mut args = vec!["init", name, "--template", template.to_str().unwrap()];
        args.extend_from_slice(extra);
        self.ok(&args);
    }

    /// director > manager > worker, worker2; other-lead > other-worker.
    pub fn ladder(&self) {
        self.init("director", &[]);
        self.init("manager", &["--dispatched-by", "director"]);
        self.init("worker", &["--dispatched-by", "manager"]);
        self.init("worker2", &["--dispatched-by", "manager"]);
        self.init("other-lead", &[]);
        self.init("other-worker", &["--dispatched-by", "other-lead"]);
    }

    pub fn escalate_run(&self, name: &str, question: &str, recipient: &str, extra: &[&str]) -> Out {
        let q = format!("question={question}");
        let t = format!("text=what about {question}?");
        let r = format!("recipient={recipient}");
        let mut args = vec![
            "decisions",
            "record",
            name,
            "--kind",
            "escalate",
            "--outcome",
            "escalated",
            "--subject",
            &q,
            "--subject",
            &t,
            "--subject",
            &r,
        ];
        for e in extra {
            args.extend_from_slice(&["--subject", e]);
        }
        args.extend_from_slice(&["--by", "session"]);
        self.run(&args)
    }

    /// Raise `question` in `name` to `recipient`, with `extra` subject
    /// fields and the content the accepted fixture shows stored for it, so
    /// a person's answer can be recorded to it.
    pub fn raise(&self, name: &str, question: &str, recipient: &str, extra: &[&str]) -> Q {
        let lines = fx::lines("accepted");
        let (shown, labels) = fx::shown(&lines, 1);
        let content = fx::content(&shown, &labels);
        let packet = format!("packet={}", fx::packet(&content));
        let mut all: Vec<&str> = extra.to_vec();
        all.push(&packet);
        let out = self.escalate_run(name, question, recipient, &all);
        assert_eq!(out.code, 0, "{}", out.all());
        let out = out.json();
        let id = out["id"].as_str().unwrap().to_string();
        let file = self.root.join(format!("content-{name}-{id}"));
        std::fs::write(&file, &content).unwrap();
        let written = self
            .ok(&[
                "decisions",
                "content",
                "write",
                name,
                &id,
                "--file",
                file.to_str().unwrap(),
            ])
            .json();
        Q {
            session: name.to_string(),
            id,
            session_id: out["decision"]["session"]["id"]
                .as_str()
                .unwrap()
                .to_string(),
            header: written["prompt_header"].as_str().unwrap().to_string(),
        }
    }

    /// `koto decisions answer <q> --in <in_session>` with `extra`.
    pub fn answer_run(&self, q: &Q, in_session: &str, extra: &[&str]) -> Out {
        let mut args = vec!["decisions", "answer", &q.session, &q.id, "--in", in_session];
        args.extend_from_slice(extra);
        self.run(&args)
    }

    /// A session's `answered` answer that must be accepted; its id.
    pub fn say_ok(&self, q: &Q, in_session: &str, text: &str) -> String {
        let out = self.answer_run(q, in_session, &["--text", text, "--by", "session"]);
        assert_eq!(out.code, 0, "{}", out.all());
        out.json()["id"].as_str().unwrap().to_string()
    }

    /// Record `person:owner`'s answer to `q` in `in_session` from a fresh
    /// copy of the accepted fixture (its own call id), with `extra` flags.
    pub fn person_run(&self, q: &Q, in_session: &str, extra: &[&str]) -> Out {
        self.person_run_as(q, in_session, "owner", extra)
    }

    /// As [`Env::person_run`], answering as `person:<handle>`.
    pub fn person_run_as(&self, q: &Q, in_session: &str, handle: &str, extra: &[&str]) -> Out {
        let by = format!("person:{handle}");
        let n = self.calls.get() + 1;
        self.calls.set(n);
        let call = format!("toolu_ZQXMARKERcall{n:012}");
        let lines = fx::with_call(&fx::lines("accepted"), &call);
        let harness = fx::harness();
        fx::install(&harness, &fx::render(&lines, &harness, &[&q.header]));
        let pointer = format!("{harness}/{call}/1");
        let mut args = vec![
            "decisions",
            "answer",
            &q.session,
            &q.id,
            "--in",
            in_session,
            "--by",
            &by,
            "--prompt",
            &pointer,
        ];
        args.extend_from_slice(extra);
        self.run_env(&[("CLAUDE_CODE_SESSION_ID", harness.as_str())], &args)
    }

    /// A person's answer that must be accepted; its id.
    pub fn person(&self, q: &Q, in_session: &str) -> String {
        self.person_with(q, in_session, &[])
    }

    pub fn person_with(&self, q: &Q, in_session: &str, extra: &[&str]) -> String {
        let out = self.person_run(q, in_session, extra);
        assert_eq!(out.code, 0, "{}", out.all());
        out.json()["id"].as_str().unwrap().to_string()
    }

    pub fn withdraw(&self, q: &Q, in_session: &str, reason: &str) -> String {
        let out = self.answer_run(
            q,
            in_session,
            &[
                "--outcome",
                "withdrawn",
                "--reason",
                reason,
                "--by",
                "session",
            ],
        );
        assert_eq!(out.code, 0, "{}", out.all());
        out.json()["id"].as_str().unwrap().to_string()
    }

    pub fn settle(&self, q: &Q, in_session: &str, extra: &[&str]) -> Out {
        let mut args = vec![
            "--outcome",
            "settled",
            "--text",
            "settled",
            "--by",
            "session",
        ];
        args.extend_from_slice(extra);
        self.answer_run(q, in_session, &args)
    }

    pub fn settle_ok(&self, q: &Q, in_session: &str, basis: &str) -> String {
        let out = self.settle(q, in_session, &["--basis", basis]);
        assert_eq!(out.code, 0, "{}", out.all());
        out.json()["id"].as_str().unwrap().to_string()
    }

    pub fn readdress(&self, q: &Q, in_session: &str, to: &str) -> String {
        let out = self.run(&[
            "decisions",
            "readdress",
            &q.session,
            &q.id,
            "--in",
            in_session,
            "--to",
            to,
            "--by",
            "session",
        ]);
        assert_eq!(out.code, 0, "{}", out.all());
        out.json()["id"].as_str().unwrap().to_string()
    }

    /// `koto decisions show <holder> <id>`.
    pub fn show(&self, holder: &str, id: &str) -> Value {
        self.ok(&["decisions", "show", holder, id]).json()
    }

    /// Record a judgment in `name` citing `basis`; its id.
    pub fn cite(&self, name: &str, basis: &str) -> String {
        self.ok(&[
            "decisions",
            "record",
            name,
            "--kind",
            "judgment",
            "--outcome",
            "settled",
            "--subject",
            "what=cited",
            "--by",
            "session",
            "--basis",
            basis,
        ])
        .json()["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Every file under the sessions directory with its bytes, lock files
    /// aside.
    pub fn snapshot_all(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, base, out);
                } else if !path.to_string_lossy().ends_with(".lock") {
                    out.insert(
                        path.strip_prefix(base).unwrap().to_path_buf(),
                        std::fs::read(&path).unwrap(),
                    );
                }
            }
        }
        let mut out = BTreeMap::new();
        let dir = self.sessions();
        walk(&dir, &dir, &mut out);
        out
    }

    /// The path of entry `id` in `holder`'s aimed area for `q`.
    pub fn aimed(&self, holder: &str, q: &Q, id: &str) -> PathBuf {
        self.sessions()
            .join(holder)
            .join("ctx/koto/aimed")
            .join(&q.session_id)
            .join(id)
    }

    pub fn stored(&self, holder: &str, q: &Q, id: &str) -> Value {
        serde_json::from_slice(&std::fs::read(self.aimed(holder, q, id)).unwrap()).unwrap()
    }

    /// Store `body` as entry `id` in `holder`'s aimed area for `q`.
    pub fn plant(&self, holder: &str, q: &Q, id: &str, body: &Value) {
        let path = self.aimed(holder, q, id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_vec(body).unwrap()).unwrap();
    }

    /// Read `holder` as a session that arrived from another host: drop the
    /// host-local note of the prompts it checked.
    pub fn forget_verified(&self, holder: &str) {
        let note = self.sessions().join(holder).join("koto-verified-here");
        if note.exists() {
            std::fs::remove_file(note).unwrap();
        }
    }

    /// Open an anchor for `harness`, with `extra` flags; its name.
    pub fn open_anchor(&self, harness: &str, extra: &[&str]) -> String {
        let mut args = vec![
            "session",
            "anchor",
            "open",
            "--harness-session",
            harness,
            "--source",
            "startup",
        ];
        args.extend_from_slice(extra);
        let opened = self.ok(&args);
        opened
            .stdout
            .lines()
            .find_map(|l| l.strip_prefix("anchor="))
            .unwrap()
            .to_string()
    }

    /// Remove `name` the way an operator does.
    pub fn cleanup(&self, name: &str) {
        self.ok(&["session", "cleanup", name]);
        assert!(!self.sessions().join(name).exists(), "{name} still stored");
    }
}

/// `time`, an RFC 3339 time, moved `years` years.
pub fn shift(time: &str, years: i32) -> String {
    let year: i32 = time[..4].parse().unwrap();
    format!("{:04}{}", year + years, &time[4..])
}
