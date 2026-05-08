---
status: Proposed
upstream: docs/prds/PRD-dashboard-full-observability.md
problem: |
  The koto dashboard has a working scaffold — event loop, session tree, polling,
  --once output — but five areas are broken or missing. Elapsed time is hardcoded
  to zero. The detail pane gate guard blocks evidence-only sessions. Session
  discovery is scoped to the current working directory's repository hash, making
  it useless for monitoring parallel workflows across multiple workspaces. Tree
  rendering has no connectors and is capped at one level deep. The layout is a
  fixed 8-row vertical strip rather than the intended horizontal split. Three new
  capabilities (tabbed detail pane, session identity fields, global session scope)
  need architectural integration.
decision: |
  placeholder — filled in Phase 6
rationale: |
  placeholder — filled in Phase 6
---

# DESIGN: koto dashboard — full observability surface

## Status

Proposed

## Context and Problem Statement

The dashboard lives in four source files: `src/cli/dashboard.rs` (event loop,
`--once` mode), `src/cli/dashboard_state.rs` (session tree, `visible_rows`,
`TaskCounts`), `src/cli/dashboard_data.rs` (session scanning, event parsing,
detail data), and `src/cli/dashboard_render.rs` (ratatui widgets).

**Five confirmed broken areas in the current code:**

1. **Elapsed is hardcoded.** `visible_rows()` in `dashboard_state.rs` sets
   `elapsed: Duration::from_secs(0)` for every session. The `--once` path uses
   `session.mtime.elapsed()` (file modification time), not event timestamps.
   `compute_elapsed_since()` already parses ISO 8601 event timestamps correctly
   but is only used for gate evaluation timestamps, not for session elapsed time.

2. **Detail pane gate guard.** `read_detail()` in `dashboard_data.rs` returns
   `None` when no `GateEvaluated` event exists in the current epoch. Evidence-only
   sessions — the dominant pattern — permanently show "No gate evaluations recorded."
   The gate-presence guard must be removed and the detail pane made universal.

3. **Session discovery is repo-scoped.** `LocalBackend::new(working_dir)` hashes
   the working directory to derive `~/.koto/sessions/<repo-id>/` as the scan root.
   Sessions in other workspaces are invisible. The backend must scan globally across
   all local sessions regardless of cwd.

4. **Tree rendering is shallow and connector-free.** `visible_rows()` renders only
   depth 0 (roots) and depth 1 (direct children). The render layer uses space
   indentation only — no `├─`/`└─` connectors. `TaskCounts` lacks `blocked` and
   `done_blocked` fields, so status rollup cannot distinguish terminal-blocked
   children from terminal-done children.

5. **Layout is a vertical strip.** `dashboard_render.rs` uses
   `Constraint::Length(8)` for an 8-row detail strip at the bottom. The intended
   design — a horizontal 40%/60% split with both panels visible simultaneously —
   was never built.

**Three new capabilities needed:**

6. **Tabbed detail pane.** Summary tab (current state, directive from
   `CompiledState`, latest evidence, gate result, intent, template_name). History
   tab (full chronological event log, 10 event types, scrollable, with gate
   condition text from the compiled template). Remaining tab (unvisited states in
   topological order from the compiled template).

7. **Session identity fields.** `intent: Option<String>` and
   `template_name: Option<String>` on `StateFileHeader` using the established
   `#[serde(default, skip_serializing_if = "Option::is_none")]` pattern. Set via
   `koto init --intent "<text>"`. Updatable mid-workflow via
   `koto session update <name> --intent "<text>"`.

8. **`EvidenceSubmitted.summary`.** Optional free-text summary on the
   `EvidenceSubmitted` event payload, following the existing `submitter_cwd`
   pattern. Surfaced in the Summary tab above raw evidence fields.

## Decision Drivers

- **Additive schema safety.** All changes to `StateFileHeader` and
  `EvidenceSubmitted` must follow `#[serde(default, skip_serializing_if =
  "Option::is_none")]`; no schema version bump; existing sessions must
  deserialize cleanly.

- **Concurrent write safety.** `koto session update --intent` writes to a state
  file that the koto engine may be reading and writing concurrently. The mutation
  strategy must not corrupt the state file or the event log.

- **Refresh performance.** A full refresh cycle must complete under 200 ms for
  a session with 500 events. The always-visible horizontal split means detail data
  may need to load on every cursor move rather than only when entering Detail mode.

- **Terminal width adaptation.** Three layout modes: ≥80 columns horizontal split,
  <80 columns list-only, <40 columns "terminal too narrow" message. Layout must
  switch cleanly without panics or widget overflow.

- **Backwards compatibility.** Existing 4-column `--once` consumers continue to
  work. New columns 5–6 are additive. Old state files without new fields
  deserialize cleanly.

- **Compiled template reuse.** The compiled template (already loaded for the
  Remaining tab) provides gate condition text for the History tab. No additional
  disk reads should be required beyond what the Remaining tab already loads.

- **Global scope as F5 foundation.** Session discovery must be
  working-directory-independent so that the same scope model extends cleanly to
  cloud storage (F5) without redesigning discovery.
