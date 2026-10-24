//! [`Graph::scan`]: one pass over the store building the undirected edge
//! list, the migrated set and the components.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Context as _;
use serde::Deserialize;

use super::facts::SessionFacts;
use super::lineage_target;
use crate::session::state_file_name;
use crate::session::validate::validate_session_id;

/// What joined two sessions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Via {
    /// A link the holder keeps, by its kind's key component.
    Link(String),
    /// The holder's header `parent_workflow`.
    Lineage,
    /// The holder's `chain/parent` context key.
    ChainParent,
    /// A link's session-id hint naming a session other than the one its
    /// name resolves to (a predecessor moved aside under a new name).
    SessionIdHint(String),
}

impl Via {
    /// How a report names this edge.
    pub fn describe(&self) -> String {
        match self {
            Via::Link(kind) => format!("{kind} link"),
            Via::Lineage => "parent_workflow".to_string(),
            Via::ChainParent => "chain/parent".to_string(),
            Via::SessionIdHint(kind) => format!("{kind} link's session-id hint"),
        }
    }
}

/// One edge: `holder` keeps the record that names `named`, which resolved
/// to `target`. `target` is `named` itself unless `named` was migrated (the
/// local copy), or a session-id hint pointed elsewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub holder: String,
    pub named: String,
    pub target: String,
    pub via: Via,
}

/// Which edges a judgment counts.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Scope {
    /// Every edge.
    Whole,
    /// Every edge except the lineage between this session and its direct
    /// children (the sessions whose `parent_workflow` resolves to it):
    /// `koto workspace prune --root X` judges X's subtree this way, so the
    /// person who named X can dissolve it while a grandchild under a kept
    /// parent stays kept.
    DropChildLineageOf(String),
}

impl Scope {
    fn keeps(&self, edge: &Edge) -> bool {
        match self {
            Scope::Whole => true,
            Scope::DropChildLineageOf(root) => !(edge.via == Via::Lineage && &edge.target == root),
        }
    }
}

/// The store as one undirected graph.
#[derive(Debug)]
pub struct Graph {
    pub(super) base: PathBuf,
    /// Every session in the store, by name.
    pub(super) facts: BTreeMap<String, SessionFacts>,
    pub(super) edges: Vec<Edge>,
    /// Each migrated session, with the local copies that name it.
    pub(super) migrated: BTreeMap<String, Vec<String>>,
    /// `(session, reason)` for every problem that taints every component.
    pub(super) store_problems: Vec<(String, String)>,
}

/// The names of every session under `base`: each directory entry whose
/// name is a valid session id and that holds its state file. Dot entries
/// (stored plans, close staging) are never sessions. A missing directory
/// holds none; one that can't be listed is an error.
pub(crate) fn session_names(base: &Path) -> anyhow::Result<Vec<String>> {
    let entries = match fs::read_dir(base) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("failed to list {}", base.display())),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("failed to list {}", base.display()))?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if name.starts_with('.') || validate_session_id(&name).is_err() {
            continue;
        }
        let state = base.join(&name).join(state_file_name(&name));
        if fs::symlink_metadata(&state).is_ok()
            || fs::symlink_metadata(base.join(&name)).is_ok_and(|m| m.file_type().is_symlink())
        {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

/// The `session_imported` fields the migration check compares.
#[derive(Deserialize)]
struct ImportedEvent {
    timestamp: String,
    #[serde(rename = "type")]
    event_type: String,
    payload: ImportedPayload,
}

#[derive(Deserialize)]
struct ImportedPayload {
    from_session: Option<String>,
    from_session_id: Option<String>,
}

/// The timestamp of the `session_imported` event in the log at `path` that
/// names `from` with id `from_id`, the latest when there are several.
fn import_instant(path: &Path, from: &str, from_id: &str) -> Option<SystemTime> {
    let file = fs::File::open(path).ok()?;
    let mut found = None;
    for line in BufReader::new(file).lines().skip(1) {
        let Ok(line) = line else { break };
        if !line.contains("\"session_imported\"") {
            continue;
        }
        let Ok(event) = serde_json::from_str::<ImportedEvent>(&line) else {
            continue;
        };
        if event.event_type == "session_imported"
            && event.payload.from_session.as_deref() == Some(from)
            && event.payload.from_session_id.as_deref() == Some(from_id)
        {
            found = crate::engine::wake::parse_rfc3339_millis(&event.timestamp).or(found);
        }
    }
    found
}

/// The timestamp of the last event in the log at `path`.
pub(super) fn last_event_instant(path: &Path) -> Option<SystemTime> {
    let text = fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = text.lines().skip(1).collect();
    let line = lines.into_iter().rev().find(|l| !l.trim().is_empty())?;
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    crate::engine::wake::parse_rfc3339_millis(value.get("timestamp")?.as_str()?)
}

impl Graph {
    /// Read every session under `base` and build the graph. Fails only when
    /// the sessions directory can't be listed; anything else unreadable is
    /// recorded as a problem that keeps what it touches.
    pub fn scan(base: &Path) -> anyhow::Result<Graph> {
        let names = session_names(base)?;
        let mut facts = BTreeMap::new();
        for name in names {
            let f = SessionFacts::read(base, &name);
            facts.insert(name, f);
        }
        let store_problems = facts
            .values()
            .flat_map(|f| f.store_problems.iter().map(|p| (f.name.clone(), p.clone())))
            .collect();
        let migrated = Self::find_migrated(base, &facts);
        let mut graph = Graph {
            base: base.to_path_buf(),
            facts,
            edges: Vec::new(),
            migrated,
            store_problems,
        };
        graph.build_edges();
        Ok(graph)
    }

    /// The migrated set. Session X counts as migrated only when a local
    /// copy's `koto/imported-from` names it with `from_session_id` equal to
    /// X's header id, the copy's log holds the matching `session_imported`
    /// event, and X's last event is no later than that event: a live X keeps
    /// appending, so a forged record can't mark it gone.
    fn find_migrated(
        base: &Path,
        facts: &BTreeMap<String, SessionFacts>,
    ) -> BTreeMap<String, Vec<String>> {
        let mut migrated: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for copy in facts.values() {
            let Some(from) = &copy.imported_from else {
                continue;
            };
            if from.from_session == copy.name {
                continue;
            }
            let Some(source) = facts.get(&from.from_session) else {
                continue;
            };
            if source.session_id.as_deref() != Some(from.from_session_id.as_str()) {
                continue;
            }
            let copy_log = base.join(&copy.name).join(state_file_name(&copy.name));
            let Some(imported_at) =
                import_instant(&copy_log, &from.from_session, &from.from_session_id)
            else {
                continue;
            };
            let source_log = base.join(&source.name).join(state_file_name(&source.name));
            let Some(last) = last_event_instant(&source_log) else {
                continue;
            };
            if last <= imported_at {
                migrated
                    .entry(source.name.clone())
                    .or_default()
                    .push(copy.name.clone());
            }
        }
        migrated
    }

    /// The join rules. `super::edge_of` repeats them for the terminal tick
    /// and must stay a superset of what is joined here; see its doc.
    fn build_edges(&mut self) {
        let mut by_id: HashMap<&str, Vec<&str>> = HashMap::new();
        for f in self.facts.values() {
            if let Some(id) = &f.session_id {
                by_id.entry(id.as_str()).or_default().push(f.name.as_str());
            }
        }
        let mut edges = Vec::new();
        for f in self.facts.values() {
            // A migrated session's own records travelled with its copy, which
            // carries them; it joins nothing itself.
            if self.migrated.contains_key(&f.name) {
                continue;
            }
            let mut add = |named: &str, via: Via| {
                for target in self.resolve(named) {
                    edges.push(Edge {
                        holder: f.name.clone(),
                        named: named.to_string(),
                        target,
                        via: via.clone(),
                    });
                }
            };
            for link in &f.links {
                add(&link.peer, Via::Link(link.kind.clone()));
            }
            if let Some(pw) = &f.parent_workflow {
                let target = lineage_target(pw, |n| self.facts.contains_key(n));
                add(&target, Via::Lineage);
            }
            if let Some(p) = &f.chain_parent {
                add(p, Via::ChainParent);
            }
            for link in &f.links {
                let Some(id) = &link.session_id else { continue };
                for other in by_id.get(id.as_str()).into_iter().flatten() {
                    if *other != link.peer && *other != f.name {
                        edges.push(Edge {
                            holder: f.name.clone(),
                            named: link.peer.clone(),
                            target: other.to_string(),
                            via: Via::SessionIdHint(link.kind.clone()),
                        });
                    }
                }
            }
        }
        edges.retain(|e| e.target != e.holder);
        edges.sort_by(|a, b| {
            (&a.holder, &a.target, &a.via, &a.named).cmp(&(&b.holder, &b.target, &b.via, &b.named))
        });
        edges.dedup();
        self.edges = edges;
    }

    /// What a name a record carries resolves to: the local copies of a
    /// migrated session plus the migrated name itself (a leaf), the session
    /// itself, or, when no session has the name, the absent name (a leaf).
    fn resolve(&self, named: &str) -> Vec<String> {
        match self.migrated.get(named) {
            Some(copies) => copies
                .iter()
                .cloned()
                .chain(std::iter::once(named.to_string()))
                .collect(),
            None => vec![named.to_string()],
        }
    }

    /// The sessions directory this graph was read from.
    pub fn base(&self) -> &Path {
        &self.base
    }

    /// Whether a session named `name` is in the store.
    pub fn contains(&self, name: &str) -> bool {
        self.facts.contains_key(name)
    }

    /// What the scan read about `name`.
    pub fn facts(&self, name: &str) -> Option<&SessionFacts> {
        self.facts.get(name)
    }

    /// Every session in the store, by name.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.facts.keys().map(String::as_str)
    }

    /// Replace the pin the scan read for `name` with what a cloud host's
    /// bucket check found (`crate::session::pin::reconcile`): the bucket's
    /// pin object (`in_bucket`), or no pin for one released on another
    /// host. Pins never change an edge, a component or a release instant,
    /// so nothing else the scan read moves. A name not in the store is
    /// left out.
    pub(crate) fn set_pin(
        &mut self,
        name: &str,
        pin: crate::session::pin::PinFact,
        in_bucket: bool,
    ) {
        if let Some(facts) = self.facts.get_mut(name) {
            facts.pin = pin;
            facts.pin_in_bucket = in_bucket;
        }
    }

    /// Every edge, sorted.
    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    /// Whether `name` is migrated: a local copy holds what it held.
    pub fn is_migrated(&self, name: &str) -> bool {
        self.migrated.contains_key(name)
    }

    /// Whether `name` is a node that can join sessions: present and not
    /// migrated. Absent and migrated names are leaves.
    pub(super) fn is_node(&self, name: &str) -> bool {
        self.facts.contains_key(name) && !self.migrated.contains_key(name)
    }

    /// The problems that taint every component.
    pub fn store_problems(&self) -> &[(String, String)] {
        &self.store_problems
    }

    /// The components under `scope`.
    pub fn components(&self, scope: &Scope) -> Components {
        let names: Vec<&str> = self.facts.keys().map(String::as_str).collect();
        let index: HashMap<&str, usize> = names.iter().enumerate().map(|(i, n)| (*n, i)).collect();
        let mut uf = UnionFind::new(names.len());
        for edge in self.edges.iter().filter(|e| scope.keeps(e)) {
            if self.is_node(&edge.holder) && self.is_node(&edge.target) {
                uf.union(index[edge.holder.as_str()], index[edge.target.as_str()]);
            }
        }
        let mut root_of = BTreeMap::new();
        for (i, name) in names.iter().enumerate() {
            root_of.insert(name.to_string(), names[uf.find(i)].to_string());
        }
        let mut members: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (name, root) in &root_of {
            members
                .entry(root.clone())
                .or_default()
                .insert(name.clone());
        }
        let mut leaves: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for edge in self.edges.iter().filter(|e| scope.keeps(e)) {
            if self.is_node(&edge.holder) && !self.is_node(&edge.target) {
                let root = &root_of[&edge.holder];
                leaves
                    .entry(root.clone())
                    .or_default()
                    .insert(edge.target.clone());
            }
        }
        Components {
            root_of,
            members,
            leaves,
        }
    }
}

/// A partition of the store into hierarchies.
#[derive(Debug, Clone)]
pub struct Components {
    root_of: BTreeMap<String, String>,
    members: BTreeMap<String, BTreeSet<String>>,
    leaves: BTreeMap<String, BTreeSet<String>>,
}

impl Components {
    /// The representative of `name`'s component, or `None` for a name not
    /// in the store.
    pub fn root_of(&self, name: &str) -> Option<&str> {
        self.root_of.get(name).map(String::as_str)
    }

    /// The sessions of the component `root` represents.
    pub fn members(&self, root: &str) -> impl Iterator<Item = &str> {
        self.members
            .get(root)
            .into_iter()
            .flatten()
            .map(String::as_str)
    }

    /// The absent and migrated names the component's records name.
    pub fn leaves(&self, root: &str) -> impl Iterator<Item = &str> {
        self.leaves
            .get(root)
            .into_iter()
            .flatten()
            .map(String::as_str)
    }
}

/// Union-find over node indices, with path halving and union by size.
struct UnionFind {
    parent: Vec<usize>,
    size: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind {
            parent: (0..n).collect(),
            size: vec![1; n],
        }
    }

    fn find(&mut self, mut i: usize) -> usize {
        while self.parent[i] != i {
            self.parent[i] = self.parent[self.parent[i]];
            i = self.parent[i];
        }
        i
    }

    fn union(&mut self, a: usize, b: usize) {
        let (mut a, mut b) = (self.find(a), self.find(b));
        if a == b {
            return;
        }
        if self.size[a] < self.size[b] {
            std::mem::swap(&mut a, &mut b);
        }
        self.parent[b] = a;
        self.size[a] += self.size[b];
    }
}
