//! Finding the earlier event a committed one pairs with, in the raw log
//! as it stood before the commit (`CommitInfo::prior_log`).
//!
//! Both lookups scan the log backwards and parse only the lines that can
//! match, so a long visit's check output is never parsed.

use crate::decider::check::DeciderFallback;
use crate::decider::DeciderConsultation;
use crate::engine::persistence::{entered_state, may_be_entry};
use crate::engine::types::{Event, EventPayload};

/// The consultation of `state` in the current decider visit to it, with the
/// `seq` of its `decider_consulted` event, or `None` when the visit holds
/// none or the log isn't in `state`.
///
/// The visit is the decider's: it begins at the last state entry naming the
/// state, a self-loop included (`Boundary::AnyEntry`, the rule
/// `engine::decider::prior_consultation` reads the parsed log by), so an
/// answer pairs with the consultation of its own lap, never an earlier one.
pub(super) fn consultation(log: &str, state: &str) -> Option<(u64, DeciderConsultation)> {
    let mut latest: Option<(u64, DeciderConsultation)> = None;
    for line in log.lines().rev() {
        let entry = may_be_entry(line);
        if !entry && !line.contains("\"decider_consulted\"") {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Event>(line.trim()) else {
            continue;
        };
        if let Some(to) = entered_state(&event.payload) {
            // The visit's opening entry: the consultation found after it
            // belongs to it only if it names this state and this visit.
            return latest.filter(|(_, c)| to == state && c.visit_seq == event.seq);
        }
        if let EventPayload::DeciderConsulted(c) = event.payload {
            if latest.is_none() && c.state == state {
                latest = Some((event.seq, c));
            }
        }
    }
    None
}

/// The `decider_fallback` event whose `seq` is `seq`, or `None` when the
/// log holds none.
pub(super) fn fallback(log: &str, seq: u64) -> Option<DeciderFallback> {
    for line in log.lines().rev() {
        if !line.contains("\"decider_fallback\"") {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Event>(line.trim()) else {
            continue;
        };
        if event.seq < seq {
            return None;
        }
        if let (true, EventPayload::DeciderFallback(f)) = (event.seq == seq, event.payload) {
            return Some(f);
        }
    }
    None
}
