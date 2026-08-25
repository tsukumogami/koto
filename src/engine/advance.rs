// Auto-advancement engine: transition resolution and advancement loop.
//
// Implemented for Issue #49.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::action::FailureKind;
use crate::engine::persistence::derive_overrides;
use crate::engine::substitute::{GateCaptureRefusal, VariableOverlay};
use crate::engine::types::{now_iso8601, Event, EventPayload};
use crate::gate::{GateOutcome, StructuredGateResult};
use crate::template::types::{
    is_is_set_matcher, is_present_matcher, ActionDecl, CompiledTemplate, TemplateState,
    ACTION_CONDITION_NAME, EVIDENCE_NAMESPACE, GATES_EVIDENCE_NAMESPACE, VARS_NAMESPACE,
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

/// Which part of a `default_action` a refusal names.
///
/// `command` and `working_dir` both carry `{{KEY}}` references and both are
/// validated against the same reference set at compile time, so an author reads
/// them as behaving alike. They do, and this is what lets the stop say which of
/// the two it was without the operator having to guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionField {
    Command,
    WorkingDir,
}

impl ActionField {
    /// The field's name as an author writes it in a template.
    pub fn as_str(self) -> &'static str {
        match self {
            ActionField::Command => "command",
            ActionField::WorkingDir => "working_dir",
        }
    }
}

/// Result of executing a default action.
#[derive(Debug, Clone)]
pub enum ActionResult {
    /// Action executed successfully. `command` is the substituted string that
    /// actually ran, carried for the same reason [`Failed`](Self::Failed)
    /// carries it: a capture taken from this output can still fail, and the
    /// report has to name what ran rather than what the template declared.
    Executed {
        command: String,
        exit_code: i32,
        stdout: String,
        stderr: String,
        truncated: bool,
    },
    /// Action was skipped (override evidence existed).
    Skipped,
    /// Action executed but requires user confirmation before continuing.
    RequiresConfirmation {
        command: String,
        exit_code: i32,
        stdout: String,
        stderr: String,
        truncated: bool,
    },
    /// The action did not run, because a `{{KEY}}` in it names a capture no
    /// state has delivered on this run.
    ///
    /// Distinct from [`Failed`](Self::Failed) because nothing was spawned:
    /// there is no exit code, no output, and no `DefaultActionExecuted` event
    /// to append. The caller turns this into the same typed run-time stop a
    /// directive reading an undelivered name already gets, rather than into an
    /// `__action__` failure -- the author's `fallback` prose describes a
    /// command that ran and failed, and an `__action__` condition is routable,
    /// so a template could carry the run past the defect with the value still
    /// unset (Issue #221).
    Refused {
        field: ActionField,
        key: String,
        producer: String,
    },
    /// The action did not run, because a `{{KEY}}` in one of this state's gate
    /// fields names a capture no state has delivered on this run.
    ///
    /// A polling action's gates are substituted before its command is spawned,
    /// so the refusal is raised on the way into the action rather than out of
    /// it. It is a separate variant from [`Refused`](Self::Refused) because
    /// [`ActionField`] means "a field of a `default_action`" and the renderer
    /// for that variant writes a sentence about one; a gate refusal arriving
    /// there would describe an action for a defect in a gate (Issue #225).
    GateRefused {
        gate: String,
        field: &'static str,
        key: String,
        producer: String,
    },
    /// The action did not succeed. `command` is the substituted string that
    /// actually ran, so the response reports what happened rather than what
    /// the template declared.
    Failed {
        command: String,
        failure_kind: FailureKind,
        exit_code: i32,
        stdout: String,
        stderr: String,
        truncated: bool,
    },
}

/// Build the synthetic condition map a failed action stops on.
///
/// The failure routes through `StopReason::GateBlocked` under the reserved
/// name `__action__` (DESIGN-koto-runs-commands.md Decision 3), which is why
/// no eighth `NextResponse` variant exists. `exit_code` is present only for a
/// non-zero exit: for a spawn failure, a timeout, or a wait error the runner
/// never obtained a status, and reporting the synthetic `-1` would be the
/// conflation `failure_kind` exists to end.
fn action_failure_conditions(
    state: &str,
    command: &str,
    failure_kind: FailureKind,
    exit_code: i32,
    stdout: &str,
    stderr: &str,
    truncated: bool,
) -> BTreeMap<String, StructuredGateResult> {
    // The outcome only sets the condition's `status` string. `failure_kind`
    // in the payload is the discriminator agents route on; this keeps the
    // status column honest for a reader skimming the list.
    let outcome = match failure_kind {
        FailureKind::NonzeroExit => GateOutcome::Failed,
        FailureKind::TimedOut => GateOutcome::TimedOut,
        FailureKind::SpawnFailed | FailureKind::WaitFailed => GateOutcome::Error,
    };
    action_condition(
        state,
        command,
        failure_kind.as_str(),
        outcome,
        (failure_kind == FailureKind::NonzeroExit).then_some(exit_code),
        stdout,
        stderr,
        truncated,
        None,
    )
}

/// Build the `__action__` condition map for a capture that could not be
/// delivered.
///
/// A failed capture is an action failure, not a skip: the command's output is
/// never silently dropped, and an author's `fallback` prose is delivered for
/// every reason the step did not work (DESIGN-koto-runs-commands.md, "Capture
/// delivery and its three failure cases"). The command itself exited zero, so
/// no `exit_code` is reported -- there is no failing status to report, and
/// `capture_error` says what actually went wrong.
fn capture_failure_conditions(
    state: &str,
    command: &str,
    stdout: &str,
    stderr: &str,
    truncated: bool,
    error: &CaptureError,
) -> BTreeMap<String, StructuredGateResult> {
    action_condition(
        state,
        command,
        CAPTURE_FAILED_KIND,
        GateOutcome::Failed,
        None,
        stdout,
        stderr,
        truncated,
        Some(error.to_json()),
    )
}

/// Assemble one `__action__` condition. Both action-failure shapes go through
/// here so the payload's field set cannot drift between them.
#[allow(clippy::too_many_arguments)]
fn action_condition(
    state: &str,
    command: &str,
    failure_kind: &str,
    outcome: GateOutcome,
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
    truncated: bool,
    capture_error: Option<serde_json::Value>,
) -> BTreeMap<String, StructuredGateResult> {
    let mut output = serde_json::Map::new();
    output.insert("state".to_string(), serde_json::json!(state));
    output.insert("command".to_string(), serde_json::json!(command));
    output.insert("failure_kind".to_string(), serde_json::json!(failure_kind));
    if let Some(code) = exit_code {
        output.insert("exit_code".to_string(), serde_json::json!(code));
    }
    output.insert("stdout".to_string(), serde_json::json!(stdout));
    output.insert("stderr".to_string(), serde_json::json!(stderr));
    output.insert("truncated".to_string(), serde_json::json!(truncated));
    if let Some(error) = capture_error {
        output.insert("capture_error".to_string(), error);
    }

    let mut map = BTreeMap::new();
    map.insert(
        ACTION_CONDITION_NAME.to_string(),
        StructuredGateResult {
            outcome,
            output: serde_json::Value::Object(output),
        },
    );
    map
}

/// Largest captured value the engine will deliver, in bytes.
///
/// Deliberately far below the 64KB response bound: a capture is a token that
/// lands in prose and possibly in a shell word, and the value allowlist
/// already rules out newlines, so anything approaching this bound is a
/// template mistake rather than a value worth carrying.
pub const MAX_CAPTURE_BYTES: usize = 4096;

/// The `failure_kind` a capture failure reports.
///
/// It is a wire string rather than a [`FailureKind`] variant because the
/// runner can never produce it: the command ran and exited zero, and only
/// delivery failed.
const CAPTURE_FAILED_KIND: &str = "capture_failed";

/// Why a capture could not be delivered.
#[derive(Debug, Clone, PartialEq)]
pub enum CaptureError {
    /// The command wrote nothing, or nothing but whitespace.
    Empty { key: String },
    /// The trimmed output is larger than [`MAX_CAPTURE_BYTES`].
    TooLarge { key: String, bytes: usize },
    /// The trimmed output holds a character the variable allowlist forbids --
    /// a newline among them, which is why a multi-line capture is not
    /// representable and trimming is mandatory rather than a courtesy.
    /// `position` is the 0-based character index of the first such character.
    DisallowedCharacter {
        key: String,
        position: usize,
        character: String,
    },
}

impl CaptureError {
    /// The `capture_error` object carried in the `__action__` payload. `case`
    /// is the discriminator; the remaining fields are what an author needs to
    /// find the offending output.
    fn to_json(&self) -> serde_json::Value {
        match self {
            CaptureError::Empty { key } => serde_json::json!({
                "key": key,
                "case": "empty",
            }),
            CaptureError::TooLarge { key, bytes } => serde_json::json!({
                "key": key,
                "case": "too_large",
                "bytes": bytes,
                "limit": MAX_CAPTURE_BYTES,
            }),
            CaptureError::DisallowedCharacter {
                key,
                position,
                character,
            } => serde_json::json!({
                "key": key,
                "case": "disallowed_character",
                "position": position,
                "character": character,
            }),
        }
    }
}

/// Prepare a command's stdout for delivery under `key`.
///
/// The order is fixed (DESIGN-koto-runs-commands.md, "Capture delivery and its
/// three failure cases"): trim, reject empty, reject oversize, then run the
/// value through the same `validate_value` allowlist every declared variable
/// passes. Reusing that function rather than restating the character set means
/// a future widening is a single reviewed change both paths inherit.
pub fn prepare_capture(key: &str, stdout: &str) -> Result<String, CaptureError> {
    let value = stdout.trim();
    if value.is_empty() {
        return Err(CaptureError::Empty {
            key: key.to_string(),
        });
    }
    if value.len() > MAX_CAPTURE_BYTES {
        return Err(CaptureError::TooLarge {
            key: key.to_string(),
            bytes: value.len(),
        });
    }
    if crate::engine::substitute::validate_value(key, value).is_err() {
        // Locate the offending character by asking the same allowlist about
        // one character at a time, rather than restating the character set
        // here. The scan stops at the first rejection and only runs on the
        // failure path, where a bounded value has already been read.
        let (position, character) = value
            .chars()
            .enumerate()
            .find(|(_, c)| crate::engine::substitute::validate_value(key, &c.to_string()).is_err())
            .map(|(i, c)| (i, c.to_string()))
            .unwrap_or((0, String::new()));
        return Err(CaptureError::DisallowedCharacter {
            key: key.to_string(),
            position,
            character,
        });
    }
    Ok(value.to_string())
}

/// Deliver a state's captured stdout, or report why it could not be.
///
/// Returns `Ok(None)` when the state declares no capture name or the value
/// was delivered, and `Ok(Some(conditions))` when delivery failed and the tick
/// must stop at this state with an `__action__` condition.
///
/// The event and the overlay are written in the same step, so the durable
/// record and the view the rest of this tick reads can never disagree. The
/// event goes first: a value the rest of the tick can see but the log does not
/// hold would survive exactly one tick and then vanish.
#[allow(clippy::too_many_arguments)]
fn deliver_capture<F>(
    state: &str,
    action: &ActionDecl,
    command: &str,
    stdout: &str,
    stderr: &str,
    truncated: bool,
    overlay: &VariableOverlay,
    append_event: &mut F,
) -> Result<Option<BTreeMap<String, StructuredGateResult>>, AdvanceError>
where
    F: FnMut(&EventPayload) -> Result<(), String>,
{
    let Some(key) = &action.capture_stdout_as else {
        return Ok(None);
    };
    match prepare_capture(key, stdout) {
        Ok(value) => {
            append_event(&EventPayload::VariableCaptured {
                key: key.clone(),
                value: value.clone(),
            })
            .map_err(AdvanceError::PersistenceError)?;
            overlay.insert(key.clone(), value);
            Ok(None)
        }
        Err(error) => Ok(Some(capture_failure_conditions(
            state, command, stdout, stderr, truncated, &error,
        ))),
    }
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
    ///
    /// `command` is the substituted string that ran, carried rather than
    /// re-derived: by the time the caller builds its response the overlay holds
    /// this state's own capture, so substituting the template text a second
    /// time can produce a command that never executed (Issue #220).
    ActionRequiresConfirmation {
        state: String,
        command: String,
        exit_code: i32,
        stdout: String,
        stderr: String,
    },
    /// A `default_action` was refused before it ran: a `{{KEY}}` in its
    /// `command` or `working_dir` names a capture no state has delivered on
    /// this run.
    ///
    /// Not a [`GateBlocked`](Self::GateBlocked) under `__action__`, which is
    /// where a failing action stops. Nothing ran, so there is nothing for the
    /// author's `fallback` prose to describe, and the caller reports it as the
    /// same authoring stop a directive gets for the same defect (Issue #221).
    ActionRefusedUnsetCapture {
        state: String,
        field: ActionField,
        key: String,
        producer: String,
    },
    /// A gate was refused before it was evaluated: a `{{KEY}}` in one of its
    /// substitutable fields names a capture no state has delivered on this run.
    ///
    /// Reached from two arms, and that is the point. The gate block reaches it
    /// when the evaluator refuses; the action block reaches it when a polling
    /// action's gate set refuses on the way in. Both carry the same five values
    /// and produce the same stop, so the advance loop and the polling loop
    /// cannot disagree about a reference -- which is where Issue #220 found
    /// them drifted.
    ///
    /// Deliberately not a [`GateBlocked`](Self::GateBlocked) result under the
    /// refusing gate's own name. Gate output is injected into the evidence map
    /// regardless of outcome so that `when` clauses can route on `gates.*`, and
    /// a recorded override can force one to pass, so a routable refusal is one
    /// a template could carry the run past with the value still unset. That is
    /// the argument Issue #221 recorded for the action case, and gate
    /// conditions are the routing surface rather than a single synthesized
    /// condition (Issue #225).
    GateRefusedUnsetCapture {
        state: String,
        gate: String,
        field: &'static str,
        key: String,
        producer: String,
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
/// 7. skip_if evaluation (if conditions met, auto-transition and continue loop)
/// 8. Transition resolution (match evidence against conditions)
///
/// I/O operations are injected as closures for testability:
/// - `append_event`: persist a state transition event
/// - `evaluate_gates`: run gate commands and return results
/// - `invoke_integration`: call an integration runner
/// - `execute_action`: run a default action command
///
/// `overlay` carries values produced during this tick. The loop re-reads it at
/// every iteration for `vars.*` when-clause evaluation, so a value written by
/// an earlier iteration routes the later ones. The caller holds the same
/// overlay for its gate and action command substitution and for the directive
/// it finally returns.
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
    overlay: &VariableOverlay,
    shutdown: &AtomicBool,
) -> Result<AdvanceResult, AdvanceError>
where
    F: FnMut(&EventPayload) -> Result<(), String>,
    G: Fn(
        &BTreeMap<String, crate::template::types::Gate>,
    ) -> Result<BTreeMap<String, StructuredGateResult>, GateCaptureRefusal>,
    I: Fn(&str) -> Result<serde_json::Value, IntegrationError>,
    A: Fn(&str, &ActionDecl, bool) -> ActionResult,
{
    let mut visited = HashSet::new();
    let mut state = current_state.to_string();
    let mut advanced = false;
    let mut transition_count: usize = 0;
    // Evidence is only used for the initial state; auto-advanced states start fresh.
    let mut current_evidence = evidence.clone();
    // Track whether the caller submitted fresh evidence this iteration. True on the
    // first iteration (agent submitted evidence) or when a state has no conditional
    // transitions (pure-routing states should still fire unconditionally). Set to
    // false after each auto-advance so states with conditional transitions require
    // their own evidence before the unconditional fallback fires.
    let mut fresh_evidence = !evidence.is_empty();

    // Extract template variables from the log for vars.* when-clause
    // evaluation (Issue #141): the WorkflowInitialized block plus every value
    // captured on an earlier tick. This is the base layer only: it is read
    // once, before the first iteration, so on its own it cannot see a value
    // produced by an earlier iteration of this same loop. Each iteration
    // layers the per-tick overlay over it below.
    let base_workflow_variables: std::collections::HashMap<String, String> =
        crate::engine::substitute::bindings_from_events(all_events);

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
                ActionResult::Executed {
                    command,
                    stdout,
                    stderr,
                    truncated,
                    ..
                } => {
                    // Deliver the capture, if the state declared a name, and
                    // then continue to gate evaluation.
                    if let Some(conditions) = deliver_capture(
                        &state,
                        action,
                        &command,
                        &stdout,
                        &stderr,
                        truncated,
                        overlay,
                        append_event,
                    )? {
                        return Ok(AdvanceResult {
                            final_state: state,
                            advanced,
                            stop_reason: StopReason::GateBlocked(conditions),
                        });
                    }
                }
                ActionResult::Skipped => {
                    // Continue to gate evaluation
                }
                ActionResult::Refused {
                    field,
                    key,
                    producer,
                } => {
                    // Stop where the refusal happened, before this state's
                    // gates, for the same reason the `Failed` arm does: a
                    // state's gates judge the work the action did, and the
                    // action did not happen. Unlike that arm there is no
                    // `__action__` condition to build -- nothing ran, so there
                    // is no command, exit code, or output to report, and the
                    // stop carries only what the operator needs to fix the
                    // template.
                    return Ok(AdvanceResult {
                        final_state: state.clone(),
                        advanced,
                        stop_reason: StopReason::ActionRefusedUnsetCapture {
                            state,
                            field,
                            key,
                            producer,
                        },
                    });
                }
                ActionResult::GateRefused {
                    gate,
                    field,
                    key,
                    producer,
                } => {
                    // A polling action substitutes this state's gates on the
                    // way in, so the refusal arrives before the command is
                    // spawned. Same stop as the gate block's own arm, carrying
                    // the same five values: the two positions meet here rather
                    // than at two stops that could drift.
                    return Ok(AdvanceResult {
                        final_state: state.clone(),
                        advanced,
                        stop_reason: StopReason::GateRefusedUnsetCapture {
                            state,
                            gate,
                            field,
                            key,
                            producer,
                        },
                    });
                }
                ActionResult::Failed {
                    command,
                    failure_kind,
                    exit_code,
                    stdout,
                    stderr,
                    truncated,
                } => {
                    // Stop at the state that ran the command, and do NOT
                    // evaluate this state's gates: a state's gates judge the
                    // work the action did, and the action did not happen.
                    // Running them would let a passing gate advance past a
                    // failed command, which is the silent advance R6 forbids
                    // (DESIGN-koto-runs-commands.md Decision 3).
                    //
                    // Because this returns before the gate block, an action
                    // failure can never be detected *by* a gate. There is no
                    // path on which a gate observes the failure the action
                    // already reported.
                    //
                    // This runs before the `requires_confirmation` branch
                    // below by construction -- the closure classifies a
                    // failure as `Failed` regardless of the flag -- so a
                    // failing action stops here whether or not confirmation
                    // was requested, and the confirm stop is reached only on
                    // success.
                    let conditions = action_failure_conditions(
                        &state,
                        &command,
                        failure_kind,
                        exit_code,
                        &stdout,
                        &stderr,
                        truncated,
                    );
                    return Ok(AdvanceResult {
                        final_state: state,
                        advanced,
                        stop_reason: StopReason::GateBlocked(conditions),
                    });
                }
                ActionResult::RequiresConfirmation {
                    command,
                    exit_code,
                    stdout,
                    stderr,
                    truncated,
                } => {
                    // The command ran and exited zero, so its capture is
                    // delivered here too. Confirming re-enters the state with
                    // evidence, which skips the action entirely -- capturing
                    // only on the unconfirmed path would mean a confirmed
                    // action never delivered its value at all.
                    if let Some(conditions) = deliver_capture(
                        &state,
                        action,
                        &command,
                        &stdout,
                        &stderr,
                        truncated,
                        overlay,
                        append_event,
                    )? {
                        return Ok(AdvanceResult {
                            final_state: state,
                            advanced,
                            stop_reason: StopReason::GateBlocked(conditions),
                        });
                    }
                    return Ok(AdvanceResult {
                        final_state: state.clone(),
                        advanced,
                        stop_reason: StopReason::ActionRequiresConfirmation {
                            state,
                            command,
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
        // True when this state has at least one transition whose when clause references
        // a gates.* key. False for legacy states (boolean pass/block only).
        // Initialized to false; set inside the gates block when gates are present.
        let mut has_gates_routing = false;
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
                // The evaluator resolves the gate's fields before it runs
                // anything, so a `{{KEY}}` naming an undelivered capture is
                // refused here rather than reaching a shell, the context store
                // or the regex engine. Stop where the refusal happened, before
                // any `GateEvaluated` event is appended: nothing ran, so there
                // is no gate result to report and nothing for a `when` clause
                // to route on (Issue #225).
                let evaluated = match evaluate_gates(&gates_to_evaluate) {
                    Ok(results) => results,
                    Err(refusal) => {
                        return Ok(AdvanceResult {
                            final_state: state.clone(),
                            advanced,
                            stop_reason: StopReason::GateRefusedUnsetCapture {
                                state,
                                gate: refusal.gate,
                                field: refusal.field,
                                key: refusal.key,
                                producer: refusal.producer,
                            },
                        });
                    }
                };
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

            // Determine whether this state uses structured gate routing: at least
            // one transition's when clause references a gates.* key. Used both to
            // decide whether to block immediately (when gates fail) and to guard
            // evidence injection — legacy states (no gates.* references) must not
            // have gate output merged into the resolver evidence map.
            has_gates_routing = template_state.transitions.iter().any(|t| {
                t.when
                    .as_ref()
                    .map(|w| {
                        w.keys()
                            .any(|k| k.starts_with(&format!("{}.", GATES_EVIDENCE_NAMESPACE)))
                    })
                    .unwrap_or(false)
            }) || template_state.skip_if.as_ref().is_some_and(|s| {
                s.keys()
                    .any(|k| k.starts_with(&format!("{}.", GATES_EVIDENCE_NAMESPACE)))
            });

            if any_failed {
                // If the state has an accepts block, fall through so the
                // transition resolver can use evidence as a fallback. The resolver
                // will skip unconditional transitions when gate_failed is true,
                // ensuring the agent must submit evidence when no conditional
                // transition matches.
                //
                // If neither condition holds, return GateBlocked immediately.
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

        // Evidence assembly (shared by steps 7 and 8)
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
        // Only inject gate output for structured-mode states (those with at least
        // one gates.* when-clause reference). Legacy states (boolean pass/block
        // only, no gates.* routing) must not have gate output in the resolver
        // evidence map — their transitions route on agent evidence alone.
        if !gate_evidence_map.is_empty() && has_gates_routing {
            merged.insert(
                "gates".to_string(),
                serde_json::Value::Object(gate_evidence_map),
            );
        }
        let evidence_value = serde_json::Value::Object(merged);

        // Read the overlay here, after this iteration's action has run and
        // before anything routes on `vars.*`, so a value this iteration
        // produced is visible to its own when-clauses and to every iteration
        // after it. Re-read per iteration, never hoisted: hoisting it would
        // restore the stale pre-loop snapshot this exists to replace. An empty
        // overlay borrows the base map untouched.
        let workflow_variables = overlay.layered_over(&base_workflow_variables);

        // 7. skip_if evaluation
        // Evaluate skip_if conditions before falling through to transition resolution.
        // If all conditions are met, auto-transition without waiting for agent evidence.
        if let Some(skip_conditions) = &template_state.skip_if {
            if conditions_satisfied(skip_conditions, &evidence_value, &workflow_variables) {
                // Resolve which transition the skip_if fires using the assembled
                // evidence_value (which contains nested gate output under "gates").
                // Using evidence_value rather than a flat skip_conditions map ensures
                // that gates.* when-clauses resolve correctly via dot-path traversal.
                // conditions_satisfied() already verified the runtime state matches,
                // so evidence_value contains the right values for routing.
                match resolve_transition(
                    template_state,
                    &evidence_value,
                    false,
                    fresh_evidence,
                    &workflow_variables,
                ) {
                    TransitionResolution::Resolved(target) => {
                        // Cycle detection BEFORE writing the event.
                        if visited.contains(&target) {
                            return Ok(AdvanceResult {
                                final_state: state,
                                advanced,
                                stop_reason: StopReason::CycleDetected { state: target },
                            });
                        }
                        // Append transitioned event.
                        let payload = EventPayload::Transitioned {
                            from: Some(state.clone()),
                            to: target.clone(),
                            condition_type: "skip_if".to_string(),
                            skip_if_matched: Some(skip_conditions.clone()),
                        };
                        append_event(&payload).map_err(AdvanceError::PersistenceError)?;
                        visited.insert(target.clone());
                        state = target;
                        advanced = true;
                        transition_count += 1;
                        current_evidence = BTreeMap::new();
                        fresh_evidence = false;
                        continue; // Chain: re-enter loop at next state
                    }
                    _ => {
                        // skip_if couldn't resolve a unique transition — fall through to
                        // normal resolution (this should not happen if compile validation passed)
                    }
                }
            }
        }

        // 8. Resolve transition
        match resolve_transition(
            template_state,
            &evidence_value,
            gates_failed,
            fresh_evidence,
            &workflow_variables,
        ) {
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
                    skip_if_matched: None,
                };
                append_event(&payload).map_err(AdvanceError::PersistenceError)?;

                visited.insert(target.clone());
                state = target;
                advanced = true;
                transition_count += 1;
                // Fresh epoch: auto-advanced states have no evidence
                current_evidence = BTreeMap::new();
                fresh_evidence = false;
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

/// Evaluate whether all skip_if conditions are satisfied.
///
/// Returns `true` only when every key-value pair in `conditions` is matched
/// against the current engine state (merged evidence + variables).
///
/// Matching rules:
/// - **`vars.NAME: {is_set: bool}`** — resolves the named template variable at
///   runtime against the `variables` map. Returns `true` when
///   `variables.get(name).map(|v| !v.is_empty()).unwrap_or(false)` equals the
///   expected bool.
/// - **all other keys** — dot-path lookup in `merged_evidence` using
///   `resolve_value`, compared with JSON equality. Gate keys like
///   `gates.ci.exit_code` require the nested gate structure in `merged_evidence`
///   (built from `gate_evidence_map` before this function is called).
///
/// Note: the compile-time analogue `skip_if_matches_when` in `src/template/types.rs`
/// uses a flat `BTreeMap::get()` lookup rather than dot-path traversal because
/// compile time has no nested gate output — both maps are flat. At runtime,
/// gate output arrives as nested JSON, so dot-path traversal is needed here.
fn conditions_satisfied(
    conditions: &std::collections::BTreeMap<String, serde_json::Value>,
    merged_evidence: &serde_json::Value,
    variables: &std::collections::HashMap<String, String>,
) -> bool {
    let vars_prefix = format!("{}.", VARS_NAMESPACE);
    conditions.iter().all(|(key, expected)| {
        if key.starts_with(&vars_prefix) {
            if let Some(expected_set) = is_is_set_matcher(expected) {
                let var_name = &key[vars_prefix.len()..];
                let is_set = variables
                    .get(var_name)
                    .map(|v| !v.is_empty())
                    .unwrap_or(false);
                return is_set == expected_set;
            }
        }
        resolve_value(merged_evidence, key) == Some(expected)
    })
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
///
/// The `fresh_evidence` parameter prevents unconditional transitions from firing when
/// a state is entered via auto-advance with no agent evidence. Without this, states
/// with both conditional and unconditional transitions would silently bypass their
/// directive when reached via chaining (skip_if or unconditional auto-advance).
/// Pure-routing states (only unconditional transitions, no conditional) are not
/// affected because `has_conditional` will be false.
pub fn resolve_transition(
    template_state: &TemplateState,
    evidence: &serde_json::Value,
    gate_failed: bool,
    fresh_evidence: bool,
    variables: &std::collections::HashMap<String, String>,
) -> TransitionResolution {
    if template_state.transitions.is_empty() {
        return TransitionResolution::NoTransitions;
    }

    let mut conditional_matches: Vec<String> = Vec::new();
    let mut unconditional_target: Option<String> = None;
    let mut has_conditional = false;

    let evidence_prefix = format!("{}.", EVIDENCE_NAMESPACE);
    let vars_prefix = format!("{}.", VARS_NAMESPACE);
    for transition in &template_state.transitions {
        match &transition.when {
            Some(conditions) => {
                has_conditional = true;
                let all_match = conditions.iter().all(|(field, expected)| {
                    // Issue #11: `evidence.<field>: present` matches when the
                    // agent-submitted evidence map contains `<field>` as a
                    // top-level key. The resolver's evidence map is built from
                    // the events since the last Transitioned event, so this
                    // reflects "any event since the last state transition".
                    if is_present_matcher(expected) && field.starts_with(&evidence_prefix) {
                        let inner = &field[evidence_prefix.len()..];
                        return !inner.is_empty()
                            && evidence
                                .as_object()
                                .is_some_and(|obj| obj.contains_key(inner));
                    }
                    // Issue #141: `vars.<name>: {is_set: bool}` checks whether
                    // a template variable was provided at init time with a
                    // non-empty value.
                    if field.starts_with(&vars_prefix) {
                        if let Some(expected_set) = is_is_set_matcher(expected) {
                            let var_name = &field[vars_prefix.len()..];
                            let is_set = variables
                                .get(var_name)
                                .map(|v| !v.is_empty())
                                .unwrap_or(false);
                            return is_set == expected_set;
                        }
                    }
                    resolve_value(evidence, field) == Some(expected)
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
                if gate_failed || (!fresh_evidence && has_conditional) {
                    // Two cases where we must not fire the unconditional fallback:
                    // 1. Gate failed — require evidence for override or recovery input.
                    // 2. Entered via auto-advance (no fresh evidence) and the state has
                    //    conditional transitions — the agent hasn't had a chance to submit
                    //    evidence yet; firing now would silently bypass the directive.
                    //    Pure-routing states (unconditional only) are unaffected because
                    //    has_conditional is false.
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
            materialize_children: None,
            failure: false,
            skipped_marker: false,
            skip_if: None,
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
    ) -> Result<BTreeMap<String, StructuredGateResult>, GateCaptureRefusal> {
        Ok(BTreeMap::new())
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
            resolve_transition(&state, &evidence, false, true, &HashMap::new()),
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
            resolve_transition(&state, &evidence, false, true, &HashMap::new()),
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
            resolve_transition(&state, &evidence, false, true, &HashMap::new()),
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
            resolve_transition(&state, &evidence, false, true, &HashMap::new()),
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
            resolve_transition(&state, &evidence, false, true, &HashMap::new()),
            TransitionResolution::Ambiguous(vec!["target_a".to_string(), "target_b".to_string()])
        );
    }

    #[test]
    fn no_transitions_returns_no_transitions() {
        let state = make_state(vec![]);
        let evidence = as_evidence(BTreeMap::new());
        assert_eq!(
            resolve_transition(&state, &evidence, false, true, &HashMap::new()),
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
            resolve_transition(&state, &evidence, false, true, &HashMap::new()),
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
            resolve_transition(&state, &evidence, false, true, &HashMap::new()),
            TransitionResolution::NeedsEvidence
        );

        // Both fields match.
        m.insert("b".to_string(), serde_json::json!("y"));
        let evidence = as_evidence(m);
        assert_eq!(
            resolve_transition(&state, &evidence, false, true, &HashMap::new()),
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
            resolve_transition(&state, &evidence, false, true, &HashMap::new()),
            TransitionResolution::Resolved("fallback_state".to_string())
        );

        // gate_failed=true: unconditional fallback skipped, needs evidence
        assert_eq!(
            resolve_transition(&state, &evidence, true, true, &HashMap::new()),
            TransitionResolution::NeedsEvidence
        );

        // gate_failed=true but evidence matches conditional: resolves normally
        let mut m = BTreeMap::new();
        m.insert("status".to_string(), serde_json::json!("completed"));
        let with_evidence = as_evidence(m);
        assert_eq!(
            resolve_transition(&state, &with_evidence, true, true, &HashMap::new()),
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
            resolve_transition(&state, &evidence, false, true, &HashMap::new()),
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
            resolve_transition(&state, &evidence_fail, false, true, &HashMap::new()),
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
            resolve_transition(&state, &evidence, false, true, &HashMap::new()),
            TransitionResolution::NeedsEvidence
        );

        // Evidence with "gates" but missing the "ci" sub-key
        let evidence_partial = serde_json::json!({ "gates": { "lint": { "exit_code": 0 } } });
        assert_eq!(
            resolve_transition(&state, &evidence_partial, false, true, &HashMap::new()),
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
            resolve_transition(&state, &evidence_both, false, true, &HashMap::new()),
            TransitionResolution::Resolved("approved".to_string())
        );

        // Only gate satisfied, no agent decision yet -- falls through to unconditional
        let evidence_gate_only = serde_json::json!({
            "gates": { "ci": { "exit_code": 0, "error": "" } }
        });
        assert_eq!(
            resolve_transition(&state, &evidence_gate_only, false, true, &HashMap::new()),
            TransitionResolution::Resolved("pending".to_string())
        );

        // Only decision provided, gate output missing -- falls through to unconditional
        let evidence_decision_only = serde_json::json!({ "decision": "approve" });
        assert_eq!(
            resolve_transition(
                &state,
                &evidence_decision_only,
                false,
                true,
                &HashMap::new()
            ),
            TransitionResolution::Resolved("pending".to_string())
        );
    }

    // -----------------------------------------------------------------------
    // Issue #11: evidence.<field>: present matcher
    // -----------------------------------------------------------------------

    #[test]
    fn present_matcher_fires_when_field_submitted() {
        // Template routes on evidence.retry_failed: present and should transition
        // only when the field is submitted.
        let state = make_state(vec![
            conditional(
                "retry",
                vec![("evidence.retry_failed", serde_json::json!("present"))],
            ),
            conditional("complete", vec![("status", serde_json::json!("done"))]),
        ]);

        // retry_failed submitted (value irrelevant to the matcher) -> routes to retry.
        let evidence = serde_json::json!({ "retry_failed": ["task-1"] });
        assert_eq!(
            resolve_transition(&state, &evidence, false, true, &HashMap::new()),
            TransitionResolution::Resolved("retry".to_string())
        );

        // retry_failed with a different payload shape still fires.
        let evidence_bool = serde_json::json!({ "retry_failed": true });
        assert_eq!(
            resolve_transition(&state, &evidence_bool, false, true, &HashMap::new()),
            TransitionResolution::Resolved("retry".to_string())
        );
    }

    #[test]
    fn present_matcher_does_not_fire_without_field() {
        // Same template — but only an unrelated evidence key is submitted. The
        // present matcher must not fire, and no other conditional matches, so
        // the result is NeedsEvidence.
        let state = make_state(vec![
            conditional(
                "retry",
                vec![("evidence.retry_failed", serde_json::json!("present"))],
            ),
            conditional("complete", vec![("status", serde_json::json!("done"))]),
        ]);

        let evidence = serde_json::json!({ "status": "pending" });
        assert_eq!(
            resolve_transition(&state, &evidence, false, true, &HashMap::new()),
            TransitionResolution::NeedsEvidence
        );
    }

    #[test]
    fn present_matcher_empty_field_name_does_not_match() {
        // `evidence.` (empty suffix) must not spuriously match any submission.
        let state = make_state(vec![conditional(
            "target",
            vec![("evidence.", serde_json::json!("present"))],
        )]);

        let evidence = serde_json::json!({ "retry_failed": ["task-1"] });
        assert_eq!(
            resolve_transition(&state, &evidence, false, true, &HashMap::new()),
            TransitionResolution::NeedsEvidence
        );
    }

    #[test]
    fn value_equality_matchers_still_work_after_present_added() {
        // Regression guard: the classic scalar-equality path must still resolve
        // when the evaluator encounters a non-"present" value, even alongside a
        // transition that uses the new present matcher.
        let state = make_state(vec![
            conditional(
                "retry",
                vec![("evidence.retry_failed", serde_json::json!("present"))],
            ),
            conditional("complete", vec![("status", serde_json::json!("done"))]),
        ]);

        // Only the value-equality branch matches.
        let evidence = serde_json::json!({ "status": "done" });
        assert_eq!(
            resolve_transition(&state, &evidence, false, true, &HashMap::new()),
            TransitionResolution::Resolved("complete".to_string())
        );

        // Value-equality miss still returns NeedsEvidence.
        let evidence_miss = serde_json::json!({ "status": "pending" });
        assert_eq!(
            resolve_transition(&state, &evidence_miss, false, true, &HashMap::new()),
            TransitionResolution::NeedsEvidence
        );
    }

    // -----------------------------------------------------------------------
    // Issue #141: vars.<name>: {is_set: bool} matcher
    // -----------------------------------------------------------------------

    #[test]
    fn vars_is_set_true_matches_when_variable_present() {
        let state = make_state(vec![
            conditional(
                "with_branch",
                vec![("vars.SHARED_BRANCH", serde_json::json!({"is_set": true}))],
            ),
            conditional(
                "without_branch",
                vec![("vars.SHARED_BRANCH", serde_json::json!({"is_set": false}))],
            ),
        ]);

        let evidence = as_evidence(BTreeMap::new());
        let mut vars = HashMap::new();
        vars.insert("SHARED_BRANCH".to_string(), "feature/foo".to_string());

        assert_eq!(
            resolve_transition(&state, &evidence, false, true, &vars),
            TransitionResolution::Resolved("with_branch".to_string())
        );
    }

    #[test]
    fn vars_is_set_false_matches_when_variable_absent() {
        let state = make_state(vec![
            conditional(
                "with_branch",
                vec![("vars.SHARED_BRANCH", serde_json::json!({"is_set": true}))],
            ),
            conditional(
                "without_branch",
                vec![("vars.SHARED_BRANCH", serde_json::json!({"is_set": false}))],
            ),
        ]);

        let evidence = as_evidence(BTreeMap::new());
        let vars = HashMap::new(); // no variables

        assert_eq!(
            resolve_transition(&state, &evidence, false, true, &vars),
            TransitionResolution::Resolved("without_branch".to_string())
        );
    }

    #[test]
    fn vars_is_set_false_matches_when_variable_empty() {
        // An empty string counts as "not set" for is_set purposes.
        let state = make_state(vec![
            conditional(
                "with_branch",
                vec![("vars.SHARED_BRANCH", serde_json::json!({"is_set": true}))],
            ),
            conditional(
                "without_branch",
                vec![("vars.SHARED_BRANCH", serde_json::json!({"is_set": false}))],
            ),
        ]);

        let evidence = as_evidence(BTreeMap::new());
        let mut vars = HashMap::new();
        vars.insert("SHARED_BRANCH".to_string(), String::new());

        assert_eq!(
            resolve_transition(&state, &evidence, false, true, &vars),
            TransitionResolution::Resolved("without_branch".to_string())
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
                    submitter_cwd: None,
                },
                idempotency_hash: None,
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
                    submitter_cwd: None,
                },
                idempotency_hash: None,
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
                    submitter_cwd: None,
                },
                idempotency_hash: None,
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
                    submitter_cwd: None,
                },
                idempotency_hash: None,
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
                skip_if_matched: None,
            },
            idempotency_hash: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
            &VariableOverlay::new(),
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
                completion: None,
                name_filter: None,
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
                materialize_children: None,
                failure: false,
                skipped_marker: false,
                skip_if: None,
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
            Ok(results)
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
            &VariableOverlay::new(),
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
                materialize_children: None,
                failure: false,
                skipped_marker: false,
                skip_if: None,
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
            &VariableOverlay::new(),
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
                materialize_children: None,
                failure: false,
                skipped_marker: false,
                skip_if: None,
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
            &VariableOverlay::new(),
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
                completion: None,
                name_filter: None,
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
                materialize_children: None,
                failure: false,
                skipped_marker: false,
                skip_if: None,
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
            Ok(results)
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
            &VariableOverlay::new(),
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
                completion: None,
                name_filter: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
            Ok(results)
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
            &VariableOverlay::new(),
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
                completion: None,
                name_filter: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
            Ok(results)
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
            &VariableOverlay::new(),
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
                completion: None,
                name_filter: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
            Ok(results)
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
            &VariableOverlay::new(),
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
                completion: None,
                name_filter: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
            Ok(results)
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
            &VariableOverlay::new(),
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
            Ok(results)
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
            &VariableOverlay::new(),
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
            Ok(results)
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
            &VariableOverlay::new(),
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
            Ok(results)
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
            &VariableOverlay::new(),
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
            &VariableOverlay::new(),
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
                materialize_children: None,
                failure: false,
                skipped_marker: false,
                skip_if: None,
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
            &VariableOverlay::new(),
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                materialize_children: None,
                failure: false,
                skipped_marker: false,
                skip_if: None,
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
            &VariableOverlay::new(),
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
                materialize_children: None,
                failure: false,
                skipped_marker: false,
                skip_if: None,
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
            &VariableOverlay::new(),
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
            &VariableOverlay::new(),
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
            &VariableOverlay::new(),
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
            fallback: None,
            capture_stdout_as: None,
        }
    }

    // -----------------------------------------------------------------------
    // capture delivery
    // -----------------------------------------------------------------------

    #[test]
    fn prepare_capture_trims_surrounding_whitespace() {
        // `echo` ends with a newline, so trimming is what makes the ordinary
        // case work at all rather than failing the allowlist.
        assert_eq!(prepare_capture("BRANCH", "  main\n").unwrap(), "main");
    }

    #[test]
    fn prepare_capture_rejects_output_that_is_only_whitespace() {
        assert_eq!(
            prepare_capture("BRANCH", " \n\t "),
            Err(CaptureError::Empty {
                key: "BRANCH".to_string()
            })
        );
    }

    #[test]
    fn prepare_capture_accepts_output_at_the_bound() {
        let value = "a".repeat(MAX_CAPTURE_BYTES);
        assert_eq!(
            prepare_capture("BRANCH", &value).unwrap().len(),
            MAX_CAPTURE_BYTES
        );
    }

    #[test]
    fn prepare_capture_rejects_output_over_the_bound() {
        let value = "a".repeat(MAX_CAPTURE_BYTES + 1);
        assert_eq!(
            prepare_capture("BRANCH", &value),
            Err(CaptureError::TooLarge {
                key: "BRANCH".to_string(),
                bytes: MAX_CAPTURE_BYTES + 1
            })
        );
    }

    #[test]
    fn prepare_capture_names_the_first_rejected_character() {
        // An interior newline is why multi-line output is not representable:
        // trimming cannot reach it, and the allowlist refuses it.
        assert_eq!(
            prepare_capture("BRANCH", "main\nsecond"),
            Err(CaptureError::DisallowedCharacter {
                key: "BRANCH".to_string(),
                position: 4,
                character: "\n".to_string()
            })
        );
    }

    /// A one-state template whose action declares a capture name.
    fn capturing_template(key: &str) -> CompiledTemplate {
        let mut action = make_action_decl("echo main");
        action.capture_stdout_as = Some(key.to_string());
        make_template(vec![(
            "detect",
            TemplateState {
                directive: "Detect.".to_string(),
                details: String::new(),
                transitions: vec![conditional(
                    "done",
                    vec![("result", serde_json::json!("ok"))],
                )],
                terminal: false,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: Some(action),
                materialize_children: None,
                failure: false,
                skipped_marker: false,
                skip_if: None,
            },
        )])
    }

    fn executed(stdout: &str) -> ActionResult {
        ActionResult::Executed {
            command: "echo main".to_string(),
            exit_code: 0,
            stdout: stdout.to_string(),
            stderr: String::new(),
            truncated: false,
        }
    }

    #[test]
    fn a_delivered_capture_appends_an_event_and_writes_the_overlay() {
        let template = capturing_template("BRANCH");
        let appended = std::cell::RefCell::new(Vec::new());
        let mut append = |p: &EventPayload| -> Result<(), String> {
            appended.borrow_mut().push(p.clone());
            Ok(())
        };
        let shutdown = AtomicBool::new(false);
        let overlay = VariableOverlay::new();
        let action = |_: &str, _: &ActionDecl, _: bool| executed("main\n");

        advance_until_stop(
            "detect",
            &template,
            &BTreeMap::new(),
            &[],
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &action,
            &overlay,
            &shutdown,
        )
        .unwrap();

        assert_eq!(overlay.get("BRANCH").as_deref(), Some("main"));
        assert!(
            appended.borrow().iter().any(|p| matches!(
                p,
                EventPayload::VariableCaptured { key, value } if key == "BRANCH" && value == "main"
            )),
            "the event and the overlay are written in the same step; got {:?}",
            appended.borrow()
        );
    }

    #[test]
    fn a_failed_capture_stops_the_tick_as_an_action_failure() {
        let template = capturing_template("BRANCH");
        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);
        let overlay = VariableOverlay::new();
        let action = |_: &str, _: &ActionDecl, _: bool| executed("   ");

        let result = advance_until_stop(
            "detect",
            &template,
            &BTreeMap::new(),
            &[],
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &action,
            &overlay,
            &shutdown,
        )
        .unwrap();

        assert_eq!(result.final_state, "detect");
        assert!(!result.advanced);
        let StopReason::GateBlocked(conditions) = result.stop_reason else {
            panic!("expected a gate-blocked stop, got {:?}", result.stop_reason);
        };
        let output = &conditions[ACTION_CONDITION_NAME].output;
        assert_eq!(output["failure_kind"], "capture_failed");
        assert_eq!(output["command"], "echo main");
        assert_eq!(output["capture_error"]["key"], "BRANCH");
        assert_eq!(output["capture_error"]["case"], "empty");
        assert!(
            output["exit_code"].is_null(),
            "the command exited zero, so there is no failing status to report"
        );
        assert!(overlay.is_empty(), "a failed capture writes nothing");
    }

    #[test]
    fn a_state_without_a_capture_name_writes_no_overlay_entry() {
        let template = make_template(vec![(
            "detect",
            TemplateState {
                directive: "Detect.".to_string(),
                details: String::new(),
                transitions: vec![conditional(
                    "done",
                    vec![("result", serde_json::json!("ok"))],
                )],
                terminal: false,
                gates: BTreeMap::new(),
                accepts: None,
                integration: None,
                default_action: Some(make_action_decl("echo main")),
                materialize_children: None,
                failure: false,
                skipped_marker: false,
                skip_if: None,
            },
        )]);
        let appended = std::cell::RefCell::new(Vec::new());
        let mut append = |p: &EventPayload| -> Result<(), String> {
            appended.borrow_mut().push(p.clone());
            Ok(())
        };
        let shutdown = AtomicBool::new(false);
        let overlay = VariableOverlay::new();
        let action = |_: &str, _: &ActionDecl, _: bool| executed("main\n");

        advance_until_stop(
            "detect",
            &template,
            &BTreeMap::new(),
            &[],
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &action,
            &overlay,
            &shutdown,
        )
        .unwrap();

        assert!(overlay.is_empty());
        assert!(!appended
            .borrow()
            .iter()
            .any(|p| matches!(p, EventPayload::VariableCaptured { .. })));
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
                materialize_children: None,
                failure: false,
                skipped_marker: false,
                skip_if: None,
            },
        )]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        let action = |_state: &str, _action: &ActionDecl, _has_evidence: bool| -> ActionResult {
            call_count.fetch_add(1, Ordering::Relaxed);
            ActionResult::Executed {
                command: "echo hello".to_string(),
                exit_code: 0,
                stdout: "hello".to_string(),
                stderr: String::new(),
                truncated: false,
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
            &VariableOverlay::new(),
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
                materialize_children: None,
                failure: false,
                skipped_marker: false,
                skip_if: None,
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
            &VariableOverlay::new(),
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
                materialize_children: None,
                failure: false,
                skipped_marker: false,
                skip_if: None,
            },
        )]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        let action = |_state: &str, _action: &ActionDecl, _has_evidence: bool| -> ActionResult {
            ActionResult::RequiresConfirmation {
                command: "gh pr create".to_string(),
                exit_code: 0,
                stdout: "PR #42 created".to_string(),
                stderr: String::new(),
                truncated: false,
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
            &VariableOverlay::new(),
            &shutdown,
        )
        .unwrap();

        assert_eq!(result.final_state, "confirm");
        assert!(!result.advanced);
        match &result.stop_reason {
            StopReason::ActionRequiresConfirmation {
                state,
                command,
                exit_code,
                stdout,
                ..
            } => {
                assert_eq!(state, "confirm");
                assert_eq!(command, "gh pr create");
                assert_eq!(*exit_code, 0);
                assert_eq!(stdout, "PR #42 created");
            }
            other => panic!("expected ActionRequiresConfirmation, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // action failure short-circuit
    // -----------------------------------------------------------------------

    fn failed_action(kind: FailureKind, exit_code: i32) -> ActionResult {
        ActionResult::Failed {
            command: "check.sh".to_string(),
            failure_kind: kind,
            exit_code,
            stdout: "partial".to_string(),
            stderr: "boom".to_string(),
            truncated: false,
        }
    }

    /// Build a one-state template whose action is followed by an
    /// unconditional transition to a terminal state. Without the
    /// short-circuit, a tick on `run` walks straight through to `done`.
    fn action_failure_template(
        gates: BTreeMap<String, crate::template::types::Gate>,
    ) -> CompiledTemplate {
        make_template(vec![
            (
                "run",
                TemplateState {
                    directive: "Run it.".to_string(),
                    details: String::new(),
                    transitions: vec![unconditional("done")],
                    terminal: false,
                    gates,
                    accepts: None,
                    integration: None,
                    default_action: Some(make_action_decl("check.sh")),
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
                },
            ),
        ])
    }

    fn run_failing_action(
        template: &CompiledTemplate,
        result: ActionResult,
        gate_calls: &std::sync::atomic::AtomicUsize,
    ) -> AdvanceResult {
        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);
        let gates_closure = |_: &BTreeMap<String, crate::template::types::Gate>| {
            gate_calls.fetch_add(1, Ordering::Relaxed);
            let mut out = BTreeMap::new();
            out.insert(
                "check".to_string(),
                StructuredGateResult {
                    outcome: GateOutcome::Passed,
                    output: serde_json::json!({"exit_code": 0}),
                },
            );
            Ok(out)
        };
        let action = |_: &str, _: &ActionDecl, _: bool| -> ActionResult { result.clone() };

        advance_until_stop(
            "run",
            template,
            &BTreeMap::new(),
            &[],
            &mut append,
            &gates_closure,
            &unavailable_integration,
            &action,
            &VariableOverlay::new(),
            &shutdown,
        )
        .unwrap()
    }

    fn action_condition(result: &AdvanceResult) -> &serde_json::Value {
        match &result.stop_reason {
            StopReason::GateBlocked(conditions) => {
                &conditions
                    .get(ACTION_CONDITION_NAME)
                    .expect("failure should be reported under the reserved name")
                    .output
            }
            other => panic!("expected GateBlocked, got {:?}", other),
        }
    }

    #[test]
    fn failing_action_stops_at_the_state_that_ran_it() {
        let gate_calls = std::sync::atomic::AtomicUsize::new(0);
        let template = action_failure_template(BTreeMap::new());
        let result = run_failing_action(
            &template,
            failed_action(FailureKind::NonzeroExit, 3),
            &gate_calls,
        );

        assert_eq!(result.final_state, "run");
        assert!(!result.advanced);
        let output = action_condition(&result);
        assert_eq!(output["state"], "run");
        assert_eq!(output["command"], "check.sh");
        assert_eq!(output["failure_kind"], "nonzero_exit");
        assert_eq!(output["exit_code"], 3);
        assert_eq!(output["stdout"], "partial");
        assert_eq!(output["stderr"], "boom");
        assert_eq!(output["truncated"], false);
    }

    #[test]
    fn failing_action_does_not_evaluate_the_states_own_gates() {
        use crate::template::types::Gate;

        let mut gates = BTreeMap::new();
        gates.insert(
            "check".to_string(),
            Gate {
                gate_type: "command".to_string(),
                command: "true".to_string(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
                completion: None,
                name_filter: None,
            },
        );

        let gate_calls = std::sync::atomic::AtomicUsize::new(0);
        let template = action_failure_template(gates);
        let result = run_failing_action(
            &template,
            failed_action(FailureKind::NonzeroExit, 1),
            &gate_calls,
        );

        // The gate would have passed. Running it would have let the tick
        // advance past a command that failed -- the silent advance R6 forbids.
        assert_eq!(gate_calls.load(Ordering::Relaxed), 0);
        assert_eq!(result.final_state, "run");
        assert!(!result.advanced);
        assert_eq!(action_condition(&result)["failure_kind"], "nonzero_exit");
    }

    #[test]
    fn refused_action_stops_before_the_gates_and_carries_no_action_condition() {
        use crate::template::types::Gate;

        let mut gates = BTreeMap::new();
        gates.insert(
            "check".to_string(),
            Gate {
                gate_type: "command".to_string(),
                command: "true".to_string(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
                completion: None,
                name_filter: None,
            },
        );

        let gate_calls = std::sync::atomic::AtomicUsize::new(0);
        let template = action_failure_template(gates);
        let result = run_failing_action(
            &template,
            ActionResult::Refused {
                field: ActionField::Command,
                key: "TOKEN".to_string(),
                producer: "producer".to_string(),
            },
            &gate_calls,
        );

        // The gate would have passed. A refusal has to short-circuit it for the
        // same reason a failure does -- a passing gate would advance the tick
        // past a command that never ran.
        assert_eq!(gate_calls.load(Ordering::Relaxed), 0);
        assert_eq!(result.final_state, "run");
        assert!(!result.advanced);
        match result.stop_reason {
            // Deliberately not a `GateBlocked` under `__action__`: nothing ran,
            // so there is no command, exit code, or output to report and no
            // condition for the author's `fallback` prose to explain.
            StopReason::ActionRefusedUnsetCapture {
                state,
                field,
                key,
                producer,
            } => {
                assert_eq!(state, "run");
                assert_eq!(field, ActionField::Command);
                assert_eq!(key, "TOKEN");
                assert_eq!(producer, "producer");
            }
            other => panic!("expected ActionRefusedUnsetCapture, got {:?}", other),
        }
    }

    #[test]
    fn spawn_failure_omits_exit_code() {
        let gate_calls = std::sync::atomic::AtomicUsize::new(0);
        let template = action_failure_template(BTreeMap::new());
        let result = run_failing_action(
            &template,
            failed_action(FailureKind::SpawnFailed, -1),
            &gate_calls,
        );

        let output = action_condition(&result);
        assert_eq!(output["failure_kind"], "spawn_failed");
        assert!(
            output.get("exit_code").is_none(),
            "a command that never ran has no exit status to report"
        );
    }

    #[test]
    fn timeout_omits_exit_code() {
        let gate_calls = std::sync::atomic::AtomicUsize::new(0);
        let template = action_failure_template(BTreeMap::new());
        let result = run_failing_action(
            &template,
            failed_action(FailureKind::TimedOut, -1),
            &gate_calls,
        );

        let output = action_condition(&result);
        assert_eq!(output["failure_kind"], "timed_out");
        assert!(output.get("exit_code").is_none());
    }

    #[test]
    fn wait_failure_is_an_action_failure_like_the_others() {
        let gate_calls = std::sync::atomic::AtomicUsize::new(0);
        let template = action_failure_template(BTreeMap::new());
        let result = run_failing_action(
            &template,
            failed_action(FailureKind::WaitFailed, -1),
            &gate_calls,
        );

        assert_eq!(result.final_state, "run");
        let output = action_condition(&result);
        assert_eq!(output["failure_kind"], "wait_failed");
        assert!(output.get("exit_code").is_none());
    }

    #[test]
    fn action_failure_status_follows_the_failure_kind() {
        let gate_calls = std::sync::atomic::AtomicUsize::new(0);
        let template = action_failure_template(BTreeMap::new());
        for (kind, expected) in [
            (FailureKind::NonzeroExit, GateOutcome::Failed),
            (FailureKind::TimedOut, GateOutcome::TimedOut),
            (FailureKind::SpawnFailed, GateOutcome::Error),
            (FailureKind::WaitFailed, GateOutcome::Error),
        ] {
            let result = run_failing_action(&template, failed_action(kind, -1), &gate_calls);
            match &result.stop_reason {
                StopReason::GateBlocked(conditions) => {
                    assert_eq!(
                        conditions[ACTION_CONDITION_NAME].outcome, expected,
                        "unexpected outcome for {:?}",
                        kind
                    );
                }
                other => panic!("expected GateBlocked, got {:?}", other),
            }
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
                completion: None,
                name_filter: None,
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
                materialize_children: None,
                failure: false,
                skipped_marker: false,
                skip_if: None,
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
            Ok(results)
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
            &VariableOverlay::new(),
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
                },
            ),
        ]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        let action = |_state: &str, _action: &ActionDecl, _has_evidence: bool| -> ActionResult {
            ActionResult::Executed {
                command: "true".to_string(),
                exit_code: 0,
                stdout: "ok".to_string(),
                stderr: String::new(),
                truncated: false,
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
            &VariableOverlay::new(),
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
            &VariableOverlay::new(),
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
            idempotency_hash: None,
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
            completion: None,
            name_filter: None,
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
                    skip_if_matched: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
            Ok(BTreeMap::new())
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
            &VariableOverlay::new(),
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
            Ok(results)
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
            &VariableOverlay::new(),
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
                completion: None,
                name_filter: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                skip_if_matched: None,
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
            Ok(results)
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
            &VariableOverlay::new(),
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
                completion: None,
                name_filter: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
            Ok(results)
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
            &VariableOverlay::new(),
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
                completion: None,
                name_filter: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
                },
            ),
        ]);

        let all_events: Vec<Event> = vec![make_event(
            1,
            EventPayload::Transitioned {
                from: None,
                to: "check-state".to_string(),
                condition_type: "auto".to_string(),
                skip_if_matched: None,
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
            Ok(results)
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
            &VariableOverlay::new(),
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

    // -----------------------------------------------------------------------
    // Gate backward compatibility: evidence map exclusion for legacy states
    // -----------------------------------------------------------------------

    /// A legacy state (gates present, no gates.* when-clause references) should
    /// advance via an unconditional transition when the gate passes. The gate
    /// output is NOT injected into the resolver evidence map, but the advance
    /// succeeds because the unconditional transition fires.
    #[test]
    fn legacy_state_no_gates_evidence() {
        use crate::template::types::Gate;

        let mut gates = BTreeMap::new();
        gates.insert(
            "ci_check".to_string(),
            Gate {
                gate_type: "command".to_string(),
                command: "true".to_string(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
                completion: None,
                name_filter: None,
            },
        );

        // Legacy state: transitions do NOT reference gates.* keys.
        let template = make_template(vec![
            (
                "verify",
                TemplateState {
                    directive: "Verify.".to_string(),
                    details: String::new(),
                    transitions: vec![unconditional("complete")],
                    terminal: false,
                    gates,
                    accepts: None,
                    integration: None,
                    default_action: None,
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
                },
            ),
            ("complete", {
                let mut s = make_state(vec![]);
                s.terminal = true;
                s
            }),
        ]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        // Gate always passes.
        let gate_eval = |gates: &BTreeMap<String, crate::template::types::Gate>| {
            let mut results = BTreeMap::new();
            for (name, _) in gates {
                results.insert(
                    name.clone(),
                    StructuredGateResult {
                        outcome: GateOutcome::Passed,
                        output: serde_json::json!({"passed": true, "exit_code": 0}),
                    },
                );
            }
            Ok(results)
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
            &VariableOverlay::new(),
            &shutdown,
        )
        .unwrap();

        // Advances to terminal state: gate passed, unconditional transition fired.
        assert_eq!(result.final_state, "complete");
        assert!(result.advanced);
        assert!(matches!(result.stop_reason, StopReason::Terminal));
    }

    /// A structured-mode state (gate present, with at least one gates.* when-clause
    /// reference) should inject gate output into the resolver evidence map and
    /// advance when the gate-output condition matches.
    #[test]
    fn structured_state_gates_evidence_present() {
        use crate::template::types::Gate;

        let mut gates = BTreeMap::new();
        gates.insert(
            "ci_check".to_string(),
            Gate {
                gate_type: "command".to_string(),
                command: "true".to_string(),
                timeout: 0,
                key: String::new(),
                pattern: String::new(),
                override_default: None,
                completion: None,
                name_filter: None,
            },
        );

        // Structured state: transition references gates.ci_check.passed.
        let template = make_template(vec![
            (
                "verify",
                TemplateState {
                    directive: "Verify.".to_string(),
                    details: String::new(),
                    transitions: vec![conditional(
                        "complete",
                        vec![("gates.ci_check.passed", serde_json::json!(true))],
                    )],
                    terminal: false,
                    gates,
                    accepts: None,
                    integration: None,
                    default_action: None,
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
                },
            ),
            ("complete", {
                let mut s = make_state(vec![]);
                s.terminal = true;
                s
            }),
        ]);

        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };
        let shutdown = AtomicBool::new(false);

        // Gate always passes with structured output.
        let gate_eval = |gates: &BTreeMap<String, crate::template::types::Gate>| {
            let mut results = BTreeMap::new();
            for (name, _) in gates {
                results.insert(
                    name.clone(),
                    StructuredGateResult {
                        outcome: GateOutcome::Passed,
                        output: serde_json::json!({"passed": true, "exit_code": 0}),
                    },
                );
            }
            Ok(results)
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
            &VariableOverlay::new(),
            &shutdown,
        )
        .unwrap();

        // Advances: gate output injected as gates.ci_check.{passed: true}, matches when clause.
        assert_eq!(result.final_state, "complete");
        assert!(result.advanced);
        assert!(matches!(result.stop_reason, StopReason::Terminal));
    }

    // -----------------------------------------------------------------------
    // conditions_satisfied tests (skip_if runtime evaluator)
    // -----------------------------------------------------------------------

    #[test]
    fn skip_if_conditions_satisfied_vars_set() {
        // vars.NAME: {is_set: true} must return true when the variable is present
        // and non-empty.
        let mut conditions = BTreeMap::new();
        conditions.insert(
            "vars.MY_VAR".to_string(),
            serde_json::json!({"is_set": true}),
        );

        let evidence = serde_json::json!({});
        let mut vars = HashMap::new();
        vars.insert("MY_VAR".to_string(), "some-value".to_string());

        assert!(
            conditions_satisfied(&conditions, &evidence, &vars),
            "conditions_satisfied should return true when variable is set"
        );
    }

    #[test]
    fn skip_if_conditions_satisfied_vars_unset() {
        // vars.NAME: {is_set: true} must return false when the variable is absent.
        let mut conditions = BTreeMap::new();
        conditions.insert(
            "vars.MY_VAR".to_string(),
            serde_json::json!({"is_set": true}),
        );

        let evidence = serde_json::json!({});
        let vars = HashMap::new(); // variable not present

        assert!(
            !conditions_satisfied(&conditions, &evidence, &vars),
            "conditions_satisfied should return false when variable is absent"
        );
    }

    #[test]
    fn skip_if_conditions_satisfied_vars_is_set_false_matches_absent() {
        // vars.NAME: {is_set: false} must return true when the variable is absent.
        let mut conditions = BTreeMap::new();
        conditions.insert(
            "vars.MY_VAR".to_string(),
            serde_json::json!({"is_set": false}),
        );

        let evidence = serde_json::json!({});
        let vars = HashMap::new(); // variable not present

        assert!(
            conditions_satisfied(&conditions, &evidence, &vars),
            "conditions_satisfied should return true when is_set: false and variable is absent"
        );
    }

    #[test]
    fn skip_if_conditions_satisfied_scalar_equality() {
        // Non-vars keys use dot-path equality against the merged evidence.
        let mut conditions = BTreeMap::new();
        conditions.insert("status".to_string(), serde_json::json!("ready"));

        let evidence = serde_json::json!({"status": "ready"});

        assert!(
            conditions_satisfied(&conditions, &evidence, &HashMap::new()),
            "conditions_satisfied should return true when scalar evidence matches"
        );
    }

    #[test]
    fn skip_if_conditions_satisfied_scalar_mismatch_returns_false() {
        let mut conditions = BTreeMap::new();
        conditions.insert("status".to_string(), serde_json::json!("ready"));

        let evidence = serde_json::json!({"status": "pending"});

        assert!(
            !conditions_satisfied(&conditions, &evidence, &HashMap::new()),
            "conditions_satisfied should return false when scalar evidence does not match"
        );
    }

    #[test]
    fn skip_if_fires_on_advance() {
        // A state with skip_if: {vars.SKIP: {is_set: true}} and a single unconditional
        // transition should auto-advance via skip_if when the variable is set.
        // The WorkflowInitialized event supplies the variable.
        let template = make_template(vec![
            (
                "skippable",
                TemplateState {
                    directive: "Skippable.".to_string(),
                    details: String::new(),
                    transitions: vec![unconditional("done")],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: {
                        let mut m = BTreeMap::new();
                        m.insert("vars.SKIP".to_string(), serde_json::json!({"is_set": true}));
                        Some(m)
                    },
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
                },
            ),
        ]);

        // Provide a WorkflowInitialized event that sets SKIP.
        let all_events = vec![Event {
            seq: 1,
            timestamp: "2026-04-01T00:00:00Z".to_string(),
            event_type: "workflow_initialized".to_string(),
            payload: EventPayload::WorkflowInitialized {
                template_path: "test.md".to_string(),
                variables: {
                    let mut m = HashMap::new();
                    m.insert("SKIP".to_string(), "true".to_string());
                    m
                },
                spawn_entry: None,
            },
            idempotency_hash: None,
        }];

        let mut appended: Vec<EventPayload> = Vec::new();
        let mut append = |payload: &EventPayload| -> Result<(), String> {
            appended.push(payload.clone());
            Ok(())
        };
        let shutdown = AtomicBool::new(false);

        let result = advance_until_stop(
            "skippable",
            &template,
            &BTreeMap::new(),
            &all_events,
            &mut append,
            &noop_gates,
            &unavailable_integration,
            &noop_action,
            &VariableOverlay::new(),
            &shutdown,
        )
        .unwrap();

        // skip_if fired: auto-advanced to terminal "done".
        assert_eq!(result.final_state, "done");
        assert!(result.advanced);
        assert_eq!(result.stop_reason, StopReason::Terminal);

        // The Transitioned event must have condition_type = "skip_if".
        let skip_event = appended.iter().find(|p| {
            matches!(p, EventPayload::Transitioned { condition_type, .. }
                if condition_type == "skip_if")
        });
        assert!(
            skip_event.is_some(),
            "expected a Transitioned event with condition_type 'skip_if'"
        );

        // The Transitioned event must carry skip_if_matched.
        if let Some(EventPayload::Transitioned {
            skip_if_matched, ..
        }) = skip_event
        {
            assert!(
                skip_if_matched.is_some(),
                "skip_if_matched should be Some in the Transitioned event"
            );
        }
    }

    #[test]
    fn vars_when_clause_reads_the_overlay() {
        // The `vars.*` staleness site. The log carries no variables at all, so
        // the base map built before the first iteration cannot satisfy
        // `vars.SKIP`. Only the overlay can -- and the loop re-reads it per
        // iteration, which is what a value produced mid-tick depends on.
        let template = make_template(vec![
            (
                "skippable",
                TemplateState {
                    directive: "Skippable.".to_string(),
                    details: String::new(),
                    transitions: vec![unconditional("done")],
                    terminal: false,
                    gates: BTreeMap::new(),
                    accepts: None,
                    integration: None,
                    default_action: None,
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: {
                        let mut m = BTreeMap::new();
                        m.insert("vars.SKIP".to_string(), serde_json::json!({"is_set": true}));
                        Some(m)
                    },
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
                    materialize_children: None,
                    failure: false,
                    skipped_marker: false,
                    skip_if: None,
                },
            ),
        ]);

        let fired_via_skip_if = |overlay: &VariableOverlay| -> bool {
            let mut appended: Vec<EventPayload> = Vec::new();
            let mut append = |payload: &EventPayload| -> Result<(), String> {
                appended.push(payload.clone());
                Ok(())
            };
            let shutdown = AtomicBool::new(false);

            advance_until_stop(
                "skippable",
                &template,
                &BTreeMap::new(),
                &[],
                &mut append,
                &noop_gates,
                &unavailable_integration,
                &noop_action,
                overlay,
                &shutdown,
            )
            .unwrap();

            appended.iter().any(|p| {
                matches!(p, EventPayload::Transitioned { condition_type, .. }
                    if condition_type == "skip_if")
            })
        };

        let overlay = VariableOverlay::new();
        overlay.insert("SKIP", "true");
        assert!(
            fired_via_skip_if(&overlay),
            "a name written to the overlay must satisfy a vars.* when-clause"
        );

        // Control: the same template, the same empty log, an empty overlay.
        // Nothing satisfies `vars.SKIP`, so the skip_if branch must not fire.
        assert!(
            !fired_via_skip_if(&VariableOverlay::new()),
            "an empty overlay must leave vars.* evaluation exactly as it was"
        );
    }
}
