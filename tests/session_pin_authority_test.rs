//! Release authority: `koto session anchor open --record-authority` records
//! what `KOTO_AUTHORITY` handed an agent session's start in its anchor, and
//! `koto session unpin` under `KOTO_ANCHOR` decides from that record alone
//! (DESIGN-session-pin.md, Decision 4). Driven through the real binary on
//! the local backend in a scratch HOME, with no niwa.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// One state, terminal: the first `koto next` finishes the session.
const TEMPLATE: &str = r#"---
name: pin-term
version: "1.0"
initial_state: done
states:
  done:
    terminal: true
---

## done

Done.
"#;

struct Env {
    _tmp: TempDir,
    home: PathBuf,
    cache: PathBuf,
    template: PathBuf,
    work: PathBuf,
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
        let template = root.join("term.md");
        std::fs::write(&template, TEMPLATE).unwrap();
        Env {
            _tmp: tmp,
            home,
            cache,
            template,
            work,
        }
    }

    /// A command in the workspace with a one-second retention and no
    /// anchor, authority, agent session or backend inherited. `env` is
    /// applied last, so it can set any of them.
    fn run_env(&self, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
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
            .env("KOTO_RETENTION_KEEP", "1s")
            .env("KOTO_RETENTION_IDLE_PERIOD", "1h")
            .env("KOTO_RETENTION_STALE_AFTER", "2h");
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.args(args).output().unwrap()
    }

    fn ok_env(&self, args: &[&str], env: &[(&str, &str)]) -> Value {
        let out = self.run_env(args, env);
        assert!(
            out.status.success(),
            "koto {args:?} failed ({:?}): {}\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }

    fn ok(&self, args: &[&str]) -> Value {
        self.ok_env(args, &[])
    }

    /// A finished session, kept on disk.
    fn done(&self, name: &str) {
        self.ok(&["init", name, "--template", self.template.to_str().unwrap()]);
        self.ok(&["next", name, "--no-cleanup"]);
    }

    /// Open `anchor-<harness>` the way the start hook does: with
    /// `--record-authority` and `authority`, when given, as
    /// `KOTO_AUTHORITY`. Returns the anchor's name.
    fn start(
        &self,
        harness: &str,
        source: &str,
        authority: Option<&str>,
        extra: &[&str],
    ) -> String {
        let mut args = vec![
            "session".to_string(),
            "anchor".to_string(),
            "open".to_string(),
            format!("--harness-session={harness}"),
            format!("--source={source}"),
            "--record-authority".to_string(),
        ];
        args.extend(extra.iter().map(|s| s.to_string()));
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let env: Vec<(&str, &str)> = authority
            .map(|a| ("KOTO_AUTHORITY", a))
            .into_iter()
            .collect();
        let out = self.run_env(&args, &env);
        assert!(out.status.success(), "open failed: {out:?}");
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .find_map(|l| l.strip_prefix("anchor=").map(str::to_string))
            .expect("an anchor= line")
    }

    fn sessions(&self) -> PathBuf {
        self.home.join(".koto/sessions")
    }

    fn pin_file(&self, name: &str) -> PathBuf {
        self.sessions().join(name).join("ctx/koto/links/pin")
    }

    fn release_file(&self, name: &str) -> PathBuf {
        self.sessions().join(name).join("ctx/koto/pin-release")
    }

    fn authority_file(&self, anchor: &str) -> PathBuf {
        self.sessions()
            .join(anchor)
            .join("ctx/koto/anchor/authority")
    }

    fn query(&self, name: &str) -> Value {
        self.ok(&["session", "hierarchy", name, "--json"])
    }

    fn unpin_under(
        &self,
        name: &str,
        anchor: &str,
        extra_env: &[(&str, &str)],
    ) -> std::process::Output {
        let mut env = vec![("KOTO_ANCHOR", anchor)];
        env.extend_from_slice(extra_env);
        self.run_env(&["session", "unpin", name], &env)
    }
}

/// SHA-256 of every file under `root`, by relative path; empty when `root`
/// isn't there.
fn hashes(root: &Path) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            let meta = std::fs::symlink_metadata(&p).unwrap();
            if meta.is_dir() {
                stack.push(p);
            } else {
                let digest = Sha256::digest(std::fs::read(&p).unwrap_or_default());
                out.insert(
                    p.strip_prefix(root).unwrap().to_path_buf(),
                    hex::encode(digest),
                );
            }
        }
    }
    out
}

/// Assert `out` is the refusal for session `name` under `anchor`: exit 2,
/// `authority_required`, naming `release-pin`, the anchor and the start,
/// and return its message.
fn refused(out: &std::process::Output, anchor: &str) -> String {
    assert_eq!(
        out.status.code(),
        Some(2),
        "expected a refusal: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let err: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(err["error"]["code"], "authority_required");
    assert_eq!(err["error"]["authority"], "release-pin");
    assert_eq!(err["error"]["anchor"], anchor);
    let message = err["error"]["message"].as_str().unwrap().to_string();
    assert!(message.contains("release-pin"), "{message}");
    assert!(message.contains(&format!("'{anchor}'")), "{message}");
    assert!(message.contains("at its start"), "{message}");
    message
}

/// Assert P is still pinned and holds no release record, every file of it
/// as it was.
fn still_pinned(env: &Env, before: &BTreeMap<PathBuf, String>) {
    assert!(env.pin_file("P").is_file());
    assert!(!env.release_file("P").exists());
    assert_eq!(&hashes(&env.sessions().join("P")), before);
}

#[test]
fn a_start_handed_release_pin_lets_its_anchor_unpin() {
    let env = Env::new();
    let anchor = env.start("h1", "startup", Some("release-pin"), &[]);
    assert_eq!(anchor, "anchor-h1");
    let q = env.query(&anchor);
    assert_eq!(q["authorities"], serde_json::json!(["release-pin"]));
    assert_eq!(q["unknown_authorities"], serde_json::json!([]));
    let text =
        String::from_utf8(env.run_env(&["session", "hierarchy", &anchor], &[]).stdout).unwrap();
    assert!(
        text.contains("authorities handed at its latest start (startup"),
        "{text}"
    );

    env.done("P");
    env.ok(&["session", "pin", "P", "--reason", "keep"]);
    let anchor_before = hashes(&env.sessions().join(&anchor));
    let out = env.unpin_under("P", &anchor, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let released: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(released["released"]["released_by"], "anchor-h1");
    assert_eq!(released["released"]["authority"], "release-pin");
    let record: Value =
        serde_json::from_slice(&std::fs::read(env.release_file("P")).unwrap()).unwrap();
    assert_eq!(record["released_by"], "anchor-h1");
    assert_eq!(record["authority"], "release-pin");
    assert!(!env.pin_file("P").exists());
    // The check read the anchor's file directly: its log and every other
    // file are unchanged, and no read spool appeared anywhere.
    assert_eq!(hashes(&env.sessions().join(&anchor)), anchor_before);
    for dir in [env.sessions().join(&anchor), env.sessions().join("P")] {
        assert!(!dir.join("context-reads.jsonl").exists());
    }

    // A call by hand still unpins.
    env.ok(&["session", "pin", "P", "--reason", "again"]);
    let by_hand = env.ok(&["session", "unpin", "P"]);
    assert_eq!(by_hand["released"]["released_by"], Value::Null);
    assert_eq!(by_hand["released"]["authority"], Value::Null);
    assert!(!env.pin_file("P").exists());
}

#[test]
fn a_start_handed_nothing_records_nothing_and_clears_an_earlier_start() {
    let env = Env::new();
    // An anchor whose starts were never handed anything: the flag changes
    // none of its context files.
    let anchor = env.start("h2", "startup", None, &["--observed-key=k2"]);
    let ctx = env.sessions().join(&anchor).join("ctx");
    let before = hashes(&ctx);
    env.start("h2", "compact", None, &[]);
    env.start("h2", "compact", Some(""), &[]);
    env.start("h2", "compact", Some(" , ,"), &[]);
    assert_eq!(hashes(&ctx), before);
    assert!(!env.authority_file(&anchor).exists());
    assert!(env.query(&anchor).get("authorities").is_none());

    // Handed release-pin, then a later start handed nothing: the record is
    // cleared, and a start after that writes nothing again.
    env.start("h2", "resume", Some("release-pin"), &[]);
    assert_eq!(
        env.query(&anchor)["authorities"],
        serde_json::json!(["release-pin"])
    );
    env.start("h2", "resume", None, &[]);
    let q = env.query(&anchor);
    assert_eq!(q["authorities"], serde_json::json!([]));
    assert_eq!(q["unknown_authorities"], serde_json::json!([]));
    let cleared = hashes(&ctx);
    env.start("h2", "compact", None, &[]);
    assert_eq!(hashes(&ctx), cleared);

    // An open without the flag leaves the record alone.
    env.start("h2", "compact", Some("release-pin"), &[]);
    let out = env.run_env(
        &[
            "session",
            "anchor",
            "open",
            "--harness-session=h2",
            "--source=compact",
        ],
        &[],
    );
    assert!(out.status.success());
    assert_eq!(
        env.query(&anchor)["authorities"],
        serde_json::json!(["release-pin"])
    );
}

#[test]
fn an_anchor_without_release_pin_is_refused() {
    let env = Env::new();
    let nothing = env.start("h3", "startup", None, &[]);
    let bogus = env.start("h4", "startup", Some("bogus"), &[]);
    let mixed = env.start("h5", "startup", Some(" BOGUS , Release-Pin "), &[]);
    let q = env.query(&bogus);
    assert_eq!(q["authorities"], serde_json::json!([]));
    assert_eq!(q["unknown_authorities"], serde_json::json!(["bogus"]));
    let q = env.query(&mixed);
    assert_eq!(q["authorities"], serde_json::json!(["release-pin"]));
    assert_eq!(q["unknown_authorities"], serde_json::json!(["bogus"]));

    env.done("P");
    env.ok(&["session", "pin", "P", "--reason", "keep"]);
    let before = hashes(&env.sessions().join("P"));

    let message = refused(&env.unpin_under("P", &nothing, &[]), &nothing);
    assert!(message.contains("records no authority"), "{message}");
    still_pinned(&env, &before);

    let message = refused(&env.unpin_under("P", &bogus, &[]), &bogus);
    assert!(message.contains("bogus"), "{message}");
    still_pinned(&env, &before);

    // The variable exported at unpin time grants nothing.
    refused(
        &env.unpin_under("P", &nothing, &[("KOTO_AUTHORITY", "release-pin")]),
        &nothing,
    );
    still_pinned(&env, &before);
}

#[test]
fn a_child_a_successor_and_a_later_start_handed_nothing_are_refused() {
    let env = Env::new();
    let parent = env.start("h6", "startup", Some("release-pin"), &["--observed-key=k6"]);
    env.done("P");
    env.ok(&["session", "pin", "P", "--reason", "keep"]);
    let before = hashes(&env.sessions().join("P"));

    // A child started from the session's shell, whose start the hook ran
    // with KOTO_AUTHORITY unset.
    let child = env.start("c6", "startup", None, &["--dispatched-by=anchor-h6"]);
    refused(&env.unpin_under("P", &child, &[]), &child);
    still_pinned(&env, &before);

    // The anchor reads closed; the next start gets a successor holding
    // only its own start's authority, none.
    let closed = env.run_env(
        &[
            "session",
            "anchor",
            "close",
            "--key=k6",
            "--harness-session=h6",
        ],
        &[],
    );
    assert!(closed.status.success());
    let successor = env.start("h6", "resume", None, &[]);
    assert_eq!(successor, "anchor-h6-s2");
    refused(&env.unpin_under("P", &successor, &[]), &successor);
    still_pinned(&env, &before);

    // A later start of the same agent session handed nothing clears it.
    let later = env.start("h7", "startup", Some("release-pin"), &[]);
    env.start("h7", "resume", None, &[]);
    refused(&env.unpin_under("P", &later, &[]), &later);
    still_pinned(&env, &before);

    // The closed anchor still records its start's authority and releases.
    let out = env.unpin_under("P", &parent, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(!env.pin_file("P").exists());
}

#[test]
fn an_anchor_whose_authority_cant_be_read_is_refused() {
    let env = Env::new();
    env.done("P");
    env.done("O");
    env.ok(&["session", "pin", "P", "--reason", "keep"]);
    let before = hashes(&env.sessions().join("P"));

    for name in ["anchor-nosuch", "O", "Bad Name"] {
        let message = refused(&env.unpin_under("P", name, &[]), name);
        assert!(message.contains("authority couldn't be read"), "{message}");
        still_pinned(&env, &before);
    }

    // An anchor whose record is damaged: garbage, then a directory.
    let damaged = env.start("h8", "startup", Some("release-pin"), &[]);
    std::fs::write(env.authority_file(&damaged), b"{not json").unwrap();
    let message = refused(&env.unpin_under("P", &damaged, &[]), &damaged);
    assert!(message.contains("authority couldn't be read"), "{message}");
    std::fs::remove_file(env.authority_file(&damaged)).unwrap();
    std::fs::create_dir(env.authority_file(&damaged)).unwrap();
    let message = refused(&env.unpin_under("P", &damaged, &[]), &damaged);
    assert!(message.contains("authority couldn't be read"), "{message}");
    still_pinned(&env, &before);

    // An anchor whose header can't be read.
    let broken = env.start("h9", "startup", Some("release-pin"), &[]);
    let state = env
        .sessions()
        .join(&broken)
        .join(format!("koto-{broken}.state.jsonl"));
    std::fs::write(&state, b"garbage\n").unwrap();
    let message = refused(&env.unpin_under("P", &broken, &[]), &broken);
    assert!(message.contains("authority couldn't be read"), "{message}");
    still_pinned(&env, &before);
}

#[test]
fn the_setters_anchor_holds_no_authority_and_the_pin_outlives_it() {
    let env = Env::new();
    let setter = env.start("h10", "startup", None, &["--observed-key=k10"]);
    env.done("P");
    let pinned = env.ok_env(
        &["session", "pin", "P", "--reason", "keep"],
        &[("KOTO_ANCHOR", &setter)],
    );
    assert_eq!(pinned["pin"]["set_by"], setter.as_str());
    let before = hashes(&env.sessions().join("P"));
    refused(&env.unpin_under("P", &setter, &[]), &setter);
    still_pinned(&env, &before);

    // Close the setter's anchor and let it idle past the idle period: the
    // prune removes it and keeps P by its pin.
    let closed = env.run_env(
        &[
            "session",
            "anchor",
            "close",
            "--key=k10",
            "--harness-session=h10",
        ],
        &[],
    );
    assert!(closed.status.success());
    std::thread::sleep(std::time::Duration::from_millis(2200));
    let idle = [("KOTO_RETENTION_IDLE_PERIOD", "1s")];
    let pruned = env.ok_env(&["session", "prune", "--apply", "--all"], &idle);
    assert!(!env.sessions().join(&setter).exists(), "{pruned}");
    assert!(env.pin_file("P").is_file());
    let kept = pruned["kept"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "P")
        .unwrap_or_else(|| panic!("P not kept: {pruned}"));
    assert_eq!(kept["kept_by_pin"], true);
    assert_eq!(kept["pin"]["set_by"], setter.as_str());
}
