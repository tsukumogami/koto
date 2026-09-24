//! A root session attaching itself to a request leg.
//!
//! `bind` admits only a dispatched child, whose leg the dispatch epoch
//! fences. A root session — one with no parent workflow — has no epoch,
//! so admitting it needs a different boundary, and this module is that
//! boundary (DESIGN-request-lifecycle.md, root attach amendment):
//!
//! - the session must not be terminal or cancelled;
//! - its template identity must match an entry the leg's `template`
//!   names ([`LegTemplates::admits`] states the rule);
//! - each of the leg's `inputs` must name a variable the session's
//!   template declares, and, unless that variable is `rebind: true`,
//!   must equal the session's recorded value;
//! - the leg must be open and unbound, or already bound to this very
//!   session (a no-op success);
//! - a session whose pointer names another leg is re-pointed only when
//!   that leg was abandoned or its request closed, so one run can't take
//!   over another live run's leg.
//!
//! Every check runs inside the request lock, in [`attach_leg`]'s
//! decision closure, so an attach racing a concurrent bind or abandon
//! can't slip past a check made on an unlocked read. The session facts
//! it checks against are gathered by the caller beforehand and handed in
//! as an [`AttachingSession`], which keeps the store free of session
//! backends and lets `koto init --koto-leg` run the same checks on a
//! session it is about to create.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use super::{
    append_under_lock, read_view, reject_closed_leg, require_open, validate_leg_name, AppendResult,
    PendingAppend, RequestStoreError, RequestView, ValidatedRequestId, LOCK_WAIT_TIMEOUT,
};
use crate::engine::leg_pointer::LegPointer;
use crate::engine::persistence::derive_state_from_log;
use crate::engine::request_store::view::LegView;
use crate::engine::substitute::bindings_from_events;
use crate::engine::types::{
    Event, EventPayload, LegAttach, LegDisposition, RequestState, StateFileHeader, TemplateIdentity,
};
use crate::template::types::{CompiledTemplate, VariableDecl};

/// What the admission checks need to know about the attaching session.
#[derive(Debug, Clone, PartialEq)]
pub struct AttachingSession {
    pub session_id: String,
    /// `None` when the session has no source template file, which no
    /// leg admits.
    pub template: Option<TemplateIdentity>,
    /// The variables the session's template declares.
    pub variables: BTreeMap<String, VariableDecl>,
    /// The session's current variable bindings, folded from its log.
    pub bindings: HashMap<String, String>,
    /// The state name when the session is terminal, or `"cancelled"`
    /// when it was cancelled; `None` while it is live.
    pub terminal_state: Option<String>,
    /// The leg pointer the session carries today, if any.
    pub pointer: Option<LegPointer>,
}

impl AttachingSession {
    /// Gather the facts from a session's header, log, compiled template
    /// and pointer.
    pub fn from_session(
        session_id: &str,
        header: &StateFileHeader,
        events: &[Event],
        compiled: &CompiledTemplate,
        pointer: Option<LegPointer>,
    ) -> Self {
        let template = header
            .template_source_file
            .as_ref()
            .map(|source| TemplateIdentity {
                name: header
                    .template_name
                    .clone()
                    .or_else(|| (!compiled.name.is_empty()).then(|| compiled.name.clone())),
                hash: header.template_hash.clone(),
                source: source.clone(),
            });
        let cancelled = events
            .iter()
            .any(|e| matches!(e.payload, EventPayload::WorkflowCancelled { .. }));
        let current = derive_state_from_log(events);
        let terminal_state = if cancelled {
            Some("cancelled".to_string())
        } else {
            current.filter(|state| {
                compiled
                    .states
                    .get(state)
                    .map(|s| s.terminal)
                    .unwrap_or(false)
            })
        };
        Self {
            session_id: session_id.to_string(),
            template,
            variables: compiled.variables.clone(),
            bindings: bindings_from_events(events),
            terminal_state,
            pointer,
        }
    }
}

/// One attach request.
#[derive(Debug, Clone, PartialEq)]
pub struct AttachLeg {
    pub leg_name: String,
    pub session: AttachingSession,
    pub issued_by: Option<String>,
    pub timestamp: String,
}

/// Check the session's own facts against the leg: terminal state,
/// template identity, then inputs. Writes nothing.
///
/// Public so a caller can run every session-side check before it
/// creates or changes anything; [`attach_leg`] runs it again under the
/// lock.
pub fn check_session_against_leg(
    request_id: &str,
    leg: &LegView,
    session: &AttachingSession,
) -> Result<(), RequestStoreError> {
    if let Some(state) = &session.terminal_state {
        return Err(RequestStoreError::SessionTerminal {
            session_id: session.session_id.clone(),
            state: state.clone(),
        });
    }

    if !leg.declaration.template.admits(session.template.as_ref()) {
        return Err(RequestStoreError::TemplateMismatch {
            request_id: request_id.to_string(),
            leg_name: leg.name.clone(),
            session_id: session.session_id.clone(),
            session_template: session.template.as_ref().map(|t| t.source.clone()),
            leg_templates: leg
                .declaration
                .template
                .entries()
                .iter()
                .map(|s| s.to_string())
                .collect(),
        });
    }

    // Only an object can name variables. A leg whose inputs are a bare
    // brief (a string, or absent) states no variable expectations, and
    // the template check above is what admits the session.
    let Some(inputs) = leg.declaration.inputs.as_object() else {
        return Ok(());
    };
    // Sorted so the first mismatch reported doesn't depend on map order.
    let mut keys: Vec<&String> = inputs.keys().collect();
    keys.sort();
    for key in keys {
        let expected_value = &inputs[key];
        let expected = input_as_string(expected_value);
        let mismatch = |recorded: Option<String>| RequestStoreError::InputMismatch {
            request_id: request_id.to_string(),
            leg_name: leg.name.clone(),
            key: key.clone(),
            recorded,
            expected: expected
                .clone()
                .unwrap_or_else(|| expected_value.to_string()),
        };
        let Some(decl) = session.variables.get(key) else {
            return Err(mismatch(None));
        };
        // A rebind variable is a per-invocation setting, re-applied on
        // every attach; it is not part of what the leg pins.
        if decl.rebind {
            continue;
        }
        let recorded = session.bindings.get(key).cloned().unwrap_or_default();
        if expected.as_deref() != Some(recorded.as_str()) {
            return Err(mismatch(Some(recorded)));
        }
    }
    Ok(())
}

/// The string a leg input compares equal to. Variables are strings, so
/// a string input compares as itself and a number or boolean as its
/// JSON text; anything else can never equal a variable.
fn input_as_string(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => Some(value.to_string()),
        _ => None,
    }
}

/// Whether the leg a session's pointer names has been let go of: it was
/// abandoned, its request closed, or it no longer exists at all.
///
/// Read inside the attaching request's lock. When the pointer names a
/// different request, that request's lock is not taken — taking two
/// request locks would need an ordering every writer agreed on — but
/// both conditions that release a pointer are monotonic: an abandoned
/// leg never reopens and a closed request never reopens, so a "released"
/// answer read here can't be invalidated by a concurrent writer.
fn pointer_released(
    root: &Path,
    attaching: &RequestView,
    pointer: &LegPointer,
) -> Result<bool, RequestStoreError> {
    let leg_released = |view: &RequestView| {
        view.request_state == RequestState::Closed
            || view
                .legs
                .get(&pointer.leg_name)
                .map(|leg| leg.disposition == LegDisposition::Abandoned)
                .unwrap_or(true)
    };
    if pointer.request_id == attaching.header.request_id {
        return Ok(leg_released(attaching));
    }
    let Ok(other) = ValidatedRequestId::new(&pointer.request_id) else {
        // A pointer naming no possible request can't hold a live leg.
        return Ok(true);
    };
    match read_view(root, &other) {
        Ok(view) => Ok(leg_released(&view)),
        Err(RequestStoreError::NotFound { .. }) => Ok(true),
        Err(e) => Err(e),
    }
}

/// Attach a root session to a leg.
///
/// Every check in the module comment runs inside the lock, against the
/// freshly re-read view, before the `request.leg_bound` event is
/// appended. Attaching a session to the leg it is already bound to is a
/// no-op success (`written: false`). The event records
/// `attach: self` and the session's template identity, and no dispatch
/// epoch: a self-attached leg is never fenced at an epoch, it refuses
/// the fenced verbs outright.
///
/// The caller writes the session's leg pointer after this returns, as
/// `bind` does, overwriting a released one.
pub fn attach_leg(
    root: &Path,
    request_id: &ValidatedRequestId,
    attach: &AttachLeg,
) -> Result<AppendResult, RequestStoreError> {
    validate_leg_name(&attach.leg_name)?;
    append_under_lock(root, request_id, LOCK_WAIT_TIMEOUT, None, |view| {
        admit(root, view, attach)
    })
}

/// Run every admission check [`attach_leg`] runs, against an unlocked
/// read of the request, writing nothing.
///
/// Returns `true` when the leg is already bound to this session (the
/// attach would be a no-op). Used by `koto init --koto-leg`, which must
/// know an attach would be admitted before it creates, replaces or
/// rebinds anything; [`attach_leg`] re-runs the same checks under the
/// lock, so a write that raced this read is still refused there.
pub fn precheck_attach(
    root: &Path,
    request_id: &ValidatedRequestId,
    attach: &AttachLeg,
) -> Result<bool, RequestStoreError> {
    validate_leg_name(&attach.leg_name)?;
    let view = read_view(root, request_id)?;
    Ok(admit(root, &view, attach)?.is_none())
}

/// The admission decision shared by [`attach_leg`] (under the lock) and
/// [`precheck_attach`] (without it): the bind event to append, `None`
/// for a no-op re-attach, or the refusal.
fn admit(
    root: &Path,
    view: &RequestView,
    attach: &AttachLeg,
) -> Result<Option<PendingAppend>, RequestStoreError> {
    let session = &attach.session;
    require_open(view)?;
    let leg = view.leg(&attach.leg_name)?;
    reject_closed_leg(view, leg)?;
    check_session_against_leg(&view.header.request_id, leg, session)?;

    if let Some(bound) = &leg.bound_child {
        if bound == &session.session_id {
            return Ok(None);
        }
        return Err(RequestStoreError::LegBoundToDifferentChild {
            request_id: view.header.request_id.clone(),
            leg_name: attach.leg_name.clone(),
            bound_child: bound.clone(),
            requested_child: session.session_id.clone(),
        });
    }

    if let Some(pointer) = &session.pointer {
        if pointer.names_a_different_leg(&view.header.request_id, &attach.leg_name)
            && !pointer_released(root, view, pointer)?
        {
            return Err(RequestStoreError::SessionBoundToDifferentLeg {
                session_id: session.session_id.clone(),
                request_id: pointer.request_id.clone(),
                leg_name: pointer.leg_name.clone(),
            });
        }
    }

    Ok(Some(PendingAppend {
        payload: EventPayload::RequestLegBound {
            request_id: view.header.request_id.clone(),
            leg_name: attach.leg_name.clone(),
            child_session_id: session.session_id.clone(),
            dispatch_epoch: None,
            issued_by: attach.issued_by.clone(),
            attach: Some(LegAttach::SelfAttached),
            template: session.template.clone(),
        },
        timestamp: attach.timestamp.clone(),
        hash: None,
    }))
}
