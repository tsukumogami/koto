//! Moving a held predecessor aside at a same-name replace
//! (DESIGN-session-retention.md, Decision 4).
//!
//! When `koto init --replace-terminal` finds a finished session whose
//! hierarchy the retention rule keeps, the old run isn't removed: it and
//! its koto descendants move aside, so the new run gets the name with no
//! children and the old one stays readable for whoever holds it.
//!
//! - The descendants are found with the walk the parent sweep uses
//!   ([`LineageTree::walk`]), by `parent_workflow` whatever the names. Where
//!   the sweep would skip a branch, the move refuses, so no part of the
//!   subtree is left under the old names.
//! - A descendant that is running, waiting to promote its result to a leg,
//!   or can't be read refuses the replace with `replace_held_live`; so does
//!   a session whose header can't be read (the listing, and so the walk,
//!   can't see it) when its name puts it under the moved one.
//! - Otherwise the predecessor becomes `<name>_r<N>`, N the lowest value
//!   leaving every target name free, here and in the bucket. A descendant
//!   named `<name>.<rest>` or `<name>~<rest>` is renamed to the same suffix
//!   under `<name>_r<N>`, deepest first, through a checked relocate; any
//!   other keeps its name. Every session in the subtree then gets its
//!   `parent_workflow` set explicitly by the prefix-swap rule
//!   ([`swap_prefix`]); the predecessor keeps its own.
//! - Under the cloud backend each renamed session is pushed under its new
//!   name and every header rewrite is pushed, strictly. A failure in any of
//!   that moves everything back, by a local-only rename and the original
//!   headers, re-pushed, deletes the new remote prefixes, and refuses with
//!   `replace_failed`. Only once all of it succeeded are the old remote
//!   prefixes deleted; from the first delete on, a failure rolls forward
//!   and is reported.
//!
//! Log events, keys and links in the moved sessions are untouched, and no
//! context key is written anywhere. Under the cloud backend the continues
//! relation's back-references follow the move: a moved session holding a
//! relation names its new self beside its predecessor before it moves and
//! takes the old name's back-reference out after, and the valid
//! back-references beside a moved predecessor (at most 64) are carried to
//! its new name, with any left behind reported and none deleted that
//! doesn't check out.

use std::collections::{BTreeMap, HashSet};

use crate::engine::types::StateFileHeader;
use crate::session::validate::validate_session_id;
use crate::session::SessionBackend;

use super::workspace::{
    awaits_leg_promotion, is_terminal_session, DescendantVisitor, LineageTree, WalkLimit,
};

/// The highest N tried for `<name>_r<N>`.
const MAX_RERUN_SUFFIX: u32 = 10_000;

/// A move that went through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Moved {
    /// The predecessor's new name, `<name>_r<N>`.
    pub moved_to: String,
    /// Every rename, `(old, new)`, deepest first, the predecessor last.
    pub renamed: Vec<(String, String)>,
    /// Old remote prefixes the roll-forward couldn't empty, with why.
    pub undeleted: Vec<(String, String)>,
}

/// Why the move didn't happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MoveRefusal {
    /// A descendant is running, waits to promote its result to a leg, or
    /// can't be read. Nothing was moved, or everything moved went back.
    HeldLive { live: Vec<String> },
    /// The move couldn't be made, or failed partway. `stranded` lists the
    /// renames a failed move-back left in place, `(old, new)`: those
    /// sessions stay readable under their new names.
    Failed {
        message: String,
        stranded: Vec<(String, String)>,
    },
}

impl MoveRefusal {
    fn failed(message: impl Into<String>) -> Self {
        MoveRefusal::Failed {
            message: message.into(),
            stranded: Vec::new(),
        }
    }
}

/// `parent_workflow` under the prefix-swap rule: a value equal to `name`,
/// or starting with `name.` or `name~`, gets that `name` replaced by
/// `moved_to`; anything else is kept.
pub(crate) fn swap_prefix(parent: &str, name: &str, moved_to: &str) -> String {
    match renamed_as(parent, name, moved_to) {
        Some(new) => new,
        None => parent.to_string(),
    }
}

/// The name `session` takes when `name` moves to `moved_to`: `moved_to`
/// for `name` itself, the same suffix under `moved_to` for `name.<rest>` or
/// `name~<rest>`, and `None` for any other name, which is kept.
fn renamed_as(session: &str, name: &str, moved_to: &str) -> Option<String> {
    if session == name {
        return Some(moved_to.to_string());
    }
    let rest = session.strip_prefix(name)?;
    (rest.starts_with('.') || rest.starts_with('~')).then(|| format!("{moved_to}{rest}"))
}

/// Whether `session` is `name` or sits under it by name (`name.<rest>` or
/// `name~<rest>`): exactly the sessions a move of `name` renames.
fn is_under(session: &str, name: &str) -> bool {
    renamed_as(session, name, name).is_some()
}

/// Whether descendant `id` holds the move: running, waiting to promote its
/// result to a leg, or unreadable (which reads as not terminal).
fn holds_the_move(backend: &dyn SessionBackend, id: &str) -> bool {
    !is_terminal_session(backend, id) || awaits_leg_promotion(backend, id)
}

/// Move `name`, a finished session, and its koto subtree aside.
pub(crate) fn move_aside(backend: &dyn SessionBackend, name: &str) -> Result<Moved, MoveRefusal> {
    // ---- the subtree ----
    let sessions = backend.list().map_err(|e| {
        MoveRefusal::failed(format!(
            "could not list sessions to find the descendants of '{name}': {e:#}"
        ))
    })?;
    struct Collect {
        order: Vec<String>,
        too_deep: Option<String>,
    }
    impl DescendantVisitor for Collect {
        fn enter(&mut self, child: &str, _parent: &str) -> bool {
            self.order.push(child.to_string());
            true
        }
        fn limit(&mut self, at: &str, why: WalkLimit) {
            // A cycle back into the subtree adds nothing: that session is in
            // it already, and moves with it.
            if why == WalkLimit::Depth {
                self.too_deep.get_or_insert_with(|| at.to_string());
            }
        }
    }
    let mut collect = Collect {
        order: Vec::new(),
        too_deep: None,
    };
    LineageTree::from_sessions(&sessions).walk(name, &mut collect);
    if let Some(at) = collect.too_deep {
        return Err(MoveRefusal::failed(format!(
            "the descendants of '{name}' go deeper than {} levels below '{at}', so the whole \
             subtree can't be moved",
            super::workspace::MAX_WALK_DEPTH
        )));
    }
    let descendants = collect.order;

    // ---- live descendants refuse ----
    let mut live: Vec<String> = descendants
        .iter()
        .filter(|d| holds_the_move(backend, d))
        .cloned()
        .collect();
    // A session whose header can't be read isn't listed, so the walk can't
    // see whether it is a descendant. One whose name puts it under the
    // moved name would be one to rename: it is unreadable, so it holds the
    // move like a live one, rather than being left under the old name.
    let listed: HashSet<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
    let local = local_names(backend, name);
    for n in &local {
        if n != name
            && !listed.contains(n.as_str())
            && is_under(n, name)
            && backend
                .session_dir(n)
                .join(crate::session::state_file_name(n))
                .exists()
        {
            live.push(n.clone());
        }
    }
    if !live.is_empty() {
        return Err(MoveRefusal::HeldLive { live });
    }

    // ---- the original headers, restored as they were on a move-back ----
    let subtree: Vec<String> = std::iter::once(name.to_string())
        .chain(descendants.iter().cloned())
        .collect();
    let mut originals: BTreeMap<String, StateFileHeader> = BTreeMap::new();
    for s in &subtree {
        match backend.read_header(s) {
            Ok(h) => {
                originals.insert(s.clone(), h);
            }
            Err(_) if s != name => {
                return Err(MoveRefusal::HeldLive {
                    live: vec![s.clone()],
                })
            }
            Err(e) => {
                return Err(MoveRefusal::failed(format!(
                    "could not read the header of '{name}': {e:#}"
                )))
            }
        }
    }

    // ---- step 1: a free N ----
    let moved_to = pick_name(backend, name, &subtree, &listed, &local)?;
    // Deepest first: the walk lists each session after its parent, so its
    // reverse puts every child before its parent, the predecessor last.
    let renames: Vec<(String, String)> = subtree
        .iter()
        .rev()
        .filter_map(|s| renamed_as(s, name, &moved_to).map(|new| (s.clone(), new)))
        .collect();

    // ---- step 2: rename and re-point, undone on any failure ----
    let mut done: Vec<(String, String)> = Vec::new();
    let mut failure: Option<MoveRefusal> = None;
    for (old, new) in &renames {
        if old != name && holds_the_move(backend, old) {
            failure = Some(MoveRefusal::HeldLive {
                live: vec![old.clone()],
            });
            break;
        }
        match backend.relocate_checked(old, new) {
            Ok(()) => done.push((old.clone(), new.clone())),
            Err(e) => {
                // A checked relocate may fail after its local rename; the
                // move-back has to know the session moved.
                if local_present(backend, new) && !local_present(backend, old) {
                    done.push((old.clone(), new.clone()));
                }
                failure = Some(MoveRefusal::failed(format!(
                    "could not move '{old}' to '{new}': {e:#}"
                )));
                break;
            }
        }
        if let Err(e) = backend.ensure_pushed(new) {
            failure = Some(MoveRefusal::failed(format!(
                "could not push '{new}' under its new name: {e:#}"
            )));
            break;
        }
    }
    // The sessions whose header a rewrite may have changed, by original
    // name; with the renamed ones, what a move-back restores.
    let mut rewritten: Vec<String> = Vec::new();
    if failure.is_none() {
        let current = |s: &str| renamed_as(s, name, &moved_to).unwrap_or_else(|| s.to_string());
        for s in &subtree {
            rewritten.push(s.clone());
            let original = &originals[s];
            let parent = if s == name {
                original.parent_workflow.clone()
            } else {
                original
                    .parent_workflow
                    .as_deref()
                    .map(|p| swap_prefix(p, name, &moved_to))
            };
            let at = current(s);
            let result = backend
                .rewrite_header(&at, &|mut h| {
                    h.parent_workflow = parent.clone();
                    h
                })
                .and_then(|()| backend.ensure_pushed(&at).map_err(anyhow::Error::from));
            if let Err(e) = result {
                failure = Some(MoveRefusal::failed(format!(
                    "could not re-point the parent of '{at}': {e:#}"
                )));
                break;
            }
        }
    }
    if let Some(refusal) = failure {
        let touched: Vec<String> = subtree
            .iter()
            .filter(|s| rewritten.contains(s) || done.iter().any(|(o, _)| o == *s))
            .cloned()
            .collect();
        let stranded = move_back(backend, name, &moved_to, &touched, &originals, &done);
        return Err(match refusal {
            MoveRefusal::Failed { message, .. } => MoveRefusal::Failed { message, stranded },
            held if stranded.is_empty() => held,
            MoveRefusal::HeldLive { live } => MoveRefusal::Failed {
                message: format!(
                    "{} started running during the move, and the move couldn't be undone",
                    live.join(", ")
                ),
                stranded,
            },
        });
    }

    // ---- step 3: delete the old remote prefixes; failures roll forward ----
    let mut undeleted = Vec::new();
    for (old, new) in &renames {
        if let Err(e) = backend.delete_remote_checked(old) {
            undeleted.push((old.clone(), format!("{e:#}")));
        }
        // The back-references its successors keep beside it move with it,
        // and its own pointer, written under the new name before the move,
        // replaces the old one. What doesn't move is reported, never
        // deleted.
        for note in backend.repoint_after_move(old, new) {
            eprintln!(
                "warning: moving '{}' aside: {}",
                crate::session::hierarchy::sanitize(old),
                crate::session::hierarchy::sanitize(&note)
            );
        }
    }
    Ok(Moved {
        moved_to,
        renamed: renames,
        undeleted,
    })
}

/// Whether `id` has a directory in the local store.
fn local_present(backend: &dyn SessionBackend, id: &str) -> bool {
    std::fs::symlink_metadata(backend.session_dir(id)).is_ok()
}

/// The names of the local store's session directories, readable or not.
fn local_names(backend: &dyn SessionBackend, name: &str) -> Vec<String> {
    let Some(base) = backend.session_dir(name).parent().map(|p| p.to_path_buf()) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&base) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| !n.starts_with('.') && validate_session_id(n).is_ok())
        .collect()
}

/// The lowest `<name>_r<N>` that leaves every target name free: no session
/// listed, no directory in the local store, and nothing under its remote
/// prefix. A remote listing that fails refuses. An N some session already
/// sits under (`<name>_r<N>.<rest>` or `<name>_r<N>~<rest>`, left by an
/// earlier move whose move-back failed) isn't free either, so the subtree
/// never joins what an earlier move stranded.
fn pick_name(
    backend: &dyn SessionBackend,
    name: &str,
    subtree: &[String],
    listed: &HashSet<&str>,
    local: &[String],
) -> Result<String, MoveRefusal> {
    for n in 1..=MAX_RERUN_SUFFIX {
        let moved_to = format!("{name}_r{n}");
        let occupied = listed
            .iter()
            .copied()
            .chain(local.iter().map(String::as_str))
            .any(|s| is_under(s, &moved_to));
        if occupied {
            continue;
        }
        let targets: Vec<String> = subtree
            .iter()
            .filter_map(|s| renamed_as(s, name, &moved_to))
            .collect();
        let mut free = true;
        for t in &targets {
            if validate_session_id(t).is_err() {
                return Err(MoveRefusal::failed(format!(
                    "'{t}' isn't a valid session name, so '{name}' can't be moved aside"
                )));
            }
            if listed.contains(t.as_str()) || local_present(backend, t) {
                free = false;
                break;
            }
            match backend.remote_prefix_free(t) {
                Ok(true) => {}
                Ok(false) => {
                    free = false;
                    break;
                }
                Err(e) => {
                    return Err(MoveRefusal::failed(format!(
                        "could not check whether the name '{t}' is free: {e:#}"
                    )))
                }
            }
        }
        if free {
            return Ok(moved_to);
        }
    }
    Err(MoveRefusal::failed(format!(
        "no name from {name}_r1 to {name}_r{MAX_RERUN_SUFFIX} is free"
    )))
}

/// Undo a move that failed partway: every session in `done` goes back to
/// its old name by a local-only rename, newest first, every header in
/// `touched` (renamed or rewritten, by original name) is restored to its
/// original value and pushed again, and the new remote prefixes are
/// deleted. Returns the renames that couldn't be undone:
/// those sessions stay under their new names, with their parent re-pointed
/// by the prefix-swap rule, so they stay readable and keep their lineage.
fn move_back(
    backend: &dyn SessionBackend,
    name: &str,
    moved_to: &str,
    touched: &[String],
    originals: &BTreeMap<String, StateFileHeader>,
    done: &[(String, String)],
) -> Vec<(String, String)> {
    let mut stranded = Vec::new();
    let mut returned = Vec::new();
    for (old, new) in done.iter().rev() {
        match backend.relocate_local_only(new, old) {
            Ok(()) => returned.push((old.clone(), new.clone())),
            Err(e) => {
                eprintln!("warning: could not move '{new}' back to '{old}': {e:#}");
                stranded.push((old.clone(), new.clone()));
            }
        }
    }
    let stranded_as: BTreeMap<&str, &str> = stranded
        .iter()
        .map(|(o, n)| (o.as_str(), n.as_str()))
        .collect();
    for s in touched {
        let original = originals[s].clone();
        let (at, header) = match stranded_as.get(s.as_str()) {
            None => (s.clone(), original),
            Some(new) => {
                let mut h = original;
                h.workflow = new.to_string();
                if s != name {
                    h.parent_workflow = h
                        .parent_workflow
                        .as_deref()
                        .map(|p| swap_prefix(p, name, moved_to));
                }
                (new.to_string(), h)
            }
        };
        let result = backend
            .rewrite_header(&at, &|_| header.clone())
            .and_then(|()| backend.ensure_pushed(&at).map_err(anyhow::Error::from));
        if let Err(e) = result {
            eprintln!("warning: could not restore the header of '{at}': {e:#}");
        }
    }
    for (_, new) in &returned {
        if let Err(e) = backend.delete_remote_checked(new) {
            eprintln!("warning: could not delete the remote copy under '{new}': {e:#}");
        }
    }
    stranded
}

#[cfg(test)]
#[path = "move_aside_tests.rs"]
mod tests;
