use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use super::*;
use crate::session::continues_bucket::{pointer_key, Listed, ListedObject};
use crate::session::hierarchy::UncheckedError;

const P1: &str = "aaaaaaaaaaaaaaaa";
const P2: &str = "bbbbbbbbbbbbbbbb";
const ID1: &str = "11111111-1111-4111-8111-111111111111";
const ID2: &str = "22222222-2222-4222-8222-222222222222";
const ID3: &str = "33333333-3333-4333-8333-333333333333";

/// An in-memory bucket that records every request.
#[derive(Default)]
struct Fake {
    objects: BTreeMap<String, Vec<u8>>,
    failing: BTreeSet<String>,
    marker: Option<Result<Option<Pointer>, UncheckedError>>,
    requests: RefCell<Vec<String>>,
}

impl Fake {
    fn put(&mut self, key: &str, bytes: &[u8]) {
        self.objects.insert(key.to_string(), bytes.to_vec());
    }

    /// Session `name` under `prefix`, anchored at `dir`, with id `id`.
    fn session(&mut self, prefix: &str, name: &str, id: &str, dir: &str) {
        let header = serde_json::json!({
            "workflow": name,
            "session_id": id,
            "execution_dir": dir,
        });
        self.put(
            &format!("{prefix}/{name}/{}", state_file_name(name)),
            format!("{header}\n{{\"seq\":1}}\n").as_bytes(),
        );
    }

    /// Session `holder` continues `pred`, recording `pred_id`.
    fn continues(&mut self, (hp, holder): (&str, &str), (pp, pred): (&str, &str), pred_id: &str) {
        let body = serde_json::json!({
            "v": 1,
            "session": pred,
            "session_id": pred_id,
            "prefix": pp,
            "at": "2026-10-27T00:00:00.000Z",
        });
        self.put(
            &format!("{hp}/{holder}/ctx/{}{pred}", continues_prefix()),
            body.to_string().as_bytes(),
        );
        self.put(&pointer_key(pp, pred, hp, holder), b"{}");
    }
}

impl BucketReads for Fake {
    fn list(&self, prefix: &str) -> Result<Listed, UncheckedError> {
        self.requests.borrow_mut().push(format!("LIST {prefix}"));
        if self.failing.contains(prefix) {
            return Err(UncheckedError::Unreachable);
        }
        Ok(Listed {
            objects: self
                .objects
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
        self.requests.borrow_mut().push(format!("GET {key}"));
        if self.failing.contains(key) {
            return Err(UncheckedError::Unreachable);
        }
        Ok(self
            .objects
            .get(key)
            .map(|v| v[..v.len().min(max + 1)].to_vec()))
    }

    fn cached_template(&self, _hash: &str) -> Option<Vec<u8>> {
        None
    }

    fn marker_target(
        &self,
        _prefix: &str,
        _name: &str,
    ) -> (Result<Option<Pointer>, UncheckedError>, usize) {
        (self.marker.clone().unwrap_or(Ok(None)), 1)
    }
}

fn walk_v1(fake: &Fake) -> Found {
    walk(fake, P1, "v1", Some(ID1), Path::new("/a/repo"))
}

#[test]
fn a_successor_only_the_bucket_knows_releases_when_anchored_outside() {
    let mut fake = Fake::default();
    fake.session(P1, "v1", ID1, "/a/repo");
    fake.session(P2, "v2", ID2, "/b/elsewhere");
    fake.continues((P2, "v2"), (P1, "v1"), ID1);
    assert_eq!(walk_v1(&fake), Found::Released);
    let requests = fake.requests.borrow();
    assert!(requests.iter().all(|r| r.starts_with("LIST ") || r.starts_with("GET ")));
}

#[test]
fn a_successor_anchored_under_the_path_keeps_it_and_its_own_successor_is_followed() {
    let mut fake = Fake::default();
    fake.session(P1, "v1", ID1, "/a/repo");
    fake.session(P2, "v2", ID2, "/a/repo/sub");
    fake.continues((P2, "v2"), (P1, "v1"), ID1);
    assert_eq!(walk_v1(&fake), Found::Kept);

    fake.session(P2, "v3", ID3, "/b/elsewhere");
    fake.continues((P2, "v3"), (P2, "v2"), ID2);
    assert_eq!(walk_v1(&fake), Found::Released);
}

#[test]
fn a_relation_naming_another_session_id_is_not_a_successor() {
    let mut fake = Fake::default();
    fake.session(P1, "v1", ID1, "/a/repo");
    fake.session(P2, "v2", ID2, "/b/elsewhere");
    fake.continues((P2, "v2"), (P1, "v1"), ID3);
    assert_eq!(walk_v1(&fake), Found::Kept);
}

#[test]
fn a_successor_that_cannot_be_read_leaves_the_path_unchecked() {
    let mut fake = Fake::default();
    fake.session(P1, "v1", ID1, "/a/repo");
    fake.session(P2, "v2", ID2, "/b/elsewhere");
    fake.continues((P2, "v2"), (P1, "v1"), ID1);
    fake.failing
        .insert(format!("{P2}/v2/{}", state_file_name("v2")));
    assert_eq!(walk_v1(&fake), Found::Unchecked);
}

#[test]
fn a_prefix_that_cannot_be_listed_leaves_the_path_unchecked() {
    let mut fake = Fake::default();
    fake.session(P1, "v1", ID1, "/a/repo");
    fake.failing.insert(format!("{P1}/v1/"));
    assert_eq!(walk_v1(&fake), Found::Unchecked);
}

#[test]
fn an_imported_copy_a_verified_marker_names_releases_when_anchored_outside() {
    let mut fake = Fake::default();
    fake.session(P1, "v1", ID1, "/a/repo");
    fake.put(&format!("{P1}/v1/{MIGRATED_MARKER}"), b"{}");
    fake.session(P2, "v1", ID2, "/b/imported");
    fake.marker = Some(Ok(Some(Pointer {
        prefix: P2.to_string(),
        name: "v1".to_string(),
    })));
    assert_eq!(walk_v1(&fake), Found::Released);

    fake.marker = Some(Err(UncheckedError::Unreachable));
    assert_eq!(walk_v1(&fake), Found::Unchecked);
}

#[test]
fn a_flood_of_back_references_is_not_walked() {
    let mut fake = Fake::default();
    fake.session(P1, "v1", ID1, "/a/repo");
    for i in 0..=MAX_POINTERS {
        fake.put(&pointer_key(P1, "v1", P2, &format!("s{i}")), b"{}");
    }
    assert_eq!(walk_v1(&fake), Found::Unchecked);
    assert_eq!(fake.requests.borrow().len(), 1);
}

#[test]
fn a_cycle_ends_the_walk() {
    let mut fake = Fake::default();
    fake.session(P1, "v1", ID1, "/a/repo");
    fake.session(P2, "v2", ID2, "/a/repo/sub");
    fake.continues((P2, "v2"), (P1, "v1"), ID1);
    fake.continues((P1, "v1"), (P2, "v2"), ID2);
    assert_eq!(walk_v1(&fake), Found::Kept);
}
