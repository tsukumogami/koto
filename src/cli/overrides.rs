//! `koto overrides` subcommands: record and list gate overrides.
//!
//! The `record` handler is modeled after `handle_decisions_record` in `mod.rs`.
//! `list` uses `derive_overrides_all` so callers see all override history across
//! epoch boundaries, not just the current epoch.

use anyhow::Result;
use clap::Subcommand;

use crate::cache::sha256_hex;
use crate::engine::persistence::{
    derive_last_gate_evaluated, derive_machine_state, derive_overrides_all, derive_state_from_log,
};
use crate::engine::types::{now_iso8601, EventPayload};
use crate::gate::built_in_default;
use crate::session::SessionBackend;

use super::{
    exit_code_for_engine_error, exit_with_error_code, resolve_with_data_source,
    EXIT_INFRASTRUCTURE, MAX_WITH_DATA_BYTES,
};

#[derive(Subcommand)]
pub enum OverridesSubcommand {
    /// Record a gate override with mandatory rationale
    Record {
        /// Workflow name
        name: String,

        /// Gate to override
        #[arg(long)]
        gate: String,

        /// Rationale for the override (required)
        #[arg(long)]
        rationale: String,

        /// Override value as JSON (optional; falls back to gate's override_default,
        /// then built-in default for the gate type)
        #[arg(long = "with-data")]
        with_data: Option<String>,
    },

    /// List all gate overrides across the full session history
    List {
        /// Workflow name
        name: String,
    },
}

/// Resolve the override_applied value from the three-tier fallback chain.
///
/// Resolution order: `with_data` argument → gate `override_default` → `built_in_default`.
/// Returns `Ok(value)` or `Err(message)` when no default is available.
pub fn resolve_override_applied(
    with_data: Option<&str>,
    gate: &crate::template::types::Gate,
) -> Result<serde_json::Value, String> {
    if let Some(data_str) = with_data {
        return serde_json::from_str(data_str)
            .map_err(|e| format!("invalid JSON in --with-data: {}", e));
    }
    if let Some(ref default_val) = gate.override_default {
        return Ok(default_val.clone());
    }
    if let Some(builtin) = built_in_default(&gate.gate_type) {
        return Ok(builtin);
    }
    Err(format!(
        "no override value available for gate (type '{}'): \
         provide --with-data or set override_default on the gate",
        gate.gate_type
    ))
}

/// Handle the `koto overrides record` command.
///
/// Flow:
/// 1. Size-limit checks on --with-data and --rationale
/// 2. Load state file and template
/// 3. Validate --gate exists in current template state
/// 4. Resolve override_applied: --with-data → gate.override_default → built_in_default
/// 5. Read actual_output from derive_last_gate_evaluated (null if absent)
/// 6. Append GateOverrideRecorded event
pub fn handle_overrides_record(
    backend: &dyn SessionBackend,
    name: String,
    gate: String,
    rationale: String,
    with_data: Option<String>,
) -> Result<()> {
    // 0. Resolve --with-data source (inline JSON or @file.json). Keeps this
    //    handler aligned with `koto next`; see `resolve_with_data_source`.
    let with_data = match with_data {
        Some(raw) => match resolve_with_data_source(&raw) {
            Ok(s) => Some(s),
            Err(err) => {
                exit_with_error_code(
                    serde_json::json!({
                        "error": err.message,
                        "command": "overrides record"
                    }),
                    err.code.exit_code(),
                );
            }
        },
        None => None,
    };

    // 1. Size-limit checks
    if let Some(ref d) = with_data {
        if d.len() > MAX_WITH_DATA_BYTES {
            exit_with_error_code(
                serde_json::json!({
                    "error": format!(
                        "--with-data payload exceeds maximum size of {} bytes",
                        MAX_WITH_DATA_BYTES
                    ),
                    "command": "overrides record"
                }),
                2,
            );
        }
    }
    if rationale.len() > MAX_WITH_DATA_BYTES {
        exit_with_error_code(
            serde_json::json!({
                "error": format!(
                    "--rationale exceeds maximum size of {} bytes",
                    MAX_WITH_DATA_BYTES
                ),
                "command": "overrides record"
            }),
            2,
        );
    }

    // 2. Load state file
    if !backend.exists(&name) {
        exit_with_error_code(
            serde_json::json!({
                "error": format!("workflow '{}' not found", name),
                "command": "overrides record"
            }),
            1,
        );
    }

    let (header, events) = match backend.read_events(&name) {
        Ok(result) => result,
        Err(err) => {
            let code = exit_code_for_engine_error(&err);
            exit_with_error_code(
                serde_json::json!({
                    "error": err.to_string(),
                    "command": "overrides record"
                }),
                code,
            );
        }
    };

    let machine_state = match derive_machine_state(&header, &events) {
        Some(ms) => ms,
        None => {
            // EXIT_INFRASTRUCTURE (3) is correct here; handle_decisions_record uses 1 by
            // mistake — that's a pre-existing inconsistency in decisions, not a reference to follow.
            exit_with_error_code(
                serde_json::json!({
                    "error": "corrupt state file: cannot derive current state",
                    "command": "overrides record"
                }),
                EXIT_INFRASTRUCTURE,
            );
        }
    };

    // Verify template hash
    let template_bytes = match std::fs::read(&machine_state.template_path) {
        Ok(b) => b,
        Err(e) => {
            exit_with_error_code(
                serde_json::json!({
                    "error": format!("failed to read template {}: {}", machine_state.template_path, e),
                    "command": "overrides record"
                }),
                EXIT_INFRASTRUCTURE,
            );
        }
    };
    let actual_hash = sha256_hex(&template_bytes);
    if actual_hash != machine_state.template_hash {
        exit_with_error_code(
            serde_json::json!({
                "error": format!(
                    "template hash mismatch: header says {} but cached template hashes to {}",
                    machine_state.template_hash, actual_hash
                ),
                "command": "overrides record"
            }),
            EXIT_INFRASTRUCTURE,
        );
    }

    let compiled: crate::template::types::CompiledTemplate =
        match serde_json::from_slice(&template_bytes) {
            Ok(t) => t,
            Err(e) => {
                exit_with_error_code(
                    serde_json::json!({
                        "error": format!(
                            "failed to parse template {}: {}",
                            machine_state.template_path, e
                        ),
                        "command": "overrides record"
                    }),
                    EXIT_INFRASTRUCTURE,
                );
            }
        };

    let current_state = machine_state.current_state.clone();

    // 3. Validate --gate exists in the current template state
    let template_state = match compiled.states.get(&current_state) {
        Some(s) => s,
        None => {
            exit_with_error_code(
                serde_json::json!({
                    "error": format!("state '{}' not found in template", current_state),
                    "command": "overrides record"
                }),
                EXIT_INFRASTRUCTURE,
            );
        }
    };

    let gate_def = match template_state.gates.get(&gate) {
        Some(g) => g,
        None => {
            let known: Vec<String> = template_state.gates.keys().cloned().collect();
            exit_with_error_code(
                serde_json::json!({
                    "error": format!(
                        "gate '{}' not found in state '{}'; known gates: [{}]",
                        gate,
                        current_state,
                        known.join(", ")
                    ),
                    "command": "overrides record"
                }),
                2,
            );
        }
    };

    // 4. Resolve override_applied: --with-data → gate.override_default → built_in_default
    let override_applied = match resolve_override_applied(with_data.as_deref(), gate_def) {
        Ok(v) => v,
        Err(msg) => {
            exit_with_error_code(
                serde_json::json!({
                    "error": msg,
                    "command": "overrides record"
                }),
                2,
            );
        }
    };

    // 5. Read actual_output from the most recent GateEvaluated event in the current epoch.
    // Use null when no GateEvaluated event exists for this gate.
    let actual_output: serde_json::Value =
        derive_last_gate_evaluated(&events, &gate).unwrap_or(serde_json::Value::Null);

    // 6. Append GateOverrideRecorded event
    let ts = now_iso8601();
    let payload = EventPayload::GateOverrideRecorded {
        state: current_state.clone(),
        gate: gate.clone(),
        rationale: rationale.clone(),
        override_applied: override_applied.clone(),
        actual_output: actual_output.clone(),
        timestamp: ts.clone(),
    };
    if let Err(e) = backend.append_event(&name, &payload, &ts) {
        exit_with_error_code(
            serde_json::json!({
                "error": e.to_string(),
                "command": "overrides record"
            }),
            1,
        );
    }

    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "status": "recorded"
        }))?
    );
    Ok(())
}

/// Handle the `koto overrides list` command.
///
/// Returns the full session override history (all epochs) as a JSON object
/// with `state` (current workflow state) and `overrides.items` array.
pub fn handle_overrides_list(backend: &dyn SessionBackend, name: String) -> Result<()> {
    if !backend.exists(&name) {
        exit_with_error_code(
            serde_json::json!({
                "error": format!("no state file found for workflow '{}'", name),
                "command": "overrides list"
            }),
            2,
        );
    }

    let (_, events) = match backend.read_events(&name) {
        Ok(result) => result,
        Err(err) => {
            let code = exit_code_for_engine_error(&err);
            exit_with_error_code(
                serde_json::json!({
                    "error": err.to_string(),
                    "command": "overrides list"
                }),
                code,
            );
        }
    };

    let current_state = derive_state_from_log(&events).unwrap_or_default();
    let override_events = derive_overrides_all(&events);

    let items: Vec<serde_json::Value> = override_events
        .iter()
        .filter_map(|e| {
            if let EventPayload::GateOverrideRecorded {
                state,
                gate,
                rationale,
                override_applied,
                actual_output,
                timestamp,
            } = &e.payload
            {
                Some(serde_json::json!({
                    "state": state,
                    "gate": gate,
                    "rationale": rationale,
                    "override_applied": override_applied,
                    "actual_output": actual_output,
                    "timestamp": timestamp,
                }))
            } else {
                None
            }
        })
        .collect();

    let count = items.len();
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "state": current_state,
            "overrides": {
                "count": count,
                "items": items
            }
        }))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::persistence::derive_last_gate_evaluated;
    use crate::engine::types::{Event, EventPayload};
    use std::collections::HashMap;

    // ---------------------------------------------------------------------------
    // Unit test: resolve_override_applied resolution order
    // ---------------------------------------------------------------------------

    fn make_command_gate(
        override_default: Option<serde_json::Value>,
    ) -> crate::template::types::Gate {
        crate::template::types::Gate {
            gate_type: "command".to_string(),
            command: "exit 1".to_string(),
            timeout: 0,
            key: String::new(),
            pattern: String::new(),
            override_default,
            completion: None,
            name_filter: None,
        }
    }

    #[test]
    fn resolve_override_applied_with_data_wins_over_all() {
        // --with-data takes precedence over override_default and built_in_default.
        let gate = make_command_gate(Some(
            serde_json::json!({"exit_code": 99, "error": "from_default"}),
        ));
        let result = resolve_override_applied(
            Some(r#"{"exit_code": 42, "error": "from_with_data"}"#),
            &gate,
        );
        assert_eq!(
            result.unwrap(),
            serde_json::json!({"exit_code": 42, "error": "from_with_data"})
        );
    }

    #[test]
    fn resolve_override_applied_falls_through_to_override_default() {
        // No --with-data: uses gate.override_default.
        let gate = make_command_gate(Some(
            serde_json::json!({"exit_code": 1, "error": "manual_review"}),
        ));
        let result = resolve_override_applied(None, &gate);
        assert_eq!(
            result.unwrap(),
            serde_json::json!({"exit_code": 1, "error": "manual_review"})
        );
    }

    #[test]
    fn resolve_override_applied_falls_through_to_built_in_default() {
        // No --with-data, no override_default: uses built_in_default for "command" type.
        let gate = make_command_gate(None);
        let result = resolve_override_applied(None, &gate);
        assert_eq!(
            result.unwrap(),
            serde_json::json!({"exit_code": 0, "error": ""})
        );
    }

    #[test]
    fn resolve_override_applied_returns_error_when_no_default_available() {
        // Unknown gate type with no override_default and no --with-data: error.
        let gate = crate::template::types::Gate {
            gate_type: "unknown-custom-type".to_string(),
            command: "exit 1".to_string(),
            timeout: 0,
            key: String::new(),
            pattern: String::new(),
            override_default: None,
            completion: None,
            name_filter: None,
        };
        let result = resolve_override_applied(None, &gate);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("no override value available"),
            "expected 'no override value available' in error, got: {}",
            msg
        );
    }

    // ---------------------------------------------------------------------------
    // Unit test: actual_output is null when no GateEvaluated event exists
    // ---------------------------------------------------------------------------

    #[test]
    fn actual_output_is_null_when_no_gate_evaluated_event() {
        let ts = now_iso8601();
        let events = vec![
            Event {
                seq: 1,
                timestamp: ts.clone(),
                event_type: "workflow_initialized".to_string(),
                payload: EventPayload::WorkflowInitialized {
                    template_path: "/tmp/test.json".to_string(),
                    variables: HashMap::new(),
                    spawn_entry: None,
                },
            },
            Event {
                seq: 2,
                timestamp: ts.clone(),
                event_type: "transitioned".to_string(),
                payload: EventPayload::Transitioned {
                    from: None,
                    to: "start".to_string(),
                    condition_type: "auto".to_string(),
                },
            },
        ];

        let actual_output =
            derive_last_gate_evaluated(&events, "ci_check").unwrap_or(serde_json::Value::Null);

        assert_eq!(actual_output, serde_json::Value::Null);
    }

    #[test]
    fn actual_output_reads_from_gate_evaluated_event() {
        let ts = now_iso8601();
        let expected_output = serde_json::json!({"exit_code": 1, "error": "test failed"});
        let events = vec![
            Event {
                seq: 1,
                timestamp: ts.clone(),
                event_type: "workflow_initialized".to_string(),
                payload: EventPayload::WorkflowInitialized {
                    template_path: "/tmp/test.json".to_string(),
                    variables: HashMap::new(),
                    spawn_entry: None,
                },
            },
            Event {
                seq: 2,
                timestamp: ts.clone(),
                event_type: "transitioned".to_string(),
                payload: EventPayload::Transitioned {
                    from: None,
                    to: "start".to_string(),
                    condition_type: "auto".to_string(),
                },
            },
            Event {
                seq: 3,
                timestamp: ts.clone(),
                event_type: "gate_evaluated".to_string(),
                payload: EventPayload::GateEvaluated {
                    state: "start".to_string(),
                    gate: "ci_check".to_string(),
                    output: expected_output.clone(),
                    outcome: "failed".to_string(),
                    timestamp: ts.clone(),
                },
            },
        ];

        let actual_output =
            derive_last_gate_evaluated(&events, "ci_check").unwrap_or(serde_json::Value::Null);

        assert_eq!(actual_output, expected_output);
    }
}
