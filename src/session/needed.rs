//! `koto session needed <PATH>`: whether a workflow session still needs a
//! directory, for a workspace manager deciding whether it may remove it.
//!
//! A path is needed while a workflow session anchored at or under it (its
//! header's `execution_dir`, compared by whole path components after
//! resolving symlinks) is running and no later member of its lineage is
//! anchored outside it. The lineage is the continues relation
//! (`koto/links/continues/<pred>`): the sessions continuing a session, the
//! sessions continuing those, and so on. A later member anchored outside the
//! path releases it whatever that member's own standing, so a run taken over
//! elsewhere, or one that finished elsewhere, stops holding its old
//! directory. A later member with no recorded anchor releases nothing.
//! Terminal and cancelled sessions never need a path, and a session anchored
//! above the path doesn't count.
//!
//! Anything that can't be judged keeps the path: a session under it whose
//! log or template can't be read, and a running one whose continues
//! relation can't be read, are needed and named as unreadable. A session
//! whose header can't be read may be anchored anywhere, so the answer is
//! undecided unless another session already needs the path.
//!
//! The read goes through the same two local reads the rest of koto uses:
//! [`inventory::inspect`] for each session's standing, as
//! `koto session list --execution-dir` reads it, and the hierarchy
//! [`Graph`]'s resolved continues links for the lineage. Both read the local
//! sessions directory only. Nothing here writes, and nothing reaches a
//! remote store.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io::{BufRead, BufReader, Read as _};
use std::path::{Path, PathBuf};

use crate::engine::types::SessionKind;
use crate::session::hierarchy::Graph;
use crate::session::inventory::{self, under_dir, SessionStatus, TemplateCache};
use crate::session::state_file_name;

/// The longest line the verb prints, in bytes.
pub const MAX_LINE_BYTES: usize = 200;

/// The longest header line read, as the hierarchy scan bounds it.
const MAX_HEADER_BYTES: u64 = 256 * 1024;

/// The verb's answer, and the one line it prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// No session needs the path: exit 0.
    NotNeeded,
    /// A session needs the path: exit 1, with the line naming it.
    Needed(String),
    /// The path couldn't be judged: exit 2, with the line saying why.
    Undecided(String),
}

impl Answer {
    /// The exit code the verb returns.
    pub fn exit_code(&self) -> i32 {
        match self {
            Answer::NotNeeded => 0,
            Answer::Needed(_) => 1,
            Answer::Undecided(_) => 2,
        }
    }

    /// The line the verb prints, at most [`MAX_LINE_BYTES`] bytes.
    pub fn line(&self) -> String {
        let line = match self {
            Answer::NotNeeded => "not needed",
            Answer::Needed(line) | Answer::Undecided(line) => line.as_str(),
        };
        truncate(line, MAX_LINE_BYTES)
    }
}

/// Why a session needs the path.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Why {
    Running,
    Unreadable,
    /// Running, and the continues relation `holder` holds about it can't be
    /// read, so whether it was continued elsewhere is unknown.
    RelationUnreadable(String),
}

impl Why {
    fn text(&self) -> String {
        match self {
            Why::Running => "is running".to_string(),
            Why::Unreadable => "is unreadable".to_string(),
            Why::RelationUnreadable(holder) => {
                format!("is running, and its continues relation from {holder} is unreadable")
            }
        }
    }
}

/// What a session's header says about its execution anchor.
type Anchor = Result<Option<PathBuf>, String>;

/// Judge `path` against the local sessions directory `base`.
pub fn needed(base: &Path, path: &Path) -> Answer {
    let target = match fs::canonicalize(path) {
        Ok(p) => p,
        Err(e) => {
            return Answer::Undecided(format!("can't decide: the path can't be resolved: {e}"))
        }
    };
    let graph = match Graph::scan(base) {
        Ok(g) => g,
        Err(e) => {
            return Answer::Undecided(format!(
                "can't decide: the sessions directory can't be listed: {e:#}"
            ))
        }
    };

    let mut anchors: BTreeMap<String, Anchor> = BTreeMap::new();
    let mut cache = TemplateCache::new();
    let mut needing: Vec<(String, Why)> = Vec::new();
    let mut undecided: Vec<String> = Vec::new();

    let names: Vec<String> = graph.names().map(str::to_string).collect();
    for name in &names {
        if graph
            .facts(name)
            .is_some_and(|f| f.kind != SessionKind::Ordinary)
        {
            continue;
        }
        let anchor = anchor_of(&mut anchors, base, name);
        let dir = match anchor {
            Ok(Some(dir)) => dir,
            Ok(None) => continue,
            Err(_) => {
                undecided.push(name.clone());
                continue;
            }
        };
        if !under_dir(&dir, &target) {
            continue;
        }
        match inventory::inspect(base, name, &mut cache).status {
            SessionStatus::Terminal | SessionStatus::Cancelled => {}
            SessionStatus::Unreadable => needing.push((name.clone(), Why::Unreadable)),
            SessionStatus::Running => {
                if let Some(why) = held(&graph, &mut anchors, base, name, &target) {
                    needing.push((name.clone(), why));
                }
            }
        }
    }

    // The count comes before the names, so a cut at
    // MAX_LINE_BYTES never drops it.
    if let Some((first, why)) = needing.first() {
        return Answer::Needed(format!(
            "needed{}: session {first} {}",
            count_note(needing.len()),
            why.text()
        ));
    }
    if let Some(first) = undecided.first() {
        return Answer::Undecided(format!(
            "can't decide{}: session {first}'s header can't be read, so where it is anchored is \
             unknown",
            count_note(undecided.len())
        ));
    }
    Answer::NotNeeded
}

/// Whether running session `name`, anchored under `target`, still holds it:
/// `None` when a later member of its lineage is anchored outside `target`,
/// otherwise why it holds. The walk follows every continues link that
/// resolved to a session, breadth first, and never revisits a session, so
/// a cycle ends it.
fn held(
    graph: &Graph,
    anchors: &mut BTreeMap<String, Anchor>,
    base: &Path,
    name: &str,
    target: &Path,
) -> Option<Why> {
    let mut seen: BTreeSet<String> = BTreeSet::from([name.to_string()]);
    let mut queue: VecDeque<String> = VecDeque::from([name.to_string()]);
    let mut unreadable: Option<String> = None;
    while let Some(current) = queue.pop_front() {
        let resolved: BTreeSet<String> = graph
            .directed()
            .iter()
            .filter(|d| d.target == current && d.holder != current)
            .map(|d| d.holder.clone())
            .collect();
        if unreadable.is_none() {
            unreadable = graph
                .successors_of(&current)
                .into_iter()
                .find(|holder| !resolved.contains(holder));
        }
        for holder in resolved {
            if !seen.insert(holder.clone()) {
                continue;
            }
            if let Ok(Some(dir)) = anchor_of(anchors, base, &holder) {
                if !under_dir(&dir, target) {
                    return None;
                }
            }
            queue.push_back(holder);
        }
    }
    Some(match unreadable {
        Some(holder) => Why::RelationUnreadable(holder),
        None => Why::Running,
    })
}

/// Session `name`'s recorded anchor, read once per call.
fn anchor_of(anchors: &mut BTreeMap<String, Anchor>, base: &Path, name: &str) -> Anchor {
    anchors
        .entry(name.to_string())
        .or_insert_with(|| read_anchor(base, name))
        .clone()
}

/// Read the `execution_dir` from the first line of session `name`'s state
/// file. Only the header is read, so a log that doesn't parse past it still
/// says where the session is anchored. `Ok(None)` is a header with no
/// anchor recorded (a session from before anchoring, never ticked since).
fn read_anchor(base: &Path, name: &str) -> Anchor {
    let dir = base.join(name);
    match fs::symlink_metadata(&dir) {
        Ok(meta) if meta.file_type().is_dir() => {}
        _ => return Err("its session directory is a symlink or not a directory".to_string()),
    }
    let path = dir.join(state_file_name(name));
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_file() => {}
        _ => return Err("its state file is a symlink or not a regular file".to_string()),
    }
    let file = fs::File::open(&path).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(file.take(MAX_HEADER_BYTES))
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let header: serde_json::Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
    match header.get("execution_dir") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(dir)) => Ok(Some(PathBuf::from(dir))),
        Some(_) => Err("its execution_dir isn't a path".to_string()),
    }
}

/// ` (N sessions)` for a list of `count` sessions, empty for one.
fn count_note(count: usize) -> String {
    if count > 1 {
        format!(" ({count} sessions)")
    } else {
        String::new()
    }
}

/// `line` cut to at most `max` bytes on a character boundary.
fn truncate(line: &str, max: usize) -> String {
    if line.len() <= max {
        return line.to_string();
    }
    let mut end = max;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    line[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_line_is_cut_on_a_character_boundary() {
        let line = format!("{}é", "a".repeat(MAX_LINE_BYTES - 1));
        let cut = Answer::Needed(line).line();
        assert_eq!(cut.len(), MAX_LINE_BYTES - 1);
        assert!(cut.chars().all(|c| c == 'a'));
    }

    #[test]
    fn exit_codes_follow_the_answer() {
        assert_eq!(Answer::NotNeeded.exit_code(), 0);
        assert_eq!(Answer::Needed(String::new()).exit_code(), 1);
        assert_eq!(Answer::Undecided(String::new()).exit_code(), 2);
    }

    #[test]
    fn a_missing_path_is_undecided() {
        let tmp = tempfile::TempDir::new().unwrap();
        let answer = needed(tmp.path(), &tmp.path().join("no-such-dir"));
        assert_eq!(answer.exit_code(), 2);
    }

    #[test]
    fn an_empty_store_needs_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sessions = tmp.path().join("sessions");
        assert_eq!(needed(&sessions, tmp.path()), Answer::NotNeeded);
    }

    #[test]
    fn a_header_is_read_even_when_the_log_after_it_is_garbage() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("s1");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(state_file_name("s1")),
            "{\"workflow\":\"s1\",\"execution_dir\":\"/x/y\"}\nnot json\n",
        )
        .unwrap();
        assert_eq!(
            read_anchor(tmp.path(), "s1"),
            Ok(Some(PathBuf::from("/x/y")))
        );
        fs::write(dir.join(state_file_name("s1")), "{\"workflow\":\"s1\"}\n").unwrap();
        assert_eq!(read_anchor(tmp.path(), "s1"), Ok(None));
        fs::write(dir.join(state_file_name("s1")), "garbage\n").unwrap();
        assert!(read_anchor(tmp.path(), "s1").is_err());
    }
}
