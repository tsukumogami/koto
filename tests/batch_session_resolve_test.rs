//! Integration tests for Issue #19's session-resolve hardening.
//!
//! scenario-29 covers the four `auto` reconciliation paths plus the
//! three explicit policy overrides that `koto session resolve
//! --children` exposes. These tests run entirely against the in-process
//! `CloudBackend::classify_reconciliation` / `reconcile_child` surfaces
//! so no reachable S3 endpoint is needed; the cloud-integration-tests
//! feature still exists for end-to-end bucket testing.
//!
//! scenario-30 asserts the "push parent before child mutation"
//! ordering (Decision 12 Q6) enforced by `handle_retry_failed`. It
//! wraps a real `LocalBackend` in a recorder that logs every mutating
//! call; the recorder lets us assert that the parent's strict
//! `ensure_pushed` probe fires BEFORE any child `append_event`. A
//! second recorder variant simulates a parent-push failure and asserts
//! that no child mutations run after the failure.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use assert_fs::TempDir;

use koto::cli::batch_error::BatchError;
use koto::cli::retry::{handle_retry_failed, RetryAction, RetryFailedPayload};
use koto::engine::batch_validation::TaskEntry;
use koto::engine::types::{now_iso8601, Event, EventPayload, StateFileHeader};
use koto::session::cloud::{ChildResolution, CloudBackend};
use koto::session::local::LocalBackend;
use koto::session::{state_file_name, SessionBackend, SessionError, SessionInfo, SessionLock};

use s3::creds::Credentials;
use s3::{Bucket, Region};

// ---------------------------------------------------------------------
//  scenario-29: reconciliation classifier exercised across all paths.
// ---------------------------------------------------------------------
//
// Every `auto` branch and every explicit policy override is exercised
// with byte patterns that mirror what a parent + 3 children fixture
// produces in practice. The classifier is pure so we can avoid the
// complexity of standing up a fake S3 in-process — the I/O seam is
// covered by the cloud.rs unit tests in the same PR.

#[test]
fn scenario_29_auto_identical_when_bytes_match() {
    let bytes = b"{\"schema_version\":1}\n{\"seq\":1}\n";
    assert_eq!(
        CloudBackend::classify_reconciliation(Some(bytes), Some(bytes), "auto"),
        ChildResolution::Identical
    );
}

#[test]
fn scenario_29_auto_accept_local_when_local_extends_remote() {
    // Child wrote a new event locally that S3 has not yet observed.
    // Strict-prefix classifier must pick AcceptedLocal so the later
    // push promotes the local log to S3.
    let remote = b"{\"schema_version\":1}\n{\"seq\":1}\n";
    let local = b"{\"schema_version\":1}\n{\"seq\":1}\n{\"seq\":2}\n";
    assert_eq!(
        CloudBackend::classify_reconciliation(Some(local), Some(remote), "auto"),
        ChildResolution::AcceptedLocal
    );
}

#[test]
fn scenario_29_auto_accept_remote_when_remote_extends_local() {
    // Another host appended to the child while we were offline; the
    // classifier must pull those bytes over our local copy.
    let local = b"{\"schema_version\":1}\n{\"seq\":1}\n";
    let remote = b"{\"schema_version\":1}\n{\"seq\":1}\n{\"seq\":2}\n";
    assert_eq!(
        CloudBackend::classify_reconciliation(Some(local), Some(remote), "auto"),
        ChildResolution::AcceptedRemote
    );
}

#[test]
fn scenario_29_auto_conflict_when_both_sides_diverge() {
    // Each side wrote a distinct event past the shared header. No
    // strict-prefix relationship holds, so the classifier must surface
    // Conflict and require a per-child `koto session resolve <child>`.
    let local = b"{\"schema_version\":1}\n{\"seq\":1-local\"}\n";
    let remote = b"{\"schema_version\":1}\n{\"seq\":1-remote\"}\n";
    assert_eq!(
        CloudBackend::classify_reconciliation(Some(local), Some(remote), "auto"),
        ChildResolution::Conflict
    );
}

#[test]
fn scenario_29_skip_override_ignores_bytes() {
    // Explicit `skip` policy must refuse to act no matter what the
    // bytes look like: divergent, identical, and one-sided all map
    // to Skipped.
    assert_eq!(
        CloudBackend::classify_reconciliation(Some(b"a"), Some(b"b"), "skip"),
        ChildResolution::Skipped
    );
    assert_eq!(
        CloudBackend::classify_reconciliation(Some(b"a"), Some(b"a"), "skip"),
        ChildResolution::Skipped
    );
    assert_eq!(
        CloudBackend::classify_reconciliation(None, Some(b"a"), "skip"),
        ChildResolution::Skipped
    );
}

#[test]
fn scenario_29_accept_remote_override_pulls_remote_unconditionally() {
    // Even when local is a strict extension of remote (normally an
    // `AcceptedLocal` signal), the explicit override must surface
    // AcceptedRemote so the operator can force the pull.
    let remote = b"{\"schema_version\":1}\n{\"seq\":1}\n";
    let local = b"{\"schema_version\":1}\n{\"seq\":1}\n{\"seq\":2}\n";
    assert_eq!(
        CloudBackend::classify_reconciliation(Some(local), Some(remote), "accept-remote"),
        ChildResolution::AcceptedRemote
    );
}

#[test]
fn scenario_29_accept_local_override_pushes_local_unconditionally() {
    // Mirror of the previous test: when the bytes say we should pull
    // remote, the `accept-local` override must still fire and push.
    let local = b"{\"schema_version\":1}\n{\"seq\":1}\n";
    let remote = b"{\"schema_version\":1}\n{\"seq\":1}\n{\"seq\":2}\n";
    assert_eq!(
        CloudBackend::classify_reconciliation(Some(local), Some(remote), "accept-local"),
        ChildResolution::AcceptedLocal
    );
}

#[test]
fn scenario_29_accept_remote_errors_when_remote_absent() {
    // The explicit override fails cleanly when the named side is
    // unavailable — no silent fallback to "the other side wins".
    assert!(matches!(
        CloudBackend::classify_reconciliation(Some(b"local"), None, "accept-remote"),
        ChildResolution::Errored { .. }
    ));
}

#[test]
fn scenario_29_accept_local_errors_when_local_absent() {
    assert!(matches!(
        CloudBackend::classify_reconciliation(None, Some(b"remote"), "accept-local"),
        ChildResolution::Errored { .. }
    ));
}

// reconcile_child uses an unreachable S3 endpoint in test_cloud_backend,
// so the transient-error path is the only branch we can drive end-to-
// end here. Under Issue #19's fix this must surface as Errored — it
// used to quietly fall through to AcceptedLocal and overwrite remote.
#[test]
fn scenario_29_reconcile_child_auto_refuses_to_overwrite_on_transient_error() {
    let tmp = TempDir::new().unwrap();
    let backend = test_cloud_backend(tmp.path());
    write_minimal_state(tmp.path(), "parent.child_a", "2026-04-14T00:00:00Z");
    let before = std::fs::read(
        tmp.path()
            .join("parent.child_a")
            .join(state_file_name("parent.child_a")),
    )
    .unwrap();

    let outcome = backend.reconcile_child("parent.child_a", "auto");
    match outcome {
        ChildResolution::Errored { .. } => {}
        other => panic!(
            "auto must surface Errored when remote is unreachable, not AcceptedLocal. got: {:?}",
            other
        ),
    }

    let after = std::fs::read(
        tmp.path()
            .join("parent.child_a")
            .join(state_file_name("parent.child_a")),
    )
    .unwrap();
    assert_eq!(
        before, after,
        "local bytes must not change on transient error"
    );
}

// ---------------------------------------------------------------------
//  scenario-30: retry_failed must push parent BEFORE any child write.
// ---------------------------------------------------------------------
//
// We wrap a real `LocalBackend` in a `RecorderBackend` that logs the
// session id of every mutating operation. The recorder intercepts
// `append_event` and `ensure_pushed`; the LocalBackend provides the
// actual durability so `validate_retry_request` can read the real
// parent + child event logs.
//
// Under a CloudBackend the mutations happen in this order:
//   1. parent append_event (retry_failed evidence)       [already sync-pushed by append_event]
//   2. parent append_event (retry_failed clearing)       [already sync-pushed by append_event]
//   3. parent ensure_pushed                              [strict fail-fast probe]
//   4. child  append_event (Rewound)                     [only if step 3 succeeded]
//
// This test asserts that the recorded order matches that interleaving,
// AND that a parent-push failure aborts before ANY child write fires.

/// Shared log of ordered mutation events against the recorder backend.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Op {
    AppendEvent(String),
    AppendHeader(String),
    EnsurePushed(String),
    InitStateFile(String),
    Cleanup(String),
}

/// Test backend that records every mutation in order. The behavior is
/// fully delegated to an inner LocalBackend so real state files land on
/// disk and `validate_retry_request` can parse them during dispatch.
///
/// When `fail_parent_push_for` is set to a session id, `ensure_pushed`
/// returns `SessionError::Other` for that id — this stands in for a
/// CloudBackend parent PUT that failed on S3.
struct RecorderBackend {
    inner: LocalBackend,
    log: Arc<Mutex<Vec<Op>>>,
    fail_parent_push_for: Option<String>,
}

impl RecorderBackend {
    fn new(base_dir: PathBuf) -> (Self, Arc<Mutex<Vec<Op>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let b = Self {
            inner: LocalBackend::with_base_dir(base_dir),
            log: Arc::clone(&log),
            fail_parent_push_for: None,
        };
        (b, log)
    }

    fn new_failing_push(base_dir: PathBuf, session: &str) -> (Self, Arc<Mutex<Vec<Op>>>) {
        let (mut b, log) = Self::new(base_dir);
        b.fail_parent_push_for = Some(session.to_string());
        (b, log)
    }
}

impl SessionBackend for RecorderBackend {
    fn create(&self, id: &str) -> anyhow::Result<PathBuf> {
        self.inner.create(id)
    }

    fn session_dir(&self, id: &str) -> PathBuf {
        self.inner.session_dir(id)
    }

    fn exists(&self, id: &str) -> bool {
        self.inner.exists(id)
    }

    fn cleanup(&self, id: &str) -> anyhow::Result<()> {
        self.log.lock().unwrap().push(Op::Cleanup(id.to_string()));
        self.inner.cleanup(id)
    }

    fn list(&self) -> anyhow::Result<Vec<SessionInfo>> {
        self.inner.list()
    }

    fn append_header(&self, id: &str, header: &StateFileHeader) -> anyhow::Result<()> {
        self.log
            .lock()
            .unwrap()
            .push(Op::AppendHeader(id.to_string()));
        self.inner.append_header(id, header)
    }

    fn append_event(
        &self,
        id: &str,
        payload: &EventPayload,
        timestamp: &str,
    ) -> anyhow::Result<()> {
        self.log
            .lock()
            .unwrap()
            .push(Op::AppendEvent(id.to_string()));
        self.inner.append_event(id, payload, timestamp)
    }

    fn read_events(&self, id: &str) -> anyhow::Result<(StateFileHeader, Vec<Event>)> {
        self.inner.read_events(id)
    }

    fn read_header(&self, id: &str) -> anyhow::Result<StateFileHeader> {
        self.inner.read_header(id)
    }

    fn init_state_file(
        &self,
        id: &str,
        header: StateFileHeader,
        initial_events: Vec<Event>,
    ) -> Result<(), SessionError> {
        self.log
            .lock()
            .unwrap()
            .push(Op::InitStateFile(id.to_string()));
        self.inner.init_state_file(id, header, initial_events)
    }

    fn lock_state_file(&self, id: &str) -> Result<SessionLock, SessionError> {
        self.inner.lock_state_file(id)
    }

    fn ensure_pushed(&self, id: &str) -> Result<(), SessionError> {
        self.log
            .lock()
            .unwrap()
            .push(Op::EnsurePushed(id.to_string()));
        if let Some(fail_id) = &self.fail_parent_push_for {
            if fail_id == id {
                return Err(SessionError::Other(anyhow::anyhow!(
                    "simulated S3 PUT failure for {}",
                    id
                )));
            }
        }
        self.inner.ensure_pushed(id)
    }
}

/// Drop a minimal parent + failed child pair onto disk using the
/// public `SessionBackend` and `engine::types` APIs so the on-disk
/// shape matches what the production code produces. The helper
/// compiles a minimal child template JSON to a tempfile so the retry
/// dispatcher can read it back when classifying the child's outcome
/// and computing the Rewound event's target state.
fn seed_parent_with_failed_child(base_dir: &Path, parent: &str, child_task: &str) {
    let child = format!("{}.{}", parent, child_task);
    let seed_backend = LocalBackend::with_base_dir(base_dir.to_path_buf());

    // Compile a minimal child template JSON on disk. Matches the
    // `CompiledTemplate` shape expected by
    // `retry::classify_child_outcome`.
    let child_template_json = serde_json::json!({
        "format_version": 1,
        "name": "batch-child",
        "version": "1.0",
        "initial_state": "work",
        "states": {
            "work": {
                "directive": "do work",
                "terminal": false,
                "failure": false,
                "skipped_marker": false,
            },
            "failed": {
                "directive": "failed",
                "terminal": true,
                "failure": true,
                "skipped_marker": false,
            },
        },
    });
    let templates_dir = base_dir.join("_templates");
    std::fs::create_dir_all(&templates_dir).unwrap();
    let child_template_path = templates_dir.join("child.compiled.json");
    std::fs::write(&child_template_path, child_template_json.to_string()).unwrap();

    // Parent state file (header only — the retry dispatcher reads
    // events back but doesn't require any pre-existing payloads on the
    // parent's log).
    let parent_header = StateFileHeader {
        schema_version: 1,
        workflow: parent.to_string(),
        template_hash: "testhash".to_string(),
        created_at: now_iso8601(),
        parent_workflow: None,
        template_source_dir: None,
    };
    seed_backend
        .init_state_file(parent, parent_header, Vec::new())
        .unwrap();

    // Child state file: WorkflowInitialized (pointing at the compiled
    // template so the dispatcher can classify the outcome) followed by
    // a Transitioned into the terminal `failed` state.
    let ts = now_iso8601();
    let child_header = StateFileHeader {
        schema_version: 1,
        workflow: child.to_string(),
        template_hash: "testhash".to_string(),
        created_at: ts.clone(),
        parent_workflow: Some(parent.to_string()),
        template_source_dir: None,
    };
    let init_event = Event {
        seq: 1,
        timestamp: ts.clone(),
        event_type: "workflow_initialized".to_string(),
        payload: EventPayload::WorkflowInitialized {
            template_path: child_template_path.to_string_lossy().to_string(),
            variables: Default::default(),
            spawn_entry: None,
        },
    };
    let transition_event = Event {
        seq: 2,
        timestamp: ts,
        event_type: "transitioned".to_string(),
        payload: EventPayload::Transitioned {
            from: Some("work".to_string()),
            to: "failed".to_string(),
            condition_type: "direct".to_string(),
        },
    };
    seed_backend
        .init_state_file(&child, child_header, vec![init_event, transition_event])
        .unwrap();
}

#[test]
fn scenario_30_parent_ensure_pushed_fires_before_any_child_write() {
    let tmp = TempDir::new().unwrap();
    seed_parent_with_failed_child(tmp.path(), "p30a", "A");

    let (backend, log) = RecorderBackend::new(tmp.path().to_path_buf());

    let payload = RetryFailedPayload {
        children: vec!["A".to_string()],
        include_skipped: false,
    };

    let outcome = handle_retry_failed(
        &backend,
        "p30a",
        "plan",
        &payload,
        &[],
        None,
        &[TaskEntry {
            name: "A".to_string(),
            waits_on: Vec::new(),
            vars: std::collections::BTreeMap::new(),
            template: None,
        }],
    )
    .expect("retry_failed dispatch must succeed with a reachable backend");
    assert_eq!(outcome.dispatched.len(), 1);
    assert_eq!(outcome.dispatched[0].retry_action, RetryAction::Rewind);

    let ops = log.lock().unwrap().clone();

    // Locate the first child mutation (any AppendEvent targeting
    // p30a.A) and the parent's EnsurePushed. The ordering invariant is
    // that every child mutation appears STRICTLY AFTER the parent's
    // strict push probe fires.
    let child_ops: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter_map(|(i, op)| match op {
            Op::AppendEvent(id) if id == "p30a.A" => Some(i),
            Op::InitStateFile(id) if id == "p30a.A" => Some(i),
            _ => None,
        })
        .collect();
    let parent_push_idx = ops
        .iter()
        .position(|op| matches!(op, Op::EnsurePushed(id) if id == "p30a"))
        .unwrap_or_else(|| panic!("parent ensure_pushed missing from log: {:?}", ops));

    assert!(
        !child_ops.is_empty(),
        "expected at least one child mutation in log: {:?}",
        ops
    );
    for idx in &child_ops {
        assert!(
            *idx > parent_push_idx,
            "child mutation at {} fired before parent ensure_pushed at {}; log: {:?}",
            idx,
            parent_push_idx,
            ops
        );
    }

    // Parent appends (evidence + clearing) must both precede the
    // strict push; that's the whole point of the probe.
    let parent_appends: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter_map(|(i, op)| match op {
            Op::AppendEvent(id) if id == "p30a" => Some(i),
            _ => None,
        })
        .collect();
    assert_eq!(
        parent_appends.len(),
        2,
        "expected two parent appends (evidence + clearing): {:?}",
        ops
    );
    for idx in &parent_appends {
        assert!(
            *idx < parent_push_idx,
            "parent append at {} fired after ensure_pushed at {}; log: {:?}",
            idx,
            parent_push_idx,
            ops
        );
    }
}

#[test]
fn scenario_30_parent_push_failure_blocks_all_child_writes() {
    let tmp = TempDir::new().unwrap();
    seed_parent_with_failed_child(tmp.path(), "p30b", "A");

    let (backend, log) = RecorderBackend::new_failing_push(tmp.path().to_path_buf(), "p30b");

    let payload = RetryFailedPayload {
        children: vec!["A".to_string()],
        include_skipped: false,
    };

    let err = handle_retry_failed(
        &backend,
        "p30b",
        "plan",
        &payload,
        &[],
        None,
        &[TaskEntry {
            name: "A".to_string(),
            waits_on: Vec::new(),
            vars: std::collections::BTreeMap::new(),
            template: None,
        }],
    )
    .expect_err("parent push failure must abort handle_retry_failed");

    match err {
        BatchError::BackendError { retryable, .. } => {
            assert!(
                retryable,
                "a transient parent push failure must be retryable so callers can re-run"
            );
        }
        other => panic!("expected BackendError, got {:?}", other),
    }

    // The recorder log must NOT carry any child mutation after the
    // parent push attempt. Parent appends are expected (they happen
    // before the probe); child appends / init calls are not.
    let ops = log.lock().unwrap().clone();
    let child_mutations: Vec<&Op> = ops
        .iter()
        .filter(|op| {
            matches!(op,
            Op::AppendEvent(id) | Op::InitStateFile(id) | Op::Cleanup(id)
                if id.starts_with("p30b."))
        })
        .collect();
    assert!(
        child_mutations.is_empty(),
        "no child writes must occur when parent push fails; saw: {:?}",
        child_mutations
    );
}

// ---------------------------------------------------------------------
//  Shared helpers
// ---------------------------------------------------------------------

/// Copy of the private helper in `src/session/cloud.rs`: a CloudBackend
/// pointed at an unreachable endpoint so every S3 call fails. Lets the
/// integration test exercise the transient-error branch of
/// `reconcile_child` without a live bucket.
fn test_cloud_backend(base_dir: &Path) -> CloudBackend {
    let local = LocalBackend::with_base_dir(base_dir.to_path_buf());
    let region = Region::Custom {
        region: "us-east-1".to_string(),
        // RFC 5737 TEST-NET-1: guaranteed non-routable so every S3
        // connection fails fast.
        endpoint: "http://192.0.2.1:19000".to_string(),
    };
    let credentials =
        Credentials::new(Some("test-key"), Some("test-secret"), None, None, None).unwrap();
    let bucket = Bucket::new("test-bucket", region, credentials).unwrap();
    CloudBackend::with_parts(local, bucket, "test-prefix".to_string())
}

/// Write a minimal state file header so `reconcile_child` has local
/// bytes to reason about.
fn write_minimal_state(base_dir: &Path, id: &str, created_at: &str) {
    let dir = base_dir.join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(state_file_name(id));
    let header = serde_json::json!({
        "schema_version": 1,
        "workflow": id,
        "template_hash": "testhash",
        "created_at": created_at,
        "parent_workflow": null,
        "template_source_dir": null,
    });
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string(&header).unwrap()),
    )
    .unwrap();
}
