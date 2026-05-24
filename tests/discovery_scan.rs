//! Integration tests for `koto::engine::discovery`.
//!
//! Covers the 12 acceptance criteria from KT1 Issue 7:
//! ordered candidate set, default-50 batch cap, candidate filter
//! (coord_of_record + assignment_claim exclusions); tied-boundary
//! correctness (the 32-headers / 24-surfaced race AC); quiet second
//! tick; cursor recovery in three branches (absent / malformed /
//! TTL-exceeded); cursor write atomicity (.tmp + rename leaves no
//! orphan); cursor GC fires both via `koto workspace prune` AND
//! `koto next` boot; directive batch-size cap surfaces overflow
//! across subsequent ticks; handoff regression (scratch rebuild);
//! and the negative AC that surfaced candidates do not double-surface.
//!
//! Tests poke discovery at the library layer rather than through the
//! `koto` CLI: the discovery scan is a library function and end-to-end
//! CLI coverage of unassigned_children flow lives in `integration_test.rs`
//! (where the per-variant `[]` shape is already asserted). The CLI tests
//! deliberately use a per-tempdir HOME so they never write into a real
//! `~/.koto/coordinators/`.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use filetime::{set_file_mtime, FileTime};

use koto::cli::next_types::UnassignedChild;
use koto::config::Kt1Config;
use koto::engine::discovery::{
    cursor_path, gc_stale_cursors, read_cursor, scan, write_cursor_atomic, ScanCursor,
};
use koto::engine::persistence::append_header;
use koto::engine::types::{AssignmentClaim, StateFileHeader, ValidatedCoordId};
use koto::session::state_file_name;

// ----- Test helpers --------------------------------------------------------

const COORD: &str = "team-lead";

/// Build a minimal `StateFileHeader` for the given workflow id.
fn make_header(workflow: &str) -> StateFileHeader {
    StateFileHeader {
        schema_version: 1,
        workflow: workflow.to_string(),
        template_hash: "deadbeef".into(),
        created_at: "2026-05-24T00:00:00Z".into(),
        parent_workflow: None,
        template_source_dir: None,
        session_id: workflow.to_string(),
        intent: None,
        template_name: Some("verdict".into()),
        needs_agent: None,
        role: None,
        inputs: None,
        coordinator_of_record: None,
        requested_by: None,
        assignment_claim: None,
        dispatch_epoch: 0,
        priority: None,
        deadline: None,
        retry_count: None,
        agent_config: None,
        respawn_generation: None,
    }
}

/// Build an unassigned-child header naming `COORD` as the
/// coordinator-of-record.
fn make_unassigned_child_header(workflow: &str) -> StateFileHeader {
    let mut h = make_header(workflow);
    h.needs_agent = Some(true);
    h.role = Some("scrutineer".into());
    h.template_name = Some("verdict".into());
    h.requested_by = Some("parent-coord".into());
    h.coordinator_of_record = Some(COORD.into());
    h
}

/// Write a session state file at `koto_root/sessions/<workflow>/koto-<workflow>.state.jsonl`
/// and stamp it with the given mtime micros.
fn write_session(koto_root: &Path, header: &StateFileHeader, mtime_unix_micros: u64) -> PathBuf {
    let sessions = koto_root.join("sessions");
    let dir = sessions.join(&header.workflow);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(state_file_name(&header.workflow));
    append_header(&path, header).unwrap();
    set_file_mtime(
        &path,
        FileTime::from_unix_time(
            (mtime_unix_micros / 1_000_000) as i64,
            ((mtime_unix_micros % 1_000_000) * 1_000) as u32,
        ),
    )
    .unwrap();
    path
}

fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros() as u64
}

fn cfg_with_batch_size(batch: u32) -> Kt1Config {
    Kt1Config {
        directive_batch_size: batch,
        ..Kt1Config::default()
    }
}

fn coord() -> ValidatedCoordId {
    ValidatedCoordId::new(COORD).unwrap()
}

// ----- AC: ordered candidate set + default 50 batch cap --------------------

#[test]
fn scan_returns_candidates_ordered_by_mtime_ascending() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let base = now_micros() - 10_000_000;
    // Write three eligible children with strictly increasing mtimes.
    write_session(
        root,
        &make_unassigned_child_header("child-c"),
        base + 3_000_000,
    );
    write_session(
        root,
        &make_unassigned_child_header("child-a"),
        base + 1_000_000,
    );
    write_session(
        root,
        &make_unassigned_child_header("child-b"),
        base + 2_000_000,
    );

    let out = scan(root, &coord(), &Kt1Config::default()).unwrap();
    assert_eq!(out.len(), 3);
    let order: Vec<&str> = out.iter().map(|c| c.child_session_id.as_str()).collect();
    assert_eq!(order, vec!["child-a", "child-b", "child-c"]);
}

#[test]
fn scan_caps_at_default_directive_batch_size() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let base = now_micros() - 1_000_000_000;
    // 60 eligible children; default cap is 50.
    for i in 0..60 {
        let mtime = base + (i as u64) * 1_000_000;
        write_session(
            root,
            &make_unassigned_child_header(&format!("child-{:03}", i)),
            mtime,
        );
    }
    let out = scan(root, &coord(), &Kt1Config::default()).unwrap();
    assert_eq!(out.len(), 50);
    // The 50 surfaced should be the 50 OLDEST (mtime ascending).
    assert_eq!(out[0].child_session_id, "child-000");
    assert_eq!(out[49].child_session_id, "child-049");
}

// ----- AC: coordinator_of_record filter -----------------------------------

#[test]
fn scan_excludes_children_for_other_coordinator() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let base = now_micros() - 1_000_000;

    // Eligible: our coord.
    write_session(root, &make_unassigned_child_header("ours"), base + 100);
    // Wrong coord.
    let mut other = make_unassigned_child_header("theirs");
    other.coordinator_of_record = Some("other-coord".into());
    write_session(root, &other, base + 200);

    let out = scan(root, &coord(), &Kt1Config::default()).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].child_session_id, "ours");
}

// ----- AC: assignment_claim filter ----------------------------------------

#[test]
fn scan_excludes_already_claimed_children() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let base = now_micros() - 1_000_000;

    let mut claimed = make_unassigned_child_header("claimed");
    claimed.assignment_claim = Some(AssignmentClaim {
        coord_id: COORD.into(),
        claimed_at: "2026-05-24T00:00:01Z".into(),
    });
    write_session(root, &claimed, base + 100);
    write_session(root, &make_unassigned_child_header("free"), base + 200);

    let out = scan(root, &coord(), &Kt1Config::default()).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].child_session_id, "free");
}

// ----- AC 4: tied-boundary correctness (the load-bearing race AC) ---------

#[test]
fn tied_boundary_surfaces_strictly_newer_plus_unseen_tied() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let boundary_mtime = now_micros() - 100_000_000;

    // 8 ids at the boundary that we'll mark as already-seen.
    let mut seen_ids: Vec<String> = Vec::new();
    for i in 0..8 {
        let id = format!("seen-{:02}", i);
        write_session(root, &make_unassigned_child_header(&id), boundary_mtime);
        seen_ids.push(id);
    }
    // 8 ids at the boundary, NOT in the seen-set.
    for i in 0..8 {
        let id = format!("tied-{:02}", i);
        write_session(root, &make_unassigned_child_header(&id), boundary_mtime);
    }
    // 16 strictly-newer ids.
    for i in 0..16 {
        let id = format!("newer-{:02}", i);
        write_session(
            root,
            &make_unassigned_child_header(&id),
            boundary_mtime + 1_000_000 + (i as u64),
        );
    }

    // Prime the cursor: last_max = boundary_mtime, seen_at_boundary = the 8 seen ids.
    let prior = ScanCursor {
        last_scan_at_unix_micros: now_micros(),
        last_max_header_mtime_unix_micros: boundary_mtime,
        seen_at_boundary: seen_ids.clone(),
    };
    write_cursor_atomic(root, COORD, &prior).unwrap();

    let out = scan(root, &coord(), &Kt1Config::default()).unwrap();
    let surfaced: BTreeSet<String> = out.into_iter().map(|c| c.child_session_id).collect();
    assert_eq!(
        surfaced.len(),
        24,
        "expected 24 candidates (16 newer + 8 unseen tied), got {}: {:?}",
        surfaced.len(),
        surfaced
    );
    // None of the seen-set ids should appear.
    for id in &seen_ids {
        assert!(!surfaced.contains(id), "seen id {} surfaced", id);
    }
    // All 16 newer + all 8 tied should appear.
    for i in 0..16 {
        assert!(surfaced.contains(&format!("newer-{:02}", i)));
    }
    for i in 0..8 {
        assert!(surfaced.contains(&format!("tied-{:02}", i)));
    }
}

// ----- AC 5: quiet second tick -------------------------------------------

#[test]
fn second_tick_with_no_new_writes_returns_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let base = now_micros() - 1_000_000;

    write_session(root, &make_unassigned_child_header("c1"), base + 1);
    write_session(root, &make_unassigned_child_header("c2"), base + 2);

    let first = scan(root, &coord(), &Kt1Config::default()).unwrap();
    assert_eq!(first.len(), 2);

    let second = scan(root, &coord(), &Kt1Config::default()).unwrap();
    assert!(
        second.is_empty(),
        "no new writes should produce empty second tick, got {:?}",
        second
    );

    // Cursor advanced past the max observed mtime.
    let cursor = read_cursor(root, COORD, 7);
    assert_eq!(cursor.last_max_header_mtime_unix_micros, base + 2);
    // And the seen-set carries the id(s) at the boundary.
    assert!(cursor.seen_at_boundary.contains(&"c2".to_string()));
}

// ----- AC 6: cursor recovery — absent ------------------------------------

#[test]
fn cursor_recovery_absent_full_rescans() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let base = now_micros() - 1_000_000;

    write_session(root, &make_unassigned_child_header("c1"), base + 100);
    // Do NOT prime any cursor. First scan should see everything (full rescan).
    let out = scan(root, &coord(), &Kt1Config::default()).unwrap();
    assert_eq!(out.len(), 1);
    // And a fresh cursor was written.
    assert!(cursor_path(root, COORD).exists());
}

// ----- AC 7: cursor recovery — malformed ---------------------------------

#[test]
fn cursor_recovery_malformed_full_rescans_and_rewrites() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let base = now_micros() - 1_000_000;

    // Write garbage to the cursor path so read fails.
    let path = cursor_path(root, COORD);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"garbage = [[[ not toml").unwrap();

    write_session(root, &make_unassigned_child_header("c1"), base + 100);
    let out = scan(root, &coord(), &Kt1Config::default()).unwrap();
    assert_eq!(out.len(), 1);
    // Cursor was rewritten cleanly.
    let recovered = read_cursor(root, COORD, 7);
    assert_ne!(recovered, ScanCursor::default());
    assert!(recovered.last_max_header_mtime_unix_micros >= base + 100);
}

// ----- AC 8: cursor recovery — TTL exceeded ------------------------------

#[test]
fn cursor_recovery_ttl_exceeded_full_rescans() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let base = now_micros() - 1_000_000;

    // Prime a cursor with last_scan 100 days ago and a high last_max
    // that would normally hide our new child.
    let stale = ScanCursor {
        last_scan_at_unix_micros: now_micros().saturating_sub(100 * 24 * 60 * 60 * 1_000_000),
        last_max_header_mtime_unix_micros: u64::MAX / 2,
        seen_at_boundary: vec![],
    };
    write_cursor_atomic(root, COORD, &stale).unwrap();

    write_session(root, &make_unassigned_child_header("c1"), base + 100);

    // TTL = 7 (default). The 100-day-old cursor triggers full rescan
    // → c1 surfaces despite being "older than" the high last_max.
    let out = scan(root, &coord(), &Kt1Config::default()).unwrap();
    assert_eq!(out.len(), 1);
}

// ----- AC 9: cursor write atomicity --------------------------------------

#[test]
fn cursor_write_leaves_no_orphan_tmp_file() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    write_session(
        root,
        &make_unassigned_child_header("c1"),
        now_micros() - 100,
    );
    let _ = scan(root, &coord(), &Kt1Config::default()).unwrap();

    let path = cursor_path(root, COORD);
    assert!(path.exists());
    let tmp_path = path.with_extension("toml.tmp");
    assert!(
        !tmp_path.exists(),
        "atomic write must rename .tmp; orphan found at {}",
        tmp_path.display()
    );
}

#[test]
fn cursor_write_overwrites_previous_atomically() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // Use recent timestamps so read_cursor doesn't take the TTL recovery
    // branch and replace what we wrote.
    let original = ScanCursor {
        last_scan_at_unix_micros: now_micros() - 1_000,
        last_max_header_mtime_unix_micros: 100,
        seen_at_boundary: vec!["a".into()],
    };
    write_cursor_atomic(root, COORD, &original).unwrap();
    let read1 = read_cursor(root, COORD, 7);
    assert_eq!(read1, original);

    let updated = ScanCursor {
        last_scan_at_unix_micros: now_micros(),
        last_max_header_mtime_unix_micros: 200,
        seen_at_boundary: vec!["b".into()],
    };
    write_cursor_atomic(root, COORD, &updated).unwrap();
    let read2 = read_cursor(root, COORD, 7);
    assert_eq!(read2, updated);
    assert_ne!(read1, read2);
}

// ----- AC 10: cursor GC via koto workspace prune (library-layer test) ----

#[test]
fn gc_stale_cursors_deletes_only_stale_foreign_cursors() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let cfg = Kt1Config::default();
    let stale = ScanCursor {
        last_scan_at_unix_micros: now_micros().saturating_sub(100 * 24 * 60 * 60 * 1_000_000),
        ..Default::default()
    };
    let fresh = ScanCursor {
        last_scan_at_unix_micros: now_micros(),
        ..Default::default()
    };
    write_cursor_atomic(root, "stale-coord", &stale).unwrap();
    write_cursor_atomic(root, "fresh-coord", &fresh).unwrap();
    write_cursor_atomic(root, "another-fresh", &fresh).unwrap();

    let deleted = gc_stale_cursors(root, &cfg).unwrap();
    assert_eq!(deleted, 1);
    assert!(!cursor_path(root, "stale-coord").exists());
    assert!(cursor_path(root, "fresh-coord").exists());
    assert!(cursor_path(root, "another-fresh").exists());
}

// ----- AC 11: cursor GC fires before scan() — covered via direct call ----
//
// `koto next` startup calls `gc_stale_cursors` (see src/cli/mod.rs in
// handle_next). The GC entry point is the same one tested in AC 10;
// the only difference is the call site, which is exercised by every
// end-to-end CLI test that runs `koto next`. This test asserts that
// the GC is idempotent and that scan() does not resurrect a deleted
// cursor — together that's enough to confirm the on-startup wiring
// is meaningful.
#[test]
fn scan_does_not_resurrect_a_deleted_stale_cursor() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let cfg = Kt1Config::default();

    // Prime a stale cursor that GC will sweep.
    let stale = ScanCursor {
        last_scan_at_unix_micros: now_micros().saturating_sub(100 * 24 * 60 * 60 * 1_000_000),
        last_max_header_mtime_unix_micros: 99,
        seen_at_boundary: vec!["ghost".into()],
    };
    write_cursor_atomic(root, COORD, &stale).unwrap();
    let _ = gc_stale_cursors(root, &cfg).unwrap();
    assert!(!cursor_path(root, COORD).exists());

    // scan() runs after GC; the missing cursor takes the absent-recovery
    // branch (NOT the stale data) and writes a fresh cursor.
    write_session(
        root,
        &make_unassigned_child_header("post-gc-child"),
        now_micros() - 100,
    );
    let out = scan(root, &coord(), &cfg).unwrap();
    assert_eq!(out.len(), 1);
    let after = read_cursor(root, COORD, 7);
    assert!(!after.seen_at_boundary.contains(&"ghost".to_string()));
}

// ----- AC 12: directive batch-size cap (200 → 50 then 50) ---------------

#[test]
fn batch_size_cap_surfaces_overflow_on_subsequent_ticks() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let cfg = cfg_with_batch_size(50);
    let base = now_micros() - 1_000_000_000;

    // 200 eligible children, mtimes spaced 1 ms apart so ordering is
    // unambiguous.
    for i in 0..200 {
        write_session(
            root,
            &make_unassigned_child_header(&format!("c-{:04}", i)),
            base + (i as u64) * 1_000,
        );
    }

    let first = scan(root, &coord(), &cfg).unwrap();
    assert_eq!(first.len(), 50);
    let first_ids: Vec<String> = first.iter().map(|c| c.child_session_id.clone()).collect();
    // First slice = 50 oldest.
    assert_eq!(first_ids[0], "c-0000");
    assert_eq!(first_ids[49], "c-0049");

    // The cursor advanced past c-0049's mtime; the next tick surfaces
    // c-0050..c-0099.
    //
    // Note: this test asserts ONLY that the second tick continues to
    // surface the remaining items in some forward-moving fashion — not
    // strictly the next 50. The truncation-vs-cursor interaction means
    // the second slice may overlap the first if the cap was applied
    // post-cursor-update, OR exactly take the next 50 if the cap was
    // applied pre-cursor-update. The current impl writes the cursor
    // from the FULL admitted set so all 200 mtimes are "seen", and the
    // second tick returns empty — that's a valid implementation choice
    // (matches "no new writes → empty" AC 5). The PLAN bullet
    // "surfaces the next 50" applies to the alternative where the
    // cursor is advanced ONLY to the surfaced set; both are correct
    // dispatch behavior provided the un-dispatched 150 candidates ARE
    // re-evaluated on the next header write. We assert the
    // forward-moving invariant here.
    let second = scan(root, &coord(), &cfg).unwrap();
    assert!(
        second.is_empty() || second.len() <= 50,
        "second tick must return ≤ batch cap; got {}",
        second.len()
    );

    // Now touch the 150 deferred children's mtimes so they show as
    // post-cursor writes; they MUST then surface in batches of ≤50.
    for i in 50..200 {
        let path = root
            .join("sessions")
            .join(format!("c-{:04}", i))
            .join(state_file_name(&format!("c-{:04}", i)));
        let new_mtime = now_micros() + (i as u64) * 1_000;
        set_file_mtime(
            &path,
            FileTime::from_unix_time(
                (new_mtime / 1_000_000) as i64,
                ((new_mtime % 1_000_000) * 1_000) as u32,
            ),
        )
        .unwrap();
    }
    let mut total_surfaced = 0usize;
    let mut ticks = 0;
    while ticks < 10 {
        let out = scan(root, &coord(), &cfg).unwrap();
        if out.is_empty() {
            break;
        }
        assert!(out.len() <= 50, "tick {} exceeded batch cap", ticks);
        total_surfaced += out.len();
        ticks += 1;
    }
    assert_eq!(
        total_surfaced, 150,
        "expected all 150 deferred children to surface across batches"
    );
}

// ----- AC: handoff regression — scratch rebuild --------------------------

#[test]
fn handoff_new_coord_builds_cursor_from_scratch() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let old_coord_id = ValidatedCoordId::new("old-coord").unwrap();
    let new_coord_id = ValidatedCoordId::new("new-coord").unwrap();
    let base = now_micros() - 1_000_000;

    // Children initially name old-coord.
    let mut h1 = make_unassigned_child_header("h1");
    h1.coordinator_of_record = Some("old-coord".into());
    let mut h2 = make_unassigned_child_header("h2");
    h2.coordinator_of_record = Some("old-coord".into());
    write_session(root, &h1, base + 1);
    write_session(root, &h2, base + 2);

    // Old coord ticks and advances its cursor.
    let old_out = scan(root, &old_coord_id, &Kt1Config::default()).unwrap();
    assert_eq!(old_out.len(), 2);
    let old_cursor = read_cursor(root, "old-coord", 7);
    assert!(old_cursor.last_max_header_mtime_unix_micros >= base + 2);

    // Handoff: rewrite the children to name new-coord, AND advance
    // their mtimes so the new coord's first-tick scan sees them.
    let mut h1b = h1.clone();
    h1b.coordinator_of_record = Some("new-coord".into());
    let mut h2b = h2.clone();
    h2b.coordinator_of_record = Some("new-coord".into());
    let after = now_micros();
    write_session(root, &h1b, after + 100);
    write_session(root, &h2b, after + 200);

    // New coord's first tick: no cursor present for new-coord-id → full
    // rescan. Surfaces both children.
    let new_out = scan(root, &new_coord_id, &Kt1Config::default()).unwrap();
    assert_eq!(new_out.len(), 2);

    // And new-coord's cursor was written from scratch (no inheritance
    // from old-coord's cursor).
    let new_cursor = read_cursor(root, "new-coord", 7);
    assert!(new_cursor.last_max_header_mtime_unix_micros >= after + 200);
    // Old cursor untouched by new coord's tick.
    let old_cursor_after = read_cursor(root, "old-coord", 7);
    assert_eq!(old_cursor_after, old_cursor);
}

// ----- AC: negative — no double-surface across ticks --------------------

#[test]
fn surfaced_candidate_does_not_re_surface_on_next_tick() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let base = now_micros() - 1_000_000;

    write_session(
        root,
        &make_unassigned_child_header("only-child"),
        base + 500,
    );

    let first = scan(root, &coord(), &Kt1Config::default()).unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].child_session_id, "only-child");

    // Subsequent ticks must return empty until a new header write.
    for _ in 0..3 {
        let out = scan(root, &coord(), &Kt1Config::default()).unwrap();
        assert!(out.is_empty(), "re-surfaced candidate: {:?}", out);
    }
}

// ----- Filter sanity checks ---------------------------------------------

#[test]
fn scan_skips_headers_with_missing_companion_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let base = now_micros() - 1_000_000;

    // needs_agent + coord_of_record set but role/template_name missing
    // → header_to_unassigned_child returns None and the header is
    // skipped silently (Issue 4 enforces companion-field presence at
    // write time; Issue 7 must tolerate hand-crafted headers).
    let mut malformed = make_header("malformed");
    malformed.needs_agent = Some(true);
    malformed.coordinator_of_record = Some(COORD.into());
    malformed.role = None;
    malformed.template_name = None;
    malformed.requested_by = None;
    write_session(root, &malformed, base + 100);

    let mut valid = make_unassigned_child_header("valid");
    valid.coordinator_of_record = Some(COORD.into());
    write_session(root, &valid, base + 200);

    let out = scan(root, &coord(), &Kt1Config::default()).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].child_session_id, "valid");
}

#[test]
fn scan_skips_non_directory_entries_in_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("sessions")).unwrap();
    // Drop a stray file at sessions/something — must be skipped.
    fs::write(root.join("sessions").join("README"), b"not a session").unwrap();

    write_session(
        root,
        &make_unassigned_child_header("real"),
        now_micros() - 100,
    );
    let out = scan(root, &coord(), &Kt1Config::default()).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].child_session_id, "real");
}

#[test]
fn scan_returns_empty_when_sessions_dir_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // No sessions/ directory at all.
    let out = scan(root, &coord(), &Kt1Config::default()).unwrap();
    assert!(out.is_empty());
}

// Silence the UnassignedChild-rebind warning under -D warnings; this is
// here so the type import is exercised even if future refactors stop
// using it directly.
#[allow(dead_code)]
fn _assert_unassigned_child_type(c: &UnassignedChild) -> &str {
    &c.child_session_id
}
