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
//!
//! The walk, and the parent read a new child's lineage starts from, read
//! headers from this host's store only (see [`local_header`]). They never
//! go through `SessionBackend::read_header`, which on a cloud store pulls
//! the remote copy and checks for a migration marker: a lineage lookup
//! must not cost a round trip per ancestor, and an ancestor that is
//! missing, unreadable or migrated ends the walk with no run id rather
//! than an error.

use crate::engine::persistence;
use crate::engine::types::StateFileHeader;
use crate::session::validate::validate_session_id;
use crate::session::{state_file_name, SessionBackend};

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

/// The header of session `name` as this host's store holds it, read
/// straight from the state file under `backend.session_dir(name)`. `None`
/// when the name isn't a valid session id or the file is missing or
/// unreadable; nothing here reaches a remote store.
fn local_header(backend: &dyn SessionBackend, name: &str) -> Option<StateFileHeader> {
    validate_session_id(name).ok()?;
    let path = backend.session_dir(name).join(state_file_name(name));
    persistence::read_header(&path).ok()
}

/// The run id recorded in, or implied by, `header` alone: its
/// `root_session_id`, or its own `session_id` when it has no parent.
///
/// The outer `None` means the header can't say (an older child: the walk
/// has to answer); `Some(None)` means it says, and the answer is that the
/// value isn't id-shaped, so the run id is left out.
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

/// Walk `parent_workflow` names from `start` to the root, reading each
/// ancestor's header from the local store. An ancestor that recorded its
/// root, or cached a run id in its sidecar, ends the walk early; one whose
/// header can't be read locally ends it with no run id.
fn walk(backend: &dyn SessionBackend, start: Option<&str>) -> Option<String> {
    let mut next = start.map(str::to_string);
    for _ in 0..MAX_WALK {
        let name = next?;
        let header = local_header(backend, &name)?;
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
/// older koto) the parent's cached run id or the walk above it. The
/// parent's header, like every ancestor's, is read from the local store.
/// Either value is `None` when it can't be resolved; nothing here fails a
/// spawn.
pub(crate) fn child_lineage(
    backend: &dyn SessionBackend,
    parent: &str,
) -> (Option<String>, Option<String>) {
    let Some(parent_header) = local_header(backend, parent) else {
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
