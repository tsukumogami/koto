//! Integration coverage for `koto session start` (KT1 Issue 4).
//!
//! Drives [`koto::cli::session::handle_start`] directly against a
//! [`LocalBackend`] so the parse-time validation, companion-flag
//! contract, DoS guards, and header population can be exercised
//! without spawning a CLI subprocess. The handler is the SAME entry
//! point the clap dispatch in `src/cli/mod.rs` calls into; the only
//! thing this skips is the clap layer's flag-name parsing.

use assert_fs::TempDir;

use koto::cli::session;
use koto::engine::types::{Event, EventPayload, StateFileHeader};
use koto::session::local::LocalBackend;
use koto::session::SessionBackend;

/// Construct a `LocalBackend` rooted at `dir` and pre-create the
/// parent session so `handle_start` has something to chain from.
///
/// The parent is given a non-empty `session_id` so the auto-derived
/// `requested_by` populates with a valid identifier on the
/// dispatch-request path.
fn init_parent_backend(dir: &std::path::Path, parent: &str) -> LocalBackend {
    let backend = LocalBackend::with_base_dir(dir.to_path_buf());
    backend.create(parent).expect("create parent");

    let header = StateFileHeader {
        schema_version: 1,
        workflow: parent.to_string(),
        template_hash: "0".repeat(64),
        created_at: "2026-05-24T00:00:00Z".to_string(),
        parent_workflow: None,
        template_source_dir: None,
        session_id: "parent-session-uuid".to_string(),
        intent: None,
        template_name: None,
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
    };
    let init_payload = EventPayload::WorkflowInitialized {
        template_path: String::new(),
        variables: Default::default(),
        spawn_entry: None,
    };
    let event = Event {
        seq: 1,
        timestamp: header.created_at.clone(),
        event_type: init_payload.type_name().to_string(),
        payload: init_payload,
    };
    backend
        .init_state_file(parent, header, vec![event])
        .expect("init parent state file");
    backend
}

#[test]
fn happy_path_populates_request_store_fields() {
    let tmp = TempDir::new().unwrap();
    let backend = init_parent_backend(tmp.path(), "parent");

    session::handle_start(
        &backend,
        "child",
        "parent",
        true,
        Some("scrutineer"),
        Some("verdict"),
        Some(r#"{"draft":"foo","n":42}"#),
        None,
    )
    .expect("happy path must succeed");

    let header = backend.read_header("child").expect("read child header");
    assert_eq!(header.workflow, "child");
    assert_eq!(header.parent_workflow.as_deref(), Some("parent"));
    assert_eq!(header.needs_agent, Some(true));
    assert_eq!(header.role.as_deref(), Some("scrutineer"));
    assert_eq!(header.template_name.as_deref(), Some("verdict"));
    assert_eq!(
        header.inputs.as_ref().expect("inputs serialized")["draft"],
        serde_json::json!("foo")
    );
    assert_eq!(header.dispatch_epoch, 0);
    // Defaults: coordinator falls back to the parent's session_id,
    // and requested_by mirrors the parent's session_id too (since
    // the parent we seeded has no coordinator_of_record yet).
    assert_eq!(
        header.coordinator_of_record.as_deref(),
        Some("parent-session-uuid")
    );
    assert_eq!(header.requested_by.as_deref(), Some("parent-session-uuid"));
    assert_eq!(header.assignment_claim, None);
}

#[test]
fn happy_path_explicit_coordinator_of_record() {
    let tmp = TempDir::new().unwrap();
    let backend = init_parent_backend(tmp.path(), "parent");

    session::handle_start(
        &backend,
        "child",
        "parent",
        true,
        Some("scrutineer"),
        Some("verdict"),
        Some(r#"{}"#),
        Some("explicit-coord-7"),
    )
    .expect("happy path must succeed");

    let header = backend.read_header("child").expect("read child header");
    assert_eq!(
        header.coordinator_of_record.as_deref(),
        Some("explicit-coord-7")
    );
}

#[test]
fn needs_agent_alone_rejects_naming_all_missing_companions() {
    let tmp = TempDir::new().unwrap();
    let backend = init_parent_backend(tmp.path(), "parent");

    let err = session::handle_start(&backend, "child", "parent", true, None, None, None, None)
        .expect_err("must reject");
    let msg = err.to_string();
    assert!(msg.contains("--role"), "must name --role: {}", msg);
    assert!(msg.contains("--template"), "must name --template: {}", msg);
    assert!(msg.contains("--inputs"), "must name --inputs: {}", msg);
    assert!(!backend.exists("child"));
}

#[test]
fn needs_agent_with_partial_companions_names_missing_ones() {
    let tmp = TempDir::new().unwrap();
    let backend = init_parent_backend(tmp.path(), "parent");

    let err = session::handle_start(
        &backend,
        "child",
        "parent",
        true,
        Some("scrutineer"),
        None,
        None,
        None,
    )
    .expect_err("must reject");
    let msg = err.to_string();
    // --role was provided, so it shouldn't be in the "missing" list.
    assert!(msg.contains("--template"), "must name --template: {}", msg);
    assert!(msg.contains("--inputs"), "must name --inputs: {}", msg);
    // Verifies we don't claim --role is missing.
    assert!(
        !msg.contains("missing: --role"),
        "must not name --role as missing: {}",
        msg
    );
}

#[test]
fn companion_without_needs_agent_rejects_naming_needs_agent() {
    let tmp = TempDir::new().unwrap();
    let backend = init_parent_backend(tmp.path(), "parent");

    // `--role` set, `--needs-agent` unset → reject naming `--needs-agent`.
    let err = session::handle_start(
        &backend,
        "child",
        "parent",
        false,
        Some("scrutineer"),
        None,
        None,
        None,
    )
    .expect_err("must reject");
    assert!(
        err.to_string().contains("--needs-agent"),
        "must name --needs-agent: {}",
        err
    );
}

#[test]
fn plain_start_without_companions_succeeds() {
    let tmp = TempDir::new().unwrap();
    let backend = init_parent_backend(tmp.path(), "parent");

    session::handle_start(&backend, "child", "parent", false, None, None, None, None)
        .expect("plain start must succeed");

    let header = backend.read_header("child").expect("read header");
    assert_eq!(header.needs_agent, None);
    assert_eq!(header.role, None);
    assert_eq!(header.inputs, None);
    assert_eq!(header.coordinator_of_record, None);
    assert_eq!(header.requested_by, None);
}

#[test]
fn malformed_inputs_json_rejects() {
    let tmp = TempDir::new().unwrap();
    let backend = init_parent_backend(tmp.path(), "parent");

    let err = session::handle_start(
        &backend,
        "child",
        "parent",
        true,
        Some("scrutineer"),
        Some("verdict"),
        Some("{not json}"),
        None,
    )
    .expect_err("must reject");
    assert!(err.to_string().contains("not valid JSON"), "got {}", err);
}

#[test]
fn oversized_inputs_payload_rejects() {
    let tmp = TempDir::new().unwrap();
    let backend = init_parent_backend(tmp.path(), "parent");

    // 1 MiB + safety pad of valid JSON.
    let mut big = String::with_capacity(1024 * 1024 + 32);
    big.push('"');
    big.push_str(&"a".repeat(1024 * 1024));
    big.push('"');

    let err = session::handle_start(
        &backend,
        "child",
        "parent",
        true,
        Some("scrutineer"),
        Some("verdict"),
        Some(&big),
        None,
    )
    .expect_err("must reject");
    assert!(err.to_string().contains("too large"), "got {}", err);
}

#[test]
fn overnested_inputs_json_rejects() {
    let tmp = TempDir::new().unwrap();
    let backend = init_parent_backend(tmp.path(), "parent");

    // 200-level-deep array.
    let depth = 200;
    let mut s = String::new();
    for _ in 0..depth {
        s.push('[');
    }
    s.push_str("0");
    for _ in 0..depth {
        s.push(']');
    }
    let err = session::handle_start(
        &backend,
        "child",
        "parent",
        true,
        Some("scrutineer"),
        Some("verdict"),
        Some(&s),
        None,
    )
    .expect_err("must reject");
    // Two rejection paths satisfy the AC: serde_json's built-in
    // recursion limit (currently 128) fires first on the parser
    // pass, so the message reads "recursion limit exceeded";
    // payloads that fit serde's limit but exceed our cap hit our
    // own "nests N levels deep" message. Either one means the
    // payload was rejected at parse time.
    let msg = err.to_string();
    assert!(
        msg.contains("nests") || msg.contains("recursion limit"),
        "expected depth rejection, got {}",
        msg
    );
}

#[test]
fn parent_injection_attempt_rejects_via_newtype() {
    let tmp = TempDir::new().unwrap();
    let backend = init_parent_backend(tmp.path(), "parent");

    let err = session::handle_start(
        &backend,
        "child",
        "../etc/passwd",
        false,
        None,
        None,
        None,
        None,
    )
    .expect_err("must reject");
    let msg = err.to_string();
    assert!(
        msg.contains("invalid session id") || msg.contains("leading dot"),
        "expected newtype rejection, got {}",
        msg
    );
}

#[test]
fn coordinator_shell_metacharacter_rejects_via_newtype() {
    let tmp = TempDir::new().unwrap();
    let backend = init_parent_backend(tmp.path(), "parent");

    let err = session::handle_start(
        &backend,
        "child",
        "parent",
        true,
        Some("scrutineer"),
        Some("verdict"),
        Some("{}"),
        Some("foo; rm -rf /"),
    )
    .expect_err("must reject");
    let msg = err.to_string();
    assert!(
        msg.contains("invalid coord id") || msg.contains("disallowed"),
        "expected coord-id rejection, got {}",
        msg
    );
}

#[test]
fn missing_parent_rejects() {
    let tmp = TempDir::new().unwrap();
    // Don't seed the parent — just point the backend at the empty tmp.
    let backend = LocalBackend::with_base_dir(tmp.path().to_path_buf());

    let err = session::handle_start(&backend, "child", "ghost", false, None, None, None, None)
        .expect_err("must reject");
    assert!(
        err.to_string().contains("not found"),
        "expected parent-not-found error, got {}",
        err
    );
}

#[test]
fn duplicate_session_name_rejects() {
    let tmp = TempDir::new().unwrap();
    let backend = init_parent_backend(tmp.path(), "parent");

    session::handle_start(&backend, "child", "parent", false, None, None, None, None)
        .expect("first start succeeds");

    let err = session::handle_start(&backend, "child", "parent", false, None, None, None, None)
        .expect_err("must reject duplicate");
    assert!(err.to_string().contains("already exists"), "got {}", err);
}
