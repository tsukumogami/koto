//! Integration tests for keeping sessions that hold open questions: the
//! terminal tick's `open_question` retention, answers and pass-ups as
//! hierarchy edges, the prunes, and the `holds_question` refusal of
//! `koto session cleanup` and `koto cancel --cleanup` with its `--force`.
//!
//! Every local test runs the real binary with `HOME` and
//! `KOTO_SESSIONS_BASE` pointed into its own temporary directory, and with
//! every variable koto reads an identity or a backend from removed. The
//! cloud case runs against the in-process endpoint in `support/fake_s3.rs`.

#![cfg(unix)]

#[path = "support/fake_s3.rs"]
mod fake_s3;
#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;
#[path = "support/prompt_fixture.rs"]
mod prompt_fixture;

use prompt_fixture as fx;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use assert_fs::TempDir;
use serde_json::{json, Value};

/// Variables koto reads an identity, a run mode, an authority or a
/// session backend from.
const CLEARED_ENV: [&str; 9] = [
    "CLAUDE_CODE_SESSION_ID",
    "KOTO_ANCHOR",
    "KOTO_AGENT_ID",
    "KOTO_RUN_MODE",
    "KOTO_AUTHORITY",
    "KOTO_TEST_DECISIONS_FAULT",
    "KOTO_SESSION_BACKEND",
    "KOTO_SESSION_ID",
    "KOTO_SESSION_NAME",
];

/// A retention short enough for a test to wait out.
const FAST: [(&str, &str); 1] = [("KOTO_RETENTION_KEEP", "1s")];

const TEMPLATE: &str = r#"---
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

/// Wait out [`FAST`]'s retention.
fn settle() {
    std::thread::sleep(std::time::Duration::from_secs(2));
}

struct Env {
    _tmp: TempDir,
    root: PathBuf,
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Out {
    fn json(&self) -> Value {
        serde_json::from_str(self.stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON: {e}\n{}\n{}", self.stdout, self.stderr))
    }

    /// The last non-empty stdout line as JSON, for a command that prints a
    /// text preview before its report.
    fn last_json(&self) -> Value {
        let line = self
            .stdout
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or_else(|| panic!("no output\n{}", self.stderr));
        serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("not JSON: {e}\n{}\n{}", self.stdout, self.stderr))
    }
}

/// A recorded escalation: its session, id and the session's id.
#[derive(Clone)]
struct Rec {
    session: String,
    id: String,
    session_id: String,
}

impl Rec {
    fn identity(&self) -> String {
        format!("{}/{}", self.session_id, self.id)
    }
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        for dir in ["home", "ws", "sessions"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("work.md"), TEMPLATE).unwrap();
        Env { _tmp: tmp, root }
    }

    fn sessions(&self) -> PathBuf {
        self.root.join("sessions")
    }

    fn template(&self) -> String {
        self.root.join("work.md").to_string_lossy().into_owned()
    }

    fn run(&self, args: &[&str]) -> Out {
        self.run_env(&[], args)
    }

    fn run_env(&self, envs: &[(&str, &str)], args: &[&str]) -> Out {
        self.run_input(envs, args, None)
    }

    /// Run koto with `envs` set and `input` on its stdin (none: empty).
    fn run_input(&self, envs: &[(&str, &str)], args: &[&str], input: Option<&str>) -> Out {
        self.run_at(&self.root.join("ws"), envs, args, input)
    }

    /// [`Env::run_input`] from directory `cwd`.
    fn run_at(&self, cwd: &Path, envs: &[(&str, &str)], args: &[&str], input: Option<&str>) -> Out {
        let mut cmd = koto_command::koto_std();
        for name in CLEARED_ENV {
            cmd.env_remove(name);
        }
        for (k, v) in envs {
            cmd.env(k, v);
        }
        cmd.current_dir(cwd);
        cmd.env("HOME", self.root.join("home"));
        cmd.env("KOTO_HOME", self.root.join("home"));
        cmd.env("KOTO_SESSIONS_BASE", self.sessions());
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        {
            let mut stdin = child.stdin.take().unwrap();
            if let Some(input) = input {
                stdin.write_all(input.as_bytes()).unwrap();
            }
        }
        let output = child.wait_with_output().unwrap();
        Out {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    fn ok(&self, args: &[&str]) -> Out {
        self.ok_env(&[], args)
    }

    fn ok_env(&self, envs: &[(&str, &str)], args: &[&str]) -> Out {
        let out = self.run_env(envs, args);
        assert_eq!(
            out.code, 0,
            "expected success from {args:?}\n{}\n{}",
            out.stdout, out.stderr
        );
        out
    }

    fn init(&self, name: &str, extra: &[&str]) {
        let template = self.template();
        let mut args = vec!["init", name, "--template", template.as_str()];
        args.extend_from_slice(extra);
        self.ok(&args);
    }

    fn escalate(&self, name: &str, question: &str, recipient: &str) -> Rec {
        let q = format!("question={question}");
        let t = format!("text=what about {question}?");
        let r = format!("recipient={recipient}");
        let out = self
            .ok(&[
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
                "--by",
                "session",
            ])
            .json();
        Rec {
            session: name.to_string(),
            id: out["id"].as_str().unwrap().to_string(),
            session_id: out["decision"]["session"]["id"]
                .as_str()
                .unwrap()
                .to_string(),
        }
    }

    /// Answer `q` from `in_session`; returns the answer's id.
    fn answer(&self, q: &Rec, in_session: &str, text: &str) -> String {
        let out = self.ok(&[
            "decisions",
            "answer",
            &q.session,
            &q.id,
            "--in",
            in_session,
            "--text",
            text,
            "--by",
            "session",
        ]);
        out.json()["id"].as_str().unwrap().to_string()
    }

    /// Pass `q` up from `in_session` to `to`.
    fn readdress(&self, q: &Rec, in_session: &str, to: &str) -> String {
        let out = self.ok(&[
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
        out.json()["id"].as_str().unwrap().to_string()
    }

    /// Where `holder` stores its entry `id` aimed at `q`.
    fn aimed_path(&self, holder: &str, q: &Rec, id: &str) -> PathBuf {
        self.sessions()
            .join(holder)
            .join("ctx/koto/aimed")
            .join(&q.session_id)
            .join(id)
    }

    fn stored(&self, holder: &str, q: &Rec, id: &str) -> Value {
        serde_json::from_slice(&std::fs::read(self.aimed_path(holder, q, id)).unwrap()).unwrap()
    }

    /// Store `body` as `holder`'s entry `id` aimed at `q`, the way
    /// `tests/decisions_precedence_test.rs` places an entry koto's commands
    /// won't write.
    fn plant(&self, holder: &str, q: &Rec, id: &str, body: &Value) {
        let path = self.aimed_path(holder, q, id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_vec(body).unwrap()).unwrap();
    }

    /// `koto decisions show <holder> <id>`.
    fn show(&self, holder: &str, id: &str) -> Value {
        self.ok(&["decisions", "show", holder, id]).json()
    }

    /// Raise `question` in `name` to `recipient` with the content the
    /// accepted prompt fixture shows stored for it, so a person's answer
    /// can be recorded to it; the question and its prompt header.
    fn raise_for_person(&self, name: &str, question: &str, recipient: &str) -> (Rec, String) {
        let lines = fx::lines("accepted");
        let (shown, labels) = fx::shown(&lines, 1);
        let content = fx::content(&shown, &labels);
        let q = format!("question={question}");
        let t = format!("text=what about {question}?");
        let r = format!("recipient={recipient}");
        let packet = format!("packet={}", fx::packet(&content));
        let out = self
            .ok(&[
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
                "--subject",
                &packet,
                "--by",
                "session",
            ])
            .json();
        let rec = Rec {
            session: name.to_string(),
            id: out["id"].as_str().unwrap().to_string(),
            session_id: out["decision"]["session"]["id"]
                .as_str()
                .unwrap()
                .to_string(),
        };
        let file = self.root.join(format!("content-{name}-{}", rec.id));
        std::fs::write(&file, &content).unwrap();
        let written = self
            .ok(&[
                "decisions",
                "content",
                "write",
                name,
                &rec.id,
                "--file",
                file.to_str().unwrap(),
            ])
            .json();
        let header = written["prompt_header"].as_str().unwrap().to_string();
        (rec, header)
    }

    /// Record `person:owner`'s verified answer to `q` in `in_session` from
    /// the accepted fixture, installed as this thread's harness record
    /// (the caller holds a [`fx::Records`]); its id.
    fn person_answer(&self, q: &Rec, header: &str, in_session: &str) -> String {
        self.person_answer_at(q, header, in_session, "toolu_ZQXMARKERretention01")
    }

    /// [`Env::person_answer`] from harness tool call `call`, so a test can
    /// record more than one: a call position is used once.
    fn person_answer_at(&self, q: &Rec, header: &str, in_session: &str, call: &str) -> String {
        let lines = fx::with_call(&fx::lines("accepted"), call);
        let harness = fx::harness();
        fx::install(&harness, &fx::render(&lines, &harness, &[header]));
        let pointer = format!("{harness}/{call}/1");
        let out = self.run_env(
            &[("CLAUDE_CODE_SESSION_ID", harness.as_str())],
            &[
                "decisions",
                "answer",
                &q.session,
                &q.id,
                "--in",
                in_session,
                "--by",
                "person:owner",
                "--prompt",
                &pointer,
            ],
        );
        assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
        out.json()["id"].as_str().unwrap().to_string()
    }

    /// `q`'s raiser withdraws it, recorded in `in_session`.
    fn withdraw(&self, q: &Rec, in_session: &str, reason: &str) {
        self.ok(&[
            "decisions",
            "answer",
            &q.session,
            &q.id,
            "--in",
            in_session,
            "--outcome",
            "withdrawn",
            "--reason",
            reason,
            "--by",
            "session",
        ]);
    }

    /// `q`'s raiser settles it from `--evidence event:<raiser>:1`, recorded
    /// in `in_session`: the evidence a question waiting on a session or
    /// anchor takes.
    fn settle_q(&self, q: &Rec, in_session: &str) {
        let evidence = format!("event:{}:1", q.session);
        self.ok(&[
            "decisions",
            "answer",
            &q.session,
            &q.id,
            "--in",
            in_session,
            "--outcome",
            "settled",
            "--text",
            "settled",
            "--evidence",
            &evidence,
            "--by",
            "session",
        ]);
    }

    /// Reverse `q` from `in_session`.
    fn reverse(&self, q: &Rec, in_session: &str) {
        self.ok(&[
            "decisions",
            "reverse",
            &q.session,
            &q.id,
            "--in",
            in_session,
            "--by",
            "session",
        ]);
    }

    fn open(&self, person: &str) -> Vec<Value> {
        let out = self.ok(&["decisions", "open", "--person", person]).json();
        out["questions"].as_array().unwrap().clone()
    }

    fn at(&self, position: &str) -> Vec<Value> {
        let out = self.ok(&["decisions", "open", "--at", position]).json();
        out["questions"].as_array().unwrap().clone()
    }

    /// Whether `koto decisions open --at <raiser>` lists `q` as raised there.
    fn listed_as_raised(&self, q: &Rec) -> bool {
        self.at(&q.session)
            .iter()
            .any(|e| e["identity"] == q.identity().as_str() && e["relation"] == "raised")
    }

    /// The terminal tick: `koto next <name>` with the evidence that ends the
    /// workflow, without `--no-cleanup`.
    fn finish(&self, name: &str) -> Value {
        self.finish_env(&[], name)
    }

    fn finish_env(&self, envs: &[(&str, &str)], name: &str) -> Value {
        let out = self.ok_env(envs, &["next", name, "--with-data", r#"{"go":"go"}"#]);
        let v = out.json();
        assert_eq!(v["action"], "done", "{v}");
        v
    }

    fn exists(&self, name: &str) -> bool {
        self.sessions().join(name).is_dir()
    }

    fn hierarchy(&self, name: &str) -> Value {
        self.ok(&["session", "hierarchy", name, "--json"]).json()
    }

    fn prune(&self, args: &[&str]) -> Value {
        let mut all = vec!["session", "prune"];
        all.extend_from_slice(args);
        self.ok_env(&FAST, &all).json()
    }

    fn cleanup(&self, name: &str, extra: &[&str]) -> Out {
        let mut args = vec!["session", "cleanup", name];
        args.extend_from_slice(extra);
        self.run(&args)
    }

    /// Every file under the scratch root (the sessions directory, the koto
    /// home, the workspace) with its bytes, lock files aside.
    fn snapshot_all(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        snapshot_dir(&self.root)
    }

    /// [`Env::snapshot_all`] less session `removed`'s directory and the
    /// wakes directory: what a forced removal of `removed` must leave
    /// byte-identical.
    fn snapshot_except(&self, removed: &str) -> BTreeMap<PathBuf, Vec<u8>> {
        let gone = Path::new("sessions").join(removed);
        let wakes = Path::new("home").join("wakes");
        self.snapshot_all()
            .into_iter()
            .filter(|(p, _)| !p.starts_with(&gone) && !p.starts_with(&wakes))
            .collect()
    }

    /// Make session `name`'s `ctx/koto/decisions` unreadable; `false`, with
    /// the mode put back, when it can still be listed (the tests run as
    /// root), so the caller skips.
    fn lock_decisions(&self, name: &str) -> bool {
        use std::os::unix::fs::PermissionsExt;
        let dir = self.decisions_dir(name);
        assert!(dir.is_dir(), "{} has no decisions", name);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read_dir(&dir).is_ok() {
            self.unlock_decisions(name);
            eprintln!("skipped: a mode-000 directory is still readable here (root?)");
            return false;
        }
        true
    }

    fn unlock_decisions(&self, name: &str) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            self.decisions_dir(name),
            std::fs::Permissions::from_mode(0o755),
        );
    }

    fn decisions_dir(&self, name: &str) -> PathBuf {
        self.sessions().join(name).join("ctx/koto/decisions")
    }

    /// `koto init <name> --replace-terminal`.
    fn replace(&self, name: &str) -> Out {
        let template = self.template();
        self.run_env(
            &FAST,
            &[
                "init",
                name,
                "--template",
                template.as_str(),
                "--replace-terminal",
            ],
        )
    }

    fn aimed_files(&self, name: &str, target_id: &str) -> Vec<String> {
        let dir = self
            .sessions()
            .join(name)
            .join("ctx/koto/aimed")
            .join(target_id);
        match std::fs::read_dir(dir) {
            Ok(rd) => rd
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect(),
            Err(_) => Vec::new(),
        }
    }
}

fn snapshot_dir(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
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
    walk(dir, dir, &mut out);
    out
}

fn session(name: &str) -> Value {
    json!({"typed": true, "kind": "session", "name": name})
}

fn person(handle: &str) -> Value {
    json!({"typed": true, "kind": "person", "handle": handle})
}

/// The sorted `name` of each entry of a JSON array.
fn names_of(list: &Value) -> Vec<String> {
    let mut out: Vec<String> = list
        .as_array()
        .unwrap_or_else(|| panic!("not an array: {list}"))
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_string())
        .collect();
    out.sort();
    out
}

fn identities(qs: &[Value]) -> Vec<String> {
    qs.iter()
        .map(|q| q["identity"].as_str().unwrap().to_string())
        .collect()
}

/// The `joined_by` vias of every member of a hierarchy report, sorted.
fn vias(h: &Value) -> Vec<String> {
    let mut out: Vec<String> = h["members"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|m| m["joined_by"].as_array().unwrap().clone())
        .map(|e| e["via"].as_str().unwrap().to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The `kept` entry for `name` in a prune preview.
fn kept_entry<'a>(preview: &'a Value, name: &str) -> &'a Value {
    preview["kept"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["name"] == name)
        .unwrap_or_else(|| panic!("{name} isn't kept: {preview:#}"))
}

/// Assert `out` is the `holds_question` refusal of `name` for `holds`, each
/// a question identity with what is held of it.
fn assert_holds_refusal(out: &Out, name: &str, holds: &[(&str, &str)]) {
    assert_eq!(out.code, 2, "{}\n{}", out.stdout, out.stderr);
    let err = &out.json()["error"];
    assert_eq!(err["code"], "holds_question", "{err}");
    assert_eq!(err["session"], name);
    let force = format!("koto session cleanup {name} --force");
    assert_eq!(err["force_command"], force.as_str());
    let message = err["message"].as_str().unwrap();
    assert!(message.contains(&force), "{message}");
    let expected: Vec<Value> = holds
        .iter()
        .map(|(q, held)| json!({"question": q, "held": held}))
        .collect();
    assert_eq!(err["questions"], Value::Array(expected));
    for (q, held) in holds {
        assert!(message.contains(q), "{message}");
        let says = match *held {
            "raised" => "raised the open question",
            "answer" => "holds the answer in effect",
            "pass_up" => "holds a pass-up",
            "person_answer" => "holds the verified person answer",
            other => panic!("unknown hold {other}"),
        };
        assert!(message.contains(says), "{message}");
    }
}

/// Assert `out` is `koto init <name> --replace-terminal` refused with
/// `holds_question` for `holds`, in the shape every `koto init` refusal
/// has: `error` the text, `command: init`, `code` beside it.
fn assert_replace_refusal(out: &Out, name: &str, holds: &[(&str, &str)]) {
    assert_eq!(out.code, 2, "{}\n{}", out.stdout, out.stderr);
    let v = out.json();
    assert_eq!(v["code"], "holds_question", "{v}");
    assert_eq!(v["command"], "init", "{v}");
    assert_eq!(v["session"], name, "{v}");
    let force = format!("koto session cleanup {name} --force");
    assert_eq!(v["force_command"], force.as_str(), "{v}");
    let message = v["error"].as_str().unwrap_or_else(|| panic!("{v}"));
    assert!(message.contains(&force), "{message}");
    assert!(message.contains("koto init again"), "{message}");
    let expected: Vec<Value> = holds
        .iter()
        .map(|(q, held)| json!({"question": q, "held": held}))
        .collect();
    assert_eq!(v["questions"], Value::Array(expected), "{v}");
    for (q, _) in holds {
        assert!(message.contains(q), "{message}");
    }
}

/// Assert `out` is the `holds_question` refusal of `name` for decisions
/// that can't be read, the first of them in session `unreadable_in`.
fn assert_unreadable_refusal(out: &Out, name: &str, unreadable_in: &str) {
    assert_eq!(out.code, 2, "{}\n{}", out.stdout, out.stderr);
    let err = &out.json()["error"];
    assert_eq!(err["code"], "holds_question", "{err}");
    assert_eq!(err["session"], name);
    assert_eq!(
        err["force_command"],
        format!("koto session cleanup {name} --force").as_str()
    );
    let listed = err["decisions_unreadable"]
        .as_array()
        .unwrap_or_else(|| panic!("{err}"));
    assert!(!listed.is_empty(), "{err}");
    let first = listed[0].as_str().unwrap();
    assert!(
        first.contains(&format!("'{unreadable_in}'")) && first.contains("can't be"),
        "{first}"
    );
    let message = err["message"].as_str().unwrap();
    assert!(message.contains("can't be read"), "{message}");
    assert!(message.contains(first), "{message}");
}

/// The `questions` entry for a session holding `answer` (`<holder session
/// id>/<answer id>`), a verified person answer to `q`.
fn person_hold(q: &Rec, answer: &str) -> Value {
    json!({"question": q.identity(), "held": "person_answer", "answer": answer})
}

/// `<session id>/<id>` of the decision `id` that `holder` holds.
fn answer_identity(env: &Env, holder: &str, id: &str) -> String {
    let shown = env.show(holder, id);
    format!(
        "{}/{}",
        shown["decision"]["session"]["id"].as_str().unwrap(),
        id
    )
}

/// Assert `out` is the `holds_question` refusal of `name` holding only the
/// verified person answer `answer` to `q`, as `koto session cleanup` and
/// `koto cancel --cleanup` print it (`error` an object) or, with
/// `replace`, as `koto init --replace-terminal` does (`error` the text).
fn assert_person_refusal(out: &Out, name: &str, q: &Rec, answer: &str, replace: bool) {
    assert_eq!(out.code, 2, "{}\n{}", out.stdout, out.stderr);
    let v = out.json();
    let (body, message) = if replace {
        assert_eq!(v["command"], "init", "{v}");
        (&v, v["error"].as_str().unwrap_or_else(|| panic!("{v}")))
    } else {
        let err = &v["error"];
        (
            err,
            err["message"].as_str().unwrap_or_else(|| panic!("{v}")),
        )
    };
    assert_eq!(body["code"], "holds_question", "{v}");
    assert_eq!(body["session"], name, "{v}");
    let force = format!("koto session cleanup {name} --force");
    assert_eq!(body["force_command"], force.as_str(), "{v}");
    assert_eq!(body["questions"], json!([person_hold(q, answer)]), "{v}");
    assert!(body.get("decisions_unreadable").is_none(), "{v}");
    for part in [
        force.as_str(),
        q.identity().as_str(),
        answer,
        "holds the verified person answer",
        "A verified person answer is kept until its session is removed",
    ] {
        assert!(message.contains(part), "{part}: {message}");
    }
}

/// `a` raises a question to `person:owner`, `b` records the person's
/// answer to it, verified on this host, and `a` (which holds nothing once
/// its question is answered) is removed without `--force`. Returns the
/// question and the answer's `<session id>/<id>`.
fn verified_without_raiser(env: &Env) -> (Rec, String) {
    env.init("a", &[]);
    env.init("b", &[]);
    let (q, header) = env.raise_for_person("a", "q1", "person:owner");
    let id = env.person_answer(&q, &header, "b");
    assert_eq!(env.show("b", &id)["reading"], "verified");
    let out = env.cleanup("a", &[]);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    assert!(!env.exists("a"));
    (q, answer_identity(env, "b", &id))
}

// ===== Kept at the terminal tick =====

#[test]
fn an_unlinked_raiser_is_kept_at_its_terminal_tick_for_its_open_question() {
    let env = Env::new();
    env.init("a", &[]);
    let q = env.escalate("a", "q1", "person:owner");

    let end = env.finish("a");
    assert_eq!(
        end["retention"],
        json!({
            "retained": true,
            "reason": "open_question",
            "open_questions": [q.identity()],
        })
    );
    assert!(env.exists("a"));
    assert_eq!(identities(&env.open("owner")), vec![q.identity()]);
}

#[test]
fn a_session_with_no_open_question_is_removed_at_its_terminal_tick() {
    let env = Env::new();

    // No decision at all.
    env.init("plain", &[]);
    assert_eq!(env.finish("plain")["retention"], json!({"retained": false}));
    assert!(!env.exists("plain"));

    // A decision, but not a question.
    env.init("judged", &[]);
    env.ok(&[
        "decisions",
        "record",
        "judged",
        "--kind",
        "judgment",
        "--outcome",
        "settled",
        "--subject",
        "what=x",
        "--by",
        "session",
    ]);
    assert_eq!(
        env.finish("judged")["retention"],
        json!({"retained": false})
    );
    assert!(!env.exists("judged"));

    // An escalation whose recipient is untyped.
    env.init("untyped", &[]);
    env.escalate("untyped", "q0", "coord");
    assert_eq!(
        env.finish("untyped")["retention"],
        json!({"retained": false})
    );
    assert!(!env.exists("untyped"));

    // A question whose escalate was reversed.
    env.init("reversed", &[]);
    let q = env.escalate("reversed", "q1", "person:owner");
    env.reverse(&q, "reversed");
    assert!(env.open("owner").is_empty());
    assert_eq!(
        env.finish("reversed")["retention"],
        json!({"retained": false})
    );
    assert!(!env.exists("reversed"));
}

#[test]
fn the_terminal_tick_keeps_a_raiser_exactly_when_the_open_read_lists_its_question() {
    let env = Env::new();
    env.init("b", &[]);
    env.init("c", &[]);

    // Each point gets its own raiser, since a session ends only once.
    let check = |q: &Rec, expect_open: bool| {
        let listed = env.listed_as_raised(q);
        assert_eq!(listed, expect_open, "open --at {}", q.session);
        let end = env.finish(&q.session);
        let kept_for_question = end["retention"]["reason"] == "open_question";
        assert_eq!(
            kept_for_question, listed,
            "{}: open read lists it: {listed}; retention: {}",
            q.session, end["retention"]
        );
        if kept_for_question {
            assert_eq!(end["retention"]["open_questions"], json!([q.identity()]));
        } else {
            assert!(end["retention"].get("open_questions").is_none());
        }
    };

    // Raised.
    env.init("raised", &[]);
    let q = env.escalate("raised", "q1", "session:b");
    check(&q, true);

    // Passed up.
    env.init("passed", &[]);
    let q = env.escalate("passed", "q1", "session:b");
    env.readdress(&q, "b", "person:owner");
    check(&q, true);

    // Answered.
    env.init("answered", &[]);
    let q = env.escalate("answered", "q1", "session:c");
    env.answer(&q, "c", "yes");
    check(&q, false);

    // Reversed.
    env.init("reversed", &[]);
    let q = env.escalate("reversed", "q1", "person:owner");
    env.reverse(&q, "reversed");
    check(&q, false);
}

#[test]
fn an_answer_or_pass_up_aimed_at_a_question_joins_holder_and_raiser() {
    let env = Env::new();

    // An answer: holder and raiser are each kept with `hierarchy`.
    env.init("asker", &[]);
    env.init("answerer", &[]);
    let q = env.escalate("asker", "q1", "session:answerer");
    env.answer(&q, "answerer", "yes");
    assert_eq!(
        env.finish("answerer")["retention"],
        json!({"retained": true, "reason": "hierarchy"})
    );
    assert_eq!(
        env.finish("asker")["retention"],
        json!({"retained": true, "reason": "hierarchy"})
    );
    assert!(env.exists("asker") && env.exists("answerer"));
    for name in ["asker", "answerer"] {
        let h = env.hierarchy(name);
        assert_eq!(names_of(&h["members"]), vec!["answerer", "asker"], "{h:#}");
        assert_eq!(vias(&h), vec!["answer aimed at its question"], "{h:#}");
    }

    // A pass-up: its holder is kept with `hierarchy`, and the raiser, whose
    // question is still open, with `open_question`.
    env.init("raiser", &[]);
    env.init("passer", &[]);
    let q = env.escalate("raiser", "q1", "session:passer");
    env.readdress(&q, "passer", "person:owner");
    assert_eq!(
        env.finish("passer")["retention"],
        json!({"retained": true, "reason": "hierarchy"})
    );
    assert_eq!(env.finish("raiser")["retention"]["reason"], "open_question");
    for name in ["raiser", "passer"] {
        let h = env.hierarchy(name);
        assert_eq!(names_of(&h["members"]), vec!["passer", "raiser"], "{h:#}");
        assert_eq!(vias(&h), vec!["pass-up aimed at its question"], "{h:#}");
    }
}

#[test]
fn a_terminal_raiser_kept_for_its_question_still_gets_the_answer() {
    let env = Env::new();
    env.init("a", &[]);
    env.init("b", &[]);
    let q = env.escalate("a", "q1", "session:b");
    assert_eq!(env.finish("a")["retention"]["reason"], "open_question");
    env.answer(&q, "b", "SQLite");

    let next = env.ok(&["next", "a"]).json();
    let qs = next["questions"]
        .as_array()
        .unwrap_or_else(|| panic!("{next}"));
    assert_eq!(qs.len(), 1, "{next}");
    assert_eq!(qs[0]["identity"], q.identity().as_str());
    assert_eq!(qs[0]["state"], "answered");
    assert_eq!(qs[0]["answer"]["text"], "SQLite");
}

// ===== Kept by the prunes =====

/// `raiser` raised a question to `passer`, which passed it up to a person:
/// both are terminal, the question is open, and the retention has passed.
fn held_pair(env: &Env) -> Rec {
    env.init("raiser", &[]);
    env.init("passer", &[]);
    let q = env.escalate("raiser", "q1", "session:passer");
    env.readdress(&q, "passer", "person:owner");
    env.finish_env(&FAST, "raiser");
    env.finish_env(&FAST, "passer");
    settle();
    q
}

#[test]
fn a_hierarchy_holding_an_open_question_is_kept_by_every_prune() {
    let env = Env::new();
    let q = held_pair(&env);
    let id = q.identity();

    for member in ["raiser", "passer"] {
        let h = env
            .ok_env(&FAST, &["session", "hierarchy", member, "--json"])
            .json();
        assert_eq!(h["kept"], true, "{h:#}");
        let keepers = h["keepers"].as_array().unwrap();
        assert_eq!(keepers.len(), 1, "{h:#}");
        assert_eq!(keepers[0]["name"], "raiser");
        assert_eq!(keepers[0]["alive"], false);
        assert_eq!(keepers[0]["open_questions"], json!([id]));
    }

    let preview = env.prune(&[]);
    assert!(
        preview["remove"].as_array().unwrap().is_empty(),
        "{preview:#}"
    );
    assert_eq!(names_of(&preview["kept"]), vec!["passer", "raiser"]);
    for member in ["raiser", "passer"] {
        let detail = kept_entry(&preview, member)["detail"].as_str().unwrap();
        assert!(
            detail.contains(&format!("holds open question {id}")),
            "{member}: {detail}"
        );
    }

    let applied = env.prune(&["--apply", "--all"]);
    assert_eq!(applied["removed"], json!([]), "{applied:#}");
    assert_eq!(names_of(&applied["kept"]), vec!["passer", "raiser"]);
    assert!(env.exists("raiser") && env.exists("passer"));

    for root in ["raiser", "passer"] {
        let out = env.run_env(&FAST, &["workspace", "prune", "--root", root, "--yes"]);
        assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
        let report = out.last_json();
        assert_eq!(report["removed"], json!([]), "{report:#}");
        assert_eq!(report["pruned"], false);
        assert!(env.exists("raiser") && env.exists("passer"));

        let out = env.run_input(
            &FAST,
            &["workspace", "prune", "--root", root, "--yes", "--force"],
            Some("force-prune\n"),
        );
        assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
        let report = out.last_json();
        assert_eq!(report["removed"], json!([]), "{report:#}");
        assert!(env.exists("raiser") && env.exists("passer"));
    }
    assert_eq!(identities(&env.open("owner")), vec![id.clone()]);

    // A same-name replace is refused: moving the raiser aside would rename
    // it, and its question would stop reading.
    let before = env.snapshot_all();
    let out = env.replace("raiser");
    assert_replace_refusal(&out, "raiser", &[(&id, "raised")]);
    assert_eq!(env.snapshot_all(), before);
    assert_eq!(identities(&env.open("owner")), vec![id.clone()]);
}

#[test]
fn the_hold_ends_with_the_question() {
    let env = Env::new();
    let q = held_pair(&env);
    // A session never answers a question waiting on a person: the raiser
    // ends it by withdrawing it.
    env.withdraw(&q, "raiser", "not needed");

    let preview = env.prune(&[]);
    assert_eq!(
        names_of(&preview["remove"]),
        vec!["passer", "raiser"],
        "{preview:#}"
    );
    assert!(
        preview["kept"].as_array().unwrap().is_empty(),
        "{preview:#}"
    );

    env.prune(&["--apply", "--all"]);
    for name in ["passer", "raiser"] {
        assert!(!env.exists(name), "{name} is still there");
    }
}

/// `w`, dispatched by `coord`; with `reverse`, `w` records a question
/// that `coord` reverses, so `coord` holds a reversal aimed at `w`. Both
/// end; returns each terminal retention, the hierarchy's vias and the
/// prune preview's removed names once the retention has passed.
fn reversal_world(reverse: bool) -> (Value, Value, Vec<String>, Vec<String>) {
    let env = Env::new();
    env.init("coord", &[]);
    env.init("w", &["--dispatched-by", "coord"]);
    if reverse {
        let q = env.escalate("w", "q1", "person:owner");
        env.reverse(&q, "coord");
        assert_eq!(env.aimed_files("coord", &q.session_id).len(), 1);
    }
    let w_end = env.finish_env(&FAST, "w")["retention"].clone();
    let coord_end = env.finish_env(&FAST, "coord")["retention"].clone();
    let h = env.hierarchy("coord");
    settle();
    let preview = env.prune(&[]);
    (w_end, coord_end, vias(&h), names_of(&preview["remove"]))
}

#[test]
fn a_reversal_aimed_elsewhere_keeps_its_holder_exactly_as_before() {
    let with_reversal = reversal_world(true);
    let without = reversal_world(false);
    assert_eq!(with_reversal, without);
    let (w_end, coord_end, vias, removed) = with_reversal;
    assert_eq!(w_end, json!({"retained": true, "reason": "hierarchy"}));
    assert_eq!(coord_end, json!({"retained": true, "reason": "hierarchy"}));
    // The reversal adds no edge: only the dispatched-by link joins them.
    assert_eq!(vias, vec!["dispatched-by link"]);
    assert_eq!(removed, vec!["coord", "w"]);
}

// ===== Explicit removal =====

#[test]
fn cleanup_refuses_a_raiser_and_changes_nothing() {
    let env = Env::new();
    env.init("a", &[]);
    let q = env.escalate("a", "q1", "person:owner");
    let before = env.snapshot_all();

    let out = env.cleanup("a", &[]);
    assert_holds_refusal(&out, "a", &[(&q.identity(), "raised")]);
    assert_eq!(env.snapshot_all(), before);

    // And after its terminal tick kept it.
    env.finish("a");
    let before = env.snapshot_all();
    let out = env.cleanup("a", &[]);
    assert_holds_refusal(&out, "a", &[(&q.identity(), "raised")]);
    assert_eq!(env.snapshot_all(), before);
}

#[test]
fn cleanup_refuses_the_holder_of_the_answer_in_effect_or_a_resting_pass_up() {
    let env = Env::new();
    env.init("a", &[]);
    env.init("answerer", &[]);
    let answered = env.escalate("a", "q1", "session:answerer");
    env.answer(&answered, "answerer", "yes");
    let before = env.snapshot_all();
    let out = env.cleanup("answerer", &[]);
    assert_holds_refusal(&out, "answerer", &[(&answered.identity(), "answer")]);
    assert_eq!(env.snapshot_all(), before);

    env.init("r", &[]);
    env.init("passer", &[]);
    let passed = env.escalate("r", "q1", "session:passer");
    env.readdress(&passed, "passer", "person:owner");
    let before = env.snapshot_all();
    let out = env.cleanup("passer", &[]);
    assert_holds_refusal(&out, "passer", &[(&passed.identity(), "pass_up")]);
    assert_eq!(env.snapshot_all(), before);
}

#[test]
fn cancel_cleanup_is_refused_before_the_cancel() {
    let env = Env::new();
    env.init("a", &[]);
    env.init("answerer", &[]);
    env.init("r", &[]);
    env.init("passer", &[]);
    let q = env.escalate("a", "q1", "person:owner");
    let answered = env.escalate("a", "q2", "session:answerer");
    env.answer(&answered, "answerer", "yes");
    let passed = env.escalate("r", "q1", "session:passer");
    env.readdress(&passed, "passer", "person:owner");

    let cases = [
        ("a", q.identity(), "raised"),
        ("answerer", answered.identity(), "answer"),
        ("passer", passed.identity(), "pass_up"),
    ];
    for (name, identity, held) in &cases {
        let before = env.snapshot_all();
        let out = env.run(&["cancel", name, "--cleanup"]);
        assert_holds_refusal(&out, name, &[(identity, held)]);
        assert_eq!(env.snapshot_all(), before, "the store changed for {name}");
        // Not cancelled: it still advances.
        let next = env.ok(&["next", name, "--no-cleanup"]).json();
        assert_eq!(next["state"], "work", "{name}: {next}");
    }
}

#[test]
fn the_refusal_follows_the_raisers_id_not_its_name() {
    let env = Env::new();
    env.init("a", &[]);
    env.init("passer", &[]);
    env.init("answerer", &[]);
    let passed = env.escalate("a", "q1", "session:passer");
    env.readdress(&passed, "passer", "person:owner");
    let answered = env.escalate("a", "q2", "session:answerer");
    env.answer(&answered, "answerer", "yes");

    let out = env.cleanup("a", &["--force"]);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    env.init("a", &[]);

    // The new `a` is joined to neither holder and is removed at its end.
    let h = env.hierarchy("a");
    assert_eq!(names_of(&h["members"]), vec!["a"], "{h:#}");
    assert_eq!(env.finish("a")["retention"], json!({"retained": false}));
    assert!(!env.exists("a"));

    // Neither holder refuses cleanup.
    for name in ["passer", "answerer"] {
        let out = env.cleanup(name, &[]);
        assert_eq!(out.code, 0, "{name}\n{}\n{}", out.stdout, out.stderr);
        assert!(out.json().get("orphaned_questions").is_none());
        assert!(!env.exists(name));
    }
}

#[test]
fn holds_that_no_open_question_rests_on_are_removed_without_force() {
    let env = Env::new();

    // The raiser of an answered question, then its answerer, whose answer
    // now aims at a session no longer in the store. (An answer that isn't
    // the one in effect is the `counted: false` and outranked cases below.)
    env.init("a", &[]);
    env.init("first", &[]);
    let q = env.escalate("a", "q1", "session:first");
    env.answer(&q, "first", "yes");
    let out = env.cleanup("a", &[]);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    let out = env.cleanup("first", &[]);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);

    // A pass-up of a question since answered: `passer` passes it up to its
    // dispatcher, which answers it.
    env.init("r", &[]);
    env.init("answerer", &[]);
    env.init("passer", &["--dispatched-by", "answerer"]);
    let q = env.escalate("r", "q1", "session:passer");
    env.readdress(&q, "passer", "session:answerer");
    env.answer(&q, "answerer", "yes");
    let out = env.cleanup("passer", &[]);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);

    for name in ["a", "first", "passer"] {
        assert!(!env.exists(name), "{name} is still there");
    }
}

#[test]
fn a_forced_removal_of_the_answer_reopens_the_question() {
    let env = Env::new();
    env.init("a", &[]);
    env.init("b", &[]);
    let q = env.escalate("a", "q1", "session:b");
    env.answer(&q, "b", "yes");
    assert!(!env.listed_as_raised(&q));

    let before = env.snapshot_except("b");
    let out = env.cleanup("b", &["--force"]);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    assert_eq!(env.snapshot_except("b"), before);
    let v = out.json();
    assert_eq!(
        v["orphaned_questions"],
        json!([{"question": q.identity(), "held": "answer"}])
    );
    assert!(v["live_members"].is_array(), "{v}");
    assert!(
        out.stderr.contains("with --force") && out.stderr.contains(&q.identity()),
        "{}",
        out.stderr
    );
    assert!(!env.exists("b"));

    assert!(env.listed_as_raised(&q));
    let next = env.ok(&["next", "a", "--no-cleanup"]).json();
    assert!(next.get("questions").is_none(), "{next}");
    assert_eq!(
        env.finish("a")["retention"],
        json!({
            "retained": true,
            "reason": "open_question",
            "open_questions": [q.identity()],
        })
    );
}

#[test]
fn a_forced_removal_of_a_pass_up_returns_the_question_to_its_earlier_addressee() {
    let env = Env::new();
    env.init("a", &[]);
    env.init("passer", &[]);
    let q = env.escalate("a", "q1", "session:passer");
    env.readdress(&q, "passer", "person:owner");
    let before = env.open("owner");
    assert_eq!(before.len(), 1);
    assert_eq!(
        before[0]["addressees"],
        json!([session("passer"), person("owner")])
    );

    let before = env.snapshot_except("passer");
    let out = env.cleanup("passer", &["--force"]);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    assert_eq!(env.snapshot_except("passer"), before);
    assert_eq!(
        out.json()["orphaned_questions"],
        json!([{"question": q.identity(), "held": "pass_up"}])
    );

    assert!(env.open("owner").is_empty());
    let at = env.at("a");
    assert_eq!(at.len(), 1, "{at:?}");
    assert_eq!(at[0]["identity"], q.identity().as_str());
    assert_eq!(at[0]["addressee"], session("passer"));
    assert_eq!(at[0]["addressees"], json!([session("passer")]));
}

#[test]
fn a_forced_removal_of_the_raiser_leaves_the_aimed_entries_listed() {
    let env = Env::new();
    env.init("a", &[]);
    env.init("passer", &[]);
    let q = env.escalate("a", "q1", "session:passer");
    env.readdress(&q, "passer", "person:owner");

    let before = env.snapshot_except("a");
    let out = env.cleanup("a", &["--force"]);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    assert_eq!(env.snapshot_except("a"), before);
    assert_eq!(
        out.json()["orphaned_questions"],
        json!([{"question": q.identity(), "held": "raised"}])
    );
    assert!(!env.exists("a"));

    assert!(env.open("owner").is_empty());
    assert!(env.at("passer").is_empty());
    let listing = env
        .ok(&["decisions", "list", "passer", "--format", "v1"])
        .json();
    let entries = listing["decisions"].as_array().unwrap();
    assert!(
        entries.iter().any(|e| e["decision"]["kind"] == "readdress"
            && e["decision"]["target"]["id"] == q.session_id.as_str()),
        "{listing:#}"
    );
}

#[test]
fn a_pinned_session_is_refused_for_its_pin_with_or_without_force() {
    let env = Env::new();
    env.init("a", &[]);
    env.escalate("a", "q1", "person:owner");
    env.ok(&["session", "pin", "a", "--reason", "x"]);
    let before = env.snapshot_all();
    for extra in [&[][..], &["--force"][..]] {
        let out = env.cleanup("a", extra);
        assert_eq!(out.code, 2, "{extra:?}\n{}\n{}", out.stdout, out.stderr);
        assert_eq!(out.json()["error"]["code"], "pinned", "{}", out.stdout);
        assert_eq!(env.snapshot_all(), before);
    }
}

#[test]
fn a_pinned_holder_of_an_answer_or_a_pass_up_is_refused_for_its_pin_even_with_force() {
    let env = Env::new();
    env.init("a", &[]);
    env.init("answerer", &[]);
    env.init("r", &[]);
    env.init("passer", &[]);
    let answered = env.escalate("a", "q1", "session:answerer");
    env.answer(&answered, "answerer", "yes");
    let passed = env.escalate("r", "q1", "session:passer");
    env.readdress(&passed, "passer", "person:owner");
    for name in ["answerer", "passer"] {
        env.ok(&["session", "pin", name, "--reason", "x"]);
    }

    let before = env.snapshot_all();
    for name in ["answerer", "passer"] {
        for extra in [&[][..], &["--force"][..]] {
            let out = env.cleanup(name, extra);
            assert_eq!(
                out.code, 2,
                "{name} {extra:?}\n{}\n{}",
                out.stdout, out.stderr
            );
            assert_eq!(out.json()["error"]["code"], "pinned", "{}", out.stdout);
            assert_eq!(env.snapshot_all(), before, "{name} {extra:?}");
        }
        // `koto cancel --cleanup` too, before anything is cancelled.
        let out = env.run(&["cancel", name, "--cleanup"]);
        assert_eq!(out.code, 2, "{name}\n{}\n{}", out.stdout, out.stderr);
        assert_eq!(out.json()["error"]["code"], "pinned", "{}", out.stdout);
        assert_eq!(env.snapshot_all(), before, "{name} cancel");
    }
    assert!(env
        .open("owner")
        .iter()
        .any(|q| q["identity"] == passed.identity().as_str()));
}

#[test]
fn cleanup_of_a_session_holding_no_question_prints_what_it_did_before() {
    let env = Env::new();
    for extra in [&[][..], &["--force"][..]] {
        env.init("a", &[]);
        let out = env.cleanup("a", extra);
        assert_eq!(out.code, 0, "{extra:?}\n{}\n{}", out.stdout, out.stderr);
        let v = out.json();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["children", "cleaned_up", "live_members", "name"],
            "{extra:?}: {v}"
        );
        assert_eq!(
            v,
            json!({"name": "a", "cleaned_up": true, "children": [], "live_members": []}),
            "{extra:?}"
        );
        assert!(!env.exists("a"), "{extra:?}");
    }
}

#[test]
fn force_leaves_children_hierarchy_members_and_the_raiser_byte_identical() {
    let env = Env::new();
    env.init("coord", &[]);
    env.init("b", &["--dispatched-by", "coord"]);
    env.init("child", &["--parent", "b"]);
    env.init("a", &[]);
    let q = env.escalate("a", "q1", "session:b");
    env.answer(&q, "b", "yes");

    let before = env.snapshot_except("b");
    let out = env.cleanup("b", &["--force"]);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    assert_eq!(
        out.json()["orphaned_questions"],
        json!([{"question": q.identity(), "held": "answer"}])
    );
    assert!(!env.exists("b"));
    // The child, the dispatcher and the raiser: every byte as it was.
    assert_eq!(env.snapshot_except("b"), before);
    for name in ["coord", "child", "a"] {
        assert!(env.exists(name), "{name} is gone");
    }
}

// ===== Replacing a finished holder =====

#[test]
fn a_replace_over_a_finished_question_holder_is_refused_and_moves_nothing() {
    let env = Env::new();
    // A raiser, kept at its end for its open question.
    env.init("a", &[]);
    let raised = env.escalate("a", "q1", "person:owner");
    assert_eq!(env.finish("a")["retention"]["reason"], "open_question");
    // A pass-up holder, kept with the raiser's hierarchy.
    env.init("r", &[]);
    env.init("passer", &[]);
    let passed = env.escalate("r", "q1", "session:passer");
    env.readdress(&passed, "passer", "person:owner");
    env.finish("passer");
    // The holder of the answer in effect.
    env.init("asker", &[]);
    env.init("answerer", &[]);
    let answered = env.escalate("asker", "q1", "session:answerer");
    env.answer(&answered, "answerer", "yes");
    env.finish("answerer");

    let cases = [
        ("a", raised.identity(), "raised"),
        ("passer", passed.identity(), "pass_up"),
        ("answerer", answered.identity(), "answer"),
    ];
    for (name, identity, held) in &cases {
        let before = env.snapshot_all();
        let out = env.replace(name);
        assert_replace_refusal(&out, name, &[(identity, held)]);
        assert_eq!(env.snapshot_all(), before, "the store changed for {name}");
        assert!(!env.exists(&format!("{name}_r1")), "{name} was moved aside");
    }
    // Each question reads as it did.
    let open = identities(&env.open("owner"));
    assert_eq!(open, vec![raised.identity(), passed.identity()]);
    assert!(!env.listed_as_raised(&answered));

    // The way out the message names: remove it with --force, then init.
    env.ok(&["session", "cleanup", "a", "--force"]);
    let out = env.replace("a");
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    assert_eq!(out.json()["outcome"], "created");
}

// ===== By-rule prunes keep a holder whose raiser stays =====

/// `a` asks `b`, `b` answers; both end and the retention passes.
fn answered_pair(env: &Env) -> Rec {
    env.init("a", &[]);
    env.init("b", &[]);
    let q = env.escalate("a", "q1", "session:b");
    env.answer(&q, "b", "yes");
    env.finish_env(&FAST, "a");
    env.finish_env(&FAST, "b");
    settle();
    q
}

/// Assert `detail` keeps a holder of `q`'s answer for its raiser `a`.
fn assert_kept_for_raiser(detail: &str, q: &Rec) {
    assert!(
        detail.contains(&q.identity())
            && detail.contains("holds the answer in effect")
            && detail.contains("its raiser a isn't removed with it"),
        "{detail}"
    );
}

#[test]
fn workspace_prune_of_the_answerer_alone_keeps_it_while_the_asker_stays() {
    let env = Env::new();
    let q = answered_pair(&env);

    let out = env.run_env(&FAST, &["workspace", "prune", "--root", "b", "--yes"]);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    let report = out.last_json();
    assert_eq!(report["removed"], json!([]), "{report:#}");
    assert_eq!(report["pruned"], false, "{report:#}");
    let kept = report["kept"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["name"] == "b")
        .unwrap_or_else(|| panic!("{report:#}"));
    assert_kept_for_raiser(kept["reason"].as_str().unwrap(), &q);
    assert!(env.exists("a") && env.exists("b"));
    // The question is still answered.
    assert!(!env.listed_as_raised(&q));
}

#[test]
fn workspace_prune_removes_asker_and_answerer_together() {
    let env = Env::new();
    env.init("a", &[]);
    env.init("b", &["--parent", "a"]);
    let q = env.escalate("a", "q1", "session:b");
    env.answer(&q, "b", "yes");
    env.finish_env(&FAST, "b");
    env.finish_env(&FAST, "a");
    settle();

    let out = env.run_env(&FAST, &["workspace", "prune", "--root", "a", "--yes"]);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    let report = out.last_json();
    let mut removed: Vec<String> = report["removed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_string())
        .collect();
    removed.sort();
    assert_eq!(removed, vec!["a", "b"], "{report:#}");
    assert!(!env.exists("a") && !env.exists("b"));
}

#[test]
fn session_prune_older_than_keeps_an_answerer_whose_asker_is_too_young() {
    let env = Env::new();
    env.init("a", &[]);
    env.init("b", &[]);
    let q = env.escalate("a", "q1", "session:b");
    env.answer(&q, "b", "yes");
    env.finish_env(&FAST, "b");
    std::thread::sleep(std::time::Duration::from_secs(3));
    env.finish_env(&FAST, "a");
    settle();

    // `b` stopped over four seconds ago, `a` about two.
    let preview = env.prune(&["--older-than", "4s"]);
    assert!(
        preview["remove"].as_array().unwrap().is_empty(),
        "{preview:#}"
    );
    let kept = kept_entry(&preview, "b");
    assert_eq!(kept["rule"], "h", "{kept}");
    assert_kept_for_raiser(kept["detail"].as_str().unwrap(), &q);

    let applied = env.prune(&["--older-than", "4s", "--apply"]);
    assert_eq!(applied["removed"], json!([]), "{applied:#}");
    assert!(env.exists("a") && env.exists("b"));
    assert!(!env.listed_as_raised(&q));

    // Both old enough: removed together.
    std::thread::sleep(std::time::Duration::from_secs(2));
    let applied = env.prune(&["--older-than", "3s", "--apply"]);
    assert_eq!(names_of(&applied["removed"]), vec!["a", "b"], "{applied:#}");
    assert!(!env.exists("a") && !env.exists("b"));
}

#[test]
fn session_prune_by_execution_dir_keeps_an_answerer_whose_asker_is_elsewhere() {
    let env = Env::new();
    let sub = env.root.join("ws").join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let sub_arg = sub.to_string_lossy().into_owned();
    env.init("a", &[]);
    env.init("b", &["--execution-dir", sub_arg.as_str()]);
    let q = env.escalate("a", "q1", "session:b");
    env.answer(&q, "b", "yes");
    env.finish_env(&FAST, "a");
    let out = env.run_at(
        &sub,
        &FAST,
        &["next", "b", "--with-data", r#"{"go":"go"}"#],
        None,
    );
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    settle();

    let preview = env.prune(&["--execution-dir", sub_arg.as_str()]);
    assert!(
        preview["remove"].as_array().unwrap().is_empty(),
        "{preview:#}"
    );
    assert_kept_for_raiser(kept_entry(&preview, "b")["detail"].as_str().unwrap(), &q);
    let applied = env.prune(&["--execution-dir", sub_arg.as_str(), "--apply"]);
    assert_eq!(applied["removed"], json!([]), "{applied:#}");
    assert!(env.exists("a") && env.exists("b"));
    assert!(!env.listed_as_raised(&q));

    // A directory holding both: removed together.
    let ws = env.root.join("ws").to_string_lossy().into_owned();
    let applied = env.prune(&["--execution-dir", ws.as_str(), "--apply"]);
    assert_eq!(names_of(&applied["removed"]), vec!["a", "b"], "{applied:#}");
    assert!(!env.exists("a") && !env.exists("b"));
}

// ===== What closes a question, and which entry holds it =====

/// Assert a plain `koto session cleanup <name>`, without `--force`, removed
/// it and reported nothing orphaned.
fn assert_cleaned_without_force(env: &Env, name: &str) {
    let out = env.cleanup(name, &[]);
    assert_eq!(out.code, 0, "{name}\n{}\n{}", out.stdout, out.stderr);
    assert!(
        out.json().get("orphaned_questions").is_none(),
        "{}",
        out.stdout
    );
    assert!(!env.exists(name), "{name} is still there");
}

#[test]
fn a_withdrawn_question_is_not_open_and_holds_nothing() {
    let env = Env::new();
    // At the terminal tick.
    env.init("w1", &[]);
    let q = env.escalate("w1", "q1", "person:owner");
    env.withdraw(&q, "w1", "not needed");
    assert!(!env.listed_as_raised(&q));
    assert!(env.open("owner").is_empty());
    assert_eq!(env.finish("w1")["retention"], json!({"retained": false}));
    assert!(!env.exists("w1"));

    // At cleanup.
    env.init("w2", &[]);
    let q = env.escalate("w2", "q1", "person:owner");
    env.withdraw(&q, "w2", "not needed");
    assert_cleaned_without_force(&env, "w2");
}

#[test]
fn a_settled_question_is_not_open_and_holds_nothing() {
    let env = Env::new();
    env.init("b", &[]);
    // At the terminal tick.
    env.init("s1", &[]);
    let q = env.escalate("s1", "q1", "session:b");
    env.settle_q(&q, "s1");
    assert!(!env.listed_as_raised(&q));
    assert_eq!(env.finish("s1")["retention"], json!({"retained": false}));
    assert!(!env.exists("s1"));

    // At cleanup, of the raiser and of the addressee it waited on.
    env.init("s2", &[]);
    let q = env.escalate("s2", "q1", "session:b");
    env.settle_q(&q, "s2");
    assert_cleaned_without_force(&env, "s2");
    assert_cleaned_without_force(&env, "b");
}

#[test]
fn an_answer_that_doesnt_count_holds_nothing() {
    let env = Env::new();
    env.init("a", &[]);
    env.init("b", &[]);
    env.init("x", &[]);
    let q1 = env.escalate("a", "q1", "session:b");
    let q2 = env.escalate("a", "q2", "session:b");
    // `b`'s answer to q2, placed in `x` as an answer to q1 recorded there:
    // `x` isn't q1's addressee, so it doesn't count.
    let id = env.answer(&q2, "b", "planted");
    let mut body = env.stored("b", &q2, &id);
    let judged = env
        .ok(&[
            "decisions",
            "record",
            "x",
            "--kind",
            "judgment",
            "--outcome",
            "settled",
            "--subject",
            "what=x",
            "--by",
            "session",
        ])
        .json();
    body["session"] = judged["decision"]["session"].clone();
    body["id"] = json!("000000000050");
    body["target"]["decision"] = json!(q1.id);
    body["subject"]["id"] = json!(q1.id);
    env.plant("x", &q1, "000000000050", &body);
    let shown = env.show("x", "000000000050");
    assert_eq!(shown["counted"], false, "{shown}");
    assert_eq!(shown["governs"], false, "{shown}");
    assert!(env.listed_as_raised(&q1));

    assert_cleaned_without_force(&env, "x");
    // q1 still waits on `b`; its raiser still holds it.
    assert!(env.listed_as_raised(&q1));
    assert_holds_refusal(&env.cleanup("a", &[]), "a", &[(&q1.identity(), "raised")]);
}

#[test]
fn a_persons_answer_in_effect_is_the_hold_and_the_outranked_answer_is_released() {
    let _records = fx::Records::new();
    let env = Env::new();
    env.init("a", &[]);
    env.init("b", &[]);
    env.init("p", &[]);
    let (q, header) = env.raise_for_person("a", "q1", "session:b");
    let by_session = env.answer(&q, "b", "Postgres");
    let by_person = env.person_answer(&q, &header, "p");
    let s = env.show("b", &by_session);
    assert_eq!(
        (&s["counted"], &s["governs"]),
        (&json!(true), &json!(false))
    );
    assert_eq!(env.show("p", &by_person)["governs"], true);

    // The person's answer is the one in effect and reads verified here:
    // its holder is refused for it, by cleanup and by a replace once it
    // has finished, which keeps it for that answer.
    let answer = answer_identity(&env, "p", &by_person);
    let before = env.snapshot_all();
    let out = env.cleanup("p", &[]);
    assert_person_refusal(&out, "p", &q, &answer, false);
    assert_eq!(env.snapshot_all(), before);
    assert_eq!(
        env.finish("p")["retention"],
        json!({
            "retained": true,
            "reason": "person_answer",
            "answered_questions": [q.identity()],
        })
    );
    let before = env.snapshot_all();
    assert_person_refusal(&env.replace("p"), "p", &q, &answer, true);
    assert_eq!(env.snapshot_all(), before);

    // The outranked session answer holds nothing.
    assert_cleaned_without_force(&env, "b");
    assert!(!env.listed_as_raised(&q));
    assert!(env.exists("p") && env.exists("a"));
}

#[test]
fn a_pass_up_that_doesnt_count_holds_nothing() {
    let env = Env::new();
    env.init("r", &[]);
    env.init("d", &[]);
    env.init("passer", &["--dispatched-by", "d"]);
    let q = env.escalate("r", "q1", "session:passer");
    let id = env.readdress(&q, "passer", "session:d");
    // Edited to a destination other than the dispatcher it recorded: the
    // destination rule leaves it out of the history.
    let mut body = env.stored("passer", &q, &id);
    body["subject"]["recipient"] = json!("session:elsewhere");
    env.plant("passer", &q, &id, &body);
    let shown = env.show("passer", &id);
    assert_eq!(shown["counted"], false, "{shown}");
    assert_eq!(shown["reason"], "destination", "{shown}");
    let at = env.at("r");
    assert_eq!(at.len(), 1, "{at:?}");
    assert_eq!(at[0]["addressees"], json!([session("passer")]));

    assert_cleaned_without_force(&env, "passer");
    assert!(env.listed_as_raised(&q));
}

// ===== A verified person answer keeps its holder =====

#[test]
fn cleanup_refuses_the_holder_of_a_verified_person_answer_after_its_raiser_is_gone() {
    let _records = fx::Records::new();
    let env = Env::new();
    let (q, answer) = verified_without_raiser(&env);

    let before = env.snapshot_all();
    let out = env.cleanup("b", &[]);
    assert_person_refusal(&out, "b", &q, &answer, false);
    assert_eq!(env.snapshot_all(), before);
    assert!(env.exists("b"));

    // `--force` removes exactly `b`, listed with its warning.
    let before = env.snapshot_except("b");
    let out = env.cleanup("b", &["--force"]);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    assert_eq!(env.snapshot_except("b"), before);
    assert_eq!(
        out.json()["orphaned_questions"],
        json!([person_hold(&q, &answer)])
    );
    assert!(
        out.stderr.contains("with --force")
            && out.stderr.contains("holds the verified person answer")
            && out.stderr.contains(&q.identity())
            && out.stderr.contains(&answer),
        "{}",
        out.stderr
    );
    assert!(!env.exists("b"));
}

#[test]
fn a_verified_person_answer_keeps_its_holder_at_its_tick_and_through_every_prune() {
    let _records = fx::Records::new();
    let env = Env::new();
    let (q, _) = verified_without_raiser(&env);
    let id = q.identity();

    assert_eq!(
        env.finish_env(&FAST, "b")["retention"],
        json!({
            "retained": true,
            "reason": "person_answer",
            "answered_questions": [id],
        })
    );
    assert!(env.exists("b"));
    settle();

    let h = env
        .ok_env(&FAST, &["session", "hierarchy", "b", "--json"])
        .json();
    assert_eq!(h["kept"], true, "{h:#}");
    let keepers = h["keepers"].as_array().unwrap();
    assert_eq!(keepers.len(), 1, "{h:#}");
    assert_eq!(keepers[0]["name"], "b");
    assert_eq!(keepers[0]["alive"], false);
    assert_eq!(keepers[0]["answered_questions"], json!([id]));
    let says = format!("b holds a verified person answer to question {id}");

    let preview = env.prune(&[]);
    assert!(
        preview["remove"].as_array().unwrap().is_empty(),
        "{preview:#}"
    );
    let detail = kept_entry(&preview, "b")["detail"].as_str().unwrap();
    assert!(detail.contains(&says), "{detail}");

    for args in [
        &["--apply", "--all"][..],
        &["--older-than", "1s", "--apply"][..],
    ] {
        let applied = env.prune(args);
        assert_eq!(applied["removed"], json!([]), "{args:?}: {applied:#}");
        assert_eq!(names_of(&applied["kept"]), vec!["b"], "{args:?}");
        assert!(env.exists("b"), "{args:?}");
    }

    let out = env.run_env(&FAST, &["workspace", "prune", "--root", "b", "--yes"]);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    assert_eq!(out.last_json()["removed"], json!([]));
    assert!(env.exists("b"));
}

#[test]
fn a_session_answer_or_a_remote_reading_person_answer_goes_once_its_raiser_is_gone() {
    let _records = fx::Records::new();
    let env = Env::new();
    env.init("a", &[]);
    // `s` holds only a session's answer.
    env.init("s", &[]);
    let by_session = env.escalate("a", "q1", "session:s");
    env.answer(&by_session, "s", "yes");
    // `r` and `t` each hold only a person's answer that reads as a remote
    // session's answer: this host's note in their directories is gone.
    env.init("r", &[]);
    env.init("t", &[]);
    let mut remote = Vec::new();
    for (holder, question, call) in [
        ("r", "q2", "toolu_ZQXMARKERretention02"),
        ("t", "q3", "toolu_ZQXMARKERretention03"),
    ] {
        let (q, header) = env.raise_for_person("a", question, "person:owner");
        let id = env.person_answer_at(&q, &header, holder, call);
        std::fs::remove_file(env.sessions().join(holder).join("koto-verified-here")).unwrap();
        assert_eq!(
            env.show(holder, &id)["reading"],
            "a remote session's answer"
        );
        remote.push(q);
    }

    // While the raiser is here, the session's answer holds as before, and
    // the remote-reading answers, which don't count for a question waiting
    // on a person, hold nothing.
    assert_holds_refusal(
        &env.cleanup("s", &[]),
        "s",
        &[(&by_session.identity(), "answer")],
    );
    for q in &remote {
        assert!(env.listed_as_raised(q));
    }

    let out = env.cleanup("a", &["--force"]);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);

    // Removed without force, by cleanup and at the terminal tick.
    assert_cleaned_without_force(&env, "s");
    assert_cleaned_without_force(&env, "r");
    assert_eq!(env.finish("t")["retention"], json!({"retained": false}));
    assert!(!env.exists("t"));
}

#[test]
fn cancel_cleanup_and_replace_refuse_the_holder_of_a_verified_person_answer() {
    let _records = fx::Records::new();
    let env = Env::new();
    let (q, answer) = verified_without_raiser(&env);

    let before = env.snapshot_all();
    let out = env.run(&["cancel", "b", "--cleanup"]);
    assert_person_refusal(&out, "b", &q, &answer, false);
    assert_eq!(env.snapshot_all(), before);
    // Not cancelled: it still advances.
    let next = env.ok(&["next", "b", "--no-cleanup"]).json();
    assert_eq!(next["state"], "work", "{next}");

    assert_eq!(env.finish("b")["retention"]["reason"], "person_answer");
    let before = env.snapshot_all();
    let out = env.replace("b");
    assert_person_refusal(&out, "b", &q, &answer, true);
    assert_eq!(env.snapshot_all(), before);
    assert!(!env.exists("b_r1"), "b was moved aside");
}

// ===== Decisions that can't be read =====

#[test]
fn unreadable_decisions_refuse_cleanup_cancel_and_replace_and_remove_nothing() {
    let env = Env::new();
    env.init("a", &[]);
    env.escalate("a", "q1", "person:owner");
    // `d` answered `c`'s question; `c`'s decisions will be unreadable.
    env.init("c", &[]);
    env.init("d", &[]);
    let cq = env.escalate("c", "q1", "session:d");
    env.answer(&cq, "d", "yes");

    let before = env.snapshot_all();
    if !env.lock_decisions("a") {
        return;
    }
    let out = env.cleanup("a", &[]);
    env.unlock_decisions("a");
    assert_unreadable_refusal(&out, "a", "a");
    assert_eq!(out.json()["error"]["questions"], json!([]));
    assert_eq!(env.snapshot_all(), before);

    assert!(env.lock_decisions("a"));
    let out = env.run(&["cancel", "a", "--cleanup"]);
    env.unlock_decisions("a");
    assert_unreadable_refusal(&out, "a", "a");
    assert_eq!(env.snapshot_all(), before);

    // The answerer of a question whose session can't be read.
    assert!(env.lock_decisions("c"));
    let out = env.cleanup("d", &[]);
    env.unlock_decisions("c");
    assert_unreadable_refusal(&out, "d", "c");
    assert_eq!(env.snapshot_all(), before);

    // The replace of a finished session whose decisions can't be read.
    assert_eq!(env.finish("a")["retention"]["reason"], "open_question");
    let before = env.snapshot_all();
    assert!(env.lock_decisions("a"));
    let out = env.replace("a");
    env.unlock_decisions("a");
    assert_eq!(out.code, 2, "{}\n{}", out.stdout, out.stderr);
    let v = out.json();
    assert_eq!(v["code"], "holds_question", "{v}");
    assert_eq!(v["command"], "init", "{v}");
    assert!(
        v["decisions_unreadable"][0]
            .as_str()
            .is_some_and(|u| u.contains("'a'")),
        "{v}"
    );
    assert!(
        v["error"].as_str().unwrap().contains("can't be read"),
        "{v}"
    );
    assert_eq!(env.snapshot_all(), before);
    assert!(!env.exists("a_r1"));
}

#[test]
fn a_forced_cleanup_of_unreadable_decisions_warns_what_went_unchecked() {
    let env = Env::new();
    env.init("a", &[]);
    env.escalate("a", "q1", "person:owner");
    if !env.lock_decisions("a") {
        return;
    }
    let out = env.cleanup("a", &["--force"]);
    env.unlock_decisions("a");
    // What follows is what koto did before questions were kept; only the
    // warning, said before anything goes, is new.
    assert!(
        out.stderr
            .contains("with --force without checking what it holds")
            && out.stderr.contains("the decisions of 'a'"),
        "{}\n{}",
        out.stdout,
        out.stderr
    );
    assert_ne!(out.code, 2, "{}\n{}", out.stdout, out.stderr);
}

// ===== Cloud backend =====

const BUCKET: &str = "koto-retention";
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

#[test]
fn a_cloud_cleanup_refusal_deletes_nothing_from_the_bucket() {
    let s3 = fake_s3::FakeS3::start(BUCKET);
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (home, cache, ws) = (root.join("home"), root.join("cache"), root.join("ws"));
    for dir in [&home, &cache, &ws] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(ws.join("work.md"), TEMPLATE).unwrap();
    let koto_dir = koto_command::koto_bin().parent().unwrap().to_path_buf();
    let run = |args: &[&str]| -> Out {
        let o = koto_command::koto_std()
            .env_clear()
            .current_dir(&ws)
            .env("PATH", format!("{}:{}", koto_dir.display(), SYSTEM_PATH))
            .env("HOME", &home)
            .env("XDG_CACHE_HOME", &cache)
            .env("KOTO_SESSION_BACKEND", "cloud")
            .env("KOTO_SESSION_CLOUD_ENDPOINT", s3.endpoint())
            .env("KOTO_SESSION_CLOUD_BUCKET", BUCKET)
            .env("KOTO_SESSION_CLOUD_REGION", "us-east-1")
            .env("KOTO_SESSION_CLOUD_PATH_STYLE", "true")
            .env("KOTO_SESSION_CLOUD_ACCESS_KEY", "test-access-key")
            .env("KOTO_SESSION_CLOUD_SECRET_KEY", "test-secret-key")
            .args(args)
            .output()
            .unwrap();
        Out {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    };
    let ok = |args: &[&str]| -> Value {
        let out = run(args);
        assert_eq!(out.code, 0, "{args:?}\n{}\n{}", out.stdout, out.stderr);
        out.json()
    };

    ok(&["init", "a", "--template", "work.md"]);
    let recorded = ok(&[
        "decisions",
        "record",
        "a",
        "--kind",
        "escalate",
        "--outcome",
        "escalated",
        "--subject",
        "question=q1",
        "--subject",
        "text=which?",
        "--subject",
        "recipient=person:owner",
        "--by",
        "session",
    ]);
    let identity = format!(
        "{}/{}",
        recorded["decision"]["session"]["id"].as_str().unwrap(),
        recorded["id"].as_str().unwrap()
    );
    let local = home.join(".koto/sessions/a");
    assert!(local.is_dir(), "no local copy at {}", local.display());
    let (keys, files) = (s3.keys(), snapshot_dir(&local));
    assert!(!keys.is_empty());

    s3.clear_requests();
    let out = run(&["session", "cleanup", "a"]);
    assert_holds_refusal(&out, "a", &[(&identity, "raised")]);
    let deletes: Vec<String> = s3
        .requests()
        .iter()
        .filter(|r| r.method == "DELETE" || r.method == "POST")
        .map(|r| format!("{} {}", r.method, r.key))
        .collect();
    assert!(deletes.is_empty(), "the refusal deleted: {deletes:?}");
    assert_eq!(s3.keys(), keys);
    assert_eq!(snapshot_dir(&local), files);

    // `koto cancel --cleanup` is refused the same way.
    s3.clear_requests();
    let out = run(&["cancel", "a", "--cleanup"]);
    assert_holds_refusal(&out, "a", &[(&identity, "raised")]);
    assert!(
        s3.requests().iter().all(|r| r.method != "DELETE"),
        "{:?}",
        s3.requests()
    );
    assert_eq!(s3.keys(), keys);
    assert_eq!(snapshot_dir(&local), files);
}
