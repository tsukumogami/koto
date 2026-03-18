// Auto-advancement engine: transition resolution and advancement loop.
//
// Implemented for Issue #49.

use std::collections::BTreeMap;

use crate::engine::types::{Event, EventPayload};
use crate::gate::GateResult;
use crate::template::types::TemplateState;

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
                let all_match = conditions.iter().all(|(field, expected)| {
                    evidence
                        .get(field)
                        .map_or(false, |actual| actual == expected)
                });
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
}
