//! The retention engine: which sessions belong to one hierarchy, and
//! whether that hierarchy is kept or released (DESIGN-session-retention.md,
//! Decision 3).
//!
//! A hierarchy is a connected component of the undirected graph whose edges
//! are the links sessions keep about themselves (`koto/links/<kind>/<peer>`),
//! koto lineage (`parent_workflow`, with the epoch-branch rule in
//! [`lineage_target`]) and a well-formed `chain/parent`. The engine works on
//! two levels:
//!
//! 1. [`Graph::scan`] reads every session's header line, `koto/` entries,
//!    link bodies and `chain/parent` once per command ([`SessionFacts`]),
//!    and builds the edge list, the migrated set and the components.
//! 2. [`Judge`] inspects in full only the members of a component it is
//!    asked about: a koto session's standing through
//!    [`crate::session::inventory::inspect_full`], an anchor's last activity
//!    from its own log and its close through the shared close check
//!    ([`crate::session::anchor::checked_close_record`]). It answers one
//!    [`Verdict`] per component, which every removal point and
//!    `koto session hierarchy` share.
//!
//! A component is kept while any member is alive. A koto session is alive
//! while it runs or can't be read. An anchor with no observed marker (one
//! never observed, or an imported one) reads closed once the idle period has
//! passed since its last activity; an observed anchor closes only through a
//! valid observed close, and past its last activity plus the stale threshold
//! it is reported `stale`, with the command an operator runs to close it by
//! hand, and nothing more. Once nothing is alive, the hierarchy's release
//! instant is the latest stop among its members, and it is released when the
//! retention (`retention.keep`) has passed since then.
//!
//! Everything fails toward keeping: a malformed link, close record or
//! `chain/parent`, an unreadable log, a symlink under `ctx/koto/`, or a
//! timestamp more than five minutes ahead of `now` taints its component; an
//! unreadable header, manifest or links directory taints every component;
//! and a sessions directory that can't be listed fails [`Graph::scan`].
//!
//! Everything here reads the local sessions directory directly, never
//! through a backend's `list` or `read_events`, so nothing reaches a cloud
//! bucket, and nothing here writes.

mod facts;
mod graph;
mod judge;
mod render;
mod settings;

#[cfg(test)]
mod tests;

use std::path::Path;

pub use facts::{ImportedFrom, LinkFact, SessionFacts, IMPORTED_FROM_KEY};
pub use graph::{Edge, Graph, Scope, Via};
pub use judge::{
    CloseKind, Judge, Judgment, Keeper, Member, MemberKind, ReleaseInstant, Retention, Unreadable,
    Verdict,
};
pub use render::{
    close_command, keeper_json, keeper_line, reason_text, render_text, sanitize, to_json,
};
pub use settings::{RetentionSettings, Setting, SettingError};

/// How far ahead of `now` a timestamp read from disk may be before it
/// taints its component.
pub const FUTURE_TOLERANCE: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// The session a `parent_workflow` value is lineage to.
///
/// This is koto's one lineage rule. The graph scan, [`edge_of`], the
/// descendant walk that the parent sweep and the same-name replace share,
/// and the superseded-branch listing of `koto status` all call it. A value
/// that names a session (`exists`) is lineage to it. One that doesn't, and
/// splits at its last `~` into `<p>` and a run of digits, is lineage to
/// `<p>`: a batch rewind leaves its relocated children pointing at an
/// epoch-branch name that was never created. Anything else is the value
/// itself, which then names no session.
pub fn lineage_target(parent_workflow: &str, exists: impl Fn(&str) -> bool) -> String {
    if exists(parent_workflow) {
        return parent_workflow.to_string();
    }
    match epoch_branch_of(parent_workflow) {
        Some(p) => p.to_string(),
        None => parent_workflow.to_string(),
    }
}

/// The `<p>` of an epoch-branch name `<p>~<digits>`, or `None` for a name
/// of any other shape. Whether such a name is lineage to `<p>` also depends
/// on it naming no session; see [`lineage_target`].
pub fn epoch_branch_of(name: &str) -> Option<&str> {
    let (p, epoch) = name.rsplit_once('~')?;
    (!p.is_empty() && !epoch.is_empty() && epoch.bytes().all(|b| b.is_ascii_digit())).then_some(p)
}

/// Whether the `chain/parent` of the session in `dir` names one of
/// `probes`. A malformed value names no session for the graph, but one that
/// is a probed name with whitespace around it was meant as that one, so the
/// tick keeps the session.
fn chain_parent_names(dir: &Path, probes: &[&str]) -> bool {
    match facts::read_chain_parent(dir) {
        facts::ChainParent::Names(p) => probes.contains(&p.as_str()),
        facts::ChainParent::Malformed(_) => {
            probes.iter().any(|p| facts::chain_parent_mentions(dir, p))
        }
        facts::ChainParent::Absent => false,
    }
}

/// Whether session `name` under `base` has any edge at all: a link of its
/// own, a `parent_workflow`, a `chain/parent`, or another session naming it
/// through lineage, a link or a `chain/parent`.
///
/// The terminal tick's test, so it reads no settings and judges no
/// retention: it checks the session's own facts first, then makes one pass
/// over the store that stops at the first inbound hit. A copy that
/// `koto session import` made also counts records naming the session it was
/// copied from, since the graph resolves those to the copy. Anything it
/// can't read, the sessions directory included, answers "edge", which keeps
/// the session.
///
/// It finds everything [`Graph::scan`] would join the session through, so
/// a tick never removes a session a prune would keep. Besides the names it
/// probes, it reads each link body in the store (at most 4 KiB each, in the
/// same pass): a link whose session-id hint carries this session's id under
/// another name (a predecessor the same-name replace moved aside to
/// `<name>_r<N>`, a descendant renamed with it), and a link body of a
/// version this koto doesn't know whose `session` names it, both join it in
/// the graph, so both count here.
///
/// These join rules repeat `Graph::build_edges`'s by design, so the tick
/// reads no more than it must. A change to either side must keep `edge_of`
/// a superset of the graph's joins; the property tests
/// `the_two_level_verdict_equals_a_full_read_reference` and
/// `edge_of_never_misses_an_edge_the_graph_joins_hints_included` check it.
pub fn edge_of(base: &Path, name: &str) -> bool {
    let own = SessionFacts::read(base, name);
    if !own.problems.is_empty() || !own.store_problems.is_empty() {
        return true;
    }
    if !own.links.is_empty() || own.parent_workflow.is_some() || own.chain_parent.is_some() {
        return true;
    }
    let names = match graph::session_names(base) {
        Ok(names) => names,
        Err(_) => return true,
    };
    let exists = |n: &str| names.binary_search_by(|x| x.as_str().cmp(n)).is_ok();
    let mut probes = vec![name];
    if let Some(from) = &own.imported_from {
        probes.push(from.from_session.as_str());
    }
    for other in &names {
        if other == name {
            continue;
        }
        let dir = base.join(other);
        match facts::read_header_line(&dir, other) {
            Ok(header) => {
                if let Some(pw) = header.parent_workflow.as_deref() {
                    if probes.contains(&lineage_target(pw, exists).as_str()) {
                        return true;
                    }
                }
            }
            Err(_) => return true,
        }
        // Every kind directory, known or not: the graph counts a link of a
        // kind this koto doesn't know as an edge too.
        let links = dir.join("ctx").join("koto").join("links");
        let kinds = match std::fs::read_dir(&links) {
            Ok(kinds) => kinds,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // No links, but it may still name this one through its
                // chain/parent.
                if chain_parent_names(&dir, &probes) {
                    return true;
                }
                continue;
            }
            Err(_) => return true,
        };
        for kind in kinds {
            let Ok(kind) = kind else { return true };
            let files = match std::fs::read_dir(kind.path()) {
                Ok(files) => files,
                // A stray file at the kind level holds no links.
                Err(_) if kind.file_type().is_ok_and(|t| !t.is_dir()) => continue,
                Err(_) => return true,
            };
            for file in files {
                let Ok(file) = file else { return true };
                // A file under a probed name counts whatever it holds, as
                // it did before bodies were read: the graph joins a listed
                // key even when its body is malformed.
                if file
                    .file_name()
                    .to_str()
                    .is_some_and(|p| probes.contains(&p))
                {
                    return true;
                }
                match facts::link_file_names(&file.path()) {
                    Ok(Some((session, session_id))) => {
                        if session.as_deref().is_some_and(|s| probes.contains(&s)) {
                            return true;
                        }
                        if own.session_id.is_some() && session_id == own.session_id {
                            return true;
                        }
                    }
                    Ok(None) => {}
                    Err(_) => return true,
                }
            }
        }
        if chain_parent_names(&dir, &probes) {
            return true;
        }
    }
    false
}
