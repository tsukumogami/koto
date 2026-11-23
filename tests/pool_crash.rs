//! Crashes, cancels and moves of background pooled runs
//! (DESIGN-host-resource-pools.md, PLAN step 4).
//!
//! A run's slot frees only when every process of it has gone, and koto
//! signals nothing but a live runner it has verified: a killed tree frees
//! its slot, an orphaned command keeps its slot and is never signalled,
//! `koto cancel` and removal stop a runner and leave the queue, re-entry
//! stops the previous visit's runner, the operator's `[background]` switch
//! and cap apply, and a session can't move while its run is in flight.

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
    fn new(config: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("koto-root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("config.toml"), config).unwrap();
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
            .env_remove("KOTO_MACHINE_NAME")
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

    /// Run koto with `args` and `env`, returning its exit code and the JSON
    /// on the last line of its stdout.
    fn run_env(
        &self,
        ws: &Path,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> (Option<i32>, serde_json::Value) {
        let mut cmd = self.koto(ws);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.args(args).output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let json = stdout
            .lines()
            .rfind(|l| !l.trim().is_empty())
            .and_then(|l| serde_json::from_str(l).ok())
            .unwrap_or_else(|| {
                serde_json::json!({
                    "stdout": stdout,
                    "stderr": String::from_utf8_lossy(&out.stderr).to_string(),
                })
            });
        (out.status.code(), json)
    }

    fn run(&self, ws: &Path, args: &[&str]) -> (Option<i32>, serde_json::Value) {
        self.run_env(ws, args, &[])
    }

    /// One tick, kept on a terminal state so its log stays readable.
    fn next_env(&self, ws: &Path, name: &str, env: &[(&str, &str)]) -> serde_json::Value {
        let (code, json) = self.run_env(ws, &["next", name, "--no-cleanup"], env);
        assert_eq!(code, Some(0), "next should exit 0: {json}");
        json
    }

    fn next(&self, ws: &Path, name: &str) -> serde_json::Value {
        self.next_env(ws, name, &[])
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

    /// The session's run directories, oldest first by start time.
    fn runs(&self, ws: &Path, name: &str) -> Vec<PathBuf> {
        let file = format!("koto-{name}.state.jsonl");
        let dir = find(&ws.join("sessions"), &file)
            .expect("the session log")
            .parent()
            .unwrap()
            .join("runs");
        let mut runs: Vec<(u64, PathBuf)> = std::fs::read_dir(dir)
            .map(|d| {
                d.flatten()
                    .map(|e| {
                        let started = run_record(&e.path())["started_at"].as_u64().unwrap_or(0);
                        (started, e.path())
                    })
                    .collect()
            })
            .unwrap_or_default();
        runs.sort();
        runs.into_iter().map(|(_, p)| p).collect()
    }

    /// The sessions in the pool's queue file, in order.
    fn queue(&self) -> Vec<String> {
        let path = self.root.join("pools/test-suite/queue.json");
        let Ok(text) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        let q: serde_json::Value = serde_json::from_str(&text).unwrap();
        q["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["session"].as_str().unwrap().to_string())
            .collect()
    }

    /// Whether the pool's slot 0 is held, by a non-blocking try that lets
    /// go at once.
    fn slot_held(&self) -> bool {
        held(&self.root.join("pools/test-suite/slot-0.lock"))
    }
}

/// Whether another process holds an flock on `path`.
fn held(path: &Path) -> bool {
    use std::os::fd::AsRawFd;
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    // SAFETY: flock on a descriptor this test owns; released on drop.
    let got = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    !got
}

fn run_record(run_dir: &Path) -> serde_json::Value {
    std::fs::read_to_string(run_dir.join("run.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(serde_json::Value::Null)
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

fn reasons(events: &[serde_json::Value]) -> Vec<String> {
    of_type(events, "pool_released")
        .iter()
        .map(|e| e["payload"]["reason"].as_str().unwrap().to_string())
        .collect()
}

/// A `test` state whose background action runs `command`, then `done`.
fn template(command: &str, action_extra: &str) -> String {
    format!(
        r#"---
name: pool-crash
version: "1.0"
initial_state: test
states:
  test:
    default_action:
      command: {command:?}
      background: true
{action_extra}    transitions:
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

/// `start` leads to the pooled background `test`, which completes on
/// `finish`; a rewind from `test` goes back to `start`.
fn leaving_template(command: &str) -> String {
    format!(
        r#"---
name: pool-crash-leave
version: "1.0"
initial_state: start
states:
  start:
    transitions:
      - target: test
  test:
    accepts:
      verdict:
        type: enum
        required: true
        values: [finish]
    default_action:
      command: {command:?}
      background: true
      pool: test-suite
      timeout_secs: 120
    transitions:
      - target: done
        when:
          verdict: finish
  done:
    terminal: true
---

## start

Start.

## test

Run the suite.

## done

Done.
"#
    )
}

const SIZE_ONE: &str = "[pools.test-suite]\nsize = 1\nwait_secs = 0\n";
const POOLED: &str = "      pool: test-suite\n      timeout_secs: 120\n";

/// The one `__action__` condition of a blocked response.
fn action_condition(resp: &serde_json::Value) -> &serde_json::Value {
    assert_eq!(resp["action"], "gate_blocked", "got {resp}");
    let conditions = resp["blocking_conditions"].as_array().unwrap();
    assert_eq!(conditions.len(), 1, "got {conditions:?}");
    assert_eq!(conditions[0]["name"], "__action__", "got {resp}");
    &conditions[0]
}

/// The `__action__` condition's output, checked to be `status`.
fn in_flight<'a>(resp: &'a serde_json::Value, status: &str) -> &'a serde_json::Value {
    let c = action_condition(resp);
    assert_eq!(c["output"]["status"], status, "got {c}");
    assert_eq!(c["category"], "temporal", "got {c}");
    &c["output"]
}

fn queued(resp: &serde_json::Value) {
    let c = &resp["blocking_conditions"][0];
    assert_eq!(c["name"], "__pool__", "got {resp}");
    assert_eq!(c["status"], "queued", "got {resp}");
}

fn wait_for(limit: Duration, what: &str, mut done: impl FnMut() -> bool) -> Duration {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < limit, "{what} not within {limit:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
    started.elapsed()
}

fn read_pid(path: &Path) -> i32 {
    wait_for(Duration::from_secs(10), "the pid file", || {
        std::fs::read_to_string(path)
            .map(|t| t.trim().parse::<i32>().is_ok())
            .unwrap_or(false)
    });
    std::fs::read_to_string(path)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

/// Whether `pid` is a live process (not a zombie).
fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only asks whether the process exists.
    if unsafe { libc::kill(pid, 0) } != 0 {
        return false;
    }
    #[cfg(target_os = "linux")]
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        if let Some(rest) = stat.rfind(')').map(|i| &stat[i + 1..]) {
            return rest.split_whitespace().next() != Some("Z");
        }
    }
    true
}

fn kill9(pid: i32) {
    // SAFETY: SIGKILL to a process this test started through koto.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
}

fn runner_pid(run_dir: &Path) -> i32 {
    run_record(run_dir)["runner_pid"]
        .as_i64()
        .expect("runner pid") as i32
}

// ---------------------------------------------------------------------------
// kill -9
// ---------------------------------------------------------------------------

#[test]
fn killing_the_whole_run_frees_the_slot_and_the_head_waiter_takes_it() {
    let host = Host::new(SIZE_ONE);
    let ws_a = host.workspace("ws-a");
    let ws_b = host.workspace("ws-b");
    let pid_file = ws_a.join("pid");
    host.init(
        &ws_a,
        "a",
        &template(
            &format!("echo $$ > {}; sleep 60", pid_file.display()),
            POOLED,
        ),
    );
    host.init(&ws_b, "b", &template("sleep 1", POOLED));

    in_flight(&host.next(&ws_a, "a"), "running");
    queued(&host.next(&ws_b, "b"));
    let group = read_pid(&pid_file);
    let runner = runner_pid(&host.runs(&ws_a, "a")[0]);
    assert!(host.slot_held());

    // SAFETY: SIGKILL to the command's group and to its runner.
    unsafe {
        libc::kill(-group, libc::SIGKILL);
    }
    kill9(runner);
    let freed = wait_for(Duration::from_secs(5), "the slot to free", || {
        !host.slot_held()
    });
    println!("slot free {freed:?} after kill -9");

    // The head waiter's next tick takes it.
    in_flight(&host.next(&ws_b, "b"), "running");
    assert_eq!(of_type(&host.events(&ws_b, "b"), "pool_acquired").len(), 1);

    // The killed session's next tick reports the run lost.
    let lost = host.next(&ws_a, "a");
    let c = action_condition(&lost);
    assert_eq!(c["output"]["failure_kind"], "lost", "got {c}");
    assert!(c["output"].get("exit_code").is_none(), "got {c}");
    assert_eq!(reasons(&host.events(&ws_a, "a")), ["lost"]);
}

#[test]
fn an_orphaned_run_keeps_its_slot_and_its_survivor_is_never_signalled() {
    let host = Host::new(SIZE_ONE);
    let ws = host.workspace("ws-o");
    let pid_file = ws.join("pid");
    let signalled = ws.join("signalled");
    let release = ws.join("release");
    // The survivor notes any TERM or HUP it gets, and exits once the test
    // lets it.
    let command = format!(
        "trap 'echo TERM >> {sig}' TERM; trap 'echo HUP >> {sig}' HUP; echo $$ > {pid}; \
         while [ ! -e {release} ]; do sleep 0.2; done",
        sig = signalled.display(),
        pid = pid_file.display(),
        release = release.display(),
    );
    host.init(&ws, "o", &template(&command, POOLED));

    in_flight(&host.next(&ws, "o"), "running");
    let survivor = read_pid(&pid_file);
    let run_dir = host.runs(&ws, "o")[0].clone();
    let runner = runner_pid(&run_dir);
    kill9(runner);
    wait_for(Duration::from_secs(5), "the runner to die", || {
        !alive(runner)
    });

    // Orphaned while the command lives: the slot stays held, nothing is
    // signalled, and no second run starts.
    for _ in 0..3 {
        let out = in_flight(&host.next(&ws, "o"), "orphaned").clone();
        assert_eq!(out["runner_pid"], runner, "got {out}");
        assert!(alive(survivor), "the survivor is gone");
        assert!(host.slot_held(), "the slot was freed under a survivor");
        std::thread::sleep(Duration::from_millis(500));
    }
    assert_eq!(host.runs(&ws, "o").len(), 1);
    println!("survivor {survivor} alive after the runner {runner} died");

    // Once it exits, the next tick reports the run lost and releases it.
    std::fs::write(&release, "").unwrap();
    wait_for(Duration::from_secs(5), "the survivor to exit", || {
        !alive(survivor)
    });
    wait_for(Duration::from_secs(5), "the slot to free", || {
        !host.slot_held()
    });
    let lost = host.next(&ws, "o");
    let c = action_condition(&lost);
    assert_eq!(c["output"]["failure_kind"], "lost", "got {c}");
    assert_eq!(reasons(&host.events(&ws, "o")), ["lost"]);
    assert!(
        !signalled.exists(),
        "the survivor was signalled: {}",
        std::fs::read_to_string(&signalled).unwrap_or_default()
    );

    // And the tick after starts a new run.
    let again = host.next(&ws, "o");
    let out = in_flight(&again, "running");
    assert_ne!(
        out["run_id"].as_str().unwrap(),
        run_dir.file_name().unwrap().to_str().unwrap()
    );
    assert_eq!(host.runs(&ws, "o").len(), 2);
    let _ = host.run(&ws, &["cancel", "o"]);
}

#[cfg(target_os = "linux")]
#[test]
fn a_descendant_that_left_with_setsid_is_stopped_when_the_runner_ends() {
    let host = Host::new(SIZE_ONE);
    let ws = host.workspace("ws-s");
    let escaped = ws.join("escaped");
    // The subshell exits at once, so the setsid process has no parent left
    // in the run: without the runner as subreaper it would belong to init.
    let command = format!(
        "( setsid sh -c 'echo $$ > {esc}; exec sleep 60' > /dev/null 2>&1 & ); \
         while [ ! -s {esc} ]; do sleep 0.1; done; exit 0",
        esc = escaped.display(),
    );
    host.init(&ws, "s", &template(&command, POOLED));

    in_flight(&host.next(&ws, "s"), "running");
    let pid = read_pid(&escaped);
    let run_dir = host.runs(&ws, "s")[0].clone();
    wait_for(Duration::from_secs(10), "the result", || {
        run_dir.join("result.json").exists()
    });
    wait_for(Duration::from_secs(5), "the runner to end", || {
        !held(&run_dir.join("runner.lock"))
    });
    assert!(!alive(pid), "the setsid process {pid} outlived the runner");
    assert!(
        !held(&run_dir.join("run.lock")),
        "a process of the run lives on"
    );
    assert!(!host.slot_held(), "the slot is still held");

    let done = host.next(&ws, "s");
    assert_eq!(done["state"], "done", "got {done}");
    assert_eq!(reasons(&host.events(&ws, "s")), ["done"]);
}

// ---------------------------------------------------------------------------
// cancel, removal, re-entry
// ---------------------------------------------------------------------------

#[test]
fn cancel_stops_a_holder_and_cancel_or_removal_takes_a_waiter_out_of_the_queue() {
    let host = Host::new(SIZE_ONE);
    let ws_a = host.workspace("ws-a");
    let ws_b = host.workspace("ws-b");
    let ws_c = host.workspace("ws-c");
    let pid_file = ws_a.join("pid");
    let child_file = ws_a.join("child");
    host.init(
        &ws_a,
        "a",
        &template(
            &format!(
                "echo $$ > {}; sleep 60 & echo $! > {}; wait",
                pid_file.display(),
                child_file.display()
            ),
            POOLED,
        ),
    );
    host.init(&ws_b, "b", &template("sleep 1", POOLED));
    host.init(&ws_c, "c", &template("sleep 1", POOLED));

    in_flight(&host.next(&ws_a, "a"), "running");
    queued(&host.next(&ws_b, "b"));
    queued(&host.next(&ws_c, "c"));
    assert_eq!(host.queue(), ["b", "c"]);
    let shell = read_pid(&pid_file);
    let child = read_pid(&child_file);

    let started = Instant::now();
    let (code, out) = host.run(&ws_a, &["cancel", "a"]);
    let took = started.elapsed();
    assert_eq!(code, Some(0), "cancel failed: {out}");
    println!("cancel of the holder took {took:?}");
    assert!(took < Duration::from_secs(5), "{took:?}");
    assert!(
        !alive(shell) && !alive(child),
        "the command's processes live on"
    );
    assert!(!host.slot_held(), "the slot is still held after cancel");
    let events = host.events(&ws_a, "a");
    assert_eq!(reasons(&events), ["cancelled"]);
    let finished = of_type(&events, "background_finished");
    assert_eq!(finished[0]["payload"]["outcome"], "cancelled");
    let order: Vec<&str> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();
    let released_at = order.iter().position(|t| *t == "pool_released").unwrap();
    let cancelled_at = order
        .iter()
        .position(|t| *t == "workflow_cancelled")
        .unwrap();
    assert!(released_at < cancelled_at, "{order:?}");

    // A waiter's cancel leaves the queue before it returns.
    let (code, out) = host.run(&ws_b, &["cancel", "b"]);
    assert_eq!(code, Some(0), "cancel failed: {out}");
    assert_eq!(host.queue(), ["c"]);
    assert_eq!(reasons(&host.events(&ws_b, "b")), ["cancelled"]);

    // Removing a waiting session does too.
    let (code, out) = host.run(&ws_c, &["session", "cleanup", "c"]);
    assert_eq!(code, Some(0), "cleanup failed: {out}");
    assert!(host.queue().is_empty(), "still queued: {:?}", host.queue());
}

#[test]
fn removing_a_holder_stops_its_runner() {
    let host = Host::new(SIZE_ONE);
    let ws = host.workspace("ws-r");
    let pid_file = ws.join("pid");
    host.init(
        &ws,
        "r",
        &template(
            &format!("echo $$ > {}; sleep 60", pid_file.display()),
            POOLED,
        ),
    );
    in_flight(&host.next(&ws, "r"), "running");
    let shell = read_pid(&pid_file);
    let (code, out) = host.run(&ws, &["session", "cleanup", "r"]);
    assert_eq!(code, Some(0), "cleanup failed: {out}");
    assert!(!alive(shell), "the command outlived its session");
    assert!(!host.slot_held());
}

#[test]
fn re_entering_the_state_stops_the_previous_visits_runner_first() {
    let host = Host::new(SIZE_ONE);
    let ws = host.workspace("ws-e");
    let pid_file = ws.join("pid");
    host.init(
        &ws,
        "e",
        &leaving_template(&format!("echo $$ >> {}; sleep 60", pid_file.display())),
    );
    let first = host.next(&ws, "e");
    assert_eq!(first["state"], "test", "got {first}");
    in_flight(&first, "running");
    let old = read_pid(&pid_file);

    let (code, out) = host.run(&ws, &["rewind", "e"]);
    assert_eq!(code, Some(0), "rewind failed: {out}");
    let again = host.next(&ws, "e");
    assert_eq!(again["state"], "test", "got {again}");
    in_flight(&again, "running");
    assert!(!alive(old), "the previous visit's command {old} still runs");
    assert_eq!(host.runs(&ws, "e").len(), 2);

    let events = host.events(&ws, "e");
    let finished = of_type(&events, "background_finished");
    assert_eq!(finished.len(), 1, "got {finished:?}");
    assert_eq!(finished[0]["payload"]["outcome"], "cancelled");
    // The old run was settled before the new one started.
    let order: Vec<(&str, &serde_json::Value)> = events
        .iter()
        .map(|e| (e["type"].as_str().unwrap(), &e["payload"]))
        .collect();
    let settled_at = order
        .iter()
        .position(|(t, _)| *t == "background_finished")
        .unwrap();
    let second_start = order
        .iter()
        .enumerate()
        .filter(|(_, (t, _))| *t == "background_started")
        .nth(1)
        .unwrap()
        .0;
    assert!(settled_at < second_start, "{order:?}");
    let _ = host.run(&ws, &["cancel", "e"]);
}

// ---------------------------------------------------------------------------
// the operator's [background] settings, output bound
// ---------------------------------------------------------------------------

#[test]
fn a_disabled_machine_refuses_background_actions() {
    let host = Host::new(&format!("{SIZE_ONE}[background]\nenabled = false\n"));
    let ws = host.workspace("ws-d");
    host.init(&ws, "d", &template("echo ran", POOLED));
    let resp = host.next(&ws, "d");
    let c = action_condition(&resp);
    assert_eq!(c["output"]["error_code"], "background_disabled", "got {c}");
    assert!(
        !host.root.join("pools").exists(),
        "the disabled tick queued"
    );
    assert!(host.runs(&ws, "d").is_empty());
}

#[test]
fn a_timeout_above_the_cap_is_cut_to_it() {
    let host = Host::new(&format!("{SIZE_ONE}[background]\nmax_timeout_secs = 10\n"));
    let ws = host.workspace("ws-t");
    let pid_file = ws.join("pid");
    host.init(
        &ws,
        "t",
        &template(
            &format!("echo $$ > {}; exec sleep 60", pid_file.display()),
            "      pool: test-suite\n      timeout_secs: 60\n",
        ),
    );
    let started = Instant::now();
    let out = in_flight(&host.next(&ws, "t"), "running").clone();
    assert_eq!(out["timeout_capped"], true, "got {out}");
    assert_eq!(out["timeout_secs"], 10, "got {out}");
    let pid = read_pid(&pid_file);
    let run_dir = host.runs(&ws, "t")[0].clone();
    wait_for(Duration::from_secs(20), "the result", || {
        run_dir.join("result.json").exists()
    });
    let took = started.elapsed();
    println!("the capped run ended after {took:?}");
    assert!(took >= Duration::from_secs(10), "{took:?}");
    assert!(took < Duration::from_secs(14), "{took:?}");
    assert!(!alive(pid));
    let resp = host.next(&ws, "t");
    let c = action_condition(&resp);
    assert_eq!(c["output"]["failure_kind"], "timed_out", "got {c}");
}

#[test]
fn a_hundred_kib_of_output_is_cut_to_its_first_64_kib() {
    let host = Host::new(SIZE_ONE);
    let ws = host.workspace("ws-big");
    host.init(
        &ws,
        "big",
        // Every byte that isn't `a` becomes one: 100 KiB of `a`.
        &template("head -c 102400 /dev/zero | tr -c a a; exit 1", POOLED),
    );
    in_flight(&host.next(&ws, "big"), "running");
    let run_dir = host.runs(&ws, "big")[0].clone();
    wait_for(Duration::from_secs(20), "the result", || {
        run_dir.join("result.json").exists()
    });
    let resp = host.next(&ws, "big");
    let c = action_condition(&resp);
    assert_eq!(c["output"]["failure_kind"], "nonzero_exit", "got {c}");
    assert_eq!(c["output"]["exit_code"], 1);
    assert_eq!(c["output"]["truncated"], true, "got {}", c["output"]);
    let stdout = c["output"]["stdout"].as_str().unwrap();
    let body: String = stdout.chars().take_while(|ch| *ch == 'a').collect();
    assert_eq!(body.len(), 64 * 1024, "kept {} bytes", body.len());
}

// ---------------------------------------------------------------------------
// moves
// ---------------------------------------------------------------------------

#[test]
fn rebind_refuses_while_a_run_is_in_flight() {
    let host = Host::new(SIZE_ONE);
    let ws = host.workspace("ws-m");
    let elsewhere = host.workspace("ws-elsewhere");
    host.init(&ws, "m", &template("sleep 60", POOLED));
    let out = in_flight(&host.next(&ws, "m"), "running").clone();
    let run_id = out["run_id"].as_str().unwrap();

    let (code, json) = host.run(
        &ws,
        &[
            "session",
            "rebind",
            "m",
            "--to",
            elsewhere.to_str().unwrap(),
        ],
    );
    assert_eq!(code, Some(2), "got {json}");
    assert_eq!(
        json["error"]["code"], "background_run_in_flight",
        "got {json}"
    );
    assert_eq!(json["error"]["run_id"], run_id);

    let _ = host.run(&ws, &["cancel", "m"]);
    let (code, json) = host.run(
        &ws,
        &[
            "session",
            "rebind",
            "m",
            "--to",
            elsewhere.to_str().unwrap(),
        ],
    );
    assert_eq!(code, Some(0), "rebind after the run ended: {json}");
}

#[test]
fn a_run_started_on_another_machine_reads_as_lost() {
    let host = Host::new(SIZE_ONE);
    let ws = host.workspace("ws-f");
    let pid_file = ws.join("pid");
    host.init(
        &ws,
        "f",
        &template(
            &format!("echo $$ > {}; sleep 30", pid_file.display()),
            POOLED,
        ),
    );
    let alpha = [("KOTO_MACHINE_NAME", "alpha")];
    let beta = [("KOTO_MACHINE_NAME", "beta")];
    in_flight(&host.next_env(&ws, "f", &alpha), "running");
    let shell = read_pid(&pid_file);
    let events = host.events(&ws, "f");
    let started = &of_type(&events, "background_started")[0];
    assert_eq!(started["payload"]["machine"], "alpha");

    let resp = host.next_env(&ws, "f", &beta);
    let c = action_condition(&resp);
    assert_eq!(c["output"]["failure_kind"], "lost", "got {c}");
    assert_eq!(reasons(&host.events(&ws, "f")), ["lost"]);
    // Read as lost, never acted on: the other machine's command runs on.
    assert!(alive(shell), "a foreign run was signalled");
    // SAFETY: SIGKILL to the group this test's command leads.
    unsafe {
        libc::kill(-shell, libc::SIGKILL);
    }
}
