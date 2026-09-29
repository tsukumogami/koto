//! The two demonstration decider checks, driven end to end against the
//! `std::net` stub in a real git repository.
//!
//! `tests/fixtures/decider_checks/comment-reason.md` and
//! `acceptance-criteria.md` ship their criteria in shadow; these tests also
//! run veto variants of them (the same file with `mode: veto`) through every
//! outcome of the PRD's outcome table.

#[path = "support/decider_stub.rs"]
mod decider_stub;

#[path = "support/decider_session.rs"]
mod decider_session;

use std::path::Path;
use std::process::Command;

use decider_session::*;
use decider_stub::Reply;
use serde_json::{json, Value};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/decider_checks")
            .join(name),
    )
    .unwrap()
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {:?}: {}", args, describe(&out));
}

/// A harness whose directory is a git repository with one commit holding
/// `path` at `before`, then `path` changed to `after` in the working tree.
fn repo_harness(template: &str, path: &str, before: &str, after: &str) -> Harness {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().to_path_buf();
    // Keep the directory for the harness's lifetime.
    std::mem::forget(tmp);
    git(&dir, &["init", "-q"]);
    git(&dir, &["config", "user.email", "t@example.org"]);
    git(&dir, &["config", "user.name", "t"]);
    std::fs::write(dir.join(path), before).unwrap();
    git(&dir, &["add", path]);
    git(&dir, &["commit", "-q", "-m", "base"]);
    std::fs::write(dir.join(path), after).unwrap();
    let h = Harness::in_dir(&dir, template);
    h.init();
    h
}

fn run(h: &Harness, mode: &str) -> Value {
    let mut cmd = h.koto_mode(mode);
    cmd.args(["next", WF, "--no-cleanup"]);
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    json_out(&out)
}

fn checks(h: &Harness) -> Vec<Value> {
    h.events_of("decider_checked")
        .into_iter()
        .map(|e| e["payload"].clone())
        .collect()
}

fn choice(rule_id: &str, pass: f64, fail: f64, unclear: f64) -> Reply {
    Reply::json(&json!({
        "model": "jev-test-1.2.3",
        "answers": {rule_id: {"type": "choice", "choice": "fail",
            "probabilities": {"pass": pass, "fail": fail, "unclear": unclear}}},
        "usage": {"input_tokens": 100, "output_tokens": 3}
    }))
}

const CODE_BEFORE: &str = "fn total(a: u32, b: u32) -> u32 {\n    a + b\n}\n";
const CODE_AFTER: &str = "fn total(a: u32, b: u32) -> u32 {\n    // add a and b\n    a + b\n}\n";

const DOC_BEFORE: &str = "# PRD\n\n## Acceptance Criteria\n\n";
const DOC_AFTER: &str =
    "# PRD\n\n## Acceptance Criteria\n\n- [ ] The report is fast enough.\n- [ ] `koto next` exits 0.\n";

#[test]
fn both_fixtures_compile_with_their_criteria_in_shadow() {
    for name in ["comment-reason.md", "acceptance-criteria.md"] {
        let mut f = tempfile::Builder::new().suffix(".md").tempfile().unwrap();
        std::io::Write::write_all(&mut f, fixture(name).as_bytes()).unwrap();
        let t = koto::template::compile::compile(f.path(), true).unwrap();
        let spec = t.states["review"]
            .gates
            .values()
            .next()
            .unwrap()
            .decider_check
            .clone()
            .unwrap();
        assert!(spec
            .criteria
            .iter()
            .all(|c| c.mode == koto::template::decider_check::CheckMode::Shadow));
    }
}

/// Every row of the outcome table for one veto fixture.
fn outcome_rows(name: &str, rule_id: &str, path: &str, before: &str, after: &str, needle: &str) {
    let veto = fixture(name).replace("mode: shadow", "mode: veto");
    let rows: Vec<(Reply, &str, bool)> = vec![
        (choice(rule_id, 0.02, 0.95, 0.03), "fail", true),
        (choice(rule_id, 0.95, 0.02, 0.03), "pass", false),
        (choice(rule_id, 0.1, 0.1, 0.8), "escape", false),
        (Reply::status(401), "unanswered", true),
    ];
    for (reply, outcome, blocks) in rows {
        let h = repo_harness(&veto, path, before, after);
        h.stub.push(reply);
        let out = run(&h, "auto");
        assert_eq!(
            out["action"] == "gate_blocked",
            blocks,
            "{} {}: {}",
            name,
            outcome,
            out
        );
        let c = &checks(&h)[0];
        assert_eq!(c["outcome"], outcome, "{}", name);
        assert_eq!(c["rule_id"], rule_id);
        // The slice the decider read is the change the fixture extracts.
        let sent = h.stub.last_request().unwrap().json();
        let slice = sent["state"]
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .clone();
        assert!(
            slice.as_str().unwrap().contains(needle),
            "{}: {}",
            name,
            slice
        );
    }

    // No change: nothing to grade, nothing sent, nothing blocked.
    let h = repo_harness(&veto, path, before, before);
    let out = run(&h, "auto");
    assert_eq!(out["state"], "done", "{}", out);
    assert_eq!(h.stub.request_count(), 0);
    assert_eq!(checks(&h)[0]["outcome"], "not_graded");
}

#[test]
fn the_comment_criterion_runs_every_outcome_in_veto() {
    outcome_rows(
        "comment-reason.md",
        "comment_reason",
        "lib.rs",
        CODE_BEFORE,
        CODE_AFTER,
        "// add a and b",
    );
}

#[test]
fn the_acceptance_criterion_criterion_runs_every_outcome_in_veto() {
    outcome_rows(
        "acceptance-criteria.md",
        "ac_binary",
        "PRD.md",
        DOC_BEFORE,
        DOC_AFTER,
        "- [ ] The report is fast enough.",
    );
}

#[test]
fn the_shipped_shadow_fixtures_never_block() {
    let h = repo_harness(
        &fixture("comment-reason.md"),
        "lib.rs",
        CODE_BEFORE,
        CODE_AFTER,
    );
    h.stub.push(choice("comment_reason", 0.02, 0.95, 0.03));
    let out = run(&h, "auto");
    assert_eq!(out["state"], "done", "{}", out);
    assert_eq!(checks(&h)[0]["outcome"], "fail");
    assert_eq!(checks(&h)[0]["mode"], "shadow");
}
