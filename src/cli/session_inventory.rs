//! One read of a session's standing: what `koto session list --state` and
//! the prune report about each session.
//!
//! [`inspect`] reads a session's header and log from the local store and
//! looks its current state up in the compiled template, which a
//! [`TemplateCache`] reads and parses once per path for a whole listing:
//! most sessions in a store share a handful of templates. Nothing here
//! writes, and nothing reaches a remote store.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::engine::persistence;
use crate::engine::types::EventPayload;
use crate::session::state_file_name;
use crate::template::types::CompiledTemplate;

/// Where a session stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Readable and not finished.
    Running,
    /// Standing in a state its template marks `terminal`.
    Terminal,
    /// Its log records a cancellation that no later transition or rewind
    /// follows.
    Cancelled,
    /// The header, the log or the template can't be read, or the session
    /// directory is a symlink.
    Unreadable,
}

impl SessionStatus {
    /// Terminal or cancelled.
    pub fn is_finished(self) -> bool {
        matches!(self, SessionStatus::Terminal | SessionStatus::Cancelled)
    }
}

/// What [`inspect`] learned about one session.
#[derive(Debug, Clone)]
pub struct Inspection {
    pub name: String,
    /// `None` for an unreadable header, or one written before the field
    /// existed (an empty `session_id`).
    pub session_id: Option<String>,
    pub execution_dir: Option<PathBuf>,
    pub parent_workflow: Option<String>,
    pub status: SessionStatus,
    /// The current state's name; `None` when it couldn't be derived.
    pub state: Option<String>,
    /// True when the current state is a terminal declared `failure: true`.
    pub failure: bool,
    /// The timestamp of the last event in the log.
    pub finished_at: Option<String>,
}

/// Compiled templates read so far, by path. A template that fails to read
/// or parse is remembered as `None`, so it isn't read again.
#[derive(Default)]
pub struct TemplateCache {
    templates: HashMap<String, Option<CompiledTemplate>>,
}

impl TemplateCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn get(&mut self, path: &str) -> Option<&CompiledTemplate> {
        self.templates
            .entry(path.to_string())
            .or_insert_with(|| {
                let bytes = std::fs::read(path).ok()?;
                serde_json::from_slice(&bytes).ok()
            })
            .as_ref()
    }
}

/// Inspect session `name` under the local store `base`.
pub fn inspect(base: &Path, name: &str, cache: &mut TemplateCache) -> Inspection {
    let unreadable = |session_id, execution_dir, parent_workflow| Inspection {
        name: name.to_string(),
        session_id,
        execution_dir,
        parent_workflow,
        status: SessionStatus::Unreadable,
        state: None,
        failure: false,
        finished_at: None,
    };

    let session_dir = base.join(name);
    match std::fs::symlink_metadata(&session_dir) {
        Ok(meta) if meta.file_type().is_dir() => {}
        _ => return unreadable(None, None, None),
    }
    let state_path = session_dir.join(state_file_name(name));

    let (header, events) = match persistence::read_events(&state_path) {
        Ok(read) => read,
        Err(_) => {
            // The log failed; the header alone may still name the session.
            return match persistence::read_header(&state_path) {
                Ok(h) => unreadable(non_empty(h.session_id), h.execution_dir, h.parent_workflow),
                Err(_) => unreadable(None, None, None),
            };
        }
    };
    let session_id = non_empty(header.session_id.clone());
    let execution_dir = header.execution_dir.clone();
    let parent_workflow = header.parent_workflow.clone();
    let finished_at = events.last().map(|e| e.timestamp.clone());

    let Some(machine) = persistence::derive_machine_state(&header, &events, &session_dir) else {
        return unreadable(session_id, execution_dir, parent_workflow);
    };
    let Some(compiled) = cache.get(&machine.template_path) else {
        return unreadable(session_id, execution_dir, parent_workflow);
    };
    let template_state = compiled.states.get(&machine.current_state);

    let status = if cancelled_last(&events) {
        SessionStatus::Cancelled
    } else if template_state.is_some_and(|s| s.terminal) {
        SessionStatus::Terminal
    } else {
        SessionStatus::Running
    };
    let failure = status == SessionStatus::Terminal && template_state.is_some_and(|s| s.failure);

    Inspection {
        name: name.to_string(),
        session_id,
        execution_dir,
        parent_workflow,
        status,
        state: Some(machine.current_state),
        failure,
        finished_at,
    }
}

/// True when the log holds a `workflow_cancelled` that no transition,
/// directed transition or rewind follows.
fn cancelled_last(events: &[crate::engine::types::Event]) -> bool {
    for event in events.iter().rev() {
        match event.payload {
            EventPayload::WorkflowCancelled { .. } => return true,
            EventPayload::Transitioned { .. }
            | EventPayload::DirectedTransition { .. }
            | EventPayload::Rewound { .. } => return false,
            _ => {}
        }
    }
    false
}

fn non_empty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Whether `recorded` is `filter` or lies under it, compared by whole path
/// components. `filter` is expected without trailing slashes; `/` matches
/// every absolute path.
pub fn under_dir(recorded: &Path, filter: &Path) -> bool {
    recorded.starts_with(filter)
}

/// Normalize an `--execution-dir` argument: canonicalized when it resolves on
/// this host, used as given otherwise. Trailing slashes are dropped by
/// `Path`'s component iteration, so `/a/b/` and `/a/b` compare the same.
pub fn normalize_filter(arg: &str) -> PathBuf {
    let given = PathBuf::from(arg);
    std::fs::canonicalize(&given).unwrap_or_else(|_| given.components().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_dir_compares_whole_components() {
        let f = Path::new("/x/a/b");
        assert!(under_dir(Path::new("/x/a/b"), f));
        assert!(under_dir(Path::new("/x/a/b/c"), f));
        assert!(!under_dir(Path::new("/x/a/bc"), f));
        assert!(!under_dir(Path::new("/x/a"), f));
        assert!(under_dir(Path::new("/anything"), Path::new("/")));
    }

    #[test]
    fn normalize_filter_drops_trailing_slashes_for_a_missing_dir() {
        assert_eq!(
            normalize_filter("/no/such/dir/for/koto/"),
            PathBuf::from("/no/such/dir/for/koto")
        );
    }

    #[test]
    fn a_missing_session_is_unreadable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let got = inspect(tmp.path(), "absent", &mut TemplateCache::new());
        assert_eq!(got.status, SessionStatus::Unreadable);
        assert!(!got.status.is_finished());
    }
}
