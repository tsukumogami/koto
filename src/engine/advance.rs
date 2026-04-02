// Auto-advancement engine: transition resolution and advancement loop.
//
// Implemented for Issue #49.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::engine::persistence::derive_overrides;
use crate::engine::types::{now_iso8601, Event, EventPayload};
use crate::gate::{GateOutcome, StructuredGateResult};
use crate::template::types::{
    ActionDecl, CompiledTemplate, TemplateState, GATES_EVIDENCE_NAMESPACE,
};

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

/// Result of executing a default action.
#[derive(Debug, Clone)]
pub enum ActionResult {
    /// Action executed successfully.
    Executed {
        exit_code: i32,
        stdout: String,
        stderr: String,
    },
    /// Action was skipped (override evidence existed).
    Skipped,
    /// Action executed but requires user confirmation before continuing.
    RequiresConfirmation {
        exit_code: i32,
        stdout: String,
        stderr: String,
    },
}

/// Why the advancement loop stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum StopReason {
    /// Reached a terminal state.
    Terminal,
    /// One or more gates failed.
    GateBlocked(BTreeMap<String, StructuredGateResult>),
    /// Conditional transitions exist but evidence doesn't match any.
    EvidenceRequired {
        failed_gates: Option<BTreeMap<String, StructuredGateResult>>,
    },
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
    /// Action executed but requires user confirmation before continuing.
    ActionRequiresConfirmation {
        state: String,
        exit_code: i32,
        stdout: String,
        stderr: String,
    },
    /// SIGTERM or SIGINT received between iterations.
    SignalReceived,
    /// Conditional transitions exist but no evidence matches, and the state
    /// has no accepts block so the agent can't submit evidence to resolve it.
    UnresolvableTransition,
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
/// 2. Chain limit check
/// 3. Terminal state
/// 4. Integration declared (invoke runner)
/// 5. Action execution (if state has default_action)
/// 6. Gates (evaluate all, stop if any fail)
/// 7. Transition resolution (match evidence against conditions)
///
/// I/O operations are injected as closures for testability:
/// - `append_event`: persist a state transition event
/// - `evaluate_gates`: run gate commands and return results
/// - `invoke_integration`: call an integration runner
/// - `execute_action`: run a default action command
#[allow(clippy::too_many_arguments)]
pub fn advance_until_stop<F, G, I, A>(
    current_state: &str,
    template: &CompiledTemplate,
    evidence: &BTreeMap<String, serde_json::Value>,
    all_events: &[Event],
    append_event: &mut F,
    evaluate_gates: &G,
    invoke_integration: &I,
    execute_action: &A,
    shutdown: &AtomicBool,
) -> Result<AdvanceResult, AdvanceError>
where
    F: FnMut(&EventPayload) -> Result<(), String>,
    G: Fn(
        &BTreeMap<String, crate::template::types::Gate>,
    ) -> BTreeMap<String, StructuredGateResult>,
    I: Fn(&str) -> Result<serde_json::Value, IntegrationError>,
    A: Fn(&str, &ActionDecl, bool) -> ActionResult,
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

        // 5. Action execution (if state has default_action)
        if let Some(action) = &template_state.default_action {
            let has_evidence = !current_evidence.is_empty();
            let result = execute_action(&state, action, has_evidence);
            match result {
                ActionResult::Executed { .. } => {
                    // Continue to gate evaluation
                }
                ActionResult::Skipped => {
                    // Continue to gate evaluation
                }
                ActionResult::RequiresConfirmation {
                    exit_code,
                    stdout,
                    stderr,
                } => {
                    return Ok(AdvanceResult {
                        final_state: state.clone(),
                        advanced,
                        stop_reason: StopReason::ActionRequiresConfirmation {
                            state,
                            exit_code,
                            stdout,
                            stderr,
                        },
                    });
                }
            }
        }

        // 6. Evaluate gates
        let mut gates_failed = false;
        let mut failed_gate_results: Option<BTreeMap<String, StructuredGateResult>> = None;
        // Gate outputs to inject into evidence (populated whenever gates are present).
        let mut gate_evidence_map: serde_json::Map<String, serde_json::Value> =
            serde_json::Map::new();
        if !template_state.gates.is_empty() {
            // Derive active overrides for the current epoch before iterating gates.
            // Convert the list of GateOverrideRecorded events to a map from gate name
            // to override_applied value (last override wins when a gate has multiple).
            let epoch_overrides: BTreeMap<String, serde_json::Value> = {
                let override_events = derive_overrides(all_events);
                let mut map = BTreeMap::new();
                for event in override_events {
                    if let EventPayload::GateOverrideRecorded {
                        gate,
                        override_applied,
                        ..
                    } = &event.payload
                    {
                        map.insert(gate.clone(), override_applied.clone());
                    }
                }
                map
            };

            // Evaluate each gate, injecting a synthetic Passed result for overridden
            // gates instead of calling evaluate_gates.
            let mut gate_results: BTreeMap<String, StructuredGateResult> = BTreeMap::new();
            let mut gates_to_evaluate: BTreeMap<String, crate::template::types::Gate> =
                BTreeMap::new();

            for (gate_name, gate_def) in &template_state.gates {
                if let Some(override_applied) = epoch_overrides.get(gate_name) {
                    // Gate has an active override: inject the override value and a
                    // synthetic Passed result without calling evaluate_gates.
                    gate_evidence_map.insert(gate_name.clone(), override_applied.clone());
                    gate_results.insert(
                        gate_name.clone(),
                        StructuredGateResult {
                            outcome: GateOutcome::Passed,
                            output: override_applied.clone(),
                        },
                    );
                    // No GateEvaluated event is emitted for overridden gates.
                    let _ = gate_def; // suppress unused variable warning
                } else {
                    gates_to_evaluate.insert(gate_name.clone(), gate_def.clone());
                }
            }

            // Evaluate non-overridden gates and emit GateEvaluated events.
            if !gates_to_evaluate.is_empty() {
                let evaluated = evaluate_gates(&gates_to_evaluate);
                for (gate_name, result) in &evaluated {
                    gate_evidence_map.insert(gate_name.clone(), result.output.clone());
                    let outcome_str = match result.outcome {
                        GateOutcome::Passed => "passed",
                        GateOutcome::Failed => "failed",
                        GateOutcome::TimedOut => "timed_out",
                        GateOutcome::Error => "error",
                    };
                    let gate_evaluated_payload = EventPayload::GateEvaluated {
                        state: state.clone(),
                        gate: gate_name.clone(),
                        output: result.output.clone(),
                        outcome: outcome_str.to_string(),
                        timestamp: now_iso8601(),
                    };
                    append_event(&gate_evaluated_payload)
                        .map_err(AdvanceError::PersistenceError)?;
                    gate_results.insert(gate_name.clone(), result.clone());
                }
            }

            // Build the gates sub-map: {"gate_name": output, ...}
            // This is injected into the evidence regardless of pass/fail so that
            // when clauses referencing gates.* can route based on gate output.
            // (gate_evidence_map is already populated above)

            let any_failed = gate_results
                .values()
                .any(|r| !matches!(r.outcome, GateOutcome::Passed));
            if any_failed {
                // Determine whether this state can route based on gate output alone.
                // A state does so when it has at least one conditional transition
                // whose when clause references a gates.* key. In that case, fall
                // through to transition resolution so the gate output can drive
                // routing (scenarios 14, 15, 16 in the structured-gate-output PRD).
                //
                // If the state has an accepts block, also fall through so the
                // transition resolver can use evidence as a fallback. The resolver
                // will skip unconditional transitions when gate_failed is true,
                // ensuring the agent must submit evidence when no conditional
                // transition matches.
                //
                // If neither condition holds, return GateBlocked immediately.
                let has_gates_routing = template_state.transitions.iter().any(|t| {
                    t.when
                        .as_ref()
                        .map(|w| {
                            w.keys()
                                .any(|k| k.starts_with(&format!("{}.", GATES_EVIDENCE_NAMESPACE)))
                        })
                        .unwrap_or(false)
                });
                if template_state.accepts.is_none() && !has_gates_routing {
                    return Ok(AdvanceResult {
                        final_state: state,
                        advanced,
                        stop_reason: StopReason::GateBlocked(gate_results),
                    });
                }
                gates_failed = true;
                failed_gate_results = Some(gate_results);
                // Fall through to transition resolution with gate_failed=true.
            }
        }

        // 7. Resolve transition
        // Build a merged evidence Value: start with agent evidence (flat keys),
        // then layer gate output under "gates" (engine data takes precedence).
        // This allows when clauses to reference both agent-submitted fields and
        // gate output via dot-path traversal (e.g. gates.ci_check.exit_code).
        //
        // The "gates" key is reserved: handle_next rejects any --with-data
        // payload containing a top-level "gates" key (InvalidSubmission), so by
        // this point current_evidence must not contain "gates". The assert below
        // enforces this invariant in debug builds and catches any future code
        // path that bypasses the CLI check.
        debug_assert!(
            !current_evidence.contains_key("gates"),
            "invariant violated: current_evidence contains reserved key 'gates'; \
             handle_next must reject submissions with this key before reaching the advance loop"
        );
        let mut merged: serde_json::Map<String, serde_json::Value> = current_evidence
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if !gate_evidence_map.is_empty() {
            merged.insert(
                "gates".to_string(),
                serde_json::Value::Object(gate_evidence_map),
            );
        }
        let evidence_value = serde_json::Value::Object(merged);
        match resolve_transition(template_state, &evidence_value, gates_failed) {
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
                if template_state.accepts.is_some() {
                    return Ok(AdvanceResult {
                        final_state: state,
                        advanced,
                        stop_reason: StopReason::EvidenceRequired {
                            failed_gates: failed_gate_results,
                        },
                    });
                } else if let Some(gate_results) = failed_gate_results {
                    // No accepts block but gate(s) failed and no gates.* condition
                    // matched -- the gate itself is blocking.
                    return Ok(AdvanceResult {
                        final_state: state,
                        advanced,
                        stop_reason: StopReason::GateBlocked(gate_results),
                    });
                } else {
                    return Ok(AdvanceResult {
                        final_state: state,
                        advanced,
                        stop_reason: StopReason::UnresolvableTransition,
                    });
                }
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

/// Traverse a nested `serde_json::Value` using a dot-separated path.
///
/// Each segment of `path` is split on `.` and used as a key into the current
/// JSON object. Returns `None` if any segment is missing, if an intermediate
/// value is not an object, or if `path` is empty.
///
/// Single-segment paths behave identically to a direct `.get()` call, so flat
/// evidence keys work without any changes at call sites.
///
/// # Examples
///
/// ```ignore
/// let v = serde_json::json!({"gates": {"ci": {"exit_code": 0}}});
/// assert_eq!(resolve_value(&v, "gates.ci.exit_code"), Some(&serde_json::json!(0)));
/// assert_eq!(resolve_value(&v, "gates.ci.missing"), None);
/// ```
fn resolve_value<'a>(root: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    if path.is_empty() {
        return None;
    }
    let mut current = root;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

/// Resolve which transition to take from a state given current evidence.
///
/// Resolution algorithm:
/// 1. Collect conditional transitions (those with `when: Some(...)`)
/// 2. For each, check if ALL `when` fields match the evidence (exact JSON equality)
///    using dot-path traversal so nested keys like `gates.ci.exit_code` work
/// 3. If exactly one matches, return `Resolved(target)`
/// 4. If multiple match, return `Ambiguous(targets)`
/// 5. If none match and an unconditional transition exists:
///    - If `gate_failed` is false, return `Resolved(fallback)` (auto-advance)
///    - If `gate_failed` is true, return `NeedsEvidence` (require evidence before advancing)
/// 6. If none match and no unconditional fallback, return `NeedsEvidence`
/// 7. If no transitions at all, return `NoTransitions`
///
/// The `gate_failed` parameter prevents unconditional transitions from firing when
/// the engine fell through from a gate failure. Without this, states with both gates
/// and accepts blocks would auto-advance via the unconditional fallback even when
/// gates fail — defeating the evidence-fallback mechanism.
pub fn resolve_transition(
    template_state: &TemplateState,
    evidence: &serde_json::Value,
    gate_failed: bool,
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
                    .all(|(field, expected)| resolve_value(evidence, field) == Some(expected));
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
                if gate_failed {
                    // Gate failed and no evidence matches a conditional transition.
                    // Don't auto-advance via the unconditional fallback — require
                    // evidence so the agent can provide override or recovery input.
                    TransitionResolution::NeedsEvidence
                } else {
                    TransitionResolution::Resolved(fallback)
                }
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
            details: String::new(),
            transitions,
            terminal: false,
            gates: BTreeMap::new(),
            accepts: None,
            integration: None,
            default_action: None,
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

    fn make_accepts(
        fields: Vec<&str>,
    ) -> Option<BTreeMap<String, crate::template::types::FieldSchema>> {
        let mut map = BTreeMap::new();
        for field in fields {
            map.insert(
                field.to_string(),
                crate::template::types::FieldSchema {
                    field_type: "string".to_string(),
                    required: true,
                    values: vec![],
                    description: String::new(),
                },
            );
        }
        Some(map)
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
    ) -> BTreeMap<String, StructuredGateResult> {
        BTreeMap::new()
    }

    fn unavailable_integration(_name: &str) -> Result<serde_json::Value, IntegrationError> {
        Err(IntegrationError::Unavailable)
    }

    fn noop_action(
        _state: &str,
        _action: &crate::template::types::ActionDecl,
        _has_evidence: bool,
    ) -> ActionResult {
        ActionResult::Skipped
    }

    // -----------------------------------------------------------------------
    // resolve_transition tests
    // -----------------------------------------------------------------------

    /// Wrap a BTreeMap as a serde_json::Value::Object for resolve_transition.
    fn as_evidence(m: BTreeMap<String, serde_json::Value>) -> serde_json::Value {
        serde_json::to_value(m).unwrap()
    }

    #[test]
    fn unconditional_transition_resolves() {
        let state = make_state(vec![unconditional("next")]);
        let evidence = as_evidence(BTreeMap::new());
        assert_eq!(
            resolve_transition(&state, &evidence, false),
            TransitionResolution::Resolved("next".to_string())
        );
    }

    #[test]
    fn single_conditional_match() {
        let state = make_state(vec![conditional(
            "approved",
            vec![("decision", serde_json::json!("approve"))],
        )]);
        let mut m = BTreeMap::new();
        m.insert("decision".to_string(), serde_json::json!("approve"));
        let evidence = as_evidence(m);
        assert_eq!(
            resolve_transition(&state, &evidence, false),
            TransitionResolution::Resolved("approved".to_string())
        );
    }

    #[test]
    fn conditional_with_fallback_match_wins() {
        let state = make_state(vec![
            conditional("approved", vec![("decision", serde_json::json!("approve"))]),
            unconditional("fallback"),
        ]);
        let mut m = BTreeMap::new();
        m.insert("decision".to_string(), serde_json::json!("approve"));
        let evidence = as_evidence(m);
        assert_eq!(
            resolve_transition(&state, &evidence, false),
            TransitionResolution::Resolved("approved".to_string())
        );
    }

    #[test]
    fn conditional_no_match_falls_to_unconditional() {
        let state = make_state(vec![
            conditional("approved", vec![("decision", serde_json::json!("approve"))]),
            unconditional("fallback"),
        ]);
        let mut m = BTreeMap::new();
        m.insert("decision".to_string(), serde_json::json!("reject"));
        let evidence = as_evidence(m);
        assert_eq!(
            resolve_transition(&state, &evidence, false),
            TransitionResolution::Resolved("fallback".to_string())
        );
    }

    #[test]
    fn multiple_conditional_matches_returns_ambiguous() {
        let state = make_state(vec![
            conditional("target_a", vec![("x", serde_json::json!(1))]),
            conditional("target_b", vec![("x", serde_json::json!(1))]),
        ]);
        let mut m = BTreeMap::new();
        m.insert("x".to_string(), serde_json::json!(1));
        let evidence = as_evidence(m);
        assert_eq!(
            resolve_transition(&state, &evidence, false),
            TransitionResolution::Ambiguous(vec!["target_a".to_string(), "target_b".to_string()])
        );
    }

    #[test]
    fn no_transitions_returns_no_transitions() {
        let state = make_state(vec![]);
        let evidence = as_evidence(BTreeMap::new());
        assert_eq!(
            resolve_transition(&state, &evidence, false),
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
        let evidence = as_evidence(BTreeMap::new());
        assert_eq!(
            resolve_transition(&state, &evidence, false),
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
        let mut m = BTreeMap::new();
        m.insert("a".to_string(), serde_json::json!("x"));
        let evidence = as_evidence(m.clone());
        assert_eq!(
            resolve_transition(&state, &evidence, false),
            TransitionResolution::NeedsEvidence
        );

        // Both fields match.
        m.insert("b".to_string(), serde_json::json!("y"));
        let evidence = as_evidence(m);
        assert_eq!(
            resolve_transition(&state, &evidence, false),
            TransitionResolution::Resolved("target".to_string())
        );
    }

    #[test]
    fn gate_failed_skips_unconditional_fallback() {
        // State with a conditional transition and an unconditional fallback.
        // When gate_failed=false, the unconditional fires. When gate_failed=true,
        // it returns NeedsEvidence instead.
        let state = make_state(vec![
            conditional(
                "next_state",
                vec![("status", serde_json::json!("completed"))],
            ),
            unconditional("fallback_state"),
        ]);

        let evidence = as_evidence(BTreeMap::new()); // no evidence

        // gate_failed=false: unconditional fallback fires
        assert_eq!(
            resolve_transition(&state, &evidence, false),
            TransitionResolution::Resolved("fallback_state".to_string())
        );

        // gate_failed=true: unconditional fallback skipped, needs evidence
        assert_eq!(
            resolve_transition(&state, &evidence, true),
            TransitionResolution::NeedsEvidence
        );

        // gate_failed=true but evidence matches conditional: resolves normally
        let mut m = BTreeMap::new();
        m.insert("status".to_string(), serde_json::json!("completed"));
        let with_evidence = as_evidence(m);
        assert_eq!(
            resolve_transition(&state, &with_evidence, true),
            TransitionResolution::Resolved("next_state".to_string())
        );
    }

    #[test]
    fn dot_path_traversal_on_nested_gate_data() {
        // gate output is nested under "gates.ci_check" -- when clause uses
        // dot-path "gates.ci_check.exit_code"
        let state = make_state(vec![
            conditional(
                "success",
                vec![("gates.ci_check.exit_code", serde_json::json!(0))],
            ),
            conditional(
                "failed",
                vec![("gates.ci_check.exit_code", serde_json::json!(1))],
            ),
        ]);

        // Evidence with nested gate output matching success condition
        let evidence = serde_json::json!({
            "gates": {
                "ci_check": {
                    "exit_code": 0,
                    "error": ""
                }
            }
        });
        assert_eq!(
            resolve_transition(&state, &evidence, false),
            TransitionResolution::Resolved("success".to_string())
        );

        // Non-zero exit code routes to failed
        let evidence_fail = serde_json::json!({
            "gates": {
                "ci_check": {
                    "exit_code": 1,
                    "error": "lint failed"
                }
            }
        });
        assert_eq!(
            resolve_transition(&state, &evidence_fail, false),
            TransitionResolution::Resolved("failed".to_string())
        );
    }

    #[test]
    fn dot_path_missing_segment_returns_none() {
        // when clause references a nested path that does not exist in evidence
        let state = make_state(vec![conditional(
            "target",
            vec![("gates.ci.exit_code", serde_json::json!(0))],
        )]);

        // Evidence without the "gates" key at all
        let evidence = serde_json::json!({ "mode": "issue_backed" });
        assert_eq!(
            resolve_transition(&state, &evidence, false),
            TransitionResolution::NeedsEvidence
        );

        // Evidence with "gates" but missing the "ci" sub-key
        let evidence_partial = serde_json::json!({ "gates": { "lint": { "exit_code": 0 } } });
        assert_eq!(
            resolve_transition(&state, &evidence_partial, false),
            TransitionResolution::NeedsEvidence
        );
    }

    #[test]
    fn mixed_gate_and_flat_evidence() {
        // when clause mixes a dot-path gate key with a flat agent-evidence key
        let state = make_state(vec![
            conditional(
                "approved",
                vec![
                    ("gates.ci.exit_code", serde_json::json!(0)),
                    ("decision", serde_json::json!("approve")),
                ],
            ),
            unconditional("pending"),
        ]);

        // Both conditions satisfied
        let evidence_both = serde_json::json!({
            "gates": { "ci": { "exit_code": 0, "error": "" } },
            "decision": "approve"
        });
        assert_eq!(
            resolve_transition(&state, &evidence_both, false),
            TransitionResolution::Resolved("approved".to_string())
        );

        // Only gate satisfied, no agent decision yet -- falls through to unconditional
        let evidence_gate_only = serde_json::json!({
            "gates": { "ci": { "exit_code": 0, "error": "" } }
        });
        assert_eq!(
            resolve_transition(&state, &evidence_gate_only, false),
            TransitionResolution::Resolved("pending".to_string())
        );

        // Only decision provided, gate output missing -- falls through to unconditional
        let evidence_decision_only = serde_json::json!({ "decision": "approve" });
        assert_eq!(
            resolve_transition(&state, &evidence_decision_only, false),
            TransitionResolution::Resolved("pending".to_string())
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
                    details: String::new(),
                    transitions: vec![unconditional("implement")],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
            (
                "implement",
                TemplateState {
                    directive: "Implement.".to_string(),
                    details: String::new(),
                    transitions: vec![unconditional("verify")],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
            (
                "verify",
                TemplateState {
                    directive: "Verify.".to_string(),
                    details: String::new(),
                    transitions: vec![
                        conditional("done", vec![("decision", serde_json::json!("approve"))]),
                        conditional("implement", vec![("decision", serde_json::json!("reject"))]),
                    ],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: make_accepts(vec!["decision"]),
                    integration: None,
                    default_action: None,
                },
            ),
            (
                "done",
                TemplateState {
                    directive: "Done.".to_string(),
                    details: String::new(),
                    transitions: vec![],
                    terminal: true,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
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
            &[],
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &noop_action,
            &shutdown,
        )
        .unwrap();

        assert_eq!(result.final_state, "verify");
        assert!(result.advanced);
        assert!(matches!(
            result.stop_reason,
            StopReason::EvidenceRequired { .. }
        ));
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
                key: String::new(),
                pattern: String::new(),
                override_default: None,
            },
        );

        let template = make_template(vec![(
            "gated",
            TemplateState {
                directive: "Gated.".to_string(),
                details: String::new(),
                transitions: vec![unconditional("next")],
                terminal: false,
                gates,
                accepts: None,
                integration: None,
                default_action: None,
            },
        )]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        let gate_eval = |gates: &BTreeMap<String, crate::template::types::Gate>| {
            let mut results = BTreeMap::new();
            for (name, _) in gates {
                results.insert(
                    name.clone(),
                    StructuredGateResult {
                        outcome: GateOutcome::Failed,
                        output: serde_json::json!({"exit_code": 1, "error": ""}),
                    },
                );
            }
            results
        };

        let result = advance_until_stop(
            "gated",
            &template,
            &BTreeMap::new(),
            &[],
            &mut append,
            &gate_eval,
            &unavailable_integration,
            &noop_action,
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
                details: String::new(),
                transitions: vec![
                    conditional("approved", vec![("decision", serde_json::json!("approve"))]),
                    conditional("rejected", vec![("decision", serde_json::json!("reject"))]),
                ],
                terminal: false,
                gates: BTreeMap::new(),
                accepts: make_accepts(vec!["decision"]),
                integration: None,
                default_action: None,
            },
        )]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        let result = advance_until_stop(
            "review",
            &template,
            &BTreeMap::new(),
            &[],
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &noop_action,
            &shutdown,
        )
        .unwrap();

        assert_eq!(result.final_state, "review");
        assert!(!result.advanced);
        assert!(matches!(
            result.stop_reason,
            StopReason::EvidenceRequired { .. }
        ));
    }

    #[test]
    fn evidence_required_no_gates_has_none_failed_gates() {
        // When no gates are defined, failed_gates should be None.
        let template = make_template(vec![(
            "review",
            TemplateState {
                directive: "Review.".to_string(),
                details: String::new(),
                transitions: vec![conditional(
                    "approved",
                    vec![("decision", serde_json::json!("approve"))],
                )],
                terminal: false,
                gates: BTreeMap::new(),
                accepts: make_accepts(vec!["decision"]),
                integration: None,
                default_action: None,
            },
        )]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        let result = advance_until_stop(
            "review",
            &template,
            &BTreeMap::new(),
            &[],
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &noop_action,
            &shutdown,
        )
        .unwrap();

        match &result.stop_reason {
            StopReason::EvidenceRequired { failed_gates } => {
                assert!(
                    failed_gates.is_none(),
                    "expected None when no gates defined"
                );
            }
            other => panic!("expected EvidenceRequired, got {:?}", other),
        }
    }

    #[test]
    fn gate_with_evidence_fallback_carries_gate_data() {
        use crate::template::types::Gate;

        // State with gates + accepts: when gates fail, engine returns
        // EvidenceRequired with failed_gates populated.
        let mut gates = BTreeMap::new();
        gates.insert(
            "ci_check".to_string(),
            Gate {
                gate_type: "command".to_string(),
                command: "false".to_string(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
            },
        );

        let template = make_template(vec![(
            "verify",
            TemplateState {
                directive: "Verify.".to_string(),
                details: String::new(),
                transitions: vec![conditional(
                    "done",
                    vec![("result", serde_json::json!("pass"))],
                )],
                terminal: false,
                gates,
                accepts: make_accepts(vec!["result"]),
                integration: None,
                default_action: None,
            },
        )]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        let gate_eval = |gates: &BTreeMap<String, crate::template::types::Gate>| {
            let mut results = BTreeMap::new();
            for (name, _) in gates {
                results.insert(
                    name.clone(),
                    StructuredGateResult {
                        outcome: GateOutcome::Failed,
                        output: serde_json::json!({"exit_code": 1, "error": ""}),
                    },
                );
            }
            results
        };

        let result = advance_until_stop(
            "verify",
            &template,
            &BTreeMap::new(),
            &[],
            &mut append,
            &gate_eval,
            &unavailable_integration,
            &noop_action,
            &shutdown,
        )
        .unwrap();

        assert_eq!(result.final_state, "verify");
        assert!(!result.advanced);
        match &result.stop_reason {
            StopReason::EvidenceRequired { failed_gates } => {
                let gates = failed_gates
                    .as_ref()
                    .expect("failed_gates should be Some when gates failed");
                assert_eq!(gates.len(), 1);
                assert!(gates.contains_key("ci_check"));
                assert_eq!(gates["ci_check"].outcome, GateOutcome::Failed);
                assert_eq!(gates["ci_check"].output["exit_code"], 1);
            }
            other => panic!("expected EvidenceRequired, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Gate evidence merging tests (scenario-10, scenario-11)
    // -----------------------------------------------------------------------

    /// scenario-10: Gate output is injected into the merged evidence map under
    /// "gates" so that when clauses referencing gates.* route correctly.
    #[test]
    fn gate_output_injected_into_evidence_for_routing() {
        use crate::template::types::Gate;

        // State has a passing gate and routes based on gates.ci.exit_code.
        let mut gates = BTreeMap::new();
        gates.insert(
            "ci".to_string(),
            Gate {
                gate_type: "command".to_string(),
                command: "exit 0".to_string(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
            },
        );

        let template = make_template(vec![
            (
                "check",
                TemplateState {
                    directive: "Check.".to_string(),
                    details: String::new(),
                    transitions: vec![
                        conditional(
                            "success",
                            vec![("gates.ci.exit_code", serde_json::json!(0))],
                        ),
                        conditional(
                            "failure",
                            vec![("gates.ci.exit_code", serde_json::json!(1))],
                        ),
                    ],
                    terminal: false,
                    gates,
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
            (
                "success",
                TemplateState {
                    directive: "Success.".to_string(),
                    details: String::new(),
                    transitions: vec![],
                    terminal: true,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
            (
                "failure",
                TemplateState {
                    directive: "Failure.".to_string(),
                    details: String::new(),
                    transitions: vec![],
                    terminal: true,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
        ]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        // Gate evaluator returns passing gate with exit_code 0.
        let gate_eval = |gates: &BTreeMap<String, crate::template::types::Gate>| {
            let mut results = BTreeMap::new();
            for (name, _) in gates {
                results.insert(
                    name.clone(),
                    StructuredGateResult {
                        outcome: GateOutcome::Passed,
                        output: serde_json::json!({"exit_code": 0, "error": ""}),
                    },
                );
            }
            results
        };

        let result = advance_until_stop(
            "check",
            &template,
            &BTreeMap::new(),
            &[],
            &mut append,
            &gate_eval,
            &unavailable_integration,
            &noop_action,
            &shutdown,
        )
        .unwrap();

        // Gate passed (exit_code 0), so the engine routes to "success".
        assert_eq!(result.final_state, "success");
        assert!(result.advanced);
        assert_eq!(result.stop_reason, StopReason::Terminal);
    }

    /// scenario-10 (failure path): Gate fails and routes via gates.* when clause.
    /// The state must have an accepts block so the engine falls through to
    /// transition resolution instead of returning GateBlocked immediately.
    #[test]
    fn gate_output_routes_to_failure_state() {
        use crate::template::types::Gate;

        let mut gates = BTreeMap::new();
        gates.insert(
            "ci".to_string(),
            Gate {
                gate_type: "command".to_string(),
                command: "exit 1".to_string(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
            },
        );

        // The state has an accepts block so that when gates fail, the engine
        // falls through to transition resolution (gate_failed=true). The
        // conditional transitions route on gate output; the matching one fires.
        let template = make_template(vec![
            (
                "check",
                TemplateState {
                    directive: "Check.".to_string(),
                    details: String::new(),
                    transitions: vec![
                        conditional(
                            "success",
                            vec![("gates.ci.exit_code", serde_json::json!(0))],
                        ),
                        conditional("fix", vec![("gates.ci.exit_code", serde_json::json!(1))]),
                    ],
                    terminal: false,
                    gates,
                    accepts: make_accepts(vec!["override"]),
                    integration: None,
                    default_action: None,
                },
            ),
            (
                "success",
                TemplateState {
                    directive: "Success.".to_string(),
                    details: String::new(),
                    transitions: vec![],
                    terminal: true,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
            (
                "fix",
                TemplateState {
                    directive: "Fix.".to_string(),
                    details: String::new(),
                    transitions: vec![],
                    terminal: true,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
        ]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        // Gate evaluator returns a failing gate with exit_code 1.
        let gate_eval = |gates: &BTreeMap<String, crate::template::types::Gate>| {
            let mut results = BTreeMap::new();
            for (name, _) in gates {
                results.insert(
                    name.clone(),
                    StructuredGateResult {
                        outcome: GateOutcome::Failed,
                        output: serde_json::json!({"exit_code": 1, "error": ""}),
                    },
                );
            }
            results
        };

        let result = advance_until_stop(
            "check",
            &template,
            &BTreeMap::new(),
            &[],
            &mut append,
            &gate_eval,
            &unavailable_integration,
            &noop_action,
            &shutdown,
        )
        .unwrap();

        // Gate failed with exit_code 1, so the engine routes to "fix" via the
        // matching gates.ci.exit_code == 1 conditional transition.
        assert_eq!(result.final_state, "fix");
        assert!(result.advanced);
        assert_eq!(result.stop_reason, StopReason::Terminal);
    }

    /// scenario-10: Agent evidence keys appear at the top level alongside
    /// gate output nested under "gates". Engine data (gates) takes precedence
    /// if both define the same top-level key.
    #[test]
    fn gate_evidence_merged_after_agent_evidence() {
        use crate::template::types::Gate;

        let mut gates = BTreeMap::new();
        gates.insert(
            "lint".to_string(),
            Gate {
                gate_type: "command".to_string(),
                command: "exit 0".to_string(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
            },
        );

        // Transition requires both gate output and agent evidence.
        let template = make_template(vec![
            (
                "verify",
                TemplateState {
                    directive: "Verify.".to_string(),
                    details: String::new(),
                    transitions: vec![conditional(
                        "done",
                        vec![
                            ("gates.lint.exit_code", serde_json::json!(0)),
                            ("decision", serde_json::json!("approve")),
                        ],
                    )],
                    terminal: false,
                    gates,
                    accepts: make_accepts(vec!["decision"]),
                    integration: None,
                    default_action: None,
                },
            ),
            (
                "done",
                TemplateState {
                    directive: "Done.".to_string(),
                    details: String::new(),
                    transitions: vec![],
                    terminal: true,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
        ]);

        // Agent evidence has the decision field.
        let mut agent_evidence = BTreeMap::new();
        agent_evidence.insert("decision".to_string(), serde_json::json!("approve"));

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        let gate_eval = |gates: &BTreeMap<String, crate::template::types::Gate>| {
            let mut results = BTreeMap::new();
            for (name, _) in gates {
                results.insert(
                    name.clone(),
                    StructuredGateResult {
                        outcome: GateOutcome::Passed,
                        output: serde_json::json!({"exit_code": 0, "error": ""}),
                    },
                );
            }
            results
        };

        let result = advance_until_stop(
            "verify",
            &template,
            &agent_evidence,
            &[],
            &mut append,
            &gate_eval,
            &unavailable_integration,
            &noop_action,
            &shutdown,
        )
        .unwrap();

        // Both gate output and agent evidence match the transition condition.
        assert_eq!(result.final_state, "done");
        assert!(result.advanced);
        assert_eq!(result.stop_reason, StopReason::Terminal);
    }

    /// scenario-11: any_failed is derived from GateOutcome. Passed gates do not
    /// contribute to any_failed; Failed/TimedOut/Error outcomes do.
    #[test]
    fn gate_pass_fail_from_outcome() {
        use crate::template::types::Gate;

        // State has a gate and an unconditional fallback. When the gate passes,
        // the engine auto-advances via the unconditional fallback (gate_failed=false).
        // When the gate fails, the unconditional fallback is suppressed and the
        // engine returns GateBlocked.
        let mut gates = BTreeMap::new();
        gates.insert(
            "check".to_string(),
            Gate {
                gate_type: "command".to_string(),
                command: "exit 0".to_string(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
            },
        );

        let template = make_template(vec![
            (
                "guarded",
                TemplateState {
                    directive: "Guarded.".to_string(),
                    details: String::new(),
                    transitions: vec![unconditional("next")],
                    terminal: false,
                    gates,
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
            (
                "next",
                TemplateState {
                    directive: "Next.".to_string(),
                    details: String::new(),
                    transitions: vec![],
                    terminal: true,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
        ]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        // Passing gate: outcome Passed -- any_failed should be false.
        let passing_eval = |gates: &BTreeMap<String, crate::template::types::Gate>| {
            let mut results = BTreeMap::new();
            for (name, _) in gates {
                results.insert(
                    name.clone(),
                    StructuredGateResult {
                        outcome: GateOutcome::Passed,
                        output: serde_json::json!({"exit_code": 0, "error": ""}),
                    },
                );
            }
            results
        };

        let result = advance_until_stop(
            "guarded",
            &template,
            &BTreeMap::new(),
            &[],
            &mut append,
            &passing_eval,
            &unavailable_integration,
            &noop_action,
            &shutdown,
        )
        .unwrap();

        // Gate passed: engine should auto-advance via unconditional to "next".
        assert_eq!(result.final_state, "next");
        assert!(result.advanced);
        assert_eq!(result.stop_reason, StopReason::Terminal);

        // Failing gate: outcome Failed -- any_failed should be true.
        let failing_eval = |gates: &BTreeMap<String, crate::template::types::Gate>| {
            let mut results = BTreeMap::new();
            for (name, _) in gates {
                results.insert(
                    name.clone(),
                    StructuredGateResult {
                        outcome: GateOutcome::Failed,
                        output: serde_json::json!({"exit_code": 1, "error": ""}),
                    },
                );
            }
            results
        };

        let mut append2 = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown2 = AtomicBool::new(false);

        let result2 = advance_until_stop(
            "guarded",
            &template,
            &BTreeMap::new(),
            &[],
            &mut append2,
            &failing_eval,
            &unavailable_integration,
            &noop_action,
            &shutdown2,
        )
        .unwrap();

        // Gate failed: engine should return GateBlocked (no accepts block).
        assert_eq!(result2.final_state, "guarded");
        assert!(!result2.advanced);
        assert!(matches!(result2.stop_reason, StopReason::GateBlocked(_)));

        // TimedOut gate: outcome TimedOut also contributes to any_failed.
        let timeout_eval = |gates: &BTreeMap<String, crate::template::types::Gate>| {
            let mut results = BTreeMap::new();
            for (name, _) in gates {
                results.insert(
                    name.clone(),
                    StructuredGateResult {
                        outcome: GateOutcome::TimedOut,
                        output: serde_json::json!({"exit_code": -1, "error": "timed_out"}),
                    },
                );
            }
            results
        };

        let mut append3 = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown3 = AtomicBool::new(false);

        let result3 = advance_until_stop(
            "guarded",
            &template,
            &BTreeMap::new(),
            &[],
            &mut append3,
            &timeout_eval,
            &unavailable_integration,
            &noop_action,
            &shutdown3,
        )
        .unwrap();

        assert!(matches!(result3.stop_reason, StopReason::GateBlocked(_)));

        // Error gate: outcome Error also contributes to any_failed.
        let error_eval = |gates: &BTreeMap<String, crate::template::types::Gate>| {
            let mut results = BTreeMap::new();
            for (name, _) in gates {
                results.insert(
                    name.clone(),
                    StructuredGateResult {
                        outcome: GateOutcome::Error,
                        output: serde_json::json!({"exit_code": -1, "error": "spawn failed"}),
                    },
                );
            }
            results
        };

        let mut append4 = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown4 = AtomicBool::new(false);

        let result4 = advance_until_stop(
            "guarded",
            &template,
            &BTreeMap::new(),
            &[],
            &mut append4,
            &error_eval,
            &unavailable_integration,
            &noop_action,
            &shutdown4,
        )
        .unwrap();

        assert!(matches!(result4.stop_reason, StopReason::GateBlocked(_)));
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
                    details: String::new(),
                    transitions: vec![unconditional("b")],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
            (
                "b",
                TemplateState {
                    directive: "B.".to_string(),
                    details: String::new(),
                    transitions: vec![unconditional("a")],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
        ]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        let result = advance_until_stop(
            "a",
            &template,
            &BTreeMap::new(),
            &[],
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &noop_action,
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
                details: String::new(),
                transitions: vec![unconditional("next")],
                terminal: false,
                gates: BTreeMap::new(),
                accepts: None,
                integration: Some("my-runner".to_string()),
                default_action: None,
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
            &[],
            &mut append,
            &noop_gates,
            &integration,
            &noop_action,
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
                    details: String::new(),
                    transitions: vec![unconditional(names[i + 1])],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ));
        }
        // Terminal state at the end
        states.push((
            *names.last().unwrap(),
            TemplateState {
                directive: "Final.".to_string(),
                details: String::new(),
                transitions: vec![],
                terminal: true,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
            },
        ));

        let template = make_template(states);
        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        let result = advance_until_stop(
            "s0",
            &template,
            &BTreeMap::new(),
            &[],
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &noop_action,
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
                details: String::new(),
                transitions: vec![],
                terminal: true,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
            },
        )]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        let result = advance_until_stop(
            "done",
            &template,
            &BTreeMap::new(),
            &[],
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &noop_action,
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
                    details: String::new(),
                    transitions: vec![unconditional("b")],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
            (
                "b",
                TemplateState {
                    directive: "B.".to_string(),
                    details: String::new(),
                    transitions: vec![unconditional("c")],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
            (
                "c",
                TemplateState {
                    directive: "C.".to_string(),
                    details: String::new(),
                    transitions: vec![],
                    terminal: true,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
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
            &[],
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &noop_action,
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
                    details: String::new(),
                    transitions: vec![conditional("middle", vec![("go", serde_json::json!(true))])],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
            (
                "middle",
                TemplateState {
                    directive: "Middle.".to_string(),
                    details: String::new(),
                    transitions: vec![conditional("end", vec![("go", serde_json::json!(true))])],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
            (
                "end",
                TemplateState {
                    directive: "End.".to_string(),
                    details: String::new(),
                    transitions: vec![],
                    terminal: true,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
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
            &[],
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &noop_action,
            &shutdown,
        )
        .unwrap();

        // Should stop at "middle" because evidence is cleared after auto-advance.
        // "middle" has conditionals but no accepts block, so the engine returns
        // UnresolvableTransition (not EvidenceRequired).
        assert_eq!(result.final_state, "middle");
        assert!(result.advanced);
        assert_eq!(result.stop_reason, StopReason::UnresolvableTransition);
    }

    // -----------------------------------------------------------------------
    // action closure tests
    // -----------------------------------------------------------------------

    fn make_action_decl(command: &str) -> ActionDecl {
        ActionDecl {
            command: command.to_string(),
            working_dir: String::new(),
            requires_confirmation: false,
            polling: None,
        }
    }

    #[test]
    fn action_closure_called_when_state_has_default_action() {
        use std::sync::atomic::AtomicUsize;

        let call_count = AtomicUsize::new(0);

        let template = make_template(vec![(
            "act",
            TemplateState {
                directive: "Act.".to_string(),
                details: String::new(),
                transitions: vec![conditional(
                    "done",
                    vec![("result", serde_json::json!("ok"))],
                )],
                terminal: false,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: Some(make_action_decl("echo hello")),
            },
        )]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        let action = |_state: &str, _action: &ActionDecl, _has_evidence: bool| -> ActionResult {
            call_count.fetch_add(1, Ordering::Relaxed);
            ActionResult::Executed {
                exit_code: 0,
                stdout: "hello".to_string(),
                stderr: String::new(),
            }
        };

        let _result = advance_until_stop(
            "act",
            &template,
            &BTreeMap::new(),
            &[],
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &action,
            &shutdown,
        )
        .unwrap();

        assert_eq!(call_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn action_closure_not_called_when_no_default_action() {
        use std::sync::atomic::AtomicUsize;

        let call_count = AtomicUsize::new(0);

        let template = make_template(vec![(
            "plain",
            TemplateState {
                directive: "Plain.".to_string(),
                details: String::new(),
                transitions: vec![conditional(
                    "done",
                    vec![("result", serde_json::json!("ok"))],
                )],
                terminal: false,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: None,
            },
        )]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        let action = |_state: &str, _action: &ActionDecl, _has_evidence: bool| -> ActionResult {
            call_count.fetch_add(1, Ordering::Relaxed);
            ActionResult::Skipped
        };

        let _result = advance_until_stop(
            "plain",
            &template,
            &BTreeMap::new(),
            &[],
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &action,
            &shutdown,
        )
        .unwrap();

        assert_eq!(call_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn action_requires_confirmation_stops_loop() {
        let template = make_template(vec![(
            "confirm",
            TemplateState {
                directive: "Confirm.".to_string(),
                details: String::new(),
                transitions: vec![unconditional("next")],
                terminal: false,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: Some(make_action_decl("create-pr")),
            },
        )]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        let action = |_state: &str, _action: &ActionDecl, _has_evidence: bool| -> ActionResult {
            ActionResult::RequiresConfirmation {
                exit_code: 0,
                stdout: "PR #42 created".to_string(),
                stderr: String::new(),
            }
        };

        let result = advance_until_stop(
            "confirm",
            &template,
            &BTreeMap::new(),
            &[],
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &action,
            &shutdown,
        )
        .unwrap();

        assert_eq!(result.final_state, "confirm");
        assert!(!result.advanced);
        match &result.stop_reason {
            StopReason::ActionRequiresConfirmation {
                state,
                exit_code,
                stdout,
                ..
            } => {
                assert_eq!(state, "confirm");
                assert_eq!(*exit_code, 0);
                assert_eq!(stdout, "PR #42 created");
            }
            other => panic!("expected ActionRequiresConfirmation, got {:?}", other),
        }
    }

    #[test]
    fn action_skipped_continues_to_gate_evaluation() {
        use crate::template::types::Gate;

        let mut gates = BTreeMap::new();
        gates.insert(
            "check".to_string(),
            Gate {
                gate_type: "command".to_string(),
                command: "false".to_string(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
            },
        );

        let template = make_template(vec![(
            "gated_action",
            TemplateState {
                directive: "Gated action.".to_string(),
                details: String::new(),
                transitions: vec![unconditional("next")],
                terminal: false,
                gates,
                accepts: None,
                integration: None,
                default_action: Some(make_action_decl("echo skip-me")),
            },
        )]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        // Action returns Skipped; gate blocks
        let action = |_state: &str, _action: &ActionDecl, _has_evidence: bool| -> ActionResult {
            ActionResult::Skipped
        };

        let gate_eval = |gates: &BTreeMap<String, crate::template::types::Gate>| {
            let mut results = BTreeMap::new();
            for (name, _) in gates {
                results.insert(
                    name.clone(),
                    StructuredGateResult {
                        outcome: GateOutcome::Failed,
                        output: serde_json::json!({"exit_code": 1, "error": ""}),
                    },
                );
            }
            results
        };

        let result = advance_until_stop(
            "gated_action",
            &template,
            &BTreeMap::new(),
            &[],
            &mut append,
            &gate_eval,
            &unavailable_integration,
            &action,
            &shutdown,
        )
        .unwrap();

        // Action was skipped, but gate blocked
        assert_eq!(result.final_state, "gated_action");
        assert!(!result.advanced);
        assert!(matches!(result.stop_reason, StopReason::GateBlocked(_)));
    }

    #[test]
    fn action_executed_continues_to_gate_evaluation() {
        let template = make_template(vec![
            (
                "act",
                TemplateState {
                    directive: "Act.".to_string(),
                    details: String::new(),
                    transitions: vec![unconditional("done")],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: Some(make_action_decl("echo ok")),
                },
            ),
            (
                "done",
                TemplateState {
                    directive: "Done.".to_string(),
                    details: String::new(),
                    transitions: vec![],
                    terminal: true,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
        ]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        let action = |_state: &str, _action: &ActionDecl, _has_evidence: bool| -> ActionResult {
            ActionResult::Executed {
                exit_code: 0,
                stdout: "ok".to_string(),
                stderr: String::new(),
            }
        };

        let result = advance_until_stop(
            "act",
            &template,
            &BTreeMap::new(),
            &[],
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &action,
            &shutdown,
        )
        .unwrap();

        // Action executed, gates passed, transitioned to terminal
        assert_eq!(result.final_state, "done");
        assert!(result.advanced);
        assert_eq!(result.stop_reason, StopReason::Terminal);
    }

    #[test]
    fn action_closure_receives_true_when_evidence_exists() {
        use std::sync::atomic::AtomicBool as AB;

        let received_has_evidence = AB::new(false);

        let template = make_template(vec![
            (
                "check",
                TemplateState {
                    directive: "Check.".to_string(),
                    details: String::new(),
                    transitions: vec![conditional(
                        "done",
                        vec![("result", serde_json::json!("ok"))],
                    )],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: Some(make_action_decl("echo check")),
                },
            ),
            (
                "done",
                TemplateState {
                    directive: "Done.".to_string(),
                    details: String::new(),
                    transitions: vec![],
                    terminal: true,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
        ]);

        let mut evidence = BTreeMap::new();
        evidence.insert("result".to_string(), serde_json::json!("ok"));

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        let action = |_state: &str, _action: &ActionDecl, has_evidence: bool| -> ActionResult {
            received_has_evidence.store(has_evidence, Ordering::Relaxed);
            ActionResult::Skipped
        };

        let _result = advance_until_stop(
            "check",
            &template,
            &evidence,
            &[],
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &action,
            &shutdown,
        )
        .unwrap();

        assert!(received_has_evidence.load(Ordering::Relaxed));
    }

    // -----------------------------------------------------------------------
    // Gate override pre-check tests
    // -----------------------------------------------------------------------

    fn make_event(seq: u64, payload: EventPayload) -> Event {
        Event {
            seq,
            timestamp: "2026-04-01T00:00:00Z".to_string(),
            event_type: payload.type_name().to_string(),
            payload,
        }
    }

    fn make_gate_def(gate_type: &str) -> crate::template::types::Gate {
        crate::template::types::Gate {
            gate_type: gate_type.to_string(),
            command: String::new(),
            timeout: 0,
            key: String::new(),
            pattern: String::new(),
            override_default: None,
        }
    }

    /// Build a minimal event log that puts `derive_overrides` in the correct epoch:
    /// a Transitioned event to `state` followed by the given GateOverrideRecorded events.
    fn override_events(state: &str, gate: &str, override_applied: serde_json::Value) -> Vec<Event> {
        vec![
            make_event(
                1,
                EventPayload::Transitioned {
                    from: None,
                    to: state.to_string(),
                    condition_type: "auto".to_string(),
                },
            ),
            make_event(
                2,
                EventPayload::GateOverrideRecorded {
                    state: state.to_string(),
                    gate: gate.to_string(),
                    rationale: "test override".to_string(),
                    override_applied,
                    actual_output: serde_json::json!({"exit_code": 1, "error": ""}),
                    timestamp: "2026-04-01T00:00:00Z".to_string(),
                },
            ),
        ]
    }

    // Test 1: one gate with an active GateOverrideRecorded; assert the gate appears
    // in gate_evidence_map with override_applied, gate_results shows Passed, and
    // no GateEvaluated event is emitted for the overridden gate.
    #[test]
    fn override_injects_passed_result_and_no_gate_evaluated_event() {
        use crate::template::types::Gate;

        let override_val = serde_json::json!({"exit_code": 0, "error": ""});
        let all_events = override_events("gated", "ci", override_val.clone());

        let mut gates = BTreeMap::new();
        gates.insert("ci".to_string(), make_gate_def("command"));

        let template = make_template(vec![
            (
                "gated",
                TemplateState {
                    directive: "Gated.".to_string(),
                    details: String::new(),
                    transitions: vec![unconditional("done")],
                    terminal: false,
                    gates,
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
            (
                "done",
                TemplateState {
                    directive: "Done.".to_string(),
                    details: String::new(),
                    transitions: vec![],
                    terminal: true,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
        ]);

        let mut appended: Vec<EventPayload> = Vec::new();
        let mut append = |payload: &EventPayload| -> Result<(), String> {
            appended.push(payload.clone());
            Ok(())
        };
        let shutdown = AtomicBool::new(false);

        // evaluate_gates should never be called for the overridden gate.
        let gate_eval = |_gates: &BTreeMap<String, Gate>| {
            // If called, this is a test failure (overridden gate should skip evaluate_gates).
            BTreeMap::new()
        };

        let result = advance_until_stop(
            "gated",
            &template,
            &BTreeMap::new(),
            &all_events,
            &mut append,
            &gate_eval,
            &unavailable_integration,
            &noop_action,
            &shutdown,
        )
        .unwrap();

        // The override causes the gate to pass, so the loop advances to "done" (terminal).
        assert_eq!(result.final_state, "done");
        assert!(result.advanced);
        assert!(matches!(result.stop_reason, StopReason::Terminal));

        // GateEvaluated event must NOT appear for the overridden gate "ci".
        let gate_evaluated_for_ci = appended
            .iter()
            .any(|p| matches!(p, EventPayload::GateEvaluated { gate, .. } if gate == "ci"));
        assert!(
            !gate_evaluated_for_ci,
            "GateEvaluated must not be emitted for an overridden gate"
        );
    }

    // Test 2: two gates, one overridden one not. The non-overridden gate fails.
    // Verify: overridden gate has Passed (no GateEvaluated), non-overridden gate
    // produces GateEvaluated, and any_failed reflects only the non-overridden gate.
    #[test]
    fn partial_override_only_non_overridden_gate_contributes_to_failure() {
        use crate::template::types::Gate;

        let override_val = serde_json::json!({"exit_code": 0, "error": ""});
        let all_events = override_events("review", "ci", override_val.clone());

        let mut gates = BTreeMap::new();
        gates.insert("ci".to_string(), make_gate_def("command"));
        gates.insert("lint".to_string(), make_gate_def("command"));

        let template = make_template(vec![
            (
                "review",
                TemplateState {
                    directive: "Review.".to_string(),
                    details: String::new(),
                    transitions: vec![unconditional("done")],
                    terminal: false,
                    gates,
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
            (
                "done",
                TemplateState {
                    directive: "Done.".to_string(),
                    details: String::new(),
                    transitions: vec![],
                    terminal: true,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
        ]);

        let mut appended: Vec<EventPayload> = Vec::new();
        let mut append = |payload: &EventPayload| -> Result<(), String> {
            appended.push(payload.clone());
            Ok(())
        };
        let shutdown = AtomicBool::new(false);

        // Only "lint" gate is evaluated; "ci" is overridden.
        let gate_eval = |gates: &BTreeMap<String, Gate>| {
            let mut results = BTreeMap::new();
            for (name, _) in gates {
                if name == "lint" {
                    results.insert(
                        name.clone(),
                        StructuredGateResult {
                            outcome: GateOutcome::Failed,
                            output: serde_json::json!({"exit_code": 1, "error": ""}),
                        },
                    );
                }
            }
            results
        };

        let result = advance_until_stop(
            "review",
            &template,
            &BTreeMap::new(),
            &all_events,
            &mut append,
            &gate_eval,
            &unavailable_integration,
            &noop_action,
            &shutdown,
        )
        .unwrap();

        // The "lint" gate failed, so the loop is blocked.
        assert_eq!(result.final_state, "review");
        assert!(matches!(result.stop_reason, StopReason::GateBlocked(_)));

        if let StopReason::GateBlocked(gate_results) = &result.stop_reason {
            // Overridden gate "ci" appears with Passed outcome (override injected).
            assert!(
                gate_results
                    .get("ci")
                    .map(|r| r.outcome == GateOutcome::Passed)
                    .unwrap_or(false),
                "ci gate was overridden; must appear with Passed outcome in gate_results"
            );
            // Non-overridden "lint" gate should be failed.
            assert_eq!(
                gate_results["lint"].outcome,
                GateOutcome::Failed,
                "lint gate should have Failed outcome"
            );
        }

        // GateEvaluated should exist for "lint" but NOT for "ci".
        let evaluated_ci = appended
            .iter()
            .any(|p| matches!(p, EventPayload::GateEvaluated { gate, .. } if gate == "ci"));
        let evaluated_lint = appended
            .iter()
            .any(|p| matches!(p, EventPayload::GateEvaluated { gate, .. } if gate == "lint"));
        assert!(
            !evaluated_ci,
            "GateEvaluated must not be emitted for overridden gate 'ci'"
        );
        assert!(
            evaluated_lint,
            "GateEvaluated must be emitted for non-overridden gate 'lint'"
        );
    }

    // Test 3: one command gate, no active override, evaluation returns non-passing.
    // The blocking condition in GateBlocked must have agent_actionable checked
    // via blocking_conditions_from_gates (tested in next_types.rs). This test
    // verifies the advance loop produces a GateBlocked stop reason for a failing gate.
    #[test]
    fn failing_command_gate_without_override_produces_gate_blocked() {
        use crate::template::types::Gate;

        let mut gates = BTreeMap::new();
        gates.insert(
            "build".to_string(),
            Gate {
                gate_type: "command".to_string(),
                command: String::new(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
            },
        );

        let template = make_template(vec![
            (
                "build-state",
                TemplateState {
                    directive: "Build.".to_string(),
                    details: String::new(),
                    transitions: vec![unconditional("done")],
                    terminal: false,
                    gates,
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
            (
                "done",
                TemplateState {
                    directive: "Done.".to_string(),
                    details: String::new(),
                    transitions: vec![],
                    terminal: true,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
        ]);

        // No overrides in the event log.
        let all_events: Vec<Event> = vec![make_event(
            1,
            EventPayload::Transitioned {
                from: None,
                to: "build-state".to_string(),
                condition_type: "auto".to_string(),
            },
        )];

        let mut appended: Vec<EventPayload> = Vec::new();
        let mut append = |payload: &EventPayload| -> Result<(), String> {
            appended.push(payload.clone());
            Ok(())
        };
        let shutdown = AtomicBool::new(false);

        let gate_eval = |gates: &BTreeMap<String, Gate>| {
            let mut results = BTreeMap::new();
            for (name, _) in gates {
                results.insert(
                    name.clone(),
                    StructuredGateResult {
                        outcome: GateOutcome::Failed,
                        output: serde_json::json!({"exit_code": 1, "error": ""}),
                    },
                );
            }
            results
        };

        let result = advance_until_stop(
            "build-state",
            &template,
            &BTreeMap::new(),
            &all_events,
            &mut append,
            &gate_eval,
            &unavailable_integration,
            &noop_action,
            &shutdown,
        )
        .unwrap();

        // Gate blocked.
        assert_eq!(result.final_state, "build-state");
        assert!(matches!(result.stop_reason, StopReason::GateBlocked(_)));

        // GateEvaluated event was emitted for "build".
        let evaluated_build = appended.iter().any(|p| {
            matches!(p, EventPayload::GateEvaluated { gate, outcome, .. }
                if gate == "build" && outcome == "failed")
        });
        assert!(
            evaluated_build,
            "GateEvaluated must be emitted for non-overridden failing gate"
        );
    }

    // Test 4: one gate with active override. The loop must advance past the gate
    // (blocking_conditions empty, status is not gate_blocked).
    #[test]
    fn active_override_causes_gate_to_pass_and_loop_advances() {
        use crate::template::types::Gate;

        let override_val = serde_json::json!({"exit_code": 0, "error": ""});
        let all_events = override_events("blocked", "ci", override_val.clone());

        let mut gates = BTreeMap::new();
        gates.insert(
            "ci".to_string(),
            Gate {
                gate_type: "command".to_string(),
                command: String::new(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
            },
        );

        let template = make_template(vec![
            (
                "blocked",
                TemplateState {
                    directive: "Blocked.".to_string(),
                    details: String::new(),
                    transitions: vec![unconditional("done")],
                    terminal: false,
                    gates,
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
            (
                "done",
                TemplateState {
                    directive: "Done.".to_string(),
                    details: String::new(),
                    transitions: vec![],
                    terminal: true,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
        ]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        // evaluate_gates would make the gate fail if called, but it shouldn't be.
        let gate_eval = |gates: &BTreeMap<String, Gate>| {
            let mut results = BTreeMap::new();
            for (name, _) in gates {
                results.insert(
                    name.clone(),
                    StructuredGateResult {
                        outcome: GateOutcome::Failed,
                        output: serde_json::json!({"exit_code": 1, "error": ""}),
                    },
                );
            }
            results
        };

        let result = advance_until_stop(
            "blocked",
            &template,
            &BTreeMap::new(),
            &all_events,
            &mut append,
            &gate_eval,
            &unavailable_integration,
            &noop_action,
            &shutdown,
        )
        .unwrap();

        // The override caused the gate to pass; the loop advanced to terminal state.
        assert_eq!(result.final_state, "done");
        assert!(result.advanced);
        assert!(matches!(result.stop_reason, StopReason::Terminal));
        // Status is NOT gate_blocked.
        assert!(!matches!(result.stop_reason, StopReason::GateBlocked(_)));
    }

    // Test 5: one gate with unknown type and no override_default; evaluation fails.
    // Verifies that the GateEvaluated event is still emitted for unknown-type gates
    // (no override path), and the loop produces GateBlocked.
    #[test]
    fn unknown_gate_type_no_override_default_produces_gate_blocked() {
        use crate::template::types::Gate;

        let mut gates = BTreeMap::new();
        gates.insert(
            "custom-check".to_string(),
            Gate {
                gate_type: "custom-unknown".to_string(),
                command: String::new(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
            },
        );

        let template = make_template(vec![
            (
                "check-state",
                TemplateState {
                    directive: "Check.".to_string(),
                    details: String::new(),
                    transitions: vec![unconditional("done")],
                    terminal: false,
                    gates,
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
            (
                "done",
                TemplateState {
                    directive: "Done.".to_string(),
                    details: String::new(),
                    transitions: vec![],
                    terminal: true,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                },
            ),
        ]);

        let all_events: Vec<Event> = vec![make_event(
            1,
            EventPayload::Transitioned {
                from: None,
                to: "check-state".to_string(),
                condition_type: "auto".to_string(),
            },
        )];

        let mut appended: Vec<EventPayload> = Vec::new();
        let mut append = |payload: &EventPayload| -> Result<(), String> {
            appended.push(payload.clone());
            Ok(())
        };
        let shutdown = AtomicBool::new(false);

        // Gate evaluator returns an Error outcome for the unknown type.
        let gate_eval = |gates: &BTreeMap<String, Gate>| {
            let mut results = BTreeMap::new();
            for (name, _) in gates {
                results.insert(
                    name.clone(),
                    StructuredGateResult {
                        outcome: GateOutcome::Error,
                        output: serde_json::json!({"exit_code": -1, "error": "unsupported gate type"}),
                    },
                );
            }
            results
        };

        let result = advance_until_stop(
            "check-state",
            &template,
            &BTreeMap::new(),
            &all_events,
            &mut append,
            &gate_eval,
            &unavailable_integration,
            &noop_action,
            &shutdown,
        )
        .unwrap();

        // Gate blocked.
        assert_eq!(result.final_state, "check-state");
        assert!(matches!(result.stop_reason, StopReason::GateBlocked(_)));

        // GateEvaluated was emitted (no override; the gate was actually evaluated).
        let evaluated = appended.iter().any(
            |p| matches!(p, EventPayload::GateEvaluated { gate, .. } if gate == "custom-check"),
        );
        assert!(
            evaluated,
            "GateEvaluated must be emitted for non-overridden gate with unknown type"
        );
    }
}
