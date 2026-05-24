//! Round-trip + pre-request-store back-compat coverage for the
//! request-store fields added to [`StateFileHeader`] in Issue 1.
//!
//! Anchors Decision 1 (wire vocabulary) and Decision 5 (bunki
//! compatibility): the new fields must serialize via additive,
//! `skip_serializing_if`-guarded keys, and pre-request-store fixture
//! headers (the JSON form that bunki BK2 already writes) must
//! deserialize cleanly with all new fields defaulted to `None` / `0`.

use koto::engine::types::{AssignmentClaim, StateFileHeader};

fn full_header() -> StateFileHeader {
    StateFileHeader {
        schema_version: 1,
        workflow: "wf".to_string(),
        template_hash: "deadbeef".to_string(),
        created_at: "2026-05-24T00:00:00Z".to_string(),
        parent_workflow: Some("parent".to_string()),
        template_source_dir: None,
        session_id: "sess-1".to_string(),
        intent: Some("ship request-store".to_string()),
        template_name: Some("agent.md".to_string()),
        needs_agent: Some(true),
        role: Some("implementer".to_string()),
        inputs: Some(serde_json::json!({"issue": 1, "wave": 0})),
        coordinator_of_record: Some("coord-7".to_string()),
        requested_by: Some("work-coord".to_string()),
        assignment_claim: Some(AssignmentClaim {
            coord_id: "coord-7".to_string(),
            claimed_at: "2026-05-24T00:00:01Z".to_string(),
        }),
        dispatch_epoch: 3,
        priority: Some(7),
        deadline: Some("2026-05-24T01:00:00Z".to_string()),
        retry_count: Some(2),
        agent_config: Some(serde_json::json!({"timeout": "1h"})),
        respawn_generation: None,
    }
}

#[test]
fn full_header_round_trips() {
    let header = full_header();
    let json = serde_json::to_string(&header).expect("serialize");
    let parsed: StateFileHeader = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(header, parsed);

    // Re-serializing must produce identical JSON.
    let json2 = serde_json::to_string(&parsed).expect("reserialize");
    assert_eq!(
        json, json2,
        "round-tripped serialization must be byte-identical"
    );

    // Sanity-check that the new request-store keys are actually present on the wire.
    assert!(json.contains("\"needs_agent\":true"));
    assert!(json.contains("\"role\":\"implementer\""));
    assert!(json.contains("\"coordinator_of_record\":\"coord-7\""));
    assert!(json.contains("\"requested_by\":\"work-coord\""));
    assert!(json.contains("\"assignment_claim\""));
    assert!(json.contains("\"claimed_at\":\"2026-05-24T00:00:01Z\""));
    assert!(json.contains("\"dispatch_epoch\":3"));
    assert!(json.contains("\"priority\":7"));
    assert!(json.contains("\"deadline\":\"2026-05-24T01:00:00Z\""));
    assert!(json.contains("\"retry_count\":2"));
    assert!(json.contains("\"agent_config\""));
}

#[test]
fn pre_request_store_fixture_header_deserializes_with_defaults() {
    // Frozen pre-request-store fixture: exactly the keys bunki BK2
    // writes today. None of the seven additive or four reserved
    // request-store keys may be required for deserialization, and
    // `dispatch_epoch` must default to 0 when absent.
    let pre_request_store_json = r#"{
        "schema_version": 1,
        "workflow": "old-wf",
        "template_hash": "abc",
        "created_at": "2026-01-01T00:00:00Z"
    }"#;
    let parsed: StateFileHeader = serde_json::from_str(pre_request_store_json)
        .expect("pre-request-store fixture must deserialize");
    assert_eq!(parsed.needs_agent, None);
    assert_eq!(parsed.role, None);
    assert_eq!(parsed.inputs, None);
    assert_eq!(parsed.coordinator_of_record, None);
    assert_eq!(parsed.requested_by, None);
    assert_eq!(parsed.assignment_claim, None);
    assert_eq!(parsed.dispatch_epoch, 0);
    assert_eq!(parsed.priority, None);
    assert_eq!(parsed.deadline, None);
    assert_eq!(parsed.retry_count, None);
    assert_eq!(parsed.agent_config, None);
}

#[test]
fn needs_agent_alone_deserializes() {
    // CLI-layer companion validation is owned by Issue 4; the type
    // layer must accept `needs_agent = true` even without role /
    // inputs / coordinator_of_record / requested_by.
    let json = r#"{
        "schema_version": 1,
        "workflow": "wf",
        "template_hash": "h",
        "created_at": "2026-05-24T00:00:00Z",
        "needs_agent": true
    }"#;
    let parsed: StateFileHeader = serde_json::from_str(json).expect("must deserialize");
    assert_eq!(parsed.needs_agent, Some(true));
    assert_eq!(parsed.role, None);
    assert_eq!(parsed.inputs, None);
    assert_eq!(parsed.coordinator_of_record, None);
    assert_eq!(parsed.requested_by, None);
}

#[test]
fn none_valued_request_store_fields_produce_no_keys_on_the_wire() {
    // Bunki compatibility contract (Decision 5): a header whose
    // request-store fields are all `None` must serialize without
    // introducing any of the new keys, so pre-request-store readers
    // see byte-identical output.
    let header = StateFileHeader {
        schema_version: 1,
        workflow: "wf".to_string(),
        template_hash: "h".to_string(),
        created_at: "2026-05-24T00:00:00Z".to_string(),
        parent_workflow: None,
        template_source_dir: None,
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
        priority: None,
        deadline: None,
        retry_count: None,
        agent_config: None,
        respawn_generation: None,
    };
    let json = serde_json::to_string(&header).expect("serialize");
    for key in [
        "needs_agent",
        "role",
        "inputs",
        "coordinator_of_record",
        "requested_by",
        "assignment_claim",
        "priority",
        "deadline",
        "retry_count",
        "agent_config",
    ] {
        assert!(
            !json.contains(key),
            "key `{}` must be omitted when None, got {}",
            key,
            json
        );
    }
    // `dispatch_epoch` is a plain u32 with `#[serde(default)]`, so it
    // is always serialized (no `skip_serializing_if`). Confirm that
    // contract here so pre-request-store readers can decide how to treat it.
    assert!(
        json.contains("\"dispatch_epoch\":0"),
        "dispatch_epoch must always serialize, got {}",
        json
    );
}

#[test]
fn dispatch_epoch_defaults_to_zero_when_absent() {
    let json = r#"{
        "schema_version": 1,
        "workflow": "wf",
        "template_hash": "h",
        "created_at": "2026-05-24T00:00:00Z"
    }"#;
    let parsed: StateFileHeader = serde_json::from_str(json).expect("must deserialize");
    assert_eq!(parsed.dispatch_epoch, 0);
}

#[test]
fn assignment_claim_round_trips() {
    let claim = AssignmentClaim {
        coord_id: "coord-7".to_string(),
        claimed_at: "2026-05-24T00:00:01Z".to_string(),
    };
    let json = serde_json::to_string(&claim).expect("serialize");
    assert!(json.contains("\"coord_id\":\"coord-7\""));
    assert!(json.contains("\"claimed_at\":\"2026-05-24T00:00:01Z\""));
    let parsed: AssignmentClaim = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(claim, parsed);
}
