//! The read-side projection of a request log.
//!
//! Every field a consumer reads about a request is derived here by
//! replaying the log. Nothing in this module writes, and nothing it
//! produces is stored: in particular [`LegDisposition`] is computed at
//! replay time and never serialized into an event, which is what makes
//! "the view cannot disagree with the log" structural rather than a
//! discipline someone has to keep
//! (DESIGN-request-lifecycle.md Decision 2).

use std::collections::BTreeMap;

use serde::Serialize;

use crate::engine::request_store::{RequestHeader, RequestStoreError};
use crate::engine::types::{
    CloseDisposition, Event, EventPayload, LegDeclaration, LegDisposition, LegResultSource,
    RequestState, WorkflowResult,
};

/// One mid-flight progress append on a leg.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProgressEntry {
    /// The appending event's own sequence number. This — not the
    /// timestamp — is the ordering key: two appends in the same
    /// millisecond still have distinct sequence numbers.
    pub seq: u64,
    /// RFC 3339 UTC timestamp the append was recorded at.
    pub timestamp: String,
    /// Caller-supplied content, in a [`BTreeMap`] so the serialized
    /// form is byte-stable across reads.
    pub content: BTreeMap<String, serde_json::Value>,
    /// Audit attribution, not authorization: koto has no principal
    /// authentication and this field asserts nothing about identity.
    pub issued_by: Option<String>,
}

/// One leg's projected state.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LegView {
    pub name: String,
    pub declaration: LegDeclaration,
    /// Derived at replay; see the module comment.
    pub disposition: LegDisposition,
    pub bound_child: Option<String>,
    /// The dispatch epoch captured when the leg was bound. The leg's
    /// mutating paths fence against this rather than against the
    /// child's header, which disappears on the child's terminal tick
    /// while this record outlives it.
    ///
    /// Never serialized. koto compares against it; nobody is handed it.
    /// The leg pointer a delegate reads deliberately omits the epoch so
    /// a displaced agent cannot present the current value — and emitting
    /// it in `koto request get` would hand back exactly what the pointer
    /// withholds, one command later. Same-uid readers can still open the
    /// log, so this is confused-agent hygiene rather than a boundary,
    /// but koto should not do the work for them.
    #[serde(skip_serializing)]
    pub bound_epoch: Option<u32>,
    pub result: Option<WorkflowResult>,
    /// Whether the result was promoted from a bound child's terminal
    /// tick or recorded explicitly.
    pub result_source: Option<LegResultSource>,
    /// The rationale recorded when the leg was abandoned. Read by the
    /// abandonment notice, which needs the verbatim text.
    pub abandoned_rationale: Option<String>,
    pub progress: Vec<ProgressEntry>,
}

impl LegView {
    fn new(name: String, declaration: LegDeclaration) -> Self {
        Self {
            name,
            declaration,
            disposition: LegDisposition::Open,
            bound_child: None,
            bound_epoch: None,
            result: None,
            result_source: None,
            abandoned_rationale: None,
            progress: Vec::new(),
        }
    }
}

/// How many legs sit in each disposition.
///
/// Every surface that reports on a request reports these four numbers,
/// so they are derived once here rather than recounted per caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LegCounts {
    pub total: usize,
    pub open: usize,
    pub resolved: usize,
    pub abandoned: usize,
}

/// A request log replayed into readable state.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RequestView {
    pub header: RequestHeader,
    pub request_state: RequestState,
    pub close_disposition: Option<CloseDisposition>,
    /// The request-level shared context recorded at creation. Present
    /// here because the creation path accepts shared inputs, and
    /// accepting something that can never be read back would make it
    /// write-only.
    pub inputs: Option<serde_json::Value>,
    /// Legs in canonical (name) order, so two reads of an unchanged
    /// request serialize byte-identically.
    pub legs: BTreeMap<String, LegView>,
    /// The sequence number of the last event on the log. Derived
    /// rather than counted: a count can decrease if history is ever
    /// compacted, and monotonicity is already enforced by the reader's
    /// sequence-gap check (DESIGN-request-lifecycle.md Decision 2).
    pub revision: u64,
}

impl RequestView {
    /// Count legs by disposition.
    pub fn leg_counts(&self) -> LegCounts {
        let mut counts = LegCounts {
            total: self.legs.len(),
            open: 0,
            resolved: 0,
            abandoned: 0,
        };
        for leg in self.legs.values() {
            match leg.disposition {
                LegDisposition::Open => counts.open += 1,
                LegDisposition::Resolved => counts.resolved += 1,
                LegDisposition::Abandoned => counts.abandoned += 1,
            }
        }
        counts
    }

    /// Borrow one leg, or report it missing.
    pub fn leg(&self, leg_name: &str) -> Result<&LegView, RequestStoreError> {
        self.legs
            .get(leg_name)
            .ok_or_else(|| RequestStoreError::LegNotFound {
                request_id: self.header.request_id.clone(),
                leg_name: leg_name.to_string(),
            })
    }
}

/// Replay `events` onto `header`.
///
/// Unrecognized payloads — including the `Unknown` fallthrough a newer
/// koto's events land in — are skipped rather than rejected, which is
/// what keeps an older build's read of a newer log a graceful
/// degradation instead of a hard error.
pub fn project(header: RequestHeader, events: &[Event]) -> Result<RequestView, RequestStoreError> {
    let request_id = header.request_id.clone();
    let mut view = RequestView {
        header,
        request_state: RequestState::Open,
        close_disposition: None,
        inputs: None,
        legs: BTreeMap::new(),
        revision: 0,
    };
    let mut seen_created = false;

    for event in events {
        view.revision = event.seq;

        // Guard against two logs having been interleaved into one
        // file — a case flock cannot prevent over a network
        // filesystem, and one that would otherwise project as a
        // plausible-looking request built from another's events.
        if let Some(event_request_id) = payload_request_id(&event.payload) {
            if event_request_id != request_id {
                return Err(RequestStoreError::Corrupt {
                    request_id: request_id.clone(),
                    reason: format!(
                        "event at seq {} belongs to request '{}'",
                        event.seq, event_request_id
                    ),
                });
            }
        }

        match &event.payload {
            EventPayload::RequestCreated { legs, inputs, .. } => {
                if seen_created {
                    return Err(RequestStoreError::Corrupt {
                        request_id: request_id.clone(),
                        reason: format!("a second request.created event at seq {}", event.seq),
                    });
                }
                seen_created = true;
                view.inputs = inputs.clone();
                for (name, declaration) in legs {
                    view.legs.insert(
                        name.clone(),
                        LegView::new(name.clone(), declaration.clone()),
                    );
                }
            }
            EventPayload::RequestLegBound {
                leg_name,
                child_session_id,
                dispatch_epoch,
                ..
            } => {
                let leg = leg_mut(&mut view.legs, &request_id, leg_name, event.seq)?;
                leg.bound_child = Some(child_session_id.clone());
                leg.bound_epoch = *dispatch_epoch;
            }
            EventPayload::RequestLegProgress {
                leg_name,
                content,
                issued_by,
                ..
            } => {
                let leg = leg_mut(&mut view.legs, &request_id, leg_name, event.seq)?;
                leg.progress.push(ProgressEntry {
                    seq: event.seq,
                    timestamp: event.timestamp.clone(),
                    content: content.clone(),
                    issued_by: issued_by.clone(),
                });
            }
            EventPayload::RequestLegResult {
                leg_name,
                result,
                source,
                ..
            } => {
                let leg = leg_mut(&mut view.legs, &request_id, leg_name, event.seq)?;
                leg.result = Some(result.clone());
                leg.result_source = Some(*source);
                // Abandonment wins if it got there first: the write
                // path rejects a result on an abandoned leg, so a log
                // carrying both can only be one written by a build
                // whose rules differed.
                if leg.disposition == LegDisposition::Open {
                    leg.disposition = LegDisposition::Resolved;
                }
            }
            EventPayload::RequestLegAbandoned {
                leg_name,
                rationale,
                ..
            } => {
                let leg = leg_mut(&mut view.legs, &request_id, leg_name, event.seq)?;
                leg.abandoned_rationale = Some(rationale.clone());
                leg.disposition = LegDisposition::Abandoned;
            }
            EventPayload::RequestClosed { disposition, .. } => {
                view.request_state = RequestState::Closed;
                view.close_disposition = Some(*disposition);
            }
            // Any other payload on a request log is either a newer
            // koto's event landing in `Unknown` or a session-family
            // event that has no business here. Both are skipped.
            _ => {}
        }
    }

    if !seen_created {
        return Err(RequestStoreError::Corrupt {
            request_id,
            reason: "no request.created event on the log".to_string(),
        });
    }

    Ok(view)
}

/// Borrow a declared leg for mutation, or report the log inconsistent.
///
/// A leg event naming a leg the creation event never declared means
/// the log disagrees with itself; there is no sensible projection, so
/// it is corruption rather than something to skip.
fn leg_mut<'a>(
    legs: &'a mut BTreeMap<String, LegView>,
    request_id: &str,
    leg_name: &str,
    seq: u64,
) -> Result<&'a mut LegView, RequestStoreError> {
    legs.get_mut(leg_name)
        .ok_or_else(|| RequestStoreError::Corrupt {
            request_id: request_id.to_string(),
            reason: format!("event at seq {seq} names undeclared leg '{leg_name}'"),
        })
}

/// The `request_id` carried by any of the six request payloads.
fn payload_request_id(payload: &EventPayload) -> Option<&str> {
    match payload {
        EventPayload::RequestCreated { request_id, .. }
        | EventPayload::RequestLegBound { request_id, .. }
        | EventPayload::RequestLegProgress { request_id, .. }
        | EventPayload::RequestLegResult { request_id, .. }
        | EventPayload::RequestLegAbandoned { request_id, .. }
        | EventPayload::RequestClosed { request_id, .. } => Some(request_id),
        _ => None,
    }
}
