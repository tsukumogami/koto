//! [`Judge`]: the full inspection of one component's members, and the
//! [`Verdict`] every removal point and the query share.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::graph::{Components, Edge, Graph, Scope};
use super::relation::{Reason, RelationKeeper, RelationVerdict, Standing};
use super::settings::RetentionSettings;
use super::FUTURE_TOLERANCE;
use crate::engine::types::{iso8601_at, SessionKind, ANCHOR_ACTIVITY, ANCHOR_STARTED};
use crate::session::anchor::{self, CloseCheck, CloseRecord, CLOSE_RECORD_KEY, OBSERVED_KEY};
use crate::session::context::ContextStore;
use crate::session::inventory::{inspect_full, SessionStatus, TemplateCache};
use crate::session::links::MAX_LINK_BODY_BYTES;
use crate::session::local::LocalBackend;
use crate::session::pin::PinFact;
use crate::session::state_file_name;

/// What a member is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberKind {
    /// A workflow session, or one whose header can't be read.
    KotoSession,
    /// An agent session's anchor.
    Anchor,
    /// The record that closes an anchor.
    AnchorClose,
}

impl MemberKind {
    fn of(kind: SessionKind) -> Self {
        match kind {
            SessionKind::Ordinary => MemberKind::KotoSession,
            SessionKind::Anchor => MemberKind::Anchor,
            SessionKind::AnchorClose => MemberKind::AnchorClose,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            MemberKind::KotoSession => "koto_session",
            MemberKind::Anchor => "anchor",
            MemberKind::AnchorClose => "anchor_close",
        }
    }
}

/// How a member stopped. The variant order is load-bearing: an anchor
/// whose observed and idle closes fall at the same instant stops as
/// `Observed` (see [`Judge`]'s anchor inspection).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseKind {
    /// A koto session reached a terminal state.
    Terminal,
    /// A koto session was cancelled.
    Cancelled,
    /// An anchor was closed by an observed close, or this is its close
    /// session.
    Observed,
    /// An anchor with no observed marker idled past the idle period.
    Idle,
}

impl CloseKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CloseKind::Terminal => "terminal",
            CloseKind::Cancelled => "cancelled",
            CloseKind::Observed => "observed",
            CloseKind::Idle => "idle",
        }
    }
}

/// A live member that keeps its hierarchy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keeper {
    pub name: String,
    pub kind: MemberKind,
    /// For an anchor, whether it holds an observed marker.
    pub observed: bool,
    /// An observed anchor past its last activity plus the stale threshold.
    /// A report only: it closes nothing.
    pub stale: bool,
    /// For a stale anchor whose marker and harness session id pass the
    /// close command's own checks: the command that closes it by hand.
    pub close_command: Option<String>,
    /// A stale anchor whose marker or harness session id fails those
    /// checks, so no command is printed for it.
    pub malformed_marker: bool,
}

/// The instant a hierarchy was released: the latest stop among its
/// members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInstant {
    pub at: String,
    pub member: String,
    pub close_kind: CloseKind,
}

/// When a hierarchy with nothing alive was released, and when the
/// retention passes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retention {
    pub release: ReleaseInstant,
    pub passes_at: String,
}

/// Something that couldn't be read, and keeps what it touches.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Unreadable {
    pub name: String,
    pub reason: String,
}

/// Whether a hierarchy is kept or released.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Something keeps it: a live member, the retention not yet passed
    /// since its release instant, or something unreadable.
    Keep {
        keepers: Vec<Keeper>,
        retention: Option<Retention>,
        unreadable: Vec<Unreadable>,
    },
    /// Nothing is alive and the retention has passed since the release
    /// instant. `None` when no member contributed a stop (every member is
    /// migrated), so there was nothing to wait for.
    Release { release: Option<Retention> },
}

impl Verdict {
    pub fn is_kept(&self) -> bool {
        matches!(self, Verdict::Keep { .. })
    }
}

/// One member of a judged hierarchy, as the query lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub kind: MemberKind,
    pub alive: bool,
    /// When it stopped, clamped to `now`.
    pub stopped_at: Option<String>,
    pub close_kind: Option<CloseKind>,
    /// Named by a record, with no session of that name in the store.
    pub absent: bool,
    /// Moved to a local copy, which holds what it held.
    pub migrated: bool,
    pub observed: bool,
    pub imported: bool,
    pub stale: bool,
    /// An instant read for it lay ahead of `now` and was clamped.
    pub clamped: bool,
    /// For an anchor, its last activity (clamped to `now`): the instant its
    /// idle close and its stale report count from. `None` for every other
    /// member, and for an anchor whose activity can't be read.
    pub last_activity: Option<String>,
    /// The edges that joined it to the hierarchy.
    pub joined_by: Vec<Edge>,
}

/// One judgment: the verdict for a session's hierarchy and its members.
///
/// The verdict is the component's and knows nothing of pins. A pin keeps
/// its own session and nothing else, so it is an overlay asked about one
/// member at a time: a gate that asks whether the component may go at all
/// reads `verdict.is_kept()`, and a decision to delete one member asks
/// [`Judgment::keeps`]. Asking `keeps` of a component's first member at a
/// component gate would let a pinned member keep its whole component.
#[derive(Debug, Clone)]
pub struct Judgment {
    pub session: String,
    pub verdict: Verdict,
    pub members: Vec<Member>,
    /// Members with an instant ahead of `now`, clamped for the judgment.
    pub clamped: Vec<String>,
    /// The `now` the judgment was made at.
    pub now: String,
    /// The same instant, for a judgment the bucket's answer is merged into
    /// (`crate::session::continues_bucket`), which compares a later
    /// member's stop against it.
    pub now_at: SystemTime,
    /// `retention.keep` as the judgment read it.
    pub keep: std::time::Duration,
    /// The judged session's own pin.
    pub pin: PinFact,
    /// Where that pin was read: `local`, or `bucket` when a cloud host's
    /// check of its scan found it there.
    pub pin_source: &'static str,
    /// Each present member holding a pin, readable or not, by name.
    pub pinned_members: BTreeMap<String, PinFact>,
    /// Each present member the continues relation keeps, with its
    /// keepers and what couldn't be judged ([`Judge::relation`]). Like a
    /// pin, an overlay asked about one member at a time: the verdict knows
    /// nothing of it.
    pub relations: BTreeMap<String, RelationVerdict>,
    /// The predecessor the judged session's continues link names, when it
    /// holds one.
    pub predecessor: Option<String>,
    /// The sessions holding a continues link that names the judged
    /// session, sorted.
    pub successors: Vec<String>,
}

impl Judgment {
    /// Whether `member` is kept: by its own pin, because the component's
    /// verdict keeps every member, or because a later member continuing
    /// it has standing (or a relation naming it can't be judged).
    pub fn keeps(&self, member: &str) -> bool {
        self.pinned_members.contains_key(member)
            || self.verdict.is_kept()
            || self.relation_of(member).keeps()
    }

    /// What the continues relation says about present member `member`:
    /// empty when nothing keeps it through a relation.
    pub fn relation_of(&self, member: &str) -> &RelationVerdict {
        static EMPTY: RelationVerdict = RelationVerdict::EMPTY;
        self.relations.get(member).unwrap_or(&EMPTY)
    }

    /// `member`'s pin, as the scan read it.
    pub fn pin_of(&self, member: &str) -> &PinFact {
        self.pinned_members.get(member).unwrap_or(&PinFact::None)
    }
}

/// What the full inspection learned about one present member.
#[derive(Debug, Clone)]
struct MemberState {
    kind: MemberKind,
    alive: bool,
    stop: Option<(SystemTime, CloseKind)>,
    observed: bool,
    imported: bool,
    stale: bool,
    close_command: Option<String>,
    malformed_marker: bool,
    clamped: bool,
    last_activity: Option<SystemTime>,
    unreadable: Vec<String>,
}

/// Judges hierarchies of one [`Graph`] at one `now`.
pub struct Judge<'g> {
    graph: &'g Graph,
    settings: RetentionSettings,
    now: SystemTime,
    store: LocalBackend,
    cache: TemplateCache,
    states: HashMap<String, MemberState>,
    components: HashMap<Scope, Components>,
}

impl<'g> Judge<'g> {
    /// A judge over `graph`. One `now` per command: every judgment it makes
    /// compares against the same instant.
    pub fn new(graph: &'g Graph, settings: RetentionSettings, now: SystemTime) -> Self {
        Judge {
            graph,
            store: LocalBackend::with_base_dir(graph.base().to_path_buf()),
            settings,
            now,
            cache: TemplateCache::new(),
            states: HashMap::new(),
            components: HashMap::new(),
        }
    }

    /// The members of `name`'s hierarchy under `scope`, present ones first
    /// and then the absent and migrated names, each sorted. `None` when no
    /// session `name` is in the store.
    pub fn component(&mut self, name: &str, scope: &Scope) -> Option<(Vec<String>, Vec<String>)> {
        if !self.components.contains_key(scope) {
            let c = self.graph.components(scope);
            self.components.insert(scope.clone(), c);
        }
        let comps = &self.components[scope];
        let root = comps.root_of(name)?.to_string();
        let present = comps.members(&root).map(str::to_string).collect();
        let leaves = comps.leaves(&root).map(str::to_string).collect();
        Some((present, leaves))
    }

    /// Judge `name`'s hierarchy under `scope`.
    pub fn judge(&mut self, name: &str, scope: &Scope) -> anyhow::Result<Judgment> {
        let Some((present, leaves)) = self.component(name, scope) else {
            anyhow::bail!(
                "no session named '{name}' in {}",
                self.graph.base().display()
            );
        };
        let in_component: BTreeSet<&str> = present
            .iter()
            .chain(leaves.iter())
            .map(String::as_str)
            .collect();

        let mut unreadable: BTreeSet<Unreadable> = self
            .graph
            .store_problems()
            .iter()
            .map(|(n, r)| Unreadable {
                name: n.clone(),
                reason: r.clone(),
            })
            .collect();
        let mut members = Vec::new();
        let mut keepers = Vec::new();
        let mut release: Option<(SystemTime, String, CloseKind)> = None;
        let mut clamped = Vec::new();
        let mut pinned_members = BTreeMap::new();

        for m in &present {
            let state = self.state_of(m);
            if let Some(f) = self.graph.facts(m) {
                if f.pin.is_pinned() {
                    pinned_members.insert(m.clone(), f.pin.clone());
                }
                for p in &f.problems {
                    unreadable.insert(Unreadable {
                        name: m.clone(),
                        reason: p.clone(),
                    });
                }
            }
            for p in &state.unreadable {
                unreadable.insert(Unreadable {
                    name: m.clone(),
                    reason: p.clone(),
                });
            }
            if state.clamped {
                clamped.push(m.clone());
            }
            if state.alive {
                keepers.push(Keeper {
                    name: m.clone(),
                    kind: state.kind,
                    observed: state.observed,
                    stale: state.stale,
                    close_command: state.close_command.clone(),
                    malformed_marker: state.malformed_marker,
                });
            }
            if let Some((at, kind)) = state.stop {
                let later = match &release {
                    None => true,
                    Some((cur, cur_name, _)) => at > *cur || (at == *cur && m < cur_name),
                };
                if later {
                    release = Some((at, m.clone(), kind));
                }
            }
            let migrated = self.graph.is_migrated(m);
            members.push(Member {
                name: m.clone(),
                kind: state.kind,
                alive: state.alive,
                stopped_at: state.stop.map(|(at, _)| iso8601_at(at)),
                close_kind: state.stop.map(|(_, k)| k),
                absent: false,
                migrated,
                observed: state.observed,
                imported: state.imported,
                stale: state.stale,
                clamped: state.clamped,
                last_activity: state.last_activity.map(iso8601_at),
                joined_by: self.joined_by(m, scope, &in_component),
            });
        }
        for leaf in &leaves {
            let migrated = self.graph.is_migrated(leaf);
            let kind = self
                .graph
                .facts(leaf)
                .map(|f| MemberKind::of(f.kind))
                .unwrap_or(MemberKind::KotoSession);
            members.push(Member {
                name: leaf.clone(),
                kind,
                alive: false,
                stopped_at: None,
                close_kind: None,
                absent: !migrated,
                migrated,
                observed: false,
                imported: false,
                stale: false,
                clamped: false,
                last_activity: None,
                joined_by: self.joined_by(leaf, scope, &in_component),
            });
        }

        let released_at = release.as_ref().map(|(at, _, _)| *at);
        let retention = release.map(|(at, member, close_kind)| Retention {
            passes_at: iso8601_at(at + self.settings.keep.value),
            release: ReleaseInstant {
                at: iso8601_at(at),
                member,
                close_kind,
            },
        });
        let mut relations = BTreeMap::new();
        for m in &present {
            let r = self.relation(m);
            if r.keeps() {
                relations.insert(m.clone(), r);
            }
        }
        let unreadable: Vec<Unreadable> = unreadable.into_iter().collect();
        let verdict = if !keepers.is_empty() {
            Verdict::Keep {
                keepers,
                retention: None,
                unreadable,
            }
        } else if !unreadable.is_empty() {
            Verdict::Keep {
                keepers,
                retention,
                unreadable,
            }
        } else {
            let passed = released_at.is_none_or(|at| at + self.settings.keep.value <= self.now);
            if passed {
                Verdict::Release { release: retention }
            } else {
                Verdict::Keep {
                    keepers,
                    retention,
                    unreadable,
                }
            }
        };
        Ok(Judgment {
            session: name.to_string(),
            verdict,
            members,
            clamped,
            now: iso8601_at(self.now),
            now_at: self.now,
            keep: self.settings.keep.value,
            pin: pinned_members.get(name).cloned().unwrap_or_default(),
            pin_source: self
                .graph
                .facts(name)
                .map_or(crate::session::pin::SOURCE_LOCAL, |f| f.pin_source()),
            pinned_members,
            relations,
            predecessor: self.graph.predecessor_of(name).map(str::to_string),
            successors: self.graph.successors_of(name),
        })
    }

    /// What the continues relation says about session `name`
    /// (DESIGN-koto-session-continues.md, Decision 2): every later member
    /// with standing, each with the path to it, and every relation on the
    /// way that can't be judged.
    ///
    /// The walk follows the graph's directed list backwards, breadth-first
    /// from `name` with a visited set, so it reaches only later members:
    /// nothing earlier than `name` is read, and nothing earlier keeps it.
    /// Each member's standing comes from that member alone, never its
    /// hierarchy ([`Self::standing`]); the walk goes on past a member with
    /// no standing, since something later may have some. A member reached
    /// again is a `cycle`, which keeps. Every session whose reserved area
    /// can't be listed is `unlistable`, which keeps too: it may hold a
    /// relation naming anything.
    pub fn relation(&mut self, name: &str) -> RelationVerdict {
        use std::collections::VecDeque;
        let mut verdict = RelationVerdict::default();
        for holder in &self.graph.unlistable {
            if holder != name {
                verdict.unreadable.push(super::relation::Unreadable {
                    holder: holder.clone(),
                    reason: Reason::Unlistable,
                });
            }
        }
        let mut visited: BTreeSet<String> = BTreeSet::from([name.to_string()]);
        let mut queue: VecDeque<(String, Vec<String>)> =
            VecDeque::from([(name.to_string(), Vec::new())]);
        while let Some((node, path)) = queue.pop_front() {
            if let Some(broken) = self.graph.broken.get(&node) {
                verdict.unreadable.extend(broken.iter().cloned());
            }
            let holders: Vec<String> = self
                .graph
                .directed
                .iter()
                .filter(|d| d.target == node)
                .map(|d| d.holder.clone())
                .collect();
            for holder in holders {
                if !visited.insert(holder.clone()) {
                    verdict.unreadable.push(super::relation::Unreadable {
                        holder,
                        reason: Reason::Cycle,
                    });
                    continue;
                }
                if let Some(keeper) = self.standing(&holder, &path) {
                    verdict.keepers.push(keeper);
                }
                let mut next = path.clone();
                next.push(holder.clone());
                queue.push_back((holder, next));
            }
        }
        verdict.normalize();
        verdict
    }

    /// Later member `name`'s standing, judged from it alone: its pin
    /// (readable or not, as the scan read it or a cloud host's check found
    /// it), then alive (it runs, or can't be read), then within
    /// `retention.keep` of its own stop. `None` for no standing: a
    /// migrated member, whose records live in its copy, and a member whose
    /// session directory is gone (removed earlier in the same run).
    fn standing(&mut self, name: &str, path: &[String]) -> Option<RelationKeeper> {
        let graph = self.graph;
        let facts = graph.facts(name)?;
        let gone = matches!(
            fs::symlink_metadata(graph.base().join(name)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound
        );
        if gone {
            return None;
        }
        let keeper = |standing, retention_passes_at, pin: PinFact| RelationKeeper {
            keeper: name.to_string(),
            standing,
            retention_passes_at,
            pin,
            pin_source: facts.pin_source(),
            path: path.to_vec(),
        };
        if facts.pin.is_pinned() {
            return Some(keeper(Standing::Pin, None, facts.pin.clone()));
        }
        if graph.is_migrated(name) {
            return None;
        }
        let state = self.state_of(name);
        if state.alive {
            return Some(keeper(Standing::Alive, None, PinFact::None));
        }
        let (stop, _) = state.stop?;
        let passes = stop + self.settings.keep.value;
        (passes > self.now)
            .then(|| keeper(Standing::Retention, Some(iso8601_at(passes)), PinFact::None))
    }

    /// Whether session `name` is in the store and alive, by the same full
    /// inspection a judgment makes of it (an anchor: no close of it has
    /// passed). A migrated session is never alive; one that can't be read
    /// is, as everywhere else.
    pub fn is_alive(&mut self, name: &str) -> bool {
        self.graph.contains(name) && self.state_of(name).alive
    }

    /// The edges inside the component that touch `name`.
    fn joined_by(&self, name: &str, scope: &Scope, in_component: &BTreeSet<&str>) -> Vec<Edge> {
        let keep = |e: &&Edge| match scope {
            Scope::Whole => true,
            Scope::DropChildLineageOf(root) => {
                !(e.via == super::graph::Via::Lineage && &e.target == root)
            }
        };
        self.graph
            .edges()
            .iter()
            .filter(keep)
            .filter(|e| e.holder == name || e.target == name)
            .filter(|e| in_component.contains(e.holder.as_str()))
            .cloned()
            .collect()
    }

    /// The full inspection of present member `name`, read once per judge.
    fn state_of(&mut self, name: &str) -> MemberState {
        if let Some(s) = self.states.get(name) {
            return s.clone();
        }
        let state = self.inspect(name);
        self.states.insert(name.to_string(), state.clone());
        state
    }

    /// Clamp an instant read from disk to `now`, noting a clamp and
    /// tainting one more than [`FUTURE_TOLERANCE`] ahead.
    fn clamp(&self, at: SystemTime, what: &str, state: &mut MemberState) -> SystemTime {
        if at > self.now {
            state.clamped = true;
            if at > self.now + FUTURE_TOLERANCE {
                state.unreadable.push(format!(
                    "its {what} ({}) is more than five minutes in the future",
                    iso8601_at(at)
                ));
            }
            self.now
        } else {
            at
        }
    }

    fn inspect(&mut self, name: &str) -> MemberState {
        let facts = self.graph.facts(name);
        let kind = facts
            .map(|f| MemberKind::of(f.kind))
            .unwrap_or(MemberKind::KotoSession);
        let mut state = MemberState {
            kind,
            alive: false,
            stop: None,
            observed: facts.is_some_and(|f| f.observed),
            imported: facts.is_some_and(|f| f.imported_from.is_some()),
            stale: false,
            close_command: None,
            malformed_marker: false,
            clamped: false,
            last_activity: None,
            unreadable: Vec::new(),
        };
        // A migrated session's records live in its copy: it keeps nothing
        // and contributes no stop.
        if self.graph.is_migrated(name) {
            return state;
        }
        // A session whose header can't be read may be anything: alive.
        if facts.is_none_or(|f| !f.store_problems.is_empty()) {
            state.alive = true;
            return state;
        }
        match kind {
            MemberKind::KotoSession => self.inspect_koto_session(name, &mut state),
            MemberKind::Anchor => self.inspect_anchor(name, &mut state),
            MemberKind::AnchorClose => self.inspect_close_session(name, &mut state),
        }
        state
    }

    fn inspect_koto_session(&mut self, name: &str, state: &mut MemberState) {
        let (seen, _) = inspect_full(self.graph.base(), name, &mut self.cache);
        let close_kind = match seen.status {
            SessionStatus::Running => {
                state.alive = true;
                return;
            }
            SessionStatus::Unreadable => {
                state.alive = true;
                state.unreadable.push("its log can't be read".to_string());
                return;
            }
            SessionStatus::Terminal => CloseKind::Terminal,
            SessionStatus::Cancelled => CloseKind::Cancelled,
        };
        match seen
            .stopped_at
            .as_deref()
            .and_then(crate::engine::wake::parse_rfc3339_millis)
        {
            Some(at) => {
                let at = self.clamp(at, "stop", state);
                state.stop = Some((at, close_kind));
            }
            None => {
                state.alive = true;
                state
                    .unreadable
                    .push("its stop instant can't be read".to_string());
            }
        }
    }

    fn inspect_close_session(&mut self, name: &str, state: &mut MemberState) {
        let record = self
            .store
            .meta(name, CLOSE_RECORD_KEY)
            .and_then(|_| self.store.get(name, CLOSE_RECORD_KEY).ok())
            .filter(|b| b.len() <= MAX_LINK_BODY_BYTES)
            .and_then(|b| serde_json::from_slice::<CloseRecord>(&b).ok());
        match record.and_then(|r| crate::engine::wake::parse_rfc3339_millis(&r.at)) {
            Some(at) => {
                let at = self.clamp(at, "close record", state);
                state.stop = Some((at, CloseKind::Observed));
            }
            None => state
                .unreadable
                .push("its close record can't be read".to_string()),
        }
    }

    fn inspect_anchor(&mut self, name: &str, state: &mut MemberState) {
        let path = self.graph.base().join(name).join(state_file_name(name));
        let log = match scan_anchor_log(&path) {
            Ok(log) => log,
            Err(why) => {
                state.alive = true;
                state.unreadable.push(why);
                return;
            }
        };
        let created = self
            .graph
            .facts(name)
            .and_then(|f| f.created_at.as_deref())
            .and_then(crate::engine::wake::parse_rfc3339_millis);
        let Some(last) = log.last_activity.or(created) else {
            state.alive = true;
            state
                .unreadable
                .push("its last activity can't be read".to_string());
            return;
        };
        let last = self.clamp(last, "last activity", state);
        state.last_activity = Some(last);

        let mut candidates: Vec<(SystemTime, CloseKind)> = Vec::new();
        match anchor::checked_close_record(&self.store, name) {
            (CloseCheck::Closed, Some(record)) => {
                match crate::engine::wake::parse_rfc3339_millis(&record.at) {
                    Some(at) => {
                        let at = self.clamp(at, "observed close", state);
                        candidates.push((at, CloseKind::Observed));
                    }
                    None => state.unreadable.push(format!(
                        "its close session {} has a close time that can't be read",
                        anchor::close_name(name)
                    )),
                }
            }
            (CloseCheck::Tainted(why), _) => state.unreadable.push(format!(
                "its close session {} doesn't count: {why}",
                anchor::close_name(name)
            )),
            _ => {}
        }
        // Idle close applies only to an anchor with no observed marker: one
        // never observed, or an imported one, whose marker import clears.
        if !state.observed {
            candidates.push((last + self.settings.idle_period.value, CloseKind::Idle));
        }
        // The earliest candidate that has passed is the stop. On a tie the
        // kind breaks it by `CloseKind`'s order, so an observed close wins
        // over an idle one at the same instant: the record is the stronger
        // statement of why the anchor closed.
        let closed = candidates
            .iter()
            .filter(|(at, _)| *at <= self.now)
            .min_by_key(|(at, kind)| (*at, *kind))
            .copied();
        if let Some(stop) = closed {
            state.stop = Some(stop);
            return;
        }
        state.alive = true;
        if state.observed && last + self.settings.stale_after.value <= self.now {
            state.stale = true;
            let marker = self
                .store
                .get(name, OBSERVED_KEY)
                .ok()
                .filter(|b| b.len() <= MAX_LINK_BODY_BYTES)
                .and_then(|b| serde_json::from_slice::<MarkerKey>(&b).ok())
                .map(|m| m.key);
            match (marker, log.harness_session) {
                (Some(key), Some(harness)) => match super::render::close_command(&key, &harness) {
                    Some(cmd) => state.close_command = Some(cmd),
                    None => state.malformed_marker = true,
                },
                _ => state.malformed_marker = true,
            }
        }
    }
}

/// The key of an observed marker, whatever else it holds.
#[derive(Deserialize)]
struct MarkerKey {
    key: String,
}

/// What an anchor's own log says about its agent session.
#[derive(Debug, Default)]
struct AnchorLog {
    /// The latest of its `anchor_started`, `anchor_activity`,
    /// `session_imported` events and its own read records (`context_read`
    /// events carrying the `session` they read). Events other processes
    /// write never count: a read of the anchor itself carries no `session`.
    last_activity: Option<SystemTime>,
    /// The harness session id its first `anchor_started` recorded.
    harness_session: Option<String>,
}

/// The fields of a log line the activity scan reads.
#[derive(Deserialize)]
struct ActivityLine {
    timestamp: String,
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    payload: ActivityPayload,
}

#[derive(Deserialize, Default)]
struct ActivityPayload {
    #[serde(default)]
    session: Option<serde_json::Value>,
    #[serde(default)]
    harness_session: Option<String>,
}

const SESSION_IMPORTED: &str = "session_imported";
const CONTEXT_READ: &str = "context_read";

/// Read an anchor's activity from its log. A line that names one of the
/// activity events but can't be parsed makes the log unreadable, except a
/// final line with no newline, which is an append still in progress.
fn scan_anchor_log(path: &Path) -> Result<AnchorLog, String> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_file() => {}
        _ => return Err("its log can't be read".to_string()),
    }
    let file = fs::File::open(path).map_err(|_| "its log can't be read".to_string())?;
    let mut reader = BufReader::new(file);
    let mut log = AnchorLog::default();
    let mut line = String::new();
    let mut first = true;
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .map_err(|_| "its log can't be read".to_string())?;
        if n == 0 {
            break;
        }
        if first {
            first = false;
            continue;
        }
        let complete = line.ends_with('\n');
        let candidate = line.contains(ANCHOR_STARTED)
            || line.contains(ANCHOR_ACTIVITY)
            || line.contains(SESSION_IMPORTED)
            || line.contains(CONTEXT_READ);
        if !candidate {
            continue;
        }
        let parsed = serde_json::from_str::<ActivityLine>(line.trim_end())
            .ok()
            .and_then(|l| {
                crate::engine::wake::parse_rfc3339_millis(&l.timestamp).map(|at| (l, at))
            });
        let Some((event, at)) = parsed else {
            if complete {
                return Err("its log holds an event that can't be read".to_string());
            }
            continue;
        };
        let counts = match event.event_type.as_str() {
            ANCHOR_STARTED => {
                if log.harness_session.is_none() {
                    log.harness_session = event.payload.harness_session.clone();
                }
                true
            }
            ANCHOR_ACTIVITY | SESSION_IMPORTED => true,
            CONTEXT_READ => event
                .payload
                .session
                .as_ref()
                .is_some_and(|s| s.as_str().is_some_and(|s| !s.is_empty())),
            _ => false,
        };
        if counts && log.last_activity.is_none_or(|cur| at > cur) {
            log.last_activity = Some(at);
        }
    }
    Ok(log)
}
