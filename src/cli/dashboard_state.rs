//! Application state layer for the `koto dashboard` command.
//!
//! Owns the session tree, cursor position, view mode, and poll timing.
//! Provides cursor movement, keyboard dispatch, and depth-first tree
//! flattening via `visible_rows()`.

use std::collections::HashSet;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::cli::dashboard_data::{compute_elapsed_since, DetailData, SessionTree};

/// Which pane the user is currently viewing.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewMode {
    /// The session list (default view).
    List,
    /// The gate-detail pane for the focused session.
    Detail,
}

/// Aggregate task counts for a coordinator row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCounts {
    /// Total number of child sessions.
    pub total: usize,
    /// Number of children currently running (not terminal, has a current state).
    pub running: usize,
    /// Number of children that have reached a terminal state (not failed).
    pub done: usize,
    /// Number of children in a failed/error terminal state.
    pub failed: usize,
    /// Number of children in a blocked state (non-terminal, gate failed).
    pub blocked: usize,
    /// Number of done children that had blocking issues before finishing.
    pub done_blocked: usize,
}

/// A flattened row produced by `visible_rows()`.
#[derive(Debug, Clone)]
pub struct RowDescriptor {
    /// Depth in the session hierarchy (0 = root / coordinator, 1 = child).
    pub indent_depth: usize,
    /// Session identifier (key in `SessionTree::sessions`).
    pub session_id: String,
    /// Human-readable display name derived from the session ID.
    pub display_name: String,
    /// Current state derived from the event log, or `None` when unknown.
    pub state: Option<String>,
    /// Elapsed duration — passed as `Duration::from_secs(0)` here;
    /// Issue 5 wires up actual timing via mtime.
    pub elapsed: Duration,
    /// Aggregate child counts for coordinator rows; `None` for leaf sessions.
    pub task_counts: Option<TaskCounts>,
}

/// All mutable state owned by the dashboard event loop.
pub struct DashboardAppState {
    /// Hierarchical view of all sessions, refreshed every `poll_every_n_ticks`.
    pub tree: SessionTree,

    /// Index into the flattened visible-row list; drives cursor rendering.
    pub cursor_idx: usize,

    /// Whether the list or detail pane is active.
    pub view_mode: ViewMode,

    /// The session whose detail pane is displayed when `view_mode` is `Detail`.
    pub focused_id: Option<String>,

    /// Set of session IDs whose children are expanded in the list view.
    pub expanded: HashSet<String>,

    /// When `true`, the event loop should exit cleanly on the next tick.
    pub should_quit: bool,

    /// Counts 50ms ticks since the last file-poll cycle.
    pub tick_count: u32,

    /// How many ticks between file-poll cycles.
    ///
    /// Set to `max(1, poll_interval_ms / 50)` by `DashboardAppState::new`.
    pub poll_every_n_ticks: u32,

    /// Cached gate detail for the currently focused session, or `None` when
    /// no session is focused or the detail has not been loaded yet.
    pub detail_cache: Option<DetailData>,
}

impl DashboardAppState {
    /// Construct initial state.
    ///
    /// `poll_interval_ms` controls how often `dashboard_data::refresh` is
    /// called: every `max(1, poll_interval_ms / 50)` ticks (one tick = 50ms).
    pub fn new(poll_interval_ms: u64) -> Self {
        let poll_every_n_ticks = u32::try_from((poll_interval_ms / 50).max(1)).unwrap_or(1);
        Self {
            tree: SessionTree::new(),
            cursor_idx: 0,
            view_mode: ViewMode::List,
            focused_id: None,
            expanded: HashSet::new(),
            should_quit: false,
            tick_count: 0,
            poll_every_n_ticks,
            detail_cache: None,
        }
    }

    /// Handle a keyboard event, updating state accordingly.
    pub fn handle_key(&mut self, key: KeyEvent) {
        match (&self.view_mode, key.code, key.modifiers) {
            // Quit on 'q' or Ctrl+C from any view.
            (_, KeyCode::Char('q'), _) => {
                self.should_quit = true;
            }
            (_, KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }

            // Force refresh: set tick_count so next tick fires a poll.
            // Must be saturating_sub(1) so the event loop's pre-increment
            // lands exactly on poll_every_n_ticks (the check boundary).
            (_, KeyCode::Char('r'), _) => {
                self.tick_count = self.poll_every_n_ticks.saturating_sub(1);
            }

            // List-view navigation.
            (ViewMode::List, KeyCode::Char('j') | KeyCode::Down, _) => {
                self.move_cursor_down();
            }
            (ViewMode::List, KeyCode::Char('k') | KeyCode::Up, _) => {
                self.move_cursor_up();
            }

            // Enter in List view: transition to Detail for the focused session.
            (ViewMode::List, KeyCode::Enter, _) => {
                let rows = self.visible_rows();
                if let Some(row) = rows.get(self.cursor_idx) {
                    self.focused_id = Some(row.session_id.clone());
                    self.view_mode = ViewMode::Detail;
                    self.detail_cache = None;
                }
            }

            // Enter in Detail view: toggle expand/collapse if focused session has children.
            (ViewMode::Detail, KeyCode::Enter, _) => {
                if let Some(ref id) = self.focused_id.clone() {
                    // Only root sessions (coordinators) are expandable.
                    if self.tree.roots.contains(id) {
                        let has_children = self.session_has_children(id);
                        if has_children {
                            if self.expanded.contains(id) {
                                self.expanded.remove(id);
                            } else {
                                self.expanded.insert(id.clone());
                            }
                        }
                    }
                }
            }

            // Escape returns to List view and clears the focused session.
            (ViewMode::Detail, KeyCode::Esc, _) => {
                self.view_mode = ViewMode::List;
                self.focused_id = None;
                self.detail_cache = None;
            }

            // Detail-view cursor movement (navigates sessions while in detail mode).
            (ViewMode::Detail, KeyCode::Char('j') | KeyCode::Down, _) => {
                self.move_cursor_down();
                // Update focused_id to track cursor.
                let rows = self.visible_rows();
                if let Some(row) = rows.get(self.cursor_idx) {
                    self.focused_id = Some(row.session_id.clone());
                    self.detail_cache = None;
                }
            }
            (ViewMode::Detail, KeyCode::Char('k') | KeyCode::Up, _) => {
                self.move_cursor_up();
                // Update focused_id to track cursor.
                let rows = self.visible_rows();
                if let Some(row) = rows.get(self.cursor_idx) {
                    self.focused_id = Some(row.session_id.clone());
                    self.detail_cache = None;
                }
            }

            _ => {}
        }
    }

    /// Increment cursor by 1, bounded to the last visible row.
    fn move_cursor_down(&mut self) {
        let len = self.visible_rows().len();
        if len == 0 {
            return;
        }
        let max = len.saturating_sub(1);
        if self.cursor_idx < max {
            self.cursor_idx += 1;
        }
    }

    /// Decrement cursor by 1, bounded at 0.
    fn move_cursor_up(&mut self) {
        if self.cursor_idx > 0 {
            self.cursor_idx -= 1;
        }
    }

    /// Clamp the cursor to the valid range after a data refresh.
    pub fn clamp_cursor(&mut self) {
        let len = self.visible_rows().len();
        if len == 0 {
            self.cursor_idx = 0;
        } else {
            let max = len.saturating_sub(1);
            if self.cursor_idx > max {
                self.cursor_idx = max;
            }
        }
    }

    /// Check whether a session (by ID) has any children in the tree.
    fn session_has_children(&self, session_id: &str) -> bool {
        self.tree.sessions.values().any(|s| {
            s.header
                .parent_workflow
                .as_deref()
                .map(|p| p == session_id)
                .unwrap_or(false)
        })
    }

    /// Collect children of a root session, sorted by priority:
    /// failed → running → pending/blocked → terminal-but-not-failed.
    fn sorted_children(&self, root_id: &str) -> Vec<String> {
        let mut children: Vec<&str> = self
            .tree
            .sessions
            .iter()
            .filter(|(_, s)| {
                s.header
                    .parent_workflow
                    .as_deref()
                    .map(|p| p == root_id)
                    .unwrap_or(false)
            })
            .map(|(name, _)| name.as_str())
            .collect();

        children.sort_by_key(|name| {
            let s = &self.tree.sessions[*name];
            sort_priority(s.is_terminal, s.current_state.as_deref())
        });

        children.iter().map(|s| s.to_string()).collect()
    }

    /// Compute aggregate `TaskCounts` for a root session based on its children.
    fn task_counts_for_root(&self, root_id: &str) -> TaskCounts {
        let mut total = 0;
        let mut running = 0;
        let mut done = 0;
        let mut failed = 0;
        let mut blocked = 0;

        for s in self.tree.sessions.values() {
            if s.header
                .parent_workflow
                .as_deref()
                .map(|p| p == root_id)
                .unwrap_or(false)
            {
                total += 1;
                if s.is_terminal {
                    if is_failed_state(s.current_state.as_deref()) {
                        failed += 1;
                    } else {
                        done += 1;
                    }
                } else if s.is_blocked {
                    blocked += 1;
                } else if s.current_state.is_some() {
                    running += 1;
                }
                // else: pending — counted in total but not in running/done/failed/blocked
            }
        }

        TaskCounts {
            total,
            running,
            done,
            failed,
            blocked,
            done_blocked: 0,
        }
    }

    /// Build a depth-first ordered list of visible rows.
    ///
    /// Roots appear at depth 0. When a root is in `expanded`, its children
    /// follow immediately at depth 1, sorted by failure priority.
    pub fn visible_rows(&self) -> Vec<RowDescriptor> {
        let mut rows = Vec::new();

        for root_id in &self.tree.roots {
            // Root row.
            let root_state = self
                .tree
                .sessions
                .get(root_id)
                .and_then(|s| s.current_state.clone());

            let task_counts = if self.session_has_children(root_id) {
                Some(self.task_counts_for_root(root_id))
            } else {
                None
            };

            let root_elapsed = self
                .tree
                .sessions
                .get(root_id)
                .map(|s| compute_elapsed_since(&s.header.created_at))
                .unwrap_or(Duration::ZERO);

            rows.push(RowDescriptor {
                indent_depth: 0,
                session_id: root_id.clone(),
                display_name: root_id.clone(),
                state: root_state,
                elapsed: root_elapsed,
                task_counts,
            });

            // Children, only when expanded.
            if self.expanded.contains(root_id) {
                for child_id in self.sorted_children(root_id) {
                    let child_state = self
                        .tree
                        .sessions
                        .get(&child_id)
                        .and_then(|s| s.current_state.clone());

                    let child_elapsed = self
                        .tree
                        .sessions
                        .get(&child_id)
                        .map(|s| compute_elapsed_since(&s.header.created_at))
                        .unwrap_or(Duration::ZERO);

                    rows.push(RowDescriptor {
                        indent_depth: 1,
                        session_id: child_id.clone(),
                        display_name: child_id.clone(),
                        state: child_state,
                        elapsed: child_elapsed,
                        task_counts: None,
                    });
                }
            }
        }

        rows
    }
}

/// Classify a session's state into a sort priority bucket (lower = shown first).
///
/// 0 = failed, 1 = running, 2 = pending/blocked, 3 = terminal-not-failed
fn sort_priority(is_terminal: bool, state: Option<&str>) -> u8 {
    if is_terminal {
        if is_failed_state(state) {
            0 // failed
        } else {
            3 // terminal but not failed
        }
    } else if state.is_some() {
        1 // running
    } else {
        2 // pending/blocked
    }
}

/// Return true if `state` contains "failed" or "error" (case-insensitive).
fn is_failed_state(state: Option<&str>) -> bool {
    state
        .map(|s| {
            let lower = s.to_lowercase();
            lower.contains("failed") || lower.contains("error")
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::dashboard_data::CachedSession;
    use crate::engine::types::StateFileHeader;
    use std::path::PathBuf;
    use std::time::SystemTime;

    fn make_header(name: &str, parent: Option<&str>) -> StateFileHeader {
        StateFileHeader {
            schema_version: 1,
            workflow: name.to_string(),
            template_hash: "deadbeef".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            parent_workflow: parent.map(|s| s.to_string()),
            template_source_dir: None,
            session_id: String::new(),
            intent: None,
            template_name: None,
        }
    }

    fn make_session(
        name: &str,
        parent: Option<&str>,
        current_state: Option<&str>,
        is_terminal: bool,
    ) -> CachedSession {
        CachedSession {
            header: make_header(name, parent),
            current_state: current_state.map(|s| s.to_string()),
            is_terminal,
            is_blocked: false,
            intent: None,
            mtime: SystemTime::UNIX_EPOCH,
            state_path: PathBuf::new(),
        }
    }

    /// Build a tree with a single root and no children.
    fn single_root_state() -> DashboardAppState {
        let mut state = DashboardAppState::new(500);
        state.tree.sessions.insert(
            "root-a".to_string(),
            make_session("root-a", None, None, false),
        );
        state.tree.roots = vec!["root-a".to_string()];
        state
    }

    /// Build a tree with a coordinator (root) and three children.
    fn coordinator_with_children_state() -> DashboardAppState {
        let mut state = DashboardAppState::new(500);
        state.tree.sessions.insert(
            "coord".to_string(),
            make_session("coord", None, Some("running"), false),
        );
        state.tree.sessions.insert(
            "child-failed".to_string(),
            make_session("child-failed", Some("coord"), Some("failed-state"), true),
        );
        state.tree.sessions.insert(
            "child-running".to_string(),
            make_session("child-running", Some("coord"), Some("gathering"), false),
        );
        state.tree.sessions.insert(
            "child-pending".to_string(),
            make_session("child-pending", Some("coord"), None, false),
        );
        state.tree.sessions.insert(
            "child-done".to_string(),
            make_session("child-done", Some("coord"), Some("done"), true),
        );
        state.tree.roots = vec!["coord".to_string()];
        state
    }

    // -----------------------------------------------------------------------
    // DashboardAppState::new
    // -----------------------------------------------------------------------

    #[test]
    fn new_sets_poll_every_n_ticks_correctly() {
        let s = DashboardAppState::new(500);
        assert_eq!(s.poll_every_n_ticks, 10); // 500 / 50

        let s = DashboardAppState::new(0);
        assert_eq!(s.poll_every_n_ticks, 1); // max(1, 0/50)

        let s = DashboardAppState::new(50);
        assert_eq!(s.poll_every_n_ticks, 1); // 50 / 50 = 1

        let s = DashboardAppState::new(100);
        assert_eq!(s.poll_every_n_ticks, 2); // 100 / 50 = 2
    }

    #[test]
    fn new_initializes_defaults() {
        let s = DashboardAppState::new(500);
        assert_eq!(s.cursor_idx, 0);
        assert!(s.focused_id.is_none());
        assert!(s.expanded.is_empty());
        assert!(!s.should_quit);
        assert_eq!(s.tick_count, 0);
        assert!(s.detail_cache.is_none());
        assert!(s.tree.sessions.is_empty());
        assert!(s.tree.roots.is_empty());
    }

    // -----------------------------------------------------------------------
    // cursor movement
    // -----------------------------------------------------------------------

    #[test]
    fn cursor_j_increments() {
        let mut state = DashboardAppState::new(500);
        state
            .tree
            .sessions
            .insert("a".to_string(), make_session("a", None, None, false));
        state
            .tree
            .sessions
            .insert("b".to_string(), make_session("b", None, None, false));
        state.tree.roots = vec!["a".to_string(), "b".to_string()];
        assert_eq!(state.cursor_idx, 0);
        state.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(state.cursor_idx, 1);
    }

    #[test]
    fn cursor_k_decrements() {
        let mut state = DashboardAppState::new(500);
        state
            .tree
            .sessions
            .insert("a".to_string(), make_session("a", None, None, false));
        state
            .tree
            .sessions
            .insert("b".to_string(), make_session("b", None, None, false));
        state.tree.roots = vec!["a".to_string(), "b".to_string()];
        state.cursor_idx = 1;
        state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(state.cursor_idx, 0);
    }

    #[test]
    fn cursor_bounded_at_zero_on_k() {
        let mut state = single_root_state();
        assert_eq!(state.cursor_idx, 0);
        state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(state.cursor_idx, 0, "cursor must not go below 0");
    }

    #[test]
    fn cursor_bounded_at_last_on_j() {
        let mut state = single_root_state();
        // Only one row visible; cursor should stay at 0.
        state.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(state.cursor_idx, 0, "cursor must not exceed last row");
    }

    #[test]
    fn cursor_down_arrow_increments() {
        let mut state = DashboardAppState::new(500);
        state
            .tree
            .sessions
            .insert("a".to_string(), make_session("a", None, None, false));
        state
            .tree
            .sessions
            .insert("b".to_string(), make_session("b", None, None, false));
        state.tree.roots = vec!["a".to_string(), "b".to_string()];
        state.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(state.cursor_idx, 1);
    }

    #[test]
    fn cursor_up_arrow_decrements() {
        let mut state = DashboardAppState::new(500);
        state
            .tree
            .sessions
            .insert("a".to_string(), make_session("a", None, None, false));
        state
            .tree
            .sessions
            .insert("b".to_string(), make_session("b", None, None, false));
        state.tree.roots = vec!["a".to_string(), "b".to_string()];
        state.cursor_idx = 1;
        state.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(state.cursor_idx, 0);
    }

    // -----------------------------------------------------------------------
    // q and Ctrl+C set should_quit
    // -----------------------------------------------------------------------

    #[test]
    fn q_sets_should_quit() {
        let mut state = DashboardAppState::new(500);
        assert!(!state.should_quit);
        state.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(state.should_quit);
    }

    #[test]
    fn ctrl_c_sets_should_quit() {
        let mut state = DashboardAppState::new(500);
        state.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(state.should_quit);
    }

    // -----------------------------------------------------------------------
    // r sets tick_count to poll_every_n_ticks
    // -----------------------------------------------------------------------

    #[test]
    fn r_forces_refresh_via_tick_count() {
        let mut state = DashboardAppState::new(500);
        state.tick_count = 0;
        state.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        assert_eq!(state.tick_count, state.poll_every_n_ticks.saturating_sub(1));
    }

    // -----------------------------------------------------------------------
    // ViewMode transitions
    // -----------------------------------------------------------------------

    #[test]
    fn enter_in_list_transitions_to_detail() {
        let mut state = single_root_state();
        assert!(matches!(state.view_mode, ViewMode::List));
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(state.view_mode, ViewMode::Detail));
        assert_eq!(state.focused_id, Some("root-a".to_string()));
    }

    #[test]
    fn escape_in_detail_returns_to_list() {
        let mut state = single_root_state();
        state.view_mode = ViewMode::Detail;
        state.focused_id = Some("root-a".to_string());
        state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(state.view_mode, ViewMode::List));
        assert!(state.focused_id.is_none());
    }

    #[test]
    fn enter_in_detail_toggles_expand_for_coordinator() {
        let mut state = coordinator_with_children_state();
        // Expand first. (state must be mut to call handle_key later)
        state.expanded.insert("coord".to_string());
        state.view_mode = ViewMode::Detail;
        state.focused_id = Some("coord".to_string());
        // Enter should collapse.
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!state.expanded.contains("coord"), "should collapse");
        // Enter again should expand.
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(state.expanded.contains("coord"), "should expand again");
    }

    // -----------------------------------------------------------------------
    // expand/collapse
    // -----------------------------------------------------------------------

    #[test]
    fn expand_root_shows_children_in_visible_rows() {
        let mut state = coordinator_with_children_state();
        // Initially not expanded — only the root row is visible.
        let rows = state.visible_rows();
        assert_eq!(rows.len(), 1, "collapsed root shows only itself");

        // Expand.
        state.expanded.insert("coord".to_string());
        let rows = state.visible_rows();
        assert!(rows.len() > 1, "expanded root shows children");
        assert_eq!(rows[0].session_id, "coord");
        assert!(rows[1..].iter().all(|r| r.indent_depth == 1));
    }

    #[test]
    fn collapse_hides_children() {
        let mut state = coordinator_with_children_state();
        state.expanded.insert("coord".to_string());
        assert!(state.visible_rows().len() > 1);

        state.expanded.remove("coord");
        let rows = state.visible_rows();
        assert_eq!(rows.len(), 1, "collapsed root hides children");
    }

    // -----------------------------------------------------------------------
    // visible_rows ordering
    // -----------------------------------------------------------------------

    #[test]
    fn visible_rows_depth_first_order() {
        let mut state = DashboardAppState::new(500);
        state.tree.sessions.insert(
            "root-1".to_string(),
            make_session("root-1", None, None, false),
        );
        state.tree.sessions.insert(
            "root-2".to_string(),
            make_session("root-2", None, None, false),
        );
        state.tree.sessions.insert(
            "child-1-a".to_string(),
            make_session("child-1-a", Some("root-1"), None, false),
        );
        state.tree.roots = vec!["root-1".to_string(), "root-2".to_string()];
        state.expanded.insert("root-1".to_string());

        let rows = state.visible_rows();
        // Depth-first: root-1, child-1-a, root-2
        assert_eq!(rows[0].session_id, "root-1");
        assert_eq!(rows[1].session_id, "child-1-a");
        assert_eq!(rows[2].session_id, "root-2");
    }

    #[test]
    fn visible_rows_children_sorted_by_priority() {
        let mut state = coordinator_with_children_state();
        state.expanded.insert("coord".to_string());
        let rows = state.visible_rows();
        // Skip the root row at index 0.
        let child_rows: Vec<&RowDescriptor> = rows.iter().filter(|r| r.indent_depth == 1).collect();
        assert!(
            child_rows.len() >= 3,
            "should have at least 3 child rows, got {}",
            child_rows.len()
        );
        // failed-state should be first.
        assert_eq!(child_rows[0].session_id, "child-failed");
        // running (has state, not terminal) should be second.
        assert_eq!(child_rows[1].session_id, "child-running");
        // pending (no state, not terminal) before terminal-not-failed.
        assert_eq!(child_rows[2].session_id, "child-pending");
        assert_eq!(child_rows[3].session_id, "child-done");
    }

    // -----------------------------------------------------------------------
    // task_counts for coordinators
    // -----------------------------------------------------------------------

    #[test]
    fn task_counts_aggregated_for_coordinator() {
        let state = coordinator_with_children_state();
        // Don't need to expand to get root row's task_counts.
        let rows = state.visible_rows();
        let root_row = &rows[0];
        assert!(root_row.task_counts.is_some());
        let counts = root_row.task_counts.as_ref().unwrap();
        assert_eq!(counts.total, 4, "should count all 4 children");
        assert_eq!(counts.failed, 1);
        assert_eq!(counts.running, 1);
        assert_eq!(counts.done, 1);
        // pending counts in total but not in running/done/failed
    }

    #[test]
    fn task_counts_none_for_leaf_sessions() {
        let mut state = coordinator_with_children_state();
        state.expanded.insert("coord".to_string());
        let rows = state.visible_rows();
        let leaf_rows: Vec<&RowDescriptor> = rows.iter().filter(|r| r.indent_depth == 1).collect();
        for leaf in leaf_rows {
            assert!(
                leaf.task_counts.is_none(),
                "leaf session '{}' should have no task_counts",
                leaf.session_id
            );
        }
    }

    // -----------------------------------------------------------------------
    // is_failed_state helper
    // -----------------------------------------------------------------------

    #[test]
    fn is_failed_state_recognizes_failed_and_error() {
        assert!(is_failed_state(Some("failed")));
        assert!(is_failed_state(Some("job-failed")));
        assert!(is_failed_state(Some("FAILED")));
        assert!(is_failed_state(Some("error")));
        assert!(is_failed_state(Some("parse_error")));
        assert!(!is_failed_state(Some("done")));
        assert!(!is_failed_state(Some("running")));
        assert!(!is_failed_state(None));
    }

    // -----------------------------------------------------------------------
    // clamp_cursor
    // -----------------------------------------------------------------------

    #[test]
    fn clamp_cursor_brings_cursor_within_bounds() {
        let mut state = single_root_state();
        state.cursor_idx = 99;
        state.clamp_cursor();
        assert_eq!(
            state.cursor_idx, 0,
            "cursor should clamp to last visible row"
        );
    }

    #[test]
    fn clamp_cursor_on_empty_tree_resets_to_zero() {
        let mut state = DashboardAppState::new(500);
        state.cursor_idx = 5;
        state.clamp_cursor();
        assert_eq!(state.cursor_idx, 0);
    }
}
