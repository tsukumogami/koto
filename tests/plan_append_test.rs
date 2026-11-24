//! `plan_append` decides what an append will do from a log's bytes alone:
//! the next seq, whether the last line lacks its newline, and whether that
//! line holds merged events to split first. These tests hand it log text
//! held in memory; nothing here opens a file, except the one test that
//! checks a truncated last line fails a real append with the same error.

#[path = "support/host_session.rs"]
mod host_session;

use std::path::Path;

use koto::engine::persistence::{append_event, plan_append};
use koto::engine::types::{Event, EventPayload, StateFileHeader};

const HEADER: &str = r#"{"schema_version":1,"workflow":"wf","template_hash":"abc","created_at":"2025-01-01T00:00:00Z"}"#;

/// A label for error messages; `plan_append` never opens it.
const LOG: &str = "/nonexistent/wf/koto-wf.state.jsonl";

fn payload(key: &str) -> EventPayload {
    EventPayload::ContextAdded {
        key: key.to_string(),
        hash: "0".repeat(64),
        size: 1,
        writer: None,
    }
}

/// One event line, without its newline.
fn event_line(seq: u64) -> String {
    let payload = payload(&format!("k{seq}"));
    serde_json::to_string(&Event {
        seq,
        timestamp: "2025-01-01T00:00:00Z".to_string(),
        event_type: payload.type_name().to_string(),
        payload,
        idempotency_hash: None,
    })
    .unwrap()
}

/// A log of the header and events `1..=n`, each on its own line.
fn clean_log(n: u64) -> String {
    let mut log = format!("{HEADER}\n");
    for seq in 1..=n {
        log.push_str(&event_line(seq));
        log.push('\n');
    }
    log
}

#[test]
fn a_clean_log_appends_at_the_next_seq() {
    let log = clean_log(3);
    let plan = plan_append::<StateFileHeader>(&log, Path::new(LOG)).unwrap();
    assert_eq!(plan.next_seq, 4);
    assert!(!plan.unterminated);
    assert!(plan.merged.is_none());
}

#[test]
fn a_header_only_log_appends_at_seq_one() {
    let log = clean_log(0);
    let plan = plan_append::<StateFileHeader>(&log, Path::new(LOG)).unwrap();
    assert_eq!(plan.next_seq, 1);
    assert!(plan.merged.is_none());
}

#[test]
fn a_last_line_without_its_newline_is_reported_unterminated() {
    let log = clean_log(2);
    let log = log.strip_suffix('\n').unwrap();
    let plan = plan_append::<StateFileHeader>(log, Path::new(LOG)).unwrap();
    assert_eq!(plan.next_seq, 3);
    assert!(plan.unterminated);
    assert!(plan.merged.is_none());
}

#[test]
fn a_merged_last_line_is_classified_for_the_split() {
    let mut log = clean_log(2);
    let offset = log.len();
    let (third, fourth) = (event_line(3), event_line(4));
    log.push_str(&third);
    log.push_str(&fourth);
    log.push('\n');

    let plan = plan_append::<StateFileHeader>(&log, Path::new(LOG)).unwrap();
    assert_eq!(plan.next_seq, 5);
    let merged = plan.merged.expect("the last line holds two merged events");
    assert_eq!(merged.offset, offset);
    assert_eq!(
        merged.line_number, 4,
        "header is line 1, events 1 and 2 are lines 2 and 3"
    );
    let seqs: Vec<u64> = merged.pieces.iter().map(|(_, e)| e.seq).collect();
    assert_eq!(seqs, vec![3, 4]);
    let texts: Vec<&str> = merged.pieces.iter().map(|(text, _)| *text).collect();
    assert_eq!(texts, vec![third.as_str(), fourth.as_str()]);
}

#[test]
fn merged_events_that_do_not_follow_are_refused_naming_the_line() {
    let mut log = clean_log(2);
    log.push_str(&event_line(5));
    log.push_str(&event_line(6));
    log.push('\n');

    let err = plan_append::<StateFileHeader>(&log, Path::new(LOG)).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("line 4 of"), "{msg}");
    assert!(msg.contains(LOG), "{msg}");
    assert!(msg.contains("(seq 5, 6)"), "{msg}");
}

/// The bytes of a log whose last line is the torn tail of a crash: an event
/// line cut partway through.
fn truncated_log() -> String {
    let mut log = clean_log(2);
    let third = event_line(3);
    log.push_str(&third[..third.len() / 2]);
    log
}

#[test]
fn a_truncated_last_line_is_refused() {
    let log = truncated_log();
    let err = plan_append::<StateFileHeader>(&log, Path::new(LOG)).unwrap_err();
    assert!(
        err.to_string()
            .starts_with("failed to parse last event line: "),
        "{err}"
    );
}

#[test]
fn a_truncated_last_line_fails_a_write_with_the_same_error() {
    let log = truncated_log();
    let planned = plan_append::<StateFileHeader>(&log, Path::new(LOG))
        .unwrap_err()
        .to_string();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("koto-wf.state.jsonl");
    std::fs::write(&path, &log).unwrap();
    let written = append_event(&path, &payload("next"), "2025-01-01T00:00:01Z")
        .unwrap_err()
        .to_string();

    assert_eq!(planned, written);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        log,
        "nothing written"
    );
}

#[test]
fn a_log_with_no_header_is_refused() {
    let err = plan_append::<StateFileHeader>("", Path::new(LOG)).unwrap_err();
    assert!(err.to_string().contains("state file is empty"), "{err}");
}
