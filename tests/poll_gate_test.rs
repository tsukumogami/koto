//! Polling command gates (DESIGN-koto-ci-wait-stale-keys.md, Decisions 4-6).
//!
//! These tests drive the real binary with a gate script that counts its runs
//! and exits with the next code from a scripted list, so a test states
//! exactly how the check answers on each run: 0 done, 75 pending, anything
//! else failed. They check the response a tick returns and the
//! `gate_evaluated` events the log keeps.

#![cfg(unix)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use assert_fs::TempDir;
use serde_json::Value;

// ===== Harness =====

fn sessions_base(dir: &Path) -> PathBuf {
    let base = dir.join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(dir));
    cmd.env("HOME", dir);
    cmd.env_remove("CLAUDE_CODE_SESSION_ID");
    cmd.env_remove("KOTO_WORKFLOWS_DIR");
    cmd
}

fn run(dir: &Path, args: &[&str]) -> std::process::Output {
    koto_cmd(dir).args(args).output().unwrap()
}

fn run_ok(dir: &Path, args: &[&str]) -> std::process::Output {
    let out = run(dir, args);
    assert!(
        out.status.success(),
        "koto {args:?} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn next(dir: &Path, name: &str, data: Option<&str>) -> Value {
    let mut args = vec!["next", name, "--no-cleanup"];
    if let Some(d) = data {
        args.push("--with-data");
        args.push(d);
    }
    let out = run(dir, &args);
    serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
        panic!(
            "invalid JSON from next: stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn events(dir: &Path, name: &str) -> Vec<Value> {
    let path = sessions_base(dir)
        .join(name)
        .join(format!("koto-{name}.state.jsonl"));
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .skip(1)
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// Every `gate_evaluated` for gate `ci`, in log order.
fn ci_evals(dir: &Path) -> Vec<Value> {
    events(dir, "wf")
        .into_iter()
        .filter(|e| e["type"] == "gate_evaluated" && e["payload"]["gate"] == "ci")
        .collect()
}

fn runs(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("runs.log"))
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

fn ci_condition(resp: &Value) -> Value {
    resp["blocking_conditions"]
        .as_array()
        .unwrap_or_else(|| panic!("no blocking conditions: {resp}"))
        .iter()
        .find(|c| c["name"] == "ci")
        .cloned()
        .unwrap_or_else(|| panic!("no ci condition: {resp}"))
}

/// The gate script: each run appends to `runs.log` and exits with the code
/// on the line of `codes.txt` matching its run number (0 past the end). A
/// run whose code is 75 also prints one error finding, which a pending
/// evaluation must log without counting.
const STATUS_SH: &str = r#"#!/bin/sh
n=$(cat runs.log 2>/dev/null | wc -l)
echo run >> runs.log
code=$(sed -n "$((n + 1))p" codes.txt)
code=${code:-0}
if [ "$code" = 75 ]; then
  echo '::koto-finding::{"rule_id":"CI-PENDING","level":"error","message":"checks still running"}'
fi
exit "$code"
"#;

/// A `ci` state gated on `./status.sh` with the given poll block; `again`
/// loops the state on itself while the gate is pending, and a passing gate
/// finishes.
fn template(poll: &str, extra_gate: &str) -> String {
    format!(
        r#"---
name: poll-gate
version: "1.0"
initial_state: ci
states:
  ci:
    accepts:
      again:
        type: enum
        values: ["yes"]
    gates:
      ci:
        type: command
        command: ./status.sh
{extra_gate}        poll:
{poll}
    transitions:
      - target: done
        when:
          gates.ci.exit_code: 0
      - target: ci
        when:
          gates.ci.exit_code: 75
          again: "yes"
  done:
    terminal: true
---

## ci

Wait for CI.

## done

Done.
"#
    )
}

fn setup(codes: &[i32], poll: &str) -> (TempDir, PathBuf) {
    setup_with(codes, poll, "")
}

fn setup_with(codes: &[i32], poll: &str, extra_gate: &str) -> (TempDir, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_path_buf();
    let script = dir.join("status.sh");
    std::fs::write(&script, STATUS_SH).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let list: Vec<String> = codes.iter().map(|c| c.to_string()).collect();
    std::fs::write(dir.join("codes.txt"), list.join("\n") + "\n").unwrap();
    let src = dir.join("wf.md");
    std::fs::write(&src, template(poll, extra_gate)).unwrap();
    run_ok(&dir, &["init", "wf", "--template", src.to_str().unwrap()]);
    (tmp, dir)
}

const NO_HOLD: &str = "          interval_secs: 1\n          timeout_secs: 600\n";

// ===== Tests =====

#[test]
fn a_pending_answer_is_a_temporal_wait_with_nothing_judged() {
    let (_tmp, dir) = setup(&[75], NO_HOLD);
    let resp = next(&dir, "wf", None);
    assert_eq!(resp["state"], "ci");
    let c = ci_condition(&resp);
    assert_eq!(c["status"], "pending", "{c}");
    assert_eq!(c["category"], "temporal", "{c}");
    assert_eq!(c["agent_actionable"], false, "{c}");
    assert!(
        c.get("failure").is_none(),
        "a pending gate has no failure: {c}"
    );
    assert_eq!(c["poll"]["status"], "pending");
    assert_eq!(c["poll"]["retry_after_secs"], 1);
    assert_eq!(c["poll"]["timeout_secs"], 600);

    let evals = ci_evals(&dir);
    assert_eq!(evals.len(), 1);
    let p = &evals[0]["payload"];
    assert_eq!(p["outcome"], "pending");
    assert!(p.get("attempt").is_none(), "{p}");
    assert!(p.get("visit_attempt").is_none(), "{p}");
    assert!(p.get("rule_counts").is_none(), "{p}");
    // The finding the script printed is logged; koto adds no fallback.
    let findings = p["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1, "{p}");
    assert_eq!(findings[0]["rule_id"], "CI-PENDING");
    assert_eq!(p["poll"]["status"], "pending");
    assert_eq!(p["poll"]["evaluations"], 1);
    assert!(p["poll"]["since"].as_str().unwrap().ends_with('Z'));
}

#[test]
fn a_hold_re_runs_until_done_and_logs_one_evaluation() {
    let poll = "          interval_secs: 1\n          timeout_secs: 60\n          hold_secs: 10\n";
    let (_tmp, dir) = setup(&[75, 75, 0], poll);
    let resp = next(&dir, "wf", None);
    assert_eq!(resp["state"], "done", "{resp}");
    assert_eq!(runs(&dir), 3);

    let evals = ci_evals(&dir);
    assert_eq!(evals.len(), 1, "one record per tick: {evals:?}");
    let p = &evals[0]["payload"];
    assert_eq!(p["outcome"], "passed");
    assert_eq!(p["poll"]["status"], "done");
    assert_eq!(p["poll"]["evaluations"], 3);
    assert_eq!(p["attempt"], 1);
}

#[test]
fn without_a_hold_each_tick_runs_once() {
    let (_tmp, dir) = setup(&[75, 75], NO_HOLD);
    next(&dir, "wf", None);
    assert_eq!(runs(&dir), 1);
    next(&dir, "wf", None);
    assert_eq!(runs(&dir), 2);
}

#[test]
fn a_failed_answer_is_a_corrective_command_gate_failure() {
    let (_tmp, dir) = setup(&[1], NO_HOLD);
    let resp = next(&dir, "wf", None);
    let c = ci_condition(&resp);
    assert_eq!(c["status"], "failed");
    assert_eq!(c["category"], "corrective");
    assert!(c["failure"].is_object(), "{c}");
    assert_eq!(c["poll"]["status"], "failed");
    assert!(c["poll"].get("retry_after_secs").is_none(), "{c}");
    let p = &ci_evals(&dir)[0]["payload"];
    assert_eq!(p["outcome"], "failed");
    assert_eq!(p["attempt"], 1);
}

#[test]
fn a_run_killed_by_its_own_timeout_is_failed_not_pending() {
    // The gate's per-run timeout (1 s) kills a command that sleeps longer.
    let (_tmp, dir) = setup_with(&[75], NO_HOLD, "        timeout: 1\n");
    std::fs::write(dir.join("status.sh"), "#!/bin/sh\nsleep 5\n").unwrap();
    let resp = next(&dir, "wf", None);
    let c = ci_condition(&resp);
    assert_eq!(c["status"], "timed_out");
    assert_eq!(c["category"], "corrective");
    assert_eq!(c["poll"]["status"], "failed");
}

#[test]
fn the_window_and_run_count_span_ticks_and_the_result_takes_one_attempt() {
    // A resolved failure first, then two pending ticks, then done.
    let (_tmp, dir) = setup(&[1, 75, 75, 0], NO_HOLD);
    for _ in 0..4 {
        next(&dir, "wf", None);
    }
    let evals = ci_evals(&dir);
    assert_eq!(evals.len(), 4);
    let outcomes: Vec<&str> = evals
        .iter()
        .map(|e| e["payload"]["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(outcomes, ["failed", "pending", "pending", "passed"]);
    let attempts: Vec<Option<u64>> = evals
        .iter()
        .map(|e| e["payload"]["attempt"].as_u64())
        .collect();
    assert_eq!(attempts, [Some(1), None, None, Some(2)]);
    let since = &evals[0]["payload"]["poll"]["since"];
    for (i, e) in evals.iter().enumerate() {
        assert_eq!(&e["payload"]["poll"]["since"], since, "eval {i}");
        assert_eq!(
            e["payload"]["poll"]["evaluations"],
            i as u64 + 1,
            "eval {i}"
        );
    }
}

#[test]
fn pending_past_the_deadline_times_out_with_koto_s_finding() {
    let poll = "          interval_secs: 1\n          timeout_secs: 1\n";
    let (_tmp, dir) = setup(&[75, 75], poll);
    next(&dir, "wf", None);
    std::thread::sleep(std::time::Duration::from_millis(1300));
    let resp = next(&dir, "wf", None);
    let c = ci_condition(&resp);
    assert_eq!(c["status"], "timed_out", "{c}");
    assert_eq!(c["category"], "corrective");
    assert_eq!(c["poll"]["status"], "timed_out");
    // The script's own error finding leads; the koto sentence replaces the
    // fallback when there is one.
    let p = &ci_evals(&dir)[1]["payload"];
    assert_eq!(p["outcome"], "timed_out");
    assert_eq!(p["poll"]["status"], "timed_out");
    assert!(p["attempt"].is_u64(), "a timeout resolves the poll: {p}");
}

#[test]
fn pending_past_the_deadline_without_a_finding_says_it_was_still_pending() {
    let poll = "          interval_secs: 1\n          timeout_secs: 1\n";
    let (_tmp, dir) = setup(&[75, 75], poll);
    // A script that prints nothing, so koto writes the fallback finding.
    std::fs::write(
        dir.join("status.sh"),
        "#!/bin/sh\necho run >> runs.log\nexit 75\n",
    )
    .unwrap();
    next(&dir, "wf", None);
    std::thread::sleep(std::time::Duration::from_millis(1300));
    let resp = next(&dir, "wf", None);
    let c = ci_condition(&resp);
    let messages: Vec<&str> = c["failure"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["message"].as_str().unwrap())
        .collect();
    assert!(
        messages.contains(&"command still pending after 1 seconds"),
        "{messages:?}"
    );
}

#[test]
fn done_after_the_deadline_is_taken_as_done() {
    let poll = "          interval_secs: 1\n          timeout_secs: 1\n";
    let (_tmp, dir) = setup(&[75, 0], poll);
    next(&dir, "wf", None);
    std::thread::sleep(std::time::Duration::from_millis(1300));
    let resp = next(&dir, "wf", None);
    assert_eq!(resp["state"], "done", "{resp}");
}

#[test]
fn a_new_entry_opens_a_new_window() {
    let (_tmp, dir) = setup(&[75, 75, 75], NO_HOLD);
    next(&dir, "wf", None);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    // The evidence tick evaluates the gate once more in the old epoch, takes
    // the self-transition, and evaluates it again in the new one.
    next(&dir, "wf", Some(r#"{"again":"yes"}"#));
    let evals = ci_evals(&dir);
    assert_eq!(evals.len(), 3);
    let (first, old, new) = (
        &evals[0]["payload"]["poll"],
        &evals[1]["payload"]["poll"],
        &evals[2]["payload"]["poll"],
    );
    assert_eq!(first["since"], old["since"]);
    assert_eq!(old["evaluations"], 2);
    assert_ne!(
        first["since"], new["since"],
        "a self-transition is a new entry"
    );
    assert_eq!(new["evaluations"], 1);
}

#[test]
fn overriding_a_pending_gate_passes_without_running_the_command() {
    let (_tmp, dir) = setup(&[75, 75], NO_HOLD);
    next(&dir, "wf", None);
    assert_eq!(runs(&dir), 1);
    run_ok(
        &dir,
        &[
            "overrides",
            "record",
            "wf",
            "--gate",
            "ci",
            "--rationale",
            "checked by hand",
        ],
    );
    let resp = next(&dir, "wf", None);
    assert_eq!(resp["state"], "done", "{resp}");
    assert_eq!(runs(&dir), 1, "the command must not run under an override");
    assert_eq!(ci_evals(&dir).len(), 1);
}

#[test]
fn a_non_overridable_polling_gate_refuses_an_override() {
    let (_tmp, dir) = setup_with(&[75], NO_HOLD, "        overridable: false\n");
    next(&dir, "wf", None);
    let out = run(
        &dir,
        &[
            "overrides",
            "record",
            "wf",
            "--gate",
            "ci",
            "--rationale",
            "checked by hand",
        ],
    );
    assert!(!out.status.success());
}

#[test]
fn a_hold_starts_no_run_past_hold_secs() {
    // Always pending; a 2-second hold at a 1-second interval allows at most
    // the first run and two more.
    let poll = "          interval_secs: 1\n          timeout_secs: 600\n          hold_secs: 2\n";
    let (_tmp, dir) = setup(&[75, 75, 75, 75, 75, 75], poll);
    let started = std::time::Instant::now();
    let resp = next(&dir, "wf", None);
    let took = started.elapsed();
    assert_eq!(ci_condition(&resp)["status"], "pending");
    let n = runs(&dir);
    assert!((2..=3).contains(&n), "runs: {n}");
    assert!(took < std::time::Duration::from_secs(4), "held {took:?}");
    let p = &ci_evals(&dir)[0]["payload"]["poll"];
    assert_eq!(p["evaluations"], n as u64);
}

#[test]
fn a_hold_starts_no_run_past_the_deadline() {
    // A long hold but a 2-second window: no run may start at or past it.
    let poll = "          interval_secs: 1\n          timeout_secs: 2\n          hold_secs: 2\n";
    let (_tmp, dir) = setup(&[75, 75, 75, 75, 75, 75], poll);
    let started = std::time::Instant::now();
    next(&dir, "wf", None);
    let took = started.elapsed();
    assert!(runs(&dir) <= 2, "runs: {}", runs(&dir));
    assert!(took < std::time::Duration::from_secs(3), "held {took:?}");
}

#[test]
fn a_signal_ends_the_hold() {
    let poll = "          interval_secs: 1\n          timeout_secs: 600\n          hold_secs: 60\n";
    let (_tmp, dir) = setup(&[75; 100], poll);
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_koto"))
        .args(["next", "wf", "--no-cleanup"])
        .current_dir(&dir)
        .env("KOTO_SESSIONS_BASE", sessions_base(&dir))
        .env("HOME", &dir)
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("KOTO_WORKFLOWS_DIR")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let started = std::time::Instant::now();
    let status = std::process::Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "koto next kept holding after SIGTERM"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[test]
fn a_custom_pending_code_is_honoured() {
    let poll =
        "          interval_secs: 1\n          timeout_secs: 600\n          pending_exit_code: 8\n";
    let (_tmp, dir) = setup(&[8, 75], poll);
    let c = ci_condition(&next(&dir, "wf", None));
    assert_eq!(c["status"], "pending");
    // 75 is an ordinary failure once another code means pending.
    let c = ci_condition(&next(&dir, "wf", None));
    assert_eq!(c["status"], "failed");
    assert_eq!(c["poll"]["status"], "failed");
}

#[test]
fn each_polling_gate_keeps_its_own_interval() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_path_buf();
    for (name, log) in [("fast.sh", "fast.log"), ("slow.sh", "slow.log")] {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\necho run >> {log}\nexit 75\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let src = dir.join("wf.md");
    std::fs::write(
        &src,
        r#"---
name: two-polls
version: "1.0"
initial_state: ci
states:
  ci:
    gates:
      fast:
        type: command
        command: ./fast.sh
        poll:
          interval_secs: 1
          timeout_secs: 600
          hold_secs: 3
      slow:
        type: command
        command: ./slow.sh
        poll:
          interval_secs: 3
          timeout_secs: 600
          hold_secs: 3
    transitions:
      - target: done
        when:
          gates.fast.exit_code: 0
          gates.slow.exit_code: 0
  done:
    terminal: true
---

## ci

Wait.

## done

Done.
"#,
    )
    .unwrap();
    run_ok(&dir, &["init", "wf", "--template", src.to_str().unwrap()]);
    next(&dir, "wf", None);
    let count = |log: &str| {
        std::fs::read_to_string(dir.join(log))
            .map(|s| s.lines().count())
            .unwrap_or(0)
    };
    let (fast, slow) = (count("fast.log"), count("slow.log"));
    assert!(fast >= 3, "the 1-second gate ran {fast} times");
    assert_eq!(slow, 2, "the 3-second gate ran {slow} times, fast {fast}");
}
