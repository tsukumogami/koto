//! Rendering layer for the `koto dashboard` command.
//!
//! Converts `DashboardAppState` into ratatui widget trees and draws them
//! to a `Frame`. Implements list view, detail pane, scrollbar, and cursor
//! highlighting.

use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Cell, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table,
    TableState,
};
use ratatui::Frame;

use crate::cli::dashboard_data::{DetailData, SessionTree};
use crate::cli::dashboard_state::{DashboardAppState, TaskCounts, ViewMode};

/// Maximum number of evidence entries rendered in the detail pane.
const EVIDENCE_DISPLAY_CAP: usize = 3;

/// Draw the full dashboard to `f`.
///
/// Switches between a full-height list (`ViewMode::List`) and a vertically-split
/// layout with a detail pane at the bottom (`ViewMode::Detail`).
pub fn render_frame(f: &mut Frame<'_>, state: &DashboardAppState) {
    match state.view_mode {
        ViewMode::List => {
            let area = f.area();
            render_list(f, state, area);
        }
        ViewMode::Detail => {
            let chunks =
                Layout::vertical([Constraint::Min(0), Constraint::Length(8)]).split(f.area());
            render_list(f, state, chunks[0]);
            render_detail(
                f,
                state.detail_cache.as_ref(),
                state.focused_id.as_deref(),
                &state.tree,
                chunks[1],
            );
        }
    }
}

/// Render the session list as a 4-column table with cursor highlighting and scrollbar.
fn render_list(f: &mut Frame<'_>, state: &DashboardAppState, area: ratatui::layout::Rect) {
    let rows_data = state.visible_rows();
    let row_count = rows_data.len();

    // Reserve 1 column for border and 1 for scrollbar on the right (2 total overhead).
    // Column widths: State=12, Elapsed=9, Tasks=10, plus separators.
    // Name fills the remaining space via Constraint::Min(0).
    let widths: Vec<Constraint> = vec![
        Constraint::Min(0),
        Constraint::Length(12),
        Constraint::Length(9),
        Constraint::Length(10),
    ];

    let rows: Vec<Row> = rows_data
        .iter()
        .map(|row| {
            let name_cell = format!("{}{}", " ".repeat(row.indent_depth * 2), row.display_name);

            let state_cell = row.state.as_deref().unwrap_or("-").to_string();
            let elapsed_cell = format_duration(row.elapsed);
            let tasks_cell = format_task_counts(row.task_counts.as_ref());

            Row::new(vec![
                Cell::from(name_cell),
                Cell::from(state_cell),
                Cell::from(elapsed_cell),
                Cell::from(tasks_cell),
            ])
        })
        .collect();

    let header = Row::new(vec![
        Cell::from("Name").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("State").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Elapsed").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Tasks").style(Style::default().add_modifier(Modifier::BOLD)),
    ]);

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("koto dashboard"),
        )
        .row_highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

    let mut table_state = TableState::default();
    table_state.select(Some(state.cursor_idx));

    f.render_stateful_widget(table, area, &mut table_state);

    // Attach a scrollbar when the row count exceeds the visible area height.
    // Visible area height minus 3 (border top + header + border bottom).
    let visible_height = area.height.saturating_sub(3) as usize;
    if row_count > visible_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
        let mut scrollbar_state = ScrollbarState::new(row_count).position(state.cursor_idx);
        f.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
    }
}

/// Render the detail pane for the focused session.
///
/// Shows a contextual message when `detail` is `None`:
/// - "No state history" when the session has never transitioned (unknown status)
/// - "No gate evaluations recorded" when the session has a current state but no gates
///
/// When detail data is present, displays gate name, command, result, elapsed,
/// and evidence entries (newest-first, capped at 3).
fn render_detail(
    f: &mut Frame<'_>,
    detail: Option<&DetailData>,
    session_name: Option<&str>,
    tree: &SessionTree,
    area: ratatui::layout::Rect,
) {
    let title = match session_name {
        Some(name) => format!(" {}: detail ", name),
        None => " detail ".to_string(),
    };
    let block = Block::default().borders(Borders::ALL).title(title);

    match detail {
        None => {
            let msg = session_name
                .and_then(|name| tree.sessions.get(name))
                .map(|s| {
                    if s.current_state.is_some() {
                        "No gate evaluations recorded"
                    } else {
                        "No state history"
                    }
                })
                .unwrap_or("No data");
            let paragraph = Paragraph::new(msg).block(block);
            f.render_widget(paragraph, area);
        }
        Some(data) => {
            let mut lines: Vec<Line> = Vec::new();

            // Gate name and result on the first line.
            lines.push(Line::from(vec![
                Span::raw(format!("Gate: {} | ", data.gate_name)),
                Span::styled(
                    data.result.clone(),
                    if data.result == "PASS" {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(Color::Red)
                    },
                ),
                Span::raw(format!(" | elapsed: {}", format_duration(data.elapsed))),
            ]));

            // Command line (if present).
            if let Some(cmd) = &data.command {
                lines.push(Line::from(format!("Command: {}", cmd)));
            }

            // Evidence entries: newest-first (already ordered), cap at EVIDENCE_DISPLAY_CAP.
            let total_evidence = data.evidence.len();
            if total_evidence > 0 {
                lines.push(Line::from("Evidence:"));
                for entry in data.evidence.iter().take(EVIDENCE_DISPLAY_CAP) {
                    let fields_str = serde_json::to_string(&entry.fields)
                        .expect("serde_json::Value serialization is infallible");
                    lines.push(Line::from(format!("  [{}] {}", entry.state, fields_str)));
                }
                if total_evidence > EVIDENCE_DISPLAY_CAP {
                    let more = total_evidence - EVIDENCE_DISPLAY_CAP;
                    lines.push(Line::from(format!("  \u{2193} {} more", more)));
                }
            }

            let text = Text::from(lines);
            let paragraph = Paragraph::new(text).block(block);
            f.render_widget(paragraph, area);
        }
    }
}

/// Format a `Duration` as a compact human-readable string.
///
/// - Zero or sub-second: `"0s"`
/// - Under a minute: `"{s}s"`
/// - Under an hour: `"{m}m{s}s"`
/// - An hour or more: `"{h}h{m}m"`
fn format_duration(d: std::time::Duration) -> String {
    let total_secs = d.as_secs();
    if total_secs == 0 {
        return "0s".to_string();
    }
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    if hours > 0 {
        format!("{}h{}m", hours, minutes)
    } else if minutes > 0 {
        format!("{}m{}s", minutes, secs)
    } else {
        format!("{}s", secs)
    }
}

/// Format `TaskCounts` into a compact string, or return an empty string for leaf sessions.
fn format_task_counts(counts: Option<&TaskCounts>) -> String {
    match counts {
        None => String::new(),
        Some(c) => format!("{}/{} done", c.done, c.total),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::dashboard_data::EvidenceEntry;
    use crate::cli::dashboard_state::DashboardAppState;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer as RatatuiBuffer;
    use ratatui::Terminal;
    use std::time::Duration;

    /// Collect all cell symbols from a rectangular region of the buffer as a single String.
    fn region_to_string(
        buffer: &RatatuiBuffer,
        rows: std::ops::Range<u16>,
        cols: std::ops::Range<u16>,
    ) -> String {
        rows.flat_map(|y| {
            cols.clone()
                .map(move |x| buffer.cell((x, y)).unwrap().symbol().to_string())
        })
        .collect()
    }

    fn empty_state() -> DashboardAppState {
        DashboardAppState::new(500)
    }

    // -----------------------------------------------------------------------
    // render_frame: TestBackend smoke test (scenario-10)
    // -----------------------------------------------------------------------

    #[test]
    fn render_frame_list_mode_renders_header_cells() {
        let state = empty_state();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        // The block title "koto dashboard" starts near the top-left border.
        // cell (1, 0) is the left border character '|'; (2,0) should be part of the title.
        // We check that the title text is present somewhere in the top row.
        let top_row: String = (0..80)
            .map(|x| buffer.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(
            top_row.contains("koto dashboard"),
            "top row should contain block title; got: {:?}",
            top_row
        );
    }

    #[test]
    fn render_frame_list_mode_header_row_present() {
        let state = empty_state();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        // Row 1 (0-indexed) is the table header inside the border.
        let row1: String = (0..80)
            .map(|x| buffer.cell((x, 1)).unwrap().symbol().to_string())
            .collect();
        assert!(
            row1.contains("Name"),
            "header row should contain 'Name'; got: {:?}",
            row1
        );
        assert!(
            row1.contains("State"),
            "header row should contain 'State'; got: {:?}",
            row1
        );
    }

    // -----------------------------------------------------------------------
    // render_detail: no-data messages (scenario-11)
    // -----------------------------------------------------------------------

    #[test]
    fn render_detail_shows_no_data_when_cache_is_none_and_no_focused_session() {
        let state = empty_state();
        let mut state = state;
        state.view_mode = ViewMode::Detail;
        state.detail_cache = None;

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        let detail_rows = region_to_string(&buffer, 16..24, 0..80);
        assert!(
            detail_rows.contains("No data"),
            "detail pane should contain 'No data'; got rows 16-23: {:?}",
            detail_rows
        );
    }

    #[test]
    fn render_detail_shows_no_state_history_for_unknown_session() {
        use crate::cli::dashboard_data::CachedSession;
        use crate::engine::types::StateFileHeader;
        use std::path::PathBuf;
        use std::time::SystemTime;

        let mut state = empty_state();
        state.view_mode = ViewMode::Detail;
        state.focused_id = Some("my-wf".to_string());
        state.detail_cache = None;
        state.tree.sessions.insert(
            "my-wf".to_string(),
            CachedSession {
                header: StateFileHeader {
                    schema_version: 1,
                    workflow: "my-wf".to_string(),
                    template_hash: "abc".to_string(),
                    created_at: "2026-01-01T00:00:00Z".to_string(),
                    parent_workflow: None,
                    template_source_dir: None,
                    session_id: String::new(),
                },
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                mtime: SystemTime::UNIX_EPOCH,
                state_path: PathBuf::new(),
            },
        );

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        let detail_rows = region_to_string(&buffer, 16..24, 0..80);
        assert!(
            detail_rows.contains("No state history"),
            "unknown session should show 'No state history'; got: {:?}",
            detail_rows
        );
    }

    #[test]
    fn render_detail_shows_gate_and_result_when_cache_is_present() {
        let mut state = empty_state();
        state.view_mode = ViewMode::Detail;
        state.detail_cache = Some(DetailData {
            session_id: "test-session".to_string(),
            gate_name: "my-gate".to_string(),
            command: None,
            result: "PASS".to_string(),
            elapsed: Duration::from_secs(0),
            evidence: vec![],
        });

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        // Detail pane is the bottom 8 rows.
        let detail_rows = region_to_string(&buffer, 16..24, 0..80);
        assert!(
            detail_rows.contains("my-gate"),
            "detail pane should contain gate name; got: {:?}",
            detail_rows
        );
        assert!(
            detail_rows.contains("PASS"),
            "detail pane should contain result; got: {:?}",
            detail_rows
        );
    }

    #[test]
    fn render_detail_shows_command_when_present() {
        let mut state = empty_state();
        state.view_mode = ViewMode::Detail;
        state.detail_cache = Some(DetailData {
            session_id: "test-session".to_string(),
            gate_name: "build-gate".to_string(),
            command: Some("cargo build".to_string()),
            result: "FAIL".to_string(),
            elapsed: Duration::from_secs(30),
            evidence: vec![],
        });

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        let detail_rows = region_to_string(&buffer, 16..24, 0..80);
        assert!(
            detail_rows.contains("cargo build"),
            "detail pane should contain command; got: {:?}",
            detail_rows
        );
    }

    #[test]
    fn render_detail_shows_evidence_entries_capped_at_3() {
        let mut state = empty_state();
        state.view_mode = ViewMode::Detail;
        state.detail_cache = Some(DetailData {
            session_id: "test-session".to_string(),
            gate_name: "evidence-gate".to_string(),
            command: None,
            result: "PASS".to_string(),
            elapsed: Duration::from_secs(0),
            evidence: vec![
                EvidenceEntry {
                    state: "s1".to_string(),
                    fields: serde_json::json!({"k": "v1"}),
                },
                EvidenceEntry {
                    state: "s2".to_string(),
                    fields: serde_json::json!({"k": "v2"}),
                },
                EvidenceEntry {
                    state: "s3".to_string(),
                    fields: serde_json::json!({"k": "v3"}),
                },
                EvidenceEntry {
                    state: "s4".to_string(),
                    fields: serde_json::json!({"k": "v4"}),
                },
            ],
        });

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        let detail_rows = region_to_string(&buffer, 16..24, 0..80);
        assert!(
            detail_rows.contains("1 more"),
            "detail pane should show '1 more' indicator for 4 entries; got: {:?}",
            detail_rows
        );
    }

    // -----------------------------------------------------------------------
    // format_duration helper
    // -----------------------------------------------------------------------

    #[test]
    fn format_duration_zero() {
        assert_eq!(format_duration(Duration::from_secs(0)), "0s");
    }

    #[test]
    fn format_duration_seconds_only() {
        assert_eq!(format_duration(Duration::from_secs(45)), "45s");
    }

    #[test]
    fn format_duration_minutes_and_seconds() {
        assert_eq!(format_duration(Duration::from_secs(125)), "2m5s");
    }

    #[test]
    fn format_duration_hours_and_minutes() {
        assert_eq!(format_duration(Duration::from_secs(3665)), "1h1m");
    }

    // -----------------------------------------------------------------------
    // format_task_counts helper
    // -----------------------------------------------------------------------

    #[test]
    fn format_task_counts_none_is_empty() {
        assert_eq!(format_task_counts(None), "");
    }

    #[test]
    fn format_task_counts_some_shows_done_of_total() {
        let counts = TaskCounts {
            total: 5,
            running: 1,
            done: 3,
            failed: 1,
        };
        let result = format_task_counts(Some(&counts));
        assert!(
            result.contains("3") && result.contains("5"),
            "got: {}",
            result
        );
    }

    // -----------------------------------------------------------------------
    // render_frame: full-height list in List mode (no split)
    // -----------------------------------------------------------------------

    #[test]
    fn render_frame_list_mode_uses_full_height() {
        let state = empty_state();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        // In List mode the table should fill the full 24 rows.
        // The bottom border should appear on row 23.
        let bottom_row: String = (0..80)
            .map(|x| buffer.cell((x, 23)).unwrap().symbol().to_string())
            .collect();
        // Bottom row contains the border character.
        assert!(
            bottom_row.contains('\u{2500}') || bottom_row.contains('-') || bottom_row.contains('+'),
            "bottom border should be present in list mode; got: {:?}",
            bottom_row
        );
    }

    // -----------------------------------------------------------------------
    // Render layer does not touch persistence (structural test)
    // -----------------------------------------------------------------------

    #[test]
    fn render_frame_with_empty_tree_does_not_panic() {
        // Validates that render layer handles empty state gracefully.
        let state = DashboardAppState::new(500);
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();
        // No panic = pass.
    }

    // -----------------------------------------------------------------------
    // Render with session tree data
    // -----------------------------------------------------------------------

    #[test]
    fn render_frame_with_sessions_shows_session_name() {
        use crate::cli::dashboard_data::CachedSession;
        use crate::engine::types::StateFileHeader;
        use std::path::PathBuf;
        use std::time::SystemTime;

        let mut state = DashboardAppState::new(500);
        state.tree.sessions.insert(
            "my-workflow".to_string(),
            CachedSession {
                header: StateFileHeader {
                    schema_version: 1,
                    workflow: "my-workflow".to_string(),
                    template_hash: "abc".to_string(),
                    created_at: "2026-01-01T00:00:00Z".to_string(),
                    parent_workflow: None,
                    template_source_dir: None,
                    session_id: String::new(),
                },
                current_state: Some("running".to_string()),
                is_terminal: false,
                is_blocked: false,
                mtime: SystemTime::UNIX_EPOCH,
                state_path: PathBuf::new(),
            },
        );
        state.tree.roots = vec!["my-workflow".to_string()];

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        let all_content = region_to_string(&buffer, 0..24, 0..80);
        assert!(
            all_content.contains("my-workflow"),
            "rendered output should contain session name; got content (truncated): {}",
            &all_content[..all_content.len().min(200)]
        );
    }
}
