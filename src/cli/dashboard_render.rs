//! Rendering layer for the `koto dashboard` command.
//!
//! Converts `DashboardAppState` into ratatui widget trees and draws them
//! to a `Frame`. Full implementation (table layout, scrollbar, detail pane)
//! arrives in Issue 4.

use ratatui::Frame;

use crate::cli::dashboard_state::DashboardAppState;

/// Draw the full dashboard to `f`.
///
/// Switches between a full-height list (for `ViewMode::List`) and a
/// vertically-split layout with a detail pane (for `ViewMode::Detail`).
/// Full implementation arrives in Issue 4.
pub fn render_frame(f: &mut Frame<'_>, _state: &DashboardAppState) {
    use ratatui::widgets::{Block, Borders};
    let area = f.area();
    f.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title("koto dashboard"),
        area,
    );
}
