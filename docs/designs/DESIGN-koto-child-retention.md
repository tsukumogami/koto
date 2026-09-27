---
schema: design/v1
status: Planned
problem: |
  koto's terminal tick removes a session unconditionally, failure terminals
  included, and the only opt-out, --no-cleanup, also suppresses the two writes
  a parent's children-complete gate reads a child's result from. A child that
  fails therefore loses the record its parent's retry_failed, koto rewind and
  any failure-reason read depend on, and a child that keeps its record
  withholds its result.
decision: |
  Keep the whole session for any failure terminal, and for any terminal ticked
  with --no-cleanup, and deliver the result on every arrival regardless:
  finish_terminal_tick records the child-log result on every arrival and moves
  the terminal-index entry and the parent's ChildCompleted out of the cleanup
  guard, gated on the arrival not already being recorded. The converge reads
  only a result recorded for a child's current arrival. A
  retained session lives as long as its parent: when koto removes or replaces a
  parent it also removes the parent's terminal descendants through a guarded
  walk, and koto workspace prune and koto session cleanup reclaim the rest. The
  koto next terminal response carries a retention object saying which applied.
rationale: |
  retry_failed and koto rewind both append to the child's own log, so anything
  short of the whole session fails the two recovery paths the fix exists for.
  The arrival guard koto already keeps for result maps makes delivery once per
  arrival without new state, which removes the per-tick flood that kept the
  parent notice behind the cleanup guard. Binding a retained session's life to
  its parent's gives retention an automatic end without a daemon or a clock,
  and the command-driven reclaim paths already exist. A session costs a median
  28 KB on disk, so keeping the failed ones is cheap.
upstream: docs/prds/PRD-koto-child-retention.md
user_visible_surface: true
---

# DESIGN: koto-child-retention

## Status

Planned

## Context and Problem Statement

Every `koto next` tick that lands a session in a terminal state ends in
`finish_terminal_tick` (`src/cli/mod.rs`), called from both terminal write
sites: the advance loop and the `--to` directed transition. The function runs
six steps in a fixed order, set by the request-lifecycle design:

1. resolve the `WorkflowResult` once (the caller does this through
   `terminal_record` before printing, since the response carries it);
2. append `request_store.result` to the session's own log;
3. promote the result onto a bound request leg;
4. append a terminal-index entry;
5. append `ChildCompleted`, carrying the result, to the parent's log;
6. remove the session directory with `backend.cleanup`, which is
   `remove_dir_all` for the local backend and takes `ctx/` with it.

`--no-cleanup` returns after step 3. Step 2 runs under the flag only when the
terminal declares a `result:` map. So a flagged child whose terminal has no map
leaves no result on its own log and none on its parent's, and those are the two
sources the `children-complete` converge reads (`src/cli/batch.rs`): the live
child's latest `request_store.result`, falling back to the parent's
`ChildCompleted.result` for a child that's no longer on disk. With neither, the
child stays in `outstanding` and `results_in` is false.

The guard exists for a reason the request-lifecycle design gives: an
already-terminal session ticked again runs `finish_terminal_tick` again, so
hoisting steps 4 and 5 unguarded would append an index entry and a parent
event on every tick of a parked session. What the guard also does, and nobody
intended, is make "keep the record" and "deliver the result" one switch.

Without the flag, step 6 runs for every terminal, and the costs land on
failure terminals:

- `retry_failed` (`src/cli/retry.rs`) reads the child's log to validate it and
  retries a failed child by appending `Rewound` to that log. A missing session
  is `unknown_children`.
- `koto rewind` refuses a missing session with "workflow not found".
- `koto context get` and `koto status` have nothing to read, so a child's
  `failure_reason` and a skill's running record are gone.

Two facts in the current code shape the answer. First, koto already knows
whether a terminal arrival has been recorded:
`recorded_result_for_current_arrival` (`src/engine/terminal_result.rs`) finds
a `request_store.result` appended after the last transitioned, directed or
rewound event, and `terminal_record` exposes that as `already_recorded`.
Second, the converge prefers an on-disk child's own log over the parent's
copy, so a retained child is classified from its own record, and a duplicate
or missing parent event for it changes nothing the gate reports.

A third fact matters once retention makes retry routine. The converge reads a
live child's *latest* `request_store.result`, not the one for its current
arrival. And three readers treat the terminal index as "this session is
terminal": discovery (with a header-mtime fallthrough), the caps counter and
the wake scan (`src/engine/caps.rs`, `src/engine/wake.rs`, neither of which
checks the header). Wake finds a dispatched child's completion only through
the index, and discovery and caps skip indexed sessions so they never re-offer
a finished one. Both assumptions held while every terminal session was removed
on the tick it arrived; a session that stays and can come back strains them.

**The rule covers the terminal that ended the reported runs.** It keys on
`failure: true`, so it fixes the incident only if the failure terminals in use
declare it. In shirabe at `9e9c287` they do: `/work-on`'s `done_blocked`, the
terminal the three children in issue 240 ended at, is `failure: true`, as are
`/execute`'s `done_blocked` (both templates), `/execute`-coordinated's
`done_error`, and `/scope`'s and `/deliver`'s `done_error` and `done_refused`.
The terminals that are not failures are the ones that should be removed or are
roots that already pass `--no-cleanup`: `/work-on`'s `done`,
`done_already_complete`, `validation_exit` and the skip marker
`skipped_due_to_dep_failure`; `/scope`'s `done_re_evaluation`,
`done_abandonment` and `done_cancelled`; and the success and pause terminals of
`/execute` and `/deliver`. No failure-ending terminal is missing the flag.

The requirements are in `docs/prds/PRD-koto-child-retention.md` and the
originating report is koto issue 240; this document cites requirements by
number.

## Decision Drivers

1. **Retry and rewind need the child's own log** (R6, R7). Both append to it.
   Any shape that keeps less than the session fails them.
2. **Delivery must not depend on retention** (R2, R4), and must happen once per
   arrival (R3, R14): no per-tick growth on a parked session.
3. **Reads go through koto's commands** (R8). Consumers never read files under
   the koto home.
4. **Retention needs an end** (R9) that works without a daemon, since koto runs
   only when invoked.
5. **Implicit removal must never touch live work.** Anything koto deletes on
   its own, without an operator naming it, has to fail toward keeping.
6. **No new failure on the terminal tick** (R15): the response prints first and
   the tick exits 0.
7. **Stay off the lines other open work changes, and land after it.**
   Checked against the open PRs' diffs on 2026-09-27: the request store's wake
   signal (PR 252) rings its wake from `record_result` in the request store,
   which `promote_leg_result` calls, and changes nothing inside
   `finish_terminal_tick`; the `--to` guard (PR 257) edits the `--to` branch of
   `handle_next` above its terminal call site, which this design edits to
   attach `retention`; the decider ledger (PR 258) edits a separate part of
   `handle_next`. None touches the converge or `init_entry`'s replace path.
   Promotion (step 3) stays outside the arrival gate, so a wake wired into it
   keeps PR 252's semantics: a re-ticked parked session is a no-op because the
   leg already holds a result. This work lands after PR 252 and rebases over
   PR 257's `--to` hunks; it does not touch the wake reader.
8. **Keep the storage cost small and stated** (R14). Measured on 2026-09-27
   with `du -sk` over every session directory on one developer workstation
   that runs koto-driven skill workflows daily: 160 sessions, median 28 KB,
   90th percentile 80 KB, maximum 104 KB.

## Considered Options

### Decision 1: What a failure terminal keeps, and for how long

This is the retention shape: what survives the terminal tick of a failed
session, where it lives, how long, and what removes it. It decides whether
retry and rewind work at all, what disk koto spends, and whether an operator
has to remember to clean up.

#### Chosen: keep the whole session; its life is bound to its parent's

A session whose terminal is declared `failure: true` is not removed on that
tick or any later one. Nothing is copied or moved: the session directory, its
event log and its `ctx/` stay exactly where they were, so every existing koto
command that reads a session (`status`, `context get`, `workflows`, `rewind`,
`retry_failed`, the converge) works on it unchanged.

The retained session ends at the first of:

- **Recovery.** A retry or rewind takes it out of the terminal; when it reaches
  a success terminal without `--no-cleanup`, it is removed on that tick, as any
  success terminal is.
- **Its parent's removal or replacement.** When koto removes a parent at the
  parent's own terminal, or replaces a finished parent with
  `koto init --replace-terminal`, it removes the parent's terminal descendants
  first (Decision 3).
- **An operator command.** `koto workspace prune --root <id>` removes a
  terminal root and its whole tree; `koto session cleanup <name>` removes one
  session. Both exist today and change not at all.

Storage cost: one session directory per failed session, which is the size it
already had at its terminal (median 28 KB, 90th percentile 80 KB on the
measured host). Retrying reuses the same directory, so a child that fails three
times costs one directory, not three. Retained sessions accumulate only under a
parent that is itself kept: a root at a failure terminal, or any root ticked
with `--no-cleanup`, which is how every shirabe-driven root runs. Under such a
root, retained children stay until the root is pruned. A thousand retained
failures at the 90th percentile is about 80 MB.

Roots get the same rule. A root at a failure terminal keeps its record for the
same reason a child does, and reaches its end through prune or session cleanup.

#### Alternatives Considered

**Keep a compact terminal record after disposing of the session.** Write the
final state, `failure_reason`, the result and timestamps to a small file (or an
entry in the terminal index), then remove the session as today. Cheaper on
disk, perhaps 1 KB instead of 28. Rejected because `retry_failed` and `koto
rewind` both append to the child's log, which would no longer exist, so the two
recovery paths the issue is about still fail. It would also need a new read
verb for the record and would drop every context key except the ones copied.

**Make `--no-cleanup` the default for children bound to a request leg.** Flip
the default only for sessions with a leg pointer. Rejected because the children
that lost their record in issue 240's incident are batch children materialized
by `/execute`, which carry no leg pointer, so it misses the case. It also keeps
successful children indefinitely, and on its own it still fuses retention with
withheld results unless Decision 2 lands anyway, at which point the narrower
failure-terminal rule covers what matters.

**Keep for a retention window and sweep with `koto workspace prune`.** Keep any
terminal session for N days, and teach prune an age filter that a user or cron
runs. Rejected because koto has no daemon to run the sweep, so the window only
closes when someone remembers to; because a wall-clock expiry can remove a
failed child's record while its parent is still waiting to retry it; and
because it adds a configuration knob whose right value depends on how long
batches run. An age filter on prune may still be worth adding later, as a
convenience rather than as the lifecycle.

**Keep every terminal session.** Stop auto-removing sessions at all, which is
issue 234's broader ask. Rejected for this work: it multiplies the storage cost
by every successful run, changes what `koto workflows` shows for every user,
and isn't needed for anything here. A success terminal is kept only when a
caller asks with `--no-cleanup`.

### Decision 2: How result delivery separates from retention

The terminal tick's result writes must happen whether or not the session is
kept (R2) and exactly once per arrival (R3), a repeat tick of a parked session
must add nothing (R14), and a reader must never mistake an old arrival's result
for the current one. Today the guard that stops per-tick growth is the same
guard that suppresses delivery.

#### Chosen: every delivery write gated on the arrival

`finish_terminal_tick` computes one boolean up front,
`arrival = !record.already_recorded`, and a second, `remove`, which is true
when the session is not retained (Decision 1's rule, or the flag). Then:

- **Step 2** appends `request_store.result` to the session's own log whenever
  `arrival` is true, for every terminal, map or not, flag or not. This is what
  makes the next tick see `already_recorded`, and so what bounds everything
  else.
- **Step 3**, leg promotion, is unchanged. It already runs under the flag and
  is already a no-op once the leg holds a result.
- **Steps 4 and 5**, the terminal-index entry and `ChildCompleted` on the
  parent, move out of the cleanup guard and run when `arrival` is true. A
  retained session needs its index entry as much as a removed one: wake learns
  a dispatched child finished only from the index, and discovery and caps rely
  on it to not offer a finished session for dispatch again.
- **Step 5 again on removal.** `ChildCompleted` also runs on a tick that is not
  an arrival but is about to remove the session: the tick after a failed parent
  append deferred removal, or the first flagless tick of a success terminal
  kept by the flag. Re-sending there costs at most one duplicate per removal
  and guarantees a removed child always left its notice behind.
- **Step 6** runs when `remove` is true and neither the parent append nor the
  promotion asked to defer, as today.

A repeat tick of a kept session finds its arrival recorded and writes nothing.
A session that leaves the terminal through `retry_failed`, `koto rewind` or a
`--to` to another terminal (where the template declares that transition)
starts a new arrival, because each of those appends a state-changing event,
and gets its writes again. A session parked by an
earlier koto version has no record for its arrival, so its next tick delivers,
which repairs a batch stuck on a flagged child.

`has_result` for the index entry is `true` when step 2 appended on this tick,
which keeps the invariant that `has_result` implies a readable result.

A retained session that is rewound or retried keeps its index entry until its
next arrival writes a newer one. Discovery already handles that through its
header-mtime fallthrough, since the rewind appends to the log. Caps under-counts
it, which caps' own comments name as the safe direction. Wake may treat an
open dispatch on it as finished early; a wake is idempotent and the woken
requester resumes from its own log, so an early wake costs one extra look. Not
touching the wake reader keeps this work off the lines other open work is
changing.

**The converge reads the current arrival only.** Two changes in
`src/cli/batch.rs`. The live-child dereference changes from "latest
`request_store.result`" to `recorded_result_for_current_arrival`. And the
fallback to the parent's `ChildCompleted.result` no longer applies to a child
whose own log is readable and whose template classifies its current state,
terminal or not: that log is the authority, and the parent's copy may belong
to an earlier arrival. A child that is gone from disk, or whose log or template
can't be read, still falls back to the parent's copy. A child whose step-2
append failed therefore reads as having no result until its next tick, which
is still an arrival and records it.

Without both, a child retried out of a failure terminal carries its old failure
result into the gate output while it runs again: the gate can't pass (the child
is not terminal, so `all_complete` is false), but its entry shows the stale
result and it drops out of `outstanding`, which a coordinator reading the
directive would take as the child's answer. Once the child lands in a terminal
again, and before that tick records its new result, the stale copy would let a
gate-waiting parent pass on the old answer. With both changes, a retried child
reports no result until its new arrival records one.

The arrival gate trades one repair path away. Today a session whose index or
parent append failed is re-tried on every later tick until it is removed.
Under the gate, once step 2 has landed, a later tick of a *retained* session
writes neither again: a failed index append (a full disk, say) leaves that
session unindexed, so wake doesn't learn its dispatched child finished, and a
failed parent append leaves the parent without the notice (the gate is still
right, since it reads the child from disk). For a session koto removes, the
removal tick re-sends the notice; the index entry is not re-written, because
a session that is gone needs none. This is accepted residual risk: it needs a
write failure between two appends in the same sessions directory, and
re-trying every tick would bring back the unbounded appends the gate exists
to stop.

The converse case is bounded only by the failure itself. While a session's
own log refuses the step-2 append, every tick is an arrival and appends
another index entry and parent notice; readers dedupe, so the gate stays
correct, and the appends stop on the first tick whose step 2 lands. A kept
child in that state also holds its parent's `results_in` at false until
something ticks it after the fault clears: the gate lists it in
`outstanding` with no result, which is the signal to tick it.

If the step-2 append fails, `arrival` stays true on the next tick and steps 4
and 5 run again, which can duplicate a `ChildCompleted` and an index entry. Both are harmless for correctness: the converge keeps the
latest `ChildCompleted` per task and prefers the on-disk child, and the index
reader keeps one entry per session.

#### Alternatives Considered

**Emit on every tick and deduplicate at the readers.** Hoist steps 4 and 5
unguarded and make the converge and index readers ignore repeats. Rejected
because it's the unbounded append the request-lifecycle design refused: a
parked session ticked by a polling loop grows its parent's log without limit,
and every reader of the parent log pays for it.

**Write the index entry only when a session is removed.** Keep step 4 with
removal, so a retained session that comes back is never indexed as terminal.
Rejected because a retained session with no entry is invisible to wake, which
would never wake the requester of a dispatched child that failed, and is
visible to discovery and caps as if it were live, so a finished session with
`needs_agent` set and no claim could be offered for dispatch again. The
staleness the chosen option accepts after a rewind is the lesser cost.

**Check the parent's log before re-sending a deferred notice.** On the removal
tick, look for a `ChildCompleted` for this child "at or after its current
arrival", and append only if none is found. Rejected because the child's
arrival and the parent's events live in two logs whose sequence numbers don't
order against each other; after a retry, an earlier arrival's failure notice
can satisfy the check and the parent would record a stale outcome. The
unconditional re-send is simpler and can't be fooled.

**A new marker event recording that delivery happened.** Append a
`terminal_delivered` event after step 5 and gate on it. Rejected because the
`request_store.result` event already marks the arrival and is already what
`koto status` and result-map pinning read; a second marker could disagree with
it after a partial failure, and needs its own compatibility handling in the
event log.

**Keep the guard and add a second flag that forces delivery.** Let a caller
pass `--deliver` alongside `--no-cleanup` to get both. This is the smallest
code change: the existing guard stays, and callers opt in. Rejected because
every caller that keeps a child's record would have to know to pass it, which
is the discovery problem the issue describes for `--no-cleanup` itself, and
because the PRD's Out of Scope already records that no caller wants a result
withheld, so the flag would only ever be set one way.

### Decision 3: How retained descendants are removed with their parent

PRD R9 requires that a retained session not outlive a parent koto removes or
replaces. What's open is how the walk finds and removes descendants without
ever deleting live work, what it costs, and how it behaves when a session
can't be read. Without it, a parent's retained children would be left with a
`parent_workflow` naming a session that no longer exists, out of reach of
`koto workspace prune --root`, and a new run reusing the parent's name would
inherit the old run's children as if they were its own.

#### Chosen: a guarded post-order walk, run only for parents that had children

The sweep is a new function, `sweep_terminal_descendants(backend, parent)`,
called just before step 6 removes a parent and before `init_entry`'s replace
path removes a finished session. It:

- **runs only for a session that can have children.** It runs when the
  session's compiled template declares a `materialize_children` hook (how koto
  already recognises a coordinator) or its own log holds a `ChildCompleted`
  (a legacy `koto init --parent` child reported to it). The template check
  comes first and needs no log scan, and it doesn't depend on a notice that a
  failed write could have lost. A leaf session, which is most terminal ticks,
  never lists sessions at all.
- **walks with a visited set and a depth cap**, the same guard
  `measure_depth_from_parent` in `src/engine/caps.rs` uses, so a
  `parent_workflow` cycle (constructible by removing a parent and re-creating
  it under its own former child) ends the walk instead of hanging the tick.
- **checks before it descends.** A descendant that isn't terminal is not
  removed and not walked below: it and everything under it are live work.
- **removes post-order.** A session is removed only when it is terminal and
  every session under it was removed. A terminal child with a live grandchild
  stays, so the grandchild keeps its parent.
- **fails toward keeping.** Any error reading or classifying a session (a
  missing template, an unreadable log, a failed download on the cloud backend)
  leaves that session and its subtree alone.
- **leaves a pending leg alone.** A descendant bound to a request leg that is
  still open was kept because its promotion failed and waits for a retry;
  removing it would leave the leg open for good, so the sweep skips it.
- **re-checks just before removing.** Each session's terminal status is read
  again immediately before `backend.cleanup`, narrowing the window in which a
  concurrent `koto rewind` could bring it back.
- **never blocks the parent.** A failure to remove a descendant warns on
  stderr and the parent's removal goes ahead; the leftover is visible in
  `koto workflows --orphaned` and removable with `koto session cleanup`.

Terminal status comes from `derive_terminal_status` in `src/cli/workspace.rs`
(made `pub(crate)`, with its `TerminalStatus`), so the sweep and prune agree on
what "terminal" means, abandoned included. The walk itself is new: prune's
`collect_descendants` gathers every descendant without stopping at live ones,
which is right behind prune's confirmation prompt and wrong for an implicit
removal. Prune's walk gets the same visited-set guard, since the cycle hangs it
today too.

This removes children a caller kept with `--no-cleanup` at a success terminal
too. The PRD accepts that: a caller who wants a child's record to outlive its
parent keeps the parent.

#### Alternatives Considered

**Reuse `collect_descendants` and filter to terminal sessions.** Collect the
whole tree, then remove the terminal ones. Smallest change. Rejected because a
failed grandchild under a live coordinator is terminal and would be removed
while that coordinator is about to `retry_failed` it, which destroys exactly
the record retention exists to keep; and because the function has no cycle
guard.

**Leave retained children as orphans.** Rely on `koto workflows --orphaned` and
`koto session cleanup`. Rejected because retention would have no automatic end
for the common case of a parent that finishes after one child failed and was
skipped, the orphans would be out of reach of tree-level commands, and a new
run under the parent's name would pick them up.

**Remove only descendants retained because of a failure terminal.** Honour a
child's `--no-cleanup` past its parent's removal. Rejected because the session
doesn't record why it was kept, so this needs a new marker, and the result is
still an orphan with no tree-level reclaim path.

**Trigger on a `ChildCompleted` in the parent's log alone.** Cheap, and true
of any parent whose children reported. Rejected because a retained failure
child whose parent notice failed to write never re-sends it (it is never
removed, and its arrival is already recorded), so its parent would finish
without sweeping it and leave exactly the orphan the sweep exists to prevent.

**Sweep on every removal, with no trigger.** Simpler. Rejected on cost: `backend.list()` reads every session header, and
the code notes workspaces of around 26,000 sessions; on the cloud backend it is
an S3 list plus a download per session. Most terminal ticks are leaf children
that have nothing to sweep.

## Decision Outcome

The three decisions fit together as one rule: a terminal arrival always
delivers, a failure terminal is always kept, and a kept session lives until it
recovers, its parent goes, or an operator removes it.

Concretely, `finish_terminal_tick` changes shape. It computes `arrival` from
the record it's handed and `retain` from the terminal's `failure` flag and
`--no-cleanup`. On arrival it appends the child-log result, promotes and
writes the index entry and appends `ChildCompleted`, independent of `retain`.
If `retain` is false it re-sends `ChildCompleted` when this isn't the arrival
and, when nothing deferred, sweeps the session's terminal descendants and
removes it. The converge reads only a result recorded for a child's current
arrival. `koto init --replace-terminal` runs the same sweep before it replaces
a finished session.

The caller computes the same `retain` value before printing, so the
`action: "done"` response carries `retention`:

- `{"retained": false}`
- `{"retained": true, "reason": "failure_terminal"}`, which wins when both
  apply
- `{"retained": true, "reason": "no_cleanup"}`

`retained: false` states koto's intent for the tick. If the parent append or
the leg promotion fails and defers removal, the session stays until a later
tick removes it, and stderr says so, as today.

`--no-cleanup`'s help text becomes "Keep the session after it reaches a
terminal state (a failure terminal is always kept)".

Nothing else in koto changes behaviour. `retry_failed`, `koto rewind`, `koto
status`, `koto context get`, `koto workflows`, `koto workspace prune` (beyond
its cycle guard), `koto session cleanup` and the refusals for reusing a
retained name already do the right thing once the session is on disk and its
result is recorded.

## Solution Architecture

### Components touched

| Component | Change |
|-----------|--------|
| `src/cli/mod.rs`, `finish_terminal_tick` | Reorder around `arrival` and `retain`; hoist the index entry and parent notice out of the guard; re-send the notice on a non-arrival removal; call the sweep before cleanup. |
| `src/cli/mod.rs`, new `terminal_retention(compiled, final_state, no_cleanup)` | Returns `Option<RetentionReason>`; used by both call sites before printing and passed to `finish_terminal_tick`. It derives the failure case from `project_terminal_outcome(..) == TerminalOutcome::Failure`, the projection `ChildCompleted` already uses, so the notice and the retention rule can't disagree about what a failure terminal is. |
| `src/cli/mod.rs`, both terminal call sites | Attach `retention` to the response before printing. |
| `src/cli/mod.rs`, `Next` clap args | `--no-cleanup` help text. |
| `src/cli/next_types.rs`, `NextResponse::Terminal` | New `retention: Option<Retention>` field, serialized as `retention` when present, set through a `with_retention` builder beside `with_terminal_result`; every construction site gains `retention: None`. |
| `src/cli/batch.rs`, converge | Live-child dereference uses `recorded_result_for_current_arrival`; the parent-copy fallback is skipped for a readable, classified on-disk child. A child known only from its parent's `ChildCompleted` keeps its full session name, so a cleaned-up `<parent>.<task>` child of a parent without a batch hook matches its result (before, it was listed as `<task>` and stayed outstanding). |
| New `sweep_terminal_descendants` (in `src/cli/workspace.rs`) | The guarded post-order walk. |
| `src/cli/workspace.rs` | `derive_terminal_status` and `TerminalStatus` become `pub(crate)`; `collect_descendants` gains a visited set. |
| `src/cli/init_entry.rs`, replace path | Calls the sweep before replacing a finished session. |

### Data flow on a terminal tick

```
koto next <name>            (advance loop or --to)
  |
  |- terminal_record()          -> result, already_recorded
  |- terminal_retention()       -> None | FailureTerminal | NoCleanup
  |- print response (result, retention)
  |
  '- finish_terminal_tick(record, retention)
       arrival = !already_recorded
       if arrival: append request_store.result to own log
       promote to leg (unchanged; no-op once the leg holds a result)
       if arrival: append terminal-index entry
       if arrival or retention is None:
           append ChildCompleted to parent              -> maybe defer
       if retention is None:
           if no deferral:
               if template has materialize_children or log has ChildCompleted:
                   sweep terminal descendants (guarded, post-order)
               backend.cleanup(name)
```

### Interfaces

`retention` on the terminal response:

```json
{"action": "done", "state": "done_blocked", "advanced": true,
 "result": {"status": "failure", "summary": "failed at done_blocked"},
 "retention": {"retained": true, "reason": "failure_terminal"}}
```

`reason` is one of `failure_terminal` and `no_cleanup`, and is absent when
`retained` is false. The field is added only on `koto next`; responses built
without a session (dispatch, unit tests) omit it, as they omit `result`.

No event types, file layouts or other command outputs change.

### How a consumer reads a retained child

Every read goes through a koto command against the child's session name
(`<parent>.<task>` for a batch child); nothing reads under the koto home.

| Need | Command |
|------|---------|
| Why it failed | `koto context get <child> failure_reason` |
| Any other key the child wrote (a running record, a plan) | `koto context get <child> <key>`, or `koto context list <child>` to see which exist |
| Its final state and result | `koto status <child>` (`is_terminal`, the state, and `result`) |
| Every child's outcome and result at once, from the parent | the parent's `children-complete` gate output on `koto next <parent>` or `koto status <parent>`: per child `outcome`, `result`, `reason_source` |
| Whether it is still on disk | `koto workflows` lists it; the terminal response's `retention` said so on the tick it ended |
| Retry it | `retry_failed` evidence on the parent: `koto next <parent> --with-data '{"retry_failed": {"children": ["<task>"]}}'` |
| Rewind it one step | `koto rewind <child>` |

A coordinator's reconcile step reads the gate output for the batch and then
`koto context get` for the children it needs detail on. `/execute`'s
`retry_failed` needs nothing new: the child it names is on disk, so validation
finds it and the retry appends `Rewound` to its log.

### Test surface

The PRD's acceptance criteria are the test list; the PLAN names the test for
each. New tests live in `tests/child_retention_test.rs` (delivery, retention,
the response field, retry and rewind) and `tests/descendant_sweep_test.rs` (the
sweep, including a `list()`-counting backend double for the leaf-tick check).
Existing contracts change with their tests: a parked terminal emits its parent
notice once on arrival (`tests/terminal_result_test.rs`,
`tests/request_dispatch.rs`); a terminal without a result map records its
result under `--no-cleanup` (`tests/terminal_result_test.rs`); tests that
faked a retry by deleting a child that had already reported now rewind it
(`tests/batch_scheduler_test.rs`); tests that relied on a flagged child
withholding its result to hold a gate open start their children in a terminal
state instead (`tests/gate_field_substitution_test.rs`); and a failed child is
kept without the flag (`tests/batch_child_cleanup_test.rs` and similar).

## Implementation Approach

1. **Delivery on arrival and current-arrival reads.** Rework
   `finish_terminal_tick` around `arrival`: step 2 on every arrival, steps 4
   and 5 hoisted and gated, the non-arrival re-send. Switch the converge to the
   current-arrival read and stop its parent-copy fallback for a known-live
   child. Update the parked-terminal tests.
   On its own this already fixes a flagged child's delivery.
2. **Retention rule and response field.** Add `terminal_retention`, the
   `retention` field on both call sites, the retain branch, and the help text.
   Tests for failure-terminal retention, retry, rewind, reads and the response.
3. **Descendant sweep.** Add the guarded walk, call it from
   `finish_terminal_tick` and from the replace path, and add the cycle guard to
   prune's walk. Tests for the sweep's reach and its limits, including a cycle,
   a live coordinator above a failed grandchild, and an unreadable template.
4. **Docs.** `docs/guides/cli-usage.md`, `docs/workspace-layout.md`, the
   koto-user skill and its command and response references, the functional
   feature files that describe cleanup, and `CHANGELOG.md`.

Steps 1 to 3 touch neighbouring code and are each small; they can land as one
PR, with step 1 first so that retention never exists without delivery.

## Security Considerations

The change keeps data on local disk longer and removes some of it
automatically. It adds no network access or dependency, and no command-line
input. It does add one input to a deletion decision: other sessions'
`parent_workflow` fields now steer what koto removes on its own.

**Data retention.** A failed session's context now stays on disk after its
terminal. Context can hold whatever a skill wrote: plans, review notes, URLs,
and sometimes command output. It lives under the same directory, with the same
permissions, that it already occupied while the session ran, and it's readable
by exactly the users who could read it a tick earlier. What changes is
duration: until recovery, parent removal or an operator command. On the cloud
backend the remote copy persists for the same span. The docs state this, and
`koto session cleanup` and `koto workspace prune` remain the way to remove it
on demand. A user who stores secrets in context should already treat the
session directory as sensitive; retention extends the window.

**Implicit deletion.** The sweep removes sessions nobody named on the command
line, so it's built to fail toward keeping (Decision 3): it never enters or
removes a live session, never removes a session with a live descendant, stops
on a `parent_workflow` cycle, skips any session it can't read or classify, and
re-checks status right before each removal. Every session it can reach belongs
to the same user and sits in the same sessions directory as the parent, so no
privilege boundary is crossed. Session ids are validated before any path is
built, and removal goes through `backend.cleanup`, which removes only
`base_dir/<id>`. Symlinked session directories are never candidates, because
`backend.list()` skips entries whose file type is a link, and `remove_dir_all`
removes a link rather than following it.

**Races.** The sweep takes no lock on the descendants it inspects. A `koto
rewind` landing between the re-check and the removal can still lose a session,
and a child that reaches a terminal after the sweep's listing can be left
behind as an orphan. Both windows are milliseconds wide and need two processes
driving the same tree at once; the orphan is recoverable with `koto session
cleanup`. This is accepted residual risk.

**Name reuse.** `parent_workflow` holds a name, not a unique identity. Running
the sweep in the replace path means a new run under a reused name starts
without the old run's terminal children. A live child of the old run still
survives a replace and would be seen by the new run, as it is today.

## Consequences

### Positive

- A failed child can be retried by its parent and rewound by hand, so one
  failure no longer forces a re-run of the parent.
- A failure reason and a skill's running record survive the failure, readable
  through koto's own commands.
- `--no-cleanup` becomes safe on a child, so a skill can keep a child's record
  without starving its parent, and shirabe's root-only workaround can go.
- A caller learns from the response whether its record is still there.
- A batch stuck on a child flagged under an older koto repairs itself on the
  child's next tick.
- The converge can no longer read a retried child's stale result, and prune's
  walk can no longer hang on a cycle.

### Negative

- Failed sessions use disk until something removes them, and under kept roots
  that means until someone prunes.
- `--no-cleanup` changes meaning; a caller that relied on a flagged child not
  reporting will see the result arrive.
- The sweep removes a child kept with `--no-cleanup` when its parent is
  removed, which a caller might not expect.
- A retained failed root now occupies its name, so re-running `koto init` with
  the same name meets the existing "already exists" refusal more often.
- `koto session cleanup` on a retained parent removes only that session, so
  its retained children are left naming a parent no session holds. They are
  listed by `koto workflows --orphaned` and removable one at a time with
  `koto session cleanup`, or with the whole tree by
  `koto workspace prune --root` on the terminal root above them.
- Children moved aside by a rewind of their batch parent (renamed to
  `<parent>~N.<task>`) point at a parent name that no session holds, so neither
  the sweep nor prune reaches them. That's true today for live and flagged
  children; retention makes it true for every failed child of a superseded
  run.

### Mitigations

- The storage cost is stated in the docs with the commands that reclaim it, and
  it's small per session.
- The CHANGELOG entry and the koto-user skill describe the new `--no-cleanup`
  meaning and the `retention` field.
- The sweep's reach is stated beside `--no-cleanup` in the CLI guide.
- The existing refusal already names `koto session cleanup` and
  `--replace-terminal`; the docs call out retained failures as the common case.
- Superseded-epoch children are listed by `koto workflows --orphaned` and
  removable with `koto session cleanup`; the docs say so. Teaching the sweep to
  follow a rewound parent's epochs is a follow-up.
