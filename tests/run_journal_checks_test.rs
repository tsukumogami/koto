//! Check records in the run journal (`check_evaluated`, `check_rule`,
//! `check_pending`), written by koto commands and read back the way tooling
//! outside koto reads them (see `docs/reference/run-journal.md`).
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
    "planted-context-5e1f",
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
    for _ in 0..4 {
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
/// path), the check scenario gives the same exit codes, standard output
/// and session log as with a writable journal, and each command prints
/// exactly one run-journal warning.
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

// ----- Context reads -----

/// `check` waits on a context-matches gate over `note` and a
/// context-exists gate over `spec`, then moves to `work`; `work` goes back
/// to `check` or on to `done`, a terminal whose result map reads `note`.
const CONTEXT: &str = r#"---
name: journal-context
version: "1.0"
initial_state: check
states:
  check:
    gates:
      note_ok:
        type: context-matches
        key: note
        pattern: "^ok"
      has_spec:
        type: context-exists
        key: spec
    transitions:
      - target: work
  work:
    accepts:
      go:
        type: enum
        required: true
        values: [back, done]
    transitions:
      - target: check
        when:
          go: back
      - target: done
        when:
          go: done
  done:
    terminal: true
    result:
      note: "${context.note}"
---

## check

Write the note. planted-directive-5e1f

## work

Work.

## done

Done.
"#;

/// A context value that must never reach the journal.
fn planted(label: &str) -> String {
    format!("{label} planted-context-5e1f")
}

impl Env {
    /// A working directory for one session of [`CONTEXT`], initialized.
    fn context_session(&self, name: &str) -> PathBuf {
        let dir = self.work.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let t = self.template(&dir, CONTEXT);
        self.ok_in(&dir, &["init", name, "--template", t.to_str().unwrap()]);
        dir
    }

    /// `koto context add` of `content` under `key`.
    fn add(&self, dir: &Path, name: &str, key: &str, content: &str) {
        let file = dir.join("context-input.txt");
        std::fs::write(&file, content).unwrap();
        self.ok_in(
            dir,
            &[
                "context",
                "add",
                name,
                key,
                "--from-file",
                file.to_str().unwrap(),
            ],
        );
    }

    fn context_records(&self, session: &str) -> Vec<Value> {
        self.records(session)
            .into_iter()
            .filter(|r| r["kind"] == "context_reads")
            .collect()
    }
}

fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// The seq of the first event in `log` at or after `from` that `pred`
/// matches.
fn seq_where(log: &[Value], from: u64, pred: impl Fn(&Value) -> bool) -> u64 {
    log.iter()
        .find(|e| e["seq"].as_u64().unwrap() >= from && pred(e))
        .unwrap_or_else(|| panic!("no such event from {from}"))["seq"]
        .as_u64()
        .unwrap()
}

fn entered(e: &Value, to: &str) -> bool {
    (e["type"] == "transitioned" || e["type"] == "rewound") && e["payload"]["to"] == to
}

/// The (key, reader, access) tuples of the `context_read` events with seq
/// in `(after, until)`, in first-read order.
fn first_reads(log: &[Value], after: u64, until: u64) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = Vec::new();
    for e in log {
        let seq = e["seq"].as_u64().unwrap();
        if e["type"] != "context_read" || seq <= after || seq >= until {
            continue;
        }
        let p = &e["payload"];
        let t = (
            p["key"].as_str().unwrap().to_string(),
            p["reader"].as_str().unwrap().to_string(),
            p["access"].as_str().unwrap_or("content").to_string(),
        );
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

fn tuple_of(r: &Value) -> (String, String, String) {
    let s = |k: &str| r[k].as_str().unwrap().to_string();
    (
        s("koto.context.key"),
        s("koto.context.reader"),
        s("koto.context.access"),
    )
}

/// A state with context gates is evaluated on three ticks, left,
/// re-entered and evaluated once, then the session reaches its terminal.
/// Each visit's reads are written once, in first-read order, right before
/// the `state_entered` (or, for the terminal's result reads, the
/// `terminal`) that ended it. Within the first visit, two keys the gates
/// read and one `koto context get` read give three records, and the note,
/// read once absent and twice present, counts 3 and 2 with the second and
/// third reads' hashes. Reads on the kept terminal afterwards aren't
/// journaled, and a rewind out of it writes no reads.
#[test]
fn context_reads_are_written_once_per_visit_before_the_record_that_ends_it() {
    let env = Env::new();
    let dir = env.context_session("ctx");
    env.add(&dir, "ctx", "spec", &planted("spec"));
    let blocked = env.koto_in(&dir, &["next", "ctx", "--no-cleanup"]);
    assert!(
        blocked.stdout.contains("gate_blocked"),
        "{}",
        blocked.stdout
    );
    assert_eq!(blocked.journal_warnings(), 0, "{}", blocked.stderr);
    let nope = planted("nope");
    env.add(&dir, "ctx", "note", &nope);
    env.koto_in(&dir, &["next", "ctx", "--no-cleanup"]);
    env.add(&dir, "ctx", "plan", &planted("plan"));
    env.ok_in(&dir, &["context", "get", "ctx", "plan"]);
    let ok = format!("ok {}", planted("note"));
    env.add(&dir, "ctx", "note", &ok);
    let moved = env.ok_in(&dir, &["next", "ctx", "--no-cleanup"]);
    assert!(moved.stdout.contains("\"work\""), "{}", moved.stdout);
    env.ok_in(
        &dir,
        &[
            "next",
            "ctx",
            "--no-cleanup",
            "--with-data",
            r#"{"go": "back"}"#,
        ],
    );
    env.ok_in(
        &dir,
        &[
            "next",
            "ctx",
            "--no-cleanup",
            "--with-data",
            r#"{"go": "done"}"#,
        ],
    );

    let log = env.events("ctx");
    let first = seq_where(&log, 0, |e| entered(e, "check"));
    let left = seq_where(&log, first, |e| entered(e, "work"));
    let back = seq_where(&log, left, |e| entered(e, "check"));
    let left_again = seq_where(&log, back, |e| entered(e, "work"));
    let terminal = seq_where(&log, left_again, |e| entered(e, "done"));
    let result = seq_where(&log, terminal, |e| e["type"] == "request_store.result");

    let c = env.context_records("ctx");
    let visit = |v: u64| -> Vec<&Value> { c.iter().filter(|r| r["koto.visit.seq"] == v).collect() };
    // The first visit: the gates' two keys and the `context get`, in the
    // order they were first read.
    let one = visit(first);
    assert_eq!(
        one.iter().map(|r| tuple_of(r)).collect::<Vec<_>>(),
        first_reads(&log, first, left)
    );
    assert_eq!(one.len(), 3, "{one:?}");
    let note = one
        .iter()
        .find(|r| r["koto.context.key"] == "note")
        .unwrap();
    assert_eq!(note["koto.context.reader"], "gate");
    assert_eq!(note["koto.context.read_count"], 3);
    assert_eq!(note["koto.context.present_count"], 2);
    assert_eq!(note["koto.context.first_hash"], sha256_hex(&nope));
    assert_eq!(note["koto.context.last_hash"], sha256_hex(&ok));
    let spec = one
        .iter()
        .find(|r| r["koto.context.key"] == "spec")
        .unwrap();
    assert_eq!(spec["koto.context.access"], "presence");
    assert_eq!(spec["koto.context.read_count"], 3);
    let plan = one
        .iter()
        .find(|r| r["koto.context.key"] == "plan")
        .unwrap();
    assert_eq!(plan["koto.context.reader"], "cli");
    assert_eq!(plan["koto.context.read_count"], 1);
    // The second visit to `check`, evaluated once.
    let two = visit(back);
    assert_eq!(
        two.iter().map(|r| tuple_of(r)).collect::<Vec<_>>(),
        first_reads(&log, back, left_again)
    );
    let note_two = two
        .iter()
        .find(|r| r["koto.context.key"] == "note")
        .unwrap();
    assert_eq!(note_two["koto.context.read_count"], 1);
    // The terminal's result reads.
    let done = visit(terminal);
    assert_eq!(done.len(), 1, "{done:?}");
    assert_eq!(done[0]["koto.context.reader"], "result");
    assert_eq!(done[0]["koto.state"], "done");
    assert_eq!(c.len(), 3 + two.len() + 1, "{c:?}");

    // Each record carries the seq of the event that ended its visit, the
    // highest in the log when it was written.
    for (records, closer, state) in [
        (&one, left, "check"),
        (&two, left_again, "check"),
        (&done, result, "done"),
    ] {
        for r in records {
            assert_eq!(r["koto.seq"], closer, "{r}");
            assert_eq!(r["koto.state"], state, "{r}");
        }
    }

    // File order: each visit's reads, then the record that ended it.
    let records = env.records("ctx");
    let k = kinds(&records);
    let mut i = 0;
    let mut enders = Vec::new();
    while i < k.len() {
        if k[i] == "context_reads" {
            while k[i] == "context_reads" {
                i += 1;
            }
            enders.push((k[i], records[i]["koto.state"].clone()));
        }
        i += 1;
    }
    assert_eq!(
        enders,
        vec![
            ("state_entered", Value::from("work")),
            ("state_entered", Value::from("work")),
            ("terminal", Value::Null),
        ]
    );

    // Reads on the kept terminal, then a rewind out of it.
    env.ok_in(&dir, &["context", "get", "ctx", "note"]);
    env.ok_in(&dir, &["rewind", "ctx"]);
    assert_eq!(env.context_records("ctx").len(), c.len());
    assert_eq!(kinds(&env.records("ctx")).last(), Some(&"state_entered"));
}

/// A cancel writes the open visit's reads right before `cancelled`, and a
/// read after the cancel writes nothing.
#[test]
fn a_cancel_writes_the_open_visits_reads_before_cancelled() {
    let env = Env::new();
    let dir = env.context_session("gone");
    env.add(&dir, "gone", "spec", &planted("spec"));
    env.koto_in(&dir, &["next", "gone"]);
    env.koto_in(&dir, &["next", "gone"]);
    env.ok_in(&dir, &["cancel", "gone"]);
    env.ok_in(&dir, &["context", "get", "gone", "spec"]);
    let log = env.events("gone");
    let cancel = seq_where(&log, 0, |e| e["type"] == "workflow_cancelled");
    let records = env.records("gone");
    let k = kinds(&records);
    assert_eq!(
        &k[k.len() - 3..],
        &["context_reads", "context_reads", "cancelled"]
    );
    for r in env.context_records("gone") {
        assert_eq!(r["koto.context.read_count"], 2, "{r}");
        assert_eq!(r["koto.seq"], cancel);
        assert_eq!(r["koto.state"], "check");
    }
}

/// A respawn fallback in another process cancels a session whose reads
/// this process's `koto next` logged: that cancel writes them.
#[test]
fn a_respawn_cancel_from_another_process_writes_the_open_visits_reads() {
    use koto::engine::respawn::{
        execute_respawn, RespawnExecuted, RespawnExecution, RespawnRequest, SubstrateRespawner,
    };
    use koto::engine::types::ValidatedSessionId;
    use koto::session::local::LocalBackend;
    use koto::session::SessionBackend;
    use std::time::{Duration, SystemTime};

    struct Refuses;
    impl SubstrateRespawner for Refuses {
        fn respawn(&self, r: &RespawnRequest) -> Result<(), koto::engine::errors::EngineError> {
            Err(koto::engine::errors::EngineError::StateNotFound(
                r.session_id.as_str().to_string(),
            ))
        }
    }

    let env = Env::new();
    let dir = env.context_session("req");
    env.koto_in(&dir, &["next", "req"]);
    // This process has no driver, as the koto commands above had none.
    std::env::remove_var("CLAUDE_CODE_SESSION_ID");
    let backend =
        LocalBackend::with_base_dir_and_journal(env.koto_home().join("sessions"), env.koto_home());
    let header = backend.read_header("req").unwrap();
    assert!(header.role.is_none());
    let sid = ValidatedSessionId::new("req").unwrap();
    let now = SystemTime::now();
    let woken_at = now - Duration::from_secs(60 * 60 * 24 * 60);
    let outcome = execute_respawn(
        &RespawnExecution {
            backend: &backend,
            header: &header,
            coord_id: "coord",
            requester_session_id: &sid,
            woken_at: Some(woken_at),
            last_log_activity: woken_at - Duration::from_secs(60),
            now,
            retention_floor: Duration::from_secs(60 * 60 * 24 * 30),
            cap: 2,
            template_exists: true,
        },
        &Refuses,
    )
    .unwrap();
    assert!(
        matches!(outcome, RespawnExecuted::Abandoned { .. }),
        "{outcome:?}"
    );

    let log = env.events("req");
    let cancel = seq_where(&log, 0, |e| e["type"] == "workflow_cancelled");
    let records = env.records("req");
    let k = kinds(&records);
    assert_eq!(k.last(), Some(&"cancelled"));
    let c = env.context_records("req");
    assert_eq!(c.len(), 2, "{c:?}");
    for r in &c {
        assert_eq!(r["koto.context.read_count"], 1, "{r}");
        assert_eq!(r["koto.seq"], cancel);
    }
    assert_eq!(
        &k[k.len() - 3..k.len() - 1],
        &["context_reads", "context_reads"]
    );
}

/// The context scenario's commands in `env`: each one's exit code, masked
/// standard output and journal warning count, whether it wrote to the
/// journal, then the masked session log.
fn context_outcome(env: &Env) -> (Vec<(Step, bool)>, Vec<Value>) {
    let dir = env.work.join("same");
    std::fs::create_dir_all(&dir).unwrap();
    let t = env.template(&dir, CONTEXT);
    let input = dir.join("input.txt");
    std::fs::write(&input, planted("ok")).unwrap();
    let tmp = env.tmp.path().to_string_lossy().to_string();
    let file = input.to_string_lossy().to_string();
    let steps: Vec<Vec<&str>> = vec![
        vec!["init", "same", "--template", t.to_str().unwrap()],
        vec!["next", "same", "--no-cleanup"],
        vec!["context", "add", "same", "spec", "--from-file", &file],
        vec!["context", "add", "same", "note", "--from-file", &file],
        vec!["context", "get", "same", "note"],
        vec!["next", "same", "--no-cleanup"],
        vec![
            "next",
            "same",
            "--no-cleanup",
            "--with-data",
            r#"{"go":"done"}"#,
        ],
    ];
    let mut outs = Vec::new();
    for args in &steps {
        let before = std::fs::metadata(env.journal_path()).map_or(0, |m| m.len());
        let out = env.koto_in(&dir, args);
        let after = std::fs::metadata(env.journal_path()).map_or(0, |m| m.len());
        let json: Value = serde_json::from_str(out.stdout.trim()).unwrap_or(Value::Null);
        outs.push((
            (out.code, masked(&json, &tmp), out.journal_warnings()),
            after != before,
        ));
    }
    let log = std::fs::read_to_string(env.state_path("same"))
        .unwrap()
        .lines()
        .map(|l| masked(&serde_json::from_str(l).unwrap(), &tmp))
        .collect();
    (outs, log)
}

/// With the journal unwritable, the context scenario gives the same exit
/// codes, standard output and session log as with a writable one, and each
/// command that writes records prints exactly one run-journal warning.
#[test]
fn an_unwritable_journal_changes_nothing_in_the_context_scenario() {
    if running_as_root() {
        eprintln!("skipped: permissions don't bind this user");
        return;
    }
    let good = Env::new();
    let (expected, expected_log) = context_outcome(&good);
    assert!(expected.iter().all(|((_, _, w), _)| *w == 0));
    assert!(
        good.context_records("same").len() >= 3,
        "{:?}",
        good.records("same")
    );

    let read_only = Env::new();
    std::fs::create_dir_all(read_only.koto_home().join("sessions")).unwrap();
    std::fs::set_permissions(
        read_only.koto_home(),
        std::fs::Permissions::from_mode(0o500),
    )
    .unwrap();
    let got = context_outcome(&read_only);
    std::fs::set_permissions(
        read_only.koto_home(),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();

    let linked = Env::new();
    std::fs::create_dir_all(linked.koto_home()).unwrap();
    let target = linked.tmp.path().join("not-the-journal.txt");
    std::fs::write(&target, "untouched\n").unwrap();
    std::os::unix::fs::symlink(&target, linked.journal_path()).unwrap();
    let got_linked = context_outcome(&linked);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched\n");

    for (label, (outs, log)) in [("read-only", got), ("symlink", got_linked)] {
        assert_eq!(outs.len(), expected.len());
        for (i, (((code, stdout, warnings), _), ((e_code, e_stdout, _), wrote))) in
            outs.iter().zip(&expected).enumerate()
        {
            assert_eq!(code, e_code, "{label} step {i}");
            assert_eq!(stdout, e_stdout, "{label} step {i}");
            assert_eq!(*warnings, usize::from(*wrote), "{label} step {i}");
        }
        assert_eq!(log, expected_log, "{label}");
    }
}
