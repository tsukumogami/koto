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

// ---------------------------------------------------------------------------
// older sessions and attach (Issue 2)
// ---------------------------------------------------------------------------

/// A session that waits for evidence, so a tick stops rather than finishing.
const WAIT: &str = r#"---
name: wait
version: "1.0"
initial_state: wait
states:
  wait:
    accepts:
      go:
        type: enum
        required: true
        values: [yes]
    transitions:
      - target: done
        when:
          go: yes
  done:
    terminal: true
---

## wait

Wait.

## done

Done.
"#;

/// Make a session look like one created by a koto that recorded no
/// command environment.
fn strip_record(env: &Env, name: &str) {
    let state = env.state_path(name);
    let text = std::fs::read_to_string(&state).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut header: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    header
        .as_object_mut()
        .unwrap()
        .remove("command_environment");
    lines[0] = header.to_string();
    std::fs::write(&state, lines.join("\n") + "\n").unwrap();
    assert!(env.record(name).is_null());
}

fn events_of_type(env: &Env, name: &str, kind: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(env.state_path(name))
        .unwrap()
        .lines()
        .skip(1)
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .filter(|e| e["type"] == kind)
        .collect()
}

#[test]
fn an_older_session_adopts_a_record_once_with_a_notice() {
    let env = Env::new();
    let tpl = env.template("wait.md", WAIT);
    assert!(env.run(&[], &["init", "old", "--template", &tpl]).success);
    strip_record(&env, "old");

    let tick_path = format!("/opt/tick/bin:.:{SYSTEM_PATH}");
    let r = env.run(&[("PATH", &tick_path)], &["next", "old"]);
    assert!(r.success, "{}", r.stderr);
    let directive = r.json["directive"].as_str().unwrap_or_default();
    assert!(
        directive.contains("had no recorded command environment"),
        "notice missing: {directive}"
    );
    assert!(directive.contains("/opt/tick/bin"), "{directive}");

    let rec = env.record("old");
    assert_eq!(rec["path"], format!("/opt/tick/bin:{SYSTEM_PATH}").as_str());
    assert!(rec.get("legacy").is_none(), "adoption never sets legacy");
    let adopted = events_of_type(&env, "old", "environment_adopted");
    assert_eq!(adopted.len(), 1);
    assert_eq!(adopted[0]["payload"]["dropped"], serde_json::json!(["."]));

    let r = env.run(&[("PATH", &tick_path)], &["next", "old"]);
    assert!(r.success, "{}", r.stderr);
    let directive = r.json["directive"].as_str().unwrap_or_default();
    assert!(!directive.contains("had no recorded command environment"));
    assert_eq!(events_of_type(&env, "old", "environment_adopted").len(), 1);
}

const BATCH_CHILD: &str = r#"---
name: batch-child
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      marker:
        type: enum
        required: true
        values: [done]
    transitions:
      - target: done
        when:
          marker: done
  done:
    terminal: true
---

## work

Work.

## done

Done.
"#;

const BATCH_PARENT: &str = r#"---
name: batch-parent
version: "1.0"
initial_state: plan
states:
  plan:
    accepts:
      tasks:
        type: tasks
        required: true
    gates:
      done:
        type: children-complete
    materialize_children:
      from_field: tasks
      default_template: child.md
    transitions:
      - target: closed
        when:
          gates.done.all_complete: true
  closed:
    terminal: true
---

## plan

Plan the batch.

## closed

Closed.
"#;

#[test]
fn a_batch_parent_adopts_before_it_spawns_and_children_copy_it() {
    let env = Env::new();
    std::fs::write(env.cwd.join("child.md"), BATCH_CHILD).unwrap();
    let parent = env.cwd.join("parent.md");
    std::fs::write(&parent, BATCH_PARENT).unwrap();
    assert!(
        env.run(
            &[],
            &["init", "parent", "--template", parent.to_str().unwrap()]
        )
        .success
    );
    strip_record(&env, "parent");

    let tick_path = format!("/opt/parent-tick/bin:{SYSTEM_PATH}");
    let tasks = serde_json::json!({"tasks": [
        {"name": "A", "waits_on": [], "vars": {}},
    ]})
    .to_string();
    let r = env.run(
        &[("PATH", &tick_path)],
        &["next", "parent", "--with-data", &tasks],
    );
    assert!(r.success, "{}", r.stderr);
    let parent_rec = env.record("parent");
    assert_eq!(parent_rec["path"], tick_path.as_str());
    assert_eq!(
        env.record("parent.A"),
        parent_rec,
        "child copies the adopted record"
    );
    // A child spawned in the same tick would record the same values from this
    // process even if spawning came first, so the order is asserted directly:
    // adoption's event precedes the scheduler's.
    let seq = |kind: &str| -> u64 {
        events_of_type(&env, "parent", kind)
            .first()
            .unwrap_or_else(|| panic!("no {kind} event"))["seq"]
            .as_u64()
            .unwrap()
    };
    assert!(seq("environment_adopted") < seq("scheduler_ran"));

    // A child that exists before the upgrade adopts on its own first tick.
    strip_record(&env, "parent.A");
    let child_path = format!("/opt/child-tick/bin:{SYSTEM_PATH}");
    let r = env.run(
        &[("PATH", &child_path)],
        &["next", "parent.A", "--no-cleanup"],
    );
    assert!(r.success, "{}", r.stderr);
    assert_eq!(env.record("parent.A")["path"], child_path.as_str());
}

#[test]
fn attach_reports_drift_by_name_and_refuses_nothing() {
    let env = Env::new();
    let tpl = env.template("wait.md", WAIT);
    assert!(env.run(&[], &["init", "wf", "--template", &tpl]).success);
    let before = env.record("wf");

    // Only a dropped entry differs: attaches with no drift reported.
    let same = format!(".:{SYSTEM_PATH}");
    let r = env.run(
        &[("PATH", &same)],
        &["init", "wf", "--template", &tpl, "--attach-live"],
    );
    assert!(r.success, "{}", r.stderr);
    assert!(r.json.get("environment_drift").is_none(), "{}", r.stdout);

    let other = format!("/opt/drifted-dir/bin:{SYSTEM_PATH}");
    let xdg = env.home().join("drifted-config");
    let r = env.run(
        &[("PATH", &other), ("XDG_CONFIG_HOME", xdg.to_str().unwrap())],
        &["init", "wf", "--template", &tpl, "--attach-live"],
    );
    assert!(r.success, "attach must not be refused: {}", r.stderr);
    assert_eq!(r.json["outcome"], "attached");
    assert_eq!(
        r.json["environment_drift"],
        serde_json::json!(["PATH", "XDG_CONFIG_HOME"])
    );
    assert!(r.stderr.contains("PATH, XDG_CONFIG_HOME"), "{}", r.stderr);
    // Names only: neither the recorded nor the caller's value is printed.
    for output in [&r.stdout, &r.stderr] {
        assert!(!output.contains("/opt/drifted-dir"), "{output}");
        assert!(!output.contains("drifted-config"), "{output}");
        assert!(!output.contains(SYSTEM_PATH), "{output}");
    }
    assert_eq!(env.record("wf"), before, "attach never changes the record");
}

#[test]
fn attach_to_a_legacy_session_reports_no_drift() {
    let env = Env::new();
    let tpl = env.template("wait.md", WAIT);
    assert!(
        env.run(
            &[],
            &["init", "wf", "--template", &tpl, "--legacy-environment"]
        )
        .success
    );
    let other = format!("/opt/elsewhere/bin:{SYSTEM_PATH}");
    let r = env.run(
        &[("PATH", &other)],
        &["init", "wf", "--template", &tpl, "--attach-live"],
    );
    assert!(r.success, "{}", r.stderr);
    assert!(r.json.get("environment_drift").is_none(), "{}", r.stdout);
}

#[test]
fn a_displaced_writer_does_not_adopt() {
    // A --with-data write to a child log with the wrong dispatch epoch is
    // refused at the fence, which runs before adoption.
    let env = Env::new();
    let tpl = env.template("wait.md", WAIT);
    assert!(env.run(&[], &["init", "p", "--template", &tpl]).success);
    assert!(
        env.run(&[], &["init", "p.kid", "--template", &tpl, "--parent", "p"])
            .success
    );
    strip_record(&env, "p.kid");
    // The fence covers dispatched (needs-agent) children only.
    let state = env.state_path("p.kid");
    let text = std::fs::read_to_string(&state).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut header: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    header["needs_agent"] = serde_json::json!(true);
    lines[0] = header.to_string();
    std::fs::write(&state, lines.join("\n") + "\n").unwrap();
    let r = env.run(
        &[],
        &[
            "next",
            "p.kid",
            "--with-data",
            r#"{"go":"yes"}"#,
            "--dispatch-epoch",
            "99",
        ],
    );
    assert!(!r.success, "the fence must refuse: {}", r.stdout);
    assert!(
        r.stdout.contains("epoch_fence_violation"),
        "refused by the fence, not an earlier check: {}",
        r.stdout
    );
    assert!(
        env.record("p.kid").is_null(),
        "a refused writer records nothing"
    );
    assert!(events_of_type(&env, "p.kid", "environment_adopted").is_empty());
}

#[test]
fn attach_to_an_unrecorded_session_is_not_compared() {
    let env = Env::new();
    let tpl = env.template("wait.md", WAIT);
    assert!(env.run(&[], &["init", "wf", "--template", &tpl]).success);
    strip_record(&env, "wf");
    let other = format!("/opt/elsewhere/bin:{SYSTEM_PATH}");
    let r = env.run(
        &[("PATH", &other)],
        &["init", "wf", "--template", &tpl, "--attach-live"],
    );
    assert!(r.success, "{}", r.stderr);
    assert!(r.json.get("environment_drift").is_none());
    assert!(
        env.record("wf").is_null(),
        "attach doesn't adopt; the next tick does"
    );
}
