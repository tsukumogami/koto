//! Writing transition `context_assignments` to the context store (koto#204).
//!
//! The `Transitioned` event is the durable record of an assignment: the
//! resolved values ride the same append as the transition, so a crash cannot
//! leave a transition without its assignments or the reverse. The store is a
//! projection of that record. It is written right after the append
//! ([`write_assignments`]), and if that write fails -- or the process dies
//! between the two -- the next read repairs it from the log ([`reconcile`]).
//!
//! For each key the latest event that touched it decides what the store should
//! hold. An assignment is superseded by a later `koto context add` or
//! `koto context remove` of the same key, and supersedes an earlier one, which
//! is the same last-write-wins rule `koto context add` already follows.

use std::collections::BTreeMap;

use crate::engine::types::{Event, EventPayload};
use crate::session::context::ContextStore;

/// Write resolved assignments to the store. Stops at the first failure; the
/// caller reports it, and [`reconcile`] repairs what did not land.
pub fn write_assignments(
    store: &dyn ContextStore,
    session: &str,
    assignments: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    for (key, value) in assignments {
        store.add(session, key, value.as_bytes())?;
    }
    Ok(())
}

/// The assignments the store should reflect: every key whose latest writer in
/// `events` is a `Transitioned` event's assignment, mapped to that value.
pub fn outstanding_assignments(events: &[Event]) -> BTreeMap<String, String> {
    let mut latest: BTreeMap<String, Option<String>> = BTreeMap::new();
    for event in events {
        match &event.payload {
            EventPayload::Transitioned {
                context_assignments: Some(assignments),
                ..
            } => {
                for (key, value) in assignments {
                    latest.insert(key.clone(), Some(value.clone()));
                }
            }
            EventPayload::ContextAdded { key, .. } | EventPayload::ContextRemoved { key } => {
                latest.insert(key.clone(), None);
            }
            _ => {}
        }
    }
    latest
        .into_iter()
        .filter_map(|(key, value)| value.map(|v| (key, v)))
        .collect()
}

/// Bring the store in line with the assignments recorded in `events`,
/// rewriting any key whose stored content differs from its latest assigned
/// value. When `only` is given, just that key is checked.
///
/// Cheap when nothing is outstanding: a log with no assignments costs one pass
/// over the events and no store access.
pub fn reconcile(
    store: &dyn ContextStore,
    session: &str,
    events: &[Event],
    only: Option<&str>,
) -> anyhow::Result<()> {
    for (key, value) in outstanding_assignments(events) {
        if only.is_some_and(|k| k != key) {
            continue;
        }
        let current = store.get(session, &key).ok();
        if current.as_deref() != Some(value.as_bytes()) {
            store.add(session, &key, value.as_bytes())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// In-memory store whose writes can be made to fail.
    #[derive(Default)]
    struct FlakyStore {
        data: Mutex<HashMap<String, Vec<u8>>>,
        fail_writes: Mutex<bool>,
    }

    impl ContextStore for FlakyStore {
        fn add(&self, _session: &str, key: &str, content: &[u8]) -> anyhow::Result<()> {
            if *self.fail_writes.lock().unwrap() {
                anyhow::bail!("store unavailable");
            }
            self.data
                .lock()
                .unwrap()
                .insert(key.to_string(), content.to_vec());
            Ok(())
        }
        fn get(&self, _session: &str, key: &str) -> anyhow::Result<Vec<u8>> {
            self.data
                .lock()
                .unwrap()
                .get(key)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("absent"))
        }
        fn ctx_exists(&self, _session: &str, key: &str) -> bool {
            self.data.lock().unwrap().contains_key(key)
        }
        fn remove(&self, _session: &str, key: &str) -> anyhow::Result<()> {
            self.data.lock().unwrap().remove(key);
            Ok(())
        }
        fn list_keys(&self, _session: &str, _prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
            Ok(self.data.lock().unwrap().keys().cloned().collect())
        }
    }

    fn event(seq: u64, payload: EventPayload) -> Event {
        Event {
            seq,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: payload.type_name().to_string(),
            payload,
            idempotency_hash: None,
        }
    }

    fn transitioned(seq: u64, pairs: &[(&str, &str)]) -> Event {
        event(
            seq,
            EventPayload::Transitioned {
                from: Some("a".to_string()),
                to: "b".to_string(),
                condition_type: "auto".to_string(),
                skip_if_matched: None,
                context_assignments: Some(
                    pairs
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                ),
            },
        )
    }

    #[test]
    fn failed_write_is_repaired_by_the_next_read() {
        let store = FlakyStore::default();
        let events = vec![transitioned(1, &[("outcome", "landed")])];
        let assigned = outstanding_assignments(&events);

        *store.fail_writes.lock().unwrap() = true;
        assert!(write_assignments(&store, "s", &assigned).is_err());
        assert!(!store.ctx_exists("s", "outcome"));

        *store.fail_writes.lock().unwrap() = false;
        reconcile(&store, "s", &events, Some("outcome")).unwrap();
        assert_eq!(store.get("s", "outcome").unwrap(), b"landed");
    }

    #[test]
    fn later_assignment_replaces_earlier() {
        let events = vec![
            transitioned(1, &[("step", "one")]),
            transitioned(2, &[("step", "two")]),
        ];
        assert_eq!(
            outstanding_assignments(&events)
                .get("step")
                .map(String::as_str),
            Some("two")
        );
    }

    #[test]
    fn context_add_or_remove_supersedes_an_assignment() {
        let events = vec![
            transitioned(1, &[("a", "x"), ("b", "y")]),
            event(
                2,
                EventPayload::ContextAdded {
                    key: "a".to_string(),
                    hash: String::new(),
                    size: 0,
                },
            ),
            event(
                3,
                EventPayload::ContextRemoved {
                    key: "b".to_string(),
                },
            ),
        ];
        assert!(outstanding_assignments(&events).is_empty());

        // And an assignment after an add wins again.
        let mut events = events;
        events.push(transitioned(4, &[("a", "z")]));
        assert_eq!(
            outstanding_assignments(&events)
                .get("a")
                .map(String::as_str),
            Some("z")
        );
    }

    #[test]
    fn reconcile_leaves_matching_content_alone() {
        let store = FlakyStore::default();
        store.add("s", "k", b"v").unwrap();
        *store.fail_writes.lock().unwrap() = true;
        // Content already matches, so no write is attempted (it would fail).
        reconcile(&store, "s", &[transitioned(1, &[("k", "v")])], None).unwrap();
    }
}
