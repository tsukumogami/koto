//! The fence a rebind puts up behind a session's old anchor (koto#477).
//!
//! `koto session rebind` bumps `origin.anchor_epoch` and lists the anchor it
//! moved away from in `origin.retired_anchors`. A predecessor still working
//! from the old anchor would otherwise keep appending to the log after the
//! successor moved it, and under the cloud backend push a second history
//! under the old anchor's prefix. The cloud backend asks [`check`] before
//! every write of a session's log or keys, and it refuses with
//! [`SessionRebound`] when:
//!
//! - the command read the session at a lower epoch than the log now has: a
//!   rebind happened while it ran; or
//! - the command runs from a retired anchor (its prefix is one of theirs and
//!   not the current anchor's).
//!
//! Both are asked of the local copy's header line alone, with no request. A
//! session whose header records no origin has no fence: nothing backfills
//! one.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::engine::types::{Event, EventPayload, StateFileHeader};

/// A write refused by a rebind's fence.
#[derive(Debug)]
pub struct SessionRebound {
    /// The session written.
    pub name: String,
    /// The anchor the session is bound to now, when its header records one.
    pub anchor: Option<String>,
    /// Whether the command runs from an anchor a rebind retired, rather than
    /// having read the session before a rebind.
    pub from_retired: bool,
}

impl fmt::Display for SessionRebound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let anchor = self.anchor.as_deref().unwrap_or("its new anchor");
        if self.from_retired {
            write!(
                f,
                "session_rebound: session '{}' was rebound away from the directory this command \
                 ran in, and is bound to {} now; nothing was written. A process still working \
                 from the old anchor must stop; run the command from {}",
                self.name, anchor, anchor
            )
        } else {
            write!(
                f,
                "session_rebound: session '{}' was rebound to {} after this command read it; \
                 nothing was written. Run the command again from {}",
                self.name, anchor, anchor
            )
        }
    }
}

impl std::error::Error for SessionRebound {}

/// A whole-binding move of `name` to `anchor`, committed at `epoch`, that
/// hasn't finished its later parts: it's running, or it stopped part way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveUnfinished {
    /// The session moved.
    pub name: String,
    /// The anchor the move binds the session to.
    pub anchor: PathBuf,
    /// The `origin.anchor_epoch` the move commits, `0` for a session with
    /// no origin.
    pub epoch: u64,
}

impl fmt::Display for MoveUnfinished {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "rebind_incomplete: '{name}' was moved whole to {anchor} (epoch {epoch}), and the \
             move hasn't finished its later parts (the held-by links, the publish location and \
             the command environment): it's still running, or it stopped part way. Finish it by \
             running `koto session rebind {name} --whole` again from {anchor}, with the \
             KOTO_ANCHOR of the agent taking the run over, then run this command again",
            name = self.name,
            anchor = self.anchor.display(),
            epoch = self.epoch,
        )
    }
}

impl std::error::Error for MoveUnfinished {}

/// `Some` when the log whose header is `header` and events `events` records
/// a whole-binding move that still owes its later parts: the mark its
/// commit set (`origin.whole_parts_owed`), or a commit torn between its
/// event and its header.
pub fn move_unfinished(
    name: &str,
    header: &StateFileHeader,
    events: &[Event],
) -> Option<MoveUnfinished> {
    if let Some(o) = header.origin.as_ref().filter(|o| o.whole_parts_owed) {
        return Some(MoveUnfinished {
            name: name.to_string(),
            anchor: o.anchor.clone(),
            epoch: o.whole_epoch,
        });
    }
    // The last rebind, plain or whole: a whole one whose header wasn't
    // rewritten is a torn commit, as the move itself reads it when it runs
    // again. With an origin, the event carries the epoch one above the
    // header's; without one, the header doesn't name the event's target.
    let last = events.iter().rev().find_map(|e| match &e.payload {
        EventPayload::ExecutionAnchorRebound { .. } => Some(None),
        EventPayload::ExecutionAnchorReboundWhole(w) => Some(Some(w)),
        _ => None,
    })??;
    if !last.whole {
        return None;
    }
    let torn = match header.origin.as_ref() {
        Some(o) => last.epoch == Some(o.anchor_epoch + 1),
        None => header.execution_dir.as_deref() != Some(last.to.as_path()),
    };
    torn.then(|| MoveUnfinished {
        name: name.to_string(),
        anchor: last.to.clone(),
        epoch: last.epoch.unwrap_or(0),
    })
}

/// [`move_unfinished`] over a whole log's text, its header line and then
/// its events. A header that doesn't parse owes nothing here; an event line
/// that doesn't parse is skipped.
pub fn move_unfinished_in_log(name: &str, content: &str) -> Option<MoveUnfinished> {
    let mut lines = content.lines().filter(|l| !l.trim().is_empty());
    let header: StateFileHeader = serde_json::from_str(lines.next()?.trim()).ok()?;
    let events: Vec<Event> = lines
        .filter_map(|l| serde_json::from_str(l.trim()).ok())
        .collect();
    move_unfinished(name, &header, &events)
}

/// What a header line records of the fence.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Fence {
    /// `origin.anchor_epoch`, `0` when absent.
    pub epoch: u64,
    /// `origin.retired_anchors`.
    pub retired: Vec<PathBuf>,
    /// The current anchor: `execution_dir`, else `origin.anchor`.
    pub anchor: Option<PathBuf>,
    /// `origin.whole_epoch`, `0` when absent: the epoch the last
    /// whole-binding move wrote, which turns on [`check_whole`].
    pub whole_epoch: u64,
}

impl Fence {
    /// The fence a state log's header line records; the default (epoch `0`,
    /// nothing retired) for a line that doesn't parse or has no origin.
    pub fn of_header_line(line: &str) -> Self {
        let Ok(header) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            return Fence::default();
        };
        let path = |pointer: &str| {
            header
                .pointer(pointer)
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
        };
        Fence {
            epoch: header
                .pointer("/origin/anchor_epoch")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            retired: header
                .pointer("/origin/retired_anchors")
                .and_then(serde_json::Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .map(PathBuf::from)
                        .collect()
                })
                .unwrap_or_default(),
            anchor: path("/execution_dir").or_else(|| path("/origin/anchor")),
            whole_epoch: header
                .pointer("/origin/whole_epoch")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
        }
    }
}

/// Refuse a write of `name` by a command running under `prefix` that read
/// the session at `seen_epoch` (`None` when it read no header), when the
/// local header line `line` says a rebind has fenced it off.
pub fn check(
    name: &str,
    line: &str,
    prefix: &str,
    seen_epoch: Option<u64>,
) -> Result<(), SessionRebound> {
    let fence = Fence::of_header_line(line);
    let prefix_of = |p: &Path| crate::session::local::prefix_for_workspace(p).ok();
    let refuse = |from_retired| SessionRebound {
        name: name.to_string(),
        anchor: fence.anchor.as_ref().map(|a| a.display().to_string()),
        from_retired,
    };
    if seen_epoch.is_some_and(|seen| seen < fence.epoch) {
        return Err(refuse(false));
    }
    let current = fence.anchor.as_deref().and_then(prefix_of);
    if current.as_deref() != Some(prefix)
        && fence
            .retired
            .iter()
            .any(|r| prefix_of(r).as_deref() == Some(prefix))
    {
        return Err(refuse(true));
    }
    Ok(())
}

/// Refuse a write of `name`, on either backend, from a process running in
/// `cwd` (canonical; `None` when it couldn't be read) that read the session
/// at `seen_epoch`, when the local header line `line` records a
/// whole-binding move (`origin.whole_epoch` above `0`). A header without one
/// passes: the plain rebind's fence is [`check`]'s alone.
///
/// Refused when the process read an epoch below the header's
/// `whole_epoch` (it read the session before the move), or when
/// `cwd` is under a retired anchor and either isn't under the current anchor
/// or is under a retired anchor deeper (more path components) than the
/// current one. So after a move up into a parent the old directory is
/// fenced, and after a move down into a subdirectory the new one is not.
pub fn check_whole(
    name: &str,
    line: &str,
    cwd: Option<&Path>,
    seen_epoch: Option<u64>,
) -> Result<(), SessionRebound> {
    let fence = Fence::of_header_line(line);
    if fence.whole_epoch == 0 {
        return Ok(());
    }
    let refuse = |from_retired| SessionRebound {
        name: name.to_string(),
        anchor: fence.anchor.as_ref().map(|a| a.display().to_string()),
        from_retired,
    };
    // Measured against the move's epoch, not the anchor's: a plain rebind
    // after the move (run from a gate, say) bumps only `anchor_epoch`, and
    // the tick that read the session after the move must keep writing.
    if seen_epoch.is_some_and(|seen| seen < fence.whole_epoch) {
        return Err(refuse(false));
    }
    let Some(cwd) = cwd else {
        return Ok(());
    };
    let depth = |p: &Path| p.components().count();
    let current = fence
        .anchor
        .as_deref()
        .filter(|a| cwd.starts_with(a))
        .map(depth);
    if fence
        .retired
        .iter()
        .any(|r| cwd.starts_with(r) && current.is_none_or(|c| depth(r) > c))
    {
        return Err(refuse(true));
    }
    Ok(())
}

/// The origin's fence after a rebind from `from` to `to`: the epoch one
/// higher, `from` retired and `to` no longer.
pub fn advance(origin: &mut crate::engine::types::SessionOrigin, from: Option<&Path>, to: &Path) {
    origin.anchor_epoch += 1;
    if let Some(from) = from {
        if from != to && !origin.retired_anchors.iter().any(|r| r == from) {
            origin.retired_anchors.push(from.to_path_buf());
        }
    }
    origin.retired_anchors.retain(|r| r != to);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefix(p: &str) -> String {
        crate::session::local::prefix_for_workspace(Path::new(p)).unwrap()
    }

    #[test]
    fn a_header_without_the_fields_reads_as_epoch_zero() {
        let fence = Fence::of_header_line(
            r#"{"workflow":"wf","origin":{"anchor":"/a","store":{"kind":"cloud","base":"/s"}}}"#,
        );
        assert_eq!(fence.epoch, 0);
        assert!(fence.retired.is_empty());
        assert_eq!(fence.anchor, Some(PathBuf::from("/a")));
        assert_eq!(
            Fence::of_header_line(r#"{"workflow":"wf"}"#),
            Fence::default()
        );
        assert_eq!(Fence::of_header_line("not json"), Fence::default());
    }

    #[test]
    fn a_header_without_the_fields_parses_with_epoch_zero() {
        let line = r#"{"workflow":"wf","template_hash":"h","created_at":"t","origin":{"anchor":"/a","store":{"kind":"cloud","base":"/s"}}}"#;
        let header: crate::engine::types::StateFileHeader = serde_json::from_str(line).unwrap();
        let origin = header.origin.unwrap();
        assert_eq!(origin.anchor_epoch, 0);
        assert!(origin.retired_anchors.is_empty());
        let written = serde_json::to_string(&origin).unwrap();
        assert!(!written.contains("anchor_epoch"), "{written}");
        assert!(!written.contains("retired_anchors"), "{written}");
    }

    #[test]
    fn a_read_before_a_rebind_or_a_retired_anchor_refuses() {
        let line = r#"{"execution_dir":"/new","origin":{"anchor":"/new","store":{"kind":"cloud","base":"/s"},"anchor_epoch":2,"retired_anchors":["/old"]}}"#;
        assert!(check("wf", line, &prefix("/new"), Some(2)).is_ok());
        assert!(check("wf", line, &prefix("/new"), None).is_ok());
        assert!(check("wf", line, &prefix("/elsewhere"), Some(2)).is_ok());

        let stale = check("wf", line, &prefix("/new"), Some(1)).unwrap_err();
        assert!(!stale.from_retired);
        assert!(stale.to_string().starts_with("session_rebound:"));

        let retired = check("wf", line, &prefix("/old"), None).unwrap_err();
        assert!(retired.from_retired);
        assert!(retired.to_string().contains("/new"), "{retired}");
    }

    #[test]
    fn a_header_without_whole_epoch_keeps_the_prefix_rule() {
        // A plain rebind's header: an epoch and retired anchors, no
        // whole_epoch. The path rule adds nothing to it, from any directory.
        let line = r#"{"execution_dir":"/ws/new","origin":{"anchor":"/ws/new","store":{"kind":"local","base":"/s"},"anchor_epoch":2,"retired_anchors":["/ws/old","/ws/new/sub"]}}"#;
        assert_eq!(Fence::of_header_line(line).whole_epoch, 0);
        for cwd in ["/ws/old", "/ws/old/deeper", "/ws/new/sub", "/elsewhere"] {
            for seen in [None, Some(0), Some(2)] {
                assert!(
                    check_whole("wf", line, Some(Path::new(cwd)), seen).is_ok(),
                    "{cwd} {seen:?}"
                );
            }
        }
        // The prefix rule is as it was: a retired anchor refuses by its own
        // prefix, and a directory below it has a prefix of its own.
        assert!(
            check("wf", line, &prefix("/ws/old"), None)
                .unwrap_err()
                .from_retired
        );
        assert!(check("wf", line, &prefix("/ws/old/deeper"), None).is_ok());
    }

    #[test]
    fn a_whole_fenced_header_refuses_a_stale_read_and_a_deeper_retired_anchor() {
        // Moved up from /ws/a/b to /ws/a, after an earlier move away from
        // /ws/old.
        let up = r#"{"execution_dir":"/ws/a","origin":{"anchor":"/ws/a","store":{"kind":"local","base":"/s"},"anchor_epoch":3,"whole_epoch":3,"retired_anchors":["/ws/a/b","/ws/old"]}}"#;
        let at = |cwd: &str, seen| check_whole("wf", up, Some(Path::new(cwd)), seen);
        assert!(at("/ws/a", Some(3)).is_ok());
        assert!(at("/ws/a/c", None).is_ok());
        assert!(at("/elsewhere", None).is_ok());
        assert!(!at("/ws/a", Some(2)).unwrap_err().from_retired);
        assert!(at("/ws/a/b", None).unwrap_err().from_retired);
        assert!(at("/ws/a/b/c", None).unwrap_err().from_retired);
        assert!(at("/ws/old/x", None).unwrap_err().from_retired);

        // Moved down from /ws/a into /ws/a/b: inside the new anchor the old
        // parent no longer decides, beside it it still does.
        let down = r#"{"execution_dir":"/ws/a/b","origin":{"anchor":"/ws/a/b","store":{"kind":"local","base":"/s"},"anchor_epoch":1,"whole_epoch":1,"retired_anchors":["/ws/a"]}}"#;
        let at = |cwd: &str| check_whole("wf", down, Some(Path::new(cwd)), None);
        assert!(at("/ws/a/b/c").is_ok());
        assert!(at("/ws/a/c").unwrap_err().from_retired);
        assert!(check_whole("wf", down, None, None).is_ok());
    }

    #[test]
    fn a_plain_rebind_after_a_whole_move_leaves_reads_since_the_move_their_epoch() {
        // A whole move to /ws/a recorded epoch 3; a plain rebind (from a
        // gate, say) then moved the anchor to /ws/a/b, epoch 4. A read taken
        // after the whole move is still current; one from before it isn't.
        let line = r#"{"execution_dir":"/ws/a/b","origin":{"anchor":"/ws/a/b","store":{"kind":"local","base":"/s"},"anchor_epoch":4,"whole_epoch":3,"retired_anchors":["/ws/a","/ws/old"]}}"#;
        let at = |cwd: &str, seen| check_whole("wf", line, Some(Path::new(cwd)), seen);
        assert!(at("/ws/a/b", Some(3)).is_ok());
        assert!(at("/ws/a/b/c", Some(4)).is_ok());
        assert!(at("/elsewhere", Some(3)).is_ok());
        assert!(!at("/ws/a/b", Some(2)).unwrap_err().from_retired);
        // The plain rebind's retired anchor still refuses by path.
        assert!(at("/ws/a/c", Some(3)).unwrap_err().from_retired);
    }

    #[test]
    fn advance_retires_the_old_anchor_and_reinstates_the_target() {
        let mut origin = crate::engine::types::SessionOrigin {
            anchor: "/a".into(),
            store: crate::engine::types::SessionStoreIdentity {
                kind: "cloud".into(),
                base: "/s".into(),
                bucket: None,
                prefix: None,
            },
            anchor_epoch: 0,
            retired_anchors: Vec::new(),
            whole_epoch: 0,
            whole_parts_owed: false,
        };
        advance(&mut origin, Some(Path::new("/a")), Path::new("/b"));
        assert_eq!(origin.anchor_epoch, 1);
        assert_eq!(origin.retired_anchors, vec![PathBuf::from("/a")]);
        advance(&mut origin, Some(Path::new("/b")), Path::new("/a"));
        assert_eq!(origin.anchor_epoch, 2);
        assert_eq!(origin.retired_anchors, vec![PathBuf::from("/b")]);
    }
}
