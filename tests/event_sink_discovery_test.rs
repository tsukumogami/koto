//! Which event sinks koto runs, driven through the koto binary.
//!
//! The sink is `$HOME/.local/libexec/fleet/event-sink`. koto calls it only
//! when it is a regular file the user owns, executable, writable by nobody
//! else, under directories that are the user's or root's and writable by
//! nobody else. Anything short of that means no call at all, and the
//! command behaves as it does with no sink. `FLEET_EVENT_SINK=off` turns
//! sending off; no other value of it is read.
//!
//! Each test builds a scratch home under the build directory
//! (`tests/support/event_sink.rs`), installs the stub sink, breaks one
//! thing, runs a short session and counts the stub's calls. The owner check
//! can't be exercised here without `chown`; it is unit-tested through
//! `trusted(meta, uid)` in `src/telemetry/sink.rs`.

#![cfg(unix)]

#[path = "support/event_sink.rs"]
mod event_sink;
#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;

use event_sink::{SinkHome, Stub};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const TEMPLATE: &str = r#"---
name: sink-discovery
version: "1.0"
initial_state: start
states:
  start:
    accepts:
      go:
        type: enum
        required: true
        values: ["yes"]
    transitions:
      - target: done
        when:
          go: "yes"
  done:
    terminal: true
---

## start

Start.

## done

Done.
"#;

fn set_mode(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// Run a short session to its terminal state with `cmd` building each koto
/// command, and return what the commands printed. Every command must
/// succeed whatever the sink is: a sink koto won't run changes nothing.
fn run_session(home: &SinkHome, cmd: &dyn Fn() -> assert_cmd::Command) -> Vec<String> {
    std::fs::write(home.work.join("template.md"), TEMPLATE).unwrap();
    let steps: [&[&str]; 3] = [
        &["init", "wf", "--template", "template.md"],
        &["next", "wf", "--no-cleanup"],
        &[
            "next",
            "wf",
            "--with-data",
            r#"{"go":"yes"}"#,
            "--no-cleanup",
        ],
    ];
    steps
        .iter()
        .map(|args| {
            let out = cmd().args(*args).output().unwrap();
            assert!(
                out.status.success(),
                "koto {args:?}: {}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).into_owned()
        })
        .collect()
}

/// The session ran, journaled as usual, and the stub was never called.
fn assert_no_call(home: &SinkHome, stub: &Stub, what: &str) {
    assert_eq!(stub.call_count(), 0, "{what}: the sink was called");
    let kinds: Vec<String> = home
        .journal()
        .iter()
        .map(|r| r["kind"].as_str().unwrap().to_string())
        .collect();
    assert!(
        kinds.contains(&"session_started".to_string()) && kinds.contains(&"terminal".to_string()),
        "{what}: the journal is short: {kinds:?}"
    );
    // Nothing was queued, so nothing was counted as dropped either.
    assert!(
        !home.session_dir("wf").join("telemetry.json").exists(),
        "{what}: a drop count was written"
    );
}

#[test]
fn a_sink_that_passes_every_check_is_called() {
    let (home, stub) = SinkHome::with_stub("ok");
    run_session(&home, &|| home.koto());
    assert!(stub.call_count() > 0, "the sink was never called");
    let calls = stub.calls();
    assert!(calls
        .iter()
        .all(|c| c.argv == ["send", "--scope", "koto-journal"]));
    assert!(calls.iter().all(|c| c.done));
}

#[test]
fn no_sink_no_call() {
    // The fixed directory exists and holds nothing.
    let home = SinkHome::new("none");
    let stub = home.install_stub_at(&home.work.join("not-the-sink"));
    let outputs = run_session(&home, &|| home.koto());
    assert_no_call(&home, &stub, "no sink");
    assert!(outputs[2].contains("\"done\""), "{}", outputs[2]);

    // No directory at all.
    let bare = SinkHome::new("bare");
    std::fs::remove_dir_all(bare.home.join(".local")).unwrap();
    let stub = bare.install_stub_at(&bare.work.join("not-the-sink"));
    run_session(&bare, &|| bare.koto());
    assert_no_call(&bare, &stub, "no sink directory");
}

#[test]
fn symlink_refused() {
    let home = SinkHome::new("symlink");
    let real = home.sink_dir().join("real-sink");
    let stub = home.install_stub_at(&real);
    std::os::unix::fs::symlink(&real, home.sink_path()).unwrap();
    run_session(&home, &|| home.koto());
    assert_no_call(&home, &stub, "a symlink to a good sink");
}

#[test]
fn group_writable_refused() {
    let (home, stub) = SinkHome::with_stub("gw");
    set_mode(&home.sink_path(), 0o770);
    run_session(&home, &|| home.koto());
    assert_no_call(&home, &stub, "a group-writable sink");

    let (home, stub) = SinkHome::with_stub("ow");
    set_mode(&home.sink_path(), 0o702);
    run_session(&home, &|| home.koto());
    assert_no_call(&home, &stub, "a world-writable sink");
}

#[test]
fn not_executable_refused() {
    let (home, stub) = SinkHome::with_stub("noexec");
    set_mode(&home.sink_path(), 0o600);
    run_session(&home, &|| home.koto());
    assert_no_call(&home, &stub, "a sink with no execute bit");
}

#[test]
fn parent_writable_refused() {
    // The sink's own directory, then two directories further up.
    for (label, up) in [("fleet", 0), ("libexec", 1), ("home", 3)] {
        let (home, stub) = SinkHome::with_stub(&format!("parent-{label}"));
        let mut dir = home.sink_dir();
        for _ in 0..up {
            dir = dir.parent().unwrap().to_path_buf();
        }
        set_mode(&dir, 0o770);
        run_session(&home, &|| home.koto());
        assert_no_call(&home, &stub, &format!("a group-writable {label} directory"));
        // Put it back so the scratch home can be removed.
        set_mode(&dir, 0o700);
    }
}

#[test]
fn home_under_sticky_tmp_refused() {
    // A home whose own directories are owner-only, under a directory that
    // is world-writable and sticky, as `/tmp` is.
    let (home, _unused) = SinkHome::with_stub("sticky-base");
    let sticky = home.work.join("sticky");
    let inner = sticky.join("home");
    let fleet = inner.join(event_sink::SINK_DIR);
    std::fs::create_dir_all(&fleet).unwrap();
    let mut d = Some(fleet.clone());
    while let Some(dir) = d {
        if dir == sticky {
            break;
        }
        set_mode(&dir, 0o700);
        d = dir.parent().map(Path::to_path_buf);
    }
    set_mode(&sticky, 0o1777);
    let stub = home.install_stub_at(&fleet.join(event_sink::SINK_FILE));

    std::fs::write(home.work.join("template.md"), TEMPLATE).unwrap();
    let out = home
        .koto()
        .env("HOME", &inner)
        .args(["init", "wf", "--template", "template.md"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        stub.call_count(),
        0,
        "a sink under a sticky directory was called"
    );
    set_mode(&sticky, 0o700);
}

#[test]
fn off_disables() {
    let (home, stub) = SinkHome::with_stub("off");
    run_session(&home, &|| {
        let mut cmd = home.koto();
        cmd.env("FLEET_EVENT_SINK", "off");
        cmd
    });
    assert_no_call(&home, &stub, "FLEET_EVENT_SINK=off");
}

#[test]
fn other_values_ignored() {
    for value in ["", "0", "false", "no", "OFF", "on", "disabled"] {
        let (home, stub) = SinkHome::with_stub("other");
        run_session(&home, &|| {
            let mut cmd = home.koto();
            cmd.env("FLEET_EVENT_SINK", value);
            cmd
        });
        assert!(
            stub.call_count() > 0,
            "FLEET_EVENT_SINK={value:?} stopped the sink being called"
        );
    }
}

#[test]
fn a_relative_or_missing_home_means_no_sink() {
    let (home, stub) = SinkHome::with_stub("relhome");
    std::fs::write(home.work.join("template.md"), TEMPLATE).unwrap();
    // A relative HOME names no fixed path. koto's own store is pointed at
    // the scratch home through KOTO_HOME, so the command still has one.
    let out = home
        .koto()
        .env("HOME", "relative/home")
        .env("KOTO_HOME", home.koto_home())
        .args(["init", "wf", "--template", "template.md"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stub.call_count(), 0, "a relative HOME found a sink");
}
