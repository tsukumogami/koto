//! Under what authority, and through which dispatch chain, a decider acted.
//!
//! koto reads lineage; it never asks for it. Three sources feed it, each
//! value keeping the label its source earns:
//!
//! - **The workspace manager's session map.** koto walks up from its working
//!   directory to the workspace root (the directory holding
//!   `.niwa/workspace.toml`) and reads `<root>/.niwa/sessions/<id>.json` for
//!   the harness session. It never runs the workspace manager and never
//!   writes under `.niwa/`. Today's entries carry `dispatch_id` and
//!   `parent_session_id`; the parent is the dispatching caller's own claim,
//!   so both are labelled `handed`, and an entry with no parent ends the
//!   chain `incomplete` (today's map can't say whether the session was a
//!   root). An entry may also carry a versioned `lineage` object
//!   ([`LINEAGE_VERSION`]) whose per-field `source` gives each value's
//!   label, whose `parent` may be `none` (an affirmed root), and whose copied
//!   `chain` survives the removal of an ancestor's entry. An unknown version
//!   is treated as absent.
//! - **The run journal's driver stamp.** When a koto session first records a
//!   driver, it stamps the lineage it resolved into that `session_started`
//!   or `driver_seen` record, so a session resumed after its map entry is
//!   removed still resolves its dispatch identifier and parent.
//! - **Handed flags.** `--dispatched-by`, `--chain-file`, `--dispatch-id` and
//!   `--instructions-digest` are always `handed`. They are resolved as a
//!   sequence of their own and set beside the observed one, never merged
//!   into it: where both give a value for one link and the values differ,
//!   both are kept and the chain is `incomplete`.
//!
//! Every identifier is checked with [`is_identifier`] before it becomes part
//! of a path, the parent walk stops at [`MAX_CHAIN_DEPTH`] and at the first
//! repeated identifier, and a map entry is read only when the opened file is
//! a regular file owned by the current user that no other user can write.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::decider::{is_identifier, is_name};
use super::record::{Authority, Chain, DeciderKind, FieldError, Slot, Source, Sourced};

/// The lineage format version koto reads, in a session-map entry's
/// `lineage.v` and a chain file's `v`.
pub const LINEAGE_VERSION: u32 = 1;

/// The most links a chain may have; the walk stops there.
pub const MAX_CHAIN_DEPTH: usize = 32;

/// The largest session-map entry koto reads.
pub const MAX_ENTRY_BYTES: u64 = 64 * 1024;

/// The largest `--chain-file` koto reads.
pub const MAX_CHAIN_FILE_BYTES: u64 = 64 * 1024;

/// How many directories koto climbs looking for the workspace root.
const MAX_WALK_UP: usize = 64;

/// The workspace manager's directory at the workspace root.
const NIWA_DIR: &str = ".niwa";
/// The file whose presence marks the workspace root.
const WORKSPACE_MARKER: &str = "workspace.toml";
/// The session map's directory under [`NIWA_DIR`].
const SESSIONS_DIR: &str = "sessions";

/// The root link value: the source affirms there is no dispatcher.
pub const ROOT_NONE: &str = "none";
/// The parent value a lineage entry gives when it doesn't know.
pub const PARENT_UNKNOWN: &str = "unknown";

/// The run-journal fields the driver stamp is written under. The run
/// journal owns their names, beside every other field it writes.
pub(crate) use crate::run_journal::lineage_fields as stamp_fields;

/// One link reference: what a `parent` or `--dispatched-by` value names.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ref {
    Session(String),
    Person,
    Root,
    Unknown,
}

fn parse_ref(value: &str) -> Option<Ref> {
    if value == ROOT_NONE {
        return Some(Ref::Root);
    }
    if value == PARENT_UNKNOWN {
        return Some(Ref::Unknown);
    }
    match value.split_once(':')? {
        ("session", id) if is_identifier(id) => Some(Ref::Session(id.to_string())),
        ("person", handle) if is_name(handle) => Some(Ref::Person),
        _ => None,
    }
}

/// Whether `value` is a well-formed instructions digest: `sha256:` and 64
/// lowercase hex characters.
pub fn is_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// Check a handed `--dispatched-by`: `session:<id>`, `person:<handle>` or
/// `none`.
pub fn validate_dispatched_by(value: &str) -> Result<(), FieldError> {
    match parse_ref(value) {
        Some(Ref::Session(_) | Ref::Person | Ref::Root) => Ok(()),
        _ => Err(FieldError::new(
            "dispatched_by",
            format!(
                "'{}' is not a dispatcher (use session:<id>, person:<handle> or none)",
                value
            ),
        )),
    }
}

/// Check a handed `--dispatch-id`.
pub fn validate_dispatch_id(value: &str) -> Result<(), FieldError> {
    if is_identifier(value) {
        Ok(())
    } else {
        Err(FieldError::new(
            "dispatch_id",
            format!("'{}' is not a dispatch identifier", value),
        ))
    }
}

/// Check a handed `--instructions-digest`.
pub fn validate_digest(value: &str) -> Result<(), FieldError> {
    if is_digest(value) {
        Ok(())
    } else {
        Err(FieldError::new(
            "instructions_digest",
            format!("'{}' is not sha256:<64 lowercase hex>", value),
        ))
    }
}

fn label(raw: Option<&str>) -> Source {
    match raw {
        Some("observed") => Source::Observed,
        _ => Source::Handed,
    }
}

// ----- Reading files safely -----

/// Open `path` without following a symlink, and return it only when the
/// descriptor shows a regular file owned by the current user that neither
/// group nor others can write, no larger than `cap`.
#[cfg(unix)]
fn open_private(path: &Path, cap: u64) -> Option<File> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    let uid = unsafe { libc::geteuid() };
    if !meta.file_type().is_file() || meta.uid() != uid || meta.mode() & 0o022 != 0 {
        return None;
    }
    (meta.len() <= cap).then_some(file)
}

#[cfg(not(unix))]
fn open_private(path: &Path, cap: u64) -> Option<File> {
    let file = File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    (meta.file_type().is_file() && meta.len() <= cap).then_some(file)
}

fn read_capped(file: File, cap: u64) -> Option<String> {
    let mut text = String::new();
    file.take(cap + 1).read_to_string(&mut text).ok()?;
    (text.len() as u64 <= cap).then_some(text)
}

// ----- The workspace manager's session map -----

/// The workspace root above `start`: the nearest directory holding
/// `.niwa/workspace.toml`.
pub fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    for _ in 0..MAX_WALK_UP {
        let d = dir?;
        if d.join(NIWA_DIR).join(WORKSPACE_MARKER).is_file() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

/// Where the workspace-root search starts: the working directory. In unit
/// tests nothing is searched unless a test sets a start, so a test run
/// inside a real workspace never reads its session map.
#[cfg(not(test))]
pub(crate) fn search_start() -> Option<PathBuf> {
    std::env::current_dir().ok()
}

#[cfg(test)]
pub(crate) fn search_start() -> Option<PathBuf> {
    SEARCH_START.with(|s| s.borrow().clone())
}

#[cfg(test)]
thread_local! {
    static SEARCH_START: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Set (or clear) where this thread's unit tests search for a workspace.
#[cfg(test)]
pub(crate) fn set_search_start_for_test(dir: Option<&Path>) {
    SEARCH_START.with(|s| *s.borrow_mut() = dir.map(Path::to_path_buf));
}

/// A session-map entry, in today's shape plus the optional `lineage`.
#[derive(Debug, Deserialize)]
struct RawEntry {
    #[serde(default)]
    dispatch_id: Option<String>,
    #[serde(default)]
    parent_session_id: Option<String>,
    #[serde(default)]
    lineage: Option<serde_json::Value>,
}

/// The versioned lineage object, read only when `v` is
/// [`LINEAGE_VERSION`].
#[derive(Debug, Deserialize)]
struct LineageV1 {
    v: u32,
    #[serde(default)]
    dispatch_id: Option<String>,
    #[serde(default)]
    brief_digest: Option<String>,
    #[serde(default)]
    parent: Option<String>,
    #[serde(default)]
    chain: Vec<CopiedLink>,
    /// Per field: `observed` or `handed`. A field with no label is handed.
    #[serde(default)]
    source: BTreeMap<String, String>,
}

/// One link of a copied chain: a session and its parent.
#[derive(Debug, Deserialize)]
struct CopiedLink {
    session: String,
    #[serde(default)]
    parent: Option<String>,
    /// `observed` or `handed` for this link's parent; falls back to the
    /// lineage's `source.chain`.
    #[serde(default)]
    source: Option<String>,
}

/// What one session's entry (or its driver stamp) says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    pub dispatch_id: Option<Sourced>,
    pub digest: Option<Sourced>,
    /// The parent reference (`session:<id>`, `person:<h>`, `none` or
    /// `unknown`), labelled.
    pub parent: Option<Sourced>,
    /// Parent references copied from ancestors, by session id.
    copied: HashMap<String, Sourced>,
}

impl Entry {
    fn from_raw(raw: RawEntry) -> Self {
        let mut e = Entry::default();
        let lineage = raw
            .lineage
            .filter(|l| {
                l.get("v").and_then(serde_json::Value::as_u64) == Some(LINEAGE_VERSION.into())
            })
            .and_then(|l| serde_json::from_value::<LineageV1>(l).ok())
            .filter(|l| l.v == LINEAGE_VERSION);
        if let Some(l) = lineage {
            let src = |f: &str| label(l.source.get(f).map(String::as_str));
            e.dispatch_id = l.dispatch_id.filter(|v| is_identifier(v)).map(|v| Sourced {
                value: v,
                source: src("dispatch_id"),
            });
            e.digest = l.brief_digest.filter(|v| is_digest(v)).map(|v| Sourced {
                value: v,
                source: src("brief_digest"),
            });
            e.parent = l
                .parent
                .filter(|v| parse_ref(v).is_some())
                .map(|v| Sourced {
                    value: v,
                    source: src("parent"),
                });
            let chain_label = l.source.get("chain").map(String::as_str);
            for link in l.chain.into_iter().take(MAX_CHAIN_DEPTH) {
                let Some(parent) = link.parent.filter(|p| parse_ref(p).is_some()) else {
                    continue;
                };
                if !is_identifier(&link.session) {
                    continue;
                }
                let source = label(link.source.as_deref().or(chain_label));
                e.copied.entry(link.session).or_insert(Sourced {
                    value: parent,
                    source,
                });
            }
        }
        // Today's fields fill whatever the lineage object left empty. The
        // workspace manager records them as its caller supplied them.
        if e.dispatch_id.is_none() {
            e.dispatch_id = raw
                .dispatch_id
                .filter(|v| is_identifier(v))
                .map(Sourced::handed);
        }
        if e.parent.is_none() {
            e.parent = raw
                .parent_session_id
                .filter(|v| is_identifier(v))
                .map(|p| Sourced::handed(format!("session:{}", p)));
        }
        e
    }
}

/// Where lineage is read from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sources {
    /// The workspace root whose `.niwa/sessions/` holds the session map.
    pub workspace_root: Option<PathBuf>,
    /// The run journal's home, for the driver-stamp fallback.
    pub journal_root: Option<PathBuf>,
}

impl Sources {
    /// The workspace root above the working directory, and the given
    /// journal home.
    pub fn discover(journal_root: Option<PathBuf>) -> Self {
        Self {
            workspace_root: search_start().as_deref().and_then(find_workspace_root),
            journal_root,
        }
    }

    /// The session-map entry for `id`, when the id is safe to build a path
    /// from and the file passes the ownership and mode checks.
    pub fn map_entry(&self, id: &str) -> Option<Entry> {
        if !is_identifier(id) {
            return None;
        }
        let path = self
            .workspace_root
            .as_ref()?
            .join(NIWA_DIR)
            .join(SESSIONS_DIR)
            .join(format!("{}.json", id));
        let text = read_capped(open_private(&path, MAX_ENTRY_BYTES)?, MAX_ENTRY_BYTES)?;
        let raw: RawEntry = serde_json::from_str(&text).ok()?;
        Some(Entry::from_raw(raw))
    }

    /// The map entry for `id`, else the newest driver stamp the run journal
    /// holds for it.
    fn entry(&self, id: &str) -> Option<Entry> {
        self.map_entry(id).or_else(|| {
            self.journal_root
                .as_deref()
                .and_then(|root| journal_stamp(root, id))
        })
    }
}

// ----- The run journal's driver stamp -----

/// The journal fields that stamp `driver`'s lineage, read from its
/// session-map entry: the dispatch identifier, the instructions digest and
/// the parent, each with its label. Empty when there is no entry.
pub(crate) fn driver_stamp(driver: &str) -> Vec<(&'static str, String)> {
    let sources = Sources::discover(None);
    let Some(e) = sources.map_entry(driver) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (value, field, source_field) in [
        (
            &e.dispatch_id,
            stamp_fields::DISPATCH_ID,
            stamp_fields::DISPATCH_ID_SOURCE,
        ),
        (&e.digest, stamp_fields::DIGEST, stamp_fields::DIGEST_SOURCE),
        (&e.parent, stamp_fields::PARENT, stamp_fields::PARENT_SOURCE),
    ] {
        if let Some(v) = value {
            out.push((field, v.value.clone()));
            out.push((source_field, source_name(v.source).to_string()));
        }
    }
    out
}

fn source_name(s: Source) -> &'static str {
    match s {
        Source::Observed => "observed",
        Source::Handed => "handed",
    }
}

/// The newest stamp the active run journal under `root` holds for `driver`.
fn journal_stamp(root: &Path, driver: &str) -> Option<Entry> {
    if !is_identifier(driver) {
        return None;
    }
    let path = root.join(crate::run_journal::JOURNAL_FILE);
    let file = open_private(&path, u64::MAX)?;
    let needle = format!("\"koto.driver.session.id\":\"{}\"", driver);
    let mut newest: Option<String> = None;
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        if line.contains(&needle) && line.contains("\"koto.lineage.") {
            newest = Some(line);
        }
    }
    let record: serde_json::Value = serde_json::from_str(&newest?).ok()?;
    let get = |value: &str, source: &str| -> Option<Sourced> {
        let v = record.get(value)?.as_str()?.to_string();
        Some(Sourced {
            value: v,
            source: label(record.get(source).and_then(serde_json::Value::as_str)),
        })
    };
    let entry = Entry {
        dispatch_id: get(stamp_fields::DISPATCH_ID, stamp_fields::DISPATCH_ID_SOURCE)
            .filter(|v| is_identifier(&v.value)),
        digest: get(stamp_fields::DIGEST, stamp_fields::DIGEST_SOURCE)
            .filter(|v| is_digest(&v.value)),
        parent: get(stamp_fields::PARENT, stamp_fields::PARENT_SOURCE)
            .filter(|v| parse_ref(&v.value).is_some()),
        copied: HashMap::new(),
    };
    (entry != Entry::default()).then_some(entry)
}

// ----- Handed lineage -----

/// A `--chain-file`: the links above the dispatcher, in the lineage format.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChainFile {
    pub v: u32,
    /// The deciding session's dispatcher, when `--dispatched-by` doesn't
    /// give it.
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub chain: Vec<ChainFileLink>,
}

/// One link of a chain file: a session and its parent.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChainFileLink {
    pub session: String,
    #[serde(default)]
    pub dispatch_id: Option<String>,
    pub parent: String,
}

/// Read and check a `--chain-file`: opened without following a symlink,
/// a regular file of at most [`MAX_CHAIN_FILE_BYTES`], strict JSON in the
/// lineage format at [`LINEAGE_VERSION`].
pub fn read_chain_file(path: &Path) -> Result<ChainFile, FieldError> {
    let err = |msg: String| FieldError::new("chain_file", msg);
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
    };
    #[cfg(not(unix))]
    let file = File::open(path);
    let file = file.map_err(|e| err(format!("can't open {}: {}", path.display(), e)))?;
    let meta = file
        .metadata()
        .map_err(|e| err(format!("can't read {}: {}", path.display(), e)))?;
    if !meta.file_type().is_file() {
        return Err(err(format!("{} is not a regular file", path.display())));
    }
    if meta.len() > MAX_CHAIN_FILE_BYTES {
        return Err(err(format!(
            "{} is larger than {} bytes",
            path.display(),
            MAX_CHAIN_FILE_BYTES
        )));
    }
    let text = read_capped(file, MAX_CHAIN_FILE_BYTES).ok_or_else(|| {
        err(format!(
            "{} could not be read as UTF-8 text",
            path.display()
        ))
    })?;
    let parsed: ChainFile =
        serde_json::from_str(&text).map_err(|e| err(format!("not a chain file: {}", e)))?;
    if parsed.v != LINEAGE_VERSION {
        return Err(err(format!(
            "chain file version {} is not supported (expected {})",
            parsed.v, LINEAGE_VERSION
        )));
    }
    if parsed.chain.len() > MAX_CHAIN_DEPTH {
        return Err(err(format!(
            "a chain file holds at most {} links",
            MAX_CHAIN_DEPTH
        )));
    }
    if let Some(p) = &parsed.parent {
        if parse_ref(p).is_none() {
            return Err(err(format!("parent '{}' is not a link", p)));
        }
    }
    for link in &parsed.chain {
        if !is_identifier(&link.session) {
            return Err(err(format!(
                "'{}' is not a session identifier",
                link.session
            )));
        }
        if parse_ref(&link.parent).is_none() {
            return Err(err(format!("parent '{}' is not a link", link.parent)));
        }
        if let Some(d) = &link.dispatch_id {
            if !is_identifier(d) {
                return Err(err(format!("'{}' is not a dispatch identifier", d)));
            }
        }
    }
    Ok(parsed)
}

/// Lineage a caller hands with the decision. Every value is `handed`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HandedLineage {
    pub dispatched_by: Option<String>,
    pub chain_file: Option<ChainFile>,
    pub dispatch_id: Option<String>,
    pub instructions_digest: Option<String>,
}

// ----- Resolving -----

/// One source's links, in order from the deciding session's dispatcher up,
/// and whether the last one is a root that source affirms.
#[derive(Debug, Default)]
struct Sequence {
    links: Vec<Sourced>,
    root: bool,
}

/// Walk parent references from `first`, asking `next` for each session's
/// parent, until a root, an unknown or missing parent, the depth cap or a
/// repeated session.
fn walk(
    start: Option<&str>,
    first: Option<Sourced>,
    mut next: impl FnMut(&str) -> Option<Sourced>,
) -> Sequence {
    let mut seq = Sequence::default();
    let mut seen: HashSet<String> = start.map(str::to_string).into_iter().collect();
    let mut cur = first;
    while let Some(link) = cur.take() {
        if seq.links.len() >= MAX_CHAIN_DEPTH {
            break;
        }
        match parse_ref(&link.value) {
            Some(Ref::Root | Ref::Person) => {
                seq.links.push(link);
                seq.root = true;
            }
            Some(Ref::Session(id)) => {
                if !seen.insert(id.clone()) {
                    break;
                }
                seq.links.push(link);
                cur = next(&id);
            }
            Some(Ref::Unknown) | None => {}
        }
    }
    seq
}

/// The observed sequence: the session's entry (or its stamp), then each
/// ancestor's parent from the copied chain or the ancestor's own entry.
fn observed_sequence(sources: &Sources, session: &str) -> (Option<Entry>, Sequence) {
    let Some(entry) = sources.entry(session) else {
        return (None, Sequence::default());
    };
    let mut copied = entry.copied.clone();
    let seq = walk(Some(session), entry.parent.clone(), |id| {
        if let Some(p) = copied.get(id) {
            return Some(p.clone());
        }
        let e = sources.map_entry(id)?;
        for (k, v) in e.copied {
            copied.entry(k).or_insert(v);
        }
        e.parent
    });
    (Some(entry), seq)
}

/// The handed sequence: `--dispatched-by` (or the chain file's `parent`),
/// then each session's parent from the chain file.
fn handed_sequence(handed: &HandedLineage) -> Sequence {
    let file = handed.chain_file.as_ref();
    let first = handed
        .dispatched_by
        .clone()
        .or_else(|| file.and_then(|f| f.parent.clone()))
        .map(Sourced::handed);
    let parents: HashMap<&str, &str> = file
        .map(|f| {
            f.chain
                .iter()
                .map(|l| (l.session.as_str(), l.parent.as_str()))
                .collect()
        })
        .unwrap_or_default();
    walk(None, first, |id| {
        parents.get(id).map(|p| Sourced::handed(*p))
    })
}

/// Two values for one identity: one entry when they agree (the observed
/// label wins), both when they differ. The flag says they differed.
fn merge_pair(a: Option<Sourced>, b: Option<Sourced>) -> (Slot, bool) {
    match (a, b) {
        (None, None) => (Vec::new(), false),
        (Some(x), None) | (None, Some(x)) => (vec![x], false),
        (Some(x), Some(y)) if x.value == y.value => {
            let keep = if y.source == Source::Observed { y } else { x };
            (vec![keep], false)
        }
        (Some(x), Some(y)) => (vec![x, y], true),
    }
}

/// The authority and chain a decision records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lineage {
    pub authority: Authority,
    pub chain: Chain,
}

/// Resolve the authority and dispatch chain for a decider of `kind`.
///
/// `session` is the session the lineage is read for: the observed harness
/// session (the deciding session, the one that entered a person's
/// decision, or the one that ran a check). `run_mode` is the slot the
/// decider resolver built. The chain is `incomplete` unless a source
/// affirms its last link as a root and no two sources disagree anywhere
/// (on a link, the dispatch identifier or the digest).
pub fn resolve_lineage(
    kind: DeciderKind,
    session: Option<&str>,
    run_mode: Slot,
    handed: &HandedLineage,
    sources: &Sources,
) -> Lineage {
    let (entry, observed) = match session {
        Some(s) => observed_sequence(sources, s),
        None => (None, Sequence::default()),
    };
    let entry = entry.unwrap_or_default();
    let handed_seq = handed_sequence(handed);

    let mut disagree = false;
    let n = observed.links.len().max(handed_seq.links.len());
    let mut links: Vec<Slot> = Vec::with_capacity(n);
    for i in 0..n {
        let (slot, differ) = merge_pair(
            observed.links.get(i).cloned(),
            handed_seq.links.get(i).cloned(),
        );
        disagree |= differ;
        links.push(slot);
    }
    let reaches_root = match observed.links.len().cmp(&handed_seq.links.len()) {
        std::cmp::Ordering::Greater => observed.root,
        std::cmp::Ordering::Less => handed_seq.root,
        std::cmp::Ordering::Equal => n > 0 && (observed.root || handed_seq.root),
    };

    let (dispatch_id, differ) = merge_pair(
        entry.dispatch_id.clone(),
        handed.dispatch_id.clone().map(Sourced::handed),
    );
    disagree |= differ;
    let (instructions_digest, differ) = merge_pair(
        entry.digest.clone(),
        handed.instructions_digest.clone().map(Sourced::handed),
    );
    disagree |= differ;

    let mut authority = Authority::for_decider(kind);
    authority.dispatch_id = dispatch_id;
    authority.instructions_digest = instructions_digest;
    authority.run_mode = run_mode;
    authority.dispatcher = links.first().cloned().unwrap_or_default();

    Lineage {
        authority,
        chain: Chain {
            links,
            incomplete: disagree || !reaches_root,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Ws {
        dir: tempfile::TempDir,
    }

    impl Ws {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(dir.path().join(".niwa/sessions")).unwrap();
            std::fs::write(dir.path().join(".niwa/workspace.toml"), "").unwrap();
            Ws { dir }
        }

        fn entry(&self, id: &str, body: serde_json::Value) {
            let p = self.dir.path().join(format!(".niwa/sessions/{}.json", id));
            std::fs::write(&p, body.to_string()).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        fn sources(&self) -> Sources {
            Sources {
                workspace_root: Some(self.dir.path().to_path_buf()),
                journal_root: None,
            }
        }
    }

    fn resolve(sources: &Sources, session: Option<&str>, handed: &HandedLineage) -> Lineage {
        resolve_lineage(DeciderKind::Session, session, Vec::new(), handed, sources)
    }

    fn link_values(chain: &Chain) -> Vec<Vec<(String, Source)>> {
        chain
            .links
            .iter()
            .map(|s| s.iter().map(|v| (v.value.clone(), v.source)).collect())
            .collect()
    }

    const H: Source = Source::Handed;
    const O: Source = Source::Observed;

    #[test]
    fn the_workspace_root_is_found_above_the_start() {
        let ws = Ws::new();
        let deep = ws.dir.path().join("a/b/c");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(find_workspace_root(&deep).as_deref(), Some(ws.dir.path()));
        let other = tempfile::tempdir().unwrap();
        assert_eq!(find_workspace_root(other.path()), None);
    }

    #[test]
    fn todays_entry_is_handed_and_ends_incomplete() {
        let ws = Ws::new();
        ws.entry(
            "child",
            serde_json::json!({"session_id": "child", "dispatch_id": "disp-1",
                "parent_session_id": "coord", "origin": "dispatch"}),
        );
        ws.entry("coord", serde_json::json!({"session_id": "coord"}));
        let l = resolve(&ws.sources(), Some("child"), &HandedLineage::default());
        assert_eq!(l.authority.kind, "dispatch");
        assert_eq!(l.authority.dispatch_id, vec![Sourced::handed("disp-1")]);
        assert_eq!(
            l.authority.dispatcher,
            vec![Sourced::handed("session:coord")]
        );
        assert_eq!(
            link_values(&l.chain),
            vec![vec![("session:coord".into(), H)]]
        );
        assert!(l.chain.incomplete);
    }

    #[test]
    fn the_walk_follows_parents_through_existing_entries() {
        let ws = Ws::new();
        ws.entry("c", serde_json::json!({"parent_session_id": "b"}));
        ws.entry("b", serde_json::json!({"parent_session_id": "a"}));
        let l = resolve(&ws.sources(), Some("c"), &HandedLineage::default());
        assert_eq!(
            link_values(&l.chain),
            vec![vec![("session:b".into(), H)], vec![("session:a".into(), H)]]
        );
        assert!(l.chain.incomplete);
    }

    #[test]
    fn a_versioned_entry_with_a_copied_chain_reaches_an_affirmed_root() {
        let ws = Ws::new();
        let digest = format!("sha256:{}", "ab".repeat(32));
        ws.entry(
            "c",
            serde_json::json!({"lineage": {"v": 1, "dispatch_id": "disp-c",
                "brief_digest": digest, "parent": "session:b",
                "chain": [{"session": "b", "parent": "session:a"},
                          {"session": "a", "parent": "none", "source": "handed"}],
                "origin": "dispatch",
                "source": {"dispatch_id": "observed", "brief_digest": "observed",
                           "parent": "observed", "chain": "observed"}}}),
        );
        let l = resolve(&ws.sources(), Some("c"), &HandedLineage::default());
        assert_eq!(
            link_values(&l.chain),
            vec![
                vec![("session:b".into(), O)],
                vec![("session:a".into(), O)],
                vec![("none".into(), H)]
            ]
        );
        assert!(!l.chain.incomplete);
        assert_eq!(l.authority.dispatch_id, vec![Sourced::observed("disp-c")]);
        assert_eq!(
            l.authority.instructions_digest,
            vec![Sourced::observed(digest)]
        );
    }

    #[test]
    fn a_versioned_entry_with_parent_none_is_a_root() {
        let ws = Ws::new();
        ws.entry(
            "r",
            serde_json::json!({"lineage": {"v": 1, "parent": "none",
                "source": {"parent": "observed"}}}),
        );
        let l = resolve(&ws.sources(), Some("r"), &HandedLineage::default());
        assert_eq!(link_values(&l.chain), vec![vec![("none".into(), O)]]);
        assert!(!l.chain.incomplete);
    }

    #[test]
    fn an_unknown_lineage_version_is_absent() {
        let ws = Ws::new();
        ws.entry(
            "c",
            serde_json::json!({"dispatch_id": "disp-1", "parent_session_id": "p",
                "lineage": {"v": 9, "parent": "none", "source": {"parent": "observed"}}}),
        );
        let l = resolve(&ws.sources(), Some("c"), &HandedLineage::default());
        assert_eq!(link_values(&l.chain), vec![vec![("session:p".into(), H)]]);
        assert!(l.chain.incomplete);
    }

    #[test]
    fn a_handed_root_closes_the_chain() {
        let handed = HandedLineage {
            dispatched_by: Some("none".into()),
            ..Default::default()
        };
        let l = resolve(&Sources::default(), Some("s"), &handed);
        assert_eq!(link_values(&l.chain), vec![vec![("none".into(), H)]]);
        assert!(!l.chain.incomplete);
        let person = HandedLineage {
            dispatched_by: Some("person:dana".into()),
            ..Default::default()
        };
        let l = resolve(&Sources::default(), None, &person);
        assert!(!l.chain.incomplete);
    }

    #[test]
    fn a_handed_value_that_disagrees_is_kept_beside_and_marks_incomplete() {
        let ws = Ws::new();
        ws.entry(
            "c",
            serde_json::json!({"lineage": {"v": 1, "parent": "session:p",
                "source": {"parent": "observed"}}}),
        );
        for by in ["none", "session:q"] {
            let handed = HandedLineage {
                dispatched_by: Some(by.into()),
                ..Default::default()
            };
            let l = resolve(&ws.sources(), Some("c"), &handed);
            assert_eq!(
                link_values(&l.chain),
                vec![vec![("session:p".into(), O), (by.into(), H)]]
            );
            assert!(l.chain.incomplete, "{}", by);
            assert_eq!(l.chain.links[0][0].source, O, "handed merged into observed");
        }
    }

    #[test]
    fn an_agreeing_handed_chain_completes_an_entry_that_stops_short() {
        let ws = Ws::new();
        ws.entry("c", serde_json::json!({"parent_session_id": "p"}));
        let handed = HandedLineage {
            dispatched_by: Some("session:p".into()),
            chain_file: Some(ChainFile {
                v: 1,
                parent: None,
                chain: vec![ChainFileLink {
                    session: "p".into(),
                    dispatch_id: None,
                    parent: "none".into(),
                }],
            }),
            ..Default::default()
        };
        let l = resolve(&ws.sources(), Some("c"), &handed);
        assert_eq!(
            link_values(&l.chain),
            vec![vec![("session:p".into(), H)], vec![("none".into(), H)]]
        );
        assert!(!l.chain.incomplete);
    }

    #[test]
    fn differing_dispatch_ids_and_digests_are_both_kept() {
        let ws = Ws::new();
        ws.entry("c", serde_json::json!({"dispatch_id": "disp-1"}));
        let handed = HandedLineage {
            dispatched_by: Some("none".into()),
            dispatch_id: Some("disp-2".into()),
            ..Default::default()
        };
        let l = resolve(&ws.sources(), Some("c"), &handed);
        assert_eq!(
            l.authority.dispatch_id,
            vec![Sourced::handed("disp-1"), Sourced::handed("disp-2")]
        );
        assert!(l.chain.incomplete);
    }

    #[test]
    fn unsafe_ids_build_no_path_and_the_walk_is_bounded() {
        let ws = Ws::new();
        assert_eq!(ws.sources().map_entry("../escape"), None);
        assert_eq!(ws.sources().map_entry("a/b"), None);
        // A parent with a slash ends the walk.
        ws.entry("c", serde_json::json!({"parent_session_id": "../x"}));
        let l = resolve(&ws.sources(), Some("c"), &HandedLineage::default());
        assert!(l.chain.links.is_empty());
        assert!(l.chain.incomplete);
        // A cycle stops at the first repeat.
        ws.entry("x", serde_json::json!({"parent_session_id": "y"}));
        ws.entry("y", serde_json::json!({"parent_session_id": "x"}));
        let l = resolve(&ws.sources(), Some("x"), &HandedLineage::default());
        assert_eq!(l.chain.links.len(), 1);
        assert!(l.chain.incomplete);
        // A long line of ancestors stops at the cap.
        for i in 0..(MAX_CHAIN_DEPTH + 5) {
            ws.entry(
                &format!("n{}", i),
                serde_json::json!({"parent_session_id": format!("n{}", i + 1)}),
            );
        }
        let l = resolve(&ws.sources(), Some("n0"), &HandedLineage::default());
        assert_eq!(l.chain.links.len(), MAX_CHAIN_DEPTH);
        assert!(l.chain.incomplete);
    }

    #[test]
    fn an_entry_others_can_write_or_a_symlink_is_absent() {
        let ws = Ws::new();
        ws.entry("c", serde_json::json!({"dispatch_id": "disp-1"}));
        let p = ws.dir.path().join(".niwa/sessions/c.json");
        assert!(ws.sources().map_entry("c").is_some());
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o622)).unwrap();
        assert_eq!(ws.sources().map_entry("c"), None);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&p, ws.dir.path().join(".niwa/sessions/l.json")).unwrap();
        assert_eq!(ws.sources().map_entry("l"), None);
        std::fs::create_dir(ws.dir.path().join(".niwa/sessions/d.json")).unwrap();
        assert_eq!(ws.sources().map_entry("d"), None);
    }

    #[test]
    fn nothing_found_records_empty_slots_and_an_incomplete_chain() {
        let l = resolve(&Sources::default(), None, &HandedLineage::default());
        assert!(l.authority.dispatch_id.is_empty());
        assert!(l.authority.instructions_digest.is_empty());
        assert!(l.authority.dispatcher.is_empty());
        assert_eq!(l.chain, Chain::unknown());
        let p = resolve_lineage(
            DeciderKind::Person,
            None,
            Vec::new(),
            &HandedLineage::default(),
            &Sources::default(),
        );
        assert_eq!(p.authority.kind, "person");
        let c = resolve_lineage(
            DeciderKind::Check,
            None,
            Vec::new(),
            &HandedLineage::default(),
            &Sources::default(),
        );
        assert_eq!(c.authority.kind, "dispatch");
    }

    #[test]
    fn the_journal_stamp_stands_in_for_a_removed_entry() {
        let journal = tempfile::tempdir().unwrap();
        let path = journal.path().join(crate::run_journal::JOURNAL_FILE);
        let lines = [
            r#"{"kind":"session_started","koto.driver.session.id":"drv","koto.lineage.dispatch.id":"old","koto.lineage.dispatch.id.source":"handed"}"#,
            r#"{"kind":"state_entered","koto.driver.session.id":"drv"}"#,
            r#"{"kind":"driver_seen","koto.driver.session.id":"drv","koto.lineage.dispatch.id":"disp-9","koto.lineage.dispatch.id.source":"handed","koto.lineage.parent":"session:coord","koto.lineage.parent.source":"handed"}"#,
            r#"{"kind":"driver_seen","koto.driver.session.id":"other","koto.lineage.dispatch.id":"nope"}"#,
        ];
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let sources = Sources {
            workspace_root: None,
            journal_root: Some(journal.path().to_path_buf()),
        };
        let l = resolve(&sources, Some("drv"), &HandedLineage::default());
        assert_eq!(l.authority.dispatch_id, vec![Sourced::handed("disp-9")]);
        assert_eq!(
            link_values(&l.chain),
            vec![vec![("session:coord".into(), H)]]
        );
        assert!(l.chain.incomplete);
    }

    #[test]
    fn the_driver_stamp_reads_the_map_from_the_search_start() {
        let ws = Ws::new();
        ws.entry(
            "drv",
            serde_json::json!({"dispatch_id": "disp-1", "parent_session_id": "coord"}),
        );
        set_search_start_for_test(Some(ws.dir.path()));
        let stamp = driver_stamp("drv");
        set_search_start_for_test(None);
        assert_eq!(
            stamp,
            vec![
                (stamp_fields::DISPATCH_ID, "disp-1".to_string()),
                (stamp_fields::DISPATCH_ID_SOURCE, "handed".to_string()),
                (stamp_fields::PARENT, "session:coord".to_string()),
                (stamp_fields::PARENT_SOURCE, "handed".to_string()),
            ]
        );
        assert!(driver_stamp("drv").is_empty(), "no search start, no stamp");
    }

    #[test]
    fn chain_files_are_strict_versioned_and_regular() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, body: &str| {
            let p = dir.path().join(name);
            std::fs::write(&p, body).unwrap();
            p
        };
        let good = write(
            "good.json",
            r#"{"v":1,"parent":"session:p","chain":[{"session":"p","parent":"none"}]}"#,
        );
        let f = read_chain_file(&good).unwrap();
        assert_eq!(f.parent.as_deref(), Some("session:p"));
        for (name, body) in [
            ("ver.json", r#"{"v":2}"#),
            ("extra.json", r#"{"v":1,"surprise":true}"#),
            (
                "badlink.json",
                r#"{"v":1,"chain":[{"session":"../p","parent":"none"}]}"#,
            ),
            ("badparent.json", r#"{"v":1,"parent":"bob"}"#),
            ("notjson.json", "nope"),
        ] {
            let p = write(name, body);
            assert_eq!(
                read_chain_file(&p).unwrap_err().field,
                "chain_file",
                "{}",
                name
            );
        }
        let big = write("big.json", &" ".repeat(MAX_CHAIN_FILE_BYTES as usize + 1));
        assert!(read_chain_file(&big).is_err());
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&good, &link).unwrap();
        assert!(read_chain_file(&link).is_err());
        assert!(read_chain_file(dir.path()).is_err());
    }

    #[test]
    fn handed_flags_are_validated() {
        assert!(validate_dispatched_by("none").is_ok());
        assert!(validate_dispatched_by("session:abc").is_ok());
        assert!(validate_dispatched_by("person:dana").is_ok());
        for bad in ["unknown", "session:../x", "abc", ""] {
            assert_eq!(
                validate_dispatched_by(bad).unwrap_err().field,
                "dispatched_by"
            );
        }
        assert!(validate_digest(&format!("sha256:{}", "0f".repeat(32))).is_ok());
        assert!(validate_digest("sha256:XYZ").is_err());
        assert!(validate_dispatch_id("disp-1").is_ok());
        assert!(validate_dispatch_id("a/b").is_err());
    }
}
