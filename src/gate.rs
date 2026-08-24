//! Gate evaluator for command, context-exists, and context-matches gates.
//!
//! Command gates spawn shell commands in isolated process groups with
//! configurable timeouts. Context gates check the session context store.
//! Evaluates all gates without short-circuiting so callers see every blocking
//! condition in a single response.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::action::{run_shell_command, CommandOutput, FailureKind};
use crate::session::context::ContextStore;
use crate::template::types::{
    Gate, GATE_TYPE_CHILDREN_COMPLETE, GATE_TYPE_COMMAND, GATE_TYPE_CONTEXT_EXISTS,
    GATE_TYPE_CONTEXT_MATCHES,
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
    let mut results = BTreeMap::new();
    for (name, gate) in gates {
        let result = match gate.gate_type.as_str() {
            GATE_TYPE_COMMAND => evaluate_command_gate(gate, working_dir),
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
                        "unsupported gate type '{}'; only command, context-exists, \
                         context-matches, and children-complete gates are evaluated",
                        other
                    )
                }),
            },
        };
        results.insert(name.clone(), result);
    }
    results
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
/// reference is now how an unusable key arises. The two character sets do not
/// line up: a variable value may hold a space, a `:` or an `@`, and
/// `validate_context_key` allows none of them; a key must also start each
/// `/`-separated component with an alphanumeric, so `{{PREFIX}}-note` with an
/// empty prefix leaves `-note`, which the store refuses. Whether the two sets
/// should converge is Issue #227; this only stops the refusal being silent.
///
/// `field` names the evidence key the gate's shape carries, so a caller reading
/// `exists` or `matches` still finds it.
fn unusable_key_result(key: &str, field: &str) -> Option<StructuredGateResult> {
    let err = crate::session::validate::validate_context_key(key).err()?;
    Some(StructuredGateResult {
        outcome: GateOutcome::Error,
        output: serde_json::json!({
            field: false,
            "error": format!(
                "context gate key {:?} is not a usable context key: {}\n  \
                 remedy: this is the key after substitution, so check what any \
                 {{{{KEY}}}} reference in the gate's key resolved to -- an unset \
                 optional variable leaves nothing behind. A variable value may \
                 hold a space, ':' or '@'; a context key may not",
                key, err
            )
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
        _ => None,
    }
}

/// Return the blocking condition category for a gate type.
///
/// `"temporal"` means the blocking condition is time-dependent (retry later).
/// `"corrective"` means the agent needs to take corrective action.
pub fn gate_blocking_category(gate_type: &str) -> &'static str {
    match gate_type {
        GATE_TYPE_CHILDREN_COMPLETE => "temporal",
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
            },
        );

        let results = evaluate_gates(&gates, dir.path(), Some(&store), Some("sess1"), None);
        assert_eq!(results.len(), 3);
        assert_eq!(results["cmd"].outcome, GateOutcome::Passed);
        assert_eq!(results["ctx_exists"].outcome, GateOutcome::Passed);
        assert_eq!(results["ctx_matches"].outcome, GateOutcome::Passed);
    }
}
