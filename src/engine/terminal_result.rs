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
    for event in events.iter().rev() {
        match &event.payload {
            EventPayload::RequestStoreResult { result } => return Some(result.clone()),
            EventPayload::Transitioned { .. }
            | EventPayload::DirectedTransition { .. }
            | EventPayload::Rewound { .. } => return None,
            _ => {}
        }
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::TerminalOutcome;

    fn declared(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn vars(name: &str) -> Option<String> {
        (name == "TOPIC").then(|| "my-topic".to_string())
    }

    #[test]
    fn resolves_literals_vars_and_context() {
        let d = declared(&[
            ("outcome", "error"),
            ("topic", "{{TOPIC}}"),
            ("step", "${context.step}"),
            ("state", "merge-state:${context.state}"),
        ]);
        let v = resolve_result_map(&d, vars, |k| match k {
            "step" => Some(b"scope:push".to_vec()),
            "state" => Some(b"open".to_vec()),
            _ => None,
        });
        assert_eq!(
            v,
            serde_json::json!({
                "outcome": "error",
                "topic": "my-topic",
                "step": "scope:push",
                "state": "merge-state:open",
            })
        );
    }

    #[test]
    fn absent_context_resolves_empty_and_is_listed_missing() {
        let d = declared(&[("pr", "${context.absent}"), ("ok", "x")]);
        let v = resolve_result_map(&d, vars, |_| None);
        assert_eq!(
            v,
            serde_json::json!({"pr": "", "ok": "x", "missing": ["pr"]})
        );
    }

    #[test]
    fn non_utf8_context_is_missing() {
        let d = declared(&[("pr", "pre-${context.bin}")]);
        let v = resolve_result_map(&d, vars, |_| Some(vec![0xff, 0xfe]));
        assert_eq!(v, serde_json::json!({"pr": "pre-", "missing": ["pr"]}));
    }

    #[test]
    fn resolved_values_are_not_re_expanded() {
        let d = declared(&[("a", "${context.x}"), ("b", "{{TOPIC}}")]);
        let v = resolve_result_map(
            &d,
            |n| (n == "TOPIC").then(|| "${context.y}".to_string()),
            |k| match k {
                "x" => Some(b"{{TOPIC}}".to_vec()),
                "y" => Some(b"never".to_vec()),
                _ => None,
            },
        );
        assert_eq!(
            v,
            serde_json::json!({"a": "{{TOPIC}}", "b": "${context.y}"})
        );
    }

    #[test]
    fn recorded_result_scoped_to_current_arrival() {
        let result = WorkflowResult {
            status: TerminalOutcome::Success,
            summary: "s".into(),
            payload: None,
        };
        let ev = |payload: EventPayload| Event {
            seq: 0,
            timestamp: String::new(),
            event_type: payload.type_name().to_string(),
            payload,
            idempotency_hash: None,
        };
        let to_done = ev(EventPayload::Rewound {
            from: "a".into(),
            to: "done".into(),
            rationale: None,
        });
        let rec = ev(EventPayload::RequestStoreResult {
            result: result.clone(),
        });
        assert_eq!(
            recorded_result_for_current_arrival(&[to_done.clone(), rec.clone()]),
            Some(result)
        );
        assert_eq!(recorded_result_for_current_arrival(&[rec, to_done]), None);
    }
}
