//! `koto session identity <name> [--json]`: a session's id, its run's id
//! and its run's template, printed under the attribute names the session's
//! records carry.
//!
//! Every test runs the koto binary with `HOME` in its own temporary
//! directory and `KOTO_SESSIONS_BASE` unset, so the store is the default one
//! under that home. What the command prints is checked against the
//! session's header, read from its state file, and against the
//! `session_started` record the run journal holds for the same session.

#![cfg(unix)]

#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const ROOT_TEMPLATE: &str = r#"---
name: identity-root
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

/// A different template, so a child that printed its own template instead
/// of its root's would be caught.
const CHILD_TEMPLATE: &str = r#"---
name: identity-child
version: "1.0"
initial_state: work
states:
  work:
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

## work

Work.

## done

Done.
"#;

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Out {
    fn json(&self) -> Value {
        serde_json::from_str(self.stdout.trim())
            .unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {}", self.describe()))
    }

    fn describe(&self) -> String {
        format!(
            "exit {:?}\nstdout: {}\nstderr: {}",
            self.code, self.stdout, self.stderr
        )
    }
}

struct Env {
    _tmp: TempDir,
    home: PathBuf,
    work: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let work = tmp.path().join("work");
        for d in [&home, &work] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(work.join("root.md"), ROOT_TEMPLATE).unwrap();
        std::fs::write(work.join("child.md"), CHILD_TEMPLATE).unwrap();
        Env {
            _tmp: tmp,
            home,
            work,
        }
    }

    fn koto(&self, args: &[&str]) -> Out {
        let mut cmd = koto_command::koto();
        cmd.current_dir(&self.work)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", self.home.join("cache"));
        for unset in [
            "KOTO_SESSIONS_BASE",
            "KOTO_HOME",
            "XDG_CONFIG_HOME",
            "KOTO_WORKFLOWS_DIR",
            "KOTO_ANCHOR",
            "CLAUDE_CODE_SESSION_ID",
        ] {
            cmd.env_remove(unset);
        }
        let output = cmd.args(args).output().unwrap();
        Out {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        }
    }

    fn ok(&self, args: &[&str]) -> Out {
        let out = self.koto(args);
        assert_eq!(out.code, Some(0), "koto {args:?}: {}", out.describe());
        out
    }

    fn sessions(&self) -> PathBuf {
        self.home.join(".koto").join("sessions")
    }

    fn session_dir(&self, name: &str) -> PathBuf {
        self.sessions().join(name)
    }

    /// The session's header: the first line of its state file.
    fn header(&self, name: &str) -> Value {
        let path = self
            .session_dir(name)
            .join(format!("koto-{name}.state.jsonl"));
        let text = std::fs::read_to_string(path).unwrap();
        serde_json::from_str(text.lines().next().unwrap()).unwrap()
    }

    /// The `session_started` record the journal holds for `name`.
    fn started(&self, name: &str) -> Value {
        let journal =
            std::fs::read_to_string(self.home.join(".koto").join("_run_journal.jsonl")).unwrap();
        journal
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .find(|r| r["kind"] == "session_started" && r["session"] == name)
            .unwrap_or_else(|| panic!("no session_started for {name}"))
    }

    fn identity(&self, name: &str) -> Value {
        self.ok(&["session", "identity", name, "--json"]).json()
    }
}

/// Every file under `dir` with its bytes, for a before-and-after comparison.
fn tree(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    out
}

fn keys(value: &Value) -> Vec<&str> {
    value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

#[test]
fn root_identity_matches_header() {
    let env = Env::new();
    env.ok(&["init", "root", "--template", "root.md"]);
    let header = env.header("root");
    let started = env.started("root");

    let id = env.identity("root");
    assert_eq!(
        keys(&id),
        vec![
            "koto.run.id",
            "koto.run.template.hash",
            "koto.run.template.name",
            "koto.session.id"
        ],
        "{id}"
    );
    // A root is its own run: both ids are the header's `session_id`, and
    // the run's template is the header's.
    assert_eq!(id["koto.session.id"], header["session_id"]);
    assert_eq!(id["koto.run.id"], header["session_id"]);
    assert_eq!(id["koto.run.template.name"], "identity-root");
    assert_eq!(id["koto.run.template.name"], header["template_name"]);
    assert_eq!(id["koto.run.template.hash"], header["template_hash"]);
    // The same values the session's records carry.
    assert_eq!(id["koto.session.id"], started["koto.session.id"]);
    assert_eq!(id["koto.run.id"], started["koto.run.id"]);
    assert_eq!(id["koto.run.template.name"], started["koto.template.name"]);
    assert_eq!(id["koto.run.template.hash"], started["koto.template.hash"]);
}

#[test]
fn child_carries_run_template() {
    let env = Env::new();
    env.ok(&["init", "root", "--template", "root.md"]);
    env.ok(&[
        "init",
        "root.kid",
        "--template",
        "child.md",
        "--parent",
        "root",
    ]);
    env.ok(&[
        "init",
        "root.kid.grand",
        "--template",
        "child.md",
        "--parent",
        "root.kid",
    ]);
    let root = env.header("root");

    for name in ["root.kid", "root.kid.grand"] {
        let header = env.header(name);
        assert_eq!(header["template_name"], "identity-child");
        assert_ne!(header["template_hash"], root["template_hash"]);

        let id = env.identity(name);
        assert_eq!(id["koto.session.id"], header["session_id"], "{name}");
        assert_ne!(id["koto.session.id"], root["session_id"], "{name}");
        assert_eq!(id["koto.run.id"], root["session_id"], "{name}");
        // The run's template is the root's, not the child's own.
        assert_eq!(id["koto.run.template.name"], "identity-root", "{name}");
        assert_eq!(
            id["koto.run.template.hash"], root["template_hash"],
            "{name}"
        );
        assert_eq!(id["koto.run.id"], env.started(name)["koto.run.id"]);
    }

    // The values are cached when the child is created, so they outlive the
    // root's local copy: with the root and the middle session gone, the
    // grandchild still names its run and its run's template.
    std::fs::remove_dir_all(env.session_dir("root")).unwrap();
    std::fs::remove_dir_all(env.session_dir("root.kid")).unwrap();
    let id = env.identity("root.kid.grand");
    assert_eq!(id["koto.run.id"], root["session_id"]);
    assert_eq!(id["koto.run.template.name"], "identity-root");
    assert_eq!(id["koto.run.template.hash"], root["template_hash"]);
}

#[test]
fn a_child_without_a_cache_resolves_while_its_root_is_here_and_omits_otherwise() {
    let env = Env::new();
    env.ok(&["init", "root", "--template", "root.md"]);
    env.ok(&[
        "init",
        "root.kid",
        "--template",
        "child.md",
        "--parent",
        "root",
    ]);
    let root = env.header("root");
    let kid = env.header("root.kid");

    // With no sidecar at all, as when it was lost, nothing is cached: the
    // walk to the root answers while the root is on this host.
    std::fs::remove_file(env.session_dir("root.kid").join("run-journal.json")).unwrap();
    let id = env.identity("root.kid");
    assert_eq!(id["koto.run.id"], root["session_id"]);
    assert_eq!(id["koto.run.template.name"], "identity-root");
    assert_eq!(id["koto.run.template.hash"], root["template_hash"]);
    assert!(
        !env.session_dir("root.kid")
            .join("run-journal.json")
            .exists(),
        "the command filled the cache"
    );

    // With the root gone too, the template is left out rather than
    // guessed, and the command still exits 0. The run id stays: the
    // child's header records it.
    std::fs::remove_dir_all(env.session_dir("root")).unwrap();
    let id = env.identity("root.kid");
    assert_eq!(keys(&id), vec!["koto.run.id", "koto.session.id"], "{id}");
    assert_eq!(id["koto.session.id"], kid["session_id"]);
    assert_eq!(id["koto.run.id"], root["session_id"]);
}

/// A child whose sidecar caches a run id and no run template, as one
/// created before the template was cached, gets the template stored by its
/// next record while its root is on this host. After that it answers
/// without the root.
#[test]
fn an_older_childs_sidecar_gains_the_run_template_on_its_next_record() {
    let env = Env::new();
    env.ok(&["init", "root", "--template", "root.md"]);
    env.ok(&[
        "init",
        "root.kid",
        "--template",
        "child.md",
        "--parent",
        "root",
    ]);
    let root = env.header("root");
    let sidecar = env.session_dir("root.kid").join("run-journal.json");
    let mut cached: Value =
        serde_json::from_str(&std::fs::read_to_string(&sidecar).unwrap()).unwrap();
    let fields = cached.as_object_mut().unwrap();
    fields.remove("run_template_name");
    fields.remove("run_template_hash");
    std::fs::write(&sidecar, cached.to_string()).unwrap();

    // Any command that journals a record for the child: here, the tick
    // that takes it to its terminal state, kept with --no-cleanup.
    env.ok(&[
        "next",
        "root.kid",
        "--with-data",
        r#"{"go":"yes"}"#,
        "--no-cleanup",
    ]);
    let cached: Value = serde_json::from_str(&std::fs::read_to_string(&sidecar).unwrap()).unwrap();
    assert_eq!(cached["run_template_name"], "identity-root", "{cached}");
    assert_eq!(cached["run_template_hash"], root["template_hash"]);

    std::fs::remove_dir_all(env.session_dir("root")).unwrap();
    let id = env.identity("root.kid");
    assert_eq!(id["koto.run.template.name"], "identity-root");
    assert_eq!(id["koto.run.template.hash"], root["template_hash"]);
}

/// A cached template isn't used when the sidecar's run id contradicts the
/// run id the header records.
#[test]
fn a_cached_template_for_another_run_is_not_printed() {
    let env = Env::new();
    env.ok(&["init", "root", "--template", "root.md"]);
    env.ok(&[
        "init",
        "root.kid",
        "--template",
        "child.md",
        "--parent",
        "root",
    ]);
    let root = env.header("root");
    let sidecar = env.session_dir("root.kid").join("run-journal.json");
    std::fs::write(
        &sidecar,
        r#"{"run_id":"another-run","driver":null,"run_template_name":"stale","run_template_hash":"abcd"}"#,
    )
    .unwrap();
    let id = env.identity("root.kid");
    // The template comes from the walk to the real root, not the cache.
    assert_eq!(id["koto.run.template.name"], "identity-root", "{id}");
    assert_eq!(id["koto.run.template.hash"], root["template_hash"]);
}

/// A session this host doesn't hold exits 2 and says so. A name that isn't
/// a valid session name exits 2 as well, with whatever the name check says.
#[test]
fn missing_session_exits_2() {
    let env = Env::new();
    env.ok(&["init", "root", "--template", "root.md"]);
    for (name, needle) in [("absent", "does not exist"), ("../etc", "")] {
        for args in [
            vec!["session", "identity", name, "--json"],
            vec!["session", "identity", name],
        ] {
            let out = env.koto(&args);
            assert_eq!(out.code, Some(2), "{args:?}: {}", out.describe());
            let json = out.json();
            assert_eq!(json["command"], "session identity", "{json}");
            let error = json["error"].as_str().unwrap();
            assert!(error.contains(needle), "{error}");
        }
    }
}

#[test]
fn an_unreadable_header_exits_3() {
    let env = Env::new();
    env.ok(&["init", "root", "--template", "root.md"]);
    let state = env.session_dir("root").join("koto-root.state.jsonl");
    std::fs::write(&state, "not a header\n").unwrap();
    let out = env.koto(&["session", "identity", "root", "--json"]);
    assert_eq!(out.code, Some(3), "{}", out.describe());
    assert_eq!(out.json()["command"], "session identity");
}

#[test]
fn the_command_writes_nothing_to_the_store() {
    let env = Env::new();
    env.ok(&["init", "root", "--template", "root.md"]);
    env.ok(&[
        "init",
        "root.kid",
        "--template",
        "child.md",
        "--parent",
        "root",
    ]);
    let koto_home = env.home.join(".koto");
    let before = tree(&koto_home);
    for name in ["root", "root.kid", "absent"] {
        env.koto(&["session", "identity", name, "--json"]);
        env.koto(&["session", "identity", name]);
    }
    assert!(before == tree(&koto_home), "the store changed");
}

#[test]
fn without_json_the_pairs_are_key_value_lines() {
    let env = Env::new();
    env.ok(&["init", "root", "--template", "root.md"]);
    let id = env.identity("root");
    let text = env.ok(&["session", "identity", "root"]).stdout;
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        vec![
            format!(
                "koto.session.id={}",
                id["koto.session.id"].as_str().unwrap()
            ),
            format!("koto.run.id={}", id["koto.run.id"].as_str().unwrap()),
            format!(
                "koto.run.template.name={}",
                id["koto.run.template.name"].as_str().unwrap()
            ),
            format!(
                "koto.run.template.hash={}",
                id["koto.run.template.hash"].as_str().unwrap()
            ),
        ]
    );
}

#[test]
fn a_root_sidecar_caches_no_run_template() {
    // A root answers from its header, so its sidecar holds the two fields
    // it always held and nothing else; a child's gains the run template.
    let env = Env::new();
    env.ok(&["init", "root", "--template", "root.md"]);
    env.ok(&[
        "init",
        "root.kid",
        "--template",
        "child.md",
        "--parent",
        "root",
    ]);
    let read = |name: &str| -> Value {
        let raw = std::fs::read_to_string(env.session_dir(name).join("run-journal.json")).unwrap();
        serde_json::from_str(&raw).unwrap()
    };
    assert_eq!(keys(&read("root")), vec!["driver", "run_id"]);
    let kid = read("root.kid");
    assert_eq!(
        keys(&kid),
        vec!["driver", "run_id", "run_template_hash", "run_template_name"]
    );
    assert_eq!(kid["run_template_name"], "identity-root");
    assert_eq!(
        kid["run_template_hash"],
        env.header("root")["template_hash"]
    );
}
