//! The run journal of one scripted session, compared byte for byte with a
//! checked-in fixture.
//!
//! The session covers the record families a journal holds: `koto init`, a
//! command gate, a decider check answered by the in-process stub, a review
//! round recorded with `koto review record`, and the terminal tick. The test
//! pins what koto writes today, so a change that is meant to leave the
//! journal alone (a module move, a new consumer of the same records) is
//! shown to have left it alone.
//!
//! The comparison is on the journal's raw lines, so field order and
//! spelling are part of what is pinned. Five values differ from run to run
//! and are replaced before comparing, each by a fixed token that keeps the
//! line's shape:
//!
//! - `at`, the time of the record;
//! - `koto.session.id` and `koto.run.id`, minted at `koto init` (the test
//!   still checks that every line carries the same one);
//! - `koto.decider.latency_ms` and `koto.check.duration_ms`, the measured
//!   times of the provider call and of a check.
//!
//! To write the fixture again after a deliberate change to the journal, run
//! this test with `KOTO_UPDATE_GOLDEN=1` and review the diff.

#![cfg(unix)]

#[path = "support/decider_stub.rs"]
mod decider_stub;
#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;

use decider_stub::{DeciderStub, Reply};
use serde_json::Value;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const JOURNAL: &str = "_run_journal.jsonl";
const FIXTURE: &str = "tests/fixtures/run-journal-golden/journal.jsonl";
const SESSION: &str = "golden";
const DRIVER: &str = "0f0e0d0c-1111-4222-8333-444455556666";

/// `work` holds a command gate and a decider check and waits for a
/// verdict; `done` is terminal.
const TEMPLATE: &str = r#"---
name: journal-golden
version: "1.0"
initial_state: work
review_vocabulary:
  seats: [clarity, scope]
  sections: [goals, document]
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
        command: "exit 0"
      comments:
        type: decider-check
        command: "cat slice.txt"
        label: comments
        criteria:
          "reasons":
            rule_ref: "https://example.org/rules/reasons"
            question: "Does each comment give a reason?"
            pass: "Every comment says why."
            fail: "A comment restates the code."
            escape: "No comment, or can't tell."
            mode: veto
    transitions:
      - target: done
        when:
          verdict: finish
  done:
    terminal: true
---

## work

Do the work.

## done

Done.
"#;

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
        std::fs::write(work.join("template.md"), TEMPLATE).unwrap();
        std::fs::write(
            work.join("slice.txt"),
            "let total = a + b; // add a and b\n",
        )
        .unwrap();
        Env { tmp, home, work }
    }

    /// Run `koto <args>` in the session's working directory, under this
    /// test's home, with the decider pointed at `stub`. Every variable that
    /// would redirect the store, the config or the driver is removed or
    /// fixed, so the journal depends on nothing outside the test.
    fn koto(&self, stub: &DeciderStub, args: &[&str]) -> String {
        let mut cmd = koto_command::koto();
        cmd.current_dir(&self.work)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", self.tmp.path().join("cache"))
            .env("CLAUDE_CODE_SESSION_ID", DRIVER)
            .env("KOTO_DECIDER", "auto")
            .env("KOTO_DECIDER_API_KEY", "golden-test-key")
            .env("KOTO_DECIDER_ENDPOINT", stub.url());
        for unset in [
            "KOTO_SESSIONS_BASE",
            "KOTO_HOME",
            "XDG_CONFIG_HOME",
            "KOTO_WORKFLOWS_DIR",
            "KOTO_ANCHOR",
        ] {
            cmd.env_remove(unset);
        }
        let output = cmd.args(args).output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        assert!(
            output.status.success(),
            "koto {args:?} failed\nstdout: {stdout}\nstderr: {stderr}"
        );
        assert!(
            !stderr.contains("run journal"),
            "koto {args:?} warned about the journal: {stderr}"
        );
        stdout
    }

    fn journal(&self) -> String {
        std::fs::read_to_string(self.home.join(".koto").join(JOURNAL)).unwrap()
    }
}

/// Run the scripted session and return its journal.
fn scripted_journal() -> String {
    let env = Env::new();
    let stub = DeciderStub::start();

    env.koto(&stub, &["init", SESSION, "--template", "template.md"]);

    env.koto(
        &stub,
        &[
            "review",
            "record",
            SESSION,
            "round-started",
            "--doc-type",
            "prd",
            "--schema",
            "prd/v1",
            "--round",
            "1",
            "--seat-count",
            "2",
        ],
    );
    let verdicts = serde_json::json!([
        {"rule_id": "rs-001", "section": "goals", "verdict": "PASS", "seat": "clarity",
         "round": 1, "doc_type": "prd", "schema": "prd/v1"},
        {"rule_id": "rs-001", "section": "goals", "verdict": "FAIL", "seat": "scope",
         "round": 1, "doc_type": "prd", "schema": "prd/v1"},
    ]);
    let file = env.work.join("verdicts.json");
    std::fs::write(&file, verdicts.to_string()).unwrap();
    env.koto(
        &stub,
        &[
            "review",
            "record",
            SESSION,
            "verdicts",
            "--file",
            file.to_str().unwrap(),
        ],
    );

    // The first tick runs the command gate and the decider check, then
    // waits for the verdict.
    stub.push(Reply::json(&serde_json::json!({
        "model": "golden-model-1.0.0",
        "answers": {"reasons": {"type": "choice", "choice": "pass",
            "probabilities": {"pass": 0.95, "fail": 0.02, "unclear": 0.03}}},
        "usage": {"input_tokens": 100, "output_tokens": 3}
    })));
    let out = env.koto(&stub, &["next", SESSION]);
    let json: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(json["action"], "evidence_required", "{json}");
    assert_eq!(stub.request_count(), 1);

    // The second tick submits the verdict and reaches the terminal state.
    let out = env.koto(
        &stub,
        &["next", SESSION, "--with-data", r#"{"verdict":"finish"}"#],
    );
    let json: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(json["action"], "done", "{json}");

    env.journal()
}

/// Replace the string or number that follows `"<field>":` on `line` with
/// `token`, leaving every other byte as it is. A line without the field is
/// returned unchanged.
fn mask(line: &str, field: &str, token: &str) -> String {
    let key = format!("\"{field}\":");
    let Some(start) = line.find(&key) else {
        return line.to_string();
    };
    let value = start + key.len();
    let rest = &line[value..];
    let end = if let Some(inner) = rest.strip_prefix('"') {
        // None of the masked values holds an escaped quote.
        inner.find('"').expect("unterminated string") + 2
    } else {
        rest.find([',', '}']).expect("unterminated value")
    };
    format!("{}{}{}", &line[..value], token, &rest[end..])
}

fn masked(journal: &str) -> String {
    let mut out = String::new();
    for line in journal.lines() {
        let mut line = mask(line, "at", "\"<at>\"");
        line = mask(&line, "koto.session.id", "\"<session-id>\"");
        line = mask(&line, "koto.run.id", "\"<run-id>\"");
        line = mask(&line, "koto.decider.latency_ms", "0");
        line = mask(&line, "koto.check.duration_ms", "0");
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)
}

#[test]
fn a_scripted_session_writes_the_golden_journal() {
    let journal = scripted_journal();
    assert!(journal.ends_with('\n'), "the journal ends with a newline");

    // The masked ids are still required to agree: one root session, so
    // every line carries the same session id and the same run id.
    let lines: Vec<Value> = journal
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let id = lines[0]["koto.session.id"].as_str().unwrap().to_string();
    for line in &lines {
        assert_eq!(line["koto.session.id"], id.as_str(), "{line}");
        assert_eq!(line["koto.run.id"], id.as_str(), "{line}");
        assert!(line["at"].is_string(), "{line}");
    }

    // The scripted session covers each family the fixture is meant to pin.
    let kinds: Vec<&str> = lines.iter().map(|l| l["kind"].as_str().unwrap()).collect();
    for kind in [
        "session_started",
        "check_evaluated",
        "decider_checked",
        "review_round",
        "review_verdict",
        "terminal",
    ] {
        assert!(kinds.contains(&kind), "no `{kind}` record in {kinds:?}");
    }

    let actual = masked(&journal);
    let path = fixture_path();
    if std::env::var_os("KOTO_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("can't read {}: {e}", path.display()));
    assert_eq!(
        actual, expected,
        "the scripted session's journal differs from {FIXTURE}; if the change is \
         deliberate, run this test with KOTO_UPDATE_GOLDEN=1 and review the diff"
    );
}

#[test]
fn mask_replaces_one_value_and_keeps_the_rest() {
    let line = r#"{"kind":"x","at":"2026-01-01T00:00:00Z","koto.decider.latency_ms":412,"n":1}"#;
    assert_eq!(
        mask(line, "at", "\"<at>\""),
        r#"{"kind":"x","at":"<at>","koto.decider.latency_ms":412,"n":1}"#
    );
    assert_eq!(
        mask(line, "koto.decider.latency_ms", "0"),
        r#"{"kind":"x","at":"2026-01-01T00:00:00Z","koto.decider.latency_ms":0,"n":1}"#
    );
    assert_eq!(mask(line, "absent", "0"), line);
    assert_eq!(
        mask(
            r#"{"koto.decider.latency_ms":7}"#,
            "koto.decider.latency_ms",
            "0"
        ),
        r#"{"koto.decider.latency_ms":0}"#
    );
}
