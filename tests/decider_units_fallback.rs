//! Units a decider check can't grade fall back to the agent one by one, and
//! the agent answers one unit at a time with `koto decider answer --unit`
//! (docs/designs/DESIGN-doc-rule-checks-m4.md, Decisions 1 to 3), driven
//! through `koto next` against the `std::net` stub.

#[path = "support/decider_stub.rs"]
mod decider_stub;
#[path = "support/host_session.rs"]
mod host_session;

#[path = "support/decider_session.rs"]
mod decider_session;
#[path = "support/koto_command.rs"]
mod koto_command;

use decider_session::*;
use decider_stub::Reply;
use serde_json::{json, Value};

const GATE: &str = "ac_binary";
const RULE: &str = "ac_binary";
const SECOND: &str = "ac_single";

/// The byte budget every unit-mode template here declares.
const MAX_BYTES: usize = 64;

/// The key of the `i`-th unit, counting from 1.
fn key(i: usize) -> String {
    format!("ac-{}", i)
}

/// A short unit text that names its unit, well inside [`MAX_BYTES`].
fn text(i: usize) -> String {
    format!("UNIT-TEXT-{:02}: batch {} exports.", i, i)
}

/// A text of exactly `n` bytes that names unit `i`.
fn sized(i: usize, n: usize) -> String {
    let head = format!("UNIT-TEXT-{:02}:", i);
    format!("{}{}", head, "x".repeat(n - head.len()))
}

/// One JSON line per `(key, text)`.
fn jsonl(units: &[(String, String)]) -> String {
    units
        .iter()
        .map(|(k, t)| json!({"key": k, "text": t}).to_string() + "\n")
        .collect()
}

/// `n` units with the default texts, except that each `(unit, size)` in
/// `sizes` gets a text of exactly that many bytes.
fn fixture(n: usize, sizes: &[(usize, usize)]) -> String {
    let units: Vec<(String, String)> = (1..=n)
        .map(|i| {
            let t = sizes
                .iter()
                .find(|(j, _)| *j == i)
                .map(|(_, size)| sized(i, *size))
                .unwrap_or_else(|| text(i));
            (key(i), t)
        })
        .collect();
    jsonl(&units)
}

fn criterion_yaml(id: &str) -> String {
    format!(
        r#"          {id}:
            rule_ref: "https://example.org/rules/{id}"
            question: "Can this acceptance criterion be checked pass or fail with no judgment?"
            pass: "It names one observable outcome a tester can check."
            fail: "It needs judgment, or names no observable outcome."
            escape: "It isn't an acceptance criterion."
            mode: veto
"#
    )
}

/// A one-state template: `review` holds the check over `units.jsonl`, with
/// the given extra check keys and criteria, then goes to `done`.
fn template(extra: &str, criteria: &[&str]) -> String {
    let criteria: String = criteria.iter().map(|c| criterion_yaml(c)).collect();
    template_with(extra, &criteria)
}

/// [`template`] with the criteria given as YAML.
fn template_with(extra: &str, criteria: &str) -> String {
    format!(
        r#"---
name: units-fallback
version: "1.0"
initial_state: review
states:
  review:
    gates:
      {GATE}:
        type: decider-check
        command: "cat units.jsonl"
        label: ac
        max_bytes: {MAX_BYTES}
{extra}        criteria:
{criteria}    transitions:
      - target: done
  done:
    terminal: true
---

## review

Review the acceptance criteria.

## done

Done.
"#
    )
}

fn unit_template() -> String {
    template("        units: jsonl\n", &[RULE])
}

/// The stub's answer to one request about `rule`.
fn reply(rule: &str, pass: f64, fail: f64, unclear: f64) -> Reply {
    Reply::json(&json!({
        "model": "jev-test-1.2.3",
        "answers": {rule: {"type": "choice", "choice": "pass",
            "probabilities": {"pass": pass, "fail": fail, "unclear": unclear}}},
        "usage": {"input_tokens": 100, "output_tokens": 3}
    }))
}

fn pass_of(rule: &str) -> Reply {
    reply(rule, 0.95, 0.02, 0.03)
}

fn pass() -> Reply {
    pass_of(RULE)
}

fn fail() -> Reply {
    reply(RULE, 0.02, 0.95, 0.03)
}

/// A 2xx answer with no probabilities: read as malformed, retried once.
fn malformed() -> Reply {
    Reply::json(&json!({"model": "m", "answers": {RULE: {"type": "choice"}}}))
}

fn harness(tpl: &str, units: &str) -> Harness {
    let h = Harness::new(tpl);
    write_units(&h, units);
    h.init();
    h
}

fn write_units(h: &Harness, units: &str) {
    std::fs::write(h.dir.join("units.jsonl"), units).unwrap();
}

fn run(h: &Harness) -> Value {
    let mut cmd = h.koto_mode("auto");
    cmd.args(["next", WF, "--no-cleanup"]);
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    json_out(&out)
}

fn condition(out: &Value) -> Value {
    out["blocking_conditions"]
        .as_array()
        .and_then(|a| a.iter().find(|c| c["name"] == GATE))
        .cloned()
        .unwrap_or_else(|| panic!("no {} condition in {}", GATE, out))
}

fn findings(out: &Value) -> Vec<Value> {
    condition(out)["failure"]["findings"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn payloads(h: &Harness, ty: &str) -> Vec<Value> {
    h.events_of(ty)
        .into_iter()
        .map(|e| e["payload"].clone())
        .collect()
}

/// The `(unit key, reason)` of every `decider_fallback`, in log order.
fn fallbacks(h: &Harness) -> Vec<(String, String)> {
    payloads(h, "decider_fallback")
        .iter()
        .map(|f| {
            (
                f["unit"]["key"].as_str().unwrap_or("").to_string(),
                f["reason"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// The output's per-unit entry for `k`.
fn unit_entry(out: &Value, k: &str) -> Value {
    condition(out)["output"]["units"]
        .as_array()
        .and_then(|u| u.iter().find(|u| u["key"] == k))
        .cloned()
        .unwrap_or_else(|| panic!("no unit {} in {}", k, out))
}

/// The command between the first pair of backticks of `finding`, with
/// `answer` chosen, split into the words after `koto`.
fn command_of(finding: &Value, answer: &str) -> Vec<String> {
    let msg = finding["message"].as_str().unwrap();
    let line = msg
        .split('`')
        .nth(1)
        .unwrap()
        .replace("<pass|fail|unclear>", answer);
    let words: Vec<String> = line.split_whitespace().map(String::from).collect();
    assert_eq!(&words[..3], &["koto", "decider", "answer"], "{}", line);
    words[1..].to_vec()
}

fn answer_cmd(h: &Harness, args: &[&str]) -> std::process::Output {
    let mut cmd = h.koto();
    cmd.args(["decider", "answer"]).args(args);
    cmd.output().unwrap()
}

fn answer(h: &Harness, extra: &[&str]) -> std::process::Output {
    let mut args = vec![WF, "--gate", GATE, "--criterion", RULE, "--answer", "pass"];
    args.extend_from_slice(extra);
    answer_cmd(h, &args)
}

/// `out` is the typed refusal `code`, exit 2, and the log gained nothing.
fn assert_refused(h: &Harness, before: usize, out: &std::process::Output, code: &str) {
    assert_eq!(out.status.code(), Some(2), "{}", describe(out));
    let body = json_out(out);
    assert_eq!(body["error"]["code"], code, "{}", body);
    assert_eq!(body["command"], "decider answer");
    assert_eq!(h.events().len(), before, "nothing appended");
}

// ---------------------------------------------------------------------------
// which units fall back
// ---------------------------------------------------------------------------

#[test]
fn a_unit_one_byte_over_max_bytes_falls_back_alone() {
    // ac-4 is one byte over the budget; ac-6 is exactly at it.
    let units = fixture(10, &[(4, MAX_BYTES + 1), (6, MAX_BYTES)]);
    let h = harness(&unit_template(), &units);
    for _ in 0..9 {
        h.stub.push(pass());
    }
    let out = run(&h);

    // One unit waits and none fails: the gate is pending.
    let cond = condition(&out);
    assert_eq!(cond["status"], "pending", "{}", cond);
    assert_eq!(
        fallbacks(&h),
        [("ac-4".to_string(), "over_budget".to_string())]
    );

    // The waiting finding names the unit and its command carries --unit.
    let f = findings(&out);
    assert_eq!(f.len(), 1, "{}", cond);
    assert_eq!(f[0]["level"], "warning");
    assert_eq!(f[0]["unit"]["key"], "ac-4");
    let msg = f[0]["message"].as_str().unwrap();
    assert!(msg.contains("(over_budget)"), "{}", msg);
    assert!(
        msg.contains(
            "`koto decider answer wf --gate ac_binary --criterion ac_binary --unit ac-4 --answer"
        ),
        "{}",
        msg
    );

    // The other nine carry the stub's verdicts; ac-4 was never sent.
    assert_eq!(h.stub.request_count(), 9);
    for r in h.stub.requests() {
        assert!(!r.body_str().contains("UNIT-TEXT-04"), "{}", r.body_str());
    }
    let sent: Vec<Value> = h.stub.requests().iter().map(|r| r.json()).collect();
    assert!(
        sent.iter()
            .any(|b| b["state"]["ac"] == sized(6, MAX_BYTES).as_str()),
        "a unit at exactly max_bytes is graded"
    );
    for i in (1..=10).filter(|i| *i != 4) {
        let u = unit_entry(&out, &key(i));
        assert_eq!(u["failed"], json!([]), "{}", u);
        assert_eq!(u["awaiting"], json!([]), "{}", u);
    }
    assert_eq!(unit_entry(&out, "ac-4")["awaiting"], json!([RULE]));
    let checked = payloads(&h, "decider_checked");
    assert_eq!(checked.iter().filter(|c| c["outcome"] == "pass").count(), 9);
}

#[test]
fn max_units_grades_the_first_units_and_limits_the_rest() {
    // Five units, the fifth far over the budget: past max_units its size
    // doesn't matter.
    let units = fixture(5, &[(5, MAX_BYTES * 4)]);
    let tpl = template(
        "        units: jsonl\n        max_units: 3\n",
        &[RULE, SECOND],
    );
    let h = harness(&tpl, &units);
    // Units in print order, criteria in declaration order.
    for _ in 0..3 {
        h.stub.push(pass_of(RULE));
        h.stub.push(pass_of(SECOND));
    }
    let out = run(&h);

    // Exactly three requests per criterion, none about units four and five.
    let sent: Vec<Value> = h.stub.requests().iter().map(|r| r.json()).collect();
    assert_eq!(sent.len(), 6);
    for rule in [RULE, SECOND] {
        let n = sent
            .iter()
            .filter(|b| b["questions"].get(rule).is_some())
            .count();
        assert_eq!(n, 3, "{} in {:?}", rule, sent);
    }
    for r in h.stub.requests() {
        let raw = r.body_str();
        assert!(!raw.contains("UNIT-TEXT-04") && !raw.contains("UNIT-TEXT-05"));
    }

    // Units four and five fall back with unit_limit for every criterion.
    let mut fb = fallbacks(&h);
    fb.sort();
    let expected: Vec<(String, String)> = ["ac-4", "ac-4", "ac-5", "ac-5"]
        .iter()
        .map(|k| (k.to_string(), "unit_limit".to_string()))
        .collect();
    assert_eq!(fb, expected);
    for k in ["ac-4", "ac-5"] {
        assert_eq!(unit_entry(&out, k)["awaiting"], json!([RULE, SECOND]));
    }
    for k in ["ac-1", "ac-2", "ac-3"] {
        assert_eq!(unit_entry(&out, k)["awaiting"], json!([]));
    }
    assert_eq!(condition(&out)["status"], "pending");
    for f in findings(&out) {
        assert!(f["message"].as_str().unwrap().contains("(unit_limit)"));
    }
}

#[test]
fn a_defaulted_max_units_caps_the_units_as_a_declared_one_does() {
    // Two one-criterion unit-mode checks on the state: neither declares
    // max_units, so each compiles to 32 / 2 = 16. The second prints no
    // units; without the default the first would grade all twenty, since
    // the per-call budget of 32 has room for them.
    let tpl = format!(
        r#"---
name: units-default-cap
version: "1.0"
initial_state: review
states:
  review:
    gates:
      {GATE}:
        type: decider-check
        command: "cat units.jsonl"
        label: ac
        max_bytes: {MAX_BYTES}
        units: jsonl
        criteria:
{first}      ac_other:
        type: decider-check
        command: "true"
        units: jsonl
        criteria:
{second}    transitions:
      - target: done
  done:
    terminal: true
---

## review

Review the acceptance criteria.

## done

Done.
"#,
        first = criterion_yaml(RULE),
        second = criterion_yaml(SECOND),
    );
    let h = harness(&tpl, &fixture(20, &[]));
    for _ in 0..16 {
        h.stub.push(pass());
    }
    let out = run(&h);

    assert_eq!(h.stub.request_count(), 16);
    for r in h.stub.requests() {
        let raw = r.body_str();
        for i in 17..=20 {
            assert!(!raw.contains(&format!("UNIT-TEXT-{:02}", i)), "{}", raw);
        }
    }
    let expected: Vec<(String, String)> = (17..=20)
        .map(|i| (key(i), "unit_limit".to_string()))
        .collect();
    assert_eq!(fallbacks(&h), expected);
    for i in 17..=20 {
        assert_eq!(unit_entry(&out, &key(i))["awaiting"], json!([RULE]));
    }
    for i in 1..=16 {
        assert_eq!(unit_entry(&out, &key(i))["awaiting"], json!([]));
    }
    assert_eq!(condition(&out)["status"], "pending");
}

#[test]
fn an_empty_unit_is_not_graded_and_falls_back_to_nobody() {
    let units = jsonl(&[
        (key(1), text(1)),
        (key(2), String::new()),
        (key(3), " \n\t ".into()),
        (key(4), text(4)),
    ]);
    let h = harness(&unit_template(), &units);
    h.stub.push(pass());
    h.stub.push(pass());
    let out = run(&h);
    assert_eq!(out["state"], "done", "{}", out);
    assert_eq!(h.stub.request_count(), 2);
    assert!(fallbacks(&h).is_empty());
    let checked = payloads(&h, "decider_checked");
    for k in ["ac-2", "ac-3"] {
        let c = checked
            .iter()
            .find(|c| c["unit"]["key"] == k)
            .unwrap_or_else(|| panic!("no record for {}", k));
        assert_eq!(c["outcome"], "not_graded", "{}", c);
    }
}

#[test]
fn an_extraction_that_prints_no_units_passes_with_nothing_graded() {
    let h = harness(&template("        units: jsonl\n", &[RULE, SECOND]), "");
    let out = run(&h);
    assert_eq!(out["state"], "done", "{}", out);
    assert_eq!(h.stub.request_count(), 0);
    assert!(fallbacks(&h).is_empty());
    let checked = payloads(&h, "decider_checked");
    assert_eq!(checked.len(), 2);
    assert!(checked.iter().all(|c| c["outcome"] == "not_graded"));
    let evals = payloads(&h, "gate_evaluated");
    let last = evals.last().unwrap();
    assert_eq!(last["outcome"], "passed", "{}", last);
    assert_eq!(last["output"]["units"], json!([]));
}

#[test]
fn an_unreadable_reply_or_a_provider_error_falls_back_that_unit_alone() {
    let h = harness(&unit_template(), &fixture(5, &[]));
    h.stub.push(pass()); // ac-1
    h.stub.push(malformed()); // ac-2, then its retry
    h.stub.push(malformed());
    h.stub.push(pass()); // ac-3
    h.stub.push(Reply::status(401)); // ac-4, not retried
    h.stub.push(pass()); // ac-5
    let out = run(&h);
    assert_eq!(h.stub.request_count(), 6);
    assert_eq!(
        fallbacks(&h),
        [
            ("ac-2".to_string(), "unreadable_response".to_string()),
            ("ac-4".to_string(), "provider_error".to_string()),
        ]
    );
    assert_eq!(condition(&out)["status"], "pending");
    for k in ["ac-1", "ac-3", "ac-5"] {
        let u = unit_entry(&out, k);
        assert_eq!(u["awaiting"], json!([]), "{}", u);
        assert_eq!(u["failed"], json!([]), "{}", u);
    }
    let passed: Vec<String> = payloads(&h, "decider_checked")
        .iter()
        .filter(|c| c["outcome"] == "pass")
        .map(|c| c["unit"]["key"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(passed, ["ac-1", "ac-3", "ac-5"]);
}

#[cfg(unix)]
#[test]
fn a_lock_held_past_the_wait_makes_every_unit_busy() {
    use std::os::unix::io::AsRawFd;
    // A 50 ms timeout makes the wait 4 x 2 x 50 ms + 1 s = 1.4 s.
    let h = harness(&unit_template(), &fixture(10, &[]));
    h.user_config("[decider]\ntimeout_ms = 50\n");
    std::fs::create_dir_all(h.session_dir()).unwrap();
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(h.lock_path())
        .unwrap();
    // SAFETY: the descriptor belongs to `lock`, which outlives the call.
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let out = run(&h);
    drop(lock);

    assert_eq!(h.stub.request_count(), 0);
    let fb = fallbacks(&h);
    let expected: Vec<(String, String)> = (1..=10).map(|i| (key(i), "busy".to_string())).collect();
    assert_eq!(fb, expected);
    let cond = condition(&out);
    assert_eq!(cond["status"], "pending", "{}", cond);
    for i in 1..=10 {
        assert_eq!(unit_entry(&out, &key(i))["awaiting"], json!([RULE]));
    }
    assert_eq!(findings(&out).len(), 10);
    assert_ne!(out["state"], "done");
}

// ---------------------------------------------------------------------------
// answering one unit
// ---------------------------------------------------------------------------

/// Ten units with ac-4 and ac-8 over the budget and the rest passed, after
/// one `koto next`.
fn two_waiting() -> (Harness, Value) {
    let units = fixture(10, &[(4, MAX_BYTES + 1), (8, MAX_BYTES + 1)]);
    let h = harness(&unit_template(), &units);
    for _ in 0..8 {
        h.stub.push(pass());
    }
    let out = run(&h);
    assert_eq!(condition(&out)["status"], "pending");
    (h, out)
}

#[test]
fn an_answer_for_one_unit_applies_to_that_unit_only() {
    let (h, out) = two_waiting();
    let f = findings(&out);
    let ac4 = f.iter().find(|f| f["unit"]["key"] == "ac-4").unwrap();
    let words = command_of(ac4, "pass");
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let rec = answer_cmd(&h, &words[2..]);
    assert!(rec.status.success(), "{}", describe(&rec));
    let body = json_out(&rec);
    assert_eq!(body["status"], "recorded");
    assert_eq!(body["unit"], "ac-4");
    let answered = payloads(&h, "decider_check_answered");
    assert_eq!(answered.len(), 1);
    assert_eq!(answered[0]["unit"]["key"], "ac-4");

    // The next tick uses the answer for ac-4 and still waits on ac-8.
    let out = run(&h);
    assert_eq!(h.stub.request_count(), 8, "nothing new is sent");
    assert_eq!(condition(&out)["status"], "pending");
    assert_eq!(unit_entry(&out, "ac-4")["agent_answered"], json!([RULE]));
    assert_eq!(unit_entry(&out, "ac-4")["awaiting"], json!([]));
    assert_eq!(unit_entry(&out, "ac-8")["awaiting"], json!([RULE]));
    let f = findings(&out);
    assert_eq!(f.len(), 1, "{:?}", f);
    assert_eq!(f[0]["unit"]["key"], "ac-8");

    // Editing ac-4's text grades it afresh: it now fits and is sent, and
    // the decider fails it. One unit failing and one waiting fails the gate.
    let units = fixture(10, &[(8, MAX_BYTES + 1)]);
    write_units(&h, &units);
    h.stub.push(fail());
    let out = run(&h);
    assert_eq!(h.stub.request_count(), 9);
    assert!(h
        .stub
        .last_request()
        .unwrap()
        .body_str()
        .contains("UNIT-TEXT-04"));
    let cond = condition(&out);
    assert_eq!(cond["status"], "failed", "{}", cond);
    assert_eq!(unit_entry(&out, "ac-4")["failed"], json!([RULE]));
    assert_eq!(unit_entry(&out, "ac-4")["agent_answered"], json!([]));
    assert_eq!(unit_entry(&out, "ac-8")["awaiting"], json!([RULE]));
}

#[test]
fn a_unit_the_agent_fails_fails_a_veto_gate() {
    let (h, _) = two_waiting();
    let out = answer_cmd(
        &h,
        &[
            WF,
            "--gate",
            GATE,
            "--criterion",
            RULE,
            "--unit",
            "ac-4",
            "--answer",
            "fail",
        ],
    );
    assert!(out.status.success(), "{}", describe(&out));
    let out = run(&h);
    let cond = condition(&out);
    assert_eq!(cond["status"], "failed", "{}", cond);
    let errors: Vec<Value> = findings(&out)
        .into_iter()
        .filter(|f| f["level"] == "error")
        .collect();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["unit"]["key"], "ac-4");
    assert_eq!(errors[0]["message_source"], "koto");
}

#[test]
fn every_unit_passing_passes_the_gate() {
    let h = harness(&unit_template(), &fixture(10, &[]));
    for _ in 0..10 {
        h.stub.push(pass());
    }
    let out = run(&h);
    assert_eq!(out["state"], "done", "{}", out);
    assert!(fallbacks(&h).is_empty());
}

#[test]
fn the_answer_command_refuses_a_missing_unknown_or_settled_unit() {
    let (h, _) = two_waiting();
    let before = h.events().len();
    assert_refused(&h, before, &answer(&h, &[]), "unit_required");
    assert_refused(
        &h,
        before,
        &answer(&h, &["--unit", "ac-99"]),
        "unknown_unit",
    );
    // ac-2 was printed but got a verdict: nothing of it waits.
    assert_refused(&h, before, &answer(&h, &["--unit", "ac-2"]), "no_fallback");
}

#[test]
fn a_unit_is_refused_where_nothing_waits_on_one() {
    // A check without units.
    let h = harness(&template("", &[RULE]), &fixture(3, &[]));
    h.stub.push(Reply::status(401));
    run(&h);
    let before = h.events().len();
    assert_refused(
        &h,
        before,
        &answer(&h, &["--unit", "ac-1"]),
        "unit_not_expected",
    );

    // A unit-mode check whose extraction failed: its fallback names no unit.
    let h = harness(&unit_template(), "not json\n");
    run(&h);
    assert_eq!(fallbacks(&h), [(String::new(), "extraction_failed".into())]);
    let before = h.events().len();
    assert_refused(
        &h,
        before,
        &answer(&h, &["--unit", "ac-1"]),
        "unit_not_expected",
    );
}

#[test]
fn a_check_without_units_is_answered_without_a_unit_as_before() {
    let h = harness(&template("", &[RULE]), &fixture(3, &[]));
    h.stub.push(Reply::status(401));
    let out = run(&h);
    let f = findings(&out);
    assert_eq!(f.len(), 1);
    let msg = f[0]["message"].as_str().unwrap();
    assert!(
        msg.contains("`koto decider answer wf --gate ac_binary --criterion ac_binary --answer"),
        "{}",
        msg
    );
    assert!(!msg.contains("--unit"), "{}", msg);
    let rec = answer(&h, &[]);
    assert!(rec.status.success(), "{}", describe(&rec));
    let body = json_out(&rec);
    assert_eq!(body["status"], "recorded");
    assert!(body.get("unit").is_none(), "{}", body);
    let out = run(&h);
    assert_eq!(out["state"], "done", "{}", out);
}

// ---------------------------------------------------------------------------
// the run journal
// ---------------------------------------------------------------------------

/// A criterion that declares a summary and a rule text, so a failure hands
/// its rule back and appends a `rule_delivered` event.
fn ruled_criterion_yaml(id: &str) -> String {
    format!(
        "{}            summary: \"State one observable outcome per criterion.\"\n            rule_text: \"Each acceptance criterion names one observable outcome.\"\n",
        criterion_yaml(id)
    )
}

/// The run journal the harness's session store writes: inside the store's
/// base directory, which the harness points `KOTO_SESSIONS_BASE` at.
fn journal(h: &Harness) -> Vec<Value> {
    let raw = journal_text(h);
    raw.lines()
        .map(|l| serde_json::from_str(l).expect("every journal line is JSON"))
        .collect()
}

fn journal_text(h: &Harness) -> String {
    std::fs::read_to_string(h.dir.join("sessions").join("_run_journal.jsonl")).unwrap_or_default()
}

fn records_of(h: &Harness, kind: &str) -> Vec<Value> {
    journal(h)
        .into_iter()
        .filter(|r| r["kind"] == kind)
        .collect()
}

/// How the journal writes a printed value: `sha256:` and the first 32 hex
/// characters of its SHA-256.
fn digest(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let hex = hex::encode(Sha256::digest(value.as_bytes()));
    format!("sha256:{}", &hex[..32])
}

/// One unit line with a printed document and section.
fn located(key: &str, text: &str, document: &str, section: &str) -> String {
    json!({"key": key, "text": text, "document": document, "section": section}).to_string() + "\n"
}

#[test]
fn the_journal_carries_unit_digests_declared_labels_and_the_confidence() {
    // Four units whose key, document and section carry a sentinel: one
    // passes, one fails, one gets 0.12345 / 0.8 / 0.07655 (below the 0.9
    // threshold, so an escape), and one is over the budget and falls back.
    let unit = |i: usize, text: &str| {
        located(
            &format!("SENTINEL-KEY-{}", i),
            text,
            "docs/SENTINEL-DOC.md",
            "SENTINEL-SECTION",
        )
    };
    let units = [
        unit(1, &text(1)),
        unit(2, &text(2)),
        unit(3, &sized(3, MAX_BYTES + 1)),
        unit(4, &text(4)),
    ]
    .concat();
    let tpl = template_with(
        "        units: jsonl\n        unit_document: prd\n        unit_section: acceptance-criteria\n",
        &ruled_criterion_yaml(RULE),
    );
    let h = harness(&tpl, &units);
    h.stub.push(pass());
    h.stub.push(fail());
    h.stub.push(reply(RULE, 0.12345, 0.8, 0.07655));
    let out = run(&h);
    assert_eq!(condition(&out)["status"], "failed", "{}", out);

    // The agent answers the waiting unit with --unit.
    let rec = answer(&h, &["--unit", "SENTINEL-KEY-3"]);
    assert!(rec.status.success(), "{}", describe(&rec));

    let checked = records_of(&h, "decider_checked");
    assert_eq!(checked.len(), 4, "{:?}", checked);
    let fallback = records_of(&h, "decider_fallback");
    assert_eq!(fallback.len(), 1, "{:?}", fallback);
    let answered = records_of(&h, "decider_check_answered");
    assert_eq!(answered.len(), 1, "{:?}", answered);
    for (r, key) in [
        (&checked[0], "SENTINEL-KEY-1"),
        (&checked[1], "SENTINEL-KEY-2"),
        (&checked[2], "SENTINEL-KEY-3"),
        (&checked[3], "SENTINEL-KEY-4"),
        (&fallback[0], "SENTINEL-KEY-3"),
        (&answered[0], "SENTINEL-KEY-3"),
    ] {
        assert_eq!(r["koto.unit.key"], digest(key).as_str(), "{}", r);
        assert_eq!(r["koto.unit.document"], "prd", "{}", r);
        assert_eq!(r["koto.unit.section"], "acceptance-criteria", "{}", r);
    }
    // The confidence, whatever the outcome the answer led to: pass 0.95 and
    // fail 0.95 are 9500, the highest of 0.12345, 0.8 and 0.07655 is 8000;
    // the unit over the budget sent nothing and read no answer.
    assert_eq!(checked[0]["koto.decider.outcome"], "pass");
    assert_eq!(checked[0]["koto.decider.confidence_bp"], 9500);
    assert_eq!(checked[1]["koto.decider.outcome"], "fail");
    assert_eq!(checked[1]["koto.decider.confidence_bp"], 9500);
    assert_eq!(checked[2]["koto.decider.reason"], "over_budget");
    assert!(
        checked[2].get("koto.decider.confidence_bp").is_none(),
        "{}",
        checked[2]
    );
    assert_eq!(checked[3]["koto.decider.outcome"], "escape");
    assert_eq!(checked[3]["koto.decider.confidence_bp"], 8000);
    assert_eq!(answered[0]["koto.decider.answer"], "pass");

    // The failed rule came back once, for one unit.
    let delivered = records_of(&h, "rule_delivered");
    assert_eq!(delivered.len(), 1, "{:?}", delivered);
    let d = &delivered[0];
    assert_eq!(d["koto.state"], "review");
    assert_eq!(d["koto.check.name"], GATE);
    assert_eq!(d["koto.rule.id"], RULE);
    assert_eq!(d["koto.rule.extent"], "summary");
    assert_eq!(d["koto.rule.unit_count"], 1);
    assert!(
        d["koto.seq"].is_u64() && d["koto.visit.seq"].is_u64(),
        "{}",
        d
    );

    // Nothing the extraction printed, and no rule text, reached the journal.
    let raw = journal_text(&h);
    assert!(!raw.contains("SENTINEL"), "{}", raw);
    assert!(!raw.contains("observable outcome"), "{}", raw);
    assert!(
        !raw.contains("0.12345") && !raw.contains("0.07655"),
        "{}",
        raw
    );
    // The session log keeps the printed unit for a local reader.
    assert!(h.raw_log().contains("SENTINEL-KEY-2"));
}

#[test]
fn without_labels_the_journal_digests_the_printed_document_and_section() {
    let units = [
        located("ac-1", &text(1), "docs/SENTINEL-DOC.md", "SENTINEL-SECTION"),
        json!({"key": "ac-2", "text": text(2)}).to_string() + "\n",
    ]
    .concat();
    let h = harness(&unit_template(), &units);
    h.stub.push(pass());
    h.stub.push(Reply::status(401));
    run(&h);
    let checked = records_of(&h, "decider_checked");
    assert_eq!(checked.len(), 2, "{:?}", checked);
    assert_eq!(checked[0]["koto.unit.key"], digest("ac-1").as_str());
    assert_eq!(
        checked[0]["koto.unit.document"],
        digest("docs/SENTINEL-DOC.md").as_str()
    );
    assert_eq!(
        checked[0]["koto.unit.section"],
        digest("SENTINEL-SECTION").as_str()
    );
    // A unit that printed no document or section has neither.
    for r in [&checked[1], &records_of(&h, "decider_fallback")[0]] {
        assert_eq!(r["koto.unit.key"], digest("ac-2").as_str(), "{}", r);
        assert!(r.get("koto.unit.document").is_none(), "{}", r);
        assert!(r.get("koto.unit.section").is_none(), "{}", r);
    }
    // A provider error read no answer: no confidence.
    assert_eq!(checked[1]["koto.decider.reason"], "provider_error");
    assert!(checked[1].get("koto.decider.confidence_bp").is_none());
    assert!(!journal_text(&h).contains("SENTINEL"));

    // A unit-mode check whose extraction failed names no unit.
    let h = harness(&unit_template(), "not json\n");
    run(&h);
    for kind in ["decider_checked", "decider_fallback"] {
        let r = &records_of(&h, kind)[0];
        assert_eq!(r["koto.decider.reason"], "extraction_failed", "{}", r);
        for absent in [
            "koto.unit.key",
            "koto.unit.document",
            "koto.unit.section",
            "koto.decider.confidence_bp",
        ] {
            assert!(r.get(absent).is_none(), "{} on {}", absent, r);
        }
    }
}

#[test]
fn a_single_slice_check_carries_the_confidence_and_no_unit() {
    // One short slice, inside the byte budget.
    let h = harness(
        &template_with("", &ruled_criterion_yaml(RULE)),
        "Exports finish within one batch.\n",
    );
    h.stub.push(fail());
    let out = run(&h);
    assert_eq!(condition(&out)["status"], "failed");
    let checked = records_of(&h, "decider_checked");
    assert_eq!(checked.len(), 1);
    let r = &checked[0];
    assert_eq!(r["koto.decider.confidence_bp"], 9500);
    for absent in ["koto.unit.key", "koto.unit.document", "koto.unit.section"] {
        assert!(r.get(absent).is_none(), "{} on {}", absent, r);
    }
    let delivered = records_of(&h, "rule_delivered");
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0]["koto.rule.extent"], "summary");
    assert!(delivered[0].get("koto.rule.unit_count").is_none());

    // The same failure again in the visit, its verdict reused, hands back
    // the full text.
    run(&h);
    let delivered = records_of(&h, "rule_delivered");
    assert_eq!(delivered.len(), 2);
    assert_eq!(delivered[1]["koto.rule.extent"], "full");
}

#[test]
fn twenty_failing_units_write_one_rule_delivered_with_a_count_of_twenty() {
    let tpl = template_with("        units: jsonl\n", &ruled_criterion_yaml(RULE));
    let h = harness(&tpl, &fixture(20, &[]));
    for _ in 0..20 {
        h.stub.push(fail());
    }
    let out = run(&h);
    assert_eq!(condition(&out)["status"], "failed");
    let delivered = records_of(&h, "rule_delivered");
    assert_eq!(delivered.len(), 1, "{:?}", delivered);
    assert_eq!(delivered[0]["koto.rule.unit_count"], 20);
    assert_eq!(records_of(&h, "decider_checked").len(), 20);
}

#[test]
fn the_unit_count_counts_failing_units_the_finding_cap_left_out() {
    // 120 units under the default cap of 32: 32 graded and failing, 88
    // falling back with unit_limit. 120 findings pass the response's cap of
    // 100, so the findings are cut, but the count isn't.
    let tpl = template_with("        units: jsonl\n", &ruled_criterion_yaml(RULE));
    let h = harness(&tpl, &fixture(120, &[]));
    for _ in 0..32 {
        h.stub.push(fail());
    }
    let out = run(&h);
    let cond = condition(&out);
    assert_eq!(cond["status"], "failed");
    assert_eq!(
        cond["failure"]["findings_truncated"], true,
        "the cap cut the findings: {}",
        cond["failure"]
    );
    let delivered = records_of(&h, "rule_delivered");
    assert_eq!(delivered.len(), 1, "{:?}", delivered);
    assert_eq!(delivered[0]["koto.rule.unit_count"], 32);
    let rules = cond["rules"].as_array().unwrap();
    assert_eq!(rules[0]["units"].as_array().unwrap().len(), 32);
}
