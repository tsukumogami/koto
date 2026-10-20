//! `koto decisions import`: bringing decisions recorded before the store
//! existed onto the record (R24).
//!
//! The first source format is a coordinator's decision table, exported as
//! strict versioned JSON:
//!
//! ```json
//! {"v": 1, "format": "coordinator-decisions",
//!  "source": "owner/repo record #12", "coordinator": "<session id>",
//!  "entries": [{"entry": "7", "time": "2026-10-01T12:00Z",
//!               "kind": "judgment", "question": "Postgres or SQLite?",
//!               "outcome": "settled", "decided_by": "coordinator",
//!               "reason": "..."}]}
//! ```
//!
//! An entry's identity is its source location plus its identifier there.
//! The import ledger (`imports.jsonl`) holds one line per imported entry,
//! written under the store lock before the decision, so importing the same
//! entry again is a lookup and adds nothing, and a reversal racing an
//! import sees either the one imported decision or none. A ledger line
//! whose decision never landed (the append after it failed) doesn't count:
//! the next import writes the entry again under a new line.
//!
//! Everything taken from a source is recorded as `handed`: the decider, the
//! authority's values and any chain the source records. With no chain, the
//! chain is `incomplete`. An imported decision gets a new identifier, keeps
//! the source's time in `time` (and its identifier names that month), and
//! records when it was imported in `recorded_at`. Because its time is the
//! source's, an imported `gate-pass` dated before a reversal of the same
//! gate never clears that reversal: only a pass recorded after it does.
//!
//! The source is opened without following a symlink, must be a regular
//! file of at most [`MAX_SOURCE_BYTES`], and is parsed and checked in full
//! before the store lock is taken. A `reverse` or `refused` entry is
//! refused: a reversal is recorded with `koto decisions reverse`, where the
//! authority rules apply.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::decider::{is_identifier, is_name, parse_by, HandedDecider, SURFACES};
use super::record::{
    Authority, Chain, DecidedBy, DeciderKind, Decision, FieldError, Kind, Sourced, Subject,
};
use super::{
    new_id, normalize_time, now_to_second, writer, Check, Store, StoreError, WriteOutcome,
    DECISION_FORMAT_VERSION, IMPORTS_FILE,
};

/// The largest import source koto reads, in bytes.
pub const MAX_SOURCE_BYTES: u64 = 4 * 1024 * 1024;
/// The most entries one source may hold.
pub const MAX_ENTRIES: usize = 10_000;
/// The import format version koto reads.
pub const IMPORT_FORMAT_VERSION: u32 = 1;
/// The one source format koto reads today.
pub const COORDINATOR_FORMAT: &str = "coordinator-decisions";
/// What `decided_by` names to mean the source's coordinator session.
pub const COORDINATOR: &str = "coordinator";

/// One import source, as read.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub v: u32,
    pub format: String,
    /// Where the entries were recorded (a record, a file, a URL), stored as
    /// given in each decision's `import_source`.
    pub source: String,
    /// The coordinator's session, for entries decided by `coordinator`.
    #[serde(default)]
    pub coordinator: Option<String>,
    pub entries: Vec<Entry>,
}

/// One source entry.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// The entry's identifier in its source.
    pub entry: String,
    /// When the source recorded it (UTC; minutes are enough).
    pub time: String,
    pub kind: String,
    #[serde(default)]
    pub outcome: Option<String>,
    /// The question an entry settled or escalated; a judgment's subject by
    /// default.
    #[serde(default)]
    pub question: Option<String>,
    /// Who an escalation went to.
    #[serde(default)]
    pub recipient: Option<String>,
    /// The subject, for kinds the question doesn't describe.
    #[serde(default)]
    pub subject: Option<Subject>,
    /// `coordinator`, `session:<id>`, `person:<handle>` or `check:<name>`.
    pub decided_by: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub surface: Option<String>,
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub run_mode: Option<String>,
    #[serde(default)]
    pub dispatch_id: Option<String>,
    #[serde(default)]
    pub instructions_digest: Option<String>,
    #[serde(default)]
    pub dispatched_by: Option<String>,
    /// The dispatch chain the source recorded, nearest link first, each as
    /// `session:<id>`, `person:<handle>` or `none`.
    #[serde(default)]
    pub chain: Option<Vec<String>>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub basis: Vec<String>,
}

/// One line of the import ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerLine {
    pub v: u32,
    pub source: String,
    pub entry: String,
    pub decision: String,
    pub recorded_at: String,
}

/// What happened to one entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntryOutcome {
    pub entry: String,
    pub id: String,
}

/// What an import did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Outcome {
    pub source: String,
    /// Entries imported by this run.
    pub imported: Vec<EntryOutcome>,
    /// Entries an earlier import already brought in, with their decision.
    pub already_imported: Vec<EntryOutcome>,
}

/// Why an import was refused before anything was written.
#[derive(Debug)]
pub enum ImportError {
    /// The source couldn't be opened or read safely.
    Source(String),
    /// The source or one of its entries is malformed.
    Invalid(FieldError),
    /// The store refused a write.
    Store(StoreError),
}

impl From<FieldError> for ImportError {
    fn from(e: FieldError) -> Self {
        ImportError::Invalid(e)
    }
}

impl From<StoreError> for ImportError {
    fn from(e: StoreError) -> Self {
        ImportError::Store(e)
    }
}

/// Read a source file: no symlink followed, a regular file, at most
/// [`MAX_SOURCE_BYTES`].
#[cfg(unix)]
pub fn read_source(path: &Path) -> Result<Source, ImportError> {
    use std::os::unix::fs::OpenOptionsExt;
    let shown = path.display();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| ImportError::Source(format!("opening {}: {}", shown, e)))?;
    let meta = file
        .metadata()
        .map_err(|e| ImportError::Source(format!("reading {}: {}", shown, e)))?;
    if !meta.is_file() {
        return Err(ImportError::Source(format!(
            "{} is not a regular file",
            shown
        )));
    }
    if meta.len() > MAX_SOURCE_BYTES {
        return Err(ImportError::Source(format!(
            "{} is {} bytes, over the import maximum of {}",
            shown,
            meta.len(),
            MAX_SOURCE_BYTES
        )));
    }
    let mut buf = Vec::new();
    file.take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| ImportError::Source(format!("reading {}: {}", shown, e)))?;
    if buf.len() as u64 > MAX_SOURCE_BYTES {
        return Err(ImportError::Source(format!(
            "{} is over the import maximum of {} bytes",
            shown, MAX_SOURCE_BYTES
        )));
    }
    serde_json::from_slice(&buf).map_err(|e| {
        ImportError::Invalid(FieldError::new(
            "source",
            format!("{} is not an import source koto reads: {}", shown, e),
        ))
    })
}

#[cfg(not(unix))]
pub fn read_source(_path: &Path) -> Result<Source, ImportError> {
    Err(ImportError::Store(StoreError::Unsupported))
}

/// A source time to the second: `YYYY-MM-DDTHH:MMZ` gains `:00`; anything
/// [`normalize_time`] takes is accepted as it normalizes.
fn source_time(raw: &str) -> Option<String> {
    let padded;
    let t = match raw.strip_suffix('Z') {
        Some(body) if body.len() == 16 && body.as_bytes().get(10) == Some(&b'T') => {
            padded = format!("{}:00Z", body);
            padded.as_str()
        }
        _ => raw,
    };
    // A bare date or month isn't a time a source records.
    if !t.contains('T') {
        return None;
    }
    normalize_time(t)
}

fn entry_field(entry: &str, field: &str, message: impl Into<String>) -> FieldError {
    FieldError::new(field, format!("entry '{}': {}", entry, message.into()))
}

/// The decider an entry names, every value labelled `handed`.
fn decided_by(src: &Source, e: &Entry) -> Result<DecidedBy, FieldError> {
    let bad = |m: String| entry_field(&e.entry, "decided_by", m);
    let handed = |v: &str| vec![Sourced::handed(v)];
    let agent = match &e.agent {
        Some(a) if is_identifier(a) => handed(a),
        Some(a) => {
            return Err(entry_field(
                &e.entry,
                "agent",
                format!("'{}' is not an agent identifier", a),
            ))
        }
        None => Vec::new(),
    };
    let by = if e.decided_by == COORDINATOR {
        match &src.coordinator {
            Some(c) if is_identifier(c) => HandedDecider::Session(c.clone()),
            Some(c) => {
                return Err(FieldError::new(
                    "coordinator",
                    format!("'{}' is not a session identifier", c),
                ))
            }
            None => {
                return Err(bad(
                    "names the coordinator, but the source names no 'coordinator' session"
                        .to_string(),
                ))
            }
        }
    } else {
        parse_by(&e.decided_by).map_err(|fe| bad(fe.message))?
    };
    Ok(match by {
        HandedDecider::Session(id) => DecidedBy::session(handed(&id), agent, false),
        HandedDecider::ObservedSession => {
            return Err(bad(
                "an imported decision names its session (session:<id>)".to_string()
            ))
        }
        HandedDecider::Person(handle) => {
            let surface = e.surface.clone().unwrap_or_else(|| "relay".to_string());
            if !SURFACES.contains(&surface.as_str()) {
                return Err(entry_field(
                    &e.entry,
                    "surface",
                    format!("'{}' is not a surface ({})", surface, SURFACES.join(", ")),
                ));
            }
            DecidedBy::person(handed(&handle), surface, e.link.clone(), Vec::new())
        }
        HandedDecider::Check(name) => DecidedBy::check(name, Vec::new()),
    })
}

fn chain_link(entry: &str, v: &str) -> Result<(), FieldError> {
    let ok = v == "none"
        || v.strip_prefix("session:").is_some_and(is_identifier)
        || v.strip_prefix("person:").is_some_and(is_name);
    if ok {
        Ok(())
    } else {
        Err(entry_field(
            entry,
            "chain",
            format!(
                "'{}' is not a chain link (session:<id>, person:<handle> or none)",
                v
            ),
        ))
    }
}

/// The subject an entry describes: its own `subject`, or one built from
/// its question for the kinds a coordinator's table records.
fn subject_of(src: &Source, e: &Entry, kind: Kind) -> Subject {
    if let Some(s) = &e.subject {
        return s.clone();
    }
    match kind {
        Kind::Judgment | Kind::Verdict => Subject {
            what: e.question.clone(),
            pointer: Some(format!("{}#{}", src.source, e.entry)),
            ..Default::default()
        },
        Kind::Escalate => Subject {
            question_id: Some(e.entry.clone()),
            question: e.question.clone(),
            recipient: e.recipient.clone(),
            ..Default::default()
        },
        Kind::DeclineEscalation => Subject {
            question_id: Some(e.entry.clone()),
            question: e.question.clone(),
            ..Default::default()
        },
        _ => Subject::default(),
    }
}

/// Build the decision one entry becomes, checked the way the writer will.
pub fn decision_for(src: &Source, e: &Entry, recorded_at: &str) -> Result<Decision, FieldError> {
    if e.entry.trim().is_empty() || e.entry.chars().any(char::is_control) {
        return Err(FieldError::new("entry", "an entry needs an identifier"));
    }
    let kind = Kind::parse(&e.kind).ok_or_else(|| {
        entry_field(
            &e.entry,
            "kind",
            format!("'{}' is not a decision kind", e.kind),
        )
    })?;
    if matches!(kind, Kind::Reverse | Kind::Refused) {
        return Err(entry_field(
            &e.entry,
            "kind",
            format!(
                "a '{}' entry can't be imported; record a reversal with `koto decisions reverse`",
                kind
            ),
        ));
    }
    let outcome = match (&e.outcome, kind.outcomes()) {
        (Some(o), _) => o.clone(),
        (None, [only]) => only.to_string(),
        (None, _) => {
            return Err(entry_field(
                &e.entry,
                "outcome",
                format!("kind '{}' needs an outcome", kind),
            ))
        }
    };
    let time = source_time(&e.time)
        .ok_or_else(|| entry_field(&e.entry, "time", format!("'{}' is not a UTC time", e.time)))?;
    let decided_by = decided_by(src, e)?;
    let mut authority = Authority::for_decider(decided_by.kind);
    let handed = |o: &Option<String>| o.iter().map(Sourced::handed).collect::<Vec<_>>();
    if let Some(m) = &e.run_mode {
        super::decider::validate_run_mode(m)
            .map_err(|fe| entry_field(&e.entry, "run_mode", fe.message))?;
    }
    if let Some(v) = &e.dispatch_id {
        super::lineage::validate_dispatch_id(v)
            .map_err(|fe| entry_field(&e.entry, "dispatch_id", fe.message))?;
    }
    if let Some(v) = &e.instructions_digest {
        super::lineage::validate_digest(v)
            .map_err(|fe| entry_field(&e.entry, "instructions_digest", fe.message))?;
    }
    if let Some(v) = &e.dispatched_by {
        super::lineage::validate_dispatched_by(v)
            .map_err(|fe| entry_field(&e.entry, "dispatched_by", fe.message))?;
    }
    authority.run_mode = handed(&e.run_mode);
    authority.dispatch_id = handed(&e.dispatch_id);
    authority.instructions_digest = handed(&e.instructions_digest);
    authority.dispatcher = handed(&e.dispatched_by);
    let chain = match &e.chain {
        Some(links) => {
            for l in links {
                chain_link(&e.entry, l)?;
            }
            let reaches_root = links
                .last()
                .is_some_and(|l| l == "none" || l.starts_with("person:"));
            Chain {
                links: links
                    .iter()
                    .filter(|l| l.as_str() != "none")
                    .map(|l| vec![Sourced::handed(l.clone())])
                    .collect(),
                incomplete: !reaches_root,
            }
        }
        None => Chain::unknown(),
    };
    if decided_by.kind == DeciderKind::Person && e.agent.is_some() {
        return Err(entry_field(
            &e.entry,
            "agent",
            "a person decider names no agent",
        ));
    }
    let d = Decision {
        v: DECISION_FORMAT_VERSION,
        id: new_id(&time)?,
        time,
        kind,
        outcome,
        subject: subject_of(src, e, kind),
        decided_by,
        authority,
        chain,
        basis: e.basis.clone(),
        evidence: e.evidence.clone(),
        reason: e.reason.clone(),
        reverses: None,
        original: None,
        repaired_from: None,
        import_source: Some(format!("{}#{}", src.source, e.entry)),
        recorded_at: Some(recorded_at.to_string()),
        compensating: None,
        replacement: None,
    };
    writer::assemble_line(&d).map_err(|fe| entry_field(&e.entry, &fe.field.clone(), fe.message))?;
    Ok(d)
}

/// Check a whole source, building every decision before anything is
/// written.
pub fn prepare(src: &Source) -> Result<Vec<(String, Decision)>, FieldError> {
    if src.v != IMPORT_FORMAT_VERSION {
        return Err(FieldError::new(
            "v",
            format!("unknown import format version {}", src.v),
        ));
    }
    if src.format != COORDINATOR_FORMAT {
        return Err(FieldError::new(
            "format",
            format!(
                "'{}' is not an import format koto reads (only {})",
                src.format, COORDINATOR_FORMAT
            ),
        ));
    }
    if src.source.trim().is_empty() || src.source.chars().any(char::is_control) {
        return Err(FieldError::new(
            "source",
            "the source must name where its entries were recorded",
        ));
    }
    if src.entries.len() > MAX_ENTRIES {
        return Err(FieldError::new(
            "entries",
            format!(
                "{} entries exceed the maximum of {}",
                src.entries.len(),
                MAX_ENTRIES
            ),
        ));
    }
    let recorded_at = now_to_second();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(src.entries.len());
    for e in &src.entries {
        if !seen.insert(e.entry.as_str()) {
            return Err(entry_field(
                &e.entry,
                "entry",
                "appears twice in the source",
            ));
        }
        out.push((e.entry.clone(), decision_for(src, e, &recorded_at)?));
    }
    Ok(out)
}

/// The ledger's entries, by (source, entry). Unreadable lines are skipped.
fn read_ledger(root: &Path) -> Result<HashMap<(String, String), String>, StoreError> {
    let path = root.join(IMPORTS_FILE);
    let content = match std::fs::read(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(e) => return Err(StoreError::io(format!("reading {}", path.display()), e)),
    };
    Ok(content
        .split(|b| *b == b'\n')
        .filter_map(|l| serde_json::from_slice::<LedgerLine>(l).ok())
        .map(|l| ((l.source, l.entry), l.decision))
        .collect())
}

fn append_ledger(root: &Path, line: &LedgerLine) -> Result<(), StoreError> {
    use std::io::Write;
    let path = root.join(IMPORTS_FILE);
    let mut file = writer::open_store_file(&path)?;
    writer::repair_torn_tail(&mut file, &path)?;
    let mut bytes = serde_json::to_vec(line).map_err(|e| {
        StoreError::io(
            "serializing the import ledger line",
            std::io::Error::other(e),
        )
    })?;
    bytes.push(b'\n');
    file.write_all(&bytes)
        .and_then(|_| file.sync_data())
        .map_err(|e| StoreError::io(format!("appending to {}", path.display()), e))
}

/// Import every entry of `src` into `store`: each entry already in the
/// ledger is reported with the decision it became; each new one gets its
/// ledger line and then its decision, under the store lock.
pub fn import(store: &Store, src: &Source) -> Result<Outcome, ImportError> {
    let prepared = prepare(src)?;
    let mut out = Outcome {
        source: src.source.clone(),
        ..Default::default()
    };
    for (entry, decision) in prepared {
        let key = (src.source.clone(), entry.clone());
        let existing = std::cell::RefCell::new(None::<String>);
        let id = decision.id.clone();
        let recorded_at = decision.recorded_at.clone().unwrap_or_default();
        let result = store.append(decision, |locked| {
            // An entry counts as imported only when the decision its
            // ledger line names is on the record: a ledger line whose
            // decision append failed is superseded by a new one, and the
            // ledger reads its last line for an entry.
            if let Some(found) = read_ledger(locked.root())?.get(&key) {
                if locked.find(found)?.is_some() {
                    *existing.borrow_mut() = Some(found.clone());
                    return Ok(Check::Skip);
                }
            }
            append_ledger(
                locked.root(),
                &LedgerLine {
                    v: 1,
                    source: key.0.clone(),
                    entry: key.1.clone(),
                    decision: id.clone(),
                    recorded_at: recorded_at.clone(),
                },
            )?;
            Ok(Check::Append)
        })?;
        match (result, existing.into_inner()) {
            (WriteOutcome::Appended { id }, _) => out.imported.push(EntryOutcome { entry, id }),
            (_, Some(id)) => out.already_imported.push(EntryOutcome { entry, id }),
            (WriteOutcome::Skipped { id }, None) => {
                out.already_imported.push(EntryOutcome { entry, id })
            }
            (WriteOutcome::Refused { .. }, None) => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision_store::record::Source as Label;

    fn source(entries: serde_json::Value) -> Source {
        serde_json::from_value(serde_json::json!({
            "v": 1,
            "format": COORDINATOR_FORMAT,
            "source": "record:owner/repo#12",
            "coordinator": "coord-1",
            "entries": entries,
        }))
        .unwrap()
    }

    fn store(dir: &Path) -> Store {
        Store::new(
            dir.join("decisions"),
            crate::redact::Redactor::new(Vec::new()),
        )
    }

    #[test]
    fn entries_import_handed_with_the_source_time_and_once() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let src = source(serde_json::json!([
            {"entry": "1", "time": "2026-09-30T23:59Z", "kind": "judgment",
             "question": "Postgres or SQLite?", "decided_by": "coordinator", "reason": "smaller"},
            {"entry": "2", "time": "2026-10-01T10:00:00Z", "kind": "escalate",
             "question": "Drop the cache?", "recipient": "person:dana",
             "decided_by": "coordinator", "chain": ["person:dana"]},
        ]));
        let first = import(&s, &src).unwrap();
        assert_eq!(first.imported.len(), 2);
        assert!(first.already_imported.is_empty());
        let d = crate::decision_store::reader::find_decision(s.root(), &first.imported[0].id)
            .unwrap()
            .unwrap();
        assert_eq!(d.time, "2026-09-30T23:59:00Z");
        assert!(d.id.starts_with("d-202609-"));
        assert!(d.recorded_at.is_some());
        assert_eq!(d.import_source.as_deref(), Some("record:owner/repo#12#1"));
        assert_eq!(d.decided_by.session[0].source, Label::Handed);
        assert_eq!(d.decided_by.session[0].value, "coord-1");
        assert!(d.chain.incomplete);
        let e = crate::decision_store::reader::find_decision(s.root(), &first.imported[1].id)
            .unwrap()
            .unwrap();
        assert!(!e.chain.incomplete);
        assert_eq!(e.chain.links[0][0].source, Label::Handed);

        let again = import(&s, &src).unwrap();
        assert!(again.imported.is_empty());
        assert_eq!(again.already_imported, first.imported);
        let months = crate::decision_store::reader::month_files(s.root()).unwrap();
        let total: usize = months
            .iter()
            .map(|(_, p)| {
                crate::decision_store::reader::read_file(
                    p,
                    &crate::decision_store::reader::Needles::none(),
                )
                .unwrap()
                .decisions
                .len()
            })
            .sum();
        assert_eq!(total, 2);
    }

    #[test]
    fn a_ledger_line_whose_decision_never_landed_is_imported_again() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let src = source(serde_json::json!([
            {"entry": "1", "time": "2026-10-01T10:00Z", "kind": "judgment",
             "question": "q", "decided_by": "coordinator"},
        ]));
        crate::decision_store::writer::ensure_dir(s.root()).unwrap();
        append_ledger(
            s.root(),
            &LedgerLine {
                v: 1,
                source: src.source.clone(),
                entry: "1".into(),
                decision: format!("d-202610-{}", "b".repeat(32)),
                recorded_at: "2026-10-01T10:00:00Z".into(),
            },
        )
        .unwrap();
        let out = import(&s, &src).unwrap();
        assert_eq!(out.imported.len(), 1, "{:?}", out);
        assert!(import(&s, &src).unwrap().imported.is_empty());
    }

    #[test]
    fn a_reversal_entry_or_a_bad_field_refuses_the_whole_import() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let src = source(serde_json::json!([
            {"entry": "1", "time": "2026-10-01T10:00Z", "kind": "judgment",
             "question": "q", "decided_by": "coordinator"},
            {"entry": "2", "time": "2026-10-01T10:00Z", "kind": "reverse",
             "decided_by": "coordinator"},
        ]));
        match import(&s, &src) {
            Err(ImportError::Invalid(e)) => assert_eq!(e.field, "kind"),
            other => panic!("expected a refusal, got {:?}", other),
        }
        assert!(!s.root().exists(), "nothing was written");
        let bad_time = source(serde_json::json!([
            {"entry": "1", "time": "yesterday", "kind": "judgment",
             "question": "q", "decided_by": "coordinator"},
        ]));
        assert!(matches!(import(&s, &bad_time), Err(ImportError::Invalid(e)) if e.field == "time"));
    }

    #[test]
    fn unknown_fields_and_versions_are_refused() {
        let v: Result<Source, _> = serde_json::from_value(serde_json::json!({
            "v": 1, "format": COORDINATOR_FORMAT, "source": "s", "entries": [], "extra": 1}));
        assert!(v.is_err());
        let mut src = source(serde_json::json!([]));
        src.v = 2;
        assert_eq!(prepare(&src).unwrap_err().field, "v");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_or_oversized_source_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.json");
        std::fs::write(&real, b"{}").unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(matches!(read_source(&link), Err(ImportError::Source(_))));
        assert!(matches!(
            read_source(dir.path()),
            Err(ImportError::Source(_))
        ));
        let big = dir.path().join("big.json");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(MAX_SOURCE_BYTES + 1).unwrap();
        assert!(matches!(read_source(&big), Err(ImportError::Source(_))));
    }
}
