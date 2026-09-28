//! The published session-feed contract covers what koto writes
//! (PLAN-koto-failure-reporting.md, Issue 5).
//!
//! One session is driven through the built `koto` so that its log holds every
//! failure-reporting field on `gate_evaluated` and `default_action_executed`,
//! a `context_read` with every field, and `context_added` and
//! `context_removed` with `writer`. The log is then checked two ways:
//!
//!   * every top-level payload key koto wrote on those five events must be
//!     declared in the frontmatter of `docs/reference/session-feed.md` with
//!     the type the value has, so a field koto starts writing without
//!     documenting it fails here;
//!   * `koto template validate-feed` must accept the log, and reject a
//!     `context_read` line missing `key` and a `gate_evaluated` line whose
//!     `attempt` is a string.
//!
//! A self-check asserts the session really exercised every new field, so the
//! coverage check can't pass by the scenario quietly writing less.

#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

/// The events whose payload keys this test holds to the contract.
const CHECKED_EVENTS: [&str; 5] = [
    "gate_evaluated",
    "default_action_executed",
    "context_added",
    "context_removed",
    "context_read",
];

/// Keys the scenario must produce on each event, so the coverage check sees
/// every field the failure-reporting work added.
fn expected_keys() -> BTreeMap<&'static str, Vec<&'static str>> {
    let check = [
        "attempt",
        "visit_attempt",
        "findings",
        "findings_truncated",
        "rule_counts",
        "rule_counts_truncated",
        "duration_ms",
    ];
    let mut gate: Vec<&str> = check.to_vec();
    gate.extend(["stdout", "stderr", "stdout_truncated", "stderr_truncated"]);
    BTreeMap::from([
        ("gate_evaluated", gate),
        ("default_action_executed", check.to_vec()),
        ("context_added", vec!["writer"]),
        ("context_removed", vec!["writer"]),
        (
            "context_read",
            vec![
                "key", "reader", "state", "present", "hash", "access", "gate",
            ],
        ),
    ])
}

fn spec_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/reference/session-feed.md")
}

/// The contract's frontmatter, parsed.
fn spec() -> Value {
    let text = std::fs::read_to_string(spec_path()).unwrap();
    let rest = text
        .strip_prefix("---\n")
        .expect("the contract starts with frontmatter");
    let end = rest.find("\n---\n").expect("the frontmatter is closed");
    serde_yaml_ng::from_str(&rest[..end]).expect("the frontmatter parses")
}

/// Whether `value` has the frontmatter type `ty`, by the rules
/// `koto template validate-feed` applies.
fn has_type(value: &Value, ty: &str) -> bool {
    match ty {
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "any" => true,
        _ => false,
    }
}

/// Every problem with `payload`'s top-level keys against the declaration of
/// `event_type` in `spec`: a key not declared, or a value of another type.
fn undeclared(spec: &Value, event_type: &str, payload: &Value) -> Vec<String> {
    let fields = &spec["events"][event_type]["fields"];
    let mut problems = Vec::new();
    for (key, value) in payload.as_object().expect("a payload object") {
        let Some(ty) = fields[key.as_str()]["type"].as_str() else {
            problems.push(format!("{event_type}.{key} is not declared"));
            continue;
        };
        if !has_type(value, ty) {
            problems.push(format!(
                "{event_type}.{key} is declared {ty} but koto wrote {value}"
            ));
        }
    }
    problems
}

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sessions")).unwrap();
        Env { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn koto(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_koto"));
        cmd.env_clear()
            .current_dir(self.path())
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.path())
            .env("KOTO_SESSIONS_BASE", self.path().join("sessions"));
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.koto().args(args).output().unwrap()
    }

    fn run_ok(&self, args: &[&str]) -> Output {
        let out = self.run(args);
        assert!(out.status.success(), "koto {args:?}: {}", describe(&out));
        out
    }

    fn script(&self, name: &str, body: &str) -> String {
        let path = self.path().join(name);
        std::fs::write(&path, body).unwrap();
        format!("sh {}", path.display())
    }

    fn next(&self) -> Value {
        let out = self.run(&["next", "wf", "--no-cleanup"]);
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("next should print JSON: {}", describe(&out)))
    }

    fn log_path(&self) -> PathBuf {
        self.path()
            .join("sessions")
            .join("wf")
            .join("koto-wf.state.jsonl")
    }

    fn validate_feed(&self, log: &Path) -> Output {
        self.koto()
            .args(["template", "validate-feed"])
            .arg(log)
            .env("KOTO_FEED_SPEC", spec_path())
            .output()
            .unwrap()
    }
}

fn describe(out: &Output) -> String {
    format!(
        "status={:?} stdout={} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A shell loop printing 60 `error` findings with distinct rule ids
/// `<prefix>0` .. `<prefix>59`, so both the 50-finding and the 50-key caps
/// are exceeded.
fn sixty_errors(prefix: &str) -> String {
    format!(
        r#"i=0
while [ $i -lt 60 ]; do
  printf '::koto-finding::{{"rule_id":"{prefix}%d","level":"error","message":"m","path":"a.py","line":1,"column":2,"rule_ref":"r"}}\n' $i
  i=$((i+1))
done
"#
    )
}

/// Drive one session through every field the contract added and return the
/// path of its log.
fn produce_log(env: &Env) -> PathBuf {
    let fail_flag = env.path().join("act-fail");
    let act = env.script(
        "act.sh",
        &format!(
            "if [ -f {flag} ]; then\n{errors}exit 1\nfi\nexit 0\n",
            flag = fail_flag.display(),
            errors = sixty_errors("A"),
        ),
    );
    // Over 4 KiB on both streams, so each logged stream is cut.
    let lint = env.script(
        "lint.sh",
        &format!(
            "{errors}head -c 10000 /dev/zero | tr '\\000' x\necho\nhead -c 5000 /dev/zero | tr '\\000' y >&2\nexit 1\n",
            errors = sixty_errors("E"),
        ),
    );
    let template = format!(
        r#"---
name: feed-contract
version: "1.0"
initial_state: act
states:
  act:
    default_action:
      command: '{act}'
    transitions:
      - target: check
  check:
    gates:
      lint:
        type: command
        command: '{lint}'
      has_note:
        type: context-exists
        key: note
    transitions:
      - target: done
  done:
    terminal: true
---

## act

Act.

## check

Check.

## done

Done.
"#
    );
    let tpl = env.path().join("template.md");
    std::fs::write(&tpl, template).unwrap();
    env.run_ok(&["init", "wf", "--template", tpl.to_str().unwrap()]);

    // A failing action: findings and rule counts over their caps.
    std::fs::write(&fail_flag, "").unwrap();
    let resp = env.next();
    assert_eq!(resp["state"], "act", "{resp}");

    // The action passes; both gates fail, and the context gate reads an
    // absent key.
    std::fs::remove_file(&fail_flag).unwrap();
    let resp = env.next();
    assert_eq!(resp["state"], "check", "{resp}");

    // A write, a content read and a presence read from the CLI.
    let note = env.path().join("note.txt");
    std::fs::write(&note, "ok\n").unwrap();
    env.run_ok(&[
        "context",
        "add",
        "wf",
        "note",
        "--from-file",
        note.to_str().unwrap(),
    ]);
    env.run_ok(&["context", "get", "wf", "note"]);
    env.run_ok(&["context", "exists", "wf", "note"]);

    // The context gate now reads a present key, so its read carries a hash.
    let resp = env.next();
    assert_eq!(resp["state"], "check", "{resp}");

    env.run_ok(&["context", "remove", "wf", "note"]);
    env.log_path()
}

fn log_events(log: &Path) -> Vec<Value> {
    std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn every_key_koto_writes_on_the_new_fields_is_declared_with_its_type() {
    let env = Env::new();
    let log = produce_log(&env);
    let spec = spec();

    let mut seen: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut problems = Vec::new();
    for event in log_events(&log) {
        let ty = event["type"].as_str().unwrap();
        if !CHECKED_EVENTS.contains(&ty) {
            continue;
        }
        let payload = &event["payload"];
        seen.entry(ty.to_string())
            .or_default()
            .extend(payload.as_object().unwrap().keys().cloned());
        problems.extend(undeclared(&spec, ty, payload));
    }
    problems.sort();
    problems.dedup();
    assert!(
        problems.is_empty(),
        "koto writes fields the contract doesn't declare:\n{}",
        problems.join("\n")
    );

    // The scenario must have produced every new field, or the check above
    // proved nothing about it.
    for (ty, keys) in expected_keys() {
        let got = seen.get(ty).cloned().unwrap_or_default();
        let missing: Vec<&str> = keys.iter().copied().filter(|k| !got.contains(*k)).collect();
        assert!(
            missing.is_empty(),
            "the session wrote no {missing:?} on {ty}; it wrote {got:?}"
        );
    }
}

#[test]
fn the_coverage_check_reports_an_undeclared_or_mistyped_key() {
    let spec = spec();
    let payload = serde_json::json!({
        "state": "s", "gate": "g", "output": {}, "outcome": "failed",
        "timestamp": "t", "attempt": "2", "not_in_the_contract": 1,
    });
    let problems = undeclared(&spec, "gate_evaluated", &payload);
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(problems.iter().any(|p| p.contains("not_in_the_contract")));
    assert!(problems.iter().any(|p| p.contains("attempt")));
}

#[test]
fn validate_feed_accepts_the_session_and_rejects_broken_new_fields() {
    let env = Env::new();
    let log = produce_log(&env);
    let out = env.validate_feed(&log);
    assert!(out.status.success(), "{}", describe(&out));

    let text = std::fs::read_to_string(&log).unwrap();
    let header = text.lines().next().unwrap();
    let find = |ty: &str| -> Value {
        log_events(&log)
            .into_iter()
            .find(|e| e["type"] == ty)
            .unwrap_or_else(|| panic!("the session wrote no {ty}"))
    };

    let mut read = find("context_read");
    read["payload"].as_object_mut().unwrap().remove("key");
    let mut gate = find("gate_evaluated");
    gate["payload"]["attempt"] = Value::String("2".to_string());

    for (event, field) in [(read, "key"), (gate, "attempt")] {
        let broken = env.path().join("broken.jsonl");
        std::fs::write(&broken, format!("{header}\n{event}\n")).unwrap();
        let out = env.validate_feed(&broken);
        assert!(!out.status.success(), "{field}: {}", describe(&out));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(&format!("'{field}'")),
            "{field}: {}",
            describe(&out)
        );
    }
}
