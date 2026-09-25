//! `koto decider report`: the ledger join, the metrics, the fixture runner,
//! and promotion eligibility.
//!
//! docs/designs/current/DESIGN-jev-decision-offload.md, Decision 4. Every harness
//! sets `HOME` to a temp directory, so no test reads or writes a developer's
//! real ledger. Only commands that run fixtures opt in, each on its own
//! `Command`, and every one of them points the endpoint at the `std::net`
//! loopback stub.

#[path = "support/decider_stub.rs"]
mod decider_stub;

#[path = "support/decider_session.rs"]
mod decider_session;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use decider_session::*;
use decider_stub::{closed_url, Reply};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn ledger_file(h: &Harness) -> PathBuf {
    h.home().join(".koto").join("_decider_ledger.jsonl")
}

/// Write `lines` (each followed by a newline) as the harness's ledger.
fn write_ledger(h: &Harness, lines: &[Value]) {
    let path = ledger_file(h);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let body: String = lines.iter().map(|l| format!("{}\n", l)).collect();
    std::fs::write(path, body).unwrap();
}

/// A `consulted` line for `review.verdict`. `winning` is `None` for a
/// consultation that got no answer; `"unclear"` is the escape.
fn consulted(
    sid: Option<&str>,
    seq: u64,
    hash: &str,
    outcome: &str,
    winning: Option<&str>,
    at: bool,
) -> Value {
    let no_answer = outcome == "error" || outcome == "input_unavailable";
    let mut field = json!({
        "declaration_hash": hash,
        "modes": {"proceed": "shadow", "exit": "shadow"},
        "at_threshold": at && !no_answer,
    });
    if !no_answer {
        let w = winning.expect("an answered consultation has a winner");
        let fo = if w == "unclear" {
            "escape"
        } else if !at {
            "below_threshold"
        } else if outcome == "applied" {
            "qualified"
        } else {
            "shadow"
        };
        field["winning"] = json!(w);
        field["confidence"] = json!(if at { 0.95 } else { 0.6 });
        if w != "unclear" {
            field["threshold"] = json!(0.9);
        }
        field["outcome"] = json!(fo);
    }
    let mut v = json!({
        "kind": "consulted",
        "v": 1,
        "at": "2026-01-01T00:00:00Z",
        "session": "wf",
        "session_id": sid,
        "state": "review",
        "visit_seq": seq,
        "provider": "jev",
        "model": "jev-test",
        "outcome": outcome,
        "latency_ms": 100,
        "directive_bytes": 100,
        "endpoint_origin": "default",
        "fields": {"verdict": field},
    });
    if outcome == "error" {
        v["error_class"] = json!("timeout");
    }
    v
}

fn answered(sid: Option<&str>, seq: u64, value: Value) -> Value {
    json!({
        "kind": "answered",
        "v": 1,
        "at": "2026-01-01T00:00:01Z",
        "session": "wf",
        "session_id": sid,
        "state": "review",
        "visit_seq": seq,
        "values": {"verdict": value},
    })
}

/// A paired visit: the decider chose `decider` at threshold, the agent
/// `agent`.
fn pair(sid: &str, seq: u64, hash: &str, decider: &str, agent: &str) -> Vec<Value> {
    vec![
        consulted(Some(sid), seq, hash, "not_applied", Some(decider), true),
        answered(Some(sid), seq, json!(agent)),
    ]
}

/// `koto decider report --json` with no decider env at all.
fn report(h: &Harness, extra: &[&str]) -> Output {
    let mut cmd = h.koto();
    cmd.args(["decider", "report"]).args(extra);
    cmd.output().unwrap()
}

fn report_json(h: &Harness, extra: &[&str]) -> Value {
    let mut args = vec!["--json"];
    args.extend_from_slice(extra);
    let out = report(h, &args);
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    json_out(&out)
}

fn question<'a>(r: &'a Value, hash: &str) -> &'a Value {
    r["questions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|q| q["declaration_hash"] == hash)
        .unwrap_or_else(|| panic!("no question {}: {}", hash, r))
}

fn value<'a>(q: &'a Value, v: &str) -> &'a Value {
    q["values"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["value"] == v)
        .unwrap_or_else(|| panic!("no value {}: {}", v, q))
}

fn f(x: &Value) -> f64 {
    x.as_f64().unwrap_or_else(|| panic!("not a number: {}", x))
}

/// The declaration hash of `state.field` in the harness's template.
fn current_hash(h: &Harness, state: &str, field: &str) -> String {
    let t = koto::template::compile::compile(&h.dir.join("template.md"), true).unwrap();
    let schema = &t.states[state].accepts.as_ref().unwrap()[field];
    koto::template::decider::declaration_hash(schema.decider.as_ref().unwrap(), &schema.description)
}

/// A Jev reply answering `verdict` as `v`: `proceed`, `exit`, or `unclear`
/// at 0.95, or `below:<value>` at 0.6.
fn say(v: &str) -> Reply {
    match v {
        "proceed" => verdict(0.95, 0.03, 0.02),
        "exit" => verdict(0.03, 0.95, 0.02),
        "unclear" => verdict(0.02, 0.03, 0.95),
        "below:proceed" => verdict(0.6, 0.3, 0.1),
        "below:exit" => verdict(0.3, 0.6, 0.1),
        other => panic!("unknown answer {}", other),
    }
}

/// Fixture lines for the standard template, one per label.
fn fixture_body(labels: &[&str]) -> String {
    labels
        .iter()
        .enumerate()
        .map(|(i, l)| {
            format!(
                "{}\n",
                json!({
                    "id": format!("c{}", i + 1),
                    "inputs": {"outline_item": format!("item {}", i + 1), "plan_path": "docs/p.md"},
                    "expected": l,
                })
            )
        })
        .collect()
}

fn write_fixtures(h: &Harness, body: &str) {
    std::fs::write(h.dir.join("fx.jsonl"), body).unwrap();
}

const FIXTURE_ARGS: [&str; 7] = [
    "--fixtures",
    "fx.jsonl",
    "--template",
    "template.md",
    "--state",
    "review",
    "--json",
];

/// Run the fixture set with `cmd`'s environment.
fn run_fixtures(mut cmd: Command, extra: &[&str]) -> Output {
    cmd.args(["decider", "report"])
        .args(FIXTURE_ARGS)
        .args(extra);
    cmd.output().unwrap()
}

/// Opted in through the environment against the stub.
fn opted_in(h: &Harness) -> Command {
    h.koto_mode("shadow")
}

fn fixtures_ok(h: &Harness, extra: &[&str]) -> Value {
    let out = run_fixtures(opted_in(h), extra);
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    json_out(&out)["fixtures"].clone()
}

fn fx_value<'a>(fx: &'a Value, v: &str) -> &'a Value {
    fx["values"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["value"] == v)
        .unwrap_or_else(|| panic!("no fixture value {}: {}", v, fx))
}

fn reasons(v: &Value) -> Vec<String> {
    v["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap().to_string())
        .collect()
}

fn failed_conditions(v: &Value) -> Vec<String> {
    v["conditions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["met"] == false)
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// The eligible baseline and its single changes
// ---------------------------------------------------------------------------

/// A fixture set, the stub's answers to it, and a ledger.
struct Scenario {
    labels: Vec<&'static str>,
    answers: Vec<&'static str>,
    ledger: Vec<Value>,
}

/// 30 paired observations under `hash`: the agent chose `proceed` 20 times
/// and `exit` 10 times, and the decider agreed at threshold every time.
fn eligible_ledger(hash: &str) -> Vec<Value> {
    eligible_ledger_in("s-elig", hash)
}

fn eligible_ledger_in(sid: &str, hash: &str) -> Vec<Value> {
    let mut l = Vec::new();
    for i in 0..30 {
        let v = if i < 20 { "proceed" } else { "exit" };
        l.extend(pair(sid, i + 1, hash, v, v));
    }
    l
}

/// Every condition met: 20 `proceed` and 20 `exit` cases answered
/// correctly, and the eligible ledger.
fn baseline(hash: &str) -> Scenario {
    let mut labels = vec!["proceed"; 20];
    labels.extend(vec!["exit"; 20]);
    Scenario {
        answers: labels.clone(),
        labels,
        ledger: eligible_ledger(hash),
    }
}

fn run_scenario(s: &Scenario, extra: &[&str]) -> Value {
    let h = Harness::new(&standard("shadow", "shadow"));
    run_scenario_in(&h, s, extra)
}

fn run_scenario_in(h: &Harness, s: &Scenario, extra: &[&str]) -> Value {
    write_ledger(h, &s.ledger);
    write_fixtures(h, &fixture_body(&s.labels));
    for a in &s.answers {
        h.stub.push(say(a));
    }
    let fx = fixtures_ok(h, extra);
    assert_eq!(h.stub.request_count(), s.labels.len());
    fx
}

fn std_hash() -> String {
    let h = Harness::new(&standard("shadow", "shadow"));
    current_hash(&h, "review", "verdict")
}

const INCLUDE: &[&str] = &["--include-custom-endpoints"];

// ---------------------------------------------------------------------------
// The verb and read-only behavior
// ---------------------------------------------------------------------------

#[test]
fn help_lists_every_flag() {
    let h = Harness::new(&standard("shadow", "shadow"));
    let out = report(&h, &["--help"]);
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    for flag in [
        "--ledger",
        "--state",
        "--json",
        "--include-custom-endpoints",
        "--fixtures",
        "--template",
        "--field",
    ] {
        assert!(help.contains(flag), "{} missing from:\n{}", flag, help);
    }
}

#[test]
fn clap_enforces_the_fixture_flag_pairings() {
    let h = Harness::new(&standard("shadow", "shadow"));
    write_fixtures(&h, &fixture_body(&["proceed"]));
    for args in [
        vec!["--fixtures", "fx.jsonl"],
        vec!["--fixtures", "fx.jsonl", "--template", "template.md"],
        vec!["--fixtures", "fx.jsonl", "--state", "review"],
        vec!["--template", "template.md"],
        vec!["--template", "template.md", "--state", "review"],
        vec!["--field", "verdict"],
        vec!["--field", "verdict", "--state", "review"],
    ] {
        let out = report(&h, &args);
        assert_eq!(out.status.code(), Some(2), "{:?}: {}", args, describe(&out));
        assert!(
            stderr(&out).contains("required"),
            "{:?}: {}",
            args,
            describe(&out)
        );
        assert!(out.stdout.is_empty());
    }
    assert_eq!(h.stub.request_count(), 0);
}

#[test]
fn default_ledger_is_under_home_and_a_missing_one_is_an_empty_report() {
    let h = Harness::new(&standard("shadow", "shadow"));
    let r = report_json(&h, &[]);
    assert_eq!(r["questions"], json!([]));
    assert_eq!(r["header"]["lines"], 0);
    assert_eq!(r["fixtures"], Value::Null);
    assert_eq!(
        r["ledger"].as_str().unwrap(),
        ledger_file(&h).display().to_string()
    );
    assert!(
        !h.home().join(".koto").exists(),
        "the report created ~/.koto"
    );
    let out = report(&h, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));

    write_ledger(&h, &pair("s1", 1, "h1", "proceed", "proceed"));
    let r = report_json(&h, &[]);
    assert_eq!(r["questions"].as_array().unwrap().len(), 1);

    // --ledger names another file.
    let other = h.dir.join("other.jsonl");
    std::fs::write(&other, "").unwrap();
    let r = report_json(&h, &["--ledger", other.to_str().unwrap()]);
    assert_eq!(r["questions"], json!([]));
}

#[test]
fn state_limits_the_questions_in_json_and_table() {
    let h = Harness::new(&standard("shadow", "shadow"));
    let mut lines = pair("s1", 1, "h1", "proceed", "proceed");
    let mut other = consulted(Some("s1"), 5, "h9", "not_applied", Some("exit"), true);
    other["state"] = json!("triage");
    lines.push(other);
    write_ledger(&h, &lines);

    let all = report_json(&h, &[]);
    assert_eq!(all["questions"].as_array().unwrap().len(), 2);
    let r = report_json(&h, &["--state", "review"]);
    let qs = r["questions"].as_array().unwrap();
    assert_eq!(qs.len(), 1);
    assert_eq!(qs[0]["state"], "review");

    let out = report(&h, &["--state", "triage"]);
    let table = String::from_utf8_lossy(&out.stdout);
    assert!(table.contains("question triage.verdict"), "{}", table);
    assert!(!table.contains("question review.verdict"), "{}", table);
}

/// Every file under `dir`, with its bytes.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                out.insert(p.clone(), b"<dir>".to_vec());
                stack.push(p);
            } else {
                out.insert(p.clone(), std::fs::read(&p).unwrap());
            }
        }
    }
    out
}

#[test]
fn the_report_changes_no_byte_with_or_without_fixtures() {
    let h = Harness::new(&standard("shadow", "shadow"));
    let hash = current_hash(&h, "review", "verdict");
    h.user_config("[decider]\ntimeout_ms = 3000\n");
    h.project_config("[decider]\nmode = \"auto\"\n");
    let mut lines = eligible_ledger(&hash);
    lines.push(consulted(Some("s2"), 1, &hash, "error", None, false));
    write_ledger(&h, &lines);
    write_fixtures(&h, &fixture_body(&["proceed", "exit"]));
    h.stub.set_default(say("proceed"));

    let before = snapshot(&h.dir);
    for key in [
        h.dir.join("template.md"),
        h.home().join(".koto").join("config.toml"),
        h.dir.join(".koto").join("config.toml"),
        ledger_file(&h),
    ] {
        assert!(
            before.contains_key(&key),
            "{} not in the snapshot",
            key.display()
        );
    }

    // Pin the compile cache inside the snapshot, wherever the developer's
    // XDG_CACHE_HOME points.
    let cache = h.dir.join("xdg-cache");
    let with_cache = |mut cmd: Command| -> Command {
        cmd.env("XDG_CACHE_HOME", &cache);
        cmd
    };
    for args in [vec![], vec!["--json"], vec!["--state", "review"]] {
        let mut cmd = with_cache(h.koto());
        cmd.args(["decider", "report"]).args(args);
        let out = cmd.output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    }
    let out = run_fixtures(with_cache(opted_in(&h)), &[]);
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert_eq!(h.stub.request_count(), 2);
    let out = run_fixtures(with_cache(opted_in(&h)), INCLUDE);
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert_eq!(h.stub.request_count(), 4);

    let after = snapshot(&h.dir);
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>(),
        "a file or directory was created or removed (compile cache, session dir, ...)"
    );
    assert_eq!(before, after, "a byte changed");
    // No session directory, and so no session event.
    assert_eq!(
        std::fs::read_dir(h.dir.join("sessions")).unwrap().count(),
        0
    );

    // The cache probe works: compiling through the cache does create it.
    assert!(!cache.exists());
    let mut cmd = with_cache(h.koto());
    cmd.args(["template", "compile", "template.md"]);
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    assert!(cache.exists());
}

// ---------------------------------------------------------------------------
// Ledger join and metrics
// ---------------------------------------------------------------------------

/// Two questions: `review.verdict` under `ha` (eight consultations, six of
/// them paired) and under `hb` (two).
fn metrics_ledger() -> Vec<Value> {
    let mut l = Vec::new();
    let mut c = |seq: u64, outcome: &str, winning: Option<&str>, at: bool, latency: u64| {
        let mut v = consulted(Some("s1"), seq, "ha", outcome, winning, at);
        v["latency_ms"] = json!(latency);
        v["directive_bytes"] = json!(500 + seq);
        l.push(v);
    };
    c(1, "not_applied", Some("proceed"), true, 100);
    c(2, "not_applied", Some("proceed"), true, 200);
    c(3, "not_applied", Some("exit"), false, 300);
    c(4, "not_applied", Some("unclear"), false, 400);
    c(5, "applied", Some("proceed"), true, 500);
    c(6, "error", None, false, 2000);
    c(7, "input_unavailable", None, false, 0);
    c(8, "not_applied", Some("exit"), true, 600);
    for (seq, agent) in [
        (1, "proceed"),
        (2, "exit"),
        (3, "exit"),
        (4, "proceed"),
        (6, "exit"),
        (7, "proceed"),
    ] {
        l.push(answered(Some("s1"), seq, json!(agent)));
    }
    for (seq, latency) in [(1, 50), (2, 70)] {
        let mut v = consulted(Some("s2"), seq, "hb", "not_applied", Some("exit"), true);
        v["latency_ms"] = json!(latency);
        l.push(v);
        l.push(answered(Some("s2"), seq, json!("exit")));
    }
    l
}

#[test]
fn json_reports_the_exact_metrics_for_a_known_ledger() {
    let h = Harness::new(&standard("shadow", "shadow"));
    write_ledger(&h, &metrics_ledger());
    let r = report_json(&h, &[]);
    assert_eq!(r["questions"].as_array().unwrap().len(), 2);

    let a = question(&r, "ha");
    assert_eq!(a["consultations"], 8);
    assert_eq!(
        a["outcomes"],
        json!({"applied": 1, "not_applied": 5, "input_unavailable": 1, "error": 1})
    );
    assert_eq!(a["well_formed"], 6);
    assert_eq!(a["paired"], 6);

    let p = value(a, "proceed");
    let e = value(a, "exit");
    assert_eq!(p["paired"], 3);
    assert_eq!(e["paired"], 3);
    assert_eq!(f(&p["recall"]), 1.0 / 3.0);
    assert_eq!(f(&e["recall"]), 0.0);
    assert_eq!(f(&p["coverage"]["well_formed"]), 3.0 / 6.0);
    assert_eq!(f(&p["coverage"]["all"]), 3.0 / 8.0);
    assert_eq!(f(&e["coverage"]["well_formed"]), 1.0 / 6.0);
    assert_eq!(f(&e["coverage"]["all"]), 1.0 / 8.0);
    assert_eq!(f(&a["coverage"]["well_formed"]), 4.0 / 6.0);
    assert_eq!(f(&a["coverage"]["all"]), 4.0 / 8.0);
    assert_eq!(p["disagreements"], 1);
    assert_eq!(e["disagreements"], 0);

    let row = |label: &str| -> Value { a["confusion"]["rows"][label].clone() };
    assert_eq!(
        row("proceed"),
        json!({"proceed": 1, "exit": 0, "below_threshold": 0, "escape": 1, "no_answer": 1})
    );
    assert_eq!(
        row("exit"),
        json!({"proceed": 1, "exit": 0, "below_threshold": 1, "escape": 0, "no_answer": 1})
    );
    for col in ["below_threshold", "escape", "no_answer"] {
        assert!(
            a["confusion"]["columns"]
                .as_array()
                .unwrap()
                .contains(&json!(col)),
            "{}",
            a["confusion"]
        );
    }

    assert_eq!(
        a["disagreements"],
        json!([{
            "session": "wf",
            "session_id": "s1",
            "visit_seq": 2,
            "agent": "exit",
            "decider": "proceed",
            "endpoint_origin": "default"
        }])
    );

    assert_eq!(f(&a["rates"]["fallback"]), 7.0 / 8.0);
    assert_eq!(f(&a["rates"]["error"]), 1.0 / 8.0);
    assert_eq!(a["rates"]["error_by_class"], json!({"timeout": 1.0 / 8.0}));
    assert_eq!(f(&a["rates"]["input_unavailable"]), 1.0 / 8.0);

    // Latencies without input_unavailable: 100 200 300 400 500 600 2000.
    assert_eq!(
        a["latency_ms"],
        json!({"samples": 7, "p50": 400, "p95": 2000})
    );

    assert_eq!(a["success_measures"]["agent_stops_removed"], 1);
    assert_eq!(a["success_measures"]["directive_bytes_not_delivered"], 505);
    assert_eq!(f(&a["success_measures"]["coverage"]), 4.0 / 6.0);
    assert_eq!(a["flags"], json!([]));

    let b = question(&r, "hb");
    assert_eq!(b["consultations"], 2);
    assert_eq!(b["paired"], 2);
    assert_eq!(f(&value(b, "exit")["recall"]), 1.0);
    assert_eq!(value(b, "proceed")["recall"], Value::Null);
    assert_eq!(f(&value(b, "exit")["coverage"]["well_formed"]), 1.0);
    assert_eq!(f(&value(b, "proceed")["coverage"]["well_formed"]), 0.0);
    assert_eq!(f(&b["rates"]["fallback"]), 1.0);
    assert_eq!(f(&b["rates"]["error"]), 0.0);
    assert_eq!(b["latency_ms"], json!({"samples": 2, "p50": 50, "p95": 70}));
    assert_eq!(b["success_measures"]["agent_stops_removed"], 0);
    assert_eq!(b["disagreements"], json!([]));
}

fn pct(x: &Value) -> String {
    if x.is_null() {
        "-".to_string()
    } else {
        format!("{:.1}%", f(x) * 100.0)
    }
}

#[test]
fn the_table_shows_the_same_numbers_as_json() {
    let h = Harness::new(&standard("shadow", "shadow"));
    write_ledger(&h, &metrics_ledger());
    let r = report_json(&h, &[]);
    let out = report(&h, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    let table = String::from_utf8_lossy(&out.stdout).into_owned();

    for q in r["questions"].as_array().unwrap() {
        let at = table
            .find(&format!(
                "declaration hash {}",
                q["declaration_hash"].as_str().unwrap()
            ))
            .unwrap_or_else(|| panic!("hash missing:\n{}", table));
        let end = table[at + 1..]
            .find("\nquestion ")
            .map_or(table.len(), |e| at + 1 + e);
        let section = &table[at..end];
        let o = &q["outcomes"];
        let expect = [
            format!(
                "consultations {} (applied {}, not_applied {}, input_unavailable {}, error {}); paired {}",
                q["consultations"], o["applied"], o["not_applied"], o["input_unavailable"], o["error"], q["paired"]
            ),
            format!(
                "coverage {} of well-formed answers, {} of all consultations",
                pct(&q["coverage"]["well_formed"]),
                pct(&q["coverage"]["all"])
            ),
            format!(
                "fallback rate {}, error rate {}",
                pct(&q["rates"]["fallback"]),
                pct(&q["rates"]["error"])
            ),
            format!(
                "latency p50 {} ms, p95 {} ms",
                q["latency_ms"]["p50"], q["latency_ms"]["p95"]
            ),
            format!(
                "agent stops removed {}, directive bytes not delivered {}",
                q["success_measures"]["agent_stops_removed"],
                q["success_measures"]["directive_bytes_not_delivered"]
            ),
        ];
        for e in expect {
            assert!(section.contains(&e), "missing {:?} in:\n{}", e, section);
        }
        for (class, rate) in q["rates"]["error_by_class"].as_object().unwrap() {
            let e = format!("{} {}", class, pct(rate));
            assert!(section.contains(&e), "missing {:?} in:\n{}", e, section);
        }
        for v in q["values"].as_array().unwrap() {
            let name = v["value"].as_str().unwrap();
            let line = section
                .lines()
                .find(|l| l.trim_start().starts_with(&format!("{} ", name)))
                .unwrap_or_else(|| panic!("no row for {}:\n{}", name, section));
            let cells: Vec<&str> = line.split_whitespace().collect();
            assert_eq!(
                cells[2..],
                [
                    v["paired"].to_string().as_str(),
                    pct(&v["recall"]).as_str(),
                    pct(&v["coverage"]["well_formed"]).as_str(),
                    pct(&v["coverage"]["all"]).as_str(),
                    v["disagreements"].to_string().as_str(),
                ],
                "{}",
                line
            );
        }
        for d in q["disagreements"].as_array().unwrap() {
            let e = format!(
                "{}/{} ({}): agent {}, decider {}",
                d["session_id"].as_str().unwrap(),
                d["visit_seq"],
                d["session"].as_str().unwrap(),
                d["agent"].as_str().unwrap(),
                d["decider"].as_str().unwrap()
            );
            assert!(section.contains(&e), "missing {:?} in:\n{}", e, section);
        }
        // Confusion rows.
        let cols: Vec<&str> = q["confusion"]["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        for (label, row) in q["confusion"]["rows"].as_object().unwrap() {
            let want: Vec<String> = std::iter::once(label.clone())
                .chain(cols.iter().map(|c| row[*c].to_string()))
                .collect();
            assert!(
                section
                    .lines()
                    .any(|l| l.split_whitespace().collect::<Vec<_>>() == want),
                "no confusion row {:?} in:\n{}",
                want,
                section
            );
        }
    }
    assert!(table.contains("66.7% of well-formed answers, 50.0% of all consultations"));
}

#[test]
fn coverage_ignores_unavailable_and_error_consultations_but_rates_do_not() {
    let h = Harness::new(&standard("shadow", "shadow"));
    let mut base = Vec::new();
    for (seq, w, at) in [
        (1, "proceed", true),
        (2, "proceed", true),
        (3, "exit", true),
        (4, "exit", false),
    ] {
        base.push(consulted(Some("s"), seq, "h", "not_applied", Some(w), at));
    }
    write_ledger(&h, &base);
    let before = report_json(&h, &[]);
    let b = question(&before, "h").clone();

    let mut more = base.clone();
    more.push(consulted(
        Some("s"),
        5,
        "h",
        "input_unavailable",
        None,
        false,
    ));
    more.push(consulted(Some("s"), 6, "h", "error", None, false));
    write_ledger(&h, &more);
    let after = report_json(&h, &[]);
    let a = question(&after, "h");

    for v in ["proceed", "exit"] {
        assert_eq!(
            value(a, v)["coverage"]["well_formed"],
            value(&b, v)["coverage"]["well_formed"],
            "{}",
            v
        );
    }
    assert_eq!(f(&value(a, "proceed")["coverage"]["well_formed"]), 0.5);
    assert_eq!(f(&b["rates"]["error"]), 0.0);
    assert_eq!(f(&a["rates"]["error"]), 1.0 / 6.0);
    // Fallback counts every consultation not applied; add an applied one to
    // the base so the rise is visible.
    let mut applied_base = base.clone();
    applied_base.push(consulted(
        Some("s"),
        7,
        "h",
        "applied",
        Some("proceed"),
        true,
    ));
    write_ledger(&h, &applied_base);
    let fb_before = f(&question(&report_json(&h, &[]), "h")["rates"]["fallback"]);
    applied_base.push(consulted(
        Some("s"),
        5,
        "h",
        "input_unavailable",
        None,
        false,
    ));
    applied_base.push(consulted(Some("s"), 6, "h", "error", None, false));
    write_ledger(&h, &applied_base);
    let fb_after = f(&question(&report_json(&h, &[]), "h")["rates"]["fallback"]);
    assert_eq!(fb_before, 4.0 / 5.0);
    assert_eq!(fb_after, 6.0 / 7.0);
    assert!(fb_after > fb_before);
}

#[test]
fn latency_percentiles_exclude_input_unavailable() {
    let h = Harness::new(&standard("shadow", "shadow"));
    let mut lines = Vec::new();
    for (seq, ms) in [(1, 10), (2, 20), (3, 30)] {
        let mut c = consulted(Some("s"), seq, "h", "not_applied", Some("proceed"), true);
        c["latency_ms"] = json!(ms);
        lines.push(c);
    }
    let mut iu = consulted(Some("s"), 4, "h", "input_unavailable", None, false);
    iu["latency_ms"] = json!(99_999);
    lines.push(iu);
    write_ledger(&h, &lines);
    let r = report_json(&h, &[]);
    let q = question(&r, "h");
    assert_eq!(q["consultations"], 4);
    assert_eq!(q["latency_ms"], json!({"samples": 3, "p50": 20, "p95": 30}));
}

#[test]
fn a_null_session_id_is_counted_but_never_paired() {
    let h = Harness::new(&standard("shadow", "shadow"));
    write_ledger(
        &h,
        &[
            consulted(None, 3, "h", "not_applied", Some("proceed"), true),
            answered(None, 3, json!("exit")),
            answered(Some("s"), 3, json!("exit")),
        ],
    );
    let r = report_json(&h, &[]);
    let q = question(&r, "h");
    assert_eq!(q["consultations"], 1);
    assert_eq!(q["paired"], 0);
    assert_eq!(q["disagreements"], json!([]));
    assert_eq!(r["header"]["null_session_id"], 1);
    assert_eq!(r["header"]["orphaned_answered"], 2);
}

#[test]
fn later_answers_win_duplicates_are_counted_once_and_orphans_are_counted() {
    let h = Harness::new(&standard("shadow", "shadow"));
    write_ledger(
        &h,
        &[
            consulted(Some("s"), 1, "h", "not_applied", Some("proceed"), true),
            answered(Some("s"), 1, json!("exit")),
            consulted(Some("s"), 1, "h", "not_applied", Some("proceed"), true),
            answered(Some("s"), 1, json!("proceed")),
            answered(Some("s"), 42, json!("exit")),
        ],
    );
    let r = report_json(&h, &[]);
    assert_eq!(r["header"]["consulted"], 2);
    assert_eq!(r["header"]["duplicate_consulted"], 1);
    assert_eq!(r["header"]["answered"], 3);
    assert_eq!(r["header"]["orphaned_answered"], 1);
    let q = question(&r, "h");
    assert_eq!(q["consultations"], 1);
    assert_eq!(q["paired"], 1);
    assert_eq!(q["confusion"]["rows"]["proceed"]["proceed"], 1);
    assert_eq!(q["confusion"]["rows"]["exit"]["proceed"], 0);
    assert_eq!(f(&value(q, "proceed")["recall"]), 1.0);
}

#[test]
fn malformed_lines_are_skipped_and_counted_and_the_report_is_full() {
    let h = Harness::new(&standard("shadow", "shadow"));
    let good = metrics_ledger();
    let clean = {
        write_ledger(&h, &good);
        report_json(&h, &[])
    };

    let mut missing_key = consulted(Some("s9"), 1, "ha", "not_applied", Some("proceed"), true);
    missing_key.as_object_mut().unwrap().remove("visit_seq");
    let mut body = String::new();
    for (i, l) in good.iter().enumerate() {
        body.push_str(&format!("{}\n", l));
        if i == 2 {
            body.push_str("{this is not json\n");
            body.push_str(&format!("{}\n", missing_key));
            body.push_str("{\"kind\":\"retracted\",\"v\":2}\n");
        }
    }
    body.push_str(&answered(Some("s1"), 8, json!("exit")).to_string());
    std::fs::write(ledger_file(&h), body).unwrap();

    let r = report_json(&h, &[]);
    assert_eq!(r["header"]["skipped_malformed"], 3);
    assert_eq!(r["header"]["unknown_kind"], 1);
    assert_eq!(r["questions"], clean["questions"]);

    let out = report(&h, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    let table = String::from_utf8_lossy(&out.stdout);
    let header: String = table.lines().take(4).collect::<Vec<_>>().join("\n");
    assert!(
        header.contains("skipped 3 malformed, 1 unknown kind"),
        "{}",
        header
    );
}

/// `n` consultations with `proceed` in auto, `covered` of them confident.
fn auto_ledger(n: u64, covered: u64) -> Vec<Value> {
    (0..n)
        .map(|i| {
            let (w, at) = if i < covered {
                ("proceed", true)
            } else {
                ("proceed", false)
            };
            let mut c = consulted(Some("s"), i + 1, "h", "not_applied", Some(w), at);
            c["fields"]["verdict"]["modes"]["proceed"] = json!("auto");
            c
        })
        .collect()
}

#[test]
fn low_coverage_is_flagged_only_past_its_boundaries() {
    let h = Harness::new(&standard("shadow", "shadow"));
    for (n, covered, flagged) in [(30, 8, true), (29, 8, false), (30, 9, false), (31, 9, true)] {
        write_ledger(&h, &auto_ledger(n, covered));
        let r = report_json(&h, &[]);
        let flags = question(&r, "h")["flags"].as_array().unwrap().clone();
        assert_eq!(
            flags.iter().any(|f| f["name"] == "low_coverage"),
            flagged,
            "{} consultations, {} covered: {:?}",
            n,
            covered,
            flags
        );
    }
    // A question with no value in auto is never flagged.
    let lines: Vec<Value> = (0..40)
        .map(|i| consulted(Some("s"), i + 1, "h", "not_applied", Some("proceed"), false))
        .collect();
    write_ledger(&h, &lines);
    assert_eq!(question(&report_json(&h, &[]), "h")["flags"], json!([]));
}

#[test]
fn the_reported_hash_follows_the_declaration_not_modes_or_thresholds() {
    let base = standard("shadow", "shadow");
    let variants: Vec<(&str, String, bool)> = vec![
        (
            "value description",
            base.replace("checkable criteria.", "checkable criteria, now."),
            true,
        ),
        (
            "question",
            base.replace("clear enough to implement?", "clear enough to build?"),
            true,
        ),
        (
            "escape description",
            base.replace("Missing, truncated, or unjudgeable.", "Cannot be judged."),
            true,
        ),
        (
            "max_bytes",
            base.replace(
                "{context: outline.md, label: outline_item}",
                "{context: outline.md, label: outline_item, max_bytes: 4096}",
            ),
            true,
        ),
        (
            "threshold",
            base.replace("mode: shadow}", "mode: shadow, threshold: 0.95}"),
            false,
        ),
        ("mode", standard("auto", "never"), false),
    ];

    let hash_of = |tpl: &str| -> String {
        let h = Harness::new(tpl);
        write_fixtures(&h, &fixture_body(&["proceed"]));
        h.stub.set_default(say("proceed"));
        let fx = fixtures_ok(&h, &[]);
        assert_eq!(h.stub.request_count(), 1);
        fx["declaration_hash"].as_str().unwrap().to_string()
    };
    let original = hash_of(&base);
    assert_eq!(original, std_hash());
    for (what, tpl, changes) in variants {
        assert_ne!(tpl, base, "{} variant didn't change the template", what);
        let got = hash_of(&tpl);
        assert_eq!(got != original, changes, "changing the {}", what);
    }
}

// ---------------------------------------------------------------------------
// Custom-endpoint exclusion
// ---------------------------------------------------------------------------

#[test]
fn custom_endpoint_pairs_count_only_with_the_flag() {
    let hash = std_hash();
    let mut s = baseline(&hash);
    for l in s.ledger.iter_mut() {
        if l["kind"] == "consulted" {
            l["endpoint_origin"] = json!("env");
        }
    }
    let fx = run_scenario(&s, &[]);
    let p = fx_value(&fx, "proceed");
    assert_eq!(p["eligible"], false);
    assert!(
        failed_conditions(p).contains(&"ledger_pairs".to_string()),
        "{}",
        p
    );
    assert!(
        reasons(p).iter().any(|r| r
            .contains("30 from a custom endpoint are excluded without --include-custom-endpoints")),
        "{:?}",
        reasons(p)
    );

    let fx = run_scenario(&s, INCLUDE);
    let p = fx_value(&fx, "proceed");
    assert_eq!(p["eligible"], true, "{}", p);
    assert_eq!(p["status"], "eligible");
}

#[test]
fn a_fixture_run_against_the_stub_counts_only_with_the_flag() {
    let hash = std_hash();
    let fx = run_scenario(&baseline(&hash), &[]);
    assert_eq!(fx["endpoint_origin"], "env");
    assert_eq!(fx["endpoint_counted"], false);
    for v in fx["values"].as_array().unwrap() {
        assert_eq!(v["eligible"], false, "{}", v);
        assert_eq!(failed_conditions(v), vec!["counted_endpoint".to_string()]);
        assert!(
            reasons(v)[0].contains("custom endpoint from env")
                && reasons(v)[0].contains("--include-custom-endpoints"),
            "{:?}",
            reasons(v)
        );
    }

    // The table says why too.
    let h = Harness::new(&standard("shadow", "shadow"));
    write_ledger(&h, &eligible_ledger(&hash));
    write_fixtures(&h, &fixture_body(&baseline(&hash).labels));
    h.stub.set_default(say("proceed"));
    let mut cmd = opted_in(&h);
    cmd.args([
        "decider",
        "report",
        "--fixtures",
        "fx.jsonl",
        "--template",
        "template.md",
        "--state",
        "review",
    ]);
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    let table = String::from_utf8_lossy(&out.stdout);
    assert!(
        table.contains(
            "custom endpoint: excluded from eligibility without --include-custom-endpoints"
        ),
        "{}",
        table
    );
}

#[test]
fn metrics_keep_custom_endpoint_consultations_and_the_header_counts_them() {
    let h = Harness::new(&standard("shadow", "shadow"));
    let mut lines = pair("s", 1, "h", "proceed", "proceed");
    let mut env = pair("s", 2, "h", "exit", "exit");
    env[0]["endpoint_origin"] = json!("env");
    let mut user = pair("s", 3, "h", "proceed", "exit");
    user[0]["endpoint_origin"] = json!("user");
    lines.extend(env);
    lines.extend(user);
    write_ledger(&h, &lines);

    let r = report_json(&h, &[]);
    assert_eq!(r["header"]["custom_endpoint_consultations"], 2);
    assert_eq!(r["header"]["excluded_from_eligibility"], 2);
    let q = question(&r, "h");
    assert_eq!(q["consultations"], 3);
    assert_eq!(q["paired"], 3);
    assert_eq!(q["counted_paired"], 1);
    assert_eq!(q["disagreements"].as_array().unwrap().len(), 1);
    assert_eq!(value(q, "proceed")["disagreements"], 1);
    assert_eq!(value(q, "proceed")["counted_disagreements"], 0);

    let r = report_json(&h, INCLUDE);
    assert_eq!(r["header"]["excluded_from_eligibility"], 0);
    assert_eq!(question(&r, "h")["counted_paired"], 3);

    let out = report(&h, &[]);
    let table = String::from_utf8_lossy(&out.stdout);
    assert!(
        table.contains("2 consultations from a custom endpoint; 2 excluded from eligibility"),
        "{}",
        table
    );
}

// ---------------------------------------------------------------------------
// The fixture runner: opt-in, validation, payloads, and the network
// ---------------------------------------------------------------------------

const NEEDS_OPT_IN: &str = "fixture runs need an opted-in decider";

fn assert_refused(h: &Harness, cmd: Command, case: &str) {
    let out = run_fixtures(cmd, INCLUDE);
    assert_eq!(out.status.code(), Some(2), "{}: {}", case, describe(&out));
    assert!(out.stdout.is_empty(), "{}: {}", case, describe(&out));
    assert!(
        stderr(&out).contains(NEEDS_OPT_IN),
        "{}: {}",
        case,
        describe(&out)
    );
    assert_eq!(h.stub.request_count(), 0, "{}", case);
}

fn refusal_harness() -> Harness {
    let h = Harness::new(&standard("shadow", "shadow"));
    write_fixtures(&h, &fixture_body(&["proceed", "exit"]));
    h.stub.set_default(say("proceed"));
    h
}

#[test]
fn no_opt_in_at_all_is_refused() {
    let h = refusal_harness();
    assert_refused(&h, h.koto(), "nothing set");
    let mut cmd = h.koto();
    cmd.env("KOTO_DECIDER", "off");
    cmd.env("KOTO_DECIDER_API_KEY", KEY);
    cmd.env("KOTO_DECIDER_ENDPOINT", h.stub.url());
    assert_refused(&h, cmd, "KOTO_DECIDER=off");
}

#[test]
fn shadow_without_a_key_is_refused() {
    let h = refusal_harness();
    let mut cmd = h.koto();
    cmd.env("KOTO_DECIDER", "shadow");
    cmd.env("KOTO_DECIDER_ENDPOINT", h.stub.url());
    assert_refused(&h, cmd, "no key");
}

#[test]
fn a_user_key_is_never_sent_to_an_env_endpoint() {
    let h = refusal_harness();
    h.user_config(&format!(
        "[decider]\nmode = \"auto\"\napi_key = \"{}\"\n",
        KEY
    ));
    let mut cmd = h.koto();
    cmd.env("KOTO_DECIDER_ENDPOINT", h.stub.url());
    assert_refused(&h, cmd, "same-layer rule");
}

#[test]
fn a_project_mode_off_lowers_the_user_mode() {
    let h = refusal_harness();
    h.user_config(&format!(
        "[decider]\nmode = \"auto\"\napi_key = \"{}\"\nendpoint = \"{}\"\n",
        KEY,
        h.stub.url()
    ));
    h.project_config("[decider]\nmode = \"off\"\n");
    assert_refused(&h, h.koto(), "project off");

    // The same user config without the project minimum reaches the stub.
    std::fs::remove_file(h.dir.join(".koto").join("config.toml")).unwrap();
    let out = run_fixtures(h.koto(), INCLUDE);
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert_eq!(h.stub.request_count(), 2);
    assert_eq!(json_out(&out)["fixtures"]["endpoint_origin"], "user");
}

#[test]
fn a_project_mode_alone_does_not_opt_in() {
    let h = refusal_harness();
    h.project_config("[decider]\nmode = \"shadow\"\n");
    let mut cmd = h.koto();
    cmd.env("KOTO_DECIDER_API_KEY", KEY);
    cmd.env("KOTO_DECIDER_ENDPOINT", h.stub.url());
    assert_refused(&h, cmd, "project shadow, user unset");
    let mut cmd = h.koto();
    cmd.env("KOTO_DECIDER", "off");
    cmd.env("KOTO_DECIDER_API_KEY", KEY);
    cmd.env("KOTO_DECIDER_ENDPOINT", h.stub.url());
    assert_refused(&h, cmd, "project shadow, env off");
}

#[test]
fn an_env_key_and_endpoint_in_shadow_reach_the_stub() {
    let h = refusal_harness();
    let out = run_fixtures(opted_in(&h), INCLUDE);
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert_eq!(h.stub.request_count(), 2);
    let fx = &json_out(&out)["fixtures"];
    assert_eq!(fx["cases"], 2);
    assert_eq!(fx["endpoint_origin"], "env");
    assert_eq!(
        h.stub.last_request().unwrap().header("authorization"),
        Some(format!("Bearer {}", KEY).as_str())
    );
}

#[test]
fn a_bad_fixture_line_aborts_before_any_request() {
    let good = |i: usize| -> Value {
        json!({"id": format!("c{}", i), "inputs": {"outline_item": "x", "plan_path": "p"}, "expected": "proceed"})
    };
    let big = "x".repeat(8193);
    let bad: Vec<(&str, Value)> = vec![
        (
            "unknown key",
            json!({"id": "b", "inputs": {"outline_item": "x", "plan_path": "p"}, "expected": "proceed", "note": "n"}),
        ),
        (
            "undeclared expected",
            json!({"id": "b", "inputs": {"outline_item": "x", "plan_path": "p"}, "expected": "maybe"}),
        ),
        (
            "missing label",
            json!({"id": "b", "inputs": {"outline_item": "x"}, "expected": "proceed"}),
        ),
        (
            "extra label",
            json!({"id": "b", "inputs": {"outline_item": "x", "plan_path": "p", "diff": "d"}, "expected": "exit"}),
        ),
        (
            "over max_bytes",
            json!({"id": "b", "inputs": {"outline_item": big, "plan_path": "p"}, "expected": "exit"}),
        ),
    ];
    for (what, line) in bad {
        let h = Harness::new(&standard("shadow", "shadow"));
        h.stub.set_default(say("proceed"));
        let body = format!("{}\n{}\n{}\n", good(1), good(2), line);
        write_fixtures(&h, &body);
        let out = run_fixtures(opted_in(&h), INCLUDE);
        assert_eq!(out.status.code(), Some(2), "{}: {}", what, describe(&out));
        assert!(
            stderr(&out).contains("line 3"),
            "{}: {}",
            what,
            describe(&out)
        );
        assert!(out.stdout.is_empty(), "{}", what);
        assert_eq!(h.stub.request_count(), 0, "{}", what);
    }

    // A string where a boolean field wants a JSON boolean.
    let h = Harness::new(BOOLEAN);
    let body = format!(
        "{}\n{}\n",
        json!({"inputs": {"diff": "d"}, "expected": true}),
        json!({"inputs": {"diff": "d"}, "expected": "true"})
    );
    write_fixtures(&h, &body);
    let mut cmd = opted_in(&h);
    cmd.args([
        "decider",
        "report",
        "--fixtures",
        "fx.jsonl",
        "--template",
        "template.md",
        "--state",
        "check",
    ]);
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", describe(&out));
    assert!(stderr(&out).contains("line 2"), "{}", describe(&out));
    assert!(stderr(&out).contains("JSON boolean"), "{}", describe(&out));
    assert_eq!(h.stub.request_count(), 0);
}

#[test]
fn a_state_without_one_declared_field_is_refused_by_name() {
    let h = Harness::new(&standard("shadow", "shadow"));
    write_fixtures(&h, &fixture_body(&["proceed"]));
    let mut cmd = opted_in(&h);
    cmd.args([
        "decider",
        "report",
        "--fixtures",
        "fx.jsonl",
        "--template",
        "template.md",
        "--state",
        "work",
    ]);
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", describe(&out));
    assert!(stderr(&out).contains("\"work\""), "{}", describe(&out));

    let two = standard("shadow", "shadow").replace(
        "            - {var: PLAN_DOC, label: plan_path}\n    transitions:",
        "            - {var: PLAN_DOC, label: plan_path}\n      scope:\n        type: boolean\n        required: false\n        description: The item is small.\n        decider:\n          answers:\n            true: {description: \"Small.\"}\n            false: {description: \"Large.\"}\n          inputs:\n            - {context: outline.md, label: outline_item}\n    transitions:",
    );
    assert_ne!(two, standard("shadow", "shadow"));
    let h = Harness::new(&two);
    write_fixtures(&h, &fixture_body(&["proceed"]));
    let out = run_fixtures(opted_in(&h), &[]);
    assert_eq!(out.status.code(), Some(2), "{}", describe(&out));
    let err = stderr(&out);
    assert!(
        err.contains("\"review\"") && err.contains("--field"),
        "{}",
        err
    );
    assert_eq!(h.stub.request_count(), 0);

    // Naming the field runs it.
    h.stub.set_default(say("proceed"));
    let out = run_fixtures(opted_in(&h), &["--field", "verdict"]);
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert_eq!(h.stub.request_count(), 1);
}

#[test]
fn fixture_payloads_are_byte_identical_to_the_runtime_payloads() {
    let h = Harness::new(&standard("shadow", "shadow"));
    h.ready();
    h.stub.push(say("proceed"));
    let out = h.next_mode("shadow");
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(h.stub.request_count(), 1);
    let runtime = h.stub.requests()[0].clone();

    write_fixtures(
        &h,
        &format!(
            "{}\n",
            json!({"id": "same", "inputs": {"outline_item": OUTLINE, "plan_path": PLAN_DOC}, "expected": "proceed"})
        ),
    );
    h.stub.push(say("proceed"));
    let out = run_fixtures(opted_in(&h), INCLUDE);
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert_eq!(h.stub.request_count(), 2);
    let fixture = h.stub.requests()[1].clone();
    assert_eq!(
        fixture.body,
        runtime.body,
        "{}\nvs\n{}",
        fixture.body_str(),
        runtime.body_str()
    );
    assert_eq!(fixture.path, runtime.path);
    assert_eq!(fixture.method, runtime.method);
}

#[test]
fn a_refused_connection_or_a_rejected_key_aborts_the_run() {
    let h = refusal_harness();
    let mut cmd = h.koto_mode("shadow");
    cmd.env("KOTO_DECIDER_ENDPOINT", closed_url());
    let out = run_fixtures(cmd, INCLUDE);
    assert_eq!(out.status.code(), Some(2), "{}", describe(&out));
    assert!(
        stderr(&out).contains("fixture runs need network access to the configured endpoint"),
        "{}",
        describe(&out)
    );
    assert!(out.stdout.is_empty());

    let h = refusal_harness();
    h.stub.push(Reply::status(401));
    let out = run_fixtures(opted_in(&h), INCLUDE);
    assert_eq!(out.status.code(), Some(2), "{}", describe(&out));
    assert!(
        stderr(&out).contains("fixture runs need network access to the configured endpoint"),
        "{}",
        describe(&out)
    );
    assert!(out.stdout.is_empty());
    assert_eq!(h.stub.request_count(), 1);
}

#[test]
fn a_timeout_or_a_malformed_answer_is_one_no_answer_case() {
    let h = Harness::new(&standard("shadow", "shadow"));
    h.user_config("[decider]\ntimeout_ms = 300\n");
    write_fixtures(&h, &fixture_body(&["proceed", "exit", "proceed"]));
    h.stub
        .push(say("proceed").delay(Duration::from_millis(1500)));
    h.stub.push(Reply::raw(200, "{\"not\": \"an answer\"}"));
    h.stub.push(say("proceed"));
    let fx = fixtures_ok(&h, INCLUDE);
    let answers: Vec<&str> = fx["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["answer"].as_str().unwrap())
        .collect();
    assert_eq!(answers, vec!["no_answer", "no_answer", "proceed"]);
    assert_eq!(fx["results"][0]["error_class"], "timeout");
    assert_eq!(fx["no_answer"], 2);
}

// ---------------------------------------------------------------------------
// Promotion eligibility boundaries
// ---------------------------------------------------------------------------

#[test]
fn the_baseline_is_eligible_and_exits_zero() {
    let hash = std_hash();
    let fx = run_scenario(&baseline(&hash), INCLUDE);
    assert_eq!(fx["declaration_hash"], hash);
    assert_eq!(fx["cases"], 40);
    assert_eq!(fx["ledger"]["counted_paired"], 30);
    for v in ["proceed", "exit"] {
        let x = fx_value(&fx, v);
        assert_eq!(x["eligible"], true, "{}", x);
        assert_eq!(x["status"], "eligible");
        assert_eq!(x["reasons"], json!([]));
    }
    assert_eq!(f(&fx["macro_recall"]), 1.0);
    assert_eq!(f(&fx["majority_baseline"]), 0.5);
}

/// Run `s` and assert `proceed` is ineligible for exactly `condition`.
fn assert_only_fails(s: &Scenario, condition: &str, wording: &str) {
    let fx = run_scenario(s, INCLUDE);
    let p = fx_value(&fx, "proceed");
    assert_eq!(p["eligible"], false, "{}", p);
    assert_eq!(p["status"], "ineligible");
    assert_eq!(failed_conditions(p), vec![condition.to_string()], "{}", p);
    assert!(reasons(p)[0].contains(wording), "{:?}", reasons(p));
}

#[test]
fn nine_labelled_cases_are_not_enough() {
    let hash = std_hash();
    let mut s = baseline(&hash);
    s.labels = vec!["proceed"; 9];
    s.labels.extend(vec!["exit"; 31]);
    s.answers = s.labels.clone();
    assert_only_fails(
        &s,
        "labelled_cases",
        "at least 10 fixture cases labelled proceed (has 9)",
    );
}

#[test]
fn thirty_nine_cases_are_not_enough() {
    let hash = std_hash();
    let mut s = baseline(&hash);
    s.labels = vec!["proceed"; 20];
    s.labels.extend(vec!["exit"; 19]);
    s.answers = s.labels.clone();
    assert_only_fails(
        &s,
        "total_cases",
        "at least 40 fixture cases in total (has 39)",
    );
}

#[test]
fn twenty_nine_pairs_are_not_enough() {
    let hash = std_hash();
    let mut s = baseline(&hash);
    s.ledger.truncate(58);
    assert_only_fails(
        &s,
        "ledger_pairs",
        "at least 30 paired observations under the current declaration hash (has 29)",
    );
}

#[test]
fn two_disagreements_where_the_decider_chose_the_value_are_too_many() {
    let hash = std_hash();
    let mut s = baseline(&hash);
    for seq in [31, 32] {
        s.ledger
            .extend(pair("s-elig", seq, &hash, "proceed", "exit"));
    }
    assert_only_fails(
        &s,
        "ledger_disagreements",
        "at most 1 ledger disagreement where the decider chose proceed (has 2)",
    );

    // One is allowed.
    let mut one = baseline(&hash);
    one.ledger
        .extend(pair("s-elig", 31, &hash, "proceed", "exit"));
    let fx = run_scenario(&one, INCLUDE);
    assert_eq!(fx_value(&fx, "proceed")["eligible"], true);
}

#[test]
fn one_confident_false_positive_is_disqualifying() {
    let hash = std_hash();
    let mut s = baseline(&hash);
    s.answers[39] = "proceed";
    assert_only_fails(
        &s,
        "no_false_positives",
        "no fixture labelled otherwise is answered proceed at or above its threshold (1 were)",
    );

    // Below threshold isn't a false positive.
    let mut below = baseline(&hash);
    below.answers[39] = "below:proceed";
    let fx = run_scenario(&below, INCLUDE);
    assert_eq!(fx_value(&fx, "proceed")["eligible"], true);
}

#[test]
fn macro_recall_equal_to_the_majority_baseline_is_not_enough() {
    let hash = std_hash();
    let mut s = baseline(&hash);
    for a in s.answers.iter_mut().skip(20) {
        *a = "below:exit";
    }
    let fx = run_scenario(&s, INCLUDE);
    assert_eq!(f(&fx["macro_recall"]), 0.5);
    assert_eq!(f(&fx["majority_baseline"]), 0.5);
    let p = fx_value(&fx, "proceed");
    assert_eq!(
        failed_conditions(p),
        vec!["beats_majority_baseline".to_string()]
    );
    assert!(
        reasons(p)[0].contains("macro recall exceeds always choosing the most frequent label"),
        "{:?}",
        reasons(p)
    );
}

#[test]
fn one_no_answer_case_makes_every_value_ineligible() {
    let hash = std_hash();
    let h = Harness::new(&standard("shadow", "shadow"));
    let mut s = baseline(&hash);
    s.labels.push("proceed");
    write_ledger(&h, &s.ledger);
    write_fixtures(&h, &fixture_body(&s.labels));
    for a in &s.answers {
        h.stub.push(say(a));
    }
    h.stub.push(Reply::status(503));
    let fx = fixtures_ok(&h, INCLUDE);
    assert_eq!(fx["no_answer"], 1);
    for v in fx["values"].as_array().unwrap() {
        assert_eq!(v["eligible"], false, "{}", v);
        assert!(failed_conditions(v).contains(&"every_case_answered".to_string()));
    }
}

#[test]
fn pairs_under_an_older_hash_do_not_count() {
    let hash = std_hash();
    let mut s = baseline(&hash);
    s.ledger = eligible_ledger_in("s-old", "an-older-declaration-hash");
    s.ledger.extend(eligible_ledger(&hash).into_iter().take(58));
    let fx = run_scenario(&s, INCLUDE);
    assert_eq!(fx["ledger"]["counted_paired"], 29);
    let p = fx_value(&fx, "proceed");
    assert_eq!(failed_conditions(p), vec!["ledger_pairs".to_string()]);
}

#[test]
fn escape_cases_count_toward_the_total_and_can_zero_the_baseline() {
    let hash = std_hash();
    let mut s = baseline(&hash);
    s.labels = vec!["proceed"; 10];
    s.labels.extend(vec!["exit"; 10]);
    s.labels.extend(vec!["unclear"; 20]);
    s.answers = s.labels.clone();
    let fx = run_scenario(&s, INCLUDE);
    assert_eq!(fx["cases"], 40);
    assert_eq!(fx["labels"]["unclear"], 20);
    assert_eq!(f(&fx["majority_baseline"]), 0.0);
    for v in ["proceed", "exit"] {
        assert_eq!(fx_value(&fx, v)["eligible"], true, "{}", fx_value(&fx, v));
    }
    assert!(fx["values"]
        .as_array()
        .unwrap()
        .iter()
        .all(|v| v["value"] != "unclear"));

    // Only 5 escape cases still totals 40 with 35 others; the escape needs
    // no minimum of its own.
    let mut few = baseline(&hash);
    few.labels = vec!["proceed"; 18];
    few.labels.extend(vec!["exit"; 17]);
    few.labels.extend(vec!["unclear"; 5]);
    few.answers = few.labels.clone();
    let fx = run_scenario(&few, INCLUDE);
    assert_eq!(fx_value(&fx, "proceed")["eligible"], true);
}

const BOOLEAN: &str = r#"---
name: boolq
version: "1.0"
initial_state: check
variables:
  DIFF:
    description: Diff summary
    default: none
states:
  check:
    accepts:
      ready:
        type: boolean
        required: true
        description: The change is ready to merge.
        decider:
          answers:
            true: {description: "Ready.", mode: shadow}
            false: {description: "Not ready.", mode: shadow}
          inputs:
            - {var: DIFF, label: diff}
    transitions:
      - target: merge
        when:
          ready: true
      - target: fix
        when:
          ready: false
  merge:
    accepts:
      merged: {type: boolean, required: true, description: m}
    transitions:
      - target: done
        when: {merged: true}
  fix:
    accepts:
      fixed: {type: boolean, required: true, description: f}
    transitions:
      - target: check
        when: {fixed: true}
  done:
    terminal: true
---

## check

Check.

## merge

Merge.

## fix

Fix.

## done

Done.
"#;

fn noul(p: f64) -> Reply {
    answers(json!({"ready": {"type": "noul", "noul": p}}))
}

#[test]
fn a_boolean_declaration_needs_ten_of_each_side_and_reports_both() {
    let h = Harness::new(BOOLEAN);
    let hash = current_hash(&h, "check", "ready");
    let mut ledger = Vec::new();
    for i in 0..30u64 {
        let side = i % 2 == 0;
        let mut c = consulted(
            Some("sb"),
            i + 1,
            &hash,
            "not_applied",
            Some(&side.to_string()),
            true,
        );
        c["state"] = json!("check");
        c["fields"] = json!({"ready": {
            "declaration_hash": hash,
            "modes": {"true": "shadow", "false": "shadow"},
            "winning": side.to_string(),
            "confidence": 0.95,
            "threshold": 0.9,
            "at_threshold": true,
            "outcome": "shadow"
        }});
        let mut a = answered(Some("sb"), i + 1, json!(side));
        a["state"] = json!("check");
        a["values"] = json!({"ready": side});
        ledger.push(c);
        ledger.push(a);
    }
    write_ledger(&h, &ledger);

    let run = |trues: usize, falses: usize| -> Value {
        let lines: String = (0..trues)
            .map(|_| true)
            .chain((0..falses).map(|_| false))
            .map(|b| format!("{}\n", json!({"inputs": {"diff": "d"}, "expected": b})))
            .collect();
        write_fixtures(&h, &lines);
        for _ in 0..trues {
            h.stub.push(noul(0.97));
        }
        for _ in 0..falses {
            h.stub.push(noul(0.03));
        }
        let mut cmd = opted_in(&h);
        cmd.args([
            "decider",
            "report",
            "--json",
            "--include-custom-endpoints",
            "--fixtures",
            "fx.jsonl",
            "--template",
            "template.md",
            "--state",
            "check",
        ]);
        let out = cmd.output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
        json_out(&out)["fixtures"].clone()
    };

    let fx = run(10, 30);
    assert_eq!(fx["kind"], "boolean");
    let names: Vec<&str> = fx["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["value"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["true", "false"]);
    assert_eq!(fx_value(&fx, "true")["eligible"], true, "{}", fx);
    assert_eq!(fx_value(&fx, "false")["eligible"], true, "{}", fx);

    let fx = run(9, 31);
    let t = fx_value(&fx, "true");
    assert_eq!(failed_conditions(t), vec!["labelled_cases".to_string()]);
    assert_eq!(fx_value(&fx, "false")["eligible"], true);
}

#[test]
fn a_never_value_is_evaluated_noted_and_can_be_eligible() {
    let h = Harness::new(&standard("shadow", "never"));
    let hash = current_hash(&h, "review", "verdict");
    let s = baseline(&hash);
    let fx = run_scenario_in(&h, &s, INCLUDE);
    let e = fx_value(&fx, "exit");
    assert_eq!(e["template_mode"], "never");
    assert_eq!(e["note"], "(template: never)");
    assert_eq!(e["eligible"], true, "{}", e);
    assert_eq!(fx_value(&fx, "proceed")["note"], Value::Null);

    // The table carries the note.
    let h = Harness::new(&standard("shadow", "never"));
    write_ledger(&h, &s.ledger);
    write_fixtures(&h, &fixture_body(&s.labels));
    for a in &s.answers {
        h.stub.push(say(a));
    }
    let mut cmd = opted_in(&h);
    cmd.args([
        "decider",
        "report",
        "--include-custom-endpoints",
        "--fixtures",
        "fx.jsonl",
        "--template",
        "template.md",
        "--state",
        "review",
    ]);
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    let table = String::from_utf8_lossy(&out.stdout);
    assert!(
        table.contains("exit (template: never): eligible"),
        "{}",
        table
    );
}

// ---------------------------------------------------------------------------
// End to end over a ledger koto itself wrote
// ---------------------------------------------------------------------------

#[test]
fn the_report_pairs_records_written_by_koto_next() {
    let h = Harness::new(&standard("shadow", "shadow"));
    h.ready();
    h.stub.push(say("proceed"));
    let out = h.next_mode("shadow");
    assert!(out.status.success(), "{}", describe(&out));
    let out = h.next_with("shadow", r#"{"verdict": "exit"}"#);
    assert!(out.status.success(), "{}", describe(&out));

    let r = report_json(&h, &[]);
    let hash = current_hash(&h, "review", "verdict");
    let q = question(&r, &hash);
    assert_eq!(q["consultations"], 1);
    assert_eq!(q["paired"], 1);
    // The stub is an env endpoint.
    assert_eq!(q["counted_paired"], 0);
    assert_eq!(r["header"]["excluded_from_eligibility"], 1);
    assert_eq!(q["confusion"]["rows"]["exit"]["proceed"], 1);
    assert_eq!(q["disagreements"][0]["agent"], "exit");
    assert_eq!(q["disagreements"][0]["decider"], "proceed");
}

// ---------------------------------------------------------------------------
// A config file that fails to parse never echoes its content
// ---------------------------------------------------------------------------

/// A key only a broken config file holds. The toml parser quotes the
/// offending line in its message, so this must never reach any output.
const LEAKED: &str = "sk-LEAKME-123";

/// Config bodies that fail to parse on a line holding [`LEAKED`]: an
/// unterminated string (a syntax error) and a string where a table belongs
/// (a shape error, whose serde message quotes the value).
fn broken_configs() -> [(&'static str, String); 2] {
    [
        ("syntax", format!("[decider]\napi_key = \"{}\n", LEAKED)),
        ("shape", format!("[session]\ncloud = \"{}\"\n", LEAKED)),
    ]
}

/// Every command that loads a user or project config, each opted in
/// against the stub so none of them would reach a real provider.
fn config_commands(h: &Harness, user: bool) -> Vec<(String, Command)> {
    let mut out = Vec::new();
    let mut push = |name: &str, args: &[&str]| {
        let mut cmd = h.koto_mode("shadow");
        cmd.args(args);
        out.push((name.to_string(), cmd));
    };
    push("next", &["next", WF]);
    push("config list", &["config", "list"]);
    push("config list --json", &["config", "list", "--json"]);
    push("config get", &["config", "get", "decider.mode"]);
    let scope: &[&str] = if user { &["--user"] } else { &[] };
    let set: Vec<&str> = [&["config", "set"][..], scope, &["decider.mode", "off"][..]].concat();
    push("config set", &set);
    let unset: Vec<&str> = [&["config", "unset"][..], scope, &["decider.mode"][..]].concat();
    push("config unset", &unset);
    let report: Vec<&str> = [&["decider", "report"][..], &FIXTURE_ARGS[..]].concat();
    push("decider report --fixtures", &report);
    out
}

#[test]
fn a_config_parse_error_never_prints_the_file_content() {
    for user in [true, false] {
        for (kind, body) in broken_configs() {
            let h = Harness::new(&standard("shadow", "shadow"));
            h.ready();
            write_fixtures(&h, &fixture_body(&["proceed"]));
            h.stub.set_default(say("proceed"));
            let (layer, path) = if user {
                h.user_config(&body);
                ("user config", h.home().join(".koto").join("config.toml"))
            } else {
                h.project_config(&body);
                ("project config", h.dir.join(".koto").join("config.toml"))
            };
            // A shape-broken file is still valid TOML, so a command may
            // rewrite it (the machine id) and move the bad line; only the
            // syntax case has a fixed position.
            let line = if kind == "syntax" {
                "at line 2, column 25"
            } else {
                " at line "
            };

            for (name, mut cmd) in config_commands(&h, user) {
                let case = format!("{} {} {}", layer, kind, name);
                let out = cmd.output().unwrap();
                let so = String::from_utf8_lossy(&out.stdout);
                let se = stderr(&out);
                assert!(!so.contains(LEAKED), "{}: stdout leaks: {}", case, so);
                assert!(!se.contains(LEAKED), "{}: stderr leaks: {}", case, se);
                assert!(!se.contains("sk-"), "{}: {}", case, se);
                // `config set/unset` edit the raw TOML, so a well-formed
                // file with a wrong-shaped value doesn't stop them.
                if kind == "shape" && name.starts_with("config set")
                    || kind == "shape" && name.starts_with("config unset")
                {
                    assert_eq!(out.status.code(), Some(0), "{}: {}", case, describe(&out));
                    continue;
                }
                assert_ne!(out.status.code(), Some(0), "{}: {}", case, describe(&out));
                assert!(se.contains(line), "{}: no position: {}", case, se);
                // The path is named; `config set/unset` read the file
                // directly, so it's shown as given.
                let shown = if user {
                    path.display().to_string()
                } else {
                    ".koto/config.toml".to_string()
                };
                assert!(se.contains(&shown), "{}: no path: {}", case, se);
            }
            assert_eq!(h.stub.request_count(), 0, "{} {}", layer, kind);
        }
    }
}
