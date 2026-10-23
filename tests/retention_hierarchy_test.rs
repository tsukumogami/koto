//! Integration tests for `koto session hierarchy` and the three retention
//! settings, through the binary: what the query prints (text and JSON),
//! the stale report and its close command end to end, the settings with
//! their origins in the query and in `koto config list`, the refusals, and
//! that the query writes nothing.
//!
//! Every command runs with a temporary HOME and sessions directory, so the
//! user's own configuration and store are never read or written, and with
//! the cloud backend variables removed.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use assert_cmd::Command;
use assert_fs::TempDir;
use serde_json::Value;
use sha2::{Digest, Sha256};

// ===== Harness =====

const TEMPLATE: &str = r#"---
name: scope
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      status:
        type: string
        required: true
    transitions:
      - target: done
        when:
          status: done
  done:
    terminal: true
---

## work

Work.

## done

Done.
"#;

/// A workspace: `proj/` is the working directory (its `.koto/config.toml`
/// is the project file), `home/` is HOME (its `.koto/config.toml` is the
/// user file), `sessions/` is the store.
struct Ws {
    tmp: TempDir,
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Ws {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        for d in ["proj", "home", "sessions"] {
            std::fs::create_dir_all(tmp.path().join(d)).unwrap();
        }
        std::fs::write(tmp.path().join("proj/scope.md"), TEMPLATE).unwrap();
        Ws { tmp }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.tmp.path().join(rel)
    }

    fn sessions(&self) -> PathBuf {
        self.path("sessions")
    }

    fn user_file(&self, text: &str) {
        std::fs::create_dir_all(self.path("home/.koto")).unwrap();
        std::fs::write(self.path("home/.koto/config.toml"), text).unwrap();
    }

    fn project_file(&self, text: &str) {
        std::fs::create_dir_all(self.path("proj/.koto")).unwrap();
        std::fs::write(self.path("proj/.koto/config.toml"), text).unwrap();
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Out {
        let mut cmd = Command::cargo_bin("koto").unwrap();
        for var in [
            "CLAUDE_CODE_SESSION_ID",
            "KOTO_SESSION_BACKEND",
            "KOTO_SESSION_CLOUD_ENDPOINT",
            "KOTO_SESSION_CLOUD_BUCKET",
            "KOTO_ANCHOR",
            "KOTO_DISPATCHER_ANCHOR",
            "KOTO_RETENTION_KEEP",
            "KOTO_RETENTION_IDLE_PERIOD",
            "KOTO_RETENTION_STALE_AFTER",
            "KOTO_WORKFLOWS_DIR",
        ] {
            cmd.env_remove(var);
        }
        cmd.current_dir(self.path("proj"));
        cmd.env("HOME", self.path("home"));
        cmd.env("KOTO_SESSIONS_BASE", self.sessions());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let output = cmd.args(args).output().unwrap();
        Out {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        }
    }

    fn ok(&self, args: &[&str], env: &[(&str, &str)]) -> Out {
        let out = self.run(args, env);
        assert_eq!(
            out.code, 0,
            "{args:?}\nstdout: {}\nstderr: {}",
            out.stdout, out.stderr
        );
        out
    }

    fn init(&self, name: &str, extra: &[&str], env: &[(&str, &str)]) {
        let template = self.path("proj/scope.md");
        let mut args = vec!["init", name, "--template", template.to_str().unwrap()];
        args.extend_from_slice(extra);
        self.ok(&args, env);
    }

    fn finish(&self, name: &str) {
        self.ok(
            &[
                "next",
                name,
                "--with-data",
                r#"{"status":"done"}"#,
                "--no-cleanup",
            ],
            &[],
        );
    }

    fn hierarchy(&self, name: &str, env: &[(&str, &str)]) -> Value {
        let out = self.ok(&["session", "hierarchy", name, "--json"], env);
        serde_json::from_str(&out.stdout).unwrap()
    }

    /// SHA-256 of every file in the store, by relative path.
    fn store_hashes(&self) -> BTreeMap<PathBuf, String> {
        let root = self.sessions();
        let mut out = BTreeMap::new();
        let mut stack = vec![root.clone()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let digest = Sha256::digest(std::fs::read(&p).unwrap());
                    out.insert(
                        p.strip_prefix(&root).unwrap().to_path_buf(),
                        format!("{digest:x}"),
                    );
                }
            }
        }
        out
    }
}

fn keeper_names(v: &Value) -> Vec<String> {
    v["keepers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["name"].as_str().unwrap().to_string())
        .collect()
}

/// Short durations for the scenarios that wait.
const FAST: [(&str, &str); 3] = [
    ("KOTO_RETENTION_KEEP", "1s"),
    ("KOTO_RETENTION_IDLE_PERIOD", "1s"),
    ("KOTO_RETENTION_STALE_AFTER", "2s"),
];

// ===== The query =====

#[test]
fn a_live_dispatcher_keeps_its_finished_child() {
    let ws = Ws::new();
    ws.init("coord", &[], &[]);
    ws.init("worker", &["--dispatched-by", "coord"], &[]);
    ws.finish("worker");

    let v = ws.hierarchy("worker", &[]);
    assert_eq!(v["session"], "worker");
    assert_eq!(v["kept"], true);
    assert_eq!(keeper_names(&v), vec!["coord"]);
    assert_eq!(v["keepers"][0]["kind"], "koto_session");
    assert_eq!(v["keepers"][0]["observed"], false);
    assert!(v["retention"].is_null());
    let members = v["members"].as_array().unwrap();
    let worker = members.iter().find(|m| m["name"] == "worker").unwrap();
    assert_eq!(worker["alive"], false);
    assert_eq!(worker["close_kind"], "terminal");
    assert!(worker["stopped_at"].is_string());
    assert_eq!(worker["joined_by"][0]["via"], "dispatched-by link");
    assert_eq!(worker["joined_by"][0]["named"], "coord");

    let text = ws.ok(&["session", "hierarchy", "worker"], &[]).stdout;
    assert!(
        text.starts_with("worker: kept: alive in its hierarchy: coord (koto_session)"),
        "{text}"
    );
    assert!(text.contains("members:"), "{text}");
    assert!(text.contains("retention.keep = 7d (default)"), "{text}");
}

#[test]
fn a_finished_hierarchy_is_kept_through_its_retention_then_released() {
    let ws = Ws::new();
    ws.init("a", &[], &[]);
    ws.init("b", &["--dispatched-by", "a"], &[]);
    ws.finish("a");
    ws.finish("b");
    let v = ws.hierarchy("a", &FAST);
    assert_eq!(v["kept"], true, "{v}");
    assert!(keeper_names(&v).is_empty());
    assert_eq!(v["retention"]["release"]["member"], "b");
    assert_eq!(v["retention"]["release"]["close_kind"], "terminal");
    assert!(v["retention"]["passes_at"].is_string());
    let text = ws.ok(&["session", "hierarchy", "a"], &FAST).stdout;
    assert!(text.contains("release: b stopped (terminal)"), "{text}");
    assert!(text.contains("retention passes at:"), "{text}");

    std::thread::sleep(Duration::from_millis(1300));
    let v = ws.hierarchy("a", &FAST);
    assert_eq!(v["kept"], false, "{v}");
    assert_eq!(v["retention"]["release"]["member"], "b");
}

#[test]
fn the_stale_report_prints_the_close_command_that_then_closes_the_anchor() {
    let ws = Ws::new();
    ws.ok(
        &[
            "session",
            "anchor",
            "open",
            "--harness-session",
            "S1",
            "--source",
            "startup",
            "--observed-key",
            "K1",
        ],
        &[],
    );
    ws.init("b", &[], &[("KOTO_ANCHOR", "anchor-S1")]);
    ws.finish("b");

    // Inside the stale threshold: kept by the anchor, no command.
    let v = ws.hierarchy("b", &FAST);
    assert_eq!(keeper_names(&v), vec!["anchor-S1"], "{v}");
    assert_eq!(v["keepers"][0]["observed"], true);
    assert!(v["keepers"][0].get("close_command").is_none());

    // Past it: stale, with the command filled in, and still kept.
    std::thread::sleep(Duration::from_millis(2200));
    let v = ws.hierarchy("b", &FAST);
    assert_eq!(keeper_names(&v), vec!["anchor-S1"], "{v}");
    let k = &v["keepers"][0];
    assert_eq!(k["stale"], true);
    let command = k["close_command"].as_str().unwrap().to_string();
    assert_eq!(
        command,
        "koto session anchor close --key='K1' --harness-session='S1'"
    );
    let text = ws.ok(&["session", "hierarchy", "b"], &FAST).stdout;
    assert!(text.contains(&command), "{text}");
    assert!(
        text.contains("anchor-S1 (anchor, observed, stale)"),
        "{text}"
    );

    // Run the printed command, as an operator would.
    let out = ws.ok(
        &[
            "session",
            "anchor",
            "close",
            "--key=K1",
            "--harness-session=S1",
        ],
        &[],
    );
    assert_eq!(out.stdout.trim(), "closed=1");
    let v = ws.hierarchy("b", &FAST);
    assert_eq!(v["kept"], true, "{v}");
    assert_eq!(v["retention"]["release"]["close_kind"], "observed");
    std::thread::sleep(Duration::from_millis(1300));
    let v = ws.hierarchy("b", &FAST);
    assert_eq!(v["kept"], false, "{v}");
}

#[test]
fn the_query_writes_nothing() {
    let ws = Ws::new();
    ws.ok(
        &[
            "session",
            "anchor",
            "open",
            "--harness-session",
            "S1",
            "--source",
            "startup",
        ],
        &[],
    );
    ws.init("a", &[], &[("KOTO_ANCHOR", "anchor-S1")]);
    ws.init("b", &["--dispatched-by", "a"], &[]);
    let before = ws.store_hashes();
    for name in ["a", "b", "anchor-S1"] {
        ws.hierarchy(name, &[]);
        ws.ok(&["session", "hierarchy", name], &[]);
    }
    assert_eq!(ws.store_hashes(), before);
}

#[test]
fn an_unknown_or_malformed_name_exits_2() {
    let ws = Ws::new();
    let out = ws.run(&["session", "hierarchy", "nobody"], &[]);
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(out.stdout.contains("does not exist") || out.stderr.contains("does not exist"));
    let out = ws.run(&["session", "hierarchy", "../etc"], &[]);
    assert_eq!(out.code, 2);
}

#[test]
fn an_unreadable_sessions_directory_exits_non_zero() {
    use std::os::unix::fs::PermissionsExt;
    let ws = Ws::new();
    ws.init("a", &[], &[]);
    std::fs::set_permissions(ws.sessions(), std::fs::Permissions::from_mode(0o000)).unwrap();
    let listable = std::fs::read_dir(ws.sessions()).is_ok();
    let out = ws.run(&["session", "hierarchy", "a"], &[]);
    std::fs::set_permissions(ws.sessions(), std::fs::Permissions::from_mode(0o755)).unwrap();
    if listable {
        return; // root: permissions don't bind
    }
    assert_ne!(out.code, 0, "{}", out.stdout);
}

// ===== Settings =====

#[test]
fn defaults_apply_with_no_configuration() {
    let ws = Ws::new();
    ws.init("a", &[], &[]);
    let v = ws.hierarchy("a", &[]);
    let s = &v["settings"];
    assert_eq!(s["retention.keep"]["value"], "7d");
    assert_eq!(s["retention.idle_period"]["value"], "7d");
    assert_eq!(s["retention.stale_after"]["value"], "30d");
    for k in [
        "retention.keep",
        "retention.idle_period",
        "retention.stale_after",
    ] {
        assert_eq!(s[k]["origin"], "default", "{k}");
    }
}

#[test]
fn environment_over_project_over_user_with_origins_reported() {
    let ws = Ws::new();
    ws.user_file("[retention]\nkeep = \"2d\"\nidle_period = \"3d\"\nstale_after = \"40d\"\n");
    ws.project_file("[retention]\nidle_period = \"4h\"\n");
    ws.init("a", &[], &[]);
    let env = [("KOTO_RETENTION_STALE_AFTER", "50d")];
    let v = ws.hierarchy("a", &env);
    let s = &v["settings"];
    assert_eq!(s["retention.keep"]["value"], "2d");
    assert_eq!(s["retention.keep"]["origin"], "user");
    assert_eq!(s["retention.idle_period"]["value"], "4h");
    assert_eq!(s["retention.idle_period"]["origin"], "project");
    assert_eq!(s["retention.stale_after"]["value"], "50d");
    assert_eq!(s["retention.stale_after"]["origin"], "env");
    assert_eq!(
        s["retention.stale_after"]["variable"],
        "KOTO_RETENTION_STALE_AFTER"
    );

    // `koto config list` reports the same values and origins.
    let list: Value =
        serde_json::from_str(&ws.ok(&["config", "list", "--json"], &env).stdout).unwrap();
    assert_eq!(list["retention"]["keep"], "2d");
    assert_eq!(list["retention"]["idle_period"], "4h");
    assert_eq!(list["retention"]["stale_after"], "50d");
    assert_eq!(list["origins"]["retention.keep"]["source"], "user");
    assert_eq!(
        list["origins"]["retention.idle_period"]["source"],
        "project"
    );
    assert_eq!(list["origins"]["retention.stale_after"]["source"], "env");
    let text = ws.ok(&["config", "list"], &env).stdout;
    assert!(
        text.contains("# origin: retention.stale_after = env KOTO_RETENTION_STALE_AFTER"),
        "{text}"
    );
    assert!(text.contains("# origin: retention.keep = user"), "{text}");

    // With nothing set, `config list` shows the defaults.
    let bare = Ws::new();
    let list: Value =
        serde_json::from_str(&bare.ok(&["config", "list", "--json"], &[]).stdout).unwrap();
    assert_eq!(list["retention"]["keep"], "7d");
    assert_eq!(list["retention"]["stale_after"], "30d");
    assert_eq!(list["origins"]["retention.keep"]["source"], "default");
}

#[test]
fn a_stale_threshold_not_longer_than_the_retention_is_refused() {
    let ws = Ws::new();
    ws.init("a", &[], &[]);
    // One from the environment, the other from the user file.
    ws.user_file("[retention]\nkeep = \"7d\"\n");
    let out = ws.run(
        &["session", "hierarchy", "a"],
        &[("KOTO_RETENTION_STALE_AFTER", "7d")],
    );
    assert_ne!(out.code, 0);
    let all = format!("{}{}", out.stdout, out.stderr);
    assert!(all.contains("retention.stale_after"), "{all}");
    assert!(all.contains("KOTO_RETENTION_STALE_AFTER"), "{all}");
    assert!(all.contains("user config"), "{all}");
}

#[test]
fn a_malformed_value_is_refused_naming_the_setting() {
    let ws = Ws::new();
    ws.init("a", &[], &[]);
    let out = ws.run(
        &["session", "hierarchy", "a"],
        &[("KOTO_RETENTION_KEEP", "7x")],
    );
    assert_ne!(out.code, 0);
    let all = format!("{}{}", out.stdout, out.stderr);
    assert!(all.contains("retention.keep"), "{all}");

    ws.user_file("[retention]\nidle_period = 7\n");
    let out = ws.run(&["session", "hierarchy", "a"], &[]);
    assert_ne!(out.code, 0);
    let all = format!("{}{}", out.stdout, out.stderr);
    assert!(all.contains("retention.idle_period"), "{all}");
    // The rest of koto still loads that file.
    ws.ok(&["config", "list"], &[]);

    // `config set` refuses a malformed value up front.
    let out = ws.run(&["config", "set", "--user", "retention.keep", "7x"], &[]);
    assert_ne!(out.code, 0);
    ws.ok(&["config", "set", "retention.keep", "3d"], &[]);
    assert_eq!(
        ws.ok(&["config", "get", "retention.keep"], &[])
            .stdout
            .trim(),
        "3d"
    );
}

#[test]
fn the_terminal_tick_never_reads_the_settings() {
    // A malformed setting never fails a tick.
    let ws = Ws::new();
    ws.init("a", &[], &[("KOTO_RETENTION_KEEP", "7x")]);
    let out = ws.run(
        &["next", "a", "--with-data", r#"{"status":"done"}"#],
        &[
            ("KOTO_RETENTION_KEEP", "7x"),
            ("KOTO_RETENTION_STALE_AFTER", "1s"),
        ],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
}
