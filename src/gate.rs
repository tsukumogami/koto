//! Gate evaluator for command, context-exists, context-matches, and
//! request-leg gates (children-complete is evaluated through a caller-supplied
//! closure).
//!
//! Command gates spawn shell commands in isolated process groups with
//! configurable timeouts. Context gates check the session context store.
//! Request-leg gates read one leg of a request log, without writing to it.
//! Evaluates all gates without short-circuiting so callers see every blocking
//! condition in a single response.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::action::{run_shell_command, CommandOutput, FailureKind};
use crate::engine::request_store::{self, LegView, RequestStoreError, ValidatedRequestId};
use crate::engine::types::{CloseDisposition, LegDisposition, LegResultSource, RequestState};
use crate::session::context::ContextStore;
use crate::template::types::{
    Gate, GATE_TYPE_CHILDREN_COMPLETE, GATE_TYPE_COMMAND, GATE_TYPE_CONTEXT_EXISTS,
    GATE_TYPE_CONTEXT_MATCHES, GATE_TYPE_REQUEST_LEG, SUPPORTED_GATE_TYPES,
};

/// Outcome of a structured gate evaluation.
///
/// Carries the control-flow signal used by the advance loop to determine
/// whether a state should block or continue. The associated structured output
/// is held in [`StructuredGateResult`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GateOutcome {
    /// The gate condition was satisfied.
    Passed,
    /// The gate condition was not satisfied.
    Failed,
    /// The command did not finish within the configured timeout.
    TimedOut,
    /// The command could not be spawned or an OS error occurred.
    Error,
}

/// Structured result of evaluating a single gate.
///
/// Carries both the control-flow outcome and the gate-type-specific JSON
/// output. The `output` field holds structured data matching the gate type's
/// schema (e.g. `{"exit_code": 0, "error": ""}` for command gates), making it
/// available for injection into the evidence map and transition routing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuredGateResult {
    /// Control-flow outcome used by the advance loop.
    pub outcome: GateOutcome,
    /// Gate-type-specific structured output for evidence injection.
    pub output: serde_json::Value,
}

/// Evaluate all gates, running each command with `working_dir` as the current
/// directory and using `context_store` + `session` for context-aware gates.
/// Every gate is evaluated regardless of individual results (no short-circuit).
///
/// When `context_store` is `None`, context-aware gate types produce an error
/// result indicating that context evaluation is unavailable.
///
/// `children_evaluator` is an optional callback for `children-complete` gates.
/// When `None`, children-complete gates produce an error result. The CLI
/// handler supplies a closure that captures the session backend and template
/// loading logic.
///
/// Gates arrive here **already substituted**. A context gate's `key` and
/// `pattern` are read as given, so passing an authored gate means asking the
/// store for a key spelled `{{SESSION_NAME}}-note` -- which is the defect
/// Issue #222 fixed, not a supported call. `crate::cli::substitute_gate_fields`
/// is what produces the substituted form, and both production callers go
/// through it. Nothing in the type distinguishes the two, so this note is the
/// whole of the guard.
pub fn evaluate_gates(
    gates: &BTreeMap<String, Gate>,
    working_dir: &Path,
    context_store: Option<&dyn ContextStore>,
    session: Option<&str>,
    children_evaluator: Option<&dyn Fn(&Gate) -> StructuredGateResult>,
) -> BTreeMap<String, StructuredGateResult> {
    evaluate_gates_with_request_store(
        gates,
        working_dir,
        context_store,
        session,
        children_evaluator,
        None,
    )
}

/// [`evaluate_gates`], with the request store `request-leg` gates read.
///
/// `request_root` is the workspace root the store lives under (`~/.koto`).
/// `None` means no request store is available -- a non-unix host, or a
/// session on the cloud backend, where request records do not replicate --
/// and every `request-leg` gate then reports outcome `Error` with the reason
/// in `error`, rather than passing or blocking silently.
pub fn evaluate_gates_with_request_store(
    gates: &BTreeMap<String, Gate>,
    working_dir: &Path,
    context_store: Option<&dyn ContextStore>,
    session: Option<&str>,
    children_evaluator: Option<&dyn Fn(&Gate) -> StructuredGateResult>,
    request_root: Option<&Path>,
) -> BTreeMap<String, StructuredGateResult> {
    let mut results = BTreeMap::new();
    for (name, gate) in gates {
        let result = match gate.gate_type.as_str() {
            GATE_TYPE_COMMAND => evaluate_command_gate(gate, working_dir),
            GATE_TYPE_REQUEST_LEG => evaluate_request_leg_gate(gate, request_root),
            GATE_TYPE_CONTEXT_EXISTS => evaluate_context_exists_gate(gate, context_store, session),
            GATE_TYPE_CONTEXT_MATCHES => {
                evaluate_context_matches_gate(gate, context_store, session)
            }
            GATE_TYPE_CHILDREN_COMPLETE => match children_evaluator {
                Some(eval_fn) => eval_fn(gate),
                None => StructuredGateResult {
                    outcome: GateOutcome::Error,
                    output: serde_json::json!({
                        "total": 0,
                        "completed": 0,
                        "pending": 0,
                        "success": 0,
                        "failed": 0,
                        "skipped": 0,
                        "blocked": 0,
                        "spawn_failed": 0,
                        "all_complete": false,
                        "all_success": false,
                        "any_failed": false,
                        "any_skipped": false,
                        "any_spawn_failed": false,
                        "needs_attention": false,
                        "children": [],
                        "error": "children-complete gate requires a session backend"
                    }),
                },
            },
            other => StructuredGateResult {
                outcome: GateOutcome::Error,
                output: serde_json::json!({
                    "exit_code": -1,
                    "error": format!(
                        "unsupported gate type '{}'; only {} gates are evaluated",
                        other,
                        SUPPORTED_GATE_TYPES.join(", ")
                    )
                }),
            },
        };
        results.insert(name.clone(), result);
    }
    results
}

/// The structured output of a `request-leg` gate, before it becomes JSON.
///
/// Every field is always emitted, so a `when` clause or an assignment can
/// read any of them on any outcome. The key set is the schema
/// `gate_type_schema("request-leg")` declares; `request_leg_output_keys_match_the_schema`
/// holds the two together.
#[derive(Debug, Default)]
struct RequestLegOutput {
    found: bool,
    disposition: &'static str,
    bound: bool,
    source: &'static str,
    status: String,
    final_state: String,
    template: String,
    outcome: String,
    step: String,
    reason: String,
    valid: bool,
    payload: serde_json::Map<String, serde_json::Value>,
    error: String,
}

impl RequestLegOutput {
    fn into_json(self) -> serde_json::Value {
        serde_json::json!({
            "found": self.found,
            "disposition": self.disposition,
            "bound": self.bound,
            "source": self.source,
            "status": self.status,
            "final_state": self.final_state,
            "template": self.template,
            "outcome": self.outcome,
            "step": self.step,
            "reason": self.reason,
            "valid": self.valid,
            "payload": serde_json::Value::Object(self.payload),
            "error": self.error,
        })
    }
}

/// An `Error` result for a `request-leg` gate that could not read the leg.
///
/// `disposition` stays empty rather than `missing`: the gate does not know
/// whether the leg exists, and an arm keyed on `disposition: missing` must
/// not fire on a store it could not read.
fn request_leg_error(error: String) -> StructuredGateResult {
    StructuredGateResult {
        outcome: GateOutcome::Error,
        output: RequestLegOutput {
            error,
            ..Default::default()
        }
        .into_json(),
    }
}

/// A `Failed` result reporting a request or leg that is not there.
fn request_leg_missing(error: String) -> StructuredGateResult {
    StructuredGateResult {
        outcome: GateOutcome::Failed,
        output: RequestLegOutput {
            disposition: "missing",
            error,
            ..Default::default()
        }
        .into_json(),
    }
}

/// Evaluate a `request-leg` gate: read one leg of one request and report its
/// disposition and recorded result.
///
/// The gate only reads. It takes no lock and appends nothing, so evaluating
/// it can never bind, resolve, or abandon the leg it watches.
///
/// - A resolved leg passes, carrying the result's status and payload.
/// - An abandoned leg, or an unresolved leg on a request that was abandoned
///   or closed, passes with `disposition: abandoned`, so a workflow can route
///   it instead of waiting on an answer that cannot come.
/// - An open leg fails; `gate_blocking_category` classes the type as
///   temporal, so the state waits rather than asking for a correction.
/// - A request or leg that is not there fails with `disposition: missing`.
/// - A request id or leg name that is unusable after substitution, an
///   unreadable record, or no request store at all is an `Error`, with the
///   reason in `error` and nothing read.
pub fn evaluate_request_leg_gate(gate: &Gate, request_root: Option<&Path>) -> StructuredGateResult {
    let Some(root) = request_root else {
        return request_leg_error(
            "request-leg gate requires the local request store, which this session cannot \
             reach (request records are local to the host and are not available on a \
             non-unix host or under the cloud backend)"
                .to_string(),
        );
    };
    // The compiler checked a literal value; a substituted one is checked here,
    // before any path is built from it.
    let request_id = match ValidatedRequestId::new(&gate.request) {
        Ok(id) => id,
        Err(e) => {
            return request_leg_error(format!(
                "request-leg gate: request {:?} is not a usable request id: {}",
                gate.request, e
            ))
        }
    };
    if let Err(e) = request_store::validate_leg_name(&gate.leg) {
        return request_leg_error(format!(
            "request-leg gate: leg {:?} is not a usable leg name: {}",
            gate.leg, e
        ));
    }
    let view = match request_store::read_view(root, &request_id) {
        Ok(view) => view,
        Err(RequestStoreError::NotFound { .. }) => {
            return request_leg_missing(format!("request {:?} not found", gate.request));
        }
        Err(e) => {
            return request_leg_error(format!(
                "request-leg gate could not read request {:?}: {}",
                gate.request, e
            ))
        }
    };
    let Some(leg) = view.legs.get(&gate.leg) else {
        return request_leg_missing(format!(
            "leg {:?} not found on request {:?}",
            gate.leg, gate.request
        ));
    };

    let mut out = RequestLegOutput {
        found: true,
        bound: leg.bound_child.is_some(),
        ..Default::default()
    };
    let request_given_up = view.request_state == RequestState::Closed
        || view.close_disposition == Some(CloseDisposition::RequestAbandoned);
    match leg.disposition {
        LegDisposition::Resolved => {
            fill_resolved(&mut out, leg, gate);
            StructuredGateResult {
                outcome: GateOutcome::Passed,
                output: out.into_json(),
            }
        }
        LegDisposition::Abandoned => {
            out.disposition = "abandoned";
            StructuredGateResult {
                outcome: GateOutcome::Passed,
                output: out.into_json(),
            }
        }
        LegDisposition::Open if request_given_up => {
            // Nothing can resolve a leg on a closed request, so waiting on it
            // would block forever. Report it the way an abandoned leg reads.
            out.disposition = "abandoned";
            StructuredGateResult {
                outcome: GateOutcome::Passed,
                output: out.into_json(),
            }
        }
        LegDisposition::Open => {
            out.disposition = "open";
            StructuredGateResult {
                outcome: GateOutcome::Failed,
                output: out.into_json(),
            }
        }
    }
}

/// Fill the result fields of a resolved leg's output.
fn fill_resolved(out: &mut RequestLegOutput, leg: &LegView, gate: &Gate) {
    out.disposition = "resolved";
    out.source = match leg.result_source {
        Some(LegResultSource::Promoted) => "promoted",
        Some(LegResultSource::Explicit) => "explicit",
        Some(LegResultSource::Refused) => "refused",
        None => "",
    };
    let Some(result) = &leg.result else {
        return;
    };
    out.status = serde_json::to_value(result.status)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    // `final_state` and `template` identify the session that answered, so
    // they are reported only for a promoted result; an explicit or refused
    // one came from no terminal state.
    if leg.result_source == Some(LegResultSource::Promoted) {
        out.final_state = leg.result_final_state.clone().unwrap_or_default();
        // The source file name: the identity a leg's `template` admits.
        out.template = leg
            .bound_template
            .as_ref()
            .map(|t| t.source.clone())
            .unwrap_or_default();
    }
    let payload_is_object = match &result.payload {
        None => true,
        Some(serde_json::Value::Object(map)) => {
            out.payload = map.clone();
            true
        }
        // A non-object payload has no keys to route on; it is reported as `{}`
        // so the field keeps its declared type, and it makes the leg invalid.
        Some(_) => false,
    };
    let string_key = |key: &str| {
        out.payload
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    out.outcome = string_key("outcome");
    out.step = string_key("step");
    out.reason = string_key("reason");
    out.valid = payload_is_object && expect_satisfied(gate, &out.payload);
}

/// Whether `payload` carries every key `expect` names with a listed value.
/// No `expect` accepts any payload; keys `expect` does not name are ignored.
fn expect_satisfied(gate: &Gate, payload: &serde_json::Map<String, serde_json::Value>) -> bool {
    let Some(expect) = &gate.expect else {
        return true;
    };
    expect
        .iter()
        .all(|(key, allowed)| payload.get(key).is_some_and(|v| allowed.contains(v)))
}

fn evaluate_context_exists_gate(
    gate: &Gate,
    context_store: Option<&dyn ContextStore>,
    session: Option<&str>,
) -> StructuredGateResult {
    let (store, sess) = match (context_store, session) {
        (Some(s), Some(n)) => (s, n),
        _ => {
            return StructuredGateResult {
                outcome: GateOutcome::Error,
                output: serde_json::json!({
                    "exists": false,
                    "error": "context-exists gate requires a context store and session"
                }),
            };
        }
    };
    if let Some(result) = unusable_key_result(&gate.key, "exists") {
        return result;
    }
    if store.ctx_exists(sess, &gate.key) {
        StructuredGateResult {
            outcome: GateOutcome::Passed,
            output: serde_json::json!({"exists": true, "error": ""}),
        }
    } else {
        StructuredGateResult {
            outcome: GateOutcome::Failed,
            output: serde_json::json!({"exists": false, "error": ""}),
        }
    }
}

/// The result for a context gate whose `key` the context store will not accept
/// by the time the gate is evaluated, or `None` when the key is fine.
///
/// The store answers an unusable key by reporting it absent: `ctx_exists`
/// returns `false` when validation fails, and `get` returns an error the gate
/// renders as a plain mismatch. Either way the evidence is
/// `{"exists": false, "error": ""}` -- byte-identical to the key genuinely not
/// being there. That is a gate that will not pass with nothing pointing at why,
/// which is the symptom Issue #222 was filed about, so the reason goes in the
/// `error` field the payload already carries.
///
/// This became worth checking when `key` started substituting, because a
/// reference is now how an unusable key arises. The wording itself lives with
/// the rule it describes, in [`crate::session::validate::unusable_context_key_reason`],
/// because `koto context exists` asks the same question about the same key and
/// the two surfaces must not answer it differently (Issue #227).
///
/// `field` names the evidence key the gate's shape carries, so a caller reading
/// `exists` or `matches` still finds it.
fn unusable_key_result(key: &str, field: &str) -> Option<StructuredGateResult> {
    let reason = crate::session::validate::unusable_context_key_reason(key)?;
    Some(StructuredGateResult {
        outcome: GateOutcome::Error,
        output: serde_json::json!({
            field: false,
            "error": reason
        }),
    })
}

fn evaluate_context_matches_gate(
    gate: &Gate,
    context_store: Option<&dyn ContextStore>,
    session: Option<&str>,
) -> StructuredGateResult {
    let (store, sess) = match (context_store, session) {
        (Some(s), Some(n)) => (s, n),
        _ => {
            return StructuredGateResult {
                outcome: GateOutcome::Error,
                output: serde_json::json!({
                    "matches": false,
                    "error": "context-matches gate requires a context store and session"
                }),
            };
        }
    };
    if let Some(result) = unusable_key_result(&gate.key, "matches") {
        return result;
    }
    // An empty pattern matches every input, so a gate whose pattern collapsed to
    // nothing would pass on content it was written to reject -- failing open,
    // which is worse than the failing-closed symptom Issue #222 was filed about.
    // The compiler refuses an empty authored pattern; it reads the authored
    // string, so a pattern that is only empty after substitution is this
    // function's to catch.
    if gate.pattern.is_empty() {
        return StructuredGateResult {
            outcome: GateOutcome::Error,
            output: serde_json::json!({
                "matches": false,
                "error": "context-matches pattern resolved to an empty string, \
                          which would match any content at all; a {{KEY}} \
                          reference in the gate's pattern has no value\n  \
                          remedy: give the variable a default, or make the \
                          pattern more than the reference alone"
            }),
        };
    }
    let content = match store.get(sess, &gate.key) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => {
                return StructuredGateResult {
                    outcome: GateOutcome::Failed,
                    output: serde_json::json!({"matches": false, "error": ""}),
                };
            }
        },
        Err(_) => {
            return StructuredGateResult {
                outcome: GateOutcome::Failed,
                output: serde_json::json!({"matches": false, "error": ""}),
            };
        }
    };
    match regex::Regex::new(&gate.pattern) {
        Ok(re) => {
            if re.is_match(&content) {
                StructuredGateResult {
                    outcome: GateOutcome::Passed,
                    output: serde_json::json!({"matches": true, "error": ""}),
                }
            } else {
                StructuredGateResult {
                    outcome: GateOutcome::Failed,
                    output: serde_json::json!({"matches": false, "error": ""}),
                }
            }
        }
        Err(e) => StructuredGateResult {
            outcome: GateOutcome::Error,
            output: serde_json::json!({
                "matches": false,
                "error": format!("invalid regex pattern: {}", e)
            }),
        },
    }
}

fn evaluate_command_gate(gate: &Gate, working_dir: &Path) -> StructuredGateResult {
    let output = run_shell_command(&gate.command, working_dir, gate.timeout);
    command_gate_result(output)
}

/// Map a command result onto a gate outcome and its evidence.
///
/// The runner reports why a command failed, so the three outcomes that used
/// to share `exit_code: -1` are told apart by `failure_kind` rather than by
/// searching stderr for "timed out". Evidence for those three gains a
/// `failure_kind` key; the passing and failing shapes are unchanged, which
/// keeps recorded gate evidence and overrides comparable byte for byte.
fn command_gate_result(output: CommandOutput) -> StructuredGateResult {
    match output.failure_kind {
        Some(FailureKind::TimedOut) => StructuredGateResult {
            outcome: GateOutcome::TimedOut,
            output: serde_json::json!({
                "exit_code": -1,
                "error": "timed_out",
                "failure_kind": FailureKind::TimedOut.as_str(),
            }),
        },
        Some(kind @ (FailureKind::SpawnFailed | FailureKind::WaitFailed)) => StructuredGateResult {
            outcome: GateOutcome::Error,
            output: serde_json::json!({
                "exit_code": -1,
                "error": output.stderr,
                "failure_kind": kind.as_str(),
            }),
        },
        Some(FailureKind::NonzeroExit) => StructuredGateResult {
            outcome: GateOutcome::Failed,
            output: serde_json::json!({"exit_code": output.exit_code, "error": ""}),
        },
        None => StructuredGateResult {
            outcome: GateOutcome::Passed,
            output: serde_json::json!({"exit_code": 0, "error": ""}),
        },
    }
}

/// Return the built-in default override value for a known gate type.
///
/// This is the fallback override value used by `koto overrides record` when
/// neither `--with-data` nor an instance-level `override_default` is present.
///
/// Returns `None` for unknown gate types, meaning no built-in default exists
/// and an explicit value must be supplied via `--with-data`.
///
/// **Sync contract**: `gate_type_builtin_default()` in `src/template/types.rs`
/// mirrors this function for compile-time use (circular dep prevents importing
/// from there). A test in types.rs asserts the two functions return identical
/// values for every `GATE_TYPE_*` constant. Update both together if defaults change.
pub fn built_in_default(gate_type: &str) -> Option<serde_json::Value> {
    match gate_type {
        GATE_TYPE_COMMAND => Some(serde_json::json!({"exit_code": 0, "error": ""})),
        GATE_TYPE_CONTEXT_EXISTS => Some(serde_json::json!({"exists": true, "error": ""})),
        GATE_TYPE_CONTEXT_MATCHES => Some(serde_json::json!({"matches": true, "error": ""})),
        GATE_TYPE_CHILDREN_COMPLETE => Some(serde_json::json!({
            "total": 0,
            "completed": 0,
            "pending": 0,
            "success": 0,
            "failed": 0,
            "skipped": 0,
            "blocked": 0,
            "spawn_failed": 0,
            "all_complete": true,
            "all_success": true,
            "any_failed": false,
            "any_skipped": false,
            "any_spawn_failed": false,
            "needs_attention": false,
            "children": [],
            "error": ""
        })),
        // Shared with `gate_type_builtin_default` rather than restated: a
        // resolved, valid record naming no child outcome.
        GATE_TYPE_REQUEST_LEG => Some(crate::template::types::request_leg_builtin_default()),
        _ => None,
    }
}

/// Return the blocking condition category for a gate type.
///
/// `"temporal"` means the blocking condition is time-dependent (retry later).
/// `"corrective"` means the agent needs to take corrective action.
pub fn gate_blocking_category(gate_type: &str) -> &'static str {
    match gate_type {
        // An open request leg is waiting on another session, like unfinished
        // children: the state waits rather than asking for a correction.
        GATE_TYPE_CHILDREN_COMPLETE | GATE_TYPE_REQUEST_LEG => "temporal",
        _ => "corrective",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::template::types::GATE_TYPE_COMMAND;

    fn make_gate(command: &str, timeout: u32) -> Gate {
        Gate {
            gate_type: GATE_TYPE_COMMAND.to_string(),
            command: command.to_string(),
            timeout,
            key: String::new(),
            pattern: String::new(),
            override_default: None,
            completion: None,
            name_filter: None,
            overridable: true,
            request: String::new(),
            leg: String::new(),
            expect: None,
        }
    }

    fn tmp_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn passing_gate() {
        let dir = tmp_dir();
        let mut gates = BTreeMap::new();
        gates.insert("check".to_string(), make_gate("exit 0", 5));

        let results = evaluate_gates(&gates, dir.path(), None, None, None);
        assert_eq!(results.len(), 1);
        assert_eq!(results["check"].outcome, GateOutcome::Passed);
        assert_eq!(results["check"].output["exit_code"], 0);
        assert_eq!(results["check"].output["error"], "");
    }

    #[test]
    fn failing_gate() {
        let dir = tmp_dir();
        let mut gates = BTreeMap::new();
        gates.insert("check".to_string(), make_gate("exit 42", 5));

        let results = evaluate_gates(&gates, dir.path(), None, None, None);
        assert_eq!(results.len(), 1);
        assert_eq!(results["check"].outcome, GateOutcome::Failed);
        assert_eq!(results["check"].output["exit_code"], 42);
        assert_eq!(results["check"].output["error"], "");
    }

    #[test]
    fn timed_out_gate() {
        let dir = tmp_dir();
        let mut gates = BTreeMap::new();
        gates.insert("slow".to_string(), make_gate("sleep 60", 1));

        let results = evaluate_gates(&gates, dir.path(), None, None, None);
        assert_eq!(results.len(), 1);
        assert_eq!(results["slow"].outcome, GateOutcome::TimedOut);
        assert_eq!(results["slow"].output["exit_code"], -1);
        assert_eq!(results["slow"].output["error"], "timed_out");
    }

    /// Build a `CommandOutput` for the mapping tests below. `wait_failed`
    /// cannot be provoked from a real command, so the mapping is exercised
    /// directly rather than through `run_shell_command`.
    fn failed_output(kind: FailureKind, exit_code: i32, stderr: &str) -> CommandOutput {
        CommandOutput {
            exit_code,
            stdout: String::new(),
            stderr: stderr.to_string(),
            failure_kind: Some(kind),
            truncated: false,
        }
    }

    #[test]
    fn timed_out_maps_to_timed_out_with_the_existing_evidence_shape() {
        let result = command_gate_result(failed_output(FailureKind::TimedOut, -1, "partial"));
        assert_eq!(result.outcome, GateOutcome::TimedOut);
        assert_eq!(result.output["exit_code"], -1);
        assert_eq!(result.output["error"], "timed_out");
        assert_eq!(result.output["failure_kind"], "timed_out");
    }

    #[test]
    fn spawn_failed_maps_to_error() {
        let result = command_gate_result(failed_output(
            FailureKind::SpawnFailed,
            -1,
            "failed to spawn command: boom",
        ));
        assert_eq!(result.outcome, GateOutcome::Error);
        assert_eq!(result.output["exit_code"], -1);
        assert_eq!(result.output["error"], "failed to spawn command: boom");
        assert_eq!(result.output["failure_kind"], "spawn_failed");
    }

    #[test]
    fn wait_failed_maps_to_error_and_is_not_reported_as_a_timeout() {
        let result = command_gate_result(failed_output(
            FailureKind::WaitFailed,
            -1,
            "error waiting for command: boom",
        ));
        assert_eq!(result.outcome, GateOutcome::Error);
        assert_eq!(result.output["exit_code"], -1);
        assert_eq!(result.output["error"], "error waiting for command: boom");
        assert_eq!(result.output["failure_kind"], "wait_failed");
    }

    #[test]
    fn nonzero_exit_evidence_is_unchanged() {
        let result = command_gate_result(failed_output(FailureKind::NonzeroExit, 3, ""));
        assert_eq!(result.outcome, GateOutcome::Failed);
        assert_eq!(
            result.output,
            serde_json::json!({"exit_code": 3, "error": ""})
        );
    }

    #[test]
    fn passing_evidence_is_byte_identical_to_the_recorded_default() {
        let result = command_gate_result(CommandOutput {
            exit_code: 0,
            stdout: "hi\n".to_string(),
            stderr: String::new(),
            failure_kind: None,
            truncated: false,
        });
        assert_eq!(result.outcome, GateOutcome::Passed);
        assert_eq!(
            serde_json::to_string(&result.output).unwrap(),
            serde_json::to_string(&built_in_default(GATE_TYPE_COMMAND).unwrap()).unwrap()
        );
    }

    #[test]
    fn gate_output_above_the_pipe_buffer_does_not_deadlock() {
        let dir = tmp_dir();
        let mut gates = BTreeMap::new();
        gates.insert(
            "loud".to_string(),
            make_gate(
                "for i in $(seq 1 4096); do printf '%063d\\n' \"$i\"; done; exit 0",
                10,
            ),
        );

        let results = evaluate_gates(&gates, dir.path(), None, None, None);
        assert_eq!(results["loud"].outcome, GateOutcome::Passed);
        assert_eq!(results["loud"].output["exit_code"], 0);
    }

    #[test]
    fn error_gate_nonexistent_command() {
        let dir = tmp_dir();
        let mut gates = BTreeMap::new();
        gates.insert("bad".to_string(), make_gate("nonexistent_cmd_xyz_12345", 5));

        let results = evaluate_gates(&gates, dir.path(), None, None, None);
        assert_eq!(results.len(), 1);
        // The shell itself exits 127 for command-not-found.
        assert_eq!(results["bad"].outcome, GateOutcome::Failed);
        assert_eq!(results["bad"].output["exit_code"], 127);
    }

    #[test]
    fn multiple_gates_mixed_results() {
        let dir = tmp_dir();
        let mut gates = BTreeMap::new();
        gates.insert("pass".to_string(), make_gate("exit 0", 5));
        gates.insert("fail".to_string(), make_gate("exit 1", 5));
        gates.insert("timeout".to_string(), make_gate("sleep 60", 1));

        let results = evaluate_gates(&gates, dir.path(), None, None, None);
        assert_eq!(results.len(), 3);
        assert_eq!(results["pass"].outcome, GateOutcome::Passed);
        assert_eq!(results["fail"].outcome, GateOutcome::Failed);
        assert_eq!(results["fail"].output["exit_code"], 1);
        assert_eq!(results["timeout"].outcome, GateOutcome::TimedOut);
    }

    #[test]
    fn gate_runs_in_working_dir() {
        let dir = tmp_dir();
        // Create a marker file in the temp dir.
        std::fs::write(dir.path().join("marker.txt"), "found").unwrap();

        let mut gates = BTreeMap::new();
        gates.insert("check_dir".to_string(), make_gate("test -f marker.txt", 5));

        let results = evaluate_gates(&gates, dir.path(), None, None, None);
        assert_eq!(results["check_dir"].outcome, GateOutcome::Passed);
    }

    #[test]
    fn default_timeout_used_when_zero() {
        // We can't easily test the 30s default without waiting, but we can
        // verify a gate with timeout=0 still works (uses default).
        let dir = tmp_dir();
        let mut gates = BTreeMap::new();
        gates.insert("quick".to_string(), make_gate("exit 0", 0));

        let results = evaluate_gates(&gates, dir.path(), None, None, None);
        assert_eq!(results["quick"].outcome, GateOutcome::Passed);
    }

    // -----------------------------------------------------------------------
    // Context-aware gate tests
    // -----------------------------------------------------------------------

    /// In-memory ContextStore for testing.
    struct MockContextStore {
        entries: std::sync::Mutex<BTreeMap<(String, String), Vec<u8>>>,
    }

    impl MockContextStore {
        fn new() -> Self {
            Self {
                entries: std::sync::Mutex::new(BTreeMap::new()),
            }
        }

        fn insert(&self, session: &str, key: &str, content: &[u8]) {
            self.entries
                .lock()
                .unwrap()
                .insert((session.to_string(), key.to_string()), content.to_vec());
        }
    }

    impl ContextStore for MockContextStore {
        fn add(&self, session: &str, key: &str, content: &[u8]) -> anyhow::Result<()> {
            self.insert(session, key, content);
            Ok(())
        }

        fn get(&self, session: &str, key: &str) -> anyhow::Result<Vec<u8>> {
            self.entries
                .lock()
                .unwrap()
                .get(&(session.to_string(), key.to_string()))
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("key not found"))
        }

        fn ctx_exists(&self, session: &str, key: &str) -> bool {
            self.entries
                .lock()
                .unwrap()
                .contains_key(&(session.to_string(), key.to_string()))
        }

        fn remove(&self, session: &str, key: &str) -> anyhow::Result<()> {
            self.entries
                .lock()
                .unwrap()
                .remove(&(session.to_string(), key.to_string()));
            Ok(())
        }

        fn list_keys(&self, session: &str, prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
            let entries = self.entries.lock().unwrap();
            let keys: Vec<String> = entries
                .keys()
                .filter(|(s, k)| s == session && prefix.map_or(true, |p| k.starts_with(p)))
                .map(|(_, k)| k.clone())
                .collect();
            Ok(keys)
        }
    }

    #[test]
    fn context_exists_gate_passes_when_key_present() {
        let dir = tmp_dir();
        let store = MockContextStore::new();
        store.insert("sess1", "research/lead.md", b"some content");

        let mut gates = BTreeMap::new();
        gates.insert(
            "research".to_string(),
            Gate {
                gate_type: GATE_TYPE_CONTEXT_EXISTS.to_string(),
                command: String::new(),
                timeout: 0,
                key: "research/lead.md".to_string(),
                pattern: String::new(),
                override_default: None,
                completion: None,
                name_filter: None,
                overridable: true,
                request: String::new(),
                leg: String::new(),
                expect: None,
            },
        );

        let results = evaluate_gates(&gates, dir.path(), Some(&store), Some("sess1"), None);
        assert_eq!(results["research"].outcome, GateOutcome::Passed);
        assert_eq!(results["research"].output["exists"], true);
        assert_eq!(results["research"].output["error"], "");
    }

    #[test]
    fn context_exists_gate_fails_when_key_missing() {
        let dir = tmp_dir();
        let store = MockContextStore::new();

        let mut gates = BTreeMap::new();
        gates.insert(
            "research".to_string(),
            Gate {
                gate_type: GATE_TYPE_CONTEXT_EXISTS.to_string(),
                command: String::new(),
                timeout: 0,
                key: "research/lead.md".to_string(),
                pattern: String::new(),
                override_default: None,
                completion: None,
                name_filter: None,
                overridable: true,
                request: String::new(),
                leg: String::new(),
                expect: None,
            },
        );

        let results = evaluate_gates(&gates, dir.path(), Some(&store), Some("sess1"), None);
        assert_eq!(results["research"].outcome, GateOutcome::Failed);
        assert_eq!(results["research"].output["exists"], false);
        assert_eq!(results["research"].output["error"], "");
    }

    #[test]
    fn context_exists_gate_errors_without_store() {
        let dir = tmp_dir();
        let mut gates = BTreeMap::new();
        gates.insert(
            "research".to_string(),
            Gate {
                gate_type: GATE_TYPE_CONTEXT_EXISTS.to_string(),
                command: String::new(),
                timeout: 0,
                key: "research/lead.md".to_string(),
                pattern: String::new(),
                override_default: None,
                completion: None,
                name_filter: None,
                overridable: true,
                request: String::new(),
                leg: String::new(),
                expect: None,
            },
        );

        let results = evaluate_gates(&gates, dir.path(), None, None, None);
        assert_eq!(results["research"].outcome, GateOutcome::Error);
        assert_eq!(results["research"].output["exists"], false);
    }

    #[test]
    fn context_matches_gate_passes_when_pattern_matches() {
        let dir = tmp_dir();
        let store = MockContextStore::new();
        store.insert(
            "sess1",
            "review.md",
            b"# Review\n\n## Approved\n\nLooks good.",
        );

        let mut gates = BTreeMap::new();
        gates.insert(
            "review".to_string(),
            Gate {
                gate_type: GATE_TYPE_CONTEXT_MATCHES.to_string(),
                command: String::new(),
                timeout: 0,
                key: "review.md".to_string(),
                pattern: "## Approved".to_string(),
                override_default: None,
                completion: None,
                name_filter: None,
                overridable: true,
                request: String::new(),
                leg: String::new(),
                expect: None,
            },
        );

        let results = evaluate_gates(&gates, dir.path(), Some(&store), Some("sess1"), None);
        assert_eq!(results["review"].outcome, GateOutcome::Passed);
        assert_eq!(results["review"].output["matches"], true);
        assert_eq!(results["review"].output["error"], "");
    }

    #[test]
    fn context_matches_gate_fails_when_pattern_does_not_match() {
        let dir = tmp_dir();
        let store = MockContextStore::new();
        store.insert(
            "sess1",
            "review.md",
            b"# Review\n\n## Rejected\n\nNeeds work.",
        );

        let mut gates = BTreeMap::new();
        gates.insert(
            "review".to_string(),
            Gate {
                gate_type: GATE_TYPE_CONTEXT_MATCHES.to_string(),
                command: String::new(),
                timeout: 0,
                key: "review.md".to_string(),
                pattern: "## Approved".to_string(),
                override_default: None,
                completion: None,
                name_filter: None,
                overridable: true,
                request: String::new(),
                leg: String::new(),
                expect: None,
            },
        );

        let results = evaluate_gates(&gates, dir.path(), Some(&store), Some("sess1"), None);
        assert_eq!(results["review"].outcome, GateOutcome::Failed);
        assert_eq!(results["review"].output["matches"], false);
        assert_eq!(results["review"].output["error"], "");
    }

    #[test]
    fn context_matches_gate_fails_when_key_missing() {
        let dir = tmp_dir();
        let store = MockContextStore::new();

        let mut gates = BTreeMap::new();
        gates.insert(
            "review".to_string(),
            Gate {
                gate_type: GATE_TYPE_CONTEXT_MATCHES.to_string(),
                command: String::new(),
                timeout: 0,
                key: "review.md".to_string(),
                pattern: "## Approved".to_string(),
                override_default: None,
                completion: None,
                name_filter: None,
                overridable: true,
                request: String::new(),
                leg: String::new(),
                expect: None,
            },
        );

        let results = evaluate_gates(&gates, dir.path(), Some(&store), Some("sess1"), None);
        assert_eq!(results["review"].outcome, GateOutcome::Failed);
        assert_eq!(results["review"].output["matches"], false);
    }

    #[test]
    fn context_matches_gate_errors_without_store() {
        let dir = tmp_dir();
        let mut gates = BTreeMap::new();
        gates.insert(
            "review".to_string(),
            Gate {
                gate_type: GATE_TYPE_CONTEXT_MATCHES.to_string(),
                command: String::new(),
                timeout: 0,
                key: "review.md".to_string(),
                pattern: "## Approved".to_string(),
                override_default: None,
                completion: None,
                name_filter: None,
                overridable: true,
                request: String::new(),
                leg: String::new(),
                expect: None,
            },
        );

        let results = evaluate_gates(&gates, dir.path(), None, None, None);
        assert_eq!(results["review"].outcome, GateOutcome::Error);
        assert_eq!(results["review"].output["matches"], false);
    }

    #[test]
    fn context_matches_with_regex_pattern() {
        let dir = tmp_dir();
        let store = MockContextStore::new();
        store.insert("sess1", "status.txt", b"status: PASS (3/3 checks)");

        let mut gates = BTreeMap::new();
        gates.insert(
            "status".to_string(),
            Gate {
                gate_type: GATE_TYPE_CONTEXT_MATCHES.to_string(),
                command: String::new(),
                timeout: 0,
                key: "status.txt".to_string(),
                pattern: r"status:\s+PASS".to_string(),
                override_default: None,
                completion: None,
                name_filter: None,
                overridable: true,
                request: String::new(),
                leg: String::new(),
                expect: None,
            },
        );

        let results = evaluate_gates(&gates, dir.path(), Some(&store), Some("sess1"), None);
        assert_eq!(results["status"].outcome, GateOutcome::Passed);
        assert_eq!(results["status"].output["matches"], true);
    }

    // -----------------------------------------------------------------------
    // StructuredGateResult / GateOutcome serialization tests
    // -----------------------------------------------------------------------

    #[test]
    fn gate_outcome_passed_round_trip() {
        let outcome = GateOutcome::Passed;
        let json = serde_json::to_string(&outcome).unwrap();
        let decoded: GateOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, GateOutcome::Passed);
    }

    #[test]
    fn gate_outcome_failed_round_trip() {
        let outcome = GateOutcome::Failed;
        let json = serde_json::to_string(&outcome).unwrap();
        let decoded: GateOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, GateOutcome::Failed);
    }

    #[test]
    fn gate_outcome_timed_out_round_trip() {
        let outcome = GateOutcome::TimedOut;
        let json = serde_json::to_string(&outcome).unwrap();
        let decoded: GateOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, GateOutcome::TimedOut);
    }

    #[test]
    fn gate_outcome_error_round_trip() {
        let outcome = GateOutcome::Error;
        let json = serde_json::to_string(&outcome).unwrap();
        let decoded: GateOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, GateOutcome::Error);
    }

    #[test]
    fn structured_gate_result_passed_round_trip() {
        let result = StructuredGateResult {
            outcome: GateOutcome::Passed,
            output: serde_json::json!({"exit_code": 0, "error": ""}),
        };
        let json = serde_json::to_string(&result).unwrap();
        let decoded: StructuredGateResult = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.outcome, GateOutcome::Passed);
        assert_eq!(decoded.output["exit_code"], 0);
        assert_eq!(decoded.output["error"], "");
    }

    #[test]
    fn structured_gate_result_failed_round_trip() {
        let result = StructuredGateResult {
            outcome: GateOutcome::Failed,
            output: serde_json::json!({"exit_code": 1, "error": ""}),
        };
        let json = serde_json::to_string(&result).unwrap();
        let decoded: StructuredGateResult = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.outcome, GateOutcome::Failed);
        assert_eq!(decoded.output["exit_code"], 1);
    }

    #[test]
    fn structured_gate_result_timed_out_round_trip() {
        let result = StructuredGateResult {
            outcome: GateOutcome::TimedOut,
            output: serde_json::json!({"exit_code": -1, "error": "timed_out"}),
        };
        let json = serde_json::to_string(&result).unwrap();
        let decoded: StructuredGateResult = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.outcome, GateOutcome::TimedOut);
        assert_eq!(decoded.output["exit_code"], -1);
        assert_eq!(decoded.output["error"], "timed_out");
    }

    #[test]
    fn structured_gate_result_error_round_trip() {
        let result = StructuredGateResult {
            outcome: GateOutcome::Error,
            output: serde_json::json!({"exit_code": -1, "error": "spawn failed: no such file"}),
        };
        let json = serde_json::to_string(&result).unwrap();
        let decoded: StructuredGateResult = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.outcome, GateOutcome::Error);
        assert_eq!(decoded.output["error"], "spawn failed: no such file");
    }

    #[test]
    fn structured_gate_result_context_exists_schema() {
        let result = StructuredGateResult {
            outcome: GateOutcome::Passed,
            output: serde_json::json!({"exists": true, "error": ""}),
        };
        let json = serde_json::to_string(&result).unwrap();
        let decoded: StructuredGateResult = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.outcome, GateOutcome::Passed);
        assert_eq!(decoded.output["exists"], true);
        assert_eq!(decoded.output["error"], "");
    }

    #[test]
    fn structured_gate_result_context_matches_schema() {
        let result = StructuredGateResult {
            outcome: GateOutcome::Failed,
            output: serde_json::json!({"matches": false, "error": ""}),
        };
        let json = serde_json::to_string(&result).unwrap();
        let decoded: StructuredGateResult = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.outcome, GateOutcome::Failed);
        assert_eq!(decoded.output["matches"], false);
    }

    #[test]
    fn gate_outcome_partial_eq() {
        assert_eq!(GateOutcome::Passed, GateOutcome::Passed);
        assert_ne!(GateOutcome::Passed, GateOutcome::Failed);
        assert_ne!(GateOutcome::TimedOut, GateOutcome::Error);
    }

    #[test]
    fn structured_gate_result_clone() {
        let result = StructuredGateResult {
            outcome: GateOutcome::Passed,
            output: serde_json::json!({"exit_code": 0, "error": ""}),
        };
        let cloned = result.clone();
        assert_eq!(cloned.outcome, GateOutcome::Passed);
        assert_eq!(cloned.output, result.output);
    }

    // -----------------------------------------------------------------------
    // built_in_default tests
    // -----------------------------------------------------------------------

    #[test]
    fn built_in_default_command_gate() {
        let val = built_in_default(GATE_TYPE_COMMAND);
        assert!(val.is_some());
        let v = val.unwrap();
        assert_eq!(v["exit_code"], 0);
        assert_eq!(v["error"], "");
    }

    #[test]
    fn built_in_default_context_exists_gate() {
        let val = built_in_default(GATE_TYPE_CONTEXT_EXISTS);
        assert!(val.is_some());
        let v = val.unwrap();
        assert_eq!(v["exists"], true);
        assert_eq!(v["error"], "");
    }

    #[test]
    fn built_in_default_context_matches_gate() {
        let val = built_in_default(GATE_TYPE_CONTEXT_MATCHES);
        assert!(val.is_some());
        let v = val.unwrap();
        assert_eq!(v["matches"], true);
        assert_eq!(v["error"], "");
    }

    #[test]
    fn built_in_default_unknown_gate_type_returns_none() {
        assert!(built_in_default("unknown-gate-type").is_none());
        assert!(built_in_default("").is_none());
        assert!(built_in_default("custom").is_none());
    }

    #[test]
    fn mixed_gate_types_all_evaluated() {
        let dir = tmp_dir();
        let store = MockContextStore::new();
        store.insert("sess1", "ready.txt", b"ready");

        let mut gates = BTreeMap::new();
        gates.insert(
            "cmd".to_string(),
            Gate {
                gate_type: GATE_TYPE_COMMAND.to_string(),
                command: "exit 0".to_string(),
                timeout: 5,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
                completion: None,
                name_filter: None,
                overridable: true,
                request: String::new(),
                leg: String::new(),
                expect: None,
            },
        );
        gates.insert(
            "ctx_exists".to_string(),
            Gate {
                gate_type: GATE_TYPE_CONTEXT_EXISTS.to_string(),
                command: String::new(),
                timeout: 0,
                key: "ready.txt".to_string(),
                pattern: String::new(),
                override_default: None,
                completion: None,
                name_filter: None,
                overridable: true,
                request: String::new(),
                leg: String::new(),
                expect: None,
            },
        );
        gates.insert(
            "ctx_matches".to_string(),
            Gate {
                gate_type: GATE_TYPE_CONTEXT_MATCHES.to_string(),
                command: String::new(),
                timeout: 0,
                key: "ready.txt".to_string(),
                pattern: "ready".to_string(),
                override_default: None,
                completion: None,
                name_filter: None,
                overridable: true,
                request: String::new(),
                leg: String::new(),
                expect: None,
            },
        );

        let results = evaluate_gates(&gates, dir.path(), Some(&store), Some("sess1"), None);
        assert_eq!(results.len(), 3);
        assert_eq!(results["cmd"].outcome, GateOutcome::Passed);
        assert_eq!(results["ctx_exists"].outcome, GateOutcome::Passed);
        assert_eq!(results["ctx_matches"].outcome, GateOutcome::Passed);
    }

    // -----------------------------------------------------------------
    // request-leg gates
    // -----------------------------------------------------------------

    mod request_leg {
        use super::super::*;
        use crate::engine::request_store::{
            abandon_leg, attach_leg, bind_leg, close_request, create_request, record_refusal,
            record_result, AbandonLeg, AttachLeg, AttachingSession, BindLeg, CloseRequest,
            LegRefusal, LegResult, LegSpec, NewRequest, RequestBounds,
        };
        use crate::engine::types::{
            LegDeclaration, TemplateIdentity, TerminalOutcome, WorkflowResult,
        };
        use crate::template::types::{gate_type_schema, GATE_TYPE_REQUEST_LEG};
        use std::collections::{BTreeMap, BTreeSet, HashMap};

        const TS: &str = "2026-01-01T00:00:00.000Z";

        fn gate(request: &str, leg: &str) -> Gate {
            Gate {
                gate_type: GATE_TYPE_REQUEST_LEG.to_string(),
                command: String::new(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
                completion: None,
                name_filter: None,
                overridable: false,
                request: request.to_string(),
                leg: leg.to_string(),
                expect: None,
            }
        }

        fn with_expect(mut g: Gate, pairs: &[(&str, &[&str])]) -> Gate {
            let mut expect = BTreeMap::new();
            for (key, values) in pairs {
                expect.insert(
                    key.to_string(),
                    values.iter().map(|v| serde_json::json!(v)).collect(),
                );
            }
            g.expect = Some(expect);
            g
        }

        /// A one-leg request (`scope`) in a fresh store.
        fn seed(root: &Path) -> String {
            let spec = NewRequest {
                requested_by: "coord".to_string(),
                coordinator_of_record: "coord".to_string(),
                legs: vec![LegSpec {
                    name: "scope".to_string(),
                    declaration: LegDeclaration {
                        role: "scope".to_string(),
                        template: "scope.md".into(),
                        inputs: serde_json::json!("brief"),
                    },
                }],
                inputs: None,
                created_at: TS.to_string(),
            };
            create_request(root, &spec, &RequestBounds::default())
                .unwrap()
                .as_str()
                .to_string()
        }

        fn id(raw: &str) -> ValidatedRequestId {
            ValidatedRequestId::new(raw).unwrap()
        }

        fn resolve_explicit(root: &Path, req: &str, payload: Option<serde_json::Value>) {
            record_result(
                root,
                &id(req),
                &LegResult {
                    leg_name: "scope".to_string(),
                    result: WorkflowResult {
                        status: TerminalOutcome::Success,
                        summary: "done".to_string(),
                        payload,
                    },
                    source: LegResultSource::Explicit,
                    issued_by: None,
                    timestamp: TS.to_string(),
                    final_state: Some("ignored for explicit".to_string()),
                },
            )
            .unwrap();
        }

        /// Self-attach a root session to the leg, then promote its result
        /// from `final_state`.
        fn attach_and_promote(
            root: &Path,
            req: &str,
            final_state: &str,
            payload: serde_json::Value,
        ) {
            attach_leg(
                root,
                &id(req),
                &AttachLeg {
                    leg_name: "scope".to_string(),
                    session: AttachingSession {
                        session_id: "root-1".to_string(),
                        template: Some(TemplateIdentity {
                            name: Some("scope".to_string()),
                            hash: "abc".to_string(),
                            source: "scope.md".to_string(),
                        }),
                        variables: BTreeMap::new(),
                        bindings: HashMap::new(),
                        terminal_state: None,
                        pointer: None,
                    },
                    issued_by: None,
                    timestamp: TS.to_string(),
                },
            )
            .unwrap();
            record_result(
                root,
                &id(req),
                &LegResult {
                    leg_name: "scope".to_string(),
                    result: WorkflowResult {
                        status: TerminalOutcome::Success,
                        summary: "scoped".to_string(),
                        payload: Some(payload),
                    },
                    source: LegResultSource::Promoted,
                    issued_by: None,
                    timestamp: TS.to_string(),
                    final_state: Some(final_state.to_string()),
                },
            )
            .unwrap();
        }

        fn eval(root: &Path, g: &Gate) -> StructuredGateResult {
            evaluate_request_leg_gate(g, Some(root))
        }

        fn keys(v: &serde_json::Value) -> BTreeSet<String> {
            v.as_object().unwrap().keys().cloned().collect()
        }

        #[test]
        fn request_leg_output_keys_match_the_schema() {
            let schema: BTreeSet<String> = gate_type_schema(GATE_TYPE_REQUEST_LEG)
                .unwrap()
                .iter()
                .map(|(name, _)| name.to_string())
                .collect();
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let req = seed(root);
            // Every shape the evaluator emits: no store, a bad id, missing,
            // open, resolved.
            let outputs = [
                evaluate_request_leg_gate(&gate(&req, "scope"), None).output,
                eval(root, &gate("BAD", "scope")).output,
                eval(root, &gate("req-nope", "scope")).output,
                eval(root, &gate(&req, "scope")).output,
            ];
            resolve_explicit(root, &req, Some(serde_json::json!({"outcome": "scoped"})));
            let resolved = eval(root, &gate(&req, "scope")).output;
            for out in outputs.iter().chain(std::iter::once(&resolved)) {
                assert_eq!(keys(out), schema, "output {out} must match the schema");
                // And each value has the schema's type.
                for (name, t) in gate_type_schema(GATE_TYPE_REQUEST_LEG).unwrap() {
                    let v = &out[*name];
                    let ok = match t {
                        crate::template::types::GateSchemaFieldType::Boolean => v.is_boolean(),
                        crate::template::types::GateSchemaFieldType::Str => v.is_string(),
                        crate::template::types::GateSchemaFieldType::Object => v.is_object(),
                        crate::template::types::GateSchemaFieldType::Number => v.is_number(),
                        crate::template::types::GateSchemaFieldType::Array => v.is_array(),
                    };
                    assert!(ok, "field {name} has the wrong type in {out}");
                }
            }
            // The built-in default has the same shape.
            assert_eq!(
                keys(&built_in_default(GATE_TYPE_REQUEST_LEG).unwrap()),
                schema
            );
        }

        #[test]
        fn no_request_store_is_an_error_not_a_pass() {
            let r = evaluate_request_leg_gate(&gate("req-a", "scope"), None);
            assert_eq!(r.outcome, GateOutcome::Error);
            assert_eq!(r.output["found"], false);
            assert!(!r.output["error"].as_str().unwrap().is_empty());
        }

        #[test]
        fn evaluate_gates_without_a_store_reports_an_error() {
            let dir = tempfile::tempdir().unwrap();
            let mut gates = BTreeMap::new();
            gates.insert("leg".to_string(), gate("req-a", "scope"));
            let results = evaluate_gates(&gates, dir.path(), None, None, None);
            assert_eq!(results["leg"].outcome, GateOutcome::Error);
            assert!(
                !results["leg"].output["error"]
                    .as_str()
                    .unwrap()
                    .contains("unsupported"),
                "request-leg must not fall through to the unsupported-type arm"
            );
        }

        #[test]
        fn a_bad_substituted_value_is_an_error_and_reads_nothing() {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            for g in [
                gate("Upper-Case", "scope"),
                gate("../escape", "scope"),
                gate("", "scope"),
                gate("req-a", "-leg"),
                gate("req-a", ""),
            ] {
                let r = eval(root, &g);
                assert_eq!(r.outcome, GateOutcome::Error, "{g:?}");
                assert_eq!(r.output["found"], false);
                assert_eq!(r.output["disposition"], "");
                assert!(!r.output["error"].as_str().unwrap().is_empty());
            }
            // Nothing was created under the root.
            assert!(!root.join("requests").exists());
        }

        #[test]
        fn a_missing_request_or_leg_reports_missing() {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let r = eval(root, &gate("req-nope", "scope"));
            assert_eq!(r.outcome, GateOutcome::Failed);
            assert_eq!(r.output["found"], false);
            assert_eq!(r.output["disposition"], "missing");
            assert!(r.output["error"].as_str().unwrap().contains("req-nope"));

            let req = seed(root);
            let r = eval(root, &gate(&req, "execute"));
            assert_eq!(r.outcome, GateOutcome::Failed);
            assert_eq!(r.output["found"], false);
            assert_eq!(r.output["disposition"], "missing");
            assert!(r.output["error"].as_str().unwrap().contains("execute"));
        }

        #[test]
        fn an_open_leg_blocks_and_reports_whether_it_is_bound() {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let req = seed(root);
            let r = eval(root, &gate(&req, "scope"));
            assert_eq!(r.outcome, GateOutcome::Failed);
            assert_eq!(r.output["found"], true);
            assert_eq!(r.output["disposition"], "open");
            assert_eq!(r.output["bound"], false);
            assert_eq!(r.output["valid"], false);

            bind_leg(
                root,
                &id(&req),
                &BindLeg {
                    leg_name: "scope".to_string(),
                    child_session_id: "child-1".to_string(),
                    dispatch_epoch: Some(1),
                    issued_by: None,
                    timestamp: TS.to_string(),
                },
            )
            .unwrap();
            let r = eval(root, &gate(&req, "scope"));
            assert_eq!(r.outcome, GateOutcome::Failed);
            assert_eq!(r.output["disposition"], "open");
            assert_eq!(r.output["bound"], true);
            assert_eq!(gate_blocking_category(GATE_TYPE_REQUEST_LEG), "temporal");
        }

        #[test]
        fn a_promoted_result_reports_its_payload_final_state_and_template() {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let req = seed(root);
            attach_and_promote(
                root,
                &req,
                "done_scoped",
                serde_json::json!({
                    "outcome": "scoped",
                    "step": "plan",
                    "reason": 7,
                    "pr": "https://example.test/pr/1",
                    "nested": {"deep": "yes"}
                }),
            );
            let r = eval(root, &gate(&req, "scope"));
            assert_eq!(r.outcome, GateOutcome::Passed);
            let out = &r.output;
            assert_eq!(out["found"], true);
            assert_eq!(out["disposition"], "resolved");
            assert_eq!(out["bound"], true);
            assert_eq!(out["source"], "promoted");
            assert_eq!(out["status"], "success");
            assert_eq!(out["final_state"], "done_scoped");
            assert_eq!(out["template"], "scope.md");
            assert_eq!(out["outcome"], "scoped");
            assert_eq!(out["step"], "plan");
            // Not a string, so it reads empty rather than as its JSON text.
            assert_eq!(out["reason"], "");
            assert_eq!(out["payload"]["pr"], "https://example.test/pr/1");
            assert_eq!(out["payload"]["nested"]["deep"], "yes");
            assert_eq!(out["valid"], true);
            assert_eq!(out["error"], "");
        }

        #[test]
        fn an_explicit_result_names_no_final_state_or_template() {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let req = seed(root);
            resolve_explicit(root, &req, None);
            let r = eval(root, &gate(&req, "scope"));
            assert_eq!(r.outcome, GateOutcome::Passed);
            assert_eq!(r.output["source"], "explicit");
            assert_eq!(r.output["final_state"], "");
            assert_eq!(r.output["template"], "");
            assert_eq!(r.output["payload"], serde_json::json!({}));
            assert_eq!(r.output["outcome"], "");
            assert_eq!(r.output["valid"], true);
        }

        #[test]
        fn a_refused_result_reports_source_refused() {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let req = seed(root);
            record_refusal(
                root,
                &id(&req),
                &LegRefusal {
                    leg_name: "scope".to_string(),
                    result: WorkflowResult::with_string_payload(
                        TerminalOutcome::Failure,
                        "refused",
                        [("outcome", "refused"), ("reason", "var_mismatch")],
                    ),
                    issued_by: None,
                    timestamp: TS.to_string(),
                },
            )
            .unwrap();
            let r = eval(root, &gate(&req, "scope"));
            assert_eq!(r.outcome, GateOutcome::Passed);
            assert_eq!(r.output["source"], "refused");
            assert_eq!(r.output["status"], "failure");
            assert_eq!(r.output["outcome"], "refused");
            assert_eq!(r.output["reason"], "var_mismatch");
            assert_eq!(r.output["final_state"], "");
            assert_eq!(r.output["template"], "");
        }

        #[test]
        fn an_abandoned_leg_passes_as_abandoned() {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let req = seed(root);
            abandon_leg(
                root,
                &id(&req),
                &AbandonLeg {
                    leg_name: "scope".to_string(),
                    rationale: "superseded".to_string(),
                    issued_by: None,
                    timestamp: TS.to_string(),
                },
            )
            .unwrap();
            let r = eval(root, &gate(&req, "scope"));
            assert_eq!(r.outcome, GateOutcome::Passed);
            assert_eq!(r.output["disposition"], "abandoned");
            assert_eq!(r.output["valid"], false);
        }

        #[test]
        fn an_open_leg_on_an_abandoned_request_passes_as_abandoned() {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let req = seed(root);
            close_request(
                root,
                &id(&req),
                &CloseRequest {
                    disposition: Some(CloseDisposition::RequestAbandoned),
                    issued_by: None,
                    timestamp: TS.to_string(),
                },
            )
            .unwrap();
            let r = eval(root, &gate(&req, "scope"));
            assert_eq!(r.outcome, GateOutcome::Passed);
            assert_eq!(r.output["disposition"], "abandoned");
        }

        #[test]
        fn valid_follows_expect() {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let expect: &[(&str, &[&str])] = &[("outcome", &["scoped", "declined"])];

            let cases: Vec<(Option<serde_json::Value>, bool, &str)> = vec![
                (
                    Some(serde_json::json!({"outcome": "scoped"})),
                    true,
                    "listed value",
                ),
                (
                    Some(serde_json::json!({"outcome": "scoped", "extra": "x"})),
                    true,
                    "extra key expect does not name",
                ),
                (
                    Some(serde_json::json!({"outcome": "other"})),
                    false,
                    "value outside the list",
                ),
                (
                    Some(serde_json::json!({"step": "plan"})),
                    false,
                    "missing key",
                ),
                (
                    Some(serde_json::json!("scoped")),
                    false,
                    "non-object payload",
                ),
                (None, false, "no payload at all"),
            ];
            for (payload, want, label) in cases {
                let req = seed(root);
                resolve_explicit(root, &req, payload);
                let r = eval(root, &with_expect(gate(&req, "scope"), expect));
                assert_eq!(r.output["valid"], want, "{label}");
                assert!(r.output["payload"].is_object(), "{label}");
            }

            // No expect: any resolved object payload is valid, and a
            // non-object one is not.
            let req = seed(root);
            resolve_explicit(root, &req, Some(serde_json::json!({"anything": 1})));
            assert_eq!(eval(root, &gate(&req, "scope")).output["valid"], true);
            let req = seed(root);
            resolve_explicit(root, &req, Some(serde_json::json!([1, 2])));
            assert_eq!(eval(root, &gate(&req, "scope")).output["valid"], false);

            // Non-resolved dispositions are never valid, expect or not.
            let req = seed(root);
            assert_eq!(
                eval(root, &with_expect(gate(&req, "scope"), expect)).output["valid"],
                false
            );
        }

        #[test]
        fn evaluating_the_gate_never_writes_to_the_request_log() {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let req = seed(root);
            let revision =
                |root: &Path| request_store::read_view(root, &id(&req)).unwrap().revision;
            let log = root.join("requests").join(&req).join("request.jsonl");
            let before_open = (revision(root), std::fs::read(&log).unwrap());
            eval(root, &gate(&req, "scope"));
            eval(root, &gate(&req, "scope"));
            assert_eq!(before_open, (revision(root), std::fs::read(&log).unwrap()));

            resolve_explicit(root, &req, Some(serde_json::json!({"outcome": "scoped"})));
            let before_resolved = (revision(root), std::fs::read(&log).unwrap());
            eval(root, &gate(&req, "scope"));
            assert_eq!(
                before_resolved,
                (revision(root), std::fs::read(&log).unwrap())
            );
        }

        #[test]
        fn built_in_default_is_a_resolved_valid_record_naming_no_outcome() {
            let d = built_in_default(GATE_TYPE_REQUEST_LEG).unwrap();
            assert_eq!(d["found"], true);
            assert_eq!(d["disposition"], "resolved");
            assert_eq!(d["bound"], true);
            assert_eq!(d["source"], "promoted");
            assert_eq!(d["valid"], true);
            assert_eq!(d["payload"], serde_json::json!({}));
            assert_eq!(d["outcome"], "");
            assert_eq!(d["error"], "");
        }
    }
}
