//! Gate evaluator for command gates.
//!
//! Spawns shell commands in isolated process groups with configurable timeouts.
//! Evaluates all gates without short-circuiting so callers see every blocking
//! condition in a single response.

use std::collections::BTreeMap;
use std::path::Path;

use crate::action::run_shell_command;
use crate::template::types::Gate;

/// Result of evaluating a single gate command.
#[derive(Debug, Clone, PartialEq)]
pub enum GateResult {
    /// The command exited with status 0.
    Passed,
    /// The command exited with a non-zero status.
    Failed { exit_code: i32 },
    /// The command did not finish within the configured timeout.
    TimedOut,
    /// The command could not be spawned or an OS error occurred.
    Error { message: String },
}

/// Evaluate all command gates in `gates`, running each command with
/// `working_dir` as the current directory. Every gate is evaluated regardless
/// of individual results (no short-circuit).
///
/// Only gates with `gate_type == "command"` are evaluated. Gates with
/// unrecognized types are skipped with a `GateResult::Error` explaining that
/// the type is not supported by this evaluator.
pub fn evaluate_gates(
    gates: &BTreeMap<String, Gate>,
    working_dir: &Path,
) -> BTreeMap<String, GateResult> {
    let mut results = BTreeMap::new();
    for (name, gate) in gates {
        let result = if gate.gate_type != crate::template::types::GATE_TYPE_COMMAND {
            GateResult::Error {
                message: format!(
                    "unsupported gate type '{}'; only command gates are evaluated",
                    gate.gate_type
                ),
            }
        } else {
            evaluate_single_gate(gate, working_dir)
        };
        results.insert(name.clone(), result);
    }
    results
}

fn evaluate_single_gate(gate: &Gate, working_dir: &Path) -> GateResult {
    let output = run_shell_command(&gate.command, working_dir, gate.timeout);

    if output.exit_code == -1 {
        // Distinguish timeout from spawn/wait errors by checking the message.
        if output.stderr.contains("timed out") {
            GateResult::TimedOut
        } else {
            GateResult::Error {
                message: output.stderr,
            }
        }
    } else if output.exit_code == 0 {
        GateResult::Passed
    } else {
        GateResult::Failed {
            exit_code: output.exit_code,
        }
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

        let results = evaluate_gates(&gates, dir.path());
        assert_eq!(results.len(), 1);
        assert_eq!(results["check"], GateResult::Passed);
    }

    #[test]
    fn failing_gate() {
        let dir = tmp_dir();
        let mut gates = BTreeMap::new();
        gates.insert("check".to_string(), make_gate("exit 42", 5));

        let results = evaluate_gates(&gates, dir.path());
        assert_eq!(results.len(), 1);
        assert_eq!(results["check"], GateResult::Failed { exit_code: 42 });
    }

    #[test]
    fn timed_out_gate() {
        let dir = tmp_dir();
        let mut gates = BTreeMap::new();
        gates.insert("slow".to_string(), make_gate("sleep 60", 1));

        let results = evaluate_gates(&gates, dir.path());
        assert_eq!(results.len(), 1);
        assert_eq!(results["slow"], GateResult::TimedOut);
    }

    #[test]
    fn error_gate_nonexistent_command() {
        let dir = tmp_dir();
        let mut gates = BTreeMap::new();
        gates.insert("bad".to_string(), make_gate("nonexistent_cmd_xyz_12345", 5));

        let results = evaluate_gates(&gates, dir.path());
        assert_eq!(results.len(), 1);
        match &results["bad"] {
            // The shell itself exits 127 for command-not-found.
            GateResult::Failed { exit_code } => {
                assert_eq!(*exit_code, 127);
            }
            other => panic!("expected Failed with exit_code 127, got {:?}", other),
        }
    }

    #[test]
    fn multiple_gates_mixed_results() {
        let dir = tmp_dir();
        let mut gates = BTreeMap::new();
        gates.insert("pass".to_string(), make_gate("exit 0", 5));
        gates.insert("fail".to_string(), make_gate("exit 1", 5));
        gates.insert("timeout".to_string(), make_gate("sleep 60", 1));

        let results = evaluate_gates(&gates, dir.path());
        assert_eq!(results.len(), 3);
        assert_eq!(results["pass"], GateResult::Passed);
        assert_eq!(results["fail"], GateResult::Failed { exit_code: 1 });
        assert_eq!(results["timeout"], GateResult::TimedOut);
    }

    #[test]
    fn gate_runs_in_working_dir() {
        let dir = tmp_dir();
        // Create a marker file in the temp dir.
        std::fs::write(dir.path().join("marker.txt"), "found").unwrap();

        let mut gates = BTreeMap::new();
        gates.insert("check_dir".to_string(), make_gate("test -f marker.txt", 5));

        let results = evaluate_gates(&gates, dir.path());
        assert_eq!(results["check_dir"], GateResult::Passed);
    }

    #[test]
    fn default_timeout_used_when_zero() {
        // We can't easily test the 30s default without waiting, but we can
        // verify a gate with timeout=0 still works (uses default).
        let dir = tmp_dir();
        let mut gates = BTreeMap::new();
        gates.insert("quick".to_string(), make_gate("exit 0", 0));

        let results = evaluate_gates(&gates, dir.path());
        assert_eq!(results["quick"], GateResult::Passed);
    }
}
