//! Tests of the bucket walk, over a bucket held in memory: what it reads,
//! what it keeps, and what it costs.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::*;
use crate::session::hierarchy::relation::Reason;

const WS1: &str = "0123456789abcdef";
const WS2: &str = "fedcba9876543210";

/// A bucket in memory: objects by key, listings that fail by prefix, and a
/// count of every request made.
#[derive(Default)]
struct MemBucket {
    objects: RefCell<BTreeMap<String, Vec<u8>>>,
    fail_list: HashSet<String>,
    cached: BTreeMap<String, Vec<u8>>,
    requests: Cell<usize>,
    /// What a migration marker beside the judged session names.
    marker: Option<Result<Option<Pointer>, UncheckedError>>,
}

impl MemBucket {
    fn put(&self, key: &str, bytes: &[u8]) {
        self.objects
            .borrow_mut()
            .insert(key.to_string(), bytes.to_vec());
    }
}

impl BucketReads for MemBucket {
    fn list(&self, prefix: &str) -> Result<Listed, UncheckedError> {
        self.requests.set(self.requests.get() + 1);
        if self.fail_list.contains(prefix) {
            return Err(UncheckedError::Unreachable);
        }
        Ok(Listed {
            objects: self
                .objects
                .borrow()
                .iter()
                .filter(|(k, _)| k.starts_with(prefix))
                .map(|(k, v)| ListedObject {
                    key: k.clone(),
                    size: v.len() as u64,
                    etag: None,
                })
                .collect(),
            requests: 1,
        })
    }

    fn get(&self, key: &str, max: usize) -> Result<Option<Vec<u8>>, UncheckedError> {
        self.requests.set(self.requests.get() + 1);
        Ok(self
            .objects
            .borrow()
            .get(key)
            .map(|v| v[..v.len().min(max + 1)].to_vec()))
    }

    fn cached_template(&self, hash: &str) -> Option<Vec<u8>> {
        self.cached.get(hash).cloned()
    }

    fn marker_target(
        &self,
        _prefix: &str,
        _name: &str,
    ) -> (Result<Option<Pointer>, UncheckedError>, usize) {
        (self.marker.clone().unwrap_or(Ok(None)), 1)
    }
}

fn id(n: u8) -> String {
    format!("{n:08x}-0000-4000-8000-000000000000")
}

fn at(secs: u64) -> String {
    crate::engine::types::iso8601_at(UNIX_EPOCH + Duration::from_secs(secs))
}

const NOW: u64 = 1_800_000_000;
const DAY: u64 = 86_400;

fn clock() -> Clock {
    Clock {
        keep: Duration::from_secs(DAY),
        now: UNIX_EPOCH + Duration::from_secs(NOW),
    }
}

fn template() -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "format_version": 1,
        "name": "t",
        "version": "1",
        "initial_state": "work",
        "states": {
            "work": {"directive": "w"},
            "done": {"directive": "d", "terminal": true},
        },
    }))
    .unwrap()
}

fn hash() -> String {
    crate::cache::sha256_hex(&template())
}

/// A state log for session `name` (id `sid`): initialized at `NOW - 10d`,
/// then moved to `to` at `stop`.
fn state(name: &str, sid: &str, to: &str, stop: u64) -> Vec<u8> {
    let header = serde_json::json!({
        "schema_version": 1,
        "workflow": name,
        "template_hash": hash(),
        "created_at": at(NOW - 10 * DAY),
        "session_id": sid,
    });
    let events = [
        serde_json::json!({
            "seq": 1, "timestamp": at(NOW - 10 * DAY), "type": "workflow_initialized",
            "payload": {"template_path": "/t.json", "variables": {}},
        }),
        serde_json::json!({
            "seq": 2, "timestamp": at(stop), "type": "transitioned",
            "payload": {"from": "work", "to": to, "condition_type": "auto"},
        }),
    ];
    let mut out = serde_json::to_string(&header).unwrap();
    for e in events {
        out.push('\n');
        out.push_str(&serde_json::to_string(&e).unwrap());
    }
    out.push('\n');
    out.into_bytes()
}

fn relation(pred: &str, pred_id: &str, holder_id: &str, pred_prefix: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "v": 1,
        "session": pred,
        "session_id": pred_id,
        "workspace": "/work",
        "prefix": pred_prefix,
        "holder_session_id": holder_id,
        "at": at(NOW - 5 * DAY),
    }))
    .unwrap()
}

/// Session `name` under `ws`, id `sid`, standing in `to` since `stop`,
/// continuing `pred` (under `pred_ws`, id `pred_id`) when given, with its
/// pointer beside the predecessor.
fn session(
    b: &MemBucket,
    ws: &str,
    name: &str,
    sid: &str,
    to: &str,
    stop: u64,
    pred: Option<(&str, &str, &str)>,
) {
    b.put(
        &format!("{ws}/{name}/{}", crate::session::state_file_name(name)),
        &state(name, sid, to, stop),
    );
    if let Some((pws, pred, pid)) = pred {
        b.put(
            &format!("{ws}/{name}/ctx/koto/links/continues/{pred}"),
            &relation(pred, pid, sid, pws),
        );
        b.put(&pointer_key(pws, pred, ws, name), &pointer_body(sid));
    }
}

fn facts_of(
    b: &MemBucket,
    ws: &str,
    name: &str,
    sid: Option<&str>,
    shared: bool,
) -> BucketRemovalFacts {
    let session = format!("{ws}/{name}/");
    let listed = b.list(&format!("{session}ctx/koto/")).unwrap();
    // A shared listing is the pin guard's: the walk doesn't pay for it.
    if shared {
        b.requests.set(b.requests.get() - 1);
    }
    BucketRemovalFacts {
        prefix: ws.to_string(),
        name: name.to_string(),
        session_id: sid.map(str::to_string),
        ctx: Ok(CtxListing::parse(&session, &listed.objects)),
        migrated: Ok(false),
        state_etag: None,
        origin: None,
        requests: if shared { 0 } else { 1 } + 1,
    }
}

fn run(b: &MemBucket, facts: &BucketRemovalFacts) -> RelationVerdict {
    let mut memo = Memo::default();
    walk(
        b,
        &mut memo,
        &Start {
            facts,
            skip: None,
            extra: &[],
        },
        &clock(),
    )
    .verdict
}

/// v1, v2 and v3 under WS1: v1 and v2 finished long ago, v3 standing in
/// `v3_to` since `v3_stop`.
fn chain(b: &MemBucket, v3_to: &str, v3_stop: u64) {
    session(b, WS1, "v1", &id(1), "done", NOW - 9 * DAY, None);
    session(
        b,
        WS1,
        "v2",
        &id(2),
        "done",
        NOW - 8 * DAY,
        Some((WS1, "v1", &id(1))),
    );
    session(
        b,
        WS1,
        "v3",
        &id(3),
        v3_to,
        v3_stop,
        Some((WS1, "v2", &id(2))),
    );
}

#[test]
fn a_pointer_key_must_have_exactly_its_parts() {
    let p = format!("{WS1}/v1/");
    let key = |rest: &str| format!("{p}ctx/koto/continued-by/{rest}");
    assert_eq!(
        is_backref_key(&p, &key(&format!("{WS2}/v2"))),
        Some(Pointer {
            prefix: WS2.to_string(),
            name: "v2".to_string()
        })
    );
    for bad in [
        format!("{}/v2", &WS2[..15]),
        format!("{}/v2", WS2.to_uppercase()),
        format!("{WS2}/v2/extra"),
        format!("{WS2}/"),
        format!("{WS2}/2bad"),
        format!("{WS2}/v\u{1b}[31m2"),
        "v2".to_string(),
        format!("../{WS2}/v2"),
    ] {
        assert_eq!(is_backref_key(&p, &key(&bad)), None, "{bad:?}");
    }
    // Under another session's prefix, or outside the area.
    assert_eq!(
        is_backref_key(&format!("{WS1}/v9/"), &key(&format!("{WS2}/v2"))),
        None
    );
    assert_eq!(
        is_backref_key(&p, &format!("{p}ctx/koto/links/continues/v0")),
        None
    );
    assert!(in_backref_area(&p, &key("anything/at/all")));
    assert!(!in_backref_area(&p, &format!("{p}ctx/koto/links/pin")));

    // A key a session may take back: whole, valid, and naming it.
    let own = pointer_key(WS1, "v1", WS2, "v2");
    assert!(is_own_pointer_key(&own, "v2"));
    assert!(!is_own_pointer_key(&own, "v3"));
    assert!(!is_own_pointer_key(&format!("{own}/x"), "v2"));
    assert!(!is_own_pointer_key(
        &pointer_key("nothex", "v1", WS2, "v2"),
        "v2"
    ));
}

#[test]
fn nothing_names_the_session_and_the_walk_reads_nothing_more() {
    let b = MemBucket::default();
    session(&b, WS1, "v1", &id(1), "done", NOW - 9 * DAY, None);
    let facts = facts_of(&b, WS1, "v1", Some(&id(1)), false);
    let before = b.requests.get();
    let v = run(&b, &facts);
    assert!(!v.keeps(), "{v:?}");
    assert_eq!(b.requests.get(), before);
}

#[test]
fn a_running_member_two_steps_on_keeps_and_is_named_with_its_path() {
    let b = MemBucket {
        cached: BTreeMap::from([(hash(), template())]),
        ..Default::default()
    };
    chain(&b, "work", NOW - DAY);
    let facts = facts_of(&b, WS1, "v1", Some(&id(1)), false);
    let start = b.requests.get() - 1;
    let v = run(&b, &facts);
    assert_eq!(v.keepers.len(), 1, "{v:?}");
    let k = &v.keepers[0];
    assert_eq!(k.keeper, "v3");
    assert_eq!(k.standing, Standing::Alive);
    assert_eq!(k.path, vec!["v2".to_string()]);
    assert_eq!(k.pin_source, crate::session::pin::SOURCE_BUCKET);
    assert!(v.unreadable.is_empty() && v.unchecked.is_empty(), "{v:?}");
    // 2 for v1, 3 for each member reached.
    assert!(
        b.requests.get() - start <= 2 + 3 * 2,
        "{}",
        b.requests.get() - start
    );
}

#[test]
fn an_uncached_template_fits_when_the_pin_listing_is_shared() {
    let b = MemBucket::default();
    chain(&b, "work", NOW - DAY);
    b.put(
        &format!("{WS1}/v2/{}", crate::session::cloud::TEMPLATE_OBJECT),
        &template(),
    );
    b.put(
        &format!("{WS1}/v3/{}", crate::session::cloud::TEMPLATE_OBJECT),
        &template(),
    );
    let facts = facts_of(&b, WS1, "v1", Some(&id(1)), true);
    let start = b.requests.get();
    let v = run(&b, &facts);
    assert_eq!(
        v.keepers
            .iter()
            .map(|k| k.keeper.as_str())
            .collect::<Vec<_>>(),
        ["v3"]
    );
    assert!(v.unchecked.is_empty(), "{v:?}");
    assert!(
        b.requests.get() - start <= 8,
        "{}",
        b.requests.get() - start
    );
}

#[test]
fn a_pin_on_the_newest_member_keeps_and_is_read_from_its_object() {
    let b = MemBucket {
        cached: BTreeMap::from([(hash(), template())]),
        ..Default::default()
    };
    chain(&b, "done", NOW - 7 * DAY);
    let pin = serde_json::json!({
        "v": 1, "id": "p1", "reason": "keep it", "set_by": "hand", "set_at": at(NOW - DAY),
    });
    b.put(
        &format!("{WS1}/v3/ctx/koto/links/pin"),
        &serde_json::to_vec(&pin).unwrap(),
    );
    let v = run(&b, &facts_of(&b, WS1, "v1", Some(&id(1)), false));
    assert_eq!(v.keepers.len(), 1, "{v:?}");
    assert_eq!(v.keepers[0].standing, Standing::Pin);
    assert!(v.keepers[0].pin.is_pinned());
}

#[test]
fn retention_keeps_until_it_passes_and_names_when() {
    let b = MemBucket {
        cached: BTreeMap::from([(hash(), template())]),
        ..Default::default()
    };
    chain(&b, "done", NOW - DAY / 2);
    let v = run(&b, &facts_of(&b, WS1, "v1", Some(&id(1)), false));
    assert_eq!(v.keepers.len(), 1, "{v:?}");
    assert_eq!(v.keepers[0].standing, Standing::Retention);
    assert_eq!(
        v.keepers[0].retention_passes_at.as_deref(),
        Some(at(NOW - DAY / 2 + DAY).as_str())
    );

    let b = MemBucket {
        cached: BTreeMap::from([(hash(), template())]),
        ..Default::default()
    };
    chain(&b, "done", NOW - 2 * DAY);
    let v = run(&b, &facts_of(&b, WS1, "v1", Some(&id(1)), false));
    assert!(!v.keeps(), "{v:?}");
}

#[test]
fn a_stale_pointer_is_ignored() {
    let b = MemBucket::default();
    session(&b, WS1, "v1", &id(1), "done", NOW - 9 * DAY, None);
    // A pointer whose holder holds no relation.
    session(&b, WS2, "s", &id(2), "work", NOW, None);
    b.put(&pointer_key(WS1, "v1", WS2, "s"), &pointer_body(&id(2)));
    let v = run(&b, &facts_of(&b, WS1, "v1", Some(&id(1)), false));
    assert!(!v.keeps(), "{v:?}");
}

#[test]
fn a_reused_name_reads_as_a_mismatch_and_keeps() {
    let b = MemBucket {
        cached: BTreeMap::from([(hash(), template())]),
        ..Default::default()
    };
    chain(&b, "work", NOW);
    // v1 was removed and a new session took the name: its id is new.
    let v = run(&b, &facts_of(&b, WS1, "v1", Some(&id(9)), false));
    assert!(v.keepers.is_empty(), "{v:?}");
    assert_eq!(
        v.unreadable,
        vec![Unreadable {
            holder: "v2".to_string(),
            reason: Reason::Mismatch
        }]
    );
}

#[test]
fn a_moved_predecessor_is_found_by_its_session_id() {
    let b = MemBucket {
        cached: BTreeMap::from([(hash(), template())]),
        ..Default::default()
    };
    session(&b, WS1, "v1_r1", &id(1), "done", NOW - 9 * DAY, None);
    session(
        &b,
        WS1,
        "v2",
        &id(2),
        "work",
        NOW,
        Some((WS1, "v1", &id(1))),
    );
    // The pointer was carried to the moved name.
    b.put(&pointer_key(WS1, "v1_r1", WS1, "v2"), &pointer_body(&id(2)));
    let v = run(&b, &facts_of(&b, WS1, "v1_r1", Some(&id(1)), false));
    assert_eq!(
        v.keepers
            .iter()
            .map(|k| k.keeper.as_str())
            .collect::<Vec<_>>(),
        ["v2"]
    );
}

#[test]
fn a_relation_that_does_not_check_out_keeps_as_unreadable() {
    for (body, reason) in [
        (b"{not json".to_vec(), Reason::Unparseable),
        (
            serde_json::to_vec(&serde_json::json!({
                "v": 1, "session": "v1", "session_id": id(1), "workspace": "relative/path",
                "prefix": WS1, "holder_session_id": id(2), "at": at(NOW),
            }))
            .unwrap(),
            Reason::Unparseable,
        ),
        (
            serde_json::to_vec(&serde_json::json!({
                "v": 1, "session": "v1", "session_id": id(1), "workspace": "/w",
                "prefix": "NOT-HEX", "holder_session_id": id(2), "at": at(NOW),
            }))
            .unwrap(),
            Reason::Unparseable,
        ),
        (vec![b' '; MAX_LINK_BODY_BYTES + 1], Reason::TooLarge),
    ] {
        let b = MemBucket::default();
        chain(&b, "work", NOW);
        b.put(&format!("{WS1}/v2/ctx/koto/links/continues/v1"), &body);
        let v = run(&b, &facts_of(&b, WS1, "v1", Some(&id(1)), false));
        assert_eq!(
            v.unreadable,
            vec![Unreadable {
                holder: "v2".to_string(),
                reason
            }],
            "{v:?}"
        );
        assert!(v.keeps());
    }
}

#[test]
fn a_member_whose_listing_fails_keeps_as_unchecked_naming_it() {
    let mut b = MemBucket::default();
    b.fail_list.insert(format!("{WS1}/v3/ctx/koto/"));
    chain(&b, "work", NOW);
    let v = run(&b, &facts_of(&b, WS1, "v1", Some(&id(1)), false));
    assert_eq!(
        v.unchecked,
        vec![Unchecked {
            name: "v3".to_string(),
            error: UncheckedError::Unreachable
        }]
    );
    let json = crate::session::hierarchy::relation_json(&v);
    assert_eq!(json["kept_by_relation"], true);
    assert_eq!(json["bucket_unchecked"]["sessions"][0]["name"], "v3");
}

#[test]
fn a_pointer_flood_is_not_walked_and_keeps() {
    let b = MemBucket::default();
    session(&b, WS1, "v1", &id(1), "done", NOW - 9 * DAY, None);
    for n in 0..=MAX_POINTERS {
        b.put(&pointer_key(WS1, "v1", WS2, &format!("s{n}")), b"{}");
    }
    let facts = facts_of(&b, WS1, "v1", Some(&id(1)), false);
    let before = b.requests.get();
    let v = run(&b, &facts);
    assert_eq!(b.requests.get(), before, "a flood costs no request");
    assert_eq!(
        v.unchecked,
        vec![Unchecked {
            name: "v1".to_string(),
            error: UncheckedError::TooManyPointers
        }]
    );
}

#[test]
fn stale_pointers_draw_from_the_allowance_until_it_runs_out() {
    let b = MemBucket::default();
    session(&b, WS1, "v1", &id(1), "done", NOW - 9 * DAY, None);
    for n in 0..3 {
        b.put(&pointer_key(WS1, "v1", WS2, &format!("s{n}")), b"{}");
    }
    let v = run(&b, &facts_of(&b, WS1, "v1", Some(&id(1)), true));
    assert!(v.keepers.is_empty());
    assert!(
        v.unchecked
            .iter()
            .any(|u| u.error == UncheckedError::Budget),
        "{v:?}"
    );
}

#[test]
fn an_invalid_pointer_keeps_and_its_name_is_shown_safely() {
    let b = MemBucket::default();
    session(&b, WS1, "v1", &id(1), "done", NOW - 9 * DAY, None);
    b.put(
        &format!("{WS1}/v1/ctx/koto/continued-by/{WS2}/evil\u{1b}[2J"),
        b"{}",
    );
    b.put(
        &format!("{WS1}/v1/ctx/koto/continued-by/{WS2}/ok/extra"),
        b"{}",
    );
    let v = run(&b, &facts_of(&b, WS1, "v1", Some(&id(1)), false));
    assert_eq!(v.unreadable.len(), 2, "{v:?}");
    assert!(v.unreadable.iter().all(|u| u.reason == Reason::Unparseable));
    for line in crate::session::hierarchy::relation_text(&v) {
        assert!(!line.contains('\u{1b}'), "{line:?}");
    }
    let json = serde_json::to_string(&crate::session::hierarchy::relation_json(&v)).unwrap();
    assert!(!json.contains("\\u001b"), "{json}");
}

#[test]
fn a_cycle_keeps() {
    let b = MemBucket {
        cached: BTreeMap::from([(hash(), template())]),
        ..Default::default()
    };
    chain(&b, "done", NOW - 9 * DAY);
    // v1 also continues v3, by hand.
    b.put(&pointer_key(WS1, "v3", WS1, "v1"), &pointer_body(&id(1)));
    b.put(
        &format!("{WS1}/v1/ctx/koto/links/continues/v3"),
        &relation("v3", &id(3), &id(1), WS1),
    );
    let v = run(&b, &facts_of(&b, WS1, "v1", Some(&id(1)), false));
    assert!(
        v.unreadable.iter().any(|u| u.reason == Reason::Cycle),
        "{v:?}"
    );
}

#[test]
fn the_final_guard_walks_only_pointers_the_judgment_did_not_see() {
    let b = MemBucket {
        cached: BTreeMap::from([(hash(), template())]),
        ..Default::default()
    };
    session(&b, WS1, "v1", &id(1), "done", NOW - 9 * DAY, None);
    session(
        &b,
        WS1,
        "v2",
        &id(2),
        "done",
        NOW - 8 * DAY,
        Some((WS1, "v1", &id(1))),
    );
    let mut memo = Memo::default();
    let facts = facts_of(&b, WS1, "v1", Some(&id(1)), false);
    let judged = walk(
        &b,
        &mut memo,
        &Start {
            facts: &facts,
            skip: None,
            extra: &[],
        },
        &clock(),
    );
    assert!(!judged.verdict.keeps());
    let seen = memo.judged(WS1, "v1").cloned().unwrap();
    assert_eq!(seen.len(), 1);

    // A successor written since, running.
    session(
        &b,
        WS2,
        "late",
        &id(4),
        "work",
        NOW,
        Some((WS1, "v1", &id(1))),
    );
    let facts = facts_of(&b, WS1, "v1", Some(&id(1)), true);
    let before = b.requests.get();
    let last = walk(
        &b,
        &mut memo,
        &Start {
            facts: &facts,
            skip: Some(&seen),
            extra: &[],
        },
        &clock(),
    );
    assert_eq!(
        last.verdict
            .keepers
            .iter()
            .map(|k| k.keeper.as_str())
            .collect::<Vec<_>>(),
        ["late"]
    );
    // Only the new holder was read: its listing, relation and state.
    assert_eq!(b.requests.get() - before, 3);
}

#[test]
fn a_member_reached_again_in_the_pass_costs_nothing() {
    let b = MemBucket {
        cached: BTreeMap::from([(hash(), template())]),
        ..Default::default()
    };
    chain(&b, "work", NOW);
    let mut memo = Memo::default();
    for name in ["v2", "v1"] {
        let sid = if name == "v1" { id(1) } else { id(2) };
        let facts = facts_of(&b, WS1, name, Some(&sid), true);
        let before = b.requests.get();
        let v = walk(
            &b,
            &mut memo,
            &Start {
                facts: &facts,
                skip: None,
                extra: &[],
            },
            &clock(),
        );
        assert!(v.verdict.keeps());
        if name == "v1" {
            // v2 and v3 were read judging v2 (v3) and are in the memo.
            assert!(
                b.requests.get() - before <= 3,
                "{}",
                b.requests.get() - before
            );
        }
    }
    memo.begin_pass();
    assert!(memo.judged(WS1, "v1").is_none());
}

#[test]
fn a_later_member_in_another_workspace_counts() {
    let b = MemBucket {
        cached: BTreeMap::from([(hash(), template())]),
        ..Default::default()
    };
    session(&b, WS1, "v1", &id(1), "done", NOW - 9 * DAY, None);
    session(
        &b,
        WS2,
        "v2",
        &id(2),
        "work",
        NOW,
        Some((WS1, "v1", &id(1))),
    );
    let v = run(&b, &facts_of(&b, WS1, "v1", Some(&id(1)), false));
    assert_eq!(
        v.keepers
            .iter()
            .map(|k| k.keeper.as_str())
            .collect::<Vec<_>>(),
        ["v2"]
    );
}

#[test]
fn a_template_that_does_not_match_its_hash_keeps_the_member_alive() {
    let b = MemBucket::default();
    session(&b, WS1, "v1", &id(1), "done", NOW - 9 * DAY, None);
    session(
        &b,
        WS1,
        "v2",
        &id(2),
        "done",
        NOW - 9 * DAY,
        Some((WS1, "v1", &id(1))),
    );
    b.put(
        &format!("{WS1}/v2/{}", crate::session::cloud::TEMPLATE_OBJECT),
        b"{\"tampered\": true}",
    );
    let v = run(&b, &facts_of(&b, WS1, "v1", Some(&id(1)), true));
    assert_eq!(v.keepers.len(), 1, "{v:?}");
    assert_eq!(v.keepers[0].standing, Standing::Alive);
}

#[test]
fn a_member_imported_into_a_copy_keeps_through_the_copy_alone() {
    let b = MemBucket {
        cached: BTreeMap::from([(hash(), template())]),
        ..Default::default()
    };
    session(&b, WS1, "v1", &id(1), "done", NOW - 9 * DAY, None);
    // v2 runs in its log, but was imported into a copy after its last event.
    session(
        &b,
        WS1,
        "v2",
        &id(2),
        "work",
        NOW - 8 * DAY,
        Some((WS1, "v1", &id(1))),
    );
    session(
        &b,
        WS2,
        "v2",
        &id(3),
        "done",
        NOW - 9 * DAY,
        Some((WS1, "v2", &id(2))),
    );
    b.put(&format!("{WS2}/v2/ctx/koto/imported-from"), b"{}");
    let v = run(&b, &facts_of(&b, WS1, "v1", Some(&id(1)), false));
    // The copy finished long ago, and the source counts as migrated.
    assert!(v.keepers.is_empty(), "{v:?}");
}

#[test]
fn owed_pointer_follows_the_mark_and_the_name() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("v2");
    std::fs::create_dir_all(dir.join("ctx/koto/links/continues")).unwrap();
    assert_eq!(owed_pointer(&dir, WS1, "v2"), None);
    std::fs::write(
        dir.join("ctx/koto/links/continues/v1"),
        relation("v1", &id(1), &id(2), WS2),
    )
    .unwrap();
    let owed = owed_pointer(&dir, WS1, "v2").expect("a pointer is owed");
    assert_eq!(owed.key, pointer_key(WS2, "v1", WS1, "v2"));
    assert_eq!(owed.holder_session_id, id(2));
    assert_eq!(
        unpushable_keys(&dir, WS1, "v2"),
        ["koto/links/continues/v1"]
    );
    mark_confirmed(&dir, &owed.key).unwrap();
    assert_eq!(owed_pointer(&dir, WS1, "v2"), None);
    assert!(unpushable_keys(&dir, WS1, "v2").is_empty());
    // Renamed, the confirmed pointer names the old name: one is owed again.
    assert!(owed_pointer(&dir, WS1, "v2_r1").is_some());
    // A relation recorded under the local backend has no prefix: its
    // workspace's prefix is used.
    std::fs::write(
        dir.join("ctx/koto/links/continues/v1"),
        serde_json::to_vec(&serde_json::json!({
            "v": 1, "session": "v1", "session_id": id(1), "workspace": "/work",
            "prefix": null, "holder_session_id": id(2), "at": at(NOW),
        }))
        .unwrap(),
    )
    .unwrap();
    let local = owed_pointer(&dir, WS1, "v2").unwrap();
    let ws = crate::session::local::prefix_for_workspace(std::path::Path::new("/work")).unwrap();
    assert_eq!(local.pred_prefix, ws);
}

#[test]
fn standing_from_bytes_follows_the_inventory_rules() {
    let terminal: BTreeSet<String> = BTreeSet::from(["done".to_string()]);
    let alive = |s: &StandingFacts| matches!(s, StandingFacts::Alive { .. });
    assert!(alive(&standing_from_bytes(b"not a log", |_| Some(
        terminal.clone()
    ))));
    let running = state("m", &id(1), "work", NOW);
    assert!(alive(&standing_from_bytes(&running, |_| Some(
        terminal.clone()
    ))));
    let done = state("m", &id(1), "done", NOW - DAY);
    assert!(matches!(
        standing_from_bytes(&done, |_| Some(terminal.clone())),
        StandingFacts::Stopped { .. }
    ));
    // No template: it can't be judged, so it keeps.
    assert!(alive(&standing_from_bytes(&done, |_| None)));
    let _ = SystemTime::now();
}

#[test]
fn the_final_guard_reads_again_a_pointer_the_judgment_found_stale() {
    let b = MemBucket {
        cached: BTreeMap::from([(hash(), template())]),
        ..Default::default()
    };
    session(&b, WS1, "v1", &id(1), "done", NOW - 9 * DAY, None);
    // A pointer written before its relation: stale at the judgment.
    session(&b, WS2, "s", &id(2), "work", NOW, None);
    b.put(&pointer_key(WS1, "v1", WS2, "s"), &pointer_body(&id(2)));
    let mut memo = Memo::default();
    let facts = facts_of(&b, WS1, "v1", Some(&id(1)), false);
    let judged = walk(
        &b,
        &mut memo,
        &Start {
            facts: &facts,
            skip: None,
            extra: &[],
        },
        &clock(),
    );
    assert!(!judged.verdict.keeps(), "{:?}", judged.verdict);
    let seen = memo.judged(WS1, "v1").cloned().unwrap();
    assert!(seen.is_empty(), "a stale pointer isn't counted as judged");

    // Its relation lands before the delete: the final guard finds it.
    b.put(
        &format!("{WS2}/s/ctx/koto/links/continues/v1"),
        &relation("v1", &id(1), &id(2), WS1),
    );
    let facts = facts_of(&b, WS1, "v1", Some(&id(1)), true);
    let last = walk(
        &b,
        &mut memo,
        &Start {
            facts: &facts,
            skip: Some(&seen),
            extra: &[],
        },
        &clock(),
    );
    assert_eq!(
        last.verdict
            .keepers
            .iter()
            .map(|k| k.keeper.as_str())
            .collect::<Vec<_>>(),
        ["s"]
    );
}

#[test]
fn a_migration_marker_beside_the_session_counts_its_target_or_keeps() {
    let facts_with_marker = |b: &MemBucket| BucketRemovalFacts {
        migrated: Ok(true),
        ..facts_of(b, WS1, "v1", Some(&id(1)), false)
    };
    // A verified target that runs keeps the source, named, with no
    // relation needed.
    let b = MemBucket {
        cached: BTreeMap::from([(hash(), template())]),
        marker: Some(Ok(Some(Pointer {
            prefix: WS2.to_string(),
            name: "copy".to_string(),
        }))),
        ..Default::default()
    };
    session(&b, WS1, "v1", &id(1), "done", NOW - 9 * DAY, None);
    session(&b, WS2, "copy", &id(3), "work", NOW, None);
    let v = run(&b, &facts_with_marker(&b));
    assert_eq!(
        v.keepers
            .iter()
            .map(|k| k.keeper.as_str())
            .collect::<Vec<_>>(),
        ["copy"]
    );
    // The same target finished long ago: nothing keeps.
    let b2 = MemBucket {
        cached: BTreeMap::from([(hash(), template())]),
        marker: b.marker.clone(),
        ..Default::default()
    };
    session(&b2, WS1, "v1", &id(1), "done", NOW - 9 * DAY, None);
    session(&b2, WS2, "copy", &id(3), "done", NOW - 9 * DAY, None);
    assert!(!run(&b2, &facts_with_marker(&b2)).keeps());
    // A marker whose target can't be read or verified keeps, unchecked.
    let b3 = MemBucket {
        marker: Some(Err(UncheckedError::Unparseable)),
        ..Default::default()
    };
    session(&b3, WS1, "v1", &id(1), "done", NOW - 9 * DAY, None);
    let v = run(&b3, &facts_with_marker(&b3));
    assert_eq!(
        v.unchecked,
        vec![Unchecked {
            name: "v1".to_string(),
            error: UncheckedError::Unparseable
        }]
    );
    // A target passed in by the caller (`prune --migrated-from`) counts the
    // same way.
    let mut memo = Memo::default();
    let facts = facts_of(&b, WS1, "v1", Some(&id(1)), false);
    let extra = [Pointer {
        prefix: WS2.to_string(),
        name: "copy".to_string(),
    }];
    let v = walk(
        &b,
        &mut memo,
        &Start {
            facts: &facts,
            skip: None,
            extra: &extra,
        },
        &clock(),
    )
    .verdict;
    assert_eq!(v.keepers.len(), 1, "{v:?}");
}
