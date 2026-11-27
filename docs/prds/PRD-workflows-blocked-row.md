---
schema: prd/v1
status: In Progress
problem: |
  An operator scanning Claude Code's `/workflows` list can't tell a koto session
  stopped on a failing gate from one that is advancing. Both rows draw the same
  running icon, the same title and the same "0s", because the row is built only
  from the entry's top-level status, its title and three number fields, and the
  blocked reading koto writes (summary suffix, Blocked step, `koto.status`)
  reaches none of them. Finding the stuck session means opening every entry.
goals: |
  On the Claude Code release current when checked, a gate-blocked koto session's
  own list row reads blocked and a running session's row doesn't, without
  opening either entry. The blocked session stays in the running count. The row
  goes back to the running reading on the first write after the gate passes or
  the session moves on, and the shape is pinned by the native-workflows guard
  fixture so a regression fails a test.
absorbed:
  - docs/briefs/BRIEF-workflows-blocked-row.md
---

# PRD: a gate-blocked koto session's `/workflows` row reads blocked

## Status

In Progress

Absorbed [BRIEF-workflows-blocked-row](docs/briefs/BRIEF-workflows-blocked-row.md); carried in Absorbed Brief.

Statements below about what the screen draws refer to Claude Code 2.1.325, the
release installed when these requirements were researched. The outcome is
verified on that release; a later release that draws the row differently is
what R17's guard exists to catch, not a failure of these requirements. Its list-row code is
the same in 2.1.322 and 2.1.323 and matches the account of 2.1.314 in
`docs/designs/current/DESIGN-koto-workflows-blocked-status.md`.

## Absorbed Brief

An operator running several koto sessions under one Claude Code session can't
tell, from the `/workflows` list, a session stopped on a failing gate from one
that is advancing: both rows show the same icon and title, because the blocked
reading koto writes lives only in the detail view. Finding the stuck session
means opening every entry, and a different status word isn't an answer,
because it would move the session into the completed count.

The outcome the operator should get: from the list alone, the gate-blocked
session's row reads blocked and the running one's doesn't, both still counted
running, and the row reads running again once the session recovers. A reviewer
can confirm the same thing from the file koto writes without starting Claude
Code, and a regression fails a test. The feature holds in what koto writes for
the row, its clearing, a revisit of the earlier decision against a blocked
title, a recorded choice for when Claude Code adds a blocked count, automated
coverage and a manual list check; it pushes out the summary counts, the
upstream blocked count, the detail view, starting Claude Code in tests, other
`/workflows` milestones, and anything requiring niwa.

## Problem Statement

koto shows each of its sessions in Claude Code's `/workflows` screen by writing
a `koto-<uuid>.json` file into the hosting session's workflows directory. The
screen has a list, one row per entry under a summary line counting "N running"
and "N completed", and a detail view each row opens.

The fix for koto#553 made a gate-blocked session read blocked in the detail
view and kept it out of the completed count by writing top-level `status:
running`. On Claude Code 2.1.325, though, a list row is built from exactly
three things:

- an icon chosen by the top-level `status` alone (running is a dim dot);
- a label, `workflowName`, falling back to `summary` and then "Dynamic
  workflow", drawn as plain text with control characters stripped and cut
  with an ellipsis at no more than 50 columns;
- a dim facts column from `durationMs`, `agentCount` and `totalTokens`, none of
  which koto writes, so every koto row ends in "0s".

Steps, the `blocked` flag, a step's `error` and `summary` (while `workflowName`
is present) never reach the row. So a session stuck for an hour on a CI gate
and a session moving normally render as the same row, character for character.
An operator with several sessions open has to open each entry to find the stuck
one, and the screen reads its files only when it opens, so they repeat that
every time they look.

Changing the status word to get a different icon isn't an answer: every status
other than `running` lands in the "completed" count, which tells the operator
the stuck session finished. A blocked count in the summary line is Claude
Code's to add (koto#590). What koto controls is the row's content, and today
nothing in it differs between a stuck session and a moving one.

## Goals

An operator running several koto sessions under one Claude Code session opens
`/workflows` and, from the list alone, sees which sessions are stopped on a
failing gate and which are moving, and still recognizes which session each row
is. The blocked session stays in the running count, so the summary line
doesn't claim finished work. Once the session stops being blocked (the events
R6 lists), its row reads like any other running session the next time the
list is opened.

A reviewer can confirm the same thing without starting Claude Code, by running
a session whose gate fails and then passes and reading the file koto wrote at
each point, and a later change that drops the blocked row fails a test.

## User Stories

- As an operator with two koto sessions under one Claude Code session, one
  waiting on a gate that keeps failing and one advancing, I want the
  gate-blocked session's list row to read blocked and the other's not to, so
  that I open the stuck one first without opening both. Both rows still count
  as running.
- As an operator who fixed what a failing gate checks, I want the session's row
  to stop reading blocked once its next evaluation passes or it moves on, so
  that a recovered session doesn't keep pulling my attention.
- As a reviewer of a change to the entry file, I want to run a session whose
  gate fails and then passes and see the row's fields mark it blocked and then
  running, and I want the guard fixture to hold that shape, so that a
  regression shows up as a failing test rather than a plain row.
- As a maintainer of a script that reads koto's entry files, I want the
  session's label without the blocked marker, and koto's classification, in
  fields that don't change meaning with the screen, so that my script keeps
  working when the row starts saying blocked.
- As an operator who later upgrades to a Claude Code release whose summary line
  has its own blocked count, I want koto's choice for that case already made
  and written down, so that the row and the count don't contradict each other
  when it arrives.

## Requirements

Terms used below:

- **Row label**: the text Claude Code 2.1.325 draws as a list row's label: the
  entry's `workflowName`, or its `summary` when `workflowName` is absent or not
  a string, or "Dynamic workflow" when both are.
- **Session label**: the label koto derives for a session today (its intent,
  else its template name and current state, else a fallback), which contract
  version 6 writes to both `name` and `workflowName`.
- **Blocked marker**: the text this work adds so a row label reads blocked.
- **Blocked**: koto's existing classification, a non-terminal session whose
  latest gate evaluation in its current state's current epoch failed; the same
  predicate that sets `koto.status` to `blocked` today.

### Functional

- **R1. The blocked row label starts with the marker.** A blocked session's
  entry has a row label whose first whitespace-delimited token is `blocked`
  (lower case; the design may follow it with punctuation such as a separator,
  and fixes the exact form), followed by the session label. Putting the marker first keeps it inside the 50-column
  cut whatever the session label's length or character widths. Neither the
  icon nor the facts column can carry the reading: the icon follows `status`,
  which R3 fixes, and the facts column takes only numbers.
- **R2. The running row label is unchanged.** A session that isn't blocked has
  a row label equal to its session label, exactly as contract version 6 writes
  it, with no blocked marker.
- **R3. Still counted running.** A blocked entry's top-level `status` is
  `running`. On 2.1.325 the summary count reads only `status` (one of
  `pending`, `running`, `paused`, `completed`, `failed`, `killed`, with every
  value but `running` counted completed), so no other field may change it.
- **R4. The session is still recognizable.** On a blocked row label, the
  session label follows the marker unchanged except for the cut R5 allows and
  the escaping R13 applies, for a session with an intent and for one without.
- **R5. The cap cuts the label, never the marker.** When koto's sanitize-and-cap
  shortens the row label, it shortens the session label part; the marker is
  always present in full.
- **R6. It clears on the next write.** Each of these, after a failed gate,
  makes the next entry koto writes carry no blocked marker: a later evaluation
  of a gate in the same state that passes; a transition to another state; a
  rewind (`koto rewind`); re-entering the same state through a transition with
  no gate evaluated since; reaching a terminal state; cancelling the session.
  These are the events that clear the koto#553 blocked summary today.
- **R7. One derivation.** On every entry koto writes, the row label carries the
  marker exactly when `koto.status` is `blocked` and the summary ends with
  `· blocked on gate <name>`. If the marker names a gate, it names the same
  gate the summary names.
- **R8. The detail view keeps what koto#553 settled.** A blocked entry's summary
  still ends `· blocked on gate <name>`, its active step still has
  `state: "error"`, `blocked: true` and `error: "gate <name>: FAIL"`, and
  `koto.status` still carries koto's classification. The design says whether
  the detail view's title changes with the row label and why that's
  acceptable.
- **R9. The unmarked session label stays readable.** Every entry carries the
  session label without the marker in a field the session-feed reference names
  for it.
- **R10. A swept entry doesn't read blocked.** When the retention sweep rewrites
  a stale entry as `killed`, the rewritten row label carries no blocked marker.
  A swept entry that wasn't blocked keeps its row label.
- **R11. What koto writes once a blocked count exists.** The design records what
  koto writes, keeps writing or stops writing once a Claude Code release adds a
  blocked count to the list's summary line, with the reasoning, and whether the
  marker stays. Nothing in this work closes or claims to fix koto#590, which
  stays open.
- **R12. The earlier rejection is revisited.** The koto#553 design turned down a
  blocked title for three stated reasons: it changes nothing about the bucket
  unless the status changes; it leaves the step Stopped or Running; and it
  makes the stable intent-derived title change with state. The design that
  settles the mechanism says, for each, whether it still holds for the row
  label and why its choice differs.

### Non-functional

- **R13. Untrusted text stays sanitized.** The row label, including the session
  label and any gate name in the marker, passes the shared sanitizer and cap
  koto already applies to printed facts.
- **R14. No new reads, best-effort.** The write stays on the existing
  commit-funnel path with no new event-log read, store read or file, and a
  failure to write still never fails the koto command that triggered it.
- **R15. Versioned and documented.** Because the entry's label fields no longer
  carry the same text for every entry, the contract version goes up by one and
  its history comment says why. The session-feed reference, the
  native-workflows verification guide, the koto-user skill's description of
  the entry and the CHANGELOG each say a blocked entry's row label starts with
  the marker.
- **R16. Pinned by the guard fixture.** The native-workflows guard fixture holds
  a blocked golden file and a running golden file, both compared by the shape
  test and both in the shape list the conventions test reads.
- **R17. The guard watches the row label's source.** The conventions fixture has
  a probe that checks, on the Claude Code release the fixture pins, that the
  list row's label is read from the field koto marks. If the pinned release is
  older than the one these requirements were checked against, the design says
  whether the pin moves and why.
- **R18. Reproducible by hand.** The verification guide has numbered steps for
  checking the list on the Claude Code release current when a reader checks:
  two koto sessions, one held on a failing gate and one running, both visible
  as rows (the screen opens a lone entry's detail view directly, so one session
  isn't enough). The steps state the expected row labels and summary line
  verbatim for the guide's example sessions, and name the release they were
  last checked against.
- **R19. No niwa.** Everything here works for a koto user with no niwa
  installed.

## Acceptance Criteria

- [ ] A session with intent `Fix the login bug`, held on a failing gate named
  `tests-pass`, writes an entry with top-level `status` `running`,
  `koto.status` `blocked`, and a row label whose first word is `blocked` and
  which contains `Fix the login bug` after it. (R1, R3, R4, R7)
- [ ] The same session, before any gate evaluation, writes a row label equal to
  `Fix the login bug`. (R2)
- [ ] A session with no intent, held on a failing gate, writes a row label whose
  first word is `blocked`, followed by its template name and state as contract
  version 6 writes them. (R1, R4)
- [ ] A blocked entry's top-level keys are those contract version 6 writes plus
  any the design adds, and none of its top-level values is a Claude Code
  status word other than `running`. (R3)
- [ ] A blocked session whose intent is 2,000 characters long, including wide
  characters, writes a row label that starts with the full marker and is no
  longer than koto's fact cap (1,024 characters, counted as Unicode scalar
  values) plus its cut marker. (R5)
- [ ] A failed evaluation of `tests-pass` produces a marked row label; a
  following passing evaluation of `tests-pass` produces a row label equal to the
  session label, a summary without `blocked`, and no blocked step. (R6, R7)
- [ ] After a failed gate, a transition to another state produces an unmarked
  row label. (R6)
- [ ] After a failed gate, `koto rewind` produces an unmarked row label. (R6)
- [ ] After a failed gate, re-entering the state with no new gate evaluation
  produces an unmarked row label. (R6)
- [ ] After a failed gate, reaching a terminal state produces an unmarked row
  label. (R6)
- [ ] After a failed gate, cancelling produces an unmarked row label. (R6)
- [ ] With two gates in one state where the last evaluated failed, the marker
  (if it names a gate) and the summary name the same gate. (R7)
- [ ] A gate that failed in an earlier visit to the state, with none evaluated
  in the current visit, produces an unmarked row label. (R7)
- [ ] A blocked entry's summary ends `· blocked on gate tests-pass` and its
  active step has `state: "error"`, `blocked: true` and
  `error: "gate tests-pass: FAIL"`. (R8)
- [ ] A blocked entry carries the unmarked session label in the field the
  session-feed reference names for it. (R9)
- [ ] A blocked entry passed through the retention sweep's rewrite comes out
  with `status` `killed` and no blocked marker in its row label; a running
  entry passed through it keeps its row label. (R10)
- [ ] The design document has a section saying what koto writes once a blocked
  count exists, and the commit messages, the PR body and the changed documents
  contain no closing keyword (`fixes`, `closes`, `resolves`) next to koto#590.
  (R11)
- [ ] The design document gives, for each of the three reasons R12 lists, a
  verdict (still holds, or no longer holds) with its reason. (R12)
- [ ] An intent, a state name and a gate name each containing a control
  character or ESC appear in the entry escaped the same way the summary escapes
  them today, and none of them reaches the row label unescaped. (R13)
- [ ] Pointing a session at a workflows directory koto can't write to leaves the
  koto command's exit status and output unchanged. (R14)
- [ ] The change adds no call that reads the event log or the session store to
  the write path; the code review checks `try_materialize` and the functions
  it calls for new reads. (R14)
- [ ] `CONTRACT_VERSION` and both golden fixtures' `contractVersion` equal the
  previous version plus one, and each of the session-feed reference, the
  verification guide, the koto-user skill and the CHANGELOG contains the
  phrase "row label". (R15)
- [ ] Removing the marker from the blocked golden file, or adding it to the
  running one, makes `cargo test --test native_workflows_shape` fail, and
  removing either file from the shape list in
  `tests/native_workflows_conventions.rs` makes that test fail. (R16)
- [ ] `scripts/check-claude-code-conventions.sh` passes on the pinned release,
  and its `--alter` run for the new row-label probe exits non-zero naming that
  probe. (R17)
- [ ] `scripts/verify-native-workflows.sh` fails if the blocked entry's row label
  doesn't start with `blocked`, or if the entry written after the gate passes
  still does. (R6, R18)
- [ ] The verification guide's two-session steps quote the expected row labels
  and summary line and name a Claude Code release. (R18)
- [ ] With no `niwa` on `PATH`, `cargo test --test native_workflows_blocked_test`
  and `scripts/verify-native-workflows.sh` pass. (R19)

## Out of Scope

- **The summary line's counts.** They stay Claude Code's. koto doesn't count a
  blocked session as completed to make its row differ.
- **A blocked count in the summary line.** That's a Claude Code change, tracked
  in koto#590, which stays open. This work records what koto would write once
  it exists (R11) and builds nothing for it.
- **The facts column.** Writing `durationMs`, `agentCount` or `totalTokens`
  to make koto rows' "0s" meaningful is a separate improvement.
- **How a blocked session reads in the detail view,** beyond what R8 allows.
  koto#553 settled it.
- **Entry files already on disk.** An entry written under contract version 6
  is replaced on its session's next write; nothing rewrites old entries for
  this change, except the retention sweep as R10 describes.
- **Starting Claude Code in tests or CI.** Automated coverage reads the file
  koto writes; the list check is a manual step (R18). The existing optional
  smoke script stays optional.
- **Hierarchy rendering, retention policy, and other `/workflows` milestones.**
  R10 touches only how the sweep's existing rewrite treats the marker.
- **niwa.** Nothing here requires or changes it.

## Decisions and Trade-offs

**The reading goes in the row label.** The list row on 2.1.325 is the icon, the
label and the facts column, and nothing else; steps, `blocked`, `error` and
`summary` (while `workflowName` is set) never reach it. The icon follows
`status`, which has to stay `running`, and the facts column takes numbers, none
of which says blocked. That leaves the label. Alternatives considered: a new
top-level field (Claude Code wouldn't read it), a different step shape (the row
ignores steps), a number in the facts column (an agent count doesn't say
blocked, and it also feeds the detail header).

**The status word stays `running`.** On 2.1.325 every other word, `paused`
included, moves the entry into the "completed" count, the same finding the
koto#553 design made on 2.1.314. The milestone outcome rules that out.

**The marker comes first.** The label is cut at no more than 50 columns, so a
marker after a long intent disappears. Requiring the first word to be `blocked`
keeps it visible and makes column-versus-character width irrelevant. How the
marker is written (separator, whether it names the gate, which field carries
it) is the design's.

**A running twin golden fixture.** Today the guard's golden file is only the
blocked entry, and the clearing half is covered by integration tests alone.
R16 adds a running golden so both readings are pinned the same way, at the cost
of one more fixture to regenerate on a shape change.

**A swept entry drops the marker.** A stale blocked entry rewritten as `killed`
is counted completed and its session isn't writing; keeping "blocked" on its
row would contradict the count.

**A contract version bump.** The koto#553 change bumped the version for a
value-only change, and entries written before and after this change sit in the
same directory; the version is how a reader tells them apart.

## Known Limitations

- With a single entry in the list, the screen opens its detail view directly,
  so the row isn't drawn; the detail view is what the operator sees, and it
  already reads blocked.
- The screen reads its files when it opens, so a row reads as of the last time
  the operator opened `/workflows`, not live.
- koto writes the entry only when the session's log changes. If the operator
  fixes what a gate checks without running koto, the row keeps reading blocked
  until the session's next evaluation.
- These requirements are pinned to what 2.1.325 draws. A later release can
  change the row label's source; R17 makes that a failing guard rather than a
  silent loss, but can't prevent it.
- Pressing the stop key on a koto row still does nothing, blocked or not.
