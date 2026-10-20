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
//! Beside the event, the dispute is a `judgment` decision in the host-level
//! decision store, about the disputed finding: its subject names the rule,
//! the gate and the state, and points at the workflow. It is written the way
//! `koto decisions record <name>` writes its judgment: the decider resolves
//! before anything is written, the event carries the decision in
//! `decision_records`, the store append follows, and the next `koto next`
//! repairs a lost append. `--by` takes the decider forms every recording
//! command shares, plus `agent`, kept as an alias of `session`; a bare
//! `person` is refused, since a person decider needs a handle. With no
//! decider at all (nothing named, no harness session observed) the dispute
//! is written alone, as before decisions existed: a dispute passes no gate,
//! so nothing is refused.
//!
//! Errors print a JSON body on stdout: exit 1 for a missing session or an
//! append that fails, nothing appended; exit 2 with a typed `code` for a
//! caller problem (a gate not evaluated in the state, a rule id or reason
//! outside its bounds), nothing appended; exit 2 naming the `field` for a
//! malformed decider flag, nothing appended; exit 3 when the session's
//! state can't be derived.

use std::path::PathBuf;

use anyhow::Result;
use clap::Subcommand;

use crate::decision_store::dual_write;
use crate::decision_store::FieldError;
use crate::engine::persistence::{derive_machine_state, derive_state_from_log};
use crate::engine::types::{
    now_iso8601, CheckEventFields, Event, EventPayload, FindingDispute, StateFileHeader,
};
use crate::session::SessionBackend;
use crate::template::types::ACTION_CONDITION_NAME;

use super::decisions::{self, DeciderArgs};
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

/// `--by agent`: the agent running the workflow, an alias of `--by session`.
pub const BY_AGENT: &str = "agent";

/// Who disputed the finding, as the event's `source` records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    #[command(mut_arg("by", |a| a.help(BY_HELP)))]
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

        /// Who disputed the finding and under what lineage. `--by` takes
        /// agent (an alias of session), session, session:<id>,
        /// person:<handle> (with --surface) or check:<name>; with none, the
        /// observed harness session, marked defaulted; with neither, the
        /// dispute is recorded without a decision
        #[command(flatten)]
        decider: Box<DeciderArgs>,
    },
}

/// The help `--by` shows on this command, which also takes `agent`.
pub const BY_HELP: &str = "Who disputes the finding: agent (an alias of session), session \
     (the observed harness session), session:<id>, person:<handle> (with --surface) or \
     check:<name>. With none, the observed harness session, marked defaulted";

/// The decider flags with `--by agent` read as `--by session`, and the
/// `source` the event records: `person` for a `person:<handle>` decider,
/// `agent` for every other. A bare `--by person` names nobody and is
/// refused, naming the form that does.
pub(crate) fn dispute_decider(
    mut args: DeciderArgs,
) -> Result<(DeciderArgs, DisputeSource), FieldError> {
    let source = match args.by.as_deref() {
        Some(BY_AGENT) => {
            args.by = Some("session".to_string());
            DisputeSource::Agent
        }
        Some("person") => {
            return Err(FieldError::new(
                "by",
                "--by person names no one: use --by person:<handle> with --surface \
                 (terminal, forge, relay)",
            ))
        }
        Some(by) if by.starts_with("person:") => DisputeSource::Person,
        _ => DisputeSource::Agent,
    };
    Ok((args, source))
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

/// The check fields of the most recent settled evaluation of `gate` in
/// `state`, the one a dispute names: a `gate_evaluated` with an outcome
/// other than `pending` for a gate, a `default_action_executed` for
/// `__action__`, anywhere in the log. `None` when there is none. A pending
/// evaluation (a polling gate still waiting, a decider check waiting on the
/// agent) judged nothing, so there is nothing to dispute.
pub(crate) fn disputed_evaluation<'e>(
    events: &'e [Event],
    state: &str,
    gate: &str,
) -> Option<&'e CheckEventFields> {
    events.iter().rev().find_map(|e| match &e.payload {
        EventPayload::GateEvaluated {
            state: s,
            gate: g,
            outcome,
            check,
            ..
        } if s == state && g == gate && outcome != PENDING_OUTCOME => Some(check),
        EventPayload::DefaultActionExecuted {
            state: s, check, ..
        } if gate == ACTION_CONDITION_NAME && s == state => Some(check),
        _ => None,
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

/// Run a disputes subcommand. `journal_root` is the run journal of the
/// backend the command writes through, where a decider's lineage is read.
pub fn handle(
    backend: &dyn SessionBackend,
    journal_root: Option<PathBuf>,
    subcommand: DisputesSubcommand,
) -> Result<()> {
    match subcommand {
        DisputesSubcommand::Record {
            name,
            gate,
            rule,
            reason,
            state,
            decider,
        } => handle_record(
            backend,
            journal_root,
            Dispute {
                name,
                gate,
                rule,
                reason,
                state,
            },
            *decider,
        ),
    }
}

/// What a dispute names, as given.
struct Dispute {
    name: String,
    gate: String,
    rule: String,
    reason: String,
    state: Option<String>,
}

/// The `judgment` a dispute records: about the finding of `rule` at `gate`
/// in `state`, pointing at the workflow, with the gate and the rule as
/// evidence and the redacted reason as its reason.
fn dispute_judgment(
    identity: &dual_write::Identity,
    name: &str,
    state: &str,
    gate: &str,
    rule: &str,
    reason: &str,
) -> Result<crate::decision_store::Decision, FieldError> {
    dual_write::judgment(
        identity,
        format!(
            "finding of rule '{}' at gate '{}' in state '{}' disputed",
            rule, gate, state
        ),
        Some(format!("workflow:{}/{}", name, state)),
        vec![format!("gate:{}", gate), format!("rule:{}", rule)],
        Some(reason.to_string()),
    )
}

fn handle_record(
    backend: &dyn SessionBackend,
    journal_root: Option<PathBuf>,
    dispute: Dispute,
    decider: DeciderArgs,
) -> Result<()> {
    let Dispute {
        name,
        gate,
        rule,
        reason,
        state,
    } = dispute;
    if !backend.exists(&name) {
        fail(1, format!("workflow '{}' not found", name));
    }
    let (decider, by) =
        dispute_decider(decider).unwrap_or_else(|e| decisions::fail_command_field(COMMAND, e));
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

    let Some(evaluation) = disputed_evaluation(&events, &state, &gate) else {
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
    };
    let (attempt, visit_attempt) = (evaluation.attempt, evaluation.visit_attempt);

    let redactor = redactor_for(backend, &name, &header, &events);
    let (reason, _) = crate::redact::redact_str(&reason, &redactor).cut_bytes(REASON_MAX_BYTES);

    let reason = reason.into_string();

    // The decider resolves before anything is written. With none, the
    // dispute is written alone and the store isn't opened.
    let mut judgments = Vec::new();
    let mut store = None;
    if let Some(identity) = decisions::resolve_command_decider(COMMAND, &decider, journal_root) {
        store = Some(decisions::command_store(COMMAND));
        judgments.push(
            dispute_judgment(&identity, &name, &state, &gate, &rule, &reason)
                .unwrap_or_else(|e| decisions::fail_command_field(COMMAND, e)),
        );
    }
    let decision_records = match &store {
        Some(store) => dual_write::prepare(store, &mut judgments)
            .unwrap_or_else(|e| decisions::fail_command_store(COMMAND, e)),
        None => Vec::new(),
    };

    let payload = EventPayload::FindingDisputed(FindingDispute {
        state,
        gate,
        rule_id: rule,
        reason,
        source: by.as_str().to_string(),
        attempt,
        visit_attempt,
        decision_records,
    });
    if let Err(e) = backend.append_event(&name, &payload, &now_iso8601()) {
        fail(1, e.to_string());
    }
    if let Some(store) = &store {
        dual_write::append_after_event(store, &name, &judgments);
    }

    let mut body = serde_json::json!({"status": "recorded"});
    if let Some(d) = judgments.first() {
        body["decision"] = serde_json::json!(d.id);
    }
    println!("{}", serde_json::to_string(&body)?);
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

    fn found(events: &[Event], state: &str, gate: &str) -> bool {
        disputed_evaluation(events, state, gate).is_some()
    }

    fn gate_with(outcome: &str, attempt: Option<u64>, visit_attempt: Option<u64>) -> Event {
        event(EventPayload::GateEvaluated {
            state: "ci".into(),
            gate: "ci".into(),
            output: serde_json::json!({}),
            outcome: outcome.into(),
            timestamp: now_iso8601(),
            check: CheckEventFields {
                attempt,
                visit_attempt,
                ..Default::default()
            },
            streams: None,
            poll: None,
        })
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
        assert!(found(&events, "build", "lint"));
        assert!(!found(&events, "ship", "lint"));
        assert!(!found(&events, "build", "test"));
        assert!(found(&events, "ship", ACTION_CONDITION_NAME));
        assert!(!found(&events, "build", ACTION_CONDITION_NAME));
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
        assert!(!found(&pending_only, "ci", "ci"));
        for settled in ["passed", "failed", "timed_out", "error"] {
            let events = vec![gate("pending"), gate(settled)];
            assert!(found(&events, "ci", "ci"), "{settled}");
        }
    }

    /// The dispute names the most recent settled evaluation: a later pending
    /// one is skipped, and an evaluation from before attempt numbers gives
    /// none.
    #[test]
    fn the_disputed_evaluation_is_the_latest_settled_one() {
        let events = vec![
            gate_with("failed", Some(1), Some(1)),
            gate_with("failed", Some(2), Some(2)),
            gate_with("pending", None, None),
        ];
        let e = disputed_evaluation(&events, "ci", "ci").unwrap();
        assert_eq!((e.attempt, e.visit_attempt), (Some(2), Some(2)));
        let old = vec![gate_with("passed", None, None)];
        let e = disputed_evaluation(&old, "ci", "ci").unwrap();
        assert_eq!((e.attempt, e.visit_attempt), (None, None));
    }

    #[test]
    fn sources_are_written_as_their_wire_values() {
        assert_eq!(DisputeSource::Agent.as_str(), "agent");
        assert_eq!(DisputeSource::Person.as_str(), "person");
    }

    fn by(value: Option<&str>) -> Result<(Option<String>, DisputeSource), FieldError> {
        let args = DeciderArgs {
            by: value.map(str::to_string),
            ..Default::default()
        };
        dispute_decider(args).map(|(a, s)| (a.by, s))
    }

    #[test]
    fn agent_is_an_alias_of_session_and_the_default_names_no_one() {
        assert_eq!(by(None).unwrap(), (None, DisputeSource::Agent));
        assert_eq!(
            by(Some("agent")).unwrap(),
            (Some("session".into()), DisputeSource::Agent)
        );
        for other in ["session", "session:s-1", "check:lint"] {
            assert_eq!(
                by(Some(other)).unwrap(),
                (Some(other.into()), DisputeSource::Agent),
                "{other}"
            );
        }
    }

    #[test]
    fn a_person_needs_a_handle() {
        assert_eq!(
            by(Some("person:ana")).unwrap(),
            (Some("person:ana".into()), DisputeSource::Person)
        );
        let e = by(Some("person")).unwrap_err();
        assert_eq!(e.field, "by");
        assert!(e.message.contains("--by person:<handle>"), "{}", e.message);
    }
}
