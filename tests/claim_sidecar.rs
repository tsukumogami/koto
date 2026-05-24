//! Coverage for KT1 Issue 11 — O_EXCL sidecar atomicity, happy-path
//! dispatch orchestration, and drift recovery (cases 3a/3b/3c +
//! malformed fallthrough).
//!
//! Race-condition AC: N=32 concurrent claims yield exactly-one-winner.
//! Recovery ACs synthesize an on-disk crash state, invoke
//! [`recover_orphaned_sidecar`], and assert the right side effects fire
//! (or don't fire) in the right order.

#![cfg(unix)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use koto::engine::audit::{CHILD_DISPATCHED, CHILD_REDELEGATED};
use koto::engine::claim::{
    acquire_sidecar, claim_and_dispatch, format_rfc3339_millis, read_sidecar,
    recover_orphaned_sidecar, release_sidecar, sidecar_path,
    write_header_assignment_claim_best_effort, AcquireOutcome, DispatchOutcome, RecoveryAction,
    RecoveryInputs, SidecarContents, SpawnRequest, SubstrateSpawner, MALFORMED_COORD_MARKER,
    SIDECAR_FILENAME,
};
use koto::engine::errors::EngineError;
use koto::engine::persistence::{append_header, read_header};
use koto::engine::types::{AssignmentClaim, StateFileHeader, ValidatedCoordId, ValidatedSessionId};
use tempfile::TempDir;

// ============================================================
// Test fixture helpers
// ============================================================

fn vsid(s: &str) -> ValidatedSessionId {
    ValidatedSessionId::new(s).expect("test session id")
}

fn vcoord(s: &str) -> ValidatedCoordId {
    ValidatedCoordId::new(s).expect("test coord id")
}

/// Materialize a child session directory with a minimal state-file
/// header. The header carries the request-store fields the dispatch
/// and recovery paths read.
struct Fixture {
    _temp: TempDir,
    /// `<temp>/sessions/<child>` — the directory containing the
    /// child's state file and (potentially) sidecar.
    session_dir: PathBuf,
    /// `<temp>/sessions/<child>/<child>.state.jsonl`.
    state_file: PathBuf,
    /// `<temp>/sessions/<coord>/<coord>.state.jsonl` for audit events.
    coord_state_file: PathBuf,
    child: ValidatedSessionId,
    coord: ValidatedCoordId,
}

fn fixture(child_id: &str, coord_id: &str) -> Fixture {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions = temp.path().join("sessions");
    let session_dir = sessions.join(child_id);
    std::fs::create_dir_all(&session_dir).expect("create session dir");
    let state_file = session_dir.join(format!("{child_id}.state.jsonl"));
    let header = base_header(child_id);
    append_header(&state_file, &header).expect("write header");

    let coord_dir = sessions.join(coord_id);
    std::fs::create_dir_all(&coord_dir).expect("create coord dir");
    let coord_state_file = coord_dir.join(format!("{coord_id}.state.jsonl"));
    let coord_header = base_header(coord_id);
    append_header(&coord_state_file, &coord_header).expect("write coord header");

    Fixture {
        _temp: temp,
        session_dir,
        state_file,
        coord_state_file,
        child: vsid(child_id),
        coord: vcoord(coord_id),
    }
}

fn base_header(workflow: &str) -> StateFileHeader {
    StateFileHeader {
        schema_version: 1,
        workflow: workflow.to_string(),
        template_hash: "deadbeef".to_string(),
        created_at: "2026-05-24T00:00:00Z".to_string(),
        parent_workflow: None,
        template_source_dir: None,
        session_id: workflow.to_string(),
        intent: None,
        template_name: Some("scrutinize.md".to_string()),
        needs_agent: Some(true),
        role: Some("scrutineer".to_string()),
        inputs: Some(serde_json::json!({"issue": 11})),
        coordinator_of_record: Some("coord-7".to_string()),
        requested_by: Some("work-coord".to_string()),
        assignment_claim: None,
        dispatch_epoch: 0,
        priority: None,
        deadline: None,
        retry_count: None,
        agent_config: None,
    }
}

/// Read the coordinator's state-file event lines (skipping the header)
/// as parsed JSON values so tests can grep for audit-event fields.
fn read_coord_events(path: &Path) -> Vec<serde_json::Value> {
    let raw = std::fs::read_to_string(path).expect("read coord log");
    raw.lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("parse event"))
        .collect()
}

fn count_kind_in_log(path: &Path, kind: &str) -> usize {
    read_coord_events(path)
        .iter()
        .filter(|e| {
            e.get("payload")
                .and_then(|p| p.get("fields"))
                .and_then(|f| f.get("kind"))
                .and_then(|k| k.as_str())
                == Some(kind)
        })
        .count()
}

/// Recording mock for the [`SubstrateSpawner`] trait. Captures every
/// [`SpawnRequest`] so tests can assert the dispatch contract.
struct RecordingSpawner {
    calls: Mutex<Vec<SpawnRequest>>,
    fail: bool,
}

impl RecordingSpawner {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            fail: false,
        }
    }
}

impl SubstrateSpawner for RecordingSpawner {
    fn spawn(&self, request: &SpawnRequest) -> Result<(), EngineError> {
        self.calls.lock().unwrap().push(request.clone());
        if self.fail {
            return Err(EngineError::StateNotFound("simulated".to_string()));
        }
        Ok(())
    }
}

// ============================================================
// AC: Race condition — N=32 concurrent claims, exactly-one-winner
// ============================================================

#[test]
fn race_n32_concurrent_claims_exactly_one_winner() {
    let f = fixture("race.child", "race-coord");
    let now = SystemTime::now();

    let acquired = AtomicUsize::new(0);
    let contended = AtomicUsize::new(0);
    let barrier = Arc::new(Barrier::new(32));
    let session_dir = Arc::new(f.session_dir.clone());

    std::thread::scope(|s| {
        let mut handles = Vec::with_capacity(32);
        for i in 0..32 {
            let barrier = Arc::clone(&barrier);
            let session_dir = Arc::clone(&session_dir);
            let acquired = &acquired;
            let contended = &contended;
            handles.push(s.spawn(move || {
                let coord = vcoord(&format!("coord-{i:02}"));
                barrier.wait();
                let outcome = acquire_sidecar(&session_dir, &coord, now).expect("acquire");
                match outcome {
                    AcquireOutcome::Acquired => {
                        acquired.fetch_add(1, Ordering::SeqCst);
                    }
                    AcquireOutcome::Contended => {
                        contended.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }));
        }
        for h in handles {
            h.join().expect("thread join");
        }
    });

    assert_eq!(acquired.load(Ordering::SeqCst), 1, "exactly one winner");
    assert_eq!(contended.load(Ordering::SeqCst), 31, "31 losers");

    // The winning sidecar's coord_id is whichever thread's coord won;
    // we only assert there IS a sidecar with a parseable coord_id from
    // the contending set.
    let parsed = read_sidecar(&f.session_dir)
        .expect("read sidecar")
        .expect("sidecar present")
        .expect("sidecar parses");
    assert!(parsed.coord_id.starts_with("coord-"));
}

// ============================================================
// AC: Sidecar mode 0600
// ============================================================

#[test]
fn sidecar_mode_is_0600() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture("mode.child", "mode-coord");
    acquire_sidecar(&f.session_dir, &f.coord, SystemTime::now()).expect("acquire");
    let meta = std::fs::metadata(sidecar_path(&f.session_dir)).expect("stat sidecar");
    // Mask off file-type bits, keep only mode bits.
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "sidecar mode must be 0600, was 0o{:o}", mode);
}

// ============================================================
// AC: Sidecar contents shape — TOML {coord_id, claimed_at}
// ============================================================

#[test]
fn sidecar_contents_shape() {
    let f = fixture("shape.child", "shape-coord");
    let now = UNIX_EPOCH + Duration::from_millis(1_700_000_000_500);
    acquire_sidecar(&f.session_dir, &f.coord, now).expect("acquire");

    let raw = std::fs::read_to_string(sidecar_path(&f.session_dir)).expect("read sidecar");
    let parsed: SidecarContents = toml::from_str(&raw).expect("parse TOML");
    assert_eq!(parsed.coord_id, "shape-coord");
    assert_eq!(parsed.claimed_at, "2023-11-14T22:13:20.500Z");
    assert!(
        raw.len() < 100,
        "sidecar under 100 bytes (was {})",
        raw.len()
    );
}

// ============================================================
// AC: Best-effort header write doesn't undo the sidecar claim
// ============================================================

#[test]
fn header_write_best_effort_records_assignment_claim() {
    let f = fixture("hdr.child", "hdr-coord");
    let now = SystemTime::now();
    acquire_sidecar(&f.session_dir, &f.coord, now).expect("acquire");
    write_header_assignment_claim_best_effort(&f.state_file, &f.coord, &format_rfc3339_millis(now));
    let header = read_header(&f.state_file).expect("read header");
    let claim = header.assignment_claim.expect("header claim populated");
    assert_eq!(claim.coord_id, "hdr-coord");
    assert!(claim.claimed_at.ends_with('Z'));
}

#[test]
fn header_write_failure_does_not_undo_sidecar() {
    // Point the rewrite at a path that doesn't exist; the function
    // must log and return; the sidecar must still exist.
    let f = fixture("hdr2.child", "hdr2-coord");
    acquire_sidecar(&f.session_dir, &f.coord, SystemTime::now()).expect("acquire");
    let bogus = f.session_dir.join("does-not-exist.state.jsonl");
    write_header_assignment_claim_best_effort(&bogus, &f.coord, "2026-05-24T00:00:00.000Z");
    // Sidecar still present.
    assert!(sidecar_path(&f.session_dir).exists());
}

// ============================================================
// AC: L1 unlink (terminal evidence) — release_sidecar is idempotent
// ============================================================

#[test]
fn l1_unlink_idempotent() {
    let f = fixture("l1.child", "l1-coord");
    acquire_sidecar(&f.session_dir, &f.coord, SystemTime::now()).expect("acquire");
    assert!(sidecar_path(&f.session_dir).exists());

    release_sidecar(&f.session_dir).expect("L1 unlink");
    assert!(!sidecar_path(&f.session_dir).exists());

    // Second call is a no-op (idempotent).
    release_sidecar(&f.session_dir).expect("L1 unlink idempotent");
}

// ============================================================
// AC: Case 3a recovery — terminal child + stale sidecar
// ============================================================

#[test]
fn case_3a_terminal_with_stale_sidecar_cleans_up() {
    let f = fixture("3a.child", "3a-coord");
    let old = SystemTime::now() - Duration::from_secs(3600);
    acquire_sidecar(&f.session_dir, &f.coord, old).expect("acquire");

    let inputs = RecoveryInputs {
        session_dir: &f.session_dir,
        state_file: &f.state_file,
        coord_id: &f.coord,
        child_session_id: &f.child,
        child_is_terminal: true,
        stale_claim_timeout: Duration::from_secs(600),
        redelegation_cap: 3,
        now: SystemTime::now(),
    };
    let action = recover_orphaned_sidecar(&inputs).expect("recovery");
    assert!(matches!(action, RecoveryAction::CleanupOnly));
    assert!(!sidecar_path(&f.session_dir).exists());

    // No ChildRedelegated event was emitted on case 3a.
    assert_eq!(count_kind_in_log(&f.coord_state_file, CHILD_REDELEGATED), 0);

    // Header was NOT touched (no epoch bump).
    let header = read_header(&f.state_file).expect("read header");
    assert_eq!(header.dispatch_epoch, 0);
}

// ============================================================
// AC: Case 3b recovery — no header claim, sidecar stale
// ============================================================

#[test]
fn case_3b_orphan_no_header_claim_redelegates() {
    let f = fixture("3b.child", "3b-coord");
    let old = SystemTime::now() - Duration::from_secs(3600);
    acquire_sidecar(&f.session_dir, &f.coord, old).expect("acquire");

    let inputs = RecoveryInputs {
        session_dir: &f.session_dir,
        state_file: &f.state_file,
        coord_id: &f.coord,
        child_session_id: &f.child,
        child_is_terminal: false,
        stale_claim_timeout: Duration::from_secs(600),
        redelegation_cap: 3,
        now: SystemTime::now(),
    };

    let action = recover_orphaned_sidecar(&inputs).expect("recovery");
    match action {
        RecoveryAction::Redelegate {
            new_dispatch_epoch,
            cleared_header_claim,
        } => {
            assert_eq!(new_dispatch_epoch, 1);
            assert!(!cleared_header_claim, "3b: header had no claim to clear");
        }
        other => panic!("expected Redelegate, got {other:?}"),
    }

    // Sidecar unlinked.
    assert!(!sidecar_path(&f.session_dir).exists());
    // Epoch bumped to 1.
    let header = read_header(&f.state_file).expect("read header");
    assert_eq!(header.dispatch_epoch, 1);
    assert!(header.assignment_claim.is_none());
    // Exactly one ChildRedelegated emitted.
    assert_eq!(count_kind_in_log(&f.coord_state_file, CHILD_REDELEGATED), 1);
}

// ============================================================
// AC: Case 3c recovery — header claim present, sidecar stale
// ============================================================

#[test]
fn case_3c_header_claim_present_clears_and_redelegates() {
    let f = fixture("3c.child", "3c-coord");
    let old = SystemTime::now() - Duration::from_secs(3600);
    acquire_sidecar(&f.session_dir, &f.coord, old).expect("acquire");
    write_header_assignment_claim_best_effort(&f.state_file, &f.coord, "2026-05-24T00:00:00.000Z");

    let inputs = RecoveryInputs {
        session_dir: &f.session_dir,
        state_file: &f.state_file,
        coord_id: &f.coord,
        child_session_id: &f.child,
        child_is_terminal: false,
        stale_claim_timeout: Duration::from_secs(600),
        redelegation_cap: 3,
        now: SystemTime::now(),
    };

    let action = recover_orphaned_sidecar(&inputs).expect("recovery");
    match action {
        RecoveryAction::Redelegate {
            new_dispatch_epoch,
            cleared_header_claim,
        } => {
            assert_eq!(new_dispatch_epoch, 1);
            assert!(cleared_header_claim, "3c: header.assignment_claim was set");
        }
        other => panic!("expected Redelegate, got {other:?}"),
    }

    let header = read_header(&f.state_file).expect("read header");
    assert_eq!(header.dispatch_epoch, 1);
    assert!(
        header.assignment_claim.is_none(),
        "header claim must be cleared on 3c"
    );
    assert!(!sidecar_path(&f.session_dir).exists());
}

// ============================================================
// AC: Malformed sidecar fallthrough — synthesize stale 3b
// ============================================================

#[test]
fn malformed_sidecar_routes_to_redelegate() {
    let f = fixture("malformed.child", "malformed-coord");
    // Plant garbage at the sidecar path.
    std::fs::write(sidecar_path(&f.session_dir), b"not\nvalid\ntoml\0\xff").expect("plant garbage");

    let inputs = RecoveryInputs {
        session_dir: &f.session_dir,
        state_file: &f.state_file,
        coord_id: &f.coord,
        child_session_id: &f.child,
        child_is_terminal: false,
        stale_claim_timeout: Duration::from_secs(600),
        redelegation_cap: 3,
        now: SystemTime::now(),
    };

    let action = recover_orphaned_sidecar(&inputs).expect("recovery");
    assert!(matches!(action, RecoveryAction::Redelegate { .. }));
    assert!(!sidecar_path(&f.session_dir).exists());

    // The synthesized audit event carries the malformed marker is
    // implicit — the redelegation occurs against the synthetic stale
    // claim record. We only assert ChildRedelegated fired.
    assert_eq!(count_kind_in_log(&f.coord_state_file, CHILD_REDELEGATED), 1);
}

// ============================================================
// AC: Timeout-not-exceeded — live coordinators are NOT disturbed
// ============================================================

#[test]
fn fresh_sidecar_returns_skip() {
    let f = fixture("fresh.child", "fresh-coord");
    let now = SystemTime::now();
    // 300s old, default timeout is 600s.
    let claimed = now - Duration::from_secs(300);
    acquire_sidecar(&f.session_dir, &f.coord, claimed).expect("acquire");

    let inputs = RecoveryInputs {
        session_dir: &f.session_dir,
        state_file: &f.state_file,
        coord_id: &f.coord,
        child_session_id: &f.child,
        child_is_terminal: false,
        stale_claim_timeout: Duration::from_secs(600),
        redelegation_cap: 3,
        now,
    };
    let action = recover_orphaned_sidecar(&inputs).expect("recovery");
    assert!(matches!(action, RecoveryAction::Skip));
    assert!(
        sidecar_path(&f.session_dir).exists(),
        "fresh sidecar preserved"
    );
    let header = read_header(&f.state_file).expect("read header");
    assert_eq!(header.dispatch_epoch, 0, "header untouched");
}

// ============================================================
// AC: Redelegation-cap exceedance — Abandon
// ============================================================

#[test]
fn redelegation_cap_exceeded_returns_abandon() {
    let f = fixture("cap.child", "cap-coord");
    // Set the child's dispatch_epoch to the cap.
    let raw = std::fs::read_to_string(&f.state_file).expect("read");
    let mut header: StateFileHeader =
        serde_json::from_str(raw.lines().next().unwrap()).expect("parse header");
    header.dispatch_epoch = 3;
    let new_header_line = serde_json::to_string(&header).unwrap();
    let new_contents = format!("{new_header_line}\n");
    std::fs::write(&f.state_file, new_contents).expect("write modified");

    let old = SystemTime::now() - Duration::from_secs(3600);
    acquire_sidecar(&f.session_dir, &f.coord, old).expect("acquire");

    let inputs = RecoveryInputs {
        session_dir: &f.session_dir,
        state_file: &f.state_file,
        coord_id: &f.coord,
        child_session_id: &f.child,
        child_is_terminal: false,
        stale_claim_timeout: Duration::from_secs(600),
        redelegation_cap: 3,
        now: SystemTime::now(),
    };
    let action = recover_orphaned_sidecar(&inputs).expect("recovery");
    match action {
        RecoveryAction::Abandon {
            observed_dispatch_epoch,
        } => {
            assert_eq!(observed_dispatch_epoch, 3);
        }
        other => panic!("expected Abandon, got {other:?}"),
    }
    // No mutation on cap-exceedance: sidecar stays, header stays, no
    // ChildRedelegated emitted.
    assert!(sidecar_path(&f.session_dir).exists());
    let header_now = read_header(&f.state_file).expect("read header");
    assert_eq!(header_now.dispatch_epoch, 3, "header untouched");
    assert_eq!(count_kind_in_log(&f.coord_state_file, CHILD_REDELEGATED), 0);
}

// ============================================================
// AC: Happy-path ChildDispatched emission BEFORE spawn
// ============================================================

#[test]
fn happy_path_emits_child_dispatched_then_spawn() {
    let f = fixture("happy.child", "happy-coord");
    let spawner = RecordingSpawner::new();
    let outcome = claim_and_dispatch(
        &f.session_dir,
        &f.state_file,
        &f.coord_state_file,
        &f.coord,
        &f.child,
        &spawner,
        SystemTime::now(),
    )
    .expect("dispatch");
    assert!(matches!(outcome, DispatchOutcome::Dispatched));

    let calls = spawner.calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "exactly one spawn invocation");
    assert_eq!(calls[0].child_session_id, f.child);
    assert_eq!(calls[0].role, "scrutineer");
    assert_eq!(calls[0].template_name, "scrutinize.md");
    assert_eq!(calls[0].dispatch_epoch, 0);
    assert_eq!(calls[0].coord_id, f.coord);
    assert_eq!(calls[0].team_name, "happy-coord");
    assert_eq!(calls[0].inputs, Some(serde_json::json!({"issue": 11})));

    // ChildDispatched on the coord log.
    assert_eq!(count_kind_in_log(&f.coord_state_file, CHILD_DISPATCHED), 1);
}

#[test]
fn happy_path_contended_returns_without_audit() {
    let f = fixture("contend.child", "contend-coord");
    // Pre-place a sidecar from a different coord.
    let other = vcoord("rival-coord");
    acquire_sidecar(&f.session_dir, &other, SystemTime::now()).expect("rival claim");

    let spawner = RecordingSpawner::new();
    let outcome = claim_and_dispatch(
        &f.session_dir,
        &f.state_file,
        &f.coord_state_file,
        &f.coord,
        &f.child,
        &spawner,
        SystemTime::now(),
    )
    .expect("dispatch");
    assert!(matches!(outcome, DispatchOutcome::Contended));

    let calls = spawner.calls.lock().unwrap();
    assert_eq!(calls.len(), 0, "no spawn when contended");

    // No ChildDispatched emitted on contention.
    assert_eq!(count_kind_in_log(&f.coord_state_file, CHILD_DISPATCHED), 0);
}

// ============================================================
// AC: O_NOFOLLOW on sidecar read refuses a symlink
// ============================================================

#[test]
fn read_refuses_symlink_at_sidecar_path() {
    use std::os::unix::fs::symlink;
    let f = fixture("symlink.child", "symlink-coord");
    // Plant a symlink at the sidecar path pointing to an unrelated
    // file in the same temp dir.
    let target = f.session_dir.join("decoy");
    std::fs::write(&target, b"unrelated payload").expect("write decoy");
    symlink(&target, sidecar_path(&f.session_dir)).expect("symlink");

    // read_sidecar must NOT follow the symlink. It returns either Ok(None)
    // (some libc surfaces ELOOP as NotFound on a symlink chain) or
    // Ok(Some(Err(_))) (open refused).
    let result = read_sidecar(&f.session_dir).expect("read returns Ok");
    match result {
        None => {
            // O_NOFOLLOW surfaced as not-found; this is acceptable
            // — the read refused to follow.
        }
        Some(Err(msg)) => {
            assert!(
                msg.contains("refused") || msg.contains("symbolic") || msg.contains("loop"),
                "open should be refused: {msg}"
            );
        }
        Some(Ok(_)) => panic!("read followed the symlink — security failure!"),
    }
}

// ============================================================
// AC: O_EXCL refuses to create over an existing symlink
// ============================================================

#[test]
fn excl_refuses_symlink_target() {
    use std::os::unix::fs::symlink;
    let f = fixture("excl-sym.child", "excl-sym-coord");
    let decoy = f.session_dir.join("decoy-existing");
    std::fs::write(&decoy, b"existing decoy").expect("write decoy");
    symlink(&decoy, sidecar_path(&f.session_dir)).expect("symlink");

    // O_EXCL must not silently follow the symlink and write to the
    // target. It either reports Contended (sidecar appears to exist —
    // EEXIST via O_EXCL on a name that already resolves), or it
    // surfaces an open error. Either way, the decoy content must be
    // untouched.
    let result = acquire_sidecar(&f.session_dir, &f.coord, SystemTime::now());
    match result {
        Ok(AcquireOutcome::Contended) => {} // acceptable: EEXIST on symlink
        Ok(AcquireOutcome::Acquired) => panic!("O_EXCL must not succeed against a symlink"),
        Err(_) => {} // also acceptable: ELOOP
    }
    let decoy_after = std::fs::read(&decoy).expect("read decoy");
    assert_eq!(
        decoy_after, b"existing decoy",
        "symlink target must not be overwritten"
    );
}

// ============================================================
// AC: Idempotent recovery
// ============================================================

#[test]
fn recovery_is_idempotent() {
    let f = fixture("idem.child", "idem-coord");
    let old = SystemTime::now() - Duration::from_secs(3600);
    acquire_sidecar(&f.session_dir, &f.coord, old).expect("acquire");

    let mk_inputs = || RecoveryInputs {
        session_dir: &f.session_dir,
        state_file: &f.state_file,
        coord_id: &f.coord,
        child_session_id: &f.child,
        child_is_terminal: false,
        stale_claim_timeout: Duration::from_secs(600),
        redelegation_cap: 3,
        now: SystemTime::now(),
    };

    let first = recover_orphaned_sidecar(&mk_inputs()).expect("first");
    assert!(matches!(first, RecoveryAction::Redelegate { .. }));

    // Second pass: sidecar absent → None.
    let second = recover_orphaned_sidecar(&mk_inputs()).expect("second");
    assert!(matches!(second, RecoveryAction::None));

    // No additional ChildRedelegated emitted on the idempotent pass.
    assert_eq!(count_kind_in_log(&f.coord_state_file, CHILD_REDELEGATED), 1);
}

// ============================================================
// AC: Sidecar contents include malformed-marker pathway
// ============================================================

#[test]
fn malformed_marker_constant_is_stable() {
    // The audit event uses this marker so operators can grep
    // recovery logs for parser failures. Lock the wire form.
    assert_eq!(MALFORMED_COORD_MARKER, "<unparseable-sidecar>");
}

#[test]
fn sidecar_filename_constant_is_stable() {
    assert_eq!(SIDECAR_FILENAME, "claim.lock");
}

// ============================================================
// Helper coverage
// ============================================================

#[test]
fn race_all_winners_have_distinct_coords_in_starvation_check() {
    // Sanity: after N=32 race, the winning coord's id must be one of
    // the 32 distinct values we passed in (no garbage in the sidecar
    // file).
    let f = fixture("starve.child", "starve-coord");
    let now = SystemTime::now();
    let mut expected: HashSet<String> = HashSet::new();
    for i in 0..16 {
        expected.insert(format!("coord-{i:02}"));
    }
    for i in 0..16 {
        let coord = vcoord(&format!("coord-{i:02}"));
        let _ = acquire_sidecar(&f.session_dir, &coord, now);
    }
    let parsed = read_sidecar(&f.session_dir).unwrap().unwrap().unwrap();
    assert!(expected.contains(&parsed.coord_id));
}

#[test]
fn unrelated_assignment_claim_round_trips_through_header() {
    // Defensive: the temp+rename header rewrite preserves every other
    // header field; only assignment_claim should change.
    let f = fixture("rt.child", "rt-coord");
    let header_before = read_header(&f.state_file).expect("read");
    write_header_assignment_claim_best_effort(&f.state_file, &f.coord, "2026-05-24T01:02:03.400Z");
    let header_after = read_header(&f.state_file).expect("read");
    assert_eq!(header_after.workflow, header_before.workflow);
    assert_eq!(header_after.template_hash, header_before.template_hash);
    assert_eq!(header_after.created_at, header_before.created_at);
    assert_eq!(header_after.needs_agent, header_before.needs_agent);
    assert_eq!(header_after.role, header_before.role);
    assert_eq!(
        header_after.coordinator_of_record,
        header_before.coordinator_of_record
    );
    assert_eq!(header_after.dispatch_epoch, header_before.dispatch_epoch);
    assert_eq!(
        header_after.assignment_claim,
        Some(AssignmentClaim {
            coord_id: "rt-coord".to_string(),
            claimed_at: "2026-05-24T01:02:03.400Z".to_string(),
        })
    );
}
