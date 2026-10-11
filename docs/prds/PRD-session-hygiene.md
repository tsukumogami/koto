---
schema: prd/v1
status: Accepted
problem: |
  koto's session store under ~/.koto/sessions holds every kind of state its
  users keep, about a thousand session directories on one workstation, and
  nothing finds it by where it ran or clears it as a set. A session removed
  at its terminal tick answers `not found` to `koto status`, the same as one
  that was lost, and the run journal and the bucket's migrated markers grow
  without bound.
goals: |
  Operators, teardowns and agents can list the sessions a directory's work
  left, remove the finished ones as a previewed set that never includes a
  session something still needs, and learn how a finished session ended
  after koto removed it. The run journal and migrated markers have bounded,
  stated retention that loses nothing a reader or a live host is owed.
upstream: docs/briefs/BRIEF-session-hygiene.md
source_issue: 308
---

# PRD: session-hygiene

## Status

Accepted

## Problem Statement

koto keeps a directory per session under `~/.koto/sessions` (or under the
base a `KOTO_SESSIONS_BASE` redirect names). It was built for workflow runs
that remove themselves at their terminal tick, and it now holds everything:
skill sessions kept with `--no-cleanup` so a later run can read them,
coordinator records, finished children their parents read, and the fixture
sessions test suites create. On 2026-10-08 one workstation's store held 983
session directories.

The store can't be managed at that size:

- **Attribution.** The only link from a session to the work that made it is
  the execution directory in its state file's header. `koto session list`
  doesn't print it or filter on it, so a teardown has to parse every header
  itself (koto#308). One teardown on 2026-10-06 left 87 sessions behind for
  this reason, 72 of them test fixtures.
- **Removal as a set.** `koto session cleanup` takes one name and
  `koto workspace prune` takes one finished root and removes its
  descendants whatever their state. No verb removes "the finished ones"
  (koto#162), and none states which finished sessions must stay: a child a
  parent hasn't read, the parent and children a cancelled chain leaves for
  its resumed run, a coordinator's record, a session another live session
  points at.
- **Vanishing at terminal.** A session that reaches a terminal state koto
  doesn't keep is removed on that tick. The `koto next` response says so in
  its `retention` field, but every later `koto status` answers
  `workflow '<name>' not found` with exit 2, the same answer a lost or
  never-created session gets, and the outcome is gone (koto#234).
- **Unbounded derived files.** The run journal gains a line for every state
  any session enters and is never rotated. Two readers on the host follow
  it: one by byte offset, shipping it off the host, and one archiving the
  whole file and keeping a shrunk or changed file as a new version. A
  careless rotation loses or repeats their lines. Every
  `koto session import` leaves a `migrated.json` marker beside the source in
  the bucket, and nothing removes one.

## Goals

- A person or a script finds every session a piece of work left, by the
  directory it ran in, and sees which are finished, without reading session
  files.
- Clearing finished sessions is safe by construction: previewable, exact
  when asked to be, and never touching a session something still needs,
  with each protection stated.
- A finished session stays answerable: `koto status` tells a clean finish
  from a missing session, with the outcome.
- The run journal stays within a stated bound, and its readers never lose
  or repeat a line across a rotation.
- Migrated markers can be cleared once a stated rule says they're safe.

## Definitions

- **Finished.** A session is finished when it stands in a state its
  template marks terminal (failure terminals included), or when its log
  records a cancellation that no later transition or rewind follows. Every
  other readable session is running. A session whose log or template can't
  be read is unreadable, and the rules below treat it as running.
- **Finished at.** For a session, the timestamp of the last event in its
  log. For a finished record (R12), the time the record was written.
- **Request leg.** One named part of a `koto request` that a session can be
  bound to; the leg waits for its result until the bound session's terminal
  result is recorded on it (`docs/designs/current/DESIGN-request-lifecycle.md`).
- **Reference.** Session A references session B when A's header names B as
  its parent (`parent_workflow`), or when A's context holds the key
  `chain/parent` whose whole content is B's name. A `chain/parent` value
  that isn't exactly one valid session name is malformed.

## User Stories

These carry the BRIEF's journeys forward at requirements altitude.

1. As a coordinator's teardown, I want to list the sessions whose execution
   directory is under my worker's instance (workflow runs and test fixtures
   alike) with each one's state, and remove the finished ones, so that no
   finished session of that work remains unless a protection keeps it.
2. As an operator, I want to read a preview once and then remove exactly
   what it listed, so that I trust the protections before any removal.
3. As an operator, I want a scheduled prune that removes whatever is
   finished and unprotected at the time it runs, with every protection still
   applied, so that the store stays bounded without my attention.
4. As an agent whose `koto next` landed on a terminal, I want `koto status`
   to say the session finished, in which state and with which result, after
   koto removed it, so that I can tell a clean finish from a lost record.
5. As an operator running a tool that ships the run journal off the host by
   byte offset, I want a rotation to leave the file I was reading where I
   can find it, so that my next pass finishes it and starts the new journal
   at its first byte with no line lost or sent twice.
6. As an operator running a tool that archives the journal file by file, I
   want to find every rotated file by name and in order, so that each file
   is archived once and complete.
7. As an operator who imported sessions from an old host, I want to see
   which markers under the old workspace's bucket prefix are safe to remove
   and why, and remove them, so that the old workspace's prefix stops
   holding copies of sessions that moved.

## Requirements

### Listing

- **R1.** `koto session list` accepts an execution-directory filter. A
  session matches when its recorded execution directory equals the given
  directory or lies under it, compared by whole path components (`/a/b`
  matches `/a/b` and `/a/b/c`, not `/a/bc`). The given directory is
  canonicalized (symlinks resolved) when it resolves on this host and used
  as given otherwise; the recorded directory is compared as recorded, since
  koto records it canonicalized. Trailing slashes are ignored on both sides.
- **R2.** Every listed row carries two new fields read from the header: the
  execution directory and the session id. Rows listed with the R1 filter, or
  with `--state`, also carry `state` (the current state's name,
  absent when unreadable), `status` (`running`, `terminal`, `cancelled` or
  `unreadable`), `failure` (true for a failure terminal), and `finished_at`.
- **R3.** A row the header can't supply (a cloud placeholder for a session
  that exists only in the bucket) never matches the R1 filter. With the
  filter, the output reports how many such rows it left out in a named
  field.
- **R4.** With no new flag, `koto session list` prints every field it prints
  today with the same meaning; the R2 header fields are added, nothing is
  removed or renamed.

### Prune

- **R5.** A new verb, `koto session prune`, removes finished sessions as a
  set. Without an apply flag it previews: it prints, as JSON, the sessions it
  would remove and every session it considered and kept, each kept one with
  the rule that kept it, plus a plan token, and removes nothing.
- **R6.** The plan token is computed from exactly the names and session ids
  in the would-remove list, so a change in any other session doesn't change
  it. Applying with a token recomputes the set and removes it only when the
  recomputed token matches; on a mismatch it removes nothing, prints the
  sessions added to and dropped from the set, and exits with a code distinct
  from success and from a usage error.
- **R7.** Applying without a token (the scheduled mode) recomputes the set
  and removes it, printing exactly what it removed. Both apply modes enforce
  every R8 rule identically; the token mode adds only the equality check.
- **R8.** The prune considers only sessions in the local store it runs
  against (the default store, or the base a `KOTO_SESSIONS_BASE` redirect
  names), filtered by the R1 execution-directory filter and an age filter
  (finished at least a given duration ago) when given, both applying
  together. A session the filters exclude is not considered and doesn't
  appear in the preview, though it still counts as a referencing or
  referenced session for rules b and c. It removes a session only when it is
  finished and no rule below keeps it:
  - a. a running or unreadable session is kept;
  - b. a session is kept while a session that references it (Definitions)
    is running, so a parent with live children keeps, and a cancelled
    chain's parent stays for its resumed run;
  - c. a finished session is kept while a session it references is running,
    so a parent never loses a child it hasn't read;
  - d. a session with a malformed `chain/parent` value is kept;
  - e. a session bound to a request leg that still waits for its result is
    kept, and so is one whose leg can't be read;
  - f. a session whose name begins `coordinate-` is kept, since a
    coordinator's session is its record.

  Every kept session's preview entry names exactly one rule, the first that
  applies in the order above.
- **R9.** Before removing each session the apply holds that session's state
  lock, re-checks that it is finished, has the same session id, and is still
  unprotected, and removes it under the lock. Before removing each finished
  record it re-checks that the record has the same session id and time. An
  entry that fails the re-check is skipped and listed under a `skipped`
  field with the reason; skips don't change the exit code of a run that
  otherwise succeeded. The prune writes no finished records.
- **R10.** Under the cloud backend the prune removes a pruned session's
  remote objects the way `koto session cleanup` does (every object under the
  session's prefix except a migration marker), and a session that exists
  only in the bucket is never considered.
- **R11.** A prune leaves the run journal byte-identical, and each pruned
  session's `run-journal.json` sidecar goes with its directory.
### Terminal signal

- **R12.** When koto removes a finished session itself (at its own terminal
  tick, or when a parent removed at its terminal sweeps its kept children,
  which can include a cancelled child),
  it first writes a finished record for that session in the store it
  belongs to: name, session id, execution directory, terminal state, status
  (`terminal` or `cancelled`), failure flag, the result recorded for that
  arrival, and the time. A record that can't be written costs one warning on
  stderr and the removal proceeds. A record is at most 64 KiB; a result that
  would exceed that is replaced by its status and summary with a
  `truncated` flag.
- **R13.** `koto status <name>` for a name with no session but a finished
  record exits 0 with `current_state`, `is_terminal: true`, `result`,
  `removed: true` and `finished_at`. A name with neither a session nor a
  record exits 2 with `not found`, as today.
- **R14.** Those fields have the same names and meanings as in the answer
  `koto status` already gives a kept terminal session, so a caller reads
  both the same way.
- **R15.** Creating a session under a name deletes that name's finished
  record. While a session exists, `koto status` reports the session and
  ignores any record. `koto session list` and `koto workflows` never list a
  finished record as a session.
- **R16.** The prune considers finished records too, under the same
  execution-directory and age filters, and removes every one that matches;
  records have no protections. Records appear in the preview and the plan
  token as their own entries.
- **R17.** Within the terminal tick, the journal's terminal or cancelled
  record is written before any removal, and the finished record is written
  after it and before the session directory is removed.

### Run journal retention

- **R18.** Every append to the run journal (`~/.koto/_run_journal.jsonl`,
  and `<base>/_run_journal.jsonl` for a redirected store) checks the file's
  size; when it is at or over the size bound, the journal is renamed to a
  rotated name and the record goes to a fresh file. The journal is never
  truncated or rewritten in place, and every record kind is treated alike.
- **R19.** Rotated files are named so a reader can list every rotated file
  and order them oldest to newest by name alone, without reading content;
  the active file is always newest. A rename keeps a file's inode, so a
  reader identifies the file it is reading by inode. After a rotation it
  finds that inode among the rotated files, reads it from its offset to the
  end, then reads each newer rotated file from byte 0 in name order, then
  the active file from byte 0. The rule is documented in
  `docs/reference/run-journal.md`. A rename that fails costs one warning on
  stderr and the record is appended to the unrotated file.
- **R20.** No record is written to a file after it was renamed: a writer
  that raced the rotation writes to the new file.
- **R21.** A reader may record its position in a file of its own in a
  documented directory beside the journal: which journal file, and the byte
  offset in it it has read up to. A whole-file archiver records the end of
  the last file it archived. Retention decisions read only these position
  files and the journal files' names and metadata, never journal records.
- **R22.** Rotated files are removed only during a rotation, only when older
  than the retention bound (measured from the file's last modification, which
  is its rotation), and only when no recorded position still needs them: a
  position needs the file it names and every newer one, except that a
  position at or past a rotated file's end needs only the newer ones. A
  position file not modified for longer than the stale bound, or one that
  can't be read or parsed, protects nothing, and koto prints a warning naming
  it. With no positions recorded, the retention bound alone applies.
- **R23.** Defaults: size bound 64 MiB, retention bound 30 days, stale bound
  90 days. Each is configurable through `koto config`; a size bound of 0
  turns rotation off.
- **R24.** An offset reader that stopped mid-file and follows the documented
  rule finishes the old file and reads the new one from byte 0 without
  losing or repeating a line, across a rotation done by an ordinary koto
  command.

### Migrated markers

- **R25.** `koto session prune --migrated-from <workspace-path>`, under the
  cloud backend, previews the migration markers under that workspace's
  bucket prefix: each marker's session name, target (session and
  workspace), age (from the `migrated_at` it records), target status
  (`migrated` when the target has its own marker, else `present` when the
  target's state file is there, else `absent`, or `unknown` when the bucket
  can't be checked), and whether it is removable under R26 and why. Under
  the local backend the flag is refused as a usage error.
- **R26.** A marker is removable when (a) it is older than the marker bound
  (default 30 days, configurable), or (b) its target is `present` and the
  caller passes a flag stating the source host is retired. Every other
  marker, including one whose body can't be read, is kept.
- **R27.** Removing a marker removes every object under the source session's
  prefix, the marker last, so a failure part way leaves the marker refusing
  as before and a retry finishes the removal.
- **R28.** Marker removal uses the R5 to R7 preview, token and apply
  contract, with its own token; one invocation handles either sessions or
  markers, never both.
- **R29.** A bucket listing of another workspace's sessions before an import
  (koto#343) is not covered: the marker preview lists markers only.

### Non-functional

- **R30.** On a store of 1,000 sessions created by koto, each with a log of
  at least 20 events, `koto session list --execution-dir` with state and a
  full `koto session prune` preview each finish in under 2 seconds on the
  development machine; the measurement is recorded in the pull request that
  adds them.
- **R31.** Every test of a removing verb runs against a scratch HOME or a
  redirected store, never the real `~/.koto`. Cloud behavior is tested
  against koto's fake S3 harness, and the docs and the pull request say so.
- **R32.** Each new verb and flag has an automated end-to-end test that
  creates its sessions with real koto commands and runs the real command.

## Acceptance Criteria

Listing:

- [ ] With sessions anchored at `/x/a/b`, `/x/a/b/c` and `/x/a/bc`,
  `koto session list --execution-dir /x/a/b` returns the first two and not
  the third; a trailing slash on the argument gives the same result.
- [ ] A filter argument that is a symlink to `/x/a/b` gives the same result
  as `/x/a/b`; a filter argument that doesn't exist on the host is matched
  as given.
- [ ] Filtered rows carry `execution_dir`, `session_id`, `state`, `status`,
  `failure` and `finished_at`; one test each produces `running`, `terminal`
  (success and failure), `cancelled` and `unreadable` (a corrupted log).
- [ ] `koto session list` with no new flag prints, for an unchanged store,
  every field the previous release printed with equal values, plus
  `execution_dir` and `session_id`.
- [ ] Under the fake S3 harness, a bucket-only session is absent from
  filtered output and counted in the left-out field.

Prune:

- [ ] A preview over a store with removable and protected sessions removes
  nothing, lists each removable session, lists each kept one with its rule,
  and prints a plan token.
- [ ] Applying that token unchanged removes exactly the listed sessions and
  records.
- [ ] Applying it after one more session finished removes nothing, names
  the added session, and exits with the stale-plan code; a malformed token
  exits with the usage-error code.
- [ ] Applying without a token removes the recomputed set and prints it.
- [ ] The token is unchanged when a session outside the would-remove list
  changes (a running session advances), and a session matching two rules
  is reported with the first in R8's order.
- [ ] A session excluded by a filter is absent from the preview, and a
  running session excluded by the filter still keeps the finished session
  it references.
- [ ] Each rule keeps its session, named in the preview: a running session
  (a), an unreadable one (a), a finished parent with a running child (b), a
  finished parent whose cancelled chain left running children (b), a
  finished child of a running parent by header (c), a finished session whose
  `chain/parent` names a running session (c), a malformed `chain/parent`
  (d), a waiting leg (e), an unreadable leg (e), and a `coordinate-` session
  (f).
- [ ] With the age filter, a session finished just inside the duration is
  kept out of the set and one just past it is in it; with the execution
  directory and age filters together, only sessions matching both are in it.
- [ ] A session rewound out of its terminal between the preview and the
  apply, and a session removed and recreated under the same name, are each
  skipped and listed under `skipped`, and the run exits 0.
- [ ] An empty set previews and applies successfully, removing nothing.
- [ ] After a prune, the run journal is byte-identical and the pruned
  sessions' directories, sidecars included, are gone.
- [ ] Under the fake S3 harness, a pruned session's objects are gone from
  the bucket except a migration marker, and a bucket-only session is never
  in the set.

Terminal signal:

- [ ] After `koto next` removes a session at a success terminal, the store
  holds a finished record with every R12 field, and `koto status <name>`
  exits 0 with `current_state`, `is_terminal: true`, `result`,
  `removed: true` and `finished_at`.
- [ ] A session kept at a failure terminal, or with `--no-cleanup`, writes no
  record, and `koto status` answers from the session. `koto cancel
  --cleanup` and `koto session cleanup` write no record either: an
  operator's removal is not koto's.
- [ ] A kept terminal session's `koto status` and the removed-session answer
  for an identical run carry equal `current_state`, `is_terminal` and
  `result` values.
- [ ] `koto status` of a never-created name exits 2 with `not found`, and so
  does a name whose record the prune removed.
- [ ] `koto init` under a name with a record deletes the record, and
  `koto status` reports the new session.
- [ ] A kept child swept by its parent's terminal removal gets a record,
  with `status: cancelled` when the child was cancelled.
- [ ] A result larger than the record cap is stored as its status and
  summary with `truncated: true`, and the record stays within 64 KiB.
- [ ] `koto session list` and `koto workflows` never show a finished record
  as a session; the prune lists matching records as their own entries,
  removes them with no protections, and skips a record replaced between
  preview and apply.
- [ ] A record write made to fail (read-only record directory) prints one
  warning and the session is still removed.
- [ ] The run journal's `terminal` line for a removed session precedes the
  record's write time, and the record exists before the session directory
  is gone (checked by a test hook between the two steps).

Run journal:

- [ ] With the size bound set low, a koto command that crosses it leaves the
  old content under a rotated name that follows the documented rule and a
  fresh `_run_journal.jsonl`; three rotations order correctly by name,
  including past a ninth and tenth rotation.
- [ ] Eight concurrent koto processes writing across a rotation produce
  every expected line exactly once, none torn, none in a rotated file after
  its rename.
- [ ] The R24 offset-reader test passes.
- [ ] A rotated file older than the retention bound is removed at the next
  rotation; one inside the bound is kept; one a fresh recorded position
  points into is kept past the bound; with two readers, the lagging one
  keeps its file; a position older than the stale bound no longer keeps its
  file and the rotation prints a warning naming it.
- [ ] Setting each bound with `koto config set` changes the behavior above;
  a size bound of 0 never rotates; `koto config get` of each key with
  nothing set prints the R23 default.
- [ ] A journal one byte under the size bound doesn't rotate on the next
  append; one at the bound does.
- [ ] An unparseable position file protects nothing and its name appears in
  the rotation's warning.
- [ ] A journal whose lines are not valid JSON rotates and is retained
  exactly as one with valid lines (retention never parses records).
- [ ] With the rename made to fail, the append lands in the unrotated file
  and one warning is printed.
- [ ] No journal record kind is added or changed.

Migrated markers (fake S3 harness):

- [ ] The marker preview lists each marker under the source prefix with its
  target, age, target status and removability; the four target statuses are
  each produced by a test.
- [ ] A marker older than the bound is removable; a marker inside the bound
  with a `present` target is removable only with the retired-source flag; a
  marker with a `migrated`, `absent` or `unknown` target inside the bound is
  kept even with the flag; an unreadable marker is kept.
- [ ] Applying removes every object under the source session's prefix,
  marker last; with a delete failure injected part way, the marker is still
  there and the old name still refuses with `session_migrated`, and a retry
  completes the removal.
- [ ] The marker token behaves as the session token does: unchanged applies,
  stale refuses with the stale-plan code.
- [ ] `--migrated-from` under the local backend exits with the usage-error
  code; the marker bound's default is 30 days.

Documentation and verification:

- [ ] `docs/workspace-layout.md` names `koto session prune`, the finished
  record's location, and the journal's rotated-file rule and three defaults;
  `docs/reference/run-journal.md` states the naming rule, the position-file
  directory and format, and the three defaults; the koto-user skill names
  `koto session prune`, `--execution-dir` and the removed-session status
  answer.
- [ ] The R30 measurement is in the pull request body that adds the verbs,
  under 2 seconds for each.
- [ ] Every test that removes sessions sets a scratch HOME or
  `KOTO_SESSIONS_BASE`.

## Out of Scope

- New run-journal record kinds, or changes to existing ones.
- The human-readable session view, and the layout of a coordinator's own
  session.
- A bucket listing of another workspace's sessions before an import
  (koto#343), per R29.
- Changing which terminals koto removes at their tick.
- Retention for the request store, the decider ledger, the terminal index,
  or the wake files.
- Rotating, pruning or recording anything on another host; the journal, the
  finished records and the reader positions are host-local.
- Changes to `koto session cleanup` and `koto workspace prune`.
- Pruning sessions under another workspace's bucket prefix, beyond the
  marker removal of R25 to R28.

## Known Limitations

- A reader that never records a position is protected by the retention
  bound alone; one offline longer than that loses the files removed while
  it was away.
- A reader whose position goes stale for longer than the stale bound stops
  protecting its files; koto warns, and removal proceeds.
- Removing a marker by age relies on the stopped-source rule koto already
  documents for import: a source host revived after the bound could advance
  its old copy again.
- Finished records are kept until a prune removes them or the name is
  reused.

## Decisions and Trade-offs

- **Preview by default, two apply modes.** The token mode is for a person
  who read the preview; the scheduled mode is for cron. Both apply the same
  protections, so the scheduled mode trades only the exact-list guarantee,
  not safety.
- **The prune keeps sessions it can't judge.** Unreadable sessions,
  malformed references and unreadable legs keep their session. The cost is a
  session that lingers until fixed; the alternative costs a record someone
  needed.
- **A finished record instead of keeping finished sessions.** Keeping every
  finished session until a prune would answer koto#234 by changing what the
  terminal tick does. A record keeps today's removal and makes it visible.
- **Marker removal: age, or a present target plus a retired source.** A
  marker's job is to stop a stale source host from advancing its copy. The
  newer session being present proves the session survived the move, not
  that the old host is gone, so that branch needs the caller to state it.
- **Reader positions are written by the readers.** koto reads only the
  positions, never the journal's records, so retention never depends on
  journal content and nothing in the session store depends on the journal.
- **koto#343 stays separate.** The marker preview lists one kind of object
  under a source prefix; a pre-import listing of sessions is a different
  question with different callers.

## Downstream Artifacts

- `docs/designs/DESIGN-session-hygiene.md` (planned): how each verb, the
  finished record, the rotation and the marker rule are built.
