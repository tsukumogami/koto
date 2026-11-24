//! State-file persistence helpers.
//!
//! Durability discipline: `sync_data()` (fdatasync) is called after
//! every header and event append. The first header write of a brand-new
//! state file ALSO fsyncs the parent directory via
//! `append::fsync_parent_dir` so the file's directory entry is durable across
//! a crash. Subsequent appends do NOT re-fsync the parent because the
//! directory entry is already committed; only the file's data + size
//! need durability, which `sync_data()` provides. See Fix 3 in the
//! polish pass for the originating review note (ext4 `data=ordered`
//! plus a crash between `sync_data()` and the implicit directory
//! flush could leave a brand-new state file invisible on remount).

use std::path::Path;

use crate::engine::errors::EngineError;
use crate::engine::types::{Event, EventPayload, StateFileHeader};

mod append;

#[cfg(debug_assertions)]
pub(crate) use append::held_state_locks;
pub use append::{
    append_event, append_event_idempotent, append_event_idempotent_in, append_event_in,
    append_header, append_header_line, append_supersede_fence, fsync_wake_preconditions,
    hold_log_lock, idempotency_hash, replace_log_bytes, serialize_header_line, session_superseded,
    state_log_busy, with_held_log_lock, with_log_replace_lock, AppendOutcome, FenceOutcome,
    HeldLogLock, SessionSupersededError, StateLogBusy, APPEND_LOCK_FILE, FENCE_LOCK_WAIT,
    LOG_REPLACE_LOCK_WAIT,
};
pub(crate) use append::{
    append_event_tracked, append_event_tracked_held, hold_log_lock_blocking, CommitInfo,
};
#[cfg(all(test, unix))]
pub(crate) use append::{hold_append_lock, hold_append_lock_for};

// The read half lives in koto-session-reader; the write half above uses it
// through `use super::*`.
pub use koto_session_reader::engine::persistence::*;

/// [`epoch_slice`], for tests outside this module that pin the visit
/// boundary against it.
#[cfg(test)]
pub(crate) fn epoch_slice_for_test<'a>(events: &'a [Event], current_state: &str) -> &'a [Event] {
    epoch_slice(events, current_state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::{Event, EventPayload, StateFileHeader, SupersedeActor};
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn make_header() -> StateFileHeader {
        StateFileHeader {
            command_environment: None,
            schema_version: 1,
            workflow: "test-wf".to_string(),
            template_hash: "deadbeef".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            parent_workflow: None,
            template_source_dir: None,
            template_source_file: None,
            origin: None,
            execution_dir: None,
            session_id: String::new(),
            intent: None,
            template_name: None,
            needs_agent: None,
            role: None,
            inputs: None,
            coordinator_of_record: None,
            requested_by: None,
            assignment_claim: None,
            dispatch_epoch: 0,
            respawn_generation: None,
            priority: None,
            deadline: None,
            retry_count: None,
            agent_config: None,
            kind: None,
            root_session_id: None,
            parent_session_id: None,
        }
    }

    fn make_event(seq: u64, payload: EventPayload) -> Event {
        Event {
            seq,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: payload.type_name().to_string(),
            payload,
            idempotency_hash: None,
        }
    }

    fn write_state_file(
        dir: &Path,
        name: &str,
        header: &StateFileHeader,
        events: &[Event],
    ) -> std::path::PathBuf {
        let path = dir.join(format!("koto-{}.state.jsonl", name));
        let mut content = serde_json::to_string(header).unwrap();
        content.push('\n');
        for event in events {
            content.push_str(&serde_json::to_string(event).unwrap());
            content.push('\n');
        }
        std::fs::write(&path, &content).unwrap();
        path
    }

    // -----------------------------------------------------------------------
    // Schema version guard
    // -----------------------------------------------------------------------

    #[test]
    fn schema_version_1_is_accepted() {
        let dir = TempDir::new().unwrap();
        let header = make_header(); // schema_version: 1
        let events = vec![make_event(
            1,
            EventPayload::WorkflowInitialized {
                template_path: "/cache/abc.json".to_string(),
                variables: HashMap::new(),
                spawn_entry: None,
                machine: None,
            },
        )];
        let path = write_state_file(dir.path(), "sv1", &header, &events);
        assert!(read_events(&path).is_ok());
    }

    #[test]
    fn schema_version_2_returns_incompatible_error() {
        let dir = TempDir::new().unwrap();
        let mut header = make_header();
        header.schema_version = 2;
        let events = vec![make_event(
            1,
            EventPayload::WorkflowInitialized {
                template_path: "/cache/abc.json".to_string(),
                variables: HashMap::new(),
                spawn_entry: None,
                machine: None,
            },
        )];
        let path = write_state_file(dir.path(), "sv2", &header, &events);
        let err = read_events(&path).unwrap_err();
        let engine_err = err.downcast::<EngineError>().expect("must be EngineError");
        assert!(
            matches!(
                engine_err,
                EngineError::IncompatibleSchemaVersion {
                    found: 2,
                    max_supported: 1
                }
            ),
            "unexpected error: {:?}",
            engine_err
        );
    }

    #[test]
    fn header_missing_schema_version_defaults_to_1() {
        // Older sessions were written before `schema_version` existed in
        // the header. Such a header must parse (as version 1) rather than
        // fail with "missing field schema_version", so discovery stops
        // warning about it and the session stays readable.
        let dir = TempDir::new().unwrap();
        let header = make_header();
        let mut header_json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&header).unwrap()).unwrap();
        header_json
            .as_object_mut()
            .unwrap()
            .remove("schema_version");
        assert!(
            header_json.get("schema_version").is_none(),
            "test setup: schema_version should be absent"
        );

        let path = dir.path().join("koto-legacy.state.jsonl");
        let mut content = serde_json::to_string(&header_json).unwrap();
        content.push('\n');
        content.push_str(
            &serde_json::to_string(&make_event(
                1,
                EventPayload::WorkflowInitialized {
                    template_path: "/cache/abc.json".to_string(),
                    variables: HashMap::new(),
                    spawn_entry: None,
                    machine: None,
                },
            ))
            .unwrap(),
        );
        content.push('\n');
        std::fs::write(&path, &content).unwrap();

        let parsed = read_header(&path).expect("header missing schema_version must parse");
        assert_eq!(parsed.schema_version, 1);

        // The full read path recovers too, not just the header.
        let (parsed_header, events) =
            read_events(&path).expect("read_events must recover legacy header");
        assert_eq!(parsed_header.schema_version, 1);
        assert_eq!(events.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Header parsing
    // -----------------------------------------------------------------------

    #[test]
    fn read_events_with_valid_header_and_events() {
        let dir = TempDir::new().unwrap();
        let header = make_header();
        let events = vec![
            make_event(
                1,
                EventPayload::WorkflowInitialized {
                    template_path: "/cache/abc.json".to_string(),
                    variables: HashMap::new(),
                    spawn_entry: None,
                    machine: None,
                },
            ),
            make_event(
                2,
                EventPayload::Transitioned {
                    from: None,
                    to: "gather".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                    context_assignments: None,
                    vars_matched: None,
                },
            ),
        ];
        let path = write_state_file(dir.path(), "test", &header, &events);

        let (parsed_header, parsed_events) = read_events(&path).unwrap();
        assert_eq!(parsed_header, header);
        assert_eq!(parsed_events.len(), 2);
        assert_eq!(parsed_events[0].seq, 1);
        assert_eq!(parsed_events[1].seq, 2);
    }

    // -----------------------------------------------------------------------
    // Sequence gap detection
    // -----------------------------------------------------------------------

    #[test]
    fn sequence_gap_in_middle_produces_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-gap.state.jsonl");

        let header = make_header();
        let e1 = make_event(
            1,
            EventPayload::WorkflowInitialized {
                template_path: "/cache/abc.json".to_string(),
                variables: HashMap::new(),
                spawn_entry: None,
                machine: None,
            },
        );
        // Seq jumps from 1 to 3 (gap at seq 2)
        let e3 = make_event(
            3,
            EventPayload::Transitioned {
                from: None,
                to: "gather".to_string(),
                condition_type: "auto".to_string(),
                skip_if_matched: None,
                context_assignments: None,
                vars_matched: None,
            },
        );
        let e4 = make_event(
            4,
            EventPayload::Transitioned {
                from: Some("gather".to_string()),
                to: "plan".to_string(),
                condition_type: "gate".to_string(),
                skip_if_matched: None,
                context_assignments: None,
                vars_matched: None,
            },
        );

        let mut content = serde_json::to_string(&header).unwrap() + "\n";
        content += &serde_json::to_string(&e1).unwrap();
        content.push('\n');
        content += &serde_json::to_string(&e3).unwrap();
        content.push('\n');
        content += &serde_json::to_string(&e4).unwrap();
        content.push('\n');
        std::fs::write(&path, &content).unwrap();

        let result = read_events(&path);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("sequence gap"),
            "error should mention sequence gap, got: {}",
            err
        );
    }

    // -----------------------------------------------------------------------
    // Truncated final line recovery
    // -----------------------------------------------------------------------

    #[test]
    fn truncated_final_line_recovered() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-trunc.state.jsonl");

        let header = make_header();
        let e1 = make_event(
            1,
            EventPayload::WorkflowInitialized {
                template_path: "/cache/abc.json".to_string(),
                variables: HashMap::new(),
                spawn_entry: None,
                machine: None,
            },
        );

        let mut content = serde_json::to_string(&header).unwrap() + "\n";
        content += &serde_json::to_string(&e1).unwrap();
        content.push('\n');
        content += "{\"seq\":2,\"truncated"; // malformed final line
        content.push('\n');
        std::fs::write(&path, &content).unwrap();

        let (parsed_header, events) = read_events(&path).unwrap();
        assert_eq!(parsed_header, header);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].seq, 1);
    }

    // -----------------------------------------------------------------------
    // append_event with seq assignment
    // -----------------------------------------------------------------------

    #[test]
    fn append_event_assigns_sequential_seq() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-seq.state.jsonl");

        // Write header first
        let header = make_header();
        append_header(&path, &header).unwrap();

        let seq1 = append_event(
            &path,
            &EventPayload::WorkflowInitialized {
                template_path: "/cache/abc.json".to_string(),
                variables: HashMap::new(),
                spawn_entry: None,
                machine: None,
            },
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        assert_eq!(seq1, 1);

        let seq2 = append_event(
            &path,
            &EventPayload::Transitioned {
                from: None,
                to: "gather".to_string(),
                condition_type: "auto".to_string(),
                skip_if_matched: None,
                context_assignments: None,
                vars_matched: None,
            },
            "2026-01-01T00:00:01Z",
        )
        .unwrap();
        assert_eq!(seq2, 2);

        let seq3 = append_event(
            &path,
            &EventPayload::Rewound {
                from: "gather".to_string(),
                to: "start".to_string(),
                rationale: None,
            },
            "2026-01-01T00:00:02Z",
        )
        .unwrap();
        assert_eq!(seq3, 3);

        // Verify by reading back
        let (_, events) = read_events(&path).unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[1].seq, 2);
        assert_eq!(events[2].seq, 3);
    }

    // -----------------------------------------------------------------------
    // append_event_tracked and the visit rule
    // -----------------------------------------------------------------------

    fn moved(from: Option<&str>, to: &str) -> EventPayload {
        EventPayload::Transitioned {
            from: from.map(str::to_string),
            to: to.to_string(),
            condition_type: "auto".to_string(),
            skip_if_matched: None,
            context_assignments: None,
            vars_matched: None,
        }
    }

    fn noted(state: &str) -> EventPayload {
        EventPayload::EvidenceSubmitted {
            state: state.to_string(),
            fields: HashMap::new(),
            submitter_cwd: None,
            source: None,
            exported_fields: None,
        }
    }

    /// Each append returns its own seq, the log as it was before it, and
    /// the visit that log was in. A self-transition stays in the visit; an
    /// arrival from elsewhere, a directed transition from a different
    /// state and a rewind each open one.
    #[test]
    fn tracked_appends_report_the_seq_the_prior_log_and_the_visit() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-tracked.state.jsonl");
        append_header(&path, &make_header()).unwrap();
        let ts = "2026-01-01T00:00:00Z";
        let steps: Vec<(EventPayload, Option<u64>, Option<u64>)> = vec![
            // (payload, prior visit, the event's own visit)
            (
                EventPayload::WorkflowInitialized {
                    template_path: "t".to_string(),
                    variables: HashMap::new(),
                    spawn_entry: None,
                    machine: None,
                },
                None,
                None,
            ),
            (moved(None, "a"), None, Some(2)),
            (noted("a"), Some(2), Some(2)),
            (moved(Some("a"), "a"), Some(2), Some(2)),
            (noted("a"), Some(2), Some(2)),
            (moved(Some("a"), "b"), Some(2), Some(6)),
            (noted("b"), Some(6), Some(6)),
            (
                EventPayload::DirectedTransition {
                    from: "b".to_string(),
                    to: "b".to_string(),
                    rationale: None,
                    decision: None,
                },
                Some(6),
                Some(6),
            ),
            (
                EventPayload::Rewound {
                    from: "b".to_string(),
                    to: "a".to_string(),
                    rationale: None,
                },
                Some(6),
                Some(9),
            ),
            (noted("a"), Some(9), Some(9)),
            (
                EventPayload::DirectedTransition {
                    from: "a".to_string(),
                    to: "c".to_string(),
                    rationale: None,
                    decision: None,
                },
                Some(9),
                Some(11),
            ),
            (
                EventPayload::Rewound {
                    from: "c".to_string(),
                    to: "c".to_string(),
                    rationale: None,
                },
                Some(11),
                Some(12),
            ),
        ];
        for (i, (payload, prior, own)) in steps.iter().enumerate() {
            let before = std::fs::read_to_string(&path).unwrap();
            let commit = append_event_tracked(&path, payload, ts).unwrap();
            assert_eq!(commit.seq, i as u64 + 1, "step {i}");
            assert_eq!(commit.prior_log, before, "step {i}");
            assert_eq!(commit.prior_visit_seq(), *prior, "step {i}");
            assert_eq!(commit.visit_seq_for(payload), *own, "step {i}");
        }
        // The tracked append writes exactly what a plain append writes.
        let (_, events) = read_events(&path).unwrap();
        assert_eq!(events.len(), steps.len());
        assert_eq!(
            visit_start(&std::fs::read_to_string(&path).unwrap()),
            Some(12)
        );
    }

    /// `visit_start` on a whole log agrees with `arrival_index` on the same
    /// events, and isn't fooled by a type word inside another event.
    #[test]
    fn visit_start_matches_arrival_index_and_skips_lookalike_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-look.state.jsonl");
        append_header(&path, &make_header()).unwrap();
        let ts = "2026-01-01T00:00:00Z";
        append_event(&path, &moved(None, "a"), ts).unwrap();
        append_event(&path, &moved(Some("a"), "b"), ts).unwrap();
        let mut fields = HashMap::new();
        fields.insert("type".to_string(), serde_json::json!("rewound"));
        fields.insert("to".to_string(), serde_json::json!("a"));
        append_event(
            &path,
            &EventPayload::EvidenceSubmitted {
                state: "b".to_string(),
                fields,
                submitter_cwd: None,
                source: None,
                exported_fields: None,
            },
            ts,
        )
        .unwrap();
        append_event(&path, &moved(Some("b"), "b"), ts).unwrap();
        let log = std::fs::read_to_string(&path).unwrap();
        let (_, events) = read_events(&path).unwrap();
        let idx = arrival_index(&events, "b").unwrap();
        assert_eq!(visit_start(&log), Some(events[idx].seq));
        assert_eq!(visit_start(&log), Some(2));
        // The state comes from the same scan, and agrees with the log's.
        assert_eq!(current_visit(&log), Some(("b".to_string(), 2)));
        assert_eq!(derive_state_from_log(&events).as_deref(), Some("b"));
        // A log with no state entry has no visit.
        assert_eq!(visit_start(log.lines().next().unwrap()), None);

        // The prefilter reads the type word where koto writes it, and falls
        // back to the whole line for any other layout.
        let lines: Vec<&str> = log.lines().collect();
        assert!(!may_be_entry(lines[3]), "evidence naming a rewind");
        assert!(may_be_entry(lines[2]));
        let reordered = format!(
            r#"{{"payload":{{"from":"{}","to":"a"}},"type":"rewound","seq":9,"timestamp":"t"}}"#,
            "b".repeat(200)
        );
        assert!(may_be_entry(&reordered));
        let log = format!("{log}{reordered}\n");
        assert_eq!(visit_start(&log), Some(9));
    }

    /// `last_seq` is the last event's seq, skips a partial last line, and
    /// is `None` for a log holding only its header.
    #[test]
    fn last_seq_reads_the_last_event_and_skips_a_partial_line() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-last.state.jsonl");
        append_header(&path, &make_header()).unwrap();
        let header_only = std::fs::read_to_string(&path).unwrap();
        assert_eq!(last_seq(&header_only), None);
        let ts = "2026-01-01T00:00:00Z";
        append_event(&path, &moved(None, "a"), ts).unwrap();
        append_event(&path, &moved(Some("a"), "b"), ts).unwrap();
        let log = std::fs::read_to_string(&path).unwrap();
        assert_eq!(last_seq(&log), Some(2));
        let torn = format!("{log}{{\"seq\":3,\"timest");
        assert_eq!(last_seq(&torn), Some(2));
    }

    // -----------------------------------------------------------------------
    // append refuses a log that has no header (koto#236, koto#200)
    // -----------------------------------------------------------------------

    fn context_added(key: &str) -> EventPayload {
        EventPayload::ContextAdded {
            key: key.to_string(),
            hash: "abc".to_string(),
            size: 1,
            writer: None,
        }
    }

    #[test]
    fn append_event_refuses_a_missing_log_and_creates_nothing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-missing.state.jsonl");

        let err = append_event(&path, &context_added("a.md"), "2026-01-01T00:00:00Z").unwrap_err();

        assert!(err.to_string().contains("does not exist"), "got: {err}");
        assert!(!path.exists(), "a refused append must not create the log");
    }

    #[test]
    fn append_event_refuses_an_empty_log_and_writes_nothing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-empty.state.jsonl");
        std::fs::write(&path, "").unwrap();

        let err = append_event(&path, &context_added("a.md"), "2026-01-01T00:00:00Z").unwrap_err();

        assert!(
            err.to_string().contains("state file is empty"),
            "got: {err}"
        );
        assert!(
            std::fs::read(&path).unwrap().is_empty(),
            "a refused append must leave an empty log empty"
        );
    }

    /// The #236 shape: a log whose first line is an event. Before the fix the
    /// append skipped that line as if it were the header, wrote a second
    /// `seq: 1`, and the log stayed unreadable.
    #[test]
    fn append_event_refuses_a_log_whose_first_line_is_an_event() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-headerless.state.jsonl");
        let line = serde_json::to_string(&make_event(1, context_added("a.md"))).unwrap();
        let content = format!("{line}\n");
        std::fs::write(&path, &content).unwrap();

        let err = append_event(&path, &context_added("b.md"), "2026-01-01T00:00:01Z").unwrap_err();

        assert!(
            err.to_string().contains("state log has no header"),
            "got: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            content,
            "a refused append must leave the log byte-for-byte unchanged"
        );
    }

    #[test]
    fn append_event_idempotent_refuses_a_missing_log_and_creates_nothing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-missing.state.jsonl");

        let result = append_event_idempotent(
            &path,
            &context_added("a.md"),
            "2026-01-01T00:00:00Z",
            "work",
            Some("hash"),
        );

        assert!(result.is_err(), "an append to a missing log must fail");
        assert!(
            !path.exists(),
            "neither the lock nor the append may create the log"
        );
    }

    /// The case the header-before-scan ordering exists for. A headerless log
    /// that already carries a matching idempotency hash used to short-circuit
    /// into `Idempotent`, reporting success on a log no reader can parse.
    /// Move the header check back below the scan and this test fails; the
    /// missing-log tests would not, because they fail earlier at the open.
    #[test]
    fn append_event_idempotent_refuses_a_headerless_log_that_holds_a_matching_hash() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-headerless.state.jsonl");
        let payload = context_added("a.md");
        let hash = "matching-hash";
        let prior = Event {
            seq: 1,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: payload.type_name().to_string(),
            payload: payload.clone(),
            idempotency_hash: Some(hash.to_string()),
        };
        let content = format!("{}\n", serde_json::to_string(&prior).unwrap());
        std::fs::write(&path, &content).unwrap();

        let err =
            append_event_idempotent(&path, &payload, "2026-01-01T00:00:01Z", "work", Some(hash))
                .unwrap_err();

        assert!(
            err.to_string().contains("state log has no header"),
            "the header check must run before the hash scan; got: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            content,
            "a refused append must leave the log unchanged"
        );
    }

    #[test]
    fn read_header_names_a_log_whose_first_line_is_an_event() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-headerless.state.jsonl");
        let line = serde_json::to_string(&make_event(1, context_added("a.md"))).unwrap();
        std::fs::write(&path, format!("{line}\n")).unwrap();

        let err = read_header(&path).unwrap_err().to_string();

        assert!(
            err.contains("state log has no header")
                && err.contains("`context_added` event (seq 1)"),
            "got: {err}"
        );
    }

    // -----------------------------------------------------------------------
    // File permissions (unix only)
    // -----------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn newly_created_state_file_has_mode_0600() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-perm.state.jsonl");

        let header = make_header();
        append_header(&path, &header).unwrap();

        let metadata = std::fs::metadata(&path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "file permissions should be 0600, got {:o}",
            mode
        );
    }

    // -- the session-directory fallback for a missing absolute template path --

    const HASH_NAME: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.json";

    #[test]
    fn missing_absolute_hash_path_falls_back_to_the_session_dir_copy() {
        let tmp = tempfile::TempDir::new().unwrap();
        let session_dir = tmp.path().join("session");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(session_dir.join(HASH_NAME), b"{}").unwrap();
        let recorded = format!("/elsewhere/.cache/koto/{}", HASH_NAME);

        assert_eq!(
            resolve_template_path_in_session(&recorded, &session_dir),
            session_dir.join(HASH_NAME).to_string_lossy()
        );
    }

    #[test]
    fn fallback_is_not_taken_for_a_name_that_is_not_a_content_hash() {
        let tmp = tempfile::TempDir::new().unwrap();
        let session_dir = tmp.path().join("session");
        std::fs::create_dir_all(&session_dir).unwrap();
        // Each of these exists in the session dir, so only the name rule
        // stands between a crafted template_path and the file.
        let names = [
            "manifest.json",
            "version.json",
            // Uppercase hex is not what the cache writes.
            "0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef.json",
            // 63 digits.
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde.json",
            // Right stem, wrong extension.
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.md",
        ];
        for name in names {
            std::fs::write(session_dir.join(name), b"{}").unwrap();
            let recorded = format!("/elsewhere/{}", name);
            assert_eq!(
                resolve_template_path_in_session(&recorded, &session_dir),
                recorded,
                "{name} must not resolve into the session dir"
            );
        }
    }

    #[test]
    fn an_existing_absolute_path_is_preferred_over_the_session_dir_copy() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cache = tmp.path().join("cache");
        let session_dir = tmp.path().join("session");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(cache.join(HASH_NAME), b"{}").unwrap();
        std::fs::write(session_dir.join(HASH_NAME), b"{}").unwrap();
        let recorded = cache.join(HASH_NAME).to_string_lossy().into_owned();

        assert_eq!(
            resolve_template_path_in_session(&recorded, &session_dir),
            recorded
        );
    }

    #[test]
    fn missing_absolute_path_with_no_session_copy_passes_through() {
        let tmp = tempfile::TempDir::new().unwrap();
        let recorded = format!("/elsewhere/.cache/koto/{}", HASH_NAME);
        assert_eq!(
            resolve_template_path_in_session(&recorded, tmp.path()),
            recorded,
            "nothing to fall back to: the reader reports the recorded path"
        );
    }

    // -----------------------------------------------------------------------
    // append_header parent-dir fsync (Fix 3)
    // -----------------------------------------------------------------------

    /// A first-write `append_header` against a brand-new state file
    /// must succeed end-to-end. The parent-dir fsync is best-effort
    /// (some filesystems reject `O_RDONLY` directory fsync); the only
    /// observable assertion is that the header is durable on the
    /// filesystem after the call returns. We don't assert the
    /// directory was actually fsynced — that requires a crash-replay
    /// harness we don't have — but we DO assert the write path
    /// tolerates the parent-dir fsync attempt without surfacing an
    /// error to the caller.
    #[test]
    fn append_header_first_write_writes_durably() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-first-write.state.jsonl");
        assert!(!path.exists());
        let header = make_header();
        append_header(&path, &header).expect("first-write append_header must succeed");
        assert!(path.exists(), "header file must exist after first write");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("\"workflow\":\"test-wf\""));
        assert!(content.ends_with('\n'), "header line must end in newline");
    }

    /// A subsequent `append_header` call (file already exists) takes
    /// the non-first-write branch and skips the parent-dir fsync.
    /// We can't directly observe the skip from a unit test, but the
    /// call must still succeed and leave the file readable. The
    /// regression we're guarding against is the parent-dir fsync
    /// branch firing on every write — which would be benign but
    /// wastes a syscall on every append in the steady state.
    #[test]
    fn append_header_subsequent_write_is_well_formed() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-second-write.state.jsonl");
        let header = make_header();
        append_header(&path, &header).expect("first write");
        // Second call — the file already exists, so the is_first_write
        // branch should NOT fire. We can only assert the call completes
        // cleanly; the syscall-count optimisation is internal.
        append_header(&path, &header).expect("second write");
        let content = std::fs::read_to_string(&path).unwrap();
        // The second call rewrites at offset 0 (the file was opened
        // with .write(true).truncate(false), so the cursor starts at
        // offset 0 and the second header overwrites the first byte for
        // byte). We assert the workflow marker is present — not the
        // count — because the call's existing contract focuses on
        // durability, not append-vs-overwrite semantics.
        assert!(content.contains("\"workflow\":\"test-wf\""));
    }

    // -----------------------------------------------------------------------
    // Merged and unterminated last lines (koto#419)
    // -----------------------------------------------------------------------

    fn intent(seq: u64, text: &str) -> Event {
        make_event(
            seq,
            EventPayload::IntentUpdated {
                intent: text.to_string(),
            },
        )
    }

    fn line(event: &Event) -> String {
        serde_json::to_string(event).unwrap()
    }

    /// A log holding the header, `events` one per line, and then `tail`
    /// written verbatim.
    fn log_with_tail(dir: &Path, events: &[Event], tail: &str) -> std::path::PathBuf {
        let path = write_state_file(dir, "tail", &make_header(), events);
        let mut content = std::fs::read_to_string(&path).unwrap();
        content.push_str(tail);
        std::fs::write(&path, content).unwrap();
        path
    }

    fn backups(dir: &Path) -> Vec<std::path::PathBuf> {
        let mut found: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "bak"))
            .collect();
        found.sort();
        found
    }

    /// Every line after the header must hold exactly one event, seqs 1..=n.
    fn assert_one_event_per_line(path: &Path) -> Vec<Event> {
        let content = std::fs::read_to_string(path).unwrap();
        assert!(content.ends_with('\n'), "log must end in a newline");
        let events: Vec<Event> = content
            .lines()
            .skip(1)
            .map(|l| {
                serde_json::from_str(l)
                    .unwrap_or_else(|e| panic!("line {l:?} is not one event: {e}"))
            })
            .collect();
        let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, (1..=events.len() as u64).collect::<Vec<_>>());
        events
    }

    #[test]
    fn append_writes_one_line_and_leaves_a_well_formed_log_byte_for_byte() {
        let dir = TempDir::new().unwrap();
        let path = write_state_file(dir.path(), "wf", &make_header(), &[intent(1, "a")]);
        let before = std::fs::read_to_string(&path).unwrap();
        let seq = append_event(
            &path,
            &EventPayload::IntentUpdated {
                intent: "b".to_string(),
            },
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        assert_eq!(seq, 2);
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after, format!("{before}{}\n", line(&intent(2, "b"))));
        assert!(backups(dir.path()).is_empty());
    }

    /// The precursor of a merge: a writer died after writing an event and
    /// before its newline. The next append supplies the newline instead of
    /// starting on the same line.
    #[test]
    fn append_after_an_unterminated_complete_event_starts_a_new_line() {
        let dir = TempDir::new().unwrap();
        let path = log_with_tail(dir.path(), &[intent(1, "a")], &line(&intent(2, "b")));
        let seq = append_event(
            &path,
            &EventPayload::IntentUpdated {
                intent: "c".to_string(),
            },
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        assert_eq!(seq, 3);
        let events = assert_one_event_per_line(&path);
        assert_eq!(events.len(), 3);
    }

    /// koto#419 as seen: two complete events on the last line with no
    /// newline between them refused every later write. The append now
    /// splits them, keeps the log as it was in a backup, and goes on.
    #[test]
    fn append_splits_a_merged_last_line_and_keeps_a_backup() {
        let dir = TempDir::new().unwrap();
        let big = "x".repeat(10_000);
        let merged = format!(
            "{}{}{}\n",
            line(&intent(3, "small")),
            line(&intent(4, &big)),
            line(&intent(5, "medium sized intent"))
        );
        let path = log_with_tail(dir.path(), &[intent(1, "a"), intent(2, "b")], &merged);
        let original = std::fs::read_to_string(&path).unwrap();
        #[cfg(unix)]
        let inode = {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(&path).unwrap().ino()
        };

        let seq = append_event(
            &path,
            &EventPayload::IntentUpdated {
                intent: "after".to_string(),
            },
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        assert_eq!(seq, 6);
        let events = assert_one_event_per_line(&path);
        assert_eq!(events.len(), 6);
        match &events[3].payload {
            EventPayload::IntentUpdated { intent } => assert_eq!(intent, &big),
            other => panic!("unexpected payload {other:?}"),
        }

        let backups = backups(dir.path());
        assert_eq!(backups.len(), 1, "{backups:?}");
        assert!(
            backups[0]
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .ends_with(".seq3-merged.bak"),
            "{backups:?}"
        );
        assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), original);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let mode = std::fs::metadata(&backups[0]).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            // Split in place: a tick's `flock` on the log's inode still holds.
            assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode);
        }
        // Everything before the merged line is byte-for-byte what it was.
        let prefix = &original[..original.len() - merged.len()];
        assert!(std::fs::read_to_string(&path).unwrap().starts_with(prefix));
    }

    /// A merged line followed by blank lines splits too, and the log ends
    /// right after the split events and the new one.
    #[test]
    fn split_drops_blank_lines_after_a_merged_last_line() {
        let dir = TempDir::new().unwrap();
        let merged = format!("{}{}\n\n\n", line(&intent(2, "b")), line(&intent(3, "c")));
        let path = log_with_tail(dir.path(), &[intent(1, "a")], &merged);
        append_event(
            &path,
            &EventPayload::IntentUpdated {
                intent: "d".to_string(),
            },
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        assert_eq!(assert_one_event_per_line(&path).len(), 4);
        assert!(!std::fs::read_to_string(&path).unwrap().contains("\n\n"));
    }

    /// The merged line without its own newline, and as the log's only
    /// event line, splits the same way; the idempotent path splits too.
    #[test]
    fn idempotent_append_splits_an_unterminated_merged_first_event_line() {
        let dir = TempDir::new().unwrap();
        let merged = format!("{}{}", line(&intent(1, "a")), line(&intent(2, "b")));
        let path = log_with_tail(dir.path(), &[], &merged);
        let outcome = append_event_idempotent(
            &path,
            &EventPayload::IntentUpdated {
                intent: "c".to_string(),
            },
            "2026-01-01T00:00:00Z",
            "s",
            Some("hash-c"),
        )
        .unwrap();
        assert_eq!(outcome, AppendOutcome::Written { seq: 3 });
        assert_eq!(assert_one_event_per_line(&path).len(), 3);
        assert_eq!(backups(dir.path()).len(), 1);
    }

    /// A genuinely truncated last line behaves as it always has: the
    /// append is refused with the same message and nothing is touched.
    #[test]
    fn append_still_refuses_a_truncated_last_line_and_touches_nothing() {
        let dir = TempDir::new().unwrap();
        let full = line(&intent(2, "b"));
        for tail in [
            format!("{}\n", &full[..full.len() / 2]),
            full[..full.len() - 1].to_string(),
            // A complete event followed by the start of another.
            format!("{}{}\n", full, &full[..10]),
        ] {
            let path = log_with_tail(dir.path(), &[intent(1, "a")], &tail);
            let before = std::fs::read_to_string(&path).unwrap();
            let err = append_event(
                &path,
                &EventPayload::IntentUpdated {
                    intent: "c".to_string(),
                },
                "2026-01-01T00:00:00Z",
            )
            .unwrap_err();
            assert!(
                err.to_string()
                    .starts_with("failed to parse last event line: "),
                "{err}"
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
            assert!(backups(dir.path()).is_empty());
        }
    }

    /// Merged events whose seqs don't continue the log are left alone, and
    /// the refusal names the line and how to repair it.
    #[test]
    fn append_refuses_merged_events_that_do_not_follow_and_names_the_line() {
        let dir = TempDir::new().unwrap();
        let merged = format!("{}{}\n", line(&intent(5, "x")), line(&intent(6, "y")));
        let path = log_with_tail(dir.path(), &[intent(1, "a"), intent(2, "b")], &merged);
        let before = std::fs::read_to_string(&path).unwrap();
        let err = append_event(
            &path,
            &EventPayload::IntentUpdated {
                intent: "c".to_string(),
            },
            "2026-01-01T00:00:00Z",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("line 4 of "), "{msg}");
        assert!(msg.contains("seq 5, 6"), "{msg}");
        assert!(
            msg.contains("put each of those events on a line of its own"),
            "{msg}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        assert!(backups(dir.path()).is_empty());
    }

    /// A reader sees a merged final line as the events it holds, instead of
    /// dropping both as a torn tail; a torn tail is still dropped.
    #[test]
    fn reader_reads_a_merged_final_line_as_its_events() {
        let dir = TempDir::new().unwrap();
        let merged = format!("{}{}\n", line(&intent(2, "b")), line(&intent(3, "c")));
        let path = log_with_tail(dir.path(), &[intent(1, "a")], &merged);
        let (_, events) = read_events(&path).unwrap();
        assert_eq!(
            events.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );

        let full = line(&intent(2, "b"));
        let path = log_with_tail(dir.path(), &[intent(1, "a")], &full[..full.len() - 3]);
        let (_, events) = read_events(&path).unwrap();
        assert_eq!(events.len(), 1);

        // Merged events that don't continue the log are a torn tail to the
        // reader, as before.
        let merged = format!("{}{}\n", line(&intent(7, "b")), line(&intent(8, "c")));
        let path = log_with_tail(dir.path(), &[intent(1, "a")], &merged);
        let (_, events) = read_events(&path).unwrap();
        assert_eq!(events.len(), 1);
    }

    // -----------------------------------------------------------------------
    // The supersede fence
    // -----------------------------------------------------------------------

    fn fence(by: &str, id: &str) -> SupersedeFence {
        SupersedeFence {
            by: by.to_string(),
            by_session_id: id.to_string(),
            actor: SupersedeActor {
                claude_session: None,
                host: Some("box".to_string()),
                user: Some("dev".to_string()),
            },
            at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    /// A session log `p` holding a header and one transition.
    fn fenceable_log(dir: &Path) -> std::path::PathBuf {
        let session = dir.join("p");
        std::fs::create_dir_all(&session).unwrap();
        let path = session.join("koto-p.state.jsonl");
        append_header(&path, &make_header()).unwrap();
        append_event(
            &path,
            &EventPayload::Transitioned {
                from: None,
                to: "start".to_string(),
                condition_type: "auto".to_string(),
                skip_if_matched: None,
                context_assignments: None,
                vars_matched: None,
            },
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        path
    }

    fn intent_payload() -> EventPayload {
        EventPayload::IntentUpdated {
            intent: "more".to_string(),
        }
    }

    #[test]
    fn a_fence_refuses_every_later_append_but_a_read() {
        let dir = TempDir::new().unwrap();
        let path = fenceable_log(dir.path());
        let outcome = append_supersede_fence(
            &path,
            &fence("s", "id-s"),
            "2026-01-01T00:00:01Z",
            FENCE_LOCK_WAIT,
        )
        .unwrap();
        assert_eq!(outcome, FenceOutcome::Fenced { seq: 2 });
        let before = std::fs::read(&path).unwrap();

        let err = append_event(&path, &intent_payload(), "2026-01-01T00:00:02Z").unwrap_err();
        let refused = session_superseded(&err).expect("a typed refusal");
        assert_eq!(refused.session, "p");
        assert_eq!(refused.fence.by, "s");
        assert!(err.to_string().contains("session_superseded"));
        assert_eq!(std::fs::read(&path).unwrap(), before, "nothing was written");

        let err = append_event_idempotent(
            &path,
            &intent_payload(),
            "2026-01-01T00:00:02Z",
            "start",
            Some("h"),
        )
        .unwrap_err();
        assert!(session_superseded(&err).is_some());
        assert_eq!(std::fs::read(&path).unwrap(), before);

        let read = EventPayload::ContextRead {
            key: "record/a".to_string(),
            reader: "cli".to_string(),
            state: "start".to_string(),
            present: true,
            hash: None,
            access: None,
            gate: None,
            session: None,
        };
        assert_eq!(
            append_event(&path, &read, "2026-01-01T00:00:03Z").unwrap(),
            3
        );
    }

    #[test]
    fn the_first_fence_wins_and_a_rerun_finds_its_own() {
        let dir = TempDir::new().unwrap();
        let path = fenceable_log(dir.path());
        let ts = "2026-01-01T00:00:01Z";
        assert!(matches!(
            append_supersede_fence(&path, &fence("s", "id-s"), ts, FENCE_LOCK_WAIT).unwrap(),
            FenceOutcome::Fenced { .. }
        ));
        let after_first = std::fs::read(&path).unwrap();
        assert_eq!(
            append_supersede_fence(&path, &fence("s", "id-s"), ts, FENCE_LOCK_WAIT).unwrap(),
            FenceOutcome::AlreadyOurs
        );
        assert_eq!(
            append_supersede_fence(&path, &fence("t", "id-t"), ts, FENCE_LOCK_WAIT).unwrap(),
            FenceOutcome::SupersededByOther(fence("s", "id-s"))
        );
        // Same name, another session id: another session, not a re-run.
        assert_eq!(
            append_supersede_fence(&path, &fence("s", "id-other"), ts, FENCE_LOCK_WAIT).unwrap(),
            FenceOutcome::SupersededByOther(fence("s", "id-s"))
        );
        assert_eq!(std::fs::read(&path).unwrap(), after_first);

        let (_, events) = read_events(&path).unwrap();
        assert_eq!(superseded_by(&events), Some(fence("s", "id-s")));
        assert_eq!(read_fence(&path).unwrap(), Some(fence("s", "id-s")));
    }

    #[test]
    fn a_log_with_no_fence_reads_as_none() {
        let dir = TempDir::new().unwrap();
        let path = fenceable_log(dir.path());
        assert_eq!(read_fence(&path).unwrap(), None);
        assert_eq!(read_fence(&dir.path().join("missing.jsonl")).unwrap(), None);
        // A line mentioning the name that isn't the event is not a fence.
        append_event(
            &path,
            &EventPayload::IntentUpdated {
                intent: "session_superseded".to_string(),
            },
            "2026-01-01T00:00:01Z",
        )
        .unwrap();
        assert_eq!(read_fence(&path).unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_held_lock_makes_the_fence_unavailable_and_writes_nothing() {
        use std::os::fd::AsRawFd;
        let dir = TempDir::new().unwrap();
        let path = fenceable_log(dir.path());
        let before = std::fs::read(&path).unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.parent().unwrap().join(APPEND_LOCK_FILE))
            .unwrap();
        // SAFETY: flock on a descriptor `lock` owns for the whole test.
        assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
        let outcome = append_supersede_fence(
            &path,
            &fence("s", "id-s"),
            "2026-01-01T00:00:01Z",
            std::time::Duration::from_millis(50),
        )
        .unwrap();
        assert_eq!(outcome, FenceOutcome::Unavailable);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        drop(lock);
        assert!(matches!(
            append_supersede_fence(
                &path,
                &fence("s", "id-s"),
                "2026-01-01T00:00:01Z",
                std::time::Duration::from_millis(50),
            )
            .unwrap(),
            FenceOutcome::Fenced { .. }
        ));
    }
}
