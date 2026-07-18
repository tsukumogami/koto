//! The extensible on-disk file contract koto writes into a Claude Code
//! session's `/workflows` directory.
//!
//! Claude Code's `/workflows` screen globs `*.json` in the session's workflows
//! directory, `JSON.parse`s each file, applies defaults to every field, sorts
//! by `startTime`, and renders each as a run entry (established empirically
//! against Claude Code v2.1.209; see
//! `docs/designs/DESIGN-native-workflows-render.md`). The shape here is a
//! minimal *valid* projection for the initial render (name, current state, running/done)
//! that later features add fields to without breaking the initial readers. The
//! koto-namespaced [`KotoBlock`] identifies the file as koto's and carries a
//! [`CONTRACT_VERSION`] that the future drift-guard/fixture anchors on.

use serde::Serialize;

/// Contract version of the koto-namespaced projection block. The future
/// drift-guard/fixture pins this; later features that add fields bump it.
///
/// Version 2 adds the additive `phases` and `workflowProgress`
/// fields and the `blocked` render status. Version 1's fields remain a valid
/// subset, so a reader that ignores the new fields still renders the initial
/// entry.
pub const CONTRACT_VERSION: u32 = 2;

/// Render status mapped onto the `/workflows` vocabulary.
///
/// Version 1 emitted `running`/`completed`/`failed`; version 2 adds `blocked`
/// (koto's blocked-in-current-epoch), distinct from running and done. Stalled/
/// pending refinements remain later-slice scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RenderStatus {
    /// Non-terminal: the session is advancing.
    Running,
    /// Non-terminal, but the most recent current-epoch gate did not pass:
    /// the session is blocked waiting on the gate condition. Precedence is
    /// terminal (completed/failed) > blocked > running.
    Blocked,
    /// Terminal, not a failure.
    Completed,
    /// Terminal failure (state name matches the failure heuristic).
    Failed,
}

/// A single phase in the `/workflows` ordered phase list. Each koto template
/// state maps to one phase; `title` is the human-readable label and `detail`
/// is the subtitle line (for a completed phase, its evidence/gate outcome; for
/// the active phase, its progress marker).
#[derive(Debug, Clone, Serialize)]
pub struct Phase {
    /// Human-readable phase label (the humanized state name).
    pub title: String,
    /// Subtitle line: outcome for completed phases, progress marker otherwise.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

/// A node in the `/workflows` `workflowProgress` progress tree. The screen
/// renders `workflow_phase` nodes as phase headers (positionally marking the
/// ordered phases) and `workflow_agent` nodes as steps under a phase. For a
/// single non-hierarchical koto session, a `workflow_agent` step represents the
/// session working one phase (its directive as `promptPreview`, its
/// evidence/gate outcome as `resultPreview`) -- not a delegate session, which
/// a future hierarchy renders as its own separate entry.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProgressNode {
    /// A phase header in the progress tree.
    WorkflowPhase {
        /// 1-based phase index.
        index: u32,
        /// Human-readable phase label.
        title: String,
    },
    /// A step under a phase (the koto session working that phase).
    WorkflowAgent {
        /// 1-based step index.
        index: u32,
        /// Step label (the humanized state name).
        label: String,
        /// 1-based index of the phase this step belongs to.
        #[serde(rename = "phaseIndex")]
        phase_index: u32,
        /// The phase's human-readable label.
        #[serde(rename = "phaseTitle")]
        phase_title: String,
        /// Step state: `done` (completed phase) or `progress` (active phase).
        state: String,
        /// The phase's directive text (what the step asks for).
        #[serde(rename = "promptPreview", skip_serializing_if = "String::is_empty")]
        prompt_preview: String,
        /// The phase's produced outcome: evidence and/or gate result.
        #[serde(rename = "resultPreview", skip_serializing_if = "String::is_empty")]
        result_preview: String,
    },
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
    /// The session's phases in order. Empty on the minimal shape; omitted from
    /// serialization when empty so the initial render is byte-preserved.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub phases: Vec<Phase>,
    /// The progress tree: a `workflow_phase` marker per phase plus
    /// a `workflow_agent` step per visited/active phase. Omitted when empty.
    #[serde(rename = "workflowProgress", skip_serializing_if = "Vec::is_empty")]
    pub workflow_progress: Vec<ProgressNode>,
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
            phases: Vec::new(),
            workflow_progress: Vec::new(),
            koto: KotoBlock {
                session_id: session_id.to_string(),
                workflow: workflow.to_string(),
                current_state,
                contract_version: CONTRACT_VERSION,
            },
        }
    }

    /// Attach the enriched phase detail (ordered phases + progress tree).
    ///
    /// Additive over [`WorkflowFile::new`]: the minimal shape leaves
    /// both empty, and empty vectors are omitted from serialization, so a file
    /// built without this call is byte-identical to the initial render's.
    pub fn with_detail(mut self, phases: Vec<Phase>, workflow_progress: Vec<ProgressNode>) -> Self {
        self.phases = phases;
        self.workflow_progress = workflow_progress;
        self
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
            (RenderStatus::Blocked, "blocked"),
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
    fn contract_version_is_two() {
        // Version 2 bumped the contract; the guard/fixture anchors on it.
        assert_eq!(CONTRACT_VERSION, 2);
    }

    #[test]
    fn minimal_shape_omits_phase_fields() {
        // A file built with `new` (the minimal path) carries neither `phases`
        // nor `workflowProgress`, so it is byte-identical to the initial shape
        // and remains a valid subset for readers that ignore the new fields.
        let wf = WorkflowFile::new("s", "w", "n".to_string(), None, RenderStatus::Running, 0);
        let v: serde_json::Value = serde_json::from_slice(&wf.to_json_bytes().unwrap()).unwrap();
        assert!(v.get("phases").is_none());
        assert!(v.get("workflowProgress").is_none());
    }

    #[test]
    fn with_detail_emits_phases_and_progress() {
        let phases = vec![
            Phase {
                title: "Gather context".to_string(),
                detail: "gate build: PASS".to_string(),
            },
            Phase {
                title: "Implement".to_string(),
                detail: String::new(),
            },
        ];
        let progress = vec![
            ProgressNode::WorkflowPhase {
                index: 1,
                title: "Gather context".to_string(),
            },
            ProgressNode::WorkflowAgent {
                index: 1,
                label: "Gather context".to_string(),
                phase_index: 1,
                phase_title: "Gather context".to_string(),
                state: "done".to_string(),
                prompt_preview: "read the issue".to_string(),
                result_preview: "3 files identified".to_string(),
            },
            ProgressNode::WorkflowPhase {
                index: 2,
                title: "Implement".to_string(),
            },
        ];
        let wf = WorkflowFile::new(
            "sid",
            "wf",
            "wf".to_string(),
            Some("implement".to_string()),
            RenderStatus::Running,
            0,
        )
        .with_detail(phases, progress);
        let v: serde_json::Value = serde_json::from_slice(&wf.to_json_bytes().unwrap()).unwrap();

        // Ordered phases with a completed phase's outcome as detail.
        assert_eq!(v["phases"][0]["title"], "Gather context");
        assert_eq!(v["phases"][0]["detail"], "gate build: PASS");
        // Empty detail is omitted.
        assert!(v["phases"][1].get("detail").is_none());

        // Progress tree: both node types, tagged by `type`.
        assert_eq!(v["workflowProgress"][0]["type"], "workflow_phase");
        assert_eq!(v["workflowProgress"][0]["index"], 1);
        assert_eq!(v["workflowProgress"][1]["type"], "workflow_agent");
        assert_eq!(v["workflowProgress"][1]["phaseIndex"], 1);
        assert_eq!(v["workflowProgress"][1]["phaseTitle"], "Gather context");
        assert_eq!(v["workflowProgress"][1]["state"], "done");
        assert_eq!(v["workflowProgress"][1]["promptPreview"], "read the issue");
        assert_eq!(
            v["workflowProgress"][1]["resultPreview"],
            "3 files identified"
        );
        assert_eq!(v["workflowProgress"][2]["type"], "workflow_phase");

        // The initial fields are unchanged and present alongside the new ones.
        assert_eq!(v["id"], "koto-sid");
        assert_eq!(v["koto"]["contractVersion"], 2);
    }

    #[test]
    fn workflow_agent_omits_empty_previews() {
        let progress = vec![ProgressNode::WorkflowAgent {
            index: 1,
            label: "Verify".to_string(),
            phase_index: 3,
            phase_title: "Verify".to_string(),
            state: "progress".to_string(),
            prompt_preview: String::new(),
            result_preview: String::new(),
        }];
        let wf = WorkflowFile::new("s", "w", "n".to_string(), None, RenderStatus::Running, 0)
            .with_detail(Vec::new(), progress);
        let v: serde_json::Value = serde_json::from_slice(&wf.to_json_bytes().unwrap()).unwrap();
        assert!(v["workflowProgress"][0].get("promptPreview").is_none());
        assert!(v["workflowProgress"][0].get("resultPreview").is_none());
        assert_eq!(v["workflowProgress"][0]["state"], "progress");
    }

    #[test]
    fn absent_current_state_serializes_null() {
        let wf = WorkflowFile::new("s", "w", "n".to_string(), None, RenderStatus::Running, 0);
        let v: serde_json::Value = serde_json::from_slice(&wf.to_json_bytes().unwrap()).unwrap();
        assert!(v["koto"]["currentState"].is_null());
    }
}
