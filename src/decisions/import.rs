//! `koto decisions import <session> <file>` (PRD R28): decisions recorded
//! somewhere else, read from JSON Lines and recorded in the importing
//! session.
//!
//! Each line is one JSON object:
//!
//! - `source`, `source_id`: non-empty strings; the pair identifies the
//!   entry, so importing it again adds nothing.
//! - `time`: RFC 3339 UTC (`...Z`), kept as the decision's time in koto's
//!   own millisecond form; a time in the future is refused.
//! - `kind`, `outcome`: R1's closed sets; `refused` isn't imported.
//! - `subject`: an object of strings, as R2 shapes it for the kind; `paths`
//!   may be an array of strings. A `reverse` names its original as
//!   `{"source": .., "source_id": ..}`, which must be another line of the
//!   same file.
//! - `decider`: `{"kind": "person", "handle": ..}`,
//!   `{"kind": "session", "session": ..}` or `{"kind": "check", "name": ..}`.
//! - `authority`: `{"kind": "dispatch" | "person", "level"?: .., "grants"?: [..]}`.
//! - optional `reason` (a string) and `evidence` (an array of pointers, as
//!   `--evidence` takes them; a `verdict` needs one).
//!
//! Any other field is refused. A file holds at most [`MAX_LINES`] lines of
//! at most [`MAX_LINE_BYTES`] bytes each.
//!
//! Every line is parsed and checked with the same validation a local record
//! gets ([`body::validate`]) before anything is written; the first failure
//! stops the import with its line number and records nothing. The bodies
//! are then written under one hold of the session's manifest lock
//! ([`store::write_batch`]), where lines already in the session are
//! skipped and one already there with different content is refused.
//!
//! Every imported decider and authority value is labelled `handed`; an
//! imported decision never claims to be `verified`. An imported `reverse`
//! is recorded in the importing session, aimed at its own original, with
//! standing `asserted` and an empty path.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, Read};

use serde_json::Value;

use super::body::{self, Compensating, DecisionBody, Imported, Request, Target};
use super::decider::{Authority, Decider, Entering, Source, Sourced};
use super::standing::{self, reasons, ASSERTED};
use super::store::{self, Area, Listing};
use super::{chain, codes, format_id, DecisionError, FORMAT_VERSION};
use crate::session::Backend;

/// The most lines one file may hold.
pub const MAX_LINES: usize = 10_000;

/// The longest line, in bytes, without its line ending.
pub const MAX_LINE_BYTES: usize = 16 * 1024;

/// The longest `source`, `source_id`, handle, check name, decider session,
/// authority level or grant, in bytes.
pub const MAX_NAME_BYTES: usize = 256;

/// The reason a `reverse` that names anything but another line of the same
/// file is refused with.
pub const NOT_IN_IMPORT: &str = "not_in_import";

/// The fields a line may carry.
const FIELDS: [&str; 10] = [
    "source",
    "source_id",
    "time",
    "kind",
    "outcome",
    "subject",
    "decider",
    "authority",
    "reason",
    "evidence",
];

/// An import's refusal: the error, the line it names, and for a refused
/// `reverse` the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportError {
    pub error: DecisionError,
    pub line: Option<usize>,
    pub reason: Option<&'static str>,
}

impl ImportError {
    fn at(line: usize, error: DecisionError) -> Self {
        Self {
            error,
            line: Some(line),
            reason: None,
        }
    }

    fn invalid(line: usize, field: &str, message: impl Into<String>) -> Self {
        Self::at(line, DecisionError::invalid(field, message))
    }

    fn reversal(line: usize, reason: &'static str, message: impl Into<String>) -> Self {
        let mut error = DecisionError::new(codes::REVERSAL_REFUSED, message);
        error.field = Some("subject".to_string());
        Self {
            error,
            line: Some(line),
            reason: Some(reason),
        }
    }
}

impl From<DecisionError> for ImportError {
    fn from(error: DecisionError) -> Self {
        Self {
            error,
            line: None,
            reason: None,
        }
    }
}

/// The decider a line names, before it is placed in the importing session.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LineDecider {
    Person(String),
    Session(String),
    Check(String),
}

/// One line, parsed and checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The line number in the file, from 1.
    pub line: usize,
    pub source: String,
    pub source_id: String,
    /// In koto's stored form, `YYYY-MM-DDTHH:MM:SS.mmmZ`.
    pub time: String,
    pub kind: String,
    pub outcome: String,
    /// The subject as stored, for every kind but `reverse`.
    subject: BTreeMap<String, Value>,
    /// The `(source, source_id)` a `reverse` names.
    original: Option<(String, String)>,
    decider: LineDecider,
    authority_kind: String,
    level: Option<String>,
    grants: Vec<String>,
    evidence: Vec<Value>,
    reason: Option<String>,
}

impl Entry {
    fn pair(&self) -> (String, String) {
        (self.source.clone(), self.source_id.clone())
    }
}

/// Parse and check every line `reader` holds, for an import into
/// `session`, refusing a time later than `now` (a stored-form time). The
/// first failure is returned with its line number.
pub fn parse(mut reader: impl BufRead, now: &str) -> Result<Vec<Entry>, ImportError> {
    let mut entries = Vec::new();
    let mut buf = Vec::new();
    let mut number = 0usize;
    loop {
        buf.clear();
        let read = reader
            .by_ref()
            .take(MAX_LINE_BYTES as u64 + 3)
            .read_until(b'\n', &mut buf)
            .map_err(|e| {
                ImportError::from(DecisionError::invalid(
                    "file",
                    format!("the file can't be read: {e}"),
                ))
            })?;
        if read == 0 {
            break;
        }
        number += 1;
        if number > MAX_LINES {
            return Err(ImportError::invalid(
                number,
                "file",
                format!("the file holds more than {MAX_LINES} lines"),
            ));
        }
        let mut line: &[u8] = &buf;
        if let Some(rest) = line.strip_suffix(b"\n") {
            line = rest;
        }
        if let Some(rest) = line.strip_suffix(b"\r") {
            line = rest;
        }
        if line.len() > MAX_LINE_BYTES {
            return Err(ImportError::invalid(
                number,
                "line",
                format!("the line is longer than {MAX_LINE_BYTES} bytes"),
            ));
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        entries.push(parse_line(number, line, now)?);
    }
    check_file(&entries)?;
    Ok(entries)
}

/// The checks that span lines: a pair given twice, and a `reverse` whose
/// original isn't another non-reverse line of the file or is reversed by
/// more than one line.
fn check_file(entries: &[Entry]) -> Result<(), ImportError> {
    let mut lines: BTreeMap<(String, String), &Entry> = BTreeMap::new();
    for e in entries {
        if let Some(first) = lines.insert(e.pair(), e) {
            return Err(ImportError::invalid(
                e.line,
                "source_id",
                format!(
                    "source '{}' and source_id '{}' are already given on line {}",
                    preview(&e.source),
                    preview(&e.source_id),
                    first.line
                ),
            ));
        }
    }
    let mut reversed: BTreeMap<(String, String), usize> = BTreeMap::new();
    for e in entries {
        let Some(original) = &e.original else {
            continue;
        };
        match lines.get(original) {
            None => {
                return Err(ImportError::reversal(
                    e.line,
                    NOT_IN_IMPORT,
                    format!(
                        "the reversal names source '{}' and source_id '{}', which no other \
                         line of this file holds; an imported reversal names only an entry \
                         from the same file",
                        preview(&original.0),
                        preview(&original.1)
                    ),
                ))
            }
            Some(o) if o.kind == "reverse" => {
                return Err(ImportError::reversal(
                    e.line,
                    reasons::REVERSES_REVERSAL,
                    format!(
                        "the reversal names line {}, which is a reversal, and a reversal \
                         isn't reversed",
                        o.line
                    ),
                ))
            }
            Some(_) => {}
        }
        if let Some(first) = reversed.insert(original.clone(), e.line) {
            return Err(ImportError::reversal(
                e.line,
                reasons::ALREADY_REVERSED,
                format!("line {first} already reverses the same entry"),
            ));
        }
    }
    Ok(())
}

/// Parse and check one line.
fn parse_line(n: usize, line: &[u8], now: &str) -> Result<Entry, ImportError> {
    let value: Value = serde_json::from_slice(line)
        .map_err(|e| ImportError::invalid(n, "line", format!("the line isn't JSON: {e}")))?;
    let Value::Object(obj) = value else {
        return Err(ImportError::invalid(
            n,
            "line",
            "the line isn't a JSON object",
        ));
    };
    if let Some(unknown) = obj.keys().find(|k| !FIELDS.contains(&k.as_str())) {
        return Err(ImportError::invalid(
            n,
            unknown,
            format!(
                "'{}' isn't a field an imported entry takes",
                preview(unknown)
            ),
        ));
    }
    let string = |field: &str| -> Result<String, ImportError> {
        match obj.get(field) {
            Some(Value::String(s)) => Ok(s.clone()),
            Some(_) => Err(ImportError::invalid(
                n,
                field,
                format!("'{field}' is a string"),
            )),
            None => Err(ImportError::invalid(
                n,
                field,
                format!("'{field}' is required"),
            )),
        }
    };

    let source = name(n, "source", string("source")?)?;
    let source_id = name(n, "source_id", string("source_id")?)?;
    let time = time(n, &string("time")?, now)?;
    let kind = string("kind")?;
    let outcome = string("outcome")?;
    if kind == "refused" {
        return Err(ImportError::invalid(
            n,
            "kind",
            "a 'refused' decision isn't imported",
        ));
    }
    let reason = match obj.get("reason") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(ImportError::invalid(n, "reason", "'reason' is a string")),
    };
    let raw_evidence: Vec<String> = match obj.get("evidence") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| ImportError::invalid(n, "evidence", "'evidence' holds strings"))
            })
            .collect::<Result<_, _>>()?,
        Some(_) => {
            return Err(ImportError::invalid(
                n,
                "evidence",
                "'evidence' is an array of strings",
            ))
        }
    };
    let subject_obj = match obj.get("subject") {
        Some(Value::Object(s)) => s,
        Some(_) => return Err(ImportError::invalid(n, "subject", "'subject' is an object")),
        None => return Err(ImportError::invalid(n, "subject", "'subject' is required")),
    };

    // A reverse names its original by source and source_id; R2's form for
    // it, a session and an id, is built once the original's id is known.
    let mut original = None;
    let subject_pairs = if kind == "reverse" {
        if subject_obj.contains_key("session") || subject_obj.contains_key("id") {
            return Err(ImportError::reversal(
                n,
                NOT_IN_IMPORT,
                "an imported reversal names its original by source and source_id from the \
                 same file, never another session's decision",
            ));
        }
        let field = |f: &str| match subject_obj.get(f) {
            Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
            _ => Err(ImportError::invalid(
                n,
                &format!("subject.{f}"),
                format!("an imported reversal's subject needs '{f}', a non-empty string"),
            )),
        };
        let pair = (field("source")?, field("source_id")?);
        if let Some(extra) = subject_obj
            .keys()
            .find(|k| *k != "source" && *k != "source_id")
        {
            return Err(ImportError::invalid(
                n,
                &format!("subject.{}", preview(extra)),
                "an imported reversal's subject is source and source_id only",
            ));
        }
        original = Some(pair);
        vec![
            ("session".to_string(), "imported".to_string()),
            ("id".to_string(), format_id(1)),
        ]
    } else {
        let mut pairs = Vec::new();
        for (field, value) in subject_obj {
            match (field.as_str(), value) {
                ("paths", Value::Array(paths)) => {
                    for p in paths {
                        let p = p.as_str().ok_or_else(|| {
                            ImportError::invalid(n, "subject.paths", "'paths' holds strings")
                        })?;
                        pairs.push(("path".to_string(), p.to_string()));
                    }
                }
                (_, Value::String(v)) => pairs.push((field.clone(), v.clone())),
                _ => {
                    return Err(ImportError::invalid(
                        n,
                        &format!("subject.{}", preview(field)),
                        format!("subject field '{}' is a string", preview(field)),
                    ))
                }
            }
        }
        pairs
    };
    let req = Request {
        kind: kind.clone(),
        outcome: outcome.clone(),
        subject: subject_pairs,
        basis: Vec::new(),
        evidence: raw_evidence,
        reason: reason.clone(),
        tool_call: None,
    };
    let subject = body::validate(&req).map_err(|e| ImportError::at(n, e))?;
    let evidence = body::parse_evidence(&req.evidence).map_err(|e| ImportError::at(n, e))?;

    let decider = decider(n, obj.get("decider"))?;
    let (authority_kind, level, grants) = authority(n, obj.get("authority"))?;
    Ok(Entry {
        line: n,
        source,
        source_id,
        time,
        kind,
        outcome,
        subject: if original.is_some() {
            BTreeMap::new()
        } else {
            subject
        },
        original,
        decider,
        authority_kind,
        level,
        grants,
        evidence,
        reason,
    })
}

/// A name a line gives: non-empty, at most [`MAX_NAME_BYTES`], no control
/// characters.
fn name(n: usize, field: &str, value: String) -> Result<String, ImportError> {
    if value.trim().is_empty()
        || value.len() > MAX_NAME_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(ImportError::invalid(
            n,
            field,
            format!("'{field}' must be 1 to {MAX_NAME_BYTES} bytes with no control characters"),
        ));
    }
    Ok(value)
}

/// A line's `time`, in koto's stored form; refused when it isn't RFC 3339
/// UTC or is later than `now`.
fn time(n: usize, value: &str, now: &str) -> Result<String, ImportError> {
    let parsed = value
        .ends_with('Z')
        .then(|| crate::engine::types::parse_timestamp_to_systemtime(value))
        .flatten()
        .ok_or_else(|| {
            ImportError::invalid(
                n,
                "time",
                format!(
                    "'{}' isn't an RFC 3339 UTC time such as 2026-11-01T09:30:00Z",
                    preview(value)
                ),
            )
        })?;
    let stored = crate::engine::types::iso8601_at(parsed);
    if stored.as_str() > now {
        return Err(ImportError::invalid(
            n,
            "time",
            format!("'{}' is in the future", preview(value)),
        ));
    }
    Ok(stored)
}

/// A line's decider.
fn decider(n: usize, value: Option<&Value>) -> Result<LineDecider, ImportError> {
    let obj = match value {
        Some(Value::Object(o)) => o,
        Some(_) => return Err(ImportError::invalid(n, "decider", "'decider' is an object")),
        None => return Err(ImportError::invalid(n, "decider", "'decider' is required")),
    };
    let kind = obj.get("kind").and_then(Value::as_str).unwrap_or_default();
    let field = match kind {
        "person" => "handle",
        "session" => "session",
        "check" => "name",
        _ => {
            return Err(ImportError::invalid(
                n,
                "decider.kind",
                "'decider.kind' is person, session or check",
            ))
        }
    };
    if let Some(extra) = obj.keys().find(|k| *k != "kind" && *k != field) {
        return Err(ImportError::invalid(
            n,
            &format!("decider.{}", preview(extra)),
            format!("a {kind} decider takes 'kind' and '{field}' only"),
        ));
    }
    let path = format!("decider.{field}");
    let value = match obj.get(field) {
        Some(Value::String(s)) => name(n, &path, s.clone())?,
        _ => {
            return Err(ImportError::invalid(
                n,
                &path,
                format!("a {kind} decider needs '{field}', a string"),
            ))
        }
    };
    Ok(match kind {
        "person" => LineDecider::Person(value),
        "session" => LineDecider::Session(value),
        _ => LineDecider::Check(value),
    })
}

/// A line's authority: its kind, level and grants.
fn authority(
    n: usize,
    value: Option<&Value>,
) -> Result<(String, Option<String>, Vec<String>), ImportError> {
    let obj = match value {
        Some(Value::Object(o)) => o,
        Some(_) => {
            return Err(ImportError::invalid(
                n,
                "authority",
                "'authority' is an object",
            ))
        }
        None => {
            return Err(ImportError::invalid(
                n,
                "authority",
                "'authority' is required",
            ))
        }
    };
    if let Some(extra) = obj
        .keys()
        .find(|k| !["kind", "level", "grants"].contains(&k.as_str()))
    {
        return Err(ImportError::invalid(
            n,
            &format!("authority.{}", preview(extra)),
            "'authority' takes kind, level and grants only",
        ));
    }
    let kind = match obj.get("kind").and_then(Value::as_str) {
        Some(k @ ("dispatch" | "person")) => k.to_string(),
        _ => {
            return Err(ImportError::invalid(
                n,
                "authority.kind",
                "'authority.kind' is dispatch or person",
            ))
        }
    };
    let level = match obj.get("level") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(name(n, "authority.level", s.clone())?),
        Some(_) => {
            return Err(ImportError::invalid(
                n,
                "authority.level",
                "'authority.level' is a string",
            ))
        }
    };
    let grants: Vec<String> = match obj.get("grants") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) if items.len() <= body::MAX_LIST_ENTRIES => items
            .iter()
            .map(|g| match g {
                Value::String(s) => name(n, "authority.grants", s.clone()),
                _ => Err(ImportError::invalid(
                    n,
                    "authority.grants",
                    "'authority.grants' holds strings",
                )),
            })
            .collect::<Result<_, _>>()?,
        Some(_) => {
            return Err(ImportError::invalid(
                n,
                "authority.grants",
                format!(
                    "'authority.grants' is an array of at most {} strings",
                    body::MAX_LIST_ENTRIES
                ),
            ))
        }
    };
    Ok((kind, level, grants))
}

/// What an import did: each line recorded, with its id, and each line
/// skipped because the session already held it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    pub imported: Vec<(usize, String)>,
    pub skipped: Vec<(usize, String)>,
}

/// Record `entries` in `session`, which the caller has checked is a session
/// in the store that hasn't moved or been superseded.
pub fn import(backend: &Backend, session: &str, entries: &[Entry]) -> Result<Outcome, ImportError> {
    // A reversal entered through this session may have lost since (R19).
    standing::settle(backend, session);
    let stamp = standing::stamp(backend, session)?;
    let chain = chain::snapshot(backend, session);
    let dispatcher = chain::dispatcher(backend, session).unwrap_or_else(|| "none".to_string());
    let imported_at = crate::engine::types::now_iso8601();
    let ctx = Context {
        session,
        stamp: &stamp,
        chain: &chain,
        dispatcher: &dispatcher,
        imported_at: &imported_at,
    };

    // Lines that aren't reversals go first, so a stop partway through never
    // leaves a reversal whose original wasn't written.
    let mut order: Vec<&Entry> = entries.iter().filter(|e| e.original.is_none()).collect();
    order.extend(entries.iter().filter(|e| e.original.is_some()));

    let mut failed: Option<(Option<usize>, Option<&'static str>)> = None;
    let written = store::write_batch(backend, session, |listing, first| {
        plan(&ctx, &order, listing, first).map_err(|refusal| {
            failed = Some((refusal.line, refusal.reason));
            refusal.error
        })
    });
    match written {
        Ok((_, outcome)) => Ok(outcome),
        Err(error) => Err(ImportError {
            error,
            line: failed.and_then(|(line, _)| line),
            reason: failed.and_then(|(_, reason)| reason),
        }),
    }
}

/// What every imported body takes from the importing session.
struct Context<'a> {
    session: &'a str,
    stamp: &'a standing::Stamp,
    chain: &'a chain::ChainSnapshot,
    dispatcher: &'a str,
    imported_at: &'a str,
}

/// Inside the session's lock: skip the lines it already holds, refuse one
/// it holds with different content, and build the rest with ids from
/// `first` on.
fn plan(
    ctx: &Context<'_>,
    order: &[&Entry],
    listing: &Listing,
    first: u64,
) -> Result<(Vec<(Area, DecisionBody)>, Outcome), ImportError> {
    let held: Vec<DecisionBody> = listing
        .found
        .iter()
        .filter_map(|f| f.bytes.as_ref().ok())
        .filter_map(|b| DecisionBody::parse(b).ok())
        .collect();
    let by_pair: BTreeMap<(String, String), &DecisionBody> = held
        .iter()
        .filter_map(|b| {
            b.imported
                .as_ref()
                .map(|i| ((i.source.clone(), i.source_id.clone()), b))
        })
        .collect();
    // The session's own decisions some reversal held here is aimed at.
    let reversed_here: BTreeSet<&str> = held
        .iter()
        .filter(|b| b.kind == "reverse")
        .filter_map(|b| b.target.as_ref())
        .filter(|t| t.id == ctx.stamp.session.id)
        .map(|t| t.decision.as_str())
        .collect();

    let mut ids: BTreeMap<(String, String), String> = BTreeMap::new();
    let mut kinds: BTreeMap<(String, String), String> = BTreeMap::new();
    let mut next = first;
    let mut out = Vec::new();
    let mut outcome = Outcome::default();
    for e in order {
        let pair = e.pair();
        let original = match &e.original {
            None => None,
            Some(o) => {
                let id = ids.get(o).cloned().ok_or_else(|| {
                    ImportError::reversal(
                        e.line,
                        NOT_IN_IMPORT,
                        "the reversal's original wasn't found",
                    )
                })?;
                Some((id, kinds.get(o).cloned().unwrap_or_default()))
            }
        };
        if let Some(existing) = by_pair.get(&pair) {
            let candidate = build(ctx, e, &existing.id, original.as_ref())
                .map_err(|x| ImportError::at(e.line, x))?;
            if !same_content(existing, &candidate) {
                return Err(ImportError::invalid(
                    e.line,
                    "source_id",
                    format!(
                        "source '{}' and source_id '{}' are already imported in '{}' \
                         as decision {} with different content",
                        preview(&e.source),
                        preview(&e.source_id),
                        ctx.session,
                        existing.id
                    ),
                ));
            }
            ids.insert(pair.clone(), existing.id.clone());
            kinds.insert(pair, existing.kind.clone());
            outcome.skipped.push((e.line, existing.id.clone()));
            continue;
        }
        if let Some((original_id, _)) = &original {
            if reversed_here.contains(original_id.as_str()) {
                return Err(ImportError::reversal(
                    e.line,
                    reasons::ALREADY_REVERSED,
                    format!(
                        "decision {original_id} in '{}' is already reversed",
                        ctx.session
                    ),
                ));
            }
        }
        let id = format_id(next);
        next += 1;
        let body = build(ctx, e, &id, original.as_ref()).map_err(|x| ImportError::at(e.line, x))?;
        let area = if original.is_some() {
            Area::Aimed(ctx.stamp.session.id.clone())
        } else {
            Area::Decisions
        };
        ids.insert(pair.clone(), id.clone());
        kinds.insert(pair, e.kind.clone());
        outcome.imported.push((e.line, id));
        out.push((area, body));
    }
    outcome.imported.sort();
    outcome.skipped.sort();
    Ok((out, outcome))
}

/// Whether a decision the session already holds for a pair records what
/// the line would: everything the line sets, and nothing the importer
/// stamps.
fn same_content(held: &DecisionBody, line: &DecisionBody) -> bool {
    held.time == line.time
        && held.kind == line.kind
        && held.outcome == line.outcome
        && held.subject == line.subject
        && held.decider == line.decider
        && held.authority.kind == line.authority.kind
        && held.authority.level == line.authority.level
        && held.authority.grants == line.authority.grants
        && held.evidence == line.evidence
        && held.reason == line.reason
        && held.target.as_ref().map(|t| &t.decision) == line.target.as_ref().map(|t| &t.decision)
}

/// The body `e` is recorded as under `id`; `original` is a reversal's
/// original id and kind.
fn build(
    ctx: &Context<'_>,
    e: &Entry,
    id: &str,
    original: Option<&(String, String)>,
) -> Result<DecisionBody, DecisionError> {
    let session = ctx.session;
    let decider = match &e.decider {
        LineDecider::Person(handle) => Decider::Person {
            handle: Sourced::handed(handle.clone()),
            surface: "relay".to_string(),
            link: None,
            entering: Entering {
                session: Sourced::handed(session),
                harness: Sourced::handed(""),
                anchor: Sourced::handed(""),
            },
        },
        LineDecider::Session(name) => Decider::Session {
            session: Sourced::handed(name.clone()),
            harness: Sourced::handed(""),
            anchor: Sourced::handed(""),
            agent: Sourced::handed("unknown"),
            defaulted: false,
            unestablished: false,
            note: None,
        },
        LineDecider::Check(name) => Decider::Check {
            name: Sourced::handed(name.clone()),
            session: Sourced::handed(session),
        },
    };
    let authority = Authority {
        kind: e.authority_kind.clone(),
        level: e.level.clone(),
        grants: e.grants.clone(),
        source: Source::Handed,
        dispatcher: ctx.dispatcher.to_string(),
        run_mode: Sourced::handed("unknown"),
    };
    let (subject, target, standing, path, compensating) = match original {
        None => (e.subject.clone(), None, None, None, None),
        Some((original_id, original_kind)) => {
            let subject = body::validate(&Request {
                kind: "reverse".to_string(),
                outcome: "reversed".to_string(),
                subject: vec![
                    ("session".to_string(), session.to_string()),
                    ("id".to_string(), original_id.clone()),
                ],
                ..Request::default()
            })?;
            let compensating = ["merge", "verdict", "judgment"]
                .contains(&original_kind.as_str())
                .then_some(Compensating { link: None });
            (
                subject,
                Some(Target {
                    name: session.to_string(),
                    id: ctx.stamp.session.id.clone(),
                    decision: original_id.clone(),
                }),
                Some(ASSERTED.to_string()),
                Some(Vec::new()),
                compensating,
            )
        }
    };
    Ok(DecisionBody {
        v: FORMAT_VERSION,
        id: id.to_string(),
        session: ctx.stamp.session.clone(),
        time: e.time.clone(),
        state: ctx.stamp.state.clone(),
        log_seq: ctx.stamp.log_seq,
        kind: e.kind.clone(),
        outcome: e.outcome.clone(),
        subject,
        decider,
        authority,
        chain: ctx.chain.clone(),
        basis: Vec::new(),
        evidence: e.evidence.clone(),
        reason: e.reason.clone(),
        tool_call: None,
        target,
        standing,
        path,
        compensating,
        replacement: None,
        refusal: None,
        imported: Some(Imported {
            source: e.source.clone(),
            source_id: e.source_id.clone(),
            imported_at: ctx.imported_at.to_string(),
        }),
    })
}

/// A short, printable preview of file input for an error message.
fn preview(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(64).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: &str = "2026-11-13T12:00:00.000Z";

    fn line(over: &[(&str, Value)]) -> String {
        let mut v = serde_json::json!({
            "source": "interim",
            "source_id": "e-1",
            "time": "2026-10-02T09:30:00Z",
            "kind": "judgment",
            "outcome": "settled",
            "subject": {"what": "use sqlite"},
            "decider": {"kind": "person", "handle": "dana"},
            "authority": {"kind": "person"},
        });
        for (k, val) in over {
            if val.is_null() {
                v.as_object_mut().unwrap().remove(*k);
            } else {
                v[*k] = val.clone();
            }
        }
        v.to_string()
    }

    fn parse_str(text: &str) -> Result<Vec<Entry>, ImportError> {
        parse(text.as_bytes(), NOW)
    }

    #[test]
    fn a_line_parses_with_its_time_in_the_stored_form() {
        let entries = parse_str(&line(&[])).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].time, "2026-10-02T09:30:00.000Z");
        assert_eq!(entries[0].subject["what"], "use sqlite");
        assert_eq!(entries[0].decider, LineDecider::Person("dana".to_string()));
    }

    #[test]
    fn blank_lines_are_skipped_but_counted() {
        let text = format!(
            "\n{}\n\n{}\n",
            line(&[]),
            line(&[("source_id", "e-2".into())])
        );
        let entries = parse_str(&text).unwrap();
        assert_eq!(
            entries.iter().map(|e| e.line).collect::<Vec<_>>(),
            vec![2, 4]
        );
    }

    #[test]
    fn each_refusal_names_its_field_and_line() {
        let cases: Vec<(String, &str)> = vec![
            (line(&[("kind", "nope".into())]), "kind"),
            (line(&[("outcome", "merged".into())]), "outcome"),
            (
                line(&[("subject", serde_json::json!({"gate": "g"}))]),
                "subject.what",
            ),
            (
                line(&[("kind", "refused".into()), ("outcome", "refused".into())]),
                "kind",
            ),
            (
                line(&[("time", "2026-10-02T09:30:00+02:00".into())]),
                "time",
            ),
            (line(&[("time", "2099-01-01T00:00:00Z".into())]), "time"),
            (line(&[("source", "".into())]), "source"),
            (line(&[("source_id", Value::Null)]), "source_id"),
            (line(&[("extra", "x".into())]), "extra"),
            (
                line(&[("decider", serde_json::json!({"kind": "robot"}))]),
                "decider.kind",
            ),
            (
                line(&[(
                    "decider",
                    serde_json::json!({"kind": "person", "handle": "d", "x": 1}),
                )]),
                "decider.x",
            ),
            (
                line(&[("authority", serde_json::json!({"kind": "god"}))]),
                "authority.kind",
            ),
            (
                line(&[("kind", "verdict".into()), ("outcome", "pass".into())]),
                "evidence",
            ),
            ("{not json".to_string(), "line"),
        ];
        for (text, field) in cases {
            let good = line(&[("source_id", "ok".into())]);
            let err = parse_str(&format!("{good}\n{text}\n")).unwrap_err();
            assert_eq!(err.line, Some(2), "{text}: {err:?}");
            assert_eq!(err.error.field.as_deref(), Some(field), "{text}: {err:?}");
            assert_eq!(err.error.code, codes::INVALID_DECISION, "{text}");
        }
    }

    #[test]
    fn a_pair_given_twice_is_refused() {
        let err = parse_str(&format!("{}\n{}\n", line(&[]), line(&[]))).unwrap_err();
        assert_eq!(err.line, Some(2));
        assert_eq!(err.error.field.as_deref(), Some("source_id"));
    }

    #[test]
    fn a_reverse_names_only_another_line_of_the_file() {
        let reverse = |subject: Value| {
            line(&[
                ("source_id", "r-1".into()),
                ("kind", "reverse".into()),
                ("outcome", "reversed".into()),
                ("subject", subject),
            ])
        };
        let ok = format!(
            "{}\n{}\n",
            line(&[]),
            reverse(serde_json::json!({"source": "interim", "source_id": "e-1"}))
        );
        let entries = parse_str(&ok).unwrap();
        assert_eq!(
            entries[1].original,
            Some(("interim".to_string(), "e-1".to_string()))
        );

        for (subject, reason) in [
            (
                serde_json::json!({"source": "interim", "source_id": "e-9"}),
                NOT_IN_IMPORT,
            ),
            (
                serde_json::json!({"session": "other", "id": "000000000001"}),
                NOT_IN_IMPORT,
            ),
            (
                serde_json::json!({"source": "interim", "source_id": "r-1"}),
                reasons::REVERSES_REVERSAL,
            ),
        ] {
            let text = format!("{}\n{}\n", line(&[]), reverse(subject.clone()));
            let err = parse_str(&text).unwrap_err();
            assert_eq!(err.error.code, codes::REVERSAL_REFUSED, "{subject}");
            assert_eq!(err.reason, Some(reason), "{subject}");
            assert_eq!(err.line, Some(2), "{subject}");
        }
    }

    #[test]
    fn line_length_and_count_are_capped() {
        let long = line(&[("reason", "a".repeat(MAX_LINE_BYTES).into())]);
        let err = parse_str(&long).unwrap_err();
        assert_eq!(err.error.field.as_deref(), Some("line"));
        assert_eq!(err.line, Some(1));

        let many = "\n".repeat(MAX_LINES + 1);
        let err = parse_str(&many).unwrap_err();
        assert_eq!(err.error.field.as_deref(), Some("file"));
        assert_eq!(err.line, Some(MAX_LINES + 1));
    }
}
