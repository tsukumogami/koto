//! The run-time half of decider checks (DESIGN-koto-decider-checks.md,
//! Decisions 3 to 6).
//!
//! [`CliCheckEvaluator`] evaluates one `decider-check` gate: it runs the
//! extraction command, bounds the redacted slice, takes the session's
//! `decider.lock`, waiting for another tick to release it for up to the
//! per-call consultation cap times two attempts times the provider timeout,
//! plus a second, and never more than 60 s ([`lock_wait_for`]), before its
//! criteria go unanswered with `busy`, reads the local log for the visit and
//! for verdicts it can reuse, consults the provider once per criterion (one
//! retry for a transient failure) under the per-call cap it shares with the
//! routing decider, appends a ledger `checked` line per consultation, and
//! returns a [`StructuredGateResult`] carrying the blocking findings and the
//! records the advance loop appends as `decider_checked` events.
//!
//! A criterion that gets no verdict falls back to the agent
//! (DESIGN-decider-fallback.md): unless an answer the agent recorded for the
//! same slice applies, it is listed under `output.awaiting`, the gate is
//! `pending` with a warning finding that hands the agent the question, and a
//! `decider_fallback` record goes beside the `decider_checked` ones.
//!
//! The pure rules (the verdict, blocking, retry, the record) live in
//! `crate::decider::check`; this module is the I/O around them.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::action::{run_shell_command, CommandEnv};
use crate::cli::decider_port::{input_sha256, try_decider_lock, LockHold};
use crate::decider::check::{
    blocks, build_check_request, effective_check_mode, is_retryable, reason_for_error,
    recorded_probabilities, verdict, AgentAnswer, CheckOutcome, DeciderCheck, DeciderFallback,
    UnansweredReason,
};
use crate::decider::ledger::{append_or_warn, LedgerRecord};
use crate::decider::types::{
    Decider, DecisionRequest, ErrorClass, GlobalMode, SettingOrigin, UsageTally,
};
use crate::engine::decider::MAX_CONSULTATIONS_PER_CALL;
use crate::engine::persistence::{arrival_index, derive_state_from_log};
use crate::engine::types::{Event, EventPayload};
use crate::findings::{Finding, FindingLevel, GateFailure, MessageSource};
use crate::gate::{GateOutcome, StructuredGateResult};
use crate::session::SessionBackend;
use crate::template::decider_check::{declaration_hash, CheckMode, DeciderCheckSpec};
use crate::template::types::{Gate, GATE_TYPE_DECIDER_CHECK};

/// Model recorded when no answer named one.
const UNKNOWN_MODEL: &str = "unknown";

/// Most provider attempts one consultation makes: the first, and one retry.
const MAX_ATTEMPTS: u32 = 2;

/// The per-`koto next` consultation count, shared by the routing decider's
/// port and the check evaluator so the two together stay under
/// [`MAX_CONSULTATIONS_PER_CALL`]. It is authoritative: the engine's own
/// counter for the routing decider can lag it, and the port answers
/// `Skipped` once it is spent.
#[derive(Debug, Clone, Default)]
pub struct ConsultBudget(Rc<Cell<usize>>);

impl ConsultBudget {
    pub fn new() -> Self {
        Self::default()
    }

    /// Take one slot for a request about to be sent. `false` when the cap
    /// is already spent. A retry, a reuse and an outcome that sent nothing
    /// never take one.
    pub fn try_take(&self) -> bool {
        let used = self.0.get();
        if used >= MAX_CONSULTATIONS_PER_CALL {
            return false;
        }
        self.0.set(used + 1);
        true
    }

    /// Slots taken so far this call.
    pub fn used(&self) -> usize {
        self.0.get()
    }
}

/// The declaration hash of each criterion of every decider check in
/// `gates`, keyed by gate name, computed from the gate as the template wrote
/// it, before its command is substituted, so a changing variable in the
/// command neither changes the hash nor defeats reuse.
pub fn declaration_hashes(gates: &BTreeMap<String, Gate>) -> BTreeMap<String, Vec<String>> {
    gates
        .iter()
        .filter(|(_, g)| g.gate_type == GATE_TYPE_DECIDER_CHECK)
        .filter_map(|(name, g)| {
            let spec = g.decider_check.as_ref()?;
            Some((
                name.clone(),
                spec.criteria
                    .iter()
                    .map(|c| declaration_hash(c, &g.command, spec.max_bytes, &spec.label))
                    .collect(),
            ))
        })
        .collect()
}

/// Whether the template declares any decider check.
pub fn declares_checks(compiled: &crate::template::types::CompiledTemplate) -> bool {
    compiled
        .states
        .values()
        .flat_map(|s| s.gates.values())
        .any(|g| g.gate_type == GATE_TYPE_DECIDER_CHECK)
}

/// The template as a user who can't consult sees it: every decider check
/// removed, so the state behaves exactly as if none were declared
/// (Decision 4). Only called when the template declares one. The template
/// hash is computed from the file, not from this view.
pub fn without_checks(
    mut compiled: crate::template::types::CompiledTemplate,
) -> crate::template::types::CompiledTemplate {
    for state in compiled.states.values_mut() {
        state
            .gates
            .retain(|_, g| g.gate_type != GATE_TYPE_DECIDER_CHECK);
    }
    compiled
}

/// The ledger's `check_overridden` records for an override of the decider
/// check `gate` in `state`: one `candidate_false_fail` per criterion the
/// check's last output (`actual_output`) lists as failed by the decider. A
/// criterion the agent answered `fail` is listed under `agent_answered` too
/// and gets none: an override of the agent's own judgment says nothing
/// about the decider. A criterion waiting on the agent is never under
/// `failed`, so it gets none either; `overridden_unanswered` survives only
/// in ledgers written before that rule. Each record takes its
/// visit and declaration hash from the latest `decider_checked` for that
/// criterion in the current visit to `state`, the visit the override applies
/// to; a reused verdict appended none, but the consultation it reused is in
/// the same visit. A criterion with no such record in this visit gets none,
/// even when an earlier visit has one: that consultation isn't what the
/// override moved past.
pub fn override_records(
    events: &[Event],
    state: &str,
    gate: &str,
    actual_output: &serde_json::Value,
    session: &str,
    session_id: Option<&str>,
) -> Vec<LedgerRecord> {
    use crate::decider::ledger::OverriddenCriterion;
    let visit = match arrival_index(events, state) {
        Some(i) => &events[i + 1..],
        None => events,
    };
    let mut out = Vec::new();
    let ids = actual_output["failed"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    // A criterion the agent failed is the agent's own judgment, not a
    // decider verdict an override could prove wrong.
    let agent_answered: Vec<&str> = actual_output["agent_answered"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    for rule_id in ids
        .iter()
        .filter_map(|v| v.as_str())
        .filter(|id| !agent_answered.contains(id))
    {
        let latest = visit.iter().rev().find_map(|e| match &e.payload {
            EventPayload::DeciderChecked(c)
                if c.state == state && c.gate == gate && c.rule_id == rule_id =>
            {
                Some((c.visit_seq, c.declaration_hash.as_str()))
            }
            _ => None,
        });
        if let Some((visit_seq, declaration_hash)) = latest {
            out.push(LedgerRecord::check_overridden(
                session,
                session_id,
                OverriddenCriterion {
                    state,
                    visit_seq,
                    gate,
                    rule_id,
                    declaration_hash,
                },
            ));
        }
    }
    out
}

/// How long a tick waits for `decider.lock` before its criteria go
/// unanswered with `busy`: the longest another tick can hold it (every
/// consultation of the per-call cap, each with its retry, at the provider
/// timeout, plus a second), capped at [`MAX_LOCK_WAIT`]. At the default
/// 2-second timeout that is 17 seconds. Waiting rather than failing fast is
/// what keeps a second, concurrent `koto next` from passing a veto check
/// the first one is still grading.
pub fn lock_wait_for(provider_timeout: Duration) -> Duration {
    let holder = provider_timeout
        .saturating_mul(MAX_CONSULTATIONS_PER_CALL as u32 * MAX_ATTEMPTS)
        .saturating_add(Duration::from_secs(1));
    holder.min(MAX_LOCK_WAIT)
}

/// The longest a tick waits for `decider.lock`, whatever the timeout.
pub const MAX_LOCK_WAIT: Duration = Duration::from_secs(60);

/// How often a waiting tick tries the lock again.
const LOCK_POLL: Duration = Duration::from_millis(50);

/// `output.error` on a check that passed because it had no spec to grade
/// against.
pub const MISSING_SPEC: &str = "missing_spec";

/// `output.error` on a check that passed because the session log couldn't
/// be read, so neither the state nor the visit is known.
pub const LOG_UNREADABLE: &str = "log_unreadable";

/// What one consultation produced before it becomes a record.
struct Consulted {
    outcome: CheckOutcome,
    probabilities: BTreeMap<String, f64>,
    model: String,
    usage: UsageTally,
    attempts: u32,
    latency_ms: u64,
    error_class: Option<ErrorClass>,
}

impl Consulted {
    /// An outcome reached without sending anything.
    fn unsent(outcome: CheckOutcome) -> Self {
        Consulted {
            outcome,
            probabilities: BTreeMap::new(),
            model: UNKNOWN_MODEL.to_string(),
            usage: UsageTally::default(),
            attempts: 0,
            latency_ms: 0,
            error_class: None,
        }
    }
}

/// One criterion's outcome this evaluation, and its records when it wasn't
/// a reuse.
struct Graded<'s> {
    criterion: &'s crate::template::decider_check::CheckCriterion,
    mode: CheckMode,
    outcome: CheckOutcome,
    /// The agent's answer, when the outcome came from one.
    agent: Option<AgentAnswer>,
    blocked: bool,
    record: Option<DeciderCheck>,
    /// Set when the criterion fell back to the agent this evaluation.
    fallback: Option<DeciderFallback>,
}

impl Graded<'_> {
    /// No verdict and no applicable answer: the criterion waits on the
    /// agent (DESIGN-decider-fallback.md, Decision 2).
    fn waiting(&self) -> Option<UnansweredReason> {
        match (self.outcome, self.agent) {
            (CheckOutcome::Unanswered(r), None) => Some(r),
            _ => None,
        }
    }
}

/// The outcome an agent's answer stands for: its `unclear` is the
/// criterion's escape.
fn agent_outcome(answer: AgentAnswer) -> CheckOutcome {
    match answer {
        AgentAnswer::Pass => CheckOutcome::Pass,
        AgentAnswer::Fail => CheckOutcome::Fail,
        AgentAnswer::Unclear => CheckOutcome::Escape,
    }
}

/// Evaluates `decider-check` gates for one `koto next`.
pub struct CliCheckEvaluator<'a> {
    backend: &'a dyn SessionBackend,
    session: &'a str,
    decider: Box<dyn Decider>,
    /// The user's effective decider mode after any project limit: veto
    /// needs `auto`.
    global: GlobalMode,
    endpoint_origin: SettingOrigin,
    budget: ConsultBudget,
    ledger_root: Option<PathBuf>,
    /// How long to wait for `decider.lock` ([`lock_wait_for`]); zero tries
    /// once.
    lock_wait: Duration,
    working_dir: &'a Path,
    env: &'a CommandEnv,
}

impl<'a> CliCheckEvaluator<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        backend: &'a dyn SessionBackend,
        session: &'a str,
        decider: Box<dyn Decider>,
        global: GlobalMode,
        endpoint_origin: SettingOrigin,
        budget: ConsultBudget,
        working_dir: &'a Path,
        env: &'a CommandEnv,
    ) -> Self {
        CliCheckEvaluator {
            backend,
            session,
            decider,
            global,
            endpoint_origin,
            budget,
            ledger_root: None,
            lock_wait: Duration::ZERO,
            working_dir,
            env,
        }
    }

    /// Where ledger lines go (`~/.koto`); `None` writes none.
    pub fn with_ledger_root(mut self, koto_root: Option<PathBuf>) -> Self {
        self.ledger_root = koto_root;
        self
    }

    /// How long to wait for `decider.lock` before a criterion is `busy`.
    pub fn with_lock_wait(mut self, wait: Duration) -> Self {
        self.lock_wait = wait;
        self
    }

    /// Take `decider.lock`, trying again until [`Self::lock_wait`] runs out.
    fn lock_waiting(&self) -> Option<LockHold> {
        let deadline = Instant::now() + self.lock_wait;
        loop {
            if let Some(hold) = self.try_lock() {
                return Some(hold);
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            std::thread::sleep(LOCK_POLL.min(deadline - now));
        }
    }

    fn try_lock(&self) -> Option<LockHold> {
        try_decider_lock(&self.backend.session_dir(self.session))
    }

    /// Evaluate the `decider-check` gate `name` (already substituted), whose
    /// criteria's declaration hashes are `hashes`, in declaration order.
    pub fn evaluate(&self, name: &str, gate: &Gate, hashes: &[String]) -> StructuredGateResult {
        // Defence in depth: loading a compiled template refuses a decider
        // check with no spec (E-DECIDER-CHECK-SPEC), so only a gate built
        // in memory reaches this. With nothing to grade against, the check
        // passes and says why; it never blocks.
        let Some(spec) = gate.decider_check.as_ref() else {
            return pass_result(MISSING_SPEC, None);
        };
        let output = run_shell_command(&gate.command, self.working_dir, gate.timeout, self.env);
        self.env.record(&format!("gate '{}'", name), &output);
        let duration_ms = Some(output.duration_ms);
        let slice = output.stdout.as_str();
        // One request per criterion, in declaration order. Each carries the
        // same labelled slice, and the recorded input hash is taken from the
        // requests themselves, so it is always the hash of what is sent.
        let requests: Vec<DecisionRequest> = spec
            .criteria
            .iter()
            .map(|c| build_check_request(c, &spec.label, slice))
            .collect();

        // What the slice decides before anything is asked.
        let (early, input) = if output.failure_kind.is_some() {
            (
                Some(CheckOutcome::Unanswered(UnansweredReason::ExtractionFailed)),
                None,
            )
        } else if slice.trim().is_empty() {
            // Empty or only whitespace: nothing to ask about.
            (Some(CheckOutcome::NotGraded), None)
        } else {
            let input = requests
                .first()
                .map(|r| (input_sha256(&r.inputs), slice.len() as u64));
            if output.stdout_truncated || slice.len() > spec.max_bytes as usize {
                (
                    Some(CheckOutcome::Unanswered(UnansweredReason::OverBudget)),
                    input,
                )
            } else {
                (None, input)
            }
        };

        // Every record needs the visit; an outcome decided above still gets
        // one. The lock is only needed to consult.
        let lock = if early.is_none() {
            self.lock_waiting()
        } else {
            None
        };
        // Without the log there is no state or visit to record under, and
        // no verdict to reuse, so nothing is asked or recorded: every veto
        // criterion is listed as unanswered and the check passes. That
        // holds whatever the slice decided (empty, over budget, a failed
        // extraction), since none of it can be recorded either. A log that
        // names no state is treated the same way. The lock wait above may
        // have been spent for nothing here; an unreadable log is rare
        // enough not to read it twice.
        let read = self.backend.read_events_local(self.session).ok();
        let Some((session_id, events, state)) = read.as_ref().and_then(|(header, events)| {
            let state = derive_state_from_log(events).filter(|s| !s.is_empty())?;
            Some((
                Some(header.session_id.as_str()).filter(|s| !s.is_empty()),
                events.as_slice(),
                state,
            ))
        }) else {
            drop(lock);
            return unreadable_log_result(name, spec, self.global, duration_ms);
        };
        let visit_index = arrival_index(events, &state);
        let visit_seq = visit_index.map(|i| events[i].seq).unwrap_or(0);
        let visit_events = visit_index.map(|i| &events[i + 1..]).unwrap_or(events);

        let mut graded = Vec::with_capacity(spec.criteria.len());
        for (i, (criterion, request)) in spec.criteria.iter().zip(&requests).enumerate() {
            let hash = hashes.get(i).cloned().unwrap_or_default();
            let mode = effective_check_mode(criterion.mode, self.global);
            let sha = input.as_ref().map(|(sha, _)| sha.as_str());
            // Order matters (DESIGN-decider-fallback.md, Decision 2): an
            // empty slice is never graded; a reusable decider verdict comes
            // first; then an answer the agent recorded for this slice; only
            // then the early outcome, the lock and the cap. Every path that
            // ends unanswered is therefore checked for an answer first, so
            // any reason, present or future, falls back the same way.
            let reused = match early {
                None => sha.and_then(|s| {
                    reusable(visit_events, &state, name, &criterion.rule_id, &hash, s)
                }),
                Some(_) => None,
            };
            let answered = match (early, reused) {
                (Some(CheckOutcome::NotGraded), _) | (_, Some(_)) => None,
                _ => applicable_answer(visit_events, &state, name, &criterion.rule_id, &hash, sha),
            };
            let (outcome, record_from) = match (early, reused, answered, &lock) {
                (Some(CheckOutcome::NotGraded), ..) => (
                    CheckOutcome::NotGraded,
                    Some(Consulted::unsent(CheckOutcome::NotGraded)),
                ),
                (_, Some(o), ..) => (o, None),
                (_, None, Some(a), _) => (agent_outcome(a), None),
                (Some(o), None, None, _) => (o, Some(Consulted::unsent(o))),
                (None, None, None, None) => {
                    let o = CheckOutcome::Unanswered(UnansweredReason::Busy);
                    (o, Some(Consulted::unsent(o)))
                }
                (None, None, None, Some(_)) => {
                    let c = if self.budget.try_take() {
                        self.consult(criterion, request)
                    } else {
                        Consulted::unsent(CheckOutcome::Unanswered(UnansweredReason::CapSpent))
                    };
                    (c.outcome, Some(c))
                }
            };
            // The decider's own record keeps its rule: only a veto fail
            // from the decider blocks there. An agent fail blocks the same
            // way, only in veto.
            let blocked = match answered {
                Some(a) => mode == CheckMode::Veto && a == AgentAnswer::Fail,
                None => blocks(outcome, mode),
            };
            let fallback = match (outcome, answered) {
                (CheckOutcome::Unanswered(reason), None) => Some(DeciderFallback {
                    state: state.clone(),
                    visit_seq,
                    gate: name.to_string(),
                    rule_id: criterion.rule_id.clone(),
                    declaration_hash: hash.clone(),
                    mode,
                    reason,
                    input_sha256: input.as_ref().map(|(sha, _)| sha.clone()),
                    input_bytes: input.as_ref().map(|(_, n)| *n),
                }),
                _ => None,
            };
            let record = record_from.map(|c| {
                let (recorded, reason) = c.outcome.recorded();
                DeciderCheck {
                    state: state.clone(),
                    visit_seq,
                    gate: name.to_string(),
                    rule_id: criterion.rule_id.clone(),
                    rule_ref: criterion.rule_ref.clone(),
                    declaration_hash: hash.clone(),
                    mode,
                    threshold: criterion.threshold,
                    outcome: recorded,
                    reason,
                    blocked,
                    provider: self.decider.provider().to_string(),
                    model: c.model,
                    probabilities: recorded_probabilities(&c.probabilities),
                    input_sha256: input.as_ref().map(|(sha, _)| sha.clone()),
                    input_bytes: input.as_ref().map(|(_, n)| *n),
                    input_tokens: c.usage.input_tokens,
                    output_tokens: c.usage.output_tokens,
                    unread_usage_attempts: c.usage.unread_attempts,
                    attempts: c.attempts,
                    latency_ms: c.latency_ms,
                    error_class: c.error_class,
                    endpoint_origin: Some(self.endpoint_origin),
                }
            });
            graded.push(Graded {
                criterion,
                mode,
                outcome,
                agent: answered,
                blocked,
                record,
                fallback,
            });
        }
        drop(lock);

        let records: Vec<DeciderCheck> = graded.iter().filter_map(|g| g.record.clone()).collect();
        for record in &records {
            let line = LedgerRecord::checked(self.session, session_id, record.clone());
            append_or_warn(self.ledger_root.as_deref(), &line);
        }
        result_for(self.session, name, spec, &graded, records, duration_ms)
    }

    /// Ask the provider about one criterion, retrying once on a transient
    /// failure. A consultation counts once against the cap whatever the
    /// number of attempts.
    fn consult(
        &self,
        criterion: &crate::template::decider_check::CheckCriterion,
        request: &DecisionRequest,
    ) -> Consulted {
        let mut c = Consulted::unsent(CheckOutcome::Unanswered(UnansweredReason::ProviderError));
        while c.attempts < MAX_ATTEMPTS {
            c.attempts += 1;
            let started = Instant::now();
            let answer = self.decider.decide(request);
            c.latency_ms += started.elapsed().as_millis() as u64;
            match answer {
                Ok(response) => {
                    c.model = response.model.clone();
                    c.usage.add(response.usage);
                    let (outcome, probabilities) =
                        verdict(&response, &criterion.rule_id, criterion.threshold);
                    c.outcome = outcome;
                    c.probabilities = probabilities;
                    if outcome.is_verdict() {
                        c.error_class = None;
                        return c;
                    }
                    // An answer that isn't pass, fail or an escape reads as
                    // a malformed one, which is retried.
                    c.error_class = Some(ErrorClass::Malformed);
                }
                Err(e) => {
                    // A 2xx answer koto couldn't use was billed; a non-2xx
                    // status or a transport failure generally isn't, and
                    // carries no usage to read.
                    if e.responded {
                        c.usage.add(e.usage);
                        if let Some(model) = e.model {
                            c.model = model.0;
                        }
                    }
                    c.error_class = Some(e.class);
                    c.outcome = CheckOutcome::Unanswered(reason_for_error(e.class));
                    c.probabilities.clear();
                    if !is_retryable(e.class, e.status) {
                        return c;
                    }
                }
            }
        }
        c
    }
}

/// The latest verdict recorded in this visit for the same gate, criterion,
/// declaration and slice. An unanswered or not-graded outcome is never
/// reused.
fn reusable(
    visit_events: &[Event],
    state: &str,
    gate: &str,
    rule_id: &str,
    hash: &str,
    sha: &str,
) -> Option<CheckOutcome> {
    visit_events.iter().rev().find_map(|e| match &e.payload {
        EventPayload::DeciderChecked(c)
            if c.state == state
                && c.gate == gate
                && c.rule_id == rule_id
                && c.declaration_hash == hash
                && c.input_sha256.as_deref() == Some(sha) =>
        {
            CheckOutcome::from_recorded_verdict(c.outcome)
        }
        _ => None,
    })
}

/// The latest answer the agent recorded in this visit for the same gate,
/// criterion, declaration and slice (DESIGN-decider-fallback.md, Decision
/// 2). Two absent slice hashes match: a failed extraction has no slice,
/// and its answer stands while extraction keeps failing.
fn applicable_answer(
    visit_events: &[Event],
    state: &str,
    gate: &str,
    rule_id: &str,
    hash: &str,
    sha: Option<&str>,
) -> Option<AgentAnswer> {
    visit_events.iter().rev().find_map(|e| match &e.payload {
        EventPayload::DeciderCheckAnswered(a)
            if a.state == state
                && a.gate == gate
                && a.rule_id == rule_id
                && a.declaration_hash == hash
                && a.input_sha256.as_deref() == sha =>
        {
            Some(a.answer)
        }
        _ => None,
    })
}

/// The check's output (DESIGN-decider-fallback.md, Decision 3): the
/// criteria that block on a fail, the veto criteria with no verdict, the
/// criteria waiting on the agent, the criteria the agent's answer decided,
/// and why no criterion could be graded at all (empty when they could).
/// `awaiting` and `agent_answered` stay out of the gate type's schema, so an
/// `override_default` without them is still valid.
fn check_output(
    failed: &[&str],
    unanswered: &[&str],
    awaiting: &[&str],
    agent_answered: &[&str],
    error: &str,
) -> serde_json::Value {
    serde_json::json!({
        "failed": failed,
        "unanswered": unanswered,
        "awaiting": awaiting,
        "agent_answered": agent_answered,
        "error": error,
    })
}

/// A passing result with nothing to grade: no spec.
fn pass_result(error: &str, duration_ms: Option<u64>) -> StructuredGateResult {
    StructuredGateResult {
        outcome: GateOutcome::Passed,
        output: check_output(&[], &[], &[], &[], error),
        duration_ms,
        ..Default::default()
    }
}

/// A check whose session log can't be read: no state or visit to record
/// under, no verdict to reuse and no answer to read, so nothing is asked or
/// recorded and every criterion waits. The gate is `pending`, so the state
/// stops without counting a judgment; with no readable log the agent can't
/// record an answer either, so the state stays stopped until the log is
/// repaired or an override is recorded.
fn unreadable_log_result(
    gate: &str,
    spec: &DeciderCheckSpec,
    global: GlobalMode,
    duration_ms: Option<u64>,
) -> StructuredGateResult {
    let all: Vec<&str> = spec.criteria.iter().map(|c| c.rule_id.as_str()).collect();
    let veto: Vec<&str> = spec
        .criteria
        .iter()
        .filter(|c| effective_check_mode(c.mode, global) == CheckMode::Veto)
        .map(|c| c.rule_id.as_str())
        .collect();
    let finding = Finding {
        rule_id: gate.to_string(),
        level: FindingLevel::Warning,
        message: format!(
            "decider check {} can't be graded: the session log couldn't be read, \
             so koto can't ask the decider or record your answer; repair the session \
             or record an override",
            gate
        ),
        path: None,
        line: None,
        column: None,
        rule_ref: None,
        effect_landed: None,
        message_source: MessageSource::Koto,
    };
    StructuredGateResult {
        outcome: GateOutcome::Pending,
        output: check_output(&[], &veto, &all, &[], LOG_UNREADABLE),
        failure: Some(GateFailure {
            fallback: None,
            captured: None,
        }),
        findings: vec![finding],
        duration_ms,
        ..Default::default()
    }
}

/// `s` as one shell word an agent can paste: as-is in the name shape,
/// otherwise single-quoted with each `'` written as `'\''`.
fn shell_word(s: &str) -> String {
    let plain = !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:/@-".contains(&b));
    if plain {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// The finding that hands a waiting criterion to the agent. The command
/// comes before the template's text: a finding's message is capped at
/// `MESSAGE_MAX_BYTES`, and a cut must lose the end of the descriptions,
/// never the command.
fn waiting_finding(
    session: &str,
    gate: &str,
    criterion: &crate::template::decider_check::CheckCriterion,
    reason: UnansweredReason,
) -> Finding {
    Finding {
        rule_id: criterion.rule_id.clone(),
        level: FindingLevel::Warning,
        message: format!(
            "criterion {} is waiting on your answer: the decider gave no verdict ({}). \
             Record it with `koto decider answer {} --gate {} --criterion {} --answer \
             <pass|fail|unclear>`, then run `koto next {}` again. {} Answer pass if: {} \
             Answer fail if: {} Answer unclear if: {}",
            criterion.rule_id,
            reason.as_str(),
            shell_word(session),
            shell_word(gate),
            shell_word(&criterion.rule_id),
            shell_word(session),
            criterion.question,
            criterion.pass,
            criterion.fail,
            criterion.escape,
        ),
        path: None,
        line: None,
        column: None,
        rule_ref: Some(criterion.rule_ref.clone()),
        effect_landed: None,
        message_source: MessageSource::Koto,
    }
}

/// The gate's result from its criteria's outcomes (Decision 3 of
/// DESIGN-decider-fallback.md): `failed` with one finding per blocking
/// criterion when a veto criterion failed, by the decider's verdict or the
/// agent's answer; otherwise `pending`, with one warning finding per
/// criterion waiting on the agent, when any waits; otherwise `passed`. A
/// `pending` evaluation judged nothing, so it stamps no attempt and counts
/// no rule. A shadow criterion never blocks, whoever answered.
fn result_for(
    session: &str,
    gate: &str,
    spec: &DeciderCheckSpec,
    graded: &[Graded],
    records: Vec<DeciderCheck>,
    duration_ms: Option<u64>,
) -> StructuredGateResult {
    let ids = |keep: &dyn Fn(&Graded) -> bool| -> Vec<&str> {
        graded
            .iter()
            .filter(|g| keep(g))
            .map(|g| g.criterion.rule_id.as_str())
            .collect()
    };
    let failed = ids(&|g| g.blocked);
    let unanswered = ids(&|g| g.mode == CheckMode::Veto && g.waiting().is_some());
    let awaiting = ids(&|g| g.waiting().is_some());
    let agent_answered = ids(&|g| g.agent.is_some());

    let mut findings = Vec::new();
    for g in graded {
        if g.blocked {
            let c = g.criterion;
            let (message, source) = if g.agent.is_some() {
                (
                    format!(
                        "criterion {} failed: you answered fail for the {}: {}",
                        c.rule_id, spec.label, c.fail
                    ),
                    MessageSource::Koto,
                )
            } else {
                (
                    format!(
                        "criterion {} failed: the decider judged the {} to fail it: {}",
                        c.rule_id, spec.label, c.fail
                    ),
                    MessageSource::Decider,
                )
            };
            findings.push(Finding {
                rule_id: c.rule_id.clone(),
                level: FindingLevel::Error,
                message,
                path: None,
                line: None,
                column: None,
                rule_ref: Some(c.rule_ref.clone()),
                effect_landed: None,
                message_source: source,
            });
        } else if let Some(reason) = g.waiting() {
            findings.push(waiting_finding(session, gate, g.criterion, reason));
        }
    }

    let outcome = if !failed.is_empty() {
        GateOutcome::Failed
    } else if !awaiting.is_empty() {
        GateOutcome::Pending
    } else {
        GateOutcome::Passed
    };
    let failure = (!findings.is_empty()).then_some(GateFailure {
        fallback: None,
        captured: None,
    });
    StructuredGateResult {
        outcome,
        output: check_output(&failed, &unanswered, &awaiting, &agent_answered, ""),
        failure,
        findings,
        duration_ms,
        decider_checks: records,
        decider_fallbacks: graded.iter().filter_map(|g| g.fallback.clone()).collect(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_budget_is_shared_and_capped() {
        let a = ConsultBudget::new();
        let b = a.clone();
        for _ in 0..MAX_CONSULTATIONS_PER_CALL - 1 {
            assert!(a.try_take());
        }
        assert!(b.try_take(), "a clone takes from the same count");
        assert!(!a.try_take());
        assert!(!b.try_take());
        assert_eq!(a.used(), MAX_CONSULTATIONS_PER_CALL);
    }

    #[test]
    fn the_lock_wait_covers_a_holder_and_is_capped() {
        assert_eq!(
            lock_wait_for(Duration::from_millis(2_000)),
            Duration::from_secs(17)
        );
        assert_eq!(
            lock_wait_for(Duration::from_millis(50)),
            Duration::from_millis(1_400)
        );
        assert_eq!(lock_wait_for(Duration::from_secs(10)), MAX_LOCK_WAIT);
    }

    // -- the evaluator against a real local session ----------------------

    use crate::decider::fake::ScriptedDecider;
    use crate::decider::types::{DecisionRequest, DecisionResponse};
    use crate::engine::types::StateFileHeader;
    use crate::session::local::LocalBackend;
    use crate::template::decider_check::CheckCriterion;
    use std::sync::Arc;

    struct Shared(Arc<ScriptedDecider>);

    impl Decider for Shared {
        fn provider(&self) -> &str {
            self.0.provider()
        }

        fn decide(
            &self,
            req: &DecisionRequest,
        ) -> Result<DecisionResponse, crate::decider::types::DeciderError> {
            self.0.decide(req)
        }
    }

    /// A decider check over a fixed slice, one criterion per `(id, mode)`.
    fn check_gate(criteria: &[(&str, CheckMode)]) -> Gate {
        let mut gate: Gate = serde_json::from_value(serde_json::json!({
            "type": GATE_TYPE_DECIDER_CHECK,
            "command": "printf 'let x = 1; // sets x'",
        }))
        .unwrap();
        gate.decider_check = Some(DeciderCheckSpec {
            max_bytes: 2560,
            label: "comments".into(),
            criteria: criteria
                .iter()
                .map(|(id, mode)| CheckCriterion {
                    rule_id: id.to_string(),
                    rule_ref: "ref".into(),
                    question: "Does each comment give a reason?".into(),
                    pass: "yes".into(),
                    fail: "no".into(),
                    escape: "can't tell".into(),
                    threshold: 0.9,
                    mode: *mode,
                })
                .collect(),
        });
        gate
    }

    /// A session `wf` in state `review`, or, with `log: false`, a session
    /// directory with no log to read.
    fn backend(tmp: &Path, log: bool) -> LocalBackend {
        let backend = LocalBackend::with_base_dir(tmp.to_path_buf());
        backend.create("wf").unwrap();
        if log {
            let header: StateFileHeader = serde_json::from_value(serde_json::json!({
                "schema_version": 1,
                "workflow": "wf",
                "template_hash": "0".repeat(64),
                "created_at": "2026-01-01T00:00:00Z",
                "session_id": "s",
            }))
            .unwrap();
            let events: Vec<Event> = [
                r#"{"seq":1,"timestamp":"2026-01-01T00:00:00Z","type":"workflow_initialized","payload":{"template_path":"t.json","variables":{}}}"#,
                r#"{"seq":2,"timestamp":"2026-01-01T00:00:00Z","type":"transitioned","payload":{"from":null,"to":"review","condition_type":"auto"}}"#,
            ]
            .iter()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
            backend.init_state_file("wf", header, events).unwrap();
        }
        backend
    }

    fn evaluate(
        backend: &LocalBackend,
        dir: &Path,
        gate: &Gate,
        decider: &Arc<ScriptedDecider>,
    ) -> StructuredGateResult {
        let env = CommandEnv::inherit();
        let evaluator = CliCheckEvaluator::new(
            backend,
            "wf",
            Box::new(Shared(decider.clone())),
            GlobalMode::Auto,
            SettingOrigin::Default,
            ConsultBudget::new(),
            dir,
            &env,
        );
        let hashes = declaration_hashes(&BTreeMap::from([("comments".to_string(), gate.clone())]));
        evaluator.evaluate("comments", gate, &hashes["comments"])
    }

    #[test]
    fn a_check_with_no_spec_passes_and_says_why() {
        let tmp = tempfile::TempDir::new().unwrap();
        let backend = backend(tmp.path(), true);
        let mut gate = check_gate(&[("r", CheckMode::Veto)]);
        gate.decider_check = None;
        let decider = Arc::new(ScriptedDecider::new());
        let env = CommandEnv::inherit();
        let evaluator = CliCheckEvaluator::new(
            &backend,
            "wf",
            Box::new(Shared(decider.clone())),
            GlobalMode::Auto,
            SettingOrigin::Default,
            ConsultBudget::new(),
            tmp.path(),
            &env,
        );
        let r = evaluator.evaluate("comments", &gate, &[]);
        assert_eq!(r.outcome, GateOutcome::Passed);
        assert_eq!(
            r.output,
            serde_json::json!({"failed": [], "unanswered": [], "awaiting": [],
                "agent_answered": [], "error": "missing_spec"})
        );
        assert!(r.decider_checks.is_empty());
        assert!(r.failure.is_none() && r.findings.is_empty());
        assert_eq!(decider.calls(), 0);
    }

    #[test]
    fn an_unreadable_log_waits_on_every_criterion_and_records_nothing() {
        for (mode, listed) in [
            (CheckMode::Veto, serde_json::json!(["a", "b"])),
            (CheckMode::Shadow, serde_json::json!([])),
        ] {
            let tmp = tempfile::TempDir::new().unwrap();
            let backend = backend(tmp.path(), false);
            let gate = check_gate(&[("a", mode), ("b", mode)]);
            let decider = Arc::new(ScriptedDecider::new());
            let r = evaluate(&backend, tmp.path(), &gate, &decider);
            assert_eq!(r.outcome, GateOutcome::Pending, "{:?}", mode);
            assert_eq!(
                r.output,
                serde_json::json!({"failed": [], "unanswered": listed,
                    "awaiting": ["a", "b"], "agent_answered": [], "error": "log_unreadable"}),
                "{:?}",
                mode
            );
            // Nothing is recorded, so nothing lands under an empty state.
            assert!(r.decider_checks.is_empty(), "{:?}", mode);
            assert!(r.decider_fallbacks.is_empty(), "{:?}", mode);
            assert_eq!(r.findings.len(), 1);
            assert_eq!(r.findings[0].level, FindingLevel::Warning);
            assert_eq!(r.findings[0].rule_id, "comments");
            assert!(r.failure.is_some());
            assert_eq!(decider.calls(), 0, "nothing is asked without a visit");
        }
    }

    #[test]
    fn a_value_outside_the_name_shape_is_quoted_for_the_shell() {
        assert_eq!(shell_word("comment_reason"), "comment_reason");
        assert_eq!(shell_word("parent.task-a"), "parent.task-a");
        assert_eq!(shell_word("a b"), "'a b'");
        assert_eq!(shell_word("x;rm -rf ~"), "'x;rm -rf ~'");
        assert_eq!(shell_word("it's"), r"'it'\''s'");
        assert_eq!(shell_word("$(id)"), "'$(id)'");
        assert_eq!(shell_word(""), "''");
    }

    /// Log events from JSON lines, numbered from 1.
    fn log(lines: &[serde_json::Value]) -> Vec<Event> {
        lines
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let mut l = l.clone();
                l["seq"] = serde_json::json!(i as u64 + 1);
                l["timestamp"] = serde_json::json!("2026-01-01T00:00:00Z");
                serde_json::from_value(l).unwrap()
            })
            .collect()
    }

    fn arrive(from: Option<&str>, to: &str) -> serde_json::Value {
        serde_json::json!({"type": "transitioned",
            "payload": {"from": from, "to": to, "condition_type": "auto"}})
    }

    fn checked(visit_seq: u64, hash: &str) -> serde_json::Value {
        serde_json::json!({"type": "decider_checked", "payload": {
            "state": "review", "visit_seq": visit_seq, "gate": "comments",
            "rule_id": "r", "rule_ref": "ref", "declaration_hash": hash,
            "mode": "veto", "threshold": 0.9, "outcome": "fail", "blocked": true,
            "provider": "jev", "model": "m", "attempts": 1, "latency_ms": 1}})
    }

    fn overridden(events: &[Event]) -> Vec<(u64, String)> {
        override_records(
            events,
            "review",
            "comments",
            &serde_json::json!({"failed": ["r"], "unanswered": [], "error": ""}),
            "wf",
            None,
        )
        .into_iter()
        .map(|r| match r {
            LedgerRecord::CheckOverridden(c) => {
                assert_eq!(
                    c.override_kind,
                    crate::decider::ledger::CheckOverrideKind::CandidateFalseFail
                );
                (c.visit_seq, c.declaration_hash)
            }
            other => panic!("unexpected record {:?}", other),
        })
        .collect()
    }

    #[test]
    fn an_override_names_the_consultation_of_the_current_visit_only() {
        let init = serde_json::json!({"type": "workflow_initialized",
            "payload": {"template_path": "t.json", "variables": {}}});
        // Visit 1 (seq 2) failed r and was left; visit 2 (seq 5) failed it
        // again. The override names visit 2's consultation.
        let two_visits = log(&[
            init.clone(),
            arrive(None, "review"),
            checked(2, "old"),
            arrive(Some("review"), "work"),
            arrive(Some("work"), "review"),
            checked(5, "new"),
        ]);
        assert_eq!(overridden(&two_visits), vec![(5, "new".to_string())]);

        // The current visit has no consultation of r: an earlier visit's is
        // not what the override moved past, so nothing is written. Looking
        // across the whole log would name visit 1 here.
        let stale = log(&[
            init.clone(),
            arrive(None, "review"),
            checked(2, "old"),
            arrive(Some("review"), "work"),
            arrive(Some("work"), "review"),
        ]);
        assert_eq!(overridden(&stale), vec![]);

        // One visit: its consultation.
        let one = log(&[init, arrive(None, "review"), checked(2, "only")]);
        assert_eq!(overridden(&one), vec![(2, "only".to_string())]);
    }

    #[test]
    fn a_readable_log_records_under_its_state() {
        let tmp = tempfile::TempDir::new().unwrap();
        let backend = backend(tmp.path(), true);
        let gate = check_gate(&[("r", CheckMode::Veto)]);
        let decider = Arc::new(ScriptedDecider::new());
        // No reply queued: the provider fails twice, so no verdict.
        let r = evaluate(&backend, tmp.path(), &gate, &decider);
        assert_eq!(r.outcome, GateOutcome::Pending);
        assert_eq!(r.output["unanswered"], serde_json::json!(["r"]));
        assert_eq!(r.output["awaiting"], serde_json::json!(["r"]));
        let c = &r.decider_checks[0];
        assert_eq!(c.state, "review");
        assert_eq!(c.visit_seq, 2);
        assert!(!c.blocked);
        assert_eq!(c.reason, Some(UnansweredReason::ProviderError));
        let f = &r.decider_fallbacks[0];
        assert_eq!((f.state.as_str(), f.visit_seq), ("review", 2));
        assert_eq!(f.reason, UnansweredReason::ProviderError);
        assert_eq!(f.declaration_hash, c.declaration_hash);
        assert_eq!(f.input_sha256, c.input_sha256);
        assert_eq!(f.input_bytes, c.input_bytes);
    }

    /// Append `answer` for criterion `r` of `comments`, bound to the slice
    /// and declaration of the last fallback the backend's log holds.
    fn answer(backend: &LocalBackend, fallback: &DeciderFallback, answer: AgentAnswer) {
        let p = EventPayload::DeciderCheckAnswered(crate::decider::check::DeciderCheckAnswered {
            state: fallback.state.clone(),
            visit_seq: fallback.visit_seq,
            gate: fallback.gate.clone(),
            rule_id: fallback.rule_id.clone(),
            declaration_hash: fallback.declaration_hash.clone(),
            input_sha256: fallback.input_sha256.clone(),
            answer,
            fallback_seq: 3,
        });
        backend
            .append_event("wf", &p, &crate::engine::types::now_iso8601())
            .unwrap();
    }

    #[test]
    fn an_answer_decides_the_criterion_by_mode_without_asking() {
        use AgentAnswer::*;
        for (mode, ans, outcome, failed) in [
            (CheckMode::Veto, Pass, GateOutcome::Passed, false),
            (CheckMode::Veto, Unclear, GateOutcome::Passed, false),
            (CheckMode::Veto, Fail, GateOutcome::Failed, true),
            (CheckMode::Shadow, Fail, GateOutcome::Passed, false),
        ] {
            let tmp = tempfile::TempDir::new().unwrap();
            let backend = backend(tmp.path(), true);
            let gate = check_gate(&[("r", mode)]);
            let decider = Arc::new(ScriptedDecider::new());
            let first = evaluate(&backend, tmp.path(), &gate, &decider);
            assert_eq!(first.outcome, GateOutcome::Pending, "{:?}", mode);
            answer(&backend, &first.decider_fallbacks[0], ans);
            let calls = decider.calls();
            let r = evaluate(&backend, tmp.path(), &gate, &decider);
            assert_eq!(r.outcome, outcome, "{:?} {:?}", mode, ans);
            assert_eq!(decider.calls(), calls, "an answer is never re-asked");
            assert!(r.decider_checks.is_empty() && r.decider_fallbacks.is_empty());
            assert_eq!(r.output["agent_answered"], serde_json::json!(["r"]));
            assert_eq!(r.output["unanswered"], serde_json::json!([]));
            assert_eq!(r.output["awaiting"], serde_json::json!([]));
            let expected = if failed {
                serde_json::json!(["r"])
            } else {
                serde_json::json!([])
            };
            assert_eq!(r.output["failed"], expected, "{:?} {:?}", mode, ans);
            if failed {
                assert_eq!(r.findings.len(), 1);
                assert_eq!(r.findings[0].level, FindingLevel::Error);
                assert_eq!(r.findings[0].message_source, MessageSource::Koto);
                assert!(r.findings[0].message.contains("you answered fail"));
            } else {
                assert!(r.findings.is_empty(), "{:?}", r.findings);
            }
        }
    }

    #[test]
    fn an_answer_for_another_slice_or_declaration_does_not_apply() {
        let tmp = tempfile::TempDir::new().unwrap();
        let backend = backend(tmp.path(), true);
        let gate = check_gate(&[("r", CheckMode::Veto)]);
        let decider = Arc::new(ScriptedDecider::new());
        let first = evaluate(&backend, tmp.path(), &gate, &decider);
        let mut other_slice = first.decider_fallbacks[0].clone();
        other_slice.input_sha256 = Some("0".repeat(64));
        answer(&backend, &other_slice, AgentAnswer::Pass);
        let mut other_decl = first.decider_fallbacks[0].clone();
        other_decl.declaration_hash = "changed".into();
        answer(&backend, &other_decl, AgentAnswer::Pass);
        let r = evaluate(&backend, tmp.path(), &gate, &decider);
        assert_eq!(r.outcome, GateOutcome::Pending);
        assert_eq!(r.output["agent_answered"], serde_json::json!([]));
        assert_eq!(r.decider_fallbacks.len(), 1, "asked again");
    }

    #[test]
    fn the_latest_answer_for_a_slice_wins() {
        let tmp = tempfile::TempDir::new().unwrap();
        let backend = backend(tmp.path(), true);
        let gate = check_gate(&[("r", CheckMode::Veto)]);
        let decider = Arc::new(ScriptedDecider::new());
        let first = evaluate(&backend, tmp.path(), &gate, &decider);
        answer(&backend, &first.decider_fallbacks[0], AgentAnswer::Fail);
        assert_eq!(
            evaluate(&backend, tmp.path(), &gate, &decider).outcome,
            GateOutcome::Failed
        );
        answer(&backend, &first.decider_fallbacks[0], AgentAnswer::Pass);
        assert_eq!(
            evaluate(&backend, tmp.path(), &gate, &decider).outcome,
            GateOutcome::Passed
        );
    }

    #[test]
    fn a_waiting_finding_puts_the_command_first_and_quotes_odd_names() {
        let mut gate = check_gate(&[("r", CheckMode::Veto)]);
        let c = &mut gate.decider_check.as_mut().unwrap().criteria[0];
        c.rule_id = "odd id".into();
        c.question = "Q?".into();
        let f = waiting_finding("wf", "comments", c, UnansweredReason::OverBudget);
        assert_eq!(f.level, FindingLevel::Warning);
        assert_eq!(f.message_source, MessageSource::Koto);
        let cmd = "`koto decider answer wf --gate comments --criterion 'odd id' --answer";
        let at = f.message.find(cmd).expect(&f.message);
        assert!(at < f.message.find("Q?").unwrap(), "{}", f.message);
        assert!(f.message.contains("(over_budget)"));
        assert!(f.message.contains("Answer unclear if: can't tell"));
    }

    #[test]
    fn an_override_skips_criteria_the_agent_answered() {
        let init = serde_json::json!({"type": "workflow_initialized",
            "payload": {"template_path": "t.json", "variables": {}}});
        let events = log(&[init, arrive(None, "review"), checked(2, "h")]);
        let records = override_records(
            &events,
            "review",
            "comments",
            &serde_json::json!({"failed": ["r"], "unanswered": [], "awaiting": [],
                "agent_answered": ["r"], "error": ""}),
            "wf",
            None,
        );
        assert!(records.is_empty());
    }
}
