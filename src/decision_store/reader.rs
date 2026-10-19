//! Lock-free readers.
//!
//! A reader opens files without following symlinks, skips a torn final
//! line (a writer may be mid-append) and any line it can't read or whose
//! format version it doesn't know, and counts what it skipped. Results are
//! ordered by time, oldest first, with the identifier breaking ties, and
//! every result carries whether it is in effect or reversed.
//!
//! A filter with no time window reads every month file, but only parses
//! the lines that contain every literal the filter implies; a windowed
//! filter reads only the month files the window touches.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::record::{DeciderKind, Decision, Kind, DECISION_FORMAT_VERSION};
use super::{id_month, time_month, StoreError, REVERSALS_FILE};

/// What one file read produced.
#[derive(Debug, Default)]
pub struct FileRead {
    pub decisions: Vec<Decision>,
    /// Lines skipped: malformed, of an unknown version, or a torn tail.
    pub skipped: usize,
}

/// Open a store file for reading without following a symlink. `None` when
/// it doesn't exist.
#[cfg(unix)]
fn open_for_read(path: &Path) -> Result<Option<std::fs::File>, StoreError> {
    use std::os::unix::fs::OpenOptionsExt;
    match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(f) => {
            let meta = f
                .metadata()
                .map_err(|e| StoreError::io(format!("reading {}", path.display()), e))?;
            if !meta.is_file() {
                return Err(StoreError::UnsafeDir {
                    path: path.display().to_string(),
                    reason: "it is not a regular file".to_string(),
                });
            }
            Ok(Some(f))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(StoreError::io(format!("opening {}", path.display()), e)),
    }
}

#[cfg(not(unix))]
fn open_for_read(_path: &Path) -> Result<Option<std::fs::File>, StoreError> {
    Err(StoreError::Unsupported)
}

/// Whether the store directory exists; refuses an unsafe one.
fn store_present(root: &Path) -> Result<bool, StoreError> {
    match std::fs::symlink_metadata(root) {
        Ok(_) => {
            super::writer::check_dir(root)?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(StoreError::io(format!("reading {}", root.display()), e)),
    }
}

/// A literal search every candidate line must pass before it is parsed.
pub struct Needles {
    finders: Vec<aho_corasick::AhoCorasick>,
}

impl Needles {
    pub fn new<I, S>(needles: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<[u8]>,
    {
        let finders = needles
            .into_iter()
            .filter(|n| !n.as_ref().is_empty())
            .filter_map(|n| aho_corasick::AhoCorasick::new([n.as_ref()]).ok())
            .collect();
        Self { finders }
    }

    pub fn none() -> Self {
        Self {
            finders: Vec::new(),
        }
    }

    fn admits(&self, line: &[u8]) -> bool {
        self.finders.iter().all(|f| f.is_match(line))
    }
}

/// Read every line of one store file that passes `needles`.
pub fn read_file(path: &Path, needles: &Needles) -> Result<FileRead, StoreError> {
    use std::io::Read;
    let Some(mut file) = open_for_read(path)? else {
        return Ok(FileRead::default());
    };
    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .map_err(|e| StoreError::io(format!("reading {}", path.display()), e))?;

    let mut out = FileRead::default();
    let mut lines: Vec<&[u8]> = content.split(|b| *b == b'\n').collect();
    // `split` leaves an empty final piece after a trailing newline; a
    // non-empty final piece has no newline and is a torn tail a writer
    // may still be finishing.
    if let Some(last) = lines.pop() {
        if !last.is_empty() {
            out.skipped += 1;
        }
    }
    for line in lines {
        if line.is_empty() || !needles.admits(line) {
            continue;
        }
        match parse_line(line) {
            Some(d) => out.decisions.push(d),
            None => out.skipped += 1,
        }
    }
    Ok(out)
}

fn parse_line(line: &[u8]) -> Option<Decision> {
    let value: serde_json::Value = serde_json::from_slice(line).ok()?;
    if value.get("v").and_then(|v| v.as_u64()) != Some(DECISION_FORMAT_VERSION as u64) {
        return None;
    }
    serde_json::from_value(value).ok()
}

/// The month files present, as `(YYYY-MM, path)`, oldest first.
pub fn month_files(root: &Path) -> Result<Vec<(String, PathBuf)>, StoreError> {
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(StoreError::io(format!("listing {}", root.display()), e)),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| StoreError::io(format!("listing {}", root.display()), e))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(month) = name.strip_suffix(".jsonl") else {
            continue;
        };
        if time_month(&format!("{}-01", month)).as_deref() == Some(month) && month.len() == 7 {
            out.push((month.to_string(), entry.path()));
        }
    }
    out.sort();
    Ok(out)
}

/// Every line of the reversal log.
pub fn read_reversals(root: &Path) -> Result<FileRead, StoreError> {
    read_file(&root.join(REVERSALS_FILE), &Needles::none())
}

/// The decision with `id`, from the month file it names or the reversal
/// log. Reads nothing else.
pub fn find_decision(root: &Path, id: &str) -> Result<Option<Decision>, StoreError> {
    let Some(month) = id_month(id) else {
        return Ok(None);
    };
    let needles = Needles::new([id.as_bytes()]);
    for path in [
        root.join(format!("{}.jsonl", month)),
        root.join(REVERSALS_FILE),
    ] {
        if let Some(d) = read_file(&path, &needles)?
            .decisions
            .into_iter()
            .find(|d| d.id == id)
        {
            return Ok(Some(d));
        }
    }
    Ok(None)
}

/// Each reversed decision's identifier, mapped to its (earliest) reversal.
pub fn reversal_index(reversals: &[Decision]) -> HashMap<String, &Decision> {
    let mut index: HashMap<String, &Decision> = HashMap::new();
    for r in reversals {
        if r.kind != Kind::Reverse {
            continue;
        }
        let Some(target) = r.reverses.as_deref() else {
            continue;
        };
        index
            .entry(target.to_string())
            .and_modify(|cur| {
                if order_key(r) < order_key(cur) {
                    *cur = r;
                }
            })
            .or_insert(r);
    }
    index
}

fn order_key(d: &Decision) -> (&str, &str) {
    (d.time.as_str(), d.id.as_str())
}

/// Whether a decision is in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    InEffect,
    Reversed,
}

/// One result: the decision and its status.
#[derive(Debug, Clone, Serialize)]
pub struct Listed {
    pub decision: Decision,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reversed_by: Option<String>,
}

impl Listed {
    fn new(decision: Decision, index: &HashMap<String, &Decision>) -> Self {
        match index.get(&decision.id) {
            Some(r) => Listed {
                reversed_by: Some(r.id.clone()),
                decision,
                status: Status::Reversed,
            },
            None => Listed {
                decision,
                status: Status::InEffect,
                reversed_by: None,
            },
        }
    }
}

/// A host-wide listing filter. Every set field must match.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub kind: Option<Kind>,
    pub decider_kind: Option<DeciderKind>,
    /// Subject fields that must equal the given value.
    pub subject: Vec<(String, String)>,
    /// Inclusive window start, normalized.
    pub since: Option<String>,
    /// Exclusive window end, normalized.
    pub until: Option<String>,
    /// Only decisions whose decider is (true) or isn't (false) defaulted.
    pub defaulted: Option<bool>,
    /// A session the decider names (its own, an entering or a running one).
    pub session: Option<String>,
    pub run_mode: Option<String>,
    pub dispatch_id: Option<String>,
    pub instructions_digest: Option<String>,
}

/// JSON-escape a string the way the writer serializes it, without its
/// quotes, for a literal search.
fn escaped(s: &str) -> Vec<u8> {
    let quoted = serde_json::to_string(s).unwrap_or_default();
    quoted.as_bytes()[1..quoted.len() - 1].to_vec()
}

impl Filter {
    fn windowed(&self) -> bool {
        self.since.is_some() || self.until.is_some()
    }

    /// The literals a matching line must contain.
    ///
    /// They are a prefilter only: they assume the writer's compact
    /// `serde_json` output, and every line that passes is still checked
    /// by [`Filter::matches`] after parsing, so a literal that over-admits
    /// costs a parse, never a wrong result.
    fn needles(&self) -> Needles {
        let mut n: Vec<Vec<u8>> = Vec::new();
        if let Some(k) = self.kind {
            n.push(format!("\"kind\":\"{}\"", k.as_str()).into_bytes());
        }
        if let Some(k) = self.decider_kind {
            n.push(format!("\"kind\":\"{}\"", k.as_str()).into_bytes());
        }
        if self.defaulted == Some(true) {
            n.push(b"\"defaulted\":true".to_vec());
        }
        for (_, v) in &self.subject {
            n.push(escaped(v));
        }
        for v in [
            &self.session,
            &self.run_mode,
            &self.dispatch_id,
            &self.instructions_digest,
        ]
        .into_iter()
        .flatten()
        {
            n.push(escaped(v));
        }
        Needles::new(n)
    }

    /// Whether a parsed decision matches every set field.
    pub fn matches(&self, d: &Decision) -> bool {
        if self.kind.is_some_and(|k| k != d.kind) {
            return false;
        }
        if self.decider_kind.is_some_and(|k| k != d.decided_by.kind) {
            return false;
        }
        if let Some(since) = &self.since {
            if d.time.as_str() < since.as_str() {
                return false;
            }
        }
        if let Some(until) = &self.until {
            if d.time.as_str() >= until.as_str() {
                return false;
            }
        }
        if self
            .defaulted
            .is_some_and(|want| want != d.decided_by.defaulted)
        {
            return false;
        }
        if !self.subject.is_empty() {
            let subject = serde_json::to_value(&d.subject).unwrap_or_default();
            for (field, want) in &self.subject {
                let ok = match subject.get(field) {
                    Some(serde_json::Value::String(s)) => s == want,
                    Some(serde_json::Value::Number(n)) => n.to_string() == *want,
                    Some(serde_json::Value::Array(a)) => a.iter().any(|v| v.as_str() == Some(want)),
                    _ => false,
                };
                if !ok {
                    return false;
                }
            }
        }
        let has = |slot: &[super::Sourced], want: &str| slot.iter().any(|s| s.value == want);
        if let Some(s) = &self.session {
            if !d.decided_by.sessions().any(|v| v.value == *s) {
                return false;
            }
        }
        if let Some(v) = &self.run_mode {
            if !has(&d.authority.run_mode, v) {
                return false;
            }
        }
        if let Some(v) = &self.dispatch_id {
            if !has(&d.authority.dispatch_id, v) {
                return false;
            }
        }
        if let Some(v) = &self.instructions_digest {
            if !has(&d.authority.instructions_digest, v) {
                return false;
            }
        }
        true
    }

    /// Whether the month `YYYY-MM` can hold a decision inside the window.
    fn month_in_window(&self, month: &str) -> bool {
        let start = format!("{}-01T00:00:00Z", month);
        let next = next_month_start(month);
        if let Some(until) = &self.until {
            if start.as_str() >= until.as_str() {
                return false;
            }
        }
        if let (Some(since), Some(next)) = (&self.since, next) {
            if next.as_str() <= since.as_str() {
                return false;
            }
        }
        true
    }
}

/// The first instant of the month after `YYYY-MM`.
pub fn next_month_start(month: &str) -> Option<String> {
    let (y, m) = month.split_once('-')?;
    let (y, m): (u32, u32) = (y.parse().ok()?, m.parse().ok()?);
    let (y, m) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    Some(format!("{:04}-{:02}-01T00:00:00Z", y, m))
}

/// A listing: matching decisions, oldest first, each with its status.
#[derive(Debug, Serialize)]
pub struct Listing {
    pub count: usize,
    pub items: Vec<Listed>,
    /// Lines the readers couldn't read and skipped.
    pub skipped_lines: usize,
}

/// Read the decisions a filter selects from month files.
///
/// Returns the matching decisions and the count of skipped lines. Reads
/// only the months a window touches; with no window, every month, parsing
/// only lines that pass the filter's literals.
pub fn select_months(root: &Path, filter: &Filter) -> Result<FileRead, StoreError> {
    let needles = filter.needles();
    let mut out = FileRead::default();
    for (month, path) in month_files(root)? {
        if filter.windowed() && !filter.month_in_window(&month) {
            continue;
        }
        let read = read_file(&path, &needles)?;
        out.skipped += read.skipped;
        out.decisions
            .extend(read.decisions.into_iter().filter(|d| filter.matches(d)));
    }
    Ok(out)
}

/// List decisions host-wide.
pub fn list(root: &Path, filter: &Filter) -> Result<Listing, StoreError> {
    if !store_present(root)? {
        return Ok(Listing {
            count: 0,
            items: Vec::new(),
            skipped_lines: 0,
        });
    }
    let reversals = read_reversals(root)?;
    let index = reversal_index(&reversals.decisions);

    let mut selected = select_months(root, filter)?;
    let skipped = selected.skipped + reversals.skipped;
    // Reversals live in their own log; they list when the filter selects
    // them.
    selected.decisions.extend(
        reversals
            .decisions
            .iter()
            .filter(|d| filter.matches(d))
            .cloned(),
    );
    selected
        .decisions
        .sort_by(|a, b| order_key(a).cmp(&order_key(b)));
    let items: Vec<Listed> = selected
        .decisions
        .into_iter()
        .map(|d| Listed::new(d, &index))
        .collect();
    Ok(Listing {
        count: items.len(),
        items,
        skipped_lines: skipped,
    })
}

/// One decision by identifier, with its status and, when reversed, the
/// reversal.
#[derive(Debug, Serialize)]
pub struct Shown {
    #[serde(flatten)]
    pub listed: Listed,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reversal: Option<Decision>,
}

/// Show one decision. Reads only the month file the identifier names and
/// the reversal log.
pub fn show(root: &Path, id: &str) -> Result<Option<Shown>, StoreError> {
    if !store_present(root)? {
        return Ok(None);
    }
    let Some(decision) = find_decision(root, id)? else {
        return Ok(None);
    };
    let reversals = read_reversals(root)?;
    let index = reversal_index(&reversals.decisions);
    let reversal = index.get(id).map(|r| (*r).clone());
    Ok(Some(Shown {
        listed: Listed::new(decision, &index),
        reversal,
    }))
}
