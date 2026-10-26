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
//! For a unit-mode check (DESIGN-doc-rule-checks-m4.md, Decision 3) the
//! answer names its unit with `--unit <key>`, and the fallback it binds to is
//! that unit's in the check's latest evaluation: the `decider_fallback`
//! events between the check's latest `gate_evaluated` in the visit and the
//! one before it. The units that evaluation printed are its output's
//! `units`.
//!
//! Errors print a JSON body on stdout and exit 2 for a caller problem (an
//! answer outside the three words, a check or criterion the current state
//! doesn't declare, no fallback to answer, a unit missing, unexpected or not
//! printed), 1 when the session is missing or
//! the event can't be appended, and 3 when the session's template can't be
//! read, as `koto overrides record` does.

use anyhow::Result;
use clap::Args;

use crate::cache::sha256_hex;
use crate::decider::check::{AgentAnswer, DeciderCheckAnswered, DeciderFallback};
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

/// The criterion's pending fallbacks name units and no `--unit` was given.
const UNIT_REQUIRED: &str = "unit_required";

/// `--unit` was given for a check, or fallbacks, that name no unit.
const UNIT_NOT_EXPECTED: &str = "unit_not_expected";

/// `--unit` names a key the check's latest evaluation didn't print.
const UNKNOWN_UNIT: &str = "unknown_unit";

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

    /// The unit the answer is for, as the waiting finding names it; needed
    /// for a check that grades units, refused for one that doesn't
    #[arg(long)]
    pub unit: Option<String>,
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

/// A `decider_fallback` with the `seq` of its event.
type Fallback<'e> = (u64, &'e DeciderFallback);

/// The events of `state`'s current visit: everything after the state's
/// latest arrival, the same rule the check evaluator uses to find an
/// applicable answer; the two must agree, or an answer lands where the next
/// tick won't read it.
fn visit_of<'e>(events: &'e [Event], state: &str) -> &'e [Event] {
    match arrival_index(events, state) {
        Some(i) => &events[i + 1..],
        None => events,
    }
}

/// `gate`'s fallbacks for `criterion` in `state` among `events`, newest
/// first.
fn fallbacks<'e>(
    events: &'e [Event],
    state: &'e str,
    gate: &'e str,
    criterion: &'e str,
) -> impl Iterator<Item = Fallback<'e>> {
    events.iter().rev().filter_map(move |e| match &e.payload {
        EventPayload::DeciderFallback(f)
            if f.state == state && f.gate == gate && f.rule_id == criterion =>
        {
            Some((e.seq, f))
        }
        _ => None,
    })
}

/// What the check's latest evaluation in the visit left: the criterion's
/// fallbacks from it (newest first) and the unit keys it printed (`None`
/// when its output lists no `units`). With no `gate_evaluated` in the visit
/// (a log written by hand), every fallback in the visit counts and no unit
/// is known to be printed.
fn latest_evaluation<'e>(
    visit: &'e [Event],
    state: &'e str,
    gate: &'e str,
    criterion: &'e str,
) -> (Vec<Fallback<'e>>, Option<Vec<&'e str>>) {
    let evaluated: Vec<usize> = visit
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            matches!(&e.payload, EventPayload::GateEvaluated { state: s, gate: g, .. }
                if s == state && g == gate)
        })
        .map(|(i, _)| i)
        .collect();
    let Some(&last) = evaluated.last() else {
        return (fallbacks(visit, state, gate, criterion).collect(), None);
    };
    // The fallbacks of one evaluation come right before its
    // `gate_evaluated`, after the previous evaluation's.
    let first = evaluated
        .len()
        .checked_sub(2)
        .map_or(0, |i| evaluated[i] + 1);
    let pending = fallbacks(&visit[first..last], state, gate, criterion).collect();
    let printed = match &visit[last].payload {
        EventPayload::GateEvaluated { output, .. } => output["units"]
            .as_array()
            .map(|units| units.iter().filter_map(|u| u["key"].as_str()).collect()),
        _ => None,
    };
    (pending, printed)
}

pub fn handle_answer(backend: &dyn SessionBackend, args: AnswerArgs) -> Result<()> {
    let AnswerArgs {
        name,
        gate,
        criterion,
        answer,
        unit,
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
    let spec = compiled
        .states
        .get(&state)
        .and_then(|s| s.gates.get(&gate))
        .filter(|g| g.gate_type == GATE_TYPE_DECIDER_CHECK)
        .and_then(|g| g.decider_check.as_ref())
        .filter(|spec| spec.criteria.iter().any(|c| c.rule_id == criterion));
    let Some(spec) = spec else {
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
    };

    let visit = visit_of(&events, &state);
    let (pending, printed) = latest_evaluation(visit, &state, &gate, &criterion);
    let names_unit = pending.iter().any(|(_, f)| f.unit.is_some());
    let refuse_here = |code: &str, message: String| -> ! {
        refuse(code, message, &gate, &criterion, Some(&state))
    };
    let no_fallback = || -> ! {
        refuse_here(
            NO_FALLBACK,
            format!(
                "criterion '{}' of '{}' has not fallen back to you in this visit to '{}': \
                 only a criterion the decider gave no verdict on takes your answer",
                criterion, gate, state
            ),
        )
    };
    let (fallback_seq, fallback) = match &unit {
        None if names_unit => refuse_here(
            UNIT_REQUIRED,
            format!(
                "criterion '{}' of '{}' is waiting on your answer for units; name the unit \
                 with --unit, as the waiting finding does",
                criterion, gate
            ),
        ),
        // A check without units, or a unit-mode check whose latest
        // evaluation named no unit, takes the latest unit-less fallback in
        // the visit, as before units existed.
        None => fallbacks(visit, &state, &gate, &criterion)
            .find(|(_, f)| f.unit.is_none())
            .unwrap_or_else(|| no_fallback()),
        Some(key) => {
            if spec.units.is_none() || (!pending.is_empty() && !names_unit) {
                refuse_here(
                    UNIT_NOT_EXPECTED,
                    format!(
                        "criterion '{}' of '{}' isn't waiting on a unit; answer without --unit",
                        criterion, gate
                    ),
                );
            }
            if !printed.as_ref().is_some_and(|p| p.contains(&key.as_str())) {
                refuse_here(
                    UNKNOWN_UNIT,
                    format!(
                        "the latest evaluation of '{}' printed no unit '{}'; run koto next and \
                         answer a unit its waiting findings name",
                        gate, key
                    ),
                );
            }
            pending
                .iter()
                .find(|(_, f)| f.unit.as_ref().is_some_and(|u| &u.key == key))
                .copied()
                .unwrap_or_else(|| no_fallback())
        }
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
        // Copied so the run journal reads the check's declared labels from
        // this event, as it does from the fallback's.
        unit_labels: fallback.unit_labels.clone(),
    });
    if let Err(e) = backend.append_event(&name, &payload, &now_iso8601()) {
        fail(1, e.to_string());
    }

    let mut body = serde_json::json!({
        "status": "recorded",
        "gate": gate,
        "criterion": criterion,
        "answer": answer.as_str(),
    });
    if let Some(u) = &fallback.unit {
        body["unit"] = serde_json::json!(u.key);
    }
    println!("{}", serde_json::to_string(&body)?);
    Ok(())
}
