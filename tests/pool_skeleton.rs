//! The pooled-action skeleton (DESIGN-host-resource-pools.md, PLAN step 1).
//!
//! A template names a host pool on a state's `default_action`; the user's
//! koto config sizes it. These cases pin the whole path for an ordinary
//! action: the compile checks, a second session on a size-1 pool told it is
//! queued while the first holds the slot, the slot passing on once the first
//! action ends, a tick that reaches the full pool after earlier transitions,
//! and a machine without `[pools]` seeing no change at all.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use assert_cmd::Command;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

/// One koto root shared by several workspaces, each its own directory.
struct Host {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Host {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("koto-root");
        std::fs::create_dir_all(&root).unwrap();
        Host { _tmp: tmp, root }
    }

    fn base(&self) -> &Path {
        self._tmp.path()
    }

    fn write_config(&self, text: &str) {
        std::fs::write(self.root.join("config.toml"), text).unwrap();
    }

    fn workspace(&self, name: &str) -> PathBuf {
        let dir = self.base().join(name);
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        dir
    }

    fn env(&self, cmd: &mut std::process::Command, ws: &Path) {
        cmd.env_remove("CLAUDE_CODE_SESSION_ID")
            .current_dir(ws)
            .env("HOME", self.base())
            .env("KOTO_HOME", &self.root)
            .env("KOTO_SESSIONS_BASE", ws.join("sessions"));
    }

    fn koto(&self, ws: &Path) -> Command {
        let mut std_cmd = koto_command::koto_std();
        self.env(&mut std_cmd, ws);
        Command::from_std(std_cmd)
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

    fn next(&self, ws: &Path, name: &str) -> serde_json::Value {
        let out = self.koto(ws).args(["next", name]).output().unwrap();
        assert!(
            out.status.success(),
            "next should exit 0: stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).expect("next prints JSON")
    }

    /// Start `koto next` without waiting for it.
    fn spawn_next(&self, ws: &Path, name: &str) -> std::process::Child {
        let mut cmd = koto_command::koto_std();
        self.env(&mut cmd, ws);
        cmd.args(["next", name])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    }
}

fn wait_for(path: &Path) {
    let started = Instant::now();
    while !path.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "{} never appeared",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A template whose `test` state runs `command`, naming `pool` when given,
/// then goes to `done`.
fn template(command: &str, pool: Option<&str>) -> String {
    let pool_line = pool
        .map(|p| format!("      pool: {p}\n"))
        .unwrap_or_default();
    format!(
        r#"---
name: pool-skeleton
version: "1.0"
initial_state: test
states:
  test:
    default_action:
      command: {command:?}
{pool_line}    transitions:
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

/// The one `__pool__` condition of a blocked response.
fn pool_condition(resp: &serde_json::Value) -> &serde_json::Value {
    assert_eq!(resp["action"], "gate_blocked", "got {resp}");
    let conditions = resp["blocking_conditions"].as_array().unwrap();
    assert_eq!(conditions.len(), 1, "got {conditions:?}");
    assert_eq!(conditions[0]["name"], "__pool__");
    &conditions[0]
}

fn assert_queued_behind(resp: &serde_json::Value, holder: &str) {
    let c = pool_condition(resp);
    assert_eq!(c["type"], "pool");
    assert_eq!(c["status"], "queued");
    assert_eq!(c["category"], "temporal");
    assert_eq!(c["agent_actionable"], false);
    let out = &c["output"];
    assert_eq!(out["pool"], "test-suite");
    assert_eq!(out["size"], 1);
    assert_eq!(out["position"], 1);
    assert_eq!(out["waiting"], 1);
    assert_eq!(out["retry_after_secs"], 15);
    let holders = out["holders"].as_array().unwrap();
    assert_eq!(holders.len(), 1, "got {holders:?}");
    assert_eq!(holders[0]["session"], holder);
    assert_eq!(holders[0]["workspace"], "ws-a");
    assert_eq!(holders[0]["state"], "test");
}

const SIZE_ONE: &str = "[pools.test-suite]\nsize = 1\nwait_secs = 0\n";

/// A command that marks it started, then waits for `release`.
fn held_command(started: &Path, release: &Path, ran: &str) -> String {
    format!(
        "touch {} && while [ ! -f {} ]; do sleep 0.05; done && echo {ran}",
        started.display(),
        release.display()
    )
}

// ---------------------------------------------------------------------------
// compile
// ---------------------------------------------------------------------------

fn compile(host: &Host, body: &str) -> std::process::Output {
    let ws = host.workspace("compile");
    let src = ws.join("t.md");
    std::fs::write(&src, body).unwrap();
    host.koto(&ws)
        .args(["template", "compile", src.to_str().unwrap()])
        .output()
        .unwrap()
}

fn action_template(action_extra: &str, state_extra: &str) -> String {
    format!(
        r#"---
name: pool-compile
version: "1.0"
initial_state: test
variables:
  POOL:
    default: test-suite
states:
  test:
{state_extra}    default_action:
      command: "echo ran"
{action_extra}    transitions:
      - target: done
  done:
    terminal: true
---

## test

Run.

## done

Done.
"#
    )
}

#[test]
fn compile_accepts_a_pool_name() {
    let host = Host::new();
    let out = compile(&host, &action_template("      pool: test-suite\n", ""));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let compiled: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(
        compiled["states"]["test"]["default_action"]["pool"],
        "test-suite"
    );
}

#[test]
fn compile_refuses_bad_pool_declarations_naming_state_and_field() {
    let host = Host::new();
    for (action_extra, state_extra, field) in [
        ("      pool: Test\n", "", "pool"),
        ("      pool: \"{{POOL}}\"\n", "", "pool"),
        ("", "    size: 2\n", "size"),
        ("      size: 2\n", "", "size"),
        ("      timeout_secs: 60\n", "", "timeout_secs"),
        ("      background: true\n", "", "timeout_secs"),
        (
            "      background: true\n      timeout_secs: 0\n",
            "",
            "timeout_secs",
        ),
        (
            "      background: true\n      timeout_secs: 86401\n",
            "",
            "timeout_secs",
        ),
    ] {
        let out = compile(&host, &action_template(action_extra, state_extra));
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            !out.status.success(),
            "{action_extra}{state_extra} should be refused; stdout={stdout}"
        );
        let text = format!("{stdout}{stderr}");
        assert!(
            text.contains("\\\"test\\\"") || text.contains("\"test\""),
            "the error names the state: {text}"
        );
        assert!(text.contains(field), "the error names {field}: {text}");
    }
}

/// The compiled-template hash of [`template`] without a pool, as koto
/// 0.37.2, the release before pools, computed it. The compile cache names
/// the file after the hash.
const PRE_POOL_HASH: &str = "41589a50efae94fa676f8190449d87640d9fa1f4187f280697ce72bc70e68040";

#[test]
fn a_template_without_pool_compiles_to_the_pre_pool_hash() {
    let host = Host::new();
    let out = compile(&host, &template("echo ran", None));
    assert!(out.status.success());
    let path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    assert_eq!(
        path.file_stem().unwrap().to_str().unwrap(),
        PRE_POOL_HASH,
        "the compiled output of a template without pool changed"
    );
    let text = std::fs::read_to_string(&path).unwrap();
    for key in ["\"pool\"", "\"background\"", "\"timeout_secs\""] {
        assert!(!text.contains(key), "{key} serialized when unset: {text}");
    }
}

// ---------------------------------------------------------------------------
// runtime
// ---------------------------------------------------------------------------

#[test]
fn a_second_session_is_queued_until_the_first_action_ends() {
    let host = Host::new();
    host.write_config(SIZE_ONE);
    let ws_a = host.workspace("ws-a");
    let ws_b = host.workspace("ws-b");
    let started_a = host.base().join("started-a");
    let release = host.base().join("release");
    let ran_b = host.base().join("ran-b");
    host.init(
        &ws_a,
        "a",
        &template(&held_command(&started_a, &release, "a"), Some("test-suite")),
    );
    host.init(
        &ws_b,
        "b",
        &template(
            &format!("touch {} && echo b", ran_b.display()),
            Some("test-suite"),
        ),
    );

    let first = host.spawn_next(&ws_a, "a");
    wait_for(&started_a);

    let queued = host.next(&ws_b, "b");
    assert_eq!(queued["state"], "test");
    assert_queued_behind(&queued, "a");
    assert!(!ran_b.exists(), "the queued session ran its action");

    std::fs::write(&release, "").unwrap();
    let out = first.wait_with_output().unwrap();
    assert!(out.status.success());
    let first_resp: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(first_resp["state"], "done", "got {first_resp}");

    let after = host.next(&ws_b, "b");
    assert_eq!(after["state"], "done", "got {after}");
    assert!(ran_b.exists(), "the second session's action ran");
}

#[test]
fn a_tick_that_reaches_a_full_pool_keeps_its_earlier_transitions() {
    let host = Host::new();
    host.write_config(SIZE_ONE);
    let ws_a = host.workspace("ws-a");
    let ws_b = host.workspace("ws-b");
    let started_a = host.base().join("started-a");
    let release = host.base().join("release");
    let ran_b = host.base().join("ran-b");
    host.init(
        &ws_a,
        "a",
        &template(&held_command(&started_a, &release, "a"), Some("test-suite")),
    );
    let chained = format!(
        r#"---
name: pool-chain
version: "1.0"
initial_state: one
states:
  one:
    gates:
      ok:
        type: command
        command: "true"
    transitions:
      - target: two
        when:
          gates.ok.exit_code: 0
  two:
    gates:
      ok:
        type: command
        command: "true"
    transitions:
      - target: test
        when:
          gates.ok.exit_code: 0
  test:
    default_action:
      command: "touch {}"
      pool: test-suite
    transitions:
      - target: done
  done:
    terminal: true
---

## one

One.

## two

Two.

## test

Run the suite.

## done

Done.
"#,
        ran_b.display()
    );
    host.init(&ws_b, "b", &chained);

    let first = host.spawn_next(&ws_a, "a");
    wait_for(&started_a);

    let resp = host.next(&ws_b, "b");
    assert_eq!(resp["state"], "test", "got {resp}");
    assert_eq!(resp["advanced"], true);
    assert_queued_behind(&resp, "a");
    assert!(!ran_b.exists(), "the action ran on a full pool");

    std::fs::write(&release, "").unwrap();
    assert!(first.wait_with_output().unwrap().status.success());
}

/// Remove every `timestamp` and `since` key, recursively.
fn strip_times(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            map.remove("timestamp");
            map.remove("since");
            map.values_mut().for_each(strip_times);
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(strip_times),
        _ => {}
    }
}

#[test]
fn without_a_pools_table_a_pooled_action_runs_as_if_unpooled() {
    let host = Host::new();
    host.write_config("[session]\nbackend = \"local\"\n");
    let ws_pooled = host.workspace("pooled");
    let ws_plain = host.workspace("plain");
    host.init(&ws_pooled, "w", &template("echo ran", Some("test-suite")));
    host.init(&ws_plain, "w", &template("echo ran", None));

    let mut pooled = host.next(&ws_pooled, "w");
    let mut plain = host.next(&ws_plain, "w");
    strip_times(&mut pooled);
    strip_times(&mut plain);
    assert_eq!(pooled, plain);
    assert_eq!(pooled["state"], "done");
    assert!(
        !host.root.join("pools").exists(),
        "an undefined pool created the pools directory"
    );
}

#[test]
fn an_invalid_pool_runs_nothing_and_names_the_key() {
    let host = Host::new();
    host.write_config("[pools.test-suite]\nsize = 0\n");
    let ws = host.workspace("ws");
    let ran = host.base().join("ran");
    host.init(
        &ws,
        "w",
        &template(&format!("touch {}", ran.display()), Some("test-suite")),
    );
    let resp = host.next(&ws, "w");
    let c = pool_condition(&resp);
    assert_eq!(c["status"], "invalid");
    assert_eq!(c["output"]["error_code"], "pool_config_invalid");
    assert_eq!(c["output"]["key"], "pools.test-suite.size");
    assert!(!ran.exists());
}
