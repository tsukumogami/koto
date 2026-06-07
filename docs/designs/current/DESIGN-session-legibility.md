---
schema: design/v1
status: Current
upstream: docs/prds/PRD-session-legibility.md
problem: |
  The local dashboard renders every session as a flat, id-sorted, equal-weight
  list with time measured from creation and an empty label, so past a handful
  of sessions it cannot answer "which session needs me now?" PRD-session-
  legibility requires the dashboard to become an attention surface: lead with
  needs-you sessions, measure liveness from last activity (without mistaking a
  parked run for a dead one), name every row by its work, and let the dead
  recede -- all working against existing sessions with no migration.
decision: |
  Compute liveness and labels at dashboard read time from data already in the
  event log -- no new schema, no migration. Add a Liveness classifier
  (active / idle / blocked / stalled / failed / done) derived from
  time-since-last-event with blocked evaluated before any idle threshold; sort
  the default view by attention band and recede terminal/abandoned sessions
  behind a count; derive a never-bare-id label via a total fallback chain and
  default `intent` at `koto init` for new sessions; mirror all of it in the
  `--once` path with appended columns and a `--status` filter.
rationale: |
  Read-time derivation satisfies the no-migration requirement (R12) and keeps
  the change additive and forward-compatible (R14): every decision is a new
  computation over the append-only log the dashboard already reads, plus one
  additive default-write at init. Blocked-before-idle is the load-bearing
  correctness rule (R4) that stops a legitimately parked session from being
  called dead. Receding via a read-time default filter (not file moves) keeps
  the feature to a single PR with no storage migration, while still meeting
  R9 because dead sessions never enter the default foreground regardless of
  how many accumulate.
---

# DESIGN: Session legibility in the local dashboard

## Status

Current

Design for PRD-session-legibility. Settles the mechanism forks the PRD
deferred: stored-vs-derived liveness, recede-by-decay-vs-filter, the label
chain, the liveness state set and thresholds, `--once` compatibility, and
parent/child folding.

## Context and Problem Statement

The dashboard reads each session's append-only JSONL event log from
`$TSUKU_HOME`-style `~/.koto/sessions/<name>/`. Today (`src/cli/dashboard*.rs`):

- `dashboard_data::refresh` builds a `SessionTree` of `CachedSession` values,
  each holding the parsed `StateFileHeader`, a `current_state` derived from the
  log (`derive_state_from_log`, `src/engine/persistence.rs`), `is_terminal`,
  `is_blocked`, `intent`, `mtime`, and `state_path`. Sessions whose header
  fails to parse are silently skipped (hence 1053 of 1155 dirs never appear).
- `dashboard.rs::classify_status` buckets into done / failed / blocked /
  running / unknown; `--once` prints six tab-separated columns
  (`id, current_state, elapsed, status, intent, template`) for **all** sessions
  sorted by `all_ids.sort()`; `elapsed` is `compute_elapsed_since(created_at)`.
- `dashboard_render::render_list` renders a 4-column table (State / Elapsed /
  Tasks + name) with the session id as the display name.

The PRD requires an attention surface. The forks to settle: how liveness is
computed and stored, how the dead recede, how rows get a label, the exact
state set and thresholds, how `--once` stays compatible, and whether session
trees fold.

## Decision Drivers

- **R12 -- no migration.** Existing sessions must benefit immediately; nothing
  may require rewriting historical state files.
- **R14 -- additive / forward-compatible.** No change may break readers of the
  current JSONL format or the `--once` column contract scripts depend on.
- **R4 -- blocked wins over idle.** Correctness: a parked session must never be
  classified as dead.
- **Single PR.** The plan is single-pr; the design must fit one coherent change
  with no storage migration.
- **The data is already there.** Each event carries an RFC3339 timestamp, so
  time-since-last-event and a derived label are pure functions of the log the
  dashboard already loads.

## Considered Options

### Fork A -- Liveness: stored on the session vs derived at read time

- **A1 (chosen): derive at read time.** Compute liveness from the loaded events
  (`events.last().timestamp`, `is_terminal`, `is_blocked`) every refresh. Zero
  migration, applies to all existing sessions instantly, nothing new written.
- A2: write a `liveness` / `waiting_on_human` field into the header on each
  advance. Rejected for this iteration: requires a write path, only helps
  sessions written after the change (violates R12 for the existing 1000+), and
  the value is trivially derivable, so storing it is redundant state that can
  drift from the log.

### Fork B -- Recede the dead: auto-archive at write time vs read-time filter

- **B1 (chosen): read-time default filter.** The default view shows only the
  live + needs-you bands; terminal and abandoned sessions are summarized as a
  count and revealed on one keypress / a `--all` flag. No files move.
- B2: auto-archive (move terminal/abandoned session dirs to `archive/` on a
  sweep). Stronger (it also shrinks disk and fixes the read cost), but it is a
  write/migration with its own correctness surface (reversibility, races) and
  pushes past a single clean PR. Deferred as a follow-up; B1 already satisfies
  R9 because dead sessions never enter the default foreground regardless of
  count.

### Fork C -- Labels: derive at read time, write a default, or both

- **C (chosen): both, layered.** Read-time `derive_label()` with a total
  fallback chain (covers all existing sessions, R12), plus an additive default
  `IntentUpdated` event appended at `koto init` when no `--intent` is supplied,
  so new sessions self-name at the source (R8) and the chain's top rung
  (`derive_intent`) is populated going forward.

### Fork D -- Liveness state set and thresholds

Chosen state set (closed enum, R2), evaluated in this precedence so terminal,
failed, blocked, and never-started always resolve before any idle test (R4):

1. `Done` -- `is_terminal` and not failed.
2. `NeedsYouFailed` -- terminal with a failed/error state (existing
   `classify_status` rule).
3. `NeedsYouBlocked` -- `is_blocked` (tail event is a gate evaluation it is
   waiting on). Evaluated before any idle test.
4. `Pending` -- `current_state == None` and non-terminal (only a
   `WorkflowInitialized` event; the run never advanced). Shown as "starting"
   while fresh (idle < `active_window`); once older it recedes. `Pending` is
   never needs-you -- a run that never did anything is cruft, not a stuck
   decision -- which keeps the never-advanced sessions out of the attention
   band.
5. `NeedsYouStalled` -- `current_state == Some` (it advanced, then went
   silent), non-terminal, not blocked, idle >= `stalled_threshold`. Surfaced
   (did work then died); do not alarm.
6. `Active` -- non-terminal, idle < `active_window`.
7. `Idle` -- non-terminal, `active_window` <= idle < `stalled_threshold`.

`NeedsYouStalled` requires `current_state == Some` precisely so a never-advanced
run (caught at rung 4 as `Pending`) is not mislabelled as a stuck mid-flight
decision.

Default thresholds (fixed now, configurable later via the existing config
substrate): `active_window = 5m`, `stalled_threshold = 2h`, `abandoned = 7d`.
`idle = compute_elapsed_since(last_event_ts)` (`dashboard_data.rs`). Thresholds
are constants in one module so a later config wire-up is a one-line change;
choosing them is design's call per R2/R3.

### Fork E -- `--once` compatibility

- **Chosen: append, don't reorder columns.** Keep the existing six columns in
  place so current scripts keep working (R14), append `idle` and `liveness` as
  columns 7-8, change the row ORDER to the attention sort (ordering is not part
  of the column contract and is still deterministic), and add a `--status
  <state>` / `--needs-you` filter (additive flags). Update
  `docs/reference/session-feed.md` to document the two new columns.

### Fork F -- Parent/child folding of the attention set

- **Chosen: defer (no folding in v1).** The PRD allows it. v1 lists needs-you
  sessions flat; folding a blocked parent to stand in for its waiting children
  is a follow-up. The attention sort and bands ship first; the tree data is
  already present for a later folding pass.

## Decision Outcome

A read-time legibility layer in the dashboard data/render path, plus one
additive init default. Concretely:

1. A `Liveness` enum + `classify_liveness(session, now)` in `dashboard_data`,
   replacing `dashboard.rs::classify_status`, using last-event recency with the
   precedence above.
2. An attention sort key `(band, urgency)` driving both `visible_rows()` (TUI)
   and the `--once` path, replacing `all_ids.sort()` and the current
   health-sort.
3. A read-time default filter that partitions sessions into a shown set
   (needs-you + active + idle) and a receded set (done + abandoned), with the
   receded count surfaced and a toggle / `--all` to reveal them.
4. A total `derive_label(session)` used as the row name, plus `handle_init`
   defaulting `header.intent`.
5. `--once` appended columns + `--status` filter; `session-feed.md` updated.
6. Parse-failure surfacing: `refresh` counts unreadable dirs instead of
   dropping them silently, exposed as an `Unknown`/unreadable tally.

## Solution Architecture

**Liveness (`src/cli/dashboard_data.rs`).**
```
enum Liveness { NeedsYouBlocked, NeedsYouFailed, NeedsYouStalled,
                Active, Idle, Pending, Done }
struct CachedSession { /* + */ last_event_at: Option<SystemTime>,
                              salient_var: Option<String>, }
fn classify_liveness(s: &CachedSession, now) -> Liveness   // precedence D1-D7
fn attention_key(l: &Liveness, idle: Duration) -> (u8, Reverse<Duration>)
```
`last_event_at` is captured in `read_session` from the timestamp of the final
event already read for `current_state` (no extra IO). `idle = now -
last_event_at`. Negative/futures clamp to zero (reuse `compute_elapsed_since`'s
existing `duration_since` error -> ZERO behavior; for a rewound session,
last_event_at is the post-rewind tail, so it reads as fresh).

**Attention bands (sort key).** Band order:
NeedsYou{Blocked,Failed,Stalled} (0) -> Active (1) -> Idle + fresh Pending
(2) -> receded (3). Within a band, sort by idle descending
(longest-waiting / most-dead first). The receded set (band 3, hidden by
default) = `Done` + `Pending` older than `active_window` + any `Idle` older
than `abandoned`. A fresh `Pending` (just-created, still starting) stays in
band 2 so a brand-new session is visible.

**Label (`derive_label`).** Fallback chain, each rung checked non-empty before
falling through (total function; never panics, never returns a bare id when a
workflow + state exist):
```
derive_intent(events)    // last IntentUpdated, including the init default
  -> template_name + " · " + salient_var + " · " + current_state
  -> template_name + " · " + current_state
  -> "untitled (" + template_name + ")"
  -> session_id            // only when even template_name is empty (corrupt header)
```
`salient_var` = first match from `WorkflowInitialized.variables` against a small
key priority list (`issue`, `target`, `name`, `task`, `query`); omitted when
none match. Child rows prefix the parent label (`parent ▸ leaf`) using the
existing tree.

**Init default (`handle_init`, `src/cli/mod.rs`).** `handle_init` already
records an explicit `--intent` by appending an `IntentUpdated` event (read back
by `derive_intent`). Extend it so that when no `--intent` is supplied it appends
a default `IntentUpdated` derived from: template `description` -> initial-state
`directive` first line -> `template_name`. Additive event on the path that
already writes intent; old sessions untouched.

**Render (`src/cli/dashboard_render.rs`, `dashboard_state.rs`).** The list
becomes attention-ordered with the receded set collapsed to a trailing summary
row ("✓ N done · M abandoned -- press a / --all"). The name column shows
`derive_label`; the time column shows idle (last-activity) with a liveness glyph
+ dimming (active bright, idle normal, stalled dim + age, done check). When the
needs-you band is empty, the list leads with an explicit all-clear row (e.g.,
`Nothing needs you -- N active, M idle`) so R6's zero-state is a deliberate
rendered artifact, not an inferred absence.

**`--once` (`src/cli/dashboard.rs`).** Rows in attention order; columns
`id, current_state, elapsed, status, intent, template, idle, liveness`
(7-8 appended); `--status <liveness>` / `--needs-you` filter the rows;
`--all` includes the receded set (default excludes it, matching the TUI).

**Parse-failure surfacing (`refresh`).** Count dirs whose header fails to parse
into an `unreadable` tally on the tree; render as a trailing note and an
`Unknown` `--once` row count, instead of silently skipping.

## Implementation Approach

Ordered, each step independently testable (feeds the plan):

1. **Liveness core.** Add `last_event_at` to `CachedSession` (populate in
   `read_session`); add `Liveness` enum + `classify_liveness` + thresholds
   module + `attention_key`. Unit tests for the precedence (esp. blocked-beats-
   stale, terminal-beats-idle, rewind freshness, clock-skew clamp).
2. **derive_label.** Implement the total fallback chain + `salient_var`; unit
   tests for every rung including the never-bare-id guarantee.
3. **Attention sort + receded partition.** Replace the `visible_rows()` sort and
   add the shown/receded split with a reveal toggle; tests on ordering and that
   the receded set leaves the default foreground.
4. **Render.** Name = label, time = idle, liveness glyph/dim, collapsed receded
   summary row.
5. **`--once`.** Attention order, appended `idle`/`liveness` columns,
   `--status`/`--needs-you`/`--all` flags; golden-output tests.
6. **Init default intent** in `handle_init` + test.
7. **Parse-failure tally** in `refresh` + test.
8. **Docs.** Update `docs/reference/session-feed.md` for the new `--once`
   columns and the liveness vocabulary.

## Security Considerations

No new external input, network, or credential surface. All computation is over
local session logs the dashboard already reads. `derive_label` interpolates
session-provided strings (intent, variables, template name) into terminal
output -- the existing render path already prints session-provided strings, so
this adds no new injection surface; the `--once` path keeps tab-separated
fields, so appended columns must not embed tabs/newlines (sanitize label/idle
fields as the existing columns are). No change to the on-disk format reduces the
migration/corruption risk surface.

## Consequences

**Positive.** Existing sessions benefit with no migration; the change is purely
additive (R12/R14); the dashboard answers "what needs me?" at a glance; `--once`
scripts keep working while gaining the liveness signal; the silent-drop bug is
fixed.

**Negative / follow-ups.** Read-time classification recomputes idle every
refresh (cheap: one subtraction per session); auto-archive (Fork B2), config-
urable thresholds (Fork D), parent/child folding (Fork F), and a push
notification (PRD Out of Scope) are deferred. The `--once` column append is a
soft contract change -- documented, backward-compatible for positional readers
of the first six fields, but a consumer doing a strict column-count check would
see eight.
