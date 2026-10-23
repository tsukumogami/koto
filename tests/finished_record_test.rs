//! Finished records: `koto status` answers for a session koto removed
//! itself, driven through the real binary in a scratch HOME.

#![cfg(unix)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

/// `start` waits for `go`; `yes` reaches the success terminal `done`, `fail`
/// the failure terminal `failed`.
const TEMPLATE: &str = r#"---
name: finished-test
version: "1.0"
initial_state: start
states:
  start:
    accepts:
      go:
        type: enum
        required: true
        values: ["yes", "fail"]
    transitions:
      - target: done
        when:
          go: "yes"
      - target: failed
        when:
          go: "fail"
  done:
    terminal: true
  failed:
    terminal: true
    failure: true
---

## start

Start.

## done

Done.

## failed

Failed.
"#;

struct Env {
    _tmp: TempDir,
    home: PathBuf,
    cache: PathBuf,
    work: PathBuf,
    template: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let home = root.join("home");
        let cache = root.join("cache");
        let work = root.join("work");
        for d in [&home, &cache, &work] {
            std::fs::create_dir_all(d).unwrap();
        }
        let template = root.join("finished-test.md");
        std::fs::write(&template, TEMPLATE).unwrap();
        Env {
            _tmp: tmp,
            home,
            cache,
            work,
            template,
        }
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        self.run_env(args, &[])
    }

    fn run_env(&self, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
        Command::cargo_bin("koto")
            .unwrap()
            .envs(env.iter().copied())
            .current_dir(&self.work)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("KOTO_SESSIONS_BASE")
            .env_remove("KOTO_WORKFLOWS_DIR")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .args(args)
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "koto {args:?} failed: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }

    fn init(&self, name: &str) {
        self.ok(&["init", name, "--template", self.template.to_str().unwrap()]);
    }

    fn init_child(&self, name: &str, parent: &str) {
        self.ok(&[
            "init",
            name,
            "--template",
            self.template.to_str().unwrap(),
            "--parent",
            parent,
        ]);
    }

    fn next(&self, name: &str, go: &str, keep: bool) -> std::process::Output {
        let data = format!("{{\"go\": \"{go}\"}}");
        let mut args = vec!["next", name, "--with-data", data.as_str()];
        if keep {
            args.push("--no-cleanup");
        }
        self.run(&args)
    }

    fn finish(&self, name: &str, go: &str, keep: bool) {
        let out = self.next(name, go, keep);
        assert!(
            out.status.success(),
            "next {name} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn sessions(&self) -> PathBuf {
        self.home.join(".koto/sessions")
    }

    fn record_path(&self, name: &str) -> PathBuf {
        self.sessions()
            .join(".finished")
            .join(format!("{name}.json"))
    }

    fn record(&self, name: &str) -> Value {
        serde_json::from_slice(&std::fs::read(self.record_path(name)).unwrap()).unwrap()
    }

    fn status(&self, name: &str) -> std::process::Output {
        self.run(&["status", name])
    }
}

fn json(out: &std::process::Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "not JSON ({e}): {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn assert_not_found(out: &std::process::Output) {
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("not found"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn status_answers_for_a_removed_session_as_it_would_for_a_kept_one() {
    let env = Env::new();
    env.init("removed");
    env.finish("removed", "yes", false);
    env.init("kept");
    env.finish("kept", "yes", true);
    assert!(!env.sessions().join("removed").exists());
    assert!(env.sessions().join("kept").is_dir());

    let removed = json(&env.status("removed"));
    let kept = json(&env.status("kept"));
    for field in ["current_state", "is_terminal", "result"] {
        assert_eq!(removed[field], kept[field], "{field}");
    }
    assert_eq!(removed["current_state"], "done");
    assert_eq!(removed["is_terminal"], true);
    assert_eq!(removed["removed"], true);
    assert!(removed["finished_at"].is_string());
    assert!(removed["session_id"].is_string());
    assert_eq!(removed["execution_dir"], env.work.to_str().unwrap());

    let record = env.record("removed");
    assert_eq!(record["v"], 1);
    assert_eq!(record["removed_by"], "terminal");
    assert_eq!(record["status"], "terminal");
    assert_eq!(record["failure"], false);
    use std::os::unix::fs::PermissionsExt;
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&env.sessions().join(".finished")), 0o700);
    assert_eq!(mode(&env.record_path("removed")), 0o600);
}

#[test]
fn an_unknown_name_and_a_pruned_record_answer_not_found() {
    let env = Env::new();
    assert_not_found(&env.status("never-created"));

    env.init("gone");
    env.finish("gone", "yes", false);
    assert_eq!(env.status("gone").status.code(), Some(0));
    let out = env.ok(&["session", "prune", "--apply", "--all"]);
    let removed: Vec<&str> = out["removed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(removed, vec!["gone"]);
    assert_eq!(out["removed"][0]["kind"], "record");
    assert_not_found(&env.status("gone"));
}

/// Leave a finished record beside a kept session, as a crash between the
/// record's write and the session's removal would.
fn crash_leaves_a_record(env: &Env, name: &str) {
    // Kept at its terminal with `--no-cleanup`, so the session is still there.
    env.init(name);
    env.finish(name, "yes", true);
    // A donor session koto removes gives a real record to copy under `name`;
    // the copy's own fields aren't checked.
    env.init("donor");
    env.finish("donor", "yes", false);
    let mut record = env.record("donor");
    record["name"] = Value::from(name);
    std::fs::write(env.record_path(name), serde_json::to_vec(&record).unwrap()).unwrap();
    std::fs::remove_file(env.record_path("donor")).unwrap();
    assert!(env.sessions().join(name).is_dir());
    assert!(env.record_path(name).exists());
}

#[test]
fn cleanup_of_a_name_koto_already_removed_keeps_its_record() {
    let env = Env::new();
    env.init("done-already");
    env.finish("done-already", "yes", false);
    assert!(!env.sessions().join("done-already").exists());
    // Cleanup is idempotent; on a name koto removed it removes nothing, and
    // the record stays the status answer.
    env.ok(&["session", "cleanup", "done-already"]);
    assert!(env.record_path("done-already").exists());
    assert_eq!(json(&env.status("done-already"))["removed"], true);
}

#[test]
fn a_persons_removal_deletes_a_record_a_crash_left_behind() {
    let env = Env::new();
    crash_leaves_a_record(&env, "cleaned");
    env.ok(&["session", "cleanup", "cleaned"]);
    assert!(!env.record_path("cleaned").exists());
    assert_not_found(&env.status("cleaned"));

    // A session at its terminal can't be cancelled, so this one is rewound
    // back to work first, the way a record beside a kept session meets
    // `cancel --cleanup`.
    crash_leaves_a_record(&env, "cancelled");
    env.ok(&["rewind", "cancelled"]);
    env.ok(&["cancel", "cancelled", "--cleanup"]);
    assert!(!env.record_path("cancelled").exists());
    assert_not_found(&env.status("cancelled"));
}

#[test]
fn only_a_removal_koto_makes_itself_writes_a_record() {
    let env = Env::new();
    // Kept at a failure terminal, and kept with --no-cleanup.
    env.init("failed");
    env.finish("failed", "fail", false);
    env.init("no-cleanup");
    env.finish("no-cleanup", "yes", true);
    // Removed by a person.
    env.init("cancelled");
    env.ok(&["cancel", "cancelled", "--cleanup"]);
    env.init("cleaned");
    env.ok(&["session", "cleanup", "cleaned"]);
    for name in ["failed", "no-cleanup", "cancelled", "cleaned"] {
        assert!(!env.record_path(name).exists(), "{name} has a record");
    }
    assert_not_found(&env.status("cancelled"));
    assert_not_found(&env.status("cleaned"));
}

#[test]
fn creating_a_session_deletes_its_names_record_and_nothing_lists_records() {
    let env = Env::new();
    env.init("again");
    env.finish("again", "yes", false);
    assert!(env.record_path("again").exists());

    let list = env.ok(&["session", "list"]);
    assert!(list.as_array().unwrap().is_empty(), "{list}");
    let workflows = env.ok(&["workflows"]);
    assert!(workflows.as_array().unwrap().is_empty(), "{workflows}");

    env.init("again");
    assert!(!env.record_path("again").exists());
    let status = json(&env.status("again"));
    assert_eq!(status["current_state"], "start");
    assert!(status.get("removed").is_none());
}

#[test]
fn a_cut_result_is_flagged_in_the_status_answer() {
    let env = Env::new();
    env.init("big");
    env.finish("big", "yes", false);
    let mut record = env.record("big");
    record["truncated"] = Value::Bool(true);
    std::fs::write(env.record_path("big"), serde_json::to_vec(&record).unwrap()).unwrap();
    let status = json(&env.status("big"));
    assert_eq!(status["result_truncated"], true);
    assert_eq!(status["result"], record["result"]);
}

#[test]
fn a_record_that_cannot_be_written_warns_and_the_session_is_still_removed() {
    let env = Env::new();
    env.init("unrecorded");
    // A file where the records directory belongs.
    std::fs::write(env.sessions().join(".finished"), "not a directory").unwrap();
    let out = env.next("unrecorded", "yes", false);
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        stderr
            .matches("could not write the finished record")
            .count(),
        1,
        "{stderr}"
    );
    assert!(!env.sessions().join("unrecorded").exists());
    assert_not_found(&env.status("unrecorded"));
}

#[test]
fn a_parent_records_each_kept_child_it_removes() {
    let env = Env::new();
    env.init("parent");
    env.init_child("child-done", "parent");
    env.finish("child-done", "yes", true);
    env.init_child("child-cancelled", "parent");
    env.ok(&["cancel", "child-cancelled"]);
    let kept_done = json(&env.status("child-done"));

    // The parent finishes and is kept with its hierarchy, children and all.
    env.finish("parent", "yes", false);
    for name in ["parent", "child-done", "child-cancelled"] {
        assert!(env.sessions().join(name).exists(), "{name} is gone");
    }
    // Once the retention passes, a replace of the parent releases the old
    // run and removes its kept children with it.
    std::thread::sleep(std::time::Duration::from_millis(1300));
    let out = env.run_env(
        &[
            "init",
            "parent",
            "--template",
            env.template.to_str().unwrap(),
            "--replace-terminal",
        ],
        &[("KOTO_RETENTION_KEEP", "1s")],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(json(&out).get("moved_to").is_none(), "released, not moved");
    for name in ["child-done", "child-cancelled"] {
        assert!(!env.sessions().join(name).exists(), "{name} is still there");
    }

    let done = env.record("child-done");
    assert_eq!(done["removed_by"], "parent");
    assert_eq!(done["status"], "terminal");
    let status = json(&env.status("child-done"));
    assert_eq!(status["result"], kept_done["result"]);
    assert_eq!(status["current_state"], "done");

    let cancelled = env.record("child-cancelled");
    assert_eq!(cancelled["removed_by"], "parent");
    assert_eq!(cancelled["status"], "cancelled");
    assert_eq!(cancelled["state"], "start");
    assert!(cancelled["result"].is_null());
    let status = json(&env.status("child-cancelled"));
    assert_eq!(status["is_terminal"], true);
    assert_eq!(status["status"], "cancelled");
    assert_eq!(status["current_state"], "start");
}

#[test]
fn prune_lists_records_under_its_filters_and_in_its_token() {
    let env = Env::new();
    env.init("rec-a");
    env.finish("rec-a", "yes", false);
    env.init("rec-b");
    env.finish("rec-b", "yes", false);

    let preview = env.ok(&["session", "prune"]);
    let kinds: Vec<(String, String)> = preview["remove"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["kind"].as_str().unwrap().to_string(),
                e["name"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("record".to_string(), "rec-a".to_string()),
            ("record".to_string(), "rec-b".to_string())
        ]
    );
    let token = preview["plan"].as_str().unwrap().to_string();

    // The directory filter reads the record's execution directory.
    let elsewhere = env.ok(&[
        "session",
        "prune",
        "--execution-dir",
        "/no/such/dir/for/koto",
    ]);
    assert!(elsewhere["remove"].as_array().unwrap().is_empty());
    // The age filter reads the record's finish time: nothing is a day old,
    // and everything is at least zero seconds old.
    let old = env.ok(&["session", "prune", "--older-than", "1d"]);
    assert!(old["remove"].as_array().unwrap().is_empty());
    let any = env.ok(&["session", "prune", "--older-than", "0s"]);
    assert_eq!(any["remove"].as_array().unwrap().len(), 2);

    // A record that changes after the preview changes the token.
    env.init("rec-b");
    env.finish("rec-b", "yes", false);
    let out = env.run(&["session", "prune", "--apply", "--plan", &token]);
    assert_eq!(out.status.code(), Some(4));
    assert!(env.record_path("rec-a").exists());

    let preview = env.ok(&["session", "prune"]);
    let token = preview["plan"].as_str().unwrap().to_string();
    let out = env.ok(&["session", "prune", "--apply", "--plan", &token]);
    assert_eq!(out["removed"].as_array().unwrap().len(), 2);
    assert!(!env.record_path("rec-a").exists());
    assert!(!env.record_path("rec-b").exists());
}
