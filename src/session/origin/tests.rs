//! Tests for `koto session origin`'s read: each state on an anchor and on a
//! workflow session through its links, anchors written before koto recorded
//! origin, the choice among several links, a failed link write, and a store
//! that isn't the local one.

use super::*;
use crate::engine::types::{Event, StateFileHeader};
use crate::session::anchor::{
    self, OpenRequest, Source, ORIGIN_LINK_FAILED, ORIGIN_MALFORMED, ORIGIN_SELF,
};
use crate::session::context_log::WRITER_KOTO;
use crate::session::links::{LinkBody, LinkHints, Prefix};
use crate::session::local::LocalBackend;
use crate::session::SessionError;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

// ===== Harness =====

fn backend() -> (tempfile::TempDir, LocalBackend) {
    let tmp = tempfile::TempDir::new().unwrap();
    let b = LocalBackend::with_base_dir(tmp.path().join("sessions"));
    (tmp, b)
}

fn header(name: &str, kind: Option<&str>) -> StateFileHeader {
    let mut h: StateFileHeader = serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "workflow": name,
        "template_hash": "abc",
        "created_at": "2026-01-01T00:00:00Z",
    }))
    .unwrap();
    h.session_id = format!("id-{name}");
    h.kind = kind.map(str::to_string);
    h
}

/// A workflow session holding just a header.
fn make_session(b: &LocalBackend, name: &str) {
    b.create(name).unwrap();
    b.append_header(name, &header(name, None)).unwrap();
}

/// An anchor written by hand: a header with `kind: "anchor"` and one
/// `anchor_started` per payload, in order.
fn make_anchor(b: &LocalBackend, name: &str, started: &[serde_json::Value]) {
    b.create(name).unwrap();
    b.append_header(name, &header(name, Some("anchor")))
        .unwrap();
    for payload in started {
        let s: AnchorStarted = serde_json::from_value(payload.clone()).unwrap();
        b.append_event(
            name,
            &EventPayload::AnchorStarted(s),
            "2026-01-01T00:00:00.000Z",
        )
        .unwrap();
    }
}

fn started(opened: &str, extra: serde_json::Value) -> serde_json::Value {
    let mut v = serde_json::json!({
        "source": "startup",
        "harness_session": "h",
        "opened": opened,
    });
    v.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    v
}

/// Write a `kind` link from `session` to `peer` with the given `at`, as
/// koto's own writer would.
fn link_at(b: &LocalBackend, session: &str, kind: LinkKind, peer: &str, at: &str) {
    let body = LinkBody {
        v: 1,
        session: peer.to_string(),
        session_id: None,
        workspace: None,
        prefix: Prefix::Absent,
        holder_session_id: None,
        at: at.to_string(),
        was: None,
        via: None,
    };
    b.add_with_writer(
        session,
        &links::link_key(kind, peer),
        &serde_json::to_vec(&body).unwrap(),
        WRITER_KOTO,
    )
    .unwrap();
}

fn link(b: &LocalBackend, session: &str, kind: LinkKind, peer: &str) {
    links::write_link(b, session, kind, peer, &LinkHints::default()).unwrap();
}

fn req(harness: &str, dispatched_by: Option<&str>) -> OpenRequest {
    OpenRequest {
        harness_session: harness.to_string(),
        source: Source::Startup,
        process: None,
        dispatched_by: dispatched_by.map(str::to_string),
        observed_key: None,
        workspace: None,
        transcript_path: None,
        authority: None,
    }
}

fn dispatched(by: &str, kind: DispatcherKind) -> Origin {
    Origin::Dispatched {
        dispatcher: by.to_string(),
        dispatcher_kind: kind,
    }
}

fn unrecorded(named: &str, reason: &str) -> Origin {
    Origin::unrecorded(named, reason)
}

fn report<S: OriginStore>(store: &S, name: &str) -> OriginReport {
    origin_of(store, name).unwrap()
}

/// SHA-256 of every file under `dir`, by relative path.
fn hashes(dir: &Path) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let bytes = std::fs::read(&p).unwrap();
                out.insert(
                    p.strip_prefix(dir).unwrap().to_path_buf(),
                    crate::cache::sha256_hex(&bytes),
                );
            }
        }
    }
    out
}

// ===== Anchors =====

#[test]
fn an_anchor_reports_each_state_its_open_recorded() {
    let (_tmp, b) = backend();
    anchor::open(&b, &req("d1", Some("anchor-parent"))).unwrap();
    anchor::open(&b, &req("n1", None)).unwrap();
    anchor::open(&b, &req("n2", Some(""))).unwrap();
    anchor::open(&b, &req("m1", Some("a b"))).unwrap();
    anchor::open(&b, &req("s1", Some("anchor-s1"))).unwrap();

    let got = report(&b, "anchor-d1");
    assert_eq!(got.session, "anchor-d1");
    assert_eq!(got.via.as_deref(), Some("anchor-d1"));
    assert_eq!(
        got.origin,
        dispatched("anchor-parent", DispatcherKind::Anchor)
    );
    assert_eq!(report(&b, "anchor-n1").origin, Origin::NoDispatcher);
    assert_eq!(report(&b, "anchor-n2").origin, Origin::NoDispatcher);
    match report(&b, "anchor-m1").origin {
        Origin::Unrecorded { named, reason } => {
            assert_eq!(reason, ORIGIN_MALFORMED);
            assert!(named.starts_with("sha256:"), "{named}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        report(&b, "anchor-s1").origin,
        unrecorded("anchor-s1", ORIGIN_SELF)
    );
}

#[test]
fn the_json_carries_the_fields_of_each_state_only() {
    let (_tmp, b) = backend();
    anchor::open(&b, &req("j1", Some("anchor-parent"))).unwrap();
    anchor::open(&b, &req("j2", None)).unwrap();
    anchor::open(&b, &req("j3", Some("anchor-j3"))).unwrap();

    let v = serde_json::to_value(report(&b, "anchor-j1")).unwrap();
    assert_eq!(
        v,
        serde_json::json!({
            "session": "anchor-j1",
            "via": "anchor-j1",
            "origin": {
                "state": "dispatched",
                "dispatcher": "anchor-parent",
                "dispatcher_kind": "anchor",
            },
        })
    );
    let v = serde_json::to_value(report(&b, "anchor-j2")).unwrap();
    assert_eq!(v["origin"], serde_json::json!({"state": "none"}));
    let v = serde_json::to_value(report(&b, "anchor-j3")).unwrap();
    assert_eq!(
        v["origin"],
        serde_json::json!({
            "state": "unrecorded",
            "named": "anchor-j3",
            "reason": ORIGIN_SELF,
        })
    );

    make_session(&b, "lone");
    let v = serde_json::to_value(report(&b, "lone")).unwrap();
    assert_eq!(
        v,
        serde_json::json!({"session": "lone", "via": null, "origin": {"state": "none"}})
    );
}

#[test]
fn a_continue_never_supplies_the_origin() {
    let (_tmp, b) = backend();
    // A real continue with another dispatcher: the creating event stands.
    anchor::open(&b, &req("c1", None)).unwrap();
    anchor::open(&b, &req("c1", Some("anchor-later"))).unwrap();
    assert_eq!(report(&b, "anchor-c1").origin, Origin::NoDispatcher);

    // A log whose first event is a continue naming a dispatcher, with its
    // link present, and whose creating event comes after it.
    make_anchor(
        &b,
        "anchor-c2",
        &[
            started(
                "continued",
                serde_json::json!({
                    "dispatched_by": "anchor-x",
                    "origin": {"state": "dispatched", "dispatcher": "anchor-x"},
                }),
            ),
            started("new", serde_json::json!({"origin": {"state": "none"}})),
        ],
    );
    link(&b, "anchor-c2", LinkKind::DispatchedBy, "anchor-x");
    assert_eq!(report(&b, "anchor-c2").origin, Origin::NoDispatcher);

    // The same for the legacy read: the continue's `dispatched_by` is not
    // the creating event's.
    make_anchor(
        &b,
        "anchor-c3",
        &[
            started("new", serde_json::json!({})),
            started(
                "continued",
                serde_json::json!({"dispatched_by": "anchor-x"}),
            ),
        ],
    );
    link(&b, "anchor-c3", LinkKind::DispatchedBy, "anchor-x");
    assert_eq!(
        report(&b, "anchor-c3").origin,
        unrecorded("", REASON_LEGACY)
    );

    // Reopened and successor events are creating events too.
    for (name, opened) in [("anchor-r1", "reopened"), ("anchor-r1-s2", "successor")] {
        make_anchor(
            &b,
            name,
            &[started(
                opened,
                serde_json::json!({"origin": {"state": "dispatched", "dispatcher": "anchor-p"}}),
            )],
        );
        link(&b, name, LinkKind::DispatchedBy, "anchor-p");
        assert_eq!(
            report(&b, name).origin,
            dispatched("anchor-p", DispatcherKind::Anchor),
            "{opened}"
        );
    }
}

#[test]
fn an_anchor_from_before_origin_reads_by_the_legacy_rule() {
    let (_tmp, b) = backend();
    make_anchor(
        &b,
        "anchor-old1",
        &[started(
            "new",
            serde_json::json!({"dispatched_by": "anchor-parent"}),
        )],
    );
    link(&b, "anchor-old1", LinkKind::DispatchedBy, "anchor-parent");
    assert_eq!(
        report(&b, "anchor-old1").origin,
        dispatched("anchor-parent", DispatcherKind::Anchor)
    );

    make_anchor(&b, "anchor-old2", &[started("new", serde_json::json!({}))]);
    assert_eq!(
        report(&b, "anchor-old2").origin,
        unrecorded("", REASON_LEGACY)
    );

    // A legacy event naming a dispatcher with no link beside it.
    make_anchor(
        &b,
        "anchor-old3",
        &[started(
            "new",
            serde_json::json!({"dispatched_by": "anchor-parent"}),
        )],
    );
    assert_eq!(
        report(&b, "anchor-old3").origin,
        unrecorded("anchor-parent", REASON_LINK_NOT_FOUND)
    );

    // A log with no anchor_started at all names nobody either.
    make_anchor(&b, "anchor-old4", &[]);
    assert_eq!(
        report(&b, "anchor-old4").origin,
        unrecorded("", REASON_LEGACY)
    );
}

#[test]
fn a_named_dispatcher_without_its_link_is_unrecorded_on_the_anchor_and_its_sessions() {
    let (_tmp, b) = backend();
    let creating = [started(
        "new",
        serde_json::json!({
            "dispatched_by": "anchor-parent",
            "origin": {"state": "dispatched", "dispatcher": "anchor-parent"},
        }),
    )];
    make_anchor(&b, "anchor-nl1", &creating);
    assert_eq!(
        report(&b, "anchor-nl1").origin,
        unrecorded("anchor-parent", REASON_LINK_NOT_FOUND)
    );
    make_session(&b, "w1");
    link(&b, "w1", LinkKind::HeldBy, "anchor-nl1");
    let got = report(&b, "w1");
    assert_eq!(got.via.as_deref(), Some("anchor-nl1"));
    assert_eq!(
        got.origin,
        unrecorded("anchor-parent", REASON_LINK_NOT_FOUND)
    );

    // With the event recording the failure, its reason is given.
    make_anchor(&b, "anchor-nl2", &creating);
    b.append_event(
        "anchor-nl2",
        &EventPayload::OriginUnrecorded(crate::engine::types::OriginUnrecorded {
            named: "anchor-parent".to_string(),
            reason: ORIGIN_LINK_FAILED.to_string(),
        }),
        "2026-01-01T00:00:01.000Z",
    )
    .unwrap();
    assert_eq!(
        report(&b, "anchor-nl2").origin,
        unrecorded("anchor-parent", ORIGIN_LINK_FAILED)
    );
}

#[test]
fn a_successors_link_to_its_predecessor_is_not_its_dispatcher() {
    let (_tmp, b) = backend();
    make_anchor(&b, "anchor-b", &[started("new", serde_json::json!({}))]);
    make_anchor(
        &b,
        "anchor-b-s2",
        &[started(
            "successor",
            serde_json::json!({
                "predecessor": "anchor-b",
                "reason": "closed",
                "origin": {"state": "none"},
            }),
        )],
    );
    link(&b, "anchor-b-s2", LinkKind::DispatchedBy, "anchor-b");
    assert_eq!(report(&b, "anchor-b-s2").origin, Origin::NoDispatcher);

    // A successor named a dispatcher whose link is missing, while its
    // predecessor link is there: still unrecorded.
    make_anchor(
        &b,
        "anchor-b-s3",
        &[started(
            "successor",
            serde_json::json!({
                "predecessor": "anchor-b",
                "reason": "closed",
                "dispatched_by": "anchor-d",
                "origin": {"state": "dispatched", "dispatcher": "anchor-d"},
            }),
        )],
    );
    link(&b, "anchor-b-s3", LinkKind::DispatchedBy, "anchor-b");
    assert_eq!(
        report(&b, "anchor-b-s3").origin,
        unrecorded("anchor-d", REASON_LINK_NOT_FOUND)
    );
}

// ===== Workflow sessions =====

#[test]
fn a_workflow_sessions_own_dispatched_by_link_comes_first() {
    let (_tmp, b) = backend();
    anchor::open(&b, &req("h1", None)).unwrap();
    make_session(&b, "w1");
    link(&b, "w1", LinkKind::HeldBy, "anchor-h1");
    link(&b, "w1", LinkKind::DispatchedBy, "coordinator");
    let got = report(&b, "w1");
    assert_eq!(got.via, None);
    assert_eq!(
        got.origin,
        dispatched("coordinator", DispatcherKind::Workflow)
    );
}

#[test]
fn a_workflow_session_reads_through_the_anchor_that_held_it() {
    let (_tmp, b) = backend();
    anchor::open(&b, &req("h1", Some("anchor-coord"))).unwrap();
    make_session(&b, "w1");
    let held = anchor::hold(&b, "w1", "anchor-h1", "init").unwrap();
    assert_eq!(held.linked, vec!["anchor-h1".to_string()]);
    let got = report(&b, "w1");
    assert_eq!(got.session, "w1");
    assert_eq!(got.via.as_deref(), Some("anchor-h1"));
    assert_eq!(
        got.origin,
        dispatched("anchor-coord", DispatcherKind::Anchor)
    );

    anchor::open(&b, &req("h2", None)).unwrap();
    make_session(&b, "w2");
    anchor::hold(&b, "w2", "anchor-h2", "init").unwrap();
    assert_eq!(report(&b, "w2").origin, Origin::NoDispatcher);
}

#[test]
fn a_held_by_anchor_that_is_gone_reports_anchor_not_found() {
    let (_tmp, b) = backend();
    anchor::open(&b, &req("gone", Some("anchor-coord"))).unwrap();
    make_session(&b, "w1");
    anchor::hold(&b, "w1", "anchor-gone", "init").unwrap();
    b.cleanup("anchor-gone").unwrap();
    let got = report(&b, "w1");
    assert_eq!(got.via.as_deref(), Some("anchor-gone"));
    assert_eq!(got.origin, unrecorded("", REASON_ANCHOR_NOT_FOUND));

    // An anchor that is there but can't be read is a failed read, not a
    // missing anchor.
    anchor::open(&b, &req("garbled", None)).unwrap();
    make_session(&b, "w3");
    anchor::hold(&b, "w3", "anchor-garbled", "init").unwrap();
    std::fs::write(
        b.session_dir("anchor-garbled")
            .join(crate::session::state_file_name("anchor-garbled")),
        "not a header\n",
    )
    .unwrap();
    let err = origin_of(&b, "w3").unwrap_err();
    assert!(err.downcast_ref::<OriginRefusal>().is_none(), "{err:#}");

    // A held-by link to a session that isn't an anchor reads the same.
    make_session(&b, "not-an-anchor");
    make_session(&b, "w2");
    link(&b, "w2", LinkKind::HeldBy, "not-an-anchor");
    assert_eq!(
        report(&b, "w2").origin,
        unrecorded("", REASON_ANCHOR_NOT_FOUND)
    );
}

#[test]
fn the_held_by_link_written_first_is_followed() {
    let (_tmp, b) = backend();
    anchor::open(&b, &req("aaa", Some("anchor-later-coord"))).unwrap();
    anchor::open(&b, &req("zzz", Some("anchor-first-coord"))).unwrap();
    make_session(&b, "w1");
    // The earlier link names the anchor whose name sorts last, so the name
    // fallback alone would pick the other one.
    link_at(
        &b,
        "w1",
        LinkKind::HeldBy,
        "anchor-zzz",
        "2026-01-01T00:00:00.000Z",
    );
    link_at(
        &b,
        "w1",
        LinkKind::HeldBy,
        "anchor-aaa",
        "2026-01-02T00:00:00.000Z",
    );
    let got = report(&b, "w1");
    assert_eq!(got.via.as_deref(), Some("anchor-zzz"));
    assert_eq!(
        got.origin,
        dispatched("anchor-first-coord", DispatcherKind::Anchor)
    );

    // Equal `at` values fall back to the lowest peer name; so do two
    // unreadable ones, and a readable one beats an unreadable one.
    make_session(&b, "w2");
    for peer in ["anchor-zzz", "anchor-aaa"] {
        link_at(&b, "w2", LinkKind::HeldBy, peer, "2026-01-01T00:00:00.000Z");
    }
    assert_eq!(report(&b, "w2").via.as_deref(), Some("anchor-aaa"));
    make_session(&b, "w3");
    for peer in ["anchor-zzz", "anchor-aaa"] {
        link_at(&b, "w3", LinkKind::HeldBy, peer, "not a time");
    }
    assert_eq!(report(&b, "w3").via.as_deref(), Some("anchor-aaa"));
    make_session(&b, "w4");
    link_at(&b, "w4", LinkKind::HeldBy, "anchor-aaa", "not a time");
    link_at(
        &b,
        "w4",
        LinkKind::HeldBy,
        "anchor-zzz",
        "2026-01-01T00:00:00.000Z",
    );
    assert_eq!(report(&b, "w4").via.as_deref(), Some("anchor-zzz"));

    // The same choice among dispatched-by links.
    make_session(&b, "w5");
    link_at(
        &b,
        "w5",
        LinkKind::DispatchedBy,
        "zeta",
        "2026-01-01T00:00:00.000Z",
    );
    link_at(
        &b,
        "w5",
        LinkKind::DispatchedBy,
        "alpha",
        "2026-01-03T00:00:00.000Z",
    );
    assert_eq!(
        report(&b, "w5").origin,
        dispatched("zeta", DispatcherKind::Workflow)
    );
}

#[test]
fn a_workflow_session_with_neither_link_reports_none() {
    let (_tmp, b) = backend();
    make_session(&b, "w1");
    // A continues link joins no origin.
    make_session(&b, "w0");
    links::write_continues(&b, "w1", "w0", &Default::default()).unwrap();
    let got = report(&b, "w1");
    assert_eq!(got.via, None);
    assert_eq!(got.origin, Origin::NoDispatcher);
}

#[test]
fn a_bad_name_or_a_missing_session_is_refused() {
    let (_tmp, b) = backend();
    for name in ["", "../x", "a b"] {
        let err = origin_of(&b, name).unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<OriginRefusal>(),
                Some(OriginRefusal::InvalidName(_))
            ),
            "{name:?}: {err}"
        );
    }
    let err = origin_of(&b, "nope").unwrap_err();
    assert_eq!(
        err.downcast_ref::<OriginRefusal>(),
        Some(&OriginRefusal::NoSuchSession)
    );
}

#[test]
fn the_read_writes_nothing() {
    let (tmp, b) = backend();
    anchor::open(&b, &req("h1", Some("anchor-coord"))).unwrap();
    make_session(&b, "w1");
    anchor::hold(&b, "w1", "anchor-h1", "init").unwrap();
    let before = hashes(tmp.path());
    report(&b, "w1");
    report(&b, "anchor-h1");
    assert_eq!(hashes(tmp.path()), before);
}

// ===== A failed link write =====

/// A local store whose dispatched-by link writes fail with an error
/// carrying [`STORE_SECRET`], as a remote store's error text might carry an
/// endpoint or a credential fragment.
struct LinkFails {
    inner: LocalBackend,
}

const STORE_SECRET: &str = "MARKER-endpoint-s3.example.internal-tok_7f3a9c";

impl SessionBackend for LinkFails {
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
    fn list(&self) -> anyhow::Result<Vec<crate::session::SessionInfo>> {
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
    fn ensure_pushed(&self, id: &str) -> Result<(), SessionError> {
        self.inner.ensure_pushed(id)
    }
    fn relocate(&self, from: &str, to: &str) -> anyhow::Result<()> {
        self.inner.relocate(from, to)
    }
    fn lock_state_file(&self, id: &str) -> Result<crate::session::SessionLock, SessionError> {
        self.inner.lock_state_file(id)
    }
}

impl OriginStore for LinkFails {
    fn read_key(&self, session: &str, key: &str) -> anyhow::Result<Vec<u8>> {
        self.inner.get(session, key)
    }
}

impl ContextStore for LinkFails {
    fn add(&self, session: &str, key: &str, content: &[u8]) -> anyhow::Result<()> {
        self.inner.add(session, key, content)
    }
    fn add_with_writer(
        &self,
        session: &str,
        key: &str,
        content: &[u8],
        writer: &str,
    ) -> anyhow::Result<()> {
        if key.starts_with(&format!("koto/links/{}/", LinkKind::DispatchedBy.as_str())) {
            anyhow::bail!("PUT {key} failed: 403 from {STORE_SECRET}");
        }
        self.inner.add_with_writer(session, key, content, writer)
    }
    fn get(&self, session: &str, key: &str) -> anyhow::Result<Vec<u8>> {
        self.inner.get(session, key)
    }
    fn ctx_exists(&self, session: &str, key: &str) -> bool {
        self.inner.ctx_exists(session, key)
    }
    fn remove(&self, session: &str, key: &str) -> anyhow::Result<()> {
        self.inner.remove(session, key)
    }
    fn list_keys(&self, session: &str, prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
        self.inner.list_keys(session, prefix)
    }
}

#[test]
fn after_a_failed_link_write_the_answer_is_unrecorded_and_carries_no_part_of_the_error() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = LinkFails {
        inner: LocalBackend::with_base_dir(tmp.path().join("sessions")),
    };
    // The double's error really does carry the marker through the link
    // writer, so its absence below is the open's and the read's doing.
    make_session(&store.inner, "probe");
    let err = links::write_link_via(
        &store,
        "probe",
        LinkKind::DispatchedBy,
        "anchor-parent",
        &LinkHints::default(),
        Some("anchor"),
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains(STORE_SECRET), "{err:#}");

    anchor::open(&store, &req("f1", Some("anchor-parent"))).unwrap();
    make_session(&store.inner, "w1");
    anchor::hold(&store, "w1", "anchor-f1", "init").unwrap();

    for name in ["anchor-f1", "w1"] {
        let got = report(&store, name);
        assert_eq!(got.via.as_deref(), Some("anchor-f1"), "{name}");
        assert_eq!(
            got.origin,
            unrecorded("anchor-parent", ORIGIN_LINK_FAILED),
            "{name}"
        );
        let json = serde_json::to_string(&got).unwrap();
        for part in [
            "MARKER",
            "endpoint",
            "s3.example",
            "internal",
            "tok_7f3a9c",
            "403",
            "PUT",
        ] {
            assert!(!json.contains(part), "{name}: {part} in {json}");
        }
    }
}

// ===== A store that isn't the local one =====

/// A store held in memory, with no sessions directory behind it: its
/// `session_dir` names a path that never exists, and every write fails. A
/// read that went to the local files, or that wrote anything, fails here.
#[derive(Default)]
struct Remote {
    sessions: HashMap<String, (StateFileHeader, Vec<Event>)>,
    keys: HashMap<(String, String), Vec<u8>>,
    forbidden_calls: Mutex<Vec<String>>,
}

impl Remote {
    fn session(&mut self, name: &str, kind: Option<&str>, events: Vec<serde_json::Value>) {
        let events = events
            .into_iter()
            .enumerate()
            .map(|(i, mut e)| {
                e["seq"] = serde_json::json!(i + 1);
                e["timestamp"] = serde_json::json!("2026-01-01T00:00:00.000Z");
                serde_json::from_value::<Event>(e).unwrap()
            })
            .collect();
        self.sessions
            .insert(name.to_string(), (header(name, kind), events));
    }

    fn key(&mut self, session: &str, kind: LinkKind, peer: &str) {
        let body = serde_json::json!({
            "v": 1,
            "session": peer,
            "session_id": null,
            "workspace": null,
            "at": "2026-01-01T00:00:00.000Z",
        });
        self.keys.insert(
            (session.to_string(), links::link_key(kind, peer)),
            serde_json::to_vec(&body).unwrap(),
        );
    }

    fn refuse(&self, what: String) -> anyhow::Error {
        self.forbidden_calls.lock().unwrap().push(what.clone());
        anyhow::anyhow!("write refused: {what}")
    }
}

impl SessionBackend for Remote {
    fn create(&self, id: &str) -> anyhow::Result<PathBuf> {
        Err(self.refuse(format!("create {id}")))
    }
    fn session_dir(&self, id: &str) -> PathBuf {
        PathBuf::from("/nonexistent/koto-origin-remote-double").join(id)
    }
    fn exists(&self, id: &str) -> bool {
        self.sessions.contains_key(id)
    }
    fn cleanup(&self, id: &str) -> anyhow::Result<()> {
        Err(self.refuse(format!("cleanup {id}")))
    }
    fn list(&self) -> anyhow::Result<Vec<crate::session::SessionInfo>> {
        Ok(Vec::new())
    }
    fn append_header(&self, id: &str, _header: &StateFileHeader) -> anyhow::Result<()> {
        Err(self.refuse(format!("append_header {id}")))
    }
    fn append_event(&self, id: &str, _payload: &EventPayload, _ts: &str) -> anyhow::Result<()> {
        Err(self.refuse(format!("append_event {id}")))
    }
    fn init_state_file(
        &self,
        id: &str,
        _header: StateFileHeader,
        _events: Vec<Event>,
    ) -> Result<(), SessionError> {
        Err(SessionError::Other(self.refuse(format!("init {id}"))))
    }
    fn read_events(&self, id: &str) -> anyhow::Result<(StateFileHeader, Vec<Event>)> {
        self.sessions
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no session {id}"))
    }
    fn read_header(&self, id: &str) -> anyhow::Result<StateFileHeader> {
        Ok(self.read_events(id)?.0)
    }
    fn ensure_pushed(&self, _id: &str) -> Result<(), SessionError> {
        Ok(())
    }
    fn relocate(&self, from: &str, _to: &str) -> anyhow::Result<()> {
        Err(self.refuse(format!("relocate {from}")))
    }
    fn lock_state_file(&self, id: &str) -> Result<crate::session::SessionLock, SessionError> {
        Err(SessionError::Other(self.refuse(format!("lock {id}"))))
    }
}

impl Remote {
    fn peek(&self, session: &str, key: &str) -> anyhow::Result<Vec<u8>> {
        self.keys
            .get(&(session.to_string(), key.to_string()))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no key {key}"))
    }
}

/// The read the origin verb uses instead of `get`, which writes nothing.
impl OriginStore for Remote {
    fn read_key(&self, session: &str, key: &str) -> anyhow::Result<Vec<u8>> {
        self.peek(session, key)
    }
}

impl ContextStore for Remote {
    fn add(&self, session: &str, key: &str, _content: &[u8]) -> anyhow::Result<()> {
        Err(self.refuse(format!("add {session} {key}")))
    }
    fn add_with_writer(
        &self,
        session: &str,
        key: &str,
        _content: &[u8],
        _writer: &str,
    ) -> anyhow::Result<()> {
        Err(self.refuse(format!("add {session} {key}")))
    }
    /// Counted as a write: the cloud backend's `get` may push this host's
    /// copy of the key to the bucket, so the origin read must never use it.
    fn get(&self, session: &str, key: &str) -> anyhow::Result<Vec<u8>> {
        self.forbidden_calls
            .lock()
            .unwrap()
            .push(format!("get (may push) {session} {key}"));
        self.peek(session, key)
    }
    /// Counted as a write too: a read that answered from it couldn't tell a
    /// failed read from a missing key.
    fn ctx_exists(&self, session: &str, key: &str) -> bool {
        self.forbidden_calls
            .lock()
            .unwrap()
            .push(format!("ctx_exists {session} {key}"));
        self.keys
            .contains_key(&(session.to_string(), key.to_string()))
    }
    fn remove(&self, session: &str, key: &str) -> anyhow::Result<()> {
        Err(self.refuse(format!("remove {session} {key}")))
    }
    fn list_keys(&self, session: &str, prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
        let mut keys: Vec<String> = self
            .keys
            .keys()
            .filter(|(s, k)| s == session && prefix.is_none_or(|p| k.starts_with(p)))
            .map(|(_, k)| k.clone())
            .collect();
        keys.sort();
        Ok(keys)
    }
}

#[test]
fn the_read_goes_through_the_configured_store_and_not_the_local_files() {
    let mut remote = Remote::default();
    let creating = |origin: serde_json::Value| {
        serde_json::json!({
            "type": "anchor_started",
            "payload": started("new", serde_json::json!({"origin": origin})),
        })
    };
    remote.session(
        "anchor-r1",
        Some("anchor"),
        vec![creating(
            serde_json::json!({"state": "dispatched", "dispatcher": "anchor-coord"}),
        )],
    );
    remote.key("anchor-r1", LinkKind::DispatchedBy, "anchor-coord");
    remote.session(
        "anchor-r2",
        Some("anchor"),
        vec![
            creating(serde_json::json!({"state": "dispatched", "dispatcher": "anchor-coord"})),
            serde_json::json!({
                "type": "origin_unrecorded",
                "payload": {"named": "anchor-coord", "reason": ORIGIN_LINK_FAILED},
            }),
        ],
    );
    // The event recording the failed link wins over a link written there
    // later: the pair still reads unrecorded.
    remote.key("anchor-r2", LinkKind::DispatchedBy, "anchor-coord");
    remote.session("w1", None, Vec::new());
    remote.key("w1", LinkKind::HeldBy, "anchor-r1");
    remote.session("w2", None, Vec::new());
    remote.key("w2", LinkKind::HeldBy, "anchor-r2");
    // Two held-by links, so their bodies are read to choose: equal `at`
    // values, so the lowest name wins.
    remote.session("w5", None, Vec::new());
    remote.key("w5", LinkKind::HeldBy, "anchor-r2");
    remote.key("w5", LinkKind::HeldBy, "anchor-r1");
    remote.session("w3", None, Vec::new());
    remote.key("w3", LinkKind::DispatchedBy, "coordinator");
    remote.session("w4", None, Vec::new());
    remote.key("w4", LinkKind::HeldBy, "anchor-missing");

    let r1 = report(&remote, "anchor-r1");
    assert_eq!(
        r1.origin,
        dispatched("anchor-coord", DispatcherKind::Anchor)
    );
    let w1 = report(&remote, "w1");
    assert_eq!(w1.via.as_deref(), Some("anchor-r1"));
    assert_eq!(
        w1.origin,
        dispatched("anchor-coord", DispatcherKind::Anchor)
    );
    assert_eq!(
        report(&remote, "w2").origin,
        unrecorded("anchor-coord", ORIGIN_LINK_FAILED)
    );
    assert_eq!(
        report(&remote, "w3").origin,
        dispatched("coordinator", DispatcherKind::Workflow)
    );
    assert_eq!(
        report(&remote, "w4").origin,
        unrecorded("", REASON_ANCHOR_NOT_FOUND)
    );
    let w5 = report(&remote, "w5");
    assert_eq!(w5.via.as_deref(), Some("anchor-r1"));
    assert_eq!(
        w5.origin,
        dispatched("anchor-coord", DispatcherKind::Anchor)
    );
    // Nothing went through `get`, which may push, or `ctx_exists`, which
    // can't report a failed read; and nothing was written.
    assert_eq!(
        *remote.forbidden_calls.lock().unwrap(),
        Vec::<String>::new()
    );
    assert!(!Path::new("/nonexistent/koto-origin-remote-double").exists());
}
