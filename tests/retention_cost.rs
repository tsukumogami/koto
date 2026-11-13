//! The retention engine's cost bound (DESIGN-session-retention.md,
//! Decision 3): on a generated store of 2,000 sessions in 200 hierarchies
//! of 10 sessions at most 4 deep, whose anchors carry large logs of read
//! records, `koto session hierarchy` for one session answers within one
//! second, and a prune-preview-shaped pass (one scan, then a judgment of
//! every component) finishes within ten. One session in each hierarchy is
//! pinned, 200 pins in all, which hold to the same bounds
//! (DESIGN-session-pin.md, R28).
//!
//! The decision record's listing bound (PRD-decision-record.md, R36) runs
//! beside it: over one hierarchy of 1,000 sessions in four levels holding
//! 100 decisions each, the median of five warm runs of `koto decisions
//! list <root> --below` and of `koto decisions report --below <root>` each
//! stays under five seconds.
//!
//! The bounds hold for a release build, which CI runs in its own job
//! (`cargo test --release --test retention_cost`). A debug build runs the
//! same code on a store a tenth the size and checks only that every
//! verdict comes out as the fixture intends, so the default suite stays
//! fast.

#![cfg(unix)]

#[path = "support/host_session.rs"]
mod host_session;
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
            // One pinned session per hierarchy: 200 pins on the full store
            // (PRD-session-pin.md, R28).
            if c == 1 {
                let pin = json!({"v": 1, "id": format!("{h:032x}"), "reason": "kept for the cost test",
                                 "set_by": null, "set_at": iso(long_ago)});
                backend
                    .add_with_writer(
                        &grandchild,
                        "koto/links/pin",
                        pin.to_string().as_bytes(),
                        "koto",
                    )
                    .unwrap();
            }
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
    let mut pinned = 0;
    for name in &names {
        let j = judge.judge(name, &Scope::Whole).unwrap();
        if !j.verdict.is_kept() {
            released += 1;
        }
        if j.pin.is_pinned() {
            pinned += 1;
        }
    }
    let full = started.elapsed();
    assert_eq!(names.len(), f.hierarchies * 10);
    // A pin keeps its own session and changes no hierarchy's verdict.
    assert_eq!(released, f.hierarchies / 2 * 10);
    assert_eq!(pinned, f.hierarchies);

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

/// The sessions on each level of the decision store's one hierarchy: 1,000
/// in four levels, or a tenth of that in a debug build.
const DECISION_LEVELS: [usize; 4] = if RELEASE {
    [1, 9, 90, 900]
} else {
    [1, 3, 24, 72]
};

/// The decisions each session of the decision store holds.
const DECISIONS_PER_SESSION: usize = 100;

/// The limit R36 puts on a listing below the root and on the report.
const DECISION_LIMIT: Duration = Duration::from_secs(5);

/// One stored decision body, as `koto decisions record` writes it.
fn decision_body(name: &str, n: usize, ancestors: &[String]) -> Vec<u8> {
    let id = format!("{n:012}");
    let observed = |v: &str| json!({"value": v, "source": "observed"});
    let (kind, outcome, subject, defaulted) = match n % 3 {
        0 => (
            "gate-pass",
            "passed",
            json!({"workflow": name, "state": "work", "gate": "ci"}),
            true,
        ),
        1 => (
            "judgment",
            "settled",
            json!({"what": format!("finding {n}")}),
            false,
        ),
        _ => (
            "accept",
            "accepted",
            json!({"location": format!("req-{n}")}),
            false,
        ),
    };
    let body = json!({
        "v": 1,
        "id": id,
        "session": {"name": name, "id": format!("id-{name}")},
        "time": format!("2026-11-{:02}T{:02}:{:02}:00.000Z", 1 + n % 28, n % 24, n % 60),
        "state": "work",
        "log_seq": 2,
        "kind": kind,
        "outcome": outcome,
        "subject": subject,
        "decider": {"kind": "session", "session": observed(name), "harness": observed(""),
                    "anchor": observed(""), "agent": observed("unknown"), "defaulted": defaulted},
        "authority": {"kind": "dispatch", "level": null, "grants": [], "source": "handed",
                      "dispatcher": ancestors.first().map(String::as_str).unwrap_or("none"),
                      "run_mode": observed("unknown")},
        "chain": {"links": ancestors.iter().map(|a| json!({"session": a, "via": "init"})).collect::<Vec<_>>(),
                  "incomplete": false},
        "basis": [],
        "evidence": [],
    });
    let mut bytes = serde_json::to_vec(&body).unwrap();
    bytes.push(b'\n');
    bytes
}

/// A store holding one hierarchy of [`DECISION_LEVELS`] sessions, each
/// dispatched by one on the level above and holding
/// [`DECISIONS_PER_SESSION`] decisions written straight to its decision
/// area. Returns the temporary directory, the store's base and the root.
fn build_decision_store() -> (tempfile::TempDir, PathBuf, String) {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    let backend = LocalBackend::with_base_dir(base.clone());
    let long_ago = SystemTime::now() - Duration::from_secs(30 * 86_400);
    let mut above: Vec<(String, Vec<String>)> = Vec::new();
    for (level, &count) in DECISION_LEVELS.iter().enumerate() {
        let mut this_level = Vec::with_capacity(count);
        for i in 0..count {
            let name = format!("d{level}-{i}");
            let events = [event(
                1,
                long_ago,
                "transitioned",
                json!({"from": null, "to": "work", "condition_type": "auto"}),
            )];
            write_log(&base, &name, json!({}), &events);
            // The parent on the level above, spread evenly; its ancestors
            // follow it on the chain.
            let ancestors = match above.get(i * above.len().max(1) / count) {
                Some((parent, theirs)) => {
                    let body = json!({"v": 1, "session": parent, "session_id": format!("id-{parent}"),
                                      "workspace": null, "at": iso(long_ago), "via": "init"});
                    backend
                        .add_with_writer(
                            &name,
                            &format!("koto/links/dispatched-by/{parent}"),
                            body.to_string().as_bytes(),
                            "koto",
                        )
                        .unwrap();
                    let mut chain = vec![parent.clone()];
                    chain.extend(theirs.iter().cloned());
                    chain
                }
                None => Vec::new(),
            };
            let dir = base.join(&name).join("ctx/koto/decisions");
            std::fs::create_dir_all(&dir).unwrap();
            for n in 1..=DECISIONS_PER_SESSION {
                std::fs::write(
                    dir.join(format!("{n:012}")),
                    decision_body(&name, n, &ancestors),
                )
                .unwrap();
            }
            this_level.push((name, ancestors));
        }
        above = this_level;
    }
    (tmp, base, "d0-0".to_string())
}

/// The median of five timed runs of `args`, after one warm-up run whose
/// output is returned for checking.
fn median_of_five(base: &Path, args: &[&str]) -> (Duration, serde_json::Value) {
    let run = || {
        let started = Instant::now();
        let out = koto_command::koto()
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("KOTO_ANCHOR")
            .env("HOME", base.parent().unwrap())
            .env("KOTO_SESSIONS_BASE", base)
            .current_dir(base.parent().unwrap())
            .args(args)
            .output()
            .unwrap();
        let took = started.elapsed();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        (took, out.stdout)
    };
    let (_, warm) = run();
    let mut times: Vec<Duration> = (0..5).map(|_| run().0).collect();
    times.sort();
    (times[2], serde_json::from_slice(&warm).unwrap())
}

#[test]
fn the_decision_listing_and_report_answer_within_five_seconds_on_a_large_store() {
    let (_tmp, base, root) = build_decision_store();
    let sessions: usize = DECISION_LEVELS.iter().sum();

    // A freshly built binary can pay once for the OS checking it.
    koto_command::koto().arg("version").output().unwrap();

    let (list, listed) = median_of_five(
        &base,
        &["decisions", "list", &root, "--below", "--format", "v1"],
    );
    // Every session below the root, the root itself not included.
    assert_eq!(
        listed["decisions"].as_array().unwrap().len(),
        (sessions - 1) * DECISIONS_PER_SESSION
    );
    assert_eq!(listed["sessions"].as_array().unwrap().len(), sessions - 1);

    let (report, reported) = median_of_five(
        &base,
        &[
            "decisions",
            "report",
            "--since",
            "2026-11-01",
            "--until",
            "2026-12-01",
            "--below",
            &root,
        ],
    );
    assert_eq!(reported["sessions_read"], sessions);
    let made: u64 = ["gate-pass", "judgment", "accept"]
        .iter()
        .map(|k| reported["made"][k]["session"]["made"].as_u64().unwrap())
        .sum();
    assert_eq!(made as usize, sessions * DECISIONS_PER_SESSION);
    assert_eq!(
        reported["defaulted_gate_passes"],
        sessions * (DECISIONS_PER_SESSION / 3)
    );

    eprintln!(
        "decisions cost: {sessions} sessions, {} decisions; list --below median {list:?}; \
         report median {report:?}",
        sessions * DECISIONS_PER_SESSION
    );
    for (what, median) in [("list --below", list), ("report", report)] {
        if median >= DECISION_LIMIT * 4 / 5 {
            eprintln!(
                "decisions cost: NOTE: the {what} median {median:?} is within 20 percent of \
                 the {DECISION_LIMIT:?} limit; a bulk listing format under koto/packs/ is the \
                 follow-up"
            );
        }
    }
    if RELEASE {
        assert!(list < DECISION_LIMIT, "list --below median took {list:?}");
        assert!(report < DECISION_LIMIT, "report median took {report:?}");
    }
}
