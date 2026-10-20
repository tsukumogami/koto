//! Check records in the run journal (`check_evaluated`, `check_rule`,
//! `check_pending`), the decider and override records relayed beside them
//! (`decider_consulted`, `decider_checked`, `check_overridden`), the
//! `response` records of printed directives, and the `context_reads` a
//! visit writes when it ends, written by koto commands and read back the
//! way tooling outside koto reads them (see `docs/reference/run-journal.md`).
//!
//! Every test runs the koto binary with `HOME` in its own temporary
//! directory, `KOTO_SESSIONS_BASE` unset and `CLAUDE_CODE_SESSION_ID`
//! cleared, so the session store and the journal are that test's own and
//! the developer's journal is never touched. A test that consults the
//! decider points it at the in-process stub, so no test reaches a real
//! provider.
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

#[path = "support/decider_stub.rs"]
mod decider_stub;

use assert_cmd::Command;
use decider_stub::{DeciderStub, Reply};
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

/// Values the tests plant in checks, commands, overrides, directives and
/// context content. None may reach the journal.
const PLANTED: &[&str] = &[
    "planted-message-5e1f",
    "planted/dir-5e1f",
    "planted-arg-5e1f",
    "planted-rationale-5e1f",
    "planted-directive-5e1f",
    "planted-context-5e1f",
    "planted-ref-5e1f",
    "planted-question-5e1f",
    "planted-slice-5e1f",
    // The decider's key and the stub's endpoint.
    DECIDER_KEY,
    "127.0.0.1",
    "systemone",
];

/// The key every command that consults the decider sends.
const DECIDER_KEY: &str = "sk-journal-planted-key-5e1f";

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
        Command::from_std(self.std_cmd(cwd))
    }

    /// [`Env::cmd`] as a `std::process::Command`, for a test that spawns
    /// koto and bounds how long it waits. Every koto command a test runs is
    /// built here: this home, this cache, and none of the developer's
    /// variables that would redirect the store, the config, the workflows
    /// directory, the decider or the driver.
    fn std_cmd(&self, cwd: &Path) -> std::process::Command {
        let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("koto"));
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
        let output = self
            .cmd(cwd)
            .envs(harness_session(args))
            .args(args)
            .output()
            .unwrap();
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
        // `check_finding` names its gate under `koto.check.name`.
        let check = if r["kind"] == "check_finding" {
            &r["koto.check.name"]
        } else {
            &r["koto.check"]
        };
        assert_eq!(check, "lint", "{r}");
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
    // fallback rule; its rule record, and the fallback finding's record,
    // share the evaluation's seq.
    let (findings, all): (Vec<Value>, Vec<Value>) =
        all.into_iter().partition(|r| r["kind"] == "check_finding");
    let (rules, c): (Vec<Value>, Vec<Value>) =
        all.into_iter().partition(|r| r["kind"] == "check_rule");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0]["koto.seq"], c[2]["koto.seq"]);
    assert_eq!(findings[0]["koto.finding.level"], "error");
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
/// record and a rule count of 0, and journals the finding as a
/// `check_finding`.
#[test]
fn a_passing_check_with_an_error_finding_writes_no_rule_record() {
    let env = Env::new();
    run_retry(&env, "clean", &[0], &["E501"], false);
    let c = env.check_records("clean");
    assert_eq!(kinds(&c), vec!["check_evaluated", "check_finding"]);
    assert_eq!(c[0]["koto.check.finding_count"], 1);
    assert_eq!(c[1]["koto.rule.id"], "E501");
    assert_eq!(c[1]["koto.finding.level"], "error");
    assert_eq!(c[1]["koto.finding.count"], 1);
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
    assert_eq!(
        kinds(&c),
        vec!["check_evaluated", "check_rule", "check_finding"],
        "{c:?}"
    );
    assert_eq!(c[2]["koto.rule.id"], logged[0].as_str());
    assert_eq!(c[2]["koto.finding.level"], "error");
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
    // from `koto status` of the removed session, which answers from the
    // finished record koto wrote before removing it and prints no directive.
    let out = env.ok_in(&dir, &["next", "c", "--with-data", r#"{"finish": "yes"}"#]);
    assert_eq!(printed(&out)["action"], "done");
    assert!(!env.session_dir("c").exists());
    let out = env.koto_in(&dir, &["status", "c"]);
    assert_eq!(out.code, Some(0), "{}", out.stdout);
    assert_eq!(printed(&out)["removed"], true);
    assert_eq!(summary(&env), before);

    // An error exit writes none either: a name that never existed and has no
    // finished record answers `not found`.
    let out = env.koto_in(&dir, &["status", "never-was"]);
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
    status_while_the_journal_lock_is_held(None);
}

/// The same with rotation on and the journal already past its bound: the
/// held lock is waited for no longer than without rotation, and nothing is
/// rotated while it's held. Once the lock is free, the next `koto status`
/// rotates the journal and its `response` record lands whole in the fresh
/// file.
#[test]
fn koto_status_returns_promptly_while_the_journal_lock_is_held_with_rotation_on() {
    status_while_the_journal_lock_is_held(Some(1));
}

/// The rotated files beside the journal, by the naming rule, oldest first.
fn rotated_journals(env: &Env) -> Vec<PathBuf> {
    let mut names: Vec<String> = std::fs::read_dir(env.koto_home())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| {
            n.strip_prefix("_run_journal.")
                .and_then(|r| r.strip_suffix(".jsonl"))
                .is_some_and(|d| d.len() == 10 && d.bytes().all(|b| b.is_ascii_digit()))
        })
        .collect();
    names.sort();
    names.into_iter().map(|n| env.koto_home().join(n)).collect()
}

/// `koto status` against a journal whose lock another process holds, with
/// `run_journal.rotate_bytes` set to `rotate_bytes` (left unset for
/// `None`) once the session's first response is written.
fn status_while_the_journal_lock_is_held(rotate_bytes: Option<u64>) {
    use std::os::unix::io::AsRawFd;
    use std::time::{Duration, Instant};
    let env = Env::new();
    let dir = env.work.join("locked-journal");
    std::fs::create_dir_all(&dir).unwrap();
    let t = env.template(&dir, CHAINED);
    env.ok_in(&dir, &["init", "held", "--template", t.to_str().unwrap()]);
    let free = env.ok_in(&dir, &["status", "held"]);
    assert_eq!(env.responses("held").len(), 1);
    if let Some(bytes) = rotate_bytes {
        env.ok_in(
            &dir,
            &[
                "config",
                "set",
                "run_journal.rotate_bytes",
                &bytes.to_string(),
                "--user",
            ],
        );
    }
    let journal_before = std::fs::read_to_string(env.journal_path()).unwrap();
    if let Some(bytes) = rotate_bytes {
        assert!(journal_before.len() as u64 >= bytes);
    }

    let holder = std::fs::File::open(env.journal_path()).unwrap();
    // SAFETY: flock on a descriptor `holder` owns until it is dropped.
    assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX) }, 0);
    // Spawned so the test can bound how long it waits.
    let mut child = env
        .std_cmd(&dir)
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
    assert!(rotated_journals(&env).is_empty());

    env.ok_in(&dir, &["status", "held"]);
    if rotate_bytes.is_none() {
        assert_eq!(env.responses("held").len(), 2);
        assert!(rotated_journals(&env).is_empty());
        return;
    }
    // The journal as it stood moved, byte for byte, to the first rotated
    // name; the fresh journal holds the one response and nothing else.
    let rotated = rotated_journals(&env);
    assert_eq!(rotated.len(), 1, "{rotated:?}");
    assert!(rotated[0].ends_with("_run_journal.0000000001.jsonl"));
    assert_eq!(
        std::fs::read_to_string(&rotated[0]).unwrap(),
        journal_before
    );
    let fresh = std::fs::read_to_string(env.journal_path()).unwrap();
    assert_eq!(fresh.lines().count(), 1, "{fresh}");
    assert!(fresh.ends_with('\n'), "{fresh:?}");
    let records = env.responses("held");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["koto.response.source"], "status");
}

// ----- Every journal write's lock wait is bounded -----

/// The variable a debug build of koto reads in place of the journal's
/// five-second lock wait, in milliseconds. Release builds ignore it, so
/// the tests that use it skip themselves there.
const LOCK_WAIT_ENV: &str = "KOTO_TEST_JOURNAL_LOCK_WAIT_MS";

/// The variable a debug build reads to hold its first rotation's slow step
/// (just before the rotated file's flush) until the file it names is
/// removed.
const ROTATE_HOLD_ENV: &str = "KOTO_TEST_JOURNAL_ROTATE_HOLD";

/// The lock wait these tests give koto through [`LOCK_WAIT_ENV`].
const TEST_BOUND: std::time::Duration = std::time::Duration::from_millis(200);

/// The journal's real bound. A command that took this long per write would
/// mean the test bound wasn't applied.
const JOURNAL_BOUND: std::time::Duration = std::time::Duration::from_secs(5);

/// Whether the koto binary under test reads the test variables, which only
/// a debug build does. The test can't ask the binary, so this reads the
/// test crate's own `debug_assertions` and assumes cargo built the binary
/// in the same profile, as `cargo test` does unless a profile override
/// sets the two apart. Under such an override this answer can be wrong.
fn seams_available() -> bool {
    if cfg!(debug_assertions) {
        return true;
    }
    eprintln!("skipped: a release build doesn't read the journal test variables");
    false
}

/// Take the journal's lock, as a writer that stopped while holding it would,
/// until the returned file is dropped.
fn hold_journal_lock(env: &Env) -> std::fs::File {
    use std::os::unix::io::AsRawFd;
    let holder = std::fs::File::open(env.journal_path()).unwrap();
    // SAFETY: flock on a descriptor `holder` owns until it is dropped.
    assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX) }, 0);
    holder
}

/// [`Env::std_cmd`] with the journal's lock wait set to [`TEST_BOUND`].
fn bounded_cmd(env: &Env, dir: &Path, args: &[&str]) -> std::process::Command {
    let mut cmd = env.std_cmd(dir);
    cmd.env(LOCK_WAIT_ENV, TEST_BOUND.as_millis().to_string())
        .env_remove(ROTATE_HOLD_ENV)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    cmd
}

/// Wait for `child` to exit, failing the test instead of hanging when it
/// runs past 20 seconds. Returns its output and how long it ran from
/// `started`.
fn finish(
    mut child: std::process::Child,
    started: std::time::Instant,
    what: &str,
) -> (Out, std::time::Duration) {
    use std::time::Duration;
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > Duration::from_secs(20) {
            child.kill().unwrap();
            panic!("{what} hung behind the journal lock");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let took = started.elapsed();
    let output = child.wait_with_output().unwrap();
    let out = Out {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    };
    (out, took)
}

/// Run `koto args` from `dir` with the journal's lock wait at
/// [`TEST_BOUND`], to the end.
fn run_bounded(env: &Env, dir: &Path, args: &[&str]) -> (Out, std::time::Duration) {
    let started = std::time::Instant::now();
    let child = bounded_cmd(env, dir, args).spawn().unwrap();
    finish(child, started, &format!("koto {args:?}"))
}

/// Two sessions of [`CHAINED`] in a fresh directory, `free` and `held`,
/// each at `start`.
fn twin_sessions(env: &Env, label: &str) -> PathBuf {
    let dir = env.work.join(label);
    std::fs::create_dir_all(&dir).unwrap();
    let t = env.template(&dir, CHAINED);
    for name in ["free", "held"] {
        env.ok_in(&dir, &["init", name, "--template", t.to_str().unwrap()]);
    }
    dir
}

/// `koto next` never hangs on the journal: with the journal's lock held by
/// another process past the bound, a tick that chains through four states
/// and prints a directive exits with the same code and standard output as
/// the same tick with the lock free, prints one run-journal warning, and
/// writes no record. Each of its journal writes gives up after the bound,
/// not the real five seconds, and none waits indefinitely. The session
/// itself moves exactly as the free one does, and once the lock is free its
/// records are written again.
#[test]
fn koto_next_exits_normally_while_the_journal_lock_is_held_past_the_bound() {
    if !seams_available() {
        return;
    }
    let env = Env::new();
    let dir = twin_sessions(&env, "next-held");
    let go = ["--with-data", r#"{"go":"yes"}"#];
    let free = env.ok_in(&dir, &[&["next", "free"][..], &go].concat());
    let journal_before = std::fs::read_to_string(env.journal_path()).unwrap();

    let holder = hold_journal_lock(&env);
    let (held, took) = run_bounded(&env, &dir, &[&["next", "held"][..], &go].concat());
    drop(holder);

    assert_eq!(held.code, free.code, "{}", held.stderr);
    assert_eq!(held.code, Some(0));
    assert_eq!(held.stdout, free.stdout);
    assert_eq!(held.journal_warnings(), 1, "{}", held.stderr);
    assert!(
        held.stderr.contains("held by another writer"),
        "{}",
        held.stderr
    );
    assert!(took >= TEST_BOUND, "took {took:?}");
    assert!(took < JOURNAL_BOUND, "took {took:?}");
    assert_eq!(
        std::fs::read_to_string(env.journal_path()).unwrap(),
        journal_before
    );
    // The session is where the free one is: the skipped records were the
    // journal's only loss.
    let state = |name: &str| -> Vec<String> {
        env.events(name)
            .iter()
            .filter(|e| e["type"] == "transitioned")
            .map(|e| e["payload"]["to"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(state("held"), state("free"));

    env.ok_in(&dir, &["status", "held"]);
    assert_eq!(env.responses("held").len(), 1);
}

/// A held lock costs a command one wait, not one per batch: a `koto next`
/// that chains through four states writes five batches with the lock free,
/// and with the lock held it gives up on the first after the bound and
/// skips the other four without waiting. It finishes in about one bound,
/// with the free run's exit code and standard output and one warning.
#[test]
fn a_chained_koto_next_waits_out_a_held_lock_once_not_once_per_batch() {
    use std::time::Duration;
    if !seams_available() {
        return;
    }
    // Long enough that one wait per batch can't pass for one wait plus the
    // command's own run time.
    let bound = Duration::from_secs(1);
    let env = Env::new();
    let dir = twin_sessions(&env, "next-chained-held");
    let go = ["--with-data", r#"{"go":"yes"}"#];
    let lines = || {
        std::fs::read_to_string(env.journal_path())
            .unwrap()
            .lines()
            .count()
    };
    let before_free = lines();
    let free = env.ok_in(&dir, &[&["next", "free"][..], &go].concat());
    let records = lines() - before_free;
    assert_eq!(records, 5, "four state entries and one response");
    let journal_before = std::fs::read_to_string(env.journal_path()).unwrap();

    let holder = hold_journal_lock(&env);
    let started = std::time::Instant::now();
    let child = bounded_cmd(&env, &dir, &[&["next", "held"][..], &go].concat())
        .env(LOCK_WAIT_ENV, bound.as_millis().to_string())
        .spawn()
        .unwrap();
    let (held, took) = finish(child, started, "the chained koto next");
    drop(holder);

    assert_eq!(held.code, free.code, "{}", held.stderr);
    assert_eq!(held.code, Some(0));
    assert_eq!(held.stdout, free.stdout);
    assert_eq!(held.journal_warnings(), 1, "{}", held.stderr);
    assert!(took >= bound, "took {took:?}");
    assert!(
        took < bound * 2,
        "took {took:?}: more than one wait for {records} records"
    );
    assert_eq!(
        std::fs::read_to_string(env.journal_path()).unwrap(),
        journal_before
    );
}

/// A terminal tick with the journal's lock held past the bound still does
/// everything after its `terminal` record: it exits with the free tick's
/// code and output, appends the terminal index entry, and removes the
/// session, or keeps it under `--no-cleanup`, exactly as the free tick
/// does. A removed session leaves its finished record, so `koto status`
/// still answers for it with `removed: true`, as it does for the free
/// twin; a kept one has no record and answers as a kept session. Neither
/// the `state_entered` nor the `terminal` record is written, and one
/// warning names the journal.
#[test]
fn a_terminal_tick_finishes_its_cleanup_while_the_journal_lock_is_held() {
    if !seams_available() {
        return;
    }
    for no_cleanup in [false, true] {
        let env = Env::new();
        let dir = twin_sessions(&env, "terminal-held");
        for name in ["free", "held"] {
            env.ok_in(&dir, &["next", name, "--with-data", r#"{"go":"yes"}"#]);
        }
        let mut finish_args = vec!["--with-data", r#"{"finish":"yes"}"#];
        if no_cleanup {
            finish_args.push("--no-cleanup");
        }
        let free = env.ok_in(&dir, &[&["next", "free"][..], &finish_args].concat());
        let journal_before = std::fs::read_to_string(env.journal_path()).unwrap();
        assert!(
            journal_before.contains(r#""kind":"terminal""#),
            "{journal_before}"
        );
        let terminals_before = journal_before.matches(r#""kind":"terminal""#).count();

        let holder = hold_journal_lock(&env);
        let (held, took) = run_bounded(&env, &dir, &[&["next", "held"][..], &finish_args].concat());
        drop(holder);

        assert_eq!(held.code, free.code, "{}", held.stderr);
        assert_eq!(held.code, Some(0));
        assert_eq!(held.stdout, free.stdout);
        assert!(held.stdout.contains(r#""state":"done""#), "{}", held.stdout);
        assert_eq!(held.journal_warnings(), 1, "{}", held.stderr);
        assert!(took >= TEST_BOUND, "took {took:?}");
        assert!(took < JOURNAL_BOUND, "took {took:?}");
        let journal_after = std::fs::read_to_string(env.journal_path()).unwrap();
        assert_eq!(journal_after, journal_before);
        assert_eq!(
            journal_after.matches(r#""kind":"terminal""#).count(),
            terminals_before
        );

        // What the tick does after the record happened all the same.
        assert_eq!(
            env.session_dir("held").exists(),
            env.session_dir("free").exists(),
            "no_cleanup={no_cleanup}"
        );
        assert_eq!(env.session_dir("held").exists(), no_cleanup);
        let index = std::fs::read_to_string(env.koto_home().join("_terminal_index.jsonl")).unwrap();
        for name in ["free", "held"] {
            assert!(
                index.contains(&format!(r#""session_id":"{name}""#)),
                "{index}"
            );
        }

        // The finished record is written just before the removal, after
        // the skipped `terminal` record, and only when the session is
        // removed; `koto status` reads it back.
        let record = |name: &str| {
            env.koto_home()
                .join("sessions")
                .join(".finished")
                .join(format!("{name}.json"))
        };
        let status = |name: &str| -> Value {
            let out = env.ok_in(&dir, &["status", name]);
            serde_json::from_str(&out.stdout).unwrap()
        };
        let (free_status, held_status) = (status("free"), status("held"));
        for field in ["current_state", "is_terminal", "result"] {
            assert_eq!(held_status[field], free_status[field], "{field}");
        }
        assert_eq!(held_status["current_state"], "done");
        assert_eq!(held_status["is_terminal"], true);
        if no_cleanup {
            assert!(!record("held").exists());
            assert!(!record("free").exists());
            assert!(held_status.get("removed").is_none(), "{held_status}");
        } else {
            assert!(record("held").is_file());
            assert!(record("free").is_file());
            assert_eq!(held_status["removed"], true, "{held_status}");
            assert_eq!(free_status["removed"], true, "{free_status}");
        }
    }
}

/// A parent that waits on its children, for
/// [`a_terminal_tick_still_notifies_its_parent_while_the_journal_lock_is_held`].
const PARENT: &str = r#"---
name: journal-parent
version: "1.0"
initial_state: wait
states:
  wait:
    accepts:
      done:
        type: enum
        required: true
        values: ["yes"]
    transitions:
      - target: end
        when:
          done: "yes"
  end:
    terminal: true
---

## wait

Wait for the children.

## end

Done.
"#;

/// A child's terminal tick under a held journal lock still notifies its
/// parent: with one child finishing with the lock free and its twin with
/// the lock held, the parent's log gets a `child_completed` for each, and
/// both children are removed. The held tick skips its journal records with
/// one warning and exits as the free one does.
#[test]
fn a_terminal_tick_still_notifies_its_parent_while_the_journal_lock_is_held() {
    if !seams_available() {
        return;
    }
    let env = Env::new();
    let dir = env.work.join("parent-held");
    std::fs::create_dir_all(&dir).unwrap();
    let parent = dir.join("parent.md");
    std::fs::write(&parent, PARENT).unwrap();
    env.ok_in(
        &dir,
        &["init", "boss", "--template", parent.to_str().unwrap()],
    );
    let child = env.template(&dir, CHAINED);
    for name in ["kid-free", "kid-held"] {
        env.ok_in(
            &dir,
            &[
                "init",
                name,
                "--template",
                child.to_str().unwrap(),
                "--parent",
                "boss",
            ],
        );
        env.ok_in(&dir, &["next", name, "--with-data", r#"{"go":"yes"}"#]);
    }
    let finish_args = ["--with-data", r#"{"finish":"yes"}"#];
    let free = env.ok_in(&dir, &[&["next", "kid-free"][..], &finish_args].concat());

    let holder = hold_journal_lock(&env);
    let (held, took) = run_bounded(
        &env,
        &dir,
        &[&["next", "kid-held"][..], &finish_args].concat(),
    );
    drop(holder);

    assert_eq!(held.code, free.code, "{}", held.stderr);
    assert_eq!(held.stdout, free.stdout);
    assert_eq!(held.journal_warnings(), 1, "{}", held.stderr);
    assert!(took < JOURNAL_BOUND, "took {took:?}");
    let notices: Vec<String> = env
        .events("boss")
        .iter()
        .filter(|e| e["type"] == "child_completed")
        .map(|e| e["payload"]["child_name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(notices, vec!["kid-free", "kid-held"]);
    for name in ["kid-free", "kid-held"] {
        assert!(!env.session_dir(name).exists(), "{name} was kept");
    }
}

/// With rotation on, a rotation that is slow under the lock (held just
/// before the rotated file's flush, the slow step) doesn't hold up another
/// `koto next` past the bound: the waiting command gives up on the lock
/// after the bound, exits normally with its usual output and one warning,
/// and writes nothing, while the rotation is still held. The rotating
/// command's slow step isn't cut short by its own bound: released well
/// after it, the rotation finishes, the rotated file keeps the journal byte
/// for byte, and that command's records land whole in the fresh file.
#[test]
fn koto_next_gives_up_at_the_bound_behind_a_slow_rotation_that_still_finishes() {
    use std::time::{Duration, Instant};
    if !seams_available() {
        return;
    }
    let env = Env::new();
    let dir = env.work.join("slow-rotation");
    std::fs::create_dir_all(&dir).unwrap();
    let t = env.template(&dir, CHAINED);
    for name in ["slow", "waiting"] {
        env.ok_in(&dir, &["init", name, "--template", t.to_str().unwrap()]);
        env.ok_in(&dir, &["next", name, "--with-data", r#"{"go":"yes"}"#]);
    }
    // The next write finds the journal at its bound and rotates; the
    // rotating command's own two records stay well under it.
    let journal_before = std::fs::read_to_string(env.journal_path()).unwrap();
    let bound = journal_before.len() as u64;
    env.ok_in(
        &dir,
        &[
            "config",
            "set",
            "run_journal.rotate_bytes",
            &bound.to_string(),
            "--user",
        ],
    );
    let back = ["--with-data", r#"{"finish":"no"}"#];

    let marker = env.tmp.path().join("rotation-held");
    let rotating_started = Instant::now();
    let rotating = bounded_cmd(&env, &dir, &[&["next", "slow"][..], &back].concat())
        .env(ROTATE_HOLD_ENV, &marker)
        .spawn()
        .unwrap();
    while !marker.exists() {
        assert!(
            rotating_started.elapsed() < Duration::from_secs(20),
            "the rotation never reached its slow step"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let held_at = Instant::now();

    let (waiting, took) = run_bounded(&env, &dir, &[&["next", "waiting"][..], &back].concat());
    // The rotation is still in its slow step, holding the lock.
    assert!(marker.exists());
    assert!(rotated_journals(&env).is_empty());
    assert_eq!(waiting.code, Some(0), "{}", waiting.stderr);
    assert_eq!(waiting.journal_warnings(), 1, "{}", waiting.stderr);
    assert!(
        waiting.stderr.contains("held by another writer"),
        "{}",
        waiting.stderr
    );
    assert!(took >= TEST_BOUND, "took {took:?}");
    assert!(took < JOURNAL_BOUND, "took {took:?}");

    // Hold the slow step well past the rotating command's own bound, then
    // let it go.
    while held_at.elapsed() < TEST_BOUND * 3 {
        std::thread::sleep(Duration::from_millis(10));
    }
    std::fs::remove_file(&marker).unwrap();
    let (rotated_out, rotated_took) = finish(rotating, rotating_started, "the rotating koto next");
    assert_eq!(rotated_out.code, Some(0), "{}", rotated_out.stderr);
    assert_eq!(rotated_out.journal_warnings(), 0, "{}", rotated_out.stderr);
    assert!(rotated_took >= TEST_BOUND * 3, "took {rotated_took:?}");
    assert_eq!(waiting.stdout, rotated_out.stdout);

    let rotated = rotated_journals(&env);
    assert_eq!(rotated.len(), 1, "{rotated:?}");
    assert!(rotated[0].ends_with("_run_journal.0000000001.jsonl"));
    assert_eq!(
        std::fs::read_to_string(&rotated[0]).unwrap(),
        journal_before
    );
    let fresh = env.journal();
    assert_eq!(
        kinds(&fresh),
        vec!["state_entered", "response"],
        "{fresh:#?}"
    );
    assert!(fresh.iter().all(|r| r["session"] == "slow"), "{fresh:#?}");
    assert!(
        fresh.iter().all(|r| r["koto.state"] == "start"),
        "{fresh:#?}"
    );
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
    // A decision identifier is random too.
    let decision = regex::Regex::new(r"d-\d{6}-[0-9a-f]{32}").unwrap();
    let time = regex::Regex::new(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?Z").unwrap();
    match value {
        Value::String(s) => {
            let s = s.replace(tmp, "<tmp>");
            let s = uuid.replace_all(&s, "<id>");
            let s = decision.replace_all(&s, "<decision>");
            Value::String(time.replace_all(&s, "<time>").into_owned())
        }
        Value::Array(items) => Value::Array(items.iter().map(|v| masked(v, tmp)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let v = if matches!(
                        k.as_str(),
                        "duration_ms"
                            | "latency_ms"
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
        good.check_records("same")
            .iter()
            .filter(|r| r["kind"] != "check_finding")
            .count(),
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

// ----- Decider and override records -----

/// `koto args` in `cwd` with the decider on (`auto`) and pointed at `stub`.
fn decider_in(env: &Env, cwd: &Path, stub: &DeciderStub, args: &[&str]) -> Out {
    let output = env
        .cmd(cwd)
        .env("KOTO_DECIDER", "auto")
        .env("KOTO_DECIDER_API_KEY", DECIDER_KEY)
        .env("KOTO_DECIDER_ENDPOINT", stub.url())
        .envs(harness_session(args))
        .args(args)
        .output()
        .unwrap();
    Out {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    }
}

/// The decider and override records of `session`.
fn decider_records(env: &Env, session: &str) -> Vec<Value> {
    env.records(session)
        .into_iter()
        .filter(|r| {
            matches!(
                r["kind"].as_str().unwrap(),
                "decider_consulted" | "decider_checked" | "decider_fallback" | "check_overridden"
            )
        })
        .collect()
}

/// A decider answer giving criterion `rule_id` a verdict of `verdict`
/// (`pass` or `fail`), billed on `model` at the given token counts.
fn criterion_answer(rule_id: &str, verdict: &str, model: &str, tokens: (u64, u64)) -> Reply {
    let (pass, fail) = if verdict == "pass" {
        (0.95, 0.02)
    } else {
        (0.02, 0.95)
    };
    Reply::json(&serde_json::json!({
        "model": model,
        "answers": {rule_id: {"type": "choice", "choice": verdict,
            "probabilities": {"pass": pass, "fail": fail, "unclear": 0.03}}},
        "usage": {"input_tokens": tokens.0, "output_tokens": tokens.1}
    }))
}

/// A routing answer of `go`, billed on `model` with `usage`.
fn route_answer(model: &str, usage: Value) -> Reply {
    Reply::json(&serde_json::json!({
        "model": model,
        "answers": {"verdict": {"type": "choice", "choice": "go",
            "probabilities": {"go": 0.97, "stop": 0.02, "unclear": 0.01}}},
        "usage": usage
    }))
}

const MODEL_A: &str = "jev-test-1.2.3";
const MODEL_B: &str = "jev-test-2.0.0";

/// One decider-check criterion, `id`, in `mode`.
fn decider_criterion(id: &str, mode: &str) -> String {
    format!(
        r#"          "{id}":
            rule_ref: "https://example.org/rules/planted-ref-5e1f"
            question: "Does each comment give a reason? planted-question-5e1f"
            pass: "Every comment says why."
            fail: "A comment restates the code."
            escape: "No comment, or can't tell."
            mode: {mode}"#
    )
}

/// `review` holds the decider check `comments` over `slice.txt`, with
/// criteria `a` (veto), `s` (shadow) and `b` (veto), and moves to `done`
/// once nothing blocks. With `routing`, the session starts in `triage`,
/// whose evidence the decider answers, and `go` leads to `review`.
fn decider_template(routing: bool, criteria: &[(&str, &str)]) -> String {
    let criteria: Vec<String> = criteria
        .iter()
        .map(|(id, mode)| decider_criterion(id, mode))
        .collect();
    let triage = if routing {
        r#"  triage:
    accepts:
      verdict:
        type: enum
        values: [go, stop]
        required: true
        description: Go?
        decider:
          answers:
            go: {description: "Yes.", mode: auto}
            stop: {description: "No.", mode: never}
          escape: {value: unclear, description: "Unknown."}
          inputs:
            - {var: PLAN_DOC, label: plan_path}
    transitions:
      - target: review
        when: {verdict: go}
      - target: done
        when: {verdict: stop}
"#
    } else {
        ""
    };
    format!(
        r#"---
name: journal-decider
version: "1.0"
initial_state: {initial}
variables:
  PLAN_DOC:
    description: plan
    default: docs/plan.md
states:
{triage}  review:
    gates:
      comments:
        type: decider-check
        command: "cat slice.txt"
        label: comments
        criteria:
{criteria}
    transitions:
      - target: done
  done:
    terminal: true
---

## triage

Triage. planted-directive-5e1f

## review

Review the comments. planted-directive-5e1f

## done

Done.
"#,
        initial = if routing { "triage" } else { "review" },
        criteria = criteria.join("\n"),
    )
}

/// The three criteria of the decider-check scenario: a veto criterion that
/// fails, a shadow criterion, and a veto criterion left unanswered.
const SCENARIO_CRITERIA: &[(&str, &str)] = &[("a", "veto"), ("s", "shadow"), ("b", "veto")];

/// Queue the stub's answers to [`SCENARIO_CRITERIA`], in the order koto
/// asks: `a` fails on [`MODEL_A`], `s` fails on [`MODEL_B`] (a shadow
/// verdict never blocks), and `b` gets a status koto doesn't retry, so it
/// stays unanswered.
fn queue_scenario_answers(stub: &DeciderStub) {
    stub.push(criterion_answer("a", "fail", MODEL_A, (100, 3)));
    stub.push(criterion_answer("s", "fail", MODEL_B, (40, 2)));
    stub.push(Reply::status(401));
}

/// A working directory for a decider-check session: the template and the
/// slice the check sends.
fn decider_dir(env: &Env, label: &str, template: &str) -> PathBuf {
    let dir = env.work.join(label);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("slice.txt"),
        "planted-slice-5e1f let total = a + b; // add a and b\n",
    )
    .unwrap();
    env.template(&dir, template);
    dir
}

/// Init `name` on the decider-check scenario and run its first `koto next`,
/// which grades the three criteria and blocks on `a`.
fn decider_scenario(env: &Env, name: &str, stub: &DeciderStub) -> PathBuf {
    let dir = decider_dir(env, name, &decider_template(false, SCENARIO_CRITERIA));
    env.ok_in(&dir, &["init", name, "--template", "template.md"]);
    queue_scenario_answers(stub);
    let out = decider_in(env, &dir, stub, &["next", name, "--no-cleanup"]);
    assert_eq!(out.journal_warnings(), 0, "{}", out.stderr);
    let json: Value = serde_json::from_str(out.stdout.trim()).unwrap();
    assert_eq!(json["action"], "gate_blocked", "{json}");
    assert_eq!(stub.request_count(), 3);
    dir
}

/// The journal's `check_evaluated` an override moved past, and that
/// evaluation's rule records, found from journal lines alone: the same
/// session, state, check and visit, and the highest `koto.seq` below the
/// override's.
fn overridden_evaluation(lines: &[Value], overridden: &Value) -> Option<(Value, Vec<Value>)> {
    let same = |r: &Value| {
        r["koto.session.id"] == overridden["koto.session.id"]
            && r["koto.state"] == overridden["koto.state"]
            && r["koto.check"] == overridden["koto.check"]
            && r["koto.visit.seq"] == overridden["koto.visit.seq"]
    };
    let below = overridden["koto.seq"].as_u64().unwrap();
    let evaluation = lines
        .iter()
        .filter(|r| r["kind"] == "check_evaluated" && same(r))
        .filter(|r| r["koto.seq"].as_u64().unwrap() < below)
        .max_by_key(|r| r["koto.seq"].as_u64().unwrap())?
        .clone();
    let rules = lines
        .iter()
        .filter(|r| r["kind"] == "check_rule" && same(r))
        .filter(|r| r["koto.seq"] == evaluation["koto.seq"])
        .cloned()
        .collect();
    Some((evaluation, rules))
}

/// Every journal line, parsed, read without the reference checks: what a
/// reader outside koto starts from.
fn journal_lines(env: &Env) -> Vec<Value> {
    std::fs::read_to_string(env.journal_path())
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// Take a session's decider lock, as another `koto next` would.
fn hold_decider_lock(env: &Env, name: &str) -> std::fs::File {
    use std::os::unix::io::AsRawFd;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(env.session_dir(name).join("decider.lock"))
        .unwrap();
    // SAFETY: the descriptor belongs to `f`, which outlives the call.
    assert_eq!(
        unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    f
}

/// With a stub decider, one decider check has a veto criterion that fails,
/// a shadow criterion and a veto criterion left unanswered. Three
/// `decider_checked` records come before the gate's `check_evaluated`,
/// which is `failed` with one unanswered criterion and one `check_rule`.
///
/// koto can only leave a criterion `busy` when it can't take the session's
/// decider lock, and then it asks about none of the criteria it doesn't
/// reuse, so `busy` can't sit beside a fresh verdict in one evaluation. The
/// unanswered criterion of the first evaluation fails at the provider; the
/// next evaluation in the same visit, with the lock held elsewhere, reuses
/// the two verdicts (writing no record for them) and leaves the third
/// `busy`.
#[test]
fn a_decider_check_writes_one_decider_checked_per_criterion_before_its_evaluation() {
    let env = Env::new();
    let stub = DeciderStub::start();
    decider_scenario(&env, "dc", &stub);

    let log = env.events("dc");
    let arrival = log
        .iter()
        .find(|e| e["type"] == "transitioned" && e["payload"]["to"] == "review")
        .unwrap()["seq"]
        .clone();
    let checked_events: Vec<&Value> = log
        .iter()
        .filter(|e| e["type"] == "decider_checked")
        .collect();
    let evaluated_event = log.iter().find(|e| e["type"] == "gate_evaluated").unwrap();

    let records: Vec<Value> = env
        .records("dc")
        .into_iter()
        .filter(|r| r["kind"] != "session_started" && r["kind"] != "state_entered")
        .collect();
    // The blocked tick's printed directive adds a `response` last.
    assert_eq!(
        kinds(&records),
        vec![
            "decider_checked",
            "decider_checked",
            "decider_checked",
            "decider_fallback",
            "check_evaluated",
            "check_rule",
            "check_finding",
            "check_finding",
            "response"
        ]
    );
    assert_eq!(records[8]["koto.state"], "review");
    for (record, event) in records[..3].iter().zip(&checked_events) {
        let p = &event["payload"];
        assert_eq!(record["koto.seq"], event["seq"], "{record}");
        assert!(record["koto.seq"].as_u64() < evaluated_event["seq"].as_u64());
        assert_eq!(record["koto.visit.seq"], arrival, "{record}");
        assert_eq!(record["koto.state"], "review");
        assert_eq!(record["koto.check"], "comments");
        assert_eq!(record["koto.rule.id"], p["rule_id"]);
        assert_eq!(record["koto.decider.provider"], p["provider"]);
        assert_eq!(record["koto.decider.model"], p["model"]);
        assert_eq!(record["koto.decider.mode"], p["mode"]);
        assert_eq!(record["koto.decider.outcome"], p["outcome"]);
        assert_eq!(record["koto.decider.blocked"], p["blocked"]);
        assert_eq!(record["koto.decider.attempts"], p["attempts"]);
        assert_eq!(record["koto.decider.latency_ms"], p["latency_ms"]);
        assert_eq!(
            record["koto.decider.unread_usage_attempts"],
            p["unread_usage_attempts"]
        );
        for (field, key) in [
            ("koto.decider.reason", "reason"),
            ("koto.decider.error_class", "error_class"),
            ("koto.decider.input_tokens", "input_tokens"),
            ("koto.decider.output_tokens", "output_tokens"),
        ] {
            assert_eq!(record.get(field), p.get(key), "{field}: {record}");
        }
    }
    let modes: Vec<(&str, &str, &str, bool)> = records[..3]
        .iter()
        .map(|r| {
            (
                r["koto.rule.id"].as_str().unwrap(),
                r["koto.decider.mode"].as_str().unwrap(),
                r["koto.decider.outcome"].as_str().unwrap(),
                r["koto.decider.blocked"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        modes,
        vec![
            ("a", "veto", "fail", true),
            ("s", "shadow", "fail", false),
            ("b", "veto", "unanswered", false),
        ]
    );
    assert_eq!(records[0]["koto.decider.model"], MODEL_A);
    assert_eq!(records[0]["koto.decider.input_tokens"], 100);
    assert_eq!(records[0]["koto.decider.output_tokens"], 3);
    assert_eq!(records[1]["koto.decider.model"], MODEL_B);
    assert_eq!(records[2]["koto.decider.reason"], "provider_error");
    assert!(records[2].get("koto.decider.input_tokens").is_none());

    // b got no verdict, so it was handed to the agent: its fallback comes
    // after the decider_checked records and before the evaluation.
    let fb = &records[3];
    assert_eq!(fb["koto.rule.id"], "b");
    assert_eq!(fb["koto.decider.mode"], "veto");
    assert_eq!(fb["koto.decider.reason"], "provider_error");
    assert!(fb["koto.decider.input_bytes"].as_u64().is_some(), "{fb}");
    assert!(fb["koto.seq"].as_u64() < evaluated_event["seq"].as_u64());
    assert_eq!(fb["koto.visit.seq"], arrival);
    let e = &records[4];
    assert_eq!(e["koto.seq"], evaluated_event["seq"]);
    assert_eq!(e["koto.check.outcome"], "failed");
    assert_eq!(e["koto.check.unanswered_count"], 1);
    assert_eq!(e["koto.check.rule_count"], 1);
    assert_eq!(e["koto.decider.awaiting_count"], 1);
    assert_eq!(e["koto.decider.agent_answered_count"], 0);
    assert_eq!(records[5]["koto.rule.id"], "a");
    assert_eq!(records[5]["koto.rule.visit_count"], 1);

    // The same visit again, with another command holding the decider lock:
    // `a` and `s` are reused and write nothing, and `b` is `busy`.
    std::fs::write(
        env.koto_home().join("config.toml"),
        "[decider]\ntimeout_ms = 50\n",
    )
    .unwrap();
    let lock = hold_decider_lock(&env, "dc");
    let dir = env.work.join("dc");
    let out = decider_in(&env, &dir, &stub, &["next", "dc", "--no-cleanup"]);
    drop(lock);
    assert_eq!(out.journal_warnings(), 0, "{}", out.stderr);
    assert_eq!(stub.request_count(), 3, "nothing more was asked");
    let after: Vec<Value> = env.records("dc").into_iter().skip(2 + 9).collect();
    assert_eq!(
        kinds(&after),
        vec![
            "decider_checked",
            "decider_fallback",
            "check_evaluated",
            "check_rule",
            "check_finding",
            "check_finding",
            "response"
        ],
        "reused verdicts write no decider_checked"
    );
    assert_eq!(after[0]["koto.rule.id"], "b");
    assert_eq!(after[0]["koto.decider.outcome"], "unanswered");
    assert_eq!(after[0]["koto.decider.reason"], "busy");
    assert_eq!(after[0]["koto.decider.attempts"], 0);
    assert_eq!(after[0]["koto.visit.seq"], arrival);
    assert_eq!(after[1]["koto.decider.reason"], "busy");
    assert_eq!(after[2]["koto.check.outcome"], "failed");
    assert_eq!(after[2]["koto.check.unanswered_count"], 1);
    assert_eq!(after[3]["koto.rule.id"], "a");
    assert_eq!(after[3]["koto.rule.visit_count"], 2);
    let logged = env
        .events("dc")
        .iter()
        .filter(|e| e["type"] == "decider_checked")
        .count();
    assert_eq!(logged, 4, "the log agrees: one more decider_checked");

    // Nothing the decider events carry beyond the record's fields.
    let raw = std::fs::read_to_string(env.journal_path()).unwrap();
    for event in &checked_events {
        let p = &event["payload"];
        for hash in ["declaration_hash", "input_sha256"] {
            let value = p[hash].as_str().unwrap();
            assert!(!raw.contains(value), "{hash} in the journal");
        }
    }
    for absent in ["probabilit", "threshold", "endpoint", "rule_ref", "0.95"] {
        assert!(!raw.contains(absent), "{absent} in {raw}");
    }
}

/// A decider check evaluated again in the same visit, on the same slice
/// and with the lock free, reuses every verdict: koto asks the stub
/// nothing and appends no `decider_checked`, so the journal gets the gate's
/// new evaluation and its rule record, and no `decider_checked`.
#[test]
fn reused_verdicts_write_no_decider_checked() {
    let env = Env::new();
    let stub = DeciderStub::start();
    let dir = decider_dir(
        &env,
        "reuse",
        &decider_template(false, &[("a", "veto"), ("s", "shadow")]),
    );
    env.ok_in(&dir, &["init", "reuse", "--template", "template.md"]);
    stub.push(criterion_answer("a", "fail", MODEL_A, (100, 3)));
    stub.push(criterion_answer("s", "fail", MODEL_B, (40, 2)));
    let next = || {
        let out = decider_in(&env, &dir, &stub, &["next", "reuse", "--no-cleanup"]);
        assert_eq!(out.journal_warnings(), 0, "{}", out.stderr);
        let json: Value = serde_json::from_str(out.stdout.trim()).unwrap();
        assert_eq!(json["action"], "gate_blocked", "{json}");
    };
    next();
    assert_eq!(stub.request_count(), 2);
    let first = env.records("reuse").len();

    next();
    assert_eq!(stub.request_count(), 2, "every verdict was reused");
    let logged = env
        .events("reuse")
        .iter()
        .filter(|e| e["type"] == "decider_checked")
        .count();
    assert_eq!(logged, 2, "koto appended no decider_checked");
    let after: Vec<Value> = env.records("reuse").into_iter().skip(first).collect();
    assert_eq!(
        kinds(&after),
        vec!["check_evaluated", "check_rule", "check_finding", "response"],
        "the blocked tick's printed directive adds a response"
    );
    assert_eq!(after[0]["koto.check.outcome"], "failed");
    assert_eq!(after[1]["koto.rule.id"], "a");
    assert_eq!(after[1]["koto.rule.visit_count"], 2);
}

/// From the decider-check scenario's journal lines alone, preceded by a
/// routing consultation, the tokens per model match the stub's answers and
/// the session log, summed over both decider kinds.
#[test]
fn the_journal_alone_sums_decider_tokens_per_model() {
    let env = Env::new();
    let stub = DeciderStub::start();
    let dir = decider_dir(&env, "spend", &decider_template(true, SCENARIO_CRITERIA));
    env.ok_in(&dir, &["init", "spend", "--template", "template.md"]);
    stub.push(route_answer(
        MODEL_B,
        serde_json::json!({"input_tokens": 250, "output_tokens": 7}),
    ));
    queue_scenario_answers(&stub);
    let out = decider_in(&env, &dir, &stub, &["next", "spend", "--no-cleanup"]);
    assert_eq!(out.journal_warnings(), 0, "{}", out.stderr);
    assert_eq!(stub.request_count(), 4);

    type Spend = BTreeMap<String, (u64, u64)>;
    let sum = |records: &mut dyn Iterator<Item = (String, u64, u64)>| -> Spend {
        let mut spend = Spend::new();
        for (model, input, output) in records {
            let entry = spend.entry(model).or_default();
            entry.0 += input;
            entry.1 += output;
        }
        spend
    };
    let from_journal = sum(&mut journal_lines(&env)
        .into_iter()
        .filter(|r| r["kind"] == "decider_checked" || r["kind"] == "decider_consulted")
        .map(|r| {
            (
                r["koto.decider.model"].as_str().unwrap().to_string(),
                r["koto.decider.input_tokens"].as_u64().unwrap_or(0),
                r["koto.decider.output_tokens"].as_u64().unwrap_or(0),
            )
        }));
    let from_log = sum(&mut env
        .events("spend")
        .into_iter()
        .filter(|e| e["type"] == "decider_checked" || e["type"] == "decider_consulted")
        .map(|e| {
            let p = &e["payload"];
            (
                p["model"].as_str().unwrap().to_string(),
                p["input_tokens"].as_u64().unwrap_or(0),
                p["output_tokens"].as_u64().unwrap_or(0),
            )
        }));
    let expected: Spend = [
        (MODEL_A.to_string(), (100, 3)),
        (MODEL_B.to_string(), (250 + 40, 7 + 2)),
        ("unknown".to_string(), (0, 0)),
    ]
    .into_iter()
    .collect();
    assert_eq!(from_journal, expected);
    assert_eq!(from_log, expected);
}

/// The routing decider's consultation writes `decider_consulted` with the
/// event's provider, model, outcome, tokens and timings, under koto's
/// visit; without usage in the answer, the token fields are left out.
#[test]
fn a_consultation_writes_decider_consulted_with_the_events_values() {
    let env = Env::new();
    for (name, usage) in [
        (
            "routed",
            serde_json::json!({"input_tokens": 250, "output_tokens": 7}),
        ),
        ("unbilled", serde_json::json!({})),
    ] {
        let stub = DeciderStub::start();
        let dir = decider_dir(&env, name, &decider_template(true, &[("a", "veto")]));
        env.ok_in(&dir, &["init", name, "--template", "template.md"]);
        stub.push(route_answer(MODEL_B, usage.clone()));
        stub.push(criterion_answer("a", "pass", MODEL_A, (100, 3)));
        let out = decider_in(&env, &dir, &stub, &["next", name, "--no-cleanup"]);
        assert_eq!(out.journal_warnings(), 0, "{}", out.stderr);

        let log = env.events(name);
        let event = log
            .iter()
            .find(|e| e["type"] == "decider_consulted")
            .expect("the decider was consulted");
        let arrival = log
            .iter()
            .find(|e| e["type"] == "transitioned" && e["payload"]["to"] == "triage")
            .unwrap()["seq"]
            .clone();
        let consulted: Vec<Value> = decider_records(&env, name)
            .into_iter()
            .filter(|r| r["kind"] == "decider_consulted")
            .collect();
        assert_eq!(consulted.len(), 1, "{consulted:?}");
        let r = &consulted[0];
        let p = &event["payload"];
        assert_eq!(r["koto.seq"], event["seq"]);
        assert_eq!(r["koto.visit.seq"], arrival);
        assert_eq!(r["koto.state"], "triage");
        assert_eq!(r["koto.decider.provider"], p["provider"]);
        assert_eq!(r["koto.decider.model"], MODEL_B);
        assert_eq!(r["koto.decider.outcome"], p["outcome"]);
        assert_eq!(r["koto.decider.outcome"], "applied");
        assert_eq!(r["koto.decider.latency_ms"], p["latency_ms"]);
        assert_eq!(r["koto.decider.directive_bytes"], p["directive_bytes"]);
        assert_eq!(
            r["koto.decider.unread_usage_attempts"],
            p["unread_usage_attempts"]
        );
        assert!(r.get("koto.decider.error_class").is_none(), "{r}");
        for (field, key) in [
            ("koto.decider.input_tokens", "input_tokens"),
            ("koto.decider.output_tokens", "output_tokens"),
        ] {
            assert_eq!(r.get(field), p.get(key), "{name} {field}: {r}");
        }
        if name == "routed" {
            assert_eq!(r["koto.decider.input_tokens"], 250);
            assert_eq!(r["koto.decider.output_tokens"], 7);
        } else {
            assert!(r.get("koto.decider.input_tokens").is_none(), "{r}");
        }
        let raw = std::fs::read_to_string(env.journal_path()).unwrap();
        assert!(!raw.contains(p["input_sha256"].as_str().unwrap()));
        assert!(!raw.contains("plan_path"), "{raw}");
    }
}

/// A criterion id outside the name shape is written as its digest, on its
/// `decider_checked` and on the rule record of the evaluation it failed.
#[test]
fn an_odd_criterion_id_is_written_as_its_digest() {
    let env = Env::new();
    let stub = DeciderStub::start();
    let odd = "no reason [x]";
    let dir = decider_dir(&env, "oddc", &decider_template(false, &[(odd, "veto")]));
    env.ok_in(&dir, &["init", "oddc", "--template", "template.md"]);
    stub.push(criterion_answer(odd, "fail", MODEL_A, (100, 3)));
    decider_in(&env, &dir, &stub, &["next", "oddc", "--no-cleanup"]);
    let checked: Vec<Value> = decider_records(&env, "oddc");
    assert_eq!(kinds(&checked), vec!["decider_checked"]);
    assert_eq!(checked[0]["koto.rule.id"], digest(odd).as_str());
    assert_eq!(checked[0]["koto.decider.outcome"], "fail");
    let rules: Vec<Value> = env
        .check_records("oddc")
        .into_iter()
        .filter(|r| r["kind"] == "check_rule")
        .collect();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["koto.rule.id"], digest(odd).as_str());
    let raw = std::fs::read_to_string(env.journal_path()).unwrap();
    assert!(!raw.contains("no reason"), "{raw}");
}

/// `koto overrides record` on the gate that failed four times in one visit
/// writes a `check_overridden` whose highest earlier same-check
/// `check_evaluated` in its visit is the fourth attempt, with the rule it
/// failed.
#[test]
fn an_override_after_a_loop_joins_to_the_fourth_attempt() {
    let env = Env::new();
    let dir = run_retry(&env, "loop6", &[1, 1, 1, 1], &["E501"], false);
    env.ok_in(
        &dir,
        &[
            "overrides",
            "record",
            "loop6",
            "--gate",
            "lint",
            "--rationale",
            "planted-rationale-5e1f",
        ],
    );
    let overridden = decider_records(&env, "loop6");
    assert_eq!(kinds(&overridden), vec!["check_overridden"]);
    let o = &overridden[0];
    let keys: BTreeSet<&str> = o.as_object().unwrap().keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = ENVELOPE
        .iter()
        .copied()
        .chain(["koto.seq", "koto.visit.seq", "koto.state", "koto.check"])
        .collect();
    assert_eq!(keys, expected, "only the envelope, the two seqs and names");
    let event = env
        .events("loop6")
        .into_iter()
        .find(|e| e["type"] == "gate_override_recorded")
        .unwrap();
    assert_eq!(o["koto.seq"], event["seq"]);
    assert_eq!(o["koto.state"], "work");
    assert_eq!(o["koto.check"], "lint");

    let lines = journal_lines(&env);
    let (evaluation, rules) = overridden_evaluation(&lines, o).expect("an evaluation");
    assert_eq!(evaluation["koto.visit_attempt"], 4);
    assert_eq!(evaluation["koto.check.outcome"], "failed");
    assert_eq!(evaluation["koto.visit.seq"], o["koto.visit.seq"]);
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["koto.rule.id"], "E501");
    assert_eq!(rules[0]["koto.rule.visit_count"], 4);
}

/// An overridden gate isn't evaluated again: the override writes one
/// `check_overridden` and no evaluation record, and its rationale stays out
/// of the journal.
#[test]
fn an_overridden_gate_writes_only_check_overridden() {
    let env = Env::new();
    let dir = env.check_dir("over", &[1, 1], &[finding("E501")]);
    let t = env.template(&dir, RETRY);
    env.ok_in(&dir, &["init", "over", "--template", t.to_str().unwrap()]);
    env.koto_in(&dir, &["next", "over", "--no-cleanup"]);
    assert_eq!(
        env.check_records("over").len(),
        3,
        "one evaluation, one rule, one finding"
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
    // The override itself is relayed as `check_overridden`; no evaluation
    // is.
    let (overridden, evaluations): (Vec<Value>, Vec<Value>) = env
        .check_records("over")
        .into_iter()
        .partition(|r| r["kind"] == "check_overridden");
    assert_eq!(overridden.len(), 1, "{overridden:?}");
    assert_eq!(
        evaluations.len(),
        3,
        "nothing more was evaluated: the first evaluation's record, rule and finding"
    );
    let runs = std::fs::read_to_string(dir.join("runs.log")).unwrap();
    assert_eq!(runs.lines().count(), 1, "the check didn't run again");
}

/// An override recorded before the gate was ever evaluated writes its
/// record, and the join finds no evaluation.
#[test]
fn an_override_with_no_earlier_evaluation_joins_to_nothing() {
    let env = Env::new();
    let dir = env.check_dir("early", &[1], &[finding("E501")]);
    let t = env.template(&dir, RETRY);
    env.ok_in(&dir, &["init", "early", "--template", t.to_str().unwrap()]);
    env.ok_in(
        &dir,
        &[
            "overrides",
            "record",
            "early",
            "--gate",
            "lint",
            "--rationale",
            "planted-rationale-5e1f",
        ],
    );
    let overridden = decider_records(&env, "early");
    assert_eq!(kinds(&overridden), vec!["check_overridden"]);
    assert_eq!(env.check_records("early").len(), 1, "the override only");
    assert!(overridden_evaluation(&journal_lines(&env), &overridden[0]).is_none());
}

/// Overriding the decider check of the decider-check scenario writes one
/// `check_overridden`, not one per criterion, and the evaluation it joins to
/// names the failed criterion through its rule record.
#[test]
fn a_decider_check_override_is_one_record_and_its_join_names_the_failed_criterion() {
    let env = Env::new();
    let stub = DeciderStub::start();
    let dir = decider_scenario(&env, "dco", &stub);
    let out = decider_in(
        &env,
        &dir,
        &stub,
        &[
            "overrides",
            "record",
            "dco",
            "--gate",
            "comments",
            "--rationale",
            "planted-rationale-5e1f",
        ],
    );
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    let overridden: Vec<Value> = decider_records(&env, "dco")
        .into_iter()
        .filter(|r| r["kind"] == "check_overridden")
        .collect();
    assert_eq!(overridden.len(), 1, "{overridden:?}");
    assert_eq!(overridden[0]["koto.check"], "comments");
    let (evaluation, rules) =
        overridden_evaluation(&journal_lines(&env), &overridden[0]).expect("an evaluation");
    assert_eq!(evaluation["koto.check.unanswered_count"], 1);
    let ids: Vec<&str> = rules
        .iter()
        .map(|r| r["koto.rule.id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec!["a"],
        "the failed criterion, not the shadow or unanswered one"
    );
}

/// `check` runs `lint` and can move to `other` and back.
const OVERRIDE_VISITS: &str = r#"---
name: journal-override-visits
version: "1.0"
initial_state: check
states:
  check:
    accepts:
      verdict:
        type: enum
        required: true
        values: [leave, finish]
    gates:
      lint:
        type: command
        command: "./check.sh"
        timeout: 600
    transitions:
      - target: other
        when:
          verdict: leave
      - target: done
        when:
          verdict: finish
  other:
    accepts:
      verdict:
        type: enum
        required: true
        values: [back]
    transitions:
      - target: check
        when:
          verdict: back
  done:
    terminal: true
---

## check

Check.

## other

Elsewhere.

## done

Done.
"#;

/// An override joins only to an evaluation in its own visit, as koto ties
/// it. Recorded in the visit where the gate failed, it joins to that
/// evaluation. Recorded after the session left and came back, with no
/// evaluation in the new visit, it joins to nothing, and koto's own record
/// of the override agrees: it found no output to move past.
#[test]
fn an_override_joins_only_to_an_evaluation_in_its_own_visit() {
    let env = Env::new();
    let dir = env.check_dir("later", &[1], &[finding("E501")]);
    let t = env.template(&dir, OVERRIDE_VISITS);
    env.ok_in(&dir, &["init", "later", "--template", t.to_str().unwrap()]);
    env.koto_in(&dir, &["next", "later", "--no-cleanup"]);
    let record = |env: &Env| {
        env.ok_in(
            &dir,
            &[
                "overrides",
                "record",
                "later",
                "--gate",
                "lint",
                "--rationale",
                "planted-rationale-5e1f",
            ],
        )
    };
    record(&env);
    env.ok_in(&dir, &["next", "later", "--to", "other"]);
    env.ok_in(&dir, &["next", "later", "--to", "check"]);
    record(&env);
    let runs = std::fs::read_to_string(dir.join("runs.log")).unwrap();
    assert_eq!(
        runs.lines().count(),
        1,
        "the check ran in the first visit only"
    );

    let overridden = decider_records(&env, "later");
    assert_eq!(
        kinds(&overridden),
        vec!["check_overridden", "check_overridden"]
    );
    let logged: Vec<Value> = env
        .events("later")
        .into_iter()
        .filter(|e| e["type"] == "gate_override_recorded")
        .collect();
    let lines = journal_lines(&env);

    // Same visit: the failed evaluation, and koto saw its output.
    let same = &overridden[0];
    let (evaluation, rules) = overridden_evaluation(&lines, same).expect("an evaluation");
    assert_eq!(evaluation["koto.visit.seq"], same["koto.visit.seq"]);
    assert_eq!(evaluation["koto.check.outcome"], "failed");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["koto.rule.id"], "E501");
    assert!(
        logged[0]["payload"]["actual_output"].is_object(),
        "{}",
        logged[0]
    );

    // A later visit with no evaluation: nothing, and koto saw no output.
    let later = &overridden[1];
    assert!(
        later["koto.visit.seq"].as_u64() > same["koto.visit.seq"].as_u64(),
        "{later}"
    );
    assert!(overridden_evaluation(&lines, later).is_none());
    assert!(
        logged[1]["payload"]["actual_output"].is_null(),
        "{}",
        logged[1]
    );
}

/// What the decider and override scenario leaves behind in one home.
struct DeciderOutcome {
    /// Each command's exit code, masked standard output and journal
    /// warning count.
    steps: Vec<Step>,
    /// The masked session log.
    log: Vec<Value>,
    /// The decider ledger's bytes with the values that vary between runs
    /// (times, ids, latencies, the temporary directory) masked, or `None`
    /// when the home holds no ledger.
    ledger: Option<String>,
}

/// `text` with every time, id, latency and the temporary directory `tmp`
/// replaced by a marker, and nothing else changed.
fn masked_text(text: &str, tmp: &str) -> String {
    let uuid =
        regex::Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}").unwrap();
    let time = regex::Regex::new(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?Z").unwrap();
    let latency = regex::Regex::new(r#""latency_ms":\d+"#).unwrap();
    let text = text.replace(tmp, "<tmp>");
    let text = uuid.replace_all(&text, "<id>");
    let text = time.replace_all(&text, "<time>");
    latency
        .replace_all(&text, r#""latency_ms":"<masked>""#)
        .into_owned()
}

/// The decider and override scenario's commands in `env`. The routing
/// decider answers `go`, the decider check blocks on `a`, an override moves
/// past it, and the session finishes.
fn decider_outcome(env: &Env) -> DeciderOutcome {
    let stub = DeciderStub::start();
    let dir = decider_dir(env, "same", &decider_template(true, SCENARIO_CRITERIA));
    stub.push(route_answer(
        MODEL_B,
        serde_json::json!({"input_tokens": 250, "output_tokens": 7}),
    ));
    queue_scenario_answers(&stub);
    let tmp = env.tmp.path().to_string_lossy().to_string();
    let steps: [&[&str]; 4] = [
        &["init", "same", "--template", "template.md"],
        &["next", "same", "--no-cleanup"],
        &[
            "overrides",
            "record",
            "same",
            "--gate",
            "comments",
            "--rationale",
            "planted-rationale-5e1f",
        ],
        &["next", "same", "--no-cleanup"],
    ];
    let mut outs = Vec::new();
    for args in steps {
        let out = decider_in(env, &dir, &stub, args);
        let json: Value = serde_json::from_str(out.stdout.trim()).unwrap_or(Value::Null);
        outs.push((out.code, masked(&json, &tmp), out.journal_warnings()));
    }
    assert_eq!(stub.request_count(), 4, "one routing call, three criteria");
    let log = std::fs::read_to_string(env.state_path("same"))
        .unwrap()
        .lines()
        .map(|l| masked(&serde_json::from_str(l).unwrap(), &tmp))
        .collect();
    let ledger = std::fs::read_to_string(env.koto_home().join("_decider_ledger.jsonl"))
        .ok()
        .map(|text| masked_text(&text, &tmp));
    DeciderOutcome {
        steps: outs,
        log,
        ledger,
    }
}

/// With the journal unwritable (a read-only koto home, a read-only journal
/// file, or a symlink at its path), the decider and override scenario gives
/// the same exit codes, standard output and session log as with a writable
/// journal, and each command prints exactly one run-journal warning. Where
/// the ledger can still be written (everywhere but the read-only home,
/// which holds it), its bytes match too once times, ids and latencies are
/// masked.
#[test]
fn an_unwritable_journal_changes_nothing_for_decider_and_override_commands() {
    if running_as_root() {
        eprintln!("skipped: permissions don't bind this user");
        return;
    }
    let good = Env::new();
    let expected = decider_outcome(&good);
    assert!(
        expected.steps.iter().all(|(_, _, w)| *w == 0),
        "{:?}",
        expected.steps
    );
    assert!(
        expected.steps.iter().all(|(_, json, _)| json.is_object()),
        "{:?}",
        expected.steps
    );
    let ledger = expected.ledger.clone().expect("a decider ledger");
    for kind in ["consulted", "checked", "check_overridden"] {
        assert!(
            ledger.contains(&format!(r#""kind":"{kind}""#)),
            "{kind}: {ledger}"
        );
    }
    let written = kinds(&decider_records(&good, "same"))
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(
        written,
        vec![
            "decider_consulted",
            "decider_checked",
            "decider_checked",
            "decider_checked",
            "decider_fallback",
            "check_overridden"
        ]
    );
    // The blocked tick's printed directive writes a `response` too, so the
    // warning count below covers that record as well.
    let responses = good
        .records("same")
        .into_iter()
        .filter(|r| r["kind"] == "response")
        .count();
    assert_eq!(responses, 1, "the blocked tick's response");
    assert_eq!(
        good.records("same").last().unwrap()["kind"],
        "terminal",
        "the override let the session finish"
    );

    let read_only = Env::new();
    std::fs::create_dir_all(read_only.koto_home().join("sessions")).unwrap();
    std::fs::set_permissions(
        read_only.koto_home(),
        std::fs::Permissions::from_mode(0o500),
    )
    .unwrap();
    let got = decider_outcome(&read_only);
    std::fs::set_permissions(
        read_only.koto_home(),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    assert!(!read_only.journal_path().exists());
    assert_eq!(got.ledger, None, "the read-only home holds the ledger too");

    let read_only_file = Env::new();
    std::fs::create_dir_all(read_only_file.koto_home()).unwrap();
    std::fs::write(read_only_file.journal_path(), "").unwrap();
    std::fs::set_permissions(
        read_only_file.journal_path(),
        std::fs::Permissions::from_mode(0o400),
    )
    .unwrap();
    let got_file = decider_outcome(&read_only_file);
    assert_eq!(
        std::fs::read_to_string(read_only_file.journal_path()).unwrap(),
        ""
    );

    let linked = Env::new();
    std::fs::create_dir_all(linked.koto_home()).unwrap();
    let target = linked.tmp.path().join("not-the-journal.txt");
    std::fs::write(&target, "untouched\n").unwrap();
    std::os::unix::fs::symlink(&target, linked.journal_path()).unwrap();
    let got_linked = decider_outcome(&linked);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched\n");

    for (label, got, compare_ledger) in [
        ("read-only home", got, false),
        ("read-only file", got_file, true),
        ("symlink", got_linked, true),
    ] {
        assert_eq!(got.steps.len(), expected.steps.len());
        for (i, ((code, stdout, warnings), (e_code, e_stdout, _))) in
            got.steps.iter().zip(&expected.steps).enumerate()
        {
            assert_eq!(code, e_code, "{label} step {i}");
            assert_eq!(stdout, e_stdout, "{label} step {i}");
            assert_eq!(*warnings, 1, "{label} step {i}");
        }
        assert_eq!(got.log, expected.log, "{label}");
        if compare_ledger {
            assert_eq!(got.ledger, expected.ledger, "{label}");
        }
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
    // The scenario writes check, response and context-read records, so the
    // comparison below covers each of them: `init`, the blocked tick, the
    // move to `work` and the terminal tick write; the context commands
    // don't.
    let records = good.records("same");
    for kind in ["check_evaluated", "response", "context_reads", "terminal"] {
        assert!(kinds(&records).contains(&kind), "no {kind}: {records:?}");
    }
    assert_eq!(
        expected.iter().map(|(_, wrote)| *wrote).collect::<Vec<_>>(),
        vec![true, true, false, false, false, true, true]
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

/// A harness session identifier for the commands that pass a gate by
/// choice (`koto overrides record`, `koto next --to`, `koto decider
/// answer`), which koto refuses when no decider can be established.
fn harness_session(args: &[&str]) -> Vec<(&'static str, &'static str)> {
    let passes_gate = args.contains(&"--to")
        || args
            .windows(2)
            .any(|w| w == ["overrides", "record"] || w == ["decider", "answer"]);
    if passes_gate {
        vec![("CLAUDE_CODE_SESSION_ID", "test-harness-session")]
    } else {
        Vec::new()
    }
}
