//! `koto decisions report --since <t> --until <t> [--below <session>]`
//! (PRD R29): decisions and reversals by kind and decider kind over a UTC
//! window that includes its start and excludes its end.
//!
//! The sessions in scope are every session in the local store, or with
//! `--below` the named root and every session below it. Counting every kind
//! but `reverse` and `refused`, the report gives:
//!
//! - `made`: the decisions in scope timed in the window, as a table of kind
//!   by decider kind, each cell with how many read `reversed` now;
//! - `reversals`: the winning reversals timed in the window whose original
//!   is in scope, by the original's kind and decider kind, wherever the
//!   reversal is stored and whenever the original was made;
//! - `reversed`: each such original with its reversal;
//! - `defaulted_gate_passes`: the `gate-pass` decisions in `made` whose
//!   decider koto filled in;
//! - `sessions_read`: how many sessions were in scope.
//!
//! It reads this host's session directories only (decision files, link
//! files and state-file headers) and writes nothing. Reversals are looked
//! for in every session on the host, since one can sit in an anchor or a
//! session above the scope.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::body::DecisionBody;
use super::read::{self, Entry};
use super::standing::Index;
use super::{DecisionError, FORMAT_VERSION};
use crate::session::Backend;

/// The kinds no decisions-made count includes.
const NOT_MADE: [&str; 2] = ["reverse", "refused"];

/// A window bound, a date (`YYYY-MM-DD`, its midnight) or an RFC 3339 UTC
/// time, in koto's stored form so it compares with a decision's time as a
/// string. `field` names the flag in a refusal.
pub fn bound(field: &str, value: &str) -> Result<String, DecisionError> {
    let full = if value.len() == 10 && !value.contains('T') {
        format!("{value}T00:00:00Z")
    } else {
        value.to_string()
    };
    full.ends_with('Z')
        .then(|| crate::engine::types::parse_timestamp_to_systemtime(&full))
        .flatten()
        .map(crate::engine::types::iso8601_at)
        .ok_or_else(|| {
            DecisionError::invalid(
                field,
                format!(
                    "--{field} takes a date (2026-11-01) or an RFC 3339 UTC time \
                     (2026-11-01T09:30:00Z)"
                ),
            )
        })
}

/// The window `[since, until)`, both in stored form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub since: String,
    pub until: String,
}

impl Window {
    /// Parse both bounds, refusing a window that ends at or before its start.
    pub fn new(since: &str, until: &str) -> Result<Self, DecisionError> {
        let since = bound("since", since)?;
        let until = bound("until", until)?;
        if since >= until {
            return Err(DecisionError::invalid(
                "until",
                format!("the window ends at {until}, not after its start {since}"),
            ));
        }
        Ok(Self { since, until })
    }

    fn holds(&self, time: &str) -> bool {
        time >= self.since.as_str() && time < self.until.as_str()
    }
}

/// One cell of the decisions-made table.
#[derive(Debug, Clone, Copy, Default)]
struct Cell {
    made: u64,
    reversed: u64,
}

/// The report over `window`, for the sessions below `below` (the root
/// included) or for every session on this host.
pub fn report(backend: &Backend, window: &Window, below: Option<&str>) -> Value {
    let all = read::local_sessions(backend);
    let scope: Vec<String> = match below {
        Some(root) => {
            let mut s = vec![root.to_string()];
            s.extend(read::sessions_below(backend, root));
            s
        }
        None => all.clone(),
    };
    let in_scope: BTreeSet<&str> = scope.iter().map(String::as_str).collect();

    // Every session's decisions, read once. The index keeps only the
    // reversals, which are all a status needs.
    let mut sessions: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    for s in &all {
        sessions.insert(s.clone(), read::read_session_with(backend, s, false));
    }
    for s in &scope {
        if !sessions.contains_key(s) {
            sessions.insert(s.clone(), read::read_session_with(backend, s, false));
        }
    }
    let reversals_only = sessions
        .iter()
        .map(|(s, entries)| {
            let kept = entries
                .iter()
                .filter(|e| matches!(e, Entry::Decision(b) if b.kind == "reverse"))
                .cloned()
                .collect();
            (s.clone(), kept)
        })
        .collect();
    let index = Index::from_sessions(reversals_only);

    let mut made: BTreeMap<&str, BTreeMap<&str, Cell>> = BTreeMap::new();
    let mut defaulted = 0u64;
    let mut unreadable = Vec::new();
    // The decisions in scope, by (session id, id), for the reversals.
    let mut originals: BTreeMap<(&str, &str), &DecisionBody> = BTreeMap::new();
    for s in &scope {
        for entry in &sessions[s] {
            let body = match entry {
                Entry::Decision(b) => b,
                Entry::Unreadable { .. } => {
                    unreadable.push(entry.to_json());
                    continue;
                }
            };
            if NOT_MADE.contains(&body.kind.as_str()) {
                continue;
            }
            originals.insert((body.session.id.as_str(), body.id.as_str()), body);
            if !window.holds(&body.time) {
                continue;
            }
            let cell = made
                .entry(body.kind.as_str())
                .or_default()
                .entry(body.decider.kind())
                .or_default();
            cell.made += 1;
            if index.status(body) == "reversed" {
                cell.reversed += 1;
            }
            if body.kind == "gate-pass" && body.decider.defaulted() {
                defaulted += 1;
            }
        }
    }

    let mut reversals: BTreeMap<&str, BTreeMap<&str, u64>> = BTreeMap::new();
    let mut reversed = Vec::new();
    for entries in sessions.values() {
        for entry in entries {
            let Entry::Decision(r) = entry else { continue };
            if r.kind != "reverse" || !window.holds(&r.time) || index.status(r) != "in-effect" {
                continue;
            }
            let Some(t) = &r.target else { continue };
            if !in_scope.contains(t.name.as_str()) {
                continue;
            }
            let Some(original) = originals.get(&(t.id.as_str(), t.decision.as_str())) else {
                continue;
            };
            *reversals
                .entry(original.kind.as_str())
                .or_default()
                .entry(original.decider.kind())
                .or_default() += 1;
            reversed.push(json!({
                "decision": summary(original),
                "reversal": {
                    "session": r.session.name,
                    "id": r.id,
                    "time": r.time,
                    "decider_kind": r.decider.kind(),
                    "standing": r.standing,
                },
            }));
        }
    }
    reversed.sort_by_key(|v| {
        (
            v["reversal"]["time"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            v["reversal"]["session"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            v["reversal"]["id"].as_str().unwrap_or_default().to_string(),
        )
    });

    let made_json: BTreeMap<&str, BTreeMap<&str, Value>> = made
        .into_iter()
        .map(|(kind, by)| {
            let by = by
                .into_iter()
                .map(|(d, c)| (d, json!({"made": c.made, "reversed": c.reversed})))
                .collect();
            (kind, by)
        })
        .collect();
    json!({
        "v": FORMAT_VERSION,
        "window": {"since": window.since, "until": window.until},
        "below": below,
        "sessions_read": scope.len(),
        "made": made_json,
        "reversals": reversals,
        "reversed": reversed,
        "defaulted_gate_passes": defaulted,
        "unreadable": unreadable,
    })
}

/// A decision as the `reversed` list names it.
fn summary(b: &DecisionBody) -> Value {
    json!({
        "session": b.session.name,
        "id": b.id,
        "time": b.time,
        "kind": b.kind,
        "outcome": b.outcome,
        "decider_kind": b.decider.kind(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bound_is_a_date_or_a_utc_time_in_the_stored_form() {
        assert_eq!(
            bound("since", "2026-11-01").unwrap(),
            "2026-11-01T00:00:00.000Z"
        );
        assert_eq!(
            bound("since", "2026-11-01T09:30:00Z").unwrap(),
            "2026-11-01T09:30:00.000Z"
        );
        assert_eq!(
            bound("since", "2026-11-01T09:30:00.5Z").unwrap(),
            "2026-11-01T09:30:00.500Z"
        );
        for bad in [
            "2026-11-01T09:30:00+02:00",
            "yesterday",
            "2026-13-01",
            "2026-11-01T09:30:00",
        ] {
            assert_eq!(
                bound("since", bad).unwrap_err().field.as_deref(),
                Some("since"),
                "{bad}"
            );
        }
    }

    #[test]
    fn the_window_includes_its_start_and_excludes_its_end() {
        let w = Window::new("2026-11-01", "2026-12-01").unwrap();
        assert!(w.holds("2026-11-01T00:00:00.000Z"));
        assert!(w.holds("2026-11-30T23:59:59.999Z"));
        assert!(!w.holds("2026-12-01T00:00:00.000Z"));
        assert!(!w.holds("2026-10-31T23:59:59.999Z"));
        // A raw bound without milliseconds would sort after a decision timed
        // exactly at it; the stored form doesn't.
        let w = Window::new("2026-11-01T00:00:00Z", "2026-11-02T00:00:00Z").unwrap();
        assert!(w.holds("2026-11-01T00:00:00.000Z"));
    }

    #[test]
    fn an_empty_or_backward_window_is_refused() {
        assert!(Window::new("2026-11-01", "2026-11-01").is_err());
        assert!(Window::new("2026-12-01", "2026-11-01").is_err());
    }
}
