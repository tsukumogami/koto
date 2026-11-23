//! The supersede marker and the chain read under the local backend, through
//! the real binary: `koto init --supersede` writes `koto/superseded-by` in
//! the predecessor beside the fence, a re-run writes a missing one and
//! leaves a present one, no context writer reaches it, and `koto decisions
//! chain` follows it forward from each chain session, confirming each hop
//! against the successor's header, without writing anything.
//!
//! Every test runs the real binary with `HOME`, `KOTO_HOME` and
//! `KOTO_SESSIONS_BASE` pointed into its own temporary directory, and with
//! every variable koto reads an identity from removed.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tempfile::TempDir;

/// Variables koto reads an identity, a run mode or an authority from.
const IDENTITY_ENV: [&str; 5] = [
    "CLAUDE_CODE_SESSION_ID",
    "KOTO_ANCHOR",
    "KOTO_AGENT_ID",
    "KOTO_RUN_MODE",
    "KOTO_AUTHORITY",
];

const TEMPLATE: &str = r#"---
name: work
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      go:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: done
        when:
          go: go
  done:
    terminal: true
---

## work

Work.

## done

Done.
"#;

const MARKER: &str = "koto/superseded-by";

struct Env {
    _tmp: TempDir,
    root: PathBuf,
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Out {
    fn json(&self) -> Value {
        serde_json::from_str(self.stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON: {e}\n{}\n{}", self.stdout, self.stderr))
    }
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        for dir in ["home", "ws", "sessions"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("work.md"), TEMPLATE).unwrap();
        Env { _tmp: tmp, root }
    }

    fn sessions(&self) -> PathBuf {
        self.root.join("sessions")
    }

    fn run_stdin(&self, args: &[&str], stdin: Option<&str>) -> Out {
        let mut cmd = koto_command::koto();
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("KOTO_") {
                cmd.env_remove(name);
            }
        }
        for name in IDENTITY_ENV {
            cmd.env_remove(name);
        }
        cmd.current_dir(self.root.join("ws"))
            .env_remove("XDG_CONFIG_HOME")
            .env("HOME", self.root.join("home"))
            .env("KOTO_HOME", self.root.join("home"))
            .env("KOTO_SESSIONS_BASE", self.sessions())
            .args(args);
        if let Some(input) = stdin {
            cmd.write_stdin(input);
        }
        let output = cmd.output().unwrap();
        Out {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    fn run(&self, args: &[&str]) -> Out {
        self.run_stdin(args, None)
    }

    fn ok(&self, args: &[&str]) -> Out {
        let out = self.run(args);
        assert_eq!(
            out.code, 0,
            "expected success from {args:?}\n{}\n{}",
            out.stdout, out.stderr
        );
        out
    }

    fn init(&self, name: &str, extra: &[&str]) {
        let template = self.root.join("work.md");
        let mut args = vec!["init", name, "--template", template.to_str().unwrap()];
        args.extend_from_slice(extra);
        self.ok(&args);
    }

    fn supersede(&self, succ: &str, pred: &str) -> Value {
        let template = self.root.join("work.md");
        self.ok(&[
            "init",
            succ,
            "--template",
            template.to_str().unwrap(),
            "--continues",
            pred,
            "--supersede",
        ])
        .json()
    }

    fn record(&self, name: &str, what: &str) {
        self.ok(&[
            "decisions",
            "record",
            name,
            "--kind",
            "judgment",
            "--outcome",
            "settled",
            "--subject",
            &format!("what={what}"),
            "--by",
            "person:dana",
            "--surface",
            "terminal",
        ]);
    }

    fn chain(&self, name: &str) -> Value {
        self.ok(&["decisions", "chain", name]).json()
    }

    fn dir(&self, name: &str) -> PathBuf {
        self.sessions().join(name)
    }

    fn marker_path(&self, name: &str) -> PathBuf {
        self.dir(name).join("ctx").join(MARKER)
    }

    fn marker(&self, name: &str) -> Value {
        serde_json::from_slice(&std::fs::read(self.marker_path(name)).unwrap()).unwrap()
    }

    fn header(&self, name: &str) -> Value {
        let log = std::fs::read_to_string(self.dir(name).join(format!("koto-{name}.state.jsonl")))
            .unwrap();
        serde_json::from_str(log.lines().next().unwrap()).unwrap()
    }

    fn fence(&self, name: &str) -> Value {
        let log = std::fs::read_to_string(self.dir(name).join(format!("koto-{name}.state.jsonl")))
            .unwrap();
        log.lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .find(|e| e["type"] == "session_superseded")
            .expect("the log holds the fence")
    }

    /// Every file under the sessions base and its bytes, lock files aside.
    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut out = BTreeMap::new();
        snapshot_into(&self.sessions(), &self.sessions(), &mut out);
        out
    }
}

fn snapshot_into(base: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd {
        let entry = entry.unwrap();
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path).unwrap();
        if meta.is_dir() {
            snapshot_into(base, &path, out);
        } else if meta.is_file() {
            if entry.file_name().to_string_lossy().ends_with(".lock") {
                continue;
            }
            out.insert(
                path.strip_prefix(base).unwrap().to_path_buf(),
                std::fs::read(&path).unwrap(),
            );
        }
    }
}

// ===== The marker =====

#[test]
fn supersede_writes_the_marker_beside_the_fence_outside_the_manifest() {
    let env = Env::new();
    env.init("p", &[]);
    let out = env.supersede("s", "p");
    assert_eq!(out["outcome"], "superseded");
    assert_eq!(out["marker"], "written");

    let marker = env.marker("p");
    let fence = env.fence("p");
    assert_eq!(marker["v"], 1);
    assert_eq!(marker["successor"]["name"], "s");
    assert_eq!(marker["successor"]["id"], env.header("s")["session_id"]);
    assert_eq!(marker["actor"], fence["payload"]["actor"]);
    assert_eq!(marker["time"], fence["payload"]["at"]);
    assert!(marker["decisions_through"].is_null());
    assert!(marker
        .as_object()
        .unwrap()
        .contains_key("decisions_through"));

    // One stat finds it: the manifest doesn't list it, and the context
    // listing doesn't show it.
    let manifest = env.dir("p").join("ctx").join("manifest.json");
    if let Ok(text) = std::fs::read_to_string(&manifest) {
        assert!(!text.contains("superseded-by"), "{text}");
    }
    let listed = env
        .ok(&["context", "list", "p", "--prefix", "koto/"])
        .stdout;
    assert!(!listed.contains("superseded-by"), "{listed}");
}

#[test]
fn the_marker_records_the_highest_decision_id_the_fence_saw() {
    let env = Env::new();
    env.init("p", &[]);
    env.record("p", "a");
    env.record("p", "b");
    env.supersede("s", "p");
    assert_eq!(env.marker("p")["decisions_through"], "000000000002");
}

#[test]
fn a_re_run_writes_a_missing_marker_and_leaves_a_present_one() {
    let env = Env::new();
    env.init("p", &[]);
    env.record("p", "a");
    env.supersede("s", "p");
    let first = env.marker("p");

    // A run stopped between the fence and the marker leaves no marker.
    std::fs::remove_file(env.marker_path("p")).unwrap();
    let out = env.supersede("s", "p");
    assert_eq!(out["outcome"], "unchanged");
    assert_eq!(out["marker"], "written");
    assert_eq!(env.marker("p"), first);

    let bytes = std::fs::read(env.marker_path("p")).unwrap();
    let out = env.supersede("s", "p");
    assert_eq!(out["outcome"], "unchanged");
    assert_eq!(out["marker"], "present");
    assert_eq!(std::fs::read(env.marker_path("p")).unwrap(), bytes);
}

#[test]
fn context_writers_are_refused_on_the_marker() {
    let env = Env::new();
    env.init("p", &[]);
    env.init("q", &[]);
    env.supersede("s", "p");
    let before = std::fs::read(env.marker_path("p")).unwrap();

    for session in ["p", "q"] {
        for key in [MARKER, "KOTO/Superseded-By", "koto/superseded-by/x"] {
            let out = env.run_stdin(&["context", "add", session, key], Some("forged"));
            assert_ne!(out.code, 0, "add {key} in {session}: {}", out.stdout);
            let out = env.run(&["context", "remove", session, key]);
            assert_ne!(out.code, 0, "remove {key} in {session}: {}", out.stdout);
        }
    }
    // The refusal says why, on a session no fence refuses first.
    let out = env.run_stdin(&["context", "add", "q", MARKER], Some("forged"));
    let text = format!("{}{}", out.stdout, out.stderr);
    assert!(text.contains("supersede marker"), "{text}");
    assert_eq!(std::fs::read(env.marker_path("p")).unwrap(), before);
    assert!(!env.marker_path("q").exists());
}

// ===== The chain read =====

#[test]
fn the_chain_read_follows_each_hop_forward_and_writes_nothing() {
    let env = Env::new();
    env.init("top", &[]);
    env.init("w", &["--dispatched-by", "top"]);
    env.supersede("top2", "top");
    env.supersede("top3", "top2");

    let before = env.snapshot();
    let read = env.chain("w");
    assert_eq!(env.snapshot(), before, "the chain read wrote something");

    assert_eq!(read["v"], 1);
    assert_eq!(read["session"], "w");
    assert!(read["marker"].is_null());
    assert_eq!(read["incomplete"], false);
    let links = read["links"].as_array().unwrap();
    assert_eq!(links.len(), 1, "{read}");
    assert_eq!(links[0]["session"], "top");
    assert!(links[0].get("unreadable").is_none());
    let hops = links[0]["hops"].as_array().unwrap();
    assert_eq!(hops.len(), 2, "{read}");
    assert_eq!(hops[0]["successor"]["name"], "top2");
    assert_eq!(hops[0]["successor"]["id"], env.header("top2")["session_id"]);
    assert_eq!(hops[0]["confirmed"], true);
    assert_eq!(hops[0]["actor"], env.fence("top")["payload"]["actor"]);
    assert_eq!(hops[0]["time"], env.fence("top")["payload"]["at"]);
    assert_eq!(hops[1]["successor"]["name"], "top3");
    assert_eq!(hops[1]["confirmed"], true);

    // The superseded session's own read shows its marker.
    let own = env.chain("top");
    assert_eq!(own["marker"], env.marker("top"));
    assert_eq!(own["marker"]["successor"]["name"], "top2");
    assert!(own["links"].as_array().unwrap().is_empty());
    assert_eq!(own["incomplete"], false);
}

#[test]
fn a_marker_whose_id_does_not_match_the_successor_is_unconfirmed() {
    let env = Env::new();
    env.init("top", &[]);
    env.init("w", &["--dispatched-by", "top"]);
    env.supersede("top2", "top");
    let mut marker = env.marker("top");
    marker["successor"]["id"] = Value::from("00000000-0000-0000-0000-000000000000");
    std::fs::write(env.marker_path("top"), marker.to_string()).unwrap();

    let read = env.chain("w");
    let hops = read["links"][0]["hops"].as_array().unwrap();
    assert_eq!(hops.len(), 1, "{read}");
    assert_eq!(hops[0]["successor"]["name"], "top2");
    assert_eq!(hops[0]["confirmed"], false);
    assert_eq!(read["incomplete"], false);
}

#[test]
fn a_malformed_marker_reads_as_unreadable_and_incomplete() {
    let env = Env::new();
    env.init("top", &[]);
    env.init("w", &["--dispatched-by", "top"]);
    env.supersede("top2", "top");
    std::fs::write(env.marker_path("top"), b"not a marker").unwrap();

    let read = env.chain("w");
    assert_eq!(read["incomplete"], true, "{read}");
    let link = &read["links"][0];
    assert!(link["unreadable"].as_str().is_some(), "{read}");
    assert!(link["hops"].as_array().unwrap().is_empty());

    let own = env.chain("top");
    assert!(own["marker"].is_null());
    assert!(own["marker_unreadable"].as_str().is_some(), "{own}");
    assert_eq!(own["incomplete"], true);

    // A re-run leaves the malformed marker as it is.
    let out = env.supersede("top2", "top");
    assert_eq!(out["marker"], "unreadable");
    assert_eq!(
        std::fs::read(env.marker_path("top")).unwrap(),
        b"not a marker"
    );
}

#[test]
fn the_chain_read_of_an_unknown_session_is_refused() {
    let env = Env::new();
    let out = env.run(&["decisions", "chain", "nobody"]);
    assert_eq!(out.code, 2, "{}\n{}", out.stdout, out.stderr);
}
