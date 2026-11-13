//! Integration tests for `koto decisions import <session> <file>` (PRD
//! R28): entries recorded elsewhere become decisions in the importing
//! session, labelled `handed`, once.
//!
//! Every test runs the real binary with `HOME` and `KOTO_SESSIONS_BASE`
//! pointed into its own temporary directory, and with every variable koto
//! reads an identity from removed.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::path::PathBuf;

use assert_fs::TempDir;
use serde_json::{json, Value};

const CLEARED_ENV: [&str; 7] = [
    "CLAUDE_CODE_SESSION_ID",
    "KOTO_ANCHOR",
    "KOTO_AGENT_ID",
    "KOTO_RUN_MODE",
    "KOTO_AUTHORITY",
    "KOTO_TEST_DECISIONS_FAULT",
    "KOTO_TEST_DECISION_FAULT",
];

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

    fn run(&self, args: &[&str]) -> Out {
        let mut cmd = koto_command::koto_std();
        for name in CLEARED_ENV {
            cmd.env_remove(name);
        }
        cmd.current_dir(self.root.join("ws"));
        cmd.env("HOME", self.root.join("home"));
        cmd.env("KOTO_HOME", self.root.join("home"));
        cmd.env("KOTO_SESSIONS_BASE", self.root.join("sessions"));
        let output = cmd.args(args).output().unwrap();
        Out {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    fn ok(&self, args: &[&str]) -> Out {
        let out = self.run(args);
        assert_eq!(
            out.code, 0,
            "expected success from {args:?}\n{}\n{}",
            out.stdout, out.stderr
        );
        out
    }

    fn init(&self, name: &str) {
        let template = self.root.join("work.md");
        self.ok(&["init", name, "--template", template.to_str().unwrap()]);
    }

    /// Write `lines` as a JSON Lines file and return its path.
    fn file(&self, name: &str, lines: &[Value]) -> String {
        let mut text = String::new();
        for l in lines {
            text.push_str(&l.to_string());
            text.push('\n');
        }
        let path = self.root.join(name);
        std::fs::write(&path, text).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn import(&self, session: &str, file: &str) -> Out {
        self.run(&["decisions", "import", session, file])
    }

    /// `list --format v1` entries.
    fn list(&self, session: &str) -> Vec<Value> {
        let out = self
            .ok(&["decisions", "list", session, "--format", "v1"])
            .json();
        out["decisions"].as_array().unwrap().clone()
    }

    /// Every file in `session`'s decision areas.
    fn files(&self, session: &str) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![self.root.join("sessions").join(session).join("ctx/koto")];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.to_string_lossy().contains("/aimed/")
                    || p.to_string_lossy().contains("/decisions/")
                {
                    out.push(p);
                }
            }
        }
        out.sort();
        out
    }
}

/// An interim entry with the given source id.
fn entry(source_id: &str) -> Value {
    json!({
        "source": "interim-log",
        "source_id": source_id,
        "time": "2026-10-02T09:30:00Z",
        "kind": "judgment",
        "outcome": "settled",
        "subject": {"what": format!("finding {source_id}")},
        "decider": {"kind": "session", "session": "coordinator-7"},
        "authority": {"kind": "dispatch"},
    })
}

fn three_entries() -> Vec<Value> {
    vec![
        json!({
            "source": "interim-log",
            "source_id": "e-1",
            "time": "2026-10-02T09:30:00Z",
            "kind": "accept",
            "outcome": "accepted",
            "subject": {"repository": "koto", "paths": ["docs/a.md", "docs/b.md"], "commit": "abc123"},
            "decider": {"kind": "person", "handle": "dana"},
            "authority": {"kind": "person", "level": "maintainer", "grants": ["merge"]},
            "reason": "reviewed in the interim",
        }),
        json!({
            "source": "interim-log",
            "source_id": "e-2",
            "time": "2026-10-03T10:00:00.250Z",
            "kind": "judgment",
            "outcome": "settled",
            "subject": {"what": "dismiss the lint finding"},
            "decider": {"kind": "session", "session": "coordinator-7"},
            "authority": {"kind": "dispatch"},
        }),
        json!({
            "source": "interim-log",
            "source_id": "e-3",
            "time": "2026-10-04T11:00:00Z",
            "kind": "verdict",
            "outcome": "pass",
            "subject": {"what": "the release", "repository": "koto", "commit": "def456"},
            "evidence": ["https://ci.example/runs/7"],
            "decider": {"kind": "check", "name": "grader"},
            "authority": {"kind": "dispatch"},
        }),
    ]
}

/// Every `source` label anywhere under `v`.
fn labels(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                if k == "source" {
                    if let Some(s) = val.as_str() {
                        out.push(s.to_string());
                    }
                }
                labels(val, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|i| labels(i, out)),
        _ => {}
    }
}

#[test]
fn three_entries_record_three_handed_decisions_and_a_second_import_adds_nothing() {
    let env = Env::new();
    env.init("coord");
    let file = env.file("interim.jsonl", &three_entries());

    let out = env.import("coord", &file);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    let first = out.json();
    assert_eq!(first["v"], 1);
    assert_eq!(first["session"], "coord");
    assert_eq!(first["imported"].as_array().unwrap().len(), 3, "{first}");
    assert_eq!(first["skipped"], json!([]));
    assert_eq!(
        first["imported"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["line"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );

    let listed = env.list("coord");
    assert_eq!(listed.len(), 3, "{listed:?}");
    let times = [
        "2026-10-02T09:30:00.000Z",
        "2026-10-03T10:00:00.250Z",
        "2026-10-04T11:00:00.000Z",
    ];
    for (i, d) in listed.iter().enumerate() {
        let body = &d["decision"];
        assert_eq!(d["status"], "in-effect");
        // The source time is kept, in koto's stored form.
        assert_eq!(body["time"], times[i], "{body}");
        let imported = &body["imported"];
        assert_eq!(imported["source"], "interim-log");
        assert_eq!(imported["source_id"], format!("e-{}", i + 1));
        let at = imported["imported_at"].as_str().unwrap();
        assert_eq!(at.len(), 24, "{at}");
        assert!(at > times[i], "imported_at {at} is the import's time");
        // Every decider and authority label is handed.
        let mut seen = Vec::new();
        labels(&body["decider"], &mut seen);
        labels(&body["authority"], &mut seen);
        assert!(!seen.is_empty());
        assert!(seen.iter().all(|s| s == "handed"), "{seen:?} in {body}");
        assert!(body.get("standing").is_none());
    }
    let accept = &listed[0]["decision"];
    assert_eq!(accept["kind"], "accept");
    assert_eq!(
        accept["subject"]["paths"],
        json!(["docs/a.md", "docs/b.md"])
    );
    assert_eq!(accept["decider"]["kind"], "person");
    assert_eq!(accept["decider"]["handle"]["value"], "dana");
    assert_eq!(accept["authority"]["level"], "maintainer");
    assert_eq!(accept["authority"]["grants"], json!(["merge"]));
    assert_eq!(accept["reason"], "reviewed in the interim");
    assert_eq!(listed[1]["decision"]["decider"]["kind"], "session");
    assert_eq!(
        listed[1]["decision"]["decider"]["session"]["value"],
        "coordinator-7"
    );
    assert_eq!(listed[2]["decision"]["decider"]["kind"], "check");
    assert_eq!(listed[2]["decision"]["evidence"][0]["kind"], "pointer");

    let files_before = env.files("coord");
    let bytes_before: Vec<Vec<u8>> = files_before
        .iter()
        .map(|p| std::fs::read(p).unwrap())
        .collect();
    let again = env.import("coord", &file);
    assert_eq!(again.code, 0, "{}\n{}", again.stdout, again.stderr);
    let again = again.json();
    assert_eq!(again["imported"], json!([]), "{again}");
    assert_eq!(again["skipped"], first["imported"], "{again}");
    assert_eq!(env.files("coord"), files_before);
    let bytes_after: Vec<Vec<u8>> = files_before
        .iter()
        .map(|p| std::fs::read(p).unwrap())
        .collect();
    assert_eq!(bytes_after, bytes_before);
    assert_eq!(env.list("coord").len(), 3);
}

#[test]
fn a_line_breaking_r1_or_r2_stops_the_import_with_its_line_and_records_nothing() {
    let cases: Vec<(Value, &str)> = vec![
        // R1: a kind outside the set, an outcome the kind doesn't allow.
        (json!({"kind": "approve"}), "kind"),
        (json!({"outcome": "merged"}), "outcome"),
        // R2: a merge without its pull request, a judgment with a gate.
        (
            json!({"kind": "merge", "outcome": "merged",
                   "subject": {"repository": "koto", "commit": "abc"}}),
            "subject.pr",
        ),
        (
            json!({"subject": {"what": "x", "gate": "ci"}}),
            "subject.gate",
        ),
        // A refused decision isn't imported.
        (
            json!({"kind": "refused", "outcome": "refused",
                   "subject": {"kind": "merge", "subject": "x"}, "reason": "r"}),
            "kind",
        ),
    ];
    for (over, field) in cases {
        let env = Env::new();
        env.init("coord");
        let mut bad = entry("e-3");
        for (k, v) in over.as_object().unwrap() {
            bad[k] = v.clone();
        }
        let file = env.file(
            "interim.jsonl",
            &[entry("e-1"), entry("e-2"), bad, entry("e-4")],
        );
        let out = env.import("coord", &file);
        assert_eq!(out.code, 2, "{over}: {}\n{}", out.stdout, out.stderr);
        let err = out.json();
        assert_eq!(err["code"], "invalid_decision", "{err}");
        assert_eq!(err["line"], 3, "{err}");
        assert_eq!(err["field"], field, "{err}");
        assert_eq!(err["command"], "decisions import");
        assert!(env.files("coord").is_empty(), "{over} recorded something");
        assert!(env.list("coord").is_empty());
    }

    // A line that isn't JSON stops it the same way.
    let env = Env::new();
    env.init("coord");
    let path = env.root.join("broken.jsonl");
    std::fs::write(&path, format!("{}\n{{not json\n", entry("e-1"))).unwrap();
    let out = env.import("coord", path.to_str().unwrap());
    assert_eq!(out.code, 2);
    let err = out.json();
    assert_eq!(err["line"], 2, "{err}");
    assert_eq!(err["field"], "line");
    assert!(env.files("coord").is_empty());
}

#[test]
fn a_reverse_from_the_same_file_records_asserted_and_its_original_reads_reversed() {
    let env = Env::new();
    env.init("coord");
    let reverse = json!({
        "source": "interim-log",
        "source_id": "r-1",
        "time": "2026-10-05T09:00:00Z",
        "kind": "reverse",
        "outcome": "reversed",
        "subject": {"source": "interim-log", "source_id": "e-1"},
        "decider": {"kind": "person", "handle": "dana"},
        "authority": {"kind": "person"},
        "reason": "the finding was real",
    });
    // The reversal comes first in the file; its original is written first.
    let file = env.file("interim.jsonl", &[reverse, entry("e-1")]);
    let out = env.import("coord", &file);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    let imported = out.json()["imported"].clone();
    assert_eq!(imported.as_array().unwrap().len(), 2);

    let listed = env.list("coord");
    assert_eq!(listed.len(), 2, "{listed:?}");
    let original = listed
        .iter()
        .find(|d| d["decision"]["kind"] == "judgment")
        .unwrap();
    let reversal = listed
        .iter()
        .find(|d| d["decision"]["kind"] == "reverse")
        .unwrap();
    let original_id = original["id"].as_str().unwrap();
    assert_eq!(original["status"], "reversed", "{original}");
    assert_eq!(original["reversed_by"]["id"], reversal["id"]);
    assert_eq!(original["reversed_by"]["standing"], "asserted");

    let body = &reversal["decision"];
    assert_eq!(reversal["status"], "in-effect");
    assert_eq!(body["standing"], "asserted");
    assert_eq!(body["path"], json!([]));
    assert_eq!(
        body["subject"],
        json!({"session": "coord", "id": original_id})
    );
    assert_eq!(body["target"]["name"], "coord");
    assert_eq!(body["target"]["decision"], original_id);
    assert_eq!(body["imported"]["source_id"], "r-1");
    // Its original's id is the one before it.
    assert!(reversal["id"].as_str().unwrap() > original_id);
    // It sits in the importing session's aimed area for itself.
    let aimed: Vec<_> = env
        .files("coord")
        .into_iter()
        .filter(|p| p.to_string_lossy().contains("/aimed/"))
        .collect();
    assert_eq!(aimed.len(), 1, "{aimed:?}");

    // Importing again adds nothing, reversal included.
    let again = env.import("coord", &file).json();
    assert_eq!(again["imported"], json!([]));
    assert_eq!(again["skipped"].as_array().unwrap().len(), 2);
}

#[test]
fn a_reverse_naming_another_sessions_decision_is_refused() {
    let env = Env::new();
    env.init("coord");
    env.init("w");
    let recorded = env
        .ok(&[
            "decisions",
            "record",
            "w",
            "--kind",
            "judgment",
            "--outcome",
            "settled",
            "--subject",
            "what=use sqlite",
            "--by",
            "session",
        ])
        .json();
    let id = recorded["id"].as_str().unwrap();
    let w_files = env.files("w");

    let reverse = |subject: Value| {
        json!({
            "source": "interim-log",
            "source_id": "r-1",
            "time": "2026-10-05T09:00:00Z",
            "kind": "reverse",
            "outcome": "reversed",
            "subject": subject,
            "decider": {"kind": "person", "handle": "dana"},
            "authority": {"kind": "person"},
        })
    };
    // Another session's decision, by R2's session and id, and an entry no
    // line of the file holds.
    for subject in [
        json!({"session": "w", "id": id}),
        json!({"source": "interim-log", "source_id": "e-9"}),
    ] {
        let file = env.file("interim.jsonl", &[entry("e-1"), reverse(subject.clone())]);
        let out = env.import("coord", &file);
        assert_eq!(out.code, 2, "{subject}: {}\n{}", out.stdout, out.stderr);
        let err = out.json();
        assert_eq!(err["code"], "reversal_refused", "{err}");
        assert_eq!(err["reason"], "not_in_import", "{err}");
        assert_eq!(err["line"], 2, "{err}");
        assert!(
            env.files("coord").is_empty(),
            "{subject} recorded something"
        );
    }
    assert_eq!(env.files("w"), w_files);
    let w = env.list("w");
    assert_eq!(w.len(), 1);
    assert_eq!(w[0]["status"], "in-effect");

    // An entry imported from an earlier file isn't in this one either.
    let ok = env.file("first.jsonl", &[entry("e-1")]);
    env.ok(&["decisions", "import", "coord", &ok]);
    let file = env.file(
        "second.jsonl",
        &[reverse(
            json!({"source": "interim-log", "source_id": "e-1"}),
        )],
    );
    let out = env.import("coord", &file);
    assert_eq!(out.code, 2);
    assert_eq!(out.json()["reason"], "not_in_import");
    assert_eq!(env.list("coord").len(), 1);
}

#[test]
fn an_entry_already_imported_with_other_content_is_refused() {
    let env = Env::new();
    env.init("coord");
    let first = env.file("first.jsonl", &[entry("e-1")]);
    env.ok(&["decisions", "import", "coord", &first]);
    let mut changed = entry("e-1");
    changed["subject"] = json!({"what": "something else"});
    let second = env.file("second.jsonl", &[entry("e-2"), changed]);
    let out = env.import("coord", &second);
    assert_eq!(out.code, 2, "{}\n{}", out.stdout, out.stderr);
    let err = out.json();
    assert_eq!(err["code"], "invalid_decision");
    assert_eq!(err["field"], "source_id");
    assert_eq!(err["line"], 2, "{err}");
    // e-2, which came before it in the file, wasn't recorded either.
    assert_eq!(env.list("coord").len(), 1);
}

#[test]
fn an_import_into_a_missing_session_or_from_a_missing_file_is_refused() {
    let env = Env::new();
    env.init("coord");
    let file = env.file("interim.jsonl", &[entry("e-1")]);
    assert_eq!(env.import("nobody", &file).code, 2);
    let out = env.import("coord", env.root.join("absent.jsonl").to_str().unwrap());
    assert_eq!(out.code, 2);
    assert_eq!(out.json()["field"], "file");
}
