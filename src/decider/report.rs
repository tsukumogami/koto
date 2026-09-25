//! `koto decider report`: read the ledger, join its records into per-question
//! metrics, run golden fixtures, and judge promotion eligibility.
//!
//! See docs/designs/current/DESIGN-jev-decision-offload.md, Decision 4. Everything
//! here is read-only: the ledger is read, never written, and the fixture
//! runner sends requests through the caller's [`Decider`] but records
//! nothing. The only I/O is [`read_ledger`]; the CLI in `src/cli/decider.rs`
//! resolves settings, compiles the template, and prints.
//!
//! ## Terms
//!
//! A **question** is one `(state, field, declaration_hash)`: two hashes for
//! the same field are two questions, so evidence gathered under an old
//! declaration never counts toward the current one.
//!
//! A **paired observation** is a consultation and the agent's `answered`
//! record for the same `(session_id, visit_seq)`. A consultation with a
//! null `session_id` is counted but never paired.
//!
//! The decider's **column** for a field is the declared value it chose at or
//! above threshold, or `below_threshold`, `escape`, or `no_answer` (the
//! consultation stopped at `input_unavailable` or `error`).
//!
//! **Coverage** for a value is the share of well-formed consultations
//! (`applied` or `not_applied`) whose column is that value; question
//! coverage is the sum over values. The share over all consultations is
//! reported alongside.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::template::decider::DeciderMode;

use super::evaluate::{evaluate, EffectiveModes, FieldEvaluation, FieldOutcome};
use super::ledger::{AnsweredRecord, ConsultedRecord, LedgerRecord};
use super::record::{ConsultationOutcome, FieldConsultation};
use super::request::{build_request, AssembledInputs, DeclaredField, DeclaredKind};
use super::types::{Decider, DeciderError, ErrorClass, SettingOrigin};

/// Fixture cases each declared value needs.
pub const MIN_CASES_PER_VALUE: u64 = 10;
/// Fixture cases a set needs in total, escape-labelled cases included.
pub const MIN_CASES_TOTAL: u64 = 40;
/// Paired ledger observations a question needs under its current hash.
pub const MIN_LEDGER_PAIRS: u64 = 30;
/// Most ledger disagreements where the decider chose a value.
pub const MAX_DISAGREEMENTS: u64 = 1;
/// Consultations in `auto` before a question can be flagged low-coverage.
pub const LOW_COVERAGE_MIN_CONSULTATIONS: u64 = 30;
/// Coverage (percent) below which such a question is flagged.
pub const LOW_COVERAGE_PERCENT: u64 = 30;

/// Column name for a declared value chosen below its threshold.
pub const COL_BELOW_THRESHOLD: &str = "below_threshold";
/// Column name for the escape, a tie, or a boolean with no single winner.
pub const COL_ESCAPE: &str = "escape";
/// Column name for a consultation or case with no usable answer.
pub const COL_NO_ANSWER: &str = "no_answer";

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// The ledger's parsed records and what was skipped.
#[derive(Debug, Clone, Default)]
pub struct LedgerRead {
    /// Records in file order.
    pub records: Vec<LedgerRecord>,
    /// Non-blank lines seen, skipped ones included.
    pub lines: u64,
    /// Lines skipped as malformed: bad JSON, a missing or mistyped key, or
    /// a final line with no newline.
    pub malformed: u64,
    /// Well-formed lines whose `kind` is neither `consulted` nor `answered`.
    pub unknown_kind: u64,
}

/// One classified line.
enum Line {
    Record(Box<LedgerRecord>),
    Malformed,
    UnknownKind,
}

fn parse_line(bytes: &[u8]) -> Line {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return Line::Malformed;
    };
    match value.get("kind").and_then(Value::as_str) {
        None => return Line::Malformed,
        Some("consulted") | Some("answered") => {}
        Some(_) => return Line::UnknownKind,
    }
    match serde_json::from_value::<LedgerRecord>(value) {
        Ok(r) => Line::Record(Box::new(r)),
        Err(_) => Line::Malformed,
    }
}

/// Parse a ledger body. Blank lines are ignored; a final line without its
/// newline is a torn write and counts as malformed.
pub fn parse_ledger(body: &[u8]) -> LedgerRead {
    let mut read = LedgerRead::default();
    let mut rest = body;
    while !rest.is_empty() {
        let (line, terminated, next) = match rest.iter().position(|b| *b == b'\n') {
            Some(i) => (&rest[..i], true, &rest[i + 1..]),
            None => (rest, false, &rest[rest.len()..]),
        };
        rest = next;
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        read.lines += 1;
        if !terminated {
            read.malformed += 1;
            continue;
        }
        match parse_line(line) {
            Line::Record(r) => read.records.push(*r),
            Line::Malformed => read.malformed += 1,
            Line::UnknownKind => read.unknown_kind += 1,
        }
    }
    read
}

/// Read and parse the ledger at `path`. A missing file is an empty ledger.
pub fn read_ledger(path: &Path) -> std::io::Result<LedgerRead> {
    match std::fs::read(path) {
        Ok(body) => Ok(parse_ledger(&body)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LedgerRead::default()),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// The ledger report
// ---------------------------------------------------------------------------

/// Which questions to report and whether custom endpoints count.
#[derive(Debug, Clone, Default)]
pub struct ReportOptions {
    /// Report only questions on this state.
    pub state: Option<String>,
    /// Count consultations from a user or env endpoint toward eligibility.
    pub include_custom_endpoints: bool,
}

/// What the ledger held, before any question is formed.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Header {
    pub lines: u64,
    pub consulted: u64,
    pub answered: u64,
    pub skipped_malformed: u64,
    pub unknown_kind: u64,
    /// `consulted` lines repeating a `(session_id, visit_seq)` already seen;
    /// only the first is counted.
    pub duplicate_consulted: u64,
    /// `answered` lines with no consultation to pair with.
    pub orphaned_answered: u64,
    /// Consultations whose session had no `session_id`: counted, never
    /// paired.
    pub null_session_id: u64,
    /// Consultations sent to a user or env endpoint. They stay in every
    /// metric.
    pub custom_endpoint_consultations: u64,
    /// Of those, how many are left out of promotion eligibility: all of them
    /// unless `--include-custom-endpoints` is passed.
    pub excluded_from_eligibility: u64,
}

/// Overall consultation outcomes for one question.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Outcomes {
    pub applied: u64,
    pub not_applied: u64,
    pub input_unavailable: u64,
    pub error: u64,
}

/// A confusion matrix: rows are the label (the agent's value, or a
/// fixture's `expected`), columns the decider's column.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Confusion {
    pub columns: Vec<String>,
    pub rows: BTreeMap<String, BTreeMap<String, u64>>,
}

impl Confusion {
    fn new(values: &[String]) -> Self {
        let mut columns: Vec<String> = values.to_vec();
        for c in [COL_BELOW_THRESHOLD, COL_ESCAPE, COL_NO_ANSWER] {
            columns.push(c.to_string());
        }
        let mut m = Confusion {
            columns,
            rows: BTreeMap::new(),
        };
        for v in values {
            m.row(v);
        }
        m
    }

    fn row(&mut self, label: &str) -> &mut BTreeMap<String, u64> {
        let columns = &self.columns;
        self.rows
            .entry(label.to_string())
            .or_insert_with(|| columns.iter().map(|c| (c.clone(), 0)).collect())
    }

    fn add(&mut self, label: &str, column: &str) {
        let row = self.row(label);
        *row.entry(column.to_string()).or_insert(0) += 1;
    }

    fn get(&self, label: &str, column: &str) -> u64 {
        self.rows
            .get(label)
            .and_then(|r| r.get(column))
            .copied()
            .unwrap_or(0)
    }

    fn row_total(&self, label: &str) -> u64 {
        self.rows.get(label).map_or(0, |r| r.values().sum())
    }
}

/// Coverage for one value or a question.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Coverage {
    /// Over well-formed consultations (`applied` and `not_applied`); `null`
    /// when there are none.
    pub well_formed: Option<f64>,
    /// Over every consultation.
    pub all: Option<f64>,
}

/// Per-value ledger metrics.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ValueReport {
    pub value: String,
    /// The effective mode recorded on the latest consultation.
    pub mode: Option<DeciderMode>,
    /// Paired observations where the agent chose this value.
    pub paired: u64,
    /// Of those, the share the decider also chose at or above threshold.
    pub recall: Option<f64>,
    /// Well-formed consultations whose column is this value.
    pub covered: u64,
    pub coverage: Coverage,
    /// Paired visits where the decider chose this value at or above
    /// threshold and the agent chose another.
    pub disagreements: u64,
    /// The same, counting only consultations that count toward eligibility.
    pub counted_disagreements: u64,
}

/// A paired visit where the decider's confident value differed from the
/// agent's.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Disagreement {
    pub session: String,
    pub session_id: String,
    pub visit_seq: u64,
    pub agent: String,
    pub decider: String,
    pub endpoint_origin: SettingOrigin,
}

/// Fallback and error rates, each over all of a question's consultations.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Rates {
    /// Consultations that weren't applied, so the agent was asked anyway.
    pub fallback: f64,
    /// Consultations that ended in `error`.
    pub error: f64,
    /// `error` consultations per `error_class`.
    pub error_by_class: BTreeMap<String, f64>,
    /// Consultations that ended in `input_unavailable`.
    pub input_unavailable: f64,
}

/// Provider latency, nearest-rank, excluding `input_unavailable`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Latency {
    pub samples: u64,
    pub p50: Option<u64>,
    pub p95: Option<u64>,
}

/// The PRD's success measures for one question.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SuccessMeasures {
    /// Visits settled automatically (`applied` consultations).
    pub agent_stops_removed: u64,
    /// Summed `directive_bytes` of those visits: a lower bound on context
    /// saved.
    pub directive_bytes_not_delivered: u64,
    /// Question coverage over well-formed consultations.
    pub coverage: Option<f64>,
}

/// A warning attached to a question.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Flag {
    pub name: String,
    pub detail: String,
}

/// Every ledger metric for one question.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuestionReport {
    pub state: String,
    pub field: String,
    pub declaration_hash: String,
    pub consultations: u64,
    pub outcomes: Outcomes,
    pub well_formed: u64,
    pub paired: u64,
    /// Paired observations that count toward eligibility.
    pub counted_paired: u64,
    /// Declared values, in the order the ledger's `modes` map lists them.
    pub values: Vec<ValueReport>,
    pub coverage: Coverage,
    pub confusion: Confusion,
    pub disagreements: Vec<Disagreement>,
    pub rates: Rates,
    pub latency_ms: Latency,
    pub success_measures: SuccessMeasures,
    pub flags: Vec<Flag>,
}

impl QuestionReport {
    fn value(&self, v: &str) -> Option<&ValueReport> {
        self.values.iter().find(|r| r.value == v)
    }
}

/// The ledger half of the report.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LedgerReport {
    pub header: Header,
    pub questions: Vec<QuestionReport>,
}

impl LedgerReport {
    /// The question with this key, if the ledger holds it.
    pub fn question(&self, state: &str, field: &str, hash: &str) -> Option<&QuestionReport> {
        self.questions
            .iter()
            .find(|q| q.state == state && q.field == field && q.declaration_hash == hash)
    }
}

/// The decider's column for one field of a consultation.
fn ledger_column(outcome: ConsultationOutcome, fc: &FieldConsultation) -> String {
    if matches!(
        outcome,
        ConsultationOutcome::InputUnavailable | ConsultationOutcome::Error
    ) {
        return COL_NO_ANSWER.to_string();
    }
    match fc.outcome {
        None => COL_NO_ANSWER.to_string(),
        Some(FieldOutcome::Escape) => COL_ESCAPE.to_string(),
        Some(FieldOutcome::BelowThreshold) => COL_BELOW_THRESHOLD.to_string(),
        Some(_) => match &fc.winning {
            Some(w) if fc.at_threshold && fc.modes.contains_key(w) => w.clone(),
            Some(_) if !fc.at_threshold => COL_BELOW_THRESHOLD.to_string(),
            _ => COL_ESCAPE.to_string(),
        },
    }
}

/// An agent's submitted value as a label: strings as themselves, booleans
/// as `true`/`false`.
fn label_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

fn ratio(n: u64, d: u64) -> Option<f64> {
    (d > 0).then(|| n as f64 / d as f64)
}

/// Nearest-rank percentile of sorted `xs`.
fn nearest_rank(sorted: &[u64], pct: u64) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    let n = sorted.len() as u64;
    let rank = (pct * n).div_ceil(100).max(1);
    Some(sorted[(rank - 1) as usize])
}

type VisitKey = (String, u64);

/// Accumulates one question.
#[derive(Default)]
struct QuestionAcc {
    consultations: u64,
    outcomes: Outcomes,
    values: Vec<String>,
    latest_modes: BTreeMap<String, DeciderMode>,
    covered: BTreeMap<String, u64>,
    latencies: Vec<u64>,
    errors_by_class: BTreeMap<String, u64>,
    applied_bytes: u64,
    auto_consultations: u64,
    auto_well_formed: u64,
    auto_covered: u64,
    /// (label, column, counted, disagreement)
    pairs: Vec<(String, String, bool, Option<Disagreement>)>,
}

/// Join the ledger into per-question metrics.
pub fn analyze(read: &LedgerRead, opts: &ReportOptions) -> LedgerReport {
    let mut header = Header {
        lines: read.lines,
        skipped_malformed: read.malformed,
        unknown_kind: read.unknown_kind,
        ..Header::default()
    };

    // Answers by visit: a later answer for the same field wins.
    let mut answers: HashMap<VisitKey, BTreeMap<String, Value>> = HashMap::new();
    let mut answered_keys: Vec<Option<VisitKey>> = Vec::new();
    for r in &read.records {
        if let LedgerRecord::Answered(a) = r {
            header.answered += 1;
            let key = answered_key(a);
            if let Some(k) = &key {
                let entry = answers.entry(k.clone()).or_default();
                for (f, v) in &a.values {
                    entry.insert(f.clone(), v.clone());
                }
            }
            answered_keys.push(key);
        }
    }

    // Consultations, first per visit.
    let mut seen: BTreeSet<VisitKey> = BTreeSet::new();
    let mut consulted: Vec<&ConsultedRecord> = Vec::new();
    for r in &read.records {
        if let LedgerRecord::Consulted(c) = r {
            header.consulted += 1;
            match &c.envelope.session_id {
                Some(id) => {
                    if !seen.insert((id.clone(), c.consultation.visit_seq)) {
                        header.duplicate_consulted += 1;
                        continue;
                    }
                }
                None => header.null_session_id += 1,
            }
            if c.consultation.endpoint_origin != SettingOrigin::Default {
                header.custom_endpoint_consultations += 1;
            }
            consulted.push(c);
        }
    }
    if !opts.include_custom_endpoints {
        header.excluded_from_eligibility = header.custom_endpoint_consultations;
    }
    header.orphaned_answered = answered_keys
        .iter()
        .filter(|k| k.as_ref().is_none_or(|k| !seen.contains(k)))
        .count() as u64;

    let mut questions: BTreeMap<(String, String, String), QuestionAcc> = BTreeMap::new();
    for c in consulted {
        let rec = &c.consultation;
        if opts.state.as_deref().is_some_and(|s| s != rec.state) {
            continue;
        }
        let counted =
            opts.include_custom_endpoints || rec.endpoint_origin == SettingOrigin::Default;
        let well_formed = matches!(
            rec.outcome,
            ConsultationOutcome::Applied | ConsultationOutcome::NotApplied
        );
        let answer = c
            .envelope
            .session_id
            .as_ref()
            .and_then(|id| answers.get(&(id.clone(), rec.visit_seq)));
        for (field, fc) in &rec.fields {
            let key = (
                rec.state.clone(),
                field.clone(),
                fc.declaration_hash.clone(),
            );
            let q = questions.entry(key).or_default();
            q.consultations += 1;
            match rec.outcome {
                ConsultationOutcome::Applied => {
                    q.outcomes.applied += 1;
                    q.applied_bytes += rec.directive_bytes;
                }
                ConsultationOutcome::NotApplied => q.outcomes.not_applied += 1,
                ConsultationOutcome::InputUnavailable => q.outcomes.input_unavailable += 1,
                ConsultationOutcome::Error => {
                    q.outcomes.error += 1;
                    let class = rec.error_class.map_or("unknown", |c| c.as_str());
                    *q.errors_by_class.entry(class.to_string()).or_insert(0) += 1;
                }
            }
            if rec.outcome != ConsultationOutcome::InputUnavailable {
                q.latencies.push(rec.latency_ms);
            }
            for v in fc.modes.keys() {
                if !q.values.contains(v) {
                    q.values.push(v.clone());
                }
            }
            q.latest_modes = fc.modes.clone();

            let column = ledger_column(rec.outcome, fc);
            let is_value = fc.modes.contains_key(&column);
            if well_formed && is_value {
                *q.covered.entry(column.clone()).or_insert(0) += 1;
            }
            if fc.modes.values().any(|m| *m == DeciderMode::Auto) {
                q.auto_consultations += 1;
                if well_formed {
                    q.auto_well_formed += 1;
                    if is_value {
                        q.auto_covered += 1;
                    }
                }
            }

            let Some(agent) = answer.and_then(|a| a.get(field)).map(label_of) else {
                continue;
            };
            let disagreement = (is_value && column != agent).then(|| Disagreement {
                session: c.envelope.session.clone(),
                session_id: c.envelope.session_id.clone().unwrap_or_default(),
                visit_seq: rec.visit_seq,
                agent: agent.clone(),
                decider: column.clone(),
                endpoint_origin: rec.endpoint_origin,
            });
            q.pairs.push((agent, column, counted, disagreement));
        }
    }

    let questions = questions
        .into_iter()
        .map(|((state, field, hash), q)| finish_question(state, field, hash, q))
        .collect();
    LedgerReport { header, questions }
}

fn answered_key(a: &AnsweredRecord) -> Option<VisitKey> {
    a.envelope
        .session_id
        .as_ref()
        .map(|id| (id.clone(), a.visit_seq))
}

fn finish_question(state: String, field: String, hash: String, q: QuestionAcc) -> QuestionReport {
    let well_formed = q.outcomes.applied + q.outcomes.not_applied;
    let mut confusion = Confusion::new(&q.values);
    let mut disagreements = Vec::new();
    let mut counted_paired = 0;
    let mut dis_all: BTreeMap<String, u64> = BTreeMap::new();
    let mut dis_counted: BTreeMap<String, u64> = BTreeMap::new();
    for (label, column, counted, dis) in &q.pairs {
        confusion.add(label, column);
        if *counted {
            counted_paired += 1;
        }
        if let Some(d) = dis {
            *dis_all.entry(d.decider.clone()).or_insert(0) += 1;
            if *counted {
                *dis_counted.entry(d.decider.clone()).or_insert(0) += 1;
            }
            disagreements.push(d.clone());
        }
    }

    let values: Vec<ValueReport> = q
        .values
        .iter()
        .map(|v| {
            let paired = confusion.row_total(v);
            let covered = q.covered.get(v).copied().unwrap_or(0);
            ValueReport {
                value: v.clone(),
                mode: q.latest_modes.get(v).copied(),
                paired,
                recall: ratio(confusion.get(v, v), paired),
                covered,
                coverage: Coverage {
                    well_formed: ratio(covered, well_formed),
                    all: ratio(covered, q.consultations),
                },
                disagreements: dis_all.get(v).copied().unwrap_or(0),
                counted_disagreements: dis_counted.get(v).copied().unwrap_or(0),
            }
        })
        .collect();

    let covered_total: u64 = q.covered.values().sum();
    let coverage = Coverage {
        well_formed: ratio(covered_total, well_formed),
        all: ratio(covered_total, q.consultations),
    };

    let n = q.consultations;
    let rate = |k: u64| ratio(k, n).unwrap_or(0.0);
    let rates = Rates {
        fallback: rate(n - q.outcomes.applied),
        error: rate(q.outcomes.error),
        error_by_class: q
            .errors_by_class
            .iter()
            .map(|(k, c)| (k.clone(), rate(*c)))
            .collect(),
        input_unavailable: rate(q.outcomes.input_unavailable),
    };

    let mut lat = q.latencies.clone();
    lat.sort_unstable();
    let latency_ms = Latency {
        samples: lat.len() as u64,
        p50: nearest_rank(&lat, 50),
        p95: nearest_rank(&lat, 95),
    };

    let mut flags = Vec::new();
    if q.auto_consultations >= LOW_COVERAGE_MIN_CONSULTATIONS
        && q.auto_covered * 100 < LOW_COVERAGE_PERCENT * q.auto_well_formed.max(1)
    {
        flags.push(Flag {
            name: "low_coverage".to_string(),
            detail: format!(
                "{} consultations with a value in auto, and coverage over them is {} of {} \
                 (below {}%): not worth its latency",
                q.auto_consultations, q.auto_covered, q.auto_well_formed, LOW_COVERAGE_PERCENT
            ),
        });
    }

    QuestionReport {
        state,
        field,
        declaration_hash: hash,
        consultations: n,
        outcomes: q.outcomes.clone(),
        well_formed,
        paired: q.pairs.len() as u64,
        counted_paired,
        values,
        coverage: coverage.clone(),
        confusion,
        disagreements,
        rates,
        latency_ms,
        success_measures: SuccessMeasures {
            agent_stops_removed: q.outcomes.applied,
            directive_bytes_not_delivered: q.applied_bytes,
            coverage: coverage.well_formed,
        },
        flags,
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// One golden case.
#[derive(Debug, Clone, PartialEq)]
pub struct FixtureCase {
    /// 1-based line number in the fixture file.
    pub line: usize,
    pub id: String,
    pub inputs: AssembledInputs,
    /// The label: a declared value, the escape, or `true`/`false`.
    pub expected: String,
}

/// A fixture line that can't be used. Aborts the run before any request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureLineError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for FixtureLineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "fixture line {}: {}", self.line, self.message)
    }
}

/// Keys a fixture line may hold.
pub const FIXTURE_KEYS: [&str; 3] = ["id", "inputs", "expected"];

/// Every label a case for `field` may carry, escape included.
pub fn fixture_labels(field: &DeclaredField<'_>) -> Vec<String> {
    match field.kind {
        DeclaredKind::Enum { values } => values
            .iter()
            .cloned()
            .chain(field.decider.escape.as_ref().map(|e| e.value.clone()))
            .collect(),
        DeclaredKind::Boolean => vec!["true".to_string(), "false".to_string()],
    }
}

/// The values eligibility is judged for: an enum's `values`, or `true` and
/// `false`.
pub fn promotable_values(field: &DeclaredField<'_>) -> Vec<String> {
    match field.kind {
        DeclaredKind::Enum { values } => values.to_vec(),
        DeclaredKind::Boolean => vec!["true".to_string(), "false".to_string()],
    }
}

/// Parse a JSON Lines fixture file for `field`. Every line is checked
/// before anything is sent: known keys only, an `expected` the field
/// declares (a JSON boolean for a boolean field), and exactly the declared
/// input labels, each within its `max_bytes`. Blank lines are skipped.
pub fn parse_fixtures(
    body: &str,
    field: &DeclaredField<'_>,
) -> Result<Vec<FixtureCase>, FixtureLineError> {
    let labels = fixture_labels(field);
    let mut cases = Vec::new();
    for (i, raw) in body.lines().enumerate() {
        let line = i + 1;
        let err = |message: String| FixtureLineError { line, message };
        if raw.trim().is_empty() {
            continue;
        }
        let value: Value =
            serde_json::from_str(raw).map_err(|e| err(format!("not valid JSON ({})", e)))?;
        let Value::Object(obj) = value else {
            return Err(err("not a JSON object".to_string()));
        };
        if let Some(k) = obj.keys().find(|k| !FIXTURE_KEYS.contains(&k.as_str())) {
            return Err(err(format!(
                "unknown key {:?} (a fixture line holds id, inputs, and expected)",
                k
            )));
        }
        let id = match obj.get("id") {
            None => format!("line-{}", line),
            Some(Value::String(s)) => s.clone(),
            Some(_) => return Err(err("id must be a string".to_string())),
        };
        let expected = match (field.kind, obj.get("expected")) {
            (_, None) => return Err(err("expected is missing".to_string())),
            (DeclaredKind::Boolean, Some(Value::Bool(b))) => b.to_string(),
            (DeclaredKind::Boolean, Some(other)) => {
                return Err(err(format!(
                    "expected {} is not a JSON boolean; {} is a boolean field",
                    other, field.name
                )))
            }
            (DeclaredKind::Enum { .. }, Some(Value::String(s))) if labels.contains(s) => s.clone(),
            (DeclaredKind::Enum { .. }, Some(other)) => {
                return Err(err(format!(
                    "expected {} is not declared for {} (one of: {})",
                    other,
                    field.name,
                    labels.join(", ")
                )))
            }
        };
        let texts: BTreeMap<String, String> = match obj.get("inputs") {
            Some(Value::Object(m)) => {
                let mut out = BTreeMap::new();
                for (k, v) in m {
                    let Value::String(s) = v else {
                        return Err(err(format!("input {:?} must be a string", k)));
                    };
                    out.insert(k.clone(), s.clone());
                }
                out
            }
            Some(_) => return Err(err("inputs must be an object".to_string())),
            None => return Err(err("inputs is missing".to_string())),
        };
        let inputs = AssembledInputs::from_texts(std::slice::from_ref(field), texts)
            .map_err(|e| err(e.to_string()))?;
        cases.push(FixtureCase {
            line,
            id,
            inputs,
            expected,
        });
    }
    if cases.is_empty() {
        return Err(FixtureLineError {
            line: 0,
            message: "the fixture file holds no cases".to_string(),
        });
    }
    Ok(cases)
}

/// What the provider made of one case.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CaseResult {
    pub line: usize,
    pub id: String,
    pub expected: String,
    /// The decider's column: a value, `below_threshold`, `escape`, or
    /// `no_answer`.
    pub answer: String,
    /// The winner's probability, when there was an answer.
    pub confidence: Option<f64>,
    /// Why there was no answer.
    pub error_class: Option<ErrorClass>,
}

/// Whether a provider error means the endpoint can't be used at all, so the
/// run stops: the connection failed or the key was refused.
pub fn aborts_fixture_run(e: &DeciderError) -> bool {
    match e.class {
        ErrorClass::Connect => true,
        ErrorClass::HttpStatus => matches!(e.status, Some(401) | Some(403)),
        _ => false,
    }
}

fn fixture_column(eval: &FieldEvaluation) -> String {
    match eval.outcome {
        FieldOutcome::Escape => COL_ESCAPE.to_string(),
        FieldOutcome::BelowThreshold => COL_BELOW_THRESHOLD.to_string(),
        _ => match &eval.winning {
            Some(w) if eval.at_threshold => w.clone(),
            _ => COL_BELOW_THRESHOLD.to_string(),
        },
    }
}

/// Run every case through the runtime's own path: [`build_request`], the
/// provider, and [`evaluate`]. A timeout or an unusable answer makes that
/// case `no_answer`; an error [`aborts_fixture_run`] stops the run.
pub fn run_fixtures(
    decider: &dyn Decider,
    field: &DeclaredField<'_>,
    modes: &EffectiveModes,
    cases: &[FixtureCase],
) -> Result<Vec<CaseResult>, DeciderError> {
    let fields = std::slice::from_ref(field);
    let mut out = Vec::with_capacity(cases.len());
    for case in cases {
        let no_answer = |class: Option<ErrorClass>| CaseResult {
            line: case.line,
            id: case.id.clone(),
            expected: case.expected.clone(),
            answer: COL_NO_ANSWER.to_string(),
            confidence: None,
            error_class: class,
        };
        let Ok(request) = build_request(fields, case.inputs.as_map()) else {
            out.push(no_answer(Some(ErrorClass::Malformed)));
            continue;
        };
        let response = match decider.decide(&request) {
            Ok(r) => r,
            Err(e) if aborts_fixture_run(&e) => return Err(e),
            Err(e) => {
                out.push(no_answer(Some(e.class)));
                continue;
            }
        };
        match evaluate(fields, &response, modes) {
            Ok(eval) => {
                let f = &eval.fields[0];
                out.push(CaseResult {
                    line: case.line,
                    id: case.id.clone(),
                    expected: case.expected.clone(),
                    answer: fixture_column(f),
                    confidence: Some(super::record::round_recorded(f.confidence)),
                    error_class: None,
                });
            }
            Err(e) => out.push(no_answer(Some(e.class))),
        }
    }
    Ok(out)
}

/// One eligibility condition and whether it held.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Condition {
    pub name: String,
    pub met: bool,
    /// The condition in words, with the observed number.
    pub detail: String,
}

/// Eligibility and fixture metrics for one value.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FixtureValue {
    pub value: String,
    pub template_mode: DeciderMode,
    /// `(template: never)` for a value the template marks `never`.
    pub note: Option<String>,
    /// Cases labelled with this value.
    pub cases: u64,
    pub recall: Option<f64>,
    /// Cases labelled otherwise answered with this value at or above its
    /// threshold.
    pub false_positives: u64,
    /// `eligible` or `ineligible`.
    pub status: String,
    pub eligible: bool,
    pub conditions: Vec<Condition>,
    /// The `detail` of every condition that failed.
    pub reasons: Vec<String>,
}

/// The ledger evidence eligibility used.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LedgerEvidence {
    pub paired: u64,
    pub counted_paired: u64,
    /// Pairs from a custom endpoint left out without
    /// `--include-custom-endpoints`.
    pub excluded_custom_endpoint: u64,
}

/// The fixture half of the report.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FixtureReport {
    pub template: String,
    pub state: String,
    pub field: String,
    pub kind: String,
    /// The current declaration hash, from the compiled template.
    pub declaration_hash: String,
    /// Where the endpoint the cases were sent to came from.
    pub endpoint_origin: SettingOrigin,
    /// Whether this run counts toward eligibility (default endpoint, or
    /// `--include-custom-endpoints`).
    pub endpoint_counted: bool,
    pub cases: u64,
    pub labels: BTreeMap<String, u64>,
    pub no_answer: u64,
    /// Mean fixture recall over the declared values that have cases.
    pub macro_recall: f64,
    /// Macro recall of always choosing the most frequent label: 0 when
    /// that label is the escape.
    pub majority_baseline: f64,
    pub confusion: Confusion,
    pub ledger: LedgerEvidence,
    pub values: Vec<FixtureValue>,
    pub results: Vec<CaseResult>,
}

/// What [`judge`] needs besides the case results.
pub struct JudgeInput<'a> {
    pub template: String,
    pub state: String,
    pub field: &'a DeclaredField<'a>,
    pub declaration_hash: String,
    pub endpoint_origin: SettingOrigin,
    pub include_custom_endpoints: bool,
    /// The ledger question for `(state, field, declaration_hash)`, if any.
    /// Computed with the same `include_custom_endpoints`.
    pub ledger: Option<&'a QuestionReport>,
}

/// Mark each value promotion-eligible or not.
pub fn judge(input: &JudgeInput<'_>, results: Vec<CaseResult>) -> FixtureReport {
    let field = input.field;
    let values = promotable_values(field);
    let mut confusion = Confusion::new(&values);
    let mut labels: BTreeMap<String, u64> = BTreeMap::new();
    for l in fixture_labels(field) {
        labels.insert(l, 0);
    }
    for r in &results {
        confusion.add(&r.expected, &r.answer);
        *labels.entry(r.expected.clone()).or_insert(0) += 1;
    }
    let total = results.len() as u64;
    let no_answer = results.iter().filter(|r| r.answer == COL_NO_ANSWER).count() as u64;

    // Macro recall over declared values with cases; the majority baseline
    // is that of a constant answer of the most frequent label.
    let with_cases: Vec<&String> = values.iter().filter(|v| labels[*v] > 0).collect();
    let macro_recall = if with_cases.is_empty() {
        0.0
    } else {
        with_cases
            .iter()
            .map(|v| confusion.get(v, v) as f64 / labels[*v] as f64)
            .sum::<f64>()
            / with_cases.len() as f64
    };
    let top = labels.values().copied().max().unwrap_or(0);
    let majority_is_value = top > 0 && values.iter().any(|v| labels[v] == top);
    let majority_baseline = if majority_is_value {
        1.0 / with_cases.len() as f64
    } else {
        0.0
    };
    let beats_baseline = macro_recall - majority_baseline > 1e-9;

    let evidence = match input.ledger {
        Some(q) => LedgerEvidence {
            paired: q.paired,
            counted_paired: q.counted_paired,
            excluded_custom_endpoint: q.paired - q.counted_paired,
        },
        None => LedgerEvidence::default(),
    };
    let endpoint_counted =
        input.include_custom_endpoints || input.endpoint_origin == SettingOrigin::Default;

    let fixture_values = values
        .iter()
        .map(|v| {
            let cases = labels[v];
            let false_positives: u64 = confusion
                .rows
                .iter()
                .filter(|(label, _)| *label != v)
                .map(|(_, row)| row.get(v).copied().unwrap_or(0))
                .sum();
            let disagreements = input
                .ledger
                .and_then(|q| q.value(v))
                .map_or(0, |r| r.counted_disagreements);
            let mut pairs_detail = format!(
                "at least {} paired observations under the current declaration hash (has {})",
                MIN_LEDGER_PAIRS, evidence.counted_paired
            );
            if evidence.excluded_custom_endpoint > 0 {
                let _ = write!(
                    pairs_detail,
                    "; {} from a custom endpoint are excluded without --include-custom-endpoints",
                    evidence.excluded_custom_endpoint
                );
            }
            let conditions = vec![
                Condition {
                    name: "labelled_cases".to_string(),
                    met: cases >= MIN_CASES_PER_VALUE,
                    detail: format!(
                        "at least {} fixture cases labelled {} (has {})",
                        MIN_CASES_PER_VALUE, v, cases
                    ),
                },
                Condition {
                    name: "total_cases".to_string(),
                    met: total >= MIN_CASES_TOTAL,
                    detail: format!(
                        "at least {} fixture cases in total (has {})",
                        MIN_CASES_TOTAL, total
                    ),
                },
                Condition {
                    name: "every_case_answered".to_string(),
                    met: no_answer == 0,
                    detail: format!(
                        "every fixture case got an answer ({} ended in no_answer)",
                        no_answer
                    ),
                },
                Condition {
                    name: "no_false_positives".to_string(),
                    met: false_positives == 0,
                    detail: format!(
                        "no fixture labelled otherwise is answered {} at or above its threshold \
                         ({} were)",
                        v, false_positives
                    ),
                },
                Condition {
                    name: "beats_majority_baseline".to_string(),
                    met: beats_baseline,
                    detail: format!(
                        "macro recall exceeds always choosing the most frequent label \
                         ({:.4} vs {:.4})",
                        macro_recall, majority_baseline
                    ),
                },
                Condition {
                    name: "ledger_pairs".to_string(),
                    met: evidence.counted_paired >= MIN_LEDGER_PAIRS,
                    detail: pairs_detail,
                },
                Condition {
                    name: "ledger_disagreements".to_string(),
                    met: disagreements <= MAX_DISAGREEMENTS,
                    detail: format!(
                        "at most {} ledger disagreement where the decider chose {} (has {})",
                        MAX_DISAGREEMENTS, v, disagreements
                    ),
                },
                Condition {
                    name: "counted_endpoint".to_string(),
                    met: endpoint_counted,
                    detail: format!(
                        "the fixture run used the default endpoint (it used a custom endpoint from \
                         {}; custom endpoints are excluded without --include-custom-endpoints)",
                        input.endpoint_origin.as_str()
                    ),
                },
            ];
            let reasons: Vec<String> = conditions
                .iter()
                .filter(|c| !c.met)
                .map(|c| c.detail.clone())
                .collect();
            let eligible = reasons.is_empty();
            let template_mode = field
                .decider
                .answers
                .get(v)
                .map_or(DeciderMode::DEFAULT, |a| a.mode);
            FixtureValue {
                value: v.clone(),
                template_mode,
                note: (template_mode == DeciderMode::Never)
                    .then(|| "(template: never)".to_string()),
                cases,
                recall: ratio(confusion.get(v, v), cases),
                false_positives,
                status: if eligible { "eligible" } else { "ineligible" }.to_string(),
                eligible,
                conditions,
                reasons,
            }
        })
        .collect();

    FixtureReport {
        template: input.template.clone(),
        state: input.state.clone(),
        field: field.name.to_string(),
        kind: match field.kind {
            DeclaredKind::Enum { .. } => "enum",
            DeclaredKind::Boolean => "boolean",
        }
        .to_string(),
        declaration_hash: input.declaration_hash.clone(),
        endpoint_origin: input.endpoint_origin,
        endpoint_counted,
        cases: total,
        labels,
        no_answer,
        macro_recall,
        majority_baseline,
        confusion,
        ledger: evidence,
        values: fixture_values,
        results,
    }
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// The whole report, as `--json` prints it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    pub ledger: String,
    pub header: Header,
    pub questions: Vec<QuestionReport>,
    pub fixtures: Option<FixtureReport>,
}

/// A share as a percentage with one decimal, or `-` when undefined.
pub fn pct(x: Option<f64>) -> String {
    match x {
        Some(x) => format!("{:.1}%", x * 100.0),
        None => "-".to_string(),
    }
}

fn opt_ms(x: Option<u64>) -> String {
    x.map_or("-".to_string(), |v| format!("{} ms", v))
}

fn render_confusion(out: &mut String, c: &Confusion, corner: &str) {
    let mut width = corner.len();
    for l in c.rows.keys().chain(c.columns.iter()) {
        width = width.max(l.len());
    }
    let _ = write!(out, "    {:<w$}", corner, w = width);
    for col in &c.columns {
        let _ = write!(out, "  {:>w$}", col, w = col.len().max(3));
    }
    out.push('\n');
    for (label, row) in &c.rows {
        let _ = write!(out, "    {:<w$}", label, w = width);
        for col in &c.columns {
            let n = row.get(col).copied().unwrap_or(0);
            let _ = write!(out, "  {:>w$}", n, w = col.len().max(3));
        }
        out.push('\n');
    }
}

/// The human-readable report. Shows the same numbers as the JSON.
pub fn render_table(report: &Report) -> String {
    let mut out = String::new();
    let h = &report.header;
    let _ = writeln!(out, "decider ledger: {}", report.ledger);
    let _ = writeln!(
        out,
        "  {} lines: {} consulted, {} answered; skipped {} malformed, {} unknown kind",
        h.lines, h.consulted, h.answered, h.skipped_malformed, h.unknown_kind
    );
    let _ = writeln!(
        out,
        "  {} duplicate consulted, {} orphaned answered, {} with no session id (never paired)",
        h.duplicate_consulted, h.orphaned_answered, h.null_session_id
    );
    let _ = writeln!(
        out,
        "  {} consultations from a custom endpoint; {} excluded from eligibility",
        h.custom_endpoint_consultations, h.excluded_from_eligibility
    );
    if report.questions.is_empty() {
        let _ = writeln!(out, "\nno consultations recorded");
    }
    for q in &report.questions {
        let _ = writeln!(out, "\nquestion {}.{}", q.state, q.field);
        let _ = writeln!(out, "  declaration hash {}", q.declaration_hash);
        let _ = writeln!(
            out,
            "  consultations {} (applied {}, not_applied {}, input_unavailable {}, error {}); \
             paired {} ({} counted toward eligibility)",
            q.consultations,
            q.outcomes.applied,
            q.outcomes.not_applied,
            q.outcomes.input_unavailable,
            q.outcomes.error,
            q.paired,
            q.counted_paired
        );
        let _ = writeln!(
            out,
            "  coverage {} of well-formed answers, {} of all consultations",
            pct(q.coverage.well_formed),
            pct(q.coverage.all)
        );
        let classes: Vec<String> = q
            .rates
            .error_by_class
            .iter()
            .map(|(class, r)| format!("{} {}", class, pct(Some(*r))))
            .collect();
        let errors = if classes.is_empty() {
            String::new()
        } else {
            format!(" ({})", classes.join(", "))
        };
        let _ = writeln!(
            out,
            "  fallback rate {}, error rate {}{}, input_unavailable rate {}",
            pct(Some(q.rates.fallback)),
            pct(Some(q.rates.error)),
            errors,
            pct(Some(q.rates.input_unavailable))
        );
        let _ = writeln!(
            out,
            "  latency p50 {}, p95 {} ({} samples)",
            opt_ms(q.latency_ms.p50),
            opt_ms(q.latency_ms.p95),
            q.latency_ms.samples
        );
        let _ = writeln!(
            out,
            "  agent stops removed {}, directive bytes not delivered {}",
            q.success_measures.agent_stops_removed,
            q.success_measures.directive_bytes_not_delivered
        );
        for f in &q.flags {
            let _ = writeln!(out, "  flag {}: {}", f.name, f.detail);
        }
        let _ = writeln!(
            out,
            "  {:<16} {:>7} {:>7} {:>8} {:>10} {:>14} {:>14}",
            "value", "mode", "paired", "recall", "coverage", "coverage(all)", "disagreements"
        );
        for v in &q.values {
            let _ = writeln!(
                out,
                "  {:<16} {:>7} {:>7} {:>8} {:>10} {:>14} {:>14}",
                v.value,
                v.mode.map_or("-", |m| m.as_str()),
                v.paired,
                pct(v.recall),
                pct(v.coverage.well_formed),
                pct(v.coverage.all),
                v.disagreements
            );
        }
        let _ = writeln!(out, "  confusion (rows: agent, columns: decider)");
        render_confusion(&mut out, &q.confusion, "agent");
        if !q.disagreements.is_empty() {
            let _ = writeln!(out, "  disagreements");
            for d in &q.disagreements {
                let _ = writeln!(
                    out,
                    "    {}/{} ({}): agent {}, decider {}",
                    d.session_id, d.visit_seq, d.session, d.agent, d.decider
                );
            }
        }
    }
    if let Some(f) = &report.fixtures {
        render_fixtures(&mut out, f);
    }
    out
}

fn render_fixtures(out: &mut String, f: &FixtureReport) {
    let _ = writeln!(out, "\nfixtures {} ({}.{})", f.template, f.state, f.field);
    let _ = writeln!(out, "  declaration hash {}", f.declaration_hash);
    let _ = writeln!(
        out,
        "  endpoint {} ({})",
        f.endpoint_origin.as_str(),
        if f.endpoint_counted {
            "counted toward eligibility"
        } else {
            "custom endpoint: excluded from eligibility without --include-custom-endpoints"
        }
    );
    let labels: Vec<String> = f
        .labels
        .iter()
        .map(|(k, v)| format!("{} {}", k, v))
        .collect();
    let _ = writeln!(
        out,
        "  cases {} ({}), no_answer {}",
        f.cases,
        labels.join(", "),
        f.no_answer
    );
    let _ = writeln!(
        out,
        "  macro recall {:.4}, majority baseline {:.4}",
        f.macro_recall, f.majority_baseline
    );
    let _ = writeln!(
        out,
        "  ledger pairs under this hash {} ({} counted, {} excluded as custom endpoint)",
        f.ledger.paired, f.ledger.counted_paired, f.ledger.excluded_custom_endpoint
    );
    let _ = writeln!(out, "  confusion (rows: expected, columns: decider)");
    render_confusion(out, &f.confusion, "expected");
    let _ = writeln!(out, "  promotion eligibility");
    for v in &f.values {
        let note = v
            .note
            .as_deref()
            .map_or(String::new(), |n| format!(" {}", n));
        let _ = writeln!(
            out,
            "    {}{}: {} (cases {}, recall {}, false positives {})",
            v.value,
            note,
            v.status,
            v.cases,
            pct(v.recall),
            v.false_positives
        );
        for r in &v.reasons {
            let _ = writeln!(out, "      not met: {}", r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decider::ledger::render_line;
    use crate::decider::record::DeciderConsultation;

    fn modes(auto: bool) -> BTreeMap<String, DeciderMode> {
        let m = if auto {
            DeciderMode::Auto
        } else {
            DeciderMode::Shadow
        };
        [("exit".to_string(), m), ("proceed".to_string(), m)]
            .into_iter()
            .collect()
    }

    fn consulted(
        sid: Option<&str>,
        seq: u64,
        outcome: ConsultationOutcome,
        winning: Option<&str>,
        at: bool,
    ) -> LedgerRecord {
        let fo = match (outcome, winning, at) {
            (ConsultationOutcome::Error | ConsultationOutcome::InputUnavailable, _, _) => None,
            (_, Some("unclear"), _) => Some(FieldOutcome::Escape),
            (_, Some(_), false) => Some(FieldOutcome::BelowThreshold),
            (_, Some(_), true) => Some(FieldOutcome::Shadow),
            (_, None, _) => Some(FieldOutcome::Escape),
        };
        let fc = FieldConsultation {
            declaration_hash: "h1".to_string(),
            modes: modes(false),
            probabilities: BTreeMap::new(),
            winning: winning.map(str::to_string),
            confidence: Some(0.95),
            threshold: Some(0.9),
            at_threshold: at,
            outcome: fo,
        };
        LedgerRecord::consulted(
            "wf",
            sid,
            DeciderConsultation {
                state: "review".to_string(),
                visit_seq: seq,
                provider: "jev".to_string(),
                model: "m".to_string(),
                input_sha256: None,
                outcome,
                error_class: (outcome == ConsultationOutcome::Error).then_some(ErrorClass::Timeout),
                latency_ms: seq * 10,
                directive_bytes: 100,
                endpoint_origin: SettingOrigin::Default,
                fields: [("verdict".to_string(), fc)].into_iter().collect(),
            },
        )
    }

    fn answered(sid: Option<&str>, seq: u64, v: &str) -> LedgerRecord {
        LedgerRecord::answered(
            "wf",
            sid,
            "review",
            seq,
            [("verdict".to_string(), Value::String(v.to_string()))]
                .into_iter()
                .collect(),
        )
    }

    fn body(records: &[LedgerRecord]) -> String {
        records
            .iter()
            .map(|r| render_line(r).unwrap() + "\n")
            .collect()
    }

    #[test]
    fn reader_skips_and_counts_bad_lines() {
        let mut b = body(&[consulted(
            Some("s"),
            1,
            ConsultationOutcome::NotApplied,
            Some("proceed"),
            true,
        )]);
        b.push_str("{not json\n");
        b.push_str("{\"kind\":\"answered\",\"v\":1}\n");
        b.push_str("{\"kind\":\"future\",\"v\":2}\n");
        b.push('\n');
        b.push_str(&render_line(&answered(Some("s"), 1, "proceed")).unwrap());
        let read = parse_ledger(b.as_bytes());
        assert_eq!(read.records.len(), 1);
        assert_eq!(read.lines, 5);
        assert_eq!(read.malformed, 3);
        assert_eq!(read.unknown_kind, 1);
    }

    #[test]
    fn nearest_rank_percentiles() {
        let xs: Vec<u64> = (1..=20).collect();
        assert_eq!(nearest_rank(&xs, 50), Some(10));
        assert_eq!(nearest_rank(&xs, 95), Some(19));
        assert_eq!(nearest_rank(&[7], 95), Some(7));
        assert_eq!(nearest_rank(&[], 50), None);
    }

    #[test]
    fn join_dedups_orphans_and_later_answers_win() {
        let records = vec![
            consulted(
                Some("s"),
                1,
                ConsultationOutcome::NotApplied,
                Some("proceed"),
                true,
            ),
            consulted(
                Some("s"),
                1,
                ConsultationOutcome::NotApplied,
                Some("proceed"),
                true,
            ),
            answered(Some("s"), 1, "proceed"),
            answered(Some("s"), 1, "exit"),
            answered(Some("s"), 9, "exit"),
            consulted(None, 2, ConsultationOutcome::NotApplied, Some("exit"), true),
            answered(None, 2, "exit"),
        ];
        let read = parse_ledger(body(&records).as_bytes());
        let r = analyze(&read, &ReportOptions::default());
        assert_eq!(r.header.duplicate_consulted, 1);
        assert_eq!(r.header.orphaned_answered, 2);
        assert_eq!(r.header.null_session_id, 1);
        let q = &r.questions[0];
        assert_eq!(q.consultations, 2);
        assert_eq!(q.paired, 1);
        assert_eq!(q.confusion.get("exit", "proceed"), 1);
        assert_eq!(q.disagreements.len(), 1);
        assert_eq!(q.value("proceed").unwrap().disagreements, 1);
    }
}
