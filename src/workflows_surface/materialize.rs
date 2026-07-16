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

use super::contract::{workflow_filename, WorkflowFile};
use super::{discover, project};

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

    let proj = match project::derive_minimal_projection(backend, session_id) {
        Some(p) => p,
        None => return Ok(()),
    };

    // A UUID is required for a non-colliding filename; skip pre-UUID legacy
    // sessions (session_id empty) rather than write an ambiguous file.
    if proj.session_id.is_empty() {
        return Ok(());
    }

    let target = dir.join(workflow_filename(&proj.session_id));
    let start_time = stable_start_time(&target);
    let file = WorkflowFile::new(
        &proj.session_id,
        &proj.workflow,
        proj.display_name,
        proj.current_state,
        proj.status,
        start_time,
    );
    let bytes = file.to_json_bytes()?;
    atomic_write(&dir, &target, &bytes)
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
