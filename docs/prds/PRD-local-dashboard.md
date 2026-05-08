---
status: Done
problem: |
  koto users running hours-long orchestration workflows have no live visibility into what's
  happening. The available commands — `koto status <name>` and `koto workflows` — produce
  static JSON snapshots and require manual re-invocation. When a batch coordinator spawns
  hundreds of parallel child sessions, users must script their own polling loop or invoke
  `koto status` across every child manually.

  Beyond the basic visibility gap, the initial dashboard implementation had several broken
  behaviors: the elapsed column was hardcoded to "0s" regardless of actual runtime; the
  detail pane was gated on gate evaluations and returned empty for evidence-only sessions,
  which are the dominant workflow pattern; tree connectors were missing; expand/collapse was
  bound to the wrong mode; and depth was capped at one level. Rich narrative data —
  transition rationale, decision rationale, rewind rationale, gate override rationale — was
  stored in the event log but rendered nowhere. Session identifiers were opaque
  machine-generated slugs with no mechanism to associate a session with its human intent.
  Session discovery was scoped to the current working directory, so a developer monitoring
  parallel agentic workflows across multiple niwa workspaces saw only sessions from
  whichever repo they happened to launch the dashboard from.
goals: |
  A terminal UI dashboard (`koto dashboard`) that gives koto users live, complete visibility
  into all workflow sessions on their local machine: which sessions are running, which have
  failed, what each gate's current evaluation result is, and how a batch coordinator's
  children are progressing. The dashboard stays current while workflows run and requires no
  setup beyond running the command.

  The full implementation delivers: correct elapsed time; a detail pane that works for every
  session type (gate-based, evidence-based, hybrid); a three-tier information hierarchy (list
  view, tabbed detail pane with Summary / History / Remaining tabs, scrollable event
  timeline); human-readable intent labels and template names surfaced in the list;
  proper tree connectors with unlimited depth; worst-case child status rollup on collapsed
  parents; health-severity sort so blocked and failed sessions surface immediately; global
  session discovery across all niwa workspaces on the local machine; and a `--once` mode
  with six-column tab-separated output for integration tests and scripting.
source_issue: 366
---

# F3: Local Dashboard

## Status

Done

## Problem Statement

koto users running hours-long orchestration workflows have no live visibility into what's
happening. The available commands — `koto status <name>` and `koto workflows` — produce
static JSON snapshots and require manual re-invocation to observe changes. When a batch
coordinator spawns hundreds of parallel child sessions, tracking progress requires scripting
a polling loop or repeatedly invoking `koto status` across every child manually.

This gap is acute as koto workflows grow in scope. A full multi-phase pipeline
(explore → prd → design → plan → work-on) spans multiple nested sessions across phases
that each last minutes to hours. A user watching this pipeline run has no way to see which
phase is active, which child sessions are progressing versus blocked, or which gates are
failing.

The initial dashboard implementation addressed the basic scaffolding but left several things
broken or missing. The elapsed column was hardcoded to `Duration::from_secs(0)` — every
session showed "0s" regardless of actual runtime. The detail pane was gated on the presence
of a `GateEvaluated` event: evidence-only sessions, the dominant pattern in shirabe-style
workflows, permanently showed "No gate evaluations recorded." Tree rendering used space
indentation only, with no `├─`/`└─` connectors, expand/collapse bound to the wrong mode, and
depth capped at one level. The layout used a fixed 8-row vertical strip instead of a
horizontal split. Session discovery was scoped to the current working directory, making the
dashboard useless for a developer monitoring parallel agentic workflows across multiple niwa
workspaces.

Beyond the missing basics, rich narrative data already recorded by the koto engine was never
surfaced: `DecisionRecorded.rationale`, `DirectedTransition.rationale`, `Rewound.rationale`,
`GateOverrideRecorded.rationale`, `ContextAdded` artifacts, and `DefaultActionExecuted`
output. Session identifiers were opaque machine-generated slugs with no mechanism to
associate a session with its human intent without running `koto query`.

F3: Local Dashboard is the first tangible observability experience in koto. It is also the
foundation that makes F5 (S3-backed dashboard) and F6 (hosted relay) legible: a global,
directory-independent local scope means extending to cloud storage is a matter of swapping
the storage backend, not redesigning the scope model.

## Goals

- Give users a live view of all workflow sessions on their local machine without any setup
  beyond running `koto dashboard`, from any directory.
- Surface hierarchy (root → batch coordinator → child tasks) with proper tree connectors,
  unlimited depth, and worst-case status rollup on collapsed parents.
- Make failures immediately visible: health-severity sort (blocked first, then running, then
  done), failed count in aggregate rows, failed children sorted to the top on expansion.
- Surface all event data the koto engine already records: decision rationale, transition
  rationale, rewind rationale, gate override rationale, context artifacts, evidence
  summaries — visible in a tabbed detail pane.
- Identify sessions with human-readable intent labels and template names, set at init time
  or updated mid-workflow via `koto session update`.
- Support the target use case: monitoring a multi-hour pipeline
  (explore → prd → design → plan → work-on) where sessions may number in the tens to
  hundreds across multiple niwa workspaces.
- Provide a `--once` mode with six-column tab-separated output for scripting and integration
  tests without breaking existing scripts that read the first four columns.
- Establish a command interface that does not preclude a future daemon mode (`koto daemon
  start/stop`) without breaking changes.

## Non-Goals

- **Daemon / always-on mode**: No background process, no `koto daemon start/stop`. The
  command interface must not preclude this in V2 (top-level `koto dashboard` leaves room
  for `koto daemon` as a parallel top-level command).
- **Web-based dashboard**: Would require introducing tokio as a first async runtime
  dependency. Deferred to V2.
- **Remote access**: F5 (S3-backed) and F6 (hosted relay) scope.
- **Authentication or multi-user scenarios**: Sessions are scoped to the current user's
  `~/.koto/` directory.
- **Session management actions from dashboard**: No cancel, rewind, or override. Read-only
  view only.
- **inotify/kqueue file watching**: Polling is sufficient. File-system event subscriptions
  deferred to V2 optimization.
- **Session filtering / search**: The `/` filter pattern is deferred to a follow-on issue.
- **Rewind epoch navigation**: Browsing archived `~`-named epoch sessions is deferred.
- **Help overlay**: A `?` key overlay is out of scope; the keyboard reference is in the
  documentation.
- **Mouse support**: Keyboard only.
- **Export or logging of dashboard output**: Except for `--once` mode, no persistent output.
- **Full raw event log viewer**: Deep event inspection remains the domain of `koto query`.
  The History tab is a browsable summary, not a replacement.

## Requirements

### Functional Requirements

**R1 — Global session discovery**
`koto dashboard` discovers all koto sessions on the local machine regardless of the working
directory from which the command is invoked. Session discovery uses the global koto sessions
directory at `~/.koto/sessions/<name>/` (flat storage; migration helper runs on first use
to move any per-repo-scoped sessions into the flat layout). Sessions from different
workspaces appear in the same list.

**R2 — Session list with health-severity sort**
Sessions are ordered by health severity: failed (0) → blocked (1) → running (2) →
unknown (3) → done (4), with most recently active descending as a tiebreaker within each
bucket. This ordering applies to both the interactive TUI and `--once` output.

**R3 — Per-session row display**
Each session row displays:
- Session name (with tree prefix for children), truncated to 60 characters including any
  ` · <intent>` suffix
- Intent label (` · <intent>`) when set; only session ID shown when absent. Below 60
  terminal columns, only the session ID is shown.
- Template name as a distinct column; `"-"` when absent
- Current state name
- Elapsed time since last state transition (human-readable: `4h 12m`, `45m`, `2s`);
  calculated from `WorkflowInitialized` timestamp for sessions that have not yet transitioned
- Status indicator: `done`, `failed`, `blocked`, `running`, or `unknown`

**Status derivation rules** (evaluated in priority order):
1. `done` — current state has `terminal: true` in the compiled template
2. `failed` — at least one gate in the current state has a `gate_evaluated` event with
   `outcome: "failed"` in the current epoch, and the session is not terminal
3. `blocked` — the session has a `children-complete` gate that has not yet passed, and no
   failing gates, and the session is not terminal
4. `running` — session is active (not terminal) and has no failing or blocking gates
5. `unknown` — compiled template cannot be loaded

**R4 — Session hierarchy display**
Sessions are rendered in a tree derived from the `parent_workflow` header field. Tree
connectors use `├─ ` for non-final siblings and `└─ ` for the final sibling in a group.
Root-level sessions have no connector prefix. The tree renders to at least 5 levels of
nesting; depth is determined by session data, not a compile-time cap. Collapsed parent rows
show a child-count badge `[N]` after the session name (direct children only, not all
descendants).

**R5 — Batch coordinator aggregate row**
When a session has child sessions, its display includes an inline aggregate row:
`N tasks · X done · Y failed · Z pending`
where failed count is rendered in red when Y > 0.

| Child session status | Aggregate bucket |
|---------------------|-----------------|
| `done` (terminal) | done |
| `failed` (gate evaluation failed) | failed |
| `unknown` (template unavailable) | failed |
| `blocked` (children-complete gate unsatisfied) | pending |
| `done_blocked` (terminal blocked state) | pending |
| `running` | pending |

**R6 — Default collapse for large batches**
Sessions with more than 5 children are collapsed by default; ≤5 children are expanded by
default. Auto-collapse is evaluated once at initial render. A user who manually expands a
batch that then grows to 1000 children during a long run remains expanded until they
collapse it.

**R7 — Failure-first child sorting**
Expanded batch children are sorted: failed first, then blocked, then running, then pending,
then done/skipped. Within each group, sorted by name.

**R8 — Parent status rollup**
When a parent is collapsed, its displayed status reflects the worst-case status across all
descendants:
1. Any descendant is `failed` → parent shows `failed`
2. Any descendant is `blocked` or `done_blocked` and no `failed` descendants → parent
   shows `blocked`
3. All descendants are `done` and parent is itself terminal → parent shows `done`
4. All descendants are terminal but set includes both `done` and `done_blocked` → parent
   shows `blocked`
5. Otherwise → parent shows `running`; if parent itself has `unknown` status and no
   descendants have `failed` or `blocked`, parent shows `unknown`

When expanded, each row shows its own independent status.

**R9 — Width-driven layout**
- ≥80 columns: horizontal split, list at 40%, detail pane at 60%
- <80 columns: list-only view (detail pane hidden)
- <40 columns: "terminal too narrow" message

**R10 — Tabbed detail pane — Summary tab**
Default tab when entering Detail view. Displays:
- Current state name
- The `directive` field from the current state's compiled template entry
- Most recent evidence submission: evidence fields as `key: value` pairs; optional
  `summary` string (from R16) rendered in bold above raw fields when present
- Gate result if evaluated in current epoch: gate name, result (PASS/FAIL), command
- Session `intent` (from R15) and `template_name` (from R16) when present

Sessions with no data beyond `WorkflowInitialized` show "No data yet" — not an error or
blank space. "No gate evaluations recorded" must not appear for any session that has
submitted evidence.

**R11 — Tabbed detail pane — History tab**
Scrollable tab showing all events for the selected session in chronological order, each
prefixed with `[YYYY-MM-DD HH:MM:SS]`. Minimum event types rendered:

| Event type | Fields to render |
|---|---|
| `WorkflowInitialized` | template name, initial variables |
| `StateTransitioned` | from-state, to-state, trigger |
| `EvidenceSubmitted` | state, fields (key: value), `summary` if present |
| `GateEvaluated` | gate name, result (PASS/FAIL); second line with gate type and condition summary from compiled template: `command` gates show `cmd: <command>`; `context-exists` shows `key: <key>`; `context-matches` shows `key: <key>  pattern: <pattern>`; `children-complete` shows `children: <completed>/<total> complete`. Condition line silently omitted when compiled template is unavailable. |
| `DecisionRecorded` | decision text, rationale |
| `DirectedTransition` | target state, rationale |
| `Rewound` | from-state, to-state, rationale |
| `GateOverrideRecorded` | gate name, override result, rationale |
| `ContextAdded` | key, artifact reference |
| `DefaultActionExecuted` | action type, first 3 newline-delimited lines of output |

Unknown event types render as `[Unknown event: <type>]` rather than crashing or being
silently omitted. Switching to a different session resets History tab scroll to the top.

**R12 — Tabbed detail pane — Remaining tab**
Lists states in the session's compiled template not yet visited (no `StateTransitioned`
event with that destination in the current epoch). States listed in topological order for
DAG templates; definition order otherwise. When the compiled template is unavailable:
> Template unavailable — run `koto template compile <path>` to restore the remaining-states view.

A terminal session shows an empty list or "All states visited."

**R13 — Gate evaluation panel**
The Summary tab shows the most recent evaluation result per gate in the current state. Each
gate row displays: gate name, gate type (`command`, `context-exists`, `context-matches`, or
`children-complete`), last outcome (`passed` / `failed` / `—` if never evaluated in current
epoch), and key output field(s):
- Command gate: exit code and error message if any
- Context gates: the `exists` or `matches` boolean
- Children-complete gate: `all_complete`, `any_failed`, and child counts

**R14 — Epoch-branched session filtering**
Sessions whose name contains `~` are hidden from the session list. In the focused parent
session view, a collapsed "Archived epochs" row shows the count of archived epoch branches.

**R15 — `intent` on session state**
`StateFileHeader` carries `intent: Option<String>`. Set at initialization via
`koto init --intent "<text>"` or updated on an existing session via
`koto session update <name> --intent "<text>"`. Both write atomically. Older state files
without this field deserialize without error; field defaults to `None`.

**R16 — `template_name` and `EvidenceSubmitted.summary`**
`StateFileHeader` carries `template_name: Option<String>`, written from the compiled
template's `name` field at `koto init` time. Read from the state file header on each
refresh without loading the compiled template from disk. `EvidenceSubmitted` carries
`summary: Option<String>`. Agents submit it via `--with-data '{"summary": "...", ...}'`.
Pre-feature payloads without either field deserialize without error.

**R17 — Live polling**
The dashboard polls at 1 second by default. Poll interval is configurable via
`--interval <ms>`. A header row shows time since last successful poll refresh.

**R18 — One-shot mode**
`koto dashboard --once` outputs current dashboard state to stdout as plain text and exits
with code 0. Output format: one session per line, 6 tab-separated columns:

```
session_id\tcurrent_state\telapsed\tstatus_bucket\tintent\ttemplate_name
```

Columns 1–4 are the original columns and remain unchanged. `intent` (column 5) is empty
string when not set. `template_name` (column 6) is empty string when not set. No ANSI color
codes. Health-severity sort applies. `koto dashboard --once` with no sessions exits with
code 0 and produces no output lines.

`koto dashboard <name> --once` outputs only that session and its children. If the named
session does not exist, exits with code 1 and a message on stderr.

**R19 — Navigation and key bindings**

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
| `Shift+Tab` | Cycle to previous tab |
| `1` | Jump to Summary tab |
| `2` | Jump to History tab |
| `3` | Jump to Remaining tab |
| `PageDown` or `Ctrl+D` | Scroll down in current tab |
| `PageUp` or `Ctrl+U` | Scroll up in current tab |
| `h` or `←` | No-op |
| `l` or `→` | No-op |

**R20 — Error handling**
- Truncated final JSONL line: treat as if the line doesn't exist; use the previous complete event
- Unknown event type in JSONL log: skip without error
- Session with no `workflow_completed` event: treat as running
- Compiled template unavailable: session shows `unknown` status; no crash

**R21 — Outside-repository behavior**
`koto dashboard` with no sessions on the local machine shows an empty list with
"No sessions found" rather than an error. If a repo-id cannot be derived, exits with code 1
and a message on stderr.

### Non-Functional Requirements

**R22 — Startup time**
`koto dashboard` (no arguments) must display the initial session list within 1 second for
≤100 sessions. `koto dashboard <name>` must display the initial session view within 100ms
for a session with ≤1000 events.

**R23 — Refresh performance**
Each full refresh cycle — reading all session state files and rendering all visible rows —
must complete in under 200ms for a session set with up to 500 events per JSONL log.

**R24 — No async runtime**
The implementation must not introduce an async runtime (tokio, async-std, etc.). koto is
synchronous by design.

**R25 — Single binary**
The dashboard is part of the existing `koto` binary. No additional binaries or daemon
processes required.

**R26 — Backwards compatibility**
State files predating this feature (without `intent`, `template_name`, or
`EvidenceSubmitted.summary`) must deserialize without errors. These fields default to `None`.
The schema version remains 1; no migration is required. Each new optional field has a
round-trip test confirming old files parse cleanly and re-serializing omits absent fields.

## Acceptance Criteria

### Session display

- [x] `koto dashboard` opens a TUI showing all sessions on the local machine regardless of
      the working directory
- [x] Sessions are ordered by health severity (failed → blocked → running → unknown → done),
      with most recently active as tiebreaker within each bucket
- [x] Sessions are displayed in a tree with `├─`/`└─` connectors; sessions with
      `parent_workflow` are indented under their parent
- [x] A session with children shows an aggregate row `N tasks · X done · Y failed · Z
      pending`; failed count is red when non-zero
- [x] A coordinator with >5 children shows only the aggregate row by default (children
      collapsed); a collapsed parent shows `[N]` badge for direct child count
- [x] A coordinator with ≤5 children shows its children expanded by default
- [x] Expanded children are sorted: failed first, then blocked, then running, then pending,
      then done/skipped
- [x] A session in a terminal state shows status `done`
- [x] A parent with a `failed` descendant shows `failed` status when collapsed
- [x] A parent with a `done_blocked` terminal descendant (and no `failed` descendants) shows
      `blocked` status when collapsed
- [x] A parent whose all descendants are `done` and is itself terminal shows `done` when
      collapsed
- [x] `intent` appears alongside the session ID in the list when set; no separator shown
      when absent
- [x] `template_name` appears as a distinct column; `"-"` when absent
- [x] Sessions with `~` in their name do not appear in the list
- [x] The focused parent view shows a collapsed "Archived epochs" row when epoch-branched
      children exist

### Layout

- [x] On a terminal ≥80 columns, the session list and detail pane are side-by-side (40/60
      split)
- [x] On a terminal <80 columns, dashboard shows list-only view (detail pane hidden)
- [x] On a terminal <40 columns, dashboard shows "terminal too narrow"

### Detail pane — Summary tab

- [x] A session with only evidence submissions shows a non-empty Summary tab with evidence
      fields rendered as `key: value` pairs
- [x] A session with only gate evaluations shows gate name, result, and command in the
      Summary tab
- [x] A session with both evidence and gate evaluations shows both sections
- [x] A newly initialized session (no events beyond `WorkflowInitialized`) shows "No data
      yet" in the Summary tab
- [x] No session shows "No gate evaluations recorded" in place of actual evidence data
- [x] The directive text for the current state is visible
- [x] When `EvidenceSubmitted` has a `summary` field, it appears above the raw `key: value`
      pairs
- [x] `intent` and `template_name` appear in the Summary tab when set

### Detail pane — History tab

- [x] All 10 event types from R11 are rendered with their relevant fields
- [x] Each event row includes a `[YYYY-MM-DD HH:MM:SS]` timestamp prefix
- [x] A `GateEvaluated` event for a `command` gate renders a second line showing
      `cmd: <command>` from the compiled template
- [x] A `GateEvaluated` event for a `context-exists` gate renders `key: <key>`
- [x] A `GateEvaluated` event for a `children-complete` gate renders
      `children: <completed>/<total> complete`
- [x] When the compiled template is unavailable, `GateEvaluated` renders only the name and
      result line — no crash, no error, no blank line
- [x] An unknown event type renders as `[Unknown event: <type>]` and does not crash
- [x] Events appear in chronological order (oldest first)
- [x] The History tab is scrollable; `PageDown`/`Ctrl+D` reaches content below visible area
- [x] After scrolling past the last event, the scroll offset does not increase further
- [x] Switching focus to a different session resets History tab scroll to the top

### Detail pane — Remaining tab

- [x] An active session lists at least one unvisited state
- [x] A terminal session shows an empty Remaining tab or "All states visited"
- [x] For a DAG template with states at equal topological depth, states are listed in
      definition order
- [x] When the compiled template is unavailable, the exact fallback message from R12 is shown

### `intent` and `template_name`

- [x] `koto init --intent "investigating issue #42"` persists `intent` in the state file
      header
- [x] `koto session update <name> --intent "new text"` overwrites `intent` without modifying
      other fields
- [x] Running `koto session update` on a session without `intent` adds the field; running it
      with `intent` already set replaces the value
- [x] A session without intent shows only the session ID (no `·` separator)
- [x] `template_name` appears for sessions whose template has a `name:` field in frontmatter
- [x] State files created before this feature (missing `intent`, `template_name`) load
      without errors

### Navigation and keyboard bindings

- [x] `l`/`→` expands a collapsed parent in List view; no-op on a leaf session
- [x] `h`/`←` collapses an expanded parent in List view; no-op on a leaf session
- [x] `Enter` in List view enters Detail view for the focused session
- [x] `Esc` returns to List view from Detail view
- [x] `Tab` cycles through the three tabs in Detail view (Summary → History → Remaining →
      Summary)
- [x] `Shift+Tab` cycles in reverse
- [x] `1`, `2`, `3` jump directly to Summary, History, and Remaining tabs
- [x] `PageDown`/`Ctrl+D` scrolls down in the History tab
- [x] `j`/`↓` in Detail view moves to the next session without leaving Detail view
- [x] `k`/`↑` in Detail view moves to the previous session without leaving Detail view
- [x] `r` triggers an immediate refresh in any view
- [x] `h`/`←` and `l`/`→` in Detail view are no-ops (no crash, no mode change)
- [x] `q` or `Ctrl+C` exits with code 0

### Edge cases

- [x] A JSONL file with a truncated final line is read without crashing; the truncated line
      is ignored
- [x] An unknown event type in the JSONL log is skipped; the dashboard does not crash
- [x] `koto dashboard` with no sessions shows an empty list with an explanatory message
- [x] A session started before `koto dashboard` was launched shows complete history (elapsed
      from session creation, not dashboard launch)
- [x] The elapsed column never shows "0s" for a session that transitioned more than 10
      seconds ago

### One-shot mode

- [x] `koto dashboard --once` outputs exactly 6 tab-separated columns per line
- [x] Column 4 (`status_bucket`) is one of: `running`, `blocked`, `done`, `failed`,
      `unknown`
- [x] Column 5 (intent) is empty string for sessions without intent
- [x] Column 6 (template_name) is empty string for sessions without template_name
- [x] A script reading only columns 1–4 continues to produce correct output
- [x] `koto dashboard <name> --once` outputs only that session and its children
- [x] `koto dashboard nonexistent --once` exits with code 1 and an error on stderr
- [x] `koto dashboard --once` with no sessions exits with code 0 and produces no output lines

### Global session scope

- [x] `koto dashboard` launched from `/workspace-a` displays sessions from `/workspace-b`
- [x] `koto dashboard --once` launched from a directory with no local sessions outputs rows
      for sessions from other workspaces on the machine
- [x] A session started in a new workspace after the dashboard is running appears on the
      next polling cycle
- [x] Health-severity sort applies consistently to TUI and `--once` output

### Backwards compatibility

- [x] State files created by older koto versions (missing `intent`, `template_name`,
      `EvidenceSubmitted.summary`) load without a parse error
- [x] Round-trip serialization of a `StateFileHeader` without `intent` or `template_name`
      omits those fields (no null values written to disk)

### Performance

- [x] Startup for `koto dashboard` with 100 sessions completes within 1 second
- [x] Startup for `koto dashboard <name>` with ≤1000 events completes within 100ms
- [x] Full refresh for a session with 500 events completes within 200ms

## Known Limitations

- **Terminal state requires template coupling**: The absence of a `workflow_completed` event
  means the dashboard must load the compiled template for each session to determine terminal
  status. If the compiled template is deleted or moved after session creation, the session
  shows `unknown` status. This matches the same limitation in `koto status`.
- **Remaining tab requires local template cache**: Sessions imported from another machine or
  whose cache was cleared show the "Template unavailable" fallback.
- **`intent` is human-editable but not auto-generated**: The dashboard renders intent when
  present but cannot require agents to provide it. Sessions without intent fall back to the
  session ID.
- **`EvidenceSubmitted.summary` is agent-optional**: The dashboard renders it when present
  but cannot require agents to provide it. Evidence without a summary falls back to raw
  field display.
- **Epoch-branched session drill-down deferred**: The focused parent view shows an
  "Archived epochs" summary row but does not expand it. Users who need to inspect archived
  epoch children must use `koto status` or `koto workflows` directly.

## Decisions and Trade-offs

**D1 — Rendering technology: ratatui over embedded web server**
ratatui is a pure-Rust TUI with no external system dependencies and no async runtime
requirement. An embedded web server (axum, actix) would introduce tokio — the first async
runtime dependency in koto, which is intentionally synchronous. A web-based dashboard is
deferred to V2 if there is demand for a preview of F5/F6-style remote access.

**D2 — Invocation model: ad-hoc command over daemon**
Sessions accumulate JSONL log data from `koto init` regardless of whether a dashboard is
running, so launching mid-session gives complete history. This "data always accumulates"
property means an always-on daemon is a convenience rather than a data-completeness
requirement. The ad-hoc model avoids daemon lifecycle management and stays compatible with
V2 daemon integration: `koto daemon start/stop` can be added as separate top-level commands
without conflict.

**D3 — Terminal state detection: template-loading over heuristics**
Loading the compiled template at `MachineState.template_path` and checking `terminal: true`
is how `koto status` determines terminal status. The dashboard uses the same mechanism for
consistency. Timeout or last-transition heuristics produce false positives and false
negatives; template-loading does not.

**D4 — Universal detail pane — no session-type discrimination**
The Summary tab renders all available data regardless of session type, with absent sections
silently omitted. Alternatives — separate gate-only and evidence-only tabs, or conditional
UI — require operators to understand which session type they're looking at. The goal is
monitoring, not diagnostics.

**D5 — `intent` supplements the session ID rather than replacing it**
The session ID is the key used in all `koto next <name>` / `koto rewind <name>` commands.
Format `<session-id> · <intent>` with 60-character truncation on the intent portion
preserves the ID while adding context.

**D6 — `--once` columns appended, not interleaved**
New columns are at positions 5 and 6. Interleaving before `status_bucket` would break
existing parsers reading column 4. Empty string (not null or placeholder) is used for absent
values so `awk '{ print $5 }'` parsing works consistently.

**D7 — Status rollup uses worst-case child status**
`failed > blocked > running > done` matches the convention used by monitoring tools and
surfaces the most actionable condition first. The rollup applies only when the parent is
collapsed.

**D8 — `l`/`h` for tree expand/collapse in List view; no-op in Detail view**
These bindings follow the vim hjkl convention where `h`/`l` mean directional navigation
(shallower/deeper in the tree). In Detail view, `h`/`←` and `l`/`→` are no-ops rather than
being rebound to tab navigation, which would create two different actions for the same key
in adjacent modes.

**D9 — Global session scope by default**
The dashboard discovers all sessions on the local machine from the outset. The primary use
case — a developer monitoring a dozen parallel niwa workflows from a fixed terminal — only
works with global scope. Repo-scoped default would need a `--global` flag, adding friction
for the primary use case. Architecturally, global-by-default at the local level makes F5
(S3-backed dashboard) a storage-backend swap rather than a scope-model redesign.

**D10 — Health-severity sort over recency sort**
Severity-first ordering (failed → blocked → running → unknown → done, recency as tiebreaker)
directly answers the operator's opening question ("which session needs attention?") without
requiring a mental re-sort. Alternatives considered: most recently active first (conflates
recency with urgency), alphabetical (no relationship to urgency), hybrid (arbitrary boundary
that complicates both implementation and mental model). `--once` consumers needing a
different order can sort the output themselves.

**D11 — Schema additive, no version bump**
`intent`, `template_name`, and `EvidenceSubmitted.summary` are added as optional fields
using `#[serde(default, skip_serializing_if = "Option::is_none")]`, following the established
pattern in the codebase. No schema version bump required. The schema version comment at
`types.rs:13` explicitly documents that additive optional fields do not require a bump.

**D12 — Epoch-branched session handling**
Sessions with `~` in their name are hidden from the session list. The focused parent view
shows an "Archived epochs" summary row (not expandable in this release). Without filtering,
rewinding a batch parent multiple times produces accumulated epoch branches that make the
list unreadable. The `~` character is reserved for epoch branching and cannot appear in
user-specified workflow names.

**D13 — Command name: `koto dashboard`**
Top-level `koto dashboard [<name>]` parallels existing top-level commands (`koto status`,
`koto workflows`) and leaves room for `koto daemon` as a parallel top-level command. Nesting
under `session` produces an awkward invocation and doesn't fit V2 daemon patterns.
Alternative names like `observe` or `watch` are more generic and less discoverable.
