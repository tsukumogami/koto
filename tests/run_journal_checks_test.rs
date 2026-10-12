//! Check records in the run journal (`check_evaluated`, `check_rule`,
//! `check_pending`), and the `response` records of printed directives,
//! written by koto commands and read back the way tooling outside koto
//! reads them (see `docs/reference/run-journal.md`).
//!
//! Every test runs the koto binary with `HOME` in its own temporary
//! directory, `KOTO_SESSIONS_BASE` unset and `CLAUDE_CODE_SESSION_ID`
//! cleared, so the session store and the journal are that test's own and
//! the developer's journal is never touched.
//!
//! Every journal line a test reads goes through [`Env::journal`], which
//! checks it against the reference itself: its kind must be in the
//! reference's kinds table and every field must be one the table lists for
//! that kind, so code that writes an undocumented kind or field fails here.
//! The same check scans each line for content: no absolute path, no home
//! directory or host name, no field whose name mentions cost, and none of
//! the finding messages, paths, commands, rationales or directive text a
//! test planted.

#![cfg(unix)]

use assert_cmd::Command;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const JOURNAL: &str = "_run_journal.jsonl";
const REFERENCE: &str = "docs/reference/run-journal.md";

/// Fields every record may carry whatever its kind.
const ENVELOPE: &[&str] = &[
    "kind",
    "v",
    "at",
    "session",
    "koto.session.id",
    "koto.run.id",
];

/// Values the tests plant in checks, commands, overrides and directives.
/// None may reach the journal.
const PLANTED: &[&str] = &[
    "planted-message-5e1f",
    "planted/dir-5e1f",
    "planted-arg-5e1f",
    "planted-rationale-5e1f",
    "planted-directive-5e1f",
];

/// A check script: run `n` exits with line `n` of `codes.txt` (0 past the
/// end) and prints `findings.txt`, when present, as koto findings.
const CHECK_SH: &str = r#"#!/bin/sh
n=$(cat runs.log 2>/dev/null | wc -l)
echo run >> runs.log
code=$(sed -n "$((n + 1))p" codes.txt)
code=${code:-0}
if [ -f findings.txt ]; then
  cat findings.txt
fi
exit "$code"
"#;

// ----- Harness -----

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Out {
    fn journal_warnings(&self) -> usize {
        self.stderr
            .lines()
            .filter(|l| l.contains("run journal"))
            .count()
    }
}

struct Env {
    tmp: TempDir,
    home: PathBuf,
    work: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let work = tmp.path().join("work");
        for d in [&home, &work] {
            std::fs::create_dir_all(d).unwrap();
        }
        Env { tmp, home, work }
    }

    fn koto_home(&self) -> PathBuf {
        self.home.join(".koto")
    }

    fn journal_path(&self) -> PathBuf {
        self.koto_home().join(JOURNAL)
    }

    fn session_dir(&self, name: &str) -> PathBuf {
        self.koto_home().join("sessions").join(name)
    }

    fn state_path(&self, name: &str) -> PathBuf {
        self.session_dir(name)
            .join(format!("koto-{name}.state.jsonl"))
    }

    /// A koto command for this home, run from `cwd`.
    fn cmd(&self, cwd: &Path) -> Command {
        let mut cmd = Command::cargo_bin("koto").unwrap();
        cmd.current_dir(cwd)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", self.tmp.path().join("cache"));
        for unset in [
            "KOTO_SESSIONS_BASE",
            "KOTO_HOME",
            "XDG_CONFIG_HOME",
            "KOTO_WORKFLOWS_DIR",
            "KOTO_DECIDER",
            "KOTO_DECIDER_API_KEY",
            "KOTO_DECIDER_ENDPOINT",
            "CLAUDE_CODE_SESSION_ID",
        ] {
            cmd.env_remove(unset);
        }
        cmd
    }

    fn koto_in(&self, cwd: &Path, args: &[&str]) -> Out {
        let output = self.cmd(cwd).args(args).output().unwrap();
        Out {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        }
    }

    fn ok_in(&self, cwd: &Path, args: &[&str]) -> Out {
        let out = self.koto_in(cwd, args);
        assert_eq!(
            out.code,
            Some(0),
            "koto {args:?} failed\nstdout: {}\nstderr: {}",
            out.stdout,
            out.stderr
        );
        assert_eq!(out.journal_warnings(), 0, "{}", out.stderr);
        out
    }

    /// A working directory for one session's check script, holding the
    /// script, its exit codes and the findings it prints.
    fn check_dir(&self, label: &str, codes: &[i32], findings: &[Value]) -> PathBuf {
        let dir = self.work.join(label);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("check.sh");
        std::fs::write(&script, CHECK_SH).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let list: Vec<String> = codes.iter().map(|c| c.to_string()).collect();
        std::fs::write(dir.join("codes.txt"), list.join("\n") + "\n").unwrap();
        if !findings.is_empty() {
            let lines: Vec<String> = findings
                .iter()
                .map(|f| format!("::koto-finding::{f}"))
                .collect();
            std::fs::write(dir.join("findings.txt"), lines.join("\n") + "\n").unwrap();
        }
        dir
    }

    fn template(&self, dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("template.md");
        std::fs::write(&path, body).unwrap();
        path
    }

    /// Every journal line, each checked against the reference and for
    /// content.
    fn journal(&self) -> Vec<Value> {
        let raw = std::fs::read_to_string(self.journal_path()).unwrap_or_default();
        let documented = documented_fields();
        raw.lines()
            .map(|line| {
                let record: Value = serde_json::from_str(line)
                    .unwrap_or_else(|e| panic!("journal line is not JSON ({e}): {line}"));
                self.check_line(&record, line, &documented);
                record
            })
            .collect()
    }

    fn records(&self, session: &str) -> Vec<Value> {
        self.journal()
            .into_iter()
            .filter(|r| r["session"] == session)
            .collect()
    }

    fn check_records(&self, session: &str) -> Vec<Value> {
        self.records(session)
            .into_iter()
            .filter(|r| r["kind"].as_str().unwrap().starts_with("check_"))
            .collect()
    }

    /// The session's log events (header skipped).
    fn events(&self, name: &str) -> Vec<Value> {
        std::fs::read_to_string(self.state_path(name))
            .unwrap()
            .lines()
            .skip(1)
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn check_line(
        &self,
        record: &Value,
        line: &str,
        documented: &BTreeMap<String, BTreeSet<String>>,
    ) {
        let obj = record.as_object().expect("a record is an object");
        let kind = obj["kind"].as_str().expect("kind is a string");
        let fields = documented
            .get(kind)
            .unwrap_or_else(|| panic!("{REFERENCE} doesn't list the kind {kind}: {line}"));
        for (key, value) in obj {
            assert!(
                ENVELOPE.contains(&key.as_str()) || fields.contains(key),
                "{REFERENCE} doesn't list {key} for {kind}: {line}"
            );
            assert!(!key.contains("cost"), "{line}");
            assert!(
                !value.is_null() && value != "",
                "{key} is null or empty: {line}"
            );
            if let Some(s) = value.as_str() {
                assert!(
                    !s.starts_with('/') && !s.starts_with('~'),
                    "{key} holds a path: {line}"
                );
            }
        }
        assert_eq!(obj["v"], 1, "{line}");
        let home = self.home.to_string_lossy().to_string();
        let tmp = self.tmp.path().to_string_lossy().to_string();
        for absolute in [
            home.as_str(),
            tmp.as_str(),
            "/tmp",
            "/var/",
            "/Users/",
            "/home/",
        ] {
            assert!(!line.contains(absolute), "{absolute} in {line}");
        }
        if let Some(host) = host_name() {
            assert!(!line.contains(&host), "host name in {line}");
        }
        for planted in PLANTED {
            assert!(!line.contains(planted), "planted {planted} in {line}");
        }
    }
}

/// Each kind the reference lists, with the fields its kinds table names
/// for it.
fn documented_fields() -> BTreeMap<String, BTreeSet<String>> {
    let text =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(REFERENCE)).unwrap();
    let section = text
        .split("### Record kinds")
        .nth(1)
        .expect("the reference has a Record kinds section");
    let row = regex::Regex::new(r"^\| `([a-z_]+)` \| ([^|]*) \|").unwrap();
    let span = regex::Regex::new(r"`([^`]+)`").unwrap();
    section
        .lines()
        .skip_while(|l| !l.starts_with('|'))
        .take_while(|l| l.starts_with('|'))
        .filter_map(|l| row.captures(l))
        .map(|c| {
            let fields = span
                .captures_iter(&c[2])
                .map(|s| s[1].to_string())
                .collect();
            (c[1].to_string(), fields)
        })
        .collect()
}

/// The host's name, when it is distinctive enough to search for.
fn host_name() -> Option<String> {
    let out = std::process::Command::new("hostname").output().ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let distinctive = name.len() >= 6
        && name
            .bytes()
            .any(|b| !(b.is_ascii_hexdigit() || b == b'-' || b == b'.'));
    distinctive.then_some(name)
}

fn kinds(records: &[Value]) -> Vec<&str> {
    records
        .iter()
        .map(|r| r["kind"].as_str().unwrap())
        .collect()
}

fn finding(rule_id: &str) -> Value {
    serde_json::json!({
        "rule_id": rule_id,
        "level": "error",
        "message": "planted-message-5e1f: line too long",
        "path": "planted/dir-5e1f/file.rs",
        "line": 3,
    })
}

fn digest(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let hex = hex::encode(Sha256::digest(value.as_bytes()));
    format!("sha256:{}", &hex[..32])
}

// ----- Templates -----
//
// Every gate sets `timeout: 600` rather than koto's 30-second default, and
// the hold test holds for up to 120 seconds, so a loaded machine slows these
// tests down instead of turning a check into `timed_out`. No assertion
// depends on how long a run takes: each script's answers come from its run
// count, and a held poll stops at its first passing run.

/// `work` runs the `lint` check and moves to `done` once it passes.
const RETRY: &str = r#"---
name: journal-checks
version: "1.0"
initial_state: work
states:
  work:
    gates:
      lint:
        type: command
        command: "./check.sh planted-arg-5e1f"
        timeout: 600
    transitions:
      - target: done
        when:
          gates.lint.exit_code: 0
  done:
    terminal: true
---

## work

Fix what the check reports. planted-directive-5e1f

## done

Done.
"#;

/// `ci` waits on a polling check and moves to `done` once it passes.
fn polling(hold: &str) -> String {
    format!(
        r#"---
name: journal-poll
version: "1.0"
initial_state: ci
states:
  ci:
    gates:
      ci:
        type: command
        command: ./check.sh
        timeout: 600
        poll:
          interval_secs: 1
          timeout_secs: 600
{hold}    transitions:
      - target: done
        when:
          gates.ci.exit_code: 0
  done:
    terminal: true
---

## ci

Wait for CI.

## done

Done.
"#
    )
}

/// `a` and `b` each run a passing probe; `a` takes `move: stay` back into
/// itself and `move: next` on to `b`.
const VISITS: &str = r#"---
name: journal-visits
version: "1.0"
initial_state: a
states:
  a:
    accepts:
      move:
        type: enum
        required: true
        values: ["stay", "next"]
    gates:
      probe:
        type: command
        command: "true"
        timeout: 600
    transitions:
      - target: a
        when:
          move: stay
      - target: b
        when:
          move: next
  b:
    accepts:
      move:
        type: enum
        required: true
        values: ["next"]
    gates:
      probe:
        type: command
        command: "true"
        timeout: 600
    transitions:
      - target: done
        when:
          move: next
  done:
    terminal: true
---

## a

A.

## b

B.

## done

Done.
"#;

/// Run the `lint` check of [`RETRY`] for `name` once per entry in `codes`,
/// failing with `rules` (in order) on every non-zero code, and return the
/// directory it ran in.
fn run_retry(env: &Env, name: &str, codes: &[i32], rules: &[&str], cleanup: bool) -> PathBuf {
    let findings: Vec<Value> = rules.iter().copied().map(finding).collect();
    let dir = env.check_dir(name, codes, &findings);
    let t = env.template(&dir, RETRY);
    env.ok_in(&dir, &["init", name, "--template", t.to_str().unwrap()]);
    for _ in codes {
        let mut args = vec!["next", name];
        if !cleanup {
            args.push("--no-cleanup");
        }
        let out = env.koto_in(&dir, &args);
        assert_eq!(out.journal_warnings(), 0, "{}", out.stderr);
    }
    dir
}

// ----- Restated scenarios -----

/// A check fails rule E501 on three attempts of one visit, then passes.
#[test]
fn a_converged_retry_writes_four_evaluations_and_three_rule_records() {
    let env = Env::new();
    run_retry(&env, "converge", &[1, 1, 1, 0], &["E501"], false);
    let c = env.check_records("converge");
    let evaluated: Vec<&Value> = c
        .iter()
        .filter(|r| r["kind"] == "check_evaluated")
        .collect();
    assert_eq!(evaluated.len(), 4, "{c:?}");
    let visit_attempts: Vec<u64> = evaluated
        .iter()
        .map(|r| r["koto.visit_attempt"].as_u64().unwrap())
        .collect();
    assert_eq!(visit_attempts, vec![1, 2, 3, 4]);
    let outcomes: Vec<&str> = evaluated
        .iter()
        .map(|r| r["koto.check.outcome"].as_str().unwrap())
        .collect();
    assert_eq!(outcomes, vec!["failed", "failed", "failed", "passed"]);
    let rules: Vec<(&str, u64)> = c
        .iter()
        .filter(|r| r["kind"] == "check_rule")
        .map(|r| {
            (
                r["koto.rule.id"].as_str().unwrap(),
                r["koto.rule.visit_count"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(rules, vec![("E501", 1), ("E501", 2), ("E501", 3)]);
    for r in &c {
        assert_eq!(r["koto.state"], "work");
        assert_eq!(r["koto.check"], "lint");
    }
}

/// The same check fails E501 on four attempts of one visit.
#[test]
fn a_loop_writes_a_fourth_rule_record_with_visit_count_four() {
    let env = Env::new();
    run_retry(&env, "loops", &[1, 1, 1, 1], &["E501"], false);
    let rules: Vec<Value> = env
        .check_records("loops")
        .into_iter()
        .filter(|r| r["kind"] == "check_rule")
        .collect();
    assert_eq!(rules.len(), 4);
    assert_eq!(rules[3]["koto.rule.visit_count"], 4);
    assert_eq!(rules[3]["koto.rule.session_count"], 4);
}

/// A polling check answers pending, pending, failed, then (after a fix)
/// pending and passed.
#[test]
fn a_polling_check_writes_pending_records_between_its_evaluations() {
    let env = Env::new();
    let dir = env.check_dir("poll", &[75, 75, 1, 75, 0], &[]);
    let t = env.template(&dir, &polling(""));
    env.ok_in(&dir, &["init", "poll", "--template", t.to_str().unwrap()]);
    for _ in 0..5 {
        env.koto_in(&dir, &["next", "poll", "--no-cleanup"]);
    }
    let all = env.check_records("poll");
    // The failed evaluation printed no finding, so koto counted its own
    // fallback rule; its rule record shares the evaluation's seq.
    let (rules, c): (Vec<Value>, Vec<Value>) =
        all.into_iter().partition(|r| r["kind"] == "check_rule");
    assert_eq!(
        kinds(&c),
        vec![
            "check_pending",
            "check_pending",
            "check_evaluated",
            "check_pending",
            "check_evaluated"
        ]
    );
    for rule in &rules {
        assert_eq!(rule["koto.seq"], c[2]["koto.seq"], "{rule}");
    }
    assert_eq!(c[2]["koto.check.outcome"], "failed");
    assert_eq!(c[4]["koto.check.outcome"], "passed");
    let n = c[2]["koto.attempt"].as_u64().unwrap();
    assert_eq!(c[4]["koto.attempt"], n + 1);
    assert_eq!(c[2]["koto.poll.status"], "failed");
    assert_eq!(c[4]["koto.poll.status"], "done");
    // Each pending record carries the poll's own numbers and nothing
    // judged.
    let log: Vec<Value> = env
        .events("poll")
        .into_iter()
        .filter(|e| e["type"] == "gate_evaluated")
        .collect();
    assert_eq!(log.len(), 5);
    for (record, event) in c.iter().zip(&log) {
        assert_eq!(record["koto.seq"], event["seq"]);
        if record["kind"] == "check_pending" {
            let poll = &event["payload"]["poll"];
            assert_eq!(record["koto.poll.status"], "pending");
            assert_eq!(record["koto.poll.evaluations"], poll["evaluations"]);
            assert_eq!(record["koto.poll.elapsed_secs"], poll["elapsed_secs"]);
            for judged in [
                "koto.check.outcome",
                "koto.attempt",
                "koto.visit_attempt",
                "koto.rule.id",
                "koto.check.rule_count",
            ] {
                assert!(record.get(judged).is_none(), "{judged}: {record}");
            }
        }
    }
}

/// A check reports a rule id outside the name shape, and its rule record
/// carries the id's digest.
#[test]
fn an_odd_rule_id_is_written_as_its_digest() {
    let env = Env::new();
    let odd = "no explicit any [x]";
    run_retry(&env, "odd", &[1], &[odd], false);
    let rules: Vec<Value> = env
        .check_records("odd")
        .into_iter()
        .filter(|r| r["kind"] == "check_rule")
        .collect();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["koto.rule.id"], digest(odd).as_str());
    let raw = std::fs::read_to_string(env.journal_path()).unwrap();
    assert!(!raw.contains("explicit"), "{raw}");
}

/// A session runs the converged retry to a successful terminal and koto
/// removes it; every check record is still in the journal.
#[test]
fn check_records_outlive_a_session_removed_at_its_terminal() {
    let env = Env::new();
    run_retry(&env, "removed", &[1, 1, 1, 0], &["E501"], true);
    assert!(
        !env.session_dir("removed").exists(),
        "the session was removed"
    );
    let records = env.records("removed");
    let k = kinds(&records);
    assert_eq!(k.iter().filter(|k| **k == "check_evaluated").count(), 4);
    assert_eq!(k.iter().filter(|k| **k == "check_rule").count(), 3);
    assert_eq!(k.last(), Some(&"terminal"));
}

/// From the journal alone: for each session, visit, check and rule, the
/// highest `koto.rule.visit_count`, and whether the check passed later in
/// the same visit. Against the default bound of 3, the converged session
/// is one retry sequence that converged and the looping one is a loop.
#[test]
fn the_journal_alone_separates_a_converged_retry_from_a_loop() {
    let env = Env::new();
    run_retry(&env, "converge", &[1, 1, 1, 0], &["E501"], true);
    run_retry(&env, "loops", &[1, 1, 1, 1], &["E501"], true);
    let lines = std::fs::read_to_string(env.journal_path()).unwrap();
    let bound = 3;

    type Key = (String, u64, String, String, String);
    let mut highest: BTreeMap<Key, (u64, u64)> = BTreeMap::new();
    let mut passed: Vec<(String, u64, String, String, u64)> = Vec::new();
    for line in lines.lines() {
        let r: Value = serde_json::from_str(line).unwrap();
        let s = |k: &str| r[k].as_str().unwrap_or_default().to_string();
        let n = |k: &str| r[k].as_u64().unwrap_or_default();
        match r["kind"].as_str() {
            Some("check_rule") => {
                let key = (
                    s("koto.session.id"),
                    n("koto.visit.seq"),
                    s("koto.state"),
                    s("koto.check"),
                    s("koto.rule.id"),
                );
                let entry = highest.entry(key).or_insert((0, 0));
                if n("koto.rule.visit_count") > entry.0 {
                    *entry = (n("koto.rule.visit_count"), n("koto.seq"));
                }
            }
            Some("check_evaluated") if r["koto.check.outcome"] == "passed" => passed.push((
                s("koto.session.id"),
                n("koto.visit.seq"),
                s("koto.state"),
                s("koto.check"),
                n("koto.seq"),
            )),
            _ => {}
        }
    }
    let mut converged = Vec::new();
    let mut loops = Vec::new();
    for ((session, visit, state, check, rule), (count, seq)) in &highest {
        let resolved = passed.iter().any(|p| {
            &p.0 == session && p.1 == *visit && &p.2 == state && &p.3 == check && p.4 > *seq
        });
        if *count > bound {
            loops.push((session.clone(), rule.clone(), *count));
        } else if resolved {
            converged.push((session.clone(), rule.clone(), *count));
        }
    }
    let id_of = |name: &str| {
        env.records(name)[0]["koto.session.id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(converged, vec![(id_of("converge"), "E501".to_string(), 3)]);
    assert_eq!(loops, vec![(id_of("loops"), "E501".to_string(), 4)]);
}

// ----- Sequence and visit -----

/// Each record's `koto.seq` is the sequence number of the session-log event
/// it relays, a check's rule records share it, and `koto.visit.seq` is the
/// sequence number of the entry that opened the visit.
#[test]
fn koto_seq_is_the_relayed_events_seq_and_rules_share_it() {
    let env = Env::new();
    run_retry(
        &env,
        "seqs",
        &[1, 1, 1, 0],
        &["W605", "E501", "no-unused-vars"],
        false,
    );
    let log = env.events("seqs");
    let evaluated: Vec<u64> = log
        .iter()
        .filter(|e| e["type"] == "gate_evaluated")
        .map(|e| e["seq"].as_u64().unwrap())
        .collect();
    let arrival = log
        .iter()
        .find(|e| e["type"] == "transitioned" && e["payload"]["to"] == "work")
        .unwrap()["seq"]
        .as_u64()
        .unwrap();
    let c = env.check_records("seqs");
    let mut current = None;
    let mut i = 0;
    for r in &c {
        if r["kind"] == "check_evaluated" {
            current = Some(evaluated[i]);
            i += 1;
        }
        assert_eq!(r["koto.seq"].as_u64(), current, "{r}");
        assert_eq!(r["koto.visit.seq"], arrival, "{r}");
    }
    assert_eq!(i, 4);
    // A failing check with three error rules writes three, in the order
    // the check first reported them.
    let first_rules: Vec<&str> = c[1..4]
        .iter()
        .map(|r| r["koto.rule.id"].as_str().unwrap())
        .collect();
    assert_eq!(first_rules, vec!["W605", "E501", "no-unused-vars"]);
}

/// A passing check that prints an error-level finding writes no rule
/// record and a rule count of 0.
#[test]
fn a_passing_check_with_an_error_finding_writes_no_rule_record() {
    let env = Env::new();
    run_retry(&env, "clean", &[0], &["E501"], false);
    let c = env.check_records("clean");
    assert_eq!(kinds(&c), vec!["check_evaluated"]);
    assert_eq!(c[0]["koto.check.outcome"], "passed");
    assert_eq!(c[0]["koto.check.rule_count"], 0);
    assert!(c[0]["koto.check.duration_ms"].is_u64(), "{}", c[0]);
}

/// A session enters `a`, takes a self-transition in `a`, moves to `b` and is
/// rewound to `a`: one visit seq for both `a` spans before the move,
/// another for `b`, and a third after the rewind, each the seq of the
/// entry that opened it.
#[test]
fn a_self_transition_keeps_the_visit_and_a_move_and_a_rewind_open_new_ones() {
    let env = Env::new();
    let dir = env.work.join("visits");
    std::fs::create_dir_all(&dir).unwrap();
    let t = env.template(&dir, VISITS);
    env.ok_in(&dir, &["init", "v", "--template", t.to_str().unwrap()]);
    env.ok_in(&dir, &["next", "v"]);
    env.ok_in(&dir, &["next", "v", "--with-data", r#"{"move": "stay"}"#]);
    env.ok_in(&dir, &["next", "v", "--with-data", r#"{"move": "next"}"#]);
    env.ok_in(&dir, &["rewind", "v"]);
    env.ok_in(&dir, &["next", "v"]);

    let log = env.events("v");
    let seq_of = |pred: &dyn Fn(&Value) -> bool| -> u64 {
        log.iter().find(|e| pred(e)).unwrap()["seq"]
            .as_u64()
            .unwrap()
    };
    let entered = |e: &Value, from: Option<&str>, to: &str| {
        e["type"] == "transitioned"
            && e["payload"]["to"] == to
            && match from {
                Some(f) => e["payload"]["from"] == f,
                None => e["payload"]["from"].is_null(),
            }
    };
    let first = seq_of(&|e: &Value| entered(e, None, "a"));
    let self_entry = seq_of(&|e: &Value| entered(e, Some("a"), "a"));
    let into_b = seq_of(&|e: &Value| entered(e, Some("a"), "b"));
    let rewind = seq_of(&|e: &Value| e["type"] == "rewound");

    let c = env.check_records("v");
    let spans: Vec<(&str, u64, u64)> = c
        .iter()
        .map(|r| {
            (
                r["koto.state"].as_str().unwrap(),
                r["koto.seq"].as_u64().unwrap(),
                r["koto.visit.seq"].as_u64().unwrap(),
            )
        })
        .collect();
    // Records in `a` on both sides of the self-transition share the first
    // visit.
    assert!(spans
        .iter()
        .any(|(s, seq, _)| *s == "a" && *seq < self_entry));
    assert!(spans
        .iter()
        .any(|(s, seq, _)| *s == "a" && *seq > self_entry && *seq < into_b));
    for (state, seq, visit) in &spans {
        let expected = if *seq > rewind {
            rewind
        } else if *seq > into_b {
            into_b
        } else {
            first
        };
        assert_eq!(*visit, expected, "{state} at {seq}");
        if *seq > into_b && *seq < rewind {
            assert_eq!(*state, "b");
        } else {
            assert_eq!(*state, "a");
        }
    }
    let distinct: BTreeSet<u64> = spans.iter().map(|(_, _, v)| *v).collect();
    assert_eq!(distinct.len(), 3, "{spans:?}");
    assert!(spans.iter().any(|(_, seq, _)| *seq > rewind));
}

// ----- What writes nothing -----

/// An overridden gate isn't evaluated, so it writes no check record, and
/// the override's rationale stays out of the journal.
#[test]
fn an_overridden_gate_writes_no_check_record() {
    let env = Env::new();
    let dir = env.check_dir("over", &[1, 1], &[finding("E501")]);
    let t = env.template(&dir, RETRY);
    env.ok_in(&dir, &["init", "over", "--template", t.to_str().unwrap()]);
    env.koto_in(&dir, &["next", "over", "--no-cleanup"]);
    assert_eq!(
        env.check_records("over").len(),
        2,
        "one evaluation, one rule"
    );
    env.ok_in(
        &dir,
        &[
            "overrides",
            "record",
            "over",
            "--gate",
            "lint",
            "--rationale",
            "planted-rationale-5e1f",
        ],
    );
    env.ok_in(&dir, &["next", "over", "--no-cleanup"]);
    let records = env.records("over");
    assert_eq!(
        records.last().unwrap()["kind"],
        "terminal",
        "the override let the session move on: {records:?}"
    );
    assert_eq!(
        env.check_records("over").len(),
        2,
        "nothing more was relayed"
    );
    let runs = std::fs::read_to_string(dir.join("runs.log")).unwrap();
    assert_eq!(runs.lines().count(), 1, "the check didn't run again");
}

/// A polling check re-run inside one evaluation writes one record for it,
/// with the run count, and no pending record for the runs it held through.
#[test]
fn re_runs_inside_one_polling_evaluation_write_one_record() {
    let env = Env::new();
    let dir = env.check_dir("hold", &[75, 75, 0], &[]);
    let t = env.template(&dir, &polling("          hold_secs: 120\n"));
    env.ok_in(&dir, &["init", "hold", "--template", t.to_str().unwrap()]);
    env.koto_in(&dir, &["next", "hold", "--no-cleanup"]);
    let runs = std::fs::read_to_string(dir.join("runs.log")).unwrap();
    assert_eq!(runs.lines().count(), 3, "the check ran three times");
    let c = env.check_records("hold");
    assert_eq!(kinds(&c), vec!["check_evaluated"]);
    assert_eq!(c[0]["koto.check.outcome"], "passed");
    assert_eq!(c[0]["koto.poll.status"], "done");
}

/// A default action that exits 0 but whose `capture_stdout_as` can't be
/// delivered (it printed nothing) is a failed check: `failed` on its
/// `check_evaluated`, under `__action__`, with the rule record koto counted
/// for the capture failure.
#[test]
fn a_default_action_whose_capture_fails_after_exit_zero_is_journaled_as_failed() {
    let body = r#"---
name: capture-fails
version: "1.0"
initial_state: detect
states:
  detect:
    default_action:
      command: "printf ' '"
      capture_stdout_as: BRANCH
      fallback: "Find the branch name yourself."
    transitions:
      - target: report
  report:
    accepts:
      ack:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: done
        when:
          ack: ok
  done:
    terminal: true
---

## detect

Detect the branch.

## report

Working on branch {{BRANCH}}.

## done

Done.
"#;
    let env = Env::new();
    let dir = env.work.join("capture");
    std::fs::create_dir_all(&dir).unwrap();
    let t = env.template(&dir, body);
    env.ok_in(
        &dir,
        &["init", "capture", "--template", t.to_str().unwrap()],
    );
    env.koto_in(&dir, &["next", "capture", "--no-cleanup"]);

    // The log records an exit code of 0 and the capture failure's rule.
    let action = env
        .events("capture")
        .into_iter()
        .find(|e| e["type"] == "default_action_executed")
        .expect("the action ran");
    assert_eq!(action["payload"]["exit_code"], 0, "{action}");
    let logged: Vec<String> = action["payload"]["rule_counts"]
        .as_object()
        .expect("a capture failure carries rule counts")
        .keys()
        .cloned()
        .collect();
    assert_eq!(logged.len(), 1, "{action}");

    let c = env.check_records("capture");
    assert_eq!(kinds(&c), vec!["check_evaluated", "check_rule"], "{c:?}");
    assert_eq!(c[0]["koto.check"], "__action__");
    assert_eq!(c[0]["koto.state"], "detect");
    assert_eq!(c[0]["koto.check.outcome"], "failed");
    assert_eq!(c[0]["koto.check.rule_count"], 1);
    assert_eq!(c[0]["koto.seq"], action["seq"]);
    assert_eq!(c[1]["koto.check"], "__action__");
    assert_eq!(c[1]["koto.seq"], action["seq"]);
    assert_eq!(c[1]["koto.rule.id"], logged[0].as_str());
}

/// A gate or default action refused before it runs (it reads a capture
/// that hasn't been delivered) writes no check record.
#[test]
fn a_refused_gate_or_action_writes_no_check_record() {
    let gate_body = r#"---
name: refused-gate
version: "1.0"
initial_state: s
states:
  s:
    gates:
      probe:
        type: command
        command: 'printf "%s" "gate-{{TOKEN}}" > gate-out.txt'
    transitions:
      - target: producer
  producer:
    default_action:
      command: 'echo abc'
      capture_stdout_as: TOKEN
    transitions:
      - target: done
  done:
    terminal: true
---

## s

Probe.

## producer

Produce.

## done

Done.
"#;
    let action_body = r#"---
name: refused-action
version: "1.0"
initial_state: s
states:
  s:
    default_action:
      command: 'printf "%s" "using-{{TOKEN}}" > out.txt'
      fallback: "the action did not run"
    transitions:
      - target: producer
  producer:
    default_action:
      command: "echo abc"
      capture_stdout_as: TOKEN
    transitions:
      - target: done
  done:
    terminal: true
---

## s

Consume the token.

## producer

Produce it.

## done

Done.
"#;
    let env = Env::new();
    for (name, body, side_effect) in [
        ("gate", gate_body, "gate-out.txt"),
        ("action", action_body, "out.txt"),
    ] {
        let dir = env.work.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let t = env.template(&dir, body);
        env.ok_in(&dir, &["init", name, "--template", t.to_str().unwrap()]);
        let out = env.koto_in(&dir, &["next", name]);
        assert_eq!(out.code, Some(3), "{name}: {}", out.stdout);
        assert!(!dir.join(side_effect).exists(), "{name} ran");
        assert_eq!(env.check_records(name), Vec::<Value>::new(), "{name}");
        assert_eq!(
            kinds(&env.records(name)),
            vec!["session_started", "state_entered"]
        );
    }
}

/// With the session's log unwritable, the gate evaluation's commit fails
/// and writes no check record.
#[test]
fn a_failed_commit_writes_no_check_record() {
    if running_as_root() {
        eprintln!("skipped: permissions don't bind this user");
        return;
    }
    let env = Env::new();
    let dir = env.check_dir("locked", &[1], &[finding("E501")]);
    let t = env.template(&dir, RETRY);
    env.ok_in(&dir, &["init", "locked", "--template", t.to_str().unwrap()]);
    let before = std::fs::read_to_string(env.journal_path()).unwrap();
    let state = env.state_path("locked");
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o400)).unwrap();
    let out = env.koto_in(&dir, &["next", "locked"]);
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_ne!(
        out.code,
        Some(0),
        "the commit should have failed: {}",
        out.stdout
    );
    assert_eq!(std::fs::read_to_string(env.journal_path()).unwrap(), before);
}

// ----- Response records -----

/// `work` blocks on the `lint` check until it passes, then moves to `done`.
/// Its directive and details hold multi-byte characters and planted text.
const BLOCKED: &str = r#"---
name: journal-responses
version: "1.0"
initial_state: work
states:
  work:
    gates:
      lint:
        type: command
        command: ./check.sh
        timeout: 600
    transitions:
      - target: done
        when:
          gates.lint.exit_code: 0
  done:
    terminal: true
---

## work

Corrija o código — ação ✓ planted-directive-5e1f

<!-- details -->

Detalhes: não há atalho ✓ planted-directive-5e1f

## done

Done.
"#;

/// `start` waits for `go`, then `one`, `two` and `three` chain to `wait`,
/// which waits for `finish` and moves to the terminal `done` (or, on `no`,
/// back to `start`).
const CHAINED: &str = r#"---
name: journal-response-chain
version: "1.0"
initial_state: start
states:
  start:
    accepts:
      go:
        type: enum
        required: true
        values: ["yes"]
    transitions:
      - target: one
        when:
          go: "yes"
  one:
    transitions:
      - target: two
  two:
    transitions:
      - target: three
  three:
    transitions:
      - target: wait
  wait:
    accepts:
      finish:
        type: enum
        required: true
        values: ["yes", "no"]
    transitions:
      - target: done
        when:
          finish: "yes"
      - target: start
        when:
          finish: "no"
  done:
    terminal: true
---

## start

Start. planted-directive-5e1f

## one

One.

## two

Two.

## three

Three.

## wait

Wait.

<!-- details -->

Wait details.

## done

Done.
"#;

/// `a` (with details) takes `move: stay` back into itself and `move: next`
/// on to `b`, which takes `move: back` to `a`.
const TWO_VISITS: &str = r#"---
name: journal-response-visits
version: "1.0"
initial_state: a
states:
  a:
    accepts:
      move:
        type: enum
        required: true
        values: ["stay", "next"]
    transitions:
      - target: a
        when:
          move: stay
      - target: b
        when:
          move: next
  b:
    accepts:
      move:
        type: enum
        required: true
        values: ["back"]
    transitions:
      - target: a
        when:
          move: back
---

## a

Étape A — lire planted-directive-5e1f

<!-- details -->

Détails de l'étape A ✓

## b

B.
"#;

impl Env {
    fn responses(&self, session: &str) -> Vec<Value> {
        self.records(session)
            .into_iter()
            .filter(|r| r["kind"] == "response")
            .collect()
    }

    /// The highest `seq` in the session's log right now.
    fn last_seq(&self, name: &str) -> u64 {
        self.events(name)
            .iter()
            .map(|e| e["seq"].as_u64().unwrap())
            .max()
            .unwrap()
    }
}

/// The decoded UTF-8 byte length of `field` in a printed response, 0 when
/// it isn't there.
fn printed_bytes(json: &Value, field: &str) -> u64 {
    json.get(field)
        .and_then(Value::as_str)
        .map_or(0, |s| s.len() as u64)
}

fn printed(out: &Out) -> Value {
    serde_json::from_str(out.stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {}", out.stdout))
}

/// Three blocked `koto next` calls on one state write three `response`
/// records. The first carries the printed details' byte length and the
/// next two 0, since koto left the details out. Each `directive_bytes` is
/// the decoded length of the printed directive, recovery pointer and
/// multi-byte characters included, and each `koto.seq` is the log's
/// highest when it was written. The tick that reaches the terminal writes
/// none.
#[test]
fn blocked_ticks_write_one_response_each_with_details_counted_once() {
    let env = Env::new();
    let dir = env.check_dir("blocked", &[1, 1, 1, 0], &[]);
    let t = env.template(&dir, BLOCKED);
    env.ok_in(
        &dir,
        &["init", "blocked", "--template", t.to_str().unwrap()],
    );
    let arrival = env
        .events("blocked")
        .iter()
        .find(|e| e["type"] == "transitioned" && e["payload"]["to"] == "work")
        .unwrap()["seq"]
        .as_u64()
        .unwrap();
    let mut printed_sizes = Vec::new();
    for tick in 0..3 {
        let out = env.koto_in(&dir, &["next", "blocked", "--no-cleanup"]);
        assert_eq!(out.journal_warnings(), 0, "{}", out.stderr);
        let json = printed(&out);
        assert_eq!(json["action"], "gate_blocked", "tick {tick}: {json}");
        let directive = json["directive"].as_str().unwrap();
        assert!(directive.contains("koto status"), "the recovery pointer");
        assert!(directive.contains('—') && directive.contains('✓'));
        assert!(directive.len() > directive.chars().count());
        printed_sizes.push((
            printed_bytes(&json, "directive"),
            printed_bytes(&json, "details"),
        ));
        let responses = env.responses("blocked");
        assert_eq!(responses.len(), tick + 1, "{responses:?}");
        let last = responses.last().unwrap();
        assert_eq!(last["koto.seq"], env.last_seq("blocked"), "tick {tick}");
        assert_eq!(last["koto.visit.seq"], arrival, "tick {tick}");
    }
    assert!(printed_sizes[0].1 > 0, "the first tick printed details");
    assert_eq!(printed_sizes[1].1, 0, "suppressed on the second tick");
    assert_eq!(printed_sizes[2].1, 0, "suppressed on the third tick");

    let responses = env.responses("blocked");
    let recorded: Vec<(u64, u64)> = responses
        .iter()
        .map(|r| {
            assert_eq!(r["koto.state"], "work");
            assert_eq!(r["koto.response.source"], "next");
            (
                r["koto.response.directive_bytes"].as_u64().unwrap(),
                r["koto.response.details_bytes"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(recorded, printed_sizes);
    let at: BTreeSet<&str> = responses
        .iter()
        .map(|r| r["at"].as_str().unwrap())
        .collect();
    assert_eq!(at.len(), 3, "three distinct times: {responses:?}");

    // The fourth tick passes the check and lands on the terminal.
    let out = env.koto_in(&dir, &["next", "blocked", "--no-cleanup"]);
    assert_eq!(printed(&out)["action"], "done", "{}", out.stdout);
    assert_eq!(env.responses("blocked").len(), 3);
}

/// A chained `koto next` writes one response, for the state it stopped
/// in. A tick refused at validation, a tick that lands on a terminal and
/// `koto status` of a removed or terminal session write none. A directed
/// `koto next --to` writes one for its target, and `koto status` of a live
/// session one with source `status`.
#[test]
fn only_a_printed_directive_writes_a_response() {
    let env = Env::new();
    let dir = env.work.join("chained");
    std::fs::create_dir_all(&dir).unwrap();
    let t = env.template(&dir, CHAINED);
    let path = t.to_str().unwrap();
    env.ok_in(&dir, &["init", "c", "--template", path]);
    let summary = |env: &Env| -> Vec<(String, String)> {
        env.responses("c")
            .iter()
            .map(|r| {
                (
                    r["koto.state"].as_str().unwrap().to_string(),
                    r["koto.response.source"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    };
    let pair = |s: &str, src: &str| (s.to_string(), src.to_string());

    env.ok_in(&dir, &["next", "c"]);
    assert_eq!(summary(&env), vec![pair("start", "next")]);

    // Refused at validation: an error, no response.
    let out = env.koto_in(&dir, &["next", "c", "--with-data", r#"{"go": "maybe"}"#]);
    assert_ne!(out.code, Some(0), "{}", out.stdout);
    assert_eq!(out.journal_warnings(), 0, "{}", out.stderr);
    assert_eq!(summary(&env).len(), 1);

    // A chained tick through one, two and three: one response, for `wait`.
    let out = env.ok_in(&dir, &["next", "c", "--with-data", r#"{"go": "yes"}"#]);
    let json = printed(&out);
    assert_eq!(json["state"], "wait");
    let entered: Vec<String> = env
        .records("c")
        .iter()
        .filter(|r| r["kind"] == "state_entered")
        .map(|r| r["koto.state"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(entered, vec!["start", "one", "two", "three", "wait"]);
    assert_eq!(
        summary(&env),
        vec![pair("start", "next"), pair("wait", "next")]
    );
    let last = env.responses("c").pop().unwrap();
    assert_eq!(
        last["koto.response.directive_bytes"],
        printed_bytes(&json, "directive")
    );
    assert_eq!(
        last["koto.response.details_bytes"],
        printed_bytes(&json, "details")
    );
    assert_eq!(last["koto.seq"], env.last_seq("c"));

    // A directed move prints, and records, its target.
    let out = env.ok_in(&dir, &["next", "c", "--to", "start", "--rationale", "redo"]);
    assert_eq!(printed(&out)["state"], "start");
    let last = env.responses("c").pop().unwrap();
    assert_eq!(last["koto.state"], "start");
    assert_eq!(last["koto.seq"], env.last_seq("c"));
    let directed = env
        .events("c")
        .iter()
        .rev()
        .find(|e| e["type"] == "directed_transition")
        .unwrap()["seq"]
        .clone();
    assert_eq!(last["koto.visit.seq"], directed, "the move opened a visit");

    // `koto status` of a live session, with the full instructions.
    let out = env.ok_in(&dir, &["status", "c"]);
    let json = printed(&out);
    let last = env.responses("c").pop().unwrap();
    assert_eq!(last["koto.response.source"], "status");
    assert_eq!(last["koto.state"], "start");
    assert_eq!(
        last["koto.response.directive_bytes"],
        printed_bytes(&json, "directive")
    );
    assert_eq!(last["koto.response.details_bytes"], 0, "start has none");
    assert_eq!(last["koto.seq"], env.last_seq("c"));
    assert_eq!(last["koto.visit.seq"], directed);

    env.ok_in(&dir, &["next", "c", "--with-data", r#"{"go": "yes"}"#]);
    let before = summary(&env);
    assert_eq!(
        before,
        vec![
            pair("start", "next"),
            pair("wait", "next"),
            pair("start", "next"),
            pair("start", "status"),
            pair("wait", "next"),
        ]
    );

    // The terminal tick, which removes the session: no response, and none
    // from `koto status` of the removed session.
    let out = env.ok_in(&dir, &["next", "c", "--with-data", r#"{"finish": "yes"}"#]);
    assert_eq!(printed(&out)["action"], "done");
    assert!(!env.session_dir("c").exists());
    let out = env.koto_in(&dir, &["status", "c"]);
    assert_eq!(out.code, Some(2), "{}", out.stdout);
    assert_eq!(summary(&env), before);

    // `koto status` of a kept terminal session prints no directive either.
    env.ok_in(&dir, &["init", "kept", "--template", path]);
    env.ok_in(&dir, &["next", "kept", "--with-data", r#"{"go": "yes"}"#]);
    env.ok_in(
        &dir,
        &[
            "next",
            "kept",
            "--no-cleanup",
            "--with-data",
            r#"{"finish": "yes"}"#,
        ],
    );
    let count = env.responses("kept").len();
    let out = env.ok_in(&dir, &["status", "kept"]);
    assert_eq!(printed(&out)["is_terminal"], true);
    assert!(printed(&out).get("directive").is_none());
    assert_eq!(env.responses("kept").len(), count);
}

/// From a session's `response` records alone, the directive and details
/// bytes summed per visit of a state match what `koto next` and
/// `koto status` printed in that visit, across two visits to one state.
#[test]
fn response_bytes_summed_per_visit_match_the_printed_responses() {
    let env = Env::new();
    let dir = env.work.join("visits2");
    std::fs::create_dir_all(&dir).unwrap();
    let t = env.template(&dir, TWO_VISITS);
    env.ok_in(&dir, &["init", "v", "--template", t.to_str().unwrap()]);
    // (visit label, command): the label is how the test, not the journal,
    // knows which visit each printed response belongs to.
    let steps: Vec<(&str, Vec<&str>)> = vec![
        ("a1", vec!["next", "v"]),
        (
            "a1",
            vec!["next", "v", "--with-data", r#"{"move": "stay"}"#],
        ),
        ("a1", vec!["status", "v"]),
        ("a1", vec!["next", "v"]),
        ("b", vec!["next", "v", "--with-data", r#"{"move": "next"}"#]),
        (
            "a2",
            vec!["next", "v", "--with-data", r#"{"move": "back"}"#],
        ),
        ("a2", vec!["next", "v"]),
        ("a2", vec!["status", "v"]),
    ];
    let mut expected: Vec<(String, u64)> = Vec::new();
    for (label, args) in &steps {
        let out = env.ok_in(&dir, args);
        let json = printed(&out);
        let bytes = printed_bytes(&json, "directive") + printed_bytes(&json, "details");
        match expected.last_mut() {
            Some((l, sum)) if l == label => *sum += bytes,
            _ => expected.push((label.to_string(), bytes)),
        }
    }
    assert_eq!(expected.len(), 3);

    // Journal side: group by (`koto.state`, `koto.visit.seq`), in file
    // order of each group's first record.
    let mut journal: Vec<((String, u64), u64)> = Vec::new();
    for r in env.responses("v") {
        let key = (
            r["koto.state"].as_str().unwrap().to_string(),
            r["koto.visit.seq"].as_u64().unwrap(),
        );
        let bytes = r["koto.response.directive_bytes"].as_u64().unwrap()
            + r["koto.response.details_bytes"].as_u64().unwrap();
        match journal.iter_mut().find(|(k, _)| *k == key) {
            Some((_, sum)) => *sum += bytes,
            None => journal.push((key, bytes)),
        }
    }
    let states: Vec<&str> = journal.iter().map(|((s, _), _)| s.as_str()).collect();
    assert_eq!(states, vec!["a", "b", "a"], "{journal:?}");
    assert_ne!(journal[0].0 .1, journal[2].0 .1, "two visits to a");
    let sums: Vec<u64> = journal.iter().map(|(_, b)| *b).collect();
    let printed_sums: Vec<u64> = expected.iter().map(|(_, b)| *b).collect();
    assert_eq!(sums, printed_sums);
    assert_eq!(env.responses("v").len(), steps.len());
}

/// `koto status` never waits long on the journal: with the journal's lock
/// held by another process, it returns promptly with the same standard
/// output and exit code, prints the one run-journal warning, and writes no
/// record. Once the lock is free it writes one again.
#[test]
fn koto_status_returns_promptly_while_the_journal_lock_is_held() {
    use std::os::unix::io::AsRawFd;
    use std::time::{Duration, Instant};
    let env = Env::new();
    let dir = env.work.join("locked-journal");
    std::fs::create_dir_all(&dir).unwrap();
    let t = env.template(&dir, CHAINED);
    env.ok_in(&dir, &["init", "held", "--template", t.to_str().unwrap()]);
    let free = env.ok_in(&dir, &["status", "held"]);
    assert_eq!(env.responses("held").len(), 1);
    let journal_before = std::fs::read_to_string(env.journal_path()).unwrap();

    let holder = std::fs::File::open(env.journal_path()).unwrap();
    // SAFETY: flock on a descriptor `holder` owns until it is dropped.
    assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX) }, 0);
    // The same command `Env::cmd` builds, spawned so the test can bound
    // how long it waits.
    let mut status = std::process::Command::new(assert_cmd::cargo::cargo_bin("koto"));
    status
        .current_dir(&dir)
        .env("HOME", &env.home)
        .env("XDG_CACHE_HOME", env.tmp.path().join("cache"));
    for unset in [
        "KOTO_SESSIONS_BASE",
        "KOTO_HOME",
        "XDG_CONFIG_HOME",
        "KOTO_WORKFLOWS_DIR",
        "KOTO_DECIDER",
        "KOTO_DECIDER_API_KEY",
        "KOTO_DECIDER_ENDPOINT",
        "CLAUDE_CODE_SESSION_ID",
    ] {
        status.env_remove(unset);
    }
    let mut child = status
        .args(["status", "held"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    // A status that hung would block here forever; fail instead.
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > Duration::from_secs(20) {
            child.kill().unwrap();
            panic!("koto status hung behind the journal lock");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    let took = started.elapsed();
    drop(holder);
    let held = Out {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    };
    assert!(took < Duration::from_secs(5), "took {took:?}");
    assert_eq!(held.code, Some(0), "{}", held.stderr);
    assert_eq!(held.stdout, free.stdout);
    assert_eq!(held.journal_warnings(), 1, "{}", held.stderr);
    assert_eq!(
        std::fs::read_to_string(env.journal_path()).unwrap(),
        journal_before
    );

    env.ok_in(&dir, &["status", "held"]);
    assert_eq!(env.responses("held").len(), 2);
}

// ----- A journal koto can't write -----

fn running_as_root() -> bool {
    let probe = TempDir::new().unwrap();
    let ro = probe.path().join("ro");
    std::fs::create_dir(&ro).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o500)).unwrap();
    let writable = std::fs::write(ro.join("x"), "x").is_ok();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o700)).unwrap();
    writable
}

/// `value` with every string that varies between runs (ids, times, the
/// temporary directory) and every timing field replaced by a marker.
fn masked(value: &Value, tmp: &str) -> Value {
    let uuid =
        regex::Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}").unwrap();
    let time = regex::Regex::new(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?Z").unwrap();
    match value {
        Value::String(s) => {
            let s = s.replace(tmp, "<tmp>");
            let s = uuid.replace_all(&s, "<id>");
            Value::String(time.replace_all(&s, "<time>").into_owned())
        }
        Value::Array(items) => Value::Array(items.iter().map(|v| masked(v, tmp)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let v = if matches!(
                        k.as_str(),
                        "duration_ms"
                            | "elapsed_secs"
                            | "since"
                            | "timestamp"
                            | "created_at"
                            | "session_id"
                    ) {
                        Value::from("<masked>")
                    } else {
                        masked(v, tmp)
                    };
                    (k.clone(), v)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// One command's exit code, masked standard output and journal warnings.
type Step = (Option<i32>, Value, usize);

/// The converged retry's commands in `env`: each one's exit code, masked
/// standard output and journal warning count, then the masked session log.
fn retry_outcome(env: &Env) -> (Vec<Step>, Vec<Value>) {
    let dir = env.check_dir("same", &[1, 1, 1, 0], &[finding("E501")]);
    let t = env.template(&dir, RETRY);
    let tmp = env.tmp.path().to_string_lossy().to_string();
    let mut steps = vec![vec![
        "init".to_string(),
        "same".to_string(),
        "--template".to_string(),
        t.to_string_lossy().to_string(),
    ]];
    for tick in 0..4 {
        // `koto status` before the passing tick writes a `response` too.
        if tick == 3 {
            steps.push(vec!["status".to_string(), "same".to_string()]);
        }
        steps.push(vec![
            "next".to_string(),
            "same".to_string(),
            "--no-cleanup".to_string(),
        ]);
    }
    let mut outs = Vec::new();
    for step in &steps {
        let args: Vec<&str> = step.iter().map(String::as_str).collect();
        let out = env.koto_in(&dir, &args);
        let json: Value = serde_json::from_str(out.stdout.trim()).unwrap_or(Value::Null);
        outs.push((out.code, masked(&json, &tmp), out.journal_warnings()));
    }
    let log = std::fs::read_to_string(env.state_path("same"))
        .unwrap()
        .lines()
        .map(|l| masked(&serde_json::from_str(l).unwrap(), &tmp))
        .collect();
    (outs, log)
}

/// With the journal unwritable (a read-only koto home, or a symlink at its
/// path), the check scenario, `koto status` included, gives the same exit
/// codes, standard output and session log as with a writable journal, and
/// each command prints exactly one run-journal warning.
#[test]
fn an_unwritable_journal_changes_nothing_but_one_warning_per_command() {
    if running_as_root() {
        eprintln!("skipped: permissions don't bind this user");
        return;
    }
    let good = Env::new();
    let (expected, expected_log) = retry_outcome(&good);
    assert!(expected.iter().all(|(_, _, w)| *w == 0));
    // Every command printed a JSON response, so the comparison below has
    // something to compare.
    assert!(
        expected.iter().all(|(_, json, _)| json.is_object()),
        "{expected:?}"
    );
    assert_eq!(
        good.check_records("same").len(),
        7,
        "four evaluations and three rules"
    );
    let sources: Vec<Value> = good
        .records("same")
        .iter()
        .filter(|r| r["kind"] == "response")
        .map(|r| r["koto.response.source"].clone())
        .collect();
    assert_eq!(
        sources,
        vec!["next", "next", "next", "status"],
        "three blocked ticks and a status"
    );

    let read_only = Env::new();
    std::fs::create_dir_all(read_only.koto_home().join("sessions")).unwrap();
    std::fs::set_permissions(
        read_only.koto_home(),
        std::fs::Permissions::from_mode(0o500),
    )
    .unwrap();
    let got = retry_outcome(&read_only);
    std::fs::set_permissions(
        read_only.koto_home(),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    assert!(!read_only.journal_path().exists());

    let linked = Env::new();
    std::fs::create_dir_all(linked.koto_home()).unwrap();
    let target = linked.tmp.path().join("not-the-journal.txt");
    std::fs::write(&target, "untouched\n").unwrap();
    std::os::unix::fs::symlink(&target, linked.journal_path()).unwrap();
    let got_linked = retry_outcome(&linked);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched\n");

    for (label, (outs, log)) in [("read-only", got), ("symlink", got_linked)] {
        assert_eq!(outs.len(), expected.len());
        for (i, ((code, stdout, warnings), (e_code, e_stdout, _))) in
            outs.iter().zip(&expected).enumerate()
        {
            assert_eq!(code, e_code, "{label} step {i}");
            assert_eq!(stdout, e_stdout, "{label} step {i}");
            assert_eq!(*warnings, 1, "{label} step {i}");
        }
        assert_eq!(log, expected_log, "{label}");
    }
}
