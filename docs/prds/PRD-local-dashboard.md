---
status: Accepted
problem: |
  koto users running hours-long orchestration workflows — AI coding pipelines spanning
  multiple phases, batch jobs with hundreds of parallel tasks, and eventually full
  multi-skill sequences (explore → prd → design → plan → work-on) — have no live
  visibility into what's happening. The only tools available are `koto status <name>`
  and `koto workflows`, which produce static JSON snapshots and require manual
  re-invocation. When a batch job has 200 child sessions running in parallel, users
  are effectively blind: they can't see which children are running, which are blocked,
  which have failed, or what the current gate state is — without scripting their own
  polling loop.
goals: |
  A terminal UI dashboard (`koto dashboard`) that gives koto users live visibility into
  their workflow sessions: which sessions are running, which have failed, what each
  gate's current evaluation result is, and how a batch coordinator's children are
  progressing. The dashboard stays current while workflows run and requires no setup
  beyond running the command. Success means a user monitoring a 1000-task batch can
  immediately see how many tasks are succeeding and failing, identify which specific
  tasks need attention, and understand the gate state blocking completion — all without
  leaving the terminal or writing any polling scripts.
source_issue: 366
---

# F3: Local Dashboard

## Status

Accepted

## Problem Statement

koto users running hours-long orchestration workflows have no live visibility into what's
happening. The available commands — `koto status <name>` and `koto workflows` — produce
static JSON snapshots and require manual re-invocation to observe changes. When a batch
coordinator spawns 200 parallel child sessions, a user wanting to track progress must
either script their own polling loop or repeatedly invoke `koto status` across every child
session manually.

This gap becomes acute as koto workflows grow in scope. Today the `/work-on` skill runs a
single session per issue. In the future, the full multi-phase pipeline
(explore → prd → design → plan → work-on) will be koto-managed, spanning multiple nested
sessions across phases that each last minutes to hours. A user watching this pipeline run
has no way to see which phase is active, which child work-on sessions are progressing
versus blocked, or which gates are failing — without disruptive polling.

F3: Local Dashboard is the first tangible observability experience that addresses this gap.
It is also the foundation that makes F5 (S3-backed dashboard) and F6 (hosted relay)
legible: a user who has watched a batch pipeline in a local TUI understands exactly why
remote dashboard access is valuable.

## Goals

- Give users a live view of workflow sessions for their current repository without any
  setup beyond running `koto dashboard`.
- Surface hierarchy (root → batch coordinator → child tasks) in a single readable view, so
  users understand the relationship between sessions at a glance.
- Make failures immediately visible: failed task count in the aggregate row, failed children
  sorted to the top on expansion, gate failure details in the focused view.
- Support the target use case: monitoring a multi-hour pipeline (the full
  explore → prd → design → plan → work-on sequence) where sessions may number in the tens
  to hundreds.
- Establish a command interface that does not preclude a future daemon mode (`koto daemon
  start/stop`) without breaking changes.

## User Stories

**UC-1: Monitoring a batch workflow**
A user submits 1000 tasks to a batch coordinator at 9am and returns at 11am. They run
`koto dashboard` and immediately see the coordinator's aggregate row: `1000 tasks · 847
done · 12 failed · 141 pending`. The failed count is red. They press Enter to expand the
coordinator and see the 12 failed tasks sorted to the top of the child list. They navigate
to a failing task and see which gate is blocking it.

**UC-2: Watching a multi-phase pipeline**
A user kicks off a full orchestration run (`koto init orchestrator ...`) that will run for
4+ hours through explore → prd → design → plan → work-on phases. They run `koto dashboard`
and see the root orchestrator session plus the currently active phase session. As the
pipeline progresses, the dashboard updates automatically; the user doesn't need to do
anything.

**UC-3: Investigating a stalled session**
A user notices a session hasn't advanced in 30 minutes. They run `koto dashboard <name>` to
open the focused view. They see the current state name, the gates blocking it, and the last
gate evaluation result. They can tell immediately whether the gate failed (command gate
returned exit code 1) or is waiting on children (children-complete gate not yet satisfied).

**UC-4: Mid-session launch**
A workflow has been running for 2 hours before the user thinks to check on it. They run
`koto dashboard`. The dashboard shows the full session history — all elapsed time, all
transitions — because koto has been writing the JSONL log since `koto init`. The user gets
complete context with no data loss.

**UC-5: Scripting and automation**
A CI harness or agent monitor wants to poll dashboard state without a TUI. The user runs
`koto dashboard --once` and gets a one-shot snapshot of current session state that can be
piped to downstream tooling.

## Requirements

### Functional Requirements

**R1 — Repo-wide session list (`koto dashboard`)**
Running `koto dashboard` without arguments opens a terminal UI showing all sessions for the
current repository. Sessions are discovered by scanning `~/.koto/sessions/<repo-id>/` where
`repo-id` is derived from the current working directory (same derivation as `koto init`).

**R2 — Focused single-session view (`koto dashboard <name>`)**
Running `koto dashboard <name>` opens a focused view for that specific session, showing its
current state, elapsed time, gate panel, evidence timeline, and (if applicable) child task
list.

**R3 — Session hierarchy display**
The repo-wide view renders sessions in a tree derived from the `parent_workflow` header
field in each session's state file:
- A session with no `parent_workflow` is a root session (indented at level 0)
- A session that other sessions name as their `parent_workflow` is a coordinator (displayed
  with an aggregate row, described in R5)
- A session with a `parent_workflow` and no children of its own is a leaf task

Sessions are connected with ASCII tree connectors (`├──`, `╰──`). A session can be both a
child (has `parent_workflow`) and a coordinator (has children) — for example, a phase
session spawned by a root orchestrator that itself spawns work-on sessions. Tree rendering
is limited to 3 visible indentation levels; sessions deeper than level 3 are rendered at
level 3 indent without further nesting. Session names longer than 40 characters are
truncated with `…` in the repo-wide list.

Example structure:
```
orchestrator                (root, level 0)
├── design                  (coordinator, level 1 — has children)
│   ├── design.issue-1      (leaf, level 2)
│   └── design.issue-2      (leaf, level 2)
└── plan                    (leaf, level 1 — no children yet)
```

**R4 — Per-session row display**
Each session row displays:
- Session name (with tree prefix for children)
- Current state name (from the most recent `transitioned`, `directed_transition`, or
  `rewound` event)
- Elapsed time since session creation (human-readable: `4h 12m`, `45m`, `2s`)
- Status indicator (one of: `done`, `failed`, `blocked`, `running`, `unknown`)

**Status derivation rules** (evaluated in priority order):
1. `done` — current state has `terminal: true` in the compiled template
2. `failed` — at least one gate in the current state has a `gate_evaluated` event with
   `outcome: "failed"` in the current epoch, and the session is not terminal
3. `blocked` — the session has a `children-complete` gate that has not yet passed, and no
   failing gates, and the session is not terminal
4. `running` — session is active (not terminal) and has no failing or blocking gates
5. `unknown` — compiled template cannot be loaded

If the compiled template is unavailable, the status is `unknown` (no crash).

**R5 — Batch coordinator aggregate row**
When a session has child sessions, its display includes an inline aggregate row showing:
`N tasks · X done · Y failed · Z pending`
where failed count is rendered in red when Y > 0.

The dashboard derives child status by applying R4's status rules to each child session.
The aggregate buckets map as follows:

| Child session status | Aggregate bucket |
|---------------------|-----------------|
| `done` (terminal) | done |
| `failed` (gate evaluation failed) | failed |
| `unknown` (template unavailable) | failed |
| `blocked` (children-complete gate unsatisfied) | pending |
| `running` | pending |

Total N = done + failed + pending. Skipped (terminal via a "skip" state) and
spawn_failed children are included via their derived status: skipped sessions are
terminal → done; spawn_failed sessions typically have a failed gate → failed.

**R6 — Default collapse for large batches**
When a session has more than 5 children, the children are collapsed behind the aggregate
row by default. The user presses Enter to expand. When a session has 5 or fewer children,
they are expanded by default.

The collapse threshold is evaluated once at initial render based on the child count at
that moment. Subsequent child additions during polling do not trigger auto-collapse of an
already-expanded batch. If a user manually expands a batch and the count grows to 1000
during a long run, the batch remains expanded until the user collapses it.

**R7 — Failure-first child sorting**
When a batch coordinator's children are expanded, they are sorted by status priority:
1. Failed (red)
2. Blocked (yellow)
3. Running (actively executing gates or actions)
4. Pending (waiting to begin)
5. Done / skipped (dim)

Within each status group, children are sorted by name.

**R8 — Gate evaluation panel**
In the focused single-session view, a gate panel shows the most recent evaluation result
for each gate in the current state. Gate data is sourced from Tier 2 `gate_evaluated`
events via the epoch-scoped `derive_last_gate_evaluated()` function.

Each gate row displays:
- Gate name (from the template's state definition)
- Gate type: `command`, `context-exists`, `context-matches`, or `children-complete`
- Last outcome: `passed` (green) or `failed` (red), or `—` if never evaluated in the
  current epoch
- Key output field(s):
  - Command gate: exit code and error message (if any)
  - Context gates: the `exists` or `matches` boolean
  - Children-complete gate: `all_complete`, `any_failed`, and child counts

Gates that have never been evaluated in the current epoch are shown with outcome `—`.

**R9 — Evidence submission timeline**
The focused view includes an evidence timeline listing the most recent `evidence_submitted`
events for that session (up to 10 entries). Each entry shows: submission timestamp, state
at submission, and a truncated preview of the evidence content (first 80 Unicode codepoints,
with embedded newlines replaced by spaces, and `…` appended when truncated).

**R10 — Terminal state detection**
A session is marked as `done` when its current state (derived from the most recent
transition event) corresponds to a state with `terminal: true` in the compiled template.
The dashboard must load the compiled template to make this determination; no
`workflow_completed` event exists in the session feed.

If the compiled template cannot be found at the path recorded in the session's machine
state, the session status is shown as `unknown` rather than crashing.

**R11 — Epoch-branched session filtering**
Sessions whose name contains `~` (epoch-branched sessions created by `koto rewind`) are
hidden from the repo-wide session list. In the focused parent session view, a collapsed
"Archived epochs" row shows the count of archived epoch branches; it does not expand in
the MVP.

**R12 — Live polling**
The dashboard polls for session updates at a default interval of 500ms. The poll interval
is configurable via `--interval <ms>`. A header row shows the time since the last
successful poll refresh.

**R13 — One-shot mode**
Running `koto dashboard --once` (with or without a session name) outputs the current
dashboard state to stdout as plain text and exits with code 0. This mode is intended for
scripting and non-interactive contexts.

Output format: one session per line. Fields are tab-separated in order: NAME, STATE,
ELAPSED, STATUS. Children are output after their parent, sorted by R7's priority order.
No ANSI color codes. No tree connector characters.

Example output:
```
orchestrator	exploring	4h 12m	running
design	in-progress	45m	running
design.issue-1	implementing	12m	running
design.issue-2	waiting	8m	pending
plan	pending	--	pending
```

`koto dashboard <name> --once` outputs only the named session and its children (same
format). If the named session does not exist, exits with code 1 and a message on stderr.

**R14 — Navigation and exit**
The dashboard supports the following key bindings:

| Key | Action |
|-----|--------|
| `j` / `↓` | Move cursor down |
| `k` / `↑` | Move cursor up |
| `g` / `Home` | Jump to first entry |
| `G` / `End` | Jump to last entry |
| `Enter` | Expand/collapse children |
| `r` | Force immediate refresh |
| `q` | Quit |
| `?` | Show key bindings help |
| `Ctrl+C` | Quit (always works) |

**R15 — Error handling for incomplete data**
The dashboard handles the following edge cases without crashing:
- Truncated final JSONL line (file written mid-event): treated as if the line doesn't
  exist; uses the previous complete event
- Unknown event type in the JSONL log: skipped with no error
- Session in progress with no `workflow_completed` event: treated as running

### Non-Functional Requirements

**R16 — Startup time (repo-wide)**
`koto dashboard` (no arguments) must display the initial session list within 1 second when
the repository has ≤100 sessions.

**R17 — Startup time (focused)**
`koto dashboard <name>` must display the initial session view within 100ms for a session
with ≤1000 events.

**R18 — Update latency**
New events written to a session's JSONL log must be visible in the dashboard within 2×
the poll interval (≤1 second at the default 500ms setting).

**R19 — No async runtime**
The implementation must not introduce an async runtime (tokio, async-std, etc.). koto is
synchronous by design; this constraint must be maintained.

**R20 — Single binary**
The dashboard must be part of the existing `koto` binary. No additional binaries or daemon
processes are required.

**R21 — Minimum terminal dimensions**
The dashboard renders correctly on terminals ≥80 columns × 24 rows. On terminals below this
size, the dashboard displays a "terminal too small (min 80×24)" message and waits for
the terminal to be resized; it does not crash. `koto dashboard --once` is not subject to
this constraint (it outputs plain text regardless of terminal size).

**R22 — Outside-repository behavior**
If `koto dashboard` is run from a directory with no `~/.koto/sessions/<repo-id>/` directory
(no sessions exist for the current repository), the dashboard displays an empty session list
with a "No sessions for this repository" message rather than an error. If the repo-id cannot
be derived (e.g., not a valid directory), it exits with code 1 and a message on stderr.

## Acceptance Criteria

**Session display**
- [ ] `koto dashboard` opens a TUI showing all sessions for the current repository
- [ ] Sessions are displayed in a tree with ASCII connectors; sessions with `parent_workflow`
      are indented under their parent
- [ ] A session with children shows an aggregate row `N tasks · X done · Y failed · Z
      pending`; failed count is red when non-zero
- [ ] A coordinator with >5 children shows only the aggregate row by default (children
      collapsed)
- [ ] A coordinator with ≤5 children shows its children expanded by default
- [ ] Pressing Enter on a collapsed coordinator expands it; pressing Enter again collapses it
- [ ] Expanded children are sorted: failed first, then blocked, then running, then pending,
      then done/skipped
- [ ] A session in a terminal state shows status `done`; the status matches `koto status
      <name>` terminal detection

**Focused view**
- [ ] `koto dashboard <name>` opens a focused view for that specific session
- [ ] The focused view shows the current state name matching what `koto status <name>`
      reports
- [ ] The focused view shows a gate panel with last evaluation result per gate in the current
      state (gate name, type, outcome, and key output field)
- [ ] The focused view shows an evidence timeline with the last ≤10 `evidence_submitted`
      events; each entry shows timestamp, state, and a truncated preview

**Filtering and edge cases**
- [ ] Sessions with `~` in their name do not appear in the repo-wide list
- [ ] The focused parent view shows a collapsed "Archived epochs" row when epoch-branched
      children exist
- [ ] A JSONL file with a truncated final line is read without crashing; the truncated line
      is ignored
- [ ] An unknown event type in the JSONL log is skipped; the dashboard does not crash
- [ ] `koto dashboard` with no sessions for the current repo shows an empty list with an
      explanatory message (no error)

**Navigation and exit**
- [ ] Pressing `q` or `Ctrl+C` exits with code 0 and no error output
- [ ] A session started before `koto dashboard` was launched shows complete history (elapsed
      time from session creation, not from dashboard launch)
- [ ] The header row shows the time since the last successful poll refresh

**One-shot mode**
- [ ] `koto dashboard --once` outputs one session per line (tab-separated: NAME, STATE,
      ELAPSED, STATUS) and exits with code 0
- [ ] `koto dashboard <name> --once` outputs only that session and its children
- [ ] `koto dashboard nonexistent-name --once` exits with code 1 and an error on stderr
- [ ] `koto dashboard nonexistent-name` (TUI mode) exits with code 1 and an error message

**Configuration**
- [ ] `koto dashboard --interval 1000` sets the poll interval to 1000ms
- [ ] `koto dashboard --interval 100` polls every 100ms without error

**Performance**
- [ ] Startup for `koto dashboard` with 100 sessions completes within 1 second (first frame
      painted)
- [ ] Startup for `koto dashboard <name>` with ≤1000 events completes within 100ms

**Terminal constraints**
- [ ] On a terminal <80 columns or <24 rows, the dashboard shows a "terminal too small"
      message and does not crash

## Out of Scope

- **Daemon / always-on mode**: No background process, no `koto daemon start/stop`. The
  command interface must not preclude this in V2 (top-level `koto dashboard` leaves room
  for `koto daemon` as a parallel top-level command).
- **Web-based dashboard**: Requires introducing an async runtime (tokio) as a first
  dependency. Deferred to V2 if demand emerges for remote access preview.
- **Remote access**: F5 (S3-backed) and F6 (hosted relay) scope.
- **Authentication or multi-user scenarios**: Sessions are scoped to the current user's
  `~/.koto/` directory.
- **Cross-repository session aggregation**: Dashboard scope is the current working
  directory's repository only.
- **Session management actions from dashboard**: No cancel, rewind, or override. Read-only
  view only.
- **inotify/kqueue file watching**: Polling at 500ms is sufficient for MVP. File-system
  event subscriptions deferred to V2 optimization.
- **Mermaid/graphical dependency graphs**: Text-based tree only. Graph rendering deferred.
- **Export or logging of dashboard output**: Except for `--once` mode, no persistent output.

## Known Limitations

- **Terminal state requires template coupling**: The absence of a `workflow_completed`
  event means the dashboard must load the compiled template for each session to determine
  terminal status. If the compiled template is deleted or moved after session creation, the
  session shows `unknown` status. This is the same limitation `koto status` has; it's not
  unique to the dashboard.
- **Gate panel requires Tier 2 event scanning**: Displaying the last gate result per state
  requires scanning `gate_evaluated` events (Tier 2) and applying epoch-boundary logic.
  This adds client-side computation that grows with log length. For very long sessions
  (1000+ events), gate display may be slightly slower than basic session metadata.
- **1000-sibling performance at polling rate**: Discovering 1000 child sessions each poll
  cycle requires ~2 seconds of I/O at sequential read speeds. The implementation should
  limit the child discovery scan to ≤500ms; if it exceeds this, it may skip the current
  poll cycle rather than block the UI.
- **Epoch-branched session drill-down deferred**: The focused parent view shows an
  "Archived epochs" summary row but does not expand it in the MVP. Users who need to
  inspect archived epoch children must use `koto status` or `koto workflows` directly.

## Decisions and Trade-offs

**D1 — Rendering technology: ratatui (TUI) over embedded web server**

Chosen: ratatui (pure-Rust TUI, no async runtime).

An embedded web server (axum, actix) would require introducing tokio — the first async
runtime dependency in koto, which is intentionally synchronous. ratatui is a single
pure-Rust dependency with no external system dependencies, integrates naturally with the
synchronous codebase, and matches the distribution model (single binary, no browser
required). A web-based dashboard is deferred to V2 if there is demand for a preview of
F5/F6-style remote access.

**D2 — Invocation model: ad-hoc command over daemon**

Chosen: ad-hoc `koto dashboard [<name>]` that users run when they want to observe.

Sessions accumulate JSONL log data from `koto init` regardless of whether a dashboard is
running, so launching mid-session gives complete history. This "data always accumulates"
property (similar to git instaweb) means an always-on daemon is a convenience rather than
a data-completeness requirement. The ad-hoc model avoids daemon lifecycle management and
stays compatible with V2 daemon integration: `koto daemon start/stop` can be added as
separate top-level commands without any conflict with `koto dashboard`.

**D3 — Terminal state detection: template-loading over heuristics**

Chosen: load compiled template at `MachineState.template_path` and check `terminal: true`.

Two alternatives were considered: (A) timeout heuristic ("session is done if no events in
N hours") and (B) last-transition inference ("if the last transition has no pending gates,
call it terminal"). Both alternatives produce false positives (a paused running session
would appear done) or false negatives (a terminal session with unusual timing would stay
marked running). Template-loading is how `koto status` determines terminal status today;
the dashboard uses the same mechanism for consistency. The compiled template is always
present when the session is active (it's created by `koto init` before any events are
written).

**D4 — Gate display scope: Tier 2 events in MVP**

Chosen: include gate display using Tier 2 `gate_evaluated` events.

Gate evaluations are explicitly listed as a display target in the observability roadmap for
F3. Deferring would narrow the MVP significantly and reduce its utility for the primary use
case (investigating stalled sessions). The F2 data contract provides all necessary event
fields; the dashboard computes "last gate result per gate per epoch" using the existing
`derive_last_gate_evaluated()` function from `src/engine/persistence.rs`. This is
achievable within the MVP scope without introducing significant complexity.

**D5 — Epoch-branched session handling: filter from main list, summarize in focused view**

Chosen: sessions with `~` in their name are hidden from the repo-wide list. The focused
parent view shows an "Archived epochs" summary row.

Without filtering, rewinding a batch parent multiple times produces accumulated epoch
branches (e.g., `research~1.task-a`, `research~3.task-a`) that make the session list
unreadable. Filtering by `~` name pattern is the simplest reliable approach: the `~`
character is reserved for epoch branching and cannot appear in user-specified workflow
names. The focused parent view shows a summary (not an expansion) of archived epochs so
users can see the breadth of historical rewinding without cluttering the active view.

**D6 — Command name: `koto dashboard` over `koto observe` or nesting under `koto session`**

Chosen: top-level `koto dashboard [<name>]`.

Three namespaces were considered: (A) `koto dashboard` (top-level), (B) `koto session
dashboard` (nested), (C) `koto observe` / `koto watch` (top-level, more generic). Option A
is the clearest: it mirrors the mental model ("I want to see the dashboard"), parallels
existing top-level commands (`koto status`, `koto workflows`), and leaves room for `koto
daemon` as a parallel top-level command when V2 arrives. Nesting under `session` produces
an awkward invocation (`koto session dashboard`) and doesn't fit V2 daemon patterns.
Alternative names like `observe` are more generic than needed and less discoverable.

## Open Questions

None. All questions from Phase 2 research were resolved during drafting. The exploration
and research phases addressed: terminal state detection strategy (template-loading),
invocation model (ad-hoc TUI), epoch-branched sessions (filter from main list), performance
envelope (≤1s startup for 100 sessions, 500ms poll interval as hard default), and command
interface V2 compatibility (top-level `koto dashboard`).
