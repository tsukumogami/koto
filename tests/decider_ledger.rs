//! The decider ledger (`~/.koto/_decider_ledger.jsonl`), written by real
//! `koto next` runs against the `std::net` stub.
//!
//! docs/designs/current/DESIGN-jev-decision-offload.md, Decision 4. Every opted-in
//! command points `KOTO_DECIDER_ENDPOINT` at the stub and sets
//! `KOTO_DECIDER` and the key explicitly, and every harness sets `HOME` to
//! a temp directory, so no test writes to a developer's real `~/.koto`.

#[path = "support/decider_stub.rs"]
mod decider_stub;

#[path = "support/decider_session.rs"]
mod decider_session;

use std::path::{Path, PathBuf};
use std::process::Output;

use decider_session::*;
use decider_stub::Reply;
use serde_json::{json, Value};

const MAX_LINE: usize = 4096;
const WARNING: &str = "warning: decider ledger write failed (";

fn go() -> Reply {
    verdict(0.95, 0.03, 0.02)
}

fn ready(tpl: &str, replies: Vec<Reply>) -> Harness {
    let h = Harness::new(tpl);
    h.ready();
    for r in replies {
        h.stub.push(r);
    }
    h
}

fn ledger_file(h: &Harness) -> PathBuf {
    h.home().join(".koto").join("_decider_ledger.jsonl")
}

/// Every ledger line, parsed. Each must end in a newline and fit the bound.
fn ledger(h: &Harness) -> Vec<Value> {
    read_ledger(&ledger_file(h))
}

fn read_ledger(path: &Path) -> Vec<Value> {
    let Ok(body) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    assert!(body.is_empty() || body.ends_with('\n'), "{}", body);
    body.lines()
        .map(|l| {
            assert!(l.len() < MAX_LINE, "line over the bound: {} bytes", l.len());
            serde_json::from_str(l).unwrap_or_else(|e| panic!("{}: {}", e, l))
        })
        .collect()
}

fn of_kind(lines: &[Value], kind: &str) -> Vec<Value> {
    lines
        .iter()
        .filter(|l| l["kind"] == kind)
        .cloned()
        .collect()
}

fn header(h: &Harness) -> Value {
    let body = h.raw_log();
    serde_json::from_str(body.lines().next().unwrap()).unwrap()
}

fn ok(out: &Output) {
    assert!(out.status.success(), "{}", describe(out));
}

fn warnings(out: &Output) -> Vec<String> {
    stderr(out)
        .lines()
        .filter(|l| l.contains(WARNING))
        .map(str::to_string)
        .collect()
}

// ---------------------------------------------------------------------------
// Pairing in one session
// ---------------------------------------------------------------------------

#[test]
fn a_shadow_consultation_and_the_agent_answer_pair_on_session_id_and_visit_seq() {
    let h = ready(&standard("shadow", "shadow"), vec![go()]);
    let schema_before = header(&h)["schema_version"].clone();
    ok(&h.next_mode("shadow"));
    ok(&h.next_with("shadow", r#"{"verdict": "exit"}"#));

    let lines = ledger(&h);
    let consulted = of_kind(&lines, "consulted");
    let answered = of_kind(&lines, "answered");
    assert_eq!(consulted.len(), 1, "{:?}", lines);
    assert_eq!(answered.len(), 1, "{:?}", lines);
    let (c, a) = (&consulted[0], &answered[0]);

    let hdr = header(&h);
    let sid = hdr["session_id"].as_str().unwrap();
    assert!(!sid.is_empty());
    assert_eq!(c["session_id"], sid);
    assert_eq!(a["session_id"], sid);
    assert_eq!(c["visit_seq"], a["visit_seq"]);
    assert_eq!(c["session"], WF);
    assert_eq!(a["session"], WF);
    for line in [c, a] {
        assert_eq!(line["v"], 1);
        assert!(line["at"].as_str().unwrap().ends_with('Z'));
    }

    // The consulted record mirrors the event.
    let event = &h.consultations()[0];
    for key in [
        "state",
        "visit_seq",
        "provider",
        "model",
        "input_sha256",
        "outcome",
        "latency_ms",
        "directive_bytes",
        "endpoint_origin",
        "fields",
    ] {
        assert_eq!(c[key], event[key], "{}", key);
    }
    assert_eq!(c["outcome"], "not_applied");
    assert_eq!(c["endpoint_origin"], "env");
    assert_eq!(c["input_sha256"].as_str().unwrap().len(), 64);
    assert!(c["directive_bytes"].as_u64().unwrap() > 0);
    assert!(c["latency_ms"].is_u64());
    let f = &c["fields"]["verdict"];
    assert_eq!(f["declaration_hash"].as_str().unwrap().len(), 64);
    assert_eq!(f["modes"], json!({"proceed": "shadow", "exit": "shadow"}));
    assert_eq!(f["winning"], "proceed");
    assert!(c.get("trimmed").is_none());

    assert_eq!(a["state"], "review");
    assert_eq!(a["values"], json!({"verdict": "exit"}));

    // The header's schema_version is untouched.
    assert_eq!(hdr["schema_version"], schema_before);
}

#[test]
fn every_outcome_writes_exactly_one_consulted_line() {
    let unset = standard("auto", "never")
        .replace(
            "- {var: PLAN_DOC, label: plan_path}",
            "- {var: PLAN_DOC, label: plan_path}\n            - {context: notes.md, label: notes}",
        )
        .replace(
            "  rethink:\n    accepts:",
            "  rethink:\n    gates:\n      notes:\n        type: context-exists\n        key: notes.md\n    accepts:",
        );
    let cases: Vec<(&str, String, Option<Reply>)> = vec![
        ("applied", standard("auto", "never"), Some(go())),
        (
            "not_applied",
            standard("auto", "never"),
            Some(verdict(0.6, 0.3, 0.1)),
        ),
        ("input_unavailable", unset, None),
        ("error", standard("auto", "never"), Some(Reply::status(500))),
    ];
    for (outcome, tpl, reply) in cases {
        let h = ready(&tpl, reply.into_iter().collect());
        ok(&h.next_mode("auto"));
        let lines = ledger(&h);
        assert_eq!(lines.len(), 1, "{}: {:?}", outcome, lines);
        assert_eq!(lines[0]["kind"], "consulted", "{}", outcome);
        assert_eq!(lines[0]["outcome"], outcome);
        assert_eq!(lines[0]["visit_seq"], h.consultations()[0]["visit_seq"]);
    }
}

#[test]
fn no_answered_record_without_an_unapplied_consultation_or_a_declared_field() {
    // No consultation: the user isn't opted in.
    let h = ready(&standard("shadow", "shadow"), vec![]);
    ok(&h.next_mode("off"));
    ok(&h.next_with("off", r#"{"verdict": "exit"}"#));
    assert!(!ledger_file(&h).exists());

    // Applied: the decider's evidence moved the session on; the agent's
    // next submission is on another state.
    let h = ready(&standard("auto", "never"), vec![go()]);
    ok(&h.next_mode("auto"));
    ok(&h.next_with("auto", r#"{"done": true}"#));
    let lines = ledger(&h);
    assert_eq!(lines.len(), 1, "{:?}", lines);
    assert_eq!(lines[0]["outcome"], "applied");

    // Not applied, but the submission holds no declared field.
    let tpl = standard("shadow", "shadow")
        .replace(
            "        required: true\n        description: Is the outline item clear",
            "        required: false\n        description: Is the outline item clear",
        )
        .replace(
            "    transitions:\n      - target: work\n",
            "      note:\n        type: string\n        required: false\n        description: A note\n    transitions:\n      - target: work\n",
        );
    let h = ready(&tpl, vec![go()]);
    ok(&h.next_mode("shadow"));
    ok(&h.next_with("shadow", r#"{"note": "looked at it"}"#));
    assert!(of_kind(&ledger(&h), "answered").is_empty());
    assert_eq!(of_kind(&ledger(&h), "consulted").len(), 1);

    // And with the declared field alongside, only the declared one lands.
    ok(&h.next_with(
        "shadow",
        r#"{"note": "free text 9d1e", "verdict": "proceed"}"#,
    ));
    let answered = of_kind(&ledger(&h), "answered");
    assert_eq!(answered.len(), 1);
    assert_eq!(answered[0]["values"], json!({"verdict": "proceed"}));
    assert!(!std::fs::read_to_string(ledger_file(&h))
        .unwrap()
        .contains("9d1e"));
}

#[test]
fn two_submissions_in_one_visit_each_write_an_answered_record() {
    // `proceed` also needs FLAG set, and FLAG is empty, so an agent's
    // `proceed` matches no transition and the session stays on `review`.
    let tpl = standard("shadow", "shadow")
        .replace(
            "variables:\n  PLAN_DOC:",
            "variables:\n  FLAG:\n    description: flag\n    default: \"\"\n  PLAN_DOC:",
        )
        .replace(
            "      - target: work\n        when:\n          verdict: proceed\n",
            "      - target: work\n        when:\n          verdict: proceed\n          vars.FLAG: {is_set: true}\n",
        );
    let h = ready(&tpl, vec![go()]);
    ok(&h.next_mode("shadow"));
    ok(&h.next_with("shadow", r#"{"verdict": "proceed"}"#));
    ok(&h.next_with("shadow", r#"{"verdict": "exit"}"#));
    assert_eq!(h.stub.request_count(), 1);

    let lines = ledger(&h);
    let c = of_kind(&lines, "consulted");
    let a = of_kind(&lines, "answered");
    assert_eq!(c.len(), 1);
    assert_eq!(a.len(), 2, "{:?}", lines);
    assert_eq!(a[0]["visit_seq"], c[0]["visit_seq"]);
    assert_eq!(a[1]["visit_seq"], c[0]["visit_seq"]);
    assert_eq!(a[0]["values"]["verdict"], "proceed");
    assert_eq!(a[1]["values"]["verdict"], "exit");
}

#[test]
fn the_answer_is_recorded_after_the_decider_is_switched_off() {
    let h = ready(&standard("shadow", "shadow"), vec![go()]);
    ok(&h.next_mode("shadow"));
    // No KOTO_DECIDER at all on the answering command.
    let mut cmd = h.koto();
    cmd.args(["next", WF, "--with-data", r#"{"verdict": "exit"}"#]);
    ok(&cmd.output().unwrap());
    let lines = ledger(&h);
    assert_eq!(of_kind(&lines, "answered").len(), 1, "{:?}", lines);
}

#[test]
fn an_empty_header_session_id_records_null_and_changes_nothing_else() {
    let control = ready(&standard("shadow", "shadow"), vec![go()]);
    let control_first = json_out(&control.next_mode("shadow"));
    let control_second = json_out(&control.next_with("shadow", r#"{"verdict": "exit"}"#));

    let h = ready(&standard("shadow", "shadow"), vec![go()]);
    // Blank the header's session_id, as an older log would have it.
    let body = h.raw_log();
    let mut lines: Vec<String> = body.lines().map(str::to_string).collect();
    let mut hdr: Value = serde_json::from_str(&lines[0]).unwrap();
    hdr["session_id"] = json!("");
    lines[0] = hdr.to_string();
    std::fs::write(h.state_path(), lines.join("\n") + "\n").unwrap();

    let first = json_out(&h.next_mode("shadow"));
    let second = json_out(&h.next_with("shadow", r#"{"verdict": "exit"}"#));
    assert_eq!(first, control_first);
    assert_eq!(second, control_second);

    let mine = h.consultations();
    let theirs = control.consultations();
    assert_eq!(mine.len(), 1);
    for key in ["state", "visit_seq", "outcome", "fields", "input_sha256"] {
        assert_eq!(mine[0][key], theirs[0][key], "{}", key);
    }

    let lines = ledger(&h);
    assert_eq!(lines.len(), 2, "{:?}", lines);
    for l in &lines {
        assert!(l.as_object().unwrap().contains_key("session_id"), "{}", l);
        assert!(l["session_id"].is_null(), "{}", l);
    }
}

// ---------------------------------------------------------------------------
// Size and content
// ---------------------------------------------------------------------------

/// A template whose one declared field has `n` values, each named with a
/// 60-character prefix, so even a trimmed `consulted` line is over 4 KiB.
fn wide_template(n: usize) -> (String, Vec<String>) {
    let names: Vec<String> = (0..n)
        .map(|i| format!("{}{:03}", "v".repeat(60), i))
        .collect();
    let answers: String = names
        .iter()
        .map(|v| {
            format!(
                "            {}: {{description: \"Value {}.\", mode: shadow}}\n",
                v, v
            )
        })
        .collect();
    let tpl = format!(
        r#"---
name: wide
version: "1.0"
initial_state: review
variables:
  PLAN_DOC:
    description: plan
    default: docs/plan-7c1e.md
states:
  review:
    accepts:
      verdict:
        type: enum
        values: [{values}]
        required: true
        description: Which one?
        decider:
          answers:
{answers}          escape: {{value: unclear, description: "Unknown."}}
          inputs:
            - {{var: PLAN_DOC, label: plan_path}}
    transitions:
      - target: done
        when:
          verdict: {first}
  done:
    terminal: true
---

## review

Pick one.

## done

d
"#,
        values = names.join(", "),
        answers = answers,
        first = names[0],
    );
    (tpl, names)
}

#[test]
fn a_line_over_the_bound_even_trimmed_is_skipped_with_a_warning() {
    let (tpl, names) = wide_template(80);
    let h = Harness::new(&tpl);
    h.init();
    let mut probs = serde_json::Map::new();
    for (i, n) in names.iter().enumerate() {
        probs.insert(n.clone(), json!(if i == 0 { 0.5 } else { 0.005 }));
    }
    probs.insert("unclear".to_string(), json!(0.105));
    h.stub.push(answers(
        json!({"verdict": {"type": "choice", "probabilities": probs}}),
    ));
    let out = h.next_mode("shadow");
    ok(&out);
    assert_eq!(h.stub.request_count(), 1);

    let w = warnings(&out);
    assert_eq!(w.len(), 1, "{}", stderr(&out));
    assert!(
        w[0].contains(&ledger_file(&h).display().to_string()),
        "{}",
        w[0]
    );
    assert!(ledger(&h).is_empty());

    // The session log still carries every probability.
    let c = h.consultations();
    assert_eq!(c.len(), 1);
    let p = c[0]["fields"]["verdict"]["probabilities"]
        .as_object()
        .unwrap();
    assert_eq!(p.len(), names.len() + 1);
}

#[test]
fn no_input_string_or_key_reaches_the_log_or_the_ledger() {
    // Context inputs only, so the log's own variables block holds no input.
    let tpl =
        standard("auto", "never").replace("            - {var: PLAN_DOC, label: plan_path}\n", "");
    for reply in [
        verdict(0.6, 0.3, 0.1),
        go(),
        Reply::status(401),
        Reply::status(500),
    ] {
        let h = ready(&tpl, vec![reply]);
        let first = h.next_mode("auto");
        ok(&first);
        // Answer if the decider didn't.
        if json_out(&first)["state"] == "review" {
            ok(&h.next_with("auto", r#"{"verdict": "exit"}"#));
        }
        let req = h.stub.last_request().unwrap();
        let inputs: Vec<String> = req.json()["state"]
            .as_object()
            .unwrap()
            .values()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(inputs.iter().any(|i| i == OUTLINE));

        let log = h.raw_log();
        let ledger_body = std::fs::read_to_string(ledger_file(&h)).unwrap();
        for text in inputs.iter().map(String::as_str).chain([KEY]) {
            assert!(!log.contains(text), "session log carries {:?}", text);
            assert!(!ledger_body.contains(text), "ledger carries {:?}", text);
        }
        assert!(!ledger(&h).is_empty());
    }
}

// ---------------------------------------------------------------------------
// Failure tolerance
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn running_as_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

#[cfg(unix)]
#[test]
fn a_read_only_ledger_warns_once_and_changes_nothing_else() {
    use std::os::unix::fs::PermissionsExt;
    if running_as_root() {
        return; // root writes through a 0400 file
    }
    let control = ready(&standard("shadow", "shadow"), vec![go()]);
    let control_out = control.next_mode("shadow");
    ok(&control_out);
    assert!(
        warnings(&control_out).is_empty(),
        "{}",
        stderr(&control_out)
    );

    let h = ready(&standard("shadow", "shadow"), vec![go()]);
    let path = ledger_file(&h);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();

    let out = h.next_mode("shadow");
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert_eq!(json_out(&out), json_out(&control_out));
    let w = warnings(&out);
    assert_eq!(w.len(), 1, "{}", stderr(&out));
    assert!(w[0].contains(&path.display().to_string()), "{}", w[0]);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
    assert_eq!(h.consultations().len(), 1);
}

#[cfg(unix)]
#[test]
fn an_uncreatable_koto_dir_warns_and_changes_nothing_else() {
    use std::os::unix::fs::PermissionsExt;
    if running_as_root() {
        return;
    }
    let control = ready(&standard("shadow", "shadow"), vec![go()]);
    let control_out = control.next_mode("shadow");
    ok(&control_out);

    let h = ready(&standard("shadow", "shadow"), vec![go()]);
    let home = h.home();
    assert!(!home.join(".koto").exists());
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o500)).unwrap();
    let out = h.next_mode("shadow");
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();

    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert_eq!(json_out(&out), json_out(&control_out));
    assert_eq!(warnings(&out).len(), 1, "{}", stderr(&out));
    assert!(!home.join(".koto").exists());
    assert_eq!(h.consultations().len(), 1);
}

// ---------------------------------------------------------------------------
// Survival across child cleanup
// ---------------------------------------------------------------------------

const PARENT: &str = r#"---
name: batch-parent
version: "1.0"
initial_state: plan
states:
  plan:
    accepts:
      tasks:
        type: tasks
        required: true
      finalize:
        type: enum
        required: false
        values: [yes]
    gates:
      done:
        type: children-complete
    materialize_children:
      from_field: tasks
      default_template: child.md
    transitions:
      - target: summarize
        when:
          finalize: yes
  summarize:
    terminal: true
---

## plan

Plan the batch.

## summarize

Summarize results.
"#;

const CHILD: &str = r#"---
name: batch-child
version: "1.0"
initial_state: review
variables:
  PLAN_DOC:
    description: plan
    default: docs/plan-7c1e.md
states:
  review:
    accepts:
      verdict:
        type: enum
        values: [proceed, exit]
        required: true
        description: Is the task clear?
        decider:
          answers:
            proceed: {description: "Clear.", mode: auto}
            exit: {description: "Vague.", mode: never}
          escape: {value: unclear, description: "Cannot tell."}
          inputs:
            - {var: PLAN_DOC, label: plan_path}
    transitions:
      - target: work
        when:
          verdict: proceed
      - target: work
        when:
          verdict: exit
  work:
    accepts:
      finished:
        type: boolean
        required: true
        description: Is the work finished?
    transitions:
      - target: done
        when:
          finished: true
  done:
    terminal: true
---

## review

Review the task.

## work

Do the task.

## done

Done.
"#;

#[test]
fn child_records_survive_child_cleanup() {
    let h = Harness::new(PARENT);
    std::fs::write(h.dir.join("child.md"), CHILD).unwrap();
    let run = |mode: &str, args: &[&str]| {
        let mut cmd = h.koto_mode(mode);
        cmd.args(args);
        let out = cmd.output().unwrap();
        ok(&out);
        out
    };
    run("off", &["init", "parent", "--template", "template.md"]);
    let tasks = json!({"tasks": [
        {"name": "A", "waits_on": [], "vars": {}},
        {"name": "B", "waits_on": [], "vars": {}},
    ]});
    run(
        "off",
        &["next", "parent", "--with-data", &tasks.to_string()],
    );
    let sessions = h.dir.join("sessions");
    assert!(sessions.join("parent.A").exists());
    assert!(sessions.join("parent.B").exists());

    // A: shadow, so the agent answers the consulted visit.
    h.stub.push(go());
    run("shadow", &["next", "parent.A"]);
    run(
        "shadow",
        &["next", "parent.A", "--with-data", r#"{"verdict": "exit"}"#],
    );
    // B: auto applies, so the agent never answers the consulted visit.
    h.stub.push(go());
    run("auto", &["next", "parent.B"]);
    assert_eq!(h.stub.request_count(), 2);
    for child in ["parent.A", "parent.B"] {
        run(
            "auto",
            &["next", child, "--with-data", r#"{"finished": true}"#],
        );
    }

    let mut finalize = tasks.clone();
    finalize["finalize"] = json!("yes");
    let out = run(
        "off",
        &["next", "parent", "--with-data", &finalize.to_string()],
    );
    assert_eq!(json_out(&out)["state"], "summarize", "{}", describe(&out));
    assert!(!sessions.join("parent.A").exists());
    assert!(!sessions.join("parent.B").exists());

    let lines = ledger(&h);
    let consulted = of_kind(&lines, "consulted");
    let answered = of_kind(&lines, "answered");
    assert_eq!(consulted.len(), 2, "{:?}", lines);
    assert_eq!(answered.len(), 1, "{:?}", lines);
    let a = consulted
        .iter()
        .find(|c| c["session"] == "parent.A")
        .unwrap();
    let b = consulted
        .iter()
        .find(|c| c["session"] == "parent.B")
        .unwrap();
    assert_eq!(a["outcome"], "not_applied");
    assert_eq!(b["outcome"], "applied");
    assert_ne!(a["session_id"], b["session_id"]);
    assert!(a["session_id"].is_string() && b["session_id"].is_string());
    assert_eq!(answered[0]["session"], "parent.A");
    assert_eq!(answered[0]["session_id"], a["session_id"]);
    assert_eq!(answered[0]["visit_seq"], a["visit_seq"]);
}

// ---------------------------------------------------------------------------
// Session-feed contract
// ---------------------------------------------------------------------------

#[test]
fn validate_feed_accepts_logs_from_consulting_runs() {
    let spec = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/reference/session-feed.md");
    for (mode, reply) in [
        ("shadow", go()),
        ("auto", go()),
        ("auto", Reply::status(500)),
    ] {
        let h = ready(&standard("auto", "never"), vec![reply]);
        ok(&h.next_mode(mode));
        assert_eq!(h.consultations().len(), 1);
        let out = h
            .koto()
            .args(["template", "validate-feed"])
            .arg(h.state_path())
            .env("KOTO_FEED_SPEC", &spec)
            .output()
            .unwrap();
        assert!(out.status.success(), "{} {}", mode, describe(&out));
    }
}
