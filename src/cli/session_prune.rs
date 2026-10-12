//! `koto session prune`: remove finished sessions as a previewed set.
//!
//! Without `--apply` the verb prints what it would remove, what it keeps and
//! why, and a plan token, and stores the would-remove list under
//! `<sessions base>/.prune-plans/<token>.json`. `--apply --plan <token>`
//! recomputes the set and removes it only when the token still matches.
//! `--apply` alone (with `--execution-dir`, `--older-than` or `--all`)
//! removes the recomputed set. Both modes run the same rules.
//!
//! The rules judge every local session that passes the filters, in order,
//! and the first that applies keeps it:
//!
//! - a: the session is running or unreadable.
//! - b: a running or unreadable session references it.
//! - c: it references a running or unreadable session.
//! - d: its `chain/parent` context key is malformed.
//! - e: it is bound to a request leg that is still waiting, or whose
//!   pointer or request can't be read.
//! - f: it is a coordinator's session (`coordinate-`).
//! - g: under the cloud backend, its remote copy may be newer.
//!
//! A reference is the header's `parent_workflow` or the `chain/parent`
//! context key, read from the local session directory. A name with no
//! session behind it protects nothing. Everything here reads the local
//! store, except rule g and the removal of a session's remote copy.

use std::collections::{BTreeMap, BTreeSet, HashMap};
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
use crate::session::validate::validate_session_id;
use crate::session::{state_file_name, Backend, SessionBackend, SessionError};

/// The directory under the sessions base where previews store their plans.
pub const PLANS_DIR: &str = ".prune-plans";

/// A stored plan older than this is removed by the next preview.
const PLAN_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// The exit code for a plan that is stale or unknown. No other koto verb
/// uses it.
const EXIT_PLAN_STALE: i32 = 4;

/// The exit code when a removal failed.
const EXIT_REMOVAL_FAILED: i32 = 3;

/// The largest `chain/parent` value read; a session id is far shorter.
const MAX_CHAIN_PARENT_BYTES: u64 = 1024;

/// What `koto session prune` was asked to do.
pub struct PruneArgs<'a> {
    pub execution_dir: Option<&'a str>,
    pub older_than: Option<&'a str>,
    pub all: bool,
    pub apply: bool,
    pub plan: Option<&'a str>,
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
}

/// One session a rule keeps.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct KeptEntry {
    pub name: String,
    pub rule: char,
    pub detail: String,
}

/// A stored plan: the would-remove list a preview printed.
#[derive(Debug, Serialize, Deserialize)]
struct StoredPlan {
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

impl Session {
    /// The sessions this one references, each once.
    fn references(&self, own: &str) -> BTreeSet<String> {
        let mut refs = BTreeSet::new();
        if let Some(parent) = self.header.as_ref().and_then(|h| h.parent_workflow.clone()) {
            refs.insert(parent);
        }
        if let ChainParent::Names(parent) = &self.chain_parent {
            refs.insert(parent.clone());
        }
        refs.remove(own);
        refs
    }
}

/// Every local session, read once.
struct Inventory {
    base: PathBuf,
    sessions: BTreeMap<String, Session>,
    /// For each name, the sessions that reference it.
    referencers: HashMap<String, BTreeSet<String>>,
}

impl Inventory {
    fn read(base: &Path) -> Result<Self> {
        let mut cache = TemplateCache::new();
        let mut sessions = BTreeMap::new();
        for name in session_names(base)? {
            let (seen, header) = inspect_full(base, &name, &mut cache);
            let chain_parent = read_chain_parent(&base.join(&name));
            sessions.insert(
                name,
                Session {
                    seen,
                    header,
                    chain_parent,
                },
            );
        }
        let mut referencers: HashMap<String, BTreeSet<String>> = HashMap::new();
        for (name, s) in &sessions {
            for target in s.references(name) {
                referencers.entry(target).or_default().insert(name.clone());
            }
        }
        Ok(Inventory {
            base: base.to_path_buf(),
            sessions,
            referencers,
        })
    }
}

/// The names of every local session under `base`: each directory entry
/// whose name is a valid session id and that holds `koto-<name>.state.jsonl`.
/// A symlinked session directory is listed, and `inspect` reports it
/// unreadable. Dot-directories, such as the stored plans, are never sessions.
fn session_names(base: &Path) -> Result<Vec<String>> {
    let entries = match std::fs::read_dir(base) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("failed to read {}", base.display())),
    };
    let mut names = Vec::new();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if name.starts_with('.') || validate_session_id(&name).is_err() {
            continue;
        }
        if session_exists(base, &name) {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

/// Whether `base/<name>` holds the session's state file.
fn session_exists(base: &Path, name: &str) -> bool {
    base.join(name).join(state_file_name(name)).exists()
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

/// How a referenced or referencing session stands, for rules b and c:
/// `Some` when it may be running (running or unreadable), `None` when it is
/// finished or doesn't exist.
fn may_run(status: Option<SessionStatus>) -> Option<&'static str> {
    match status {
        Some(SessionStatus::Running) => Some("running"),
        Some(SessionStatus::Unreadable) => Some("unreadable"),
        _ => None,
    }
}

/// Everything a rule reads about one session.
struct Facts<'a> {
    name: &'a str,
    status: SessionStatus,
    chain_parent: &'a ChainParent,
    references: BTreeSet<String>,
    referencers: BTreeSet<String>,
}

/// The first rule that keeps a session, with its detail. `status_of`
/// answers for another session by name (`None` when there is none).
fn first_rule(
    facts: &Facts<'_>,
    base: &Path,
    backend: &Backend,
    status_of: &mut dyn FnMut(&str) -> Option<SessionStatus>,
) -> Option<(char, String)> {
    match facts.status {
        SessionStatus::Running => return Some(('a', "the session is running".to_string())),
        SessionStatus::Unreadable => return Some(('a', "the session can't be read".to_string())),
        SessionStatus::Terminal | SessionStatus::Cancelled => {}
    }
    for referencer in &facts.referencers {
        if let Some(how) = may_run(status_of(referencer)) {
            return Some(('b', format!("referenced by {how} session '{referencer}'")));
        }
    }
    for target in &facts.references {
        if let Some(how) = may_run(status_of(target)) {
            return Some(('c', format!("references {how} session '{target}'")));
        }
    }
    if let ChainParent::Malformed(why) = facts.chain_parent {
        return Some((
            'd',
            format!("its chain/parent context key is malformed: {why}"),
        ));
    }
    if let Some(detail) = leg_keeps(&base.join(facts.name)) {
        return Some(('e', detail));
    }
    if facts.name.starts_with("coordinate-") {
        return Some(('f', "a coordinator's session".to_string()));
    }
    if let Backend::Cloud(cloud) = backend {
        match cloud.remote_copy_check(facts.name) {
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
    let Some(root) = dirs::home_dir().map(|h| h.join(".koto")) else {
        return Some("the request store can't be found (no home directory)".to_string());
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

/// The filters a session must pass to be judged at all.
struct Filters {
    execution_dir: Option<PathBuf>,
    older_than: Option<Duration>,
}

impl Filters {
    fn pass(&self, session: &Session, now: SystemTime) -> bool {
        if let Some(filter) = &self.execution_dir {
            let recorded = session
                .header
                .as_ref()
                .and_then(|h| h.execution_dir.as_deref());
            if !recorded.is_some_and(|dir| under_dir(dir, filter)) {
                return false;
            }
        }
        if let Some(min_age) = self.older_than {
            let finished = session
                .seen
                .finished_at
                .as_deref()
                .and_then(crate::engine::wake::parse_rfc3339_millis);
            let Some(finished) = finished else {
                return false;
            };
            match now.duration_since(finished) {
                Ok(age) if age >= min_age => {}
                _ => return false,
            }
        }
        true
    }
}

/// The verdict over a whole inventory: what would go, and what stays.
struct Verdict {
    remove: Vec<RemoveEntry>,
    kept: Vec<KeptEntry>,
    token: String,
}

fn judge(inv: &Inventory, filters: &Filters, backend: &Backend) -> Verdict {
    let now = SystemTime::now();
    let empty = BTreeSet::new();
    let mut remove = Vec::new();
    let mut kept = Vec::new();
    for (name, session) in &inv.sessions {
        if !filters.pass(session, now) {
            continue;
        }
        let facts = Facts {
            name,
            status: session.seen.status,
            chain_parent: &session.chain_parent,
            references: session.references(name),
            referencers: inv.referencers.get(name).unwrap_or(&empty).clone(),
        };
        let mut status_of = |other: &str| inv.sessions.get(other).map(|s| s.seen.status);
        match first_rule(&facts, &inv.base, backend, &mut status_of) {
            Some((rule, detail)) => kept.push(KeptEntry {
                name: name.clone(),
                rule,
                detail,
            }),
            None => remove.push(RemoveEntry {
                kind: "session".to_string(),
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
            }),
        }
    }
    let token = plan_token(&remove);
    Verdict {
        remove,
        kept,
        token,
    }
}

/// The first 32 hex digits of SHA-256 over the sorted token lines.
fn plan_token(remove: &[RemoveEntry]) -> String {
    use sha2::{Digest, Sha256};
    let mut lines: Vec<String> = remove.iter().map(RemoveEntry::token_line).collect();
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

/// Parse `--older-than`: a whole number followed by `s`, `m`, `h` or `d`.
fn parse_age(arg: &str) -> Option<Duration> {
    let unit = arg.chars().last()?;
    let digits = &arg[..arg.len() - unit.len_utf8()];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u64 = digits.parse().ok()?;
    let secs = match unit {
        's' => n,
        'm' => n.checked_mul(60)?,
        'h' => n.checked_mul(3600)?,
        'd' => n.checked_mul(86_400)?,
        _ => return None,
    };
    Some(Duration::from_secs(secs))
}

fn plan_unknown(token: &str) -> ! {
    exit_with_error_code(
        json!({"error": {
            "code": "prune_plan_unknown",
            "message": format!(
                "no stored plan {token}: it expired, was applied already, or was \
                 previewed on another host; preview again"
            ),
        }}),
        EXIT_PLAN_STALE,
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

    let inv = Inventory::read(&base)?;
    let verdict = judge(&inv, &filters, backend);

    if !args.apply {
        store_plan(&base, &verdict, &plan_filters)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "mode": "preview",
                "plan": verdict.token,
                "remove": verdict.remove,
                "kept": verdict.kept,
            }))?
        );
        return Ok(());
    }

    if let (Some(token), Some(stored)) = (args.plan, &stored) {
        if verdict.token != token {
            let now: BTreeSet<_> = verdict.remove.iter().map(RemoveEntry::key).collect();
            let then: BTreeSet<_> = stored.remove.iter().map(RemoveEntry::key).collect();
            let added: Vec<_> = verdict
                .remove
                .iter()
                .filter(|e| !then.contains(&e.key()))
                .map(RemoveEntry::brief)
                .collect();
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
                        "the sessions plan {token} would remove have changed since the \
                         preview; nothing was removed"
                    ),
                    "plan": token,
                    "current": verdict.token,
                    "added": added,
                    "dropped": dropped,
                }}),
                EXIT_PLAN_STALE,
            );
        }
    }

    // With a plan the two lists are equal here (the tokens matched), so this
    // changes nothing; it keeps "never remove what the preview didn't name"
    // from resting on the hash alone.
    let mut verdict = verdict;
    if let Some(stored) = &stored {
        let named: BTreeSet<_> = stored.remove.iter().map(RemoveEntry::key).collect();
        verdict.remove.retain(|e| named.contains(&e.key()));
    }
    let outcome = apply(backend, &inv, &verdict);
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
        }))?
    );
    if !outcome.failed.is_empty() {
        std::process::exit(EXIT_REMOVAL_FAILED);
    }
    Ok(())
}

#[derive(Default)]
struct Outcome {
    removed: Vec<RemoveEntry>,
    skipped: Vec<serde_json::Value>,
    failed: Vec<serde_json::Value>,
}

/// Remove the verdict's sessions, children before their parents, each
/// under its state lock after checking again that it may go.
fn apply(backend: &Backend, inv: &Inventory, verdict: &Verdict) -> Outcome {
    let mut order: Vec<&RemoveEntry> = verdict.remove.iter().collect();
    order.sort_by(|a, b| {
        depth(inv, &b.name)
            .cmp(&depth(inv, &a.name))
            .then_with(|| a.name.cmp(&b.name))
    });
    let mut outcome = Outcome::default();
    let mut cache = TemplateCache::new();
    for entry in order {
        match remove_one(backend, inv, entry, &mut cache) {
            Ok(()) => outcome.removed.push(entry.clone()),
            Err(Refusal::Skipped(reason)) => outcome
                .skipped
                .push(json!({"name": entry.name, "reason": reason})),
            Err(Refusal::Failed(error)) => outcome
                .failed
                .push(json!({"name": entry.name, "error": error})),
        }
    }
    outcome
}

/// How many `parent_workflow` links lie above `name` in the inventory.
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
    Failed(String),
}

fn remove_one(
    backend: &Backend,
    inv: &Inventory,
    entry: &RemoveEntry,
    cache: &mut TemplateCache,
) -> std::result::Result<(), Refusal> {
    let base = &inv.base;
    let name = entry.name.as_str();
    let _lock = match backend.lock_state_file(name) {
        Ok(lock) => lock,
        Err(SessionError::Locked { .. }) => {
            return Err(Refusal::Skipped(
                "another koto process holds the session's lock".to_string(),
            ))
        }
        Err(SessionError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Refusal::Skipped("the session is already gone".to_string()))
        }
        Err(e) => {
            return Err(Refusal::Skipped(format!(
                "the session can't be locked: {e}"
            )))
        }
    };

    let (seen, header) = inspect_full(base, name, cache);
    if !matches!(
        seen.status,
        SessionStatus::Terminal | SessionStatus::Cancelled
    ) {
        return Err(Refusal::Skipped(format!(
            "the session is no longer finished (now {})",
            status_word(seen.status)
        )));
    }
    let session_id = header.as_ref().and_then(|h| h.session_id.clone());
    if session_id != entry.session_id {
        return Err(Refusal::Skipped(
            "a different session now has this name".to_string(),
        ));
    }
    let fresh = Session {
        seen,
        header,
        chain_parent: read_chain_parent(&base.join(name)),
    };
    let facts = Facts {
        name,
        status: fresh.seen.status,
        chain_parent: &fresh.chain_parent,
        references: fresh.references(name),
        referencers: inv.referencers.get(name).cloned().unwrap_or_default(),
    };
    let mut status_of = |other: &str| -> Option<SessionStatus> {
        if !session_exists(base, other) {
            return None;
        }
        Some(inspect_full(base, other, cache).0.status)
    };
    if let Some((rule, detail)) = first_rule(&facts, base, backend, &mut status_of) {
        return Err(Refusal::Skipped(format!(
            "now kept by rule {rule}: {detail}"
        )));
    }

    // The remote copy goes first, so a failure leaves the local copy for a
    // later prune to retry.
    if let Backend::Cloud(cloud) = backend {
        cloud
            .delete_remote_session_checked(name)
            .map_err(|e| Refusal::Failed(format!("the remote copy wasn't removed: {e:#}")))?;
    }
    std::fs::remove_dir_all(base.join(name))
        .map_err(|e| Refusal::Failed(format!("the local directory wasn't removed: {e}")))?;
    Ok(())
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

/// Store the preview's would-remove list, creating the plans directory 0700,
/// and remove stored plans older than a day. A plans path that is a symlink
/// or not a directory is refused.
fn store_plan(base: &Path, verdict: &Verdict, filters: &PlanFilters) -> Result<()> {
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

    let plan = StoredPlan {
        plan: verdict.token.clone(),
        created_at: crate::engine::types::now_iso8601(),
        filters: filters.clone(),
        remove: verdict.remove.clone(),
    };
    let path = dir.join(format!("{}.json", verdict.token));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .with_context(|| format!("failed to write {}", path.display()))?;
    file.write_all(serde_json::to_string(&plan)?.as_bytes())
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

/// Read stored plan `token`; `None` when there is none.
fn read_plan(base: &Path, token: &str) -> Result<Option<StoredPlan>> {
    let path = plans_dir(base).join(format!("{token}.json"));
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_file() => {}
        Ok(_) => return Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
    let bytes =
        std::fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let plan: StoredPlan = match serde_json::from_slice(&bytes) {
        Ok(plan) => plan,
        Err(_) => return Ok(None),
    };
    if plan.plan != token {
        return Ok(None);
    }
    Ok(Some(plan))
}

fn remove_plan(base: &Path, token: &str) {
    let _ = std::fs::remove_file(plans_dir(base).join(format!("{token}.json")));
}

#[cfg(test)]
mod tests {
    use super::*;

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
        }
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

    #[test]
    fn a_session_rewound_recreated_or_locked_since_the_judgment_is_skipped() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = Store::new(tmp.path());
        for name in ["rewound", "recreated", "locked", "plain"] {
            store.finished(name, &format!("id-{name}"));
        }
        let backend = Backend::Local(crate::session::local::LocalBackend::with_base_dir(
            store.base.clone(),
        ));
        let inv = Inventory::read(&store.base).unwrap();
        let filters = Filters {
            execution_dir: None,
            older_than: None,
        };
        let verdict = judge(&inv, &filters, &backend);
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

        let outcome = apply(&backend, &inv, &verdict);
        let removed: Vec<_> = outcome.removed.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(removed, vec!["plain"]);
        assert!(outcome.failed.is_empty());
        let skipped: BTreeMap<String, String> = outcome
            .skipped
            .iter()
            .map(|s| {
                (
                    s["name"].as_str().unwrap().to_string(),
                    s["reason"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert!(
            skipped["rewound"].contains("no longer finished"),
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
    fn a_session_newly_referenced_by_a_running_one_is_skipped() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = Store::new(tmp.path());
        store.finished("parent", "id-parent");
        store.finished("child", "id-child");
        let backend = Backend::Local(crate::session::local::LocalBackend::with_base_dir(
            store.base.clone(),
        ));
        // The child already names the parent, and is finished at judgment.
        std::fs::create_dir_all(store.base.join("child/ctx/chain")).unwrap();
        std::fs::write(store.base.join("child/ctx/chain/parent"), "parent").unwrap();
        let inv = Inventory::read(&store.base).unwrap();
        let filters = Filters {
            execution_dir: None,
            older_than: None,
        };
        let verdict = judge(&inv, &filters, &backend);
        assert_eq!(verdict.remove.len(), 2);

        // The child is rewound before the removal reaches the parent. The
        // child goes first only when it is deeper by `parent_workflow`, which
        // it isn't here, so the parent's re-check sees a running referencer.
        store.append(
            "child",
            json!({"seq": 4, "timestamp": "2026-10-12T00:00:02.000Z", "type": "rewound",
                   "payload": {"from": "done", "to": "start"}}),
        );
        let outcome = apply(&backend, &inv, &verdict);
        let skipped: Vec<_> = outcome
            .skipped
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(skipped, vec!["child".to_string(), "parent".to_string()]);
        assert!(outcome.skipped[1]["reason"]
            .as_str()
            .unwrap()
            .contains("rule b"));
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
