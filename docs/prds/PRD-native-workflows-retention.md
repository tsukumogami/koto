---
schema: prd/v1
status: Draft
problem: |
  Every koto session run inside Claude Code writes a `koto-<session-id>.json`
  entry into that Claude Code session's `workflows` directory, next to Claude
  Code's own files, and nothing ever removes one. With rendering on by default,
  finished entries pile up without bound and crowd out Claude Code's own
  workflows, and a koto session whose driver dies keeps reading as running
  forever, because koto runs no daemon and nothing rewrites its last status.
goals: |
  An operator's `/workflows` list stays legible over a long Claude Code session:
  koto's entries in a directory never exceed a stated bound, a koto session that
  stopped without finishing stops claiming to be running once a stated window
  has passed, and nothing koto didn't write is ever touched. The behavior is
  tested, documented, and reproducible from the implementation's pull request.
upstream: docs/briefs/BRIEF-native-workflows-retention.md
source_issue: 190
---

# PRD: bounded and honest koto entries in Claude Code's `/workflows`

## Status

Draft

Requirements for the lifecycle of the files koto writes into Claude Code's
`/workflows` directories. The brief left two calls open, the retention policy
and how crash-staleness is handled; this PRD states what either answer must
satisfy and leaves the mechanism, the number and the window to the design.

## Problem Statement

koto renders each of its sessions in Claude Code's native `/workflows` screen
by writing one file, `koto-<session-id>.json`, into the viewing Claude Code
session's `workflows` directory. Claude Code keeps its own `wf_*.json` files in
the same directory, and its screen lists every `*.json` file there, newest
`startTime` first, reading them once when the screen opens. koto rewrites its
file on every committed step and leaves it in place when the session ends.

Nothing removes these files. koto's session cleanup, its store pruning, its
workspace pruning and a session's terminal cleanup all work on koto's own
store and never look at the `workflows` directory. Completed, failed and
cancelled sessions' files stay forever. Since rendering became the default,
any koto use inside Claude Code adds files, so an operator who drives many
koto workflows in one Claude Code session finds koto's finished entries
filling the list and burying Claude Code's own workflows.

The second half misleads rather than clutters. koto is a command-line tool:
between commands no koto process exists, and koto keeps no process id,
heartbeat or lease for a workflow session. When the agent driving a session
is killed, its Claude Code session ends, or the machine sleeps, the file keeps
whatever koto last wrote, which is usually `running`. Claude Code counts that
entry in its "running" total indefinitely, and the operator can't tell it from
a session that's still in progress without going to koto's own tools.

This is the work tracked in koto issue #190.

## Goals

- An operator who runs many koto sessions in one Claude Code session finds
  the number of koto entries in that session's `workflows` directory at or
  under a stated bound, with Claude Code's own entries still visible.
- An operator who returns to a koto session that died mid-run doesn't see it
  as running once a stated window has passed, or finds in koto's docs a
  stated way to tell a live koto entry from a stale one.
- Files koto didn't write are never deleted or modified.
- The behavior needs no new command, setting or tool to work, and depends on
  nothing outside koto.

## User Stories

- As an operator who drives many koto workflows to completion in one Claude
  Code session, I want koto's entries in `/workflows` to stay under a fixed
  count, so that I can still find Claude Code's own workflows late in the
  day.
- As an operator whose agent was killed while a koto session was running, I
  want that entry to stop reading as running after a known window, or to know
  exactly how to tell it's stale, so that the screen's running count means
  what it says.
- As an operator who also runs Claude Code's own workflows and other tools in
  the same Claude Code session, I want koto's clean-up to touch only koto's
  own files, so that nothing I or another tool wrote disappears.
- As a koto maintainer changing what the entry's file holds as part of this
  work, I want the Claude Code conventions guard extended in the same change,
  so that a later Claude Code release that stops recognizing it fails CI
  rather than an operator's screen.

## Requirements

### Functional

**R1. A stated count bound.** koto enforces a numeric upper bound on the
number of koto-owned entries (R5) in each `workflows` directory it writes
to. The bound has a stated default, documented with its reasoning. After any
koto write into a directory completes, that directory holds no more koto-owned
entries than the bound, except in the case R3 allows.

**R2. The same policy for every finished outcome.** Completed, failed and
cancelled sessions' entries fall under the same policy. An entry's status word
doesn't exempt it from the bound.

**R3. Live entries are kept.** The entry koto is writing in the current
operation is never removed by that operation, and an entry whose session the
policy can't show to be finished or stale is not removed to make room. When
more koto sessions are live in one directory than the bound allows, the
directory may hold more entries than the bound until they finish; the docs
state this case.

**R4. Oldest first.** When entries must be removed to meet the bound, finished
and stale entries go before live ones, and among those the oldest go first,
judged by the same `startTime` order the screen sorts by, or by the file's
last write when `startTime` can't be read.

**R5. Ownership.** koto removes or rewrites only files it can show it wrote:
a regular file (not a symlink or directory) whose name matches koto's entry
filename pattern and whose content carries koto's marker, an entry `id` and
`koto` block that agree with the filename. A file that fails either check,
including an unparseable `koto-*.json`, is left exactly as it was. Claude
Code's `wf_*.json` files and every other file are never deleted, rewritten,
renamed or changed in mode. A leftover temporary file from an interrupted
koto write (the `.koto-wf-*.tmp` files koto's atomic write creates) counts as
koto-written and may be removed once it is older than a stated age.

**R6. Crash-staleness, one of two outcomes.** For a koto session that stopped
without finishing, the design settles on exactly one of:

- (a) *Reconcile.* Once the session has shown no activity for a stated
  window, the next koto write into that directory changes the entry so it no
  longer reads as running on Claude Code's screen, and says in the entry's
  visible text that the session went quiet and since when. A later commit by
  the same session restores its live status.
- (b) *Accept as a limitation.* The entry keeps its last status, and koto's
  docs state the limitation, the stated way an operator tells a live koto
  entry from a stale one, and how long a stale entry can remain before the
  bound or another rule removes it.

The design records which it chose and why. Under (a) the docs also state that
reconciliation needs some koto write into the same directory after the window,
since no koto process runs between commands.

**R7. Never fail the operation.** The policy never fails, slows to a visible
degree, or changes the outcome of the koto command whose write triggered it.
A removal or rewrite that errors (a file already gone, a permission problem, a
concurrent koto process) is skipped and the command goes on, as koto's write
of the entry already does.

**R8. Tool independence.** The behavior works with koto alone. It needs no
niwa, no workspace tool and no koto-skills plugin hook, and asks nothing of a
user who runs niwa without koto.

**R9. Guard kept in step.** Any change this work makes to the entry's
screen-read shape, a new top-level field or a new top-level `status` word,
lands in the same change with Claude Code's conventions guard extended to
cover it: the shape fixture updated, a probe added for the field, and for a
status word a probe proving the recorded Claude Code release knows it, with
the guard's coverage test extended to collect status values so the next new
word is caught too. A change that adds nothing screen-read leaves the guard
alone and says so.

**R10. Documented.** The operator-facing guide for native `/workflows`
rendering, the file's field reference, and the koto-user skill's
`/workflows` section state the bound and its default, the eviction order, the
ownership rule, the R3 exception, and the crash-staleness outcome R6 chose,
including the stated window or the stated way to tell live from stale. Any
new setting appears in the CLI configuration reference.

### Non-functional

**R11. Bounded cost per write.** The policy adds at most one directory listing
per koto write into a directory, reads file contents only for entries that
are candidates for removal or rewrite, and does nothing further when the
directory is already within the bound and holds no stale candidate.

**R12. Concurrency-safe.** Two koto processes applying the policy to the same
directory at once never remove a file that isn't a valid candidate for at
least one of them, and neither reports an error for a file the other already
removed.

**R13. Testable without Claude Code.** Every requirement above can be checked
by tests that point koto at a temporary `workflows` directory, with a clock
the test controls for any time window, and no Claude Code installed.

## Acceptance Criteria

- [ ] With the bound at its default N, a test runs at least 3N koto sessions
      to completion into one temporary `workflows` directory and, after every
      session, counts the koto-owned files there: the count is never above N.
- [ ] The same test leaves the N most recently started koto entries in place
      and removed the older ones.
- [ ] A test with N completed, failed and cancelled entries in a directory at
      the bound, plus one new session, removes the oldest of them regardless
      of status word.
- [ ] A test with N+2 koto sessions live (non-finished, inside the staleness
      window) in one directory removes none of them, and the docs name this
      case.
- [ ] A test seeds the directory with a `wf_test.json`, a `koto-notours.json`
      whose content lacks koto's marker, an unparseable `koto-broken.json`, a
      symlink named `koto-<id>.json`, and an unrelated `notes.txt`, drives the
      directory well past the bound, and asserts each of those five files is
      byte-identical, same mode and same mtime afterwards.
- [ ] A test leaves a `.koto-wf-*.tmp` file older than the stated age and one
      younger than it; after a koto write the older one is gone and the
      younger one remains.
- [ ] If R6(a) is chosen: a test writes a running entry, advances the test
      clock past the stated window, triggers a koto write from another session
      into the same directory, and asserts the entry's top-level `status` is no
      longer `running` and its `summary` names the quiet period; a further
      commit by the original session restores `running`.
- [ ] If R6(a) is chosen: the same test before the window has passed leaves the
      entry's `status` as `running`.
- [ ] If R6(b) is chosen: the operator guide contains a section that names the
      limitation, gives a command or check an operator runs to tell a live koto
      entry from a stale one, and states how long a stale entry can remain.
- [ ] The design document records which of R6(a) or R6(b) was chosen, the
      window or the live-versus-stale check, and the reasoning.
- [ ] A test in which removal of a candidate fails (the file is removed by the
      test between listing and removal, or made unremovable) shows the
      triggering koto command still exits with its normal result and the
      entry it was writing is up to date.
- [ ] If the change adds a top-level field or status word to the entry file,
      the conventions guard's fixture has a probe for each, the coverage test
      collects top-level `status` values from the shape fixture, and a test run
      with an unprobed status word added to the shape fixture fails naming the
      word. If the change adds neither, the shape fixture and conventions
      fixture are unchanged.
- [ ] The operator guide, the field reference and the koto-user skill state the
      bound, its default, the eviction order, the ownership rule, the
      many-live-sessions exception and the crash-staleness outcome; any new
      setting appears in the CLI configuration reference with its default and
      environment override.
- [ ] The implementation pull request shows the bound holding across many runs
      and states the commands a reader runs to reproduce it locally.
- [ ] The implementation pull request closes koto issue #190.
- [ ] None of the tests or code paths call niwa or require the koto-skills
      plugin hook.

## Out of Scope

- **Any niwa or workspace-tool dependency.** koto users without niwa get the
  full behavior, and nothing here asks a niwa user to use koto.
- **koto's own session store.** Retention of sessions, anchors and their
  records is an existing, separate policy; this work governs only koto's files
  in Claude Code's `workflows` directories.
- **Hierarchy and fan-out entries.** Showing coordinators and delegates as
  their own `/workflows` entries is separate work.
- **A blocked bucket in the list.** Claude Code's screen decides its buckets;
  that limitation is tracked on its own.
- **The rendered smoke check and its verdict.** This work extends the
  conventions guard only where its own change needs it.
- **A daemon, background watcher or per-turn hook.** koto stays a
  command-line tool; any reconciliation happens while koto is already running
  for another reason.
- **Claude Code's own files.** Their lifecycle belongs to Claude Code.
- **The stale publish location after a Claude Code resume.** Which directory
  a resumed session writes to is tracked separately; this work governs what
  happens inside whichever directory koto writes to.

## Known Limitations

- koto acts only when it runs. With no later koto write into a directory,
  nothing there changes: a directory whose Claude Code session ended keeps
  whatever koto left in it, up to the bound.
- The screen reads files once when it opens. A change koto makes after the
  operator opened `/workflows` shows only when they reopen it.
- Without a heartbeat, a long quiet step (a human-gated state, a slow agent)
  looks the same as a dead driver. Whatever window the design picks, a live
  but quiet session can cross it; under R6(a) its next commit restores it.
- A directory shared across machines (a synced home directory) can hold
  entries for sessions this host's store has never seen; the policy can't
  tell those apart from finished ones by store lookup alone.

## Decisions and Trade-offs

- **The bound is a count, not an age.** An age cutoff alone doesn't bound a
  busy directory: a burst of sessions inside the age window still piles up.
  A count bound holds across any number of runs, which is what the operator
  sees. The design may add an age rule on top, but the count is the
  requirement.
- **Live entries are exempt rather than strictly bounded.** Removing a live
  session's entry to meet a number would make the screen lie in the other
  direction. The rare case of more live sessions than the bound is documented
  instead.
- **Ownership is name plus content, not name alone.** A file named
  `koto-*.json` that another tool or a person wrote must survive, and koto's
  session ids aren't guaranteed to be UUIDs, so a filename regex can't carry
  the whole check. The marker block every contract version writes is the
  evidence.
- **Both crash-staleness outcomes are allowed here; the design picks.** koto
  has no liveness signal beyond its log and the file's last write, so whether
  re-stamping is honest enough is a mechanism question. The requirement fixes
  what each outcome must deliver and that the choice and its reasoning are
  recorded.
- **koto issue #190 is covered in full.** Its five acceptance criteria map to
  R1 and the bound test, R6, R2, R5, and R10 with R13. The implementation
  closes it.
- **The guard rule is conditional on what changes.** The conventions guard
  doesn't check status values today, so a new status word would slip past it;
  R9 makes adding one carry its own probe and a coverage-test extension,
  while a change that writes nothing new to the screen leaves the guard
  alone.
