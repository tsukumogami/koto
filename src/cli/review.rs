//! `koto review record`: record document review events as telemetry.
//!
//! A caller running a document review (a jury of reviewer seats over a
//! brief, PRD, design or plan) records what happened through four events:
//! a round starting, each seat's verdict per rule and section, the decision
//! that follows the round, and publish-time document check results. Each
//! appends typed session events, and the run journal relays each event as
//! one telemetry record (see `crate::run_journal`).
//!
//! Like `koto disputes record`, the events are recorded, never obeyed:
//! nothing the advance loop routes on reads them, so no gate outcome, route
//! or state changes, and the verb works on a session in any non-terminal
//! state.
//!
//! Every value is checked before anything is appended, and only koto's own
//! words for it are stored: the document type, verdict, decision and check
//! outcome come from closed sets; the schema is bucketed into a closed set
//! plus `other`; a seat or section is stored as given only when the
//! session's template declares it (`review_vocabulary`), and as `other`
//! otherwise; a rule id must have the journal's name shape; a round and a
//! seat count are positive integers. One invalid entry refuses the whole
//! batch, so a refused call appends nothing. A batch holds at most
//! [`BATCH_MAX`] entries, and a session holds at most [`SESSION_CAP`]
//! recorded review events; a call that would pass the cap appends nothing.
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
    now_iso8601, DocCheckRecorded, Event, EventPayload, ReviewDecisionRecorded, ReviewRoundStarted,
    ReviewVerdictRecorded,
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

/// The most entries one `verdicts` or `doc-checks` call records.
pub const BATCH_MAX: usize = 200;
/// The most review events one session holds, all four kinds counted.
pub const SESSION_CAP: usize = 2000;
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

/// The keys of a `verdicts` entry; `schema` may be absent or null.
const VERDICT_KEYS: &[&str] = &[
    "rule_id", "section", "verdict", "seat", "round", "doc_type", "schema",
];
/// The keys of a `doc-checks` entry; `schema` may be absent or null.
const DOC_CHECK_KEYS: &[&str] = &["rule_id", "outcome", "doc_type", "schema"];

#[derive(Subcommand)]
pub enum ReviewSubcommand {
    /// Record a document review event as telemetry. Changes no gate
    /// outcome, route or state
    Record {
        /// Workflow name
        name: String,

        #[command(subcommand)]
        event: ReviewEvent,
    },
}

/// The four review events.
#[derive(Subcommand)]
pub enum ReviewEvent {
    /// A review round's seats were spawned
    RoundStarted {
        /// The document type: brief, prd, design or plan
        #[arg(long = "doc-type")]
        doc_type: String,
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
    /// optional schema, at most 200
    Verdicts {
        /// The JSON file holding the batch
        #[arg(long)]
        file: PathBuf,
    },
    /// What follows a review round
    Decision {
        /// The document type: brief, prd, design or plan
        #[arg(long = "doc-type")]
        doc_type: String,
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
}

impl ReviewEvent {
    /// The subcommand's name, as the success body echoes it.
    fn name(&self) -> &'static str {
        match self {
            ReviewEvent::RoundStarted { .. } => "round-started",
            ReviewEvent::Verdicts { .. } => "verdicts",
            ReviewEvent::Decision { .. } => "decision",
            ReviewEvent::DocChecks { .. } => "doc-checks",
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

/// The `round-started` event, from its flags.
pub fn round_started(
    doc_type_flag: &str,
    schema: Option<&str>,
    round: &str,
    seat_count: &str,
    fallback: bool,
) -> Result<ReviewRoundStarted, Refusal> {
    Ok(ReviewRoundStarted {
        doc_type: doc_type(doc_type_flag)
            .ok_or_else(|| bad_value("--doc-type", &one_of(DOC_TYPES)))?
            .to_string(),
        schema: bucket_schema(schema).to_string(),
        round: positive(round).ok_or_else(|| bad_value("--round", "a positive integer"))?,
        seat_count: positive(seat_count)
            .ok_or_else(|| bad_value("--seat-count", "a positive integer"))?,
        fallback,
    })
}

/// The `decision` event, from its flags.
pub fn decision_recorded(
    doc_type_flag: &str,
    schema: Option<&str>,
    round: &str,
    decision_flag: &str,
) -> Result<ReviewDecisionRecorded, Refusal> {
    Ok(ReviewDecisionRecorded {
        doc_type: doc_type(doc_type_flag)
            .ok_or_else(|| bad_value("--doc-type", &one_of(DOC_TYPES)))?
            .to_string(),
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

fn entry_rule_id(index: usize, map: &Map<String, Value>) -> Result<String, Refusal> {
    let value = string_field(index, map, "rule_id")?;
    if valid_rule_id(value) {
        Ok(value.to_string())
    } else {
        Err(bad_entry_value(
            index,
            "rule_id",
            "1 to 128 characters from A-Z a-z 0-9 . _ : / @ -, not starting with / or ~",
        ))
    }
}

/// The verdict events of a `verdicts` batch, one per entry, in order. A seat
/// or section `vocabulary` doesn't declare is stored as `other`.
pub fn verdict_entries(
    text: &str,
    vocabulary: &ReviewVocabulary,
) -> Result<Vec<ReviewVerdictRecorded>, Refusal> {
    let entries = batch_entries(text)?;
    let mut out = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let map = entry_object(index, entry, VERDICT_KEYS)?;
        let doc_type = entry_doc_type(index, map)?;
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

/// Whether `payload` is one of the four review events, the ones
/// [`SESSION_CAP`] counts.
pub fn is_review_event(payload: &EventPayload) -> bool {
    matches!(
        payload,
        EventPayload::ReviewRoundStarted(_)
            | EventPayload::ReviewVerdictRecorded(_)
            | EventPayload::ReviewDecisionRecorded(_)
            | EventPayload::DocCheckRecorded(_)
    )
}

/// Refuse a call that would take a log holding `events` past
/// [`SESSION_CAP`] review events by appending `adding` more.
pub fn within_cap(events: &[Event], adding: usize) -> Result<(), Refusal> {
    let held = events
        .iter()
        .filter(|e| is_review_event(&e.payload))
        .count();
    if held + adding > SESSION_CAP {
        return Err(Refusal::new(
            CAP_REACHED,
            format!(
                "the session holds {held} review events; recording {adding} more would pass \
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
        ReviewSubcommand::Record { name, event } => handle_record(backend, name, event),
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
    let payloads: Vec<EventPayload> = match event {
        ReviewEvent::RoundStarted {
            doc_type,
            schema,
            round,
            seat_count,
            fallback,
        } => vec![EventPayload::ReviewRoundStarted(
            round_started(&doc_type, schema.as_deref(), &round, &seat_count, fallback)
                .unwrap_or_else(|r| refuse(r)),
        )],
        ReviewEvent::Decision {
            doc_type,
            schema,
            round,
            decision,
        } => vec![EventPayload::ReviewDecisionRecorded(
            decision_recorded(&doc_type, schema.as_deref(), &round, &decision)
                .unwrap_or_else(|r| refuse(r)),
        )],
        ReviewEvent::Verdicts { file } => {
            let text = read_batch_file(&file).unwrap_or_else(|r| refuse(r));
            verdict_entries(&text, &template.review_vocabulary)
                .unwrap_or_else(|r| refuse(r))
                .into_iter()
                .map(EventPayload::ReviewVerdictRecorded)
                .collect()
        }
        ReviewEvent::DocChecks { file } => {
            let text = read_batch_file(&file).unwrap_or_else(|r| refuse(r));
            doc_check_entries(&text)
                .unwrap_or_else(|r| refuse(r))
                .into_iter()
                .map(EventPayload::DocCheckRecorded)
                .collect()
        }
    };
    within_cap(&events, payloads.len()).unwrap_or_else(|r| refuse(r));

    // One event per entry, so each journal record gets its own `koto.seq`.
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
            "count": payloads.len(),
        }))?
    );
    Ok(())
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
        let got = verdict_entries(&entries, &v).unwrap();
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
        let got = verdict_entries(&entries, &ReviewVocabulary::default()).unwrap();
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
        let got = verdict_entries(&entries.to_string(), &v).unwrap();
        assert_eq!(
            got,
            vec![ReviewVerdictRecorded {
                doc_type: "design".into(),
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
        assert_eq!(verdict_entries(&text(&full), &v).unwrap().len(), 200);
        let mut over = full.clone();
        over.push(verdict_entry("a", "b"));
        let err = verdict_entries(&text(&over), &v).unwrap_err();
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
        let err = verdict_entries(&text(&entries), &v).unwrap_err();
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
            let err = verdict_entries(&text(&entries), &v).unwrap_err();
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
            verdict_entries(&text(&[entry]), &v).unwrap()[0]
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
            let err = verdict_entries(case, &v).expect_err(case);
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
        let ok = round_started("brief", Some("brief/v1"), "1", "3", true).unwrap();
        assert_eq!(
            ok,
            ReviewRoundStarted {
                doc_type: "brief".into(),
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
            let err = round_started(args.0, None, args.1, args.2, false).unwrap_err();
            assert_eq!(err.code, INVALID_VALUE);
            assert_eq!(err.field.as_deref(), Some(field));
            assert!(!err.message.contains(args.0) || args.0 == "brief");
        }
        let d = decision_recorded("plan", None, "2", "loop_back").unwrap();
        assert_eq!(
            (d.schema.as_str(), d.decision.as_str()),
            ("absent", "loop_back")
        );
        let err = decision_recorded("plan", None, "2", "retry").unwrap_err();
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
            doc_type: "plan".into(),
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
}
