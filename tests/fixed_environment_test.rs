//! End-to-end coverage for the command environment a session records at
//! creation (DESIGN-koto-fixed-environment.md).
//!
//! Every case drives the real binary with a cleared environment, so the
//! variables a session sees are exactly the ones the test sets. Tests name
//! variables and compare values; none prints an environment wholesale.

#![cfg(unix)]

use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

/// A system `PATH` that finds the coreutils these tests' templates use.
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

struct Env {
    home: TempDir,
    cwd: PathBuf,
}

impl Env {
    fn new() -> Self {
        let home = TempDir::new().unwrap();
        let cwd = home.path().join("work");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(home.path().join("sessions")).unwrap();
        Env { home, cwd }
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    fn sessions(&self) -> PathBuf {
        self.home().join("sessions")
    }

    /// A `koto` invocation with nothing inherited from the test process:
    /// `PATH`, `HOME` and the session store are set here, plus `extra`.
    fn koto(&self, extra: &[(&str, &str)]) -> Command {
        let mut cmd = Command::cargo_bin("koto").unwrap();
        cmd.env_clear();
        cmd.current_dir(&self.cwd);
        cmd.env("PATH", SYSTEM_PATH);
        cmd.env("HOME", self.home());
        cmd.env("KOTO_SESSIONS_BASE", self.sessions());
        for (k, v) in extra {
            cmd.env(k, v);
        }
        cmd
    }

    fn run(&self, extra: &[(&str, &str)], args: &[&str]) -> Run {
        let output = self.koto(extra).args(args).output().unwrap();
        Run::from(output)
    }

    fn run_stdin(&self, extra: &[(&str, &str)], args: &[&str], stdin: &str) -> Run {
        let output = self
            .koto(extra)
            .args(args)
            .write_stdin(stdin.to_string())
            .output()
            .unwrap();
        Run::from(output)
    }

    fn template(&self, name: &str, body: &str) -> String {
        let path = self.home().join(name);
        std::fs::write(&path, body).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn state_path(&self, name: &str) -> PathBuf {
        self.sessions()
            .join(name)
            .join(format!("koto-{}.state.jsonl", name))
    }

    fn header(&self, name: &str) -> serde_json::Value {
        let text = std::fs::read_to_string(self.state_path(name)).unwrap();
        serde_json::from_str(text.lines().next().unwrap()).unwrap()
    }

    fn record(&self, name: &str) -> serde_json::Value {
        self.header(name)["command_environment"].clone()
    }

    /// Every file under a session's directory, read as bytes.
    fn session_files(&self, name: &str) -> Vec<(PathBuf, Vec<u8>)> {
        fn walk(dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let p = entry.unwrap().path();
                if p.is_dir() {
                    walk(&p, out);
                } else {
                    out.push((p.clone(), std::fs::read(&p).unwrap()));
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.sessions().join(name), &mut out);
        out
    }
}

struct Run {
    success: bool,
    code: Option<i32>,
    stdout: String,
    stderr: String,
    json: serde_json::Value,
}

impl From<std::process::Output> for Run {
    fn from(output: std::process::Output) -> Self {
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
        let json = serde_json::from_str(last).unwrap_or(serde_json::Value::Null);
        Run {
            success: output.status.success(),
            code: output.status.code(),
            stdout,
            stderr,
            json,
        }
    }
}

const SIMPLE: &str = r#"---
name: simple
version: "1.0"
initial_state: start
states:
  start:
    transitions:
      - target: done
  done:
    terminal: true
---

## start

Start.

## done

Done.
"#;

// ---------------------------------------------------------------------------
// recording
// ---------------------------------------------------------------------------

#[test]
fn plain_init_records_the_three_values_and_the_default_names() {
    let env = Env::new();
    let tpl = env.template("simple.md", SIMPLE);
    let xdg = env.home().join("cfg");
    let r = env.run(
        &[("XDG_CONFIG_HOME", xdg.to_str().unwrap())],
        &["init", "wf", "--template", &tpl],
    );
    assert!(r.success, "init failed: {}", r.stderr);
    let rec = env.record("wf");
    assert_eq!(rec["path"], SYSTEM_PATH);
    assert_eq!(rec["home"], env.home().to_str().unwrap());
    assert_eq!(rec["xdg_config_home"], xdg.to_str().unwrap());
    let pass: Vec<&str> = rec["pass"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(pass.contains(&"TMPDIR"));
    assert!(pass.contains(&"GH_TOKEN"));
    assert!(!pass.contains(&"GH_REPO"));
    assert!(!pass.iter().any(|n| n.starts_with("GIT_")));
    assert!(rec.get("legacy").is_none(), "legacy is omitted when false");
    assert!(r.json.get("environment").is_none(), "nothing to report");
}

#[test]
fn every_creation_form_records() {
    let env = Env::new();
    let tpl = env.template("simple.md", SIMPLE);

    assert!(env.run(&[], &["init", "plain", "--template", &tpl]).success);

    let vars = env.home().join("vars.json");
    std::fs::write(&vars, "[]").unwrap();
    assert!(
        env.run(
            &[],
            &[
                "init",
                "varsfile",
                "--template",
                &tpl,
                "--vars-file",
                vars.to_str().unwrap()
            ]
        )
        .success
    );

    assert!(
        env.run(
            &[],
            &["init", "replace", "--template", &tpl, "--replace-terminal"]
        )
        .success
    );

    assert!(
        env.run_stdin(&[], &["init", "inline", "--from-stdin"], SIMPLE)
            .success
    );

    assert!(
        env.run(&[], &["session", "start", "started", "--parent", "plain"])
            .success
    );

    for name in ["plain", "varsfile", "replace", "inline", "started"] {
        let rec = env.record(name);
        assert_eq!(rec["path"], SYSTEM_PATH, "{name}");
        assert_eq!(rec["home"], env.home().to_str().unwrap(), "{name}");
    }
}

#[test]
fn koto_leg_init_records() {
    let env = Env::new();
    let tpl = env.template("simple.md", SIMPLE);
    let req = env.run(
        &[],
        &[
            "request",
            "create",
            "--requested-by",
            "tester",
            "--coordinator-of-record",
            "coord",
            "--role",
            "work",
            "--template",
            "simple.md",
            "--inputs",
            "{}",
        ],
    );
    assert!(req.success, "request create: {}", req.stderr);
    let id = req.json["request_id"]
        .as_str()
        .or_else(|| req.json["id"].as_str())
        .unwrap_or_else(|| panic!("no request id in {}", req.stdout))
        .to_string();
    let leg = format!("{id}:work");
    let r = env.run(
        &[],
        &["init", "legged", "--template", &tpl, "--koto-leg", &leg],
    );
    assert!(r.success, "{}", r.stderr);
    assert_eq!(env.record("legged")["path"], SYSTEM_PATH);
}

#[test]
fn relative_and_empty_path_entries_are_dropped_and_reported() {
    let env = Env::new();
    let tpl = env.template("simple.md", SIMPLE);
    let path = format!(":.:bin:~/bin:{SYSTEM_PATH}:");
    let r = env.run(&[("PATH", &path)], &["init", "wf", "--template", &tpl]);
    assert!(r.success, "{}", r.stderr);
    assert_eq!(env.record("wf")["path"], SYSTEM_PATH);
    let dropped = &r.json["environment"]["dropped_path_entries"];
    assert_eq!(dropped, &serde_json::json!(["", ".", "bin", "~/bin", ""]));
}

#[test]
fn credential_markers_never_reach_the_session_directory() {
    let env = Env::new();
    let tpl = env.template("simple.md", SIMPLE);
    let markers = [
        ("GH_TOKEN", "marker-gh-token-9f1c2e"),
        ("GITHUB_TOKEN", "marker-github-token-4b7d"),
        ("NPM_TOKEN", "marker-npm-token-11aa"),
        ("SERVICE_SECRET", "marker-service-secret-77"),
        ("API_KEY", "marker-api-key-3c3c3c"),
        ("DB_PASSWORD", "marker-db-password-55"),
    ];
    let vars = env.home().join("vars.json");
    std::fs::write(&vars, "[]").unwrap();
    let vars = vars.to_string_lossy().into_owned();
    let forms = ["plain", "inline", "varsfile", "replace", "started"];
    for (i, form) in forms.iter().enumerate() {
        let name = format!("wf{i}");
        let r = match *form {
            "inline" => env.run_stdin(&markers, &["init", &name, "--from-stdin"], SIMPLE),
            "varsfile" => env.run(
                &markers,
                &["init", &name, "--template", &tpl, "--vars-file", &vars],
            ),
            "replace" => env.run(
                &markers,
                &["init", &name, "--template", &tpl, "--replace-terminal"],
            ),
            "started" => env.run(&markers, &["session", "start", &name, "--parent", "wf0"]),
            _ => env.run(&markers, &["init", &name, "--template", &tpl]),
        };
        assert!(r.success, "{form}: {}", r.stderr);
        for (path, bytes) in env.session_files(&name) {
            let text = String::from_utf8_lossy(&bytes);
            for (var, value) in &markers {
                assert!(
                    !text.contains(value),
                    "{var}'s value reached {}",
                    path.display()
                );
            }
        }
        // Values are recorded only for the three fixed variables.
        let rec = env.record(&name);
        let keys: Vec<&String> = rec.as_object().unwrap().keys().collect();
        for key in keys {
            assert!(
                ["path", "home", "xdg_config_home", "pass", "legacy"].contains(&key.as_str()),
                "unexpected record key {key}"
            );
        }
    }
}

#[test]
fn a_home_carrying_a_token_is_recorded_unset() {
    let env = Env::new();
    let tpl = env.template("simple.md", SIMPLE);
    let token = "ghp_marker0123456789";
    let home = env.home().join(token);
    std::fs::create_dir_all(&home).unwrap();
    let mut cmd = env.koto(&[("GH_TOKEN", token)]);
    cmd.env("HOME", &home);
    let r = Run::from(
        cmd.args(["init", "wf", "--template", &tpl])
            .output()
            .unwrap(),
    );
    assert!(r.success, "{}", r.stderr);
    assert!(env.record("wf").get("home").is_none());
    assert_eq!(
        r.json["environment"]["unset"],
        serde_json::json!([{"variable": "HOME", "reason": "credential"}])
    );
    assert!(!r.stdout.contains(token));
}

// ---------------------------------------------------------------------------
// the legacy flag
// ---------------------------------------------------------------------------

#[test]
fn legacy_environment_is_recorded_and_refused_with_parent() {
    let env = Env::new();
    let tpl = env.template("simple.md", SIMPLE);
    let r = env.run(
        &[],
        &["init", "old", "--template", &tpl, "--legacy-environment"],
    );
    assert!(r.success, "{}", r.stderr);
    assert_eq!(env.record("old")["legacy"], true);

    let r = env.run(
        &[],
        &[
            "init",
            "old.child",
            "--template",
            &tpl,
            "--parent",
            "old",
            "--legacy-environment",
        ],
    );
    assert!(!r.success);
    assert_eq!(r.code, Some(2));
    assert!(r.stdout.contains("--legacy-environment") || r.stderr.contains("--legacy-environment"));
}

// ---------------------------------------------------------------------------
// children
// ---------------------------------------------------------------------------

#[test]
fn a_parent_child_copies_the_parent_record_from_any_process() {
    let env = Env::new();
    let tpl = env.template("simple.md", SIMPLE);
    assert!(
        env.run(
            &[],
            &["init", "p", "--template", &tpl, "--legacy-environment"]
        )
        .success
    );

    let other_path = format!("/opt/elsewhere/bin:{SYSTEM_PATH}");
    let r = env.run(
        &[("PATH", &other_path)],
        &["init", "p.kid", "--template", &tpl, "--parent", "p"],
    );
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.json.get("environment").is_none(),
        "a copied record reports nothing: {}",
        r.stdout
    );
    let r = env.run(
        &[("PATH", &other_path)],
        &["session", "start", "p.started", "--parent", "p"],
    );
    assert!(r.success, "{}", r.stderr);

    let parent = env.record("p");
    assert_eq!(env.record("p.kid"), parent);
    assert_eq!(env.record("p.started"), parent);
    assert_eq!(parent["legacy"], true);
}

#[test]
fn a_child_of_an_unrecorded_parent_records_from_its_own_process() {
    let env = Env::new();
    let tpl = env.template("simple.md", SIMPLE);
    assert!(env.run(&[], &["init", "old", "--template", &tpl]).success);
    // Make the parent look like a session from an earlier koto.
    let state = env.state_path("old");
    let text = std::fs::read_to_string(&state).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut header: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    header
        .as_object_mut()
        .unwrap()
        .remove("command_environment");
    lines[0] = header.to_string();
    std::fs::write(&state, lines.join("\n") + "\n").unwrap();
    assert!(env.record("old").is_null());

    let child_path = format!("/opt/child/bin:{SYSTEM_PATH}");
    let child_path_with_relative = format!("/opt/child/bin:.:{SYSTEM_PATH}");
    let r = env.run(
        &[("PATH", &child_path_with_relative)],
        &["init", "old.kid", "--template", &tpl, "--parent", "old"],
    );
    assert!(r.success, "{}", r.stderr);
    // This child recorded from its own process, so the response reports
    // what recording dropped.
    assert_eq!(
        r.json["environment"]["dropped_path_entries"],
        serde_json::json!(["."])
    );
    let r = env.run(
        &[("PATH", &child_path_with_relative)],
        &["session", "start", "old.started", "--parent", "old"],
    );
    assert!(r.success, "{}", r.stderr);
    for child in ["old.kid", "old.started"] {
        let rec = env.record(child);
        assert_eq!(rec["path"], child_path.as_str(), "{child}");
        assert!(rec.get("legacy").is_none(), "{child}");
    }
}

#[test]
fn rebind_leaves_the_record_unchanged() {
    let env = Env::new();
    let tpl = env.template("simple.md", SIMPLE);
    assert!(env.run(&[], &["init", "wf", "--template", &tpl]).success);
    let before = env.record("wf");
    let elsewhere = env.home().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let r = env.run(
        &[],
        &[
            "session",
            "rebind",
            "wf",
            "--to",
            elsewhere.to_str().unwrap(),
        ],
    );
    assert!(r.success, "{}", r.stderr);
    assert_eq!(env.record("wf"), before);
}

// ---------------------------------------------------------------------------
// template declarations
// ---------------------------------------------------------------------------

fn with_pass_env(names: &str) -> String {
    SIMPLE.replacen(
        "initial_state: start\n",
        &format!("initial_state: start\npass_env: [{names}]\n"),
        1,
    )
}

#[test]
fn a_refused_or_malformed_declaration_is_a_compile_error() {
    let env = Env::new();
    for bad in [
        "BASH_ENV",
        "GIT_CONFIG_COUNT",
        "GH_CONFIG_DIR",
        "\"1BAD\"",
        "\"A*\"",
    ] {
        let tpl = env.template("bad.md", &with_pass_env(bad));
        let r = env.run(&[], &["init", "bad", "--template", &tpl]);
        assert!(!r.success, "{bad} should be refused");
        assert!(
            !env.state_path("bad").exists(),
            "{bad}: no session is created"
        );
    }
}

#[test]
fn declaring_a_koto_supplied_name_warns() {
    let env = Env::new();
    let tpl = env.template("warn.md", &with_pass_env("PATH, GH_DB"));
    let r = env.run(&[], &["init", "wf", "--template", &tpl]);
    assert!(r.success, "{}", r.stderr);
    assert!(r.stderr.contains("W7"), "expected W7, got: {}", r.stderr);
}
