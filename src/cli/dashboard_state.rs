//! Application state layer for the `koto dashboard` command.
//!
//! Owns the session tree, cursor position, view mode, and poll timing.
//! Full implementation (cursor movement, key dispatch, `visible_rows()`)
//! arrives in Issue 3.

use std::collections::HashSet;

use crate::cli::dashboard_data::{DetailData, SessionTree};

/// Which pane the user is currently viewing.
pub enum ViewMode {
    /// The session list (default view).
    List,
    /// The gate-detail pane for the focused session.
    Detail,
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
}
