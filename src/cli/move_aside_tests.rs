//! Tests for the move-aside: the prefix-swap rule, the name choice, each
//! refusal, and each failure point of the procedure, against a local store
//! wrapped in a backend that fails at a chosen step the way the cloud
//! backend's remote half can.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{json, Value};

use super::*;
use crate::engine::types::{Event, EventPayload};
use crate::session::local::LocalBackend;
use crate::session::{state_file_name, SessionError, SessionInfo, SessionLock};

const TEMPLATE: &str = r#"---
name: move
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

/// A step the backend fails at.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Fail {
    /// `relocate_checked` to this name fails after its local rename, as a
    /// failed remote copy does.
    RelocateTo(String),
    /// `ensure_pushed` of this name fails.
    Push(String),
    /// `rewrite_header` of this name fails before writing.
    Rewrite(String),
    /// `relocate_local_only` from this name fails.
    MoveBackFrom(String),
    /// Every `remote_prefix_free` listing fails.
    Listing,
    /// `delete_remote_checked` of this name fails.
    Delete(String),
}

/// A local store standing in for a backend with a remote half: it records
/// what a remote half would be asked to do, and fails where told to.
struct Flaky {
    inner: LocalBackend,
    fails: Mutex<Vec<Fail>>,
    /// Names whose remote prefix holds something.
    remote_taken: Mutex<HashSet<String>>,
    pushed: Mutex<Vec<String>>,
    deleted: Mutex<Vec<String>>,
}

impl Flaky {
    fn failing(&self, f: &Fail) -> bool {
        self.fails.lock().unwrap().contains(f)
    }
}

impl SessionBackend for Flaky {
    fn create(&self, id: &str) -> anyhow::Result<PathBuf> {
        self.inner.create(id)
    }
    fn session_dir(&self, id: &str) -> PathBuf {
        self.inner.session_dir(id)
    }
    fn exists(&self, id: &str) -> bool {
        self.inner.exists(id)
    }
    fn cleanup(&self, id: &str) -> anyhow::Result<()> {
        self.inner.cleanup(id)
    }
    fn list(&self) -> anyhow::Result<Vec<SessionInfo>> {
        self.inner.list()
    }
    fn append_header(&self, id: &str, header: &StateFileHeader) -> anyhow::Result<()> {
        self.inner.append_header(id, header)
    }
    fn append_event(&self, id: &str, payload: &EventPayload, ts: &str) -> anyhow::Result<()> {
        self.inner.append_event(id, payload, ts)
    }
    fn init_state_file(
        &self,
        id: &str,
        header: StateFileHeader,
        events: Vec<Event>,
    ) -> Result<(), SessionError> {
        self.inner.init_state_file(id, header, events)
    }
    fn read_events(&self, id: &str) -> anyhow::Result<(StateFileHeader, Vec<Event>)> {
        self.inner.read_events(id)
    }
    fn read_header(&self, id: &str) -> anyhow::Result<StateFileHeader> {
        self.inner.read_header(id)
    }
    fn rewrite_header(
        &self,
        id: &str,
        f: &dyn Fn(StateFileHeader) -> StateFileHeader,
    ) -> anyhow::Result<()> {
        if self.failing(&Fail::Rewrite(id.to_string())) {
            anyhow::bail!("injected: rewrite of {id} failed");
        }
        self.inner.rewrite_header(id, f)
    }
    fn ensure_pushed(&self, id: &str) -> Result<(), SessionError> {
        if self.failing(&Fail::Push(id.to_string())) {
            return Err(SessionError::Other(anyhow::anyhow!(
                "injected: push of {id} failed"
            )));
        }
        self.pushed.lock().unwrap().push(id.to_string());
        Ok(())
    }
    fn relocate(&self, from: &str, to: &str) -> anyhow::Result<()> {
        self.inner.relocate(from, to)
    }
    fn remote_prefix_free(&self, id: &str) -> anyhow::Result<bool> {
        if self.failing(&Fail::Listing) {
            anyhow::bail!("injected: listing failed");
        }
        Ok(!self.remote_taken.lock().unwrap().contains(id))
    }
    fn relocate_checked(&self, from: &str, to: &str) -> anyhow::Result<()> {
        self.inner.relocate(from, to)?;
        if self.failing(&Fail::RelocateTo(to.to_string())) {
            anyhow::bail!("injected: a remote object of {from} could not be copied");
        }
        self.remote_taken.lock().unwrap().insert(to.to_string());
        Ok(())
    }
    fn relocate_local_only(&self, from: &str, to: &str) -> anyhow::Result<()> {
        if self.failing(&Fail::MoveBackFrom(from.to_string())) {
            anyhow::bail!("injected: local rename of {from} failed");
        }
        self.inner.relocate(from, to)
    }
    fn delete_remote_checked(&self, id: &str) -> anyhow::Result<()> {
        if self.failing(&Fail::Delete(id.to_string())) {
            anyhow::bail!("injected: delete of {id}'s prefix failed");
        }
        self.remote_taken.lock().unwrap().remove(id);
        self.deleted.lock().unwrap().push(id.to_string());
        Ok(())
    }
    fn lock_state_file(&self, id: &str) -> Result<SessionLock, SessionError> {
        self.inner.lock_state_file(id)
    }
}

struct Store {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    template: String,
    backend: Flaky,
}

fn store() -> Store {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("sessions");
    std::fs::create_dir_all(&base).unwrap();
    let tdir = tmp.path().join("templates");
    std::fs::create_dir_all(&tdir).unwrap();
    let src = tdir.join("move.md");
    std::fs::write(&src, TEMPLATE).unwrap();
    let (compiled, _) = crate::cache::compile_cached_into(&src, &tdir, true).unwrap();
    Store {
        backend: Flaky {
            inner: LocalBackend::with_base_dir(base.clone()),
            fails: Mutex::new(Vec::new()),
            remote_taken: Mutex::new(HashSet::new()),
            pushed: Mutex::new(Vec::new()),
            deleted: Mutex::new(Vec::new()),
        },
        base,
        template: compiled.to_string_lossy().into_owned(),
        _tmp: tmp,
    }
}

impl Store {
    fn log(&self, name: &str) -> PathBuf {
        self.base.join(name).join(state_file_name(name))
    }

    /// A session with `parent` as its `parent_workflow`, finished unless
    /// `running`.
    fn session(&self, name: &str, parent: Option<&str>, running: bool) {
        std::fs::create_dir_all(self.base.join(name)).unwrap();
        let mut header = json!({
            "schema_version": 1,
            "workflow": name,
            "template_hash": "abc",
            "created_at": "2026-10-01T00:00:00.000Z",
            "session_id": format!("id-{name}"),
        });
        if let Some(p) = parent {
            header["parent_workflow"] = Value::from(p);
        }
        let mut events = vec![
            json!({"seq": 1, "timestamp": "2026-10-01T00:00:00.000Z", "type": "workflow_initialized",
                   "payload": {"template_path": self.template, "variables": {}}}),
            json!({"seq": 2, "timestamp": "2026-10-01T00:00:00.000Z", "type": "transitioned",
                   "payload": {"from": null, "to": "work", "condition_type": "auto"}}),
        ];
        if !running {
            events.push(json!({"seq": 3, "timestamp": "2026-10-01T00:00:01.000Z",
                "type": "transitioned",
                "payload": {"from": "work", "to": "done", "condition_type": "auto"}}));
        }
        // Through the header type, so a restored header reads back byte for
        // byte.
        let header: StateFileHeader = serde_json::from_value(header).unwrap();
        let mut text = serde_json::to_string(&header).unwrap() + "\n";
        for e in events {
            text.push_str(&(serde_json::to_string(&e).unwrap() + "\n"));
        }
        std::fs::write(self.log(name), text).unwrap();
        std::fs::create_dir_all(self.base.join(name).join("ctx")).unwrap();
        std::fs::write(
            self.base.join(name).join("ctx").join("note"),
            format!("key of {name}"),
        )
        .unwrap();
    }

    fn parent_of(&self, name: &str) -> Option<String> {
        self.backend.read_header(name).unwrap().parent_workflow
    }

    fn exists(&self, name: &str) -> bool {
        self.base.join(name).exists()
    }

    /// Every file of every session, byte for byte.
    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut out = BTreeMap::new();
        let mut stack = vec![self.base.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.insert(
                        p.strip_prefix(&self.base).unwrap().to_path_buf(),
                        std::fs::read(&p).unwrap(),
                    );
                }
            }
        }
        out
    }

    fn fail(&self, f: Fail) {
        self.backend.fails.lock().unwrap().push(f);
    }

    /// The events of `name`'s log, without the header.
    fn events_text(&self, name: &str) -> String {
        let text = std::fs::read_to_string(self.log(name)).unwrap();
        text.lines().skip(1).collect::<Vec<_>>().join("\n")
    }
}

/// B, finished, under `coord`, with a subtree of every shape: a dotted child
/// with a grandchild, a relocated batch child under an epoch branch nothing
/// holds, and a child whose name is unrelated.
fn subtree(s: &Store) {
    s.session("coord", None, true);
    s.session("b", Some("coord"), false);
    s.session("b.a", Some("b"), false);
    s.session("b.a.g", Some("b.a"), false);
    s.session("b~2.c", Some("b~2"), false);
    s.session("other", Some("b"), false);
    s.session("unrelated", None, false);
}

#[test]
fn the_prefix_swap_rule() {
    assert_eq!(swap_prefix("b", "b", "b_r1"), "b_r1");
    assert_eq!(swap_prefix("b.x", "b", "b_r1"), "b_r1.x");
    assert_eq!(swap_prefix("b~3.x", "b", "b_r1"), "b_r1~3.x");
    assert_eq!(swap_prefix("b~3", "b", "b_r1"), "b_r1~3");
    assert_eq!(swap_prefix("coord", "b", "b_r1"), "coord");
    // A name that only starts with the same letters is another session.
    assert_eq!(swap_prefix("bx", "b", "b_r1"), "bx");
    assert_eq!(swap_prefix("b_r1", "b", "b_r2"), "b_r1");
}

#[test]
fn the_subtree_moves_with_lineage_re_pointed_and_logs_untouched() {
    let s = store();
    subtree(&s);
    let events_before: BTreeMap<&str, String> = ["b", "b.a", "b.a.g", "b~2.c", "other"]
        .into_iter()
        .map(|n| (n, s.events_text(n)))
        .collect();
    let unrelated = std::fs::read(s.log("unrelated")).unwrap();
    let coord = std::fs::read(s.log("coord")).unwrap();

    let moved = move_aside(&s.backend, "b").unwrap();
    assert_eq!(moved.moved_to, "b_r1");
    assert!(moved.undeleted.is_empty());
    // Deepest first, the predecessor last.
    let order: Vec<&str> = moved.renamed.iter().map(|(o, _)| o.as_str()).collect();
    assert_eq!(order.last(), Some(&"b"));
    let pos = |n: &str| order.iter().position(|o| *o == n).unwrap();
    assert!(pos("b.a.g") < pos("b.a"));

    for gone in ["b", "b.a", "b.a.g", "b~2.c"] {
        assert!(!s.exists(gone), "{gone} still under its old name");
    }
    // The predecessor keeps its own parent; the rest follow the swap.
    assert_eq!(s.parent_of("b_r1").as_deref(), Some("coord"));
    assert_eq!(s.parent_of("b_r1.a").as_deref(), Some("b_r1"));
    assert_eq!(s.parent_of("b_r1.a.g").as_deref(), Some("b_r1.a"));
    assert_eq!(s.parent_of("b_r1~2.c").as_deref(), Some("b_r1~2"));
    assert_eq!(s.parent_of("other").as_deref(), Some("b_r1"));
    // Events and keys are untouched; sessions outside the subtree too.
    for (old, new) in [
        ("b", "b_r1"),
        ("b.a", "b_r1.a"),
        ("b.a.g", "b_r1.a.g"),
        ("b~2.c", "b_r1~2.c"),
        ("other", "other"),
    ] {
        assert_eq!(s.events_text(new), events_before[old], "{old}'s events");
        assert_eq!(
            std::fs::read_to_string(s.base.join(new).join("ctx/note")).unwrap(),
            format!("key of {old}")
        );
    }
    assert_eq!(std::fs::read(s.log("unrelated")).unwrap(), unrelated);
    assert_eq!(std::fs::read(s.log("coord")).unwrap(), coord);
    // Every renamed session was pushed under its new name, every rewrite
    // pushed, and the old remote prefixes deleted.
    let pushed = s.backend.pushed.lock().unwrap().clone();
    for n in ["b_r1", "b_r1.a", "b_r1.a.g", "b_r1~2.c", "other"] {
        assert!(
            pushed.contains(&n.to_string()),
            "{n} not pushed: {pushed:?}"
        );
    }
    let deleted: HashSet<String> = s.backend.deleted.lock().unwrap().iter().cloned().collect();
    let expect: HashSet<String> = ["b", "b.a", "b.a.g", "b~2.c"]
        .iter()
        .map(|n| n.to_string())
        .collect();
    assert_eq!(deleted, expect);
}

#[test]
fn n_is_the_lowest_leaving_every_target_free_here_and_remotely() {
    let s = store();
    s.session("b", None, false);
    s.session("b.a", Some("b"), false);
    // _r1: the predecessor's name is taken locally.
    s.session("b_r1", None, false);
    // _r2: a descendant's target is taken locally.
    s.session("b_r2.a", None, false);
    // _r3: taken in the bucket only.
    s.backend
        .remote_taken
        .lock()
        .unwrap()
        .insert("b_r3".to_string());
    // _r10 being taken says nothing about _r4.
    s.backend
        .remote_taken
        .lock()
        .unwrap()
        .insert("b_r10".to_string());
    let moved = move_aside(&s.backend, "b").unwrap();
    assert_eq!(moved.moved_to, "b_r4");
    assert!(s.exists("b_r4.a"));
}

#[test]
fn a_second_and_third_move_take_distinct_names() {
    let s = store();
    s.session("b", None, false);
    assert_eq!(move_aside(&s.backend, "b").unwrap().moved_to, "b_r1");
    s.session("b", None, false);
    assert_eq!(move_aside(&s.backend, "b").unwrap().moved_to, "b_r2");
}

#[test]
fn a_live_or_unreadable_descendant_refuses_and_nothing_moves() {
    let s = store();
    subtree(&s);
    s.session("b.a.live", Some("b.a"), true);
    let before = s.snapshot();
    match move_aside(&s.backend, "b") {
        Err(MoveRefusal::HeldLive { live }) => assert_eq!(live, vec!["b.a.live".to_string()]),
        other => panic!("expected replace_held_live, got {other:?}"),
    }
    assert_eq!(s.snapshot(), before);

    // A header that can't be read hides the session from the listing; its
    // name puts it under the moved one, so it holds the move.
    let s = store();
    subtree(&s);
    std::fs::write(s.log("b.a.g"), "{\"not\": \"a header\"}\n").unwrap();
    let before = s.snapshot();
    match move_aside(&s.backend, "b") {
        Err(MoveRefusal::HeldLive { live }) => assert_eq!(live, vec!["b.a.g".to_string()]),
        other => panic!("expected replace_held_live, got {other:?}"),
    }
    assert_eq!(s.snapshot(), before);

    // A readable header over a log that can't be read: a descendant the
    // walk sees, whose status can't be read.
    let s = store();
    subtree(&s);
    let header = std::fs::read_to_string(s.log("other"))
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_string();
    std::fs::write(s.log("other"), format!("{header}\nnot an event\n")).unwrap();
    let before = s.snapshot();
    match move_aside(&s.backend, "b") {
        Err(MoveRefusal::HeldLive { live }) => assert_eq!(live, vec!["other".to_string()]),
        other => panic!("expected replace_held_live, got {other:?}"),
    }
    assert_eq!(s.snapshot(), before);
}

#[test]
fn a_failed_name_listing_refuses_before_anything_moves() {
    let s = store();
    subtree(&s);
    s.fail(Fail::Listing);
    let before = s.snapshot();
    assert!(matches!(
        move_aside(&s.backend, "b"),
        Err(MoveRefusal::Failed { .. })
    ));
    assert_eq!(s.snapshot(), before);
}

/// Each failure point of step 2 moves everything back: every file of every
/// session is as it was, and the new remote prefixes are deleted.
#[test]
fn each_failure_in_step_two_moves_everything_back() {
    let points = [
        // The first rename's remote copy (deepest first: a grandchild).
        Fail::RelocateTo("b_r1.a.g".to_string()),
        // The predecessor's own rename, last.
        Fail::RelocateTo("b_r1".to_string()),
        // The push of a renamed session.
        Fail::Push("b_r1.a".to_string()),
        // A header rewrite of a kept-name descendant, and its push.
        Fail::Rewrite("other".to_string()),
        Fail::Push("other".to_string()),
        // The predecessor's own rewrite.
        Fail::Rewrite("b_r1".to_string()),
    ];
    for point in points {
        let s = store();
        subtree(&s);
        let before = s.snapshot();
        s.fail(point.clone());
        match move_aside(&s.backend, "b") {
            Err(MoveRefusal::Failed { stranded, .. }) => {
                assert!(stranded.is_empty(), "{point:?}: {stranded:?}")
            }
            other => panic!("{point:?}: expected replace_failed, got {other:?}"),
        }
        assert_eq!(s.snapshot(), before, "{point:?}: the store wasn't restored");
        // Whatever was renamed had its new prefix deleted, and no old
        // prefix was touched.
        for d in s.backend.deleted.lock().unwrap().iter() {
            assert!(d.starts_with("b_r1"), "{point:?}: deleted {d}");
        }
        assert!(
            s.backend.remote_taken.lock().unwrap().is_empty(),
            "{point:?}"
        );
    }
}

#[test]
fn a_failed_delete_of_an_old_prefix_rolls_forward() {
    let s = store();
    subtree(&s);
    s.fail(Fail::Delete("b.a".to_string()));
    let moved = move_aside(&s.backend, "b").unwrap();
    assert_eq!(moved.moved_to, "b_r1");
    assert_eq!(moved.undeleted.len(), 1);
    assert_eq!(moved.undeleted[0].0, "b.a");
    assert!(s.exists("b_r1.a"));
    assert_eq!(s.parent_of("b_r1.a").as_deref(), Some("b_r1"));
    // The other old prefixes went.
    assert!(s.backend.deleted.lock().unwrap().contains(&"b".to_string()));
}

#[test]
fn a_failed_move_back_leaves_the_renamed_readable_and_the_next_move_picks_a_fresh_n() {
    let s = store();
    subtree(&s);
    // The move fails on the predecessor's rewrite, and moving the
    // grandchild back fails too.
    s.fail(Fail::Rewrite("b_r1".to_string()));
    s.fail(Fail::MoveBackFrom("b_r1.a.g".to_string()));
    match move_aside(&s.backend, "b") {
        Err(MoveRefusal::Failed { stranded, .. }) => assert_eq!(
            stranded,
            vec![("b.a.g".to_string(), "b_r1.a.g".to_string())]
        ),
        other => panic!("expected replace_failed, got {other:?}"),
    }
    // The stranded session reads under its new name, lineage re-pointed.
    assert!(s.exists("b_r1.a.g"));
    let h = s.backend.read_header("b_r1.a.g").unwrap();
    assert_eq!(h.workflow, "b_r1.a.g");
    assert_eq!(h.parent_workflow.as_deref(), Some("b_r1.a"));
    // Everything else is back.
    assert!(s.exists("b") && s.exists("b.a") && s.exists("b~2.c"));
    assert_eq!(s.parent_of("other").as_deref(), Some("b"));

    // The next replace skips the N the stranded session holds.
    s.backend.fails.lock().unwrap().clear();
    let moved = move_aside(&s.backend, "b").unwrap();
    assert_eq!(moved.moved_to, "b_r2");
}

#[test]
fn a_cycle_through_the_predecessor_moves_once() {
    let s = store();
    // b's parent is its own child: a cycle a removal and re-creation can
    // build.
    s.session("b", Some("b.x"), false);
    s.session("b.x", Some("b"), false);
    let moved = move_aside(&s.backend, "b").unwrap();
    assert_eq!(moved.moved_to, "b_r1");
    // The predecessor keeps its original parent, by name.
    assert_eq!(s.parent_of("b_r1").as_deref(), Some("b.x"));
    assert_eq!(s.parent_of("b_r1.x").as_deref(), Some("b_r1"));
}

#[test]
fn the_walk_follows_lineage_whatever_the_names() {
    let s = store();
    s.session("b", None, false);
    // An epoch-branch parent nothing holds is lineage to `b`.
    s.session("zz", Some("b~7"), false);
    let sessions = s.backend.list().unwrap();
    let tree = LineageTree::from_sessions(&sessions);
    assert_eq!(tree.children("b"), &["zz".to_string()]);
    let moved = move_aside(&s.backend, "b").unwrap();
    assert_eq!(s.parent_of("zz").as_deref(), Some("b_r1~7"));
    assert_eq!(moved.renamed, vec![("b".to_string(), "b_r1".to_string())]);
}
