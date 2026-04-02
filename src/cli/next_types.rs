use std::collections::BTreeMap;

use serde::ser::SerializeMap;
use serde::Serialize;

use crate::gate::{GateOutcome, StructuredGateResult};
use crate::template::types::{Gate, TemplateState};

/// Summary of a recorded decision, used in `koto decisions list` responses.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DecisionSummary {
    pub choice: String,
    pub rationale: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alternatives_considered: Option<Vec<String>>,
}

/// The six possible responses from `koto next`.
///
/// Each variant maps 1:1 to a JSON output shape. Custom `Serialize`
/// writes the correct fields per variant, including `action` and
/// `error: null`. Fields marked "no" in the design's field presence
/// table are absent; fields marked "null" serialize as `null`.
#[derive(Debug, Clone, PartialEq)]
pub enum NextResponse {
    EvidenceRequired {
        state: String,
        directive: String,
        details: Option<String>,
        advanced: bool,
        expects: ExpectsSchema,
        blocking_conditions: Vec<BlockingCondition>,
    },
    GateBlocked {
        state: String,
        directive: String,
        details: Option<String>,
        advanced: bool,
        blocking_conditions: Vec<BlockingCondition>,
    },
    Integration {
        state: String,
        directive: String,
        details: Option<String>,
        advanced: bool,
        expects: Option<ExpectsSchema>,
        integration: IntegrationOutput,
    },
    IntegrationUnavailable {
        state: String,
        directive: String,
        details: Option<String>,
        advanced: bool,
        expects: Option<ExpectsSchema>,
        integration: IntegrationUnavailableMarker,
    },
    Terminal {
        state: String,
        advanced: bool,
    },
    ActionRequiresConfirmation {
        state: String,
        directive: String,
        details: Option<String>,
        advanced: bool,
        action_output: ActionOutput,
        expects: Option<ExpectsSchema>,
    },
}

impl NextResponse {
    /// Return a new `NextResponse` with the directive and details fields substituted
    /// using the given function. Terminal variants have no directive and are returned
    /// unchanged.
    pub fn with_substituted_directive<F>(self, f: F) -> Self
    where
        F: Fn(&str) -> String,
    {
        match self {
            NextResponse::EvidenceRequired {
                state,
                directive,
                details,
                advanced,
                expects,
                blocking_conditions,
            } => NextResponse::EvidenceRequired {
                state,
                directive: f(&directive),
                details: details.map(|d| f(&d)),
                advanced,
                expects,
                blocking_conditions,
            },
            NextResponse::GateBlocked {
                state,
                directive,
                details,
                advanced,
                blocking_conditions,
            } => NextResponse::GateBlocked {
                state,
                directive: f(&directive),
                details: details.map(|d| f(&d)),
                advanced,
                blocking_conditions,
            },
            NextResponse::Integration {
                state,
                directive,
                details,
                advanced,
                expects,
                integration,
            } => NextResponse::Integration {
                state,
                directive: f(&directive),
                details: details.map(|d| f(&d)),
                advanced,
                expects,
                integration,
            },
            NextResponse::IntegrationUnavailable {
                state,
                directive,
                details,
                advanced,
                expects,
                integration,
            } => NextResponse::IntegrationUnavailable {
                state,
                directive: f(&directive),
                details: details.map(|d| f(&d)),
                advanced,
                expects,
                integration,
            },
            terminal @ NextResponse::Terminal { .. } => terminal,
            NextResponse::ActionRequiresConfirmation {
                state,
                directive,
                details,
                advanced,
                action_output,
                expects,
            } => NextResponse::ActionRequiresConfirmation {
                state,
                directive: f(&directive),
                details: details.map(|d| f(&d)),
                advanced,
                action_output,
                expects,
            },
        }
    }
}

impl Serialize for NextResponse {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            NextResponse::EvidenceRequired {
                state,
                directive,
                details,
                advanced,
                expects,
                blocking_conditions,
            } => {
                let count = 7 + details.as_ref().map_or(0, |_| 1);
                let mut map = serializer.serialize_map(Some(count))?;
                map.serialize_entry("action", "evidence_required")?;
                map.serialize_entry("state", state)?;
                map.serialize_entry("directive", directive)?;
                if let Some(d) = details {
                    map.serialize_entry("details", d)?;
                }
                map.serialize_entry("advanced", advanced)?;
                map.serialize_entry("expects", expects)?;
                map.serialize_entry("blocking_conditions", blocking_conditions)?;
                map.serialize_entry("error", &None::<()>)?;
                map.end()
            }
            NextResponse::GateBlocked {
                state,
                directive,
                details,
                advanced,
                blocking_conditions,
            } => {
                let count = 7 + details.as_ref().map_or(0, |_| 1);
                let mut map = serializer.serialize_map(Some(count))?;
                map.serialize_entry("action", "gate_blocked")?;
                map.serialize_entry("state", state)?;
                map.serialize_entry("directive", directive)?;
                if let Some(d) = details {
                    map.serialize_entry("details", d)?;
                }
                map.serialize_entry("advanced", advanced)?;
                map.serialize_entry("expects", &None::<()>)?;
                map.serialize_entry("blocking_conditions", blocking_conditions)?;
                map.serialize_entry("error", &None::<()>)?;
                map.end()
            }
            NextResponse::Integration {
                state,
                directive,
                details,
                advanced,
                expects,
                integration,
            } => {
                let count = 7 + details.as_ref().map_or(0, |_| 1);
                let mut map = serializer.serialize_map(Some(count))?;
                map.serialize_entry("action", "integration")?;
                map.serialize_entry("state", state)?;
                map.serialize_entry("directive", directive)?;
                if let Some(d) = details {
                    map.serialize_entry("details", d)?;
                }
                map.serialize_entry("advanced", advanced)?;
                map.serialize_entry("expects", expects)?;
                map.serialize_entry("integration", integration)?;
                map.serialize_entry("error", &None::<()>)?;
                map.end()
            }
            NextResponse::IntegrationUnavailable {
                state,
                directive,
                details,
                advanced,
                expects,
                integration,
            } => {
                let count = 7 + details.as_ref().map_or(0, |_| 1);
                let mut map = serializer.serialize_map(Some(count))?;
                map.serialize_entry("action", "integration_unavailable")?;
                map.serialize_entry("state", state)?;
                map.serialize_entry("directive", directive)?;
                if let Some(d) = details {
                    map.serialize_entry("details", d)?;
                }
                map.serialize_entry("advanced", advanced)?;
                map.serialize_entry("expects", expects)?;
                map.serialize_entry("integration", integration)?;
                map.serialize_entry("error", &None::<()>)?;
                map.end()
            }
            NextResponse::Terminal { state, advanced } => {
                let mut map = serializer.serialize_map(Some(5))?;
                map.serialize_entry("action", "done")?;
                map.serialize_entry("state", state)?;
                map.serialize_entry("advanced", advanced)?;
                map.serialize_entry("expects", &None::<()>)?;
                map.serialize_entry("error", &None::<()>)?;
                map.end()
            }
            NextResponse::ActionRequiresConfirmation {
                state,
                directive,
                details,
                advanced,
                action_output,
                expects,
            } => {
                let count = 7 + details.as_ref().map_or(0, |_| 1);
                let mut map = serializer.serialize_map(Some(count))?;
                map.serialize_entry("action", "confirm")?;
                map.serialize_entry("state", state)?;
                map.serialize_entry("directive", directive)?;
                if let Some(d) = details {
                    map.serialize_entry("details", d)?;
                }
                map.serialize_entry("advanced", advanced)?;
                map.serialize_entry("action_output", action_output)?;
                map.serialize_entry("expects", expects)?;
                map.serialize_entry("error", &None::<()>)?;
                map.end()
            }
        }
    }
}

/// Structured error returned by the dispatcher for domain-level failures.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NextError {
    pub code: NextErrorCode,
    pub message: String,
    pub details: Vec<ErrorDetail>,
}

/// The nine error codes for `koto next` domain errors.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NextErrorCode {
    GateBlocked,
    InvalidSubmission,
    PreconditionFailed,
    IntegrationUnavailable,
    TerminalState,
    WorkflowNotInitialized,
    TemplateError,
    PersistenceError,
    ConcurrentAccess,
}

impl NextErrorCode {
    /// Map each error code to its process exit code.
    ///
    /// Exit code 1 = transient (may resolve on retry).
    /// Exit code 2 = caller error (agent must change behavior).
    /// Exit code 3 = infrastructure / config errors.
    pub fn exit_code(&self) -> i32 {
        match self {
            NextErrorCode::GateBlocked => 1,
            NextErrorCode::IntegrationUnavailable => 1,
            NextErrorCode::ConcurrentAccess => 1,
            NextErrorCode::InvalidSubmission => 2,
            NextErrorCode::PreconditionFailed => 2,
            NextErrorCode::TerminalState => 2,
            NextErrorCode::WorkflowNotInitialized => 2,
            NextErrorCode::TemplateError => 3,
            NextErrorCode::PersistenceError => 3,
        }
    }
}

/// Schema describing what evidence the agent should submit.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExpectsSchema {
    pub event_type: String,
    pub fields: BTreeMap<String, ExpectsFieldSchema>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<TransitionOption>,
}

/// Schema for a single evidence field.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExpectsFieldSchema {
    #[serde(rename = "type")]
    pub field_type: String,
    pub required: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
}

/// A transition option surfaced to the agent.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TransitionOption {
    pub target: String,
    pub when: BTreeMap<String, serde_json::Value>,
}

/// A condition blocking state advancement.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BlockingCondition {
    pub name: String,
    #[serde(rename = "type")]
    pub condition_type: String,
    pub status: String,
    // False until Feature 2 (override mechanism) lands. Feature 2 sets this
    // true when the gate has an override_default, signaling the agent can call
    // `koto overrides record` to substitute gate output with the default.
    pub agent_actionable: bool,
    pub output: serde_json::Value,
}

/// Output from a default action that requires confirmation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActionOutput {
    pub command: String,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Output from an integration that ran successfully.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IntegrationOutput {
    pub name: String,
    pub output: serde_json::Value,
}

/// Marker indicating an integration is declared but unavailable.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IntegrationUnavailableMarker {
    pub name: String,
    pub available: bool,
}

/// Per-field detail in an error response.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ErrorDetail {
    pub field: String,
    pub reason: String,
}

/// Convert gate evaluation results into a list of blocking conditions.
///
/// Passed gates are excluded. Each non-passing gate produces a `BlockingCondition`
/// with `condition_type` taken from the gate definition (falling back to `"command"`
/// when the gate name is not found in `gate_defs`), and `output` from the structured
/// gate result.
pub fn blocking_conditions_from_gates(
    gate_results: &BTreeMap<String, StructuredGateResult>,
    gate_defs: &BTreeMap<String, Gate>,
) -> Vec<BlockingCondition> {
    gate_results
        .iter()
        .filter_map(|(name, result)| {
            let status = match result.outcome {
                GateOutcome::Passed => return None,
                GateOutcome::Failed => "failed",
                GateOutcome::TimedOut => "timed_out",
                GateOutcome::Error => "error",
            };
            let condition_type = gate_defs
                .get(name)
                .map(|g| g.gate_type.clone())
                .unwrap_or_else(|| "command".to_string());
            Some(BlockingCondition {
                name: name.clone(),
                condition_type,
                status: status.to_string(),
                agent_actionable: false,
                output: result.output.clone(),
            })
        })
        .collect()
}

/// Derive an `ExpectsSchema` from a template state's `accepts` block and transitions.
///
/// Returns `None` when the state has no `accepts` block. When present, maps each
/// `FieldSchema` to `ExpectsFieldSchema` and populates `options` from transitions
/// that have `when` conditions. Options are omitted entirely when no transitions
/// have `when`.
pub fn derive_expects(state: &TemplateState) -> Option<ExpectsSchema> {
    let accepts = state.accepts.as_ref()?;

    let fields: BTreeMap<String, ExpectsFieldSchema> = accepts
        .iter()
        .map(|(name, schema)| {
            (
                name.clone(),
                ExpectsFieldSchema {
                    field_type: schema.field_type.clone(),
                    required: schema.required,
                    values: schema.values.clone(),
                },
            )
        })
        .collect();

    let options: Vec<TransitionOption> = state
        .transitions
        .iter()
        .filter_map(|t| {
            t.when.as_ref().map(|when| TransitionOption {
                target: t.target.clone(),
                when: when.clone(),
            })
        })
        .collect();

    Some(ExpectsSchema {
        event_type: "evidence_submitted".to_string(),
        fields,
        options,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- NextResponse variant serialization tests --

    #[test]
    fn serialize_evidence_required() {
        let mut fields = BTreeMap::new();
        fields.insert(
            "decision".to_string(),
            ExpectsFieldSchema {
                field_type: "enum".to_string(),
                required: true,
                values: vec!["proceed".to_string(), "escalate".to_string()],
            },
        );

        let mut when = BTreeMap::new();
        when.insert("decision".to_string(), serde_json::json!("proceed"));

        let resp = NextResponse::EvidenceRequired {
            state: "review".to_string(),
            directive: "Review the code changes.".to_string(),
            details: None,
            advanced: false,
            expects: ExpectsSchema {
                event_type: "evidence_submitted".to_string(),
                fields,
                options: vec![TransitionOption {
                    target: "implement".to_string(),
                    when,
                }],
            },
            blocking_conditions: vec![],
        };

        let json: serde_json::Value = serde_json::to_value(&resp).unwrap();

        assert_eq!(json["action"], "evidence_required");
        assert_eq!(json["state"], "review");
        assert_eq!(json["directive"], "Review the code changes.");
        assert_eq!(json["advanced"], false);
        assert!(json["error"].is_null());
        assert!(json.get("details").is_none());

        // expects present as object
        let expects = &json["expects"];
        assert_eq!(expects["event_type"], "evidence_submitted");
        assert_eq!(expects["fields"]["decision"]["type"], "enum");
        assert_eq!(expects["fields"]["decision"]["required"], true);
        assert_eq!(
            expects["fields"]["decision"]["values"],
            serde_json::json!(["proceed", "escalate"])
        );
        assert_eq!(expects["options"][0]["target"], "implement");
        assert_eq!(
            expects["options"][0]["when"]["decision"],
            serde_json::json!("proceed")
        );

        // blocking_conditions present as empty array
        assert_eq!(json["blocking_conditions"], serde_json::json!([]));
        // integration should be absent
        assert!(json.get("integration").is_none());
    }

    #[test]
    fn serialize_evidence_required_no_options() {
        let mut fields = BTreeMap::new();
        fields.insert(
            "notes".to_string(),
            ExpectsFieldSchema {
                field_type: "string".to_string(),
                required: false,
                values: vec![],
            },
        );

        let resp = NextResponse::EvidenceRequired {
            state: "gather".to_string(),
            directive: "Collect notes.".to_string(),
            details: Some("Extra context here.".to_string()),
            advanced: true,
            expects: ExpectsSchema {
                event_type: "evidence_submitted".to_string(),
                fields,
                options: vec![],
            },
            blocking_conditions: vec![],
        };

        let json: serde_json::Value = serde_json::to_value(&resp).unwrap();

        assert_eq!(json["advanced"], true);
        assert_eq!(json["details"], "Extra context here.");
        // options should be omitted when empty
        assert!(json["expects"].get("options").is_none());
        // values should be omitted when empty
        assert!(json["expects"]["fields"]["notes"].get("values").is_none());
    }

    #[test]
    fn serialize_gate_blocked() {
        let resp = NextResponse::GateBlocked {
            state: "deploy".to_string(),
            directive: "Deploy to staging.".to_string(),
            details: None,
            advanced: false,
            blocking_conditions: vec![
                BlockingCondition {
                    name: "ci_check".to_string(),
                    condition_type: "command".to_string(),
                    status: "failed".to_string(),
                    agent_actionable: false,
                    output: serde_json::json!({"exit_code": 1, "error": ""}),
                },
                BlockingCondition {
                    name: "lint_check".to_string(),
                    condition_type: "command".to_string(),
                    status: "timed_out".to_string(),
                    agent_actionable: false,
                    output: serde_json::json!({"exit_code": -1, "error": "timed_out"}),
                },
            ],
        };

        let json: serde_json::Value = serde_json::to_value(&resp).unwrap();

        assert_eq!(json["action"], "gate_blocked");
        assert_eq!(json["state"], "deploy");
        assert_eq!(json["directive"], "Deploy to staging.");
        assert_eq!(json["advanced"], false);
        assert!(json["error"].is_null());
        assert!(json["expects"].is_null());

        let conditions = json["blocking_conditions"].as_array().unwrap();
        assert_eq!(conditions.len(), 2);
        assert_eq!(conditions[0]["name"], "ci_check");
        assert_eq!(conditions[0]["type"], "command");
        assert_eq!(conditions[0]["status"], "failed");
        assert_eq!(conditions[0]["agent_actionable"], false);
        assert_eq!(conditions[1]["name"], "lint_check");
        assert_eq!(conditions[1]["status"], "timed_out");

        // integration should be absent
        assert!(json.get("integration").is_none());
    }

    #[test]
    fn serialize_integration() {
        let resp = NextResponse::Integration {
            state: "delegate".to_string(),
            directive: "Run the integration.".to_string(),
            details: None,
            advanced: false,
            expects: None,
            integration: IntegrationOutput {
                name: "code_review".to_string(),
                output: serde_json::json!({"result": "approved"}),
            },
        };

        let json: serde_json::Value = serde_json::to_value(&resp).unwrap();

        assert_eq!(json["action"], "integration");
        assert_eq!(json["state"], "delegate");
        assert_eq!(json["directive"], "Run the integration.");
        assert!(json["error"].is_null());
        assert!(json["expects"].is_null());

        assert_eq!(json["integration"]["name"], "code_review");
        assert_eq!(
            json["integration"]["output"],
            serde_json::json!({"result": "approved"})
        );

        // blocking_conditions should be absent
        assert!(json.get("blocking_conditions").is_none());
    }

    #[test]
    fn serialize_integration_with_expects() {
        let mut fields = BTreeMap::new();
        fields.insert(
            "approval".to_string(),
            ExpectsFieldSchema {
                field_type: "boolean".to_string(),
                required: true,
                values: vec![],
            },
        );

        let resp = NextResponse::Integration {
            state: "delegate".to_string(),
            directive: "Run and confirm.".to_string(),
            details: None,
            advanced: true,
            expects: Some(ExpectsSchema {
                event_type: "evidence_submitted".to_string(),
                fields,
                options: vec![],
            }),
            integration: IntegrationOutput {
                name: "review_tool".to_string(),
                output: serde_json::json!({"status": "done"}),
            },
        };

        let json: serde_json::Value = serde_json::to_value(&resp).unwrap();

        assert_eq!(json["advanced"], true);
        assert!(json["expects"].is_object());
        assert_eq!(json["expects"]["fields"]["approval"]["type"], "boolean");
        assert_eq!(json["integration"]["name"], "review_tool");
    }

    #[test]
    fn serialize_integration_unavailable() {
        let resp = NextResponse::IntegrationUnavailable {
            state: "delegate".to_string(),
            directive: "Run the integration.".to_string(),
            details: None,
            advanced: false,
            expects: None,
            integration: IntegrationUnavailableMarker {
                name: "code_review".to_string(),
                available: false,
            },
        };

        let json: serde_json::Value = serde_json::to_value(&resp).unwrap();

        assert_eq!(json["action"], "integration_unavailable");
        assert_eq!(json["state"], "delegate");
        assert!(json["error"].is_null());
        assert!(json["expects"].is_null());
        assert_eq!(json["integration"]["name"], "code_review");
        assert_eq!(json["integration"]["available"], false);

        assert!(json.get("blocking_conditions").is_none());
    }

    #[test]
    fn serialize_integration_unavailable_with_expects() {
        let mut fields = BTreeMap::new();
        fields.insert(
            "fallback".to_string(),
            ExpectsFieldSchema {
                field_type: "string".to_string(),
                required: true,
                values: vec![],
            },
        );

        let resp = NextResponse::IntegrationUnavailable {
            state: "delegate".to_string(),
            directive: "Integration unavailable, provide fallback.".to_string(),
            details: None,
            advanced: false,
            expects: Some(ExpectsSchema {
                event_type: "evidence_submitted".to_string(),
                fields,
                options: vec![],
            }),
            integration: IntegrationUnavailableMarker {
                name: "review_tool".to_string(),
                available: false,
            },
        };

        let json: serde_json::Value = serde_json::to_value(&resp).unwrap();

        assert!(json["expects"].is_object());
        assert_eq!(json["expects"]["fields"]["fallback"]["type"], "string");
        assert_eq!(json["integration"]["available"], false);
    }

    #[test]
    fn serialize_terminal() {
        let resp = NextResponse::Terminal {
            state: "done".to_string(),
            advanced: true,
        };

        let json: serde_json::Value = serde_json::to_value(&resp).unwrap();

        assert_eq!(json["action"], "done");
        assert_eq!(json["state"], "done");
        assert_eq!(json["advanced"], true);
        assert!(json["error"].is_null());
        assert!(json["expects"].is_null());

        // These fields should be absent for Terminal
        assert!(json.get("directive").is_none());
        assert!(json.get("blocking_conditions").is_none());
        assert!(json.get("integration").is_none());
    }

    #[test]
    fn serialize_terminal_not_advanced() {
        let resp = NextResponse::Terminal {
            state: "complete".to_string(),
            advanced: false,
        };

        let json: serde_json::Value = serde_json::to_value(&resp).unwrap();

        assert_eq!(json["action"], "done");
        assert_eq!(json["advanced"], false);
    }

    // -- NextErrorCode serialization tests --

    #[test]
    fn error_code_serializes_as_snake_case() {
        assert_eq!(
            serde_json::to_value(&NextErrorCode::GateBlocked).unwrap(),
            serde_json::json!("gate_blocked")
        );
        assert_eq!(
            serde_json::to_value(&NextErrorCode::InvalidSubmission).unwrap(),
            serde_json::json!("invalid_submission")
        );
        assert_eq!(
            serde_json::to_value(&NextErrorCode::PreconditionFailed).unwrap(),
            serde_json::json!("precondition_failed")
        );
        assert_eq!(
            serde_json::to_value(&NextErrorCode::IntegrationUnavailable).unwrap(),
            serde_json::json!("integration_unavailable")
        );
        assert_eq!(
            serde_json::to_value(&NextErrorCode::TerminalState).unwrap(),
            serde_json::json!("terminal_state")
        );
        assert_eq!(
            serde_json::to_value(&NextErrorCode::WorkflowNotInitialized).unwrap(),
            serde_json::json!("workflow_not_initialized")
        );
        assert_eq!(
            serde_json::to_value(&NextErrorCode::TemplateError).unwrap(),
            serde_json::json!("template_error")
        );
        assert_eq!(
            serde_json::to_value(&NextErrorCode::PersistenceError).unwrap(),
            serde_json::json!("persistence_error")
        );
        assert_eq!(
            serde_json::to_value(&NextErrorCode::ConcurrentAccess).unwrap(),
            serde_json::json!("concurrent_access")
        );
    }

    // -- Exit code mapping tests --

    #[test]
    fn exit_code_transient_errors() {
        assert_eq!(NextErrorCode::GateBlocked.exit_code(), 1);
        assert_eq!(NextErrorCode::IntegrationUnavailable.exit_code(), 1);
        assert_eq!(NextErrorCode::ConcurrentAccess.exit_code(), 1);
    }

    #[test]
    fn exit_code_caller_errors() {
        assert_eq!(NextErrorCode::InvalidSubmission.exit_code(), 2);
        assert_eq!(NextErrorCode::PreconditionFailed.exit_code(), 2);
        assert_eq!(NextErrorCode::TerminalState.exit_code(), 2);
        assert_eq!(NextErrorCode::WorkflowNotInitialized.exit_code(), 2);
    }

    #[test]
    fn exit_code_infrastructure_errors() {
        assert_eq!(NextErrorCode::TemplateError.exit_code(), 3);
        assert_eq!(NextErrorCode::PersistenceError.exit_code(), 3);
    }

    // -- NextError serialization tests --

    #[test]
    fn serialize_next_error() {
        let err = NextError {
            code: NextErrorCode::InvalidSubmission,
            message: "evidence validation failed".to_string(),
            details: vec![
                ErrorDetail {
                    field: "decision".to_string(),
                    reason: "required field missing".to_string(),
                },
                ErrorDetail {
                    field: "count".to_string(),
                    reason: "expected number, got string".to_string(),
                },
            ],
        };

        let json: serde_json::Value = serde_json::to_value(&err).unwrap();

        assert_eq!(json["code"], "invalid_submission");
        assert_eq!(json["message"], "evidence validation failed");

        let details = json["details"].as_array().unwrap();
        assert_eq!(details.len(), 2);
        assert_eq!(details[0]["field"], "decision");
        assert_eq!(details[0]["reason"], "required field missing");
        assert_eq!(details[1]["field"], "count");
        assert_eq!(details[1]["reason"], "expected number, got string");
    }

    #[test]
    fn serialize_next_error_no_details() {
        let err = NextError {
            code: NextErrorCode::TerminalState,
            message: "workflow is already complete".to_string(),
            details: vec![],
        };

        let json: serde_json::Value = serde_json::to_value(&err).unwrap();

        assert_eq!(json["code"], "terminal_state");
        assert_eq!(json["message"], "workflow is already complete");
        assert_eq!(json["details"], serde_json::json!([]));
    }

    // -- Supporting type serialization tests --

    #[test]
    fn expects_field_schema_type_rename() {
        let schema = ExpectsFieldSchema {
            field_type: "string".to_string(),
            required: true,
            values: vec![],
        };

        let json: serde_json::Value = serde_json::to_value(&schema).unwrap();

        // field_type serializes as "type"
        assert_eq!(json["type"], "string");
        assert!(json.get("field_type").is_none());
        // empty values omitted
        assert!(json.get("values").is_none());
    }

    #[test]
    fn expects_field_schema_with_values() {
        let schema = ExpectsFieldSchema {
            field_type: "enum".to_string(),
            required: true,
            values: vec!["a".to_string(), "b".to_string()],
        };

        let json: serde_json::Value = serde_json::to_value(&schema).unwrap();

        assert_eq!(json["type"], "enum");
        assert_eq!(json["values"], serde_json::json!(["a", "b"]));
    }

    #[test]
    fn blocking_condition_type_rename() {
        let cond = BlockingCondition {
            name: "ci".to_string(),
            condition_type: "command".to_string(),
            status: "failed".to_string(),
            agent_actionable: false,
            output: serde_json::json!({"exit_code": 1, "error": ""}),
        };

        let json: serde_json::Value = serde_json::to_value(&cond).unwrap();

        assert_eq!(json["type"], "command");
        assert!(json.get("condition_type").is_none());
    }

    #[test]
    fn expects_schema_omits_empty_options() {
        let schema = ExpectsSchema {
            event_type: "evidence_submitted".to_string(),
            fields: BTreeMap::new(),
            options: vec![],
        };

        let json: serde_json::Value = serde_json::to_value(&schema).unwrap();

        assert!(json.get("options").is_none());
    }

    #[test]
    fn expects_schema_includes_options_when_present() {
        let mut when = BTreeMap::new();
        when.insert("decision".to_string(), serde_json::json!("yes"));

        let schema = ExpectsSchema {
            event_type: "evidence_submitted".to_string(),
            fields: BTreeMap::new(),
            options: vec![TransitionOption {
                target: "next".to_string(),
                when,
            }],
        };

        let json: serde_json::Value = serde_json::to_value(&schema).unwrap();

        let options = json["options"].as_array().unwrap();
        assert_eq!(options.len(), 1);
        assert_eq!(options[0]["target"], "next");
    }

    // -- derive_expects tests --

    use crate::template::types::{FieldSchema, Transition};

    fn make_template_state(
        accepts: Option<BTreeMap<String, FieldSchema>>,
        transitions: Vec<Transition>,
    ) -> TemplateState {
        TemplateState {
            directive: "Do the thing.".to_string(),
            details: String::new(),
            transitions,
            terminal: false,
            gates: BTreeMap::new(),
            accepts,
            integration: None,
            default_action: None,
        }
    }

    #[test]
    fn derive_expects_no_accepts_returns_none() {
        let state = make_template_state(None, vec![]);
        assert!(derive_expects(&state).is_none());
    }

    #[test]
    fn derive_expects_with_accepts_and_conditional_transitions() {
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
        accepts.insert(
            "notes".to_string(),
            FieldSchema {
                field_type: "string".to_string(),
                required: false,
                values: vec![],
                description: "Optional notes".to_string(),
            },
        );

        let mut when_proceed = BTreeMap::new();
        when_proceed.insert("decision".to_string(), serde_json::json!("proceed"));
        let mut when_escalate = BTreeMap::new();
        when_escalate.insert("decision".to_string(), serde_json::json!("escalate"));

        let transitions = vec![
            Transition {
                target: "implement".to_string(),
                when: Some(when_proceed),
            },
            Transition {
                target: "review".to_string(),
                when: Some(when_escalate),
            },
        ];

        let state = make_template_state(Some(accepts), transitions);
        let expects = derive_expects(&state).unwrap();

        assert_eq!(expects.event_type, "evidence_submitted");
        assert_eq!(expects.fields.len(), 2);

        // Check field mapping
        let decision_field = &expects.fields["decision"];
        assert_eq!(decision_field.field_type, "enum");
        assert!(decision_field.required);
        assert_eq!(
            decision_field.values,
            vec!["proceed".to_string(), "escalate".to_string()]
        );

        let notes_field = &expects.fields["notes"];
        assert_eq!(notes_field.field_type, "string");
        assert!(!notes_field.required);
        assert!(notes_field.values.is_empty());

        // Check options
        assert_eq!(expects.options.len(), 2);
        assert_eq!(expects.options[0].target, "implement");
        assert_eq!(
            expects.options[0].when["decision"],
            serde_json::json!("proceed")
        );
        assert_eq!(expects.options[1].target, "review");
        assert_eq!(
            expects.options[1].when["decision"],
            serde_json::json!("escalate")
        );

        // Verify serialization: field_type -> "type", options present
        let json = serde_json::to_value(&expects).unwrap();
        assert_eq!(json["fields"]["decision"]["type"], "enum");
        assert!(json["fields"]["decision"].get("field_type").is_none());
        assert!(json.get("options").is_some());
    }

    #[test]
    fn derive_expects_with_accepts_no_conditional_transitions() {
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

        // Unconditional transition only
        let transitions = vec![Transition {
            target: "next_state".to_string(),
            when: None,
        }];

        let state = make_template_state(Some(accepts), transitions);
        let expects = derive_expects(&state).unwrap();

        assert_eq!(expects.event_type, "evidence_submitted");
        assert_eq!(expects.fields.len(), 1);
        assert!(expects.options.is_empty());

        // When serialized, options should be omitted
        let json = serde_json::to_value(&expects).unwrap();
        assert!(json.get("options").is_none());
    }

    // -- blocking_conditions_from_gates tests --

    use crate::gate::{GateOutcome, StructuredGateResult};
    use crate::template::types::Gate;

    fn make_command_gate() -> Gate {
        Gate {
            gate_type: "command".to_string(),
            command: "exit 0".to_string(),
            key: String::new(),
            pattern: String::new(),
            timeout: 0,
            override_default: None,
        }
    }

    fn make_context_exists_gate() -> Gate {
        Gate {
            gate_type: "context-exists".to_string(),
            command: String::new(),
            key: "my_key".to_string(),
            pattern: String::new(),
            timeout: 0,
            override_default: None,
        }
    }

    #[test]
    fn blocking_conditions_passed_gate_excluded() {
        let mut gate_results = BTreeMap::new();
        gate_results.insert(
            "ci_check".to_string(),
            StructuredGateResult {
                outcome: GateOutcome::Passed,
                output: serde_json::json!({"exit_code": 0, "error": ""}),
            },
        );

        let mut gate_defs = BTreeMap::new();
        gate_defs.insert("ci_check".to_string(), make_command_gate());

        let conditions = blocking_conditions_from_gates(&gate_results, &gate_defs);
        assert!(
            conditions.is_empty(),
            "passed gate must not appear in blocking_conditions"
        );
    }

    #[test]
    fn blocking_conditions_command_gate_failed_includes_structured_output() {
        let mut gate_results = BTreeMap::new();
        gate_results.insert(
            "ci_check".to_string(),
            StructuredGateResult {
                outcome: GateOutcome::Failed,
                output: serde_json::json!({"exit_code": 1, "error": ""}),
            },
        );

        let mut gate_defs = BTreeMap::new();
        gate_defs.insert("ci_check".to_string(), make_command_gate());

        let conditions = blocking_conditions_from_gates(&gate_results, &gate_defs);
        assert_eq!(conditions.len(), 1);

        let cond = &conditions[0];
        assert_eq!(cond.name, "ci_check");
        assert_eq!(cond.condition_type, "command");
        assert_eq!(cond.status, "failed");
        assert!(!cond.agent_actionable);
        assert_eq!(
            cond.output,
            serde_json::json!({"exit_code": 1, "error": ""})
        );
    }

    #[test]
    fn blocking_conditions_context_exists_gate_includes_structured_output() {
        let mut gate_results = BTreeMap::new();
        gate_results.insert(
            "ctx_gate".to_string(),
            StructuredGateResult {
                outcome: GateOutcome::Failed,
                output: serde_json::json!({"exists": false, "error": ""}),
            },
        );

        let mut gate_defs = BTreeMap::new();
        gate_defs.insert("ctx_gate".to_string(), make_context_exists_gate());

        let conditions = blocking_conditions_from_gates(&gate_results, &gate_defs);
        assert_eq!(conditions.len(), 1);

        let cond = &conditions[0];
        assert_eq!(cond.name, "ctx_gate");
        assert_eq!(cond.condition_type, "context-exists");
        assert_eq!(cond.status, "failed");
        assert_eq!(
            cond.output,
            serde_json::json!({"exists": false, "error": ""})
        );
    }

    #[test]
    fn blocking_conditions_timed_out_gate() {
        let mut gate_results = BTreeMap::new();
        gate_results.insert(
            "slow_gate".to_string(),
            StructuredGateResult {
                outcome: GateOutcome::TimedOut,
                output: serde_json::json!({"exit_code": -1, "error": "timed_out"}),
            },
        );

        let mut gate_defs = BTreeMap::new();
        gate_defs.insert("slow_gate".to_string(), make_command_gate());

        let conditions = blocking_conditions_from_gates(&gate_results, &gate_defs);
        assert_eq!(conditions.len(), 1);
        assert_eq!(conditions[0].status, "timed_out");
        assert_eq!(
            conditions[0].output,
            serde_json::json!({"exit_code": -1, "error": "timed_out"})
        );
    }

    #[test]
    fn blocking_conditions_error_gate() {
        let mut gate_results = BTreeMap::new();
        gate_results.insert(
            "bad_gate".to_string(),
            StructuredGateResult {
                outcome: GateOutcome::Error,
                output: serde_json::json!({"exit_code": -1, "error": "spawn failed"}),
            },
        );

        let mut gate_defs = BTreeMap::new();
        gate_defs.insert("bad_gate".to_string(), make_command_gate());

        let conditions = blocking_conditions_from_gates(&gate_results, &gate_defs);
        assert_eq!(conditions.len(), 1);
        assert_eq!(conditions[0].status, "error");
        assert_eq!(conditions[0].output["error"], "spawn failed");
    }

    #[test]
    fn blocking_conditions_gate_not_in_defs_falls_back_to_command_type() {
        let mut gate_results = BTreeMap::new();
        gate_results.insert(
            "unknown_gate".to_string(),
            StructuredGateResult {
                outcome: GateOutcome::Failed,
                output: serde_json::json!({"exit_code": 1, "error": ""}),
            },
        );

        // gate_defs is empty -- gate name not found
        let gate_defs: BTreeMap<String, Gate> = BTreeMap::new();

        let conditions = blocking_conditions_from_gates(&gate_results, &gate_defs);
        assert_eq!(conditions.len(), 1);
        assert_eq!(conditions[0].condition_type, "command");
    }

    #[test]
    fn blocking_conditions_mixed_passed_and_failed() {
        let mut gate_results = BTreeMap::new();
        gate_results.insert(
            "gate_pass".to_string(),
            StructuredGateResult {
                outcome: GateOutcome::Passed,
                output: serde_json::json!({"exit_code": 0, "error": ""}),
            },
        );
        gate_results.insert(
            "gate_fail".to_string(),
            StructuredGateResult {
                outcome: GateOutcome::Failed,
                output: serde_json::json!({"exit_code": 2, "error": ""}),
            },
        );

        let mut gate_defs = BTreeMap::new();
        gate_defs.insert("gate_pass".to_string(), make_command_gate());
        gate_defs.insert("gate_fail".to_string(), make_command_gate());

        let conditions = blocking_conditions_from_gates(&gate_results, &gate_defs);
        assert_eq!(conditions.len(), 1, "only the failed gate should appear");
        assert_eq!(conditions[0].name, "gate_fail");
        assert_eq!(conditions[0].output["exit_code"], 2);
    }

    #[test]
    fn derive_expects_mixed_conditional_and_unconditional() {
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "choice".to_string(),
            FieldSchema {
                field_type: "enum".to_string(),
                required: true,
                values: vec!["a".to_string(), "b".to_string()],
                description: String::new(),
            },
        );

        let mut when = BTreeMap::new();
        when.insert("choice".to_string(), serde_json::json!("a"));

        let transitions = vec![
            Transition {
                target: "path_a".to_string(),
                when: Some(when),
            },
            Transition {
                target: "fallback".to_string(),
                when: None,
            },
        ];

        let state = make_template_state(Some(accepts), transitions);
        let expects = derive_expects(&state).unwrap();

        // Only the conditional transition appears in options
        assert_eq!(expects.options.len(), 1);
        assert_eq!(expects.options[0].target, "path_a");
    }
}
