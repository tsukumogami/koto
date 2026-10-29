//! The continues relation through the real binary on the local backend:
//! `--continues` on `koto init` and `koto session link`, every local
//! removal point keeping a predecessor while a later session has standing,
//! the report each one prints, and the refusals. The cloud backend's half,
//! the back-reference and the bucket walk, is in
//! `session_continues_cloud_test.rs`.
//!
//! The **chain** is v1, v2 and v3 in one workspace: v2 created continuing v1
//! in its creating command, v3 continuing v2 the same way, v1 and v2
//! finished with `--no-cleanup` and v3 running. The retention is one second,
//! waited out before each removal, so only the relation can keep a session.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use assert_cmd::Command;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const TEMPLATE: &str = r#"---
name: continues-test
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

/// Wait out the one-second retention.
fn settle() {
    std::thread::sleep(Duration::from_millis(1300));
}

struct Env {
    _tmp: TempDir,
    home: PathBuf,
    cache: PathBuf,
    template: PathBuf,
    work: PathBuf,
    keep: &'static str,
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let home = root.join("home");
        let cache = root.join("cache");
        let work = root.join("work");
        for d in [&home, &cache, &work] {
            std::fs::create_dir_all(d).unwrap();
        }
        let template = root.join("continues-test.md");
        std::fs::write(&template, TEMPLATE).unwrap();
        Env {
            _tmp: tmp,
            home,
            cache,
            template,
            work,
            keep: "1s",
        }
    }

    fn cmd(&self, env: &[(&str, &str)]) -> Command {
        let mut cmd = koto_command::koto();
        cmd.current_dir(&self.work)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("KOTO_SESSIONS_BASE")
            .env_remove("KOTO_WORKFLOWS_DIR")
            .env_remove("KOTO_ANCHOR")
            .env_remove("KOTO_AUTHORITY")
            .env_remove("KOTO_DISPATCHER_ANCHOR")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env("KOTO_RETENTION_KEEP", self.keep)
            .env("KOTO_RETENTION_IDLE_PERIOD", "1h")
            .env("KOTO_RETENTION_STALE_AFTER", "2h");
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        self.run_env(args, &[])
    }

    fn run_env(&self, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
        self.cmd(env).args(args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "koto {args:?} failed ({:?}): {}\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }

    /// Run `args`, expect exit `code`, and return stdout's JSON.
    fn fails(&self, args: &[&str], code: i32) -> Value {
        let out = self.run(args);
        assert_eq!(
            out.status.code(),
            Some(code),
            "koto {args:?}: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }

    fn init(&self, name: &str, extra: &[&str]) -> Value {
        let mut args = vec!["init", name, "--template", self.template.to_str().unwrap()];
        args.extend_from_slice(extra);
        self.ok(&args)
    }

    fn tick(&self, name: &str, no_cleanup: bool) -> Value {
        let mut args = vec!["next", name, "--with-data", r#"{"go": "yes"}"#];
        if no_cleanup {
            args.push("--no-cleanup");
        }
        let v = self.ok(&args);
        assert_eq!(v["action"], "done", "{v:#}");
        v
    }

    /// The chain: v1 <- v2 <- v3, v1 and v2 finished, v3 running, the
    /// retention waited out.
    fn chain(&self) {
        self.init("v1", &[]);
        self.init("v2", &["--continues", "v1"]);
        self.init("v3", &["--continues", "v2"]);
        self.tick("v1", true);
        self.tick("v2", true);
        settle();
    }

    fn sessions(&self) -> PathBuf {
        self.home.join(".koto/sessions")
    }

    fn exists(&self, name: &str) -> bool {
        self.sessions().join(name).is_dir()
    }

    /// SHA-256 of every file in session `name`'s directory.
    fn hashes(&self, name: &str) -> BTreeMap<PathBuf, String> {
        let root = self.sessions().join(name);
        let mut out = BTreeMap::new();
        let mut stack = vec![root.clone()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if !p.ends_with("context-reads.jsonl") {
                    let digest = Sha256::digest(std::fs::read(&p).unwrap());
                    out.insert(
                        p.strip_prefix(&root).unwrap().to_path_buf(),
                        hex::encode(digest),
                    );
                }
            }
        }
        out
    }

    fn header(&self, name: &str) -> Value {
        let path = self
            .sessions()
            .join(name)
            .join(format!("koto-{name}.state.jsonl"));
        let text = std::fs::read_to_string(path).unwrap();
        serde_json::from_str(text.lines().next().unwrap()).unwrap()
    }

    /// The relation file `holder` holds naming `pred`.
    fn relation_path(&self, holder: &str, pred: &str) -> PathBuf {
        self.sessions()
            .join(holder)
            .join("ctx/koto/links/continues")
            .join(pred)
    }

    fn hierarchy(&self, name: &str) -> Value {
        self.ok(&["session", "hierarchy", name, "--json"])
    }

    fn prune_all(&self) -> Value {
        self.ok(&["session", "prune", "--apply", "--all"])
    }

    fn preview(&self) -> Value {
        self.ok(&["session", "prune", "--all"])
    }
}

/// The entry for `name` in a prune's `kept` list.
fn kept<'a>(out: &'a Value, name: &str) -> &'a Value {
    out["kept"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == name)
        .unwrap_or_else(|| panic!("{name} isn't kept: {out:#}"))
}

fn removed(out: &Value) -> Vec<String> {
    let mut names: Vec<String> = out["removed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    names
}

fn keeper(name: &str, standing: &str, path: &[&str]) -> Value {
    json!({"keeper": name, "standing": standing, "path": path})
}

#[test]
fn the_relation_records_the_predecessors_name_id_and_workspace() {
    let env = Env::new();
    env.init("v1", &[]);
    env.init("v2", &["--continues", "v1"]);
    let body: Value =
        serde_json::from_slice(&std::fs::read(env.relation_path("v2", "v1")).unwrap()).unwrap();
    assert_eq!(body["v"], 1);
    assert_eq!(body["session"], "v1");
    assert_eq!(body["session_id"], env.header("v1")["session_id"]);
    assert_eq!(body["workspace"], env.work.to_str().unwrap());
    assert_eq!(body["prefix"], Value::Null);
    assert_eq!(body["holder_session_id"], env.header("v2")["session_id"]);
    assert!(body["at"].as_str().unwrap().ends_with('Z'));
    // Written by koto, with an event, and listed like any link.
    let listed = env.ok(&["context", "list", "v2"]);
    assert!(listed
        .as_array()
        .unwrap()
        .contains(&json!("koto/links/continues/v1")));
}

#[test]
fn the_chain_keeps_its_predecessors_at_every_removal_point() {
    let env = Env::new();
    env.chain();
    let before = (env.hashes("v1"), env.hashes("v2"));

    // The preview names the keeper and the path.
    let preview = env.preview();
    let e1 = kept(&preview, "v1");
    assert_eq!(e1["rule"], "r");
    assert_eq!(e1["kept_by_relation"], true);
    assert_eq!(
        e1["relation_keepers"],
        json!([keeper("v3", "alive", &["v2"])])
    );
    assert_eq!(e1["detail"], "kept: continued by v3 (alive) via v2");
    assert!(e1.get("kept_by_pin").is_none());
    let e2 = kept(&preview, "v2");
    assert_eq!(e2["relation_keepers"], json!([keeper("v3", "alive", &[])]));
    assert_eq!(e2["detail"], "kept: continued by v3 (alive)");

    // Every session-prune form, and the workspace prune with and without
    // --force, keeps both and leaves them byte-identical.
    let work = env.work.to_str().unwrap().to_string();
    let token = preview["plan"].as_str().unwrap().to_string();
    let steps: Vec<Vec<&str>> = vec![
        vec!["session", "prune", "--apply", "--all"],
        vec!["session", "prune", "--apply", "--execution-dir", &work],
        vec!["session", "prune", "--apply", "--older-than", "1s"],
        vec!["session", "prune", "--apply", "--plan", &token],
        vec!["workspace", "prune", "--root", "v1", "--yes"],
        vec!["workspace", "prune", "--root", "v1", "--yes", "--force"],
        vec!["workspace", "prune", "--root", "v2", "--yes"],
    ];
    for args in steps {
        // `--force` with `--yes` asks for the phrase on stdin.
        let run = env
            .cmd(&[])
            .args(&args)
            .write_stdin("force-prune\n")
            .output()
            .unwrap();
        assert!(run.status.success(), "{args:?}: {run:?}");
        // `workspace prune` prints its preview first and its JSON last.
        let stdout = String::from_utf8_lossy(&run.stdout);
        let out: Value = serde_json::from_str(
            stdout
                .lines()
                .rev()
                .find(|l| l.starts_with('{'))
                .unwrap_or_else(|| panic!("{args:?}: {stdout}")),
        )
        .or_else(|_| serde_json::from_str(&stdout))
        .unwrap_or_else(|e| panic!("{args:?}: {e}: {stdout}"));
        assert!(env.exists("v1") && env.exists("v2"), "{args:?}: {out:#}");
        assert_eq!((env.hashes("v1"), env.hashes("v2")), before, "{args:?}");
        if args[0] == "session" {
            assert!(removed(&out).is_empty(), "{args:?}: {out:#}");
            assert_eq!(
                kept(&out, "v1")["relation_keepers"],
                json!([keeper("v3", "alive", &["v2"])]),
                "{args:?}"
            );
        } else {
            let entry = out["kept"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["name"] == args[3])
                .unwrap_or_else(|| panic!("{args:?}: {out:#}"))
                .clone();
            assert_eq!(entry["kept_by_relation"], true, "{args:?}: {out:#}");
        }
    }

    // v3 reads both predecessors' keys, and they stay byte-identical.
    for name in ["v1", "v2"] {
        env.ok(&["context", "list", name]);
    }
    assert_eq!((env.hashes("v1"), env.hashes("v2")), before);
}

#[test]
fn the_terminal_tick_keeps_a_predecessor_and_names_its_keeper() {
    let env = Env::new();
    env.init("v1", &[]);
    env.init("v2", &["--continues", "v1"]);
    env.init("v3", &["--continues", "v2"]);
    // Without --no-cleanup: kept all the same, every keeper named (v2 and
    // v3 both still run).
    let t1 = env.tick("v1", false);
    assert_eq!(t1["retention"]["retained"], true, "{t1:#}");
    assert_eq!(t1["retention"]["reason"], "hierarchy");
    assert_eq!(
        t1["retention"]["relation_keepers"],
        json!([keeper("v2", "alive", &[]), keeper("v3", "alive", &["v2"])])
    );
    let t2 = env.tick("v2", true);
    assert_eq!(
        t2["retention"]["relation_keepers"],
        json!([keeper("v3", "alive", &[])])
    );
    assert!(env.exists("v1") && env.exists("v2"));
    // A session no relation names reports nothing new.
    env.init("lone", &[]);
    let t = env.tick("lone", true);
    assert!(t["retention"].get("relation_keepers").is_none(), "{t:#}");
}

#[test]
fn the_apply_judges_again_and_release_follows_the_last_member() {
    let env = Env {
        keep: "2s",
        ..Env::new()
    };
    env.chain();
    // The chain settles for one second; this retention is two.
    std::thread::sleep(Duration::from_millis(1000));
    let preview = env.preview();
    let token = preview["plan"].as_str().unwrap().to_string();
    assert_eq!(kept(&preview, "v1")["rule"], "r", "{preview:#}");

    // v3 finishes: within its retention it keeps the chain, with when the
    // retention passes.
    env.tick("v3", true);
    let out = env.prune_all();
    assert!(removed(&out).is_empty(), "{out:#}");
    // v3's hierarchy is v3 alone (the relation joins nothing), so its
    // retention passes at v3's own stop plus the two seconds.
    let v3 = env.hierarchy("v3");
    assert_eq!(v3["members"].as_array().unwrap().len(), 1, "{v3:#}");
    let passes = v3["retention"]["passes_at"].clone();
    assert!(passes.is_string(), "{v3:#}");
    let k = &kept(&out, "v1")["relation_keepers"][0];
    assert_eq!(k["standing"], "retention");
    assert_eq!(k["retention_passes_at"], passes);
    assert_eq!(k["path"], json!(["v2"]));

    // Once it passes, the preview's plan is stale (it kept them) and a
    // fresh apply removes all three.
    std::thread::sleep(Duration::from_millis(2300));
    let stale = env.fails(&["session", "prune", "--apply", "--plan", &token], 4);
    assert_eq!(stale["error"]["code"], "prune_plan_stale", "{stale:#}");
    assert!(env.exists("v1"));
    let out = env.prune_all();
    assert_eq!(removed(&out), vec!["v1", "v2", "v3"], "{out:#}");
}

#[test]
fn a_pin_on_the_newest_keeps_the_chain_and_one_on_the_oldest_only_itself() {
    let env = Env::new();
    env.chain();
    env.tick("v3", true);
    env.ok(&["session", "pin", "v3", "--reason", "keep the chain"]);
    settle();
    let out = env.prune_all();
    assert!(removed(&out).is_empty(), "{out:#}");
    for (name, path) in [("v1", vec!["v2"]), ("v2", vec![])] {
        let e = kept(&out, name);
        assert!(e.get("kept_by_pin").is_none(), "{e:#}");
        let k = &e["relation_keepers"][0];
        assert_eq!(k["keeper"], "v3");
        assert_eq!(k["standing"], "pin");
        assert_eq!(k["pin"]["reason"], "keep the chain");
        assert!(k["pin"]["set_at"].is_string());
        assert_eq!(k["path"], json!(path));
    }
    assert_eq!(kept(&out, "v3")["kept_by_pin"], true);

    // Backward: a pin on v1 keeps v1 alone.
    let env = Env::new();
    env.chain();
    env.tick("v3", true);
    env.ok(&["session", "pin", "v1", "--reason", "only the oldest"]);
    settle();
    let out = env.prune_all();
    assert_eq!(removed(&out), vec!["v2", "v3"], "{out:#}");
    let e = kept(&out, "v1");
    assert_eq!(e["kept_by_pin"], true);
    assert!(e.get("relation_keepers").is_none(), "{e:#}");

    // A running v1 keeps nothing later either.
    let env = Env::new();
    env.init("v1", &[]);
    env.init("v2", &["--continues", "v1"]);
    env.init("v3", &["--continues", "v2"]);
    env.tick("v2", true);
    env.tick("v3", true);
    settle();
    let out = env.prune_all();
    assert_eq!(removed(&out), vec!["v2", "v3"], "{out:#}");
    assert!(env.exists("v1"));
}

#[test]
fn two_successors_keep_the_predecessor_while_either_stands() {
    let env = Env::new();
    env.init("v1", &[]);
    env.init("v2a", &["--continues", "v1"]);
    env.init("v2b", &["--continues", "v1"]);
    env.tick("v1", true);
    settle();
    let out = env.preview();
    assert_eq!(
        kept(&out, "v1")["relation_keepers"],
        json!([keeper("v2a", "alive", &[]), keeper("v2b", "alive", &[])])
    );
    env.tick("v2a", true);
    settle();
    let out = env.preview();
    assert_eq!(
        kept(&out, "v1")["relation_keepers"],
        json!([keeper("v2b", "alive", &[])])
    );
    env.tick("v2b", true);
    settle();
    let out = env.prune_all();
    assert_eq!(removed(&out), vec!["v1", "v2a", "v2b"], "{out:#}");
}

#[test]
fn the_hierarchy_query_shows_the_relation_and_no_join() {
    let env = Env::new();
    env.chain();
    let with: Vec<Value> = ["v1", "v2", "v3"]
        .iter()
        .map(|n| env.hierarchy(n))
        .collect();
    assert_eq!(with[1]["predecessor"], "v1");
    assert_eq!(with[1]["successors"], json!(["v3"]));
    assert_eq!(
        with[0]["relation_keepers"],
        json!([keeper("v3", "alive", &["v2"])])
    );
    assert!(with[0].get("predecessor").is_none());
    assert!(with[2].get("successors").is_none());
    let text = env.run(&["session", "hierarchy", "v1"]);
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(
        text.contains("kept: continued by v3 (alive) via v2"),
        "{text}"
    );
    assert!(text.contains("continued by: v2"), "{text}");

    // The same store with the relations removed: same members, keepers and
    // release instants.
    std::fs::remove_file(env.relation_path("v2", "v1")).unwrap();
    std::fs::remove_file(env.relation_path("v3", "v2")).unwrap();
    let manifest_drop = |name: &str, key: &str| {
        let path = env.sessions().join(name).join("ctx/manifest.json");
        let mut m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        m["keys"].as_object_mut().unwrap().remove(key);
        std::fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();
    };
    manifest_drop("v2", "koto/links/continues/v1");
    manifest_drop("v3", "koto/links/continues/v2");
    let without: Vec<Value> = ["v1", "v2", "v3"]
        .iter()
        .map(|n| env.hierarchy(n))
        .collect();
    for (a, b) in with.iter().zip(&without) {
        for field in ["members", "keepers", "retention", "kept"] {
            let strip = |v: &Value| {
                let mut v = v[field].clone();
                if let Some(ms) = v.as_array_mut() {
                    for m in ms {
                        if let Some(o) = m.as_object_mut() {
                            o.remove("now");
                        }
                    }
                }
                v
            };
            assert_eq!(strip(a), strip(b), "{field}: {a:#} / {b:#}");
        }
    }
}

#[test]
fn the_same_name_replace_moves_a_kept_predecessor_aside_with_its_id() {
    let env = Env::new();
    env.chain();
    let id = env.header("v1")["session_id"].clone();
    let out = env.ok(&[
        "init",
        "v1",
        "--template",
        env.template.to_str().unwrap(),
        "--replace-terminal",
    ]);
    let moved = out["moved_to"]
        .as_str()
        .unwrap_or_else(|| panic!("{out:#}"));
    assert_eq!(out["kept_by_relation"], true, "{out:#}");
    assert_eq!(env.header(moved)["session_id"], id);
    let h = env.hierarchy(moved);
    assert_eq!(
        h["relation_keepers"],
        json!([keeper("v3", "alive", &["v2"])]),
        "{h:#}"
    );
    let preview = env.preview();
    assert_eq!(
        kept(&preview, moved)["relation_keepers"],
        json!([keeper("v3", "alive", &["v2"])])
    );
}

#[test]
fn the_parent_sweep_keeps_a_child_a_later_session_continues() {
    let env = Env::new();
    env.init("x", &[]);
    env.init("child", &["--parent", "x"]);
    env.tick("child", true);
    env.tick("x", true);
    env.init("succ", &["--continues", "child"]);
    settle();
    // x's hierarchy (x and its child) is released, so the replace removes
    // x and sweeps its children; the relation keeps the child.
    let out = env.ok(&[
        "init",
        "x",
        "--template",
        env.template.to_str().unwrap(),
        "--replace-terminal",
    ]);
    assert!(out.get("moved_to").is_none(), "{out:#}");
    assert!(env.exists("child"), "{out:#}");
    let kept = &out["kept_children"][0];
    assert_eq!(kept["name"], "child", "{out:#}");
    assert_eq!(kept["reason"], "relation");
    assert_eq!(
        kept["relation_keepers"],
        json!([keeper("succ", "alive", &[])])
    );
}

#[test]
fn cleanup_and_cancel_remove_anyway_and_report_what_they_passed_over() {
    let env = Env::new();
    env.chain();
    let out = env.run(&["session", "cleanup", "v1"]);
    assert!(out.status.success(), "{out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        v["relation_keepers"],
        json!([keeper("v3", "alive", &["v2"])])
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("kept: continued by v3 (alive) via v2"));
    assert!(!env.exists("v1"));

    // A running v1, cancelled with --cleanup.
    let env = Env::new();
    env.init("v1", &[]);
    env.init("v2", &["--continues", "v1"]);
    env.init("v3", &["--continues", "v2"]);
    env.tick("v2", true);
    settle();
    let out = env.run(&["cancel", "v1", "--cleanup"]);
    assert!(out.status.success(), "{out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        v["relation_keepers"],
        json!([keeper("v3", "alive", &["v2"])])
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("kept: continued by v3 (alive) via v2"));
    assert!(!env.exists("v1"));

    // A pin still refuses both.
    let env = Env::new();
    env.chain();
    env.ok(&["session", "pin", "v1", "--reason", "hold"]);
    let v = env.fails(&["session", "cleanup", "v1"], 2);
    assert_eq!(v["error"]["code"], "pinned", "{v:#}");
    let env = Env::new();
    env.init("v1", &[]);
    env.init("v2", &["--continues", "v1"]);
    env.ok(&["session", "pin", "v1", "--reason", "hold"]);
    let v = env.fails(&["cancel", "v1", "--cleanup"], 2);
    assert_eq!(v["error"]["code"], "pinned", "{v:#}");
}

#[test]
fn a_relation_recorded_on_an_open_session_keeps_the_predecessor() {
    let env = Env::new();
    env.init("v1", &[]);
    env.tick("v1", true);
    env.init("v2", &[]);
    let v = env.ok(&["session", "link", "v2", "--continues", "v1"]);
    assert_eq!(
        v,
        json!({"name": "v2", "kind": "continues", "peer": "v1", "written": true})
    );
    settle();
    let out = env.prune_all();
    assert!(removed(&out).is_empty(), "{out:#}");
    assert_eq!(
        kept(&out, "v1")["relation_keepers"],
        json!([keeper("v2", "alive", &[])])
    );
}

#[test]
fn a_stranger_records_a_takeover_with_no_anchor_authority_or_leg() {
    let env = Env::new();
    // v1 was dispatched by a coordinator that is gone.
    env.init("v1", &["--dispatched-by", "coordinator-a"]);
    env.tick("v1", true);
    // An operator by hand, then an unrelated agent session's driver, each
    // with nothing linking it to v1 or its dispatcher.
    env.init("v2", &[]);
    env.ok(&["session", "link", "v2", "--continues", "v1"]);
    let b = env.run_env(
        &[
            "init",
            "v3",
            "--template",
            env.template.to_str().unwrap(),
            "--continues",
            "v1",
        ],
        &[("KOTO_ANCHOR", "anchor-b")],
    );
    assert!(b.status.success(), "{b:?}");
    settle();
    let out = env.prune_all();
    assert!(env.exists("v1"), "{out:#}");
    let e = kept(&out, "v1");
    assert_eq!(
        e["relation_keepers"],
        json!([keeper("v2", "alive", &[]), keeper("v3", "alive", &[])]),
        "{e:#}"
    );
    // The relation is the only record naming v1, and nothing names the
    // dispatcher.
    for s in ["v2", "v3"] {
        let keys = env.ok(&["context", "list", s]);
        let links: Vec<&str> = keys
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|k| k.as_str())
            .filter(|k| k.starts_with("koto/links/"))
            .filter(|k| k.ends_with("/v1") || k.contains("coordinator-a"))
            .collect();
        assert_eq!(links, vec!["koto/links/continues/v1"], "{s}");
    }
}

#[test]
fn the_verb_refuses_and_writes_nothing() {
    let env = Env::new();
    env.init("v1", &[]);
    env.init("v2", &["--continues", "v1"]);
    env.init("other", &[]);
    let anchor = env.run(&[
        "session",
        "anchor",
        "open",
        "--harness-session",
        "h1",
        "--source",
        "startup",
    ]);
    assert!(anchor.status.success(), "{anchor:?}");
    let anchors: Vec<String> = std::fs::read_dir(env.sessions())
        .unwrap()
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| n.starts_with("anchor-"))
        .collect();
    let anchor = anchors.first().expect("an anchor").clone();
    // An anchor's close record, as its header marks it.
    let closed = env.sessions().join("anchor-x-closed");
    std::fs::create_dir_all(&closed).unwrap();
    let header = json!({
        "schema_version": 1, "workflow": "anchor-x-closed", "template_hash": "h",
        "created_at": "2026-01-01T00:00:00Z", "session_id": "id-closed", "kind": "anchor-close",
    });
    std::fs::write(
        closed.join("koto-anchor-x-closed.state.jsonl"),
        format!("{header}\n"),
    )
    .unwrap();
    let before = env.hashes("v2");
    for pred in [
        "a/b",
        "v2",
        "nosuch",
        anchor.as_str(),
        "anchor-x-closed",
        "other",
    ] {
        let v = env.fails(&["session", "link", "v2", "--continues", pred], 2);
        assert_eq!(v["command"], "session link", "{pred}: {v:#}");
        assert_eq!(env.hashes("v2"), before, "{pred}");
    }
    // Repeating the relation the session holds changes nothing.
    let v = env.ok(&["session", "link", "v2", "--continues", "v1"]);
    assert_eq!(v["written"], false);
    assert_eq!(env.hashes("v2"), before);
    // Exactly one of the two link kinds.
    env.fails(&["session", "link", "v2"], 2);
    env.fails(
        &[
            "session",
            "link",
            "v2",
            "--continues",
            "v1",
            "--dispatched-by",
            "other",
        ],
        2,
    );
    // --continues-from takes a workspace.
    env.fails(
        &[
            "session",
            "link",
            "v2",
            "--continues",
            "v1",
            "--continues-from",
            "relative/path",
        ],
        2,
    );
    // At init, a refused predecessor leaves no session behind.
    let out = env.run(&[
        "init",
        "v9",
        "--template",
        env.template.to_str().unwrap(),
        "--continues",
        "nosuch",
    ]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(!env.exists("v9"));
}

#[test]
fn the_verb_takes_a_running_finished_or_pinned_predecessor() {
    let env = Env::new();
    env.init("running", &[]);
    env.init("finished", &[]);
    env.tick("finished", true);
    env.init("pinned", &[]);
    env.ok(&["session", "pin", "pinned", "--reason", "hold"]);
    for pred in ["running", "finished", "pinned"] {
        let succ = format!("after-{pred}");
        env.init(&succ, &[]);
        let v = env.ok(&["session", "link", &succ, "--continues", pred]);
        assert_eq!(v["written"], true, "{pred}");
    }
    let from = env.work.to_str().unwrap().to_string();
    env.init(
        "by-path",
        &["--continues", "finished", "--continues-from", &from],
    );
    assert!(env.relation_path("by-path", "finished").is_file());
}

#[test]
fn an_unreadable_relation_keeps_what_it_names() {
    for (body, reason) in [
        (b"not json".to_vec(), "unparseable"),
        (vec![b' '; 5 * 1024], "too_large"),
    ] {
        let env = Env::new();
        env.init("v1", &[]);
        env.init("v2", &["--continues", "v1"]);
        env.tick("v1", true);
        env.tick("v2", true);
        std::fs::write(env.relation_path("v2", "v1"), &body).unwrap();
        settle();
        let out = env.prune_all();
        assert_eq!(removed(&out), vec!["v2"], "{reason}: {out:#}");
        assert_eq!(
            kept(&out, "v1")["relation_unreadable"],
            json!([{"holder": "v2", "reason": reason}])
        );
    }
}

#[test]
fn a_new_session_under_a_removed_predecessors_name_is_a_mismatch() {
    let env = Env::new();
    env.init("v1", &[]);
    env.init("v2", &["--continues", "v1"]);
    env.ok(&["session", "cleanup", "v1"]);
    env.init("v1", &[]);
    env.tick("v1", true);
    settle();
    let out = env.prune_all();
    assert!(env.exists("v1"), "{out:#}");
    assert_eq!(
        kept(&out, "v1")["relation_unreadable"],
        json!([{"holder": "v2", "reason": "mismatch"}])
    );
}

#[test]
fn a_cycle_of_relations_keeps_every_session_on_it() {
    let env = Env::new();
    env.init("v1", &[]);
    env.init("v2", &["--continues", "v1"]);
    env.init("v3", &["--continues", "v2"]);
    // v1 continues v3 too, written by hand.
    let body = json!({
        "v": 1, "session": "v3", "session_id": env.header("v3")["session_id"],
        "workspace": null, "prefix": null, "at": "2026-01-01T00:00:00Z",
    });
    let path = env.relation_path("v1", "v3");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, body.to_string()).unwrap();
    for n in ["v1", "v2", "v3"] {
        env.tick(n, true);
    }
    settle();
    let out = env.prune_all();
    assert!(removed(&out).is_empty(), "{out:#}");
    for n in ["v1", "v2", "v3"] {
        assert_eq!(
            kept(&out, n)["relation_unreadable"],
            json!([{"holder": n, "reason": "cycle"}]),
            "{n}"
        );
    }
}

#[test]
fn the_relation_grants_no_authority() {
    // Releasing a pin under an anchor needs `release-pin`, relation or not,
    // and the anchor's authority display is the same.
    let mut shown = Vec::new();
    for related in [false, true] {
        let env = Env::new();
        env.init("v1", &[]);
        env.init("v2", &[]);
        if related {
            env.ok(&["session", "link", "v2", "--continues", "v1"]);
        }
        env.ok(&["session", "pin", "v1", "--reason", "hold"]);
        let a = env.run(&[
            "session",
            "anchor",
            "open",
            "--harness-session",
            "h",
            "--source",
            "startup",
        ]);
        assert!(a.status.success(), "{a:?}");
        let anchor = std::fs::read_dir(env.sessions())
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .find(|n| n.starts_with("anchor-"))
            .unwrap();
        let out = env.run_env(&["session", "unpin", "v1"], &[("KOTO_ANCHOR", &anchor)]);
        assert_eq!(out.status.code(), Some(2), "related={related}: {out:?}");
        let h = env.hierarchy(&anchor);
        assert!(h.get("relation_keepers").is_none(), "{h:#}");
        shown.push((
            h.get("authorities").cloned(),
            h.get("unknown_authorities").cloned(),
        ));
    }
    assert_eq!(shown[0], shown[1]);
}
