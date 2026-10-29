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
    UnansweredReason, UnitLabels, UnitRef,
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
use crate::template::decider_check::{CheckMode, DeciderCheckSpec, UnitsMode};
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

/// Most (unit, criterion) consultations one `koto next` sends for unit-mode
/// checks (DESIGN-doc-rule-checks-m4.md, Decision 2). It is the per-state
/// compile-time bound, so one state's declared units always fit.
pub const MAX_UNIT_CONSULTATIONS_PER_CALL: usize =
    crate::template::decider_check::MAX_UNIT_CONSULTATIONS_PER_STATE as usize;

/// The per-`koto next` count of unit-mode consultations. It is separate
/// from [`ConsultBudget`]: a unit-mode check draws only from this one, so
/// the routing decider's allowance is never spent on units. Like
/// [`ConsultBudget`], a slot is taken only for a request about to be sent.
#[derive(Debug, Clone, Default)]
pub struct UnitBudget(Rc<Cell<usize>>);

impl UnitBudget {
    pub fn new() -> Self {
        Self::default()
    }

    /// Take one slot for a request about to be sent; `false` when spent.
    pub fn try_take(&self) -> bool {
        let used = self.0.get();
        if used >= MAX_UNIT_CONSULTATIONS_PER_CALL {
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

/// Longest unit key, document or section, in bytes.
pub const MAX_UNIT_NAME_BYTES: usize = 128;

/// Whether `s` fits the name shape a unit's key, document and section must
/// have: 1 to 128 bytes of letters, digits and `._:@/-`, not starting with
/// `/` or `-`, so it can go into a command line as written and never reads
/// as an option.
pub fn unit_name_is_valid(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_UNIT_NAME_BYTES
        && !s.starts_with(['/', '-'])
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:@/-".contains(&b))
}

/// One unit an extraction printed.
#[derive(Debug, Clone, PartialEq)]
struct Unit {
    reference: UnitRef,
    text: String,
}

/// The units in a unit-mode extraction's output: one JSON object per line,
/// with string `key` and `text` and optional string `document` and
/// `section`. Blank lines are skipped, unknown fields ignored, and a `null`
/// optional field read as absent. `None` when any line isn't such an
/// object, a name is outside [`unit_name_is_valid`], or a key repeats.
fn parse_units(stdout: &str) -> Option<Vec<Unit>> {
    let mut units: Vec<Unit> = Vec::new();
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        let obj = value.as_object()?;
        let key = obj.get("key")?.as_str()?;
        let text = obj.get("text")?.as_str()?;
        let optional = |name: &str| -> Option<Option<String>> {
            match obj.get(name) {
                None | Some(serde_json::Value::Null) => Some(None),
                Some(serde_json::Value::String(s)) if unit_name_is_valid(s) => {
                    Some(Some(s.clone()))
                }
                Some(_) => None,
            }
        };
        let document = optional("document")?;
        let section = optional("section")?;
        if !unit_name_is_valid(key) || units.iter().any(|u| u.reference.key == key) {
            return None;
        }
        units.push(Unit {
            reference: UnitRef {
                key: key.to_string(),
                document,
                section,
            },
            text: text.to_string(),
        });
    }
    Some(units)
}

/// The visit a check is graded in: the state, the `seq` of the event that
/// opened the visit, and the events since.
struct Visit<'e> {
    state: String,
    seq: u64,
    events: &'e [Event],
}

impl<'e> Visit<'e> {
    fn of(events: &'e [Event], state: String) -> Self {
        let index = arrival_index(events, &state);
        Visit {
            seq: index.map(|i| events[i].seq).unwrap_or(0),
            events: index.map(|i| &events[i + 1..]).unwrap_or(events),
            state,
        }
    }
}

/// One unit's requests (one per criterion, in declaration order), what the
/// unit decided before anything is asked, and its input's hash and size.
type UnitRequests = (
    Vec<DecisionRequest>,
    Option<CheckOutcome>,
    Option<(String, u64)>,
);

/// One criterion to grade, on the whole slice or on one unit.
struct Pair<'s, 'a> {
    criterion: &'s crate::template::decider_check::CheckCriterion,
    hash: String,
    request: &'a DecisionRequest,
    /// What the slice or unit decided before anything is asked.
    early: Option<CheckOutcome>,
    /// The input's hash and size; absent with no input.
    input: Option<&'a (String, u64)>,
    unit: Option<&'a UnitRef>,
    /// The check's declared unit labels, which the records carry beside
    /// `unit`; absent with no unit.
    labels: Option<&'a UnitLabels>,
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
                    .map(|c| spec.declaration_hash(c, &g.command))
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
    /// The unit graded, for a unit-mode check.
    unit: Option<UnitRef>,
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
    /// Unit-mode consultations this call; never shared with the routing
    /// decider.
    unit_budget: UnitBudget,
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
            unit_budget: UnitBudget::new(),
            ledger_root: None,
            lock_wait: Duration::ZERO,
            working_dir,
            env,
        }
    }

    /// The per-call unit budget unit-mode checks draw from; an evaluator
    /// built without this gets a fresh one.
    pub fn with_unit_budget(mut self, budget: UnitBudget) -> Self {
        self.unit_budget = budget;
        self
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
        if spec.units == Some(UnitsMode::Jsonl) {
            // A cut capture ends in a line that can't be parsed, so it is an
            // extraction failure, like a command that failed.
            let units = if output.failure_kind.is_some() || output.stdout_truncated {
                None
            } else {
                parse_units(&output.stdout)
            };
            return self.evaluate_units(name, spec, hashes, units, duration_ms);
        }
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
        // Without the log there is no state or visit to record under, no
        // verdict to reuse and no answer to read, so nothing is asked or
        // recorded: every criterion waits and the check is pending. That
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
        let visit = Visit::of(events, state);

        let take = || self.budget.try_take();
        let mut graded = Vec::with_capacity(spec.criteria.len());
        for (i, (criterion, request)) in spec.criteria.iter().zip(&requests).enumerate() {
            graded.push(self.grade(
                &visit,
                name,
                Pair {
                    criterion,
                    hash: hashes.get(i).cloned().unwrap_or_default(),
                    request,
                    early,
                    input: input.as_ref(),
                    unit: None,
                    labels: None,
                },
                lock.is_some(),
                (&take, UnansweredReason::CapSpent),
            ));
        }
        drop(lock);
        self.finish(name, spec, graded, session_id, None, duration_ms)
    }

    /// Evaluate a unit-mode check (DESIGN-doc-rule-checks-m4.md, Decisions 1
    /// to 3). `units` is what the extraction printed, or `None` when it
    /// failed, timed out, was cut at the capture bound, or printed a line
    /// that isn't a unit: then every criterion is unanswered with
    /// `extraction_failed` and no unit is named. No units at all grades
    /// nothing. Otherwise each unit gets its own early outcome (empty is
    /// not graded, longer than `max_bytes` is over budget), and the rest are
    /// graded one (unit, criterion) request at a time, units in print order
    /// and criteria in declaration order, each drawing a slot from the
    /// per-call unit budget, never from the cap the routing decider shares.
    fn evaluate_units(
        &self,
        name: &str,
        spec: &DeciderCheckSpec,
        hashes: &[String],
        units: Option<Vec<Unit>>,
        duration_ms: Option<u64>,
    ) -> StructuredGateResult {
        // One request per (unit, criterion). Each unit's input hash is taken
        // from its own request, so two units' verdicts never collide. A unit
        // past the check's `max_units` is decided first, whatever its size,
        // so it is never sent; then an empty unit, then one over budget.
        let limit = spec.max_units.map_or(usize::MAX, |n| n as usize);
        let per_unit: Vec<UnitRequests> = units
            .iter()
            .flatten()
            .enumerate()
            .map(|(index, u)| {
                let requests: Vec<DecisionRequest> = spec
                    .criteria
                    .iter()
                    .map(|c| build_check_request(c, &spec.label, &u.text))
                    .collect();
                let empty = u.text.trim().is_empty();
                let input = requests
                    .first()
                    .filter(|_| !empty)
                    .map(|r| (input_sha256(&r.inputs), u.text.len() as u64));
                let early = if index >= limit {
                    Some(CheckOutcome::Unanswered(UnansweredReason::UnitLimit))
                } else if empty {
                    Some(CheckOutcome::NotGraded)
                } else if u.text.len() > spec.max_bytes as usize {
                    Some(CheckOutcome::Unanswered(UnansweredReason::OverBudget))
                } else {
                    None
                };
                (requests, early, input)
            })
            .collect();
        let lock = if per_unit.iter().any(|(_, early, _)| early.is_none()) {
            self.lock_waiting()
        } else {
            None
        };
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
        let visit = Visit::of(events, state);

        let mut graded = Vec::new();
        match per_unit.is_empty() {
            // No unit to grade: each criterion once, with no unit, exactly
            // as a failed extraction or an empty slice is graded today.
            true => {
                let early = if units.is_none() {
                    CheckOutcome::Unanswered(UnansweredReason::ExtractionFailed)
                } else {
                    CheckOutcome::NotGraded
                };
                let none = || false;
                for (i, criterion) in spec.criteria.iter().enumerate() {
                    let request = build_check_request(criterion, &spec.label, "");
                    graded.push(self.grade(
                        &visit,
                        name,
                        Pair {
                            criterion,
                            hash: hashes.get(i).cloned().unwrap_or_default(),
                            request: &request,
                            early: Some(early),
                            input: None,
                            unit: None,
                            labels: None,
                        },
                        false,
                        (&none, UnansweredReason::UnitLimit),
                    ));
                }
            }
            false => {
                let take = || self.unit_budget.try_take();
                let labels = UnitLabels::declared(
                    spec.unit_document.as_deref(),
                    spec.unit_section.as_deref(),
                );
                for (unit, (requests, early, input)) in units.iter().flatten().zip(&per_unit) {
                    for (i, (criterion, request)) in spec.criteria.iter().zip(requests).enumerate()
                    {
                        graded.push(self.grade(
                            &visit,
                            name,
                            Pair {
                                criterion,
                                hash: hashes.get(i).cloned().unwrap_or_default(),
                                request,
                                early: *early,
                                input: input.as_ref(),
                                unit: Some(&unit.reference),
                                labels: labels.as_ref(),
                            },
                            lock.is_some(),
                            (&take, UnansweredReason::UnitLimit),
                        ));
                    }
                }
            }
        }
        drop(lock);
        // A failed extraction and one that printed nothing both list no
        // units: in neither case did the extraction name a unit.
        let printed: Vec<UnitRef> = units
            .iter()
            .flatten()
            .map(|u| u.reference.clone())
            .collect();
        self.finish(name, spec, graded, session_id, Some(&printed), duration_ms)
    }

    /// Append each new record's ledger line and build the gate's result.
    fn finish(
        &self,
        name: &str,
        spec: &DeciderCheckSpec,
        graded: Vec<Graded>,
        session_id: Option<&str>,
        units: Option<&[UnitRef]>,
        duration_ms: Option<u64>,
    ) -> StructuredGateResult {
        let records: Vec<DeciderCheck> = graded.iter().filter_map(|g| g.record.clone()).collect();
        for record in &records {
            let line = LedgerRecord::checked(self.session, session_id, record.clone());
            append_or_warn(self.ledger_root.as_deref(), &line);
        }
        result_for(
            self.session,
            name,
            spec,
            &graded,
            units,
            records,
            duration_ms,
        )
    }

    /// Grade one criterion, on the whole slice or on one unit, and build its
    /// records. `locked` says whether this tick holds `decider.lock`;
    /// `budget` takes a slot for a request about to be sent and names the
    /// reason recorded when none is left.
    ///
    /// Order matters (DESIGN-decider-fallback.md, Decision 2): an empty
    /// slice is never graded; a reusable decider verdict comes first; then
    /// an answer the agent recorded for this slice; only then the early
    /// outcome, the lock and the budget. Every path that ends unanswered is
    /// therefore checked for an answer first, so any reason, present or
    /// future, falls back the same way. Reuse and answers match on the unit
    /// too, so neither ever carries over from one unit to another.
    fn grade<'s>(
        &self,
        visit: &Visit,
        name: &str,
        pair: Pair<'s, '_>,
        locked: bool,
        budget: (&dyn Fn() -> bool, UnansweredReason),
    ) -> Graded<'s> {
        let Pair {
            criterion,
            hash,
            request,
            early,
            input,
            unit,
            labels,
        } = pair;
        // The labels go only beside a unit.
        let unit_labels = unit.and(labels).cloned();
        let (take, spent) = budget;
        let mode = effective_check_mode(criterion.mode, self.global);
        let sha = input.map(|(sha, _)| sha.as_str());
        let reused = match early {
            None => sha.and_then(|s| reusable(visit, name, &criterion.rule_id, &hash, s, unit)),
            Some(_) => None,
        };
        let answered = match (early, reused) {
            (Some(CheckOutcome::NotGraded), _) | (_, Some(_)) => None,
            _ => applicable_answer(visit, name, &criterion.rule_id, &hash, sha, unit),
        };
        let (outcome, record_from) = match (early, reused, answered, locked) {
            (Some(CheckOutcome::NotGraded), ..) => (
                CheckOutcome::NotGraded,
                Some(Consulted::unsent(CheckOutcome::NotGraded)),
            ),
            (_, Some(o), ..) => (o, None),
            (_, None, Some(a), _) => (agent_outcome(a), None),
            (Some(o), None, None, _) => (o, Some(Consulted::unsent(o))),
            (None, None, None, false) => {
                let o = CheckOutcome::Unanswered(UnansweredReason::Busy);
                (o, Some(Consulted::unsent(o)))
            }
            (None, None, None, true) => {
                let c = if take() {
                    self.consult(criterion, request)
                } else {
                    Consulted::unsent(CheckOutcome::Unanswered(spent))
                };
                (c.outcome, Some(c))
            }
        };
        // The decider's own record keeps its rule: only a veto fail from the
        // decider blocks there. An agent fail blocks the same way, only in
        // veto.
        let blocked = match answered {
            Some(a) => mode == CheckMode::Veto && a == AgentAnswer::Fail,
            None => blocks(outcome, mode),
        };
        let fallback = match (outcome, answered) {
            (CheckOutcome::Unanswered(reason), None) => Some(DeciderFallback {
                state: visit.state.clone(),
                visit_seq: visit.seq,
                gate: name.to_string(),
                rule_id: criterion.rule_id.clone(),
                declaration_hash: hash.clone(),
                mode,
                reason,
                input_sha256: input.map(|(sha, _)| sha.clone()),
                input_bytes: input.map(|(_, n)| *n),
                unit: unit.cloned(),
                unit_labels: unit_labels.clone(),
            }),
            _ => None,
        };
        let record = record_from.map(|c| {
            let (recorded, reason) = c.outcome.recorded();
            DeciderCheck {
                state: visit.state.clone(),
                visit_seq: visit.seq,
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
                input_sha256: input.map(|(sha, _)| sha.clone()),
                input_bytes: input.map(|(_, n)| *n),
                input_tokens: c.usage.input_tokens,
                output_tokens: c.usage.output_tokens,
                unread_usage_attempts: c.usage.unread_attempts,
                attempts: c.attempts,
                latency_ms: c.latency_ms,
                error_class: c.error_class,
                endpoint_origin: Some(self.endpoint_origin),
                unit: unit.cloned(),
                unit_labels,
            }
        });
        Graded {
            criterion,
            mode,
            outcome,
            agent: answered,
            blocked,
            record,
            fallback,
            unit: unit.cloned(),
        }
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
/// declaration, slice and unit (none, for a single-slice check). An
/// unanswered or not-graded outcome is never reused.
fn reusable(
    visit: &Visit,
    gate: &str,
    rule_id: &str,
    hash: &str,
    sha: &str,
    unit: Option<&UnitRef>,
) -> Option<CheckOutcome> {
    visit.events.iter().rev().find_map(|e| match &e.payload {
        EventPayload::DeciderChecked(c)
            if c.state == visit.state
                && c.gate == gate
                && c.rule_id == rule_id
                && c.declaration_hash == hash
                && c.input_sha256.as_deref() == Some(sha)
                && c.unit.as_ref() == unit =>
        {
            CheckOutcome::from_recorded_verdict(c.outcome)
        }
        _ => None,
    })
}

/// The latest answer the agent recorded in this visit for the same gate,
/// criterion, declaration and slice (DESIGN-decider-fallback.md, Decision
/// 2). Two absent slice hashes match: a failed extraction has no slice,
/// and its answer stands while extraction keeps failing. In unit mode the
/// answer must name the same unit.
fn applicable_answer(
    visit: &Visit,
    gate: &str,
    rule_id: &str,
    hash: &str,
    sha: Option<&str>,
    unit: Option<&UnitRef>,
) -> Option<AgentAnswer> {
    visit.events.iter().rev().find_map(|e| match &e.payload {
        EventPayload::DeciderCheckAnswered(a)
            if a.state == visit.state
                && a.gate == gate
                && a.rule_id == rule_id
                && a.declaration_hash == hash
                && a.input_sha256.as_deref() == sha
                && a.unit.as_ref() == unit =>
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
/// record an answer either (nor an override, which reads the log too), so
/// the state stays stopped until the log can be read again.
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
             so koto can't ask the decider or record your answer; repair the session's \
             log, then run koto next again",
            gate
        ),
        path: None,
        line: None,
        column: None,
        rule_ref: None,
        effect_landed: None,
        message_source: MessageSource::Koto,
        unit: None,
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
    unit: Option<&UnitRef>,
) -> Finding {
    let on_unit = unit
        .map(|u| format!(" on unit {}", u.key))
        .unwrap_or_default();
    let unit_flag = unit
        .map(|u| format!(" --unit {}", shell_word(&u.key)))
        .unwrap_or_default();
    Finding {
        rule_id: criterion.rule_id.clone(),
        level: FindingLevel::Warning,
        message: capped(format!(
            "criterion {}{} is waiting on your answer: the decider gave no verdict ({}). \
             Record it with `koto decider answer {} --gate {} --criterion {}{} --answer \
             <pass|fail|unclear>`, then run `koto next {}` again. {} Answer pass if: {} \
             Answer fail if: {} Answer unclear if: {}",
            criterion.rule_id,
            on_unit,
            reason.as_str(),
            shell_word(session),
            shell_word(gate),
            shell_word(&criterion.rule_id),
            unit_flag,
            shell_word(session),
            criterion.question,
            criterion.pass,
            criterion.fail,
            criterion.escape,
        )),
        path: None,
        line: None,
        column: None,
        rule_ref: Some(criterion.rule_ref.clone()),
        effect_landed: None,
        message_source: MessageSource::Koto,
        unit: unit.cloned(),
    }
}

/// `message` cut to the finding bound, never splitting a character. A unit
/// finding adds the unit's key to the template's text, so the cut is made
/// here rather than left to whoever reads the finding; a message already
/// within the bound comes back unchanged.
fn capped(mut message: String) -> String {
    let n = crate::redact::safe_cut_len(message.as_bytes(), crate::findings::MESSAGE_MAX_BYTES);
    message.truncate(n);
    message
}

/// `id` appended to `ids` unless already there, so a criterion that fails
/// on several units is listed once, in first-seen order.
fn push_once<'s>(ids: &mut Vec<&'s str>, id: &'s str) {
    if !ids.contains(&id) {
        ids.push(id);
    }
}

/// The gate's result from its criteria's outcomes (Decision 3 of
/// DESIGN-decider-fallback.md): `failed` with one finding per blocking
/// criterion when a veto criterion failed, by the decider's verdict or the
/// agent's answer; otherwise `pending`, with one warning finding per
/// criterion waiting on the agent, when any waits; otherwise `passed`. A
/// `pending` evaluation judged nothing, so it stamps no attempt and counts
/// no rule. A shadow criterion never blocks, whoever answered.
///
/// In unit mode (`units` is the list the extraction printed) every finding
/// names its unit, the criterion lists name each criterion once, and the
/// output gains `units`: one entry per printed unit, in print order, with
/// the criteria that failed it, wait on it, or were answered for it.
fn result_for(
    session: &str,
    gate: &str,
    spec: &DeciderCheckSpec,
    graded: &[Graded],
    units: Option<&[UnitRef]>,
    records: Vec<DeciderCheck>,
    duration_ms: Option<u64>,
) -> StructuredGateResult {
    let ids = |keep: &dyn Fn(&Graded) -> bool, unit: Option<&str>| -> Vec<&str> {
        let mut out = Vec::new();
        for g in graded.iter().filter(|g| keep(g)) {
            if unit.is_none() || g.unit.as_ref().map(|u| u.key.as_str()) == unit {
                push_once(&mut out, g.criterion.rule_id.as_str());
            }
        }
        out
    };
    let failed = ids(&|g| g.blocked, None);
    let unanswered = ids(
        &|g| g.mode == CheckMode::Veto && g.waiting().is_some(),
        None,
    );
    let awaiting = ids(&|g| g.waiting().is_some(), None);
    let agent_answered = ids(&|g| g.agent.is_some(), None);

    let mut findings = Vec::new();
    for g in graded {
        if g.blocked {
            let c = g.criterion;
            let (message, source) = if let Some(u) = &g.unit {
                let whose = if g.agent.is_some() {
                    ("you answered fail", MessageSource::Koto)
                } else {
                    ("the decider judged it to fail", MessageSource::Decider)
                };
                (
                    capped(format!(
                        "criterion {} failed on unit {}: {}: {}",
                        c.rule_id, u.key, whose.0, c.fail
                    )),
                    whose.1,
                )
            } else if g.agent.is_some() {
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
                unit: g.unit.clone(),
            });
        } else if let Some(reason) = g.waiting() {
            findings.push(waiting_finding(
                session,
                gate,
                g.criterion,
                reason,
                g.unit.as_ref(),
            ));
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
    let mut output = check_output(&failed, &unanswered, &awaiting, &agent_answered, "");
    if let Some(units) = units {
        output["units"] = units
            .iter()
            .map(|u| {
                let key = Some(u.key.as_str());
                serde_json::json!({
                    "key": u.key,
                    "failed": ids(&|g| g.blocked, key),
                    "awaiting": ids(&|g| g.waiting().is_some(), key),
                    "agent_answered": ids(&|g| g.agent.is_some(), key),
                })
            })
            .collect();
    }
    StructuredGateResult {
        outcome,
        output,
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
    fn the_unit_budget_is_its_own_count() {
        let units = UnitBudget::new();
        let shared = ConsultBudget::new();
        for _ in 0..MAX_UNIT_CONSULTATIONS_PER_CALL {
            assert!(units.try_take());
        }
        assert!(!units.try_take());
        assert_eq!(units.used(), MAX_UNIT_CONSULTATIONS_PER_CALL);
        assert_eq!(shared.used(), 0, "units never spend the shared cap");
    }

    #[test]
    fn units_parse_from_json_lines() {
        let out = "{\"key\":\"ac-1\",\"text\":\"one\",\"document\":\"docs/x.md\",\"section\":null,\"extra\":1}\n\
                   \n\
                   {\"key\":\"ac-2\",\"text\":\"\"}\r\n";
        let units = parse_units(out).unwrap();
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].reference.key, "ac-1");
        assert_eq!(units[0].reference.document.as_deref(), Some("docs/x.md"));
        assert_eq!(units[0].reference.section, None);
        assert_eq!(units[0].text, "one");
        assert_eq!(units[1].text, "");
        assert_eq!(parse_units("").unwrap(), vec![]);
    }

    #[test]
    fn a_bad_line_name_or_repeat_refuses_every_unit() {
        for out in [
            "not json",
            "[1]",
            "{\"text\":\"t\"}",
            "{\"key\":\"k\"}",
            "{\"key\":1,\"text\":\"t\"}",
            "{\"key\":\"-k\",\"text\":\"t\"}",
            "{\"key\":\"/k\",\"text\":\"t\"}",
            "{\"key\":\"a b\",\"text\":\"t\"}",
            "{\"key\":\"k\",\"text\":\"t\",\"section\":\"has space\"}",
            "{\"key\":\"k\",\"text\":\"t\",\"document\":7}",
            "{\"key\":\"k\",\"text\":\"t\"}\n{\"key\":\"k\",\"text\":\"u\"}",
        ] {
            assert!(parse_units(out).is_none(), "{}", out);
        }
        let long = format!("{{\"key\":\"{}\",\"text\":\"t\"}}", "k".repeat(129));
        assert!(parse_units(&long).is_none());
        let max = format!("{{\"key\":\"{}\",\"text\":\"t\"}}", "k".repeat(128));
        assert!(parse_units(&max).is_some());
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
                    summary: None,
                    rule_text: None,
                })
                .collect(),
            units: None,
            max_units: None,
            unit_document: None,
            unit_section: None,
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

    /// Every `UnansweredReason`. The match has no wildcard, so a reason
    /// added to the enum fails to compile here until it is listed, and the
    /// test below then proves it falls back like the others.
    fn every_reason() -> Vec<UnansweredReason> {
        use UnansweredReason::*;
        let all = vec![
            ProviderError,
            UnreadableResponse,
            OverBudget,
            ExtractionFailed,
            CapSpent,
            Busy,
            UnitLimit,
        ];
        for r in &all {
            match r {
                ProviderError | UnreadableResponse | OverBudget | ExtractionFailed | CapSpent
                | Busy | UnitLimit => {}
            }
        }
        all
    }

    #[test]
    fn every_unanswered_reason_falls_back_in_veto_and_in_shadow() {
        let gate = check_gate(&[("r", CheckMode::Veto)]);
        let spec = gate.decider_check.as_ref().unwrap();
        for reason in every_reason() {
            for mode in [CheckMode::Veto, CheckMode::Shadow] {
                let fallback = DeciderFallback {
                    state: "review".into(),
                    visit_seq: 2,
                    gate: "comments".into(),
                    rule_id: "r".into(),
                    declaration_hash: "h".into(),
                    mode,
                    reason,
                    input_sha256: None,
                    input_bytes: None,
                    unit: None,
                    unit_labels: None,
                };
                let graded = vec![Graded {
                    criterion: &spec.criteria[0],
                    mode,
                    outcome: CheckOutcome::Unanswered(reason),
                    agent: None,
                    blocked: false,
                    record: None,
                    fallback: Some(fallback),
                    unit: None,
                }];
                let r = result_for("wf", "comments", spec, &graded, None, Vec::new(), None);
                let label = format!("{:?} {:?}", reason, mode);
                assert_eq!(r.outcome, GateOutcome::Pending, "{}", label);
                assert_eq!(r.output["awaiting"], serde_json::json!(["r"]), "{}", label);
                let unanswered = if mode == CheckMode::Veto {
                    serde_json::json!(["r"])
                } else {
                    serde_json::json!([])
                };
                assert_eq!(r.output["unanswered"], unanswered, "{}", label);
                assert_eq!(r.decider_fallbacks.len(), 1, "{}", label);
                assert_eq!(r.findings.len(), 1, "{}", label);
                assert_eq!(r.findings[0].level, FindingLevel::Warning, "{}", label);
                assert!(
                    r.findings[0]
                        .message
                        .contains(&format!("({})", reason.as_str())),
                    "{}",
                    label
                );
            }
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
            unit: fallback.unit.clone(),
            unit_labels: fallback.unit_labels.clone(),
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
        let f = waiting_finding("wf", "comments", c, UnansweredReason::OverBudget, None);
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
