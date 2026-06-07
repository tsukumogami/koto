---
schema: prd/v1
status: Done
problem: |
  Developers running koto accumulate sessions fast -- every workflow run,
  child spawn, and experiment leaves one behind. The dashboard lists them
  all flat, sorted by id, at equal weight, with time counted from creation
  and the descriptive label empty in practice. Past a handful of sessions it
  can no longer answer the only question a developer opens it to ask --
  "which session needs me right now?" -- because abandoned runs are
  indistinguishable from live ones, rows are unrecognizable, and live work
  is buried under finished and never-started noise.
goals: |
  Make the local dashboard an attention surface, not an inventory: it leads
  with the sessions that need a human decision, tells a silently-dead run
  from an advancing one (without mistaking a legitimately parked run for a
  dead one), shows every session by the work it is doing, and lets finished
  and abandoned sessions recede so the live set is never drowned -- holding
  that way as session count grows into the thousands.
---

# PRD: Session legibility in the local dashboard

## Status

Done

Requirements for the local-scope dashboard legibility work, derived from the
accepted `docs/briefs/BRIEF-session-legibility.md`. Mechanism choices
(thresholds, stored-vs-derived state, decay-vs-filter, label-derivation
internals, rendering layout) are deliberately left to the downstream design.

## Problem Statement

The koto dashboard (`koto dashboard`, plus the non-interactive `--once` mode)
renders every session a developer has as a flat list sorted by session id,
every row the same visual weight. Three properties of that list make it
unusable at scale:

- **Liveness is invisible.** The status shown conflates "actively
  progressing" with "abandoned weeks ago," and the only time column is
  measured from session creation, so a run idle for a month and one advancing
  this second read identically. Against a real long-lived `~/.koto`, 60 of 61
  "running" sessions were more than a week old since creation -- corpses
  presented as live work.
- **Sessions are unrecognizable.** Rows are identified by opaque ids
  (`var-wf`, `issue_2336`). The descriptive `intent` field that already
  exists is empty for 100% of real sessions, so the list is a column of
  names that carry no meaning.
- **Signal is buried in noise.** Finished, never-advanced, and unparseable
  sessions share the view with live work, and there is no way to set them
  aside. Of 1155 session directories on disk, only 102 even appeared; of
  those, half had never left their first state.

The compounding effect is the real cost: the more a developer uses koto, the
less the dashboard helps. Affected users are koto's primary audience --
single developers running local agent workflows, often many at once via batch
orchestration and parent/child session trees.

## Goals

- Lead with attention: a developer's first answer on opening the dashboard is
  whether anything needs them, with those sessions ordered by urgency.
- Honest liveness: a silently-stalled run is visibly distinct from an
  advancing one, and a session legitimately parked on a gate is never
  mistaken for a dead one.
- Recognizable rows: every session is shown by the work it is doing, never a
  bare id.
- Recede the dead: finished and abandoned sessions leave the default view on
  their own and stop accumulating, without manual filtering each time.
- Trust at scale: the "needs you" set stays small and actionable, so it
  remains believable on the thousandth session.
- Scriptable parity: the non-interactive `--once` mode answers the same
  question for scripts.

## User Stories

Use-case form (this is a developer-tooling feature; some stories read as
operator use cases rather than end-user narratives):

- As a developer with several sessions in flight, I open the dashboard and the
  sessions needing my decision lead the view, ordered most-urgent-first, so I
  act on what matters without scanning past everything that does not.
- As a developer, I can tell a session that has silently stalled (agent died,
  no events) from one actively advancing, so I notice failures instead of
  trusting a stale "running."
- As a developer, a session legitimately waiting on a slow gate is shown as
  waiting -- not as dead -- so I keep trusting the liveness signal.
- As a developer, I recognize each session by a label naming its work, so I
  find the one I care about without opening sessions one by one.
- As a developer, finished and abandoned sessions are out of my default view
  (and do not pile up forever), so the live set is never buried -- but the
  history is one deliberate step away when I want it.
- As a developer scripting against `koto dashboard --once`, the output is
  ordered by attention and lets me filter to the sessions that need a human,
  so automation sees the same priorities the TUI does.

## Requirements

Functional:

- **R1 -- Liveness from last activity.** The dashboard SHALL derive each
  session's liveness from the time since its most recent event, not from time
  since creation.
- **R2 -- Liveness classification.** The dashboard SHALL classify every
  session into a small, closed set of liveness states that at minimum
  distinguishes: needs-you (a human decision is required), active (advancing
  now), idle/quiet (recently active, not yet stale), stalled (silent past a
  threshold, likely dead), and done (terminal). The exact state set and
  thresholds are design's to fix.
- **R3 -- Grace window before stalled.** A session SHALL NOT be classified as
  stalled for a brief pause; the classification SHALL require the quiet period
  to exceed a grace threshold, so momentary gaps do not flap.
- **R4 -- Blocked wins over idle (load-bearing).** A session whose most recent
  event shows it waiting on a gate (blocked, awaiting a command result,
  context, or human input) SHALL be classified as needs-you/waiting and SHALL
  NOT be classified as stalled, regardless of how long it has been quiet.
- **R5 -- Attention-ordered default view.** The default ordering SHALL lead
  with the needs-you set, then active/idle sessions, then terminal ones;
  within needs-you, sessions SHALL be ordered by urgency (e.g., failed or
  blocked first, then by age). This replaces the current sort-by-id default.
- **R6 -- Empty-attention state.** When no session needs the user, the
  dashboard SHALL make that obvious at a glance rather than leaving the user
  to infer it from a list.
- **R7 -- Recognizable label, never a bare id.** Every session row SHALL show
  a human-readable label derived from available data, with a fallback order
  that guarantees a row is never shown as only its raw session id when a
  workflow name and current state are known.
- **R8 -- Self-naming from the start.** A session SHALL carry a meaningful
  label from the moment it begins, including sessions created with no
  explicit description (the label may be defaulted from the workflow/template
  and initiating context).
- **R9 -- The dead recede from the default view.** Finished, abandoned, and
  never-advanced sessions SHALL be kept out of the default foreground --
  surfaced as a count and reachable in one step -- and SHALL NOT accumulate
  there over time as more sessions are created.
- **R10 -- No silent drops.** Sessions whose state files cannot be parsed
  SHALL NOT silently disappear from accounting; they SHALL be surfaced (at
  minimum counted, distinct from healthy sessions) rather than dropped without
  a trace.
- **R11 -- Non-interactive parity.** The `--once` output SHALL use the same
  attention ordering as the TUI, expose each session's liveness
  classification, and support restricting output to a status subset (at
  minimum the needs-you set) for scripting.

Non-functional:

- **R12 -- Works for existing sessions without migration.** Liveness
  classification, attention ordering, and label derivation SHALL apply to
  sessions already on disk with no migration step and no required rewrite of
  historical state files.
- **R13 -- Responsive at scale.** The dashboard SHALL remain responsive with
  at least 1000 session directories present, including the `--once` path.
- **R14 -- Additive and compatible.** Any new data written to sessions SHALL
  be additive and SHALL preserve the existing append-only event-log
  forward-compatibility and the session-feed data contract; no change SHALL
  break readers of the current format.

## Acceptance Criteria

- [ ] Opening `koto dashboard` against a `~/.koto` containing many sessions
      shows needs-you and active sessions first; finished and abandoned
      sessions are not in the default foreground.
- [ ] A non-terminal session with no events past the grace threshold, and not
      waiting on a gate, is shown as stalled -- visibly distinct from an
      advancing session.
- [ ] A gate-blocked session that has been quiet past the grace threshold is
      shown as waiting/needs-you, NOT as stalled.
- [ ] When at least one session needs the user, it leads the view ordered by
      urgency; when none do, the dashboard clearly shows an all-clear state.
- [ ] No session row is shown as only its bare id when its workflow and
      current state are known; a freshly-created session already shows a
      meaningful label.
- [ ] `koto dashboard --once` emits rows in attention order, includes the
      liveness classification, and supports filtering to a status subset.
- [ ] Session directories with unparseable headers are surfaced in the
      accounting (counted/flagged), not silently dropped.
- [ ] Liveness, ordering, and labels work against pre-existing sessions with
      no migration step run.
- [ ] The existing dashboard and persistence test suites pass; the event-log
      forward-compatibility checks are unbroken.

## Out of Scope

- **Cross-machine / multi-host visibility** (`host` / `owner`, "which machine
  produced this session", the aggregated cross-machine view, remote storage) --
  owned by the separate S3-backed dashboard work.
- **Multi-user** ownership distinctions -- single-user koto has one owner.
- **New event families** (coordination, review) and their panels.
- **Push notifications.** This iteration delivers the pull surface (the
  dashboard a developer opens). A single, sparingly-used "blocked on you"
  notification is a plausible follow-up but is deliberately excluded here to
  keep the scope to one shippable change.
- **The original five lifecycle-metadata header fields** as a fixed schema;
  any field that gets written is whatever these requirements actually need,
  decided in the design.

## Decisions and Trade-offs

- **Liveness is measured from last activity, not creation.** Creation-age is
  meaningless for triage (it cannot tell live from dead); last-event recency
  is the only signal that separates them. Alternative (keep creation-age,
  add a separate idle column) was rejected as redundant once recency exists.
- **Blocked always wins over idle.** Treating a long quiet period as "dead"
  would misclassify a session legitimately parked on a slow gate or awaiting a
  human, and a developer might kill good work. Gate-blocked state is therefore
  evaluated before any idle threshold. This is the load-bearing correctness
  rule and is called out as R4.
- **Pull now, push later.** The dashboard is fundamentally a pull surface, and
  the SRE discipline warns that push notifications must be rare and trustworthy
  or they get muted. Shipping the attention-ordered pull view first, and
  deferring any notification, keeps this to one coherent change and protects
  the trust the feature depends on.
- **Existing sessions must benefit without migration (R12).** This biases the
  design toward computing liveness and labels at read time from data already
  in the event log, while still allowing an additive default-label write for
  newly-created sessions. The stored-vs-derived split for any "waiting on you"
  marker is left to the design, constrained by R12 and R14.
- **The dead must leave the default view, mechanism deferred.** Whether
  sessions recede by aging out on their own (auto-archive) or by a default
  filter is a design decision; the requirement (R9) only fixes the outcome --
  dead sessions do not dominate the default view and do not accumulate there.
- **Folding related sessions is deferred to design.** R5 and the goals
  require the needs-you set to stay short. Whether related sessions fold
  together to keep it short -- for instance a blocked parent standing in for
  the children waiting on it, so a stuck session tree reads as one entry
  rather than many -- is a grouping/rendering decision left to the design,
  bounded by R5's ordering and R13's scale requirement. The design may also
  choose not to fold in the first iteration; the requirement is only that the
  set stay short and trustworthy.
