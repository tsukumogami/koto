//! Integration tests for the decision record under the local backend:
//! `koto decisions record --kind`, `list --format v1` and `show`.
//!
//! Every test runs the real binary with `HOME` and `KOTO_SESSIONS_BASE`
//! pointed into its own temporary directory, and with every variable koto
//! reads an identity from removed unless the test sets it.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};

use assert_fs::TempDir;
use serde_json::Value;

/// Variables koto reads an identity, a run mode or an authority from.
const IDENTITY_ENV: [&str; 5] = [
    "CLAUDE_CODE_SESSION_ID",
    "KOTO_ANCHOR",
    "KOTO_AGENT_ID",
    "KOTO_RUN_MODE",
    "KOTO_AUTHORITY",
];

/// A two-state template: `work` waits for `go`, `done` is a success
/// terminal.
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

/// A template the leg of a request names.
const SCOPE_TEMPLATE: &str = r#"---
name: scope
version: "1.0"
initial_state: work
variables:
  TOPIC:
    required: true
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

Scope {{TOPIC}}.

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
        std::fs::write(root.join("ws").join("scope.md"), SCOPE_TEMPLATE).unwrap();
        Env { _tmp: tmp, root }
    }

    fn sessions(&self) -> PathBuf {
        self.root.join("sessions")
    }

    fn ws(&self) -> PathBuf {
        self.root.join("ws")
    }

    fn command(&self, envs: &[(&str, &str)]) -> std::process::Command {
        let mut cmd = koto_command::koto_std();
        for name in IDENTITY_ENV {
            cmd.env_remove(name);
        }
        cmd.current_dir(self.ws());
        cmd.env("HOME", self.root.join("home"));
        cmd.env("KOTO_HOME", self.root.join("home"));
        cmd.env("KOTO_SESSIONS_BASE", self.sessions());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        cmd
    }

    fn run_env(&self, envs: &[(&str, &str)], args: &[&str]) -> Out {
        let output = self.command(envs).args(args).output().unwrap();
        Out {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    fn run(&self, args: &[&str]) -> Out {
        self.run_env(&[], args)
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

    fn ok(&self, args: &[&str]) -> Out {
        self.ok_env(&[], args)
    }

    fn init(&self, name: &str, extra: &[&str]) {
        let template = self.root.join("work.md");
        let mut args = vec!["init", name, "--template", template.to_str().unwrap()];
        args.extend_from_slice(extra);
        self.ok(&args);
    }

    /// Record with `--by person:dana --surface terminal` unless `extra`
    /// names a decider, so no harness variable is needed.
    fn record(&self, name: &str, kind: &str, outcome: &str, subject: &[&str]) -> Value {
        self.record_with(&[], name, kind, outcome, subject, &[])
    }

    fn record_with(
        &self,
        envs: &[(&str, &str)],
        name: &str,
        kind: &str,
        outcome: &str,
        subject: &[&str],
        extra: &[&str],
    ) -> Value {
        let args = record_args(name, kind, outcome, subject, extra);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        self.ok_env(envs, &args).json()
    }

    fn decision_dir(&self, name: &str) -> PathBuf {
        self.sessions()
            .join(name)
            .join("ctx")
            .join("koto")
            .join("decisions")
    }

    fn decision_files(&self, name: &str) -> Vec<String> {
        let mut names: Vec<String> = match std::fs::read_dir(self.decision_dir(name)) {
            Ok(rd) => rd
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect(),
            Err(_) => Vec::new(),
        };
        names.sort();
        names
    }

    fn list(&self, name: &str, extra: &[&str]) -> Value {
        let mut args = vec!["decisions", "list", name, "--format", "v1"];
        args.extend_from_slice(extra);
        self.ok(&args).json()
    }

    /// Every file under the sessions base and its bytes, lock files aside.
    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut out = BTreeMap::new();
        snapshot_into(&self.sessions(), &self.sessions(), &mut out);
        out
    }
}

fn snapshot_into(base: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd {
        let entry = entry.unwrap();
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path).unwrap();
        if meta.is_dir() {
            snapshot_into(base, &path, out);
        } else if meta.is_file() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".lock") {
                continue;
            }
            out.insert(
                path.strip_prefix(base).unwrap().to_path_buf(),
                std::fs::read(&path).unwrap(),
            );
        }
    }
}

fn record_args(
    name: &str,
    kind: &str,
    outcome: &str,
    subject: &[&str],
    extra: &[&str],
) -> Vec<String> {
    let mut args: Vec<String> = [
        "decisions",
        "record",
        name,
        "--kind",
        kind,
        "--outcome",
        outcome,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for s in subject {
        args.push("--subject".to_string());
        args.push(s.to_string());
    }
    if !extra.contains(&"--by") {
        args.extend(
            ["--by", "person:dana", "--surface", "terminal"]
                .iter()
                .map(|s| s.to_string()),
        );
    }
    args.extend(extra.iter().map(|s| s.to_string()));
    args
}

fn is_twelve_digits(s: &str) -> bool {
    s.len() == 12 && s.bytes().all(|b| b.is_ascii_digit())
}

/// The ids of a `list --format v1` result, in listed order.
fn ids(listing: &Value) -> Vec<String> {
    listing["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_str().unwrap_or("").to_string())
        .collect()
}

/// Every object key anywhere in `v`.
fn all_keys(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                out.push(k.clone());
                all_keys(child, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|i| all_keys(i, out)),
        _ => {}
    }
}

/// Open the anchor for harness session `harness`, returning its name.
fn open_anchor(env: &Env, harness: &str, extra: &[&str], envs: &[(&str, &str)]) -> String {
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
    let out = env.ok_env(envs, &args);
    out.stdout
        .lines()
        .find_map(|l| l.strip_prefix("anchor="))
        .unwrap_or_else(|| panic!("no anchor line: {}", out.stdout))
        .to_string()
}

// ===== R1, R2: kinds, outcomes and subjects =====

#[test]
fn each_kind_and_subject_shape_writes_exactly_one_twelve_digit_key() {
    let env = Env::new();
    env.init("w", &[]);
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "accept",
            "accepted",
            &["repository=r", "path=a.md", "path=b.md", "commit=c1"],
        ),
        (
            "accept",
            "rejected",
            &["repository=r", "plan=p", "step=2", "commit=c1"],
        ),
        (
            "accept",
            "accepted",
            &["location=req-1", "link=https://x.example/1"],
        ),
        ("merge", "merged", &["repository=r", "pr=12", "commit=c1"]),
        ("gate-pass", "passed", &["workflow=w", "state=s", "gate=g"]),
        (
            "escalate",
            "escalated",
            &["question=q1", "text=t", "recipient=coord"],
        ),
        ("decline-escalation", "kept", &["question=q1", "text=t"]),
        ("judgment", "settled", &["what=a finding"]),
        ("verdict", "pass", &["what=the change"]),
        (
            "verdict",
            "fail",
            &["what=the release", "repository=r", "commit=c1"],
        ),
        ("refused", "refused", &["kind=merge", "subject=pr 12"]),
    ];
    for (n, (kind, outcome, subject)) in cases.iter().enumerate() {
        let extra: &[&str] = match *kind {
            "verdict" => &["--evidence", "event:w:1"],
            "refused" => &["--reason", "the gate refused it"],
            _ => &[],
        };
        let out = env.record_with(&[], "w", kind, outcome, subject, extra);
        let id = out["id"].as_str().unwrap();
        assert!(is_twelve_digits(id), "{kind}: {id}");
        assert_eq!(out["decision"]["kind"], *kind);
        assert_eq!(out["decision"]["outcome"], *outcome);
        let files = env.decision_files("w");
        assert_eq!(files.len(), n + 1, "{kind}: {files:?}");
        assert!(files.contains(&id.to_string()));
    }
    // `path` repeats and is stored as `paths`.
    let first = env.ok(&["decisions", "show", "w", "000000000001"]).json();
    assert_eq!(
        first["decision"]["subject"]["paths"],
        serde_json::json!(["a.md", "b.md"])
    );
}

#[test]
fn a_refused_record_names_the_field_and_writes_no_key() {
    let env = Env::new();
    env.init("w", &[]);
    let refusals: &[(&str, &str, &[&str], &str)] = &[
        ("nope", "settled", &["what=x"], "kind"),
        (
            "merge",
            "settled",
            &["repository=r", "pr=1", "commit=c"],
            "outcome",
        ),
        (
            "merge",
            "merged",
            &["repository=r", "commit=c"],
            "subject.pr",
        ),
        ("judgment", "settled", &[], "subject.what"),
    ];
    for (kind, outcome, subject, field) in refusals {
        let args = record_args("w", kind, outcome, subject, &[]);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = env.run(&args);
        assert_eq!(out.code, 2, "{kind}/{outcome}: {}", out.stdout);
        let err = out.json();
        assert_eq!(err["code"], "invalid_decision", "{err}");
        assert_eq!(err["field"], *field, "{err}");
        assert!(
            err["error"].as_str().unwrap().contains(field),
            "error doesn't name {field}: {err}"
        );
        assert!(
            env.decision_files("w").is_empty(),
            "{kind}: a key was written"
        );
    }
    // Reversals belong to a later command.
    let out = env.run(&[
        "decisions",
        "record",
        "w",
        "--kind",
        "reverse",
        "--outcome",
        "reversed",
        "--subject",
        "session=w",
        "--subject",
        "id=000000000001",
        "--by",
        "person:dana",
        "--surface",
        "terminal",
    ]);
    assert_eq!(out.code, 2, "{}", out.stdout);
    assert!(env.decision_files("w").is_empty());
}

#[test]
fn legacy_record_and_list_keep_their_output_without_a_new_flag() {
    let env = Env::new();
    env.init("w", &[]);
    let out = env.ok(&["decisions", "list", "w"]).json();
    assert_eq!(
        out,
        serde_json::json!({"decisions": {"count": 0, "items": []}, "state": "work"})
    );
    env.ok(&[
        "decisions",
        "record",
        "w",
        "--with-data",
        r#"{"choice":"a","rationale":"b"}"#,
    ]);
    let out = env.ok(&["decisions", "list", "w"]).json();
    assert_eq!(out["decisions"]["count"], 1, "{out}");
    assert_eq!(out["decisions"]["items"][0]["choice"], "a", "{out}");
    // The choice is also recorded as a judgment, the one decision key.
    assert_eq!(env.decision_files("w").len(), 1);
    let listed = env.list("w", &["--kind", "judgment"]);
    let items = listed["decisions"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{listed}");
    let d = items[0].get("decision").unwrap_or(&items[0]);
    assert_eq!(d["subject"]["what"], "a", "{listed}");
}

// ===== R3, R32: concurrent allocation =====

#[test]
fn two_processes_recording_fifty_each_end_with_a_hundred_ordered_ids() {
    let env = Arc::new(Env::new());
    env.init("w", &[]);
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|p| {
            let env = Arc::clone(&env);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                let mut got = Vec::new();
                for i in 0..50 {
                    let what = format!("what=p{p}-{i}");
                    let out = env.record("w", "judgment", "settled", &[&what]);
                    got.push(out["id"].as_str().unwrap().to_string());
                }
                got
            })
        })
        .collect();
    let per_process: Vec<Vec<String>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for got in &per_process {
        let mut sorted = got.clone();
        sorted.sort();
        assert_eq!(got, &sorted, "one process's ids out of its commit order");
    }
    let files = env.decision_files("w");
    assert_eq!(files.len(), 100);
    let distinct: std::collections::BTreeSet<_> = files.iter().collect();
    assert_eq!(distinct.len(), 100);
    assert!(files.iter().all(|f| is_twelve_digits(f)));

    // Byte order is commit order: the time stamped under the allocation
    // lock never goes backwards along the ids.
    let listing = env.list("w", &[]);
    let mut by_id: Vec<(String, String)> = listing["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["id"].as_str().unwrap().to_string(),
                d["decision"]["time"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(by_id.len(), 100);
    by_id.sort();
    for pair in by_id.windows(2) {
        assert!(pair[0].1 <= pair[1].1, "{pair:?}");
    }
}

// ===== R5: write-once =====

#[test]
fn context_writers_are_refused_on_a_decision_key() {
    let env = Env::new();
    env.init("w", &[]);
    env.init("other", &[]);
    env.record("w", "judgment", "settled", &["what=x"]);
    let key = "koto/decisions/000000000001";
    let path = env.decision_dir("w").join("000000000001");
    let before = std::fs::read(&path).unwrap();

    let input = env.root.join("input.txt");
    std::fs::write(&input, b"overwrite").unwrap();
    for session in ["w", "other"] {
        let out = env.run(&[
            "context",
            "add",
            session,
            key,
            "--from-file",
            input.to_str().unwrap(),
        ]);
        assert_ne!(out.code, 0, "add in {session}: {}", out.stdout);
        assert!(
            out.stdout.contains(key) || out.stderr.contains(key),
            "{}",
            out.stdout
        );
        let out = env.run(&["context", "remove", session, key]);
        assert_ne!(out.code, 0, "remove in {session}: {}", out.stdout);
        assert!(
            out.stdout.contains(key) || out.stderr.contains(key),
            "{}",
            out.stdout
        );
    }
    // From another session's agent: the identity variables don't change it.
    let out = env.run_env(
        &[("CLAUDE_CODE_SESSION_ID", "other-harness")],
        &["context", "remove", "w", key],
    );
    assert_ne!(out.code, 0);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!env.decision_dir("other").exists());

    let template = env.root.join("assign.md");
    std::fs::write(
        &template,
        format!(
            r#"---
name: assign
version: "1.0"
initial_state: start
states:
  start:
    accepts:
      go:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: done
        when:
          go: go
        context_assignments:
          "{key}": x
  done:
    terminal: true
---

## start

Start.

## done

Done.
"#
        ),
    )
    .unwrap();
    let out = env.run(&["init", "assign", "--template", template.to_str().unwrap()]);
    assert_ne!(out.code, 0, "{}", out.stdout);
    let text = format!("{}{}", out.stdout, out.stderr);
    assert!(text.contains("reserved prefix 'koto/'"), "{text}");
}

// ===== R7 to R9, R47: the decider =====

#[test]
fn a_harness_session_with_an_anchor_is_an_observed_session_decider() {
    let env = Env::new();
    let harness = "3f1e2a9c-0b7d-4e55-a1b2-c3d4e5f60718";
    let anchor = open_anchor(&env, harness, &[], &[]);
    env.init("w", &[]);
    let out = env.record_with(
        &[
            ("CLAUDE_CODE_SESSION_ID", harness),
            ("KOTO_ANCHOR", &anchor),
        ],
        "w",
        "judgment",
        "settled",
        &["what=x"],
        &["--by", "session"],
    );
    let d = &out["decision"]["decider"];
    assert_eq!(d["kind"], "session", "{d}");
    assert_eq!(d["session"]["value"], "w");
    assert_eq!(
        d["harness"],
        serde_json::json!({"value": harness, "source": "observed"})
    );
    assert_eq!(
        d["anchor"],
        serde_json::json!({"value": anchor, "source": "observed"})
    );
    assert_eq!(d["agent"]["value"], "unknown");
    assert_eq!(d["defaulted"], false);

    // The listing carries the same decider.
    let listing = env.list("w", &[]);
    assert_eq!(listing["decisions"][0]["decision"]["decider"], *d);

    // KOTO_AGENT_ID names the agent.
    let out = env.record_with(
        &[
            ("CLAUDE_CODE_SESSION_ID", harness),
            ("KOTO_ANCHOR", &anchor),
            ("KOTO_AGENT_ID", "agent-7"),
        ],
        "w",
        "judgment",
        "settled",
        &["what=y"],
        &["--by", "session"],
    );
    assert_eq!(
        out["decision"]["decider"]["agent"],
        serde_json::json!({"value": "agent-7", "source": "observed"})
    );
}

#[test]
fn a_person_on_a_terminal_is_handed_with_the_entering_session() {
    let env = Env::new();
    let harness = "h-person-1";
    env.init("w", &[]);
    let out = env.record_with(
        &[("CLAUDE_CODE_SESSION_ID", harness)],
        "w",
        "judgment",
        "settled",
        &["what=x"],
        &["--by", "person:dana", "--surface", "terminal"],
    );
    let d = &out["decision"]["decider"];
    assert_eq!(d["kind"], "person", "{d}");
    assert_eq!(
        d["handle"],
        serde_json::json!({"value": "dana", "source": "handed"})
    );
    assert_eq!(d["surface"], "terminal");
    assert_eq!(d["entering"]["session"]["value"], "w");
    assert_eq!(d["entering"]["harness"]["value"], harness);
}

#[test]
fn a_relay_without_a_valid_https_link_records_the_relaying_session() {
    let env = Env::new();
    let harness = "h-relay-1";
    env.init("w", &[]);
    for link in [None, Some("http://chat.example/m/1"), Some("not a link")] {
        let mut extra = vec!["--by", "person:dana", "--surface", "relay"];
        if let Some(l) = link {
            extra.extend_from_slice(&["--link", l]);
        }
        let out = env.record_with(
            &[("CLAUDE_CODE_SESSION_ID", harness)],
            "w",
            "judgment",
            "settled",
            &["what=x"],
            &extra,
        );
        let d = &out["decision"]["decider"];
        assert_eq!(d["kind"], "session", "{link:?}: {d}");
        assert_eq!(d["session"]["value"], "w");
        assert_eq!(d["harness"]["value"], harness);
    }
    let out = env.record_with(
        &[("CLAUDE_CODE_SESSION_ID", harness)],
        "w",
        "judgment",
        "settled",
        &["what=x"],
        &[
            "--by",
            "person:dana",
            "--surface",
            "relay",
            "--link",
            "https://chat.example/m/1",
        ],
    );
    assert_eq!(out["decision"]["decider"]["kind"], "person");
}

#[test]
fn the_harness_id_is_recorded_only_when_id_shaped() {
    let env = Env::new();
    env.init("w", &[]);
    for bad in ["a/b", " leading-space", "has space", &"x".repeat(129)] {
        // No anchor and no --by: no decider.
        let args = [
            "decisions",
            "record",
            "w",
            "--kind",
            "judgment",
            "--outcome",
            "settled",
            "--subject",
            "what=x",
        ];
        let out = env.run_env(&[("CLAUDE_CODE_SESSION_ID", bad)], &args);
        assert_eq!(out.code, 2, "{bad:?}: {}", out.stdout);
        assert_eq!(out.json()["code"], "decider_missing");

        // With a person, the entering harness id is empty.
        let out = env.record_with(
            &[("CLAUDE_CODE_SESSION_ID", bad)],
            "w",
            "judgment",
            "settled",
            &["what=x"],
            &[],
        );
        assert_eq!(
            out["decision"]["decider"]["entering"]["harness"]["value"], "",
            "{bad:?}"
        );

        // A session decider named with --by records the harness id empty.
        let out = env.record_with(
            &[("CLAUDE_CODE_SESSION_ID", bad)],
            "w",
            "judgment",
            "settled",
            &["what=x"],
            &["--by", "session"],
        );
        assert_eq!(
            out["decision"]["decider"]["harness"],
            serde_json::json!({"value": "", "source": "observed"}),
            "{bad:?}"
        );
    }
    let good = "3f1e2a9c-0b7d-4e55-a1b2-c3d4e5f60718";
    let out = env.record_with(
        &[("CLAUDE_CODE_SESSION_ID", good)],
        "w",
        "judgment",
        "settled",
        &["what=x"],
        &["--by", "session"],
    );
    assert_eq!(
        out["decision"]["decider"]["harness"],
        serde_json::json!({"value": good, "source": "observed"})
    );
}

// ===== R8: no registration =====

#[test]
fn a_never_seen_person_records_in_a_fresh_home_with_no_credential_field() {
    let env = Env::new();
    assert_eq!(std::fs::read_dir(env.root.join("home")).unwrap().count(), 0);
    env.init("w", &[]);
    let handle = "Never-Seen.Handle_42";
    let out = env.record_with(
        &[],
        "w",
        "judgment",
        "settled",
        &["what=x"],
        &["--by", &format!("person:{handle}"), "--surface", "terminal"],
    );
    assert_eq!(out["decision"]["decider"]["handle"]["value"], handle);
    let stored: Value = serde_json::from_slice(
        &std::fs::read(env.decision_dir("w").join(out["id"].as_str().unwrap())).unwrap(),
    )
    .unwrap();
    assert_eq!(stored["decider"]["handle"]["value"], handle);
    let mut keys = Vec::new();
    all_keys(&stored, &mut keys);
    for key in keys {
        let k = key.to_ascii_lowercase();
        for banned in ["account", "login", "password", "token", "credential"] {
            assert!(!k.contains(banned), "stored body has key {key}");
        }
    }
}

// ===== R10 to R13: authority, run mode and chain =====

/// Sessions linked every way R12 names:
///
/// `w` -leg-> `req-owner` -link-> `top` -init-> `root`, and `w` -held-by->
/// the anchor, which -anchor-> `opener`. `opener` is not created.
fn chain_fixture(env: &Env) -> String {
    env.init("root", &[]);
    env.init("top", &["--dispatched-by", "root"]);
    env.init("req-owner", &[]);
    env.ok(&["session", "link", "req-owner", "--dispatched-by", "top"]);
    let harness = "3f1e2a9c-0b7d-4e55-a1b2-c3d4e5f60719";
    let anchor = open_anchor(
        env,
        harness,
        &["--dispatched-by", "opener", "--record-authority"],
        &[("KOTO_AUTHORITY", "release-pin")],
    );
    let req = env
        .ok(&[
            "request",
            "create",
            "--with-data",
            r#"{"legs":[{"name":"scope","role":"scope","template":"scope.md","inputs":{"TOPIC":"t1"}}]}"#,
            "--requested-by",
            "req-owner",
            "--coordinator-of-record",
            "req-owner",
        ])
        .json()["request_id"]
        .as_str()
        .unwrap()
        .to_string();
    let template = env.ws().join("scope.md");
    let vars = env.root.join("vars.json");
    std::fs::write(&vars, r#"[["TOPIC","t1"]]"#).unwrap();
    env.ok_env(
        &[("KOTO_ANCHOR", &anchor)],
        &[
            "init",
            "w",
            "--template",
            template.to_str().unwrap(),
            "--vars-file",
            vars.to_str().unwrap(),
            "--koto-leg",
            &format!("{req}:scope"),
        ],
    );
    anchor
}

#[test]
fn a_decision_records_authority_run_mode_and_a_labelled_chain() {
    let env = Env::new();
    let anchor = chain_fixture(&env);
    let harness = "3f1e2a9c-0b7d-4e55-a1b2-c3d4e5f60719";
    let envs = [
        ("CLAUDE_CODE_SESSION_ID", harness),
        ("KOTO_ANCHOR", anchor.as_str()),
    ];
    let out = env.record_with(
        &envs,
        "w",
        "judgment",
        "settled",
        &["what=x"],
        &["--by", "session"],
    );
    let d = &out["decision"];
    let chain: Vec<(String, String)> = d["chain"]["links"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                l["session"].as_str().unwrap().to_string(),
                l["via"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let expected: Vec<(String, String)> = [
        ("req-owner", "leg"),
        ("top", "link"),
        ("root", "init"),
        (anchor.as_str(), "held-by"),
    ]
    .iter()
    .map(|(s, v)| (s.to_string(), v.to_string()))
    .collect();
    assert_eq!(chain, expected, "{}", d["chain"]);
    // `opener` is missing: the chain stops short of it, marked incomplete.
    assert_eq!(d["chain"]["incomplete"], true, "opener is missing");

    let a = &d["authority"];
    assert_eq!(a["level"], Value::Null);
    assert_eq!(a["grants"], serde_json::json!(["release-pin"]), "{a}");
    assert_eq!(a["dispatcher"], "req-owner");
    assert_eq!(
        a["run_mode"],
        serde_json::json!({"value": "unknown", "source": "observed"})
    );

    // With every chain session present the chain is complete.
    env.init("opener", &[]);
    let out = env.record_with(
        &envs,
        "w",
        "judgment",
        "settled",
        &["what=y"],
        &["--by", "session"],
    );
    assert_eq!(out["decision"]["chain"]["incomplete"], false);
    let last = out["decision"]["chain"]["links"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(
        last,
        serde_json::json!({"session": "opener", "via": "anchor"})
    );

    // Run mode: observed from KOTO_RUN_MODE, handed from --run-mode.
    let mut with_mode = envs.to_vec();
    with_mode.push(("KOTO_RUN_MODE", "unattended"));
    let out = env.record_with(
        &with_mode,
        "w",
        "judgment",
        "settled",
        &["what=z"],
        &["--by", "session"],
    );
    assert_eq!(
        out["decision"]["authority"]["run_mode"],
        serde_json::json!({"value": "unattended", "source": "observed"})
    );
    let out = env.record_with(
        &envs,
        "w",
        "judgment",
        "settled",
        &["what=z"],
        &["--by", "session", "--run-mode", "attended"],
    );
    assert_eq!(
        out["decision"]["authority"]["run_mode"],
        serde_json::json!({"value": "attended", "source": "handed"})
    );

    // A session with no anchor or dispatcher: null level, no grants.
    env.init("lone", &[]);
    let out = env.record("lone", "judgment", "settled", &["what=x"]);
    let a = &out["decision"]["authority"];
    assert_eq!(a["level"], Value::Null);
    assert_eq!(a["grants"], serde_json::json!([]));
    assert_eq!(a["dispatcher"], "none");
    assert_eq!(out["decision"]["chain"]["links"], serde_json::json!([]));
}

#[test]
fn a_workspace_manager_session_file_does_not_change_the_chain() {
    let env = Env::new();
    env.init("top", &[]);
    env.init("w", &["--dispatched-by", "top"]);
    let harness = "3f1e2a9c-0b7d-4e55-a1b2-c3d4e5f60720";
    let envs = [("CLAUDE_CODE_SESSION_ID", harness)];
    let before = env.record_with(
        &envs,
        "w",
        "judgment",
        "settled",
        &["what=x"],
        &["--by", "session"],
    );

    let planted = env.ws().join(".niwa").join("sessions");
    std::fs::create_dir_all(&planted).unwrap();
    std::fs::write(
        planted.join(format!("{harness}.json")),
        serde_json::json!({"id": harness, "parent": "someone-else", "session": "w"}).to_string(),
    )
    .unwrap();
    let after = env.record_with(
        &envs,
        "w",
        "judgment",
        "settled",
        &["what=x"],
        &["--by", "session"],
    );
    assert_eq!(before["decision"]["chain"], after["decision"]["chain"]);
    assert_eq!(
        before["decision"]["authority"],
        after["decision"]["authority"]
    );
}

// ===== R14 to R16, R44: reading =====

#[test]
fn a_coordinator_lists_a_finished_workers_decisions_with_filters() {
    let env = Env::new();
    env.init("coord", &[]);
    env.init("mid", &["--dispatched-by", "coord"]);
    env.init("worker", &["--dispatched-by", "mid"]);
    let harness = "3f1e2a9c-0b7d-4e55-a1b2-c3d4e5f60721";
    let session_env = [("CLAUDE_CODE_SESSION_ID", harness)];
    env.record_with(
        &session_env,
        "worker",
        "merge",
        "merged",
        &["repository=r", "pr=1", "commit=c"],
        &["--by", "session", "--run-mode", "unattended"],
    );
    env.record("worker", "judgment", "settled", &["what=a"]);
    env.record_with(
        &[],
        "worker",
        "verdict",
        "pass",
        &["what=a"],
        &["--by", "check:grader", "--evidence", "event:worker:1"],
    );
    env.record(
        "worker",
        "merge",
        "merged",
        &["repository=r", "pr=2", "commit=d"],
    );
    env.record("mid", "judgment", "settled", &["what=mid"]);

    // The worker and the middle session reach their success terminals.
    for s in ["worker", "mid"] {
        env.ok(&["next", s, "--with-data", r#"{"go":"go"}"#]);
    }

    let all = env.list("worker", &[]);
    assert_eq!(all["v"], 1);
    assert_eq!(
        ids(&all),
        [
            "000000000001",
            "000000000002",
            "000000000003",
            "000000000004"
        ]
    );
    let times: Vec<&str> = all["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["decision"]["time"].as_str().unwrap())
        .collect();
    assert!(times.windows(2).all(|w| w[0] <= w[1]));

    let check = |filters: &[&str], want: &[&str]| {
        let got = ids(&env.list("worker", filters));
        assert_eq!(got, want, "{filters:?}");
    };
    check(&["--kind", "merge"], &["000000000001", "000000000004"]);
    check(&["--decider", "person"], &["000000000002", "000000000004"]);
    check(&["--decider", "check"], &["000000000003"]);
    check(&["--subject", "pr=2"], &["000000000004"]);
    check(&["--run-mode", "unattended"], &["000000000001"]);
    check(
        &["--defaulted", "false"],
        &[
            "000000000001",
            "000000000002",
            "000000000003",
            "000000000004",
        ],
    );
    check(&["--defaulted", "true"], &[]);
    let third = all["decisions"][2]["decision"]["time"].as_str().unwrap();
    check(&["--since", third], &["000000000003", "000000000004"]);
    check(&["--until", third], &["000000000001", "000000000002"]);
    check(
        &["--kind", "merge", "--decider", "person"],
        &["000000000004"],
    );

    // --below lists through the finished middle session.
    let below = env.list("coord", &["--below"]);
    let sessions: Vec<&str> = below["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["session"].as_str().unwrap())
        .collect();
    assert_eq!(
        sessions.iter().filter(|s| **s == "worker").count(),
        4,
        "{below}"
    );
    assert_eq!(
        sessions.iter().filter(|s| **s == "mid").count(),
        1,
        "{below}"
    );
    let below = env.list(
        "coord",
        &["--below", "--kind", "judgment", "--decider", "person"],
    );
    assert_eq!(below["decisions"].as_array().unwrap().len(), 2, "{below}");
}

#[test]
fn reading_changes_no_session() {
    let env = Env::new();
    env.init("director", &[]);
    env.init("w", &["--dispatched-by", "director"]);
    env.init("unrelated", &[]);
    env.record_with(
        &[("CLAUDE_CODE_SESSION_ID", "h-director")],
        "director",
        "judgment",
        "settled",
        &["what=the finding", "pointer=https://x.example/f/1"],
        &[
            "--by",
            "person:dana",
            "--surface",
            "terminal",
            "--reason",
            "it holds",
        ],
    );
    env.record("w", "judgment", "settled", &["what=x"]);
    let anchor = open_anchor(&env, "h-reader", &[], &[]);
    let before = env.snapshot();

    for envs in [
        vec![
            ("CLAUDE_CODE_SESSION_ID", "h-reader"),
            ("KOTO_ANCHOR", anchor.as_str()),
        ],
        vec![("CLAUDE_CODE_SESSION_ID", "h-unrelated")],
    ] {
        let shown = env
            .ok_env(&envs, &["decisions", "show", "director", "000000000001"])
            .json();
        assert_eq!(shown["v"], 1);
        assert_eq!(shown["status"], "in-effect");
        let d = &shown["decision"];
        assert_eq!(d["decider"]["handle"]["value"], "dana");
        assert_eq!(d["decider"]["surface"], "terminal");
        assert_eq!(d["decider"]["entering"]["session"]["value"], "director");
        assert_eq!(d["decider"]["entering"]["harness"]["value"], "h-director");
        assert_eq!(d["subject"]["what"], "the finding");
        assert_eq!(d["reason"], "it holds");
        assert!(d["time"].as_str().unwrap().ends_with('Z'));
        assert!(d["chain"]["links"].is_array());
        env.ok_env(&envs, &["decisions", "list", "director", "--format", "v1"]);
        env.ok_env(
            &envs,
            &["decisions", "list", "director", "--below", "--format", "v1"],
        );
        env.ok_env(&envs, &["decisions", "show", "w", "000000000001"]);
    }
    assert_eq!(env.snapshot(), before, "a read changed a session");

    // A chain read from w's decision names each link's kind.
    let shown = env.ok(&["decisions", "show", "w", "000000000001"]).json();
    assert_eq!(
        shown["decision"]["chain"]["links"],
        serde_json::json!([{"session": "director", "via": "init"}])
    );

    // A decision koto can't read shows unreadable.
    std::fs::write(env.decision_dir("w").join("000000000001"), b"{not json").unwrap();
    let shown = env.ok(&["decisions", "show", "w", "000000000001"]).json();
    assert_eq!(shown["status"], "unreadable", "{shown}");
    let out = env.run(&["decisions", "show", "w", "000000000009"]);
    assert_ne!(out.code, 0);
}

#[test]
fn record_list_and_show_act_on_the_named_session_only() {
    let env = Env::new();
    env.init("named", &[]);
    env.init("other", &[]);
    let anchor = open_anchor(&env, "h-other", &[], &[]);
    let envs = [
        ("CLAUDE_CODE_SESSION_ID", "h-other"),
        ("KOTO_ANCHOR", anchor.as_str()),
        ("KOTO_SESSION_NAME", "other"),
        ("KOTO_SESSION", "other"),
    ];
    let out = env.record_with(
        &envs,
        "named",
        "judgment",
        "settled",
        &["what=x"],
        &["--by", "session"],
    );
    assert_eq!(out["session"], "named");
    assert_eq!(out["decision"]["decider"]["session"]["value"], "named");
    assert_eq!(env.decision_files("named"), ["000000000001"]);
    assert!(env.decision_files("other").is_empty());
    assert!(env.decision_files(&anchor).is_empty());
    let listing = env
        .ok_env(&envs, &["decisions", "list", "named", "--format", "v1"])
        .json();
    assert_eq!(ids(&listing), ["000000000001"]);
    assert_eq!(listing["session"], "named");
    let listing = env
        .ok_env(&envs, &["decisions", "list", "other", "--format", "v1"])
        .json();
    assert!(ids(&listing).is_empty());
    env.ok_env(&envs, &["decisions", "show", "named", "000000000001"]);
}

// ===== Size caps and integrity =====

#[test]
fn an_oversized_body_or_reason_is_refused() {
    let env = Env::new();
    env.init("w", &[]);
    let reason = "r".repeat(4 * 1024 + 1);
    let args = record_args(
        "w",
        "judgment",
        "settled",
        &["what=x"],
        &["--reason", &reason],
    );
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = env.run(&args);
    assert_eq!(out.code, 2, "{}", out.stdout);
    assert_eq!(out.json()["field"], "reason");

    let what = format!("what={}", "w".repeat(17 * 1024));
    let args = record_args("w", "judgment", "settled", &[&what], &[]);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = env.run(&args);
    assert_eq!(out.code, 2, "{}", out.stdout);
    assert!(env.decision_files("w").is_empty());

    // Just under the reason cap records.
    let reason = "r".repeat(4 * 1024);
    env.record_with(
        &[],
        "w",
        "judgment",
        "settled",
        &["what=x"],
        &["--reason", &reason],
    );
}

#[test]
fn files_koto_cannot_trust_list_as_unreadable() {
    let statuses = |env: &Env| -> Vec<(Value, String)> {
        env.list("w", &[])["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| (d["id"].clone(), d["status"].as_str().unwrap().to_string()))
            .collect()
    };
    let fresh = || {
        let env = Env::new();
        env.init("w", &[]);
        env.record("w", "judgment", "settled", &["what=x"]);
        env.record("w", "judgment", "settled", &["what=y"]);
        env
    };

    // A symlink in the area.
    let env = fresh();
    let target = env.root.join("elsewhere");
    std::fs::write(&target, b"{}").unwrap();
    std::os::unix::fs::symlink(&target, env.decision_dir("w").join("000000000003")).unwrap();
    let got = statuses(&env);
    assert!(
        got.contains(&(Value::from("000000000003"), "unreadable".to_string())),
        "{got:?}"
    );
    assert_eq!(
        got.iter().filter(|(_, s)| s == "in-effect").count(),
        2,
        "{got:?}"
    );
    // A symlinked area directory makes the whole session unreadable.
    let env = fresh();
    let dir = env.decision_dir("w");
    let moved = env.root.join("moved");
    std::fs::rename(&dir, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &dir).unwrap();
    let got = statuses(&env);
    assert!(
        !got.is_empty() && got.iter().all(|(_, s)| s == "unreadable"),
        "{got:?}"
    );

    // A name that isn't twelve digits.
    let env = fresh();
    std::fs::write(env.decision_dir("w").join("12345"), b"{}").unwrap();
    let got = statuses(&env);
    assert!(
        got.contains(&(Value::Null, "unreadable".to_string())),
        "{got:?}"
    );
    assert_eq!(got.iter().filter(|(_, s)| s == "in-effect").count(), 2);

    // An id in both areas.
    let env = fresh();
    let aimed = env
        .sessions()
        .join("w")
        .join("ctx")
        .join("koto")
        .join("aimed")
        .join("target-session");
    std::fs::create_dir_all(&aimed).unwrap();
    std::fs::copy(
        env.decision_dir("w").join("000000000002"),
        aimed.join("000000000002"),
    )
    .unwrap();
    let got = statuses(&env);
    assert!(
        got.contains(&(Value::from("000000000002"), "unreadable".to_string())),
        "{got:?}"
    );
    assert!(
        got.contains(&(Value::from("000000000001"), "in-effect".to_string())),
        "{got:?}"
    );

    // An unparseable body.
    let env = fresh();
    std::fs::write(env.decision_dir("w").join("000000000001"), b"not json").unwrap();
    let got = statuses(&env);
    assert!(
        got.contains(&(Value::from("000000000001"), "unreadable".to_string())),
        "{got:?}"
    );
    assert!(
        got.contains(&(Value::from("000000000002"), "in-effect".to_string())),
        "{got:?}"
    );
}

// ===== R30, R35 =====

#[test]
fn harness_pointers_and_a_check_verdict_are_recorded_as_pointers() {
    let env = Env::new();
    env.init("w", &[]);
    let harness = "3f1e2a9c-0b7d-4e55-a1b2-c3d4e5f60722";
    let out = env.record_with(
        &[
            ("CLAUDE_CODE_SESSION_ID", harness),
            ("KOTO_AGENT_ID", "agent-1"),
        ],
        "w",
        "judgment",
        "settled",
        &["what=x"],
        &["--by", "session", "--tool-call", "toolu_01ABC"],
    );
    let id = out["id"].as_str().unwrap();
    let listed = env.list("w", &[]);
    assert_eq!(
        listed["decisions"][0]["decision"]["tool_call"],
        "toolu_01ABC"
    );
    let stored: Value =
        serde_json::from_slice(&std::fs::read(env.decision_dir("w").join(id)).unwrap()).unwrap();
    // The harness's pointers are its session id, the tool call and the
    // agent; nothing else names the harness.
    let mut keys = Vec::new();
    all_keys(&stored, &mut keys);
    let harnessy: Vec<&String> = keys
        .iter()
        .filter(|k| {
            let k = k.to_ascii_lowercase();
            k.contains("harness")
                || k.contains("tool")
                || k.contains("agent")
                || k.contains("transcript")
        })
        .collect();
    assert_eq!(harnessy, ["agent", "harness", "tool_call"], "{stored}");
    assert_eq!(stored["decider"]["harness"]["value"], harness);
    assert_eq!(stored["decider"]["agent"]["value"], "agent-1");

    let out = env.record_with(
        &[],
        "w",
        "verdict",
        "fail",
        &["what=the change", "repository=r", "commit=c1"],
        &["--by", "check:grader", "--evidence", "event:w:3"],
    );
    let d = &out["decision"];
    assert_eq!(d["decider"]["kind"], "check");
    assert_eq!(d["decider"]["name"]["value"], "grader");
    assert_eq!(d["decider"]["session"]["value"], "w");
    let shown = env
        .ok(&["decisions", "show", "w", out["id"].as_str().unwrap()])
        .json();
    assert_eq!(shown["decision"]["evidence"], d["evidence"]);
    let evidence = d["evidence"].to_string();
    assert!(
        evidence.contains("\"w\"") && evidence.contains('3'),
        "{evidence}"
    );
}

#[test]
fn record_list_and_show_work_with_the_network_blocked() {
    let env = Env::new();
    env.init("w", &[]);
    let blocked = [
        ("HTTPS_PROXY", "http://127.0.0.1:9"),
        ("HTTP_PROXY", "http://127.0.0.1:9"),
        ("ALL_PROXY", "http://127.0.0.1:9"),
        ("https_proxy", "http://127.0.0.1:9"),
        ("http_proxy", "http://127.0.0.1:9"),
        ("KOTO_NO_TELEMETRY", "1"),
    ];
    let out = env.record_with(&blocked, "w", "judgment", "settled", &["what=x"], &[]);
    let id = out["id"].as_str().unwrap();
    let listing = env
        .ok_env(&blocked, &["decisions", "list", "w", "--format", "v1"])
        .json();
    assert_eq!(ids(&listing), [id]);
    env.ok_env(&blocked, &["decisions", "show", "w", id]);
}
