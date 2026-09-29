//! Polling command gates (DESIGN-koto-ci-wait-stale-keys.md, Decisions 4-6).
//!
//! A command gate that declares `poll:` can answer "not yet": its command
//! exits with the pending code while the check it watches hasn't settled.
//! After the advance loop has evaluated a state's gates once, [`settle`]
//! re-runs the pending polling gates every `interval_secs` for at most
//! `hold_secs` of this tick, never starting a run past the gate's deadline,
//! and then classifies each polling gate's last run: done, pending, failed or
//! timed out.
//!
//! It lives beside the advance loop rather than inside it because the loop
//! only needs its outcome: the loop passes in the tick's log and the shutdown
//! flag, and `settle` owns the waiting. Several polling gates in one state
//! share one hold but keep their own schedules: each is re-run one interval
//! after its last run started, and only the gates that are due run.
//!
//! The polling window opens at the first run of the gate since the latest
//! entry into the state and is recorded on every logged evaluation as
//! `poll.since`, so it spans ticks and a new entry opens a new one. A pending
//! result judged nothing: it carries no `failure`, so no fallback finding,
//! and the advance loop logs it with no attempt stamp and no rule counts.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use crate::engine::persistence::any_entry_index;
use crate::engine::types::{iso8601_at, Event, EventPayload};
use crate::engine::wake::parse_rfc3339_millis;
use crate::findings::MessageSource;
use crate::gate::{GateOutcome, PollReport, PollStatus, StructuredGateResult};
use crate::template::types::{Gate, PollSpec};

/// The polling window a gate already has in the current epoch: its recorded
/// start and the runs counted so far. `(None, 0)` when no evaluation of the
/// gate has been recorded since the latest entry into `state`.
///
/// `state` must be the state the workflow occupies, as for
/// [`any_entry_index`].
pub(crate) fn recorded_window(events: &[Event], state: &str, gate: &str) -> (Option<String>, u64) {
    let start = any_entry_index(events, state).map_or(0, |i| i + 1);
    let mut since = None;
    let mut runs = 0;
    for e in &events[start..] {
        if let EventPayload::GateEvaluated {
            state: s,
            gate: g,
            poll: Some(p),
            ..
        } = &e.payload
        {
            if s == state && g == gate {
                if since.is_none() {
                    since = Some(p.since.clone());
                }
                runs = p.evaluations;
            }
        }
    }
    (since, runs)
}

/// Whether a command gate's result is its command's pending answer: a plain
/// non-zero exit with the pending code. A run killed by the gate's own
/// timeout or one that couldn't start carries a `failure_kind` and is never
/// pending.
pub(crate) fn is_pending(result: &StructuredGateResult, spec: &PollSpec) -> bool {
    result.outcome == GateOutcome::Failed
        && result.output.get("failure_kind").is_none()
        && result.output.get("exit_code").and_then(|v| v.as_i64())
            == Some(i64::from(spec.pending_exit_code))
}

/// One polling gate's window during this tick.
struct Window {
    since: SystemTime,
    since_text: String,
    runs: u64,
}

/// Hold, re-evaluate and classify this state's polling gates.
///
/// `results` holds the state's first evaluation of `gates`, taken just after
/// `first_run_start`; `events` is the log as the tick sees it. Only pending
/// polling gates are re-run, through the same `evaluate` closure, and a
/// re-run the evaluator refuses ends the hold with the results in hand. The
/// shutdown flag is checked every 100 ms of every wait.
pub fn settle<G, E>(
    results: &mut BTreeMap<String, StructuredGateResult>,
    gates: &BTreeMap<String, Gate>,
    events: &[Event],
    state: &str,
    first_run_start: SystemTime,
    evaluate: &G,
    shutdown: &AtomicBool,
) where
    G: Fn(&BTreeMap<String, Gate>) -> Result<BTreeMap<String, StructuredGateResult>, E>,
{
    let polling: Vec<(&String, &PollSpec)> = gates
        .iter()
        .filter_map(|(name, gate)| gate.poll.as_ref().map(|spec| (name, spec)))
        .filter(|(name, _)| results.contains_key(*name))
        .collect();
    if polling.is_empty() {
        return;
    }

    let mut windows: BTreeMap<&str, Window> = BTreeMap::new();
    for (name, _) in &polling {
        let (recorded, prior_runs) = recorded_window(events, state, name);
        let parsed = recorded
            .as_deref()
            .and_then(|t| parse_rfc3339_millis(t).map(|at| (at, t.to_string())));
        let (since, since_text) =
            parsed.unwrap_or_else(|| (first_run_start, iso8601_at(first_run_start)));
        windows.insert(
            name.as_str(),
            Window {
                since,
                since_text,
                runs: prior_runs + 1,
            },
        );
    }

    // Each gate keeps its own schedule: it is next due one interval after its
    // last run started, and a wait lasts until the earliest due gate, so a
    // slow-interval gate is never re-run at a faster gate's pace.
    let mut next_due: BTreeMap<&str, SystemTime> = polling
        .iter()
        .map(|(name, spec)| {
            (
                name.as_str(),
                first_run_start + Duration::from_secs(u64::from(spec.interval_secs)),
            )
        })
        .collect();
    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        // A pending gate may run again only if that run would start within
        // this tick's hold and before its window's deadline.
        let schedulable: Vec<(&String, SystemTime)> = polling
            .iter()
            .filter(|(name, spec)| {
                if !is_pending(&results[name.as_str()], spec) {
                    return false;
                }
                let due = next_due[name.as_str()];
                let hold_end = first_run_start + Duration::from_secs(u64::from(spec.hold_secs));
                let deadline = windows[name.as_str()].since
                    + Duration::from_secs(u64::from(spec.timeout_secs));
                due <= hold_end && due < deadline
            })
            .map(|(name, _)| (*name, next_due[name.as_str()]))
            .collect();
        let Some(earliest) = schedulable.iter().map(|(_, due)| *due).min() else {
            break;
        };
        let wait = earliest
            .duration_since(SystemTime::now())
            .unwrap_or_default();
        if !sleep_unless_shutdown(wait, shutdown) {
            break;
        }
        let started = SystemTime::now();
        let subset: BTreeMap<String, Gate> = schedulable
            .iter()
            .filter(|(_, due)| *due <= earliest)
            .map(|(name, _)| ((*name).clone(), gates[name.as_str()].clone()))
            .collect();
        match evaluate(&subset) {
            Ok(rerun) => {
                for (name, result) in rerun {
                    if let Some(w) = windows.get_mut(name.as_str()) {
                        w.runs += 1;
                    }
                    if let Some((key, spec)) = polling.iter().find(|(n, _)| **n == name) {
                        next_due.insert(
                            key.as_str(),
                            started + Duration::from_secs(u64::from(spec.interval_secs)),
                        );
                    }
                    results.insert(name, result);
                }
            }
            Err(_) => break,
        }
    }

    let end = SystemTime::now();
    for (name, spec) in polling {
        let w = &windows[name.as_str()];
        let Some(result) = results.get_mut(name) else {
            continue;
        };
        let deadline = w.since + Duration::from_secs(u64::from(spec.timeout_secs));
        let status = if result.outcome == GateOutcome::Passed {
            PollStatus::Done
        } else if is_pending(result, spec) {
            if end >= deadline {
                PollStatus::TimedOut
            } else {
                PollStatus::Pending
            }
        } else {
            PollStatus::Failed
        };
        match status {
            PollStatus::Pending => {
                result.outcome = GateOutcome::Pending;
                result.failure = None;
            }
            PollStatus::TimedOut => {
                result.outcome = GateOutcome::TimedOut;
                if let Some(fallback) = result.failure.as_mut().and_then(|f| f.fallback.as_mut()) {
                    fallback.message =
                        format!("command still pending after {} seconds", spec.timeout_secs);
                    fallback.message_source = MessageSource::Koto;
                }
            }
            PollStatus::Done | PollStatus::Failed => {}
        }
        result.poll = Some(PollReport {
            status,
            evaluations: w.runs,
            since: w.since_text.clone(),
            elapsed_secs: end.duration_since(w.since).unwrap_or_default().as_secs(),
            interval_secs: spec.interval_secs,
            timeout_secs: spec.timeout_secs,
        });
    }
}

/// Sleep for `total`, waking every 100 ms to check `shutdown`. Returns false
/// when the flag cut the sleep short.
fn sleep_unless_shutdown(total: Duration, shutdown: &AtomicBool) -> bool {
    let end = std::time::Instant::now() + total;
    loop {
        if shutdown.load(Ordering::Relaxed) {
            return false;
        }
        let now = std::time::Instant::now();
        if now >= end {
            return true;
        }
        std::thread::sleep((end - now).min(Duration::from_millis(100)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(pending: i32) -> PollSpec {
        PollSpec {
            interval_secs: 1,
            timeout_secs: 60,
            hold_secs: 0,
            pending_exit_code: pending,
        }
    }

    fn result(outcome: GateOutcome, output: serde_json::Value) -> StructuredGateResult {
        StructuredGateResult {
            outcome,
            output,
            ..Default::default()
        }
    }

    #[test]
    fn only_a_plain_exit_with_the_pending_code_is_pending() {
        let s = spec(75);
        let pending = result(
            GateOutcome::Failed,
            serde_json::json!({"exit_code": 75, "error": ""}),
        );
        assert!(is_pending(&pending, &s));
        let other = result(
            GateOutcome::Failed,
            serde_json::json!({"exit_code": 1, "error": ""}),
        );
        assert!(!is_pending(&other, &s));
        let passed = result(
            GateOutcome::Passed,
            serde_json::json!({"exit_code": 0, "error": ""}),
        );
        assert!(!is_pending(&passed, &s));
    }

    #[test]
    fn a_killed_or_unstartable_run_is_never_pending() {
        let s = spec(75);
        // The shapes `command_gate_result` gives a per-run timeout and a
        // spawn failure, even if an exit code happened to match.
        let timed_out = result(
            GateOutcome::TimedOut,
            serde_json::json!({"exit_code": 75, "error": "timed_out", "failure_kind": "timed_out"}),
        );
        assert!(!is_pending(&timed_out, &s));
        let spawn = result(
            GateOutcome::Error,
            serde_json::json!({"exit_code": -1, "error": "no shell", "failure_kind": "spawn_failed"}),
        );
        assert!(!is_pending(&spawn, &s));
        let odd = result(
            GateOutcome::Failed,
            serde_json::json!({"exit_code": 75, "error": "", "failure_kind": "spawn_failed"}),
        );
        assert!(!is_pending(&odd, &s));
    }
}
