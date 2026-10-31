//! koto's own shape check for the enriched `/workflows` file contract.
//!
//! The committed fixture at `tests/fixtures/native-workflows/enriched-shape.json`
//! pins the enriched `koto-<uuid>.json` shape (ordered `phases`, the
//! `workflowProgress` tree, and the blocked reading: top-level `running`,
//! `koto.status` `blocked` and a Blocked `error` step) that koto emits. This test
//! checks that koto writes the shape it means to: whenever the emitted shape
//! changes, this fixture must change with it, deliberately. Both sides of the
//! comparison are koto's, so it can't see Claude Code change. That is the
//! convention guard's job (`scripts/check-claude-code-conventions.sh`), which
//! reads a Claude Code release, and `tests/native_workflows_conventions.rs`
//! keeps every field of this fixture covered by one of the guard's probes.

#[path = "support/host_session.rs"]
mod host_session;

use koto::workflows_surface::{
    HoldingKey, Holdings, Phase, ProgressNode, RenderStatus, WorkflowFile, CONTRACT_VERSION,
};

const FIXTURE: &str = "tests/fixtures/native-workflows/enriched-shape.json";

/// A canonical enriched file: a four-phase session with two completed phases
/// (one carrying evidence, one bare), an active phase blocked on a failed gate,
/// and one upcoming phase. Built through the public contract API only.
fn canonical_enriched_file() -> WorkflowFile {
    let phases = vec![
        Phase {
            title: "Gather context".to_string(),
            detail: "evidence: files".to_string(),
        },
        Phase {
            title: "Implement".to_string(),
            detail: "done".to_string(),
        },
        Phase {
            title: "Verify".to_string(),
            detail: "gate tests-pass: FAIL".to_string(),
        },
        Phase {
            title: "Review".to_string(),
            detail: String::new(),
        },
    ];
    let workflow_progress = vec![
        ProgressNode::WorkflowPhase {
            index: 1,
            title: "Gather context".to_string(),
        },
        ProgressNode::WorkflowAgent {
            index: 1,
            label: "Gather context".to_string(),
            phase_index: 1,
            phase_title: "Gather context".to_string(),
            state: "done".to_string(),
            prompt_preview: "Read the issue and map the module.".to_string(),
            result_preview: "evidence: files".to_string(),
            blocked: false,
            error: String::new(),
        },
        ProgressNode::WorkflowPhase {
            index: 2,
            title: "Implement".to_string(),
        },
        ProgressNode::WorkflowAgent {
            index: 2,
            label: "Implement".to_string(),
            phase_index: 2,
            phase_title: "Implement".to_string(),
            state: "done".to_string(),
            prompt_preview: "Implement the change per the gathered context.".to_string(),
            result_preview: String::new(),
            blocked: false,
            error: String::new(),
        },
        ProgressNode::WorkflowPhase {
            index: 3,
            title: "Verify".to_string(),
        },
        ProgressNode::WorkflowAgent {
            index: 3,
            label: "Verify".to_string(),
            phase_index: 3,
            phase_title: "Verify".to_string(),
            state: "error".to_string(),
            prompt_preview: "Run the test suite and submit the result.".to_string(),
            result_preview: "gate tests-pass: FAIL".to_string(),
            blocked: true,
            error: "gate tests-pass: FAIL".to_string(),
        },
        ProgressNode::WorkflowPhase {
            index: 4,
            title: "Review".to_string(),
        },
    ];
    WorkflowFile::new(
        "11111111-2222-4333-8444-555555555555",
        "impl-feature",
        "impl-feature \u{b7} verify".to_string(),
        Some("verify".to_string()),
        RenderStatus::Blocked,
        1_700_000_000_000,
    )
    .with_detail(phases, workflow_progress)
    .with_blocked_gate("tests-pass")
    .with_holdings(Holdings {
        key_count: 3,
        total_bytes: 5_160,
        keys: vec![
            HoldingKey {
                key: "notes.md".to_string(),
                size: 64,
            },
            HoldingKey {
                key: "research/long.md".to_string(),
                size: 1_000,
            },
            HoldingKey {
                key: "data/blob.bin".to_string(),
                size: 4_096,
            },
        ],
        keys_truncated: false,
        anchor: Some("/srv/checkout".to_string()),
        store: Some("local".to_string()),
    })
}

/// The emitted enriched shape must match the committed golden fixture. A
/// koto-side change (a renamed field, a dropped node type, a changed status
/// vocabulary) fails here, so the readers of earlier versions are not silently
/// broken and the convention guard's coverage test reads the shape koto
/// actually writes.
#[test]
fn enriched_shape_matches_golden_fixture() {
    let bytes = canonical_enriched_file().to_json_bytes().unwrap();
    let actual: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    let fixture = std::fs::read_to_string(FIXTURE)
        .unwrap_or_else(|e| panic!("read golden fixture {FIXTURE}: {e}"));
    let expected: serde_json::Value = serde_json::from_str(&fixture).unwrap();

    assert_eq!(
        actual, expected,
        "the emitted enriched /workflows shape drifted from the golden fixture \
         ({FIXTURE}). If the contract change is intentional, regenerate it with \
         `cargo test --test native_workflows_shape -- --ignored --nocapture`, \
         commit the printed JSON, and run \
         `cargo test --test native_workflows_conventions` to see whether a new \
         field needs a probe in the Claude Code convention guard."
    );
}

/// The fixture pins contract version 5 and the additive fields, holdings,
/// the /workflows title fields and the blocked step included, so a future
/// reader can assert the shape it targets.
#[test]
fn fixture_pins_contract_v5_and_new_fields() {
    let fixture = std::fs::read_to_string(FIXTURE).unwrap();
    let v: serde_json::Value = serde_json::from_str(&fixture).unwrap();
    assert_eq!(v["koto"]["contractVersion"], CONTRACT_VERSION);
    assert_eq!(v["koto"]["contractVersion"], 5);
    // v5: the top-level status is the screen's word, and a blocked session
    // is running; koto's classification is koto.status.
    assert_eq!(v["status"], "running");
    assert_eq!(v["koto"]["status"], "blocked");
    assert_eq!(
        v["summary"],
        "koto \u{b7} impl-feature \u{b7} verify \u{b7} blocked on gate tests-pass"
    );
    let blocked: Vec<&serde_json::Value> = v["workflowProgress"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["type"] == "workflow_agent" && n["state"] == "error")
        .collect();
    assert_eq!(blocked.len(), 1, "one blocked step");
    assert_eq!(blocked[0]["blocked"], true);
    assert_eq!(blocked[0]["error"], "gate tests-pass: FAIL");
    // v4: the /workflows screen titles an entry from workflowName.
    assert!(v["workflowName"].is_string());
    assert!(v["summary"].is_string());
    // v3: the holdings summary carries names and sizes, never content.
    assert!(v["koto"]["holdings"]["keyCount"].is_u64());
    assert!(v["koto"]["holdings"]["totalBytes"].is_u64());
    assert!(v["koto"]["holdings"]["keys"].is_array());
    assert!(v["koto"]["holdings"]["keysTruncated"].is_boolean());
    assert!(v["phases"].is_array());
    assert!(v["workflowProgress"].is_array());
    // The initial top-level fields survive unchanged.
    assert!(v["id"].is_string());
    assert!(v["name"].is_string());
    assert!(v["startTime"].is_u64());
}

/// Regeneration helper (ignored by default): prints the canonical enriched JSON
/// so the golden fixture can be refreshed after a deliberate contract change.
/// Run with `cargo test --test native_workflows_shape -- --ignored --nocapture`.
#[test]
#[ignore = "regeneration helper; prints the golden fixture JSON"]
fn print_canonical_enriched_file() {
    let bytes = canonical_enriched_file().to_json_bytes().unwrap();
    print!("{}", String::from_utf8(bytes).unwrap());
}
