//! Integration tests for `koto::engine::terminal_index` compaction
//! lease (Issue 9).
//!
//! Inline unit tests in `src/engine/terminal_index.rs` cover threshold
//! gating, lease EEXIST handling, dedup-with-first-appearance-order,
//! stale-lock recovery's three branches (foreign+stale, own+stale,
//! foreign+fresh), missing-lock no-op, and malformed-lock reclaim.
//! This file exercises the cross-cutting ACs:
//!
//! - **Lease acquisition race (N=2 concurrent coords)**: exactly one
//!   `O_EXCL` syscall succeeds; the loser sees `EEXIST` and skips.
//! - **Crash-mid-compaction recovery**: synthesize foreign-coord stale
//!   lock + residual `.tmp` + intact original index; the recovery
//!   walk unlinks both auxiliaries and leaves the original untouched.
//! - **Compaction is semantics-preserving end-to-end**: read the
//!   reader's dedup map pre- and post-compact and confirm equality.

use std::path::Path;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use koto::config::RequestStoreConfig;
use koto::engine::terminal_index::{
    append_terminal_index_entry, compact_lock_path, drain_terminal_index_ordered,
    maybe_compact_terminal_index, read_terminal_index, recover_stale_compact_lock, temp_index_path,
    terminal_index_path, CompactLock, CompactionOutcome, SkippedReason, TerminalIndexEntry,
};

fn fixture_entry(sid: &str, mtime_ns: u64, state: &str) -> TerminalIndexEntry {
    TerminalIndexEntry {
        session_id: sid.to_string(),
        terminal_at: "2026-05-24T14:35:01.000Z".to_string(),
        header_mtime_ns: mtime_ns,
        terminal_state: state.to_string(),
        has_result: false,
    }
}

fn cfg(threshold: u64, timeout_seconds: u64) -> RequestStoreConfig {
    RequestStoreConfig {
        terminal_index_compact_lines: threshold,
        compact_lock_timeout_seconds: timeout_seconds,
        ..RequestStoreConfig::default()
    }
}

fn write_lock(root: &Path, lock: &CompactLock) {
    std::fs::create_dir_all(root).unwrap();
    let s = toml::to_string(lock).unwrap();
    std::fs::write(compact_lock_path(root), s).unwrap();
}

// ----- Lease acquisition race AC ------------------------------------------

#[test]
fn two_coordinators_race_on_compaction_only_one_runs_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = Arc::new(tmp.path().to_path_buf());

    // Populate the index above threshold so both coords would
    // legitimately attempt compaction in isolation.
    for i in 0..50 {
        append_terminal_index_entry(
            tmp.path(),
            &fixture_entry(&format!("s{:02}", i), (i as u64) + 1, "completed"),
        )
        .unwrap();
    }

    let cfg_shared = Arc::new(cfg(50, 3600));
    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::with_capacity(2);
    for name in ["coord-a", "coord-b"] {
        let root_c = Arc::clone(&root);
        let cfg_c = Arc::clone(&cfg_shared);
        let barrier_c = Arc::clone(&barrier);
        let coord = name.to_string();
        handles.push(thread::spawn(move || {
            // Sync the two threads so both call into the lease
            // acquire at the same time. The kernel's atomic O_EXCL
            // settles the race.
            barrier_c.wait();
            maybe_compact_terminal_index(&root_c, &coord, &cfg_c).unwrap()
        }));
    }

    let outcomes: Vec<CompactionOutcome> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    // Exactly one Compacted and exactly one Skipped { LeaseHeldByPeer }.
    let compacted_count = outcomes
        .iter()
        .filter(|o| matches!(o, CompactionOutcome::Compacted { .. }))
        .count();
    let skipped_count = outcomes
        .iter()
        .filter(|o| {
            matches!(
                o,
                CompactionOutcome::Skipped {
                    reason: SkippedReason::LeaseHeldByPeer
                }
            )
        })
        .count();
    assert_eq!(
        compacted_count, 1,
        "exactly one of the racing coords must compact: {:?}",
        outcomes
    );
    assert_eq!(
        skipped_count, 1,
        "exactly one of the racing coords must lose the race: {:?}",
        outcomes
    );

    // Lease released regardless of which coord won.
    assert!(!compact_lock_path(tmp.path()).exists());
    // Temp file cleaned up by the atomic rename.
    assert!(!temp_index_path(tmp.path()).exists());
    // Compaction was semantics-preserving (no dupes in the fixture).
    let post = drain_terminal_index_ordered(tmp.path()).unwrap();
    assert_eq!(post.len(), 50);
}

// ----- Crash-mid-compaction recovery AC -----------------------------------

#[test]
fn crash_mid_compaction_recovery_removes_lock_and_tmp_preserves_original() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Synthesize the post-crash workspace: a complete original index,
    // a foreign-coord stale lock past timeout, and a residual temp
    // file with the crashing coord's partial output.
    for i in 0..5 {
        append_terminal_index_entry(
            root,
            &fixture_entry(&format!("s{}", i), (i as u64) + 1, "completed"),
        )
        .unwrap();
    }
    let original_contents = std::fs::read_to_string(terminal_index_path(root)).unwrap();
    let original_byte_count = original_contents.as_bytes().len();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    write_lock(
        root,
        &CompactLock {
            coord_id: "dead-peer".into(),
            started_at: "2026-05-24T12:00:00.000Z".into(),
            started_at_unix_seconds: now.saturating_sub(7200),
        },
    );
    std::fs::write(
        temp_index_path(root),
        b"{\"session_id\":\"partial\",\"terminal_at\":\"2026-05-24T14:35:00.000Z\",\"header_mtime_ns\":99,\"terminal_state\":\"completed\"}\n",
    )
    .unwrap();

    let reclaimed =
        recover_stale_compact_lock(root, "coord-recovery", &cfg(100_000, 3600)).unwrap();
    assert!(
        reclaimed,
        "stale lock past timeout + foreign coord must reclaim"
    );
    assert!(!compact_lock_path(root).exists());
    assert!(!temp_index_path(root).exists());

    // The original index file is untouched.
    let post_contents = std::fs::read_to_string(terminal_index_path(root)).unwrap();
    assert_eq!(post_contents, original_contents);
    assert_eq!(post_contents.as_bytes().len(), original_byte_count);

    // A subsequent compaction (above threshold) acquires a fresh lease
    // and runs to completion.
    let outcome = maybe_compact_terminal_index(root, "coord-recovery", &cfg(5, 3600)).unwrap();
    assert!(matches!(outcome, CompactionOutcome::Compacted { .. }));
}

// ----- Compaction is semantics-preserving end-to-end ----------------------

#[test]
fn reader_dedup_map_invariant_holds_across_compaction() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Build a workspace with duplicates so dedup actually has work
    // to do.
    for sid in ["s1", "s2", "s3"] {
        for mtime in [100, 200, 300] {
            append_terminal_index_entry(root, &fixture_entry(sid, mtime, "completed")).unwrap();
        }
    }
    // Pre-compaction: 9 lines, dedup map size 3, each winner at
    // mtime 300.
    let pre = read_terminal_index(root);
    assert_eq!(pre.len(), 3);
    for sid in ["s1", "s2", "s3"] {
        assert_eq!(pre.get(sid).unwrap().header_mtime_ns, 300);
    }

    // Above threshold (9 >= 5).
    let outcome = maybe_compact_terminal_index(root, "coord-a", &cfg(5, 3600)).unwrap();
    match outcome {
        CompactionOutcome::Compacted {
            lines_before,
            lines_after,
        } => {
            assert_eq!(lines_before, 9);
            assert_eq!(lines_after, 3);
        }
        other => panic!("expected Compacted, got {:?}", other),
    }

    // Post-compaction: dedup map identical.
    let post = read_terminal_index(root);
    assert_eq!(post.len(), 3);
    for sid in ["s1", "s2", "s3"] {
        assert_eq!(post.get(sid).unwrap().header_mtime_ns, 300);
    }
    // Equivalence: pre and post produce the same in-memory dedup
    // result. This is the design's "semantics-preserving" contract.
    assert_eq!(pre, post);
}

// ----- Compaction error path: caller releases lease, removes temp ---------

#[test]
fn compaction_error_path_releases_lease_and_removes_temp_when_file_too_long() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Build an oversized entry (the writer's PIPE_BUF guard would
    // normally refuse this on the append path — but if it landed
    // historically via a different writer, the compaction must
    // gracefully fail without corrupting state).
    //
    // We can't append > PIPE_BUF via the normal writer (it would
    // bail), so synthesize by direct file write: 5 valid lines + 1
    // oversized line. The compaction will fail mid-write.
    for i in 0..5 {
        append_terminal_index_entry(
            root,
            &fixture_entry(&format!("s{}", i), (i as u64) + 1, "completed"),
        )
        .unwrap();
    }
    // Append an oversized line directly (bypassing the writer's
    // bound check) so the compaction's serializer hits the limit.
    let huge_sid = "x".repeat(5000);
    let huge_entry = TerminalIndexEntry {
        session_id: huge_sid,
        terminal_at: "2026-05-24T14:35:01.000Z".to_string(),
        header_mtime_ns: 9999,
        terminal_state: "completed".to_string(),
        has_result: false,
    };
    let mut huge_line = serde_json::to_string(&huge_entry).unwrap();
    huge_line.push('\n');
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(terminal_index_path(root))
            .unwrap();
        f.write_all(huge_line.as_bytes()).unwrap();
    }

    // Threshold = 6 → triggers; compaction's per-line bound check
    // refuses the oversize line and surfaces Failed.
    let outcome = maybe_compact_terminal_index(root, "coord-a", &cfg(6, 3600)).unwrap();
    match outcome {
        CompactionOutcome::Failed { error } => {
            assert!(
                error.contains("PIPE_BUF") || error.contains("exceeds"),
                "expected PIPE_BUF bound failure, got {}",
                error
            );
        }
        other => panic!("expected Failed, got {:?}", other),
    }
    // Lease released + temp file removed on the error path.
    assert!(!compact_lock_path(root).exists());
    assert!(!temp_index_path(root).exists());
    // Original index intact (line count unchanged: 5 valid + 1 huge = 6).
    let post = std::fs::read_to_string(terminal_index_path(root)).unwrap();
    let line_count = post.lines().count();
    assert_eq!(line_count, 6, "original index must be untouched on Failed");
}

// ----- Threshold gating is exact, not approximate -------------------------

#[test]
fn threshold_gating_exact_boundary() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // 9 lines, threshold 10 → skipped.
    for i in 0..9 {
        append_terminal_index_entry(
            root,
            &fixture_entry(&format!("s{}", i), (i as u64) + 1, "completed"),
        )
        .unwrap();
    }
    let outcome = maybe_compact_terminal_index(root, "coord-a", &cfg(10, 3600)).unwrap();
    assert!(matches!(
        outcome,
        CompactionOutcome::Skipped {
            reason: SkippedReason::BelowThreshold { .. }
        }
    ));
    // Append one more → 10 lines, threshold 10 → triggers.
    append_terminal_index_entry(root, &fixture_entry("s9", 99, "completed")).unwrap();
    let outcome = maybe_compact_terminal_index(root, "coord-a", &cfg(10, 3600)).unwrap();
    assert!(matches!(outcome, CompactionOutcome::Compacted { .. }));
}
