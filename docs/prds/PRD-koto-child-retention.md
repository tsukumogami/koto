---
schema: prd/v1
status: Done
problem: |
  koto removes a session, context included, on the tick it reaches any
  terminal state. For a child that ended at a failure terminal, that removal
  is what stops a batch recovering: the parent's retry_failed can't find the
  child, koto rewind can't either, and the failure reason is gone. The only
  way to keep a session, --no-cleanup, also stops a child's result reaching
  its parent, so no child can use it safely.
goals: |
  A session that reaches a failure terminal stays retryable, rewindable and
  readable through koto's own commands. Every child delivers its result to
  its parent on the tick it arrives at a terminal, whether or not it is kept.
  The terminal response says whether the session was kept, and every kept
  session has a stated way out.
absorbed:
  - docs/briefs/BRIEF-koto-child-retention.md
source_issue: 240
---

# PRD: koto-child-retention

## Status

Done

Absorbed [BRIEF-koto-child-retention](docs/briefs/BRIEF-koto-child-retention.md); carried in Absorbed Brief.

Phase 4 jury: completeness and testability passed on re-review after the
revision that named the `retention` field and added criteria for every
requirement; clarity passed on the first round.

## Absorbed Brief

koto removes a session, context and all, on the tick it reaches any terminal
state, and a failure terminal is treated like a success. For a child that
failed, that removal lands at the worst moment: the failure is where the
parent's recovery starts, and everything recovery needs is gone. The parent's
retry can't find the child, nobody can rewind it one step, and the reason it
failed was deleted on the tick that recorded it. The one way to keep a
session, `--no-cleanup`, also keeps the child's result from its parent, so a
child has no safe setting.

What a coordinator or operator should get instead is a failed child they can
still act on after its terminal: retried by its parent, rewound by hand, and
read through `koto status` and `koto context get`, with the parent receiving
the child's result on the same tick whether or not the record is kept. The
terminal response says what happened to the session, and a kept record has a
stated point where it's reclaimed. The work covers failure terminals and
deliberate retention; keeping every successful session, moving context out of
the session, a time-based sweeper and any change to shirabe's skills are
outside it.

## Problem Statement

koto disposes of a session on the tick that lands it in a terminal state. The
disposal removes the session directory, so the event log and the context store
go together. Whether the terminal is a success or is declared `failure: true`
makes no difference.

Batch parents are hit hardest. A child materialized by a parent's
`materialize_children` that ends at a failure terminal such as `done_blocked`
is gone before its parent's next tick. The parent can see from its own log
that the child failed, but it can't act on it: `retry_failed` is refused with
`unknown_children` because retry works by rewinding the child's own log, and
`koto rewind` on the child reports the workflow missing. The child's
`failure_reason` and the running record a skill kept in context are gone too.
A parent like `/execute` then has only escalation left. In one overnight run
three `/work-on` children ended this way with their work complete and pushed,
and each cost a full re-run of the parent.

`koto next --no-cleanup` keeps a session, but on a child it also stops the
child's result being recorded on its own log (for a terminal without a
`result:` map) and stops the completion notice reaching the parent's log.
Those are the only two places a `children-complete` gate reads a child's
result. A child that keeps its record therefore withholds its result: the gate
reports `all_complete: true` with `results_in: false`, a parent that waits on
the gate never advances, and one keyed on `all_complete` advances without the
result. Retention and result delivery are one switch, and neither setting is
safe for a child.

## Goals

- A session that reached a failure terminal can still be retried, rewound, and
  read after the terminal tick, using koto's commands only.
- A child's result reaches its parent, and any request leg it is bound to, on
  the tick it arrives at a terminal, independent of whether the session is
  kept.
- A caller learns from the terminal response itself whether the session was
  kept.
- Retained sessions don't accumulate without bound: each has a documented way
  to be reclaimed, and one retained under a parent that koto disposes of
  doesn't outlive that parent.

## User Stories

- As a coordinator running `/execute`, I want `retry_failed` to reach a child
  that ended at `done_blocked`, so that the batch retries it instead of
  escalating.
- As a coordinator template's reconcile step, I want to read a failed child's
  final state, result and `failure_reason` after its terminal through
  `koto status` and `koto context get`, so that I can decide the next step
  without reading koto's files directly.
- As an operator, I want to `koto rewind` a failed child one step and
  resubmit, so that one bad submission doesn't force a re-run of the parent.
- As a skill author, I want to keep a child's session with `--no-cleanup` and
  still have its parent converge on its result, so that retaining a record
  isn't a trade against the batch.
- As an operator, I want one command to reclaim every retained session under
  a finished root, and I want sessions under a parent koto already removed not
  to linger, so that retained records don't pile up.
- As a caller of `koto next`, I want the terminal response to say whether the
  session was kept, so that I know whether I can read it afterwards.

## Requirements

### Functional

**R1. Failure terminals retain the session.** When a `koto next` tick lands a
session, root or child, in a state declared `terminal: true` and
`failure: true`, koto does not remove the session. Its event log and context
store stay readable after the tick, with or without `--no-cleanup`. This holds
on both terminal write sites: the advance loop and `koto next --to <terminal>`.
It holds on every later tick of that session too, so a retained failure
terminal is never removed by being ticked again.

**R2. Result delivery is independent of retention.** On the tick a session
arrives at any terminal state, koto performs every result write it performs
today for a removed session: the result on the session's own log, promotion to
a bound request leg, the terminal-index entry, and the completion notice with
the result on the parent's log. It does so whether the session is then kept or
removed. The result is the one koto already reports for that terminal: the
resolved `result:` map when the terminal declares one, and otherwise the
synthesized envelope (`status`, a `summary` defaulting to `failed at <state>`
or `completed at <state>`, and the terminal evidence as `payload`).

**R3. Once per arrival.** The writes in R2 happen once per arrival at a
terminal. An arrival begins at the transition, directed transition or rewind
that put the session in its current terminal state; `koto next --to` from one
terminal to another (where the template declares that transition) is therefore
a new arrival. Ticking a session that is
already standing in a terminal state, and whose log already records a result
for this arrival, adds no result event, no terminal-index entry and no parent
notice. The one exception is a tick that removes a session after an earlier
tick kept it: that tick re-sends the parent notice before removing, so for a
session koto removes, a notice lost to a failed write is never lost for good; a
duplicate is harmless under R12. A retained session is never removed by a tick,
so a lost notice for it stays lost, and R12 is what keeps its parent's gate
correct. A session parked at a terminal by an earlier koto version, with no
result recorded for its arrival, gets the R2 writes on its next tick.

**R4. `--no-cleanup` controls retention only.** `koto next --no-cleanup` keeps
the session on the tick it reaches any terminal, success included, and changes
no other write on that tick. The flag applies per tick, as today: a success
terminal kept by the flag and ticked again without it is removed on that later
tick (with no repeated R2 writes). Its help text reads "Keep the session after
it reaches a terminal state (a failure terminal is always kept)".

**R5. The terminal response states retention.** Every `koto next` response
with `action: "done"` carries a `retention` object:

- `{"retained": false}` when koto removes the session after the tick;
- `{"retained": true, "reason": "failure_terminal"}` when the terminal is
  declared `failure: true` (this reason wins when `--no-cleanup` was also
  passed);
- `{"retained": true, "reason": "no_cleanup"}` when a non-failure terminal is
  kept because `--no-cleanup` was passed.

The object is computed on each tick from that tick's terminal and flag, so a
later tick of a kept session reports the same way.

**R6. Retry reaches a retained child.** A parent's `retry_failed` naming a
child retained under R1 is accepted, rewinds the child to its template's
initial state, and the parent's `children-complete` gate reports that child
with `outcome: "pending"` until it reaches a terminal again.

**R7. Rewind reaches a retained session.** `koto rewind` on a session retained
under R1, root or child, moves it back to the state before the terminal and
leaves it non-terminal.

**R8. Reads use koto's commands.** For a retained session, `koto status`
reports `is_terminal: true`, its terminal state and its `result` (existing
output, unchanged); `koto context get` returns any key it wrote, including
`failure_reason`; and `koto workflows` lists it. No consumer needs to read
files under the koto home directory.

**R9. Retained sessions have a way out.** Each of these removes retained
sessions:

- a retained child that is retried or rewound and then reaches a success
  terminal without `--no-cleanup` is removed on that tick, after its R2
  writes;
- when koto removes a parent session at its own terminal, it also removes
  every descendant that stands in a terminal state, whatever kept it, so no
  retained session outlives the parent that koto removed; the walk does not
  descend through a descendant that is not terminal, and that descendant and
  everything under it are left alone;
- `koto workspace prune --root <id>` removes a terminal root and every session
  under it, live or not (existing behaviour, unchanged);
- `koto session cleanup <name>` removes one session (existing command,
  unchanged);
- `koto init --attach-live --replace-terminal` on a finished session removes
  that session's terminal descendants the same way before replacing it, so a
  new run under a reused name never inherits the old run's retained children.

A parent that is itself kept keeps its retained descendants in place.

**R10. Names stay reusable.** A retained session occupies its name. The
existing refusals already say how to reuse it: `koto init` on a retained root's
name is refused naming `koto session cleanup`, and `koto init --attach-live
--replace-terminal` replaces a finished session. These refusals are unchanged,
and the docs say that a retained failure terminal is the new common case for
them.

**R11. Bound legs are unaffected.** A session bound to a request leg has its
result promoted to the leg on its first terminal arrival, with the same
payload, `source: promoted` and `final_state` as today, whether or not it is
retained. A leg that already holds a result keeps it: a later arrival
attempts no second promotion, as today.

**R12. Lost or duplicate records are harmless.** If a completion notice for a
retained child is missing from the parent's log, or appears more than once,
the parent's gate still classifies the child and reads its result correctly,
because a child on disk is read from its own log first.

**R13. Documentation.** `docs/guides/cli-usage.md` and
`docs/workspace-layout.md` state the retention rule, the `retention` field and
each reclaim path in R9. The koto-skills files that describe `--no-cleanup` or
terminal cleanup (`plugins/koto-skills/skills/koto-user/SKILL.md`, its
`references/command-reference.md` and `references/response-shapes.md`) say
the same and no longer describe the flag as withholding a child's result or as
a debugging aid. `CHANGELOG.md` records the change under Unreleased.

### Non-functional

**R14. Bounded growth.** A repeat tick of a kept session at its terminal adds
no event to its log, to its parent's log or to the terminal index. The DESIGN
states the per-session storage cost of retention and the conditions under
which retained sessions accumulate.

**R15. No new failure mode on the terminal tick.** The terminal tick still
exits 0 once its response is printed, and a failure in any post-response write
is reported on stderr as today.

## Acceptance Criteria

Retention

- [ ] A root driven to a `failure: true` terminal without `--no-cleanup` still
  exists afterwards, and `koto context get <root> <key>` returns a key written
  before the terminal.
- [ ] A child driven to `done_blocked` (declared `failure: true`) without
  `--no-cleanup` still exists afterwards, and `koto context get <child>
  failure_reason` returns the value it wrote.
- [ ] The same holds when the child reaches the failure terminal through
  `koto next <child> --to <failure-terminal>`.
- [ ] Ticking that retained child again without `--no-cleanup` leaves it on
  disk.
- [ ] `koto status <child>` reports `is_terminal: true`, the terminal state and
  a `result` with `status: "failure"`; `koto workflows` lists the child.

Delivery

- [ ] After the retained failed child's terminal tick, the parent's gate lists
  it with `outcome: "failure"`, a `result` whose `status` is `"failure"`, and
  `results_in: true`, for a parent whose exit is unconditional and for one
  keyed on `gates.<gate>.all_complete: true`.
- [ ] A child ticked to a success terminal with `--no-cleanup` still exists
  afterwards, and its parent's gate reports `results_in: true` and passes, for
  both parent shapes.
- [ ] A child ticked to a success terminal without a `result:` map and with
  `--no-cleanup` has one `request_store.result` event on its own log and one
  `ChildCompleted` on its parent's, the same as the same tick without the flag.
- [ ] A child at a terminal parked by an earlier version (log has no
  `request_store.result` after its last transition), ticked once, appends one
  `ChildCompleted` to its parent and the parent's gate reports `results_in:
  true`.

Once per arrival

- [ ] Ticking a retained terminal child three more times leaves its log's event
  count, its parent's log event count and the terminal-index entry count for
  it unchanged, and the index holds exactly one entry for it; the same holds
  for a retained root.
- [ ] A retried child whose log holds a failure result from its earlier
  arrival is reported by its parent's gate as `pending`, never with that old
  result, until it records a result for its new arrival.
- [ ] A retried child that reaches a terminal again appends exactly one new
  `ChildCompleted` to its parent, and the gate reports the new result.
- [ ] A retained failed child moved with `koto next <child> --to <other
  terminal>` appends exactly one new `ChildCompleted` to its parent, carrying
  the new terminal's name as `final_state`.
- [ ] A success terminal kept with `--no-cleanup`, ticked again without the
  flag, no longer exists after that tick; its own log and the terminal index
  gained no events, and its parent's log gained at most one `ChildCompleted`
  carrying the same result as the arrival's.

Retry and rewind

- [ ] The parent's `retry_failed` naming a retained failed child is accepted
  (no `unknown_children`); the child's `koto status` then shows its initial
  state, and the parent's gate lists it as `pending`.
- [ ] `koto rewind` on a freshly retained failed child succeeds, leaves it in
  the state it was in before the terminal, and `koto status` reports
  `is_terminal: false`; the same holds for a retained failed root.
- [ ] `retry_failed` naming a child that reached a success terminal and was
  removed is still refused with `unknown_children`, and `koto rewind` on a
  removed session still fails with "workflow not found".

Response

- [ ] The `action: "done"` response carries `retention: {"retained": true,
  "reason": "failure_terminal"}` for a failure terminal (with or without
  `--no-cleanup`), `{"retained": true, "reason": "no_cleanup"}` for a success
  terminal ticked with the flag, and `{"retained": false}` for a success
  terminal ticked without it.
- [ ] A second tick of a retained failure terminal carries the same
  `retention` object as the first.
- [ ] `koto next --help` describes `--no-cleanup` with the R4 wording.

Reclaim

- [ ] A retained failed child that is retried and reaches a success terminal
  without `--no-cleanup` no longer exists after that tick, and the parent's
  gate still reports its result.
- [ ] When a parent reaches a success terminal without `--no-cleanup`, its
  retained failed child, a child it kept with `--no-cleanup` at success, and a
  terminal grandchild under that child are all removed on the same tick; a
  non-terminal child of the same parent, and a terminal child under that
  non-terminal child, are not.
- [ ] When a parent reaches a failure terminal, or a success terminal with
  `--no-cleanup`, its retained children stay.
- [ ] `koto workspace prune --root <root> --yes` removes a retained root and
  every session under it, and leaves sessions under another root untouched.
- [ ] `koto session cleanup <child>` removes one retained child and leaves its
  siblings and parent in place.
- [ ] `koto init <name>` on a retained root's name is refused with a message
  naming `koto session cleanup`, and `koto init <name> --attach-live
  --replace-terminal` replaces it and removes its retained terminal children.

Legs and robustness

- [ ] A child bound to a request leg that ends at a failure terminal has its
  leg resolved once with `source: promoted` and the terminal's name as
  `final_state`, and the child session still exists; a bound child that is
  retried and reaches a terminal again does not change the already-resolved
  leg.
- [ ] A parent log carrying two `ChildCompleted` events for one retained
  child's arrival, and one carrying none, both yield the same gate
  classification and result for that child as one carrying a single event.
- [ ] When the parent's log cannot be written on a child's terminal tick, the
  tick exits 0, prints a warning on stderr, and the child session is kept.

Docs and suite

- [ ] `docs/guides/cli-usage.md` and `docs/workspace-layout.md` state the
  retention rule, the `retention` field and every reclaim path in R9; the three
  koto-skills files in R13 name the `retention` field, and none of them
  describes `--no-cleanup` as a debugging aid or as withholding a result;
  `CHANGELOG.md` has an Unreleased entry.
- [ ] `cargo test` passes, and each criterion above is exercised by a named
  test.

## Out of Scope

- **Keeping successful terminal sessions by default.** Issue 234 asks for a
  completed run to stay queryable; that has a different storage profile and is
  a separate change. A success terminal is kept only when `--no-cleanup` asks.
- **Separating context from the session's lifetime.** Context stays inside the
  session directory and shares its fate.
- **A time-based retention window or background sweeper.** koto runs no
  daemon; reclaim is event- or command-driven.
- **A flag to withhold a child's result.** No caller needs a child's result
  held back from its parent; the old coupling was the defect.
- **Changes to shirabe skills**, including dropping `/work-on`'s root-only
  retention rule, and deciding whether `/work-on`'s `validation_exit` or
  `/execute`'s `ready_awaiting_merge` should be declared failures. Neither is
  today, so neither is retained. That is the consumer's follow-up.
- **The request store's wake signal, the `--to` guard for non-overridable
  gates, and the decider ledger**, which other work covers.

## Known Limitations

- The response is printed before koto's post-response writes run. If the
  parent notice cannot be written, koto keeps the session so the parent can
  still see it (today's behaviour), even though the response said
  `retained: false`. The warning on stderr is the signal in that case, and the
  next tick of that session retries the writes and the removal.
- A parent that koto removes takes every terminal descendant with it,
  including a child a caller kept with `--no-cleanup`. A caller who wants a
  child's record to outlive its parent keeps the parent too.
- `koto session cleanup` on a retained parent removes only that session and
  leaves its retained children naming a parent no session holds; they show in
  `koto workflows --orphaned` and are removed one at a time with
  `koto session cleanup`, or by pruning the terminal root above them.
- `koto workspace prune --root` refuses a root that is still live (without
  `--force`), so retained children under a running root wait for it to finish
  or are removed one at a time with `koto session cleanup`.
- Retained sessions under a parent that is itself kept, which is every parent a
  shirabe skill drives, stay until someone runs `koto workspace prune` or
  `koto session cleanup`. Retention costs disk until then.
- Changing what `--no-cleanup` means is visible: a caller that relied on a
  flagged child not delivering its result will see the result arrive. No such
  caller is known; koto's own tests that pinned the old behaviour change with
  this work.

## Decisions and Trade-offs

- **Failure terminals are retained for roots as well as children.** The issue's
  first criterion speaks of any run reaching a failure terminal, and a root's
  failure record is as useful as a child's. Alternative: children only. Rejected
  because it leaves the reported root loss in place and splits the rule by a
  property the terminal state doesn't carry.
- **Result delivery has no switch.** Alternative: a second flag that withholds
  the result, making the two behaviours "independently controllable" in both
  directions. Rejected: no known caller wants a result withheld, and a switch
  nobody should set is a way to reintroduce the defect. Independence here means
  retention no longer changes delivery.
- **Once per arrival rather than once ever.** A retried child that fails again
  must notify its parent again, so the unit is the arrival, which koto already
  tracks for result maps.
- **What retention keeps and how long it lasts** is the DESIGN's to settle. R6
  and R7 need the child's own log, which rules out keeping only a summary; the
  DESIGN compares the remaining shapes with their costs.
