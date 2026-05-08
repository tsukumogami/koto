# /prd Scope: local-dashboard

## Problem Statement

koto users running hours-long orchestration workflows — AI coding pipelines spanning multiple phases, batch jobs with hundreds of parallel tasks, and eventually full multi-skill sequences (explore → prd → design → plan → work-on) — have no live visibility into what's happening. The only tools available are `koto status <name>` and `koto workflows`, which produce static JSON snapshots and require manual re-invocation. When a batch job has 200 child sessions running in parallel, users are effectively blind: they can't see which children are running, which are blocked, which have failed, or what the current gate state is — without scripting their own polling. F3: Local Dashboard is the first tangible observability experience that addresses this gap, and the baseline that makes F5 (S3-backed) and F6 (relay) dashboards' remote value legible.

## Initial Scope

### In Scope

- Terminal UI dashboard (`koto dashboard [<name>]`) showing session hierarchy for the current koto repository
- Session hierarchy view: root sessions → batch coordinator sessions → sibling task sessions (up to ~3 levels deep, up to 1000 siblings per batch)
- Per-session display: workflow name, current state, elapsed time, is_terminal
- Batch coordinator display: aggregated task counts (total/success/failed/skipped/pending/blocked), phase (active/final)
- Gate evaluation display: most recent `gate_evaluated` event per state (Tier 2 events, client-side computation)
- Evidence submission display: `evidence_submitted` events shown as timeline entries
- Live updates via file polling (~500ms interval) — dashboard stays current while workflows run
- Session discovery: scans `~/.koto/sessions/<repo-id>/` for the current repository (repo-id derived from working directory hash)
- `koto dashboard` (repo-wide session list) and `koto dashboard <name>` (single-session focused view)
- Graceful handling of F2 data contract edge cases: truncated final lines, unknown event types, no `workflow_completed` event

### Out of Scope

- Daemon / always-on mode (explicitly deferred V2; command interface must not preclude it)
- Web-based dashboard (requires introducing tokio; V2 if demand emerges for remote access preview)
- Remote access (F5/F6 scope)
- Auth or multi-user scenarios
- Cross-repository session aggregation
- Non-koto observability (system metrics, external logs)
- Session management actions (cancel, rewind, override from dashboard)

## Research Leads

1. **UX design for the ratatui TUI layout**: How should the root/coordinator/child hierarchy be presented visually? What fits on a standard 80×24 terminal? What's the right key bindings for navigation (up/down, expand/collapse, refresh)?
2. **Handling 1000+ sibling batches**: The batch view must aggregate without overwhelming. What's the right summary row format? How does drill-down work for inspecting individual failing tasks?
3. **Terminal state detection without `workflow_completed`**: F2 has a known gap. PRD must specify the approach: load compiled template to check terminal states vs. timeout heuristic vs. "last transition" inference.
4. **Gate display design**: `gate_evaluated` carries different schemas for different gate types (command gate, context-exists, children-complete). PRD must specify what to show for each type without requiring the user to parse raw JSON.
5. **Command interface design for V2 compatibility**: `koto dashboard` / `koto dashboard <name>` must be nameable in a way that doesn't preclude a future `koto dashboard --daemon` or `koto daemon start` without breaking changes.

## Coverage Notes

Things NOT answered by exploration that the PRD should address:

- **Exact terminal state detection strategy**: exploration identified the gap (no `workflow_completed` event) but didn't decide between template-coupling, timeout, and last-transition approaches. PRD must choose.
- **Key bindings and navigation UX**: exploration established ratatui as the rendering framework but didn't design the UI interaction model. PRD needs wireframe-level specification.
- **Epoch-branched sessions (rewound batches)**: Sessions with names like `parent~1.task-a` appear after `koto rewind`. PRD must specify whether the dashboard shows them separately, hides them, or annotates them.
- **Performance at scale**: With hundreds of sessions and polling, what's the acceptable startup time? PRD should specify a rough performance envelope.
- **Update latency specification**: Exploration identified 500ms polling as reasonable but didn't set it as a firm requirement. PRD should codify this.

## Decisions from Exploration

The following were settled during exploration and should be treated as given in the PRD:

- **Rendering technology**: Terminal UI using ratatui. Not web server (requires tokio — first async code in the project). Not native desktop (no distribution story).
- **Invocation model**: Ad-hoc command (`koto dashboard`). Not daemon/always-on. JSONL logs accumulate from `koto init` regardless of dashboard state — launching mid-session still shows full history.
- **Live update mechanism**: File system polling at ~500ms. inotify/kqueue as optional V2 optimization. Cross-platform; no new platform-specific dependencies for MVP.
- **Gate display scope**: In scope. Tier 2 `gate_evaluated` events are the data source. Client-side computation of "last gate result per state."
- **Session scope**: Current repository only (repo-id derived from working directory). Not cross-repo aggregation.
- **Daemon mode**: Explicitly deferred to V2. PRD and design must use command naming compatible with future `koto dashboard --daemon` or `koto daemon start/stop` pattern.
- **Target use case**: Monitor hours-long orchestration pipelines — today the `/work-on` workflow; in the future, the full multi-phase sequence (explore → prd → design → plan → work-on) when managed by koto.
