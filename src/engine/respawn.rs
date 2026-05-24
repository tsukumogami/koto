//! F1 cold-restart re-priming — spawn a fresh subagent when the
//! substrate's transcript-retention window has elapsed and the
//! requester is genuinely silent past that window.
//!
//! ## Boundary with Issue 15's wake-recovery
//!
//! Wake-recovery ([`crate::engine::wake::maybe_recover_stale_wake`])
//! fires when `RequesterWoken.woken_at` is past `stale_dispatch_timeout`
//! AND the requester's log shows no post-woken_at writes — the
//! substrate's wake-delivery primitive is RE-INVOKED to retry the
//! lost wake. That branch presumes the substrate transcript is still
//! reachable.
//!
//! F1 cold-restart fires precisely when wake-recovery cannot reach
//! the requester: the substrate's documented transcript-retention
//! window (30 days on Claude Code Agent Teams) has elapsed AND
//! `last_log_activity` confirms the requester is silent past that
//! window. At that point re-invoking wake is futile (the substrate
//! has no record of the agent), so we spawn a fresh subagent with
//! the requester's saved `role` / `template_name` / `inputs` and a
//! fixed-form resume-context prompt.
//!
//! The two recovery modes are mutually exclusive: wake-recovery's
//! activity check on `requester_log_mtime > woken_at` short-circuits
//! before F1's preconditions become reachable, and F1's preconditions
//! require `woken_at + transcript_retention_floor < now()` which is
//! always older than wake-recovery's `stale_dispatch_timeout`
//! default of 600 seconds.
//!
//! ## F1 preconditions (ALL must hold)
//!
//! 1. `RequesterWoken.woken_at` is OLDER than
//!    `transcript_retention_floor` (default 30 days for Claude Code
//!    Agent Teams). This rules out the wake-recovery case.
//! 2. The requester's session log has NO writes with mtime >
//!    `woken_at`. The requester never resumed on the original
//!    substrate transcript.
//! 3. `now() - requester.last_log_activity > transcript_retention_floor`.
//!    The defensive read: even if the requester resumed once after
//!    `woken_at`, if `last_log_activity` is still past the retention
//!    floor the substrate transcript is gone.
//!
//! When all three hold, [`F1Outcome::Respawned`] is returned and the
//! caller writes a `RequesterRespawn` event via
//! [`crate::engine::audit::requester_respawn_fields`].
//!
//! ## F3 fallback (terminal `abandoned`)
//!
//! Four cause classes each emit a `RequesterRespawn` event with
//! `reason: "respawn_failed: <cause>"` rather than spawning a fresh
//! subagent. The requester transitions to a terminal `abandoned`
//! state via a `WorkflowCancelled` event written by the caller:
//!
//! 1. `respawn_generation_cap_exceeded`: the requester's header
//!    already records `respawn_generation == cap` (default 2 per
//!    `kt1.respawn_generation_cap`).
//! 2. `missing_role`: the requester's header has `role: None`
//!    (legacy session predating Issue 4). F1 cannot dispatch a
//!    fresh subagent without a role.
//! 3. `template_not_found`: the requester's `template_name`
//!    references a template no longer in the workflow registry.
//! 4. `substrate_refused`: the substrate-spawn primitive errored on
//!    agent-membership invocation. No fresh subagent is left
//!    running.
//!
//! ## Fixed-form resume-context prompt (load-bearing)
//!
//! Per Decision 5 lines 724-732, the resume-context prompt is
//! COMMITTED — never synthesized from session content. A
//! free-form prompt would create a prompt-injection surface that
//! Decision 5 explicitly avoids. The exact template lives in
//! [`RESUME_CONTEXT_PROMPT`] and is enforced by snapshot test.
//!
//! See DESIGN-koto-request-store.md Decision 5 (lines 673-738),
//! Phase 5 of Implementation Approach.

use std::path::Path;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};

use crate::engine::audit::requester_respawn_fields;
use crate::engine::claim::format_rfc3339_millis;
use crate::engine::errors::EngineError;
use crate::engine::persistence::append_event;
use crate::engine::types::{EventPayload, StateFileHeader, ValidatedSessionId};
use crate::session::state_file_name;

/// Fixed-form resume-context prompt handed to a freshly-spawned
/// subagent under F1 cold-restart re-priming.
///
/// **Load-bearing: this string is COMMITTED.** Free-form LLM-authored
/// synthesis from session content would introduce a prompt-injection
/// surface that Decision 5 (design lines 724-732) explicitly avoids.
/// Any change to this template must be authored as a deliberate
/// security-reviewed edit; the snapshot test in `tests/respawn.rs`
/// asserts byte-equality so accidental drift is caught at CI.
pub const RESUME_CONTEXT_PROMPT: &str = "You are resuming session <id>. Read your prior state via `koto session info <id>` and prior children via `koto session list --parent <id>`; advance from where you left off.";

/// Default substrate transcript-retention floor. Claude Code Agent
/// Teams documents 30 days; bunki BK2's hosted substrate may differ.
/// Currently hard-coded; a future Issue 18 extension may expose this
/// as `kt1.transcript_retention_days`.
pub const DEFAULT_TRANSCRIPT_RETENTION_DAYS: u32 = 30;

/// Render the [`RESUME_CONTEXT_PROMPT`] with `<id>` substituted by
/// the resuming session id. The template is verbatim — only the
/// `<id>` literal is replaced.
pub fn render_resume_context_prompt(session_id: &ValidatedSessionId) -> String {
    RESUME_CONTEXT_PROMPT.replace("<id>", session_id.as_str())
}

/// Pluggable substrate respawn-delivery abstraction.
///
/// Sibling to [`crate::engine::claim::SubstrateSpawner`] and
/// [`crate::engine::wake::SubstrateWaker`]. Respawn carries more
/// context than spawn (the saved `role` / `template` / `inputs` plus
/// the resume-context prompt), and more context than wake (the
/// substrate has to allocate a fresh agent-membership identifier).
/// A separate trait keeps each substrate primitive's payload
/// honest.
///
/// The implementation MUST invoke the substrate's agent-membership
/// primitive AFTER the spawn so wake-delivery to the new subagent
/// remains addressable (per Decision 5).
pub trait SubstrateRespawner {
    /// Spawn a fresh subagent with the requester's saved identity.
    /// Returns the new subagent's identifier on success;
    /// `Err(EngineError)` on substrate-level failure (treated as the
    /// `substrate_refused` F3 fallback by the caller).
    fn respawn(&self, request: &RespawnRequest) -> Result<(), EngineError>;
}

/// Request payload passed to [`SubstrateRespawner::respawn`].
///
/// All fields are sourced from the requester's existing
/// [`StateFileHeader`]; the substrate-side handler uses them to
/// configure the new agent's runtime context.
#[derive(Debug, Clone)]
pub struct RespawnRequest {
    /// Requester's session id (the workflow being re-primed).
    pub session_id: ValidatedSessionId,
    /// Saved role from the requester's header.
    pub role: String,
    /// Saved template name from the requester's header.
    pub template_name: String,
    /// Saved input bag from the requester's header.
    pub inputs: Option<serde_json::Value>,
    /// Resume-context prompt rendered from [`RESUME_CONTEXT_PROMPT`]
    /// with the session id substituted.
    pub resume_prompt: String,
    /// Coordinator id orchestrating the respawn.
    pub coord_id: String,
    /// New `respawn_generation` value the subagent's header should
    /// carry (always `prior_generation + 1`).
    pub new_respawn_generation: u32,
}

/// Default [`SubstrateRespawner`] used by `handle_next` until a
/// concrete substrate implementation (Claude Code agent-membership
/// poke, bunki BK2 hosted respawn) ships. Logs the respawn intent
/// and returns Ok — the audit event on the requester's log is the
/// source of truth for "respawn was emitted", same discipline as
/// [`crate::engine::wake::LoggingWaker`].
pub struct LoggingRespawner;

impl SubstrateRespawner for LoggingRespawner {
    fn respawn(&self, request: &RespawnRequest) -> Result<(), EngineError> {
        eprintln!(
            "info: SubstrateRespawner stub invoked for session '{}' \
             (role: '{}', template: '{}', new_gen: {}); \
             concrete respawn-delivery primitive not yet wired",
            request.session_id.as_str(),
            request.role,
            request.template_name,
            request.new_respawn_generation,
        );
        Ok(())
    }
}

/// Outcome of evaluating F1 preconditions for a requester.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum F1Outcome {
    /// All three preconditions held AND the cap was not exceeded;
    /// the substrate respawn primitive was invoked successfully.
    /// The caller appends a `RequesterRespawn` audit event with
    /// `reason: "transcript_expired"`.
    Respawned { new_generation: u32 },
    /// F1 did not fire — typically because at least one
    /// precondition is not met. No event emitted; the requester
    /// continues to be reachable via wake-recovery (Issue 15).
    NoOp { reason: NoOpReason },
    /// F3 fallback fired — F1 wanted to spawn but a structural
    /// condition prevents it. The caller writes a
    /// `RequesterRespawn` event with `reason: "respawn_failed:
    /// <cause>"` AND transitions the requester to terminal
    /// `abandoned`.
    F3Fallback { cause: F3Cause },
}

/// Reason F1 declined to fire (no event emitted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoOpReason {
    /// Precondition #1 failed: `woken_at` is younger than the
    /// retention floor. Wake-recovery (Issue 15) is the right
    /// rule here.
    WokenYoungerThanFloor,
    /// Precondition #2 failed: requester resumed after
    /// `woken_at` (its log has a write with mtime > woken_at).
    /// The substrate transcript is still reachable; no recovery
    /// needed.
    RequesterResumedAfterWake,
    /// Precondition #3 failed: requester's
    /// `last_log_activity` is FRESHER than the retention floor.
    /// The substrate transcript could still reach the requester
    /// via wake-delivery; F1 is not the right rule.
    RequesterRecentlyActive,
    /// No `RequesterWoken` event exists on the coord's log for
    /// this requester. Nothing to act on.
    NoWokenAtTimestamp,
}

/// Cause class for F3 fallback. Each emits a distinct
/// `respawn_failed: <cause>` reason string on the
/// `RequesterRespawn` event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum F3Cause {
    /// `respawn_generation == cap`. Default cap is 2 per
    /// `kt1.respawn_generation_cap`.
    RespawnGenerationCapExceeded,
    /// Requester's header has `role: None` (legacy session
    /// predating Issue 4). F1 cannot dispatch without a role.
    MissingRole,
    /// Requester's `template_name` references a template no
    /// longer present in the workflow registry. The caller
    /// determines this; F1's evaluation surfaces it via the
    /// `template_check` callback.
    TemplateNotFound,
    /// The substrate-spawn primitive returned an error on
    /// agent-membership invocation.
    SubstrateRefused,
}

impl F3Cause {
    /// Render the cause as the `respawn_failed: <cause>` reason
    /// string used on the audit event.
    pub fn reason(&self) -> &'static str {
        match self {
            F3Cause::RespawnGenerationCapExceeded => {
                "respawn_failed: respawn_generation_cap_exceeded"
            }
            F3Cause::MissingRole => "respawn_failed: missing_role",
            F3Cause::TemplateNotFound => "respawn_failed: template_not_found",
            F3Cause::SubstrateRefused => "respawn_failed: substrate_refused",
        }
    }
}

/// Inputs to [`evaluate_f1_preconditions`].
#[derive(Debug, Clone)]
pub struct F1Inputs<'a> {
    /// The requester's parsed header.
    pub header: &'a StateFileHeader,
    /// `RequesterWoken.woken_at` from the coord's log, or `None`
    /// if no wake has been emitted for this requester yet.
    pub woken_at: Option<SystemTime>,
    /// mtime of the requester's session log (the most recent
    /// write).
    pub last_log_activity: SystemTime,
    /// Current wall-clock time.
    pub now: SystemTime,
    /// Substrate transcript-retention window.
    pub retention_floor: Duration,
    /// Respawn-generation cap (`kt1.respawn_generation_cap`).
    pub cap: u32,
    /// Template-existence check. Returns `false` when the
    /// template referenced in `header.template_name` is no
    /// longer compilable / present in the registry.
    pub template_exists: bool,
}

/// Decide F1 / NoOp / F3 for one requester. Pure function —
/// callers handle the side-effects (audit emission, substrate
/// invocation, terminal-state writes).
///
/// The pre-condition order is:
///
/// 1. Cap exceeded? → F3 `RespawnGenerationCapExceeded` regardless
///    of other conditions (cap is the structural cut-off).
/// 2. `woken_at` present and OLDER than retention floor? If not →
///    `NoOp`.
/// 3. Requester resumed after `woken_at` (precondition #2 fails)?
///    → `NoOp`.
/// 4. `last_log_activity` newer than retention floor (precondition
///    #3 fails)? → `NoOp`.
/// 5. `header.role` missing? → F3 `MissingRole`.
/// 6. `template_exists == false`? → F3 `TemplateNotFound`.
/// 7. Otherwise → `Respawned { new_generation: prior + 1 }`. The
///    caller invokes the substrate primitive; on substrate failure
///    the caller falls back to F3 `SubstrateRefused`.
pub fn evaluate_f1_preconditions(inputs: &F1Inputs<'_>) -> F1Outcome {
    let current_generation = inputs.header.respawn_generation.unwrap_or(0);

    // Cap check first — structural cut-off regardless of other
    // conditions. Note: cap=2 means gen=0,1,2 are allowed;
    // gen==cap triggers F3 (the next respawn would be gen=3).
    if current_generation >= inputs.cap {
        return F1Outcome::F3Fallback {
            cause: F3Cause::RespawnGenerationCapExceeded,
        };
    }

    // Precondition #1: woken_at present and OLDER than retention floor.
    let Some(woken_at) = inputs.woken_at else {
        return F1Outcome::NoOp {
            reason: NoOpReason::NoWokenAtTimestamp,
        };
    };
    let age_since_wake = inputs
        .now
        .duration_since(woken_at)
        .unwrap_or(Duration::ZERO);
    if age_since_wake < inputs.retention_floor {
        return F1Outcome::NoOp {
            reason: NoOpReason::WokenYoungerThanFloor,
        };
    }

    // Precondition #2: requester has NO writes after woken_at.
    // last_log_activity > woken_at means the requester resumed.
    if inputs.last_log_activity > woken_at {
        // The requester resumed after the wake. Precondition #3
        // (the defensive read) still gates whether F1 fires when
        // the requester subsequently went idle.
        let age_since_activity = inputs
            .now
            .duration_since(inputs.last_log_activity)
            .unwrap_or(Duration::ZERO);
        if age_since_activity < inputs.retention_floor {
            return F1Outcome::NoOp {
                reason: NoOpReason::RequesterRecentlyActive,
            };
        }
        // Resumed then went idle past the floor → F1 fires.
    }

    // F1 preconditions all hold. Check structural requirements for
    // a successful respawn.
    if inputs.header.role.as_deref().unwrap_or("").is_empty() {
        return F1Outcome::F3Fallback {
            cause: F3Cause::MissingRole,
        };
    }
    if !inputs.template_exists {
        return F1Outcome::F3Fallback {
            cause: F3Cause::TemplateNotFound,
        };
    }

    F1Outcome::Respawned {
        new_generation: current_generation + 1,
    }
}

/// Execute the F1 respawn for one requester. Composes
/// [`evaluate_f1_preconditions`] with the substrate primitive and
/// the audit-event emission.
///
/// On `F1Outcome::Respawned`:
/// 1. Build a [`RespawnRequest`] from the requester's header +
///    rendered resume-context prompt.
/// 2. Invoke `respawner.respawn(&request)`. Substrate failure →
///    fall through to F3 `SubstrateRefused`.
/// 3. Append a `RequesterRespawn` event on the requester's session
///    log via [`requester_respawn_fields`] with
///    `reason: "transcript_expired"`.
/// 4. Return the new generation in [`RespawnExecuted::Respawned`].
///
/// On `F1Outcome::F3Fallback`:
/// 1. Append a `RequesterRespawn` event with the F3 reason.
/// 2. Append a `WorkflowCancelled` event so the requester
///    transitions to terminal `abandoned`.
/// 3. Return [`RespawnExecuted::Abandoned`].
///
/// On `F1Outcome::NoOp`: no events emitted; return
/// [`RespawnExecuted::NoOp`].
/// Inputs to [`execute_respawn`]. Grouped into a struct so the
/// function signature stays clean and so future fields (e.g. a
/// caller-supplied agent-membership target) compose naturally.
#[derive(Debug, Clone)]
pub struct RespawnExecution<'a> {
    /// Path to the requester's session log file.
    pub requester_state_file: &'a Path,
    /// The requester's parsed header (source of role / template /
    /// inputs / current respawn_generation).
    pub header: &'a StateFileHeader,
    /// Coordinator id orchestrating the respawn. Used for both
    /// `prior_coordinator_of_record` and `new_coordinator_of_record`
    /// on the audit event in the single-coordinator model.
    pub coord_id: &'a str,
    /// Requester's session id (validated). Used to render the
    /// resume-context prompt and address the substrate primitive.
    pub requester_session_id: &'a ValidatedSessionId,
    /// `RequesterWoken.woken_at` from the coord's log, or `None`
    /// when no wake has been emitted for this requester yet.
    pub woken_at: Option<SystemTime>,
    /// mtime of the requester's session log.
    pub last_log_activity: SystemTime,
    /// Wall-clock at evaluation time.
    pub now: SystemTime,
    /// Substrate transcript-retention window.
    pub retention_floor: Duration,
    /// Respawn-generation cap (`kt1.respawn_generation_cap`).
    pub cap: u32,
    /// Whether the requester's `template_name` is currently
    /// compilable / present in the registry. Caller-determined.
    pub template_exists: bool,
}

pub fn execute_respawn(
    exec: &RespawnExecution<'_>,
    respawner: &dyn SubstrateRespawner,
) -> Result<RespawnExecuted> {
    let inputs = F1Inputs {
        header: exec.header,
        woken_at: exec.woken_at,
        last_log_activity: exec.last_log_activity,
        now: exec.now,
        retention_floor: exec.retention_floor,
        cap: exec.cap,
        template_exists: exec.template_exists,
    };
    let outcome = evaluate_f1_preconditions(&inputs);
    match outcome {
        F1Outcome::NoOp { reason } => Ok(RespawnExecuted::NoOp { reason }),

        F1Outcome::Respawned { new_generation } => {
            // Build the substrate request.
            let role = exec.header.role.clone().unwrap_or_default();
            let template_name = exec.header.template_name.clone().unwrap_or_default();
            let resume_prompt = render_resume_context_prompt(exec.requester_session_id);
            let request = RespawnRequest {
                session_id: exec.requester_session_id.clone(),
                role,
                template_name,
                inputs: exec.header.inputs.clone(),
                resume_prompt,
                coord_id: exec.coord_id.to_string(),
                new_respawn_generation: new_generation,
            };

            // Substrate invocation. Failure → F3 SubstrateRefused.
            if let Err(e) = respawner.respawn(&request) {
                eprintln!(
                    "warning: substrate respawn for '{}' returned err: {}; falling back to F3",
                    exec.requester_session_id.as_str(),
                    e
                );
                emit_respawn_event(
                    exec.requester_state_file,
                    exec.requester_session_id,
                    exec.header.respawn_generation.unwrap_or(0),
                    F3Cause::SubstrateRefused.reason(),
                    exec.coord_id,
                    exec.coord_id,
                    exec.now,
                )?;
                emit_workflow_cancelled(
                    exec.requester_state_file,
                    F3Cause::SubstrateRefused.reason(),
                    exec.now,
                )?;
                return Ok(RespawnExecuted::Abandoned {
                    cause: F3Cause::SubstrateRefused,
                });
            }

            // Successful respawn → emit RequesterRespawn with
            // reason: transcript_expired.
            emit_respawn_event(
                exec.requester_state_file,
                exec.requester_session_id,
                new_generation,
                "transcript_expired",
                exec.coord_id,
                exec.coord_id,
                exec.now,
            )?;
            Ok(RespawnExecuted::Respawned { new_generation })
        }

        F1Outcome::F3Fallback { cause } => {
            emit_respawn_event(
                exec.requester_state_file,
                exec.requester_session_id,
                exec.header.respawn_generation.unwrap_or(0),
                cause.reason(),
                exec.coord_id,
                exec.coord_id,
                exec.now,
            )?;
            emit_workflow_cancelled(exec.requester_state_file, cause.reason(), exec.now)?;
            Ok(RespawnExecuted::Abandoned { cause })
        }
    }
}

/// Result of [`execute_respawn`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RespawnExecuted {
    /// Substrate respawn succeeded; `RequesterRespawn` event with
    /// `reason: "transcript_expired"` was appended.
    Respawned { new_generation: u32 },
    /// F3 fallback fired: `RequesterRespawn` event with the
    /// `respawn_failed: <cause>` reason AND a `WorkflowCancelled`
    /// event were appended; the requester is terminal `abandoned`.
    Abandoned { cause: F3Cause },
    /// Preconditions not met; no events emitted.
    NoOp { reason: NoOpReason },
}

/// Append a `RequesterRespawn` audit event to the requester's
/// session log via the audit-helper.
fn emit_respawn_event(
    requester_state_file: &Path,
    requester_session_id: &ValidatedSessionId,
    respawn_generation: u32,
    reason: &str,
    prior_coord: &str,
    new_coord: &str,
    now: SystemTime,
) -> Result<()> {
    let respawned_at = format_rfc3339_millis(now);
    let fields = requester_respawn_fields(
        requester_session_id,
        respawn_generation,
        reason,
        prior_coord,
        new_coord,
        &respawned_at,
    );
    let payload = EventPayload::EvidenceSubmitted {
        state: "kt1.respawn".to_string(),
        fields,
        submitter_cwd: None,
    };
    append_event(requester_state_file, &payload, &respawned_at).with_context(|| {
        format!(
            "append RequesterRespawn to {}",
            requester_state_file.display()
        )
    })?;
    Ok(())
}

/// Append a `WorkflowCancelled` event so the requester transitions
/// to terminal `abandoned` (F3 fallback).
fn emit_workflow_cancelled(
    requester_state_file: &Path,
    reason: &str,
    now: SystemTime,
) -> Result<()> {
    let payload = EventPayload::WorkflowCancelled {
        state: "kt1.respawn".to_string(),
        reason: reason.to_string(),
    };
    let timestamp = format_rfc3339_millis(now);
    append_event(requester_state_file, &payload, &timestamp).with_context(|| {
        format!(
            "append WorkflowCancelled to {}",
            requester_state_file.display()
        )
    })?;
    Ok(())
}

/// Compute the path to a session's state file from its
/// `koto_root/sessions` parent.
pub fn requester_state_path(sessions_dir: &Path, session_id: &str) -> std::path::PathBuf {
    sessions_dir
        .join(session_id)
        .join(state_file_name(session_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::StateFileHeader;

    fn base_header() -> StateFileHeader {
        StateFileHeader {
            schema_version: 1,
            workflow: "wf".into(),
            template_hash: "deadbeef".into(),
            created_at: "2026-05-24T00:00:00Z".into(),
            parent_workflow: None,
            template_source_dir: None,
            session_id: "wf".into(),
            intent: None,
            template_name: Some("verdict".into()),
            needs_agent: Some(true),
            role: Some("scrutineer".into()),
            inputs: Some(serde_json::json!({"k": "v"})),
            coordinator_of_record: Some("coord".into()),
            requested_by: Some("parent".into()),
            assignment_claim: None,
            dispatch_epoch: 0,
            respawn_generation: None,
            priority: None,
            deadline: None,
            retry_count: None,
            agent_config: None,
        }
    }

    fn floor() -> Duration {
        Duration::from_secs(60 * 60 * 24 * 30) // 30 days
    }

    fn inputs<'a>(
        header: &'a StateFileHeader,
        woken_at: Option<SystemTime>,
        last_log_activity: SystemTime,
        now: SystemTime,
    ) -> F1Inputs<'a> {
        F1Inputs {
            header,
            woken_at,
            last_log_activity,
            now,
            retention_floor: floor(),
            cap: 2,
            template_exists: true,
        }
    }

    #[test]
    fn fires_when_all_three_preconditions_hold() {
        let header = base_header();
        let now = SystemTime::now();
        let woken_at = now - Duration::from_secs(60 * 60 * 24 * 60); // 60 days ago
        let last_activity = woken_at - Duration::from_secs(60); // before woken_at
        let result =
            evaluate_f1_preconditions(&inputs(&header, Some(woken_at), last_activity, now));
        assert_eq!(result, F1Outcome::Respawned { new_generation: 1 });
    }

    #[test]
    fn no_op_when_woken_younger_than_floor() {
        let header = base_header();
        let now = SystemTime::now();
        let woken_at = now - Duration::from_secs(60 * 60 * 24); // 1 day ago
        let result = evaluate_f1_preconditions(&inputs(&header, Some(woken_at), now, now));
        assert_eq!(
            result,
            F1Outcome::NoOp {
                reason: NoOpReason::WokenYoungerThanFloor
            }
        );
    }

    #[test]
    fn no_op_when_no_woken_at() {
        let header = base_header();
        let now = SystemTime::now();
        let result = evaluate_f1_preconditions(&inputs(&header, None, now, now));
        assert_eq!(
            result,
            F1Outcome::NoOp {
                reason: NoOpReason::NoWokenAtTimestamp
            }
        );
    }

    #[test]
    fn no_op_when_requester_recently_active() {
        let header = base_header();
        let now = SystemTime::now();
        let woken_at = now - Duration::from_secs(60 * 60 * 24 * 60); // 60 days ago
                                                                     // requester resumed AFTER woken_at and only 1 day ago — recently active.
        let last_activity = now - Duration::from_secs(60 * 60 * 24);
        let result =
            evaluate_f1_preconditions(&inputs(&header, Some(woken_at), last_activity, now));
        assert_eq!(
            result,
            F1Outcome::NoOp {
                reason: NoOpReason::RequesterRecentlyActive
            }
        );
    }

    #[test]
    fn fires_after_resume_then_idle_past_floor() {
        // Requester resumed once after woken_at, then went idle for
        // > floor. Precondition #3 (defensive read) confirms the
        // transcript is gone → F1 fires.
        let header = base_header();
        let now = SystemTime::now();
        let woken_at = now - Duration::from_secs(60 * 60 * 24 * 90); // 90 days ago
                                                                     // resumed once 60 days ago (after woken_at), then idle since.
        let last_activity = now - Duration::from_secs(60 * 60 * 24 * 60);
        let result =
            evaluate_f1_preconditions(&inputs(&header, Some(woken_at), last_activity, now));
        assert_eq!(result, F1Outcome::Respawned { new_generation: 1 });
    }

    #[test]
    fn f3_when_cap_exceeded() {
        let mut header = base_header();
        header.respawn_generation = Some(2);
        let now = SystemTime::now();
        let woken_at = now - Duration::from_secs(60 * 60 * 24 * 60);
        let last_activity = woken_at - Duration::from_secs(60);
        let result =
            evaluate_f1_preconditions(&inputs(&header, Some(woken_at), last_activity, now));
        assert_eq!(
            result,
            F1Outcome::F3Fallback {
                cause: F3Cause::RespawnGenerationCapExceeded
            }
        );
    }

    #[test]
    fn f3_when_role_missing() {
        let mut header = base_header();
        header.role = None;
        let now = SystemTime::now();
        let woken_at = now - Duration::from_secs(60 * 60 * 24 * 60);
        let last_activity = woken_at - Duration::from_secs(60);
        let result =
            evaluate_f1_preconditions(&inputs(&header, Some(woken_at), last_activity, now));
        assert_eq!(
            result,
            F1Outcome::F3Fallback {
                cause: F3Cause::MissingRole
            }
        );
    }

    #[test]
    fn f3_when_template_missing() {
        let header = base_header();
        let now = SystemTime::now();
        let woken_at = now - Duration::from_secs(60 * 60 * 24 * 60);
        let last_activity = woken_at - Duration::from_secs(60);
        let mut i = inputs(&header, Some(woken_at), last_activity, now);
        i.template_exists = false;
        let result = evaluate_f1_preconditions(&i);
        assert_eq!(
            result,
            F1Outcome::F3Fallback {
                cause: F3Cause::TemplateNotFound
            }
        );
    }

    #[test]
    fn generation_increments_from_existing() {
        let mut header = base_header();
        header.respawn_generation = Some(1);
        let now = SystemTime::now();
        let woken_at = now - Duration::from_secs(60 * 60 * 24 * 60);
        let last_activity = woken_at - Duration::from_secs(60);
        let result =
            evaluate_f1_preconditions(&inputs(&header, Some(woken_at), last_activity, now));
        assert_eq!(result, F1Outcome::Respawned { new_generation: 2 });
    }

    #[test]
    fn f3_cause_reasons_are_stable() {
        assert_eq!(
            F3Cause::RespawnGenerationCapExceeded.reason(),
            "respawn_failed: respawn_generation_cap_exceeded"
        );
        assert_eq!(
            F3Cause::MissingRole.reason(),
            "respawn_failed: missing_role"
        );
        assert_eq!(
            F3Cause::TemplateNotFound.reason(),
            "respawn_failed: template_not_found"
        );
        assert_eq!(
            F3Cause::SubstrateRefused.reason(),
            "respawn_failed: substrate_refused"
        );
    }

    #[test]
    fn render_resume_context_substitutes_id() {
        let id = ValidatedSessionId::new("session-xyz").unwrap();
        let rendered = render_resume_context_prompt(&id);
        assert!(rendered.contains("session-xyz"));
        // The literal "<id>" must NOT remain in the output.
        assert!(!rendered.contains("<id>"));
    }

    #[test]
    fn resume_context_prompt_is_committed_form() {
        // Snapshot guard: any edit to this template requires
        // changing the test deliberately.
        assert_eq!(
            RESUME_CONTEXT_PROMPT,
            "You are resuming session <id>. Read your prior state via `koto session info <id>` and prior children via `koto session list --parent <id>`; advance from where you left off."
        );
    }
}
