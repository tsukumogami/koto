//! Context reads and writers in the session log (DESIGN-koto-failure-reporting.md,
//! Decision 4).
//!
//! Every logged read of a context key appends one `context_read` naming the
//! reader, the state current at the read, whether the key was there and, when
//! it was, the SHA-256 of its content. Every write records its writer, in the
//! log and in the store's manifest. These tests drive the real binary and
//! check both halves, plus the join a log reader performs to name the write a
//! read saw: the latest write of the key below the read whose hash matches.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use assert_fs::TempDir;
use serde_json::Value;
use sha2::{Digest, Sha256};

// ===== Harness =====

fn sessions_base(dir: &Path) -> PathBuf {
    let base = dir.join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn koto_binary() -> PathBuf {
    koto_command::koto_bin()
}

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = koto_command::koto();
    cmd.env_remove("CLAUDE_CODE_SESSION_ID");
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(dir));
    cmd.env("HOME", dir);
    // No hosting Claude Code session: materializing a `/workflows` file would
    // publish a location and add a koto write to every log.
    cmd.env_remove("CLAUDE_CODE_SESSION_ID");
    cmd.env_remove("KOTO_WORKFLOWS_DIR");
    // No reader's anchor unless a test names one, and the default retention.
    for var in [
        "KOTO_ANCHOR",
        "KOTO_DISPATCHER_ANCHOR",
        "KOTO_SESSION_BACKEND",
        "KOTO_RETENTION_KEEP",
        "KOTO_RETENTION_IDLE_PERIOD",
        "KOTO_RETENTION_STALE_AFTER",
    ] {
        cmd.env_remove(var);
    }
    // By default a command runs as the agent session that owns the sessions
    // `init` starts. Its `koto context get` outside the session's own tick
    // writes nothing into the session read: it goes to the read spool. A test
    // that wants the read in the session's log runs it as a command the
    // session's own tick runs ([`run_in_tick`]); one that wants another
    // reader names it per command.
    cmd.env("KOTO_ANCHOR", OWN_ANCHOR);
    // A gate script shells out to this build's `koto`.
    cmd.env(
        "PATH",
        format!(
            "{}:{}",
            koto_binary().parent().unwrap().display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    );
    cmd
}

fn run(dir: &Path, args: &[&str]) -> std::process::Output {
    koto_cmd(dir).args(args).output().unwrap()
}

fn run_ok(dir: &Path, args: &[&str]) -> std::process::Output {
    let out = run(dir, args);
    assert!(
        out.status.success(),
        "koto {args:?} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// Run `args` as a command `session`'s own tick runs (a gate script or a
/// default action): with the tick's marker, `KOTO_TICK_SESSION`, naming
/// `session`, so a read of `session` logs in its log.
fn run_in_tick(dir: &Path, session: &str, args: &[&str]) -> std::process::Output {
    koto_cmd(dir)
        .env("KOTO_TICK_SESSION", session)
        .args(args)
        .output()
        .unwrap()
}

fn run_in_tick_ok(dir: &Path, session: &str, args: &[&str]) -> std::process::Output {
    let out = run_in_tick(dir, session, args);
    assert!(
        out.status.success(),
        "koto {args:?} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// The anchor of the agent session that owns every session [`init`] starts.
const OWN_ANCHOR: &str = "anchor-own";

/// Start `name` from `template` under [`OWN_ANCHOR`], opened first, so the
/// session holds `koto/links/held-by/anchor-own` and the harness's reads of
/// it are its own agent session's.
fn init(dir: &Path, name: &str, template: &str) {
    if !sessions_base(dir).join(OWN_ANCHOR).exists() {
        run_ok(
            dir,
            &[
                "session",
                "anchor",
                "open",
                "--harness-session",
                "own",
                "--source",
                "startup",
            ],
        );
    }
    let src = dir.join(format!("{name}.md"));
    std::fs::write(&src, template).unwrap();
    run_ok(dir, &["init", name, "--template", src.to_str().unwrap()]);
}

fn next(dir: &Path, name: &str, data: Option<&str>) -> Value {
    let mut args = vec!["next", name, "--no-cleanup"];
    if let Some(d) = data {
        args.push("--with-data");
        args.push(d);
    }
    let out = run(dir, &args);
    serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
        panic!(
            "invalid JSON from next: stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn add(dir: &Path, name: &str, key: &str, content: &str) {
    let file = dir.join("add-input.txt");
    std::fs::write(&file, content).unwrap();
    run_ok(
        dir,
        &[
            "context",
            "add",
            name,
            key,
            "--from-file",
            file.to_str().unwrap(),
        ],
    );
}

fn log_path(dir: &Path, name: &str) -> PathBuf {
    sessions_base(dir)
        .join(name)
        .join(format!("koto-{name}.state.jsonl"))
}

fn raw_log(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(log_path(dir, name)).unwrap()
}

/// Every event line of `name`'s log, parsed.
fn events(dir: &Path, name: &str) -> Vec<Value> {
    raw_log(dir, name)
        .lines()
        .skip(1)
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn reads(dir: &Path, name: &str) -> Vec<Value> {
    events(dir, name)
        .into_iter()
        .filter(|e| e["type"] == "context_read")
        .collect()
}

fn manifest_writer(dir: &Path, name: &str, key: &str) -> Option<String> {
    let path = sessions_base(dir)
        .join(name)
        .join("ctx")
        .join("manifest.json");
    let manifest: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let meta = &manifest["keys"][key];
    assert!(meta.is_object(), "{key} is not in the manifest: {manifest}");
    meta.get("writer")
        .and_then(|w| w.as_str())
        .map(str::to_string)
}

fn sha256(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// The writer of the write a read saw, by the documented join: among the
/// writes of the read's key with a `seq` below the read's, the latest whose
/// hash equals the read's hash -- a `context_added`'s `hash`, or the SHA-256
/// of a `transitioned` assignment's string. Its writer is its `writer`,
/// `transition` for an assignment, `unknown` for an event with no `writer`;
/// no match is `unknown` too. Returns the writer and the write's seq.
fn join(events: &[Value], read: &Value) -> (String, Option<u64>) {
    let key = read["payload"]["key"].as_str().unwrap();
    let hash = read["payload"]["hash"].as_str().expect("a present read");
    let seq = read["seq"].as_u64().unwrap();
    for e in events.iter().rev() {
        if e["seq"].as_u64().unwrap() >= seq {
            continue;
        }
        let p = &e["payload"];
        match e["type"].as_str().unwrap() {
            "context_added" if p["key"] == key && p["hash"] == hash => {
                let writer = p["writer"].as_str().unwrap_or("unknown").to_string();
                return (writer, e["seq"].as_u64());
            }
            "transitioned" => {
                if let Some(v) = p["context_assignments"][key].as_str() {
                    if sha256(v) == hash {
                        return ("transition".to_string(), e["seq"].as_u64());
                    }
                }
            }
            _ => {}
        }
    }
    ("unknown".to_string(), None)
}

// ===== Templates =====

/// `check` blocks on a context-exists and a context-matches gate over `note`;
/// `work` takes a marker into a terminal with a result map, or a failure
/// terminal.
const READS_TEMPLATE: &str = r#"---
name: ctx-reads
version: "1.0"
initial_state: check
states:
  check:
    gates:
      has_note:
        type: context-exists
        key: note
      note_ok:
        type: context-matches
        key: note
        pattern: "^ok"
    transitions:
      - target: work
  work:
    accepts:
      marker:
        type: enum
        required: true
        values: [done, fail]
    transitions:
      - target: done
        when:
          marker: done
      - target: failed
        when:
          marker: fail
  done:
    terminal: true
    result:
      pr: "${context.note}"
      gone: "${context.absent_key}"
  failed:
    terminal: true
    failure: true
---

## check

Wait for the note.

## work

Work.

## done

Done.

## failed

Failed.
"#;

/// A transition that assigns two keys, then a state that waits.
const WRITERS_TEMPLATE: &str = r#"---
name: ctx-writers
version: "1.0"
initial_state: start
states:
  start:
    accepts:
      step:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: middle
        when:
          step: go
        context_assignments:
          slot: from-transition
          spare: also-from-transition
  middle:
    accepts:
      step:
        type: enum
        required: true
        values: [go]
    transitions:
      - target: end
        when:
          step: go
  end:
    terminal: true
---

## start

Start.

## middle

Middle.

## end

End.
"#;

const NOTE: &str = "ok, the note content 7f3a";

// ===== Reads =====

#[test]
fn gate_reads_land_immediately_before_their_gate_evaluated() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init(dir, "s", READS_TEMPLATE);

    // Blocked: both gates read an absent key.
    assert_eq!(next(dir, "s", None)["action"], "gate_blocked");
    add(dir, "s", "note", NOTE);
    // Both pass and the session moves on to `work`.
    assert_eq!(next(dir, "s", None)["state"], "work");

    let evs = events(dir, "s");
    let gate_events: Vec<usize> = evs
        .iter()
        .enumerate()
        .filter(|(_, e)| e["type"] == "gate_evaluated")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(gate_events.len(), 4, "{}", raw_log(dir, "s"));
    for i in gate_events {
        let gate = &evs[i]["payload"]["gate"];
        let read = &evs[i - 1];
        assert_eq!(read["type"], "context_read", "{}", raw_log(dir, "s"));
        let p = &read["payload"];
        assert_eq!(p["reader"], "gate");
        assert_eq!(&p["gate"], gate);
        assert_eq!(p["key"], "note");
        assert_eq!(p["state"], "check");
        let want_access = if gate == "has_note" {
            "presence"
        } else {
            "content"
        };
        assert_eq!(p["access"], want_access);
        let passed = evs[i]["payload"]["outcome"] == "passed";
        assert_eq!(p["present"], passed);
        if passed {
            assert_eq!(p["hash"], sha256(NOTE));
        } else {
            assert!(p.get("hash").is_none(), "absent key carries a hash: {p}");
        }
    }
    // One read per recorded evaluation, and nothing else read.
    assert_eq!(reads(dir, "s").len(), 4);
}

#[test]
fn cli_reads_log_reader_access_state_and_hash_but_never_content() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init(dir, "s", READS_TEMPLATE);
    add(dir, "s", "note", NOTE);
    assert_eq!(next(dir, "s", None)["state"], "work");
    let before = reads(dir, "s").len();

    // Read as a command in s's own tick, so the reads log in s.
    let out = run_in_tick_ok(dir, "s", &["context", "get", "s", "note"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), NOTE);
    let out = run_in_tick(dir, "s", &["context", "exists", "s", "note"]);
    assert_eq!(out.status.code(), Some(0));
    // An absent key: get fails as before, exists says no, both are logged.
    let out = run_in_tick(dir, "s", &["context", "get", "s", "missing"]);
    assert!(!out.status.success());
    let out = run_in_tick(dir, "s", &["context", "exists", "s", "missing"]);
    assert_eq!(out.status.code(), Some(1));
    // Keys failing the key grammar log nothing.
    assert!(
        !run_in_tick(dir, "s", &["context", "get", "s", "has space"])
            .status
            .success()
    );
    assert_eq!(
        run_in_tick(dir, "s", &["context", "exists", "s", "../escape"])
            .status
            .code(),
        Some(2)
    );

    let all = reads(dir, "s");
    let new = &all[before..];
    assert_eq!(new.len(), 4, "{}", raw_log(dir, "s"));
    let summary: Vec<(String, String, bool, Option<String>)> = new
        .iter()
        .map(|r| {
            let p = &r["payload"];
            assert_eq!(p["reader"], "cli");
            assert_eq!(p["state"], "work");
            assert!(p.get("gate").is_none());
            (
                p["key"].as_str().unwrap().to_string(),
                p["access"].as_str().unwrap().to_string(),
                p["present"].as_bool().unwrap(),
                p.get("hash").and_then(|h| h.as_str()).map(str::to_string),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            ("note".into(), "content".into(), true, Some(sha256(NOTE))),
            ("note".into(), "presence".into(), true, Some(sha256(NOTE))),
            ("missing".into(), "content".into(), false, None),
            ("missing".into(), "presence".into(), false, None),
        ]
    );
    // No event anywhere carries the content, and no read carries a size.
    assert!(!raw_log(dir, "s").contains(NOTE));
    assert!(all.iter().all(|r| r["payload"].get("size").is_none()));
}

#[test]
fn terminal_result_and_failure_reason_reads_are_logged_once_on_the_recording_path() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();

    // A result map reading one present and one absent key.
    init(dir, "s", READS_TEMPLATE);
    add(dir, "s", "note", NOTE);
    assert_eq!(next(dir, "s", None)["state"], "work");
    let before = reads(dir, "s").len();
    let out = next(dir, "s", Some(r#"{"marker": "done"}"#));
    assert_eq!(out["state"], "done", "{out}");
    let result_reads: Vec<Value> = reads(dir, "s")[before..].to_vec();
    let keys: Vec<(&str, bool)> = result_reads
        .iter()
        .map(|r| {
            let p = &r["payload"];
            assert_eq!(p["reader"], "result");
            assert_eq!(p["state"], "done");
            assert_eq!(p["access"], "content");
            (p["key"].as_str().unwrap(), p["present"].as_bool().unwrap())
        })
        .collect();
    assert_eq!(keys, vec![("absent_key", false), ("note", true)]);
    assert_eq!(result_reads[1]["payload"]["hash"], sha256(NOTE));

    // The status path reads the recorded result and logs nothing, and a
    // later tick on the recorded arrival reads nothing either.
    let count = reads(dir, "s").len();
    run_ok(dir, &["status", "s"]);
    next(dir, "s", None);
    assert_eq!(reads(dir, "s").len(), count);

    // A failure terminal reads the session's failure_reason.
    init(dir, "f", READS_TEMPLATE);
    add(dir, "f", "note", NOTE);
    assert_eq!(next(dir, "f", None)["state"], "work");
    add(dir, "f", "failure_reason", "the build broke");
    let before = reads(dir, "f").len();
    assert_eq!(
        next(dir, "f", Some(r#"{"marker": "fail"}"#))["state"],
        "failed"
    );
    let fr: Vec<Value> = reads(dir, "f")[before..].to_vec();
    assert_eq!(fr.len(), 1, "{}", raw_log(dir, "f"));
    let p = &fr[0]["payload"];
    assert_eq!(p["reader"], "result");
    assert_eq!(p["key"], "failure_reason");
    assert_eq!(p["state"], "failed");
    assert_eq!(p["hash"], sha256("the build broke"));
}

/// A default action that polls its gates re-evaluates them inside the
/// polling loop; only the evaluation the advance loop records logs reads.
#[test]
fn polling_re_evaluations_log_no_reads() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init(
        dir,
        "p",
        r#"---
name: ctx-polling
version: "1.0"
initial_state: wait
states:
  wait:
    default_action:
      command: "true"
      polling:
        interval_secs: 1
        timeout_secs: 1
    gates:
      has_note:
        type: context-exists
        key: note
    transitions:
      - target: done
  done:
    terminal: true
---

## wait

Wait.

## done

Done.
"#,
    );
    let gate_evaluated = |dir: &Path| {
        events(dir, "p")
            .iter()
            .filter(|e| e["type"] == "gate_evaluated")
            .count()
    };

    // The loop evaluates the gate twice and times out: nothing is recorded,
    // so nothing is read into the log.
    next(dir, "p", None);
    assert_eq!(gate_evaluated(dir), 0, "{}", raw_log(dir, "p"));
    assert_eq!(reads(dir, "p").len(), 0, "{}", raw_log(dir, "p"));

    // With the note there, the loop's own evaluation passes and is dropped;
    // the advance loop's recorded evaluation logs the one read.
    add(dir, "p", "note", NOTE);
    assert_eq!(next(dir, "p", None)["state"], "done");
    assert_eq!(gate_evaluated(dir), 1, "{}", raw_log(dir, "p"));
    let r = reads(dir, "p");
    assert_eq!(r.len(), 1, "{}", raw_log(dir, "p"));
    assert_eq!(r[0]["payload"]["reader"], "gate");
}

// ===== Writers =====

#[test]
fn writers_are_recorded_in_the_log_and_the_manifest() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init(dir, "w", WRITERS_TEMPLATE);
    assert_eq!(next(dir, "w", Some(r#"{"step": "go"}"#))["state"], "middle");

    // A transition's assignment: the manifest names it, and the log's record
    // is the `transitioned` event itself, not a `context_added`.
    assert_eq!(
        manifest_writer(dir, "w", "slot").as_deref(),
        Some("transition")
    );
    // (The held-by link `init` records under the harness's anchor is koto's
    // own write, under `koto/`, and isn't one of these.)
    let own_key = |e: &Value| {
        e["payload"]["key"]
            .as_str()
            .is_some_and(|k| k.starts_with("koto/"))
    };
    assert!(events(dir, "w")
        .iter()
        .all(|e| e["type"] != "context_added" || own_key(e)));

    // koto context add and remove: agent.
    add(dir, "w", "extra", "agent text");
    assert_eq!(manifest_writer(dir, "w", "extra").as_deref(), Some("agent"));
    run_ok(dir, &["context", "remove", "w", "extra"]);
    let evs = events(dir, "w");
    let added = evs
        .iter()
        .find(|e| e["type"] == "context_added" && !own_key(e))
        .unwrap();
    assert_eq!(added["payload"]["writer"], "agent");
    let removed = evs.iter().find(|e| e["type"] == "context_removed").unwrap();
    assert_eq!(removed["payload"]["writer"], "agent");

    // A lost assigned value is answered from the log and not stored; the
    // next tick's reconcile restores it as `transition`.
    let ctx = sessions_base(dir).join("w").join("ctx");
    std::fs::remove_file(ctx.join("spare")).unwrap();
    let manifest_path = ctx.join("manifest.json");
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["keys"].as_object_mut().unwrap().remove("spare");
    std::fs::write(&manifest_path, manifest.to_string()).unwrap();
    let out = run_ok(dir, &["context", "get", "w", "spare"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "also-from-transition");
    assert!(!ctx.join("spare").exists());
    assert_eq!(next(dir, "w", None)["state"], "middle");
    assert_eq!(
        manifest_writer(dir, "w", "spare").as_deref(),
        Some("transition")
    );
    // A read of the repaired key joins to the assignment.
    run_in_tick_ok(dir, "w", &["context", "get", "w", "spare"]);
    let evs = events(dir, "w");
    let read = evs
        .iter()
        .rev()
        .find(|e| e["type"] == "context_read")
        .unwrap();
    assert_eq!(join(&evs, read).0, "transition");

    // The published /workflows location: koto, logged.
    let wf = dir.join("workflows-dir");
    run_ok(
        dir,
        &[
            "workflows",
            "publish",
            "--session",
            "w",
            "--dir",
            wf.to_str().unwrap(),
        ],
    );
    let key = "workflows/publish-location";
    assert_eq!(manifest_writer(dir, "w", key).as_deref(), Some("koto"));
    let published: Vec<Value> = events(dir, "w")
        .into_iter()
        .filter(|e| e["type"] == "context_added" && e["payload"]["key"] == key)
        .collect();
    assert_eq!(published.len(), 1);
    assert_eq!(published[0]["payload"]["writer"], "koto");
    assert_eq!(
        published[0]["payload"]["hash"],
        sha256(wf.to_str().unwrap())
    );
}

#[test]
fn a_read_after_an_add_over_a_transition_value_joins_to_the_add() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init(dir, "w", WRITERS_TEMPLATE);
    assert_eq!(next(dir, "w", Some(r#"{"step": "go"}"#))["state"], "middle");

    // Read the transition's value, overwrite it, read again, each read as a
    // command in w's own tick so it logs in w.
    run_in_tick_ok(dir, "w", &["context", "get", "w", "slot"]);
    add(dir, "w", "slot", "from the agent");
    run_in_tick_ok(dir, "w", &["context", "get", "w", "slot"]);

    let evs = events(dir, "w");
    let slot_reads: Vec<&Value> = evs
        .iter()
        .filter(|e| e["type"] == "context_read" && e["payload"]["key"] == "slot")
        .collect();
    assert_eq!(slot_reads.len(), 2);
    let (first_writer, first_seq) = join(&evs, slot_reads[0]);
    assert_eq!(first_writer, "transition");
    let transition_seq = evs
        .iter()
        .find(|e| e["type"] == "transitioned" && e["payload"]["to"] == "middle")
        .and_then(|e| e["seq"].as_u64());
    assert_eq!(first_seq, transition_seq);

    let (second_writer, second_seq) = join(&evs, slot_reads[1]);
    assert_eq!(second_writer, "agent");
    let add_seq = evs
        .iter()
        .find(|e| e["type"] == "context_added" && e["payload"]["key"] == "slot")
        .and_then(|e| e["seq"].as_u64());
    assert_eq!(second_seq, add_seq);
    assert_eq!(slot_reads[1]["payload"]["hash"], sha256("from the agent"));
}

/// A key written by koto v0.14.1 has no writer in the manifest or on its
/// `context_added`: it reads without error and joins as writer unknown.
#[test]
fn a_key_from_an_older_koto_reads_as_writer_unknown() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init(dir, "w", WRITERS_TEMPLATE);

    // What v0.14.1 leaves behind: content, a manifest entry with no writer,
    // and a context_added with no writer.
    let content = "written by an older koto";
    let ctx = sessions_base(dir).join("w").join("ctx");
    std::fs::create_dir_all(&ctx).unwrap();
    std::fs::write(ctx.join("legacy"), content).unwrap();
    let manifest = serde_json::json!({"keys": {"legacy": {
        "created_at": "2026-01-01T00:00:00Z",
        "size": content.len(),
        "hash": sha256(content),
    }}});
    std::fs::write(ctx.join("manifest.json"), manifest.to_string()).unwrap();
    let last_seq = events(dir, "w").last().unwrap()["seq"].as_u64().unwrap();
    let line = serde_json::json!({
        "seq": last_seq + 1,
        "timestamp": "2026-01-01T00:00:01Z",
        "type": "context_added",
        "payload": {"key": "legacy", "hash": sha256(content), "size": content.len()},
    });
    let mut log = raw_log(dir, "w");
    log.push_str(&line.to_string());
    log.push('\n');
    std::fs::write(log_path(dir, "w"), log).unwrap();

    let out = run_in_tick_ok(dir, "w", &["context", "get", "w", "legacy"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), content);
    assert_eq!(
        run_in_tick(dir, "w", &["context", "exists", "w", "legacy"])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(manifest_writer(dir, "w", "legacy"), None);

    let evs = events(dir, "w");
    let read = evs.iter().find(|e| e["type"] == "context_read").unwrap();
    assert_eq!(
        join(&evs, read),
        ("unknown".to_string(), Some(last_seq + 1))
    );
    // The presence read took its hash from the writer-less manifest entry.
    let presence = evs
        .iter()
        .find(|e| e["type"] == "context_read" && e["payload"]["access"] == "presence")
        .unwrap();
    assert_eq!(presence["payload"]["hash"], sha256(content));
}

// ===== A gate script reading context during a batch tick =====

/// A batch parent's tick holds the state-file lock for the whole tick. A gate
/// script on that state calling `koto context get` appends its read under the
/// separate append lock, so it completes rather than waiting on the tick that
/// is running it.
#[test]
fn a_gate_script_reading_context_during_a_batch_tick_does_not_block() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    std::fs::write(
        dir.join("child.md"),
        "---\nname: c\nversion: \"1.0\"\ninitial_state: done\nstates:\n  done:\n    terminal: true\n---\n\n## done\n\nDone.\n",
    )
    .unwrap();
    init(
        dir,
        "parent",
        r#"---
name: batch-peek
version: "1.0"
initial_state: plan
states:
  plan:
    accepts:
      tasks:
        type: tasks
        required: true
    gates:
      done:
        type: children-complete
      peek:
        type: command
        timeout: 20
        command: "koto context get parent note > peek.out; echo $? > peek.code"
    materialize_children:
      from_field: tasks
      default_template: child.md
    transitions:
      - target: closed
        when:
          gates.done.all_complete: true
  closed:
    terminal: true
---

## plan

Plan.

## closed

Closed.
"#,
    );
    add(dir, "parent", "note", NOTE);

    let started = std::time::Instant::now();
    next(dir, "parent", None);
    assert!(started.elapsed() < std::time::Duration::from_secs(15));

    assert_eq!(
        std::fs::read_to_string(dir.join("peek.code"))
            .unwrap()
            .trim(),
        "0"
    );
    assert_eq!(std::fs::read_to_string(dir.join("peek.out")).unwrap(), NOTE);
    let evs = events(dir, "parent");
    let peek = evs
        .iter()
        .find(|e| e["type"] == "gate_evaluated" && e["payload"]["gate"] == "peek")
        .unwrap();
    assert_eq!(
        peek["payload"]["outcome"],
        "passed",
        "{}",
        raw_log(dir, "parent")
    );
    // The script's own read appears as the CLI's, before the gate's record.
    let cli_read = evs
        .iter()
        .find(|e| e["type"] == "context_read" && e["payload"]["reader"] == "cli")
        .expect("the gate script's read was logged");
    assert!(cli_read["seq"].as_u64() < peek["seq"].as_u64());
    assert_eq!(cli_read["payload"]["hash"], sha256(NOTE));
}

// ===== A read that can't be logged =====

/// A log the process can read but not append to: `koto context get` and
/// `koto context exists` answer exactly as before and warn on stderr that the
/// read went unrecorded.
#[test]
fn an_unwritable_log_warns_and_reads_answer_as_before() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    init(dir, "s", READS_TEMPLATE);
    add(dir, "s", "note", NOTE);
    let log = log_path(dir, "s");
    let before = raw_log(dir, "s");
    std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o444)).unwrap();
    if std::fs::OpenOptions::new().append(true).open(&log).is_ok() {
        // Running as a user permissions don't bind (root): nothing to show.
        return;
    }

    // Read as a command in s's own tick, whose reads go to s's log.
    let out = run_in_tick(dir, "s", &["context", "get", "s", "note"]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), NOTE);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("warning: failed to record context_read"),
        "{stderr}"
    );
    let out = run_in_tick(dir, "s", &["context", "exists", "s", "note"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).contains("warning: failed to record"));

    std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(raw_log(dir, "s"), before);
}

// ===== Reads from another agent session (DESIGN-session-retention.md, Decision 5) =====

fn run_env(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
    let mut cmd = koto_cmd(dir);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.args(args).output().unwrap()
}

fn run_env_ok(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
    let out = run_env(dir, args, env);
    assert!(
        out.status.success(),
        "koto {args:?} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// Open the anchor of harness session `harness`, `anchor-<harness>`.
fn open_anchor(dir: &Path, harness: &str) {
    run_env_ok(
        dir,
        &[
            "session",
            "anchor",
            "open",
            "--harness-session",
            harness,
            "--source",
            "startup",
        ],
        &[],
    );
}

type Hashes = std::collections::BTreeMap<PathBuf, String>;
type StoreHashes = std::collections::BTreeMap<String, Hashes>;

/// SHA-256 of every file under session `name`'s directory, by path.
fn session_hashes(dir: &Path, name: &str) -> Hashes {
    let root = sessions_base(dir).join(name);
    let mut out = Hashes::new();
    let mut stack = vec![root.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let digest = hex::encode(Sha256::digest(std::fs::read(&p).unwrap()));
                out.insert(p.strip_prefix(&root).unwrap().to_path_buf(), digest);
            }
        }
    }
    out
}

/// Every session directory's files, hashed: the whole store but its
/// top-level files (the run journal) and dot-directories (stored plans).
fn store_hashes(dir: &Path) -> StoreHashes {
    let mut out = StoreHashes::new();
    for e in std::fs::read_dir(sessions_base(dir)).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if e.path().is_dir() && !name.starts_with('.') {
            out.insert(name.clone(), session_hashes(dir, &name));
        }
    }
    out
}

/// The `context_read` events in `name`'s log, as (key, access, session).
fn read_rows(dir: &Path, name: &str) -> Vec<(String, String, Option<String>)> {
    reads(dir, name)
        .iter()
        .map(|r| {
            let p = &r["payload"];
            (
                p["key"].as_str().unwrap().to_string(),
                p["access"].as_str().unwrap_or("content").to_string(),
                p["session"].as_str().map(str::to_string),
            )
        })
        .collect()
}

/// A workspace with two agent sessions' anchors: `anchor-H`, which opened
/// session `s` (so `s` holds `koto/links/held-by/anchor-H`), and
/// `anchor-R`, another agent session's. `s` waits in `check` with `note`.
fn two_agent_sessions(dir: &Path) {
    open_anchor(dir, "H");
    open_anchor(dir, "R");
    let src = dir.join("s.md");
    std::fs::write(&src, READS_TEMPLATE).unwrap();
    run_env_ok(
        dir,
        &["init", "s", "--template", src.to_str().unwrap()],
        &[("KOTO_ANCHOR", "anchor-H")],
    );
    assert!(sessions_base(dir)
        .join("s/ctx/koto/links/held-by/anchor-H")
        .is_file());
    add(dir, "s", "note", NOTE);
}

#[test]
fn a_read_from_another_agent_session_is_recorded_in_the_readers_anchor() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    two_agent_sessions(dir);
    let before = store_hashes(dir);
    let reader = [("KOTO_ANCHOR", "anchor-R")];

    let out = run_env_ok(dir, &["context", "get", "s", "note"], &reader);
    assert_eq!(String::from_utf8_lossy(&out.stdout), NOTE);
    run_env_ok(dir, &["context", "exists", "s", "note"], &reader);
    let out = run_env(dir, &["context", "exists", "s", "absent"], &reader);
    assert_eq!(out.status.code(), Some(1));

    // s and anchor-H are byte-identical; only anchor-R changed.
    let after = store_hashes(dir);
    for (name, hashes) in &before {
        if name != "anchor-R" {
            assert_eq!(&after[name], hashes, "{name} changed");
        }
    }
    let s = Some("s".to_string());
    assert_eq!(
        read_rows(dir, "anchor-R"),
        vec![
            ("note".into(), "content".into(), s.clone()),
            ("note".into(), "presence".into(), s.clone()),
            ("absent".into(), "presence".into(), s),
        ]
    );
    let r = reads(dir, "anchor-R");
    assert_eq!(r[0]["payload"]["reader"], "cli");
    assert_eq!(
        r[0]["payload"]["state"], "check",
        "the read session's state"
    );
    assert_eq!(r[0]["payload"]["hash"], sha256(NOTE));
    assert_eq!(r[1]["payload"]["hash"], sha256(NOTE));
    assert!(r[2]["payload"].get("hash").is_none());
}

#[test]
fn listing_and_judging_commands_from_another_agent_session_write_nothing() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    two_agent_sessions(dir);
    let reader = [("KOTO_ANCHOR", "anchor-R")];
    let before = store_hashes(dir);
    for args in [
        &["context", "list", "s"][..],
        &["status", "s"][..],
        &["session", "list"][..],
        &["session", "hierarchy", "s", "--json"][..],
        &["session", "prune"][..],
    ] {
        run_env_ok(dir, args, &reader);
        assert_eq!(store_hashes(dir), before, "{args:?} wrote a session");
    }
}

/// A read by the session's own agent session (its `KOTO_ANCHOR` holds the
/// session) outside the session's tick writes nothing into the session or
/// the anchor: it goes to the read spool. A gate in the session's own tick
/// still logs its reads in the session.
#[test]
fn an_own_anchors_read_outside_the_tick_is_spooled_and_a_gate_read_logs_in_the_session() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    two_agent_sessions(dir);
    let own = [("KOTO_ANCHOR", "anchor-H")];
    let before = store_hashes(dir);

    run_env_ok(dir, &["context", "get", "s", "note"], &own);
    run_env_ok(dir, &["context", "exists", "s", "note"], &own);
    let after = store_hashes(dir);
    for (name, hashes) in &before {
        assert_eq!(
            without_spool(after[name].clone()),
            without_spool(hashes.clone()),
            "{name} changed"
        );
    }
    assert!(read_rows(dir, "s").is_empty());
    assert_eq!(
        spooled_rows(dir, "s"),
        vec![
            ("note".into(), "content".into(), None),
            ("note".into(), "presence".into(), None),
        ]
    );

    // s's own tick under its own anchor: the gates' reads go to s.
    run_env_ok(dir, &["next", "s", "--no-cleanup"], &own);
    let gate_reads: Vec<Value> = reads(dir, "s")
        .into_iter()
        .filter(|r| r["payload"]["reader"] == "gate")
        .collect();
    assert_eq!(gate_reads.len(), 2, "{gate_reads:?}");
    assert!(gate_reads
        .iter()
        .all(|r| r["payload"].get("session").is_none()));
    assert!(reads(dir, "anchor-H").is_empty());
    assert!(reads(dir, "anchor-R").is_empty());

    // An anchor reading itself outside its tick is spooled too.
    let anchor_before = without_spool(session_hashes(dir, "anchor-H"));
    let out = run_env(dir, &["context", "exists", "anchor-H", "nothing"], &own);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        without_spool(session_hashes(dir, "anchor-H")),
        anchor_before
    );
    assert!(read_rows(dir, "anchor-H").is_empty());
    assert_eq!(
        spooled_rows(dir, "anchor-H"),
        vec![("nothing".into(), "presence".into(), None)]
    );
}

/// The read spool's file name beside a session.
const SPOOL: &str = "context-reads.jsonl";

/// `name`'s file hashes but its read spool's.
fn without_spool(hashes: Hashes) -> Hashes {
    hashes
        .into_iter()
        .filter(|(p, _)| !p.ends_with(SPOOL))
        .collect()
}

/// The reads spooled beside `name`, as (key, access, session), from the
/// spool's lines.
fn spooled_rows(dir: &Path, name: &str) -> Vec<(String, String, Option<String>)> {
    std::fs::read_to_string(sessions_base(dir).join(name).join(SPOOL))
        .unwrap_or_default()
        .lines()
        .map(|l| {
            let v: Value = serde_json::from_str(l).unwrap();
            assert_eq!(v["type"], "context_read", "{l}");
            assert!(v["timestamp"].is_string(), "{l}");
            let p = &v["payload"];
            (
                p["key"].as_str().unwrap().to_string(),
                p["access"].as_str().unwrap_or("content").to_string(),
                p["session"].as_str().map(str::to_string),
            )
        })
        .collect()
}

#[test]
fn with_no_reader_anchor_a_read_goes_to_the_spool_and_the_session_stays_byte_identical() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    two_agent_sessions(dir);
    let before = store_hashes(dir);
    run_env_ok(dir, &["context", "get", "s", "note"], &[]);
    run_env_ok(
        dir,
        &["context", "get", "s", "note"],
        &[("KOTO_ANCHOR", "anchor-nobody")],
    );
    // An ordinary session's name is no anchor either, the read one included.
    run_env_ok(
        dir,
        &["context", "exists", "s", "note"],
        &[("KOTO_ANCHOR", "s")],
    );
    // Nothing of s changed but its spool, and no anchor changed at all.
    let after = store_hashes(dir);
    for (name, hashes) in &before {
        assert_eq!(
            without_spool(after[name].clone()),
            without_spool(hashes.clone()),
            "{name} changed"
        );
    }
    for anchor in ["anchor-H", "anchor-R"] {
        assert_eq!(after[anchor], before[anchor], "{anchor} changed");
    }
    assert!(read_rows(dir, "s").is_empty(), "a read reached s's log");
    assert_eq!(
        spooled_rows(dir, "s"),
        vec![
            ("note".into(), "content".into(), None),
            ("note".into(), "content".into(), None),
            ("note".into(), "presence".into(), None),
        ]
    );
}

/// Spooled reads are journal input only: an anchorless read of the session
/// an anchor holds, of the anchor itself and of another anchor moves no
/// anchor's last activity. With a two second idle period, each anchor
/// idles out exactly two seconds after its own last event.
#[test]
fn a_spooled_read_counts_as_no_sessions_activity() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    two_agent_sessions(dir);
    let last_of = |name: &str| {
        millis(
            events(dir, name).last().unwrap()["timestamp"]
                .as_str()
                .unwrap(),
        )
    };
    let lasts = [
        ("anchor-H", last_of("anchor-H")),
        ("anchor-R", last_of("anchor-R")),
    ];
    sleep_until(lasts[1].1 + 800);
    for (name, key) in [
        ("s", "note"),
        ("anchor-H", "nothing"),
        ("anchor-R", "nothing"),
    ] {
        let _ = run_env(dir, &["context", "get", name, key], &[]);
        let _ = run_env(dir, &["context", "exists", name, key], &[]);
    }
    assert_eq!(spooled_rows(dir, "anchor-H").len(), 2);
    assert_eq!(spooled_rows(dir, "anchor-R").len(), 2);
    assert_eq!(spooled_rows(dir, "s").len(), 2);
    for (name, last) in lasts {
        assert_eq!(last_of(name), last, "{name}'s log moved");
    }

    sleep_until(lasts[1].1 + 2300);
    let idle = [("KOTO_RETENTION_IDLE_PERIOD", "2s")];
    for (name, last) in lasts {
        let out = run_env_ok(dir, &["session", "hierarchy", name, "--json"], &idle);
        let h: Value = serde_json::from_slice(&out.stdout).unwrap();
        let m = member(&h, name);
        assert_eq!(m["alive"], false, "{h:#}");
        assert_eq!(m["close_kind"], "idle");
        assert_eq!(
            millis(m["stopped_at"].as_str().unwrap()),
            last + 2000,
            "{name}"
        );
    }
}

#[test]
fn a_failed_append_to_the_readers_anchor_warns_and_the_read_answers() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    two_agent_sessions(dir);
    let log = log_path(dir, "anchor-R");
    std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o444)).unwrap();
    if std::fs::OpenOptions::new().append(true).open(&log).is_ok() {
        // Running as a user permissions don't bind (root): nothing to show.
        return;
    }
    let before = store_hashes(dir);
    let reader = [("KOTO_ANCHOR", "anchor-R")];

    let out = run_env(dir, &["context", "get", "s", "note"], &reader);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), NOTE);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("warning: failed to record context_read for context key \"note\""),
        "{stderr}"
    );
    let out = run_env(dir, &["context", "exists", "s", "note"], &reader);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).contains("warning: failed to record"));

    std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o644)).unwrap();
    // Nothing was written: the read session and the other anchor are
    // byte-identical, and the reader's anchor holds the same files but for
    // the lock its refused append took.
    let after = store_hashes(dir);
    for (name, hashes) in &before {
        let unlocked = |h: &Hashes| -> Hashes {
            h.iter()
                .filter(|(p, _)| !p.to_string_lossy().ends_with(".lock"))
                .map(|(p, d)| (p.clone(), d.clone()))
                .collect()
        };
        if name == "anchor-R" {
            assert_eq!(unlocked(&after[name]), unlocked(hashes), "{name} changed");
        } else {
            assert_eq!(&after[name], hashes, "{name} changed");
        }
    }
}

/// Milliseconds since the epoch of a UTC timestamp as koto writes them
/// (`YYYY-MM-DDTHH:MM:SS.mmmZ`).
fn millis(ts: &str) -> i64 {
    let n = |r: std::ops::Range<usize>| ts[r].parse::<i64>().unwrap();
    let (y, m, d) = (n(0..4), n(5..7), n(8..10));
    let (hh, mm, ss) = (n(11..13), n(14..16), n(17..19));
    let ms = if ts.as_bytes().get(19) == Some(&b'.') {
        n(20..23)
    } else {
        0
    };
    // Days from the civil date.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    (days * 86_400 + hh * 3600 + mm * 60 + ss) * 1000 + ms
}

/// The named member of a `koto session hierarchy --json` answer.
fn member<'a>(h: &'a Value, name: &str) -> &'a Value {
    h["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == name)
        .unwrap_or_else(|| panic!("{name} isn't a member: {h:#}"))
}

/// Sleep until the clock passes `at_ms` (milliseconds since the epoch).
fn sleep_until(at_ms: i64) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    if at_ms > now {
        std::thread::sleep(std::time::Duration::from_millis((at_ms - now) as u64));
    }
}

/// A read record is activity of the reader's anchor only. With a four
/// second idle period, the anchor holding `s` idles out four seconds after
/// its own last event, the read from `anchor-R` notwithstanding, while
/// `anchor-R`, opened at the same moment, stays open past that and then
/// stops exactly four seconds after the read.
#[test]
fn a_read_record_is_activity_of_the_readers_anchor_and_never_of_the_holder() {
    const IDLE_MS: i64 = 4000;
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let idle = [("KOTO_RETENTION_IDLE_PERIOD", "4s")];
    two_agent_sessions(dir);
    let last_of = |name: &str| {
        millis(
            events(dir, name).last().unwrap()["timestamp"]
                .as_str()
                .unwrap(),
        )
    };
    let holder_last = last_of("anchor-H");
    sleep_until(holder_last + 1800);
    run_env_ok(
        dir,
        &["context", "get", "s", "note"],
        &[("KOTO_ANCHOR", "anchor-R")],
    );
    let read_at = millis(reads(dir, "anchor-R")[0]["timestamp"].as_str().unwrap());
    assert_eq!(last_of("anchor-R"), read_at);
    assert_eq!(
        last_of("anchor-H"),
        holder_last,
        "the read wrote the holder"
    );

    let hierarchy = |name: &str| -> Value {
        let out = run_env_ok(dir, &["session", "hierarchy", name, "--json"], &idle);
        serde_json::from_slice(&out.stdout).unwrap()
    };
    // Past the holder's idle close, before the reader's.
    sleep_until(holder_last + IDLE_MS + 300);
    let h = hierarchy("s");
    let holder = member(&h, "anchor-H");
    assert_eq!(holder["alive"], false, "{h:#}");
    assert_eq!(holder["close_kind"], "idle");
    assert_eq!(
        millis(holder["stopped_at"].as_str().unwrap()),
        holder_last + IDLE_MS
    );
    // s runs on.
    assert_eq!(member(&h, "s")["alive"], true);
    // The reader, opened with the holder, is still open: its read moved its
    // last activity.
    let r = hierarchy("anchor-R");
    assert_eq!(member(&r, "anchor-R")["alive"], true, "{r:#}");

    sleep_until(read_at + IDLE_MS + 300);
    let r = hierarchy("anchor-R");
    let reader = member(&r, "anchor-R");
    assert_eq!(reader["alive"], false, "{r:#}");
    assert_eq!(reader["close_kind"], "idle");
    assert_eq!(
        millis(reader["stopped_at"].as_str().unwrap()),
        read_at + IDLE_MS
    );
}

/// The run journal: a read from another agent session during one of s's
/// visits adds no context read to s's visit, and is journaled against s,
/// with the reader's anchor as `koto.context.read_by`, when the anchor
/// records it. An anchorless read in the same visit is spooled, and s's
/// visit sum reads it through the spool, merged by time with the gates'
/// reads from the log.
#[test]
fn the_run_journal_attributes_an_anchors_read_record_to_the_session_it_names() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    two_agent_sessions(dir);
    run_env_ok(
        dir,
        &["context", "get", "s", "note"],
        &[("KOTO_ANCHOR", "anchor-R")],
    );
    run_env_ok(dir, &["context", "exists", "s", "note"], &[]);
    assert_eq!(
        spooled_rows(dir, "s"),
        vec![("note".into(), "presence".into(), None)]
    );
    // s's check visit ends: its gates pass and it moves to `work`.
    let v = next(dir, "s", None);
    assert_eq!(v["state"], "work", "{v:#}");

    let journal: Vec<Value> =
        std::fs::read_to_string(sessions_base(dir).join("_run_journal.jsonl"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
    let s_reads: Vec<&Value> = journal
        .iter()
        .filter(|r| r["kind"] == "context_reads" && r["session"] == "s")
        .collect();
    let (by_anchor, fold): (Vec<&Value>, Vec<&Value>) = s_reads
        .iter()
        .partition(|r| r.get("koto.context.read_by").is_some());
    assert_eq!(by_anchor.len(), 1, "{s_reads:#?}");
    let r = by_anchor[0];
    assert_eq!(r["koto.context.read_by"], "anchor-R");
    assert_eq!(r["koto.context.key"], "note");
    assert_eq!(r["koto.context.reader"], "cli");
    assert_eq!(r["koto.state"], "check");
    assert_eq!(r["koto.context.first_hash"], sha256(NOTE));
    let header: Value = serde_json::from_str(raw_log(dir, "s").lines().next().unwrap()).unwrap();
    assert_eq!(r["koto.session.id"], header["session_id"]);
    assert!(r.get("koto.seq").is_none());

    // s's own sum of the check visit: the spooled anchorless read first (it
    // came before the tick), then its gates' reads from the log, and never
    // the read the anchor recorded.
    assert!(fold.len() >= 2, "{fold:#?}");
    assert_eq!(fold[0]["koto.context.reader"], "cli", "{fold:#?}");
    assert_eq!(fold[0]["koto.context.access"], "presence");
    assert_eq!(fold[0]["koto.context.read_count"], 1);
    assert!(
        fold[1..].iter().all(|r| r["koto.context.reader"] == "gate"),
        "{fold:#?}"
    );
    // The anchor's log holds the read, naming s and the key.
    assert_eq!(
        read_rows(dir, "anchor-R"),
        vec![("note".into(), "content".into(), Some("s".into()))]
    );
    assert!(journal
        .iter()
        .all(|r| !(r["kind"] == "context_reads" && r["session"] == "anchor-R")));
}
