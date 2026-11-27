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
//! A run also has a template: its root session's. Every record of a run
//! can name it, so a reader can group a run's sessions by what the run was
//! started as. A root answers from its own header. A child caches the
//! answer in its sidecar when it is created, the one time its parent is
//! sure to be on this host, and otherwise finds it by the same walk, which
//! accepts only the header of the session whose id is the run id. A
//! template that can't be resolved is left out, never guessed.
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

use crate::telemetry::sidecar;

/// How far the parent-name walk goes before giving up. Far deeper than any
/// real hierarchy; it only stops a cycle in hand-edited headers.
const MAX_WALK: usize = 64;

fn id(value: &str) -> Option<String> {
    if crate::names::is_id(value) {
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

/// The template of a run: its root session's template name and hash, each
/// present only when it resolved to a value of the right shape.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RunTemplate {
    pub(crate) name: Option<String>,
    pub(crate) hash: Option<String>,
}

impl RunTemplate {
    /// Whether neither value resolved.
    pub(crate) fn is_empty(&self) -> bool {
        self.name.is_none() && self.hash.is_none()
    }

    /// The template `header` itself records: its name when it has the name
    /// shape, and its hash when it is non-empty and id-shaped. An empty
    /// hash is left out, as `session_started` leaves it out.
    fn of(header: &StateFileHeader) -> Self {
        RunTemplate {
            name: header
                .template_name
                .clone()
                .filter(|n| crate::names::is_name(n)),
            hash: id(&header.template_hash),
        }
    }
}

/// The run template of the session `name`, whose header is `header`.
///
/// A root answers from its own header. A child answers from the two fields
/// its sidecar caches, and when they are empty from [`found_run_template`].
/// A cached template is used only when the sidecar's run id doesn't
/// contradict the run id the header records, so a header that was re-rooted
/// by hand never keeps printing another run's template.
///
/// Nothing is written here. The sidecar is kept by the telemetry module,
/// which stores what [`found_run_template`] returns when a child is created
/// and on a later record of a child that caches none.
pub(crate) fn run_template(
    backend: &dyn SessionBackend,
    name: &str,
    header: &StateFileHeader,
) -> RunTemplate {
    if header.parent_workflow.is_none() {
        return RunTemplate::of(header);
    }
    // The run id the header alone states, when it states one.
    let recorded = from_header_alone(header).flatten();
    let cached = sidecar::read(&backend.session_dir(name))
        .filter(|s| match (&recorded, &s.run_id) {
            (Some(recorded), Some(cached)) => recorded == cached,
            _ => true,
        })
        .map(|s| RunTemplate {
            name: s.run_template_name,
            hash: s.run_template_hash,
        })
        .unwrap_or_default();
    if !cached.is_empty() {
        return cached;
    }
    found_run_template(backend, header)
}

/// The run template of a child whose header is `header`, found without its
/// own sidecar: from the first ancestor on this host that can answer.
///
/// The walk goes up `parent_workflow` names, as [`walk`] does. A root ends
/// it: its template is the run's only when its `session_id` is the child's
/// run id, so a chain that was re-rooted or hand-edited never lends another
/// run's template. An ancestor whose sidecar caches a template for the same
/// run id ends it early. An ancestor that is missing or unreadable ends it
/// with nothing, as does a child with no run id.
pub(crate) fn found_run_template(
    backend: &dyn SessionBackend,
    header: &StateFileHeader,
) -> RunTemplate {
    let Some(run) = run_id(backend, header) else {
        return RunTemplate::default();
    };
    let mut next = header.parent_workflow.clone();
    for _ in 0..MAX_WALK {
        let Some(name) = next else {
            break;
        };
        let Some(ancestor) = local_header(backend, &name) else {
            break;
        };
        if ancestor.parent_workflow.is_none() {
            if ancestor.session_id == run {
                return RunTemplate::of(&ancestor);
            }
            break;
        }
        if let Some(cached) = sidecar::read(&backend.session_dir(&name)) {
            let template = RunTemplate {
                name: cached.run_template_name,
                hash: cached.run_template_hash,
            };
            if cached.run_id.as_deref() == Some(run.as_str()) && !template.is_empty() {
                return template;
            }
        }
        next = ancestor.parent_workflow;
    }
    RunTemplate::default()
}

/// What `koto session identity` prints: a session's id, its run's id and
/// its run's template. Each value is present only when it resolved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Identity {
    pub(crate) session_id: Option<String>,
    pub(crate) run_id: Option<String>,
    pub(crate) run_template: RunTemplate,
}

/// Why [`identity`] has no answer.
#[derive(Debug)]
pub(crate) enum IdentityError {
    /// The name isn't a valid session name.
    InvalidName(String),
    /// This host's store holds no session of that name.
    NoSuchSession,
    /// The session's state file is there and its header can't be read.
    Unreadable(String),
}

/// The identity of session `name`, read from this host's store alone and
/// with nothing written: the lookup the journal's records use, minus the
/// caching, so the values are the ones those records carry.
///
/// The run id is the one the sidecar caches when it caches one, as it is
/// for a record, and otherwise the one the header and the walk give.
pub(crate) fn identity(
    backend: &dyn SessionBackend,
    name: &str,
) -> Result<Identity, IdentityError> {
    validate_session_id(name).map_err(|e| IdentityError::InvalidName(e.to_string()))?;
    let dir = backend.session_dir(name);
    let path = dir.join(state_file_name(name));
    if !path.is_file() {
        return Err(IdentityError::NoSuchSession);
    }
    let header =
        persistence::read_header(&path).map_err(|e| IdentityError::Unreadable(format!("{e:#}")))?;
    let run_id = sidecar::read(&dir)
        .and_then(|s| s.run_id)
        .or_else(|| run_id(backend, &header));
    Ok(Identity {
        session_id: id(&header.session_id),
        run_id,
        run_template: run_template(backend, name, &header),
    })
}
