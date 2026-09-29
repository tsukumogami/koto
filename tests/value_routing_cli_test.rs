//! Value routing end to end through the `koto` binary: routes taken on
//! entry, the `vars_matched` and `previous` records, `koto init`'s refusal
//! of a value a routed variable can't hold, and the one known template in
//! use whose behavior the change touches (shirabe's `/work-on` entry).
//!
//! docs/designs/DESIGN-koto-value-routing.md.

use assert_cmd::Command;
use assert_fs::TempDir;
use std::path::{Path, PathBuf};

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(dir));
    cmd.env("HOME", dir);
    cmd.env_remove("CLAUDE_CODE_SESSION_ID");
    cmd.env_remove("KOTO_WORKFLOWS_DIR");
    cmd
}

fn sessions_base(dir: &Path) -> PathBuf {
    let base = dir.join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn events(dir: &Path, name: &str) -> Vec<serde_json::Value> {
    let path = sessions_base(dir)
        .join(name)
        .join(format!("koto-{}.state.jsonl", name));
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .skip(1) // the header record
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn of_type<'a>(events: &'a [serde_json::Value], ty: &str) -> Vec<&'a serde_json::Value> {
    events.iter().filter(|e| e["type"] == ty).collect()
}

fn run(cmd: &mut Command) -> (bool, serde_json::Value, String) {
    let out = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let json = serde_json::from_str(&stdout).unwrap_or(serde_json::Value::Null);
    (out.status.success(), json, stdout)
}

const MODE_TEMPLATE: &str = r#"---
name: mode-routes
version: "1.0"
initial_state: route
variables:
  MODE:
    values: [auto, interactive]
    default: interactive
    rebind: true
  TAG:
    pattern: "v[0-9]+"
    default: v1
  NOTE:
    default: ""
states:
  route:
    transitions:
      - target: fast
        when:
          vars.MODE: auto
      - target: slow
        when:
          vars.MODE: interactive
  fast:
    accepts:
      note:
        type: string
        required: true
    transitions:
      - target: reroute
        when:
          evidence.note: present
  slow:
    accepts:
      note:
        type: string
        required: true
    transitions:
      - target: reroute
        when:
          evidence.note: present
  reroute:
    transitions:
      - target: done_auto
        when:
          vars.MODE: auto
      - target: done_interactive
        when:
          vars.MODE: interactive
  done_auto:
    terminal: true
  done_interactive:
    terminal: true
---

## route

Routes on MODE.

## fast

Fast path.

## slow

Slow path.

## reroute

Routes on MODE again.

## done_auto

Done.

## done_interactive

Done.
"#;

fn write_template(dir: &Path) -> PathBuf {
    let path = dir.join("mode-routes.md");
    std::fs::write(&path, MODE_TEMPLATE).unwrap();
    path
}

#[test]
fn value_routes_are_taken_on_entry_and_recorded() {
    for (mode, want) in [("auto", "fast"), ("interactive", "slow")] {
        let dir = TempDir::new().unwrap();
        let tpl = write_template(dir.path());
        let (ok, _, out) = run(koto_cmd(dir.path()).args([
            "init",
            "wf",
            "--template",
            tpl.to_str().unwrap(),
            "--var",
            &format!("MODE={mode}"),
            "--var",
            "TAG=v2",
        ]));
        assert!(ok, "init failed: {out}");
        let (ok, next, out) = run(koto_cmd(dir.path()).args(["next", "wf", "--no-cleanup"]));
        assert!(ok, "next failed: {out}");
        assert_eq!(next["state"], want, "MODE={mode}: {out}");
        assert_eq!(next["advanced"], true);

        let log = events(dir.path(), "wf");
        let init = of_type(&log, "workflow_initialized");
        let vars = &init[0]["payload"]["variables"];
        assert_eq!(vars["MODE"], mode, "passed value");
        assert_eq!(vars["TAG"], "v2", "passed value");
        assert_eq!(vars["NOTE"], "", "an empty variable is still recorded");
        let routed: Vec<_> = of_type(&log, "transitioned")
            .into_iter()
            .filter(|e| e["payload"]["from"] == "route")
            .collect();
        assert_eq!(routed.len(), 1);
        assert_eq!(
            routed[0]["payload"]["vars_matched"],
            serde_json::json!({ "MODE": mode })
        );
        // The initial transition didn't route on a value.
        let first = &of_type(&log, "transitioned")[0];
        assert!(first["payload"].get("vars_matched").is_none());
    }
}

#[test]
fn defaulted_variable_is_recorded_at_init() {
    let dir = TempDir::new().unwrap();
    let tpl = write_template(dir.path());
    let (ok, _, out) =
        run(koto_cmd(dir.path()).args(["init", "wf", "--template", tpl.to_str().unwrap()]));
    assert!(ok, "init failed: {out}");
    let log = events(dir.path(), "wf");
    let vars = &of_type(&log, "workflow_initialized")[0]["payload"]["variables"];
    assert_eq!(vars["MODE"], "interactive");
    assert_eq!(vars["TAG"], "v1");
}

#[test]
fn init_refuses_a_value_a_routed_variable_cannot_hold() {
    for bad in ["MODE=atuo", "TAG=release"] {
        let dir = TempDir::new().unwrap();
        let tpl = write_template(dir.path());
        let out = koto_cmd(dir.path())
            .args([
                "init",
                "wf",
                "--template",
                tpl.to_str().unwrap(),
                "--var",
                bad,
            ])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{bad}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("invalid_var"), "{bad}: {stdout}");
        assert!(
            !sessions_base(dir.path()).join("wf").exists(),
            "{bad}: a refused init left a session directory"
        );
    }
}

#[test]
fn a_rebind_logs_the_old_value_and_later_routes_see_the_new_one() {
    let dir = TempDir::new().unwrap();
    let tpl = write_template(dir.path());
    let tpl = tpl.to_str().unwrap();
    let (ok, _, out) =
        run(koto_cmd(dir.path()).args(["init", "wf", "--template", tpl, "--var", "MODE=auto"]));
    assert!(ok, "init failed: {out}");
    let (_, next, _) = run(koto_cmd(dir.path()).args(["next", "wf", "--no-cleanup"]));
    assert_eq!(next["state"], "fast");

    // Re-applying the same value changes nothing and logs nothing.
    let (ok, _, out) = run(koto_cmd(dir.path()).args([
        "init",
        "wf",
        "--template",
        tpl,
        "--attach-live",
        "--var",
        "MODE=auto",
    ]));
    assert!(ok, "same-value attach failed: {out}");
    assert!(of_type(&events(dir.path(), "wf"), "variables_rebound").is_empty());

    let (ok, _, out) = run(koto_cmd(dir.path()).args([
        "init",
        "wf",
        "--template",
        tpl,
        "--attach-live",
        "--var",
        "MODE=interactive",
    ]));
    assert!(ok, "rebinding attach failed: {out}");
    let log = events(dir.path(), "wf");
    let rebound = of_type(&log, "variables_rebound");
    assert_eq!(rebound.len(), 1);
    assert_eq!(
        rebound[0]["payload"]["variables"],
        serde_json::json!({"MODE": "interactive"})
    );
    assert_eq!(
        rebound[0]["payload"]["previous"],
        serde_json::json!({"MODE": "auto"})
    );

    let (ok, next, out) = run(koto_cmd(dir.path()).args([
        "next",
        "wf",
        "--no-cleanup",
        "--with-data",
        r#"{"note": "done"}"#,
    ]));
    assert!(ok, "next failed: {out}");
    assert_eq!(next["state"], "done_interactive", "{out}");
    let log = events(dir.path(), "wf");
    let last = of_type(&log, "transitioned").into_iter().last().unwrap();
    assert_eq!(
        last["payload"]["vars_matched"],
        serde_json::json!({"MODE": "interactive"})
    );
}

/// shirabe's `/work-on` entry state carries `vars.ISSUE_SOURCE: plan_outline`
/// in its `skip_if`, dead before value routing. Pinned: with `mode:
/// plan_backed` submitted it reaches `plan_context_injection` whether or not
/// ISSUE_SOURCE is set; only the record differs.
#[test]
fn shirabe_work_on_entry_reaches_the_same_target_with_and_without_issue_source() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/value-routing/work-on-entry.md");
    for issue_source in [Some("plan_outline"), None] {
        let dir = TempDir::new().unwrap();
        let mut init = koto_cmd(dir.path());
        init.args(["init", "wf", "--template", fixture.to_str().unwrap()]);
        if let Some(value) = issue_source {
            init.args(["--var", &format!("ISSUE_SOURCE={value}")]);
        }
        let (ok, _, out) = run(&mut init);
        assert!(ok, "init failed: {out}");

        // On arrival nothing is submitted, so the skip_if can't hold.
        let (_, next, out) = run(koto_cmd(dir.path()).args(["next", "wf", "--no-cleanup"]));
        assert_eq!(next["state"], "entry", "{out}");
        assert_eq!(next["action"], "evidence_required", "{out}");

        let (ok, next, out) = run(koto_cmd(dir.path()).args([
            "next",
            "wf",
            "--no-cleanup",
            "--with-data",
            r#"{"mode": "plan_backed", "issue_source": "plan_outline"}"#,
        ]));
        assert!(ok, "next failed: {out}");
        assert_eq!(
            next["state"], "plan_context_injection",
            "{issue_source:?}: {out}"
        );

        let log = events(dir.path(), "wf");
        let taken = of_type(&log, "transitioned").into_iter().last().unwrap();
        match issue_source {
            Some(_) => {
                assert_eq!(taken["payload"]["condition_type"], "skip_if");
                assert_eq!(
                    taken["payload"]["vars_matched"],
                    serde_json::json!({"ISSUE_SOURCE": "plan_outline"})
                );
            }
            None => {
                assert_eq!(taken["payload"]["condition_type"], "auto");
                assert!(taken["payload"].get("vars_matched").is_none());
            }
        }
    }
}
