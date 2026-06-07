//! Entry point for the `koto dashboard` command.
//!
//! Wires the data layer, state machine, and renderer into a tick loop with
//! RAII terminal cleanup and optional `--once` scripting output.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event, execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use crate::cli::dashboard_data::liveness::{attention_key, idle_for, is_receded};
use crate::cli::dashboard_data::{
    self, classify_liveness, compute_elapsed_since, CachedSession, Liveness, SessionTree,
};
use crate::cli::dashboard_render::render_frame;
use crate::cli::dashboard_state::DashboardAppState;
use crate::cli::DashboardArgs;
use crate::session::SessionBackend;

/// RAII guard that restores the terminal on any exit path, including `?` propagation.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

/// Classify a session into one of five status buckets for `--once` output.
fn classify_status(session: &CachedSession) -> &'static str {
    if session.is_terminal {
        let state = session
            .current_state
            .as_deref()
            .unwrap_or("")
            .to_lowercase();
        if state.contains("failed") || state.contains("error") {
            "failed"
        } else {
            "done"
        }
    } else if session.is_blocked {
        "blocked"
    } else if session.current_state.is_some() {
        "running"
    } else {
        "unknown"
    }
}

/// Stable machine-readable liveness token for the `--once` `liveness` column
/// and the `--status` filter. Kebab-case so it is shell-friendly.
fn liveness_token(liveness: Liveness) -> &'static str {
    match liveness {
        Liveness::NeedsYouBlocked => "needs-you-blocked",
        Liveness::NeedsYouFailed => "needs-you-failed",
        Liveness::NeedsYouStalled => "needs-you-stalled",
        Liveness::Active => "active",
        Liveness::Idle => "idle",
        Liveness::Pending => "pending",
        Liveness::Done => "done",
    }
}

/// Replace tab and newline characters with single spaces so an appended column
/// value can never break the tab-separated `--once` contract.
fn sanitize_field(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

/// Format an elapsed duration as a compact human-readable string.
///
/// Mirrors the format used by the render layer: `"0s"`, `"{s}s"`, `"{m}m{s}s"`, `"{h}h{m}m"`.
fn format_elapsed(secs: u64) -> String {
    if secs == 0 {
        return "0s".to_string();
    }
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let s = secs % 60;
    if hours > 0 {
        format!("{}h{}m", hours, minutes)
    } else if minutes > 0 {
        format!("{}m{}s", minutes, s)
    } else {
        format!("{}s", secs)
    }
}

/// Print the `--once` tab-separated feed.
///
/// Columns: `id, current_state, elapsed, status, intent, template` (the
/// original six, unchanged for positional compatibility) plus appended
/// `idle, liveness` (columns 7-8). Rows are emitted in attention order. The
/// receded set is excluded by default; `--all` includes it. `--status` /
/// `--needs-you` filter the rows. Appended fields are sanitized of tabs and
/// newlines so the tab-separated contract holds.
fn run_once(args: &DashboardArgs, tree: &SessionTree) {
    use std::time::SystemTime;

    let now = SystemTime::now();

    // Build (session_id, &session, liveness, idle) tuples, applying filters.
    let mut rows: Vec<(&str, &CachedSession, Liveness, std::time::Duration)> = Vec::new();
    for (session_id, session) in tree.sessions.iter() {
        // Optional name filter (matches the legacy behavior).
        if let Some(ref filter) = args.name {
            if session_id != filter {
                continue;
            }
        }

        let liveness = classify_liveness(session, now);
        let idle = idle_for(session, now);

        // Default excludes the receded set unless --all.
        if !args.all && is_receded(liveness, idle) {
            continue;
        }

        // --needs-you: only the needs-you band.
        if args.needs_you && attention_key(liveness, idle).0 != 0 {
            continue;
        }

        // --status <liveness>: exact liveness token match.
        if let Some(ref status) = args.status {
            if liveness_token(liveness) != status.as_str() {
                continue;
            }
        }

        rows.push((session_id.as_str(), session, liveness, idle));
    }

    // Attention order; tie-break on session id for determinism.
    rows.sort_by(|a, b| {
        attention_key(a.2, a.3)
            .cmp(&attention_key(b.2, b.3))
            .then_with(|| a.0.cmp(b.0))
    });

    for (session_id, session, liveness, idle) in rows {
        println!("{}", format_once_line(session_id, session, liveness, idle));
    }
}

/// Build a single `--once` line for a session. First six columns match the
/// legacy positional contract; columns 7-8 are the appended `idle` and
/// `liveness`. Appended/interpolated fields are sanitized of tabs/newlines.
fn format_once_line(
    session_id: &str,
    session: &CachedSession,
    liveness: Liveness,
    idle: std::time::Duration,
) -> String {
    let current_state = session.current_state.as_deref().unwrap_or("");
    let elapsed_secs = compute_elapsed_since(&session.header.created_at).as_secs();
    let elapsed = format_elapsed(elapsed_secs);
    let status_bucket = classify_status(session);
    let idle_str = format_elapsed(idle.as_secs());
    let liveness_str = liveness_token(liveness);
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        session_id,
        sanitize_field(current_state),
        elapsed,
        status_bucket,
        sanitize_field(session.intent.as_deref().unwrap_or("")),
        sanitize_field(session.header.template_name.as_deref().unwrap_or("")),
        sanitize_field(&idle_str),
        liveness_str,
    )
}

/// Entry point called from the CLI dispatch in `src/cli/mod.rs`.
pub fn run(args: DashboardArgs, backend: &dyn SessionBackend) -> Result<()> {
    // --once: print tab-separated lines and exit without a TUI.
    if args.once {
        let mut tree = SessionTree::new();
        dashboard_data::refresh(&mut tree, backend)?;
        run_once(&args, &tree);
        return Ok(());
    }

    // TUI mode: set up terminal, register signal handler, run tick loop.
    let poll_interval_ms = args.interval.unwrap_or(500);
    let mut state = DashboardAppState::new(poll_interval_ms);

    // Register SIGINT/SIGTERM handlers using signal-hook so that Ctrl+C from a
    // non-interactive context (e.g. piped stdin) is caught even without a keypress.
    let shutdown = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    {
        if let Err(e) =
            signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&shutdown))
        {
            eprintln!("warning: failed to register SIGINT handler: {}", e);
        }
        if let Err(e) =
            signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&shutdown))
        {
            eprintln!("warning: failed to register SIGTERM handler: {}", e);
        }
    }

    enable_raw_mode()?;
    // Bind the guard immediately after enable_raw_mode so that if
    // EnterAlternateScreen fails, disable_raw_mode still runs on drop.
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen)?;

    let crossterm_backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(crossterm_backend)?;

    loop {
        // Poll for keyboard input with a 50ms timeout (one tick).
        if event::poll(Duration::from_millis(50))? {
            if let event::Event::Key(key) = event::read()? {
                state.handle_key(key);
            }
        }

        state.tick_count += 1;
        if state.tick_count >= state.poll_every_n_ticks {
            dashboard_data::refresh(&mut state.tree, backend)?;
            state.clamp_cursor();
            state.tick_count = 0;

            // Refresh detail cache for the focused session using mtime guard.
            if let Some(ref id) = state.focused_id.clone() {
                let current_mtime = state.tree.sessions.get(id).map(|s| s.mtime);
                let path = state.tree.sessions.get(id).map(|s| s.state_path.clone());

                let session_changed = state.detail_cache_session.as_deref() != Some(id.as_str());
                let mtime_changed = current_mtime != state.detail_cache_mtime;

                if session_changed || mtime_changed {
                    if let Some(path) = path {
                        state.detail_cache = dashboard_data::read_detail(&path, id);
                        state.detail_cache_mtime = current_mtime;
                        state.detail_cache_session = Some(id.clone());
                    }
                }
            }
        }

        if state.should_quit || shutdown.load(Ordering::SeqCst) {
            break;
        }

        terminal.draw(|f| render_frame(f, &state))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // classify_status
    // -----------------------------------------------------------------------

    fn make_session(current_state: Option<&str>, is_terminal: bool) -> CachedSession {
        make_session_with_blocked(current_state, is_terminal, false)
    }

    fn make_session_with_blocked(
        current_state: Option<&str>,
        is_terminal: bool,
        is_blocked: bool,
    ) -> CachedSession {
        use crate::engine::types::StateFileHeader;
        use std::path::PathBuf;
        use std::time::SystemTime;
        CachedSession {
            header: StateFileHeader {
                schema_version: 1,
                workflow: "test".to_string(),
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
            current_state: current_state.map(|s| s.to_string()),
            is_terminal,
            is_blocked,
            intent: None,
            mtime: SystemTime::UNIX_EPOCH,
            state_path: PathBuf::new(),
            last_event_at: None,
            salient_var: None,
        }
    }

    #[test]
    fn classify_status_running_session() {
        let s = make_session(Some("gather"), false);
        assert_eq!(classify_status(&s), "running");
    }

    #[test]
    fn classify_status_terminal_done() {
        let s = make_session(Some("done"), true);
        assert_eq!(classify_status(&s), "done");
    }

    #[test]
    fn classify_status_terminal_failed() {
        let s = make_session(Some("failed"), true);
        assert_eq!(classify_status(&s), "failed");
    }

    #[test]
    fn classify_status_terminal_error() {
        let s = make_session(Some("parse_error"), true);
        assert_eq!(classify_status(&s), "failed");
    }

    #[test]
    fn classify_status_blocked_gate_failed() {
        let s = make_session_with_blocked(Some("build"), false, true);
        assert_eq!(classify_status(&s), "blocked");
    }

    #[test]
    fn classify_status_unknown_no_state() {
        let s = make_session(None, false);
        assert_eq!(classify_status(&s), "unknown");
    }

    // -----------------------------------------------------------------------
    // format_elapsed
    // -----------------------------------------------------------------------

    #[test]
    fn format_elapsed_zero() {
        assert_eq!(format_elapsed(0), "0s");
    }

    #[test]
    fn format_elapsed_seconds() {
        assert_eq!(format_elapsed(45), "45s");
    }

    #[test]
    fn format_elapsed_minutes_and_seconds() {
        assert_eq!(format_elapsed(125), "2m5s");
    }

    #[test]
    fn format_elapsed_hours() {
        assert_eq!(format_elapsed(3661), "1h1m");
    }

    // -----------------------------------------------------------------------
    // I5: --once appended columns + filters
    // -----------------------------------------------------------------------

    fn once_session(
        template_name: Option<&str>,
        current_state: Option<&str>,
        intent: Option<&str>,
        is_terminal: bool,
        is_blocked: bool,
    ) -> CachedSession {
        let mut s = make_session_with_blocked(current_state, is_terminal, is_blocked);
        s.header.template_name = template_name.map(|t| t.to_string());
        s.intent = intent.map(|i| i.to_string());
        // Fresh last_event_at so non-terminal sessions read as Active.
        s.last_event_at = Some(std::time::SystemTime::now());
        s
    }

    #[test]
    fn once_line_keeps_first_six_columns_and_appends_two() {
        let s = once_session(
            Some("work-on"),
            Some("implement"),
            Some("fix bug"),
            false,
            false,
        );
        let line = format_once_line("sess-1", &s, Liveness::Active, std::time::Duration::ZERO);
        let cols: Vec<&str> = line.split('\t').collect();
        assert_eq!(
            cols.len(),
            8,
            "must emit exactly eight tab-separated columns"
        );
        // First six unchanged from the legacy contract.
        assert_eq!(cols[0], "sess-1");
        assert_eq!(cols[1], "implement");
        assert_eq!(cols[3], "running"); // classify_status bucket
        assert_eq!(cols[4], "fix bug"); // intent
        assert_eq!(cols[5], "work-on"); // template
                                        // Appended columns 7-8.
        assert_eq!(cols[7], "active", "column 8 is the liveness token");
    }

    #[test]
    fn once_line_sanitizes_tabs_and_newlines_in_appended_fields() {
        let s = once_session(
            Some("tmpl\twith\ttabs"),
            Some("state\nbreak"),
            Some("intent\twith\ttab"),
            false,
            false,
        );
        let line = format_once_line("sess-2", &s, Liveness::Active, std::time::Duration::ZERO);
        let cols: Vec<&str> = line.split('\t').collect();
        assert_eq!(
            cols.len(),
            8,
            "embedded tabs/newlines must not add columns; got: {:?}",
            cols
        );
        assert!(!cols[4].contains('\n') && !cols[5].contains('\n'));
    }

    #[test]
    fn liveness_token_is_kebab_case_for_each_variant() {
        assert_eq!(
            liveness_token(Liveness::NeedsYouBlocked),
            "needs-you-blocked"
        );
        assert_eq!(liveness_token(Liveness::NeedsYouFailed), "needs-you-failed");
        assert_eq!(
            liveness_token(Liveness::NeedsYouStalled),
            "needs-you-stalled"
        );
        assert_eq!(liveness_token(Liveness::Active), "active");
        assert_eq!(liveness_token(Liveness::Idle), "idle");
        assert_eq!(liveness_token(Liveness::Pending), "pending");
        assert_eq!(liveness_token(Liveness::Done), "done");
    }

    /// Build a tree-backed run_once filter test by exercising the same predicate
    /// logic run_once uses. We construct a small tree and assert which session
    /// ids survive each filter.
    fn surviving_ids(args: &DashboardArgs, tree: &SessionTree) -> Vec<String> {
        use crate::cli::dashboard_data::liveness::{attention_key, idle_for, is_receded};
        use std::time::SystemTime;
        let now = SystemTime::now();
        let mut out: Vec<String> = Vec::new();
        for (id, session) in tree.sessions.iter() {
            if let Some(ref filter) = args.name {
                if id != filter {
                    continue;
                }
            }
            let liveness = classify_liveness(session, now);
            let idle = idle_for(session, now);
            if !args.all && is_receded(liveness, idle) {
                continue;
            }
            if args.needs_you && attention_key(liveness, idle).0 != 0 {
                continue;
            }
            if let Some(ref status) = args.status {
                if liveness_token(liveness) != status.as_str() {
                    continue;
                }
            }
            out.push(id.clone());
        }
        out.sort();
        out
    }

    fn dashboard_args(needs_you: bool, all: bool, status: Option<&str>) -> DashboardArgs {
        DashboardArgs {
            name: None,
            once: true,
            interval: None,
            status: status.map(|s| s.to_string()),
            needs_you,
            all,
        }
    }

    fn once_tree() -> SessionTree {
        let mut tree = SessionTree::new();
        // blocked (needs-you), active, done (receded).
        tree.sessions.insert(
            "s-blocked".to_string(),
            once_session(Some("work-on"), Some("build"), None, false, true),
        );
        tree.sessions.insert(
            "s-active".to_string(),
            once_session(Some("work-on"), Some("implement"), None, false, false),
        );
        tree.sessions.insert(
            "s-done".to_string(),
            once_session(Some("work-on"), Some("done"), None, true, false),
        );
        tree
    }

    #[test]
    fn once_default_excludes_receded() {
        let tree = once_tree();
        let ids = surviving_ids(&dashboard_args(false, false, None), &tree);
        assert!(
            !ids.contains(&"s-done".to_string()),
            "done is receded by default"
        );
        assert!(ids.contains(&"s-blocked".to_string()));
        assert!(ids.contains(&"s-active".to_string()));
    }

    #[test]
    fn once_all_includes_receded() {
        let tree = once_tree();
        let ids = surviving_ids(&dashboard_args(false, true, None), &tree);
        assert!(
            ids.contains(&"s-done".to_string()),
            "--all includes receded"
        );
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn once_status_blocked_restricts_output() {
        let tree = once_tree();
        let ids = surviving_ids(
            &dashboard_args(false, false, Some("needs-you-blocked")),
            &tree,
        );
        assert_eq!(ids, vec!["s-blocked".to_string()]);
    }

    #[test]
    fn once_needs_you_keeps_only_needs_you_band() {
        let tree = once_tree();
        let ids = surviving_ids(&dashboard_args(true, false, None), &tree);
        assert_eq!(ids, vec!["s-blocked".to_string()]);
    }
}
