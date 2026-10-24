//! `koto workspace prune` -- operator-facing workspace reclaim verb.
//!
//! Reads the root header, validates the workflow has reached a terminal
//! state (`completed` or `abandoned`), walks descendants via
//! `backend.list()` + parent filter, judges the root and every descendant
//! by the retention rule (`crate::session::hierarchy`), and after operator
//! confirmation reclaims only the members the rule releases, listing the
//! rest with their reasons. Symlinked roots reject via `lstat()` before any
//! directory traversal; `fs::remove_dir_all` (the underlying reclaim
//! primitive) does not follow symlinks inside the descendant tree, so
//! a symlink whose target lives outside `~/.koto/` cannot be removed
//! through this verb.
//!
//! The verb intentionally does NOT consult `coordinator_of_record`:
//! Request-store workspaces can be pruned by any operator regardless of which
//! coordinator (if any) is currently dispatching to the tree (Decision
//! 4 line 578). It also does NOT mutate
//! `~/.koto/_terminal_index.jsonl` -- Issue 9 owns terminal-index
//! compaction.
//!
//! TODO(issue-3): switch the `--root` validator to `ValidatedSessionId::new`
//! once Issue 3 lands. The current site reuses `validate_session_id()`
//! from `src/session/validate.rs` which already implements the same
//! character allowlist; the refactor is type-signature only.

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::json;

use crate::engine::persistence::derive_machine_state;
use crate::engine::types::{EventPayload, StateFileHeader};
use crate::session::hierarchy::{
    self, Graph, Judge, MemberKind, RetentionSettings, Scope, Verdict,
};
use crate::session::{validate::validate_session_id, SessionBackend, SessionInfo};
use crate::template::types::CompiledTemplate;

use super::exit_with_error_code;
use super::next_types::KeptChild;

/// Outcome of the terminal-state gate.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TerminalStatus {
    /// Workflow reached a terminal state in the compiled template.
    Completed,
    /// Workflow was cancelled (a `WorkflowCancelled` event is in the log).
    Abandoned,
    /// Workflow has not reached a terminal state; the variant carries
    /// the derived current state name for operator-facing messaging.
    NonTerminal { current_state: String },
}

impl TerminalStatus {
    fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Abandoned)
    }

    fn describe(&self) -> String {
        match self {
            Self::Completed => "completed".to_string(),
            Self::Abandoned => "abandoned".to_string(),
            Self::NonTerminal { current_state } => {
                format!("not terminal (current state: {})", current_state)
            }
        }
    }
}

/// Handle `koto workspace prune --root <id> [--dry-run] [--yes] [--force]`.
///
/// The root and its descendants are each judged by the retention rule
/// (see [`scope_for`]); only the members it releases are removed, and the
/// rest are listed with their reasons. `--force` passes the terminal-state
/// gate, never the rule.
///
/// Returns on success; on caller errors (invalid root, non-terminal
/// without `--force`, symlinked root, declined confirmation, a malformed
/// retention setting) calls `exit_with_error_code` and never returns.
pub fn handle_prune(
    backend: &crate::session::Backend,
    root: String,
    dry_run: bool,
    yes: bool,
    force: bool,
) -> Result<()> {
    // 1. Parse-time validation. Reject injection attempts before any
    //    filesystem operation.
    if let Err(e) = validate_session_id(&root) {
        exit_with_error_code(
            json!({
                "error": format!("invalid --root: {}", e),
                "command": "workspace prune",
            }),
            2,
        );
    }

    // 2. Symlink refusal: `lstat()` the root session directory BEFORE
    //    opening anything. A symlinked root is a workspace-escape
    //    vector and must be rejected categorically.
    let root_dir = backend.session_dir(&root);
    reject_if_symlink(&root_dir);

    // 3. Existence check. Operator may have typoed the id.
    if !backend.exists(&root) {
        exit_with_error_code(
            json!({
                "error": format!("session '{}' not found", root),
                "command": "workspace prune",
            }),
            2,
        );
    }

    // 4. Read header + events; derive terminal status.
    let (header, events) = backend
        .read_events(&root)
        .map_err(|e| anyhow::anyhow!("failed to read state file for '{}': {}", root, e))?;
    let status = derive_terminal_status(&header, &events, &root_dir)?;

    // 5. Terminal-state gate.
    if !status.is_terminal() && !force {
        exit_with_error_code(
            json!({
                "error": format!(
                    "session '{}' is {}; use --force to prune anyway",
                    root,
                    status.describe()
                ),
                "command": "workspace prune",
            }),
            2,
        );
    }

    // 6. Enumerate descendants via backend.list() + parent filter.
    //    Includes transitive descendants, each listed after its parent.
    let all_sessions = backend
        .list()
        .with_context(|| "failed to list sessions for descendant walk")?;
    let descendants = collect_descendants(&root, &all_sessions);
    // A pinned child the root's tree names but its lineage no longer
    // reaches: listed as kept, never removed (it isn't in the subtree).
    let by_name: Vec<String> = pinned_children_by_name(backend, &root, &descendants)
        .into_iter()
        .map(|(n, _)| n)
        .collect();

    // 7. Judge every member of the subtree by the retention rule, over one
    //    scan of the store. `--force` passes the terminal-state gate above
    //    and never this rule.
    let settings = RetentionSettings::load().unwrap_or_else(|e| {
        exit_with_error_code(
            json!({
                "error": format!("{e:#}; nothing was judged or removed"),
                "command": "workspace prune",
            }),
            2,
        )
    });
    let graph = Graph::scan(backend.local_base_dir()).unwrap_or_else(|e| {
        exit_with_error_code(
            json!({
                "error": format!(
                    "the sessions directory can't be read, so nothing was judged or removed: {e:#}"
                ),
                "command": "workspace prune",
            }),
            3,
        )
    });
    let now = std::time::SystemTime::now();
    let plan = judge_subtree(&graph, &settings, now, &root, &descendants, &by_name);

    // 8. Compute non-terminal sessions in the to-be-pruned set. Operator
    //    visibility before any confirmation prompt.
    let non_terminal_in_set = non_terminal_sessions(
        backend,
        std::iter::once(root.as_str()).chain(descendants.iter().map(String::as_str)),
    );

    // 9. Print preview (what goes, what stays and why, non-terminal
    //    warnings).
    print_preview(&root, &plan, &non_terminal_in_set, &status);

    // 10. Dry-run exits 0 here without reclaiming.
    if dry_run {
        return Ok(());
    }

    // 10. Confirmation prompt. `--yes` skips. Issue 18 will plumb
    //     `KOTO_REQUEST_STORE_PRUNE_CONFIRM=1` as another bypass through this
    //     same `prompt_required` parameter.
    let prompt_required = !yes;
    if !confirm_prune(prompt_required)? {
        exit_with_error_code(
            json!({
                "error": "prune aborted by operator",
                "command": "workspace prune",
            }),
            2,
        );
    }

    // 10b. Second confirmation when --force is combined with --yes.
    //      --yes alone covers the normal --terminal path; --force
    //      bypasses the terminal-state gate and is destructive enough
    //      to warrant a second explicit gate even when the operator
    //      pre-consented. Requires typing the literal string
    //      "force-prune" — exact match, no fuzziness, no case
    //      insensitivity. EOF on stdin is treated as negative consent.
    if yes && force && !confirm_force_prune()? {
        exit_with_error_code(
            json!({
                "error": "force-prune aborted: confirmation phrase not entered",
                "command": "workspace prune",
            }),
            2,
        );
    }

    // 11. Reclaim the released members. Descendants first, deepest first,
    //     and the root last, so a partial failure leaves every remaining
    //     session under a parent that still exists and the root visible in
    //     `koto workflows`. Each is judged again under its own lock first.
    let Reclaimed {
        removed,
        skipped,
        failed,
    } = reclaim(backend, &graph, &settings, now, &root, &plan.remove);
    let root_removed = removed.contains(&root);

    // 12. Issue 7: invoke the cursor GC walk so stale coordinator
    //     cursors are reclaimed as part of prune. A GC failure is
    //     non-fatal — the prune already succeeded, we surface the
    //     count as 0 on error and let `koto next` startup retry.
    let cursors_gc = match (|| -> anyhow::Result<usize> {
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("could not determine home directory"))?;
        let rs = crate::config::resolve::load_config()
            .unwrap_or_default()
            .request_store;
        crate::engine::discovery::gc_stale_cursors(&home.join(".koto"), &rs)
    })() {
        Ok(n) => n,
        Err(e) => {
            eprintln!("warning: cursor GC failed during workspace prune: {}", e);
            0
        }
    };

    println!(
        "{}",
        json!({
            "name": root,
            "pruned": root_removed,
            "descendants_removed": removed.iter().filter(|n| **n != root).count(),
            "removed": removed,
            "kept": plan.kept.iter().map(KeptMember::to_json).collect::<Vec<_>>(),
            "skipped": skipped,
            "failed": failed,
            "stale_anchors": plan.stale.values().collect::<Vec<_>>(),
            "cursors_gc": cursors_gc,
        })
    );
    if !failed.is_empty() {
        std::process::exit(3);
    }

    Ok(())
}

/// One subtree member the rule lets go, as the judgment saw it.
struct ReleasedMember {
    name: String,
    session_id: Option<String>,
    kind: MemberKind,
}

/// One subtree member the rule or its own pin keeps, with why.
struct KeptMember {
    name: String,
    reason: String,
    hierarchy: Option<serde_json::Value>,
    /// For a pinned member: `kept_by_pin` and `pin` or `pin_unreadable`.
    /// Empty for every other member, whose entry is unchanged.
    pin: serde_json::Map<String, serde_json::Value>,
}

impl KeptMember {
    fn to_json(&self) -> serde_json::Value {
        let mut v = json!({"name": self.name, "reason": self.reason});
        if let Some(h) = &self.hierarchy {
            v["hierarchy"] = h.clone();
        }
        for (k, value) in &self.pin {
            v[k] = value.clone();
        }
        v
    }
}

/// What a workspace prune removes and keeps.
struct SubtreePlan {
    /// Deepest first, the root last.
    remove: Vec<ReleasedMember>,
    kept: Vec<KeptMember>,
    /// Every stale anchor a judgment named as a keeper, by name.
    stale: std::collections::BTreeMap<String, serde_json::Value>,
}

/// The scope a subtree member is judged under: the root with every edge,
/// each other member with only the lineage between the root and its direct
/// children dropped. That lets the person who named the root dissolve its
/// subtree, while a grandchild under a kept child stays kept with it.
fn scope_for(root: &str, member: &str) -> Scope {
    if member == root {
        Scope::Whole
    } else {
        Scope::DropChildLineageOf(root.to_string())
    }
}

/// Judge the root and each descendant (listed each after its parent), then
/// list each of `by_name`, the pinned children the subtree's logs name
/// outside its lineage, as kept by its pin (as the scan read it), never for
/// removal.
fn judge_subtree(
    graph: &Graph,
    settings: &RetentionSettings,
    now: std::time::SystemTime,
    root: &str,
    descendants: &[String],
    by_name: &[String],
) -> SubtreePlan {
    let mut judge = Judge::new(graph, settings.clone(), now);
    let mut plan = SubtreePlan {
        remove: Vec::new(),
        kept: Vec::new(),
        stale: Default::default(),
    };
    let order = descendants
        .iter()
        .rev()
        .map(String::as_str)
        .chain(std::iter::once(root));
    for member in order {
        match judge.judge(member, &scope_for(root, member)) {
            Ok(j) => {
                if let Verdict::Keep { keepers, .. } = &j.verdict {
                    for k in keepers.iter().filter(|k| k.stale) {
                        plan.stale
                            .entry(k.name.clone())
                            .or_insert_with(|| hierarchy::keeper_json(k));
                    }
                }
                // A member's own pin keeps it, whatever its hierarchy's
                // verdict, and keeps nothing else: the other members are
                // judged by the rule alone.
                if let Some(pinned) = hierarchy::pin_text(&j.pin) {
                    plan.kept.push(KeptMember {
                        name: member.to_string(),
                        reason: pinned,
                        hierarchy: Some(hierarchy::to_json(&j, settings)),
                        pin: hierarchy::pin_fields(&j.pin, crate::session::pin::SOURCE_LOCAL),
                    });
                } else if j.verdict.is_kept() {
                    plan.kept.push(KeptMember {
                        name: member.to_string(),
                        reason: hierarchy::reason_text(&j.verdict),
                        hierarchy: Some(hierarchy::to_json(&j, settings)),
                        pin: Default::default(),
                    });
                } else {
                    let facts = graph.facts(member);
                    plan.remove.push(ReleasedMember {
                        name: member.to_string(),
                        session_id: facts.and_then(|f| f.session_id.clone()),
                        kind: j
                            .members
                            .iter()
                            .find(|m| m.name == member)
                            .map(|m| m.kind)
                            .unwrap_or(MemberKind::KotoSession),
                    });
                }
            }
            Err(e) => plan.kept.push(KeptMember {
                name: member.to_string(),
                reason: format!("kept: it can't be judged: {e:#}"),
                hierarchy: None,
                pin: Default::default(),
            }),
        }
    }
    for member in by_name {
        let Some(pin) = graph.facts(member).map(|f| &f.pin) else {
            continue;
        };
        let Some(reason) = hierarchy::pin_text(pin) else {
            continue;
        };
        plan.kept.push(KeptMember {
            name: member.to_string(),
            reason,
            hierarchy: judge
                .judge(member, &Scope::Whole)
                .ok()
                .map(|j| hierarchy::to_json(&j, settings)),
            pin: hierarchy::pin_fields(pin, crate::session::pin::SOURCE_LOCAL),
        });
    }
    plan
}

/// What the reclaim did with each released member.
struct Reclaimed {
    removed: Vec<String>,
    skipped: Vec<serde_json::Value>,
    failed: Vec<serde_json::Value>,
}

/// Remove the released members, in `members`' order (deepest first, the
/// root last). Every one is locked first, then judged again with a fresh
/// judge that reads each hierarchy member afresh, before anything is
/// removed; one now kept, held by another process, or replaced under its
/// name is skipped. A member is removed only when every released member
/// below it went, so no session left behind names a removed parent.
fn reclaim(
    backend: &crate::session::Backend,
    graph: &Graph,
    settings: &RetentionSettings,
    now: std::time::SystemTime,
    root: &str,
    members: &[ReleasedMember],
) -> Reclaimed {
    use crate::session::SessionError;
    let mut out = Reclaimed {
        removed: Vec::new(),
        skipped: Vec::new(),
        failed: Vec::new(),
    };
    let mut left: Vec<String> = Vec::new();
    fn skip(out: &mut Reclaimed, left: &mut Vec<String>, name: &str, reason: String) {
        out.skipped.push(json!({"name": name, "reason": reason}));
        left.push(name.to_string());
    }

    let mut locked = Vec::new();
    for member in members {
        match backend.lock_state_file(&member.name) {
            Ok(lock) => locked.push((member, lock)),
            Err(SessionError::Locked { .. }) => skip(
                &mut out,
                &mut left,
                &member.name,
                "another koto process holds the session's lock".to_string(),
            ),
            Err(SessionError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                skip(
                    &mut out,
                    &mut left,
                    &member.name,
                    "the session is already gone".to_string(),
                );
            }
            Err(e) => skip(
                &mut out,
                &mut left,
                &member.name,
                format!("the session can't be locked: {e}"),
            ),
        }
    }

    // Judge again, under the locks and before anything goes.
    let mut judge = Judge::new(graph, settings.clone(), now);
    let mut cleared = Vec::new();
    for (member, lock) in locked {
        let name = member.name.as_str();
        let refusal = match judge.judge(name, &scope_for(root, name)) {
            Ok(j) if j.verdict.is_kept() => {
                Some(format!("now {}", hierarchy::reason_text(&j.verdict)))
            }
            Ok(_) => match backend.read_header(name) {
                Err(e) => Some(format!("its header can't be read: {e:#}")),
                Ok(h)
                    if Some(h.session_id.clone()).filter(|id| !id.is_empty())
                        != member.session_id =>
                {
                    Some("a different session now has this name".to_string())
                }
                // An anchor never reaches a terminal state; the judgment just
                // made is what says it is closed.
                Ok(_)
                    if member.kind == MemberKind::KotoSession
                        && !is_terminal_session(backend, name) =>
                {
                    Some("the session is no longer finished".to_string())
                }
                Ok(_) => None,
            },
            Err(e) => Some(format!("it can't be judged again: {e:#}")),
        };
        match refusal {
            Some(reason) => skip(&mut out, &mut left, name, reason),
            None => cleared.push((member, lock)),
        }
    }

    for (member, lock) in cleared {
        let name = member.name.as_str();
        if let Some(below) = left.iter().find(|l| descends_from(graph, l, name)) {
            let reason = format!(
                "{} below it wasn't removed, and a session goes only after everything below it",
                hierarchy::sanitize(below)
            );
            skip(&mut out, &mut left, name, reason);
            continue;
        }
        // Through the pin guard, under the lock held since the judgment: a
        // pin set during the prompt keeps the member, which is listed as
        // skipped with its pin. Like a member planned as kept by its pin, it
        // keeps nothing above it.
        match crate::session::pin::remove_guarded_with(backend, name, &lock, || {
            backend.cleanup(name)
        }) {
            Ok(crate::session::pin::Removal::Removed) => out.removed.push(name.to_string()),
            Ok(crate::session::pin::Removal::Kept(guard)) => {
                let mut entry = json!({
                    "name": name,
                    "reason": format!("now kept by its pin: {}", guard.text().unwrap_or_default()),
                });
                for (k, v) in guard.fields() {
                    entry[k] = v;
                }
                out.skipped.push(entry);
            }
            Err(e) => {
                out.failed
                    .push(json!({"name": name, "error": format!("{e:#}")}));
                left.push(name.to_string());
            }
        }
    }
    out
}

/// Whether `name`'s `parent_workflow` chain passes through `ancestor`.
fn descends_from(graph: &Graph, name: &str, ancestor: &str) -> bool {
    let mut seen = HashSet::new();
    let mut current = name.to_string();
    while let Some(parent) = graph
        .facts(&current)
        .and_then(|f| f.parent_workflow.clone())
    {
        if parent == ancestor {
            return true;
        }
        if !seen.insert(parent.clone()) {
            return false;
        }
        current = parent;
    }
    false
}

/// `lstat()` the candidate path; if it is a symlink, reject with a
/// clear error. This catches both an attacker-crafted root pointing
/// outside `~/.koto/` and the legitimate-but-disallowed case of an
/// operator symlinking a session directory into the workspace.
fn reject_if_symlink(path: &Path) {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            exit_with_error_code(
                json!({
                    "error": format!(
                        "symlink not permitted: {}",
                        path.display()
                    ),
                    "command": "workspace prune",
                }),
                2,
            );
        }
        // Path doesn't exist yet: that's caller-error, surfaced below
        // by the `backend.exists()` check. Other I/O errors are surfaced
        // there too.
        _ => {}
    }
}

/// Walk events + header to determine whether the root has reached a
/// terminal state. `WorkflowCancelled` events take precedence (the
/// workflow was explicitly aborted); otherwise compare the derived
/// current state against the compiled template's `terminal` flag.
fn derive_terminal_status(
    header: &StateFileHeader,
    events: &[crate::engine::types::Event],
    session_dir: &Path,
) -> Result<TerminalStatus> {
    if events
        .iter()
        .any(|e| matches!(e.payload, EventPayload::WorkflowCancelled { .. }))
    {
        return Ok(TerminalStatus::Abandoned);
    }

    let machine_state = derive_machine_state(header, events, session_dir).ok_or_else(|| {
        anyhow::anyhow!(
            "corrupt state file: cannot derive current state for header.workflow={}",
            header.workflow
        )
    })?;

    let template_bytes = std::fs::read(&machine_state.template_path)
        .with_context(|| format!("failed to read template at {}", machine_state.template_path))?;
    let compiled: CompiledTemplate =
        serde_json::from_slice(&template_bytes).with_context(|| {
            format!(
                "failed to parse template at {}",
                machine_state.template_path
            )
        })?;

    let is_terminal = compiled
        .states
        .get(&machine_state.current_state)
        .is_some_and(|s| s.terminal);
    if is_terminal {
        Ok(TerminalStatus::Completed)
    } else {
        Ok(TerminalStatus::NonTerminal {
            current_state: machine_state.current_state,
        })
    }
}

/// Walk `SessionInfo.parent_workflow` to collect every transitive
/// descendant of `root`. Each session is listed after its parent, so the
/// caller in `handle_prune` removes the list in reverse, deepest first, and
/// removes the root last: a failed removal then leaves every remaining
/// session under a parent that still exists, reachable by the next prune
/// from the same root.
fn collect_descendants(root: &str, sessions: &[SessionInfo]) -> Vec<String> {
    struct Collect(Vec<String>);
    impl DescendantVisitor for Collect {
        fn enter(&mut self, child: &str, _parent: &str) -> bool {
            self.0.push(child.to_string());
            true
        }
    }
    let mut collect = Collect(Vec::new());
    LineageTree::from_sessions(sessions).walk(root, &mut collect);
    collect.0
}

// ===========================================================
// The descendant walk
// ===========================================================

/// Deeper than any real fan-out; only a corrupted header graph reaches it.
pub(crate) const MAX_WALK_DEPTH: usize = 1000;

/// Where a [`LineageTree::walk`] stopped short of a branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WalkLimit {
    /// The child was already walked: a `parent_workflow` cycle (A -> B ->
    /// A), constructible by removing a parent and re-creating it under its
    /// own former child.
    Cycle,
    /// The session's children are deeper than [`MAX_WALK_DEPTH`].
    Depth,
}

/// What a [`LineageTree::walk`] does at each descendant.
pub(crate) trait DescendantVisitor {
    /// Called before `child`'s own children are walked. `false` skips the
    /// child and everything under it, and the walk counts it as not
    /// cleared.
    fn enter(&mut self, child: &str, parent: &str) -> bool;

    /// Called once everything under `child` was walked; `below_cleared`
    /// says whether every child under it was cleared. Returns whether
    /// `child` itself is cleared. The default clears whatever its subtree
    /// says.
    fn leave(&mut self, _child: &str, below_cleared: bool) -> bool {
        below_cleared
    }

    /// Called where the walk stops short: a child already walked, or a
    /// session whose children are too deep. Either counts as not cleared.
    fn limit(&mut self, _at: &str, _why: WalkLimit) {}
}

/// The store's koto lineage: each session's children, by the one lineage
/// rule ([`hierarchy::lineage_target`]), so a child whose `parent_workflow`
/// is an epoch-branch name nothing holds (`<p>~<digits>`) is a child of
/// `<p>`, whatever the names.
///
/// The parent sweep, the same-name replace's move-aside, the listing of a
/// kept parent's children and `koto workspace prune`'s subtree all walk it
/// through [`LineageTree::walk`], the one guarded walk: a visited set ends
/// a `parent_workflow` cycle, and a depth cap ends a corrupted chain.
pub(crate) struct LineageTree {
    children: HashMap<String, Vec<String>>,
}

impl LineageTree {
    pub(crate) fn from_sessions(sessions: &[SessionInfo]) -> Self {
        let names: HashSet<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        for s in sessions {
            if let Some(pw) = s.parent_workflow.as_deref() {
                let parent = hierarchy::lineage_target(pw, |n| names.contains(n));
                if parent != s.id {
                    children.entry(parent).or_default().push(s.id.clone());
                }
            }
        }
        LineageTree { children }
    }

    /// The direct children of `id`.
    pub(crate) fn children(&self, id: &str) -> &[String] {
        self.children.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Walk every descendant of `root`, each after its parent, depth first.
    /// Returns whether every one was cleared (see [`DescendantVisitor`]).
    pub(crate) fn walk(&self, root: &str, visitor: &mut dyn DescendantVisitor) -> bool {
        let mut visited: HashSet<String> = HashSet::from([root.to_string()]);
        self.walk_from(root, visitor, &mut visited, 0)
    }

    fn walk_from(
        &self,
        id: &str,
        visitor: &mut dyn DescendantVisitor,
        visited: &mut HashSet<String>,
        depth: usize,
    ) -> bool {
        let kids = self.children(id);
        if kids.is_empty() {
            return true;
        }
        if depth >= MAX_WALK_DEPTH {
            visitor.limit(id, WalkLimit::Depth);
            return false;
        }
        let mut all = true;
        for child in kids {
            if !visited.insert(child.clone()) {
                visitor.limit(child, WalkLimit::Cycle);
                all = false;
                continue;
            }
            if !visitor.enter(child, id) {
                all = false;
                continue;
            }
            let below = self.walk_from(child, visitor, visited, depth + 1);
            if !visitor.leave(child, below) {
                all = false;
            }
        }
        all
    }
}

// ===========================================================
// Removing a parent's retained descendants with it
// ===========================================================

/// Remove `name`'s terminal descendants, if `name` can have children at all.
///
/// Called just before koto removes a session at its own terminal, so a child
/// kept on disk doesn't outlive the parent that koto removed (koto issue
/// 240). Since the tick keeps every session with an edge, and a child's
/// lineage is one, a parent with a child is kept at its terminal and never
/// reaches this: the call stays as the safety net it always was, for a
/// child the tick's edge test could not see. A session can have children
/// when its template declares a `materialize_children` hook, which is how
/// koto recognises a coordinator, or when its own log holds a
/// `ChildCompleted` from a child created with `koto init --parent`. A leaf
/// session, which is most terminal ticks, never lists sessions.
pub(crate) fn sweep_if_parent(
    backend: &dyn SessionBackend,
    context_store: &dyn crate::session::context::ContextStore,
    name: &str,
    compiled: &CompiledTemplate,
) {
    if may_have_children(backend, name, compiled) {
        sweep_terminal_descendants(backend, context_store, name);
    }
}

/// Remove every descendant of `parent` that stands in a terminal state and
/// has nothing live under it, deepest first.
///
/// Two callers remain: the same-name replace of a predecessor the retention
/// rule released, and [`sweep_if_parent`], the terminal tick's safety net.
/// A parent's own terminal tick keeps it and its children (they have
/// lineage), so the tick lists them in `kept_children` instead of sweeping.
///
/// The removal is implicit -- nobody named these sessions -- so every rule
/// fails toward keeping:
///
/// - a descendant that isn't terminal is live work: it is not removed and
///   the walk does not descend into it;
/// - a descendant is removed only after every session under it was;
/// - a descendant whose log or template can't be read or classified is left
///   alone with its subtree;
/// - a descendant bound to a request leg that is still open is left alone:
///   its result hasn't reached the leg (typically a failed promotion waiting
///   for a retry), and removing it would leave the leg open for good;
/// - a `parent_workflow` cycle ends the walk (visited set, depth cap);
/// - each session's status is read again just before its removal, narrowing
///   the window in which a concurrent `koto rewind` could bring it back;
/// - a failed removal warns and the walk continues; the caller removes the
///   parent either way, and a leftover shows in `koto workflows --orphaned`.
///
/// A descendant holding a pin, readable or not, is never removed: each
/// removal goes through the pin guard, and the pinned descendants it kept
/// are returned, each with its pin, for the caller to list. A pin keeps its
/// own session only: the walk enters a pinned child like any terminal one
/// and removes what is released under it, and a pinned child counts as
/// cleared for the sessions above it, so its own parent still goes and the
/// child stays with a `parent_workflow` naming an absent peer. The sweep
/// walks `parent_workflow` lineage, so a pinned session whose lineage to
/// `parent` was removed is outside the walk and nothing here removes it;
/// when a log in the tree names it as a child, it is listed with the
/// pinned descendants all the same ([`pinned_children_by_name`]).
pub(crate) fn sweep_terminal_descendants(
    backend: &dyn SessionBackend,
    context_store: &dyn crate::session::context::ContextStore,
    parent: &str,
) -> Vec<KeptChild> {
    let sessions = match backend.list() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("warning: could not list sessions to remove {parent}'s kept children: {e}");
            return Vec::new();
        }
    };
    let tree = LineageTree::from_sessions(&sessions);
    // The lineage before the sweep removes any of it, so the logs of the
    // children it removes are read too.
    let lineage = lineage_of(&tree, parent);
    let by_name = pinned_children_by_name(backend, parent, &lineage);
    let mut sweep = Sweep {
        backend,
        context_store,
        pinned: Vec::new(),
    };
    tree.walk(parent, &mut sweep);
    let mut pinned = sweep.pinned;
    pinned.extend(by_name.iter().map(|(n, pin)| pinned_child(n, pin)));
    pinned
}

/// The sweep's half of the shared walk: it skips a branch it can't clear
/// (a live child, one waiting on its leg, one it can't read, a cycle or a
/// chain too deep), and removes a child only once everything under it went.
struct Sweep<'a> {
    backend: &'a dyn SessionBackend,
    context_store: &'a dyn crate::session::context::ContextStore,
    /// The descendants the pin guard kept, each with its pin.
    pinned: Vec<KeptChild>,
}

impl DescendantVisitor for Sweep<'_> {
    fn enter(&mut self, child: &str, _parent: &str) -> bool {
        is_terminal_session(self.backend, child) && !awaits_leg_promotion(self.backend, child)
    }

    fn leave(&mut self, child: &str, below_cleared: bool) -> bool {
        let backend = self.backend;
        // Read it again: something may have rewound it since the walk began.
        if !below_cleared || !is_terminal_session(backend, child) {
            return false;
        }
        // Its finished record first, so `koto status` can answer for it once
        // it is gone.
        #[cfg(unix)]
        if let Some(record) = super::finished_record_for_child(backend, self.context_store, child) {
            super::write_finished_record(backend, &record);
        }
        match crate::session::pin::remove_guarded(backend, child) {
            Ok(crate::session::pin::Removal::Removed) => true,
            Ok(crate::session::pin::Removal::Kept(guard)) => {
                #[cfg(unix)]
                super::forget_finished_record(backend, child);
                self.pinned.push(pinned_child(child, &guard.fact()));
                // Kept by its own pin, which keeps nothing above it: the
                // parent is judged as if this child had gone.
                true
            }
            Err(e) => {
                eprintln!("warning: could not remove kept session {child}: {e}");
                #[cfg(unix)]
                super::forget_finished_record(backend, child);
                false
            }
        }
    }
}

/// Every descendant of `root` by koto lineage, whatever state it is in.
fn lineage_of(tree: &LineageTree, root: &str) -> Vec<String> {
    struct All(Vec<String>);
    impl DescendantVisitor for All {
        fn enter(&mut self, child: &str, _parent: &str) -> bool {
            self.0.push(child.to_string());
            true
        }
    }
    let mut all = All(Vec::new());
    tree.walk(root, &mut all);
    all.0
}

/// The pinned children of `root` that its lineage no longer reaches: the
/// finished sessions the log of `root`, or of one of its lineage
/// descendants (`lineage`), names as a child (`ChildCompleted`), that
/// aren't `root` or in `lineage`, and that hold a pin, readable or not.
/// Each with its pin, sorted by name.
///
/// A pinned child is found by its own name and its own pin, never through
/// its `parent_workflow`, which import doesn't carry and which can be gone.
/// These are never removed here (a walk of lineage never reaches them);
/// they are listed so the parent's report names every child a pin keeps.
/// A read only: it writes nothing into the child or its pin.
fn pinned_children_by_name(
    backend: &dyn SessionBackend,
    root: &str,
    lineage: &[String],
) -> Vec<(String, crate::session::pin::PinFact)> {
    let walked: HashSet<&str> = std::iter::once(root)
        .chain(lineage.iter().map(String::as_str))
        .collect();
    let mut named = std::collections::BTreeSet::new();
    for parent in &walked {
        let Ok((_, events)) = backend.read_events(parent) else {
            continue;
        };
        for e in events {
            if let EventPayload::ChildCompleted { child_name, .. } = e.payload {
                if !walked.contains(child_name.as_str()) {
                    named.insert(child_name);
                }
            }
        }
    }
    named
        .into_iter()
        .filter(|n| validate_session_id(n).is_ok() && backend.exists(n))
        .filter(|n| is_terminal_session(backend, n))
        .filter_map(|n| {
            let pin = crate::session::pin::local_guard(backend, &n).fact();
            pin.is_pinned().then_some((n, pin))
        })
        .collect()
}

/// The `kept_children` entry for a child its own pin keeps: reason
/// `pinned`, the pin's text as its detail, and the pin's fields.
fn pinned_child(name: &str, pin: &crate::session::pin::PinFact) -> KeptChild {
    KeptChild {
        name: name.to_string(),
        reason: super::next_types::RetentionReason::Pinned.as_str(),
        detail: hierarchy::pin_text(pin).unwrap_or_default(),
        pin: hierarchy::pin_fields(pin, crate::session::pin::SOURCE_LOCAL),
    }
}

/// The terminal descendants of `parent`, each with the edges that keep it,
/// for the `kept_children` field of a kept parent's terminal tick.
///
/// A parent with children always has an edge, so the tick keeps it, and its
/// children with it: they have lineage to it. The sweep that would have
/// removed them at the parent's removal removes nothing; this lists what it
/// would have walked instead (the same walk, which doesn't descend into a
/// live child), so `koto session prune` can be pointed at it later. Only a
/// session that can have children lists anything: one whose template
/// declares a batch hook, or whose log holds a `ChildCompleted`. A pinned
/// child its lineage no longer reaches is listed too, when a log in the
/// tree names it ([`pinned_children_by_name`]).
pub(crate) fn kept_children(
    backend: &dyn SessionBackend,
    name: &str,
    compiled: &CompiledTemplate,
) -> Vec<KeptChild> {
    if !may_have_children(backend, name, compiled) {
        return Vec::new();
    }
    let Ok(sessions) = backend.list() else {
        return Vec::new();
    };
    struct List<'a> {
        backend: &'a dyn SessionBackend,
        out: Vec<String>,
    }
    impl DescendantVisitor for List<'_> {
        fn enter(&mut self, child: &str, _parent: &str) -> bool {
            if !is_terminal_session(self.backend, child) {
                return false;
            }
            self.out.push(child.to_string());
            true
        }
    }
    let mut list = List {
        backend,
        out: Vec::new(),
    };
    let tree = LineageTree::from_sessions(&sessions);
    tree.walk(name, &mut list);
    let by_name = pinned_children_by_name(backend, name, &lineage_of(&tree, name));
    let base = backend
        .session_dir(name)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let mut kept: Vec<KeptChild> = list
        .out
        .into_iter()
        .map(|child| {
            let facts = hierarchy::SessionFacts::read(&base, &child);
            if facts.pin.is_pinned() {
                return pinned_child(&child, &facts.pin);
            }
            let mut via: Vec<String> = facts
                .links
                .iter()
                .map(|l| {
                    format!(
                        "its {} link names {}",
                        hierarchy::sanitize(&l.kind),
                        hierarchy::sanitize(&l.peer)
                    )
                })
                .collect();
            if let Some(pw) = &facts.parent_workflow {
                via.push(format!(
                    "its parent_workflow names {}",
                    hierarchy::sanitize(pw)
                ));
            }
            if let Some(p) = &facts.chain_parent {
                via.push(format!("its chain/parent names {}", hierarchy::sanitize(p)));
            }
            KeptChild {
                reason: super::next_types::RetentionReason::Hierarchy.as_str(),
                detail: format!(
                    "kept with its hierarchy ({}); koto session prune removes it once the \
                     hierarchy is released",
                    via.join("; ")
                ),
                name: child,
                pin: Default::default(),
            }
        })
        .collect();
    kept.extend(by_name.iter().map(|(n, pin)| pinned_child(n, pin)));
    kept
}

/// Whether `name` can have koto children: its template declares a batch
/// hook, which is how koto recognises a coordinator, or its own log holds a
/// `ChildCompleted` from a child created with `koto init --parent`. A leaf
/// session, which is most terminal ticks, never lists sessions.
fn may_have_children(
    backend: &dyn SessionBackend,
    name: &str,
    compiled: &CompiledTemplate,
) -> bool {
    compiled
        .states
        .values()
        .any(|s| s.materialize_children.is_some())
        || backend.read_events(name).is_ok_and(|(_, events)| {
            events
                .iter()
                .any(|e| matches!(e.payload, EventPayload::ChildCompleted { .. }))
        })
}

/// True when `id` is bound to a request leg that is still open, so its
/// terminal result has not reached the leg yet -- typically because the
/// promotion failed and the session's next tick would retry it. The rules
/// follow `promote_leg_result`'s own: a request that is gone, closed,
/// unreadable, or whose leg is resolved or abandoned is not pending (the
/// tick would give up too); an I/O error, which the tick would retry, keeps
/// the session.
pub(crate) fn awaits_leg_promotion(backend: &dyn SessionBackend, id: &str) -> bool {
    use crate::engine::request_store::{self, RequestStoreError, ValidatedRequestId};
    use crate::engine::types::{LegDisposition, RequestState};

    let Some(pointer) =
        crate::engine::leg_pointer::read_pointer_best_effort(&backend.session_dir(id))
    else {
        return false;
    };
    let (Some(home), Ok(request_id)) = (
        dirs::home_dir(),
        ValidatedRequestId::new(&pointer.request_id),
    ) else {
        return false;
    };
    match request_store::read_view(&home.join(".koto"), &request_id) {
        Ok(view) => {
            view.request_state == RequestState::Open
                && view
                    .leg(&pointer.leg_name)
                    .is_ok_and(|leg| leg.disposition == LegDisposition::Open)
        }
        Err(RequestStoreError::Io { .. }) => true,
        Err(_) => false,
    }
}

/// True only when `id` reads and classifies as terminal (completed or
/// abandoned). Any read or classification error is "not terminal", so the
/// sweep leaves the session alone. This is deliberately the opposite of
/// `non_terminal_sessions`, which only warns before an operator-confirmed
/// prune and so treats an unreadable session as nothing to warn about.
pub(crate) fn is_terminal_session(backend: &dyn SessionBackend, id: &str) -> bool {
    let Ok((header, events)) = backend.read_events(id) else {
        return false;
    };
    derive_terminal_status(&header, &events, &backend.session_dir(id))
        .is_ok_and(|status| status.is_terminal())
}

/// Filter the candidate set to sessions whose terminal status is
/// non-terminal. Returns a sorted vector for stable operator-facing
/// output. Errors during inspection are skipped silently -- a session
/// we cannot read is a session we cannot warn about, but the reclaim
/// step will surface the underlying issue.
fn non_terminal_sessions<'a, I>(backend: &dyn SessionBackend, candidates: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut out: Vec<String> = candidates
        .into_iter()
        .filter(|id| {
            let Ok((header, events)) = backend.read_events(id) else {
                return false;
            };
            match derive_terminal_status(&header, &events, &backend.session_dir(id)) {
                Ok(status) => !status.is_terminal(),
                Err(_) => false,
            }
        })
        .map(|s| s.to_string())
        .collect();
    out.sort();
    out
}

/// Print the operator-facing preview: what the rule removes, in order,
/// what it keeps and why, the stale anchors with their close commands, and
/// the sessions under the root that aren't terminal.
///
/// Always emits to stdout (the verb's primary output channel).
fn print_preview(
    root: &str,
    plan: &SubtreePlan,
    non_terminal_in_set: &[String],
    status: &TerminalStatus,
) {
    println!("root: {} ({})", root, status.describe());
    let descendants = plan.remove.len() + plan.kept.len() - 1;
    if descendants == 0 {
        println!("descendants: (none)");
    } else {
        println!("descendants: {descendants}");
    }
    if plan.remove.is_empty() {
        println!("remove: (none)");
    } else {
        println!("remove ({}, in this order):", plan.remove.len());
        for m in &plan.remove {
            println!("  {}", hierarchy::sanitize(&m.name));
        }
    }
    if !plan.kept.is_empty() {
        println!("keep ({}):", plan.kept.len());
        for k in &plan.kept {
            println!(
                "  {}: {}",
                hierarchy::sanitize(&k.name),
                hierarchy::sanitize(&k.reason)
            );
        }
    }
    if !plan.stale.is_empty() {
        println!("stale anchors (close each by hand once its agent session is gone):");
        for (name, v) in &plan.stale {
            match v.get("close_command").and_then(|c| c.as_str()) {
                Some(cmd) => println!("  {}: {}", hierarchy::sanitize(name), cmd),
                None => println!(
                    "  {}: its observed marker or harness session id is malformed, so no close \
                     command is printed",
                    hierarchy::sanitize(name)
                ),
            }
        }
    }
    if !non_terminal_in_set.is_empty() {
        println!();
        println!("note: these sessions under the root are not terminal; the rule keeps each while it runs:");
        for id in non_terminal_in_set {
            println!("  {}", id);
        }
    }
}

/// Prompt the operator for confirmation, returning whether to proceed.
///
/// `prompt_required = false` means the caller has already gathered
/// consent (e.g. `--yes` on the CLI, or Issue 18's
/// `KOTO_REQUEST_STORE_PRUNE_CONFIRM=1` env-var bypass) and the prompt is
/// skipped. With `prompt_required = true`, the function writes the
/// prompt to stdout, reads one line from stdin, and returns true on
/// `y`/`yes` (case-insensitive, trimmed). EOF on stdin counts as
/// negative consent.
fn confirm_prune(prompt_required: bool) -> io::Result<bool> {
    if !prompt_required {
        return Ok(true);
    }
    print!("Proceed with prune? [y/N] ");
    io::stdout().flush()?;
    let mut input = String::new();
    let n = io::stdin().read_line(&mut input)?;
    if n == 0 {
        return Ok(false); // EOF
    }
    let trimmed = input.trim().to_lowercase();
    Ok(trimmed == "y" || trimmed == "yes")
}

/// Second-tier confirmation gate for `--yes --force`. Fires AFTER
/// [`confirm_prune`] returns true and requires the operator to type
/// the literal string `force-prune` (exact match, no case folding)
/// before the destructive prune executes.
///
/// The terminal-state safety gate (refusing to prune a non-terminal
/// root without `--force`) is the first line of defense. `--yes` lets
/// cron skip the standard y/N prompt. The combination of `--yes` AND
/// `--force` removes both gates; this helper is the manual override
/// that ensures the operator INTENDS to force-prune and isn't running
/// a templated cron with `--force` baked in by accident.
fn confirm_force_prune() -> io::Result<bool> {
    println!();
    println!("WARNING: --force bypasses the terminal-state safety gate.");
    println!(
        "         A force-prune of a live tree corrupts any coordinator still holding a claim."
    );
    print!("Type 'force-prune' to confirm: ");
    io::stdout().flush()?;
    let mut input = String::new();
    let n = io::stdin().read_line(&mut input)?;
    // Print a newline so subsequent stdout (the success JSON or an
    // error payload) lands on a fresh line. Without this, piped stdin
    // (no terminal echo) leaves the prompt and the next println on the
    // same physical line, mangling downstream JSON parsers.
    println!();
    if n == 0 {
        return Ok(false); // EOF
    }
    // Trim trailing newline but keep case + body. Exact match only.
    Ok(input.trim_end_matches(['\n', '\r']) == "force-prune")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- sweep_if_parent: a leaf tick never lists sessions -----

    /// A backend that delegates to a local one and counts `list()` calls.
    struct CountingBackend {
        inner: crate::session::local::LocalBackend,
        lists: std::sync::atomic::AtomicUsize,
    }

    impl SessionBackend for CountingBackend {
        fn create(&self, id: &str) -> anyhow::Result<std::path::PathBuf> {
            self.inner.create(id)
        }
        fn session_dir(&self, id: &str) -> std::path::PathBuf {
            self.inner.session_dir(id)
        }
        fn exists(&self, id: &str) -> bool {
            self.inner.exists(id)
        }
        fn cleanup(&self, id: &str) -> anyhow::Result<()> {
            self.inner.cleanup(id)
        }
        fn list(&self) -> anyhow::Result<Vec<SessionInfo>> {
            self.lists.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.list()
        }
        fn append_header(&self, id: &str, header: &StateFileHeader) -> anyhow::Result<()> {
            self.inner.append_header(id, header)
        }
        fn append_event(
            &self,
            id: &str,
            payload: &EventPayload,
            timestamp: &str,
        ) -> anyhow::Result<()> {
            self.inner.append_event(id, payload, timestamp)
        }
        fn init_state_file(
            &self,
            id: &str,
            header: StateFileHeader,
            initial_events: Vec<crate::engine::types::Event>,
        ) -> Result<(), crate::session::SessionError> {
            self.inner.init_state_file(id, header, initial_events)
        }
        fn read_events(
            &self,
            id: &str,
        ) -> anyhow::Result<(StateFileHeader, Vec<crate::engine::types::Event>)> {
            self.inner.read_events(id)
        }
        fn read_header(&self, id: &str) -> anyhow::Result<StateFileHeader> {
            self.inner.read_header(id)
        }
        fn ensure_pushed(&self, id: &str) -> Result<(), crate::session::SessionError> {
            self.inner.ensure_pushed(id)
        }
        fn relocate(&self, from: &str, to: &str) -> anyhow::Result<()> {
            self.inner.relocate(from, to)
        }
        fn lock_state_file(
            &self,
            id: &str,
        ) -> Result<crate::session::SessionLock, crate::session::SessionError> {
            self.inner.lock_state_file(id)
        }
    }

    fn header(name: &str) -> StateFileHeader {
        StateFileHeader {
            command_environment: None,
            schema_version: 1,
            workflow: name.to_string(),
            template_hash: "h".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            parent_workflow: None,
            template_source_dir: None,
            template_source_file: None,
            origin: None,
            execution_dir: None,
            session_id: String::new(),
            intent: None,
            template_name: None,
            needs_agent: None,
            role: None,
            inputs: None,
            coordinator_of_record: None,
            requested_by: None,
            assignment_claim: None,
            dispatch_epoch: 0,
            priority: None,
            deadline: None,
            retry_count: None,
            agent_config: None,
            kind: None,
            root_session_id: None,
            parent_session_id: None,
            respawn_generation: None,
        }
    }

    fn leaf_template() -> CompiledTemplate {
        CompiledTemplate {
            pass_env: Vec::new(),
            format_version: 1,
            name: "leaf".to_string(),
            version: "1.0".to_string(),
            description: String::new(),
            initial_state: "work".to_string(),
            variables: Default::default(),
            states: Default::default(),
        }
    }

    #[test]
    fn sweep_lists_sessions_only_for_a_session_that_had_children() {
        let tmp = tempfile::TempDir::new().unwrap();
        let backend = CountingBackend {
            inner: crate::session::local::LocalBackend::with_base_dir(tmp.path().to_path_buf()),
            lists: Default::default(),
        };
        backend
            .init_state_file("leaf", header("leaf"), vec![])
            .unwrap();

        sweep_if_parent(&backend, &backend.inner, "leaf", &leaf_template());
        assert_eq!(
            backend.lists.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a session with no batch hook and no reported child is not swept"
        );

        // A session a child reported to is swept, which lists once.
        backend
            .append_event(
                "leaf",
                &EventPayload::ChildCompleted {
                    child_name: "leaf.c".to_string(),
                    task_name: "c".to_string(),
                    outcome: crate::engine::types::TerminalOutcome::Success,
                    final_state: "done".to_string(),
                    result: None,
                    failure_reason: None,
                },
                "2026-01-01T00:00:01Z",
            )
            .unwrap();
        sweep_if_parent(&backend, &backend.inner, "leaf", &leaf_template());
        assert_eq!(backend.lists.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// Prune removes descendants in reverse, deepest first, which is only
    /// correct if every session is listed after its parent.
    #[test]
    fn collect_descendants_lists_each_session_after_its_parent() {
        let info = |id: &str, parent: Option<&str>| SessionInfo {
            id: id.to_string(),
            created_at: "t".to_string(),
            template_hash: "h".to_string(),
            parent_workflow: parent.map(str::to_string),
            template_source_status: None,
            execution_dir: None,
            session_id: None,
            kind: None,
        };
        // Listed out of order on purpose.
        let sessions = vec![
            info("gc", Some("c")),
            info("c", Some("root")),
            info("root", None),
            info("c2", Some("root")),
        ];
        let order = collect_descendants("root", &sessions);
        let pos = |id: &str| order.iter().position(|x| x == id).unwrap();
        assert!(pos("c") < pos("gc"), "{order:?}");
        assert_eq!(order.len(), 3);
    }

    #[test]
    fn collect_descendants_finds_direct_and_transitive_children() {
        let sessions = vec![
            SessionInfo {
                id: "root".to_string(),
                created_at: "t0".to_string(),
                template_hash: "h0".to_string(),
                parent_workflow: None,
                template_source_status: None,
                execution_dir: None,
                session_id: None,
                kind: None,
            },
            SessionInfo {
                id: "child-a".to_string(),
                created_at: "t1".to_string(),
                template_hash: "h0".to_string(),
                parent_workflow: Some("root".to_string()),
                template_source_status: None,
                execution_dir: None,
                session_id: None,
                kind: None,
            },
            SessionInfo {
                id: "child-b".to_string(),
                created_at: "t1".to_string(),
                template_hash: "h0".to_string(),
                parent_workflow: Some("root".to_string()),
                template_source_status: None,
                execution_dir: None,
                session_id: None,
                kind: None,
            },
            SessionInfo {
                id: "grandchild".to_string(),
                created_at: "t2".to_string(),
                template_hash: "h0".to_string(),
                parent_workflow: Some("child-a".to_string()),
                template_source_status: None,
                execution_dir: None,
                session_id: None,
                kind: None,
            },
            SessionInfo {
                id: "unrelated".to_string(),
                created_at: "t0".to_string(),
                template_hash: "h0".to_string(),
                parent_workflow: None,
                template_source_status: None,
                execution_dir: None,
                session_id: None,
                kind: None,
            },
        ];

        let descendants = collect_descendants("root", &sessions);
        assert_eq!(descendants.len(), 3);
        assert!(descendants.contains(&"child-a".to_string()));
        assert!(descendants.contains(&"child-b".to_string()));
        assert!(descendants.contains(&"grandchild".to_string()));
        assert!(!descendants.contains(&"unrelated".to_string()));
        assert!(!descendants.contains(&"root".to_string()));
    }

    fn walk_info(id: &str, parent: Option<&str>) -> SessionInfo {
        SessionInfo {
            id: id.to_string(),
            created_at: "t".to_string(),
            template_hash: "h".to_string(),
            parent_workflow: parent.map(str::to_string),
            template_source_status: None,
            execution_dir: None,
            session_id: None,
            kind: None,
        }
    }

    /// Records what the walk does, and skips the children it is told to.
    #[derive(Default)]
    struct Trace {
        entered: Vec<String>,
        left: Vec<(String, bool)>,
        limits: Vec<(String, WalkLimit)>,
        skip: Vec<&'static str>,
    }

    impl DescendantVisitor for Trace {
        fn enter(&mut self, child: &str, _parent: &str) -> bool {
            self.entered.push(child.to_string());
            !self.skip.contains(&child)
        }
        fn leave(&mut self, child: &str, below_cleared: bool) -> bool {
            self.left.push((child.to_string(), below_cleared));
            below_cleared
        }
        fn limit(&mut self, at: &str, why: WalkLimit) {
            self.limits.push((at.to_string(), why));
        }
    }

    /// The walk follows the one lineage rule: a `parent_workflow` naming an
    /// epoch branch nothing holds is a child of the branch's base, whatever
    /// the names; one naming a session that exists is that session's.
    #[test]
    fn the_walk_follows_the_lineage_rule_through_epoch_branches() {
        let sessions = vec![
            walk_info("p", None),
            walk_info("p~1.a", Some("p~1")),
            walk_info("helper", Some("p")),
            walk_info("q~2", None),
            walk_info("q~2.b", Some("q~2")),
            walk_info("q", None),
        ];
        let tree = LineageTree::from_sessions(&sessions);
        let mut trace = Trace::default();
        assert!(tree.walk("p", &mut trace));
        assert_eq!(trace.entered, vec!["p~1.a", "helper"]);
        // `q~2` is a session, so its child is its own, not `q`'s.
        assert!(tree.children("q").is_empty());
        assert_eq!(tree.children("q~2"), ["q~2.b".to_string()]);
    }

    /// A skipped child is not descended into and clears nothing above it; a
    /// cycle and a chain past the depth cap are reported and stop the walk
    /// there, never looping.
    #[test]
    fn the_walk_is_guarded_by_a_visited_set_and_a_depth_cap() {
        let sessions = vec![
            walk_info("r", Some("b")),
            walk_info("a", Some("r")),
            walk_info("a.x", Some("a")),
            walk_info("b", Some("r")),
        ];
        let tree = LineageTree::from_sessions(&sessions);
        let mut trace = Trace {
            skip: vec!["a"],
            ..Trace::default()
        };
        assert!(!tree.walk("r", &mut trace));
        assert!(
            !trace.entered.contains(&"a.x".to_string()),
            "{:?}",
            trace.entered
        );
        assert_eq!(trace.limits, vec![("r".to_string(), WalkLimit::Cycle)]);
        assert!(
            trace.left.contains(&("b".to_string(), false)),
            "{:?}",
            trace.left
        );

        let deep: Vec<SessionInfo> = (0..=MAX_WALK_DEPTH + 1)
            .map(|i| {
                let parent = (i > 0).then(|| format!("d{}", i - 1));
                walk_info(&format!("d{i}"), parent.as_deref())
            })
            .collect();
        let mut trace = Trace::default();
        assert!(!LineageTree::from_sessions(&deep).walk("d0", &mut trace));
        assert_eq!(trace.entered.len(), MAX_WALK_DEPTH);
        assert!(matches!(trace.limits.as_slice(), [(_, WalkLimit::Depth)]));
    }

    #[test]
    fn collect_descendants_empty_when_no_children() {
        let sessions = vec![SessionInfo {
            id: "lonely".to_string(),
            created_at: "t0".to_string(),
            template_hash: "h0".to_string(),
            parent_workflow: None,
            template_source_status: None,
            execution_dir: None,
            session_id: None,
            kind: None,
        }];
        let descendants = collect_descendants("lonely", &sessions);
        assert!(descendants.is_empty());
    }

    #[test]
    fn terminal_status_describes_each_variant() {
        assert_eq!(TerminalStatus::Completed.describe(), "completed");
        assert_eq!(TerminalStatus::Abandoned.describe(), "abandoned");
        assert_eq!(
            TerminalStatus::NonTerminal {
                current_state: "review".to_string()
            }
            .describe(),
            "not terminal (current state: review)"
        );
    }

    #[test]
    fn terminal_status_is_terminal() {
        assert!(TerminalStatus::Completed.is_terminal());
        assert!(TerminalStatus::Abandoned.is_terminal());
        assert!(!TerminalStatus::NonTerminal {
            current_state: "s".to_string()
        }
        .is_terminal());
    }
}
