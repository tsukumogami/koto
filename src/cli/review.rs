//! `koto review record`: record document review events as telemetry.
//!
//! A caller running a document review (a jury of reviewer seats over a
//! brief, PRD, design, plan, vision, strategy or roadmap) records what
//! happened through four events: a round starting, each seat's verdict per
//! rule and section, the decision that follows the round, and publish-time
//! document check results. Each appends typed session events, and the run
//! journal relays each event as one telemetry record (see
//! `crate::run_journal`).
//!
//! Three more record what happens to the document while it is drafted:
//! `rule-checks` (each check-held rule's outcome at one moment),
//! `section-written` (a section written for the first time or rewritten)
//! and `fix` (the rules and sections a fix answers). Each call appends one
//! session event holding all its entries, and the journal relays one record
//! per entry, so the records of one call share the event's `koto.seq`.
//!
//! One more records a code review round (a pull request review panel) as
//! kinds of its own, apart from the document review events: `code-round`
//! takes the round's number, the reviewed head and a batch of seats, each
//! with the model it ran on and its verdict, and appends one event holding
//! them all. The head is stored only as its digest, never as given, so a
//! commit id doesn't reach the log or the journal.
//!
//! Like `koto disputes record`, the events are recorded, never obeyed:
//! nothing the advance loop routes on reads them, so no gate outcome, route
//! or state changes, and the verb works on a session in any non-terminal
//! state.
//!
//! Every value is checked before anything is appended, and only koto's own
//! words for it are stored: the document type and format, verdict,
//! decision, check outcome, rule outcome, moment, write and what a fix
//! answers come from closed sets; the schema is bucketed into a closed set
//! plus `other`; a seat or section is stored as given only when the
//! session's template declares it (`review_vocabulary`), and as `other`
//! otherwise; a rule id, and a fix's check rule, must have the journal's
//! name shape; a round and a seat count are positive integers, and a fix's
//! round is at most [`FIX_ROUND_MAX`]. One invalid entry refuses the whole
//! batch, so a refused call appends nothing. A batch holds at most
//! [`BATCH_MAX`] entries, and a session holds at most [`SESSION_CAP`]
//! recorded review entries; a call that would pass the cap appends nothing.
//!
//! Errors name the entry (its 0-based index in the `--file` array) and the
//! field, and never echo a value the caller gave, so a document's text that
//! ended up in a seat's verdict can't come back out through an error.
//!
//! Errors print a JSON body on stdout: exit 1 for a missing session or an
//! append that fails; exit 2 with a typed `code` for a caller problem (a
//! terminal session, a value outside its set, a malformed file, a batch too
//! large, the session's cap reached), nothing appended; exit 3 when the
//! session's state or template can't be read.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Subcommand;
use serde_json::{Map, Value};

use crate::engine::persistence::derive_machine_state;
use crate::engine::types::{
    now_iso8601, CodeReviewRoundRecorded, CodeReviewSeat, DocCheckRecorded, DocFix, DocFixRecorded,
    DocRuleCheck, DocRuleChecksRecorded, DocSectionRecorded, Event, EventPayload,
    ReviewDecisionRecorded, ReviewRoundStarted, ReviewVerdictRecorded,
};
use crate::session::SessionBackend;
use crate::template::types::{CompiledTemplate, ReviewVocabulary};

use super::{exit_code_for_engine_error, exit_with_error_code, EXIT_INFRASTRUCTURE};

/// The command name every error body carries.
const COMMAND: &str = "review record";

/// The session is at a terminal state or was cancelled.
pub const SESSION_TERMINAL: &str = "session_terminal";
/// A flag's or an entry field's value is outside its set or bounds.
pub const INVALID_VALUE: &str = "invalid_value";
/// A `--file` entry isn't an object, lacks a field, has an unknown field or
/// a field of the wrong type.
pub const INVALID_ENTRY: &str = "invalid_entry";
/// The `--file` can't be read, is too large, isn't JSON or isn't an array.
pub const INVALID_FILE: &str = "invalid_file";
/// The `--file` array holds more than [`BATCH_MAX`] entries.
pub const BATCH_TOO_LARGE: &str = "batch_too_large";
/// The call would take the session past [`SESSION_CAP`] review events.
pub const CAP_REACHED: &str = "cap_reached";

/// The most entries one `verdicts`, `doc-checks`, `rule-checks`, `fix` or
/// `code-round` call records.
pub const BATCH_MAX: usize = 200;
/// The most review entries one session holds, every kind counted: one for
/// each round, verdict, decision, document check or section write, one per
/// entry of a `rule-checks` or `fix` call, and for a `code-round` call one
/// for the round and one per seat.
pub const SESSION_CAP: usize = 2000;
/// The longest `--head` a `code-round` call takes, in bytes. The value is
/// only ever stored as its digest.
pub const HEAD_MAX_BYTES: usize = 256;
/// The largest `--file` read, in bytes: far more than [`BATCH_MAX`] entries
/// of the longest valid values need.
pub const FILE_MAX_BYTES: u64 = 1024 * 1024;

/// The document types a review event names.
pub const DOC_TYPES: &[&str] = &["brief", "prd", "design", "plan"];
/// The schema values stored as given; any other non-empty value is
/// [`SCHEMA_OTHER`] and a missing or empty one [`SCHEMA_ABSENT`].
pub const SCHEMA_BUCKETS: &[&str] = &["brief/v1", "prd/v1", "design/v1", "plan/v1", "plan/v2"];
/// The schema bucket for a document with no schema value.
pub const SCHEMA_ABSENT: &str = "absent";
/// The schema bucket for any schema value outside [`SCHEMA_BUCKETS`].
pub const SCHEMA_OTHER: &str = "other";
/// What a seat or section the session's template doesn't declare is stored
/// as.
pub const UNDECLARED: &str = "other";
/// The decisions that may follow a review round.
pub const DECISIONS: &[&str] = &["rerun", "fix_and_proceed", "loop_back", "proceed", "stop"];
/// The outcomes of a document check.
pub const CHECK_OUTCOMES: &[&str] = &["passed", "failed", "error"];
/// The document formats `--doc-format` names: the four document types and
/// the three charter formats.
pub const DOC_FORMATS: &[&str] = &[
    "brief", "prd", "design", "plan", "vision", "strategy", "roadmap",
];
/// The outcomes of one rule in a `rule-checks` call.
pub const RULE_OUTCOMES: &[&str] = &["pass", "fail", "not_ready", "error"];
/// When a `rule-checks` call's checks ran.
pub const CHECK_MOMENTS: &[&str] = &["pre_jury", "post_fix", "post_create"];
/// How a `section-written` call's section was written.
pub const SECTION_WRITES: &[&str] = &["first", "rewrite"];
/// What a `fix` call answers.
pub const FIX_ANSWERS: &[&str] = &["jury_fail", "check_fail"];
/// The `fix` answer that takes a `--round`.
pub const JURY_FAIL: &str = "jury_fail";
/// The highest jury round a `fix` names.
pub const FIX_ROUND_MAX: u64 = 50;

/// The keys of a `verdicts` entry; `schema` may be absent or null, and
/// `doc_type` may be absent when the call gives `--doc-format`.
const VERDICT_KEYS: &[&str] = &[
    "rule_id", "section", "verdict", "seat", "round", "doc_type", "schema",
];
/// The keys of a `doc-checks` entry; `schema` may be absent or null.
const DOC_CHECK_KEYS: &[&str] = &["rule_id", "outcome", "doc_type", "schema"];
/// The keys of a `rule-checks` entry, all required.
const RULE_CHECK_KEYS: &[&str] = &["rule_id", "outcome"];
/// The keys of a `fix` entry, all required.
const FIX_KEYS: &[&str] = &["rule_id", "section", "check_rule"];
/// The keys of a `code-round` entry, all required.
const CODE_SEAT_KEYS: &[&str] = &["seat", "model", "verdict"];

#[derive(Subcommand)]
pub enum ReviewSubcommand {
    /// Record a document or code review event as telemetry. Changes no gate
    /// outcome, route or state
    Record {
        /// Workflow name (required)
        //
        // Optional to clap and required by the handler: a required
        // positional ahead of a subcommand can't be filled in by tooling
        // that builds a verb's argv from its leaf's own required arguments
        // (the `--workspace` guard's test walks every verb that way). A
        // missing name is still refused, with exit 2, before anything runs.
        #[arg(value_name = "NAME")]
        name: Option<String>,

        #[command(subcommand)]
        event: ReviewEvent,
    },
}

/// The review events.
///
/// Every value flag is a `String`, numbers included, and is checked by
/// this module rather than by clap: clap's own refusal of a value it can't
/// parse repeats the value, and koto's never does (see [`Refusal`]).
///
/// `round-started`, `verdicts` and `decision` take `--doc-format` beside
/// `--doc-type`; `round-started` and `decision` need one of the two, and
/// when both are given they must name the same document.
#[derive(Subcommand)]
pub enum ReviewEvent {
    /// A review round's seats were spawned
    RoundStarted {
        /// The document type: brief, prd, design or plan
        #[arg(long = "doc-type")]
        doc_type: Option<String>,
        /// The document format: brief, prd, design, plan, vision, strategy
        /// or roadmap
        #[arg(long = "doc-format")]
        doc_format: Option<String>,
        /// The document's schema value (e.g. plan/v2); omitted or empty
        /// records `absent`, an unknown value `other`
        #[arg(long)]
        schema: Option<String>,
        /// The round's number, a positive integer
        #[arg(long)]
        round: String,
        /// How many seats the round spawned, a positive integer
        #[arg(long = "seat-count")]
        seat_count: String,
        /// The round is a fallback round
        #[arg(long)]
        fallback: bool,
    },
    /// A batch of seat verdicts: a JSON array of objects with rule_id,
    /// section, verdict (PASS or FAIL), seat, round, doc_type and an
    /// optional schema, at most 200; doc_type is optional with --doc-format
    Verdicts {
        /// The JSON file holding the batch
        #[arg(long)]
        file: PathBuf,
        /// The document format: brief, prd, design, plan, vision, strategy
        /// or roadmap
        #[arg(long = "doc-format")]
        doc_format: Option<String>,
    },
    /// What follows a review round
    Decision {
        /// The document type: brief, prd, design or plan
        #[arg(long = "doc-type")]
        doc_type: Option<String>,
        /// The document format: brief, prd, design, plan, vision, strategy
        /// or roadmap
        #[arg(long = "doc-format")]
        doc_format: Option<String>,
        /// The document's schema value; omitted or empty records `absent`
        #[arg(long)]
        schema: Option<String>,
        /// The round's number, a positive integer
        #[arg(long)]
        round: String,
        /// rerun, fix_and_proceed, loop_back, proceed or stop
        #[arg(long)]
        decision: String,
    },
    /// A batch of document check results: a JSON array of objects with
    /// rule_id, outcome (passed, failed or error), doc_type and an optional
    /// schema, at most 200
    DocChecks {
        /// The JSON file holding the batch
        #[arg(long)]
        file: PathBuf,
    },
    /// The outcome of each check-held rule of a document at one moment: a
    /// JSON array of objects with rule_id and outcome (pass, fail,
    /// not_ready or error), at most 200
    RuleChecks {
        /// The document format: brief, prd, design, plan, vision, strategy
        /// or roadmap
        #[arg(long = "doc-format")]
        doc_format: String,
        /// When the checks ran: pre_jury, post_fix or post_create
        #[arg(long)]
        moment: String,
        /// The JSON file holding the batch
        #[arg(long)]
        file: PathBuf,
    },
    /// A document section was written for the first time or rewritten
    SectionWritten {
        /// The document format: brief, prd, design, plan, vision, strategy
        /// or roadmap
        #[arg(long = "doc-format")]
        doc_format: String,
        /// The section's slug; one the session's template doesn't declare
        /// records `other`
        #[arg(long)]
        section: String,
        /// first or rewrite
        #[arg(long)]
        write: String,
    },
    /// A fix applied to a document: a JSON array of objects with rule_id,
    /// section and check_rule (a rule id, or none), at most 200
    Fix {
        /// The document format: brief, prd, design, plan, vision, strategy
        /// or roadmap
        #[arg(long = "doc-format")]
        doc_format: String,
        /// What the fix answers: jury_fail or check_fail
        #[arg(long)]
        answers: String,
        /// The jury round the fix answers, 1 to 50; only with jury_fail
        #[arg(long)]
        round: Option<String>,
        /// The JSON file holding the batch
        #[arg(long)]
        file: PathBuf,
    },
    /// A code review round (a pull request review panel), recorded apart
    /// from document reviews: a JSON array of objects with seat, model and
    /// verdict (pass or fail), one per seat, at most 200
    CodeRound {
        /// The round's number, a positive integer
        #[arg(long)]
        round: String,
        /// The reviewed head (a commit id, say), 1 to 256 bytes; stored
        /// only as its digest
        #[arg(long)]
        head: String,
        /// The JSON file holding the seats
        #[arg(long)]
        file: PathBuf,
    },
}

impl ReviewEvent {
    /// The subcommand's name, as the success body echoes it.
    fn name(&self) -> &'static str {
        match self {
            ReviewEvent::RoundStarted { .. } => "round-started",
            ReviewEvent::Verdicts { .. } => "verdicts",
            ReviewEvent::Decision { .. } => "decision",
            ReviewEvent::DocChecks { .. } => "doc-checks",
            ReviewEvent::RuleChecks { .. } => "rule-checks",
            ReviewEvent::SectionWritten { .. } => "section-written",
            ReviewEvent::Fix { .. } => "fix",
            ReviewEvent::CodeRound { .. } => "code-round",
        }
    }
}

/// A caller problem: exit 2, nothing appended. `entry` is the 0-based index
/// of the `--file` entry at fault and `field` the flag or key; neither the
/// message nor any other part of it carries a value the caller gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: &'static str,
    pub message: String,
    pub entry: Option<usize>,
    pub field: Option<String>,
}

impl Refusal {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Refusal {
            code,
            message: message.into(),
            entry: None,
            field: None,
        }
    }

    fn field(mut self, field: &str) -> Self {
        self.field = Some(field.to_string());
        self
    }

    fn entry(mut self, index: usize) -> Self {
        self.entry = Some(index);
        self
    }

    fn body(&self) -> Value {
        let mut error = serde_json::json!({
            "code": self.code,
            "message": self.message,
        });
        if let Some(entry) = self.entry {
            error["entry"] = serde_json::json!(entry);
        }
        if let Some(field) = &self.field {
            error["field"] = serde_json::json!(field);
        }
        serde_json::json!({"error": error, "command": COMMAND})
    }
}

/// A refusal of `field`'s value, naming the allowed set.
fn bad_value(field: &str, allowed: &str) -> Refusal {
    Refusal::new(INVALID_VALUE, format!("{field} must be {allowed}")).field(field)
}

fn one_of(set: &[&str]) -> String {
    format!("one of {}", set.join(", "))
}

/// The closed-set value `value` names, as koto stores it.
fn closed(value: &str, set: &[&'static str]) -> Option<&'static str> {
    set.iter().copied().find(|s| *s == value)
}

/// `brief`, `prd`, `design` or `plan`.
pub fn doc_type(value: &str) -> Option<&'static str> {
    closed(value, DOC_TYPES)
}

/// `pass` or `fail`, from `PASS`, `FAIL` or the lower-case forms.
pub fn verdict(value: &str) -> Option<&'static str> {
    match value {
        "PASS" | "pass" => Some("pass"),
        "FAIL" | "fail" => Some("fail"),
        _ => None,
    }
}

/// One of [`DECISIONS`].
pub fn decision(value: &str) -> Option<&'static str> {
    closed(value, DECISIONS)
}

/// One of [`CHECK_OUTCOMES`].
pub fn check_outcome(value: &str) -> Option<&'static str> {
    closed(value, CHECK_OUTCOMES)
}

/// The schema bucket a document's schema value falls in: the value itself
/// when it is one of [`SCHEMA_BUCKETS`] (surrounding whitespace ignored),
/// [`SCHEMA_ABSENT`] for no value or an empty one, and [`SCHEMA_OTHER`] for
/// anything else. The value is author text, so it is never stored as
/// written.
pub fn bucket_schema(value: Option<&str>) -> &'static str {
    match value.map(str::trim) {
        None | Some("") => SCHEMA_ABSENT,
        Some(v) => closed(v, SCHEMA_BUCKETS).unwrap_or(SCHEMA_OTHER),
    }
}

/// `value` when `declared` holds it, otherwise [`UNDECLARED`].
pub fn declared_or_other(value: &str, declared: &[String]) -> String {
    if declared.iter().any(|d| d == value) {
        value.to_string()
    } else {
        UNDECLARED.to_string()
    }
}

/// Whether `value` is a rule id koto records: 1 to 128 bytes in the run
/// journal's name shape, the bound koto puts on a finding's rule id.
pub fn valid_rule_id(value: &str) -> bool {
    value.len() <= crate::findings::RULE_ID_MAX_BYTES && crate::run_journal::is_name(value)
}

/// A positive integer written in decimal digits.
pub fn positive(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse::<u64>().ok().filter(|n| *n > 0)
}

/// One of [`DOC_FORMATS`].
pub fn doc_format(value: &str) -> Option<&'static str> {
    closed(value, DOC_FORMATS)
}

/// The checked `--doc-format`, refused when it is outside [`DOC_FORMATS`].
fn format_flag(value: &str) -> Result<&'static str, Refusal> {
    doc_format(value).ok_or_else(|| bad_value("--doc-format", &one_of(DOC_FORMATS)))
}

/// The document a review event names, as `(doc_type, doc_format)`, from
/// `--doc-type` and `--doc-format`. `--doc-format` alone gives the type
/// when the format is one of the four types, and none for a charter
/// format; both together must name the same document; neither is refused.
pub fn doc_identity(
    doc_type_flag: Option<&str>,
    doc_format_flag: Option<&str>,
) -> Result<(Option<String>, Option<String>), Refusal> {
    let format = doc_format_flag.map(format_flag).transpose()?;
    let typed = doc_type_flag
        .map(|t| doc_type(t).ok_or_else(|| bad_value("--doc-type", &one_of(DOC_TYPES))))
        .transpose()?;
    match (typed, format) {
        (None, None) => Err(Refusal::new(
            INVALID_VALUE,
            "one of --doc-type or --doc-format is required",
        )
        .field("--doc-type")),
        (Some(t), Some(f)) if t != f => Err(Refusal::new(
            INVALID_VALUE,
            "--doc-type and --doc-format must name the same document",
        )
        .field("--doc-type")),
        (t, f) => Ok((
            t.or_else(|| f.and_then(doc_type)).map(str::to_string),
            f.map(str::to_string),
        )),
    }
}

/// The `round-started` event, from its flags.
pub fn round_started(
    doc_type_flag: Option<&str>,
    doc_format_flag: Option<&str>,
    schema: Option<&str>,
    round: &str,
    seat_count: &str,
    fallback: bool,
) -> Result<ReviewRoundStarted, Refusal> {
    let (doc_type, doc_format) = doc_identity(doc_type_flag, doc_format_flag)?;
    Ok(ReviewRoundStarted {
        doc_type,
        doc_format,
        schema: bucket_schema(schema).to_string(),
        round: positive(round).ok_or_else(|| bad_value("--round", "a positive integer"))?,
        seat_count: positive(seat_count)
            .ok_or_else(|| bad_value("--seat-count", "a positive integer"))?,
        fallback,
    })
}

/// The `decision` event, from its flags.
pub fn decision_recorded(
    doc_type_flag: Option<&str>,
    doc_format_flag: Option<&str>,
    schema: Option<&str>,
    round: &str,
    decision_flag: &str,
) -> Result<ReviewDecisionRecorded, Refusal> {
    let (doc_type, doc_format) = doc_identity(doc_type_flag, doc_format_flag)?;
    Ok(ReviewDecisionRecorded {
        doc_type,
        doc_format,
        schema: bucket_schema(schema).to_string(),
        round: positive(round).ok_or_else(|| bad_value("--round", "a positive integer"))?,
        decision: decision(decision_flag)
            .ok_or_else(|| bad_value("--decision", &one_of(DECISIONS)))?
            .to_string(),
    })
}

/// The entries of a `--file` batch: a JSON array of at most [`BATCH_MAX`]
/// objects. Errors carry the parser's line and column, never its text.
pub fn batch_entries(text: &str) -> Result<Vec<Value>, Refusal> {
    let value: Value = serde_json::from_str(text).map_err(|e| {
        Refusal::new(
            INVALID_FILE,
            format!(
                "--file is not valid JSON (line {}, column {})",
                e.line(),
                e.column()
            ),
        )
        .field("--file")
    })?;
    let Value::Array(entries) = value else {
        return Err(
            Refusal::new(INVALID_FILE, "--file must hold a JSON array of entries").field("--file"),
        );
    };
    if entries.len() > BATCH_MAX {
        return Err(Refusal::new(
            BATCH_TOO_LARGE,
            format!(
                "--file holds {} entries; a batch holds at most {}",
                entries.len(),
                BATCH_MAX
            ),
        )
        .field("--file"));
    }
    Ok(entries)
}

/// One `--file` entry, checked for shape: an object whose keys are all in
/// `keys`. An unknown key is reported without its name, since the name is
/// the caller's text.
fn entry_object<'v>(
    index: usize,
    entry: &'v Value,
    keys: &[&str],
) -> Result<&'v Map<String, Value>, Refusal> {
    let Value::Object(map) = entry else {
        return Err(
            Refusal::new(INVALID_ENTRY, format!("entry {index} must be an object")).entry(index),
        );
    };
    if map.keys().any(|k| !keys.contains(&k.as_str())) {
        return Err(Refusal::new(
            INVALID_ENTRY,
            format!(
                "entry {index} has an unknown field; the fields are {}",
                keys.join(", ")
            ),
        )
        .entry(index));
    }
    Ok(map)
}

/// `key`'s string value in entry `index`.
fn string_field<'v>(
    index: usize,
    map: &'v Map<String, Value>,
    key: &str,
) -> Result<&'v str, Refusal> {
    match map.get(key) {
        None => Err(Refusal::new(
            INVALID_ENTRY,
            format!("entry {index}: field {key} is missing"),
        )
        .entry(index)
        .field(key)),
        Some(Value::String(s)) => Ok(s.as_str()),
        Some(_) => Err(Refusal::new(
            INVALID_ENTRY,
            format!("entry {index}: field {key} must be a string"),
        )
        .entry(index)
        .field(key)),
    }
}

/// The optional `schema` of entry `index`: absent, null or a string.
fn schema_field(index: usize, map: &Map<String, Value>) -> Result<Option<&str>, Refusal> {
    match map.get("schema") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(Refusal::new(
            INVALID_ENTRY,
            format!("entry {index}: field schema must be a string or null"),
        )
        .entry(index)
        .field("schema")),
    }
}

/// A value outside its set in entry `index`.
fn bad_entry_value(index: usize, key: &str, allowed: &str) -> Refusal {
    Refusal::new(
        INVALID_VALUE,
        format!("entry {index}: field {key} must be {allowed}"),
    )
    .entry(index)
    .field(key)
}

fn entry_doc_type(index: usize, map: &Map<String, Value>) -> Result<String, Refusal> {
    let value = string_field(index, map, "doc_type")?;
    doc_type(value)
        .map(str::to_string)
        .ok_or_else(|| bad_entry_value(index, "doc_type", &one_of(DOC_TYPES)))
}

/// A `verdicts` entry's document type under the call's `--doc-format`.
/// Without the flag the entry's `doc_type` is required, as it always was.
/// With it the entry's `doc_type` may be left out, and when given must name
/// the flag's document; the type is the format's own when the format is
/// one of the four types, and none for a charter format.
fn entry_doc_type_under(
    index: usize,
    map: &Map<String, Value>,
    format: Option<&'static str>,
) -> Result<Option<String>, Refusal> {
    let Some(format) = format else {
        return entry_doc_type(index, map).map(Some);
    };
    if map.contains_key("doc_type") {
        let given = entry_doc_type(index, map)?;
        if Some(given.as_str()) != doc_type(format) {
            return Err(bad_entry_value(
                index,
                "doc_type",
                "the document --doc-format names, or left out",
            ));
        }
    }
    Ok(doc_type(format).map(str::to_string))
}

/// A rule id field of entry `index` (`rule_id`, or a fix's `check_rule`),
/// in the bounds koto puts on `koto.rule.id`.
fn entry_rule_field(index: usize, map: &Map<String, Value>, key: &str) -> Result<String, Refusal> {
    let value = string_field(index, map, key)?;
    if valid_rule_id(value) {
        Ok(value.to_string())
    } else {
        Err(bad_entry_value(
            index,
            key,
            "1 to 128 characters from A-Z a-z 0-9 . _ : / @ -, not starting with / or ~",
        ))
    }
}

fn entry_rule_id(index: usize, map: &Map<String, Value>) -> Result<String, Refusal> {
    entry_rule_field(index, map, "rule_id")
}

/// The verdict events of a `verdicts` batch, one per entry, in order. A seat
/// or section `vocabulary` doesn't declare is stored as `other`. With
/// `--doc-format` every event carries the format (see
/// [`entry_doc_type_under`] for the type).
pub fn verdict_entries(
    text: &str,
    vocabulary: &ReviewVocabulary,
    doc_format_flag: Option<&str>,
) -> Result<Vec<ReviewVerdictRecorded>, Refusal> {
    let format = doc_format_flag.map(format_flag).transpose()?;
    let entries = batch_entries(text)?;
    let mut out = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let map = entry_object(index, entry, VERDICT_KEYS)?;
        let doc_type = entry_doc_type_under(index, map, format)?;
        let schema = bucket_schema(schema_field(index, map)?).to_string();
        let round = match map.get("round") {
            None => {
                return Err(Refusal::new(
                    INVALID_ENTRY,
                    format!("entry {index}: field round is missing"),
                )
                .entry(index)
                .field("round"))
            }
            Some(v) => v
                .as_u64()
                .filter(|n| *n > 0)
                .ok_or_else(|| bad_entry_value(index, "round", "a positive integer"))?,
        };
        let seat = declared_or_other(string_field(index, map, "seat")?, &vocabulary.seats);
        let rule_id = entry_rule_id(index, map)?;
        let section = declared_or_other(string_field(index, map, "section")?, &vocabulary.sections);
        let verdict = verdict(string_field(index, map, "verdict")?)
            .ok_or_else(|| bad_entry_value(index, "verdict", "PASS or FAIL"))?
            .to_string();
        out.push(ReviewVerdictRecorded {
            doc_type,
            doc_format: format.map(str::to_string),
            schema,
            round,
            seat,
            rule_id,
            section,
            verdict,
        });
    }
    Ok(out)
}

/// The document check events of a `doc-checks` batch, one per entry, in
/// order.
pub fn doc_check_entries(text: &str) -> Result<Vec<DocCheckRecorded>, Refusal> {
    let entries = batch_entries(text)?;
    let mut out = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let map = entry_object(index, entry, DOC_CHECK_KEYS)?;
        let doc_type = entry_doc_type(index, map)?;
        let schema = bucket_schema(schema_field(index, map)?).to_string();
        let rule_id = entry_rule_id(index, map)?;
        let outcome = check_outcome(string_field(index, map, "outcome")?)
            .ok_or_else(|| bad_entry_value(index, "outcome", &one_of(CHECK_OUTCOMES)))?
            .to_string();
        out.push(DocCheckRecorded {
            doc_type,
            schema,
            rule_id,
            outcome,
        });
    }
    Ok(out)
}

/// The `rule-checks` event, from its flags and its batch, recorded at
/// `state`. Every entry goes in the one event.
pub fn rule_checks_recorded(
    state: &str,
    doc_format_flag: &str,
    moment_flag: &str,
    text: &str,
) -> Result<DocRuleChecksRecorded, Refusal> {
    let doc_format = format_flag(doc_format_flag)?;
    let moment = closed(moment_flag, CHECK_MOMENTS)
        .ok_or_else(|| bad_value("--moment", &one_of(CHECK_MOMENTS)))?;
    let entries = batch_entries(text)?;
    let mut out = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let map = entry_object(index, entry, RULE_CHECK_KEYS)?;
        let rule_id = entry_rule_id(index, map)?;
        let outcome = closed(string_field(index, map, "outcome")?, RULE_OUTCOMES)
            .ok_or_else(|| bad_entry_value(index, "outcome", &one_of(RULE_OUTCOMES)))?
            .to_string();
        out.push(DocRuleCheck { rule_id, outcome });
    }
    Ok(DocRuleChecksRecorded {
        state: state.to_string(),
        doc_format: doc_format.to_string(),
        moment: moment.to_string(),
        entries: out,
    })
}

/// The `section-written` event, from its flags, recorded at `state`. A
/// section `vocabulary` doesn't declare is stored as `other`.
pub fn section_recorded(
    state: &str,
    doc_format_flag: &str,
    section: &str,
    write_flag: &str,
    vocabulary: &ReviewVocabulary,
) -> Result<DocSectionRecorded, Refusal> {
    let doc_format = format_flag(doc_format_flag)?;
    let write = closed(write_flag, SECTION_WRITES)
        .ok_or_else(|| bad_value("--write", &one_of(SECTION_WRITES)))?;
    Ok(DocSectionRecorded {
        state: state.to_string(),
        doc_format: doc_format.to_string(),
        section: declared_or_other(section, &vocabulary.sections),
        write: write.to_string(),
    })
}

/// The `fix` event, from its flags and its batch, recorded at `state`.
/// `--round` is taken only with `--answers jury_fail`, from 1 to
/// [`FIX_ROUND_MAX`]. A section `vocabulary` doesn't declare is stored as
/// `other`; `check_rule` is a rule id in `koto.rule.id`'s bounds, `none`
/// included.
pub fn fix_recorded(
    state: &str,
    doc_format_flag: &str,
    answers_flag: &str,
    round: Option<&str>,
    text: &str,
    vocabulary: &ReviewVocabulary,
) -> Result<DocFixRecorded, Refusal> {
    let doc_format = format_flag(doc_format_flag)?;
    let answers = closed(answers_flag, FIX_ANSWERS)
        .ok_or_else(|| bad_value("--answers", &one_of(FIX_ANSWERS)))?;
    let round = match round {
        None => None,
        Some(_) if answers != JURY_FAIL => {
            return Err(Refusal::new(
                INVALID_VALUE,
                "--round is taken only with --answers jury_fail",
            )
            .field("--round"))
        }
        Some(r) => Some(positive(r).filter(|n| *n <= FIX_ROUND_MAX).ok_or_else(|| {
            bad_value("--round", &format!("an integer from 1 to {FIX_ROUND_MAX}"))
        })?),
    };
    let entries = batch_entries(text)?;
    let mut out = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let map = entry_object(index, entry, FIX_KEYS)?;
        let rule_id = entry_rule_id(index, map)?;
        let section = declared_or_other(string_field(index, map, "section")?, &vocabulary.sections);
        let check_rule = entry_rule_field(index, map, "check_rule")?;
        out.push(DocFix {
            rule_id,
            section,
            check_rule,
        });
    }
    Ok(DocFixRecorded {
        state: state.to_string(),
        doc_format: doc_format.to_string(),
        answers: answers.to_string(),
        round,
        entries: out,
    })
}

/// A `code-round` call's `--round` and `--head`, as `(round, head digest)`.
/// The round is a positive integer; the head is 1 to [`HEAD_MAX_BYTES`]
/// bytes of any text and is kept only as its digest
/// ([`crate::run_journal::digest`]), so the same head gives the same value
/// in every session and the value given is never stored.
pub fn code_round_flags(round: &str, head: &str) -> Result<(u64, String), Refusal> {
    let round = positive(round).ok_or_else(|| bad_value("--round", "a positive integer"))?;
    if head.is_empty() || head.len() > HEAD_MAX_BYTES {
        return Err(bad_value("--head", &format!("1 to {HEAD_MAX_BYTES} bytes")));
    }
    Ok((round, crate::run_journal::digest(head)))
}

/// A seat's model as a `code-round` call stores it: as given when it has the
/// journal's name shape (1 to 128 characters from `A-Z a-z 0-9 . _ : / @ -`,
/// not starting with `/` or `~`), and [`UNDECLARED`] (`other`) otherwise,
/// such as a model id with a `[1m]` suffix. Callers record best effort, so
/// an odd model id costs that seat's model, not the round.
pub fn code_model(value: &str) -> String {
    if crate::run_journal::is_name(value) {
        value.to_string()
    } else {
        UNDECLARED.to_string()
    }
}

/// The `code-round` event, from its flags and its batch of seats, recorded
/// at `state`. Every seat goes in the one event, in order. A seat
/// `vocabulary` doesn't declare is stored as `other`, and so is a model
/// outside the journal's name shape ([`code_model`]); a missing or empty
/// model is refused. A verdict is `pass` or `fail` (`PASS` and `FAIL` are
/// taken too).
pub fn code_round_recorded(
    state: &str,
    round: &str,
    head: &str,
    text: &str,
    vocabulary: &ReviewVocabulary,
) -> Result<CodeReviewRoundRecorded, Refusal> {
    let (round, head) = code_round_flags(round, head)?;
    code_round_seats(state, round, head, text, vocabulary)
}

/// [`code_round_recorded`] from flags [`code_round_flags`] already checked:
/// `round` and `head`, the head's digest, as it returned them.
fn code_round_seats(
    state: &str,
    round: u64,
    head: String,
    text: &str,
    vocabulary: &ReviewVocabulary,
) -> Result<CodeReviewRoundRecorded, Refusal> {
    let entries = batch_entries(text)?;
    let mut out = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let map = entry_object(index, entry, CODE_SEAT_KEYS)?;
        let seat = declared_or_other(string_field(index, map, "seat")?, &vocabulary.seats);
        let model = string_field(index, map, "model")?;
        if model.is_empty() {
            return Err(bad_entry_value(index, "model", "a non-empty string"));
        }
        let model = code_model(model);
        let verdict = verdict(string_field(index, map, "verdict")?)
            .ok_or_else(|| bad_entry_value(index, "verdict", "pass or fail"))?
            .to_string();
        out.push(CodeReviewSeat {
            seat,
            model,
            verdict,
        });
    }
    Ok(CodeReviewRoundRecorded {
        state: state.to_string(),
        round,
        head,
        entries: out,
    })
}

/// The text of a `--file`, refused when it can't be read, is larger than
/// [`FILE_MAX_BYTES`] or isn't UTF-8. The refusal names the cause, never
/// the path or the content.
pub fn read_batch_file(path: &Path) -> Result<String, Refusal> {
    let unreadable = |cause: &str| {
        Refusal::new(INVALID_FILE, format!("--file could not be read: {cause}")).field("--file")
    };
    let file = std::fs::File::open(path).map_err(|e| unreadable(&io_cause(&e)))?;
    let mut bytes = Vec::new();
    file.take(FILE_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| unreadable(&io_cause(&e)))?;
    if bytes.len() as u64 > FILE_MAX_BYTES {
        return Err(Refusal::new(
            INVALID_FILE,
            format!("--file is larger than {FILE_MAX_BYTES} bytes"),
        )
        .field("--file"));
    }
    String::from_utf8(bytes)
        .map_err(|_| Refusal::new(INVALID_FILE, "--file is not UTF-8 text").field("--file"))
}

/// An I/O error's kind in words, without the path the OS message may hold.
fn io_cause(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "no such file".to_string(),
        std::io::ErrorKind::PermissionDenied => "permission denied".to_string(),
        std::io::ErrorKind::IsADirectory => "it is a directory".to_string(),
        other => format!("{other:?}"),
    }
}

/// How many review entries `payload` holds, the count [`SESSION_CAP`]
/// bounds: one for each round, verdict, decision, document check and
/// section write, the entry count of a `rule-checks` or `fix` event, one
/// for a code review round and one per seat of it (none when it has no
/// seats), and 0 for anything else.
pub fn review_entries(payload: &EventPayload) -> usize {
    match payload {
        EventPayload::ReviewRoundStarted(_)
        | EventPayload::ReviewVerdictRecorded(_)
        | EventPayload::ReviewDecisionRecorded(_)
        | EventPayload::DocCheckRecorded(_)
        | EventPayload::DocSectionRecorded(_) => 1,
        EventPayload::DocRuleChecksRecorded(c) => c.entries.len(),
        EventPayload::DocFixRecorded(f) => f.entries.len(),
        EventPayload::CodeReviewRoundRecorded(c) if c.entries.is_empty() => 0,
        EventPayload::CodeReviewRoundRecorded(c) => 1 + c.entries.len(),
        _ => 0,
    }
}

/// Refuse a call that would take a log holding `events` past
/// [`SESSION_CAP`] review entries by recording `adding` more.
pub fn within_cap(events: &[Event], adding: usize) -> Result<(), Refusal> {
    let held: usize = events.iter().map(|e| review_entries(&e.payload)).sum();
    if held + adding > SESSION_CAP {
        return Err(Refusal::new(
            CAP_REACHED,
            format!(
                "the session holds {held} review entries; recording {adding} more would pass \
                 the cap of {SESSION_CAP}"
            ),
        ));
    }
    Ok(())
}

/// A typed refusal: exit 2, nothing appended.
fn refuse(refusal: Refusal) -> ! {
    exit_with_error_code(refusal.body(), 2);
}

/// A plain failure with `code` as the exit status.
fn fail(code: i32, message: String) -> ! {
    exit_with_error_code(
        serde_json::json!({"error": message, "command": COMMAND}),
        code,
    );
}

pub fn handle(backend: &dyn SessionBackend, subcommand: ReviewSubcommand) -> Result<()> {
    match subcommand {
        ReviewSubcommand::Record { name, event } => {
            let Some(name) = name else {
                refuse(Refusal::new(INVALID_VALUE, "the workflow name is missing").field("<NAME>"));
            };
            handle_record(backend, name, event)
        }
    }
}

fn handle_record(backend: &dyn SessionBackend, name: String, event: ReviewEvent) -> Result<()> {
    if !backend.exists(&name) {
        fail(1, format!("workflow '{}' not found", name));
    }
    let (header, events) = match backend.read_events(&name) {
        Ok(r) => r,
        Err(err) => fail(exit_code_for_engine_error(&err), err.to_string()),
    };
    let Some(machine) = derive_machine_state(&header, &events, &backend.session_dir(&name)) else {
        fail(
            EXIT_INFRASTRUCTURE,
            "corrupt state file: cannot derive current state".to_string(),
        );
    };
    let template: CompiledTemplate = match std::fs::read(&machine.template_path)
        .map_err(|e| e.to_string())
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string()))
    {
        Ok(t) => t,
        Err(e) => fail(
            EXIT_INFRASTRUCTURE,
            format!("failed to read the session's template: {e}"),
        ),
    };
    let cancelled = events
        .iter()
        .any(|e| matches!(e.payload, EventPayload::WorkflowCancelled { .. }));
    let terminal = template
        .states
        .get(&machine.current_state)
        .is_some_and(|s| s.terminal);
    if cancelled || terminal {
        refuse(Refusal::new(
            SESSION_TERMINAL,
            format!(
                "workflow '{}' has finished ({}); review events are recorded only on a \
                 session in a non-terminal state",
                name,
                if cancelled {
                    "cancelled".to_string()
                } else {
                    format!("terminal state '{}'", machine.current_state)
                }
            ),
        ));
    }

    let event_name = event.name();
    let state = machine.current_state.as_str();
    let vocabulary = &template.review_vocabulary;
    let payloads: Vec<EventPayload> = match event {
        ReviewEvent::RoundStarted {
            doc_type,
            doc_format,
            schema,
            round,
            seat_count,
            fallback,
        } => vec![EventPayload::ReviewRoundStarted(
            round_started(
                doc_type.as_deref(),
                doc_format.as_deref(),
                schema.as_deref(),
                &round,
                &seat_count,
                fallback,
            )
            .unwrap_or_else(|r| refuse(r)),
        )],
        ReviewEvent::Decision {
            doc_type,
            doc_format,
            schema,
            round,
            decision,
        } => vec![EventPayload::ReviewDecisionRecorded(
            decision_recorded(
                doc_type.as_deref(),
                doc_format.as_deref(),
                schema.as_deref(),
                &round,
                &decision,
            )
            .unwrap_or_else(|r| refuse(r)),
        )],
        ReviewEvent::Verdicts { file, doc_format } => {
            // The flag is checked before the file is read.
            if let Some(f) = doc_format.as_deref() {
                format_flag(f).unwrap_or_else(|r| refuse(r));
            }
            let text = read_batch_file(&file).unwrap_or_else(|r| refuse(r));
            verdict_entries(&text, vocabulary, doc_format.as_deref())
                .unwrap_or_else(|r| refuse(r))
                .into_iter()
                .map(EventPayload::ReviewVerdictRecorded)
                .collect()
        }
        ReviewEvent::RuleChecks {
            doc_format,
            moment,
            file,
        } => {
            format_flag(&doc_format).unwrap_or_else(|r| refuse(r));
            let text = read_batch_file(&file).unwrap_or_else(|r| refuse(r));
            let event = rule_checks_recorded(state, &doc_format, &moment, &text)
                .unwrap_or_else(|r| refuse(r));
            non_empty(EventPayload::DocRuleChecksRecorded(event))
        }
        ReviewEvent::SectionWritten {
            doc_format,
            section,
            write,
        } => vec![EventPayload::DocSectionRecorded(
            section_recorded(state, &doc_format, &section, &write, vocabulary)
                .unwrap_or_else(|r| refuse(r)),
        )],
        ReviewEvent::Fix {
            doc_format,
            answers,
            round,
            file,
        } => {
            format_flag(&doc_format).unwrap_or_else(|r| refuse(r));
            let text = read_batch_file(&file).unwrap_or_else(|r| refuse(r));
            let event = fix_recorded(
                state,
                &doc_format,
                &answers,
                round.as_deref(),
                &text,
                vocabulary,
            )
            .unwrap_or_else(|r| refuse(r));
            non_empty(EventPayload::DocFixRecorded(event))
        }
        ReviewEvent::DocChecks { file } => {
            let text = read_batch_file(&file).unwrap_or_else(|r| refuse(r));
            doc_check_entries(&text)
                .unwrap_or_else(|r| refuse(r))
                .into_iter()
                .map(EventPayload::DocCheckRecorded)
                .collect()
        }
        ReviewEvent::CodeRound { round, head, file } => {
            // The flags are checked before the file is read.
            let flags = code_round_flags(&round, &head);
            let (round, head) = flags.unwrap_or_else(|r| refuse(r));
            let text = read_batch_file(&file).unwrap_or_else(|r| refuse(r));
            let event = code_round_seats(state, round, head, &text, vocabulary)
                .unwrap_or_else(|r| refuse(r));
            non_empty(EventPayload::CodeReviewRoundRecorded(event))
        }
    };
    let count: usize = payloads.iter().map(review_entries).sum();
    within_cap(&events, count).unwrap_or_else(|r| refuse(r));

    // `verdicts` and `doc-checks` append one event per entry, so each
    // journal record gets its own `koto.seq`; `rule-checks`, `fix` and
    // `code-round` append one event holding every entry, so their records
    // share one.
    // Everything was validated above, so only an append failure (exit 1)
    // can stop a batch partway.
    for payload in &payloads {
        if let Err(e) = backend.append_event(&name, payload, &now_iso8601()) {
            fail(1, e.to_string());
        }
    }

    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "status": "recorded",
            "event": event_name,
            "count": count,
        }))?
    );
    Ok(())
}

/// `payload` as the one event a batch call appends, or nothing when its
/// batch was empty: an event with no entries would relay no record.
fn non_empty(payload: EventPayload) -> Vec<EventPayload> {
    if review_entries(&payload) == 0 {
        Vec::new()
    } else {
        vec![payload]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocabulary(seats: &[&str], sections: &[&str]) -> ReviewVocabulary {
        ReviewVocabulary {
            seats: seats.iter().map(|s| s.to_string()).collect(),
            sections: sections.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn verdict_entry(seat: &str, section: &str) -> Value {
        serde_json::json!({
            "rule_id": "rs-001",
            "section": section,
            "verdict": "PASS",
            "seat": seat,
            "round": 1,
            "doc_type": "prd",
            "schema": "prd/v1",
        })
    }

    fn text(entries: &[Value]) -> String {
        serde_json::to_string(entries).unwrap()
    }

    #[test]
    fn closed_sets_accept_their_values_and_nothing_else() {
        for t in DOC_TYPES {
            assert_eq!(doc_type(t), Some(*t));
        }
        for bad in ["", "PRD", "spec", "brief ", "plan/v2"] {
            assert_eq!(doc_type(bad), None, "{bad:?}");
        }
        assert_eq!(verdict("PASS"), Some("pass"));
        assert_eq!(verdict("FAIL"), Some("fail"));
        assert_eq!(verdict("pass"), Some("pass"));
        assert_eq!(verdict("fail"), Some("fail"));
        for bad in ["", "Pass", "ok", "PASSED", "unclear"] {
            assert_eq!(verdict(bad), None, "{bad:?}");
        }
        for d in DECISIONS {
            assert_eq!(decision(d), Some(*d));
        }
        for bad in ["", "RERUN", "retry", "fix-and-proceed"] {
            assert_eq!(decision(bad), None, "{bad:?}");
        }
        for o in CHECK_OUTCOMES {
            assert_eq!(check_outcome(o), Some(*o));
        }
        for bad in ["", "pass", "timed_out", "FAILED"] {
            assert_eq!(check_outcome(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_schema_is_bucketed_into_its_seven_values() {
        for b in SCHEMA_BUCKETS {
            assert_eq!(bucket_schema(Some(b)), *b);
        }
        assert_eq!(bucket_schema(Some(" plan/v2\n")), "plan/v2");
        assert_eq!(bucket_schema(None), "absent");
        assert_eq!(bucket_schema(Some("")), "absent");
        assert_eq!(bucket_schema(Some("   ")), "absent");
        for other in ["plan/v3", "PRD/V1", "design", "a schema an author wrote"] {
            assert_eq!(bucket_schema(Some(other)), "other", "{other:?}");
        }
        let seen: std::collections::BTreeSet<&str> = SCHEMA_BUCKETS
            .iter()
            .copied()
            .chain([SCHEMA_ABSENT, SCHEMA_OTHER])
            .collect();
        assert_eq!(seen.len(), 7);
    }

    #[test]
    fn declared_seats_and_sections_are_kept_and_the_rest_are_other() {
        let v = vocabulary(&["clarity", "scope"], &["goals", "frontmatter"]);
        let entries = text(&[
            verdict_entry("clarity", "goals"),
            verdict_entry("intruder", "frontmatter"),
            verdict_entry("scope", "not-a-section"),
        ]);
        let got = verdict_entries(&entries, &v, None).unwrap();
        let pairs: Vec<(&str, &str)> = got
            .iter()
            .map(|e| (e.seat.as_str(), e.section.as_str()))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("clarity", "goals"),
                ("other", "frontmatter"),
                ("scope", "other")
            ]
        );
    }

    #[test]
    fn a_template_that_declares_nothing_gets_other_for_every_seat_and_section() {
        let entries = text(&[verdict_entry("clarity", "goals")]);
        let got = verdict_entries(&entries, &ReviewVocabulary::default(), None).unwrap();
        assert_eq!(got[0].seat, "other");
        assert_eq!(got[0].section, "other");
    }

    #[test]
    fn a_verdict_entry_is_stored_in_koto_words() {
        let v = vocabulary(&["clarity"], &["goals"]);
        let entries = serde_json::json!([{
            "rule_id": "rs-007",
            "section": "goals",
            "verdict": "FAIL",
            "seat": "clarity",
            "round": 3,
            "doc_type": "design",
        }]);
        let got = verdict_entries(&entries.to_string(), &v, None).unwrap();
        assert_eq!(
            got,
            vec![ReviewVerdictRecorded {
                doc_type: Some("design".into()),
                doc_format: None,
                schema: "absent".into(),
                round: 3,
                seat: "clarity".into(),
                rule_id: "rs-007".into(),
                section: "goals".into(),
                verdict: "fail".into(),
            }]
        );
    }

    #[test]
    fn a_batch_of_two_hundred_is_taken_and_two_hundred_and_one_refused() {
        let v = ReviewVocabulary::default();
        let full: Vec<Value> = (0..BATCH_MAX).map(|_| verdict_entry("a", "b")).collect();
        assert_eq!(verdict_entries(&text(&full), &v, None).unwrap().len(), 200);
        let mut over = full.clone();
        over.push(verdict_entry("a", "b"));
        let err = verdict_entries(&text(&over), &v, None).unwrap_err();
        assert_eq!(err.code, BATCH_TOO_LARGE);

        let check = serde_json::json!({"rule_id": "r1", "outcome": "passed", "doc_type": "plan"});
        let full: Vec<Value> = (0..BATCH_MAX).map(|_| check.clone()).collect();
        assert_eq!(doc_check_entries(&text(&full)).unwrap().len(), 200);
        let mut over = full;
        over.push(check);
        assert_eq!(
            doc_check_entries(&text(&over)).unwrap_err().code,
            BATCH_TOO_LARGE
        );
    }

    #[test]
    fn an_invalid_entry_names_its_index_and_field() {
        let v = ReviewVocabulary::default();
        let mut entries: Vec<Value> = (0..5).map(|_| verdict_entry("a", "b")).collect();
        entries[3]["verdict"] = Value::from("MAYBE");
        let err = verdict_entries(&text(&entries), &v, None).unwrap_err();
        assert_eq!(err.code, INVALID_VALUE);
        assert_eq!(err.entry, Some(3));
        assert_eq!(err.field.as_deref(), Some("verdict"));

        for (field, value) in [
            ("round", Value::from(0)),
            ("round", Value::from(-1)),
            ("round", Value::from(1.5)),
            ("doc_type", Value::from("spec")),
            ("rule_id", Value::from("has space")),
            ("rule_id", Value::from("x".repeat(129))),
            ("rule_id", Value::from("/abs")),
            ("rule_id", Value::from("")),
        ] {
            let mut entries = vec![verdict_entry("a", "b"), verdict_entry("a", "b")];
            entries[1][field] = value.clone();
            let err = verdict_entries(&text(&entries), &v, None).unwrap_err();
            assert_eq!(err.code, INVALID_VALUE, "{field}={value}");
            assert_eq!(err.entry, Some(1), "{field}={value}");
            assert_eq!(err.field.as_deref(), Some(field), "{field}={value}");
        }

        let mut checks =
            vec![serde_json::json!({"rule_id": "r", "outcome": "skipped", "doc_type": "plan"})];
        let err = doc_check_entries(&text(&checks)).unwrap_err();
        assert_eq!(
            (err.code, err.entry, err.field.as_deref()),
            (INVALID_VALUE, Some(0), Some("outcome"))
        );
        checks[0]["outcome"] = Value::from("error");
        assert_eq!(
            doc_check_entries(&text(&checks)).unwrap()[0].outcome,
            "error"
        );
    }

    #[test]
    fn a_rule_id_of_the_longest_length_is_taken() {
        let v = ReviewVocabulary::default();
        let mut entry = verdict_entry("a", "b");
        entry["rule_id"] = Value::from("r".repeat(128));
        assert_eq!(
            verdict_entries(&text(&[entry]), &v, None).unwrap()[0]
                .rule_id
                .len(),
            128
        );
    }

    /// Every way a `--file` can be refused, fed a distinctive string, and
    /// none of the refusals carries it back out.
    #[test]
    fn parse_errors_never_echo_what_they_rejected() {
        const SENTINEL: &str = "SECRET-SENTINEL-7f3a";
        let v = vocabulary(&["a"], &["b"]);
        let mut cases: Vec<String> = vec![
            format!("{{\"{SENTINEL}\": "),
            format!("[{SENTINEL}]"),
            format!("\"{SENTINEL}\""),
            format!("{{\"rows\": [\"{SENTINEL}\"]}}"),
            format!("[\"{SENTINEL}\"]"),
            format!("[[\"{SENTINEL}\"]]"),
        ];
        for (field, value) in [
            ("verdict", Value::from(SENTINEL)),
            ("doc_type", Value::from(SENTINEL)),
            ("rule_id", Value::from(format!("{SENTINEL} with space"))),
            ("round", Value::from(SENTINEL)),
            ("seat", serde_json::json!([SENTINEL])),
            ("section", serde_json::json!({ SENTINEL: 1 })),
            ("schema", serde_json::json!([SENTINEL])),
            (SENTINEL, Value::from(SENTINEL)),
        ] {
            let mut entry = verdict_entry("a", "b");
            entry[field] = value;
            cases.push(text(&[entry]));
        }
        let mut missing = verdict_entry("a", "b");
        missing.as_object_mut().unwrap().remove("seat");
        missing["section"] = Value::from(SENTINEL);
        cases.push(text(&[missing]));

        for case in &cases {
            let err = verdict_entries(case, &v, None).expect_err(case);
            let body = err.body().to_string();
            assert!(!body.contains(SENTINEL), "{case} -> {body}");
            let err = doc_check_entries(case).expect_err(case);
            let body = err.body().to_string();
            assert!(!body.contains(SENTINEL), "{case} -> {body}");
        }

        // An unreadable path: the cause, not the path.
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join(SENTINEL);
        let err = read_batch_file(&missing).unwrap_err();
        assert!(!err.body().to_string().contains(SENTINEL), "{:?}", err);
        // Bytes that aren't UTF-8.
        let raw = dir.path().join("raw.json");
        let mut bytes = SENTINEL.as_bytes().to_vec();
        bytes.extend([0xff, 0xfe]);
        std::fs::write(&raw, bytes).unwrap();
        let err = read_batch_file(&raw).unwrap_err();
        assert!(!err.body().to_string().contains(SENTINEL), "{:?}", err);
        // Too large.
        let big = dir.path().join("big.json");
        std::fs::write(
            &big,
            SENTINEL.repeat((FILE_MAX_BYTES as usize) / SENTINEL.len() + 2),
        )
        .unwrap();
        let err = read_batch_file(&big).unwrap_err();
        assert_eq!(err.code, INVALID_FILE);
        assert!(!err.body().to_string().contains(SENTINEL), "{:?}", err);
    }

    #[test]
    fn flags_are_checked_like_entries() {
        let ok = round_started(Some("brief"), None, Some("brief/v1"), "1", "3", true).unwrap();
        assert_eq!(
            ok,
            ReviewRoundStarted {
                doc_type: Some("brief".into()),
                doc_format: None,
                schema: "brief/v1".into(),
                round: 1,
                seat_count: 3,
                fallback: true,
            }
        );
        for (args, field) in [
            (("memo", "1", "3"), "--doc-type"),
            (("brief", "0", "3"), "--round"),
            (("brief", "-1", "3"), "--round"),
            (("brief", "+1", "3"), "--round"),
            (("brief", "1", "0"), "--seat-count"),
            (("brief", "1", "three"), "--seat-count"),
        ] {
            let err = round_started(Some(args.0), None, None, args.1, args.2, false).unwrap_err();
            assert_eq!(err.code, INVALID_VALUE);
            assert_eq!(err.field.as_deref(), Some(field));
            assert!(!err.message.contains(args.0) || args.0 == "brief");
        }
        let d = decision_recorded(Some("plan"), None, None, "2", "loop_back").unwrap();
        assert_eq!(
            (d.schema.as_str(), d.decision.as_str()),
            ("absent", "loop_back")
        );
        let err = decision_recorded(Some("plan"), None, None, "2", "retry").unwrap_err();
        assert_eq!(err.field.as_deref(), Some("--decision"));
    }

    fn event(payload: EventPayload) -> Event {
        Event {
            seq: 1,
            timestamp: now_iso8601(),
            event_type: payload.type_name().to_string(),
            payload,
            idempotency_hash: None,
        }
    }

    #[test]
    fn the_cap_counts_every_review_event_and_refuses_past_two_thousand() {
        let check = DocCheckRecorded {
            doc_type: "plan".into(),
            schema: "plan/v2".into(),
            rule_id: "r".into(),
            outcome: "passed".into(),
        };
        let round = ReviewRoundStarted {
            doc_type: Some("plan".into()),
            doc_format: None,
            schema: "plan/v2".into(),
            round: 1,
            seat_count: 3,
            fallback: false,
        };
        let mut events: Vec<Event> = (0..SESSION_CAP - 1)
            .map(|_| event(EventPayload::DocCheckRecorded(check.clone())))
            .collect();
        events.push(event(EventPayload::IntentUpdated {
            intent: "not a review event".into(),
        }));
        assert!(within_cap(&events, 1).is_ok());
        assert_eq!(within_cap(&events, 2).unwrap_err().code, CAP_REACHED);
        events.push(event(EventPayload::ReviewRoundStarted(round)));
        assert_eq!(within_cap(&events, 1).unwrap_err().code, CAP_REACHED);
        assert!(within_cap(&events, 0).is_ok());
    }

    /// A `rule-checks` or `fix` event counts one per entry, so ten events of
    /// two hundred entries fill the cap.
    #[test]
    fn the_cap_counts_entries_of_a_batch_event() {
        let checks = rule_checks_recorded(
            "draft",
            "prd",
            "pre_jury",
            &text(&vec![
                serde_json::json!({"rule_id": "r", "outcome": "pass"});
                BATCH_MAX
            ]),
        )
        .unwrap();
        let mut events: Vec<Event> = (0..SESSION_CAP / BATCH_MAX - 1)
            .map(|_| event(EventPayload::DocRuleChecksRecorded(checks.clone())))
            .collect();
        assert!(within_cap(&events, BATCH_MAX).is_ok());
        assert_eq!(
            within_cap(&events, BATCH_MAX + 1).unwrap_err().code,
            CAP_REACHED
        );
        let fix = fix_recorded(
            "draft",
            "prd",
            "check_fail",
            None,
            &text(&[serde_json::json!({"rule_id": "r", "section": "s", "check_rule": "none"})]),
            &ReviewVocabulary::default(),
        )
        .unwrap();
        events.push(event(EventPayload::DocFixRecorded(fix)));
        assert_eq!(
            within_cap(&events, BATCH_MAX).unwrap_err().code,
            CAP_REACHED
        );
        assert!(within_cap(&events, BATCH_MAX - 1).is_ok());
    }

    #[test]
    fn doc_format_names_the_type_for_four_formats_and_none_for_the_charter_three() {
        for f in DOC_FORMATS {
            let (t, fmt) = doc_identity(None, Some(f)).unwrap();
            assert_eq!(fmt.as_deref(), Some(*f));
            let expected = DOC_TYPES.contains(f).then(|| f.to_string());
            assert_eq!(t, expected, "{f}");
        }
        assert_eq!(
            doc_identity(Some("plan"), None).unwrap(),
            (Some("plan".to_string()), None)
        );
        assert_eq!(
            doc_identity(Some("plan"), Some("plan")).unwrap(),
            (Some("plan".to_string()), Some("plan".to_string()))
        );
        for (t, f, field) in [
            (None, Some("memo"), "--doc-format"),
            (None, Some("Vision"), "--doc-format"),
            (Some("plan"), Some("prd"), "--doc-type"),
            (Some("plan"), Some("vision"), "--doc-type"),
            (None, None, "--doc-type"),
        ] {
            let err = doc_identity(t, f).unwrap_err();
            assert_eq!(
                (err.code, err.field.as_deref()),
                (INVALID_VALUE, Some(field))
            );
        }
        let r = round_started(None, Some("vision"), None, "1", "3", false).unwrap();
        assert_eq!(
            (r.doc_type, r.doc_format.as_deref()),
            (None, Some("vision"))
        );
    }

    #[test]
    fn verdicts_under_doc_format_take_or_leave_out_doc_type() {
        let v = ReviewVocabulary::default();
        let mut entry = verdict_entry("a", "b");
        entry.as_object_mut().unwrap().remove("doc_type");
        let got = verdict_entries(&text(&[entry.clone()]), &v, Some("strategy")).unwrap();
        assert_eq!(
            (got[0].doc_type.as_deref(), got[0].doc_format.as_deref()),
            (None, Some("strategy"))
        );
        let got = verdict_entries(&text(&[entry.clone()]), &v, Some("prd")).unwrap();
        assert_eq!(
            (got[0].doc_type.as_deref(), got[0].doc_format.as_deref()),
            (Some("prd"), Some("prd"))
        );
        // Without the flag doc_type stays required.
        let err = verdict_entries(&text(&[entry]), &v, None).unwrap_err();
        assert_eq!(
            (err.code, err.field.as_deref()),
            (INVALID_ENTRY, Some("doc_type"))
        );
        // A doc_type that names another document is refused.
        let err =
            verdict_entries(&text(&[verdict_entry("a", "b")]), &v, Some("roadmap")).unwrap_err();
        assert_eq!(
            (err.code, err.field.as_deref()),
            (INVALID_VALUE, Some("doc_type"))
        );
        let err = verdict_entries(&text(&[verdict_entry("a", "b")]), &v, Some("memo")).unwrap_err();
        assert_eq!(err.field.as_deref(), Some("--doc-format"));
    }

    #[test]
    fn rule_checks_take_their_sets_and_refuse_the_rest() {
        let entry = |outcome: &str| serde_json::json!({"rule_id": "docs/x", "outcome": outcome});
        for m in CHECK_MOMENTS {
            for o in RULE_OUTCOMES {
                let got = rule_checks_recorded("s", "vision", m, &text(&[entry(o)])).unwrap();
                assert_eq!(
                    got,
                    DocRuleChecksRecorded {
                        state: "s".into(),
                        doc_format: "vision".into(),
                        moment: m.to_string(),
                        entries: vec![DocRuleCheck {
                            rule_id: "docs/x".into(),
                            outcome: o.to_string(),
                        }],
                    }
                );
            }
        }
        let err = rule_checks_recorded("s", "prd", "later", &text(&[entry("pass")])).unwrap_err();
        assert_eq!(err.field.as_deref(), Some("--moment"));
        let err =
            rule_checks_recorded("s", "spec", "pre_jury", &text(&[entry("pass")])).unwrap_err();
        assert_eq!(err.field.as_deref(), Some("--doc-format"));
        let err = rule_checks_recorded(
            "s",
            "prd",
            "pre_jury",
            &text(&[entry("pass"), entry("passed")]),
        )
        .unwrap_err();
        assert_eq!(
            (err.code, err.entry, err.field.as_deref()),
            (INVALID_VALUE, Some(1), Some("outcome"))
        );
        let mut extra = entry("pass");
        extra["section"] = Value::from("goals");
        let err = rule_checks_recorded("s", "prd", "pre_jury", &text(&[extra])).unwrap_err();
        assert_eq!(err.code, INVALID_ENTRY);
    }

    #[test]
    fn a_section_write_keeps_a_declared_section_and_others_become_other() {
        let v = vocabulary(&[], &["vision-statement"]);
        let got = section_recorded("s", "vision", "vision-statement", "first", &v).unwrap();
        assert_eq!(
            (got.section.as_str(), got.write.as_str()),
            ("vision-statement", "first")
        );
        let got = section_recorded("s", "vision", "## Some Heading", "rewrite", &v).unwrap();
        assert_eq!(got.section, "other");
        let got = section_recorded(
            "s",
            "vision",
            "vision-statement",
            "rewrite",
            &ReviewVocabulary::default(),
        )
        .unwrap();
        assert_eq!(got.section, "other");
        let err = section_recorded("s", "vision", "x", "append", &v).unwrap_err();
        assert_eq!(err.field.as_deref(), Some("--write"));
    }

    #[test]
    fn a_fix_takes_a_round_only_for_a_jury_fail_from_one_to_fifty() {
        let v = vocabulary(&[], &["goals"]);
        let batch = text(&[
            serde_json::json!({"rule_id": "rs-001", "section": "goals", "check_rule": "docs/required-sections"}),
            serde_json::json!({"rule_id": "rs-002", "section": "Goals text", "check_rule": "none"}),
        ]);
        for r in ["1", "50"] {
            let got = fix_recorded("s", "design", "jury_fail", Some(r), &batch, &v).unwrap();
            assert_eq!(got.round, r.parse().ok());
            assert_eq!(got.entries.len(), 2);
            assert_eq!(got.entries[0].section, "goals");
            assert_eq!(got.entries[1].section, "other");
            assert_eq!(got.entries[1].check_rule, "none");
        }
        assert_eq!(
            fix_recorded("s", "design", "jury_fail", None, &batch, &v)
                .unwrap()
                .round,
            None
        );
        for r in ["0", "51", "1.5", "two", "-1", ""] {
            let err = fix_recorded("s", "design", "jury_fail", Some(r), &batch, &v).unwrap_err();
            assert_eq!(err.field.as_deref(), Some("--round"), "{r:?}");
        }
        let got = fix_recorded("s", "design", "check_fail", None, &batch, &v).unwrap();
        assert_eq!(got.round, None);
        let err = fix_recorded("s", "design", "check_fail", Some("2"), &batch, &v).unwrap_err();
        assert_eq!(err.field.as_deref(), Some("--round"));
        let err = fix_recorded("s", "design", "jury_pass", None, &batch, &v).unwrap_err();
        assert_eq!(err.field.as_deref(), Some("--answers"));
    }

    #[test]
    fn a_fix_check_rule_is_bound_like_a_rule_id() {
        let v = ReviewVocabulary::default();
        let digest = crate::run_journal::digest("a rule id outside the name shape");
        for ok in ["none", "docs/required-sections", digest.as_str()] {
            let batch =
                text(&[serde_json::json!({"rule_id": ok, "section": "s", "check_rule": ok})]);
            assert!(fix_recorded("s", "plan", "check_fail", None, &batch, &v).is_ok());
        }
        for (field, bad) in [
            ("check_rule", "has space".to_string()),
            ("check_rule", "x".repeat(129)),
            ("check_rule", "/abs".to_string()),
            ("check_rule", String::new()),
            ("rule_id", "~home".to_string()),
        ] {
            let mut good =
                serde_json::json!({"rule_id": "r", "section": "s", "check_rule": "none"});
            let mut bad_entry = good.clone();
            bad_entry[field] = Value::from(bad.clone());
            good["rule_id"] = Value::from("r2");
            let err = fix_recorded(
                "s",
                "plan",
                "check_fail",
                None,
                &text(&[good, bad_entry]),
                &v,
            )
            .unwrap_err();
            assert_eq!(
                (err.code, err.entry, err.field.as_deref()),
                (INVALID_VALUE, Some(1), Some(field)),
                "{field}={bad}"
            );
            assert!(!err.body().to_string().contains("has space"));
        }
    }

    // ----- code-round -----

    fn seat(seat: &str, model: &str, verdict: &str) -> Value {
        serde_json::json!({"seat": seat, "model": model, "verdict": verdict})
    }

    #[test]
    fn a_code_round_keeps_its_seats_in_order_and_an_undeclared_seat_is_other() {
        let v = vocabulary(&["correctness", "security"], &[]);
        let batch = text(&[
            seat("correctness", "claude-opus-4-5", "pass"),
            seat("security", "gpt-5.1", "FAIL"),
            seat("intruder", "claude-sonnet-4-5@20250929", "PASS"),
        ]);
        let got = code_round_recorded("review", "2", "0123abcd", &batch, &v).unwrap();
        assert_eq!(
            got,
            CodeReviewRoundRecorded {
                state: "review".into(),
                round: 2,
                head: crate::run_journal::digest("0123abcd"),
                entries: vec![
                    CodeReviewSeat {
                        seat: "correctness".into(),
                        model: "claude-opus-4-5".into(),
                        verdict: "pass".into(),
                    },
                    CodeReviewSeat {
                        seat: "security".into(),
                        model: "gpt-5.1".into(),
                        verdict: "fail".into(),
                    },
                    CodeReviewSeat {
                        seat: "other".into(),
                        model: "claude-sonnet-4-5@20250929".into(),
                        verdict: "pass".into(),
                    },
                ],
            }
        );
        // A template that declares no seats gets `other` for every one.
        let got =
            code_round_recorded("review", "1", "h", &batch, &ReviewVocabulary::default()).unwrap();
        assert!(got.entries.iter().all(|e| e.seat == "other"));
    }

    #[test]
    fn a_code_round_head_is_stored_only_as_its_digest() {
        let head = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
        let (round, digest) = code_round_flags("1", head).unwrap();
        assert_eq!(round, 1);
        assert_eq!(digest, crate::run_journal::digest(head));
        assert!(digest.starts_with("sha256:"));
        assert!(!digest.contains(head));
        // The same head gives the same digest; another head another.
        assert_eq!(code_round_flags("3", head).unwrap().1, digest);
        assert_ne!(code_round_flags("1", "other-head").unwrap().1, digest);
        // A value that is already a digest is digested again.
        assert_ne!(code_round_flags("1", &digest).unwrap().1, digest);
        // The longest head is taken.
        assert!(code_round_flags("1", &"h".repeat(HEAD_MAX_BYTES)).is_ok());
    }

    #[test]
    fn a_code_round_refuses_bad_flags_and_entries_and_names_them() {
        let v = vocabulary(&["correctness"], &[]);
        let ok = text(&[seat("correctness", "m", "pass")]);
        for (round, head, field) in [
            ("0", "h", "--round"),
            ("-1", "h", "--round"),
            ("two", "h", "--round"),
            ("", "h", "--round"),
            ("1", "", "--head"),
        ] {
            let err = code_round_recorded("s", round, head, &ok, &v).unwrap_err();
            assert_eq!(
                (err.code, err.field.as_deref()),
                (INVALID_VALUE, Some(field)),
                "{round:?} {head:?}"
            );
        }
        let long = "h".repeat(HEAD_MAX_BYTES + 1);
        let err = code_round_recorded("s", "1", &long, &ok, &v).unwrap_err();
        assert_eq!(err.field.as_deref(), Some("--head"));
        assert!(!err.body().to_string().contains(&long));

        let good = seat("correctness", "m", "pass");
        let mut missing_seat = good.clone();
        missing_seat.as_object_mut().unwrap().remove("seat");
        let mut missing_model = good.clone();
        missing_model.as_object_mut().unwrap().remove("model");
        let mut extra = good.clone();
        extra["rule_id"] = Value::from("rs-001");
        for (bad, code, field) in [
            (missing_seat, INVALID_ENTRY, Some("seat")),
            (missing_model, INVALID_ENTRY, Some("model")),
            (extra, INVALID_ENTRY, None),
            (
                seat("correctness", "m", "maybe"),
                INVALID_VALUE,
                Some("verdict"),
            ),
            (
                seat("correctness", "m", "Pass"),
                INVALID_VALUE,
                Some("verdict"),
            ),
            (
                seat("correctness", "", "pass"),
                INVALID_VALUE,
                Some("model"),
            ),
            (
                serde_json::json!({"seat": "correctness", "model": 7, "verdict": "pass"}),
                INVALID_ENTRY,
                Some("model"),
            ),
            (
                serde_json::json!({"seat": 1, "model": "m", "verdict": "pass"}),
                INVALID_ENTRY,
                Some("seat"),
            ),
        ] {
            let batch = text(&[good.clone(), bad.clone()]);
            let err = code_round_recorded("s", "1", "h", &batch, &v).unwrap_err();
            assert_eq!(
                (err.code, err.entry, err.field.as_deref()),
                (code, Some(1), field),
                "{bad}"
            );
        }
        // The longest model is taken.
        let batch = text(&[seat("correctness", &"m".repeat(128), "pass")]);
        let got = code_round_recorded("s", "1", "h", &batch, &v).unwrap();
        assert_eq!(got.entries[0].model, "m".repeat(128));
    }

    /// A model outside the journal's name shape is recorded as `other`, as
    /// an undeclared seat is, and the rest of the round is kept.
    #[test]
    fn a_model_that_isnt_name_shaped_is_recorded_as_other() {
        let v = vocabulary(&["correctness", "security"], &[]);
        let batch = text(&[
            seat("correctness", "claude-opus-4-1[1m]", "pass"),
            seat("security", "claude-opus-4-5", "fail"),
        ]);
        let got = code_round_recorded("s", "1", "h", &batch, &v).unwrap();
        let models: Vec<(&str, &str, &str)> = got
            .entries
            .iter()
            .map(|e| (e.seat.as_str(), e.model.as_str(), e.verdict.as_str()))
            .collect();
        assert_eq!(
            models,
            vec![
                ("correctness", "other", "pass"),
                ("security", "claude-opus-4-5", "fail"),
            ]
        );
        let too_long = "m".repeat(129);
        for odd in [
            "claude-opus-4-1[1m]",
            "has space",
            "/abs",
            "~home",
            too_long.as_str(),
        ] {
            assert_eq!(code_model(odd), "other", "{odd:?}");
        }
        for kept in [
            "claude-opus-4-5",
            "gpt-5.1",
            "claude-sonnet-4-5@20250929",
            "org/model:v2",
        ] {
            assert_eq!(code_model(kept), kept);
        }
    }

    #[test]
    fn a_code_round_of_two_hundred_seats_is_taken_and_two_hundred_and_one_refused() {
        let v = ReviewVocabulary::default();
        let full: Vec<Value> = (0..BATCH_MAX).map(|_| seat("a", "m", "pass")).collect();
        let got = code_round_recorded("s", "1", "h", &text(&full), &v).unwrap();
        assert_eq!(got.entries.len(), BATCH_MAX);
        let mut over = full;
        over.push(seat("a", "m", "pass"));
        let err = code_round_recorded("s", "1", "h", &text(&over), &v).unwrap_err();
        assert_eq!(err.code, BATCH_TOO_LARGE);
    }

    #[test]
    fn code_round_errors_never_echo_what_they_rejected() {
        const SENTINEL: &str = "SECRET-SENTINEL-7f3a";
        let v = vocabulary(&["a"], &[]);
        let mut cases: Vec<String> =
            vec![format!("{{\"{SENTINEL}\": "), format!("[\"{SENTINEL}\"]")];
        for (field, value) in [
            ("verdict", Value::from(SENTINEL)),
            ("model", serde_json::json!([SENTINEL])),
            ("seat", serde_json::json!([SENTINEL])),
            (SENTINEL, Value::from(SENTINEL)),
        ] {
            let mut entry = seat("a", "m", "pass");
            entry[field] = value;
            cases.push(text(&[entry]));
        }
        for case in &cases {
            let err = code_round_recorded("s", "1", "h", case, &v).expect_err(case);
            assert!(!err.body().to_string().contains(SENTINEL), "{case}");
        }
        let err = code_round_recorded("s", SENTINEL, "h", "[]", &v).unwrap_err();
        assert!(!err.body().to_string().contains(SENTINEL));
    }

    /// A code round counts one for the round and one per seat against the
    /// session cap, and an empty one counts nothing.
    #[test]
    fn a_code_round_counts_its_round_and_each_seat_against_the_cap() {
        let v = ReviewVocabulary::default();
        let three = code_round_recorded(
            "s",
            "1",
            "h",
            &text(&[
                seat("a", "m", "pass"),
                seat("b", "m", "fail"),
                seat("c", "m", "pass"),
            ]),
            &v,
        )
        .unwrap();
        assert_eq!(
            review_entries(&EventPayload::CodeReviewRoundRecorded(three.clone())),
            4
        );
        let empty = code_round_recorded("s", "1", "h", "[]", &v).unwrap();
        let empty = EventPayload::CodeReviewRoundRecorded(empty);
        assert_eq!(review_entries(&empty), 0);
        assert!(non_empty(empty).is_empty());

        let check = DocCheckRecorded {
            doc_type: "plan".into(),
            schema: "plan/v2".into(),
            rule_id: "r".into(),
            outcome: "passed".into(),
        };
        let mut events: Vec<Event> = (0..SESSION_CAP - 4)
            .map(|_| event(EventPayload::DocCheckRecorded(check.clone())))
            .collect();
        assert!(within_cap(&events, 4).is_ok());
        events.push(event(EventPayload::CodeReviewRoundRecorded(three)));
        assert!(within_cap(&events, 0).is_ok());
        assert_eq!(within_cap(&events, 1).unwrap_err().code, CAP_REACHED);
    }
}
