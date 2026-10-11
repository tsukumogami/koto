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
    TableState, Tabs, Wrap,
};
use ratatui::Frame;

use crate::cli::dashboard_data::{DetailData, Liveness, SessionTree};
use crate::cli::dashboard_state::{
    DashboardAppState, DashboardTab, Membership, SessionTabState, TaskCounts,
};
use crate::session_view::{
    format_size, KeyPreview, Locality, MigratedTo, Sanitized, SessionFacts, SessionViewSnapshot,
    CUT_MARKER,
};

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
        &state.session_tab,
        chunks[1],
    );
}

/// Render the session list as a 4-column table with cursor highlighting and scrollbar.
fn render_list(f: &mut Frame<'_>, state: &DashboardAppState, area: ratatui::layout::Rect) {
    let rows_data = state.visible_rows();

    // Reserve 1 column for border and 1 for scrollbar on the right (2 total overhead).
    // Column widths: State=12, Idle=9, Children=10, plus separators.
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
        let name_cell = format!(
            "{}{}{}",
            row.connector,
            row.display_name,
            membership_badge(row.membership)
        );

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
        // `Children`, not `Tasks`: the count below it is derived from
        // parent-workflow parentage and never from the batch view, so the
        // old name implied batch semantics it never had
        // (DESIGN-request-lifecycle.md Decision 7).
        Cell::from("Children").style(Style::default().add_modifier(Modifier::BOLD)),
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
    session_tab: &SessionTabState,
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
    let tab_titles = vec!["Summary", "History", "Remaining", "Session"];
    let selected = match active_tab {
        DashboardTab::Summary => 0,
        DashboardTab::History => 1,
        DashboardTab::Remaining => 2,
        DashboardTab::Session => 3,
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
        DashboardTab::Session => render_session_tab(f, session_tab, session_name, chunks[1]),
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

/// Prefix on every line of untrusted content, so a value cannot pass for a
/// fact, the banner, or a table row.
const GUTTER: &str = "\u{2502} ";

/// Printed after a field name when the session does not record the field.
const ABSENT: &str = "\u{2014}";

/// Directive lines shown in the facts block before the cut marker.
const DIRECTIVE_LINES: usize = 4;

/// Columns a character may take. Non-ASCII counts double so wrapped text
/// never overflows a width it was fitted to, whatever the terminal draws.
fn cell_cost(c: char) -> usize {
    if c.is_ascii() {
        1
    } else {
        2
    }
}

/// Cut `line` to `width` columns, ending in the cut marker when it was cut.
fn clip(line: &str, width: usize) -> String {
    let cost: usize = line.chars().map(cell_cost).sum();
    if cost <= width {
        return line.to_string();
    }
    let budget = width.saturating_sub(CUT_MARKER.chars().map(cell_cost).sum());
    let mut used = 0;
    let mut out = String::new();
    for c in line.chars() {
        used += cell_cost(c);
        if used > budget {
            break;
        }
        out.push(c);
    }
    out.push_str(CUT_MARKER);
    out
}

/// Lay `text` out behind the gutter in rows of at most `width` columns,
/// stopping once there are more than `limit` rows.
fn gutter_rows(text: &str, width: usize, limit: usize) -> Vec<String> {
    let budget = width
        .saturating_sub(GUTTER.chars().map(cell_cost).sum())
        .max(1);
    let mut rows = Vec::new();
    for line in text.split('\n') {
        let line = line.replace('\t', "    ");
        let mut row = String::new();
        let mut used = 0;
        for c in line.chars() {
            if used + cell_cost(c) > budget {
                rows.push(format!("{GUTTER}{row}"));
                row.clear();
                used = 0;
            }
            row.push(c);
            used += cell_cost(c);
        }
        rows.push(format!("{GUTTER}{row}"));
        if rows.len() > limit {
            break;
        }
    }
    rows
}

/// At most `height` gutter rows of `text`. A cut, from the row limit or
/// from `cut` (the source was already truncated), ends in a marker row.
fn gutter_block(text: &str, cut: bool, width: usize, height: usize) -> Vec<String> {
    if height == 0 {
        return Vec::new();
    }
    let mut rows = gutter_rows(text, width, height);
    let over = rows.len() > height;
    if over || cut {
        rows.truncate(height);
        if rows.len() == height {
            rows.pop();
        }
        rows.push(format!("{GUTTER}{CUT_MARKER}"));
    }
    rows
}

/// The hash as one unbroken token. When the full hash does not fit in
/// `budget` columns it is shortened to its first 16 and last 8 characters
/// around `…`, so it is never split across rows. `koto dashboard <name>
/// --detail` prints the full hash.
fn hash_text(hash: &Option<Sanitized>, budget: usize) -> String {
    let Some(h) = hash.as_ref() else {
        return "invalid-hash".to_string();
    };
    let full = h.to_string();
    let cost: usize = full.chars().map(cell_cost).sum();
    let chars: Vec<char> = full.chars().collect();
    if cost <= budget || chars.len() <= 16 + 8 + 1 {
        return full;
    }
    let head: String = chars[..16].iter().collect();
    let tail: String = chars[chars.len() - 8..].iter().collect();
    format!("{head}\u{2026}{tail}")
}

/// The text a preview shows, and whether the source was cut.
fn preview_text(preview: &KeyPreview, hash_budget: usize) -> (String, bool) {
    match preview {
        KeyPreview::Text { excerpt, truncated } => (excerpt.to_string(), *truncated),
        KeyPreview::Binary { size, hash } => (
            format!(
                "type: binary\nsize: {}\nhash: {}",
                format_size(*size),
                hash_text(hash, hash_budget)
            ),
            false,
        ),
        KeyPreview::NotLocal { size, hash } => (
            format!(
                "not local: the content is in the remote store and is not fetched\nsize: {}\nhash: {}",
                format_size(*size),
                hash_text(hash, hash_budget)
            ),
            false,
        ),
        KeyPreview::Unreadable { reason } => (format!("unreadable: {reason}"), false),
        KeyPreview::InvalidName => ("invalid key name: not read".to_string(), false),
    }
}

/// Rows one fact may take before it ends in the cut marker. Values are
/// already capped by the data layer; this keeps one fact from crowding the
/// key table off a short pane.
const FACT_ROWS: usize = 6;

/// Indent of a fact's continuation rows.
const FACT_INDENT: &str = "  ";

/// `Label: value` laid out in rows of at most `width` columns. A value too
/// long for one row continues on indented rows, so its tail stays readable.
/// A row breaks right after the last `/` or space that fits; only a run with
/// neither within the width is cut at the column limit.
fn wrap_fact(label: &str, value: &str, width: usize) -> Vec<String> {
    let chars: Vec<char> = value.chars().collect();
    let mut rows: Vec<String> = Vec::new();
    let mut prefix = format!("{label}: ");
    let mut pos = 0;
    loop {
        let mut used: usize = prefix.chars().map(cell_cost).sum();
        // The longest run from `pos` that fits, always at least one char.
        let mut end = pos;
        while end < chars.len() && (used + cell_cost(chars[end]) <= width || end == pos) {
            used += cell_cost(chars[end]);
            end += 1;
        }
        if end < chars.len() {
            if let Some(i) = chars[pos..end].iter().rposition(|c| *c == '/' || *c == ' ') {
                end = pos + i + 1;
            }
        }
        rows.push(format!(
            "{prefix}{}",
            chars[pos..end].iter().collect::<String>()
        ));
        pos = end;
        if pos >= chars.len() {
            break;
        }
        prefix = FACT_INDENT.to_string();
    }
    if rows.len() > FACT_ROWS {
        rows.truncate(FACT_ROWS);
        let last = rows.pop().unwrap_or_default();
        rows.push(clip(&format!("{last}{CUT_MARKER}"), width));
    }
    rows
}

/// One `Label: value` entry per fact, `Label: -` for each the session does
/// not record. A directive follows behind the gutter.
fn fact_rows(facts: &SessionFacts, width: usize) -> Vec<String> {
    let one = |label: &str, value: Option<&Sanitized>| {
        wrap_fact(label, value.map(|v| v.as_str()).unwrap_or(ABSENT), width)
    };
    let mut rows = one("State", facts.current_state.as_ref());
    match &facts.directive {
        Some(d) => {
            rows.push("Directive:".to_string());
            rows.extend(gutter_block(d.as_str(), false, width, DIRECTIVE_LINES));
        }
        None => rows.extend(one("Directive", None)),
    }
    rows.extend(one("Anchor", facts.anchor.as_ref()));
    rows.extend(one("Execution dir", facts.execution_dir.as_ref()));
    let store = facts
        .store
        .as_ref()
        .map(|(kind, base)| format!("{kind} {base}"));
    rows.extend(wrap_fact(
        "Store origin",
        store.as_deref().unwrap_or(ABSENT),
        width,
    ));
    rows.extend(one("Intent", facts.intent.as_ref()));
    rows.extend(one("Template", facts.template_name.as_ref()));
    rows.extend(one("Session id", facts.session_id.as_ref()));
    rows.extend(wrap_fact(
        "Created",
        facts.created_at.as_deref().unwrap_or(ABSENT),
        width,
    ));
    rows
}

/// A banner value in double quotes, with `"` and `\` escaped so the value
/// cannot close the quote and forge the fixed wording around it.
fn quote_banner_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

fn migration_banner(m: &MigratedTo) -> String {
    format!(
        "migrated to session {} in workspace {}",
        quote_banner_value(m.target.as_str()),
        quote_banner_value(m.workspace.as_str())
    )
}

/// Render the Session tab: the facts, then either the migration banner or
/// the key table with the focused key's preview.
fn render_session_tab(
    f: &mut Frame<'_>,
    tab: &SessionTabState,
    session_name: Option<&str>,
    area: ratatui::layout::Rect,
) {
    let loaded = session_name.is_some() && tab.loaded_session() == session_name;
    if !loaded {
        let msg = if session_name.is_some() {
            "Loading session"
        } else {
            "No data"
        };
        f.render_widget(Paragraph::new(msg), area);
        return;
    }
    match &tab.snapshot {
        Some(Ok(snap)) => render_snapshot(f, tab, snap, area),
        Some(Err(reason)) => {
            let msg = format!("Session view unavailable: {reason}");
            f.render_widget(Paragraph::new(msg).wrap(Wrap { trim: false }), area);
        }
        None => f.render_widget(Paragraph::new("Loading session"), area),
    }
}

fn render_snapshot(
    f: &mut Frame<'_>,
    tab: &SessionTabState,
    snap: &SessionViewSnapshot,
    area: ratatui::layout::Rect,
) {
    let facts = fact_rows(&snap.facts, area.width as usize);
    let facts_height = (facts.len() as u16).min(area.height);
    let facts_widget = Paragraph::new(Text::from(
        facts.into_iter().map(Line::from).collect::<Vec<_>>(),
    ));

    if let Some(m) = &snap.migrated {
        let chunks =
            Layout::vertical([Constraint::Length(facts_height), Constraint::Min(0)]).split(area);
        f.render_widget(facts_widget, chunks[0]);
        let banner = Paragraph::new(Line::styled(
            migration_banner(m),
            Style::default().fg(Color::Yellow),
        ))
        .wrap(Wrap { trim: false });
        f.render_widget(banner, chunks[1]);
        return;
    }

    let chunks = Layout::vertical([
        Constraint::Length(facts_height),
        Constraint::Fill(3),
        Constraint::Fill(2),
    ])
    .split(area);
    f.render_widget(facts_widget, chunks[0]);
    render_key_table(f, tab, snap, chunks[1]);
    render_key_preview(f, tab, snap, chunks[2]);
}

/// The key table. Rows are built only for the window that ends at or after
/// the cursor, so the cost follows the rows drawn, not the manifest's size.
fn render_key_table(
    f: &mut Frame<'_>,
    tab: &SessionTabState,
    snap: &SessionViewSnapshot,
    area: ratatui::layout::Rect,
) {
    let title = if snap.unlisted_keys > 0 {
        format!(
            " keys ({}, {} not listed) ",
            snap.keys.len(),
            snap.unlisted_keys
        )
    } else {
        format!(" keys ({}) ", snap.keys.len())
    };
    let block = Block::default().borders(Borders::TOP).title(title);
    if snap.keys.is_empty() {
        f.render_widget(Paragraph::new("No context keys").block(block), area);
        return;
    }

    // The top border and the header take two rows.
    let visible = (area.height as usize).saturating_sub(2).max(1);
    let start = (tab.cursor + 1).saturating_sub(visible);
    let end = snap.keys.len().min(start + visible);
    let rows: Vec<Row> = snap.keys[start..end]
        .iter()
        .map(|k| {
            tab.rows_built.set(tab.rows_built.get() + 1);
            let state = match (k.raw.is_some(), k.locality, k.stale) {
                (false, _, _) => "invalid-name",
                (_, Locality::RemoteOnly, _) => "remote-only",
                (_, Locality::Local, true) => "local, stale",
                (_, Locality::Local, false) => "local",
            };
            Row::new(vec![
                Cell::from(k.display.to_string()),
                Cell::from(
                    k.size
                        .map(format_size)
                        .unwrap_or_else(|| ABSENT.to_string()),
                ),
                Cell::from(
                    k.writer
                        .as_ref()
                        .map(|w| w.to_string())
                        .unwrap_or_else(|| ABSENT.to_string()),
                ),
                Cell::from(state),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Min(10),
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Length(12),
        ],
    )
    .header(
        Row::new(vec!["Key", "Size", "Writer", "State"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(block)
    .row_highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    let mut table_state = TableState::default();
    table_state.select(Some(tab.cursor - start));
    f.render_stateful_widget(table, area, &mut table_state);
}

/// The focused key's preview, every row behind the gutter.
fn render_key_preview(
    f: &mut Frame<'_>,
    tab: &SessionTabState,
    snap: &SessionViewSnapshot,
    area: ratatui::layout::Rect,
) {
    let Some(entry) = snap.keys.get(tab.cursor) else {
        return;
    };
    let block = Block::default()
        .borders(Borders::TOP)
        .title(format!(" preview: {} ", entry.display));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Columns left for a hash after the gutter and the `hash: ` label.
    let hash_budget =
        (inner.width as usize).saturating_sub(GUTTER.chars().count() + "hash: ".len());
    let (text, cut) = if entry.raw.is_none() {
        preview_text(&KeyPreview::InvalidName, 0)
    } else {
        match tab.focused_preview() {
            Some(preview) => preview_text(preview, hash_budget),
            None => ("loading".to_string(), false),
        }
    };
    let rows = gutter_block(&text, cut, inner.width as usize, inner.height as usize);
    // Rows are fitted to the width already; the wrapped widget is the one
    // the design names, and it must not need to reflow them.
    let widget = Paragraph::new(Text::from(
        rows.into_iter().map(Line::from).collect::<Vec<_>>(),
    ))
    .wrap(Wrap { trim: false });
    f.render_widget(widget, inner);
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

/// The badge suffix for a row's membership, or an empty string when the
/// session belongs to no container.
///
/// A badge and a count answer different questions: the count says how many
/// children a session has, the badge says which containers the session itself
/// belongs to. A session that is both a batch task and a request leg gets one
/// badge reading `both`, never two counts.
fn membership_badge(membership: Membership) -> &'static str {
    match membership {
        Membership::None => "",
        Membership::Batch => " [batch]",
        Membership::Leg => " [leg]",
        Membership::Both => " [both]",
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
        // enough to show all four header columns (Name, State, Elapsed, Children).
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
                    command_environment: None,
                    schema_version: 1,
                    workflow: "my-wf".to_string(),
                    template_hash: "abc".to_string(),
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
                    root_session_id: None,
                    parent_session_id: None,
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
                is_batch_task: false,
                is_request_leg: false,
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
                    command_environment: None,
                    schema_version: 1,
                    workflow: "my-workflow".to_string(),
                    template_hash: "abc".to_string(),
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
                    root_session_id: None,
                    parent_session_id: None,
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
                is_batch_task: false,
                is_request_leg: false,
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
    fn tab_cycles_four_times_returns_to_summary() {
        use crate::cli::dashboard_state::DashboardTab;
        use crossterm::event::KeyModifiers;

        let mut state = empty_state();
        assert_eq!(state.active_tab, DashboardTab::Summary);
        for _ in 0..4 {
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
                command_environment: None,
                schema_version: 1,
                workflow: "wf".to_string(),
                template_hash: "abc".to_string(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                parent_workflow: None,
                template_source_dir: None,
                template_source_file: None,
                origin: None,
                execution_dir: None,
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
                root_session_id: None,
                parent_session_id: None,
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
            is_batch_task: false,
            is_request_leg: false,
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

    // -----------------------------------------------------------------------
    // I13: membership badges and the Children column
    // -----------------------------------------------------------------------

    /// An active session with the given parentage and memberships.
    fn membership_session(
        parent: Option<&str>,
        is_batch_task: bool,
        is_request_leg: bool,
    ) -> crate::cli::dashboard_data::CachedSession {
        let mut session = render_session(
            Some("work-on"),
            Some("implement"),
            None,
            false,
            false,
            Some(std::time::SystemTime::now()),
        );
        session.header.parent_workflow = parent.map(|p| p.to_string());
        session.is_batch_task = is_batch_task;
        session.is_request_leg = is_request_leg;
        session
    }

    /// Render a one-root tree at list-only width and return the whole pane.
    fn render_tree_to_string(state: &DashboardAppState) -> String {
        let backend = TestBackend::new(78, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, state)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        region_to_string(&buffer, 0..24, 0..78)
    }

    #[test]
    fn the_child_count_column_is_headed_children() {
        let state = empty_state();
        let backend = TestBackend::new(78, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let header = region_to_string(&buffer, 1..2, 0..78);
        assert!(
            header.contains("Children"),
            "the count column header should read 'Children'; got: {header:?}"
        );
        assert!(
            !header.contains("Tasks"),
            "'Tasks' implied batch semantics the count never had; got: {header:?}"
        );
    }

    #[test]
    fn a_batch_task_renders_a_batch_badge() {
        let mut state = DashboardAppState::new(500);
        state
            .tree
            .sessions
            .insert("task".to_string(), membership_session(None, true, false));
        state.tree.roots = vec!["task".to_string()];

        let list = render_tree_to_string(&state);
        assert!(
            list.contains("[batch]") && !list.contains("[leg]") && !list.contains("[both]"),
            "a batch task's row carries the batch badge alone; got: {:?}",
            &list[..list.len().min(400)]
        );
    }

    #[test]
    fn a_bound_leg_renders_a_leg_badge() {
        let mut state = DashboardAppState::new(500);
        state
            .tree
            .sessions
            .insert("leg".to_string(), membership_session(None, false, true));
        state.tree.roots = vec!["leg".to_string()];

        let list = render_tree_to_string(&state);
        assert!(
            list.contains("[leg]") && !list.contains("[batch]") && !list.contains("[both]"),
            "a bound leg's row carries the leg badge alone; got: {:?}",
            &list[..list.len().min(400)]
        );
    }

    #[test]
    fn a_session_in_no_container_renders_no_badge() {
        let mut state = DashboardAppState::new(500);
        state
            .tree
            .sessions
            .insert("plain".to_string(), membership_session(None, false, false));
        state.tree.roots = vec!["plain".to_string()];

        let list = render_tree_to_string(&state);
        assert!(
            !list.contains('['),
            "an unaffiliated session gets no badge at all; got: {:?}",
            &list[..list.len().min(400)]
        );
    }

    #[test]
    fn a_batch_task_that_is_also_a_leg_renders_one_count_and_a_both_badge() {
        // The R66 case: dual membership must be stated on one row rather
        // than implied by a second count. The count itself stays what it
        // has always been -- the number of child sessions.
        let mut state = DashboardAppState::new(500);
        state
            .tree
            .sessions
            .insert("coord".to_string(), membership_session(None, true, true));
        state.tree.sessions.insert(
            "child".to_string(),
            membership_session(Some("coord"), false, false),
        );
        state.tree.roots = vec!["coord".to_string()];

        let list = render_tree_to_string(&state);
        assert!(
            list.contains("[both]"),
            "dual membership renders one 'both' badge; got: {:?}",
            &list[..list.len().min(400)]
        );
        assert!(
            !list.contains("[batch]") && !list.contains("[leg]"),
            "the 'both' badge replaces the single-container badges; got: {:?}",
            &list[..list.len().min(400)]
        );
        assert_eq!(
            list.matches("0/1 done").count(),
            1,
            "one count per surface, over the session set only; got: {:?}",
            &list[..list.len().min(400)]
        );
    }

    #[test]
    fn the_badge_does_not_disturb_the_count() {
        // The badge is an attribute of the row, not an input to the count:
        // the same tree renders the same count with and without membership.
        let mut plain = DashboardAppState::new(500);
        plain
            .tree
            .sessions
            .insert("coord".to_string(), membership_session(None, false, false));
        plain.tree.sessions.insert(
            "child".to_string(),
            membership_session(Some("coord"), false, false),
        );
        plain.tree.roots = vec!["coord".to_string()];

        let mut member = DashboardAppState::new(500);
        member
            .tree
            .sessions
            .insert("coord".to_string(), membership_session(None, true, true));
        member.tree.sessions.insert(
            "child".to_string(),
            membership_session(Some("coord"), false, false),
        );
        member.tree.roots = vec!["coord".to_string()];

        let plain_rows = plain.visible_rows();
        let member_rows = member.visible_rows();
        assert_eq!(
            plain_rows[0].task_counts, member_rows[0].task_counts,
            "membership must not change how children are counted"
        );
    }

    // -----------------------------------------------------------------------
    // Session tab
    // -----------------------------------------------------------------------

    use crate::cli::dashboard_state::SessionLoad;
    use crate::session_view::{
        sanitize, sanitize_capped, KeyEntry, KeyPreview, Locality, MigratedTo, SessionFacts,
        SessionViewSnapshot, BANNER_VALUE_CAP, CUT_MARKER,
    };
    use std::time::SystemTime;

    const WIDTH: u16 = 100;
    const HEIGHT: u16 = 30;
    /// The detail pane's inner left edge at `WIDTH`: 40% list, then a border.
    const PANE_X: u16 = 41;

    fn facts() -> SessionFacts {
        SessionFacts {
            name: "wf".to_string(),
            session_id: Some(sanitize("id-1")),
            created_at: Some("2026-01-01T00:00:00Z".to_string()),
            intent: Some(sanitize("ship it")),
            template_name: Some(sanitize("work-on")),
            current_state: Some(sanitize("implement")),
            directive: Some(sanitize("Do the thing")),
            execution_dir: Some(sanitize("/work")),
            anchor: Some(sanitize("/anchor")),
            store: Some((sanitize("local"), sanitize("/store"))),
        }
    }

    fn entry(name: &str, size: u64) -> KeyEntry {
        KeyEntry {
            display: sanitize_capped(name, 256),
            raw: Some(name.to_string()),
            size: Some(size),
            writer: None,
            created_at: None,
            locality: Locality::Local,
            stale: false,
        }
    }

    fn snapshot(keys: Vec<KeyEntry>) -> SessionViewSnapshot {
        SessionViewSnapshot {
            facts: facts(),
            migrated: None,
            keys,
            unlisted_keys: 0,
        }
    }

    fn text(s: &str, truncated: bool) -> KeyPreview {
        KeyPreview::Text {
            excerpt: sanitize(s),
            truncated,
        }
    }

    /// A tab loaded the way the tick loop loads it: through `tick`.
    fn loaded(snap: SessionViewSnapshot, cursor: usize, preview: KeyPreview) -> SessionTabState {
        let mut tab = SessionTabState::default();
        let mtime = SystemTime::UNIX_EPOCH;
        tab.tick("wf", mtime, |_| Ok(snap.clone()), |_, _| preview.clone());
        tab.move_cursor(cursor as isize);
        let again = preview.clone();
        tab.tick("wf", mtime, |_| unreachable!(), |_, _| again);
        tab
    }

    fn draw(tab: SessionTabState) -> (RatatuiBuffer, SessionTabState) {
        let mut state = empty_state();
        state.active_tab = DashboardTab::Session;
        state.focused_id = Some("wf".to_string());
        state.session_tab = tab;
        let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();
        (terminal.backend().buffer().clone(), state.session_tab)
    }

    /// The pane's rows, left border and right border stripped.
    fn pane_rows(buffer: &RatatuiBuffer) -> Vec<String> {
        (1..HEIGHT - 1)
            .map(|y| region_to_string(buffer, y..y + 1, PANE_X..WIDTH - 1))
            .collect()
    }

    fn pane_text(buffer: &RatatuiBuffer) -> String {
        pane_rows(buffer).join("\n")
    }

    #[test]
    fn session_tab_shows_facts_keys_with_size_and_marks_absent_fields() {
        let mut snap = snapshot(vec![entry("notes.md", 11), entry("big.md", 4096)]);
        snap.facts.intent = None;
        snap.facts.anchor = None;
        snap.facts.store = None;
        let (buffer, _) = draw(loaded(snap, 0, text("hello", false)));
        let pane = pane_text(&buffer);

        assert!(pane.contains("State: implement"), "{pane}");
        assert!(pane.contains("Directive:"), "{pane}");
        // Every absent field prints its name and the dash, never a blank.
        for field in ["Intent", "Anchor", "Store origin"] {
            assert!(
                pane.contains(&format!("{field}: \u{2014}")),
                "{field}: {pane}"
            );
        }
        assert!(pane.contains("Template: work-on"), "{pane}");
        assert!(pane.contains("Session"), "tab title: {pane}");
        // Every key is listed with its size.
        assert!(pane.contains("notes.md"), "{pane}");
        assert!(pane.contains("11 B"), "{pane}");
        assert!(pane.contains("big.md"), "{pane}");
        assert!(pane.contains("4.0 KiB"), "{pane}");
    }

    #[test]
    fn session_tab_shows_store_origin_when_recorded() {
        let (buffer, _) = draw(loaded(snapshot(vec![]), 0, text("", false)));
        let pane = pane_text(&buffer);
        assert!(pane.contains("Store origin: local /store"), "{pane}");
        assert!(pane.contains("No context keys"), "{pane}");
    }

    #[test]
    fn session_tab_cuts_a_long_excerpt_with_a_marker_and_guttered_rows() {
        let excerpt = "a line of excerpt text that is fairly long\n".repeat(100);
        let (buffer, _) = draw(loaded(
            snapshot(vec![entry("long.md", 9000)]),
            0,
            text(&excerpt, true),
        ));
        let rows = pane_rows(&buffer);
        let start = rows
            .iter()
            .position(|r| r.contains("preview: long.md"))
            .expect("preview title");
        let content = &rows[start + 1..];
        assert!(content.len() > 3);
        for row in content {
            assert!(
                row.starts_with("\u{2502} "),
                "every preview row carries the gutter: {row:?}"
            );
        }
        assert!(
            content.last().unwrap().contains(CUT_MARKER),
            "the cut is marked: {:?}",
            content.last()
        );
    }

    #[test]
    fn session_tab_marks_a_cut_even_when_the_excerpt_fits_the_viewport() {
        let (buffer, _) = draw(loaded(
            snapshot(vec![entry("k", 5000)]),
            0,
            text("short body", true),
        ));
        let pane = pane_text(&buffer);
        assert!(pane.contains("\u{2502} short body"), "{pane}");
        assert!(pane.contains(&format!("\u{2502} {CUT_MARKER}")), "{pane}");
    }

    #[test]
    fn session_tab_binary_key_shows_type_size_and_hash_but_no_bytes() {
        let hash = "ab".repeat(32);
        let preview = KeyPreview::Binary {
            size: 4096,
            hash: Some(sanitize(&hash)),
        };
        let (buffer, _) = draw(loaded(snapshot(vec![entry("blob.bin", 4096)]), 0, preview));
        let pane = pane_text(&buffer);
        assert!(pane.contains("\u{2502} type: binary"), "{pane}");
        assert!(pane.contains("size: 4.0 KiB"), "{pane}");
        assert!(
            pane.contains(&format!("hash: {}\u{2026}{}", &hash[..16], &hash[56..])),
            "a pane too narrow for the full hash shows it shortened: {pane}"
        );
    }

    #[test]
    fn session_tab_wraps_a_long_store_path_instead_of_cutting_it() {
        let tail = "sessions-end-marker";
        let base = format!("/Users/someone/{}/{tail}", "deep-directory/".repeat(8));
        let mut snap = snapshot(vec![]);
        snap.facts.store = Some((sanitize("local"), sanitize(&base)));
        let (buffer, _) = draw(loaded(snap, 0, text("", false)));
        let rows = pane_rows(&buffer);
        let start = rows
            .iter()
            .position(|r| r.starts_with("Store origin: local /Users/someone/"))
            .expect("store row");
        // Rejoin the continuation rows (indented) and compare to the value.
        let mut joined = rows[start].trim_end().to_string();
        for row in &rows[start + 1..] {
            match row.strip_prefix("  ") {
                Some(rest) if !rest.starts_with(' ') && !rest.trim().is_empty() => {
                    joined.push_str(rest.trim_end());
                }
                _ => break,
            }
            if joined.ends_with(tail) {
                break;
            }
        }
        assert_eq!(joined, format!("Store origin: local {base}"), "{rows:#?}");
        assert!(!joined.contains(CUT_MARKER));
    }

    #[test]
    fn wrap_fact_breaks_a_path_after_a_slash_not_mid_segment() {
        let value = "local /Users/danielgazineu/koto-session-view-demo/sessions";
        let rows = wrap_fact("Store origin", value, 70);
        assert_eq!(
            rows,
            vec![
                "Store origin: local /Users/danielgazineu/koto-session-view-demo/".to_string(),
                "  sessions".to_string(),
            ]
        );
        // Every row but the last ends at a boundary.
        for row in &rows[..rows.len() - 1] {
            assert!(row.ends_with('/') || row.ends_with(' '), "{row:?}");
        }
    }

    #[test]
    fn wrap_fact_breaks_after_a_space_in_prose() {
        let rows = wrap_fact("Intent", "alpha beta gamma delta", 20);
        assert_eq!(rows, vec!["Intent: alpha beta ", "  gamma delta"]);
    }

    #[test]
    fn wrap_fact_hard_breaks_an_unbreakable_token_and_keeps_the_cap() {
        let token = "x".repeat(40);
        let rows = wrap_fact("Anchor", &token, 20);
        assert_eq!(rows.len(), 3, "{rows:?}");
        assert_eq!(
            rows.concat().replace("Anchor: ", "").replace("  ", ""),
            token
        );
        assert!(rows.iter().all(|r| r.chars().count() <= 20), "{rows:?}");

        let long = "y".repeat(500);
        let rows = wrap_fact("Anchor", &long, 20);
        assert_eq!(rows.len(), FACT_ROWS);
        assert!(rows.last().unwrap().ends_with(CUT_MARKER), "{rows:?}");
    }

    #[test]
    fn session_tab_binary_hash_is_never_split_across_rows() {
        let hash = "f4".repeat(32);
        let preview = KeyPreview::Binary {
            size: 4096,
            hash: Some(sanitize(&hash)),
        };
        let (buffer, _) = draw(loaded(snapshot(vec![entry("blob.bin", 4096)]), 0, preview));
        let rows = pane_rows(&buffer);
        let hash_rows: Vec<&String> = rows.iter().filter(|r| r.contains("hash: ")).collect();
        assert_eq!(hash_rows.len(), 1, "{rows:#?}");
        let row = hash_rows[0];
        // Either the whole hash sits on its row, or a shortened form does:
        // first 16 characters, an ellipsis, last 8.
        let whole = row.contains(&hash);
        let short = row.contains(&format!("{}\u{2026}{}", &hash[..16], &hash[56..]));
        assert!(whole || short, "{row:?}");
        // No row after the hash row continues it.
        let after = rows.iter().skip_while(|r| !r.contains("hash: ")).nth(1);
        assert!(
            after.map(|r| !r.contains("f4f4")).unwrap_or(true),
            "{rows:#?}"
        );
    }

    #[test]
    fn session_tab_unreadable_key_shows_its_marker_while_other_keys_render() {
        let preview = KeyPreview::Unreadable {
            reason: sanitize("permission denied"),
        };
        let snap = snapshot(vec![entry("a-bad.md", 3), entry("b-good.md", 4)]);
        let (buffer, _) = draw(loaded(snap, 0, preview));
        let pane = pane_text(&buffer);
        assert!(
            pane.contains("\u{2502} unreadable: permission denied"),
            "{pane}"
        );
        assert!(pane.contains("a-bad.md"), "{pane}");
        assert!(pane.contains("b-good.md"), "{pane}");
    }

    #[test]
    fn session_tab_remote_only_key_shows_not_local_with_recorded_size_and_hash() {
        let mut remote = entry("remote.md", 2048);
        remote.locality = Locality::RemoteOnly;
        let hash = "cd".repeat(32);
        let preview = KeyPreview::NotLocal {
            size: 2048,
            hash: Some(sanitize(&hash)),
        };
        let (buffer, _) = draw(loaded(snapshot(vec![remote]), 0, preview));
        let pane = pane_text(&buffer);
        assert!(pane.contains("remote-only"), "{pane}");
        assert!(pane.contains("\u{2502} not local"), "{pane}");
        assert!(pane.contains("size: 2.0 KiB"), "{pane}");
        assert!(
            pane.contains(&format!("hash: {}\u{2026}{}", &hash[..16], &hash[56..])),
            "a pane too narrow for the full hash shows it shortened: {pane}"
        );
    }

    #[test]
    fn session_tab_invalid_name_says_so_and_stale_is_flagged() {
        let mut bad = entry("a-bad", 1);
        bad.raw = None;
        let mut stale = entry("b-stale", 1);
        stale.stale = true;
        let (buffer, tab) = draw(loaded(snapshot(vec![bad, stale]), 0, text("x", false)));
        let pane = pane_text(&buffer);
        assert!(pane.contains("invalid-name"), "{pane}");
        assert!(
            pane.contains("\u{2502} invalid key name: not read"),
            "{pane}"
        );
        assert!(pane.contains("local, stale"), "{pane}");
        assert_eq!(tab.preview_reads, 0, "an invalid name is never read");
    }

    fn migrated_snapshot(target: &str, workspace: &str) -> SessionViewSnapshot {
        let mut snap = snapshot(vec![entry("should-not-show.md", 1)]);
        snap.migrated = Some(MigratedTo {
            target: sanitize_capped(target, BANNER_VALUE_CAP),
            workspace: sanitize_capped(workspace, BANNER_VALUE_CAP),
        });
        snap
    }

    #[test]
    fn migration_banner_quotes_and_escapes_values_inside_fixed_wording() {
        let snap = migrated_snapshot("x\" in workspace \"trusted", "a\\b");
        let banner = migration_banner(snap.migrated.as_ref().unwrap());
        assert_eq!(
            banner,
            "migrated to session \"x\\\" in workspace \\\"trusted\" in workspace \"a\\\\b\""
        );
    }

    #[test]
    fn migration_banner_caps_each_value_and_hides_the_key_table() {
        let long = "t".repeat(BANNER_VALUE_CAP * 3);
        let snap = migrated_snapshot(&long, "ws");
        let m = snap.migrated.clone().unwrap();
        assert!(m.target.as_str().ends_with(CUT_MARKER));
        assert_eq!(
            m.target.as_str().chars().count(),
            BANNER_VALUE_CAP + CUT_MARKER.chars().count()
        );

        let (buffer, tab) = draw(loaded(snap, 0, text("x", false)));
        let pane = pane_text(&buffer);
        assert!(pane.contains("migrated to session"), "{pane}");
        assert!(pane.contains("in workspace"), "{pane}");
        assert!(!pane.contains("should-not-show.md"), "{pane}");
        assert!(!pane.contains("keys ("), "{pane}");
        assert_eq!(tab.rows_built.get(), 0);
    }

    #[test]
    fn hostile_values_leave_no_control_characters_in_any_cell() {
        let hostile = "a\u{1b}[2J\u{1b}]0;owned\u{7}\u{9b}31m\u{202e}evil\u{7f}\u{200b}z";
        // Long enough to force the wrapped widget to reflow.
        let excerpt = format!("{hostile} {}\n{hostile}", "w".repeat(200));
        let mut key = entry("k", 1);
        key.display = sanitize_capped(hostile, 256);
        key.writer = Some(sanitize_capped(hostile, 64));
        let mut snap = snapshot(vec![key]);
        snap.facts.intent = Some(sanitize_capped(hostile, 1024));
        snap.facts.directive = Some(sanitize(hostile));
        let (buffer, _) = draw(loaded(snap, 0, text(&excerpt, false)));

        let mut seen_escape = false;
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let symbol = buffer.cell((x, y)).unwrap().symbol().to_string();
                for c in symbol.chars() {
                    assert!(!c.is_control(), "control char {c:?} at ({x},{y})");
                    assert!(
                        !matches!(c, '\u{202A}'..='\u{202E}' | '\u{200B}'..='\u{200F}'),
                        "invisible formatting char {c:?} at ({x},{y})"
                    );
                }
            }
        }
        let pane = pane_text(&buffer);
        seen_escape |= pane.contains("\\u{1B}[2J");
        assert!(seen_escape, "the escape stays visible as text: {pane}");
    }

    #[test]
    fn key_table_builds_rows_only_for_the_visible_window() {
        let names: Vec<String> = (0..5000).map(|i| format!("key-{i:05}")).collect();
        let keys: Vec<KeyEntry> = names.iter().map(|n| entry(n, 1)).collect();

        let (buffer, tab) = draw(loaded(snapshot(keys.clone()), 0, text("x", false)));
        let built = tab.rows_built.get();
        assert!(built > 0 && built < 30, "built {built} rows for 5000 keys");
        assert!(pane_text(&buffer).contains("key-00000"));

        // Deep in the list the window follows the cursor, still bounded.
        let (buffer, tab) = draw(loaded(snapshot(keys), 3000, text("x", false)));
        let built = tab.rows_built.get();
        assert!(built > 0 && built < 30, "built {built} rows at depth");
        let pane = pane_text(&buffer);
        assert!(pane.contains("key-03000"), "{pane}");
        assert!(!pane.contains("key-00000"), "{pane}");
    }

    // Independence, not a pre-change golden: Summary, History and Remaining
    // must not read Session-tab state. Their own render code is unchanged by
    // the Session tab and stays pinned by the existing tests above.
    #[test]
    fn other_tabs_render_the_same_with_or_without_session_data() {
        let detail = || DetailData {
            session_id: "wf".to_string(),
            gate_name: Some("g".to_string()),
            command: None,
            result: Some("PASS".to_string()),
            elapsed: Duration::from_secs(0),
            evidence: vec![],
            current_state: Some("implement".to_string()),
            directive: Some("Do it".to_string()),
            intent: None,
            template_name: None,
            history: vec![],
            remaining: vec!["done".to_string()],
        };
        for tab in [
            DashboardTab::Summary,
            DashboardTab::History,
            DashboardTab::Remaining,
        ] {
            let render = |session_tab: SessionTabState| {
                let mut state = empty_state();
                state.active_tab = tab.clone();
                state.focused_id = Some("wf".to_string());
                state.detail_cache = Some(detail());
                state.session_tab = session_tab;
                let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
                terminal.draw(|f| render_frame(f, &state)).unwrap();
                terminal.backend().buffer().clone()
            };
            let with = render(loaded(
                snapshot(vec![entry("k", 1)]),
                0,
                text("body", false),
            ));
            let without = render(SessionTabState::default());
            assert_eq!(with, without, "{tab:?} must not depend on the Session tab");
        }
    }

    #[test]
    fn session_tab_without_a_loaded_snapshot_says_so() {
        let (buffer, _) = draw(SessionTabState::default());
        assert!(pane_text(&buffer).contains("Loading session"));
    }

    /// Intent is recorded as `intent_updated` events with the header field
    /// left empty; the tab shows the latest one, not `—`.
    #[test]
    fn session_tab_shows_the_intent_koto_records_as_events() {
        use crate::engine::types::EventPayload;
        use crate::session::local::LocalBackend;
        use crate::session::{Backend, SessionBackend};

        let tmp = tempfile::TempDir::new().unwrap();
        let local = LocalBackend::with_base_dir(tmp.path().to_path_buf());
        let header = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "workflow": "wf",
            "template_hash": "h",
            "created_at": "2026-01-01T00:00:00Z",
        }))
        .unwrap();
        let init = serde_json::from_value(serde_json::json!({
            "seq": 1,
            "timestamp": "2026-01-01T00:00:00Z",
            "type": "workflow_initialized",
            "payload": {"template_path": "", "variables": {}},
        }))
        .unwrap();
        local.init_state_file("wf", header, vec![init]).unwrap();
        for text in ["first intent", "second intent"] {
            local
                .append_event(
                    "wf",
                    &EventPayload::IntentUpdated {
                        intent: text.to_string(),
                    },
                    "2026-01-01T00:00:01Z",
                )
                .unwrap();
        }
        let backend = Backend::Local(local);

        let snap = crate::session_view::snapshot(&backend, "wf").unwrap();
        let (buffer, _) = draw(loaded(snap, 0, text("", false)));
        let pane = pane_text(&buffer);
        assert!(pane.contains("Intent: second intent"), "{pane}");
        assert!(!pane.contains("Intent: \u{2014}"), "{pane}");
    }

    #[test]
    fn session_tab_reads_a_real_store_one_preview_per_tick() {
        use crate::session::context::ContextStore;
        use crate::session::local::LocalBackend;
        use crate::session::{Backend, SessionBackend};

        let tmp = tempfile::TempDir::new().unwrap();
        let local = LocalBackend::with_base_dir(tmp.path().to_path_buf());
        let header = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "workflow": "wf",
            "template_hash": "h",
            "created_at": "2026-01-01T00:00:00Z",
        }))
        .unwrap();
        let init = serde_json::from_value(serde_json::json!({
            "seq": 1,
            "timestamp": "2026-01-01T00:00:00Z",
            "type": "workflow_initialized",
            "payload": {"template_path": "", "variables": {}},
        }))
        .unwrap();
        local.init_state_file("wf", header, vec![init]).unwrap();
        local.add("wf", "a.md", b"alpha body").unwrap();
        local.add("wf", "b.md", b"beta body").unwrap();
        let backend = Backend::Local(local);

        let mut tab = SessionTabState::default();
        let mtime = SystemTime::UNIX_EPOCH;
        let tick = |tab: &mut SessionTabState| {
            tab.tick(
                "wf",
                mtime,
                |n| crate::session_view::snapshot(&backend, n),
                |n, k| {
                    crate::session_view::read_key_preview(
                        &backend,
                        n,
                        k,
                        crate::session_view::EXCERPT_BOUND,
                    )
                },
            )
        };
        tick(&mut tab);
        assert_eq!(tab.preview_reads, 1);
        for _ in 0..4 {
            tick(&mut tab);
        }
        assert_eq!(tab.preview_reads, 1, "cached per (session, key, mtime)");
        assert_eq!(tab.plan("wf", mtime), SessionLoad::Idle);

        let (buffer, _) = draw(tab);
        let pane = pane_text(&buffer);
        assert!(pane.contains("\u{2502} alpha body"), "{pane}");
        assert!(pane.contains("b.md"), "{pane}");
        assert!(pane.contains("Anchor: \u{2014}"), "{pane}");
    }
}
