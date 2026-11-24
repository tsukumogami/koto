//! Data layer for the `koto dashboard` command.
//!
//! Responsible for reading session state from disk and maintaining an
//! up-to-date `SessionTree` via mtime-based incremental diffing.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;

use crate::engine::types::parse_timestamp_to_systemtime;
use crate::session::local::LocalBackend;
use crate::session::remote_feed::{
    sanitize_bucket_text, LocalView, ReadFailure, RemoteRow, RemoteSnapshot, RowStatus,
};
use crate::session::{SessionBackend, SessionInfo};

pub use koto_session_reader::dashboard::*;

/// The dashboard's session tree, whose bucket overlay records its read
/// failure as a [`ReadFailure`].
pub type SessionTree = koto_session_reader::dashboard::SessionTree<ReadFailure>;

/// Where the dashboard reads sessions from.
///
/// Kept in a module of its own and not re-exported, so code that has
/// [`SessionBackend`] in scope never has [`source::SessionSource`] there
/// too: both traits name `list`, `session_dir` and `count_unreadable`, and
/// a method call on a [`LocalBackend`] with both in scope is ambiguous.
pub mod source {
    pub use koto_session_reader::source::SessionSource;
}

impl source::SessionSource for LocalBackend {
    fn sessions_dir(&self) -> &Path {
        self.base_dir()
    }

    fn list(&self) -> Result<Vec<SessionInfo>> {
        SessionBackend::list(self)
    }

    fn session_dir(&self, id: &str) -> PathBuf {
        SessionBackend::session_dir(self, id)
    }

    fn count_unreadable(&self) -> usize {
        SessionBackend::count_unreadable(self)
    }
}

// `is_terminal_state` now lives in `crate::engine::persistence` as the single
// shared implementation (imported above); the `/workflows` projection writer
// reuses the same function.

/// A machine name read from the bucket, kept only when it has the shape
/// koto writes: 1 to 64 characters of `[A-Za-z0-9._-]`, never `unknown`.
/// Anything else reads as no recorded machine.
fn valid_machine(name: Option<&str>) -> Option<String> {
    let name = name?;
    crate::session::version::validate_machine_name(name)
        .is_ok()
        .then(|| name.to_string())
}

/// A remote row as the dashboard holds it: the summary another machine
/// published, in the shape a local row has. It has no local state file, so
/// `state_path` is empty and `mtime` is its last event's time.
///
/// A row listed as unknown carries only the listing's facts: its name, its
/// workspace and, as `mtime`, the state file's LastModified, which its idle
/// time is measured from. No state, intent, template or machine, so its
/// status reads `unknown` and it is never shown with a state it may not have.
///
/// Every string the row carries from the bucket, its name included, is
/// passed through [`sanitize_bucket_text`] here, so `--once` and the
/// interactive dashboard show the same text and neither meets a control
/// character another machine wrote. The row's displayed name is
/// `header.workflow`; key the row by the name as listed, `row.name`
/// ([`remote_key`]), since two listed names can sanitize to the same text.
/// The summary's name was compared with its key before this.
pub fn remote_session(row: &RemoteRow) -> CachedSession {
    let text = |s: &str| sanitize_bucket_text(s);
    let opt = |s: &Option<String>| s.as_deref().map(sanitize_bucket_text);
    let s = match &row.status {
        RowStatus::Shown(summary) => summary.as_ref(),
        RowStatus::Unknown {
            state_modified,
            session_id,
            ..
        } => {
            let mut header = make_empty_header();
            header.workflow = text(&row.name);
            header.session_id = opt(session_id).unwrap_or_default();
            return CachedSession {
                header,
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent: None,
                mtime: state_modified.unwrap_or(SystemTime::UNIX_EPOCH),
                state_path: PathBuf::new(),
                last_event_at: None,
                salient_var: None,
                is_unreadable: false,
                is_batch_task: false,
                is_request_leg: false,
                machine: None,
                workspace: Some(row.workspace.clone()),
                remote: true,
                imported_from: None,
                receded_by_source: row.receded_by_source,
            };
        }
    };
    let mut header = make_empty_header();
    header.workflow = text(&row.name);
    header.session_id = text(&s.session_id);
    header.created_at = text(&s.created_at);
    header.template_name = opt(&s.template_name);
    header.parent_workflow = opt(&s.parent);
    header.kind = opt(&s.kind);
    let last_event_at = opt(&s.last_event_at)
        .as_deref()
        .and_then(parse_timestamp_to_systemtime);
    CachedSession {
        header,
        current_state: opt(&s.current_state),
        is_terminal: s.is_terminal,
        is_blocked: s.is_blocked,
        intent: opt(&s.intent),
        mtime: last_event_at.unwrap_or(SystemTime::UNIX_EPOCH),
        state_path: PathBuf::new(),
        last_event_at,
        salient_var: opt(&s.salient_var),
        is_unreadable: false,
        is_batch_task: s.is_batch_task,
        is_request_leg: s.is_request_leg,
        machine: valid_machine(s.machine.as_deref()),
        workspace: Some(row.workspace.clone()),
        remote: true,
        imported_from: None,
        receded_by_source: row.receded_by_source,
    }
}

/// What a remote scan needs to know about the tree's local rows: their
/// `session_id`s, the workspace prefix and name of each one whose home is
/// known, and the source each imported one names.
///
/// The home prefix is the row's `workspace` when the caller set it, else
/// the prefix its header recorded at creation; nothing here makes a
/// request.
pub fn local_view(tree: &SessionTree) -> LocalView {
    local_view_of(tree, |_| true)
}

/// [`local_view`] over the local rows `keep` accepts by name.
pub fn local_view_of(tree: &SessionTree, keep: impl Fn(&str) -> bool) -> LocalView {
    let mut view = LocalView::default();
    for (name, s) in &tree.sessions {
        if s.remote || !keep(name) {
            continue;
        }
        if !s.header.session_id.is_empty() {
            view.session_ids.insert(s.header.session_id.clone());
        }
        let home = s.workspace.clone().or_else(|| {
            s.header
                .origin
                .as_ref()
                .and_then(|o| o.store.prefix.clone())
        });
        if let Some(home) = home {
            view.homes.insert((home, name.clone()));
        }
        if let Some(source) = &s.imported_from {
            view.imports.push(source.clone());
        }
    }
    view
}

/// Drop each remote row whose session this machine now holds a copy of,
/// so the session shows once, as its local row: by `session_id`, or for a
/// row that has none (no readable summary) by its workspace and name.
/// Returns whether any went.
fn drop_shadowed_remote(tree: &mut SessionTree) -> bool {
    let local = local_view(tree);
    let before = tree.sessions.len();
    tree.sessions.retain(|key, s| {
        if !s.remote {
            return true;
        }
        if s.header.session_id.is_empty() {
            let home = (
                s.workspace.clone().unwrap_or_default(),
                row_name(key, s).to_string(),
            );
            !local.homes.contains(&home)
        } else {
            !local.session_ids.contains(&s.header.session_id)
        }
    });
    tree.sessions.len() != before
}

/// Take a complete feed snapshot: its rows replace the tree's remote rows,
/// keyed by [`remote_key`] of the name as listed, so two rows whose names
/// sanitize to the same text both stay, except that the rows of a workspace whose
/// listing failed (every workspace, when the top-level listing failed) stay
/// as they were, since a failed listing says nothing about removal. A
/// session this machine holds a copy of stays its local row. The tallies,
/// the failure and whether a listing ceiling was reached come from the
/// snapshot.
pub fn apply_snapshot(tree: &mut SessionTree, snapshot: &RemoteSnapshot) {
    tree.sessions
        .retain(|_, s| !s.remote || s.workspace.as_deref().is_some_and(|ws| snapshot.keeps(ws)));
    for row in &snapshot.rows {
        let session = remote_session(row);
        tree.sessions
            .insert(remote_key(&row.name, &row.workspace), session);
    }
    tree.remote_unreadable = snapshot.unreadable;
    tree.remote_hidden = snapshot.hidden_sources;
    tree.remote_failure = snapshot.failure.clone();
    tree.remote_truncated = snapshot.truncated;
    drop_shadowed_remote(tree);
    tree.rebuild_roots();
}

/// Refresh the session tree with the bucket overlay applied: the same as
/// [`refresh_with_overlay`].
pub fn refresh(tree: &mut SessionTree, source: &dyn source::SessionSource) -> Result<()> {
    refresh_with_overlay(tree, source)
}

/// [`refresh_scan`], then the bucket overlay: a remote row whose session
/// this machine now holds a copy of leaves the tree, so the session shows
/// once, as its local row, and roots are rebuilt if any row went.
pub fn refresh_with_overlay(
    tree: &mut SessionTree,
    source: &dyn source::SessionSource,
) -> Result<()> {
    refresh_scan(tree, source)?;
    if drop_shadowed_remote(tree) {
        tree.rebuild_roots();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::liveness::idle_for;
    use super::*;
    use crate::engine::persistence::{append_event, append_header};
    use crate::engine::types::{EventPayload, StateFileHeader};
    use crate::session::local::LocalBackend;
    use crate::session::state_file_name;
    use std::collections::HashMap;
    use std::time::Duration;
    use tempfile::TempDir;

    fn make_header(name: &str, parent: Option<&str>) -> StateFileHeader {
        StateFileHeader {
            command_environment: None,
            schema_version: 1,
            workflow: name.to_string(),
            template_hash: "deadbeef".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            parent_workflow: parent.map(|s| s.to_string()),
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
            respawn_generation: None,
            priority: None,
            deadline: None,
            retry_count: None,
            agent_config: None,
            kind: None,
            root_session_id: None,
            parent_session_id: None,
        }
    }

    fn write_minimal_state_file(dir: &std::path::Path, name: &str) -> PathBuf {
        let path = dir.join(format!("koto-{}.state.jsonl", name));
        let header = make_header(name, None);
        append_header(&path, &header).unwrap();
        append_event(
            &path,
            &EventPayload::WorkflowInitialized {
                template_path: "/cache/test.json".to_string(),
                variables: HashMap::new(),
                spawn_entry: None,
                machine: None,
            },
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        path
    }

    fn write_state_file_with_transition(
        dir: &std::path::Path,
        name: &str,
        to_state: &str,
    ) -> PathBuf {
        let path = write_minimal_state_file(dir, name);
        append_event(
            &path,
            &EventPayload::Transitioned {
                from: None,
                to: to_state.to_string(),
                condition_type: "auto".to_string(),
                skip_if_matched: None,
                context_assignments: None,
                vars_matched: None,
            },
            "2026-01-01T00:00:01Z",
        )
        .unwrap();
        path
    }

    // -----------------------------------------------------------------------
    // scan_sessions: filtering of ~-named sessions
    // -----------------------------------------------------------------------

    #[test]
    fn scan_sessions_filters_epoch_branched_names() {
        let dir = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());

        // Create a regular session.
        backend.create("my-session").unwrap();
        write_minimal_state_file(&dir.path().join("my-session"), "my-session");

        // Create an epoch-branched session (contains ~).
        backend.create("my-session~1").unwrap();
        write_minimal_state_file(&dir.path().join("my-session~1"), "my-session~1");

        let result = scan_sessions(&backend).unwrap();
        assert_eq!(result.len(), 1, "epoch-branched session should be filtered");
        assert_eq!(result[0].0, "my-session");
    }

    #[test]
    fn scan_sessions_leaves_out_anchors_and_close_sessions() {
        use crate::session::anchor::{self, OpenRequest, Source};
        let dir = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());
        backend.create("my-session").unwrap();
        write_minimal_state_file(&dir.path().join("my-session"), "my-session");
        let req = OpenRequest {
            harness_session: "h1".to_string(),
            source: Source::Startup,
            process: None,
            dispatched_by: None,
            observed_key: Some("k1".to_string()),
            workspace: None,
            transcript_path: None,
            authority: None,
        };
        anchor::open(&backend, &req).unwrap();
        anchor::close(&backend, "k1", "h1").unwrap();
        assert_eq!(backend.list().unwrap().len(), 3);

        let result = scan_sessions(&backend).unwrap();
        let names: Vec<&str> = result.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["my-session"]);
    }

    #[test]
    fn scan_sessions_includes_all_non_epoch_branched() {
        let dir = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());

        // Create multiple regular sessions.
        for name in &["session-a", "session-b", "session-c"] {
            backend.create(name).unwrap();
            write_minimal_state_file(&dir.path().join(name), name);
        }

        // Create epoch-branched sessions that should be filtered.
        for name in &["session-a~1", "session-b~old"] {
            backend.create(name).unwrap();
            write_minimal_state_file(&dir.path().join(name), name);
        }

        let result = scan_sessions(&backend).unwrap();
        assert_eq!(result.len(), 3);
        let names: Vec<&str> = result.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"session-a"));
        assert!(names.contains(&"session-b"));
        assert!(names.contains(&"session-c"));
    }

    #[test]
    fn scan_sessions_empty_backend_returns_empty() {
        let dir = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());
        let result = scan_sessions(&backend).unwrap();
        assert!(result.is_empty());
    }

    // -----------------------------------------------------------------------
    // stat_and_diff: add/remove/mtime-change detection
    // -----------------------------------------------------------------------

    #[test]
    fn stat_and_diff_detects_adds() {
        let tree = SessionTree::new();
        let dir = TempDir::new().unwrap();
        let path = write_minimal_state_file(dir.path(), "new-session");

        let session_paths = vec![("new-session".to_string(), path)];
        let diff = stat_and_diff(&tree, &session_paths);

        assert_eq!(diff.adds.len(), 1);
        assert_eq!(diff.adds[0].0, "new-session");
        assert!(diff.removes.is_empty());
        assert!(diff.changed.is_empty());
    }

    #[test]
    fn stat_and_diff_detects_removes() {
        let dir = TempDir::new().unwrap();
        let path = write_minimal_state_file(dir.path(), "old-session");
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();

        let mut tree = SessionTree::new();
        tree.sessions.insert(
            "old-session".to_string(),
            CachedSession {
                header: make_header("old-session", None),
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent: None,
                mtime,
                state_path: path,
                last_event_at: None,
                salient_var: None,
                is_unreadable: false,
                is_batch_task: false,
                is_request_leg: false,
                machine: None,
                workspace: None,
                remote: false,
                imported_from: None,
                receded_by_source: false,
            },
        );

        // No sessions in the scan — should detect remove.
        let diff = stat_and_diff(&tree, &[]);

        assert!(diff.adds.is_empty());
        assert_eq!(diff.removes.len(), 1);
        assert_eq!(diff.removes[0], "old-session");
        assert!(diff.changed.is_empty());
    }

    #[test]
    fn stat_and_diff_detects_mtime_change() {
        let dir = TempDir::new().unwrap();
        let path = write_minimal_state_file(dir.path(), "changing-session");

        // Use a very old mtime for the cached entry to force detection.
        let old_mtime = SystemTime::UNIX_EPOCH;

        let mut tree = SessionTree::new();
        tree.sessions.insert(
            "changing-session".to_string(),
            CachedSession {
                header: make_header("changing-session", None),
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent: None,
                mtime: old_mtime,
                state_path: path.clone(),
                last_event_at: None,
                salient_var: None,
                is_unreadable: false,
                is_batch_task: false,
                is_request_leg: false,
                machine: None,
                workspace: None,
                remote: false,
                imported_from: None,
                receded_by_source: false,
            },
        );

        let session_paths = vec![("changing-session".to_string(), path)];
        let diff = stat_and_diff(&tree, &session_paths);

        assert!(diff.adds.is_empty());
        assert!(diff.removes.is_empty());
        assert_eq!(diff.changed.len(), 1, "should detect mtime change");
        assert_eq!(diff.changed[0].0, "changing-session");
    }

    #[test]
    fn stat_and_diff_unchanged_session_not_in_any_list() {
        let dir = TempDir::new().unwrap();
        let path = write_minimal_state_file(dir.path(), "stable-session");

        // Use the actual mtime from the file.
        let current_mtime = std::fs::metadata(&path).unwrap().modified().unwrap();

        let mut tree = SessionTree::new();
        tree.sessions.insert(
            "stable-session".to_string(),
            CachedSession {
                header: make_header("stable-session", None),
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent: None,
                mtime: current_mtime,
                state_path: path.clone(),
                last_event_at: None,
                salient_var: None,
                is_unreadable: false,
                is_batch_task: false,
                is_request_leg: false,
                machine: None,
                workspace: None,
                remote: false,
                imported_from: None,
                receded_by_source: false,
            },
        );

        let session_paths = vec![("stable-session".to_string(), path)];
        let diff = stat_and_diff(&tree, &session_paths);

        assert!(
            diff.adds.is_empty(),
            "stable session should not appear in adds"
        );
        assert!(
            diff.removes.is_empty(),
            "stable session should not appear in removes"
        );
        assert!(
            diff.changed.is_empty(),
            "stable session should not appear in changed"
        );
    }

    // -----------------------------------------------------------------------
    // read_session: graceful handling of parse errors
    // -----------------------------------------------------------------------

    #[test]
    fn read_session_missing_file_returns_fallback() {
        let path = PathBuf::from("/nonexistent/path/koto-missing.state.jsonl");
        let cached = read_session(&path);

        assert!(
            cached.current_state.is_none(),
            "missing file should produce None current_state"
        );
        assert!(
            !cached.is_terminal,
            "missing file should produce is_terminal = false"
        );
    }

    #[test]
    fn read_session_corrupted_file_returns_fallback() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("koto-corrupt.state.jsonl");
        std::fs::write(&path, "this is not valid jsonl").unwrap();

        let cached = read_session(&path);

        assert!(
            cached.current_state.is_none(),
            "corrupted file should produce None current_state"
        );
        assert!(
            !cached.is_terminal,
            "corrupted file should produce is_terminal = false"
        );
    }

    #[test]
    fn read_session_valid_file_without_state_change_returns_none_current_state() {
        let dir = TempDir::new().unwrap();
        let path = write_minimal_state_file(dir.path(), "my-session");

        let cached = read_session(&path);

        // Only a WorkflowInitialized event — no transition, so current_state is None.
        assert!(
            cached.current_state.is_none(),
            "session with only init event should have None current_state"
        );
        assert_eq!(cached.header.workflow, "my-session");
    }

    #[test]
    fn read_session_with_transition_returns_state() {
        let dir = TempDir::new().unwrap();
        let path = write_state_file_with_transition(dir.path(), "my-session", "gather");

        let cached = read_session(&path);

        assert_eq!(
            cached.current_state,
            Some("gather".to_string()),
            "session with transition should reflect current state"
        );
        // is_terminal will be false because the template path doesn't exist in tests.
        assert!(!cached.is_terminal);
    }

    #[test]
    fn read_session_with_failed_gate_sets_is_blocked() {
        let dir = TempDir::new().unwrap();
        // Write a session with a transition to "build" followed by a failed gate.
        let path = write_state_file_with_transition(dir.path(), "blocked-session", "build");
        append_event(
            &path,
            &EventPayload::GateEvaluated {
                state: "build".to_string(),
                gate: "lint-gate".to_string(),
                output: serde_json::Value::Null,
                outcome: "failed".to_string(),
                timestamp: "2026-01-01T00:00:02Z".to_string(),
                check: Default::default(),
                streams: None,
                poll: None,
            },
            "2026-01-01T00:00:02Z",
        )
        .unwrap();

        let cached = read_session(&path);

        assert_eq!(cached.current_state, Some("build".to_string()));
        assert!(
            cached.is_blocked,
            "failed gate should set is_blocked = true"
        );
        assert!(!cached.is_terminal);
    }

    #[test]
    fn read_session_with_passed_gate_is_not_blocked() {
        let dir = TempDir::new().unwrap();
        let path = write_state_file_with_transition(dir.path(), "passing-session", "build");
        append_event(
            &path,
            &EventPayload::GateEvaluated {
                state: "build".to_string(),
                gate: "lint-gate".to_string(),
                output: serde_json::Value::Null,
                outcome: "passed".to_string(),
                timestamp: "2026-01-01T00:00:02Z".to_string(),
                check: Default::default(),
                streams: None,
                poll: None,
            },
            "2026-01-01T00:00:02Z",
        )
        .unwrap();

        let cached = read_session(&path);

        assert_eq!(cached.current_state, Some("build".to_string()));
        assert!(
            !cached.is_blocked,
            "passed gate should leave is_blocked = false"
        );
    }

    // -----------------------------------------------------------------------
    // read_session: where the two memberships come from (I13)
    // -----------------------------------------------------------------------

    /// A state file whose init event carries a spawn entry, i.e. one a
    /// batch scheduler materialized.
    fn write_spawned_state_file(dir: &std::path::Path, name: &str) -> PathBuf {
        let path = dir.join(format!("koto-{}.state.jsonl", name));
        append_header(&path, &make_header(name, Some("coord"))).unwrap();
        append_event(
            &path,
            &EventPayload::WorkflowInitialized {
                template_path: "/cache/test.json".to_string(),
                variables: HashMap::new(),
                spawn_entry: Some(crate::engine::types::SpawnEntrySnapshot::new(
                    "child.md".to_string(),
                    Default::default(),
                    vec![],
                )),
                machine: None,
            },
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        path
    }

    fn write_leg_pointer(dir: &std::path::Path) {
        crate::engine::leg_pointer::write_pointer(
            dir,
            &crate::engine::leg_pointer::LegPointer {
                request_id: "req-abcdef".to_string(),
                leg_name: "reviewer-a".to_string(),
                bound_at: "2026-01-01T00:00:00.000Z".to_string(),
            },
        )
        .unwrap();
    }

    #[test]
    fn batch_membership_comes_from_the_init_events_spawn_entry() {
        let dir = TempDir::new().unwrap();
        let path = write_spawned_state_file(dir.path(), "spawned");

        let cached = read_session(&path);

        assert!(
            cached.is_batch_task,
            "a spawn entry on the init event is what marks a batch task"
        );
        assert!(
            !cached.is_request_leg,
            "a batch task is not a request leg by itself"
        );
    }

    #[test]
    fn a_session_with_no_spawn_entry_is_not_a_batch_task() {
        let dir = TempDir::new().unwrap();
        let path = write_minimal_state_file(dir.path(), "top-level");

        let cached = read_session(&path);

        assert!(!cached.is_batch_task);
        // The header carries the parentage a spawn entry would too, which
        // is exactly why membership cannot be read off it.
        assert!(!cached.is_request_leg);
    }

    #[test]
    fn leg_membership_comes_from_the_pointer_sidecar() {
        let dir = TempDir::new().unwrap();
        let path = write_minimal_state_file(dir.path(), "bound");
        write_leg_pointer(dir.path());

        let cached = read_session(&path);

        assert!(
            cached.is_request_leg,
            "the pointer in the session directory is the child-side record of a bind"
        );
        assert!(!cached.is_batch_task);
    }

    #[test]
    fn a_batch_task_bound_to_a_leg_reads_as_both() {
        let dir = TempDir::new().unwrap();
        let path = write_spawned_state_file(dir.path(), "dual");
        write_leg_pointer(dir.path());

        let cached = read_session(&path);

        assert!(cached.is_batch_task);
        assert!(cached.is_request_leg);
    }

    #[test]
    fn an_unreadable_pointer_leaves_the_session_readable() {
        let dir = TempDir::new().unwrap();
        let path = write_minimal_state_file(dir.path(), "garbled-pointer");
        std::fs::write(
            crate::engine::leg_pointer::pointer_path(dir.path()),
            "this is not toml = = =",
        )
        .unwrap();

        let cached = read_session(&path);

        assert!(
            !cached.is_request_leg,
            "a malformed pointer degrades to no membership"
        );
        assert!(
            !cached.is_unreadable,
            "and never makes the session itself unreadable"
        );
    }

    // -----------------------------------------------------------------------
    // refresh: orchestration
    // -----------------------------------------------------------------------

    #[test]
    fn refresh_adds_new_sessions_to_tree() {
        let dir = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());

        backend.create("session-a").unwrap();
        write_minimal_state_file(&dir.path().join("session-a"), "session-a");

        let mut tree = SessionTree::new();
        refresh(&mut tree, &backend).unwrap();

        assert!(tree.sessions.contains_key("session-a"));
        assert!(tree.roots.contains(&"session-a".to_string()));
    }

    #[test]
    fn refresh_removes_deleted_sessions_from_tree() {
        let dir = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());

        // Start with a session in the tree but not on disk.
        let mut tree = SessionTree::new();
        tree.sessions.insert(
            "gone-session".to_string(),
            CachedSession {
                header: make_header("gone-session", None),
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent: None,
                mtime: SystemTime::UNIX_EPOCH,
                state_path: PathBuf::from("/nonexistent"),
                last_event_at: None,
                salient_var: None,
                is_unreadable: false,
                is_batch_task: false,
                is_request_leg: false,
                machine: None,
                workspace: None,
                remote: false,
                imported_from: None,
                receded_by_source: false,
            },
        );

        refresh(&mut tree, &backend).unwrap();

        assert!(
            !tree.sessions.contains_key("gone-session"),
            "deleted session should be removed from tree"
        );
    }

    #[test]
    fn refresh_epoch_branched_sessions_not_included() {
        let dir = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());

        // Create a regular session and an epoch-branched session.
        backend.create("regular").unwrap();
        write_minimal_state_file(&dir.path().join("regular"), "regular");

        backend.create("regular~1").unwrap();
        write_minimal_state_file(&dir.path().join("regular~1"), "regular~1");

        let mut tree = SessionTree::new();
        refresh(&mut tree, &backend).unwrap();

        assert!(tree.sessions.contains_key("regular"));
        assert!(
            !tree.sessions.contains_key("regular~1"),
            "epoch-branched session must not appear in tree"
        );
    }

    // -----------------------------------------------------------------------
    // I7: surface unparseable sessions in the refresh tally
    // -----------------------------------------------------------------------

    #[test]
    fn refresh_counts_unparseable_session_instead_of_dropping() {
        let dir = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());

        // A readable session and a session whose state file has a corrupt header.
        backend.create("good").unwrap();
        write_minimal_state_file(&dir.path().join("good"), "good");

        backend.create("corrupt").unwrap();
        let corrupt_path = dir.path().join("corrupt").join(state_file_name("corrupt"));
        std::fs::write(&corrupt_path, "this is not valid jsonl header\n").unwrap();

        let mut tree = SessionTree::new();
        refresh(&mut tree, &backend).unwrap();

        // The corrupt session is excluded from the readable tree (its header
        // does not parse) but is surfaced via the unreadable tally rather than
        // being silently dropped.
        assert_eq!(
            tree.unreadable, 1,
            "exactly one session should be counted as unreadable"
        );
        assert!(
            tree.sessions.contains_key("good"),
            "the readable session must be present in the tree"
        );
        assert!(
            !tree.sessions.contains_key("corrupt"),
            "the corrupt session is not in the readable tree (its header does not parse)"
        );
    }

    // -----------------------------------------------------------------------
    // SessionSource: the scan runs against any `&dyn SessionSource`
    // -----------------------------------------------------------------------

    /// What a row shows, for comparing two trees row by row.
    type RowView = (
        Option<String>,
        bool,
        bool,
        bool,
        Option<String>,
        Option<SystemTime>,
        SystemTime,
        PathBuf,
    );

    fn row_views(tree: &SessionTree) -> std::collections::BTreeMap<String, RowView> {
        tree.sessions
            .iter()
            .map(|(key, s)| {
                let view = (
                    s.current_state.clone(),
                    s.is_terminal,
                    s.is_blocked,
                    s.is_unreadable,
                    parent_key(s),
                    s.last_event_at,
                    s.mtime,
                    s.state_path.clone(),
                );
                (key.clone(), view)
            })
            .collect()
    }

    #[test]
    fn refresh_through_a_session_source_matches_the_overlay_path() {
        let dir = TempDir::new().unwrap();
        let backend = LocalBackend::with_base_dir(dir.path().to_path_buf());

        backend.create("good").unwrap();
        write_state_file_with_transition(&dir.path().join("good"), "good", "working");

        backend.create("parent").unwrap();
        write_minimal_state_file(&dir.path().join("parent"), "parent");
        backend.create("child").unwrap();
        let child_path = dir.path().join("child").join(state_file_name("child"));
        append_header(&child_path, &make_header("child", Some("parent"))).unwrap();
        append_event(
            &child_path,
            &EventPayload::WorkflowInitialized {
                template_path: "/cache/test.json".to_string(),
                variables: HashMap::new(),
                spawn_entry: None,
                machine: None,
            },
            "2026-01-01T00:00:00Z",
        )
        .unwrap();

        backend.create("corrupt").unwrap();
        let corrupt_path = dir.path().join("corrupt").join(state_file_name("corrupt"));
        std::fs::write(&corrupt_path, "this is not valid jsonl header\n").unwrap();

        let as_source: &dyn source::SessionSource = &backend;
        assert_eq!(as_source.sessions_dir(), dir.path());
        let mut through_source = SessionTree::new();
        refresh_scan(&mut through_source, as_source).unwrap();

        let mut through_overlay = SessionTree::new();
        refresh_with_overlay(&mut through_overlay, &backend).unwrap();

        let rows = row_views(&through_source);
        assert_eq!(
            rows.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["child", "good", "parent"]
        );
        assert_eq!(rows["good"].0.as_deref(), Some("working"));
        assert_eq!(rows["child"].4.as_deref(), Some("parent"));
        assert_eq!(rows, row_views(&through_overlay));
        assert_eq!(through_source.roots, vec!["good", "parent"]);
        assert_eq!(through_source.roots, through_overlay.roots);
        assert_eq!(through_source.unreadable, 1);
        assert_eq!(through_source.unreadable, through_overlay.unreadable);
    }

    // -----------------------------------------------------------------------
    // rebuild_roots: parent-child relationships
    // -----------------------------------------------------------------------

    #[test]
    fn rebuild_roots_no_parent_is_root() {
        let mut sessions: HashMap<String, CachedSession> = HashMap::new();
        sessions.insert(
            "root-a".to_string(),
            CachedSession {
                header: make_header("root-a", None),
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent: None,
                mtime: SystemTime::UNIX_EPOCH,
                state_path: PathBuf::new(),
                last_event_at: None,
                salient_var: None,
                is_unreadable: false,
                is_batch_task: false,
                is_request_leg: false,
                machine: None,
                workspace: None,
                remote: false,
                imported_from: None,
                receded_by_source: false,
            },
        );
        sessions.insert(
            "root-b".to_string(),
            CachedSession {
                header: make_header("root-b", None),
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent: None,
                mtime: SystemTime::UNIX_EPOCH,
                state_path: PathBuf::new(),
                last_event_at: None,
                salient_var: None,
                is_unreadable: false,
                is_batch_task: false,
                is_request_leg: false,
                machine: None,
                workspace: None,
                remote: false,
                imported_from: None,
                receded_by_source: false,
            },
        );

        let roots = rebuild_roots(&sessions, &cyclic_keys(&sessions));
        assert_eq!(roots.len(), 2);
        assert!(roots.contains(&"root-a".to_string()));
        assert!(roots.contains(&"root-b".to_string()));
    }

    #[test]
    fn rebuild_roots_child_with_known_parent_is_not_root() {
        let mut sessions: HashMap<String, CachedSession> = HashMap::new();
        sessions.insert(
            "parent".to_string(),
            CachedSession {
                header: make_header("parent", None),
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent: None,
                mtime: SystemTime::UNIX_EPOCH,
                state_path: PathBuf::new(),
                last_event_at: None,
                salient_var: None,
                is_unreadable: false,
                is_batch_task: false,
                is_request_leg: false,
                machine: None,
                workspace: None,
                remote: false,
                imported_from: None,
                receded_by_source: false,
            },
        );
        sessions.insert(
            "child".to_string(),
            CachedSession {
                header: make_header("child", Some("parent")),
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent: None,
                mtime: SystemTime::UNIX_EPOCH,
                state_path: PathBuf::new(),
                last_event_at: None,
                salient_var: None,
                is_unreadable: false,
                is_batch_task: false,
                is_request_leg: false,
                machine: None,
                workspace: None,
                remote: false,
                imported_from: None,
                receded_by_source: false,
            },
        );

        let roots = rebuild_roots(&sessions, &cyclic_keys(&sessions));
        assert_eq!(roots.len(), 1);
        assert!(roots.contains(&"parent".to_string()));
        assert!(!roots.contains(&"child".to_string()));
    }

    #[test]
    fn rebuild_roots_orphaned_child_is_root() {
        let mut sessions: HashMap<String, CachedSession> = HashMap::new();
        // A session whose parent is not in the tree becomes a root.
        sessions.insert(
            "orphan".to_string(),
            CachedSession {
                header: make_header("orphan", Some("missing-parent")),
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent: None,
                mtime: SystemTime::UNIX_EPOCH,
                state_path: PathBuf::new(),
                last_event_at: None,
                salient_var: None,
                is_unreadable: false,
                is_batch_task: false,
                is_request_leg: false,
                machine: None,
                workspace: None,
                remote: false,
                imported_from: None,
                receded_by_source: false,
            },
        );

        let roots = rebuild_roots(&sessions, &cyclic_keys(&sessions));
        assert_eq!(roots.len(), 1);
        assert!(roots.contains(&"orphan".to_string()));
    }

    // -----------------------------------------------------------------------
    // read_detail: returns Some for sessions without gate evaluations
    // -----------------------------------------------------------------------

    #[test]
    fn read_detail_returns_data_for_evidence_only_session() {
        use crate::session::state_file_name;

        let tmp = TempDir::new().unwrap();
        let session_name = "test_sess";
        let state_path = tmp.path().join(state_file_name(session_name));

        // Write a minimal header.
        let header = StateFileHeader {
            command_environment: None,
            schema_version: 1,
            workflow: session_name.to_string(),
            template_hash: "hash".to_string(),
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
        };
        append_header(&state_path, &header).unwrap();

        // Append only an EvidenceSubmitted event (no GateEvaluated).
        append_event(
            &state_path,
            &EventPayload::EvidenceSubmitted {
                state: "gather".to_string(),
                fields: HashMap::new(),
                submitter_cwd: None,
                source: None,
                exported_fields: None,
            },
            "2026-01-01T00:01:00Z",
        )
        .unwrap();

        // read_detail should return Some even with no GateEvaluated event.
        let detail = read_detail(&state_path, session_name);
        assert!(
            detail.is_some(),
            "read_detail must return Some for evidence-only session"
        );
        let d = detail.unwrap();
        assert_eq!(d.session_id, session_name);
        assert!(d.gate_name.is_none());
        // The detail struct is populated; history/remaining contents may vary
        // by template, so we only assert the call succeeded above.
    }

    // -----------------------------------------------------------------------
    // I1: liveness classification (classify_liveness precedence D1-D7)
    // -----------------------------------------------------------------------

    /// Build a CachedSession with explicit liveness inputs for classifier tests.
    /// `idle` is applied by setting `last_event_at = now - idle`.
    fn liveness_session(
        current_state: Option<&str>,
        is_terminal: bool,
        is_blocked: bool,
        last_event_at: Option<SystemTime>,
    ) -> CachedSession {
        CachedSession {
            header: make_header("liveness", None),
            current_state: current_state.map(|s| s.to_string()),
            is_terminal,
            is_blocked,
            intent: None,
            mtime: SystemTime::UNIX_EPOCH,
            state_path: PathBuf::new(),
            last_event_at,
            salient_var: None,
            is_unreadable: false,
            is_batch_task: false,
            is_request_leg: false,
            machine: None,
            workspace: None,
            remote: false,
            imported_from: None,
            receded_by_source: false,
        }
    }

    #[test]
    fn classify_liveness_blocked_beats_stalled() {
        let now = SystemTime::now();
        // Gate-blocked session quiet for 3h (> stalled threshold) must be
        // NeedsYouBlocked, never NeedsYouStalled.
        let quiet = now - Duration::from_secs(3 * 60 * 60);
        let s = liveness_session(Some("build"), false, true, Some(quiet));
        assert_eq!(classify_liveness(&s, now), Liveness::NeedsYouBlocked);
    }

    #[test]
    fn classify_liveness_terminal_beats_idle() {
        let now = SystemTime::now();
        // Terminal session quiet for a long time is Done, never Idle/Stalled.
        let quiet = now - Duration::from_secs(10 * 60 * 60);
        let s = liveness_session(Some("done"), true, false, Some(quiet));
        assert_eq!(classify_liveness(&s, now), Liveness::Done);
    }

    #[test]
    fn classify_liveness_terminal_failed_is_needs_you_failed() {
        let now = SystemTime::now();
        let s = liveness_session(Some("build-failed"), true, false, Some(now));
        assert_eq!(classify_liveness(&s, now), Liveness::NeedsYouFailed);
    }

    #[test]
    fn classify_liveness_never_advanced_is_pending_not_stalled() {
        let now = SystemTime::now();
        // current_state == None, quiet for 3h: must be Pending, NOT stalled.
        let quiet = now - Duration::from_secs(3 * 60 * 60);
        let s = liveness_session(None, false, false, Some(quiet));
        assert_eq!(classify_liveness(&s, now), Liveness::Pending);
    }

    #[test]
    fn classify_liveness_advanced_then_silent_is_stalled() {
        let now = SystemTime::now();
        let quiet = now - Duration::from_secs(3 * 60 * 60);
        let s = liveness_session(Some("implement"), false, false, Some(quiet));
        assert_eq!(classify_liveness(&s, now), Liveness::NeedsYouStalled);
    }

    #[test]
    fn classify_liveness_recent_is_active() {
        let now = SystemTime::now();
        let recent = now - Duration::from_secs(30);
        let s = liveness_session(Some("implement"), false, false, Some(recent));
        assert_eq!(classify_liveness(&s, now), Liveness::Active);
    }

    #[test]
    fn classify_liveness_between_thresholds_is_idle() {
        let now = SystemTime::now();
        // 30 minutes: > active window (5m), < stalled threshold (2h).
        let mid = now - Duration::from_secs(30 * 60);
        let s = liveness_session(Some("implement"), false, false, Some(mid));
        assert_eq!(classify_liveness(&s, now), Liveness::Idle);
    }

    #[test]
    fn classify_liveness_rewound_reads_fresh_from_tail() {
        let now = SystemTime::now();
        // After a rewind, last_event_at is the post-rewind tail event, which is
        // recent — so the session reads as Active even if created long ago.
        let recent_tail = now - Duration::from_secs(10);
        let s = liveness_session(Some("review"), false, false, Some(recent_tail));
        assert_eq!(classify_liveness(&s, now), Liveness::Active);
    }

    #[test]
    fn classify_liveness_future_timestamp_clamps_to_zero_idle() {
        let now = SystemTime::now();
        // A future last_event_at (clock skew) clamps idle to zero -> Active.
        let future = now + Duration::from_secs(60 * 60);
        let s = liveness_session(Some("implement"), false, false, Some(future));
        assert_eq!(idle_for(&s, now), Duration::ZERO);
        assert_eq!(classify_liveness(&s, now), Liveness::Active);
    }

    #[test]
    fn classify_liveness_falls_back_to_mtime_when_no_event_ts() {
        let now = SystemTime::now();
        let mut s = liveness_session(Some("implement"), false, false, None);
        // No last_event_at: idle measured from mtime (recent here -> Active).
        s.mtime = now - Duration::from_secs(15);
        assert_eq!(classify_liveness(&s, now), Liveness::Active);
    }

    // -----------------------------------------------------------------------
    // I1: attention_key band ordering
    // -----------------------------------------------------------------------

    #[test]
    fn attention_key_band_order() {
        // NeedsYou (0) < Active (1) < Idle/fresh-Pending (2) < receded (3).
        let needs_you = attention_key(Liveness::NeedsYouBlocked, Duration::from_secs(60)).0;
        let active = attention_key(Liveness::Active, Duration::from_secs(60)).0;
        let idle = attention_key(Liveness::Idle, Duration::from_secs(60 * 30)).0;
        let done = attention_key(Liveness::Done, Duration::ZERO).0;
        assert!(needs_you < active);
        assert!(active < idle);
        assert!(idle < done);
    }

    #[test]
    fn attention_key_fresh_pending_visible_stale_pending_recedes() {
        // A fresh Pending (idle < active window) stays in band 2 (visible).
        let fresh = attention_key(Liveness::Pending, Duration::from_secs(60)).0;
        assert_eq!(fresh, 2);
        // An older Pending recedes to band 3.
        let stale = attention_key(Liveness::Pending, Duration::from_secs(10 * 60)).0;
        assert_eq!(stale, 3);
    }

    #[test]
    fn attention_key_abandoned_idle_recedes() {
        // Idle younger than abandoned stays visible (band 2).
        let visible = attention_key(Liveness::Idle, Duration::from_secs(30 * 60)).0;
        assert_eq!(visible, 2);
        // Idle older than abandoned (7d) recedes to band 3.
        let abandoned = attention_key(Liveness::Idle, Duration::from_secs(8 * 24 * 60 * 60)).0;
        assert_eq!(abandoned, 3);
    }

    #[test]
    fn attention_key_abandoned_needsyou_recedes() {
        // A recent needs-you session leads (band 0)...
        let recent = attention_key(Liveness::NeedsYouStalled, Duration::from_secs(3 * 60 * 60)).0;
        assert_eq!(recent, 0);
        let recent_blocked =
            attention_key(Liveness::NeedsYouBlocked, Duration::from_secs(3 * 60 * 60)).0;
        assert_eq!(recent_blocked, 0);
        // ...but once silent past the abandoned threshold (7d) it recedes,
        // regardless of being stalled or blocked -- a run dead for weeks is not
        // "waiting on you".
        let dead = 8 * 24 * 60 * 60;
        assert_eq!(
            attention_key(Liveness::NeedsYouStalled, Duration::from_secs(dead)).0,
            3
        );
        assert_eq!(
            attention_key(Liveness::NeedsYouBlocked, Duration::from_secs(dead)).0,
            3
        );
    }

    #[test]
    fn attention_key_within_band_longest_idle_first() {
        // Within a band, Reverse(idle) means longer idle sorts first.
        let longer = attention_key(Liveness::NeedsYouStalled, Duration::from_secs(5 * 60 * 60));
        let shorter = attention_key(Liveness::NeedsYouStalled, Duration::from_secs(3 * 60 * 60));
        assert!(
            longer < shorter,
            "longer idle must sort ahead within a band"
        );
    }

    // -----------------------------------------------------------------------
    // I2: derive_label (total fallback chain, never a bare id)
    // -----------------------------------------------------------------------

    /// Build a session for label tests with explicit label inputs.
    fn label_session(
        intent: Option<&str>,
        template_name: Option<&str>,
        current_state: Option<&str>,
        salient_var: Option<&str>,
    ) -> CachedSession {
        let mut header = make_header("display-id", None);
        header.template_name = template_name.map(|s| s.to_string());
        CachedSession {
            header,
            current_state: current_state.map(|s| s.to_string()),
            is_terminal: false,
            is_blocked: false,
            intent: intent.map(|s| s.to_string()),
            mtime: SystemTime::UNIX_EPOCH,
            state_path: PathBuf::new(),
            last_event_at: None,
            salient_var: salient_var.map(|s| s.to_string()),
            is_unreadable: false,
            is_batch_task: false,
            is_request_leg: false,
            machine: None,
            workspace: None,
            remote: false,
            imported_from: None,
            receded_by_source: false,
        }
    }

    #[test]
    fn derive_label_rung1_intent_wins() {
        let s = label_session(
            Some("Fix the login bug"),
            Some("work-on"),
            Some("implement"),
            None,
        );
        assert_eq!(derive_label(&s, "session-7"), "Fix the login bug");
    }

    #[test]
    fn derive_label_rung2_template_salient_state() {
        let s = label_session(None, Some("work-on"), Some("implement"), Some("issue-42"));
        assert_eq!(
            derive_label(&s, "session-7"),
            "work-on \u{b7} issue-42 \u{b7} implement"
        );
    }

    #[test]
    fn derive_label_rung3_template_state_when_no_salient() {
        let s = label_session(None, Some("work-on"), Some("implement"), None);
        assert_eq!(derive_label(&s, "session-7"), "work-on \u{b7} implement");
    }

    #[test]
    fn derive_label_rung4_untitled_template_when_no_state() {
        // Freshly WorkflowInitialized: template known, no current_state.
        let s = label_session(None, Some("work-on"), None, None);
        assert_eq!(derive_label(&s, "session-7"), "untitled (work-on)");
    }

    #[test]
    fn derive_label_never_bare_id_when_template_and_state_known() {
        let s = label_session(None, Some("work-on"), Some("implement"), None);
        let label = derive_label(&s, "session-7");
        assert_ne!(label, "session-7", "must never fall back to bare id");
        assert!(label.contains("work-on"));
    }

    #[test]
    fn derive_label_rung5_bare_id_only_when_template_empty() {
        // Corrupt header: no template name at all -> bare id is the last resort.
        let s = label_session(None, None, Some("implement"), None);
        assert_eq!(derive_label(&s, "session-7"), "session-7");
    }

    #[test]
    fn derive_label_empty_intent_falls_through() {
        // An empty intent string must not short-circuit the chain.
        let s = label_session(Some(""), Some("work-on"), Some("implement"), None);
        assert_eq!(derive_label(&s, "session-7"), "work-on \u{b7} implement");
    }

    const WS_A: &str = "aaaaaaaaaaaaaaaa";
    const WS_B: &str = "bbbbbbbbbbbbbbbb";

    fn unknown_row(ws: &str, name: &str) -> RemoteRow {
        RemoteRow {
            workspace: ws.to_string(),
            name: name.to_string(),
            status: RowStatus::Unknown {
                reason: "no-summary",
                state_modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000)),
                session_id: None,
            },
            receded_by_source: false,
        }
    }

    fn keys(tree: &SessionTree) -> Vec<String> {
        let mut keys: Vec<String> = tree.sessions.keys().cloned().collect();
        keys.sort();
        keys
    }

    #[test]
    fn an_unknown_remote_row_carries_only_the_listings_facts() {
        let s = remote_session(&unknown_row(WS_A, "old"));
        assert!(s.remote);
        assert_eq!(s.header.workflow, "old");
        assert_eq!(s.workspace.as_deref(), Some(WS_A));
        assert_eq!(s.current_state, None);
        assert_eq!(s.intent, None);
        assert_eq!(s.header.template_name, None);
        assert_eq!(s.machine, None);
        assert_eq!(s.last_event_at, None);
        assert_eq!(
            s.mtime,
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_000),
            "idle is measured from the state file's LastModified"
        );
    }

    #[test]
    fn a_remote_row_carries_every_bucket_string_sanitized() {
        let hostile = |head: &str| format!("{head}\t\n\u{1B}[2J\u{85}\u{2028}\u{202E}z");
        let clean = |head: &str| format!("{head}   [2J   z");
        let summary = crate::session::summary::SessionSummary {
            schema: 1,
            session_id: hostile("id"),
            name: hostile("n"),
            home: WS_A.to_string(),
            kind: Some(hostile("k")),
            parent: Some(hostile("p")),
            created_at: hostile("c"),
            template_name: Some(hostile("t")),
            current_state: Some(hostile("s")),
            is_terminal: false,
            is_blocked: true,
            failed: false,
            intent: Some(hostile("i")),
            salient_var: Some(hostile("v")),
            is_batch_task: false,
            is_request_leg: false,
            last_event_at: None,
            last_seq: 1,
            state_etag: "e".to_string(),
            machine: Some("bad\u{1B}machine".to_string()),
            sources: Vec::new(),
        };
        let row = RemoteRow {
            workspace: WS_A.to_string(),
            name: hostile("n"),
            status: RowStatus::Shown(Box::new(summary)),
            receded_by_source: false,
        };
        let s = remote_session(&row);
        assert_eq!(s.header.workflow, clean("n"));
        assert_eq!(s.header.session_id, clean("id"));
        assert_eq!(s.header.created_at, clean("c"));
        assert_eq!(s.header.kind, Some(clean("k")));
        assert_eq!(s.header.parent_workflow, Some(clean("p")));
        assert_eq!(s.header.template_name, Some(clean("t")));
        assert_eq!(s.current_state, Some(clean("s")));
        assert_eq!(s.intent, Some(clean("i")));
        assert_eq!(s.salient_var, Some(clean("v")));
        assert_eq!(s.machine, None, "a machine name that breaks the rules");

        let mut tree = SessionTree::new();
        apply_snapshot(
            &mut tree,
            &RemoteSnapshot {
                rows: vec![row, unknown_row(WS_A, &hostile("u"))],
                ..RemoteSnapshot::default()
            },
        );
        assert_eq!(
            keys(&tree),
            vec![
                remote_key(&hostile("n"), WS_A),
                remote_key(&hostile("u"), WS_A)
            ],
            "rows are keyed by their names as listed"
        );
        let shown: Vec<String> = keys(&tree)
            .iter()
            .map(|k| row_name(k, &tree.sessions[k]).to_string())
            .collect();
        assert_eq!(shown, vec![clean("n"), clean("u")], "and shown sanitized");
    }

    #[test]
    fn remote_rows_whose_names_sanitize_alike_both_stay() {
        let mut tree = SessionTree::new();
        apply_snapshot(
            &mut tree,
            &RemoteSnapshot {
                rows: vec![unknown_row(WS_A, "a\tb"), unknown_row(WS_A, "a\u{200B}b")],
                ..RemoteSnapshot::default()
            },
        );
        assert_eq!(tree.sessions.len(), 2, "neither row replaces the other");
        assert_eq!(tree.roots.len(), 2);
        for key in keys(&tree) {
            let shown = row_name(&key, &tree.sessions[&key]);
            assert_eq!(shown, "a b");
            assert!(!shown.contains(['\t', '\u{200B}']), "{shown:?}");
        }
    }

    #[test]
    fn apply_snapshot_keeps_the_rows_of_a_failed_listing() {
        let mut tree = SessionTree::new();
        apply_snapshot(
            &mut tree,
            &RemoteSnapshot {
                rows: vec![unknown_row(WS_A, "a"), unknown_row(WS_B, "b")],
                unreadable: 2,
                hidden_sources: 1,
                ..RemoteSnapshot::default()
            },
        );
        assert_eq!(keys(&tree), vec![format!("a@{WS_A}"), format!("b@{WS_B}")]);
        assert_eq!((tree.remote_unreadable, tree.remote_hidden), (2, 1));

        // A's listing fails: its row stays; B's, read and gone, drops.
        let failed = ReadFailure {
            class: crate::session::remote_feed::FailureClass::Unreachable,
            host: "127.0.0.1".to_string(),
            top_level: false,
            workspaces: vec![WS_A.to_string()],
        };
        apply_snapshot(
            &mut tree,
            &RemoteSnapshot {
                failure: Some(failed.clone()),
                ..RemoteSnapshot::default()
            },
        );
        assert_eq!(keys(&tree), vec![format!("a@{WS_A}")]);
        assert_eq!(tree.remote_failure.as_ref(), Some(&failed));

        // The top-level listing fails: every remote row stays.
        apply_snapshot(
            &mut tree,
            &RemoteSnapshot {
                failure: Some(ReadFailure {
                    top_level: true,
                    workspaces: Vec::new(),
                    ..failed
                }),
                ..RemoteSnapshot::default()
            },
        );
        assert_eq!(keys(&tree), vec![format!("a@{WS_A}")]);

        // A clean read without the session drops it and clears the failure.
        apply_snapshot(&mut tree, &RemoteSnapshot::default());
        assert!(tree.sessions.is_empty());
        assert_eq!(tree.remote_failure, None);
    }

    // -- forged parents and sources -------------------------------------------

    /// A blocked remote row `name` under [`WS_A`] whose summary names
    /// `parent`, shown from that summary.
    fn shown_row(name: &str, parent: Option<&str>) -> RemoteRow {
        RemoteRow {
            workspace: WS_A.to_string(),
            name: name.to_string(),
            status: RowStatus::Shown(Box::new(crate::session::summary::SessionSummary {
                schema: 1,
                session_id: format!("sid-{name}"),
                name: name.to_string(),
                home: WS_A.to_string(),
                kind: None,
                parent: parent.map(str::to_string),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                template_name: Some("gated".to_string()),
                current_state: Some("check".to_string()),
                is_terminal: false,
                is_blocked: true,
                failed: false,
                intent: None,
                salient_var: None,
                is_batch_task: false,
                is_request_leg: false,
                // Recent, so the blocked row sits in the needs-you band.
                last_event_at: Some(crate::engine::types::now_iso8601()),
                last_seq: 1,
                state_etag: "e".to_string(),
                machine: Some("machine-a".to_string()),
                sources: Vec::new(),
            })),
            receded_by_source: false,
        }
    }

    fn tree_of(rows: Vec<RemoteRow>) -> SessionTree {
        let mut tree = SessionTree::new();
        apply_snapshot(
            &mut tree,
            &RemoteSnapshot {
                rows,
                ..RemoteSnapshot::default()
            },
        );
        tree
    }

    #[test]
    fn dashboard_data_a_parent_naming_itself_leaves_the_row_a_root() {
        let tree = tree_of(vec![shown_row("loop", Some("loop"))]);
        let key = remote_key("loop", WS_A);
        assert_eq!(tree.roots, vec![key.clone()]);
        assert!(tree.detached.contains(&key));
        let s = &tree.sessions[&key];
        assert_eq!(tree.parent_of(&key, s), None, "no one's child");
        assert_eq!(s.header.parent_workflow.as_deref(), Some("loop"));
    }

    #[test]
    fn dashboard_data_two_parents_naming_each_other_leave_both_roots() {
        let tree = tree_of(vec![
            shown_row("ping", Some("pong")),
            shown_row("pong", Some("ping")),
            // Nested under one of the ring: a child, not a ring member.
            shown_row("leaf", Some("ping")),
            // A longer ring of three.
            shown_row("r1", Some("r2")),
            shown_row("r2", Some("r3")),
            shown_row("r3", Some("r1")),
        ]);
        let k = |name: &str| remote_key(name, WS_A);
        let mut roots = vec![k("ping"), k("pong"), k("r1"), k("r2"), k("r3")];
        roots.sort();
        assert_eq!(tree.roots, roots);
        let leaf = &tree.sessions[&k("leaf")];
        assert_eq!(tree.parent_of(&k("leaf"), leaf), Some(k("ping")));
        assert!(!tree.detached.contains(&k("leaf")));
    }

    #[test]
    fn dashboard_data_a_parent_cycle_shows_every_row_and_expands_without_end() {
        use crate::cli::dashboard_state::DashboardAppState;
        let mut state = DashboardAppState::new(500);
        state.tree = tree_of(vec![
            shown_row("loop", Some("loop")),
            shown_row("ping", Some("pong")),
            shown_row("pong", Some("ping")),
            shown_row("leaf", Some("ping")),
        ]);
        let k = |name: &str| remote_key(name, WS_A);
        for key in state.tree.sessions.keys() {
            state.expanded.insert(key.clone());
        }
        let mut shown: Vec<String> = state
            .visible_rows()
            .into_iter()
            .map(|r| r.session_id)
            .collect();
        shown.sort();
        let mut want = vec![k("leaf"), k("loop"), k("ping"), k("pong")];
        want.sort();
        assert_eq!(shown, want, "each row once, the leaf under its parent");
    }

    #[test]
    fn dashboard_data_a_row_named_as_a_source_recedes_whatever_its_liveness() {
        use super::liveness::{is_session_receded, session_attention};
        let now = SystemTime::now();
        let mut row = shown_row("y", None);
        let blocked = remote_session(&row);
        assert_eq!(session_attention(&blocked, now).0, 0, "needs-you");
        row.receded_by_source = true;
        let named = remote_session(&row);
        assert!(named.receded_by_source);
        assert_eq!(
            classify_liveness(&named, now),
            liveness::Liveness::NeedsYouBlocked
        );
        assert!(is_session_receded(&named, now));
    }

    /// Each out-of-range input another machine could write reads as no time,
    /// at once: a year of `u64::MAX` (once a loop of that many years), an
    /// hour of `u64::MAX` (once an overflow), a fraction of two-byte
    /// characters (once a slice inside one) and a word.
    #[test]
    fn dashboard_data_timestamp_parser_reads_each_out_of_range_input_as_none() {
        for input in [
            "18446744073709551615-01-01T00:00:00Z",
            "2026-01-01T18446744073709551615:00:00Z",
            "2026-01-01T00:18446744073709551615:00Z",
            "2026-01-01T00:00:18446744073709551615Z",
            "2026-01-01T00:00:00.\u{e9}\u{e9}Z",
            "2026-01-01T00:00:00.Z",
            "2026-01-01T00:00:00.1x2Z",
            "yesterday",
            "",
            "10000-01-01T00:00:00Z",
            "1969-12-31T23:59:59Z",
            "2026-13-01T00:00:00Z",
            "2026-02-30T00:00:00Z",
            "2026-01-00T00:00:00Z",
            "2026-01-01T24:00:00Z",
            "2026-01-01T00:60:00Z",
            "2026-01-01T00:00:61Z",
        ] {
            let started = std::time::Instant::now();
            assert_eq!(parse_timestamp_to_systemtime(input), None, "{input:?}");
            assert_eq!(compute_elapsed_since(input), Duration::ZERO, "{input:?}");
            assert!(started.elapsed() < Duration::from_secs(1), "{input:?}");
        }
    }

    #[test]
    fn dashboard_data_timestamp_parser_reads_the_calendar_edges() {
        let at = |s| {
            parse_timestamp_to_systemtime(s)
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|d| d.as_millis())
        };
        assert_eq!(at("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(at("2026-01-01T00:00:00Z"), Some(1_767_225_600_000));
        assert_eq!(at("2026-01-01T00:00:00.5Z"), Some(1_767_225_600_500));
        assert_eq!(at("2026-01-01T00:00:00.123456Z"), Some(1_767_225_600_123));
        assert_eq!(at("2024-02-29T23:59:60Z"), Some(1_709_251_200_000));
        assert!(at("9999-12-31T23:59:59.999Z").is_some());
    }
}
