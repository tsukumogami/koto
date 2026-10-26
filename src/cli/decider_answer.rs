//! `koto decider answer`: the agent's answer to a decider-check criterion
//! that fell back to it (DESIGN-decider-fallback.md, Decision 1).
//!
//! The command loads the session the way `koto overrides record` does,
//! finds the latest `decider_fallback` for the check and criterion in the
//! current visit, and appends one `decider_check_answered` bound to that
//! fallback's slice and declaration. The next `koto next` reads the answer
//! back for the same slice. It never consults the decider and never takes
//! the decider lock: an evaluation already running doesn't see the answer,
//! and the next one does.
//!
//! Errors print a JSON body on stdout and exit 2 for a caller problem (an
//! answer outside the three words, a check or criterion the current state
//! doesn't declare, no fallback to answer), 1 when the session is missing or
//! the event can't be appended, and 3 when the session's template can't be
//! read, as `koto overrides record` does.

use anyhow::Result;
use clap::Args;

use crate::cache::sha256_hex;
use crate::decider::check::{AgentAnswer, DeciderCheckAnswered};
use crate::engine::persistence::{arrival_index, derive_machine_state};
use crate::engine::types::{now_iso8601, Event, EventPayload};
use crate::session::SessionBackend;
use crate::template::types::GATE_TYPE_DECIDER_CHECK;

use super::{exit_code_for_engine_error, exit_with_error_code, EXIT_INFRASTRUCTURE};

/// The command name every error body carries.
const COMMAND: &str = "decider answer";

/// The answer isn't `pass`, `fail` or `unclear`.
const INVALID_ANSWER: &str = "invalid_answer";

/// The gate isn't a decider check of the current state, or it declares no
/// such criterion.
const UNKNOWN_CRITERION: &str = "unknown_criterion";

/// The criterion has no `decider_fallback` in the current visit.
const NO_FALLBACK: &str = "no_fallback";

/// Flags of `koto decider answer`.
#[derive(Args, Debug, Clone)]
pub struct AnswerArgs {
    /// Workflow name
    pub name: String,

    /// The decider check (gate) whose criterion is waiting on you
    #[arg(long)]
    pub gate: String,

    /// The criterion's id, as the waiting finding names it
    #[arg(long)]
    pub criterion: String,

    /// Your answer: pass, fail, or unclear (the criterion's escape)
    #[arg(long)]
    pub answer: String,
}

/// A typed refusal: exit 2, nothing appended.
fn refuse(code: &str, message: String, gate: &str, criterion: &str, state: Option<&str>) -> ! {
    let mut error = serde_json::json!({
        "code": code,
        "message": message,
        "gate": gate,
        "criterion": criterion,
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

/// The latest `decider_fallback` for `gate`/`criterion` in `state`'s
/// current visit, with its `seq`. The visit starts after the state's latest
/// arrival, the same rule the check evaluator uses to find an applicable
/// answer; the two must agree, or an answer lands where the next tick won't
/// read it.
fn latest_fallback<'e>(
    events: &'e [Event],
    state: &str,
    gate: &str,
    criterion: &str,
) -> Option<(u64, &'e crate::decider::check::DeciderFallback)> {
    let visit = match arrival_index(events, state) {
        Some(i) => &events[i + 1..],
        None => events,
    };
    visit.iter().rev().find_map(|e| match &e.payload {
        EventPayload::DeciderFallback(f)
            if f.state == state && f.gate == gate && f.rule_id == criterion =>
        {
            Some((e.seq, f))
        }
        _ => None,
    })
}

pub fn handle_answer(backend: &dyn SessionBackend, args: AnswerArgs) -> Result<()> {
    let AnswerArgs {
        name,
        gate,
        criterion,
        answer,
    } = args;

    // The answer is checked first: it needs nothing loaded.
    let Some(answer) = AgentAnswer::parse(&answer) else {
        refuse(
            INVALID_ANSWER,
            format!("answer '{}' is not one of pass, fail or unclear", answer),
            &gate,
            &criterion,
            None,
        );
    };

    if !backend.exists(&name) {
        fail(1, format!("workflow '{}' not found", name));
    }
    let (header, events) = match backend.read_events(&name) {
        Ok(r) => r,
        Err(err) => fail(exit_code_for_engine_error(&err), err.to_string()),
    };
    let Some(machine_state) = derive_machine_state(&header, &events, &backend.session_dir(&name))
    else {
        fail(
            EXIT_INFRASTRUCTURE,
            "corrupt state file: cannot derive current state".to_string(),
        );
    };
    let template_bytes = std::fs::read(&machine_state.template_path).unwrap_or_else(|e| {
        fail(
            EXIT_INFRASTRUCTURE,
            format!(
                "failed to read template {}: {}",
                machine_state.template_path, e
            ),
        )
    });
    if sha256_hex(&template_bytes) != machine_state.template_hash {
        fail(
            EXIT_INFRASTRUCTURE,
            "template hash mismatch: the session's template changed".to_string(),
        );
    }
    let compiled: crate::template::types::CompiledTemplate =
        serde_json::from_slice(&template_bytes).unwrap_or_else(|e| {
            fail(
                EXIT_INFRASTRUCTURE,
                format!(
                    "failed to parse template {}: {}",
                    machine_state.template_path, e
                ),
            )
        });
    let state = machine_state.current_state;

    // The gate must be a decider check of the current state that declares
    // the criterion.
    let declared = compiled
        .states
        .get(&state)
        .and_then(|s| s.gates.get(&gate))
        .filter(|g| g.gate_type == GATE_TYPE_DECIDER_CHECK)
        .and_then(|g| g.decider_check.as_ref())
        .is_some_and(|spec| spec.criteria.iter().any(|c| c.rule_id == criterion));
    if !declared {
        refuse(
            UNKNOWN_CRITERION,
            format!(
                "state '{}' has no decider check '{}' with a criterion '{}'",
                state, gate, criterion
            ),
            &gate,
            &criterion,
            Some(&state),
        );
    }

    let Some((fallback_seq, fallback)) = latest_fallback(&events, &state, &gate, &criterion) else {
        refuse(
            NO_FALLBACK,
            format!(
                "criterion '{}' of '{}' has not fallen back to you in this visit to '{}': \
                 only a criterion the decider gave no verdict on takes your answer",
                criterion, gate, state
            ),
            &gate,
            &criterion,
            Some(&state),
        );
    };

    let payload = EventPayload::DeciderCheckAnswered(DeciderCheckAnswered {
        state: state.clone(),
        visit_seq: fallback.visit_seq,
        gate: gate.clone(),
        rule_id: criterion.clone(),
        declaration_hash: fallback.declaration_hash.clone(),
        input_sha256: fallback.input_sha256.clone(),
        answer,
        fallback_seq,
        // An answer binds to the fallback it answers, its unit included.
        unit: fallback.unit.clone(),
    });
    if let Err(e) = backend.append_event(&name, &payload, &now_iso8601()) {
        fail(1, e.to_string());
    }

    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "status": "recorded",
            "gate": gate,
            "criterion": criterion,
            "answer": answer.as_str(),
        }))?
    );
    Ok(())
}
