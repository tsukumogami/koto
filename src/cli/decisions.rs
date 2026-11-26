//! `koto decisions record --kind`, `list --format v1`, `show` and
//! `reverse` (DESIGN-decision-record.md, Decision 4).
//!
//! Every form acts on the session named in its arguments and never reads
//! the session from the environment (PRD R46). The forms that existed
//! before, `record <name> --with-data` and `list <name>` with none of the
//! new flags, keep their behaviour and live in `cli/mod.rs`.

use anyhow::Result;
use clap::Args;

use crate::decisions::body::{
    self, BasisRef, Compensating, DecisionBody, Refusal, Request, SessionRef, Target,
};
use crate::decisions::decider::{self, Flags, Observed};
use crate::decisions::read::{Entry, Filters};
use crate::decisions::standing::{self, reasons, Index};
use crate::decisions::store::Area;
use crate::decisions::{chain, codes, store, DecisionError, FORMAT_VERSION};
use crate::session::{Backend, SessionBackend};

use super::{exit_with_error_code, EXIT_CALLER_ERROR, EXIT_INFRASTRUCTURE};

/// The flags `record` takes for a decision of any kind.
#[derive(Args, Debug, Clone, Default)]
pub struct RecordArgs {
    /// Decision kind: accept, merge, gate-pass, escalate,
    /// decline-escalation, judgment, verdict or refused
    #[arg(long)]
    pub kind: Option<String>,
    /// Outcome, one the kind allows
    #[arg(long)]
    pub outcome: Option<String>,
    /// A subject field as field=value (repeatable; `path` may repeat)
    #[arg(long = "subject", value_name = "FIELD=VALUE")]
    pub subject: Vec<String>,
    /// Who decided: session, person:<handle> or check:<name>
    #[arg(long)]
    pub by: Option<String>,
    /// A person decider's surface: terminal, forge or relay
    #[arg(long)]
    pub surface: Option<String>,
    /// A person decider's link to the original
    #[arg(long)]
    pub link: Option<String>,
    /// The agent within the deciding session that decided
    #[arg(long)]
    pub agent: Option<String>,
    /// The run mode: attended, unattended or unknown
    #[arg(long = "run-mode")]
    pub run_mode: Option<String>,
    /// An earlier decision this one relies on, as <session>:<id> (repeatable)
    #[arg(long)]
    pub basis: Vec<String>,
    /// An evidence pointer (repeatable); event:<session>:<seq> names a koto
    /// session event
    #[arg(long)]
    pub evidence: Vec<String>,
    /// Why
    #[arg(long)]
    pub reason: Option<String>,
    /// The harness's identifier for the tool call that recorded it
    #[arg(long = "tool-call")]
    pub tool_call: Option<String>,
}

impl RecordArgs {
    /// Whether any flag of the new form was given.
    pub fn any(&self) -> bool {
        self.kind.is_some()
            || self.outcome.is_some()
            || !self.subject.is_empty()
            || self.by.is_some()
            || self.surface.is_some()
            || self.link.is_some()
            || self.agent.is_some()
            || self.run_mode.is_some()
            || !self.basis.is_empty()
            || !self.evidence.is_empty()
            || self.reason.is_some()
            || self.tool_call.is_some()
    }
}

/// The decider flags of a command that records a decision alongside its
/// own event: `koto overrides record`, `koto next --to` and `koto decider
/// answer` (PRD R24).
#[derive(Args, Debug, Clone, Default)]
pub struct DeciderArgs {
    /// Who decided: session, person:<handle> or check:<name>. With none,
    /// a gate pass is the agent session's, marked defaulted
    #[arg(long)]
    pub by: Option<String>,
    /// A person decider's surface: terminal, forge or relay
    #[arg(long)]
    pub surface: Option<String>,
    /// A person decider's link to the original
    #[arg(long)]
    pub link: Option<String>,
    /// The agent within the deciding session that decided
    #[arg(long)]
    pub agent: Option<String>,
    /// The run mode: attended, unattended or unknown
    #[arg(long = "run-mode")]
    pub run_mode: Option<String>,
}

impl DeciderArgs {
    pub fn flags(&self) -> Flags {
        Flags {
            by: self.by.clone(),
            surface: self.surface.clone(),
            link: self.link.clone(),
            agent: self.agent.clone(),
            run_mode: self.run_mode.clone(),
        }
    }
}

/// The flags `list` takes beyond the session.
#[derive(Args, Debug, Clone, Default)]
pub struct ListArgs {
    /// Output format; `v1` is the decision record's listing
    #[arg(long)]
    pub format: Option<String>,
    /// List the decisions of every session below the named one
    #[arg(long)]
    pub below: bool,
    /// Only this kind
    #[arg(long = "kind")]
    pub kind: Option<String>,
    /// Only this decider kind: session, person or check
    #[arg(long = "decider")]
    pub decider: Option<String>,
    /// Only decisions whose subject field equals the value (repeatable)
    #[arg(long = "subject", value_name = "FIELD=VALUE")]
    pub subject: Vec<String>,
    /// Only decisions at or after this time (RFC 3339 UTC)
    #[arg(long)]
    pub since: Option<String>,
    /// Only decisions before this time (RFC 3339 UTC)
    #[arg(long)]
    pub until: Option<String>,
    /// Only this run mode
    #[arg(long = "run-mode")]
    pub run_mode: Option<String>,
    /// Only decisions whose decider was (true) or wasn't (false) defaulted
    #[arg(long)]
    pub defaulted: Option<bool>,
    /// Only reversals whose standing path holds a supersede hop from this
    /// fenced session: the reversals that one fence enabled
    #[arg(long, value_name = "SESSION")]
    pub fenced: Option<String>,
}

impl ListArgs {
    /// Whether any flag of the new form was given; with none, `list`
    /// answers as it always has.
    pub fn any(&self) -> bool {
        self.format.is_some()
            || self.below
            || self.kind.is_some()
            || self.decider.is_some()
            || !self.subject.is_empty()
            || self.since.is_some()
            || self.until.is_some()
            || self.run_mode.is_some()
            || self.defaulted.is_some()
            || self.fenced.is_some()
    }
}

/// Print a decision refusal for `command` and exit: 3 for a write that
/// failed, 2 for anything the caller has to change.
pub(crate) fn refuse(command: &str, err: &DecisionError) -> ! {
    let mut out = serde_json::json!({
        "error": err.message,
        "code": err.code,
        "command": command,
    });
    if let Some(field) = &err.field {
        out["field"] = serde_json::json!(field);
    }
    let code = if err.code == "decision_write_failed" {
        EXIT_INFRASTRUCTURE
    } else {
        EXIT_CALLER_ERROR
    };
    exit_with_error_code(out, code)
}

fn split_pairs(command: &str, field: &str, pairs: &[String]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|p| match p.split_once('=') {
            Some((f, v)) => (f.trim().to_string(), v.to_string()),
            None => refuse(
                command,
                &DecisionError::invalid(field, format!("--{field} takes FIELD=VALUE")),
            ),
        })
        .collect()
}

/// `koto decisions record <session> --kind <k> --outcome <o> ...`.
pub fn handle_record(backend: &Backend, name: &str, args: RecordArgs) -> Result<()> {
    const COMMAND: &str = "decisions record";
    let kind = args.kind.clone().unwrap_or_default();
    if kind.is_empty() {
        refuse(
            COMMAND,
            &DecisionError::invalid("kind", "--kind is required"),
        );
    }
    if kind == "reverse" {
        refuse(
            COMMAND,
            &DecisionError::invalid(
                "kind",
                "a reversal isn't recorded with `koto decisions record`: it is aimed at the \
                 decision it overrules",
            ),
        );
    }
    if kind == "answer" {
        refuse(
            COMMAND,
            &DecisionError::invalid(
                "kind",
                "an answer isn't recorded with `koto decisions record`: it is aimed at the \
                 question it answers, with `koto decisions answer`",
            ),
        );
    }
    if kind == "readdress" {
        refuse(
            COMMAND,
            &DecisionError::invalid(
                "kind",
                "a pass-up isn't recorded with `koto decisions record`: it is aimed at the \
                 question it passes up, with `koto decisions readdress`",
            ),
        );
    }
    let req = Request {
        kind,
        outcome: args.outcome.clone().unwrap_or_default(),
        subject: split_pairs(COMMAND, "subject", &args.subject),
        basis: args.basis.clone(),
        evidence: args.evidence.clone(),
        reason: args.reason.clone(),
        tool_call: args.tool_call.clone(),
    };
    let subject = body::validate(&req).unwrap_or_else(|e| refuse(COMMAND, &e));
    let basis = body::parse_basis(&req.basis).unwrap_or_else(|e| refuse(COMMAND, &e));
    let evidence = body::parse_evidence(&req.evidence).unwrap_or_else(|e| refuse(COMMAND, &e));

    if !backend.exists(name) {
        exit_with_error_code(
            serde_json::json!({
                "error": format!("no state file found for workflow '{}'", name),
                "command": COMMAND
            }),
            EXIT_CALLER_ERROR,
        );
    }
    if let Err(e) = backend.check_not_migrated(name) {
        let code = if e.is::<crate::session::SessionMigrated>() {
            "session_migrated"
        } else {
            "session_unreadable"
        };
        exit_with_error_code(
            serde_json::json!({"error": e.to_string(), "code": code, "command": COMMAND}),
            EXIT_CALLER_ERROR,
        );
    }

    let flags = Flags {
        by: args.by.clone(),
        surface: args.surface.clone(),
        link: args.link.clone(),
        agent: args.agent.clone(),
        run_mode: args.run_mode.clone(),
    };
    let observed = Observed::from_env(backend);
    let decider = decider::resolve(name, &flags, &observed).unwrap_or_else(|e| refuse(COMMAND, &e));
    let run_mode = decider::run_mode(&flags, &observed).unwrap_or_else(|e| refuse(COMMAND, &e));
    let authority = decider::authority(
        backend,
        &decider,
        observed.anchor.as_deref(),
        chain::dispatcher(backend, name),
        run_mode,
    );
    let chain = chain::snapshot(backend, name);

    let (header, events) = match backend.read_events(name) {
        Ok(read) => read,
        Err(e) => exit_with_error_code(
            serde_json::json!({"error": e.to_string(), "command": COMMAND}),
            super::exit_code_for_engine_error(&e),
        ),
    };
    let state = crate::engine::persistence::derive_state_from_log(&events).unwrap_or_default();
    let log_seq = events.last().map(|e| e.seq).unwrap_or(0);

    // A reversal entered through this session may have lost since (R19).
    standing::settle(backend, name);
    let written = store::write(backend, name, |id| {
        Ok(DecisionBody {
            v: FORMAT_VERSION,
            id: id.to_string(),
            session: SessionRef {
                name: name.to_string(),
                id: header.session_id.clone(),
            },
            time: crate::engine::types::now_iso8601(),
            state,
            log_seq,
            kind: req.kind.clone(),
            outcome: req.outcome.clone(),
            subject,
            decider,
            authority,
            chain,
            basis,
            evidence,
            reason: req.reason.clone(),
            tool_call: req.tool_call.clone(),
            target: None,
            standing: None,
            path: None,
            compensating: None,
            replacement: None,
            refusal: None,
        })
    })
    .unwrap_or_else(|e| refuse(COMMAND, &e));

    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "v": FORMAT_VERSION,
            "session": name,
            "id": written.id,
            "status": if written.kind == "refused" { "refused" } else { "in-effect" },
            "decision": written,
        }))?
    );
    Ok(())
}

/// Record the state choice `koto decisions record <session> --with-data`
/// just appended as a `judgment` (R25): the choice as what was judged, its
/// rationale as the reason, and the session as the decider. The command's
/// output is unchanged: a decision that can't be written is one warning.
pub fn record_choice_judgment(
    backend: &Backend,
    name: &str,
    session_id: &str,
    events: &[crate::engine::types::Event],
    choice: String,
    rationale: Option<String>,
) {
    use crate::decisions::gate_pass;
    let written = (|| {
        let flags = Flags::default();
        let observed = Observed::from_env(backend);
        let decider = gate_pass::judgment_decider(name, &flags, &observed)?;
        let state = crate::engine::persistence::derive_state_from_log(events).unwrap_or_default();
        let at = gate_pass::At {
            session: name,
            session_id,
            state: &state,
            log_seq: events.last().map(|e| e.seq).unwrap_or(0),
        };
        let req = gate_pass::judgment(choice, rationale, vec![]);
        let prepared = gate_pass::Prepared::new(backend, at, req, decider, &flags, &observed)?;
        gate_pass::record(backend, &prepared)
    })();
    if let Err(e) = written {
        eprintln!(
            "warning: the choice recorded in '{name}' wasn't recorded as a decision: {}",
            e.message
        );
    }
}

/// The `gate-pass` of a `koto next --to` that moves past `gates` (the
/// overridable gates it skips that haven't passed, joined with `,`), ready
/// to be written with the `directed_transition` event. Refuses the tick in
/// `koto next`'s error shape, with nothing appended, when no decider is
/// established (a `refused` decision is recorded then) or a flag is
/// malformed.
#[allow(clippy::too_many_arguments)]
pub fn directed_gate_pass(
    backend: &Backend,
    name: &str,
    session_id: &str,
    state: &str,
    events: &[crate::engine::types::Event],
    gates: &str,
    rationale: Option<String>,
    args: &DeciderArgs,
) -> crate::decisions::gate_pass::Prepared {
    use super::next_types::{NextError, NextErrorCode};
    use crate::decisions::gate_pass;
    let flags = args.flags();
    let observed = Observed::from_env(backend);
    let at = gate_pass::At {
        session: name,
        session_id,
        state,
        log_seq: events.last().map(|e| e.seq).unwrap_or(0),
    };
    let reason = rationale.filter(|r| !r.trim().is_empty());
    gate_pass::resolve_or_refuse(backend, at, gates, &flags, &observed)
        .and_then(|decider| {
            gate_pass::Prepared::new(
                backend,
                at,
                gate_pass::gate_pass(at, gates, reason),
                decider,
                &flags,
                &observed,
            )
        })
        .unwrap_or_else(|e| {
            let code = if e.code == codes::DECIDER_MISSING {
                NextErrorCode::DeciderMissing
            } else {
                NextErrorCode::InvalidDecision
            };
            let err = NextError {
                code,
                message: format!("cannot take --to past {gates}: {}", e.message),
                details: vec![],
            };
            let json = serde_json::json!({"error": err});
            exit_with_error_code(json, err.code.exit_code())
        })
}

/// `koto decisions list <session> --format v1 [--below] [filters]`.
pub fn handle_list(backend: &Backend, name: &str, args: &ListArgs) -> Result<()> {
    const COMMAND: &str = "decisions list";
    if args.format.as_deref().is_some_and(|f| f != "v1") {
        refuse(
            COMMAND,
            &DecisionError::invalid("format", "--format takes v1"),
        );
    }
    if let Some(d) = &args.decider {
        if !["session", "person", "check"].contains(&d.as_str()) {
            refuse(
                COMMAND,
                &DecisionError::invalid("decider", "--decider takes session, person or check"),
            );
        }
    }
    if let Some(m) = &args.run_mode {
        if !decider::RUN_MODES.contains(&m.as_str()) {
            refuse(
                COMMAND,
                &DecisionError::invalid(
                    "run_mode",
                    "--run-mode takes attended, unattended or unknown",
                ),
            );
        }
    }
    if let Some(f) = &args.fenced {
        if crate::discover::validate_workflow_name(f).is_err() {
            refuse(
                COMMAND,
                &DecisionError::invalid("fenced", "--fenced takes a session name"),
            );
        }
    }
    if !chain::session_present(backend, name) && !backend_holds_remote(backend, name) {
        exit_with_error_code(
            serde_json::json!({
                "error": format!("no state file found for workflow '{}'", name),
                "command": COMMAND
            }),
            EXIT_CALLER_ERROR,
        );
    }
    let filters = Filters {
        kind: args.kind.clone(),
        decider: args.decider.clone(),
        subject: split_pairs(COMMAND, "subject", &args.subject),
        since: args.since.clone(),
        until: args.until.clone(),
        run_mode: args.run_mode.clone(),
        defaulted: args.defaulted,
        fenced: args.fenced.clone(),
    };
    println!(
        "{}",
        serde_json::to_string(&standing::list(backend, name, args.below, &filters))?
    );
    Ok(())
}

/// `koto decisions show <session> <id>`.
pub fn handle_show(backend: &Backend, name: &str, id: &str) -> Result<()> {
    const COMMAND: &str = "decisions show";
    if !crate::decisions::is_decision_id(id) {
        refuse(
            COMMAND,
            &DecisionError::invalid(
                "id",
                format!(
                    "'{id}' isn't a decision id: {} digits",
                    crate::decisions::ID_WIDTH
                ),
            ),
        );
    }
    if !chain::session_present(backend, name) && !backend_holds_remote(backend, name) {
        exit_with_error_code(
            serde_json::json!({
                "error": format!("no state file found for workflow '{}'", name),
                "command": COMMAND
            }),
            EXIT_CALLER_ERROR,
        );
    }
    match standing::show(backend, name, id) {
        Some(out) => println!("{}", serde_json::to_string(&out)?),
        None => refuse(
            COMMAND,
            &DecisionError::new(
                codes::DECISION_NOT_FOUND,
                format!("'{name}' holds no decision {id}"),
            ),
        ),
    }
    Ok(())
}

/// `koto decisions chain <name>`: print the chain read
/// ([`chain::read`]), `{"v", "session", "marker", "links", "incomplete"}`,
/// recording nothing. The read reaches only the sessions this host holds a
/// copy of, as the walk standing uses does, so a session this host doesn't
/// hold is refused as `show` refuses it.
pub fn handle_chain(backend: &Backend, name: &str) -> Result<()> {
    if !chain::session_present(backend, name) {
        exit_with_error_code(
            serde_json::json!({
                "error": format!("no state file found for workflow '{}'", name),
                "command": "decisions chain"
            }),
            EXIT_CALLER_ERROR,
        );
    }
    println!("{}", serde_json::to_string(&chain::read(backend, name))?);
    Ok(())
}

/// Whether the bucket holds `name` though this host has no copy: a read
/// answers from the bucket's objects then.
fn backend_holds_remote(backend: &Backend, name: &str) -> bool {
    match backend {
        Backend::Cloud(c) => c.remote_header(name).ok().flatten().is_some(),
        Backend::Local(_) => false,
    }
}

/// The flags `reverse` takes beyond the original's session and id.
#[derive(Args, Debug, Clone, Default)]
pub struct ReverseArgs {
    /// The reverser's own session. The reversal is recorded there when it
    /// is on the original's chain, otherwise in its anchor
    #[arg(long = "in", value_name = "SESSION", required = true)]
    pub in_session: String,
    /// A link to the compensating change (merge, verdict or judgment);
    /// without it the reversal records that none exists yet
    #[arg(long)]
    pub compensating: Option<String>,
    /// The outcome that replaces a reversed verdict or judgment
    #[arg(long)]
    pub replacement: Option<String>,
    /// Who decided: session, person:<handle> or check:<name>
    #[arg(long)]
    pub by: Option<String>,
    /// A person decider's surface: terminal, forge or relay
    #[arg(long)]
    pub surface: Option<String>,
    /// A person decider's link to the original
    #[arg(long)]
    pub link: Option<String>,
    /// The agent within the deciding session that decided
    #[arg(long)]
    pub agent: Option<String>,
    /// The run mode: attended, unattended or unknown
    #[arg(long = "run-mode")]
    pub run_mode: Option<String>,
    /// An earlier decision this one relies on, as <session>:<id> (repeatable)
    #[arg(long)]
    pub basis: Vec<String>,
    /// An evidence pointer (repeatable); event:<session>:<seq> names a koto
    /// session event
    #[arg(long)]
    pub evidence: Vec<String>,
    /// Why
    #[arg(long)]
    pub reason: Option<String>,
    /// The harness's identifier for the tool call that recorded it
    #[arg(long = "tool-call")]
    pub tool_call: Option<String>,
}

/// The fault-injection seam for the binary's tests, in the pattern of the
/// other `KOTO_TEST_*` hooks: a debug build reads it, a release build never
/// does. `hide_competitors` hides every other reversal of the same decision
/// from `reverse`; `hide_competitors_before_write` hides them only from the
/// check made before the reversal is written.
const FAULT_ENV: &str = "KOTO_TEST_DECISIONS_FAULT";

fn fault(point: &str) -> bool {
    cfg!(debug_assertions) && std::env::var(FAULT_ENV).as_deref() == Ok(point)
}

/// Exit unless `name` is a session in the store that hasn't moved.
fn require_session(backend: &Backend, name: &str, command: &str) {
    if !backend.exists(name) {
        exit_with_error_code(
            serde_json::json!({
                "error": format!("no state file found for workflow '{}'", name),
                "command": command
            }),
            EXIT_CALLER_ERROR,
        );
    }
    if let Err(e) = backend.check_not_migrated(name) {
        let code = if e.is::<crate::session::SessionMigrated>() {
            "session_migrated"
        } else {
            "session_unreadable"
        };
        exit_with_error_code(
            serde_json::json!({"error": e.to_string(), "code": code, "command": command}),
            EXIT_CALLER_ERROR,
        );
    }
}

/// Print a `reversal_refused` refusal naming the `refused` decision it
/// recorded, and exit.
fn refuse_reversal(
    reason: &str,
    message: &str,
    winner: Option<&BasisRef>,
    recorded: Option<&DecisionBody>,
) -> ! {
    let mut out = serde_json::json!({
        "error": message,
        "code": codes::REVERSAL_REFUSED,
        "reason": reason,
        "command": "decisions reverse",
    });
    if let Some(r) = recorded {
        out["refused"] = serde_json::json!({"session": r.session.name, "id": r.id});
    }
    if let Some(w) = winner {
        out["winner"] = serde_json::json!(w);
    }
    exit_with_error_code(out, EXIT_CALLER_ERROR)
}

/// `koto decisions reverse <session> <id> --in <session> ...` (R17 to R21).
pub fn handle_reverse(backend: &Backend, name: &str, id: &str, args: ReverseArgs) -> Result<()> {
    const COMMAND: &str = "decisions reverse";
    if !crate::decisions::is_decision_id(id) {
        refuse(
            COMMAND,
            &DecisionError::invalid(
                "id",
                format!(
                    "'{id}' isn't a decision id: {} digits",
                    crate::decisions::ID_WIDTH
                ),
            ),
        );
    }
    if crate::discover::validate_workflow_name(name).is_err() {
        refuse(
            COMMAND,
            &DecisionError::invalid("session", "the original's session isn't a session name"),
        );
    }
    let reverser = args.in_session.clone();
    let req = Request {
        kind: "reverse".to_string(),
        outcome: "reversed".to_string(),
        subject: vec![
            ("session".to_string(), name.to_string()),
            ("id".to_string(), id.to_string()),
        ],
        basis: args.basis.clone(),
        evidence: args.evidence.clone(),
        reason: args.reason.clone(),
        tool_call: args.tool_call.clone(),
    };
    let subject = body::validate(&req).unwrap_or_else(|e| refuse(COMMAND, &e));
    let basis = body::parse_basis(&req.basis).unwrap_or_else(|e| refuse(COMMAND, &e));
    let evidence = body::parse_evidence(&req.evidence).unwrap_or_else(|e| refuse(COMMAND, &e));
    if let Some(link) = &args.compensating {
        if link.len() > 2048 || link.chars().any(char::is_control) || url::Url::parse(link).is_err()
        {
            refuse(
                COMMAND,
                &DecisionError::invalid(
                    "compensating",
                    "--compensating takes a URL of at most 2048 bytes",
                ),
            );
        }
    }
    require_session(backend, &reverser, COMMAND);

    let flags = Flags {
        by: args.by.clone(),
        surface: args.surface.clone(),
        link: args.link.clone(),
        agent: args.agent.clone(),
        run_mode: args.run_mode.clone(),
    };
    let observed = Observed::from_env(backend);
    let decider =
        decider::resolve(&reverser, &flags, &observed).unwrap_or_else(|e| refuse(COMMAND, &e));
    let run_mode = decider::run_mode(&flags, &observed).unwrap_or_else(|e| refuse(COMMAND, &e));
    let anchor = observed.anchor.as_deref();
    let authority_in = |session: &str| {
        decider::authority(
            backend,
            &decider,
            anchor,
            chain::dispatcher(backend, session),
            run_mode.clone(),
        )
    };

    // A reversal this session recorded earlier may have lost since.
    standing::settle(backend, &reverser);

    let hide_all = fault("hide_competitors");
    let hide_before = hide_all || fault("hide_competitors_before_write");
    let aimed_at = format!("{name}:{id}");

    // Every refusal from here on is recorded in the reverser's session.
    let refuse_recorded = |reason: &'static str,
                           message: String,
                           target: Option<Target>,
                           winner: Option<BasisRef>|
     -> ! {
        let stamp = standing::stamp(backend, &reverser).unwrap_or_else(|e| refuse(COMMAND, &e));
        let recorded = store::write_in(backend, &reverser, &Area::Decisions, |rid| {
            standing::refused_body(
                rid,
                &stamp,
                decider.clone(),
                authority_in(&reverser),
                chain::snapshot(backend, &reverser),
                standing::RefusedReversal {
                    aimed_at: aimed_at.clone(),
                    target: target.clone(),
                    reason: message.clone(),
                    refusal: Refusal {
                        reason: reason.to_string(),
                        winner: winner.clone(),
                        reversal: None,
                    },
                },
            )
        })
        .unwrap_or_else(|e| refuse(COMMAND, &e));
        refuse_reversal(reason, &message, winner.as_ref(), Some(&recorded))
    };

    let index = Index::scan(backend, false);
    let entries = index.entries(backend, name);
    let found = entries.iter().find(|e| match e {
        Entry::Decision(b) => b.id == id,
        Entry::Unreadable { id: Some(i), .. } => i == id,
        Entry::Unreadable { id: None, .. } => false,
    });
    let original = match found {
        None => refuse_recorded(
            reasons::NOT_FOUND,
            format!("'{name}' holds no decision {id}"),
            None,
            None,
        ),
        Some(Entry::Unreadable { reason, .. }) => refuse_recorded(
            reasons::UNREADABLE,
            format!("decision {id} in '{name}' can't be read: {reason}"),
            None,
            None,
        ),
        Some(Entry::Decision(b)) => (**b).clone(),
    };
    let target = Target {
        name: name.to_string(),
        id: original.session.id.clone(),
        decision: id.to_string(),
    };
    match original.kind.as_str() {
        "answer" => refuse_recorded(
            reasons::REVERSES_ANSWER,
            format!("decision {id} in '{name}' is an answer, and an answer isn't reversed"),
            Some(target),
            None,
        ),
        "readdress" => refuse_recorded(
            reasons::REVERSES_READDRESS,
            format!("decision {id} in '{name}' is a pass-up, and a pass-up isn't reversed"),
            Some(target),
            None,
        ),
        "reverse" => refuse_recorded(
            reasons::REVERSES_REVERSAL,
            format!("decision {id} in '{name}' is a reversal, and a reversal isn't reversed"),
            Some(target),
            None,
        ),
        "refused" => refuse_recorded(
            reasons::REVERSES_REFUSAL,
            format!("decision {id} in '{name}' is a refusal, and a refusal isn't reversed"),
            Some(target),
            None,
        ),
        _ => {}
    }
    // What R21 lets a reversal of this kind carry. A flag the kind can't
    // take is the caller's error, refused before anything is recorded.
    let kind = original.kind.as_str();
    if args.compensating.is_some() && !["merge", "verdict", "judgment"].contains(&kind) {
        refuse(
            COMMAND,
            &DecisionError::invalid(
                "compensating",
                format!("a reversal of a '{kind}' takes no --compensating link"),
            ),
        );
    }
    if let Some(r) = &args.replacement {
        let allowed = ["verdict", "judgment"].contains(&kind)
            && body::outcomes_of(kind).is_some_and(|o| o.contains(&r.as_str()));
        if !allowed {
            refuse(
                COMMAND,
                &DecisionError::invalid(
                    "replacement",
                    format!("'{r}' can't replace the outcome of a '{kind}'"),
                ),
            );
        }
    }
    let compensating = ["merge", "verdict", "judgment"]
        .contains(&kind)
        .then(|| Compensating {
            link: args.compensating.clone(),
        });

    if !hide_before {
        if let Some(w) = index.winner_of(&target.id, id) {
            let w = BasisRef {
                session: w.session.name.clone(),
                id: w.id.clone(),
            };
            refuse_recorded(
                reasons::ALREADY_REVERSED,
                format!(
                    "decision {id} in '{name}' is already reversed by {}:{}",
                    w.session, w.id
                ),
                Some(target),
                Some(w),
            );
        }
    }

    let placement = match standing::judge(backend, &original, &reverser, &decider, anchor) {
        Ok(p) => p,
        Err(reason) if reason == reasons::SUPERSEDE_UNCONFIRMED => refuse_recorded(
            reason,
            format!(
                "'{reverser}' would have standing to reverse decision {id} in '{name}' only \
                 through a supersede hop koto can't confirm ({reason})"
            ),
            Some(target),
            None,
        ),
        Err(reason) => refuse_recorded(
            reason,
            format!("'{reverser}' has no standing to reverse decision {id} in '{name}' ({reason})"),
            Some(target),
            None,
        ),
    };

    let recorded_in = placement.session.clone();
    let stamp = standing::stamp(backend, &recorded_in).unwrap_or_else(|e| refuse(COMMAND, &e));
    let authority = authority_in(&recorded_in);
    let chain = chain::snapshot(backend, &recorded_in);
    let written = store::write_in(
        backend,
        &recorded_in,
        &Area::Aimed(target.id.clone()),
        |rid| {
            Ok(DecisionBody {
                v: FORMAT_VERSION,
                id: rid.to_string(),
                session: stamp.session.clone(),
                time: crate::engine::types::now_iso8601(),
                state: stamp.state.clone(),
                log_seq: stamp.log_seq,
                kind: "reverse".to_string(),
                outcome: "reversed".to_string(),
                subject,
                decider: decider.clone(),
                authority,
                chain,
                basis,
                evidence,
                reason: req.reason.clone(),
                tool_call: req.tool_call.clone(),
                target: Some(target.clone()),
                standing: Some(placement.standing.to_string()),
                path: Some(placement.path.clone()),
                compensating,
                replacement: args.replacement.clone(),
                refusal: None,
            })
        },
    )
    .unwrap_or_else(|e| refuse(COMMAND, &e));

    // R19: read the competing reversals this host can reach now.
    let index = Index::scan(backend, true);
    let is_ours = |b: &DecisionBody| b.session.name == written.session.name && b.id == written.id;
    let competitors = index
        .reversals_of(&target.id, id)
        .iter()
        .filter(|b| !hide_all || is_ours(b));
    if let Some(w) = standing::winner(competitors).filter(|w| !is_ours(w)) {
        let winner = BasisRef {
            session: w.session.name.clone(),
            id: w.id.clone(),
        };
        // The refusal goes in the reverser's own session even when the
        // reversal sits in its anchor (R19).
        let recorded = standing::record_lost(backend, &reverser, &written, w)
            .unwrap_or_else(|e| refuse(COMMAND, &e));
        let message = standing::lost_message(&written, w);
        refuse_reversal(
            reasons::LOST_RACE,
            &message,
            Some(&winner),
            recorded.as_ref(),
        );
    }

    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "v": FORMAT_VERSION,
            "session": recorded_in,
            "id": written.id,
            "status": "in-effect",
            "decision": written,
        }))?
    );
    Ok(())
}

/// The flags `answer` takes beyond the question's session and id.
#[derive(Args, Debug, Clone, Default)]
pub struct AnswerArgs {
    /// The session the answer is given in. The answer is recorded there,
    /// never in the question's session
    #[arg(long = "in", value_name = "SESSION", required = true)]
    pub in_session: String,
    /// The answer
    #[arg(long, required = true)]
    pub text: String,
    /// Who answered: session. A person's answer can't be recorded yet
    #[arg(long)]
    pub by: Option<String>,
    /// The agent within the answering session that answered
    #[arg(long)]
    pub agent: Option<String>,
    /// The run mode: attended, unattended or unknown
    #[arg(long = "run-mode")]
    pub run_mode: Option<String>,
    /// An earlier decision this one relies on, as <session>:<id> (repeatable)
    #[arg(long)]
    pub basis: Vec<String>,
    /// An evidence pointer (repeatable); event:<session>:<seq> names a koto
    /// session event
    #[arg(long)]
    pub evidence: Vec<String>,
    /// Why
    #[arg(long)]
    pub reason: Option<String>,
    /// The harness's identifier for the tool call that recorded it
    #[arg(long = "tool-call")]
    pub tool_call: Option<String>,
}

/// `koto decisions answer <session> <id> --in <session> --text <text>`:
/// record an `answer` to the escalation `id` raised in `name`, in the
/// answering session's `koto/aimed/<raiser's session id>/`. Every check
/// runs before anything is written, and a refusal writes nothing in
/// either session.
pub fn handle_answer(backend: &Backend, name: &str, id: &str, args: AnswerArgs) -> Result<()> {
    const COMMAND: &str = "decisions answer";
    if !crate::decisions::is_decision_id(id) {
        refuse(
            COMMAND,
            &DecisionError::invalid(
                "id",
                format!(
                    "'{id}' isn't a decision id: {} digits",
                    crate::decisions::ID_WIDTH
                ),
            ),
        );
    }
    if crate::discover::validate_workflow_name(name).is_err() {
        refuse(
            COMMAND,
            &DecisionError::invalid("session", "the question's session isn't a session name"),
        );
    }
    let answerer = args.in_session.clone();
    if answerer == name {
        refuse(
            COMMAND,
            &DecisionError::invalid(
                "in",
                "an answer is recorded in the session it is given in, never in the asker's",
            ),
        );
    }
    let person_refusal = || {
        DecisionError::invalid(
            "by",
            "a person's answer can't be recorded yet: koto doesn't verify that the person \
             gave it",
        )
    };
    if args.by.as_deref().is_some_and(|b| b.starts_with("person:")) {
        refuse(COMMAND, &person_refusal());
    }
    let req = Request {
        kind: "answer".to_string(),
        outcome: "answered".to_string(),
        subject: vec![
            ("session".to_string(), name.to_string()),
            ("id".to_string(), id.to_string()),
            ("text".to_string(), args.text.clone()),
        ],
        basis: args.basis.clone(),
        evidence: args.evidence.clone(),
        reason: args.reason.clone(),
        tool_call: args.tool_call.clone(),
    };
    let subject = body::validate(&req).unwrap_or_else(|e| refuse(COMMAND, &e));
    if args.text.len() > body::MAX_REASON_BYTES {
        refuse(
            COMMAND,
            &DecisionError::invalid(
                "text",
                format!("the answer is larger than {} bytes", body::MAX_REASON_BYTES),
            ),
        );
    }
    let basis = body::parse_basis(&req.basis).unwrap_or_else(|e| refuse(COMMAND, &e));
    let evidence = body::parse_evidence(&req.evidence).unwrap_or_else(|e| refuse(COMMAND, &e));
    require_session(backend, &answerer, COMMAND);

    let flags = Flags {
        by: args.by.clone(),
        surface: None,
        link: None,
        agent: args.agent.clone(),
        run_mode: args.run_mode.clone(),
    };
    let observed = Observed::from_env(backend);
    let decider =
        decider::resolve(&answerer, &flags, &observed).unwrap_or_else(|e| refuse(COMMAND, &e));
    if !matches!(decider, decider::Decider::Session { .. }) {
        refuse(COMMAND, &person_refusal());
    }
    let run_mode = decider::run_mode(&flags, &observed).unwrap_or_else(|e| refuse(COMMAND, &e));

    // The question, read from the asker's own copy with no lock and no
    // pull: nothing in the asker's session is touched.
    // An unreadable file whose name isn't a decision id (a stray file in
    // the directory) is no candidate for the id asked for.
    let entries = crate::decisions::read::read_session_with(backend, name, false);
    let found = entries.iter().find(|e| match e {
        Entry::Decision(b) => b.id == id,
        Entry::Unreadable { id: Some(i), .. } => i == id,
        Entry::Unreadable { id: None, .. } => false,
    });
    let question = match found {
        None => refuse(
            COMMAND,
            &DecisionError::new(
                codes::DECISION_NOT_FOUND,
                format!("'{name}' holds no decision {id}"),
            ),
        ),
        Some(Entry::Unreadable { reason, .. }) => refuse(
            COMMAND,
            &DecisionError::invalid(
                "id",
                format!("decision {id} in '{name}' can't be read: {reason}"),
            ),
        ),
        Some(Entry::Decision(b)) => (**b).clone(),
    };
    if question.kind != "escalate" {
        refuse(
            COMMAND,
            &DecisionError::invalid(
                "id",
                format!(
                    "decision {id} in '{name}' is a '{}', and only an 'escalate' is answered",
                    question.kind
                ),
            ),
        );
    }
    if !crate::decisions::questions::is_question(&question) {
        refuse(
            COMMAND,
            &DecisionError::invalid(
                "id",
                format!(
                    "decision {id} in '{name}' has no typed recipient (session:, anchor: or \
                     person:), so it isn't a question koto answers"
                ),
            ),
        );
    }
    let status = Index::scan(backend, false).status(&question);
    if status != "in-effect" {
        refuse(
            COMMAND,
            &DecisionError::invalid(
                "id",
                format!(
                    "decision {id} in '{name}' reads '{status}', and only a question in effect \
                     is answered"
                ),
            ),
        );
    }
    let target = Target {
        name: name.to_string(),
        id: question.session.id.clone(),
        decision: id.to_string(),
    };

    standing::settle(backend, &answerer);
    let stamp = standing::stamp(backend, &answerer).unwrap_or_else(|e| refuse(COMMAND, &e));
    let authority = decider::authority(
        backend,
        &decider,
        observed.anchor.as_deref(),
        chain::dispatcher(backend, &answerer),
        run_mode,
    );
    let chain = chain::snapshot(backend, &answerer);
    let written = store::write_in(backend, &answerer, &Area::Aimed(target.id.clone()), |aid| {
        Ok(DecisionBody {
            v: FORMAT_VERSION,
            id: aid.to_string(),
            session: stamp.session.clone(),
            time: crate::engine::types::now_iso8601(),
            state: stamp.state.clone(),
            log_seq: stamp.log_seq,
            kind: "answer".to_string(),
            outcome: "answered".to_string(),
            subject,
            decider: decider.clone(),
            authority,
            chain,
            basis,
            evidence,
            reason: req.reason.clone(),
            tool_call: req.tool_call.clone(),
            target: Some(target.clone()),
            standing: None,
            path: None,
            compensating: None,
            replacement: None,
            refusal: None,
        })
    })
    .unwrap_or_else(|e| refuse(COMMAND, &e));
    ring_asker(backend, name);

    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "v": FORMAT_VERSION,
            "session": answerer,
            "id": written.id,
            "status": "in-effect",
            "question": crate::decisions::questions::identity(&question),
            "decision": written,
        }))?
    );
    Ok(())
}

/// Ring the wake file of `asker`, a question's raiser, after an answer to
/// it was recorded, and of the newest session continuing it when it was
/// superseded (`crate::engine::wake_signal::ring_forwarded`). A wake
/// carries no content: the asker's next tick reads the store. Ringing
/// never fails the answer, which is already written: any refusal (no
/// koto root, an unwritable wakes directory, a symlink, a directory or a
/// FIFO at the path) is one warning on stderr, and the path is left as it
/// was.
fn ring_asker(backend: &Backend, asker: &str) {
    use crate::engine::wake_signal::ring;
    let Some(root) = crate::koto_home::koto_home() else {
        eprintln!("warning: no wake delivered to {asker:?}: the koto root can't be resolved");
        return;
    };
    // The succession is read from the sessions directory this command
    // used, wherever it sits, so a moved sessions base still reaches the
    // newest session continuing the asker.
    let newest = crate::engine::supersede_chain::newest_member(backend.local_base_dir(), asker);
    for session in std::iter::once(asker.to_string()).chain(newest) {
        if let Err(e) = ring(&root, &session) {
            eprintln!("warning: could not deliver a wake to {session:?}: {e}");
        }
    }
}

/// The flags `readdress` takes beyond the question's session and id.
#[derive(Args, Debug, Clone, Default)]
pub struct ReaddressArgs {
    /// The session the pass-up is recorded in: the question's raiser or
    /// the session or anchor it waits on now
    #[arg(long = "in", value_name = "SESSION", required = true)]
    pub in_session: String,
    /// The new recipient: session:<name>, anchor:<name> or person:<handle>
    #[arg(long, required = true)]
    pub to: String,
    /// Who passed it up: session. A person's pass-up can't be recorded yet
    #[arg(long)]
    pub by: Option<String>,
    /// The agent within the recording session that passed it up
    #[arg(long)]
    pub agent: Option<String>,
    /// The run mode: attended, unattended or unknown
    #[arg(long = "run-mode")]
    pub run_mode: Option<String>,
    /// An earlier decision this one relies on, as <session>:<id> (repeatable)
    #[arg(long)]
    pub basis: Vec<String>,
    /// An evidence pointer (repeatable); event:<session>:<seq> names a koto
    /// session event
    #[arg(long)]
    pub evidence: Vec<String>,
    /// Why
    #[arg(long)]
    pub reason: Option<String>,
    /// The harness's identifier for the tool call that recorded it
    #[arg(long = "tool-call")]
    pub tool_call: Option<String>,
}

/// `koto decisions readdress <session> <id> --in <session> --to <recipient>`:
/// record a `readdress` that passes the question `id` raised in `name` to a
/// new typed recipient, in the recording session's
/// `koto/aimed/<raiser's session id>/`. The recording session is the raiser
/// or the session or anchor the question waits on now. The question keeps
/// its identity; every check runs before anything is written, and a refusal
/// writes nothing anywhere.
pub fn handle_readdress(
    backend: &Backend,
    name: &str,
    id: &str,
    args: ReaddressArgs,
) -> Result<()> {
    use crate::decisions::body::Recipient;
    use crate::decisions::questions;
    const COMMAND: &str = "decisions readdress";
    if !crate::decisions::is_decision_id(id) {
        refuse(
            COMMAND,
            &DecisionError::invalid(
                "id",
                format!(
                    "'{id}' isn't a decision id: {} digits",
                    crate::decisions::ID_WIDTH
                ),
            ),
        );
    }
    if crate::discover::validate_workflow_name(name).is_err() {
        refuse(
            COMMAND,
            &DecisionError::invalid("session", "the question's session isn't a session name"),
        );
    }
    let recorder = args.in_session.clone();
    let person_refusal = || {
        DecisionError::invalid(
            "by",
            "a person's pass-up can't be recorded yet: koto doesn't verify that the person \
             made it",
        )
    };
    if args.by.as_deref().is_some_and(|b| b.starts_with("person:")) {
        refuse(COMMAND, &person_refusal());
    }
    let to = Recipient::parse(&args.to);
    if matches!(to, Recipient::Untyped(_)) {
        let has_kind = args
            .to
            .split_once(':')
            .is_some_and(|(k, _)| body::RECIPIENT_KINDS.contains(&k));
        refuse(
            COMMAND,
            &DecisionError::invalid(
                "to",
                if has_kind {
                    "--to needs a session name or a handle after its prefix".to_string()
                } else {
                    "--to takes a typed recipient: session:<name>, anchor:<name> or \
                     person:<handle>"
                        .to_string()
                },
            ),
        );
    }
    let req = Request {
        kind: "readdress".to_string(),
        outcome: "passed".to_string(),
        subject: vec![
            ("session".to_string(), name.to_string()),
            ("id".to_string(), id.to_string()),
            ("recipient".to_string(), args.to.clone()),
        ],
        basis: args.basis.clone(),
        evidence: args.evidence.clone(),
        reason: args.reason.clone(),
        tool_call: args.tool_call.clone(),
    };
    let subject = body::validate(&req).unwrap_or_else(|e| refuse(COMMAND, &e));
    let basis = body::parse_basis(&req.basis).unwrap_or_else(|e| refuse(COMMAND, &e));
    let evidence = body::parse_evidence(&req.evidence).unwrap_or_else(|e| refuse(COMMAND, &e));
    require_session(backend, &recorder, COMMAND);

    let flags = Flags {
        by: args.by.clone(),
        surface: None,
        link: None,
        agent: args.agent.clone(),
        run_mode: args.run_mode.clone(),
    };
    let observed = Observed::from_env(backend);
    let decider =
        decider::resolve(&recorder, &flags, &observed).unwrap_or_else(|e| refuse(COMMAND, &e));
    if !matches!(decider, decider::Decider::Session { .. }) {
        refuse(COMMAND, &person_refusal());
    }
    let run_mode = decider::run_mode(&flags, &observed).unwrap_or_else(|e| refuse(COMMAND, &e));

    // The question, read from the raiser's own copy with no lock and no
    // pull: nothing in the raiser's session is touched.
    let entries = crate::decisions::read::read_session_with(backend, name, false);
    let found = entries.iter().find(|e| match e {
        Entry::Decision(b) => b.id == id,
        Entry::Unreadable { id: Some(i), .. } => i == id,
        Entry::Unreadable { id: None, .. } => false,
    });
    let question = match found {
        None => refuse(
            COMMAND,
            &DecisionError::new(
                codes::DECISION_NOT_FOUND,
                format!("'{name}' holds no decision {id}"),
            ),
        ),
        Some(Entry::Unreadable { reason, .. }) => refuse(
            COMMAND,
            &DecisionError::invalid(
                "id",
                format!("decision {id} in '{name}' can't be read: {reason}"),
            ),
        ),
        Some(Entry::Decision(b)) => (**b).clone(),
    };
    if question.kind != "escalate" {
        refuse(
            COMMAND,
            &DecisionError::invalid(
                "id",
                format!(
                    "decision {id} in '{name}' is a '{}', and only an 'escalate' is passed up",
                    question.kind
                ),
            ),
        );
    }
    if !questions::is_question(&question) {
        refuse(
            COMMAND,
            &DecisionError::invalid(
                "id",
                format!(
                    "decision {id} in '{name}' has no typed recipient (session:, anchor: or \
                     person:), so it isn't a question koto passes up"
                ),
            ),
        );
    }
    let index = Index::scan(backend, false);
    let status = index.status(&question);
    if status != "in-effect" {
        refuse(
            COMMAND,
            &DecisionError::invalid(
                "id",
                format!(
                    "decision {id} in '{name}' reads '{status}', and only a question in effect \
                     is passed up"
                ),
            ),
        );
    }
    if !index.answers_of(&question.session.id, id).is_empty() {
        refuse(
            COMMAND,
            &DecisionError::invalid(
                "id",
                format!("decision {id} in '{name}' is answered, and an answered question isn't passed up"),
            ),
        );
    }
    let runs = questions::Runs::new(backend);
    let history = questions::addressees(&index, &runs, &question);
    let current = history
        .last()
        .cloned()
        .expect("a question has a typed recipient");
    let stamp = standing::stamp(backend, &recorder).unwrap_or_else(|e| refuse(COMMAND, &e));
    if !questions::may_pass(&runs, &question, &current, &recorder, &stamp.session.id) {
        refuse(
            COMMAND,
            &DecisionError::invalid(
                "in",
                format!(
                    "a pass-up is recorded by the question's raiser or by the session or anchor \
                     it waits on now; '{recorder}' is neither ({})",
                    questions::history_labels(&index, &runs, &question).join(" -> ")
                ),
            ),
        );
    }
    if to == current {
        refuse(
            COMMAND,
            &DecisionError::invalid(
                "to",
                format!("decision {id} in '{name}' already waits on '{}'", args.to),
            ),
        );
    }
    let target = Target {
        name: name.to_string(),
        id: question.session.id.clone(),
        decision: id.to_string(),
    };

    standing::settle(backend, &recorder);
    let stamp = standing::stamp(backend, &recorder).unwrap_or_else(|e| refuse(COMMAND, &e));
    let authority = decider::authority(
        backend,
        &decider,
        observed.anchor.as_deref(),
        chain::dispatcher(backend, &recorder),
        run_mode,
    );
    let chain = chain::snapshot(backend, &recorder);
    let written = store::write_in(backend, &recorder, &Area::Aimed(target.id.clone()), |aid| {
        Ok(DecisionBody {
            v: FORMAT_VERSION,
            id: aid.to_string(),
            session: stamp.session.clone(),
            time: crate::engine::types::now_iso8601(),
            state: stamp.state.clone(),
            log_seq: stamp.log_seq,
            kind: "readdress".to_string(),
            outcome: "passed".to_string(),
            subject,
            decider: decider.clone(),
            authority,
            chain,
            basis,
            evidence,
            reason: req.reason.clone(),
            tool_call: req.tool_call.clone(),
            target: Some(target.clone()),
            standing: None,
            path: None,
            compensating: None,
            replacement: None,
            refusal: None,
        })
    })
    .unwrap_or_else(|e| refuse(COMMAND, &e));

    let mut addressees: Vec<serde_json::Value> = history.iter().map(Recipient::to_json).collect();
    addressees.push(to.to_json());
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "v": FORMAT_VERSION,
            "session": recorder,
            "id": written.id,
            "status": "in-effect",
            "question": questions::identity(&question),
            "addressee": to.to_json(),
            "addressees": addressees,
            "decision": written,
        }))?
    );
    Ok(())
}

/// `koto decisions open --at <session-or-anchor>`: the questions open at a
/// position, across every session in the local store.
pub fn handle_open_at(backend: &Backend, at: &str) -> Result<()> {
    const COMMAND: &str = "decisions open";
    if crate::discover::validate_workflow_name(at).is_err() {
        refuse(
            COMMAND,
            &DecisionError::invalid("at", "--at takes a session or anchor name"),
        );
    }
    if !chain::session_present(backend, at) {
        refuse(
            COMMAND,
            &DecisionError::invalid(
                "at",
                format!("no session or anchor named '{at}' in the local store"),
            ),
        );
    }
    println!(
        "{}",
        serde_json::to_string(&crate::decisions::questions::open_at(backend, at))?
    );
    Ok(())
}

/// `koto decisions open --person <handle>`: the questions open for a
/// person, across every session in the local store. Takes no session.
pub fn handle_open(backend: &Backend, person: &str) -> Result<()> {
    const COMMAND: &str = "decisions open";
    if decider::check_handle("person", person).is_err() {
        refuse(
            COMMAND,
            &DecisionError::invalid(
                "person",
                format!(
                    "--person takes a handle of 1 to {} bytes with no control characters",
                    decider::MAX_HANDLE_BYTES
                ),
            ),
        );
    }
    println!(
        "{}",
        serde_json::to_string(&crate::decisions::questions::open(backend, person))?
    );
    Ok(())
}
