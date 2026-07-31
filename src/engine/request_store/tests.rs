//! Tests for the request store.
//!
//! In-module rather than under `tests/` on purpose: the layout helpers
//! and the lock-held body are private — the module's whole safety
//! argument rests on no caller outside it holding the log path — so
//! the tests that drive them have to live inside the module too.

use super::*;
use crate::engine::types::TerminalOutcome;
use std::sync::{Arc, Barrier};

// ===== Fixtures =====

fn ts(n: u32) -> String {
    format!("2026-01-01T00:00:{:02}.000Z", n)
}

fn declaration(role: &str) -> LegDeclaration {
    LegDeclaration {
        role: role.to_string(),
        template: "review".to_string(),
        inputs: serde_json::json!({"brief": role}),
    }
}

fn two_leg_spec() -> NewRequest {
    NewRequest {
        requested_by: "coord-a".to_string(),
        coordinator_of_record: "coord-a".to_string(),
        legs: vec![
            LegSpec {
                name: "reviewer-b".to_string(),
                declaration: declaration("security"),
            },
            LegSpec {
                name: "reviewer-a".to_string(),
                declaration: declaration("perf"),
            },
        ],
        inputs: Some(serde_json::json!({"pr": 42})),
        created_at: ts(0),
    }
}

/// Create a two-leg request in a fresh workspace.
fn seed(root: &Path) -> ValidatedRequestId {
    create_request(root, &two_leg_spec(), &RequestBounds::default()).expect("create must succeed")
}

fn result(summary: &str) -> WorkflowResult {
    WorkflowResult {
        status: TerminalOutcome::Success,
        summary: summary.to_string(),
        payload: None,
    }
}

fn resolve(root: &Path, id: &ValidatedRequestId, leg: &str, summary: &str) -> AppendResult {
    record_result(
        root,
        id,
        &LegResult {
            leg_name: leg.to_string(),
            result: result(summary),
            source: LegResultSource::Explicit,
            issued_by: Some("coord-a".to_string()),
            timestamp: ts(3),
        },
    )
    .expect("resolve must succeed")
}

fn progress_content(note: &str) -> BTreeMap<String, serde_json::Value> {
    let mut content = BTreeMap::new();
    content.insert("note".to_string(), serde_json::json!(note));
    content
}

// ===== Issue 3: layout, header, and the view =====

#[test]
fn create_lays_out_the_request_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);

    let request_dir = root.join("requests").join(id.as_str());
    assert!(request_dir.is_dir(), "the request directory must exist");
    assert!(
        request_dir.join("request.jsonl").is_file(),
        "the log must exist"
    );
}

#[test]
#[cfg(unix)]
fn directories_are_0700_and_the_log_is_0600() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);

    let mode = |p: PathBuf| {
        std::fs::metadata(&p)
            .unwrap_or_else(|e| panic!("stat {}: {e}", p.display()))
            .permissions()
            .mode()
            & 0o777
    };

    assert_eq!(mode(requests_root(root)), 0o700, "requests/ must be 0700");
    assert_eq!(
        mode(request_dir(root, &id)),
        0o700,
        "the request directory must be 0700"
    );
    assert_eq!(mode(log_path(root, &id)), 0o600, "the log must be 0600");

    // The lock is created on the first append, with the same restraint.
    resolve(root, &id, "reviewer-a", "done");
    assert_eq!(mode(lock_path(root, &id)), 0o600, "the lock must be 0600");
}

#[test]
fn creation_is_one_write_carrying_both_the_header_and_the_event() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);

    let content = std::fs::read_to_string(log_path(root, &id)).expect("read log");
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "a fresh request is exactly a header line and a creation event"
    );

    let header: RequestHeader = serde_json::from_str(lines[0]).expect("header must parse");
    assert_eq!(header.schema_version, REQUEST_SCHEMA_VERSION);
    assert_eq!(header.request_id, id.as_str());
    assert_eq!(header.requested_by, "coord-a");
    assert_eq!(header.coordinator_of_record, "coord-a");
    assert_eq!(header.created_at, ts(0));

    let event: Event = serde_json::from_str(lines[1]).expect("event must parse");
    assert_eq!(event.seq, 1);
    assert_eq!(event.event_type, "request.created");

    // A crash cannot leave a header with an empty log: there is no
    // intermediate state in which the file exists without its
    // creation event.
    assert!(content.ends_with('\n'), "the log must end in a newline");
}

#[test]
fn a_colliding_request_id_is_refused_by_the_rename() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = ValidatedRequestId::new("req-fixed-id").expect("valid id");

    create_request_with_id(root, &id, &two_leg_spec(), &RequestBounds::default())
        .expect("first create must succeed");
    let err = create_request_with_id(root, &id, &two_leg_spec(), &RequestBounds::default())
        .expect_err("second create must be refused");

    assert!(
        matches!(err, RequestStoreError::RequestIdCollision { .. }),
        "want RequestIdCollision, got {err:?}"
    );
    // The refused create left the original record intact.
    let view = read_view(root, &id).expect("the original must still read");
    assert_eq!(view.revision, 1);
}

#[test]
fn generated_identifiers_are_lowercase_and_pass_their_own_constructor() {
    for _ in 0..16 {
        let id = ValidatedRequestId::generate();
        assert!(
            !id.as_str().chars().any(|c| c.is_ascii_uppercase()),
            "generated ids must be single-case: {id}"
        );
        ValidatedRequestId::new(id.as_str()).expect("a generated id must re-validate");
    }
}

#[test]
fn an_uppercase_identifier_is_refused() {
    let err = ValidatedRequestId::new("req-ABC").expect_err("uppercase must be refused");
    assert!(
        matches!(err, RequestStoreError::InvalidRequestId { .. }),
        "want InvalidRequestId, got {err:?}"
    );
}

#[test]
fn reading_an_absent_request_is_distinguishably_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let id = ValidatedRequestId::new("req-absent").expect("valid id");
    let err = read_view(dir.path(), &id).expect_err("must not be found");
    assert!(
        matches!(err, RequestStoreError::NotFound { .. }),
        "want NotFound, got {err:?}"
    );
}

#[test]
fn an_open_request_projects_every_leg_open() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);

    let view = read_view(root, &id).expect("read");
    assert_eq!(view.request_state, RequestState::Open);
    assert_eq!(view.close_disposition, None);
    assert_eq!(view.revision, 1, "revision is the last event's seq");
    assert_eq!(
        view.leg_counts(),
        LegCounts {
            total: 2,
            open: 2,
            resolved: 0,
            abandoned: 0
        }
    );
    for leg in view.legs.values() {
        assert_eq!(leg.disposition, LegDisposition::Open);
        assert!(leg.result.is_none());
        assert!(leg.bound_child.is_none());
        assert!(leg.progress.is_empty());
    }
}

#[test]
fn legs_project_in_name_order_whatever_order_they_were_declared_in() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    // `two_leg_spec` declares reviewer-b first.
    let id = seed(root);

    let view = read_view(root, &id).expect("read");
    let names: Vec<&str> = view.legs.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        vec!["reviewer-a", "reviewer-b"],
        "the view's map is canonically ordered, which is what makes two reads byte-equal"
    );
}

#[test]
fn a_partially_resolved_request_projects_one_result_and_one_open_leg() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);

    let appended = resolve(root, &id, "reviewer-a", "looks fine");
    assert!(appended.written);
    assert_eq!(appended.revision, 2);

    let view = read_view(root, &id).expect("read");
    assert_eq!(view.revision, 2, "the revision advanced with the append");
    let resolved = view.leg("reviewer-a").expect("leg");
    assert_eq!(resolved.disposition, LegDisposition::Resolved);
    assert_eq!(
        resolved.result.as_ref().map(|r| r.summary.as_str()),
        Some("looks fine")
    );
    assert_eq!(resolved.result_source, Some(LegResultSource::Explicit));
    assert_eq!(
        view.leg("reviewer-b").expect("leg").disposition,
        LegDisposition::Open
    );
    assert_eq!(
        view.leg_counts(),
        LegCounts {
            total: 2,
            open: 1,
            resolved: 1,
            abandoned: 0
        }
    );
}

#[test]
fn an_abandoned_leg_projects_abandoned_with_its_rationale() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);

    abandon_leg(
        root,
        &id,
        &AbandonLeg {
            leg_name: "reviewer-b".to_string(),
            rationale: "shipping without the perf pass".to_string(),
            issued_by: Some("coord-a".to_string()),
            timestamp: ts(4),
        },
    )
    .expect("abandon");

    let view = read_view(root, &id).expect("read");
    let leg = view.leg("reviewer-b").expect("leg");
    assert_eq!(leg.disposition, LegDisposition::Abandoned);
    assert_eq!(
        leg.abandoned_rationale.as_deref(),
        Some("shipping without the perf pass")
    );
}

#[test]
fn a_closed_request_projects_closed_with_a_disposition() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);
    resolve(root, &id, "reviewer-a", "a");
    resolve(root, &id, "reviewer-b", "b");

    close_request(
        root,
        &id,
        &CloseRequest {
            disposition: None,
            issued_by: Some("coord-a".to_string()),
            timestamp: ts(5),
        },
    )
    .expect("close");

    let view = read_view(root, &id).expect("read");
    assert_eq!(view.request_state, RequestState::Closed);
    assert_eq!(
        view.close_disposition,
        Some(CloseDisposition::AllResolved),
        "every leg answered, so the derived disposition says so"
    );
}

#[test]
fn closing_with_an_abandoned_leg_derives_the_partial_disposition() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);
    resolve(root, &id, "reviewer-a", "a");
    abandon_leg(
        root,
        &id,
        &AbandonLeg {
            leg_name: "reviewer-b".to_string(),
            rationale: "out of time".to_string(),
            issued_by: None,
            timestamp: ts(4),
        },
    )
    .expect("abandon");

    close_request(
        root,
        &id,
        &CloseRequest {
            disposition: None,
            issued_by: None,
            timestamp: ts(5),
        },
    )
    .expect("close");

    let view = read_view(root, &id).expect("read");
    assert_eq!(
        view.close_disposition,
        Some(CloseDisposition::ClosedWithAbandonedLegs)
    );
}

#[test]
fn the_view_exposes_the_shared_inputs_recorded_at_creation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);

    let view = read_view(root, &id).expect("read");
    assert_eq!(
        view.inputs,
        Some(serde_json::json!({"pr": 42})),
        "shared context must be readable, or accepting it would make it write-only"
    );
    // Per-leg inputs stay on the declaration.
    assert_eq!(
        view.leg("reviewer-a").expect("leg").declaration.inputs,
        serde_json::json!({"brief": "perf"})
    );
}

#[test]
fn progress_entries_read_back_in_the_order_they_were_appended() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);

    for i in 0..10 {
        append_progress(
            root,
            &id,
            &LegProgress {
                leg_name: "reviewer-a".to_string(),
                content: progress_content(&format!("step {i}")),
                issued_by: Some("delegate".to_string()),
                timestamp: ts(i as u32 + 1),
            },
            &RequestBounds::default(),
        )
        .expect("progress");
    }

    let view = read_view(root, &id).expect("read");
    let leg = view.leg("reviewer-a").expect("leg");
    assert_eq!(leg.progress.len(), 10);
    let notes: Vec<&serde_json::Value> = leg
        .progress
        .iter()
        .map(|p| p.content.get("note").expect("note"))
        .collect();
    for (i, note) in notes.iter().enumerate() {
        assert_eq!(**note, serde_json::json!(format!("step {i}")));
    }
    // The sequence numbers are strictly increasing, which is the
    // ordering key rather than the timestamp.
    let seqs: Vec<u64> = leg.progress.iter().map(|p| p.seq).collect();
    let mut sorted = seqs.clone();
    sorted.sort_unstable();
    assert_eq!(seqs, sorted);
    assert_eq!(view.revision, 11);
}

#[test]
fn two_reads_of_an_unchanged_request_are_byte_equal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);
    resolve(root, &id, "reviewer-a", "a");
    append_progress(
        root,
        &id,
        &LegProgress {
            leg_name: "reviewer-b".to_string(),
            content: progress_content("halfway"),
            issued_by: None,
            timestamp: ts(6),
        },
        &RequestBounds::default(),
    )
    .expect("progress");

    let first = serde_json::to_string(&read_view(root, &id).expect("read")).expect("serialize");
    let second = serde_json::to_string(&read_view(root, &id).expect("read")).expect("serialize");
    assert_eq!(first, second);
}

// ===== Identifier and leg-name validation =====

#[test]
fn traversal_shapes_are_rejected_before_the_filesystem_is_touched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    for candidate in [
        "..",
        ".",
        "../../etc/passwd",
        "a/b",
        "/absolute",
        "a\\b",
        "a\0b",
        "req.id",
        "",
    ] {
        let id_err = ValidatedRequestId::new(candidate)
            .err()
            .unwrap_or_else(|| panic!("{candidate:?} must be refused as a request id"));
        assert!(
            matches!(id_err, RequestStoreError::InvalidRequestId { .. }),
            "want InvalidRequestId for {candidate:?}, got {id_err:?}"
        );

        // `.` and `..` are legal *session* id characters but not legal
        // member names, so the leg grammar refuses the same shapes.
        let leg_err = validate_leg_name(candidate)
            .err()
            .unwrap_or_else(|| panic!("{candidate:?} must be refused as a leg name"));
        assert!(
            matches!(leg_err, RequestStoreError::InvalidLegName { .. }),
            "want InvalidLegName for {candidate:?}, got {leg_err:?}"
        );
    }

    // Nothing was created: rejection happens before any path is joined.
    assert!(
        !requests_root(root).exists(),
        "a rejected identifier must not create the requests directory"
    );
}

#[test]
fn a_leading_hyphen_is_refused_because_it_reads_as_a_flag() {
    let err = validate_leg_name("--rationale").expect_err("must be refused");
    assert!(
        matches!(err, RequestStoreError::InvalidLegName { .. }),
        "want InvalidLegName, got {err:?}"
    );
}

#[test]
fn creating_with_a_traversal_leg_name_writes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let spec = NewRequest {
        legs: vec![LegSpec {
            name: "../escape".to_string(),
            declaration: declaration("x"),
        }],
        ..two_leg_spec()
    };

    let err = create_request(root, &spec, &RequestBounds::default()).expect_err("must be refused");
    assert!(
        matches!(err, RequestStoreError::InvalidLegName { .. }),
        "want InvalidLegName, got {err:?}"
    );
    assert!(
        !requests_root(root).exists(),
        "a rejected creation must leave no directory behind"
    );
}

// ===== Issue 4: preconditions =====

#[test]
fn a_second_result_on_a_resolved_leg_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);
    resolve(root, &id, "reviewer-a", "first answer");

    let err = record_result(
        root,
        &id,
        &LegResult {
            leg_name: "reviewer-a".to_string(),
            result: result("second answer"),
            source: LegResultSource::Explicit,
            issued_by: None,
            timestamp: ts(7),
        },
    )
    .expect_err("a second result must be rejected");

    assert!(
        matches!(err, RequestStoreError::LegAlreadyResolved { .. }),
        "want LegAlreadyResolved, got {err:?}"
    );
    let view = read_view(root, &id).expect("read");
    assert_eq!(
        view.leg("reviewer-a")
            .expect("leg")
            .result
            .as_ref()
            .map(|r| r.summary.as_str()),
        Some("first answer"),
        "the first answer stands"
    );
}

#[test]
fn a_result_on_an_abandoned_leg_is_rejected_distinctly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);
    abandon_leg(
        root,
        &id,
        &AbandonLeg {
            leg_name: "reviewer-a".to_string(),
            rationale: "no longer needed".to_string(),
            issued_by: None,
            timestamp: ts(4),
        },
    )
    .expect("abandon");

    let err = record_result(
        root,
        &id,
        &LegResult {
            leg_name: "reviewer-a".to_string(),
            result: result("too late"),
            source: LegResultSource::Promoted,
            issued_by: None,
            timestamp: ts(7),
        },
    )
    .expect_err("a result on an abandoned leg must be rejected");

    assert!(
        matches!(err, RequestStoreError::LegAbandoned { .. }),
        "want LegAbandoned — distinct from LegAlreadyResolved so the caller can tell \
         'someone beat you to it' from 'nobody is waiting' — got {err:?}"
    );
}

#[test]
fn rebinding_to_the_same_child_is_a_no_op_success() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);
    let bind = BindLeg {
        leg_name: "reviewer-a".to_string(),
        child_session_id: "child-1".to_string(),
        dispatch_epoch: Some(3),
        issued_by: None,
        timestamp: ts(2),
    };

    let first = bind_leg(root, &id, &bind).expect("first bind");
    assert!(first.written);
    let second = bind_leg(root, &id, &bind).expect("rebinding the same pair must succeed");
    assert!(
        !second.written,
        "the state the caller asked for already held, so nothing is appended"
    );
    assert_eq!(second.revision, first.revision);

    let view = read_view(root, &id).expect("read");
    let leg = view.leg("reviewer-a").expect("leg");
    assert_eq!(leg.bound_child.as_deref(), Some("child-1"));
    assert_eq!(leg.bound_epoch, Some(3));
}

#[test]
fn rebinding_to_a_different_child_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);
    bind_leg(
        root,
        &id,
        &BindLeg {
            leg_name: "reviewer-a".to_string(),
            child_session_id: "child-1".to_string(),
            dispatch_epoch: Some(1),
            issued_by: None,
            timestamp: ts(2),
        },
    )
    .expect("first bind");

    let err = bind_leg(
        root,
        &id,
        &BindLeg {
            leg_name: "reviewer-a".to_string(),
            child_session_id: "child-2".to_string(),
            dispatch_epoch: Some(1),
            issued_by: None,
            timestamp: ts(3),
        },
    )
    .expect_err("a conflicting rebind must be rejected");

    assert!(
        matches!(err, RequestStoreError::LegBoundToDifferentChild { .. }),
        "want LegBoundToDifferentChild, got {err:?}"
    );
}

#[test]
fn closing_an_already_closed_request_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);
    close_request(
        root,
        &id,
        &CloseRequest {
            disposition: Some(CloseDisposition::RequestAbandoned),
            issued_by: None,
            timestamp: ts(5),
        },
    )
    .expect("first close");

    let err = close_request(
        root,
        &id,
        &CloseRequest {
            disposition: None,
            issued_by: None,
            timestamp: ts(6),
        },
    )
    .expect_err("a second close must be rejected");

    assert!(
        matches!(err, RequestStoreError::RequestClosed { .. }),
        "want RequestClosed, got {err:?}"
    );
}

#[test]
fn leg_mutations_on_a_closed_request_are_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);
    close_request(
        root,
        &id,
        &CloseRequest {
            disposition: Some(CloseDisposition::RequestAbandoned),
            issued_by: None,
            timestamp: ts(5),
        },
    )
    .expect("close");

    let err = append_progress(
        root,
        &id,
        &LegProgress {
            leg_name: "reviewer-a".to_string(),
            content: progress_content("still going"),
            issued_by: None,
            timestamp: ts(6),
        },
        &RequestBounds::default(),
    )
    .expect_err("a closed request accepts no leg activity");
    assert!(
        matches!(err, RequestStoreError::RequestClosed { .. }),
        "want RequestClosed, got {err:?}"
    );
}

#[test]
fn a_mutation_naming_an_undeclared_leg_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);

    let err = append_progress(
        root,
        &id,
        &LegProgress {
            leg_name: "reviewer-z".to_string(),
            content: progress_content("hello"),
            issued_by: None,
            timestamp: ts(2),
        },
        &RequestBounds::default(),
    )
    .expect_err("must be rejected");
    assert!(
        matches!(err, RequestStoreError::LegNotFound { .. }),
        "want LegNotFound, got {err:?}"
    );
}

// ===== Issue 4: the five bounds =====

#[test]
fn duplicate_leg_names_are_rejected_at_create() {
    let dir = tempfile::tempdir().expect("tempdir");
    let spec = NewRequest {
        legs: vec![
            LegSpec {
                name: "reviewer-a".to_string(),
                declaration: declaration("perf"),
            },
            LegSpec {
                name: "reviewer-a".to_string(),
                declaration: declaration("security"),
            },
        ],
        ..two_leg_spec()
    };

    let err = create_request(dir.path(), &spec, &RequestBounds::default())
        .expect_err("two legs sharing a name must be refused");
    assert!(
        matches!(err, RequestStoreError::DuplicateLegName { .. }),
        "want DuplicateLegName — without the check one declaration would silently vanish \
         into the view's map — got {err:?}"
    );
}

#[test]
fn the_leg_cap_is_enforced_at_create() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bounds = RequestBounds {
        legs_per_request: 4,
        ..RequestBounds::default()
    };
    let spec = NewRequest {
        legs: (0..5)
            .map(|i| LegSpec {
                name: format!("leg-{i}"),
                declaration: declaration("x"),
            })
            .collect(),
        ..two_leg_spec()
    };

    let err = create_request(dir.path(), &spec, &bounds).expect_err("must be refused");
    match err {
        RequestStoreError::BoundExceeded {
            dimension, limit, ..
        } => {
            assert_eq!(dimension, "legs_per_request");
            assert_eq!(limit, 4);
        }
        other => panic!("want BoundExceeded, got {other:?}"),
    }
}

#[test]
fn the_progress_append_bound_is_enforced_inside_the_lock() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);
    let bounds = RequestBounds {
        progress_appends_per_leg: 3,
        ..RequestBounds::default()
    };

    for i in 0..3 {
        append_progress(
            root,
            &id,
            &LegProgress {
                leg_name: "reviewer-a".to_string(),
                content: progress_content(&format!("step {i}")),
                issued_by: None,
                timestamp: ts(i + 1),
            },
            &bounds,
        )
        .expect("within the bound");
    }

    let err = append_progress(
        root,
        &id,
        &LegProgress {
            leg_name: "reviewer-a".to_string(),
            content: progress_content("one too many"),
            issued_by: None,
            timestamp: ts(9),
        },
        &bounds,
    )
    .expect_err("the fourth append must be rejected");
    match err {
        RequestStoreError::BoundExceeded {
            dimension, limit, ..
        } => {
            assert_eq!(dimension, "progress_appends_per_leg");
            assert_eq!(limit, 3);
        }
        other => panic!("want BoundExceeded, got {other:?}"),
    }

    // Rejecting rather than truncating or rolling over: the three
    // recorded appends are untouched.
    let view = read_view(root, &id).expect("read");
    assert_eq!(view.leg("reviewer-a").expect("leg").progress.len(), 3);
}

#[test]
fn an_oversized_append_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);

    let mut content = BTreeMap::new();
    content.insert(
        "blob".to_string(),
        serde_json::json!("x".repeat(MAX_APPEND_BYTES + 1)),
    );

    let err = append_progress(
        root,
        &id,
        &LegProgress {
            leg_name: "reviewer-a".to_string(),
            content,
            issued_by: None,
            timestamp: ts(2),
        },
        &RequestBounds::default(),
    )
    .expect_err("must be refused");
    match err {
        RequestStoreError::BoundExceeded {
            dimension, limit, ..
        } => {
            assert_eq!(dimension, "append_bytes");
            assert_eq!(limit, MAX_APPEND_BYTES);
        }
        other => panic!("want BoundExceeded, got {other:?}"),
    }
}

#[test]
fn a_too_deep_json_payload_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");

    // Build a value one level past the depth cap.
    let mut deep = serde_json::json!("leaf");
    for _ in 0..MAX_JSON_DEPTH {
        deep = serde_json::Value::Array(vec![deep]);
    }
    let spec = NewRequest {
        legs: vec![LegSpec {
            name: "reviewer-a".to_string(),
            declaration: LegDeclaration {
                role: "perf".to_string(),
                template: "review".to_string(),
                inputs: deep,
            },
        }],
        ..two_leg_spec()
    };

    let err =
        create_request(dir.path(), &spec, &RequestBounds::default()).expect_err("must be refused");
    match err {
        RequestStoreError::BoundExceeded { dimension, .. } => {
            assert_eq!(dimension, "json_depth");
        }
        other => panic!("want BoundExceeded, got {other:?}"),
    }
}

#[test]
fn an_oversized_json_payload_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let spec = NewRequest {
        inputs: Some(serde_json::json!({
            "blob": "x".repeat(MAX_JSON_PAYLOAD_BYTES + 1)
        })),
        ..two_leg_spec()
    };

    let err =
        create_request(dir.path(), &spec, &RequestBounds::default()).expect_err("must be refused");
    match err {
        RequestStoreError::BoundExceeded {
            dimension, limit, ..
        } => {
            assert_eq!(dimension, "request_inputs_bytes");
            assert_eq!(limit, MAX_JSON_PAYLOAD_BYTES);
        }
        other => panic!("want BoundExceeded, got {other:?}"),
    }
}

#[test]
fn an_oversized_rationale_is_rejected_and_control_characters_are_stripped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);

    let err = abandon_leg(
        root,
        &id,
        &AbandonLeg {
            leg_name: "reviewer-a".to_string(),
            rationale: "x".repeat(MAX_RATIONALE_BYTES + 1),
            issued_by: None,
            timestamp: ts(4),
        },
    )
    .expect_err("must be refused");
    match err {
        RequestStoreError::BoundExceeded {
            dimension, limit, ..
        } => {
            assert_eq!(dimension, "rationale_bytes");
            assert_eq!(limit, MAX_RATIONALE_BYTES);
        }
        other => panic!("want BoundExceeded, got {other:?}"),
    }

    abandon_leg(
        root,
        &id,
        &AbandonLeg {
            leg_name: "reviewer-a".to_string(),
            // Control characters and newlines: this text is prepended
            // to a delegate's directive, so neither may survive.
            rationale: "line one\nline two\u{7}\ttail".to_string(),
            issued_by: None,
            timestamp: ts(4),
        },
    )
    .expect("abandon");

    let view = read_view(root, &id).expect("read");
    assert_eq!(
        view.leg("reviewer-a")
            .expect("leg")
            .abandoned_rationale
            .as_deref(),
        Some("line one line two tail")
    );
}

#[test]
fn bounds_read_from_the_request_store_config_table() {
    let config = RequestStoreConfig {
        request_leg_append_cap: 12,
        request_leg_cap: 34,
        ..RequestStoreConfig::default()
    };
    let bounds = RequestBounds::from_config(&config);
    assert_eq!(bounds.progress_appends_per_leg, 12);
    assert_eq!(bounds.legs_per_request, 34);

    // The shipped defaults are the design's numbers.
    let default = RequestBounds::from_config(&RequestStoreConfig::default());
    assert_eq!(default, RequestBounds::default());
    assert_eq!(default.progress_appends_per_leg, 256);
    assert_eq!(default.legs_per_request, 256);
}

// ===== Issue 4: concurrency, retries, and crash safety =====

#[test]
fn two_simultaneous_resolves_of_one_leg_leave_exactly_one_winner() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let id = seed(&root);

    let barrier = Arc::new(Barrier::new(2));
    let outcomes: Vec<Result<AppendResult, RequestStoreError>> = std::thread::scope(|scope| {
        let handles: Vec<_> = ["from thread one", "from thread two"]
            .into_iter()
            .map(|summary| {
                let root = root.clone();
                let id = id.clone();
                let barrier = Arc::clone(&barrier);
                scope.spawn(move || {
                    barrier.wait();
                    record_result(
                        &root,
                        &id,
                        &LegResult {
                            leg_name: "reviewer-a".to_string(),
                            result: result(summary),
                            source: LegResultSource::Explicit,
                            issued_by: None,
                            timestamp: ts(3),
                        },
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("join"))
            .collect()
    });

    let winners = outcomes.iter().filter(|o| o.is_ok()).count();
    assert_eq!(winners, 1, "exactly one resolve may win: {outcomes:?}");
    let loser = outcomes
        .iter()
        .find_map(|o| o.as_ref().err())
        .expect("one must lose");
    assert!(
        matches!(loser, RequestStoreError::LegAlreadyResolved { .. }),
        "the loser must be told why, got {loser:?}"
    );

    // The log survived the race and still reads.
    let view = read_view(&root, &id).expect("the log must stay readable");
    assert_eq!(view.revision, 2, "exactly one event was appended");
    assert_eq!(
        view.leg("reviewer-a").expect("leg").disposition,
        LegDisposition::Resolved
    );
}

#[test]
fn an_identical_retry_collapses_rather_than_double_appending() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);
    let progress = LegProgress {
        leg_name: "reviewer-a".to_string(),
        content: progress_content("halfway"),
        issued_by: Some("delegate".to_string()),
        timestamp: ts(2),
    };

    let first = append_progress(root, &id, &progress, &RequestBounds::default()).expect("first");
    assert!(first.written);
    let retry = append_progress(root, &id, &progress, &RequestBounds::default()).expect("retry");
    assert!(
        !retry.written,
        "the retry must be a no-op, not a double append"
    );

    let view = read_view(root, &id).expect("read");
    assert_eq!(view.leg("reviewer-a").expect("leg").progress.len(), 1);
}

#[test]
fn an_identical_resolve_retry_is_not_a_spurious_second_result_rejection() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);
    let payload = LegResult {
        leg_name: "reviewer-a".to_string(),
        result: result("the answer"),
        source: LegResultSource::Explicit,
        issued_by: None,
        timestamp: ts(3),
    };

    record_result(root, &id, &payload).expect("first");
    let retry = record_result(root, &id, &payload)
        .expect("an identical retry must succeed rather than report a second result");
    assert!(!retry.written);

    let view = read_view(root, &id).expect("read");
    assert_eq!(view.revision, 2, "one event, not two");
}

#[test]
fn a_bounded_lock_wait_surfaces_contention_rather_than_hanging() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);

    // Hold the lock on a separate descriptor. flock conflicts across
    // open file descriptions even within one process, so this is a
    // faithful stand-in for a second writer.
    let _held = acquire_request_lock(root, &id, Duration::from_secs(1)).expect("hold the lock");

    let started = Instant::now();
    let err = append_under_lock(root, &id, Duration::from_millis(100), None, |_| {
        panic!("the precondition must never run without the lock")
    })
    .expect_err("must time out");
    assert!(
        matches!(err, RequestStoreError::LockContention { .. }),
        "want LockContention — a transient class the caller may retry — got {err:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the wait must be bounded, not blocking"
    );
}

#[test]
fn a_truncated_final_line_is_recovered_and_a_broken_middle_line_is_not() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);
    resolve(root, &id, "reviewer-a", "a");
    resolve(root, &id, "reviewer-b", "b");

    let path = log_path(root, &id);
    let original = std::fs::read_to_string(&path).expect("read");

    // Crash mid-write: the last line is cut in half.
    let cut = original.len() - 20;
    std::fs::write(&path, &original[..cut]).expect("truncate");
    let view = read_view(root, &id).expect("a truncated final line is recoverable");
    assert_eq!(
        view.revision, 2,
        "the events before the torn line are still there"
    );
    assert_eq!(
        view.leg("reviewer-b").expect("leg").disposition,
        LegDisposition::Open,
        "the torn event did not land"
    );

    // A broken line that is *not* final is corruption, not recovery:
    // reporting it beats silently dropping the events after it.
    let mut lines: Vec<String> = original.lines().map(str::to_string).collect();
    lines[1] = "{not json".to_string();
    std::fs::write(&path, lines.join("\n") + "\n").expect("write");
    let err = read_view(root, &id).expect_err("a broken middle line must be reported");
    assert!(
        matches!(err, RequestStoreError::Corrupt { .. }),
        "want Corrupt, got {err:?}"
    );
}

#[test]
fn a_torn_tail_is_repaired_before_the_next_append() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);
    resolve(root, &id, "reviewer-a", "a");

    // A crash between the payload write and the newline write leaves a
    // line with no terminator.
    let path = log_path(root, &id);
    let mut content = std::fs::read_to_string(&path).expect("read");
    content.push_str("{\"seq\":3,\"timestamp\":\"2026");
    std::fs::write(&path, &content).expect("write torn tail");

    // Appending must repair it first. Without the repair this append
    // would concatenate onto the partial line and make it a non-final
    // malformed line — permanently fatal.
    resolve(root, &id, "reviewer-b", "b");

    let view = read_view(root, &id).expect("the log must still read");
    assert_eq!(view.revision, 3);
    assert_eq!(
        view.leg("reviewer-b")
            .expect("leg")
            .result
            .as_ref()
            .map(|r| r.summary.as_str()),
        Some("b")
    );
    let repaired = std::fs::read_to_string(&path).expect("read");
    assert!(repaired.ends_with('\n'), "the log ends in a newline again");
    assert_eq!(repaired.lines().count(), 4, "header plus three events");
}

#[test]
fn repairing_a_log_that_needs_nothing_changes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);
    let path = log_path(root, &id);
    let before = std::fs::read(&path).expect("read");

    assert!(!repair_torn_tail(&path).expect("repair"), "nothing to do");
    assert_eq!(std::fs::read(&path).expect("read"), before);
}

// ===== Issue 5: listing =====

fn seeded_workspace() -> (tempfile::TempDir, Vec<ValidatedRequestId>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();

    let open = create_request_with_id(
        &root,
        &ValidatedRequestId::new("req-open").expect("id"),
        &two_leg_spec(),
        &RequestBounds::default(),
    )
    .expect("create");

    let other_requester = create_request_with_id(
        &root,
        &ValidatedRequestId::new("req-other").expect("id"),
        &NewRequest {
            requested_by: "coord-b".to_string(),
            coordinator_of_record: "coord-c".to_string(),
            ..two_leg_spec()
        },
        &RequestBounds::default(),
    )
    .expect("create");

    let closed = create_request_with_id(
        &root,
        &ValidatedRequestId::new("req-closed").expect("id"),
        &two_leg_spec(),
        &RequestBounds::default(),
    )
    .expect("create");
    resolve(&root, &closed, "reviewer-a", "a");
    resolve(&root, &closed, "reviewer-b", "b");
    close_request(
        &root,
        &closed,
        &CloseRequest {
            disposition: None,
            issued_by: None,
            timestamp: ts(5),
        },
    )
    .expect("close");

    (dir, vec![closed, open, other_requester])
}

#[test]
fn listing_returns_every_request_in_canonical_order() {
    let (dir, _) = seeded_workspace();
    let rows = list_requests(dir.path(), &ListFilter::default()).expect("list");
    let ids: Vec<&str> = rows.iter().map(|r| r.request_id.as_str()).collect();
    assert_eq!(ids, vec!["req-closed", "req-open", "req-other"]);

    let closed = &rows[0];
    assert_eq!(closed.request_state, RequestState::Closed);
    assert_eq!(
        closed.leg_counts,
        LegCounts {
            total: 2,
            open: 0,
            resolved: 2,
            abandoned: 0
        }
    );
    assert_eq!(closed.requested_by, "coord-a");
    assert_eq!(closed.coordinator_of_record, "coord-a");
    assert_eq!(closed.created_at, ts(0));
}

#[test]
fn listing_filters_by_requester_and_by_coordinator_of_record() {
    let (dir, _) = seeded_workspace();

    let by_requester = list_requests(
        dir.path(),
        &ListFilter {
            requested_by: Some("coord-b".to_string()),
            ..ListFilter::default()
        },
    )
    .expect("list");
    assert_eq!(
        by_requester
            .iter()
            .map(|r| r.request_id.as_str())
            .collect::<Vec<_>>(),
        vec!["req-other"]
    );

    let by_coordinator = list_requests(
        dir.path(),
        &ListFilter {
            coordinator_of_record: Some("coord-c".to_string()),
            ..ListFilter::default()
        },
    )
    .expect("list");
    assert_eq!(
        by_coordinator
            .iter()
            .map(|r| r.request_id.as_str())
            .collect::<Vec<_>>(),
        vec!["req-other"]
    );
}

#[test]
fn listing_filters_by_state_and_by_unresolved_legs() {
    let (dir, _) = seeded_workspace();

    let open = list_requests(
        dir.path(),
        &ListFilter {
            state: Some(RequestState::Open),
            ..ListFilter::default()
        },
    )
    .expect("list");
    assert_eq!(
        open.iter()
            .map(|r| r.request_id.as_str())
            .collect::<Vec<_>>(),
        vec!["req-open", "req-other"]
    );

    let unresolved = list_requests(
        dir.path(),
        &ListFilter {
            unresolved_legs_only: true,
            ..ListFilter::default()
        },
    )
    .expect("list");
    assert_eq!(
        unresolved
            .iter()
            .map(|r| r.request_id.as_str())
            .collect::<Vec<_>>(),
        vec!["req-open", "req-other"],
        "the fully resolved request has no open leg left"
    );
}

#[test]
fn listing_writes_nothing_and_advances_no_cursor() {
    let (dir, _) = seeded_workspace();
    let root = dir.path();

    let before = snapshot(root);
    list_requests(root, &ListFilter::default()).expect("list");
    list_requests(
        root,
        &ListFilter {
            state: Some(RequestState::Open),
            ..ListFilter::default()
        },
    )
    .expect("list");
    let after = snapshot(root);

    assert_eq!(
        before, after,
        "listing is a pure read: no file created, grown, or restamped"
    );
    assert!(
        !root.join("coordinators").exists(),
        "listing must not touch the dispatch cursor"
    );
}

/// Every file under `root` with its length and modification time.
fn snapshot(root: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read_dir") {
            let entry = entry.expect("entry");
            let md = entry.metadata().expect("metadata");
            if md.is_dir() {
                stack.push(entry.path());
            } else {
                out.push((
                    entry.path(),
                    md.len(),
                    md.modified().expect("modified time"),
                ));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn listing_skips_a_malformed_request_directory_rather_than_failing() {
    let (dir, _) = seeded_workspace();
    let root = dir.path();

    // A directory whose log is unparseable.
    let broken = requests_root(root).join("req-broken");
    std::fs::create_dir_all(&broken).expect("mkdir");
    std::fs::write(broken.join("request.jsonl"), b"not a header\n").expect("write");

    // A directory whose name is not a valid identifier.
    std::fs::create_dir_all(requests_root(root).join("Not.An.Id")).expect("mkdir");

    // A directory with no log at all.
    std::fs::create_dir_all(requests_root(root).join("req-empty")).expect("mkdir");

    let rows =
        list_requests(root, &ListFilter::default()).expect("one bad record must not fail the call");
    assert_eq!(
        rows.iter()
            .map(|r| r.request_id.as_str())
            .collect::<Vec<_>>(),
        vec!["req-closed", "req-open", "req-other"]
    );
}

#[test]
fn listing_an_empty_workspace_is_empty_rather_than_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let rows = list_requests(dir.path(), &ListFilter::default()).expect("list");
    assert!(rows.is_empty());
}

// ===== The generic log primitives =====

#[test]
fn the_request_header_rides_the_shared_reader() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);
    let path = log_path(root, &id);

    // The same `read_log` the session store uses, instantiated at the
    // request header — which is the whole point of generifying it.
    let (header, events) =
        persistence::read_log::<RequestHeader>(&path).expect("the shared reader must parse it");
    assert_eq!(header.request_id, id.as_str());
    assert_eq!(events.len(), 1);

    // And the header-only read stops at line one.
    let header_only =
        persistence::read_header_only::<RequestHeader>(&path).expect("header-only read");
    assert_eq!(header_only, header);
}

#[test]
fn a_request_log_from_a_newer_koto_is_refused_at_line_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);
    let path = log_path(root, &id);

    let content = std::fs::read_to_string(&path).expect("read");
    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
    let mut header: serde_json::Value = serde_json::from_str(&lines[0]).expect("parse header");
    header["schema_version"] = serde_json::json!(REQUEST_SCHEMA_VERSION + 1);
    lines[0] = serde_json::to_string(&header).expect("serialize");
    std::fs::write(&path, lines.join("\n") + "\n").expect("write");

    let err = read_view(root, &id).expect_err("a newer log must be refused");
    assert!(
        err.to_string().contains("incompatible schema version"),
        "want a version error, got {err}"
    );
}

#[test]
fn an_unrecognized_request_event_is_skipped_rather_than_fatal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);
    let path = log_path(root, &id);

    // What a newer koto's seventh variant looks like to this build.
    let unknown = serde_json::json!({
        "seq": 2,
        "timestamp": ts(9),
        "type": "request.leg_deferred",
        "payload": {"request_id": id.as_str(), "leg_name": "reviewer-a"}
    });
    let mut content = std::fs::read_to_string(&path).expect("read");
    content.push_str(&serde_json::to_string(&unknown).expect("serialize"));
    content.push('\n');
    std::fs::write(&path, content).expect("write");

    let view = read_view(root, &id).expect("an unknown event must degrade gracefully");
    assert_eq!(
        view.revision, 2,
        "the unknown event still advances the revision"
    );
    assert_eq!(
        view.leg("reviewer-a").expect("leg").disposition,
        LegDisposition::Open
    );
}

#[test]
fn a_log_carrying_another_requests_events_is_corruption() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);
    let path = log_path(root, &id);

    let foreign = Event {
        seq: 2,
        timestamp: ts(9),
        event_type: "request.closed".to_string(),
        payload: EventPayload::RequestClosed {
            request_id: "req-somebody-else".to_string(),
            disposition: CloseDisposition::AllResolved,
            issued_by: None,
        },
        idempotency_hash: None,
    };
    let mut content = std::fs::read_to_string(&path).expect("read");
    content.push_str(&serde_json::to_string(&foreign).expect("serialize"));
    content.push('\n');
    std::fs::write(&path, content).expect("write");

    let err = read_view(root, &id).expect_err("two interleaved logs must not project");
    assert!(
        matches!(err, RequestStoreError::Corrupt { .. }),
        "want Corrupt, got {err:?}"
    );
}

#[test]
fn the_public_primitive_appends_only_when_its_precondition_passes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);

    let payload = EventPayload::RequestLegProgress {
        request_id: id.as_str().to_string(),
        leg_name: "reviewer-a".to_string(),
        content: progress_content("via the primitive"),
        issued_by: None,
    };

    let err = validate_and_append(root, &id, payload.clone(), &ts(2), |view| {
        Err(RequestStoreError::LegNotFound {
            request_id: view.header.request_id.clone(),
            leg_name: "nope".to_string(),
        })
    })
    .expect_err("a rejecting precondition must block the append");
    assert!(matches!(err, RequestStoreError::LegNotFound { .. }));
    assert_eq!(read_view(root, &id).expect("read").revision, 1);

    let revision = validate_and_append(root, &id, payload, &ts(2), |_| Ok(())).expect("append");
    assert_eq!(revision, 2);
}

#[test]
fn summary_field_names_reuse_the_unassigned_child_vocabulary() {
    let (dir, _) = seeded_workspace();
    let rows = list_requests(dir.path(), &ListFilter::default()).expect("list");
    let json = serde_json::to_value(&rows[0]).expect("serialize");
    let keys: Vec<&str> = json
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    // `serde_json::Value` holds object keys sorted, so compare sorted.
    assert_eq!(
        keys,
        vec![
            "coordinator_of_record",
            "created_at",
            "leg_counts",
            "request_id",
            "request_state",
            "requested_by",
        ],
        "`requested_by` and `created_at` are spelled the way an unassigned child spells them, \
         so an operator reading both surfaces learns one vocabulary"
    );
}

#[test]
fn the_quiet_reader_recovers_exactly_what_the_warning_one_does() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let id = seed(root);
    resolve(root, &id, "reviewer-a", "a");

    // A reader racing a concurrent append sees a torn final line. The
    // request path must recover it without a word on stderr — `wait`
    // polls every two seconds, so warning would make normal operation
    // a stderr stream. The two readers differ in that warning alone.
    let path = log_path(root, &id);
    let mut content = std::fs::read_to_string(&path).expect("read");
    content.push_str("{\"seq\":3,\"timest");
    std::fs::write(&path, &content).expect("write");

    let (loud_header, loud_events) =
        persistence::read_log::<RequestHeader>(&path).expect("warning reader");
    let (quiet_header, quiet_events) =
        persistence::read_log_quiet::<RequestHeader>(&path).expect("quiet reader");
    assert_eq!(loud_header, quiet_header);
    assert_eq!(loud_events.len(), quiet_events.len());
    assert_eq!(quiet_events.len(), 2, "both recover the intact events");
}
