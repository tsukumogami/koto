// Auto-advancement engine: transition resolution and advancement loop.
//
// Implemented for Issue #49.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::engine::types::{Event, EventPayload};
use crate::gate::GateResult;
use crate::template::types::{CompiledTemplate, TemplateState};

/// Maximum number of transitions per invocation. Defense-in-depth against
/// template bugs with hundreds of linearly chaining states.
const MAX_CHAIN_LENGTH: usize = 100;

/// Result of resolving which transition to take from a state.
#[derive(Debug, Clone, PartialEq)]
pub enum TransitionResolution {
    /// Exactly one transition matched; advance to the target state.
    Resolved(String),
    /// Conditional transitions exist but none matched the current evidence.
    NeedsEvidence,
    /// Multiple conditional transitions matched (template bug at runtime).
    Ambiguous(Vec<String>),
    /// The state has no transitions at all (dead-end, not terminal).
    NoTransitions,
}

/// Why the advancement loop stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum StopReason {
    /// Reached a terminal state.
    Terminal,
    /// One or more gates failed.
    GateBlocked(BTreeMap<String, GateResult>),
    /// Conditional transitions exist but evidence doesn't match any.
    EvidenceRequired,
    /// Integration was invoked and returned output.
    Integration {
        name: String,
        output: serde_json::Value,
    },
    /// Integration is declared but no runner is configured.
    IntegrationUnavailable { name: String },
    /// The loop visited the same state twice (cycle in template).
    CycleDetected { state: String },
    /// Safety limit: exceeded 100 transitions in one invocation.
    ChainLimitReached,
    /// SIGTERM or SIGINT received between iterations.
    SignalReceived,
}

/// Result returned by `advance_until_stop`.
#[derive(Debug, Clone, PartialEq)]
pub struct AdvanceResult {
    /// The state the engine stopped in.
    pub final_state: String,
    /// True if at least one transition was made.
    pub advanced: bool,
    /// Why the loop stopped.
    pub stop_reason: StopReason,
}

/// Errors that can occur during advancement (not stop reasons).
#[derive(Debug)]
pub enum AdvanceError {
    /// Multiple conditional transitions matched the same evidence.
    AmbiguousTransition { state: String, targets: Vec<String> },
    /// A state with no transitions and not marked terminal.
    DeadEndState { state: String },
    /// The state doesn't exist in the template.
    UnknownState { state: String },
    /// Failed to persist an event.
    PersistenceError(String),
}

impl std::fmt::Display for AdvanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdvanceError::AmbiguousTransition { state, targets } => {
                write!(
                    f,
                    "ambiguous transition from state '{}': multiple matches {:?}",
                    state, targets
                )
            }
            AdvanceError::DeadEndState { state } => {
                write!(
                    f,
                    "state '{}' has no transitions and is not terminal",
                    state
                )
            }
            AdvanceError::UnknownState { state } => {
                write!(f, "state '{}' not found in template", state)
            }
            AdvanceError::PersistenceError(msg) => {
                write!(f, "failed to persist event: {}", msg)
            }
        }
    }
}

impl std::error::Error for AdvanceError {}

/// Error returned by the integration runner closure.
#[derive(Debug)]
pub enum IntegrationError {
    /// The integration is not configured or no runner is available.
    Unavailable,
    /// The integration runner failed.
    Failed(String),
}

/// Advance through workflow states until a stopping condition is reached.
///
/// The loop iterates states, checking each against stopping conditions in order:
/// 1. Signal received (shutdown flag)
/// 2. Cycle detected (visited-state set)
/// 3. Terminal state
/// 4. Integration already invoked this epoch (re-invocation prevention)
/// 5. Integration declared (invoke runner)
/// 6. Gates (evaluate all, stop if any fail)
/// 7. Transition resolution (match evidence against conditions)
///
/// I/O operations are injected as closures for testability:
/// - `append_event`: persist a state transition event
/// - `evaluate_gates`: run gate commands and return results
/// - `invoke_integration`: call an integration runner
pub fn advance_until_stop<F, G, I>(
    current_state: &str,
    template: &CompiledTemplate,
    evidence: &BTreeMap<String, serde_json::Value>,
    append_event: &mut F,
    evaluate_gates: &G,
    invoke_integration: &I,
    shutdown: &AtomicBool,
) -> Result<AdvanceResult, AdvanceError>
where
    F: FnMut(&EventPayload) -> Result<(), String>,
    G: Fn(&BTreeMap<String, crate::template::types::Gate>) -> BTreeMap<String, GateResult>,
    I: Fn(&str) -> Result<serde_json::Value, IntegrationError>,
{
    let mut visited = HashSet::new();
    let mut state = current_state.to_string();
    let mut advanced = false;
    let mut transition_count: usize = 0;
    // Evidence is only used for the initial state; auto-advanced states start fresh.
    let mut current_evidence = evidence.clone();

    // The starting state is NOT added to visited. The visited set tracks states
    // we've auto-advanced THROUGH during this invocation. The starting state was
    // already arrived at before this invocation, so re-visiting it (e.g., in a
    // review -> implement -> review loop) is legitimate.

    loop {
        // 1. Check shutdown flag
        if shutdown.load(Ordering::Relaxed) {
            return Ok(AdvanceResult {
                final_state: state,
                advanced,
                stop_reason: StopReason::SignalReceived,
            });
        }

        // 2. Chain limit check
        if transition_count >= MAX_CHAIN_LENGTH {
            return Ok(AdvanceResult {
                final_state: state,
                advanced,
                stop_reason: StopReason::ChainLimitReached,
            });
        }

        // Look up the current state in the template
        let template_state =
            template
                .states
                .get(&state)
                .ok_or_else(|| AdvanceError::UnknownState {
                    state: state.clone(),
                })?;

        // 3. Terminal state
        if template_state.terminal {
            return Ok(AdvanceResult {
                final_state: state,
                advanced,
                stop_reason: StopReason::Terminal,
            });
        }

        // 4. Integration check
        if let Some(integration_name) = &template_state.integration {
            match invoke_integration(integration_name) {
                Ok(output) => {
                    return Ok(AdvanceResult {
                        final_state: state,
                        advanced,
                        stop_reason: StopReason::Integration {
                            name: integration_name.clone(),
                            output,
                        },
                    });
                }
                Err(IntegrationError::Unavailable) => {
                    return Ok(AdvanceResult {
                        final_state: state,
                        advanced,
                        stop_reason: StopReason::IntegrationUnavailable {
                            name: integration_name.clone(),
                        },
                    });
                }
                Err(IntegrationError::Failed(msg)) => {
                    return Ok(AdvanceResult {
                        final_state: state,
                        advanced,
                        stop_reason: StopReason::IntegrationUnavailable {
                            name: format!("{}: {}", integration_name, msg),
                        },
                    });
                }
            }
        }

        // 5. Evaluate gates
        if !template_state.gates.is_empty() {
            let gate_results = evaluate_gates(&template_state.gates);
            let any_failed = gate_results
                .values()
                .any(|r| !matches!(r, GateResult::Passed));
            if any_failed {
                return Ok(AdvanceResult {
                    final_state: state,
                    advanced,
                    stop_reason: StopReason::GateBlocked(gate_results),
                });
            }
        }

        // 6. Resolve transition
        match resolve_transition(template_state, &current_evidence) {
            TransitionResolution::Resolved(target) => {
                // Check for cycle before transitioning
                if visited.contains(&target) {
                    return Ok(AdvanceResult {
                        final_state: state,
                        advanced,
                        stop_reason: StopReason::CycleDetected { state: target },
                    });
                }

                // Append transitioned event
                let payload = EventPayload::Transitioned {
                    from: Some(state.clone()),
                    to: target.clone(),
                    condition_type: "auto".to_string(),
                };
                append_event(&payload).map_err(AdvanceError::PersistenceError)?;

                visited.insert(target.clone());
                state = target;
                advanced = true;
                transition_count += 1;
                // Fresh epoch: auto-advanced states have no evidence
                current_evidence = BTreeMap::new();
            }
            TransitionResolution::NeedsEvidence => {
                return Ok(AdvanceResult {
                    final_state: state,
                    advanced,
                    stop_reason: StopReason::EvidenceRequired,
                });
            }
            TransitionResolution::Ambiguous(targets) => {
                return Err(AdvanceError::AmbiguousTransition {
                    state: state.clone(),
                    targets,
                });
            }
            TransitionResolution::NoTransitions => {
                return Err(AdvanceError::DeadEndState {
                    state: state.clone(),
                });
            }
        }
    }
}

/// Resolve which transition to take from a state given current evidence.
///
/// Resolution algorithm:
/// 1. Collect conditional transitions (those with `when: Some(...)`)
/// 2. For each, check if ALL `when` fields match the evidence (exact JSON equality)
/// 3. If exactly one matches, return `Resolved(target)`
/// 4. If multiple match, return `Ambiguous(targets)`
/// 5. If none match and an unconditional transition exists, return `Resolved(fallback)`
/// 6. If none match and no unconditional fallback, return `NeedsEvidence`
/// 7. If no transitions at all, return `NoTransitions`
pub fn resolve_transition(
    template_state: &TemplateState,
    evidence: &BTreeMap<String, serde_json::Value>,
) -> TransitionResolution {
    if template_state.transitions.is_empty() {
        return TransitionResolution::NoTransitions;
    }

    let mut conditional_matches: Vec<String> = Vec::new();
    let mut unconditional_target: Option<String> = None;
    let mut has_conditional = false;

    for transition in &template_state.transitions {
        match &transition.when {
            Some(conditions) => {
                has_conditional = true;
                let all_match = conditions
                    .iter()
                    .all(|(field, expected)| evidence.get(field) == Some(expected));
                if all_match {
                    conditional_matches.push(transition.target.clone());
                }
            }
            None => {
                unconditional_target = Some(transition.target.clone());
            }
        }
    }

    match conditional_matches.len() {
        1 => TransitionResolution::Resolved(conditional_matches.into_iter().next().unwrap()),
        n if n > 1 => TransitionResolution::Ambiguous(conditional_matches),
        _ => {
            // No conditional match.
            if let Some(fallback) = unconditional_target {
                TransitionResolution::Resolved(fallback)
            } else if has_conditional {
                TransitionResolution::NeedsEvidence
            } else {
                // All transitions are unconditional (shouldn't happen with valid templates,
                // but handle gracefully).
                TransitionResolution::NoTransitions
            }
        }
    }
}

/// Merge evidence from the current epoch's `evidence_submitted` events.
///
/// Returns a single map where later submissions for the same field override
/// earlier ones (last-write-wins within the epoch).
pub fn merge_epoch_evidence(events: &[Event]) -> BTreeMap<String, serde_json::Value> {
    let mut merged = BTreeMap::new();
    for event in events {
        if let EventPayload::EvidenceSubmitted { fields, .. } = &event.payload {
            for (key, value) in fields {
                merged.insert(key.clone(), value.clone());
            }
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::types::Transition;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicBool;

    fn make_state(transitions: Vec<Transition>) -> TemplateState {
        TemplateState {
            directive: "test".to_string(),
            transitions,
            terminal: false,
            gates: BTreeMap::new(),
            accepts: None,
            integration: None,
        }
    }

    fn unconditional(target: &str) -> Transition {
        Transition {
            target: target.to_string(),
            when: None,
        }
    }

    fn conditional(target: &str, conditions: Vec<(&str, serde_json::Value)>) -> Transition {
        let mut when = BTreeMap::new();
        for (k, v) in conditions {
            when.insert(k.to_string(), v);
        }
        Transition {
            target: target.to_string(),
            when: Some(when),
        }
    }

    fn make_template(states: Vec<(&str, TemplateState)>) -> CompiledTemplate {
        let mut state_map = BTreeMap::new();
        let initial = states
            .first()
            .map(|(name, _)| name.to_string())
            .unwrap_or_default();
        for (name, state) in states {
            state_map.insert(name.to_string(), state);
        }
        CompiledTemplate {
            format_version: 1,
            name: "test".to_string(),
            version: "1.0.0".to_string(),
            description: String::new(),
            initial_state: initial,
            variables: BTreeMap::new(),
            states: state_map,
        }
    }

    fn noop_gates(
        _gates: &BTreeMap<String, crate::template::types::Gate>,
    ) -> BTreeMap<String, GateResult> {
        BTreeMap::new()
    }

    fn unavailable_integration(_name: &str) -> Result<serde_json::Value, IntegrationError> {
        Err(IntegrationError::Unavailable)
    }

    // -----------------------------------------------------------------------
    // resolve_transition tests
    // -----------------------------------------------------------------------

    #[test]
    fn unconditional_transition_resolves() {
        let state = make_state(vec![unconditional("next")]);
        let evidence = BTreeMap::new();
        assert_eq!(
            resolve_transition(&state, &evidence),
            TransitionResolution::Resolved("next".to_string())
        );
    }

    #[test]
    fn single_conditional_match() {
        let state = make_state(vec![conditional(
            "approved",
            vec![("decision", serde_json::json!("approve"))],
        )]);
        let mut evidence = BTreeMap::new();
        evidence.insert("decision".to_string(), serde_json::json!("approve"));
        assert_eq!(
            resolve_transition(&state, &evidence),
            TransitionResolution::Resolved("approved".to_string())
        );
    }

    #[test]
    fn conditional_with_fallback_match_wins() {
        let state = make_state(vec![
            conditional("approved", vec![("decision", serde_json::json!("approve"))]),
            unconditional("fallback"),
        ]);
        let mut evidence = BTreeMap::new();
        evidence.insert("decision".to_string(), serde_json::json!("approve"));
        assert_eq!(
            resolve_transition(&state, &evidence),
            TransitionResolution::Resolved("approved".to_string())
        );
    }

    #[test]
    fn conditional_no_match_falls_to_unconditional() {
        let state = make_state(vec![
            conditional("approved", vec![("decision", serde_json::json!("approve"))]),
            unconditional("fallback"),
        ]);
        let mut evidence = BTreeMap::new();
        evidence.insert("decision".to_string(), serde_json::json!("reject"));
        assert_eq!(
            resolve_transition(&state, &evidence),
            TransitionResolution::Resolved("fallback".to_string())
        );
    }

    #[test]
    fn multiple_conditional_matches_returns_ambiguous() {
        let state = make_state(vec![
            conditional("target_a", vec![("x", serde_json::json!(1))]),
            conditional("target_b", vec![("x", serde_json::json!(1))]),
        ]);
        let mut evidence = BTreeMap::new();
        evidence.insert("x".to_string(), serde_json::json!(1));
        assert_eq!(
            resolve_transition(&state, &evidence),
            TransitionResolution::Ambiguous(vec!["target_a".to_string(), "target_b".to_string()])
        );
    }

    #[test]
    fn no_transitions_returns_no_transitions() {
        let state = make_state(vec![]);
        let evidence = BTreeMap::new();
        assert_eq!(
            resolve_transition(&state, &evidence),
            TransitionResolution::NoTransitions
        );
    }

    #[test]
    fn no_match_no_fallback_returns_needs_evidence() {
        let state = make_state(vec![
            conditional("approved", vec![("decision", serde_json::json!("approve"))]),
            conditional("rejected", vec![("decision", serde_json::json!("reject"))]),
        ]);
        // Empty evidence -- no match.
        let evidence = BTreeMap::new();
        assert_eq!(
            resolve_transition(&state, &evidence),
            TransitionResolution::NeedsEvidence
        );
    }

    #[test]
    fn multi_field_condition_requires_all_match() {
        let state = make_state(vec![conditional(
            "target",
            vec![("a", serde_json::json!("x")), ("b", serde_json::json!("y"))],
        )]);

        // Only one field matches -- should not resolve.
        let mut evidence = BTreeMap::new();
        evidence.insert("a".to_string(), serde_json::json!("x"));
        assert_eq!(
            resolve_transition(&state, &evidence),
            TransitionResolution::NeedsEvidence
        );

        // Both fields match.
        evidence.insert("b".to_string(), serde_json::json!("y"));
        assert_eq!(
            resolve_transition(&state, &evidence),
            TransitionResolution::Resolved("target".to_string())
        );
    }

    // -----------------------------------------------------------------------
    // merge_epoch_evidence tests
    // -----------------------------------------------------------------------

    #[test]
    fn merge_evidence_last_write_wins() {
        let events = vec![
            Event {
                seq: 1,
                timestamp: "2026-01-01T00:00:00Z".to_string(),
                event_type: "evidence_submitted".to_string(),
                payload: EventPayload::EvidenceSubmitted {
                    state: "verify".to_string(),
                    fields: {
                        let mut m = HashMap::new();
                        m.insert("decision".to_string(), serde_json::json!("reject"));
                        m
                    },
                },
            },
            Event {
                seq: 2,
                timestamp: "2026-01-01T00:00:01Z".to_string(),
                event_type: "evidence_submitted".to_string(),
                payload: EventPayload::EvidenceSubmitted {
                    state: "verify".to_string(),
                    fields: {
                        let mut m = HashMap::new();
                        m.insert("decision".to_string(), serde_json::json!("approve"));
                        m
                    },
                },
            },
        ];

        let merged = merge_epoch_evidence(&events);
        assert_eq!(merged.get("decision"), Some(&serde_json::json!("approve")));
    }

    #[test]
    fn merge_evidence_preserves_different_fields() {
        let events = vec![
            Event {
                seq: 1,
                timestamp: "2026-01-01T00:00:00Z".to_string(),
                event_type: "evidence_submitted".to_string(),
                payload: EventPayload::EvidenceSubmitted {
                    state: "review".to_string(),
                    fields: {
                        let mut m = HashMap::new();
                        m.insert("quality".to_string(), serde_json::json!("good"));
                        m
                    },
                },
            },
            Event {
                seq: 2,
                timestamp: "2026-01-01T00:00:01Z".to_string(),
                event_type: "evidence_submitted".to_string(),
                payload: EventPayload::EvidenceSubmitted {
                    state: "review".to_string(),
                    fields: {
                        let mut m = HashMap::new();
                        m.insert("coverage".to_string(), serde_json::json!(85));
                        m
                    },
                },
            },
        ];

        let merged = merge_epoch_evidence(&events);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged.get("quality"), Some(&serde_json::json!("good")));
        assert_eq!(merged.get("coverage"), Some(&serde_json::json!(85)));
    }

    #[test]
    fn merge_evidence_ignores_non_evidence_events() {
        let events = vec![Event {
            seq: 1,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: "transitioned".to_string(),
            payload: EventPayload::Transitioned {
                from: Some("a".to_string()),
                to: "b".to_string(),
                condition_type: "auto".to_string(),
            },
        }];

        let merged = merge_epoch_evidence(&events);
        assert!(merged.is_empty());
    }

    // -----------------------------------------------------------------------
    // advance_until_stop tests
    // -----------------------------------------------------------------------

    #[test]
    fn auto_advance_chain_through_three_states() {
        // plan -> implement -> verify (has accepts, stops with EvidenceRequired)
        let template = make_template(vec![
            (
                "plan",
                TemplateState {
                    directive: "Plan.".to_string(),
                    transitions: vec![unconditional("implement")],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                },
            ),
            (
                "implement",
                TemplateState {
                    directive: "Implement.".to_string(),
                    transitions: vec![unconditional("verify")],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                },
            ),
            (
                "verify",
                TemplateState {
                    directive: "Verify.".to_string(),
                    transitions: vec![
                        conditional("done", vec![("decision", serde_json::json!("approve"))]),
                        conditional("implement", vec![("decision", serde_json::json!("reject"))]),
                    ],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                },
            ),
            (
                "done",
                TemplateState {
                    directive: "Done.".to_string(),
                    transitions: vec![],
                    terminal: true,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                },
            ),
        ]);

        let mut appended: Vec<EventPayload> = Vec::new();
        let mut append = |payload: &EventPayload| -> Result<(), String> {
            appended.push(payload.clone());
            Ok(())
        };
        let shutdown = AtomicBool::new(false);

        let result = advance_until_stop(
            "plan",
            &template,
            &BTreeMap::new(),
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &shutdown,
        )
        .unwrap();

        assert_eq!(result.final_state, "verify");
        assert!(result.advanced);
        assert_eq!(result.stop_reason, StopReason::EvidenceRequired);
        assert_eq!(appended.len(), 2); // plan->implement, implement->verify
    }

    #[test]
    fn gate_blocked_stops_loop() {
        use crate::template::types::Gate;

        let mut gates = BTreeMap::new();
        gates.insert(
            "check".to_string(),
            Gate {
                gate_type: "command".to_string(),
                command: "false".to_string(),
                timeout: 0,
            },
        );

        let template = make_template(vec![(
            "gated",
            TemplateState {
                directive: "Gated.".to_string(),
                transitions: vec![unconditional("next")],
                terminal: false,
                gates,
                accepts: None,
                integration: None,
            },
        )]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        let gate_eval = |gates: &BTreeMap<String, crate::template::types::Gate>| {
            let mut results = BTreeMap::new();
            for (name, _) in gates {
                results.insert(name.clone(), GateResult::Failed { exit_code: 1 });
            }
            results
        };

        let result = advance_until_stop(
            "gated",
            &template,
            &BTreeMap::new(),
            &mut append,
            &gate_eval,
            &unavailable_integration,
            &shutdown,
        )
        .unwrap();

        assert_eq!(result.final_state, "gated");
        assert!(!result.advanced);
        assert!(matches!(result.stop_reason, StopReason::GateBlocked(_)));
    }

    #[test]
    fn evidence_required_stops_loop() {
        let template = make_template(vec![(
            "review",
            TemplateState {
                directive: "Review.".to_string(),
                transitions: vec![
                    conditional("approved", vec![("decision", serde_json::json!("approve"))]),
                    conditional("rejected", vec![("decision", serde_json::json!("reject"))]),
                ],
                terminal: false,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
            },
        )]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        let result = advance_until_stop(
            "review",
            &template,
            &BTreeMap::new(),
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &shutdown,
        )
        .unwrap();

        assert_eq!(result.final_state, "review");
        assert!(!result.advanced);
        assert_eq!(result.stop_reason, StopReason::EvidenceRequired);
    }

    #[test]
    fn cycle_detection() {
        // a -> b -> a -> b (cycle detected on second visit to b)
        // Starting state (a) is not in the visited set, so a -> b -> a is allowed.
        // The cycle is detected when trying to visit b a second time.
        let template = make_template(vec![
            (
                "a",
                TemplateState {
                    directive: "A.".to_string(),
                    transitions: vec![unconditional("b")],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                },
            ),
            (
                "b",
                TemplateState {
                    directive: "B.".to_string(),
                    transitions: vec![unconditional("a")],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                },
            ),
        ]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        let result = advance_until_stop(
            "a",
            &template,
            &BTreeMap::new(),
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &shutdown,
        )
        .unwrap();

        // a -> b (b added to visited), b -> a (a added to visited),
        // a -> b (b already visited: cycle detected)
        assert_eq!(result.final_state, "a");
        assert!(result.advanced); // a -> b -> a happened
        assert_eq!(
            result.stop_reason,
            StopReason::CycleDetected {
                state: "b".to_string()
            }
        );
    }

    #[test]
    fn integration_stops_loop() {
        let template = make_template(vec![(
            "integrate",
            TemplateState {
                directive: "Integrate.".to_string(),
                transitions: vec![unconditional("next")],
                terminal: false,
                gates: BTreeMap::new(),
                accepts: None,
                integration: Some("my-runner".to_string()),
            },
        )]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        let integration = |name: &str| -> Result<serde_json::Value, IntegrationError> {
            Ok(serde_json::json!({"runner": name, "status": "ok"}))
        };

        let result = advance_until_stop(
            "integrate",
            &template,
            &BTreeMap::new(),
            &mut append,
            &noop_gates,
            &integration,
            &shutdown,
        )
        .unwrap();

        assert_eq!(result.final_state, "integrate");
        assert!(!result.advanced);
        assert!(matches!(result.stop_reason, StopReason::Integration { .. }));
    }

    #[test]
    fn chain_limit_reached() {
        // Build a template with 101+ linearly chaining states.
        let mut states: Vec<(&str, TemplateState)> = Vec::new();
        let names: Vec<String> = (0..=MAX_CHAIN_LENGTH + 1)
            .map(|i| format!("s{}", i))
            .collect();

        // Leak the names so we can use &str references.
        // This is fine in tests.
        let names: Vec<&str> = names.iter().map(|s| &**s).collect();

        for i in 0..names.len() - 1 {
            states.push((
                names[i],
                TemplateState {
                    directive: format!("State {}.", i),
                    transitions: vec![unconditional(names[i + 1])],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                },
            ));
        }
        // Terminal state at the end
        states.push((
            *names.last().unwrap(),
            TemplateState {
                directive: "Final.".to_string(),
                transitions: vec![],
                terminal: true,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
            },
        ));

        let template = make_template(states);
        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        let result = advance_until_stop(
            "s0",
            &template,
            &BTreeMap::new(),
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &shutdown,
        )
        .unwrap();

        assert!(result.advanced);
        assert_eq!(result.stop_reason, StopReason::ChainLimitReached);
    }

    #[test]
    fn terminal_state_stops_immediately() {
        let template = make_template(vec![(
            "done",
            TemplateState {
                directive: "Done.".to_string(),
                transitions: vec![],
                terminal: true,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
            },
        )]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        let result = advance_until_stop(
            "done",
            &template,
            &BTreeMap::new(),
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &shutdown,
        )
        .unwrap();

        assert_eq!(result.final_state, "done");
        assert!(!result.advanced);
        assert_eq!(result.stop_reason, StopReason::Terminal);
    }

    #[test]
    fn signal_received_stops_loop() {
        let template = make_template(vec![
            (
                "a",
                TemplateState {
                    directive: "A.".to_string(),
                    transitions: vec![unconditional("b")],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                },
            ),
            (
                "b",
                TemplateState {
                    directive: "B.".to_string(),
                    transitions: vec![unconditional("c")],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                },
            ),
            (
                "c",
                TemplateState {
                    directive: "C.".to_string(),
                    transitions: vec![],
                    terminal: true,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                },
            ),
        ]);

        // Set shutdown before starting
        let shutdown = AtomicBool::new(true);
        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };

        let result = advance_until_stop(
            "a",
            &template,
            &BTreeMap::new(),
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &shutdown,
        )
        .unwrap();

        assert_eq!(result.final_state, "a");
        assert!(!result.advanced);
        assert_eq!(result.stop_reason, StopReason::SignalReceived);
    }

    #[test]
    fn auto_advance_clears_evidence_for_new_states() {
        // State "start" has evidence matching condition, advances to "middle".
        // "middle" has a conditional transition that should NOT match (fresh epoch).
        let template = make_template(vec![
            (
                "start",
                TemplateState {
                    directive: "Start.".to_string(),
                    transitions: vec![conditional("middle", vec![("go", serde_json::json!(true))])],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                },
            ),
            (
                "middle",
                TemplateState {
                    directive: "Middle.".to_string(),
                    transitions: vec![conditional("end", vec![("go", serde_json::json!(true))])],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                },
            ),
            (
                "end",
                TemplateState {
                    directive: "End.".to_string(),
                    transitions: vec![],
                    terminal: true,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                },
            ),
        ]);

        let mut evidence = BTreeMap::new();
        evidence.insert("go".to_string(), serde_json::json!(true));

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        let result = advance_until_stop(
            "start",
            &template,
            &evidence,
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &shutdown,
        )
        .unwrap();

        // Should stop at "middle" because evidence is cleared after auto-advance.
        assert_eq!(result.final_state, "middle");
        assert!(result.advanced);
        assert_eq!(result.stop_reason, StopReason::EvidenceRequired);
    }
}
