//! Publish and discover a session's `/workflows` directory through koto's
//! per-session context store.
//!
//! The published location lives under the reserved, namespaced key
//! [`PUBLISH_LOCATION_KEY`] in a session's context store; the value is the
//! absolute `/workflows` directory path (stored as *content*, never as a key,
//! so the key-charset restrictions never bite the path).
//!
//! Discovery is the **nearest-published-ancestor walk**: a session resolves its
//! target directory by walking from itself up its `parent_workflow` chain and
//! taking the first ancestor that published a location. For a single
//! session the walk starts and ends at the session itself; a future hierarchy exercises
//! the same walk over a real tree with no change to this key or algorithm. The
//! walk copies `crate::engine::caps::measure_depth_from_parent`'s robustness:
//! a cycle guard, a hop cap, and treating a missing header or empty parent as
//! the root. [`resolve_publish_owner`] runs the same walk and also names the
//! session the location was found on, which is the only session whose key
//! the renderer may rewrite.
//!
//! A location set with `koto workflows publish` is marked explicit: the same
//! directory is written under [`EXPLICIT_LOCATION_KEY`] too
//! ([`publish_explicit_location`]). The renderer moves a self-discovered
//! location when another Claude Code session starts driving the run
//! (koto#361), but never an explicit one. The marker counts only while its
//! content equals the stored location ([`is_explicit_location`]), so once
//! anything else rewrites the location, the stale marker has no effect.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::session::context::ContextStore;
use crate::session::SessionBackend;

/// Reserved context-store key holding a session's published `/workflows`
/// directory. Namespaced under `workflows/` so koto's agent-surface reserved
/// keys never collide with user context keys; passes `validate_context_key`.
pub const PUBLISH_LOCATION_KEY: &str = "workflows/publish-location";

/// Reserved context-store key marking a session's published location as
/// explicit: `koto workflows publish` writes the published directory here as
/// well as under [`PUBLISH_LOCATION_KEY`]. While the two hold the same
/// directory, a change of driving Claude Code session doesn't move the
/// location.
pub const EXPLICIT_LOCATION_KEY: &str = "workflows/publish-explicit";

/// Upper bound on ancestor-walk hops (matches `measure_depth_from_parent`).
const MAX_WALK_HOPS: u32 = 1000;

/// Publish `dir` as `session_id`'s `/workflows` location.
///
/// Writes the reserved key with writer `koto`, then appends a
/// `context_added` event naming that writer to the session's log,
/// best-effort: a failed append warns and still leaves the location
/// published. That append goes through the commit funnel, and when
/// publishing happens during a materialization,
/// [`materialize_after_commit`](super::materialize_after_commit)'s
/// re-entrancy guard keeps it from materializing again.
pub fn publish_location(
    backend: &dyn SessionBackend,
    store: &dyn ContextStore,
    session_id: &str,
    dir: &str,
) -> anyhow::Result<()> {
    use crate::session::context_log::{added_event, append_to_session_best_effort, WRITER_KOTO};
    store.add_with_writer(
        session_id,
        PUBLISH_LOCATION_KEY,
        dir.as_bytes(),
        WRITER_KOTO,
    )?;
    append_to_session_best_effort(
        backend,
        session_id,
        &added_event(PUBLISH_LOCATION_KEY, dir.as_bytes(), WRITER_KOTO),
    );
    Ok(())
}

/// Publish `dir` as `session_id`'s location and mark it explicit, for
/// `koto workflows publish`.
///
/// The marker goes to the store first, then [`publish_location`] writes the
/// location and appends its `context_added`, then the marker's own
/// `context_added` is appended (best-effort). The order matters: the
/// location's event goes through the commit funnel and materializes, and
/// that render has to see a matching marker, or a driver change could move
/// the location away before the marker lands.
pub fn publish_explicit_location(
    backend: &dyn SessionBackend,
    store: &dyn ContextStore,
    session_id: &str,
    dir: &str,
) -> anyhow::Result<()> {
    use crate::session::context_log::{added_event, append_to_session_best_effort, WRITER_KOTO};
    store.add_with_writer(
        session_id,
        EXPLICIT_LOCATION_KEY,
        dir.as_bytes(),
        WRITER_KOTO,
    )?;
    publish_location(backend, store, session_id, dir)?;
    append_to_session_best_effort(
        backend,
        session_id,
        &added_event(EXPLICIT_LOCATION_KEY, dir.as_bytes(), WRITER_KOTO),
    );
    Ok(())
}

/// Whether `location`, the location published on `owner`, was set with
/// `koto workflows publish`: `owner` holds [`EXPLICIT_LOCATION_KEY`] and its
/// content names the same directory. A marker naming any other directory is
/// stale and doesn't count.
pub fn is_explicit_location(store: &dyn ContextStore, owner: &str, location: &Path) -> bool {
    if !store.ctx_exists(owner, EXPLICIT_LOCATION_KEY) {
        return false;
    }
    let Ok(bytes) = store.get(owner, EXPLICIT_LOCATION_KEY) else {
        return false;
    };
    let Ok(marker) = String::from_utf8(bytes) else {
        return false;
    };
    let marker = marker.trim();
    !marker.is_empty() && Path::new(marker) == location
}

/// Whether `session_id` already has a published location in its own store.
pub fn has_published_location(store: &dyn ContextStore, session_id: &str) -> bool {
    store.ctx_exists(session_id, PUBLISH_LOCATION_KEY)
}

/// Resolve the target `/workflows` directory for `session_id` by walking from
/// itself up the `parent_workflow` chain and taking the nearest ancestor that
/// published a location. Returns `None` when no ancestor published one.
///
/// `backend` supplies the header chain (`parent_workflow`); `store` holds the
/// published keys. For a `LocalBackend` these are the same object, passed twice.
pub fn resolve_publish_location(
    backend: &dyn SessionBackend,
    store: &dyn ContextStore,
    session_id: &str,
) -> Option<PathBuf> {
    resolve_publish_owner(backend, store, session_id).map(|(_, location)| location)
}

/// [`resolve_publish_location`], also naming the session the location was
/// found on: `session_id` itself, or the ancestor that owns the key.
pub fn resolve_publish_owner(
    backend: &dyn SessionBackend,
    store: &dyn ContextStore,
    session_id: &str,
) -> Option<(String, PathBuf)> {
    let mut current = session_id.to_string();
    let mut hops: u32 = 0;
    let mut seen: HashSet<String> = HashSet::new();

    loop {
        if hops > MAX_WALK_HOPS {
            break;
        }
        // Cycle guard: a name already visited ends the walk.
        if !seen.insert(current.clone()) {
            break;
        }
        hops += 1;

        // Probe this session's store for a published location.
        if store.ctx_exists(&current, PUBLISH_LOCATION_KEY) {
            if let Ok(bytes) = store.get(&current, PUBLISH_LOCATION_KEY) {
                if let Ok(s) = String::from_utf8(bytes) {
                    let trimmed = s.trim();
                    if !trimmed.is_empty() {
                        return Some((current, PathBuf::from(trimmed)));
                    }
                }
            }
        }

        // Walk to the parent; a missing header or empty parent is the root.
        let header = match backend.read_header(&current) {
            Ok(h) => h,
            Err(_) => break,
        };
        match header.parent_workflow {
            Some(p) if !p.is_empty() => current = p,
            _ => break,
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::{Event, EventPayload, StateFileHeader};
    use crate::session::local::LocalBackend;
    use tempfile::TempDir;

    fn make_header(workflow: &str, parent: Option<&str>) -> StateFileHeader {
        let mut h: StateFileHeader = serde_json::from_str(
            r#"{"schema_version":1,"workflow":"x","template_hash":"h","created_at":"2026-01-01T00:00:00Z"}"#,
        )
        .expect("minimal header deserializes");
        h.workflow = workflow.to_string();
        h.parent_workflow = parent.map(str::to_string);
        h
    }

    fn init_session(backend: &LocalBackend, workflow: &str, parent: Option<&str>) {
        let header = make_header(workflow, parent);
        let _ = parent;
        let init_event = Event {
            seq: 1,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: "workflow_initialized".to_string(),
            payload: EventPayload::WorkflowInitialized {
                template_path: "template.json".to_string(),
                variables: Default::default(),
                spawn_entry: None,
                machine: None,
            },
            idempotency_hash: None,
        };
        backend
            .init_state_file(workflow, header, vec![init_event])
            .expect("init session");
    }

    #[test]
    fn resolves_own_published_location() {
        let tmp = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(tmp.path().to_path_buf());
        init_session(&backend, "solo", None);
        publish_location(&backend, &backend, "solo", "/tmp/wf/solo").unwrap();

        let got = resolve_publish_location(&backend, &backend, "solo");
        assert_eq!(got, Some(PathBuf::from("/tmp/wf/solo")));
    }

    #[test]
    fn resolves_nearest_published_ancestor() {
        let tmp = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(tmp.path().to_path_buf());
        init_session(&backend, "root", None);
        init_session(&backend, "child", Some("root"));
        // Only the ancestor published; the child inherits via the walk.
        publish_location(&backend, &backend, "root", "/tmp/wf/root").unwrap();

        let got = resolve_publish_location(&backend, &backend, "child");
        assert_eq!(got, Some(PathBuf::from("/tmp/wf/root")));
    }

    #[test]
    fn nearest_wins_over_farther_ancestor() {
        let tmp = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(tmp.path().to_path_buf());
        init_session(&backend, "root", None);
        init_session(&backend, "mid", Some("root"));
        init_session(&backend, "leaf", Some("mid"));
        publish_location(&backend, &backend, "root", "/tmp/wf/root").unwrap();
        publish_location(&backend, &backend, "mid", "/tmp/wf/mid").unwrap();

        // The nearer ancestor (mid) wins over the farther one (root).
        let got = resolve_publish_location(&backend, &backend, "leaf");
        assert_eq!(got, Some(PathBuf::from("/tmp/wf/mid")));
    }

    #[test]
    fn returns_none_with_no_published_location() {
        let tmp = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(tmp.path().to_path_buf());
        init_session(&backend, "lonely", None);

        assert_eq!(resolve_publish_location(&backend, &backend, "lonely"), None);
    }

    #[test]
    fn resolve_publish_owner_names_the_session_holding_the_key() {
        let tmp = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(tmp.path().to_path_buf());
        init_session(&backend, "root", None);
        init_session(&backend, "mid", Some("root"));
        init_session(&backend, "leaf", Some("mid"));
        publish_location(&backend, &backend, "root", "/tmp/wf/root").unwrap();

        assert_eq!(
            resolve_publish_owner(&backend, &backend, "leaf"),
            Some(("root".to_string(), PathBuf::from("/tmp/wf/root")))
        );
        assert_eq!(
            resolve_publish_owner(&backend, &backend, "root"),
            Some(("root".to_string(), PathBuf::from("/tmp/wf/root")))
        );

        publish_location(&backend, &backend, "leaf", "/tmp/wf/leaf").unwrap();
        assert_eq!(
            resolve_publish_owner(&backend, &backend, "leaf"),
            Some(("leaf".to_string(), PathBuf::from("/tmp/wf/leaf")))
        );
    }

    #[test]
    fn an_explicit_publish_writes_the_marker_and_logs_both_keys() {
        let tmp = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(tmp.path().to_path_buf());
        init_session(&backend, "solo", None);

        publish_explicit_location(&backend, &backend, "solo", "/tmp/wf/solo").unwrap();

        assert_eq!(
            backend.get("solo", EXPLICIT_LOCATION_KEY).unwrap(),
            b"/tmp/wf/solo"
        );
        assert_eq!(
            resolve_publish_location(&backend, &backend, "solo"),
            Some(PathBuf::from("/tmp/wf/solo"))
        );
        let (_, events) = backend.read_events("solo").unwrap();
        let added: Vec<&str> = events
            .iter()
            .filter_map(|e| match &e.payload {
                EventPayload::ContextAdded { key, .. } => Some(key.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(added, vec![PUBLISH_LOCATION_KEY, EXPLICIT_LOCATION_KEY]);
    }

    #[test]
    fn a_location_is_explicit_only_while_the_marker_names_it() {
        let tmp = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(tmp.path().to_path_buf());
        init_session(&backend, "solo", None);
        let here = Path::new("/tmp/wf/solo");

        // No marker: not explicit.
        publish_location(&backend, &backend, "solo", "/tmp/wf/solo").unwrap();
        assert!(!is_explicit_location(&backend, "solo", here));

        // An explicit publish marks it.
        publish_explicit_location(&backend, &backend, "solo", "/tmp/wf/solo").unwrap();
        assert!(is_explicit_location(&backend, "solo", here));

        // Anything else rewriting the location leaves the marker stale.
        publish_location(&backend, &backend, "solo", "/tmp/wf/other").unwrap();
        assert!(!is_explicit_location(
            &backend,
            "solo",
            Path::new("/tmp/wf/other")
        ));

        // The marker is read from the owner only.
        init_session(&backend, "child", Some("solo"));
        assert!(!is_explicit_location(&backend, "child", here));
    }
}
