---
schema: brief/v1
status: Accepted
problem: |
  In Claude Code's `/workflows` list, a koto session stopped on a failing gate
  and one that is still advancing look the same: both rows show the session's
  title and the running icon. The blocked reading koto writes lives only in the
  detail view, so an operator opens every entry to find the stuck one.
outcome: |
  An operator scanning the `/workflows` list tells a gate-blocked koto session
  from a running one by its own row, without opening either entry. The summary
  counts stay Claude Code's, a blocked session is never counted as completed,
  and the row reads running again once the gate passes.
motivating_context: |
  The koto#553 fix made a gate-blocked session read blocked once its entry is
  opened, and kept it out of the list's completed count. The list row itself
  was left unchanged, and the blocked bucket the list summary lacks is a Claude
  Code gap tracked in koto#590. This brief frames the part koto can fix: the
  row it writes.
---

# BRIEF: a gate-blocked koto session's `/workflows` row reads blocked

## Status

Accepted

## Problem Statement

koto shows each of its sessions in Claude Code's `/workflows` screen by writing
a `koto-<uuid>.json` file into the hosting session's workflows directory. The
screen has two levels. The list shows one row per entry, with a summary line
above the rows counting "N running" and "N completed". Opening a row shows the
detail view: the line under the title, the phases, and each step's outcome.

The fix for koto#553 settled how a session stuck on a failing gate reads in the
detail view. Its file keeps top-level `status: running`, because on the Claude
Code release that fix was checked against every other status word, `blocked`
included, lands in the completed count. Its summary ends with `· blocked on
gate <name>`, its active step is drawn Blocked with the gate's FAIL as the
outcome, and `koto.status` records `blocked` for scripts.

None of that reaches the list. The row is titled from the file's
`workflowName`, which koto fills from the session's stable label (its intent,
else its template and state), and the row's icon follows the top-level status,
which is `running` for both kinds of session. So an operator looking at a list
with five koto sessions in it, one of them stuck for an hour on a CI gate,
sees five rows that read alike. To find the stuck one they open each entry in
turn. The longer the list, the more likely the stuck session is the one they
don't open.

The obvious fix for the count, a different status word, isn't available: it
would move the stuck session into "completed", which is worse than reading
running. The count itself belongs to Claude Code, and a blocked bucket there is
something koto can only ask for (koto#590). What koto does control is the
content of the row it writes, and today that row carries nothing that differs
between a stuck session and a moving one.

## User Outcome

An operator running several koto sessions under one Claude Code session opens
`/workflows` and, from the list alone, sees which sessions are stopped on a
failing gate and which are moving. They don't open an entry to find out. The
blocked session still sits in the running count, not the completed one, so the
summary line doesn't claim finished work that isn't finished. When the gate
starts passing and the session advances, the next time the operator opens the
list its row reads like any other running session.

A reviewer can confirm the same thing without starting Claude Code: run a
session whose gate fails and then passes, and see it marked blocked while the
gate fails and running once it passes. A later change that drops the blocked
row fails a test instead of shipping a plain row.

## User Journeys

### Spot the stuck session in the list

An operator has two koto sessions under one Claude Code session: one waiting
on a gate that keeps failing, one advancing normally. Trigger: opening
`/workflows` to check on them. Outcome shape: the gate-blocked session's row
reads blocked and the running session's row doesn't, both rows are counted as
running in the summary line, and the operator knows which entry to open
without opening the other.

### Watch a blocked session recover

The operator fixes whatever the failing gate checks, and the session's next
evaluation passes and it advances. Trigger: reopening `/workflows` after the
advance. Outcome shape: the session's row no longer reads blocked; it reads
like the other running sessions, with nothing left over from the blocked
period.

### Check the row's fields in review

A reviewer looking at a change to the entry file runs a koto session whose
gate fails and then passes, and reads the file koto wrote at each point.
Trigger: reviewing the change, or the guard fixture failing after a later
edit. Outcome shape: the fields the list row is built from mark the session
blocked while the gate fails and running once it passes, and the guard fixture
holds that shape so a regression shows up as a failing test rather than a
silently plain row.

### Upgrade to a Claude Code that counts blocked sessions

An operator upgrades Claude Code to a release whose list summary has its own
blocked count. Trigger: opening `/workflows` after the upgrade with a koto
session still stopped on a failing gate. Outcome shape: the session's row and
the summary line agree about it. The row doesn't say blocked while the summary
counts it somewhere that contradicts that, and the operator isn't left
guessing which of the two to believe.

## Scope Boundary

**In:**

- What koto writes so that a gate-blocked session's own list row differs from
  a running session's row, readable without opening the entry.
- Keeping a gate-blocked session out of the list's completed count, as the
  koto#553 fix already does.
- The row returning to its running reading on the first write after the gate
  passes, the session advances, or it ends.
- Revisiting the earlier decision not to say blocked in the session's title,
  with a stated reason the choice now differs or still holds.
- What koto writes once Claude Code's list summary gains a blocked count.
- Automated regression coverage of the blocked row's shape, plus a documented
  manual check of the list on the Claude Code release current when checked.

**Out:**

- The list summary's counts. They stay Claude Code's; koto writes no status
  word that changes them, and doesn't count a blocked session as completed to
  make its row differ.
- A blocked bucket in the summary line. That's a Claude Code change, tracked
  in koto#590, which stays open.
- How a blocked session reads in the detail view. The koto#553 fix settled it,
  and this work doesn't change it beyond what the row's mechanism requires.
- Starting a real Claude Code process in tests or CI. The list check is a
  manual reproduction step; automated coverage reads the file koto writes.
- Anything requiring niwa. A koto user gets the blocked row without niwa
  installed.
- Hierarchy rendering, retention and other `/workflows` milestones beyond this
  one.

