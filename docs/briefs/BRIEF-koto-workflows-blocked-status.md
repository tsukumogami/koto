---
schema: brief/v1
status: Draft
problem: |
  An operator can't tell from Claude Code's `/workflows` screen that a koto
  session is stopped on a failing gate: the entry is counted as completed, its
  active step reads "Stopped", and the gate's FAIL is hidden. A finished
  session has the mirror fault, with its last phase shown as unfinished.
outcome: |
  An operator opens `/workflows` and reads that a koto session is blocked, and
  on which gate, without leaving the screen; a finished session reads done all
  the way down. Everything the entry already shows correctly keeps working.
---

# BRIEF: a gate-blocked koto session reads as blocked in `/workflows`

## Status

Draft

Framing for the fix to tsukumogami/koto#553, with tsukumogami/koto#554 taken
in as the same field. It follows the phase-detail work, which promised that a
gate-blocked session would read as blocked and wrote a file that, on Claude
Code 2.1.314, doesn't.

## Problem Statement

koto writes one entry file per session into the hosting Claude Code session's
`/workflows` directory, and the screen renders it as a run. When a session
stops on a gate that didn't pass, koto writes that entry with a top-level
status of `blocked` and the failed gate as the active phase's result. The
operator opening `/workflows` sees none of it. The list counts the session
among the completed runs and gives it no icon. The detail header has no status
word. The active phase's step reads "Stopped", and its outcome is a generic
"The workflow stopped before this agent finished." instead of `gate
ready_flag: FAIL`. An operator who wants to know whether a session needs them
has to leave the screen and ask koto.

The cause is that Claude Code's screen doesn't know the word koto chose.
Reading what Claude Code 2.1.314 actually ships shows a fixed set of run
statuses (pending, running, completed, failed, killed, paused). The list has
two buckets, running and completed, and every status that isn't `running`
lands in completed. Only a running run counts as live, so on any other run a
step still marked in progress is drawn as stopped. `blocked` is none of the
six, so the screen falls through to its defaults on every surface.

A finished session has the mirror problem (#554). koto writes the step for the
phase a session is in as in progress whatever the run's status, so on a
completed run the terminal phase reads `Done 0/1`, the header counts one step
short, and the terminal step reads "Stopped". Both faults sit in the same
place: how koto marks the active phase's step, and which status it trusts the
screen to understand.

## User Outcome

An operator driving koto sessions inside Claude Code opens `/workflows` and can
tell at a glance which session needs them. A session stopped on a failing gate
isn't counted among the completed runs. Its active step reads Blocked rather
than Stopped, and the step's outcome is the gate that failed and its result,
so the operator knows what to fix. The line under the title says the session
is blocked, so the state is visible before drilling in. Once the operator
fixes the condition and the session moves on, reopening the screen shows it
running again, and when it finishes it reads done everywhere, with no phase
left looking unfinished.

Nothing that already reads correctly changes for the worse. Each entry still
shows the session's current state, a reopen still picks up the new state, a
finished session still reads done, and a session with rendering turned off or
with no Claude Code session to write into still writes nothing. A tool that
reads the entry file for koto's own view of the session can still find that
the session is blocked.

## User Journeys

### Spot a session stuck on a gate

An operator has several koto sessions in one Claude Code session; one of them
has stopped because its verify gate failed. Trigger: opening `/workflows`.
Outcome shape: the stuck session isn't counted among the completed runs, its
summary line says it is blocked on that gate, and opening it shows the verify
step as Blocked with `gate <name>: FAIL` as its outcome. The operator knows
which session to attend to and what it is waiting on.

### Unblock it and watch it finish

The operator makes the gate's condition true and advances the session.
Trigger: reopening `/workflows` after each advance. Outcome shape: the session
reads running while it moves, then done once it reaches its terminal state,
with every phase counted complete and no step reading Stopped.

### Read koto's own classification from the file

A script or another tool reads the entry file rather than the screen, wanting
to know whether koto considers the session blocked. Trigger: parsing the file
after a gate failure. Outcome shape: koto's own classification is still there
to read, even though the top-level field now speaks the screen's vocabulary.

### Nothing appears where nothing should

An operator runs koto with rendering turned off, or outside any Claude Code
session. Trigger: a session hits a failing gate. Outcome shape: no entry file
is written, exactly as before.

## Scope Boundary

**In:**

- How a gate-blocked session's entry reads in the `/workflows` list, the
  detail header and summary line, and the active step's display and outcome.
- How a finished session's terminal phase and step read (#554), including a
  failed session's terminal step, because it is the same field.
- Keeping koto's own blocked classification readable in the entry file for
  machine readers.
- The contract version, the shape fixture and shape test, the verification
  script and guide, and any koto-skills text that describes the entry, as far
  as the change makes them untrue.
- Grounding every rendering claim in what Claude Code 2.1.314 ships, written
  down in koto's own words.

**Out:**

- The `0s` durations every koto entry shows. The file omits a duration and the
  screen computes 0s from that, but blocked-or-not doesn't depend on it; it is
  a separate fix.
- Making the screen's stop and pause keys act on a koto entry. Claude Code
  ignores them for a file it didn't start, and koto can't change that from the
  file.
- Live refresh while `/workflows` is open. The screen reads the files once per
  opening; that is Claude Code's behavior.
- Parent and child sessions rendering together, and sessions waiting on
  anything other than a failed gate (evidence, a decision, a child). Only the
  gate-blocked case is in.
- Changing when koto considers a session blocked. The predicate stays as it
  is; only how the entry presents it changes.

## References

- `docs/briefs/BRIEF-native-workflows-phase-detail.md`: the feature that
  introduced the `blocked` status this brief corrects.
- `docs/designs/current/DESIGN-native-workflows-phase-detail.md`: the field
  mapping being changed.
- `docs/guides/native-workflows-verification.md`: the verification that must
  stay true.
