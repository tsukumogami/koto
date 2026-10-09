//! A session's run id and its lineage.
//!
//! A run is a root session and every session spawned under it. Its id is
//! the root's `session_id`. A child records its root and parent ids in its
//! header when it is created (`root_session_id`, `parent_session_id`), so
//! its run stays identifiable after the root has been removed.
//!
//! A session created by an older koto has neither field. Its run id is
//! found by walking `parent_workflow` names up to the root, and is `None`
//! when a link in that chain is gone: an omitted run id is better than a
//! wrong one, so a child's run id is never a non-root ancestor's id.

use crate::engine::types::StateFileHeader;
use crate::session::SessionBackend;

use super::sidecar;

/// How far the parent-name walk goes before giving up. Far deeper than any
/// real hierarchy; it only stops a cycle in hand-edited headers.
const MAX_WALK: usize = 64;

fn id(value: &str) -> Option<String> {
    if super::is_id(value) {
        Some(value.to_string())
    } else {
        None
    }
}

/// The run id recorded in, or implied by, `header` alone: its
/// `root_session_id`, or its own `session_id` when it has no parent.
fn from_header_alone(header: &StateFileHeader) -> Option<Option<String>> {
    if let Some(root) = header.root_session_id.as_deref() {
        return Some(id(root));
    }
    if header.parent_workflow.is_none() {
        return Some(id(&header.session_id));
    }
    None
}

/// The run id of the session whose header is `header`.
///
/// The header's own fields answer for every session this koto created. A
/// child from an older koto falls back to the walk up its parent chain.
pub(crate) fn run_id(backend: &dyn SessionBackend, header: &StateFileHeader) -> Option<String> {
    if let Some(found) = from_header_alone(header) {
        return found;
    }
    walk(backend, header.parent_workflow.as_deref())
}

/// Walk `parent_workflow` names from `start` to the root. An ancestor that
/// recorded its root, or cached a run id in its sidecar, ends the walk
/// early.
fn walk(backend: &dyn SessionBackend, start: Option<&str>) -> Option<String> {
    let mut next = start.map(str::to_string);
    for _ in 0..MAX_WALK {
        let name = next?;
        if !backend.exists(&name) {
            return None;
        }
        let header = backend.read_header(&name).ok()?;
        if let Some(found) = from_header_alone(&header) {
            return found;
        }
        if let Some(cached) = sidecar::read(&backend.session_dir(&name)).and_then(|s| s.run_id) {
            return Some(cached);
        }
        next = header.parent_workflow;
    }
    None
}

/// The `(root_session_id, parent_session_id)` a new child of `parent`
/// records in its header.
///
/// The root is the parent's recorded root when it has one, the parent
/// itself when it has no parent, and otherwise (a parent created by an
/// older koto) the parent's cached run id or the walk above it. Either
/// value is `None` when it can't be resolved; nothing here fails a spawn.
pub(crate) fn child_lineage(
    backend: &dyn SessionBackend,
    parent: &str,
) -> (Option<String>, Option<String>) {
    let Ok(parent_header) = backend.read_header(parent) else {
        return (None, None);
    };
    let parent_id = id(&parent_header.session_id);
    let root = match from_header_alone(&parent_header) {
        Some(found) => found,
        None => sidecar::read(&backend.session_dir(parent))
            .and_then(|s| s.run_id)
            .or_else(|| walk(backend, parent_header.parent_workflow.as_deref())),
    };
    (root, parent_id)
}
