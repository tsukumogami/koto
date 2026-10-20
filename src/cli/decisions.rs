//! `koto decisions` against the host-level decision store: `record
//! --kind`, `show`, the host-wide `list` filters and `report`.
//!
//! Today's per-workflow forms (`record <name> --with-data`, `list <name>`)
//! stay in `mod.rs` and keep their arguments and output. Every response
//! here is one JSON object on stdout, errors included.

use anyhow::Result;
use clap::Args;

use crate::decision_store::reader::{self, Filter};
use crate::decision_store::record::{
    Authority, Chain, DecidedBy, DeciderKind, Decision, Kind, Subject, DECISION_FORMAT_VERSION,
};
use crate::decision_store::{
    new_id, normalize_time, now_to_second, report, validate_link, Check, FieldError, Store,
    StoreError, WriteOutcome,
};

use super::exit_with_error_code;

/// Flags for `koto decisions record --kind`.
#[derive(Args, Debug, Default, Clone)]
pub struct RecordKindArgs {
    /// Record a decision of this kind in the host-level decision store
    /// (accept, merge, gate-pass, escalate, decline-escalation, judgment,
    /// verdict)
    #[arg(long)]
    pub kind: Option<String>,
    /// The decision's outcome; may be left out when the kind has only one
    #[arg(long)]
    pub outcome: Option<String>,
    /// What an accept is of (document, plan, step, test-change, intent),
    /// or what a verdict is on (change, release)
    #[arg(long)]
    pub of: Option<String>,
    /// Repository the subject lives in
    #[arg(long)]
    pub repo: Option<String>,
    /// Path of the subject in its repository (repeatable for a test change)
    #[arg(long = "path")]
    pub paths: Vec<String>,
    /// Commit the decision is about
    #[arg(long)]
    pub commit: Option<String>,
    /// Plan path, for the acceptance of a plan step
    #[arg(long)]
    pub plan: Option<String>,
    /// Step identifier, for the acceptance of a plan step
    #[arg(long)]
    pub step: Option<String>,
    /// Where an accepted intent lives (a request id, a repository path or a
    /// forge issue)
    #[arg(long = "intent-location")]
    pub intent_location: Option<String>,
    /// The ticket or alert an accepted intent was drafted from (https URL)
    #[arg(long = "source-link")]
    pub source_link: Option<String>,
    /// Pull request number, for a merge
    #[arg(long)]
    pub pr: Option<u64>,
    /// Workflow (koto session name), for a gate pass
    #[arg(long)]
    pub workflow: Option<String>,
    /// State, for a gate pass
    #[arg(long)]
    pub state: Option<String>,
    /// Gate, for a gate pass
    #[arg(long)]
    pub gate: Option<String>,
    /// Question identifier, for an escalation or a decline
    #[arg(long = "question-id")]
    pub question_id: Option<String>,
    /// Question text, for an escalation or a decline
    #[arg(long)]
    pub question: Option<String>,
    /// Who an escalation goes to (a session or a person)
    #[arg(long)]
    pub recipient: Option<String>,
    /// What a judgment or verdict judged, as free text
    #[arg(long)]
    pub what: Option<String>,
    /// A pointer to what a judgment or verdict judged
    #[arg(long)]
    pub pointer: Option<String>,
    /// An earlier decision this one relies on (repeatable)
    #[arg(long = "basis")]
    pub basis: Vec<String>,
    /// A pointer to evidence, such as a harness tool-call id or a run-journal
    /// record (repeatable); stored as given, never followed
    #[arg(long = "evidence")]
    pub evidence: Vec<String>,
    /// Why, as free text; don't put secrets here, the record is permanent
    #[arg(long)]
    pub reason: Option<String>,
    #[command(flatten)]
    pub decider: DeciderArgs,
}

/// The decider and handed-lineage flags every recording command shares.
/// All are optional: with none, the decider is the observed harness
/// session, marked defaulted.
#[derive(Args, Debug, Default, Clone)]
pub struct DeciderArgs {
    /// Who decided: session (the observed harness session), session:<id>,
    /// person:<handle> or check:<name>
    #[arg(long)]
    pub by: Option<String>,
    /// For a person: the surface the decision came through (terminal, forge,
    /// relay)
    #[arg(long)]
    pub surface: Option<String>,
    /// For a person: a link to the original, such as a forge review (https
    /// URL, no credentials or query string); stored, never fetched
    #[arg(long)]
    pub link: Option<String>,
    /// The agent within the session that decided (main or an agent id), when
    /// the koto-skills hook didn't set KOTO_AGENT_ID
    #[arg(long)]
    pub agent: Option<String>,
    /// The run mode (attended, unattended), when the harness doesn't report it
    #[arg(long = "run-mode")]
    pub run_mode: Option<String>,
    /// Who dispatched the deciding session: session:<id>, person:<handle> or
    /// none (a root)
    #[arg(long = "dispatched-by")]
    pub dispatched_by: Option<String>,
    /// A file holding the dispatch chain above the dispatcher, in koto's
    /// versioned lineage format
    #[arg(long = "chain-file")]
    pub chain_file: Option<std::path::PathBuf>,
    /// The digest of the instructions the deciding session was dispatched
    /// with (sha256:<hex>)
    #[arg(long = "instructions-digest")]
    pub instructions_digest: Option<String>,
    /// The dispatch identifier the deciding session was launched under
    #[arg(long = "dispatch-id")]
    pub dispatch_id: Option<String>,
}

impl DeciderArgs {
    /// Whether any decider or handed-lineage flag was given.
    pub fn any_set(&self) -> bool {
        let Self {
            by,
            surface,
            link,
            agent,
            run_mode,
            dispatched_by,
            chain_file,
            instructions_digest,
            dispatch_id,
        } = self;
        [
            by,
            surface,
            link,
            agent,
            run_mode,
            dispatched_by,
            instructions_digest,
            dispatch_id,
        ]
        .iter()
        .any(|o| o.is_some())
            || chain_file.is_some()
    }
}

/// Why the decider and lineage didn't resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityError {
    /// No decider was named and no harness session was observed.
    DeciderMissing,
    /// A flag is malformed, naming the field at fault.
    Invalid(FieldError),
}

/// Resolve who decided, under what authority and through which chain, from
/// the shared flags and what koto observes. Every file read (the chain file,
/// the workspace manager's session map, the run journal) happens here,
/// before any store lock is taken.
pub fn resolve_identity(
    args: &DeciderArgs,
    journal_root: Option<std::path::PathBuf>,
) -> Result<(DecidedBy, Chain, Authority), IdentityError> {
    use crate::decision_store::decider::{self, DeciderError, DeciderInput, Observed};
    use crate::decision_store::lineage::{self, HandedLineage, Sources};
    let invalid = IdentityError::Invalid;

    if let Some(l) = &args.link {
        validate_link("link", l).map_err(invalid)?;
    }
    if let Some(m) = &args.run_mode {
        decider::validate_run_mode(m).map_err(invalid)?;
    }
    if let Some(v) = &args.dispatched_by {
        lineage::validate_dispatched_by(v).map_err(invalid)?;
    }
    if let Some(v) = &args.dispatch_id {
        lineage::validate_dispatch_id(v).map_err(invalid)?;
    }
    if let Some(v) = &args.instructions_digest {
        lineage::validate_digest(v).map_err(invalid)?;
    }
    let by = match &args.by {
        Some(b) => Some(decider::parse_by(b).map_err(invalid)?),
        None => None,
    };
    let chain_file = match &args.chain_file {
        Some(p) => Some(lineage::read_chain_file(p).map_err(invalid)?),
        None => None,
    };

    let observed = Observed::from_host();
    let input = DeciderInput {
        by,
        surface: args.surface.clone(),
        link: args.link.clone(),
        agent: args.agent.clone(),
    };
    let decided_by = decider::resolve_decider(&input, &observed).map_err(|e| match e {
        DeciderError::Missing => IdentityError::DeciderMissing,
        DeciderError::Invalid(fe) => IdentityError::Invalid(fe),
    })?;
    let run_mode = decider::run_mode_slot(&observed, args.run_mode.as_deref());
    let handed = HandedLineage {
        dispatched_by: args.dispatched_by.clone(),
        chain_file,
        dispatch_id: args.dispatch_id.clone(),
        instructions_digest: args.instructions_digest.clone(),
    };
    let resolved = lineage::resolve_lineage(
        decided_by.kind,
        observed.session.as_deref(),
        run_mode,
        &handed,
        &Sources::discover(journal_root),
    );
    Ok((decided_by, resolved.chain, resolved.authority))
}

/// The run journal's home for the store this process would use: the
/// `KOTO_SESSIONS_BASE` store journals inside its base, the user's store in
/// `~/.koto`. Built without constructing a backend, so resolving lineage
/// never touches a cloud configuration.
pub fn journal_root() -> Option<std::path::PathBuf> {
    match std::env::var_os("KOTO_SESSIONS_BASE") {
        Some(base) => Some(std::path::PathBuf::from(base)),
        None => dirs::home_dir().map(|h| h.join(".koto")),
    }
}

impl RecordKindArgs {
    /// Whether any flag of the `--kind` form was given.
    pub fn any_set(&self) -> bool {
        let Self {
            kind,
            outcome,
            of,
            repo,
            paths,
            commit,
            plan,
            step,
            intent_location,
            source_link,
            pr,
            workflow,
            state,
            gate,
            question_id,
            question,
            recipient,
            what,
            pointer,
            basis,
            evidence,
            reason,
            decider,
        } = self;
        [
            kind,
            outcome,
            of,
            repo,
            commit,
            plan,
            step,
            intent_location,
            source_link,
            workflow,
            state,
            gate,
            question_id,
            question,
            recipient,
            what,
            pointer,
            reason,
        ]
        .iter()
        .any(|o| o.is_some())
            || decider.any_set()
            || pr.is_some()
            || !paths.is_empty()
            || !basis.is_empty()
            || !evidence.is_empty()
    }
}

/// Host-wide filters for `koto decisions list`.
#[derive(Args, Debug, Default, Clone)]
pub struct ListFilterArgs {
    /// Only decisions of this kind
    #[arg(long)]
    pub kind: Option<String>,
    /// Only decisions whose decider is of this kind (session, person, check)
    #[arg(long = "decider-kind")]
    pub decider_kind: Option<String>,
    /// Only decisions whose subject field equals a value, as FIELD=VALUE
    /// (repeatable)
    #[arg(long = "subject")]
    pub subject: Vec<String>,
    /// Window start, inclusive (UTC: YYYY-MM, YYYY-MM-DD or
    /// YYYY-MM-DDTHH:MM:SSZ)
    #[arg(long)]
    pub since: Option<String>,
    /// Window end, exclusive (same forms as --since)
    #[arg(long)]
    pub until: Option<String>,
    /// Only decisions whose decider was defaulted rather than named
    #[arg(long)]
    pub defaulted: bool,
    /// Only decisions made by this session
    #[arg(long)]
    pub session: Option<String>,
    /// Only decisions made in this run mode (attended, unattended, unknown)
    #[arg(long = "run-mode")]
    pub run_mode: Option<String>,
    /// Only decisions made under this dispatch identifier
    #[arg(long = "dispatch-id")]
    pub dispatch_id: Option<String>,
    /// Only decisions made under this instructions digest
    #[arg(long = "instructions-digest")]
    pub instructions_digest: Option<String>,
}

impl ListFilterArgs {
    pub fn any_set(&self) -> bool {
        self.kind.is_some()
            || self.decider_kind.is_some()
            || !self.subject.is_empty()
            || self.since.is_some()
            || self.until.is_some()
            || self.defaulted
            || self.session.is_some()
            || self.run_mode.is_some()
            || self.dispatch_id.is_some()
            || self.instructions_digest.is_some()
    }
}

fn fail(command: &str, field: Option<&str>, message: impl Into<String>, code: i32) -> ! {
    let mut body = serde_json::json!({
        "error": message.into(),
        "command": command,
    });
    if let Some(f) = field {
        body["field"] = serde_json::Value::String(f.to_string());
    }
    exit_with_error_code(body, code)
}

/// Refuse a decision nobody can be named for: exit 2, field `by`, reason
/// `decider_missing`, nothing written.
pub fn fail_decider_missing(command: &str) -> ! {
    exit_with_error_code(
        serde_json::json!({
            "error": crate::decision_store::decider::DeciderError::Missing.to_string(),
            "command": command,
            "field": "by",
            "reason": crate::decision_store::decider::DeciderError::MISSING_REASON,
        }),
        2,
    )
}

fn fail_field(command: &str, e: FieldError) -> ! {
    fail(command, Some(&e.field), e.message, 2)
}

fn fail_store(command: &str, e: StoreError) -> ! {
    match e {
        StoreError::Invalid(fe) => fail_field(command, fe),
        StoreError::IdConflict { .. } => fail(command, Some("id"), e.to_string(), 2),
        other => fail(command, None, other.to_string(), 1),
    }
}

fn open_store(command: &str) -> Store {
    match Store::open_default() {
        Ok(s) => s,
        Err(e) => fail_store(command, e),
    }
}

/// Build the subject from the flags.
fn subject_from(args: &RecordKindArgs, kind: Kind) -> Result<Subject, FieldError> {
    let mut subject = Subject {
        of: args.of.clone(),
        repo: args.repo.clone(),
        commit: args.commit.clone(),
        plan: args.plan.clone(),
        step: args.step.clone(),
        intent_location: args.intent_location.clone(),
        source_link: args.source_link.clone(),
        pr: args.pr,
        workflow: args.workflow.clone(),
        state: args.state.clone(),
        gate: args.gate.clone(),
        question_id: args.question_id.clone(),
        question: args.question.clone(),
        recipient: args.recipient.clone(),
        what: args.what.clone(),
        pointer: args.pointer.clone(),
        ..Default::default()
    };
    let test_change = kind == Kind::Accept && args.of.as_deref() == Some("test-change");
    if test_change {
        subject.paths = args.paths.clone();
    } else {
        match args.paths.as_slice() {
            [] => {}
            [one] => subject.path = Some(one.clone()),
            _ => {
                return Err(FieldError::new(
                    "path",
                    "only an accept of a test change takes more than one --path",
                ))
            }
        }
    }
    Ok(subject)
}

/// `koto decisions record --kind ...`.
pub fn handle_record_kind(args: RecordKindArgs) -> Result<()> {
    const CMD: &str = "decisions record";
    let kind_name = args.kind.clone().unwrap_or_default();
    let Some(kind) = Kind::parse(&kind_name) else {
        fail(
            CMD,
            Some("kind"),
            format!(
                "'{}' is not a decision kind (allowed: accept, merge, gate-pass, escalate, \
                 decline-escalation, judgment, verdict)",
                kind_name
            ),
            2,
        );
    };
    if matches!(kind, Kind::Reverse | Kind::Refused) {
        fail(
            CMD,
            Some("kind"),
            format!("kind '{}' can't be recorded directly", kind),
            2,
        );
    }
    let outcome = match (&args.outcome, kind.outcomes()) {
        (Some(o), _) => o.clone(),
        (None, [only]) => only.to_string(),
        (None, many) => fail(
            CMD,
            Some("outcome"),
            format!(
                "kind '{}' needs --outcome (one of: {})",
                kind,
                many.join(", ")
            ),
            2,
        ),
    };
    let subject = subject_from(&args, kind).unwrap_or_else(|e| fail_field(CMD, e));
    if let Some(l) = args.source_link.as_deref() {
        if let Err(e) = validate_link("source_link", l) {
            fail_field(CMD, e);
        }
    }

    let (decided_by, chain, authority) = match resolve_identity(&args.decider, journal_root()) {
        Ok(r) => r,
        Err(IdentityError::Invalid(e)) => fail_field(CMD, e),
        Err(IdentityError::DeciderMissing) => fail_decider_missing(CMD),
    };

    let time = now_to_second();
    let id = new_id(&time).unwrap_or_else(|e| fail_field(CMD, e));
    let decision = Decision {
        v: DECISION_FORMAT_VERSION,
        id,
        time,
        kind,
        outcome,
        subject,
        decided_by,
        authority,
        chain,
        basis: args.basis.clone(),
        evidence: args.evidence.clone(),
        reason: args.reason.clone(),
        reverses: None,
        original: None,
        repaired_from: None,
        import_source: None,
        recorded_at: None,
    };

    let store = open_store(CMD);
    let outcome = store
        .append(decision, |_| Ok(Check::Append))
        .unwrap_or_else(|e| fail_store(CMD, e));
    let (id, written) = match outcome {
        WriteOutcome::Appended { id } => (id, true),
        WriteOutcome::Skipped { id } => (id, false),
        WriteOutcome::Refused { .. } => fail(CMD, None, "the decision was refused", 1),
    };
    let stored = reader::show(store.root(), &id)
        .unwrap_or_else(|e| fail_store(CMD, e))
        .map(|s| s.listed.decision);
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "id": id,
            "written": written,
            "decision": stored,
        }))?
    );
    Ok(())
}

/// `koto decisions show <id>`.
pub fn handle_show(id: String) -> Result<()> {
    const CMD: &str = "decisions show";
    let store = open_store(CMD);
    match reader::show(store.root(), &id).unwrap_or_else(|e| fail_store(CMD, e)) {
        Some(shown) => {
            println!("{}", serde_json::to_string(&shown)?);
            Ok(())
        }
        None => fail(
            CMD,
            Some("id"),
            format!("decision '{}' is not on the record", id),
            1,
        ),
    }
}

fn window_bound(command: &str, field: &str, raw: &str) -> String {
    normalize_time(raw).unwrap_or_else(|| {
        fail(
            command,
            Some(field),
            format!(
                "'{}' is not a UTC time (use YYYY-MM, YYYY-MM-DD or YYYY-MM-DDTHH:MM:SSZ)",
                raw
            ),
            2,
        )
    })
}

/// Turn the list flags into a filter, refusing a malformed one.
pub fn filter_from(args: &ListFilterArgs) -> Result<Filter, FieldError> {
    let kind =
        match &args.kind {
            None => None,
            Some(k) => Some(Kind::parse(k).ok_or_else(|| {
                FieldError::new("kind", format!("'{}' is not a decision kind", k))
            })?),
        };
    let decider_kind = match &args.decider_kind {
        None => None,
        Some(k) => Some(DeciderKind::parse(k).ok_or_else(|| {
            FieldError::new(
                "decider_kind",
                format!("'{}' is not a decider kind (session, person, check)", k),
            )
        })?),
    };
    let mut subject = Vec::new();
    for pair in &args.subject {
        let Some((field, value)) = pair.split_once('=') else {
            return Err(FieldError::new(
                "subject",
                format!("'{}' is not FIELD=VALUE", pair),
            ));
        };
        subject.push((field.replace('-', "_"), value.to_string()));
    }
    let bound = |field: &str, raw: &Option<String>| -> Result<Option<String>, FieldError> {
        match raw {
            None => Ok(None),
            Some(r) => normalize_time(r)
                .map(Some)
                .ok_or_else(|| FieldError::new(field, format!("'{}' is not a UTC time", r))),
        }
    };
    Ok(Filter {
        kind,
        decider_kind,
        subject,
        since: bound("since", &args.since)?,
        until: bound("until", &args.until)?,
        defaulted: args.defaulted.then_some(true),
        session: args.session.clone(),
        run_mode: args.run_mode.clone(),
        dispatch_id: args.dispatch_id.clone(),
        instructions_digest: args.instructions_digest.clone(),
    })
}

/// `koto decisions list` with no workflow name: the host-wide listing.
pub fn handle_list_host(args: ListFilterArgs) -> Result<()> {
    const CMD: &str = "decisions list";
    let filter = filter_from(&args).unwrap_or_else(|e| fail_field(CMD, e));
    let store = open_store(CMD);
    let listing = reader::list(store.root(), &filter).unwrap_or_else(|e| fail_store(CMD, e));
    println!("{}", serde_json::to_string(&listing)?);
    Ok(())
}

/// `koto decisions report --since <t> --until <t>`.
pub fn handle_report(since: String, until: String) -> Result<()> {
    const CMD: &str = "decisions report";
    let since = window_bound(CMD, "since", &since);
    let until = window_bound(CMD, "until", &until);
    if since >= until {
        fail(CMD, Some("until"), "--until must be later than --since", 2);
    }
    let store = open_store(CMD);
    let r = report::report(store.root(), &since, &until).unwrap_or_else(|e| fail_store(CMD, e));
    println!("{}", serde_json::to_string(&r)?);
    Ok(())
}
