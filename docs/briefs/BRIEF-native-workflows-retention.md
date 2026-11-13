---
schema: brief/v1
status: Draft
problem: |
  Every koto session run inside Claude Code leaves a `/workflows` entry that
  nothing ever removes, so over a long session koto's finished entries pile up
  and crowd out Claude Code's own, and a koto session that stopped without
  finishing reads as running forever. The list stops saying what is live.
outcome: |
  An operator opens `/workflows` at any point in a long Claude Code session and
  reads it at a glance: koto's finished entries stay under a stated count, and a
  koto session that died mid-run stops claiming to be running after a stated
  window, or koto's docs say how to tell a live entry from a stale one.
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

An operator runs Claude Code's own workflows alongside koto sessions in the
same Claude Code session, so its `workflows` directory holds both kinds of
entry. Trigger: the operator finishes a koto session after many others and
opens `/workflows`. Outcome shape: only files koto wrote, and
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

- A retention policy for koto's entries in each Claude Code session's
  workflow list, with a stated numeric bound that holds across any number of
  runs.
- The same policy applied to completed, failed and cancelled sessions'
  entries alike.
- What the operator sees for a koto session that stopped without finishing:
  either the entry stops reading as running within a stated window, or the
  limitation is documented with a stated way to tell live from stale. The
  design picks which, with reasoning.
- koto only ever removing or rewriting entries it wrote itself, and leaving
  every other entry in the list alone.
- Operator-facing documentation of the policy and the staleness behavior,
  and tests that show the bound holds and other tools' entries are
  untouched.
- Keeping koto's compatibility check against Claude Code releases in step
  with any change this work makes to what koto writes.

**Out:**

- Any dependency on niwa or any other workspace tool. A koto user who
  doesn't use niwa gets the full behavior; nothing here asks a niwa user to
  use koto.
- Pruning koto's own session records. Their retention is a separate,
  existing policy; this work governs only koto's entries in Claude Code's
  workflow list.
- Showing coordinator and delegate hierarchies, or fan-out, as workflow
  entries. That is separate work.
- A separate blocked count or bucket in the list. Claude Code's screen
  decides its buckets; that limitation is tracked on its own.
- Checking by hand that Claude Code still displays koto's entries, and the
  verdict on that check. This work touches the compatibility check only
  where its own change needs it.
- Background watching of sessions. koto stays a command-line tool, and any
  reconciliation happens while koto is already running for another reason.
- Cleaning up Claude Code's own workflow entries. Those belong to Claude
  Code.

## Open Questions

- What is the numeric bound, and is it a count, an age, or both? The PRD
  states the requirement; the design picks the number and says why.
- Is staleness reconciled from what koto can observe, or accepted as a
  documented limitation? Both satisfy the outcome; the design decides.
- Which of koto issue #190's acceptance criteria this work covers, and so
  whether it closes the issue. The PRD records the call.

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
