# Lead: Daemon vs. ad-hoc invocation model

## Findings

### 1. Current Process Lifecycle & Signal Handling

koto implements graceful shutdown for long-running workflows using POSIX signals:
- **Signal handlers already in place:** `/home/dgazineu/dev/niwaw/tsuku/tsukumogami/public/koto/src/cli/mod.rs` lines 2334-2341 register `SIGTERM` and `SIGINT` handlers via the `signal-hook` crate (v0.3).
- **Shutdown mechanism:** `Arc<AtomicBool>` flag checked inside the advancement loop (`engine/advance.rs`, line 82: `SignalReceived` stop reason). The loop halts cleanly when a signal arrives, allowing the engine to finalize state writes before exit.
- **One-shot command model:** Every `koto next` / `koto init` / etc. invocation is a discrete process that exits after completing its work. No daemon infrastructure exists.
- **Synchronous-only:** The entire codebase is synchronous (`std::sync` primitives only). `Cargo.toml` imports zero async runtimes (no `tokio`, `async-std`, etc.). This is by design: action execution, gate evaluation, and template processing all use blocking I/O with configurable timeouts.

### 2. Session Discovery & File Layout

- **Stable, predictable paths:** Sessions live at `~/.koto/sessions/<repo-id>/<name>/koto-<name>.state.jsonl`, derived from canonicalized working directory hash (`session/local.rs`).
- **fsync after every write:** State files are opened with append-only semantics and fsynced after each event (`engine/persistence.rs` pattern). Safe for concurrent tail-ing or file watching without corruption risk.
- **Parent discovery:** `SessionInfo` struct (accessible via `backend.list()`) carries parent_workflow reference. Tree reconstruction requires scanning all sessions and grouping by parent pointer. For 1000-sibling batches, this is O(n) per discovery.
- **No watch infrastructure:** The `session/local.rs` module implements `list()` via `fs::read_dir()`. No file watcher, inotify wrapper, or file-watch abstraction layer exists. Discovery is synchronous, directory-based polling.
- **Session state mutation via flock:** Advisory file locking (`flock` on Unix) gates concurrent state transitions at the session level. Lock is acquired per `koto next` invocation (non-blocking; fails if locked). This is the mechanism that prevents simultaneous advancement of batch-scoped parents.

### 3. Event Log Structure & Query Patterns

- **JSONL format, event-sourced:** State files are append-only logs of typed events. Header line carries schema version, workflow name, template hash, creation timestamp. Subsequent lines are newline-delimited JSON events with sequence numbers.
- **Tier 1 events sufficient for basic dashboard:** `workflow_initialized`, `transitioned`, `evidence_submitted`, `evidence_rejected`, `action_executed`, `action_skipped` cover lifecycle display. No live-update latency penalty for basic state + directive display.
- **Tier 2 events exist but require scanning:** `gate_evaluated` events (Tier 2) carry per-gate results, but no aggregated "all gates passed/failed" summary. Dashboards must scan events per state to compute last gate status. File I/O is cheap (files are KB-sized), but adds client-side logic.
- **No `workflow_completed` event:** Terminal state is inferred by comparing `transitioned.to` against the compiled template's list of terminal states. Requires template coupling. Alternative: timeout heuristic (session hasn't advanced in N hours).

### 4. Comparable Rust CLI Daemon Patterns

Three patterns observed in production Rust CLI tools:

#### Pattern A: Embedded HTTP Server (actix-web, axum)
- Examples: `cargo-watch`, `cargo-expand` (some versions), Rust language servers (rust-analyzer)
- **Pros:** Single binary, browser UI accessible, clean separation of rendering logic.
- **Cons:** Introduces async runtime dependency (tokio or actix), complicates architecture. koto would need to refactor sync codebase to async. Adds 2-3 crates; binary bloat.
- **Invocation:** `koto daemon start` (or `--serve`) spawns background server at `localhost:PORT`, prints URL, optionally opens browser. `koto daemon stop` kills it. Requires systemd/launchd for user-level service registration.

#### Pattern B: Ad-hoc TUI (ratatui, Crossterm)
- Examples: `lsd` (ls clone with colors), `gitui`, `bottom` (system monitor).
- **Pros:** Single binary, zero dependencies (ratatui is pure Rust, no async). Full-screen terminal UI. Discoverable via `koto dashboard <name>` or `koto dashboard --all`.
- **Cons:** No networking, no browser; limited to same machine + terminal. Can't be headless. Large batch widths (1000 siblings) require aggregation + drill-down UI patterns.
- **Invocation:** `koto dashboard` or `koto dashboard <name>` opens ratatui TUI, polls filesystem for updates, exits on `q` or Ctrl+C. No background process.

#### Pattern C: Systemd/Launchd Service with IPC (Unix socket, HTTP)
- Examples: `systemd --user`, `Docker daemon`, some observability agents.
- **Pros:** True always-on observability; can be registered per-user via systemd user units. Socket-based IPC is lightweight. Decouple dashboard UI from session state engine.
- **Cons:** Requires systemd/launchd availability (some dev environments, cloud shells lack this). Adds setup complexity (install unit files, `systemctl --user enable`). Two binaries (daemon + CLI) or one binary with subcommands.
- **Invocation:** `koto daemon start` registers user service, restarts on reboot. `koto daemon stop` disables. `koto dashboard` connects to socket, fetches live state. Session discovery is daemon-side (always up-to-date).

### 5. Distribution & Installation Implications

- **Single-binary ad-hoc (TUI):** Install via `curl | bash` script (current koto pattern). No additional setup. Users run `koto dashboard` when they need visibility; exits when done.
- **Daemon + Web:** Install binary + systemd unit file or launchd plist. Requires OS-specific packaging. User runs `systemctl --user enable --now koto-daemon` or `brew services start koto-daemon`. Startup is implicit (systemd handles it); discovery is transparent.
- **Daemon + CLI IPC:** Single binary with `koto daemon start/stop` subcommands. User controls lifecycle explicitly. Requires IPC library (Unix socket abstraction).

### 6. Session Lifetime & Observability Requirements

- **Long-running workflows:** Batch tasks can run for hours (AI coding agent loops). User submits task list to coordinator at `t=0`, agent polls `koto next` every 10-60s, child workflows execute concurrently. Coordinator waits on `children-complete` gate.
- **Observability gap:** During this time, user has zero visibility into which children are running, which are blocked, which have failed (except by manually running `koto session list` and `koto next` on each child). Dashboard fills this gap.
- **Polling vs. always-on trade-off:**
  - Ad-hoc TUI: Polls filesystem every 500ms (configurable), displays current snapshot. Closes when user exits. No ongoing resource cost when dashboard isn't running.
  - Daemon: Always watching, pre-computed aggregations available instantly. Minimal memory overhead (session list is O(n) in count, not size). Resource cost is ~1-2 MB for daemon process at idle.

### 7. Process Group Isolation & Long-Running Actions

- **Action execution:** Gate evaluation and default actions spawn shell commands in isolated process groups (`setpgid(0,0)`, `action.rs` line 44). Timeouts are enforced per-command via `wait_timeout` crate. Process groups allow killing runaway subshells without affecting koto itself.
- **Signal safety:** If koto receives SIGTERM while a gate command is running, the signal handler sets the atomic flag; the next loop iteration checks it and returns `SignalReceived`. The gate command itself is killed by the process group cleanup in `CommandOutput` error handling.
- **No async I/O:** All timeouts use wall-clock time (via `wait_timeout::ChildExt::wait_timeout()`), not reactor-based async. This is appropriate for CLI usage where latency is measured in seconds, not milliseconds.

### 8. Dependency Impact

Current Cargo.toml includes:
- `signal-hook = "0.3"` for signal registration (already present).
- `wait-timeout = "0.2"` for process timeouts (already present).
- **Adding web:** Would require `tokio`, `actix-web` or `axum` (~500 KB binary size delta), plus HTTP routing crate.
- **Adding TUI:** Would require `ratatui`, `crossterm` (~400 KB binary size delta, pure Rust, no external dependencies).
- **Adding IPC daemon:** Would require Unix socket abstraction (e.g., `tokio-uds` or homegrown), minimal size impact.

### 9. Ecosystem Patterns: How Other Tools Do This

- **`cargo-watch`** (Rust CLI tool): Spawns a `cargo build` subprocess on file changes. Uses `notify` crate for FS events. No daemon; ad-hoc watcher invoked per session.
- **`rust-analyzer`** (Language server): Tokio-based daemon listening on stdin/stdout (LSP protocol). Single binary, invoked per editor instance, but stays alive for the editor session.
- **`systemd --user`**: Always-on per-user daemon. Manages other services. Can be introspected via `systemctl --user` CLI.
- **`ps` / `top`** analogy: `ps` lists processes (ad-hoc read), `systemd` is the manager (always-on). Dashboard could be either.

## Implications

### For the PRD spec:

1. **Invocation model is an architectural fork:** Choosing between ad-hoc TUI vs. daemon+web determines:
   - Binary size (TUI adds ~400 KB, web adds ~1 MB).
   - Installation complexity (TUI is zero-setup, daemon requires systemd/launchd or manual startup).
   - Rendering strategy (TUI is rasterized terminal, web is browser-based).
   - Dependency risk (TUI is pure Rust, web couples to tokio).

2. **Ad-hoc TUI is the "safer" MVP:** Lowest risk, zero new dependencies (ratatui is vendored widely), no daemon lifecycle management. User runs `koto dashboard` when needed, gets full-screen terminal UI, polls filesystem every 500ms. Good for initial observability.

3. **Daemon + web is the "observability-first" design:** True always-on state tracking, browser UI (familiar paradigm), easily extensible (HTTP API). Requires systemd/launchd setup or manual `koto daemon start` on session startup. Better for long-running batch workflows where visibility is critical.

4. **Hybrid option:** Implement ad-hoc TUI first (low risk), provide systemd unit file + daemon subcommand as optional beta feature. Users can opt-in to always-on mode without affecting default behavior.

### Architecture for daemon mode (if chosen):

- Single koto binary with `daemon` and `dashboard` subcommands.
- Daemon spawns at `~/.koto/daemon.sock` (Unix socket) and listens for HTTP requests over that socket.
- Daemon maintains in-memory session cache (rebuilt from `~/.koto/sessions/` on startup). Watches `~/.koto/sessions/` for new/updated state files using inotify or polling.
- Dashboard CLI (`koto dashboard`) or web UI connects to socket, queries live state, renders view.
- `koto daemon start`: Forks to background (or registers systemd service), writes PID to `~/.koto/daemon.pid`, returns immediately.
- `koto daemon stop`: Kills daemon or disables systemd service.
- **No tokio required:** Use `std::thread` for background watcher thread, synchronous socket handling (libunix or homegrown), no async runtime. Keeps dependencies minimal.

## Surprises

1. **Signal handling already exists:** koto has production-grade signal handling for clean shutdown (`signal_hook` + atomic flag). This is not greenfield; the infrastructure is ready for a daemon to use it.

2. **No async runtime in the entire codebase:** This is a deliberate design choice, not an oversight. Implication: adding a daemon doesn't automatically require tokio. A threaded daemon with synchronous socket I/O is viable and keeps the codebase simple.

3. **File locking is already session-aware:** The flock mechanism gates concurrent advancement of batch parents. A daemon would need to respect this lock to avoid races (don't mutate a parent's state while a child is advancing).

4. **Batch workflows are inherently long-running:** The dominant use case (1000-sibling task fan-out) runs for hours. Current `koto next` polling model leaves users blind. Dashboard (whether ad-hoc or daemon) is not luxury; it's the observability layer for this use case.

## Open Questions

1. **Should the daemon be always-on or lazy-start?** Systemd user services can use `Type=notify` and socket activation, so the daemon only starts when a dashboard client connects. Reduces idle resource cost. Complexity: requires systemd-specific setup.

2. **How large are in-memory session caches?** With 10 repos × 100 active workflows × 100 KB state file each, we're looking at ~100 MB. Acceptable but measurable. Daemon should be designed to allow cache eviction or pruning.

3. **What's the browser UI story?** If we choose daemon+web, the HTTP API and HTML/JS rendering are in scope. Simple: JSON API serving session list + event log; JS frontend does rendering. Complex: real-time updates via WebSocket or Server-Sent Events.

4. **Terminal vs. headless:** Ad-hoc TUI requires a terminal. In headless environments (GitHub Actions, CI), dashboards wouldn't work. Daemon + HTTP API is more flexible. Should the spec require headless support?

5. **Multi-user scenario:** If koto scales to team-shared workflows (future), should the daemon be system-wide or per-user? Current design assumes per-user sessions (`~/.koto/sessions/`). Daemon should probably be per-user service (`systemctl --user`).

6. **Backward compatibility:** If we ship ad-hoc TUI first and add daemon later, existing scripts/workflows won't break. But if we ship daemon-only, every user must manage the service. Which is the safer progression?

## Summary

koto has production-grade signal handling and file I/O primitives for clean shutdown, but zero daemon infrastructure today. Ad-hoc TUI (ratatui) is the lower-risk MVP with no new async dependencies; daemon+web is architecturally superior for always-on observability of long-running batch workflows but requires systemd/launchd setup and browser stack. Signal handling already exists, so a daemon can be built synchronously without tokio, keeping the architecture simple. The key decision is whether observability should be on-demand (TUI) or always-on (daemon), which determines scope, distribution complexity, and rendering strategy.

