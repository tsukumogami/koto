//! The `decision` field three events carry when they pass a gate changes
//! nothing koto exports as telemetry.
//!
//! `decider_check_answered`, `gate_override_recorded` and
//! `directed_transition` carry the `gate-pass` decision they commit as
//! `decision` (docs/designs/DESIGN-decision-record.md, Decision 3). The run
//! journal builds its records from named event fields, so the field never
//! reaches it, and the event sink queues a copy of each journal record, so
//! it never reaches the sink either. These tests hold that in place:
//!
//! - One scripted run through the koto binary covers a decider consultation,
//!   a decider answer, a gate override and a directed transition past a gate
//!   that hasn't passed, each of the last three recording a decision with
//!   its event.
//! - That run's own log is replayed twice through `LocalBackend`, the store
//!   the binary writes through, into two scratch stores: once as recorded
//!   and once with `decision` removed from every event that carries it. Both
//!   replays take the same header and the same event timestamps, so the only
//!   value that differs between them is the time each journal line was
//!   written, `at`, which is pinned before comparing. The two journals must
//!   match byte for byte, and the replay as recorded must write the same
//!   records the binary wrote, so the replay is the projection the binary
//!   runs and not a model of it.
//! - A decision's reason stays in the session: the planted reasons are in the
//!   session's decisions and in no journal line (PRD R34).
//! - The event sink's records get the same check once the sink's emit path
//!   (docs/designs/DESIGN-event-sink.md) is on main; until then that test
//!   says why it skipped.
//!
//! Every koto command runs with `HOME` in the test's own temporary
//! directory, `KOTO_SESSIONS_BASE` unset and one fixed
//! `CLAUDE_CODE_SESSION_ID`, so the store and the journal are the test's own
//! and a gate pass has a decider. The decider is the in-process stub, so no
//! test reaches a real provider.

#![cfg(unix)]

#[path = "support/decider_stub.rs"]
mod decider_stub;
#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;

use decider_stub::{DeciderStub, Reply};
use koto::engine::types::{Event, StateFileHeader};
use koto::session::local::LocalBackend;
use koto::session::SessionBackend;
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const WF: &str = "telemetry";
const JOURNAL: &str = "_run_journal.jsonl";

/// The harness session every command runs under, so a gate pass has an
/// established decider.
const HARNESS_SESSION: &str = "0f0e0d0c-1111-4222-8333-444455556666";

/// The key every command that consults the decider sends.
const DECIDER_KEY: &str = "sk-telemetry-identity-key-7a3c";

/// The reasons the run's decisions carry. None may reach a telemetry record.
const OVERRIDE_REASON: &str = "planted-override-reason-7a3c";
const DIRECTED_REASON: &str = "planted-directed-reason-7a3c";
const JUDGMENT_REASON: &str = "planted-judgment-reason-7a3c";
const REASONS: &[&str] = &[OVERRIDE_REASON, DIRECTED_REASON, JUDGMENT_REASON];

/// The events that carry `decision` when they pass a gate.
const DECISION_EVENTS: &[&str] = &[
    "decider_check_answered",
    "gate_override_recorded",
    "directed_transition",
];

/// Where the event sink's emit path lives once it lands
/// (docs/designs/DESIGN-event-sink.md, "Implementation Approach").
const SINK_EMIT_PATH: &str = "src/telemetry/sink.rs";

/// A check that never passes.
const CHECK_SH: &str = "#!/bin/sh\nexit 1\n";

/// `triage` takes evidence the decider answers; `review` holds a decider
/// check with two veto criteria; `check` and `fence` each hold a command
/// check that fails.
const TEMPLATE: &str = r#"---
name: telemetry-identity
version: "1.0"
initial_state: triage
variables:
  PLAN_DOC:
    description: plan
    default: docs/plan.md
states:
  triage:
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
  review:
    gates:
      comments:
        type: decider-check
        command: "cat slice.txt"
        label: comments
        criteria:
          "a":
            rule_ref: "https://example.org/rules/a"
            question: "Does each comment give a reason?"
            pass: "Every comment says why."
            fail: "A comment restates the code."
            escape: "No comment, or can't tell."
            mode: veto
          "b":
            rule_ref: "https://example.org/rules/b"
            question: "Is each comment current?"
            pass: "Every comment matches the code."
            fail: "A comment describes code that changed."
            escape: "No comment, or can't tell."
            mode: veto
    transitions:
      - target: check
  check:
    gates:
      lint:
        type: command
        command: "./check.sh"
    transitions:
      - target: fence
  fence:
    gates:
      tests:
        type: command
        command: "./check.sh"
    transitions:
      - target: done
  done:
    terminal: true
---

## triage

Triage.

## review

Review the comments.

## check

Check.

## fence

Fence.

## done

Done.
"#;

// ----- Harness -----

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Out {
    fn json(&self) -> Value {
        serde_json::from_str(self.stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}\n{}", self.stdout, self.stderr))
    }
}

/// One scratch home, the work directory the session runs in, and the
/// decider stub its commands consult.
struct Env {
    tmp: TempDir,
    home: PathBuf,
    work: PathBuf,
    stub: DeciderStub,
}

impl Env {
    fn new() -> Self {
        Self::in_tmp(TempDir::new().unwrap())
    }

    fn in_tmp(tmp: TempDir) -> Self {
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
        let script = work.join("check.sh");
        std::fs::write(&script, CHECK_SH).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        Env {
            tmp,
            home,
            work,
            stub: DeciderStub::start(),
        }
    }

    fn koto_home(&self) -> PathBuf {
        self.home.join(".koto")
    }

    fn state_path(&self) -> PathBuf {
        self.koto_home()
            .join("sessions")
            .join(WF)
            .join(format!("koto-{WF}.state.jsonl"))
    }

    /// A koto command for this home: none of the developer's variables that
    /// would redirect the store, the config or the decider, the harness
    /// session, and the stub as the decider.
    fn koto(&self, args: &[&str]) -> Out {
        let mut cmd = koto_command::koto_std();
        cmd.current_dir(&self.work)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", self.tmp.path().join("cache"));
        for unset in [
            "KOTO_SESSIONS_BASE",
            "KOTO_HOME",
            "XDG_CONFIG_HOME",
            "KOTO_WORKFLOWS_DIR",
            "KOTO_ANCHOR",
            "FLEET_EVENT_SINK",
        ] {
            cmd.env_remove(unset);
        }
        cmd.env("CLAUDE_CODE_SESSION_ID", HARNESS_SESSION)
            .env("KOTO_DECIDER", "auto")
            .env("KOTO_DECIDER_API_KEY", DECIDER_KEY)
            .env("KOTO_DECIDER_ENDPOINT", self.stub.url());
        let output = cmd.args(args).output().unwrap();
        Out {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        }
    }

    fn ok(&self, args: &[&str]) -> Out {
        let out = self.koto(args);
        assert_eq!(
            out.code,
            Some(0),
            "koto {args:?} failed\nstdout: {}\nstderr: {}",
            out.stdout,
            out.stderr
        );
        out
    }

    /// The raw journal lines of the session, as the binary wrote them.
    fn journal(&self) -> Vec<String> {
        session_lines(&self.koto_home().join(JOURNAL))
    }

    /// The session's log events, header skipped.
    fn events(&self) -> Vec<Value> {
        std::fs::read_to_string(self.state_path())
            .unwrap()
            .lines()
            .skip(1)
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
}

/// A decider answer of `pass` for criterion `rule_id`.
fn pass_answer(rule_id: &str) -> Reply {
    Reply::json(&serde_json::json!({
        "model": "telemetry-identity-1.0.0",
        "answers": {rule_id: {"type": "choice", "choice": "pass",
            "probabilities": {"pass": 0.95, "fail": 0.02, "unclear": 0.03}}},
        "usage": {"input_tokens": 100, "output_tokens": 3}
    }))
}

/// A routing answer of `go`.
fn go_answer() -> Reply {
    Reply::json(&serde_json::json!({
        "model": "telemetry-identity-1.0.0",
        "answers": {"verdict": {"type": "choice", "choice": "go",
            "probabilities": {"go": 0.97, "stop": 0.02, "unclear": 0.01}}},
        "usage": {"input_tokens": 250, "output_tokens": 7}
    }))
}

/// Every decision id in a `koto decisions list --format v1` body.
fn decision_ids(value: &Value, ids: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let (Some(Value::String(id)), Some(_)) = (map.get("id"), map.get("kind")) {
                ids.push(id.clone());
            }
            map.values().for_each(|v| decision_ids(v, ids));
        }
        Value::Array(items) => items.iter().for_each(|v| decision_ids(v, ids)),
        _ => {}
    }
}

/// The scripted run: a judgment with a reason, a decider consultation, a
/// decider answer, a gate override and a directed transition past a gate
/// that hasn't passed, ending at `done` with the session kept.
fn scripted_run(env: &Env) {
    env.ok(&["init", WF, "--template", "template.md"]);
    env.ok(&[
        "decisions",
        "record",
        WF,
        "--kind",
        "judgment",
        "--outcome",
        "settled",
        "--subject",
        "what=telemetry-identity",
        "--reason",
        JUDGMENT_REASON,
    ]);

    // The decider answers `triage`'s evidence with `go` (a consultation),
    // then criterion `a`, and refuses `b`, which is left to the agent.
    env.stub.push(go_answer());
    env.stub.push(pass_answer("a"));
    env.stub.push(Reply::status(401));
    let out = env.ok(&["next", WF, "--no-cleanup"]);
    assert_eq!(out.json()["action"], "gate_blocked", "{}", out.stdout);
    env.ok(&[
        "decider",
        "answer",
        WF,
        "--gate",
        "comments",
        "--criterion",
        "b",
        "--answer",
        "pass",
    ]);

    // The answer passes `comments`; `check`'s command fails.
    let out = env.ok(&["next", WF, "--no-cleanup"]);
    let json = out.json();
    assert_eq!(json["state"], "check", "{}", out.stdout);
    assert_eq!(json["action"], "gate_blocked", "{}", out.stdout);
    env.ok(&[
        "overrides",
        "record",
        WF,
        "--gate",
        "lint",
        "--rationale",
        OVERRIDE_REASON,
    ]);

    // The override moves past `check`; `fence`'s command fails, and a
    // directed transition moves past it.
    let out = env.ok(&["next", WF, "--no-cleanup"]);
    let json = out.json();
    assert_eq!(json["state"], "fence", "{}", out.stdout);
    assert_eq!(json["action"], "gate_blocked", "{}", out.stdout);
    let out = env.ok(&[
        "next",
        WF,
        "--to",
        "done",
        "--rationale",
        DIRECTED_REASON,
        "--no-cleanup",
    ]);
    assert_eq!(out.json()["state"], "done", "{}", out.stdout);
}

/// The three gate-passing events of the run, each carrying its decision.
fn assert_decisions_carried(env: &Env) {
    let events = env.events();
    for kind in DECISION_EVENTS {
        let carried: Vec<&Value> = events
            .iter()
            .filter(|e| e["type"] == *kind)
            .filter(|e| e["payload"]["decision"]["kind"] == "gate-pass")
            .collect();
        assert_eq!(carried.len(), 1, "one {kind} carrying a gate-pass decision");
    }
}

// ----- Reading and replaying -----

/// The lines of the journal at `path` that belong to the session.
fn session_lines(path: &Path) -> Vec<String> {
    let session = format!("\"session\":\"{WF}\"");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains(&session))
        .map(str::to_string)
        .collect()
}

/// `line` with its `at`, the time the journal wrote it, pinned. Every other
/// byte is left as written.
fn pinned(line: &str) -> String {
    const KEY: &str = "\"at\":\"";
    let Some(start) = line.find(KEY).map(|i| i + KEY.len()) else {
        return line.to_string();
    };
    let end = start + line[start..].find('"').expect("a closed at value");
    format!("{}PINNED{}", &line[..start], &line[end..])
}

/// The record's kind, read from its line.
fn kind_of(line: &str) -> String {
    let record: Value = serde_json::from_str(line).unwrap();
    record["kind"].as_str().unwrap().to_string()
}

/// Kinds whose bytes come from the process that writes them rather than from
/// the session's events: the driver, the harness session the binary ran
/// under and the test process doesn't. The replay never compares them with
/// the binary's.
const PROCESS_KINDS: &[&str] = &["driver_seen", "session_started"];

/// Replay the log at `state_path` into a scratch store under `root`, with
/// `decision` removed from every event that carries it when `strip` is set,
/// and return the journal lines that store writes. The events `koto init`
/// wrote (through the first `transitioned`) go in with the session, as the
/// init put them there, and the rest are appended one by one at their
/// recorded timestamps.
fn replay(state_path: &Path, strip: bool, root: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(state_path).unwrap();
    let mut lines = text.lines();
    let header: StateFileHeader = serde_json::from_str(lines.next().unwrap()).unwrap();
    let events: Vec<Event> = lines
        .map(|l| {
            let mut v: Value = serde_json::from_str(l).unwrap();
            let carries = DECISION_EVENTS.contains(&v["type"].as_str().unwrap_or_default());
            if strip && carries {
                v["payload"].as_object_mut().unwrap().remove("decision");
            }
            serde_json::from_value(v).unwrap()
        })
        .collect();
    let init_end = events
        .iter()
        .position(|e| e.event_type == "transitioned")
        .expect("the init's transitioned")
        + 1;
    let backend =
        LocalBackend::with_base_dir_and_journal(root.join("sessions"), root.to_path_buf());
    backend
        .init_state_file(WF, header, events[..init_end].to_vec())
        .unwrap();
    for e in &events[init_end..] {
        backend.append_event(WF, &e.payload, &e.timestamp).unwrap();
    }
    // The replay carried the field exactly when it was meant to.
    let (_, replayed) = backend.read_events(WF).unwrap();
    for e in replayed
        .iter()
        .filter(|e| DECISION_EVENTS.contains(&e.event_type.as_str()))
    {
        let line = serde_json::to_string(&e.payload).unwrap();
        assert_eq!(
            line.contains("\"decision\""),
            !strip,
            "{} in the {} replay: {line}",
            e.event_type,
            if strip { "stripped" } else { "recorded" }
        );
    }
    session_lines(&root.join(JOURNAL))
}

/// The run's log replayed as recorded and without `decision`, each into its
/// own store.
fn both_replays(env: &Env) -> (Vec<String>, Vec<String>) {
    let with_root = env.tmp.path().join("replay-with");
    let without_root = env.tmp.path().join("replay-without");
    (
        replay(&env.state_path(), false, &with_root),
        replay(&env.state_path(), true, &without_root),
    )
}

// ----- The run journal -----

#[test]
fn run_journal_lines_are_byte_identical_with_and_without_the_decision_field() {
    let env = Env::new();
    scripted_run(&env);
    assert_decisions_carried(&env);

    let (with, without) = both_replays(&env);
    assert!(!with.is_empty());
    let with: Vec<String> = with.iter().map(|l| pinned(l)).collect();
    let without: Vec<String> = without.iter().map(|l| pinned(l)).collect();
    assert_eq!(
        with.len(),
        without.len(),
        "with:\n{}\nwithout:\n{}",
        with.join("\n"),
        without.join("\n")
    );
    for (i, (a, b)) in with.iter().zip(&without).enumerate() {
        assert_eq!(a, b, "journal line {i} differs");
    }

    // The comparison covers what the three events project: the decider's
    // consultation, the override and the directed move to `done`.
    let kinds: Vec<String> = with.iter().map(|l| kind_of(l)).collect();
    for kind in ["decider_consulted", "check_overridden", "state_entered"] {
        assert!(kinds.iter().any(|k| k == kind), "no {kind} in {kinds:?}");
    }
    assert!(
        with.iter()
            .any(|l| l.contains("\"kind\":\"state_entered\"")
                && l.contains("\"koto.state\":\"done\"")),
        "no state_entered for done"
    );

    // The replay as recorded is the binary's own projection: every record
    // it writes from the events is a line the binary wrote, in order.
    let binary: Vec<String> = env.journal().iter().map(|l| pinned(l)).collect();
    let mut from = 0;
    for line in with
        .iter()
        .filter(|l| !PROCESS_KINDS.contains(&kind_of(l).as_str()))
    {
        let at = binary[from..]
            .iter()
            .position(|b| b == line)
            .unwrap_or_else(|| {
                panic!(
                    "the replay wrote a line the binary did not:\n{line}\nbinary:\n{}",
                    binary.join("\n")
                )
            });
        from += at + 1;
    }
}

#[test]
fn a_decisions_reason_reaches_no_journal_record() {
    let env = Env::new();
    scripted_run(&env);

    // The reasons are in the session: in the decisions it lists and in the
    // events the journal is built from.
    let list = env.ok(&["decisions", "list", WF, "--format", "v1"]);
    let listed = list.stdout.clone();
    let log = std::fs::read_to_string(env.state_path()).unwrap();
    let mut ids = Vec::new();
    decision_ids(&list.json(), &mut ids);
    assert!(ids.len() >= 4, "four decisions listed: {listed}");
    let mut held = String::new();
    for id in &ids {
        held.push_str(&env.ok(&["decisions", "show", WF, id]).stdout);
    }
    for reason in REASONS {
        assert!(
            listed.contains(reason) || held.contains(reason),
            "the session's decisions do not hold {reason}:\n{listed}\n{held}"
        );
    }
    for reason in [OVERRIDE_REASON, DIRECTED_REASON] {
        assert!(log.contains(reason), "the log does not hold {reason}");
    }

    // None reaches the journal, as the binary wrote it or as either replay
    // writes it.
    let (with, without) = both_replays(&env);
    let all = std::fs::read_to_string(env.koto_home().join(JOURNAL)).unwrap();
    assert!(!all.is_empty());
    for reason in REASONS {
        assert!(!all.contains(reason), "the journal holds {reason}:\n{all}");
        for line in with.iter().chain(&without) {
            assert!(
                !line.contains(reason),
                "a replayed line holds {reason}: {line}"
            );
        }
    }
}

// ----- The event sink -----

/// Whether the event sink's emit path is on this tree.
fn sink_emit_path_present() -> bool {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(SINK_EMIT_PATH)
        .is_file()
}

/// A sink at the fixed path under `home` (`.local/libexec/fleet/event-sink`)
/// that appends every line it reads to the returned log. Every directory is
/// the user's with no group or other write bit, as discovery requires.
fn install_sink(home: &Path) -> PathBuf {
    let dir = home.join(".local/libexec/fleet");
    std::fs::create_dir_all(&dir).unwrap();
    let mut d = dir.as_path();
    while d.starts_with(home) {
        std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)).unwrap();
        match d.parent() {
            Some(p) => d = p,
            None => break,
        }
    }
    let log = dir.join("received.jsonl");
    let sink = dir.join("event-sink");
    std::fs::write(
        &sink,
        format!("#!/bin/sh\ncat >> '{}'\nexit 0\n", log.display()),
    )
    .unwrap();
    std::fs::set_permissions(&sink, std::fs::Permissions::from_mode(0o700)).unwrap();
    log
}

/// Whether the sink's `sent` line is the journal's `record`, or the record
/// with stamps appended after its last field.
fn sent_as(sent: &str, record: &str) -> bool {
    sent == record
        || record
            .strip_suffix('}')
            .is_some_and(|open| sent.starts_with(&format!("{open},")))
}

#[test]
fn records_handed_to_the_event_sink_are_byte_identical_with_and_without_the_decision_field() {
    if !sink_emit_path_present() {
        eprintln!(
            "skipped: the event sink's emit path ({SINK_EMIT_PATH}, \
             docs/designs/DESIGN-event-sink.md) is not on this tree yet, so koto \
             hands no record to a sink; this test runs once it is"
        );
        return;
    }
    // Under the build directory, not the system temporary directory, whose
    // sticky world-writable root would fail the sink's parent check.
    let env = Env::in_tmp(TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).unwrap());
    let received = install_sink(&env.home);
    scripted_run(&env);
    assert_decisions_carried(&env);

    let sent: Vec<String> = std::fs::read_to_string(&received)
        .unwrap_or_default()
        .lines()
        .map(pinned)
        .collect();
    assert!(
        !sent.is_empty(),
        "the sink at {} received nothing; check that the test home passes discovery",
        received.display()
    );

    // Every record the field-free replay journals reached the sink from the
    // run whose events carried the field, unchanged but for its stamps.
    let (_, without) = both_replays(&env);
    for line in without
        .iter()
        .map(|l| pinned(l))
        .filter(|l| !PROCESS_KINDS.contains(&kind_of(l).as_str()))
        .filter(|l| kind_of(l) != "context_reads")
    {
        assert!(
            sent.iter().any(|s| sent_as(s, &line)),
            "the sink was not handed {line}\nsent:\n{}",
            sent.join("\n")
        );
    }
    for reason in REASONS {
        for line in &sent {
            assert!(
                !line.contains(reason),
                "the sink was handed {reason}: {line}"
            );
        }
    }
}
