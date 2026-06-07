//! Application state layer for the `koto dashboard` command.
//!
//! Owns the session tree, cursor position, view mode, and poll timing.
//! Provides cursor movement, keyboard dispatch, and depth-first tree
//! flattening via `visible_rows()`.

use std::cmp::Reverse;
use std::collections::HashSet;
use std::time::{Duration, SystemTime};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::cli::dashboard_data::{compute_elapsed_since, DetailData, SessionTree};

/// Which tab is active in the detail pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DashboardTab {
    Summary,
    History,
    Remaining,
}

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
    /// Tree connector string for display (e.g. "├─ ", "└─ ").
    /// Empty for root/depth-0 rows.
    pub connector: String,
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

    /// The last mtime seen when `detail_cache` was populated, for invalidation.
    pub detail_cache_mtime: Option<SystemTime>,

    /// The session ID that was focused when `detail_cache` was last populated.
    pub detail_cache_session: Option<String>,

    /// Which tab is currently selected in the detail pane.
    pub active_tab: DashboardTab,
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
            detail_cache_mtime: None,
            detail_cache_session: None,
            active_tab: DashboardTab::Summary,
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

            // List-view navigation updates focused_id.
            (ViewMode::List, KeyCode::Char('j') | KeyCode::Down, _) => {
                self.move_cursor_down();
                let rows = self.visible_rows();
                if let Some(row) = rows.get(self.cursor_idx) {
                    self.focused_id = Some(row.session_id.clone());
                }
            }
            (ViewMode::List, KeyCode::Char('k') | KeyCode::Up, _) => {
                self.move_cursor_up();
                let rows = self.visible_rows();
                if let Some(row) = rows.get(self.cursor_idx) {
                    self.focused_id = Some(row.session_id.clone());
                }
            }

            // List-view expand/collapse via arrow keys or vim keys.
            (ViewMode::List, KeyCode::Right | KeyCode::Char('l'), _) => {
                if let Some(ref id) = self.focused_id.clone() {
                    if self.session_has_children(id) {
                        self.expanded.insert(id.clone());
                    }
                }
            }
            (ViewMode::List, KeyCode::Left | KeyCode::Char('h'), _) => {
                if let Some(ref id) = self.focused_id.clone() {
                    self.expanded.remove(id);
                }
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

            // Detail-view expand/collapse via arrow keys or vim keys.
            (ViewMode::Detail, KeyCode::Right | KeyCode::Char('l'), _) => {
                if let Some(ref id) = self.focused_id.clone() {
                    if self.session_has_children(id) {
                        self.expanded.insert(id.clone());
                    }
                }
            }
            (ViewMode::Detail, KeyCode::Left | KeyCode::Char('h'), _) => {
                if let Some(ref id) = self.focused_id.clone() {
                    self.expanded.remove(id);
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

            (_, KeyCode::Tab, _) => {
                self.active_tab = match self.active_tab {
                    DashboardTab::Summary => DashboardTab::History,
                    DashboardTab::History => DashboardTab::Remaining,
                    DashboardTab::Remaining => DashboardTab::Summary,
                };
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
    /// Roots appear at depth 0, sorted by health severity. When a session is
    /// in `expanded`, its children follow immediately at depth+1 with tree
    /// connectors (`├─ `/`└─ `), also sorted by health severity.
    pub fn visible_rows(&self) -> Vec<RowDescriptor> {
        let mut rows = Vec::new();

        // Sort roots by health severity.
        let mut sorted_roots: Vec<&String> = self.tree.roots.iter().collect();
        sorted_roots.sort_by_key(|id| {
            self.tree
                .sessions
                .get(*id)
                .map(session_sort_key)
                .unwrap_or((3, Reverse(SystemTime::UNIX_EPOCH)))
        });

        for root_id in sorted_roots {
            self.append_rows_for_session(root_id, &[], &mut rows);
        }

        rows
    }

    /// Recursively append rows for a session and, if expanded, its children.
    ///
    /// `ancestor_is_last[i]` is `true` when the ancestor at depth `i` was the
    /// last child of its own parent. This drives the `│ `/`  ` prefix and
    /// `├─ `/`└─ ` connector at the current level.
    fn append_rows_for_session(
        &self,
        session_id: &str,
        ancestor_is_last: &[bool],
        rows: &mut Vec<RowDescriptor>,
    ) {
        let depth = ancestor_is_last.len();

        // Build the connector string (empty for root rows).
        let connector = if depth == 0 {
            String::new()
        } else {
            let mut s = String::new();
            // For each ancestor above the current level, emit a continuation
            // prefix: "│ " when that ancestor was not the last child, "  " when
            // it was (no more siblings to connect).
            for &is_last in &ancestor_is_last[..depth - 1] {
                s.push_str(if is_last { "  " } else { "│ " });
            }
            // Current-level connector.
            let is_last = *ancestor_is_last.last().unwrap_or(&false);
            s.push_str(if is_last { "└─ " } else { "├─ " });
            s
        };

        let session = self.tree.sessions.get(session_id);
        let state = session.and_then(|s| s.current_state.clone());
        let elapsed = session
            .map(|s| compute_elapsed_since(&s.header.created_at))
            .unwrap_or(Duration::ZERO);

        let task_counts = if self.session_has_children(session_id) {
            Some(self.task_counts_for_root(session_id))
        } else {
            None
        };

        rows.push(RowDescriptor {
            indent_depth: depth,
            connector,
            session_id: session_id.to_string(),
            display_name: session_id.to_string(),
            state,
            elapsed,
            task_counts,
        });

        // Recurse into children if this session is expanded.
        if self.expanded.contains(session_id) {
            let mut children: Vec<&String> = self
                .tree
                .sessions
                .iter()
                .filter(|(_, s)| s.header.parent_workflow.as_deref() == Some(session_id))
                .map(|(name, _)| name)
                .collect();
            children.sort_by_key(|id| {
                self.tree
                    .sessions
                    .get(*id)
                    .map(session_sort_key)
                    .unwrap_or((3, Reverse(SystemTime::UNIX_EPOCH)))
            });

            let child_count = children.len();
            for (i, child_id) in children.iter().enumerate() {
                let is_last = i == child_count - 1;
                let mut new_ancestors = ancestor_is_last.to_vec();
                new_ancestors.push(is_last);
                self.append_rows_for_session(child_id, &new_ancestors, rows);
            }
        }
    }
}

/// Sort key for health-severity ordering.
///
/// Priority buckets: failed=0, blocked=1, running=2, unknown=3, done=4.
/// Within each bucket, sessions are ordered by most-recent mtime descending.
pub(crate) fn session_sort_key(
    session: &crate::cli::dashboard_data::CachedSession,
) -> (u8, Reverse<SystemTime>) {
    let bucket = if session.is_terminal {
        if is_failed_state(session.current_state.as_deref()) {
            0 // failed
        } else {
            4 // done
        }
    } else if session.is_blocked {
        1 // blocked
    } else if session.current_state.is_some() {
        2 // running
    } else {
        3 // unknown/pending
    };
    (bucket, Reverse(session.mtime))
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
            last_event_at: None,
            salient_var: None,
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
    // Tab cycling
    // -----------------------------------------------------------------------

    #[test]
    fn tab_key_cycles_through_tabs() {
        let mut state = DashboardAppState::new(500);
        assert_eq!(state.active_tab, DashboardTab::Summary);

        // Press Tab once: Summary -> History
        state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(state.active_tab, DashboardTab::History);

        // Press Tab again: History -> Remaining
        state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(state.active_tab, DashboardTab::Remaining);

        // Press Tab again: Remaining -> Summary (wrap)
        state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(state.active_tab, DashboardTab::Summary);
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

    // -----------------------------------------------------------------------
    // three-level tree connectors
    // -----------------------------------------------------------------------

    #[test]
    fn visible_rows_three_level_tree_connectors() {
        // Tree: root -> [child-a (non-last), child-b (last)]
        //       child-a -> [gc-1 (last)]
        let mut state = DashboardAppState::new(500);
        state.tree.sessions.insert(
            "root".to_string(),
            make_session("root", None, Some("running"), false),
        );
        state.tree.sessions.insert(
            "child-a".to_string(),
            make_session("child-a", Some("root"), Some("running"), false),
        );
        state.tree.sessions.insert(
            "child-b".to_string(),
            make_session("child-b", Some("root"), Some("done"), true),
        );
        state.tree.sessions.insert(
            "gc-1".to_string(),
            make_session("gc-1", Some("child-a"), Some("running"), false),
        );
        state.tree.roots = vec!["root".to_string()];
        state.expanded.insert("root".to_string());
        state.expanded.insert("child-a".to_string());

        let rows = state.visible_rows();
        // Expect root, child-a, gc-1, child-b (health-severity order: running before done)
        assert_eq!(rows.len(), 4);

        let root_row = rows.iter().find(|r| r.session_id == "root").unwrap();
        assert_eq!(root_row.indent_depth, 0);
        assert_eq!(root_row.connector, "");

        let gc_row = rows.iter().find(|r| r.session_id == "gc-1").unwrap();
        assert_eq!(gc_row.indent_depth, 2);
        assert!(
            !gc_row.connector.is_empty(),
            "grandchild must have a non-empty connector"
        );
    }

    // -----------------------------------------------------------------------
    // health-severity ordering
    // -----------------------------------------------------------------------

    #[test]
    fn visible_rows_health_severity_ordering() {
        let mut state = DashboardAppState::new(500);

        let make_session_blocked = |name: &str,
                                    parent: Option<&str>,
                                    state_str: Option<&str>,
                                    is_terminal: bool,
                                    is_blocked: bool| {
            let mut s = make_session(name, parent, state_str, is_terminal);
            s.is_blocked = is_blocked;
            s
        };

        state.tree.sessions.insert(
            "s-done".to_string(),
            make_session_blocked("s-done", None, Some("done"), true, false),
        );
        state.tree.sessions.insert(
            "s-failed".to_string(),
            make_session_blocked("s-failed", None, Some("failed"), true, false),
        );
        state.tree.sessions.insert(
            "s-running".to_string(),
            make_session_blocked("s-running", None, Some("gather"), false, false),
        );
        state.tree.sessions.insert(
            "s-blocked".to_string(),
            make_session_blocked("s-blocked", None, Some("waiting"), false, true),
        );
        state.tree.sessions.insert(
            "s-unknown".to_string(),
            make_session_blocked("s-unknown", None, None, false, false),
        );

        // Let visible_rows sort — do not pre-sort roots.
        state.tree.roots = state.tree.sessions.keys().cloned().collect();

        let rows = state.visible_rows();
        assert_eq!(rows.len(), 5);

        let positions: std::collections::HashMap<&str, usize> = rows
            .iter()
            .enumerate()
            .map(|(i, r)| (r.session_id.as_str(), i))
            .collect();
        assert!(
            positions["s-failed"] < positions["s-blocked"],
            "failed must come before blocked"
        );
        assert!(
            positions["s-blocked"] < positions["s-running"],
            "blocked must come before running"
        );
        assert!(
            positions["s-running"] < positions["s-unknown"],
            "running must come before unknown"
        );
        assert!(
            positions["s-unknown"] < positions["s-done"],
            "unknown must come before done"
        );
    }
}
