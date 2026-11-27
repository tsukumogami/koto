//! `run_journal.enabled`, driven through the real `koto` binary in a scratch
//! HOME. The key is on by default; `false` in user config stops every write
//! to the run journal and changes nothing else about a session.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;
#[path = "support/project_trust.rs"]
mod project_trust;

use std::path::PathBuf;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

const JOURNAL: &str = "_run_journal.jsonl";

/// `work` runs a command gate, loops on `again: yes` and ends on `no`, so a
/// session's commands write every family of record: identity, checks,
/// responses, evidence and a terminal.
const TEMPLATE: &str = r#"---
name: enabled-loop
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      again:
        type: enum
        required: true
        values: ["yes", "no"]
    gates:
      lint:
        type: command
        command: "exit 0"
    transitions:
      - target: work
        when:
          again: "yes"
      - target: done
        when:
          again: "no"
  done:
    terminal: true
---

## work

Work.

## done

Done.
"#;

struct Env {
    _tmp: TempDir,
    home: PathBuf,
    work: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let home = root.join("home");
        let work = root.join("work");
        for d in [&home, &work] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(work.join("template.md"), TEMPLATE).unwrap();
        Env {
            _tmp: tmp,
            home,
            work,
        }
    }

    fn koto_home(&self) -> PathBuf {
        self.home.join(".koto")
    }

    fn journal_path(&self) -> PathBuf {
        self.koto_home().join(JOURNAL)
    }

    fn cmd(&self) -> Command {
        let mut cmd = koto_command::koto();
        cmd.current_dir(&self.work)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", self.home.join("cache"))
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("KOTO_HOME")
            .env_remove("KOTO_SESSIONS_BASE")
            .env_remove("KOTO_WORKFLOWS_DIR")
            .env_remove("KOTO_ANCHOR")
            .env_remove("CLAUDE_CODE_SESSION_ID");
        cmd
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        self.cmd().args(args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "koto {args:?}: {}\n{stderr}",
            String::from_utf8_lossy(&out.stdout),
        );
        // A journal that is off is silent: no warning about a file that
        // was never opened.
        assert!(!stderr.contains("run journal"), "koto {args:?}: {stderr}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn set_enabled(&self, value: &str) {
        self.ok(&["config", "set", "run_journal.enabled", value, "--user"]);
    }

    /// A whole session: init, a tick that runs the gate and waits, a
    /// submission that loops, and one that reaches the terminal state.
    fn run_session(&self, name: &str) {
        self.ok(&["init", name, "--template", "template.md"]);
        self.ok(&["next", name, "--no-cleanup"]);
        self.ok(&[
            "next",
            name,
            "--with-data",
            r#"{"again":"yes"}"#,
            "--no-cleanup",
        ]);
        let out = self.ok(&[
            "next",
            name,
            "--with-data",
            r#"{"again":"no"}"#,
            "--no-cleanup",
        ]);
        let json: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(json["action"], "done", "{json}");
    }

    /// The journal's records for `session`; none when the file isn't there.
    fn records(&self, session: &str) -> Vec<Value> {
        std::fs::read_to_string(self.journal_path())
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .filter(|r| r["session"] == session)
            .collect()
    }

    /// The names of the files directly under the koto home.
    fn koto_home_files(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.koto_home())
            .unwrap()
            .map(|e| e.unwrap())
            .filter(|e| e.path().is_file())
            .map(|e| e.file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }
}

#[test]
fn the_journal_is_written_by_default() {
    let env = Env::new();
    assert_eq!(
        env.ok(&["config", "get", "run_journal.enabled"]).trim(),
        "true"
    );
    env.run_session("on");
    let kinds: Vec<String> = env
        .records("on")
        .iter()
        .map(|r| r["kind"].as_str().unwrap().to_string())
        .collect();
    for kind in ["session_started", "check_evaluated", "response", "terminal"] {
        assert!(kinds.iter().any(|k| k == kind), "no {kind} in {kinds:?}");
    }
}

#[test]
fn disabled_in_user_config_a_sessions_commands_write_nothing_to_the_journal() {
    let env = Env::new();
    env.set_enabled("false");
    assert_eq!(
        env.ok(&["config", "get", "run_journal.enabled"]).trim(),
        "false"
    );

    env.run_session("off");
    env.ok(&["status", "off"]);
    env.ok(&["rewind", "off"]);
    env.ok(&["cancel", "off"]);

    // No journal file, no lock for one, and no rotated or reader file: the
    // koto home holds nothing whose name starts with the journal's.
    assert!(!env.journal_path().exists(), "the journal was created");
    let stray: Vec<String> = env
        .koto_home_files()
        .into_iter()
        .filter(|n| n.starts_with("_run_journal"))
        .collect();
    assert!(stray.is_empty(), "{stray:?}");

    // The session itself is untouched by the setting: it ran to its
    // terminal state, and its sidecar is still kept.
    let sidecar = env
        .koto_home()
        .join("sessions")
        .join("off")
        .join("run-journal.json");
    assert!(sidecar.is_file(), "the session sidecar is missing");
}

#[test]
fn turning_the_journal_off_keeps_what_it_held_and_turning_it_on_resumes() {
    let env = Env::new();
    env.run_session("first");
    let before = std::fs::read(env.journal_path()).unwrap();
    assert!(!before.is_empty());

    env.set_enabled("false");
    env.run_session("second");
    assert_eq!(
        std::fs::read(env.journal_path()).unwrap(),
        before,
        "a command wrote to the journal while it was off"
    );
    assert!(env.records("second").is_empty());

    env.set_enabled("true");
    env.run_session("third");
    let after = std::fs::read(env.journal_path()).unwrap();
    assert!(after.starts_with(&before), "earlier lines changed");
    assert!(!env.records("third").is_empty());
    assert!(env.records("second").is_empty());

    // Unsetting the key is the default again.
    env.set_enabled("false");
    env.ok(&["config", "unset", "run_journal.enabled", "--user"]);
    env.run_session("fourth");
    assert!(!env.records("fourth").is_empty());
}

#[test]
fn a_project_config_cannot_turn_the_journal_off() {
    let env = Env::new();
    // `koto config set` without --user refuses the key...
    let out = env.run(&["config", "set", "run_journal.enabled", "false"]);
    assert!(!out.status.success());
    // ...and a hand-written project file is ignored for it, even trusted.
    std::fs::create_dir_all(env.work.join(".koto")).unwrap();
    std::fs::write(
        env.work.join(".koto/config.toml"),
        "[run_journal]\nenabled = false\n",
    )
    .unwrap();
    project_trust::trust(&env.home.join(".koto"), &env.work);
    assert_eq!(
        env.ok(&["config", "get", "run_journal.enabled"]).trim(),
        "true"
    );
    env.run_session("project");
    assert!(!env.records("project").is_empty());
}

#[test]
fn a_value_that_is_not_a_boolean_is_refused() {
    let env = Env::new();
    for bad in ["0", "off", "False"] {
        let out = env.run(&["config", "set", "run_journal.enabled", bad, "--user"]);
        assert!(!out.status.success(), "{bad} was accepted");
    }
    assert_eq!(
        env.ok(&["config", "get", "run_journal.enabled"]).trim(),
        "true"
    );
}
