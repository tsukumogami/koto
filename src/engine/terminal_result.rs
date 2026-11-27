//! [`failure_reason_for_current_run`], the half of resolving a terminal
//! result that reads the context store. The resolver and the recorded-result
//! readers live in koto-session-reader and are re-exported here.

use crate::engine::types::{Event, EventPayload};
use crate::session::context::ContextStore;

pub use koto_session_reader::engine::terminal_result::*;

/// The `failure_reason` a session wrote during its current run, as it is
/// recorded with a failure terminal's result, or `None` when it wrote none.
///
/// The session's own log names every write to the key: a transition's
/// `context_assignments` carries the value, and `koto context add` or
/// `koto context remove` leaves a `ContextAdded` or `ContextRemoved` event,
/// whose content is read from `context_store`. The latest write since the
/// session's last `Rewound` decides -- the same last-write-wins rule
/// [`crate::engine::context_assign::outstanding_assignments`] applies to the
/// store, cut at the rewind so a reason from a run that `retry_failed` or
/// `koto rewind` restarted is not reported for the new one. A removal or an
/// empty value is no reason. See [`one_line_reason`] for the bound.
pub fn failure_reason_for_current_run(
    context_store: &dyn ContextStore,
    session: &str,
    events: &[Event],
) -> Option<String> {
    // Each event either decides the answer (`Some(value)`, where a `None`
    // value means "no reason") or says nothing about the key (`None`), so
    // the first deciding event from the end wins.
    let raw = events
        .iter()
        .rev()
        .find_map(|e| match &e.payload {
            EventPayload::Rewound { .. } => Some(None),
            EventPayload::Transitioned {
                context_assignments: Some(assignments),
                ..
            } => assignments.get(FAILURE_REASON_KEY).map(|v| Some(v.clone())),
            EventPayload::ContextAdded { key, .. } if key == FAILURE_REASON_KEY => Some(
                context_store
                    .get(session, FAILURE_REASON_KEY)
                    .ok()
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
            ),
            EventPayload::ContextRemoved { key, .. } if key == FAILURE_REASON_KEY => Some(None),
            EventPayload::ContextCleared { keys, .. }
                if keys.iter().any(|k| k == FAILURE_REASON_KEY) =>
            {
                Some(None)
            }
            _ => None,
        })
        .flatten()?;
    one_line_reason(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::{TerminalOutcome, WorkflowResult};
    use std::collections::BTreeMap;

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

    /// A store the code under test must not read.
    struct Untouched;

    impl ContextStore for Untouched {
        fn add(&self, _: &str, _: &str, _: &[u8]) -> anyhow::Result<()> {
            unreachable!("the store was written")
        }
        fn get(&self, _: &str, _: &str) -> anyhow::Result<Vec<u8>> {
            unreachable!("the store was read")
        }
        fn ctx_exists(&self, _: &str, _: &str) -> bool {
            unreachable!("the store was read")
        }
        fn remove(&self, _: &str, _: &str) -> anyhow::Result<()> {
            unreachable!("the store was written")
        }
        fn list_keys(&self, _: &str, _: Option<&str>) -> anyhow::Result<Vec<String>> {
            unreachable!("the store was read")
        }
    }

    #[test]
    fn a_clearing_that_names_the_failure_reason_leaves_no_reason() {
        let ev = |seq: u64, payload: EventPayload| Event {
            seq,
            timestamp: String::new(),
            event_type: payload.type_name().to_string(),
            payload,
            idempotency_hash: None,
        };
        let events = vec![
            ev(
                1,
                EventPayload::ContextAdded {
                    key: FAILURE_REASON_KEY.to_string(),
                    hash: "h".to_string(),
                    size: 1,
                    writer: None,
                },
            ),
            ev(
                2,
                EventPayload::ContextCleared {
                    state: "work".to_string(),
                    keys: vec![FAILURE_REASON_KEY.to_string()],
                    entry_seq: 1,
                },
            ),
        ];
        assert_eq!(
            failure_reason_for_current_run(&Untouched, "s", &events),
            None
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
            failure_reason: None,
        });
        assert_eq!(
            recorded_result_for_current_arrival(&[to_done.clone(), rec.clone()]),
            Some(result)
        );
        assert_eq!(recorded_result_for_current_arrival(&[rec, to_done]), None);
    }
}
