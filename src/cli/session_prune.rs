//! `koto session prune`: remove finished sessions as a previewed set.
//!
//! Without `--apply` the verb prints what it would remove, what it keeps and
//! why, and a plan token, and stores the would-remove list under
//! `<sessions base>/.prune-plans/<token>.json`. `--apply --plan <token>`
//! works the set out again and refuses when it would remove anything the
//! plan didn't name; what the plan named and the rules now keep is skipped
//! and listed. `--apply` alone (with `--execution-dir`, `--older-than` or
//! `--all`) removes the set it works out now. Both modes run the same rules.
//!
//! Every session is judged by the retention engine
//! ([`crate::session::hierarchy`]) over one scan of the store, with one set
//! of retention settings and one `now`. The rules judge every local session
//! that passes the filters, in order, and the first that applies keeps it:
//!
//! - p: the session holds a pin, readable or not. A pin keeps its own
//!   session and nothing else, so the rest of its hierarchy is judged by the
//!   rules below with the pinned session counted as an ordinary member. The
//!   entry carries `kept_by_pin` and `pin` (or `pin_unreadable`), and the pin
//!   is read again from disk under the session's lock just before a delete.
//! - a: the session is alive by the engine: a koto session that runs or
//!   can't be read, or an anchor that is open.
//! - h: its hierarchy is kept: a live member, the retention not yet passed
//!   since the hierarchy's release instant, or something unreadable. The
//!   detail is the rendered verdict, and the entry carries the JSON
//!   `koto session hierarchy` prints as `hierarchy`. A close session whose
//!   anchor stays in the store is kept by h too: it goes only with its
//!   anchor.
//! - d: its `chain/parent` context key is malformed.
//! - e: it is bound to a request leg that is still waiting, or whose
//!   pointer or request can't be read.
//! - f: it is a coordinator's session (`coordinate-`).
//! - g: under the cloud backend, its remote copy may be newer.
//!
//! The filters narrow which sessions are candidates, never which count as
//! members of a hierarchy, and `--older-than` measures a session's age from
//! the stop instant the engine reports for it. Everything here reads the
//! local store, except rule g and the removal of a session's remote copy.
//!
//! `--migrated-from <workspace>` switches to marker mode (see [`markers`]),
//! which removes migration markers and the old copies beside them from the
//! bucket instead, with the same preview, token and stored-plan contract.

mod markers;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::exit_with_error_code;
use crate::cli::session_inventory::{
    inspect_full, normalize_filter, under_dir, HeaderFields, Inspection, SessionStatus,
    TemplateCache,
};
use crate::session::cloud::RemoteCopy;
use crate::session::finished::{self, FinishedRecord};
use crate::session::hierarchy::{
    self, Graph, Judge, Judgment, MemberKind, RetentionSettings, Scope, Verdict as Hierarchy,
};
use crate::session::links::LinkKind;
use crate::session::pin::PinFact;
use crate::session::validate::validate_session_id;
use crate::session::{Backend, SessionBackend, SessionError, SessionLock};

/// The directory under the sessions base where previews store their plans.
pub const PLANS_DIR: &str = ".prune-plans";

/// A stored plan older than this is removed by the next preview.
const PLAN_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// The exit code for a plan that is stale or unknown. No other koto verb
/// uses it.
const EXIT_PLAN_REFUSED: i32 = 4;

/// The exit code when a removal failed, or the store couldn't be judged.
const EXIT_REMOVAL_FAILED: i32 = 3;

/// Where this prune reads pins: the local store.
const PIN_SOURCE: &str = "local";

/// The kinds of entry the would-remove list holds.
const KIND_SESSION: &str = "session";
const KIND_RECORD: &str = "record";

/// The largest `chain/parent` value read; a session id is far shorter.
const MAX_CHAIN_PARENT_BYTES: u64 = 1024;

/// What `koto session prune` was asked to do.
pub struct PruneArgs<'a> {
    pub execution_dir: Option<&'a str>,
    pub older_than: Option<&'a str>,
    pub all: bool,
    pub apply: bool,
    pub plan: Option<&'a str>,
    /// Marker mode: the source workspace whose migration markers to judge.
    pub migrated_from: Option<&'a str>,
    /// Marker mode: the source host is gone, so a present target's marker is
    /// removable inside the retention bound too.
    pub source_retired: bool,
    /// Marker mode: `session.cloud.marker_retain_days`.
    pub marker_retain_days: u64,
}

/// One session the prune would remove, as the preview prints it and the
/// stored plan keeps it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoveEntry {
    pub kind: String,
    pub name: String,
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<SessionStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default)]
    pub failure: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_dir: Option<PathBuf>,
    /// `anchor` or `anchor_close` for a session that isn't a workflow
    /// session; absent for one that is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_kind: Option<String>,
    /// When the session stopped, as the retention engine reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_at: Option<String>,
}

impl RemoveEntry {
    /// The line this entry contributes to the plan token.
    fn token_line(&self) -> String {
        format!(
            "{}\t{}\t{}\n",
            self.kind,
            self.name,
            self.session_id.as_deref().unwrap_or("")
        )
    }

    /// What identifies the entry when two plans are compared.
    fn key(&self) -> (String, String, Option<String>) {
        (
            self.kind.clone(),
            self.name.clone(),
            self.session_id.clone(),
        )
    }

    fn brief(&self) -> serde_json::Value {
        json!({"kind": self.kind, "name": self.name, "session_id": self.session_id})
    }

    fn is_close_session(&self) -> bool {
        self.session_kind.as_deref() == Some(MemberKind::AnchorClose.as_str())
    }

    fn is_anchor(&self) -> bool {
        self.session_kind.as_deref() == Some(MemberKind::Anchor.as_str())
    }
}

/// One session a rule keeps.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct KeptEntry {
    pub name: String,
    pub rule: char,
    pub detail: String,
    /// For rules a and h: the JSON `koto session hierarchy <name> --json`
    /// prints for the same judgment, less the blocks only the query prints
    /// (`hierarchy::query_json`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hierarchy: Option<serde_json::Value>,
    /// For rule p: `kept_by_pin` and `pin` or `pin_unreadable`. Empty, and
    /// absent from the JSON, for every other entry.
    #[serde(flatten)]
    pub pin: serde_json::Map<String, serde_json::Value>,
}

/// The `mode` a session-mode stored plan records. Marker plans record
/// [`markers::MODE`] and live under another file name.
const SESSIONS_MODE: &str = "sessions";

fn sessions_mode() -> String {
    SESSIONS_MODE.to_string()
}

/// A stored plan: the would-remove list a preview printed.
#[derive(Debug, Serialize, Deserialize)]
struct StoredPlan {
    /// Always [`SESSIONS_MODE`]; plans stored before marker mode existed
    /// have none and read as session plans.
    #[serde(default = "sessions_mode")]
    mode: String,
    plan: String,
    created_at: String,
    /// The filters the preview ran with; `--apply --plan` runs with them.
    #[serde(default)]
    filters: PlanFilters,
    remove: Vec<RemoveEntry>,
}

/// The filters a plan was previewed with, as given (the directory
/// normalized).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PlanFilters {
    #[serde(default)]
    execution_dir: Option<PathBuf>,
    #[serde(default)]
    older_than: Option<String>,
}

/// What a session's `chain/parent` context key holds.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ChainParent {
    Absent,
    Names(String),
    Malformed(String),
}

/// One local session as the inventory read it.
struct Session {
    seen: Inspection,
    header: Option<HeaderFields>,
    chain_parent: ChainParent,
}

/// Every local session the graph scan found, inspected once, and the
/// finished records.
struct Inventory {
    base: PathBuf,
    sessions: BTreeMap<String, Session>,
    /// The finished records of sessions koto removed itself.
    records: Vec<FinishedRecord>,
}

impl Inventory {
    fn read(graph: &Graph) -> Self {
        let base = graph.base().to_path_buf();
        let mut cache = TemplateCache::new();
        let mut sessions = BTreeMap::new();
        for name in graph.names() {
            let (seen, header) = inspect_full(&base, name, &mut cache);
            let chain_parent = read_chain_parent(&base.join(name));
            sessions.insert(
                name.to_string(),
                Session {
                    seen,
                    header,
                    chain_parent,
                },
            );
        }
        // A records directory that can't be read leaves the records out; the
        // sessions are judged all the same.
        let records = finished::list(&base).unwrap_or_else(|e| {
            eprintln!("warning: finished records can't be read: {e:#}");
            Vec::new()
        });
        Inventory {
            base,
            sessions,
            records,
        }
    }
}

/// Read `chain/parent` from a session directory's local context. It is
/// well formed only as a regular file holding exactly one session id, with
/// nothing around it.
fn read_chain_parent(session_dir: &Path) -> ChainParent {
    let path = session_dir.join("ctx").join("chain").join("parent");
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ChainParent::Absent,
        Err(e) => return ChainParent::Malformed(format!("it can't be read: {e}")),
    };
    if !meta.file_type().is_file() {
        return ChainParent::Malformed("it isn't a regular file".to_string());
    }
    if meta.len() > MAX_CHAIN_PARENT_BYTES {
        return ChainParent::Malformed("it is too long to be a session name".to_string());
    }
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) => return ChainParent::Malformed(format!("it can't be read: {e}")),
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return ChainParent::Malformed("it isn't UTF-8".to_string());
    };
    if text.trim() != text || validate_session_id(&text).is_err() {
        return ChainParent::Malformed("it isn't exactly one session name".to_string());
    }
    ChainParent::Names(text)
}

/// What one prune run judges with: one scan of the store, one set of
/// retention settings and one `now`.
struct Engine<'g> {
    graph: &'g Graph,
    settings: RetentionSettings,
    now: SystemTime,
}

impl Engine<'_> {
    /// A judge over this run's scan. A fresh one reads every member it is
    /// asked about again.
    fn judge(&self) -> Judge<'_> {
        Judge::new(self.graph, self.settings.clone(), self.now)
    }
}

/// The first rule that keeps session `name`, given its hierarchy's
/// judgment.
fn first_rule(
    name: &str,
    session: &Session,
    judgment: &Judgment,
    settings: &RetentionSettings,
    base: &Path,
    backend: &Backend,
) -> Option<KeptEntry> {
    // Rule p: the session's own pin, before anything else; it keeps this
    // session only, so it never reaches the hierarchy rules of the others.
    // A hierarchy kept besides (a live anchor, the retention not yet
    // passed) rides along, so the entry names the pin and the keeper.
    if let Some(detail) = hierarchy::pin_text(judgment.pin_of(name)) {
        return Some(KeptEntry {
            name: name.to_string(),
            rule: 'p',
            detail,
            hierarchy: judgment
                .verdict
                .is_kept()
                .then(|| hierarchy::to_json(judgment, settings)),
            pin: hierarchy::pin_fields(judgment.pin_of(name), PIN_SOURCE),
        });
    }
    if let Hierarchy::Keep { keepers, .. } = &judgment.verdict {
        let hierarchy = Some(hierarchy::to_json(judgment, settings));
        if let Some(own) = keepers.iter().find(|k| k.name == name) {
            let detail = match own.kind {
                MemberKind::KotoSession if session.seen.status == SessionStatus::Running => {
                    "the session is running".to_string()
                }
                MemberKind::KotoSession => "the session can't be read".to_string(),
                _ => hierarchy::keeper_line(own),
            };
            return Some(KeptEntry {
                name: name.to_string(),
                rule: 'a',
                detail,
                hierarchy,
                pin: Default::default(),
            });
        }
        return Some(KeptEntry {
            name: name.to_string(),
            rule: 'h',
            detail: hierarchy::reason_text(&judgment.verdict),
            hierarchy,
            pin: Default::default(),
        });
    }
    later_rule(name, &session.chain_parent, base, backend).map(|(rule, detail)| KeptEntry {
        name: name.to_string(),
        rule,
        detail,
        hierarchy: None,
        pin: Default::default(),
    })
}

/// Rules d to g: what keeps a session whose hierarchy is released.
fn later_rule(
    name: &str,
    chain_parent: &ChainParent,
    base: &Path,
    backend: &Backend,
) -> Option<(char, String)> {
    if let ChainParent::Malformed(why) = chain_parent {
        return Some((
            'd',
            format!("its chain/parent context key is malformed: {why}"),
        ));
    }
    if let Some(detail) = leg_keeps(&base.join(name)) {
        return Some(('e', detail));
    }
    if name.starts_with("coordinate-") {
        return Some(('f', "a coordinator's session".to_string()));
    }
    if let Backend::Cloud(cloud) = backend {
        match cloud.remote_copy_check(name) {
            RemoteCopy::Safe => {}
            RemoteCopy::NotSafe(detail) => return Some(('g', detail)),
            RemoteCopy::Unreadable(why) => {
                return Some(('g', format!("the remote copy can't be checked: {why}")))
            }
        }
    }
    None
}

/// Rule e: why a session's request leg keeps it, if it does. A session with
/// no pointer, or bound to a request that no longer exists, isn't kept.
fn leg_keeps(session_dir: &Path) -> Option<String> {
    use crate::engine::request_store::{self, RequestStoreError, ValidatedRequestId};
    use crate::engine::types::{LegDisposition, RequestState};

    let pointer = match crate::engine::leg_pointer::read_pointer(session_dir) {
        Ok(None) => return None,
        Ok(Some(pointer)) => pointer,
        Err(e) => return Some(format!("its request-leg pointer can't be read: {e}")),
    };
    let id = match ValidatedRequestId::new(&pointer.request_id) {
        Ok(id) => id,
        Err(e) => {
            return Some(format!(
                "its request-leg pointer names no usable request: {e}"
            ))
        }
    };
    let Some(root) = crate::koto_home::koto_home() else {
        return Some("the request store can't be found (no koto root)".to_string());
    };
    let view = match request_store::read_view(&root, &id) {
        Ok(view) => view,
        Err(RequestStoreError::NotFound { .. }) => return None,
        Err(e) => {
            return Some(format!(
                "request '{}' can't be read: {e}",
                pointer.request_id
            ))
        }
    };
    let Some(leg) = view.legs.get(&pointer.leg_name) else {
        return Some(format!(
            "request '{}' has no leg '{}'",
            pointer.request_id, pointer.leg_name
        ));
    };
    if view.request_state == RequestState::Open && leg.disposition == LegDisposition::Open {
        return Some(format!(
            "bound to leg '{}' of request '{}', which is still waiting",
            pointer.leg_name, pointer.request_id
        ));
    }
    None
}

/// The filters a session must pass to be a candidate at all. They never
/// change which sessions count as members of a hierarchy.
struct Filters {
    execution_dir: Option<PathBuf>,
    older_than: Option<Duration>,
}

impl Filters {
    fn pass_dir(&self, execution_dir: Option<&Path>) -> bool {
        match &self.execution_dir {
            Some(filter) => execution_dir.is_some_and(|dir| under_dir(dir, filter)),
            None => true,
        }
    }

    /// Whether something that stopped (or, for a finished record, finished)
    /// at `at` is old enough. Nothing that hasn't stopped is.
    fn pass_age(&self, at: Option<&str>, now: SystemTime) -> bool {
        let Some(min_age) = self.older_than else {
            return true;
        };
        let Some(at) = at.and_then(crate::engine::wake::parse_rfc3339_millis) else {
            return false;
        };
        now.duration_since(at).is_ok_and(|age| age >= min_age)
    }
}

/// The verdict over a whole store: what would go, and what stays.
struct Verdict {
    remove: Vec<RemoveEntry>,
    kept: Vec<KeptEntry>,
    /// Every stale anchor a judgment named as a keeper, by name, as the
    /// query lists it (with its close command when one can be built).
    stale: BTreeMap<String, serde_json::Value>,
    token: String,
}

fn judge(inv: &Inventory, engine: &Engine<'_>, filters: &Filters, backend: &Backend) -> Verdict {
    let mut judge = engine.judge();
    let mut remove = Vec::new();
    let mut kept = Vec::new();
    let mut stale = BTreeMap::new();
    let mut hierarchies = BTreeMap::new();
    for (name, session) in &inv.sessions {
        let execution_dir = session
            .header
            .as_ref()
            .and_then(|h| h.execution_dir.as_deref());
        if !filters.pass_dir(execution_dir) {
            continue;
        }
        let judgment = match judge.judge(name, &Scope::Whole) {
            Ok(j) => j,
            Err(e) => {
                kept.push(KeptEntry {
                    name: name.clone(),
                    rule: 'h',
                    detail: format!("kept: its hierarchy can't be judged: {e:#}"),
                    hierarchy: None,
                    pin: Default::default(),
                });
                continue;
            }
        };
        let member = judgment.members.iter().find(|m| &m.name == name);
        let stopped_at = member.and_then(|m| m.stopped_at.clone());
        if !filters.pass_age(stopped_at.as_deref(), engine.now) {
            continue;
        }
        if let Hierarchy::Keep { keepers, .. } = &judgment.verdict {
            for k in keepers.iter().filter(|k| k.stale) {
                stale
                    .entry(k.name.clone())
                    .or_insert_with(|| hierarchy::keeper_json(k));
            }
        }
        // A released close session may still be kept below for its anchor;
        // its entry carries the judgment then.
        if member.is_some_and(|m| m.kind == MemberKind::AnchorClose) && !judgment.verdict.is_kept()
        {
            hierarchies.insert(
                name.clone(),
                hierarchy::to_json(&judgment, &engine.settings),
            );
        }
        match first_rule(
            name,
            session,
            &judgment,
            &engine.settings,
            &inv.base,
            backend,
        ) {
            Some(entry) => kept.push(entry),
            None => remove.push(RemoveEntry {
                kind: KIND_SESSION.to_string(),
                name: name.clone(),
                session_id: session.header.as_ref().and_then(|h| h.session_id.clone()),
                status: Some(session.seen.status),
                state: session.seen.state.clone(),
                failure: session.seen.failure,
                finished_at: session.seen.finished_at.clone(),
                execution_dir: session
                    .header
                    .as_ref()
                    .and_then(|h| h.execution_dir.clone()),
                session_kind: member
                    .map(|m| m.kind)
                    .filter(|k| *k != MemberKind::KotoSession)
                    .map(|k| k.as_str().to_string()),
                stopped_at,
            }),
        }
    }
    // A close session goes only with its anchor: one whose anchor stays in
    // the store (kept, or left out by a filter) is kept with it, or the
    // anchor would read open again.
    let removing: BTreeSet<String> = remove.iter().map(|e| e.name.clone()).collect();
    remove.retain(|e| {
        if !e.is_close_session() {
            return true;
        }
        match closed_anchor(engine.graph, &e.name) {
            Some(anchor) if engine.graph.contains(&anchor) && !removing.contains(&anchor) => {
                kept.push(KeptEntry {
                    name: e.name.clone(),
                    rule: 'h',
                    detail: format!(
                        "kept: a close session goes only with its anchor, and {} stays",
                        hierarchy::sanitize(&anchor)
                    ),
                    hierarchy: hierarchies.get(&e.name).cloned(),
                    pin: Default::default(),
                });
                false
            }
            _ => true,
        }
    });
    // Finished records have no rules: koto writes one only when it removes a
    // session itself, after the session's parent had its result, so nothing
    // still needs a record to learn how a session ended.
    for record in &inv.records {
        if !filters.pass_dir(record.execution_dir.as_deref())
            || !filters.pass_age(Some(record.finished_at.as_str()), engine.now)
        {
            continue;
        }
        remove.push(RemoveEntry {
            kind: KIND_RECORD.to_string(),
            name: record.name.clone(),
            session_id: record.session_id.clone(),
            status: Some(if record.status == finished::STATUS_CANCELLED {
                SessionStatus::Cancelled
            } else {
                SessionStatus::Terminal
            }),
            state: Some(record.state.clone()),
            failure: record.failure,
            finished_at: Some(record.finished_at.clone()),
            execution_dir: record.execution_dir.clone(),
            session_kind: None,
            stopped_at: None,
        });
    }
    let token = plan_token(&remove);
    Verdict {
        remove,
        kept,
        stale,
        token,
    }
}

/// The anchor close session `name` closes: the peer of its `closes` link.
fn closed_anchor(graph: &Graph, name: &str) -> Option<String> {
    graph.facts(name).and_then(|f| {
        f.links
            .iter()
            .find(|l| l.kind == LinkKind::Closes.as_str())
            .map(|l| l.peer.clone())
    })
}

/// The first 32 hex digits of SHA-256 over the sorted token lines.
fn plan_token(remove: &[RemoveEntry]) -> String {
    token_over(remove.iter().map(RemoveEntry::token_line).collect())
}

/// The first 32 hex digits of SHA-256 over `lines`, sorted.
fn token_over(mut lines: Vec<String>) -> String {
    use sha2::{Digest, Sha256};
    lines.sort();
    let mut hasher = Sha256::new();
    for line in &lines {
        hasher.update(line.as_bytes());
    }
    hex::encode(hasher.finalize())[..32].to_string()
}

/// Whether `token` has a plan token's shape.
fn is_token(token: &str) -> bool {
    token.len() == 32
        && token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

// `--older-than`: a whole number followed by `s`, `m`, `h` or `d`.
use crate::session::duration::parse_age;

fn plan_unknown(token: &str) -> ! {
    exit_with_error_code(
        json!({"error": {
            "code": "prune_plan_unknown",
            "message": format!(
                "no stored plan {token}: it expired, was applied already, or was \
                 previewed on another host; preview again"
            ),
        }}),
        EXIT_PLAN_REFUSED,
    )
}

fn usage(message: impl Into<String>) -> ! {
    exit_with_error_code(
        json!({"error": {"code": "invalid_arguments", "message": message.into()}}),
        2,
    )
}

/// Handle `koto session prune`.
pub fn handle_prune(backend: &Backend, args: PruneArgs<'_>) -> Result<()> {
    if args.migrated_from.is_some() {
        return markers::handle(backend, &args);
    }
    if args.source_retired {
        usage("--source-retired applies to migration markers; pass it with --migrated-from");
    }
    let filtered = args.execution_dir.is_some() || args.older_than.is_some() || args.all;
    if args.plan.is_some() && !args.apply {
        usage("--plan applies a previewed plan; pass it with --apply");
    }
    if let Some(token) = args.plan {
        if !is_token(token) {
            usage(format!(
                "--plan {token:?} isn't a plan token (32 lowercase hex digits)"
            ));
        }
        if filtered {
            usage(
                "--plan applies the filters its preview ran with; pass no \
                 --execution-dir, --older-than or --all with it",
            );
        }
    }
    if args.apply && args.plan.is_none() && !filtered {
        usage(
            "--apply without --plan removes every finished session it selects; \
             name the set with --execution-dir, --older-than or --all",
        );
    }

    let base = backend.local_base_dir().to_path_buf();
    let stored = match args.plan {
        Some(token) => Some(read_plan(&base, token)?.unwrap_or_else(|| plan_unknown(token))),
        None => None,
    };
    let plan_filters = match &stored {
        Some(plan) => plan.filters.clone(),
        None => PlanFilters {
            execution_dir: args.execution_dir.map(normalize_filter),
            older_than: args.older_than.map(str::to_string),
        },
    };
    let older_than = plan_filters.older_than.as_deref().map(|arg| {
        parse_age(arg).unwrap_or_else(|| {
            usage(format!(
                "--older-than {arg:?} isn't a duration such as 30d, 12h, 45m or 90s"
            ))
        })
    });
    let filters = Filters {
        execution_dir: plan_filters.execution_dir.clone(),
        older_than,
    };

    let settings = RetentionSettings::load().unwrap_or_else(|e| {
        exit_with_error_code(
            json!({"error": {
                "code": "invalid_retention_setting",
                "message": format!("{e:#}; nothing was judged or removed"),
            }}),
            2,
        )
    });
    // One scan for the whole run, preview or apply. A store that can't be
    // listed can't be judged, so nothing is removed.
    let graph = Graph::scan(&base).unwrap_or_else(|e| {
        exit_with_error_code(
            json!({"error": {
                "code": "prune_scan_failed",
                "message": format!(
                    "the sessions directory can't be read, so nothing was judged or \
                     removed: {e:#}"
                ),
            }}),
            EXIT_REMOVAL_FAILED,
        )
    });
    let engine = Engine {
        graph: &graph,
        settings,
        now: SystemTime::now(),
    };
    let inv = Inventory::read(&graph);
    let mut verdict = judge(&inv, &engine, &filters, backend);
    let stale: Vec<_> = verdict.stale.values().cloned().collect();

    if !args.apply {
        store_plan(&base, &verdict, &plan_filters)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "mode": "preview",
                "plan": verdict.token,
                "remove": verdict.remove,
                "kept": verdict.kept,
                "stale_anchors": stale,
            }))?
        );
        return Ok(());
    }

    let mut outcome = Outcome::default();
    if let (Some(token), Some(stored)) = (args.plan, &stored) {
        let named: BTreeSet<_> = stored.remove.iter().map(RemoveEntry::key).collect();
        let now: BTreeSet<_> = verdict.remove.iter().map(RemoveEntry::key).collect();
        // Refused only when the fresh judgment would remove something the
        // plan didn't name; one that only keeps more than the plan goes on,
        // and lists what it now keeps as skipped.
        let added: Vec<_> = verdict
            .remove
            .iter()
            .filter(|e| !named.contains(&e.key()))
            .map(RemoveEntry::brief)
            .collect();
        if !added.is_empty() {
            let dropped: Vec<_> = stored
                .remove
                .iter()
                .filter(|e| !now.contains(&e.key()))
                .map(RemoveEntry::brief)
                .collect();
            exit_with_error_code(
                json!({"error": {
                    "code": "prune_plan_stale",
                    "message": format!(
                        "the prune would now remove sessions plan {token} didn't name; \
                         nothing was removed"
                    ),
                    "plan": token,
                    "current": verdict.token,
                    "added": added,
                    "dropped": dropped,
                }}),
                EXIT_PLAN_REFUSED,
            );
        }
        for entry in stored.remove.iter().filter(|e| !now.contains(&e.key())) {
            let mut skipped = json!({
                "name": entry.name,
                "reason": no_longer_removed(entry, &verdict, &graph),
            });
            // A session pinned since the preview carries its pin here too.
            if entry.kind == KIND_SESSION {
                if let Some(k) = verdict.kept.iter().find(|k| k.name == entry.name) {
                    for (field, value) in &k.pin {
                        skipped[field] = value.clone();
                    }
                }
            }
            outcome.skipped.push(skipped);
        }
        // Never remove what the preview didn't name, whatever the hash says.
        verdict.remove.retain(|e| named.contains(&e.key()));
    }

    apply(backend, &inv, &engine, &verdict, &mut outcome);
    if let Some(token) = args.plan {
        if outcome.failed.is_empty() {
            remove_plan(&base, token);
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "mode": "apply",
            "plan": verdict.token,
            "removed": outcome.removed,
            "skipped": outcome.skipped,
            "kept": verdict.kept,
            "failed": outcome.failed,
            "stale_anchors": stale,
        }))?
    );
    if !outcome.failed.is_empty() {
        std::process::exit(EXIT_REMOVAL_FAILED);
    }
    Ok(())
}

/// Why an entry a plan named isn't removed now.
fn no_longer_removed(entry: &RemoveEntry, verdict: &Verdict, graph: &Graph) -> String {
    if entry.kind == KIND_SESSION {
        if let Some(k) = verdict.kept.iter().find(|k| k.name == entry.name) {
            return format!("now kept by rule {}: {}", k.rule, k.detail);
        }
        if !graph.contains(&entry.name) {
            return "the session is already gone".to_string();
        }
        return "a different session now has this name, or it no longer passes the plan's \
                filters"
            .to_string();
    }
    "the record is gone or was replaced since the preview".to_string()
}

#[derive(Default)]
struct Outcome {
    removed: Vec<RemoveEntry>,
    skipped: Vec<serde_json::Value>,
    failed: Vec<serde_json::Value>,
}

/// Remove the verdict's entries: sessions one hierarchy at a time, each
/// hierarchy judged again as a unit just before its members go, then the
/// finished records.
fn apply(
    backend: &Backend,
    inv: &Inventory,
    engine: &Engine<'_>,
    verdict: &Verdict,
    outcome: &mut Outcome,
) {
    let mut grouping = engine.judge();
    let mut components: BTreeMap<String, Vec<&RemoveEntry>> = BTreeMap::new();
    let mut others = Vec::new();
    for entry in &verdict.remove {
        if entry.kind != KIND_SESSION {
            others.push(entry);
            continue;
        }
        let key = grouping
            .component(&entry.name, &Scope::Whole)
            .and_then(|(present, _)| present.into_iter().next())
            .unwrap_or_else(|| entry.name.clone());
        components.entry(key).or_default().push(entry);
    }
    for entries in components.values() {
        apply_component(backend, inv, engine, entries, outcome);
    }
    for entry in others {
        // Everything below a session entry removes a session directory.
        // Another kind of entry needs its own removal, never that one.
        let result = if entry.kind == KIND_RECORD {
            remove_record(&inv.base, entry)
        } else {
            Err(Refusal::Skipped(format!(
                "this koto can't remove a {:?} entry",
                entry.kind
            )))
        };
        record_result(outcome, entry, result);
    }
}

fn record_result(
    outcome: &mut Outcome,
    entry: &RemoveEntry,
    result: std::result::Result<(), Refusal>,
) {
    match result {
        Ok(()) => outcome.removed.push(entry.clone()),
        Err(Refusal::Skipped(reason)) => outcome
            .skipped
            .push(json!({"name": entry.name, "reason": reason})),
        Err(Refusal::Pinned(pin)) => {
            let mut skipped = json!({
                "name": entry.name,
                "reason": format!(
                    "now kept by rule p: {}",
                    hierarchy::pin_text(&pin).unwrap_or_default()
                ),
            });
            for (k, v) in hierarchy::pin_fields(&pin, PIN_SOURCE) {
                skipped[k] = v;
            }
            outcome.skipped.push(skipped);
        }
        Err(Refusal::Failed(error)) => outcome
            .failed
            .push(json!({"name": entry.name, "error": error})),
    }
}

/// Remove the entries of one hierarchy. Every member is locked first, so
/// no koto command ticks one while it is judged; then the hierarchy is
/// judged again as a unit, reading each member afresh, and only a released
/// hierarchy loses members. Children go before their parents, anchors
/// before their close sessions, and a close session whose anchor is still
/// there afterwards stays.
fn apply_component(
    backend: &Backend,
    inv: &Inventory,
    engine: &Engine<'_>,
    entries: &[&RemoveEntry],
    outcome: &mut Outcome,
) {
    let mut order = entries.to_vec();
    order.sort_by(|a, b| {
        a.is_close_session()
            .cmp(&b.is_close_session())
            .then_with(|| depth(inv, &b.name).cmp(&depth(inv, &a.name)))
            .then_with(|| b.is_anchor().cmp(&a.is_anchor()))
            .then_with(|| a.name.cmp(&b.name))
    });
    let mut locked: Vec<(&RemoveEntry, SessionLock)> = Vec::new();
    for entry in order {
        match lock(backend, &entry.name) {
            Ok(lock) => locked.push((entry, lock)),
            Err(refusal) => record_result(outcome, entry, Err(refusal)),
        }
    }
    let Some((first, _)) = locked.first() else {
        return;
    };
    let mut judge = engine.judge();
    // The component gate asks the component's verdict; a pin never keeps
    // the whole component, only its own member, which is asked below.
    let judgment = match judge.judge(&first.name, &Scope::Whole) {
        Ok(j) if !j.verdict.is_kept() => j,
        Ok(j) => {
            let reason = format!("now kept by rule h: {}", hierarchy::reason_text(&j.verdict));
            for (entry, _) in &locked {
                record_result(outcome, entry, Err(Refusal::Skipped(reason.clone())));
            }
            return;
        }
        Err(e) => {
            let reason = format!("its hierarchy can't be judged again: {e:#}");
            for (entry, _) in &locked {
                record_result(outcome, entry, Err(Refusal::Skipped(reason.clone())));
            }
            return;
        }
    };
    let mut removed = BTreeSet::new();
    let mut cache = TemplateCache::new();
    for (entry, lock) in &locked {
        let result = if judgment.keeps(&entry.name) {
            Err(Refusal::Pinned(judgment.pin_of(&entry.name).clone()))
        } else {
            remove_member(
                backend,
                inv,
                engine.graph,
                entry,
                lock,
                &removed,
                &mut cache,
            )
        };
        if result.is_ok() {
            removed.insert(entry.name.clone());
        }
        record_result(outcome, entry, result);
    }
}

/// How many `parent_workflow` links lie above `name` in the inventory.
/// Removal goes deepest first so a koto child goes before its parent.
fn depth(inv: &Inventory, name: &str) -> usize {
    let mut seen = BTreeSet::new();
    let mut current = name.to_string();
    while let Some(parent) = inv
        .sessions
        .get(&current)
        .and_then(|s| s.header.as_ref())
        .and_then(|h| h.parent_workflow.clone())
    {
        if !seen.insert(parent.clone()) {
            break;
        }
        current = parent;
    }
    seen.len()
}

enum Refusal {
    Skipped(String),
    /// Kept by its own pin, read since the preview: listed as skipped with
    /// the pin's fields.
    Pinned(PinFact),
    Failed(String),
}

fn lock(backend: &Backend, name: &str) -> std::result::Result<SessionLock, Refusal> {
    match backend.lock_state_file(name) {
        Ok(lock) => Ok(lock),
        Err(SessionError::Locked { .. }) => Err(Refusal::Skipped(
            "another koto process holds the session's lock".to_string(),
        )),
        Err(SessionError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(Refusal::Skipped("the session is already gone".to_string()))
        }
        Err(e) => Err(Refusal::Skipped(format!(
            "the session can't be locked: {e}"
        ))),
    }
}

/// Remove one member of a hierarchy just judged released, under its lock
/// (`held`), after checking it is still the session the judgment saw and
/// still stopped. The delete goes through the pin guard, the last thing
/// before it.
fn remove_member(
    backend: &Backend,
    inv: &Inventory,
    graph: &Graph,
    entry: &RemoveEntry,
    held: &SessionLock,
    removed: &BTreeSet<String>,
    cache: &mut TemplateCache,
) -> std::result::Result<(), Refusal> {
    let base = &inv.base;
    let name = entry.name.as_str();
    let (seen, header) = inspect_full(base, name, cache);
    let session_id = header.as_ref().and_then(|h| h.session_id.clone());
    if session_id != entry.session_id {
        return Err(Refusal::Skipped(
            "a different session now has this name".to_string(),
        ));
    }
    // An anchor never reaches a terminal state; the hierarchy's judgment,
    // made just now under the lock, is what says it is closed.
    if entry.session_kind.is_none()
        && !matches!(
            seen.status,
            SessionStatus::Terminal | SessionStatus::Cancelled
        )
    {
        return Err(Refusal::Skipped(format!(
            "the session is no longer finished (now {})",
            status_word(seen.status)
        )));
    }
    if entry.is_close_session() {
        if let Some(anchor) = closed_anchor(graph, name) {
            if base.join(&anchor).exists() && !removed.contains(&anchor) {
                return Err(Refusal::Skipped(format!(
                    "its anchor {} is still there, and a close session goes only after its \
                     anchor",
                    hierarchy::sanitize(&anchor)
                )));
            }
        }
    }
    if let Some((rule, detail)) =
        later_rule(name, &read_chain_parent(&base.join(name)), base, backend)
    {
        return Err(Refusal::Skipped(format!(
            "now kept by rule {rule}: {detail}"
        )));
    }
    // The pin guard, under the session's lock, the last thing before the
    // delete: a pin set since the scan keeps the session.
    let removal = crate::session::pin::remove_guarded_with(backend, name, held, || {
        // The remote copy goes first, so a failure leaves the local copy
        // for a later prune to retry.
        if let Backend::Cloud(cloud) = backend {
            cloud
                .delete_remote_session_checked(name)
                .map_err(|e| Refusal::Failed(format!("the remote copy wasn't removed: {e:#}")))?;
        }
        std::fs::remove_dir_all(base.join(name))
            .map_err(|e| Refusal::Failed(format!("the local directory wasn't removed: {e}")))
    })?;
    match removal {
        crate::session::pin::Removal::Removed => Ok(()),
        crate::session::pin::Removal::Kept(guard) => Err(Refusal::Pinned(guard.fact())),
    }
}

/// Remove a finished record, after reading it again: a record rewritten
/// since the judgment (another session of the name finished and was removed)
/// is a different record, and is skipped.
fn remove_record(base: &Path, entry: &RemoveEntry) -> std::result::Result<(), Refusal> {
    let current = match finished::read(base, &entry.name) {
        Ok(Some(record)) => record,
        Ok(None) => return Err(Refusal::Skipped("the record is already gone".to_string())),
        Err(e) => return Err(Refusal::Skipped(format!("the record can't be read: {e:#}"))),
    };
    if current.session_id != entry.session_id
        || Some(current.finished_at.as_str()) != entry.finished_at.as_deref()
    {
        return Err(Refusal::Skipped(
            "the record was replaced since the preview".to_string(),
        ));
    }
    finished::remove(base, &entry.name)
        .map_err(|e| Refusal::Failed(format!("the record wasn't removed: {e:#}")))
}

fn status_word(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Running => "running",
        SessionStatus::Terminal => "terminal",
        SessionStatus::Cancelled => "cancelled",
        SessionStatus::Unreadable => "unreadable",
    }
}

// ----- Stored plans -----

fn plans_dir(base: &Path) -> PathBuf {
    base.join(PLANS_DIR)
}

/// Store the preview's would-remove list (see [`write_plan_file`]).
fn store_plan(base: &Path, verdict: &Verdict, filters: &PlanFilters) -> Result<()> {
    let plan = StoredPlan {
        mode: sessions_mode(),
        plan: verdict.token.clone(),
        created_at: crate::engine::types::now_iso8601(),
        filters: filters.clone(),
        remove: verdict.remove.clone(),
    };
    write_plan_file(
        base,
        &format!("{}.json", verdict.token),
        &serde_json::to_vec(&plan)?,
    )
}

/// Write `bytes` as stored plan `file_name`, creating the plans directory
/// 0700 and the file 0600, and remove stored plans older than a day. A plans
/// path that is a symlink or not a directory is refused, and so is a plan
/// file that is a symlink.
fn write_plan_file(base: &Path, file_name: &str, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

    let dir = plans_dir(base);
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if meta.file_type().is_dir() => {}
        Ok(_) => anyhow::bail!("{} isn't a directory; not storing the plan", dir.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(base)
                .with_context(|| format!("failed to create {}", base.display()))?;
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&dir)
                .or_else(|e| match e.kind() {
                    std::io::ErrorKind::AlreadyExists => Ok(()),
                    _ => Err(e),
                })
                .with_context(|| format!("failed to create {}", dir.display()))?;
        }
        Err(e) => return Err(e).with_context(|| format!("failed to read {}", dir.display())),
    }
    expire_plans(&dir);

    let path = dir.join(file_name);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .with_context(|| format!("failed to write {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Remove stored plans older than [`PLAN_MAX_AGE`]. Best-effort.
fn expire_plans(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let Ok(meta) = entry.path().symlink_metadata() else {
            continue;
        };
        if !meta.file_type().is_file() {
            continue;
        }
        let old = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age > PLAN_MAX_AGE);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Read stored plan `token`; `None` when there is none, or when the file
/// holds another mode's plan.
fn read_plan(base: &Path, token: &str) -> Result<Option<StoredPlan>> {
    let Some(bytes) = read_plan_file(base, &format!("{token}.json"))? else {
        return Ok(None);
    };
    let plan: StoredPlan = match serde_json::from_slice(&bytes) {
        Ok(plan) => plan,
        Err(_) => return Ok(None),
    };
    if plan.plan != token || plan.mode != SESSIONS_MODE {
        return Ok(None);
    }
    Ok(Some(plan))
}

/// The bytes of stored plan file `file_name`; `None` when there is no
/// regular file by that name.
fn read_plan_file(base: &Path, file_name: &str) -> Result<Option<Vec<u8>>> {
    let path = plans_dir(base).join(file_name);
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_file() => {}
        Ok(_) => return Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
    std::fs::read(&path)
        .map(Some)
        .with_context(|| format!("failed to read {}", path.display()))
}

fn remove_plan(base: &Path, token: &str) {
    remove_plan_file(base, &format!("{token}.json"));
}

fn remove_plan_file(base: &Path, file_name: &str) {
    let _ = std::fs::remove_file(plans_dir(base).join(file_name));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::state_file_name;

    #[test]
    fn ages_parse_whole_units_only() {
        assert_eq!(parse_age("90s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_age("45m"), Some(Duration::from_secs(2700)));
        assert_eq!(parse_age("12h"), Some(Duration::from_secs(43_200)));
        assert_eq!(parse_age("30d"), Some(Duration::from_secs(2_592_000)));
        for bad in [
            "",
            "d",
            "1",
            "1w",
            "-1d",
            "1.5h",
            " 1d",
            "1d ",
            "99999999999999999999d",
        ] {
            assert_eq!(parse_age(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn tokens_are_32_lowercase_hex() {
        assert!(is_token("0123456789abcdef0123456789abcdef"));
        assert!(!is_token("0123456789ABCDEF0123456789abcdef"));
        assert!(!is_token("0123456789abcdef0123456789abcde"));
        assert!(!is_token("../../../../etc/passwd/xxxxxxxxx"));
    }

    fn entry(name: &str, id: &str) -> RemoveEntry {
        RemoveEntry {
            kind: "session".to_string(),
            name: name.to_string(),
            session_id: Some(id.to_string()),
            status: Some(SessionStatus::Terminal),
            state: Some("done".to_string()),
            failure: false,
            finished_at: None,
            execution_dir: None,
            session_kind: None,
            stopped_at: None,
        }
    }

    /// Settings with a one-second retention: the fixtures stopped days ago.
    fn engine(graph: &Graph) -> Engine<'_> {
        Engine {
            graph,
            settings: RetentionSettings::from_durations(
                Duration::from_secs(1),
                Duration::from_secs(3600),
                Duration::from_secs(7200),
            ),
            now: SystemTime::now(),
        }
    }

    fn no_filters() -> Filters {
        Filters {
            execution_dir: None,
            older_than: None,
        }
    }

    fn local(base: &Path) -> Backend {
        Backend::Local(crate::session::local::LocalBackend::with_base_dir(
            base.to_path_buf(),
        ))
    }

    fn applied(backend: &Backend, inv: &Inventory, engine: &Engine<'_>, v: &Verdict) -> Outcome {
        let mut outcome = Outcome::default();
        apply(backend, inv, engine, v, &mut outcome);
        outcome
    }

    #[test]
    fn the_token_covers_names_and_ids_in_any_order() {
        let a = entry("a", "1");
        let b = entry("b", "2");
        let token = plan_token(&[a.clone(), b.clone()]);
        assert_eq!(token.len(), 32);
        assert_eq!(token, plan_token(&[b.clone(), a.clone()]));
        // Other fields don't enter it.
        let mut moved = a.clone();
        moved.finished_at = Some("2026-01-01T00:00:00.000Z".to_string());
        assert_eq!(token, plan_token(&[moved, b.clone()]));
        // A recreated session (new id) does.
        assert_ne!(token, plan_token(&[entry("a", "3"), b]));
    }

    const TEMPLATE: &str = "---\nname: prune-unit\nversion: \"1.0\"\ninitial_state: start\n\
        states:\n  start:\n    transitions:\n      - target: done\n  done:\n    terminal: true\n\
        ---\n\n## start\n\nStart.\n\n## done\n\nDone.\n";

    /// A store under `tmp` with a compiled template, and a writer of
    /// sessions standing in `done`.
    struct Store {
        base: PathBuf,
        compiled: PathBuf,
    }

    impl Store {
        fn new(tmp: &Path) -> Self {
            let md = tmp.join("t.md");
            std::fs::write(&md, TEMPLATE).unwrap();
            let compiled_template = crate::template::compile::compile(&md, false).unwrap();
            let compiled = tmp.join("t.json");
            std::fs::write(&compiled, serde_json::to_vec(&compiled_template).unwrap()).unwrap();
            let base = tmp.join("sessions");
            std::fs::create_dir_all(&base).unwrap();
            Store { base, compiled }
        }

        fn finished(&self, name: &str, session_id: &str) {
            let dir = self.base.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let header = json!({
                "schema_version": 1,
                "workflow": name,
                "template_hash": "h",
                "created_at": "2026-10-12T00:00:00.000Z",
                "session_id": session_id,
            });
            let events = [
                json!({"seq": 1, "timestamp": "2026-10-12T00:00:00.000Z",
                       "type": "workflow_initialized",
                       "payload": {"template_path": self.compiled, "variables": {}}}),
                json!({"seq": 2, "timestamp": "2026-10-12T00:00:00.000Z", "type": "transitioned",
                       "payload": {"from": null, "to": "start", "condition_type": "auto"}}),
                json!({"seq": 3, "timestamp": "2026-10-12T00:00:01.000Z", "type": "transitioned",
                       "payload": {"from": "start", "to": "done", "condition_type": "auto"}}),
            ];
            let mut body = format!("{header}\n");
            for e in events {
                body.push_str(&format!("{e}\n"));
            }
            std::fs::write(dir.join(state_file_name(name)), body).unwrap();
        }

        fn append(&self, name: &str, event: serde_json::Value) {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(self.base.join(name).join(state_file_name(name)))
                .unwrap();
            writeln!(f, "{event}").unwrap();
        }
    }

    fn reasons(outcome: &Outcome) -> BTreeMap<String, String> {
        outcome
            .skipped
            .iter()
            .map(|s| {
                (
                    s["name"].as_str().unwrap().to_string(),
                    s["reason"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn a_session_rewound_recreated_or_locked_since_the_judgment_is_skipped() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = Store::new(tmp.path());
        for name in ["rewound", "recreated", "locked", "plain"] {
            store.finished(name, &format!("id-{name}"));
        }
        let backend = local(&store.base);
        let graph = Graph::scan(&store.base).unwrap();
        let engine = engine(&graph);
        let inv = Inventory::read(&graph);
        let verdict = judge(&inv, &engine, &no_filters(), &backend);
        let names: Vec<_> = verdict.remove.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["locked", "plain", "recreated", "rewound"]);

        // Between the judgment and the removal: one session is rewound, one
        // is replaced under the same name, one is locked by a koto command.
        store.append(
            "rewound",
            json!({"seq": 4, "timestamp": "2026-10-12T00:00:02.000Z", "type": "rewound",
                   "payload": {"from": "done", "to": "start"}}),
        );
        std::fs::remove_dir_all(store.base.join("recreated")).unwrap();
        store.finished("recreated", "id-recreated-2");
        let _held = backend.lock_state_file("locked").unwrap();

        let outcome = applied(&backend, &inv, &engine, &verdict);
        let removed: Vec<_> = outcome.removed.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(removed, vec!["plain"]);
        assert!(outcome.failed.is_empty());
        let skipped = reasons(&outcome);
        // The rewound session runs again, so its hierarchy is judged kept.
        assert!(
            skipped["rewound"].contains("now kept by rule h")
                && skipped["rewound"].contains("alive"),
            "{skipped:?}"
        );
        assert!(
            skipped["recreated"].contains("different session"),
            "{skipped:?}"
        );
        assert!(skipped["locked"].contains("lock"), "{skipped:?}");
        for name in ["rewound", "recreated", "locked"] {
            assert!(store.base.join(name).is_dir(), "{name} was removed");
        }
        assert!(!store.base.join("plain").exists());
    }

    #[test]
    fn a_hierarchy_with_a_member_running_again_is_skipped_whole() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = Store::new(tmp.path());
        store.finished("parent", "id-parent");
        store.finished("child", "id-child");
        let backend = local(&store.base);
        // The child names the parent, and both are finished at judgment.
        std::fs::create_dir_all(store.base.join("child/ctx/chain")).unwrap();
        std::fs::write(store.base.join("child/ctx/chain/parent"), "parent").unwrap();
        let graph = Graph::scan(&store.base).unwrap();
        let engine = engine(&graph);
        let inv = Inventory::read(&graph);
        let verdict = judge(&inv, &engine, &no_filters(), &backend);
        assert_eq!(verdict.remove.len(), 2);

        // The child is rewound before the apply: the hierarchy is judged
        // again as a unit, so neither member goes.
        store.append(
            "child",
            json!({"seq": 4, "timestamp": "2026-10-12T00:00:02.000Z", "type": "rewound",
                   "payload": {"from": "done", "to": "start"}}),
        );
        let outcome = applied(&backend, &inv, &engine, &verdict);
        assert!(outcome.removed.is_empty());
        let skipped = reasons(&outcome);
        assert_eq!(skipped.len(), 2, "{skipped:?}");
        for name in ["child", "parent"] {
            assert!(
                skipped[name].contains("now kept by rule h") && skipped[name].contains("child"),
                "{skipped:?}"
            );
            assert!(store.base.join(name).is_dir());
        }
    }

    #[test]
    fn a_record_replaced_since_the_judgment_is_skipped() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = Store::new(tmp.path());
        let record = |finished_at: &str| FinishedRecord {
            v: finished::RECORD_VERSION,
            name: "rec".to_string(),
            session_id: Some("id-1".to_string()),
            execution_dir: None,
            state: "done".to_string(),
            status: finished::STATUS_TERMINAL.to_string(),
            failure: false,
            result: None,
            truncated: false,
            finished_at: finished_at.to_string(),
            removed_by: finished::REMOVED_BY_TERMINAL.to_string(),
        };
        finished::write(&store.base, &record("2026-10-12T00:00:00.000Z")).unwrap();
        finished::write(
            &store.base,
            &FinishedRecord {
                name: "other".to_string(),
                ..record("2026-10-12T00:00:00.000Z")
            },
        )
        .unwrap();
        let backend = local(&store.base);
        let graph = Graph::scan(&store.base).unwrap();
        let engine = engine(&graph);
        let inv = Inventory::read(&graph);
        let verdict = judge(&inv, &engine, &no_filters(), &backend);
        let kinds: Vec<_> = verdict
            .remove
            .iter()
            .map(|e| (e.kind.as_str(), e.name.as_str()))
            .collect();
        assert_eq!(kinds, vec![("record", "other"), ("record", "rec")]);

        // Another session of the name finished and was removed since.
        finished::write(&store.base, &record("2026-10-12T01:00:00.000Z")).unwrap();
        let outcome = applied(&backend, &inv, &engine, &verdict);
        let removed: Vec<_> = outcome.removed.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(removed, vec!["other"]);
        assert_eq!(outcome.skipped.len(), 1);
        assert!(outcome.skipped[0]["reason"]
            .as_str()
            .unwrap()
            .contains("replaced"));
        assert!(finished::read(&store.base, "rec").unwrap().is_some());
    }

    #[test]
    fn a_preview_removes_plans_older_than_a_day_and_keeps_the_rest() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        let old = dir.join("old.json");
        let fresh = dir.join("fresh.json");
        std::fs::write(&old, "{}").unwrap();
        std::fs::write(&fresh, "{}").unwrap();
        let day_and_a_bit = SystemTime::now() - PLAN_MAX_AGE - Duration::from_secs(60);
        filetime::set_file_mtime(&old, filetime::FileTime::from_system_time(day_and_a_bit))
            .unwrap();
        let almost_a_day = SystemTime::now() - PLAN_MAX_AGE + Duration::from_secs(60);
        filetime::set_file_mtime(&fresh, filetime::FileTime::from_system_time(almost_a_day))
            .unwrap();
        expire_plans(dir);
        assert!(!old.exists());
        assert!(fresh.exists());
    }

    #[test]
    fn an_entry_that_isnt_a_session_is_never_removed_as_one() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = Store::new(tmp.path());
        store.finished("both", "id-both");
        let backend = local(&store.base);
        let graph = Graph::scan(&store.base).unwrap();
        let engine = engine(&graph);
        let inv = Inventory::read(&graph);
        let mut record = entry("both", "id-both");
        record.kind = "record".to_string();
        let verdict = Verdict {
            remove: vec![record],
            kept: Vec::new(),
            stale: BTreeMap::new(),
            token: String::new(),
        };
        let outcome = applied(&backend, &inv, &engine, &verdict);
        assert!(outcome.removed.is_empty());
        assert_eq!(outcome.skipped.len(), 1);
        assert!(store.base.join("both").is_dir());
    }

    #[test]
    fn chain_parent_must_be_exactly_one_name() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        assert_eq!(read_chain_parent(dir), ChainParent::Absent);
        std::fs::create_dir_all(dir.join("ctx/chain")).unwrap();
        let key = dir.join("ctx/chain/parent");
        std::fs::write(&key, "parent-1").unwrap();
        assert_eq!(
            read_chain_parent(dir),
            ChainParent::Names("parent-1".to_string())
        );
        for bad in ["parent-1\n", " parent-1", "", "a b", "../x"] {
            std::fs::write(&key, bad).unwrap();
            assert!(
                matches!(read_chain_parent(dir), ChainParent::Malformed(_)),
                "{bad:?}"
            );
        }
        std::fs::remove_file(&key).unwrap();
        std::os::unix::fs::symlink(dir.join("elsewhere"), &key).unwrap();
        assert!(matches!(read_chain_parent(dir), ChainParent::Malformed(_)));
    }
}
