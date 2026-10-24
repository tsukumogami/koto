//! koto v0.23.0, the last release before pins, keeps a session this build
//! pinned (PRD-session-pin.md, R26): its reader takes the peerless key
//! `koto/links/pin` for a malformed link, a problem that keeps the session's
//! hierarchy, so its prunes, its terminal tick and its same-name replace
//! leave the pinned session in place.
//!
//! Opt in by setting `KOTO_COMPAT_V0_23_BIN` to a koto v0.23.0 release
//! binary; without it the test passes having checked nothing, and says so.
//! The binary must match one of the SHA-256 digests in
//! `tests/fixtures/koto-v0.23.0.sha256`, the release's own `checksums.txt`.
//! CI's `pin-compat-v0-23` job downloads the release asset and sets the
//! variable, so a run that should have opted in and didn't shows there.
//! Both binaries run in a scratch HOME on the local backend, every
//! `KOTO_SESSION_*` variable removed.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tempfile::TempDir;

const BIN_VAR: &str = "KOTO_COMPAT_V0_23_BIN";

const WORK: &str = r#"---
name: pin-compat
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

struct Store {
    _tmp: TempDir,
    home: PathBuf,
    work: PathBuf,
    template: PathBuf,
}

impl Store {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let home = root.join("home");
        let work = root.join("work");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        let template = root.join("work.md");
        std::fs::write(&template, WORK).unwrap();
        Store {
            _tmp: tmp,
            home,
            work,
            template,
        }
    }

    fn setup(&self, cmd: &mut std::process::Command) {
        koto_command::scrub(cmd);
        cmd.current_dir(&self.work)
            .env("HOME", &self.home)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("KOTO_SESSIONS_BASE")
            .env_remove("KOTO_ANCHOR")
            .env_remove("KOTO_AUTHORITY")
            .env_remove("KOTO_DISPATCHER_ANCHOR")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env("KOTO_RETENTION_KEEP", "1s")
            .env("KOTO_RETENTION_IDLE_PERIOD", "1h")
            .env("KOTO_RETENTION_STALE_AFTER", "2h");
    }

    /// Run this build of koto, which must succeed.
    fn new_koto(&self, args: &[&str]) -> String {
        let mut cmd = koto_command::koto_std();
        self.setup(&mut cmd);
        let out = cmd.args(args).output().unwrap();
        assert!(
            out.status.success(),
            "koto {args:?}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Run the v0.23.0 binary at `old`, which must succeed.
    fn old_koto(&self, old: &Path, args: &[&str]) -> String {
        let mut cmd = std::process::Command::new(old);
        self.setup(&mut cmd);
        let out = cmd.args(args).output().unwrap();
        assert!(
            out.status.success(),
            "v0.23.0 koto {args:?}: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn pin_file(&self, name: &str) -> PathBuf {
        self.home
            .join(".koto/sessions")
            .join(name)
            .join("ctx/koto/links/pin")
    }
}

/// The v0.23.0 binary the run opted in with, checked against the release's
/// digests; `None` when the run didn't opt in.
fn old_binary() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os(BIN_VAR)?);
    let digests = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/koto-v0.23.0.sha256"),
    )
    .unwrap();
    let digest = hex::encode(Sha256::digest(std::fs::read(&path).unwrap()));
    assert!(
        digests
            .lines()
            .filter_map(|l| l.split_whitespace().next())
            .any(|d| d == digest),
        "{} (sha256 {digest}) is not a koto v0.23.0 release binary",
        path.display()
    );
    Some(path)
}

#[test]
fn koto_v0_23_keeps_a_session_this_build_pinned() {
    let Some(old) = old_binary() else {
        eprintln!("{BIN_VAR} is not set; the v0.23.0 compatibility check did not run");
        return;
    };
    let s = Store::new();
    let template = s.template.to_str().unwrap().to_string();
    let done = r#"{"marker":"done"}"#;
    // X runs with its koto child P, which finishes and is pinned by this
    // build; U finishes on its own.
    s.new_koto(&["init", "X", "--template", &template]);
    s.new_koto(&["init", "P", "--template", &template, "--parent", "X"]);
    s.new_koto(&["next", "P", "--with-data", done, "--no-cleanup"]);
    s.new_koto(&["init", "U", "--template", &template]);
    s.new_koto(&["next", "U", "--with-data", done, "--no-cleanup"]);
    s.new_koto(&["session", "pin", "P", "--reason", "kept for v0.23.0"]);
    let pin = std::fs::read(s.pin_file("P")).unwrap();

    // v0.23.0's terminal tick of X keeps X and its child.
    s.old_koto(&old, &["next", "X", "--with-data", done]);
    assert_eq!(std::fs::read(s.pin_file("P")).unwrap(), pin);

    // Its prune keeps P, naming the pin's record as what it can't read,
    // and removes U.
    std::thread::sleep(std::time::Duration::from_millis(1300));
    let pruned = s.old_koto(&old, &["session", "prune", "--apply", "--all"]);
    assert!(pruned.contains("koto/links/pin"), "{pruned}");
    let v: serde_json::Value = serde_json::from_str(&pruned).unwrap();
    let removed: Vec<&str> = v["removed"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert!(!removed.contains(&"P"), "{pruned}");
    assert!(removed.contains(&"U"), "{pruned}");
    assert_eq!(std::fs::read(s.pin_file("P")).unwrap(), pin);

    // Its workspace prune keeps P.
    s.old_koto(&old, &["workspace", "prune", "--root", "X", "--yes"]);
    assert_eq!(std::fs::read(s.pin_file("P")).unwrap(), pin);

    // Its same-name replace moves P aside, pin and all.
    let replaced = s.old_koto(
        &old,
        &["init", "P", "--template", &template, "--replace-terminal"],
    );
    let v: serde_json::Value = serde_json::from_str(replaced.trim()).unwrap();
    let moved = v["moved_to"]
        .as_str()
        .unwrap_or_else(|| panic!("{replaced}"));
    assert_eq!(std::fs::read(s.pin_file(moved)).unwrap(), pin);
    assert!(!s.pin_file("P").exists());
}
