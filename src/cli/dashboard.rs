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

use crate::cli::dashboard_data::{self, CachedSession, SessionTree};
use crate::cli::dashboard_render::render_frame;
use crate::cli::dashboard_state::{DashboardAppState, ViewMode};
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

/// Entry point called from the CLI dispatch in `src/cli/mod.rs`.
pub fn run(args: DashboardArgs, backend: &dyn SessionBackend) -> Result<()> {
    // --once: print tab-separated lines and exit without a TUI.
    if args.once {
        let mut tree = SessionTree::new();
        dashboard_data::refresh(&mut tree, backend)?;
        // Collect and sort all session IDs for deterministic output.
        let mut all_ids: Vec<&str> = tree.sessions.keys().map(|s| s.as_str()).collect();
        all_ids.sort();
        for session_id in all_ids {
            // Apply optional name filter.
            if let Some(ref filter) = args.name {
                if session_id != filter {
                    continue;
                }
            }
            if let Some(session) = tree.sessions.get(session_id) {
                let current_state = session.current_state.as_deref().unwrap_or("").to_string();
                let elapsed_secs = session.mtime.elapsed().unwrap_or(Duration::ZERO).as_secs();
                let elapsed = format_elapsed(elapsed_secs);
                let status_bucket = classify_status(session);
                println!(
                    "{}\t{}\t{}\t{}",
                    session_id, current_state, elapsed, status_bucket
                );
            }
        }
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
    execute!(io::stdout(), EnterAlternateScreen)?;
    // The guard restores the terminal on any exit path from this point forward,
    // including returns via `?` and panics.
    let _guard = TerminalGuard;

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

            // If in Detail mode, refresh the detail cache for the focused session.
            if let (ViewMode::Detail, Some(ref id)) = (&state.view_mode, &state.focused_id) {
                let path = state.tree.sessions.get(id).map(|s| s.state_path.clone());
                if let Some(path) = path {
                    state.detail_cache = dashboard_data::read_detail(&path, id);
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
            },
            current_state: current_state.map(|s| s.to_string()),
            is_terminal,
            is_blocked,
            mtime: SystemTime::UNIX_EPOCH,
            state_path: PathBuf::new(),
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
}
