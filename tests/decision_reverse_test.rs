//! Reversing decisions through the CLI, and `koto next` blocking a live
//! workflow on a reversed gate pass.
//!
//! Every test runs koto from inside a fixture workspace whose session map
//! gives each session a chain: by default a coordinator that is a root, two
//! workers it dispatched, and a session with no entry at all. `HOME` points
//! at the test's own directory, so the store is private to it.

use assert_cmd::Command;
use assert_fs::TempDir;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const W: &str = "9c1e0000-0000-4000-8000-000000000001";
const W2: &str = "9c1e0000-0000-4000-8000-000000000002";
const COORD: &str = "4b2d0000-0000-4000-8000-00000000000c";
const LONE: &str = "77770000-0000-4000-8000-000000000077";

struct Ws {
    dir: TempDir,
}

impl Ws {
    /// A workspace where COORD is an observed root and W and W2 are its
    /// workers, every link observed: complete chains.
    fn new() -> Self {
        Self::rooted_at("none")
    }

    /// The same, with COORD's own parent given as `root` (`none` or
    /// `person:<handle>`).
    fn rooted_at(root: &str) -> Self {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("ws/.niwa/sessions")).unwrap();
        std::fs::create_dir_all(dir.path().join("ws/repo")).unwrap();
        std::fs::write(dir.path().join("ws/.niwa/workspace.toml"), "").unwrap();
        let ws = Ws { dir };
        ws.entry(
            COORD,
            json!({"lineage": {"v": 1, "parent": root, "source": {"parent": "observed"}}}),
        );
        for worker in [W, W2] {
            ws.entry(
                worker,
                json!({"lineage": {"v": 1, "parent": format!("session:{}", COORD),
                    "chain": [{"session": COORD, "parent": root}],
                    "source": {"parent": "observed", "chain": "observed"}}}),
            );
        }
        ws
    }

    fn home(&self) -> &Path {
        self.dir.path()
    }

    fn cwd(&self) -> PathBuf {
        self.dir.path().join("ws/repo")
    }

    fn store(&self) -> PathBuf {
        self.home().join(".koto/decisions")
    }

    fn entry(&self, id: &str, body: Value) {
        let p = self
            .dir
            .path()
            .join("ws/.niwa/sessions")
            .join(format!("{}.json", id));
        std::fs::write(p, body.to_string()).unwrap();
    }

    fn env(&self, cmd: &mut std::process::Command, session: Option<&str>) {
        cmd.current_dir(self.cwd());
        cmd.env("HOME", self.home());
        cmd.env("KOTO_SESSIONS_BASE", self.home().join("sessions"));
        for name in [
            "CLAUDE_CODE_SESSION_ID",
            "KOTO_AGENT_ID",
            "CLAUDE_CODE_SESSION_ATTENDED",
            "KOTO_WORKFLOWS_DIR",
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GH_ENTERPRISE_TOKEN",
            "GITHUB_ENTERPRISE_TOKEN",
        ] {
            cmd.env_remove(name);
        }
        if let Some(s) = session {
            cmd.env("CLAUDE_CODE_SESSION_ID", s);
        }
    }

    fn koto(&self, session: Option<&str>) -> Command {
        let mut std_cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("koto"));
        self.env(&mut std_cmd, session);
        Command::from_std(std_cmd)
    }

    fn run(&self, session: Option<&str>, args: &[&str]) -> (i32, Value) {
        let out = self.koto(session).args(args).output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let json: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
            panic!(
                "koto {:?} printed non-JSON ({}): stdout={} stderr={}",
                args,
                e,
                stdout,
                String::from_utf8_lossy(&out.stderr)
            )
        });
        (out.status.code().unwrap_or(-1), json)
    }

    fn ok(&self, session: Option<&str>, args: &[&str]) -> Value {
        let (code, json) = self.run(session, args);
        assert_eq!(code, 0, "koto {:?} failed: {}", args, json);
        json
    }

    /// Record and return the decision's identifier.
    fn record(&self, session: Option<&str>, args: &[&str]) -> String {
        let mut full = vec!["decisions", "record"];
        full.extend_from_slice(args);
        self.ok(session, &full)["id"].as_str().unwrap().to_string()
    }

    fn reverse(&self, session: Option<&str>, id: &str, extra: &[&str]) -> (i32, Value) {
        let mut full = vec!["decisions", "reverse", id];
        full.extend_from_slice(extra);
        self.run(session, &full)
    }

    fn show(&self, id: &str) -> Value {
        self.ok(Some(W2), &["decisions", "show", id])
    }

    fn list(&self, args: &[&str]) -> Vec<Value> {
        let mut full = vec!["decisions", "list"];
        full.extend_from_slice(args);
        self.ok(Some(W2), &full)["items"]
            .as_array()
            .unwrap()
            .clone()
    }

    fn refused(&self) -> Vec<Value> {
        self.list(&["--kind", "refused"])
            .into_iter()
            .map(|i| i["decision"].clone())
            .collect()
    }
}

const JUDGMENT: &[&str] = &[
    "--kind",
    "judgment",
    "--what",
    "flaky test is a false positive",
];
const ACCEPT: &[&str] = &[
    "--kind",
    "accept",
    "--outcome",
    "accepted",
    "--of",
    "document",
    "--repo",
    "o/r",
    "--path",
    "docs/a.md",
    "--commit",
    "abc",
];
const MERGE: &[&str] = &[
    "--kind", "merge", "--repo", "o/r", "--pr", "12", "--commit", "abc",
];

fn with<'a>(base: &[&'a str], extra: &[&'a str]) -> Vec<&'a str> {
    base.iter().chain(extra).copied().collect()
}

// ----- Reversing and authority -----

#[test]
fn the_coordinator_reverses_an_acceptance_a_gate_pass_and_a_merge() {
    let ws = Ws::new();
    let accept = ws.record(Some(W), ACCEPT);
    let pass = ws.record(
        Some(W),
        &[
            "--kind",
            "gate-pass",
            "--workflow",
            "wf",
            "--state",
            "review",
            "--gate",
            "panel",
        ],
    );
    let merge = ws.record(Some(W), MERGE);
    let link = "https://example.com/o/r/pull/13";
    for (id, extra) in [
        (&accept, vec![]),
        (&pass, vec![]),
        (&merge, vec!["--compensating", link]),
    ] {
        let (code, out) = ws.reverse(Some(COORD), id, &extra);
        assert_eq!(code, 0, "{}", out);
        let r = &out["decision"];
        assert_eq!(r["kind"], "reverse");
        assert_eq!(r["reverses"], json!(id));
        assert_eq!(r["subject"]["decision"], json!(id));
        assert_eq!(r["decided_by"]["kind"], "session");
        assert_eq!(r["decided_by"]["session"][0]["value"], COORD);
        assert_eq!(r["authority"]["kind"], "dispatch");
        assert_eq!(r["authority"]["standing"], "verified");
        assert!(r["original"]["kind"].is_string());
        assert_eq!(ws.show(id)["status"], "reversed");
        assert_eq!(ws.show(id)["reversed_by"], out["id"]);
    }
    let r = ws.show(&merge)["reversal"].clone();
    assert_eq!(r["compensating"], json!({"exists": true, "link": link}));

    let bare = ws.record(Some(W), MERGE);
    let (_, out) = ws.reverse(Some(COORD), &bare, &[]);
    assert_eq!(out["decision"]["compensating"], json!({"exists": false}));
}

#[test]
fn reversing_an_acceptance_lists_the_acceptances_that_rely_on_it() {
    let ws = Ws::new();
    let first = ws.record(Some(W), ACCEPT);
    let mut args: Vec<&str> = ACCEPT
        .iter()
        .map(|a| if *a == "docs/a.md" { "docs/b.md" } else { *a })
        .collect();
    args.extend(["--basis", first.as_str()]);
    let later = ws.record(Some(W), &args);
    let (code, out) = ws.reverse(Some(COORD), &first, &[]);
    assert_eq!(code, 0, "{}", out);
    assert_eq!(out["affected"], json!([later]));
    assert_eq!(out["effect"]["meaning"], "not_accepted");
    assert_eq!(ws.show(&first)["affected"], json!([later]));
}

#[test]
fn a_worker_reverses_its_own_judgment_with_verified_standing() {
    let ws = Ws::new();
    let j = ws.record(Some(W), JUDGMENT);
    let (code, out) = ws.reverse(Some(W), &j, &[]);
    assert_eq!(code, 0, "{}", out);
    assert_eq!(out["standing"], "verified");
}

#[test]
fn a_session_cant_reverse_a_persons_decision_and_the_root_person_can() {
    let ws = Ws::rooted_at("person:ana");
    let d = ws.record(
        Some(W),
        &with(JUDGMENT, &["--by", "person:ana", "--surface", "terminal"]),
    );
    let (code, out) = ws.reverse(Some(COORD), &d, &[]);
    assert_eq!(code, 1, "{}", out);
    assert_eq!(out["reason"], "session_over_person");
    assert!(out["refused_id"].is_string());

    let (code, out) = ws.reverse(
        Some(W),
        &d,
        &["--by", "person:ana", "--surface", "terminal"],
    );
    assert_eq!(code, 0, "{}", out);
    assert_eq!(out["decision"]["decided_by"]["kind"], "person");
}

#[test]
fn a_maintainer_at_the_root_reverses_a_workers_judgment_from_the_terminal() {
    let ws = Ws::rooted_at("person:ana");
    let j = ws.record(Some(W), JUDGMENT);
    let (code, out) = ws.reverse(
        Some(COORD),
        &j,
        &["--by", "person:ana", "--surface", "terminal"],
    );
    assert_eq!(code, 0, "{}", out);
    // A handle is always handed.
    assert_eq!(out["standing"], "asserted");
    let (code, _) = ws.reverse(
        Some(COORD),
        &ws.record(Some(W), JUDGMENT),
        &["--by", "person:bo", "--surface", "terminal"],
    );
    assert_eq!(code, 1);
}

#[test]
fn an_identifier_not_on_the_record_is_refused_and_recorded() {
    let ws = Ws::new();
    let missing = "d-202601-00000000000000000000000000000000";
    let (code, out) = ws.reverse(Some(COORD), missing, &[]);
    assert_eq!(code, 1, "{}", out);
    assert_eq!(out["reason"], "not_on_record");
    let refused = ws.refused();
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0]["subject"]["attempted_kind"], "reverse");
    assert_eq!(refused[0]["subject"]["attempted"]["decision"], missing);
    assert_eq!(refused[0]["subject"]["refusal_reason"], "not_on_record");
}

#[test]
fn two_racing_reversals_record_one_reverse_and_one_refusal() {
    let ws = Ws::new();
    for _ in 0..3 {
        let j = ws.record(Some(W), JUDGMENT);
        let spawn = |session: &str| {
            let mut c = std::process::Command::new(assert_cmd::cargo::cargo_bin("koto"));
            ws.env(&mut c, Some(session));
            c.args(["decisions", "reverse", &j])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        };
        let a = spawn(COORD);
        let b = spawn(W);
        let codes: Vec<i32> = [a, b]
            .into_iter()
            .map(|c| c.wait_with_output().unwrap().status.code().unwrap())
            .collect();
        let mut sorted = codes.clone();
        sorted.sort();
        assert_eq!(sorted, vec![0, 1], "exit codes {:?}", codes);
        let reversals: Vec<Value> = ws
            .list(&["--kind", "reverse"])
            .into_iter()
            .filter(|i| i["decision"]["reverses"] == json!(j))
            .collect();
        assert_eq!(reversals.len(), 1);
        let lost: Vec<Value> = ws
            .refused()
            .into_iter()
            .filter(|d| d["subject"]["attempted"]["decision"] == json!(j))
            .collect();
        assert_eq!(lost.len(), 1);
        assert_eq!(lost[0]["subject"]["refusal_reason"], "already_reversed");
    }
}

#[test]
fn a_worker_cant_reverse_its_coordinators_decision_when_both_chains_are_complete() {
    let ws = Ws::new();
    let d = ws.record(Some(COORD), JUDGMENT);
    let (code, out) = ws.reverse(Some(W), &d, &[]);
    assert_eq!(code, 1, "{}", out);
    assert_eq!(out["reason"], "no_standing");
    let refused = ws.refused();
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0]["id"], out["refused_id"]);
    assert_eq!(refused[0]["decided_by"]["session"][0]["value"], W);
}

#[test]
fn an_unmatched_reversal_of_a_decision_with_an_incomplete_chain_is_asserted() {
    let ws = Ws::new();
    let d = ws.record(Some(LONE), JUDGMENT);
    let (code, out) = ws.reverse(Some(W), &d, &[]);
    assert_eq!(code, 0, "{}", out);
    assert_eq!(out["standing"], "asserted");
}

#[test]
fn reversals_of_a_reversal_a_refusal_and_a_reversed_decision_are_refused() {
    let ws = Ws::new();
    let j = ws.record(Some(W), JUDGMENT);
    let (_, out) = ws.reverse(Some(W), &j, &[]);
    let reversal = out["id"].as_str().unwrap().to_string();
    let (code, out) = ws.reverse(Some(W), &j, &[]);
    assert_eq!(
        (code, out["reason"].clone()),
        (1, json!("already_reversed"))
    );
    let (code, out) = ws.reverse(Some(W), &reversal, &[]);
    assert_eq!(
        (code, out["reason"].clone()),
        (1, json!("reverse_of_reverse"))
    );
    let refused_id = out["refused_id"].as_str().unwrap().to_string();
    let (code, out) = ws.reverse(Some(W), &refused_id, &[]);
    assert_eq!(
        (code, out["reason"].clone()),
        (1, json!("reverse_of_refused"))
    );
    assert_eq!(ws.refused().len(), 3);
}

#[test]
fn a_new_acceptance_after_a_reversal_is_in_effect_beside_the_reversed_one() {
    let ws = Ws::new();
    let first = ws.record(Some(W), ACCEPT);
    ws.reverse(Some(COORD), &first, &[]);
    let second = ws.record(Some(W), ACCEPT);
    let items = ws.list(&["--kind", "accept"]);
    let status = |id: &str| {
        items
            .iter()
            .find(|i| i["decision"]["id"] == json!(id))
            .unwrap()["status"]
            .clone()
    };
    assert_eq!(status(&first), "reversed");
    assert_eq!(status(&second), "in_effect");
}

#[test]
fn the_original_line_is_byte_identical_after_its_reversal() {
    let ws = Ws::new();
    let j = ws.record(Some(W), JUDGMENT);
    let month = std::fs::read_dir(ws.store())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.file_name().unwrap().len() == "2026-10.jsonl".len())
        .unwrap();
    let line = |p: &Path| {
        std::fs::read_to_string(p)
            .unwrap()
            .lines()
            .find(|l| l.contains(&j))
            .unwrap()
            .to_string()
    };
    let before = line(&month);
    let (code, _) = ws.reverse(Some(W), &j, &[]);
    assert_eq!(code, 0);
    assert_eq!(line(&month), before);
}

// ----- Escalation and judgment effects -----

#[test]
fn a_reversed_judgment_lists_its_decider_and_carries_its_replacement() {
    let ws = Ws::new();
    let j = ws.record(Some(W), JUDGMENT);
    let (code, out) = ws.reverse(
        Some(COORD),
        &j,
        &["--replacement", "the test is really broken"],
    );
    assert_eq!(code, 0, "{}", out);
    let reversals = ws.list(&["--kind", "reverse"]);
    assert_eq!(reversals.len(), 1);
    let r = &reversals[0]["decision"];
    assert_eq!(r["decided_by"]["session"][0]["value"], COORD);
    assert_eq!(r["replacement"], "the test is really broken");

    let v = ws.record(
        Some(W),
        &[
            "--kind",
            "verdict",
            "--outcome",
            "pass",
            "--what",
            "review",
            "--evidence",
            "journal:wf#1",
        ],
    );
    let (code, out) = ws.reverse(Some(COORD), &v, &["--replacement", "maybe"]);
    assert_eq!(code, 2, "{}", out);
    assert_eq!(out["field"], "replacement");
}

#[test]
fn reversing_an_escalation_withdraws_it_and_reversing_a_decline_escalates_it() {
    let ws = Ws::new();
    let recipient = format!("session:{}", COORD);
    let esc = ws.record(
        Some(W),
        &[
            "--kind",
            "escalate",
            "--question-id",
            "q1",
            "--question",
            "ship it?",
            "--recipient",
            recipient.as_str(),
        ],
    );
    let dec = ws.record(
        Some(W),
        &[
            "--kind",
            "decline-escalation",
            "--question-id",
            "q2",
            "--question",
            "ask a person?",
        ],
    );
    ws.reverse(Some(W), &esc, &[]);
    ws.reverse(Some(COORD), &dec, &[]);
    let e = ws.show(&esc);
    assert_eq!(e["effect"]["meaning"], "question_withdrawn");
    assert_eq!(e["effect"]["withdrawn_from"], json!(recipient));
    let d = ws.show(&dec);
    assert_eq!(d["effect"]["meaning"], "question_escalated");
    assert_eq!(d["effect"]["escalated_to"], json!(recipient));
}

#[test]
fn a_reversed_rejection_leaves_the_subject_neither_accepted_nor_rejected() {
    let ws = Ws::new();
    let args: Vec<&str> = ACCEPT
        .iter()
        .map(|a| if *a == "accepted" { "rejected" } else { *a })
        .collect();
    let rej = ws.record(Some(W), &args);
    ws.reverse(Some(W), &rej, &[]);
    assert_eq!(
        ws.show(&rej)["effect"]["meaning"],
        "neither_accepted_nor_rejected"
    );
}

// ----- Received decisions -----

#[test]
fn received_lists_escalations_to_a_session_and_reversals_of_its_decisions() {
    let ws = Ws::new();
    let recipient = format!("session:{}", COORD);
    let own = ws.record(Some(COORD), JUDGMENT);
    let esc = ws.record(
        Some(W),
        &[
            "--kind",
            "escalate",
            "--question-id",
            "q1",
            "--question",
            "ship it?",
            "--recipient",
            recipient.as_str(),
        ],
    );
    let (_, out) = ws.reverse(Some(COORD), &own, &[]);
    let rev_own = out["id"].as_str().unwrap().to_string();
    // A reversal of someone else's decision is not COORD's to receive.
    let other = ws.record(Some(LONE), JUDGMENT);
    ws.reverse(Some(W), &other, &[]);

    let ids = |items: Vec<Value>| -> Vec<String> {
        items
            .into_iter()
            .map(|i| i["decision"]["id"].as_str().unwrap().to_string())
            .collect()
    };
    let mut received = ids(ws.list(&["--session", COORD, "--received"]));
    received.sort();
    let mut want = vec![esc.clone(), rev_own.clone()];
    want.sort();
    assert_eq!(received, want);

    let mut made = ids(ws.list(&["--session", COORD]));
    made.sort();
    let mut want = vec![own, rev_own];
    want.sort();
    assert_eq!(made, want);
}

// ----- Engine enforcement -----

const TEMPLATE: &str = r#"---
name: gated
version: "1.0"
initial_state: gated
states:
  gated:
    gates:
      g:
        type: command
        command: "test -f ok.flag"
    transitions:
      - target: after
  after:
    accepts:
      go:
        type: enum
        required: true
        values: [yes, no]
    transitions:
      - target: done
        when:
          go: yes
      - target: gated
        when:
          go: no
  done:
    terminal: true
---

## gated

Pass the gate.

## after

Carry on.

## done

Done.
"#;

impl Ws {
    /// Start `name` and tick it past the gate into `after`.
    fn workflow_past_gate(&self, name: &str) {
        let tpl = self.cwd().join("gated.md");
        std::fs::write(&tpl, TEMPLATE).unwrap();
        std::fs::write(self.cwd().join("ok.flag"), "").unwrap();
        self.ok(
            Some(W),
            &["init", name, "--template", tpl.to_str().unwrap()],
        );
        let out = self.ok(Some(W), &["next", name]);
        assert_eq!(out["state"], "after", "{}", out);
    }

    fn gate_pass(&self, session: Option<&str>, name: &str, extra: &[&str]) -> String {
        let args = with(
            &[
                "--kind",
                "gate-pass",
                "--workflow",
                name,
                "--state",
                "gated",
                "--gate",
                "g",
            ],
            extra,
        );
        self.record(session, &args)
    }

    /// Wait into the next second: decisions record their time to the
    /// second, and a pass in the reversal's own second doesn't clear it.
    fn tick_second(&self) {
        std::thread::sleep(std::time::Duration::from_millis(1100));
    }

    fn next(&self, name: &str) -> Value {
        self.ok(Some(W), &["next", name])
    }
}

fn reversal_conditions(out: &Value) -> Vec<Value> {
    out["blocking_conditions"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|c| c["category"] == "decision_reversed")
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_reversed_gate_pass_blocks_until_the_workflow_is_rewound() {
    let ws = Ws::new();
    ws.workflow_past_gate("wf");
    let pass = ws.gate_pass(Some(W), "wf", &[]);
    let (code, out) = ws.reverse(Some(COORD), &pass, &["--reason", "secret-reason-text"]);
    assert_eq!(code, 0, "{}", out);
    let rev = out["id"].clone();

    let blocked = ws.next("wf");
    assert_eq!(blocked["action"], "gate_blocked", "{}", blocked);
    assert_eq!(blocked["state"], "after");
    let conds = reversal_conditions(&blocked);
    assert_eq!(conds.len(), 1);
    let output = conds[0]["output"].as_object().unwrap();
    let mut keys: Vec<&str> = output.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, vec!["kind", "reversal", "standing", "time"]);
    assert_eq!(output["reversal"], rev);
    assert_eq!(output["kind"], "gate-pass");
    assert_eq!(output["standing"], "verified");
    assert!(!blocked.to_string().contains("secret-reason-text"));

    // Evidence is refused too, and the state doesn't move.
    let out = ws.ok(Some(W), &["next", "wf", "--with-data", r#"{"go":"yes"}"#]);
    assert_eq!(out["action"], "gate_blocked");
    assert_eq!(out["state"], "after");

    ws.ok(Some(W), &["rewind", "wf"]);
    let out = ws.next("wf");
    assert!(reversal_conditions(&out).is_empty(), "{}", out);
    assert_ne!(out["action"], "gate_blocked", "{}", out);
}

#[test]
fn a_new_gate_pass_by_the_reverser_clears_the_block() {
    let ws = Ws::new();
    ws.workflow_past_gate("wf");
    let pass = ws.gate_pass(Some(W), "wf", &[]);
    ws.reverse(Some(COORD), &pass, &[]);
    assert_eq!(ws.next("wf")["action"], "gate_blocked");
    ws.tick_second();
    ws.gate_pass(Some(COORD), "wf", &[]);
    let out = ws.next("wf");
    assert!(reversal_conditions(&out).is_empty(), "{}", out);
}

#[test]
fn a_gate_pass_without_standing_over_the_reverser_does_not_clear_the_block() {
    let ws = Ws::new();
    ws.workflow_past_gate("wf");
    let pass = ws.gate_pass(Some(W), "wf", &[]);
    ws.reverse(Some(COORD), &pass, &[]);
    ws.tick_second();
    // W2 is not in COORD's (complete) chain.
    ws.gate_pass(Some(W2), "wf", &[]);
    assert_eq!(reversal_conditions(&ws.next("wf")).len(), 1);
    // Naming COORD by hand from another session is only an asserted match,
    // which doesn't clear a verified reversal.
    let by = format!("session:{}", COORD);
    ws.gate_pass(Some(W2), "wf", &["--by", by.as_str()]);
    assert_eq!(reversal_conditions(&ws.next("wf")).len(), 1);
}

#[test]
fn a_reversal_blocks_only_on_the_pass_it_names() {
    let ws = Ws::new();
    ws.workflow_past_gate("wf");
    let first = ws.gate_pass(Some(W), "wf", &[]);
    let _second = ws.gate_pass(Some(W), "wf", &[]);
    let (_, out) = ws.reverse(Some(W), &first, &[]);
    let conds = reversal_conditions(&ws.next("wf"));
    assert_eq!(conds.len(), 1);
    assert_eq!(conds[0]["output"]["reversal"], out["id"]);

    // Another run of a workflow with the same name isn't the one the pass
    // named.
    ws.ok(Some(W), &["session", "cleanup", "wf"]);
    ws.workflow_past_gate("wf");
    assert!(reversal_conditions(&ws.next("wf")).is_empty());
}

#[test]
fn a_release_verdict_reversal_records_its_rollback_or_its_absence() {
    let ws = Ws::new();
    let verdict = &[
        "--kind",
        "verdict",
        "--outcome",
        "pass",
        "--of",
        "release",
        "--what",
        "production verdict",
        "--repo",
        "o/r",
        "--commit",
        "abc",
        "--evidence",
        "journal:rel#3",
    ];
    let a = ws.record(Some(W), verdict);
    let link = "https://example.com/o/r/actions/runs/9";
    let (_, out) = ws.reverse(Some(W), &a, &["--compensating", link]);
    assert_eq!(
        out["decision"]["compensating"],
        json!({"exists": true, "link": link})
    );
    let b = ws.record(Some(W), verdict);
    let (_, out) = ws.reverse(Some(W), &b, &[]);
    assert_eq!(out["decision"]["compensating"], json!({"exists": false}));
    // A judgment takes no compensating link.
    let j = ws.record(Some(W), JUDGMENT);
    let (code, out) = ws.reverse(Some(W), &j, &["--compensating", link]);
    assert_eq!((code, out["field"].clone()), (2, json!("compensating")));
}

#[test]
fn a_workflow_with_no_reversed_pass_reads_no_month_file() {
    use std::os::unix::fs::PermissionsExt;
    let ws = Ws::new();
    ws.workflow_past_gate("wf");
    // A reversal of something else is in the log.
    let j = ws.record(Some(W), JUDGMENT);
    ws.reverse(Some(W), &j, &[]);
    let months: Vec<PathBuf> = std::fs::read_dir(ws.store())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.file_name().unwrap().len() == "2026-10.jsonl".len())
        .collect();
    for m in &months {
        std::fs::set_permissions(m, std::fs::Permissions::from_mode(0o000)).unwrap();
    }
    let out = ws.next("wf");
    for m in &months {
        std::fs::set_permissions(m, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert_eq!(out["action"], "evidence_required", "{}", out);
}

#[test]
fn an_unreadable_reversal_line_blocks_as_unreadable() {
    let ws = Ws::new();
    ws.workflow_past_gate("wf");
    let j = ws.record(Some(W), JUDGMENT);
    ws.reverse(Some(W), &j, &[]);
    let log = ws.store().join("reversals.jsonl");
    let mut content = std::fs::read_to_string(&log).unwrap();
    content.push_str("{not json\n");
    std::fs::write(&log, content).unwrap();
    let out = ws.next("wf");
    assert_eq!(out["action"], "gate_blocked", "{}", out);
    assert_eq!(
        out["blocking_conditions"][0]["category"],
        "decision_store_unreadable"
    );
}
