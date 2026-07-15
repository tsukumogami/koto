//! The extensible on-disk file contract koto writes into a Claude Code
//! session's `/workflows` directory.
//!
//! Claude Code's `/workflows` screen globs `*.json` in the session's workflows
//! directory, `JSON.parse`s each file, applies defaults to every field, sorts
//! by `startTime`, and renders each as a run entry (established empirically
//! against Claude Code v2.1.209; see
//! `docs/designs/DESIGN-native-workflows-render.md`). The shape here is a
//! minimal *valid* projection for Feature 1 (name, current state, running/done)
//! that later features add fields to without breaking F1 readers. The
//! koto-namespaced [`KotoBlock`] identifies the file as koto's and carries a
//! [`CONTRACT_VERSION`] that Feature 4's guard/fixture anchors on.

use serde::Serialize;

/// Contract version of the koto-namespaced projection block. Feature 4's
/// guard/fixture pins this; later features that add fields bump it.
pub const CONTRACT_VERSION: u32 = 1;

/// Render status mapped onto the `/workflows` vocabulary.
///
/// Feature 1 emits only these three; blocked/stalled/pending refinements are
/// Feature 2/5 scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RenderStatus {
    /// Non-terminal: the session is advancing.
    Running,
    /// Terminal, not a failure.
    Completed,
    /// Terminal failure (state name matches the failure heuristic).
    Failed,
}

/// koto-namespaced identification and extension block. Nested under the `koto`
/// key so it never clashes with Claude Code's own top-level fields.
#[derive(Debug, Clone, Serialize)]
pub struct KotoBlock {
    /// The session's stable init-time UUID (`StateFileHeader.session_id`).
    #[serde(rename = "sessionId")]
    pub session_id: String,
    /// The session's workflow name.
    pub workflow: String,
    /// The current state, or `None` if the session never advanced.
    #[serde(rename = "currentState")]
    pub current_state: Option<String>,
    /// The file-contract version (see [`CONTRACT_VERSION`]).
    #[serde(rename = "contractVersion")]
    pub contract_version: u32,
}

/// The file koto writes as `koto-<session-uuid>.json`.
///
/// Top-level fields are the ones Claude Code's `/workflows` renders (id, name,
/// status, startTime); the `koto` block is koto's extension surface.
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowFile {
    /// Entry id, koto-namespaced so it cannot collide with Claude Code's own
    /// `wf_*` ids: `koto-<session-uuid>`.
    pub id: String,
    /// Human-readable entry name (the session's derived display label).
    pub name: String,
    /// Running / completed / failed.
    pub status: RenderStatus,
    /// Start time in epoch milliseconds; Claude Code sorts entries by this.
    #[serde(rename = "startTime")]
    pub start_time: u64,
    /// koto's namespaced identification + extension block.
    pub koto: KotoBlock,
}

impl WorkflowFile {
    /// Build a `WorkflowFile` from a derived projection.
    ///
    /// `session_id` is the stable UUID used for both the entry id and the
    /// filename; `start_time` is epoch milliseconds.
    pub fn new(
        session_id: &str,
        workflow: &str,
        name: String,
        current_state: Option<String>,
        status: RenderStatus,
        start_time: u64,
    ) -> Self {
        WorkflowFile {
            id: entry_id(session_id),
            name,
            status,
            start_time,
            koto: KotoBlock {
                session_id: session_id.to_string(),
                workflow: workflow.to_string(),
                current_state,
                contract_version: CONTRACT_VERSION,
            },
        }
    }

    /// Serialize to pretty JSON bytes (the on-disk form).
    pub fn to_json_bytes(&self) -> serde_json::Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

/// The koto-namespaced entry id for a session: `koto-<session-uuid>`.
pub fn entry_id(session_id: &str) -> String {
    format!("koto-{session_id}")
}

/// The filename koto writes for a session: `koto-<session-uuid>.json`.
///
/// UUID-keyed so it never collides with Claude Code's own `wf_*.json` files in
/// the same directory and is identifiable as koto's.
pub fn workflow_filename(session_id: &str) -> String {
    format!("koto-{session_id}.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_and_id_are_uuid_namespaced() {
        let sid = "11111111-2222-4333-8444-555555555555";
        assert_eq!(workflow_filename(sid), format!("koto-{sid}.json"));
        assert_eq!(entry_id(sid), format!("koto-{sid}"));
        // Never collides with Claude Code's own wf_*.json convention.
        assert!(!workflow_filename(sid).starts_with("wf_"));
        assert!(!workflow_filename(sid).starts_with("wf-"));
    }

    #[test]
    fn serialized_shape_has_expected_keys() {
        let wf = WorkflowFile::new(
            "abc-uuid",
            "my.workflow",
            "my.workflow · building".to_string(),
            Some("building".to_string()),
            RenderStatus::Running,
            1_700_000_000_000,
        );
        let v: serde_json::Value = serde_json::from_slice(&wf.to_json_bytes().unwrap()).unwrap();
        assert_eq!(v["id"], "koto-abc-uuid");
        assert_eq!(v["name"], "my.workflow · building");
        assert_eq!(v["status"], "running");
        assert_eq!(v["startTime"], 1_700_000_000_000u64);
        assert_eq!(v["koto"]["sessionId"], "abc-uuid");
        assert_eq!(v["koto"]["workflow"], "my.workflow");
        assert_eq!(v["koto"]["currentState"], "building");
        assert_eq!(v["koto"]["contractVersion"], CONTRACT_VERSION);
    }

    #[test]
    fn status_serializes_lowercase() {
        for (status, want) in [
            (RenderStatus::Running, "running"),
            (RenderStatus::Completed, "completed"),
            (RenderStatus::Failed, "failed"),
        ] {
            let wf = WorkflowFile::new("s", "w", "n".to_string(), None, status, 0);
            let v: serde_json::Value =
                serde_json::from_slice(&wf.to_json_bytes().unwrap()).unwrap();
            assert_eq!(v["status"], want);
        }
    }

    #[test]
    fn absent_current_state_serializes_null() {
        let wf = WorkflowFile::new("s", "w", "n".to_string(), None, RenderStatus::Running, 0);
        let v: serde_json::Value = serde_json::from_slice(&wf.to_json_bytes().unwrap()).unwrap();
        assert!(v["koto"]["currentState"].is_null());
    }
}
