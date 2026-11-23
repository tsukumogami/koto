//! Succession in the decision record under the local backend, through the
//! real binary (PRD R37, R39 to R41, R43): after `koto init S --continues P
//! --supersede`, S stands exactly in P's position for standing, each hop
//! is confirmed against the successor's header and recorded in the
//! reversal's path, a hop koto can't confirm is refused
//! `supersede_unconfirmed`, `list --below` and `list --fenced` follow the
//! hops, and a decision past the marker's `decisions_through` reads
//! `refused`.
//!
//! Every test runs the real binary with `HOME` and `KOTO_SESSIONS_BASE`
//! pointed into its own temporary directory, and with every variable koto
//! reads an identity from removed.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use assert_fs::TempDir;
use serde_json::{json, Value};

const CLEARED_ENV: [&str; 6] = [
    "CLAUDE_CODE_SESSION_ID",
    "KOTO_ANCHOR",
    "KOTO_AGENT_ID",
    "KOTO_RUN_MODE",
    "KOTO_AUTHORITY",
    "KOTO_TEST_DECISIONS_FAULT",
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

const MARKER: &str = "ctx/koto/superseded-by";

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

/// A recorded decision: its session and id.
struct Rec {
    session: String,
    id: String,
}

const BY_SESSION: [&str; 2] = ["--by", "session"];

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

    fn dir(&self, name: &str) -> PathBuf {
        self.sessions().join(name)
    }

    fn run(&self, args: &[&str]) -> Out {
        let mut cmd = koto_command::koto_std();
        for name in CLEARED_ENV {
            cmd.env_remove(name);
        }
        cmd.current_dir(self.root.join("ws"));
        cmd.env("HOME", self.root.join("home"));
        cmd.env("KOTO_HOME", self.root.join("home"));
        cmd.env("KOTO_SESSIONS_BASE", self.sessions());
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

    fn init(&self, name: &str, extra: &[&str]) {
        let template = self.root.join("work.md");
        let mut args = vec!["init", name, "--template", template.to_str().unwrap()];
        args.extend_from_slice(extra);
        self.ok(&args);
    }

    fn supersede(&self, succ: &str, pred: &str) {
        self.init(succ, &["--continues", pred, "--supersede"]);
    }

    /// A `judgment` in `name`; `by` names the decider.
    fn judgment(&self, name: &str, what: &str, by: &[&str]) -> Rec {
        let subject = format!("what={what}");
        let mut args = vec![
            "decisions",
            "record",
            name,
            "--kind",
            "judgment",
            "--outcome",
            "settled",
            "--subject",
            &subject,
        ];
        args.extend_from_slice(by);
        let out = self.ok(&args).json();
        Rec {
            session: name.to_string(),
            id: out["id"].as_str().unwrap().to_string(),
        }
    }

    fn reverse(&self, of: &Rec, by_in: &str, by: &[&str]) -> Out {
        let mut args = vec![
            "decisions",
            "reverse",
            of.session.as_str(),
            of.id.as_str(),
            "--in",
            by_in,
        ];
        args.extend_from_slice(by);
        self.run(&args)
    }

    /// Reverse and expect success; returns the printed decision.
    fn reversed(&self, of: &Rec, by_in: &str, by: &[&str]) -> Value {
        let out = self.reverse(of, by_in, by);
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

    fn show(&self, of: &Rec) -> Value {
        self.ok(&["decisions", "show", &of.session, &of.id]).json()
    }

    fn header_id(&self, name: &str) -> Value {
        let log = std::fs::read_to_string(self.dir(name).join(format!("koto-{name}.state.jsonl")))
            .unwrap();
        serde_json::from_str::<Value>(log.lines().next().unwrap()).unwrap()["session_id"].clone()
    }

    fn fence(&self, name: &str) -> Value {
        let log = std::fs::read_to_string(self.dir(name).join(format!("koto-{name}.state.jsonl")))
            .unwrap();
        log.lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .find(|e| e["type"] == "session_superseded")
            .expect("the log holds the fence")["payload"]
            .clone()
    }

    fn snapshot(&self, session: &str) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut out = BTreeMap::new();
        let dir = self.dir(session);
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

fn entry<'a>(listing: &'a Value, session: &str, id: &str) -> &'a Value {
    listing["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["session"] == session && d["id"] == id)
        .unwrap_or_else(|| panic!("no {session}:{id} in {listing}"))
}

fn ids(listing: &Value) -> Vec<(String, String)> {
    listing["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["session"].as_str().unwrap().to_string(),
                d["id"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn refusals(listing: &Value) -> Vec<Value> {
    listing["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["decision"]["kind"] == "refused")
        .cloned()
        .collect()
}

/// The supersede hops in a standing path.
fn hops(path: &Value) -> Vec<Value> {
    path.as_array()
        .unwrap()
        .iter()
        .filter(|s| s["via"] == "supersede")
        .cloned()
        .collect()
}

/// X above P above W, a session judgment in W, and P fenced by S.
fn fenced_run(env: &Env) -> Rec {
    env.init("x", &[]);
    env.init("p", &["--dispatched-by", "x"]);
    env.init("w", &["--dispatched-by", "p"]);
    let call = env.judgment("w", "use postgres", &BY_SESSION);
    env.supersede("s", "p");
    call
}

fn assert_hop(env: &Env, step: &Value, fenced: &str, successor: &str) {
    assert_eq!(step["via"], "supersede", "{step}");
    assert_eq!(step["session"], fenced, "{step}");
    assert_eq!(step["to"], successor, "{step}");
    assert_eq!(step["successor_id"], env.header_id(successor), "{step}");
    let fence = env.fence(fenced);
    assert_eq!(step["actor"], fence["actor"], "{step}");
    assert_eq!(step["time"], fence["at"], "{step}");
}

// ===== R39, R41: a successor stands in the fenced session's position =====

#[test]
fn a_successor_reverses_a_workers_judgment_through_a_confirmed_hop() {
    let env = Env::new();
    let call = fenced_run(&env);
    let before: Vec<_> = ["w", "p", "x"].iter().map(|s| env.snapshot(s)).collect();

    let d = env.reversed(&call, "s", &BY_SESSION);
    assert_eq!(d["standing"], "verified");
    assert_eq!(d["session"]["name"], "s", "recorded in the successor");
    let path = d["path"].as_array().unwrap();
    assert_eq!(path.first().unwrap()["session"], "w", "{}", d["path"]);
    assert_eq!(path.first().unwrap()["to"], "p", "{}", d["path"]);
    let hop_steps = hops(&d["path"]);
    assert_eq!(hop_steps.len(), 1, "{}", d["path"]);
    assert_hop(&env, &hop_steps[0], "p", "s");
    assert_eq!(path.last().unwrap(), &hop_steps[0]);

    let after: Vec<_> = ["w", "p", "x"].iter().map(|s| env.snapshot(s)).collect();
    assert_eq!(before, after, "the reversal touched w, p or x");

    // The worker's judgment reads reversed, with the hop in the winning
    // reversal's path.
    let shown = env.show(&call);
    assert_eq!(shown["status"], "reversed");
    assert_eq!(shown["reversed_by"]["session"], "s");
    assert_eq!(shown["reversed_by"]["standing"], "verified");
    assert_eq!(shown["reversed_by"]["path"], d["path"]);
    assert_hop(&env, &hops(&shown["reversed_by"]["path"])[0], "p", "s");
}

#[test]
fn a_successor_reverses_the_fenced_sessions_own_session_judgment() {
    let env = Env::new();
    env.init("x", &[]);
    env.init("p", &["--dispatched-by", "x"]);
    let own = env.judgment("p", "ship it", &BY_SESSION);
    env.supersede("s", "p");

    let d = env.reversed(&own, "s", &BY_SESSION);
    assert_eq!(d["standing"], "verified");
    assert_eq!(d["session"]["name"], "s");
    assert_eq!(d["path"].as_array().unwrap().len(), 1, "{}", d["path"]);
    assert_hop(&env, &d["path"][0], "p", "s");
}

#[test]
fn a_successor_alone_cannot_reverse_a_person_but_a_person_through_it_can() {
    let env = Env::new();
    env.init("x", &[]);
    env.init("p", &["--dispatched-by", "x"]);
    let persons = env.judgment(
        "p",
        "keep the cache",
        &["--by", "person:ana", "--surface", "terminal"],
    );
    env.supersede("s", "p");

    let out = env.reverse(&persons, "s", &BY_SESSION);
    assert_eq!(out.code, 2, "{}", out.stdout);
    let err = out.json();
    assert_eq!(err["code"], "reversal_refused");
    assert_eq!(err["reason"], "no_standing");
    let refused = refusals(&env.list("s", &[]));
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0]["id"], err["refused"]["id"]);

    let d = env.reversed(
        &persons,
        "s",
        &["--by", "person:bea", "--surface", "terminal"],
    );
    assert_eq!(d["standing"], "verified");
    assert_eq!(d["decider"]["entering"]["session"]["value"], "s");
    assert_hop(&env, &d["path"][0], "p", "s");

    // show reads the ruling reversed: its own entering session, and the
    // hop (fence actor and time) in the winning reversal's path.
    let shown = env.show(&persons);
    assert_eq!(shown["status"], "reversed");
    assert_eq!(
        shown["decision"]["decider"]["entering"]["session"]["value"],
        "p"
    );
    assert_hop(&env, &shown["reversed_by"]["path"][0], "p", "s");
}

#[test]
fn two_hops_are_named_in_order_and_list_below_follows_them() {
    let env = Env::new();
    let call = fenced_run(&env);
    let other = env.judgment("w", "use sqlite", &BY_SESSION);
    env.supersede("s2", "s");

    let d = env.reversed(&call, "s2", &BY_SESSION);
    assert_eq!(d["standing"], "verified");
    assert_eq!(d["session"]["name"], "s2");
    let hop_steps = hops(&d["path"]);
    assert_eq!(hop_steps.len(), 2, "{}", d["path"]);
    assert_hop(&env, &hop_steps[0], "p", "s");
    assert_hop(&env, &hop_steps[1], "s", "s2");

    let below = env.list("s2", &["--below"]);
    let listed = ids(&below);
    assert!(
        listed.contains(&("w".to_string(), call.id.clone())),
        "{below}"
    );
    assert!(
        listed.contains(&("w".to_string(), other.id.clone())),
        "{below}"
    );
    assert_eq!(entry(&below, "w", &call.id)["status"], "reversed");

    // P's own successor list below also reaches W, and X still does.
    assert!(ids(&env.list("s", &["--below"])).contains(&("w".to_string(), call.id.clone())));
    assert!(ids(&env.list("x", &["--below"])).contains(&("w".to_string(), call.id)));
}

#[test]
fn a_successor_gains_no_standing_above_or_beside_the_fenced_session() {
    let env = Env::new();
    fenced_run(&env);
    env.init("q", &["--dispatched-by", "x"]);
    let siblings = env.judgment("q", "freeze", &BY_SESSION);
    let aboves = env.judgment("x", "plan", &BY_SESSION);

    for (n, of) in [&siblings, &aboves].into_iter().enumerate() {
        let out = env.reverse(of, "s", &BY_SESSION);
        assert_eq!(out.code, 2, "{}", out.stdout);
        let err = out.json();
        assert_eq!(err["code"], "reversal_refused");
        assert_eq!(err["reason"], "not_on_chain");
        let refused = refusals(&env.list("s", &[]));
        assert_eq!(refused.len(), n + 1);
        assert_eq!(refused[n]["id"], err["refused"]["id"]);
    }
    assert_eq!(
        entry(&env.list("q", &[]), "q", &siblings.id)["status"],
        "in-effect"
    );
    assert_eq!(
        entry(&env.list("x", &[]), "x", &aboves.id)["status"],
        "in-effect"
    );
}

// ===== R40: an unconfirmed hop =====

fn assert_unconfirmed(env: &Env, call: &Rec) {
    let out = env.reverse(call, "s", &BY_SESSION);
    assert_eq!(out.code, 2, "{}\n{}", out.stdout, out.stderr);
    let err = out.json();
    assert_eq!(err["code"], "reversal_refused");
    assert_eq!(err["reason"], "supersede_unconfirmed");
    let s = env.list("s", &[]);
    let refused = refusals(&s);
    assert_eq!(refused.len(), 1, "{s}");
    assert_eq!(refused[0]["id"], err["refused"]["id"]);
    assert_eq!(
        refused[0]["decision"]["refusal"]["reason"],
        "supersede_unconfirmed"
    );
    for d in s["decisions"].as_array().unwrap() {
        assert_ne!(d["decision"]["standing"], "asserted", "{d}");
    }
    assert_eq!(
        entry(&env.list("w", &[]), "w", &call.id)["status"],
        "in-effect"
    );
}

#[test]
fn a_fence_without_its_marker_confirms_no_hop() {
    let env = Env::new();
    let call = fenced_run(&env);
    std::fs::remove_file(env.dir("p").join(MARKER)).unwrap();
    assert_unconfirmed(&env, &call);
}

#[test]
fn a_marker_naming_another_session_id_confirms_no_hop() {
    let env = Env::new();
    let call = fenced_run(&env);
    let path = env.dir("p").join(MARKER);
    let mut marker: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    marker["successor"]["id"] = json!("00000000-0000-0000-0000-000000000000");
    std::fs::write(&path, marker.to_string()).unwrap();
    assert_unconfirmed(&env, &call);
}

#[test]
fn an_unconfirmed_hop_is_not_asserted_beyond_a_broken_chain() {
    let env = Env::new();
    let call = fenced_run(&env);
    // With X gone, W's chain is incomplete: off the chain, S would be
    // asserted, but the only way on is the unconfirmed hop.
    std::fs::remove_dir_all(env.dir("x")).unwrap();
    std::fs::remove_file(env.dir("p").join(MARKER)).unwrap();
    assert_unconfirmed(&env, &call);
}

// ===== R41: the reversals one fence enabled =====

#[test]
fn list_fenced_returns_exactly_the_hop_enabled_reversals() {
    let env = Env::new();
    let call = fenced_run(&env);
    let other = env.judgment("w", "use sqlite", &BY_SESSION);
    let through = env.reversed(&call, "s", &BY_SESSION);
    // A reversal from X, above, uses no hop.
    let direct = env.reversed(&other, "x", &BY_SESSION);
    assert!(hops(&direct["path"]).is_empty());

    for session in ["s", "x"] {
        let fenced = env.list(session, &["--fenced", "p"]);
        let listed = ids(&fenced);
        if session == "s" {
            assert_eq!(
                listed,
                vec![("s".to_string(), through["id"].as_str().unwrap().to_string())],
                "{fenced}"
            );
        } else {
            assert!(listed.is_empty(), "{fenced}");
        }
    }
    assert!(ids(&env.list("s", &["--fenced", "w"])).is_empty());
}

// ===== R37: a decision written after the fence =====

#[test]
fn a_decision_past_decisions_through_reads_refused() {
    let env = Env::new();
    env.init("p", &[]);
    let before = env.judgment("p", "ship it", &BY_SESSION);
    env.supersede("s", "p");
    let marker: Value =
        serde_json::from_slice(&std::fs::read(env.dir("p").join(MARKER)).unwrap()).unwrap();
    assert_eq!(marker["decisions_through"], before.id.as_str());

    // A decision written in P after the fence by a writer that hadn't
    // seen it, as under the cloud backend: the next id.
    let decisions = env.dir("p").join("ctx/koto/decisions");
    let mut late: Value =
        serde_json::from_slice(&std::fs::read(decisions.join(&before.id)).unwrap()).unwrap();
    let late_id = "000000000002";
    late["id"] = json!(late_id);
    std::fs::write(decisions.join(late_id), late.to_string()).unwrap();

    let listing = env.list("p", &[]);
    assert_eq!(entry(&listing, "p", &before.id)["status"], "in-effect");
    let e = entry(&listing, "p", late_id);
    assert_eq!(e["status"], "refused", "{listing}");
    assert_eq!(e["after_fence"], true);
    let shown = env.show(&Rec {
        session: "p".to_string(),
        id: late_id.to_string(),
    });
    assert_eq!(shown["status"], "refused");
}
