//! The walk and the plans behind `koto session index audit`, `declare` and
//! `revoke` (koto#610), the operator commands that keep the bucket's name
//! index ([`crate::session::name_index`]) complete.
//!
//! An audit lists every workspace prefix's sessions and every index entry,
//! backfills the entry each state object lacks, and records what it did at
//! [`AUDIT_KEY`]. A declaration at [`DECLARATION_KEY`] then tells lookups
//! that a miss in the index is an answer. A host running an older release
//! writes state objects with no entry, so the declaration waits a probation
//! after the audit ([`PROBATION`]) for such a writer to show itself, and a
//! later audit that finds one revokes the declaration.
//!
//! Time is the bucket's. The probation is measured from the `LastModified`
//! the bucket gave [`AUDIT_KEY`] to the newest `LastModified` among the
//! state objects the walk saw, so on a bucket where no state object is
//! written for [`PROBATION`] after the audit, `declare` can't clear the
//! probation and `--no-probation` is the way out. The times a record holds
//! are host times, and only differences between two of them are ever used,
//! each turned into a bucket time through the `LastModified` of the object
//! that holds them: a skewed host clock cancels out, and no comparison here
//! reads the host clock.
//!
//! Every name and prefix the walk reads passes [`validate_session_id`] or
//! [`is_workspace_prefix`] before a key is built from it. One that fails
//! is skipped and counted, never written, deleted or allowed to stop the
//! walk.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use s3::serde_types::ListBucketResult;
use s3::Bucket;

use crate::session::continues_bucket::is_workspace_prefix;
use crate::session::name_index::{
    self, Audit, CappedRead, Declaration, AUDIT_KEY, DECLARATION_KEY, ENTRY_ROOT,
    MAX_DECLARATION_BYTES,
};
use crate::session::validate::validate_session_id;
use crate::session_view::{sanitize_capped, Sanitized};

/// How long after the audit, in bucket time, `declare` waits for an older
/// release's write to show up before it declares.
pub const PROBATION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The most characters of one piece of bucket text the commands print.
pub const PRINT_CAP: usize = 200;

/// The declaration format `declare` writes.
pub const DECLARATION_VERSION: u64 = 1;

/// `text`, read from the bucket, as the commands print it: control and
/// invisible characters escaped, cut at [`PRINT_CAP`] with a visible marker.
pub fn shown(text: &str) -> Sanitized {
    sanitize_capped(text, PRINT_CAP)
}

/// `<prefix>/<name>/koto-<name>.state.jsonl`, the state object of `name`'s
/// copy under `prefix`.
pub fn state_key(prefix: &str, name: &str) -> String {
    format!("{prefix}/{name}/koto-{name}.state.jsonl")
}

/// A state object the walk found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateObject {
    pub prefix: String,
    pub name: String,
    /// The bucket's `LastModified`; `None` when it didn't parse.
    pub last_modified: Option<SystemTime>,
}

impl StateObject {
    pub fn key(&self) -> String {
        state_key(&self.prefix, &self.name)
    }
}

/// What the walk read of [`AUDIT_KEY`] or [`DECLARATION_KEY`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Record<T> {
    /// Not listed.
    Missing,
    /// Listed, but over [`MAX_DECLARATION_BYTES`], gone by the read, or not
    /// parsing strictly: read as missing.
    Invalid,
    /// Listed and parsed, with the bucket's `LastModified` for it (`None`
    /// when that didn't parse).
    Found {
        value: T,
        last_modified: Option<SystemTime>,
    },
    /// The read got no answer; the text says how.
    Unanswered(String),
}

/// Everything one walk of the bucket read.
#[derive(Debug, Clone)]
pub struct Walk {
    /// Whether the top-level listing answered.
    pub top_level_answered: bool,
    /// The workspace prefixes the top-level listing named, in its order.
    pub prefixes: Vec<String>,
    /// Per prefix, the session names its listing named, or `None` when
    /// that listing got no answer.
    pub listed_names: BTreeMap<String, Option<BTreeSet<String>>>,
    /// Per `(prefix, name)`, whether the listing of `<prefix>/<name>/`
    /// answered.
    pub session_listings: BTreeMap<(String, String), bool>,
    /// The state objects found.
    pub states: Vec<StateObject>,
    /// The index entries found, as `(name, prefix)`.
    pub entries: BTreeSet<(String, String)>,
    /// Whether the listing of `koto-names/` answered.
    pub index_answered: bool,
    pub declaration: Record<Declaration>,
    pub audit: Record<Audit>,
    /// The listed keys and prefixes that failed validation, as listed.
    pub skipped: Vec<String>,
    /// The listings that still failed after their retries: the listed
    /// prefix and why.
    pub unanswered: Vec<(String, String)>,
}

impl Walk {
    /// A walk that has read nothing yet.
    pub fn empty() -> Self {
        Walk {
            top_level_answered: false,
            prefixes: Vec::new(),
            listed_names: BTreeMap::new(),
            session_listings: BTreeMap::new(),
            states: Vec::new(),
            entries: BTreeSet::new(),
            index_answered: false,
            declaration: Record::Missing,
            audit: Record::Missing,
            skipped: Vec::new(),
            unanswered: Vec::new(),
        }
    }

    /// The bucket's own "now": the newest `LastModified` among the state
    /// objects the walk saw. `None` when it saw none with a parsed time.
    pub fn bucket_now(&self) -> Option<SystemTime> {
        self.states.iter().filter_map(|s| s.last_modified).max()
    }

    fn has_entry(&self, state: &StateObject) -> bool {
        self.entries
            .contains(&(state.name.clone(), state.prefix.clone()))
    }

    /// The state objects with no index entry.
    pub fn entry_less(&self) -> Vec<&StateObject> {
        self.states.iter().filter(|s| !self.has_entry(s)).collect()
    }

    /// Whether the walk knows `prefix` holds no state object for `name`:
    /// a listing that answered showed the prefix, the session or its state
    /// object missing. A listing that got no answer knows nothing.
    pub fn holds_no_state(&self, prefix: &str, name: &str) -> bool {
        if !self.top_level_answered {
            return false;
        }
        match self.listed_names.get(prefix) {
            None => true,
            Some(None) => false,
            Some(Some(names)) if !names.contains(name) => true,
            Some(Some(_)) => {
                self.session_listings
                    .get(&(prefix.to_string(), name.to_string()))
                    == Some(&true)
                    && !self
                        .states
                        .iter()
                        .any(|s| s.prefix == prefix && s.name == name)
            }
        }
    }

    /// The entries whose prefix the walk knows holds no state object for
    /// their name, as keys.
    pub fn stale_entries(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|(name, prefix)| self.holds_no_state(prefix, name))
            .filter_map(|(name, prefix)| name_index::entry_key(name, prefix).ok())
            .collect()
    }

    /// Record the top-level listing's common prefixes.
    pub fn take_top_level<'a>(&mut self, common_prefixes: impl IntoIterator<Item = &'a str>) {
        self.top_level_answered = true;
        for listed in common_prefixes {
            if listed == ENTRY_ROOT {
                continue;
            }
            match listed.strip_suffix('/') {
                Some(prefix) if is_workspace_prefix(prefix) => {
                    if !self.prefixes.iter().any(|p| p == prefix) {
                        self.prefixes.push(prefix.to_string());
                    }
                }
                _ => self.skipped.push(listed.to_string()),
            }
        }
    }

    /// Record the listing of `<prefix>/`: its session common prefixes
    /// `<prefix>/<name>/`, each name validated. Returns the valid names.
    pub fn take_prefix<'a>(
        &mut self,
        prefix: &str,
        common_prefixes: impl IntoIterator<Item = &'a str>,
    ) -> Vec<String> {
        let head = format!("{prefix}/");
        let mut names = BTreeSet::new();
        for listed in common_prefixes {
            let name = listed
                .strip_prefix(&head)
                .and_then(|rest| rest.strip_suffix('/'));
            match name {
                Some(name) if validate_session_id(name).is_ok() => {
                    names.insert(name.to_string());
                }
                _ => self.skipped.push(listed.to_string()),
            }
        }
        let valid: Vec<String> = names.iter().cloned().collect();
        self.listed_names.insert(prefix.to_string(), Some(names));
        valid
    }

    /// Record the listing of `<prefix>/<name>/`: its objects as `(key,
    /// LastModified)`. Only the state object counts.
    pub fn take_session<'a>(
        &mut self,
        prefix: &str,
        name: &str,
        objects: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) {
        self.session_listings
            .insert((prefix.to_string(), name.to_string()), true);
        let wanted = state_key(prefix, name);
        if let Some((_, last_modified)) = objects.into_iter().find(|(key, _)| *key == wanted) {
            self.states.push(StateObject {
                prefix: prefix.to_string(),
                name: name.to_string(),
                last_modified: crate::engine::wake::parse_rfc3339_millis(last_modified),
            });
        }
    }

    /// Record the listing of `koto-names/` as `(key, LastModified, size)`.
    /// Returns, for the declaration and the audit record, whether each was
    /// listed small enough to read, with its `LastModified`.
    pub fn take_index<'a>(
        &mut self,
        objects: impl IntoIterator<Item = (&'a str, &'a str, u64)>,
    ) -> IndexListing {
        self.index_answered = true;
        let mut listing = IndexListing::default();
        for (key, last_modified, size) in objects {
            let listed = Listed {
                last_modified: crate::engine::wake::parse_rfc3339_millis(last_modified),
                readable: size <= MAX_DECLARATION_BYTES as u64,
            };
            if key == DECLARATION_KEY {
                listing.declaration = Some(listed);
                continue;
            }
            if key == AUDIT_KEY {
                listing.audit = Some(listed);
                continue;
            }
            let parts: Option<(&str, &str)> = key
                .strip_prefix(ENTRY_ROOT)
                .and_then(|rest| rest.split_once('/'));
            match parts {
                Some((name, prefix))
                    if validate_session_id(name).is_ok() && is_workspace_prefix(prefix) =>
                {
                    self.entries.insert((name.to_string(), prefix.to_string()));
                }
                _ => self.skipped.push(key.to_string()),
            }
        }
        listing
    }

    fn unanswered(&mut self, prefix: &str, why: String) {
        self.unanswered.push((prefix.to_string(), why));
    }
}

/// How the listing of `koto-names/` showed one of its two records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Listed {
    pub last_modified: Option<SystemTime>,
    /// Whether its size is within [`MAX_DECLARATION_BYTES`].
    pub readable: bool,
}

/// The two records the listing of `koto-names/` showed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexListing {
    pub declaration: Option<Listed>,
    pub audit: Option<Listed>,
}

/// The listing prefix as the commands name it.
pub fn listing_name(prefix: &str) -> String {
    if prefix.is_empty() {
        "the bucket's top level".to_string()
    } else {
        prefix.to_string()
    }
}

/// Walk `bucket`: the top-level listing, each workspace prefix's listing,
/// each session's listing, and the listing of `koto-names/`, each asked
/// through [`crate::session::cloud::list_retrying`] in the `url` encoding;
/// then a capped read of the declaration and the audit record when listed.
/// A listing that still fails is recorded in [`Walk::unanswered`] and the
/// walk goes on. `Err` only when the listing client can't be built.
pub fn walk(bucket: &Bucket) -> anyhow::Result<Walk> {
    use crate::session::remote_feed::LIST_ENCODING;

    let mut query = bucket.extra_query.clone();
    query.insert("encoding-type".to_string(), LIST_ENCODING.to_string());
    let lister = bucket.with_extra_query(query)?;
    let list = |prefix: &str, delimiter: Option<&str>| -> Result<Vec<ListBucketResult>, String> {
        let mut pages =
            crate::session::cloud::list_retrying(&lister, prefix.to_string(), delimiter)
                .map_err(|e| crate::session::cloud::without_url_userinfo(&e.to_string()))?;
        crate::session::remote_feed::decode_listing(&mut pages);
        Ok(pages)
    };
    let common = |pages: &[ListBucketResult]| -> Vec<String> {
        pages
            .iter()
            .filter_map(|page| page.common_prefixes.as_ref())
            .flatten()
            .map(|cp| cp.prefix.clone())
            .collect()
    };

    let mut walk = Walk::empty();
    match list("", Some("/")) {
        Ok(pages) => walk.take_top_level(common(&pages).iter().map(String::as_str)),
        Err(why) => walk.unanswered("", why),
    }
    for prefix in walk.prefixes.clone() {
        let head = format!("{prefix}/");
        let names = match list(&head, Some("/")) {
            Ok(pages) => walk.take_prefix(&prefix, common(&pages).iter().map(String::as_str)),
            Err(why) => {
                walk.listed_names.insert(prefix.clone(), None);
                walk.unanswered(&head, why);
                continue;
            }
        };
        for name in names {
            let session = format!("{prefix}/{name}/");
            match list(&session, Some("/")) {
                Ok(pages) => walk.take_session(
                    &prefix,
                    &name,
                    pages
                        .iter()
                        .flat_map(|page| page.contents.iter())
                        .map(|obj| (obj.key.as_str(), obj.last_modified.as_str())),
                ),
                Err(why) => {
                    walk.session_listings
                        .insert((prefix.clone(), name.clone()), false);
                    walk.unanswered(&session, why);
                }
            }
        }
    }
    let listing = match list(ENTRY_ROOT, None) {
        Ok(pages) => walk.take_index(
            pages
                .iter()
                .flat_map(|page| page.contents.iter())
                .map(|obj| (obj.key.as_str(), obj.last_modified.as_str(), obj.size)),
        ),
        Err(why) => {
            walk.unanswered(ENTRY_ROOT, why);
            IndexListing::default()
        }
    };
    if !walk.index_answered {
        let why = format!("the listing of {ENTRY_ROOT} got no answer");
        walk.declaration = Record::Unanswered(why.clone());
        walk.audit = Record::Unanswered(why);
    } else {
        walk.declaration = read_record(
            bucket,
            DECLARATION_KEY,
            listing.declaration,
            name_index::parse_declaration,
        );
        walk.audit = read_record(bucket, AUDIT_KEY, listing.audit, name_index::parse_audit);
    }
    Ok(walk)
}

/// The record at `key`, read with the capped GET when the listing showed it
/// small enough, and parsed strictly by `parse`.
fn read_record<T>(
    bucket: &Bucket,
    key: &str,
    listed: Option<Listed>,
    parse: fn(&[u8]) -> Option<T>,
) -> Record<T> {
    let Some(listed) = listed else {
        return Record::Missing;
    };
    if !listed.readable {
        return Record::Invalid;
    }
    match name_index::get_capped(bucket, key) {
        Ok(CappedRead::Body(bytes)) => match parse(&bytes) {
            Some(value) => Record::Found {
                value,
                last_modified: listed.last_modified,
            },
            None => Record::Invalid,
        },
        Ok(CappedRead::Absent) => Record::Invalid,
        Ok(CappedRead::Status(status)) => {
            Record::Unanswered(format!("reading {key} answered status {status}"))
        }
        Err(e) => Record::Unanswered(format!(
            "reading {key} failed: {}",
            crate::session::cloud::without_url_userinfo(&e.to_string())
        )),
    }
}

/// `last_modified` less the host-time span from `earlier` to `later`: the
/// bucket time of the moment a record's host time `earlier` names, when the
/// record was written at its host time `later` and the bucket stamped it
/// `last_modified`. A span that runs backwards counts as zero.
pub fn bucket_time_of(
    last_modified: SystemTime,
    later: SystemTime,
    earlier: SystemTime,
) -> SystemTime {
    let span = later.duration_since(earlier).unwrap_or(Duration::ZERO);
    last_modified.checked_sub(span).unwrap_or(UNIX_EPOCH)
}

/// Whether the probation has passed: the bucket's newest state object is at
/// least [`PROBATION`] younger than the audit record. Both are bucket
/// times; an unknown one hasn't passed.
pub fn probation_passed(audit_at: Option<SystemTime>, bucket_now: Option<SystemTime>) -> bool {
    match (audit_at, bucket_now) {
        (Some(audit_at), Some(now)) => now
            .duration_since(audit_at)
            .is_ok_and(|waited| waited >= PROBATION),
        _ => false,
    }
}

/// Why an audit run removes the declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revocation {
    /// `--prune --revoke` asked for it.
    pub requested: bool,
    /// The entry-less state objects newer than the declaration's audit (or
    /// of unknown age), as keys: each is a write the declared index misses.
    pub older_writers: Vec<String>,
}

/// What an audit run will write, in order: the declaration's DELETE, the
/// entry PUTs, the entry DELETEs, then the audit record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditPlan {
    pub revoke: Option<Revocation>,
    pub puts: Vec<String>,
    /// Entries whose prefix holds no state for their name.
    pub stale: Vec<String>,
    /// The entries the run deletes: [`AuditPlan::stale`] under `--prune`.
    pub deletes: Vec<String>,
    /// Whether the run ends by writing [`AUDIT_KEY`]: only after a walk in
    /// which every listing answered.
    pub write_record: bool,
}

impl AuditPlan {
    /// Every key the plan deletes, in the order it deletes them.
    pub fn deletion_order(&self) -> Vec<String> {
        let mut keys = Vec::new();
        if self.revoke.is_some() {
            keys.push(DECLARATION_KEY.to_string());
        }
        keys.extend(self.deletes.iter().cloned());
        keys
    }
}

/// Why an audit run refuses before it writes anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditRefusal {
    /// `--prune` on a declared bucket without `--revoke`.
    PruneOnDeclared,
    /// Whether the bucket is declared got no answer.
    DeclarationUnread(String),
}

/// Plan an audit run from `walk`.
pub fn plan_audit(walk: &Walk, prune: bool, revoke: bool) -> Result<AuditPlan, AuditRefusal> {
    let declared = match &walk.declaration {
        Record::Unanswered(why) => return Err(AuditRefusal::DeclarationUnread(why.clone())),
        Record::Found {
            value,
            last_modified,
        } => Some((value, *last_modified)),
        Record::Missing | Record::Invalid => None,
    };
    if prune && declared.is_some() && !revoke {
        return Err(AuditRefusal::PruneOnDeclared);
    }
    let entry_less = walk.entry_less();
    let puts: Vec<String> = entry_less
        .iter()
        .filter_map(|s| name_index::entry_key(&s.name, &s.prefix).ok())
        .collect();
    let stale = walk.stale_entries();
    let deletes = if prune { stale.clone() } else { Vec::new() };
    let revoke = declared.and_then(|(declaration, last_modified)| {
        let start = last_modified
            .map(|lm| bucket_time_of(lm, declaration.declared_at, declaration.audit.started_at));
        let older_writers: Vec<String> = entry_less
            .iter()
            .filter(|s| match (start, s.last_modified) {
                (Some(start), Some(lm)) => lm > start,
                _ => true,
            })
            .map(|s| s.key())
            .collect();
        let requested = prune && revoke;
        (requested || !older_writers.is_empty()).then_some(Revocation {
            requested,
            older_writers,
        })
    });
    Ok(AuditPlan {
        revoke,
        puts,
        stale,
        deletes,
        write_record: walk.unanswered.is_empty(),
    })
}

/// The audit record a run writes at its end.
pub fn audit_record(
    walk: &Walk,
    started_at: SystemTime,
    finished_at: SystemTime,
    entries_written: u64,
    entries_pruned: u64,
) -> Audit {
    Audit {
        started_at,
        finished_at,
        prefixes: walk.prefixes.len() as u64,
        sessions: walk.states.len() as u64,
        entries_written,
        entries_pruned,
    }
}

/// Why `declare` refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclareRefusal {
    /// Listings that still failed after their retries.
    Unanswered(Vec<(String, String)>),
    /// The audit record's read got no answer.
    AuditUnread(String),
    /// No audit record, or one that doesn't parse.
    NoAudit,
    /// State objects with no index entry, as keys.
    EntryLess(Vec<String>),
    /// The probation hasn't passed.
    Probation {
        audit_at: Option<SystemTime>,
        bucket_now: Option<SystemTime>,
    },
}

impl DeclareRefusal {
    /// Whether the refusal is the bucket's failure to answer rather than
    /// what it answered.
    pub fn is_infrastructure(&self) -> bool {
        matches!(
            self,
            DeclareRefusal::Unanswered(_) | DeclareRefusal::AuditUnread(_)
        )
    }
}

/// Plan `declare` from `walk`: the audit record the declaration rests on,
/// or every reason it refuses. `no_probation` lifts only the probation.
///
/// Any state object with no entry refuses, whatever its age: the audit
/// backfilled every one it saw, so one without an entry now was written
/// after it by a release that doesn't index, or lost its entry since, and
/// a declaration would make a lookup read that session as absent.
pub fn plan_declare(walk: &Walk, no_probation: bool) -> Result<Audit, Vec<DeclareRefusal>> {
    let mut refusals = Vec::new();
    if !walk.unanswered.is_empty() {
        refusals.push(DeclareRefusal::Unanswered(walk.unanswered.clone()));
    }
    let audit = match &walk.audit {
        Record::Found {
            value,
            last_modified,
        } => Some((value.clone(), *last_modified)),
        Record::Unanswered(why) => {
            refusals.push(DeclareRefusal::AuditUnread(why.clone()));
            None
        }
        Record::Missing | Record::Invalid => {
            refusals.push(DeclareRefusal::NoAudit);
            None
        }
    };
    let entry_less: Vec<String> = walk.entry_less().iter().map(|s| s.key()).collect();
    if !entry_less.is_empty() {
        refusals.push(DeclareRefusal::EntryLess(entry_less));
    }
    if let Some((_, audit_at)) = &audit {
        let bucket_now = walk.bucket_now();
        if !no_probation && !probation_passed(*audit_at, bucket_now) {
            refusals.push(DeclareRefusal::Probation {
                audit_at: *audit_at,
                bucket_now,
            });
        }
    }
    match audit {
        Some((record, _)) if refusals.is_empty() => Ok(record),
        _ => Err(refusals),
    }
}

/// The declaration `declare` writes.
pub fn declaration(audit: Audit, declared_at: SystemTime, machine: String) -> Declaration {
    Declaration {
        version: DECLARATION_VERSION,
        declared_at,
        machine,
        first_release: format!("v{}", crate::buildinfo::build_info().version),
        audit,
    }
}

/// A span as whole days, hours and minutes, for a refusal's text.
pub fn span_text(span: Duration) -> String {
    let minutes = span.as_secs() / 60;
    format!(
        "{}d {}h {}m",
        minutes / (24 * 60),
        (minutes / 60) % 24,
        minutes % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbb";

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn rfc(t: SystemTime) -> String {
        crate::engine::types::iso8601_at(t)
    }

    const DAY: u64 = 24 * 60 * 60;

    /// A walk in which every listing answered, with `sessions` as `(prefix,
    /// name, LastModified)` and `entries` as `(name, prefix)`.
    fn walked(sessions: &[(&str, &str, u64)], entries: &[(&str, &str)]) -> Walk {
        let mut walk = Walk::empty();
        let mut prefixes: Vec<&str> = sessions.iter().map(|s| s.0).collect();
        prefixes.dedup();
        let top: Vec<String> = prefixes.iter().map(|p| format!("{p}/")).collect();
        walk.take_top_level(top.iter().map(String::as_str));
        for prefix in &prefixes {
            let listed: Vec<String> = sessions
                .iter()
                .filter(|s| s.0 == *prefix)
                .map(|s| format!("{}/{}/", s.0, s.1))
                .collect();
            walk.take_prefix(prefix, listed.iter().map(String::as_str));
        }
        for (prefix, name, lm) in sessions {
            let key = state_key(prefix, name);
            let lm = rfc(at(*lm));
            walk.take_session(prefix, name, [(key.as_str(), lm.as_str())]);
        }
        let keys: Vec<String> = entries
            .iter()
            .map(|(n, p)| name_index::entry_key(n, p).unwrap())
            .collect();
        walk.take_index(
            keys.iter()
                .map(|k| (k.as_str(), "2026-01-01T00:00:00.000Z", 0)),
        );
        walk
    }

    fn audit_found(started: u64, finished: u64, lm: u64) -> Record<Audit> {
        Record::Found {
            value: Audit {
                started_at: at(started),
                finished_at: at(finished),
                prefixes: 1,
                sessions: 1,
                entries_written: 0,
                entries_pruned: 0,
            },
            last_modified: Some(at(lm)),
        }
    }

    fn declared(audit_started: u64, declared_at: u64, lm: u64) -> Record<Declaration> {
        let Record::Found { value, .. } = audit_found(audit_started, audit_started, 0) else {
            unreachable!()
        };
        Record::Found {
            value: Declaration {
                version: 1,
                declared_at: at(declared_at),
                machine: "m".into(),
                first_release: "v0.35.0".into(),
                audit: value,
            },
            last_modified: Some(at(lm)),
        }
    }

    #[test]
    fn bucket_time_uses_only_the_host_span() {
        // Host clock 1000 days ahead: only the 60 s span counts.
        let lm = at(500 * DAY);
        let start = bucket_time_of(lm, at(1000 * DAY + 60), at(1000 * DAY));
        assert_eq!(start, at(500 * DAY - 60));
        // A span that runs backwards counts as zero.
        assert_eq!(bucket_time_of(lm, at(1), at(2)), lm);
    }

    #[test]
    fn probation_boundary() {
        let audit = at(100 * DAY);
        let short = audit + Duration::from_secs(6 * DAY + 23 * 3600);
        let long = audit + Duration::from_secs(7 * DAY + 60);
        assert!(!probation_passed(Some(audit), Some(short)));
        assert!(probation_passed(Some(audit), Some(long)));
        assert!(probation_passed(Some(audit), Some(audit + PROBATION)));
        // A bucket whose newest state object predates the audit hasn't
        // passed, and neither has an unknown time.
        assert!(!probation_passed(Some(audit), Some(at(DAY))));
        assert!(!probation_passed(None, Some(long)));
        assert!(!probation_passed(Some(audit), None));
    }

    #[test]
    fn walk_skips_and_counts_what_fails_validation() {
        let mut walk = Walk::empty();
        walk.take_top_level([
            "aaaaaaaaaaaaaaaa/",
            "koto-names/",
            "not-a-prefix/",
            "AAAAAAAAAAAAAAAA/",
        ]);
        assert_eq!(walk.prefixes, vec![A.to_string()]);
        let names = walk.take_prefix(
            A,
            [
                "aaaaaaaaaaaaaaaa/good/",
                "aaaaaaaaaaaaaaaa/9bad/",
                "aaaaaaaaaaaaaaaa/has space/",
                "aaaaaaaaaaaaaaaa/ctx/",
            ],
        );
        assert_eq!(names, vec!["ctx".to_string(), "good".to_string()]);
        walk.take_index([
            ("koto-names/good/aaaaaaaaaaaaaaaa", "", 0),
            ("koto-names/good/short", "", 0),
            ("koto-names/good/aaaaaaaaaaaaaaaa/extra", "", 0),
            ("koto-names/9bad/aaaaaaaaaaaaaaaa", "", 0),
            ("koto-names/_complete", "", 10),
            ("koto-names/_audit", "", 5000),
        ]);
        assert_eq!(
            walk.entries,
            BTreeSet::from([("good".to_string(), A.to_string())])
        );
        assert_eq!(walk.skipped.len(), 2 + 2 + 3);
    }

    #[test]
    fn the_index_listing_reports_its_records() {
        let mut walk = Walk::empty();
        let listing = walk.take_index([
            ("koto-names/_complete", "2026-01-01T00:00:00.000Z", 10),
            ("koto-names/_audit", "2026-01-01T00:00:00.000Z", 5000),
        ]);
        assert!(listing.declaration.unwrap().readable);
        assert!(!listing.audit.unwrap().readable);
        assert!(walk.skipped.is_empty());
    }

    #[test]
    fn the_audit_backfills_missing_entries_only() {
        let walk = walked(
            &[(A, "one", DAY), (A, "two", DAY), (B, "one", DAY)],
            &[("one", A)],
        );
        let plan = plan_audit(&walk, false, false).unwrap();
        assert_eq!(
            plan.puts,
            vec![format!("koto-names/two/{A}"), format!("koto-names/one/{B}")]
        );
        assert!(plan.deletes.is_empty());
        assert!(plan.revoke.is_none());
        assert!(plan.write_record);
    }

    #[test]
    fn stale_entries_are_deleted_only_under_prune() {
        let walk = walked(&[(A, "one", DAY)], &[("one", A), ("one", B), ("gone", A)]);
        let plan = plan_audit(&walk, false, false).unwrap();
        assert_eq!(plan.stale.len(), 2);
        assert!(plan.deletes.is_empty());
        let plan = plan_audit(&walk, true, false).unwrap();
        assert_eq!(
            plan.deletes,
            vec![
                format!("koto-names/gone/{A}"),
                format!("koto-names/one/{B}")
            ]
        );
    }

    #[test]
    fn an_unanswered_listing_knows_nothing_stale_and_writes_no_record() {
        let mut walk = walked(&[(A, "one", DAY)], &[("two", A)]);
        walk.listed_names.insert(A.to_string(), None);
        walk.unanswered.push((format!("{A}/"), "timeout".into()));
        let plan = plan_audit(&walk, true, false).unwrap();
        assert!(plan.deletes.is_empty());
        assert!(!plan.write_record);
    }

    #[test]
    fn prune_on_a_declared_bucket_needs_revoke_and_deletes_the_declaration_first() {
        let mut walk = walked(&[(A, "one", DAY)], &[("one", A), ("gone", A)]);
        walk.declaration = declared(DAY, DAY, 2 * DAY);
        assert_eq!(
            plan_audit(&walk, true, false),
            Err(AuditRefusal::PruneOnDeclared)
        );
        let plan = plan_audit(&walk, true, true).unwrap();
        assert!(plan.revoke.as_ref().unwrap().requested);
        assert_eq!(
            plan.deletion_order(),
            vec![DECLARATION_KEY.to_string(), format!("koto-names/gone/{A}")]
        );
        // Without --prune, the stale entry is counted and kept.
        let plan = plan_audit(&walk, false, false).unwrap();
        assert_eq!(plan.stale.len(), 1);
        assert!(plan.deletes.is_empty());
        assert!(plan.revoke.is_none());
    }

    #[test]
    fn only_an_entry_less_object_newer_than_the_declared_audit_revokes() {
        // The declaration's audit started 10 s (host time) before it was
        // declared; the bucket stamped the declaration at 100 d, so the
        // audit started at 100 d - 10 s in bucket time.
        let start = 100 * DAY - 10;
        let mut older = walked(&[(B, "old", start - 1)], &[]);
        older.declaration = declared(5000 * DAY, 5000 * DAY + 10, 100 * DAY);
        let plan = plan_audit(&older, false, false).unwrap();
        assert!(plan.revoke.is_none());
        assert_eq!(plan.puts, vec![format!("koto-names/old/{B}")]);

        let mut newer = walked(&[(B, "new", start + 1)], &[]);
        newer.declaration = declared(5000 * DAY, 5000 * DAY + 10, 100 * DAY);
        let plan = plan_audit(&newer, false, false).unwrap();
        assert_eq!(
            plan.revoke.as_ref().unwrap().older_writers,
            vec![state_key(B, "new")]
        );
        assert_eq!(plan.puts, vec![format!("koto-names/new/{B}")]);
        assert_eq!(plan.deletion_order(), vec![DECLARATION_KEY.to_string()]);
    }

    #[test]
    fn an_unread_declaration_refuses_the_audit() {
        let mut walk = walked(&[(A, "one", DAY)], &[]);
        walk.declaration = Record::Unanswered("timeout".into());
        assert!(matches!(
            plan_audit(&walk, false, false),
            Err(AuditRefusal::DeclarationUnread(_))
        ));
    }

    #[test]
    fn declare_refuses_without_an_audit_record() {
        let walk = walked(&[(A, "one", 30 * DAY)], &[("one", A)]);
        assert_eq!(
            plan_declare(&walk, true),
            Err(vec![DeclareRefusal::NoAudit])
        );
        let mut invalid = walk.clone();
        invalid.audit = Record::Invalid;
        assert_eq!(
            plan_declare(&invalid, true),
            Err(vec![DeclareRefusal::NoAudit])
        );
    }

    #[test]
    fn declare_measures_the_probation_in_bucket_time() {
        let audit_lm = 10 * DAY;
        let mut walk = walked(&[(A, "one", audit_lm + 6 * DAY + 23 * 3600)], &[("one", A)]);
        walk.audit = audit_found(9_000 * DAY, 9_000 * DAY + 5, audit_lm);
        assert!(matches!(
            plan_declare(&walk, false).unwrap_err().as_slice(),
            [DeclareRefusal::Probation { .. }]
        ));
        assert!(plan_declare(&walk, true).is_ok());

        let mut walk = walked(&[(A, "one", audit_lm + 7 * DAY + 60)], &[("one", A)]);
        walk.audit = audit_found(1, 5, audit_lm);
        let record = plan_declare(&walk, false).unwrap();
        assert_eq!(record.started_at, at(1));
    }

    #[test]
    fn no_probation_lifts_only_the_probation() {
        let mut walk = walked(
            &[(A, "one", 20 * DAY), (B, "late", 20 * DAY)],
            &[("one", A)],
        );
        walk.audit = audit_found(1, 5, 19 * DAY);
        walk.unanswered
            .push((format!("{A}/one/"), "timeout".into()));
        let refusals = plan_declare(&walk, true).unwrap_err();
        assert!(refusals.iter().any(|r| r.is_infrastructure()));
        assert!(refusals.contains(&DeclareRefusal::EntryLess(vec![state_key(B, "late")])));
        assert!(!refusals
            .iter()
            .any(|r| matches!(r, DeclareRefusal::Probation { .. })));
    }

    #[test]
    fn the_declaration_round_trips_through_the_strict_parse() {
        let Record::Found { value: audit, .. } = audit_found(1, 5, 0) else {
            unreachable!()
        };
        let audit = Audit {
            started_at: at(1) + Duration::from_millis(250),
            ..audit
        };
        let written = declaration(audit.clone(), at(DAY), "host".into());
        let bytes = serde_json::to_vec(&written).unwrap();
        assert!(bytes.len() < MAX_DECLARATION_BYTES);
        assert_eq!(name_index::parse_declaration(&bytes), Some(written));
        let bytes = serde_json::to_vec(&audit).unwrap();
        assert_eq!(name_index::parse_audit(&bytes), Some(audit));
    }

    #[test]
    fn the_audit_record_rejects_a_missing_field_or_an_oversized_body() {
        assert_eq!(name_index::parse_audit(b"{\"started_at\":\"x\"}"), None);
        assert_eq!(name_index::parse_audit(b"not json"), None);
        let big = vec![b' '; MAX_DECLARATION_BYTES + 1];
        assert_eq!(name_index::parse_audit(&big), None);
    }

    #[test]
    fn printed_bucket_text_is_escaped_and_capped() {
        let shown = shown(&format!("\u{1b}[2J{}", "x".repeat(PRINT_CAP * 2)));
        assert!(!shown.as_str().bytes().any(|b| b < 0x20 || b == 0x7f));
        assert!(shown.as_str().starts_with("\\u{1B}[2J"));
        assert!(shown.as_str().ends_with(crate::session_view::CUT_MARKER));
    }

    #[test]
    fn span_text_reads_in_days_hours_and_minutes() {
        assert_eq!(
            span_text(Duration::from_secs(6 * DAY + 23 * 3600 + 120)),
            "6d 23h 2m"
        );
    }
}
