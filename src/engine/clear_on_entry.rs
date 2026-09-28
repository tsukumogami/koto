//! Clearing a state's declared context keys when the workflow enters it
//! (DESIGN-koto-ci-wait-stale-keys.md, Decisions 1-3).
//!
//! Whether an entry still needs clearing is decided from the event log
//! alone, never by reading the context store: every logged read appends an
//! event and, on the cloud backend, uploads it. [`pending_clearing`] answers
//! it for the state the workflow now occupies, and the advance loop and the
//! `koto next --to` and `koto rewind` paths ask it right after an entry.
//! [`apply`] removes the keys and then appends one `context_cleared` event,
//! in that order, so an interrupted clearing leaves no event and the next
//! call finishes it.

use crate::engine::persistence::any_entry_index;
use crate::engine::types::now_iso8601;
use crate::engine::types::{Event, EventPayload};
use crate::session::context::ContextStore;
use crate::session::context_log::WRITER_SYNC;
use crate::session::SessionBackend;

/// One entry's clearing: the keys still to remove and the entry it belongs
/// to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clearing {
    pub state: String,
    pub keys: Vec<String>,
    pub entry_seq: u64,
}

impl Clearing {
    /// The event that records this clearing.
    pub fn event(&self) -> EventPayload {
        EventPayload::ContextCleared {
            state: self.state.clone(),
            keys: self.keys.clone(),
            entry_seq: self.entry_seq,
        }
    }
}

/// The clearing `state`'s latest entry still needs, or `None`.
///
/// `state` must be the state the workflow occupies, as for
/// [`any_entry_index`]. The entry is the latest `transitioned`,
/// `directed_transition` or `rewound` event into it, self-transitions
/// included (the epoch boundary, deliberately not `visit_attempt`'s). None
/// is owed when:
///
/// - the state declares no keys;
/// - the entry is `koto init`'s initial `transitioned` (no `from`), which
///   nothing can precede;
/// - a `context_cleared` for this state and this entry already follows it;
/// - every declared key was written after the entry. A `context_added` counts
///   as a write unless its writer is `sync`: a sync pull restores the remote
///   copy, which is the stale value this clearing exists to drop.
pub fn pending_clearing(events: &[Event], state: &str, keys: &[String]) -> Option<Clearing> {
    if keys.is_empty() {
        return None;
    }
    let idx = any_entry_index(events, state)?;
    let entry = &events[idx];
    if matches!(
        &entry.payload,
        EventPayload::Transitioned { from: None, .. }
    ) {
        return None;
    }
    let mut written: Vec<&str> = Vec::new();
    for e in &events[idx + 1..] {
        match &e.payload {
            EventPayload::ContextCleared {
                state: s,
                entry_seq,
                ..
            } if s == state && *entry_seq == entry.seq => return None,
            EventPayload::ContextAdded { key, writer, .. }
                if writer.as_deref() != Some(WRITER_SYNC) =>
            {
                written.push(key.as_str());
            }
            _ => {}
        }
    }
    let keys: Vec<String> = keys
        .iter()
        .filter(|k| !written.contains(&k.as_str()))
        .cloned()
        .collect();
    if keys.is_empty() {
        return None;
    }
    Some(Clearing {
        state: state.to_string(),
        keys,
        entry_seq: entry.seq,
    })
}

/// Remove a clearing's keys from the store. Stops at the first failure,
/// which the caller reports without appending the event.
pub fn remove_keys(store: &dyn ContextStore, session: &str, keys: &[String]) -> anyhow::Result<()> {
    for key in keys {
        store.remove(session, key)?;
    }
    Ok(())
}

/// Perform whatever clearing `state`'s latest entry still owes, reading the
/// session's persisted log, and report whether one was made.
///
/// This is the one place a clearing happens. The log it reads is the
/// persisted one, so `entry_seq` is the entry's real sequence number (the
/// advance loop's in-memory log numbers the events it appends itself, and a
/// gate script's own `koto context get` can append in between), and a write
/// another process made since the entry is seen and spared. The keys are
/// removed before the event is appended: a removal that fails returns the
/// error with nothing recorded, and the next call makes the clearing again.
pub fn apply_from_log(
    backend: &dyn SessionBackend,
    store: &dyn ContextStore,
    session: &str,
    state: &str,
    keys: &[String],
) -> anyhow::Result<bool> {
    if keys.is_empty() {
        return Ok(false);
    }
    let (_, events) = backend.read_events(session)?;
    let Some(clearing) = pending_clearing(&events, state, keys) else {
        return Ok(false);
    };
    remove_keys(store, session, &clearing.keys)?;
    backend.append_event(session, &clearing.event(), &now_iso8601())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(seq: u64, payload: EventPayload) -> Event {
        Event {
            seq,
            timestamp: String::new(),
            event_type: payload.type_name().to_string(),
            payload,
            idempotency_hash: None,
        }
    }

    fn tr(from: Option<&str>, to: &str) -> EventPayload {
        EventPayload::Transitioned {
            from: from.map(str::to_string),
            to: to.to_string(),
            condition_type: "auto".to_string(),
            skip_if_matched: None,
            context_assignments: None,
        }
    }

    fn added(key: &str, writer: Option<&str>) -> EventPayload {
        EventPayload::ContextAdded {
            key: key.to_string(),
            hash: "h".to_string(),
            size: 1,
            writer: writer.map(str::to_string),
        }
    }

    fn cleared(state: &str, keys: &[&str], entry_seq: u64) -> EventPayload {
        EventPayload::ContextCleared {
            state: state.to_string(),
            keys: keys.iter().map(|k| k.to_string()).collect(),
            entry_seq,
        }
    }

    fn keys(k: &[&str]) -> Vec<String> {
        k.iter().map(|s| s.to_string()).collect()
    }

    /// init -> review, a verdict written, review -> fix, fix -> review: the
    /// second entry into review is at seq 4.
    fn looped() -> Vec<Event> {
        vec![
            ev(1, tr(None, "review")),
            ev(2, added("verdict", Some("agent"))),
            ev(3, tr(Some("review"), "fix")),
            ev(4, tr(Some("fix"), "review")),
        ]
    }

    #[test]
    fn a_return_into_the_state_owes_a_clearing_of_every_declared_key() {
        let c = pending_clearing(&looped(), "review", &keys(&["verdict", "notes"])).unwrap();
        assert_eq!(
            c,
            Clearing {
                state: "review".to_string(),
                keys: keys(&["verdict", "notes"]),
                entry_seq: 4,
            }
        );
    }

    #[test]
    fn the_initial_entry_owes_nothing() {
        let events = vec![ev(1, tr(None, "review"))];
        assert_eq!(
            pending_clearing(&events, "review", &keys(&["verdict"])),
            None
        );
    }

    #[test]
    fn a_state_without_keys_owes_nothing() {
        assert_eq!(pending_clearing(&looped(), "review", &[]), None);
    }

    #[test]
    fn a_recorded_clearing_for_this_entry_settles_it() {
        let mut events = looped();
        events.push(ev(5, cleared("review", &["verdict"], 4)));
        assert_eq!(
            pending_clearing(&events, "review", &keys(&["verdict"])),
            None
        );
    }

    #[test]
    fn a_clearing_recorded_for_an_earlier_entry_does_not_settle_this_one() {
        let mut events = vec![
            ev(1, tr(None, "review")),
            ev(2, tr(Some("review"), "review")),
            ev(3, cleared("review", &["verdict"], 2)),
            ev(4, tr(Some("review"), "review")),
        ];
        assert_eq!(
            pending_clearing(&events, "review", &keys(&["verdict"])).map(|c| c.entry_seq),
            Some(4)
        );
        events.push(ev(5, cleared("review", &["verdict"], 4)));
        assert_eq!(
            pending_clearing(&events, "review", &keys(&["verdict"])),
            None
        );
    }

    #[test]
    fn a_self_transition_is_an_entry() {
        let events = vec![
            ev(1, tr(None, "analysis")),
            ev(2, tr(Some("analysis"), "analysis")),
        ];
        assert_eq!(
            pending_clearing(&events, "analysis", &keys(&["plan.md"])).map(|c| c.entry_seq),
            Some(2)
        );
    }

    #[test]
    fn a_directed_transition_and_a_rewind_are_entries() {
        let directed = vec![
            ev(1, tr(None, "work")),
            ev(
                2,
                EventPayload::DirectedTransition {
                    from: "work".to_string(),
                    to: "review".to_string(),
                    rationale: None,
                },
            ),
        ];
        assert_eq!(
            pending_clearing(&directed, "review", &keys(&["v"])).map(|c| c.entry_seq),
            Some(2)
        );
        let rewound = vec![
            ev(1, tr(None, "review")),
            ev(2, tr(Some("review"), "work")),
            ev(
                3,
                EventPayload::Rewound {
                    from: "work".to_string(),
                    to: "review".to_string(),
                    rationale: None,
                },
            ),
        ];
        assert_eq!(
            pending_clearing(&rewound, "review", &keys(&["v"])).map(|c| c.entry_seq),
            Some(3)
        );
    }

    #[test]
    fn a_key_written_since_the_entry_is_spared() {
        let mut events = looped();
        events.push(ev(5, added("verdict", Some("agent"))));
        assert_eq!(
            pending_clearing(&events, "review", &keys(&["verdict", "notes"])).map(|c| c.keys),
            Some(keys(&["notes"]))
        );
    }

    #[test]
    fn when_every_key_was_written_since_the_entry_nothing_is_owed() {
        let mut events = looped();
        events.push(ev(5, added("verdict", None)));
        assert_eq!(
            pending_clearing(&events, "review", &keys(&["verdict"])),
            None
        );
    }

    #[test]
    fn a_sync_pull_is_not_a_write_that_spares_the_key() {
        let mut events = looped();
        events.push(ev(5, added("verdict", Some(WRITER_SYNC))));
        assert_eq!(
            pending_clearing(&events, "review", &keys(&["verdict"])).map(|c| c.keys),
            Some(keys(&["verdict"]))
        );
    }

    #[test]
    fn a_write_before_the_entry_does_not_spare_the_key() {
        // The verdict written at seq 2 predates the entry at seq 4.
        assert_eq!(
            pending_clearing(&looped(), "review", &keys(&["verdict"])).map(|c| c.keys),
            Some(keys(&["verdict"]))
        );
    }

    #[test]
    fn the_event_carries_key_names_only() {
        let c = pending_clearing(&looped(), "review", &keys(&["verdict"])).unwrap();
        let json = serde_json::to_value(c.event()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"state": "review", "keys": ["verdict"], "entry_seq": 4})
        );
    }
}
