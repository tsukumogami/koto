//! `koto pool status`, the nesting marker, and the pool rules end to end
//! (DESIGN-host-resource-pools.md, "Nesting" and "Interfaces"; PLAN step 5).
//!
//! An operator sees who holds and who waits on each pool without touching
//! anything; a pooled command that runs koto again runs on the slot it
//! already holds; and the config, directory and event rules the earlier
//! steps built hold together when real `koto` processes run them.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use assert_cmd::Command;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

/// A koto root shared by several workspaces, one per session.
struct Host {
    tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Host {
    fn new(config: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("koto-root");
        std::fs::create_dir_all(&root).unwrap();
        let host = Host { tmp, root };
        host.write_config(config);
        host
    }

    fn base(&self) -> &Path {
        self.tmp.path()
    }

    fn write_config(&self, text: &str) {
        std::fs::write(self.root.join("config.toml"), text).unwrap();
    }

    /// The workspace of session `name`, `ws-<name>`.
    fn ws(&self, name: &str) -> PathBuf {
        let dir = self.base().join(format!("ws-{name}"));
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        dir
    }

    fn env(&self, cmd: &mut std::process::Command, ws: &Path, root: &Path) {
        cmd.env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("KOTO_TICK_SESSION")
            .env_remove("KOTO_BACKGROUND_OF")
            .env_remove("KOTO_POOL_HELD")
            .env_remove("KOTO_POOL_ROOT")
            .current_dir(ws)
            .env("HOME", self.base())
            .env("KOTO_HOME", root)
            .env("KOTO_SESSIONS_BASE", ws.join("sessions"));
    }

    /// koto run in session `name`'s workspace.
    fn koto(&self, name: &str) -> Command {
        let mut cmd = koto_command::koto_std();
        self.env(&mut cmd, &self.ws(name), &self.root);
        Command::from_std(cmd)
    }

    /// Initialize session `name` in its own workspace.
    fn init(&self, name: &str, template: &str) {
        self.init_in(name, &self.ws(name), &self.root.clone(), template);
    }

    /// Initialize session `name` in workspace `ws` under koto root `root`.
    fn init_in(&self, name: &str, ws: &Path, root: &Path, template: &str) {
        let src = ws.join(format!("{name}.md"));
        std::fs::write(&src, template).unwrap();
        let mut cmd = koto_command::koto_std();
        self.env(&mut cmd, ws, root);
        let out = cmd
            .args(["init", name, "--template", src.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "init failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn run(&self, name: &str, args: &[&str]) -> serde_json::Value {
        let out = self.koto(name).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "{args:?} should exit 0: stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).expect("koto prints JSON")
    }

    fn next(&self, name: &str) -> serde_json::Value {
        self.run(name, &["next", name, "--no-cleanup"])
    }

    /// Start `koto next` without waiting for it, in its own process group.
    fn spawn_next(&self, name: &str) -> std::process::Child {
        let mut cmd = koto_command::koto_std();
        self.env(&mut cmd, &self.ws(name), &self.root);
        cmd.args(["next", name, "--no-cleanup"])
            .process_group(0)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    }

    /// `koto pool status` with `args`, run from a directory holding no
    /// session: its exit code and stdout.
    fn pool_status(&self, args: &[&str]) -> (i32, String) {
        let out = self
            .koto("status-reader")
            .args(["pool", "status"])
            .args(args)
            .output()
            .unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    }

    fn pool_status_json(&self) -> (i32, serde_json::Value) {
        let (code, out) = self.pool_status(&["--json"]);
        (
            code,
            serde_json::from_str(&out).expect("pool status prints JSON"),
        )
    }

    /// Session `name`'s log file, found in workspace `ws`.
    fn log_in(&self, ws: &Path, name: &str) -> PathBuf {
        let file = format!("koto-{name}.state.jsonl");
        find(&ws.join("sessions"), &file).expect("the session log")
    }

    /// Session `name`'s log in workspace `ws`, header left out.
    fn events_in(&self, ws: &Path, name: &str) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.log_in(ws, name))
            .unwrap()
            .lines()
            .skip(1)
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn events(&self, name: &str) -> Vec<serde_json::Value> {
        self.events_in(&self.ws(name), name)
    }

    fn flag(&self, name: &str) -> PathBuf {
        self.base().join(name)
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

fn wait_for(path: &Path) {
    let started = Instant::now();
    while !path.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "{} never appeared",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A template whose `test` state runs `command` as its default action, with
/// `extra` lines under it (four-space indent plus two), then goes to `done`.
fn template(command: &str, extra: &str) -> String {
    format!(
        r#"---
name: pool-status
version: "1.0"
initial_state: test
states:
  test:
    default_action:
      command: {command:?}
{extra}    transitions:
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

/// `template` with the action on pool `test-suite`.
fn pooled(command: &str) -> String {
    template(command, "      pool: test-suite\n")
}

/// A template that first waits in `prep` until `go` exists, then runs
/// `command` on pool `test-suite` in `test`.
fn gated(go: &Path, command: &str) -> String {
    format!(
        r#"---
name: pool-status-gated
version: "1.0"
initial_state: prep
states:
  prep:
    gates:
      go:
        type: command
        command: "test -f {go}"
    transitions:
      - target: test
  test:
    default_action:
      command: {command:?}
      pool: test-suite
    transitions:
      - target: done
  done:
    terminal: true
---

## prep

Get ready.

## test

Run the suite.

## done

Done.
"#,
        go = go.display()
    )
}

/// A command that waits for `release`, after writing `started`.
fn held(started: &Path, release: &Path) -> String {
    format!(
        "touch {} && while [ ! -f {} ]; do sleep 0.05; done",
        started.display(),
        release.display()
    )
}

/// A session holding a slot of `test-suite` until its release file appears.
struct Holder {
    child: std::process::Child,
    release: PathBuf,
}

impl Holder {
    fn start(host: &Host, name: &str) -> Holder {
        let started = host.flag(&format!("started-{name}"));
        let release = host.flag(&format!("release-{name}"));
        host.init(name, &pooled(&held(&started, &release)));
        let child = host.spawn_next(name);
        wait_for(&started);
        Holder { child, release }
    }

    fn release(self) {
        std::fs::write(&self.release, "").unwrap();
        let out = self.child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "holder tick failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// The one `__pool__` condition of a blocked response.
fn pool_condition(resp: &serde_json::Value) -> &serde_json::Value {
    assert_eq!(resp["action"], "gate_blocked", "got {resp}");
    let conditions = resp["blocking_conditions"].as_array().unwrap();
    assert_eq!(conditions.len(), 1, "got {conditions:?}");
    assert_eq!(conditions[0]["name"], "__pool__", "got {resp}");
    &conditions[0]
}

/// Every file under `dir` with its modification time.
fn mtimes(dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap().flatten() {
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            out.push((path.clone(), meta.modified().unwrap()));
            if meta.is_dir() {
                stack.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Tick `name` until it reaches `done`, for at most 60 seconds.
fn tick_until_done(host: &Host, name: &str) -> serde_json::Value {
    let started = Instant::now();
    loop {
        let resp = host.next(name);
        if resp["state"] == "done" {
            return resp;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "{name} never reached done: {resp}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

// ---------------------------------------------------------------------------
// koto pool status
// ---------------------------------------------------------------------------

#[test]
fn status_lists_holders_and_waiters_in_both_forms_without_touching_anything() {
    let host = Host::new("[pools.test-suite]\nsize = 1\nwait_secs = 0\n");
    let holder = Holder::start(&host, "a");
    host.init("b", &pooled("true"));
    host.init("c", &pooled("true"));
    pool_condition(&host.next("b"));
    pool_condition(&host.next("c"));

    // A session's state lock held by another process changes nothing.
    let log = std::fs::File::open(host.log_in(&host.ws("b"), "b")).unwrap();
    // SAFETY: flock on a descriptor this test owns.
    assert_eq!(unsafe { libc::flock(log.as_raw_fd(), libc::LOCK_EX) }, 0);

    let before = mtimes(&host.root);
    std::thread::sleep(Duration::from_millis(1100));
    let started = Instant::now();
    let (code, status) = host.pool_status_json();
    let (plain_code, plain) = host.pool_status(&[]);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(mtimes(&host.root), before, "pool status changed a file");
    drop(log);

    assert_eq!(code, 0);
    assert_eq!(plain_code, 0);
    let pools = status["pools"].as_array().unwrap();
    assert_eq!(pools.len(), 1, "got {status}");
    let p = &pools[0];
    assert_eq!(p["name"], "test-suite");
    assert_eq!(p["size"], 1);
    assert_eq!(p["held"], 1);
    assert_eq!(p["free"], 0);
    assert_eq!(p["waiting"], 2);
    let h = &p["holders"][0];
    assert_eq!(h["session"], "a");
    assert_eq!(h["workspace"], "ws-a");
    assert_eq!(h["state"], "test");
    assert!(h["held_secs"].as_u64().unwrap() >= 1, "got {h}");
    let waiters: Vec<&str> = p["waiters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["session"].as_str().unwrap())
        .collect();
    assert_eq!(waiters, ["b", "c"]);
    assert_eq!(p["waiters"][0]["workspace"], "ws-b");
    assert!(p["waiters"][0]["waited_secs"].as_u64().is_some());

    println!("{plain}");
    assert!(plain.contains("pool test-suite: size 1, held 1, free 0, waiting 2"));
    assert!(plain.contains("holder: session a, workspace ws-a, state test, held"));
    assert!(plain.contains("waiter 1: session b, workspace ws-b, waited"));
    assert!(plain.contains("waiter 2: session c, workspace ws-c, waited"));

    holder.release();
    let (_, status) = host.pool_status_json();
    assert_eq!(status["pools"][0]["held"], 0);
    assert_eq!(status["pools"][0]["free"], 1);
}

#[test]
fn free_never_goes_below_zero_when_the_pool_shrinks() {
    let host = Host::new("[pools.test-suite]\nsize = 2\nwait_secs = 0\n");
    let a = Holder::start(&host, "a");
    let b = Holder::start(&host, "b");
    host.write_config("[pools.test-suite]\nsize = 1\nwait_secs = 0\n");
    let (code, status) = host.pool_status_json();
    assert_eq!(code, 0);
    assert_eq!(status["pools"][0]["held"], 2);
    assert_eq!(status["pools"][0]["free"], 0);
    a.release();
    b.release();
}

#[test]
fn with_no_pools_configured_status_says_so_and_exits_zero() {
    let host = Host::new("");
    let (code, plain) = host.pool_status(&[]);
    assert_eq!(code, 0);
    assert!(plain.contains("no pools are configured"), "got {plain}");
    let (code, status) = host.pool_status_json();
    assert_eq!(code, 0);
    assert_eq!(status["pools"], serde_json::json!([]));
    assert!(!host.root.join("pools").exists());
}

#[test]
fn an_undefined_pool_runs_unthrottled_and_is_not_listed() {
    let host = Host::new("[pools.test-suite]\nsize = 1\nwait_secs = 0\n");
    let ran = host.flag("ran");
    host.init(
        "u",
        &template(&format!("touch {}", ran.display()), "      pool: other\n"),
    );
    let resp = host.next("u");
    assert_eq!(resp["state"], "done", "got {resp}");
    assert!(ran.exists());
    let events = host.events("u");
    assert!(of_type(&events, "pool_acquired").is_empty());
    assert!(of_type(&events, "pool_queued").is_empty());
    assert!(!host.root.join("pools/other").exists());
    let (_, status) = host.pool_status_json();
    let names: Vec<&str> = status["pools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["test-suite"]);
}

// ---------------------------------------------------------------------------
// bad config fails only where it is used
// ---------------------------------------------------------------------------

#[test]
fn an_invalid_pool_fails_only_the_tick_that_reaches_it() {
    let host = Host::new("");
    for (i, (body, key)) in [
        ("size = \"two\"", "pools.test-suite.size"),
        ("size = 0", "pools.test-suite.size"),
        ("size = -1", "pools.test-suite.size"),
        ("size = 1.5", "pools.test-suite.size"),
        ("size = 1025", "pools.test-suite.size"),
        ("wait_secs = 1", "pools.test-suite.size"),
        ("size = 1\nclaim_secs = 0", "pools.test-suite.claim_secs"),
        (
            "size = 1\nlapse_secs = 86401",
            "pools.test-suite.lapse_secs",
        ),
        ("size = 1\nwait_secs = 61", "pools.test-suite.wait_secs"),
    ]
    .into_iter()
    .enumerate()
    {
        host.write_config(&format!("[pools.test-suite]\n{body}\n"));
        let name = format!("bad{i}");
        let go = host.flag(&format!("go-{name}"));
        let ran = host.flag(&format!("ran-{name}"));
        host.init(&name, &gated(&go, &format!("touch {}", ran.display())));

        host.run(&name, &["status", &name]);
        let list = host.koto(&name).args(["config", "list"]).output().unwrap();
        assert!(list.status.success(), "{body}: config list failed");
        // The tick that stays in `prep` never reaches the pool.
        let resp = host.next(&name);
        assert_eq!(resp["state"], "prep", "{body}: got {resp}");

        std::fs::write(&go, "").unwrap();
        let resp = host.next(&name);
        let c = pool_condition(&resp);
        assert_eq!(resp["state"], "test", "{body}: got {resp}");
        assert_eq!(c["output"]["error_code"], "pool_config_invalid", "{body}");
        assert_eq!(c["output"]["key"], key, "{body}");
        assert_eq!(
            c["output"]["config_file"],
            host.root.join("config.toml").display().to_string(),
            "{body}"
        );
        assert!(!ran.exists(), "{body}: the action ran");

        let (code, status) = host.pool_status_json();
        assert_eq!(code, 1, "{body}: got {status}");
        assert_eq!(status["pools"][0]["error_code"], "pool_config_invalid");
        assert_eq!(status["pools"][0]["key"], key);
        let (code, plain) = host.pool_status(&[]);
        assert_eq!(code, 1);
        assert!(plain.contains("pool_config_invalid"), "got {plain}");
    }
}

#[test]
fn the_bounds_themselves_are_accepted() {
    let host = Host::new("");
    for (i, body) in [
        "size = 1024",
        "size = 1\nclaim_secs = 1",
        "size = 1\nlapse_secs = 86400",
        "size = 1\nwait_secs = 0",
    ]
    .into_iter()
    .enumerate()
    {
        host.write_config(&format!("[pools.test-suite]\n{body}\n"));
        let name = format!("ok{i}");
        let ran = host.flag(&format!("ran-{name}"));
        host.init(&name, &pooled(&format!("touch {}", ran.display())));
        let resp = host.next(&name);
        assert_eq!(resp["state"], "done", "{body}: got {resp}");
        assert!(ran.exists(), "{body}: the action didn't run");
        assert_eq!(of_type(&host.events(&name), "pool_acquired").len(), 1);
        let (code, _) = host.pool_status_json();
        assert_eq!(code, 0, "{body}");
    }
}

#[test]
fn a_project_config_pool_changes_nothing_and_is_listed_as_ignored() {
    let host = Host::new("");
    let ws = host.ws("p");
    std::fs::create_dir_all(ws.join(".koto")).unwrap();
    std::fs::write(
        ws.join(".koto/config.toml"),
        "[pools.test-suite]\nsize = 1\n",
    )
    .unwrap();
    let trust = host.koto("p").args(["config", "trust"]).output().unwrap();
    assert!(
        trust.status.success(),
        "trust failed: {}",
        String::from_utf8_lossy(&trust.stderr)
    );

    let list = host.koto("p").args(["config", "list"]).output().unwrap();
    assert!(list.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&list.stdout),
        String::from_utf8_lossy(&list.stderr)
    );
    println!("{text}");
    assert!(
        text.contains("project config: pools.test-suite.size ignored"),
        "got {text}"
    );

    let ran = host.flag("ran-p");
    host.init("p", &pooled(&format!("touch {}", ran.display())));
    let resp = host.next("p");
    assert_eq!(resp["state"], "done", "got {resp}");
    assert!(ran.exists());
    assert!(of_type(&host.events("p"), "pool_acquired").is_empty());
    assert!(!host.root.join("pools").exists());
}

// ---------------------------------------------------------------------------
// the pools directory
// ---------------------------------------------------------------------------

#[test]
fn an_unwritable_pools_directory_stops_the_tick_naming_the_path() {
    let host = Host::new("[pools.test-suite]\nsize = 1\nwait_secs = 0\n");
    let pools = host.root.join("pools");
    std::fs::create_dir(&pools).unwrap();
    std::fs::set_permissions(&pools, std::fs::Permissions::from_mode(0o500)).unwrap();

    let ran = host.flag("ran");
    host.init("d", &pooled(&format!("touch {}", ran.display())));
    let resp = host.next("d");
    std::fs::set_permissions(&pools, std::fs::Permissions::from_mode(0o700)).unwrap();

    let c = pool_condition(&resp);
    assert_eq!(
        c["output"]["error_code"], "pool_dir_unavailable",
        "got {resp}"
    );
    let path = c["output"]["path"].as_str().unwrap();
    assert!(
        path.starts_with(pools.to_str().unwrap()),
        "{path} is not under {}",
        pools.display()
    );
    assert!(!ran.exists(), "the action ran");
}

// ---------------------------------------------------------------------------
// the events
// ---------------------------------------------------------------------------

#[test]
fn a_queued_background_run_records_its_events_in_order() {
    let host = Host::new("[pools.test-suite]\nsize = 1\nwait_secs = 0\n");
    let holder = Holder::start(&host, "h");
    host.init(
        "bg",
        &template(
            "true",
            "      pool: test-suite\n      background: true\n      timeout_secs: 30\n",
        ),
    );
    for _ in 0..5 {
        let resp = host.next("bg");
        assert_eq!(pool_condition(&resp)["status"], "queued");
    }
    assert_eq!(of_type(&host.events("bg"), "pool_queued").len(), 1);
    holder.release();
    tick_until_done(&host, "bg");

    let events = host.events("bg");
    let kinds = [
        "pool_queued",
        "pool_acquired",
        "background_started",
        "background_finished",
        "pool_released",
    ];
    let seen: Vec<&serde_json::Value> = events
        .iter()
        .filter(|e| kinds.contains(&e["type"].as_str().unwrap()))
        .collect();
    let types: Vec<&str> = seen.iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert_eq!(types, kinds, "got {types:?}");
    for e in &seen {
        if e["type"].as_str().unwrap().starts_with("pool_") {
            assert_eq!(e["payload"]["pool"], "test-suite", "got {e}");
        }
    }
    assert_eq!(seen[4]["payload"]["reason"], "done");
    let run_id = &seen[2]["payload"]["run_id"];
    assert!(run_id.is_string());
    assert_eq!(&seen[3]["payload"]["run_id"], run_id);
}

// ---------------------------------------------------------------------------
// nesting
// ---------------------------------------------------------------------------

/// A template that reaches its pooled `test` state through a non-pooled
/// `start` state's action.
fn pooled_after_start(command: &str) -> String {
    format!(
        r#"---
name: pool-status-nested
version: "1.0"
initial_state: start
states:
  start:
    default_action:
      command: "true"
    transitions:
      - target: test
  test:
    default_action:
      command: {command:?}
      pool: test-suite
    transitions:
      - target: done
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

/// The `pool_acquired` events of every log named.
fn acquired(host: &Host, logs: &[(PathBuf, &str)]) -> usize {
    logs.iter()
        .map(|(ws, name)| of_type(&host.events_in(ws, name), "pool_acquired").len())
        .sum()
}

#[test]
fn a_nested_same_pool_action_runs_on_the_outer_slot() {
    let host = Host::new("[pools.test-suite]\nsize = 1\nwait_secs = 0\n");
    let koto = koto_command::koto_bin();
    let ws = host.ws("outer");
    let ran1 = host.flag("ran-inner1");
    let ran2 = host.flag("ran-inner2");
    // inner1 starts in its pooled state; inner2 reaches it after a
    // transition. Both live in the outer session's workspace.
    host.init_in(
        "inner1",
        &ws,
        &host.root.clone(),
        &pooled(&format!("touch {}", ran1.display())),
    );
    host.init_in(
        "inner2",
        &ws,
        &host.root.clone(),
        &pooled_after_start(&format!("touch {}", ran2.display())),
    );
    let out1 = host.flag("out-inner1.json");
    let out2 = host.flag("out-inner2.json");
    let command = format!(
        "set -e; {k} next inner1 --no-cleanup > {o1}; {k} next inner2 --no-cleanup > {o2}",
        k = koto.display(),
        o1 = out1.display(),
        o2 = out2.display(),
    );
    host.init(
        "outer",
        &template(
            &command,
            "      pool: test-suite\n      background: true\n      timeout_secs: 60\n",
        ),
    );
    let started = Instant::now();
    tick_until_done(&host, "outer");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the nested runs waited: {:?}",
        started.elapsed()
    );

    for (out, ran) in [(&out1, &ran1), (&out2, &ran2)] {
        let resp: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(out).unwrap()).unwrap();
        assert_eq!(resp["state"], "done", "got {resp}");
        assert!(ran.exists());
    }
    let logs = [
        (ws.clone(), "outer"),
        (ws.clone(), "inner1"),
        (ws.clone(), "inner2"),
    ];
    let total = acquired(&host, &logs);
    println!("pool_acquired across the outer and nested sessions: {total}");
    assert_eq!(total, 1);
    assert_eq!(
        of_type(&host.events_in(&ws, "outer"), "pool_acquired").len(),
        1
    );
    for name in ["inner1", "inner2"] {
        let events = host.events_in(&ws, name);
        assert!(of_type(&events, "pool_queued").is_empty(), "{name} queued");
        assert!(
            of_type(&events, "pool_released").is_empty(),
            "{name} released"
        );
    }
}

#[test]
fn a_nested_action_on_another_pool_or_root_queues_normally() {
    let host = Host::new("[pools.test-suite]\nsize = 1\nwait_secs = 0\n[pools.other]\nsize = 1\n");
    let koto = koto_command::koto_bin();
    let ws = host.ws("outer");
    let root2 = host.base().join("koto-root-2");
    std::fs::create_dir_all(&root2).unwrap();
    std::fs::write(
        root2.join("config.toml"),
        "[pools.test-suite]\nsize = 1\nwait_secs = 0\n",
    )
    .unwrap();
    host.init_in(
        "other-pool",
        &ws,
        &host.root.clone(),
        &template("true", "      pool: other\n"),
    );
    host.init_in("other-root", &ws, &root2, &pooled("true"));
    let out_pool = host.flag("out-other-pool.json");
    let out_root = host.flag("out-other-root.json");
    let command = format!(
        "set -e; {k} next other-pool --no-cleanup > {o1}; \
         KOTO_HOME={r2} {k} next other-root --no-cleanup > {o2}",
        k = koto.display(),
        r2 = root2.display(),
        o1 = out_pool.display(),
        o2 = out_root.display(),
    );
    host.init(
        "outer",
        &template(
            &command,
            "      pool: test-suite\n      background: true\n      timeout_secs: 60\n",
        ),
    );
    tick_until_done(&host, "outer");

    for out in [&out_pool, &out_root] {
        let resp: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(out).unwrap()).unwrap();
        assert_eq!(resp["state"], "done", "got {resp}");
    }
    // Each took a slot of its own through the ordinary guard.
    let other = host.events_in(&ws, "other-pool");
    let acquired = of_type(&other, "pool_acquired");
    assert_eq!(acquired.len(), 1);
    assert_eq!(acquired[0]["payload"]["pool"], "other");
    let other_root = host.events_in(&ws, "other-root");
    assert_eq!(of_type(&other_root, "pool_acquired").len(), 1);
    assert!(root2.join("pools/test-suite").is_dir());
}

#[test]
fn a_stale_marker_grants_nothing() {
    let host = Host::new("[pools.test-suite]\nsize = 1\nwait_secs = 0\n");
    let holder = Holder::start(&host, "a");
    host.init("s", &pooled("true"));
    // A marker naming the held slot but not its holder is ignored.
    let out = host
        .koto("s")
        .env("KOTO_POOL_HELD", "test-suite/0/not-the-holder")
        .env("KOTO_POOL_ROOT", &host.root)
        .args(["next", "s", "--no-cleanup"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let resp: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(pool_condition(&resp)["status"], "queued", "got {resp}");
    holder.release();
}
