//! Builds the session store `tests/dashboard_golden_test.rs` reads.
//!
//! Include it from a test file with
//!
//! ```ignore
//! #[path = "support/golden_store.rs"]
//! mod golden_store;
//! ```
//!
//! The store is written at run time, with every event timestamp an offset
//! from the clock the caller passes, so a session's liveness class doesn't
//! drift as the calendar moves. Each offset sits at least 4 minutes from
//! every liveness threshold the dashboard applies (5 minutes, 2 hours and
//! 7 days), so a slow run can't push a session across one.
//!
//! The builder writes state files and compiled templates as JSON text and
//! uses nothing from the koto library, so a change to koto's own types or
//! module paths can't change what it writes. It holds:
//!
//! - one live session per liveness class: blocked on a failed gate, ended
//!   in a failed terminal state, stalled, active, idle, and pending (never
//!   advanced);
//! - a session that ended in a terminal state that isn't a failure (done);
//! - an abandoned session, silent for longer than 7 days;
//! - a parent with two children;
//! - a session whose log holds an event type koto doesn't know, between
//!   two transitions;
//! - a session whose state file header doesn't parse;
//! - a staging directory, the shape a close session takes while it is
//!   being staged.

#![allow(dead_code)]

use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The id of the session whose state file header doesn't parse.
pub const CORRUPT_SESSION: &str = "golden-corrupt";

/// The name of the staging directory in the store.
pub const STAGING_DIR: &str = ".koto-staging-anchor-golden-closed-0a1b2c3d";

/// The template every session runs, compiled. `work` takes evidence,
/// `review` waits on a context gate, and `done` and `failed` are terminal.
const COMPILED_TEMPLATE: &str = r#"{
  "format_version": 1,
  "name": "golden-flow",
  "version": "1.0",
  "initial_state": "work",
  "states": {
    "work": {
      "directive": "Do the work.",
      "transitions": [
        {
          "target": "review",
          "when": {
            "result": "ok"
          }
        },
        {
          "target": "failed",
          "when": {
            "result": "broken"
          }
        }
      ],
      "accepts": {
        "result": {
          "field_type": "enum",
          "required": true,
          "values": ["ok", "broken"]
        }
      }
    },
    "review": {
      "directive": "Wait for approval.",
      "transitions": [
        {
          "target": "done",
          "when": {
            "gates.approved.exists": true
          }
        }
      ],
      "gates": {
        "approved": {
          "type": "context-exists",
          "key": "approved"
        }
      }
    },
    "done": {
      "directive": "Done.",
      "terminal": true
    },
    "failed": {
      "directive": "Failed.",
      "terminal": true
    }
  }
}
"#;

/// One event in a session's log: its type, its payload, and how long
/// before the run's clock it happened.
struct Ev {
    ago: Duration,
    kind: &'static str,
    payload: serde_json::Value,
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn mins(n: u64) -> Duration {
    secs(n * 60)
}

fn hours(n: u64) -> Duration {
    mins(n * 60)
}

fn days(n: u64) -> Duration {
    hours(n * 24)
}

/// `t` as koto writes a timestamp: UTC, millisecond precision.
pub fn iso8601(t: SystemTime) -> String {
    let since = t.duration_since(UNIX_EPOCH).expect("clock before 1970");
    let total = since.as_secs();
    let millis = since.subsec_millis();
    let (days_since, rem) = (total / 86_400, total % 86_400);
    let (hour, minute, second) = (rem / 3600, (rem % 3600) / 60, rem % 60);

    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days_since as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

fn template_hash() -> String {
    hex::encode(Sha256::digest(COMPILED_TEMPLATE.as_bytes()))
}

fn initialized(ago: Duration) -> Ev {
    Ev {
        ago,
        kind: "workflow_initialized",
        payload: serde_json::json!({
            "template_path": format!("{}.json", template_hash()),
            "variables": {}
        }),
    }
}

fn transitioned(ago: Duration, from: Option<&str>, to: &str) -> Ev {
    Ev {
        ago,
        kind: "transitioned",
        payload: serde_json::json!({
            "from": from,
            "to": to,
            "condition_type": "auto"
        }),
    }
}

fn intent(ago: Duration, text: &str) -> Ev {
    Ev {
        ago,
        kind: "intent_updated",
        payload: serde_json::json!({ "intent": text }),
    }
}

fn gate_failed(ago: Duration, at: &str) -> Ev {
    Ev {
        ago,
        kind: "gate_evaluated",
        payload: serde_json::json!({
            "state": "review",
            "gate": "approved",
            "output": { "error": "", "exists": false },
            "outcome": "failed",
            "timestamp": at,
            "attempt": 1,
            "visit_attempt": 1,
            "findings": [{
                "rule_id": "approved",
                "level": "error",
                "message": "context key 'approved' is not set",
                "effect_landed": false,
                "message_source": "koto"
            }],
            "rule_counts": { "approved": { "visit": 1, "session": 1 } },
            "reason": "no_finding",
            "fallback_logged": true
        }),
    }
}

/// An event type no koto build defines.
fn unknown_event(ago: Duration) -> Ev {
    Ev {
        ago,
        kind: "golden_future_event",
        payload: serde_json::json!({ "note": "written by a newer koto" }),
    }
}

/// Writes one session: its directory, its compiled template and its state
/// file, a header line followed by one line per event.
fn write_session(
    sessions: &Path,
    now: SystemTime,
    id: &str,
    parent: Option<&str>,
    events: Vec<Ev>,
) {
    let dir = sessions.join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{}.json", template_hash())),
        COMPILED_TEMPLATE,
    )
    .unwrap();

    let created = events.first().map(|e| e.ago).unwrap_or_default();
    let mut header = serde_json::json!({
        "schema_version": 1,
        "workflow": id,
        "template_hash": template_hash(),
        "created_at": iso8601(now - created),
        "session_id": format!("golden-session-{id}"),
        "template_name": "golden-flow"
    });
    if let Some(parent) = parent {
        header["parent_workflow"] = serde_json::json!(parent);
    }

    let mut log = serde_json::to_string(&header).unwrap();
    log.push('\n');
    for (i, ev) in events.into_iter().enumerate() {
        let line = serde_json::json!({
            "seq": i + 1,
            "timestamp": iso8601(now - ev.ago),
            "type": ev.kind,
            "payload": ev.payload
        });
        log.push_str(&serde_json::to_string(&line).unwrap());
        log.push('\n');
    }
    std::fs::write(dir.join(format!("koto-{id}.state.jsonl")), log).unwrap();
}

/// Builds the golden store in `sessions`, the directory `KOTO_SESSIONS_BASE`
/// names, with every timestamp an offset from `now`.
pub fn build(sessions: &Path, now: SystemTime) {
    std::fs::create_dir_all(sessions).unwrap();

    // Needs you: waiting on a gate that failed, 20 minutes ago.
    let blocked_at = iso8601(now - mins(20));
    write_session(
        sessions,
        now,
        "golden-blocked",
        None,
        vec![
            initialized(hours(1)),
            transitioned(hours(1), None, "work"),
            intent(hours(1), "Ship the release notes"),
            transitioned(mins(21), Some("work"), "review"),
            gate_failed(mins(20), &blocked_at),
        ],
    );

    // Needs you: ended in the failed terminal state 40 minutes ago.
    write_session(
        sessions,
        now,
        "golden-failed",
        None,
        vec![
            initialized(hours(1)),
            transitioned(hours(1), None, "work"),
            intent(hours(1), "Migrate the schema"),
            transitioned(mins(40), Some("work"), "failed"),
        ],
    );

    // Needs you: advanced, then silent for 3 hours.
    write_session(
        sessions,
        now,
        "golden-stalled",
        None,
        vec![
            initialized(hours(4)),
            transitioned(hours(4), None, "work"),
            intent(hours(3), "Rebuild the search index"),
        ],
    );

    // Active: last event 20 seconds ago.
    write_session(
        sessions,
        now,
        "golden-active",
        None,
        vec![
            initialized(mins(30)),
            transitioned(mins(30), None, "work"),
            intent(secs(20), "Fix the flaky upload test"),
        ],
    );

    // Idle: last event 30 minutes ago.
    write_session(
        sessions,
        now,
        "golden-idle",
        None,
        vec![
            initialized(hours(1)),
            transitioned(hours(1), None, "work"),
            intent(mins(30), "Review the cache layer"),
        ],
    );

    // Pending: initialized 40 seconds ago and never advanced.
    write_session(
        sessions,
        now,
        "golden-pending",
        None,
        vec![initialized(secs(40))],
    );

    // Done: reached the done terminal state an hour ago. Receded, so only
    // `--all` shows it.
    write_session(
        sessions,
        now,
        "golden-done",
        None,
        vec![
            initialized(hours(1) + mins(30)),
            transitioned(hours(1) + mins(30), None, "work"),
            intent(hours(1) + mins(30), "Tag the build"),
            transitioned(hours(1) + mins(10), Some("work"), "review"),
            transitioned(hours(1), Some("review"), "done"),
        ],
    );

    // Abandoned: silent for 8 days. Receded, so only `--all` shows it.
    write_session(
        sessions,
        now,
        "golden-abandoned",
        None,
        vec![
            initialized(days(9)),
            transitioned(days(9), None, "work"),
            intent(days(8), "Prototype the old exporter"),
        ],
    );

    // A parent with two children: the parent idle 10 minutes, one child
    // active 10 seconds ago, the other idle 50 minutes.
    write_session(
        sessions,
        now,
        "golden-parent",
        None,
        vec![
            initialized(hours(1)),
            transitioned(hours(1), None, "work"),
            intent(mins(10), "Coordinate the fan-out"),
        ],
    );
    write_session(
        sessions,
        now,
        "golden-parent-child-a",
        Some("golden-parent"),
        vec![
            initialized(hours(1)),
            transitioned(hours(1), None, "work"),
            intent(secs(10), "First half of the fan-out"),
        ],
    );
    write_session(
        sessions,
        now,
        "golden-parent-child-b",
        Some("golden-parent"),
        vec![
            initialized(hours(1)),
            transitioned(hours(1), None, "work"),
            intent(mins(50), "Second half of the fan-out"),
        ],
    );

    // An event type koto doesn't know, between two transitions: the read
    // carries on past it to the state the session reached, idle 1h20m.
    write_session(
        sessions,
        now,
        "golden-unknown-event",
        None,
        vec![
            initialized(hours(2) - mins(10)),
            transitioned(hours(2) - mins(10), None, "work"),
            intent(hours(2) - mins(10), "Read past a newer event"),
            unknown_event(hours(1) + mins(30)),
            transitioned(hours(1) + mins(20), Some("work"), "review"),
        ],
    );

    // A state file whose header doesn't parse.
    let corrupt = sessions.join(CORRUPT_SESSION);
    std::fs::create_dir_all(&corrupt).unwrap();
    std::fs::write(
        corrupt.join(format!("koto-{CORRUPT_SESSION}.state.jsonl")),
        "not a valid header line\n",
    )
    .unwrap();

    // A close session being staged: its own session directory inside the
    // staging directory, with a header and no events yet.
    let staged_name = "anchor-golden-closed";
    let staged = sessions.join(STAGING_DIR).join(staged_name);
    std::fs::create_dir_all(&staged).unwrap();
    let staged_header = serde_json::json!({
        "schema_version": 1,
        "workflow": staged_name,
        "template_hash": template_hash(),
        "created_at": iso8601(now - secs(30)),
        "session_id": "golden-session-staged"
    });
    std::fs::write(
        staged.join(format!("koto-{staged_name}.state.jsonl")),
        format!("{}\n", serde_json::to_string(&staged_header).unwrap()),
    )
    .unwrap();
}
