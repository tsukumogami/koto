//! The CLI's [`DeciderPort`]: the I/O half of a consultation.
//!
//! `handle_next` builds one only when `DeciderSettings::opted_in()` holds.
//! For each consultation the port:
//!
//! 1. takes a non-blocking `flock` on `<session_dir>/decider.lock` (never
//!    the state file, which `_batch_lock` and the append path already lock);
//! 2. re-reads the local log through `SessionBackend::read_events_local`,
//!    so no network round trip happens under the lock, and keeps the
//!    header's `session_id`;
//! 3. derives the visit's seq and re-checks for a prior consultation;
//! 4. assembles each declared input from the context store or the
//!    variable bindings, within its byte budget;
//! 5. calls the provider and times it.
//!
//! Any failure before the provider call (lock, read, left the state,
//! already consulted) is a [`ConsultReply::Skipped`], which the engine
//! turns into the opted-out response. An input problem is recorded as
//! `input_unavailable` and nothing is sent.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::time::Instant;

use sha2::{Digest, Sha256};

use crate::decider::request::{build_request, DeclaredField};
use crate::decider::types::{Decider, LabelledInput, SettingOrigin};
use crate::engine::decider::{
    prior_consultation, visit_start_index, ConsultReply, ConsultRequest, ConsultResult,
    Consultation, DeciderPolicy, DeciderPort, VisitGuard,
};
use crate::engine::persistence::instructions_delivered_this_window;
use crate::engine::substitute::bindings_from_events;
use crate::engine::types::{Event, EventPayload};
use crate::session::context::ContextStore;
use crate::session::SessionBackend;
use crate::template::decider::DeciderInputSource;
use crate::template::types::TemplateState;

/// File name of the per-session consultation lock. It holds no data.
pub const DECIDER_LOCK_FILE: &str = "decider.lock";

/// Renders a template string the way `koto next` renders a directive:
/// runtime names, then the tick's overlay, then the log's bindings.
pub type Render<'a> = Box<dyn Fn(&str) -> String + 'a>;

/// Held for the length of one consultation. Closing the file releases
/// the `flock`.
struct LockHold {
    _file: File,
}

/// Why an input couldn't be assembled. Never carries input content.
#[derive(Debug, Clone, PartialEq, Eq)]
enum InputUnavailable {
    Unset,
    OverBudget,
    NotText,
}

pub struct CliDeciderPort<'a> {
    backend: &'a dyn SessionBackend,
    context_store: &'a dyn ContextStore,
    session: String,
    decider: Box<dyn Decider>,
    policy: DeciderPolicy,
    endpoint_origin: SettingOrigin,
    render: Render<'a>,
    full: bool,
    session_id: Option<String>,
    recorded: usize,
}

impl<'a> CliDeciderPort<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        backend: &'a dyn SessionBackend,
        context_store: &'a dyn ContextStore,
        session: &str,
        decider: Box<dyn Decider>,
        policy: DeciderPolicy,
        endpoint_origin: SettingOrigin,
        render: Render<'a>,
        full: bool,
    ) -> Self {
        CliDeciderPort {
            backend,
            context_store,
            session: session.to_string(),
            decider,
            policy,
            endpoint_origin,
            render,
            full,
            session_id: None,
            recorded: 0,
        }
    }

    /// The session header's `session_id` from the last locked re-read;
    /// `None` before any consultation and for headers that predate it.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// How many `decider_consulted` events this port has been told about.
    pub fn recorded_count(&self) -> usize {
        self.recorded
    }

    fn lock_path(&self) -> PathBuf {
        self.backend
            .session_dir(&self.session)
            .join(DECIDER_LOCK_FILE)
    }

    /// Take `decider.lock` without waiting. `None` on contention or any
    /// error: a loser never waits and never consults.
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
            .open(self.lock_path())
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
        None
    }

    /// Byte length of the directive and details the opted-out `koto next`
    /// response would carry for this state on this visit: the substituted
    /// directive, the recovery pointer a state with details gets, and the
    /// substituted details unless this window already delivered them.
    fn directive_bytes(
        &self,
        state: &str,
        template_state: &TemplateState,
        events: &[Event],
    ) -> u64 {
        let mut bytes = (self.render)(&template_state.directive).len();
        if !template_state.details.is_empty() {
            bytes += crate::cli::next_types::RECOVERY_POINTER.len();
            let suppressed = instructions_delivered_this_window(events, state) && !self.full;
            if !suppressed {
                bytes += (self.render)(&template_state.details).len();
            }
        }
        bytes as u64
    }

    /// Assemble every declared input, keyed by label, each within its
    /// budget. Sources are closed: a context-store key (with `{{KEY}}`
    /// references substituted) or a template variable or capture. Nothing
    /// else in the session is read.
    fn assemble_inputs(
        &self,
        fields: &[DeclaredField<'_>],
        events: &[Event],
    ) -> Result<BTreeMap<String, String>, InputUnavailable> {
        let bindings = bindings_from_events(events);
        let mut out = BTreeMap::new();
        for field in fields {
            for input in &field.decider.inputs {
                if out.contains_key(&input.label) {
                    continue;
                }
                let budget = input.max_bytes as usize;
                let content = match &input.source {
                    DeciderInputSource::Context(raw_key) => {
                        let key = (self.render)(raw_key);
                        let bytes = self
                            .context_store
                            .get(&self.session, &key)
                            .map_err(|_| InputUnavailable::Unset)?;
                        if bytes.len() > budget {
                            return Err(InputUnavailable::OverBudget);
                        }
                        String::from_utf8(bytes).map_err(|_| InputUnavailable::NotText)?
                    }
                    DeciderInputSource::Var(name) => {
                        let value = bindings.get(name).ok_or(InputUnavailable::Unset)?;
                        if value.len() > budget {
                            return Err(InputUnavailable::OverBudget);
                        }
                        value.clone()
                    }
                };
                out.insert(input.label.clone(), content);
            }
        }
        Ok(out)
    }

    fn consulted(
        &self,
        visit_seq: u64,
        input_sha256: Option<String>,
        latency_ms: u64,
        directive_bytes: u64,
        result: ConsultResult,
        guard: VisitGuard,
    ) -> ConsultReply {
        ConsultReply::Consulted(Box::new(Consultation {
            visit_seq,
            provider: self.decider.provider().to_string(),
            input_sha256,
            latency_ms,
            directive_bytes,
            endpoint_origin: self.endpoint_origin,
            result,
            guard,
        }))
    }
}

/// SHA-256 (hex) of the assembled inputs, in the order they're sent. Each
/// label and content is length-prefixed, so two different input sets can't
/// produce the same bytes.
pub fn input_sha256(inputs: &[LabelledInput]) -> String {
    let mut h = Sha256::new();
    h.update(b"koto-decider-inputs/v1");
    h.update((inputs.len() as u64).to_be_bytes());
    for input in inputs {
        for part in [&input.label, &input.content] {
            h.update((part.len() as u64).to_be_bytes());
            h.update(part.as_bytes());
        }
    }
    hex::encode(h.finalize())
}

impl DeciderPort for CliDeciderPort<'_> {
    fn policy(&self) -> &DeciderPolicy {
        &self.policy
    }

    fn consult(&mut self, req: &ConsultRequest<'_>) -> ConsultReply {
        let Some(lock) = self.try_lock() else {
            return ConsultReply::Skipped;
        };
        let Ok((header, events)) = self.backend.read_events_local(&self.session) else {
            return ConsultReply::Skipped;
        };
        self.session_id = Some(header.session_id).filter(|id| !id.is_empty());

        let Some(start) = visit_start_index(&events, req.state) else {
            return ConsultReply::Skipped;
        };
        if prior_consultation(&events, req.state).is_some() {
            return ConsultReply::Skipped;
        }

        let guard = VisitGuard::new(lock);
        let directive_bytes = self.directive_bytes(req.state, req.template_state, &events);

        let request = self
            .assemble_inputs(req.fields, &events)
            .ok()
            .and_then(|inputs| build_request(req.fields, &inputs).ok());
        let Some(request) = request else {
            return self.consulted(
                start.seq,
                None,
                0,
                directive_bytes,
                ConsultResult::InputUnavailable,
                guard,
            );
        };
        let sha = input_sha256(&request.inputs);

        let started = Instant::now();
        let answer = self.decider.decide(&request);
        let latency_ms = started.elapsed().as_millis() as u64;
        let result = match answer {
            Ok(response) => ConsultResult::Answered(response),
            Err(e) => ConsultResult::Failed(e.class),
        };
        self.consulted(
            start.seq,
            Some(sha),
            latency_ms,
            directive_bytes,
            result,
            guard,
        )
    }

    fn recorded(&mut self, event: &EventPayload) {
        if matches!(event, EventPayload::DeciderConsulted(_)) {
            self.recorded += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn li(label: &str, content: &str) -> LabelledInput {
        LabelledInput {
            label: label.to_string(),
            content: content.to_string(),
        }
    }

    #[test]
    fn input_hash_is_stable_and_unambiguous() {
        let a = input_sha256(&[li("plan", "body"), li("item", "x")]);
        assert_eq!(a, input_sha256(&[li("plan", "body"), li("item", "x")]));
        assert_eq!(a.len(), 64);
        assert_ne!(a, input_sha256(&[li("plan", "bod"), li("yitem", "x")]));
        assert_ne!(a, input_sha256(&[li("item", "x"), li("plan", "body")]));
    }

    // -- the port against a real local session ---------------------------

    use crate::config::resolve::{apply_decider_env, resolve_decider};
    use crate::config::DeciderConfig;
    use crate::decider::fake::ScriptedDecider;
    use crate::decider::request::declared_fields;
    use crate::decider::types::{Answer, DeciderError, DecisionRequest, DecisionResponse};
    use crate::engine::types::{Event, EventPayload, StateFileHeader};
    use crate::session::local::LocalBackend;
    use crate::template::types::CompiledTemplate;
    use std::io::Write as _;
    use std::sync::Arc;

    struct Shared(Arc<ScriptedDecider>);

    impl Decider for Shared {
        fn provider(&self) -> &str {
            self.0.provider()
        }
        fn decide(&self, req: &DecisionRequest) -> Result<DecisionResponse, DeciderError> {
            self.0.decide(req)
        }
    }

    const TPL: &str = r#"---
name: port
version: "1.0"
initial_state: review
variables:
  PLAN_DOC:
    description: plan
    default: docs/plan.md
states:
  review:
    accepts:
      verdict:
        type: enum
        values: [proceed, exit]
        required: true
        description: Clear?
        decider:
          answers:
            proceed: {description: "Yes.", mode: shadow}
            exit: {description: "No.", mode: shadow}
          escape: {value: unclear, description: "Unknown."}
          inputs:
            - {var: PLAN_DOC, label: plan_path}
    transitions:
      - target: done
        when:
          verdict: proceed
      - target: done
        when:
          verdict: exit
  done:
    terminal: true
---

## review

Review {{PLAN_DOC}}.

## done

d
"#;

    fn compiled() -> CompiledTemplate {
        let mut f = tempfile::Builder::new().suffix(".md").tempfile().unwrap();
        f.write_all(TPL.as_bytes()).unwrap();
        crate::template::compile::compile(f.path(), true).unwrap()
    }

    fn header(session_id: &str) -> StateFileHeader {
        StateFileHeader {
            schema_version: 1,
            workflow: "wf".to_string(),
            template_hash: "0".repeat(64),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            parent_workflow: None,
            template_source_dir: None,
            execution_dir: None,
            session_id: session_id.to_string(),
            intent: None,
            template_name: None,
            needs_agent: None,
            role: None,
            inputs: None,
            coordinator_of_record: None,
            requested_by: None,
            assignment_claim: None,
            dispatch_epoch: 0,
            priority: None,
            deadline: None,
            retry_count: None,
            agent_config: None,
            respawn_generation: None,
        }
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

    fn session(tmp: &std::path::Path, session_id: &str) -> LocalBackend {
        let backend = LocalBackend::with_base_dir(tmp.to_path_buf());
        backend.create("wf").unwrap();
        let mut vars = std::collections::HashMap::new();
        vars.insert("PLAN_DOC".to_string(), "docs/plan.md".to_string());
        backend
            .init_state_file(
                "wf",
                header(session_id),
                vec![
                    event(
                        1,
                        EventPayload::WorkflowInitialized {
                            template_path: "t.json".to_string(),
                            variables: vars,
                            spawn_entry: None,
                        },
                    ),
                    event(
                        2,
                        EventPayload::Transitioned {
                            from: None,
                            to: "review".to_string(),
                            condition_type: "auto".to_string(),
                            skip_if_matched: None,
                        },
                    ),
                ],
            )
            .unwrap();
        backend
    }

    fn answer() -> DecisionResponse {
        let mut answers = BTreeMap::new();
        answers.insert(
            "verdict".to_string(),
            Answer::Choice {
                probabilities: [
                    ("proceed".to_string(), 0.95),
                    ("exit".to_string(), 0.03),
                    ("unclear".to_string(), 0.02),
                ]
                .into_iter()
                .collect(),
                provider_confidence: None,
            },
        );
        DecisionResponse {
            model: "m".to_string(),
            answers,
        }
    }

    /// Settings with a key and no endpoint: the endpoint is the default.
    fn default_endpoint_settings() -> crate::config::resolve::DeciderSettings {
        let mut cfg = DeciderConfig::default();
        apply_decider_env(&mut cfg, |k| match k {
            "KOTO_DECIDER" => Some("shadow".to_string()),
            "KOTO_DECIDER_API_KEY" => Some("k".to_string()),
            _ => None,
        });
        resolve_decider(&cfg).0
    }

    fn port<'a>(backend: &'a LocalBackend, decider: &Arc<ScriptedDecider>) -> CliDeciderPort<'a> {
        let settings = default_endpoint_settings();
        CliDeciderPort::new(
            backend,
            backend,
            "wf",
            Box::new(Shared(Arc::clone(decider))),
            DeciderPolicy::from_settings(&settings),
            settings.endpoint_origin(),
            Box::new(|s: &str| s.to_string()),
            false,
        )
    }

    fn consult(p: &mut CliDeciderPort<'_>, tpl: &CompiledTemplate) -> ConsultReply {
        let ts = &tpl.states["review"];
        let fields = declared_fields(ts.accepts.as_ref().unwrap());
        p.consult(&ConsultRequest {
            state: "review",
            template_state: ts,
            fields: &fields,
        })
    }

    #[test]
    fn consults_a_fresh_visit_and_keeps_the_session_id() {
        let tmp = tempfile::TempDir::new().unwrap();
        let backend = session(tmp.path(), "sess-uuid-1");
        let tpl = compiled();
        let decider = Arc::new(ScriptedDecider::new());
        decider.push_answer(answer());
        let mut p = port(&backend, &decider);
        assert_eq!(p.session_id(), None);
        let ConsultReply::Consulted(c) = consult(&mut p, &tpl) else {
            panic!("expected a consultation");
        };
        assert_eq!(c.visit_seq, 2);
        assert_eq!(c.provider, "fake");
        assert_eq!(c.endpoint_origin, SettingOrigin::Default);
        assert_eq!(c.input_sha256.as_ref().map(String::len), Some(64));
        assert_eq!(c.directive_bytes, "Review {{PLAN_DOC}}.".len() as u64);
        assert!(matches!(c.result, ConsultResult::Answered(_)));
        assert_eq!(p.session_id(), Some("sess-uuid-1"));
        assert!(tmp.path().join("wf").join(DECIDER_LOCK_FILE).exists());
        let sent = decider.requests();
        assert_eq!(sent[0].inputs, vec![li("plan_path", "docs/plan.md")]);
    }

    #[test]
    fn an_empty_header_session_id_is_none() {
        let tmp = tempfile::TempDir::new().unwrap();
        let backend = session(tmp.path(), "");
        let tpl = compiled();
        let decider = Arc::new(ScriptedDecider::new());
        decider.push_answer(answer());
        let mut p = port(&backend, &decider);
        assert!(matches!(consult(&mut p, &tpl), ConsultReply::Consulted(_)));
        assert_eq!(p.session_id(), None);
    }

    #[test]
    fn a_held_lock_skips_without_waiting_or_calling() {
        let tmp = tempfile::TempDir::new().unwrap();
        let backend = session(tmp.path(), "s");
        let tpl = compiled();
        let decider = Arc::new(ScriptedDecider::new());
        decider.push_answer(answer());
        let mut winner = port(&backend, &decider);
        let held = consult(&mut winner, &tpl);
        assert!(matches!(held, ConsultReply::Consulted(_)));

        let mut loser = port(&backend, &decider);
        let started = Instant::now();
        assert!(matches!(consult(&mut loser, &tpl), ConsultReply::Skipped));
        assert!(started.elapsed() < std::time::Duration::from_millis(500));
        assert_eq!(decider.calls(), 1);
        drop(held);
    }

    #[test]
    fn consults_while_the_state_file_lock_is_held() {
        // `handle_next` holds `_batch_lock` on the state file during a tick
        // that starts on a batch-scoped state. The port never touches it.
        let tmp = tempfile::TempDir::new().unwrap();
        let backend = session(tmp.path(), "s");
        let _batch_lock = backend.lock_state_file("wf").unwrap();
        let tpl = compiled();
        let decider = Arc::new(ScriptedDecider::new());
        decider.push_answer(answer());
        let mut p = port(&backend, &decider);
        assert!(matches!(consult(&mut p, &tpl), ConsultReply::Consulted(_)));
    }

    #[test]
    fn a_prior_consultation_or_a_departed_state_skips() {
        let tmp = tempfile::TempDir::new().unwrap();
        let backend = session(tmp.path(), "s");
        let tpl = compiled();
        let decider = Arc::new(ScriptedDecider::new());
        backend
            .append_event(
                "wf",
                &EventPayload::DeciderConsulted(crate::decider::DeciderConsultation {
                    state: "review".to_string(),
                    visit_seq: 2,
                    provider: "fake".to_string(),
                    model: "m".to_string(),
                    input_sha256: None,
                    outcome: crate::decider::ConsultationOutcome::Applied,
                    error_class: None,
                    latency_ms: 0,
                    directive_bytes: 0,
                    endpoint_origin: SettingOrigin::Default,
                    fields: BTreeMap::new(),
                }),
                "2026-01-01T00:00:01Z",
            )
            .unwrap();
        let mut p = port(&backend, &decider);
        assert!(matches!(consult(&mut p, &tpl), ConsultReply::Skipped));

        let tmp = tempfile::TempDir::new().unwrap();
        let backend = session(tmp.path(), "s");
        backend
            .append_event(
                "wf",
                &EventPayload::Transitioned {
                    from: Some("review".to_string()),
                    to: "done".to_string(),
                    condition_type: "auto".to_string(),
                    skip_if_matched: None,
                },
                "2026-01-01T00:00:01Z",
            )
            .unwrap();
        let mut p = port(&backend, &decider);
        assert!(matches!(consult(&mut p, &tpl), ConsultReply::Skipped));
        assert_eq!(decider.calls(), 0);
    }
}
