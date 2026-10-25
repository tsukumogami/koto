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

/// What a header line records of the fence.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Fence {
    /// `origin.anchor_epoch`, `0` when absent.
    pub epoch: u64,
    /// `origin.retired_anchors`.
    pub retired: Vec<PathBuf>,
    /// The current anchor: `execution_dir`, else `origin.anchor`.
    pub anchor: Option<PathBuf>,
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
        };
        advance(&mut origin, Some(Path::new("/a")), Path::new("/b"));
        assert_eq!(origin.anchor_epoch, 1);
        assert_eq!(origin.retired_anchors, vec![PathBuf::from("/a")]);
        advance(&mut origin, Some(Path::new("/b")), Path::new("/a"));
        assert_eq!(origin.anchor_epoch, 2);
        assert_eq!(origin.retired_anchors, vec![PathBuf::from("/b")]);
    }
}
