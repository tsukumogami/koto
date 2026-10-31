//! The extensible on-disk file contract koto writes into a Claude Code
//! session's `/workflows` directory.
//!
//! Claude Code's `/workflows` screen globs `*.json` in the session's workflows
//! directory, `JSON.parse`s each file, applies defaults to every field, sorts
//! by `startTime`, and renders each as a run entry (established empirically
//! against Claude Code v2.1.209; see
//! `docs/designs/current/DESIGN-native-workflows-render.md`). The shape here is a
//! minimal *valid* projection for the initial render (name, current state, running/done)
//! that later features add fields to without breaking the initial readers. The
//! koto-namespaced [`KotoBlock`] identifies the file as koto's and carries a
//! [`CONTRACT_VERSION`] that the future drift-guard/fixture anchors on.

use serde::Serialize;

/// Contract version of the koto-namespaced projection block. The future
/// drift-guard/fixture pins this; later features that add fields bump it.
///
/// Version 2 adds the additive `phases` and `workflowProgress`
/// fields and wrote a gate-blocked session's top-level `status` as `blocked`;
/// version 5 replaces that word (see below). Version 1's fields remain a valid
/// subset, so a reader that ignores the new fields still renders the initial
/// entry. Version 3 adds the `holdings` block. Version 4 adds the top-level
/// `workflowName` and `summary` fields the `/workflows` screen titles an entry
/// with; `name` stays, carrying the same title, for readers of earlier
/// versions.
///
/// Version 5 writes the top-level `status` in Claude Code's vocabulary (a
/// gate-blocked session reads `running`), adds `koto.status` with koto's own
/// classification, adds the optional `blocked` and `error` fields on a
/// `workflow_agent` step, and appends `· blocked on gate <name>` to a blocked
/// session's `summary`.
pub const CONTRACT_VERSION: u32 = 5;

/// Longest gate name, in raw characters, written into a step's `error` or a
/// blocked `summary` before the cut marker. The name is cut before it is
/// escaped, so a cut never splits an escape sequence.
pub const GATE_NAME_CAP: usize = 128;

/// koto's own classification of a session, written as `koto.status`.
///
/// It is not the screen's word: Claude Code knows no `blocked` status and
/// counts every status but `running` as completed, so the top-level `status`
/// is [`RenderStatus::screen_status`] instead.
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

impl RenderStatus {
    /// The top-level `status` word for this classification: a blocked
    /// session is still running as far as the screen is concerned.
    pub fn screen_status(self) -> ScreenStatus {
        match self {
            RenderStatus::Running | RenderStatus::Blocked => ScreenStatus::Running,
            RenderStatus::Completed => ScreenStatus::Completed,
            RenderStatus::Failed => ScreenStatus::Failed,
        }
    }
}

/// The top-level `status` of an entry, in the words Claude Code's
/// `/workflows` screen knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ScreenStatus {
    Running,
    Completed,
    Failed,
}

/// Longest escaped gate name, in characters, written before the cut marker.
///
/// [`GATE_NAME_CAP`] bounds the raw name, but one raw character can escape to
/// as many as nine (`\u{E007F}`), so a name of control characters would
/// otherwise grow the blocked summary's suffix past the fact cap. Four times
/// the raw cap is half the fact cap, so the summary's
/// `koto · <workflow> · <state>` prefix keeps about the other half, and an
/// ASCII name never reaches it.
pub const GATE_NAME_DISPLAY_CAP: usize = 4 * GATE_NAME_CAP;

/// A gate name as it is written into a step's `error`, a phase's outcome line
/// and a blocked `summary`: cut at [`GATE_NAME_CAP`] raw characters, escaped,
/// then bounded at [`GATE_NAME_DISPLAY_CAP`] escaped characters, with one cut
/// marker when either cut applied. The raw cut comes first, so neither cut
/// ever splits an escape sequence.
pub fn gate_name_display(name: &str) -> String {
    use crate::session_view::{sanitize_one_line, CUT_MARKER};
    let raw_cut = name.chars().count() > GATE_NAME_CAP;
    let kept: String = name.chars().take(GATE_NAME_CAP).collect();
    let escaped = sanitize_one_line(&kept).into_string();
    let (mut shown, display_cut) = cut_escaped(&escaped, GATE_NAME_DISPLAY_CAP);
    if raw_cut || display_cut {
        shown.push_str(CUT_MARKER);
    }
    shown
}

/// Cut already-escaped text to at most `cap` characters without splitting a
/// `\u{...}` escape; returns the kept text and whether anything was cut.
///
/// The sanitizer escapes a literal backslash that would read as `\u{`, so in
/// its output every `\u{` opens an escape: when the cut lands inside one, the
/// whole escape is dropped rather than left half-written.
pub(crate) fn cut_escaped(escaped: &str, cap: usize) -> (String, bool) {
    if escaped.chars().count() <= cap {
        return (escaped.to_string(), false);
    }
    // Byte offset of the cut, then the last escape that opens before it: when
    // that escape closes at or past the cut, the cut lands inside it (even
    // between its `\` and its `u{`), so it goes whole.
    let end = escaped
        .char_indices()
        .nth(cap)
        .map_or(escaped.len(), |(i, _)| i);
    let mut keep_to = end;
    if let Some((open, _)) = escaped
        .match_indices("\\u{")
        .take_while(|(i, _)| *i < end)
        .last()
    {
        let closes_at = escaped[open..].find('}').map(|j| open + j);
        if closes_at.is_none_or(|c| c >= end) {
            keep_to = open;
        }
    }
    (escaped[..keep_to].to_string(), true)
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
        /// Step state: `done` (completed phase), `progress` (active phase of a
        /// running session) or `error` (active phase of a blocked session).
        state: String,
        /// The phase's directive text (what the step asks for).
        #[serde(rename = "promptPreview", skip_serializing_if = "String::is_empty")]
        prompt_preview: String,
        /// The phase's produced outcome: evidence and/or gate result.
        #[serde(rename = "resultPreview", skip_serializing_if = "String::is_empty")]
        result_preview: String,
        /// Whether the step is blocked on a gate; Claude Code draws an
        /// `error` step with this set as Blocked. Omitted when false.
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        blocked: bool,
        /// The step's Outcome line on an `error` step. Omitted when empty.
        #[serde(skip_serializing_if = "String::is_empty")]
        error: String,
    },
}

/// One context key in the holdings summary: its name and recorded size.
#[derive(Debug, Clone, Serialize)]
pub struct HoldingKey {
    pub key: String,
    pub size: u64,
}

/// A compact account of what the session holds, folded from the context
/// events in the log the projection already reads (contract v3). Names and
/// sizes only — the file lives under the hosting application's directory,
/// and key content never belongs in it.
#[derive(Debug, Clone, Serialize)]
pub struct Holdings {
    /// Every key the log's last-writer-wins fold leaves present, valid
    /// names and invalid ones alike.
    #[serde(rename = "keyCount")]
    pub key_count: usize,
    /// Recorded sizes summed with saturating addition.
    #[serde(rename = "totalBytes")]
    pub total_bytes: u64,
    /// At most [`HOLDINGS_KEY_CAP`] grammar-valid key names with sizes, in
    /// lexicographic order; an invalid name is counted above but never
    /// listed.
    pub keys: Vec<HoldingKey>,
    /// Whether `keys` was cut at the cap.
    #[serde(rename = "keysTruncated")]
    pub keys_truncated: bool,
    /// The session's execution anchor, sanitized and capped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    /// The store origin kind (`local` or `cloud`), sanitized.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<String>,
}

/// Most keys a holdings summary lists by name.
pub const HOLDINGS_KEY_CAP: usize = 50;

/// Longest `holdings.store` value written, in characters, before the cut
/// marker.
pub const HOLDINGS_STORE_CAP: usize = 64;

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
    /// koto's own classification (contract v5): `running`, `blocked`,
    /// `completed` or `failed`.
    pub status: RenderStatus,
    /// The holdings summary (contract v3). `None` on the minimal shape,
    /// omitted from serialization so that render stays lean.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub holdings: Option<Holdings>,
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
    /// Human-readable entry name (the session's derived display label),
    /// sanitized and capped. Kept for readers of contract versions 1-3; the
    /// `/workflows` screen titles the entry from `workflow_name`.
    pub name: String,
    /// The entry's title on Claude Code's `/workflows` screen: the same
    /// sanitized label as `name`.
    ///
    /// Observed, not documented: the native workflow file Claude Code
    /// v2.1.296 writes titles a run with `workflowName` and describes it with
    /// `summary`, and an entry without `workflowName` is titled "Dynamic
    /// Workflow" whatever its `name` says. If titles regress after a Claude
    /// Code upgrade, compare against a fresh native file first.
    #[serde(rename = "workflowName")]
    pub workflow_name: String,
    /// One-line description under the title: `koto · <session> · <state>`,
    /// sanitized and capped like every printed fact. A gate-blocked session's
    /// summary ends `· blocked on gate <name>` (see
    /// [`WorkflowFile::with_blocked_gate`]), with the prefix cut, never the
    /// suffix, to stay within the cap.
    pub summary: String,
    /// Running / completed / failed, in the screen's vocabulary; koto's own
    /// classification is `koto.status`.
    pub status: ScreenStatus,
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
        use crate::session_view::{sanitize_capped, FACT_CAP};
        // The label is user content (it carries the session's intent) shown
        // in Claude Code's own screen, so it passes the shared sanitizer and
        // cap before either field is written.
        let title = sanitize_capped(&name, FACT_CAP).into_string();
        let summary = match current_state.as_deref() {
            Some(state) if !state.is_empty() => format!("koto \u{b7} {workflow} \u{b7} {state}"),
            _ => format!("koto \u{b7} {workflow}"),
        };
        WorkflowFile {
            id: entry_id(session_id),
            name: title.clone(),
            workflow_name: title,
            summary: sanitize_capped(&summary, FACT_CAP).into_string(),
            status: status.screen_status(),
            start_time,
            phases: Vec::new(),
            workflow_progress: Vec::new(),
            koto: KotoBlock {
                session_id: session_id.to_string(),
                workflow: workflow.to_string(),
                current_state,
                contract_version: CONTRACT_VERSION,
                status,
                holdings: None,
            },
        }
    }

    /// Rebuild the summary for a session blocked on `gate`:
    /// `koto · <workflow> · <state> · blocked on gate <name>`.
    ///
    /// The gate name is cut at [`GATE_NAME_CAP`] and escaped on its own, and
    /// the `koto · <workflow> · <state>` prefix is capped so the whole line
    /// stays within the fact cap: a long workflow or state name is what gets
    /// cut, never the "blocked on gate" suffix.
    pub fn with_blocked_gate(mut self, gate: &str) -> Self {
        use crate::session_view::{sanitize_capped, CUT_MARKER, FACT_CAP};
        let workflow = &self.koto.workflow;
        let prefix = match self.koto.current_state.as_deref() {
            Some(state) if !state.is_empty() => format!("koto \u{b7} {workflow} \u{b7} {state}"),
            _ => format!("koto \u{b7} {workflow}"),
        };
        let suffix = format!(" \u{b7} blocked on gate {}", gate_name_display(gate));
        let budget = FACT_CAP.saturating_sub(suffix.chars().count() + CUT_MARKER.chars().count());
        self.summary = format!("{}{suffix}", sanitize_capped(&prefix, budget));
        self
    }

    /// Attach the holdings summary (contract v3). Additive like
    /// [`WorkflowFile::with_detail`]: a file built without it carries no
    /// `holdings` member at all.
    pub fn with_holdings(mut self, holdings: Holdings) -> Self {
        self.koto.holdings = Some(holdings);
        self
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
    fn top_level_status_is_the_screen_word() {
        // Claude Code knows no `blocked`, and counts every status but
        // `running` as completed: a blocked session is written `running`.
        for (status, want) in [
            (RenderStatus::Running, "running"),
            (RenderStatus::Blocked, "running"),
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
    fn koto_status_carries_kotos_classification() {
        for (status, want) in [
            (RenderStatus::Running, "running"),
            (RenderStatus::Blocked, "blocked"),
            (RenderStatus::Completed, "completed"),
            (RenderStatus::Failed, "failed"),
        ] {
            let wf = WorkflowFile::new("s", "w", "n".to_string(), None, status, 0);
            let v: serde_json::Value =
                serde_json::from_slice(&wf.to_json_bytes().unwrap()).unwrap();
            assert_eq!(v["koto"]["status"], want);
        }
    }

    #[test]
    fn contract_version_is_five() {
        // Version 5 moved koto's classification to koto.status and added the
        // blocked step and summary; the guard/fixture anchors on it.
        assert_eq!(CONTRACT_VERSION, 5);
    }

    fn blocked_file(workflow: &str, state: &str, gate: &str) -> serde_json::Value {
        let wf = WorkflowFile::new(
            "s",
            workflow,
            "n".to_string(),
            Some(state.to_string()),
            RenderStatus::Blocked,
            0,
        )
        .with_blocked_gate(gate);
        serde_json::from_slice(&wf.to_json_bytes().unwrap()).unwrap()
    }

    #[test]
    fn a_blocked_summary_names_the_gate() {
        let v = blocked_file("legibility", "verify", "ready_flag");
        assert_eq!(
            v["summary"],
            "koto \u{b7} legibility \u{b7} verify \u{b7} blocked on gate ready_flag"
        );
    }

    #[test]
    fn a_control_character_in_the_gate_name_is_escaped_in_the_summary() {
        let v = blocked_file("wf", "verify", "g\u{1B}[31m\u{202E}x");
        let summary = v["summary"].as_str().unwrap();
        assert!(!summary.contains('\u{1B}') && !summary.contains('\u{202E}'));
        assert!(
            summary.ends_with(" \u{b7} blocked on gate g\\u{1B}[31m\\u{202E}x"),
            "{summary}"
        );
    }

    #[test]
    fn an_over_cap_gate_name_is_cut_and_keeps_the_suffix() {
        use crate::session_view::{CUT_MARKER, FACT_CAP};
        // Longer than the fact cap itself: cut at the gate-name cap.
        let gate = "g".repeat(FACT_CAP + 10);
        let v = blocked_file("wf", "verify", &gate);
        let summary = v["summary"].as_str().unwrap();
        let want_tail = format!(
            " \u{b7} blocked on gate {}{CUT_MARKER}",
            "g".repeat(GATE_NAME_CAP)
        );
        assert!(summary.ends_with(&want_tail), "{summary}");
        assert!(summary.starts_with("koto \u{b7} wf \u{b7} verify"));
        assert_eq!(
            gate_name_display(&gate),
            format!("{}{CUT_MARKER}", "g".repeat(GATE_NAME_CAP))
        );
    }

    #[test]
    fn a_long_workflow_name_is_cut_before_the_blocked_suffix() {
        use crate::session_view::{CUT_MARKER, FACT_CAP};
        let workflow = "w".repeat(FACT_CAP * 2);
        let v = blocked_file(&workflow, "verify", "ready_flag");
        let summary = v["summary"].as_str().unwrap();
        assert!(
            summary.ends_with(&format!("{CUT_MARKER} \u{b7} blocked on gate ready_flag")),
            "{summary}"
        );
        assert!(summary.chars().count() <= FACT_CAP, "{}", summary.len());
    }

    #[test]
    fn a_cut_never_splits_an_escape() {
        // Control characters straddling the cap: the name is cut raw, so
        // every escape in the output is whole.
        let gate = format!("{}\u{1B}\u{1B}", "a".repeat(GATE_NAME_CAP - 1));
        let shown = gate_name_display(&gate);
        assert!(shown.contains("\\u{1B}"), "{shown}");
        assert_eq!(shown.matches("\\u{1B}").count(), 1, "{shown}");
    }

    #[test]
    fn an_escaped_gate_name_is_bounded_and_keeps_the_summary_within_the_cap() {
        use crate::session_view::{CUT_MARKER, FACT_CAP};
        // Every raw character escapes to nine: the raw cut alone would leave
        // 1152 characters, more than the whole fact cap.
        let gate = "\u{E007F}".repeat(GATE_NAME_CAP);
        let shown = gate_name_display(&gate);
        assert!(shown.ends_with(CUT_MARKER), "{shown}");
        assert!(
            shown.chars().count() <= GATE_NAME_DISPLAY_CAP + CUT_MARKER.chars().count(),
            "{}",
            shown.chars().count()
        );
        let body = shown.strip_suffix(CUT_MARKER).unwrap();
        assert_eq!(body.matches("\\u{E007F}").count() * 9, body.chars().count());

        let v = blocked_file("wf", "verify", &gate);
        let summary = v["summary"].as_str().unwrap();
        assert!(summary.chars().count() <= FACT_CAP, "{}", summary.len());
        assert!(
            summary.starts_with("koto \u{b7} wf \u{b7} verify"),
            "{summary}"
        );
        assert!(
            summary.contains(" \u{b7} blocked on gate \\u{E007F}"),
            "{summary}"
        );
        assert!(summary.ends_with(CUT_MARKER), "{summary}");
    }

    #[test]
    fn cut_escaped_drops_a_split_escape_whole() {
        assert_eq!(cut_escaped("abc", 3), ("abc".to_string(), false));
        assert_eq!(cut_escaped("ab\\u{1B}c", 4), ("ab".to_string(), true));
        assert_eq!(
            cut_escaped("ab\\u{1B}cd", 9),
            ("ab\\u{1B}c".to_string(), true)
        );
        // A sanitized literal `\u{` (written `\u{5C}u{`) is not an open escape.
        assert_eq!(
            cut_escaped("\\u{5C}u{1Bxyz", 10),
            ("\\u{5C}u{1B".to_string(), true)
        );
    }

    #[test]
    fn a_blocked_step_serializes_blocked_and_error() {
        let progress = vec![ProgressNode::WorkflowAgent {
            index: 1,
            label: "Verify".to_string(),
            phase_index: 1,
            phase_title: "Verify".to_string(),
            state: "error".to_string(),
            prompt_preview: String::new(),
            result_preview: String::new(),
            blocked: true,
            error: "gate tests-pass: FAIL".to_string(),
        }];
        let wf = WorkflowFile::new("s", "w", "n".to_string(), None, RenderStatus::Blocked, 0)
            .with_detail(Vec::new(), progress);
        let v: serde_json::Value = serde_json::from_slice(&wf.to_json_bytes().unwrap()).unwrap();
        assert_eq!(v["workflowProgress"][0]["state"], "error");
        assert_eq!(v["workflowProgress"][0]["blocked"], true);
        assert_eq!(v["workflowProgress"][0]["error"], "gate tests-pass: FAIL");
    }

    #[test]
    fn title_and_summary_are_written_for_the_workflows_screen() {
        let f = WorkflowFile::new(
            "sid",
            "release-notes",
            "Draft the release notes".to_string(),
            Some("review".to_string()),
            RenderStatus::Running,
            1,
        );
        let v: serde_json::Value = serde_json::from_slice(&f.to_json_bytes().unwrap()).unwrap();
        assert_eq!(v["workflowName"], "Draft the release notes");
        assert_eq!(v["name"], "Draft the release notes");
        assert_eq!(v["summary"], "koto \u{b7} release-notes \u{b7} review");
    }

    #[test]
    fn a_hostile_title_is_sanitized_and_capped_in_every_title_field() {
        use crate::session_view::{CUT_MARKER, FACT_CAP};
        let hostile = format!(
            "evil\u{1B}[2J\u{1B}]0;title\u{7}\u{202E}flip{}",
            "x".repeat(FACT_CAP * 2)
        );
        let f = WorkflowFile::new(
            "sid",
            "wf",
            hostile,
            Some("s\u{1B}[31m".to_string()),
            RenderStatus::Running,
            1,
        );
        let v: serde_json::Value = serde_json::from_slice(&f.to_json_bytes().unwrap()).unwrap();
        for field in ["workflowName", "name", "summary"] {
            let text = v[field].as_str().unwrap();
            assert!(
                !text
                    .chars()
                    .any(|c| c == '\u{1B}' || c == '\u{7}' || c == '\u{202E}'),
                "{field} carries a raw control or bidi character: {text:?}"
            );
        }
        let title = v["workflowName"].as_str().unwrap();
        assert!(title.starts_with("evil\\u{1B}[2J"), "{title}");
        assert!(title.contains("\\u{202E}flip"), "{title}");
        assert!(title.ends_with(CUT_MARKER), "{title}");
        assert_eq!(title.chars().count(), FACT_CAP + CUT_MARKER.chars().count());
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
                blocked: false,
                error: String::new(),
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
        assert_eq!(v["koto"]["contractVersion"], CONTRACT_VERSION);
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
            blocked: false,
            error: String::new(),
        }];
        let wf = WorkflowFile::new("s", "w", "n".to_string(), None, RenderStatus::Running, 0)
            .with_detail(Vec::new(), progress);
        let v: serde_json::Value = serde_json::from_slice(&wf.to_json_bytes().unwrap()).unwrap();
        assert!(v["workflowProgress"][0].get("promptPreview").is_none());
        assert!(v["workflowProgress"][0].get("resultPreview").is_none());
        assert!(v["workflowProgress"][0].get("blocked").is_none());
        assert!(v["workflowProgress"][0].get("error").is_none());
        assert_eq!(v["workflowProgress"][0]["state"], "progress");
    }

    #[test]
    fn absent_current_state_serializes_null() {
        let wf = WorkflowFile::new("s", "w", "n".to_string(), None, RenderStatus::Running, 0);
        let v: serde_json::Value = serde_json::from_slice(&wf.to_json_bytes().unwrap()).unwrap();
        assert!(v["koto"]["currentState"].is_null());
    }
}
