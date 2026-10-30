//! Every command in koto's verification map runs through
//! `scripts/without-session-env.sh`, which removes `KOTO_SESSIONS_BASE` and
//! every `KOTO_SESSION_*` variable before the command starts and refuses to
//! start it while any is still set. The koto driving a `/work-on` run sets
//! `KOTO_SESSIONS_BASE` on every command its gate starts, so a suite started
//! there would otherwise run against that run's store.
//!
//! The first test reads the map and fails on a command that doesn't start
//! with the wrapper. The others run the wrapper with the variables set to
//! scratch values: once to show the command it starts sees none of them,
//! and once with `--check`, the step that guards the command, to show it
//! refuses and names the variable without starting the command.

#[path = "support/test_home.rs"]
mod test_home;

use std::path::Path;
use std::process::Command;

const WRAPPER: &str = "scripts/without-session-env.sh";

/// A name the shell can't hold as a variable, which a shell-only scrub
/// would miss.
const ODD_NAME: &str = "KOTO_SESSION_ODD.NAME";

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn wrapper(args: &[&str]) -> Command {
    let mut cmd = Command::new("bash");
    cmd.current_dir(root()).arg(WRAPPER).args(args);
    cmd
}

#[test]
fn every_map_command_runs_through_the_wrapper() {
    let path = root().join(".claude/shirabe-extensions/verification-map.json");
    let map: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let commands = map["commands"].as_object().expect("the map has commands");
    assert!(!commands.is_empty(), "the map has no commands");
    let unwrapped: Vec<String> = commands
        .iter()
        .filter(|(_, command)| {
            let run: Vec<&str> = command["run"]
                .as_array()
                .map(|run| run.iter().filter_map(|arg| arg.as_str()).collect())
                .unwrap_or_default();
            run.len() < 3 || run[..2] != ["bash", WRAPPER]
        })
        .map(|(id, command)| format!("{id}: {}", command["run"]))
        .collect();
    assert!(
        unwrapped.is_empty(),
        "these verification map commands run with the session store variables \
         of the koto that started them; start each with \"bash\", \"{WRAPPER}\":\n  {}",
        unwrapped.join("\n  ")
    );
}

#[test]
fn the_wrapper_starts_its_command_with_no_store_variable() {
    let scratch = tempfile::tempdir().unwrap();
    let out = wrapper(&["sh", "-c", "env | cut -d= -f1"])
        .env("KOTO_SESSIONS_BASE", scratch.path().join("sessions"))
        .env("KOTO_SESSION_BACKEND", "local")
        .env(ODD_NAME, "scratch")
        .env("KOTO_TEST_WRAPPER_KEPT", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "the wrapper failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let names = String::from_utf8_lossy(&out.stdout);
    let names: Vec<&str> = names.lines().collect();
    assert!(
        names.contains(&"KOTO_TEST_WRAPPER_KEPT"),
        "the command didn't see a variable the wrapper leaves alone, so an \
         empty result proves nothing: {names:?}"
    );
    let left: Vec<&&str> = names
        .iter()
        .filter(|name| **name == "KOTO_SESSIONS_BASE" || name.starts_with("KOTO_SESSION_"))
        .collect();
    assert!(
        left.is_empty(),
        "the command the wrapper started still has {left:?}"
    );
}

#[test]
fn the_check_refuses_a_set_variable_without_starting_the_command() {
    for name in ["KOTO_SESSIONS_BASE", "KOTO_SESSION_BACKEND", ODD_NAME] {
        let scratch = tempfile::tempdir().unwrap();
        let ran = scratch.path().join("ran");
        let out = wrapper(&["--check", "touch", ran.to_str().unwrap()])
            .env(name, scratch.path().join("sessions"))
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            out.status.code(),
            Some(1),
            "the check didn't refuse with {name} set: {stderr}"
        );
        assert!(
            stderr.contains(&format!("{name} is set")),
            "the refusal doesn't name {name}: {stderr}"
        );
        assert!(
            !ran.exists(),
            "the check started the command with {name} set"
        );
    }
}

/// An `env` that can't list the environment with `-0` must stop the
/// wrapper, not read as an environment with nothing to remove.
#[cfg(unix)]
#[test]
fn the_wrapper_refuses_when_it_cannot_list_the_environment() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = tempfile::tempdir().unwrap();
    let bin = scratch.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let fake_env = bin.join("env");
    std::fs::write(
        &fake_env,
        "#!/bin/sh\n[ \"$1\" = -0 ] && exit 1\nexec /usr/bin/env \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake_env, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    for args in [&["--check"][..], &[][..]] {
        let ran = scratch.path().join("ran");
        let mut full: Vec<&str> = args.to_vec();
        full.extend(["touch", ran.to_str().unwrap()]);
        let out = wrapper(&full)
            .env("PATH", &path)
            .env("KOTO_SESSIONS_BASE", scratch.path().join("sessions"))
            .output()
            .unwrap();
        assert!(
            !out.status.success(),
            "the wrapper ({args:?}) succeeded though it couldn't list the environment"
        );
        assert!(
            !ran.exists(),
            "the wrapper ({args:?}) started the command though it couldn't list the environment"
        );
    }
}

#[test]
fn the_check_starts_the_command_when_none_is_set() {
    let scratch = tempfile::tempdir().unwrap();
    let ran = scratch.path().join("ran");
    let out = wrapper(&["--check", "touch", ran.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        out.status.success() && ran.exists(),
        "the check didn't start the command: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
