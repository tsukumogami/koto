//! The retention engine's cost bound (DESIGN-session-retention.md,
//! Decision 3): on a generated store of 2,000 sessions in 200 hierarchies
//! of 10 sessions at most 4 deep, whose anchors carry large logs of read
//! records, `koto session hierarchy` for one session answers within one
//! second, and a prune-preview-shaped pass (one scan, then a judgment of
//! every component) finishes within ten.
//!
//! The bounds hold for a release build, which CI runs in its own job
//! (`cargo test --release --test retention_cost`). A debug build runs the
//! same code on a store a tenth the size and checks only that every
//! verdict comes out as the fixture intends, so the default suite stays
//! fast.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use koto::session::context::ContextStore;
use koto::session::hierarchy::{Graph, Judge, RetentionSettings, Scope};
use koto::session::local::LocalBackend;
use serde_json::json;

const TEMPLATE: &str = r#"---
name: scope
version: "1.0"
initial_state: work
states:
  work:
    accepts:
      status:
        type: string
        required: true
    transitions:
      - target: done
        when:
          status: done
  done:
    terminal: true
---

## work

Work.

## done

Done.
"#;

const RELEASE: bool = !cfg!(debug_assertions);

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    hierarchies: usize,
}

fn iso(t: SystemTime) -> String {
    koto::engine::types::iso8601_at(t)
}

/// Write one session's log: header, then the events.
fn write_log(base: &Path, name: &str, header_extra: serde_json::Value, events: &[String]) {
    let dir = base.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let mut header = json!({
        "schema_version": 1,
        "workflow": name,
        "template_hash": "abc",
        "created_at": iso(SystemTime::now() - Duration::from_secs(3600)),
        "session_id": format!("id-{name}"),
    });
    for (k, v) in header_extra.as_object().unwrap() {
        header[k] = v.clone();
    }
    let mut text = header.to_string();
    text.push('\n');
    for e in events {
        text.push_str(e);
        text.push('\n');
    }
    std::fs::write(dir.join(format!("koto-{name}.state.jsonl")), text).unwrap();
}

fn event(seq: usize, t: SystemTime, kind: &str, payload: serde_json::Value) -> String {
    json!({"seq": seq, "timestamp": iso(t), "type": kind, "payload": payload}).to_string()
}

/// 200 hierarchies (20 in a debug build), each an anchor holding a root
/// session, three children dispatched by the root, three grandchildren
/// under them by koto lineage and two great-grandchildren dispatched by
/// those. Even hierarchies are finished past their retention; odd ones
/// have a running great-grandchild.
fn build() -> Fixture {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    let tdir = tmp.path().join("templates");
    std::fs::create_dir_all(&tdir).unwrap();
    std::fs::write(tdir.join("scope.md"), TEMPLATE).unwrap();
    let (template, _) =
        koto::cache::compile_cached_into(&tdir.join("scope.md"), &tdir, true).unwrap();
    let template = template.to_string_lossy().into_owned();
    let backend = LocalBackend::with_base_dir(base.clone());

    let hierarchies = if RELEASE { 200 } else { 20 };
    let reads = if RELEASE { 2_000 } else { 200 };
    let long_ago = SystemTime::now() - Duration::from_secs(30 * 86_400);
    let link = |holder: &str, kind: &str, peer: &str| {
        let body = json!({"v": 1, "session": peer, "session_id": format!("id-{peer}"), "workspace": null, "at": iso(long_ago)});
        backend
            .add_with_writer(
                holder,
                &format!("koto/links/{kind}/{peer}"),
                body.to_string().as_bytes(),
                "koto",
            )
            .unwrap();
    };
    for h in 0..hierarchies {
        let anchor = format!("anchor-h{h}");
        let mut events = vec![event(
            1,
            long_ago,
            "anchor_started",
            json!({"source": "startup", "harness_session": format!("h{h}"), "opened": "new"}),
        )];
        for i in 0..reads {
            events.push(event(
                i + 2,
                long_ago,
                "context_read",
                json!({"key": format!("research/notes-{i}.md"), "reader": "cli", "state": "open",
                       "present": true, "hash": "0".repeat(64), "session": format!("s{h}-{}", i % 9)}),
            ));
        }
        write_log(&base, &anchor, json!({"kind": "anchor"}), &events);

        let session = |name: &str, parent: Option<&str>, running: bool| {
            let mut events = vec![
                event(
                    1,
                    long_ago,
                    "workflow_initialized",
                    json!({"template_path": template, "variables": {}}),
                ),
                event(
                    2,
                    long_ago,
                    "transitioned",
                    json!({"from": null, "to": "work", "condition_type": "auto"}),
                ),
            ];
            if !running {
                events.push(event(
                    3,
                    long_ago,
                    "transitioned",
                    json!({"from": "work", "to": "done", "condition_type": "auto"}),
                ));
            }
            let extra = match parent {
                Some(p) => json!({"parent_workflow": p}),
                None => json!({}),
            };
            write_log(&base, name, extra, &events);
        };
        let root = format!("s{h}-0");
        session(&root, None, false);
        link(&root, "held-by", &anchor);
        for c in 1..=3 {
            let child = format!("s{h}-{c}");
            session(&child, None, false);
            link(&child, "dispatched-by", &root);
            let grandchild = format!("s{h}-{}", c + 3);
            session(&grandchild, Some(&child), false);
            if c <= 2 {
                let great = format!("s{h}-{}", c + 6);
                let running = h % 2 == 1 && c == 1;
                session(&great, None, running);
                link(&great, "dispatched-by", &grandchild);
            }
        }
    }
    Fixture {
        _tmp: tmp,
        base,
        hierarchies,
    }
}

fn settings() -> RetentionSettings {
    let day = Duration::from_secs(86_400);
    RetentionSettings::from_durations(7 * day, 7 * day, 30 * day)
}

#[test]
fn the_engine_answers_within_its_bounds_on_a_large_store() {
    let f = build();

    // The query for one session, through the binary. A first run of a
    // freshly built binary can pay for the OS checking it (macOS scans a new
    // executable once), which isn't the query's cost, so one throwaway run
    // goes first.
    koto_command::koto().arg("version").output().unwrap();
    let started = Instant::now();
    let out = koto_command::koto()
        .env_remove("KOTO_SESSION_BACKEND")
        .env_remove("KOTO_RETENTION_KEEP")
        .env_remove("KOTO_RETENTION_IDLE_PERIOD")
        .env_remove("KOTO_RETENTION_STALE_AFTER")
        .env("HOME", f.base.parent().unwrap())
        .env("KOTO_SESSIONS_BASE", &f.base)
        .current_dir(f.base.parent().unwrap())
        .args(["session", "hierarchy", "s1-8", "--json"])
        .output()
        .unwrap();
    let query = started.elapsed();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["kept"], true);
    assert_eq!(v["keepers"][0]["name"], "s1-7");
    assert_eq!(v["members"].as_array().unwrap().len(), 10);

    // A prune-preview-shaped pass: one scan, every component judged.
    let started = Instant::now();
    let graph = Graph::scan(&f.base).unwrap();
    let scan = started.elapsed();
    let mut judge = Judge::new(&graph, settings(), SystemTime::now());
    let names: Vec<String> = graph.names().map(str::to_string).collect();
    let mut released = 0;
    for name in &names {
        let j = judge.judge(name, &Scope::Whole).unwrap();
        if !j.verdict.is_kept() {
            released += 1;
        }
    }
    let full = started.elapsed();
    assert_eq!(names.len(), f.hierarchies * 10);
    assert_eq!(released, f.hierarchies / 2 * 10);

    eprintln!(
        "retention cost: {} sessions; query {:?}; scan {:?}; full scan and judgment {:?}",
        names.len(),
        query,
        scan,
        full
    );
    if RELEASE {
        assert!(query < Duration::from_secs(1), "query took {query:?}");
        assert!(full < Duration::from_secs(10), "full pass took {full:?}");
    }
}
