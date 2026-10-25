//! Where the bucket's answer about the continues relation meets the local
//! one, and the walk that gives it.
//!
//! [`crate::session::hierarchy::Judgment::keeps`] stays synchronous and
//! local. Every removal point that can delete a session asks
//! [`relation_at_removal`] (or [`relation_with_bucket`], where it holds a
//! relation verdict rather than a judgment) once nothing local keeps the
//! session, and keeps the session when the answer keeps it, merging the
//! answer into the entry's report. The points are: `koto session prune`'s
//! judgment and its apply, `koto workspace prune`'s judgment and its
//! reclaim, the same-name replace's disposition, the parent sweep, the
//! terminal tick (when no local edge names the session) and the report of
//! `koto session cleanup` and `koto cancel --cleanup`. Each removal that
//! deletes by rule also asks [`final_guard`] right after the pin guard
//! clears it and before the delete, which walks only the back-references
//! the judgment didn't see.
//!
//! # The back-reference
//!
//! A successor `S` under the cloud backend holds its relation at
//! `<Sp>/<S>/ctx/koto/links/continues/<P>`, where nothing indexes it by the
//! session it names. So it also writes one standalone object beside its
//! predecessor, `<Pp>/<P>/ctx/koto/continued-by/<Sp>/<S>`, whose body is
//! only `{"v":1,"holder_session_id":"<id>"}`. It is written by a direct PUT,
//! never through the context store, so no manifest lists it and no push
//! deletes it, and it sits outside `koto/links/`, so no scan reads it as a
//! link. The pin guard's listing of `<Pp>/<P>/ctx/koto/` therefore finds
//! every successor of `P`, under any prefix.
//!
//! A pointer is only a hint. Nothing is decided from its body: its key
//! names where to look, and the walk trusts only the holder's own relation,
//! once its recorded session id matches the predecessor's. A forged pointer
//! can only make a host read a relation the holder really holds, so it can
//! only over-keep.
//!
//! # The walk
//!
//! [`walk`] starts from the predecessor's facts ([`BucketRemovalFacts`]):
//! its `ctx/koto/` listing (shared with the pin guard when the guard made
//! it), and one delimiter listing of `<Pp>/<P>/` for a migration marker.
//! For each valid pointer it lists the holder's `ctx/koto/`, GETs its
//! relation and, when the relation names the predecessor by session id,
//! reads the holder's standing from the bucket: its pin object when one is
//! listed, else its state file, judged by the inventory's rules against its
//! compiled template (from the local cache by hash, or one bounded GET of
//! `template.json` per hash per run, checked against the hash). Then it
//! follows the holder's own pointers, breadth-first with a visited set.
//!
//! Work is counted. Beyond the predecessor's own one or two listings (one
//! when the pin guard's is shared), its back-references may spend 2
//! requests, and each later member reached adds 3 more, a template read
//! counted inside; every pointer examined, valid or not, stale or not,
//! draws from that allowance. So a session nothing names costs at most 2
//! requests, a stale pointer or two (a rollback whose DELETE failed) is
//! read and ignored, and the three-member chain judged on a host holding
//! only its first member costs 2 plus 3 for each of the other two. A
//! listing with more than 64 pointers isn't walked, and a walk stops at 16
//! members or 50 requests. What the walk can't afford, list or read keeps
//! the predecessor as `bucket_unchecked`, naming it, so a credentials gap
//! or a pointer flood can never become a deletion.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::session::hierarchy::relation::{Reason, Unreadable};
use crate::session::hierarchy::{
    Judgment, RelationKeeper, RelationVerdict, Standing, Unchecked, UncheckedError,
};
use crate::session::links::{LinkBody, Prefix, MAX_LINK_BODY_BYTES};
use crate::session::pin::{PinFact, PinProblem, MAX_PIN_BYTES, PIN_KEY, RELEASE_KEY};
use crate::session::SessionBackend;

/// Where a predecessor's back-references live, under its `ctx/`.
pub const CONTINUED_BY: &str = "koto/continued-by/";

/// The bare file in a successor's session directory that records the
/// pointer key the bucket confirmed. The sync doesn't track it. Its absence,
/// or a key other than the one the session's relation and current name
/// give, means a pointer is owed.
pub const POINTER_MARK: &str = ".continues-pointer";

/// A listing with more back-references than this isn't walked.
pub const MAX_POINTERS: usize = 64;

/// A walk stops after reaching this many later members.
pub const MAX_MEMBERS: usize = 16;

/// A walk stops after this many requests.
pub const MAX_REQUESTS: usize = 50;

/// What the predecessor's back-references may cost before any later
/// member is reached.
const PRED_ALLOWANCE: usize = 2;

/// What each later member reached adds to the allowance.
const MEMBER_ALLOWANCE: usize = 3;

/// Largest state file the walk reads.
pub const MAX_STATE_BYTES: usize = 16 * 1024 * 1024;

/// Largest compiled template the walk reads.
pub const MAX_TEMPLATE_BYTES: usize = 4 * 1024 * 1024;

/// How a name read from the bucket that fails its validator is shown.
const PLACEHOLDER: &str = "(invalid name)";

// ---------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------

/// A back-reference, as its key names it: the holder's workspace prefix
/// and name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pointer {
    pub prefix: String,
    pub name: String,
}

/// Whether `prefix` is a workspace prefix: 16 lowercase hex digits.
pub fn is_workspace_prefix(prefix: &str) -> bool {
    prefix.len() == 16
        && prefix
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_name(name: &str) -> bool {
    crate::session::validate::validate_session_id(name).is_ok()
}

/// The key of the back-reference `holder` (under `holder_prefix`) keeps
/// beside `pred` (under `pred_prefix`).
pub fn pointer_key(pred_prefix: &str, pred: &str, holder_prefix: &str, holder: &str) -> String {
    format!("{pred_prefix}/{pred}/ctx/{CONTINUED_BY}{holder_prefix}/{holder}")
}

/// The pointer `key` names when it is exactly `continued-by/<Sp>/<S>` under
/// `session_prefix` (`<Pp>/<P>/`, with its slash) `ctx/koto/`: `<Sp>` 16
/// lowercase hex digits and `<S>` a valid session name. Anything else,
/// including a key with more segments, is `None`. Every pointer key read
/// from the bucket passes here before it becomes a request.
pub fn is_backref_key(session_prefix: &str, key: &str) -> Option<Pointer> {
    let rest = key
        .strip_prefix(session_prefix)?
        .strip_prefix("ctx/")?
        .strip_prefix(CONTINUED_BY)?;
    let (prefix, name) = rest.split_once('/')?;
    (is_workspace_prefix(prefix) && valid_name(name)).then(|| Pointer {
        prefix: prefix.to_string(),
        name: name.to_string(),
    })
}

/// Whether `key` lies in a session's `ctx/koto/continued-by/` area under
/// `session_prefix`, valid or not: the objects every delete path leaves.
pub fn in_backref_area(session_prefix: &str, key: &str) -> bool {
    key.strip_prefix(session_prefix)
        .and_then(|rest| rest.strip_prefix("ctx/"))
        .is_some_and(|rest| rest.starts_with(CONTINUED_BY))
}

/// The body of a back-reference: the holder's session id and nothing else,
/// so no workspace path is copied into another workspace's prefix.
pub fn pointer_body(holder_session_id: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"v": 1, "holder_session_id": holder_session_id}))
        .expect("a JSON object serializes")
}

/// Whether `id` has the shape of a session id koto generates: 36
/// characters of lowercase hex in the 8-4-4-4-12 grouping.
pub fn is_session_id(id: &str) -> bool {
    id.len() == 36
        && id.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_digit() || ('a'..='f').contains(&c),
        })
}

// ---------------------------------------------------------------------
// Listings and facts
// ---------------------------------------------------------------------

/// One object a listing named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedObject {
    pub key: String,
    pub size: u64,
    pub etag: Option<String>,
}

/// A listing, and the requests (pages) it took.
#[derive(Debug, Clone, Default)]
pub struct Listed {
    pub objects: Vec<ListedObject>,
    pub requests: usize,
}

/// What a session's `ctx/koto/` listing says, by the keys the walk reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CtxListing {
    /// The pin object's size, when one is listed.
    pub pin: Option<u64>,
    /// Objects lie under the pin's key: an unreadable pin.
    pub pin_not_a_file: bool,
    /// The release record's size, when one is listed.
    pub release: Option<u64>,
    /// The valid back-references, sorted.
    pub pointers: Vec<Pointer>,
    /// The keys in the back-reference area that aren't valid ones, as the
    /// part after `continued-by/`.
    pub invalid_pointers: Vec<String>,
    /// The session's own continues relations: the predecessor name in each
    /// key, with the object's size.
    pub continues: Vec<(String, u64)>,
    /// The session holds `koto/imported-from`: it is an import's copy.
    pub imported_from: bool,
}

impl CtxListing {
    /// Read a listing of `session_prefix` (`<prefix>/<name>/`) `ctx/koto/`.
    pub fn parse(session_prefix: &str, objects: &[ListedObject]) -> Self {
        let ctx = format!("{session_prefix}ctx/");
        let pin_under = format!("{PIN_KEY}/");
        let continues = crate::session::links::continues_prefix();
        let mut out = CtxListing::default();
        for obj in objects {
            let Some(key) = obj.key.strip_prefix(&ctx) else {
                continue;
            };
            if key == PIN_KEY {
                out.pin = Some(obj.size);
            } else if key.starts_with(&pin_under) {
                out.pin_not_a_file = true;
            } else if key == RELEASE_KEY {
                out.release = Some(obj.size);
            } else if let Some(rest) = key.strip_prefix(CONTINUED_BY) {
                match is_backref_key(session_prefix, &obj.key) {
                    Some(p) => out.pointers.push(p),
                    None => out.invalid_pointers.push(rest.to_string()),
                }
            } else if let Some(pred) = key.strip_prefix(&continues) {
                if !pred.is_empty() && !pred.contains('/') {
                    out.continues.push((pred.to_string(), obj.size));
                }
            } else if key == crate::session::hierarchy::IMPORTED_FROM_KEY {
                out.imported_from = true;
            }
        }
        out.pointers.sort();
        out.pointers.dedup();
        out.invalid_pointers.sort();
        out
    }

    /// Every back-reference the walk examines, valid or not.
    pub fn pointer_count(&self) -> usize {
        self.pointers.len() + self.invalid_pointers.len()
    }
}

/// What the bucket holds beside a session about to be deleted: the
/// removal's facts ([`SessionBackend::bucket_removal_facts`]). One listing
/// of `<Pp>/<P>/ctx/koto/`, the pin guard's when it just made one, and one
/// delimiter listing of `<Pp>/<P>/`.
#[derive(Debug, Clone)]
pub struct BucketRemovalFacts {
    /// The session's workspace prefix and name.
    pub prefix: String,
    pub name: String,
    /// The session's own id, from this host's copy of its header.
    pub session_id: Option<String>,
    /// The `ctx/koto/` listing: the pin and release keys and sizes, the
    /// validated pointers and the invalid ones.
    pub ctx: Result<CtxListing, UncheckedError>,
    /// Whether a migration marker lies beside the session.
    pub migrated: Result<bool, UncheckedError>,
    /// The state object's ETag, from the delimiter listing.
    pub state_etag: Option<String>,
    /// When the session's header names another `origin.store.prefix`: that
    /// prefix and its `ctx/koto/` listing of the session.
    pub origin: Option<(String, Result<CtxListing, UncheckedError>)>,
    /// What these facts cost.
    pub requests: usize,
}

// ---------------------------------------------------------------------
// The bucket, as the walk reads it
// ---------------------------------------------------------------------

/// The reads the walk makes. The cloud backend answers them from its
/// bucket; a test, from memory. Each listing page and each GET is one
/// request.
pub trait BucketReads {
    /// Every object under `prefix`, paged.
    fn list(&self, prefix: &str) -> Result<Listed, UncheckedError>;
    /// GET `key`, reading at most one byte more than `max`: `Ok(None)` when
    /// it is absent.
    fn get(&self, key: &str, max: usize) -> Result<Option<Vec<u8>>, UncheckedError>;
    /// The compiled template with hash `hash` from this host's cache, at no
    /// request; `None` when the cache doesn't hold it.
    fn cached_template(&self, hash: &str) -> Option<Vec<u8>>;
    /// Whether a migration marker beside `<prefix>/<name>` names a target
    /// that is there and verified: the target's prefix and name, `Ok(None)`
    /// when there is no usable target (absent or replaced), and the
    /// requests it took either way. The default reads nothing.
    fn marker_target(
        &self,
        _prefix: &str,
        _name: &str,
    ) -> (Result<Option<Pointer>, UncheckedError>, usize) {
        (Ok(None), 0)
    }
}

/// The instant and setting a walk judges standing against.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    pub keep: Duration,
    pub now: SystemTime,
}

impl Clock {
    /// The clock a removal outside a judgment uses: `retention.keep` as
    /// configured (the default when it can't be read; the walk only keeps
    /// more with a longer one) and now.
    pub fn configured() -> Self {
        let keep = crate::session::hierarchy::RetentionSettings::load()
            .map(|s| s.keep.value)
            .unwrap_or(Duration::from_secs(7 * 86_400));
        Clock {
            keep,
            now: SystemTime::now(),
        }
    }
}

// ---------------------------------------------------------------------
// The memo
// ---------------------------------------------------------------------

/// Where a member was found, before its session id is known.
type Loc = (String, String);

/// What the walk read of one later member, kept for the judgment pass so a
/// member reached from several predecessors costs its requests once.
#[derive(Debug, Clone)]
struct MemberRead {
    listing: CtxListing,
    /// Each relation the member holds, by the predecessor name in its key:
    /// the parsed body, or why it couldn't be read.
    relations: Vec<(String, Result<LinkBody, Reason>)>,
    /// What its standing rests on, once read.
    standing: Option<Result<StandingFacts, UncheckedError>>,
}

/// What a member's own records say about its standing, before a clock is
/// applied.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StandingFacts {
    /// It holds a pin, readable or not.
    Pin(PinFact),
    /// It runs, or its log or template can't be read.
    Alive { last_event: Option<SystemTime> },
    /// It stopped at `stop`.
    Stopped {
        stop: SystemTime,
        last_event: Option<SystemTime>,
    },
    /// Its state file is gone.
    Gone,
}

/// A judgment pass's reads, and the templates read this run.
#[derive(Debug, Default)]
pub struct Memo {
    ids: HashMap<Loc, String>,
    by_id: HashMap<String, MemberRead>,
    by_loc: HashMap<Loc, MemberRead>,
    /// The pointers each predecessor's judgment saw, by its prefix and
    /// name: what [`final_guard`] compares a fresh listing with.
    judged: HashMap<Loc, BTreeSet<Pointer>>,
    /// Per run, not per pass: each template hash's terminal states, `None`
    /// when it couldn't be read or didn't match its hash.
    templates: HashMap<String, Option<BTreeSet<String>>>,
    /// Per run, not per pass: what the delimiter listing of each judged
    /// session's prefix said (a migration marker, the state's ETag), so a
    /// run that judges a session again lists its back-references and its
    /// later members afresh without listing its prefix twice.
    tops: HashMap<Loc, (Result<bool, UncheckedError>, Option<String>)>,
}

impl Memo {
    /// Start a judgment pass: forget every member read and what each
    /// judgment saw, so the pass reads the bucket again. Templates and the
    /// judged sessions' prefix listings stay, read once per run.
    pub fn begin_pass(&mut self) {
        self.ids.clear();
        self.by_id.clear();
        self.by_loc.clear();
        self.judged.clear();
    }

    /// The delimiter listing of `<prefix>/<name>/` this run already made.
    pub fn top(
        &self,
        prefix: &str,
        name: &str,
    ) -> Option<(Result<bool, UncheckedError>, Option<String>)> {
        self.tops
            .get(&(prefix.to_string(), name.to_string()))
            .cloned()
    }

    /// Keep what the delimiter listing of `<prefix>/<name>/` said, when it
    /// could be made.
    pub fn set_top(
        &mut self,
        prefix: &str,
        name: &str,
        top: (Result<bool, UncheckedError>, Option<String>),
    ) {
        if top.0.is_ok() {
            self.tops
                .insert((prefix.to_string(), name.to_string()), top);
        }
    }

    fn get(&self, loc: &Loc) -> Option<&MemberRead> {
        match self.ids.get(loc) {
            Some(id) => self.by_id.get(id),
            None => self.by_loc.get(loc),
        }
    }

    fn put(&mut self, loc: Loc, id: Option<&str>, read: MemberRead) {
        match id {
            Some(id) => {
                self.by_loc.remove(&loc);
                self.ids.insert(loc, id.to_string());
                self.by_id.insert(id.to_string(), read);
            }
            None => {
                self.by_loc.insert(loc, read);
            }
        }
    }

    /// The pointers the judgment of `<prefix>/<name>` saw, when one ran in
    /// this pass.
    pub fn judged(&self, prefix: &str, name: &str) -> Option<&BTreeSet<Pointer>> {
        self.judged.get(&(prefix.to_string(), name.to_string()))
    }
}

// ---------------------------------------------------------------------
// The walk
// ---------------------------------------------------------------------

/// What the walk spent and may spend.
struct Budget {
    used: usize,
    allowance: usize,
    requests: usize,
    members: usize,
}

impl Budget {
    /// Whether one more request fits: within the allowance and the request
    /// ceiling.
    fn fits(&self, n: usize) -> bool {
        self.used + n <= self.allowance && self.requests + n <= MAX_REQUESTS
    }

    fn spend(&mut self, n: usize) {
        self.used += n;
        self.requests += n;
    }
}

/// One back-reference to follow: the holder, the members between the
/// judged session and it, and the predecessor it should name.
struct Visit {
    at: Pointer,
    path: Vec<String>,
    /// Where the predecessor the holder should name lives, and its id.
    pred: Pointer,
    pred_id: Option<String>,
    /// Reached through a migration marker rather than a relation: the
    /// marker, verified, is the evidence.
    by_marker: bool,
}

/// One later member the walk reached.
struct Reached {
    at: Pointer,
    path: Vec<String>,
    standing: Option<StandingFacts>,
    /// The predecessor it names, and the relation's `at`, when it is an
    /// import's copy (`koto/imported-from`).
    copy_of: Option<(Pointer, Option<SystemTime>)>,
}

/// What a walk found: the verdict, and the pointers it saw beside the
/// judged session.
#[derive(Debug, Clone, Default)]
pub struct Walked {
    pub verdict: RelationVerdict,
    pub pointers: BTreeSet<Pointer>,
}

/// Where a walk starts: the judged session's facts, and, for
/// [`final_guard`], the pointers already walked.
pub struct Start<'a> {
    pub facts: &'a BucketRemovalFacts,
    /// Pointers not to walk again: the judgment's, when this is the final
    /// guard.
    pub skip: Option<&'a BTreeSet<Pointer>>,
    /// Later members passed in by the caller (a marker's target), walked
    /// like members reached through a marker.
    pub extra: &'a [Pointer],
}

/// Walk the back-references beside `start.facts`'s session, verify each
/// against its holder's relation, and judge each later member's standing
/// from the bucket, within the budget the module doc describes.
pub fn walk(reads: &dyn BucketReads, memo: &mut Memo, start: &Start<'_>, clock: &Clock) -> Walked {
    let facts = start.facts;
    let mut verdict = RelationVerdict::default();
    let mut seen = BTreeSet::new();
    // The judged session's own listings (one or two) are what any removal
    // reads; the allowance is for what its back-references cost.
    let mut budget = Budget {
        used: 0,
        allowance: PRED_ALLOWANCE,
        requests: facts.requests,
        members: 0,
    };
    let me = Pointer {
        prefix: facts.prefix.clone(),
        name: facts.name.clone(),
    };
    let unchecked = |name: &str, error| Unchecked {
        name: shown(name),
        error,
    };

    let mut listings: Vec<&CtxListing> = Vec::new();
    match &facts.ctx {
        Ok(l) => listings.push(l),
        Err(e) => verdict.unchecked.push(unchecked(&facts.name, *e)),
    }
    if let Some((_, origin)) = &facts.origin {
        match origin {
            Ok(l) => listings.push(l),
            Err(e) => verdict.unchecked.push(unchecked(&facts.name, *e)),
        }
    }

    let mut queue: VecDeque<Visit> = VecDeque::new();
    let mut flood = false;
    for listing in &listings {
        if listing.pointer_count() > MAX_POINTERS {
            flood = true;
            continue;
        }
        for raw in &listing.invalid_pointers {
            // Examined, so it draws from the allowance, but no request is
            // made for it.
            budget.used += 1;
            verdict.unreadable.push(Unreadable {
                holder: holder_of_invalid(raw),
                reason: Reason::Unparseable,
            });
        }
        for p in &listing.pointers {
            seen.insert(p.clone());
            if start.skip.is_some_and(|s| s.contains(p)) {
                continue;
            }
            queue.push_back(Visit {
                at: p.clone(),
                path: Vec::new(),
                pred: me.clone(),
                pred_id: facts.session_id.clone(),
                by_marker: false,
            });
        }
    }
    if flood {
        verdict
            .unchecked
            .push(unchecked(&facts.name, UncheckedError::TooManyPointers));
    }
    match facts.migrated {
        Ok(false) => {}
        Err(e) => verdict.unchecked.push(unchecked(&facts.name, e)),
        Ok(true) if start.skip.is_some() => {}
        Ok(true) => {
            let (target, cost) = reads.marker_target(&facts.prefix, &facts.name);
            // The marker's own reads come with the member it names.
            budget.allowance += cost;
            budget.spend(cost);
            match target {
                Ok(Some(t)) => queue.push_back(Visit {
                    at: t,
                    path: Vec::new(),
                    pred: me.clone(),
                    pred_id: facts.session_id.clone(),
                    by_marker: true,
                }),
                Ok(None) => {}
                Err(e) => verdict.unchecked.push(unchecked(&facts.name, e)),
            }
        }
    }
    for extra in start.extra {
        queue.push_back(Visit {
            at: extra.clone(),
            path: Vec::new(),
            pred: me.clone(),
            pred_id: facts.session_id.clone(),
            by_marker: true,
        });
    }

    // The members reached, the judged session among them: reaching one
    // again through a relation is a cycle, which keeps. A stale pointer
    // doesn't count as reaching its holder.
    let mut visited: BTreeSet<Pointer> = BTreeSet::from([me.clone()]);
    let mut reached: Vec<Reached> = Vec::new();
    while let Some(visit) = queue.pop_front() {
        if visited.contains(&visit.at) {
            verdict.unreadable.push(Unreadable {
                holder: shown(&visit.at.name),
                reason: Reason::Cycle,
            });
            continue;
        }
        if budget.members >= MAX_MEMBERS || budget.requests >= MAX_REQUESTS {
            verdict
                .unchecked
                .push(unchecked(&visit.at.name, UncheckedError::Budget));
            continue;
        }
        // Granted while the pointer is followed, taken back if it turns out
        // stale: a member reached pays for its own reads, a stale pointer
        // out of what is left.
        budget.allowance += MEMBER_ALLOWANCE;
        let loc = (visit.at.prefix.clone(), visit.at.name.clone());
        let read = match read_member(reads, memo, &loc, &mut budget) {
            Ok(read) => read,
            Err(e) => {
                verdict.unchecked.push(unchecked(&visit.at.name, e));
                continue;
            }
        };
        let body = match match_relation(&read, &visit) {
            Match::Member(body) => body,
            Match::Unreadable(reason) => {
                verdict.unreadable.push(Unreadable {
                    holder: shown(&visit.at.name),
                    reason,
                });
                continue;
            }
            Match::Stale => {
                budget.allowance -= MEMBER_ALLOWANCE;
                if budget.used > budget.allowance {
                    verdict
                        .unchecked
                        .push(unchecked(&visit.at.name, UncheckedError::Budget));
                }
                continue;
            }
        };
        visited.insert(visit.at.clone());
        budget.members += 1;
        let id = body
            .as_ref()
            .and_then(|b| b.holder_session_id.clone())
            .filter(|id| is_session_id(id));
        let standing = match read_standing(reads, memo, &loc, id.as_deref(), &mut budget) {
            Ok(s) => Some(s),
            Err(e) => {
                verdict.unchecked.push(unchecked(&visit.at.name, e));
                None
            }
        };
        let copy_of = read.listing.imported_from.then(|| {
            (
                visit.pred.clone(),
                body.as_ref()
                    .and_then(|b| crate::engine::wake::parse_rfc3339_millis(&b.at)),
            )
        });
        let mut next_path = visit.path.clone();
        next_path.push(visit.at.name.clone());
        if read.listing.pointer_count() > MAX_POINTERS {
            verdict
                .unchecked
                .push(unchecked(&visit.at.name, UncheckedError::TooManyPointers));
        } else {
            for raw in &read.listing.invalid_pointers {
                budget.used += 1;
                verdict.unreadable.push(Unreadable {
                    holder: holder_of_invalid(raw),
                    reason: Reason::Unparseable,
                });
            }
            for p in &read.listing.pointers {
                queue.push_back(Visit {
                    at: p.clone(),
                    path: next_path.clone(),
                    pred: visit.at.clone(),
                    pred_id: id.clone(),
                    by_marker: false,
                });
            }
        }
        reached.push(Reached {
            at: visit.at.clone(),
            path: visit.path.clone(),
            standing,
            copy_of,
        });
    }

    // A member counts as migrated, with no standing of its own, when one of
    // its successors is an import's copy whose relation names it and was
    // written no earlier than its last event: the local graph's rule.
    let migrated: BTreeSet<Pointer> = reached
        .iter()
        .filter_map(|copy| {
            let (of, at) = copy.copy_of.as_ref()?;
            let source = reached.iter().find(|r| &r.at == of)?;
            let last = match &source.standing {
                Some(StandingFacts::Alive { last_event })
                | Some(StandingFacts::Stopped { last_event, .. }) => (*last_event)?,
                _ => return None,
            };
            (at.is_some_and(|at| last <= at)).then(|| source.at.clone())
        })
        .collect();
    for r in &reached {
        if migrated.contains(&r.at) {
            continue;
        }
        if let Some(keeper) = keeper_of(r, clock) {
            verdict.keepers.push(keeper);
        }
    }
    verdict.normalize();
    memo.judged
        .entry((me.prefix.clone(), me.name.clone()))
        .or_default()
        .extend(seen.iter().cloned());
    Walked {
        verdict,
        pointers: seen,
    }
}

/// How a holder's relations answer a visit.
enum Match {
    /// A relation names the predecessor by its session id (or by name, when
    /// the predecessor's id isn't known), with that relation; or the holder
    /// was reached through a verified marker, with none.
    Member(Option<LinkBody>),
    /// A relation names the predecessor's name with another session id, or
    /// can't be read.
    Unreadable(Reason),
    /// Nothing the holder holds names the predecessor: a stale pointer.
    Stale,
}

fn match_relation(read: &MemberRead, visit: &Visit) -> Match {
    if visit.by_marker {
        return Match::Member(None);
    }
    let mut unreadable = None;
    for (_, body) in &read.relations {
        match body {
            Ok(b) => {
                let by_id = matches!(
                    (&b.session_id, &visit.pred_id),
                    (Some(theirs), Some(ours)) if theirs == ours
                );
                if by_id || (visit.pred_id.is_none() && b.session == visit.pred.name) {
                    return Match::Member(Some(b.clone()));
                }
                if b.session == visit.pred.name {
                    unreadable.get_or_insert(Reason::Mismatch);
                }
            }
            Err(reason) => {
                unreadable = Some(*reason);
            }
        }
    }
    match unreadable {
        Some(reason) => Match::Unreadable(reason),
        None => Match::Stale,
    }
}

/// The member at `loc`'s listing and relations, from the memo or the
/// bucket.
fn read_member(
    reads: &dyn BucketReads,
    memo: &mut Memo,
    loc: &Loc,
    budget: &mut Budget,
) -> Result<MemberRead, UncheckedError> {
    if let Some(read) = memo.get(loc) {
        return Ok(read.clone());
    }
    let session_prefix = format!("{}/{}/", loc.0, loc.1);
    if !budget.fits(1) {
        return Err(UncheckedError::Budget);
    }
    let listed = reads.list(&format!("{session_prefix}ctx/koto/"));
    // A listing that pages costs each page.
    let listed = match listed {
        Ok(l) => {
            budget.spend(l.requests.max(1));
            l
        }
        Err(e) => {
            budget.spend(1);
            return Err(e);
        }
    };
    let listing = CtxListing::parse(&session_prefix, &listed.objects);
    let mut relations = Vec::new();
    for (pred, size) in &listing.continues {
        if *size > MAX_LINK_BODY_BYTES as u64 {
            relations.push((pred.clone(), Err(Reason::TooLarge)));
            continue;
        }
        if !budget.fits(1) {
            return Err(UncheckedError::Budget);
        }
        budget.spend(1);
        let key = format!(
            "{session_prefix}ctx/{}{pred}",
            crate::session::links::continues_prefix()
        );
        match reads.get(&key, MAX_LINK_BODY_BYTES)? {
            // Listed, then gone: removed meanwhile, which names nothing.
            None => {}
            Some(bytes) if bytes.len() > MAX_LINK_BODY_BYTES => {
                relations.push((pred.clone(), Err(Reason::TooLarge)))
            }
            Some(bytes) => relations.push((pred.clone(), parse_relation(&bytes, pred))),
        }
    }
    let read = MemberRead {
        listing,
        relations,
        standing: None,
    };
    let id = read
        .relations
        .iter()
        .find_map(|(_, b)| b.as_ref().ok()?.holder_session_id.clone())
        .filter(|id| is_session_id(id));
    memo.put(loc.clone(), id.as_deref(), read.clone());
    Ok(read)
}

/// A relation body read from the bucket, checked field by field: `session`
/// names the key's predecessor, `prefix` is 16 lowercase hex digits or
/// null, the session ids have the id format, `at` is RFC 3339 and
/// `workspace` is an absolute path (a hint, never joined into a request or
/// a path). Anything else is `unparseable`.
pub fn parse_relation(bytes: &[u8], pred: &str) -> Result<LinkBody, Reason> {
    let body: LinkBody = serde_json::from_slice(bytes).map_err(|_| Reason::Unparseable)?;
    let ok = body.session == pred
        && match &body.prefix {
            Prefix::Value(p) => is_workspace_prefix(p),
            _ => true,
        }
        && body.session_id.as_deref().is_none_or(is_session_id)
        && body.holder_session_id.as_deref().is_none_or(is_session_id)
        && crate::engine::wake::parse_rfc3339_millis(&body.at).is_some()
        && body
            .workspace
            .as_deref()
            .is_none_or(|w| Path::new(w).is_absolute());
    if ok {
        Ok(body)
    } else {
        Err(Reason::Unparseable)
    }
}

/// The member at `loc`'s standing facts, from the memo or the bucket: its
/// pin object when one is listed, else its state file.
fn read_standing(
    reads: &dyn BucketReads,
    memo: &mut Memo,
    loc: &Loc,
    id: Option<&str>,
    budget: &mut Budget,
) -> Result<StandingFacts, UncheckedError> {
    let mut read = memo.get(loc).cloned().ok_or(UncheckedError::Unreachable)?;
    if let Some(done) = &read.standing {
        return done.clone();
    }
    let session_prefix = format!("{}/{}/", loc.0, loc.1);
    let result = standing_facts(reads, memo, &session_prefix, &loc.1, &read.listing, budget);
    read.standing = Some(result.clone());
    let id = id.map(str::to_string);
    memo.put(loc.clone(), id.as_deref(), read);
    result
}

fn standing_facts(
    reads: &dyn BucketReads,
    memo: &mut Memo,
    session_prefix: &str,
    name: &str,
    listing: &CtxListing,
    budget: &mut Budget,
) -> Result<StandingFacts, UncheckedError> {
    if listing.pin_not_a_file {
        return Ok(StandingFacts::Pin(PinFact::Unreadable(
            PinProblem::NotAFile,
        )));
    }
    if let Some(size) = listing.pin {
        if size > MAX_PIN_BYTES as u64 {
            return Ok(StandingFacts::Pin(PinFact::Unreadable(
                PinProblem::TooLarge,
            )));
        }
        if !budget.fits(1) {
            return Err(UncheckedError::Budget);
        }
        budget.spend(1);
        return Ok(StandingFacts::Pin(
            match reads.get(&format!("{session_prefix}ctx/{PIN_KEY}"), MAX_PIN_BYTES)? {
                Some(bytes) => crate::session::pin::pin_of_bytes(&bytes),
                None => PinFact::Unreadable(PinProblem::MissingContent),
            },
        ));
    }
    if !budget.fits(1) {
        return Err(UncheckedError::Budget);
    }
    budget.spend(1);
    let state_key = format!("{session_prefix}{}", crate::session::state_file_name(name));
    let Some(bytes) = reads.get(&state_key, MAX_STATE_BYTES)? else {
        return Ok(StandingFacts::Gone);
    };
    Ok(standing_from_bytes(&bytes, |hash| {
        template_terminals(reads, memo, session_prefix, hash, budget)
    }))
}

/// The terminal states of the compiled template with hash `hash`: from this
/// host's cache, the run's memo, or one bounded GET of the member's
/// `template.json`, which must hash to `hash` and parse as a compiled
/// template (data only: nothing in it runs). `None` when none of those
/// serves.
fn template_terminals(
    reads: &dyn BucketReads,
    memo: &mut Memo,
    session_prefix: &str,
    hash: &str,
    budget: &mut Budget,
) -> Option<BTreeSet<String>> {
    if let Some(known) = memo.templates.get(hash) {
        return known.clone();
    }
    if let Some(set) = reads
        .cached_template(hash)
        .and_then(|b| terminals_of(&b, hash))
    {
        memo.templates.insert(hash.to_string(), Some(set.clone()));
        return Some(set);
    }
    if !budget.fits(1) {
        return None;
    }
    budget.spend(1);
    let key = format!("{session_prefix}{}", crate::session::cloud::TEMPLATE_OBJECT);
    let set = reads
        .get(&key, MAX_TEMPLATE_BYTES)
        .ok()
        .flatten()
        .and_then(|b| terminals_of(&b, hash));
    memo.templates.insert(hash.to_string(), set.clone());
    set
}

/// The terminal states of `bytes` as a compiled template, when it hashes to
/// `hash`, fits [`MAX_TEMPLATE_BYTES`] and parses.
fn terminals_of(bytes: &[u8], hash: &str) -> Option<BTreeSet<String>> {
    if hash.is_empty()
        || bytes.len() > MAX_TEMPLATE_BYTES
        || crate::cache::sha256_hex(bytes) != hash
    {
        return None;
    }
    let compiled: crate::template::types::CompiledTemplate = serde_json::from_slice(bytes).ok()?;
    Some(
        compiled
            .states
            .iter()
            .filter(|(_, s)| s.terminal)
            .map(|(n, _)| n.clone())
            .collect(),
    )
}

/// A member's standing from its state file's bytes, by the inventory's
/// rules: a log that doesn't parse, or whose template can't be had, is
/// alive (it keeps); a trailing cancel or a terminal state stopped at the
/// event that set it; anything else runs. `terminals` gives the terminal
/// states of the template with the hash the header records.
fn standing_from_bytes(
    bytes: &[u8],
    terminals: impl FnOnce(&str) -> Option<BTreeSet<String>>,
) -> StandingFacts {
    use crate::session::inventory::{status_of, SessionStatus};
    let alive = |last_event| StandingFacts::Alive { last_event };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return alive(None);
    };
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let Some(header) = lines
        .next()
        .and_then(|l| serde_json::from_str::<crate::engine::types::StateFileHeader>(l).ok())
    else {
        return alive(None);
    };
    let mut events = Vec::new();
    for line in lines {
        match serde_json::from_str::<crate::engine::types::Event>(line) {
            Ok(e) => events.push(e),
            Err(_) => return alive(None),
        }
    }
    let last_event = events
        .last()
        .and_then(|e| crate::engine::wake::parse_rfc3339_millis(&e.timestamp));
    if header.session_kind() != crate::engine::types::SessionKind::Ordinary {
        return alive(last_event);
    }
    let Some(state) = crate::engine::persistence::derive_state_from_log(&events) else {
        return alive(last_event);
    };
    let Some(terminal) = terminals(&header.template_hash) else {
        return alive(last_event);
    };
    let (status, stopped_at) = status_of(&events, terminal.contains(&state));
    match status {
        SessionStatus::Terminal | SessionStatus::Cancelled => {
            match stopped_at
                .as_deref()
                .and_then(crate::engine::wake::parse_rfc3339_millis)
            {
                Some(stop) => StandingFacts::Stopped { stop, last_event },
                None => alive(last_event),
            }
        }
        _ => alive(last_event),
    }
}

/// The keeper `r` is, when its standing keeps: a pin, alive, or within
/// `retention.keep` of its stop.
fn keeper_of(r: &Reached, clock: &Clock) -> Option<RelationKeeper> {
    let keeper = |standing, retention_passes_at, pin| RelationKeeper {
        keeper: r.at.name.clone(),
        standing,
        retention_passes_at,
        pin,
        pin_source: crate::session::pin::SOURCE_BUCKET,
        path: r.path.clone(),
    };
    match r.standing.as_ref()? {
        StandingFacts::Pin(fact) => Some(keeper(Standing::Pin, None, fact.clone())),
        StandingFacts::Alive { .. } => Some(keeper(Standing::Alive, None, PinFact::None)),
        StandingFacts::Stopped { stop, .. } => {
            let passes = *stop + clock.keep;
            (passes > clock.now).then(|| {
                keeper(
                    Standing::Retention,
                    Some(crate::engine::types::iso8601_at(passes)),
                    PinFact::None,
                )
            })
        }
        StandingFacts::Gone => None,
    }
}

/// The holder an invalid pointer key names, for the report: its holder
/// segment when that is a valid name, a placeholder otherwise.
fn holder_of_invalid(raw: &str) -> String {
    raw.rsplit('/')
        .next()
        .filter(|n| valid_name(n))
        .map(str::to_string)
        .unwrap_or_else(|| PLACEHOLDER.to_string())
}

/// A name from the bucket as a report shows it: itself when valid, a
/// placeholder otherwise.
fn shown(name: &str) -> String {
    if valid_name(name) {
        name.to_string()
    } else {
        PLACEHOLDER.to_string()
    }
}

// ---------------------------------------------------------------------
// The removal points' entry points
// ---------------------------------------------------------------------

/// What the bucket says keeps `pred` through the continues relation:
/// later members the bucket holds, which this host may never have pulled.
/// Empty under the local backend, which makes no request.
pub fn bucket_keepers(backend: &dyn SessionBackend, pred: &str, clock: &Clock) -> RelationVerdict {
    match backend.as_cloud() {
        Some(cloud) => cloud.relation_walk(pred, clock, false),
        None => RelationVerdict::default(),
    }
}

/// [`bucket_keepers`] for session `pred` (id `pred_id`, when known) under
/// any workspace prefix, with `extra` passed in as later members, walked
/// like a migration marker's verified target: what `koto session prune
/// --migrated-from` asks about a migrated source. Empty under the local
/// backend.
pub fn bucket_keepers_with(
    backend: &dyn SessionBackend,
    pred: &Pointer,
    pred_id: Option<&str>,
    extra: &[Pointer],
    clock: &Clock,
) -> RelationVerdict {
    match backend.as_cloud() {
        Some(cloud) => cloud.relation_walk_with(pred, pred_id, extra, clock),
        None => RelationVerdict::default(),
    }
}

/// What keeps `member` through the relation at a removal point: the local
/// judgment's verdict, and, only when the local judgment doesn't keep the
/// member at all, the bucket's answer merged in. A member kept for its own
/// standing or by the local relation costs no bucket request.
pub fn relation_at_removal(
    backend: &dyn SessionBackend,
    judgment: &Judgment,
    member: &str,
) -> RelationVerdict {
    let mut verdict = judgment.relation_of(member).clone();
    if !judgment.keeps(member) {
        verdict.merge(bucket_keepers(
            backend,
            member,
            &Clock {
                keep: judgment.keep,
                now: judgment.now_at,
            },
        ));
    }
    verdict
}

/// [`relation_at_removal`] for a removal point that holds the local
/// relation verdict of `member` rather than a judgment (the parent sweep):
/// the bucket is asked only when the local verdict doesn't keep it.
pub fn relation_with_bucket(
    backend: &dyn SessionBackend,
    local: RelationVerdict,
    member: &str,
    clock: &Clock,
) -> RelationVerdict {
    let mut verdict = local;
    if !verdict.keeps() {
        verdict.merge(bucket_keepers(backend, member, clock));
    }
    verdict
}

/// The last check before a removal by rule deletes `name` from the bucket,
/// made after the pin guard cleared it: the guard's fresh listing of the
/// session's `ctx/koto/`, compared with the pointers the judgment saw, and
/// a walk of only the new ones (all of them when no judgment ran in this
/// pass). Empty under the local backend.
pub fn final_guard(backend: &dyn SessionBackend, name: &str) -> RelationVerdict {
    match backend.as_cloud() {
        Some(cloud) => cloud.relation_walk(name, &Clock::configured(), true),
        None => RelationVerdict::default(),
    }
}

/// Start a judgment pass: the bucket's answers this process kept from
/// earlier judgments are forgotten, so the next ones read the bucket again.
/// `--apply` calls it before it judges again.
pub fn begin_pass(backend: &dyn SessionBackend) {
    if let Some(cloud) = backend.as_cloud() {
        cloud.begin_relation_pass();
    }
}

// ---------------------------------------------------------------------
// The successor's side: what pointer it owes
// ---------------------------------------------------------------------

/// The pointer a successor's relation calls for, from its local copy alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwedPointer {
    /// The predecessor's prefix and name.
    pub pred_prefix: String,
    pub pred: String,
    /// The relation's context key.
    pub relation_key: String,
    /// The successor's own session id, the pointer's body.
    pub holder_session_id: String,
    /// The full key under which the pointer lives.
    pub key: String,
}

/// The pointer session `name` (local copy at `session_dir`, workspace
/// prefix `own_prefix`) needs beside its predecessor, when it holds a
/// relation whose predecessor prefix can be told: the prefix the relation
/// records, else the one its recorded workspace hashes to, else the
/// session's own. `None` when it holds no relation, or one that can't be
/// read.
pub fn wanted_pointer(session_dir: &Path, own_prefix: &str, name: &str) -> Option<OwedPointer> {
    let prefix = crate::session::links::continues_prefix();
    let dir = session_dir.join("ctx").join(&prefix);
    let mut preds: Vec<String> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|p| valid_name(p))
        .collect();
    preds.sort();
    let pred = preds.into_iter().next()?;
    let bytes = std::fs::read(dir.join(&pred)).ok()?;
    if bytes.len() > MAX_LINK_BODY_BYTES {
        return None;
    }
    let body: LinkBody = serde_json::from_slice(&bytes).ok()?;
    let pred_prefix = match &body.prefix {
        Prefix::Value(p) if is_workspace_prefix(p) => p.clone(),
        _ => body
            .workspace
            .as_deref()
            .filter(|w| Path::new(w).is_absolute())
            .and_then(|w| crate::session::local::prefix_for_workspace(Path::new(w)).ok())
            .unwrap_or_else(|| own_prefix.to_string()),
    };
    let holder_session_id = body
        .holder_session_id
        .clone()
        .or_else(|| {
            crate::engine::persistence::read_header(
                &session_dir.join(crate::session::state_file_name(name)),
            )
            .ok()
            .map(|h| h.session_id)
        })
        .filter(|id| !id.is_empty())
        .unwrap_or_default();
    Some(OwedPointer {
        key: pointer_key(&pred_prefix, &pred, own_prefix, name),
        pred_prefix,
        relation_key: format!("{prefix}{pred}"),
        pred,
        holder_session_id,
    })
}

/// The pointer key the session directory's mark records as confirmed.
pub fn confirmed_pointer(session_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(session_dir.join(POINTER_MARK)).ok()?;
    let key = text.trim();
    (!key.is_empty()).then(|| key.to_string())
}

/// Record that the bucket confirmed pointer `key` for the session at
/// `session_dir`.
pub fn mark_confirmed(session_dir: &Path, key: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut tmp = tempfile::Builder::new()
        .prefix(".koto-pointer-")
        .suffix(".tmp")
        .tempfile_in(session_dir)?;
    tmp.write_all(key.as_bytes())?;
    tmp.as_file().sync_all()?;
    tmp.persist(session_dir.join(POINTER_MARK))
        .map_err(|e| e.error)?;
    Ok(())
}

/// Forget the session's confirmed pointer.
pub fn clear_mark(session_dir: &Path) {
    let _ = std::fs::remove_file(session_dir.join(POINTER_MARK));
}

/// The pointer the session owes: the one its relation calls for, when the
/// mark doesn't record it as confirmed.
pub fn owed_pointer(session_dir: &Path, own_prefix: &str, name: &str) -> Option<OwedPointer> {
    let wanted = wanted_pointer(session_dir, own_prefix, name)?;
    (confirmed_pointer(session_dir).as_deref() != Some(wanted.key.as_str())).then_some(wanted)
}

/// The relation keys a push of the session leaves out: its relation, while
/// the pointer it calls for isn't confirmed. Empty when no pointer is owed.
pub fn unpushable_keys(session_dir: &Path, own_prefix: &str, name: &str) -> Vec<String> {
    owed_pointer(session_dir, own_prefix, name)
        .map(|o| vec![o.relation_key])
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
