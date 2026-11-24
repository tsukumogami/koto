//! Resolving and recording a terminal state's declared `result:` map.
//!
//! The compile-time grammar lives in [`crate::template::result_map`]; this
//! module is the run-time half. Two rules shape it:
//!
//! - **One pass, no re-expansion.** Each value is scanned once for `{{VAR}}`
//!   and `${context.<key>}` references, and each resolved value is written
//!   into the output of that single pass. A variable or context value that
//!   itself contains `{{X}}` or `${context.y}` is copied literally, so stored
//!   content can never smuggle in a second lookup.
//! - **Resolved once.** The map is resolved on the first tick that lands in
//!   the terminal, recorded as a `request_store.result` event on the
//!   session's own log, and every later read returns that record.
//!   [`recorded_result_for_current_arrival`] is how a later tick or
//!   `koto status` finds it, so a `koto context add` after the terminal
//!   cannot change what the session reported.

use std::collections::{BTreeMap, HashMap};

use crate::engine::types::{Event, EventPayload, WorkflowResult};
use crate::template::result_map::{result_ref_regex, RESULT_MISSING_KEY};

/// Resolve a declared result map into its payload object.
///
/// `lookup_var` answers a `{{VAR}}` reference; `read_context` answers a
/// `${context.<key>}` reference with the stored bytes. A reference that
/// does not resolve -- an unbound variable, an absent context key, or
/// context content that is not valid UTF-8 -- resolves to the empty string,
/// and the result key it sits in is listed under the reserved
/// [`RESULT_MISSING_KEY`]. That key is present only when the list is
/// non-empty. Resolution never fails: a terminal tick always has a result.
pub fn resolve_result_map(
    declared: &BTreeMap<String, String>,
    lookup_var: impl Fn(&str) -> Option<String>,
    read_context: impl Fn(&str) -> Option<Vec<u8>>,
) -> serde_json::Value {
    let re = result_ref_regex();
    let mut out = serde_json::Map::new();
    let mut missing: Vec<String> = Vec::new();

    for (key, raw) in declared {
        let mut resolved = String::with_capacity(raw.len());
        let mut last = 0;
        let mut unresolved = false;
        for caps in re.captures_iter(raw) {
            let whole = caps.get(0).expect("group 0 always matches");
            resolved.push_str(&raw[last..whole.start()]);
            last = whole.end();
            let value = if let Some(var) = caps.get(1) {
                lookup_var(var.as_str())
            } else if let Some(ctx_key) = caps.get(2) {
                read_context(ctx_key.as_str()).and_then(|bytes| String::from_utf8(bytes).ok())
            } else {
                None
            };
            match value {
                Some(v) => resolved.push_str(&v),
                None => unresolved = true,
            }
        }
        resolved.push_str(&raw[last..]);
        if unresolved {
            missing.push(key.clone());
        }
        out.insert(key.clone(), serde_json::Value::String(resolved));
    }

    if !missing.is_empty() {
        out.insert(
            RESULT_MISSING_KEY.to_string(),
            serde_json::Value::Array(missing.into_iter().map(serde_json::Value::String).collect()),
        );
    }
    serde_json::Value::Object(out)
}

/// The result already recorded for the session's current stay in its
/// state, if any.
///
/// A stay begins at the most recent state-changing event (`transitioned`,
/// `directed_transition`, `rewound`); a `request_store.result` appended
/// after it belongs to this arrival. One recorded before it belongs to an
/// earlier arrival -- a session rewound out of a terminal and landed in one
/// again resolves afresh. A log with no state-changing event at all is one
/// long stay, so any record in it counts.
pub fn recorded_result_for_current_arrival(events: &[Event]) -> Option<WorkflowResult> {
    recorded_for_current_arrival(events).map(|(result, _)| result)
}

/// The `failure_reason` recorded with the current arrival's result, if the
/// arrival has a result and it carries one. Scoped exactly as
/// [`recorded_result_for_current_arrival`].
pub fn recorded_failure_reason_for_current_arrival(events: &[Event]) -> Option<String> {
    recorded_for_current_arrival(events).and_then(|(_, reason)| reason)
}

fn recorded_for_current_arrival(events: &[Event]) -> Option<(WorkflowResult, Option<String>)> {
    for event in events.iter().rev() {
        match &event.payload {
            EventPayload::RequestStoreResult {
                result,
                failure_reason,
            } => return Some((result.clone(), failure_reason.clone())),
            EventPayload::Transitioned { .. }
            | EventPayload::DirectedTransition { .. }
            | EventPayload::Rewound { .. } => return None,
            _ => {}
        }
    }
    None
}

pub use crate::template::types::FAILURE_REASON_KEY;

/// Longest `failure_reason` recorded with a result, in characters. The text
/// is written by an agent or a script and is copied into a parent's gate
/// output, its frozen batch view and its responses, so it is bounded.
pub const FAILURE_REASON_MAX_CHARS: usize = 500;

/// Fold a `failure_reason` onto one line and bound it at
/// [`FAILURE_REASON_MAX_CHARS`] characters, through
/// [`crate::redact::fold_one_line`]: whitespace runs become one space, and a
/// longer reason ends in `...` without splitting a redaction marker. Empty or
/// all-whitespace text is `None`.
pub fn one_line_reason(raw: &str) -> Option<String> {
    crate::redact::fold_one_line(raw, FAILURE_REASON_MAX_CHARS)
}

/// The variable lookup a result map resolves `{{VAR}}` through: the runtime
/// names first, then the bindings folded from the log, matching the order
/// every other substitution in a tick uses.
pub fn var_lookup<'a>(
    runtime_vars: &'a HashMap<String, String>,
    bindings: &'a HashMap<String, String>,
) -> impl Fn(&str) -> Option<String> + 'a {
    move |name| {
        runtime_vars
            .get(name)
            .or_else(|| bindings.get(name))
            .cloned()
    }
}
