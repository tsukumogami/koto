//! Background pooled actions (DESIGN-host-resource-pools.md, PLAN step 2).
//!
//! A `default_action` with `background: true` and `timeout_secs` runs its
//! command through a detached `koto __run-action` runner. The tick that
//! starts it returns at once with a temporal `__action__` condition, the
//! run keeps its pool slot for the command's real length, and the first
//! tick after the command ends delivers the result as the action's outcome.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use assert_cmd::Command;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

/// A koto root shared by several workspaces, each its own directory.
struct Host {
    tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Host {
    fn new(config: Option<&str>) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("koto-root");
        std::fs::create_dir_all(&root).unwrap();
        if let Some(text) = config {
            std::fs::write(root.join("config.toml"), text).unwrap();
        }
        Host { tmp, root }
    }

    fn workspace(&self, name: &str) -> PathBuf {
        let dir = self.tmp.path().join(name);
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        dir
    }

    fn koto(&self, ws: &Path) -> Command {
        let mut cmd = koto_command::koto_std();
        cmd.env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("KOTO_TICK_SESSION")
            .env_remove("KOTO_BACKGROUND_OF")
            .current_dir(ws)
            .env("HOME", self.tmp.path())
            .env("KOTO_HOME", &self.root)
            .env("KOTO_SESSIONS_BASE", ws.join("sessions"));
        Command::from_std(cmd)
    }

    fn init(&self, ws: &Path, name: &str, template: &str) {
        let src = ws.join(format!("{name}.md"));
        std::fs::write(&src, template).unwrap();
        let out = self
            .koto(ws)
            .args(["init", name, "--template", src.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "init failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// One tick, kept on a terminal state so its log stays readable.
    fn next(&self, ws: &Path, name: &str) -> serde_json::Value {
        let out = self
            .koto(ws)
            .args(["next", name, "--no-cleanup"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "next should exit 0: stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).expect("next prints JSON")
    }

    /// The session's log, header left out.
    fn events(&self, ws: &Path, name: &str) -> Vec<serde_json::Value> {
        let file = format!("koto-{name}.state.jsonl");
        let path = find(&ws.join("sessions"), &file).expect("the session log");
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .skip(1)
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    /// The session's run directories.
    fn runs(&self, ws: &Path, name: &str) -> Vec<PathBuf> {
        let file = format!("koto-{name}.state.jsonl");
        let dir = find(&ws.join("sessions"), &file)
            .expect("the session log")
            .parent()
            .unwrap()
            .join("runs");
        let mut runs: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|d| d.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        runs.sort();
        runs
    }
}

/// The first file named `name` below `dir`.
fn find(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find(&path, name) {
                return Some(found);
            }
        } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
            return Some(path);
        }
    }
    None
}

fn of_type<'a>(events: &'a [serde_json::Value], ty: &str) -> Vec<&'a serde_json::Value> {
    events.iter().filter(|e| e["type"] == ty).collect()
}

/// A `test` state whose background action runs `command`, then `done`.
/// `gates` is the state's `gates:` block, if any.
fn template(command: &str, action_extra: &str, gates: &str) -> String {
    format!(
        r#"---
name: pool-background
version: "1.0"
initial_state: test
states:
  test:
    default_action:
      command: {command:?}
      background: true
{action_extra}{gates}    transitions:
      - target: done
  done:
    terminal: true
---

## test

Run the suite.

## done

Done.
"#
    )
}

const SIZE_ONE: &str = "[pools.test-suite]\nsize = 1\nwait_secs = 0\n";

/// The one `__action__` condition of a blocked response.
fn action_condition(resp: &serde_json::Value) -> &serde_json::Value {
    assert_eq!(resp["action"], "gate_blocked", "got {resp}");
    let conditions = resp["blocking_conditions"].as_array().unwrap();
    assert_eq!(conditions.len(), 1, "got {conditions:?}");
    assert_eq!(conditions[0]["name"], "__action__");
    &conditions[0]
}

/// The running condition's output, checked for its fixed fields.
fn running(resp: &serde_json::Value, timeout_secs: u64) -> &serde_json::Value {
    let c = action_condition(resp);
    assert_eq!(c["status"], "running", "got {c}");
    assert_eq!(c["category"], "temporal");
    assert_eq!(c["agent_actionable"], false);
    assert_eq!(c["output"]["timeout_secs"], timeout_secs);
    &c["output"]
}

fn wait_for(path: &Path, limit: Duration) {
    let started = Instant::now();
    while !path.exists() {
        assert!(
            started.elapsed() < limit,
            "{} never appeared",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ---------------------------------------------------------------------------
// the run holds the slot for the command's length
// ---------------------------------------------------------------------------

#[test]
fn a_ninety_second_run_holds_the_slot_and_queues_a_second_session() {
    let host = Host::new(Some(SIZE_ONE));
    let body = template(
        "sleep 90",
        "      pool: test-suite\n      timeout_secs: 200\n",
        "",
    );
    let ws_a = host.workspace("ws-a");
    let ws_b = host.workspace("ws-b");
    host.init(&ws_a, "a", &body);
    host.init(&ws_b, "b", &body);

    let started = Instant::now();
    let first = host.next(&ws_a, "a");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the starting tick took {:?}",
        started.elapsed()
    );
    let run_id = running(&first, 200)["run_id"].as_str().unwrap().to_string();
    let result = host.runs(&ws_a, "a")[0].join("result.json");

    // Until the command exits, every tick reports the same run, older each
    // time, and the second session is queued.
    let mut last_elapsed = 0;
    let mut ticks = 0;
    while !result.exists() {
        let resp = host.next(&ws_a, "a");
        if result.exists() {
            break;
        }
        let out = running(&resp, 200);
        assert_eq!(out["run_id"], run_id.as_str());
        let elapsed = out["elapsed_secs"].as_u64().unwrap();
        assert!(elapsed >= last_elapsed, "{elapsed} < {last_elapsed}");
        last_elapsed = elapsed;
        ticks += 1;

        let queued = host.next(&ws_b, "b");
        let c = &queued["blocking_conditions"][0];
        assert_eq!(c["name"], "__pool__", "got {queued}");
        assert_eq!(c["status"], "queued");
        assert_eq!(c["output"]["holders"][0]["session"], "a");
        std::thread::sleep(Duration::from_secs(10));
    }
    assert!(ticks >= 8, "only {ticks} ticks saw the run");
    assert!(
        last_elapsed >= 80,
        "the run read as running for {last_elapsed}s"
    );
    assert_eq!(host.runs(&ws_a, "a").len(), 1, "a second copy started");

    // The first tick after the command exits delivers it and advances.
    let done = host.next(&ws_a, "a");
    assert_eq!(done["state"], "done", "got {done}");
    let taken = host.next(&ws_b, "b");
    running(&taken, 200);

    let a = host.events(&ws_a, "a");
    let b = host.events(&ws_b, "b");
    let finished = of_type(&a, "background_finished");
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0]["payload"]["run_id"], run_id.as_str());
    assert_eq!(finished[0]["payload"]["outcome"], "done");
    let released = of_type(&a, "pool_released");
    assert_eq!(released.len(), 1);
    assert_eq!(released[0]["payload"]["reason"], "done");
    let acquired = of_type(&b, "pool_acquired");
    assert_eq!(acquired.len(), 1, "b took the slot once");
    let finished_at = finished[0]["timestamp"].as_str().unwrap();
    let acquired_at = acquired[0]["timestamp"].as_str().unwrap();
    println!("a background_finished {finished_at}; b pool_acquired {acquired_at}");
    assert!(acquired_at > finished_at, "{acquired_at} <= {finished_at}");
}

// ---------------------------------------------------------------------------
// the result is the action's outcome
// ---------------------------------------------------------------------------

#[test]
fn a_finished_run_delivers_its_capture_evaluates_gates_and_advances() {
    let host = Host::new(Some(SIZE_ONE));
    let ws = host.workspace("ws-c");
    host.init(
        &ws,
        "c",
        &template(
            "sleep 1; echo ran",
            "      pool: test-suite\n      timeout_secs: 30\n      capture_stdout_as: OUT\n",
            "    gates:\n      got:\n        type: command\n        command: 'test \"{{OUT}}\" = ran'\n",
        ),
    );
    running(&host.next(&ws, "c"), 30);
    wait_for(
        &host.runs(&ws, "c")[0].join("result.json"),
        Duration::from_secs(20),
    );

    let resp = host.next(&ws, "c");
    assert_eq!(resp["state"], "done", "got {resp}");
    let events = host.events(&ws, "c");
    let captured = of_type(&events, "variable_captured");
    assert_eq!(captured[0]["payload"]["value"], "ran");
    let gate = of_type(&events, "gate_evaluated");
    assert_eq!(gate.len(), 1);
    let executed = of_type(&events, "default_action_executed");
    assert_eq!(executed[0]["payload"]["exit_code"], 0);
}

#[test]
fn a_nonzero_exit_fails_the_action_and_the_next_tick_starts_a_new_run() {
    let host = Host::new(Some(SIZE_ONE));
    let ws = host.workspace("ws-d");
    host.init(
        &ws,
        "d",
        &template(
            "exit 3",
            "      pool: test-suite\n      timeout_secs: 30\n",
            "",
        ),
    );
    let first = host.next(&ws, "d");
    let run_id = running(&first, 30)["run_id"].as_str().unwrap().to_string();
    wait_for(
        &host.runs(&ws, "d")[0].join("result.json"),
        Duration::from_secs(20),
    );

    let failed = host.next(&ws, "d");
    let c = action_condition(&failed);
    assert_eq!(c["category"], "corrective");
    assert_eq!(c["output"]["failure_kind"], "nonzero_exit", "got {c}");
    assert_eq!(c["output"]["exit_code"], 3);

    let again = host.next(&ws, "d");
    let next_id = running(&again, 30)["run_id"].as_str().unwrap().to_string();
    assert_ne!(next_id, run_id);
    assert_eq!(host.runs(&ws, "d").len(), 2);
    let released = of_type(&host.events(&ws, "d"), "pool_released")
        .into_iter()
        .map(|e| e["payload"]["reason"].clone())
        .collect::<Vec<_>>();
    assert_eq!(released, vec!["failed"]);
}

#[test]
fn a_command_outliving_its_timeout_is_killed_and_reported_timed_out() {
    let host = Host::new(Some(SIZE_ONE));
    let ws = host.workspace("ws-e");
    let pid = ws.join("pid");
    host.init(
        &ws,
        "e",
        &template(
            &format!("echo $$ > {}; exec sleep 60", pid.display()),
            "      pool: test-suite\n      timeout_secs: 5\n",
            "",
        ),
    );
    let started = Instant::now();
    running(&host.next(&ws, "e"), 5);
    wait_for(&pid, Duration::from_secs(5));
    let pid: i32 = std::fs::read_to_string(&pid)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let result = host.runs(&ws, "e")[0].join("result.json");
    wait_for(&result, Duration::from_secs(7));
    // SAFETY: signal 0 only asks whether the process exists.
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    assert!(!alive, "the command outlived its timeout");
    assert!(
        started.elapsed() < Duration::from_secs(7),
        "{:?}",
        started.elapsed()
    );

    let resp = host.next(&ws, "e");
    let c = action_condition(&resp);
    assert_eq!(c["status"], "timed_out", "got {c}");
    assert_eq!(c["output"]["failure_kind"], "timed_out");
    let events = host.events(&ws, "e");
    let released = of_type(&events, "pool_released");
    assert_eq!(released[0]["payload"]["reason"], "timed_out");
}

#[test]
fn a_background_action_without_a_pool_runs_without_a_slot() {
    let host = Host::new(None);
    let ws = host.workspace("ws-f");
    host.init(
        &ws,
        "f",
        &template("echo free", "      timeout_secs: 30\n", ""),
    );
    let first = host.next(&ws, "f");
    let out = running(&first, 30);
    assert!(out.get("pool").is_none(), "got {out}");
    wait_for(
        &host.runs(&ws, "f")[0].join("result.json"),
        Duration::from_secs(20),
    );
    let resp = host.next(&ws, "f");
    assert_eq!(resp["state"], "done", "got {resp}");
    let events = host.events(&ws, "f");
    assert!(of_type(&events, "pool_acquired").is_empty());
    assert!(of_type(&events, "pool_released").is_empty());
    assert_eq!(of_type(&events, "background_started").len(), 1);
    assert!(!host.root.join("pools").exists());
}

// ---------------------------------------------------------------------------
// compile
// ---------------------------------------------------------------------------

#[test]
fn compile_refuses_a_background_action_that_cannot_run_detached() {
    let host = Host::new(None);
    let ws = host.workspace("compile");
    for (extra, field) in [
        ("", "timeout_secs"),
        (
            "      timeout_secs: 30\n      requires_confirmation: true\n",
            "requires_confirmation",
        ),
        (
            "      timeout_secs: 30\n      polling:\n        interval_secs: 1\n        timeout_secs: 10\n",
            "polling",
        ),
    ] {
        let src = ws.join("t.md");
        std::fs::write(&src, template("echo ran", extra, "")).unwrap();
        let out = host
            .koto(&ws)
            .args(["template", "compile", src.to_str().unwrap()])
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!out.status.success(), "{extra} should be refused: {text}");
        assert!(text.contains("test"), "the error names the state: {text}");
        assert!(text.contains(field), "the error names {field}: {text}");
    }
}
