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
    PathBuf::from(env!("CARGO_BIN_EXE_koto"))
}

fn koto_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("koto").unwrap();
    cmd.current_dir(dir);
    cmd.env("KOTO_SESSIONS_BASE", sessions_base(dir));
    cmd.env("HOME", dir);
    // No hosting Claude Code session: materializing a `/workflows` file would
    // publish a location and add a koto write to every log.
    cmd.env_remove("CLAUDE_CODE_SESSION_ID");
    cmd.env_remove("KOTO_WORKFLOWS_DIR");
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

fn init(dir: &Path, name: &str, template: &str) {
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

    let out = run_ok(dir, &["context", "get", "s", "note"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), NOTE);
    let out = run(dir, &["context", "exists", "s", "note"]);
    assert_eq!(out.status.code(), Some(0));
    // An absent key: get fails as before, exists says no, both are logged.
    let out = run(dir, &["context", "get", "s", "missing"]);
    assert!(!out.status.success());
    let out = run(dir, &["context", "exists", "s", "missing"]);
    assert_eq!(out.status.code(), Some(1));
    // Keys failing the key grammar log nothing.
    assert!(!run(dir, &["context", "get", "s", "has space"])
        .status
        .success());
    assert_eq!(
        run(dir, &["context", "exists", "s", "../escape"])
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
    assert!(events(dir, "w")
        .iter()
        .all(|e| e["type"] != "context_added"));

    // koto context add and remove: agent.
    add(dir, "w", "extra", "agent text");
    assert_eq!(manifest_writer(dir, "w", "extra").as_deref(), Some("agent"));
    run_ok(dir, &["context", "remove", "w", "extra"]);
    let evs = events(dir, "w");
    let added = evs.iter().find(|e| e["type"] == "context_added").unwrap();
    assert_eq!(added["payload"]["writer"], "agent");
    let removed = evs.iter().find(|e| e["type"] == "context_removed").unwrap();
    assert_eq!(removed["payload"]["writer"], "agent");

    // A reconcile repair restores the transition's value as `transition`.
    let ctx = sessions_base(dir).join("w").join("ctx");
    std::fs::remove_file(ctx.join("spare")).unwrap();
    let manifest_path = ctx.join("manifest.json");
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["keys"].as_object_mut().unwrap().remove("spare");
    std::fs::write(&manifest_path, manifest.to_string()).unwrap();
    let out = run_ok(dir, &["context", "get", "w", "spare"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "also-from-transition");
    assert_eq!(
        manifest_writer(dir, "w", "spare").as_deref(),
        Some("transition")
    );
    // The repaired read joins to the assignment.
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

    // Read the transition's value, overwrite it, read again.
    run_ok(dir, &["context", "get", "w", "slot"]);
    add(dir, "w", "slot", "from the agent");
    run_ok(dir, &["context", "get", "w", "slot"]);

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

    let out = run_ok(dir, &["context", "get", "w", "legacy"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), content);
    assert_eq!(
        run(dir, &["context", "exists", "w", "legacy"])
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

    let out = run(dir, &["context", "get", "s", "note"]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), NOTE);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("warning: failed to record context_read"),
        "{stderr}"
    );
    let out = run(dir, &["context", "exists", "s", "note"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).contains("warning: failed to record"));

    std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(raw_log(dir, "s"), before);
}
