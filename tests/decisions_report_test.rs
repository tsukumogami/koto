//! Integration tests for `koto decisions report` (PRD R29): decisions made
//! and reversed over a UTC window, by kind and decider kind.
//!
//! Every test runs the real binary with `HOME` and `KOTO_SESSIONS_BASE`
//! pointed into its own temporary directory, and with every variable koto
//! reads an identity from removed unless the test sets it. Decision times
//! are set by rewriting the stored bodies after they are recorded, so one
//! fixture spans two months.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::path::{Path, PathBuf};

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

/// An id-shaped harness session identifier, so a gate pass with no `--by`
/// is the agent session's, marked defaulted.
const HARNESS: (&str, &str) = (
    "CLAUDE_CODE_SESSION_ID",
    "0f0e0d0c-1111-4222-8333-444455556666",
);

const WORK: &str = r#"---
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

/// One state with a failing, overridable command gate.
const GATED: &str = r#"---
name: gated
version: "1.0"
initial_state: work
states:
  work:
    gates:
      ci:
        type: command
        command: "exit 1"
    transitions:
      - target: done
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
        std::fs::write(root.join("work.md"), WORK).unwrap();
        std::fs::write(root.join("gated.md"), GATED).unwrap();
        Env { _tmp: tmp, root }
    }

    fn sessions(&self) -> PathBuf {
        self.root.join("sessions")
    }

    fn run(&self, envs: &[(&str, &str)], args: &[&str]) -> Out {
        let mut cmd = koto_command::koto_std();
        for name in CLEARED_ENV {
            cmd.env_remove(name);
        }
        cmd.current_dir(self.root.join("ws"));
        cmd.env("HOME", self.root.join("home"));
        cmd.env("KOTO_HOME", self.root.join("home"));
        cmd.env("KOTO_SESSIONS_BASE", self.sessions());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let output = cmd.args(args).output().unwrap();
        Out {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    fn ok(&self, envs: &[(&str, &str)], args: &[&str]) -> Out {
        let out = self.run(envs, args);
        assert_eq!(
            out.code, 0,
            "expected success from {args:?}\n{}\n{}",
            out.stdout, out.stderr
        );
        out
    }

    fn init(&self, name: &str, template: &str, extra: &[&str]) {
        let template = self.root.join(template);
        let mut args = vec!["init", name, "--template", template.to_str().unwrap()];
        args.extend_from_slice(extra);
        self.ok(&[], &args);
    }

    /// Record a decision and set its time; returns its id.
    fn record(&self, name: &str, args: &[&str], time: &str) -> String {
        let mut all = vec!["decisions", "record", name];
        all.extend_from_slice(args);
        let out = self.ok(&[], &all).json();
        let id = out["id"].as_str().unwrap().to_string();
        set_time(
            &self
                .sessions()
                .join(name)
                .join("ctx/koto/decisions")
                .join(&id),
            time,
        );
        id
    }

    fn session_id(&self, name: &str) -> String {
        let log = self
            .sessions()
            .join(name)
            .join(format!("koto-{name}.state.jsonl"));
        let header = std::fs::read_to_string(log).unwrap();
        let header: Value = serde_json::from_str(header.lines().next().unwrap()).unwrap();
        header["session_id"].as_str().unwrap().to_string()
    }

    /// Reverse `name`'s decision `id` from `by_in` and set the reversal's
    /// time; returns where it was recorded and its id.
    fn reverse(&self, name: &str, id: &str, by_in: &str, by: &str, time: &str) -> (String, String) {
        let out = self
            .ok(
                &[],
                &["decisions", "reverse", name, id, "--in", by_in, "--by", by],
            )
            .json();
        let holder = out["session"].as_str().unwrap().to_string();
        let rid = out["id"].as_str().unwrap().to_string();
        assert_eq!(out["decision"]["standing"], "verified", "{out}");
        set_time(
            &self
                .sessions()
                .join(&holder)
                .join("ctx/koto/aimed")
                .join(self.session_id(name))
                .join(&rid),
            time,
        );
        (holder, rid)
    }

    fn report(&self, extra: &[&str]) -> Value {
        let mut args = vec![
            "decisions",
            "report",
            "--since",
            "2026-11-01",
            "--until",
            "2026-12-01",
        ];
        args.extend_from_slice(extra);
        self.ok(&[], &args).json()
    }
}

/// Set the `time` of the decision file at `path`.
fn set_time(path: &Path, time: &str) {
    let mut body: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    body["time"] = json!(time);
    let mut bytes = serde_json::to_vec(&body).unwrap();
    bytes.push(b'\n');
    std::fs::write(path, bytes).unwrap();
}

/// The ids the fixture's decisions got, for the assertions.
struct Fixture {
    env: Env,
    october_call: String,
    november_verdict: String,
}

/// Two months of decisions: a hierarchy `coord` > `w1` > `w2`, a gated
/// session `gated` below `coord`, and an unrelated session `other`.
///
/// October: a judgment in `w1` (reversed by `coord` in November) and a
/// judgment in `w2` (reversed by `coord` in October).
///
/// November, the reported month: a person acceptance in `w1` timed exactly
/// at the window's start; a merge in `w1` timed exactly at its end; a check
/// verdict in `w1` reversed by the same check in November; a `refused` in
/// `w1`; a defaulted gate pass in `gated`; a judgment in `other`.
fn fixture() -> Fixture {
    let env = Env::new();
    env.init("coord", "work.md", &[]);
    env.init("w1", "work.md", &["--dispatched-by", "coord"]);
    env.init("w2", "work.md", &["--dispatched-by", "w1"]);
    env.init("other", "work.md", &[]);
    env.init("gated", "gated.md", &["--dispatched-by", "coord"]);

    let october_call = env.record(
        "w1",
        &[
            "--kind",
            "judgment",
            "--outcome",
            "settled",
            "--subject",
            "what=use sqlite",
            "--by",
            "session",
        ],
        "2026-10-15T12:00:00.000Z",
    );
    let october_w2 = env.record(
        "w2",
        &[
            "--kind",
            "judgment",
            "--outcome",
            "settled",
            "--subject",
            "what=retry twice",
            "--by",
            "session",
        ],
        "2026-10-20T12:00:00.000Z",
    );
    env.record(
        "w1",
        &[
            "--kind",
            "accept",
            "--outcome",
            "accepted",
            "--subject",
            "location=req-1",
            "--by",
            "person:dana",
            "--surface",
            "terminal",
        ],
        "2026-11-01T00:00:00.000Z",
    );
    env.record(
        "w1",
        &[
            "--kind",
            "merge",
            "--outcome",
            "merged",
            "--subject",
            "repository=koto",
            "--subject",
            "pr=12",
            "--subject",
            "commit=abc123",
            "--by",
            "session",
        ],
        "2026-12-01T00:00:00.000Z",
    );
    let november_verdict = env.record(
        "w1",
        &[
            "--kind",
            "verdict",
            "--outcome",
            "pass",
            "--subject",
            "what=the release",
            "--evidence",
            "https://ci.example/runs/1",
            "--by",
            "check:grader",
        ],
        "2026-11-05T08:00:00.000Z",
    );
    env.record(
        "w1",
        &[
            "--kind",
            "refused",
            "--outcome",
            "refused",
            "--subject",
            "kind=merge",
            "--subject",
            "subject=koto#12",
            "--reason",
            "not allowed",
            "--by",
            "session",
        ],
        "2026-11-03T08:00:00.000Z",
    );
    env.record(
        "other",
        &[
            "--kind",
            "judgment",
            "--outcome",
            "settled",
            "--subject",
            "what=elsewhere",
            "--by",
            "session",
        ],
        "2026-11-08T08:00:00.000Z",
    );

    // A defaulted gate pass: an override with no --by from an agent session.
    env.run(&[], &["next", "gated"]);
    env.ok(
        &[HARNESS],
        &[
            "overrides",
            "record",
            "gated",
            "--gate",
            "ci",
            "--rationale",
            "flaky",
        ],
    );
    let listed = env
        .ok(&[], &["decisions", "list", "gated", "--format", "v1"])
        .json();
    let pass_id = listed["decisions"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(
        listed["decisions"][0]["decision"]["decider"]["defaulted"],
        true
    );
    set_time(
        &env.sessions()
            .join("gated/ctx/koto/decisions")
            .join(&pass_id),
        "2026-11-07T08:00:00.000Z",
    );

    // Reversals: November of an October decision, November of a November
    // decision, and October of an October decision.
    env.reverse(
        "w1",
        &october_call,
        "coord",
        "session",
        "2026-11-10T09:00:00.000Z",
    );
    env.reverse(
        "w1",
        &november_verdict,
        "w1",
        "check:grader",
        "2026-11-12T09:00:00.000Z",
    );
    env.reverse(
        "w2",
        &october_w2,
        "coord",
        "session",
        "2026-10-25T09:00:00.000Z",
    );

    Fixture {
        env,
        october_call,
        november_verdict,
    }
}

#[test]
fn the_second_month_counts_what_was_made_and_reversed_in_it() {
    let f = fixture();
    let r = f.env.report(&[]);
    assert_eq!(r["v"], 1);
    assert_eq!(
        r["window"],
        json!({"since": "2026-11-01T00:00:00.000Z", "until": "2026-12-01T00:00:00.000Z"})
    );
    assert_eq!(r["below"], Value::Null);
    assert_eq!(r["sessions_read"], 5, "{r}");

    // Kind by decider kind. The acceptance timed exactly at the start is
    // counted; the merge timed exactly at the end is not; neither the
    // reversals nor the refusal appear; the October decisions don't either.
    assert_eq!(
        r["made"],
        json!({
            "accept": {"person": {"made": 1, "reversed": 0}},
            "gate-pass": {"session": {"made": 1, "reversed": 0}},
            "judgment": {"session": {"made": 1, "reversed": 0}},
            "verdict": {"check": {"made": 1, "reversed": 1}},
        }),
        "{r}"
    );
    for kind in ["reverse", "refused", "merge"] {
        assert!(r["made"].get(kind).is_none(), "{kind} counted: {r}");
    }

    // The window's reversals: November's reversal of the October judgment
    // and of the November verdict, not October's reversal.
    assert_eq!(
        r["reversals"],
        json!({"judgment": {"session": 1}, "verdict": {"check": 1}}),
        "{r}"
    );
    let reversed = r["reversed"].as_array().unwrap();
    assert_eq!(reversed.len(), 2, "{r}");
    assert_eq!(reversed[0]["decision"]["session"], "w1");
    assert_eq!(reversed[0]["decision"]["id"], json!(f.october_call));
    assert_eq!(reversed[0]["decision"]["time"], "2026-10-15T12:00:00.000Z");
    assert_eq!(reversed[0]["reversal"]["session"], "coord");
    assert_eq!(reversed[0]["reversal"]["time"], "2026-11-10T09:00:00.000Z");
    assert_eq!(reversed[0]["reversal"]["standing"], "verified");
    assert_eq!(reversed[1]["decision"]["id"], json!(f.november_verdict));
    assert_eq!(reversed[1]["decision"]["kind"], "verdict");
    assert_eq!(reversed[1]["reversal"]["session"], "w1");
    assert_eq!(reversed[1]["reversal"]["decider_kind"], "check");

    assert_eq!(r["defaulted_gate_passes"], 1, "{r}");
    assert_eq!(r["unreadable"], json!([]));
}

#[test]
fn the_window_includes_its_start_and_excludes_its_end() {
    let f = fixture();
    // A window ending exactly at the acceptance's time leaves it out; one
    // starting exactly at the merge's time takes it in.
    let before = f
        .env
        .ok(
            &[],
            &[
                "decisions",
                "report",
                "--since",
                "2026-10-01T00:00:00Z",
                "--until",
                "2026-11-01T00:00:00Z",
            ],
        )
        .json();
    assert!(before["made"].get("accept").is_none(), "{before}");
    assert_eq!(
        before["made"]["judgment"]["session"],
        json!({"made": 2, "reversed": 2}),
        "{before}"
    );
    assert_eq!(before["reversals"], json!({"judgment": {"session": 1}}));

    let after = f
        .env
        .ok(
            &[],
            &[
                "decisions",
                "report",
                "--since",
                "2026-12-01T00:00:00.000Z",
                "--until",
                "2027-01-01",
            ],
        )
        .json();
    assert_eq!(
        after["made"],
        json!({"merge": {"session": {"made": 1, "reversed": 0}}}),
        "{after}"
    );
}

#[test]
fn below_a_root_counts_only_that_hierarchy() {
    let f = fixture();
    let r = f.env.report(&["--below", "coord"]);
    assert_eq!(r["below"], "coord");
    assert_eq!(r["sessions_read"], 4, "{r}");
    // `other`'s November judgment is outside the hierarchy.
    assert_eq!(
        r["made"],
        json!({
            "accept": {"person": {"made": 1, "reversed": 0}},
            "gate-pass": {"session": {"made": 1, "reversed": 0}},
            "verdict": {"check": {"made": 1, "reversed": 1}},
        }),
        "{r}"
    );
    assert_eq!(
        r["reversals"],
        json!({"judgment": {"session": 1}, "verdict": {"check": 1}})
    );
    assert_eq!(r["defaulted_gate_passes"], 1);

    let other = f.env.report(&["--below", "other"]);
    assert_eq!(other["sessions_read"], 1);
    assert_eq!(
        other["made"],
        json!({"judgment": {"session": {"made": 1, "reversed": 0}}})
    );
    assert_eq!(other["reversals"], json!({}));
    assert_eq!(other["reversed"], json!([]));
    assert_eq!(other["defaulted_gate_passes"], 0);
}

#[test]
fn a_bad_window_or_root_is_refused() {
    let env = Env::new();
    env.init("w", "work.md", &[]);
    for (since, until, field) in [
        ("2026-11-01", "2026-11-01", "until"),
        ("2026-12-01", "2026-11-01", "until"),
        ("2026-11-01T00:00:00+02:00", "2026-12-01", "since"),
        ("2026-11-01", "soon", "until"),
    ] {
        let out = env.run(
            &[],
            &["decisions", "report", "--since", since, "--until", until],
        );
        assert_eq!(out.code, 2, "{since} {until}: {}", out.stdout);
        let err = out.json();
        assert_eq!(err["code"], "invalid_decision");
        assert_eq!(err["field"], field, "{err}");
    }
    let out = env.run(
        &[],
        &[
            "decisions",
            "report",
            "--since",
            "2026-11-01",
            "--until",
            "2026-12-01",
            "--below",
            "nobody",
        ],
    );
    assert_eq!(out.code, 2, "{}", out.stdout);
}
