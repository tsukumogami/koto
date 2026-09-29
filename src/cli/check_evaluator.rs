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
use std::time::Instant;

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
use crate::template::decider_check::{declaration_hash, DeciderCheckSpec};
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
/// check `gate` in `state`: one per criterion the check's last output
/// (`actual_output`) lists as blocking, `candidate_false_fail` for a failed
/// one and `overridden_unanswered` for an unanswered one. Each takes its
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
    for (key, kind) in [
        ("failed", CheckOverrideKind::CandidateFalseFail),
        ("unanswered", CheckOverrideKind::OverriddenUnanswered),
    ] {
        let ids = actual_output[key].as_array().cloned().unwrap_or_default();
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
                    session, session_id, state, visit_seq, gate, rule_id, &hash, kind,
                ));
            }
        }
    }
    out
}

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
            working_dir,
            env,
        }
    }

    /// Where ledger lines go (`~/.koto`); `None` writes none.
    pub fn with_ledger_root(mut self, koto_root: Option<PathBuf>) -> Self {
        self.ledger_root = koto_root;
        self
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
        let Some(spec) = gate.decider_check.as_ref() else {
            return pass_result(Vec::new(), None);
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
            self.try_lock()
        } else {
            None
        };
        let read = self.backend.read_events_local(self.session).ok();
        let (session_id, events) = match &read {
            Some((header, events)) => (
                Some(header.session_id.as_str()).filter(|s| !s.is_empty()),
                events.as_slice(),
            ),
            None => (None, &[][..]),
        };
        let state = derive_state_from_log(events).unwrap_or_default();
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
        result_for(name, spec, &graded, records, duration_ms)
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

/// A passing result: nothing blocked.
fn pass_result(records: Vec<DeciderCheck>, duration_ms: Option<u64>) -> StructuredGateResult {
    StructuredGateResult {
        outcome: GateOutcome::Passed,
        output: crate::template::types::decider_check_default_output(),
        duration_ms,
        decider_checks: records,
        ..Default::default()
    }
}

/// The gate's result from its criteria's outcomes (Decision 5): `failed`
/// with one finding per failing veto criterion when any failed on a
/// verdict; otherwise `error` with koto's one finding for the check when a
/// veto criterion went unanswered; otherwise `passed`. Unanswered criteria
/// are listed in the output and never named by a finding: a missing
/// verdict is a checker fault, not a violation.
fn result_for(
    name: &str,
    spec: &DeciderCheckSpec,
    graded: &[Graded],
    records: Vec<DeciderCheck>,
    duration_ms: Option<u64>,
) -> StructuredGateResult {
    let failed: Vec<&Graded> = graded
        .iter()
        .filter(|g| g.blocked && g.outcome == CheckOutcome::Fail)
        .collect();
    let unanswered: Vec<&Graded> = graded
        .iter()
        .filter(|g| g.blocked && matches!(g.outcome, CheckOutcome::Unanswered(_)))
        .collect();
    if failed.is_empty() && unanswered.is_empty() {
        return pass_result(records, duration_ms);
    }
    let output = serde_json::json!({
        "failed": failed.iter().map(|g| g.rule_id.as_str()).collect::<Vec<_>>(),
        "unanswered": unanswered.iter().map(|g| g.rule_id.as_str()).collect::<Vec<_>>(),
        "error": "",
    });
    if !failed.is_empty() {
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
        return StructuredGateResult {
            outcome: GateOutcome::Failed,
            output,
            failure: Some(GateFailure {
                fallback: None,
                captured: None,
            }),
            findings,
            duration_ms,
            decider_checks: records,
            ..Default::default()
        };
    }
    let reasons: Vec<String> = unanswered
        .iter()
        .map(|g| match g.outcome {
            CheckOutcome::Unanswered(r) => format!("{} ({})", g.rule_id, r.as_str()),
            _ => g.rule_id.clone(),
        })
        .collect();
    let fallback = Finding {
        rule_id: name.to_string(),
        level: FindingLevel::Error,
        message: format!(
            "no verdict was read for {}; call koto next again, or override the check with a \
             reason if this persists",
            reasons.join(", ")
        ),
        path: None,
        line: None,
        column: None,
        rule_ref: None,
        effect_landed: None,
        message_source: MessageSource::Koto,
    };
    StructuredGateResult {
        outcome: GateOutcome::Error,
        output,
        failure: Some(GateFailure {
            fallback: Some(fallback),
            captured: None,
        }),
        findings: Vec::new(),
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
}
