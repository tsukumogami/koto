//! Integration tests for a person's `answer`, checked against the
//! harness's record of the question-prompt call (PRD R62 to R68).
//!
//! Every test runs the real binary with `HOME` and `KOTO_SESSIONS_BASE`
//! in its own temporary directory, installs a fixture from
//! `tests/fixtures/prompt/` as the record of a harness session under
//! `$HOME/.claude/projects/`, and runs the answer with
//! `CLAUDE_CODE_SESSION_ID` naming that harness session. No live prompt is
//! involved. Each refusal asserts the exit code, that the answering
//! session holds no new entry, the check named, and that the fixture's
//! marker string appears nowhere in koto's output.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;
#[path = "support/prompt_fixture.rs"]
mod prompt_fixture;

use std::path::{Path, PathBuf};

use assert_fs::TempDir;
use prompt_fixture as fx;
use serde_json::Value;

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

const HARNESS: &str = "harness-a";
const OTHER_HARNESS: &str = "harness-b";
const RAISER: &str = "raiser";
const ANSWERER: &str = "answerer";

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

    fn all(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
    }
}

/// A raised question with stored content.
#[derive(Clone)]
struct Q {
    id: String,
    header: String,
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        for dir in ["home", "ws", "sessions"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("work.md"), TEMPLATE).unwrap();
        let env = Env { _tmp: tmp, root };
        env.init(RAISER);
        env.init(ANSWERER);
        env
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn command(&self, harness: Option<&str>, args: &[&str]) -> std::process::Command {
        let mut cmd = koto_command::koto_std();
        for name in CLEARED_ENV {
            cmd.env_remove(name);
        }
        if let Some(h) = harness {
            cmd.env("CLAUDE_CODE_SESSION_ID", h);
        }
        cmd.current_dir(self.root.join("ws"));
        cmd.env("HOME", self.home());
        cmd.env("KOTO_HOME", self.home());
        cmd.env("KOTO_SESSIONS_BASE", self.root.join("sessions"));
        cmd.args(args);
        cmd
    }

    fn run_h(&self, harness: Option<&str>, args: &[&str]) -> Out {
        let output = self.command(harness, args).output().unwrap();
        Out {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    fn ok(&self, args: &[&str]) -> Out {
        let out = self.run_h(None, args);
        assert_eq!(out.code, 0, "{args:?}\n{}", out.all());
        out
    }

    fn init(&self, name: &str) {
        let template = self.root.join("work.md");
        self.ok(&["init", name, "--template", template.to_str().unwrap()]);
    }

    /// Raise a question to `person:owner` in the raiser with `content`
    /// stored for it.
    fn raise(&self, content: &[u8]) -> Q {
        let packet = format!("packet={}", fx::packet(content));
        let out = self
            .ok(&[
                "decisions",
                "record",
                RAISER,
                "--kind",
                "escalate",
                "--outcome",
                "escalated",
                "--subject",
                "question=which-way",
                "--subject",
                "text=which way?",
                "--subject",
                "recipient=person:owner",
                "--subject",
                &packet,
                "--by",
                "session",
            ])
            .json();
        let id = out["id"].as_str().unwrap().to_string();
        let file = self.root.join(format!("content-{id}"));
        std::fs::write(&file, content).unwrap();
        let written = self
            .ok(&[
                "decisions",
                "content",
                "write",
                RAISER,
                &id,
                "--file",
                file.to_str().unwrap(),
            ])
            .json();
        Q {
            id,
            header: written["prompt_header"].as_str().unwrap().to_string(),
        }
    }

    /// Raise a question whose content is what `lines` shows at `position`.
    fn raise_shown(&self, lines: &[Value], position: usize) -> Q {
        let (question, labels) = fx::shown(lines, position);
        self.raise(&fx::content(&question, &labels))
    }

    fn answer(
        &self,
        q: &Q,
        harness: Option<&str>,
        by: &str,
        pointer: Option<&str>,
        text: Option<&str>,
    ) -> Out {
        let mut args = vec!["decisions", "answer", RAISER, &q.id, "--in", ANSWERER];
        args.extend_from_slice(&["--by", by]);
        if let Some(p) = pointer {
            args.extend_from_slice(&["--prompt", p]);
        }
        if let Some(t) = text {
            args.extend_from_slice(&["--text", t]);
        }
        self.run_h(harness, &args)
    }

    /// The answering session's decision files.
    fn entries(&self) -> usize {
        fn count(p: &Path) -> usize {
            match std::fs::read_dir(p) {
                Ok(rd) => rd
                    .flatten()
                    .map(|e| {
                        let p = e.path();
                        if p.is_dir() {
                            count(&p)
                        } else {
                            1
                        }
                    })
                    .sum(),
                Err(_) => 0,
            }
        }
        let ctx = self.root.join("sessions").join(ANSWERER).join("ctx/koto");
        count(&ctx.join("aimed")) + count(&ctx.join("decisions"))
    }
}

fn pointer(harness: &str, call: &str, position: usize) -> String {
    format!("{harness}/{call}/{position}")
}

/// Assert `out` is a refusal naming `check`, that left `before` entries
/// and printed no fixture text.
fn refused(env: &Env, out: &Out, before: usize, check: &str) {
    assert_eq!(out.code, 2, "{}", out.all());
    assert_eq!(env.entries(), before, "a refusal wrote an entry");
    let json = out.json();
    let message = json["error"].as_str().unwrap();
    assert!(
        message.contains(&format!("'{check}' check")),
        "expected the {check} check: {message}"
    );
    assert!(
        !out.all().contains(fx::MARKER),
        "record text leaked: {}",
        out.all()
    );
}

/// Install `lines` (one question at position 1, header `q`'s) for HARNESS,
/// raise nothing, and answer `q` with the fixture pointer.
fn answer_with(env: &Env, q: &Q, lines: &[Value]) -> Out {
    fx::install(
        &env.home(),
        "proj",
        HARNESS,
        &fx::render(lines, HARNESS, &[&q.header]),
    );
    env.answer(
        q,
        Some(HARNESS),
        "person:owner",
        Some(&pointer(HARNESS, fx::CALL, 1)),
        None,
    )
}

#[test]
fn a_matching_call_is_accepted_and_the_entry_shows_the_pointer() {
    let env = Env::new();
    let lines = fx::lines("accepted");
    let q = env.raise_shown(&lines, 1);
    let out = answer_with(&env, &q, &lines);
    assert_eq!(out.code, 0, "{}", out.all());
    let d = &out.json()["decision"];
    assert_eq!(d["prompt"]["harness"], HARNESS);
    assert_eq!(d["prompt"]["call"], fx::CALL);
    assert_eq!(d["prompt"]["position"], 1);
    assert_eq!(d["prompt"]["mark"]["value"], "verified");
    assert_eq!(d["prompt"]["mark"]["source"], "observed");
    assert_eq!(d["decider"]["handle"]["value"], "owner");
    assert_eq!(d["decider"]["handle"]["source"], "handed");
    assert_eq!(d["subject"]["text"], fx::answer_of(&lines, 1));
    assert_eq!(env.entries(), 1);

    let id = out.json()["id"].as_str().unwrap().to_string();
    let shown = env.ok(&["decisions", "show", ANSWERER, &id]).json();
    assert_eq!(shown["reading"], "verified");
    assert_eq!(shown["decision"]["prompt"]["mark"]["source"], "observed");
    assert_eq!(shown["decision"]["decider"]["handle"]["source"], "handed");
    let listed = env
        .ok(&["decisions", "list", ANSWERER, "--format", "v1"])
        .json();
    let entry = &listed["decisions"][0];
    assert_eq!(entry["reading"], "verified");
    assert_eq!(entry["decision"]["prompt"]["mark"]["value"], "verified");
}

#[test]
fn a_passed_up_question_takes_a_verified_answer_by_its_identity() {
    let env = Env::new();
    let lines = fx::lines("accepted");
    let q = env.raise_shown(&lines, 1);
    env.ok(&[
        "decisions",
        "readdress",
        RAISER,
        &q.id,
        "--in",
        RAISER,
        "--to",
        "person:dana",
        "--by",
        "session",
    ]);
    let open = env.ok(&["decisions", "open", "--person", "dana"]).json();
    assert_eq!(open["questions"].as_array().unwrap().len(), 1, "{open}");

    // The prompt still names the question by the identity it was raised
    // with: passing it up changes who it waits on, not what it is.
    fx::install(
        &env.home(),
        "proj",
        HARNESS,
        &fx::render(&lines, HARNESS, &[&q.header]),
    );
    let out = env.answer(
        &q,
        Some(HARNESS),
        "person:dana",
        Some(&pointer(HARNESS, fx::CALL, 1)),
        None,
    );
    assert_eq!(out.code, 0, "{}", out.all());
    let d = &out.json()["decision"];
    assert_eq!(d["prompt"]["mark"]["value"], "verified");
    assert_eq!(d["subject"]["text"], fx::answer_of(&lines, 1));
    let open = env.ok(&["decisions", "open", "--person", "dana"]).json();
    assert!(open["questions"].as_array().unwrap().is_empty(), "{open}");
}

#[test]
fn answer_text_is_taken_from_the_result_when_the_caller_passed_other_text() {
    let env = Env::new();
    let mut lines = fx::lines("accepted");
    // A free-text reply, stored verbatim.
    let (question, _) = fx::shown(&lines, 1);
    let reply = format!(
        "{} free text  with  spacing\nand a second line ",
        fx::MARKER
    );
    lines[1]["toolUseResult"]["answers"][question.as_str()] = Value::String(reply.clone());
    let q = env.raise_shown(&lines, 1);
    fx::install(
        &env.home(),
        "proj",
        HARNESS,
        &fx::render(&lines, HARNESS, &[&q.header]),
    );
    let out = env.answer(
        &q,
        Some(HARNESS),
        "person:owner",
        Some(&pointer(HARNESS, fx::CALL, 1)),
        Some("something else"),
    );
    assert_eq!(out.code, 0, "{}", out.all());
    assert_eq!(out.json()["decision"]["subject"]["text"], reply.as_str());
}

#[test]
fn three_positions_of_one_call_are_each_accepted_once() {
    let env = Env::new();
    let lines = fx::lines("accepted-three");
    let qs: Vec<Q> = (1..=3).map(|p| env.raise_shown(&lines, p)).collect();
    let headers: Vec<&str> = qs.iter().map(|q| q.header.as_str()).collect();
    fx::install(
        &env.home(),
        "proj",
        HARNESS,
        &fx::render(&lines, HARNESS, &headers),
    );
    for (i, q) in qs.iter().enumerate() {
        let out = env.answer(
            q,
            Some(HARNESS),
            "person:owner",
            Some(&pointer(HARNESS, fx::CALL, i + 1)),
            None,
        );
        assert_eq!(out.code, 0, "position {}: {}", i + 1, out.all());
        let d = &out.json()["decision"];
        assert_eq!(d["subject"]["text"], fx::answer_of(&lines, i + 1));
        assert_eq!(d["prompt"]["position"], i + 1);
    }
    assert_eq!(env.entries(), 3);
}

#[test]
fn a_reused_call_position_is_refused_and_the_first_entry_is_unchanged() {
    let env = Env::new();
    let lines = fx::lines("accepted");
    let q = env.raise_shown(&lines, 1);
    let first = answer_with(&env, &q, &lines);
    assert_eq!(first.code, 0, "{}", first.all());
    let id = first.json()["id"].as_str().unwrap().to_string();
    let before_show = env.ok(&["decisions", "show", ANSWERER, &id]).json();
    let out = env.answer(
        &q,
        Some(HARNESS),
        "person:owner",
        Some(&pointer(HARNESS, fx::CALL, 1)),
        None,
    );
    refused(&env, &out, 1, "call-position-reused");
    let after_show = env.ok(&["decisions", "show", ANSWERER, &id]).json();
    assert_eq!(before_show["decision"], after_show["decision"]);
}

#[test]
fn a_missing_call_is_refused() {
    let env = Env::new();
    let lines = fx::lines("accepted");
    let q = env.raise_shown(&lines, 1);
    fx::install(
        &env.home(),
        "proj",
        HARNESS,
        &fx::render(&lines, HARNESS, &[&q.header]),
    );
    let out = env.answer(
        &q,
        Some(HARNESS),
        "person:owner",
        Some(&pointer(HARNESS, "toolu_absent", 1)),
        None,
    );
    refused(&env, &out, 0, "call");
}

#[test]
fn a_call_that_is_not_the_question_tool_is_refused() {
    let env = Env::new();
    let mut lines = fx::lines("accepted");
    let q = env.raise_shown(&lines, 1);
    lines[0]["message"]["content"][0]["name"] = Value::String("Bash".into());
    refused(&env, &answer_with(&env, &q, &lines), 0, "tool");
}

#[test]
fn a_call_for_another_question_is_refused() {
    let env = Env::new();
    let lines = fx::lines("accepted");
    let q = env.raise_shown(&lines, 1);
    let other = env.raise(&fx::content("another question", &["x".to_string()]));
    let wrong = Q {
        id: q.id.clone(),
        header: other.header.clone(),
    };
    fx::install(
        &env.home(),
        "proj",
        HARNESS,
        &fx::render(&lines, HARNESS, &[&wrong.header]),
    );
    let out = env.answer(
        &q,
        Some(HARNESS),
        "person:owner",
        Some(&pointer(HARNESS, fx::CALL, 1)),
        None,
    );
    refused(&env, &out, 0, "identity");
}

#[test]
fn a_position_outside_the_call_is_refused() {
    let env = Env::new();
    let lines = fx::lines("accepted");
    let q = env.raise_shown(&lines, 1);
    fx::install(
        &env.home(),
        "proj",
        HARNESS,
        &fx::render(&lines, HARNESS, &[&q.header]),
    );
    let out = env.answer(
        &q,
        Some(HARNESS),
        "person:owner",
        Some(&pointer(HARNESS, fx::CALL, 2)),
        None,
    );
    refused(&env, &out, 0, "position");
}

/// One byte-comparison case: the call shows `edit` applied to what the
/// stored content holds.
fn byte_case(content_question: &str, labels: &[&str], edit: impl Fn(&mut Value), check: &str) {
    let env = Env::new();
    let labels: Vec<String> = labels.iter().map(|s| s.to_string()).collect();
    let mut lines = fx::lines("accepted");
    {
        let qs = fx::questions_mut(&mut lines);
        qs[0]["question"] = Value::String(content_question.to_string());
        qs[0]["options"] = Value::Array(
            labels
                .iter()
                .map(|l| serde_json::json!({"label": l, "description": fx::MARKER}))
                .collect(),
        );
    }
    let answers = serde_json::json!({ content_question: format!("{} reply", fx::MARKER) });
    lines[1]["toolUseResult"]["answers"] = answers;
    let q = env.raise(&fx::content(content_question, &labels));
    // The untouched copy is accepted in its own environment.
    if check == "none" {
        assert_eq!(answer_with(&env, &q, &lines).code, 0);
        return;
    }
    edit(&mut fx::questions_mut(&mut lines)[0]);
    let shown_q = lines[0]["message"]["content"][0]["input"]["questions"][0]["question"].clone();
    lines[1]["toolUseResult"]["answers"] =
        serde_json::json!({ shown_q.as_str().unwrap(): format!("{} reply", fx::MARKER) });
    refused(&env, &answer_with(&env, &q, &lines), 0, check);
}

const Q1: &str = "ZQX-MARKER caf\u{e9} question: which way?";
const LABELS: [&str; 3] = ["ZQX-MARKER alpha", "ZQX-MARKER beta", "ZQX-MARKER gamma"];

#[test]
fn the_unchanged_byte_fixture_is_accepted() {
    byte_case(Q1, &LABELS, |_| {}, "none");
}

#[test]
fn a_question_text_with_one_byte_changed_is_refused() {
    byte_case(
        Q1,
        &LABELS,
        |q| q["question"] = Value::String(Q1.replace("way?", "way!")),
        "question-text",
    );
}

#[test]
fn a_question_text_with_trailing_whitespace_is_refused() {
    byte_case(
        Q1,
        &LABELS,
        |q| q["question"] = Value::String(format!("{Q1} ")),
        "question-text",
    );
}

#[test]
fn a_question_text_differing_only_in_unicode_normalization_is_refused() {
    byte_case(
        Q1,
        &LABELS,
        |q| q["question"] = Value::String(Q1.replace('\u{e9}', "e\u{301}")),
        "question-text",
    );
}

#[test]
fn an_option_label_with_one_byte_changed_is_refused() {
    byte_case(
        Q1,
        &LABELS,
        |q| q["options"][1]["label"] = Value::String("ZQX-MARKER betb".into()),
        "options",
    );
}

#[test]
fn an_option_label_differing_only_in_case_is_refused() {
    byte_case(
        Q1,
        &LABELS,
        |q| q["options"][1]["label"] = Value::String("ZQX-MARKER Beta".into()),
        "options",
    );
}

#[test]
fn a_dropped_option_is_refused() {
    byte_case(
        Q1,
        &LABELS,
        |q| {
            q["options"].as_array_mut().unwrap().pop();
        },
        "options",
    );
}

#[test]
fn an_added_option_is_refused() {
    byte_case(
        Q1,
        &LABELS,
        |q| {
            q["options"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({"label": "ZQX-MARKER delta", "description": "x"}));
        },
        "options",
    );
}

#[test]
fn two_swapped_options_are_refused() {
    byte_case(
        Q1,
        &LABELS,
        |q| q["options"].as_array_mut().unwrap().swap(0, 1),
        "options",
    );
}

#[test]
fn a_call_in_another_harness_sessions_record_is_refused() {
    let env = Env::new();
    let lines = fx::lines("accepted");
    let q = env.raise_shown(&lines, 1);
    // The matching call exists only in harness B's record; the answer runs
    // in harness A.
    fx::install(
        &env.home(),
        "proj",
        OTHER_HARNESS,
        &fx::render(&lines, OTHER_HARNESS, &[&q.header]),
    );
    let out = env.answer(
        &q,
        Some(HARNESS),
        "person:owner",
        Some(&pointer(OTHER_HARNESS, fx::CALL, 1)),
        None,
    );
    refused(&env, &out, 0, "session");
    // Naming its own harness session with B's call id finds no record.
    let out = env.answer(
        &q,
        Some(HARNESS),
        "person:owner",
        Some(&pointer(HARNESS, fx::CALL, 1)),
        None,
    );
    refused(&env, &out, 0, "record");
    // With a record of its own that lacks the call, the call isn't found.
    fx::install(
        &env.home(),
        "proj",
        HARNESS,
        &fx::render(&fx::with_call(&lines, "toolu_mine"), HARNESS, &[&q.header]),
    );
    let out = env.answer(
        &q,
        Some(HARNESS),
        "person:owner",
        Some(&pointer(HARNESS, fx::CALL, 1)),
        None,
    );
    refused(&env, &out, 0, "call");
    // No harness session at all.
    let out = env.answer(
        &q,
        None,
        "person:owner",
        Some(&pointer(OTHER_HARNESS, fx::CALL, 1)),
        None,
    );
    refused(&env, &out, 0, "session");
}

#[test]
fn a_session_decider_handing_in_a_pointer_is_refused() {
    let env = Env::new();
    let lines = fx::lines("accepted");
    let q = env.raise_shown(&lines, 1);
    fx::install(
        &env.home(),
        "proj",
        HARNESS,
        &fx::render(&lines, HARNESS, &[&q.header]),
    );
    let out = env.answer(
        &q,
        Some(HARNESS),
        "session",
        Some(&pointer(HARNESS, fx::CALL, 1)),
        Some("x"),
    );
    assert_eq!(out.code, 2, "{}", out.all());
    assert_eq!(env.entries(), 0);
    assert!(out.json()["error"].as_str().unwrap().contains("--prompt"));
    assert!(!out.all().contains(fx::MARKER));
}

#[test]
fn a_handle_other_than_owner_reads_as_that_handles_answer() {
    let env = Env::new();
    let lines = fx::lines("accepted");
    let q = env.raise_shown(&lines, 1);
    fx::install(
        &env.home(),
        "proj",
        HARNESS,
        &fx::render(&lines, HARNESS, &[&q.header]),
    );
    let out = env.answer(
        &q,
        Some(HARNESS),
        "person:dana",
        Some(&pointer(HARNESS, fx::CALL, 1)),
        None,
    );
    assert_eq!(out.code, 0, "{}", out.all());
    let d = &out.json()["decision"];
    assert_eq!(d["decider"]["handle"]["value"], "dana");
    assert_eq!(d["decider"]["handle"]["source"], "handed");
}

#[test]
fn two_answers_racing_on_one_call_position_leave_one_entry() {
    let env = Env::new();
    let base = fx::lines("accepted");
    for i in 0..20 {
        let call = format!("toolu_ZQXMARKERrace{i:04}");
        let lines = fx::with_call(&base, &call);
        let q = env.raise_shown(&lines, 1);
        fx::install(
            &env.home(),
            "proj",
            HARNESS,
            &fx::render(&lines, HARNESS, &[&q.header]),
        );
        let before = env.entries();
        let p = pointer(HARNESS, &call, 1);
        let args = [
            "decisions",
            "answer",
            RAISER,
            &q.id,
            "--in",
            ANSWERER,
            "--by",
            "person:owner",
            "--prompt",
            &p,
        ];
        let a = env.command(Some(HARNESS), &args).output_spawned();
        let b = env.command(Some(HARNESS), &args).output_spawned();
        let (a, b) = (a.wait_with_output().unwrap(), b.wait_with_output().unwrap());
        let codes = [a.status.code(), b.status.code()];
        assert!(
            codes.contains(&Some(0)) && codes.contains(&Some(2)),
            "round {i}: {codes:?}\n{}\n{}",
            String::from_utf8_lossy(&a.stdout),
            String::from_utf8_lossy(&b.stdout)
        );
        let loser = if a.status.code() == Some(2) { &a } else { &b };
        let text = String::from_utf8_lossy(&loser.stdout);
        assert!(
            text.contains("'call-position-reused' check"),
            "round {i}: {text}"
        );
        assert!(!text.contains(fx::MARKER));
        assert_eq!(env.entries(), before + 1, "round {i}");
    }
}

type LineEdit = Box<dyn Fn(&mut Vec<Value>)>;

trait Spawn {
    fn output_spawned(&mut self) -> std::process::Child;
}

impl Spawn for std::process::Command {
    fn output_spawned(&mut self) -> std::process::Child {
        self.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    }
}

#[test]
fn a_result_not_from_the_user_is_refused() {
    // A denied call: the captured denial's structure.
    let env = Env::new();
    let lines = fx::lines("denied");
    let q = env.raise_shown(&lines, 1);
    fx::install(
        &env.home(),
        "proj",
        HARNESS,
        &fx::render(&lines, HARNESS, &[&q.header, "#x"]),
    );
    let out = env.answer(
        &q,
        Some(HARNESS),
        "person:owner",
        Some(&pointer(HARNESS, fx::CALL, 1)),
        None,
    );
    refused(&env, &out, 0, "result");

    // An accepted shape whose permission came from something other than
    // the user's reply.
    let env = Env::new();
    let mut lines = fx::lines("accepted");
    lines[1]["permissionDecision"]["source"] = Value::String("hook".into());
    let q = env.raise_shown(&lines, 1);
    refused(&env, &answer_with(&env, &q, &lines), 0, "result");

    // An errored result.
    let env = Env::new();
    let mut lines = fx::lines("accepted");
    lines[1]["message"]["content"][0]["is_error"] = Value::Bool(true);
    let q = env.raise_shown(&lines, 1);
    refused(&env, &answer_with(&env, &q, &lines), 0, "result");
}

#[test]
fn an_unrecognized_record_shape_is_refused() {
    let cases: Vec<(&str, LineEdit)> = vec![
        (
            "a renamed field in the call",
            Box::new(|l: &mut Vec<Value>| {
                let input = l[0]["message"]["content"][0]["input"]
                    .as_object_mut()
                    .unwrap();
                let qs = input.remove("questions").unwrap();
                input.insert("qs".into(), qs);
            }),
        ),
        (
            "a missing field in the result",
            Box::new(|l: &mut Vec<Value>| {
                l[1]["toolUseResult"]
                    .as_object_mut()
                    .unwrap()
                    .remove("answers");
            }),
        ),
        (
            "a result with no per-question answer",
            Box::new(|l: &mut Vec<Value>| {
                l[1]["toolUseResult"]["answers"] = serde_json::json!({});
            }),
        ),
        (
            "a missing header",
            Box::new(|l: &mut Vec<Value>| {
                fx::questions_mut(l)[0]
                    .as_object_mut()
                    .unwrap()
                    .remove("header");
            }),
        ),
    ];
    for (what, edit) in cases {
        let env = Env::new();
        let mut lines = fx::lines("accepted");
        let q = env.raise_shown(&lines, 1);
        edit(&mut lines);
        let out = answer_with(&env, &q, &lines);
        assert!(out.code == 2, "{what}: {}", out.all());
        refused(&env, &out, 0, "shape");
    }

    // A line that is not valid JSON where the call should be.
    let env = Env::new();
    let lines = fx::lines("accepted");
    let q = env.raise_shown(&lines, 1);
    let text = fx::render(&lines, HARNESS, &[&q.header]);
    let mut broken: Vec<String> = text.lines().map(str::to_string).collect();
    broken[0] = format!(
        "{{\"type\":\"assistant\",\"id\":\"{}\", {} not json",
        fx::CALL,
        fx::MARKER
    );
    fx::install(&env.home(), "proj", HARNESS, &(broken.join("\n") + "\n"));
    let out = env.answer(
        &q,
        Some(HARNESS),
        "person:owner",
        Some(&pointer(HARNESS, fx::CALL, 1)),
        None,
    );
    refused(&env, &out, 0, "shape");
}

#[test]
fn a_person_answer_with_no_evidence_is_refused() {
    let env = Env::new();
    let lines = fx::lines("accepted");
    let q = env.raise_shown(&lines, 1);
    let out = env.answer(&q, Some(HARNESS), "person:owner", None, Some("yes"));
    assert_eq!(out.code, 2, "{}", out.all());
    assert_eq!(env.entries(), 0);
    assert!(out.json()["error"]
        .as_str()
        .unwrap()
        .contains("a person's answer needs the question-prompt evidence"));
    // A session's answer with no pointer is still accepted.
    let out = env.answer(&q, Some(HARNESS), "session", None, Some("yes"));
    assert_eq!(out.code, 0, "{}", out.all());
}

#[test]
fn no_caller_input_produces_the_mark() {
    let env = Env::new();
    let lines = fx::lines("accepted");
    let q = env.raise_shown(&lines, 1);
    fx::install(
        &env.home(),
        "proj",
        HARNESS,
        &fx::render(&lines, HARNESS, &[&q.header]),
    );
    // A claim of verification in the caller's text, with a pointer that
    // fails a check, is refused.
    let out = env.answer(
        &q,
        Some(HARNESS),
        "person:owner",
        Some(&pointer(HARNESS, fx::CALL, 3)),
        Some("verified"),
    );
    refused(&env, &out, 0, "position");
    let out = env.run_h(
        Some(HARNESS),
        &[
            "decisions",
            "answer",
            RAISER,
            &q.id,
            "--in",
            ANSWERER,
            "--by",
            "session",
            "--text",
            "verified",
            "--reason",
            "verified",
        ],
    );
    assert_eq!(out.code, 0, "{}", out.all());
    assert!(out.json()["decision"].get("prompt").is_none());
}

#[test]
fn reading_a_verified_entry_does_not_open_the_record() {
    let env = Env::new();
    let lines = fx::lines("accepted");
    let q = env.raise_shown(&lines, 1);
    let out = answer_with(&env, &q, &lines);
    assert_eq!(out.code, 0, "{}", out.all());
    let id = out.json()["id"].as_str().unwrap().to_string();
    std::fs::remove_file(fx::record_path(&env.home(), "proj", HARNESS)).unwrap();
    let shown = env.ok(&["decisions", "show", ANSWERER, &id]).json();
    assert_eq!(shown["reading"], "verified");
}

#[test]
fn a_question_without_stored_content_is_refused() {
    let env = Env::new();
    let lines = fx::lines("accepted");
    // A question raised with no packet at all.
    let out = env
        .ok(&[
            "decisions",
            "record",
            RAISER,
            "--kind",
            "escalate",
            "--outcome",
            "escalated",
            "--subject",
            "question=q",
            "--subject",
            "text=t",
            "--subject",
            "recipient=person:owner",
            "--by",
            "session",
        ])
        .json();
    let q = Q {
        id: out["id"].as_str().unwrap().to_string(),
        header: "#00000000000".to_string(),
    };
    refused(&env, &answer_with(&env, &q, &lines), 0, "content");
}
