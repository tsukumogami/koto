//! Derive the minimal `/workflows` projection from koto's event log.
//!
//! This is the "reuse the read seam" half of the feature: the projection is a
//! derivation over the append-only log, not a second store. It reuses the same
//! pure helpers the dashboard's read seam uses -- `derive_state_from_log`,
//! `derive_machine_state`, `is_terminal_state`, `is_failed_state` (all in
//! `crate::engine::persistence`) -- so the rendered running/done/failed status
//! matches the dashboard's classification by construction.

use crate::engine::persistence::{
    derive_machine_state, derive_state_from_log, is_failed_state, is_terminal_state,
};
use crate::engine::types::EventPayload;
use crate::session::SessionBackend;

use super::contract::RenderStatus;

const LABEL_SEP: &str = " \u{b7} "; // " · "

/// The minimal per-session projection Feature 1 renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection {
    /// The session's stable init-time UUID.
    pub session_id: String,
    /// The session's workflow name.
    pub workflow: String,
    /// The derived display label (intent, else template·state, else name).
    pub display_name: String,
    /// The current state, or `None` if the session never advanced.
    pub current_state: Option<String>,
    /// Running / completed / failed.
    pub status: RenderStatus,
}

/// Derive the minimal projection for `session_id`, reading through `backend`.
///
/// Returns `None` only when the session's event log cannot be read (a
/// just-committed session always reads back). All classification is a pure
/// function of the log plus the compiled template.
pub fn derive_minimal_projection(
    backend: &dyn SessionBackend,
    session_id: &str,
) -> Option<Projection> {
    let (header, events) = backend.read_events(session_id).ok()?;

    let current_state = derive_state_from_log(&events);

    // Terminal detection reuses the dashboard's derivation: resolve the machine
    // state (current state + template path), then read the template's terminal
    // flag for that state.
    let session_dir = backend.session_dir(session_id);
    let is_terminal = match derive_machine_state(&header, &events, &session_dir) {
        Some(ms) => is_terminal_state(&ms.template_path, &ms.current_state),
        None => false,
    };

    let cancelled = events
        .iter()
        .any(|e| matches!(e.payload, EventPayload::WorkflowCancelled { .. }));

    let status = if is_terminal {
        if is_failed_state(current_state.as_deref()) {
            RenderStatus::Failed
        } else {
            RenderStatus::Completed
        }
    } else if cancelled {
        // A cancelled session renders terminal (abandoned) rather than a stuck
        // `running` spinner, even if its state is not template-terminal.
        RenderStatus::Failed
    } else {
        RenderStatus::Running
    };

    // The workflow name is the stable header identity; session_id is the UUID
    // (may be empty on pre-UUID legacy state files -- callers fall back).
    let workflow = header.workflow.clone();
    let display_name = derive_display_name(&header, current_state.as_deref(), &workflow);

    Some(Projection {
        session_id: header.session_id.clone(),
        workflow,
        display_name,
        current_state,
        status,
    })
}

/// Minimal display-label derivation, mirroring the dashboard's `derive_label`
/// rungs 1-3 without the `CachedSession` coupling: explicit intent, else
/// `template_name · current_state`, else `untitled (template_name)`, else the
/// bare workflow name.
fn derive_display_name(
    header: &crate::engine::types::StateFileHeader,
    current_state: Option<&str>,
    workflow: &str,
) -> String {
    if let Some(intent) = header.intent.as_deref() {
        if !intent.is_empty() {
            return intent.to_string();
        }
    }
    let template_name = header.template_name.as_deref().unwrap_or("");
    if !template_name.is_empty() {
        if let Some(cs) = current_state {
            if !cs.is_empty() {
                return format!("{template_name}{LABEL_SEP}{cs}");
            }
        }
        return format!("untitled ({template_name})");
    }
    workflow.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::StateFileHeader;

    fn header(intent: Option<&str>, template_name: Option<&str>) -> StateFileHeader {
        let mut h: StateFileHeader = serde_json::from_str(
            r#"{"schema_version":1,"workflow":"wf.name","template_hash":"h","created_at":"2026-01-01T00:00:00Z"}"#,
        )
        .expect("minimal header deserializes");
        h.intent = intent.map(str::to_string);
        h.template_name = template_name.map(str::to_string);
        h
    }

    #[test]
    fn label_prefers_intent() {
        let h = header(Some("fix the bug"), Some("tmpl"));
        assert_eq!(
            derive_display_name(&h, Some("building"), "wf.name"),
            "fix the bug"
        );
    }

    #[test]
    fn label_falls_back_to_template_and_state() {
        let h = header(None, Some("tmpl"));
        assert_eq!(
            derive_display_name(&h, Some("building"), "wf.name"),
            "tmpl \u{b7} building"
        );
    }

    #[test]
    fn label_falls_back_to_workflow_name() {
        let h = header(None, None);
        assert_eq!(
            derive_display_name(&h, Some("building"), "wf.name"),
            "wf.name"
        );
    }
}
