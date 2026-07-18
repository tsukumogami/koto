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
//! the root.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::session::context::ContextStore;
use crate::session::SessionBackend;

/// Reserved context-store key holding a session's published `/workflows`
/// directory. Namespaced under `workflows/` so koto's agent-surface reserved
/// keys never collide with user context keys; passes `validate_context_key`.
pub const PUBLISH_LOCATION_KEY: &str = "workflows/publish-location";

/// Upper bound on ancestor-walk hops (matches `measure_depth_from_parent`).
const MAX_WALK_HOPS: u32 = 1000;

/// Publish `dir` as `session_id`'s `/workflows` location.
///
/// Writes the reserved key through [`ContextStore::add`], which does **not**
/// append an event -- so publishing neither perturbs the session's event log
/// nor re-enters the commit funnel.
pub fn publish_location(
    store: &dyn ContextStore,
    session_id: &str,
    dir: &str,
) -> anyhow::Result<()> {
    store.add(session_id, PUBLISH_LOCATION_KEY, dir.as_bytes())
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
                        return Some(PathBuf::from(trimmed));
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
        publish_location(&backend, "solo", "/tmp/wf/solo").unwrap();

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
        publish_location(&backend, "root", "/tmp/wf/root").unwrap();

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
        publish_location(&backend, "root", "/tmp/wf/root").unwrap();
        publish_location(&backend, "mid", "/tmp/wf/mid").unwrap();

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
}
