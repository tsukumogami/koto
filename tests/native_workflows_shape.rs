//! Golden-shape guard for the enriched `/workflows` file contract.
//!
//! The committed fixture at `tests/fixtures/native-workflows/enriched-shape.json`
//! pins the enriched `koto-<uuid>.json` shape (ordered `phases`, the
//! `workflowProgress` tree, and the `blocked` status) that koto emits. The future
//! drift-guard adopts this fixture as the anchor for its version/fixture guard over the
//! undocumented Claude Code surface; this test is the discharge of the
//! shape-change / drift-guard obligation (whenever the emitted shape changes,
//! this fixture must change with it, deliberately).

use koto::workflows_surface::{Phase, ProgressNode, RenderStatus, WorkflowFile, CONTRACT_VERSION};

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
            state: "progress".to_string(),
            prompt_preview: "Run the test suite and submit the result.".to_string(),
            result_preview: "gate tests-pass: FAIL".to_string(),
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
}

/// The emitted enriched shape must match the committed golden fixture. Drift
/// (a renamed field, a dropped node type, a changed status vocabulary) fails
/// here so the future drift-guard has a stable anchor and the initial readers are not
/// silently broken.
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
         ({FIXTURE}). This fixture is the drift-guard anchor; if the contract \
         change is intentional, regenerate it with \
         `cargo test --test native_workflows_shape -- --ignored --nocapture` and \
         commit the printed JSON."
    );
}

/// The fixture pins contract version 2 and the enriched additive fields, so a
/// future reader can assert the shape it targets.
#[test]
fn fixture_pins_contract_v2_and_new_fields() {
    let fixture = std::fs::read_to_string(FIXTURE).unwrap();
    let v: serde_json::Value = serde_json::from_str(&fixture).unwrap();
    assert_eq!(v["koto"]["contractVersion"], CONTRACT_VERSION);
    assert_eq!(v["koto"]["contractVersion"], 2);
    assert_eq!(v["status"], "blocked");
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
