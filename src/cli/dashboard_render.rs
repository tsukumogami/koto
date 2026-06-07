//! Rendering layer for the `koto dashboard` command.
//!
//! Converts `DashboardAppState` into ratatui widget trees and draws them
//! to a `Frame`. Implements list view, detail pane, scrollbar, and cursor
//! highlighting.
//!
//! Layout is driven by terminal width:
//! - width < 40: "terminal too narrow" message
//! - width < 80: list-only (full width)
//! - width >= 80: horizontal split — 40% list / 60% detail

use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Cell, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table,
    TableState, Tabs,
};
use ratatui::Frame;

use crate::cli::dashboard_data::{DetailData, Liveness, SessionTree};
use crate::cli::dashboard_state::{DashboardAppState, DashboardTab, TaskCounts};

/// Maximum number of evidence entries rendered in the detail pane.
const EVIDENCE_DISPLAY_CAP: usize = 3;

/// Glyph prefix for a liveness band, shown ahead of the idle time.
fn liveness_glyph(liveness: Liveness) -> &'static str {
    match liveness {
        Liveness::NeedsYouBlocked => "\u{25cf}", // ● needs you (blocked)
        Liveness::NeedsYouFailed => "\u{2717}",  // ✗ failed
        Liveness::NeedsYouStalled => "\u{25cb}", // ○ stalled
        Liveness::Active => "\u{25b6}",          // ▶ active
        Liveness::Idle => "\u{00b7}",            // · idle
        Liveness::Pending => "\u{2026}",         // … starting
        Liveness::Done => "\u{2713}",            // ✓ done
    }
}

/// Display style for a liveness band: needs-you bright, active bright, idle
/// normal, stalled/done dim.
fn liveness_style(liveness: Liveness) -> Style {
    match liveness {
        Liveness::NeedsYouBlocked | Liveness::NeedsYouFailed => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        Liveness::Active => Style::default().fg(Color::Green),
        Liveness::Idle => Style::default(),
        Liveness::NeedsYouStalled | Liveness::Pending | Liveness::Done => {
            Style::default().add_modifier(Modifier::DIM)
        }
    }
}

/// Draw the full dashboard to `f`.
///
/// Layout is width-driven:
/// - width < 40: shows a "terminal too narrow" message
/// - width < 80: list-only, using the full area
/// - width >= 80: horizontal 40%/60% split — list on the left, detail on the right
pub fn render_frame(f: &mut Frame<'_>, state: &DashboardAppState) {
    let area = f.area();
    let width = area.width;

    if width < 40 {
        let msg =
            Paragraph::new("terminal too narrow").block(Block::default().borders(Borders::ALL));
        f.render_widget(msg, area);
        return;
    }

    if width < 80 {
        render_list(f, state, area);
        return;
    }

    // Width >= 80: horizontal 40% list / 60% detail.
    let chunks =
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).split(area);

    render_list(f, state, chunks[0]);
    render_detail(
        f,
        state.detail_cache.as_ref(),
        state.focused_id.as_deref(),
        &state.tree,
        &state.active_tab,
        chunks[1],
    );
}

/// Render the session list as a 4-column table with cursor highlighting and scrollbar.
fn render_list(f: &mut Frame<'_>, state: &DashboardAppState, area: ratatui::layout::Rect) {
    let rows_data = state.visible_rows();

    // Reserve 1 column for border and 1 for scrollbar on the right (2 total overhead).
    // Column widths: State=12, Idle=9, Tasks=10, plus separators.
    // Name fills the remaining space via Constraint::Min(0).
    let widths: Vec<Constraint> = vec![
        Constraint::Min(0),
        Constraint::Length(12),
        Constraint::Length(9),
        Constraint::Length(10),
    ];

    let mut rows: Vec<Row> = Vec::new();

    // R6 zero-state: when nothing needs the user, lead with an explicit
    // all-clear row instead of letting absence be inferred. This synthetic row
    // shifts the selectable session rows down by one; `cursor_offset` keeps the
    // cursor (which indexes into `visible_rows`) pointed at the right session.
    let live = state.live_summary();
    let mut cursor_offset = 0usize;
    if live.needs_you == 0 {
        let msg = format!(
            "Nothing needs you \u{2014} {} active, {} idle",
            live.active, live.idle
        );
        rows.push(Row::new(vec![Cell::from(msg)]).style(Style::default().fg(Color::Green)));
        cursor_offset = 1;
    }

    for row in &rows_data {
        let name_cell = format!("{}{}", row.connector, row.display_name);

        let state_cell = row.state.as_deref().unwrap_or("-").to_string();
        let elapsed_cell = format!(
            "{} {}",
            liveness_glyph(row.liveness),
            format_duration(row.elapsed)
        );
        let tasks_cell = format_task_counts(row.task_counts.as_ref());

        rows.push(
            Row::new(vec![
                Cell::from(name_cell),
                Cell::from(state_cell),
                Cell::from(elapsed_cell),
                Cell::from(tasks_cell),
            ])
            .style(liveness_style(row.liveness)),
        );
    }

    // Trailing collapsed summary for the receded set (hidden by default).
    let receded = state.receded_summary();
    if receded.total() > 0 && !state.show_receded {
        let summary = format!(
            "\u{2713} {} done \u{00b7} {} abandoned \u{2014} press a / --all",
            receded.done, receded.abandoned
        );
        rows.push(
            Row::new(vec![Cell::from(summary)]).style(Style::default().add_modifier(Modifier::DIM)),
        );
    }

    // Trailing note for sessions whose state file failed to parse, so they are
    // surfaced rather than silently dropped.
    if state.tree.unreadable > 0 {
        let note = format!("\u{26a0} {} unreadable session(s)", state.tree.unreadable);
        rows.push(Row::new(vec![Cell::from(note)]).style(Style::default().fg(Color::Red)));
    }

    let row_count = rows.len();

    let header = Row::new(vec![
        Cell::from("Name").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("State").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Idle").style(Style::default().add_modifier(Modifier::BOLD)),
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
    table_state.select(Some(state.cursor_idx + cursor_offset));

    f.render_stateful_widget(table, area, &mut table_state);

    // Attach a scrollbar when the row count exceeds the visible area height.
    // Visible area height minus 3 (border top + header + border bottom).
    let visible_height = area.height.saturating_sub(3) as usize;
    if row_count > visible_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
        let mut scrollbar_state =
            ScrollbarState::new(row_count).position(state.cursor_idx + cursor_offset);
        f.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
    }
}

/// Render the detail pane for the focused session with tabbed navigation.
fn render_detail(
    f: &mut Frame<'_>,
    detail: Option<&DetailData>,
    session_name: Option<&str>,
    tree: &SessionTree,
    active_tab: &DashboardTab,
    area: ratatui::layout::Rect,
) {
    let title = match session_name {
        Some(name) => format!(" {} ", name),
        None => " session ".to_string(),
    };

    let outer_block = Block::default().borders(Borders::ALL).title(title);
    let inner_area = outer_block.inner(area);
    f.render_widget(outer_block, area);

    // Split inner_area: tabs row at top (height 1), content below.
    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(inner_area);

    // Tabs widget.
    let tab_titles = vec!["Summary", "History", "Remaining"];
    let selected = match active_tab {
        DashboardTab::Summary => 0,
        DashboardTab::History => 1,
        DashboardTab::Remaining => 2,
    };
    let tabs = Tabs::new(tab_titles)
        .select(selected)
        .style(Style::default())
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Yellow),
        );
    f.render_widget(tabs, chunks[0]);

    // Tab content.
    match active_tab {
        DashboardTab::Summary => render_summary_tab(f, detail, session_name, tree, chunks[1]),
        DashboardTab::History => render_history_tab(f, detail, chunks[1]),
        DashboardTab::Remaining => render_remaining_tab(f, detail, chunks[1]),
    }
}

/// Render the Summary tab content.
fn render_summary_tab(
    f: &mut Frame<'_>,
    detail: Option<&DetailData>,
    session_name: Option<&str>,
    tree: &SessionTree,
    area: ratatui::layout::Rect,
) {
    match detail {
        None => {
            let msg = session_name
                .and_then(|name| tree.sessions.get(name))
                .map(|s| {
                    if s.current_state.is_some() {
                        "No data"
                    } else {
                        "No state history"
                    }
                })
                .unwrap_or("No data");
            f.render_widget(Paragraph::new(msg), area);
        }
        Some(data) => {
            let mut lines: Vec<Line> = Vec::new();

            if let Some(ref cs) = data.current_state {
                lines.push(Line::from(format!("State: {}", cs)));
            }
            if let Some(ref d) = data.directive {
                lines.push(Line::from(format!("Directive: {}", d)));
            }
            if let (Some(ref gn), Some(ref result)) = (&data.gate_name, &data.result) {
                lines.push(Line::from(vec![
                    Span::raw(format!("Gate: {} | ", gn)),
                    Span::styled(
                        result.clone(),
                        if result == "PASS" {
                            Style::default().fg(Color::Green)
                        } else {
                            Style::default().fg(Color::Red)
                        },
                    ),
                ]));
            }
            if let Some(ref intent) = data.intent {
                lines.push(Line::from(format!("Intent: {}", intent)));
            }
            if let Some(ref tn) = data.template_name {
                lines.push(Line::from(format!("Template: {}", tn)));
            }

            // Evidence (newest-first, capped at EVIDENCE_DISPLAY_CAP).
            let total_evidence = data.evidence.len();
            if total_evidence > 0 {
                lines.push(Line::from("Evidence:"));
                for entry in data.evidence.iter().take(EVIDENCE_DISPLAY_CAP) {
                    let fields_str = serde_json::to_string(&entry.fields).unwrap_or_default();
                    lines.push(Line::from(format!("  [{}] {}", entry.state, fields_str)));
                }
                if total_evidence > EVIDENCE_DISPLAY_CAP {
                    lines.push(Line::from(format!(
                        "  \u{2193} {} more",
                        total_evidence - EVIDENCE_DISPLAY_CAP
                    )));
                }
            }

            f.render_widget(Paragraph::new(Text::from(lines)), area);
        }
    }
}

/// Render the History tab content.
fn render_history_tab(f: &mut Frame<'_>, detail: Option<&DetailData>, area: ratatui::layout::Rect) {
    let lines: Vec<Line> = match detail {
        None => vec![Line::from("No data")],
        Some(data) if data.history.is_empty() => vec![Line::from("No events in current epoch")],
        Some(data) => {
            let mut lines = Vec::new();
            for entry in &data.history {
                let ts_end = entry.timestamp.len().min(19);
                lines.push(Line::from(format!(
                    "[{}] {}",
                    &entry.timestamp[..ts_end],
                    entry.summary
                )));
                if let Some(ref cond) = entry.gate_condition {
                    lines.push(Line::from(format!("  {}", cond)));
                }
            }
            lines
        }
    };
    f.render_widget(Paragraph::new(Text::from(lines)), area);
}

/// Render the Remaining tab content.
fn render_remaining_tab(
    f: &mut Frame<'_>,
    detail: Option<&DetailData>,
    area: ratatui::layout::Rect,
) {
    let lines: Vec<Line> = match detail {
        None => vec![Line::from("No data")],
        Some(data) if data.remaining.is_empty() => vec![Line::from("No remaining states")],
        Some(data) => data
            .remaining
            .iter()
            .map(|s| Line::from(s.clone()))
            .collect(),
    };
    f.render_widget(Paragraph::new(Text::from(lines)), area);
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
        // Use width=100 so the horizontal split gives the list pane ~40 columns,
        // enough to show all four header columns (Name, State, Elapsed, Tasks).
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        // Row 1 (0-indexed) is the table header inside the border.
        // At width=100 the left pane is columns 0-39.
        let row1 = region_to_string(&buffer, 1..2, 0..40);
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
        let mut state = empty_state();
        state.detail_cache = None;

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        // At width=80 with 40/60 split, the detail pane occupies the right 60%
        // (columns 32-79, all rows).
        let detail_cols = region_to_string(&buffer, 0..24, 32..80);
        assert!(
            detail_cols.contains("No data"),
            "detail pane should contain 'No data'; got right pane: {:?}",
            detail_cols
        );
    }

    #[test]
    fn render_detail_shows_no_state_history_for_unknown_session() {
        use crate::cli::dashboard_data::CachedSession;
        use crate::engine::types::StateFileHeader;
        use std::path::PathBuf;
        use std::time::SystemTime;

        let mut state = empty_state();
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
                    respawn_generation: None,
                },
                current_state: None,
                is_terminal: false,
                is_blocked: false,
                intent: None,
                mtime: SystemTime::UNIX_EPOCH,
                state_path: PathBuf::new(),
                last_event_at: None,
                salient_var: None,
                is_unreadable: false,
            },
        );

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        // At width=80 with 40/60 split, the detail pane occupies columns 32-79.
        let detail_cols = region_to_string(&buffer, 0..24, 32..80);
        assert!(
            detail_cols.contains("No state history"),
            "unknown session should show 'No state history'; got: {:?}",
            detail_cols
        );
    }

    #[test]
    fn render_detail_shows_gate_and_result_when_cache_is_present() {
        let mut state = empty_state();
        state.detail_cache = Some(DetailData {
            session_id: "test-session".to_string(),
            gate_name: Some("my-gate".to_string()),
            command: None,
            result: Some("PASS".to_string()),
            elapsed: Duration::from_secs(0),
            evidence: vec![],
            current_state: None,
            directive: None,
            intent: None,
            template_name: None,
            history: vec![],
            remaining: vec![],
        });

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        // At width=80 with 40/60 split, the detail pane occupies columns 32-79.
        let detail_cols = region_to_string(&buffer, 0..24, 32..80);
        assert!(
            detail_cols.contains("my-gate"),
            "detail pane should contain gate name; got: {:?}",
            detail_cols
        );
        assert!(
            detail_cols.contains("PASS"),
            "detail pane should contain result; got: {:?}",
            detail_cols
        );
    }

    #[test]
    fn render_detail_shows_gate_and_fail_result_in_summary_tab() {
        let mut state = empty_state();
        state.detail_cache = Some(DetailData {
            session_id: "test-session".to_string(),
            gate_name: Some("build-gate".to_string()),
            command: Some("cargo build".to_string()),
            result: Some("FAIL".to_string()),
            elapsed: Duration::from_secs(30),
            evidence: vec![],
            current_state: None,
            directive: None,
            intent: None,
            template_name: None,
            history: vec![],
            remaining: vec![],
        });

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        // At width=80 with 40/60 split, the detail pane occupies columns 32-79.
        let detail_cols = region_to_string(&buffer, 0..24, 32..80);
        assert!(
            detail_cols.contains("build-gate"),
            "detail pane should contain gate name; got: {:?}",
            detail_cols
        );
        assert!(
            detail_cols.contains("FAIL"),
            "detail pane should contain FAIL result; got: {:?}",
            detail_cols
        );
    }

    #[test]
    fn render_detail_shows_evidence_entries_capped_at_3() {
        let mut state = empty_state();
        state.detail_cache = Some(DetailData {
            session_id: "test-session".to_string(),
            gate_name: Some("evidence-gate".to_string()),
            command: None,
            result: Some("PASS".to_string()),
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
            current_state: None,
            directive: None,
            intent: None,
            template_name: None,
            history: vec![],
            remaining: vec![],
        });

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        // At width=80 with 40/60 split, the detail pane occupies columns 32-79.
        let detail_cols = region_to_string(&buffer, 0..24, 32..80);
        assert!(
            detail_cols.contains("1 more"),
            "detail pane should show '1 more' indicator for 4 entries; got: {:?}",
            detail_cols
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
            blocked: 0,
            done_blocked: 0,
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
                    respawn_generation: None,
                },
                current_state: Some("running".to_string()),
                is_terminal: false,
                is_blocked: false,
                intent: None,
                // A live session (recent last activity) so it stays in the
                // default (non-receded) view -- an ancient timestamp would
                // recede past the abandoned threshold and hide the name.
                mtime: SystemTime::now(),
                state_path: PathBuf::new(),
                last_event_at: Some(SystemTime::now()),
                salient_var: None,
                is_unreadable: false,
            },
        );
        state.tree.roots = vec!["my-workflow".to_string()];

        // Use width=100 so the list pane (left 40% = 40 cols) has enough room
        // for the Name column to render the session name.
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        // Check the left pane (columns 0-39) for the session name.
        // The table may truncate the name to fit — check for a known prefix.
        let list_pane = region_to_string(&buffer, 0..24, 0..40);
        assert!(
            list_pane.contains("my-workflow") || list_pane.contains("my-w"),
            "rendered output should contain session name (or prefix); got content (truncated): {}",
            &list_pane[..list_pane.len().min(200)]
        );
    }

    // -----------------------------------------------------------------------
    // Responsive layout tests
    // -----------------------------------------------------------------------

    #[test]
    fn render_frame_width_100_shows_horizontal_split() {
        let state = empty_state();
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        // Left side (list pane) — columns 0-39, all rows.
        let left_side = region_to_string(&buffer, 0..24, 0..40);
        assert!(
            left_side.contains("koto dashboard") || left_side.contains("Name"),
            "left side should contain list content; got: {:?}",
            &left_side[..left_side.len().min(200)]
        );
        // Right side (detail pane) — columns 40-99, all rows.
        let right_side = region_to_string(&buffer, 0..24, 40..100);
        assert!(
            right_side.contains('\u{2500}')
                || right_side.contains('─')
                || right_side.contains('|')
                || right_side.contains('│')
                || right_side.contains("detail"),
            "right side should contain detail pane border; got: {:?}",
            &right_side[..right_side.len().min(200)]
        );
    }

    #[test]
    fn render_frame_width_60_shows_list_only() {
        let state = empty_state();
        let backend = TestBackend::new(60, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        // The list should span the full width — check the top row for the block title.
        let top_row = region_to_string(&buffer, 0..1, 0..60);
        assert!(
            top_row.contains("koto dashboard"),
            "width=60 should show list-only with dashboard title; got: {:?}",
            top_row
        );
        // Should NOT have a "detail" pane title anywhere.
        let all = region_to_string(&buffer, 0..24, 0..60);
        assert!(
            !all.contains("detail"),
            "width=60 should not show detail pane; got: {:?}",
            &all[..all.len().min(200)]
        );
    }

    #[test]
    fn render_frame_width_30_shows_too_narrow_message() {
        let state = empty_state();
        let backend = TestBackend::new(30, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        let all = region_to_string(&buffer, 0..24, 0..30);
        assert!(
            all.contains("terminal too narrow") || all.contains("too narrow"),
            "width=30 should show too-narrow message; got: {:?}",
            &all[..all.len().min(200)]
        );
        // Should NOT contain "koto dashboard" (list not rendered).
        assert!(
            !all.contains("koto dashboard"),
            "width=30 should not render list; got: {:?}",
            &all[..all.len().min(200)]
        );
    }

    // -----------------------------------------------------------------------
    // Tabbed detail pane tests
    // -----------------------------------------------------------------------

    #[test]
    fn render_detail_summary_tab_shows_intent() {
        use crate::cli::dashboard_state::DashboardTab;

        let mut state = empty_state();
        state.active_tab = DashboardTab::Summary;
        state.detail_cache = Some(DetailData {
            session_id: "sess".to_string(),
            gate_name: None,
            command: None,
            result: None,
            elapsed: Duration::ZERO,
            evidence: vec![],
            current_state: Some("gather".to_string()),
            directive: Some("Do work.".to_string()),
            intent: Some("test intent value".to_string()),
            template_name: Some("my-template".to_string()),
            history: vec![],
            remaining: vec![],
        });

        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        let right = region_to_string(&buffer, 0..24, 40..100);
        assert!(
            right.contains("test intent value"),
            "Summary tab should show intent; got: {:?}",
            &right[..right.len().min(300)]
        );
    }

    #[test]
    fn tab_cycles_three_times_returns_to_summary() {
        use crate::cli::dashboard_state::DashboardTab;
        use crossterm::event::KeyModifiers;

        let mut state = empty_state();
        assert_eq!(state.active_tab, DashboardTab::Summary);
        for _ in 0..3 {
            state.handle_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Tab,
                KeyModifiers::NONE,
            ));
        }
        assert_eq!(state.active_tab, DashboardTab::Summary);
    }

    #[test]
    fn render_history_tab_shows_evidence_only_session() {
        use crate::cli::dashboard_data::HistoryEntry;
        use crate::cli::dashboard_state::DashboardTab;

        let mut state = empty_state();
        state.active_tab = DashboardTab::History;
        state.detail_cache = Some(DetailData {
            session_id: "sess".to_string(),
            gate_name: None,
            command: None,
            result: None,
            elapsed: Duration::ZERO,
            evidence: vec![EvidenceEntry {
                state: "gather".to_string(),
                fields: serde_json::json!({"k": "v"}),
            }],
            current_state: Some("gather".to_string()),
            directive: None,
            intent: None,
            template_name: None,
            history: vec![HistoryEntry {
                event_type: "evidence_submitted".to_string(),
                timestamp: "2026-01-01T00:00:00Z".to_string(),
                summary: "evidence: gather (1 fields)".to_string(),
                gate_condition: None,
            }],
            remaining: vec![],
        });

        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let right = region_to_string(&buffer, 0..24, 40..100);
        assert!(
            right.contains("evidence") || right.contains("gather"),
            "History tab should show evidence event; got: {:?}",
            &right[..right.len().min(300)]
        );
    }

    #[test]
    fn render_remaining_tab_empty_when_all_states_visited() {
        use crate::cli::dashboard_state::DashboardTab;

        let mut state = empty_state();
        state.active_tab = DashboardTab::Remaining;
        state.detail_cache = Some(DetailData {
            session_id: "sess".to_string(),
            gate_name: None,
            command: None,
            result: None,
            elapsed: Duration::ZERO,
            evidence: vec![],
            current_state: Some("done".to_string()),
            directive: None,
            intent: None,
            template_name: None,
            history: vec![],
            remaining: vec![],
        });

        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let right = region_to_string(&buffer, 0..24, 40..100);
        assert!(
            right.contains("No remaining") || right.contains("remaining"),
            "Remaining tab should show empty message; got: {:?}",
            &right[..right.len().min(300)]
        );
    }

    // -----------------------------------------------------------------------
    // I4: attention-aware list rendering (list-only mode at width 78)
    // -----------------------------------------------------------------------

    fn render_session(
        template_name: Option<&str>,
        current_state: Option<&str>,
        intent: Option<&str>,
        is_terminal: bool,
        is_blocked: bool,
        last_event_at: Option<std::time::SystemTime>,
    ) -> crate::cli::dashboard_data::CachedSession {
        use crate::engine::types::StateFileHeader;
        use std::path::PathBuf;
        use std::time::SystemTime;
        crate::cli::dashboard_data::CachedSession {
            header: StateFileHeader {
                schema_version: 1,
                workflow: "wf".to_string(),
                template_hash: "abc".to_string(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                parent_workflow: None,
                template_source_dir: None,
                session_id: String::new(),
                intent: None,
                template_name: template_name.map(|s| s.to_string()),
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
                respawn_generation: None,
            },
            current_state: current_state.map(|s| s.to_string()),
            is_terminal,
            is_blocked,
            intent: intent.map(|s| s.to_string()),
            mtime: SystemTime::UNIX_EPOCH,
            state_path: PathBuf::new(),
            last_event_at,
            salient_var: None,
            is_unreadable: false,
        }
    }

    #[test]
    fn render_list_shows_label_as_name_not_bare_id() {
        let mut state = DashboardAppState::new(500);
        // A blocked session with a template + state but no intent: label must be
        // derived (e.g. "work-on · implement"), never the bare id "sess-xyz".
        state.tree.sessions.insert(
            "sess-xyz".to_string(),
            render_session(
                Some("work-on"),
                Some("implement"),
                None,
                false,
                true,
                Some(std::time::SystemTime::now()),
            ),
        );
        state.tree.roots = vec!["sess-xyz".to_string()];

        // Width 78 keeps the list full-width (no detail split), so the Name
        // column has room for the label.
        let backend = TestBackend::new(78, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let list = region_to_string(&buffer, 0..24, 0..78);
        assert!(
            list.contains("work-on"),
            "list must show the derived label; got: {:?}",
            &list[..list.len().min(300)]
        );
        assert!(
            !list.contains("sess-xyz"),
            "list must not show the bare session id; got: {:?}",
            &list[..list.len().min(300)]
        );
    }

    #[test]
    fn render_list_shows_idle_column_header() {
        let state = empty_state();
        let backend = TestBackend::new(78, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let header = region_to_string(&buffer, 1..2, 0..78);
        assert!(
            header.contains("Idle"),
            "time column header should read 'Idle'; got: {:?}",
            header
        );
    }

    #[test]
    fn render_list_collapses_receded_into_summary_row() {
        let mut state = DashboardAppState::new(500);
        // One blocked (needs-you) session and one done session (receded).
        state.tree.sessions.insert(
            "live".to_string(),
            render_session(
                Some("work-on"),
                Some("implement"),
                None,
                false,
                true,
                Some(std::time::SystemTime::now()),
            ),
        );
        state.tree.sessions.insert(
            "finished".to_string(),
            render_session(
                Some("work-on"),
                Some("done"),
                None,
                true,
                false,
                Some(std::time::SystemTime::now()),
            ),
        );
        state.tree.roots = vec!["live".to_string(), "finished".to_string()];

        let backend = TestBackend::new(78, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let list = region_to_string(&buffer, 0..24, 0..78);
        assert!(
            list.contains("done") && list.contains("abandoned"),
            "receded set must collapse to a summary row; got: {:?}",
            &list[..list.len().min(400)]
        );
    }

    #[test]
    fn render_list_shows_all_clear_row_when_nothing_needs_you() {
        let mut state = DashboardAppState::new(500);
        // A single active session: nobody needs the user.
        state.tree.sessions.insert(
            "live".to_string(),
            render_session(
                Some("work-on"),
                Some("implement"),
                None,
                false,
                false,
                Some(std::time::SystemTime::now()),
            ),
        );
        state.tree.roots = vec!["live".to_string()];

        let backend = TestBackend::new(78, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let list = region_to_string(&buffer, 0..24, 0..78);
        assert!(
            list.contains("Nothing needs you"),
            "all-clear row must render when needs-you band is empty; got: {:?}",
            &list[..list.len().min(400)]
        );
    }
}
