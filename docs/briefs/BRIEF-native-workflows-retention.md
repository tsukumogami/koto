---
schema: brief/v1
status: Draft
problem: |
  Every koto session an operator runs inside Claude Code leaves an entry in
  that Claude Code session's `/workflows` list, and nothing ever removes one.
  Over a long working session koto's finished entries pile up and crowd out
  Claude Code's own workflows, and a koto session that stopped without
  finishing keeps reading as running forever, so the list stops saying what
  is actually live.
outcome: |
  An operator opens `/workflows` at any point in a long Claude Code session
  and reads it at a glance: koto's finished entries stay under a stated count
  next to Claude Code's own, and a koto session that died mid-run stops
  claiming to be running after a stated window, or the operator knows from
  koto's docs how to tell a live entry from a stale one.
---

# BRIEF: keep koto's `/workflows` entries bounded and honest over time

## Status

Draft

Framing for the lifecycle work the native `/workflows` render left for later:
the initial render, the phase detail and the blocked status made a koto
session readable in Claude Code's `/workflows` screen, and the conventions
guard watches the file shape against Claude Code releases. None of them
decided what happens to an entry once its session is over, or once its
session silently stops. This is the work tracked in koto issue #190.

## Problem Statement

Since rendering became the default, every koto session an operator drives
inside Claude Code writes an entry into that Claude Code session's
`/workflows` directory, right beside Claude Code's own workflow files. The
entry is rewritten on every committed step and stays when the session ends.
Nothing removes it afterwards: not koto's session cleanup, not its store
pruning, not the session finishing. Cancelled sessions' entries stay too.

That's fine for a handful of sessions and bad for a working day. An operator
who runs dozens of koto workflows in one Claude Code session opens
`/workflows` and finds koto's finished entries filling the list, sorted in
among, and pushing down, the Claude Code workflows they came to look at. The
list has no bound, so it only gets worse the more koto gets used, and koto is
used most by exactly the operators who most need the list to stay readable.

The second half is worse because it misleads rather than clutters. koto runs
as a command, not a daemon: between commands no koto process exists. When
the agent driving a koto session is killed, its Claude Code session ends, or
the machine goes to sleep, the last thing koto wrote is what the entry keeps
saying, and that's usually "running". The entry then reads as running
indefinitely, counted in the screen's "running" total, indistinguishable from
a session that is genuinely in progress. The operator can't trust the list's
running count, and can't tell a dead entry from a live one without going to
koto's own tools.

## User Outcome

An operator working through a long Claude Code session opens `/workflows`
whenever they like and can read it at a glance. koto's finished entries never
exceed a stated count in that list, so Claude Code's own workflows stay
visible next to them no matter how many koto sessions have come and gone. The
operator doesn't do anything to get this: no command to run, no setting to
turn on.

When a koto session dies without finishing, the operator isn't left looking
at a permanent "running". After a stated window the entry stops claiming to
be running, or, if koto can't honestly tell from what it has, koto's docs say
plainly how an operator tells a live koto entry from a stale one. Either way
the operator knows what a "running" koto entry means.

Through all of this, files koto didn't write are never touched. Claude Code's
own workflow files and anything else in the directory stay exactly as they
were.

## User Journeys

### Run many koto sessions in one Claude Code session

An operator spends a day in one Claude Code session and drives many koto
workflows to completion there. Trigger: opening `/workflows` late in the
day. Outcome shape: the number of koto entries in the list is at or under
the stated bound, the most recent ones are the ones kept, and Claude Code's
own workflows are still visible. Counting the koto files in the session's
`workflows` directory gives the same answer.

### Come back to a session that died mid-run

An operator's agent is killed, or their Claude Code session crashes, while a
koto session is partway through. Trigger: returning after the stated window
and opening `/workflows`. Outcome shape: the entry no longer reads as
running; it reads as stopped or stale. If the chosen approach is to document
the limitation instead, the operator finds in koto's docs a stated way to
tell this entry is stale rather than live.

### Keep Claude Code's own files safe

An operator's `workflows` directory holds Claude Code's own workflow files
and possibly files from other tools. Trigger: koto applying its retention
policy after a session finishes. Outcome shape: only files koto wrote, and
can recognize as its own, are ever removed or rewritten; every other file is
untouched.

### Change the file without breaking the screen

A koto maintainer changes what the entry's file holds as part of this work,
for example a new status word or field. Trigger: opening the pull request.
Outcome shape: the existing Claude Code conventions guard is extended in the
same change to cover what was added, so a later Claude Code release that
stops recognizing it fails CI rather than an operator's screen.

## Scope Boundary

**In:**

- A retention policy for koto-written `/workflows` files in each Claude Code
  session's `workflows` directory, with a stated numeric bound that holds
  across any number of runs.
- The same policy applied to completed, failed and cancelled sessions'
  files alike.
- What the operator sees for a koto session that stopped without finishing:
  either the entry is reconciled away from "running" within a stated window,
  or the limitation is documented with a stated way to tell live from
  stale. The design picks which, with reasoning.
- The ownership rule: which files koto treats as its own, so nothing else in
  the directory is deleted or modified.
- Operator-facing documentation of the policy and the staleness behavior,
  and tests that show the bound holds and foreign files are untouched.
- Extending the existing Claude Code conventions guard in the same change
  when the file's shape or status vocabulary changes.

**Out:**

- Any dependency on niwa or any other workspace tool. A koto user who
  doesn't use niwa gets the full behavior; nothing here asks a niwa user to
  use koto.
- Pruning koto's own session store. Store retention is a separate, existing
  policy; this work governs only the files koto writes into Claude Code's
  directory.
- Rendering coordinator and delegate hierarchies, or fan-out, as
  `/workflows` entries. That is separate work.
- A separate blocked count or bucket in the list. Claude Code's screen
  decides its buckets; that limitation is tracked on its own.
- The rendered smoke check and the guard's verdict on it. This work extends
  the guard's fixture where its own change needs it and nothing more.
- A long-running koto process, daemon or per-turn hook to watch sessions.
  koto stays a command-line tool; any reconciliation happens when koto
  already runs.
- Cleaning up Claude Code's own workflow files. Those belong to Claude Code.

## Open Questions

- What is the numeric bound, and is it a count, an age, or both? The PRD
  states the requirement; the design picks the number and says why.
- Is staleness reconciled from what koto can observe, or accepted as a
  documented limitation? Both satisfy the outcome; the design decides.
- Does this work close koto issue #190 in full? The PRD should say which of
  its acceptance criteria it covers.

None of these questions block the brief: each one is a decision the PRD or
the design owns.

## References

- `docs/briefs/BRIEF-native-workflows-render.md` -- the initial render's
  framing, which deferred retention and crash-staleness.
- `docs/designs/current/DESIGN-native-workflows-render.md` -- where the file
  is written and when; its consequences section carries retention and
  crash-staleness forward as known limitations.
- `docs/designs/current/DESIGN-koto-workflows-blocked-status.md` -- what
  Claude Code's screen does with each status word.
- `docs/designs/current/DESIGN-native-workflows-ci-guard.md` -- the
  conventions guard any shape change here must extend.
