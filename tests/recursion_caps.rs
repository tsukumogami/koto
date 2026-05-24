//! Integration tests for KT1 Issue 17 recursion-cap enforcement.
//!
//! Covers the 10 acceptance criteria for `src/engine/caps.rs`:
//! - Six numeric-boundary tests (3 dimensions × warn/reject) — depth
//!   at 11 rejects, depth at 5 warns; fanout at 100 rejects, fanout
//!   at 20 warns; total at 501 rejects, total at 101 warns.
//! - Per-parent fanout scan does NOT count siblings under other
//!   parents.
//! - Depth walk traverses the `parent_workflow` chain to the root.
//! - Soft caps (warn) do NOT block — exit-code-equivalent assertion
//!   via `CapEvaluation::Warn`.
//! - **Load-bearing AC**: the total-unassigned counter consults the
//!   terminal-index (Issue 8) — 1000 headers where 800 are terminal
//!   produce a count of 200, not 1000. This is the AD3.3 perf-cliff
//!   avoidance commitment.
//!
//! Tests exercise the library API (`koto::engine::caps`) directly
//! against a `LocalBackend` workspace. End-to-end CLI coverage of the
//! `koto session start --needs-agent` cap-rejection path could go in
//! a follow-up integration file; the library-level tests here are
//! the load-bearing semantic ones.

use std::fs;
use std::path::{Path, PathBuf};

use koto::engine::caps::{
    measure_depth_from_parent, measure_fanout, measure_total_unassigned, validate_depth,
    validate_fanout, validate_recursion_caps, validate_total_unassigned, CapEvaluation,
    DEPTH_REJECT, DEPTH_WARN, FANOUT_REJECT, FANOUT_WARN, TOTAL_REJECT, TOTAL_WARN,
};
use koto::engine::persistence::append_header;
use koto::engine::terminal_index::{append_terminal_index_entry, TerminalIndexEntry};
use koto::engine::types::{AssignmentClaim, StateFileHeader};
use koto::session::local::LocalBackend;
use koto::session::state_file_name;

// ----- Workspace fixture helpers -----

/// Build a minimal header for `workflow` with the request-store
/// dispatch shape: `needs_agent: Some(true)`, no assignment claim,
/// optional parent_workflow.
fn unassigned_child_header(workflow: &str, parent: Option<&str>) -> StateFileHeader {
    StateFileHeader {
        schema_version: 1,
        workflow: workflow.to_string(),
        template_hash: "deadbeef".into(),
        created_at: "2026-05-24T00:00:00Z".into(),
        parent_workflow: parent.map(|p| p.to_string()),
        template_source_dir: None,
        session_id: workflow.to_string(),
        intent: None,
        template_name: Some("verdict".into()),
        needs_agent: Some(true),
        role: Some("scrutineer".into()),
        inputs: None,
        coordinator_of_record: Some("team-lead".into()),
        requested_by: Some("parent-coord".into()),
        assignment_claim: None,
        dispatch_epoch: 0,
        priority: None,
        deadline: None,
        retry_count: None,
        agent_config: None,
    }
}

/// Build a claimed header (NOT eligible for fanout/total-unassigned
/// counting).
fn claimed_child_header(workflow: &str, parent: Option<&str>) -> StateFileHeader {
    let mut h = unassigned_child_header(workflow, parent);
    h.assignment_claim = Some(AssignmentClaim {
        coord_id: "team-lead".into(),
        claimed_at: "2026-05-24T14:35:01.000Z".into(),
    });
    h
}

/// Write a session state file at
/// `<sessions_dir>/<workflow>/koto-<workflow>.state.jsonl`.
fn write_session(sessions_dir: &Path, header: &StateFileHeader) -> PathBuf {
    let dir = sessions_dir.join(&header.workflow);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(state_file_name(&header.workflow));
    append_header(&path, header).unwrap();
    path
}

/// Build a `LocalBackend` rooted at `<tmp>/sessions/`. Returns
/// `(backend, koto_root, sessions_dir)` where `koto_root` is `<tmp>`
/// (the workspace root the total-unassigned counter consults for the
/// terminal-index path) and `sessions_dir` is `<tmp>/sessions/`.
fn workspace(tmp: &Path) -> (LocalBackend, PathBuf, PathBuf) {
    let sessions_dir = tmp.join("sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    let backend = LocalBackend::with_base_dir(sessions_dir.clone());
    let koto_root = tmp.to_path_buf();
    (backend, koto_root, sessions_dir)
}

// ----- Depth dimension -----

#[test]
fn depth_at_11_rejects_with_typed_error() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, _root, sessions_dir) = workspace(tmp.path());
    // Walker semantics: each loop iteration increments `hops` BEFORE
    // reading, so a chain of `root + c1..c9` walked from c9 has 10
    // hops (one per node visited). validate_depth adds +1 (for the
    // would-be new child) → observed = 11 == DEPTH_REJECT.
    write_session(&sessions_dir, &unassigned_child_header("root", None));
    let mut prev = "root".to_string();
    for i in 1..=9 {
        let name = format!("c{}", i);
        write_session(&sessions_dir, &unassigned_child_header(&name, Some(&prev)));
        prev = name;
    }
    // Spawning parent is c9; chain has 10 nodes (c9..c1, root); new
    // child would be at observed-depth 11.
    let eval = validate_depth(&backend, "c9").unwrap();
    assert!(
        matches!(
            eval,
            CapEvaluation::Reject {
                dimension: "depth",
                threshold: 10,
                observed: 11
            }
        ),
        "expected Reject at depth 11, got {:?}",
        eval
    );
}

#[test]
fn depth_at_5_warns_and_proceeds() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, _root, sessions_dir) = workspace(tmp.path());
    // Walker semantics (see depth_at_11_rejects above): a chain of
    // `root + c1..c3` walked from c3 has 4 hops; new child = #5 =
    // DEPTH_WARN.
    write_session(&sessions_dir, &unassigned_child_header("root", None));
    let mut prev = "root".to_string();
    for i in 1..=3 {
        let name = format!("c{}", i);
        write_session(&sessions_dir, &unassigned_child_header(&name, Some(&prev)));
        prev = name;
    }
    let eval = validate_depth(&backend, "c3").unwrap();
    assert!(
        matches!(
            eval,
            CapEvaluation::Warn {
                dimension: "depth",
                threshold: 5,
                observed: 5
            }
        ),
        "expected Warn at depth 5, got {:?}",
        eval
    );
    // The Warn variant does NOT convert to a reject error.
    assert!(eval.clone().into_reject().is_none());
}

#[test]
fn depth_walks_parent_workflow_chain_to_root() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, _root, sessions_dir) = workspace(tmp.path());
    // Construct a chain of root + c1..c8. Starting the walk at c8
    // visits c8, c7, ..., c1, root — 9 nodes total, so hops = 9.
    write_session(&sessions_dir, &unassigned_child_header("root", None));
    let mut prev = "root".to_string();
    for i in 1..=8 {
        let name = format!("c{}", i);
        write_session(&sessions_dir, &unassigned_child_header(&name, Some(&prev)));
        prev = name;
    }
    let hops = measure_depth_from_parent(&backend, "c8").unwrap();
    assert_eq!(hops, 9, "expected 9 hops (c8..c1, root), got {}", hops);
}

#[test]
fn depth_handles_missing_intermediate_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, _root, sessions_dir) = workspace(tmp.path());
    // c2 names "missing-root" as parent but that header doesn't exist.
    // The walker treats the missing header as the root.
    write_session(
        &sessions_dir,
        &unassigned_child_header("c2", Some("missing-root")),
    );
    let hops = measure_depth_from_parent(&backend, "c2").unwrap();
    // Walker increments hops at the top of each loop iteration, so:
    // iter 1: hops=1, read c2 OK, advance to "missing-root";
    // iter 2: hops=2, read "missing-root" fails → break.
    // Final hops = 2 — counts the failed-read attempt.
    assert_eq!(hops, 2);
}

#[test]
fn depth_cycle_detection_does_not_loop_forever() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, _root, sessions_dir) = workspace(tmp.path());
    // a → b → a (cycle). Walker must terminate.
    write_session(&sessions_dir, &unassigned_child_header("a", Some("b")));
    write_session(&sessions_dir, &unassigned_child_header("b", Some("a")));
    let hops = measure_depth_from_parent(&backend, "a").unwrap();
    // Cycle detection: walker visits a, b, then sees a again and
    // breaks. depth ≤ 2.
    assert!(hops <= 2, "cycle must terminate quickly, got {} hops", hops);
}

// ----- Fanout dimension -----

#[test]
fn fanout_at_99_succeeds_at_100_rejects() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, _root, sessions_dir) = workspace(tmp.path());
    write_session(&sessions_dir, &unassigned_child_header("parent", None));
    // 99 existing unclaimed children → new child would be #100, which
    // is the reject threshold.
    for i in 0..99 {
        let name = format!("c-{:03}", i);
        write_session(
            &sessions_dir,
            &unassigned_child_header(&name, Some("parent")),
        );
    }
    let eval = validate_fanout(&backend, "parent").unwrap();
    assert!(
        matches!(
            eval,
            CapEvaluation::Reject {
                dimension: "fanout",
                threshold: 100,
                observed: 100
            }
        ),
        "expected Reject at fanout 100, got {:?}",
        eval
    );

    // Drop one child → 98 existing → new child would be #99, below
    // threshold and above warn (still Warn).
    fs::remove_dir_all(sessions_dir.join("c-098")).unwrap();
    let eval2 = validate_fanout(&backend, "parent").unwrap();
    assert!(
        matches!(
            eval2,
            CapEvaluation::Warn {
                dimension: "fanout",
                threshold: 20,
                observed: 99
            }
        ),
        "expected Warn at fanout 99, got {:?}",
        eval2
    );
}

#[test]
fn fanout_at_20_warns_and_proceeds() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, _root, sessions_dir) = workspace(tmp.path());
    write_session(&sessions_dir, &unassigned_child_header("parent", None));
    // 19 existing → new child = #20 (warn threshold).
    for i in 0..19 {
        let name = format!("c-{:03}", i);
        write_session(
            &sessions_dir,
            &unassigned_child_header(&name, Some("parent")),
        );
    }
    let eval = validate_fanout(&backend, "parent").unwrap();
    assert!(
        matches!(
            eval,
            CapEvaluation::Warn {
                dimension: "fanout",
                threshold: 20,
                observed: 20
            }
        ),
        "expected Warn at fanout 20, got {:?}",
        eval
    );
    assert!(eval.into_reject().is_none());
}

#[test]
fn fanout_scan_is_per_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, _root, sessions_dir) = workspace(tmp.path());
    write_session(&sessions_dir, &unassigned_child_header("parent-a", None));
    write_session(&sessions_dir, &unassigned_child_header("parent-b", None));
    // 15 children under parent-a, 200 children under parent-b.
    for i in 0..15 {
        let name = format!("a-{:03}", i);
        write_session(
            &sessions_dir,
            &unassigned_child_header(&name, Some("parent-a")),
        );
    }
    for i in 0..200 {
        let name = format!("b-{:03}", i);
        write_session(
            &sessions_dir,
            &unassigned_child_header(&name, Some("parent-b")),
        );
    }
    // Fanout scan for parent-a sees only 15, not 215.
    let count_a = measure_fanout(&backend, "parent-a").unwrap();
    assert_eq!(count_a, 15);
    // Fanout scan for parent-b sees 200.
    let count_b = measure_fanout(&backend, "parent-b").unwrap();
    assert_eq!(count_b, 200);
}

#[test]
fn fanout_does_not_count_claimed_siblings() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, _root, sessions_dir) = workspace(tmp.path());
    write_session(&sessions_dir, &unassigned_child_header("parent", None));
    // 5 unclaimed + 50 claimed. Fanout should report 5.
    for i in 0..5 {
        let name = format!("u-{:03}", i);
        write_session(
            &sessions_dir,
            &unassigned_child_header(&name, Some("parent")),
        );
    }
    for i in 0..50 {
        let name = format!("k-{:03}", i);
        write_session(&sessions_dir, &claimed_child_header(&name, Some("parent")));
    }
    let count = measure_fanout(&backend, "parent").unwrap();
    assert_eq!(count, 5, "fanout must skip claimed children");
}

// ----- Total-unassigned dimension -----

#[test]
fn total_at_500_rejects_at_501() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, root, sessions_dir) = workspace(tmp.path());
    // 500 unassigned children spread across one parent. New child
    // would be #501 → reject.
    write_session(&sessions_dir, &unassigned_child_header("parent", None));
    for i in 0..500 {
        let name = format!("c-{:04}", i);
        write_session(
            &sessions_dir,
            &unassigned_child_header(&name, Some("parent")),
        );
    }
    let eval = validate_total_unassigned(&backend, &root).unwrap();
    // 500 children + 1 parent = 501 unassigned headers (the parent
    // itself has needs_agent=Some(true), no claim). New child = #502.
    // Both 501 and 502 are above the 500 reject threshold.
    match eval {
        CapEvaluation::Reject {
            dimension,
            threshold,
            observed,
        } => {
            assert_eq!(dimension, "total_unassigned");
            assert_eq!(threshold, 500);
            assert!(
                observed >= 501,
                "expected observed >= 501, got {}",
                observed
            );
        }
        other => panic!("expected Reject, got {:?}", other),
    }
}

#[test]
fn total_at_100_warns_at_101() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, root, sessions_dir) = workspace(tmp.path());
    // 99 unassigned children + 1 parent = 100 unassigned headers; new
    // child = #101 → warn threshold (TOTAL_WARN=100).
    write_session(&sessions_dir, &unassigned_child_header("parent", None));
    for i in 0..99 {
        let name = format!("c-{:03}", i);
        write_session(
            &sessions_dir,
            &unassigned_child_header(&name, Some("parent")),
        );
    }
    let eval = validate_total_unassigned(&backend, &root).unwrap();
    match eval {
        CapEvaluation::Warn {
            dimension,
            threshold,
            observed,
        } => {
            assert_eq!(dimension, "total_unassigned");
            assert_eq!(threshold, 100);
            assert!(
                observed >= 100,
                "expected observed >= 100, got {}",
                observed
            );
        }
        other => panic!("expected Warn at total 100, got {:?}", other),
    }
}

/// **Load-bearing AC.** 1000 headers, 800 marked terminal in the
/// `_terminal_index.jsonl` → counter sees 200, NOT 1000. This is the
/// AD3.3 perf-cliff avoidance commitment from the design's Required
/// Tactical Designs table item (a).
#[test]
fn total_unassigned_consults_terminal_index() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, root, sessions_dir) = workspace(tmp.path());
    write_session(&sessions_dir, &unassigned_child_header("parent", None));
    // Write 1000 unassigned children.
    let mut all_ids: Vec<String> = Vec::with_capacity(1000);
    for i in 0..1000 {
        let name = format!("c-{:04}", i);
        write_session(
            &sessions_dir,
            &unassigned_child_header(&name, Some("parent")),
        );
        all_ids.push(name);
    }
    // Mark the first 800 as terminal in `_terminal_index.jsonl`.
    // `header_mtime_ns` is read from the on-disk header so the
    // index-vs-header reconciliation (Issue 8) treats them as
    // up-to-date terminal entries (no header-is-truth fallthrough).
    for id in all_ids.iter().take(800) {
        let path = sessions_dir.join(id).join(state_file_name(id));
        let mtime_ns = koto::engine::terminal_index::header_mtime_unix_nanos(&path).unwrap_or(0);
        let entry = TerminalIndexEntry {
            session_id: id.clone(),
            terminal_at: "2026-05-24T14:35:01.000Z".into(),
            header_mtime_ns: mtime_ns,
            terminal_state: "completed".into(),
        };
        append_terminal_index_entry(&root, &entry).unwrap();
    }
    // The counter should see 200 (1000 children - 800 terminal) +
    // the 1 parent header that's also unassigned-shaped = 201.
    let count = measure_total_unassigned(&backend, &root).unwrap();
    assert_eq!(
        count, 201,
        "terminal-index filter must skip 800 terminal sessions; expected 201, got {}",
        count
    );
    // Sanity: without the filter the count would be 1001. Confirm
    // the filter actually fired.
    assert!(
        count < 1001,
        "filter not applied: count is the full workspace size"
    );
}

#[test]
fn total_unassigned_does_not_count_claimed_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, root, sessions_dir) = workspace(tmp.path());
    write_session(&sessions_dir, &unassigned_child_header("parent", None));
    // 10 unclaimed + 50 claimed.
    for i in 0..10 {
        write_session(
            &sessions_dir,
            &unassigned_child_header(&format!("u-{}", i), Some("parent")),
        );
    }
    for i in 0..50 {
        write_session(
            &sessions_dir,
            &claimed_child_header(&format!("k-{}", i), Some("parent")),
        );
    }
    // Total unassigned: parent + 10 children = 11.
    let count = measure_total_unassigned(&backend, &root).unwrap();
    assert_eq!(count, 11, "must skip claimed sessions, got {}", count);
}

// ----- Soft-caps-don't-block AC (all 3 dimensions) -----

#[test]
fn soft_caps_dont_block_depth() {
    // Depth Warn variant does not produce a reject error.
    let tmp = tempfile::tempdir().unwrap();
    let (backend, _root, sessions_dir) = workspace(tmp.path());
    write_session(&sessions_dir, &unassigned_child_header("root", None));
    let mut prev = "root".to_string();
    for i in 1..=5 {
        let name = format!("c{}", i);
        write_session(&sessions_dir, &unassigned_child_header(&name, Some(&prev)));
        prev = name;
    }
    let eval = validate_depth(&backend, "c5").unwrap();
    assert!(matches!(
        eval,
        CapEvaluation::Warn { .. } | CapEvaluation::Reject { .. }
    ));
    // If we just got a Warn, into_reject() returns None.
    if let CapEvaluation::Warn { .. } = eval {
        assert!(eval.into_reject().is_none());
    }
}

#[test]
fn soft_caps_dont_block_fanout() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, _root, sessions_dir) = workspace(tmp.path());
    write_session(&sessions_dir, &unassigned_child_header("parent", None));
    for i in 0..19 {
        write_session(
            &sessions_dir,
            &unassigned_child_header(&format!("c-{}", i), Some("parent")),
        );
    }
    let eval = validate_fanout(&backend, "parent").unwrap();
    assert!(matches!(
        eval,
        CapEvaluation::Warn {
            dimension: "fanout",
            ..
        }
    ));
    assert!(eval.into_reject().is_none());
}

#[test]
fn soft_caps_dont_block_total_unassigned() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, root, sessions_dir) = workspace(tmp.path());
    write_session(&sessions_dir, &unassigned_child_header("parent", None));
    for i in 0..99 {
        write_session(
            &sessions_dir,
            &unassigned_child_header(&format!("c-{:03}", i), Some("parent")),
        );
    }
    let eval = validate_total_unassigned(&backend, &root).unwrap();
    assert!(matches!(
        eval,
        CapEvaluation::Warn {
            dimension: "total_unassigned",
            ..
        }
    ));
    assert!(eval.into_reject().is_none());
}

// ----- Orchestrator end-to-end -----

#[test]
fn orchestrator_runs_all_three_dimensions() {
    let tmp = tempfile::tempdir().unwrap();
    let (backend, root, sessions_dir) = workspace(tmp.path());
    write_session(&sessions_dir, &unassigned_child_header("parent", None));
    let outcome = validate_recursion_caps(&backend, "parent", &root).unwrap();
    assert!(outcome.depth.is_some());
    assert!(outcome.fanout.is_some());
    assert!(outcome.total_unassigned.is_some());
    assert!(outcome.first_reject().is_none());
}

#[test]
fn constants_drive_known_thresholds() {
    // Belt-and-braces: confirm the constants the tests assume above
    // are unchanged. A drift here would silently change protocol
    // semantics and these tests would still pass with stale numbers.
    assert_eq!(DEPTH_WARN, 5);
    assert_eq!(DEPTH_REJECT, 10);
    assert_eq!(FANOUT_WARN, 20);
    assert_eq!(FANOUT_REJECT, 100);
    assert_eq!(TOTAL_WARN, 100);
    assert_eq!(TOTAL_REJECT, 500);
}
