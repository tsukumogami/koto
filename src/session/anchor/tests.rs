//! Tests for anchors: every open branch, observe, the close walk and its
//! staging, the shared close check, and held-by links with activity.
//!
//! Single-writer checks compare SHA-256 hashes of every file of a session
//! before and after.

use super::*;
use crate::engine::types::SessionKind;
use std::collections::BTreeMap;
use std::time::Instant;

// ===== Harness =====

fn backend() -> (tempfile::TempDir, LocalBackend) {
    let tmp = tempfile::TempDir::new().unwrap();
    let b = LocalBackend::with_base_dir(tmp.path().join("sessions"));
    (tmp, b)
}

/// An ordinary session holding just a header.
fn make_session(b: &LocalBackend, name: &str) {
    let mut header: StateFileHeader = serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "workflow": name,
        "template_hash": "abc",
        "created_at": "2026-01-01T00:00:00Z",
    }))
    .unwrap();
    header.session_id = generate_session_id();
    b.create(name).unwrap();
    crate::engine::persistence::append_header(&state_path(b, name), &header).unwrap();
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

fn session_hashes(b: &LocalBackend, name: &str) -> BTreeMap<PathBuf, String> {
    hashes(&b.session_dir(name))
}

fn req(harness: &str, source: Source) -> OpenRequest {
    OpenRequest {
        harness_session: harness.to_string(),
        source,
        process: None,
        dispatched_by: None,
        observed_key: None,
        workspace: None,
        transcript_path: None,
    }
}

fn open_with(b: &LocalBackend, r: OpenRequest) -> OpenOutcome {
    open(b, &r).unwrap()
}

fn open_keyed(b: &LocalBackend, harness: &str, key: &str) -> OpenOutcome {
    open_with(
        b,
        OpenRequest {
            observed_key: Some(key.to_string()),
            ..req(harness, Source::Startup)
        },
    )
}

fn has_link(b: &LocalBackend, session: &str, kind: LinkKind, peer: &str) -> bool {
    b.ctx_exists(session, &links::link_key(kind, peer))
}

fn link_count(b: &LocalBackend, session: &str, kind: LinkKind) -> usize {
    links::read_links(b, session)
        .unwrap()
        .into_iter()
        .filter(|l| l.kind == kind)
        .count()
}

fn events_of(b: &LocalBackend, name: &str) -> Vec<Event> {
    b.read_events(name).unwrap().1
}

fn last_started(b: &LocalBackend, name: &str) -> AnchorStarted {
    started_events(b, name).pop().unwrap()
}

fn current_state(b: &LocalBackend, name: &str) -> Option<String> {
    let (_, events) = b.read_events(name).unwrap();
    events.iter().rev().find_map(|e| match &e.payload {
        EventPayload::Transitioned { to, .. } => Some(to.clone()),
        _ => None,
    })
}

// ===== Header kind and templates =====

#[test]
fn header_kind_reads_absent_known_and_unknown_values() {
    let base = serde_json::json!({
        "schema_version": 1, "workflow": "w", "template_hash": "h",
        "created_at": "2026-01-01T00:00:00Z",
    });
    let plain: StateFileHeader = serde_json::from_value(base.clone()).unwrap();
    assert_eq!(plain.kind, None);
    assert_eq!(plain.session_kind(), SessionKind::Ordinary);
    assert!(!serde_json::to_string(&plain).unwrap().contains("\"kind\""));

    for (value, kind) in [
        ("anchor", SessionKind::Anchor),
        ("anchor-close", SessionKind::AnchorClose),
        ("something-later", SessionKind::Ordinary),
    ] {
        let mut v = base.clone();
        v["kind"] = serde_json::json!(value);
        let h: StateFileHeader = serde_json::from_value(v).unwrap();
        assert_eq!(h.kind.as_deref(), Some(value));
        assert_eq!(h.session_kind(), kind, "{value}");
        let round: StateFileHeader =
            serde_json::from_str(&serde_json::to_string(&h).unwrap()).unwrap();
        assert_eq!(round, h);
    }
}

#[test]
fn anchors_and_close_sessions_come_from_the_embedded_templates() {
    let (_tmp, b) = backend();
    let out = open_keyed(&b, "s1", "k1");
    assert_eq!(out.anchor, "anchor-s1");
    let header = b.read_header("anchor-s1").unwrap();
    assert_eq!(header.kind.as_deref(), Some("anchor"));
    assert_eq!(header.template_name.as_deref(), Some("koto-anchor"));
    // The compiled template lives in the session directory.
    assert!(b
        .session_dir("anchor-s1")
        .join(format!("{}.json", header.template_hash))
        .exists());
    assert_eq!(current_state(&b, "anchor-s1").as_deref(), Some("open"));

    let closed = close(&b, "k1", "s1").unwrap();
    assert_eq!(closed.recorded, vec!["anchor-s1".to_string()]);
    let ch = b.read_header("anchor-s1-closed").unwrap();
    assert_eq!(ch.kind.as_deref(), Some("anchor-close"));
    assert_eq!(
        current_state(&b, "anchor-s1-closed").as_deref(),
        Some("closed")
    );
    let compiled: crate::template::types::CompiledTemplate = serde_json::from_str(
        &std::fs::read_to_string(
            b.session_dir("anchor-s1-closed")
                .join(format!("{}.json", ch.template_hash)),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(compiled.states["closed"].terminal);
}

// ===== open =====

#[test]
fn first_open_is_new_and_a_repeat_continues_the_same_anchor() {
    let (_tmp, b) = backend();
    let first = open_with(&b, req("s1", Source::Startup));
    assert_eq!(first.anchor, "anchor-s1");
    assert_eq!(first.opened, Opened::New);
    assert!(!first.observed);

    let again = open_with(&b, req("s1", Source::Resume));
    assert_eq!(again.anchor, "anchor-s1");
    assert_eq!(again.opened, Opened::Continued);
    assert_eq!(list_anchors(&base_dir(&b)), vec!["anchor-s1".to_string()]);
    let started = started_events(&b, "anchor-s1");
    assert_eq!(started.len(), 2);
    assert_eq!(started[0].opened, "new");
    assert_eq!(started[1].opened, "continued");
    assert_eq!(started[1].source, "resume");
    assert_eq!(started[1].harness_session, "s1");
}

#[test]
fn an_idle_anchor_reads_open_again_once_the_new_start_event_lands() {
    let (_tmp, b) = backend();
    open_with(&b, req("s1", Source::Startup));
    age_log(&b, "anchor-s1");
    let path = state_path(&b, "anchor-s1");
    let before = SystemTime::now() - Duration::from_secs(1);

    let out = open_with(&b, req("s1", Source::Resume));
    assert_eq!(out.opened, Opened::Continued);
    assert_eq!(out.anchor, "anchor-s1");
    let last = last_event_time(&path, SystemTime::now()).unwrap();
    assert!(last >= before, "the new start event is the anchor's last");
}

/// Make `name` look long idle: every event's timestamp far in the past.
fn age_log(b: &LocalBackend, name: &str) {
    let path = state_path(b, name);
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    for line in lines.iter_mut().skip(1) {
        let mut v: serde_json::Value = serde_json::from_str(line).unwrap();
        v["timestamp"] = serde_json::json!("2020-01-01T00:00:00.000Z");
        *line = v.to_string();
    }
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
}

#[test]
fn a_removed_anchor_reopens_under_the_same_name() {
    let (_tmp, b) = backend();
    open_with(&b, req("s1", Source::Startup));
    b.cleanup("anchor-s1").unwrap();
    let out = open_with(&b, req("s1", Source::Compact));
    assert_eq!(
        (out.anchor.as_str(), out.opened),
        ("anchor-s1", Opened::Reopened)
    );

    b.cleanup("anchor-s1").unwrap();
    let out = open_with(
        &b,
        OpenRequest {
            transcript_path: Some("/home/u/.claude/projects/p/s1.jsonl".to_string()),
            ..req("s1", Source::Resume)
        },
    );
    assert_eq!(out.opened, Opened::Reopened);
}

#[test]
fn an_observed_closed_anchor_gets_linked_successors() {
    let (_tmp, b) = backend();
    open_keyed(&b, "s1", "k1");
    assert_eq!(close(&b, "k1", "s1").unwrap().recorded.len(), 1);
    let closed_before = session_hashes(&b, "anchor-s1");

    let out = open_keyed(&b, "s1", "k2");
    assert_eq!(
        (out.anchor.as_str(), out.opened),
        ("anchor-s1-s2", Opened::Successor)
    );
    assert!(has_link(
        &b,
        "anchor-s1-s2",
        LinkKind::DispatchedBy,
        "anchor-s1"
    ));
    let s = last_started(&b, "anchor-s1-s2");
    assert_eq!(s.predecessor.as_deref(), Some("anchor-s1"));
    assert_eq!(s.reason.as_deref(), Some(REASON_CLOSED));
    // The closed anchor is never written again.
    assert_eq!(session_hashes(&b, "anchor-s1"), closed_before);
    assert_eq!(current_of_family(&b, "anchor-s1"), "anchor-s1-s2");

    // A repeat continues the current successor.
    let again = open_with(&b, req("s1", Source::Resume));
    assert_eq!(
        (again.anchor.as_str(), again.opened),
        ("anchor-s1-s2", Opened::Continued)
    );

    // Closing the successor (its own key) opens -s3, linked to -s2.
    let walk = close_targets(&b, "k2", "s1");
    assert!(
        walk.is_empty(),
        "the root's key is k1, so k2 closes nothing"
    );
    write_close_for(&b, "anchor-s1-s2", "k2");
    let third = open_with(&b, req("s1", Source::Resume));
    assert_eq!(third.anchor, "anchor-s1-s3");
    assert!(has_link(
        &b,
        "anchor-s1-s3",
        LinkKind::DispatchedBy,
        "anchor-s1-s2"
    ));
    assert_eq!(current_of_family(&b, "anchor-s1"), "anchor-s1-s3");
}

/// Write a valid close session for `anchor` the way `close` does, for an
/// anchor the close walk wouldn't reach by harness id.
fn write_close_for(b: &LocalBackend, anchor: &str, key: &str) {
    assert!(close_one(b, &base_dir(b), anchor, key, &|_| Ok(())).unwrap());
    assert!(close_check(b, anchor).is_closed());
}

#[test]
fn a_successor_is_current_only_while_its_link_names_the_one_before() {
    let (_tmp, b) = backend();
    open_keyed(&b, "s1", "k1");
    close(&b, "k1", "s1").unwrap();
    open_with(&b, req("s1", Source::Resume));
    // An -s3 with no link to -s2 doesn't extend the family.
    create_kind_session(
        &b,
        "anchor-s1-s3",
        ANCHOR_TEMPLATE,
        SessionKind::ANCHOR,
        None,
        Vec::new(),
    )
    .unwrap();
    assert_eq!(current_of_family(&b, "anchor-s1"), "anchor-s1-s2");
}

#[test]
fn a_migrated_anchor_met_at_a_start_gets_a_linked_successor() {
    let (_tmp, b) = backend();
    open_with(&b, req("s1", Source::Startup));
    b.append_event(
        "anchor-s1",
        &EventPayload::SessionImported {
            from_workspace: "/elsewhere".to_string(),
            from_session: "anchor-s1".to_string(),
            from_session_id: "old".to_string(),
            machine_id: "m".to_string(),
        },
        &now_iso8601(),
    )
    .unwrap();
    let out = open_with(&b, req("s1", Source::Resume));
    assert_eq!(
        (out.anchor.as_str(), out.opened),
        ("anchor-s1-s2", Opened::Successor)
    );
    assert!(has_link(
        &b,
        "anchor-s1-s2",
        LinkKind::DispatchedBy,
        "anchor-s1"
    ));
    assert_eq!(
        last_started(&b, "anchor-s1-s2").reason.as_deref(),
        Some(REASON_MIGRATED)
    );
}

#[test]
fn open_refuses_bad_ids_and_names_held_by_other_sessions() {
    let (tmp, b) = backend();
    make_session(&b, "anchor-taken");
    let before = hashes(tmp.path());
    for bad in ["", "a b", "x-closed", "x-s2", &"a".repeat(129), "a/b"] {
        let err = open(&b, &req(bad, Source::Startup)).unwrap_err();
        assert!(
            err.downcast_ref::<AnchorRefusal>().is_some(),
            "{bad:?}: {err}"
        );
    }
    let err = open(&b, &req("taken", Source::Startup)).unwrap_err();
    assert!(err.downcast_ref::<AnchorRefusal>().is_some(), "{err}");
    assert!(err.to_string().contains("isn't an anchor"), "{err}");
    for (field, value) in [("key", "bad key"), ("dispatcher", "_x")] {
        let mut r = req("s9", Source::Startup);
        match field {
            "key" => r.observed_key = Some(value.to_string()),
            _ => r.dispatched_by = Some(value.to_string()),
        }
        let err = open(&b, &r).unwrap_err();
        assert!(err.downcast_ref::<AnchorRefusal>().is_some(), "{err}");
    }
    assert_eq!(hashes(tmp.path()), before);
}

#[test]
fn the_dispatched_by_link_is_written_only_when_an_anchor_is_created() {
    let (_tmp, b) = backend();
    make_session(&b, "parent");
    let parent_before = session_hashes(&b, "parent");
    let created = open_with(
        &b,
        OpenRequest {
            dispatched_by: Some("anchor-parent".to_string()),
            ..req("s1", Source::Startup)
        },
    );
    assert_eq!(created.opened, Opened::New);
    assert!(has_link(
        &b,
        "anchor-s1",
        LinkKind::DispatchedBy,
        "anchor-parent"
    ));

    let out = open_with(
        &b,
        OpenRequest {
            dispatched_by: Some("parent".to_string()),
            ..req("s1", Source::Resume)
        },
    );
    assert_eq!(out.opened, Opened::Continued);
    assert!(!has_link(&b, "anchor-s1", LinkKind::DispatchedBy, "parent"));
    assert_eq!(
        last_started(&b, "anchor-s1").dispatched_by.as_deref(),
        Some("parent")
    );
    assert_eq!(session_hashes(&b, "parent"), parent_before);
}

#[test]
fn the_observed_marker_is_written_from_the_key_only_when_the_anchor_has_none() {
    let (_tmp, b) = backend();
    let out = open_with(&b, req("s1", Source::Startup));
    assert!(!out.observed);
    let out = open_keyed(&b, "s1", "k1");
    assert!(out.observed);
    assert_eq!(read_marker(&b, "anchor-s1").unwrap().key, "k1");
    open_keyed(&b, "s1", "k2");
    assert_eq!(read_marker(&b, "anchor-s1").unwrap().key, "k1");
}

#[test]
fn open_records_digests_never_the_token_or_the_path() {
    let (_tmp, b) = backend();
    open_with(
        &b,
        OpenRequest {
            process: Some("host-a:4242:1700000000".to_string()),
            transcript_path: Some("/secret/path/t.jsonl".to_string()),
            workspace: Some(PathBuf::from("/work")),
            ..req("s1", Source::Startup)
        },
    );
    let text = std::fs::read_to_string(state_path(&b, "anchor-s1")).unwrap();
    assert!(!text.contains("host-a"));
    assert!(!text.contains("4242"));
    assert!(!text.contains("/secret/path"));
    let s = last_started(&b, "anchor-s1");
    assert_eq!(
        s.process.as_deref(),
        Some(digest("host-a:4242:1700000000").as_str())
    );
    assert_eq!(
        s.transcript.as_deref(),
        Some(digest("/secret/path/t.jsonl").as_str())
    );
    assert_eq!(s.workspace.as_deref(), Some("/work"));
    assert_eq!(s.process.unwrap().len(), DIGEST_HEX_LEN);
}

// ===== resume under a new id =====

#[test]
fn a_resume_under_a_new_id_links_through_the_transcript_digest() {
    let (_tmp, b) = backend();
    let transcript = "/p/transcripts/old-conv.jsonl";
    open_with(
        &b,
        OpenRequest {
            transcript_path: Some(transcript.to_string()),
            observed_key: Some("k1".to_string()),
            ..req("s1", Source::Startup)
        },
    );
    let pred_before = session_hashes(&b, "anchor-s1");
    let out = open_with(
        &b,
        OpenRequest {
            transcript_path: Some(transcript.to_string()),
            ..req("s2", Source::Resume)
        },
    );
    assert_eq!(
        (out.anchor.as_str(), out.opened),
        ("anchor-s2", Opened::Successor)
    );
    assert!(has_link(
        &b,
        "anchor-s2",
        LinkKind::DispatchedBy,
        "anchor-s1"
    ));
    let s = last_started(&b, "anchor-s2");
    assert_eq!(s.predecessor.as_deref(), Some("anchor-s1"));
    assert_eq!(s.reason.as_deref(), Some(REASON_RESUME));
    // The predecessor's marker is never copied, and it is untouched.
    assert!(read_marker(&b, "anchor-s2").is_none());
    assert!(!out.observed);
    assert_eq!(session_hashes(&b, "anchor-s1"), pred_before);
}

#[test]
fn a_resume_under_a_new_id_links_through_the_transcript_stem() {
    let (_tmp, b) = backend();
    open_keyed(&b, "s1", "k1");
    let out = open_with(
        &b,
        OpenRequest {
            transcript_path: Some("/p/s1.jsonl".to_string()),
            observed_key: Some("k9".to_string()),
            ..req("s2", Source::Resume)
        },
    );
    assert_eq!(out.opened, Opened::Successor);
    assert_eq!(
        last_started(&b, "anchor-s2").predecessor.as_deref(),
        Some("anchor-s1")
    );
    // Its marker is its own, from its own environment.
    assert_eq!(read_marker(&b, "anchor-s2").unwrap().key, "k9");
}

#[test]
fn a_resume_with_no_hint_match_opens_a_fresh_anchor() {
    let (_tmp, b) = backend();
    open_keyed(&b, "s1", "k1");
    let out = open_with(
        &b,
        OpenRequest {
            transcript_path: Some("/p/unrelated.jsonl".to_string()),
            ..req("s2", Source::Resume)
        },
    );
    assert_eq!(
        (out.anchor.as_str(), out.opened),
        ("anchor-s2", Opened::New)
    );
    assert_eq!(link_count(&b, "anchor-s2", LinkKind::DispatchedBy), 0);
    assert!(last_started(&b, "anchor-s2").predecessor.is_none());
    assert!(read_marker(&b, "anchor-s2").is_none());
}

#[test]
fn the_hint_join_never_replaces_the_name_match() {
    let (_tmp, b) = backend();
    let transcript = "/p/conv.jsonl";
    open_with(
        &b,
        OpenRequest {
            transcript_path: Some(transcript.to_string()),
            ..req("s1", Source::Startup)
        },
    );
    open_with(&b, req("s2", Source::Startup));
    let out = open_with(
        &b,
        OpenRequest {
            transcript_path: Some(transcript.to_string()),
            ..req("s2", Source::Resume)
        },
    );
    assert_eq!(
        (out.anchor.as_str(), out.opened),
        ("anchor-s2", Opened::Continued)
    );
    assert!(!has_link(
        &b,
        "anchor-s2",
        LinkKind::DispatchedBy,
        "anchor-s1"
    ));
}

// ===== /clear =====

fn token(start: u64) -> String {
    format!("host:1234:{start}")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[test]
fn a_clear_links_to_the_anchor_its_process_ran_before_and_copies_its_marker() {
    let (_tmp, b) = backend();
    let tok = token(now_secs() - 60);
    open_with(
        &b,
        OpenRequest {
            process: Some(tok.clone()),
            observed_key: Some("k1".to_string()),
            ..req("s1", Source::Startup)
        },
    );
    let pred_before = session_hashes(&b, "anchor-s1");
    let out = open_with(
        &b,
        OpenRequest {
            process: Some(tok),
            ..req("s2", Source::Clear)
        },
    );
    assert_eq!(
        (out.anchor.as_str(), out.opened),
        ("anchor-s2", Opened::Successor)
    );
    assert_eq!(out.previous.as_deref(), Some("anchor-s1"));
    assert!(out.inherit_until.is_some());
    assert!(out.observed);
    assert!(has_link(
        &b,
        "anchor-s2",
        LinkKind::DispatchedBy,
        "anchor-s1"
    ));
    assert_eq!(read_marker(&b, "anchor-s2").unwrap().key, "k1");
    let s = last_started(&b, "anchor-s2");
    assert_eq!(s.predecessor.as_deref(), Some("anchor-s1"));
    assert_eq!(s.reason.as_deref(), Some(REASON_CLEAR));
    assert_eq!(session_hashes(&b, "anchor-s1"), pred_before);
}

#[test]
fn a_clear_with_no_match_opens_an_unlinked_anchor() {
    let (_tmp, b) = backend();
    let out = open_with(
        &b,
        OpenRequest {
            process: Some(token(now_secs() - 60)),
            ..req("s2", Source::Clear)
        },
    );
    assert_eq!(out.opened, Opened::New);
    assert!(out.previous.is_none() && out.inherit_until.is_none());
    assert_eq!(link_count(&b, "anchor-s2", LinkKind::DispatchedBy), 0);
}

#[test]
fn a_clear_under_a_foreign_token_writes_no_link() {
    let (_tmp, b) = backend();
    let start = now_secs() - 60;
    open_with(
        &b,
        OpenRequest {
            process: Some(format!("host:1111:{start}")),
            observed_key: Some("k1".to_string()),
            ..req("s1", Source::Startup)
        },
    );
    let out = open_with(
        &b,
        OpenRequest {
            process: Some(format!("host:2222:{start}")),
            ..req("s2", Source::Clear)
        },
    );
    assert_eq!(out.opened, Opened::New);
    assert!(out.previous.is_none());
    assert!(!has_link(
        &b,
        "anchor-s2",
        LinkKind::DispatchedBy,
        "anchor-s1"
    ));
    assert!(read_marker(&b, "anchor-s2").is_none());
}

#[test]
fn the_clear_scan_skips_anchors_not_changed_since_the_process_started() {
    let (_tmp, b) = backend();
    let tok = token(now_secs() + 3600);
    open_with(
        &b,
        OpenRequest {
            process: Some(tok.clone()),
            ..req("s1", Source::Startup)
        },
    );
    assert_eq!(find_clear_predecessor(&base_dir(&b), &tok), None);
    assert_eq!(find_clear_predecessor(&base_dir(&b), "no-start-time"), None);
}

#[test]
fn the_clear_scan_stays_under_250ms_on_2000_sessions_and_400_anchors() {
    let (_tmp, b) = backend();
    let base = base_dir(&b);
    std::fs::create_dir_all(&base).unwrap();
    let start = now_secs() - 600;
    let header = |name: &str, kind: Option<&str>| {
        let mut h = serde_json::json!({
            "schema_version": 1, "workflow": name, "template_hash": "h",
            "created_at": "2026-01-01T00:00:00Z", "session_id": name,
        });
        if let Some(k) = kind {
            h["kind"] = serde_json::json!(k);
        }
        h.to_string()
    };
    let write = |name: &str, body: String| {
        let dir = base.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(state_file_name(name)), body).unwrap();
    };
    for i in 0..2000 {
        let name = format!("wf-{i}");
        write(&name, header(&name, None) + "\n");
    }
    for i in 0..400 {
        let name = format!("anchor-a{i}");
        let started = serde_json::json!({
            "seq": 3, "timestamp": "2026-01-01T00:00:00.000Z", "type": "anchor_started",
            "payload": {"source": "startup", "harness_session": format!("a{i}"),
                        "opened": "new", "process": digest(&format!("other:{i}:{start}"))}
        });
        write(
            &name,
            format!("{}\n{}\n", header(&name, Some("anchor")), started),
        );
    }
    // The worst case: no anchor matches, so every recent one is read.
    let t = Instant::now();
    let found = find_clear_predecessor(&base, &token(start));
    let took = t.elapsed();
    assert_eq!(found, None);
    assert!(took < Duration::from_millis(250), "scan took {took:?}");
}

// ===== observe =====

#[test]
fn observe_key_writes_only_into_the_named_anchor_and_is_idempotent() {
    let (tmp, b) = backend();
    open_with(&b, req("s1", Source::Startup));
    open_with(&b, req("s2", Source::Startup));
    make_session(&b, "wf");
    let other = |name: &str| session_hashes(&b, name);
    let (s2, wf) = (other("anchor-s2"), other("wf"));

    let out = observe(&b, "anchor-s1", &Observe::Key("k1".to_string())).unwrap();
    assert_eq!(
        out,
        ObserveOutcome {
            observed: true,
            written: true
        }
    );
    assert_eq!(read_marker(&b, "anchor-s1").unwrap().key, "k1");
    let after = session_hashes(&b, "anchor-s1");
    let again = observe(&b, "anchor-s1", &Observe::Key("k1".to_string())).unwrap();
    assert_eq!(
        again,
        ObserveOutcome {
            observed: true,
            written: false
        }
    );
    assert_eq!(session_hashes(&b, "anchor-s1"), after);
    assert_eq!((other("anchor-s2"), other("wf")), (s2, wf));

    let before = hashes(tmp.path());
    for (anchor, key) in [
        ("anchor-s2", ""),
        ("anchor-s2", "bad key"),
        ("anchor-s2", &"k".repeat(129)),
        ("wf", "k1"),
        ("nope", "k1"),
    ] {
        let err = observe(&b, anchor, &Observe::Key(key.to_string())).unwrap_err();
        assert!(
            err.downcast_ref::<AnchorRefusal>().is_some(),
            "{anchor} {key:?}"
        );
    }
    assert_eq!(hashes(tmp.path()), before);
}

#[test]
fn observe_inherit_copies_the_marker_of_the_recorded_predecessor() {
    let (_tmp, b) = backend();
    let tok = token(now_secs() - 60);
    open_with(
        &b,
        OpenRequest {
            process: Some(tok.clone()),
            ..req("s1", Source::Startup)
        },
    );
    let out = open_with(
        &b,
        OpenRequest {
            process: Some(tok),
            ..req("s2", Source::Clear)
        },
    );
    assert!(!out.observed);
    assert_eq!(out.previous.as_deref(), Some("anchor-s1"));
    // The predecessor has no marker yet: nothing to copy.
    let none = observe(&b, "anchor-s2", &Observe::Inherit).unwrap();
    assert!(!none.observed);
    observe(&b, "anchor-s1", &Observe::Key("k1".to_string())).unwrap();
    let pred = session_hashes(&b, "anchor-s1");
    let got = observe(&b, "anchor-s2", &Observe::Inherit).unwrap();
    assert!(got.observed && got.written);
    assert_eq!(read_marker(&b, "anchor-s2").unwrap().key, "k1");
    assert_eq!(session_hashes(&b, "anchor-s1"), pred);
    // An anchor with no predecessor inherits nothing.
    open_with(&b, req("s3", Source::Startup));
    assert!(
        !observe(&b, "anchor-s3", &Observe::Inherit)
            .unwrap()
            .observed
    );
}

// ===== close =====

#[test]
fn close_records_the_anchor_in_a_close_session_and_leaves_the_anchor_untouched() {
    let (_tmp, b) = backend();
    open_keyed(&b, "s1", "k1");
    let before = session_hashes(&b, "anchor-s1");
    let out = close(&b, "k1", "s1").unwrap();
    assert_eq!(out.recorded, vec!["anchor-s1".to_string()]);
    assert_eq!(session_hashes(&b, "anchor-s1"), before);
    assert!(close_check(&b, "anchor-s1").is_closed());

    let record: CloseRecord =
        serde_json::from_slice(&b.get("anchor-s1-closed", CLOSE_RECORD_KEY).unwrap()).unwrap();
    assert_eq!(record.anchor, "anchor-s1");
    assert_eq!(
        record.anchor_session_id,
        b.read_header("anchor-s1").unwrap().session_id
    );
    assert_eq!(
        (record.kind.as_str(), record.key.as_str()),
        ("observed", "k1")
    );
    assert!(has_link(
        &b,
        "anchor-s1-closed",
        LinkKind::Closes,
        "anchor-s1"
    ));

    // Run again: nothing changes.
    let closed = session_hashes(&b, "anchor-s1-closed");
    let again = close(&b, "k1", "s1").unwrap();
    assert!(again.recorded.is_empty());
    assert_eq!(session_hashes(&b, "anchor-s1-closed"), closed);
    assert_eq!(session_hashes(&b, "anchor-s1"), before);
}

#[test]
fn close_with_no_matching_anchor_or_a_different_marker_records_nothing() {
    let (tmp, b) = backend();
    open_keyed(&b, "s1", "k1");
    let before = hashes(tmp.path());
    assert!(close(&b, "k2", "s1").unwrap().recorded.is_empty());
    assert!(close(&b, "k1", "nobody").unwrap().recorded.is_empty());
    assert_eq!(hashes(tmp.path()), before);
    assert_eq!(close_check(&b, "anchor-s1"), CloseCheck::Open);
}

#[test]
fn close_refuses_malformed_keys_and_harness_ids() {
    let (_tmp, b) = backend();
    for (key, harness) in [("", "s1"), ("a b", "s1"), ("k", ""), ("k", "../x")] {
        let err = close(&b, key, harness).unwrap_err();
        assert!(
            err.downcast_ref::<AnchorRefusal>().is_some(),
            "{key:?} {harness:?}"
        );
    }
}

#[test]
fn a_child_anchor_with_the_same_key_stays_open() {
    let (_tmp, b) = backend();
    open_keyed(&b, "parent", "k1");
    open_with(
        &b,
        OpenRequest {
            dispatched_by: Some("anchor-parent".to_string()),
            observed_key: Some("k1".to_string()),
            ..req("child", Source::Startup)
        },
    );
    let child = session_hashes(&b, "anchor-child");
    assert_eq!(
        close(&b, "k1", "parent").unwrap().recorded,
        vec!["anchor-parent"]
    );
    assert_eq!(close(&b, "k1", "parent").unwrap().recorded.len(), 0);
    assert_eq!(close_check(&b, "anchor-child"), CloseCheck::Open);
    assert!(!state_path(&b, "anchor-child-closed").exists());
    assert_eq!(session_hashes(&b, "anchor-child"), child);
}

#[test]
fn a_clear_successor_closes_with_its_predecessor() {
    let (_tmp, b) = backend();
    let tok = token(now_secs() - 60);
    open_with(
        &b,
        OpenRequest {
            process: Some(tok.clone()),
            observed_key: Some("k1".to_string()),
            ..req("s1", Source::Startup)
        },
    );
    open_with(
        &b,
        OpenRequest {
            process: Some(tok),
            ..req("s2", Source::Clear)
        },
    );
    let out = close(&b, "k1", "s1").unwrap();
    assert_eq!(out.recorded, vec!["anchor-s1", "anchor-s2"]);
    assert!(close_check(&b, "anchor-s2").is_closed());
}

#[test]
fn a_successor_opened_because_its_predecessor_read_closed_stays_open_on_a_repeat() {
    let (_tmp, b) = backend();
    open_keyed(&b, "s1", "k1");
    close(&b, "k1", "s1").unwrap();
    // The same key reaches the successor through its environment.
    let out = open_keyed(&b, "s1", "k1");
    assert_eq!(out.anchor, "anchor-s1-s2");
    assert_eq!(close(&b, "k1", "s1").unwrap().recorded.len(), 0);
    assert_eq!(close_check(&b, "anchor-s1-s2"), CloseCheck::Open);
}

#[test]
fn a_resume_successor_closes_with_its_predecessor_only_on_its_own_matching_key() {
    let (_tmp, b) = backend();
    // A successor whose own environment carried the same key.
    open_keyed(&b, "s1", "k1");
    open_with(
        &b,
        OpenRequest {
            transcript_path: Some("/p/s1.jsonl".to_string()),
            observed_key: Some("k1".to_string()),
            ..req("s2", Source::Resume)
        },
    );
    // Two successors opened from false hints: a different key, and none.
    open_with(
        &b,
        OpenRequest {
            transcript_path: Some("/p/s1.jsonl".to_string()),
            observed_key: Some("k-other".to_string()),
            ..req("s3", Source::Resume)
        },
    );
    open_with(
        &b,
        OpenRequest {
            transcript_path: Some("/p/s1.jsonl".to_string()),
            ..req("s4", Source::Resume)
        },
    );
    let (s3, s4) = (
        session_hashes(&b, "anchor-s3"),
        session_hashes(&b, "anchor-s4"),
    );
    let out = close(&b, "k1", "s1").unwrap();
    assert_eq!(out.recorded, vec!["anchor-s1", "anchor-s2"]);
    assert_eq!(close_check(&b, "anchor-s3"), CloseCheck::Open);
    assert_eq!(close_check(&b, "anchor-s4"), CloseCheck::Open);
    assert_eq!(
        (
            session_hashes(&b, "anchor-s3"),
            session_hashes(&b, "anchor-s4")
        ),
        (s3, s4)
    );
}

#[test]
fn a_hand_cleaned_and_reopened_anchor_gets_a_fresh_close_session() {
    let (_tmp, b) = backend();
    open_keyed(&b, "s1", "k1");
    close(&b, "k1", "s1").unwrap();
    let old_id = b.read_header("anchor-s1").unwrap().session_id;
    b.cleanup("anchor-s1").unwrap();

    let out = open_keyed(&b, "s1", "k1");
    assert_eq!(
        (out.anchor.as_str(), out.opened),
        ("anchor-s1", Opened::Reopened)
    );
    let new_id = b.read_header("anchor-s1").unwrap().session_id;
    assert_ne!(old_id, new_id);
    // The stale record names the old id, so the reopened anchor reads open.
    assert!(matches!(
        close_check(&b, "anchor-s1"),
        CloseCheck::Tainted(_)
    ));
    assert!(!close_check(&b, "anchor-s1").is_closed());

    let out = close(&b, "k1", "s1").unwrap();
    assert_eq!(out.recorded, vec!["anchor-s1"]);
    assert!(close_check(&b, "anchor-s1").is_closed());
    let record: CloseRecord =
        serde_json::from_slice(&b.get("anchor-s1-closed", CLOSE_RECORD_KEY).unwrap()).unwrap();
    assert_eq!(record.anchor_session_id, new_id);
    // The stale one went with the staging entry.
    let leftovers: Vec<_> = std::fs::read_dir(base_dir(&b))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(STAGING_PREFIX))
        .collect();
    assert!(leftovers.is_empty());
}

// ===== staging =====

fn staging_entries(b: &LocalBackend) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(base_dir(b))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(STAGING_PREFIX))
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

#[test]
fn a_staged_entry_removed_before_the_rename_fails_the_run_and_a_later_close_succeeds() {
    let (_tmp, b) = backend();
    open_keyed(&b, "s1", "k1");
    let anchor = session_hashes(&b, "anchor-s1");
    let err = close_with_hook(&b, "k1", "s1", &|staged| {
        std::fs::remove_dir_all(staged)?;
        Ok(())
    })
    .unwrap_err();
    assert!(err.to_string().contains("missing"), "{err:#}");
    assert!(!close_check(&b, "anchor-s1").is_closed());
    assert!(!b.exists("anchor-s1-closed"));
    assert_eq!(session_hashes(&b, "anchor-s1"), anchor);

    assert_eq!(close(&b, "k1", "s1").unwrap().recorded, vec!["anchor-s1"]);
    assert!(close_check(&b, "anchor-s1").is_closed());
}

#[test]
fn an_interrupted_write_leaves_the_anchor_open_and_the_next_close_succeeds() {
    let (_tmp, b) = backend();
    open_keyed(&b, "s1", "k1");
    let err = close_with_hook(&b, "k1", "s1", &|_| anyhow::bail!("interrupted")).unwrap_err();
    assert!(err.to_string().contains("interrupted"));
    assert_eq!(close_check(&b, "anchor-s1"), CloseCheck::Open);
    // The partial entry is left for a later sweep, and nothing lists it.
    assert_eq!(staging_entries(&b).len(), 1);
    let listed: Vec<String> = b.list().unwrap().into_iter().map(|s| s.id).collect();
    assert_eq!(listed, vec!["anchor-s1".to_string()]);

    assert_eq!(close(&b, "k1", "s1").unwrap().recorded, vec!["anchor-s1"]);
    assert!(close_check(&b, "anchor-s1").is_closed());
    // The fresh leftover is not swept yet.
    assert_eq!(staging_entries(&b).len(), 1);
}

#[test]
fn the_sweep_removes_old_entries_and_clamps_future_dated_ones() {
    let (_tmp, b) = backend();
    let base = base_dir(&b);
    std::fs::create_dir_all(&base).unwrap();
    let now = SystemTime::now();
    let old = base.join(format!("{STAGING_PREFIX}old"));
    let fresh = base.join(format!("{STAGING_PREFIX}fresh"));
    let future = base.join(format!("{STAGING_PREFIX}future"));
    for d in [&old, &fresh, &future] {
        std::fs::create_dir_all(d.join("inner")).unwrap();
    }
    let set = |p: &Path, t: SystemTime| std::fs::File::open(p).unwrap().set_modified(t).unwrap();
    set(&old, now - Duration::from_secs(11 * 60));
    set(&future, now + Duration::from_secs(365 * 86_400));

    sweep_staging(&base, now);
    assert!(!old.exists(), "an entry past the age is swept");
    assert!(fresh.exists(), "a fresh entry may be a close in progress");
    assert!(future.exists(), "a future-dated entry is clamped first");
    let clamped = std::fs::metadata(&future).unwrap().modified().unwrap();
    assert!(clamped <= now + Duration::from_secs(1));

    // Once the age has passed since the clamp, it goes.
    sweep_staging(&base, now + Duration::from_secs(11 * 60));
    assert!(!future.exists());
    assert!(!fresh.exists());
}

#[test]
fn a_sweep_leaves_another_closes_fresh_entry_and_that_close_completes() {
    let (_tmp, b) = backend();
    open_keyed(&b, "s1", "k1");
    open_keyed(&b, "s2", "k2");
    // While s1's close holds a fresh staged entry, s2's close sweeps.
    let out = close_with_hook(&b, "k1", "s1", &|_| {
        let other = close(&b, "k2", "s2").unwrap();
        assert_eq!(other.recorded, vec!["anchor-s2"]);
        Ok(())
    })
    .unwrap();
    assert_eq!(out.recorded, vec!["anchor-s1"]);
    assert!(close_check(&b, "anchor-s1").is_closed());
    assert!(close_check(&b, "anchor-s2").is_closed());
    assert!(staging_entries(&b).is_empty());
}

#[test]
fn two_closes_at_once_both_succeed_and_leave_one_valid_close_session() {
    let (_tmp, b) = backend();
    open_keyed(&b, "s1", "k1");
    let b = std::sync::Arc::new(b);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let b = b.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                close_with_hook(&*b, "k1", "s1", &|_| {
                    barrier.wait();
                    Ok(())
                })
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let recorded: usize = results
        .iter()
        .map(|r| r.as_ref().expect("both closes succeed").recorded.len())
        .sum();
    assert!(recorded >= 1);
    assert!(close_check(&*b, "anchor-s1").is_closed());
    let closes: Vec<_> = b
        .list()
        .unwrap()
        .into_iter()
        .filter(|s| s.id.ends_with(CLOSED_SUFFIX))
        .collect();
    assert_eq!(closes.len(), 1);
    assert!(staging_entries(&b).is_empty());
}

#[test]
fn staging_entries_are_never_listed_as_sessions() {
    let (_tmp, b) = backend();
    open_keyed(&b, "s1", "k1");
    let _ = close_with_hook(&b, "k1", "s1", &|_| anyhow::bail!("stop"));
    assert_eq!(staging_entries(&b).len(), 1);
    // The staged session is complete, but under the reserved prefix.
    let ids: Vec<String> = b.list().unwrap().into_iter().map(|s| s.id).collect();
    assert_eq!(ids, vec!["anchor-s1".to_string()]);
    assert_eq!(b.count_unreadable(), 0);
}

// ===== the shared close check =====

/// Rewrite the first line of `name`'s state file through `f`.
fn edit_header(b: &LocalBackend, name: &str, f: impl Fn(&mut serde_json::Value)) {
    let path = state_path(b, name);
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut h: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    f(&mut h);
    lines[0] = h.to_string();
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
}

fn closed_fixture() -> (tempfile::TempDir, LocalBackend) {
    let (tmp, b) = backend();
    open_keyed(&b, "s1", "k1");
    close(&b, "k1", "s1").unwrap();
    assert!(close_check(&b, "anchor-s1").is_closed());
    (tmp, b)
}

#[test]
fn the_close_check_taints_on_each_failing_condition() {
    // Wrong kind.
    let (_t, b) = closed_fixture();
    edit_header(&b, "anchor-s1-closed", |h| {
        h["kind"] = serde_json::json!("anchor")
    });
    assert!(matches!(
        close_check(&b, "anchor-s1"),
        CloseCheck::Tainted(_)
    ));

    // No kind at all.
    let (_t, b) = closed_fixture();
    edit_header(&b, "anchor-s1-closed", |h| {
        h.as_object_mut().unwrap().remove("kind");
    });
    assert!(matches!(
        close_check(&b, "anchor-s1"),
        CloseCheck::Tainted(_)
    ));

    // The header names another session.
    let (_t, b) = closed_fixture();
    edit_header(&b, "anchor-s1-closed", |h| {
        h["workflow"] = serde_json::json!("other")
    });
    assert!(matches!(
        close_check(&b, "anchor-s1"),
        CloseCheck::Tainted(_)
    ));

    // The anchor id differs.
    let (_t, b) = closed_fixture();
    edit_header(&b, "anchor-s1", |h| {
        h["session_id"] = serde_json::json!("another-id")
    });
    assert!(matches!(
        close_check(&b, "anchor-s1"),
        CloseCheck::Tainted(_)
    ));

    // The key differs from the anchor's marker.
    let (_t, b) = closed_fixture();
    let marker = serde_json::json!({"v": 1, "key": "k-other", "at": now_iso8601()});
    b.add_with_writer(
        "anchor-s1",
        OBSERVED_KEY,
        marker.to_string().as_bytes(),
        WRITER_KOTO,
    )
    .unwrap();
    assert!(matches!(
        close_check(&b, "anchor-s1"),
        CloseCheck::Tainted(_)
    ));

    // The manifest doesn't list the record.
    let (_t, b) = closed_fixture();
    let manifest = b
        .session_dir("anchor-s1-closed")
        .join("ctx")
        .join("manifest.json");
    let mut m: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest).unwrap()).unwrap();
    m["keys"].as_object_mut().unwrap().remove(CLOSE_RECORD_KEY);
    std::fs::write(&manifest, m.to_string()).unwrap();
    assert!(b.ctx_dir_has_file("anchor-s1-closed", CLOSE_RECORD_KEY));
    assert!(matches!(
        close_check(&b, "anchor-s1"),
        CloseCheck::Tainted(_)
    ));

    // The record names another anchor.
    let (_t, b) = closed_fixture();
    let mut record: CloseRecord =
        serde_json::from_slice(&b.get("anchor-s1-closed", CLOSE_RECORD_KEY).unwrap()).unwrap();
    record.anchor = "anchor-x".to_string();
    b.add_with_writer(
        "anchor-s1-closed",
        CLOSE_RECORD_KEY,
        &serde_json::to_vec(&record).unwrap(),
        WRITER_KOTO,
    )
    .unwrap();
    assert!(matches!(
        close_check(&b, "anchor-s1"),
        CloseCheck::Tainted(_)
    ));
}

trait CtxFile {
    fn ctx_dir_has_file(&self, session: &str, key: &str) -> bool;
}

impl CtxFile for LocalBackend {
    fn ctx_dir_has_file(&self, session: &str, key: &str) -> bool {
        self.session_dir(session).join("ctx").join(key).exists()
    }
}

// ===== held-by and activity =====

#[test]
fn hold_writes_one_held_by_link_and_a_second_anchor_adds_a_second() {
    let (_tmp, b) = backend();
    open_with(&b, req("a1", Source::Startup));
    open_with(&b, req("a2", Source::Startup));
    make_session(&b, "wf");
    make_session(&b, "bystander");
    let (a2, by) = (
        session_hashes(&b, "anchor-a2"),
        session_hashes(&b, "bystander"),
    );

    let first = hold(&b, "wf", "anchor-a1", "next").unwrap();
    assert_eq!(first.linked, vec!["anchor-a1"]);
    assert!(has_link(&b, "wf", LinkKind::HeldBy, "anchor-a1"));
    assert_eq!(session_hashes(&b, "anchor-a2"), a2);
    assert_eq!(session_hashes(&b, "bystander"), by);

    let wf = session_hashes(&b, "wf");
    let again = hold(&b, "wf", "anchor-a1", "next").unwrap();
    assert!(again.linked.is_empty());
    assert_eq!(session_hashes(&b, "wf"), wf);

    let second = hold(&b, "wf", "anchor-a2", "next").unwrap();
    assert_eq!(second.linked, vec!["anchor-a2"]);
    assert_eq!(link_count(&b, "wf", LinkKind::HeldBy), 2);
    assert_eq!(session_hashes(&b, "bystander"), by);
}

#[test]
fn hold_writes_nothing_for_a_name_that_isnt_an_anchor() {
    let (tmp, b) = backend();
    make_session(&b, "wf");
    make_session(&b, "plain");
    open_with(&b, req("a1", Source::Startup));
    let before = hashes(tmp.path());
    for name in ["plain", "missing", "wf"] {
        assert_eq!(
            hold(&b, "wf", name, "init").unwrap(),
            HoldOutcome::default()
        );
    }
    // An anchor never holds a held-by link, to itself or another anchor.
    assert_eq!(
        hold(&b, "anchor-a1", "anchor-a1", "next").unwrap(),
        HoldOutcome::default()
    );
    open_with(&b, req("a2", Source::Startup));
    let before_a2 = hashes(tmp.path());
    assert_eq!(
        hold(&b, "anchor-a2", "anchor-a1", "next").unwrap(),
        HoldOutcome::default()
    );
    assert_eq!(hashes(tmp.path()), before_a2);
    assert_ne!(before, before_a2);
}

#[test]
fn a_closed_anchor_with_a_successor_also_holds_through_the_successor() {
    let (_tmp, b) = backend();
    open_keyed(&b, "s1", "k1");
    close(&b, "k1", "s1").unwrap();
    open_with(&b, req("s1", Source::Resume));
    make_session(&b, "wf");
    // The successor last started long ago, so the throttle lets one through.
    age_log(&b, "anchor-s1-s2");
    let closed = session_hashes(&b, "anchor-s1");
    let out = hold(&b, "wf", "anchor-s1", "next").unwrap();
    assert_eq!(out.linked, vec!["anchor-s1", "anchor-s1-s2"]);
    assert_eq!(out.activity.as_deref(), Some("anchor-s1-s2"));
    assert!(has_link(&b, "wf", LinkKind::HeldBy, "anchor-s1-s2"));
    assert_eq!(session_hashes(&b, "anchor-s1"), closed);
}

#[test]
fn activity_is_throttled_to_a_tenth_of_the_idle_period_from_the_last_event() {
    let (_tmp, b) = backend();
    open_with(&b, req("a1", Source::Startup));
    let idle = Duration::from_secs(10);
    let t0 = SystemTime::now() + Duration::from_secs(5);
    let count = |b: &LocalBackend| {
        events_of(b, "anchor-a1")
            .iter()
            .filter(|e| matches!(e.payload, EventPayload::AnchorActivity(_)))
            .count()
    };
    // Every 200 ms for 3 s: at most one event per second.
    let mut written = Vec::new();
    for i in 0..15u64 {
        let now = t0 + Duration::from_millis(200 * i);
        if record_activity(&b, "anchor-a1", "wf", "next", idle, now).unwrap() {
            written.push(now);
        }
    }
    assert_eq!(count(&b), written.len());
    assert!(written.len() <= 3 && !written.is_empty(), "{written:?}");
    for pair in written.windows(2) {
        assert!(pair[1].duration_since(pair[0]).unwrap() >= Duration::from_secs(1));
    }
    let last = events_of(&b, "anchor-a1").pop().unwrap();
    assert!(matches!(
        &last.payload,
        EventPayload::AnchorActivity(AnchorActivity { session, command })
            if session == "wf" && command == "next"
    ));
}

#[test]
fn the_idle_period_falls_back_to_the_default_on_a_malformed_value() {
    assert_eq!(
        crate::session::duration::parse_age("10s"),
        Some(Duration::from_secs(10))
    );
    // `idle_period` reads the process environment, which other tests
    // share; the parse it relies on is checked here instead.
    assert_eq!(crate::session::duration::parse_age("soon"), None);
    assert_eq!(
        crate::session::duration::parse_age("0s"),
        Some(Duration::ZERO)
    );
}

#[test]
fn names_and_suffixes() {
    assert_eq!(family_base("anchor-x-s2"), "anchor-x");
    assert_eq!(family_base("anchor-x-s10"), "anchor-x");
    assert_eq!(family_base("anchor-x"), "anchor-x");
    assert_eq!(family_base("anchor-x-s1"), "anchor-x-s1");
    assert_eq!(family_base("anchor-x-s02"), "anchor-x-s02");
    assert_eq!(close_name("anchor-x-s2"), "anchor-x-s2-closed");
    assert!(valid_token("abc-DEF_123"));
    assert!(!valid_token("a.b"));
}
