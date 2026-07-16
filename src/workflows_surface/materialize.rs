//! Materialize a session's `/workflows` artifact off the commit funnel.
//!
//! [`materialize_after_commit`] is called at the end of
//! `LocalBackend::append_event` -- the single low-level commit funnel every
//! state mutation passes through -- so it fires uniformly for `koto next`,
//! directed `--to`, `koto rewind`, and error/limit exits without instrumenting
//! individual commands. It is **best-effort**: it never fails the commit
//! (mirroring the cloud backend's swallow-on-side-effect discipline).
//!
//! Opt-in is the presence of a published location (or the `KOTO_WORKFLOWS_DIR`
//! host handoff). With neither, the function returns after a cheap probe,
//! writing nothing -- koto's default path is untouched.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::session::context::ContextStore;
use crate::session::SessionBackend;

use super::contract::{workflow_filename, Phase, ProgressNode, WorkflowFile};
use super::discover;
use super::project::{self, PhaseEntry, PhaseStatus, StateOutcome};

/// Max characters in a `promptPreview` / `resultPreview` line before it is
/// truncated with an ellipsis (keeps the entry legible, not a full dump).
const PREVIEW_MAX: usize = 240;

/// Environment variable through which a hosting Claude Code session's
/// `SessionStart` hook hands koto the session's `/workflows` directory.
pub const WORKFLOWS_DIR_ENV: &str = "KOTO_WORKFLOWS_DIR";

/// Materialize `session_id`'s `/workflows` artifact after a state-commit.
///
/// Best-effort: any failure is logged and swallowed so the commit itself never
/// fails. `backend` and `store` are the same `LocalBackend`, passed as its two
/// trait faces.
pub fn materialize_after_commit(
    backend: &dyn SessionBackend,
    store: &dyn ContextStore,
    session_id: &str,
) {
    if let Err(e) = try_materialize(backend, store, session_id) {
        eprintln!("warning: /workflows materialization skipped for {session_id}: {e}");
    }
}

fn try_materialize(
    backend: &dyn SessionBackend,
    store: &dyn ContextStore,
    session_id: &str,
) -> anyhow::Result<()> {
    // Opt-in gate: resolve the target directory, or return writing nothing.
    let dir = match resolve_target_dir(backend, store, session_id) {
        Some(d) => d,
        None => return Ok(()),
    };

    let enriched = match project::derive_enriched_projection(backend, session_id) {
        Some(p) => p,
        None => return Ok(()),
    };
    let proj = &enriched.base;

    // A UUID is required for a non-colliding filename; skip pre-UUID legacy
    // sessions (session_id empty) rather than write an ambiguous file.
    if proj.session_id.is_empty() {
        return Ok(());
    }

    let target = dir.join(workflow_filename(&proj.session_id));
    let start_time = stable_start_time(&target);
    let (phases, progress) = build_detail(&enriched.phases);
    let file = WorkflowFile::new(
        &proj.session_id,
        &proj.workflow,
        proj.display_name.clone(),
        proj.current_state.clone(),
        proj.status,
        start_time,
    )
    .with_detail(phases, progress);
    let bytes = file.to_json_bytes()?;
    atomic_write(&dir, &target, &bytes)
}

/// Map the enriched phase list onto the `/workflows` `phases` and
/// `workflowProgress` fields: one `Phase` per phase, a `workflow_phase` marker
/// per phase, and a `workflow_agent` step per visited/active phase (carrying
/// the phase's directive as `promptPreview` and its outcome as `resultPreview`).
fn build_detail(phases: &[PhaseEntry]) -> (Vec<Phase>, Vec<ProgressNode>) {
    let mut out_phases = Vec::with_capacity(phases.len());
    let mut progress = Vec::new();
    let mut step_index: u32 = 0;

    for (i, entry) in phases.iter().enumerate() {
        let phase_index = (i + 1) as u32;
        out_phases.push(Phase {
            title: entry.title.clone(),
            detail: phase_detail(entry),
        });
        progress.push(ProgressNode::WorkflowPhase {
            index: phase_index,
            title: entry.title.clone(),
        });
        // A step is emitted only for phases the session has entered (done) or
        // is in (active); upcoming phases render as a bare marker.
        let state = match entry.status {
            PhaseStatus::Done => "done",
            PhaseStatus::Active => "progress",
            PhaseStatus::Upcoming => continue,
        };
        step_index += 1;
        progress.push(ProgressNode::WorkflowAgent {
            index: step_index,
            label: entry.title.clone(),
            phase_index,
            phase_title: entry.title.clone(),
            state: state.to_string(),
            prompt_preview: preview(&entry.directive),
            result_preview: preview(&outcome_line(&entry.outcome)),
        });
    }

    (out_phases, progress)
}

/// The subtitle line for a phase in the `phases` array.
fn phase_detail(entry: &PhaseEntry) -> String {
    let line = outcome_line(&entry.outcome);
    match entry.status {
        PhaseStatus::Active if line.is_empty() => "in progress".to_string(),
        PhaseStatus::Done if line.is_empty() => "done".to_string(),
        PhaseStatus::Upcoming => String::new(),
        _ => line,
    }
}

/// A one-line summary of what a phase produced: its gate outcome if a gate was
/// evaluated, else its evidence field names, else empty.
fn outcome_line(outcome: &StateOutcome) -> String {
    if let Some(gate) = &outcome.gate {
        return format!(
            "gate {}: {}",
            gate.name,
            if gate.passed { "PASS" } else { "FAIL" }
        );
    }
    if !outcome.evidence.is_empty() {
        return evidence_summary(&outcome.evidence);
    }
    String::new()
}

/// Summarize submitted evidence by its field names, e.g.
/// `evidence: acknowledgement, diff`. Falls back to a plain marker when the
/// field-sets carry no keys.
fn evidence_summary(evidence: &[serde_json::Value]) -> String {
    let mut keys: Vec<String> = Vec::new();
    for entry in evidence {
        if let Some(obj) = entry.as_object() {
            for k in obj.keys() {
                if !keys.iter().any(|seen| seen == k) {
                    keys.push(k.clone());
                }
            }
        }
    }
    if keys.is_empty() {
        "evidence submitted".to_string()
    } else {
        format!("evidence: {}", keys.join(", "))
    }
}

/// Collapse a directive/outcome into a single-line, length-bounded preview.
fn preview(s: &str) -> String {
    let one_line: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= PREVIEW_MAX {
        return one_line;
    }
    let mut out: String = one_line.chars().take(PREVIEW_MAX).collect();
    out.push('\u{2026}'); // ellipsis
    out
}

/// Resolve the target `/workflows` directory, applying the opt-in gate.
///
/// If `KOTO_WORKFLOWS_DIR` is set (the host handoff), self-publish it into this
/// session's own context store -- so descendants discover it by the ancestor
/// walk -- and use it. Otherwise resolve it by walking to the nearest published
/// ancestor. `None` means no participation: write nothing.
fn resolve_target_dir(
    backend: &dyn SessionBackend,
    store: &dyn ContextStore,
    session_id: &str,
) -> Option<PathBuf> {
    if let Ok(env_dir) = std::env::var(WORKFLOWS_DIR_ENV) {
        let env_dir = env_dir.trim().to_string();
        if !env_dir.is_empty() {
            if !discover::has_published_location(store, session_id) {
                // Best-effort: self-publish so F3 descendants can discover it.
                let _ = discover::publish_location(store, session_id, &env_dir);
            }
            return Some(PathBuf::from(env_dir));
        }
    }
    discover::resolve_publish_location(backend, store, session_id)
}

/// A stable `startTime`: reuse the value already on disk (so the entry's start
/// does not jump on each re-write), else the current epoch-millis.
fn stable_start_time(target: &Path) -> u64 {
    if let Ok(bytes) = std::fs::read(target) {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            if let Some(n) = v.get("startTime").and_then(serde_json::Value::as_u64) {
                return n;
            }
        }
    }
    now_millis()
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Write `bytes` to `target` atomically (temp file in the same directory, then
/// rename), creating `dir` when absent, so a concurrent `/workflows` reopen
/// never observes a half-written file.
fn atomic_write(dir: &Path, target: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".koto-wf-")
        .suffix(".tmp")
        .tempfile_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(target)
        .map_err(|e| anyhow::anyhow!("persist /workflows file: {}", e.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::{Event, EventPayload, StateFileHeader};
    use crate::session::local::LocalBackend;
    use tempfile::TempDir;

    fn make_header(workflow: &str) -> StateFileHeader {
        let mut h: StateFileHeader = serde_json::from_str(
            r#"{"schema_version":1,"workflow":"x","template_hash":"h","created_at":"2026-01-01T00:00:00Z"}"#,
        )
        .expect("minimal header deserializes");
        h.workflow = workflow.to_string();
        h.session_id = format!("{workflow}-uuid");
        h
    }

    fn init_session(backend: &LocalBackend, workflow: &str) {
        let header = make_header(workflow);
        let init = Event {
            seq: 1,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: "workflow_initialized".to_string(),
            payload: EventPayload::WorkflowInitialized {
                template_path: "template.json".to_string(),
                variables: Default::default(),
                spawn_entry: None,
            },
            idempotency_hash: None,
        };
        backend
            .init_state_file(workflow, header, vec![init])
            .expect("init session");
    }

    fn transition(backend: &LocalBackend, id: &str, to: &str) {
        let payload = EventPayload::Transitioned {
            from: None,
            to: to.to_string(),
            condition_type: "evidence".to_string(),
            skip_if_matched: None,
        };
        backend
            .append_event(id, &payload, "2026-01-01T00:01:00Z")
            .expect("append transition");
    }

    /// A four-state linear template (gather_context -> implement -> verify ->
    /// review[terminal]) with a directive per state and a gate on `verify`.
    const MULTI_PHASE_TEMPLATE: &str = r#"{
        "format_version": 1,
        "name": "impl-feature",
        "version": "1.0",
        "initial_state": "gather_context",
        "states": {
            "gather_context": {
                "directive": "Read the issue and map the module.",
                "transitions": [{"target": "implement"}]
            },
            "implement": {
                "directive": "Implement the change per the gathered context.",
                "transitions": [{"target": "verify"}]
            },
            "verify": {
                "directive": "Run the test suite and submit the result.",
                "transitions": [{"target": "review"}],
                "gates": {"tests-pass": {"type": "command", "command": "cargo test"}}
            },
            "review": {
                "directive": "Review the change and finish.",
                "terminal": true
            }
        }
    }"#;

    /// Write a compiled template at the session's `template.json` (the path the
    /// init event references, resolved relative to the session dir).
    fn write_template(backend: &LocalBackend, id: &str, json: &str) {
        let path = backend.session_dir(id).join("template.json");
        std::fs::write(path, json).expect("write template");
    }

    fn submit_evidence(backend: &LocalBackend, id: &str, state: &str, key: &str, value: &str) {
        let mut fields = std::collections::HashMap::new();
        fields.insert(
            key.to_string(),
            serde_json::Value::String(value.to_string()),
        );
        let payload = EventPayload::EvidenceSubmitted {
            state: state.to_string(),
            fields,
            submitter_cwd: None,
        };
        backend
            .append_event(id, &payload, "2026-01-01T00:01:00Z")
            .expect("append evidence");
    }

    fn evaluate_gate(backend: &LocalBackend, id: &str, state: &str, gate: &str, outcome: &str) {
        let payload = EventPayload::GateEvaluated {
            state: state.to_string(),
            gate: gate.to_string(),
            output: serde_json::json!({}),
            outcome: outcome.to_string(),
            timestamp: "2026-01-01T00:01:00Z".to_string(),
        };
        backend
            .append_event(id, &payload, "2026-01-01T00:01:00Z")
            .expect("append gate");
    }

    /// AC1 + AC2: a multi-phase running session renders its phases in order with
    /// the active one marked, completed phases carrying their outcomes, and the
    /// active phase's directive legible.
    #[test]
    fn ac1_ac2_multi_phase_ordered_active_marked_with_detail() {
        let base = TempDir::new().unwrap();
        let wf = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(base.path().to_path_buf());
        init_session(&backend, "solo");
        write_template(&backend, "solo", MULTI_PHASE_TEMPLATE);
        discover::publish_location(&backend, "solo", wf.path().to_str().unwrap()).unwrap();

        // Drive through gather_context (with evidence) and implement, landing on
        // verify as the active (non-terminal) phase.
        submit_evidence(
            &backend,
            "solo",
            "gather_context",
            "files",
            "auth/provider.rs",
        );
        transition(&backend, "solo", "implement");
        transition(&backend, "solo", "verify");

        let v = read_file(wf.path(), "solo-uuid").expect("file written");

        // Non-terminal -> running.
        assert_eq!(v["status"], "running");

        // Phases in structural order.
        let titles: Vec<&str> = v["phases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["title"].as_str().unwrap())
            .collect();
        assert_eq!(
            titles,
            vec!["Gather context", "Implement", "Verify", "Review"]
        );

        // The progress tree marks every phase, in order.
        let progress = v["workflowProgress"].as_array().unwrap();
        let phase_markers: Vec<&str> = progress
            .iter()
            .filter(|n| n["type"] == "workflow_phase")
            .map(|n| n["title"].as_str().unwrap())
            .collect();
        assert_eq!(
            phase_markers,
            vec!["Gather context", "Implement", "Verify", "Review"]
        );

        // The active phase (verify) is a `progress` step carrying its directive.
        let active = progress
            .iter()
            .find(|n| n["type"] == "workflow_agent" && n["phaseTitle"] == "Verify")
            .expect("active verify step");
        assert_eq!(active["state"], "progress");
        assert!(active["promptPreview"]
            .as_str()
            .unwrap()
            .contains("Run the test suite"));

        // A completed phase (gather_context) is `done` and shows its evidence.
        let done = progress
            .iter()
            .find(|n| n["type"] == "workflow_agent" && n["phaseTitle"] == "Gather context")
            .expect("done gather step");
        assert_eq!(done["state"], "done");
        assert!(done["resultPreview"]
            .as_str()
            .unwrap()
            .contains("evidence: files"));

        // Upcoming phase (review) has a marker but no agent step.
        assert!(!progress
            .iter()
            .any(|n| n["type"] == "workflow_agent" && n["phaseTitle"] == "Review"));
    }

    /// AC3: a session whose latest current-epoch gate did not pass renders
    /// `blocked` -- not running, not done.
    #[test]
    fn ac3_gate_blocked_renders_blocked() {
        let base = TempDir::new().unwrap();
        let wf = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(base.path().to_path_buf());
        init_session(&backend, "solo");
        write_template(&backend, "solo", MULTI_PHASE_TEMPLATE);
        discover::publish_location(&backend, "solo", wf.path().to_str().unwrap()).unwrap();

        transition(&backend, "solo", "implement");
        transition(&backend, "solo", "verify");
        evaluate_gate(&backend, "solo", "verify", "tests-pass", "failed");

        let v = read_file(wf.path(), "solo-uuid").expect("file written");
        assert_eq!(v["status"], "blocked");

        // The active phase's outcome shows the failed gate.
        let verify_detail = v["phases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["title"] == "Verify")
            .unwrap()["detail"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(verify_detail, "gate tests-pass: FAIL");
    }

    /// A passed gate does not block: the session stays running (and the phase's
    /// outcome shows the PASS).
    #[test]
    fn passed_gate_is_not_blocked() {
        let base = TempDir::new().unwrap();
        let wf = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(base.path().to_path_buf());
        init_session(&backend, "solo");
        write_template(&backend, "solo", MULTI_PHASE_TEMPLATE);
        discover::publish_location(&backend, "solo", wf.path().to_str().unwrap()).unwrap();

        transition(&backend, "solo", "implement");
        transition(&backend, "solo", "verify");
        evaluate_gate(&backend, "solo", "verify", "tests-pass", "passed");

        let v = read_file(wf.path(), "solo-uuid").expect("file written");
        assert_eq!(v["status"], "running");
    }

    /// Reaching the terminal state renders `completed` and still carries the
    /// full ordered phase list.
    #[test]
    fn terminal_renders_completed_with_phases() {
        let base = TempDir::new().unwrap();
        let wf = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(base.path().to_path_buf());
        init_session(&backend, "solo");
        write_template(&backend, "solo", MULTI_PHASE_TEMPLATE);
        discover::publish_location(&backend, "solo", wf.path().to_str().unwrap()).unwrap();

        transition(&backend, "solo", "implement");
        transition(&backend, "solo", "verify");
        evaluate_gate(&backend, "solo", "verify", "tests-pass", "passed");
        transition(&backend, "solo", "review");

        let v = read_file(wf.path(), "solo-uuid").expect("file written");
        assert_eq!(v["status"], "completed");
        assert_eq!(v["phases"].as_array().unwrap().len(), 4);
        // The terminal phase is the active one now.
        let review = v["workflowProgress"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["type"] == "workflow_agent" && n["phaseTitle"] == "Review")
            .expect("review step");
        assert_eq!(review["state"], "progress");
    }

    fn read_file(dir: &Path, uuid: &str) -> Option<serde_json::Value> {
        let path = dir.join(workflow_filename(uuid));
        let bytes = std::fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    #[test]
    fn ac1_file_appears_with_current_state() {
        let base = TempDir::new().unwrap();
        let wf = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(base.path().to_path_buf());
        init_session(&backend, "solo");
        discover::publish_location(&backend, "solo", wf.path().to_str().unwrap()).unwrap();

        transition(&backend, "solo", "building");

        let v = read_file(wf.path(), "solo-uuid").expect("file written");
        assert_eq!(v["id"], "koto-solo-uuid");
        assert_eq!(v["koto"]["currentState"], "building");
        assert_eq!(v["status"], "running");
    }

    #[test]
    fn ac2_file_updates_on_advance() {
        let base = TempDir::new().unwrap();
        let wf = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(base.path().to_path_buf());
        init_session(&backend, "solo");
        discover::publish_location(&backend, "solo", wf.path().to_str().unwrap()).unwrap();

        transition(&backend, "solo", "building");
        assert_eq!(
            read_file(wf.path(), "solo-uuid").unwrap()["koto"]["currentState"],
            "building"
        );

        transition(&backend, "solo", "review");
        assert_eq!(
            read_file(wf.path(), "solo-uuid").unwrap()["koto"]["currentState"],
            "review"
        );
    }

    #[test]
    fn ac4_no_write_without_published_location() {
        let base = TempDir::new().unwrap();
        let wf = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(base.path().to_path_buf());
        init_session(&backend, "solo");
        // No publish_location call, and KOTO_WORKFLOWS_DIR is unset in this test.

        transition(&backend, "solo", "building");

        // Nothing materialized; the /workflows dir is untouched (empty).
        assert!(read_file(wf.path(), "solo-uuid").is_none());
        let entries: Vec<_> = std::fs::read_dir(wf.path()).unwrap().collect();
        assert!(entries.is_empty(), "no koto-*.json written");
    }

    #[test]
    fn start_time_is_stable_across_rewrites() {
        let base = TempDir::new().unwrap();
        let wf = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(base.path().to_path_buf());
        init_session(&backend, "solo");
        discover::publish_location(&backend, "solo", wf.path().to_str().unwrap()).unwrap();

        transition(&backend, "solo", "building");
        let first = read_file(wf.path(), "solo-uuid").unwrap()["startTime"]
            .as_u64()
            .unwrap();
        transition(&backend, "solo", "review");
        let second = read_file(wf.path(), "solo-uuid").unwrap()["startTime"]
            .as_u64()
            .unwrap();
        assert_eq!(first, second, "startTime is preserved across rewrites");
    }

    #[test]
    fn creates_absent_target_directory() {
        let base = TempDir::new().unwrap();
        let wf_parent = TempDir::new().unwrap();
        let nested = wf_parent.path().join("does/not/exist/workflows");
        let backend = LocalBackend::with_base_dir(base.path().to_path_buf());
        init_session(&backend, "solo");
        discover::publish_location(&backend, "solo", nested.to_str().unwrap()).unwrap();

        transition(&backend, "solo", "building");

        assert!(nested.join(workflow_filename("solo-uuid")).exists());
    }
}
