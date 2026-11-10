//! Integration tests for `koto decisions reverse` under the local backend:
//! standing, refusals, racing reversals and the statuses every reader
//! computes from them (PRD R4, R17 to R21, R44).
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
use serde_json::{json, Value};

/// Variables koto reads an identity, a run mode or an authority from, and
/// the reversal fault seam.
const CLEARED_ENV: [&str; 6] = [
    "CLAUDE_CODE_SESSION_ID",
    "KOTO_ANCHOR",
    "KOTO_AGENT_ID",
    "KOTO_RUN_MODE",
    "KOTO_AUTHORITY",
    "KOTO_TEST_DECISIONS_FAULT",
];

/// The fault that hides every competing reversal from `reverse`.
const HIDE_ALL: (&str, &str) = ("KOTO_TEST_DECISIONS_FAULT", "hide_competitors");

/// The fault that hides competing reversals only from the check made
/// before the reversal is written.
const HIDE_BEFORE: (&str, &str) = ("KOTO_TEST_DECISIONS_FAULT", "hide_competitors_before_write");

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
        std::fs::write(root.join("work.md"), TEMPLATE).unwrap();
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

    fn ok(&self, args: &[&str]) -> Out {
        let out = self.run_env(&[], args);
        assert_eq!(
            out.code, 0,
            "expected success from {args:?}\n{}\n{}",
            out.stdout, out.stderr
        );
        out
    }

    fn init(&self, name: &str, extra: &[&str]) {
        let template = self.root.join("work.md");
        let mut args = vec!["init", name, "--template", template.to_str().unwrap()];
        args.extend_from_slice(extra);
        self.ok(&args);
    }

    /// Record a decision; `extra` names the decider.
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

    /// A session decision in `name`.
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

    fn reverse_env(&self, envs: &[(&str, &str)], of: &Rec, by_in: &str, extra: &[&str]) -> Out {
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
        self.run_env(envs, &args)
    }

    fn reverse(&self, of: &Rec, by_in: &str, extra: &[&str]) -> Out {
        self.reverse_env(&[], of, by_in, extra)
    }

    /// Reverse and expect success; returns the printed decision.
    fn reversed(&self, of: &Rec, by_in: &str, extra: &[&str]) -> Value {
        let out = self.reverse(of, by_in, extra);
        assert_eq!(
            out.code, 0,
            "reverse failed:\n{}\n{}",
            out.stdout, out.stderr
        );
        out.json()["decision"].clone()
    }

    fn list(&self, name: &str, extra: &[&str]) -> Value {
        let mut args = vec!["decisions", "list", name, "--format", "v1"];
        args.extend_from_slice(extra);
        self.ok(&args).json()
    }

    /// Every file under `session`'s directory and its bytes, locks aside.
    fn snapshot(&self, session: &str) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut out = BTreeMap::new();
        let dir = self.sessions().join(session);
        snapshot_into(&dir, &dir, &mut out);
        out
    }

    fn aimed_dir(&self, session: &str, target_id: &str) -> PathBuf {
        self.sessions()
            .join(session)
            .join("ctx/koto/aimed")
            .join(target_id)
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

/// The listed entry for `session`'s `id`.
fn entry<'a>(listing: &'a Value, session: &str, id: &str) -> &'a Value {
    listing["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["session"] == session && d["id"] == id)
        .unwrap_or_else(|| panic!("no {session}:{id} in {listing}"))
}

/// The `refused` entries a listing holds, newest last.
fn refusals(listing: &Value) -> Vec<Value> {
    listing["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["decision"]["kind"] == "refused")
        .cloned()
        .collect()
}

/// A coordinator and a worker it dispatched.
fn coordinator_and_worker(env: &Env) {
    env.init("coord", &[]);
    env.init("w", &["--dispatched-by", "coord"]);
}

// ===== R4, R17, R20: reversing from above =====

#[test]
fn a_coordinator_reverses_a_finished_workers_decisions() {
    let env = Env::new();
    coordinator_and_worker(&env);
    let by_session = ["--by", "session"];
    let accept = env.record("w", "accept", "accepted", &["location=req-1"], &by_session);
    let gate = env.record(
        "w",
        "gate-pass",
        "passed",
        &["workflow=w", "state=work", "gate=panel"],
        &by_session,
    );
    let merge = env.record(
        "w",
        "merge",
        "merged",
        &["repository=r", "pr=7", "commit=abc"],
        &by_session,
    );
    let before = env.snapshot("w");

    let mut reversals = Vec::new();
    for original in [&accept, &gate, &merge] {
        let d = env.reversed(original, "coord", &["--reason", "wrong call"]);
        assert_eq!(d["kind"], "reverse");
        assert_eq!(d["session"]["name"], "coord");
        assert_eq!(d["standing"], "verified");
        assert_eq!(d["target"]["name"], "w");
        assert_eq!(d["target"]["id"], original.session_id.as_str());
        assert_eq!(d["target"]["decision"], original.id.as_str());
        assert_eq!(
            d["path"],
            json!([{"session": "w", "via": d["path"][0]["via"], "to": "coord"}])
        );
        let rid = d["id"].as_str().unwrap().to_string();
        assert!(
            env.aimed_dir("coord", &original.session_id)
                .join(&rid)
                .is_file(),
            "no key under coord's koto/aimed/<worker-id>/"
        );
        reversals.push(rid);
    }

    let listing = env.list("w", &[]);
    for (original, rid) in [&accept, &gate, &merge].iter().zip(&reversals) {
        let e = entry(&listing, "w", &original.id);
        assert_eq!(e["status"], "reversed", "{e}");
        assert_eq!(e["reversed_by"]["session"], "coord");
        assert_eq!(e["reversed_by"]["id"], rid.as_str());
    }
    assert!(
        listing["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["status"] != "in-effect"),
        "{listing}"
    );
    let coord = env.list("coord", &[]);
    for rid in &reversals {
        assert_eq!(entry(&coord, "coord", rid)["status"], "in-effect");
    }
    assert_eq!(
        env.snapshot("w"),
        before,
        "the worker's keys or log changed"
    );
}

#[test]
fn a_reverser_off_the_chain_records_in_its_anchor_when_that_is_on_it() {
    let env = Env::new();
    let harness = "3f1e2a9c-0b7d-4e55-a1b2-c3d4e5f60731";
    let opened = env.run_env(
        &[],
        &[
            "session",
            "anchor",
            "open",
            "--harness-session",
            harness,
            "--source",
            "startup",
        ],
    );
    assert_eq!(opened.code, 0, "{}", opened.stderr);
    let anchor = opened
        .stdout
        .lines()
        .find_map(|l| l.strip_prefix("anchor="))
        .unwrap()
        .to_string();
    let template = env.root.join("work.md");
    let held = [("KOTO_ANCHOR", anchor.as_str())];
    let out = env.run_env(
        &held,
        &["init", "w", "--template", template.to_str().unwrap()],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    env.init("outsider", &[]);
    let call = env.judgment("w", "use postgres");

    let out = env.reverse_env(&held, &call, "outsider", &[]);
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    let d = &out.json()["decision"];
    assert_eq!(d["session"]["name"], anchor.as_str());
    assert_eq!(d["standing"], "verified");
    assert_eq!(
        d["path"],
        json!([{"session": "w", "via": "held-by", "to": anchor}])
    );
    assert!(env
        .aimed_dir(&anchor, &call.session_id)
        .join(d["id"].as_str().unwrap())
        .is_file());
    assert!(!env.sessions().join("outsider/ctx/koto/aimed").exists());
}

// ===== R21: what reversing each kind means =====

#[test]
fn each_kind_reversal_carries_what_r21_says() {
    let env = Env::new();
    coordinator_and_worker(&env);
    let by_session = ["--by", "session"];
    let merge_a = env.record(
        "w",
        "merge",
        "merged",
        &["repository=r", "pr=1", "commit=a"],
        &by_session,
    );
    let merge_b = env.record(
        "w",
        "merge",
        "merged",
        &["repository=r", "pr=2", "commit=b"],
        &by_session,
    );
    let verdict = env.record(
        "w",
        "verdict",
        "pass",
        &["what=release 1.2", "repository=r", "commit=c"],
        &["--by", "session", "--evidence", "event:w:1"],
    );
    let rejected = env.record("w", "accept", "rejected", &["location=req-9"], &by_session);
    let escalate = env.record(
        "w",
        "escalate",
        "escalated",
        &["question=q1", "text=which db", "recipient=coord"],
        &by_session,
    );
    let decline = env.record(
        "w",
        "decline-escalation",
        "kept",
        &["question=q2", "text=which cache"],
        &by_session,
    );

    let fix = "https://forge.example/r/pull/3";
    let d = env.reversed(&merge_a, "coord", &["--compensating", fix]);
    assert_eq!(d["compensating"], json!({"link": fix}));
    let d = env.reversed(&merge_b, "coord", &[]);
    assert_eq!(d["compensating"], json!({"link": null}), "none recorded");

    let rollback = "https://forge.example/r/pull/4";
    let d = env.reversed(
        &verdict,
        "coord",
        &["--replacement", "fail", "--compensating", rollback],
    );
    assert_eq!(d["replacement"], "fail");
    assert_eq!(d["compensating"], json!({"link": rollback}));

    env.reversed(&rejected, "coord", &[]);
    let accepts = env.list("w", &["--kind", "accept", "--subject", "location=req-9"]);
    assert!(
        accepts["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["status"] != "in-effect"),
        "{accepts}"
    );

    env.reversed(&escalate, "coord", &[]);
    env.reversed(&decline, "coord", &[]);
    let listing = env.list("w", &[]);
    let e = entry(&listing, "w", &escalate.id);
    assert_eq!(e["effect"]["question"], "q1");
    assert_eq!(e["effect"]["withdrawn"], true);
    let e = entry(&listing, "w", &decline.id);
    assert_eq!(e["effect"]["question"], "q2");
    assert_eq!(e["effect"]["escalated_to"], "coord");
    let e = entry(&listing, "w", &verdict.id);
    assert_eq!(e["reversed_by"]["replacement"], "fail");

    // What a kind can't carry is refused, and nothing is recorded.
    let other = env.record("w", "judgment", "settled", &["what=x"], &by_session);
    let out = env.reverse(&other, "coord", &["--replacement", "pass"]);
    assert_eq!(out.code, 2);
    assert_eq!(out.json()["field"], "replacement");
    let out = env.reverse(&escalate, "coord", &["--compensating", fix]);
    assert_eq!(out.code, 2);
    assert_eq!(out.json()["field"], "compensating");
}

#[test]
fn reversing_an_acceptance_lists_its_dependents() {
    let env = Env::new();
    coordinator_and_worker(&env);
    let first = env.record(
        "w",
        "accept",
        "accepted",
        &["location=req-1"],
        &["--by", "session"],
    );
    let basis = format!("w:{}", first.id);
    let later = env.record(
        "w",
        "accept",
        "accepted",
        &["location=req-2"],
        &["--by", "session", "--basis", &basis],
    );
    let rid = env.reversed(&first, "coord", &[])["id"]
        .as_str()
        .unwrap()
        .to_string();

    let affected = json!([{"session": "w", "id": later.id}]);
    let coord = env.list("coord", &[]);
    assert_eq!(entry(&coord, "coord", &rid)["affected"], affected);
    let w = env.list("w", &[]);
    assert_eq!(entry(&w, "w", &first.id)["affected"], affected);
    assert_eq!(entry(&w, "w", &later.id)["status"], "in-effect");
}

// ===== R17: who may reverse =====

#[test]
fn a_person_reverses_through_the_coordinator_and_a_session_alone_cannot() {
    let env = Env::new();
    coordinator_and_worker(&env);
    let call = env.judgment("w", "use postgres");
    let d = env.reversed(
        &call,
        "coord",
        &["--by", "person:dana", "--surface", "terminal"],
    );
    assert_eq!(d["decider"]["kind"], "person");
    assert_eq!(d["decider"]["handle"]["value"], "dana");
    assert_eq!(d["decider"]["entering"]["session"]["value"], "coord");
    assert_eq!(d["session"]["name"], "coord");

    // A person's decision in the worker; the coordinator alone can't
    // reverse it.
    let persons = env.record(
        "w",
        "judgment",
        "settled",
        &["what=keep the cache"],
        &["--by", "person:ana", "--surface", "terminal"],
    );
    let out = env.reverse(&persons, "coord", &[]);
    assert_eq!(out.code, 2, "{}", out.stdout);
    let err = out.json();
    assert_eq!(err["code"], "reversal_refused");
    assert_eq!(err["reason"], "no_standing");
    let refused = refusals(&env.list("coord", &[]));
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0]["id"], err["refused"]["id"]);
    assert_eq!(refused[0]["decision"]["refusal"]["reason"], "no_standing");
    assert_eq!(
        entry(&env.list("w", &[]), "w", &persons.id)["status"],
        "in-effect"
    );
}

// ===== R17, R18: refusals are recorded =====

#[test]
fn refusals_are_recorded_in_the_attempting_session() {
    let env = Env::new();
    coordinator_and_worker(&env);

    // A worker reversing its coordinator's decision, chains complete.
    let coords = env.judgment("coord", "ship it");
    let out = env.reverse(&coords, "w", &[]);
    assert_eq!(out.code, 2, "{}", out.stdout);
    assert_eq!(out.json()["reason"], "not_on_chain");
    let w_refused = refusals(&env.list("w", &[]));
    assert_eq!(w_refused.len(), 1);
    assert_eq!(
        w_refused[0]["decision"]["refusal"]["reason"],
        "not_on_chain"
    );
    assert_eq!(w_refused[0]["decision"]["subject"]["kind"], "reverse");

    // A reversal, a refusal, an unknown id and an already reversed
    // decision, each from the coordinator.
    let call = env.judgment("w", "use postgres");
    let rid = env.reversed(&call, "coord", &[])["id"]
        .as_str()
        .unwrap()
        .to_string();
    let reversal = Rec {
        session: "coord".to_string(),
        id: rid.clone(),
        session_id: String::new(),
    };
    let refusal = Rec {
        session: "w".to_string(),
        id: w_refused[0]["id"].as_str().unwrap().to_string(),
        session_id: String::new(),
    };
    let unknown = Rec {
        session: "w".to_string(),
        id: "000000000099".to_string(),
        session_id: String::new(),
    };
    let cases = [
        (&reversal, "reverses_reversal"),
        (&refusal, "reverses_refusal"),
        (&unknown, "not_found"),
        (&call, "already_reversed"),
    ];
    for (n, (of, reason)) in cases.iter().enumerate() {
        let out = env.reverse(of, "coord", &[]);
        assert_eq!(out.code, 2, "{reason}: {}", out.stdout);
        let err = out.json();
        assert_eq!(err["code"], "reversal_refused");
        assert_eq!(err["reason"], *reason);
        let refused = refusals(&env.list("coord", &[]));
        assert_eq!(refused.len(), n + 1, "{reason}");
        assert_eq!(refused[n]["decision"]["refusal"]["reason"], *reason);
        assert_eq!(refused[n]["id"], err["refused"]["id"]);
    }
    let last = refusals(&env.list("coord", &[]));
    assert_eq!(
        last[3]["decision"]["refusal"]["winner"],
        json!({"session": "coord", "id": rid})
    );
}

#[test]
fn an_asserted_reversal_beyond_a_broken_chain() {
    let env = Env::new();
    env.init("gone", &[]);
    env.init("w", &["--dispatched-by", "gone"]);
    std::fs::remove_dir_all(env.sessions().join("gone")).unwrap();
    env.init("outsider", &[]);
    let call = env.judgment("w", "use postgres");

    let d = env.reversed(&call, "outsider", &[]);
    assert_eq!(d["standing"], "asserted");
    assert_eq!(d["session"]["name"], "outsider");
    assert_eq!(d["path"], json!([]));
    let e = entry(&env.list("w", &[]), "w", &call.id).clone();
    assert_eq!(e["status"], "reversed");
    assert_eq!(e["reversed_by"]["session"], "outsider");
    assert_eq!(e["reversed_by"]["standing"], "asserted");

    // With the chain complete, a session off it is refused.
    env.init("coord", &[]);
    env.init("w2", &["--dispatched-by", "coord"]);
    let call = env.judgment("w2", "use sqlite");
    let out = env.reverse(&call, "outsider", &[]);
    assert_eq!(out.code, 2);
    let err = out.json();
    assert_eq!(err["code"], "reversal_refused");
    assert_eq!(err["reason"], "not_on_chain");
}

// ===== R19: racing reversals =====

/// Set the `time` of the decision file at `path`.
fn set_time(path: &Path, time: &str) {
    let mut body: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    body["time"] = json!(time);
    let mut bytes = serde_json::to_vec(&body).unwrap();
    bytes.push(b'\n');
    std::fs::write(path, bytes).unwrap();
}

/// Sessions `a` and `b` both above `c`, and a session decision in `c`.
fn race_fixture(env: &Env) -> Rec {
    env.init("a", &[]);
    env.init("b", &[]);
    env.init("c", &["--dispatched-by", "a"]);
    env.ok(&["session", "link", "c", "--dispatched-by", "b"]);
    env.judgment("c", "use postgres")
}

#[test]
fn racing_reversals_settle_the_same_way_for_every_reader() {
    const EARLY: &str = "2026-11-10T10:00:00.000Z";
    const LATE: &str = "2026-11-10T10:00:05.000Z";
    for a_first in [true, false] {
        for (a_time, b_time, winner) in
            [(EARLY, LATE, "a"), (LATE, EARLY, "b"), (EARLY, EARLY, "a")]
        {
            let env = Env::new();
            let call = race_fixture(&env);
            let c_before = env.snapshot("c");
            let order = if a_first { ["a", "b"] } else { ["b", "a"] };
            let mut written = BTreeMap::new();
            for (n, s) in order.iter().enumerate() {
                // The second reverser can't see the first, as if both ran
                // at once on different hosts.
                let envs: &[(&str, &str)] = if n == 0 { &[] } else { &[HIDE_ALL] };
                let out = env.reverse_env(envs, &call, s, &[]);
                assert_eq!(out.code, 0, "{s}: {}", out.stdout);
                written.insert(
                    s.to_string(),
                    out.json()["id"].as_str().unwrap().to_string(),
                );
            }
            for (s, time) in [("a", a_time), ("b", b_time)] {
                set_time(&env.aimed_dir(s, &call.session_id).join(&written[s]), time);
            }
            let loser = if winner == "a" { "b" } else { "a" };
            let case = format!("a_first={a_first} a={a_time} b={b_time}");

            for reader in ["a", "b", "c"] {
                let listing = env.list(reader, &[]);
                if reader == "c" {
                    let e = entry(&listing, "c", &call.id);
                    assert_eq!(e["status"], "reversed", "{case}");
                    assert_eq!(e["reversed_by"]["session"], winner, "{case}");
                    assert_eq!(e["reversed_by"]["id"], written[winner].as_str());
                } else {
                    let e = entry(&listing, reader, &written[reader]);
                    let want = if reader == winner {
                        "in-effect"
                    } else {
                        "refused"
                    };
                    assert_eq!(e["status"], want, "{case} reader {reader}");
                    if reader == loser {
                        assert_eq!(
                            e["winner"],
                            json!({"session": winner, "id": written[winner]}),
                            "{case}"
                        );
                    }
                }
            }
            let below = env.list("a", &["--below"]);
            assert_eq!(
                entry(&below, "c", &call.id)["reversed_by"]["session"],
                winner
            );
            assert_eq!(env.snapshot("c"), c_before, "{case}: c changed");
        }
    }
}

#[test]
fn a_losing_reverse_records_its_refusal() {
    // The competitor is there when the reversal is written.
    let env = Env::new();
    let call = race_fixture(&env);
    let b_rid = env.reversed(&call, "b", &[])["id"]
        .as_str()
        .unwrap()
        .to_string();
    let out = env.reverse_env(&[HIDE_BEFORE], &call, "a", &[]);
    assert_ne!(out.code, 0, "{}", out.stdout);
    let err = out.json();
    assert_eq!(err["code"], "reversal_refused");
    assert_eq!(err["reason"], "lost_race");
    assert_eq!(err["winner"], json!({"session": "b", "id": b_rid}));
    let listing = env.list("a", &[]);
    let refused = refusals(&listing);
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0]["id"], err["refused"]["id"]);
    let refusal = &refused[0]["decision"]["refusal"];
    assert_eq!(refusal["winner"], json!({"session": "b", "id": b_rid}));
    let a_rid = refusal["reversal"]["id"].as_str().unwrap();
    assert_eq!(entry(&listing, "a", a_rid)["status"], "refused");

    // The competitor is hidden until after the command returns: the
    // losing session's next decision command records the refusal, once.
    let env = Env::new();
    let call = race_fixture(&env);
    let b_rid = env.reversed(&call, "b", &[])["id"]
        .as_str()
        .unwrap()
        .to_string();
    let out = env.reverse_env(&[HIDE_ALL], &call, "a", &[]);
    assert_eq!(out.code, 0, "{}", out.stdout);
    let a_rid = out.json()["id"].as_str().unwrap().to_string();
    assert!(refusals(&env.list("a", &[])).is_empty());
    assert_eq!(entry(&env.list("a", &[]), "a", &a_rid)["status"], "refused");

    env.judgment("a", "next call");
    let refused = refusals(&env.list("a", &[]));
    assert_eq!(refused.len(), 1, "{refused:?}");
    let refusal = &refused[0]["decision"]["refusal"];
    assert_eq!(refusal["reason"], "lost_race");
    assert_eq!(refusal["winner"], json!({"session": "b", "id": b_rid}));
    assert_eq!(refusal["reversal"], json!({"session": "a", "id": a_rid}));
    env.judgment("a", "another call");
    assert_eq!(refusals(&env.list("a", &[])).len(), 1, "recorded twice");
}

// ===== R20: restoring takes a new decision =====

#[test]
fn a_new_accept_after_a_reversal_is_in_effect() {
    let env = Env::new();
    coordinator_and_worker(&env);
    let by_session = ["--by", "session"];
    let first = env.record("w", "accept", "accepted", &["location=req-1"], &by_session);
    env.reversed(&first, "coord", &[]);
    let again = env.record("w", "accept", "accepted", &["location=req-1"], &by_session);
    let listing = env.list("w", &["--kind", "accept", "--subject", "location=req-1"]);
    assert_eq!(entry(&listing, "w", &first.id)["status"], "reversed");
    assert_eq!(entry(&listing, "w", &again.id)["status"], "in-effect");
}

// ===== R17: the same decider =====

#[test]
fn a_decider_reverses_its_own_decision() {
    let env = Env::new();
    coordinator_and_worker(&env);

    let verdict = env.record(
        "w",
        "verdict",
        "fail",
        &["what=lint"],
        &["--by", "check:grader", "--evidence", "event:w:1"],
    );
    let d = env.reversed(&verdict, "w", &["--by", "check:grader"]);
    assert_eq!(d["standing"], "verified");
    assert_eq!(d["path"], json!([]));
    assert!(env
        .aimed_dir("w", &verdict.session_id)
        .join(d["id"].as_str().unwrap())
        .is_file());
    // Another check in the same session has no standing.
    let other = env.record(
        "w",
        "verdict",
        "pass",
        &["what=tests"],
        &["--by", "check:grader", "--evidence", "event:w:1"],
    );
    let out = env.reverse(&other, "w", &["--by", "check:linter"]);
    assert_eq!(out.json()["reason"], "no_standing");

    let call = env.judgment("w", "skip the flaky test");
    let d = env.reversed(&call, "w", &[]);
    assert_eq!(d["standing"], "verified");

    let persons = env.record(
        "coord",
        "judgment",
        "settled",
        &["what=ship friday"],
        &["--by", "person:ana", "--surface", "terminal"],
    );
    let d = env.reversed(
        &persons,
        "coord",
        &["--by", "person:bea", "--surface", "terminal"],
    );
    assert_eq!(d["standing"], "verified");
    assert_eq!(d["decider"]["handle"]["value"], "bea");
}

// ===== R44, R20: show answers with the winning reversal =====

#[test]
fn show_returns_the_winning_reversal() {
    let env = Env::new();
    env.init("director", &[]);
    env.init("kid", &["--dispatched-by", "director"]);
    let call = env.record(
        "director",
        "judgment",
        "settled",
        &["what=freeze the api"],
        &["--by", "person:ana", "--surface", "terminal"],
    );
    let d = env.reversed(
        &call,
        "director",
        &[
            "--by",
            "person:bea",
            "--surface",
            "terminal",
            "--reason",
            "the freeze blocks the fix",
        ],
    );

    let before = env.snapshot("director");
    // Read as the dispatched session would; show takes no reader.
    let out = env.ok(&["decisions", "show", "director", &call.id]).json();
    assert_eq!(out["status"], "reversed");
    let by = &out["reversed_by"];
    assert_eq!(by["session"], "director");
    assert_eq!(by["id"], d["id"]);
    assert_eq!(by["time"], d["time"]);
    assert_eq!(by["decider"], d["decider"]);
    assert_eq!(by["decider"]["handle"]["value"], "bea");
    assert_eq!(by["reason"], "the freeze blocks the fix");
    assert_eq!(by["standing"], "verified");
    assert_eq!(by["path"], json!([]));
    assert_eq!(out["decision"]["decider"]["handle"]["value"], "ana");

    // The reversal itself shows in effect.
    let rid = d["id"].as_str().unwrap();
    let shown = env.ok(&["decisions", "show", "director", rid]).json();
    assert_eq!(shown["status"], "in-effect");
    assert_eq!(shown["decision"]["kind"], "reverse");
    assert_eq!(
        env.snapshot("director"),
        before,
        "show wrote to the session"
    );
    assert!(env
        .snapshot("kid")
        .keys()
        .all(|k| !k.starts_with("ctx/koto/aimed")));
}
