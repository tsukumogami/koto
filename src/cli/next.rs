//! Pure dispatcher for `koto next`.
//!
//! Classifies the current workflow state into a `NextResponse` variant
//! or `NextError`. No I/O -- all inputs are pre-computed by the handler.

use std::collections::BTreeMap;

use crate::cli::next_types::{
    BlockingCondition, ExpectsSchema, IntegrationUnavailableMarker, NextError, NextResponse,
};
#[cfg(unix)]
use crate::gate::GateResult;
use crate::template::types::TemplateState;

/// Classify the current workflow state into a response or error.
///
/// Classification order:
/// 1. Terminal state -> `Terminal`
/// 2. Any gate failed/timed_out/errored -> `GateBlocked`
/// 3. Integration declared -> `IntegrationUnavailable` (runner deferred to #49)
/// 4. Accepts block exists -> `EvidenceRequired`
/// 5. Fallback: `EvidenceRequired` with empty expects (auto-advance candidate)
///
/// The `advanced` flag is set by the caller (true when an event was appended
/// before dispatching). This function never does I/O.
#[cfg(unix)]
pub fn dispatch_next(
    state: &str,
    template_state: &TemplateState,
    advanced: bool,
    gate_results: &BTreeMap<String, GateResult>,
) -> Result<NextResponse, NextError> {
    // 1. Terminal
    if template_state.terminal {
        return Ok(NextResponse::Terminal {
            state: state.to_string(),
            advanced,
        });
    }

    // 2. Gates failed
    let blocking: Vec<BlockingCondition> = gate_results
        .iter()
        .filter_map(|(name, result)| {
            let status = match result {
                GateResult::Passed => return None,
                GateResult::Failed { .. } => "failed",
                GateResult::TimedOut => "timed_out",
                GateResult::Error { .. } => "error",
            };
            Some(BlockingCondition {
                name: name.clone(),
                condition_type: "command".to_string(),
                status: status.to_string(),
                agent_actionable: false,
            })
        })
        .collect();

    if !blocking.is_empty() {
        // If the state has an accepts block, fall through to EvidenceRequired
        // instead of returning GateBlocked. The agent can provide override or
        // recovery evidence when gates fail on a state that accepts evidence.
        if template_state.accepts.is_none() {
            return Ok(NextResponse::GateBlocked {
                state: state.to_string(),
                directive: template_state.directive.clone(),
                advanced,
                blocking_conditions: blocking,
            });
        }
        // Fall through to step 5 (accepts block -> EvidenceRequired)
    }

    // Derive expects for use in integration and evidence branches.
    let expects = crate::cli::next_types::derive_expects(template_state);

    // 3. Integration declared -> unavailable (runner deferred to #49).
    // TODO(#49): Add availability check and `Integration` branch when the
    // integration runner is implemented.
    if let Some(integration_name) = &template_state.integration {
        return Ok(NextResponse::IntegrationUnavailable {
            state: state.to_string(),
            directive: template_state.directive.clone(),
            advanced,
            expects,
            integration: IntegrationUnavailableMarker {
                name: integration_name.clone(),
                available: false,
            },
        });
    }

    // 5. Accepts block -> EvidenceRequired
    if let Some(ref es) = expects {
        return Ok(NextResponse::EvidenceRequired {
            state: state.to_string(),
            directive: template_state.directive.clone(),
            advanced,
            expects: es.clone(),
        });
    }

    // 6. Fallback: state has no accepts, no integration, no gates blocking,
    // and is not terminal. This is an auto-advance candidate.
    // Return EvidenceRequired with an empty expects as a signal that the
    // state will auto-advance (the caller loop in #49 handles this).
    Ok(NextResponse::EvidenceRequired {
        state: state.to_string(),
        directive: template_state.directive.clone(),
        advanced,
        expects: ExpectsSchema {
            event_type: "evidence_submitted".to_string(),
            fields: BTreeMap::new(),
            options: vec![],
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::next_types::*;
    use crate::gate::GateResult;
    use crate::template::types::{FieldSchema, Gate, TemplateState, Transition};
    use std::collections::BTreeMap;

    fn make_state(
        directive: &str,
        terminal: bool,
        transitions: Vec<Transition>,
        gates: BTreeMap<String, Gate>,
        accepts: Option<BTreeMap<String, FieldSchema>>,
        integration: Option<String>,
    ) -> TemplateState {
        TemplateState {
            directive: directive.to_string(),
            transitions,
            terminal,
            gates,
            accepts,
            integration,
            default_action: None,
        }
    }

    // -------------------------------------------------------------------
    // Classification: Terminal
    // -------------------------------------------------------------------

    #[test]
    fn terminal_state_returns_terminal() {
        let ts = make_state("Done.", true, vec![], BTreeMap::new(), None, None);
        let result = dispatch_next("done", &ts, false, &BTreeMap::new());
        let resp = result.unwrap();
        assert_eq!(
            resp,
            NextResponse::Terminal {
                state: "done".to_string(),
                advanced: false,
            }
        );
    }

    #[test]
    fn terminal_state_with_advanced_true() {
        let ts = make_state("Done.", true, vec![], BTreeMap::new(), None, None);
        let result = dispatch_next("done", &ts, true, &BTreeMap::new());
        let resp = result.unwrap();
        match resp {
            NextResponse::Terminal { advanced, .. } => assert!(advanced),
            other => panic!("expected Terminal, got {:?}", other),
        }
    }

    // -------------------------------------------------------------------
    // Classification: GateBlocked
    // -------------------------------------------------------------------

    #[test]
    fn gate_blocked_when_gate_failed() {
        let ts = make_state("Deploy.", false, vec![], BTreeMap::new(), None, None);
        let mut gates = BTreeMap::new();
        gates.insert("ci".to_string(), GateResult::Failed { exit_code: 1 });

        let result = dispatch_next("deploy", &ts, false, &gates);
        let resp = result.unwrap();
        match resp {
            NextResponse::GateBlocked {
                state,
                blocking_conditions,
                ..
            } => {
                assert_eq!(state, "deploy");
                assert_eq!(blocking_conditions.len(), 1);
                assert_eq!(blocking_conditions[0].name, "ci");
                assert_eq!(blocking_conditions[0].status, "failed");
                assert_eq!(blocking_conditions[0].condition_type, "command");
                assert!(!blocking_conditions[0].agent_actionable);
            }
            other => panic!("expected GateBlocked, got {:?}", other),
        }
    }

    #[test]
    fn gate_blocked_when_gate_timed_out() {
        let ts = make_state("Deploy.", false, vec![], BTreeMap::new(), None, None);
        let mut gates = BTreeMap::new();
        gates.insert("slow_check".to_string(), GateResult::TimedOut);

        let result = dispatch_next("deploy", &ts, false, &gates);
        let resp = result.unwrap();
        match resp {
            NextResponse::GateBlocked {
                blocking_conditions,
                ..
            } => {
                assert_eq!(blocking_conditions[0].status, "timed_out");
            }
            other => panic!("expected GateBlocked, got {:?}", other),
        }
    }

    #[test]
    fn gate_blocked_when_gate_errored() {
        let ts = make_state("Deploy.", false, vec![], BTreeMap::new(), None, None);
        let mut gates = BTreeMap::new();
        gates.insert(
            "broken".to_string(),
            GateResult::Error {
                message: "spawn failed".to_string(),
            },
        );

        let result = dispatch_next("deploy", &ts, false, &gates);
        let resp = result.unwrap();
        match resp {
            NextResponse::GateBlocked {
                blocking_conditions,
                ..
            } => {
                assert_eq!(blocking_conditions[0].status, "error");
            }
            other => panic!("expected GateBlocked, got {:?}", other),
        }
    }

    #[test]
    fn gate_blocked_includes_all_failures() {
        let ts = make_state("Deploy.", false, vec![], BTreeMap::new(), None, None);
        let mut gates = BTreeMap::new();
        gates.insert("ci".to_string(), GateResult::Failed { exit_code: 1 });
        gates.insert("lint".to_string(), GateResult::TimedOut);
        gates.insert("ok".to_string(), GateResult::Passed);

        let result = dispatch_next("deploy", &ts, false, &gates);
        let resp = result.unwrap();
        match resp {
            NextResponse::GateBlocked {
                blocking_conditions,
                ..
            } => {
                // Only failing gates, not the passing one
                assert_eq!(blocking_conditions.len(), 2);
                let names: Vec<&str> = blocking_conditions
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect();
                assert!(names.contains(&"ci"));
                assert!(names.contains(&"lint"));
            }
            other => panic!("expected GateBlocked, got {:?}", other),
        }
    }

    #[test]
    fn passing_gates_do_not_block() {
        let ts = make_state("Deploy.", false, vec![], BTreeMap::new(), None, None);
        let mut gates = BTreeMap::new();
        gates.insert("ci".to_string(), GateResult::Passed);
        gates.insert("lint".to_string(), GateResult::Passed);

        let result = dispatch_next("deploy", &ts, false, &gates);
        let resp = result.unwrap();
        // Should not be GateBlocked since all gates passed
        match resp {
            NextResponse::GateBlocked { .. } => {
                panic!("should not be GateBlocked when all gates pass")
            }
            _ => {} // any other variant is fine
        }
    }

    // -------------------------------------------------------------------
    // Classification: Terminal takes priority over gates
    // -------------------------------------------------------------------

    #[test]
    fn terminal_takes_priority_over_failed_gates() {
        let ts = make_state("Done.", true, vec![], BTreeMap::new(), None, None);
        let mut gates = BTreeMap::new();
        gates.insert("ci".to_string(), GateResult::Failed { exit_code: 1 });

        let result = dispatch_next("done", &ts, false, &gates);
        let resp = result.unwrap();
        match resp {
            NextResponse::Terminal { .. } => {}
            other => panic!("expected Terminal (priority over gates), got {:?}", other),
        }
    }

    // -------------------------------------------------------------------
    // Classification: IntegrationUnavailable
    // -------------------------------------------------------------------

    #[test]
    fn integration_unavailable_when_declared() {
        let ts = make_state(
            "Delegate review.",
            false,
            vec![],
            BTreeMap::new(),
            None,
            Some("code_review".to_string()),
        );

        let result = dispatch_next("delegate", &ts, false, &BTreeMap::new());
        let resp = result.unwrap();
        match resp {
            NextResponse::IntegrationUnavailable {
                state,
                integration,
                expects,
                ..
            } => {
                assert_eq!(state, "delegate");
                assert_eq!(integration.name, "code_review");
                assert!(!integration.available);
                assert!(expects.is_none());
            }
            other => panic!("expected IntegrationUnavailable, got {:?}", other),
        }
    }

    #[test]
    fn integration_unavailable_with_expects() {
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "approval".to_string(),
            FieldSchema {
                field_type: "boolean".to_string(),
                required: true,
                values: vec![],
                description: String::new(),
            },
        );

        let ts = make_state(
            "Delegate review.",
            false,
            vec![],
            BTreeMap::new(),
            Some(accepts),
            Some("code_review".to_string()),
        );

        let result = dispatch_next("delegate", &ts, false, &BTreeMap::new());
        let resp = result.unwrap();
        match resp {
            NextResponse::IntegrationUnavailable { expects, .. } => {
                assert!(expects.is_some());
                let es = expects.unwrap();
                assert!(es.fields.contains_key("approval"));
            }
            other => panic!("expected IntegrationUnavailable, got {:?}", other),
        }
    }

    #[test]
    fn integration_blocked_by_gates() {
        let ts = make_state(
            "Delegate.",
            false,
            vec![],
            BTreeMap::new(),
            None,
            Some("review".to_string()),
        );
        let mut gates = BTreeMap::new();
        gates.insert("ci".to_string(), GateResult::Failed { exit_code: 1 });

        let result = dispatch_next("delegate", &ts, false, &gates);
        let resp = result.unwrap();
        match resp {
            NextResponse::GateBlocked { .. } => {}
            other => panic!(
                "expected GateBlocked (priority over integration), got {:?}",
                other
            ),
        }
    }

    // -------------------------------------------------------------------
    // Classification: EvidenceRequired
    // -------------------------------------------------------------------

    #[test]
    fn evidence_required_with_accepts() {
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "decision".to_string(),
            FieldSchema {
                field_type: "enum".to_string(),
                required: true,
                values: vec!["proceed".to_string(), "escalate".to_string()],
                description: String::new(),
            },
        );

        let mut when = BTreeMap::new();
        when.insert("decision".to_string(), serde_json::json!("proceed"));

        let ts = make_state(
            "Review the changes.",
            false,
            vec![Transition {
                target: "implement".to_string(),
                when: Some(when),
            }],
            BTreeMap::new(),
            Some(accepts),
            None,
        );

        let result = dispatch_next("review", &ts, false, &BTreeMap::new());
        let resp = result.unwrap();
        match resp {
            NextResponse::EvidenceRequired {
                state,
                directive,
                expects,
                ..
            } => {
                assert_eq!(state, "review");
                assert_eq!(directive, "Review the changes.");
                assert_eq!(expects.event_type, "evidence_submitted");
                assert!(expects.fields.contains_key("decision"));
                assert_eq!(expects.options.len(), 1);
            }
            other => panic!("expected EvidenceRequired, got {:?}", other),
        }
    }

    #[test]
    fn evidence_required_advanced_flag_propagates() {
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "data".to_string(),
            FieldSchema {
                field_type: "string".to_string(),
                required: true,
                values: vec![],
                description: String::new(),
            },
        );

        let ts = make_state(
            "Collect data.",
            false,
            vec![],
            BTreeMap::new(),
            Some(accepts),
            None,
        );

        let result = dispatch_next("gather", &ts, true, &BTreeMap::new());
        let resp = result.unwrap();
        match resp {
            NextResponse::EvidenceRequired { advanced, .. } => assert!(advanced),
            other => panic!("expected EvidenceRequired, got {:?}", other),
        }
    }

    // -------------------------------------------------------------------
    // Classification: Fallback (no accepts, no integration, not terminal)
    // -------------------------------------------------------------------

    #[test]
    fn fallback_returns_evidence_required_with_empty_expects() {
        let ts = make_state(
            "Auto-advance step.",
            false,
            vec![Transition {
                target: "next_step".to_string(),
                when: None,
            }],
            BTreeMap::new(),
            None,
            None,
        );

        let result = dispatch_next("step", &ts, false, &BTreeMap::new());
        let resp = result.unwrap();
        match resp {
            NextResponse::EvidenceRequired { expects, .. } => {
                assert!(expects.fields.is_empty());
                assert!(expects.options.is_empty());
            }
            other => panic!("expected EvidenceRequired fallback, got {:?}", other),
        }
    }

    // -------------------------------------------------------------------
    // Classification priority: gates > integration
    // -------------------------------------------------------------------

    #[test]
    fn gates_take_priority_over_integration() {
        let ts = make_state(
            "Delegate.",
            false,
            vec![],
            BTreeMap::new(),
            None,
            Some("tool".to_string()),
        );
        let mut gates = BTreeMap::new();
        gates.insert("check".to_string(), GateResult::Failed { exit_code: 1 });

        let result = dispatch_next("step", &ts, false, &gates);
        match result.unwrap() {
            NextResponse::GateBlocked { .. } => {}
            other => panic!("expected GateBlocked, got {:?}", other),
        }
    }

    // -------------------------------------------------------------------
    // Classification priority: integration > accepts
    // -------------------------------------------------------------------

    // -------------------------------------------------------------------
    // Gate-with-evidence-fallback: gates fail + accepts -> EvidenceRequired
    // -------------------------------------------------------------------

    #[test]
    fn gate_failed_with_accepts_returns_evidence_required() {
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "status".to_string(),
            FieldSchema {
                field_type: "enum".to_string(),
                required: true,
                values: vec![
                    "completed".to_string(),
                    "override".to_string(),
                    "blocked".to_string(),
                ],
                description: String::new(),
            },
        );

        let ts = make_state(
            "Setup branch.",
            false,
            vec![Transition {
                target: "analysis".to_string(),
                when: Some({
                    let mut w = BTreeMap::new();
                    w.insert("status".to_string(), serde_json::json!("completed"));
                    w
                }),
            }],
            BTreeMap::new(),
            Some(accepts),
            None,
        );

        let mut gates = BTreeMap::new();
        gates.insert(
            "branch_check".to_string(),
            GateResult::Failed { exit_code: 1 },
        );

        let result = dispatch_next("setup", &ts, false, &gates);
        let resp = result.unwrap();
        match resp {
            NextResponse::EvidenceRequired { state, expects, .. } => {
                assert_eq!(state, "setup");
                assert!(expects.fields.contains_key("status"));
            }
            other => panic!(
                "expected EvidenceRequired (gate-with-evidence-fallback), got {:?}",
                other
            ),
        }
    }

    #[test]
    fn gate_failed_without_accepts_still_returns_gate_blocked() {
        let ts = make_state("Deploy.", false, vec![], BTreeMap::new(), None, None);
        let mut gates = BTreeMap::new();
        gates.insert("ci".to_string(), GateResult::Failed { exit_code: 1 });

        let result = dispatch_next("deploy", &ts, false, &gates);
        let resp = result.unwrap();
        match resp {
            NextResponse::GateBlocked { .. } => {}
            other => panic!("expected GateBlocked (no accepts), got {:?}", other),
        }
    }

    #[test]
    fn gate_passed_with_accepts_returns_evidence_required() {
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "status".to_string(),
            FieldSchema {
                field_type: "string".to_string(),
                required: true,
                values: vec![],
                description: String::new(),
            },
        );

        let ts = make_state(
            "Check.",
            false,
            vec![],
            BTreeMap::new(),
            Some(accepts),
            None,
        );

        let mut gates = BTreeMap::new();
        gates.insert("check".to_string(), GateResult::Passed);

        let result = dispatch_next("check_state", &ts, false, &gates);
        let resp = result.unwrap();
        match resp {
            NextResponse::EvidenceRequired { .. } => {}
            other => panic!(
                "expected EvidenceRequired (gates passed, has accepts), got {:?}",
                other
            ),
        }
    }

    #[test]
    fn mixed_gates_with_accepts_returns_evidence_required() {
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "status".to_string(),
            FieldSchema {
                field_type: "string".to_string(),
                required: true,
                values: vec![],
                description: String::new(),
            },
        );

        let ts = make_state(
            "Setup.",
            false,
            vec![],
            BTreeMap::new(),
            Some(accepts),
            None,
        );

        let mut gates = BTreeMap::new();
        gates.insert("check_a".to_string(), GateResult::Passed);
        gates.insert("check_b".to_string(), GateResult::Failed { exit_code: 1 });

        let result = dispatch_next("setup", &ts, false, &gates);
        let resp = result.unwrap();
        match resp {
            NextResponse::EvidenceRequired { .. } => {}
            other => panic!(
                "expected EvidenceRequired (mixed gates with accepts), got {:?}",
                other
            ),
        }
    }

    // -------------------------------------------------------------------
    // Classification priority: integration > accepts
    // -------------------------------------------------------------------

    #[test]
    fn integration_takes_priority_over_accepts() {
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "data".to_string(),
            FieldSchema {
                field_type: "string".to_string(),
                required: true,
                values: vec![],
                description: String::new(),
            },
        );

        let ts = make_state(
            "Delegate with fallback.",
            false,
            vec![],
            BTreeMap::new(),
            Some(accepts),
            Some("tool".to_string()),
        );

        let result = dispatch_next("step", &ts, false, &BTreeMap::new());
        match result.unwrap() {
            NextResponse::IntegrationUnavailable { expects, .. } => {
                // Expects should be populated even though integration is unavailable
                assert!(expects.is_some());
            }
            other => panic!("expected IntegrationUnavailable, got {:?}", other),
        }
    }
}
