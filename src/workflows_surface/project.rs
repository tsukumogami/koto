//! Derive the minimal `/workflows` projection from koto's event log.
//!
//! This is the "reuse the read seam" half of the feature: the projection is a
//! derivation over the append-only log, not a second store. It reuses the same
//! pure helpers the dashboard's read seam uses -- `derive_state_from_log`,
//! `derive_machine_state`, `is_terminal_state`, `is_failed_state` (all in
//! `crate::engine::persistence`) -- so the rendered running/done/failed status
//! matches the dashboard's classification by construction.

use std::collections::{HashMap, HashSet};

use crate::engine::persistence::{
    derive_machine_state, derive_state_from_log, derive_visit_counts, is_failed_state,
    is_terminal_state, latest_epoch_gate,
};
use crate::engine::types::{Event, EventPayload, StateFileHeader};
use crate::session::SessionBackend;
use crate::template::types::CompiledTemplate;

use super::contract::{HoldingKey, Holdings, RenderStatus, HOLDINGS_KEY_CAP};

const LABEL_SEP: &str = " \u{b7} "; // " · "

/// The minimal per-session projection the initial render produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection {
    /// The session's stable init-time UUID.
    pub session_id: String,
    /// The session's workflow name.
    pub workflow: String,
    /// The derived display label (intent, else template·state, else name).
    pub display_name: String,
    /// The current state, or `None` if the session never advanced.
    pub current_state: Option<String>,
    /// Running / completed / failed.
    pub status: RenderStatus,
    /// The session was cancelled and is not at a terminal state: what made
    /// it `Failed`. A session at a terminal state is classified by the state
    /// it ended at even when it was also cancelled, so this is `false` there.
    pub cancelled: bool,
}

/// Derive the minimal projection for `session_id`, reading through `backend`.
///
/// Returns `None` only when the session's event log cannot be read (a
/// just-committed session always reads back). All classification is a pure
/// function of the log plus the compiled template.
pub fn derive_minimal_projection(
    backend: &dyn SessionBackend,
    session_id: &str,
) -> Option<Projection> {
    let (header, events) = backend.read_events(session_id).ok()?;
    let session_dir = backend.session_dir(session_id);
    Some(project_from_log(&header, &events, &session_dir))
}

/// The pure minimal-projection computation over an already-read log. Shared by
/// [`derive_minimal_projection`] (the minimal shape) and [`derive_enriched_projection`]
/// (the enriched shape) so both derive the base status identically from one read.
fn project_from_log(
    header: &StateFileHeader,
    events: &[Event],
    session_dir: &std::path::Path,
) -> Projection {
    let current_state = derive_state_from_log(events);

    // Terminal detection reuses the dashboard's derivation: resolve the machine
    // state (current state + template path), then read the template's terminal
    // flag for that state.
    let is_terminal = match derive_machine_state(header, events, session_dir) {
        Some(ms) => is_terminal_state(&ms.template_path, &ms.current_state),
        None => false,
    };

    let cancelled = events
        .iter()
        .any(|e| matches!(e.payload, EventPayload::WorkflowCancelled { .. }));

    let status = if is_terminal {
        if is_failed_state(current_state.as_deref()) {
            RenderStatus::Failed
        } else {
            RenderStatus::Completed
        }
    } else if cancelled {
        // A cancelled session renders terminal (abandoned) rather than a stuck
        // `running` spinner, even if its state is not template-terminal.
        RenderStatus::Failed
    } else {
        RenderStatus::Running
    };

    // The workflow name is the stable header identity; session_id is the UUID
    // (may be empty on pre-UUID legacy state files -- callers fall back).
    let workflow = header.workflow.clone();
    let intent = crate::engine::types::effective_intent(header, events);
    let display_name = derive_display_name(
        intent.as_deref(),
        header,
        current_state.as_deref(),
        &workflow,
    );

    Projection {
        session_id: header.session_id.clone(),
        workflow,
        display_name,
        current_state,
        status,
        cancelled: cancelled && !is_terminal,
    }
}

// ---------------------------------------------------------------------------
// Enriched projection (ordered phases, per-phase outcomes, blocked)
// ---------------------------------------------------------------------------

/// Where a phase sits relative to the session's current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseStatus {
    /// Visited and left behind.
    Done,
    /// The session's current state.
    Active,
    /// Reachable but not yet entered.
    Upcoming,
}

/// The outcome of a gate evaluated in a state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateOutcome {
    /// The gate's identifier.
    pub name: String,
    /// Whether it passed.
    pub passed: bool,
}

/// What a state produced: its latest gate outcome and its submitted evidence.
#[derive(Debug, Clone, Default)]
pub struct StateOutcome {
    /// The most recent gate evaluation recorded in this state, if any.
    pub gate: Option<GateOutcome>,
    /// Evidence field-sets submitted while in this state, oldest-first.
    pub evidence: Vec<serde_json::Value>,
}

/// One entry in the enriched ordered phase list.
#[derive(Debug, Clone)]
pub struct PhaseEntry {
    /// The koto state name.
    pub state: String,
    /// The human-readable phase label.
    pub title: String,
    /// Done / active / upcoming relative to the current state.
    pub status: PhaseStatus,
    /// The state's directive text from the compiled template.
    pub directive: String,
    /// The state's evidence/gate outcome.
    pub outcome: StateOutcome,
}

/// The enriched projection: the minimal base plus the
/// ordered phase list.
#[derive(Debug, Clone)]
pub struct EnrichedProjection {
    /// The minimal projection (status already blocked-adjusted).
    pub base: Projection,
    /// The session's phases in structural order.
    pub phases: Vec<PhaseEntry>,
    /// The holdings summary, folded from the same log read (contract v3).
    pub holdings: Holdings,
    /// The gate the session is blocked on: `Some` exactly when
    /// `base.status` is `Blocked`, read from the same epoch-scoped
    /// evaluation the blocked classification decided on.
    pub blocked_gate: Option<String>,
}

/// Fold the holdings summary from the log: last writer wins per key, with
/// `context_removed` and `context_cleared` taking keys back out. Counts and
/// the byte total cover every live key; the listed names are the first
/// [`HOLDINGS_KEY_CAP`] that pass the key grammar, because a pulled log can
/// carry names no local write ever validated, and those are counted but
/// never listed. Local state only: no store read, no remote request —
/// `materialize_after_commit` runs this on every event append.
fn fold_holdings(header: &StateFileHeader, events: &[Event]) -> Holdings {
    use crate::session::context_log::loggable_key;
    use std::collections::BTreeMap;

    let mut live: BTreeMap<&str, u64> = BTreeMap::new();
    let mut event_anchor: Option<&std::path::Path> = None;
    for e in events {
        match &e.payload {
            EventPayload::ContextAdded { key, size, .. } => {
                live.insert(key.as_str(), *size);
            }
            EventPayload::ContextRemoved { key, .. } => {
                live.remove(key.as_str());
            }
            EventPayload::ContextCleared { keys, .. } => {
                for k in keys {
                    live.remove(k.as_str());
                }
            }
            EventPayload::ExecutionAnchorAdopted { anchor } => {
                event_anchor = Some(anchor.as_path());
            }
            EventPayload::ExecutionAnchorRebound { to, .. } => {
                event_anchor = Some(to.as_path());
            }
            _ => {}
        }
    }

    let total_bytes = live.values().fold(0u64, |acc, s| acc.saturating_add(*s));
    let valid: Vec<(&str, u64)> = live
        .iter()
        .filter(|(k, _)| loggable_key(k))
        .map(|(k, s)| (*k, *s))
        .collect();
    let keys_truncated = valid.len() > HOLDINGS_KEY_CAP;
    let keys = valid
        .into_iter()
        .take(HOLDINGS_KEY_CAP)
        .map(|(k, s)| HoldingKey {
            key: k.to_string(),
            size: s,
        })
        .collect();

    let cap_fact = |p: &std::path::Path| {
        crate::session_view::sanitize_capped(&p.to_string_lossy(), crate::session_view::FACT_CAP)
            .into_string()
    };
    let anchor = event_anchor
        .map(cap_fact)
        .or_else(|| header.execution_dir.as_deref().map(cap_fact))
        .or_else(|| header.origin.as_ref().map(|o| cap_fact(&o.anchor)));
    let store = header.origin.as_ref().map(|o| {
        crate::session_view::sanitize_capped(&o.store.kind, super::contract::HOLDINGS_STORE_CAP)
            .into_string()
    });

    Holdings {
        key_count: live.len(),
        total_bytes,
        keys,
        keys_truncated,
        anchor,
        store,
    }
}

/// Derive the enriched projection for `session_id`, reading the log once.
///
/// Reuses the read-seam derivations the minimal projection uses for the base fields and the
/// read the dashboard's blocked predicate is built on ([`latest_epoch_gate`])
/// for the `blocked` status and the gate it names, then adds the ordered phase list ([`ordered_phases`]) with
/// per-phase status, directive, and outcome ([`per_state_outcomes`]). Returns
/// `None` only when the log cannot be read.
pub fn derive_enriched_projection(
    backend: &dyn SessionBackend,
    session_id: &str,
) -> Option<EnrichedProjection> {
    let (header, events) = backend.read_events(session_id).ok()?;
    let session_dir = backend.session_dir(session_id);

    let mut base = project_from_log(&header, &events, &session_dir);

    // Blocked overrides a non-terminal Running: a non-terminal session whose
    // latest current-epoch gate did not pass renders `blocked` (same predicate
    // the dashboard's `is_blocked` uses). Precedence: terminal > blocked >
    // running, so only Running is upgraded. The gate named is the one the
    // predicate read, so the entry's text and its classification agree.
    let mut blocked_gate = None;
    if base.status == RenderStatus::Running {
        if let Some(cs) = base.current_state.as_deref() {
            if let Some((gate, false)) = latest_epoch_gate(&events, cs) {
                base.status = RenderStatus::Blocked;
                blocked_gate = Some(gate);
            }
        }
    }

    // Load the compiled template (best-effort): no template -> no phase list,
    // and the entry still renders the minimal fields.
    let compiled = derive_machine_state(&header, &events, &session_dir).and_then(|ms| {
        std::fs::read(&ms.template_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<CompiledTemplate>(&bytes).ok())
    });

    let phases = match &compiled {
        Some(t) => build_phase_entries(t, &events, base.current_state.as_deref()),
        None => Vec::new(),
    };

    let holdings = fold_holdings(&header, &events);
    Some(EnrichedProjection {
        base,
        phases,
        holdings,
        blocked_gate,
    })
}

/// Build the ordered phase entries with per-phase status, directive, and
/// outcome.
fn build_phase_entries(
    template: &CompiledTemplate,
    events: &[Event],
    current_state: Option<&str>,
) -> Vec<PhaseEntry> {
    let order = ordered_phases(template);
    let visited = visited_states(template, events);
    let outcomes = per_state_outcomes(events);

    order
        .into_iter()
        .map(|state| {
            let status = if current_state == Some(state.as_str()) {
                PhaseStatus::Active
            } else if visited.contains(&state) {
                PhaseStatus::Done
            } else {
                PhaseStatus::Upcoming
            };
            let directive = template
                .states
                .get(&state)
                .map(|s| s.directive.clone())
                .unwrap_or_default();
            let outcome = outcomes.get(&state).cloned().unwrap_or_default();
            let title = humanize_state(&state);
            PhaseEntry {
                state,
                title,
                status,
                directive,
                outcome,
            }
        })
        .collect()
}

/// Order the template's states into a stable phase sequence: a pre-order walk
/// from `initial_state` following each state's declared `transitions` in order
/// (dedup, self-loops skipped), then any states unreachable from
/// `initial_state` appended in template (`BTreeMap`) order.
///
/// The order is a pure function of the template, so it does not reshuffle as
/// the session advances or rewinds -- only the per-phase status moves.
pub fn ordered_phases(template: &CompiledTemplate) -> Vec<String> {
    let mut order: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // Pre-order DFS in declared-transition order. Push children reversed so
    // they pop in declared order.
    let mut stack: Vec<String> = vec![template.initial_state.clone()];
    while let Some(state) = stack.pop() {
        if !seen.insert(state.clone()) {
            continue;
        }
        order.push(state.clone());
        if let Some(s) = template.states.get(&state) {
            for tr in s.transitions.iter().rev() {
                if !seen.contains(&tr.target) {
                    stack.push(tr.target.clone());
                }
            }
        }
    }

    // Deterministic tail: states unreachable from `initial_state`, in BTreeMap
    // (sorted) order.
    for name in template.states.keys() {
        if seen.insert(name.clone()) {
            order.push(name.clone());
        }
    }

    order
}

/// The set of states the session has entered: the initial state plus every
/// transition/directed/rewind target (via [`derive_visit_counts`]).
fn visited_states(template: &CompiledTemplate, events: &[Event]) -> HashSet<String> {
    let mut visited: HashSet<String> = derive_visit_counts(events).into_keys().collect();
    // The initial state is entered at init without a transition event.
    if !template.initial_state.is_empty() {
        visited.insert(template.initial_state.clone());
    }
    visited
}

/// Bucket, per state, the latest gate outcome and the evidence submitted while
/// in that state, over the full event log. Reuses the same `EventPayload`
/// matching the dashboard read seam uses; both `GateEvaluated` and
/// `EvidenceSubmitted` carry their own `state`, so no epoch tracking is needed.
pub fn per_state_outcomes(events: &[Event]) -> HashMap<String, StateOutcome> {
    let mut map: HashMap<String, StateOutcome> = HashMap::new();
    for e in events {
        match &e.payload {
            EventPayload::GateEvaluated {
                state,
                gate,
                outcome,
                ..
            } => {
                // Forward iteration overwrites, so the latest gate wins.
                map.entry(state.clone()).or_default().gate = Some(GateOutcome {
                    name: gate.clone(),
                    passed: outcome == "passed",
                });
            }
            EventPayload::EvidenceSubmitted { state, fields, .. } => {
                let obj = serde_json::Value::Object(
                    fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                );
                map.entry(state.clone()).or_default().evidence.push(obj);
            }
            _ => {}
        }
    }
    map
}

/// Humanize a snake/kebab-case state name into a sentence-case phase label:
/// `gather_context` -> `Gather context`. Falls back to the raw name if empty.
pub fn humanize_state(name: &str) -> String {
    let mut out = String::new();
    for (i, word) in name.split(['_', '-']).filter(|w| !w.is_empty()).enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if i == 0 {
            let mut chars = word.chars();
            if let Some(f) = chars.next() {
                out.extend(f.to_uppercase());
                out.push_str(chars.as_str());
            }
        } else {
            out.push_str(word);
        }
    }
    if out.is_empty() {
        name.to_string()
    } else {
        out
    }
}

/// Minimal display-label derivation, mirroring the dashboard's `derive_label`
/// rungs 1-3 without the `CachedSession` coupling: explicit intent, else
/// `template_name · current_state`, else `untitled (template_name)`, else the
/// bare workflow name.
fn derive_display_name(
    intent: Option<&str>,
    header: &crate::engine::types::StateFileHeader,
    current_state: Option<&str>,
    workflow: &str,
) -> String {
    if let Some(intent) = intent {
        if !intent.is_empty() {
            return intent.to_string();
        }
    }
    let template_name = header.template_name.as_deref().unwrap_or("");
    if !template_name.is_empty() {
        if let Some(cs) = current_state {
            if !cs.is_empty() {
                return format!("{template_name}{LABEL_SEP}{cs}");
            }
        }
        return format!("untitled ({template_name})");
    }
    workflow.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::StateFileHeader;

    fn header(intent: Option<&str>, template_name: Option<&str>) -> StateFileHeader {
        let mut h: StateFileHeader = serde_json::from_str(
            r#"{"schema_version":1,"workflow":"wf.name","template_hash":"h","created_at":"2026-01-01T00:00:00Z"}"#,
        )
        .expect("minimal header deserializes");
        h.intent = intent.map(str::to_string);
        h.template_name = template_name.map(str::to_string);
        h
    }

    fn event(seq: u64, payload: EventPayload) -> Event {
        Event {
            seq,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: payload.type_name().to_string(),
            payload,
            idempotency_hash: None,
        }
    }

    fn added(seq: u64, key: &str, size: u64) -> Event {
        event(
            seq,
            EventPayload::ContextAdded {
                key: key.to_string(),
                hash: "h".to_string(),
                size,
                writer: None,
            },
        )
    }

    #[test]
    fn holdings_fold_is_last_writer_wins() {
        let h = header(None, None);
        let events = vec![
            added(1, "a.md", 10),
            added(2, "b.md", 20),
            added(3, "a.md", 30), // re-add replaces
            event(
                4,
                EventPayload::ContextRemoved {
                    key: "b.md".to_string(),
                    writer: None,
                },
            ),
            added(5, "c.md", 5),
            event(
                6,
                EventPayload::ContextCleared {
                    state: "s".to_string(),
                    keys: vec!["c.md".to_string()],
                    entry_seq: 5,
                },
            ),
        ];
        let holdings = fold_holdings(&h, &events);
        assert_eq!(holdings.key_count, 1);
        assert_eq!(holdings.total_bytes, 30);
        assert_eq!(holdings.keys.len(), 1);
        assert_eq!(holdings.keys[0].key, "a.md");
        assert_eq!(holdings.keys[0].size, 30);
        assert!(!holdings.keys_truncated);
        assert!(holdings.anchor.is_none(), "no anchor recorded anywhere");
        assert!(holdings.store.is_none());
    }

    #[test]
    fn holdings_counts_invalid_names_but_never_lists_them() {
        let h = header(None, None);
        let events = vec![added(1, "ok.md", 7), added(2, "../../etc/hosts", 9)];
        let holdings = fold_holdings(&h, &events);
        assert_eq!(holdings.key_count, 2);
        assert_eq!(holdings.total_bytes, 16);
        assert_eq!(holdings.keys.len(), 1);
        assert_eq!(holdings.keys[0].key, "ok.md");
    }

    #[test]
    fn holdings_saturate_and_truncate_at_the_cap() {
        let h = header(None, None);
        let mut events = vec![added(1, "big.bin", u64::MAX), added(2, "more.bin", 1)];
        for i in 0..super::HOLDINGS_KEY_CAP {
            events.push(added(3 + i as u64, &format!("k{i:03}.md"), 1));
        }
        let holdings = fold_holdings(&h, &events);
        assert_eq!(holdings.total_bytes, u64::MAX, "saturating, no overflow");
        assert_eq!(holdings.keys.len(), super::HOLDINGS_KEY_CAP);
        assert!(holdings.keys_truncated);
    }

    #[test]
    fn holdings_empty_session_is_zeroes() {
        let h = header(None, None);
        let holdings = fold_holdings(&h, &[]);
        assert_eq!(holdings.key_count, 0);
        assert_eq!(holdings.total_bytes, 0);
        assert!(holdings.keys.is_empty());
        assert!(!holdings.keys_truncated);
    }

    #[test]
    fn holdings_anchor_prefers_the_last_anchor_event() {
        let mut h = header(None, None);
        h.execution_dir = Some(std::path::PathBuf::from("/srv/original"));
        let events = vec![
            event(
                1,
                EventPayload::ExecutionAnchorAdopted {
                    anchor: std::path::PathBuf::from("/srv/adopted"),
                },
            ),
            event(
                2,
                EventPayload::ExecutionAnchorRebound {
                    from: Some(std::path::PathBuf::from("/srv/adopted")),
                    to: std::path::PathBuf::from("/srv/rebound"),
                },
            ),
        ];
        let holdings = fold_holdings(&h, &events);
        assert_eq!(holdings.anchor.as_deref(), Some("/srv/rebound"));
        let none = fold_holdings(&h, &[]);
        assert_eq!(none.anchor.as_deref(), Some("/srv/original"));
    }

    #[test]
    fn holdings_sanitizes_a_hostile_anchor() {
        let mut h = header(None, None);
        h.execution_dir = Some(std::path::PathBuf::from("/srv/\u{1B}[2J/evil"));
        let holdings = fold_holdings(&h, &[]);
        assert_eq!(holdings.anchor.as_deref(), Some("/srv/\\u{1B}[2J/evil"));
    }

    #[test]
    fn label_prefers_intent() {
        let h = header(Some("fix the bug"), Some("tmpl"));
        assert_eq!(
            derive_display_name(h.intent.as_deref(), &h, Some("building"), "wf.name"),
            "fix the bug"
        );
    }

    #[test]
    fn label_falls_back_to_template_and_state() {
        let h = header(None, Some("tmpl"));
        assert_eq!(
            derive_display_name(h.intent.as_deref(), &h, Some("building"), "wf.name"),
            "tmpl \u{b7} building"
        );
    }

    #[test]
    fn label_falls_back_to_workflow_name() {
        let h = header(None, None);
        assert_eq!(
            derive_display_name(h.intent.as_deref(), &h, Some("building"), "wf.name"),
            "wf.name"
        );
    }

    fn compile(json: &str) -> CompiledTemplate {
        serde_json::from_str(json).expect("compiled template deserializes")
    }

    fn transitioned(to: &str) -> Event {
        Event {
            seq: 0,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            event_type: "transitioned".to_string(),
            payload: EventPayload::Transitioned {
                from: None,
                to: to.to_string(),
                condition_type: "auto".to_string(),
                skip_if_matched: None,
                context_assignments: None,
                vars_matched: None,
            },
            idempotency_hash: None,
        }
    }

    #[test]
    fn humanize_state_sentence_cases() {
        assert_eq!(humanize_state("gather_context"), "Gather context");
        assert_eq!(humanize_state("implement"), "Implement");
        assert_eq!(humanize_state("run-tests"), "Run tests");
        assert_eq!(humanize_state(""), "");
    }

    #[test]
    fn ordered_phases_linear() {
        let t = compile(
            r#"{"format_version":1,"name":"n","version":"1","initial_state":"a",
                "states":{
                  "a":{"directive":"d","transitions":[{"target":"b"}]},
                  "b":{"directive":"d","transitions":[{"target":"c"}]},
                  "c":{"directive":"d","terminal":true}
                }}"#,
        );
        assert_eq!(ordered_phases(&t), vec!["a", "b", "c"]);
    }

    #[test]
    fn ordered_phases_branching_declared_order() {
        // staging branches to production then rollback (declared order);
        // pre-order DFS visits production's subtree before rollback.
        let t = compile(
            r#"{"format_version":1,"name":"n","version":"1","initial_state":"staging",
                "states":{
                  "staging":{"directive":"d","transitions":[{"target":"production"},{"target":"rollback"}]},
                  "production":{"directive":"d","terminal":true},
                  "rollback":{"directive":"d","terminal":true}
                }}"#,
        );
        assert_eq!(
            ordered_phases(&t),
            vec!["staging", "production", "rollback"]
        );
    }

    #[test]
    fn ordered_phases_skips_self_loop() {
        let t = compile(
            r#"{"format_version":1,"name":"n","version":"1","initial_state":"build",
                "states":{
                  "build":{"directive":"d","transitions":[{"target":"test"},{"target":"build"}]},
                  "test":{"directive":"d","terminal":true}
                }}"#,
        );
        // The self-loop back to build is skipped (already visited).
        assert_eq!(ordered_phases(&t), vec!["build", "test"]);
    }

    #[test]
    fn ordered_phases_appends_unreachable_tail_sorted() {
        // `orphan` is unreachable from `a`; it appends after the reachable set,
        // in BTreeMap (sorted) order.
        let t = compile(
            r#"{"format_version":1,"name":"n","version":"1","initial_state":"a",
                "states":{
                  "a":{"directive":"d","transitions":[{"target":"b"}]},
                  "b":{"directive":"d","terminal":true},
                  "orphan":{"directive":"d","terminal":true}
                }}"#,
        );
        assert_eq!(ordered_phases(&t), vec!["a", "b", "orphan"]);
    }

    #[test]
    fn per_state_outcomes_latest_gate_wins_and_buckets_evidence() {
        let mut fields = std::collections::HashMap::new();
        fields.insert("files".to_string(), serde_json::json!("auth.rs"));
        let events = vec![
            Event {
                seq: 1,
                timestamp: "t".to_string(),
                event_type: "evidence_submitted".to_string(),
                payload: EventPayload::EvidenceSubmitted {
                    state: "gather".to_string(),
                    fields,
                    submitter_cwd: None,
                    source: None,
                    exported_fields: None,
                },
                idempotency_hash: None,
            },
            Event {
                seq: 2,
                timestamp: "t".to_string(),
                event_type: "gate_evaluated".to_string(),
                payload: EventPayload::GateEvaluated {
                    state: "verify".to_string(),
                    gate: "tests".to_string(),
                    output: serde_json::json!({}),
                    outcome: "failed".to_string(),
                    timestamp: "t".to_string(),
                    check: Default::default(),
                    streams: None,
                    poll: None,
                },
                idempotency_hash: None,
            },
            Event {
                seq: 3,
                timestamp: "t".to_string(),
                event_type: "gate_evaluated".to_string(),
                payload: EventPayload::GateEvaluated {
                    state: "verify".to_string(),
                    gate: "tests".to_string(),
                    output: serde_json::json!({}),
                    outcome: "passed".to_string(),
                    timestamp: "t".to_string(),
                    check: Default::default(),
                    streams: None,
                    poll: None,
                },
                idempotency_hash: None,
            },
        ];
        let outcomes = per_state_outcomes(&events);

        // Evidence bucketed under its state.
        let gather = outcomes.get("gather").expect("gather outcome");
        assert_eq!(gather.evidence.len(), 1);
        assert_eq!(gather.evidence[0]["files"], "auth.rs");

        // Latest gate wins: the passed evaluation overrides the earlier failure.
        let verify = outcomes.get("verify").expect("verify outcome");
        let gate = verify.gate.as_ref().expect("gate");
        assert_eq!(gate.name, "tests");
        assert!(gate.passed);
    }

    #[test]
    fn visited_states_includes_initial_and_targets() {
        let t = compile(
            r#"{"format_version":1,"name":"n","version":"1","initial_state":"a",
                "states":{
                  "a":{"directive":"d","transitions":[{"target":"b"}]},
                  "b":{"directive":"d","transitions":[{"target":"c"}]},
                  "c":{"directive":"d","terminal":true}
                }}"#,
        );
        // Only transitioned to b; a is visited (initial), c is not.
        let events = vec![transitioned("b")];
        let visited = visited_states(&t, &events);
        assert!(visited.contains("a"));
        assert!(visited.contains("b"));
        assert!(!visited.contains("c"));
    }
}
