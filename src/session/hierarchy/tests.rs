//! Tests for the retention engine: reading facts, the graph, liveness, the
//! verdict scenarios, rendering, settings and `edge_of`, plus a property
//! test that the two-level judgment equals a full-read reference.
//!
//! Stores are written directly, event by event, with chosen timestamps, so
//! every scenario runs against a fixed clock instead of sleeping.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::*;
use crate::engine::types::iso8601_at;
use crate::session::anchor::{CLOSE_RECORD_KEY, OBSERVED_KEY};
use crate::session::context::ContextStore;
use crate::session::context_log::WRITER_KOTO;
use crate::session::links::link_key;
use crate::session::local::LocalBackend;
use crate::session::state_file_name;

// ===== Harness =====

/// The clock every scenario starts from.
fn t0() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

fn at(secs: u64) -> SystemTime {
    t0() + Duration::from_secs(secs)
}

fn ts(secs: u64) -> String {
    iso8601_at(at(secs))
}

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

struct Store {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    template: String,
    backend: LocalBackend,
}

fn store() -> Store {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    let tdir = tmp.path().join("templates");
    std::fs::create_dir_all(&tdir).unwrap();
    let src = tdir.join("scope.md");
    std::fs::write(&src, TEMPLATE).unwrap();
    let (compiled, _) = crate::cache::compile_cached_into(&src, &tdir, true).unwrap();
    Store {
        backend: LocalBackend::with_base_dir(base.clone()),
        base,
        template: compiled.to_string_lossy().into_owned(),
        _tmp: tmp,
    }
}

fn event(seq: usize, secs: u64, kind: &str, payload: Value) -> Value {
    json!({"seq": seq, "timestamp": ts(secs), "type": kind, "payload": payload})
}

impl Store {
    fn dir(&self, name: &str) -> PathBuf {
        self.base.join(name)
    }

    fn log(&self, name: &str) -> PathBuf {
        self.dir(name).join(state_file_name(name))
    }

    /// Write a session's log: a header with `extra` merged in, then
    /// `events` as `(seconds, type, payload)`.
    fn write(&self, name: &str, extra: Value, events: &[(u64, &str, Value)]) {
        std::fs::create_dir_all(self.dir(name)).unwrap();
        let mut header = json!({
            "schema_version": 1,
            "workflow": name,
            "template_hash": "abc",
            "created_at": ts(0),
            "session_id": format!("id-{name}"),
        });
        for (k, v) in extra.as_object().unwrap() {
            header[k] = v.clone();
        }
        let mut text = serde_json::to_string(&header).unwrap();
        text.push('\n');
        for (i, (secs, kind, payload)) in events.iter().enumerate() {
            text.push_str(
                &serde_json::to_string(&event(i + 1, *secs, kind, payload.clone())).unwrap(),
            );
            text.push('\n');
        }
        std::fs::write(self.log(name), text).unwrap();
    }

    fn append(&self, name: &str, secs: u64, kind: &str, payload: Value) {
        let text = std::fs::read_to_string(self.log(name)).unwrap();
        let seq = text.lines().count();
        let line = serde_json::to_string(&event(seq, secs, kind, payload)).unwrap();
        std::fs::write(self.log(name), format!("{text}{line}\n")).unwrap();
    }

    fn start_events(&self) -> Vec<(u64, &'static str, Value)> {
        vec![
            (
                0,
                "workflow_initialized",
                json!({"template_path": self.template, "variables": {}}),
            ),
            (
                0,
                "transitioned",
                json!({"from": null, "to": "work", "condition_type": "auto"}),
            ),
        ]
    }

    /// A koto session still running.
    fn running(&self, name: &str) {
        self.write(name, json!({}), &self.start_events());
    }

    /// A koto session that reached its terminal state at `stop`.
    fn finished(&self, name: &str, stop: u64) {
        let mut events = self.start_events();
        events.push((
            stop,
            "transitioned",
            json!({"from": "work", "to": "done", "condition_type": "auto"}),
        ));
        self.write(name, json!({}), &events);
    }

    /// A koto session cancelled at `stop`.
    fn cancelled(&self, name: &str, stop: u64) {
        let mut events = self.start_events();
        events.push((
            stop,
            "workflow_cancelled",
            json!({"state": "work", "reason": "test"}),
        ));
        self.write(name, json!({}), &events);
    }

    fn with_parent(&self, name: &str, parent: &str, stop: Option<u64>) {
        let mut events = self.start_events();
        if let Some(stop) = stop {
            events.push((
                stop,
                "transitioned",
                json!({"from": "work", "to": "done", "condition_type": "auto"}),
            ));
        }
        self.write(name, json!({"parent_workflow": parent}), &events);
    }

    /// An anchor whose agent session started at `started` (harness id
    /// `harness`).
    fn anchor(&self, name: &str, harness: &str, started: u64) {
        self.write(
            name,
            json!({"kind": "anchor"}),
            &[(
                started,
                "anchor_started",
                json!({"source": "startup", "harness_session": harness, "opened": "new"}),
            )],
        );
    }

    fn key(&self, name: &str, key: &str, body: &[u8]) {
        self.backend
            .add_with_writer(name, key, body, WRITER_KOTO)
            .unwrap();
    }

    fn link(&self, holder: &str, kind: &str, peer: &str) {
        let body =
            json!({"v": 1, "session": peer, "session_id": null, "workspace": null, "at": ts(0)});
        self.key(
            holder,
            &format!("koto/links/{kind}/{peer}"),
            body.to_string().as_bytes(),
        );
    }

    fn observe(&self, anchor: &str, key: &str) {
        let body = json!({"v": 1, "key": key, "at": ts(0)});
        self.key(anchor, OBSERVED_KEY, body.to_string().as_bytes());
    }

    /// The close session of `anchor`, recorded at `secs` with `key`.
    fn close(&self, anchor: &str, key: &str, secs: u64) {
        let name = format!("{anchor}-closed");
        self.write(
            &name,
            json!({"kind": "anchor-close"}),
            &[(
                secs,
                "workflow_initialized",
                json!({"template_path": self.template, "variables": {}}),
            )],
        );
        self.link(&name, "closes", anchor);
        let record = json!({
            "v": 1,
            "anchor": anchor,
            "anchor_session_id": format!("id-{anchor}"),
            "kind": "observed",
            "key": key,
            "at": ts(secs),
        });
        self.key(&name, CLOSE_RECORD_KEY, record.to_string().as_bytes());
    }

    fn remove(&self, name: &str) {
        std::fs::remove_dir_all(self.dir(name)).unwrap();
    }

    fn judge_with(&self, name: &str, settings: RetentionSettings, now: u64) -> Judgment {
        let graph = Graph::scan(&self.base).unwrap();
        let mut judge = Judge::new(&graph, settings, at(now));
        judge.judge(name, &Scope::Whole).unwrap()
    }

    /// Judge `name` with a retention of `keep`, an idle period of `idle`
    /// and a stale threshold of `stale` seconds.
    fn judge(&self, name: &str, keep: u64, idle: u64, stale: u64, now: u64) -> Judgment {
        self.judge_with(
            name,
            RetentionSettings::from_durations(
                Duration::from_secs(keep),
                Duration::from_secs(idle),
                Duration::from_secs(stale),
            ),
            now,
        )
    }
}

fn keepers(j: &Judgment) -> Vec<String> {
    match &j.verdict {
        Verdict::Keep { keepers, .. } => keepers.iter().map(|k| k.name.clone()).collect(),
        Verdict::Release { .. } => Vec::new(),
    }
}

fn keeper<'a>(j: &'a Judgment, name: &str) -> &'a Keeper {
    match &j.verdict {
        Verdict::Keep { keepers, .. } => keepers.iter().find(|k| k.name == name).unwrap(),
        Verdict::Release { .. } => panic!("released: {j:?}"),
    }
}

fn retention(j: &Judgment) -> Retention {
    match &j.verdict {
        Verdict::Keep { retention, .. } => retention.clone().expect("a retention"),
        Verdict::Release { release } => release.clone().expect("a release"),
    }
}

fn unreadable(j: &Judgment) -> Vec<Unreadable> {
    match &j.verdict {
        Verdict::Keep { unreadable, .. } => unreadable.clone(),
        Verdict::Release { .. } => Vec::new(),
    }
}

fn released(j: &Judgment) -> bool {
    !j.verdict.is_kept()
}

fn member<'a>(j: &'a Judgment, name: &str) -> &'a Member {
    j.members.iter().find(|m| m.name == name).unwrap()
}

const DAY: u64 = 86_400;

// ===== Verdict scenarios =====

#[test]
fn co_parent_keeps_every_member_naming_the_live_co_parent() {
    let s = store();
    s.finished("a", 10);
    s.running("b");
    s.finished("c", 20);
    s.finished("d", 30);
    s.link("c", "dispatched-by", "a");
    s.link("c", "dispatched-by", "b");
    s.link("d", "dispatched-by", "a");
    for name in ["a", "c", "d"] {
        let j = s.judge(name, 20, 7 * DAY, 30 * DAY, 10 * DAY);
        assert_eq!(keepers(&j), vec!["b"], "{name}: {j:?}");
        let k = keeper(&j, "b");
        assert_eq!(k.kind, MemberKind::KotoSession);
        assert!(!k.observed);
    }
}

#[test]
fn release_instant_is_the_later_stop_and_the_retention_runs_from_it() {
    for cancel_later in [false, true] {
        let s = store();
        s.finished("a", 100);
        if cancel_later {
            s.cancelled("b", 112);
        } else {
            s.finished("b", 112);
        }
        s.link("b", "dispatched-by", "a");
        let j = s.judge("a", 20, DAY, 2 * DAY, 122);
        assert!(j.verdict.is_kept(), "{j:?}");
        assert!(keepers(&j).is_empty());
        let r = retention(&j);
        assert_eq!(r.release.member, "b");
        assert_eq!(r.release.at, ts(112));
        let kind = if cancel_later {
            CloseKind::Cancelled
        } else {
            CloseKind::Terminal
        };
        assert_eq!(r.release.close_kind, kind);
        assert_eq!(r.passes_at, ts(132));
        let j = s.judge("a", 20, DAY, 2 * DAY, 140);
        assert!(released(&j), "{j:?}");
        assert_eq!(retention(&j).release.member, "b");
    }
}

#[test]
fn two_finished_sessions_release_together_after_the_retention() {
    let s = store();
    s.finished("a", 10);
    s.finished("b", 20);
    s.link("b", "dispatched-by", "a");
    for name in ["a", "b"] {
        assert!(s.judge(name, 30, DAY, 2 * DAY, 49).verdict.is_kept());
        assert!(released(&s.judge(name, 30, DAY, 2 * DAY, 51)));
    }
}

#[test]
fn whole_hierarchy_is_judged_as_a_unit() {
    let s = store();
    s.finished("a1", 10);
    s.finished("a2", 20);
    s.finished("a3", 30);
    s.link("a2", "dispatched-by", "a1");
    s.with_parent("a3", "a2", Some(30));
    s.finished("b1", 10);
    s.running("b2");
    s.finished("b3", 30);
    s.link("b2", "dispatched-by", "b1");
    s.key("b3", "chain/parent", b"b2");
    for name in ["a1", "a2", "a3"] {
        assert!(released(&s.judge(name, 10, DAY, 2 * DAY, 1000)), "{name}");
    }
    for name in ["b1", "b2", "b3"] {
        let j = s.judge(name, 10, DAY, 2 * DAY, 1000);
        assert_eq!(keepers(&j), vec!["b2"], "{name}");
    }
}

#[test]
fn an_absent_peer_contributes_no_stop() {
    let s = store();
    s.finished("b", 50);
    s.link("b", "dispatched-by", "a");
    let j = s.judge("b", 20, DAY, 2 * DAY, 60);
    assert!(j.verdict.is_kept());
    assert_eq!(retention(&j).release.member, "b");
    assert!(member(&j, "a").absent);
    assert!(released(&s.judge("b", 20, DAY, 2 * DAY, 71)));
}

/// Mark `copy` as the import of `source`: its `koto/imported-from` key and
/// a `session_imported` event at `secs`.
fn import(s: &Store, copy: &str, source: &str, secs: u64) {
    let body = json!({"from_session": source, "from_session_id": format!("id-{source}")});
    s.key(copy, "koto/imported-from", body.to_string().as_bytes());
    s.append(
        copy,
        secs,
        "session_imported",
        json!({"from_workspace": "/w", "from_session": source, "from_session_id": format!("id-{source}"), "machine_id": "m"}),
    );
}

#[test]
fn a_link_to_a_migrated_session_resolves_to_its_local_copy() {
    let s = store();
    // a was moved to a-copy in the same store; b links a by name.
    s.running("a");
    s.running("a-copy");
    import(&s, "a-copy", "a", 5);
    s.finished("b", 10);
    s.link("b", "dispatched-by", "a");
    let j = s.judge("b", 20, DAY, 2 * DAY, 100);
    assert_eq!(keepers(&j), vec!["a-copy"], "{j:?}");
    assert!(member(&j, "a").migrated);
    assert!(!member(&j, "a").alive);
}

#[test]
fn a_migrated_anchor_keeps_nothing_and_a_missing_copy_reads_absent() {
    let s = store();
    // The anchor was moved; its copy idled long ago.
    s.anchor("anchor-x", "x", 0);
    s.anchor("anchor-x-copy", "x", 0);
    import(&s, "anchor-x-copy", "anchor-x", 1);
    s.finished("b", 10);
    s.link("b", "held-by", "anchor-x");
    let j = s.judge("b", 20, 100, 2 * DAY, 500);
    assert!(released(&j), "{j:?}");
    assert!(member(&j, "anchor-x").migrated);

    // With no copy in the store, the peer is absent and adds no stop.
    let s = store();
    s.finished("b", 10);
    s.link("b", "held-by", "anchor-gone");
    let j = s.judge("b", 20, 100, 2 * DAY, 25);
    assert!(member(&j, "anchor-gone").absent);
    assert_eq!(retention(&j).release.member, "b");
}

#[test]
fn migration_needs_the_id_the_event_and_a_source_that_stopped_writing() {
    // Wrong id.
    let s = store();
    s.running("a");
    s.running("a-copy");
    let body = json!({"from_session": "a", "from_session_id": "someone-else"});
    s.key("a-copy", "koto/imported-from", body.to_string().as_bytes());
    s.append(
        "a-copy",
        5,
        "session_imported",
        json!({"from_workspace": "/w", "from_session": "a", "from_session_id": "someone-else", "machine_id": "m"}),
    );
    assert!(!Graph::scan(&s.base).unwrap().is_migrated("a"));

    // No session_imported event.
    let s = store();
    s.running("a");
    s.running("a-copy");
    let body = json!({"from_session": "a", "from_session_id": "id-a"});
    s.key("a-copy", "koto/imported-from", body.to_string().as_bytes());
    assert!(!Graph::scan(&s.base).unwrap().is_migrated("a"));

    // The source wrote after the import: it is alive, not migrated.
    let s = store();
    s.running("a");
    s.running("a-copy");
    import(&s, "a-copy", "a", 5);
    s.append(
        "a",
        9,
        "evidence_submitted",
        json!({"state": "work", "fields": {}}),
    );
    assert!(!Graph::scan(&s.base).unwrap().is_migrated("a"));
}

#[test]
fn activity_extends_the_idle_period() {
    let s = store();
    s.anchor("anchor-s", "s", 0);
    s.running("b");
    s.link("b", "held-by", "anchor-s");
    let idle = Duration::from_secs(10);
    // The agent session ticks b every 5 seconds for 30 seconds.
    for tick in (0..=30).step_by(5) {
        let wrote = crate::session::anchor::record_activity(
            &s.backend,
            "anchor-s",
            "b",
            "next",
            idle,
            at(tick),
        )
        .unwrap();
        assert!(wrote || tick == 0, "tick at {tick}s wrote no activity");
        // At most one per second: a second tick in the same second adds
        // nothing.
        assert!(!crate::session::anchor::record_activity(
            &s.backend,
            "anchor-s",
            "b",
            "next",
            idle,
            at(tick)
        )
        .unwrap());
        let j = s.judge("b", 5, 10, 100, tick);
        assert!(member(&j, "anchor-s").alive, "closed at {tick}s");
    }
    // b finished long ago; the anchor alone holds it.
    s.finished("b", 1);
    s.link("b", "held-by", "anchor-s");
    assert!(member(&s.judge("b", 5, 10, 100, 39), "anchor-s").alive);
    let j = s.judge("b", 5, 10, 100, 40);
    let a = member(&j, "anchor-s");
    assert!(!a.alive);
    assert_eq!(a.close_kind, Some(CloseKind::Idle));
    assert_eq!(a.stopped_at.as_deref(), Some(ts(40).as_str()));
}

#[test]
fn what_is_not_activity_never_extends_the_anchor() {
    let s = store();
    s.anchor("anchor-s", "s", 0);
    s.finished("b", 0);
    s.link("b", "held-by", "anchor-s");
    s.running("unrelated");
    s.finished("b.child", 0);
    s.with_parent("b.child", "b", Some(0));
    for tick in (3..=30).step_by(3) {
        // Another agent session reads the anchor: the read lands in the
        // anchor with no `session`, since it isn't the anchor's own record.
        s.append(
            "anchor-s",
            tick,
            "context_read",
            json!({"key": "k", "reader": "cli", "state": "open", "present": false}),
        );
        // ...and reads the held session, and ticks one the anchor doesn't
        // hold.
        s.append(
            "b",
            tick,
            "context_read",
            json!({"key": "k", "reader": "cli", "state": "done", "present": false}),
        );
        s.append(
            "unrelated",
            tick,
            "evidence_submitted",
            json!({"state": "work", "fields": {}}),
        );
        // A child of the held session writes its completion record, and a
        // bare shell adds a key to the held session.
        s.append(
            "b.child",
            tick,
            "context_added",
            json!({"key": "result", "hash": "00", "size": 1}),
        );
        s.append(
            "b",
            tick,
            "context_added",
            json!({"key": "note", "hash": "00", "size": 1, "writer": "agent"}),
        );
    }
    assert!(member(&s.judge("b", 5, 10, 100, 9), "anchor-s").alive);
    let j = s.judge("b", 5, 10, 100, 10);
    let a = member(&j, "anchor-s");
    assert!(!a.alive, "{j:?}");
    assert_eq!(a.stopped_at.as_deref(), Some(ts(10).as_str()));
}

#[test]
fn an_anchors_own_read_records_are_activity() {
    let s = store();
    s.anchor("anchor-s", "s", 0);
    s.append(
        "anchor-s",
        50,
        "context_read",
        json!({"key": "k", "reader": "cli", "state": "open", "present": false, "session": "other"}),
    );
    let j = s.judge("anchor-s", 5, 10, 100, 59);
    assert!(member(&j, "anchor-s").alive);
    let j = s.judge("anchor-s", 5, 10, 100, 60);
    assert!(!member(&j, "anchor-s").alive);
}

#[test]
fn observed_anchors_never_idle_close() {
    let s = store();
    // A's anchor dispatched B's agent session; B's anchor holds b.
    s.anchor("anchor-a", "a", 0);
    s.observe("anchor-a", "ka");
    s.anchor("anchor-b", "b", 0);
    s.observe("anchor-b", "kb");
    s.link("anchor-b", "dispatched-by", "anchor-a");
    s.finished("b", 5);
    s.link("b", "held-by", "anchor-b");
    s.close("anchor-a", "ka", 20);
    let (keep, idle, stale) = (100, 10, 200);
    // Past the idle period, and past the stale threshold, b stays kept,
    // even though its dispatcher's anchor reads closed.
    for now in [50, 150, 250, 10 * DAY] {
        let j = s.judge("b", keep, idle, stale, now);
        assert_eq!(keepers(&j), vec!["anchor-b"], "at {now}: {j:?}");
        assert_eq!(
            member(&j, "anchor-a").close_kind,
            Some(CloseKind::Observed),
            "at {now}"
        );
        let k = keeper(&j, "anchor-b");
        assert!(k.observed);
        assert_eq!(k.stale, now >= stale, "at {now}");
        if k.stale {
            assert_eq!(
                k.close_command.as_deref(),
                Some("koto session anchor close --key='kb' --harness-session='b'")
            );
        } else {
            assert_eq!(k.close_command, None);
        }
    }
    // Running the printed command closes it as observed; after the
    // retention from that close, b is released.
    s.close("anchor-b", "kb", 10 * DAY);
    let j = s.judge("b", keep, idle, stale, 10 * DAY + 50);
    assert!(j.verdict.is_kept());
    let r = retention(&j);
    assert_eq!(r.release.close_kind, CloseKind::Observed);
    assert!(released(&s.judge("b", keep, idle, stale, 10 * DAY + 101)));
}

#[test]
fn an_observed_anchor_with_nothing_left_stays_open_and_stale() {
    // No links at all.
    let s = store();
    s.anchor("anchor-a", "a", 0);
    s.observe("anchor-a", "ka");
    let j = s.judge("anchor-a", 100, 10, 200, 300);
    assert_eq!(keepers(&j), vec!["anchor-a"]);
    assert!(keeper(&j, "anchor-a").stale);

    // Every session it held was removed.
    let s = store();
    s.anchor("anchor-a", "a", 0);
    s.observe("anchor-a", "ka");
    s.finished("b", 1);
    s.link("b", "held-by", "anchor-a");
    s.remove("b");
    let j = s.judge("anchor-a", 100, 10, 200, 300);
    assert_eq!(keepers(&j), vec!["anchor-a"]);
    let k = keeper(&j, "anchor-a");
    assert!(k.stale);
    assert!(k.close_command.is_some());
}

#[test]
fn the_stale_report_names_the_close_command_only_for_safe_values() {
    let case = |key: &str, harness: &str| -> Keeper {
        let s = store();
        s.anchor("anchor-a", harness, 0);
        s.observe("anchor-a", key);
        keeper(&s.judge("anchor-a", 100, 10, 200, 300), "anchor-a").clone()
    };
    let k = case("K-1_x", "S_2-y");
    assert!(k.stale);
    assert_eq!(
        k.close_command.as_deref(),
        Some("koto session anchor close --key='K-1_x' --harness-session='S_2-y'")
    );
    for bad in ["a;rm", "$(id)", "it's", "two\nlines", "a\"b", ""] {
        let k = case(bad, "s");
        assert!(k.stale, "{bad:?}");
        assert!(k.malformed_marker, "{bad:?}");
        assert_eq!(k.close_command, None, "{bad:?}");
        let k = case("k", bad);
        assert!(
            k.stale && k.malformed_marker && k.close_command.is_none(),
            "{bad:?}"
        );
    }

    // Inside the threshold: no command.
    let s = store();
    s.anchor("anchor-a", "a", 0);
    s.observe("anchor-a", "k");
    let k = keeper(&s.judge("anchor-a", 100, 10, 200, 150), "anchor-a").clone();
    assert!(!k.stale && k.close_command.is_none());

    // Unobserved: it idle-closes, so it is never stale.
    let s = store();
    s.anchor("anchor-a", "a", 0);
    let j = s.judge("anchor-a", 100, 10, 200, 5);
    let k = keeper(&j, "anchor-a");
    assert!(!k.stale && k.close_command.is_none());

    // Imported (its marker cleared by the import): idle-closes too, and
    // lists no command while it is open.
    let s = store();
    s.anchor("anchor-a", "a", 0);
    s.running("anchor-src");
    import(&s, "anchor-a", "elsewhere", 300);
    let j = s.judge("anchor-a", 100, 10, 200, 305);
    let k = keeper(&j, "anchor-a");
    assert!(!k.stale && k.close_command.is_none());
    assert!(member(&j, "anchor-a").imported);
    assert!(!member(&s.judge("anchor-a", 100, 10, 200, 310), "anchor-a").alive);
}

#[test]
fn close_records_last_read_half() {
    let s = store();
    s.anchor("anchor-a", "a", 0);
    s.observe("anchor-a", "k");
    s.finished("b", 1);
    s.link("b", "held-by", "anchor-a");
    s.close("anchor-a", "k", 50);
    let j = s.judge("b", 100, 10, 200, 60);
    assert_eq!(member(&j, "anchor-a").close_kind, Some(CloseKind::Observed));
    assert!(!member(&j, "anchor-a").alive);
    s.remove("anchor-a-closed");
    let j = s.judge("b", 100, 10, 200, 60);
    assert!(member(&j, "anchor-a").alive);
    assert_eq!(keepers(&j), vec!["anchor-a"]);
}

#[test]
fn verdicts_need_no_koto_lineage_and_no_request_store() {
    let s = store();
    // Finished child.
    s.running("a");
    s.finished("b", 10);
    s.link("b", "dispatched-by", "a");
    // Grandchild.
    s.finished("c", 20);
    s.link("c", "dispatched-by", "b");
    // Co-parent.
    s.running("p");
    s.finished("q", 10);
    s.link("q", "dispatched-by", "p");
    s.link("q", "dispatched-by", "a");
    assert!(!s.base.join(".requests").exists());
    for name in ["b", "c", "q"] {
        let j = s.judge(name, 10, DAY, 2 * DAY, 1000);
        let mut k = keepers(&j);
        k.sort();
        assert_eq!(k, vec!["a", "p"], "{name}");
    }
}

#[test]
fn a_cleaned_up_dispatcher_releases_the_finished_child() {
    let s = store();
    s.running("a");
    s.finished("b", 10);
    s.link("b", "dispatched-by", "a");
    assert_eq!(keepers(&s.judge("b", 10, DAY, 2 * DAY, 100)), vec!["a"]);
    s.remove("a");
    assert!(released(&s.judge("b", 10, DAY, 2 * DAY, 100)));
}

// ===== Fail toward keeping =====

#[test]
fn an_unreadable_log_keeps_its_hierarchy_and_names_it() {
    let s = store();
    s.finished("a", 10);
    s.finished("b", 10);
    s.link("b", "dispatched-by", "a");
    let mut text = std::fs::read_to_string(s.log("a")).unwrap();
    text.push_str("{not json}\n{\"seq\":9}\n");
    std::fs::write(s.log("a"), text).unwrap();
    for name in ["a", "b"] {
        let j = s.judge(name, 10, DAY, 2 * DAY, 1000);
        assert!(j.verdict.is_kept(), "{name}");
        assert!(
            unreadable(&j).iter().any(|u| u.name == "a"),
            "{name}: {j:?}"
        );
    }
}

#[test]
fn a_truncated_link_keeps_its_hierarchy() {
    let s = store();
    s.finished("a", 10);
    s.finished("b", 10);
    s.link("b", "dispatched-by", "a");
    let path = s.dir("b").join("ctx/koto/links/dispatched-by/a");
    std::fs::write(&path, b"{\"v\":1,\"sess").unwrap();
    for name in ["a", "b"] {
        let j = s.judge(name, 10, DAY, 2 * DAY, 1000);
        let u = unreadable(&j);
        assert!(
            u.iter()
                .any(|u| u.name == "b" && u.reason.contains("koto/links/dispatched-by/a")),
            "{name}: {j:?}"
        );
    }
}

#[test]
fn a_malformed_close_record_keeps_its_hierarchy() {
    let s = store();
    s.anchor("anchor-a", "a", 0);
    s.observe("anchor-a", "k");
    s.finished("b", 1);
    s.link("b", "held-by", "anchor-a");
    s.close("anchor-a", "k", 5);
    std::fs::write(
        s.dir("anchor-a-closed").join("ctx/koto/anchor-close"),
        b"{oops",
    )
    .unwrap();
    let j = s.judge("b", 10, 10, 200, 1000);
    assert!(j.verdict.is_kept());
    let u = unreadable(&j);
    assert!(u.iter().any(|u| u.name == "anchor-a"), "{j:?}");
    assert!(u.iter().any(|u| u.name == "anchor-a-closed"), "{j:?}");
}

#[test]
fn an_unreadable_header_taints_every_component() {
    let s = store();
    s.finished("a", 10);
    s.finished("z", 10);
    std::fs::write(s.log("z"), "not a header\n").unwrap();
    let j = s.judge("a", 10, DAY, 2 * DAY, 1000);
    assert!(j.verdict.is_kept());
    assert!(unreadable(&j).iter().any(|u| u.name == "z"), "{j:?}");
}

#[test]
fn an_unreadable_manifest_or_links_directory_taints_every_component() {
    let s = store();
    s.finished("a", 10);
    s.finished("z", 10);
    s.link("z", "dispatched-by", "y");
    std::fs::write(s.dir("z").join("ctx/manifest.json"), b"{broken").unwrap();
    assert!(s.judge("a", 10, DAY, 2 * DAY, 1000).verdict.is_kept());

    let s = store();
    s.finished("a", 10);
    s.finished("z", 10);
    std::fs::create_dir_all(s.dir("z").join("ctx")).unwrap();
    let elsewhere = s.base.parent().unwrap().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, s.dir("z").join("ctx/koto")).unwrap();
    let j = s.judge("a", 10, DAY, 2 * DAY, 1000);
    assert!(j.verdict.is_kept());
    assert!(unreadable(&j).iter().any(|u| u.name == "z"), "{j:?}");
}

#[test]
fn an_unlistable_sessions_directory_fails_the_scan_and_answers_edge() {
    use std::os::unix::fs::PermissionsExt;
    let s = store();
    s.finished("a", 10);
    std::fs::set_permissions(&s.base, std::fs::Permissions::from_mode(0o000)).unwrap();
    let listable = std::fs::read_dir(&s.base).is_ok();
    let scan = Graph::scan(&s.base);
    let edge = edge_of(&s.base, "a");
    std::fs::set_permissions(&s.base, std::fs::Permissions::from_mode(0o755)).unwrap();
    if listable {
        // Running as root: permissions don't bind, nothing to check.
        return;
    }
    assert!(scan.is_err());
    assert!(edge);
}

#[test]
fn symlinks_and_special_files_under_the_reserved_area_are_unreadable() {
    let s = store();
    s.finished("a", 10);
    s.link("a", "dispatched-by", "x");
    let target = s.base.parent().unwrap().join("t");
    std::fs::write(&target, b"{}").unwrap();
    std::os::unix::fs::symlink(&target, s.dir("a").join("ctx/koto/links/held-by-sym")).unwrap();
    let f = SessionFacts::read(&s.base, "a");
    assert!(
        f.problems.iter().any(|p| p.contains("held-by-sym")),
        "{f:?}"
    );

    // A symlinked session directory is unreadable for the whole store.
    let s = store();
    s.finished("real", 10);
    std::os::unix::fs::symlink(s.dir("real"), s.dir("linked")).unwrap();
    let g = Graph::scan(&s.base).unwrap();
    assert!(g.store_problems().iter().any(|(n, _)| n == "linked"));
}

#[test]
fn link_reading_rules() {
    let s = store();
    s.finished("a", 10);
    // An oversized body.
    let big = json!({"v": 1, "session": "p1", "pad": "x".repeat(5000)});
    s.key(
        "a",
        &link_key(crate::session::links::LinkKind::DispatchedBy, "p1"),
        big.to_string().as_bytes(),
    );
    // An unknown version, read by its `session`.
    s.key("a", "koto/links/held-by/p2", br#"{"v":9,"session":"p3"}"#);
    // An unknown kind still counts as an edge.
    s.key(
        "a",
        "koto/links/watched-by/p4",
        br#"{"v":1,"session":"p4"}"#,
    );
    // Unlisted files: dispatched-by and held-by count, closes never.
    let links = s.dir("a").join("ctx/koto/links");
    std::fs::write(links.join("dispatched-by/p5"), br#"{"v":1,"session":"p5"}"#).unwrap();
    std::fs::create_dir_all(links.join("closes")).unwrap();
    std::fs::write(links.join("closes/p6"), br#"{"v":1,"session":"p6"}"#).unwrap();
    // An empty unlisted file is a lock file, not a link.
    std::fs::write(links.join("held-by/p7"), b"").unwrap();
    let f = SessionFacts::read(&s.base, "a");
    let peers: BTreeSet<&str> = f.links.iter().map(|l| l.peer.as_str()).collect();
    assert!(peers.contains("p1"));
    assert!(f.problems.iter().any(|p| p.contains("larger than")));
    assert!(peers.contains("p3") && !peers.contains("p2"));
    assert!(peers.contains("p4"));
    assert!(peers.contains("p5"));
    assert!(!peers.contains("p6"));
    assert!(!peers.contains("p7"));
}

#[test]
fn an_unknown_kind_reads_as_an_ordinary_session() {
    let s = store();
    s.write("odd", json!({"kind": "something-new"}), &s.start_events());
    let j = s.judge("odd", 10, DAY, 2 * DAY, 100);
    assert_eq!(member(&j, "odd").kind, MemberKind::KotoSession);
    assert!(member(&j, "odd").alive);
}

#[test]
fn a_future_timestamp_taints_and_a_near_one_is_clamped() {
    let s = store();
    s.finished("a", 10);
    s.finished("b", 1000);
    s.link("b", "dispatched-by", "a");
    // b's stop is 600s ahead of now (500): tainted and named.
    let j = s.judge("a", 10, DAY, 2 * DAY, 400);
    assert!(j.verdict.is_kept());
    assert!(unreadable(&j)
        .iter()
        .any(|u| u.name == "b" && u.reason.contains("future")));
    // 100s ahead: clamped to now, reported, not tainted.
    let j = s.judge("a", 10, DAY, 2 * DAY, 900);
    assert!(unreadable(&j).is_empty(), "{j:?}");
    assert_eq!(j.clamped, vec!["b"]);
    assert!(member(&j, "b").clamped);
    assert_eq!(retention(&j).release.at, ts(900));
}

// ===== Graph =====

#[test]
fn epoch_branch_lineage_reaches_the_parent() {
    assert_eq!(lineage_target("p~3", |_| false), "p");
    assert_eq!(lineage_target("p~3", |n| n == "p~3"), "p~3");
    assert_eq!(lineage_target("p~x", |_| false), "p~x");
    assert_eq!(lineage_target("p~", |_| false), "p~");
    let s = store();
    s.running("p");
    s.with_parent("c", "p~2", Some(10));
    assert_eq!(keepers(&s.judge("c", 10, DAY, 2 * DAY, 100)), vec!["p"]);
}

#[test]
fn a_session_id_hint_joins_a_moved_aside_predecessor() {
    let s = store();
    // b's link names "a" with the id of the run now moved to a_r1.
    s.running("a_r1");
    s.write("a_r1", json!({"session_id": "id-old-a"}), &s.start_events());
    s.finished("a", 5);
    s.finished("b", 10);
    let body =
        json!({"v": 1, "session": "a", "session_id": "id-old-a", "workspace": null, "at": ts(0)});
    s.key(
        "b",
        "koto/links/dispatched-by/a",
        body.to_string().as_bytes(),
    );
    let j = s.judge("b", 10, DAY, 2 * DAY, 100);
    assert_eq!(keepers(&j), vec!["a_r1"]);
}

#[test]
fn a_subtree_scope_drops_only_the_roots_child_lineage() {
    let s = store();
    s.running("x");
    s.with_parent("c", "x", Some(10));
    s.with_parent("g", "c", Some(10));
    s.running("k");
    s.with_parent("k2", "x", Some(10));
    s.link("k2", "dispatched-by", "k");
    let graph = Graph::scan(&s.base).unwrap();
    let mut judge = Judge::new(
        &graph,
        RetentionSettings::from_durations(
            Duration::from_secs(10),
            Duration::from_secs(DAY),
            Duration::from_secs(2 * DAY),
        ),
        at(100),
    );
    let scope = Scope::DropChildLineageOf("x".to_string());
    assert!(!judge.judge("c", &scope).unwrap().verdict.is_kept());
    assert!(!judge.judge("g", &scope).unwrap().verdict.is_kept());
    let j = judge.judge("k2", &scope).unwrap();
    assert_eq!(keepers(&j), vec!["k"]);
    assert_eq!(
        keepers(&judge.judge("x", &Scope::Whole).unwrap()),
        vec!["k", "x"]
    );
}

#[test]
fn each_member_lists_the_edge_that_joined_it() {
    let s = store();
    s.running("a");
    s.finished("b", 10);
    s.link("b", "dispatched-by", "a");
    let j = s.judge("b", 10, DAY, 2 * DAY, 100);
    let a = member(&j, "a");
    assert_eq!(a.joined_by.len(), 1);
    assert_eq!(a.joined_by[0].holder, "b");
    assert_eq!(a.joined_by[0].via, Via::Link("dispatched-by".to_string()));
}

// ===== edge_of =====

#[test]
fn edge_of_finds_own_and_inbound_edges() {
    let s = store();
    s.finished("lone", 1);
    assert!(!edge_of(&s.base, "lone"));
    s.finished("child", 1);
    s.link("child", "dispatched-by", "parent");
    assert!(edge_of(&s.base, "child"));
    s.finished("parent", 1);
    assert!(edge_of(&s.base, "parent"));
    s.finished("kp", 1);
    s.with_parent("kc", "kp~4", Some(1));
    assert!(edge_of(&s.base, "kp"));
    s.finished("cp", 1);
    s.finished("cc", 1);
    s.key("cc", "chain/parent", b"cp");
    assert!(edge_of(&s.base, "cp"));
    assert!(!edge_of(&s.base, "lone"));
    // A link of a kind this koto doesn't know still counts.
    s.finished("odd", 1);
    s.finished("watcher", 1);
    s.key(
        "watcher",
        "koto/links/watched-by/odd",
        br#"{"v":1,"session":"odd"}"#,
    );
    assert!(edge_of(&s.base, "odd"));
    // A copy counts records naming the session it was imported from.
    s.finished("src", 1);
    s.finished("src-copy", 1);
    import(&s, "src-copy", "src", 5);
    s.finished("disp", 1);
    s.link("disp", "dispatched-by", "src");
    assert!(edge_of(&s.base, "src-copy"));
    // Anything unreadable answers "edge".
    std::fs::write(s.log("cc"), "garbage\n").unwrap();
    assert!(edge_of(&s.base, "lone"));
}

/// `edge_of` finds every edge the graph joins a session through, the ones
/// a link body names included: a session-id hint naming a predecessor moved
/// aside under a new name, and a body of an unknown version naming a
/// session other than its key's.
#[test]
fn edge_of_counts_what_link_bodies_name_as_the_graph_does() {
    let s = store();
    // `b` moved aside to `b_r1` (keeping its id), and a new run took `b`.
    // `c`'s link names `b` by name and `b_r1` by the id it recorded.
    s.finished("b_r1", 1);
    s.finished("b", 2);
    s.finished("c", 1);
    let body = json!({"v": 1, "session": "b", "session_id": "id-b_r1", "at": ts(0)});
    s.key(
        "c",
        "koto/links/dispatched-by/b",
        body.to_string().as_bytes(),
    );
    let graph = Graph::scan(&s.base).unwrap();
    assert!(graph
        .edges()
        .iter()
        .any(|e| e.target == "b_r1" && matches!(e.via, Via::SessionIdHint(_))));
    assert!(
        edge_of(&s.base, "b_r1"),
        "the hint joins the moved predecessor"
    );

    // A body of a version this koto doesn't know is an edge to the session
    // it names, whatever the key's name.
    s.finished("named", 1);
    s.finished("holder", 1);
    let body = json!({"v": 9, "session": "named"});
    s.key(
        "holder",
        "koto/links/watched-by/other-name",
        body.to_string().as_bytes(),
    );
    let graph = Graph::scan(&s.base).unwrap();
    assert!(graph.edges().iter().any(|e| e.target == "named"));
    assert!(edge_of(&s.base, "named"));

    // A lone session is still lone: neither body names it.
    s.finished("lone", 1);
    assert!(!edge_of(&s.base, "lone"));
}

// ===== Rendering =====

#[test]
fn the_renderers_strip_control_characters_and_agree() {
    let s = store();
    s.running("a");
    s.finished("b", 10);
    s.link("b", "dispatched-by", "a");
    let mut j = s.judge("b", 10, DAY, 2 * DAY, 100);
    if let Verdict::Keep { unreadable, .. } = &mut j.verdict {
        unreadable.push(Unreadable {
            name: "x\u{1b}[31m".to_string(),
            reason: "bad\nline\u{7}".to_string(),
        });
    }
    let settings = RetentionSettings::from_durations(
        Duration::from_secs(10),
        Duration::from_secs(DAY),
        Duration::from_secs(2 * DAY),
    );
    let text = render_text(&j, &settings);
    let value = to_json(&j, &settings);
    let as_text = value.to_string();
    for out in [&text, &as_text] {
        assert!(!out.contains('\u{1b}') && !out.contains('\u{7}'), "{out}");
    }
    assert!(text.contains("badline"), "{text}");
    assert_eq!(value["kept"], json!(true));
    assert_eq!(value["keepers"][0]["name"], json!("a"));
    assert_eq!(
        value["settings"]["retention.keep"]["origin"],
        json!("default")
    );
    assert!(reason_text(&j.verdict).starts_with("kept: alive in its hierarchy: a"));
}

#[test]
fn close_command_quotes_both_values_and_refuses_unsafe_ones() {
    assert_eq!(
        close_command("K", "S").as_deref(),
        Some("koto session anchor close --key='K' --harness-session='S'")
    );
    assert_eq!(close_command("K'", "S"), None);
    assert_eq!(close_command("K", &"s".repeat(129)), None);
}

// ===== Settings =====

#[test]
fn settings_default_and_refuse_with_the_setting_named() {
    use crate::config::retention::RetentionConfig;
    use crate::config::session_env::Origin;
    let s = RetentionSettings::resolve(&RetentionConfig::default()).unwrap();
    assert_eq!(s.keep.value, Duration::from_secs(7 * DAY));
    assert_eq!(s.idle_period.value, Duration::from_secs(7 * DAY));
    assert_eq!(s.stale_after.value, Duration::from_secs(30 * DAY));

    let mut c = RetentionConfig::default();
    c.merge_file(
        &RetentionConfig {
            keep: Some("7d".into()),
            ..Default::default()
        },
        Origin::User,
    );
    c.apply_env(|v| (v == crate::config::retention::ENV_STALE_AFTER).then(|| "7d".to_string()));
    let e = RetentionSettings::resolve(&c).unwrap_err().to_string();
    assert!(e.contains("retention.stale_after"), "{e}");
    assert!(
        e.contains("KOTO_RETENTION_STALE_AFTER") && e.contains("user config"),
        "{e}"
    );

    let mut c = RetentionConfig::default();
    c.apply_env(|v| (v == crate::config::retention::ENV_KEEP).then(|| "7x".to_string()));
    let e = RetentionSettings::resolve(&c).unwrap_err().to_string();
    assert!(e.starts_with("retention.keep"), "{e}");

    for unit in ["90s", "45m", "12h", "3d"] {
        let mut c = RetentionConfig::default();
        c.apply_env(|v| (v == crate::config::retention::ENV_IDLE_PERIOD).then(|| unit.to_string()));
        assert!(RetentionSettings::resolve(&c).is_ok(), "{unit}");
    }
}

// ===== Two levels equal a full read =====

/// A tiny deterministic generator, so the property test needs no crate.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

/// Write a random store: koto sessions in every standing, anchors
/// (observed or not, some closed, some with a malformed close), links among
/// them and to absent names, lineage, migrated pairs and tainted members.
fn random_store(rng: &mut Rng) -> Store {
    let s = store();
    let n = 6 + rng.below(14) as usize;
    let mut names = Vec::new();
    for i in 0..n {
        let roll = rng.below(10);
        let name = if roll < 3 {
            format!("anchor-h{i}")
        } else {
            format!("s{i}")
        };
        if name.starts_with("anchor-") {
            s.anchor(&name, &format!("h{i}"), rng.below(200));
            if rng.chance(30) {
                s.append(
                    &name,
                    rng.below(400),
                    "anchor_activity",
                    json!({"session": "x", "command": "next"}),
                );
            }
            if rng.chance(50) {
                s.observe(&name, &format!("k{i}"));
                if rng.chance(50) {
                    s.close(&name, &format!("k{i}"), rng.below(400));
                    if rng.chance(20) {
                        std::fs::write(
                            s.dir(&format!("{name}-closed"))
                                .join("ctx/koto/anchor-close"),
                            b"{",
                        )
                        .unwrap();
                    }
                }
            }
        } else {
            match rng.below(4) {
                0 => s.running(&name),
                1 => s.cancelled(&name, rng.below(400)),
                _ => s.finished(&name, rng.below(400)),
            }
        }
        names.push(name);
    }
    for (i, name) in names.clone().iter().enumerate() {
        for _ in 0..rng.below(3) {
            let peer = if rng.chance(15) {
                format!("gone{}", rng.below(5))
            } else {
                names[rng.below(n as u64) as usize].clone()
            };
            if &peer == name {
                continue;
            }
            let kind = if peer.starts_with("anchor-") && !name.starts_with("anchor-") {
                "held-by"
            } else {
                "dispatched-by"
            };
            s.link(name, kind, &peer);
        }
        if !name.starts_with("anchor-") && i > 0 && rng.chance(20) {
            let parent = names[rng.below(i as u64) as usize].clone();
            let mut text = std::fs::read_to_string(s.log(name)).unwrap();
            let first = text.lines().next().unwrap().to_string();
            let mut header: Value = serde_json::from_str(&first).unwrap();
            header["parent_workflow"] = json!(parent);
            text = text.replacen(&first, &header.to_string(), 1);
            std::fs::write(s.log(name), text).unwrap();
        }
        if rng.chance(5) {
            let path = s.dir(name).join("ctx/chain");
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("parent"), b" bad ").unwrap();
        }
    }
    // A migrated pair, sometimes.
    if rng.chance(40) && n > 2 {
        let source = names[rng.below(n as u64) as usize].clone();
        if !source.starts_with("anchor-") {
            let copy = format!("{source}-moved");
            s.running(&copy);
            import(&s, &copy, &source, 500);
        }
    }
    s
}

/// The reference: inspect every session in full first, then find the
/// component by a breadth-first walk over the links, lineage and
/// `chain/parent` each session's own facts name, resolving migrated names
/// to their copies, and judge it from those inspections alone. Answers for
/// every session in the store: whether its hierarchy is kept, and its
/// present members.
fn reference(
    base: &Path,
    settings: &RetentionSettings,
    now: SystemTime,
) -> BTreeMap<String, (bool, BTreeSet<String>)> {
    let graph = Graph::scan(base).unwrap();
    let names: Vec<String> = graph.names().map(str::to_string).collect();
    // Every session inspected in full, up front, with what it alone can't
    // read.
    let mut full: BTreeMap<String, (Member, bool)> = BTreeMap::new();
    let mut inspector = Judge::new(&graph, settings.clone(), now);
    for n in &names {
        let j = inspector.judge(n, &Scope::Whole).unwrap();
        let own_unreadable = unreadable(&j).iter().any(|u| &u.name == n);
        full.insert(n.clone(), (member(&j, n).clone(), own_unreadable));
    }
    let mut adjacency: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let copies_of = |n: &str| -> Vec<String> {
        names
            .iter()
            .filter(|c| {
                graph
                    .facts(c)
                    .and_then(|f| f.imported_from.as_ref())
                    .is_some_and(|i| i.from_session == n)
                    && graph.is_migrated(n)
            })
            .cloned()
            .collect()
    };
    for n in &names {
        if graph.is_migrated(n) {
            continue;
        }
        let f = graph.facts(n).unwrap();
        let mut named: Vec<String> = f.links.iter().map(|l| l.peer.clone()).collect();
        if let Some(pw) = &f.parent_workflow {
            named.push(lineage_target(pw, |x| graph.contains(x)));
        }
        named.extend(f.chain_parent.clone());
        for peer in named {
            let targets = if graph.is_migrated(&peer) {
                copies_of(&peer)
            } else if graph.contains(&peer) {
                vec![peer]
            } else {
                vec![]
            };
            for t in targets {
                adjacency.entry(n.clone()).or_default().insert(t.clone());
                adjacency.entry(t).or_default().insert(n.clone());
            }
        }
    }
    let mut answers = BTreeMap::new();
    for name in &names {
        let mut seen = BTreeSet::from([name.clone()]);
        let mut queue = VecDeque::from([name.clone()]);
        while let Some(n) = queue.pop_front() {
            if graph.is_migrated(&n) {
                continue;
            }
            for m in adjacency.get(&n).into_iter().flatten() {
                if seen.insert(m.clone()) {
                    queue.push_back(m.clone());
                }
            }
        }
        let tainted = !graph.store_problems().is_empty()
            || seen
                .iter()
                .any(|m| !graph.facts(m).unwrap().problems.is_empty());
        let alive = seen.iter().any(|m| full[m].0.alive);
        let latest = seen
            .iter()
            .filter_map(|m| full[m].0.stopped_at.clone())
            .max();
        let unreadable_member = seen.iter().any(|m| full[m].1);
        let kept = alive
            || tainted
            || unreadable_member
            || latest.is_some_and(|l| {
                crate::engine::wake::parse_rfc3339_millis(&l).unwrap() + settings.keep.value > now
            });
        answers.insert(name.clone(), (kept, seen));
    }
    answers
}

#[test]
fn the_two_level_verdict_equals_a_full_read_reference() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for round in 0..60 {
        let s = random_store(&mut rng);
        let settings = RetentionSettings::from_durations(
            Duration::from_secs(50 + rng.below(100)),
            Duration::from_secs(50 + rng.below(300)),
            Duration::from_secs(400),
        );
        let now = at(300 + rng.below(400));
        let graph = Graph::scan(&s.base).unwrap();
        let mut judge = Judge::new(&graph, settings.clone(), now);
        let answers = reference(&s.base, &settings, now);
        for name in graph.names() {
            let j = judge.judge(name, &Scope::Whole).unwrap();
            let present: BTreeSet<String> = j
                .members
                .iter()
                .filter(|m| !m.absent && !(m.migrated && m.name != name))
                .map(|m| m.name.clone())
                .collect();
            let (kept, members) = &answers[name];
            assert_eq!(&present, members, "round {round}, {name}: members");
            assert_eq!(j.verdict.is_kept(), *kept, "round {round}, {name}: {j:?}");
            // The terminal tick's probe never answers "no edge" for a session
            // the graph joins to anything.
            let joined = graph
                .edges()
                .iter()
                .any(|e| e.holder == name || e.target == name);
            if joined {
                assert!(edge_of(&s.base, name), "round {round}, {name}: edge_of");
            }
        }
    }
}

/// The terminal tick removes a session only when `edge_of` answers "no
/// edge", so it must never miss an edge the graph joins a session through,
/// session-id hints included. Over random stores (migrated copies among
/// them) where some links name one session and carry the id of another, as
/// a link does once a re-run moves its peer aside or an import copies it,
/// every session the graph joins to anything has an edge.
#[test]
fn edge_of_never_misses_an_edge_the_graph_joins_hints_included() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut hinted = 0;
    for round in 0..60 {
        let s = random_store(&mut rng);
        let first = Graph::scan(&s.base).unwrap();
        let names: Vec<String> = first.names().map(str::to_string).collect();
        let pick = |rng: &mut Rng| names[rng.below(names.len() as u64) as usize].clone();
        for _ in 0..1 + rng.below(4) {
            let (holder, named, carrier) = (pick(&mut rng), pick(&mut rng), pick(&mut rng));
            if holder == named || holder == carrier || named == carrier {
                continue;
            }
            let Some(id) = first.facts(&carrier).and_then(|f| f.session_id.clone()) else {
                continue;
            };
            let body = json!({"v": 1, "session": named, "session_id": id, "at": ts(0)});
            // A holder koto refuses to write (a migrated source) just
            // contributes no hint.
            let _ = s.backend.add_with_writer(
                &holder,
                &format!("koto/links/dispatched-by/{named}"),
                body.to_string().as_bytes(),
                WRITER_KOTO,
            );
        }
        let graph = Graph::scan(&s.base).unwrap();
        for name in graph.names() {
            let touching: Vec<_> = graph
                .edges()
                .iter()
                .filter(|e| e.holder == name || e.target == name)
                .collect();
            // Joined by a hint alone: the case a probe by name would miss.
            hinted += usize::from(
                !touching.is_empty()
                    && touching
                        .iter()
                        .all(|e| e.target == name && matches!(e.via, Via::SessionIdHint(_))),
            );
            if !touching.is_empty() {
                assert!(edge_of(&s.base, name), "round {round}, {name}: edge_of");
            }
        }
    }
    assert!(hinted > 0, "no session was joined by a hint alone");
}

// ===== The continues relation =====

impl Store {
    /// `holder` continues `pred`: a continues link whose body records
    /// `pred_id` (or no id).
    fn continues(&self, holder: &str, pred: &str, pred_id: Option<&str>) {
        let body = json!({
            "v": 1,
            "session": pred,
            "session_id": pred_id,
            "workspace": null,
            "prefix": null,
            "holder_session_id": format!("id-{holder}"),
            "at": ts(0),
        });
        self.key(
            holder,
            &format!("koto/links/continues/{pred}"),
            body.to_string().as_bytes(),
        );
    }

    /// Pin `name` by hand with `reason`.
    fn pin(&self, name: &str, reason: &str) {
        let body = json!({
            "v": 1,
            "id": "0123456789abcdef0123456789abcdef",
            "reason": reason,
            "set_by": null,
            "set_at": ts(1),
        });
        self.key(
            name,
            crate::session::pin::PIN_KEY,
            body.to_string().as_bytes(),
        );
    }

    /// The chain v1 <- v2 <- v3, each continuing the one before by name
    /// and session id: v1 and v2 finished at 10 and 20, v3 running.
    fn chain(&self) {
        self.finished("v1", 10);
        self.finished("v2", 20);
        self.running("v3");
        self.continues("v2", "v1", Some("id-v1"));
        self.continues("v3", "v2", Some("id-v2"));
    }

    fn relation(&self, name: &str, keep: u64, now: u64) -> RelationVerdict {
        let graph = Graph::scan(&self.base).unwrap();
        let mut judge = Judge::new(
            &graph,
            RetentionSettings::from_durations(
                Duration::from_secs(keep),
                Duration::from_secs(DAY),
                Duration::from_secs(2 * DAY),
            ),
            at(now),
        );
        judge.relation(name)
    }
}

/// `(keeper, standing, path)` for each keeper, in order.
fn relation_keepers(r: &RelationVerdict) -> Vec<(String, &'static str, Vec<String>)> {
    r.keepers
        .iter()
        .map(|k| (k.keeper.clone(), k.standing.as_str(), k.path.clone()))
        .collect()
}

fn relation_unreadable(r: &RelationVerdict) -> Vec<(String, &'static str)> {
    r.unreadable
        .iter()
        .map(|u| (u.holder.clone(), u.reason.as_str()))
        .collect()
}

fn path(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| n.to_string()).collect()
}

#[test]
fn a_running_successor_keeps_every_earlier_member_and_names_the_path() {
    let s = store();
    s.chain();
    let j1 = s.judge("v1", 10, DAY, 2 * DAY, 1000);
    // The hierarchy is v1's alone, and released: the relation joins nothing.
    assert!(released(&j1));
    assert_eq!(member(&j1, "v1").joined_by, vec![]);
    assert_eq!(j1.members.len(), 1);
    assert!(j1.keeps("v1"));
    assert_eq!(
        relation_keepers(j1.relation_of("v1")),
        vec![("v3".to_string(), "alive", path(&["v2"]))]
    );
    let j2 = s.judge("v2", 10, DAY, 2 * DAY, 1000);
    assert_eq!(
        relation_keepers(j2.relation_of("v2")),
        vec![("v3".to_string(), "alive", vec![])]
    );
    assert_eq!(j2.predecessor.as_deref(), Some("v1"));
    assert_eq!(j2.successors, vec!["v3".to_string()]);
    // Nothing later than v3, so nothing keeps it through a relation.
    let j3 = s.judge("v3", 10, DAY, 2 * DAY, 1000);
    assert!(!j3.relation_of("v3").keeps());
    assert_eq!(j3.predecessor.as_deref(), Some("v2"));
    assert!(j3.successors.is_empty());
}

#[test]
fn the_relation_releases_once_the_last_member_loses_standing() {
    let s = store();
    s.chain();
    // v3 stops at 100, with a retention of 50.
    s.finished("v3", 100);
    let r = s.relation("v1", 50, 120);
    assert_eq!(
        relation_keepers(&r),
        vec![("v3".to_string(), "retention", path(&["v2"]))]
    );
    assert_eq!(
        r.keepers[0].retention_passes_at.as_deref(),
        Some(ts(150).as_str())
    );
    // v2's own retention (stop 20) passed long ago and gives no standing.
    assert!(!r.keepers.iter().any(|k| k.keeper == "v2"));
    // At the instant v3's retention passes, nothing keeps v1 or v2.
    assert!(!s.relation("v1", 50, 150).keeps());
    assert!(!s.relation("v2", 50, 151).keeps());
    let j = s.judge("v1", 50, DAY, 2 * DAY, 151);
    assert!(!j.keeps("v1"));
}

#[test]
fn a_pin_on_the_newest_member_keeps_the_chain_and_one_on_the_oldest_only_itself() {
    let s = store();
    s.chain();
    s.finished("v3", 30);
    s.pin("v3", "keep the chain");
    let r = s.relation("v1", 10, 1000);
    assert_eq!(
        relation_keepers(&r),
        vec![("v3".to_string(), "pin", path(&["v2"]))]
    );
    match &r.keepers[0].pin {
        crate::session::pin::PinFact::Present(p) => assert_eq!(p.reason, "keep the chain"),
        other => panic!("{other:?}"),
    }
    let j = s.judge("v1", 10, DAY, 2 * DAY, 1000);
    assert!(j.keeps("v1"));
    // The pin keeps v1 through the relation, not as v1's own pin.
    assert!(!j.pin.is_pinned());

    let s = store();
    s.chain();
    s.finished("v3", 30);
    s.pin("v1", "only the oldest");
    let j1 = s.judge("v1", 10, DAY, 2 * DAY, 1000);
    assert!(j1.keeps("v1"));
    assert!(!j1.relation_of("v1").keeps());
    for later in ["v2", "v3"] {
        let j = s.judge(later, 10, DAY, 2 * DAY, 1000);
        assert!(!j.keeps(later), "{later}: {j:?}");
    }
}

#[test]
fn nothing_earlier_keeps_anything_later() {
    let s = store();
    s.running("v1");
    s.finished("v2", 10);
    s.finished("v3", 20);
    s.continues("v2", "v1", Some("id-v1"));
    s.continues("v3", "v2", Some("id-v2"));
    for later in ["v2", "v3"] {
        let j = s.judge(later, 5, DAY, 2 * DAY, 1000);
        assert!(!j.keeps(later), "{later}: {j:?}");
        assert!(keepers(&j).is_empty());
    }
}

#[test]
fn two_successors_each_keep_the_predecessor_named_in_order() {
    let s = store();
    s.finished("v1", 10);
    s.finished("v2b", 90);
    s.running("v2a");
    s.continues("v2b", "v1", Some("id-v1"));
    s.continues("v2a", "v1", Some("id-v1"));
    let r = s.relation("v1", 50, 100);
    assert_eq!(
        relation_keepers(&r),
        vec![
            ("v2a".to_string(), "alive", vec![]),
            ("v2b".to_string(), "retention", vec![]),
        ]
    );
    s.finished("v2a", 95);
    assert_eq!(s.relation("v1", 50, 139).keepers.len(), 2);
    assert_eq!(
        relation_keepers(&s.relation("v1", 50, 141)),
        vec![("v2a".to_string(), "retention", vec![])]
    );
    assert!(!s.relation("v1", 50, 146).keeps());
    let j = s.judge("v1", 50, DAY, 2 * DAY, 146);
    assert_eq!(j.successors, vec!["v2a".to_string(), "v2b".to_string()]);
}

#[test]
fn the_relation_changes_no_hierarchy() {
    // v1 is a koto child of p, v2 a child of q, and v2 continues v1.
    let s = store();
    s.running("p");
    s.with_parent("v1", "p", Some(10));
    s.finished("q", 30);
    s.with_parent("v2", "q", Some(20));
    let before: Vec<_> = ["p", "v1", "q", "v2"]
        .iter()
        .map(|n| {
            let j = s.judge(n, 50, DAY, 2 * DAY, 1000);
            (j.members.clone(), j.verdict.clone())
        })
        .collect();
    s.continues("v2", "v1", Some("id-v1"));
    let after: Vec<_> = ["p", "v1", "q", "v2"]
        .iter()
        .map(|n| {
            let j = s.judge(n, 50, DAY, 2 * DAY, 1000);
            (j.members.clone(), j.verdict.clone())
        })
        .collect();
    assert_eq!(before, after);
}

#[test]
fn an_unreadable_relation_keeps_what_it_names_without_tainting_a_hierarchy() {
    for (body, reason) in [
        (b"not json".to_vec(), "unparseable"),
        (vec![b' '; 5 * 1024], "too_large"),
    ] {
        let s = store();
        s.finished("v1", 10);
        s.finished("v2", 20);
        s.key("v2", "koto/links/continues/v1", &body);
        let j1 = s.judge("v1", 5, DAY, 2 * DAY, 1000);
        assert!(j1.keeps("v1"));
        assert_eq!(
            relation_unreadable(j1.relation_of("v1")),
            vec![("v2".to_string(), reason)]
        );
        // The holder's own hierarchy is released: the bad body is the
        // relation's problem, not v2's.
        let j2 = s.judge("v2", 5, DAY, 2 * DAY, 1000);
        assert!(released(&j2), "{reason}: {j2:?}");
        assert!(!j2.keeps("v2"));
    }
}

#[test]
fn a_recorded_id_that_names_no_session_is_a_mismatch_and_keeps_the_name() {
    let s = store();
    // v1 was removed by hand and a new v1 created; v2's relation records
    // the old id.
    s.finished("v1", 10);
    s.finished("v2", 20);
    s.continues("v2", "v1", Some("id-old-v1"));
    let j = s.judge("v1", 5, DAY, 2 * DAY, 1000);
    assert!(j.keeps("v1"));
    assert_eq!(
        relation_unreadable(j.relation_of("v1")),
        vec![("v2".to_string(), "mismatch")]
    );
}

#[test]
fn a_relation_follows_its_recorded_id_to_a_predecessor_moved_aside() {
    let s = store();
    // The predecessor moved to v1_r1 with its id; a new v1 took the name.
    s.write("v1_r1", json!({"session_id": "id-old-v1"}), &{
        let mut e = s.start_events();
        e.push((
            10,
            "transitioned",
            json!({"from": "work", "to": "done", "condition_type": "auto"}),
        ));
        e
    });
    s.finished("v1", 30);
    s.running("v2");
    s.continues("v2", "v1", Some("id-old-v1"));
    let moved = s.judge("v1_r1", 5, DAY, 2 * DAY, 1000);
    assert_eq!(
        relation_keepers(moved.relation_of("v1_r1")),
        vec![("v2".to_string(), "alive", vec![])]
    );
    let fresh = s.judge("v1", 5, DAY, 2 * DAY, 1000);
    assert!(!fresh.keeps("v1"), "{fresh:?}");
}

#[test]
fn a_cycle_keeps_every_session_on_it() {
    let s = store();
    s.finished("v1", 10);
    s.finished("v2", 20);
    s.finished("v3", 30);
    s.continues("v2", "v1", Some("id-v1"));
    s.continues("v3", "v2", Some("id-v2"));
    s.continues("v1", "v3", Some("id-v3"));
    for (name, closer) in [("v1", "v1"), ("v2", "v2"), ("v3", "v3")] {
        let j = s.judge(name, 5, DAY, 2 * DAY, 1000);
        assert!(j.keeps(name), "{name}");
        assert!(j.relation_of(name).keepers.is_empty());
        assert_eq!(
            relation_unreadable(j.relation_of(name)),
            vec![(closer.to_string(), "cycle")]
        );
    }
}

#[test]
fn a_session_whose_relations_cant_be_listed_keeps_everything() {
    let s = store();
    s.finished("v1", 10);
    s.finished("bad", 20);
    std::fs::create_dir_all(s.dir("bad").join("ctx")).unwrap();
    std::fs::write(s.dir("bad").join("ctx").join("manifest.json"), b"{").unwrap();
    let r = s.relation("v1", 5, 1000);
    assert_eq!(
        relation_unreadable(&r),
        vec![("bad".to_string(), "unlistable")]
    );
}

#[test]
fn a_successor_gone_since_the_scan_has_no_standing() {
    let s = store();
    s.finished("v1", 10);
    s.running("v2");
    s.continues("v2", "v1", Some("id-v1"));
    let graph = Graph::scan(&s.base).unwrap();
    s.remove("v2");
    let mut judge = Judge::new(
        &graph,
        RetentionSettings::from_durations(
            Duration::from_secs(5),
            Duration::from_secs(DAY),
            Duration::from_secs(2 * DAY),
        ),
        at(1000),
    );
    assert!(!judge.relation("v1").keeps());
}

#[test]
fn a_relation_naming_the_holders_own_name_names_nothing_here() {
    let s = store();
    s.finished("v1", 10);
    s.continues("v1", "v1", None);
    let j = s.judge("v1", 5, DAY, 2 * DAY, 1000);
    assert!(!j.keeps("v1"), "{j:?}");
    assert!(j.successors.is_empty());
}

#[test]
fn the_tick_probes_count_a_continues_link_on_both_sides() {
    let s = store();
    s.finished("v1", 10);
    s.finished("v2", 20);
    s.finished("other", 20);
    s.continues("v2", "v1", Some("id-v1"));
    assert!(edge_of(&s.base, "v1"));
    assert!(edge_of(&s.base, "v2"));
    assert!(!edge_of(&s.base, "other"));
    assert!(relation_probe(&s.base, "v1"));
    assert!(!relation_probe(&s.base, "v2"));
    assert!(!relation_probe(&s.base, "other"));
    // A relation that names a moved predecessor by its id is found too.
    s.write("v0_r1", json!({"session_id": "id-v0"}), &s.start_events());
    s.continues("other", "v0", Some("id-v0"));
    assert!(relation_probe(&s.base, "v0_r1"));
}

#[test]
fn the_relation_renders_its_fields_and_lines() {
    let s = store();
    s.chain();
    let r = s.relation("v1", 10, 1000);
    let fields = relation_json(&r);
    assert_eq!(fields["kept_by_relation"], json!(true));
    assert_eq!(
        fields["relation_keepers"],
        json!([{"keeper": "v3", "standing": "alive", "path": ["v2"]}])
    );
    assert!(!fields.contains_key("relation_unreadable"));
    assert!(!relation_report_json(&r).contains_key("kept_by_relation"));
    assert_eq!(
        relation_text(&r),
        vec!["kept: continued by v3 (alive) via v2".to_string()]
    );
    // Nothing for a session no relation keeps.
    assert!(relation_json(&RelationVerdict::default()).is_empty());
    assert!(relation_text(&RelationVerdict::default()).is_empty());
}

/// A random store with continues links among its koto sessions (some by
/// name with no id, some with a malformed body, some naming absent names),
/// over [`random_store`]'s.
fn random_store_with_relations(rng: &mut Rng) -> Store {
    let s = random_store(rng);
    let graph = Graph::scan(&s.base).unwrap();
    let names: Vec<String> = graph
        .names()
        .filter(|n| !n.starts_with("anchor-"))
        .map(str::to_string)
        .collect();
    for holder in &names {
        if !rng.chance(45) {
            continue;
        }
        let pred = if rng.chance(10) {
            format!("gone{}", rng.below(3))
        } else {
            names[rng.below(names.len() as u64) as usize].clone()
        };
        if &pred == holder {
            continue;
        }
        if rng.chance(8) {
            // A holder koto refuses to write (a migrated source) holds none.
            let _ = s.backend.add_with_writer(
                holder,
                &format!("koto/links/continues/{pred}"),
                b"{",
                WRITER_KOTO,
            );
        } else {
            let body = json!({"v": 1, "session": pred, "session_id": null, "at": ts(0)});
            let _ = s.backend.add_with_writer(
                holder,
                &format!("koto/links/continues/{pred}"),
                body.to_string().as_bytes(),
                WRITER_KOTO,
            );
        }
        if rng.chance(10) {
            s.pin(holder, "random");
        }
    }
    s
}

/// The directed reference: each session's later members found by a
/// breadth-first walk over the continues links each session's own facts
/// name (by name, the random stores record no ids), each judged from its
/// own full inspection. Answers, per session, the keepers' names and
/// whether the relation keeps it.
fn directed_reference(
    base: &Path,
    settings: &RetentionSettings,
    now: SystemTime,
) -> BTreeMap<String, (BTreeSet<String>, bool)> {
    let graph = Graph::scan(base).unwrap();
    let names: Vec<String> = graph.names().map(str::to_string).collect();
    let mut inspector = Judge::new(&graph, settings.clone(), now);
    let mut standing: BTreeMap<String, bool> = BTreeMap::new();
    for n in &names {
        let j = inspector.judge(n, &Scope::Whole).unwrap();
        let m = member(&j, n).clone();
        let within = m.stopped_at.as_deref().is_some_and(|at| {
            crate::engine::wake::parse_rfc3339_millis(at).unwrap() + settings.keep.value > now
        });
        let pinned = graph.facts(n).unwrap().pin.is_pinned();
        standing.insert(n.clone(), pinned || (!m.migrated && (m.alive || within)));
    }
    let mut later: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut broken: BTreeSet<String> = BTreeSet::new();
    for n in &names {
        for c in &graph.facts(n).unwrap().continues {
            if &c.peer == n || !graph.contains(&c.peer) {
                continue;
            }
            match &c.body {
                Ok(_) => later.entry(c.peer.clone()).or_default().push(n.clone()),
                Err(_) => {
                    broken.insert(c.peer.clone());
                }
            }
        }
    }
    let unlistable = names
        .iter()
        .filter(|n| graph.facts(n).unwrap().relations_unlistable)
        .count();
    let mut answers = BTreeMap::new();
    for name in &names {
        let mut seen = BTreeSet::from([name.clone()]);
        let mut queue = VecDeque::from([name.clone()]);
        let mut keepers = BTreeSet::new();
        let mut kept = broken.contains(name)
            || unlistable > usize::from(graph.facts(name).unwrap().relations_unlistable);
        while let Some(n) = queue.pop_front() {
            for h in later.get(&n).into_iter().flatten() {
                if !seen.insert(h.clone()) {
                    kept = true;
                    continue;
                }
                if broken.contains(h) {
                    kept = true;
                }
                if standing[h] {
                    keepers.insert(h.clone());
                }
                queue.push_back(h.clone());
            }
        }
        kept |= !keepers.is_empty();
        answers.insert(name.clone(), (keepers, kept));
    }
    answers
}

/// The relation is an overlay and nothing more: over random stores with
/// continues links, every hierarchy's members and verdict equal the
/// undirected reference's, which reads no continues link, and every
/// session's relation keepers equal a directed full-read reference's.
#[test]
fn the_relation_overlay_equals_a_directed_reference_and_joins_nothing() {
    let mut rng = Rng(0x517c_c1b7_2722_0a95);
    let mut related = 0;
    for round in 0..60 {
        let s = random_store_with_relations(&mut rng);
        let settings = RetentionSettings::from_durations(
            Duration::from_secs(50 + rng.below(100)),
            Duration::from_secs(50 + rng.below(300)),
            Duration::from_secs(400),
        );
        let now = at(300 + rng.below(400));
        let graph = Graph::scan(&s.base).unwrap();
        let mut judge = Judge::new(&graph, settings.clone(), now);
        let undirected = reference(&s.base, &settings, now);
        let directed = directed_reference(&s.base, &settings, now);
        for name in graph.names() {
            let j = judge.judge(name, &Scope::Whole).unwrap();
            let present: BTreeSet<String> = j
                .members
                .iter()
                .filter(|m| !m.absent && !(m.migrated && m.name != name))
                .map(|m| m.name.clone())
                .collect();
            let (kept, members) = &undirected[name];
            assert_eq!(&present, members, "round {round}, {name}: members");
            assert_eq!(j.verdict.is_kept(), *kept, "round {round}, {name}: {j:?}");
            let r = j.relation_of(name);
            let names: BTreeSet<String> = r.keepers.iter().map(|k| k.keeper.clone()).collect();
            let (want, want_kept) = &directed[name];
            assert_eq!(&names, want, "round {round}, {name}: keepers");
            assert_eq!(r.keeps(), *want_kept, "round {round}, {name}: {r:?}");
            assert_eq!(
                j.keeps(name),
                j.pin.is_pinned() || j.verdict.is_kept() || r.keeps(),
                "round {round}, {name}"
            );
            related += usize::from(!r.keepers.is_empty());
        }
    }
    assert!(related > 0, "no session was kept by a relation");
}

/// The tick's probes must find every continues link the overlay resolves,
/// session-id hints included: over random stores where some continues
/// links name one session and carry the id of another (a predecessor the
/// same-name replace moved aside), `edge_of` answers "edge" for both sides
/// of every resolved relation and `relation_probe` answers yes for its
/// target.
#[test]
fn the_tick_probes_never_miss_a_relation_the_overlay_resolves_hints_included() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d ^ 0x00c0_ffee);
    let mut hinted = 0;
    for round in 0..60 {
        let s = random_store_with_relations(&mut rng);
        let first = Graph::scan(&s.base).unwrap();
        let names: Vec<String> = first.names().map(str::to_string).collect();
        let pick = |rng: &mut Rng| names[rng.below(names.len() as u64) as usize].clone();
        for _ in 0..1 + rng.below(4) {
            let (holder, named, carrier) = (pick(&mut rng), pick(&mut rng), pick(&mut rng));
            if holder == named || holder == carrier || named == carrier {
                continue;
            }
            let Some(id) = first.facts(&carrier).and_then(|f| f.session_id.clone()) else {
                continue;
            };
            let body = json!({"v": 1, "session": named, "session_id": id, "at": ts(0)});
            let _ = s.backend.add_with_writer(
                &holder,
                &format!("koto/links/continues/{named}"),
                body.to_string().as_bytes(),
                WRITER_KOTO,
            );
        }
        let graph = Graph::scan(&s.base).unwrap();
        for d in graph.directed() {
            hinted += usize::from(d.named != d.target);
            assert!(edge_of(&s.base, &d.holder), "round {round}: {d:?}");
            assert!(edge_of(&s.base, &d.target), "round {round}: {d:?}");
            assert!(relation_probe(&s.base, &d.target), "round {round}: {d:?}");
        }
    }
    assert!(hinted > 0, "no relation resolved by a hint alone");
}

// ===== Layering =====

#[test]
fn the_session_layer_depends_on_nothing_in_the_cli() {
    // Built at run time, so this file doesn't match itself.
    let needle = ["crate", "::", "cli"].concat();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/session");
    let mut stack = vec![root];
    let mut offenders = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                if text.contains(&format!("{needle}::")) || text.contains(&format!("{needle};")) {
                    offenders.push(path.display().to_string());
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "src/session depends on src/cli: {offenders:?}"
    );
}
