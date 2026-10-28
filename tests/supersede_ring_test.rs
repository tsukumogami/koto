//! Wake rings follow supersede fences, through the real binary on the local
//! backend.
//!
//! A request names its requester once. After `koto init B --continues A
//! --supersede` and `koto init C --continues B --supersede`, a leg A
//! opened still names A, and its result has to wake C's armed `koto
//! request watch` as well as A's wake file. The rest of the file pins the
//! walk's limits: a principal never superseded is rung exactly as before, a
//! broken hop stops at the last good member, a cycle ends, the walk gives up
//! after 1024 hops, and the stale-wake recovery in `koto next` follows the
//! same chain.
//!
//! Every koto process runs in its own temporary directory, under a scratch
//! `HOME` and `KOTO_HOME`, with every `KOTO_*` variable inherited from the
//! test's environment removed (`KOTO_SESSIONS_BASE` and every
//! `KOTO_SESSION_*` included) and `CLAUDE_CODE_SESSION_ID` removed. Each
//! test checks that koto resolves its store under that directory before it
//! runs anything else.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant, SystemTime};

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

/// The requirement: a watch armed for the newest member exits within this
/// long of the leg's result.
const WAKE_BOUND: Duration = Duration::from_secs(10);

/// koto's own hop cap, restated: a chain this many hops long still reaches
/// its newest member, and one hop more reaches the principal only.
const MAX_HOPS: usize = 1024;

const TEMPLATE: &str = r#"---
name: supersede-ring-test
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
    work: PathBuf,
    template: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let home = root.join("home");
        let koto_home = root.join("koto-home");
        let cache = root.join("cache");
        let work = root.join("work");
        for d in [&home, &koto_home, &cache, &work] {
            std::fs::create_dir_all(d).unwrap();
        }
        let template = root.join("supersede-ring-test.md");
        std::fs::write(&template, TEMPLATE).unwrap();
        let env = Env {
            _tmp: tmp,
            root,
            home,
            koto_home,
            cache,
            work,
            template,
        };
        env.assert_scratch_store();
        env
    }

    /// koto resolves its root to this test's `KOTO_HOME`, inside its own
    /// temporary directory, so no command here reaches a real store.
    fn assert_scratch_store(&self) {
        let out = self.cmd().args(["session", "dir"]).output().unwrap();
        assert!(out.status.success(), "{out:?}");
        let resolved = PathBuf::from(String::from_utf8(out.stdout).unwrap().trim());
        assert_eq!(resolved, self.koto_home);
        assert!(resolved.starts_with(&self.root));
    }

    fn scrub(&self, cmd: &mut std::process::Command) {
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("KOTO_") {
                cmd.env_remove(name);
            }
        }
        cmd.current_dir(&self.work)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env("HOME", &self.home)
            .env("KOTO_HOME", &self.koto_home)
            .env("XDG_CACHE_HOME", &self.cache);
    }

    fn cmd(&self) -> Command {
        let mut cmd = koto_command::koto();
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("KOTO_") {
                cmd.env_remove(name);
            }
        }
        cmd.current_dir(&self.work)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env("HOME", &self.home)
            .env("KOTO_HOME", &self.koto_home)
            .env("XDG_CACHE_HOME", &self.cache)
            .timeout(Duration::from_secs(60));
        cmd
    }

    /// Run `args`, expect success, and return the last JSON line printed.
    fn ok(&self, args: &[&str]) -> Value {
        let out = self.cmd().args(args).output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "koto {args:?} failed ({:?}): {stdout}\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
        serde_json::from_str(last).unwrap_or(Value::Null)
    }

    fn sessions(&self) -> PathBuf {
        self.koto_home.join("sessions")
    }

    fn log(&self, name: &str) -> PathBuf {
        self.sessions()
            .join(name)
            .join(format!("koto-{name}.state.jsonl"))
    }

    fn init(&self, name: &str) {
        self.ok(&["init", name, "--template", self.template.to_str().unwrap()]);
    }

    fn supersede(&self, pred: &str, succ: &str) {
        self.ok(&[
            "init",
            succ,
            "--template",
            self.template.to_str().unwrap(),
            "--continues",
            pred,
            "--supersede",
        ]);
    }

    /// The `session_id` `name`'s header records.
    fn session_id(&self, name: &str) -> String {
        let content = std::fs::read_to_string(self.log(name)).unwrap();
        let header: Value = serde_json::from_str(content.lines().next().unwrap()).unwrap();
        header["session_id"].as_str().unwrap().to_string()
    }

    /// Append a fence to `name`'s log by hand, naming `by` with
    /// `by_session_id`, as a log carrying a broken or cyclic chain would.
    fn force_fence(&self, name: &str, by: &str, by_session_id: &str) {
        let path = self.log(name);
        let mut content = std::fs::read_to_string(&path).unwrap();
        let seq = content.lines().count();
        let event = serde_json::json!({
            "seq": seq,
            "timestamp": "2026-01-01T00:00:00.000Z",
            "type": "session_superseded",
            "payload": {"by": by, "by_session_id": by_session_id, "actor": {}, "at": "2026-01-01T00:00:00Z"},
        });
        content.push_str(&format!("{event}\n"));
        std::fs::write(&path, content).unwrap();
    }

    fn create_request(&self, requested_by: &str) -> String {
        let envelope = self.ok(&[
            "request",
            "create",
            "--with-data",
            r#"{"legs":[{"name":"scope","role":"scope","template":"scope.md","inputs":"brief"}]}"#,
            "--requested-by",
            requested_by,
            "--coordinator-of-record",
            requested_by,
        ]);
        envelope["request_id"].as_str().unwrap().to_string()
    }

    fn resolve(&self, request_id: &str) {
        self.ok(&[
            "request",
            "resolve",
            request_id,
            "scope",
            "--with-data",
            r#"{"status":"success","summary":"ok"}"#,
        ]);
    }

    /// The session's wake cursor right now: a watch with no budget.
    fn cursor(&self, session: &str) -> String {
        let out = self.ok(&[
            "request",
            "watch",
            "--session",
            session,
            "--timeout-secs",
            "0",
        ]);
        assert_eq!(out["woke"], false, "{out}");
        out["cursor"].as_str().unwrap().to_string()
    }

    /// Whether `session`'s wake file changed since `cursor`.
    fn woke_since(&self, session: &str, cursor: &str) -> bool {
        let out = self.ok(&[
            "request",
            "watch",
            "--session",
            session,
            "--timeout-secs",
            "0",
            "--since",
            cursor,
        ]);
        out["woke"].as_bool().unwrap()
    }

    /// The sessions that have a wake file.
    fn rung(&self) -> BTreeSet<String> {
        match std::fs::read_dir(self.koto_home.join("wakes")) {
            Ok(entries) => entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect(),
            Err(_) => BTreeSet::new(),
        }
    }

    fn wake_lines(&self, session: &str) -> usize {
        std::fs::read_to_string(self.koto_home.join("wakes").join(session))
            .map(|c| c.lines().count())
            .unwrap_or(0)
    }

    /// Start a watch for `session` from `since`, in the background.
    fn spawn_watch(&self, session: &str, since: &str) -> Child {
        let mut cmd = koto_command::koto_std();
        self.scrub(&mut cmd);
        cmd.args([
            "request",
            "watch",
            "--session",
            session,
            "--timeout-secs",
            "30",
            "--since",
            since,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
    }
}

fn names(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// Wait for a watch to exit; return its JSON and when it exited.
fn finish_watch(mut child: Child) -> (Value, Instant) {
    let deadline = Instant::now() + Duration::from_secs(40);
    let exited_at = loop {
        if child.try_wait().unwrap().is_some() {
            break Instant::now();
        }
        assert!(Instant::now() < deadline, "the watch never exited");
        std::thread::sleep(Duration::from_millis(5));
    };
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "watch failed: {output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap();
    (serde_json::from_str(last).unwrap(), exited_at)
}

fn request_ids(list: &Value) -> Vec<String> {
    list["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["request_id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_leg_a_opened_two_takeovers_back_wakes_cs_armed_watch() {
    let env = Env::new();
    env.init("a");
    let request = env.create_request("a");
    env.supersede("a", "b");
    env.supersede("b", "c");

    let a = env.cursor("a");
    let b = env.cursor("b");
    let c = env.cursor("c");
    let watch = env.spawn_watch("c", &c);
    // Give the watch time to start polling, so the bound measures the ring
    // and not process start-up.
    std::thread::sleep(Duration::from_millis(500));

    env.resolve(&request);
    let resolved_at = Instant::now();
    let (out, exited_at) = finish_watch(watch);
    assert_eq!(out["woke"], true, "{out}");
    assert_eq!(out["session"], "c", "{out}");
    let latency = exited_at.saturating_duration_since(resolved_at);
    assert!(
        latency <= WAKE_BOUND,
        "c's watch exited {latency:?} after the result, past {WAKE_BOUND:?}"
    );

    assert!(
        env.woke_since("a", &a),
        "a's own wake file must still be rung"
    );
    assert!(
        !env.woke_since("b", &b),
        "b is neither principal nor newest"
    );

    // The request still names the member that opened it.
    let by_a = env.ok(&["request", "list", "--requested-by", "a"]);
    assert_eq!(request_ids(&by_a), vec![request.clone()]);
    assert_eq!(by_a["requests"][0]["requested_by"], "a", "{by_a}");
    let by_c = env.ok(&["request", "list", "--requested-by", "c"]);
    assert!(request_ids(&by_c).is_empty(), "{by_c}");
}

#[test]
fn a_principal_never_superseded_is_rung_as_before() {
    let env = Env::new();
    env.init("solo");
    env.init("bystander");
    let request = env.create_request("solo");
    env.resolve(&request);
    assert_eq!(env.rung(), names(&["solo"]));
    assert_eq!(env.wake_lines("solo"), 1);
}

#[test]
fn a_broken_hop_stops_the_walk_at_the_last_good_member() {
    // b's fence names c with an id that isn't c's.
    let env = Env::new();
    for name in ["a", "b", "c"] {
        env.init(name);
    }
    env.force_fence("a", "b", &env.session_id("b"));
    env.force_fence("b", "c", "not-cs-id");
    let request = env.create_request("a");
    env.resolve(&request);
    assert_eq!(env.rung(), names(&["a", "b"]));

    // b's fence names a session with no log.
    let env = Env::new();
    for name in ["a", "b"] {
        env.init(name);
    }
    env.force_fence("a", "b", &env.session_id("b"));
    env.force_fence("b", "ghost", "id-ghost");
    let request = env.create_request("a");
    env.resolve(&request);
    assert_eq!(env.rung(), names(&["a", "b"]));

    // b's fence names something no path may be built from.
    let env = Env::new();
    for name in ["a", "b"] {
        env.init(name);
    }
    std::fs::create_dir_all(env.koto_home.join("escape")).unwrap();
    env.force_fence("a", "b", &env.session_id("b"));
    env.force_fence("b", "../escape", "id-escape");
    let request = env.create_request("a");
    env.resolve(&request);
    assert_eq!(env.rung(), names(&["a", "b"]));
    assert_eq!(
        std::fs::read_dir(env.koto_home.join("escape"))
            .unwrap()
            .count(),
        0,
        "nothing may be written outside the wake directory"
    );

    // The first hop itself is broken: the principal alone is rung.
    let env = Env::new();
    for name in ["a", "b"] {
        env.init(name);
    }
    env.force_fence("a", "b", "not-bs-id");
    let request = env.create_request("a");
    env.resolve(&request);
    assert_eq!(env.rung(), names(&["a"]));
}

#[test]
fn a_cycle_in_the_fences_does_not_loop() {
    let env = Env::new();
    for name in ["a", "b"] {
        env.init(name);
    }
    env.force_fence("a", "b", &env.session_id("b"));
    env.force_fence("b", "a", &env.session_id("a"));
    let request = env.create_request("a");
    // `ok` runs under a 60 second timeout, so a walk that looped would fail
    // here instead of hanging.
    env.resolve(&request);
    assert_eq!(env.rung(), names(&["a", "b"]));
    assert_eq!(env.wake_lines("a"), 1);
    assert_eq!(env.wake_lines("b"), 1);
}

#[test]
fn the_walk_gives_up_after_1024_hops() {
    let env = Env::new();
    env.init("template-session");
    let content = std::fs::read_to_string(env.log("template-session")).unwrap();
    let header: Value = serde_json::from_str(content.lines().next().unwrap()).unwrap();

    // s0 -> s1 -> ... -> s1025, each fence naming the next one's id.
    let chain: Vec<String> = (0..=MAX_HOPS + 1).map(|i| format!("s{i}")).collect();
    for (i, name) in chain.iter().enumerate() {
        let mut h = header.clone();
        h["workflow"] = Value::from(name.as_str());
        h["session_id"] = Value::from(format!("id-{name}"));
        std::fs::create_dir_all(env.sessions().join(name)).unwrap();
        std::fs::write(env.log(name), format!("{h}\n")).unwrap();
        if i > 0 {
            env.force_fence(&chain[i - 1], name, &format!("id-{name}"));
        }
    }
    let newest = chain.last().unwrap();

    // s1 is exactly 1024 hops from the newest member.
    let request = env.create_request("s1");
    env.resolve(&request);
    assert_eq!(env.rung(), names(&["s1", newest]));

    // s0 is one hop more: the ring reaches the principal only.
    let request = env.create_request("s0");
    env.resolve(&request);
    assert_eq!(env.rung(), names(&["s0", "s1", newest]));
    assert_eq!(env.wake_lines(newest), 1);
    assert_eq!(env.wake_lines("s0"), 1);
}

#[test]
fn the_stale_wake_recovery_reaches_the_newest_member() {
    use koto::engine::audit::requester_woken_fields;
    use koto::engine::claim::format_rfc3339_millis;
    use koto::engine::persistence::append_event;
    use koto::engine::types::{EventPayload, ValidatedSessionId};

    let env = Env::new();
    env.init("a");
    env.init("coord");
    env.init("child");
    env.supersede("a", "b");
    env.supersede("b", "c");

    // coord woke a an hour ago for its child, and nobody heard it.
    let now = SystemTime::now();
    let woken_at = now - Duration::from_secs(3600);
    let payload = EventPayload::EvidenceSubmitted {
        state: "request_store.wake".to_string(),
        fields: requester_woken_fields(&[ValidatedSessionId::new("child").unwrap()], &[0], "a"),
        submitter_cwd: None,
        source: None,
        exported_fields: None,
    };
    append_event(
        &env.log("coord"),
        &payload,
        &format_rfc3339_millis(woken_at),
    )
    .unwrap();

    // c, which drives a's run now, has done nothing since the wake. a's own
    // log was written after it, by the fence.
    let idle = filetime::FileTime::from_system_time(woken_at - Duration::from_secs(60));
    filetime::set_file_mtime(env.log("c"), idle).unwrap();
    let a = env.cursor("a");
    let c = env.cursor("c");

    env.ok(&["next", "coord"]);
    assert!(env.woke_since("c", &c), "the recovery must reach c");
    assert!(env.woke_since("a", &a), "the recovery must still ring a");
}
