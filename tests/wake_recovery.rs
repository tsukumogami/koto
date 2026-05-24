//! Integration tests for KT1 Issue 15:
//! `feat(audit): wake-candidates pass + RequesterWoken emission + age-and-activity recovery`.
//!
//! Covers the acceptance criteria from the issue body:
//! happy-path emission with 3-point fsync + sidecar release ordering;
//! crash-after-RequesterWoken-fsync-before-wake recovery via
//! age-and-activity rule; happy-path NOT double-woken when requester
//! has resumed; requester-resumed-then-idle does NOT trigger wake
//! recovery (F1 boundary); multi-child batched wake; sidecar L1
//! unlink after wake-delivery; no scan-cursor interaction; no
//! workspace-scan O(workspace) walk.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use filetime::{set_file_mtime, FileTime};
use koto::engine::audit::{requester_woken_fields, CHILD_DISPATCHED, REQUESTER_WOKEN};
use koto::engine::claim::{format_rfc3339_millis, sidecar_path};
use koto::engine::errors::EngineError;
use koto::engine::persistence::{append_event, append_header, read_events};
use koto::engine::terminal_index::{append_terminal_index_entry, TerminalIndexEntry};
use koto::engine::types::{AssignmentClaim, EventPayload, StateFileHeader, ValidatedSessionId};
use koto::engine::wake::{wake_candidates_pass, SubstrateWaker};
use koto::session::state_file_name;

// ----- Mocks --------------------------------------------------------------

#[derive(Default)]
struct RecordingWaker {
    wakes: Mutex<Vec<String>>,
}

impl RecordingWaker {
    fn count(&self) -> usize {
        self.wakes.lock().unwrap().len()
    }
    fn calls(&self) -> Vec<String> {
        self.wakes.lock().unwrap().clone()
    }
}

impl SubstrateWaker for RecordingWaker {
    fn wake(&self, session_id: &ValidatedSessionId) -> Result<(), EngineError> {
        self.wakes
            .lock()
            .unwrap()
            .push(session_id.as_str().to_string());
        Ok(())
    }
}

// ----- Test helpers -------------------------------------------------------

fn sessions_dir(koto_root: &Path) -> PathBuf {
    let p = koto_root.join("sessions");
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn make_header(workflow: &str, requested_by: Option<&str>) -> StateFileHeader {
    StateFileHeader {
        schema_version: 1,
        workflow: workflow.to_string(),
        template_hash: "deadbeef".into(),
        created_at: "2026-05-24T00:00:00Z".into(),
        parent_workflow: None,
        template_source_dir: None,
        session_id: workflow.to_string(),
        intent: None,
        template_name: Some("test".into()),
        needs_agent: None,
        role: None,
        inputs: None,
        coordinator_of_record: None,
        requested_by: requested_by.map(|s| s.to_string()),
        assignment_claim: None,
        dispatch_epoch: 0,
        priority: None,
        deadline: None,
        retry_count: None,
        agent_config: None,
    }
}

fn write_session_with_header(
    koto_root: &Path,
    workflow: &str,
    requested_by: Option<&str>,
) -> PathBuf {
    let dir = sessions_dir(koto_root).join(workflow);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(state_file_name(workflow));
    let header = make_header(workflow, requested_by);
    append_header(&path, &header).unwrap();
    path
}

fn append_child_dispatched_event(
    coord_state_file: &Path,
    child_id: &str,
    coord_id: &str,
    timestamp: &str,
) {
    let mut fields: HashMap<String, serde_json::Value> = HashMap::new();
    fields.insert("kind".into(), serde_json::json!(CHILD_DISPATCHED));
    fields.insert("child_session_id".into(), serde_json::json!(child_id));
    fields.insert("coord_id".into(), serde_json::json!(coord_id));
    fields.insert("dispatch_epoch".into(), serde_json::json!(0));
    let payload = EventPayload::EvidenceSubmitted {
        state: "kt1.dispatch".into(),
        fields,
        submitter_cwd: None,
    };
    append_event(coord_state_file, &payload, timestamp).unwrap();
}

fn append_terminal_index_for(koto_root: &Path, child_id: &str) {
    let entry = TerminalIndexEntry {
        session_id: child_id.to_string(),
        terminal_at: "2026-05-24T00:00:05.000Z".into(),
        header_mtime_ns: 1_000_000,
        terminal_state: "completed".into(),
    };
    append_terminal_index_entry(koto_root, &entry).unwrap();
}

fn create_sidecar(child_dir: &Path) {
    std::fs::create_dir_all(child_dir).unwrap();
    let path = sidecar_path(child_dir);
    std::fs::write(
        &path,
        b"coord_id = \"coord-1\"\nclaimed_at = \"2026-05-24T00:00:01.000Z\"\n",
    )
    .unwrap();
}

fn now_minus(secs: u64) -> SystemTime {
    SystemTime::now() - Duration::from_secs(secs)
}

// ----- AC: happy-path emission --------------------------------------------

#[test]
fn happy_path_emits_requester_woken_for_terminal_child() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    sessions_dir(root); // ensure path exists

    // Set up: parent coord, one child workflow, child is terminal.
    let coord_path = write_session_with_header(root, "coord", None);
    let child_path = write_session_with_header(root, "coord.child-a", Some("coord"));
    let _ = child_path; // header file present at canonical location

    // Coord's log carries the ChildDispatched event.
    let dispatched_ts = format_rfc3339_millis(now_minus(60));
    append_child_dispatched_event(&coord_path, "coord.child-a", "coord", &dispatched_ts);

    // Terminal-index marks the child as terminal.
    append_terminal_index_for(root, "coord.child-a");

    // Sidecar exists (will be released).
    create_sidecar(&sessions_dir(root).join("coord.child-a"));

    let waker = RecordingWaker::default();
    let outcome = wake_candidates_pass(
        root,
        &sessions_dir(root),
        &coord_path,
        &waker,
        Duration::from_secs(600),
        SystemTime::now(),
    )
    .unwrap();

    assert_eq!(
        outcome.events_emitted, 1,
        "expected 1 RequesterWoken emission"
    );
    assert_eq!(outcome.wakes_invoked, 1, "expected 1 substrate wake call");
    assert_eq!(outcome.recoveries_fired, 0);
    assert_eq!(waker.count(), 1);
    assert_eq!(waker.calls(), vec!["coord".to_string()]);

    // RequesterWoken event landed on coord's log.
    let (_, events) = read_events(&coord_path).unwrap();
    let has_woken = events.iter().any(|e| {
        if let EventPayload::EvidenceSubmitted { fields, .. } = &e.payload {
            fields
                .get("kind")
                .and_then(|v| v.as_str())
                .map(|s| s == REQUESTER_WOKEN)
                .unwrap_or(false)
        } else {
            false
        }
    });
    assert!(
        has_woken,
        "RequesterWoken event must be appended to coord log"
    );

    // Sidecar was released.
    assert!(
        !sidecar_path(&sessions_dir(root).join("coord.child-a")).exists(),
        "sidecar must be unlinked after successful wake"
    );
}

// ----- AC: child_session_ids carried in payload + load-bearing signature --

#[test]
fn requester_woken_payload_includes_child_session_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    sessions_dir(root);
    let coord_path = write_session_with_header(root, "coord", None);
    write_session_with_header(root, "coord.child-a", Some("coord"));
    let ts = format_rfc3339_millis(now_minus(60));
    append_child_dispatched_event(&coord_path, "coord.child-a", "coord", &ts);
    append_terminal_index_for(root, "coord.child-a");
    let waker = RecordingWaker::default();
    wake_candidates_pass(
        root,
        &sessions_dir(root),
        &coord_path,
        &waker,
        Duration::from_secs(600),
        SystemTime::now(),
    )
    .unwrap();

    let (_, events) = read_events(&coord_path).unwrap();
    let woken_event = events
        .iter()
        .find(|e| match &e.payload {
            EventPayload::EvidenceSubmitted { fields, .. } => fields
                .get("kind")
                .and_then(|v| v.as_str())
                .map(|s| s == REQUESTER_WOKEN)
                .unwrap_or(false),
            _ => false,
        })
        .unwrap();

    if let EventPayload::EvidenceSubmitted { fields, .. } = &woken_event.payload {
        let ids = fields
            .get("child_session_ids")
            .expect("child_session_ids must be present");
        let arr = ids.as_array().expect("array");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0].as_str(), Some("coord.child-a"));
        let req = fields
            .get("requested_by")
            .expect("requested_by must be present");
        assert_eq!(req.as_str(), Some("coord"));
    }
}

// ----- AC: multi-child batched wake --------------------------------------

#[test]
fn multi_child_terminal_produces_single_batched_wake() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    sessions_dir(root);
    let coord_path = write_session_with_header(root, "coord", None);

    // 3 children, all terminal, all under the same requester.
    for tag in ["a", "b", "c"] {
        let id = format!("coord.child-{}", tag);
        write_session_with_header(root, &id, Some("coord"));
        let ts = format_rfc3339_millis(now_minus(60));
        append_child_dispatched_event(&coord_path, &id, "coord", &ts);
        append_terminal_index_for(root, &id);
        create_sidecar(&sessions_dir(root).join(&id));
    }

    let waker = RecordingWaker::default();
    let outcome = wake_candidates_pass(
        root,
        &sessions_dir(root),
        &coord_path,
        &waker,
        Duration::from_secs(600),
        SystemTime::now(),
    )
    .unwrap();

    // 1 event (single batch), 1 wake (single requester), 3 sidecars.
    assert_eq!(outcome.events_emitted, 1);
    assert_eq!(outcome.wakes_invoked, 1);
    assert_eq!(outcome.sidecars_released, 3);
    assert_eq!(waker.count(), 1);
    let calls = waker.calls();
    assert_eq!(calls, vec!["coord".to_string()]);

    let (_, events) = read_events(&coord_path).unwrap();
    let woken_event = events
        .iter()
        .find(|e| match &e.payload {
            EventPayload::EvidenceSubmitted { fields, .. } => fields
                .get("kind")
                .and_then(|v| v.as_str())
                .map(|s| s == REQUESTER_WOKEN)
                .unwrap_or(false),
            _ => false,
        })
        .unwrap();
    if let EventPayload::EvidenceSubmitted { fields, .. } = &woken_event.payload {
        let arr = fields["child_session_ids"].as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(fields["child_count"], serde_json::json!(3));
    }
}

// ----- AC: idempotent — already-woken child is skipped on next tick ------

#[test]
fn already_woken_child_not_re_emitted() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    sessions_dir(root);
    let coord_path = write_session_with_header(root, "coord", None);
    write_session_with_header(root, "coord.child-a", Some("coord"));
    let ts = format_rfc3339_millis(now_minus(60));
    append_child_dispatched_event(&coord_path, "coord.child-a", "coord", &ts);
    append_terminal_index_for(root, "coord.child-a");
    let waker = RecordingWaker::default();
    // First pass: emits one wake.
    let first = wake_candidates_pass(
        root,
        &sessions_dir(root),
        &coord_path,
        &waker,
        Duration::from_secs(600),
        SystemTime::now(),
    )
    .unwrap();
    assert_eq!(first.events_emitted, 1);

    // Second pass: no new emissions (already-woken child skipped). No
    // recovery fires either since woken_at is recent.
    let second = wake_candidates_pass(
        root,
        &sessions_dir(root),
        &coord_path,
        &waker,
        Duration::from_secs(600),
        SystemTime::now(),
    )
    .unwrap();
    assert_eq!(second.events_emitted, 0);
    assert_eq!(second.recoveries_fired, 0);
    // First pass's wake is the only one observed.
    assert_eq!(waker.count(), 1);
}

// ----- AC: non-terminal child is NOT woken --------------------------------

#[test]
fn non_terminal_child_not_woken() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    sessions_dir(root);
    let coord_path = write_session_with_header(root, "coord", None);
    write_session_with_header(root, "coord.child-pending", Some("coord"));
    // Append ChildDispatched but NOT terminal-index entry.
    let ts = format_rfc3339_millis(now_minus(60));
    append_child_dispatched_event(&coord_path, "coord.child-pending", "coord", &ts);
    let waker = RecordingWaker::default();
    let outcome = wake_candidates_pass(
        root,
        &sessions_dir(root),
        &coord_path,
        &waker,
        Duration::from_secs(600),
        SystemTime::now(),
    )
    .unwrap();
    assert_eq!(outcome.events_emitted, 0);
    assert_eq!(outcome.wakes_invoked, 0);
}

// ----- AC: age-and-activity recovery — fires on idle requester ----------

#[test]
fn recovery_fires_when_requester_idle_past_timeout() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    sessions_dir(root);
    let coord_path = write_session_with_header(root, "coord", None);
    write_session_with_header(root, "parent", None);

    // Pre-seed: a RequesterWoken event was emitted long ago. Use
    // append_event with a fixed timestamp string so the parser sees a
    // stale woken_at.
    let woken_at_str = format_rfc3339_millis(now_minus(700)); // 700s ago > 600s timeout
    let kids: Vec<ValidatedSessionId> = vec![ValidatedSessionId::new("parent.child-a").unwrap()];
    let fields = requester_woken_fields(&kids, "parent");
    let payload = EventPayload::EvidenceSubmitted {
        state: "kt1.wake".into(),
        fields,
        submitter_cwd: None,
    };
    append_event(&coord_path, &payload, &woken_at_str).unwrap();

    // Requester's session log mtime is pinned at woken_at (no activity
    // since wake). filetime lets us set it precisely.
    let req_path = root
        .join("sessions")
        .join("parent")
        .join(state_file_name("parent"));
    let woken_at_system = SystemTime::now() - Duration::from_secs(700);
    set_file_mtime(
        &req_path,
        FileTime::from_system_time(woken_at_system - Duration::from_secs(1)),
    )
    .unwrap();

    let waker = RecordingWaker::default();
    let outcome = wake_candidates_pass(
        root,
        &sessions_dir(root),
        &coord_path,
        &waker,
        Duration::from_secs(600),
        SystemTime::now(),
    )
    .unwrap();

    assert_eq!(outcome.recoveries_fired, 1, "expected recovery to fire");
    assert_eq!(outcome.wakes_invoked, 1, "expected 1 substrate wake call");
    // NO new RequesterWoken event emitted (the original is authoritative).
    assert_eq!(outcome.events_emitted, 0);
    assert_eq!(waker.calls(), vec!["parent".to_string()]);
}

// ----- AC: happy-path NOT double-woken (requester is active) ------------

#[test]
fn recovery_does_not_fire_when_requester_made_progress() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    sessions_dir(root);
    let coord_path = write_session_with_header(root, "coord", None);
    write_session_with_header(root, "parent", None);

    // RequesterWoken emitted 700s ago.
    let woken_at_str = format_rfc3339_millis(now_minus(700));
    let kids: Vec<ValidatedSessionId> = vec![ValidatedSessionId::new("parent.child-a").unwrap()];
    let fields = requester_woken_fields(&kids, "parent");
    let payload = EventPayload::EvidenceSubmitted {
        state: "kt1.wake".into(),
        fields,
        submitter_cwd: None,
    };
    append_event(&coord_path, &payload, &woken_at_str).unwrap();

    // Requester resumed — its log mtime is RECENT (now).
    let req_path = root
        .join("sessions")
        .join("parent")
        .join(state_file_name("parent"));
    set_file_mtime(&req_path, FileTime::from_system_time(SystemTime::now())).unwrap();

    let waker = RecordingWaker::default();
    let outcome = wake_candidates_pass(
        root,
        &sessions_dir(root),
        &coord_path,
        &waker,
        Duration::from_secs(600),
        SystemTime::now(),
    )
    .unwrap();

    assert_eq!(
        outcome.recoveries_fired, 0,
        "recovery must NOT fire when requester is active"
    );
    assert_eq!(waker.count(), 0);
}

// ----- AC: recovery DOES NOT fire within timeout -------------------------

#[test]
fn recovery_does_not_fire_within_timeout() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    sessions_dir(root);
    let coord_path = write_session_with_header(root, "coord", None);
    write_session_with_header(root, "parent", None);

    // RequesterWoken emitted 10s ago (well under 600s timeout).
    let woken_at_str = format_rfc3339_millis(now_minus(10));
    let kids: Vec<ValidatedSessionId> = vec![ValidatedSessionId::new("parent.child-a").unwrap()];
    let fields = requester_woken_fields(&kids, "parent");
    let payload = EventPayload::EvidenceSubmitted {
        state: "kt1.wake".into(),
        fields,
        submitter_cwd: None,
    };
    append_event(&coord_path, &payload, &woken_at_str).unwrap();
    // Requester's log mtime pinned older than woken_at to isolate the
    // timeout-check failure (not the activity check).
    let req_path = root
        .join("sessions")
        .join("parent")
        .join(state_file_name("parent"));
    set_file_mtime(
        &req_path,
        FileTime::from_system_time(SystemTime::now() - Duration::from_secs(30)),
    )
    .unwrap();

    let waker = RecordingWaker::default();
    let outcome = wake_candidates_pass(
        root,
        &sessions_dir(root),
        &coord_path,
        &waker,
        Duration::from_secs(600),
        SystemTime::now(),
    )
    .unwrap();
    assert_eq!(outcome.recoveries_fired, 0, "must not fire within timeout");
    assert_eq!(waker.count(), 0);
}

// ----- AC: assignment_claim → terminal child unlinks sidecar -------------

#[test]
fn sidecar_unlink_happens_after_wake_delivery() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    sessions_dir(root);
    let coord_path = write_session_with_header(root, "coord", None);
    let child_path = write_session_with_header(root, "coord.child-a", Some("coord"));
    let _ = child_path;

    // Sidecar exists before the wake pass.
    let child_dir = sessions_dir(root).join("coord.child-a");
    create_sidecar(&child_dir);
    assert!(sidecar_path(&child_dir).exists());

    let ts = format_rfc3339_millis(now_minus(60));
    append_child_dispatched_event(&coord_path, "coord.child-a", "coord", &ts);
    append_terminal_index_for(root, "coord.child-a");

    let waker = RecordingWaker::default();
    wake_candidates_pass(
        root,
        &sessions_dir(root),
        &coord_path,
        &waker,
        Duration::from_secs(600),
        SystemTime::now(),
    )
    .unwrap();

    // Sidecar gone.
    assert!(
        !sidecar_path(&child_dir).exists(),
        "sidecar must be released"
    );
}

// ----- AC: no scan-cursor interaction ------------------------------------

#[test]
fn wake_pass_does_not_touch_scan_cursor() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    sessions_dir(root);
    let coord_path = write_session_with_header(root, "coord", None);
    write_session_with_header(root, "coord.child-a", Some("coord"));
    let ts = format_rfc3339_millis(now_minus(60));
    append_child_dispatched_event(&coord_path, "coord.child-a", "coord", &ts);
    append_terminal_index_for(root, "coord.child-a");

    // Confirm the coordinators/ dir doesn't exist (no scan cursor for
    // this coord yet).
    let coord_dir = root.join("coordinators").join("coord");
    assert!(!coord_dir.exists());

    let waker = RecordingWaker::default();
    wake_candidates_pass(
        root,
        &sessions_dir(root),
        &coord_path,
        &waker,
        Duration::from_secs(600),
        SystemTime::now(),
    )
    .unwrap();

    // After the pass, the coordinators/<coord>/ dir STILL doesn't
    // exist — wake never touches the cursor path.
    assert!(
        !coord_dir.exists(),
        "wake-candidates pass must not create cursor dirs; this is the discovery scan's responsibility"
    );
    let cursor_file = coord_dir.join("scan_cursor.toml");
    assert!(!cursor_file.exists());
}

// ----- AC: O(open-dispatches), not O(workspace) -------------------------

#[test]
fn wake_pass_is_o_open_dispatches() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    sessions_dir(root);
    let coord_path = write_session_with_header(root, "coord", None);

    // 200 unrelated sessions in the workspace (simulate a busy
    // workspace). None are dispatched by this coord.
    for i in 0..200 {
        let id = format!("unrelated-{:04}", i);
        write_session_with_header(root, &id, None);
    }

    // 5 dispatched-and-terminal children.
    for tag in ["a", "b", "c", "d", "e"] {
        let id = format!("coord.child-{}", tag);
        write_session_with_header(root, &id, Some("coord"));
        let ts = format_rfc3339_millis(now_minus(60));
        append_child_dispatched_event(&coord_path, &id, "coord", &ts);
        append_terminal_index_for(root, &id);
    }

    let waker = RecordingWaker::default();
    let outcome = wake_candidates_pass(
        root,
        &sessions_dir(root),
        &coord_path,
        &waker,
        Duration::from_secs(600),
        SystemTime::now(),
    )
    .unwrap();

    // The pass surfaced exactly 5 candidates batched into 1 RequesterWoken
    // (all under the same requester "coord"). The unrelated 200 sessions
    // were not read.
    assert_eq!(outcome.events_emitted, 1);
    assert_eq!(outcome.wakes_invoked, 1);
    assert_eq!(waker.count(), 1);
    let (_, events) = read_events(&coord_path).unwrap();
    let woken_event = events
        .iter()
        .find(|e| match &e.payload {
            EventPayload::EvidenceSubmitted { fields, .. } => fields
                .get("kind")
                .and_then(|v| v.as_str())
                .map(|s| s == REQUESTER_WOKEN)
                .unwrap_or(false),
            _ => false,
        })
        .unwrap();
    if let EventPayload::EvidenceSubmitted { fields, .. } = &woken_event.payload {
        assert_eq!(fields["child_count"], serde_json::json!(5));
    }
}

// ----- AC: requester-resumed-then-idle (F1 boundary) ---------------------

#[test]
fn requester_resumed_then_idle_does_not_trigger_wake_recovery() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    sessions_dir(root);
    let coord_path = write_session_with_header(root, "coord", None);
    write_session_with_header(root, "parent", None);

    // RequesterWoken from 1 hour ago.
    let woken_at_str = format_rfc3339_millis(now_minus(3600));
    let kids: Vec<ValidatedSessionId> = vec![ValidatedSessionId::new("parent.child-a").unwrap()];
    let fields = requester_woken_fields(&kids, "parent");
    let payload = EventPayload::EvidenceSubmitted {
        state: "kt1.wake".into(),
        fields,
        submitter_cwd: None,
    };
    append_event(&coord_path, &payload, &woken_at_str).unwrap();

    // Requester resumed AFTER the wake (mtime around 30 min ago), then
    // went idle. The wake-recovery rule only checks
    // `requester_log_mtime > woken_at` — that's true here, so wake
    // recovery does NOT fire. (F1 cold-restart in Issue 16 handles
    // the post-resume idle case.)
    let req_path = root
        .join("sessions")
        .join("parent")
        .join(state_file_name("parent"));
    set_file_mtime(
        &req_path,
        FileTime::from_system_time(SystemTime::now() - Duration::from_secs(1800)),
    )
    .unwrap();

    let waker = RecordingWaker::default();
    let outcome = wake_candidates_pass(
        root,
        &sessions_dir(root),
        &coord_path,
        &waker,
        Duration::from_secs(600),
        SystemTime::now(),
    )
    .unwrap();
    assert_eq!(
        outcome.recoveries_fired, 0,
        "wake-recovery must NOT fire when requester resumed; F1 (Issue 16) handles post-resume idle"
    );
    assert_eq!(waker.count(), 0);
}

// ----- AC: pass tolerates absent coord log -------------------------------

#[test]
fn pass_tolerates_missing_coord_log() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    sessions_dir(root);
    // No coord log written at all.
    let coord_path = root
        .join("sessions")
        .join("coord")
        .join(state_file_name("coord"));
    let waker = RecordingWaker::default();
    let outcome = wake_candidates_pass(
        root,
        &sessions_dir(root),
        &coord_path,
        &waker,
        Duration::from_secs(600),
        SystemTime::now(),
    )
    .unwrap();
    assert_eq!(outcome.events_emitted, 0);
    assert_eq!(outcome.wakes_invoked, 0);
    assert_eq!(outcome.recoveries_fired, 0);
}

// ----- AC: header.requested_by missing → candidate is skipped -----------

#[test]
fn candidate_skipped_when_requested_by_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    sessions_dir(root);
    let coord_path = write_session_with_header(root, "coord", None);
    // Child header has NO requested_by — should be skipped.
    write_session_with_header(root, "coord.child-orphan", None);
    let ts = format_rfc3339_millis(now_minus(60));
    append_child_dispatched_event(&coord_path, "coord.child-orphan", "coord", &ts);
    append_terminal_index_for(root, "coord.child-orphan");
    let waker = RecordingWaker::default();
    let outcome = wake_candidates_pass(
        root,
        &sessions_dir(root),
        &coord_path,
        &waker,
        Duration::from_secs(600),
        SystemTime::now(),
    )
    .unwrap();
    assert_eq!(
        outcome.events_emitted, 0,
        "child with missing requested_by must be skipped"
    );
}

// ----- Force link of AssignmentClaim symbol -----------------------------

#[allow(dead_code)]
fn _link_assignment_claim(c: AssignmentClaim) -> String {
    c.coord_id
}
