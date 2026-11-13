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
  koto's entries in a directory stay at or under a stated bound unless more koto
  sessions than that are live there at once, a koto session that stopped without
  finishing stops claiming to be running once a stated window has passed (or the
  docs say how to tell), and nothing koto didn't write is ever touched.
upstream: docs/briefs/BRIEF-native-workflows-retention.md
source_issue: 190
---

# PRD: bounded and honest koto entries in Claude Code's `/workflows`

## Status

Draft

Requirements for the lifecycle of the files koto writes into Claude Code's
`/workflows` directories. The brief left two calls open, the retention policy
and how crash-staleness is handled; this PRD states what either answer must
satisfy and leaves the mechanism, the numbers and the choice between the two
crash-staleness outcomes to the design.

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
  under a stated bound, with Claude Code's own entries still visible. The one
  exception is more koto sessions live in that directory at once than the
  bound allows.
- An operator who returns to a koto session that died mid-run doesn't see it
  as running once a stated window has passed, or finds in koto's docs a
  stated way to tell a live koto entry from a stale one.
- Files koto didn't write are never deleted or modified.
- The behavior works with no command run and no setting changed, and depends
  on nothing outside koto.

## User Stories

- As an operator who drives many koto workflows to completion in one Claude
  Code session, I want koto's entries in `/workflows` to stay under a fixed
  count, so that I can still find Claude Code's own workflows late in the
  day.
- As an operator upgrading koto with a directory that already holds dozens of
  old koto entries, I want the first koto write after the upgrade to bring
  that directory down to the bound, so that I don't have to clean it by hand.
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

### Terms

- **koto write**: koto writing a session's entry into a `workflows`
  directory, which it does on every committed step of that session. The
  policy runs after each one, in the directory just written.
- **Owned entry**: a file in a `workflows` directory that passes R5's
  ownership check.
- **Last write**: the entry file's modification time. koto rewrites an entry
  on every committed step of its session, so this is the session's last
  recorded activity in that directory, and it is known without reading the
  file.
- **Staleness window (W)**: a stated duration the design fixes. A last write
  exactly W ago is still live.
- **Finished entry**: an owned entry whose top-level `status` is anything but
  `running`. That covers completed, failed, cancelled, and (under R6(a))
  reconciled entries.
- **Stale entry**: an owned entry whose top-level `status` is `running` and
  whose last write is more than W ago.
- **Live entry**: an owned entry whose top-level `status` is `running` and
  whose last write is W ago or less, or lies in the future.
- **Bound (N)**, **temp-file age (A)**: stated values the design fixes.

Classification reads only the entry file itself, so it works the same for
entries whose session koto's store has already removed. Tests read N, W and A
from the named values the implementation defines rather than from literals.

### Functional

**R1. A stated count bound.** koto enforces a numeric upper bound N, at least
1, on the number of owned entries in each `workflows` directory it writes to.
N has a stated default, documented with its reasoning. Each directory is
bounded on its own: writes into one never remove entries from another. After
any koto write into a directory completes, that directory holds at most N
owned entries, except in the case R3 allows. Temporary files (R5) don't count
toward N. Whether N is a setting or a fixed value is the design's call; if it
is a setting, a value below 1 is refused with an error naming the setting.

**R2. The same policy for every finished outcome.** Completed, failed and
cancelled sessions' entries fall under the same policy. A finished entry's
status word doesn't exempt it from the bound.

**R3. Live entries are kept.** The entry koto is writing in the current
operation is never removed by that operation, even when it is the oldest, and
no live entry is removed to make room. When more live entries exist in one
directory than N allows, the directory may hold more than N owned entries
until enough of them finish or go stale; the docs state this case.

**R4. Eviction order.** When entries must be removed to meet the bound, only
finished and stale entries are candidates, and they are removed oldest first
until the directory is within N or no candidates remain. An entry's age is its
`startTime` when that parses as a timestamp, otherwise its last write; all
candidates are compared on that one key, and ties are broken by filename in
ascending byte order.

**R5. Ownership.** koto removes or rewrites only files it can show it wrote:
a regular file (not a symlink or directory) whose name matches koto's entry
filename pattern and whose content carries koto's marker: an entry `id` and
`koto` block that agree with the filename, and a string top-level `status`.
Every contract version koto has written carries that marker, so older entries
qualify. A file that fails either check, including an unparseable
`koto-*.json`, is left exactly as it was and doesn't count toward N. Claude
Code's `wf_*.json` files and every other file are never deleted, rewritten,
renamed or changed in mode. A leftover temporary file from an interrupted
koto write (the `.koto-wf-*.tmp` files koto's atomic write creates) is also
koto-written: one whose modification time is more than A ago is removed, and
one A ago or younger is left.

**R6. Crash-staleness, one of two outcomes.** For a stale entry, the design
settles on exactly one of:

- (a) *Reconcile.* The next koto write into that directory after the entry
  became stale rewrites it so its top-level `status` isn't `running` and is a
  word the conventions fixture has a probe for, proving the recorded Claude
  Code release knows it, and its `summary` says the session went quiet and
  gives the time of its last write in the same format as `startTime`. The
  entry is then a finished entry for R4. A later commit by the same session
  rewrites it as live again. Reconciliation never replaces a write made after
  it judged the entry stale: it checks the entry's last write again
  immediately before replacing it, and leaves an entry that changed.
- (b) *Accept as a limitation.* The entry keeps its last status. koto's docs
  state the limitation, a command or check an operator runs to tell a live
  koto entry from a stale one, and that a stale entry remains only until the
  bound removes it as a candidate.

W applies under both outcomes, since it also decides when a dead session's
entry becomes a removal candidate.

The design records which it chose and why. Under (a) the docs also state that
reconciliation needs some koto write into the same directory after the window,
since no koto process runs between commands.

**R7. Never fail the operation.** The policy never fails or changes the
outcome of the koto command whose write triggered it. A removal or rewrite
that errors (a file already gone, a permission problem, a concurrent koto
process) is skipped and the command goes on, as koto's write of the entry
already does. A `workflows` directory koto can't list is skipped the same way.

**R8. Tool independence.** The behavior works with koto alone. It needs no
niwa, no workspace tool and no koto-skills plugin hook, and asks nothing of a
user who runs niwa without koto.

**R9. Guard kept in step.** Any change this work makes to the entry's
screen-read shape lands in the same change with Claude Code's conventions
guard extended to cover it. A new top-level field gets a probe in the guard's
fixture and an entry in the shape fixture. A top-level `status` word koto
didn't write before gets a probe showing the recorded Claude Code release
knows it, and the guard's coverage test is extended to collect top-level
`status` values from the shape fixture, so the next new word is caught too. A
change that writes nothing new to the screen leaves both fixtures alone and
says so in its pull request.

**R10. Documented.** The operator-facing guide for native `/workflows`
rendering, the file's field reference, and the koto-user skill's
`/workflows` section each state the default value of N, the eviction order,
the ownership rule, the many-live-sessions case, and the crash-staleness
outcome R6 chose, with the value of W. Any new setting appears in the CLI
configuration reference with its default and environment override.

### Non-functional

**R11. Bounded cost per write.** The policy lists a directory at most once per
koto write into it. When the number of files matching koto's entry filename
pattern is at most N and none of them has a last write older than W, it reads
no file contents. Otherwise it reads contents only of files matching that
pattern, never of any other file.

**R12. Concurrency-safe.** Two koto processes applying the policy to the same
directory at once never remove a live entry or a file R5 protects, never
replace a newer write (R6(a)), and neither reports an error because the other
already removed a file.

**R13. Testable without Claude Code.** Every requirement above can be checked
by tests that point koto at a temporary `workflows` directory, with a clock
the test controls for every time comparison, and no Claude Code or niwa
installed.

## Acceptance Criteria

Tests set every time comparison through a clock they control and read N, W
and A from the named values the implementation defines.

Bound and order:

- [ ] A test runs 3N koto sessions to completion into one temporary
      `workflows` directory and, after every session, counts owned entries:
      the count is never above N, and at the end the N most recently started
      entries remain.
- [ ] A test with a directory at N finished entries of mixed status
      (completed, failed, cancelled) adds one session and finds the entry with
      the oldest `startTime` removed, whatever its status.
- [ ] A test seeds a directory with 3N finished entries, the oldest of them in
      the first contract version's shape, then makes one koto write: N owned
      entries remain and the first-version entry is gone.
- [ ] A test with N = 1, one finished entry, and a new session whose entry has
      the oldest `startTime` finds only the new session's entry left.
- [ ] A test with a directory over the bound holding one live entry older
      than every finished entry removes finished entries, not the live one.
- [ ] A test with N+2 live entries and three finished entries in one directory
      makes one more write: all three finished entries are gone, every live
      entry and the entry just written remain.
- [ ] A test with a stale entry and finished entries over the bound treats the
      stale entry as a candidate in `startTime` order.
- [ ] A test with candidates where one lacks a parseable `startTime` orders it
      by its last write among the others, and two candidates with equal keys
      are removed in ascending filename order.
- [ ] Staleness boundary: with the test clock set so a `running` entry's last
      write is exactly W ago it is kept as live; one second past W it is a
      candidate; with its last write in the future it is kept as live.
- [ ] A test writes into two directories, one at the bound, and finds the
      other directory's entries unchanged.

Ownership and temp files:

- [ ] A test seeds the directory with `wf_test.json`, `koto-notours.json`
      lacking koto's marker, a `koto-<id>.json` whose `id` disagrees with its
      filename, an owned-looking `koto-<id>.json` with no `status`, an
      unparseable `koto-broken.json`, a symlink named `koto-<id>.json`, a
      directory named `koto-<id>.json` and `notes.txt`, drives the directory
      well past the bound, and asserts each is present and byte-identical with
      the same mode and modification time.
- [ ] A test leaves `.koto-wf-*.tmp` files whose modification times are one
      second past A, exactly A, and younger than A; after a koto write only
      the first is gone.
- [ ] A test with N owned entries plus three `.koto-wf-*.tmp` files younger
      than A and one koto write by an existing session removes no owned entry.

Crash-staleness (whichever R6 outcome the design chose):

- [ ] Under R6(a): a test writes a running entry, sets the test clock past W,
      makes a koto write from another session into the same directory, and
      finds the entry's top-level `status` not `running`, a word with a probe
      in the conventions fixture, and its `summary` containing its last-write
      time in the same format as `startTime`.
- [ ] Under R6(a): the same test with the clock short of W leaves the entry's
      `status` as `running`, and a commit by the original session after
      reconciliation sets it back to `running`.
- [ ] Under R6(a): a test where the original session's commit lands after the
      policy judged its entry stale and before it rewrites it finds the entry
      still `running` with the commit's content.
- [ ] Under R6(a): a reconciled entry in a directory over the bound is removed
      in `startTime` order like any finished entry.
- [ ] Under R6(b): a test with a stale entry in a directory within the bound
      leaves it byte-identical after a koto write.
- [ ] Under R6(b): the operator guide has a section that names the
      limitation, gives the command an operator runs to tell a live koto entry
      from a stale one, and states that a stale entry is removed by the bound.
- [ ] The design document states which R6 outcome it chose, the values of N, W
      and A, and the reasoning for each.

Failure, cost and concurrency:

- [ ] A test in which a candidate is removed by the test after the policy
      lists the directory and before it removes the file shows the
      triggering koto command exits with its normal result, reports no error
      for that file, and its own entry is up to date.
- [ ] A test in which a candidate can't be removed (its directory made
      read-only for the removal, skipped when the test runs with privileges
      that ignore it) shows the koto command exits with its normal result and
      its own entry is up to date.
- [ ] A test pointing koto at a `workflows` path it can't list shows the koto
      command exits with its normal result.
- [ ] Through a counter the implementation exposes to tests: with at most N
      name-matching files, all with a last write within W, the policy lists
      the directory once and reads no file contents; with more than N
      name-matching files plus non-matching files present, it lists the
      directory once and reads no non-matching file.
- [ ] A test runs two policy passes over the same directory so both list it
      before either removes anything, and finds no error from either, no live
      or protected file removed, and at most N owned entries left.
- [ ] If N is a setting: a test sets it to 0 and finds the command refused
      with an error naming the setting.

Guard, docs and independence:

- [ ] If the change adds a top-level field, the conventions fixture has a
      probe for it and the shape fixture shows it; if it adds a top-level
      `status` word, the fixture has a probe for that word, the coverage test
      collects top-level `status` values, and a test run with an unprobed
      `status` word added to the shape fixture fails naming the word. If it
      adds neither, both fixtures are unchanged and the pull request says so.
- [ ] Each of the operator guide, the field reference and the koto-user
      skill's `/workflows` section states the default value of N, the value of
      W, the eviction order, the ownership rule, the many-live-sessions case
      and the R6 outcome chosen.
- [ ] Any new setting appears in the CLI configuration reference with its
      default and its environment override.
- [ ] A test runs the policy with no koto setting changed from its default
      and finds the bound enforced.
- [ ] The implementation pull request names the test that runs 3N sessions
      and the exact command a reader runs to reproduce it locally, and that
      command passes.
- [ ] The implementation pull request closes koto issue #190.
- [ ] The new tests pass with niwa absent from `PATH`, no Claude Code
      installed and no koto-skills plugin present, and koto's dependency
      manifest gains no niwa dependency.

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
  looks the same as a dead driver once W has passed. Under R6(a) its next
  commit restores it; under either outcome a stale entry can be removed by
  the bound while its session is still alive, after which its next commit
  writes it again.
- Classification reads only the entry file. A directory shared across
  machines (a synced home directory) is handled by the same rule, so a live
  session on another machine whose writes don't sync within W reads as stale.

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
- **Classification reads the entry file, not koto's store.** Terminal
  sessions are removed from the store once they finish, while their entry
  already records `completed` or `failed`; a store lookup would need a scan
  of every session and would still say nothing about a killed one. The
  entry's own status and last write are enough and cost nothing extra.
- **A file koto can't prove it wrote is left forever.** An unparseable or
  marker-less `koto-*.json` never counts toward N and is never removed,
  because removing a file koto didn't write is the one failure the policy
  must not have; such files are rare and cost a reader one line in the list.
- **Ownership is name plus content, not name alone.** A file named
  `koto-*.json` that another tool or a person wrote must survive, and koto's
  session ids aren't guaranteed to be UUIDs, so a filename pattern can't carry
  the whole check. The marker every contract version writes is the evidence.
- **Both crash-staleness outcomes are allowed here; the design picks.** koto
  has no liveness signal beyond its log and the entry's last write, so
  whether re-stamping is honest enough is a mechanism question. The
  requirement fixes what each outcome must deliver and that the choice and
  its reasoning are recorded. W exists under both, because it also decides
  when a dead session's entry becomes a removal candidate.
- **koto issue #190 is covered in full.** Its five acceptance criteria map to
  R1 with the 3N test, R6, R2, R5, and R10 with R13. The implementation
  closes it.
- **The guard rule is conditional on what changes.** The conventions guard
  doesn't check status values today, so a new status word would slip past it;
  R9 makes adding one carry its own probe and a coverage-test extension,
  while a change that writes nothing new to the screen leaves the guard
  alone.
