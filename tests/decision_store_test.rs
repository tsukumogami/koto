//! The host-level decision store through the CLI: `koto decisions record
//! --kind`, `show`, the host-wide `list` filters and `report`.
//!
//! Every test points `HOME` at its own temporary directory, which is where
//! the store resolves (`$HOME/.koto/decisions`), and puts sessions under a
//! separate `KOTO_SESSIONS_BASE` so removing a session can be checked
//! against the store.

use assert_cmd::Command;
use assert_fs::TempDir;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn koto(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("HOME", dir);
    cmd.env("KOTO_SESSIONS_BASE", dir.join("sessions"));
    cmd.env_remove("CLAUDE_CODE_SESSION_ID");
    cmd.env_remove("KOTO_WORKFLOWS_DIR");
    for name in [
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "GH_ENTERPRISE_TOKEN",
        "GITHUB_ENTERPRISE_TOKEN",
    ] {
        cmd.env_remove(name);
    }
    cmd
}

fn store(dir: &Path) -> PathBuf {
    dir.join(".koto").join("decisions")
}

/// Run koto and parse its stdout as one JSON object, whatever the exit.
fn run(dir: &Path, args: &[&str]) -> (i32, Value) {
    let out = koto(dir).args(args).output().unwrap();
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

fn ok(dir: &Path, args: &[&str]) -> Value {
    let (code, json) = run(dir, args);
    assert_eq!(code, 0, "koto {:?} failed: {}", args, json);
    json
}

/// A refused record names its field, exits 2 and writes nothing.
fn refused(dir: &Path, args: &[&str], field: &str) {
    let before = month_lines(dir);
    let (code, json) = run(dir, args);
    assert_eq!(code, 2, "expected a refusal for {:?}: {}", args, json);
    assert_eq!(json["field"], field, "wrong field for {:?}: {}", args, json);
    assert_eq!(month_lines(dir), before, "a refused record wrote a line");
}

/// Every line in every month file.
fn month_lines(dir: &Path) -> Vec<String> {
    let root = store(dir);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let n = p.file_name().unwrap().to_string_lossy().to_string();
            n.ends_with(".jsonl") && n.len() == "2026-10.jsonl".len()
        })
        .collect();
    paths.sort();
    paths
        .iter()
        .flat_map(|p| {
            std::fs::read_to_string(p)
                .unwrap()
                .lines()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn two_decisions_get_different_ids_and_keep_their_reason() {
    let dir = TempDir::new().unwrap();
    let a = ok(
        dir.path(),
        &[
            "decisions",
            "record",
            "--kind",
            "judgment",
            "--what",
            "finding 3 is a false positive",
            "--reason",
            "the path is test-only",
        ],
    );
    let b = ok(
        dir.path(),
        &["decisions", "record", "--kind", "judgment", "--what", "x"],
    );
    let (ida, idb) = (a["id"].as_str().unwrap(), b["id"].as_str().unwrap());
    assert_ne!(ida, idb);
    assert!(ida.starts_with("d-") && ida.len() == "d-202610-".len() + 32);
    assert_eq!(a["written"], true);

    let shown = ok(dir.path(), &["decisions", "show", ida]);
    assert_eq!(shown["decision"]["reason"], "the path is test-only");
    assert_eq!(shown["status"], "in_effect");

    let listed = ok(dir.path(), &["decisions", "list"]);
    assert_eq!(listed["count"], 2);
    let item = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["decision"]["id"] == ida)
        .unwrap();
    assert_eq!(item["decision"]["reason"], "the path is test-only");
    assert_eq!(item["status"], "in_effect");
}

#[test]
fn a_rejected_accept_lists_with_its_outcome() {
    let dir = TempDir::new().unwrap();
    ok(
        dir.path(),
        &[
            "decisions",
            "record",
            "--kind",
            "accept",
            "--of",
            "document",
            "--repo",
            "o/r",
            "--path",
            "docs/x.md",
            "--commit",
            "abc123",
            "--outcome",
            "rejected",
        ],
    );
    let listed = ok(dir.path(), &["decisions", "list", "--kind", "accept"]);
    assert_eq!(listed["count"], 1);
    assert_eq!(listed["items"][0]["decision"]["outcome"], "rejected");
    assert_eq!(listed["items"][0]["decision"]["kind"], "accept");
}

#[test]
fn records_outside_the_kinds_outcomes_and_subjects_are_refused_by_field() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    refused(
        d,
        &["decisions", "record", "--kind", "approve", "--what", "x"],
        "kind",
    );
    refused(
        d,
        &[
            "decisions",
            "record",
            "--kind",
            "judgment",
            "--what",
            "x",
            "--outcome",
            "accepted",
        ],
        "outcome",
    );
    refused(
        d,
        &[
            "decisions",
            "record",
            "--kind",
            "accept",
            "--of",
            "document",
            "--repo",
            "o/r",
            "--path",
            "p",
            "--commit",
            "c",
        ],
        "outcome",
    );
    refused(
        d,
        &[
            "decisions",
            "record",
            "--kind",
            "verdict",
            "--what",
            "review",
            "--outcome",
            "pass",
        ],
        "evidence",
    );
    refused(
        d,
        &[
            "decisions",
            "record",
            "--kind",
            "merge",
            "--repo",
            "o/r",
            "--commit",
            "c",
        ],
        "pr",
    );
    refused(
        d,
        &["decisions", "record", "--kind", "reverse", "--what", "x"],
        "kind",
    );
    refused(
        d,
        &[
            "decisions",
            "record",
            "--kind",
            "judgment",
            "--what",
            "x",
            "--pr",
            "4",
        ],
        "pr",
    );
    assert!(month_lines(d).is_empty());
}

#[test]
fn an_evidence_pointer_is_stored_as_given() {
    let dir = TempDir::new().unwrap();
    let r = ok(
        dir.path(),
        &[
            "decisions",
            "record",
            "--kind",
            "judgment",
            "--what",
            "dismissed",
            "--evidence",
            "harness:toolu_01ABCdef",
        ],
    );
    let shown = ok(
        dir.path(),
        &["decisions", "show", r["id"].as_str().unwrap()],
    );
    assert_eq!(
        shown["decision"]["evidence"],
        serde_json::json!(["harness:toolu_01ABCdef"])
    );
    let lines = month_lines(dir.path());
    assert_eq!(lines.len(), 1);
    let line: Value = serde_json::from_str(&lines[0]).unwrap();
    // The line holds the pointer and nothing copied from the harness.
    assert_eq!(
        line["evidence"],
        serde_json::json!(["harness:toolu_01ABCdef"])
    );
    assert!(line.get("tool_call").is_none() && line.get("transcript").is_none());
}

#[test]
fn a_verdict_on_a_change_carries_repo_and_commit() {
    let dir = TempDir::new().unwrap();
    let base = [
        "decisions",
        "record",
        "--kind",
        "verdict",
        "--outcome",
        "pass",
        "--of",
        "change",
        "--what",
        "review of PR 12",
        "--evidence",
        "journal:work-on-12#40",
        "--repo",
        "o/r",
    ];
    refused(dir.path(), &base, "commit");
    let mut with_commit = base.to_vec();
    with_commit.extend(["--commit", "deadbeef"]);
    ok(dir.path(), &with_commit);
    let listed = ok(dir.path(), &["decisions", "list", "--kind", "verdict"]);
    let s = &listed["items"][0]["decision"]["subject"];
    assert_eq!(s["repo"], "o/r");
    assert_eq!(s["commit"], "deadbeef");
}

#[test]
fn an_accepted_intent_lists_its_location_and_source_link() {
    let dir = TempDir::new().unwrap();
    ok(
        dir.path(),
        &[
            "decisions",
            "record",
            "--kind",
            "accept",
            "--outcome",
            "accepted",
            "--of",
            "intent",
            "--intent-location",
            "forge:o/r#88",
            "--source-link",
            "https://tickets.example.com/T-5",
        ],
    );
    let listed = ok(
        dir.path(),
        &[
            "decisions",
            "list",
            "--subject",
            "intent_location=forge:o/r#88",
        ],
    );
    assert_eq!(listed["count"], 1);
    let s = &listed["items"][0]["decision"]["subject"];
    assert_eq!(s["intent_location"], "forge:o/r#88");
    assert_eq!(s["source_link"], "https://tickets.example.com/T-5");
}

#[test]
fn links_must_be_plain_https() {
    let dir = TempDir::new().unwrap();
    for bad in [
        "http://example.com/r/1",
        "https://user:pw@example.com/r/1",
        "https://example.com/r/1?token=x",
        "example.com/r/1",
    ] {
        refused(
            dir.path(),
            &[
                "decisions",
                "record",
                "--kind",
                "judgment",
                "--what",
                "x",
                "--link",
                bad,
            ],
            "link",
        );
    }
    ok(
        dir.path(),
        &[
            "decisions",
            "record",
            "--kind",
            "judgment",
            "--what",
            "x",
            "--link",
            "https://example.com/o/r/pull/1#review-3",
        ],
    );
}

#[test]
fn a_known_credential_in_a_reason_is_redacted_before_it_is_written() {
    let dir = TempDir::new().unwrap();
    let secret = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    let out = koto(dir.path())
        .env("GH_TOKEN", secret)
        .args([
            "decisions",
            "record",
            "--kind",
            "judgment",
            "--what",
            "x",
            "--reason",
            &format!("used token {} here", secret),
            "--evidence",
            &format!("note:{}", secret),
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let lines = month_lines(dir.path()).join("\n");
    assert!(!lines.contains(secret), "the credential reached the store");
    assert!(lines.contains("GH_TOKEN"), "no redaction marker: {}", lines);
}

#[test]
fn the_store_files_are_private_and_symlinks_are_refused() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    ok(
        dir.path(),
        &["decisions", "record", "--kind", "judgment", "--what", "x"],
    );
    let root = store(dir.path());
    let mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
    for entry in std::fs::read_dir(&root).unwrap() {
        let p = entry.unwrap().path();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{} has mode {:o}", p.display(), mode);
    }

    // A symlink at the current month's file is refused, not followed.
    let month = month_files(&root).pop().unwrap();
    let target = dir.path().join("elsewhere.jsonl");
    std::fs::write(&target, "").unwrap();
    std::fs::remove_file(&month).unwrap();
    std::os::unix::fs::symlink(&target, &month).unwrap();
    let (code, json) = run(
        dir.path(),
        &["decisions", "record", "--kind", "judgment", "--what", "y"],
    );
    assert_ne!(code, 0, "a write followed a symlink: {}", json);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "");
}

fn month_files(root: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let n = p.file_name().unwrap().to_string_lossy().to_string();
            n.ends_with(".jsonl") && n.len() == "2026-10.jsonl".len()
        })
        .collect();
    v.sort();
    v
}

#[test]
fn a_store_writable_by_others_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let root = store(dir.path());
    std::fs::create_dir_all(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o777)).unwrap();
    let (code, json) = run(
        dir.path(),
        &["decisions", "record", "--kind", "judgment", "--what", "x"],
    );
    assert_eq!(code, 1, "{}", json);
    assert!(json["error"]
        .as_str()
        .unwrap()
        .contains("writable by group or others"));
    let (code, _) = run(dir.path(), &["decisions", "list"]);
    assert_eq!(code, 1, "a reader used an unsafe store");
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn show_of_an_unknown_id_is_an_error_object() {
    let dir = TempDir::new().unwrap();
    let (code, json) = run(
        dir.path(),
        &[
            "decisions",
            "show",
            "d-202610-00000000000000000000000000000000",
        ],
    );
    assert_eq!(code, 1);
    assert_eq!(json["field"], "id");
}

#[test]
fn report_counts_the_window() {
    let dir = TempDir::new().unwrap();
    ok(
        dir.path(),
        &["decisions", "record", "--kind", "judgment", "--what", "x"],
    );
    ok(
        dir.path(),
        &[
            "decisions",
            "record",
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
    let r = ok(
        dir.path(),
        &[
            "decisions",
            "report",
            "--since",
            "2000-01",
            "--until",
            "2100-01",
        ],
    );
    assert_eq!(r["decisions"]["total"], 2);
    // The gate pass was recorded with no named decider, so it is defaulted.
    assert_eq!(r["gate_pass_defaulted"], 1);
    let (code, json) = run(
        dir.path(),
        &[
            "decisions",
            "report",
            "--since",
            "2026-11",
            "--until",
            "2026-10",
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(json["field"], "until");
}

#[test]
fn two_processes_recording_at_once_lose_nothing() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_path_buf();
    // Seed one entry so "no existing entry changes" has something to check.
    ok(
        &path,
        &[
            "decisions",
            "record",
            "--kind",
            "judgment",
            "--what",
            "seed",
        ],
    );
    let seed = month_lines(&path);

    let workers: Vec<_> = (0..2)
        .map(|w| {
            let path = path.clone();
            std::thread::spawn(move || {
                for i in 0..100 {
                    let what = format!("w{}-{}", w, i);
                    let out = koto(&path)
                        .args(["decisions", "record", "--kind", "judgment", "--what", &what])
                        .output()
                        .unwrap();
                    assert!(
                        out.status.success(),
                        "{}",
                        String::from_utf8_lossy(&out.stdout)
                    );
                }
            })
        })
        .collect();
    for w in workers {
        w.join().unwrap();
    }

    let lines = month_lines(&path);
    assert_eq!(lines.len(), 201);
    assert_eq!(lines[0], seed[0], "an existing entry changed");
    let mut whats = std::collections::BTreeSet::new();
    for line in &lines {
        let v: Value = serde_json::from_str(line).expect("a corrupt entry");
        whats.insert(v["subject"]["what"].as_str().unwrap().to_string());
    }
    assert_eq!(whats.len(), 201);
}

fn template(dir: &Path) -> PathBuf {
    let src = dir.join("t.md");
    std::fs::write(
        &src,
        r#"---
name: t
version: "1.0"
initial_state: start
states:
  start:
    accepts:
      go:
        type: string
        required: false
    transitions:
      - target: done
  done:
    terminal: true
---

## start

Work.

## done

Done.
"#,
    )
    .unwrap();
    src
}

#[test]
fn todays_workflow_forms_are_unchanged_and_cleanup_leaves_the_store() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::create_dir_all(d.join("sessions")).unwrap();
    let src = template(d);
    let out = koto(d)
        .args(["init", "wf", "--template", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    let rec = ok(
        d,
        &[
            "decisions",
            "record",
            "wf",
            "--with-data",
            r#"{"choice":"a","rationale":"b"}"#,
        ],
    );
    assert_eq!(rec["decisions_recorded"], 1);
    assert_eq!(rec["state"], "start");
    let list = ok(d, &["decisions", "list", "wf"]);
    assert_eq!(list["state"], "start");
    assert_eq!(list["decisions"]["count"], 1);
    assert_eq!(list["decisions"]["items"][0]["choice"], "a");

    // A workflow name with host-wide filters is refused rather than
    // silently dropping either.
    let (code, _) = run(d, &["decisions", "list", "wf", "--kind", "judgment"]);
    assert_eq!(code, 2);

    ok(
        d,
        &[
            "decisions",
            "record",
            "--kind",
            "judgment",
            "--what",
            "kept after cleanup",
        ],
    );
    let before = month_lines(d);
    let out = koto(d).args(["session", "cleanup", "wf"]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(!d.join("sessions").join("wf").exists());
    assert_eq!(month_lines(d), before);
    assert_eq!(ok(d, &["decisions", "list"])["count"], 1);
}
