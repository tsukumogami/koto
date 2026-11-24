//! A session's row: the facts the dashboard shows for it, derived from its
//! header and event log by one pure function.
//!
//! The local dashboard reader (`read_session` in the CLI's dashboard data
//! module) and the summary a push publishes to the bucket
//! ([`crate::session::summary`]) both call [`derive_row`], so a row another
//! machine reads from a summary equals what the producing machine's own
//! dashboard derives from the log.

use std::collections::HashMap;
use std::path::Path;

use crate::engine::persistence::{
    derive_machine_state, derive_state_from_log, is_failed_state, is_terminal_state,
    latest_epoch_gate_failed,
};
use crate::engine::types::{
    effective_intent, producing_machine, Event, EventPayload, StateFileHeader,
};

/// Priority list of variable keys to surface as a session's salient var.
const SALIENT_KEYS: &[&str] = &["issue", "target", "name", "task", "query"];

/// Pick the most salient variable value from a `WorkflowInitialized`
/// variables map, in key-priority order. Returns `None` when no priority key
/// is present (or its value is empty).
pub fn salient_var(variables: &HashMap<String, String>) -> Option<String> {
    SALIENT_KEYS
        .iter()
        .filter_map(|key| variables.get(*key))
        .find(|value| !value.is_empty())
        .cloned()
}

/// The facts of one session the dashboard shows, read from its header and
/// events. Nothing here is read from the session's context store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionRow {
    /// Current state derived from the event log; `None` when the session
    /// never left its initial event.
    pub current_state: Option<String>,
    /// Whether the current state is a terminal state in the template.
    pub is_terminal: bool,
    /// Whether the session waits on a gate that hasn't passed in the
    /// current state's epoch. Always `false` for a terminal session.
    pub is_blocked: bool,
    /// Terminal in a failed or error state.
    pub failed: bool,
    /// The intent every surface shows ([`effective_intent`]).
    pub intent: Option<String>,
    /// The template name the header records.
    pub template_name: Option<String>,
    /// The most salient `WorkflowInitialized` variable ([`salient_var`]).
    pub salient_var: Option<String>,
    /// Spawned as a batch task: its initial event carries a spawn entry.
    pub is_batch_task: bool,
    /// Bound to a request leg (the pointer sidecar beside its log).
    pub is_request_leg: bool,
    /// The RFC 3339 timestamp of the log's last event.
    pub last_event_at: Option<String>,
    /// The parent workflow the header records.
    pub parent: Option<String>,
    /// The machine that produced the session ([`producing_machine`]).
    pub machine: Option<String>,
}

/// Derive a session's row from its header and events.
///
/// `session_dir` resolves a session-relative template path for the terminal
/// check; `is_request_leg` is the one fact that lives beside the log rather
/// than in it, which the caller reads. Pure apart from reading the compiled
/// template the events name.
pub fn derive_row(
    header: &StateFileHeader,
    events: &[Event],
    session_dir: &Path,
    is_request_leg: bool,
) -> SessionRow {
    let current_state = derive_state_from_log(events);

    let is_terminal = match derive_machine_state(header, events, session_dir) {
        Some(machine_state) => {
            is_terminal_state(&machine_state.template_path, &machine_state.current_state)
        }
        None => false,
    };

    // Blocked: a non-terminal session whose most recent gate evaluation in
    // the current epoch did not pass. The same predicate the `/workflows`
    // projection writer uses, so the two classify a session identically.
    let is_blocked = !is_terminal
        && current_state
            .as_ref()
            .map(|cs| latest_epoch_gate_failed(events, cs))
            .unwrap_or(false);

    let failed = is_terminal && is_failed_state(current_state.as_deref());

    let salient = events.iter().find_map(|e| match &e.payload {
        EventPayload::WorkflowInitialized { variables, .. } => salient_var(variables),
        _ => None,
    });

    let is_batch_task = events.iter().any(|e| {
        matches!(
            &e.payload,
            EventPayload::WorkflowInitialized {
                spawn_entry: Some(_),
                ..
            }
        )
    });

    SessionRow {
        current_state,
        is_terminal,
        is_blocked,
        failed,
        intent: effective_intent(header, events),
        template_name: header.template_name.clone(),
        salient_var: salient,
        is_batch_task,
        is_request_leg,
        last_event_at: events.last().map(|e| e.timestamp.clone()),
        parent: header.parent_workflow.clone(),
        machine: producing_machine(events),
    }
}

/// Where a session was imported from: the source named by the log's latest
/// `session_imported` event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRecord {
    /// The source's workspace (its execution anchor), as recorded.
    pub from_workspace: String,
    /// The source session's name in that workspace.
    pub from_session: String,
    /// The source session's `session_id`.
    pub from_session_id: String,
}

/// The source the log's latest `session_imported` event names; `None` for a
/// session that was never imported.
pub fn latest_import(events: &[Event]) -> Option<ImportRecord> {
    events.iter().rev().find_map(|e| match &e.payload {
        EventPayload::SessionImported {
            from_workspace,
            from_session,
            from_session_id,
            ..
        } => Some(ImportRecord {
            from_workspace: from_workspace.clone(),
            from_session: from_session.clone(),
            from_session_id: from_session_id.clone(),
        }),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(seq: u64, payload: EventPayload) -> Event {
        Event {
            seq,
            timestamp: "2026-10-31T00:00:00Z".to_string(),
            event_type: payload.type_name().to_string(),
            payload,
            idempotency_hash: None,
        }
    }

    fn imported(from: &str, id: &str) -> EventPayload {
        EventPayload::SessionImported {
            from_workspace: "/work/a".to_string(),
            from_session: from.to_string(),
            from_session_id: id.to_string(),
            machine_id: "m".to_string(),
            machine_name: None,
        }
    }

    #[test]
    fn latest_import_names_the_last_session_imported_source() {
        assert_eq!(latest_import(&[]), None);
        let events = [
            event(1, imported("first", "id-1")),
            event(2, imported("second", "id-2")),
        ];
        let record = latest_import(&events).unwrap();
        assert_eq!(record.from_session, "second");
        assert_eq!(record.from_session_id, "id-2");
        assert_eq!(record.from_workspace, "/work/a");
    }

    #[test]
    fn salient_var_takes_the_first_non_empty_key_in_priority_order() {
        let vars = HashMap::from([
            ("name".to_string(), "n".to_string()),
            ("issue".to_string(), String::new()),
            ("target".to_string(), "t".to_string()),
        ]);
        assert_eq!(salient_var(&vars).as_deref(), Some("t"));
        assert_eq!(salient_var(&HashMap::new()), None);
    }
}
