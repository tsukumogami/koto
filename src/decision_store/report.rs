//! The windowed report.
//!
//! For a UTC window that includes its start and excludes its end, counting
//! every kind except `reverse` and `refused` as decisions: the decisions
//! made in the window and how many are reversed as of now, as a kind by
//! decider-kind table; the reversals recorded in the window, as a table of
//! the reversed decision's kind by its decider kind; each reversed
//! decision with its reversal; and how many `gate-pass` decisions had a
//! defaulted decider.
//!
//! It reads the month files the window touches plus the reversal log; the
//! reversal table reads only the copy of the original each reversal line
//! carries.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;

use super::reader::{self, Filter, Needles};
use super::record::{DeciderKind, Decision, Kind};
use super::{id_month, StoreError};

/// One cell of the decisions table.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DecisionCell {
    pub kind: Kind,
    pub decider_kind: DeciderKind,
    pub count: usize,
    pub reversed: usize,
}

/// One cell of the reversals table.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ReversalCell {
    pub kind: Kind,
    pub decider_kind: DeciderKind,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct Window {
    pub since: String,
    pub until: String,
}

#[derive(Debug, Serialize)]
pub struct DecisionsMade {
    pub total: usize,
    pub reversed: usize,
    pub table: Vec<DecisionCell>,
}

#[derive(Debug, Serialize)]
pub struct ReversalsRecorded {
    pub total: usize,
    pub table: Vec<ReversalCell>,
}

/// A reversed decision with its reversal.
#[derive(Debug, Serialize)]
pub struct ReversedPair {
    pub decision: Decision,
    pub reversal: Decision,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub window: Window,
    pub decisions: DecisionsMade,
    pub reversals: ReversalsRecorded,
    /// Each reversed decision made in the window, and each decision whose
    /// reversal was recorded in it, with its reversal; oldest first.
    pub reversed: Vec<ReversedPair>,
    /// `gate-pass` decisions made in the window whose decider was
    /// defaulted.
    pub gate_pass_defaulted: usize,
    /// Lines the readers couldn't read and skipped, plus reversal lines in
    /// the window that carry no copy of their original and so can't be
    /// tabled.
    pub skipped_lines: usize,
}

fn in_window(time: &str, since: &str, until: &str) -> bool {
    time >= since && time < until
}

/// Build the report for `[since, until)`; both bounds normalized.
pub fn report(root: &Path, since: &str, until: &str) -> Result<Report, StoreError> {
    let window = Window {
        since: since.to_string(),
        until: until.to_string(),
    };
    let listing_filter = Filter {
        since: Some(since.to_string()),
        until: Some(until.to_string()),
        ..Default::default()
    };
    let empty = || Report {
        window: Window {
            since: since.to_string(),
            until: until.to_string(),
        },
        decisions: DecisionsMade {
            total: 0,
            reversed: 0,
            table: Vec::new(),
        },
        reversals: ReversalsRecorded {
            total: 0,
            table: Vec::new(),
        },
        reversed: Vec::new(),
        gate_pass_defaulted: 0,
        skipped_lines: 0,
    };
    match std::fs::symlink_metadata(root) {
        Ok(_) => super::writer::check_dir(root)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(empty()),
        Err(e) => return Err(StoreError::io(format!("reading {}", root.display()), e)),
    }

    let reversals = reader::read_reversals(root)?;
    let index = reader::reversal_index(&reversals.decisions);
    let made = reader::select_months(root, &listing_filter)?;
    let mut skipped = made.skipped + reversals.skipped;

    let mut table: BTreeMap<(Kind, DeciderKind), (usize, usize)> = BTreeMap::new();
    let mut gate_pass_defaulted = 0;
    let mut total = 0;
    let mut reversed_total = 0;
    let mut pairs: Vec<ReversedPair> = Vec::new();
    let mut paired: BTreeSet<String> = BTreeSet::new();
    for d in &made.decisions {
        if !d.kind.counts_as_decision() {
            continue;
        }
        total += 1;
        let cell = table.entry((d.kind, d.decided_by.kind)).or_default();
        cell.0 += 1;
        if let Some(r) = index.get(&d.id) {
            cell.1 += 1;
            reversed_total += 1;
            paired.insert(d.id.clone());
            pairs.push(ReversedPair {
                decision: d.clone(),
                reversal: (*r).clone(),
            });
        }
        if d.kind == Kind::GatePass && d.decided_by.defaulted {
            gate_pass_defaulted += 1;
        }
    }

    // Reversals recorded in the window, tabled by what each line copied
    // from its original.
    let mut r_table: BTreeMap<(Kind, DeciderKind), usize> = BTreeMap::new();
    let mut r_total = 0;
    let mut wanted: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for r in &reversals.decisions {
        if r.kind != Kind::Reverse || !in_window(&r.time, since, until) {
            continue;
        }
        let Some(original) = &r.original else {
            skipped += 1;
            continue;
        };
        r_total += 1;
        *r_table
            .entry((original.kind, original.decider_kind))
            .or_default() += 1;
        if let Some(target) = r.reverses.as_deref() {
            if !paired.contains(target) {
                if let Some(month) = id_month(target) {
                    wanted.entry(month).or_default().insert(target.to_string());
                }
            }
        }
    }

    // Fetch the originals of in-window reversals made outside the window,
    // one read per month.
    for (month, ids) in wanted {
        let read = reader::read_file(&root.join(format!("{}.jsonl", month)), &Needles::none())?;
        skipped += read.skipped;
        for d in read.decisions {
            if ids.contains(&d.id) && paired.insert(d.id.clone()) {
                if let Some(r) = index.get(&d.id) {
                    pairs.push(ReversedPair {
                        decision: d,
                        reversal: (*r).clone(),
                    });
                }
            }
        }
    }
    pairs.sort_by(|a, b| {
        (a.decision.time.as_str(), a.decision.id.as_str())
            .cmp(&(b.decision.time.as_str(), b.decision.id.as_str()))
    });

    Ok(Report {
        window,
        decisions: DecisionsMade {
            total,
            reversed: reversed_total,
            table: table
                .into_iter()
                .map(|((kind, decider_kind), (count, reversed))| DecisionCell {
                    kind,
                    decider_kind,
                    count,
                    reversed,
                })
                .collect(),
        },
        reversals: ReversalsRecorded {
            total: r_total,
            table: r_table
                .into_iter()
                .map(|((kind, decider_kind), count)| ReversalCell {
                    kind,
                    decider_kind,
                    count,
                })
                .collect(),
        },
        reversed: pairs,
        gate_pass_defaulted,
        skipped_lines: skipped,
    })
}
