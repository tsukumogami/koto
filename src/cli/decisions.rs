//! `koto decisions record --kind`, `list --format v1` and `show`
//! (DESIGN-decision-record.md, Decision 4).
//!
//! Every form acts on the session named in its arguments and never reads
//! the session from the environment (PRD R46). The forms that existed
//! before, `record <name> --with-data` and `list <name>` with none of the
//! new flags, keep their behaviour and live in `cli/mod.rs`.

use anyhow::Result;
use clap::Args;

use crate::decisions::body::{self, DecisionBody, Request, SessionRef};
use crate::decisions::decider::{self, Flags, Observed};
use crate::decisions::read::{self, Filters};
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
        })
    })
    .unwrap_or_else(|e| refuse(COMMAND, &e));

    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "v": FORMAT_VERSION,
            "session": name,
            "id": written.id,
            "status": read::status_of(&written),
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
    };
    println!(
        "{}",
        serde_json::to_string(&read::list(backend, name, args.below, &filters))?
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
    match read::show(backend, name, id) {
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

/// Whether the bucket holds `name` though this host has no copy: a read
/// answers from the bucket's objects then.
fn backend_holds_remote(backend: &Backend, name: &str) -> bool {
    match backend {
        Backend::Cloud(c) => c.remote_header(name).ok().flatten().is_some(),
        Backend::Local(_) => false,
    }
}
