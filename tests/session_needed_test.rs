//! `koto session needed <PATH>` through the real binary on the local
//! backend: which sessions anchored under a path keep it needed, how the
//! continues relation releases a path a run was taken away from, path
//! matching, the unreadable cases, and that the verb writes nothing.
//!
//! Every test builds its sessions in its own temporary directory, under a
//! scratch `HOME` and `KOTO_HOME`, with every `KOTO_SESSION_*` name,
//! `KOTO_SESSIONS_BASE` and `CLAUDE_CODE_SESSION_ID` removed, and checks
//! that koto resolves its store under that directory before running
//! anything else.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const TEMPLATE: &str = r#"---
name: needed-test
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

struct Env {
    _tmp: TempDir,
    root: PathBuf,
    home: PathBuf,
    koto_home: PathBuf,
    cache: PathBuf,
    template: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let home = root.join("home");
        let koto_home = root.join("koto-home");
        let cache = root.join("cache");
        for d in [&home, &koto_home, &cache] {
            std::fs::create_dir_all(d).unwrap();
        }
        let template = root.join("needed-test.md");
        std::fs::write(&template, TEMPLATE).unwrap();
        let env = Env {
            _tmp: tmp,
            root,
            home,
            koto_home,
            cache,
            template,
        };
        env.assert_scratch_store();
        env
    }

    /// koto resolves its root to this test's `KOTO_HOME`, inside its own
    /// temporary directory, so no command here reaches a real store.
    fn assert_scratch_store(&self) {
        let out = self
            .cmd(&self.root)
            .args(["session", "dir"])
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let resolved = PathBuf::from(String::from_utf8(out.stdout).unwrap().trim());
        assert_eq!(resolved, self.koto_home);
        assert!(resolved.starts_with(&self.root));
    }

    fn cmd(&self, cwd: &Path) -> Command {
        let mut cmd = koto_command::koto();
        cmd.current_dir(cwd)
            .env("HOME", &self.home)
            .env("KOTO_HOME", &self.koto_home)
            .env("XDG_CACHE_HOME", &self.cache)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("KOTO_SESSIONS_BASE")
            .env_remove("KOTO_WORKFLOWS_DIR")
            .env_remove("KOTO_ANCHOR")
            .env_remove("KOTO_AUTHORITY")
            .env_remove("KOTO_DISPATCHER_ANCHOR")
            .env_remove("CLAUDE_CODE_SESSION_ID");
        cmd
    }

    fn sessions(&self) -> PathBuf {
        self.koto_home.join("sessions")
    }

    /// A directory under the test's root, created.
    fn dir(&self, rel: &str) -> PathBuf {
        let d = self.root.join(rel);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn ok(&self, cwd: &Path, args: &[&str]) {
        let out = self.cmd(cwd).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "koto {args:?} failed ({:?}): {}\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Create session `name` anchored at `at`, with `extra` init flags.
    fn init(&self, name: &str, at: &Path, extra: &[&str]) {
        let mut args = vec!["init", name, "--template", self.template.to_str().unwrap()];
        args.extend_from_slice(extra);
        self.ok(at, &args);
    }

    /// Move session `name`, anchored at `at`, to its terminal state, keeping
    /// it in the store.
    fn finish(&self, name: &str, at: &Path) {
        self.ok(
            at,
            &[
                "next",
                name,
                "--with-data",
                r#"{"go": "yes"}"#,
                "--no-cleanup",
            ],
        );
    }

    fn cancel(&self, name: &str) {
        self.ok(&self.root, &["cancel", name]);
    }

    /// Run the verb on `path` and return its exit code and output line.
    fn needed(&self, path: &Path) -> (i32, String) {
        let out = self
            .cmd(&self.root)
            .args(["session", "needed", path.to_str().unwrap()])
            .output()
            .unwrap();
        let code = out.status.code().expect("exit code");
        let text = if code == 2 { out.stderr } else { out.stdout };
        let text = String::from_utf8(text).unwrap();
        let line = text.trim_end_matches('\n');
        assert!(!line.contains('\n'), "more than one line: {text:?}");
        assert!(line.len() <= 200, "line longer than 200 bytes: {line:?}");
        (code, line.to_string())
    }

    fn state_file(&self, name: &str) -> PathBuf {
        self.sessions()
            .join(name)
            .join(format!("koto-{name}.state.jsonl"))
    }

    /// Rewrite session `name`'s header without its execution anchor.
    fn drop_anchor(&self, name: &str) {
        let path = self.state_file(name);
        let text = std::fs::read_to_string(&path).unwrap();
        let (header, rest) = text.split_once('\n').unwrap();
        let mut header: serde_json::Value = serde_json::from_str(header).unwrap();
        header.as_object_mut().unwrap().remove("execution_dir");
        std::fs::write(&path, format!("{header}\n{rest}")).unwrap();
    }

    /// Make session `name`'s log unparseable past its header.
    fn corrupt_log(&self, name: &str) {
        let path = self.state_file(name);
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("this is not an event\n");
        std::fs::write(&path, text).unwrap();
    }

    /// SHA-256 of every file under the koto root, with its mode, plus every
    /// directory, so an added, removed or changed entry shows.
    fn store_hash(&self) -> BTreeMap<PathBuf, String> {
        let mut out = BTreeMap::new();
        let mut stack = vec![self.koto_home.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                let meta = std::fs::symlink_metadata(&p).unwrap();
                let rel = p.strip_prefix(&self.koto_home).unwrap().to_path_buf();
                if meta.is_dir() {
                    out.insert(rel, format!("dir {:o}", meta.permissions().mode()));
                    stack.push(p);
                } else {
                    let digest = Sha256::digest(std::fs::read(&p).unwrap());
                    out.insert(
                        rel,
                        format!("{:o} {}", meta.permissions().mode(), hex::encode(digest)),
                    );
                }
            }
        }
        out
    }
}

#[test]
fn a_running_session_needs_its_path_until_continued_elsewhere() {
    let env = Env::new();
    let repo = env.dir("a/repo");
    let other = env.dir("b/other");
    env.init("v1", &repo, &[]);

    let (code, line) = env.needed(&repo);
    assert_eq!(code, 1, "{line}");
    assert!(line.contains("v1"), "{line}");
    assert!(line.contains("running"), "{line}");

    env.init("v2", &other, &["--continues", "v1"]);
    assert_eq!(env.needed(&repo).0, 0);
}

#[test]
fn a_terminal_or_cancelled_session_needs_nothing() {
    let env = Env::new();
    let done = env.dir("done");
    let gone = env.dir("gone");
    env.init("finished", &done, &[]);
    env.init("cancelled", &gone, &[]);
    assert_eq!(env.needed(&done).0, 1);
    assert_eq!(env.needed(&gone).0, 1);

    env.finish("finished", &done);
    env.cancel("cancelled");
    assert_eq!(env.needed(&done), (0, "not needed".to_string()));
    assert_eq!(env.needed(&gone).0, 0);
}

#[test]
fn a_later_member_outside_the_path_releases_the_whole_lineage_under_it() {
    let env = Env::new();
    let repo = env.dir("repo");
    let sub = env.dir("repo/sub");
    let elsewhere = env.dir("elsewhere");
    env.init("v1", &repo, &[]);
    env.init("v2", &sub, &["--continues", "v1"]);
    env.init("v3", &elsewhere, &["--continues", "v2"]);
    assert_eq!(env.needed(&repo).0, 0);
}

#[test]
fn a_running_last_member_under_the_path_keeps_it() {
    let env = Env::new();
    let repo = env.dir("repo");
    let sub = env.dir("repo/sub");
    env.init("v1", &repo, &[]);
    env.init("v2", &repo, &["--continues", "v1"]);
    env.init("v3", &sub, &["--continues", "v2"]);
    let (code, line) = env.needed(&repo);
    assert_eq!(code, 1, "{line}");
    // The session whose name sorts first is named, with the count of others.
    assert!(line.contains("v1"), "{line}");
    assert!(
        line.starts_with("needed (3 sessions): session v1 "),
        "{line}"
    );
}

#[test]
fn a_successor_with_no_recorded_anchor_releases_nothing() {
    let env = Env::new();
    let repo = env.dir("repo");
    let elsewhere = env.dir("elsewhere");
    env.init("v1", &repo, &[]);
    env.init("v2", &elsewhere, &["--continues", "v1"]);
    assert_eq!(env.needed(&repo).0, 0);

    env.drop_anchor("v2");
    let (code, line) = env.needed(&repo);
    assert_eq!(code, 1, "{line}");
    assert!(line.contains("v1"), "{line}");
}

#[test]
fn a_terminal_successor_outside_the_path_still_releases_it() {
    let env = Env::new();
    let repo = env.dir("repo");
    let elsewhere = env.dir("elsewhere");
    env.init("v1", &repo, &[]);
    env.init("v2", &elsewhere, &["--continues", "v1"]);
    env.finish("v2", &elsewhere);
    assert_eq!(env.needed(&repo).0, 0);
}

#[test]
fn paths_compare_by_whole_components_after_resolving_symlinks() {
    let env = Env::new();
    let repo = env.dir("a/repo");
    let repo2 = env.dir("a/repo-2");
    env.init("next-door", &repo2, &[]);
    assert_eq!(env.needed(&repo).0, 0);
    assert_eq!(env.needed(&repo2).0, 1);

    let link = env.root.join("link-to-repo-2");
    std::os::unix::fs::symlink(&repo2, &link).unwrap();
    assert_eq!(env.needed(&link), env.needed(&repo2));
}

#[test]
fn a_session_anchored_above_the_path_does_not_count() {
    let env = Env::new();
    let top = env.dir("workspace");
    let instance = env.dir("workspace/instance");
    env.init("at-top", &top, &[]);
    assert_eq!(env.needed(&instance).0, 0);
    assert_eq!(env.needed(&top).0, 1);
}

#[test]
fn a_path_that_cannot_be_resolved_is_undecided() {
    let env = Env::new();
    let (code, line) = env.needed(&env.root.join("no/such/dir"));
    assert_eq!(code, 2, "{line}");
}

#[test]
fn an_unreadable_session_under_the_path_keeps_it_and_is_named() {
    let env = Env::new();
    let repo = env.dir("repo");
    env.init("a-broken", &repo, &[]);
    env.init("b-finished", &repo, &[]);
    env.finish("b-finished", &repo);
    env.init("c-running-elsewhere", &env.dir("elsewhere"), &[]);
    env.corrupt_log("a-broken");

    let (code, line) = env.needed(&repo);
    assert_eq!(code, 1, "{line}");
    assert!(line.contains("a-broken"), "{line}");
    assert!(line.contains("unreadable"), "{line}");
}

#[test]
fn a_session_whose_header_cannot_be_read_leaves_the_answer_undecided() {
    let env = Env::new();
    let repo = env.dir("repo");
    env.init("garbled", &repo, &[]);
    std::fs::write(env.state_file("garbled"), "not a header\n").unwrap();

    // Where it is anchored is unknown, so no path is ever answered 0.
    let (code, line) = env.needed(&repo);
    assert_eq!(code, 2, "{line}");
    assert!(line.contains("garbled"), "{line}");
    assert_eq!(env.needed(&env.dir("elsewhere")).0, 2);

    // A session that does need the path still answers 1.
    env.init("running", &repo, &[]);
    let (code, line) = env.needed(&repo);
    assert_eq!(code, 1, "{line}");
    assert!(line.contains("session running is running"), "{line}");
}

#[test]
fn a_store_that_cannot_be_listed_is_undecided() {
    let env = Env::new();
    let repo = env.dir("repo");
    env.init("v1", &repo, &[]);
    let sessions = env.sessions();
    std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o000)).unwrap();
    // A process that can still list it (root) can't exercise this.
    let listable = std::fs::read_dir(&sessions).is_ok();
    let got = env.needed(&repo);
    std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o700)).unwrap();
    if listable {
        return;
    }
    assert_eq!(got.0, 2, "{}", got.1);
}

#[test]
fn the_verb_writes_nothing_even_over_an_unreadable_session() {
    let env = Env::new();
    let repo = env.dir("repo");
    let elsewhere = env.dir("elsewhere");
    env.init("v1", &repo, &[]);
    env.init("v2", &elsewhere, &["--continues", "v1"]);
    env.init("broken", &repo, &[]);
    env.init("done", &repo, &[]);
    env.finish("done", &repo);
    env.corrupt_log("broken");

    let before = env.store_hash();
    for path in [&repo, &elsewhere, &env.root] {
        env.needed(path);
    }
    env.needed(&env.root.join("no/such/dir"));
    assert_eq!(env.store_hash(), before);
}
