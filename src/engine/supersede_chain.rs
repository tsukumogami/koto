//! Following a session's supersede fences forward, for the wake rings.
//!
//! `koto init SUCC --continues PRED --supersede` appends a
//! `session_superseded` event to PRED's log naming SUCC and SUCC's
//! session id. A request's header names its requester and coordinator of
//! record once, so a leg that member A opened still names A after A was
//! superseded by B and B by C. The rings in
//! [`crate::engine::wake_signal`] and [`crate::engine::wake`] use
//! [`newest_member`] to reach C as well as A; nothing in the request
//! changes.
//!
//! # The walk
//!
//! Every name read from a fence is data. It must be a valid session id
//! and a valid workflow name before a path is built from it. Each hop's
//! log must exist and its header's `session_id` must equal the fence's
//! `by_session_id`; a hop that fails either, or that the walk already
//! visited, ends the walk at the last good member. After
//! [`MAX_CHAIN_HOPS`] hops the walk gives up and the ring reaches the
//! principal only.
//!
//! # Cost
//!
//! A principal that was never superseded costs one read of its log and a
//! substring search. Each superseded hop costs one read of the next
//! member's log, which yields both its header and its own fence. Logs are
//! read from the local backend's layout,
//! `<sessions>/<name>/koto-<name>.state.jsonl`, without a lock.

use std::collections::HashSet;
use std::path::Path;

use crate::engine::persistence::{superseded_by_in_log, SupersedeFence};
use crate::engine::types::ValidatedSessionId;
use crate::session::state_file_name;

/// The most supersede hops [`newest_member`] follows before it gives up.
pub const MAX_CHAIN_HOPS: usize = 1024;

/// A name a fence carries that is safe to build a path from and to ring.
fn usable_name(name: &str) -> bool {
    ValidatedSessionId::new(name).is_ok() && crate::discover::validate_workflow_name(name).is_ok()
}

/// One read of `name`'s log: its header's `session_id` (`None` when the
/// header has none) and its own fence. `None` when the log can't be read.
fn read_member(
    sessions_dir: &Path,
    name: &str,
) -> Option<(Option<String>, Option<SupersedeFence>)> {
    #[derive(serde::Deserialize)]
    struct Identity {
        session_id: Option<String>,
    }
    let content =
        std::fs::read_to_string(sessions_dir.join(name).join(state_file_name(name))).ok()?;
    let session_id = content
        .lines()
        .next()
        .and_then(|header| serde_json::from_str::<Identity>(header).ok())
        .and_then(|identity| identity.session_id);
    Some((session_id, superseded_by_in_log(&content)))
}

/// The newest member of `principal`'s supersede chain under
/// `sessions_dir`, or `None` when the principal isn't superseded, its
/// first hop is broken, or the chain is longer than [`MAX_CHAIN_HOPS`].
///
/// A broken hop (a name that isn't usable, a missing or unreadable log, a
/// header whose `session_id` isn't the fence's `by_session_id`, or a
/// member already visited) ends the walk at the last good member before
/// it. The principal itself is never returned.
pub fn newest_member(sessions_dir: &Path, principal: &str) -> Option<String> {
    if !usable_name(principal) {
        return None;
    }
    let mut fence = read_member(sessions_dir, principal)?.1?;
    let mut visited = HashSet::from([principal.to_string()]);
    let mut last_good = None;
    for _ in 0..MAX_CHAIN_HOPS {
        let member = fence.by;
        if !usable_name(&member) || visited.contains(&member) {
            return last_good;
        }
        let Some((session_id, next)) = read_member(sessions_dir, &member) else {
            return last_good;
        };
        if session_id.as_deref() != Some(fence.by_session_id.as_str()) {
            return last_good;
        }
        visited.insert(member.clone());
        last_good = Some(member);
        match next {
            Some(next) => fence = next,
            None => return last_good,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::{Event, EventPayload, SupersedeActor};
    use std::path::PathBuf;

    fn log(dir: &Path, name: &str) -> PathBuf {
        dir.join(name).join(state_file_name(name))
    }

    /// A session `name` whose header records `session_id`.
    fn session(dir: &Path, name: &str, session_id: &str) {
        std::fs::create_dir_all(dir.join(name)).unwrap();
        let header =
            serde_json::json!({"schema_version": 1, "workflow": name, "session_id": session_id});
        std::fs::write(log(dir, name), format!("{header}\n")).unwrap();
    }

    /// Fence `name` in favour of `by`, whose session id the fence records
    /// as `by_session_id`.
    fn fence(dir: &Path, name: &str, by: &str, by_session_id: &str) {
        let event = Event {
            seq: 1,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: "session_superseded".to_string(),
            payload: EventPayload::SessionSuperseded {
                by: by.to_string(),
                by_session_id: by_session_id.to_string(),
                actor: SupersedeActor::default(),
                at: "2026-01-01T00:00:00Z".to_string(),
            },
            idempotency_hash: None,
        };
        let mut content = std::fs::read_to_string(log(dir, name)).unwrap();
        content.push_str(&serde_json::to_string(&event).unwrap());
        content.push('\n');
        std::fs::write(log(dir, name), content).unwrap();
    }

    /// Sessions `names`, each fenced in favour of the next with the next
    /// one's real id.
    fn chain(dir: &Path, names: &[&str]) {
        for name in names {
            session(dir, name, &format!("id-{name}"));
        }
        for pair in names.windows(2) {
            fence(dir, pair[0], pair[1], &format!("id-{}", pair[1]));
        }
    }

    #[test]
    fn a_session_never_superseded_has_no_newest_member() {
        let tmp = tempfile::tempdir().unwrap();
        session(tmp.path(), "a", "id-a");
        assert_eq!(newest_member(tmp.path(), "a"), None);
        assert_eq!(newest_member(tmp.path(), "absent"), None);
        assert_eq!(newest_member(tmp.path(), "../a"), None);
    }

    #[test]
    fn the_walk_follows_every_fence_to_the_newest_member() {
        let tmp = tempfile::tempdir().unwrap();
        chain(tmp.path(), &["a", "b", "c"]);
        assert_eq!(newest_member(tmp.path(), "a").as_deref(), Some("c"));
        assert_eq!(newest_member(tmp.path(), "b").as_deref(), Some("c"));
        assert_eq!(newest_member(tmp.path(), "c"), None);
    }

    #[test]
    fn a_broken_hop_stops_the_walk_at_the_last_good_member() {
        // An id that isn't the member's.
        let tmp = tempfile::tempdir().unwrap();
        chain(tmp.path(), &["a", "b"]);
        session(tmp.path(), "c", "id-c");
        fence(tmp.path(), "b", "c", "id-other");
        assert_eq!(newest_member(tmp.path(), "a").as_deref(), Some("b"));
        assert_eq!(newest_member(tmp.path(), "b"), None);

        // A member whose log is missing.
        let tmp = tempfile::tempdir().unwrap();
        chain(tmp.path(), &["a", "b"]);
        fence(tmp.path(), "b", "ghost", "id-ghost");
        assert_eq!(newest_member(tmp.path(), "a").as_deref(), Some("b"));

        // A name no path may be built from.
        let tmp = tempfile::tempdir().unwrap();
        chain(tmp.path(), &["a", "b"]);
        fence(tmp.path(), "b", "../escape", "id-x");
        assert_eq!(newest_member(tmp.path(), "a").as_deref(), Some("b"));
    }

    #[test]
    fn a_cycle_in_the_fences_ends_the_walk() {
        let tmp = tempfile::tempdir().unwrap();
        chain(tmp.path(), &["a", "b"]);
        fence(tmp.path(), "b", "a", "id-a");
        assert_eq!(newest_member(tmp.path(), "a").as_deref(), Some("b"));
        assert_eq!(newest_member(tmp.path(), "b").as_deref(), Some("a"));
    }

    #[test]
    fn a_chain_past_the_hop_cap_reaches_no_member() {
        let tmp = tempfile::tempdir().unwrap();
        let names: Vec<String> = (0..=MAX_CHAIN_HOPS + 1).map(|i| format!("s{i}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        chain(tmp.path(), &refs);
        let last = names.last().unwrap();
        // s1 is exactly MAX_CHAIN_HOPS hops from the last member; s0 is one more.
        assert_eq!(
            newest_member(tmp.path(), "s1").as_deref(),
            Some(last.as_str())
        );
        assert_eq!(newest_member(tmp.path(), "s0"), None);
    }
}
