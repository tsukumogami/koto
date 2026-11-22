//! Where a session came from: the read behind `koto session origin`
//! (DESIGN-koto-session-dispatcher-origin.md, Decision 3).
//!
//! An anchor's origin is what its creating `anchor_started` event recorded
//! (the `origin` field `koto session anchor open` writes), taken together
//! with the `origin_unrecorded` event an open appends when the dispatched-by
//! link it owed could not be written, and with the link itself. A workflow
//! session's origin comes from a dispatched-by link of its own when it has
//! one (written by `koto init --dispatched-by` or `koto session link`), and
//! otherwise from the anchor that held it when it was opened.
//!
//! Everything here reads through the store's own methods ([`SessionBackend`]
//! and [`ContextStore`]), so under the cloud backend it reads what `koto
//! status` reads, and none of it writes: no event, no key, no link.

use serde::Serialize;

use crate::engine::types::{AnchorOrigin, AnchorStarted, EventPayload, SessionKind};
use crate::session::anchor::named_preview;
use crate::session::context::ContextStore;
use crate::session::links::{self, Link, LinkKind};
use crate::session::SessionBackend;

/// A held-by link names an anchor the store doesn't hold (it was removed
/// after the link was written), or a session that isn't an anchor.
pub const REASON_ANCHOR_NOT_FOUND: &str = "anchor not found";
/// The creating event names a dispatcher, the anchor holds no dispatched-by
/// link to it, and no `origin_unrecorded` event says why.
pub const REASON_LINK_NOT_FOUND: &str = "link not found";
/// An anchor written before koto recorded origin whose creating event names
/// no dispatcher. Its dispatcher may have been lost, so it isn't shown as a
/// root.
pub const REASON_LEGACY: &str = "recorded before koto tracked origin";

/// What `dispatcher` names in a `dispatched` answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatcherKind {
    /// The answer came through an anchor: the dispatcher is the anchor of
    /// the agent session that started it.
    Anchor,
    /// The answer came from a dispatched-by link on the workflow session
    /// itself, as `koto init --dispatched-by` or `koto session link` write
    /// it: the dispatcher is whatever session that link names.
    Workflow,
}

/// A session's origin, in one of three states.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Origin {
    /// The starting environment named `dispatcher`, and its link is there.
    /// An unauthenticated claim: any process in that environment could have
    /// set the name.
    Dispatched {
        dispatcher: String,
        dispatcher_kind: DispatcherKind,
    },
    /// Nothing named a dispatcher.
    #[serde(rename = "none")]
    NoDispatcher,
    /// A dispatcher may have been named and wasn't recorded. `named` is the
    /// bounded preview of what was named, empty when nothing was; `reason`
    /// is a fixed text.
    Unrecorded { named: String, reason: String },
}

impl Origin {
    fn unrecorded(named: impl Into<String>, reason: impl Into<String>) -> Self {
        Origin::Unrecorded {
            named: named.into(),
            reason: reason.into(),
        }
    }

    /// The `state` value: `dispatched`, `none` or `unrecorded`.
    pub fn state(&self) -> &'static str {
        match self {
            Origin::Dispatched { .. } => "dispatched",
            Origin::NoDispatcher => "none",
            Origin::Unrecorded { .. } => "unrecorded",
        }
    }
}

/// The answer `koto session origin` prints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OriginReport {
    /// The session asked about.
    pub session: String,
    /// The anchor the answer came from: the session itself when it is an
    /// anchor, the anchor its held-by link names otherwise, and `null` when
    /// no anchor was read (a workflow session's own dispatched-by link, or
    /// no link at all).
    pub via: Option<String>,
    pub origin: Origin,
}

/// Why no origin could be given at all, which is not one of the three
/// states: the name isn't a session name, or no session holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OriginRefusal {
    InvalidName(String),
    NoSuchSession,
}

impl std::fmt::Display for OriginRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OriginRefusal::InvalidName(e) => write!(f, "invalid session name: {e}"),
            OriginRefusal::NoSuchSession => f.write_str("the session does not exist"),
        }
    }
}

impl std::error::Error for OriginRefusal {}

/// Report where `name` came from.
///
/// Refuses with an [`OriginRefusal`] a name that fails the session-name rule
/// and a name with no session. A read that fails for any other reason (an
/// unreachable store, a log that can't be parsed) is returned as is.
pub fn origin_of<S>(store: &S, name: &str) -> anyhow::Result<OriginReport>
where
    S: SessionBackend + ContextStore,
{
    crate::discover::validate_workflow_name(name).map_err(OriginRefusal::InvalidName)?;
    if !store.exists(name) {
        return Err(OriginRefusal::NoSuchSession.into());
    }
    let header = store.read_header(name)?;
    if header.session_kind() == SessionKind::Anchor {
        return Ok(OriginReport {
            session: name.to_string(),
            via: Some(name.to_string()),
            origin: anchor_origin(store, name)?,
        });
    }

    let held = links::read_links(store, name)?;
    if let Some(link) = earliest(&held, LinkKind::DispatchedBy) {
        return Ok(OriginReport {
            session: name.to_string(),
            via: None,
            origin: Origin::Dispatched {
                dispatcher: link.peer.clone(),
                dispatcher_kind: DispatcherKind::Workflow,
            },
        });
    }
    let Some(link) = earliest(&held, LinkKind::HeldBy) else {
        return Ok(OriginReport {
            session: name.to_string(),
            via: None,
            origin: Origin::NoDispatcher,
        });
    };
    let anchor = link.peer.clone();
    // Only an anchor the store doesn't hold, or a session that isn't an
    // anchor, reads as not found. A header that exists and can't be read is a
    // failed read, returned as one, so an unreachable store is never shown
    // as a lost dispatcher.
    let is_anchor =
        store.exists(&anchor) && store.read_header(&anchor)?.session_kind() == SessionKind::Anchor;
    let origin = if is_anchor {
        anchor_origin(store, &anchor)?
    } else {
        Origin::unrecorded("", REASON_ANCHOR_NOT_FOUND)
    };
    Ok(OriginReport {
        session: name.to_string(),
        via: Some(anchor),
        origin,
    })
}

/// The `kind` link `links` holds that was written first: the earliest `at`,
/// and among links whose `at` is equal or can't be read, the lowest peer
/// name. A link with a readable `at` comes before one without, so a body
/// damaged later can't displace the link written when the session opened.
fn earliest(links: &[Link], kind: LinkKind) -> Option<&Link> {
    links.iter().filter(|l| l.kind == kind).min_by(|a, b| {
        let at = |l: &Link| {
            l.body
                .as_ref()
                .and_then(|b| crate::engine::wake::parse_rfc3339_millis(&b.at))
        };
        match (at(a), at(b)) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
        .then_with(|| a.peer.cmp(&b.peer))
    })
}

/// The origin anchor `anchor` records.
///
/// The creating event is the first `anchor_started` the open that created
/// the anchor wrote (`opened` is `new`, `reopened` or `successor`); a
/// `continued` event never supplies it, since a continue writes no origin
/// and its `dispatched_by` names whatever a later start inherited.
///
/// A creating event that names a dispatcher counts as `dispatched` only when
/// the anchor holds a dispatched-by link to that same name. Checking that one
/// link, and no other, is what keeps a successor's dispatched-by link to its
/// predecessor from being read as its dispatcher's. Without the link the
/// answer is `unrecorded`, with the reason an `origin_unrecorded` event gives
/// when there is one, and [`REASON_LINK_NOT_FOUND`] otherwise: the event is
/// appended after the link write fails, best effort, so it can be missing.
fn anchor_origin<S>(store: &S, anchor: &str) -> anyhow::Result<Origin>
where
    S: SessionBackend + ContextStore,
{
    let (_, events) = store.read_events(anchor)?;
    let mut creating: Option<&AnchorStarted> = None;
    let mut unrecorded = None;
    for event in &events {
        match &event.payload {
            EventPayload::AnchorStarted(s) if creating.is_none() && s.opened != "continued" => {
                creating = Some(s)
            }
            EventPayload::OriginUnrecorded(o) if unrecorded.is_none() => unrecorded = Some(o),
            _ => {}
        }
    }

    let named = match creating.and_then(|s| s.origin.as_ref()) {
        Some(AnchorOrigin::NoDispatcher) => return Ok(Origin::NoDispatcher),
        Some(AnchorOrigin::Unrecorded { named, reason }) => {
            return Ok(Origin::unrecorded(named.clone(), reason.clone()))
        }
        Some(AnchorOrigin::Dispatched { dispatcher }) => dispatcher.clone(),
        // Recorded before koto wrote `origin`: the legacy field is the only
        // record of who was named, and an event naming nobody can't be told
        // apart from one whose dispatcher was lost. A named one goes through
        // the same link check below: the older koto wrote the link after the
        // event too, so the event alone never proves the link landed.
        None => match creating.and_then(|s| s.dispatched_by.as_ref()) {
            Some(d) => d.clone(),
            None => return Ok(Origin::unrecorded("", REASON_LEGACY)),
        },
    };

    if let Some(o) = unrecorded {
        return Ok(Origin::unrecorded(o.named.clone(), o.reason.clone()));
    }
    let linked = links::validate_peer(LinkKind::DispatchedBy, &named).is_ok()
        && store.ctx_exists(anchor, &links::link_key(LinkKind::DispatchedBy, &named));
    if linked {
        Ok(Origin::Dispatched {
            dispatcher: named,
            dispatcher_kind: DispatcherKind::Anchor,
        })
    } else {
        Ok(Origin::unrecorded(
            named_preview(&named),
            REASON_LINK_NOT_FOUND,
        ))
    }
}

#[cfg(test)]
mod tests;
