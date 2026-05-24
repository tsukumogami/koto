//! Integration tests for `koto::engine::terminal_index` plus the
//! Issue 7 discovery scan's terminal-index filter integration.
//!
//! Inline unit tests in `src/engine/terminal_index.rs` cover the
//! writer's atomic-append discipline (race-condition AC for N=32
//! concurrent writers, PIPE_BUF refusal, fsync), reader skip-and-
//! continue parsing (truncated trailing line, malformed JSON, missing
//! fields), and reader dedup by `(session_id, max header_mtime_ns)`.
//! This file exercises the cross-module ACs:
//!
//! - Discovery scan skips a session that appears in the terminal index
//!   with a `header_mtime_ns` ≥ the disk header mtime.
//! - Header-is-truth fallthrough: when the disk header mtime exceeds
//!   the index entry's `header_mtime_ns`, the scan re-surfaces the
//!   session for re-evaluation.
//! - Multi-writer same-session_id case: the reader's dedup keeps the
//!   higher-mtime entry; the lower-mtime entry is NOT consulted by
//!   the scan filter.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use filetime::{set_file_mtime, FileTime};

use koto::config::Kt1Config;
use koto::engine::discovery::scan;
use koto::engine::persistence::append_header;
use koto::engine::terminal_index::{
    append_terminal_index, append_terminal_index_entry, header_mtime_unix_nanos,
    read_terminal_index, terminal_index_path, TerminalIndexEntry,
};
use koto::engine::types::{StateFileHeader, ValidatedCoordId};
use koto::session::state_file_name;

const COORD: &str = "team-lead";

fn make_unassigned_child_header(workflow: &str) -> StateFileHeader {
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
        needs_agent: Some(true),
        role: Some("scrutineer".into()),
        inputs: None,
        coordinator_of_record: Some(COORD.into()),
        requested_by: Some("parent-coord".into()),
        assignment_claim: None,
        dispatch_epoch: 0,
        priority: None,
        deadline: None,
        retry_count: None,
        agent_config: None,
        respawn_generation: None,
    }
}

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

fn coord() -> ValidatedCoordId {
    ValidatedCoordId::new(COORD).unwrap()
}

// ----- Writer happy path ---------------------------------------------------

#[test]
fn writer_round_trips_through_reader() {
    let tmp = tempfile::tempdir().unwrap();
    let header_path = write_session(
        tmp.path(),
        &make_unassigned_child_header("scrutineer-a"),
        now_micros() - 1_000_000,
    );
    append_terminal_index(tmp.path(), "scrutineer-a", "completed", &header_path).unwrap();

    let map = read_terminal_index(tmp.path());
    let entry = map.get("scrutineer-a").expect("must round-trip");
    assert_eq!(entry.session_id, "scrutineer-a");
    assert_eq!(entry.terminal_state, "completed");
    // header_mtime_ns reflects the actual on-disk file's mtime.
    let disk_ns = header_mtime_unix_nanos(&header_path).unwrap();
    assert_eq!(entry.header_mtime_ns, disk_ns);
    // terminal_at is ISO 8601 with millisecond precision.
    assert!(
        entry.terminal_at.ends_with('Z'),
        "terminal_at must be UTC: {}",
        entry.terminal_at
    );
    assert!(
        entry
            .terminal_at
            .chars()
            .nth(19)
            .map(|c| c == '.')
            .unwrap_or(false),
        "terminal_at must carry milliseconds: {}",
        entry.terminal_at
    );
}

// ----- Discovery integration: skip terminal sessions when index ≥ header --

#[test]
fn discovery_skips_session_listed_in_terminal_index() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let base = now_micros() - 10_000_000;
    // Two unassigned candidates; one is terminal, one is not.
    let live_path = write_session(
        root,
        &make_unassigned_child_header("child-live"),
        base + 1_000_000,
    );
    let terminal_path = write_session(
        root,
        &make_unassigned_child_header("child-terminal"),
        base + 2_000_000,
    );
    // Mark child-terminal as terminal in the index with mtime ≥ disk.
    let terminal_disk_ns = header_mtime_unix_nanos(&terminal_path).unwrap();
    append_terminal_index_entry(
        root,
        &TerminalIndexEntry {
            session_id: "child-terminal".into(),
            terminal_at: "2026-05-24T14:35:01.000Z".into(),
            header_mtime_ns: terminal_disk_ns,
            terminal_state: "completed".into(),
        },
    )
    .unwrap();

    let candidates = scan(root, &coord(), &Kt1Config::default()).unwrap();
    let ids: Vec<&str> = candidates
        .iter()
        .map(|c| c.child_session_id.as_str())
        .collect();
    assert!(
        ids.contains(&"child-live"),
        "live session must surface: {:?}",
        ids
    );
    assert!(
        !ids.contains(&"child-terminal"),
        "terminal-indexed session must be filtered out: {:?}",
        ids
    );
    // Sanity: live header was the one not filtered.
    let _ = live_path;
}

// ----- Header-is-truth fallthrough ----------------------------------------

#[test]
fn discovery_resurfaces_session_when_disk_header_mtime_exceeds_index() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let base = now_micros() - 10_000_000;
    let header_path = write_session(
        root,
        &make_unassigned_child_header("recovered"),
        base + 1_000_000,
    );
    // Write a terminal-index entry with header_mtime_ns BELOW the
    // current on-disk mtime — simulates an older index entry that a
    // recovery walk (Issue 11 cases 3b/3c) has since superseded.
    let disk_ns = header_mtime_unix_nanos(&header_path).unwrap();
    let stale_ns = disk_ns.saturating_sub(1_000_000_000); // 1 s older
    append_terminal_index_entry(
        root,
        &TerminalIndexEntry {
            session_id: "recovered".into(),
            terminal_at: "2026-05-24T14:35:01.000Z".into(),
            header_mtime_ns: stale_ns,
            terminal_state: "completed".into(),
        },
    )
    .unwrap();

    let candidates = scan(root, &coord(), &Kt1Config::default()).unwrap();
    let ids: Vec<&str> = candidates
        .iter()
        .map(|c| c.child_session_id.as_str())
        .collect();
    assert!(
        ids.contains(&"recovered"),
        "header-is-truth fallthrough must re-surface the session: {:?}",
        ids
    );
}

// ----- Multi-writer same-session_id (Issue 11 cases 3a/3b/3c + dispatched
// agent terminal append) ---------------------------------------------------

#[test]
fn multi_writer_same_session_dedups_to_higher_mtime() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let base = now_micros() - 10_000_000;
    let header_path = write_session(
        root,
        &make_unassigned_child_header("multi-writer"),
        base + 1_000_000,
    );
    let disk_ns = header_mtime_unix_nanos(&header_path).unwrap();
    // Case 3a: sidecar recovery writes an entry with the older mtime
    // (the cleanup-only unlink + index append happens before the
    // dispatched agent's terminal write lands).
    append_terminal_index_entry(
        root,
        &TerminalIndexEntry {
            session_id: "multi-writer".into(),
            terminal_at: "2026-05-24T14:35:01.000Z".into(),
            header_mtime_ns: disk_ns.saturating_sub(500_000_000),
            terminal_state: "abandoned".into(),
        },
    )
    .unwrap();
    // Dispatched agent's terminal write: higher mtime, "completed".
    append_terminal_index_entry(
        root,
        &TerminalIndexEntry {
            session_id: "multi-writer".into(),
            terminal_at: "2026-05-24T14:35:05.000Z".into(),
            header_mtime_ns: disk_ns,
            terminal_state: "completed".into(),
        },
    )
    .unwrap();

    // Reader dedups to the higher-mtime entry, AND the scan filter
    // consults only that entry (so the session is skipped when the
    // dispatched agent's write covers the disk header's mtime).
    let map = read_terminal_index(root);
    let winner = map.get("multi-writer").expect("must dedup to one entry");
    assert_eq!(winner.header_mtime_ns, disk_ns);
    assert_eq!(winner.terminal_state, "completed");

    let candidates = scan(root, &coord(), &Kt1Config::default()).unwrap();
    let ids: Vec<&str> = candidates
        .iter()
        .map(|c| c.child_session_id.as_str())
        .collect();
    assert!(
        !ids.contains(&"multi-writer"),
        "dispatched agent's higher-mtime entry must cause the session to skip: {:?}",
        ids
    );
}

// ----- Discovery scan unaffected when index is absent ---------------------

#[test]
fn discovery_unaffected_when_index_file_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_session(
        root,
        &make_unassigned_child_header("only-child"),
        now_micros() - 1_000_000,
    );
    // No terminal index file written.
    assert!(!terminal_index_path(root).exists());
    let candidates = scan(root, &coord(), &Kt1Config::default()).unwrap();
    let ids: Vec<&str> = candidates
        .iter()
        .map(|c| c.child_session_id.as_str())
        .collect();
    assert_eq!(ids, vec!["only-child"]);
}

// ----- Index entry shape (positive): extra unknown keys preserved ---------

#[test]
fn reader_tolerates_extra_unknown_keys_in_jsonl_line() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let path = terminal_index_path(root);
    fs::create_dir_all(root).unwrap();
    let line = b"{\"session_id\":\"sX\",\"terminal_at\":\"2026-05-24T00:00:00.000Z\",\"header_mtime_ns\":99,\"terminal_state\":\"completed\",\"future_field\":\"ignored\"}\n";
    fs::write(&path, line).unwrap();
    let map = read_terminal_index(root);
    let entry = map.get("sX").expect("forward-compat line must parse");
    assert_eq!(entry.header_mtime_ns, 99);
}
