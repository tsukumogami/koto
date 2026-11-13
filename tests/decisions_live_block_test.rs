//! Integration tests for the reversal read at `koto next` under the local
//! backend (PRD R16, R22, R23): a live workflow whose decision was reversed
//! from a session on its chain refuses to advance until a rewind or a
//! replacement from someone with standing clears it, and fails closed when
//! a chain session can't be read.
//!
//! Every test runs the real binary with `HOME` and `KOTO_SESSIONS_BASE`
//! pointed into its own temporary directory, and with every variable koto
//! reads an identity from removed unless the test sets it.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use assert_fs::TempDir;
use serde_json::Value;

const CLEARED_ENV: [&str; 6] = [
    "CLAUDE_CODE_SESSION_ID",
    "KOTO_ANCHOR",
    "KOTO_AGENT_ID",
    "KOTO_RUN_MODE",
    "KOTO_AUTHORITY",
    "KOTO_TEST_DECISIONS_FAULT",
];

const REVERSED: &str = "__decision_reversed__";
const UNREADABLE: &str = "__decision_unreadable__";

/// The template of a session nothing dispatched.
const TOP_TEMPLATE: &str = r#"---
name: top
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

/// The template a request's leg names: `work`, `review` (which can loop
/// back to `work`), `check`, then `done`.
const LEG_TEMPLATE: &str = r#"---
name: leg
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
        values: [next]
    transitions:
      - target: review
        when:
          go: next
  review:
    accepts:
      go:
        type: enum
        required: true
        values: [next, back]
    transitions:
      - target: check
        when:
          go: next
      - target: work
        when:
          go: back
  check:
    accepts:
      go:
        type: enum
        required: true
        values: [finish]
    transitions:
      - target: done
        when:
          go: finish
  done:
    terminal: true
---

## work

Work on {{TOPIC}}.

## review

Review.

## check

Check.

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

/// A recorded decision: its session, id and the session's id.
#[derive(Clone)]
struct Rec {
    session: String,
    id: String,
    session_id: String,
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        for dir in ["home", "ws", "sessions"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("top.md"), TOP_TEMPLATE).unwrap();
        std::fs::write(root.join("ws").join("leg.md"), LEG_TEMPLATE).unwrap();
        std::fs::write(root.join("vars.json"), r#"[["TOPIC","t1"]]"#).unwrap();
        Env { _tmp: tmp, root }
    }

    fn sessions(&self) -> PathBuf {
        self.root.join("sessions")
    }

    fn run_env(&self, envs: &[(&str, &str)], args: &[&str]) -> Out {
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

    /// A session nothing dispatched; `extra` adds init flags.
    fn top(&self, name: &str, extra: &[&str]) {
        let template = self.root.join("top.md");
        let mut args = vec!["init", name, "--template", template.to_str().unwrap()];
        args.extend_from_slice(extra);
        self.ok(&args);
    }

    /// `name`, started as the leg of a request `coordinator` owns, so it
    /// holds a dispatched-by link to `coordinator` written by the leg.
    fn leg(&self, coordinator: &str, name: &str, envs: &[(&str, &str)]) {
        let req = self
            .ok(&[
                "request",
                "create",
                "--with-data",
                r#"{"legs":[{"name":"scope","role":"scope","template":"leg.md","inputs":{"TOPIC":"t1"}}]}"#,
                "--requested-by",
                coordinator,
                "--coordinator-of-record",
                coordinator,
            ])
            .json()["request_id"]
            .as_str()
            .unwrap()
            .to_string();
        let template = self.root.join("ws").join("leg.md");
        let vars = self.root.join("vars.json");
        self.ok_env(
            envs,
            &[
                "init",
                name,
                "--template",
                template.to_str().unwrap(),
                "--vars-file",
                vars.to_str().unwrap(),
                "--koto-leg",
                &format!("{req}:scope"),
            ],
        );
    }

    fn record(
        &self,
        name: &str,
        kind: &str,
        outcome: &str,
        subject: &[&str],
        extra: &[&str],
    ) -> Rec {
        let mut args = vec![
            "decisions",
            "record",
            name,
            "--kind",
            kind,
            "--outcome",
            outcome,
        ];
        for s in subject {
            args.push("--subject");
            args.push(s);
        }
        args.extend_from_slice(extra);
        let out = self.ok(&args).json();
        Rec {
            session: name.to_string(),
            id: out["id"].as_str().unwrap().to_string(),
            session_id: out["decision"]["session"]["id"]
                .as_str()
                .unwrap()
                .to_string(),
        }
    }

    /// A `judgment` in `name` by the session.
    fn judgment(&self, name: &str, what: &str) -> Rec {
        let subject = format!("what={what}");
        self.record(
            name,
            "judgment",
            "settled",
            &[&subject],
            &["--by", "session"],
        )
    }

    /// A `judgment` in `name` naming `basis`, decided as `extra` says.
    fn replacement(&self, name: &str, basis: &Rec, extra: &[&str]) -> Rec {
        let basis = format!("{}:{}", basis.session, basis.id);
        let mut args = vec!["--basis", basis.as_str()];
        args.extend_from_slice(extra);
        self.record(name, "judgment", "settled", &["what=again"], &args)
    }

    /// Reverse `of` from `by_in` and expect success; `extra` names the
    /// decider, the session otherwise.
    fn reverse(&self, of: &Rec, by_in: &str, extra: &[&str]) -> Rec {
        let mut args = vec![
            "decisions",
            "reverse",
            of.session.as_str(),
            of.id.as_str(),
            "--in",
            by_in,
        ];
        if !extra.contains(&"--by") {
            args.extend_from_slice(&["--by", "session"]);
        }
        args.extend_from_slice(extra);
        let out = self.ok(&args).json();
        let d = &out["decision"];
        Rec {
            session: d["session"]["name"].as_str().unwrap().to_string(),
            id: d["id"].as_str().unwrap().to_string(),
            session_id: d["session"]["id"].as_str().unwrap().to_string(),
        }
    }

    fn next(&self, name: &str, extra: &[&str]) -> Out {
        let mut args = vec!["next", name];
        args.extend_from_slice(extra);
        self.run(&args)
    }

    fn advance(&self, name: &str, go: &str) {
        let data = format!(r#"{{"go":"{go}"}}"#);
        let out = self.next(name, &["--with-data", &data]);
        assert_eq!(
            out.code, 0,
            "advance {name}: {}\n{}",
            out.stdout, out.stderr
        );
    }

    fn state(&self, name: &str) -> String {
        self.ok(&["status", name]).json()["current_state"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn state_file(&self, name: &str) -> PathBuf {
        self.sessions()
            .join(name)
            .join(format!("koto-{name}.state.jsonl"))
    }

    fn aimed_dir(&self, session: &str, target_id: &str) -> PathBuf {
        self.sessions()
            .join(session)
            .join("ctx/koto/aimed")
            .join(target_id)
    }

    /// Every file under `session`'s directory and its bytes, locks aside.
    fn snapshot(&self, session: &str) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut out = BTreeMap::new();
        let dir = self.sessions().join(session);
        snapshot_into(&dir, &dir, &mut out);
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
        } else if meta.is_file() && !entry.file_name().to_string_lossy().ends_with(".lock") {
            out.insert(
                path.strip_prefix(base).unwrap().to_path_buf(),
                std::fs::read(&path).unwrap(),
            );
        }
    }
}

/// The blocking condition called `name` in a `koto next` response.
fn condition<'a>(resp: &'a Value, name: &str) -> Option<&'a Value> {
    resp["blocking_conditions"]
        .as_array()?
        .iter()
        .find(|c| c["name"] == name)
}

/// The ids `__decision_reversed__` lists.
fn reversed_ids(resp: &Value) -> Vec<String> {
    condition(resp, REVERSED)
        .map(|c| {
            c["output"]["decisions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|d| d["id"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The sessions `__decision_unreadable__` names.
fn unreadable_sessions(resp: &Value) -> Vec<String> {
    condition(resp, UNREADABLE)
        .map(|c| {
            c["output"]["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s["session"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Expect `koto next` on `name` to stop on `__decision_reversed__` listing
/// exactly `ids`; returns the response.
fn assert_blocked(env: &Env, name: &str, ids: &[&str]) -> Value {
    let out = env.next(name, &[]);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    let resp = out.json();
    assert_eq!(resp["action"], "gate_blocked", "{resp}");
    assert_eq!(resp["advanced"], false, "{resp}");
    assert_eq!(reversed_ids(&resp), ids, "{resp}");
    resp
}

/// Expect `koto next` on `name` not to stop on a decision condition.
fn assert_not_blocked(env: &Env, name: &str) -> Value {
    let out = env.next(name, &[]);
    let resp = out.json();
    assert!(
        condition(&resp, REVERSED).is_none() && condition(&resp, UNREADABLE).is_none(),
        "expected no decision condition: {resp}\n{}",
        out.stderr
    );
    resp
}

/// `a` dispatches `b` and `b` dispatches `c`, each through a request leg.
fn three_levels(env: &Env) {
    env.top("a", &[]);
    env.leg("a", "b", &[]);
    env.leg("b", "c", &[]);
}

/// Drive `b` to its terminal state.
fn finish(env: &Env, name: &str) {
    for go in ["next", "next", "finish"] {
        env.advance(name, go);
    }
    assert_eq!(env.state(name), "done");
}

// ===== R22: a reversal from two levels up blocks the live worker =====

#[test]
fn a_reversal_from_two_levels_up_blocks_the_live_worker() {
    let env = Env::new();
    three_levels(&env);
    let judgment = env.judgment("c", "approach");
    finish(&env, "b");
    let reversal = env.reverse(&judgment, "a", &[]);

    let log_before = std::fs::read_to_string(env.state_file("c")).unwrap();
    let resp = assert_blocked(&env, "c", &[&judgment.id]);
    let cond = condition(&resp, REVERSED).unwrap();
    assert_eq!(cond["type"], "decision");
    assert_eq!(cond["category"], "corrective");
    assert_eq!(cond["agent_actionable"], false);
    let listed = &cond["output"]["decisions"][0];
    assert_eq!(listed["session"], "c");
    assert_eq!(listed["kind"], "judgment");
    assert_eq!(listed["state"], "work");
    assert_eq!(listed["reversal"]["session"], "a");
    assert_eq!(listed["reversal"]["id"], reversal.id.as_str());
    assert_eq!(listed["reversal"]["standing"], "verified");
    assert_eq!(listed["decider"]["kind"], "session");
    assert_eq!(listed["decider"]["session"]["value"], "a");
    assert_eq!(resp["state"], "work");
    assert_eq!(env.state("c"), "work");
    // The response's own delivery record is the one thing the tick may
    // append: no transition, evidence or check event.
    let after = std::fs::read_to_string(env.state_file("c")).unwrap();
    assert!(after.starts_with(&log_before));
    for line in after.lines().skip(log_before.lines().count()) {
        let event: Value = serde_json::from_str(line).unwrap();
        assert_eq!(event["type"], "instructions_delivered", "{line}");
    }
    let log_before = after;

    // Evidence and a directed transition are refused too, with nothing
    // appended.
    for extra in [
        vec!["--with-data", r#"{"go":"next"}"#],
        vec!["--to", "review"],
    ] {
        let out = env.next("c", &extra);
        assert_eq!(out.code, 2, "{extra:?}: {}\n{}", out.stdout, out.stderr);
        let err = out.json();
        assert_eq!(err["error"]["code"], "precondition_failed", "{err}");
        assert_eq!(err["blocking_conditions"][0]["name"], REVERSED, "{err}");
    }
    assert_eq!(env.state("c"), "work");
    assert_eq!(
        std::fs::read_to_string(env.state_file("c")).unwrap(),
        log_before
    );
}

// ===== R16, R23: reads and the blocked tick write nothing elsewhere =====

#[test]
fn reads_and_reversals_change_no_session_below_or_between() {
    let env = Env::new();
    three_levels(&env);
    let judgment = env.judgment("c", "approach");
    finish(&env, "b");

    let (b0, c0) = (env.snapshot("b"), env.snapshot("c"));
    env.ok(&["decisions", "list", "c", "--format", "v1"]);
    env.ok(&["decisions", "show", "c", &judgment.id]);
    env.ok(&["decisions", "list", "a", "--below", "--format", "v1"]);
    env.reverse(&judgment, "a", &[]);
    env.ok(&["decisions", "list", "a", "--below", "--format", "v1"]);
    assert_eq!(env.snapshot("b"), b0, "b changed");
    assert_eq!(env.snapshot("c"), c0, "c changed");
}

#[test]
fn the_blocked_tick_changes_no_session_above() {
    let env = Env::new();
    three_levels(&env);
    let judgment = env.judgment("c", "approach");
    finish(&env, "b");
    env.reverse(&judgment, "a", &[]);

    let (a0, b0) = (env.snapshot("a"), env.snapshot("b"));
    assert_blocked(&env, "c", &[&judgment.id]);
    assert_blocked(&env, "c", &[&judgment.id]);
    assert_eq!(env.snapshot("a"), a0, "a changed");
    assert_eq!(env.snapshot("b"), b0, "b changed");
}

// ===== R22: what clears the block =====

#[test]
fn a_rewind_to_the_decisions_state_clears_and_a_later_one_does_not() {
    let env = Env::new();
    three_levels(&env);
    // `koto rewind` steps back one state, so each worker rewinds once:
    // `c` back to the state its decision recorded, `d` to a state it
    // entered after its decision.
    env.leg("b", "d", &[]);
    let at_c = env.judgment("c", "approach");
    let at_d = env.judgment("d", "approach");
    env.advance("c", "next");
    env.advance("d", "next");
    env.advance("d", "next");
    assert_eq!(env.state("c"), "review");
    assert_eq!(env.state("d"), "check");
    env.reverse(&at_c, "a", &[]);
    env.reverse(&at_d, "a", &[]);
    assert_blocked(&env, "c", &[&at_c.id]);
    assert_blocked(&env, "d", &[&at_d.id]);

    env.ok(&["rewind", "d"]);
    assert_eq!(env.state("d"), "review");
    assert_blocked(&env, "d", &[&at_d.id]);

    env.ok(&["rewind", "c"]);
    assert_eq!(env.state("c"), "work");
    let resp = assert_not_blocked(&env, "c");
    assert_eq!(resp["action"], "evidence_required", "{resp}");
    env.advance("c", "next");
    assert_eq!(env.state("c"), "review");
    assert_not_blocked(&env, "c");
}

#[test]
fn a_replacement_by_the_worker_leaves_it_blocked_and_one_by_the_reverser_clears() {
    let env = Env::new();
    three_levels(&env);
    let judgment = env.judgment("c", "approach");
    let reversal = env.reverse(&judgment, "a", &[]);
    assert_blocked(&env, "c", &[&judgment.id]);

    let own = env.replacement("c", &reversal, &["--by", "session"]);
    assert!(
        env.aimed_dir("c", &judgment.session_id)
            .join(&own.id)
            .is_file(),
        "a replacement is recorded in the aimed area"
    );
    assert_blocked(&env, "c", &[&judgment.id]);

    let from_above = env.replacement("a", &reversal, &["--by", "session"]);
    assert!(env
        .aimed_dir("a", &judgment.session_id)
        .join(&from_above.id)
        .is_file());
    assert_not_blocked(&env, "c");
    env.advance("c", "next");
}

#[test]
fn a_reversed_gate_pass_clears_only_with_a_verified_pass_from_above() {
    let env = Env::new();
    // `a`'s own chain is broken once `z` is gone, so a pass from beyond it
    // is asserted.
    env.top("z", &[]);
    env.top("a", &["--dispatched-by", "z"]);
    env.leg("a", "b", &[]);
    let anchor = open_anchor(&env, "6a1f0c2e-7d4b-4c8e-9a10-b2c3d4e5f607");
    env.leg("b", "c", &[("KOTO_ANCHOR", &anchor)]);
    std::fs::remove_dir_all(env.sessions().join("z")).unwrap();

    let gate = ["workflow=c", "state=work", "gate=g"];
    let pass = env.record("c", "gate-pass", "passed", &gate, &["--by", "session"]);
    let reversal = env.reverse(&pass, "a", &[]);
    assert_blocked(&env, "c", &[&pass.id]);

    let basis = format!("a:{}", reversal.id);
    let by = ["--by", "session", "--basis", basis.as_str()];
    env.record("c", "gate-pass", "passed", &gate, &by);
    assert_blocked(&env, "c", &[&pass.id]);

    // The anchor is on c's chain but not above `a`, and `a`'s chain is
    // incomplete: its pass is asserted, which doesn't clear a verified
    // reversal.
    let asserted = env.record(&anchor, "gate-pass", "passed", &gate, &by);
    assert!(env
        .aimed_dir(&anchor, &pass.session_id)
        .join(&asserted.id)
        .is_file());
    assert_blocked(&env, "c", &[&pass.id]);

    // A pass for another gate from `a` doesn't clear it either.
    let other = ["workflow=c", "state=work", "gate=h"];
    env.record("a", "gate-pass", "passed", &other, &by);
    assert_blocked(&env, "c", &[&pass.id]);

    env.record("a", "gate-pass", "passed", &gate, &by);
    assert_not_blocked(&env, "c");
}

#[test]
fn a_looping_workflow_is_blocked_only_by_the_named_decision() {
    let env = Env::new();
    three_levels(&env);
    let first = env.judgment("c", "first visit");
    env.advance("c", "next");
    env.advance("c", "back");
    assert_eq!(env.state("c"), "work");
    let second = env.judgment("c", "second visit");

    env.reverse(&first, "a", &[]);
    assert_blocked(&env, "c", &[&first.id]);
    env.reverse(&second, "a", &[]);
    assert_blocked(&env, "c", &[&first.id, &second.id]);

    // A rewind to a state entered after the first visit's decision, but
    // before the second's, clears only the second.
    env.ok(&["rewind", "c"]);
    assert_eq!(env.state("c"), "review");
    assert_blocked(&env, "c", &[&first.id]);
}

#[test]
fn a_person_reversal_through_the_worker_clears_only_with_a_person_replacement() {
    let env = Env::new();
    three_levels(&env);
    let person = ["--by", "person:dana", "--surface", "terminal"];
    let judgment = env.record("c", "judgment", "settled", &["what=x"], &person);
    let reversal = env.reverse(&judgment, "c", &person);
    assert_eq!(reversal.session, "c");
    assert_blocked(&env, "c", &[&judgment.id]);

    env.replacement("c", &reversal, &["--by", "session"]);
    assert_blocked(&env, "c", &[&judgment.id]);

    let by_person = env.replacement("c", &reversal, &person);
    assert!(env
        .aimed_dir("c", &judgment.session_id)
        .join(&by_person.id)
        .is_file());
    assert_not_blocked(&env, "c");
}

// ===== R23: reads fail closed =====

#[test]
fn a_malformed_reversal_in_a_chain_session_blocks_as_unreadable() {
    let env = Env::new();
    three_levels(&env);
    let judgment = env.judgment("c", "approach");
    env.reverse(&judgment, "a", &[]);
    let dir = env.aimed_dir("b", &judgment.session_id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("000000000099"), b"{not json").unwrap();

    let resp = env.next("c", &[]).json();
    assert_eq!(resp["action"], "gate_blocked", "{resp}");
    assert_eq!(unreadable_sessions(&resp), ["b"], "{resp}");
    assert_eq!(condition(&resp, UNREADABLE).unwrap()["type"], "decision");
    assert_eq!(reversed_ids(&resp), [judgment.id.as_str()], "{resp}");
}

#[test]
fn a_removed_middle_session_ends_the_chain_incomplete() {
    let env = Env::new();
    three_levels(&env);
    let judgment = env.judgment("c", "approach");
    env.reverse(&judgment, "a", &[]);
    let dir = env.aimed_dir("b", &judgment.session_id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("000000000099"), b"{not json").unwrap();
    std::fs::remove_dir_all(env.sessions().join("b")).unwrap();

    let resp = assert_not_blocked(&env, "c");
    assert_eq!(resp["action"], "evidence_required", "{resp}");
    env.advance("c", "next");
    let later = env.record(
        "c",
        "judgment",
        "settled",
        &["what=later"],
        &["--by", "session"],
    );
    let shown = env.ok(&["decisions", "show", "c", &later.id]).json();
    assert_eq!(shown["decision"]["chain"]["incomplete"], true, "{shown}");
}

#[test]
fn a_chain_longer_than_64_blocks_as_unreadable() {
    let env = Env::new();
    env.top("s0", &[]);
    for i in 1..=65 {
        let above = format!("s{}", i - 1);
        env.top(&format!("s{i}"), &["--dispatched-by", &above]);
    }
    env.judgment("s65", "deep");
    let resp = env.next("s65", &[]).json();
    assert_eq!(resp["action"], "gate_blocked", "{resp}");
    assert_eq!(unreadable_sessions(&resp), ["s0"], "{resp}");
    let reason = condition(&resp, UNREADABLE).unwrap()["output"]["sessions"][0]["reason"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(reason.contains("64"), "{reason}");
}

#[test]
fn an_unreadable_session_off_the_chain_blocks_nothing() {
    let env = Env::new();
    three_levels(&env);
    env.top("other", &[]);
    let judgment = env.judgment("c", "approach");
    let dir = env.aimed_dir("other", &judgment.session_id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("000000000001"), b"{not json").unwrap();
    std::fs::write(
        env.sessions()
            .join("other/ctx/koto/aimed")
            .join("not-an-id!"),
        b"x",
    )
    .unwrap();
    let resp = assert_not_blocked(&env, "c");
    assert_eq!(resp["action"], "evidence_required", "{resp}");
}

#[test]
fn a_session_without_decisions_reads_no_chain_session() {
    let env = Env::new();
    three_levels(&env);
    let backend = koto::session::Backend::Local(koto::session::local::LocalBackend::with_base_dir(
        env.sessions(),
    ));
    let id = session_id(&env, "c");
    let read = koto::decisions::read::aimed_on_chain(&backend, "c", &id);
    assert!(read.skipped);
    assert_eq!(read.reads, koto::decisions::read::Reads::default());
    assert!(read.sessions.is_empty());

    env.judgment("c", "approach");
    let read = koto::decisions::read::aimed_on_chain(&backend, "c", &id);
    assert!(!read.skipped);
    assert_eq!(read.sessions, ["c", "b", "a"]);
    assert!(read.reads.links_listed > 0 && read.reads.aimed_listed == 3);
    assert_eq!(read.reads.files_read, 0);
}

// ===== R22 with an imported session =====

#[test]
fn a_reversal_aimed_at_the_pre_import_id_blocks() {
    let env = Env::new();
    three_levels(&env);
    let judgment = env.judgment("c", "approach");
    let reversal = env.reverse(&judgment, "a", &[]);
    // Turn `c` into the copy an import made of a session whose id was the
    // reversal's target: the reversal is now aimed at an earlier id, and
    // `koto/imported-from` names it in today's single-source form.
    let earlier = "0b9c2d4e-1f3a-4b5c-8d6e-7f8091a2b3c4";
    let reversal_path = env.aimed_dir("a", &judgment.session_id).join(&reversal.id);
    let mut body: Value = serde_json::from_slice(&std::fs::read(&reversal_path).unwrap()).unwrap();
    body["target"]["id"] = Value::from(earlier);
    let moved = env.aimed_dir("a", earlier);
    std::fs::create_dir_all(&moved).unwrap();
    std::fs::write(moved.join(&reversal.id), serde_json::to_vec(&body).unwrap()).unwrap();
    std::fs::remove_file(&reversal_path).unwrap();
    assert_not_blocked(&env, "c");

    std::fs::write(
        env.sessions().join("c/ctx/koto/imported-from"),
        serde_json::to_vec(&serde_json::json!({
            "from_session": "c",
            "from_session_id": earlier,
        }))
        .unwrap(),
    )
    .unwrap();
    assert_blocked(&env, "c", &[&judgment.id]);
}

// ===== Template compile: the condition names are reserved =====

#[test]
fn a_gate_named_after_a_decision_condition_is_refused() {
    let env = Env::new();
    for name in [REVERSED, UNREADABLE] {
        let template = TOP_TEMPLATE.replace(
            "    accepts:\n",
            &format!(
                "    gates:\n      {name}:\n        type: command\n        command: \"true\"\n    accepts:\n"
            ),
        );
        let path = env.root.join(format!("{name}.md"));
        std::fs::write(&path, template).unwrap();
        let out = env.run(&["init", "wf", "--template", path.to_str().unwrap()]);
        assert_ne!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
        let all = format!("{}{}", out.stdout, out.stderr);
        assert!(
            all.contains("reserved for koto's decision conditions"),
            "{all}"
        );
    }
}

/// Open an anchor for `harness` and return its name.
fn open_anchor(env: &Env, harness: &str) -> String {
    let opened = env.run(&[
        "session",
        "anchor",
        "open",
        "--harness-session",
        harness,
        "--source",
        "startup",
    ]);
    assert_eq!(opened.code, 0, "{}", opened.stderr);
    opened
        .stdout
        .lines()
        .find_map(|l| l.strip_prefix("anchor="))
        .unwrap()
        .to_string()
}

/// The header id of `name`, from its state file.
fn session_id(env: &Env, name: &str) -> String {
    let log = std::fs::read_to_string(env.state_file(name)).unwrap();
    let header: Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
    header["session_id"].as_str().unwrap().to_string()
}
