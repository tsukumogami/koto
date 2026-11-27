//! The one place a session record leaves the hooks that build it.
//!
//! Every hook hands its records to [`emit`] with the context it already
//! holds. `emit` queues a stamped copy of each record for the event sink,
//! then writes the records to the run journal exactly as they were built.
//! The two halves don't meet: the queue takes copies, so nothing the sender
//! adds reaches the journal, and the journal's lock and its outcome never
//! touch the queue.
//!
//! A hook that called the journal write directly would skip the queue, and
//! one that queued by hand could send what must never be sent. Going
//! through here, a new hook gets both, with the rules below, and can't skip
//! any of them.
//!
//! ## The queued copy
//!
//! - A `context_reads` record is never queued. It is skipped before
//!   anything is cloned, whatever hook built it.
//! - Every kind gains the run's template, `koto.run.template.name` and
//!   `koto.run.template.hash`, each when it has a value.
//! - A `check_rule` gains `koto.check.outcome`, the outcome of the
//!   `check_evaluated` before it in the same batch, so a rule can be read
//!   without its evaluation.
//! - The added fields go at the end of the record's own, so the copy is the
//!   journal's line with a suffix.
//! - A copy longer than the journal's line cap isn't sent. It is counted
//!   as dropped for its session.
//!
//! With no sink installed, or with sending turned off, nothing is queued
//! and no record is cloned: the only cost is one cached check.

use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::engine::types::StateFileHeader;
use crate::session::{lineage, SessionBackend};

use super::{queue, Record};

/// The kind that is never sent.
const NEVER_SENT: &str = "context_reads";

/// The fields only the queued copy carries.
const RUN_TEMPLATE_NAME: &str = "koto.run.template.name";
const RUN_TEMPLATE_HASH: &str = "koto.run.template.hash";
const CHECK_OUTCOME: &str = "koto.check.outcome";

/// What a hook knows about the records it emits.
pub(crate) struct EmitCtx<'a> {
    /// Where the journal is written. `None` when the store has no journal,
    /// or `run_journal.enabled` is false; the records are still queued.
    pub journal_root: Option<&'a Path>,
    /// The directory of the session the records belong to, where a dropped
    /// record is counted.
    pub session_dir: &'a Path,
    /// The run's template name and hash, each when it resolved.
    pub run_template: (Option<String>, Option<String>),
}

/// Where a hook's records can go. `None` means the hook has nothing to
/// do: the store has no journal root and no sender is active, which is
/// what a missing root alone meant before there was a sender. Otherwise
/// `Some(root)` is the journal root to write to, itself `None` when the
/// store has none or `run_journal.enabled` is false. A hook with a root
/// and the journal turned off still runs, so the session sidecar it keeps
/// is written whether or not anything is journaled or queued.
pub(crate) fn destination(journal_root: Option<&Path>) -> Option<Option<&Path>> {
    if journal_root.is_none() && !queue::active() {
        return None;
    }
    Some(journal_root.filter(|_| super::journal_enabled()))
}

/// The run template the queued copies of `session`'s records carry, looked
/// up only when records are being queued.
pub(crate) fn run_template(
    backend: &dyn SessionBackend,
    session: &str,
    header: &StateFileHeader,
) -> (Option<String>, Option<String>) {
    if !queue::active() {
        return (None, None);
    }
    let template = lineage::run_template(backend, session, header);
    (template.name, template.hash)
}

/// Queue a stamped copy of each of `records` for the event sink, then write
/// them to the journal unchanged, waiting at most `lock_wait` for its lock.
pub(crate) fn emit(ctx: &EmitCtx<'_>, records: &[Record], lock_wait: Duration) {
    enqueue(ctx, records);
    if let Some(root) = ctx.journal_root {
        super::write_within(root, records, lock_wait);
    }
}

/// The queue half of [`emit`].
fn enqueue(ctx: &EmitCtx<'_>, records: &[Record]) {
    if !queue::active() {
        return;
    }
    let max = super::max_line_bytes();
    let mut lines = Vec::with_capacity(records.len());
    let mut oversize = 0u64;
    // The outcome of the `check_evaluated` most recently seen in this
    // batch, for the `check_rule` records that follow it.
    let mut outcome: Option<Value> = None;
    for record in records {
        let kind = record.kind();
        if kind == NEVER_SENT {
            continue;
        }
        if kind == "check_evaluated" {
            outcome = record.get(CHECK_OUTCOME).cloned();
        }
        let mut copy = record.clone();
        if let Some(name) = &ctx.run_template.0 {
            copy.fields
                .push((RUN_TEMPLATE_NAME, Value::from(name.as_str())));
        }
        if let Some(hash) = &ctx.run_template.1 {
            copy.fields
                .push((RUN_TEMPLATE_HASH, Value::from(hash.as_str())));
        }
        if kind == "check_rule" {
            if let Some(outcome) = &outcome {
                copy.fields.push((CHECK_OUTCOME, outcome.clone()));
            }
        }
        let line = copy.to_line();
        if line.len() + 1 > max {
            oversize += 1;
        } else {
            lines.push(line);
        }
    }
    queue::push(ctx.session_dir, lines, oversize);
}

#[cfg(test)]
mod tests {
    // Each item is unix-only, as the sender is.
    #[cfg(unix)]
    use super::*;
    #[cfg(unix)]
    use crate::telemetry::sink;

    #[cfg(unix)]
    fn record(kind: &str) -> Record {
        Record::new(kind, "wf", "wf-id", Some("run-id"))
    }

    /// Queue through `emit` with a sink installed for this thread and no
    /// journal root, and return the queued lines.
    #[cfg(unix)]
    fn queued(records: &[Record], template: (Option<&str>, Option<&str>)) -> Vec<Value> {
        let tmp = tempfile::tempdir().unwrap();
        queue::reset_for_test();
        sink::set_sink_for_test(Some(sink::SinkPath::for_test(tmp.path().join("sink"))));
        let ctx = EmitCtx {
            journal_root: None,
            session_dir: tmp.path(),
            run_template: (template.0.map(String::from), template.1.map(String::from)),
        };
        emit(&ctx, records, Duration::ZERO);
        let lines = queue::queued_for_test();
        sink::set_sink_for_test(None);
        queue::reset_for_test();
        lines
            .iter()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[cfg(unix)]
    fn kinds(lines: &[Value]) -> Vec<&str> {
        lines.iter().map(|l| l["kind"].as_str().unwrap()).collect()
    }

    #[cfg(unix)]
    #[test]
    fn context_reads_never_enqueued() {
        let batch = [
            record("state_entered"),
            record("context_reads"),
            record("terminal"),
        ];
        let lines = queued(&batch, (Some("work-on"), Some("abcd")));
        assert_eq!(kinds(&lines), vec!["state_entered", "terminal"]);
        // A batch that holds nothing else queues nothing.
        assert!(queued(&[record("context_reads")], (None, None)).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn every_kind_is_stamped_with_the_run_template_at_the_end() {
        let batch = [record("session_started"), record("response")];
        for line in queued(&batch, (Some("work-on"), Some("abcd"))) {
            assert_eq!(line["koto.run.template.name"], "work-on");
            assert_eq!(line["koto.run.template.hash"], "abcd");
        }
        // The stamps follow the record's own fields: the queued line is the
        // journal's line with a suffix.
        let own = record("terminal");
        let journal_line = own.to_line();
        queue::reset_for_test();
        let tmp = tempfile::tempdir().unwrap();
        sink::set_sink_for_test(Some(sink::SinkPath::for_test(tmp.path().join("sink"))));
        emit(
            &EmitCtx {
                journal_root: None,
                session_dir: tmp.path(),
                run_template: (Some("work-on".into()), Some("abcd".into())),
            },
            std::slice::from_ref(&own),
            Duration::ZERO,
        );
        let sent = queue::queued_for_test().remove(0);
        sink::set_sink_for_test(None);
        queue::reset_for_test();
        assert!(
            sent.starts_with(journal_line.strip_suffix('}').unwrap()),
            "{sent}\n{journal_line}"
        );
        assert!(sent
            .ends_with(r#","koto.run.template.name":"work-on","koto.run.template.hash":"abcd"}"#));
    }

    #[cfg(unix)]
    #[test]
    fn a_stamp_with_no_value_is_left_out() {
        let lines = queued(&[record("terminal")], (Some("work-on"), None));
        assert_eq!(lines[0]["koto.run.template.name"], "work-on");
        assert!(lines[0].get("koto.run.template.hash").is_none());
        let lines = queued(&[record("terminal")], (None, None));
        assert!(lines[0].get("koto.run.template.name").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_check_rule_carries_the_outcome_of_the_evaluation_before_it() {
        let mut passed = record("check_evaluated");
        passed.fields.push((CHECK_OUTCOME, Value::from("passed")));
        let mut failed = record("check_evaluated");
        failed.fields.push((CHECK_OUTCOME, Value::from("failed")));
        let batch = [
            record("check_rule"),
            passed,
            record("check_rule"),
            record("check_finding"),
            failed,
            record("check_rule"),
            record("check_rule"),
        ];
        let lines = queued(&batch, (None, None));
        let outcomes: Vec<Option<&str>> = lines
            .iter()
            .map(|l| l.get("koto.check.outcome").and_then(Value::as_str))
            .collect();
        assert_eq!(
            outcomes,
            vec![
                // A rule with no evaluation before it in the batch.
                None,
                Some("passed"),
                Some("passed"),
                // A finding isn't stamped.
                None,
                Some("failed"),
                Some("failed"),
                Some("failed"),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn nothing_is_queued_without_a_sink() {
        queue::reset_for_test();
        sink::set_sink_for_test(None);
        let tmp = tempfile::tempdir().unwrap();
        emit(
            &EmitCtx {
                journal_root: None,
                session_dir: tmp.path(),
                run_template: (None, None),
            },
            &[record("terminal")],
            Duration::ZERO,
        );
        assert!(queue::queued_for_test().is_empty());
        assert_eq!(destination(None), None);
    }
}
