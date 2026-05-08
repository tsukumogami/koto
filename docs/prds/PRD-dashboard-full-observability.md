---
status: In Progress
problem: |
  The koto dashboard introduced in PRD-local-dashboard.md gives users a live list of
  sessions with basic status and elapsed time. While the infrastructure is in place, the
  implementation has several broken behaviors and a large gap between what the event log
  records and what the dashboard surfaces. The elapsed column always shows "0s" regardless
  of actual runtime. The detail pane is gated on gate evaluations and returns empty for
  evidence-based sessions, which are the dominant workflow pattern. Three derivation
  functions for decisions, overrides, and visit counts are implemented but never called.
  Rich narrative data — transition rationale, decision rationale, rewind rationale, gate
  override rationale — is stored in the event log but rendered nowhere. Session identifiers
  are opaque machine-generated slugs. Parent-child tree rendering is incomplete: connectors
  are missing, expand/collapse is bound to the wrong mode, and depth is capped at one level.
  Session discovery is scoped to the current working directory, so a developer monitoring
  a dozen parallel agentic workflows across multiple niwa workspaces sees only the sessions
  from whichever repo they happened to launch the dashboard from. The net result is a
  dashboard that operators open, find incomplete and uninformative, and abandon in favor
  of running manual CLI queries.
goals: |
  The enhanced dashboard gives operators a complete picture of any workflow session without
  leaving the TUI or running auxiliary commands. A three-tier information hierarchy (list
  view, tabbed detail pane, scrollable history) surfaces all event data the koto engine
  already records. Sessions are labeled with human-readable intent and template names. The
  detail pane works for every session type — gate-based, evidence-based, and hybrid. The
  full event narrative is browsable in a scrollable History tab. Parent-child session trees
  use proper connectors, correct key bindings, and propagate worst-case child status to
  collapsed parent rows. Session discovery spans the full local machine rather than the
  current working directory, so developers monitoring parallel agentic workflows across
  multiple niwa workspaces see everything in one view. The `--once` mode includes all new
  fields for integration tests without breaking existing scripts that read the current
  four-column output.
---

# PRD: koto dashboard — full observability surface

## Status

In Progress

## Problem Statement

The koto dashboard (`PRD-local-dashboard.md`, source issue #366) was built to give operators
live visibility into workflow sessions. The basic scaffolding — session list, state names,
tree hierarchy, polling — is in place. What is missing or broken prevents it from being the
primary monitoring tool it was designed to be.

**Broken basics.** The elapsed column is hardcoded to `Duration::from_secs(0)` in
`visible_rows()`. Every session shows "0s" regardless of how long it has been running. The
detail pane is gated on the presence of a `GateEvaluated` event in the current epoch: the
`read_detail()` function returns `None` when no gate evaluations exist, forcing the pane to
show "No gate evaluations recorded." Evidence-only sessions — the dominant pattern in
shirabe-style workflows — permanently show this message.

**Invisible event data.** The koto event log records rich narrative data that the dashboard
never renders: `DecisionRecorded.decision.rationale`, `DirectedTransition.rationale`,
`Rewound.rationale`, `GateOverrideRecorded.rationale`, `ContextAdded` artifacts, and
`DefaultActionExecuted` output. The data layer has no functions for deriving decisions,
overrides, or visit counts from the event log — this capability needs to be built, not
merely wired up.

**No context at the list level.** Session identifiers like `task_session-feed-issue-1` are
machine-generated slugs that tell an operator nothing about what the session is trying to
accomplish or which template it runs. There is no mechanism to associate a session with its
human intent without running `koto query`.

**Incomplete tree rendering.** The tree infrastructure is 80% built: `expanded`, `visible_rows`,
`indent_depth`, `TaskCounts`, and `sorted_children` are all implemented in `dashboard_state.rs`.
The gaps are: the render layer uses space indentation only (no `├─`/`└─` connectors), expand
and collapse are bound to Detail mode instead of List mode, `visible_rows` is hardcoded to
one level deep, and the status rollup does not account for `done_blocked` (blocked-terminal)
children — only gate failures.

**Layout mismatch.** The current layout uses `Constraint::Length(8)` for a fixed 8-row strip
at the bottom. The stated design intent — horizontal split with list at ~40% and detail at
~60% — was never built. The 8-row strip cannot accommodate meaningful content for any of the
three tiers (quick-check, status review, deep history).

**No cross-workspace visibility.** Session discovery is scoped to the repository containing
the current working directory. A developer running koto workflows across a dozen niwa
workspaces — the primary use case for the dashboard — sees only the sessions in whichever
repo they invoked `koto dashboard` from. This makes the dashboard useless as a real-time
monitoring tool for parallel agentic work and undermines the path to F5 (S3-backed
dashboard): a scope model tied to the current directory cannot extend naturally to cloud
storage.

## Personas

**Operator** — a developer running long-form agentic workflows and monitoring their
progress from a single terminal. The primary scenario: a developer has a dozen niwa
workspaces open on their local machine, each running an independent shirabe workflow
(explore → prd → design → plan → work-on). From a single `koto dashboard` session they
watch all workflows advance in real time, deep-dive into the decisions and assumptions any
AI agent has made, and navigate to a specific Claude Code session when they need to nudge
it in a new direction.

This persona is the same audience targeted by the koto observability VISION: a developer
who needs to watch parallel agents from one screen without switching between terminal
sessions. The dashboard is their control surface.

## Goals

1. **Fix the broken basics.** Elapsed time shows the real duration since the last state
   transition. The detail pane renders useful content for every session type without requiring
   a gate evaluation.

2. **Surface the rich event data that already exists.** Decision rationale, transition
   rationale, rewind rationale, gate override rationale, context artifacts, and evidence
   summaries are visible in the detail pane. Existing derivation functions are wired in.

3. **Three-tier information hierarchy.** Quick-check (list): name, status, elapsed, template.
   Status review (Summary tab): current state, directive, latest evidence, gate result.
   Deep history (History tab): full chronological event timeline, scrollable. Remaining
   (Remaining tab): states not yet visited, derived from the template graph.

4. **Identifiable sessions.** Operators can label sessions with a human-readable intent at
   init time. The template name is surfaced in the list view without disk reads on every
   refresh.

5. **Correct tree visualization.** `├─`/`└─` connectors. Expand/collapse in List mode.
   Unlimited depth. Status rollup covers `done_blocked` children. Parent shows worst-case
   descendant status when collapsed.

6. **Testability via `--once`.** The `--once` mode provides a non-interactive snapshot of
   dashboard state, primarily useful for integration tests that assert on session data
   without running the interactive TUI. Extending its columns to include `intent` and
   `template_name` makes the new fields verifiable in automated tests.

7. **Global session scope.** The dashboard discovers all koto sessions on the local
   machine regardless of the directory from which it is invoked. A developer running a
   dozen niwa workspaces sees all workflow sessions in a single view. This scope model
   is the foundation for F5 (S3-backed dashboard) from the koto observability roadmap:
   once scope is global and directory-independent, extending it to cloud storage is a
   matter of swapping the storage backend, not redesigning the scope model.

## User Stories

**US-1: Status check across all workspaces**
As an operator with multiple niwa workspaces running independent shirabe workflows, I want
to open the dashboard from any directory and immediately see the status of all sessions on
my local machine — running, blocked, failed, or done — without running `koto query` or
switching between workspace directories.

**US-2: Understanding a session's current directive**
As an operator who sees a session in an unexpected state, I want to select that session and
read its current directive (the human-readable instruction from the compiled template), the
most recent evidence submitted, and the gate result if applicable — all within the detail
pane.

**US-3: Tracing how a session got here**
As an operator investigating a stuck or unexpected session, I want to scroll through the
full event timeline — transitions, decisions, evidence submissions, gate evaluations, rewinds
— from within the dashboard, without running `koto query --events`.

**US-4: Knowing what's left**
As an operator monitoring progress, I want to see which states in a session's template have
not yet been visited so I can estimate remaining work.

**US-5: Parent-child pipeline visibility**
As an operator running a workflow with child sessions, I want to see those children grouped
under their parent with proper tree connectors, expand or collapse the subtree with keyboard
shortcuts, and have the parent row show `blocked` or `failed` immediately if any descendant
is in that state.

**US-6: Labeled sessions**
As an operator who initializes many sessions from the same template for different tasks, I
want each session to display a human-readable label alongside its slug identifier so I can
tell them apart without memorizing session names.

**US-7: Integration test verification**
As a contributor writing integration tests for the dashboard, I want to run
`koto dashboard --once` and assert on the tab-separated output to verify that `intent`,
`template_name`, and `status_bucket` are correctly extracted and displayed — without
driving the interactive TUI.

**US-8: Cross-workspace monitoring from a fixed location**
As an operator who keeps a dedicated terminal with `koto dashboard` running, I want that
terminal to show sessions from all my niwa workspaces without having to restart the
dashboard from each workspace directory. When I start a new workflow in workspace B while
the dashboard is running from workspace A, the new session appears automatically on the
next polling cycle.

## Requirements

### Functional Requirements

**R1: Elapsed column — real duration**
The elapsed column must display the true time since the session's most recent state
transition, derived from event timestamps in the JSONL log. It must not show "0s" for a
session that transitioned more than one second ago. For sessions that have been initialized
but have never produced a `StateTransitioned` event, elapsed is calculated from the
`WorkflowInitialized` event timestamp.

**R2: Universal detail pane — no gate guard**
The detail pane must render content for any session regardless of whether it has produced a
`GateEvaluated` event. The gate-presence guard in `read_detail()` must be removed. Minimum
content for each session state:
- Evidence-only session: state name + at least the most recent evidence `key: value` pairs
- Gate-only session: state name + gate name, result (PASS/FAIL), and command
- Hybrid (evidence + gate): state name + evidence fields + gate result
- Newly initialized (no evidence or gate events): state name + "No data yet" placeholder

"No gate evaluations recorded" must not appear for any session that has submitted evidence.

**R3: Horizontal split layout**
The layout must be a horizontal split using `Constraint::Percentage(40)` for the session
list on the left and `Constraint::Percentage(60)` for the detail pane on the right. Both
panels must be visible simultaneously on terminals ≥80 columns. The current
`Constraint::Length(8)` vertical strip must be replaced.

**R4: Tabbed detail pane — Summary tab**
The detail pane must have a Summary tab (displayed by default when entering Detail view)
showing:
- Current state name
- The `directive` field from the current state's `CompiledState` entry (the human-readable
  instruction that tells the agent what to do in this state)
- Most recent evidence submission: evidence fields as `key: value` pairs; the optional
  `summary` string (from R9) rendered preceded by a blank line and displayed in bold above
  the raw fields
- Gate result (if a gate was evaluated in the current epoch): gate name, result (PASS/FAIL),
  command
- Session `intent` (from R7) and `template_name` (from R8) if present

Sessions with no data beyond `WorkflowInitialized` must show a "No data yet" placeholder
in the Summary tab, not an error or empty white space.

**R5: Tabbed detail pane — History tab**
The detail pane must have a History tab showing all events for the selected session in
chronological order (oldest to newest). The tab must be scrollable. Each event row must
include a formatted timestamp prefix in `[YYYY-MM-DD HH:MM:SS]` format. At minimum the
following event types must be rendered with meaningful labels and their relevant fields:

| Event type | Fields to render |
|---|---|
| `WorkflowInitialized` | template name, initial variables |
| `StateTransitioned` | from-state, to-state, trigger |
| `EvidenceSubmitted` | state, fields (key: value), `summary` if present |
| `GateEvaluated` | gate name, result (PASS/FAIL), and a second line with gate type and condition summary derived from the compiled template: `command` gates show `cmd: <command>`; `context-exists` gates show `key: <key>`; `context-matches` gates show `key: <key>  pattern: <pattern>`; `children-complete` gates show `children: <completed>/<total> complete`. When the compiled template is unavailable, the condition line is silently omitted. |
| `DecisionRecorded` | decision text, rationale |
| `DirectedTransition` | target state, rationale |
| `Rewound` | from-state, to-state, rationale |
| `GateOverrideRecorded` | gate name, override result, rationale |
| `ContextAdded` | key, artifact reference |
| `DefaultActionExecuted` | action type, first 3 newline-delimited lines of output |

Unknown event types must be rendered as `[Unknown event: <type>]` rather than silently
omitted or crashing. Switching to a different session resets the History tab scroll
position to the top.

**R6: Tabbed detail pane — Remaining tab**
The detail pane must have a Remaining tab listing states in the session's compiled template
that have not yet been visited (no `StateTransitioned` event with that destination in the
current epoch). States must be listed in topological order when the template is a DAG; for
states at the same topological depth, ties are broken by definition order (the order state
keys appear in the compiled JSON). When the template is not a DAG, states are listed in
definition order.

When the compiled template is unavailable (cache cleared, session from another machine), the
tab must display exactly:
> Template unavailable — run `koto template compile <path>` to restore the remaining-states
> view.

A terminal session (all states visited) must show an empty list or "All states visited."

**R7: `intent` on StateFileHeader**
`StateFileHeader` must gain `intent: Option<String>` following the established pattern:
`#[serde(default, skip_serializing_if = "Option::is_none")]`. The field is set at
initialization via `koto init --intent "<text>"` and can be updated on an existing session
via `koto session update <name> --intent "<text>"`. Both commands write the value to the
state file header atomically. Older state files that predate this field must deserialize
without error; the field defaults to `None`.

**R8: `template_name` on StateFileHeader**
`StateFileHeader` must gain `template_name: Option<String>` using the same serde pattern as
`intent`. The value is written from the compiled template's `name` field at `koto init` time.
The dashboard reads it from the state file header on each refresh without loading the
compiled template from disk.

**R9: `summary` on EvidenceSubmitted**
The `EvidenceSubmitted` event payload must gain `summary: Option<String>`, following the
existing `submitter_cwd` pattern. Agents submit it via `--with-data '{"summary": "...",
...}'`. The dashboard renders it as a one-line narrative above raw evidence fields in the
Summary tab. Pre-feature event payloads without this field must deserialize without error.

**R10: Intent display in the list view**
The session list must display `intent` when present. Display format: `<session-id> · <intent>`,
where the 60-character limit applies to the full rendered string including the ` · ` separator.
If the full string exceeds 60 characters, truncate the intent portion (not the session ID)
with `…`, so the session ID is always fully visible. When `intent` is absent, only the
session ID is shown (no ` · ` separator or placeholder). Below 60 terminal columns, only
the session ID is shown regardless of whether intent is set. `template_name` must appear
as a distinct column in the list, showing `"-"` when absent.

**R11: Tree visualization — connectors and depth**
The tree render layer must use ASCII connector characters:
- `├─ ` for non-final siblings
- `└─ ` for the final sibling in a group

Root-level sessions (depth 0) have no connector prefix. Each level of depth adds one set
of connectors; child indentation uses 2 spaces per additional level beyond depth 1. The tree
must render to at least 5 levels of nesting without compile-time caps; the actual depth is
determined by session data. Collapsed parent rows must show a child-count badge `[N]` after
the session name, where N is the count of direct children only (not all descendants). Expand
and collapse must work in List view mode (not only in Detail mode) via the keys defined in
R14. Pressing expand or collapse on a leaf session (no children) is a no-op.

**R12: Parent-child status rollup**
When a parent session is collapsed, its displayed status must reflect the worst-case status
across all descendants. Rollup priority (highest to lowest):
1. Any descendant is `failed` → parent shows `failed`
2. Any descendant is `blocked` (non-terminal gate failure) or `done_blocked` (terminal
   blocked state) → parent shows `blocked` (when no `failed` descendants exist)
3. All descendants are `done` and parent is itself terminal → parent shows `done`
4. All descendants are terminal but the set includes both `done` and `done_blocked` →
   parent shows `blocked`
5. Otherwise → parent shows `running`; if the parent itself has `unknown` status and no
   descendants have `failed` or `blocked`, the parent shows `unknown`

When expanded, each row shows its own independent status.

The existing `TaskCounts` struct must be extended to track `blocked` and `done_blocked` as
distinct buckets so that `done_blocked` terminal children are not conflated with `done`.

**R13: `--once` mode — extended columns for testability**
`koto dashboard --once` is a non-interactive snapshot mode primarily used in integration
tests to assert on dashboard state without driving the TUI. Its output must be extended from
4 to 6 tab-separated columns per line:
```
session_id\tcurrent_state\telapsed\tstatus_bucket\tintent\ttemplate_name
```
Columns 1–4 are unchanged. `intent` (column 5) is empty string when not set.
`template_name` (column 6) is empty string when not set. The `status_bucket` values
(`running`, `blocked`, `done`, `failed`, `unknown`) are unchanged.

**R14: Complete keyboard reference**
The dashboard must implement the following keyboard bindings:

*Global (any view):*
| Key | Action |
|-----|--------|
| `q` or `Ctrl+C` | Quit |
| `r` | Force refresh |

*List view:*
| Key | Action |
|-----|--------|
| `j` or `↓` | Cursor down |
| `k` or `↑` | Cursor up |
| `Enter` | Enter Detail view for focused session |
| `l` or `→` | Expand focused session's children (no-op if none or already expanded) |
| `h` or `←` | Collapse focused session's children (no-op if none or already collapsed) |

*Detail view:*
| Key | Action |
|-----|--------|
| `j` or `↓` | Select next session (stays in Detail view) |
| `k` or `↑` | Select previous session (stays in Detail view) |
| `Esc` | Return to List view |
| `Tab` | Cycle to next tab (Summary → History → Remaining → Summary) |
| `Shift+Tab` | Cycle to previous tab (Summary → Remaining → History → Summary) |
| `1` | Jump to Summary tab |
| `2` | Jump to History tab |
| `3` | Jump to Remaining tab |
| `PageDown` or `Ctrl+D` | Scroll down in current tab |
| `PageUp` or `Ctrl+U` | Scroll up in current tab |
| `h` or `←` | No-op (these keys collapse the tree in List view; Detail view ignores them) |
| `l` or `→` | No-op (these keys expand the tree in List view; Detail view ignores them) |

### Non-functional Requirements

**R15: Backwards compatibility for schema changes**
State files predating this feature (without `intent`, `template_name`, or
`EvidenceSubmitted.summary`) must deserialize without errors. These fields default to `None`.
The schema version remains 1; no migration is required.

Each new optional field must have a round-trip test confirming that:
- Old state files (without the field) parse cleanly
- Re-serializing a state header without the field omits it (no null written)

**R16: Terminal width adaptation**
The horizontal split requires ≥80 terminal columns. Below 80 columns the dashboard must
automatically switch to a list-only view (the detail pane is hidden and the session list
fills the full width). Below 40 columns the dashboard shows a "terminal too narrow" message.
`--once` output is unaffected by terminal width.

**R17: Refresh performance**
The dashboard polls at 1 second intervals by default (inheriting the existing polling interval).
Each full refresh cycle — reading all session state files and rendering all visible rows —
must complete in under 200 ms for a session set with up to 500 events per JSONL log. This
ensures one complete render per polling cycle without skipped frames.

**R18: Global session discovery**
`koto dashboard` must discover and display all koto sessions on the local machine regardless
of the working directory from which the command is invoked. Session discovery uses the
global koto sessions directory (as defined by the F2 data contract) rather than a
per-repository or per-directory subset. Sessions from different workspaces appear in the
same flat list ordered by health severity, with recency as a tiebreaker within each
severity bucket. Sort key: failed (0) → blocked (1) → running (2) → unknown (3) → done (4),
then most recently active descending within each bucket. This ordering applies to both the
interactive TUI and `--once` output.

This scope is the prerequisite for F5 (S3-backed dashboard) in the koto observability
roadmap. Once session scope is global and directory-independent at the local level, the
storage backend can be swapped for S3 without redesigning the scope model.

`koto dashboard --once` launched from a directory with no local koto sessions must still
output rows for sessions discovered in other workspaces on the machine.

## Acceptance Criteria

### Elapsed column

- [ ] A session observed 30 seconds after its last state transition shows elapsed between
  28s and 32s (allowing for ±2s polling variance at a 1-second polling interval)
- [ ] The elapsed column never shows "0s" for a session that transitioned more than 10
  seconds ago
- [ ] A session that has been initialized but has not yet transitioned shows elapsed
  calculated from the `WorkflowInitialized` event timestamp, not "0s"

### Universal detail pane

- [ ] A session with only evidence submissions (no gate evaluations) shows a non-empty
  Summary tab with evidence fields rendered as `key: value` pairs
- [ ] A session with only gate evaluations shows gate name, result, and command in the
  Summary tab
- [ ] A session with both evidence and gate evaluations shows both sections in the Summary
  tab: evidence fields above gate result
- [ ] A newly initialized session (no events beyond `WorkflowInitialized`) shows "No data
  yet" in the Summary tab
- [ ] No session shows "No gate evaluations recorded" in place of actual session data

### Layout

- [ ] On an 80-column terminal the session list and detail pane are side-by-side
- [ ] The detail pane widget is given at least 48 columns of horizontal space when the
  terminal is exactly 80 columns wide (60% of 80 = 48 columns)
- [ ] On a 79-column terminal the dashboard shows list-only view (detail pane hidden)
- [ ] On a 39-column terminal the dashboard shows "terminal too narrow" instead of a
  corrupted list view

### Summary tab

- [ ] The directive text for the current state is visible in the Summary tab
- [ ] When `EvidenceSubmitted` has a `summary` field, it appears as the first line of the
  evidence section, above the raw `key: value` pairs
- [ ] `intent` appears in the Summary tab when set
- [ ] `template_name` appears in the Summary tab when set

### History tab

- [ ] All 10 event types from R5 are rendered with their relevant fields in the History tab
- [ ] Each event row includes a `[YYYY-MM-DD HH:MM:SS]` timestamp prefix
- [ ] A `GateEvaluated` event for a `command` gate renders two lines: the name/result line
  and a second line showing `cmd: <command>` from the compiled template
- [ ] A `GateEvaluated` event for a `context-exists` gate renders a second line showing
  `key: <key>`
- [ ] A `GateEvaluated` event for a `children-complete` gate renders a second line showing
  `children: <completed>/<total> complete`
- [ ] When the compiled template is unavailable, `GateEvaluated` renders only the name and
  result line — no crash, no error message, no blank line
- [ ] An event type not in the known list is rendered as `[Unknown event: <type>]` and does
  not crash the dashboard
- [ ] Events appear in chronological order (oldest first)
- [ ] The History tab is scrollable; `PageDown`/`Ctrl+D` reaches content below the visible
  area
- [ ] After pressing `PageDown` past the last event, the scroll offset does not increase
  further and the last event entry remains visible on screen
- [ ] Switching focus to a different session resets the History tab scroll position to the
  top

### Remaining tab

- [ ] An active session (not yet terminal) lists at least one unvisited state in the
  Remaining tab
- [ ] A terminal session shows an empty Remaining tab or "All states visited"
- [ ] For a DAG template with states at equal topological depth, states are listed in
  definition order (the order they appear in the compiled JSON), not alphabetical order
- [ ] When the compiled template is unavailable, the exact fallback message from R6 is shown
  instead of a crash or blank content

### `intent` and `template_name`

- [ ] `koto init --intent "investigating issue #42"` persists `intent: investigating issue
  #42` in the state file header
- [ ] `koto session update <name> --intent "new text"` overwrites the `intent` field in the
  state file header of an existing session without modifying any other field
- [ ] Running `koto session update` on a session that has no `intent` set adds the field;
  running it on a session that already has `intent` replaces the value
- [ ] A session initialized without `--intent` shows only the session ID in the list
  (no `·` separator)
- [ ] A session where `<session-id> · <intent>` is exactly 60 characters shows the full
  string without `…`
- [ ] A session where `<session-id> · <intent>` is 61 characters shows the intent truncated
  with `…`, keeping the full session ID visible
- [ ] `template_name` appears as a column in the list for sessions whose template has a
  `name:` field in its frontmatter
- [ ] State files created before this feature (missing `intent`, `template_name`) load
  without errors; these fields display as absent

### Tree visualization

- [ ] A collapsed parent session shows `[N]` where N is the count of direct children (not
  all descendants)
- [ ] Pressing `l` or `→` in List view on a collapsed parent reveals its children with
  `├─` and `└─` connectors
- [ ] Pressing `h` or `←` in List view on an expanded parent collapses its children
- [ ] Pressing `l` or `→` in List view on a leaf session (no children) does not crash and
  has no visible effect
- [ ] A parent with a `failed` descendant shows `failed` status when collapsed
- [ ] A parent with a `done_blocked` terminal descendant (and no `failed` descendants) shows
  `blocked` status when collapsed
- [ ] A parent whose all descendants are `done` and is itself terminal shows `done` when
  collapsed
- [ ] When a parent is expanded, each child row shows its own independent status (not the
  parent's rollup status)

### `--once` output

- [ ] `koto dashboard --once` outputs exactly 6 tab-separated columns per line
- [ ] Column 4 (`status_bucket`) outputs one of exactly five values: `running`, `blocked`,
  `done`, `failed`, or `unknown` — no other values
- [ ] Column 5 (intent) is empty string for sessions without intent
- [ ] Column 6 (template_name) is empty string for sessions without template_name
- [ ] A script that reads columns 1–4 only continues to produce correct output after this
  change (new columns are additive)
- [ ] `koto dashboard --once` with no koto sessions on the local machine exits with code 0
  and produces no output lines (empty stdout)

### Keyboard reference

- [ ] `l`/`→` expands a collapsed parent in List view
- [ ] `h`/`←` collapses an expanded parent in List view
- [ ] `Tab` cycles through the three tabs in Detail view (Summary → History → Remaining →
  Summary)
- [ ] `Shift+Tab` cycles in reverse through the three tabs (Summary → Remaining → History →
  Summary)
- [ ] `1`, `2`, `3` jump directly to Summary, History, and Remaining tabs
- [ ] `PageDown`/`Ctrl+D` scrolls down in the History tab when content exceeds pane height
- [ ] `Esc` returns to List view from Detail view
- [ ] `j`/`↓` in Detail view moves to the next session without leaving Detail view
- [ ] `k`/`↑` in Detail view moves to the previous session without leaving Detail view
- [ ] `r` in any view triggers an immediate refresh without waiting for the polling interval
- [ ] `h`/`←` and `l`/`→` in Detail view are no-ops (no crash, no mode change)

### Global session scope

- [ ] `koto dashboard` launched from `/workspace-a` displays sessions originating in
  `/workspace-b` when both have active koto sessions on the same machine
- [ ] `koto dashboard --once` launched from a directory with no koto sessions outputs
  rows for sessions discovered in other workspaces on the local machine
- [ ] A session started in a new workspace after the dashboard is already running appears
  on the next polling cycle without restarting the dashboard
- [ ] A `failed` session appears above a `running` session in the list regardless of which
  transitioned more recently
- [ ] A `blocked` session appears above a `running` session and below a `failed` session
- [ ] Two `running` sessions are ordered by most recent transition (more recently active first)
- [ ] `done` sessions appear below all active sessions (`failed`, `blocked`, `running`,
  `unknown`)
- [ ] `--once` output reflects the same severity-first ordering as the TUI

### Backwards compatibility

- [ ] A state file created by an older koto version (missing `intent`, `template_name`,
  `EvidenceSubmitted.summary`) loads without a parse error
- [ ] Round-trip serialization of a `StateFileHeader` without `intent` or `template_name`
  omits those fields (no null values written to disk)
- [ ] New round-trip tests for `intent`, `template_name`, and `EvidenceSubmitted.summary`
  follow the pattern of `header_none_parent_workflow_not_serialized()` in `types.rs`

### Performance

- [ ] A full refresh cycle for a session list containing a single session with 500 events
  completes within 200 ms (measured from start of polling cycle to frame render)
- [ ] A session below 79 terminal columns shows list-only mode — detail pane is not rendered
  and no layout overflow or panic occurs

## Out of Scope

- **Full raw event log viewer**: Deep event inspection remains the domain of `koto query`.
  The History tab is a browsable summary, not a replacement.
- **Engine changes for composable child-spawning**: Tree visualization uses existing
  `parent_workflow` pointers. A `spawn_child` action or `child-terminal` gate type are
  deferred to a future milestone.
- **Web or browser-based visualization**: Terminal TUI only.
- **Real-time remote streaming**: Local state files only.
- **Session filtering / search**: The `/` filter pattern is deferred to a follow-on issue.
- **Rewind epoch navigation**: Browsing archived `~`-named epoch sessions is deferred.
- **Help overlay**: A `?` key overlay is out of scope. R14 documents the bindings; in-app
  surfacing is deferred.
- **Mouse support**: Keyboard only.
- **`koto init --intent` validation**: No length limits or character restrictions on intent.

## Known Limitations

- **Remaining tab requires local template cache**: The Remaining tab depends on the compiled
  template in `~/.cache/koto/<hash>.json`. Sessions imported from another machine or whose
  cache was cleared show the "Template unavailable" fallback. This matches the existing
  behavior of `koto status` for terminal detection.
- **`intent` is human-editable but not auto-generated**: The dashboard renders intent when
  present but cannot require agents to provide it. Sessions without intent fall back to
  displaying the session ID only.
- **`EvidenceSubmitted.summary` is agent-optional**: The dashboard renders it when present
  but cannot require agents to provide it. Evidence without a summary falls back to raw
  field display.

## Decisions and Trade-offs

**Decision: Separate new PRD from PRD-local-dashboard.md**
The original PRD (source issue #366) covers the basic dashboard infrastructure that is
already implemented and in progress. Extending that PRD would mix "done" requirements with
new ones, making implementation tracking unclear. A new PRD captures the enhancement scope
cleanly. The original PRD is not superseded; both remain valid.

**Decision: Universal Summary tab — no session-type discrimination**
Alternatives: (1) separate gate-only and evidence-only tabs; (2) conditional UI based on
session type. Both require operators to understand which session type they are looking at.
The universal Summary tab renders all available data regardless of session type, with absent
sections silently omitted. This is simpler to implement and easier to use. Justification:
the goal is monitoring, not diagnostics.

**Decision: `intent` supplements the session ID rather than replacing it**
The session ID is the key used in all `koto next <name>` / `koto rewind <name>` commands.
Replacing it in the list view would break the operator's ability to copy a row name and use
it in CLI commands. Format `<session-id> · <intent>` with 60-character truncation preserves
the ID while adding context. Alternatives considered: intent in a separate column (adds
width pressure), intent replaces ID (breaks CLI handoff).

**Decision: `--once` columns appended, not interleaved**
New columns are at positions 5 and 6. Interleaving them before `status_bucket` would break
existing parsers reading column 4. The backwards-compatibility requirement (R15) mandates
that scripts reading columns 1–4 continue to work. Empty string (not null or placeholder)
is used for absent values so `awk '{ print $5 }'` parsing works consistently.

**Decision: Status rollup uses worst-case child status**
Alternatives: best-case, running-if-any-running, custom aggregate. Worst-case
(failed > blocked > running > done) matches the convention used by monitoring tools in
general and surfaces the most actionable condition first. The rollup applies only when the
parent is collapsed.

**Decision: `l`/`h` for tree expand/collapse in List view; no-op in Detail view**
These bindings follow the vim hjkl convention where `h`/`l` mean directional navigation
(left/right, i.e., shallower/deeper in the tree). Since `j`/`k` are bound to cursor
movement, `h`/`l` for tree depth matches the established pattern. Arrow key equivalents are
provided. In Detail view, `h`/`←` and `l`/`→` are no-ops. The alternative — binding them
to tab navigation in Detail view — was considered but rejected: two different actions for
the same key in adjacent modes would confuse users. Tab navigation uses `Tab`/`Shift+Tab`
and number keys instead.

**Decision: Intent hidden below 60 terminal columns**
On terminals narrower than 60 columns, displaying `<session-id> · <intent>` would leave
fewer than 20 characters for the intent after a typical 30-character session ID and the
3-character separator. The resulting display would be too truncated to be useful. Below 60
columns, only the session ID is shown. This is an extension of the R16 list-only mode
(below 80 columns) and resolves the narrow-terminal intent display question without a
separate config option.

**Decision: Gate condition shown inline in History tab — static definition, not runtime re-evaluation**
`GateEvaluated` events render a second line with the gate type and condition summary derived
from the compiled template. Alternatives considered: (1) no additional context — leaves
`context-exists` and `context-matches` gates with zero explanation on failure; (2) re-evaluate
the gate condition at render time to show "why it failed given current state" — rejected
because this renders current truth not historical truth, misleading operators investigating
past failures, and requires shell execution at render time; (3) collapsible section — adds
a second navigation model inside an already-scrolling tab for marginal space savings; (4)
show condition only on failure — pass context is equally useful when tracing progression
and selective rendering forces non-uniform scrolling. The static two-line format is
sufficient because the operator's question is always "what did this gate check?" — which
the definition answers without runtime evaluation. The compiled template is already loaded
for R6, so no new I/O is required.

**Decision: Session list ordered by health severity, recency as tiebreaker**
Sort key: failed (0) → blocked (1) → running (2) → unknown (3) → done (4), then most
recently active descending within each bucket. Alternatives considered: (1) most recently
active first (PRD draft) — conflates recency with urgency, forces the operator to mentally
re-sort by status; (2) alphabetical (current `--once` implementation) — no relationship to
urgency; (3) hybrid active-by-severity/done-by-recency — introduces an arbitrary boundary
that complicates both the implementation and the mental model; (4) configurable ordering —
premature for one well-defined primary use case, and `--once` consumers needing a different
order can sort the output themselves. The severity-first ordering directly answers the
operator's opening question ("which session needs attention?") without requiring a mental
re-sort across the list.

**Decision: Global session scope by default — no per-repo filtering**
The dashboard discovers all sessions on the local machine from the outset rather than
scoping to the current working directory. Alternatives considered: (1) repo-scoped by
default, global via `--global` flag; (2) configurable scope with a default setting.
Both alternatives were rejected because the primary use case — a developer monitoring
a dozen parallel niwa workflows from a fixed terminal — only works with global scope.
A `--scope` filter can be added later as a follow-on if users need to narrow the view.
The deeper reason for global-by-default is architectural: the koto observability roadmap's
F5 (S3-backed dashboard) extends F3's local scope to cloud storage. If F3 were
repo-scoped, F5 would need to redesign the scope model rather than just swapping the
storage backend.

**Decision: Schema additive — no version bump**
The codebase already uses `#[serde(default, skip_serializing_if = "Option::is_none")]` on
optional `StateFileHeader` fields and has round-trip tests that confirm old files parse
cleanly. Adding `intent`, `template_name`, and `EvidenceSubmitted.summary` as optional
fields following this pattern requires no schema version bump and no migration. The schema
version comment at `types.rs:13` explicitly documents that additive optional fields do not
require a version bump.
