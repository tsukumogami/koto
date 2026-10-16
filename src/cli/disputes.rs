//! `koto disputes record`: record that a rule's finding at a gate is wrong.
//!
//! A dispute is recorded, not obeyed. The command appends one
//! `finding_disputed` event naming the state, the gate, the rule id, the
//! reason and who disputed it, and the run journal relays it as one
//! `check_disputed` record without the reason. Nothing the advance loop
//! routes on reads the event: no gate outcome, route or state changes, and
//! the next `koto next` responds as it would have without it. To move past a
//! gate, `koto overrides record` is the verb.
//!
//! The gate must have a settled evaluation logged in the named state (the
//! current state unless `--state` names another) anywhere in the session's
//! log, one with an outcome rather than `pending`; the dispute is about the
//! most recent one. A finding for the rule isn't
//! required, since a logged finding list may have been cut at its cap.
//!
//! The reason goes through koto's known-credential redaction, the same pass
//! captured output and findings get, before it is stored.
//!
//! Errors print a JSON body on stdout: exit 1 for a missing session or an
//! append that fails, nothing appended; exit 2 with a typed `code` for a
//! caller problem (a gate not evaluated in the state, a rule id or reason
//! outside its bounds), nothing appended; exit 3 when the session's state
//! can't be derived.

use anyhow::Result;
use clap::{Subcommand, ValueEnum};

use crate::engine::persistence::{derive_machine_state, derive_state_from_log};
use crate::engine::types::{now_iso8601, Event, EventPayload, FindingDispute, StateFileHeader};
use crate::session::SessionBackend;
use crate::template::types::ACTION_CONDITION_NAME;

use super::{exit_code_for_engine_error, exit_with_error_code, EXIT_INFRASTRUCTURE};

/// The command name every error body carries.
const COMMAND: &str = "disputes record";

/// The gate has no settled evaluation logged in the named state: none at
/// all, or only pending ones.
pub const GATE_NOT_EVALUATED: &str = "gate_not_evaluated";

/// The outcome of a `gate_evaluated` that judged nothing yet.
const PENDING_OUTCOME: &str = "pending";

/// The rule id is empty or longer than [`RULE_ID_MAX_BYTES`].
pub const INVALID_RULE: &str = "invalid_rule";

/// The reason is empty or longer than [`REASON_MAX_BYTES`].
pub const INVALID_REASON: &str = "invalid_reason";

/// Longest rule id, in bytes: the bound koto puts on a finding's rule id.
pub const RULE_ID_MAX_BYTES: usize = crate::findings::RULE_ID_MAX_BYTES;

/// Longest reason, in bytes, as given and as stored after redaction.
pub const REASON_MAX_BYTES: usize = 1000;

/// Who disputed the finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DisputeSource {
    /// The agent running the workflow.
    Agent,
    /// A person.
    Person,
}

impl DisputeSource {
    /// The wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            DisputeSource::Agent => "agent",
            DisputeSource::Person => "person",
        }
    }
}

#[derive(Subcommand)]
pub enum DisputesSubcommand {
    /// Record that a rule's finding at a gate is wrong. Changes no gate
    /// outcome, route or state
    Record {
        /// Workflow name
        name: String,

        /// The gate whose finding is disputed (`__action__` for a state's
        /// default action)
        #[arg(long)]
        gate: String,

        /// The disputed rule's id, 1 to 128 bytes
        #[arg(long)]
        rule: String,

        /// Why the finding is wrong, 1 to 1000 bytes (required)
        #[arg(long)]
        reason: String,

        /// The state the gate was evaluated in (defaults to the current state)
        #[arg(long)]
        state: Option<String>,

        /// Who disputes the finding
        #[arg(long = "by", value_enum, default_value = "agent")]
        by: DisputeSource,
    },
}

/// A typed refusal: exit 2, nothing appended.
fn refuse(code: &str, message: String, gate: &str, state: Option<&str>) -> ! {
    let mut error = serde_json::json!({
        "code": code,
        "message": message,
        "gate": gate,
    });
    if let Some(state) = state {
        error["state"] = serde_json::json!(state);
    }
    exit_with_error_code(serde_json::json!({"error": error, "command": COMMAND}), 2);
}

/// A plain failure with `code` as the exit status.
fn fail(code: i32, message: String) -> ! {
    exit_with_error_code(
        serde_json::json!({"error": message, "command": COMMAND}),
        code,
    );
}

/// Whether `events` hold a settled evaluation of `gate` in `state`: a
/// `gate_evaluated` with an outcome other than `pending` for a gate, a
/// `default_action_executed` for `__action__`, anywhere in the log. A
/// pending evaluation (a polling gate still waiting, a decider check
/// waiting on the agent) judged nothing, so there is nothing to dispute.
pub(crate) fn gate_evaluated_in(events: &[Event], state: &str, gate: &str) -> bool {
    events.iter().any(|e| match &e.payload {
        EventPayload::GateEvaluated {
            state: s,
            gate: g,
            outcome,
            ..
        } => s == state && g == gate && outcome != PENDING_OUTCOME,
        EventPayload::DefaultActionExecuted { state: s, .. } => {
            gate == ACTION_CONDITION_NAME && s == state
        }
        _ => false,
    })
}

/// The known credentials a reason is redacted against: those a tick of
/// this session would redact from its commands' output, from this process's
/// environment and koto's configuration. The template's `pass_env` names
/// are included when the session's template can be read.
fn redactor_for(
    backend: &dyn SessionBackend,
    name: &str,
    header: &StateFileHeader,
    events: &[Event],
) -> crate::redact::Redactor {
    let config = crate::config::resolve::load_config().unwrap_or_default();
    let config_keys = crate::config::redaction_keys(&config);
    // A session without a record redacts against an empty one: the
    // credential carriers and koto's own keys are still searched for.
    let record = header.command_environment.clone().unwrap_or_else(|| {
        serde_json::from_value(serde_json::json!({}))
            .expect("an empty command environment record deserializes")
    });
    let pass_env = derive_machine_state(header, events, &backend.session_dir(name))
        .and_then(|ms| std::fs::read(&ms.template_path).ok())
        .and_then(|bytes| {
            serde_json::from_slice::<crate::template::types::CompiledTemplate>(&bytes).ok()
        })
        .map(|t| t.pass_env)
        .unwrap_or_default();
    crate::engine::command_env::known_credentials(&record, &pass_env, &config_keys, |n| {
        std::env::var(n).ok()
    })
}

pub fn handle(backend: &dyn SessionBackend, subcommand: DisputesSubcommand) -> Result<()> {
    match subcommand {
        DisputesSubcommand::Record {
            name,
            gate,
            rule,
            reason,
            state,
            by,
        } => handle_record(backend, name, gate, rule, reason, state, by),
    }
}

fn handle_record(
    backend: &dyn SessionBackend,
    name: String,
    gate: String,
    rule: String,
    reason: String,
    state: Option<String>,
    by: DisputeSource,
) -> Result<()> {
    if !backend.exists(&name) {
        fail(1, format!("workflow '{}' not found", name));
    }
    if rule.is_empty() || rule.len() > RULE_ID_MAX_BYTES {
        refuse(
            INVALID_RULE,
            format!(
                "--rule must be 1 to {} bytes; got {}",
                RULE_ID_MAX_BYTES,
                rule.len()
            ),
            &gate,
            state.as_deref(),
        );
    }
    if reason.is_empty() || reason.len() > REASON_MAX_BYTES {
        refuse(
            INVALID_REASON,
            format!(
                "--reason must be 1 to {} bytes; got {}",
                REASON_MAX_BYTES,
                reason.len()
            ),
            &gate,
            state.as_deref(),
        );
    }

    let (header, events) = match backend.read_events(&name) {
        Ok(r) => r,
        Err(err) => fail(exit_code_for_engine_error(&err), err.to_string()),
    };
    let state = match state {
        Some(s) => s,
        None => derive_state_from_log(&events).unwrap_or_else(|| {
            fail(
                EXIT_INFRASTRUCTURE,
                "corrupt state file: cannot derive current state".to_string(),
            )
        }),
    };

    if !gate_evaluated_in(&events, &state, &gate) {
        refuse(
            GATE_NOT_EVALUATED,
            format!(
                "gate '{}' has no settled evaluation logged in state '{}': \
                 only a check koto evaluated to an outcome can be disputed, \
                 not one still pending",
                gate, state
            ),
            &gate,
            Some(&state),
        );
    }

    let redactor = redactor_for(backend, &name, &header, &events);
    let (reason, _) = crate::redact::redact_str(&reason, &redactor).cut_bytes(REASON_MAX_BYTES);

    let payload = EventPayload::FindingDisputed(FindingDispute {
        state,
        gate,
        rule_id: rule,
        reason: reason.into_string(),
        source: by.as_str().to_string(),
    });
    if let Err(e) = backend.append_event(&name, &payload, &now_iso8601()) {
        fail(1, e.to_string());
    }

    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({"status": "recorded"}))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(payload: EventPayload) -> Event {
        Event {
            seq: 1,
            timestamp: now_iso8601(),
            event_type: payload.type_name().to_string(),
            payload,
            idempotency_hash: None,
        }
    }

    #[test]
    fn an_evaluation_of_the_gate_in_the_state_is_found_and_no_other() {
        let gate = event(EventPayload::GateEvaluated {
            state: "build".into(),
            gate: "lint".into(),
            output: serde_json::json!({}),
            outcome: "passed".into(),
            timestamp: now_iso8601(),
            check: Default::default(),
            streams: None,
            poll: None,
        });
        let action = event(EventPayload::DefaultActionExecuted {
            state: "ship".into(),
            command: "true".into(),
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            truncated: false,
            check: Default::default(),
        });
        let events = vec![gate, action];
        assert!(gate_evaluated_in(&events, "build", "lint"));
        assert!(!gate_evaluated_in(&events, "ship", "lint"));
        assert!(!gate_evaluated_in(&events, "build", "test"));
        assert!(gate_evaluated_in(&events, "ship", ACTION_CONDITION_NAME));
        assert!(!gate_evaluated_in(&events, "build", ACTION_CONDITION_NAME));
    }

    #[test]
    fn a_pending_evaluation_alone_is_not_one_to_dispute() {
        let gate = |outcome: &str| {
            event(EventPayload::GateEvaluated {
                state: "ci".into(),
                gate: "ci".into(),
                output: serde_json::json!({}),
                outcome: outcome.into(),
                timestamp: now_iso8601(),
                check: Default::default(),
                streams: None,
                poll: None,
            })
        };
        let pending_only = vec![gate("pending"), gate("pending")];
        assert!(!gate_evaluated_in(&pending_only, "ci", "ci"));
        for settled in ["passed", "failed", "timed_out", "error"] {
            let events = vec![gate("pending"), gate(settled)];
            assert!(gate_evaluated_in(&events, "ci", "ci"), "{settled}");
        }
    }

    #[test]
    fn sources_are_written_as_their_wire_values() {
        assert_eq!(DisputeSource::Agent.as_str(), "agent");
        assert_eq!(DisputeSource::Person.as_str(), "person");
    }
}
