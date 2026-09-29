//! The run-time half of decider checks (DESIGN-koto-decider-checks.md,
//! Decisions 3 to 6).
//!
//! [`CliCheckEvaluator`] evaluates one `decider-check` gate: it runs the
//! extraction command, bounds the redacted slice, takes the session's
//! `decider.lock` without waiting, reads the local log for the visit and
//! for verdicts it can reuse, consults the provider once per criterion (one
//! retry for a transient failure) under the per-call cap it shares with the
//! routing decider, appends a ledger `checked` line per consultation, and
//! returns a [`StructuredGateResult`] carrying the blocking findings and the
//! records the advance loop appends as `decider_checked` events.
//!
//! The pure rules (the verdict, blocking, retry, the record) live in
//! `crate::decider::check`; this module is the I/O around them.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::action::{run_shell_command, CommandEnv};
use crate::cli::decider_port::{input_sha256, DECIDER_LOCK_FILE};
use crate::decider::check::{
    blocks, build_check_request, effective_check_mode, is_retryable, reason_for_error,
    recorded_probabilities, verdict, CheckOutcome, DeciderCheck, UnansweredReason,
};
use crate::decider::ledger::{append_or_warn, LedgerRecord};
use crate::decider::types::{Decider, ErrorClass, GlobalMode, LabelledInput, SettingOrigin};
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
/// check's last output (`actual_output`) lists as failed, the only kind of
/// criterion that blocks. An unanswered criterion never blocked, so the
/// override didn't move past it and it gets none; `overridden_unanswered`
/// survives only in ledgers written before that rule. Each record takes its
/// visit and declaration hash from the latest `decider_checked` for that
/// criterion in the log; a reused verdict appended none, but the
/// consultation it reused is there. A criterion with no such record gets
/// none.
pub fn override_records(
    events: &[Event],
    state: &str,
    gate: &str,
    actual_output: &serde_json::Value,
    session: &str,
    session_id: Option<&str>,
) -> Vec<LedgerRecord> {
    use crate::decider::ledger::CheckOverrideKind;
    let mut out = Vec::new();
    let ids = actual_output["failed"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for rule_id in ids.iter().filter_map(|v| v.as_str()) {
        let latest = events.iter().rev().find_map(|e| match &e.payload {
            EventPayload::DeciderChecked(c)
                if c.state == state && c.gate == gate && c.rule_id == rule_id =>
            {
                Some((c.visit_seq, c.declaration_hash.clone()))
            }
            _ => None,
        });
        if let Some((visit_seq, hash)) = latest {
            out.push(LedgerRecord::check_overridden(
                session,
                session_id,
                state,
                visit_seq,
                gate,
                rule_id,
                &hash,
                CheckOverrideKind::CandidateFalseFail,
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

/// Held while one gate's criteria are consulted. Closing the file releases
/// the `flock`.
struct LockHold {
    _file: File,
}

/// What one consultation produced before it becomes a record.
struct Consulted {
    outcome: CheckOutcome,
    probabilities: BTreeMap<String, f64>,
    model: String,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    unread_usage_attempts: u32,
    attempts: u32,
    latency_ms: u64,
    error_class: Option<ErrorClass>,
}

impl Consulted {
    /// Add one billed answer's usage, or count it as unread.
    fn add_usage(&mut self, usage: Option<crate::decider::types::Usage>) {
        match usage {
            Some(u) => {
                self.input_tokens = Some(self.input_tokens.unwrap_or(0) + u.input_tokens);
                self.output_tokens = Some(self.output_tokens.unwrap_or(0) + u.output_tokens);
            }
            None => self.unread_usage_attempts += 1,
        }
    }
}

impl Consulted {
    /// An outcome reached without sending anything.
    fn unsent(outcome: CheckOutcome) -> Self {
        Consulted {
            outcome,
            probabilities: BTreeMap::new(),
            model: UNKNOWN_MODEL.to_string(),
            input_tokens: None,
            output_tokens: None,
            unread_usage_attempts: 0,
            attempts: 0,
            latency_ms: 0,
            error_class: None,
        }
    }
}

/// One criterion's outcome this evaluation, and its record when it wasn't a
/// reuse.
struct Graded {
    rule_id: String,
    rule_ref: String,
    fail_description: String,
    mode: CheckMode,
    outcome: CheckOutcome,
    blocked: bool,
    record: Option<DeciderCheck>,
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

    #[cfg(unix)]
    fn try_lock(&self) -> Option<LockHold> {
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::io::AsRawFd;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(
                self.backend
                    .session_dir(self.session)
                    .join(DECIDER_LOCK_FILE),
            )
            .ok()?;
        // SAFETY: `fd` is borrowed from `file`, which outlives the call.
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if ret != 0 {
            return None;
        }
        Some(LockHold { _file: file })
    }

    #[cfg(not(unix))]
    fn try_lock(&self) -> Option<LockHold> {
        let _ = OpenOptions::new;
        None
    }

    /// Evaluate the `decider-check` gate `name` (already substituted), whose
    /// criteria's declaration hashes are `hashes`, in declaration order.
    pub fn evaluate(&self, name: &str, gate: &Gate, hashes: &[String]) -> StructuredGateResult {
        // A validated template always has a spec, but a compiled one read
        // back from JSON isn't revalidated. With nothing to grade against,
        // the check passes and says why; it never blocks.
        let Some(spec) = gate.decider_check.as_ref() else {
            return pass_result(&[], MISSING_SPEC, Vec::new(), None);
        };
        let output = run_shell_command(&gate.command, self.working_dir, gate.timeout, self.env);
        self.env.record(&format!("gate '{}'", name), &output);
        let duration_ms = Some(output.duration_ms);
        let slice = output.stdout.as_str();

        // What the slice decides before anything is asked.
        let (early, input) = if output.failure_kind.is_some() {
            (
                Some(CheckOutcome::Unanswered(UnansweredReason::ExtractionFailed)),
                None,
            )
        } else if slice.trim().is_empty() {
            (Some(CheckOutcome::NotGraded), None)
        } else {
            let input = Some((
                // The labelled input every criterion's request carries.
                input_sha256(&[LabelledInput {
                    label: spec.label.clone(),
                    content: slice.to_string(),
                }]),
                slice.len() as u64,
            ));
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
            let veto: Vec<&str> = spec
                .criteria
                .iter()
                .filter(|c| effective_check_mode(c.mode, self.global) == CheckMode::Veto)
                .map(|c| c.rule_id.as_str())
                .collect();
            return pass_result(&veto, LOG_UNREADABLE, Vec::new(), duration_ms);
        };
        let visit_index = arrival_index(events, &state);
        let visit_seq = visit_index.map(|i| events[i].seq).unwrap_or(0);
        let visit_events = visit_index.map(|i| &events[i + 1..]).unwrap_or(events);

        let mut graded = Vec::with_capacity(spec.criteria.len());
        for (i, criterion) in spec.criteria.iter().enumerate() {
            let hash = hashes.get(i).cloned().unwrap_or_default();
            let mode = effective_check_mode(criterion.mode, self.global);
            let reused = input.as_ref().and_then(|(sha, _)| {
                reusable(visit_events, &state, name, &criterion.rule_id, &hash, sha)
            });
            let (outcome, record_from) = match (early, reused, &lock) {
                (Some(o), _, _) => (o, Some(Consulted::unsent(o))),
                (None, Some(o), _) => (o, None),
                (None, None, None) => {
                    let o = CheckOutcome::Unanswered(UnansweredReason::Busy);
                    (o, Some(Consulted::unsent(o)))
                }
                (None, None, Some(_)) => {
                    let c = if self.budget.try_take() {
                        self.consult(criterion, &spec.label, slice)
                    } else {
                        Consulted::unsent(CheckOutcome::Unanswered(UnansweredReason::CapSpent))
                    };
                    (c.outcome, Some(c))
                }
            };
            let blocked = blocks(outcome, mode);
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
                    input_tokens: c.input_tokens,
                    output_tokens: c.output_tokens,
                    unread_usage_attempts: c.unread_usage_attempts,
                    attempts: c.attempts,
                    latency_ms: c.latency_ms,
                    error_class: c.error_class,
                    endpoint_origin: Some(self.endpoint_origin),
                }
            });
            graded.push(Graded {
                rule_id: criterion.rule_id.clone(),
                rule_ref: criterion.rule_ref.clone(),
                fail_description: criterion.fail.clone(),
                mode,
                outcome,
                blocked,
                record,
            });
        }
        drop(lock);

        let records: Vec<DeciderCheck> = graded.iter().filter_map(|g| g.record.clone()).collect();
        for record in &records {
            let line = LedgerRecord::checked(self.session, session_id, record.clone());
            append_or_warn(self.ledger_root.as_deref(), &line);
        }
        result_for(spec, &graded, records, duration_ms)
    }

    /// Ask the provider about one criterion, retrying once on a transient
    /// failure. A consultation counts once against the cap whatever the
    /// number of attempts.
    fn consult(
        &self,
        criterion: &crate::template::decider_check::CheckCriterion,
        label: &str,
        slice: &str,
    ) -> Consulted {
        let request = build_check_request(criterion, label, slice);
        let mut c = Consulted::unsent(CheckOutcome::Unanswered(UnansweredReason::ProviderError));
        while c.attempts < MAX_ATTEMPTS {
            c.attempts += 1;
            let started = Instant::now();
            let answer = self.decider.decide(&request);
            c.latency_ms += started.elapsed().as_millis() as u64;
            match answer {
                Ok(response) => {
                    c.model = response.model.clone();
                    c.add_usage(response.usage);
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
                        c.add_usage(e.usage);
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

/// The check's output: the failed and unanswered veto criteria, and why no
/// criterion could be graded at all (empty when they could).
fn check_output(failed: &[&str], unanswered: &[&str], error: &str) -> serde_json::Value {
    serde_json::json!({
        "failed": failed,
        "unanswered": unanswered,
        "error": error,
    })
}

/// A passing result: nothing blocked. `unanswered` names the veto criteria
/// that got no verdict, so a pass that checked nothing never reads as one
/// that was checked and met.
fn pass_result(
    unanswered: &[&str],
    error: &str,
    records: Vec<DeciderCheck>,
    duration_ms: Option<u64>,
) -> StructuredGateResult {
    StructuredGateResult {
        outcome: GateOutcome::Passed,
        output: check_output(&[], unanswered, error),
        duration_ms,
        decider_checks: records,
        ..Default::default()
    }
}

/// The gate's result from its criteria's outcomes (Decision 5): `failed`
/// with one finding per failing veto criterion when any failed on a
/// verdict, otherwise `passed`. Either way the output lists the veto
/// criteria that went unanswered. A missing verdict is a checker fault,
/// not a violation: it never blocks and no finding names it.
fn result_for(
    spec: &DeciderCheckSpec,
    graded: &[Graded],
    records: Vec<DeciderCheck>,
    duration_ms: Option<u64>,
) -> StructuredGateResult {
    let failed: Vec<&Graded> = graded.iter().filter(|g| g.blocked).collect();
    let unanswered: Vec<&str> = graded
        .iter()
        .filter(|g| g.mode == CheckMode::Veto && matches!(g.outcome, CheckOutcome::Unanswered(_)))
        .map(|g| g.rule_id.as_str())
        .collect();
    if failed.is_empty() {
        return pass_result(&unanswered, "", records, duration_ms);
    }
    let failed_ids: Vec<&str> = failed.iter().map(|g| g.rule_id.as_str()).collect();
    let findings = failed
        .iter()
        .map(|g| Finding {
            rule_id: g.rule_id.clone(),
            level: FindingLevel::Error,
            message: format!(
                "criterion {} failed: the decider judged the {} to fail it: {}",
                g.rule_id, spec.label, g.fail_description
            ),
            path: None,
            line: None,
            column: None,
            rule_ref: Some(g.rule_ref.clone()),
            effect_landed: None,
            message_source: MessageSource::Decider,
        })
        .collect();
    StructuredGateResult {
        outcome: GateOutcome::Failed,
        output: check_output(&failed_ids, &unanswered, ""),
        failure: Some(GateFailure {
            fallback: None,
            captured: None,
        }),
        findings,
        duration_ms,
        decider_checks: records,
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
            serde_json::json!({"failed": [], "unanswered": [], "error": "missing_spec"})
        );
        assert!(r.decider_checks.is_empty());
        assert!(r.failure.is_none() && r.findings.is_empty());
        assert_eq!(decider.calls(), 0);
    }

    #[test]
    fn an_unreadable_log_passes_lists_veto_criteria_and_records_nothing() {
        for (mode, listed) in [
            (CheckMode::Veto, serde_json::json!(["a", "b"])),
            (CheckMode::Shadow, serde_json::json!([])),
        ] {
            let tmp = tempfile::TempDir::new().unwrap();
            let backend = backend(tmp.path(), false);
            let gate = check_gate(&[("a", mode), ("b", mode)]);
            let decider = Arc::new(ScriptedDecider::new());
            let r = evaluate(&backend, tmp.path(), &gate, &decider);
            assert_eq!(r.outcome, GateOutcome::Passed, "{:?}", mode);
            assert_eq!(
                r.output,
                serde_json::json!({"failed": [], "unanswered": listed, "error": "log_unreadable"}),
                "{:?}",
                mode
            );
            // Nothing is recorded, so nothing lands under an empty state.
            assert!(r.decider_checks.is_empty(), "{:?}", mode);
            assert!(r.failure.is_none() && r.findings.is_empty());
            assert_eq!(decider.calls(), 0, "nothing is asked without a visit");
        }
    }

    #[test]
    fn a_readable_log_records_under_its_state() {
        let tmp = tempfile::TempDir::new().unwrap();
        let backend = backend(tmp.path(), true);
        let gate = check_gate(&[("r", CheckMode::Veto)]);
        let decider = Arc::new(ScriptedDecider::new());
        // No reply queued: the provider fails twice, so no verdict.
        let r = evaluate(&backend, tmp.path(), &gate, &decider);
        assert_eq!(r.outcome, GateOutcome::Passed);
        assert_eq!(r.output["unanswered"], serde_json::json!(["r"]));
        let c = &r.decider_checks[0];
        assert_eq!(c.state, "review");
        assert_eq!(c.visit_seq, 2);
        assert!(!c.blocked);
        assert_eq!(c.reason, Some(UnansweredReason::ProviderError));
    }
}
