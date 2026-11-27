//! The assignments a session's log says the context store should hold
//! ([`outstanding_assignments`]): a pure fold over the events, which the
//! context read uses to answer a value the store has not caught up with.
//! koto's `engine::context_assign` writes them.

use std::collections::BTreeMap;

use crate::engine::types::{Event, EventPayload};

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
            EventPayload::ContextAdded { key, .. } | EventPayload::ContextRemoved { key, .. } => {
                latest.insert(key.clone(), None);
            }
            // A `clear_on_entry` clearing removed these keys; it supersedes
            // an earlier assignment exactly as a `koto context remove` does.
            EventPayload::ContextCleared { keys, .. } => {
                for key in keys {
                    latest.insert(key.clone(), None);
                }
            }
            _ => {}
        }
    }
    latest
        .into_iter()
        .filter_map(|(key, value)| value.map(|v| (key, v)))
        .collect()
}
