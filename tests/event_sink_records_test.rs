//! What the event sink receives, driven through the koto binary against
//! the stub sink of `tests/support/event_sink.rs`.
//!
//! The sink gets the records the run journal gets: the same kinds, the
//! same fields with the same values, in the same order. The copy it gets
//! carries three fields the journal's line doesn't: the run's template
//! name and hash on every record, and on a `check_rule` the outcome of the
//! evaluation it belongs to. A `context_reads` record is journaled and
//! never sent. Turning the journal off stops the journal and not the
//! sending. The sink's environment is a short allowlist.

#![cfg(unix)]

#[path = "support/event_sink.rs"]
mod event_sink;
#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;

use event_sink::{sent, SinkHome};
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;

/// The fields only the sent copy carries.
const STAMPS: &[&str] = &[
    "koto.run.template.name",
    "koto.run.template.hash",
    "koto.check.outcome",
];

/// `work` holds a command gate that reports one error rule and fails until
/// its script is told to pass, and waits for a verdict; `done` is terminal.
const TEMPLATE: &str = r#"---
name: sink-records
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      verdict:
        type: enum
        required: true
        values: [finish]
    gates:
      lint:
        type: command
        command: "./check.sh"
    transitions:
      - target: done
        when:
          verdict: finish
          gates.lint.exit_code: 0
  done:
    terminal: true
---

## work

Fix what the check reports.

## done

Done.
"#;

/// Fails with one finding while `fail` exists beside it, passes otherwise.
const CHECK_SH: &str = r#"#!/bin/sh
if [ -f fail ]; then
  echo '::koto-finding::{"rule_id":"line-length","level":"error","message":"line too long","path":"src/a.rs","line":3}'
  exit 1
fi
exit 0
"#;

fn ok(home: &SinkHome, args: &[&str]) -> String {
    ok_with(home, args, &[])
}

fn ok_with(home: &SinkHome, args: &[&str], env: &[(&str, &str)]) -> String {
    let mut cmd = home.koto();
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.args(args).output().unwrap();
    assert!(
        out.status.success(),
        "koto {args:?}: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn write_session_files(home: &SinkHome) {
    std::fs::write(home.work.join("template.md"), TEMPLATE).unwrap();
    let script = home.work.join("check.sh");
    std::fs::write(&script, CHECK_SH).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// One session that writes every family of record: identity, a failing
/// check with a rule and a finding, a response, a context read, a passing
/// check, evidence and a terminal.
fn run_session(home: &SinkHome, env: &[(&str, &str)]) {
    write_session_files(home);
    std::fs::write(home.work.join("fail"), "").unwrap();
    ok_with(home, &["init", "wf", "--template", "template.md"], env);
    // Blocked on the failing check.
    ok_with(home, &["next", "wf", "--no-cleanup"], env);
    // A context key written and read back inside the visit.
    let notes = home.work.join("notes.md");
    std::fs::write(&notes, "notes\n").unwrap();
    ok_with(
        home,
        &[
            "context",
            "add",
            "wf",
            "notes.md",
            "--from-file",
            notes.to_str().unwrap(),
        ],
        env,
    );
    ok_with(home, &["context", "get", "wf", "notes.md"], env);
    // The check passes now, and the verdict ends the session.
    std::fs::remove_file(home.work.join("fail")).unwrap();
    let out = ok_with(
        home,
        &[
            "next",
            "wf",
            "--with-data",
            r#"{"verdict":"finish"}"#,
            "--no-cleanup",
        ],
        env,
    );
    let json: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(json["action"], "done", "{json}");
}

fn kinds(records: &[Value]) -> Vec<&str> {
    records
        .iter()
        .map(|r| r["kind"].as_str().unwrap())
        .collect()
}

/// `record` without the fields only the sent copy carries.
fn without_stamps(record: &Value) -> Value {
    let mut copy = record.clone();
    let fields = copy.as_object_mut().unwrap();
    for stamp in STAMPS {
        // `koto.check.outcome` is a stamp only on a `check_rule`; a
        // `check_evaluated` carries it in the journal too.
        if *stamp == "koto.check.outcome" && record["kind"] != "check_rule" {
            continue;
        }
        fields.remove(*stamp);
    }
    copy
}

#[test]
fn sent_records_match_journal_plus_stamps() {
    let (home, stub) = SinkHome::with_stub("records");
    run_session(&home, &[]);

    let journal = home.journal();
    let calls = stub.calls();
    assert!(calls.iter().all(|c| c.done), "a call didn't finish");
    let sent = sent(&calls);

    // The journal holds a `context_reads`; the sink never got one.
    assert!(
        kinds(&journal).contains(&"context_reads"),
        "the session journaled no context_reads, so the exclusion isn't exercised: {:?}",
        kinds(&journal)
    );
    assert!(!kinds(&sent).contains(&"context_reads"));

    // Apart from that kind: the same records, in the same order, each one
    // the journal's record plus the stamps.
    let journaled: Vec<&Value> = journal
        .iter()
        .filter(|r| r["kind"] != "context_reads")
        .collect();
    assert_eq!(
        kinds(&sent),
        journaled
            .iter()
            .map(|r| r["kind"].as_str().unwrap())
            .collect::<Vec<_>>()
    );
    for (s, j) in sent.iter().zip(&journaled) {
        assert_eq!(&without_stamps(s), *j, "sent {s}\njournaled {j}");
    }
    for kind in [
        "session_started",
        "state_entered",
        "check_evaluated",
        "check_rule",
        "check_finding",
        "response",
        "evidence_field",
        "terminal",
    ] {
        assert!(kinds(&sent).contains(&kind), "no {kind} was sent");
    }

    // Every sent record names the run's template, which for a root is its
    // own: the name and hash its `session_started` journals.
    let started = journal
        .iter()
        .find(|r| r["kind"] == "session_started")
        .unwrap();
    assert_eq!(started["koto.template.name"], "sink-records");
    for s in &sent {
        assert_eq!(s["koto.run.template.name"], "sink-records", "{s}");
        assert_eq!(
            s["koto.run.template.hash"], started["koto.template.hash"],
            "{s}"
        );
    }
    // The journal carries neither stamp.
    for j in &journal {
        assert!(j.get("koto.run.template.name").is_none(), "{j}");
        assert!(j.get("koto.run.template.hash").is_none(), "{j}");
    }

    // A `check_rule` carries the outcome of the evaluation it follows,
    // which the journal's `check_rule` doesn't.
    let rules: Vec<&Value> = sent.iter().filter(|r| r["kind"] == "check_rule").collect();
    assert!(!rules.is_empty());
    for rule in rules {
        let evaluation = sent
            .iter()
            .find(|r| {
                r["kind"] == "check_evaluated"
                    && r["koto.seq"] == rule["koto.seq"]
                    && r["koto.check"] == rule["koto.check"]
            })
            .unwrap();
        assert_eq!(rule["koto.check.outcome"], "failed", "{rule}");
        assert_eq!(rule["koto.check.outcome"], evaluation["koto.check.outcome"]);
    }
    for j in journal.iter().filter(|r| r["kind"] == "check_rule") {
        assert!(j.get("koto.check.outcome").is_none(), "{j}");
    }

    // The stamps come last on the sent line, so it is the journal's line
    // with a suffix. Checked on the raw text of the first record.
    let raw_journal = std::fs::read_to_string(home.koto_home().join("_run_journal.jsonl")).unwrap();
    let first_journaled = raw_journal.lines().next().unwrap();
    let first_sent = &calls[0].lines[0];
    assert!(
        first_sent.starts_with(first_journaled.strip_suffix('}').unwrap()),
        "sent {first_sent}\njournaled {first_journaled}"
    );

    // Nothing was dropped.
    assert_eq!(home.dropped("wf"), 0);
}

#[test]
fn a_childs_records_carry_its_roots_template() {
    let (home, stub) = SinkHome::with_stub("child");
    write_session_files(&home);
    ok(&home, &["init", "wf", "--template", "template.md"]);
    let other = TEMPLATE.replace("name: sink-records", "name: sink-records-child");
    std::fs::write(home.work.join("child.md"), other).unwrap();
    ok(
        &home,
        &["init", "wf.kid", "--template", "child.md", "--parent", "wf"],
    );
    let sent = sent(&stub.calls());
    let journal = home.journal();
    let root = journal
        .iter()
        .find(|r| r["kind"] == "session_started" && r["session"] == "wf")
        .unwrap();
    let kid: Vec<&Value> = sent.iter().filter(|r| r["session"] == "wf.kid").collect();
    assert!(!kid.is_empty());
    for record in kid {
        assert_eq!(record["koto.run.template.name"], "sink-records", "{record}");
        assert_eq!(record["koto.run.template.hash"], root["koto.template.hash"]);
    }
}

#[test]
fn journal_disabled_still_sends() {
    let (home, stub) = SinkHome::with_stub("nojournal");
    ok(
        &home,
        &["config", "set", "run_journal.enabled", "false", "--user"],
    );
    run_session(&home, &[]);

    // No journal was written...
    let stray: Vec<String> = std::fs::read_dir(home.koto_home())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with("_run_journal"))
        .collect();
    assert!(stray.is_empty(), "{stray:?}");

    // ...and the sink still got the session's records, stamped.
    let sent = sent(&stub.calls());
    for kind in [
        "session_started",
        "state_entered",
        "check_evaluated",
        "check_rule",
        "response",
        "terminal",
    ] {
        assert!(
            kinds(&sent).contains(&kind),
            "no {kind} in {:?}",
            kinds(&sent)
        );
    }
    assert!(!kinds(&sent).contains(&"context_reads"));
    for s in &sent {
        assert_eq!(s["koto.run.template.name"], "sink-records", "{s}");
    }
}

#[test]
fn sink_env_is_allowlist_only() {
    let (home, stub) = SinkHome::with_stub("env");
    std::fs::write(home.work.join("template.md"), TEMPLATE).unwrap();
    let env = [
        (
            "CLAUDE_CODE_SESSION_ID",
            "0f0e0d0c-1111-4222-8333-444455556666",
        ),
        ("NIWA_DISPATCH_ID", "dispatch-7"),
        ("NIWA_WORKSPACE", "workspace-a"),
        // Things a shell holds that the sink must not see.
        ("AWS_SECRET_ACCESS_KEY", "decoy-credential"),
        ("GH_TOKEN", "decoy-token"),
        ("HTTPS_PROXY", "http://proxy.invalid:8080"),
        ("SSL_CERT_FILE", "/nonexistent/cert.pem"),
        ("KOTO_DECIDER_API_KEY", "decoy-key"),
        ("LANG", "C.UTF-8"),
    ];
    write_session_files(&home);
    ok_with(&home, &["init", "wf", "--template", "template.md"], &env);

    let calls = stub.calls();
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(call.argv, vec!["send", "--scope", "koto-journal"]);

    let allowed = [
        "HOME",
        "PATH",
        "TMPDIR",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "CLAUDE_CODE_SESSION_ID",
        "NIWA_DISPATCH_ID",
        "NIWA_WORKSPACE",
    ];
    // Variables the stub's own shell sets for itself.
    let shell_own = [
        "PWD", "OLDPWD", "SHLVL", "_", "IFS", "OPTIND", "PS1", "PS2", "PS4",
    ];
    for name in call.env_names() {
        assert!(
            allowed.contains(&name.as_str()) || shell_own.contains(&name.as_str()),
            "the sink was started with {name}"
        );
    }
    let value = |name: &str| -> Option<&str> {
        call.env
            .iter()
            .find_map(|l| l.strip_prefix(&format!("{name}=")))
    };
    assert_eq!(
        value("CLAUDE_CODE_SESSION_ID"),
        Some("0f0e0d0c-1111-4222-8333-444455556666")
    );
    assert_eq!(value("NIWA_DISPATCH_ID"), Some("dispatch-7"));
    assert_eq!(value("NIWA_WORKSPACE"), Some("workspace-a"));
    assert_eq!(value("LANG"), Some("C.UTF-8"));
    assert_eq!(value("HOME"), home.home.to_str());
    // The sink runs from `/`, not from the session's directory.
    assert_eq!(value("PWD"), Some("/"));
    let all = call.env.join("\n");
    for decoy in [
        "decoy-credential",
        "decoy-token",
        "proxy.invalid",
        "decoy-key",
    ] {
        assert!(!all.contains(decoy), "the sink's environment holds {decoy}");
    }
}
