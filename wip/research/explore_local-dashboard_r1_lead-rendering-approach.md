# Lead: What rendering approach fits the local dashboard requirements?

## Findings

### Current Codebase State

**CLI Architecture**: koto uses `clap` 4.6 with a Subcommand-based architecture. The main command types are `init`, `next`, `cancel`, `rewind`, `status`, `workflows`, and subcommand groups for `session`, `context`, `template`, `config`, `decisions`, and `overrides` (see `/src/cli/mod.rs` lines 77-209). There is **no existing `watch`, `serve`, `dashboard`, or `observe` command**. The codebase is purely command-line oriented.

**Session Storage**: Sessions are stored at `~/.koto/sessions/<repo-id>/<workflow-name>/` using a `LocalBackend` struct (`/src/session/local.rs`). Sessions can optionally sync to S3-compatible backends via `CloudConfig`. The default backend is "local" (`/src/config/mod.rs` line 32).

**Event Log Data Access**: Session state is derived from JSONL files via `derive_state_from_log()` in `/src/engine/persistence.rs` (line 235). The session-feed contract (`/docs/reference/session-feed.md`) defines a header record (schema version, workflow name, template hash, session_id, parent_workflow, created_at) followed by typed events across 15 event types split into Tier 1 (user-facing: transitioned, directed_transition, evidence_submitted, workflow_cancelled) and Tier 2/3 (internal/audit). The log files are write-once-append and flushed with `sync_data()` after each write, making them suitable for tail/inotify monitoring.

**Existing Visualization**: koto ships export/render functionality for templates only — not for live sessions:
- `/src/export/html.rs`: Generates self-contained HTML with Cytoscape.js for static template graph visualization
- `/src/export/mermaid.rs`: Renders templates as Mermaid diagrams
- No runtime/session visualization exists; export is compile-time template analysis only

**Dependencies and Ecosystem**: The project has minimal UI/rendering dependencies. Current deps are: `clap` (CLI), `serde_json`, `serde_yaml_ng`, `opener` (open URLs), `tempfile`, `rust-s3`, signal-hook (Unix), no HTTP server, and no terminal UI library. For a dashboard, the project would need to add either ratatui/tui-rs, actix-web/axum, or use platform-specific native UI libraries.

### Rendering Approach Feasibility Analysis

**Terminal UI (ratatui/tui-rs)**:
- Integration: Straightforward; ratatui is a pure Rust library with no external dependencies. Can be added directly to Cargo.toml and invoked from a new `koto dashboard` or `koto watch` subcommand.
- Data Flow: The dashboard would tail the JSONL file at `~/.koto/sessions/<repo-id>/<name>/koto-<name>.state.jsonl`, parse Tier 1 events via the existing `Event` struct, and derive session state using existing `derive_state_from_log()` and `derive_machine_state()` functions.
- Distribution: Single binary, no external runtime or browser required. Ships with `koto` as-is.
- Session Discovery: Must either accept a session name argument, scan `~/.koto/sessions/<repo-id>/` for running sessions, or watch all sessions in a hierarchy.
- Constraints: Terminal-based UI limits formatting and interactivity; best for tree/list views with text output. Not suitable for rich graphs or interactive node inspection without significant complexity (mouse support, paging, etc.).

**Embedded Web Server (actix-web, axum, tiny-http)**:
- Integration: Adds HTTP server dependency. koto currently has no async runtime; embedding would require adding tokio or similar. Moderate complexity; new async infrastructure in an otherwise synchronous codebase.
- Data Flow: Server reads JSONL files from disk or via a file-watching integration. Serves REST API endpoints (`/sessions`, `/sessions/<id>/events`, `/sessions/<id>/state`) and static HTML/JS frontend. Real-time updates via WebSocket or polling.
- Distribution: Binary still shipped with koto. User runs `koto serve --port 8080`; browser opens to `http://localhost:8080`. Extra step vs. terminal UI.
- Session Discovery: Natural fit for web UX; can list all sessions, filter by parent, watch multiple hierarchies simultaneously.
- Constraints: Requires a browser; adds HTTP/WebSocket maintenance surface. But allows rich visualization (SVG/Canvas graphs, interactive drill-down, real-time updates without polling).

**Native Desktop UI (gtk-rs, iced, fltk-rs)**:
- Integration: gtk-rs requires GTK library; iced/fltk-rs are pure Rust but less mature. Adds significant dependency complexity and platform-specific build requirements.
- Data Flow: Same JSONL reading as terminal UI, but with native widgets and layout.
- Distribution: Platform-specific binaries; would require separate macOS/Windows/Linux releases. Not currently done for koto (see `.goreleaser.yaml` — no GUI binary targets).
- Session Discovery: Good UX for multi-window/multi-session views.
- Constraints: Highest distribution and maintenance burden; not justified for a local-only observability tool. The project has no GUI distribution infrastructure.

### Session Hierarchy and Data Display Requirements

The session-feed contract includes a `parent_workflow` field (header, nullable) and a `child_completed` event (Tier 2, with child_name, task_name, outcome, final_state). This enables:
- **Root-to-child tracking**: Parent sessions know their children; dashboards can display a tree of root → child → grandchild hierarchies.
- **Live state derivation**: Current state is always the `to` field of the last `transitioned`/`directed_transition`/`rewound` event (or null if not yet transitioned). Tier 1 events (transitioned, directed_transition, evidence_submitted, workflow_cancelled, gate_override_recorded, batch_finalized) are the user-facing audit trail. Tier 2/3 events (integration_invoked, context_added, default_action_executed, decision_recorded, gate_evaluated, child_completed, scheduler_ran) provide depth for detailed investigation.
- **Gate and evidence visibility**: `gate_override_recorded` and `gate_evaluated` (Tier 2) show gate decisions; `evidence_submitted` (Tier 1) shows what the agent submitted.

### What's Already in Place for Data Access

1. **JSONL Parsing**: The codebase has `read_events()` in `/src/engine/persistence.rs` (line 151) that reads all events from a state file.
2. **State Derivation**: Existing `derive_state_from_log()`, `derive_machine_state()`, and `derive_decisions()` functions are reusable.
3. **Session Discovery**: LocalBackend provides `list()` and `dir()` methods to enumerate sessions on disk.
4. **Event Validation**: The `validate_feed` CLI subcommand (`/src/cli/validate_feed.rs`) can parse and validate logs — shows that feed reading is already implemented.

No existing web server, no async runtime, no terminal UI library, and no watch/serve command pattern.

## Implications

1. **Terminal UI is the lowest-friction option for MVP**. It requires only ratatui (a pure Rust library) and can reuse all existing session/state reading infrastructure. The `koto watch` or `koto dashboard` command becomes a leaf feature in the existing CLI architecture. No HTTP, no async runtime, minimal new code.

2. **Web server approach unlocks richer UX but with higher integration cost**. It would require introducing tokio, WebSocket support, and a frontend build pipeline. This is justified if the dashboard needs to show complex session graphs, real-time multi-session monitoring, or drill-down inspection. Not essential for the MVP.

3. **Native desktop UI is out of scope** for distribution and maintainability reasons. koto has no desktop distribution story today.

4. **Session hierarchy viewing is data-ready**. The `parent_workflow` header field and `child_completed` events provide the relationships. Tier 1 events suffice for user-facing progress; Tier 2 events add depth for debugging.

5. **Live-update challenge is real**. JSONL files are append-only and flushed after each write. A dashboard can tail the file using platform-specific APIs (inotify on Linux, kqueue on macOS, ReadDirectoryChangesW on Windows), but this requires platform-specific code or a cross-platform file watcher library. Polling on a 1-second interval is simpler but less responsive.

6. **Session discovery UX must be explicit**. Without a clear invocation model (e.g., `koto dashboard <name>`, `koto dashboard --all`, or `koto dashboard --watch-dir ~/projects`), the PRD cannot specify the command interface. The scope doc identifies this as an open question.

## Surprises

1. **No existing watch/serve pattern**: Despite the roadmap mentioning F3, F5, and F6 dashboards, there is zero infrastructure for any of them. The codebase is purely command-line; even exporting templates to HTML is a static operation (`export template compile --output file.html`). This suggests the dashboard work is genuinely new.

2. **Template visualization ≠ session visualization**: The existing HTML/Mermaid export is for static template graphs (state machine structure), not for live session traces or runtime state. These are orthogonal concerns.

3. **Tier classification exists but isn't used**: The session-feed contract defines Tier 1 (user-facing), Tier 2 (detailed audit), and Tier 3 (internal scheduler), but the codebase has no Tier logic. A dashboard implementation would be the first consumer of this classification.

4. **No async runtime**: koto is entirely synchronous (even S3 I/O uses `attohttpc` with `sync-rustls-tls`). Adding a web server would be the first async code in the project.

## Open Questions

1. **What is the invocation model?** Does `koto dashboard <name>` watch a single session, or does `koto dashboard --all` watch a repo-level directory? Does the command block (watching in a loop) or return immediately after starting a background watcher?

2. **What is the update latency expectation?** Sub-second (requires file watcher), 1-second polling, or "good enough" opportunistic updates?

3. **How deep are real hierarchies?** Are parent-child relations typically 2 levels (root → child) or do you see grandchild hierarchies often? This affects whether a tree view or a flatter list with grouping is appropriate.

4. **Is multi-session viewing in scope for MVP?** Watching all sessions in a repo simultaneously, or just one named session?

5. **What level of detail is displayed at each hierarchy level?** Is it just (workflow_name, current_state, last_transition_time) or does it include gate results, evidence counts, etc.? The session-feed has all this data; the question is what's essential to display.

6. **Platform-specific file watching**: Is a cross-platform file-watcher dependency (notify, watchify) acceptable, or should the implementation be platform-native (inotify/kqueue) with fallback polling?

## Summary

Terminal UI (ratatui) is the lowest-friction rendering approach because koto can add it as a pure Rust dependency without introducing async infrastructure, HTTP server maintenance, or platform-specific GUI requirements. The session feed (F2 contract) provides all necessary data (session hierarchy, Tier 1 user-facing events, gate evaluations, evidence submissions), and existing session reading functions are reusable. However, the PRD must define the invocation model (single session vs. repository-wide watch), update latency expectations, and hierarchy visualization strategy before implementation can begin.

