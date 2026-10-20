//! Who decided, under what authority and through which chain, through the
//! CLI: the decider resolver, the agent and run mode, people as deciders,
//! and lineage read from the workspace manager's session map, the run
//! journal's driver stamp and handed flags.
//!
//! Every test runs koto from inside a fixture workspace (a directory holding
//! `.niwa/workspace.toml` and `.niwa/sessions/`), with `HOME` pointed at its
//! own temporary directory so the store and the journal are private to it.
//! Harness variables are set per run; nothing is inherited from the host.

use assert_cmd::Command;
use assert_fs::TempDir;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const S1: &str = "9c1e0000-0000-4000-8000-000000000001";
const S2: &str = "9c1e0000-0000-4000-8000-000000000002";
const COORD: &str = "4b2d0000-0000-4000-8000-00000000000c";

struct Ws {
    dir: TempDir,
}

impl Ws {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("ws/.niwa/sessions")).unwrap();
        std::fs::create_dir_all(dir.path().join("ws/repo")).unwrap();
        std::fs::write(dir.path().join("ws/.niwa/workspace.toml"), "").unwrap();
        Ws { dir }
    }

    fn home(&self) -> &Path {
        self.dir.path()
    }

    /// The working directory: a repo inside the workspace.
    fn cwd(&self) -> PathBuf {
        self.dir.path().join("ws/repo")
    }

    fn niwa(&self) -> PathBuf {
        self.dir.path().join("ws/.niwa")
    }

    fn entry_path(&self, id: &str) -> PathBuf {
        self.niwa().join("sessions").join(format!("{}.json", id))
    }

    fn entry(&self, id: &str, body: Value) {
        std::fs::write(self.entry_path(id), body.to_string()).unwrap();
    }

    /// koto in the workspace, with the harness session `session` (or none)
    /// and every other host variable cleared.
    fn koto(&self, session: Option<&str>) -> Command {
        let mut cmd = Command::cargo_bin("koto").unwrap();
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
        cmd
    }

    fn run_with(&self, mut cmd: Command, args: &[&str]) -> (i32, Value) {
        let out = cmd.args(args).output().unwrap();
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

    /// Record and return the stored decision.
    fn record_with(&self, cmd: Command, args: &[&str]) -> Value {
        let mut full = vec!["decisions", "record"];
        full.extend_from_slice(args);
        let (code, json) = self.run_with(cmd, &full);
        assert_eq!(code, 0, "koto {:?} failed: {}", full, json);
        json["decision"].clone()
    }

    fn record(&self, session: Option<&str>, args: &[&str]) -> Value {
        self.record_with(self.koto(session), args)
    }

    fn list(&self, args: &[&str]) -> Vec<Value> {
        let mut full = vec!["decisions", "list"];
        full.extend_from_slice(args);
        let (code, json) = self.run_with(self.koto(Some(S2)), &full);
        assert_eq!(code, 0, "koto {:?} failed: {}", full, json);
        json["items"]
            .as_array()
            .unwrap_or_else(|| panic!("no items array: {}", json))
            .iter()
            .map(|d| d["decision"].clone())
            .collect()
    }

    fn month_lines(&self) -> usize {
        let root = self.home().join(".koto/decisions");
        let Ok(entries) = std::fs::read_dir(root) else {
            return 0;
        };
        entries
            .map(|e| e.unwrap().path())
            .filter(|p| {
                let n = p.file_name().unwrap().to_string_lossy().to_string();
                n.ends_with(".jsonl") && n.len() == "2026-10.jsonl".len()
            })
            .map(|p| std::fs::read_to_string(p).unwrap().lines().count())
            .sum()
    }
}

const JUDGMENT: &[&str] = &["--kind", "judgment", "--what", "x"];

fn with(base: &[&'static str], extra: &[&'static str]) -> Vec<&'static str> {
    base.iter().chain(extra).copied().collect()
}

fn slot(v: &Value) -> Vec<(String, String)> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|e| {
                    (
                        e["value"].as_str().unwrap().to_string(),
                        e["source"].as_str().unwrap().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn pair(value: &str, source: &str) -> (String, String) {
    (value.to_string(), source.to_string())
}

fn links(d: &Value) -> Vec<Vec<(String, String)>> {
    d["chain"]["links"]
        .as_array()
        .unwrap()
        .iter()
        .map(slot)
        .collect()
}

// ----- Decider and agent -----

#[test]
fn an_agent_nobody_establishes_is_unknown_never_main() {
    let ws = Ws::new();
    let d = ws.record(Some(S1), JUDGMENT);
    assert_eq!(
        slot(&d["decided_by"]["agent"]),
        vec![pair("unknown", "observed")]
    );
}

#[test]
fn the_agent_is_observed_from_the_hook_variable_handed_by_flag_and_both_kept() {
    let ws = Ws::new();
    let mut cmd = ws.koto(Some(S1));
    cmd.env("KOTO_AGENT_ID", "main");
    let d = ws.record_with(cmd, JUDGMENT);
    assert_eq!(
        slot(&d["decided_by"]["agent"]),
        vec![pair("main", "observed")]
    );

    let d = ws.record(Some(S1), &with(JUDGMENT, &["--agent", "agent-7"]));
    assert_eq!(
        slot(&d["decided_by"]["agent"]),
        vec![pair("agent-7", "handed")]
    );

    let mut cmd = ws.koto(Some(S1));
    cmd.env("KOTO_AGENT_ID", "main");
    let d = ws.record_with(cmd, &with(JUDGMENT, &["--agent", "agent-7"]));
    assert_eq!(
        slot(&d["decided_by"]["agent"]),
        vec![pair("main", "observed"), pair("agent-7", "handed")]
    );

    let mut cmd = ws.koto(Some(S1));
    cmd.env("KOTO_AGENT_ID", "x; rm -rf /");
    let d = ws.record_with(cmd, JUDGMENT);
    assert_eq!(
        slot(&d["decided_by"]["agent"]),
        vec![pair("unknown", "observed")]
    );
}

#[test]
fn the_run_mode_is_observed_handed_or_unknown_and_filters() {
    let ws = Ws::new();
    let mut cmd = ws.koto(Some(S1));
    cmd.env("CLAUDE_CODE_SESSION_ATTENDED", "0");
    let unattended = ws.record_with(cmd, JUDGMENT);
    assert_eq!(unattended["decided_by"]["kind"], "session");
    assert_eq!(
        slot(&unattended["authority"]["run_mode"]),
        vec![pair("unattended", "observed")]
    );

    let none = ws.record(Some(S1), JUDGMENT);
    assert_eq!(
        slot(&none["authority"]["run_mode"]),
        vec![pair("unknown", "observed")]
    );

    let handed = ws.record(Some(S1), &with(JUDGMENT, &["--run-mode", "attended"]));
    assert_eq!(
        slot(&handed["authority"]["run_mode"]),
        vec![pair("attended", "handed")]
    );

    let listed = ws.list(&["--run-mode", "unattended"]);
    let ids: Vec<&Value> = listed.iter().map(|d| &d["id"]).collect();
    assert_eq!(ids, vec![&unattended["id"]]);
}

#[test]
fn no_by_defaults_to_the_observed_session_and_by_is_handed() {
    let ws = Ws::new();
    let d = ws.record(Some(S1), JUDGMENT);
    assert_eq!(d["decided_by"]["defaulted"], true);
    assert_eq!(
        slot(&d["decided_by"]["session"]),
        vec![pair(S1, "observed")]
    );

    let by = format!("session:{}", S2);
    let mut cmd = ws.koto(Some(S1));
    cmd.args([
        "decisions",
        "record",
        "--kind",
        "judgment",
        "--what",
        "x",
        "--by",
        &by,
    ]);
    let (code, json) = ws.run_with(cmd, &[]);
    assert_eq!(code, 0, "{}", json);
    let d = &json["decision"];
    assert_eq!(d["decided_by"]["defaulted"], false);
    assert_eq!(
        slot(&d["decided_by"]["session"]),
        vec![pair(S1, "observed"), pair(S2, "handed")]
    );
}

#[test]
fn neither_by_nor_a_harness_session_is_refused_as_decider_missing() {
    let ws = Ws::new();
    let mut full = vec!["decisions", "record"];
    full.extend_from_slice(JUDGMENT);
    let (code, json) = ws.run_with(ws.koto(None), &full);
    assert_eq!(code, 2, "{}", json);
    assert_eq!(json["field"], "by");
    assert_eq!(json["reason"], "decider_missing");
    assert_eq!(ws.month_lines(), 0, "a refused decision wrote a line");
}

#[test]
fn listing_by_dispatch_id_and_instructions_digest_is_exact() {
    let ws = Ws::new();
    let digest_a = format!("sha256:{}", "a1".repeat(32));
    let digest_b = format!("sha256:{}", "b2".repeat(32));
    let mut ids = Vec::new();
    for (disp, digest) in [
        ("disp-a", &digest_a),
        ("disp-a", &digest_b),
        ("disp-b", &digest_a),
    ] {
        let mut cmd = ws.koto(Some(S1));
        cmd.args([
            "decisions",
            "record",
            "--kind",
            "judgment",
            "--what",
            "x",
            "--dispatch-id",
            disp,
            "--instructions-digest",
            digest,
        ]);
        let (code, json) = ws.run_with(cmd, &[]);
        assert_eq!(code, 0, "{}", json);
        ids.push(json["id"].clone());
    }
    ws.record(Some(S1), JUDGMENT);
    // Decisions in one second list in identifier order, so compare sets.
    let ids_of = |listed: Vec<Value>| -> BTreeSet<String> {
        listed
            .iter()
            .map(|d| d["id"].as_str().unwrap().to_string())
            .collect()
    };
    let want = |i: &[usize]| -> BTreeSet<String> {
        i.iter()
            .map(|&n| ids[n].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(ids_of(ws.list(&["--dispatch-id", "disp-a"])), want(&[0, 1]));
    assert_eq!(
        ids_of(ws.list(&["--instructions-digest", &digest_a])),
        want(&[0, 2])
    );
}

#[test]
fn a_worker_escalates_declines_and_judges_and_all_three_list() {
    let ws = Ws::new();
    let recipient = format!("session:{}", COORD);
    let e = ws.record(
        Some(S1),
        &[
            "--kind",
            "escalate",
            "--question-id",
            "q1",
            "--question",
            "which schema?",
            "--recipient",
            &recipient,
        ],
    );
    let d = ws.record(
        Some(S1),
        &[
            "--kind",
            "decline-escalation",
            "--question-id",
            "q2",
            "--question",
            "rename the flag?",
        ],
    );
    let j = ws.record(Some(S1), JUDGMENT);
    let listed = ws.list(&["--session", S1]);
    assert_eq!(listed.len(), 3);
    let mut want: Vec<&Value> = vec![&e["id"], &d["id"], &j["id"]];
    let mut got_ids: Vec<&Value> = listed.iter().map(|x| &x["id"]).collect();
    want.sort_by_key(|v| v.as_str().unwrap().to_string());
    got_ids.sort_by_key(|v| v.as_str().unwrap().to_string());
    assert_eq!(want, got_ids);
    for got in &listed {
        assert_eq!(
            slot(&got["decided_by"]["session"]),
            vec![pair(S1, "observed")]
        );
    }
    let esc = listed.iter().find(|d| d["kind"] == "escalate").unwrap();
    assert_eq!(esc["subject"]["recipient"], recipient);
}

#[test]
fn a_judgment_about_another_sessions_workflow_lists_for_a_third_session() {
    let ws = Ws::new();
    let d = ws.record(
        Some(S1),
        &[
            "--kind",
            "judgment",
            "--what",
            "the review panel's finding 2 is a false positive",
            "--pointer",
            "workflow:work-on-77",
        ],
    );
    // `list` runs as S2, a third party to both.
    let listed = ws.list(&["--subject", "pointer=workflow:work-on-77"]);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], d["id"]);
}

// ----- People as deciders -----

#[test]
fn a_maintainer_accepting_at_the_terminal_is_a_person_decider() {
    let ws = Ws::new();
    let d = ws.record(
        Some(S1),
        &[
            "--kind",
            "accept",
            "--outcome",
            "accepted",
            "--of",
            "document",
            "--repo",
            "o/r",
            "--path",
            "docs/prds/PRD-x.md",
            "--commit",
            "abc123",
            "--by",
            "person:dana",
            "--surface",
            "terminal",
        ],
    );
    let by = &d["decided_by"];
    assert_eq!(by["kind"], "person");
    assert_eq!(slot(&by["handle"]), vec![pair("dana", "handed")]);
    assert_eq!(by["surface"], "terminal");
    assert_eq!(slot(&by["entered_by"]), vec![pair(S1, "observed")]);
    assert_eq!(d["authority"]["kind"], "person");
}

#[test]
fn a_relay_with_a_link_is_the_person_and_without_one_is_the_relaying_session() {
    let ws = Ws::new();
    // A host that doesn't resolve: the record succeeds because the link is
    // stored, never fetched.
    let link = "https://forge.invalid/o/r/pull/9#pullrequestreview-1";
    let d = ws.record(
        Some(COORD),
        &with(
            JUDGMENT,
            &["--by", "person:dana", "--surface", "relay", "--link", link],
        ),
    );
    assert_eq!(d["decided_by"]["kind"], "person");
    assert_eq!(d["decided_by"]["link"], link);
    assert_eq!(
        slot(&d["decided_by"]["handle"]),
        vec![pair("dana", "handed")]
    );

    let d = ws.record(
        Some(COORD),
        &with(JUDGMENT, &["--by", "person:dana", "--surface", "relay"]),
    );
    let by = &d["decided_by"];
    assert_eq!(by["kind"], "session");
    assert_eq!(slot(&by["session"]), vec![pair(COORD, "observed")]);
    assert_eq!(slot(&by["relayed_person"]), vec![pair("dana", "handed")]);
    let shown = ws.run_with(
        ws.koto(Some(S2)),
        &["decisions", "show", d["id"].as_str().unwrap()],
    );
    assert_eq!(
        slot(&shown.1["decision"]["decided_by"]["relayed_person"]),
        vec![pair("dana", "handed")]
    );
}

#[test]
fn a_link_belongs_to_a_person_decider() {
    let ws = Ws::new();
    let mut full = vec!["decisions", "record"];
    full.extend(with(JUDGMENT, &["--link", "https://example.com/r/1"]));
    let (code, json) = ws.run_with(ws.koto(Some(S1)), &full);
    assert_eq!(code, 2, "{}", json);
    assert_eq!(json["field"], "link");
}

#[test]
fn the_stored_format_has_no_account_fields_and_needs_no_registration() {
    let ws = Ws::new();
    let d = ws.record(
        Some(S1),
        &with(
            JUDGMENT,
            &["--by", "person:someone-new", "--surface", "forge"],
        ),
    );
    fn keys(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => {
                for (k, val) in m {
                    out.push(k.to_ascii_lowercase());
                    keys(val, out);
                }
            }
            Value::Array(a) => a.iter().for_each(|x| keys(x, out)),
            _ => {}
        }
    }
    let mut all = Vec::new();
    keys(&d, &mut all);
    for banned in [
        "account",
        "login",
        "credential",
        "password",
        "token",
        "email",
        "user",
    ] {
        assert!(
            !all.iter().any(|k| k.contains(banned)),
            "field naming {} in {:?}",
            banned,
            all
        );
    }
}

// ----- Labels and chains -----

#[test]
fn direct_and_detached_dispatch_entries_are_handed_and_incomplete() {
    let ws = Ws::new();
    // Today's shape, as the workspace manager writes it for a dispatch.
    for (id, disp) in [(S1, "disp-direct"), (S2, "disp-detached")] {
        ws.entry(
            id,
            json!({"session_id": id, "instance_name": "i", "instance_path": "/x",
                   "transcript_path": "", "created": "2026-10-19T00:00:00Z",
                   "ephemeral": true, "origin": "dispatch",
                   "dispatch_id": disp, "parent_session_id": COORD}),
        );
    }
    for (id, disp) in [(S1, "disp-direct"), (S2, "disp-detached")] {
        let d = ws.record(Some(id), JUDGMENT);
        assert_eq!(
            slot(&d["authority"]["dispatch_id"]),
            vec![pair(disp, "handed")]
        );
        let parent = format!("session:{}", COORD);
        assert_eq!(
            slot(&d["authority"]["dispatcher"]),
            vec![pair(&parent, "handed")]
        );
        assert_eq!(links(&d), vec![vec![pair(&parent, "handed")]]);
        assert_eq!(d["chain"]["incomplete"], true);
    }
}

#[test]
fn a_session_with_no_entry_records_an_incomplete_chain() {
    let ws = Ws::new();
    let d = ws.record(Some(S1), JUDGMENT);
    assert!(links(&d).is_empty());
    assert_eq!(d["chain"]["incomplete"], true);
    assert_eq!(d["authority"]["kind"], "dispatch");
    assert_eq!(d["authority"]["dispatch_id"], json!([]));
}

#[test]
fn a_versioned_entry_with_a_copied_chain_reaches_its_root() {
    let ws = Ws::new();
    let digest = format!("sha256:{}", "cd".repeat(32));
    ws.entry(
        S1,
        json!({"session_id": S1, "lineage": {
            "v": 1, "dispatch_id": "disp-1", "brief_digest": digest,
            "parent": format!("session:{}", COORD),
            "chain": [{"session": COORD, "dispatch_id": "disp-0", "parent": "none"}],
            "origin": "dispatch",
            "source": {"dispatch_id": "observed", "brief_digest": "observed",
                       "parent": "handed", "chain": "observed"}}}),
    );
    let d = ws.record(Some(S1), JUDGMENT);
    assert_eq!(
        links(&d),
        vec![
            vec![pair(&format!("session:{}", COORD), "handed")],
            vec![pair("none", "observed")]
        ]
    );
    assert_eq!(d["chain"]["incomplete"], false);
    assert_eq!(
        slot(&d["authority"]["dispatch_id"]),
        vec![pair("disp-1", "observed")]
    );
    assert_eq!(
        slot(&d["authority"]["instructions_digest"]),
        vec![pair(&digest, "observed")]
    );
}

#[test]
fn an_unknown_lineage_version_is_treated_as_absent() {
    let ws = Ws::new();
    ws.entry(
        S1,
        json!({"session_id": S1, "lineage": {"v": 2, "parent": "none",
               "source": {"parent": "observed"}}}),
    );
    let d = ws.record(Some(S1), JUDGMENT);
    assert!(links(&d).is_empty());
    assert_eq!(d["chain"]["incomplete"], true);
}

#[test]
fn a_handed_dispatched_by_none_closes_the_chain_at_a_handed_root() {
    let ws = Ws::new();
    let d = ws.record(Some(S1), &with(JUDGMENT, &["--dispatched-by", "none"]));
    assert_eq!(links(&d), vec![vec![pair("none", "handed")]]);
    assert_eq!(d["chain"]["incomplete"], false);
}

#[test]
fn a_handed_none_against_an_observed_parent_marks_the_chain_incomplete() {
    let ws = Ws::new();
    let parent = format!("session:{}", COORD);
    ws.entry(
        S1,
        json!({"lineage": {"v": 1, "parent": parent, "source": {"parent": "observed"}}}),
    );
    ws.entry(
        COORD,
        json!({"lineage": {"v": 1, "parent": "none", "source": {"parent": "observed"}}}),
    );
    let d = ws.record(Some(S1), &with(JUDGMENT, &["--dispatched-by", "none"]));
    assert_eq!(
        links(&d)[0],
        vec![pair(&parent, "observed"), pair("none", "handed")]
    );
    assert_eq!(d["chain"]["incomplete"], true);
}

#[test]
fn a_chain_file_supplies_the_links_above_the_dispatcher() {
    let ws = Ws::new();
    let file = ws.home().join("chain.json");
    std::fs::write(
        &file,
        json!({"v": 1, "chain": [{"session": COORD, "parent": "person:dana"}]}).to_string(),
    )
    .unwrap();
    let by = format!("session:{}", COORD);
    let mut cmd = ws.koto(Some(S1));
    cmd.args([
        "decisions",
        "record",
        "--kind",
        "judgment",
        "--what",
        "x",
        "--dispatched-by",
        &by,
        "--chain-file",
        file.to_str().unwrap(),
    ]);
    let (code, json) = ws.run_with(cmd, &[]);
    assert_eq!(code, 0, "{}", json);
    let d = &json["decision"];
    assert_eq!(
        links(d),
        vec![
            vec![pair(&by, "handed")],
            vec![pair("person:dana", "handed")]
        ]
    );
    assert_eq!(d["chain"]["incomplete"], false);

    // A symlinked chain file is refused before anything is written.
    let link = ws.home().join("link.json");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    let before = ws.month_lines();
    let mut cmd = ws.koto(Some(S1));
    cmd.args([
        "decisions",
        "record",
        "--kind",
        "judgment",
        "--what",
        "x",
        "--chain-file",
        link.to_str().unwrap(),
    ]);
    let (code, json) = ws.run_with(cmd, &[]);
    assert_eq!(code, 2, "{}", json);
    assert_eq!(json["field"], "chain_file");
    assert_eq!(ws.month_lines(), before);
}

/// Every file under `dir`, with its contents and modification time.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>, std::time::SystemTime)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let m = std::fs::metadata(&p).unwrap().modified().unwrap();
                out.push((p.clone(), std::fs::read(&p).unwrap(), m));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn koto_never_runs_the_workspace_manager_or_writes_under_its_directory() {
    let ws = Ws::new();
    ws.entry(
        S1,
        json!({"dispatch_id": "disp-1", "parent_session_id": COORD}),
    );
    // A `niwa` on PATH that leaves a mark if anything runs it.
    let bin = ws.home().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let mark = ws.home().join("niwa-ran");
    let script = bin.join("niwa");
    std::fs::write(&script, format!("#!/bin/sh\ntouch '{}'\n", mark.display())).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let before = snapshot(&ws.niwa());
    let mut cmd = ws.koto(Some(S1));
    cmd.env("PATH", &path);
    let d = ws.record_with(cmd, JUDGMENT);
    assert_eq!(
        slot(&d["authority"]["dispatch_id"]),
        vec![pair("disp-1", "handed")]
    );
    assert!(!mark.exists(), "koto ran the workspace manager");
    assert_eq!(snapshot(&ws.niwa()), before, "koto wrote under .niwa/");
}

#[test]
fn a_check_verdict_names_the_grader_the_session_that_ran_it_and_the_pointer() {
    let ws = Ws::new();
    let d = ws.record(
        Some(S1),
        &[
            "--kind",
            "verdict",
            "--outcome",
            "pass",
            "--what",
            "criterion c3",
            "--evidence",
            "journal:work-on-12#42",
            "--by",
            "check:koto-decider",
        ],
    );
    let by = &d["decided_by"];
    assert_eq!(by["kind"], "check");
    assert_eq!(by["name"], "koto-decider");
    assert_eq!(slot(&by["ran_in"]), vec![pair(S1, "observed")]);
    assert_eq!(d["evidence"], json!(["journal:work-on-12#42"]));
    assert_eq!(d["authority"]["kind"], "dispatch");
    let listed = ws.list(&["--decider-kind", "check"]);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], d["id"]);
}

#[test]
fn offline_with_no_session_and_no_lineage_a_named_decider_still_records() {
    let ws = Ws::new();
    let by = format!("session:{}", S1);
    let mut args = JUDGMENT.to_vec();
    args.extend(["--by", by.as_str()]);
    let d = ws.record(None, &args);
    assert_eq!(slot(&d["decided_by"]["session"]), vec![pair(S1, "handed")]);
    assert_eq!(
        slot(&d["decided_by"]["agent"]),
        vec![pair("unknown", "observed")]
    );
    assert_eq!(d["authority"]["dispatch_id"], json!([]));
    assert_eq!(d["authority"]["instructions_digest"], json!([]));
    assert_eq!(d["authority"]["dispatcher"], json!([]));
    assert!(links(&d).is_empty());
    assert_eq!(d["chain"]["incomplete"], true);
}

#[test]
fn an_unsafe_harness_session_id_builds_no_path() {
    let ws = Ws::new();
    // A planted file one directory up that a `..` id would reach.
    std::fs::write(
        ws.niwa().join("evil.json"),
        json!({"dispatch_id": "planted", "parent_session_id": COORD}).to_string(),
    )
    .unwrap();
    let mut cmd = ws.koto(Some("../evil"));
    cmd.args([
        "decisions",
        "record",
        "--kind",
        "judgment",
        "--what",
        "x",
        "--by",
        "person:dana",
        "--surface",
        "terminal",
    ]);
    let (code, json) = ws.run_with(cmd, &[]);
    assert_eq!(code, 0, "{}", json);
    let d = &json["decision"];
    assert_eq!(d["decided_by"]["entered_by"], Value::Null);
    assert_eq!(d["authority"]["dispatch_id"], json!([]));
    assert_eq!(d["chain"]["incomplete"], true);
}

fn template(dir: &Path) -> PathBuf {
    let src = dir.join("t.md");
    std::fs::write(
        &src,
        "---\nname: t\nversion: \"1.0\"\ninitial_state: start\nstates:\n  start:\n    transitions:\n      - target: done\n  done:\n    terminal: true\n---\n\n## start\n\nWork.\n\n## done\n\nDone.\n",
    )
    .unwrap();
    src
}

#[test]
fn a_session_resumed_after_its_entry_is_removed_resolves_from_the_journal_stamp() {
    let ws = Ws::new();
    ws.entry(
        S1,
        json!({"dispatch_id": "disp-r", "parent_session_id": COORD}),
    );
    std::fs::create_dir_all(ws.home().join("sessions")).unwrap();
    let src = template(ws.home());
    let out = ws
        .koto(Some(S1))
        .args(["init", "wf", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let journal = std::fs::read_to_string(ws.home().join("sessions/_run_journal.jsonl")).unwrap();
    assert!(
        journal.contains("\"koto.lineage.dispatch.id\":\"disp-r\""),
        "no stamp in {}",
        journal
    );

    // The workspace manager reaps the entry; the session resumes.
    std::fs::remove_file(ws.entry_path(S1)).unwrap();
    let d = ws.record(Some(S1), JUDGMENT);
    assert_eq!(
        slot(&d["authority"]["dispatch_id"]),
        vec![pair("disp-r", "handed")]
    );
    assert_eq!(
        links(&d),
        vec![vec![pair(&format!("session:{}", COORD), "handed")]]
    );
    assert_eq!(d["chain"]["incomplete"], true);
}
