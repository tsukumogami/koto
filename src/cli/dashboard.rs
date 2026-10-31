//! Entry point for the `koto dashboard` command.
//!
//! Wires the data layer, state machine, and renderer into a tick loop with
//! RAII terminal cleanup and optional `--once` scripting output.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;
use crossterm::{
    event::{self, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, buffer::Buffer, Terminal};

use crate::cli::dashboard_data::liveness::{attention_key, idle_for, is_receded};
use crate::cli::dashboard_data::{
    self, classify_liveness, compute_elapsed_since, CachedSession, Liveness, SessionTree,
};
use crate::cli::dashboard_render::render_frame;
use crate::cli::dashboard_state::{DashboardAppState, DashboardTab, SessionLoad};
use crate::cli::DashboardArgs;
use crate::session::remote_feed::{RemoteFeed, RESCAN_INTERVAL};
use crate::session_view::{KeyPreview, SessionViewSnapshot, SnapshotError};

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

/// The remote rows `--once` prints under the cloud backend, read in one
/// synchronous pass over the bucket ([`RemoteReader::scan`]).
///
/// Sets each local row's `workspace` to its copy's prefix. A session this
/// machine holds a copy of is left to its local row (matched by
/// `session_id`, or by workspace and name when its summary can't be read),
/// a migrated source whose copy is listed or held here is hidden, and a
/// local row with no state file under this directory's own prefix is
/// dropped when the pass listed that session, so it shows once, from the
/// pass. A failed pass leaves the local rows standing and says on one
/// stderr line which class of failure it met at which host (and, when only
/// some workspaces failed, how many); rows listed as unknown and hidden
/// sources are counted in the tree for the stderr notes.
///
/// [`RemoteReader::scan`]: crate::session::remote_feed::RemoteReader::scan
fn once_remote_rows(
    cloud: &crate::session::cloud::CloudBackend,
    tree: &mut SessionTree,
) -> Vec<(String, CachedSession)> {
    use crate::session::remote_feed::RemoteReader;

    let own = cloud.workspace_prefix().to_string();
    let mut placeholders = std::collections::HashSet::new();
    for (name, session) in tree.sessions.iter_mut() {
        session.workspace = Some(
            cloud
                .home_prefix(name)
                .map(|home| home.prefix)
                .unwrap_or_else(|_| own.clone()),
        );
        if !session.state_path.exists() {
            placeholders.insert(name.clone());
        }
    }

    let snapshot = match RemoteReader::new(&cloud.bucket_handle()) {
        // A placeholder has no local state file: it isn't a local copy.
        Ok(reader) => reader.scan(&dashboard_data::local_view_of(tree, |name| {
            !placeholders.contains(name)
        })),
        Err(e) => {
            eprintln!("note: other machines' sessions weren't read: {e}");
            return Vec::new();
        }
    };
    if let Some(failure) = &snapshot.failure {
        eprintln!(
            "note: other machines' sessions weren't {}read: {}",
            if failure.top_level { "" } else { "all " },
            failure.line()
        );
    }
    tree.remote_unreadable = snapshot.unreadable;
    tree.remote_hidden = snapshot.hidden_sources;

    for row in &snapshot.rows {
        if row.workspace == own && placeholders.contains(&row.name) {
            tree.sessions.remove(&row.name);
        }
    }
    snapshot
        .rows
        .iter()
        .map(|row| (row.name.clone(), dashboard_data::remote_session(row)))
        .collect()
}

/// Print the `--once` tab-separated feed.
///
/// Columns: `id, current_state, elapsed, status, intent, template` (the
/// original six, unchanged for positional compatibility) plus appended
/// `idle, liveness` (columns 7-8) and `machine, workspace` (columns 9-10).
/// `remote` holds the rows read from other machines' summaries; they are
/// filtered and ordered with the local rows. On the local backend (`cloud`
/// false) columns 9 and 10 are empty. Rows are emitted in attention order.
/// The receded set is excluded by default; `--all` includes it. `--status`
/// / `--needs-you` filter the rows. Appended fields are sanitized of tabs
/// and newlines so the tab-separated contract holds.
fn run_once(
    args: &DashboardArgs,
    tree: &SessionTree,
    remote: &[(String, CachedSession)],
    cloud: bool,
) {
    use std::time::SystemTime;

    let now = SystemTime::now();

    // Build (session_id, &session, liveness, idle) tuples, applying filters.
    let mut rows: Vec<(&str, &CachedSession, Liveness, std::time::Duration)> = Vec::new();
    let all = tree
        .sessions
        .iter()
        .chain(remote.iter().map(|(name, session)| (name, session)));
    for (session_id, session) in all {
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

    // Attention order; tie-break on session id, then workspace, for
    // determinism.
    rows.sort_by(|a, b| {
        attention_key(a.2, a.3)
            .cmp(&attention_key(b.2, b.3))
            .then_with(|| a.0.cmp(b.0))
            .then_with(|| a.1.workspace.cmp(&b.1.workspace))
    });

    for (session_id, session, liveness, idle) in rows {
        println!(
            "{}",
            format_once_line(session_id, session, liveness, idle, cloud)
        );
    }

    // Surface unparseable sessions as a trailing note on stderr so the
    // tab-separated stdout contract stays clean for positional readers.
    // Other machines' sessions without a readable, current summary are
    // listed as unknown rather than skipped, and join the same tally.
    let unreadable = tree.unreadable + tree.remote_unreadable;
    if tree.remote_unreadable > 0 {
        eprintln!(
            "note: {} unreadable session(s), {} listed as unknown",
            unreadable, tree.remote_unreadable
        );
    } else if unreadable > 0 {
        eprintln!("note: {} unreadable session(s) skipped", unreadable);
    }
    if tree.remote_hidden > 0 {
        eprintln!(
            "note: {} hidden source(s): imported into a copy that is listed",
            tree.remote_hidden
        );
    }
}

/// Build a single `--once` line for a session. First six columns match the
/// legacy positional contract; columns 7-8 are the appended `idle` and
/// `liveness`, and 9-10 the `machine` that produced the session and the
/// `workspace` id of its copy, both empty unless `cloud` (PRD R20).
/// Appended/interpolated fields are sanitized of tabs/newlines.
fn format_once_line(
    session_id: &str,
    session: &CachedSession,
    liveness: Liveness,
    idle: std::time::Duration,
    cloud: bool,
) -> String {
    let current_state = session.current_state.as_deref().unwrap_or("");
    let elapsed_secs = compute_elapsed_since(&session.header.created_at).as_secs();
    let elapsed = format_elapsed(elapsed_secs);
    let status_bucket = classify_status(session);
    let idle_str = format_elapsed(idle.as_secs());
    let liveness_str = liveness_token(liveness);
    let (machine, workspace) = if cloud {
        (
            session.machine.as_deref().unwrap_or(""),
            session.workspace.as_deref().unwrap_or(""),
        )
    } else {
        ("", "")
    };
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        sanitize_field(session_id),
        sanitize_field(current_state),
        elapsed,
        status_bucket,
        sanitize_field(session.intent.as_deref().unwrap_or("")),
        sanitize_field(session.header.template_name.as_deref().unwrap_or("")),
        sanitize_field(&idle_str),
        liveness_str,
        sanitize_field(machine),
        sanitize_field(workspace),
    )
}

/// `--detail`: print one JSON object describing the named session and exit.
///
/// Errors follow the `handle_status` convention — `{error, command}` on
/// stdout, exit 2 for a missing session, exit 3 for a corrupt record or an
/// infrastructure failure. A migrated session is a successful exit-0 answer
/// carrying `migrated {target, workspace}` and no `keys` member. Otherwise
/// the object holds the session facts (absent facts are omitted, not
/// nulled), every listed key in the snapshot's lexicographic order with its
/// metadata, and an eagerly read bounded `content` preview for at most
/// [`DETAIL_PREVIEW_CEILING`](crate::session_view::DETAIL_PREVIEW_CEILING)
/// keys; past the ceiling a key carries `contentOmitted: "key-limit"` and
/// the top-level `contentOmittedCount` counts them. Every string has been
/// through the snapshot layer's sanitizer, and serde_json's own escaping
/// runs after it, so the stream carries no raw control bytes.
fn run_detail(name: &str, backend: &crate::session::Backend) -> ! {
    use crate::cli::{exit_with_error_code, EXIT_CALLER_ERROR, EXIT_INFRASTRUCTURE};
    use crate::session_view::{
        self, KeyPreview, Locality, SnapshotError, DETAIL_PREVIEW_CEILING, EXCERPT_BOUND,
    };

    let snap = match session_view::snapshot(backend, name) {
        Ok(s) => s,
        Err(SnapshotError::NotFound) => {
            exit_with_error_code(
                serde_json::json!({
                    "error": format!(
                        "session '{}' not found",
                        session_view::sanitize_capped(name, session_view::KEY_DISPLAY_CAP)
                    ),
                    "command": "dashboard",
                }),
                EXIT_CALLER_ERROR,
            );
        }
        Err(e @ (SnapshotError::Corrupt(_) | SnapshotError::Infrastructure(_))) => {
            exit_with_error_code(
                serde_json::json!({
                    "error": e.reason().as_str(),
                    "command": "dashboard",
                }),
                EXIT_INFRASTRUCTURE,
            );
        }
    };

    // The shape is assembled by hand, so nothing at compile time holds it:
    // `tests/dashboard_detail_test.rs` and the schema in
    // `docs/reference/session-feed.md` are the contract. A field change
    // updates both in the same commit.
    let mut out = serde_json::Map::new();
    let facts = &snap.facts;
    out.insert("name".into(), facts.name.clone().into());
    let mut put = |k: &str, v: Option<String>| {
        if let Some(v) = v {
            out.insert(k.into(), v.into());
        }
    };
    put(
        "session_id",
        facts.session_id.clone().map(|s| s.into_string()),
    );
    put("created_at", facts.created_at.clone());
    put("intent", facts.intent.clone().map(|s| s.into_string()));
    put(
        "template_name",
        facts.template_name.clone().map(|s| s.into_string()),
    );
    put(
        "current_state",
        facts.current_state.clone().map(|s| s.into_string()),
    );
    put(
        "directive",
        facts.directive.clone().map(|s| s.into_string()),
    );
    put(
        "execution_dir",
        facts.execution_dir.clone().map(|s| s.into_string()),
    );
    put("anchor", facts.anchor.clone().map(|s| s.into_string()));
    if let Some((kind, base)) = &facts.store {
        out.insert(
            "store".into(),
            serde_json::json!({"kind": kind.as_str(), "base": base.as_str()}),
        );
    }
    // The one fact printed as `null` rather than left out: a session that
    // records no machine says so.
    out.insert(
        "machine".into(),
        facts
            .machine
            .as_ref()
            .map_or(serde_json::Value::Null, |m| m.as_str().into()),
    );

    if let Some(m) = &snap.migrated {
        // A migrated session is an answer, not an error: the successor is
        // named and no key list follows (the local copy's keys describe a
        // session that lives elsewhere now).
        out.insert(
            "migrated".into(),
            serde_json::json!({"target": m.target.as_str(), "workspace": m.workspace.as_str()}),
        );
        println!("{}", serde_json::Value::Object(out));
        std::process::exit(0);
    }

    let mut keys = Vec::with_capacity(snap.keys.len());
    let mut omitted: u64 = 0;
    for (i, entry) in snap.keys.iter().enumerate() {
        let mut k = serde_json::Map::new();
        k.insert("key".into(), entry.display.as_str().into());
        if let Some(size) = entry.size {
            k.insert("size".into(), size.into());
        }
        if let Some(w) = &entry.writer {
            k.insert("writer".into(), w.as_str().into());
        }
        if let Some(c) = &entry.created_at {
            k.insert("created_at".into(), c.as_str().into());
        }
        k.insert(
            "locality".into(),
            match entry.locality {
                Locality::Local => "local",
                Locality::RemoteOnly => "remote-only",
            }
            .into(),
        );
        k.insert("stale".into(), entry.stale.into());

        if i >= DETAIL_PREVIEW_CEILING {
            omitted += 1;
            k.insert("contentOmitted".into(), "key-limit".into());
        } else {
            let preview = match &entry.raw {
                Some(raw) => session_view::read_key_preview(backend, name, raw, EXCERPT_BOUND),
                None => KeyPreview::InvalidName,
            };
            let content = match preview {
                KeyPreview::Text { excerpt, truncated } => serde_json::json!({
                    "type": "text", "excerpt": excerpt.as_str(), "truncated": truncated,
                }),
                KeyPreview::Binary { size, hash } => serde_json::json!({
                    "type": "binary", "size": size,
                    "hash": hash.map_or_else(|| "invalid-hash".to_string(), |h| h.into_string()),
                }),
                KeyPreview::NotLocal { size, hash } => serde_json::json!({
                    "type": "not-local", "size": size,
                    "hash": hash.map_or_else(|| "invalid-hash".to_string(), |h| h.into_string()),
                }),
                KeyPreview::Unreadable { reason } => serde_json::json!({
                    "type": "unreadable", "reason": reason.as_str(),
                }),
                KeyPreview::InvalidName => serde_json::json!({"type": "invalid-name"}),
            };
            k.insert("content".into(), content);
        }
        keys.push(serde_json::Value::Object(k));
    }
    out.insert("keys".into(), keys.into());
    out.insert("contentOmittedCount".into(), omitted.into());
    out.insert("unlistedKeys".into(), (snap.unlisted_keys as u64).into());

    println!("{}", serde_json::Value::Object(out));
    std::process::exit(0);
}

/// Entry point called from the CLI dispatch in `src/cli/mod.rs`.
///
/// Takes the concrete [`Backend`]: the Session tab and the `--detail`
/// mode reach the context store and the migration check through it
/// (`crate::session_view`), which no trait object carries. The TUI shares
/// it with the Session tab's worker thread, so it is taken by value.
///
/// In the TUI the input and render thread makes no bucket request: it
/// lists this machine's sessions from the local store only, the remote
/// rows arrive from a [`RemoteFeed`] thread, and the Session tab's reads
/// run on a [`SessionLoader`] thread.
///
/// [`Backend`]: crate::session::Backend
pub fn run(args: DashboardArgs, backend: crate::session::Backend) -> Result<()> {
    use crate::session::Backend;

    // --detail: one JSON object for one session, checked before --once and
    // the TUI so it can never fall into either. clap's `requires` guarantees
    // the positional name is present.
    if args.detail {
        let name = args
            .name
            .as_deref()
            .expect("clap `requires` guarantees the positional name");
        run_detail(name, &backend);
    }

    // rust-s3 asks every failed request again on its own, a second later,
    // through one process-wide setting with no per-bucket form. The reader
    // decides itself what to ask again (a throttle, never a timeout), so in
    // this process, which only reads, rust-s3 asks once: a bucket that
    // never answers costs a pass one request timeout.
    s3::set_retries(0);

    // --once: print tab-separated lines and exit without a TUI.
    if args.once {
        // The local store only, as the TUI reads it: under the cloud
        // backend the listing it wraps asks the bucket with no request
        // timeout. The pass below reads the bucket.
        let mut tree = SessionTree::new();
        match &backend {
            Backend::Cloud(cloud) => dashboard_data::refresh(&mut tree, cloud.local())?,
            Backend::Local(store) => dashboard_data::refresh(&mut tree, store)?,
        }
        let remote = match &backend {
            Backend::Cloud(cloud) => once_remote_rows(cloud, &mut tree),
            Backend::Local(_) => Vec::new(),
        };
        let cloud = matches!(backend, Backend::Cloud(_));
        run_once(&args, &tree, &remote, cloud);
        return Ok(());
    }

    // TUI mode: set up terminal, register signal handler, run tick loop.
    let poll_interval_ms = args.interval.unwrap_or(500);
    let mut state = DashboardAppState::new(poll_interval_ms);
    let backend = Arc::new(backend);

    // The local store only: under the cloud backend the listing it wraps
    // adds the bucket's sessions with a LIST per call, which this thread
    // never makes. Sessions found only in the bucket come from the feed.
    let local = Arc::clone(&backend);
    let mut refresh_local: LocalRefresh = Box::new(move |tree| match &*local {
        Backend::Cloud(cloud) => dashboard_data::refresh(tree, cloud.local()),
        Backend::Local(store) => dashboard_data::refresh(tree, store),
    });

    // The first local read comes before the feed starts, so its first scan
    // already leaves this machine's own sessions to their local rows.
    refresh_local(&mut state.tree)?;
    let feed = match &*backend {
        Backend::Cloud(cloud) => match RemoteFeed::spawn(
            &cloud.bucket_handle(),
            dashboard_data::local_view(&state.tree),
            RESCAN_INTERVAL,
        ) {
            Ok(feed) => Some(feed),
            Err(e) => {
                eprintln!("note: other machines' sessions won't be read: {e}");
                None
            }
        },
        Backend::Local(_) => None,
    };
    let mut sources = LoopSources {
        refresh_local,
        feed,
        sessions: Some(SessionLoader::spawn(Arc::clone(&backend))?),
    };

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

    // Quitting returns from here and the process exits; the feed and
    // Session tab threads are never joined, so a request that never
    // answers can't hold the exit.
    run_loop(
        &mut terminal,
        &mut state,
        &mut sources,
        &mut TerminalEvents,
        &SystemClock,
        &shutdown,
        &mut |_, _, _| {},
    )
}

/// How long the loop waits for input on each tick.
pub const TICK: Duration = Duration::from_millis(50);

/// Where the dashboard loop reads terminal events from.
pub trait EventSource {
    /// The next event, waiting at most `timeout`; `None` when none came.
    fn next_event(&mut self, timeout: Duration) -> io::Result<Option<Event>>;
}

/// The terminal's own events, read through crossterm.
pub struct TerminalEvents;

impl EventSource for TerminalEvents {
    fn next_event(&mut self, timeout: Duration) -> io::Result<Option<Event>> {
        if event::poll(timeout)? {
            Ok(Some(event::read()?))
        } else {
            Ok(None)
        }
    }
}

/// The time the loop stamps each drawn frame with.
pub trait Clock {
    /// Now.
    fn now(&self) -> Instant;
}

/// The monotonic system clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Re-reads this machine's sessions into the tree.
pub type LocalRefresh = Box<dyn FnMut(&mut SessionTree) -> Result<()>>;

/// What the dashboard loop reads besides the terminal. None of it makes a
/// request on the loop's thread.
pub struct LoopSources {
    /// Re-read this machine's sessions into the tree, from the local store
    /// only.
    pub refresh_local: LocalRefresh,
    /// Other machines' sessions, read on the feed's own thread; `None` on
    /// the local backend.
    pub feed: Option<RemoteFeed>,
    /// The Session tab's reads, on the loader's own thread; `None` leaves
    /// the tab loading.
    pub sessions: Option<SessionLoader>,
}

/// Run the dashboard until a quit key or `shutdown`, drawing into
/// `terminal`. Each tick waits up to [`TICK`] for one event from `events`,
/// refreshes local rows every `poll_every_n_ticks`, swaps in the feed's
/// newest snapshot whole, takes the Session tab's answers, and draws;
/// `on_frame` sees each frame with the time `clock` gives for it.
pub fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    state: &mut DashboardAppState,
    sources: &mut LoopSources,
    events: &mut dyn EventSource,
    clock: &dyn Clock,
    shutdown: &AtomicBool,
    on_frame: &mut dyn FnMut(Instant, &Buffer, &DashboardAppState),
) -> Result<()> {
    refresh_local_rows(state, sources)?;
    loop {
        if let Some(Event::Key(key)) = events.next_event(TICK)? {
            state.handle_key(key);
        }

        state.tick_count += 1;
        if state.tick_count >= state.poll_every_n_ticks {
            refresh_local_rows(state, sources)?;
            state.tick_count = 0;
        }

        // The feed hands over complete snapshots; the remote rows are
        // replaced whole, never patched, except a failed listing's, which
        // stay as they were.
        if let Some(snapshot) = sources.feed.as_ref().and_then(RemoteFeed::latest) {
            dashboard_data::apply_snapshot(&mut state.tree, &snapshot);
            state.clamp_cursor();
        }

        // Refresh the detail cache for the focused session on every tick (not
        // only on poll ticks) so opening a session shows its data at once; the
        // session/mtime guard keeps this to a map lookup when nothing changed.
        // A remote row reads nothing.
        state.refresh_detail(dashboard_data::read_detail);

        if let Some(loader) = sources.sessions.as_mut() {
            load_session_tab(state, loader);
        }

        if state.should_quit || shutdown.load(Ordering::SeqCst) {
            break;
        }

        let frame = terminal.draw(|f| render_frame(f, state))?;
        on_frame(clock.now(), frame.buffer, state);
    }

    Ok(())
}

/// Re-read local rows and tell the feed which sessions they hold.
fn refresh_local_rows(state: &mut DashboardAppState, sources: &mut LoopSources) -> Result<()> {
    (sources.refresh_local)(&mut state.tree)?;
    state.clamp_cursor();
    if let Some(feed) = &sources.feed {
        feed.set_local(dashboard_data::local_view(&state.tree));
    }
    Ok(())
}

/// Take the Session tab worker's answers that still match what the tab
/// shows, then ask for what its plan says is stale. Only a local row loads:
/// a remote row's Session tab is drawn from its summary.
fn load_session_tab(state: &mut DashboardAppState, loader: &mut SessionLoader) {
    let focused = state.focused_id.clone().and_then(|id| {
        let s = state.tree.sessions.get(&id)?;
        (!s.remote).then_some((id, s.mtime))
    });
    for answer in loader.answers() {
        let Some((id, mtime)) = focused.as_ref() else {
            continue;
        };
        match answer {
            SessionAnswer::Snapshot {
                session,
                mtime: at,
                result,
            } if session == *id
                && at == *mtime
                && state.session_tab.plan(id, *mtime) == SessionLoad::Snapshot =>
            {
                state.session_tab.apply_snapshot(id, *mtime, *result);
            }
            SessionAnswer::Preview {
                session,
                key,
                mtime: at,
                preview,
            } if session == *id
                && at == *mtime
                && state.session_tab.plan(id, *mtime) == SessionLoad::Preview(key.clone()) =>
            {
                state.session_tab.apply_preview(id, key, *mtime, preview);
            }
            _ => {}
        }
    }
    if state.active_tab != DashboardTab::Session {
        return;
    }
    let Some((session, mtime)) = focused else {
        return;
    };
    match state.session_tab.plan(&session, mtime) {
        SessionLoad::Snapshot => loader.ask(SessionRequest::Snapshot { session, mtime }),
        SessionLoad::Preview(key) => loader.ask(SessionRequest::Preview {
            session,
            key,
            mtime,
        }),
        SessionLoad::Idle => {}
    }
}

/// A read the Session tab asks its worker for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SessionRequest {
    Snapshot {
        session: String,
        mtime: SystemTime,
    },
    Preview {
        session: String,
        key: String,
        mtime: SystemTime,
    },
}

/// The worker's answer to a [`SessionRequest`].
enum SessionAnswer {
    Snapshot {
        session: String,
        mtime: SystemTime,
        result: Box<Result<SessionViewSnapshot, SnapshotError>>,
    },
    Preview {
        session: String,
        key: String,
        mtime: SystemTime,
        preview: KeyPreview,
    },
}

impl SessionAnswer {
    /// The request this answers.
    fn request(&self) -> SessionRequest {
        match self {
            SessionAnswer::Snapshot { session, mtime, .. } => SessionRequest::Snapshot {
                session: session.clone(),
                mtime: *mtime,
            },
            SessionAnswer::Preview {
                session,
                key,
                mtime,
                ..
            } => SessionRequest::Preview {
                session: session.clone(),
                key: key.clone(),
                mtime: *mtime,
            },
        }
    }
}

/// The Session tab's reads (the snapshot, which under the cloud backend
/// can probe the bucket and fetch its manifest, and the focused key's
/// preview), run on a thread of their own so the loop never waits on them.
/// The tab says "Loading session" until the answer comes. Dropping the
/// handle ends the thread once its current read returns; nothing waits.
pub struct SessionLoader {
    requests: Sender<SessionRequest>,
    answers: Receiver<SessionAnswer>,
    in_flight: Option<SessionRequest>,
}

impl SessionLoader {
    /// Start the worker on `backend`.
    pub fn spawn(backend: Arc<crate::session::Backend>) -> io::Result<Self> {
        let (requests, inbox) = mpsc::channel::<SessionRequest>();
        let (outbox, answers) = mpsc::channel();
        std::thread::Builder::new()
            .name("koto-dashboard-session".to_string())
            .spawn(move || {
                for request in inbox {
                    let answer = match request {
                        SessionRequest::Snapshot { session, mtime } => SessionAnswer::Snapshot {
                            result: Box::new(crate::session_view::snapshot(&backend, &session)),
                            session,
                            mtime,
                        },
                        SessionRequest::Preview {
                            session,
                            key,
                            mtime,
                        } => SessionAnswer::Preview {
                            preview: crate::session_view::read_key_preview(
                                &backend,
                                &session,
                                &key,
                                crate::session_view::EXCERPT_BOUND,
                            ),
                            session,
                            key,
                            mtime,
                        },
                    };
                    if outbox.send(answer).is_err() {
                        return;
                    }
                }
            })?;
        Ok(Self {
            requests,
            answers,
            in_flight: None,
        })
    }

    /// Ask for `request` unless it is the one already in flight.
    fn ask(&mut self, request: SessionRequest) {
        if self.in_flight.as_ref() == Some(&request) {
            return;
        }
        if self.requests.send(request.clone()).is_ok() {
            self.in_flight = Some(request);
        }
    }

    /// Every answer that has arrived, oldest first. Never blocks.
    fn answers(&mut self) -> Vec<SessionAnswer> {
        let mut out = Vec::new();
        while let Ok(answer) = self.answers.try_recv() {
            if self.in_flight.as_ref() == Some(&answer.request()) {
                self.in_flight = None;
            }
            out.push(answer);
        }
        out
    }
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
                command_environment: None,
                schema_version: 1,
                workflow: "test".to_string(),
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
                kind: None,
                root_session_id: None,
                parent_session_id: None,
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
            is_unreadable: false,
            is_batch_task: false,
            is_request_leg: false,
            machine: None,
            workspace: None,
            remote: false,
            imported_from: None,
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
    fn once_line_keeps_first_eight_columns_and_appends_machine_and_workspace() {
        let mut s = once_session(
            Some("work-on"),
            Some("implement"),
            Some("fix bug"),
            false,
            false,
        );
        s.machine = Some("machine-a".to_string());
        s.workspace = Some("0123456789abcdef".to_string());
        let line = format_once_line(
            "sess-1",
            &s,
            Liveness::Active,
            std::time::Duration::ZERO,
            true,
        );
        let cols: Vec<&str> = line.split('\t').collect();
        assert_eq!(
            cols.len(),
            10,
            "must emit exactly ten tab-separated columns"
        );
        assert_eq!(cols[8], "machine-a", "column 9 is the machine");
        assert_eq!(cols[9], "0123456789abcdef", "column 10 is the workspace");
        // The local backend prints both empty, whatever the row records.
        let local = format_once_line(
            "sess-1",
            &s,
            Liveness::Active,
            std::time::Duration::ZERO,
            false,
        );
        assert!(local.ends_with("\tactive\t\t"), "got: {local:?}");
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
        let line = format_once_line(
            "sess-2",
            &s,
            Liveness::Active,
            std::time::Duration::ZERO,
            false,
        );
        let cols: Vec<&str> = line.split('\t').collect();
        assert_eq!(
            cols.len(),
            10,
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
            detail: false,
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
