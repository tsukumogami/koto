//! The consultation arm of the advance loop.
//!
//! When a state with a `decider` declaration would stop for evidence, the
//! loop asks a [`DeciderPort`] whether an opted-in decider can answer
//! instead (DESIGN-jev-decision-offload.md, Decision 2). This module holds
//! the port interface, the per-value mode rule ([`effective_mode`]), the
//! visit boundary ([`visit_start_index`], [`prior_consultation`]), and the
//! pure decision of whether an answer applies.
//!
//! Nothing here does file or network I/O. Locking, reading the log under
//! the lock, assembling inputs, and calling the provider belong to the
//! port's implementation (`src/cli/decider_port.rs`); the engine only sees
//! a [`ConsultReply`].

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{Map, Value};

use crate::config::resolve::DeciderSettings;
use crate::decider::evaluate::{evaluate, EffectiveModes};
use crate::decider::record::{ConsultationOutcome, DeciderConsultation, FieldConsultation};
use crate::decider::request::{declared_fields, DeclaredField};
use crate::decider::types::{DecisionResponse, ErrorClass, GlobalMode, SettingOrigin};
use crate::engine::advance::{conditional_match_indices, AdvanceError};
use crate::engine::evidence::validate_evidence;
use crate::engine::persistence::{any_entry_index, derive_state_from_log};
use crate::engine::types::{Event, EventPayload};
use crate::template::decider::{declaration_hash, DeciderMode};
use crate::template::types::{CompiledTemplate, TemplateState};

/// Most consultations one `koto next` makes. Consultations that ended at
/// `input_unavailable` or `error` count; a [`ConsultReply::Skipped`] doesn't.
pub const MAX_CONSULTATIONS_PER_CALL: usize = 4;

/// The `source` recorded on evidence the engine applied from a decider.
pub const DECIDER_SOURCE: &str = "decider";

/// Model recorded when no response named one.
const UNKNOWN_MODEL: &str = "unknown";

// ---------------------------------------------------------------------------
// Modes
// ---------------------------------------------------------------------------

/// The mode of one declared value after every layer has had its say.
///
/// The minimum of the user mode, the project mode (absent means no project
/// limit), and the template's per-value mode, in the order
/// `off < shadow < auto`. A template `never` ranks as `shadow` for deciding
/// whether to consult, and is returned as [`DeciderMode::Never`] whenever
/// the result isn't `off`, so it is recorded as `never` and never applied.
///
/// This is the only definition of the rule. Config, the CLI, and the
/// decider module call it rather than re-deriving a mode.
pub fn effective_mode(
    user: GlobalMode,
    project: Option<GlobalMode>,
    template: DeciderMode,
) -> DeciderMode {
    fn rank_global(m: GlobalMode) -> u8 {
        match m {
            GlobalMode::Off => 0,
            GlobalMode::Shadow => 1,
            GlobalMode::Auto => 2,
        }
    }
    let template_rank = match template {
        DeciderMode::Off => 0,
        DeciderMode::Shadow | DeciderMode::Never => 1,
        DeciderMode::Auto => 2,
    };
    let rank = rank_global(user)
        .min(project.map_or(2, rank_global))
        .min(template_rank);
    match (rank, template) {
        (0, _) => DeciderMode::Off,
        (_, DeciderMode::Never) => DeciderMode::Never,
        (1, _) => DeciderMode::Shadow,
        _ => DeciderMode::Auto,
    }
}

/// The effective global mode: [`effective_mode`] with no template limit,
/// as a [`GlobalMode`]. What `resolve_decider` reports as the user's mode
/// after the project minimum.
pub fn effective_global_mode(user: GlobalMode, project: Option<GlobalMode>) -> GlobalMode {
    match effective_mode(user, project, DeciderMode::Auto) {
        DeciderMode::Off => GlobalMode::Off,
        DeciderMode::Shadow | DeciderMode::Never => GlobalMode::Shadow,
        DeciderMode::Auto => GlobalMode::Auto,
    }
}

/// The user and project modes a port consults under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeciderPolicy {
    /// `KOTO_DECIDER` or user `decider.mode`.
    pub user: GlobalMode,
    /// Project `decider.mode`; `None` when the project sets none.
    pub project: Option<GlobalMode>,
}

impl DeciderPolicy {
    pub fn from_settings(settings: &DeciderSettings) -> Self {
        DeciderPolicy {
            user: settings.user_mode(),
            project: settings.project_mode(),
        }
    }

    /// [`effective_mode`] for a template value under this policy.
    pub fn mode_for(&self, template: DeciderMode) -> DeciderMode {
        effective_mode(self.user, self.project, template)
    }
}

// ---------------------------------------------------------------------------
// The port
// ---------------------------------------------------------------------------

/// Holds whatever serializes a consultation (the CLI's `decider.lock`)
/// until the engine drops it. The engine drops it only after appending
/// `decider_consulted`, any decider evidence, and the `transitioned` event.
pub struct VisitGuard {
    _held: Option<Box<dyn std::any::Any>>,
}

impl VisitGuard {
    /// A guard that releases `held` when dropped.
    pub fn new<T: 'static>(held: T) -> Self {
        VisitGuard {
            _held: Some(Box::new(held)),
        }
    }

    /// A guard that holds nothing.
    pub fn none() -> Self {
        VisitGuard { _held: None }
    }
}

impl std::fmt::Debug for VisitGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VisitGuard")
    }
}

/// What the engine asks the port to consult.
#[derive(Debug, Clone, Copy)]
pub struct ConsultRequest<'a> {
    /// The state that would stop for evidence.
    pub state: &'a str,
    pub template_state: &'a TemplateState,
    /// The state's declared fields, in declaration order.
    pub fields: &'a [DeclaredField<'a>],
}

/// What the provider call produced.
#[derive(Debug, Clone, PartialEq)]
pub enum ConsultResult {
    /// A declared input was unset or over its byte budget; nothing was sent.
    InputUnavailable,
    /// The provider call failed.
    Failed(ErrorClass),
    /// The provider answered. The engine still evaluates the answer.
    Answered(DecisionResponse),
}

/// A consultation the port made, with the lock that makes it the visit's
/// only one.
#[derive(Debug)]
pub struct Consultation {
    /// The seq of the event that began the visit.
    pub visit_seq: u64,
    pub provider: String,
    /// SHA-256 of the assembled inputs; `None` when they couldn't be
    /// assembled.
    pub input_sha256: Option<String>,
    pub latency_ms: u64,
    pub directive_bytes: u64,
    pub endpoint_origin: SettingOrigin,
    pub result: ConsultResult,
    /// The visit moved on while the provider call was in flight: the
    /// port's re-read after the call (still under the lock) found the
    /// session out of the state, in a later visit to it, or holding
    /// evidence for a declared field. The consultation is still recorded,
    /// once, but never applied.
    pub superseded: bool,
    pub guard: VisitGuard,
}

/// The port's answer to [`DeciderPort::consult`].
#[derive(Debug)]
pub enum ConsultReply {
    /// Nothing was consulted: the lock was taken by another process, the
    /// visit already has a consultation, the session left the state, or
    /// the log couldn't be read. The engine records nothing, doesn't count
    /// it against the cap, and returns the opted-out response.
    Skipped,
    Consulted(Box<Consultation>),
}

/// The engine's view of an opted-in decider.
///
/// A port exists only when `handle_next` built one, and it builds one only
/// when `DeciderSettings::opted_in()` holds, so a tick with no port makes
/// no consultation, takes no lock, and makes no network call.
pub trait DeciderPort {
    /// The user and project modes to consult under.
    fn policy(&self) -> &DeciderPolicy;

    /// Consult for the visit `req` names, or say why not.
    fn consult(&mut self, req: &ConsultRequest<'_>) -> ConsultReply;

    /// Called exactly once after each `decider_consulted` event is durably
    /// appended, while the visit's guard is still held.
    fn recorded(&mut self, _event: &EventPayload) {}
}

// ---------------------------------------------------------------------------
// The visit boundary
// ---------------------------------------------------------------------------

/// Where the current visit to a state began.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisitStart {
    /// Position of the entry event in the slice given.
    pub index: usize,
    /// That event's seq: the `visit_seq` a consultation is keyed on.
    pub seq: u64,
}

/// The event that began the current visit to `state`, or `None` when the
/// session isn't in `state` (it has left, or never entered).
///
/// A visit begins at the last state-entry event naming the state: a
/// transition (initialization's `from: null` included), a directed
/// transition, or a rewind, a self-loop included. That is the
/// `Boundary::AnyEntry` rule in `engine::persistence`, read from the same
/// scan.
pub fn visit_start_index(events: &[Event], state: &str) -> Option<VisitStart> {
    if derive_state_from_log(events).as_deref() != Some(state) {
        return None;
    }
    let index = any_entry_index(events, state)?;
    Some(VisitStart {
        index,
        seq: events[index].seq,
    })
}

/// The consultation already recorded for the current visit to `state`, if
/// any. `None` when there is none or the session isn't in `state`.
pub fn prior_consultation<'a>(events: &'a [Event], state: &str) -> Option<&'a DeciderConsultation> {
    let start = visit_start_index(events, state)?;
    events[start.index + 1..]
        .iter()
        .find_map(|e| match &e.payload {
            EventPayload::DeciderConsulted(c) if c.state == state && c.visit_seq == start.seq => {
                Some(c)
            }
            _ => None,
        })
}

/// Whether the visit to `state` that began at `visit_seq` is still the
/// current one and still waiting on its declared fields.
///
/// False when the session isn't in `state`, when a later entry began a new
/// visit, or when an `evidence_submitted` event in the visit carries any
/// of `fields`. The port asks this after the provider call returns, so an
/// answer never lands on top of a submission made while it was in flight.
pub fn visit_still_open(
    events: &[Event],
    state: &str,
    visit_seq: u64,
    fields: &[DeclaredField<'_>],
) -> bool {
    let Some(start) = visit_start_index(events, state) else {
        return false;
    };
    if start.seq != visit_seq {
        return false;
    }
    !events[start.index + 1..].iter().any(|e| match &e.payload {
        EventPayload::EvidenceSubmitted {
            state: s,
            fields: submitted,
            ..
        } => s == state && fields.iter().any(|f| submitted.contains_key(f.name)),
        _ => false,
    })
}

// ---------------------------------------------------------------------------
// The arm
// ---------------------------------------------------------------------------

/// What the loop knows at a stop that the arm needs.
pub(crate) struct StopContext<'a> {
    pub state: &'a str,
    pub template: &'a CompiledTemplate,
    pub template_state: &'a TemplateState,
    /// Evidence the agent submitted for this visit (empty for a state the
    /// loop auto-advanced into).
    pub agent_evidence: &'a BTreeMap<String, Value>,
    pub gates_failed: bool,
    /// The resolver's evidence map: agent evidence plus gate output.
    pub evidence_value: &'a Value,
    pub variables: &'a HashMap<String, String>,
    /// States this call has already advanced into.
    pub visited: &'a HashSet<String>,
}

/// What the arm decided.
pub(crate) enum StopOutcome {
    /// No consultation was recorded; stop as an opted-out user would.
    NotConsulted,
    /// A consultation was recorded but not applied; stop as an opted-out
    /// user would.
    NotApplied,
    /// Decider evidence was appended; take the transition to `target`, then
    /// drop `guard`. `edge` is the index of the matched transition and
    /// `evidence` the agent's evidence with the decider's answer merged in,
    /// so the edge's `context_assignments` resolve against what picked it.
    Apply {
        target: String,
        edge: usize,
        evidence: BTreeMap<String, Value>,
        guard: VisitGuard,
    },
}

/// The effective mode of each declared value, per field.
fn field_modes(
    policy: &DeciderPolicy,
    fields: &[DeclaredField<'_>],
) -> BTreeMap<String, BTreeMap<String, DeciderMode>> {
    fields
        .iter()
        .map(|f| {
            let modes = f
                .decider
                .answers
                .iter()
                .map(|(value, answer)| (value.clone(), policy.mode_for(answer.mode)))
                .collect();
            (f.name.to_string(), modes)
        })
        .collect()
}

/// Consult at a stop, if this stop is eligible, and record the result.
///
/// Eligible means: the state accepts evidence and declares at least one
/// field, its gates passed, the call is under its cap, some declared value
/// isn't `off`, and the agent's evidence holds no declared field. The port
/// then decides whether this visit has already been consulted.
pub(crate) fn consult_at_stop<F>(
    port: &mut dyn DeciderPort,
    ctx: &StopContext<'_>,
    consultations: &mut usize,
    append_event: &mut F,
) -> Result<StopOutcome, AdvanceError>
where
    F: FnMut(&EventPayload) -> Result<(), String>,
{
    let Some(accepts) = &ctx.template_state.accepts else {
        return Ok(StopOutcome::NotConsulted);
    };
    if ctx.gates_failed || *consultations >= MAX_CONSULTATIONS_PER_CALL {
        return Ok(StopOutcome::NotConsulted);
    }
    let fields = declared_fields(accepts);
    if fields.is_empty()
        || fields
            .iter()
            .any(|f| ctx.agent_evidence.contains_key(f.name))
    {
        return Ok(StopOutcome::NotConsulted);
    }
    let modes = field_modes(port.policy(), &fields);
    if modes
        .values()
        .flat_map(|m| m.values())
        .all(|m| *m == DeciderMode::Off)
    {
        return Ok(StopOutcome::NotConsulted);
    }

    let consultation = match port.consult(&ConsultRequest {
        state: ctx.state,
        template_state: ctx.template_state,
        fields: &fields,
    }) {
        ConsultReply::Skipped => return Ok(StopOutcome::NotConsulted),
        ConsultReply::Consulted(c) => *c,
    };
    *consultations += 1;

    let Consultation {
        visit_seq,
        provider,
        input_sha256,
        latency_ms,
        directive_bytes,
        endpoint_origin,
        result,
        superseded,
        guard,
    } = consultation;

    let hashes: BTreeMap<&str, String> = fields
        .iter()
        .map(|f| (f.name, declaration_hash(f.decider, f.description)))
        .collect();
    let unevaluated = || -> BTreeMap<String, FieldConsultation> {
        fields
            .iter()
            .map(|f| {
                (
                    f.name.to_string(),
                    FieldConsultation::unevaluated(hashes[f.name].clone(), modes[f.name].clone()),
                )
            })
            .collect()
    };

    let mut model = UNKNOWN_MODEL.to_string();
    let mut apply: Option<(Map<String, Value>, (usize, String))> = None;
    let (mut outcome, error_class, recorded_fields) = match result {
        ConsultResult::InputUnavailable => {
            (ConsultationOutcome::InputUnavailable, None, unevaluated())
        }
        ConsultResult::Failed(class) => (ConsultationOutcome::Error, Some(class), unevaluated()),
        ConsultResult::Answered(response) => {
            model = response.model.clone();
            let mut eff = EffectiveModes::new();
            for (field, values) in &modes {
                for (value, mode) in values {
                    eff.set(field.clone(), value.clone(), *mode);
                }
            }
            match evaluate(&fields, &response, &eff) {
                Err(e) => (ConsultationOutcome::Error, Some(e.class), unevaluated()),
                Ok(evaluation) => {
                    let recorded = fields
                        .iter()
                        .zip(&evaluation.fields)
                        .map(|(f, eval)| {
                            (
                                f.name.to_string(),
                                FieldConsultation::evaluated(
                                    hashes[f.name].clone(),
                                    modes[f.name].clone(),
                                    eval,
                                ),
                            )
                        })
                        .collect();
                    apply = evaluation.candidate.and_then(|candidate| {
                        applicable_target(ctx, accepts, &candidate).map(|t| (candidate, t))
                    });
                    let outcome = if apply.is_some() {
                        ConsultationOutcome::Applied
                    } else {
                        ConsultationOutcome::NotApplied
                    };
                    (outcome, None, recorded)
                }
            }
        }
    };

    // A visit that moved on during the call keeps its record but never
    // gets the answer applied.
    if superseded {
        apply = None;
        if outcome == ConsultationOutcome::Applied {
            outcome = ConsultationOutcome::NotApplied;
        }
    }

    let payload = EventPayload::DeciderConsulted(DeciderConsultation {
        state: ctx.state.to_string(),
        visit_seq,
        provider,
        model,
        input_sha256,
        outcome,
        error_class,
        latency_ms,
        directive_bytes,
        endpoint_origin,
        fields: recorded_fields,
    });
    // A failed append falls back to the opted-out stop: the decider never
    // turns a tick into an error.
    if append_event(&payload).is_err() {
        return Ok(StopOutcome::NotApplied);
    }
    port.recorded(&payload);

    let Some((candidate, (edge, target))) = apply else {
        drop(guard);
        return Ok(StopOutcome::NotApplied);
    };
    let mut evidence = ctx.agent_evidence.clone();
    for (k, v) in &candidate {
        evidence.insert(k.clone(), v.clone());
    }
    let submitted = EventPayload::EvidenceSubmitted {
        state: ctx.state.to_string(),
        fields: candidate.into_iter().collect(),
        submitter_cwd: None,
        source: Some(DECIDER_SOURCE.to_string()),
    };
    if append_event(&submitted).is_err() {
        return Ok(StopOutcome::NotApplied);
    }
    Ok(StopOutcome::Apply {
        target,
        edge,
        evidence,
        guard,
    })
}

/// The transition an all-qualified answer may take, as its index and
/// target, or `None` when it may not be applied.
///
/// The candidate must pass `validate_evidence` alongside the agent's own
/// evidence (so an escape or undeclared value can never be written), must
/// match exactly one conditional transition, that transition's target must
/// not already be visited in this call, and the transition must clear the
/// runtime floor: not terminal, not confirmation-guarded, and no `gates.*`
/// key in its `when`. The floor is checked on the transition actually
/// matched, which covers routes that reach the field only through
/// `evidence.*` or `vars.*` keys the compiler couldn't attribute.
fn applicable_target(
    ctx: &StopContext<'_>,
    accepts: &BTreeMap<String, crate::template::types::FieldSchema>,
    candidate: &Map<String, Value>,
) -> Option<(usize, String)> {
    let mut submission: Map<String, Value> = ctx
        .agent_evidence
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (k, v) in candidate {
        submission.insert(k.clone(), v.clone());
    }
    validate_evidence(&Value::Object(submission), accepts).ok()?;

    let mut merged = ctx.evidence_value.as_object().cloned().unwrap_or_default();
    for (k, v) in candidate {
        merged.insert(k.clone(), v.clone());
    }
    let matches =
        conditional_match_indices(ctx.template_state, &Value::Object(merged), ctx.variables);
    let [index] = matches.as_slice() else {
        return None;
    };
    let transition = &ctx.template_state.transitions[*index];
    if ctx.visited.contains(&transition.target) {
        return None;
    }
    if !ctx
        .template
        .transition_floor_violations(transition)
        .is_empty()
    {
        return None;
    }
    Some((*index, transition.target.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::Event;

    // -- effective_mode ---------------------------------------------------

    use DeciderMode as D;
    use GlobalMode as G;

    #[test]
    fn per_value_mode_is_the_minimum_of_the_three() {
        // User lowest.
        assert_eq!(effective_mode(G::Shadow, Some(G::Auto), D::Auto), D::Shadow);
        assert_eq!(effective_mode(G::Off, Some(G::Auto), D::Auto), D::Off);
        // Project lowest.
        assert_eq!(effective_mode(G::Auto, Some(G::Shadow), D::Auto), D::Shadow);
        assert_eq!(effective_mode(G::Auto, Some(G::Off), D::Auto), D::Off);
        // Template lowest.
        assert_eq!(effective_mode(G::Auto, Some(G::Auto), D::Shadow), D::Shadow);
        assert_eq!(effective_mode(G::Auto, Some(G::Auto), D::Off), D::Off);
        // All auto, and no project limit.
        assert_eq!(effective_mode(G::Auto, Some(G::Auto), D::Auto), D::Auto);
        assert_eq!(effective_mode(G::Auto, None, D::Auto), D::Auto);
    }

    #[test]
    fn never_is_consulted_as_shadow_and_recorded_as_never() {
        // Under user and project auto: consulted (not off), recorded never.
        assert_eq!(effective_mode(G::Auto, Some(G::Auto), D::Never), D::Never);
        assert_eq!(effective_mode(G::Auto, None, D::Never), D::Never);
        assert_eq!(effective_mode(G::Shadow, None, D::Never), D::Never);
        // Off anywhere still wins.
        assert_eq!(effective_mode(G::Off, None, D::Never), D::Off);
        assert_eq!(effective_mode(G::Auto, Some(G::Off), D::Never), D::Off);
    }

    #[test]
    fn effective_global_mode_is_the_user_project_minimum() {
        assert_eq!(effective_global_mode(G::Auto, None), G::Auto);
        assert_eq!(effective_global_mode(G::Auto, Some(G::Shadow)), G::Shadow);
        assert_eq!(effective_global_mode(G::Shadow, Some(G::Auto)), G::Shadow);
        assert_eq!(effective_global_mode(G::Auto, Some(G::Off)), G::Off);
        assert_eq!(effective_global_mode(G::Off, Some(G::Auto)), G::Off);
    }

    // -- visit boundary ---------------------------------------------------

    fn ev(seq: u64, payload: EventPayload) -> Event {
        Event {
            seq,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: payload.type_name().to_string(),
            payload,
            idempotency_hash: None,
        }
    }

    fn init() -> EventPayload {
        EventPayload::WorkflowInitialized {
            template_path: "t.json".to_string(),
            variables: Default::default(),
            spawn_entry: None,
        }
    }

    fn tr(from: Option<&str>, to: &str) -> EventPayload {
        EventPayload::Transitioned {
            from: from.map(str::to_string),
            to: to.to_string(),
            condition_type: "auto".to_string(),
            skip_if_matched: None,
            context_assignments: None,
        }
    }

    fn consulted(state: &str, visit_seq: u64) -> EventPayload {
        EventPayload::DeciderConsulted(DeciderConsultation {
            state: state.to_string(),
            visit_seq,
            provider: "fake".to_string(),
            model: "m".to_string(),
            input_sha256: None,
            outcome: ConsultationOutcome::NotApplied,
            error_class: None,
            latency_ms: 1,
            directive_bytes: 1,
            endpoint_origin: SettingOrigin::Default,
            fields: BTreeMap::new(),
        })
    }

    fn evidence(state: &str) -> EventPayload {
        EventPayload::EvidenceSubmitted {
            state: state.to_string(),
            fields: Default::default(),
            submitter_cwd: None,
            source: None,
        }
    }

    /// Named logs covering each way a visit can begin, and one that has
    /// left the state.
    fn logs() -> Vec<(&'static str, Vec<Event>, &'static str, Option<u64>)> {
        vec![
            (
                "initialization",
                vec![ev(1, init()), ev(2, tr(None, "review"))],
                "review",
                Some(2),
            ),
            (
                "self-loop",
                vec![
                    ev(1, init()),
                    ev(2, tr(None, "review")),
                    ev(3, consulted("review", 2)),
                    ev(4, tr(Some("review"), "review")),
                ],
                "review",
                Some(4),
            ),
            (
                "rewind",
                vec![
                    ev(1, init()),
                    ev(2, tr(None, "review")),
                    ev(3, consulted("review", 2)),
                    ev(4, tr(Some("review"), "work")),
                    ev(
                        5,
                        EventPayload::Rewound {
                            from: "work".to_string(),
                            to: "review".to_string(),
                            rationale: None,
                        },
                    ),
                ],
                "review",
                Some(5),
            ),
            (
                "directed",
                vec![
                    ev(1, init()),
                    ev(2, tr(None, "gather")),
                    ev(
                        3,
                        EventPayload::DirectedTransition {
                            from: "gather".to_string(),
                            to: "review".to_string(),
                            rationale: None,
                        },
                    ),
                    ev(4, evidence("review")),
                ],
                "review",
                Some(3),
            ),
            (
                "left the state",
                vec![
                    ev(1, init()),
                    ev(2, tr(None, "review")),
                    ev(3, consulted("review", 2)),
                    ev(4, tr(Some("review"), "work")),
                ],
                "review",
                None,
            ),
        ]
    }

    #[test]
    fn visit_start_index_for_each_entry_kind() {
        for (name, events, state, want) in logs() {
            assert_eq!(
                visit_start_index(&events, state).map(|v| v.seq),
                want,
                "{}",
                name
            );
        }
    }

    #[test]
    fn visit_start_agrees_with_the_any_entry_boundary() {
        use crate::engine::persistence::epoch_slice_for_test;
        for (name, events, state, want) in logs() {
            let Some(_) = want else { continue };
            let start = visit_start_index(&events, state).unwrap();
            let slice = epoch_slice_for_test(&events, state);
            assert_eq!(
                events.len() - slice.len(),
                start.index + 1,
                "{}: visit start and the AnyEntry boundary disagree",
                name
            );
        }
    }

    #[test]
    fn prior_consultation_is_scoped_to_the_visit() {
        let expect = [
            ("initialization", None),
            ("self-loop", None),
            ("rewind", None),
            ("directed", None),
            ("left the state", None),
        ];
        for ((name, events, state, _), (_, want)) in logs().into_iter().zip(expect) {
            assert_eq!(
                prior_consultation(&events, state).map(|c| c.visit_seq),
                want,
                "{}",
                name
            );
        }

        // Consulted in the current visit.
        let events = vec![
            ev(1, init()),
            ev(2, tr(None, "review")),
            ev(3, consulted("review", 2)),
        ];
        let c = prior_consultation(&events, "review").unwrap();
        assert_eq!(c.visit_seq, 2);
        assert_eq!(c.outcome, ConsultationOutcome::NotApplied);

        // A record for the right state but another visit doesn't count.
        let events = vec![
            ev(1, init()),
            ev(2, tr(None, "review")),
            ev(3, consulted("review", 1)),
        ];
        assert!(prior_consultation(&events, "review").is_none());
    }

    // -- the arm, through the loop with a fake port -----------------------

    use crate::decider::types::Answer;
    use crate::engine::advance::{
        advance_until_stop_with_decider, ActionResult, AdvanceResult, IntegrationError, StopReason,
    };
    use crate::engine::substitute::{GateCaptureRefusal, VariableOverlay};
    use crate::gate::{GateOutcome, StructuredGateResult};
    use crate::template::types::{ActionDecl, Gate};
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::io::Write as _;
    use std::rc::Rc;
    use std::sync::atomic::AtomicBool;

    /// Compile a template from source.
    fn compile_src(src: &str) -> CompiledTemplate {
        let mut f = tempfile::Builder::new().suffix(".md").tempfile().unwrap();
        f.write_all(src.as_bytes()).unwrap();
        // Not strict, so a test state may carry a legacy pass/block gate.
        crate::template::compile::compile(f.path(), false)
            .unwrap_or_else(|e| panic!("compile: {:#}\n{}", e, src))
    }

    const DECIDER: &str = r#"{answers: {go: {description: "Yes.", mode: GO_MODE}, hold: {description: "No.", mode: auto}}, escape: {value: unclear, description: "Unknown."}, inputs: [{var: NOTE, label: note}]}"#;

    /// Declared states `(name, go_target)`, each with `verdict` (`go` in
    /// `go_mode`, `hold` in auto) routing `go` to its target and `hold` to
    /// `parked`. `last` and `parked` are undeclared, non-terminal stops.
    fn chain_with(states: &[(&str, &str)], go_mode: &str, gates: &str) -> CompiledTemplate {
        let mut src = format!(
            "---\nname: chain\nversion: \"1.0\"\ninitial_state: {}\nvariables:\n  NOTE:\n    description: n\n    default: x\nstates:\n",
            states[0].0
        );
        for (name, target) in states {
            src.push_str(&format!(
                "  {name}:\n{gates}    accepts:\n      verdict:\n        type: enum\n        values: [go, hold]\n        required: true\n        description: \"Go on?\"\n        decider: {decider}\n    transitions:\n      - target: {target}\n        when:\n          verdict: go\n      - target: parked\n        when:\n          verdict: hold\n",
                decider = DECIDER.replace("GO_MODE", go_mode),
            ));
        }
        src.push_str(
            "  parked:\n    accepts:\n      ok:\n        type: boolean\n        required: true\n        description: ok\n    transitions:\n      - target: done\n        when:\n          ok: true\n  last:\n    accepts:\n      ok:\n        type: boolean\n        required: true\n        description: ok\n    transitions:\n      - target: done\n        when:\n          ok: true\n  done:\n    terminal: true\n---\n",
        );
        for (name, _) in states {
            src.push_str(&format!("\n## {}\n\n{} directive\n", name, name));
        }
        src.push_str("\n## parked\n\np\n\n## last\n\nl\n\n## done\n\nd\n");
        compile_src(&src)
    }

    fn chain(states: &[(&str, &str)]) -> CompiledTemplate {
        chain_with(states, "auto", "")
    }

    enum Scripted {
        Skip,
        Input,
        Fail(ErrorClass),
        Answer(f64, f64, f64),
    }

    /// A fake port: scripted replies, a call counter, and guards that
    /// record how many events had been appended when they were dropped.
    struct FakePort {
        policy: DeciderPolicy,
        replies: VecDeque<Scripted>,
        calls: usize,
        appended: Rc<Cell<usize>>,
        drops: Rc<RefCell<Vec<usize>>>,
        /// `(event count at the call, guard dropped yet)` per `recorded`.
        recorded: Vec<(usize, usize)>,
        /// Report every consultation as superseded (the visit moved on
        /// during the call).
        superseded: bool,
    }

    struct DropProbe {
        appended: Rc<Cell<usize>>,
        drops: Rc<RefCell<Vec<usize>>>,
    }

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.drops.borrow_mut().push(self.appended.get());
        }
    }

    impl FakePort {
        fn new(replies: Vec<Scripted>) -> Self {
            Self::with_policy(replies, GlobalMode::Auto, None)
        }

        fn with_policy(
            replies: Vec<Scripted>,
            user: GlobalMode,
            project: Option<GlobalMode>,
        ) -> Self {
            FakePort {
                policy: DeciderPolicy { user, project },
                replies: replies.into(),
                calls: 0,
                appended: Rc::new(Cell::new(0)),
                drops: Rc::new(RefCell::new(Vec::new())),
                recorded: Vec::new(),
                superseded: false,
            }
        }
    }

    impl DeciderPort for FakePort {
        fn policy(&self) -> &DeciderPolicy {
            &self.policy
        }

        fn consult(&mut self, _req: &ConsultRequest<'_>) -> ConsultReply {
            self.calls += 1;
            let result = match self.replies.pop_front().expect("unscripted consult") {
                Scripted::Skip => return ConsultReply::Skipped,
                Scripted::Input => ConsultResult::InputUnavailable,
                Scripted::Fail(c) => ConsultResult::Failed(c),
                Scripted::Answer(go, hold, unclear) => {
                    let mut answers = BTreeMap::new();
                    answers.insert(
                        "verdict".to_string(),
                        Answer::Choice {
                            probabilities: [
                                ("go".to_string(), go),
                                ("hold".to_string(), hold),
                                ("unclear".to_string(), unclear),
                            ]
                            .into_iter()
                            .collect(),
                            provider_confidence: None,
                        },
                    );
                    ConsultResult::Answered(DecisionResponse {
                        model: "fake-1".to_string(),
                        answers,
                    })
                }
            };
            ConsultReply::Consulted(Box::new(Consultation {
                visit_seq: 1,
                provider: "fake".to_string(),
                input_sha256: None,
                latency_ms: 3,
                directive_bytes: 10,
                endpoint_origin: SettingOrigin::Default,
                result,
                superseded: self.superseded,
                guard: VisitGuard::new(DropProbe {
                    appended: Rc::clone(&self.appended),
                    drops: Rc::clone(&self.drops),
                }),
            }))
        }

        fn recorded(&mut self, event: &EventPayload) {
            assert!(matches!(event, EventPayload::DeciderConsulted(_)));
            self.recorded
                .push((self.appended.get(), self.drops.borrow().len()));
        }
    }

    const GO: Scripted = Scripted::Answer(0.95, 0.03, 0.02);

    fn run(
        tpl: &CompiledTemplate,
        start: &str,
        evidence: BTreeMap<String, Value>,
        port: Option<&mut FakePort>,
        gates_fail: bool,
    ) -> (AdvanceResult, Vec<EventPayload>) {
        let log: RefCell<Vec<EventPayload>> = RefCell::new(Vec::new());
        let counter = port.as_ref().map(|p| Rc::clone(&p.appended));
        let mut append = |p: &EventPayload| -> Result<(), String> {
            log.borrow_mut().push(p.clone());
            if let Some(c) = &counter {
                c.set(c.get() + 1);
            }
            Ok(())
        };
        let gates = |g: &BTreeMap<String, Gate>| -> Result<
            BTreeMap<String, StructuredGateResult>,
            GateCaptureRefusal,
        > {
            Ok(g.keys()
                .map(|k| {
                    (
                        k.clone(),
                        StructuredGateResult {
                            outcome: if gates_fail {
                                GateOutcome::Failed
                            } else {
                                GateOutcome::Passed
                            },
                            output: serde_json::json!({"exit_code": if gates_fail { 1 } else { 0 }, "error": ""}),
                        },
                    )
                })
                .collect())
        };
        let integration =
            |_: &str| -> Result<Value, IntegrationError> { Err(IntegrationError::Unavailable) };
        let action = |_: &str, _: &ActionDecl, _: bool| ActionResult::Skipped;
        let overlay = VariableOverlay::new();
        let shutdown = AtomicBool::new(false);
        let result = advance_until_stop_with_decider(
            start,
            tpl,
            &evidence,
            &[],
            &mut append,
            &gates,
            &integration,
            &action,
            &overlay,
            &shutdown,
            port.map(|p| p as &mut dyn DeciderPort),
        )
        .expect("advance");
        let events = log.into_inner();
        (result, events)
    }

    fn names(events: &[EventPayload]) -> Vec<&'static str> {
        events.iter().map(|e| e.type_name()).collect()
    }

    fn consultation(events: &[EventPayload]) -> &DeciderConsultation {
        events
            .iter()
            .find_map(|e| match e {
                EventPayload::DeciderConsulted(c) => Some(c),
                _ => None,
            })
            .expect("a decider_consulted event")
    }

    fn is_evidence_required(r: &AdvanceResult) -> bool {
        matches!(r.stop_reason, StopReason::EvidenceRequired { .. })
    }

    #[test]
    fn applied_answer_appends_in_order_and_releases_the_guard_last() {
        let tpl = chain(&[("s1", "last")]);
        let mut port = FakePort::new(vec![GO]);
        let (r, events) = run(&tpl, "s1", BTreeMap::new(), Some(&mut port), false);
        assert_eq!(r.final_state, "last");
        assert!(r.advanced);
        assert!(is_evidence_required(&r));
        assert_eq!(
            names(&events),
            vec!["decider_consulted", "evidence_submitted", "transitioned"]
        );
        assert_eq!(consultation(&events).outcome, ConsultationOutcome::Applied);
        match &events[1] {
            EventPayload::EvidenceSubmitted { fields, source, .. } => {
                assert_eq!(source.as_deref(), Some("decider"));
                assert_eq!(fields["verdict"], serde_json::json!("go"));
            }
            other => panic!("{:?}", other),
        }
        match &events[2] {
            EventPayload::Transitioned { condition_type, .. } => {
                assert_eq!(condition_type, "auto")
            }
            other => panic!("{:?}", other),
        }
        // Guard dropped only after all three appends.
        assert_eq!(*port.drops.borrow(), vec![3]);
        // `recorded` ran once, right after the first append, guard held.
        assert_eq!(port.recorded, vec![(1, 0)]);
    }

    #[test]
    fn not_applied_releases_the_guard_after_the_record() {
        let tpl = chain(&[("s1", "last")]);
        let mut port = FakePort::new(vec![Scripted::Answer(0.6, 0.3, 0.1)]);
        let (r, events) = run(&tpl, "s1", BTreeMap::new(), Some(&mut port), false);
        assert_eq!(r.final_state, "s1");
        assert!(!r.advanced);
        assert!(is_evidence_required(&r));
        assert_eq!(names(&events), vec!["decider_consulted"]);
        let c = consultation(&events);
        assert_eq!(c.outcome, ConsultationOutcome::NotApplied);
        assert_eq!(
            c.fields["verdict"].outcome,
            Some(crate::decider::FieldOutcome::BelowThreshold)
        );
        assert_eq!(*port.drops.borrow(), vec![1]);
        assert_eq!(port.recorded, vec![(1, 0)]);
    }

    #[test]
    fn a_superseded_consultation_is_recorded_once_and_never_applied() {
        // GO would apply; the port reports the visit moved on meanwhile.
        let tpl = chain(&[("s1", "last")]);
        let mut port = FakePort::new(vec![GO]);
        port.superseded = true;
        let (r, events) = run(&tpl, "s1", BTreeMap::new(), Some(&mut port), false);
        assert_eq!(r.final_state, "s1");
        assert!(!r.advanced);
        assert!(is_evidence_required(&r));
        assert_eq!(names(&events), vec!["decider_consulted"]);
        let c = consultation(&events);
        assert_eq!(c.outcome, ConsultationOutcome::NotApplied);
        // The answer is still evaluated, so the record can be paired.
        assert_eq!(
            c.fields["verdict"].outcome,
            Some(crate::decider::FieldOutcome::Qualified)
        );
        assert_eq!(port.calls, 1);
        assert_eq!(*port.drops.borrow(), vec![1]);
        assert_eq!(port.recorded, vec![(1, 0)]);

        // A superseded failure keeps its own outcome.
        let mut port = FakePort::new(vec![Scripted::Fail(ErrorClass::Timeout)]);
        port.superseded = true;
        let (_, events) = run(&tpl, "s1", BTreeMap::new(), Some(&mut port), false);
        assert_eq!(names(&events), vec!["decider_consulted"]);
        assert_eq!(consultation(&events).outcome, ConsultationOutcome::Error);
    }

    #[test]
    fn visit_still_open_follows_the_visit_and_its_declared_evidence() {
        let tpl = chain(&[("s1", "last")]);
        let accepts = tpl.states["s1"].accepts.as_ref().unwrap();
        let fields = declared_fields(accepts);
        let with = |extra: Vec<EventPayload>| -> Vec<Event> {
            let mut out = vec![ev(1, init()), ev(2, tr(None, "s1"))];
            for (i, p) in extra.into_iter().enumerate() {
                out.push(ev(3 + i as u64, p));
            }
            out
        };
        let declared = |state: &str, field: &str| EventPayload::EvidenceSubmitted {
            state: state.to_string(),
            fields: [(field.to_string(), serde_json::json!("go"))]
                .into_iter()
                .collect(),
            submitter_cwd: None,
            source: None,
        };

        assert!(visit_still_open(&with(vec![]), "s1", 2, &fields));
        // Undeclared evidence, or declared evidence for another state,
        // leaves the visit open.
        assert!(visit_still_open(
            &with(vec![declared("s1", "note"), declared("other", "verdict")]),
            "s1",
            2,
            &fields
        ));
        // Declared evidence in the visit closes it, whoever sent it.
        assert!(!visit_still_open(
            &with(vec![declared("s1", "verdict")]),
            "s1",
            2,
            &fields
        ));
        // Left the state.
        assert!(!visit_still_open(
            &with(vec![tr(Some("s1"), "last")]),
            "s1",
            2,
            &fields
        ));
        // Left and came back: a new visit.
        assert!(!visit_still_open(
            &with(vec![tr(Some("s1"), "last"), tr(Some("last"), "s1")]),
            "s1",
            2,
            &fields
        ));
        // A self-loop is a new visit too.
        assert!(!visit_still_open(
            &with(vec![tr(Some("s1"), "s1")]),
            "s1",
            2,
            &fields
        ));
    }

    #[test]
    fn agent_evidence_for_a_declared_field_skips_the_consultation() {
        let tpl = chain(&[("s1", "last")]);
        let mut port = FakePort::new(vec![]);
        let mut evidence = BTreeMap::new();
        evidence.insert("verdict".to_string(), serde_json::json!("go"));
        let (r, events) = run(&tpl, "s1", evidence, Some(&mut port), false);
        assert_eq!(port.calls, 0);
        // The agent's value stands.
        assert_eq!(r.final_state, "last");
        assert_eq!(names(&events), vec!["transitioned"]);
    }

    #[test]
    fn a_failed_gate_skips_the_consultation() {
        let tpl = chain_with(
            &[("s1", "last")],
            "auto",
            "    gates:\n      ci:\n        type: command\n        command: \"true\"\n",
        );
        let mut port = FakePort::new(vec![]);
        let (r, events) = run(&tpl, "s1", BTreeMap::new(), Some(&mut port), true);
        assert_eq!(port.calls, 0);
        assert!(is_evidence_required(&r));
        assert!(!names(&events).contains(&"decider_consulted"));

        // The same state with the gate passing is consulted.
        let mut port = FakePort::new(vec![GO]);
        let (r, _) = run(&tpl, "s1", BTreeMap::new(), Some(&mut port), false);
        assert_eq!(port.calls, 1);
        assert_eq!(r.final_state, "last");
    }

    #[test]
    fn all_values_off_skips_the_consultation() {
        let tpl = chain(&[("s1", "last")]);
        let mut port = FakePort::with_policy(vec![], GlobalMode::Auto, Some(GlobalMode::Off));
        let (r, _) = run(&tpl, "s1", BTreeMap::new(), Some(&mut port), false);
        assert_eq!(port.calls, 0);
        assert!(is_evidence_required(&r));
    }

    #[test]
    fn skipped_reply_records_nothing() {
        let tpl = chain(&[("s1", "last")]);
        let mut port = FakePort::new(vec![Scripted::Skip]);
        let (r, events) = run(&tpl, "s1", BTreeMap::new(), Some(&mut port), false);
        assert_eq!(port.calls, 1);
        assert!(events.is_empty());
        assert!(is_evidence_required(&r));
        assert!(port.recorded.is_empty());
    }

    #[test]
    fn no_port_is_the_opted_out_loop() {
        let tpl = chain(&[("s1", "last")]);
        let (r, events) = run(&tpl, "s1", BTreeMap::new(), None, false);
        assert!(events.is_empty());
        assert_eq!(r.final_state, "s1");
        assert!(is_evidence_required(&r));
    }

    #[test]
    fn states_reached_by_auto_advance_are_consulted_up_to_the_cap() {
        let tpl = chain(&[
            ("s1", "s2"),
            ("s2", "s3"),
            ("s3", "s4"),
            ("s4", "s5"),
            ("s5", "last"),
        ]);
        let mut port = FakePort::new(vec![GO, GO, GO, GO, GO]);
        let (r, events) = run(&tpl, "s1", BTreeMap::new(), Some(&mut port), false);
        assert_eq!(port.calls, MAX_CONSULTATIONS_PER_CALL);
        assert_eq!(r.final_state, "s5");
        assert!(is_evidence_required(&r));
        assert_eq!(
            names(&events)
                .iter()
                .filter(|n| **n == "decider_consulted")
                .count(),
            4
        );
    }

    #[test]
    fn input_unavailable_and_error_count_toward_the_cap_and_skips_do_not() {
        let tpl = chain(&[("s1", "last")]);
        let ts = &tpl.states["s1"];
        let evidence_value = serde_json::json!({});
        let variables = HashMap::new();
        let visited = HashSet::new();
        let agent = BTreeMap::new();
        let ctx = StopContext {
            state: "s1",
            template: &tpl,
            template_state: ts,
            agent_evidence: &agent,
            gates_failed: false,
            evidence_value: &evidence_value,
            variables: &variables,
            visited: &visited,
        };
        let mut append = |_: &EventPayload| -> Result<(), String> { Ok(()) };

        // Skips don't count.
        let mut port = FakePort::new((0..6).map(|_| Scripted::Skip).collect());
        let mut count = 0;
        for _ in 0..6 {
            consult_at_stop(&mut port, &ctx, &mut count, &mut append).unwrap();
        }
        assert_eq!(count, 0);
        assert_eq!(port.calls, 6);

        // input_unavailable and error do; the fifth stop isn't offered.
        let mut port = FakePort::new(vec![
            Scripted::Input,
            Scripted::Fail(ErrorClass::Timeout),
            Scripted::Input,
            Scripted::Fail(ErrorClass::HttpStatus),
        ]);
        let mut count = 0;
        for _ in 0..5 {
            consult_at_stop(&mut port, &ctx, &mut count, &mut append).unwrap();
        }
        assert_eq!(count, MAX_CONSULTATIONS_PER_CALL);
        assert_eq!(port.calls, 4);
    }

    #[test]
    fn a_target_already_visited_is_not_applied() {
        // s1 -> s2 -> s3, whose `go` route leads back to s2.
        let tpl = chain(&[("s1", "s2"), ("s2", "s3"), ("s3", "s2")]);
        let mut port = FakePort::new(vec![GO, GO, GO]);
        let (r, events) = run(&tpl, "s1", BTreeMap::new(), Some(&mut port), false);
        assert_eq!(r.final_state, "s3");
        assert!(is_evidence_required(&r));
        let outcomes: Vec<ConsultationOutcome> = events
            .iter()
            .filter_map(|e| match e {
                EventPayload::DeciderConsulted(c) => Some(c.outcome),
                _ => None,
            })
            .collect();
        assert_eq!(
            outcomes,
            vec![
                ConsultationOutcome::Applied,
                ConsultationOutcome::Applied,
                ConsultationOutcome::NotApplied
            ]
        );
    }

    #[test]
    fn never_is_consulted_recorded_and_not_applied() {
        let tpl = chain_with(&[("s1", "last")], "never", "");
        let mut port = FakePort::new(vec![GO]);
        let (r, events) = run(&tpl, "s1", BTreeMap::new(), Some(&mut port), false);
        assert_eq!(port.calls, 1);
        assert_eq!(r.final_state, "s1");
        let c = consultation(&events);
        assert_eq!(c.outcome, ConsultationOutcome::NotApplied);
        assert_eq!(c.fields["verdict"].modes["go"], DeciderMode::Never);
        assert_eq!(c.fields["verdict"].modes["hold"], DeciderMode::Auto);
        assert_eq!(
            c.fields["verdict"].outcome,
            Some(crate::decider::FieldOutcome::Never)
        );
    }

    #[test]
    fn project_shadow_or_user_shadow_is_not_applied() {
        let tpl = chain(&[("s1", "last")]);
        for (user, project) in [
            (GlobalMode::Auto, Some(GlobalMode::Shadow)),
            (GlobalMode::Shadow, Some(GlobalMode::Auto)),
        ] {
            let mut port = FakePort::with_policy(vec![GO], user, project);
            let (r, events) = run(&tpl, "s1", BTreeMap::new(), Some(&mut port), false);
            assert_eq!(port.calls, 1);
            assert_eq!(r.final_state, "s1");
            let c = consultation(&events);
            assert_eq!(c.outcome, ConsultationOutcome::NotApplied);
            assert_eq!(c.fields["verdict"].modes["go"], DeciderMode::Shadow);
        }
    }

    #[test]
    fn failures_record_their_class_and_stop_as_opted_out() {
        let tpl = chain(&[("s1", "last")]);
        for (reply, outcome, class) in [
            (
                Scripted::Fail(ErrorClass::Connect),
                ConsultationOutcome::Error,
                Some(ErrorClass::Connect),
            ),
            (Scripted::Input, ConsultationOutcome::InputUnavailable, None),
            // Probabilities that don't sum to one.
            (
                Scripted::Answer(0.9, 0.9, 0.9),
                ConsultationOutcome::Error,
                Some(ErrorClass::Malformed),
            ),
        ] {
            let mut port = FakePort::new(vec![reply]);
            let (r, events) = run(&tpl, "s1", BTreeMap::new(), Some(&mut port), false);
            let (opted_out, _) = run(&tpl, "s1", BTreeMap::new(), None, false);
            assert_eq!(r, opted_out);
            assert_eq!(names(&events), vec!["decider_consulted"]);
            let c = consultation(&events);
            assert_eq!(c.outcome, outcome);
            assert_eq!(c.error_class, class);
            if outcome == ConsultationOutcome::InputUnavailable
                || class == Some(ErrorClass::Connect)
            {
                // No response, so no model.
                assert_eq!(c.model, "unknown");
            }
            assert!(c.fields["verdict"].probabilities.is_empty());
            assert!(!c.fields["verdict"].declaration_hash.is_empty());
        }
    }

    #[test]
    fn the_escape_is_never_applied() {
        let tpl = chain(&[("s1", "last")]);
        let mut port = FakePort::new(vec![Scripted::Answer(0.02, 0.03, 0.95)]);
        let (r, events) = run(&tpl, "s1", BTreeMap::new(), Some(&mut port), false);
        assert_eq!(r.final_state, "s1");
        let c = consultation(&events);
        assert_eq!(c.outcome, ConsultationOutcome::NotApplied);
        assert_eq!(
            c.fields["verdict"].outcome,
            Some(crate::decider::FieldOutcome::Escape)
        );
        assert!(!names(&events).contains(&"evidence_submitted"));
    }

    #[test]
    fn an_answer_matching_two_transitions_is_not_applied() {
        // The compiler refuses overlapping routes, so build one by hand: a
        // second route that also matches `go` through `evidence.verdict`.
        let mut tpl = chain(&[("s1", "last")]);
        let mut when = BTreeMap::new();
        when.insert("evidence.verdict".to_string(), serde_json::json!("present"));
        tpl.states
            .get_mut("s1")
            .unwrap()
            .transitions
            .push(crate::template::types::Transition {
                target: "parked".to_string(),
                when: Some(when),
                context_assignments: Default::default(),
            });
        let mut port = FakePort::new(vec![GO]);
        let (r, events) = run(&tpl, "s1", BTreeMap::new(), Some(&mut port), false);
        let (opted_out, _) = run(&tpl, "s1", BTreeMap::new(), None, false);
        assert_eq!(r, opted_out);
        assert_eq!(names(&events), vec!["decider_consulted"]);
        assert_eq!(
            consultation(&events).outcome,
            ConsultationOutcome::NotApplied
        );
    }

    #[test]
    fn an_applied_answer_writes_the_taken_edges_context_assignments() {
        // The `go` edge assigns from the evidence that picked it; the
        // decider's answer is that evidence, so it resolves to `go`.
        let mut tpl = chain(&[("s1", "last")]);
        let s1 = tpl.states.get_mut("s1").unwrap();
        let mut assignments = BTreeMap::new();
        assignments.insert("picked".to_string(), "${evidence.verdict}".to_string());
        assignments.insert("route".to_string(), "fast".to_string());
        s1.transitions[0].context_assignments = assignments;
        let mut parked = BTreeMap::new();
        parked.insert("route".to_string(), "parked".to_string());
        s1.transitions[1].context_assignments = parked;

        let mut port = FakePort::new(vec![GO]);
        let (r, events) = run(&tpl, "s1", BTreeMap::new(), Some(&mut port), false);
        assert_eq!(r.final_state, "last");
        match &events[2] {
            EventPayload::Transitioned {
                context_assignments,
                ..
            } => {
                let written = context_assignments.as_ref().expect("assignments");
                assert_eq!(written["picked"], "go");
                assert_eq!(written["route"], "fast");
            }
            other => panic!("{:?}", other),
        }
    }
}
