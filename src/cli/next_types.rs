use std::collections::BTreeMap;

use serde::ser::SerializeMap;
use serde::Serialize;

use crate::cli::batch_error::BatchError;
use crate::gate::{built_in_default, GateOutcome, StructuredGateResult};
use crate::template::types::{Gate, TemplateState, FIELD_TYPE_TASKS};

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
    /// Rejected submission — typed error envelope with optional
    /// batch-specific context. Emits `action: "error"`. Carries the
    /// typed `NextError` alongside an optional [`BatchErrorContext`]
    /// that re-emits the same JSON payload under `error.batch` so
    /// agents parse the batch shape from the same place regardless of
    /// whether the error is domain- or batch-scoped.
    Error {
        state: String,
        advanced: bool,
        error: NextError,
        batch: Option<BatchErrorContext>,
        blocking_conditions: Vec<BlockingCondition>,
    },
}

/// Sibling payload attached to `NextResponse::Error` when the underlying
/// failure originates from batch machinery (Decision 11). Carries the
/// exact JSON payload [`BatchError::to_batch_payload`] produces so
/// downstream agents see the same shape whether the error comes in via
/// the top-level `{"action": "error", "batch": ...}` envelope or nested
/// under `error.batch` in a typed-NextError response.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct BatchErrorContext {
    /// Raw JSON payload — opaque from this layer's perspective. Use
    /// [`BatchErrorContext::from_batch_error`] to build one from a
    /// typed [`BatchError`] so the payload stays in sync.
    pub payload: serde_json::Value,
}

impl BatchErrorContext {
    /// Build a [`BatchErrorContext`] from a typed [`BatchError`]. The
    /// payload matches what `BatchError::to_envelope()["batch"]` would
    /// produce, keeping one source of truth for the `batch` key shape.
    pub fn from_batch_error(err: &BatchError) -> Self {
        Self {
            payload: err.to_batch_payload(),
        }
    }
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
            // `Error` carries no directive to substitute; return as-is.
            err @ NextResponse::Error { .. } => err,
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
            NextResponse::Error {
                state,
                advanced,
                error,
                batch,
                blocking_conditions,
            } => {
                // Emit the error payload as a single object containing
                // the typed NextError fields plus an optional `batch`
                // sibling carrying the typed batch context.
                let error_value = {
                    let mut obj = serde_json::Map::new();
                    obj.insert(
                        "code".into(),
                        serde_json::to_value(&error.code).expect("NextErrorCode serializable"),
                    );
                    obj.insert(
                        "message".into(),
                        serde_json::Value::String(error.message.clone()),
                    );
                    obj.insert(
                        "details".into(),
                        serde_json::to_value(&error.details).expect("ErrorDetail serializable"),
                    );
                    if let Some(ctx) = batch {
                        obj.insert("batch".into(), ctx.payload.clone());
                    }
                    serde_json::Value::Object(obj)
                };
                let mut map = serializer.serialize_map(Some(6))?;
                map.serialize_entry("action", "error")?;
                map.serialize_entry("state", state)?;
                map.serialize_entry("advanced", advanced)?;
                map.serialize_entry("expects", &None::<()>)?;
                map.serialize_entry("blocking_conditions", blocking_conditions)?;
                map.serialize_entry("error", &error_value)?;
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
///
/// `item_schema` is auto-generated for `tasks`-typed fields and describes the
/// expected shape of each task-list entry. It is always `None` for other field
/// types. The template author never writes this — it is synthesized by
/// `derive_expects` from the fixed task-entry contract defined by koto
/// itself. See DESIGN-batch-child-spawning.md Decision 8.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExpectsFieldSchema {
    #[serde(rename = "type")]
    pub field_type: String,
    pub required: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_schema: Option<serde_json::Value>,
}

/// A transition option surfaced to the agent.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TransitionOption {
    pub target: String,
    pub when: BTreeMap<String, serde_json::Value>,
}

/// Default category for blocking conditions: agent must take corrective action.
#[allow(dead_code)]
fn default_category() -> String {
    "corrective".to_string()
}

/// A condition blocking state advancement.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BlockingCondition {
    pub name: String,
    #[serde(rename = "type")]
    pub condition_type: String,
    pub status: String,
    /// `"temporal"` (retry later) for children-complete, `"corrective"` (fix
    /// something) for all other gate types.
    #[serde(default = "default_category")]
    pub category: String,
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
/// gate result. `agent_actionable` is set to `true` when the gate has either an
/// instance-level `override_default` or a built-in default for its gate type, signaling
/// that the agent can call `koto overrides record` to substitute the gate output.
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
            let category = crate::gate::gate_blocking_category(&condition_type).to_string();
            let agent_actionable = gate_defs
                .get(name)
                .map(|g| g.override_default.is_some() || built_in_default(&g.gate_type).is_some())
                .unwrap_or(false);
            Some(BlockingCondition {
                name: name.clone(),
                condition_type,
                status: status.to_string(),
                category,
                agent_actionable,
                output: result.output.clone(),
            })
        })
        .collect()
}

/// Build the auto-generated `item_schema` object for a `tasks`-typed field.
///
/// The shape is fixed by koto — template authors never author or override it.
/// When the enclosing state declares a `materialize_children` hook whose
/// `from_field` matches, the hook's `default_template` becomes the
/// `template.default` value; otherwise `template.default` is omitted.
///
/// See DESIGN-batch-child-spawning.md Decision 8 and Decision E7 for the full
/// rationale.
fn tasks_item_schema(state: &TemplateState, field_name: &str) -> serde_json::Value {
    use serde_json::{json, Map, Value};

    let default_template = state
        .materialize_children
        .as_ref()
        .filter(|hook| hook.from_field == field_name)
        .map(|hook| hook.default_template.clone());

    let mut template_entry = Map::new();
    template_entry.insert("type".to_string(), Value::String("string".to_string()));
    template_entry.insert("required".to_string(), Value::Bool(false));
    if let Some(default) = default_template {
        template_entry.insert("default".to_string(), Value::String(default));
    }

    json!({
        "name": {
            "type": "string",
            "required": true,
            "description": "Child workflow short name"
        },
        "template": Value::Object(template_entry),
        "vars": {
            "type": "object",
            "required": false
        },
        "waits_on": {
            "type": "array",
            "required": false,
            "default": []
        },
        "trigger_rule": {
            "type": "string",
            "required": false,
            "default": "all_success"
        }
    })
}

/// Derive an `ExpectsSchema` from a template state's `accepts` block and transitions.
///
/// Returns `None` when the state has no `accepts` block. When present, maps each
/// `FieldSchema` to `ExpectsFieldSchema` and populates `options` from transitions
/// that have `when` conditions. Options are omitted entirely when no transitions
/// have `when`.
///
/// For fields whose type is `tasks`, an auto-generated `item_schema` object is
/// attached describing the task-entry contract (name, template, vars,
/// waits_on, trigger_rule). The template author does not — and cannot —
/// customize this schema; see Decision E7 in DESIGN-batch-child-spawning.md.
pub fn derive_expects(state: &TemplateState) -> Option<ExpectsSchema> {
    let accepts = state.accepts.as_ref()?;

    let fields: BTreeMap<String, ExpectsFieldSchema> = accepts
        .iter()
        .map(|(name, schema)| {
            let item_schema = if schema.field_type == FIELD_TYPE_TASKS {
                Some(tasks_item_schema(state, name))
            } else {
                None
            };
            (
                name.clone(),
                ExpectsFieldSchema {
                    field_type: schema.field_type.clone(),
                    required: schema.required,
                    values: schema.values.clone(),
                    item_schema,
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
                item_schema: None,
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
                item_schema: None,
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
                    category: "corrective".to_string(),
                    agent_actionable: false,
                    output: serde_json::json!({"exit_code": 1, "error": ""}),
                },
                BlockingCondition {
                    name: "lint_check".to_string(),
                    condition_type: "command".to_string(),
                    status: "timed_out".to_string(),
                    category: "corrective".to_string(),
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
                item_schema: None,
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
                item_schema: None,
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
            item_schema: None,
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
            item_schema: None,
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
            category: "corrective".to_string(),
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
            materialize_children: None,
            failure: false,
            skipped_marker: false,
            skip_if: None,
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
            completion: None,
            name_filter: None,
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
            completion: None,
            name_filter: None,
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
        // command gate has a built-in default, so agent_actionable is true.
        assert!(cond.agent_actionable);
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

    // -- agent_actionable tests (Issue #4: override pre-check) --

    #[test]
    fn agent_actionable_true_for_command_gate_with_builtin_default() {
        // command gate has a built-in default; agent_actionable should be true.
        let mut gate_results = BTreeMap::new();
        gate_results.insert(
            "ci".to_string(),
            StructuredGateResult {
                outcome: GateOutcome::Failed,
                output: serde_json::json!({"exit_code": 1, "error": ""}),
            },
        );

        let mut gate_defs = BTreeMap::new();
        gate_defs.insert("ci".to_string(), make_command_gate());

        let conditions = blocking_conditions_from_gates(&gate_results, &gate_defs);
        assert_eq!(conditions.len(), 1);
        assert!(
            conditions[0].agent_actionable,
            "command gate has built-in default, agent_actionable must be true"
        );
    }

    #[test]
    fn agent_actionable_true_for_gate_with_instance_override_default() {
        // Gate with an explicit override_default on the instance.
        let mut gate_results = BTreeMap::new();
        gate_results.insert(
            "custom".to_string(),
            StructuredGateResult {
                outcome: GateOutcome::Failed,
                output: serde_json::json!({"exit_code": 1, "error": ""}),
            },
        );

        let mut gate_defs = BTreeMap::new();
        gate_defs.insert(
            "custom".to_string(),
            Gate {
                gate_type: "custom-unknown".to_string(), // no built-in default
                command: String::new(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: Some(serde_json::json!({"result": "ok"})),
                completion: None,
                name_filter: None,
            },
        );

        let conditions = blocking_conditions_from_gates(&gate_results, &gate_defs);
        assert_eq!(conditions.len(), 1);
        assert!(
            conditions[0].agent_actionable,
            "gate with instance override_default must have agent_actionable true"
        );
    }

    #[test]
    fn agent_actionable_false_for_unknown_type_without_override_default() {
        // Unknown gate type with no override_default; agent_actionable must be false.
        let mut gate_results = BTreeMap::new();
        gate_results.insert(
            "weird".to_string(),
            StructuredGateResult {
                outcome: GateOutcome::Failed,
                output: serde_json::json!({"exit_code": 1, "error": ""}),
            },
        );

        let mut gate_defs = BTreeMap::new();
        gate_defs.insert(
            "weird".to_string(),
            Gate {
                gate_type: "custom-unknown".to_string(),
                command: String::new(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
                completion: None,
                name_filter: None,
            },
        );

        let conditions = blocking_conditions_from_gates(&gate_results, &gate_defs);
        assert_eq!(conditions.len(), 1);
        assert!(
            !conditions[0].agent_actionable,
            "unknown gate type with no override_default must have agent_actionable false"
        );
    }

    // -----------------------------------------------------------------
    // Issue 7: tasks-typed accepts field auto-generates item_schema.
    // -----------------------------------------------------------------

    use crate::template::types::{FailurePolicy, MaterializeChildrenSpec};

    fn tasks_accepts(field: &str, required: bool) -> BTreeMap<String, FieldSchema> {
        let mut accepts = BTreeMap::new();
        accepts.insert(
            field.to_string(),
            FieldSchema {
                field_type: "tasks".to_string(),
                required,
                values: vec![],
                description: String::new(),
            },
        );
        accepts
    }

    #[test]
    fn tasks_field_gets_auto_generated_item_schema() {
        // A tasks-typed accepts field on a state with a materialize_children
        // hook produces an item_schema whose shape matches the design's
        // Decision 8 (name, template, vars, waits_on, trigger_rule).
        let mut state = make_template_state(Some(tasks_accepts("tasks", true)), vec![]);
        state.materialize_children = Some(MaterializeChildrenSpec {
            from_field: "tasks".to_string(),
            default_template: "impl-issue.md".to_string(),
            failure_policy: FailurePolicy::SkipDependents,
        });
        let expects = derive_expects(&state).unwrap();
        let tasks_field = expects.fields.get("tasks").expect("tasks field");
        assert_eq!(tasks_field.field_type, "tasks");
        let item_schema = tasks_field
            .item_schema
            .as_ref()
            .expect("item_schema must be auto-generated for tasks fields");

        // Fixed shape.
        assert_eq!(item_schema["name"]["type"], "string");
        assert_eq!(item_schema["name"]["required"], true);
        assert!(item_schema["name"]
            .get("description")
            .map(|d| d.is_string())
            .unwrap_or(false));

        assert_eq!(item_schema["template"]["type"], "string");
        assert_eq!(item_schema["template"]["required"], false);
        // default_template flows through as template.default.
        assert_eq!(item_schema["template"]["default"], "impl-issue.md");

        assert_eq!(item_schema["vars"]["type"], "object");
        assert_eq!(item_schema["vars"]["required"], false);

        assert_eq!(item_schema["waits_on"]["type"], "array");
        assert_eq!(item_schema["waits_on"]["required"], false);
        assert_eq!(item_schema["waits_on"]["default"], serde_json::json!([]));

        assert_eq!(item_schema["trigger_rule"]["type"], "string");
        assert_eq!(item_schema["trigger_rule"]["required"], false);
        assert_eq!(item_schema["trigger_rule"]["default"], "all_success");
    }

    #[test]
    fn tasks_field_without_hook_omits_template_default() {
        // When there is no materialize_children hook, the template.default
        // entry is absent from item_schema — agents receive no default.
        let state = make_template_state(Some(tasks_accepts("tasks", true)), vec![]);
        let expects = derive_expects(&state).unwrap();
        let item_schema = expects.fields["tasks"]
            .item_schema
            .as_ref()
            .expect("item_schema present even without hook");
        let template_entry = item_schema["template"].as_object().unwrap();
        assert!(
            !template_entry.contains_key("default"),
            "template.default must be omitted when no materialize_children hook exists"
        );
    }

    #[test]
    fn tasks_field_hook_with_mismatched_from_field_omits_default() {
        // The default_template pulls through only when the hook's from_field
        // matches this accepts field's name. A mismatch is unusual (Issue 8
        // will fail the template at validation) but the response-side must
        // behave sanely in the meantime.
        let mut state = make_template_state(Some(tasks_accepts("tasks", true)), vec![]);
        state.materialize_children = Some(MaterializeChildrenSpec {
            from_field: "other".to_string(),
            default_template: "ignored.md".to_string(),
            failure_policy: FailurePolicy::SkipDependents,
        });
        let expects = derive_expects(&state).unwrap();
        let template_entry = expects.fields["tasks"].item_schema.as_ref().unwrap()["template"]
            .as_object()
            .unwrap()
            .clone();
        assert!(!template_entry.contains_key("default"));
    }

    #[test]
    fn non_tasks_fields_have_no_item_schema() {
        // Only tasks-typed fields get item_schema — enum/string/number/boolean
        // still serialize without the field.
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "name".to_string(),
            FieldSchema {
                field_type: "string".to_string(),
                required: false,
                values: vec![],
                description: String::new(),
            },
        );
        let state = make_template_state(Some(accepts), vec![]);
        let expects = derive_expects(&state).unwrap();
        assert!(expects.fields["name"].item_schema.is_none());
    }

    #[test]
    fn tasks_item_schema_serializes_cleanly_over_json() {
        // The auto-generated schema serializes with `item_schema` as a
        // sibling of `type`, `required`, and `values`.
        let mut state = make_template_state(Some(tasks_accepts("tasks", true)), vec![]);
        state.materialize_children = Some(MaterializeChildrenSpec {
            from_field: "tasks".to_string(),
            default_template: "child.md".to_string(),
            failure_policy: FailurePolicy::SkipDependents,
        });
        let expects = derive_expects(&state).unwrap();
        let json = serde_json::to_value(&expects).unwrap();
        assert_eq!(json["fields"]["tasks"]["type"], "tasks");
        assert!(json["fields"]["tasks"]["item_schema"].is_object());
        assert_eq!(
            json["fields"]["tasks"]["item_schema"]["template"]["default"],
            "child.md"
        );
    }

    // --- NextResponse::Error serialization ---------------------------

    #[test]
    fn serialize_error_variant_without_batch_context() {
        let resp = NextResponse::Error {
            state: "plan".into(),
            advanced: false,
            error: NextError {
                code: NextErrorCode::InvalidSubmission,
                message: "bad input".into(),
                details: vec![ErrorDetail {
                    field: "tasks".into(),
                    reason: "empty".into(),
                }],
            },
            batch: None,
            blocking_conditions: vec![],
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["action"], "error");
        assert_eq!(v["state"], "plan");
        assert_eq!(v["advanced"], false);
        assert_eq!(v["error"]["code"], "invalid_submission");
        assert_eq!(v["error"]["message"], "bad input");
        assert_eq!(v["error"]["details"][0]["field"], "tasks");
        assert!(v["error"].get("batch").is_none());
        assert!(v["expects"].is_null());
        assert_eq!(v["blocking_conditions"], serde_json::json!([]));
    }

    #[test]
    fn serialize_error_variant_with_batch_context_embeds_payload() {
        use crate::cli::batch_error::{BatchError, InvalidBatchReason};

        let batch_err = BatchError::InvalidBatchDefinition {
            reason: InvalidBatchReason::EmptyTaskList,
        };
        let resp = NextResponse::Error {
            state: "plan".into(),
            advanced: false,
            error: NextError {
                code: NextErrorCode::InvalidSubmission,
                message: "batch rejected".into(),
                details: vec![],
            },
            batch: Some(BatchErrorContext::from_batch_error(&batch_err)),
            blocking_conditions: vec![],
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["action"], "error");
        assert_eq!(v["error"]["batch"]["kind"], "invalid_batch_definition");
        assert_eq!(v["error"]["batch"]["reason"]["reason"], "empty_task_list");
    }

    #[test]
    fn error_variant_batch_context_matches_to_envelope_shape() {
        use crate::cli::batch_error::BatchError;

        // The payload nested under `error.batch` must equal the `batch`
        // payload of `BatchError::to_envelope()` byte-for-byte — one
        // source of truth for the batch shape.
        let err = BatchError::ConcurrentTick {
            holder_pid: Some(42),
        };
        let expected_batch = err.to_envelope()["batch"].clone();

        let ctx = BatchErrorContext::from_batch_error(&err);
        assert_eq!(ctx.payload, expected_batch);
    }
}
