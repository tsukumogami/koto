//! One read of a session's standing: what `koto session list --state`
//! reports about each session, and what the retention engine
//! ([`crate::session::hierarchy`]) inspects in full for the members of a
//! hierarchy it judges.
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
use crate::engine::types::{Event, EventPayload};
use crate::session::state_file_name;
use crate::template::types::CompiledTemplate;

/// Where a session stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
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

/// What [`inspect`] learned about one session.
#[derive(Debug, Clone)]
pub struct Inspection {
    pub status: SessionStatus,
    /// The current state's name; `None` when it couldn't be derived.
    pub state: Option<String>,
    /// True when the current state is a terminal declared `failure: true`.
    pub failure: bool,
    /// The timestamp of the last event in the log.
    pub finished_at: Option<String>,
    /// For a terminal session, the timestamp of the event that moved it
    /// into its terminal state; for a cancelled one, that of the trailing
    /// cancel. `None` while it runs or can't be read. Unlike
    /// `finished_at`, a later read or context write doesn't move it.
    pub stopped_at: Option<String>,
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

/// The header fields `koto session prune` reads alongside an [`Inspection`].
#[derive(Debug, Clone, Default)]
pub struct HeaderFields {
    /// The header's `session_id`; `None` when it is empty.
    pub session_id: Option<String>,
    pub execution_dir: Option<PathBuf>,
    pub parent_workflow: Option<String>,
}

/// Inspect session `name` under the local store `base`.
pub fn inspect(base: &Path, name: &str, cache: &mut TemplateCache) -> Inspection {
    inspect_full(base, name, cache).0
}

/// [`inspect`], plus the header fields when the header could be read. A
/// session whose header parses but whose log or template doesn't is still
/// `Unreadable`, and its header fields are still returned.
pub fn inspect_full(
    base: &Path,
    name: &str,
    cache: &mut TemplateCache,
) -> (Inspection, Option<HeaderFields>) {
    let unreadable = || Inspection {
        status: SessionStatus::Unreadable,
        state: None,
        failure: false,
        finished_at: None,
        stopped_at: None,
    };

    let session_dir = base.join(name);
    match std::fs::symlink_metadata(&session_dir) {
        Ok(meta) if meta.file_type().is_dir() => {}
        _ => return (unreadable(), None),
    }
    let state_path = session_dir.join(state_file_name(name));

    let Ok((header, events)) = persistence::read_events(&state_path) else {
        return (unreadable(), None);
    };
    let fields = HeaderFields {
        session_id: Some(header.session_id.clone()).filter(|id| !id.is_empty()),
        execution_dir: header.execution_dir.clone(),
        parent_workflow: header.parent_workflow.clone(),
    };
    let finished_at = events.last().map(|e| e.timestamp.clone());

    let Some(machine) = persistence::derive_machine_state(&header, &events, &session_dir) else {
        return (unreadable(), Some(fields));
    };
    let Some(compiled) = cache.get(&machine.template_path) else {
        return (unreadable(), Some(fields));
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
    let stopped_at = match status {
        SessionStatus::Cancelled => trailing_cancel(&events),
        SessionStatus::Terminal => last_state_change(&events),
        _ => None,
    }
    .map(|e| e.timestamp.clone());

    (
        Inspection {
            status,
            state: Some(machine.current_state),
            failure,
            finished_at,
            stopped_at,
        },
        Some(fields),
    )
}

/// True when the log holds a `workflow_cancelled` that no transition,
/// directed transition or rewind follows.
fn cancelled_last(events: &[Event]) -> bool {
    trailing_cancel(events).is_some()
}

/// The last event that is a cancel, a transition, a directed transition
/// or a rewind: the one that last set where the session stands.
fn last_standing_event(events: &[Event]) -> Option<&Event> {
    events.iter().rev().find(|e| {
        matches!(
            e.payload,
            EventPayload::WorkflowCancelled { .. }
                | EventPayload::Transitioned { .. }
                | EventPayload::DirectedTransition { .. }
                | EventPayload::Rewound { .. }
        )
    })
}

/// The `workflow_cancelled` no transition, directed transition or rewind
/// follows.
fn trailing_cancel(events: &[Event]) -> Option<&Event> {
    last_standing_event(events)
        .filter(|e| matches!(e.payload, EventPayload::WorkflowCancelled { .. }))
}

/// The transition, directed transition or rewind that moved the session
/// into the state it stands in.
fn last_state_change(events: &[Event]) -> Option<&Event> {
    last_standing_event(events)
        .filter(|e| !matches!(e.payload, EventPayload::WorkflowCancelled { .. }))
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

    fn event(seq: u64, payload: EventPayload) -> Event {
        Event {
            seq,
            timestamp: format!("2026-10-12T00:00:0{seq}Z"),
            event_type: String::new(),
            payload,
            idempotency_hash: None,
        }
    }

    #[test]
    fn a_cancel_counts_only_until_the_log_moves_on() {
        let cancel = || EventPayload::WorkflowCancelled {
            state: "start".to_string(),
            reason: "test".to_string(),
        };
        let rewind = || EventPayload::Rewound {
            from: "start".to_string(),
            to: "start".to_string(),
            rationale: None,
        };
        assert!(cancelled_last(&[event(1, cancel())]));
        assert!(!cancelled_last(&[event(1, cancel()), event(2, rewind())]));
        assert!(cancelled_last(&[event(1, rewind()), event(2, cancel())]));
        assert!(!cancelled_last(&[]));
    }

    #[test]
    fn the_stop_is_the_event_that_set_the_standing_not_the_last_event() {
        let to_done = || EventPayload::Transitioned {
            from: Some("work".to_string()),
            to: "done".to_string(),
            condition_type: "auto".to_string(),
            skip_if_matched: None,
            context_assignments: None,
            vars_matched: None,
        };
        let cancel = || EventPayload::WorkflowCancelled {
            state: "work".to_string(),
            reason: "test".to_string(),
        };
        let read = || EventPayload::ContextRead {
            key: "k".to_string(),
            reader: "cli".to_string(),
            state: "done".to_string(),
            present: false,
            hash: None,
            access: None,
            gate: None,
        };
        let events = [event(1, to_done()), event(2, read())];
        assert_eq!(last_state_change(&events).map(|e| e.seq), Some(1));
        assert!(trailing_cancel(&events).is_none());

        let events = [event(1, to_done()), event(2, cancel()), event(3, read())];
        assert_eq!(trailing_cancel(&events).map(|e| e.seq), Some(2));
        assert!(last_state_change(&events).is_none());
    }

    #[test]
    fn a_symlinked_session_directory_is_unreadable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(&real, tmp.path().join("linked")).unwrap();
        let got = inspect(tmp.path(), "linked", &mut TemplateCache::new());
        assert_eq!(got.status, SessionStatus::Unreadable);
    }

    #[test]
    fn a_missing_session_is_unreadable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let got = inspect(tmp.path(), "absent", &mut TemplateCache::new());
        assert_eq!(got.status, SessionStatus::Unreadable);
    }
}
