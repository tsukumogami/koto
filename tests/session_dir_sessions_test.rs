//! `koto session dir --json` prints the sessions directory koto uses, from
//! the rule `koto_session_reader::source::sessions_dir` owns: `<root>/sessions`,
//! or `KOTO_SESSIONS_BASE` when that is set on the local backend. Computing
//! it creates nothing, so the root still doesn't exist afterwards.
//!
//! The rule's cases without the environment are the crate's unit tests; this
//! file runs the binary, so the environment is the real one.

#![cfg(unix)]

#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;

use std::path::{Path, PathBuf};

use serde_json::Value;
use tempfile::TempDir;

struct Env {
    _tmp: TempDir,
    home: PathBuf,
    koto_home: PathBuf,
    work: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let env = Env {
            home: root.join("home"),
            koto_home: root.join("koto-home"),
            work: root.join("work"),
            _tmp: tmp,
        };
        for d in [&env.home, &env.work] {
            std::fs::create_dir_all(d).unwrap();
        }
        env
    }

    /// `koto session dir --json` with `HOME` and `KOTO_HOME` at scratch
    /// directories and every variable that could move the store removed,
    /// then `extra` set.
    fn report(&self, extra: &[(&str, &Path)]) -> Value {
        let mut cmd = koto_command::koto();
        cmd.current_dir(&self.work)
            .env("HOME", &self.home)
            .env("KOTO_HOME", &self.koto_home)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("KOTO_SESSIONS_BASE")
            .env_remove("KOTO_WORKFLOWS_DIR")
            .env_remove("KOTO_ANCHOR")
            .env_remove("KOTO_DISPATCHER_ANCHOR")
            .env_remove("KOTO_TICK_SESSION")
            .env_remove("CLAUDE_CODE_SESSION_ID");
        for (k, v) in extra {
            cmd.env(k, v);
        }
        let out = cmd.args(["session", "dir", "--json"]).output().unwrap();
        assert!(
            out.status.success(),
            "koto session dir --json failed\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
}

#[test]
fn sessions_is_under_the_scratch_root() {
    let env = Env::new();
    let report = env.report(&[]);
    assert_eq!(
        report["sessions"],
        env.koto_home.join("sessions").to_str().unwrap(),
        "{report}"
    );
    assert_eq!(report["root"], env.koto_home.to_str().unwrap());
    assert_eq!(report["backend"], "local");
    assert_eq!(report["backend_source"], "default");
    assert!(report.get("bucket").is_none(), "{report}");
    assert!(!env.koto_home.exists(), "the report wrote the root");
}

#[test]
fn sessions_base_moves_sessions_on_the_local_backend() {
    let env = Env::new();
    let base = env.work.join("moved-sessions");
    let report = env.report(&[("KOTO_SESSIONS_BASE", &base)]);
    assert_eq!(report["sessions"], base.to_str().unwrap(), "{report}");
    assert_eq!(report["root"], env.koto_home.to_str().unwrap());
    assert_eq!(report["backend"], "local");
    assert!(!env.koto_home.exists(), "the report wrote the root");
    assert!(!base.exists(), "the report wrote the sessions directory");
}
