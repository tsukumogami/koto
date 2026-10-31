//! Application state layer for the `koto dashboard` command.
//!
//! Owns the session tree, cursor position, view mode, and poll timing.
//! Provides cursor movement, keyboard dispatch, and depth-first tree
//! flattening via `visible_rows()`.

use std::cell::Cell;
use std::collections::HashSet;
use std::time::{Duration, SystemTime};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::session_view::{KeyPreview, Sanitized, SessionViewSnapshot, SnapshotError};

use crate::cli::dashboard_data::liveness::{attention_key, idle_for, is_receded};
use crate::cli::dashboard_data::{
    classify_liveness, derive_label, CachedSession, DetailData, Liveness, SessionTree,
};

/// Which tab is active in the detail pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DashboardTab {
    Summary,
    History,
    Remaining,
    Session,
}

/// Which pane the user is currently viewing.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewMode {
    /// The session list (default view).
    List,
    /// The gate-detail pane for the focused session.
    Detail,
}

/// Which containers a session belongs to, rendered as a per-row badge.
///
/// The dashboard's count is derived from parent-workflow parentage alone and
/// is not a batch count, so a batch or a request cannot be inferred from it.
/// Membership is carried as its own attribute instead: one count per surface,
/// with dual membership stated rather than implied
/// (DESIGN-request-lifecycle.md Decision 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Membership {
    /// Neither a batch task nor a request leg.
    None,
    /// Spawned by a batch scheduler (its init event carries a spawn entry).
    Batch,
    /// Bound to a request leg (its session directory carries a leg pointer).
    Leg,
    /// Both: a batch task that was later bound to a leg.
    Both,
}

impl Membership {
    /// Combine the two independent memberships a session can hold.
    pub fn from_flags(is_batch_task: bool, is_request_leg: bool) -> Self {
        match (is_batch_task, is_request_leg) {
            (true, true) => Membership::Both,
            (true, false) => Membership::Batch,
            (false, true) => Membership::Leg,
            (false, false) => Membership::None,
        }
    }
}

/// Aggregate child counts for a coordinator row.
///
/// Every field counts child *sessions* — those whose header names this row as
/// their `parent_workflow`. Nothing here consults the batch view, which is why
/// the column is `Children` and not `Tasks`.
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
    /// Idle duration (time since last activity), used for the time column.
    pub elapsed: Duration,
    /// Read-time liveness classification for this row.
    pub liveness: Liveness,
    /// Aggregate child counts for coordinator rows; `None` for leaf sessions.
    pub task_counts: Option<TaskCounts>,
    /// Which containers this session belongs to, independent of the count.
    pub membership: Membership,
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

    /// When `true`, the receded set (done + abandoned + stale-pending) is
    /// included in `visible_rows`. Toggled with `a`. Default `false`, matching
    /// the `--once` default and the TUI's attention-first view.
    pub show_receded: bool,

    /// Loaded data and key cursor for the Session tab.
    pub session_tab: SessionTabState,
}

/// What the Session tab should load on this tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionLoad {
    /// The snapshot is missing or built for another session or mtime.
    Snapshot,
    /// The snapshot is current; the focused key's preview is not.
    Preview(String),
    /// Everything on screen is current.
    Idle,
}

/// The preview read for one (session, key, mtime), kept so the same key is
/// not read again until the state file changes.
#[derive(Debug)]
pub struct CachedPreview {
    session: String,
    key: String,
    mtime: SystemTime,
    pub preview: KeyPreview,
}

/// Session-tab data: the snapshot, the key cursor, and the focused key's
/// preview. Loads are driven by [`SessionTabState::tick`], never by a thread.
#[derive(Default)]
pub struct SessionTabState {
    session: Option<String>,
    mtime: Option<SystemTime>,
    /// The snapshot, or why there is none.
    pub snapshot: Option<Result<SessionViewSnapshot, Sanitized>>,
    /// Index into the snapshot's keys.
    pub cursor: usize,
    preview: Option<CachedPreview>,
    /// Bounded preview reads issued so far.
    pub preview_reads: usize,
    /// Key-table rows built by the renderer so far.
    pub rows_built: Cell<usize>,
}

impl SessionTabState {
    /// The session the loaded snapshot belongs to.
    pub fn loaded_session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    fn key_count(&self) -> usize {
        match &self.snapshot {
            Some(Ok(snap)) => snap.keys.len(),
            _ => 0,
        }
    }

    /// The readable name of the key under the cursor, when it has one.
    fn focused_key(&self) -> Option<&str> {
        match &self.snapshot {
            Some(Ok(snap)) => snap.keys.get(self.cursor)?.raw.as_deref(),
            _ => None,
        }
    }

    /// The cached preview, only when it is for the key under the cursor.
    pub fn focused_preview(&self) -> Option<&KeyPreview> {
        let cached = self.preview.as_ref()?;
        let current = self.session.as_deref()?;
        (cached.session == current
            && Some(cached.mtime) == self.mtime
            && Some(cached.key.as_str()) == self.focused_key())
        .then_some(&cached.preview)
    }

    /// Decide what to load for `session` at `mtime`. Pure: the loop calls
    /// it, and tests assert on it.
    pub fn plan(&self, session: &str, mtime: SystemTime) -> SessionLoad {
        if self.session.as_deref() != Some(session) || self.mtime != Some(mtime) {
            return SessionLoad::Snapshot;
        }
        match self.focused_key() {
            Some(key) if self.focused_preview().is_none() => SessionLoad::Preview(key.to_string()),
            _ => SessionLoad::Idle,
        }
    }

    /// Run one tick of loading: the snapshot when `plan` asks for it, then
    /// at most one bounded preview read for the focused key.
    pub fn tick(
        &mut self,
        session: &str,
        mtime: SystemTime,
        snapshot: impl FnOnce(&str) -> Result<SessionViewSnapshot, SnapshotError>,
        preview: impl FnOnce(&str, &str) -> KeyPreview,
    ) {
        if self.plan(session, mtime) == SessionLoad::Snapshot {
            if self.session.as_deref() != Some(session) {
                self.cursor = 0;
            }
            self.snapshot = Some(snapshot(session).map_err(|e| e.reason()));
            self.session = Some(session.to_string());
            self.mtime = Some(mtime);
            self.preview = None;
            self.cursor = self.cursor.min(self.key_count().saturating_sub(1));
        }
        if let SessionLoad::Preview(key) = self.plan(session, mtime) {
            let read = preview(session, &key);
            self.preview_reads += 1;
            self.preview = Some(CachedPreview {
                session: session.to_string(),
                key,
                mtime,
                preview: read,
            });
        }
    }

    /// Move the key cursor by `delta`, staying inside the listing.
    pub fn move_cursor(&mut self, delta: isize) {
        let max = self.key_count().saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(delta).min(max);
    }
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
            show_receded: false,
            session_tab: SessionTabState::default(),
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

            // Toggle the receded (done + abandoned) set in/out of the list.
            (_, KeyCode::Char('a'), _) => {
                self.show_receded = !self.show_receded;
                self.clamp_cursor();
            }

            // List-view navigation updates focused_id.
            (ViewMode::List, KeyCode::Char('j') | KeyCode::Down, _) => {
                // Nothing is focused at launch: the first press focuses the
                // row under the cursor (row 1) instead of moving past it.
                if self.focused_id.is_some() {
                    self.move_cursor_down();
                }
                let rows = self.visible_rows();
                if let Some(row) = rows.get(self.cursor_idx) {
                    self.focused_id = Some(row.session_id.clone());
                }
            }
            (ViewMode::List, KeyCode::Char('k') | KeyCode::Up, _) => {
                if self.focused_id.is_some() {
                    self.move_cursor_up();
                }
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
                    self.invalidate_detail();
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
                self.invalidate_detail();
            }

            // On the Session tab the detail view's cursor walks the key
            // table instead of the session list.
            (ViewMode::Detail, KeyCode::Char('j') | KeyCode::Down, _)
                if self.active_tab == DashboardTab::Session =>
            {
                self.session_tab.move_cursor(1);
            }
            (ViewMode::Detail, KeyCode::Char('k') | KeyCode::Up, _)
                if self.active_tab == DashboardTab::Session =>
            {
                self.session_tab.move_cursor(-1);
            }
            (ViewMode::Detail, KeyCode::PageDown, _)
                if self.active_tab == DashboardTab::Session =>
            {
                self.session_tab.move_cursor(10);
            }
            (ViewMode::Detail, KeyCode::PageUp, _) if self.active_tab == DashboardTab::Session => {
                self.session_tab.move_cursor(-10);
            }

            // Detail-view cursor movement (navigates sessions while in detail mode).
            (ViewMode::Detail, KeyCode::Char('j') | KeyCode::Down, _) => {
                self.move_cursor_down();
                // Update focused_id to track cursor.
                let rows = self.visible_rows();
                if let Some(row) = rows.get(self.cursor_idx) {
                    self.focused_id = Some(row.session_id.clone());
                    self.invalidate_detail();
                }
            }
            (ViewMode::Detail, KeyCode::Char('k') | KeyCode::Up, _) => {
                self.move_cursor_up();
                // Update focused_id to track cursor.
                let rows = self.visible_rows();
                if let Some(row) = rows.get(self.cursor_idx) {
                    self.focused_id = Some(row.session_id.clone());
                    self.invalidate_detail();
                }
            }

            (_, KeyCode::Tab, _) => {
                self.active_tab = match self.active_tab {
                    DashboardTab::Summary => DashboardTab::History,
                    DashboardTab::History => DashboardTab::Remaining,
                    DashboardTab::Remaining => DashboardTab::Session,
                    DashboardTab::Session => DashboardTab::Summary,
                };
            }

            _ => {}
        }
    }

    /// Drop the cached detail and the markers the refresh guard compares
    /// against. Clearing only `detail_cache` leaves the guard believing the
    /// cache is still current for the session, so it is never reloaded.
    fn invalidate_detail(&mut self) {
        self.detail_cache = None;
        self.detail_cache_mtime = None;
        self.detail_cache_session = None;
    }

    /// Reload the focused session's detail when the cache is for another
    /// session or an older mtime. `read` is given the state-file path and ID.
    pub fn refresh_detail(
        &mut self,
        read: impl FnOnce(&std::path::Path, &str) -> Option<DetailData>,
    ) {
        let Some(id) = self.focused_id.clone() else {
            return;
        };
        let Some(session) = self.tree.sessions.get(&id) else {
            return;
        };
        let (mtime, path) = (Some(session.mtime), session.state_path.clone());
        if self.detail_cache_session.as_deref() != Some(id.as_str())
            || mtime != self.detail_cache_mtime
        {
            self.detail_cache = read(&path, &id);
            self.detail_cache_mtime = mtime;
            self.detail_cache_session = Some(id);
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
    /// Roots appear at depth 0, sorted by attention band (needs-you -> active ->
    /// idle/fresh-pending -> receded), longest-idle first within a band. The
    /// receded set (done + abandoned + stale pending) is excluded by default and
    /// included only when `show_receded` is set. When a root is in `expanded`,
    /// its children follow immediately at depth+1 with tree connectors, ordered
    /// by the same attention key.
    pub fn visible_rows(&self) -> Vec<RowDescriptor> {
        let now = SystemTime::now();
        let mut rows = Vec::new();

        // Sort roots by attention key.
        let mut sorted_roots: Vec<&String> = self.tree.roots.iter().collect();
        sorted_roots.sort_by_key(|id| {
            self.tree
                .sessions
                .get(*id)
                .map(|s| session_sort_key(s, now))
                .unwrap_or(receded_fallback_key())
        });

        for root_id in sorted_roots {
            // Apply the read-time default filter: skip receded roots unless the
            // user has revealed them.
            if !self.show_receded {
                if let Some(s) = self.tree.sessions.get(root_id.as_str()) {
                    let liveness = classify_liveness(s, now);
                    let idle = idle_for(s, now);
                    if is_receded(liveness, idle) {
                        continue;
                    }
                }
            }
            self.append_rows_for_session(root_id, None, &[], &mut rows, now);
        }

        rows
    }

    /// Count the receded sessions (done + abandoned idle + stale pending) so the
    /// render layer can show a collapsed summary. Counts every session in the
    /// tree, regardless of `show_receded`.
    pub fn receded_summary(&self) -> RecededSummary {
        let now = SystemTime::now();
        let mut done = 0usize;
        let mut abandoned = 0usize;
        for s in self.tree.sessions.values() {
            let liveness = classify_liveness(s, now);
            let idle = idle_for(s, now);
            if !is_receded(liveness, idle) {
                continue;
            }
            if liveness == Liveness::Done {
                done += 1;
            } else {
                abandoned += 1;
            }
        }
        RecededSummary { done, abandoned }
    }

    /// Count the sessions in each non-receded liveness band, for the all-clear
    /// row when nothing needs the user.
    pub fn live_summary(&self) -> LiveSummary {
        let now = SystemTime::now();
        let mut needs_you = 0usize;
        let mut active = 0usize;
        let mut idle = 0usize;
        for s in self.tree.sessions.values() {
            let liveness = classify_liveness(s, now);
            let idle_dur = idle_for(s, now);
            match attention_key(liveness, idle_dur).0 {
                0 => needs_you += 1,
                1 => active += 1,
                2 => idle += 1,
                _ => {}
            }
        }
        LiveSummary {
            needs_you,
            active,
            idle,
        }
    }

    /// Recursively append rows for a session and, if expanded, its children.
    ///
    /// `parent_label` is the derived label of the parent row (when this is a
    /// child), used to prefix child labels as `parent ▸ leaf`.
    /// `ancestor_is_last[i]` is `true` when the ancestor at depth `i` was the
    /// last child of its own parent. This drives the `│ `/`  ` prefix and
    /// `├─ `/`└─ ` connector at the current level.
    fn append_rows_for_session(
        &self,
        session_id: &str,
        parent_label: Option<&str>,
        ancestor_is_last: &[bool],
        rows: &mut Vec<RowDescriptor>,
        now: SystemTime,
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
        let elapsed = session.map(|s| idle_for(s, now)).unwrap_or(Duration::ZERO);
        let liveness = session
            .map(|s| classify_liveness(s, now))
            .unwrap_or(Liveness::Pending);

        // Leaf label, with parent prefix for child rows (`parent ▸ leaf`).
        let leaf_label = session
            .map(|s| derive_label(s, session_id))
            .unwrap_or_else(|| session_id.to_string());
        let display_name = match parent_label {
            Some(parent) => format!("{parent} \u{25b8} {leaf_label}"),
            None => leaf_label.clone(),
        };

        let task_counts = if self.session_has_children(session_id) {
            Some(self.task_counts_for_root(session_id))
        } else {
            None
        };

        // Membership is orthogonal to the count above: a session can be a
        // batch task, a request leg, both, or neither, and none of those
        // change how many children it has.
        let membership = session
            .map(|s| Membership::from_flags(s.is_batch_task, s.is_request_leg))
            .unwrap_or(Membership::None);

        rows.push(RowDescriptor {
            indent_depth: depth,
            connector,
            session_id: session_id.to_string(),
            display_name,
            state,
            elapsed,
            liveness,
            task_counts,
            membership,
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
                    .map(|s| session_sort_key(s, now))
                    .unwrap_or(receded_fallback_key())
            });

            let child_count = children.len();
            for (i, child_id) in children.iter().enumerate() {
                let is_last = i == child_count - 1;
                let mut new_ancestors = ancestor_is_last.to_vec();
                new_ancestors.push(is_last);
                self.append_rows_for_session(
                    child_id,
                    Some(&leaf_label),
                    &new_ancestors,
                    rows,
                    now,
                );
            }
        }
    }
}

/// Collapsed counts for the receded (hidden-by-default) set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecededSummary {
    /// Sessions in a terminal, non-failed (`Done`) state.
    pub done: usize,
    /// Abandoned idle + stale pending sessions.
    pub abandoned: usize,
}

impl RecededSummary {
    /// Total receded sessions.
    pub fn total(&self) -> usize {
        self.done + self.abandoned
    }
}

/// Counts of live (non-receded) sessions per attention band, for the all-clear
/// row shown when no session needs the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveSummary {
    /// Sessions in the needs-you band.
    pub needs_you: usize,
    /// Active sessions.
    pub active: usize,
    /// Idle (non-abandoned) + fresh-pending sessions.
    pub idle: usize,
}

/// Attention sort key for a session: `(band, Reverse(idle))`.
///
/// Bands: needs-you (0) -> active (1) -> idle/fresh-pending (2) -> receded (3).
/// Within a band, longest-idle sorts first.
pub(crate) fn session_sort_key(
    session: &CachedSession,
    now: SystemTime,
) -> (u8, std::cmp::Reverse<Duration>) {
    let liveness = classify_liveness(session, now);
    let idle = idle_for(session, now);
    attention_key(liveness, idle)
}

/// Fallback sort key for a session id missing from the tree: receded band,
/// zero idle (sorts last within the receded band).
fn receded_fallback_key() -> (u8, std::cmp::Reverse<Duration>) {
    (3, std::cmp::Reverse(Duration::ZERO))
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

    fn make_session(
        name: &str,
        parent: Option<&str>,
        current_state: Option<&str>,
        is_terminal: bool,
    ) -> CachedSession {
        // Default to a fresh last_event_at so non-terminal sessions read as
        // Active / fresh-Pending (visible by default). Tests that want a stale
        // / abandoned session override `last_event_at` after construction.
        CachedSession {
            header: make_header(name, parent),
            current_state: current_state.map(|s| s.to_string()),
            is_terminal,
            is_blocked: false,
            intent: None,
            mtime: SystemTime::UNIX_EPOCH,
            state_path: PathBuf::new(),
            last_event_at: Some(SystemTime::now()),
            salient_var: None,
            is_unreadable: false,
            is_batch_task: false,
            is_request_leg: false,
            machine: None,
            workspace: None,
            remote: false,
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
        // The first press focuses row 1; the second moves to row 2.
        state.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
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
        state.focused_id = Some("b".to_string());
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
        assert_eq!(state.cursor_idx, 0);
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
        state.focused_id = Some("b".to_string());
        state.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(state.cursor_idx, 0);
    }

    fn two_root_state() -> DashboardAppState {
        let mut state = DashboardAppState::new(500);
        for n in ["a", "b"] {
            state
                .tree
                .sessions
                .insert(n.to_string(), make_session(n, None, Some("work"), false));
        }
        state.tree.roots = vec!["a".to_string(), "b".to_string()];
        state
    }

    fn press(state: &mut DashboardAppState, code: KeyCode) {
        state.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn detail_for(id: &str) -> DetailData {
        DetailData {
            session_id: id.to_string(),
            gate_name: None,
            command: None,
            result: None,
            elapsed: Duration::ZERO,
            evidence: Vec::new(),
            current_state: Some("work".to_string()),
            directive: None,
            intent: None,
            template_name: None,
            history: Vec::new(),
            remaining: Vec::new(),
        }
    }

    /// The tick loop's detail refresh, with a reader that always succeeds.
    fn tick_detail(state: &mut DashboardAppState) {
        state.refresh_detail(|_, id| Some(detail_for(id)));
    }

    #[test]
    fn first_j_or_down_focuses_row_one_not_row_two() {
        for code in [KeyCode::Char('j'), KeyCode::Down] {
            let mut state = two_root_state();
            let first = state.visible_rows()[0].session_id.clone();
            let second = state.visible_rows()[1].session_id.clone();
            assert!(state.focused_id.is_none());
            press(&mut state, code);
            assert_eq!(state.cursor_idx, 0, "{code:?}");
            assert_eq!(state.focused_id.as_deref(), Some(first.as_str()));
            press(&mut state, code);
            assert_eq!(state.focused_id.as_deref(), Some(second.as_str()));
        }
    }

    #[test]
    fn j_then_enter_loads_the_detail_without_extra_tabs() {
        let mut state = two_root_state();
        press(&mut state, KeyCode::Char('j'));
        // The loop loads the focused session's detail while still in the list.
        tick_detail(&mut state);
        assert!(state.detail_cache.is_some());

        press(&mut state, KeyCode::Enter);
        tick_detail(&mut state);
        let id = state.focused_id.clone().unwrap();
        let detail = state.detail_cache.as_ref().expect("detail after Enter");
        assert_eq!(detail.session_id, id);
    }

    #[test]
    fn j_k_enter_reloads_the_detail_for_the_same_session() {
        let mut state = two_root_state();
        press(&mut state, KeyCode::Char('j'));
        tick_detail(&mut state);
        press(&mut state, KeyCode::Char('j'));
        tick_detail(&mut state);
        press(&mut state, KeyCode::Char('k'));
        tick_detail(&mut state);
        press(&mut state, KeyCode::Enter);
        tick_detail(&mut state);
        assert!(state.detail_cache.is_some());
        press(&mut state, KeyCode::Tab);
        tick_detail(&mut state);
        assert!(state.detail_cache.is_some());
    }

    #[test]
    fn refresh_detail_keeps_the_cache_while_session_and_mtime_match() {
        let mut state = two_root_state();
        press(&mut state, KeyCode::Char('j'));
        tick_detail(&mut state);
        state.refresh_detail(|_, _| panic!("unchanged session must not be re-read"));
        assert!(state.detail_cache.is_some());
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

        // Press Tab again: Remaining -> Session
        state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(state.active_tab, DashboardTab::Session);

        // Press Tab again: Session -> Summary (wrap)
        state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(state.active_tab, DashboardTab::Summary);
    }

    // -----------------------------------------------------------------------
    // Session tab loading and key cursor
    // -----------------------------------------------------------------------

    use crate::session_view::{
        sanitize, KeyEntry, Locality, SessionFacts, SessionViewSnapshot, SnapshotError,
    };
    use std::cell::RefCell;

    fn key_entry(name: &str) -> KeyEntry {
        KeyEntry {
            display: sanitize(name),
            raw: Some(name.to_string()),
            size: Some(10),
            writer: None,
            created_at: None,
            locality: Locality::Local,
            stale: false,
        }
    }

    fn snapshot_of(names: &[&str]) -> SessionViewSnapshot {
        SessionViewSnapshot {
            facts: SessionFacts {
                name: "wf".to_string(),
                session_id: None,
                created_at: None,
                intent: None,
                template_name: None,
                current_state: None,
                directive: None,
                execution_dir: None,
                anchor: None,
                store: None,
            },
            migrated: None,
            keys: names.iter().map(|n| key_entry(n)).collect(),
            unlisted_keys: 0,
        }
    }

    fn text_preview() -> KeyPreview {
        KeyPreview::Text {
            excerpt: sanitize("x"),
            truncated: false,
        }
    }

    #[test]
    fn session_tab_loads_snapshot_once_per_mtime_and_one_preview_per_tick() {
        let mut tab = SessionTabState::default();
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let snapshots = RefCell::new(0usize);
        let previews = RefCell::new(Vec::<String>::new());
        let run_tick = |tab: &mut SessionTabState, mtime: SystemTime| {
            tab.tick(
                "wf",
                mtime,
                |_| {
                    *snapshots.borrow_mut() += 1;
                    Ok(snapshot_of(&["a", "b", "c"]))
                },
                |_, k| {
                    previews.borrow_mut().push(k.to_string());
                    text_preview()
                },
            );
        };

        // First tick on the tab: the snapshot and exactly one preview.
        run_tick(&mut tab, t0);
        assert_eq!(*snapshots.borrow(), 1);
        assert_eq!(previews.borrow().as_slice(), ["a"]);
        assert_eq!(tab.preview_reads, 1);

        // Later ticks with the same mtime read nothing.
        for _ in 0..5 {
            run_tick(&mut tab, t0);
        }
        assert_eq!(*snapshots.borrow(), 1);
        assert_eq!(tab.preview_reads, 1);

        // Moving the cursor costs one read on the next tick, not before.
        tab.move_cursor(1);
        assert_eq!(tab.plan("wf", t0), SessionLoad::Preview("b".to_string()));
        run_tick(&mut tab, t0);
        run_tick(&mut tab, t0);
        assert_eq!(previews.borrow().as_slice(), ["a", "b"]);

        // A state-file change reloads the snapshot and re-reads the key.
        let t1 = t0 + Duration::from_secs(1);
        run_tick(&mut tab, t1);
        assert_eq!(*snapshots.borrow(), 2);
        assert_eq!(previews.borrow().as_slice(), ["a", "b", "b"]);
        assert_eq!(
            tab.cursor, 1,
            "cursor survives a reload of the same session"
        );
    }

    #[test]
    fn session_tab_skips_preview_for_invalid_names_and_resets_on_session_change() {
        let mut tab = SessionTabState::default();
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let mut snap = snapshot_of(&["a", "b"]);
        snap.keys[0].raw = None;
        tab.tick("wf", t0, |_| Ok(snap.clone()), |_, _| panic!("no read"));
        assert_eq!(tab.preview_reads, 0);

        tab.move_cursor(1);
        tab.tick("wf", t0, |_| unreachable!(), |_, _| text_preview());
        assert_eq!(tab.preview_reads, 1);

        tab.tick(
            "other",
            t0,
            |_| Ok(snapshot_of(&["z"])),
            |_, _| text_preview(),
        );
        assert_eq!(tab.cursor, 0);
        assert_eq!(tab.loaded_session(), Some("other"));
    }

    #[test]
    fn session_tab_keeps_an_error_as_sanitized_text_and_loads_no_preview() {
        let mut tab = SessionTabState::default();
        let t0 = SystemTime::UNIX_EPOCH;
        tab.tick(
            "wf",
            t0,
            |_| Err(SnapshotError::Corrupt("bad \u{1B}[2J".to_string())),
            |_, _| panic!("no read"),
        );
        match &tab.snapshot {
            Some(Err(reason)) => {
                assert!(!reason.as_str().contains('\u{1B}'), "{reason}");
                assert!(reason.as_str().contains("\\u{1B}[2J"), "{reason}");
            }
            other => panic!("expected a sanitized error, got {other:?}"),
        }
        assert_eq!(tab.plan("wf", t0), SessionLoad::Idle);
    }

    #[test]
    fn session_tab_cursor_keys_walk_keys_in_detail_and_sessions_in_list() {
        let mut state = DashboardAppState::new(500);
        state.active_tab = DashboardTab::Session;
        state.session_tab.snapshot = Some(Ok(snapshot_of(&["a", "b", "c"])));
        state.view_mode = ViewMode::Detail;

        state.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        state.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(state.session_tab.cursor, 2);
        state.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(state.session_tab.cursor, 2, "bounded at the last key");
        state.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(state.session_tab.cursor, 1);
        state.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        assert_eq!(state.session_tab.cursor, 0);
        assert_eq!(state.cursor_idx, 0);

        // In the list view the same key still moves the session cursor.
        state.view_mode = ViewMode::List;
        state.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(state.session_tab.cursor, 0);
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
    // I3: attention-band ordering + receded partition
    // -----------------------------------------------------------------------

    /// Build a tree with one session per attention band for ordering tests.
    /// Returns the state (roots populated, not pre-sorted).
    fn attention_band_state() -> DashboardAppState {
        let mut state = DashboardAppState::new(500);

        let make_blocked =
            |name: &str, state_str: Option<&str>, is_terminal: bool, is_blocked: bool| {
                let mut s = make_session(name, None, state_str, is_terminal);
                s.is_blocked = is_blocked;
                s
            };

        // Band 0 (needs-you): failed (terminal+failed) and blocked.
        state.tree.sessions.insert(
            "s-failed".to_string(),
            make_blocked("s-failed", Some("failed"), true, false),
        );
        state.tree.sessions.insert(
            "s-blocked".to_string(),
            make_blocked("s-blocked", Some("waiting"), false, true),
        );
        // Band 1 (active): fresh, non-terminal, has state.
        state.tree.sessions.insert(
            "s-active".to_string(),
            make_blocked("s-active", Some("gather"), false, false),
        );
        // Band 2 (fresh pending): non-terminal, no state, fresh.
        state.tree.sessions.insert(
            "s-pending".to_string(),
            make_blocked("s-pending", None, false, false),
        );
        // Band 3 (receded): done (terminal, not failed).
        state.tree.sessions.insert(
            "s-done".to_string(),
            make_blocked("s-done", Some("done"), true, false),
        );

        state.tree.roots = state.tree.sessions.keys().cloned().collect();
        state
    }

    #[test]
    fn visible_rows_needs_you_sorts_ahead_of_active_idle() {
        let state = attention_band_state();
        let rows = state.visible_rows();
        let positions: std::collections::HashMap<&str, usize> = rows
            .iter()
            .enumerate()
            .map(|(i, r)| (r.session_id.as_str(), i))
            .collect();

        // needs-you (failed, blocked) lead, then active, then fresh pending.
        assert!(positions["s-failed"] < positions["s-active"]);
        assert!(positions["s-blocked"] < positions["s-active"]);
        assert!(positions["s-active"] < positions["s-pending"]);
    }

    #[test]
    fn visible_rows_receded_excluded_by_default() {
        let state = attention_band_state();
        let rows = state.visible_rows();
        // s-done is receded and must not appear by default.
        assert!(
            !rows.iter().any(|r| r.session_id == "s-done"),
            "receded (done) session must be excluded from the default view"
        );
        // The four live sessions are shown.
        assert_eq!(rows.len(), 4);
    }

    #[test]
    fn visible_rows_reveal_toggle_includes_receded() {
        let mut state = attention_band_state();
        state.show_receded = true;
        let rows = state.visible_rows();
        assert!(
            rows.iter().any(|r| r.session_id == "s-done"),
            "revealing receded set must include the done session"
        );
        assert_eq!(rows.len(), 5);
    }

    #[test]
    fn a_key_toggles_show_receded() {
        let mut state = attention_band_state();
        assert!(!state.show_receded);
        state.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert!(state.show_receded);
        state.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert!(!state.show_receded);
    }

    #[test]
    fn visible_rows_ordering_is_deterministic() {
        let state = attention_band_state();
        let first: Vec<String> = state
            .visible_rows()
            .iter()
            .map(|r| r.session_id.clone())
            .collect();
        let second: Vec<String> = state
            .visible_rows()
            .iter()
            .map(|r| r.session_id.clone())
            .collect();
        assert_eq!(first, second, "ordering must be deterministic");
    }

    #[test]
    fn receded_summary_counts_done_and_abandoned() {
        let mut state = attention_band_state();
        // Add a stale (non-fresh) Pending: never advanced, idle past the active
        // window. This recedes into the abandoned bucket.
        let mut stale_pending = make_session("s-stale-pending", None, None, false);
        stale_pending.last_event_at = Some(SystemTime::now() - Duration::from_secs(60 * 60));
        state
            .tree
            .sessions
            .insert("s-stale-pending".to_string(), stale_pending);
        state.tree.roots.push("s-stale-pending".to_string());

        let summary = state.receded_summary();
        assert_eq!(summary.done, 1, "one done session");
        assert_eq!(
            summary.abandoned, 1,
            "one stale-pending (abandoned) session"
        );
        assert_eq!(summary.total(), 2);
    }

    #[test]
    fn live_summary_counts_bands() {
        let state = attention_band_state();
        let summary = state.live_summary();
        assert_eq!(summary.needs_you, 2, "failed + blocked");
        assert_eq!(summary.active, 1);
        assert_eq!(summary.idle, 1, "fresh pending counts as band-2 idle");
    }

    #[test]
    fn child_row_label_prefixes_parent() {
        let mut state = DashboardAppState::new(500);
        let mut parent = make_session("parent", None, Some("coordinate"), false);
        parent.header.template_name = Some("batch".to_string());
        let mut child = make_session("child", Some("parent"), Some("implement"), false);
        child.header.template_name = Some("work-on".to_string());
        state.tree.sessions.insert("parent".to_string(), parent);
        state.tree.sessions.insert("child".to_string(), child);
        state.tree.roots = vec!["parent".to_string()];
        state.expanded.insert("parent".to_string());

        let rows = state.visible_rows();
        let child_row = rows.iter().find(|r| r.session_id == "child").unwrap();
        assert!(
            child_row.display_name.contains('\u{25b8}'),
            "child label must include the parent-prefix separator; got {:?}",
            child_row.display_name
        );
        assert!(
            child_row.display_name.contains("work-on"),
            "child label must include its own leaf label"
        );
    }

    // -----------------------------------------------------------------------
    // membership (I13)
    // -----------------------------------------------------------------------

    /// A session with the given batch / leg memberships.
    fn member_session(
        name: &str,
        parent: Option<&str>,
        is_batch_task: bool,
        is_request_leg: bool,
    ) -> CachedSession {
        let mut session = make_session(name, parent, Some("working"), false);
        session.is_batch_task = is_batch_task;
        session.is_request_leg = is_request_leg;
        session
    }

    #[test]
    fn membership_combines_two_independent_flags() {
        assert_eq!(Membership::from_flags(false, false), Membership::None);
        assert_eq!(Membership::from_flags(true, false), Membership::Batch);
        assert_eq!(Membership::from_flags(false, true), Membership::Leg);
        assert_eq!(Membership::from_flags(true, true), Membership::Both);
    }

    #[test]
    fn each_row_carries_its_own_membership() {
        let mut state = DashboardAppState::new(500);
        for (name, batch, leg) in [
            ("plain", false, false),
            ("task", true, false),
            ("leg", false, true),
            ("dual", true, true),
        ] {
            state
                .tree
                .sessions
                .insert(name.to_string(), member_session(name, None, batch, leg));
        }
        state.tree.roots = vec![
            "plain".to_string(),
            "task".to_string(),
            "leg".to_string(),
            "dual".to_string(),
        ];

        let rows = state.visible_rows();
        let membership_of = |id: &str| {
            rows.iter()
                .find(|r| r.session_id == id)
                .unwrap_or_else(|| panic!("no row for {id}"))
                .membership
        };
        assert_eq!(membership_of("plain"), Membership::None);
        assert_eq!(membership_of("task"), Membership::Batch);
        assert_eq!(membership_of("leg"), Membership::Leg);
        assert_eq!(membership_of("dual"), Membership::Both);
    }

    #[test]
    fn a_dual_member_row_carries_both_and_a_single_child_count() {
        let mut state = DashboardAppState::new(500);
        state.tree.sessions.insert(
            "coord".to_string(),
            member_session("coord", None, true, true),
        );
        state.tree.sessions.insert(
            "child".to_string(),
            member_session("child", Some("coord"), false, false),
        );
        state.tree.roots = vec!["coord".to_string()];

        let rows = state.visible_rows();
        let coord = rows.iter().find(|r| r.session_id == "coord").unwrap();
        assert_eq!(coord.membership, Membership::Both);
        let counts = coord
            .task_counts
            .as_ref()
            .expect("a coordinator row carries child counts");
        assert_eq!(
            counts.total, 1,
            "the count is over child sessions and nothing else"
        );
        assert_eq!(counts.running, 1);
    }

    #[test]
    fn membership_does_not_reach_the_child_count() {
        // Batch membership on the children must not change the parent's
        // count: the count is parentage, not batch task list.
        let mut state = DashboardAppState::new(500);
        state.tree.sessions.insert(
            "coord".to_string(),
            member_session("coord", None, false, false),
        );
        state.tree.sessions.insert(
            "a".to_string(),
            member_session("a", Some("coord"), true, true),
        );
        state.tree.sessions.insert(
            "b".to_string(),
            member_session("b", Some("coord"), false, false),
        );
        state.tree.roots = vec!["coord".to_string()];

        let rows = state.visible_rows();
        let counts = rows[0].task_counts.as_ref().unwrap();
        assert_eq!(counts.total, 2);
        assert_eq!(counts.running, 2);
    }
}
